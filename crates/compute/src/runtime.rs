// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError, atomic::AtomicBool};
use std::time::Duration;

use cubecl::prelude::Client;
use cubecl::wgpu::{RuntimeOptions, Vulkan, WgpuBackend, WgpuDevice, WgpuDeviceKind};
use cubecl_environment::stream::Stream;

use crate::{
    ComputeBackend, ComputeDevice, ComputeError, ComputeSettings, ComputeStatus, MaskRows, Shape,
    TILE_WORDS, check_cancel, kernels, ordered_key, zeroed_words,
};

static SESSION: OnceLock<Session> = OnceLock::new();
static INITIALIZATION: Mutex<()> = Mutex::new(());

struct Session {
    settings: ComputeSettings,
    context: Result<Context, ComputeError>,
}

pub(crate) struct Context {
    status: ComputeStatus,
    default: ResolvedDevice,
    _fea: ResolvedDevice,
    dispatch: Mutex<Option<ComputeError>>,
}

#[derive(Clone)]
struct ResolvedDevice {
    client: Client,
    stream: Stream,
    description: ComputeDevice,
}

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
        let default = resolve(settings.default_backend, None)?;
        let fea = match settings.fea_backend {
            None => default.clone(),
            Some(backend) => resolve(backend, Some(&default))?,
        };
        let status = ComputeStatus {
            settings,
            default_device: default.description.clone(),
            fea_device: fea.description.clone(),
        };
        status.validate().map_err(ComputeError::Initialization)?;
        Ok(Context {
            status,
            default,
            _fea: fea,
            dispatch: Mutex::new(None),
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
) -> Result<ResolvedDevice, ComputeError> {
    let hardware = [
        WgpuDeviceKind::DiscreteGpu(0),
        WgpuDeviceKind::IntegratedGpu(0),
    ];
    let selected = if backend == ComputeBackend::Cpu {
        None
    } else {
        let software = [
            WgpuDeviceKind::VirtualGpu(0),
            WgpuDeviceKind::Other(0),
            WgpuDeviceKind::Cpu,
        ];
        let mut choices = hardware.into_iter().chain(software.into_iter().take(
            if backend == ComputeBackend::Vulkan {
                3
            } else {
                0
            },
        ));
        choices.find(|kind| cubecl::Device::vulkan(kind.clone()).is_ok())
    };
    if backend == ComputeBackend::Vulkan && selected.is_none() {
        return Err(ComputeError::Initialization(
            "no Vulkan adapter is available".into(),
        ));
    }
    if let Some(previous) = previous {
        let same = match &selected {
            None => previous.description.backend == ComputeBackend::Cpu,
            Some(kind) => {
                previous.description.backend == ComputeBackend::Vulkan
                    && previous.description.hardware_accelerated
                        == matches!(
                            kind,
                            WgpuDeviceKind::DiscreteGpu(_) | WgpuDeviceKind::IntegratedGpu(_)
                        )
            }
        };
        if same {
            return Ok(previous.clone());
        }
    }
    let stream = Stream::new();
    let resolved = if let Some(kind) = selected {
        let device = WgpuDevice::new(kind).on(WgpuBackend::Vulkan);
        let setup = cubecl::wgpu::try_init_setup::<Vulkan>(
            &device,
            RuntimeOptions::with_tasks_max(Some(1))
                .map_err(|error| ComputeError::Initialization(bounded_message(error)))?,
        )
        .map_err(|error| ComputeError::Initialization(bounded_message(error)))?;
        let info = setup.adapter.get_info();
        let hardware_accelerated = matches!(
            info.device_type,
            cubecl::wgpu::wgpu::DeviceType::DiscreteGpu
                | cubecl::wgpu::wgpu::DeviceType::IntegratedGpu
        );
        ResolvedDevice {
            client: cubecl::Device::Wgpu(device).client(),
            stream,
            description: ComputeDevice {
                backend: ComputeBackend::Vulkan,
                name: device_name(&info.name),
                hardware_accelerated,
            },
        }
    } else {
        let client = cubecl::Device::Cpu(Default::default()).client();
        let name = device_name(&client.properties().identity.name);
        ResolvedDevice {
            client,
            stream,
            description: ComputeDevice {
                backend: ComputeBackend::Cpu,
                name,
                hardware_accelerated: false,
            },
        }
    };
    probe(&resolved)?;
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
    let result = guarded(|| {
        device.stream.enter(|| {
            device.client.memory_dedicated_allocation((), |_| {
                guarded(|| {
                    let batch = kernels::UploadedBatch::new(
                        &device.client,
                        device.description.backend == ComputeBackend::Cpu,
                        &values,
                        &queries,
                        2,
                        0,
                    )?;
                    batch.dispatch(0, 1)
                })
            })
        })
    });
    let drained = drain(device);
    let words = result?;
    drained?;
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

fn drain(device: &ResolvedDevice) -> Result<(), ComputeError> {
    guarded(|| {
        device.stream.enter(|| {
            cubecl::future::block_on(device.client.sync())
                .map_err(|error| ComputeError::Execution(bounded_message(error)))
        })
    })
}

fn lock_dispatch<'context>(
    context: &'context Context,
    cancel: &AtomicBool,
) -> Result<MutexGuard<'context, Option<ComputeError>>, ComputeError> {
    loop {
        check_cancel(cancel)?;
        match context.dispatch.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => return Err(ComputeError::Poisoned),
            Err(TryLockError::WouldBlock) => std::thread::park_timeout(Duration::from_millis(1)),
        }
    }
}

pub(crate) fn execute(
    shape: Shape,
    values: &[[f64; 2]],
    queries: &[f64],
    cancel: &AtomicBool,
    kind: u32,
) -> Result<Option<MaskRows>, ComputeError> {
    execute_with(shape, cancel, kind, || {
        let values = encode_values(values, shape.words_per_row * 32, cancel)?;
        let mut encoded_queries = zeroed_words(queries.len() * 2)?;
        for (index, query) in queries.iter().enumerate() {
            if index % 1_024 == 0 {
                check_cancel(cancel)?;
            }
            encoded_queries[index * 2..index * 2 + 2].copy_from_slice(&ordered_key(*query));
        }
        Ok((values, encoded_queries))
    })
}

pub(crate) fn execute_boxes(
    shape: Shape,
    values: &[[f64; 4]],
    queries: &[[f64; 4]],
    cancel: &AtomicBool,
) -> Result<Option<MaskRows>, ComputeError> {
    execute_with(shape, cancel, 1, || {
        Ok((
            encode_values(values, shape.words_per_row * 32, cancel)?,
            encode_queries(queries, cancel)?,
        ))
    })
}

fn execute_with(
    shape: Shape,
    cancel: &AtomicBool,
    kind: u32,
    encode: impl FnOnce() -> Result<(Vec<u32>, Vec<u32>), ComputeError> + Send,
) -> Result<Option<MaskRows>, ComputeError> {
    let context = context()?;
    let mut failure = lock_dispatch(context, cancel)?;
    if let Some(error) = failure.as_ref() {
        return Err(error.clone());
    }
    let result = guarded(|| {
        context.default.stream.enter(|| {
            context.default.client.memory_dedicated_allocation((), |_| {
                guarded(|| {
                    check_cancel(cancel)?;
                    let (values, queries) = encode()?;
                    check_cancel(cancel)?;
                    let batch = kernels::UploadedBatch::new(
                        &context.default.client,
                        context.default.description.backend == ComputeBackend::Cpu,
                        &values,
                        &queries,
                        shape.columns,
                        kind,
                    )?;
                    let mut words = zeroed_words(shape.words)?;
                    for (tile_index, tile) in words.chunks_mut(TILE_WORDS).enumerate() {
                        check_cancel(cancel)?;
                        let result = batch.dispatch(tile_index * TILE_WORDS, tile.len())?;
                        tile.copy_from_slice(&result);
                    }
                    check_cancel(cancel)?;
                    Ok(Some(MaskRows {
                        rows: shape.rows,
                        columns: shape.columns,
                        words_per_row: shape.words_per_row,
                        words,
                    }))
                })
            })
        })
    });
    let drained = drain(&context.default);
    let result = match (result, drained) {
        (_, Err(error)) => Err(error),
        (result, Ok(())) => result.and_then(|value| {
            check_cancel(cancel)?;
            Ok(value)
        }),
    };
    if let Err(error @ ComputeError::Execution(_)) = &result {
        *failure = Some(error.clone());
    }
    result
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
    fn cancelled_waiter_does_not_need_the_dispatch_gate() {
        configure(ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: None,
        })
        .unwrap();
        let context = context().unwrap();
        let gate = context.dispatch.lock().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (started, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            lock_dispatch(context, &worker_cancel).err().unwrap()
        });
        receive.recv().unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(worker.join().unwrap(), ComputeError::Cancelled);
        drop(gate);
    }

    #[test]
    fn caller_threads_share_one_stream_and_complete_serially() {
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
                    .unwrap()
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
    fn cancellation_after_encoding_refuses_before_upload() {
        configure(ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: None,
        })
        .unwrap();
        let cancel = AtomicBool::new(false);
        let shape = Shape::admit(65, 65).unwrap();
        let result = execute_with(shape, &cancel, 0, || {
            cancel.store(true, Ordering::Relaxed);
            Ok((Vec::new(), Vec::new()))
        });
        assert_eq!(result, Err(ComputeError::Cancelled));
    }
}
