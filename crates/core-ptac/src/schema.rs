// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    MAX_CHUNKS, MAX_MAPPING_ROWS, MAX_MEMBERSHIP_ROWS, MAX_OBJECTS, MAX_SNAPSHOTS, Result, invalid,
    limit,
};
use rusqlite::{Connection, config::DbConfig, limits::Limit};

pub(crate) const TABLES: &[&str] = &[
    "container",
    "chunks",
    "objects",
    "object_chunks",
    "snapshots",
    "snapshot_objects",
];
pub(crate) const INDEXES: &[&str] = &[
    "object_chunks_by_chunk",
    "snapshots_by_root",
    "snapshot_objects_by_object",
    "sqlite_autoindex_chunks_1",
    "sqlite_autoindex_objects_1",
    "sqlite_autoindex_object_chunks_1",
    "sqlite_autoindex_snapshots_1",
    "sqlite_autoindex_snapshot_objects_1",
];

pub(crate) const DDL: &str = "
CREATE TABLE chunks (
 chunk_id BLOB NOT NULL PRIMARY KEY CHECK(length(chunk_id)=32),
 raw_length INTEGER NOT NULL CHECK(raw_length BETWEEN 1 AND 262144),
 codec INTEGER NOT NULL CHECK(codec IN (0,1)),
 data BLOB NOT NULL CHECK(length(data) BETWEEN 1 AND 262144),
 CHECK(codec!=0 OR length(data)=raw_length)
) STRICT;
CREATE TABLE objects (
 object_id BLOB NOT NULL PRIMARY KEY CHECK(length(object_id)=32),
 raw_length INTEGER NOT NULL CHECK(raw_length BETWEEN 1 AND 16777216),
 chunk_count INTEGER NOT NULL CHECK(chunk_count BETWEEN 1 AND 64),
 CHECK(chunk_count=(raw_length+262143)/262144)
) STRICT;
CREATE TABLE object_chunks (
 object_id BLOB NOT NULL REFERENCES objects(object_id),
 ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 63),
 chunk_id BLOB NOT NULL REFERENCES chunks(chunk_id),
 PRIMARY KEY(object_id,ordinal)
) STRICT;
CREATE INDEX object_chunks_by_chunk ON object_chunks(chunk_id);
CREATE TABLE snapshots (
 snapshot_id BLOB NOT NULL PRIMARY KEY CHECK(length(snapshot_id)=32),
 root_object BLOB NOT NULL REFERENCES objects(object_id) CHECK(length(root_object)=32)
) STRICT;
CREATE INDEX snapshots_by_root ON snapshots(root_object);
CREATE TABLE snapshot_objects (
 snapshot_id BLOB NOT NULL REFERENCES snapshots(snapshot_id),
 role INTEGER NOT NULL CHECK(role BETWEEN 1 AND 65535),
 object_id BLOB NOT NULL REFERENCES objects(object_id),
 PRIMARY KEY(snapshot_id,role,object_id)
) STRICT;
CREATE INDEX snapshot_objects_by_object ON snapshot_objects(object_id);
CREATE TABLE container (
 singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
 profile TEXT NOT NULL CHECK(profile='spiling.project'),
 generation INTEGER NOT NULL CHECK(generation BETWEEN 1 AND 9223372036854775807),
 current_snapshot BLOB NOT NULL REFERENCES snapshots(snapshot_id) CHECK(length(current_snapshot)=32),
 previous_snapshot BLOB REFERENCES snapshots(snapshot_id) CHECK(previous_snapshot IS NULL OR length(previous_snapshot)=32),
 guard BLOB NOT NULL CHECK(length(guard) BETWEEN 1 AND 4096),
 guard_hash BLOB NOT NULL CHECK(length(guard_hash)=32),
 CHECK(previous_snapshot IS NULL OR previous_snapshot!=current_snapshot)
) STRICT;
";

pub(crate) fn configure(connection: &Connection, read_only: bool) -> Result<()> {
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    for (kind, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, 1024 * 1024),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, 65536),
        (Limit::SQLITE_LIMIT_COLUMN, 32),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 64),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 128),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
    ] {
        connection.set_limit(kind, value)?;
    }
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA recursive_triggers=OFF; PRAGMA mmap_size=0; PRAGMA cache_size=-8192; PRAGMA temp_store=MEMORY; PRAGMA synchronous=EXTRA;")?;
    if read_only {
        connection.execute_batch("PRAGMA query_only=ON;")?;
    }
    Ok(())
}

pub(crate) fn create(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=DELETE; PRAGMA auto_vacuum=NONE; PRAGMA application_id=1347699011; PRAGMA user_version=1;")?;
    connection.execute_batch(DDL)?;
    Ok(())
}

type CatalogRow = (String, String, String, Option<String>);

fn catalog(connection: &Connection) -> Result<Vec<CatalogRow>> {
    let mut statement = connection
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name LIMIT 33")?;
    let mut rows = statement.query([])?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        for column in 0..4 {
            let value = row.get_ref(column)?;
            if let rusqlite::types::ValueRef::Text(text) = value
                && text.len() > 65536
            {
                return Err(limit("catalog text"));
            }
        }
        output.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
        if output.len() > 32 {
            return Err(limit("catalog entries"));
        }
    }
    Ok(output)
}

pub(crate) fn admit(connection: &Connection) -> Result<()> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(DDL)?;
    if catalog(connection)? != catalog(&expected)? {
        return Err(invalid("unexpected or weakened SQLite catalog"));
    }
    for table in TABLES {
        for pragma in ["table_xinfo", "foreign_key_list", "index_list"] {
            let sql = format!("PRAGMA {pragma}('{table}')");
            if pragma_rows(connection, &sql)? != pragma_rows(&expected, &sql)? {
                return Err(invalid("table/foreign-key/index metadata mismatch"));
            }
        }
    }
    for index in INDEXES {
        let sql = format!("PRAGMA index_xinfo('{index}')");
        if pragma_rows(connection, &sql)? != pragma_rows(&expected, &sql)? {
            return Err(invalid("index metadata mismatch"));
        }
    }
    let application: u32 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let page_size: u32 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let journal: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let auto_vacuum: u32 = connection.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
    if application != crate::APPLICATION_ID
        || version != crate::SCHEMA_VERSION
        || page_size != 4096
        || journal != "delete"
        || auto_vacuum != 0
    {
        return Err(invalid("SQLite container identity/settings mismatch"));
    }
    for (table, ceiling) in [
        ("container", 1),
        ("snapshots", MAX_SNAPSHOTS),
        ("snapshot_objects", MAX_MEMBERSHIP_ROWS),
        ("objects", MAX_OBJECTS),
        ("object_chunks", MAX_MAPPING_ROWS),
        ("chunks", MAX_CHUNKS),
    ] {
        let count: i64 = connection.query_row(
            &format!(
                "SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT {})",
                ceiling + 1
            ),
            [],
            |row| row.get(0),
        )?;
        if count > ceiling as i64 {
            return Err(limit(format!(
                "{table} retained rows; explicitly collect unreachable content or compact to a new .spil"
            )));
        }
    }
    Ok(())
}

fn pragma_rows(connection: &Connection, sql: &str) -> Result<Vec<Vec<rusqlite::types::Value>>> {
    let mut statement = connection.prepare(sql)?;
    let columns = statement.column_count();
    if columns > 32 {
        return Err(limit("introspection columns"));
    }
    let mut rows = statement.query([])?;
    let mut output = Vec::new();
    while let Some(row) = rows.next()? {
        if output.len() >= 64 {
            return Err(limit("introspection rows"));
        }
        let mut values = Vec::with_capacity(columns);
        for column in 0..columns {
            let value = row.get_ref(column)?;
            if let rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) =
                value
                && bytes.len() > 65536
            {
                return Err(limit("introspection value bytes"));
            }
            values.push(
                rusqlite::types::Value::try_from(value)
                    .map_err(|_| invalid("invalid introspection text"))?,
            );
        }
        output.push(values);
    }
    Ok(output)
}
