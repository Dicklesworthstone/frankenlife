//! The Metal owner (FL-028; plan §9.5, §10.3, §12.4): every device buffer a
//! backend uses, with its state, its typed handle, its leases and the
//! commands that bind it.
//!
//! - [`DeviceOwner`] allocates typed buffers ([`Buffer<f32>`],
//!   [`Buffer<u32>`]) within a declared byte budget. It is the only path by
//!   which a command binds them ([`DeviceOwner::submit_batch`], or
//!   [`DeviceOwner::submit`] for one kernel).
//! - A command is one or more ordered kernels in one command buffer (FL-029):
//!   each kernel reads what earlier ones wrote, and the bindings of all of
//!   them are checked, held and released as one command.
//! - A buffer has any number of immutable readers or one writer, never
//!   both. Readers are [`ReadLease`]s (rendering, checkpointing, host
//!   readback) and the commands that read it. The writer is a
//!   [`WriteLease`] (host writes) or the command that writes it. One buffer
//!   bound twice to a command, once written, is refused as an alias.
//! - A command keeps every buffer it binds until its completion is
//!   verified. The gateway's batch waits for the command buffer and checks
//!   its status, and only then are the bindings released. While it
//!   runs, every other thread sees those buffers as
//!   [`ResourceState::Computing`] and is refused incompatible access.
//! - A command that fails after it was committed may have written part of
//!   its outputs. Those buffers become undefined and refuse reads until they
//!   are rewritten. A command refused before it was encoded changes nothing.
//! - Retiring a buffer refuses new use at once, and frees it when its last
//!   lease or command releases it. A handle to a freed buffer is stale and
//!   refused, even after its slot is reused, and so is a handle of another
//!   owner.
//! - A lease keeps the owner's table alive and is released when dropped,
//!   so a consumer that exits, or unwinds, releases what it held.

use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use flife_core::CoreError;
use flife_core::ids::ResourceEpoch;
use ft_kernel_metal::Error as MetalError;
use ft_kernel_metal::compute::{
    Argument, Gateway, Grid, Input, Library, MathMode, Output, Pipeline, SharedBuffer,
};

/// An element type a device buffer holds: `f32` or `u32`.
pub trait Element: Copy + Default + Send + Sync + 'static {
    /// Its name, for messages.
    const NAME: &'static str;

    /// A device buffer holding `data`.
    ///
    /// # Errors
    /// The gateway's.
    fn upload(gateway: &Gateway, data: &[Self]) -> Result<SharedBuffer, MetalError>;

    /// Overwrite `data.len()` elements of `buffer`, from element `offset`.
    ///
    /// # Errors
    /// The gateway's, for a range outside the buffer; nothing is written.
    fn write(buffer: &mut SharedBuffer, offset: usize, data: &[Self]) -> Result<(), MetalError>;

    /// Copy the first `out.len()` elements of `buffer` out.
    ///
    /// # Errors
    /// The gateway's, for more elements than the buffer holds.
    fn read(buffer: &SharedBuffer, out: &mut [Self]) -> Result<(), MetalError>;
}

impl Element for f32 {
    const NAME: &'static str = "f32";

    fn upload(gateway: &Gateway, data: &[Self]) -> Result<SharedBuffer, MetalError> {
        gateway.buffer_f32(data)
    }

    fn write(buffer: &mut SharedBuffer, offset: usize, data: &[Self]) -> Result<(), MetalError> {
        buffer.write_f32(offset, data)
    }

    fn read(buffer: &SharedBuffer, out: &mut [Self]) -> Result<(), MetalError> {
        buffer.read_f32(out)
    }
}

impl Element for u32 {
    const NAME: &'static str = "u32";

    fn upload(gateway: &Gateway, data: &[Self]) -> Result<SharedBuffer, MetalError> {
        gateway.buffer_u32(data)
    }

    fn write(buffer: &mut SharedBuffer, offset: usize, data: &[Self]) -> Result<(), MetalError> {
        buffer.write_u32(offset, data)
    }

    fn read(buffer: &SharedBuffer, out: &mut [Self]) -> Result<(), MetalError> {
        buffer.read_u32(out)
    }
}

/// A buffer's state (plan §9.5), from its current use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceState {
    /// Allocated zeroed with nothing written yet, or undefined after a
    /// command that failed while writing it.
    Available,
    /// Bound to a running command, or held by a host writer.
    Computing,
    /// Holding what a completed command or a host writer wrote, with no
    /// renderer or checkpoint reading it.
    Committed,
    /// Leased to a renderer.
    Rendering,
    /// Leased to a checkpoint writer.
    Checkpointing,
    /// Retired: refused to new use, and freed once its last lease or
    /// command releases it.
    Retiring,
}

/// Why a [`ReadLease`] is held. Each purpose has its own lease limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// A renderer reads it.
    Rendering,
    /// A checkpoint writer reads it.
    Checkpointing,
    /// The host copies a command's results out.
    Readback,
}

const PURPOSES: usize = 3;

impl Purpose {
    const fn index(self) -> usize {
        match self {
            Self::Rendering => 0,
            Self::Checkpointing => 1,
            Self::Readback => 2,
        }
    }

    /// The purpose's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rendering => "rendering",
            Self::Checkpointing => "checkpointing",
            Self::Readback => "readback",
        }
    }
}

/// What an owner admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Device bytes its buffers may hold at once, retiring ones included.
    pub bytes: u64,
    /// Read leases one purpose may hold at once (plan §9.5: outstanding
    /// leases are limited per consumer).
    pub leases_per_purpose: u32,
}

impl Limits {
    /// No limit but the device's.
    pub const UNBOUNDED: Self = Self {
        bytes: u64::MAX,
        leases_per_purpose: u32::MAX,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Handle {
    owner: u64,
    index: u32,
    generation: u32,
}

/// A typed handle to one of a [`DeviceOwner`]'s buffers: its elements are
/// `T`, and its length counts them. It is only a name. Every access goes
/// through the owner, which refuses a handle whose buffer was freed or that
/// another owner issued.
pub struct Buffer<T> {
    handle: Handle,
    element: PhantomData<fn() -> T>,
}

impl<T> Clone for Buffer<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Buffer<T> {}

impl<T> PartialEq for Buffer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle
    }
}

impl<T> Eq for Buffer<T> {}

impl<T: Element> fmt::Debug for Buffer<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Buffer<{}>(#{} gen {} of owner {})",
            T::NAME,
            self.handle.index,
            self.handle.generation,
            self.handle.owner
        )
    }
}

impl<T: Element> Buffer<T> {
    /// This buffer, bound for a command to read.
    #[must_use]
    pub const fn reads(self) -> Binding {
        Binding {
            handle: self.handle,
            access: Access::Read,
        }
    }

    /// This buffer, bound for a command to write.
    #[must_use]
    pub const fn writes(self) -> Binding {
        Binding {
            handle: self.handle,
            access: Access::Write,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

/// One buffer bound to a command at the next argument index, read or
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    handle: Handle,
    access: Access,
}

/// One kernel of a command (FL-029): `pipeline` over `grid`, with
/// `bindings` at argument indices `0..`.
#[derive(Clone, Copy)]
pub struct Kernel<'a> {
    /// The compute pipeline.
    pub pipeline: &'a Pipeline,
    /// Its buffer arguments, in index order, each read or written.
    pub bindings: &'a [Binding],
    /// Its dispatch geometry.
    pub grid: Grid,
}

impl fmt::Debug for Kernel<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Kernel")
            .field("bindings", &self.bindings)
            .field("grid", &self.grid)
            .finish_non_exhaustive()
    }
}

/// A completed command.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    /// The epoch each buffer it wrote now holds, in order of each buffer's
    /// first binding. A buffer several of its kernels wrote advances one
    /// epoch: only the command's final contents are ever observable.
    pub written: Vec<ResourceEpoch>,
    /// Kernels it ran, in order, in one command buffer.
    pub kernels: usize,
    /// The device's time for its command buffer, in seconds, from the GPU
    /// starting it to finishing it.
    pub device_seconds: f64,
}

/// One buffer a command binds, after its bindings were checked.
struct Bound {
    handle: Handle,
    written: bool,
    memory: Arc<RwLock<SharedBuffer>>,
}

/// A bound buffer's lock while its command runs: written buffers are held
/// exclusively, read ones shared.
enum Held<'a> {
    Read(RwLockReadGuard<'a, SharedBuffer>),
    Write(RwLockWriteGuard<'a, SharedBuffer>),
}

/// A bound buffer's registration in the gateway's batch.
#[derive(Clone, Copy)]
enum Slot {
    Input(Input),
    Output(Output),
}

/// What an owner holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Live buffers, retiring ones included.
    pub buffers: usize,
    /// Their bytes.
    pub bytes: u64,
    /// Of them, retiring.
    pub retiring: usize,
    /// Read leases held, by purpose: rendering, checkpointing, readback.
    pub read_leases: [u32; PURPOSES],
    /// Write leases held.
    pub write_leases: usize,
    /// Commands running.
    pub running: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Host,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contents {
    Zeroed,
    Defined,
    Undefined,
}

struct Resident {
    memory: Arc<RwLock<SharedBuffer>>,
    element: &'static str,
    len: usize,
    bytes: u64,
    epoch: u64,
    contents: Contents,
    writer: Option<Writer>,
    computing: u32,
    leases: [u32; PURPOSES],
    retiring: bool,
}

impl Resident {
    fn readers(&self) -> u64 {
        u64::from(self.computing) + self.leases.iter().map(|&n| u64::from(n)).sum::<u64>()
    }

    fn unused(&self) -> bool {
        self.writer.is_none() && self.readers() == 0
    }

    fn state(&self) -> ResourceState {
        if self.retiring {
            ResourceState::Retiring
        } else if self.writer.is_some() || self.computing > 0 {
            ResourceState::Computing
        } else if self.leases[Purpose::Checkpointing.index()] > 0 {
            ResourceState::Checkpointing
        } else if self.leases[Purpose::Rendering.index()] > 0 {
            ResourceState::Rendering
        } else if self.contents == Contents::Defined {
            ResourceState::Committed
        } else {
            ResourceState::Available
        }
    }
}

struct Entry {
    generation: u32,
    resident: Option<Resident>,
}

#[derive(Default)]
struct Table {
    entries: Vec<Entry>,
    free: Vec<u32>,
    bytes: u64,
    leases: [u32; PURPOSES],
    write_leases: usize,
    running: usize,
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "metal resource",
        detail,
    }
}

/// A gateway error as a [`CoreError`]: no device is a missing capability,
/// anything else a refused operation.
pub(crate) fn metal(error: MetalError) -> CoreError {
    match error {
        MetalError::Unavailable => CoreError::MissingCapability {
            capability: "a Metal device".to_owned(),
        },
        other => CoreError::StateConflict {
            operation: "metal dispatch",
            detail: other.to_string(),
        },
    }
}

impl Table {
    fn resident(&mut self, owner: u64, handle: Handle) -> Result<&mut Resident, CoreError> {
        if handle.owner != owner {
            return Err(conflict(format!(
                "buffer #{} belongs to owner {}, not {owner}",
                handle.index, handle.owner
            )));
        }
        let entry = usize::try_from(handle.index)
            .ok()
            .and_then(|index| self.entries.get_mut(index));
        match entry {
            Some(Entry {
                generation,
                resident: Some(resident),
            }) if *generation == handle.generation => Ok(resident),
            _ => Err(conflict(format!(
                "buffer #{} generation {} has been freed",
                handle.index, handle.generation
            ))),
        }
    }

    /// Free a retiring buffer nothing uses any more.
    fn settle(&mut self, handle: Handle) {
        let Some(entry) = usize::try_from(handle.index)
            .ok()
            .and_then(|index| self.entries.get_mut(index))
        else {
            return;
        };
        if entry.generation != handle.generation {
            return;
        }
        let done = entry
            .resident
            .as_ref()
            .is_some_and(|resident| resident.retiring && resident.unused());
        if !done {
            return;
        }
        if let Some(resident) = entry.resident.take() {
            self.bytes = self.bytes.saturating_sub(resident.bytes);
        }
        // A slot whose generations are exhausted is never reused, so no
        // handle can name a later buffer by accident.
        if let Some(next) = entry.generation.checked_add(1) {
            entry.generation = next;
            self.free.push(handle.index);
        }
    }

    fn usage(&self) -> Usage {
        let mut usage = Usage {
            bytes: self.bytes,
            read_leases: self.leases,
            write_leases: self.write_leases,
            running: self.running,
            ..Usage::default()
        };
        for resident in self
            .entries
            .iter()
            .filter_map(|entry| entry.resident.as_ref())
        {
            usage.buffers += 1;
            usage.retiring += usize::from(resident.retiring);
        }
        usage
    }
}

struct Shared {
    id: u64,
    gateway: Gateway,
    limits: Limits,
    table: Mutex<Table>,
}

impl Shared {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

static OWNERS: AtomicU64 = AtomicU64::new(1);

/// The owner of a Metal device's buffers for one backend (see the
/// [module documentation](self)). Clones are handles to the same owner and
/// may be used from any thread.
#[derive(Clone)]
pub struct DeviceOwner {
    shared: Arc<Shared>,
}

impl fmt::Debug for DeviceOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceOwner")
            .field("id", &self.shared.id)
            .field("device", &self.shared.gateway.device_name())
            .field("limits", &self.shared.limits)
            .field("usage", &self.usage())
            .finish()
    }
}

impl DeviceOwner {
    /// An owner of the default Metal device's buffers, admitting `limits`.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device, which is
    /// always the case off macOS.
    pub fn open(limits: Limits) -> Result<Self, CoreError> {
        let gateway = Gateway::open().map_err(metal)?;
        Ok(Self {
            shared: Arc::new(Shared {
                id: OWNERS.fetch_add(1, Ordering::Relaxed),
                gateway,
                limits,
                table: Mutex::new(Table::default()),
            }),
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.shared.gateway.device_name()
    }

    /// What this owner admits.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.shared.limits
    }

    /// The largest buffer the device allocates, in bytes.
    #[must_use]
    pub fn max_buffer_bytes(&self) -> usize {
        self.shared.gateway.max_buffer_length()
    }

    /// What this owner holds.
    #[must_use]
    pub fn usage(&self) -> Usage {
        self.shared.table().usage()
    }

    /// Compile Metal Shading Language `source` in `mode`.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when it does not compile.
    pub fn library(&self, source: &str, mode: MathMode) -> Result<Library, CoreError> {
        self.shared
            .gateway
            .library_with(source, mode)
            .map_err(metal)
    }

    /// A new buffer of `len` zeroed elements, [`ResourceState::Available`]
    /// at epoch 0.
    ///
    /// # Errors
    /// As [`DeviceOwner::upload`].
    pub fn zeroed<T: Element>(&self, len: usize) -> Result<Buffer<T>, CoreError> {
        let bytes = Self::bytes_of::<T>(len)?;
        self.admit::<T>(len, bytes, Contents::Zeroed, |gateway| {
            gateway.buffer_zeroed(bytes)
        })
    }

    /// A new buffer holding `data`, [`ResourceState::Committed`] at
    /// epoch 1.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for no elements;
    /// [`CoreError::BudgetExhausted`] beyond the owner's byte limit;
    /// [`CoreError::AllocationFailed`] when the device refuses it (above its
    /// largest buffer, for instance). Nothing is allocated on refusal.
    pub fn upload<T: Element>(&self, data: &[T]) -> Result<Buffer<T>, CoreError> {
        let bytes = Self::bytes_of::<T>(data.len())?;
        self.admit::<T>(data.len(), bytes, Contents::Defined, |gateway| {
            T::upload(gateway, data)
        })
    }

    fn bytes_of<T: Element>(len: usize) -> Result<usize, CoreError> {
        if len == 0 {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal buffer",
                detail: "a buffer holds at least one element".to_owned(),
            });
        }
        len.checked_mul(size_of::<T>()).ok_or(CoreError::Overflow {
            quantity: "metal buffer bytes",
        })
    }

    fn admit<T: Element>(
        &self,
        len: usize,
        bytes: usize,
        contents: Contents,
        allocate: impl FnOnce(&Gateway) -> Result<SharedBuffer, MetalError>,
    ) -> Result<Buffer<T>, CoreError> {
        let wide = u64::try_from(bytes).unwrap_or(u64::MAX);
        let mut table = self.shared.table();
        let available = self.shared.limits.bytes.saturating_sub(table.bytes);
        if wide > available {
            return Err(CoreError::BudgetExhausted {
                budget: "metal owner bytes",
                required: wide,
                available,
            });
        }
        let memory = allocate(&self.shared.gateway).map_err(|error| match error {
            MetalError::Unavailable => metal(error),
            _ => CoreError::AllocationFailed {
                buffer: "metal buffer",
                bytes: wide,
            },
        })?;
        let resident = Resident {
            memory: Arc::new(RwLock::new(memory)),
            element: T::NAME,
            len,
            bytes: wide,
            epoch: u64::from(contents == Contents::Defined),
            contents,
            writer: None,
            computing: 0,
            leases: [0; PURPOSES],
            retiring: false,
        };
        let (index, generation) = if let Some(index) = table.free.pop() {
            let Some(entry) = usize::try_from(index)
                .ok()
                .and_then(|at| table.entries.get_mut(at))
            else {
                return Err(conflict(format!("free slot {index} is missing")));
            };
            entry.resident = Some(resident);
            (index, entry.generation)
        } else {
            let index = u32::try_from(table.entries.len()).map_err(|_| CoreError::Overflow {
                quantity: "metal buffer slots",
            })?;
            table.entries.push(Entry {
                generation: 0,
                resident: Some(resident),
            });
            (index, 0)
        };
        table.bytes = table.bytes.saturating_add(wide);
        Ok(Buffer {
            handle: Handle {
                owner: self.shared.id,
                index,
                generation,
            },
            element: PhantomData,
        })
    }

    /// The host's exclusive write access to `buffer`. When the lease is
    /// released the buffer's contents are defined, at its next epoch.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stale or foreign handle, a
    /// retiring buffer, or one with any reader or writer.
    pub fn write<T: Element>(&self, buffer: Buffer<T>) -> Result<WriteLease<T>, CoreError> {
        let mut table = self.shared.table();
        let resident = table.resident(self.shared.id, buffer.handle)?;
        if resident.retiring {
            return Err(conflict(format!("{buffer:?} is retiring")));
        }
        if resident.writer.is_some() || resident.readers() > 0 {
            return Err(conflict(format!(
                "{buffer:?} is {:?}: a writer needs it unread and unwritten",
                resident.state()
            )));
        }
        resident.writer = Some(Writer::Host);
        let memory = Arc::clone(&resident.memory);
        let len = resident.len;
        table.write_leases += 1;
        Ok(WriteLease {
            shared: Arc::clone(&self.shared),
            handle: buffer.handle,
            memory,
            len,
            element: PhantomData,
        })
    }

    /// Immutable read access to `buffer` for `purpose`: nothing writes it
    /// while the lease is held.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stale or foreign handle, a
    /// retiring buffer, one being written, or one whose contents are
    /// undefined; [`CoreError::BudgetExhausted`] when `purpose` holds its
    /// limit of leases.
    pub fn read<T: Element>(
        &self,
        buffer: Buffer<T>,
        purpose: Purpose,
    ) -> Result<ReadLease<T>, CoreError> {
        let limit = self.shared.limits.leases_per_purpose;
        let mut table = self.shared.table();
        let held = table.leases[purpose.index()];
        let resident = table.resident(self.shared.id, buffer.handle)?;
        if resident.retiring {
            return Err(conflict(format!("{buffer:?} is retiring")));
        }
        if resident.writer.is_some() {
            return Err(conflict(format!("{buffer:?} is being written")));
        }
        if resident.contents == Contents::Undefined {
            return Err(conflict(format!(
                "{buffer:?} is undefined: a command failed while writing it"
            )));
        }
        if held >= limit {
            return Err(CoreError::BudgetExhausted {
                budget: "metal read leases",
                required: u64::from(held) + 1,
                available: u64::from(limit),
            });
        }
        resident.leases[purpose.index()] += 1;
        let lease = ReadLease {
            shared: Arc::clone(&self.shared),
            handle: buffer.handle,
            memory: Arc::clone(&resident.memory),
            purpose,
            epoch: ResourceEpoch::new(resident.epoch),
            len: resident.len,
            element: PhantomData,
        };
        table.leases[purpose.index()] += 1;
        Ok(lease)
    }

    /// Run `pipeline` over `grid` with `bindings` at argument indices
    /// `0..`: [`DeviceOwner::submit_batch`] of that one kernel.
    ///
    /// # Errors
    /// As [`DeviceOwner::submit_batch`].
    pub fn submit(
        &self,
        pipeline: &Pipeline,
        bindings: &[Binding],
        grid: Grid,
    ) -> Result<Receipt, CoreError> {
        self.submit_batch(&[Kernel {
            pipeline,
            bindings,
            grid,
        }])
    }

    /// Run `kernels` in order as one command: one command buffer, committed
    /// once, its completion verified once. Each kernel reads what earlier
    /// ones wrote. The command keeps every buffer it binds from the first
    /// kernel to the verified completion of the last (see the [module
    /// documentation](self)).
    ///
    /// The bindings are checked as one command. A buffer any kernel writes
    /// needs no other reader or writer; a buffer the kernels only read needs
    /// no writer. A buffer whose first binding reads it needs defined
    /// contents, so an undefined buffer may be rewritten by one kernel and
    /// read by a later one.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`], with nothing bound or run, for no
    /// kernels, a stale, foreign or retiring buffer, one buffer bound twice
    /// to a kernel and written there, a written buffer with any reader or
    /// writer, a read-only buffer being written, or a buffer read first
    /// while undefined. The gateway's refusal or failure, after which the
    /// bindings are released: buffers the command wrote are undefined if its
    /// command buffer failed, and unchanged if the gateway refused a kernel
    /// before anything was encoded.
    pub fn submit_batch(&self, kernels: &[Kernel<'_>]) -> Result<Receipt, CoreError> {
        let lists: Vec<&[Binding]> = kernels.iter().map(|kernel| kernel.bindings).collect();
        let bound = self.bind(&lists)?;
        let outcome = {
            let mut held: Vec<Held<'_>> = bound
                .iter()
                .map(|buffer| {
                    if buffer.written {
                        Held::Write(
                            buffer
                                .memory
                                .write()
                                .unwrap_or_else(PoisonError::into_inner),
                        )
                    } else {
                        Held::Read(buffer.memory.read().unwrap_or_else(PoisonError::into_inner))
                    }
                })
                .collect();
            let mut batch = self.shared.gateway.batch();
            let slots: Vec<Slot> = held
                .iter_mut()
                .map(|guard| match guard {
                    Held::Read(memory) => Slot::Input(batch.input(memory)),
                    Held::Write(memory) => Slot::Output(batch.output(memory)),
                })
                .collect();
            let mut encoded = Ok(());
            for kernel in kernels {
                let arguments: Option<Vec<Argument>> = kernel
                    .bindings
                    .iter()
                    .map(|binding| {
                        let at = bound
                            .iter()
                            .position(|buffer| buffer.handle == binding.handle)?;
                        match (slots.get(at)?, binding.access) {
                            (Slot::Output(output), Access::Write) => Some(output.write()),
                            (Slot::Output(output), Access::Read) => Some(output.read()),
                            (Slot::Input(input), Access::Read) => Some(input.read()),
                            (Slot::Input(_), Access::Write) => None,
                        }
                    })
                    .collect();
                let Some(arguments) = arguments else {
                    encoded = Err(MetalError::Kernel(
                        "a binding does not match what its buffer was bound for".to_owned(),
                    ));
                    break;
                };
                encoded = batch.dispatch(kernel.pipeline, &arguments, kernel.grid);
                if encoded.is_err() {
                    break;
                }
            }
            encoded.and_then(|()| batch.submit())
        };
        let written = self.release(&bound, outcome.as_ref().map(|_| ()));
        outcome.map_err(metal).map(|completed| Receipt {
            written,
            kernels: completed.kernels,
            device_seconds: completed.device_seconds,
        })
    }

    /// Check every kernel's bindings as one command, then mark every buffer
    /// it binds, returning them in order of first binding.
    fn bind(&self, kernels: &[&[Binding]]) -> Result<Vec<Bound>, CoreError> {
        if kernels.is_empty() {
            return Err(conflict("a command of no kernels".to_owned()));
        }
        // Each buffer once: whether any kernel writes it, and whether its
        // first binding reads it.
        let mut uses: Vec<(Binding, bool)> = Vec::new();
        for bindings in kernels {
            for (at, binding) in bindings.iter().enumerate() {
                let aliased = bindings[..at].iter().any(|earlier| {
                    earlier.handle == binding.handle
                        && (earlier.access == Access::Write || binding.access == Access::Write)
                });
                if aliased {
                    return Err(conflict(format!(
                        "buffer #{} is bound twice and written: an alias",
                        binding.handle.index
                    )));
                }
                match uses
                    .iter_mut()
                    .find(|(first, _)| first.handle == binding.handle)
                {
                    Some((_, written)) => *written |= binding.access == Access::Write,
                    None => uses.push((*binding, binding.access == Access::Write)),
                }
            }
        }
        let mut table = self.shared.table();
        for &(first, written) in &uses {
            let resident = table.resident(self.shared.id, first.handle)?;
            let name = || format!("buffer #{} ({})", first.handle.index, resident.element);
            if resident.retiring {
                return Err(conflict(format!("{} is retiring", name())));
            }
            if written && (resident.writer.is_some() || resident.readers() > 0) {
                return Err(conflict(format!(
                    "{} is {:?}: a command writes it only unread and unwritten",
                    name(),
                    resident.state()
                )));
            }
            if !written && resident.writer.is_some() {
                return Err(conflict(format!("{} is being written", name())));
            }
            if first.access == Access::Read && resident.contents == Contents::Undefined {
                return Err(conflict(format!(
                    "{} is undefined: a command failed while writing it",
                    name()
                )));
            }
        }
        let mut bound = Vec::with_capacity(uses.len());
        for (first, written) in uses {
            let resident = table.resident(self.shared.id, first.handle)?;
            if written {
                resident.writer = Some(Writer::Command);
            } else {
                resident.computing += 1;
            }
            bound.push(Bound {
                handle: first.handle,
                written,
                memory: Arc::clone(&resident.memory),
            });
        }
        table.running += 1;
        Ok(bound)
    }

    /// Release a command's buffers once its outcome is known, returning the
    /// epochs of the buffers it wrote.
    fn release(&self, bound: &[Bound], outcome: Result<(), &MetalError>) -> Vec<ResourceEpoch> {
        let mut table = self.shared.table();
        let mut written = Vec::new();
        for buffer in bound {
            if let Ok(resident) = table.resident(self.shared.id, buffer.handle) {
                if buffer.written {
                    resident.writer = None;
                    match outcome {
                        Ok(()) => {
                            resident.contents = Contents::Defined;
                            resident.epoch = resident.epoch.saturating_add(1);
                        }
                        Err(MetalError::CommandBuffer(_)) => {
                            resident.contents = Contents::Undefined;
                            resident.epoch = resident.epoch.saturating_add(1);
                        }
                        Err(_) => {}
                    }
                    written.push(ResourceEpoch::new(resident.epoch));
                } else {
                    resident.computing = resident.computing.saturating_sub(1);
                }
            }
            table.settle(buffer.handle);
        }
        table.running = table.running.saturating_sub(1);
        written
    }

    /// Retire `buffer`: refuse new use now, and free it once nothing uses
    /// it. Returns whether it was freed now. Retiring a retiring buffer
    /// again changes nothing.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stale or foreign handle.
    pub fn retire<T: Element>(&self, buffer: Buffer<T>) -> Result<bool, CoreError> {
        let mut table = self.shared.table();
        let resident = table.resident(self.shared.id, buffer.handle)?;
        resident.retiring = true;
        table.settle(buffer.handle);
        Ok(table.resident(self.shared.id, buffer.handle).is_err())
    }

    /// `buffer`'s state.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stale or foreign handle.
    pub fn state<T: Element>(&self, buffer: Buffer<T>) -> Result<ResourceState, CoreError> {
        let mut table = self.shared.table();
        Ok(table.resident(self.shared.id, buffer.handle)?.state())
    }

    /// The epoch of `buffer`'s contents: 0 when allocated zeroed, 1 when
    /// uploaded, and one more for each completed write.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stale or foreign handle.
    pub fn epoch<T: Element>(&self, buffer: Buffer<T>) -> Result<ResourceEpoch, CoreError> {
        let mut table = self.shared.table();
        Ok(ResourceEpoch::new(
            table.resident(self.shared.id, buffer.handle)?.epoch,
        ))
    }

    /// How many elements `buffer` holds.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stale or foreign handle.
    pub fn elements<T: Element>(&self, buffer: Buffer<T>) -> Result<usize, CoreError> {
        let mut table = self.shared.table();
        Ok(table.resident(self.shared.id, buffer.handle)?.len)
    }

    /// Settle a command's bindings as if the gateway had returned
    /// `outcome`, for the failure path no device fault on the test host can
    /// reach (see the crate's contract).
    #[cfg(test)]
    fn bind_and_release(
        &self,
        bindings: &[Binding],
        outcome: &Result<(), MetalError>,
    ) -> Result<(), CoreError> {
        let bound = self.bind(&[bindings])?;
        self.release(&bound, outcome.as_ref().copied());
        Ok(())
    }
}

/// A buffer its holder retires when it drops it: a backend's resident
/// buffers and a call's temporaries, freed as soon as nothing else uses
/// them.
pub struct Owned<T: Element> {
    owner: DeviceOwner,
    buffer: Buffer<T>,
}

impl<T: Element> fmt::Debug for Owned<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Owned")
            .field("buffer", &self.buffer)
            .finish_non_exhaustive()
    }
}

impl<T: Element> Owned<T> {
    /// `buffer` of `owner`, retired when this is dropped.
    #[must_use]
    pub const fn new(owner: DeviceOwner, buffer: Buffer<T>) -> Self {
        Self { owner, buffer }
    }

    /// The buffer.
    #[must_use]
    pub const fn buffer(&self) -> Buffer<T> {
        self.buffer
    }
}

impl<T: Element> Drop for Owned<T> {
    fn drop(&mut self) {
        // A stale handle has nothing left to retire.
        let _ = self.owner.retire(self.buffer);
    }
}

impl DeviceOwner {
    /// [`DeviceOwner::zeroed`], retired when dropped.
    ///
    /// # Errors
    /// As [`DeviceOwner::zeroed`].
    pub fn zeroed_owned<T: Element>(&self, len: usize) -> Result<Owned<T>, CoreError> {
        Ok(Owned::new(self.clone(), self.zeroed(len)?))
    }

    /// [`DeviceOwner::upload`], retired when dropped.
    ///
    /// # Errors
    /// As [`DeviceOwner::upload`].
    pub fn upload_owned<T: Element>(&self, data: &[T]) -> Result<Owned<T>, CoreError> {
        Ok(Owned::new(self.clone(), self.upload(data)?))
    }

    /// The first `out.len()` elements of `buffer`, read under a readback
    /// lease.
    ///
    /// # Errors
    /// As [`DeviceOwner::read`] and [`ReadLease::read`].
    pub fn read_back<T: Element>(&self, buffer: Buffer<T>, out: &mut [T]) -> Result<(), CoreError> {
        self.read(buffer, Purpose::Readback)?.read(out)
    }
}

/// The host's exclusive write access to one buffer (see
/// [`DeviceOwner::write`]). Released when dropped.
pub struct WriteLease<T: Element> {
    shared: Arc<Shared>,
    handle: Handle,
    memory: Arc<RwLock<SharedBuffer>>,
    len: usize,
    element: PhantomData<fn() -> T>,
}

impl<T: Element> fmt::Debug for WriteLease<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WriteLease")
            .field("element", &T::NAME)
            .field("buffer", &self.handle.index)
            .field("len", &self.len)
            .finish()
    }
}

impl<T: Element> WriteLease<T> {
    /// Overwrite `data.len()` elements from element `offset`.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a range outside the buffer; nothing
    /// is written.
    pub fn write(&mut self, offset: usize, data: &[T]) -> Result<(), CoreError> {
        let mut memory = self.memory.write().unwrap_or_else(PoisonError::into_inner);
        T::write(&mut memory, offset, data).map_err(metal)
    }

    /// How many elements the buffer holds.
    #[must_use]
    pub const fn elements(&self) -> usize {
        self.len
    }
}

impl<T: Element> Drop for WriteLease<T> {
    fn drop(&mut self) {
        let mut table = self.shared.table();
        if let Ok(resident) = table.resident(self.shared.id, self.handle) {
            resident.writer = None;
            resident.contents = Contents::Defined;
            resident.epoch = resident.epoch.saturating_add(1);
        }
        table.write_leases = table.write_leases.saturating_sub(1);
        table.settle(self.handle);
    }
}

/// Immutable read access to one buffer (see [`DeviceOwner::read`]).
/// Released when dropped.
pub struct ReadLease<T: Element> {
    shared: Arc<Shared>,
    handle: Handle,
    memory: Arc<RwLock<SharedBuffer>>,
    purpose: Purpose,
    epoch: ResourceEpoch,
    len: usize,
    element: PhantomData<fn() -> T>,
}

impl<T: Element> fmt::Debug for ReadLease<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadLease")
            .field("element", &T::NAME)
            .field("buffer", &self.handle.index)
            .field("purpose", &self.purpose)
            .field("epoch", &self.epoch)
            .finish()
    }
}

impl<T: Element> ReadLease<T> {
    /// Copy the first `out.len()` elements out.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for more elements than the buffer holds.
    pub fn read(&self, out: &mut [T]) -> Result<(), CoreError> {
        let memory = self.memory.read().unwrap_or_else(PoisonError::into_inner);
        T::read(&memory, out).map_err(metal)
    }

    /// The epoch of the contents this lease reads; it cannot change while
    /// the lease is held.
    #[must_use]
    pub const fn epoch(&self) -> ResourceEpoch {
        self.epoch
    }

    /// Why it is held.
    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.purpose
    }

    /// How many elements the buffer holds.
    #[must_use]
    pub const fn elements(&self) -> usize {
        self.len
    }
}

impl<T: Element> Drop for ReadLease<T> {
    fn drop(&mut self) {
        let index = self.purpose.index();
        let mut table = self.shared.table();
        if let Ok(resident) = table.resident(self.shared.id, self.handle) {
            resident.leases[index] = resident.leases[index].saturating_sub(1);
        }
        table.leases[index] = table.leases[index].saturating_sub(1);
        table.settle(self.handle);
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceOwner, Limits, ResourceState};
    use ft_kernel_metal::{CommandBufferState, Error as MetalError};

    /// No device fault on the test host fails a command after it was
    /// committed (GPU validation does not fail an out-of-bounds store), so
    /// this settles bindings with the gateway's own post-commit error: the
    /// written buffer becomes undefined and refuses reads until rewritten,
    /// while the read buffer keeps its contents and epoch.
    #[test]
    fn a_command_that_failed_after_commit_leaves_its_outputs_undefined() {
        let Ok(owner) = DeviceOwner::open(Limits::UNBOUNDED) else {
            return;
        };
        let (Ok(input), Ok(output)) = (owner.upload(&[1.0_f32; 4]), owner.upload(&[2.0_f32; 4]))
        else {
            panic!("allocates");
        };
        let failed = Err(MetalError::CommandBuffer(CommandBufferState::Error));
        assert!(
            owner
                .bind_and_release(&[input.reads(), output.writes()], &failed)
                .is_ok()
        );
        assert_eq!(owner.state(output).ok(), Some(ResourceState::Available));
        assert_eq!(owner.epoch(output).map(|epoch| epoch.get()).ok(), Some(2));
        assert!(owner.read(output, super::Purpose::Readback).is_err());
        assert!(
            owner
                .bind_and_release(&[output.reads(), input.writes()], &Ok(()))
                .is_err(),
            "an undefined buffer is not read by a command either"
        );
        assert_eq!(owner.state(input).ok(), Some(ResourceState::Committed));
        assert_eq!(owner.epoch(input).map(|epoch| epoch.get()).ok(), Some(1));
        // Rewritten by the host, it is defined again.
        drop(owner.write(output));
        assert_eq!(owner.state(output).ok(), Some(ResourceState::Committed));
        assert!(owner.read(output, super::Purpose::Readback).is_ok());
        // A refusal before encoding changes nothing.
        let refused = Err(MetalError::Kernel("refused".to_owned()));
        assert!(
            owner
                .bind_and_release(&[input.reads(), output.writes()], &refused)
                .is_ok()
        );
        assert_eq!(owner.epoch(output).map(|epoch| epoch.get()).ok(), Some(3));
        assert_eq!(owner.state(output).ok(), Some(ResourceState::Committed));
        assert_eq!(owner.usage().running, 0);
    }
}
