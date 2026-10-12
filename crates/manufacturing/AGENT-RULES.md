<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing boundary rules

Own native-section planar planning, inert specification realization and independent emitted-program verification. Depend inward on contracts and the native geometry facade; no storage, engine, filesystem import, TypeScript execution or alternate CAD kernel. Keep planner, backend and verifier distinct. Reuse immutable native definitions for occurrences.

`compile` accepts revision-captured definitions/occurrences/intent and cancellation; successful bundles have undergone independent replay. `verify_bundle` validates provenance, semantic fingerprint and bounded contracts before fresh replay; persisted verification flags are never authority. Versions are pinned and included in provenance. All dimensions are right-handed Cartesian millimetres. No generated supports, partial top layers, implicit repair or silently erased regions. Resource exhaustion and unsupported geometry are explicit errors.

`inspect_bundle` borrows the immutable bundle and caller-owned exact asset hash, returning fresh report/captured provenance and optional `EncodedReplayDisplay {info,bytes}`. Report-only APIs allocate no replay buffers/index/hash. The independent decoder streams actual f64 XYZ/E/F directly to one checked-capacity SPLR byte sink only when selected; no copied motion vector, emitter helper or second parser. Corruption/cancellation drops unpublished bytes. Packed admission and bindings live in contracts; persistence/publication remains the engine's responsibility.

Artifacts are SOFTWARE VALIDATION ONLY, NOT MACHINE READY: no printer connection, homing, heating, lifecycle macros, collisions, firmware, calibration or physical printability certification. Component shapes remain metadata.

Meaningful evidence exercises imported native corpus and repeated placed definitions through public compilation and independent replay, plus unsupported thin/support geometry, cancellation, envelope and resource rejection. Exercise `cargo test -p spiling-manufacturing` and the fresh-process durable CLI workflow; tests alone establish no physical support.
