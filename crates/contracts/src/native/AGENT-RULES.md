<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Checked native adapter rules

- Own explicit checked protobuf/domain conversions, typed service dispatch, domain Status details and the generated NativeOperationView. Protobuf is wire authority; domain validators and bounded persistence serde remain semantic authority. No JSON tunnels, gRPC-Web, scheduler, storage or kernel dependencies.
- Decode both directions through checked TryFrom and domain validate methods: IDs, finite vectors, ordered bounds, unit poses, native host path encoding, bounded pages/chunks, intent constraints, hash/size/resource correspondence. Never normalize invalid poses or invent missing fields.
- Native operation names are sessions/{UUID}/operations/{UUID}; immutable resources retain artifacts/{SHA256}. Private numeric task IDs and packed artifact IDs are not observation handles. Acceptance does not imply completion or scene attachment.
- Diagnostic codecs/limits are separate. Native snapshots require exact Any types, nonzero version, valid session/state/progress/timestamps and terminal/result/error consistency; compiled/verified resources must appear in immutable outputs. Interrupted restart work stays terminal and never resumes handles.
- Preserve the domain discriminator, exact code and bounded message in one typed DomainErrorDetail. Short wire replies have no error arms; checked shell adapters may expose recoverable domain Response::Error values.
- A failed committed Save with Project Io may additionally carry exactly one NativeOperationResult Any containing ProjectSaved with checked save_uncertain=true ProjectInfo in Status.details. NativeOperationView retains this operation-specific attachment receipt alongside the error without changing Google's terminal error/response oneof. Reject duplicate, unknown, wrong-domain, wrong-result or certain receipts; never replace the receipt with a later sticky Get status.
- Exercise contracts adapter/invariant tests, generator drift and integrated standard Operations/ByteStream workflows through the main verifier. Packed schema fixtures are unchanged by transport integration.
