// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    chunks,
    execution::Execution,
    model::{Candidate, cancelled, digest_bytes, hash, snapshot_id},
    schema,
    storage_os::{Location, Parent, Spool, Stage},
    vfs::Guard,
    *,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

struct Database {
    connection: Connection,
    #[cfg(test)]
    _fault: Option<crate::fault_vfs::FaultVfs>,
    _guard: Guard,
}

impl Database {
    fn open(
        location: Arc<Location>,
        read_only: bool,
        creating: bool,
        profile: DocumentProfile,
    ) -> Result<Self> {
        let guard = Guard::new(location.clone(), !read_only, creating)?;
        #[cfg(test)]
        let fault = crate::fault_vfs::FaultVfs::for_current_thread(guard.name())?;
        #[cfg(test)]
        let vfs_name = fault.as_ref().map_or(guard.name(), |fault| fault.name());
        #[cfg(not(test))]
        let vfs_name = guard.name();
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else if creating {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags_and_vfs(&location.path, flags, vfs_name)?;
        let cancel = AtomicBool::new(false);
        let mut execution = Execution::new(&connection, &cancel);
        schema::configure(&connection, read_only)?;
        if creating {
            schema::create(&connection, profile)?;
        } else {
            schema::admit(&connection, profile)?;
            let result: String =
                connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
            if result != "ok" {
                return Err(invalid("SQLite structural check failed"));
            }
        }
        if !read_only {
            connection.execute_batch(&format!(
                "PRAGMA max_page_count={};",
                MAX_DATABASE_BYTES / 4096
            ))?;
        }
        connection.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
            use rusqlite::hooks::{AuthAction, Authorization};
            match context.action {
                AuthAction::Read { table_name, .. }
                | AuthAction::Insert { table_name }
                | AuthAction::Delete { table_name }
                | AuthAction::Update { table_name, .. }
                    if [
                        "container",
                        "chunks",
                        "objects",
                        "object_chunks",
                        "snapshots",
                        "snapshot_objects",
                        "sqlite_master",
                    ]
                    .contains(&table_name) =>
                {
                    Authorization::Allow
                }
                AuthAction::Select
                | AuthAction::Transaction { .. }
                | AuthAction::Savepoint { .. } => Authorization::Allow,
                AuthAction::Function { function_name }
                    if ["length", "count", "sum", "coalesce"].contains(&function_name) =>
                {
                    Authorization::Allow
                }
                AuthAction::Pragma {
                    pragma_name: "table_xinfo" | "foreign_key_list" | "index_list",
                    pragma_value: Some(table),
                } if schema::TABLES.contains(&table) => Authorization::Allow,
                AuthAction::Pragma {
                    pragma_name: "index_xinfo",
                    pragma_value: Some(index),
                } if schema::INDEXES.contains(&index) => Authorization::Allow,
                AuthAction::Pragma {
                    pragma_name: "quick_check" | "integrity_check",
                    pragma_value: Some("1"),
                } => Authorization::Allow,
                AuthAction::Pragma {
                    pragma_name,
                    pragma_value: None,
                } if [
                    "application_id",
                    "user_version",
                    "page_size",
                    "journal_mode",
                    "auto_vacuum",
                    "quick_check",
                    "integrity_check",
                    "foreign_key_check",
                    "page_count",
                    "freelist_count",
                ]
                .contains(&pragma_name) =>
                {
                    Authorization::Allow
                }
                _ => Authorization::Deny,
            }
        }))?;
        execution.finish();
        Ok(Self {
            connection,
            #[cfg(test)]
            _fault: fault,
            _guard: guard,
        })
    }
}

pub struct Container {
    profile: DocumentProfile,
    database: Mutex<Option<Database>>,
    location: Arc<Location>,
    lease: Option<File>,
    poisoned: AtomicBool,
}

impl std::fmt::Debug for Container {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Container")
            .field("path", &self.location.path)
            .field("read_only", &self.read_only())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationOutcome {
    PublishedConfirmed,
    PublishedUnconfirmed,
    OutcomeUnknown,
}

#[derive(Debug)]
pub struct Publication {
    pub container: Arc<Container>,
    pub outcome: PublicationOutcome,
    pub heads: Heads,
    pub error: Option<Error>,
}

pub struct Capture<'a> {
    execution: Execution<'a>,
    database: DatabaseGuard<'a>,
    location: &'a Location,
    pub heads: Heads,
    pub selected: SnapshotId,
    pub root: Vec<u8>,
    pub guard: Vec<u8>,
    pub members: Vec<Member>,
}

impl Drop for Capture<'_> {
    fn drop(&mut self) {
        self.execution.finish();
        let _ = self.database.connection.execute_batch("ROLLBACK");
        self.location.readers.fetch_sub(1, Ordering::AcqRel);
    }
}

struct DatabaseGuard<'a>(MutexGuard<'a, Option<Database>>);

impl std::ops::Deref for DatabaseGuard<'_> {
    type Target = Database;
    fn deref(&self) -> &Database {
        self.0.as_ref().expect("checked live database")
    }
}

impl Capture<'_> {
    pub fn object_length(&self, object: ObjectId) -> Result<usize> {
        object_length(&self.database.connection, object)
    }
    pub fn read_object(
        &self,
        object: ObjectId,
        budget: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        if !self.members.iter().any(|member| member.object == object) {
            return Err(invalid("object is not a selected snapshot member"));
        }
        read_object(&self.database.connection, object, budget, cancel)
    }
    /// Stream one selected member's verified raw chunks in order. Each chunk is
    /// checked against its own identity before `consume`; the whole-object
    /// digest and coverage are checked last, so a consumer must discard its
    /// output if this returns an error.
    pub fn stream_object(
        &self,
        object: ObjectId,
        budget: usize,
        cancel: &AtomicBool,
        mut consume: impl FnMut(&[u8]) -> std::io::Result<()>,
    ) -> Result<()> {
        if !self.members.iter().any(|member| member.object == object) {
            return Err(invalid("object is not a selected snapshot member"));
        }
        stream_object(&self.database.connection, object, budget, cancel, |raw| {
            consume(raw).map_err(Error::Io)
        })
    }
    pub fn verify_all(&self, cancel: &AtomicBool) -> Result<()> {
        for object in self
            .members
            .iter()
            .map(|member| member.object)
            .collect::<BTreeSet<_>>()
        {
            stream_object(
                &self.database.connection,
                object,
                MAX_OBJECT_BYTES,
                cancel,
                |_| Ok(()),
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageStats {
    pub current_raw_bytes: u64,
    pub previous_only_raw_bytes: u64,
    pub reachable_payload_bytes: u64,
    pub unreachable_payload_bytes: u64,
    pub database_bytes: u64,
    pub free_page_bytes: u64,
    pub journal_bytes: u64,
    pub snapshots: u64,
    pub objects: u64,
    pub chunks: u64,
    pub previous_only_payload_bytes: u64,
    pub scratch_bytes: u64,
    pub scratch_entries: u64,
    pub membership_rows: u64,
    pub mapping_rows: u64,
    pub retained_roots: u64,
    pub limits: StorageLimits,
}

impl Container {
    pub fn path(&self) -> &Path {
        &self.location.path
    }
    pub fn matches_path(&self, path: &Path) -> Result<bool> {
        self.location.matches_path(path)
    }
    pub fn read_only(&self) -> bool {
        self.lease.is_none()
    }
    pub fn needs_reopen(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
    pub fn reopen(&self) -> Result<()> {
        self.location.check(false)?;
        if let Some(lease) = &self.lease {
            self.location.check_lease(lease)?;
        }
        let mut database = self.database.try_lock().map_err(|_| Error::Locked)?;
        self.poisoned.store(true, Ordering::Release);
        drop(database.take());
        *database = Some(Database::open(
            self.location.clone(),
            self.read_only(),
            false,
            self.profile,
        )?);
        self.poisoned.store(false, Ordering::Release);
        Ok(())
    }
    fn writable(&self) -> Result<()> {
        if self.read_only() {
            return Err(Error::ReadOnly);
        }
        if self.needs_reopen() {
            return Err(invalid(
                "uncertain attachment requires explicit reopen before writing",
            ));
        }
        Ok(())
    }
    fn lock(&self) -> Result<DatabaseGuard<'_>> {
        if let Some(lease) = &self.lease {
            self.location.check_lease(lease)?;
        }
        let guard = self.database.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => Error::Locked,
            _ => invalid("container mutex poisoned"),
        })?;
        if guard.is_none() {
            return Err(invalid(
                "published attachment unavailable; close and explicitly reopen",
            ));
        }
        Ok(DatabaseGuard(guard))
    }
    pub fn open_existing(path: &Path, read_only: bool) -> Result<Arc<Self>> {
        Self::open_existing_for_profile(path, read_only, DocumentProfile::Project)
    }
    pub fn open_existing_for_profile(
        path: &Path,
        read_only: bool,
        profile: DocumentProfile,
    ) -> Result<Arc<Self>> {
        let location = Location::attach(path, false)?;
        let lease = if read_only {
            None
        } else {
            Some(location.writer_lease()?)
        };
        let database = Database::open(location.clone(), read_only, false, profile)?;
        Ok(Arc::new(Self {
            profile,
            database: Mutex::new(Some(database)),
            location,
            lease,
            poisoned: AtomicBool::new(false),
        }))
    }
    pub fn capture<'a>(
        &'a self,
        selection: Selection,
        cancel: &'a AtomicBool,
    ) -> Result<Capture<'a>> {
        cancelled(cancel)?;
        self.location.check(false)?;
        if self.location.readers.fetch_add(1, Ordering::AcqRel) >= 2 {
            self.location.readers.fetch_sub(1, Ordering::AcqRel);
            return Err(limit("concurrent captures"));
        }
        let result = (|| {
            let database = self.lock()?;
            let mut execution = Execution::new(&database.connection, cancel);
            database.connection.execute_batch("BEGIN")?;
            let result = (|| {
                let (heads, guard) = read_heads(&database.connection, self.profile)?;
                let selected = match selection {
                    Selection::Current => heads.current,
                    Selection::Previous => heads.previous.ok_or(Error::NoPrevious)?,
                };
                let (root_id, members) = graph(&database.connection, selected)?;
                let root = read_object(&database.connection, root_id, MAX_ROOT_BYTES, cancel)?;
                Ok((heads, guard, selected, root, members))
            })();
            match result {
                Ok((heads, guard, selected, root, members)) => Ok(Capture {
                    execution,
                    database,
                    location: &self.location,
                    heads,
                    guard,
                    selected,
                    root,
                    members,
                }),
                Err(error) => {
                    execution.finish();
                    let _ = database.connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })();
        if result.is_err() {
            self.location.readers.fetch_sub(1, Ordering::AcqRel);
        }
        result
    }
    pub fn create_new(
        path: &Path,
        input: SnapshotInput<'_>,
        cancel: &AtomicBool,
        observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        Self::create(path, DocumentProfile::Project, input, None, cancel, observe)
    }
    pub fn create_new_retaining(
        path: &Path,
        input: SnapshotInput<'_>,
        previous: SnapshotInput<'_>,
        cancel: &AtomicBool,
        observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        Self::create(
            path,
            DocumentProfile::Project,
            input,
            Some(previous),
            cancel,
            observe,
        )
    }
    pub fn create_new_for_profile(
        path: &Path,
        profile: DocumentProfile,
        input: SnapshotInput<'_>,
        cancel: &AtomicBool,
        observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        Self::create(path, profile, input, None, cancel, observe)
    }
    fn create(
        path: &Path,
        profile: DocumentProfile,
        input: SnapshotInput<'_>,
        previous: Option<SnapshotInput<'_>>,
        cancel: &AtomicBool,
        mut observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        cancelled(cancel)?;
        let candidate = Candidate::checked(input)?;
        let previous = previous.map(Candidate::checked).transpose()?;
        let target = Location::attach(path, true)?;
        let lease = target.writer_lease()?;
        let stage = Stage::new(&target.parent)?;
        let mut staged = Staged::new(&candidate, &stage.location.parent, cancel)?;
        if !observe(SaveStage::Staged) {
            return Err(Error::Cancelled);
        }
        let database = Database::open(stage.location.clone(), false, true, profile)?;
        let mut execution = Execution::new(&database.connection, cancel);
        database.connection.execute_batch("BEGIN IMMEDIATE")?;
        insert(&database.connection, &candidate, &mut staged, cancel)?;
        drop(staged);
        let previous_id = previous
            .as_ref()
            .map(|previous| previous.id)
            .filter(|id| *id != candidate.id);
        if let Some(previous) = &previous {
            let mut staged_previous = Staged::new(previous, &stage.location.parent, cancel)?;
            insert(&database.connection, previous, &mut staged_previous, cancel)?;
        }
        database.connection.execute(
            "INSERT INTO container VALUES(1,?1,1,?2,?3,?4,?5)",
            params![
                profile.name(),
                &candidate.id.0[..],
                previous_id.as_ref().map(|id| &id.0[..]),
                candidate.guard,
                &hash(candidate.guard)[..]
            ],
        )?;
        cancelled(cancel)?;
        if !observe(SaveStage::BeforeCommit) {
            return Err(Error::Cancelled);
        }
        database.connection.execute_batch("COMMIT")?;
        if !observe(SaveStage::Committed) {
            return Err(Error::Cancelled);
        }
        execution.finish();
        drop(execution);
        drop(database);
        let checker = Database::open(stage.location.clone(), true, false, profile)?;
        let verification = Execution::new(&checker.connection, cancel);
        verify_snapshot(&checker.connection, candidate.id, cancel)?;
        if let Some(previous) = previous_id {
            verify_snapshot(&checker.connection, previous, cancel)?;
        }
        drop(verification);
        drop(checker);
        let heads = Heads {
            generation: 1,
            current: candidate.id,
            previous: previous_id,
        };
        Self::publish_stage(target, lease, stage, heads, profile, cancel, observe)
    }
    fn publish_stage(
        target: Arc<Location>,
        lease: File,
        stage: Stage,
        heads: Heads,
        profile: DocumentProfile,
        cancel: &AtomicBool,
        mut observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        stage.seal()?;
        cancelled(cancel)?;
        if !observe(SaveStage::BeforePublish) {
            return Err(Error::Cancelled);
        }
        stage.publish(&target.path, &target.parent)?;
        let observed = observe(SaveStage::Published);
        let mut durability = target
            .parent
            .sync()
            .and_then(|_| stage.location.parent.sync());
        if !observed && durability.is_ok() {
            durability = Err(invalid("post-publication observer failed"));
        }
        let (database, reopen_error) = match Database::open(target.clone(), false, false, profile) {
            Ok(database) => (Some(database), None),
            Err(error) => (None, Some(error)),
        };
        let unavailable = reopen_error.is_some();
        let container = Arc::new(Self {
            profile,
            database: Mutex::new(database),
            location: target,
            lease: Some(lease),
            poisoned: AtomicBool::new(unavailable),
        });
        Ok(Publication {
            container,
            heads,
            outcome: if unavailable {
                PublicationOutcome::PublishedUnconfirmed
            } else if durability.is_ok() {
                PublicationOutcome::PublishedConfirmed
            } else {
                PublicationOutcome::PublishedUnconfirmed
            },
            error: reopen_error.or_else(|| durability.err()),
        })
    }
    pub fn publish(
        self: &Arc<Self>,
        base: Heads,
        retain: Selection,
        input: SnapshotInput<'_>,
        cancel: &AtomicBool,
        mut observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        self.writable()?;
        cancelled(cancel)?;
        self.location.check(false)?;
        let candidate = Candidate::checked(input)?;
        let mut staged = Staged::new(&candidate, &self.location.parent, cancel)?;
        if !observe(SaveStage::Staged) {
            return Err(Error::Cancelled);
        }
        let database = self.lock()?;
        let connection = &database.connection;
        let mut execution = Execution::new(connection, cancel);
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let preparation = (|| {
            if read_heads(connection, self.profile)?.0 != base {
                return Err(Error::Conflict);
            }
            let retained = match retain {
                Selection::Current => base.current,
                Selection::Previous => base.previous.ok_or(Error::NoPrevious)?,
            };
            verify_snapshot(connection, retained, cancel)?;
            insert(connection, &candidate, &mut staged, cancel)?;
            let previous = if candidate.id == retained {
                base.previous.filter(|id| *id != candidate.id)
            } else {
                Some(retained)
            };
            if let Some(previous) = previous.filter(|id| *id != retained) {
                verify_snapshot(connection, previous, cancel)?;
            }
            let generation = base
                .generation
                .checked_add(1)
                .ok_or_else(|| limit("generation exhausted"))?;
            connection.execute("UPDATE container SET generation=?1,current_snapshot=?2,previous_snapshot=?3,guard=?4,guard_hash=?5 WHERE singleton=1", params![generation, &candidate.id.0[..], previous.as_ref().map(|id| &id.0[..]), candidate.guard, &hash(candidate.guard)[..]])?;
            schema::admit(connection, self.profile)?;
            cancelled(cancel)?;
            execution.commit_phase();
            if !observe(SaveStage::BeforeCommit) {
                return Err(Error::Cancelled);
            }
            Ok(Heads {
                generation,
                current: candidate.id,
                previous,
            })
        })();
        let heads = match preparation {
            Ok(heads) => heads,
            Err(error) => {
                execution.finish();
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        };
        match connection.execute_batch("COMMIT") {
            Ok(()) => {
                let observed = observe(SaveStage::Committed);
                Ok(Publication {
                    container: self.clone(),
                    heads,
                    outcome: if observed {
                        PublicationOutcome::PublishedConfirmed
                    } else {
                        PublicationOutcome::PublishedUnconfirmed
                    },
                    error: (!observed).then(|| invalid("post-commit observer failed")),
                })
            }
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                let observed = if connection.is_autocommit() {
                    reconcile(connection, self.profile)
                } else {
                    Err(invalid("commit/rollback outcome unknown"))
                };
                match observed {
                    Ok((actual, _)) if actual == base => Err(error.into()),
                    Ok((actual, _)) if actual == heads => Ok(Publication {
                        container: self.clone(),
                        heads,
                        outcome: PublicationOutcome::PublishedUnconfirmed,
                        error: Some(error.into()),
                    }),
                    _ => {
                        self.poisoned.store(true, Ordering::Release);
                        Ok(Publication {
                            container: self.clone(),
                            heads: base,
                            outcome: PublicationOutcome::OutcomeUnknown,
                            error: Some(error.into()),
                        })
                    }
                }
            }
        }
    }
    pub fn check(&self, cancel: &AtomicBool) -> Result<()> {
        let current = self.capture(Selection::Current, cancel)?;
        check_connection(&current.database.connection, self.profile, cancel)
    }
    pub fn compact_to_new(
        &self,
        path: &Path,
        cancel: &AtomicBool,
        mut observe: impl FnMut(SaveStage) -> bool,
    ) -> Result<Publication> {
        self.writable()?;
        let current = self.capture(Selection::Current, cancel)?;
        let source = &current.database.connection;
        check_connection(source, self.profile, cancel)?;
        let mut graphs = vec![(current.heads.current, graph(source, current.heads.current)?)];
        if let Some(previous) = current.heads.previous {
            graphs.push((previous, graph(source, previous)?));
        }
        let ids: BTreeSet<_> = graphs
            .iter()
            .flat_map(|(_, (root, members))| {
                std::iter::once(*root).chain(members.iter().map(|member| member.object))
            })
            .collect();
        let target = Location::attach(path, true)?;
        let lease = target.writer_lease()?;
        let stage = Stage::new(&target.parent)?;
        if !observe(SaveStage::Staged) {
            return Err(Error::Cancelled);
        }
        let destination = Database::open(stage.location.clone(), false, true, self.profile)?;
        let mut execution = Execution::new(&destination.connection, cancel);
        let connection = &destination.connection;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        for id in ids {
            cancelled(cancel)?;
            stream_object(source, id, MAX_OBJECT_BYTES, cancel, |_| Ok(()))?;
            let (length, count): (i64, i64) = source.query_row(
                "SELECT raw_length,chunk_count FROM objects WHERE object_id=?1",
                [&id.as_bytes()[..]],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            connection.execute(
                "INSERT INTO objects VALUES(?1,?2,?3)",
                params![&id.as_bytes()[..], length, count],
            )?;
            let mut statement = source.prepare("SELECT m.ordinal,c.chunk_id,c.raw_length,c.codec,c.data FROM object_chunks m JOIN chunks c ON c.chunk_id=m.chunk_id WHERE m.object_id=?1 ORDER BY m.ordinal LIMIT ?2")?;
            let mut rows =
                statement.query(params![&id.as_bytes()[..], (MAX_OBJECT_CHUNKS + 1) as i64])?;
            while let Some(row) = rows.next()? {
                cancelled(cancel)?;
                let chunk = digest_bytes(
                    row.get_ref(1)?
                        .as_blob()
                        .map_err(|_| invalid("chunk ID type"))?,
                )?;
                let data = row
                    .get_ref(4)?
                    .as_blob()
                    .map_err(|_| invalid("chunk data type"))?;
                if data.is_empty() || data.len() > CHUNK_BYTES {
                    return Err(limit("compact chunk bytes"));
                }
                if connection
                    .query_row(
                        "SELECT 1 FROM chunks WHERE chunk_id=?1",
                        [&chunk[..]],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_none()
                {
                    connection.execute(
                        "INSERT INTO chunks VALUES(?1,?2,?3,?4)",
                        params![
                            &chunk[..],
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            data
                        ],
                    )?;
                }
                connection.execute(
                    "INSERT INTO object_chunks VALUES(?1,?2,?3)",
                    params![&id.as_bytes()[..], row.get::<_, i64>(0)?, &chunk[..]],
                )?;
            }
        }
        for (id, (root, members)) in graphs {
            connection.execute(
                "INSERT INTO snapshots VALUES(?1,?2)",
                params![&id.0[..], &root.as_bytes()[..]],
            )?;
            for member in members {
                connection.execute(
                    "INSERT INTO snapshot_objects VALUES(?1,?2,?3)",
                    params![&id.0[..], member.role.get(), &member.object.as_bytes()[..]],
                )?;
            }
        }
        let heads = Heads {
            generation: 1,
            current: current.heads.current,
            previous: current.heads.previous,
        };
        connection.execute(
            "INSERT INTO container VALUES(1,?1,1,?2,?3,?4,?5)",
            params![
                self.profile.name(),
                &heads.current.0[..],
                heads.previous.as_ref().map(|id| &id.0[..]),
                &current.guard,
                &hash(&current.guard)[..]
            ],
        )?;
        cancelled(cancel)?;
        connection.execute_batch("COMMIT")?;
        execution.finish();
        drop(execution);
        drop(destination);
        let checker = Database::open(stage.location.clone(), true, false, self.profile)?;
        let verification = Execution::new(&checker.connection, cancel);
        verify_snapshot(&checker.connection, heads.current, cancel)?;
        if let Some(previous) = heads.previous {
            verify_snapshot(&checker.connection, previous, cancel)?;
        }
        drop(verification);
        drop(checker);
        Self::publish_stage(target, lease, stage, heads, self.profile, cancel, observe)
    }
    pub fn collect_unreachable(
        &self,
        cancel: &AtomicBool,
        mut decide: impl FnMut() -> bool,
    ) -> Result<()> {
        self.writable()?;
        let database = self.lock()?;
        let connection = &database.connection;
        let mut execution = Execution::new(connection, cancel);
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            check_connection(connection, self.profile, cancel)?;
            connection.execute_batch("DELETE FROM snapshot_objects WHERE snapshot_id NOT IN (SELECT current_snapshot FROM container UNION SELECT previous_snapshot FROM container WHERE previous_snapshot IS NOT NULL); DELETE FROM snapshots WHERE snapshot_id NOT IN (SELECT current_snapshot FROM container UNION SELECT previous_snapshot FROM container WHERE previous_snapshot IS NOT NULL); DELETE FROM object_chunks WHERE object_id NOT IN (SELECT root_object FROM snapshots UNION SELECT object_id FROM snapshot_objects); DELETE FROM objects WHERE object_id NOT IN (SELECT root_object FROM snapshots UNION SELECT object_id FROM snapshot_objects); DELETE FROM chunks WHERE chunk_id NOT IN (SELECT chunk_id FROM object_chunks);")?;
            cancelled(cancel)?;
            execution.commit_phase();
            if !decide() {
                return Err(Error::Cancelled);
            }
            connection.execute_batch("COMMIT").map_err(|error| {
                self.poisoned.store(true, Ordering::Release);
                Error::Io(std::io::Error::other(format!(
                    "collection COMMIT uncertain; explicitly reopen: {error}"
                )))
            })?;
            Ok(())
        })();
        if result.is_err() {
            execution.finish();
            let _ = connection.execute_batch("ROLLBACK");
        }
        result
    }
    pub fn stats(&self, cancel: &AtomicBool) -> Result<StorageStats> {
        let current = self.capture(Selection::Current, cancel)?;
        let connection = &current.database.connection;
        let (current_root, _) = graph(connection, current.heads.current)?;
        let mut current_ids: BTreeSet<_> =
            current.members.iter().map(|member| member.object).collect();
        current_ids.insert(current_root);
        let mut previous_ids = BTreeSet::new();
        if let Some(previous) = current.heads.previous {
            let (root, members) = graph(connection, previous)?;
            previous_ids.insert(root);
            previous_ids.extend(members.into_iter().map(|member| member.object));
        }
        let current_raw_bytes = sum_lengths(connection, &current_ids)?;
        let previous_only_raw_bytes = sum_lengths(
            connection,
            &previous_ids.difference(&current_ids).copied().collect(),
        )?;
        let scalar = |sql| -> Result<u64> {
            u64::try_from(connection.query_row(sql, [], |row| row.get::<_, i64>(0))?)
                .map_err(|_| invalid("negative storage accounting"))
        };
        let reachable_payload_bytes = scalar(
            "SELECT coalesce(sum(length(data)),0) FROM chunks WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id IN (SELECT root_object FROM snapshots WHERE snapshot_id IN (SELECT current_snapshot FROM container UNION SELECT previous_snapshot FROM container) UNION SELECT object_id FROM snapshot_objects WHERE snapshot_id IN (SELECT current_snapshot FROM container UNION SELECT previous_snapshot FROM container)))",
        )?;
        let total = scalar("SELECT coalesce(sum(length(data)),0) FROM chunks")?;
        let previous_only_payload_bytes = scalar(
            "SELECT coalesce(sum(length(data)),0) FROM chunks WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id IN (SELECT root_object FROM snapshots WHERE snapshot_id=(SELECT previous_snapshot FROM container) UNION SELECT object_id FROM snapshot_objects WHERE snapshot_id=(SELECT previous_snapshot FROM container))) AND chunk_id NOT IN (SELECT chunk_id FROM object_chunks WHERE object_id IN (SELECT root_object FROM snapshots WHERE snapshot_id=(SELECT current_snapshot FROM container) UNION SELECT object_id FROM snapshot_objects WHERE snapshot_id=(SELECT current_snapshot FROM container)))",
        )?;
        let (scratch_bytes, scratch_entries) =
            crate::storage_os::scratch_usage(&self.location.parent)?;
        self.location.check_journal(true)?;
        Ok(StorageStats {
            current_raw_bytes,
            previous_only_raw_bytes,
            reachable_payload_bytes,
            unreachable_payload_bytes: total
                .checked_sub(reachable_payload_bytes)
                .ok_or_else(|| invalid("payload accounting"))?,
            database_bytes: fs::metadata(self.path())?.len(),
            free_page_bytes: scalar("PRAGMA freelist_count")? * 4096,
            journal_bytes: fs::metadata(&self.location.journal)
                .map_or(0, |metadata| metadata.len()),
            snapshots: scalar("SELECT count(*) FROM snapshots")?,
            objects: scalar("SELECT count(*) FROM objects")?,
            chunks: scalar("SELECT count(*) FROM chunks")?,
            previous_only_payload_bytes,
            scratch_bytes,
            scratch_entries,
            membership_rows: scalar("SELECT count(*) FROM snapshot_objects")?,
            mapping_rows: scalar("SELECT count(*) FROM object_chunks")?,
            retained_roots: 1 + u64::from(current.heads.previous.is_some()),
            limits: STORAGE_LIMITS,
        })
    }
}

fn check_connection(
    connection: &Connection,
    profile_kind: DocumentProfile,
    cancel: &AtomicBool,
) -> Result<()> {
    schema::admit(connection, profile_kind)?;
    let (heads, _) = read_heads(connection, profile_kind)?;
    verify_snapshot(connection, heads.current, cancel)?;
    if let Some(previous) = heads.previous {
        verify_snapshot(connection, previous, cancel)?;
    }
    let integrity: String =
        connection.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(invalid("SQLite integrity check failed"));
    }
    if connection
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err(invalid("SQLite foreign key check failed"));
    }
    let connection = &connection;
    let mut snapshots = connection.prepare("SELECT snapshot_id FROM snapshots LIMIT ?1")?;
    let mut rows = snapshots.query([(MAX_SNAPSHOTS + 1) as i64])?;
    while let Some(row) = rows.next()? {
        cancelled(cancel)?;
        graph(
            connection,
            SnapshotId(digest_bytes(
                row.get_ref(0)?
                    .as_blob()
                    .map_err(|_| invalid("snapshot ID type"))?,
            )?),
        )?;
    }
    let mut objects = connection.prepare("SELECT object_id FROM objects LIMIT ?1")?;
    let mut rows = objects.query([(MAX_OBJECTS + 1) as i64])?;
    while let Some(row) = rows.next()? {
        let id = ObjectId::from_bytes(digest_bytes(
            row.get_ref(0)?
                .as_blob()
                .map_err(|_| invalid("object ID type"))?,
        )?);
        stream_object(connection, id, MAX_OBJECT_BYTES, cancel, |_| Ok(()))?;
    }
    let mut chunks =
        connection.prepare("SELECT chunk_id,raw_length,codec,data FROM chunks LIMIT ?1")?;
    let mut rows = chunks.query([(MAX_CHUNKS + 1) as i64])?;
    while let Some(row) = rows.next()? {
        cancelled(cancel)?;
        let digest = digest_bytes(
            row.get_ref(0)?
                .as_blob()
                .map_err(|_| invalid("chunk ID type"))?,
        )?;
        let length = usize::try_from(row.get::<_, i64>(1)?).map_err(|_| invalid("chunk length"))?;
        chunks::decode(
            row.get(2)?,
            length,
            row.get_ref(3)?
                .as_blob()
                .map_err(|_| invalid("chunk data type"))?,
            &digest,
        )?;
    }
    Ok(())
}

fn reconcile(connection: &Connection, profile_kind: DocumentProfile) -> Result<(Heads, Vec<u8>)> {
    let cancel = AtomicBool::new(false);
    let mut execution = Execution::new(connection, &cancel);
    connection.execute_batch("BEGIN")?;
    let result = (|| {
        let integrity: String =
            connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(invalid("uncertain commit failed structural inspection"));
        }
        let (heads, guard) = read_heads(connection, profile_kind)?;
        verify_snapshot(connection, heads.current, &cancel)?;
        if let Some(previous) = heads.previous {
            verify_snapshot(connection, previous, &cancel)?;
        }
        Ok((heads, guard))
    })();
    execution.finish();
    connection.execute_batch("ROLLBACK")?;
    result
}

fn read_heads(connection: &Connection, profile_kind: DocumentProfile) -> Result<(Heads, Vec<u8>)> {
    let mut statement = connection.prepare("SELECT profile,generation,current_snapshot,previous_snapshot,guard,guard_hash FROM container WHERE singleton=1")?;
    let mut rows = statement.query([])?;
    let row = rows
        .next()?
        .ok_or_else(|| invalid("missing container singleton"))?;
    let profile = row
        .get_ref(0)?
        .as_str()
        .map_err(|_| invalid("profile type"))?;
    let generation: i64 = row.get(1)?;
    let blob = |column| -> Result<&[u8]> {
        row.get_ref(column)?
            .as_blob()
            .map_err(|_| invalid("singleton blob type"))
    };
    let current = digest_bytes(blob(2)?)?;
    let previous = if matches!(row.get_ref(3)?, rusqlite::types::ValueRef::Null) {
        None
    } else {
        Some(digest_bytes(blob(3)?)?)
    };
    let guard = blob(4)?;
    let digest = digest_bytes(blob(5)?)?;
    if profile != profile_kind.name()
        || generation < 1
        || guard.is_empty()
        || guard.len() > MAX_GUARD_BYTES
        || hash(guard) != digest
    {
        return Err(invalid("container singleton/guard"));
    }
    let current = SnapshotId(current);
    let previous = previous.map(SnapshotId);
    if previous == Some(current) {
        return Err(invalid("current equals previous"));
    }
    Ok((
        Heads {
            generation,
            current,
            previous,
        },
        guard.to_vec(),
    ))
}

fn graph(connection: &Connection, id: SnapshotId) -> Result<(ObjectId, Vec<Member>)> {
    let mut root_statement =
        connection.prepare("SELECT root_object FROM snapshots WHERE snapshot_id=?1")?;
    let mut root_rows = root_statement.query([&id.0[..]])?;
    let row = root_rows
        .next()?
        .ok_or_else(|| invalid("missing snapshot"))?;
    let root = ObjectId::from_bytes(digest_bytes(
        row.get_ref(0)?
            .as_blob()
            .map_err(|_| invalid("snapshot root ID type"))?,
    )?);
    drop(root_rows);
    drop(root_statement);
    let mut statement = connection.prepare("SELECT role,object_id FROM snapshot_objects WHERE snapshot_id=?1 ORDER BY role,object_id LIMIT ?2")?;
    let mut rows = statement.query(params![&id.0[..], (MAX_MEMBERS + 1) as i64])?;
    let mut members = Vec::new();
    let mut ids = BTreeSet::from([root]);
    while let Some(row) = rows.next()? {
        let role: i64 = row.get(0)?;
        let object = ObjectId::from_bytes(digest_bytes(
            row.get_ref(1)?
                .as_blob()
                .map_err(|_| invalid("member ID type"))?,
        )?);
        members.push(Member {
            role: ObjectRole::new(u16::try_from(role).map_err(|_| invalid("member role"))?)?,
            object,
        });
        if members.len() > MAX_MEMBERS {
            return Err(limit("snapshot members"));
        }
        ids.insert(object);
    }
    if snapshot_id(root, &members) != id {
        return Err(invalid("snapshot digest/membership"));
    }
    if sum_lengths(connection, &ids)? > MAX_SNAPSHOT_BYTES {
        return Err(limit("snapshot raw bytes"));
    }
    Ok((root, members))
}

fn sum_lengths(connection: &Connection, ids: &BTreeSet<ObjectId>) -> Result<u64> {
    ids.iter().try_fold(0u64, |total, id| {
        total
            .checked_add(object_length(connection, *id)? as u64)
            .ok_or_else(|| limit("object byte arithmetic"))
    })
}

fn object_length(connection: &Connection, id: ObjectId) -> Result<usize> {
    let length: i64 = connection.query_row(
        "SELECT raw_length FROM objects WHERE object_id=?1",
        [&id.as_bytes()[..]],
        |row| row.get(0),
    )?;
    let length = usize::try_from(length).map_err(|_| invalid("negative object size"))?;
    model::validate_object_length(length)?;
    Ok(length)
}

fn read_object(
    connection: &Connection,
    id: ObjectId,
    budget: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    let length = object_length(connection, id)?;
    if length > budget {
        return Err(limit("incoming object allocation budget"));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| limit("cannot allocate incoming object"))?;
    if output.capacity() > budget {
        return Err(limit("incoming object capacity exceeds allocation budget"));
    }
    stream_object(connection, id, budget, cancel, |raw| {
        output.extend_from_slice(raw);
        Ok(())
    })?;
    Ok(output)
}

fn stream_object(
    connection: &Connection,
    id: ObjectId,
    budget: usize,
    cancel: &AtomicBool,
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let length = object_length(connection, id)?;
    if length > budget {
        return Err(limit("incoming object byte budget"));
    }
    let mut statement = connection.prepare("SELECT m.ordinal,c.chunk_id,c.raw_length,c.codec,c.data FROM object_chunks m LEFT JOIN chunks c ON c.chunk_id=m.chunk_id WHERE m.object_id=?1 ORDER BY m.ordinal LIMIT ?2")?;
    let mut rows = statement.query(params![&id.as_bytes()[..], (MAX_OBJECT_CHUNKS + 1) as i64])?;
    let mut total = 0usize;
    let mut ordinal = 0i64;
    let mut digest = Sha256::new();
    while let Some(row) = rows.next()? {
        cancelled(cancel)?;
        let actual: i64 = row.get(0)?;
        let raw_length =
            usize::try_from(row.get::<_, i64>(2)?).map_err(|_| invalid("negative chunk length"))?;
        if actual != ordinal
            || ordinal >= MAX_OBJECT_CHUNKS as i64
            || total >= length
            || raw_length != (length - total).min(CHUNK_BYTES)
            || raw_length == 0
        {
            return Err(invalid("chunk coverage/length"));
        }
        let expected = digest_bytes(
            row.get_ref(1)?
                .as_blob()
                .map_err(|_| invalid("chunk ID type"))?,
        )?;
        let raw = chunks::decode(
            row.get(3)?,
            raw_length,
            row.get_ref(4)?
                .as_blob()
                .map_err(|_| invalid("chunk data type"))?,
            &expected,
        )?;
        digest.update(&raw);
        consume(&raw)?;
        total += raw.len();
        ordinal += 1;
    }
    let declared: i64 = connection.query_row(
        "SELECT chunk_count FROM objects WHERE object_id=?1",
        [&id.as_bytes()[..]],
        |row| row.get(0),
    )?;
    if total != length
        || ordinal != declared
        || <[u8; 32]>::from(digest.finalize()) != *id.as_bytes()
    {
        return Err(invalid("object digest/coverage"));
    }
    Ok(())
}

fn verify_snapshot(connection: &Connection, id: SnapshotId, cancel: &AtomicBool) -> Result<()> {
    let (root, members) = graph(connection, id)?;
    stream_object(connection, root, MAX_ROOT_BYTES, cancel, |_| Ok(()))?;
    for object in members
        .iter()
        .map(|member| member.object)
        .collect::<BTreeSet<_>>()
    {
        stream_object(connection, object, MAX_OBJECT_BYTES, cancel, |_| Ok(()))?;
    }
    Ok(())
}

struct ChunkPosition {
    offset: u64,
    length: usize,
    raw_length: usize,
    codec: u8,
}
struct Staged {
    spool: Spool,
    chunks: BTreeMap<[u8; 32], ChunkPosition>,
    objects: BTreeMap<ObjectId, Vec<[u8; 32]>>,
}

impl Staged {
    fn new(candidate: &Candidate<'_>, parent: &Arc<Parent>, cancel: &AtomicBool) -> Result<Self> {
        let mut staged = Self {
            spool: Spool::new(parent)?,
            chunks: BTreeMap::new(),
            objects: BTreeMap::new(),
        };
        let mut offset = 0u64;
        for (id, bytes) in &candidate.objects {
            let mut mapping = Vec::new();
            bytes.for_each_chunk(cancel, |raw| {
                let id = hash(raw);
                mapping.push(id);
                if staged.chunks.contains_key(&id) {
                    return Ok(());
                }
                let encoded = chunks::encode(raw)?;
                if offset + encoded.data.len() as u64 > MAX_SCRATCH_BYTES {
                    return Err(limit("staged encoded bytes"));
                }
                staged.spool.append(&encoded.data)?;
                staged.chunks.insert(
                    encoded.hash,
                    ChunkPosition {
                        offset,
                        length: encoded.data.len(),
                        raw_length: encoded.raw_length,
                        codec: encoded.codec,
                    },
                );
                offset += encoded.data.len() as u64;
                Ok(())
            })?;
            staged.objects.insert(*id, mapping);
        }
        Ok(staged)
    }
}

fn insert(
    connection: &Connection,
    candidate: &Candidate<'_>,
    staged: &mut Staged,
    cancel: &AtomicBool,
) -> Result<()> {
    for (id, position) in &staged.chunks {
        cancelled(cancel)?;
        let mut query =
            connection.prepare("SELECT raw_length,codec,data FROM chunks WHERE chunk_id=?1")?;
        let mut rows = query.query([&id[..]])?;
        if let Some(row) = rows.next()? {
            let length = usize::try_from(row.get::<_, i64>(0)?)
                .map_err(|_| invalid("negative chunk length"))?;
            chunks::decode(
                row.get(1)?,
                length,
                row.get_ref(2)?
                    .as_blob()
                    .map_err(|_| invalid("chunk data type"))?,
                id,
            )?;
            continue;
        }
        let mut data = vec![0u8; position.length];
        staged.spool.file.seek(SeekFrom::Start(position.offset))?;
        staged.spool.file.read_exact(&mut data)?;
        connection.execute(
            "INSERT INTO chunks VALUES(?1,?2,?3,?4)",
            params![&id[..], position.raw_length as i64, position.codec, data],
        )?;
    }
    for (id, mapping) in &staged.objects {
        let bytes = candidate.objects[id];
        let length = bytes.len()?;
        if connection
            .query_row(
                "SELECT 1 FROM objects WHERE object_id=?1",
                [&id.as_bytes()[..]],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            let mut incoming = bytes.reader();
            let mut expected = vec![0u8; CHUNK_BYTES];
            stream_object(connection, *id, length, cancel, |raw| {
                let expected = &mut expected[..raw.len()];
                incoming
                    .read_exact(expected)
                    .map_err(|_| invalid("existing object disagrees with incoming bytes"))?;
                if expected != raw {
                    return Err(invalid("existing object disagrees with incoming bytes"));
                }
                Ok(())
            })?;
        } else {
            connection.execute(
                "INSERT INTO objects VALUES(?1,?2,?3)",
                params![&id.as_bytes()[..], length as i64, mapping.len() as i64],
            )?;
            for (ordinal, chunk) in mapping.iter().enumerate() {
                connection.execute(
                    "INSERT INTO object_chunks VALUES(?1,?2,?3)",
                    params![&id.as_bytes()[..], ordinal as i64, &chunk[..]],
                )?;
            }
        }
    }
    if connection
        .query_row(
            "SELECT 1 FROM snapshots WHERE snapshot_id=?1",
            [&candidate.id.0[..]],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        if graph(connection, candidate.id)? != (candidate.root, candidate.members.clone()) {
            return Err(invalid("existing snapshot disagrees"));
        }
    } else {
        connection.execute(
            "INSERT INTO snapshots VALUES(?1,?2)",
            params![&candidate.id.0[..], &candidate.root.as_bytes()[..]],
        )?;
        for member in &candidate.members {
            connection.execute(
                "INSERT INTO snapshot_objects VALUES(?1,?2,?3)",
                params![
                    &candidate.id.0[..],
                    member.role.get(),
                    &member.object.as_bytes()[..]
                ],
            )?;
        }
    }
    Ok(())
}
