// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Host admission belongs to the engine, not source validity or Core persistence.
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub const SOURCE_MEMORY_ENV: &str = "SPILING_SOURCE_MEMORY_BYTES";
pub const MANUFACTURING_MEMORY_ENV: &str = "SPILING_MANUFACTURING_MEMORY_BYTES";

pub fn source_memory_limit(value: Option<std::ffi::OsString>) -> Result<Option<usize>, String> {
    memory_limit(SOURCE_MEMORY_ENV, value)
}

pub fn manufacturing_memory_limit(
    value: Option<std::ffi::OsString>,
) -> Result<Option<usize>, String> {
    memory_limit(MANUFACTURING_MEMORY_ENV, value)
}

fn memory_limit(name: &str, value: Option<std::ffi::OsString>) -> Result<Option<usize>, String> {
    value
        .map(|value| {
            value
                .to_str()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("{name} must be a positive integer byte allowance"))
        })
        .transpose()
}

/// Operator override for the allocator arena count, read by glibc itself.
const MALLOC_ARENA_ENV: &str = "MALLOC_ARENA_MAX";
/// glibc arenas shared by the engine's worker threads.
const ENGINE_MALLOC_ARENAS: i32 = 4;

/// Bound glibc's per-thread allocator arenas before worker threads start.
///
/// glibc gives each concurrently allocating thread its own arena (up to eight
/// per core) and rarely returns their freed pages. Parallel per-layer planning
/// on a wide machine then keeps several times its live working set resident,
/// which also shrinks the headroom later admissions observe. Four shared
/// arenas keep that parallel planning's throughput while its resident peak
/// tracks live data. This tunes allocation reuse; it refuses no work.
pub fn configure_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if std::env::var_os(MALLOC_ARENA_ENV).is_none() {
        // SAFETY: mallopt only adjusts glibc allocator tuning; it is called at
        // startup and has no Rust-visible memory effects.
        unsafe {
            libc::mallopt(libc::M_ARENA_MAX, ENGINE_MALLOC_ARENAS);
        }
    }
}

fn resource(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}

#[derive(Debug, Clone, Copy)]
struct Headroom {
    available: u64,
    container: Option<u64>,
    address_space: Option<u64>,
}
impl Headroom {
    fn growth(self, retained: usize, selected: Option<usize>) -> usize {
        // Keep half of currently available RAM for opaque native transients,
        // the workbench and other applications. This is additional live memory,
        // not a ceiling on total job size or a second charge for existing pins.
        let available = self
            .available
            .min(self.container.unwrap_or(u64::MAX))
            .min(self.address_space.unwrap_or(u64::MAX));
        usize::try_from(available / 2)
            .unwrap_or(usize::MAX)
            .min(selected.map_or(usize::MAX, |maximum| maximum.saturating_sub(retained)))
    }

    fn current() -> Result<Self, GeometryError> {
        let pid = sysinfo::get_current_pid()
            .map_err(|_| resource("cannot observe engine memory headroom"))?;
        let mut system = System::new();
        system.refresh_memory();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let process = system
            .process(pid)
            .ok_or_else(|| resource("cannot observe engine process memory"))?;
        if system.total_memory() == 0 {
            return Err(resource("system memory observation is unavailable"));
        }
        // Process-specific cgroups include ancestor limits. System-wide cgroup
        // information alone misses a nested service/container's actual bound.
        let container = process.cgroup_limits().map(|limits| limits.free_memory);
        #[cfg(unix)]
        let address_space = {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: getrlimit only writes the initialized caller-owned structure.
            if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) } == 0
                && limit.rlim_cur != libc::RLIM_INFINITY
            {
                Some(limit.rlim_cur.saturating_sub(process.virtual_memory()))
            } else {
                None
            }
        };
        #[cfg(not(unix))]
        let address_space = None;
        Ok(Self {
            available: system.available_memory(),
            container,
            address_space,
        })
    }
}

/// Fresh additional source/proof capacity. Existing current/history/candidate
/// allocations are already resident in the OS observation; only the explicitly
/// selected total source allowance subtracts their accounted bytes again.
pub fn source_growth(retained: usize) -> Result<usize, GeometryError> {
    let selected = source_memory_limit(std::env::var_os(SOURCE_MEMORY_ENV)).map_err(resource)?;
    let headroom = Headroom::current()?;
    let growth = headroom.growth(retained, selected);
    crate::diagnostic(
        "info",
        "source_memory_admission",
        &format!(
            "retained_bytes={retained} growth_bytes={growth} available_bytes={} container_headroom={:?} address_headroom={:?} selected_bytes={selected:?}",
            headroom.available, headroom.container, headroom.address_space,
        ),
    );
    Ok(growth)
}

/// Fresh live bytes one staged ByteStream transfer may admit. Earlier staged
/// transfers are already resident in the OS observation.
pub fn transfer_growth() -> Result<usize, GeometryError> {
    Ok(Headroom::current()?.growth(0, None))
}

/// Fresh bytes retained project state (manufacturing options across
/// current/history/prepared snapshots) may grow by. An unobservable host
/// admits no growth rather than guessing.
pub fn retained_growth() -> usize {
    Headroom::current().map_or(0, |headroom| headroom.growth(0, None))
}

/// A Core project whose retained-state growth is admitted against this host.
pub fn observed_project(mut project: spiling_core::Project) -> spiling_core::Project {
    project.observe_retained_growth(retained_growth);
    project
}

/// Live working set one manufacturing operation (compile, alternative,
/// preflight, inspection) may admit, observed fresh when the job starts.
/// Inputs, sources and previous artifacts are already resident in the OS
/// observation; the planner and verifier count only their own growth.
pub fn manufacturing_working_set()
-> Result<spiling_manufacturing::WorkingSet, spiling_contracts::manufacturing::ManufacturingError> {
    use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
    let refuse =
        |message: String| ManufacturingError::new(ManufacturingErrorCode::ResourceLimit, message);
    let selected =
        manufacturing_memory_limit(std::env::var_os(MANUFACTURING_MEMORY_ENV)).map_err(refuse)?;
    let headroom = Headroom::current().map_err(|error| refuse(error.message))?;
    let growth = headroom.growth(0, selected);
    crate::diagnostic(
        "info",
        "manufacturing_memory_admission",
        &format!(
            "working_set_bytes={growth} available_bytes={} container_headroom={:?} address_headroom={:?} selected_bytes={selected:?}",
            headroom.available, headroom.container, headroom.address_space,
        ),
    );
    spiling_manufacturing::WorkingSet::new(growth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_follows_free_memory_container_and_selected_total_without_double_charging() {
        let host = Headroom {
            available: 96 << 30,
            container: None,
            address_space: None,
        };
        assert_eq!(host.growth(8 << 30, None), 48 << 30);
        assert_eq!(host.growth(8 << 30, Some(12 << 30)), 4 << 30);
        assert_eq!(host.growth(12 << 30, Some(8 << 30)), 0);
        let constrained = Headroom {
            container: Some(2 << 30),
            ..host
        };
        assert_eq!(constrained.growth(8 << 30, None), 1 << 30);
        assert_eq!(
            Headroom {
                address_space: Some(800),
                ..constrained
            }
            .growth(0, None),
            400
        );
        assert_eq!(
            Headroom {
                container: Some(0),
                ..host
            }
            .growth(0, None),
            0
        );
        assert!(
            Headroom {
                available: 8 << 30,
                ..host
            }
            .growth(0, None)
                < host.growth(0, None)
        );
    }

    #[test]
    fn explicit_source_allowance_is_optional_and_invalid_configuration_is_not_ignored() {
        assert_eq!(source_memory_limit(None).unwrap(), None);
        assert_eq!(
            source_memory_limit(Some("128000000".into())).unwrap(),
            Some(128000000)
        );
        for value in [
            "",
            "0",
            "-1",
            "1.5",
            "lots",
            "999999999999999999999999999999",
        ] {
            assert!(source_memory_limit(Some(value.into())).is_err());
            assert!(manufacturing_memory_limit(Some(value.into())).is_err());
        }
        assert_eq!(
            manufacturing_memory_limit(Some("128000000".into())).unwrap(),
            Some(128000000)
        );
    }

    #[test]
    fn current_process_memory_observation_has_no_fixed_small_allowance() {
        let snapshot = Headroom::current().unwrap();
        assert!(snapshot.available > 0);
        let growth = snapshot.growth(0, None);
        assert!(growth as u64 <= snapshot.available / 2);
        if let Some(container) = snapshot.container {
            assert!(growth as u64 <= container / 2);
        }
    }
}
