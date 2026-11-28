// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, atomic::AtomicBool};
use std::time::Duration;

use cubecl::prelude::Client;
use cubecl::wgpu::{RuntimeOptions, Vulkan, WgpuBackend, WgpuDevice, WgpuDeviceKind};
use cubecl_environment::stream::Stream;

use crate::{
    ComputeBackend, ComputeDevice, ComputeError, ComputeSettings, ComputeStatus,
    MASK_SEGMENT_INPUT, MASK_SEGMENT_WORDS, MaskRows, Shape, check_cancel, kernels, ordered_key,
    zeroed_words,
};

static SESSION: OnceLock<Session> = OnceLock::new();
static INITIALIZATION: Mutex<()> = Mutex::new(());

struct Session {
    settings: ComputeSettings,
    context: Result<Context, ComputeError>,
}

pub(crate) struct Context {
    status: ComputeStatus,
    /// Primary devices initialized synchronously.
    devices: Vec<ResolvedDevice>,
    /// Optional CPU helper resolved on a background thread when `Auto` picks a
    /// hardware GPU, so engine startup never blocks on CPU kernel JIT.
    helper: Arc<OnceLock<ResolvedDevice>>,
    _fea: ResolvedDevice,
    /// How many devices, from the first, take exclusive stage batches. The
    /// CPU helper beside a hardware GPU takes queued mask requests only: a
    /// whole stage segment runs many times slower there and finishes last.
    stage_devices: usize,
    /// The first execution failure; every later round refuses with it.
    failure: Mutex<Option<ComputeError>>,
    pending: Mutex<Pending>,
    wake: Condvar,
}

impl Context {
    fn total_devices(&self) -> usize {
        self.devices.len() + usize::from(self.helper.get().is_some())
    }

    fn device(&self, index: usize) -> &ResolvedDevice {
        if index < self.devices.len() {
            &self.devices[index]
        } else {
            self.helper
                .get()
                .expect("helper device slot only selected once initialized")
        }
    }
}

struct ProbeGate {
    state: Mutex<Option<Result<(), ComputeError>>>,
    ready: Condvar,
}

impl ProbeGate {
    fn pending() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    fn completed(result: Result<(), ComputeError>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Some(result)),
            ready: Condvar::new(),
        })
    }

    fn finish(&self, result: Result<(), ComputeError>) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = Some(result);
            self.ready.notify_all();
        }
    }

    fn wait(&self) -> Result<(), ComputeError> {
        let mut guard = self.state.lock().map_err(|_| ComputeError::Poisoned)?;
        loop {
            if let Some(result) = guard.as_ref() {
                return result.clone();
            }
            guard = self.ready.wait(guard).map_err(|_| ComputeError::Poisoned)?;
        }
    }
}

/// Queued mask requests and the devices' leaders. Requests from every thread
/// wait here; whoever finds a free slot takes everything queued and runs it
/// as one round (group commit), so launches scale with device time, not calls.
#[derive(Default)]
struct Pending {
    /// Whether each slot of each device is leading a round.
    leaders: Vec<Vec<bool>>,
    next: u64,
    queue: Vec<(u64, Vec<MaskGroup>)>,
    /// Exclusive callers waiting for the device, served first come first
    /// served so a long batch's later segments queue behind a short request.
    turns: std::collections::VecDeque<u64>,
    done: Vec<(u64, Result<Vec<MaskRows>, ComputeError>)>,
    abandoned: Vec<u64>,
}

/// One encoded mask request: `shape.rows` query rows against
/// `shape.columns` padded four-lane input fields.
pub(crate) struct MaskGroup {
    kind: u32,
    shape: Shape,
    values: Vec<u32>,
    queries: Vec<u32>,
}

#[derive(Clone)]
struct ResolvedDevice {
    client: Client,
    /// One fixed stream per slot: rounds on different slots are in flight
    /// together (one uploads while another computes or reads back), and the
    /// pool never grows with callers.
    streams: Vec<Stream>,
    description: ComputeDevice,
    /// Background startup probe gate; awaited before the device runs a round.
    probe_gate: Arc<ProbeGate>,
}

impl ResolvedDevice {
    fn ensure_probed(&self) -> Result<(), ComputeError> {
        self.probe_gate.wait()
    }
}

/// Rounds in flight on a hardware GPU. One CPU runtime round already runs a
/// worker per core, so the CPU runtime has one slot.
const GPU_SLOTS: usize = 4;

/// Configure once, before native workers. Repeating the same settings is idempotent;
/// changing any preference after initialization refuses, including after failed initialization.
/// This blocking operation must not run on an async executor worker.
pub fn configure(settings: ComputeSettings) -> Result<ComputeStatus, ComputeError> {
    let _guard = INITIALIZATION.lock().map_err(|_| ComputeError::Poisoned)?;
    if let Some(session) = SESSION.get() {
        if session.settings != settings {
            return Err(ComputeError::AlreadyConfigured);
        }
        return session
            .context
            .as_ref()
            .map(|context| context.status.clone())
            .map_err(Clone::clone);
    }
    let context = initialize(settings);
    let result = context
        .as_ref()
        .map(|context| context.status.clone())
        .map_err(Clone::clone);
    let _ = SESSION.set(Session { settings, context });
    result
}

/// Read the immutable session status, lazily choosing defaults for direct native callers.
pub fn status() -> Result<ComputeStatus, ComputeError> {
    Ok(context()?.status.clone())
}

fn context() -> Result<&'static Context, ComputeError> {
    if SESSION.get().is_none() {
        let _guard = INITIALIZATION.lock().map_err(|_| ComputeError::Poisoned)?;
        SESSION.get_or_init(|| Session {
            settings: ComputeSettings::default(),
            context: initialize(ComputeSettings::default()),
        });
    }
    SESSION
        .get()
        .expect("session initialized")
        .context
        .as_ref()
        .map_err(Clone::clone)
}

fn initialize(settings: ComputeSettings) -> Result<Context, ComputeError> {
    guarded(|| {
        let default = resolve(settings.default_backend, None, false)?;
        let fea = match settings.fea_backend {
            None => default.clone(),
            Some(backend) => resolve(backend, Some(&default), false)?,
        };
        let status = ComputeStatus {
            settings,
            default_device: default.description.clone(),
            fea_device: fea.description.clone(),
        };
        status.validate().map_err(ComputeError::Initialization)?;
        let want_cpu_helper = settings.default_backend == ComputeBackend::Auto
            && default.description.backend == ComputeBackend::Vulkan
            && default.description.hardware_accelerated;
        let helper = Arc::new(OnceLock::new());
        let mut leaders = vec![vec![false; default.streams.len()]];
        if want_cpu_helper {
            leaders.push(vec![false; 1]);
        }
        // Run exact-predicate probes (and optional CPU helper JIT warmup) on a
        // background thread so engine startup overlaps shader compilation with
        // RPC/SQLite/STEP work and read-only return jobs never pay GPU warmup.
        let default_warm = default.clone();
        let fea_warm = if std::ptr::eq(
            Arc::as_ptr(&fea.probe_gate),
            Arc::as_ptr(&default.probe_gate),
        ) {
            None
        } else {
            Some(fea.clone())
        };
        let helper_slot = Arc::clone(&helper);
        let _ = std::thread::Builder::new()
            .name("spiling-compute-warmup".into())
            .spawn(move || {
                let result = probe(&default_warm);
                default_warm.probe_gate.finish(result);
                if let Some(fea_dev) = fea_warm {
                    let result = probe(&fea_dev);
                    fea_dev.probe_gate.finish(result);
                }
                if want_cpu_helper && let Ok(cpu) = resolve(ComputeBackend::Cpu, None, true) {
                    let _ = helper_slot.set(cpu);
                }
            });
        let pending = Pending {
            leaders,
            ..Pending::default()
        };
        Ok(Context {
            status,
            stage_devices: 1,
            devices: vec![default],
            helper,
            _fea: fea,
            failure: Mutex::new(None),
            pending: Mutex::new(pending),
            wake: Condvar::new(),
        })
    })
    .map_err(|error| match error {
        ComputeError::Execution(message) => ComputeError::Initialization(message),
        other => other,
    })
}

fn resolve(
    backend: ComputeBackend,
    previous: Option<&ResolvedDevice>,
    probe_now: bool,
) -> Result<ResolvedDevice, ComputeError> {
    let hardware = [
        WgpuDeviceKind::DiscreteGpu(0),
        WgpuDeviceKind::IntegratedGpu(0),
    ];
    let software = [
        WgpuDeviceKind::VirtualGpu(0),
        WgpuDeviceKind::Other(0),
        WgpuDeviceKind::Cpu,
    ];
    let mut selected_setup = None;
    if backend != ComputeBackend::Cpu {
        let choices = hardware.into_iter().chain(software.into_iter().take(
            if backend == ComputeBackend::Vulkan {
                3
            } else {
                0
            },
        ));
        for kind in choices {
            if let Some(previous) = previous
                && previous.description.backend == ComputeBackend::Vulkan
                && previous.description.hardware_accelerated
                    == matches!(
                        kind,
                        WgpuDeviceKind::DiscreteGpu(_) | WgpuDeviceKind::IntegratedGpu(_)
                    )
                && cubecl::Device::vulkan(kind.clone()).is_ok()
            {
                return Ok(previous.clone());
            }
            let device = WgpuDevice::new(kind).on(WgpuBackend::Vulkan);
            let options = RuntimeOptions::with_tasks_max(Some(1))
                .map_err(|error| ComputeError::Initialization(bounded_message(error)))?;
            if let Ok(setup) = cubecl::wgpu::try_init_setup::<Vulkan>(&device, options) {
                selected_setup = Some((device, setup));
                break;
            }
        }
    }
    if backend == ComputeBackend::Vulkan && selected_setup.is_none() {
        return Err(ComputeError::Initialization(
            "no Vulkan adapter is available".into(),
        ));
    }
    if selected_setup.is_none()
        && let Some(previous) = previous
        && previous.description.backend == ComputeBackend::Cpu
    {
        return Ok(previous.clone());
    }
    let resolved = if let Some((device, setup)) = selected_setup {
        let info = setup.adapter.get_info();
        let hardware_accelerated = matches!(
            info.device_type,
            cubecl::wgpu::wgpu::DeviceType::DiscreteGpu
                | cubecl::wgpu::wgpu::DeviceType::IntegratedGpu
        );
        let slots = if hardware_accelerated { GPU_SLOTS } else { 1 };
        ResolvedDevice {
            client: cubecl::Device::Wgpu(device).client(),
            streams: (0..slots).map(|_| Stream::new()).collect(),
            description: ComputeDevice {
                backend: ComputeBackend::Vulkan,
                name: device_name(&info.name),
                hardware_accelerated,
            },
            probe_gate: if probe_now {
                ProbeGate::completed(Ok(()))
            } else {
                ProbeGate::pending()
            },
        }
    } else {
        let client = cubecl::Device::Cpu(Default::default()).client();
        let name = device_name(&client.properties().identity.name);
        ResolvedDevice {
            client,
            streams: vec![Stream::new()],
            description: ComputeDevice {
                backend: ComputeBackend::Cpu,
                name,
                hardware_accelerated: false,
            },
            probe_gate: if probe_now {
                ProbeGate::completed(Ok(()))
            } else {
                ProbeGate::pending()
            },
        }
    };
    if probe_now {
        probe(&resolved)?;
    }
    Ok(resolved)
}

fn device_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|character| *character != '\0')
        .scan(0usize, |bytes, character| {
            *bytes += character.len_utf8();
            (*bytes <= 256).then_some(character)
        })
        .collect();
    if name.is_empty() {
        "Unnamed compute device".into()
    } else {
        name
    }
}

fn probe(device: &ResolvedDevice) -> Result<(), ComputeError> {
    let cancel = AtomicBool::new(false);
    let values = encode_values(&[[-1.0, 1.0], [1.0, 2.0]], 32, &cancel)?;
    let queries = encode_queries(&[[0.0]], &cancel)?;
    let stream = device.streams[0];
    let result = guarded(|| {
        stream.enter(|| {
            guarded(|| {
                kernels::masks_dispatch(
                    &device.client,
                    device.description.backend == ComputeBackend::Cpu,
                    0,
                    &values,
                    &queries,
                    &[0, 0, 2, 1],
                    &[0, 1],
                )
            })
        })
    });
    if result.is_err() {
        let _ = drain(device, stream);
    }
    let words = result?;
    if words != [1] {
        return Err(ComputeError::Initialization(
            "exact predicate initialization probe failed".into(),
        ));
    }
    Ok(())
}

pub(crate) fn bounded_message(message: impl std::fmt::Display) -> String {
    message.to_string().chars().take(512).collect()
}

fn guarded<T>(operation: impl FnOnce() -> Result<T, ComputeError>) -> Result<T, ComputeError> {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("native compute runtime panicked");
        Err(ComputeError::Execution(bounded_message(message)))
    })
}

fn drain(device: &ResolvedDevice, stream: Stream) -> Result<(), ComputeError> {
    guarded(|| {
        stream.enter(|| {
            cubecl::future::block_on(device.client.sync())
                .map_err(|error| ComputeError::Execution(bounded_message(error)))
        })
    })
}

/// Encode `queries.len()` closed or half-open interval rows on the caller's thread.
pub(crate) fn interval_group(
    shape: Shape,
    values: &[[f64; 2]],
    queries: &[f64],
    cancel: &AtomicBool,
    kind: u32,
) -> Result<MaskGroup, ComputeError> {
    let values = encode_values(values, shape.words_per_row * 32, cancel)?;
    let mut encoded = zeroed_words(queries.len() * 2)?;
    for (index, query) in queries.iter().enumerate() {
        if index % 1_024 == 0 {
            check_cancel(cancel)?;
        }
        encoded[index * 2..index * 2 + 2].copy_from_slice(&ordered_key(*query));
    }
    Ok(MaskGroup {
        kind,
        shape,
        values,
        queries: encoded,
    })
}

/// Encode box queries on the caller's thread.
pub(crate) fn box_group(
    shape: Shape,
    values: &[[f64; 4]],
    queries: &[[f64; 4]],
    cancel: &AtomicBool,
) -> Result<MaskGroup, ComputeError> {
    Ok(MaskGroup {
        kind: 1,
        shape,
        values: encode_values(values, shape.words_per_row * 32, cancel)?,
        queries: encode_queries(queries, cancel)?,
    })
}

fn lock_pending(context: &Context) -> Result<MutexGuard<'_, Pending>, ComputeError> {
    context.pending.lock().map_err(|_| ComputeError::Poisoned)
}

/// Run `groups` in the next round that starts, alongside every other queued
/// request. Results are per group and exact, so batching never changes them.
pub(crate) fn execute_masks(
    groups: Vec<MaskGroup>,
    cancel: &AtomicBool,
) -> Result<Vec<MaskRows>, ComputeError> {
    check_cancel(cancel)?;
    if groups.iter().all(|group| group.shape.words == 0) {
        return groups
            .into_iter()
            .map(|group| group.shape.empty())
            .collect();
    }
    masks_in(context()?, groups, cancel)
}

fn masks_in(
    context: &Context,
    groups: Vec<MaskGroup>,
    cancel: &AtomicBool,
) -> Result<Vec<MaskRows>, ComputeError> {
    let mut pending = lock_pending(context)?;
    let id = pending.next;
    pending.next += 1;
    pending.queue.push((id, groups));
    loop {
        if let Some(index) = pending.done.iter().position(|(done, _)| *done == id) {
            return pending.done.swap_remove(index).1;
        }
        if check_cancel(cancel).is_err() {
            if let Some(index) = pending.queue.iter().position(|(queued, _)| *queued == id) {
                pending.queue.remove(index);
            } else {
                pending.abandoned.push(id);
            }
            return Err(ComputeError::Cancelled);
        }
        if let Some(slot) = free_slot(&pending, context.total_devices()) {
            pending = lead(context, pending, slot, |_, _| Ok(()))?.0;
            continue;
        }
        pending = context
            .wake
            .wait_timeout(pending, Duration::from_millis(1))
            .map_err(|_| ComputeError::Poisoned)?
            .0;
    }
}

/// The first free slot among the first `devices`, earlier devices first: a
/// helper leads only while every slot ahead of it is busy.
fn free_slot(pending: &Pending, devices: usize) -> Option<(usize, usize)> {
    pending.leaders[..devices]
        .iter()
        .enumerate()
        .find_map(|(device, slots)| {
            slots
                .iter()
                .position(|busy| !busy)
                .map(|slot| (device, slot))
        })
}

/// Run `work` on the first free slot in turn order, after every queued
/// request; `work` may borrow the caller's data.
pub(crate) fn execute_exclusive<T: Send>(
    cancel: &AtomicBool,
    work: impl FnOnce(&Client, bool) -> Result<T, ComputeError> + Send,
) -> Result<T, ComputeError> {
    exclusive_in(context()?, cancel, work)
}

fn exclusive_in<T: Send>(
    context: &Context,
    cancel: &AtomicBool,
    work: impl FnOnce(&Client, bool) -> Result<T, ComputeError> + Send,
) -> Result<T, ComputeError> {
    let mut pending = lock_pending(context)?;
    let id = pending.next;
    pending.next += 1;
    pending.turns.push_back(id);
    loop {
        if let Err(error) = check_cancel(cancel) {
            pending.turns.retain(|turn| *turn != id);
            context.wake.notify_all();
            return Err(error);
        }
        if pending.turns.front() == Some(&id)
            && let Some(slot) = free_slot(&pending, context.stage_devices)
        {
            pending.turns.pop_front();
            // The next turn may take another free slot now.
            context.wake.notify_all();
            let (_pending, result) = lead(context, pending, slot, work)?;
            return result;
        }
        pending = context
            .wake
            .wait_timeout(pending, Duration::from_millis(1))
            .map_err(|_| ComputeError::Poisoned)?
            .0;
    }
}

/// Lead one round: take every queued request, run them and then `work`,
/// publish every result and wake the waiters.
fn lead<'context, T: Send>(
    context: &'context Context,
    mut pending: MutexGuard<'context, Pending>,
    (device, slot): (usize, usize),
    work: impl FnOnce(&Client, bool) -> Result<T, ComputeError> + Send,
) -> Result<(MutexGuard<'context, Pending>, Result<T, ComputeError>), ComputeError> {
    pending.leaders[device][slot] = true;
    let queued = std::mem::take(&mut pending.queue);
    drop(pending);
    let ids: Vec<u64> = queued.iter().map(|(id, _)| *id).collect();
    let (masks, own) = round(context, (device, slot), queued, work);
    let mut pending = lock_pending(context)?;
    pending.leaders[device][slot] = false;
    let masks: Vec<Result<Vec<MaskRows>, ComputeError>> = match masks {
        Ok(masks) => masks.into_iter().map(Ok).collect(),
        Err(error) => ids.iter().map(|_| Err(error.clone())).collect(),
    };
    for (id, result) in ids.into_iter().zip(masks) {
        if let Some(index) = pending.abandoned.iter().position(|gone| *gone == id) {
            pending.abandoned.swap_remove(index);
        } else {
            pending.done.push((id, result));
        }
    }
    context.wake.notify_all();
    Ok((pending, own))
}

type RoundResult<T> = (
    Result<Vec<Vec<MaskRows>>, ComputeError>,
    Result<T, ComputeError>,
);

fn round<T: Send>(
    context: &Context,
    (device, slot): (usize, usize),
    queued: Vec<(u64, Vec<MaskGroup>)>,
    work: impl FnOnce(&Client, bool) -> Result<T, ComputeError> + Send,
) -> RoundResult<T> {
    // Checked and recorded briefly, so rounds on two devices overlap.
    match context.failure.lock() {
        Ok(failure) => {
            if let Some(error) = failure.as_ref() {
                return (Err(error.clone()), Err(error.clone()));
            }
        }
        Err(_) => return (Err(ComputeError::Poisoned), Err(ComputeError::Poisoned)),
    }
    let device = context.device(device);
    if let Err(error) = device.ensure_probed() {
        if let Ok(mut failure) = context.failure.lock() {
            failure.get_or_insert_with(|| error.clone());
        }
        return (Err(error.clone()), Err(error));
    }
    let stream = device.streams[slot];
    let cpu = device.description.backend == ComputeBackend::Cpu;
    let mut own = Err(ComputeError::Execution("round did not run".into()));
    let masks = guarded(|| {
        stream.enter(|| {
            guarded(|| {
                let masks = run_masks(&device.client, cpu, queued);
                own = guarded(|| work(&device.client, cpu));
                masks
            })
        })
    });
    let (masks, own) = if masks.is_err() || own.is_err() {
        match drain(device, stream) {
            Err(error) => (Err(error.clone()), Err(error)),
            Ok(()) => (masks, own),
        }
    } else {
        (masks, own)
    };
    for result in [masks.as_ref().err(), own.as_ref().err()] {
        if let Some(error @ ComputeError::Execution(_)) = result
            && let Ok(mut failure) = context.failure.lock()
        {
            failure.get_or_insert_with(|| error.clone());
        }
    }
    (masks, own)
}

/// Pack every queued group into segments of whole rows, one launch per
/// segment and kind, and split the words back per group.
fn run_masks(
    client: &Client,
    cpu: bool,
    queued: Vec<(u64, Vec<MaskGroup>)>,
) -> Result<Vec<Vec<MaskRows>>, ComputeError> {
    let groups: Vec<&MaskGroup> = queued.iter().flat_map(|(_, groups)| groups).collect();
    let mut words: Vec<Vec<u32>> = Vec::new();
    words
        .try_reserve_exact(groups.len())
        .map_err(|_| ComputeError::Allocation)?;
    for group in &groups {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(group.shape.words)
            .map_err(|_| ComputeError::Allocation)?;
        words.push(buffer);
    }
    let index = |value: usize| {
        u32::try_from(value).map_err(|_| ComputeError::InvalidInput("mask input exceeds u32"))
    };
    for kind in 0..3 {
        // (group, first row, end row), each at most one segment of words.
        let mut entries = Vec::new();
        for (at, group) in groups.iter().enumerate() {
            if group.kind != kind || group.shape.words == 0 {
                continue;
            }
            let step = (MASK_SEGMENT_WORDS / group.shape.words_per_row).max(1);
            let mut row = 0;
            while row < group.shape.rows {
                let end = (row + step).min(group.shape.rows);
                entries.push((at, row, end));
                row = end;
            }
        }
        let mut next = 0;
        while next < entries.len() {
            let (mut values, mut queries, mut table) = (Vec::new(), Vec::new(), Vec::new());
            let mut starts = vec![0u32];
            let mut total = 0usize;
            let mut last = None;
            let first = next;
            while next < entries.len() {
                let (at, row, end) = entries[next];
                let group = groups[at];
                let length = (end - row) * group.shape.words_per_row;
                let fresh = last != Some(at);
                let added = if fresh { group.values.len() } else { 0 };
                if next > first
                    && (total + length > MASK_SEGMENT_WORDS
                        || values.len() + added > MASK_SEGMENT_INPUT)
                {
                    break;
                }
                if fresh {
                    values.extend_from_slice(&group.values);
                    last = Some(at);
                }
                let fields = group.queries.len() / group.shape.rows;
                let value_base = index((values.len() - group.values.len()) / 4)?;
                let query_base = index(queries.len())?;
                queries.extend_from_slice(&group.queries[row * fields..end * fields]);
                table.extend_from_slice(&[
                    value_base,
                    query_base,
                    index(group.shape.columns)?,
                    index(group.shape.words_per_row)?,
                ]);
                total += length;
                starts.push(index(total)?);
                next += 1;
            }
            index(values.len())?;
            let output =
                kernels::masks_dispatch(client, cpu, kind, &values, &queries, &table, &starts)?;
            for (offset, &(at, _, _)) in entries[first..next].iter().enumerate() {
                let range = starts[offset] as usize..starts[offset + 1] as usize;
                words[at].extend_from_slice(&output[range]);
            }
        }
    }
    let mut words = words.into_iter();
    let mut results = Vec::new();
    for (_, groups) in &queued {
        let mut masks = Vec::new();
        for group in groups {
            let words = words.next().unwrap_or_default();
            if words.len() != group.shape.words {
                return Err(ComputeError::Execution(
                    "mask segments differ from the shape".into(),
                ));
            }
            masks.push(MaskRows {
                rows: group.shape.rows,
                columns: group.shape.columns,
                words_per_row: group.shape.words_per_row,
                words,
            });
        }
        results.push(masks);
    }
    Ok(results)
}

/// Jobs and output words one uploaded capsule segment holds at most, unless
/// a single group is larger; larger batches run as several segments.
const CAPSULE_SEGMENT_JOBS: usize = 1 << 22;
const CAPSULE_SEGMENT_WORDS: usize = 1 << 24;

/// Group windows are `[row0, rows, word0, width]` in 64-cell words.
///
/// Segments are independent device turns: up to one worker per device slot
/// packs, runs and unpacks them at once, so one large stage batch keeps every
/// GPU slot (and then the CPU helper) busy. Results return in group order.
pub(crate) fn execute_capsule_masks(
    capsules: &[[i64; 5]],
    rows: &[[u32; 2]],
    group_ends: &[usize],
    windows: &[[u32; 4]],
    cell: i64,
    columns: i64,
    cancel: &AtomicBool,
) -> Result<Vec<crate::CellMask>, ComputeError> {
    let group_jobs = |group: usize| -> usize {
        let first = if group == 0 { 0 } else { group_ends[group - 1] };
        rows[first..group_ends[group]]
            .iter()
            .map(|&[_, count]| count as usize)
            .sum()
    };
    let group_words = |group: usize| windows[group][1] as usize * windows[group][3] as usize * 2;
    // Whole groups per segment, up to the segment's bounds.
    let mut segments: Vec<(usize, usize, usize)> = Vec::new();
    let mut group = 0usize;
    while group < group_ends.len() {
        check_cancel(cancel)?;
        let first_group = group;
        let (mut jobs, mut words) = (0usize, 0usize);
        while group < group_ends.len()
            && (group == first_group
                || (jobs + group_jobs(group) <= CAPSULE_SEGMENT_JOBS
                    && words + group_words(group) <= CAPSULE_SEGMENT_WORDS))
        {
            jobs += group_jobs(group);
            words += group_words(group);
            group += 1;
        }
        segments.push((first_group, group, words));
    }
    let run = |&(first_group, end_group, words): &(usize, usize, usize)| {
        capsule_segment(
            capsules,
            rows,
            group_ends,
            windows,
            (first_group, end_group, words),
            cell,
            columns,
            cancel,
        )
    };
    let workers = segments.len().min(slot_count()?);
    let results: Vec<SegmentMasks> = if workers <= 1 {
        segments.iter().map(run).collect()
    } else {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let slots: Vec<Mutex<Option<SegmentMasks>>> =
            segments.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let at = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if at >= segments.len() || failed.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            return;
                        }
                        let result = run(&segments[at]);
                        if result.is_err() {
                            failed.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        if let Ok(mut slot) = slots[at].lock() {
                            *slot = Some(result);
                        }
                    }
                });
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .map_err(|_| ComputeError::Poisoned)?
                    .unwrap_or(Err(ComputeError::Cancelled))
            })
            .collect()
    };
    let mut masks = Vec::new();
    masks
        .try_reserve_exact(group_ends.len())
        .map_err(|_| ComputeError::Allocation)?;
    // Workers take segments in order, so any segment skipped after a
    // failure comes after it: the first failure in order is the one reported.
    for result in results {
        masks.extend(result?);
    }
    check_cancel(cancel)?;
    Ok(masks)
}

type SegmentMasks = Result<Vec<crate::CellMask>, ComputeError>;

/// Slots that take stage batches: how many segments can be in flight at once.
fn slot_count() -> Result<usize, ComputeError> {
    let context = context()?;
    Ok(context.devices[..context.stage_devices]
        .iter()
        .map(|device| device.streams.len())
        .sum())
}

/// Pack, run and unpack the capsule groups `first_group..end_group` as one
/// device turn.
#[allow(clippy::too_many_arguments)]
fn capsule_segment(
    capsules: &[[i64; 5]],
    rows: &[[u32; 2]],
    group_ends: &[usize],
    windows: &[[u32; 4]],
    (first_group, group, words): (usize, usize, usize),
    cell: i64,
    columns: i64,
    cancel: &AtomicBool,
) -> SegmentMasks {
    check_cancel(cancel)?;
    let capsule_first = if first_group == 0 {
        0
    } else {
        group_ends[first_group - 1]
    };
    let capsule_end = group_ends[group - 1];
    let mut segment_windows = Vec::new();
    let mut capsule_windows = Vec::new();
    let mut starts = vec![0u32];
    let mut first_rows = Vec::new();
    let (mut base, mut total) = (0u32, 0u32);
    let mut previous = capsule_first;
    for (local, at) in (first_group..group).enumerate() {
        let [row0, rows_in, word0, width] = windows[at];
        segment_windows.extend_from_slice(&[base, row0, rows_in, word0 * 2, width * 2]);
        base = rows_in
            .checked_mul(width)
            .and_then(|cells| cells.checked_mul(2))
            .and_then(|cells| cells.checked_add(base))
            .ok_or(ComputeError::Allocation)?;
        for &[row, count] in &rows[previous..group_ends[at]] {
            if count == 0 {
                continue;
            }
            total = total.checked_add(count).ok_or(ComputeError::Allocation)?;
            starts.push(total);
            first_rows.push(row);
            capsule_windows.push(local as u32);
        }
        previous = group_ends[at];
    }
    let output = if first_rows.is_empty() {
        Vec::new()
    } else {
        let kept: Vec<i64> = capsules[capsule_first..capsule_end]
            .iter()
            .zip(&rows[capsule_first..capsule_end])
            .filter(|(_, row)| row[1] > 0)
            .flat_map(|(capsule, _)| *capsule)
            .collect();
        // Each segment is its own device turn: packing and
        // unpacking stay on this thread, and a short request queued
        // behind a long batch waits at most one segment.
        execute_exclusive(cancel, |client, cpu| {
            kernels::capsule_masks_dispatch(
                client,
                cpu,
                &kept,
                &starts,
                &first_rows,
                &capsule_windows,
                &segment_windows,
                words,
                cell,
                columns,
                cancel,
            )
        })?
    };
    let mut masks = Vec::new();
    masks
        .try_reserve_exact(group - first_group)
        .map_err(|_| ComputeError::Allocation)?;
    for (local, at) in (first_group..group).enumerate() {
        let [row0, rows_in, word0, width] = windows[at];
        let base = segment_windows[local * 5] as usize;
        let length = rows_in as usize * width as usize;
        let mut words = Vec::new();
        words
            .try_reserve_exact(length)
            .map_err(|_| ComputeError::Allocation)?;
        // Windows start on whole 64-bit words: `base` counts 32-bit ones.
        let window = output.get(base / 2..base / 2 + length).ok_or_else(|| {
            ComputeError::Execution("capsule readback is shorter than its windows".into())
        })?;
        words.extend_from_slice(window);
        masks.push(crate::CellMask {
            row0,
            rows: rows_in,
            word0,
            width,
            words,
        });
    }
    Ok(masks)
}

fn encode_values<const FIELDS: usize>(
    values: &[[f64; FIELDS]],
    padded_columns: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u32>, ComputeError> {
    let mut encoded = zeroed_words(padded_columns * FIELDS * 2)?;
    for (index, value) in values.iter().enumerate() {
        if index % 1_024 == 0 {
            check_cancel(cancel)?;
        }
        for (field, coordinate) in value.iter().enumerate() {
            let key = ordered_key(*coordinate);
            encoded[field * 2 * padded_columns + index] = key[0];
            encoded[(field * 2 + 1) * padded_columns + index] = key[1];
        }
    }
    Ok(encoded)
}

fn encode_queries<const FIELDS: usize>(
    queries: &[[f64; FIELDS]],
    cancel: &AtomicBool,
) -> Result<Vec<u32>, ComputeError> {
    let mut encoded = zeroed_words(queries.len() * FIELDS * 2)?;
    for (index, query) in queries.iter().enumerate() {
        if index % 1_024 == 0 {
            check_cancel(cancel)?;
        }
        for (field, coordinate) in query.iter().enumerate() {
            encoded[index * FIELDS * 2 + field * 2..index * FIELDS * 2 + field * 2 + 2]
                .copy_from_slice(&ordered_key(*coordinate));
        }
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    #[test]
    fn cancelled_waiter_leaves_the_queue_while_another_round_leads() {
        configure(ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: None,
        })
        .unwrap();
        let context = context().unwrap();
        // Take the leader role the way a round does, so no real round resets it.
        // Every device, so the waiter cannot lead a round of its own.
        let mut taken: Vec<Vec<bool>> = context
            .devices
            .iter()
            .map(|device| vec![false; device.streams.len()])
            .collect();
        while taken.iter().flatten().any(|taken| !taken) {
            let mut pending = lock_pending(context).unwrap();
            for (device, slots) in taken.iter_mut().enumerate() {
                for (slot, taken) in slots.iter_mut().enumerate() {
                    if !*taken && !pending.leaders[device][slot] {
                        pending.leaders[device][slot] = true;
                        *taken = true;
                    }
                }
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (started, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            crate::row_crossings(&[[0.0, 1.0]], &[0.5], &worker_cancel)
                .err()
                .unwrap()
        });
        receive.recv().unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(worker.join().unwrap(), ComputeError::Cancelled);
        let mut pending = lock_pending(context).unwrap();
        assert!(pending.abandoned.is_empty());
        pending
            .leaders
            .iter_mut()
            .flatten()
            .for_each(|busy| *busy = false);
        context.wake.notify_all();
    }

    #[test]
    fn caller_threads_share_the_slot_streams() {
        configure(ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: None,
        })
        .unwrap();
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..4 {
                workers.push(scope.spawn(|| {
                    let result = crate::row_crossings(
                        &[[0.0, 1.0]; 65],
                        &[0.5; 65],
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                    for row in 0..65 {
                        assert_eq!(result.selected_columns(row).unwrap().count(), 65);
                    }
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
    }

    #[test]
    fn concurrent_groups_of_every_kind_match_their_single_runs() {
        configure(ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: None,
        })
        .unwrap();
        let cancel = AtomicBool::new(false);
        let ranges: Vec<[f64; 2]> = (0..70)
            .map(|i| [f64::from(i), f64::from(i) + 2.5])
            .collect();
        let boxes: Vec<[f64; 4]> = (0..40)
            .map(|i| [f64::from(i), 0.0, f64::from(i) + 1.0, 1.0])
            .collect();
        let expected_crossings: Vec<_> = (0..12)
            .map(|rows| {
                let coordinates: Vec<f64> = (0..rows).map(|row| f64::from(row) * 3.5).collect();
                let group = interval_group(
                    Shape::new(coordinates.len(), ranges.len()).unwrap(),
                    &ranges,
                    &coordinates,
                    &cancel,
                    0,
                )
                .unwrap();
                (coordinates, group)
            })
            .collect();
        let queries: Vec<[f64; 4]> = (0..9)
            .map(|i| [f64::from(i) * 4.0, 0.5, f64::from(i) * 4.0 + 0.5, 0.5])
            .collect();
        let mut groups = Vec::new();
        for (coordinates, _) in &expected_crossings {
            for kind in [0, 2] {
                groups.push(
                    interval_group(
                        Shape::new(coordinates.len(), ranges.len()).unwrap(),
                        &ranges,
                        coordinates,
                        &cancel,
                        kind,
                    )
                    .unwrap(),
                );
            }
            groups.push(
                box_group(
                    Shape::new(queries.len(), boxes.len()).unwrap(),
                    &boxes,
                    &queries,
                    &cancel,
                )
                .unwrap(),
            );
        }
        let batched = execute_masks(groups, &cancel).unwrap();
        for (index, (coordinates, _)) in expected_crossings.iter().enumerate() {
            let single = crate::row_crossings(&ranges, coordinates, &cancel).unwrap();
            let closed = crate::interval_membership(&ranges, coordinates, &cancel).unwrap();
            let boxed = crate::box_overlaps(&boxes, &queries, &cancel).unwrap();
            assert_eq!(batched[index * 3], single);
            assert_eq!(batched[index * 3 + 1], closed);
            assert_eq!(batched[index * 3 + 2], boxed);
        }
    }

    #[test]
    fn a_busy_device_hands_rounds_to_the_next_with_identical_results() {
        let cpu = || resolve(ComputeBackend::Cpu, None, true).unwrap();
        let first = cpu();
        let status = ComputeStatus {
            settings: ComputeSettings {
                default_backend: ComputeBackend::Cpu,
                fea_backend: None,
            },
            default_device: first.description.clone(),
            fea_device: first.description.clone(),
        };
        let context = Context {
            status,
            _fea: first.clone(),
            devices: vec![first, cpu()],
            helper: Arc::new(OnceLock::new()),
            stage_devices: 2,
            failure: Mutex::new(None),
            pending: Mutex::new(Pending {
                leaders: vec![vec![false], vec![false]],
                ..Pending::default()
            }),
            wake: Condvar::new(),
        };
        let cancel = AtomicBool::new(false);
        let ranges: Vec<[f64; 2]> = (0..90)
            .map(|i| [f64::from(i), f64::from(i) + 4.5])
            .collect();
        let coordinates: Vec<f64> = (0..33).map(|i| f64::from(i) * 2.75).collect();
        let group = || {
            interval_group(
                Shape::new(coordinates.len(), ranges.len()).unwrap(),
                &ranges,
                &coordinates,
                &cancel,
                0,
            )
            .unwrap()
        };
        let free = masks_in(&context, vec![group()], &cancel).unwrap();
        // Device 0 stays busy, so both kinds of work must lead on device 1.
        lock_pending(&context).unwrap().leaders[0][0] = true;
        let handed = masks_in(&context, vec![group()], &cancel).unwrap();
        assert_eq!(handed, free);
        let second = &context.devices[1].client as *const Client as usize;
        let device = exclusive_in(&context, &cancel, |client, _| {
            Ok(client as *const Client as usize == second)
        })
        .unwrap();
        assert!(device, "exclusive work ran on the free device");
        lock_pending(&context).unwrap().leaders[0][0] = false;
    }
}
