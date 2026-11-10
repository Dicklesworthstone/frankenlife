//! Checkpoint modes and their memory (FL-038; plan §16.5, §19.4, A.2).
//!
//! A run directory's checkpoints are taken in one of two modes, chosen by
//! `[persistence] checkpoint_mode`:
//!
//! - `pause-and-stream`: the run takes no step while the checkpoint is
//!   encoded straight into the chunk store, chunk by chunk, holding one
//!   page ([`CHECKPOINT_PAGE_BYTES`]) of a chunk at a time, then committed,
//!   read back and verified, indexed and retained. It borrows the committed
//!   world; it never copies it.
//! - `bounded-background`: the pages go through a queue to a writer
//!   thread, which stores, commits, verifies, indexes and retains the
//!   checkpoint. The queue holds at most `[execution]
//!   checkpoint_retained_bytes` of pages, counting the one the writer is
//!   writing; the encoder waits while it is full, so the session stays
//!   paused only until the last page is queued, and the run steps on while
//!   the writer drains. One checkpoint is in flight at a time: the next one
//!   due, the run's end, or any other use of the store waits for the writer
//!   first. The live closure is therefore one checkpoint's retained pages,
//!   never a queue of epochs (§16.5).
//!
//! [`admit`] is the checkpoint term of §19.4's preflight: the most heap a
//! checkpoint holds beyond the live world in each mode, admitted against
//! what `maximum_total_bytes` leaves after `maximum_allocated_genome_bytes`.
//! A `bounded-background` run whose budget cannot hold a page, or whose
//! checkpoint peak exceeds that headroom, takes its checkpoints as
//! `pause-and-stream` and says so; one whose `pause-and-stream` peak
//! exceeds the headroom is refused before it starts. The other terms of
//! §19.4 (state, scratch, render, births, backend) are FL-043's.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use flife_core::checkpoint::{CHECKPOINT_PAGE_BYTES, ChunkFamily, ChunkSink};
use flife_core::{CoreError, Sha256Digest, ValidatedConfig};
use flife_runtime::Boundary;
use flife_store::MANIFEST_FILE_MAX_BYTES;
use serde::Serialize;

/// `[persistence] checkpoint_mode` for [`CheckpointMode::PauseAndStream`].
pub const PAUSE_AND_STREAM: &str = "pause-and-stream";
/// `[persistence] checkpoint_mode` for [`CheckpointMode::BoundedBackground`].
pub const BOUNDED_BACKGROUND: &str = "bounded-background";

/// The heap a checkpoint holds beyond the live world apart from retained
/// pages, in either mode: the encoder's page, one read buffer (a chunk
/// hashed against the store, or a checkpoint verified), the sealed prefix
/// at its largest, and a declared one-mebibyte allowance for paths,
/// handles, hashers and records.
pub const CHECKPOINT_FIXED_BYTES: u64 =
    2 * CHECKPOINT_PAGE_BYTES as u64 + MANIFEST_FILE_MAX_BYTES + (1 << 20);

/// How a run directory's checkpoints are taken (see the
/// [module documentation](self)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointMode {
    /// The run waits while the checkpoint streams into the store.
    PauseAndStream,
    /// The run steps on while a writer drains a bounded page queue.
    BoundedBackground,
}

impl CheckpointMode {
    /// The mode `[persistence] checkpoint_mode` names. The validator admits
    /// only registered names, so any other is `pause-and-stream`.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        if name == BOUNDED_BACKGROUND {
            Self::BoundedBackground
        } else {
            Self::PauseAndStream
        }
    }

    /// Its configuration name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PauseAndStream => PAUSE_AND_STREAM,
            Self::BoundedBackground => BOUNDED_BACKGROUND,
        }
    }
}

/// [`admit`]'s decision, reported in the start record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckpointAdmission {
    /// The mode asked for.
    pub declared: &'static str,
    /// The mode checkpoints are taken in.
    pub admitted: &'static str,
    /// The most page bytes a checkpoint retains while the run steps; zero
    /// for `pause-and-stream`.
    pub retained_bytes: u64,
    /// The most heap a checkpoint holds beyond the live world:
    /// [`CHECKPOINT_FIXED_BYTES`] plus the retained bytes.
    pub peak_bytes: u64,
    /// What `maximum_total_bytes` leaves after
    /// `maximum_allocated_genome_bytes`.
    pub headroom_bytes: u64,
    /// Why the declared mode was not admitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

impl CheckpointAdmission {
    /// The mode checkpoints are taken in.
    #[must_use]
    pub fn mode(&self) -> CheckpointMode {
        CheckpointMode::from_name(self.admitted)
    }
}

/// Admit `mode`'s checkpoints for `config`, with `retained_bytes` the
/// `bounded-background` page budget (see the [module documentation](self)).
///
/// # Errors
/// [`CoreError::BudgetExhausted`] when even a `pause-and-stream`
/// checkpoint's peak exceeds the headroom.
pub fn admit(
    config: &ValidatedConfig,
    mode: CheckpointMode,
    retained_bytes: u64,
) -> Result<CheckpointAdmission, CoreError> {
    let execution = &config.config().execution;
    let headroom = execution
        .maximum_total_bytes
        .saturating_sub(execution.maximum_allocated_genome_bytes);
    let streamed = CheckpointAdmission {
        declared: mode.name(),
        admitted: PAUSE_AND_STREAM,
        retained_bytes: 0,
        peak_bytes: CHECKPOINT_FIXED_BYTES,
        headroom_bytes: headroom,
        fallback: None,
    };
    if streamed.peak_bytes > headroom {
        return Err(CoreError::BudgetExhausted {
            budget: "checkpoint memory",
            required: streamed.peak_bytes,
            available: headroom,
        });
    }
    if mode == CheckpointMode::PauseAndStream {
        return Ok(streamed);
    }
    let peak = CHECKPOINT_FIXED_BYTES.saturating_add(retained_bytes);
    let fallback = if retained_bytes < CHECKPOINT_PAGE_BYTES as u64 {
        Some(format!(
            "checkpoint_retained_bytes {retained_bytes} holds less than one {CHECKPOINT_PAGE_BYTES}-byte page"
        ))
    } else if peak > headroom {
        Some(format!(
            "a bounded-background checkpoint's peak of {peak} bytes exceeds the {headroom} bytes \
             maximum_total_bytes leaves after maximum_allocated_genome_bytes"
        ))
    } else {
        None
    };
    Ok(match fallback {
        Some(reason) => CheckpointAdmission {
            fallback: Some(reason),
            ..streamed
        },
        None => CheckpointAdmission {
            declared: mode.name(),
            admitted: BOUNDED_BACKGROUND,
            retained_bytes,
            peak_bytes: peak,
            headroom_bytes: headroom,
            fallback: None,
        },
    })
}

/// What the encoder hands the writer.
pub(crate) enum Message {
    Begin(usize, ChunkFamily),
    Page(Vec<u8>),
    End(u64, Sha256Digest),
    /// The sealed prefix and the boundary the checkpoint holds: the last.
    Seal(Vec<u8>, Boundary),
}

#[derive(Default)]
struct State {
    messages: VecDeque<Message>,
    /// Page bytes queued or being written.
    retained: u64,
    /// The most `retained` has been.
    peak: u64,
    /// The writer stopped: the encoder's next call refuses.
    writer_stopped: bool,
    /// The encoder is gone, sealed or not.
    encoder_gone: bool,
}

/// The bounded page queue between the encoder and the writer.
pub(crate) struct Queue {
    budget: u64,
    state: Mutex<State>,
    changed: Condvar,
}

impl Queue {
    pub(crate) fn new(budget: u64) -> Arc<Self> {
        Arc::new(Self {
            budget,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves only counters and queued
        // messages, which stay consistent; carry on with them.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The most page bytes retained at once.
    pub(crate) fn peak(&self) -> u64 {
        self.lock().peak
    }

    /// Page bytes queued or being written now.
    #[cfg(test)]
    pub(crate) fn retained(&self) -> u64 {
        self.lock().retained
    }

    fn push(&self, message: Message) -> Result<(), CoreError> {
        let mut state = self.lock();
        if state.writer_stopped {
            return Err(stopped());
        }
        state.messages.push_back(message);
        self.changed.notify_all();
        Ok(())
    }

    /// Wait for room for `length` page bytes and count them retained. A
    /// page is always admitted to an empty queue, and the admission keeps
    /// the budget at least one page.
    fn reserve(&self, length: u64) -> Result<(), CoreError> {
        let mut state = self.lock();
        loop {
            if state.writer_stopped {
                return Err(stopped());
            }
            if state.retained == 0 || state.retained.saturating_add(length) <= self.budget {
                break;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.retained = state.retained.saturating_add(length);
        state.peak = state.peak.max(state.retained);
        Ok(())
    }

    fn release(&self, length: u64) {
        let mut state = self.lock();
        state.retained = state.retained.saturating_sub(length);
        self.changed.notify_all();
    }

    /// The writer's next message, waiting for one; `None` once the
    /// encoder is gone and nothing is queued.
    pub(crate) fn next(&self) -> Option<Message> {
        let mut state = self.lock();
        loop {
            if let Some(message) = state.messages.pop_front() {
                return Some(message);
            }
            if state.encoder_gone {
                return None;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// A page the writer has written: its bytes are no longer retained.
    pub(crate) fn written(&self, page: Vec<u8>) {
        let length = u64::try_from(page.len()).unwrap_or(u64::MAX);
        drop(page);
        self.release(length);
    }

    /// The writer stopped: refuse the encoder and release what is queued.
    pub(crate) fn stop(&self) {
        let mut state = self.lock();
        state.writer_stopped = true;
        let queued: u64 = state
            .messages
            .drain(..)
            .map(|message| match message {
                Message::Page(page) => u64::try_from(page.len()).unwrap_or(u64::MAX),
                _ => 0,
            })
            .sum();
        state.retained = state.retained.saturating_sub(queued);
        self.changed.notify_all();
    }
}

/// The encoder's refusal once the writer has stopped.
pub(crate) const WRITER_STOPPED: &str = "the checkpoint writer stopped";

fn stopped() -> CoreError {
    CoreError::Cancelled {
        reason: WRITER_STOPPED.into(),
    }
}

/// The encoder's end of the queue: a [`ChunkSink`] whose pages are copied
/// into the queue, waiting while it holds its budget.
pub(crate) struct Producer {
    queue: Arc<Queue>,
}

impl Producer {
    pub(crate) const fn new(queue: Arc<Queue>) -> Self {
        Self { queue }
    }

    /// Queue the sealed prefix and boundary: the checkpoint is complete.
    pub(crate) fn seal(self, sealed: Vec<u8>, boundary: Boundary) -> Result<(), CoreError> {
        self.queue.push(Message::Seal(sealed, boundary))
    }
}

impl ChunkSink for Producer {
    fn begin(&mut self, index: usize, family: ChunkFamily) -> Result<(), CoreError> {
        self.queue.push(Message::Begin(index, family))
    }

    fn page(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.queue.reserve(length)?;
        let mut page = Vec::new();
        if page.try_reserve_exact(bytes.len()).is_err() {
            self.queue.release(length);
            return Err(CoreError::AllocationFailed {
                buffer: "retained checkpoint page",
                bytes: length,
            });
        }
        page.extend_from_slice(bytes);
        let pushed = self.queue.push(Message::Page(page));
        if pushed.is_err() {
            self.queue.release(length);
        }
        pushed
    }

    fn end(&mut self, length: u64, digest: Sha256Digest) -> Result<(), CoreError> {
        self.queue.push(Message::End(length, digest))
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        let mut state = self.queue.lock();
        state.encoder_gone = true;
        self.queue.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use flife_core::checkpoint::{CHECKPOINT_PAGE_BYTES, ChunkFamily, ChunkSink};
    use flife_core::ids::ResourceEpoch;
    use flife_core::{CoreError, Sha256Digest, Tick};
    use flife_runtime::Boundary;

    use super::{Message, Producer, Queue, WRITER_STOPPED};

    const PAGE: usize = CHECKPOINT_PAGE_BYTES;

    fn boundary() -> Boundary {
        Boundary {
            tick: Tick::new(7),
            epoch: ResourceEpoch::new(7),
        }
    }

    /// Feed `pages` pages (page `i` filled with byte `i`) and seal.
    fn produce(queue: &Arc<Queue>, pages: usize) -> Result<(), CoreError> {
        let mut producer = Producer::new(Arc::clone(queue));
        producer.begin(0, ChunkFamily::Genomes)?;
        for page in 0..pages {
            producer.page(&vec![u8::try_from(page % 251).unwrap_or(0); PAGE])?;
        }
        producer.end(0, Sha256Digest::of(b""))?;
        producer.seal(vec![1, 2, 3], boundary())
    }

    /// §16.5: with a writer slower than the encoder, the pages queued and
    /// being written never exceed the budget, every page arrives once and
    /// in order, and all are released once written.
    #[test]
    fn a_full_queue_holds_the_encoder_at_its_budget() {
        let budget = 4 * PAGE as u64;
        let queue = Queue::new(budget);
        let writer_queue = Arc::clone(&queue);
        let writer = std::thread::spawn(move || {
            let mut seen = Vec::new();
            while let Some(message) = writer_queue.next() {
                match message {
                    Message::Page(page) => {
                        std::thread::sleep(Duration::from_millis(2));
                        seen.push(page[0]);
                        assert!(page.iter().all(|byte| *byte == page[0]));
                        writer_queue.written(page);
                    }
                    Message::Seal(sealed, at) => {
                        assert_eq!((sealed, at), (vec![1, 2, 3], boundary()));
                        break;
                    }
                    Message::Begin(..) | Message::End(..) => {}
                }
            }
            seen
        });
        produce(&queue, 64).expect("every page is queued");
        let seen = writer.join().expect("the writer finishes");
        let expected: Vec<u8> = (0..64_u8).collect();
        assert_eq!(seen, expected);
        assert!(queue.peak() <= budget, "peak {} > {budget}", queue.peak());
        assert!(
            queue.peak() >= 2 * PAGE as u64,
            "the queue was used: {}",
            queue.peak()
        );
        assert_eq!(queue.retained(), 0);
    }

    /// A stopped writer refuses the encoder's next page and releases every
    /// page it had queued.
    #[test]
    fn a_stopped_writer_releases_its_pages_and_refuses_more() {
        let queue = Queue::new(8 * PAGE as u64);
        let mut producer = Producer::new(Arc::clone(&queue));
        producer.begin(0, ChunkFamily::Field).expect("begins");
        for _ in 0..3 {
            producer.page(&[9; PAGE]).expect("queues");
        }
        assert_eq!(queue.retained(), 3 * PAGE as u64);
        queue.stop();
        assert_eq!(queue.retained(), 0);
        let refused = producer.page(&[9; PAGE]);
        assert!(
            matches!(&refused, Err(CoreError::Cancelled { reason }) if reason == WRITER_STOPPED),
            "{refused:?}"
        );
        assert_eq!(queue.retained(), 0);
    }

    /// An encoder dropped before sealing ends the writer's stream: the
    /// writer sees the end of the queue, not a seal.
    #[test]
    fn an_abandoned_encoder_ends_the_queue_without_a_seal() {
        let queue = Queue::new(2 * PAGE as u64);
        let mut producer = Producer::new(Arc::clone(&queue));
        producer.begin(0, ChunkFamily::Field).expect("begins");
        producer.page(&[1; 16]).expect("queues");
        drop(producer);
        let mut sealed = false;
        while let Some(message) = queue.next() {
            match message {
                Message::Seal(..) => sealed = true,
                Message::Page(page) => queue.written(page),
                Message::Begin(..) | Message::End(..) => {}
            }
        }
        assert!(!sealed);
        assert_eq!(queue.retained(), 0);
    }
}
