//! Bounded bins for contribution reductions (plan §9.4, §11.2, §11.3).
//!
//! A reduction over cells, such as uptake demand or deposits, first groups
//! its packets by the cell each one touches. [`Bins`] does this with a
//! stable counting sort:
//! 1. count each bucket's items;
//! 2. take the exclusive prefix sum of the counts;
//! 3. scatter the item indices in the order they were given.
//!
//! The result is compressed sparse rows: `offsets` (one per bucket, plus
//! the total) and `order` (item indices, bucket by bucket). Items given in
//! organism identity order come out in ascending bucket order, and in
//! identity order within each bucket. That is the qualified deterministic
//! route's order, and every per-cell sum in this crate runs in it.
//!
//! The work is `O(buckets + items)` however the items cluster: one hotspot
//! cell holding every packet costs the same as a spread population, and
//! uses no scratch beyond what the bins declared.
//!
//! The bins have a declared capacity (§9.4: scratch capacity is part of
//! admission), which [`Bins::with_capacity`] admits against a
//! [`BufferLimits`] and allocates once. A grouping with more buckets or
//! items than declared is refused and leaves the bins empty, so steady-state
//! ticks never allocate. [`Bins::new`] declares only the index range and
//! grows on demand, for callers outside a world's preflight. Indices are
//! `u32`, the width a device buffer holds them in.

use std::ops::Range;

use flife_core::CoreError;
use flife_core::memory::capacity_bytes;

use crate::footprint::BufferLimits;

/// The bytes of one index.
const INDEX_BYTES: u64 = 4;

/// Grouped item indices, reusable from grouping to grouping.
#[derive(Debug, Clone)]
pub struct Bins {
    name: &'static str,
    buckets_limit: usize,
    items_limit: usize,
    /// `offsets[b]..offsets[b + 1]` are bucket `b`'s positions in `order`.
    offsets: Vec<u32>,
    /// Item indices, bucket by bucket, each bucket in the order given.
    order: Vec<u32>,
    largest: usize,
}

impl Bins {
    /// Bytes its offsets and order hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.offsets).saturating_add(capacity_bytes(&self.order))
    }

    /// Bins named `name` for at most `buckets` buckets and `items` items,
    /// allocated now. `name` labels refusals.
    ///
    /// # Errors
    /// - [`CoreError::BudgetExhausted`] when either index buffer exceeds
    ///   `limits`, or `items` exceeds the `u32` index range.
    /// - [`CoreError::Overflow`] if a byte count overflows `u64`.
    pub fn with_capacity(
        name: &'static str,
        buckets: u64,
        items: u64,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        let index_range = u64::from(u32::MAX);
        if items > index_range || buckets > index_range {
            return Err(CoreError::BudgetExhausted {
                budget: name,
                required: items.max(buckets),
                available: index_range,
            });
        }
        let overflow = || CoreError::Overflow { quantity: name };
        let offset_bytes = buckets
            .checked_add(1)
            .and_then(|count| count.checked_mul(INDEX_BYTES))
            .ok_or_else(overflow)?;
        let order_bytes = items.checked_mul(INDEX_BYTES).ok_or_else(overflow)?;
        limits.admit(name, offset_bytes)?;
        limits.admit(name, order_bytes)?;
        let buckets_limit = usize::try_from(buckets).map_err(|_| overflow())?;
        let items_limit = usize::try_from(items).map_err(|_| overflow())?;
        Ok(Self {
            name,
            buckets_limit,
            items_limit,
            offsets: Vec::with_capacity(buckets_limit + 1),
            order: Vec::with_capacity(items_limit),
            largest: 0,
        })
    }

    /// Bins named `name` that grow on demand within the `u32` index range,
    /// for a caller that declares no capacity.
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        let range = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
        Self {
            name,
            buckets_limit: range,
            items_limit: range,
            offsets: Vec::new(),
            order: Vec::new(),
            largest: 0,
        }
    }

    /// Group items `0..items` into `buckets` buckets by `bucket_of`, keeping
    /// the given order within each bucket. `bucket_of` is called twice per
    /// item and must return the same bucket both times.
    ///
    /// # Errors
    /// - [`CoreError::BudgetExhausted`] for more buckets or items than the
    ///   bins declared.
    /// - [`CoreError::LayoutMismatch`] for an item whose bucket is out of
    ///   range.
    ///
    /// On error the bins are left empty.
    pub fn group(
        &mut self,
        buckets: usize,
        items: usize,
        bucket_of: impl Fn(usize) -> usize,
    ) -> Result<(), CoreError> {
        self.clear();
        let result = self.group_into(buckets, items, bucket_of);
        if result.is_err() {
            self.clear();
        }
        result
    }

    fn group_into(
        &mut self,
        buckets: usize,
        items: usize,
        bucket_of: impl Fn(usize) -> usize,
    ) -> Result<(), CoreError> {
        for (count, limit) in [(buckets, self.buckets_limit), (items, self.items_limit)] {
            if count > limit {
                return Err(CoreError::BudgetExhausted {
                    budget: self.name,
                    required: u64::try_from(count).unwrap_or(u64::MAX),
                    available: u64::try_from(limit).unwrap_or(u64::MAX),
                });
            }
        }
        let outside = |item: usize, bucket: usize| CoreError::LayoutMismatch {
            buffer: self.name,
            detail: format!("item {item} names bucket {bucket} of {buckets}"),
        };
        // 1. Count: bucket b's count goes in `offsets[b + 1]`. Both limits are
        //    within the u32 index range, so no count or sum overflows.
        self.offsets.resize(buckets + 1, 0);
        for item in 0..items {
            let bucket = bucket_of(item);
            match self.offsets.get_mut(bucket + 1) {
                Some(count) if bucket < buckets => *count += 1,
                _ => return Err(outside(item, bucket)),
            }
        }
        // 2. Exclusive prefix sum: `offsets[b]` becomes bucket b's start and
        //    `offsets[buckets]` the total.
        let mut largest = 0;
        for bucket in 0..buckets {
            let count = self.offsets[bucket + 1];
            largest = largest.max(count);
            self.offsets[bucket + 1] = self.offsets[bucket] + count;
        }
        // 3. Scatter in the given order, using `offsets[b]` as bucket b's
        //    cursor; afterwards it holds bucket b's end, the next one's start.
        self.order.resize(items, 0);
        for item in 0..items {
            let bucket = bucket_of(item);
            let cursor = match self.offsets.get_mut(bucket) {
                Some(cursor) if bucket < buckets => cursor,
                _ => return Err(outside(item, bucket)),
            };
            let at = *cursor as usize;
            *cursor += 1;
            match self.order.get_mut(at) {
                // `item < items ≤ u32::MAX`.
                Some(slot) => *slot = item as u32,
                None => return Err(outside(item, bucket)),
            }
        }
        // Restore the starts: bucket b starts where bucket b − 1 ends.
        for bucket in (1..buckets).rev() {
            self.offsets[bucket] = self.offsets[bucket - 1];
        }
        if let Some(first) = self.offsets.first_mut() {
            *first = 0;
        }
        self.largest = largest as usize;
        Ok(())
    }

    /// Empty the bins, keeping their allocation.
    pub fn clear(&mut self) {
        self.offsets.clear();
        self.order.clear();
        self.largest = 0;
    }

    /// The number of buckets of the last grouping.
    #[must_use]
    pub fn buckets(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Every grouped item index, bucket by bucket.
    #[must_use]
    pub fn order(&self) -> &[u32] {
        &self.order
    }

    /// Bucket `bucket`'s positions in [`Bins::order`]; empty for an empty
    /// or out-of-range bucket.
    #[must_use]
    pub fn range(&self, bucket: usize) -> Range<usize> {
        match (self.offsets.get(bucket), self.offsets.get(bucket + 1)) {
            (Some(&start), Some(&end)) => start as usize..end as usize,
            _ => 0..0,
        }
    }

    /// Bucket `bucket`'s item indices, in the order they were given.
    #[must_use]
    pub fn bucket(&self, bucket: usize) -> &[u32] {
        self.order.get(self.range(bucket)).unwrap_or(&[])
    }

    /// Each nonempty bucket with its positions in [`Bins::order`], in
    /// ascending bucket order.
    pub fn occupied(&self) -> impl Iterator<Item = (usize, Range<usize>)> + '_ {
        self.offsets
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0] < pair[1])
            .map(|(bucket, pair)| (bucket, pair[0] as usize..pair[1] as usize))
    }

    /// The most items in one bucket of the last grouping: the hotspot.
    #[must_use]
    pub const fn largest(&self) -> usize {
        self.largest
    }

    /// The declared limits: buckets and items.
    #[must_use]
    pub const fn limits(&self) -> (usize, usize) {
        (self.buckets_limit, self.items_limit)
    }

    /// The allocated capacity of the two index buffers, in entries.
    #[must_use]
    pub fn allocated(&self) -> (usize, usize) {
        (self.offsets.capacity(), self.order.capacity())
    }
}
