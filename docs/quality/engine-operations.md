<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Engine operation ownership

[Control protocol](../protocol/control.md) owns authentication, service schemas, lifecycle, limits and ByteStream semantics. [Authoring](authoring-evidence.md) maps Core/storage. Operations are durable observation records, not native handles or a second project authority.

## Follow admission to publication

- [Jobs](../../apps/engine/src/jobs/mod.rs): `Jobs::open` holds the exclusive ledger lock and configures SQLite WAL/FULL; `start` admits diagnostics. [Native admission](../../apps/engine/src/jobs/native.rs), `start_native` / `accept_native`, persists typed request identity and captured revisions before acknowledgement. Retained equivalent UUID retries deduplicate before live-session checks; conflicting parameters fail. RPC drop does not cancel admitted work.
- [Native dispatcher](../../apps/engine/src/native.rs) owns short controls and output capture; [Worker](../../apps/engine/src/geometry/worker.rs) alone runs kernel/storage work. One shared execution permit covers diagnostics/native jobs. [Runtime::event](../../apps/engine/src/geometry/mod.rs) checks cancellation/staleness, reserves pins, sends promotion/discard, then waits for matching worker ACK before Core/scene publication. Staged output is never public.
- [Watchdog / CommitToken](../../apps/engine/src/session/mod.rs) serialize cancellation with promotion/save commit. Cancellation records intent, not rollback; committed results win late cancellation. Missing safe-boundary ACK terminates the engine rather than inventing successful cancellation. Owner EOF/shutdown drains native work before interrupting unfinished ledger records.
- Dispatcher `publish_terminal` captures immutable bytes before another short edit can intervene; `Jobs::native_finish` atomically commits descriptors/blobs and terminal outcome. `terminal_error` preserves a committed uncertain-save receipt with its typed IO failure. Ledger failure after native/disk commit exits the process; reopen/reconcile explicitly, never auto-retry.
- `Jobs::open` / `interrupt_all` mark unfinished work Interrupted/ABORTED without resuming computation. Terminal states never change. Durable historical operations/blobs survive restart; expired session references cannot activate old scenes. Watchers coalesce complete committed snapshots rather than replaying every event; partial diagnostic output means complete individual artifacts, not successful overall work.

## Minimal diagnostic flow and limits

```sh
target/debug/spiling-cli job --engine "$PWD/target/debug/spiling-engine" --chunks 4 --delay-ms 250
```

Use matching built binaries; progress is stderr and final JSON follows child cleanup. The [canonical service procedure](../protocol/control.md#exercising-the-actual-services) covers persistent-store/request-ID reuse. Admission, output reservations, observers and downloads are independently bounded; exhaustion refuses explicitly, with no deletion/GC API. Complete bytes use checked immutable descriptors and ranged ByteStream reads. Diagnostic/native bridge success does not establish geometry correctness, automatic computational resumption, remote-service security, power-loss durability, physical GPU or machine qualification.
