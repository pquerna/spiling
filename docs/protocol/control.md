<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# B0 local-pipe control protocol

Rust `spiling-contracts` is the executable schema authority. Its ts-rs generator produces `packages/protocol/src/generated.ts`, including protocol, framing limits, and triangle metadata constants. Generate with `cargo run -p spiling-contracts --bin generate`; append `-- --check` to reject byte drift. Generated output is not independently formatted or hand-maintained.

B0 uses inherited stdin/stdout pipes. Engine stdout contains **only** frames; stderr contains UTF-8 JSON-line diagnostics with `level`, `event`, and `message`. No network listener, CAD import, kernel execution, planner, or printable artifact is implemented. `kernel: "monstertruck"` identifies the configured future default, not a loaded CAD kernel; `geometry_capabilities` is empty.

## Framing

All multi-byte fields are unsigned little-endian integers. The header is exactly 16 bytes:

| Offset | Bytes | Meaning                                 |
| ------ | ----- | --------------------------------------- |
| 0      | 4     | ASCII `SPLG`                            |
| 4      | 2     | Frame protocol version, currently 1     |
| 6      | 2     | Kind: 1 JSON control, 2 binary triangle |
| 8      | 4     | Nonzero request ID                      |
| 12     | 4     | Payload byte length, excluding header   |

Control payloads are limited to 65,536 bytes; binary payloads to 4,194,304 bytes, inclusive. Validate the complete header and bounds **before** allocating or reading the payload. Unknown magic, versions, kinds, zero IDs, oversized lengths, and truncation are errors. EOF between frames is a clean disconnect; EOF inside a header/payload is malformed. An in-memory frame decoder also rejects trailing bytes. The stream decoder consumes one frame at a time.

Only control frames are valid engine requests. The client uses strictly increasing IDs starting at 1, refuses integer wraparound, and permits one in-flight request. The engine also enforces strictly increasing IDs. Every response carries exactly the initiating request ID; the client rejects mismatches before reading a response payload. B0 has no unsolicited frames, event stream, or concurrent request queues.

## Control messages

Control JSON is UTF-8, with a flat internally tagged enum and snake_case `type`. There is no `content` envelope. Unknown fields/types, missing required fields, and malformed JSON are rejected.

Requests:

```json
{"type":"hello","protocol_version":1,"client_build":"0.1.0"}
{"type":"ping"}
{"type":"triangle"}
{"type":"shutdown"}
```

Responses:

```json
{"type":"hello","protocol_version":1,"engine_build":"0.1.0","kernel":"monstertruck","geometry_capabilities":[],"max_control_bytes":65536,"max_binary_bytes":4194304,"pid":1234}
{"type":"pong"}
{"type":"bye"}
{"type":"error","code":"upgrade_required","message":"protocol mismatch: client 2, engine 1; upgrade required"}
```

`Hello` is also exported as a dedicated Rust/TypeScript struct without the enum tag, for desktop invoke responses. `Response::Hello(Hello)` adds `type: "hello"` on the wire without nesting the struct. Build strings are package versions; PID is the actual engine process ID. The client validates negotiated version/limits and verifies PID against its spawned child.

## Negotiation and failure

Hello is mandatory and may occur only once per process. The first request must be hello; other requests fail with `handshake_required`. A hello payload version mismatch produces `upgrade_required`, logs stderr diagnostics, and exits nonzero without executing queued commands. Frames themselves always use the current framing version; the CLI's `--protocol-version` overrides the negotiation payload so incompatibility can be diagnosed using the known framing envelope. An unrecognized **frame** version is a framing error and exits without a response.

After negotiation, ping returns pong; triangle returns one kind-2 frame; shutdown returns bye and exits successfully. Error codes for other fatal control failures are `invalid_request`, `invalid_request_id`, and `already_negotiated`. B0 treats protocol misuse as fatal instead of attempting resynchronization. Invalid framing has no trustworthy correlation envelope, so the engine emits only stderr diagnostics and exits nonzero. Diagnostic messages do not echo an arbitrary untrusted request body.

The shared asynchronous EngineClient gives each complete write/read exchange five seconds and requires exact response IDs/kinds/control variants. It kills and reaps a child after request failures, validates clean shutdown acknowledgement and successful observed exit within five seconds, and exposes OS-observed running status. Explicit terminate sends a kill and waits for exit. Dropping the client enables Tokio kill-on-drop; cancelled in-flight operations also initiate kill. The client inherits engine stderr and exposes no unbounded queues. Unexpected process exits are errors, never successful cached status. The standalone TypeScript triangle decoder validates packed data independently before typed views/upload.

## Synthetic triangle payload

This is a diagnostic display triangle, **not** a BREP or manufacturing artifact. The 64-byte payload has a 16-byte header:

| Offset | Bytes | Meaning                                        |
| ------ | ----- | ---------------------------------------------- |
| 0      | 4     | ASCII `SPLT`                                   |
| 4      | 2     | Schema version 1                               |
| 6      | 2     | Reserved, exactly 0                            |
| 8      | 4     | Vertex count 3                                 |
| 12     | 4     | Index count 3                                  |
| 16     | 36    | Nine little-endian float32 position components |
| 52     | 12    | Three little-endian u32 indices                |

Positions are `[-0.75, -0.6, 0, 0.75, -0.6, 0, 0, 0.75, 0]`; indices are `[0, 1, 2]`. Validate magic, schema, reserved bits, configured count limits, exact size using checked arithmetic, finite coordinates, triangle index count, and every index against vertex count before constructing views or GPU buffers. The canonical hex fixture is [`fixtures/protocol/triangle.json`](../../fixtures/protocol/triangle.json); its adjacent license sidecar records original authorship.

## Real operator smoke

Build both native executables, then run:

```sh
spiling-cli diagnose --engine /path/to/spiling-engine
spiling-cli triangle --engine /path/to/spiling-engine --output triangle.bin
spiling-cli diagnose --engine /path/to/spiling-engine --protocol-version 2
```

Options can appear before or after the command. Without `--engine`, the CLI resolves `spiling-engine` (with the platform executable suffix) beside its own executable. Diagnose prints a JSON object containing `hello`, `ping: "pong"`, `handshake_us`, and `ping_us`. Triangle writes the unframed 64-byte payload when `--output` is given and reports `bytes`, `transfer_us`, `handshake_us`, `synthetic: true`, and PID. Timing uses monotonic elapsed microseconds; transfer timing excludes file output. Reports appear only after successful engine shutdown. Mismatch and file/process errors print stderr JSON and return nonzero.

Run `cargo test -p spiling-contracts -p spiling-engine-client -p spiling-engine` for deterministic boundary/error and actual-child regressions. Linux additionally exercises process disappearance after dropping a client, five-second request timeout cleanup, and cancellation cleanup using an actual stopped engine process (`kill -STOP`). Unit tests do not establish packaged desktop support; the parent integration checks cross-language decoding and real CLI smoke separately.
