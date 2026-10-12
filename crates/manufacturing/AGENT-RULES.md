<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing boundary rules

Own native-section planar planning, inert specification realization and independent emitted-program verification. Depend inward on contracts and the native geometry facade; no storage, engine, filesystem import, TypeScript execution or alternate CAD kernel. Keep planner, backend and verifier distinct. Reuse immutable native definitions for occurrences.

`compile` accepts revision-captured definitions/occurrences/intent and cancellation; successful bundles have undergone independent replay. `verify_bundle` validates provenance, semantic fingerprint and bounded contracts before fresh replay; persisted verification flags are never authority. Versions are pinned and included in provenance. All dimensions are right-handed Cartesian millimetres. No generated supports, partial top layers, implicit repair or silently erased regions. Resource exhaustion and unsupported geometry are explicit errors.

Keep native material sections, immutable shared wall layouts, disjoint skin/interior ownership and deposition generation separate. Physical top/bottom thickness uses conservative whole-layer horizons; bottom owns overlapping skins. Sparse rectilinear fill preserves holes and reports nominal intentional void, not strength or physical coverage. Cache only admitted invariant spans and current wall layout; charge every emitted layer and retained geometry against bounded budgets.

`inspect_bundle` borrows the immutable bundle and caller-owned exact asset hash, returning fresh report/captured provenance and optional `EncodedReplayDisplay {info,bytes}`. Report-only APIs allocate no replay buffers/index/hash. The independent decoder streams actual f64 XYZ/E/F directly to one checked-capacity SPLR byte sink only when selected; no copied motion vector, emitter helper or second parser. Corruption/cancellation drops unpublished bytes. Packed admission and bindings live in contracts; persistence/publication remains the engine's responsibility.

Artifacts remain SOFTWARE VALIDATION ONLY, NOT MACHINE READY. Explicit diagnostic-body and complete single-extruder Marlin workflows have distinct validated dialects; complete jobs declare machine/firmware/configuration/calibration identity, native Cartesian homing, integral Celsius waits, bounded object-excluded priming and final barriers/lift/park/shutdown. Decode emitted bytes independently; separate object and priming accounting. No printer connection, physical homing/thermal truth, collisions, firmware/configuration qualification or printability certification. Component shapes remain metadata.

Meaningful evidence exercises imported native corpus and repeated placed definitions through public compilation and independent replay, plus unsupported thin/support geometry, cancellation, envelope and resource rejection. Exercise `cargo test -p spiling-manufacturing` and the fresh-process durable CLI workflow; tests alone establish no physical support.
