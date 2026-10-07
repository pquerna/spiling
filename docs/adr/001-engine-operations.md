<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Engine operations use gRPC and standard Google resources

Status: adopted for the diagnostic operation implementation.

## Decision

Replace the bootstrap SPLG frame transport with generated Protobuf/gRPC native services. Use the actual Google long-running Operation/Operations, canonical Status and ByteStream definitions. A typed action accepts work promptly; observation, explicit cancellation and immutable binary retrieval are separate operations. Engine and consumers evolve in the same repository without cross-version compatibility infrastructure.

The Rust shell and CLI share a client. The webview receives generated Rust adapter views and raw binary invokes, not gRPC-Web. A parent-owned child binds authenticated ephemeral loopback; inherited pipes supply startup readiness and ownership liveness. Domain services remain independent of Tauri.

SQLite transactions persist deduplicated admission and output/state publication. A supplied optional request UUID follows AIP-155. Complete operation snapshots support polling or coalesced watching; ByteStream supplies range reads. Client RPC cancellation does not own the accepted job or process. Terminal results are immutable and unfinished retained operations recover as interrupted.

## Alternatives and consequences

JSON-RPC with LSP-style progress over stdio would simplify child transport but require another binary delivery/multiplexing design. gRPC supplies generated clients, concurrent calls, streaming and flow control. Neither supplies application capacity, job durability or UI consumption semantics; those remain explicit Spiling responsibilities.

Loopback introduces a local listener and per-launch capability. It assumes a trusted host and is not a remote service. Protobuf generation adds pinned build dependencies and imported Apache schemas whose licensing must remain intact. Wire messages and saved domain state must not become accidentally identical schemas.

The diagnostic implementation deliberately has bounded retained operations/storage and a single cooperative worker. It does not implement authoring state, a generic geometry scheduler, isolated native workers, computational checkpoints or manufactured output. Standards do not justify inventing those capabilities in the current API.

## Validation

The real-process suite exercises long jobs beyond unary deadlines, concurrent retries, cancellation races, partial output, reconnect, restart/deduplication, immutable binary/range reads, slow downloads, admission/observer bounds, authentication, owner EOF and child cleanup. CLI smoke checks actual streamed bytes against the TypeScript decoder. Native shell compile/bridge and CEF/GPU acceptance remain distinct; see [current operation evidence](../quality/engine-operations.md).
