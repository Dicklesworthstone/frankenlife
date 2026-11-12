//! Publishing render epochs: preallocated slots, typed per-consumer leases
//! and latest-value publication (plan §9.5, §12.4, B.3).
//!
//! A [`Publisher`] belongs to the world's owner. It allocates
//! `execution.render_queue_capacity` epoch slots once and publishes into
//! them at committed boundaries. Consumers hold a [`Feed`], open a
//! [`Viewer`] with a lease limit, and read epochs through [`Lease`]s.
//!
//! - A slot is written only when no one else holds it. A lease and the
//!   feed's latest-epoch cell each hold a shared reference, and the
//!   publisher writes through [`Arc::get_mut`], which succeeds only when its
//!   own reference is the last. A leased or displayable epoch therefore
//!   cannot be overwritten: the type system refuses it.
//! - An epoch is filled while the publisher holds it exclusively, and enters
//!   the cell only once complete. No consumer can read one before
//!   publication.
//! - When every slot is leased or displayable, the boundary is not
//!   published, nothing is allocated, and the last displayable epoch stays.
//!   The boundary stays pending: the next offer, or one the owner makes
//!   when a lease is released, publishes the newest committed state. This
//!   is the drop arm of `drop-observer-or-pause-unchanged-world` (§7.5): a
//!   display consumer that retains its leases only loses freshness.
//! - Each viewer holds at most its limit of leases, and a limit is below
//!   the slot count, so no single consumer can hold every slot.
//! - The owner hands the publisher every tick's report through
//!   [`Publisher::count`], published or skipped, and each epoch carries the
//!   running [`Tally`] through its own tick (plan §14.1). A tick missed or
//!   counted twice ends the tally: no later epoch carries one, rather than
//!   a total that is short or doubled.
//!
//! Nothing here can reach the world mutably: the publisher reads it through
//! `&World`. Observation cannot change the science (INV-09).

use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use flife_core::error::ConfigIssue;
use flife_core::ids::{IdSequence, RenderEpoch, ResourceEpoch};
use flife_core::{CoreError, Tick};
use flife_engine::{TickReport, World};

use crate::epoch::{Epoch, Prior, Tally, reserved};

/// The default lease limit: the current epoch and its prior, for display
/// one committed state late (§13.5, `committed-states-one-epoch-late`).
pub const DISPLAY_PAIR: usize = 2;

/// The release hook's type.
type Released = Box<dyn Fn() + Send + Sync>;

/// What the publisher and its consumers share.
struct Shared {
    latest: Mutex<Option<Arc<Epoch>>>,
    capacity: usize,
    closed: AtomicBool,
    released: Released,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shared")
            .field("capacity", &self.capacity)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl Shared {
    /// The latest epoch. The lock is held only to clone a reference, never
    /// across a projection or a callback; a poisoned lock still holds a
    /// whole `Option<Arc<_>>`.
    fn latest(&self) -> Option<Arc<Epoch>> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn replace(&self, epoch: Arc<Epoch>) -> Option<Arc<Epoch>> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(epoch)
    }
}

/// What an offer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publication {
    /// `epoch` is now the latest displayable epoch; it replaced `replaced`.
    Published {
        /// The new epoch.
        epoch: Prior,
        /// The epoch it replaced.
        replaced: Prior,
    },
    /// The latest epoch already shows this committed state.
    Current(Prior),
    /// Every slot is leased or displayable. The boundary at `tick` stays
    /// pending, and `kept` stays displayable.
    Busy {
        /// The committed tick that was not published.
        tick: Tick,
        /// The epoch that stays the latest.
        kept: Prior,
    },
}

/// The publisher's counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Epochs published, including the founding one.
    pub published: u64,
    /// Committed boundaries that found every slot busy, each counted once
    /// however often it was retried.
    pub busy: u64,
    /// The committed tick waiting for a free slot, if any.
    pub pending: Option<Tick>,
    /// The first projection failure, if any. A failure never touches the
    /// world; the last displayable epoch stays.
    pub failure: Option<CoreError>,
}

/// The owner's side: the slots and the identity sequence.
#[derive(Debug)]
pub struct Publisher {
    shared: Arc<Shared>,
    slots: Vec<Arc<Epoch>>,
    ids: IdSequence<RenderEpoch>,
    latest: Prior,
    resource: ResourceEpoch,
    stats: RenderStats,
    tally: Tally,
    /// The tick the tally runs through; `None` once a tick was missed.
    counted: Option<Tick>,
}

impl Publisher {
    /// A publisher with `capacity` slots for `world`'s epochs, which
    /// publishes `world`'s committed state as epoch 1. `released` runs after
    /// every lease is dropped and its slot reference is gone, so an owner
    /// can retry a pending boundary.
    ///
    /// # Errors
    /// As [`Publisher::continuing`].
    pub fn new(
        world: &World,
        capacity: usize,
        released: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, CoreError> {
        Self::continuing(world, capacity, Some(1), released)
    }

    /// [`Publisher::new`] with epoch identities continuing from `next`, as a
    /// resumed run does; `None` means the sequence is exhausted.
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] for fewer than two slots, since one
    /// displayable epoch and one being written need two (§9.5);
    /// [`CoreError::AllocationFailed`] when a slot cannot be allocated;
    /// [`CoreError::Overflow`] for an exhausted sequence; and the founding
    /// projection's errors.
    pub fn continuing(
        world: &World,
        capacity: usize,
        next: Option<u64>,
        released: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, CoreError> {
        if capacity < 2 {
            return Err(CoreError::InvalidConfig(vec![ConfigIssue {
                path: "execution.render_queue_capacity".into(),
                problem: format!(
                    "must be at least 2, got {capacity}: one displayable epoch and one being written"
                ),
            }]));
        }
        let mut slots = reserved(capacity, "render epoch slots")?;
        for _ in 0..capacity {
            slots.push(Arc::new(Epoch::allocate(world)?));
        }
        let mut ids = IdSequence::resume(next);
        let id = ids.issue()?;
        let founding = Prior {
            id,
            tick: world.tick(),
        };
        let first =
            slots
                .first_mut()
                .and_then(Arc::get_mut)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "found render publisher",
                    detail: "a new slot is already shared".into(),
                })?;
        let tally = Tally::from(world.tick());
        first.project(world, id, None, Some(tally))?;
        let shared = Arc::new(Shared {
            latest: Mutex::new(slots.first().map(Arc::clone)),
            capacity,
            closed: AtomicBool::new(false),
            released: Box::new(released),
        });
        Ok(Self {
            shared,
            slots,
            ids,
            latest: founding,
            resource: world.epoch(),
            stats: RenderStats {
                published: 1,
                ..RenderStats::default()
            },
            tally,
            counted: Some(world.tick()),
        })
    }

    /// Count a committed tick's report into the tally its epochs carry
    /// ([`crate::Metrics::tally`]). The owner gives it every report it
    /// publishes, in order, before offering the world. A report that does
    /// not follow the last one counted, a tick missed or given twice, ends
    /// the count: no later epoch carries a tally.
    pub fn count(&mut self, report: &TickReport) {
        if self.counted == Some(report.from) {
            self.tally.add(report);
            self.counted = Some(report.tick);
        } else {
            self.counted = None;
        }
    }

    /// The tally through the last tick counted, or `None` once a tick was
    /// missed.
    #[must_use]
    pub fn tally(&self) -> Option<Tally> {
        self.counted.map(|_| self.tally)
    }

    /// Offer `world`'s committed state. It is published into the lowest
    /// free slot, unless the latest epoch already shows it or every slot is
    /// busy.
    ///
    /// # Errors
    /// The projection's errors and [`CoreError::Overflow`] for an exhausted
    /// identity sequence. Each is also recorded in [`RenderStats::failure`];
    /// the last displayable epoch stays.
    pub fn offer(&mut self, world: &World) -> Result<Publication, CoreError> {
        if world.epoch() == self.resource {
            return Ok(Publication::Current(self.latest));
        }
        let Some(slot) = self
            .slots
            .iter_mut()
            .find_map(|slot| Arc::get_mut(slot).is_some().then_some(slot))
        else {
            // Counted once per boundary, however often it is retried.
            if self.stats.pending != Some(world.tick()) {
                self.stats.busy += 1;
            }
            self.stats.pending = Some(world.tick());
            return Ok(Publication::Busy {
                tick: world.tick(),
                kept: self.latest,
            });
        };
        // The tally rides only on the tick it was counted through.
        let tally = (self.counted == Some(world.tick())).then_some(self.tally);
        let projected = match self.ids.issue() {
            Ok(id) => match Arc::get_mut(slot) {
                Some(epoch) => epoch
                    .project(world, id, Some(self.latest), tally)
                    .map(|()| id),
                None => Err(CoreError::StateConflict {
                    operation: "publish render epoch",
                    detail: "a free slot became shared".into(),
                }),
            },
            Err(error) => Err(error),
        };
        let id = match projected {
            Ok(id) => id,
            Err(error) => {
                self.stats.pending = Some(world.tick());
                if self.stats.failure.is_none() {
                    self.stats.failure = Some(error.clone());
                }
                return Err(error);
            }
        };
        let epoch = Prior {
            id,
            tick: world.tick(),
        };
        // The replaced reference is dropped after the lock is released.
        let replaced_reference = self.shared.replace(Arc::clone(slot));
        drop(replaced_reference);
        let replaced = self.latest;
        self.latest = epoch;
        self.resource = world.epoch();
        self.stats.published += 1;
        self.stats.pending = None;
        Ok(Publication::Published { epoch, replaced })
    }

    /// A feed for consumers.
    #[must_use]
    pub fn feed(&self) -> Feed {
        Feed {
            shared: Arc::clone(&self.shared),
        }
    }

    /// The counters.
    #[must_use]
    pub const fn stats(&self) -> &RenderStats {
        &self.stats
    }

    /// The latest epoch's identity and tick.
    #[must_use]
    pub const fn latest(&self) -> Prior {
        self.latest
    }

    /// The slots.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Slots no lease and no cell holds: the ones an offer could write now.
    #[must_use]
    pub fn free(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| Arc::strong_count(slot) == 1)
            .count()
    }

    /// Bytes reserved by every slot: the render budget (§7.5).
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        self.slots
            .iter()
            .map(|slot| slot.reserved_bytes())
            .fold(0, u64::saturating_add)
    }

    /// The raw identity the next publication takes, for a resumed run.
    #[must_use]
    pub const fn next_id(&self) -> Option<u64> {
        self.ids.next_raw()
    }

    /// Mark the feed closed: the world has stopped. The last epoch stays
    /// readable.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
    }
}

/// A consumer's handle on a publisher's epochs. Cloning it shares the feed.
#[derive(Debug, Clone)]
pub struct Feed {
    shared: Arc<Shared>,
}

impl Feed {
    /// A viewer that holds at most `limit` leases at once.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `1 <= limit < capacity`: no
    /// single consumer may hold every slot.
    pub fn viewer(&self, limit: usize) -> Result<Viewer, CoreError> {
        if limit == 0 || limit >= self.shared.capacity {
            return Err(CoreError::NumericalInvalid {
                quantity: "render lease limit",
                detail: format!(
                    "{limit} is outside 1..{}: no consumer may hold every slot",
                    self.shared.capacity
                ),
            });
        }
        Ok(Viewer {
            shared: Arc::clone(&self.shared),
            held: Arc::new(AtomicUsize::new(0)),
            limit,
        })
    }

    /// The slots.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Whether the world has stopped publishing.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// The latest epoch's identity and tick, without leasing it.
    #[must_use]
    pub fn latest(&self) -> Option<Prior> {
        self.shared.latest().map(|epoch| Prior {
            id: epoch.id(),
            tick: epoch.tick(),
        })
    }
}

/// One consumer: a lease budget over a feed. It is not `Clone`; every
/// lease it takes counts against its own limit.
#[derive(Debug)]
pub struct Viewer {
    shared: Arc<Shared>,
    held: Arc<AtomicUsize>,
    limit: usize,
}

impl Viewer {
    /// Lease the latest epoch.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] when this viewer already holds its
    /// limit.
    pub fn latest(&self) -> Result<Lease, CoreError> {
        self.take()?;
        match self.shared.latest() {
            Some(epoch) => Ok(self.lease(epoch)),
            None => {
                self.held.fetch_sub(1, Ordering::AcqRel);
                Err(CoreError::StateConflict {
                    operation: "lease render epoch",
                    detail: "nothing has been published".into(),
                })
            }
        }
    }

    /// Lease the latest epoch if it is newer than `seen`; otherwise take
    /// nothing from the budget.
    ///
    /// # Errors
    /// As [`Viewer::latest`].
    pub fn latest_after(&self, seen: RenderEpoch) -> Result<Option<Lease>, CoreError> {
        match self.shared.latest() {
            Some(epoch) if epoch.id() > seen => {
                self.take()?;
                Ok(Some(self.lease(epoch)))
            }
            _ => Ok(None),
        }
    }

    /// Leases held now.
    #[must_use]
    pub fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// The lease limit.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn take(&self) -> Result<(), CoreError> {
        let limit = self.limit;
        self.held
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < limit).then_some(held + 1)
            })
            .map(|_| ())
            .map_err(|held| CoreError::BudgetExhausted {
                budget: "render leases",
                required: held as u64 + 1,
                available: limit as u64,
            })
    }

    fn lease(&self, epoch: Arc<Epoch>) -> Lease {
        Lease {
            held: Held {
                epoch,
                count: Arc::clone(&self.held),
            },
            _released: Signal {
                shared: Arc::clone(&self.shared),
            },
        }
    }
}

/// A typed read lease on one published epoch (B.3: keeps its storage alive
/// and immutable to this reader). Dropping it releases the slot reference,
/// then returns the budget, then tells the owner.
#[derive(Debug)]
pub struct Lease {
    // Fields drop in order: the reference and the budget, then the signal.
    held: Held,
    _released: Signal,
}

#[derive(Debug)]
struct Held {
    epoch: Arc<Epoch>,
    count: Arc<AtomicUsize>,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct Signal {
    shared: Arc<Shared>,
}

impl Drop for Signal {
    fn drop(&mut self) {
        (self.shared.released)();
    }
}

impl Lease {
    /// The leased epoch.
    #[must_use]
    pub fn epoch(&self) -> &Epoch {
        &self.held.epoch
    }
}

impl Deref for Lease {
    type Target = Epoch;

    fn deref(&self) -> &Epoch {
        &self.held.epoch
    }
}
