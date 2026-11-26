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
    /// Nonempty bucket indices in strictly ascending order, populated by
    /// [`Bins::group`] so sparse reductions avoid scanning empty buckets.
    occupied: Vec<u32>,
    largest: usize,
}

impl Bins {
    /// Bytes its offsets, order and occupied bucket list hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.offsets)
            .saturating_add(capacity_bytes(&self.order))
            .saturating_add(capacity_bytes(&self.occupied))
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
        let occupied_bytes = buckets
            .min(items)
            .checked_mul(INDEX_BYTES)
            .ok_or_else(overflow)?;
        limits.admit(name, offset_bytes)?;
        limits.admit(name, order_bytes)?;
        limits.admit(name, occupied_bytes)?;
        let buckets_limit = usize::try_from(buckets).map_err(|_| overflow())?;
        let items_limit = usize::try_from(items).map_err(|_| overflow())?;
        let occupied_limit = buckets_limit.min(items_limit);
        Ok(Self {
            name,
            buckets_limit,
            items_limit,
            offsets: Vec::with_capacity(buckets_limit + 1),
            order: Vec::with_capacity(items_limit),
            occupied: Vec::with_capacity(occupied_limit),
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
            occupied: Vec::new(),
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

    /// Parallel, L1-cache-striped counterpart to [`Bins::group`] using up to
    /// `workers` threads. Produces bit-identical `offsets`, `order`,
    /// `occupied`, and `largest` to [`Bins::group`] (preserving ascending item
    /// order within every bucket and exact lowest-index error reporting).
    ///
    /// Below `PARALLEL_ITEMS_MIN` (small worlds <= 4k organisms) or when
    /// `workers <= 1`, delegates directly to [`Bins::group`] with zero
    /// transient allocation.
    ///
    /// # Errors
    /// As [`Bins::group`], plus [`CoreError::AllocationFailed`] or
    /// [`CoreError::StateConflict`] if the parallel scratch or worker pool fails.
    pub fn group_on(
        &mut self,
        buckets: usize,
        items: usize,
        workers: usize,
        bucket_of: impl Fn(usize) -> usize + Sync,
    ) -> Result<(), CoreError> {
        const PARALLEL_ITEMS_MIN: usize = 24_576;
        if workers <= 1 || items < PARALLEL_ITEMS_MIN || buckets < 256 {
            return self.group(buckets, items, bucket_of);
        }
        self.clear();
        let result = self.group_into_parallel(buckets, items, workers, &bucket_of);
        if result.is_err() {
            self.clear();
        }
        result
    }

    #[expect(
        clippy::too_many_lines,
        reason = "two-level 64-stripe L1-resident stable parallel counting sort preserving exact serial order"
    )]
    fn group_into_parallel(
        &mut self,
        buckets: usize,
        items: usize,
        workers: usize,
        bucket_of: &(impl Fn(usize) -> usize + Sync),
    ) -> Result<(), CoreError> {
        const STRIPES: usize = 64;
        const STACK_STRIPE_BUCKETS: usize = 4_096;

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

        self.offsets.resize(buckets + 1, 0);
        if items == 0 {
            self.largest = 0;
            return Ok(());
        }

        let stripe_width = buckets.div_ceil(STRIPES).max(1);
        let stripe_count = buckets.div_ceil(stripe_width);
        let chunk_count = workers.min(items.div_ceil(4_096)).clamp(1, 32);
        let chunk_len = items.div_ceil(chunk_count);

        // Pass 1: per-chunk coarse stripe histograms + exact lowest-index bounds check.
        struct ChunkCount {
            start: usize,
            end: usize,
            counts: [usize; STRIPES],
        }
        let mut chunk_counts: Vec<ChunkCount> = (0..chunk_count)
            .filter_map(|c| {
                let start = c * chunk_len;
                (start < items).then(|| ChunkCount {
                    start,
                    end: (start + chunk_len).min(items),
                    counts: [0; STRIPES],
                })
            })
            .collect();

        flife_core::pool::try_parallel_tasks(
            workers,
            "bins stripe count",
            &mut chunk_counts,
            |_, chunk| {
                for item in chunk.start..chunk.end {
                    let bucket = bucket_of(item);
                    if bucket >= buckets {
                        return Err(outside(item, bucket));
                    }
                    chunk.counts[bucket / stripe_width] += 1;
                }
                Ok(())
            },
        )?;

        // Prefix-sum stripe totals; within each stripe, chunks stay in ascending
        // item order so every stripe buffer is strictly ascending by `item`.
        let mut stripe_bases = [0_usize; STRIPES + 1];
        for s in 0..stripe_count {
            let mut sum = 0_usize;
            for c in &chunk_counts {
                sum += c.counts[s];
            }
            stripe_bases[s + 1] = stripe_bases[s] + sum;
        }

        // Transient `(bucket, item)` buffer grouped into 64 contiguous L1-friendly stripes.
        let mut staged: Vec<(u32, u32)> = Vec::new();
        staged
            .try_reserve_exact(items)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: self.name,
                bytes: u64::try_from(items.saturating_mul(std::mem::size_of::<(u32, u32)>()))
                    .unwrap_or(u64::MAX),
            })?;
        staged.resize(items, (0, 0));

        // Pass 2: parallel scatter of `(bucket, item)` into disjoint per-(chunk, stripe) slices.
        {
            struct ScatterTask<'a> {
                start: usize,
                end: usize,
                dsts: Vec<&'a mut [(u32, u32)]>,
            }
            let mut scatter_tasks: Vec<ScatterTask<'_>> = chunk_counts
                .iter()
                .map(|c| ScatterTask {
                    start: c.start,
                    end: c.end,
                    dsts: Vec::with_capacity(stripe_count),
                })
                .collect();
            let mut rem = staged.as_mut_slice();
            for s in 0..stripe_count {
                for (chunk, task) in chunk_counts.iter().zip(&mut scatter_tasks) {
                    let count = chunk.counts[s];
                    let (slice, tail) = rem.split_at_mut(count);
                    rem = tail;
                    task.dsts.push(slice);
                }
            }
            flife_core::pool::try_parallel_tasks(
                workers,
                "bins stripe scatter",
                &mut scatter_tasks,
                |_, task| {
                    let mut cursors = [0_usize; STRIPES];
                    for item in task.start..task.end {
                        let bucket = bucket_of(item);
                        if bucket >= buckets {
                            return Err(outside(item, bucket));
                        }
                        let s = bucket / stripe_width;
                        let pos = cursors[s];
                        cursors[s] = pos + 1;
                        let Some(slot) = task.dsts[s].get_mut(pos) else {
                            return Err(outside(item, bucket));
                        };
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "buckets and items are bounded by u32::MAX"
                        )]
                        {
                            *slot = (bucket as u32, item as u32);
                        }
                    }
                    Ok(())
                },
            )?;
        }

        // Pass 3: 64 independent L1-resident local counting sorts directly into
        // disjoint slices of `self.offsets[1..=buckets]` and `self.order[..items]`.
        self.offsets[0] = 0;
        self.order.resize(items, 0);
        struct StripeTask<'a> {
            bucket_base: usize,
            item_base: usize,
            staged: &'a [(u32, u32)],
            offsets_out: &'a mut [u32],
            order_out: &'a mut [u32],
            occupied: Vec<u32>,
            largest: u32,
        }
        let mut stripe_tasks = Vec::with_capacity(stripe_count);
        let mut rem_offsets = &mut self.offsets[1..=buckets];
        let mut rem_order = self.order.as_mut_slice();
        for s in 0..stripe_count {
            let bucket_base = s * stripe_width;
            let bucket_len = (buckets - bucket_base).min(stripe_width);
            let item_base = stripe_bases[s];
            let item_end = stripe_bases[s + 1];
            let item_len = item_end - item_base;
            let (off_slice, off_tail) = rem_offsets.split_at_mut(bucket_len);
            rem_offsets = off_tail;
            let (ord_slice, ord_tail) = rem_order.split_at_mut(item_len);
            rem_order = ord_tail;
            stripe_tasks.push(StripeTask {
                bucket_base,
                item_base,
                staged: &staged[item_base..item_end],
                offsets_out: off_slice,
                order_out: ord_slice,
                occupied: Vec::new(),
                largest: 0,
            });
        }

        let name = self.name;
        flife_core::pool::try_parallel_tasks(
            workers,
            "bins stripe sort",
            &mut stripe_tasks,
            |_, task| {
                let b_len = task.offsets_out.len();
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "items is bounded by u32::MAX"
                )]
                let base_u32 = task.item_base as u32;
                if task.staged.is_empty() {
                    task.offsets_out.fill(base_u32);
                    return Ok(());
                }
                let mut stack_counts = [0_u32; STACK_STRIPE_BUCKETS];
                let mut heap_counts = Vec::new();
                let counts: &mut [u32] = if b_len <= STACK_STRIPE_BUCKETS {
                    &mut stack_counts[..b_len]
                } else {
                    heap_counts.try_reserve_exact(b_len).map_err(|_| {
                        CoreError::AllocationFailed {
                            buffer: name,
                            bytes: u64::try_from(b_len.saturating_mul(std::mem::size_of::<u32>()))
                                .unwrap_or(u64::MAX),
                        }
                    })?;
                    heap_counts.resize(b_len, 0);
                    &mut heap_counts
                };
                for &(bucket_u32, _) in task.staged {
                    counts[(bucket_u32 as usize) - task.bucket_base] += 1;
                }
                let mut local_run = 0_u32;
                let mut largest = 0_u32;
                let mut nonempty = 0_usize;
                for &c in counts.iter() {
                    nonempty += usize::from(c > 0);
                }
                task.occupied.try_reserve_exact(nonempty).map_err(|_| {
                    CoreError::AllocationFailed {
                        buffer: name,
                        bytes: u64::try_from(nonempty.saturating_mul(std::mem::size_of::<u32>()))
                            .unwrap_or(u64::MAX),
                    }
                })?;
                for (local_b, (count_slot, out_off)) in counts
                    .iter_mut()
                    .zip(task.offsets_out.iter_mut())
                    .enumerate()
                {
                    let c = *count_slot;
                    *count_slot = local_run;
                    if c > 0 {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "buckets is bounded by u32::MAX"
                        )]
                        task.occupied.push((task.bucket_base + local_b) as u32);
                        largest = largest.max(c);
                        local_run += c;
                    }
                    *out_off = base_u32 + local_run;
                }
                for &(bucket_u32, item_u32) in task.staged {
                    let local_b = (bucket_u32 as usize) - task.bucket_base;
                    let dst = counts[local_b] as usize;
                    counts[local_b] = (dst as u32) + 1;
                    task.order_out[dst] = item_u32;
                }
                task.largest = largest;
                Ok(())
            },
        )?;

        let mut largest = 0_u32;
        for task in &stripe_tasks {
            self.occupied.extend_from_slice(&task.occupied);
            largest = largest.max(task.largest);
        }
        self.largest = largest as usize;
        Ok(())
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
        if items == 0 {
            self.largest = 0;
            return Ok(());
        }
        for item in 0..items {
            let bucket = bucket_of(item);
            if bucket >= buckets {
                return Err(outside(item, bucket));
            }
            match bucket
                .checked_add(1)
                .and_then(|next| self.offsets.get_mut(next))
            {
                Some(count) => *count += 1,
                None => return Err(outside(item, bucket)),
            }
        }
        // 2. Exclusive prefix sum: `offsets[b]` becomes bucket b's start and
        //    `offsets[buckets]` the total. Record nonempty buckets in ascending
        //    order so downstream reductions skip empty buckets.
        let mut running = 0_u32;
        let mut largest = 0_u32;
        for bucket in 0..buckets {
            let count = self.offsets[bucket + 1];
            if count > 0 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "buckets is bounded by u32::MAX"
                )]
                self.occupied.push(bucket as u32);
                largest = largest.max(count);
                running += count;
            }
            self.offsets[bucket + 1] = running;
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
        // Restore the starts in O(occupied): empty buckets already hold their
        // start, and walking nonempty buckets in descending order copies each
        // predecessor's end (which equals this bucket's start).
        for &bucket_u32 in self.occupied.iter().rev() {
            let bucket = bucket_u32 as usize;
            self.offsets[bucket] = if bucket == 0 {
                0
            } else {
                self.offsets[bucket - 1]
            };
        }
        self.largest = largest as usize;
        Ok(())
    }

    /// Empty the bins, keeping their allocation.
    pub fn clear(&mut self) {
        self.offsets.clear();
        self.order.clear();
        self.occupied.clear();
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

    /// Each bucket's first position in [`Bins::order`], then the number of
    /// items: bucket `b`'s positions are `offsets[b]..offsets[b + 1]`. Empty
    /// before the first grouping.
    #[must_use]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// Bucket `bucket`'s positions in [`Bins::order`]; empty for an empty
    /// or out-of-range bucket.
    #[inline]
    #[must_use]
    pub fn range(&self, bucket: usize) -> Range<usize> {
        match (
            self.offsets.get(bucket),
            bucket
                .checked_add(1)
                .and_then(|next| self.offsets.get(next)),
        ) {
            (Some(&start), Some(&end)) => start as usize..end as usize,
            _ => 0..0,
        }
    }

    /// Bucket `bucket`'s item indices, in the order they were given.
    #[inline]
    #[must_use]
    pub fn bucket(&self, bucket: usize) -> &[u32] {
        self.order.get(self.range(bucket)).unwrap_or(&[])
    }

    /// Nonempty bucket indices of the last grouping, in strictly ascending
    /// bucket order.
    #[must_use]
    pub fn occupied_buckets(&self) -> &[u32] {
        &self.occupied
    }

    /// Each nonempty bucket with its positions in [`Bins::order`], in
    /// ascending bucket order.
    pub fn occupied(&self) -> impl Iterator<Item = (usize, Range<usize>)> + '_ {
        self.occupied.iter().map(|&bucket| {
            let b = bucket as usize;
            (b, self.offsets[b] as usize..self.offsets[b + 1] as usize)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_group_on_matches_serial_group_bit_for_bit() {
        let buckets = 65_536_usize;
        let items = 40_000_usize;
        // Mix of hashed uniform positions, a heavy hotspot bucket, and empty stripes.
        let keys: Vec<usize> = (0..items)
            .map(|i| {
                if i % 17 == 0 {
                    42_123
                } else {
                    let mixed = (i.wrapping_mul(2_654_435_761)) ^ (i >> 3);
                    let raw = mixed % buckets;
                    // Leave a whole stripe empty to verify empty-stripe offset fill.
                    if (8_192..10_240).contains(&raw) {
                        raw + 4_096
                    } else {
                        raw
                    }
                }
            })
            .collect();

        let mut serial = Bins::new("serial");
        serial
            .group(buckets, items, |i| keys[i])
            .expect("serial group succeeds");

        for workers in [1_usize, 2, 4, 8] {
            let mut parallel = Bins::new("parallel");
            parallel
                .group_on(buckets, items, workers, |i| keys[i])
                .expect("parallel group_on succeeds");
            assert_eq!(parallel.offsets(), serial.offsets(), "workers={workers}");
            assert_eq!(parallel.order(), serial.order(), "workers={workers}");
            assert_eq!(
                parallel.occupied_buckets(),
                serial.occupied_buckets(),
                "workers={workers}"
            );
            assert_eq!(parallel.largest(), serial.largest(), "workers={workers}");
        }

        // Verify exact lowest-index out-of-range error reporting.
        let mut bad_keys = keys;
        bad_keys[10_001] = buckets;
        bad_keys[35_000] = buckets + 7;
        let serial_err = serial
            .group(buckets, items, |i| bad_keys[i])
            .expect_err("serial rejects out-of-range bucket");
        let mut parallel = Bins::new("parallel");
        let parallel_err = parallel
            .group_on(buckets, items, 4, |i| bad_keys[i])
            .expect_err("parallel rejects out-of-range bucket");
        assert!(
            serial_err.to_string().contains("item 10001")
                && parallel_err.to_string().contains("item 10001"),
            "serial={serial_err}, parallel={parallel_err}"
        );
        assert!(parallel.offsets().is_empty());
        assert!(parallel.order().is_empty());
    }
}
