//! The command journal port (FL-036; plan §12.3, §12.4, §16.6) and the
//! writer thread that performs its I/O.
//!
//! A `write-ahead-before-apply` session makes each acceptance and each
//! terminal outcome durable through a [`Journal`] before it relies on it.
//! The runtime defines only the port: `flife-store` implements it over
//! FrankenSQLite, and a test can implement it to lose a receipt.
//!
//! - Records are appended in the order the owner sends them, by one writer
//!   thread ([`JournalWriter`]), so the journal never holds a record
//!   without every earlier one. Acceptances are numbered by admission order
//!   and outcomes by the outcome stream's cursor; an append that is not the
//!   next number is refused. The durable [`Watermarks`] are therefore two
//!   contiguous counts, and they are distinct: an acceptance for tick 1000
//!   can be durable long before the outcome of one for tick 101.
//! - An append is idempotent by identity. Appending a record the journal
//!   already holds succeeds without writing; the same identity with other
//!   content is refused. So reconciling an uncertain append is appending it
//!   again: a record that did persist is found by its command identity, and
//!   one that did not is written once.
//! - The writer retries a failed append [`JOURNAL_ATTEMPTS`] times. If the
//!   last attempt fails, it fails every later record without trying it, so
//!   nothing after an uncertain record can become durable before it.
//!
//! The owner never waits on the thread: it sends records and reads
//! receipts as messages, and holds a boundary until the receipts that
//! boundary needs arrive.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};

use asupersync::Cx;
use asupersync::channel::mpsc;
use flife_core::CoreError;
use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, SessionId};

use crate::command::{Accepted, Command, Outcome};

/// How many times the writer tries one append before it reports failure.
pub const JOURNAL_ATTEMPTS: u32 = 3;

/// One record the journal makes durable, for the session it is bound to.
#[derive(Debug, Clone, PartialEq)]
pub enum JournalRecord {
    /// An acceptance: the command's canonical record and what admission
    /// assigned it. Numbered by `accepted.order`.
    Accepted {
        /// The command as the client submitted it.
        command: Command,
        /// Its admission order, effective tick and retargeting.
        accepted: Accepted,
    },
    /// A terminal outcome. Numbered by its cursor.
    Ended {
        /// The command that ended.
        id: CommandId,
        /// How it ended.
        outcome: Outcome,
    },
}

/// The journal's durable positions, as two contiguous counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Watermarks {
    /// Acceptances `0..admitted`, by admission order, are durable.
    pub admitted: u64,
    /// Outcomes `0..outcomes`, by outcome cursor, are durable.
    pub outcomes: u64,
}

/// Everything a journal holds for one session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JournalContents {
    /// Every acceptance, in admission order from zero.
    pub accepted: Vec<(Command, Accepted)>,
    /// Every terminal outcome, in cursor order from zero.
    pub outcomes: Vec<(CommandId, Outcome)>,
}

impl JournalContents {
    /// The positions these contents reach.
    #[must_use]
    pub fn watermarks(&self) -> Watermarks {
        Watermarks {
            admitted: u64::try_from(self.accepted.len()).unwrap_or(u64::MAX),
            outcomes: u64::try_from(self.outcomes.len()).unwrap_or(u64::MAX),
        }
    }
}

/// A durable command journal: the port a `write-ahead-before-apply`
/// session writes through. It is used from one thread only.
pub trait Journal {
    /// Bind the journal to `session`, which runs configuration `revision`,
    /// recording the run if it is new, and read back everything it holds
    /// for that session.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the session was recorded under
    /// another revision; [`CoreError::StorageFailed`] when the host refuses
    /// an operation; [`CoreError::IncompatibleCheckpoint`] for a record that
    /// does not decode.
    fn open_run(
        &mut self,
        session: SessionId,
        revision: Sha256Digest,
    ) -> Result<JournalContents, CoreError>;

    /// Make `record` durable and return the watermarks after it. A record
    /// the journal already holds is not written again.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a record that is not the next of its
    /// kind, or whose identity the journal holds with other content;
    /// [`CoreError::StorageFailed`] when the host refuses an operation. A
    /// failure is uncertain: the record may or may not have persisted.
    fn append(&mut self, record: &JournalRecord) -> Result<Watermarks, CoreError>;
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Drive `future` to completion on this thread, with an ambient Asupersync
/// context that has no blocking pool, so the blocking I/O it starts runs
/// inline on this thread rather than on a new thread per operation (the
/// FrankenSQLite lesson recorded in rust_scriptbots: about 0.5 s per
/// statement under load with a thread each, milliseconds inline). Use it
/// only on a thread that may block and runs no Asupersync runtime, such as
/// the journal's writer thread: never on the session owner or a runtime
/// worker. There FrankenSQLite reaches for the thread's runtime while this
/// parks the thread, and an open never completes.
pub fn block_on_inline<F: Future>(future: F) -> F::Output {
    let _inline_io = Cx::detached_cancel_context().set_current_restricted();
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        thread::park();
    }
}

/// The receipt for one appended record: the watermarks after it, or why
/// it is uncertain.
pub type Receipt = Result<Watermarks, CoreError>;

/// The thread that owns a [`Journal`] and appends what the owner sends, in
/// order, answering each record with one [`Receipt`].
pub struct JournalWriter {
    requests: Option<mpsc::UnboundedSender<JournalRecord>>,
    receipts: mpsc::UnboundedReceiver<Receipt>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for JournalWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalWriter").finish_non_exhaustive()
    }
}

fn writer_failed(reason: impl Into<String>) -> CoreError {
    CoreError::StorageFailed {
        operation: "run the journal writer",
        path: "flife-journal thread".into(),
        reason: reason.into(),
    }
}

impl JournalWriter {
    /// Start the writer thread. It calls `open` to make the journal there
    /// (a FrankenSQLite connection cannot move between threads), binds it to
    /// `session` and `revision`, and returns what the journal already holds
    /// for the session, which a recovering session must start from.
    ///
    /// # Errors
    /// `open`'s or [`Journal::open_run`]'s error, or
    /// [`CoreError::StorageFailed`] if the thread cannot start.
    pub fn spawn<F>(
        open: F,
        session: SessionId,
        revision: Sha256Digest,
    ) -> Result<(Self, JournalContents), CoreError>
    where
        F: FnOnce() -> Result<Box<dyn Journal>, CoreError> + Send + 'static,
    {
        let (request_sender, mut requests) = mpsc::unbounded_channel::<JournalRecord>();
        let (receipt_sender, receipts) = mpsc::unbounded_channel::<Receipt>();
        let (opened_sender, opened) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("flife-journal".into())
            .spawn(move || {
                let opened_run = open().and_then(|mut journal| {
                    let contents = journal.open_run(session, revision)?;
                    Ok((journal, contents))
                });
                let mut journal = match opened_run {
                    Ok((journal, contents)) => {
                        if opened_sender.send(Ok(contents)).is_err() {
                            return;
                        }
                        journal
                    }
                    Err(error) => {
                        let _ = opened_sender.send(Err(error));
                        return;
                    }
                };
                let cx = Cx::detached_cancel_context();
                let mut failed: Option<CoreError> = None;
                while let Ok(record) = block_on_inline(requests.recv(&cx)) {
                    let receipt = match &failed {
                        Some(error) => Err(error.clone()),
                        None => {
                            let mut attempt = 1;
                            loop {
                                match journal.append(&record) {
                                    Ok(watermarks) => break Ok(watermarks),
                                    Err(_) if attempt < JOURNAL_ATTEMPTS => attempt += 1,
                                    Err(error) => {
                                        failed = Some(error.clone());
                                        break Err(error);
                                    }
                                }
                            }
                        }
                    };
                    if receipt_sender.send(receipt).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| writer_failed(error.to_string()))?;
        let contents = opened
            .recv()
            .map_err(|_| writer_failed("the writer thread ended before opening the journal"))?;
        let writer = Self {
            requests: Some(request_sender),
            receipts,
            thread: Some(thread),
        };
        Ok((writer, contents?))
    }

    /// Queue `record` for the writer. Its receipt arrives in order.
    pub(crate) fn send(&self, record: JournalRecord) -> Result<(), CoreError> {
        self.requests
            .as_ref()
            .ok_or_else(|| writer_failed("the writer is finished"))?
            .send(record)
            .map_err(|_| writer_failed("the writer thread has ended"))
    }

    /// The next receipt, if one has arrived.
    pub(crate) fn try_receipt(&mut self) -> Option<Receipt> {
        match self.receipts.try_recv() {
            Ok(receipt) => Some(receipt),
            Err(mpsc::RecvError::Empty) => None,
            Err(_) => Some(Err(writer_failed("the writer thread has ended"))),
        }
    }

    /// The receipts lane, for the owner's wait.
    pub(crate) const fn receipts(&mut self) -> &mut mpsc::UnboundedReceiver<Receipt> {
        &mut self.receipts
    }

    /// Stop the writer once it has appended everything sent, and join it.
    pub(crate) fn finish(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for JournalWriter {
    fn drop(&mut self) {
        self.finish();
    }
}
