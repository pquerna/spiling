// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Durable operation admission, execution ownership and immutable artifact publication.
use prost::Message;
mod native;
pub use native::NativeAdmission;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use spiling_contracts::{
    MAX_ARTIFACT_BYTES, METADATA_TYPE, RESULT_TYPE, google::longrunning::Operation,
    google::longrunning::operation::Result as Outcome, google::rpc::Status as RpcStatus, metadata,
    rpc::*, synthetic_triangle,
};
use spiling_contracts::{NATIVE_METADATA_TYPE, native_metadata};
use std::{
    collections::HashMap,
    fs::File,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, watch};
use tonic::{Code, Status};
use uuid::Uuid;

pub const MAX_OPERATIONS: usize = 128;
const MAX_PENDING: usize = 8;
const STORAGE_BUDGET: u64 = 32 * 1024 * 1024;

pub fn internal(error: impl std::fmt::Display) -> Status {
    eprintln!(
        "{}",
        serde_json::json!({"level":"error", "event":"job_store_failure", "message":error.to_string()})
    );
    Status::internal("operation store failure")
}
fn now() -> prost_types::Timestamp {
    std::time::SystemTime::now().into()
}
fn pack(meta: &DiagnosticMetadata) -> prost_types::Any {
    prost_types::Any {
        type_url: METADATA_TYPE.into(),
        value: meta.encode_to_vec(),
    }
}
fn terminal(
    op: &mut Operation,
    state: DiagnosticState,
    error: Option<(Code, &str)>,
) -> Result<(), Status> {
    if op
        .metadata
        .as_ref()
        .is_some_and(|m| m.type_url == NATIVE_METADATA_TYPE)
    {
        return native::terminal_native(op, native::state_from_diagnostic(state), error);
    }
    let mut meta = metadata(op).map_err(internal)?;
    meta.state = state as i32;
    meta.phase = state.as_str_name().to_lowercase();
    meta.state_version += 1;
    meta.update_time = Some(now());
    meta.end_time = meta.update_time;
    op.done = true;
    op.result = Some(match error {
        Some((code, message)) => Outcome::Error(RpcStatus {
            code: code as i32,
            message: message.into(),
            details: vec![],
        }),
        None => Outcome::Response(prost_types::Any {
            type_url: RESULT_TYPE.into(),
            value: DiagnosticResult {
                artifacts: meta.outputs.clone(),
            }
            .encode_to_vec(),
        }),
    });
    op.metadata = Some(pack(&meta));
    Ok(())
}

pub struct Inner {
    db: Connection,
    watchers: HashMap<String, watch::Sender<Operation>>,
    _lock: File,
}
impl Inner {
    pub(crate) fn get(&self, name: &str) -> Result<Operation, Status> {
        let bytes: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT operation FROM operations WHERE name=?",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(internal)?;
        Operation::decode(
            bytes
                .ok_or_else(|| Status::not_found("operation not found"))?
                .as_slice(),
        )
        .map_err(internal)
    }
    fn save(&mut self, op: &Operation) -> Result<(), Status> {
        if op.done
            && op
                .metadata
                .as_ref()
                .is_some_and(|m| m.type_url == NATIVE_METADATA_TYPE)
        {
            let size: u64 = native_metadata(op)
                .map_err(internal)?
                .outputs
                .iter()
                .map(|a| a.size_bytes)
                .sum();
            self.db
                .execute(
                    "UPDATE operations SET operation=?,reserved_bytes=? WHERE name=?",
                    params![
                        op.encode_to_vec(),
                        i64::try_from(size).map_err(internal)?,
                        op.name
                    ],
                )
                .map_err(internal)?;
        } else {
            self.db
                .execute(
                    "UPDATE operations SET operation=? WHERE name=?",
                    params![op.encode_to_vec(), op.name],
                )
                .map_err(internal)?;
        }
        self.notify(op);
        Ok(())
    }
    fn notify(&mut self, op: &Operation) {
        if let Some(tx) = self.watchers.get(&op.name) {
            tx.send_replace(op.clone());
        }
    }
    fn subscribe(&mut self, name: &str) -> Result<watch::Receiver<Operation>, Status> {
        let op = self.get(name)?;
        Ok(self
            .watchers
            .entry(name.into())
            .or_insert_with(|| watch::channel(op).0)
            .subscribe())
    }
}

#[derive(Clone)]
pub struct Jobs {
    inner: Arc<Mutex<Inner>>,
    worker: Arc<Semaphore>,
}
impl Jobs {
    pub fn open(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(path)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join("owner.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)?;
        let db = Connection::open(path.join("operations.sqlite"))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS operations(name TEXT PRIMARY KEY, parent TEXT NOT NULL,
                request_id TEXT NOT NULL, digest TEXT NOT NULL, reserved_bytes INTEGER NOT NULL,
                operation BLOB NOT NULL, UNIQUE(parent,request_id));
            CREATE TABLE IF NOT EXISTS artifacts(name TEXT PRIMARY KEY, bytes BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS artifact_types(name TEXT PRIMARY KEY, media_type TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS native_operations(name TEXT PRIMARY KEY);",
        )?;
        let mut inner = Inner {
            db,
            watchers: HashMap::new(),
            _lock: lock,
        };
        let rows = {
            let mut stmt = inner.db.prepare("SELECT operation FROM operations")?;
            stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for bytes in rows {
            let mut op = Operation::decode(bytes.as_slice())?;
            if !op.done {
                terminal(
                    &mut op,
                    DiagnosticState::Interrupted,
                    Some((Code::Aborted, "engine stopped before operation completed")),
                )?;
                inner.save(&op)?;
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            worker: Arc::new(Semaphore::new(1)),
        })
    }
    async fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Inner) -> Result<T, Status> + Send + 'static,
    ) -> Result<T, Status> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = inner.lock().map_err(internal)?;
            f(&mut guard)
        })
        .await
        .map_err(internal)?
    }
    pub(crate) fn with_sync<T>(
        &self,
        f: impl FnOnce(&mut Inner) -> Result<T, Status>,
    ) -> Result<T, Status> {
        let mut guard = self.inner.lock().map_err(internal)?;
        f(&mut guard)
    }
    pub async fn interrupt_all(&self) -> Result<(), Status> {
        self.with(|s| {
            let rows = {
                let mut stmt =
                    s.db.prepare("SELECT name FROM operations")
                        .map_err(internal)?;
                stmt.query_map([], |r| r.get::<_, String>(0))
                    .map_err(internal)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(internal)?
            };
            for name in rows {
                let mut op = s.get(&name)?;
                if !op.done {
                    terminal(
                        &mut op,
                        DiagnosticState::Interrupted,
                        Some((Code::Aborted, "engine shutting down")),
                    )?;
                    s.save(&op)?;
                }
            }
            Ok(())
        })
        .await
    }
    pub async fn get(&self, name: String) -> Result<Operation, Status> {
        self.with(move |s| s.get(&name)).await
    }
    pub async fn subscribe(&self, name: String) -> Result<watch::Receiver<Operation>, Status> {
        self.with(move |s| s.subscribe(&name)).await
    }
    pub async fn start(&self, request: RunDiagnosticRequest) -> Result<Operation, Status> {
        validate(&request)?;
        // Admission belongs to the engine, even if its caller loses the acknowledgement.
        let jobs = self.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let outcome = jobs.accept(request).await;
            let _ = tx.send(outcome);
        });
        rx.await.map_err(internal)?
    }
    async fn accept(&self, mut request: RunDiagnosticRequest) -> Result<Operation, Status> {
        validate(&request)?;
        let id = if request.request_id.is_empty() {
            Uuid::new_v4()
        } else {
            Uuid::parse_str(&request.request_id)
                .map_err(|_| Status::invalid_argument("request_id must be a UUID"))?
        };
        if id.is_nil() {
            return Err(Status::invalid_argument(
                "request_id must not be the zero UUID",
            ));
        }
        let request_id = id.to_string();
        request.request_id.clear();
        let digest = format!("{:x}", Sha256::digest(request.encode_to_vec()));
        let parameters = request.clone();
        let (op, created) = self
            .with(move |s| {
                let old: Option<(String, Vec<u8>)> =
                    s.db.query_row(
                        "SELECT digest,operation FROM operations WHERE parent=? AND request_id=?",
                        params![request.parent, request_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(internal)?;
                if let Some((previous, bytes)) = old {
                    if previous != digest {
                        return Err(Status::already_exists(
                            "request_id already used with different parameters",
                        ));
                    }
                    return Ok((
                        Operation::decode(bytes.as_slice()).map_err(internal)?,
                        false,
                    ));
                }
                let mut stmt =
                    s.db.prepare("SELECT operation FROM operations")
                        .map_err(internal)?;
                let rows = stmt
                    .query_map([], |r| r.get::<_, Vec<u8>>(0))
                    .map_err(internal)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(internal)?;
                let active = rows
                    .iter()
                    .filter(|b| Operation::decode(b.as_slice()).is_ok_and(|o| !o.done))
                    .count();
                if rows.len() >= MAX_OPERATIONS || active >= MAX_PENDING {
                    return Err(Status::resource_exhausted(
                        "operation admission capacity reached",
                    ));
                }
                drop(stmt);
                let reserved: i64 =
                    s.db.query_row(
                        "SELECT COALESCE(SUM(reserved_bytes),0) FROM operations WHERE name NOT IN (SELECT name FROM native_operations)",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(internal)?;
                let size = u64::from(request.chunk_count) * u64::from(request.chunk_bytes);
                if reserved as u64 + size > STORAGE_BUDGET {
                    return Err(Status::resource_exhausted(
                        "artifact storage reservation capacity reached",
                    ));
                }
                let meta = DiagnosticMetadata {
                    state: DiagnosticState::Queued as i32,
                    state_version: 1,
                    phase: "queued".into(),
                    completed_units: 0,
                    total_units: request.chunk_count,
                    input_revision: request.input_revision.clone(),
                    input_digest: digest.clone(),
                    outputs: vec![],
                    create_time: Some(now()),
                    update_time: Some(now()),
                    end_time: None,
                };
                let op = Operation {
                    name: format!("{}/operations/{}", request.parent, Uuid::new_v4()),
                    metadata: Some(pack(&meta)),
                    done: false,
                    result: None,
                };
                s.db.execute(
                    "INSERT INTO operations VALUES(?,?,?,?,?,?)",
                    params![
                        op.name,
                        request.parent,
                        request_id,
                        digest,
                        size as i64,
                        op.encode_to_vec()
                    ],
                )
                .map_err(internal)?;
                Ok((op, true))
            })
            .await?;
        if created {
            let jobs = self.clone();
            let name = op.name.clone();
            tokio::spawn(async move {
                if let Err(error) = jobs.run(name.clone(), parameters).await {
                    let _ = jobs
                        .with(move |s| {
                            let mut op = s.get(&name)?;
                            if !op.done {
                                terminal(
                                    &mut op,
                                    DiagnosticState::Failed,
                                    Some((error.code(), error.message())),
                                )?;
                                s.save(&op)?;
                            }
                            Ok(())
                        })
                        .await;
                }
            });
        }
        Ok(op)
    }
    pub async fn cancel(&self, name: String) -> Result<(), Status> {
        self.with(move |s| {
            let mut op = s.get(&name)?;
            if op
                .metadata
                .as_ref()
                .is_some_and(|m| m.type_url == NATIVE_METADATA_TYPE)
            {
                return native::cancel_native(s, op);
            }
            let mut meta = metadata(&op).map_err(internal)?;
            if !op.done && meta.state != DiagnosticState::Cancelling as i32 {
                meta.state = DiagnosticState::Cancelling as i32;
                meta.phase = "cancelling".into();
                meta.state_version += 1;
                meta.update_time = Some(now());
                op.metadata = Some(pack(&meta));
                s.save(&op)?;
            }
            Ok(())
        })
        .await
    }
    async fn run(&self, name: String, request: RunDiagnosticRequest) -> Result<(), Status> {
        let mut updates = self.subscribe(name.clone()).await?;
        let permit = loop {
            if is_cancelling(&updates.borrow()) {
                return self.finish_cancel(name).await;
            }
            tokio::select! {
                permit = self.worker.clone().acquire_owned() => break permit.map_err(internal)?,
                changed = updates.changed() => { changed.map_err(internal)?; }
            }
        };
        let key = name.clone();
        let running = self
            .with(move |s| {
                let mut op = s.get(&key)?;
                if is_cancelling(&op) {
                    return Ok(false);
                }
                let mut meta = metadata(&op).map_err(internal)?;
                meta.state = DiagnosticState::Running as i32;
                meta.phase = "generating".into();
                meta.state_version += 1;
                meta.update_time = Some(now());
                op.metadata = Some(pack(&meta));
                s.save(&op)?;
                Ok(true)
            })
            .await?;
        if !running {
            return self.finish_cancel(name).await;
        }
        for index in 0..request.chunk_count {
            let deadline =
                tokio::time::Instant::now() + Duration::from_millis(u64::from(request.delay_ms));
            loop {
                if is_cancelling(&updates.borrow()) {
                    return self.finish_cancel(name).await;
                }
                tokio::select! { _ = tokio::time::sleep_until(deadline) => break,
                changed = updates.changed() => { changed.map_err(internal)?; } }
            }
            let mut bytes = vec![0; request.chunk_bytes as usize];
            bytes[..64].copy_from_slice(&synthetic_triangle());
            // Synthetic padding varies by chunk to exercise distinct immutable downloads.
            if bytes.len() > 64 {
                bytes[64..].fill(index as u8);
            }
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let artifact = Artifact {
                name: format!("artifacts/{hash}"),
                size_bytes: bytes.len() as u64,
                sha256: hash,
                media_type: if bytes.len() == 64 {
                    "application/x-spiling-triangle"
                } else {
                    "application/x-spiling-diagnostic-padded-triangle"
                }
                .into(),
            };
            let key = name.clone();
            let continued = self
                .with(move |s| {
                    let mut op = s.get(&key)?;
                    if is_cancelling(&op) {
                        return Ok(false);
                    }
                    let mut meta = metadata(&op).map_err(internal)?;
                    meta.completed_units += 1;
                    meta.state_version += 1;
                    meta.update_time = Some(now());
                    meta.outputs.push(artifact.clone());
                    op.metadata = Some(pack(&meta));
                    if index + 1 == request.chunk_count {
                        terminal(&mut op, DiagnosticState::Succeeded, None)?;
                    }
                    let tx = s.db.transaction().map_err(internal)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO artifacts VALUES(?,?)",
                        params![artifact.name, bytes],
                    )
                    .map_err(internal)?;
                    tx.execute(
                        "UPDATE operations SET operation=? WHERE name=?",
                        params![op.encode_to_vec(), op.name],
                    )
                    .map_err(internal)?;
                    tx.commit().map_err(internal)?;
                    s.notify(&op);
                    Ok(true)
                })
                .await?;
            if !continued {
                return self.finish_cancel(name).await;
            }
        }
        drop(permit);
        Ok(())
    }
    async fn finish_cancel(&self, name: String) -> Result<(), Status> {
        self.with(move |s| {
            let mut op = s.get(&name)?;
            if !op.done {
                terminal(
                    &mut op,
                    DiagnosticState::Cancelled,
                    Some((Code::Cancelled, "operation cancelled")),
                )?;
                s.save(&op)?;
            }
            Ok(())
        })
        .await
    }
    pub async fn list(
        &self,
        parent: String,
        page_size: i32,
        page_token: String,
    ) -> Result<spiling_contracts::google::longrunning::ListOperationsResponse, Status> {
        if parent.starts_with("sessions/") {
            native::validate_native_parent(&parent)?;
        } else {
            validate_parent(&parent)?;
        }
        if page_size < 0 {
            return Err(Status::invalid_argument("negative page_size"));
        }
        let limit = if page_size == 0 {
            20
        } else {
            page_size.min(50)
        };
        let binding = format!("{:x}", Sha256::digest(parent.as_bytes()));
        let after = if page_token.is_empty() {
            String::new()
        } else {
            let (scope, id) = page_token
                .split_once(':')
                .ok_or_else(|| Status::invalid_argument("invalid page token"))?;
            if scope != binding || Uuid::parse_str(id).is_err() {
                return Err(Status::invalid_argument("invalid page token scope"));
            }
            format!("{parent}/operations/{id}")
        };
        self.with(move |s| {
            let mut stmt = s.db.prepare("SELECT operation FROM operations WHERE parent=? AND name>? ORDER BY name LIMIT ?").map_err(internal)?;
            let bytes = stmt.query_map(params![parent, after, limit+1], |r| r.get::<_,Vec<u8>>(0)).map_err(internal)?.collect::<Result<Vec<_>,_>>().map_err(internal)?;
            let mut operations = bytes.into_iter().map(|b| Operation::decode(b.as_slice()).map_err(internal)).collect::<Result<Vec<_>,_>>()?;
            let token = if operations.len() > limit as usize {
                operations.truncate(limit as usize);
                let id = operations.last().ok_or_else(|| Status::internal("empty page"))?.name.rsplit('/').next().unwrap_or_default();
                format!("{binding}:{id}")
            } else { String::new() };
            Ok(spiling_contracts::google::longrunning::ListOperationsResponse { operations, next_page_token: token, unreachable: vec![] })
        }).await
    }
    pub async fn artifact(&self, name: String) -> Result<Artifact, Status> {
        self.with(move |s| {
            let size: Option<i64> =
                s.db.query_row(
                    "SELECT length(bytes) FROM artifacts WHERE name=?",
                    [&name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(internal)?;
            let size = size.ok_or_else(|| Status::not_found("artifact not found"))?;
            Ok(Artifact {
                sha256: name
                    .strip_prefix("artifacts/")
                    .ok_or_else(|| Status::invalid_argument("invalid artifact name"))?
                    .into(),
                size_bytes: size as u64,
                media_type: s
                    .db
                    .query_row(
                        "SELECT media_type FROM artifact_types WHERE name=?",
                        [&name],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(internal)?
                    .unwrap_or_else(|| {
                        if size == 64 {
                            "application/x-spiling-triangle".into()
                        } else {
                            "application/x-spiling-diagnostic-padded-triangle".into()
                        }
                    }),
                name,
            })
        })
        .await
    }
    pub async fn fragment(
        &self,
        name: String,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, Status> {
        self.with(move |s| {
            s.db.query_row(
                "SELECT substr(bytes,?,?) FROM artifacts WHERE name=?",
                params![(offset + 1) as i64, length as i64, name],
                |r| r.get(0),
            )
            .map_err(internal)
        })
        .await
    }
}
fn is_cancelling(op: &Operation) -> bool {
    op.done || metadata(op).is_ok_and(|m| m.state == DiagnosticState::Cancelling as i32)
}
fn validate_parent(parent: &str) -> Result<(), Status> {
    let id = parent
        .strip_prefix("diagnostics/")
        .ok_or_else(|| Status::invalid_argument("parent must be diagnostics/{id}"))?;
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Status::invalid_argument("invalid diagnostic parent"));
    }
    Ok(())
}
fn validate(request: &RunDiagnosticRequest) -> Result<(), Status> {
    validate_parent(&request.parent)?;
    if !(1..=64).contains(&request.chunk_count)
        || request.delay_ms > 1000
        || !(64..=MAX_ARTIFACT_BYTES).contains(&request.chunk_bytes)
        || request.input_revision.len() > 128
    {
        return Err(Status::invalid_argument(
            "diagnostic limits: 1..64 chunks, 0..1000ms delay, 64..262144 bytes, revision <=128 bytes",
        ));
    }
    Ok(())
}
