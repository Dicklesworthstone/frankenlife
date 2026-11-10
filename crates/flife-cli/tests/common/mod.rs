//! Chunk stores the tests open in-process, and the `flife` processes they
//! start, kept apart.
//!
//! A store's writer lock is a lock on an open file. Starting a process
//! duplicates every open descriptor, the lock's included, until the child
//! executes `flife` (the descriptor closes on execution), so a store a test
//! closes in that window stays locked until then, and the `flife` it starts
//! next is refused ("another writer holds"). On Linux the window is real:
//! `flife-store`'s own tests were refused once that way (rch vmi1227854 at
//! 372cbc8). macOS starts processes without it. Processes are started
//! holding [`SPAWNING`] shared, and `Command::spawn` returns only once the
//! child has executed; a [`Store`] is closed holding it exclusively, so no
//! child holds a copy of its lock when it closes.

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{PoisonError, RwLock};

use flife_core::CoreError;
use flife_store::{ChunkStore, Durability};

static SPAWNING: RwLock<()> = RwLock::new(());

/// A chunk store closed under [`SPAWNING`].
pub(crate) struct Store(Option<ChunkStore>);

impl Deref for Store {
    type Target = ChunkStore;

    fn deref(&self) -> &ChunkStore {
        self.0.as_ref().expect("open until dropped")
    }
}

impl DerefMut for Store {
    fn deref_mut(&mut self) -> &mut ChunkStore {
        self.0.as_mut().expect("open until dropped")
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _quiet = SPAWNING.write().unwrap_or_else(PoisonError::into_inner);
        drop(self.0.take());
    }
}

/// [`ChunkStore::open`], closed under [`SPAWNING`].
pub(crate) fn open_store(
    root: impl AsRef<Path>,
    durability: Durability,
) -> Result<Store, CoreError> {
    ChunkStore::open(root, durability).map(|store| Store(Some(store)))
}

/// `command` started under [`SPAWNING`].
pub(crate) fn spawn(command: &mut Command) -> Child {
    let _spawning = SPAWNING.read().unwrap_or_else(PoisonError::into_inner);
    command.spawn().expect("flife starts")
}

/// `command` run to completion, its output captured, started under
/// [`SPAWNING`].
pub(crate) fn output(command: &mut Command) -> Output {
    spawn(command.stdout(Stdio::piped()).stderr(Stdio::piped()))
        .wait_with_output()
        .expect("flife runs")
}
