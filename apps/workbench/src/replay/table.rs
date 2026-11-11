// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Shared compact CPU table; an anonymous spool is backing, never part validity.
use super::beads::Instance;
use spiling_contracts::geometry::SourceHash;
use std::{io::Write, sync::Arc};
#[derive(Debug)]
enum Storage {
    Resident(Arc<Vec<Instance>>),
    Backed(spiling_chunks::ChunkSource),
}
#[derive(Debug)]
pub struct Table {
    storage: Storage,
    len: usize,
    width: [f32; 2],
    height: [f32; 2],
}
impl Table {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            storage: Storage::Resident(Arc::new(Vec::with_capacity(capacity))),
            len: 0,
            width: [f32::INFINITY, f32::NEG_INFINITY],
            height: [f32::INFINITY, f32::NEG_INFINITY],
        }
    }
    pub fn push(&mut self, instance: Instance) {
        let Storage::Resident(data) = &mut self.storage else {
            unreachable!("sealed table")
        };
        Arc::get_mut(data).unwrap().push(instance);
        self.len += 1;
        self.width = [
            self.width[0].min(instance.width),
            self.width[1].max(instance.width),
        ];
        self.height = [
            self.height[0].min(instance.height),
            self.height[1].max(instance.height),
        ];
    }
    pub fn statistics(&self) -> super::beads::Statistics {
        super::beads::Statistics {
            count: self.len,
            width: self.width,
            height: self.height,
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn capacity(&self) -> usize {
        match &self.storage {
            Storage::Resident(data) => data.capacity(),
            Storage::Backed(_) => 0,
        }
    }
    pub fn retained_bytes(&self) -> usize {
        match &self.storage {
            Storage::Resident(data) => data.capacity() * std::mem::size_of::<Instance>(),
            Storage::Backed(source) => source.retained_bytes(),
        }
    }
    pub fn spool(&mut self, spool: &spiling_chunks::Spool) -> Result<(), String> {
        let Storage::Resident(data) = &self.storage else {
            return Ok(());
        };
        let mut writer = spiling_chunks::ChunkWriter::spooled_in(u64::MAX, spool);
        for instance in data.iter() {
            for value in instance
                .start
                .into_iter()
                .chain([instance.width])
                .chain(instance.end)
                .chain([instance.height])
            {
                writer
                    .write_all(&value.to_le_bytes())
                    .map_err(|e| e.to_string())?;
            }
            writer
                .write_all(&instance.class.to_le_bytes())
                .map_err(|e| e.to_string())?;
        }
        self.storage = Storage::Backed(writer.finish().map_err(|e| e.to_string())?);
        Ok(())
    }
    pub fn load(&self) -> Result<Arc<Vec<Instance>>, String> {
        match &self.storage {
            Storage::Resident(data) => Ok(data.clone()),
            Storage::Backed(source) => {
                let bytes = source.to_vec().map_err(|e| e.to_string())?;
                if bytes.len() != self.len * 36
                    || SourceHash::from_bytes(&bytes) != SourceHash::from_digest(source.digest())
                {
                    return Err("compact display backing checksum mismatch".into());
                }
                let mut instances = Vec::new();
                instances
                    .try_reserve_exact(self.len)
                    .map_err(|e| e.to_string())?;
                for r in bytes.as_chunks::<36>().0 {
                    let value = |at| f32::from_le_bytes(r[at..at + 4].try_into().unwrap());
                    instances.push(Instance {
                        start: [value(0), value(4), value(8)],
                        width: value(12),
                        end: [value(16), value(20), value(24)],
                        height: value(28),
                        class: u32::from_le_bytes(r[32..36].try_into().unwrap()),
                    });
                }
                Ok(Arc::new(instances))
            }
        }
    }
}

/// Fresh headroom for additional CPU retention and upload staging. Required
/// source allocations already appear in the OS/container observation.
pub fn available_growth() -> usize {
    let selected = std::env::var("SPILING_REPLAY_RESIDENT_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let mut available = system.available_memory();
    if let Ok(pid) = sysinfo::get_current_pid() {
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_memory(),
        );
        if let Some(process) = system.process(pid) {
            if let Some(container) = process.cgroup_limits() {
                available = available.min(container.free_memory);
            }
            #[cfg(unix)]
            {
                let mut limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                // SAFETY: getrlimit writes only the initialized caller-owned value.
                if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) } == 0
                    && limit.rlim_cur != libc::RLIM_INFINITY
                {
                    available =
                        available.min(limit.rlim_cur.saturating_sub(process.virtual_memory()));
                }
            }
        }
    }
    usize::try_from(available / 2)
        .unwrap_or(usize::MAX)
        .min(selected.unwrap_or(usize::MAX))
}
