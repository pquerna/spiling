<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Checked native adapter rules

- Own explicit checked protobuf/domain conversions, typed service dispatch, domain Status details and the generated NativeOperationView. Protobuf is wire authority; domain validators and bounded persistence serde remain semantic authority. No JSON tunnels, gRPC-Web, scheduler, storage or kernel dependencies.
- Decode both directions through checked TryFrom and domain validate methods: IDs, finite vectors, ordered bounds, unit poses, native host path encoding, bounded pages/chunks, intent constraints, hash/size/resource correspondence. Never normalize invalid poses or invent missing fields.
- Projects surface commands carry persistent ProjectRevision, live FaceRef and explicit scope/finish oneofs; unknown/unspecified scope or missing kinds fail. ProjectInfo and inspection provenance always carry the full typed bounded ordered surface document. Native face-table admission remains engine-owned.
- Native operation names are sessions/{UUID}/operations/{UUID}; immutable resources retain artifacts/{SHA256}. Private numeric task IDs and packed artifact IDs are not observation handles. Acceptance does not imply completion or scene attachment.
- Diagnostic codecs/limits are separate. Native snapshots require exact Any types, nonzero version, valid session/state/progress/timestamps and terminal/result/error consistency; compiled/verified resources must appear in immutable outputs. Interrupted restart work stays terminal and never resumes handles.
- Manufacturing Verify requires explicit output selection. Verified results require captured inspection provenance and optional checked SPLR descriptor; reject malformed bindings/counts/frame/hash/MIME/size and require replay resources in immutable outputs alongside the original bundle. No program parsing in adapters.
- Manufacturing Preflight is a checked typed Google-operation call/result with captured project revision and bounded software-only report/checks; no artifact outputs or mutation are permitted. Required process/support policies and separate object/support/retraction reports have checked bidirectional mappings. Face identifiers retain canonical STEP strings beyond JavaScript integer precision.
- Preserve the domain discriminator, exact code and bounded message in one typed DomainErrorDetail. Short wire replies have no error arms; checked shell adapters may expose recoverable domain Response::Error values.
- A failed committed Save with Project Io may additionally carry exactly one NativeOperationResult Any containing ProjectSaved with checked save_uncertain=true ProjectInfo in Status.details. NativeOperationView retains this operation-specific attachment receipt alongside the error without changing Google's terminal error/response oneof. Reject duplicate, unknown, wrong-domain, wrong-result or certain receipts; never replace the receipt with a later sticky Get status.
- Exercise contracts adapter/invariant tests, generator drift and integrated standard Operations/ByteStream workflows through the main verifier. SPLR schema3 and native-produced goldens evolve in lockstep; no old schema decoder exists.
