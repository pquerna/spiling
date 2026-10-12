<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Shared immutable export rules

- CLI and native shell use this single filesystem transaction implementation. No kernel, planner, Tauri, project storage or browser filesystem dependency belongs here.
- `OutputDirectory::create(&Path)` admits only a previously nonexistent directory; `write` accepts one normal filename and creates it exclusively. Unix cleanup uses parent/directory handles and exact device/inode identity; Windows holds canonical ancestry without delete sharing and deletes only owned handles. Never recurse, follow substituted symlinks, overwrite files, or remove foreign replacements.
- `write_manufacturing_bundle(&ManufacturingBundle, &VerificationReport)` writes plan.json, program.gcode, verification.json and provenance.json. The caller must provide a client-validated bundle and the exact fresh engine inspection report; stored verification flags are not freshness or machine readiness. `ExportError` distinguishes I/O and JSON encoding.
- `OutputFile` applies the same exclusive creation and owned-handle rollback to packed replay output. Bytes must already have passed descriptor/record/hash/packed validation. Dropping any uncommitted transaction rolls back only its own files.
- Commit only after complete validated writes and the caller's lifecycle/freshness completion boundary. Shell owns native dialog authorization and active-record rechecks; CLI owns observed child reap and completion manifests/stdout. This module never retries or grants arbitrary renderer paths authority.
- Migrated export tests cover exclusive creation, cleanup, committed retention, symlink substitution and foreign inode replacement. Real CLI tests establish fresh bundle/replay export and no overwrite.
