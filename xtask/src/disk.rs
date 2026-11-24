// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Host disk retention for the shared development volume.
//!
//! Everything under the work root is disposable except source in worktrees,
//! pinned corpus baselines (named in a `KEEP` file) and small receipts. The
//! rules live here, once, so agents and timers apply the same policy.

use crate::Result;
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const USAGE: &str = "cargo xtask disk [--apply] [--root WORK_DIR] [--checkout DIR]...\nDry run by default: lists what the retention policy would delete.";

#[cfg(test)]
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// A worktree target nobody has built in this long is rebuilt from sccache.
const IDLE_TARGET: Duration = Duration::from_secs(2 * 24 * 60 * 60);
/// Files in a checkout's `.cache` at least this large are outputs, not receipts.
const LARGE_OUTPUT_BYTES: u64 = 2 * 1024 * 1024;
const STALE_OUTPUT: Duration = Duration::from_secs(2 * 24 * 60 * 60);
/// Corpus runs not pinned in a `KEEP` file.
const STALE_CORPUS: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// Profiling binary caches (`cache/fp-*`) kept, newest first.
const KEPT_PROFILING_CACHES: usize = 3;
const STALE_STAGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Heavy jobs refuse to start below this much free space on the work volume.
pub(crate) const FREE_FLOOR_BYTES: u64 = 60 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub bytes: u64,
    pub reason: &'static str,
}

pub(crate) fn execute(current: &Path, args: &[OsString]) -> Result<()> {
    let mut apply = false;
    let mut root = None;
    let mut checkouts = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.to_str() {
            Some("--apply") => apply = true,
            Some("--root") => root = Some(PathBuf::from(rest.next().ok_or(USAGE)?)),
            Some("--checkout") => checkouts.push(PathBuf::from(rest.next().ok_or(USAGE)?)),
            _ => return Err(USAGE.into()),
        }
    }
    let root = match root.or_else(|| env::var_os("SPILING_WORK").map(PathBuf::from)) {
        Some(root) => root,
        None => PathBuf::from(env::var_os("HOME").ok_or("HOME is not configured")?).join("work"),
    };
    let now = SystemTime::now();
    let mut candidates = work_root(&root, current, now)?;
    for checkout in &checkouts {
        candidates.extend(checkout_outputs(checkout, current, now)?);
    }
    let total: u64 = candidates.iter().map(|c| c.bytes).sum();
    for candidate in &candidates {
        println!(
            "{:>9}  {:<28} {}",
            human(candidate.bytes),
            candidate.reason,
            candidate.path.display()
        );
    }
    if apply {
        for candidate in &candidates {
            remove(&candidate.path)?;
        }
        println!("removed {} in {} entries", human(total), candidates.len());
    } else {
        println!(
            "would remove {} in {} entries (dry run; pass --apply)",
            human(total),
            candidates.len()
        );
    }
    if let Some(free) = free_bytes(&root) {
        println!(
            "free on {}: {} (floor {})",
            root.display(),
            human(free),
            human(FREE_FLOOR_BYTES)
        );
    }
    Ok(())
}

/// Applies the policy to the work root's fixed layout:
/// `worktrees/<repo>/<task>`, `corpus/`, `cache/`.
pub(crate) fn work_root(root: &Path, current: &Path, now: SystemTime) -> Result<Vec<Candidate>> {
    let mut found = Vec::new();
    for repo in children(&root.join("worktrees"))? {
        for worktree in children(&repo)? {
            if is_dir(&worktree) {
                found.extend(checkout_outputs(&worktree, current, now)?);
            }
        }
    }
    corpus_group(&root.join("corpus"), now, &mut found)?;
    let cache = root.join("cache");
    let mut profiling: Vec<(SystemTime, PathBuf)> = children(&cache)?
        .into_iter()
        .filter(|p| name(p).starts_with("fp-") && is_dir(p))
        .map(|p| (newest(&p, 2), p))
        .collect();
    profiling.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in profiling.into_iter().skip(KEPT_PROFILING_CACHES) {
        found.push(candidate(path, "profiling cache beyond newest")?);
    }
    for stage in children(&cache.join("stages"))? {
        if age(now, newest(&stage, 1)) > STALE_STAGE {
            found.push(candidate(stage, "stage capture older than 7d")?);
        }
    }
    Ok(found)
}

/// Idle Cargo targets and large stale `.cache` outputs inside one checkout.
/// The checkout running this task keeps its own target.
pub(crate) fn checkout_outputs(
    checkout: &Path,
    current: &Path,
    now: SystemTime,
) -> Result<Vec<Candidate>> {
    let mut found = Vec::new();
    for entry in children(checkout)? {
        let target = name(&entry);
        let is_target = (target == "target" || target.starts_with("target-"))
            && entry.join("CACHEDIR.TAG").is_file();
        if is_target && !same(checkout, current) && age(now, newest(&entry, 3)) > IDLE_TARGET {
            found.push(candidate(entry, "idle cargo target (>2d)")?);
        }
    }
    let mut outputs = Vec::new();
    large_files(&checkout.join(".cache"), now, &mut outputs)?;
    found.extend(outputs);
    Ok(found)
}

fn large_files(dir: &Path, now: SystemTime, found: &mut Vec<Candidate>) -> Result<()> {
    for entry in children(dir)? {
        let meta = fs::symlink_metadata(&entry)?;
        if meta.is_dir() {
            large_files(&entry, now, found)?;
        } else if meta.is_file()
            && meta.len() >= LARGE_OUTPUT_BYTES
            && age(now, meta.modified()?) > STALE_OUTPUT
        {
            found.push(Candidate {
                path: entry,
                bytes: meta.len(),
                reason: "large .cache output (>2d)",
            });
        }
    }
    Ok(())
}

/// A corpus group is `corpus/` or any directory in it holding a `KEEP` file.
/// `KEEP` pins children (one pattern per line, `#` comments); unpinned
/// children idle past the corpus window are removed. Pinned groups recurse.
fn corpus_group(dir: &Path, now: SystemTime, found: &mut Vec<Candidate>) -> Result<()> {
    let keep = fs::read_to_string(dir.join("KEEP")).unwrap_or_default();
    let pinned: Vec<&str> = keep
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    for entry in children(dir)? {
        let entry_name = name(&entry);
        if entry_name == "KEEP" || entry_name.ends_with(".lock") {
            continue;
        }
        if pinned.iter().any(|pattern| pins(pattern, &entry_name)) {
            if entry.join("KEEP").is_file() {
                corpus_group(&entry, now, found)?;
            }
            continue;
        }
        if age(now, newest(&entry, 2)) > STALE_CORPUS {
            found.push(candidate(entry, "unpinned corpus run (>3d)")?);
        }
    }
    Ok(())
}

/// `KEEP` lines are exact names, `prefix*` or `*suffix`.
fn pins(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix)
    } else {
        pattern == name
    }
}

fn candidate(path: PathBuf, reason: &'static str) -> Result<Candidate> {
    Ok(Candidate {
        bytes: size(&path)?,
        path,
        reason,
    })
}

fn children(dir: &Path) -> Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(entries) => {
            let mut paths = entries
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{}: {error}", dir.display()).into()),
    }
}

fn is_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn same(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn age(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or_default()
}

/// The newest modification time at most `depth` levels below `path`,
/// without following symlinks. Cargo and corpus writers touch these levels.
fn newest(path: &Path, depth: usize) -> SystemTime {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return SystemTime::UNIX_EPOCH;
    };
    let mut latest = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if meta.is_dir() && depth > 0 {
        for child in children(path).unwrap_or_default() {
            latest = latest.max(newest(&child, depth - 1));
        }
    }
    latest
}

fn size(path: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for child in children(path)? {
        total += size(&child)?;
    }
    Ok(total)
}

fn remove(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    let result = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|e| format!("{}: {e}", path.display()).into())
}

#[cfg(unix)]
// statvfs field types differ by platform; on Linux they are already u64.
#[allow(clippy::useless_conversion)]
pub(crate) fn free_bytes(path: &Path) -> Option<u64> {
    let stat = nix::sys::statvfs::statvfs(path).ok()?;
    let blocks = u64::try_from(stat.blocks_available()).ok()?;
    blocks.checked_mul(u64::try_from(stat.fragment_size()).ok()?)
}

#[cfg(not(unix))]
pub(crate) fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

fn human(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.1}G", bytes as f64 / GIB)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn write(path: &Path, bytes: usize, age_days: u64, now: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; bytes]).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(now - DAY * age_days as u32)
            .unwrap();
    }

    fn age_dir(path: &Path, age_days: u64, now: SystemTime) {
        File::open(path)
            .unwrap()
            .set_modified(now - DAY * age_days as u32)
            .unwrap();
    }

    #[test]
    fn policy_keeps_pins_receipts_fresh_work_and_the_current_target() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let now = SystemTime::now();
        let tree = root.join("worktrees/spiling");
        // Idle target in another worktree goes; the current one stays.
        for wt in ["idle", "current"] {
            write(&tree.join(wt).join("target/CACHEDIR.TAG"), 1, 5, now);
            write(&tree.join(wt).join("target/debug/x"), 1, 5, now);
            age_dir(&tree.join(wt).join("target/debug"), 5, now);
            age_dir(&tree.join(wt).join("target"), 5, now);
        }
        // Large stale output goes; small receipt and fresh output stay.
        let cache = tree.join("idle/.cache/study");
        write(&cache.join("big.bin"), 3 * 1024 * 1024, 5, now);
        write(&cache.join("fresh.bin"), 3 * 1024 * 1024, 0, now);
        write(&cache.join("receipt.json"), 100, 30, now);
        // Corpus: pinned baseline stays, old run goes, fresh run stays,
        // pinned group prunes inside itself.
        let corpus = root.join("corpus");
        fs::create_dir_all(&corpus).unwrap();
        fs::write(corpus.join("KEEP"), "baseline*\npanel\n").unwrap();
        write(&corpus.join("baseline/a"), 1, 30, now);
        write(&corpus.join("baseline.log"), 1, 30, now);
        write(&corpus.join("panel/run.sh"), 1, 30, now);
        write(&corpus.join("old/a"), 1, 30, now);
        write(&corpus.join("old.log"), 1, 30, now);
        write(&corpus.join("new/a"), 1, 0, now);
        write(&corpus.join("heavy.lock"), 0, 30, now);
        fs::create_dir_all(corpus.join("panel")).unwrap();
        fs::write(corpus.join("panel/KEEP"), "bin\n*.sh\n").unwrap();
        write(&corpus.join("panel/bin/x"), 1, 30, now);
        write(&corpus.join("panel/run-1/a"), 1, 30, now);
        for dir in ["baseline", "old", "panel/bin", "panel/run-1"] {
            age_dir(&corpus.join(dir), 30, now);
        }

        let found = work_root(root, &tree.join("current"), now).unwrap();
        let mut paths: Vec<String> = found
            .iter()
            .map(|c| c.path.strip_prefix(root).unwrap().display().to_string())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "corpus/old",
                "corpus/old.log",
                "corpus/panel/run-1",
                "worktrees/spiling/idle/.cache/study/big.bin",
                "worktrees/spiling/idle/target",
            ]
        );
    }

    #[test]
    fn newest_profiling_caches_are_kept() {
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        for (i, name) in ["fp-a", "fp-b", "fp-c", "fp-d"].iter().enumerate() {
            let dir = root.path().join("cache").join(name);
            write(&dir.join("bin"), 1, i as u64 + 1, now);
            age_dir(&dir, i as u64 + 1, now);
        }
        let found = work_root(root.path(), root.path(), now).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("cache/fp-d"));
    }
}
