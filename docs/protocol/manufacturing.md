<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software-only manufacturing protocol and schema

`spiling-contracts::manufacturing` is executable schema authority; generated TypeScript is the authoring data language. The Rust manufacturing crate owns planar compilation, a narrow emitter and independent replay. Core owns transactions/storage. The engine composes them, and the shared client/CLI consume their results. These are software-validation artifacts, **not machine-ready programs, physical printer support or printability certification**.

This is protocol 4 / project format 2, with manufacturing data schema 1. There is no old-format decoder, migration shim or implicit runtime profile/recipe default. SPLT/SPLM/SPLS remain schema 1. Historical milestone evidence describes its labeled historical version, not current support.

## Authoring and trust boundary

An operator may author a TypeScript data module against generated Rust-defined types. Build tooling may execute that explicitly trusted local authoring module to produce JSON; executing authoring code has the operator's permissions and is not a sandbox. The engine never executes TypeScript: it receives bounded inert JSON, rejects unknown/duplicate fields and trailing JSON, and validates it in Rust. A schema capability declaration does not enable unsupported behavior. Ordinary printer data does not require a new backend; genuinely custom behavior requires separately reviewed Rust implementation.

All names, IDs and revision labels below are nonempty strings of at most 256 UTF-8 bytes, without control characters. Numeric process parameters must be finite and positive. Unknown enum discriminants fail decoding. Profile/intent JSON is at most 32,768 bytes. All fields below are required, including explicit `null` manifest option values.

## Printer and recipe data

`PrinterSpecification` fields:

- `schema_version: 1`, `id`, `revision`, `name`.
- `coordinate_frame: "right_handed_millimetres"`: fixed Cartesian XYZ, right-handed millimetres. Layer planning uses the bed plane at `build_envelope.min[2]`, not an assumed global Z=0.
- `build_envelope: AabbMm {min:[f64;3],max:[f64;3]}`: finite ordered strictly positive dimensions.
- `components: MachineComponent[]`, at most 64, unique component IDs.
- `capabilities: PrinterCapabilities {motion_space:"cartesian_xyz",extruders:1,output_dialect:"cartesian_absolute_gcode_v1",generated_supports:false}`. Other extruder counts/support requests are typed unsupported capabilities.
- `nozzle_diameter_mm`, `max_feed_mm_s`, `max_volumetric_flow_mm3_s`.

`MachineComponent` has `id`, `role: "bed"|"toolhead"|"fixture"`, a checked `pose: RigidPoseMm {translation_mm:[f64;3],rotation_xyzw:[f64;4]}`, and `shape`:

- `{kind:"box",bounds_mm:AabbMm}` with positive dimensions in component-local millimetres.
- `{kind:"cylinder",radius_mm:f64,height_mm:f64}` with positive dimensions; cylinder is local +Z with base at local origin.

Pose is proper finite rigid motion with a unit quaternion: no scale, shear or reflection. Shapes are retained metadata only. They provide neither collision verification nor a rendered/executable machine model.

`PlanarPrintRecipe` requires `id`, `revision`, `material_id`, `material_revision`, `filament_diameter_mm`, `layer_height_mm`, `bead_width_mm`, `perimeter_count`, `infill_fraction`, `print_speed_mm_s`, `travel_speed_mm_s`, `flow_multiplier`. Perimeters are 1..8; layer height is at most bead width; only `infill_fraction:1.0` is implemented. Print/travel speed must not exceed printer feed limits. Nominal bead area is `(width-height)*height + pi*(height/2)^2`; flow is area × multiplier × print speed and must not exceed the explicit volumetric limit. Filament area is `pi*(diameter/2)^2`. Numeric overflow/underflow that destroys finite positive areas is rejected. These are declared geometric accounting models, not material calibration or measured deposition physics.

`ManufacturingIntent {printer:PrinterSpecification,recipe:PlanarPrintRecipe}` validates both declarations and their cross-constraints. `decode_intent(bytes)` bounds input before decoding; direct DTO consumers must call `validate()`.

## Normalized plan

`NormalizedPrintPlan` has `schema_version:1`, `layers`, positive finite `sampling_tolerance_mm`, `offset_tolerance_mm`, `layer_quantization_mm`.

A `PrintLayer` contains zero-based consecutive `index`, deposition `z_mm`, conceptual interior sample `section_z_mm`, and nonempty deposition `paths`. Fixed-height deposition is bed+(index+1)×height within declared quantization; section sample is strictly between bed+index×height and deposition. Planner policy uses midpoint samples and rejects a global top not integral-height within 0.000001 mm, rather than invent a partial top. Empty per-occurrence native sections are allowed, but every aggregate layer must deposit material: an omitted intermediate or final layer is invalid before replay.

A `DepositionPath` contains `kind:"perimeter"|"solid_fill"` and at least two finite `points_mm:[f64;3][]`. Every point lies in the explicit envelope and at the layer's exact deposition Z. Segments have finite positive XY length. A perimeter repeats its first point as its final point; closure is never implicit. Width/flow/speeds are recipe-wide, not redundant per-point fields. Plans are capped at 4,096 layers and 100,000 deposition segments. JSON sequence admission bounds component/layer/path/point/provenance arrays; nested layer/path/point seeds share one explicitly owned cumulative segment budget and reject excess before growing the point vectors. Empty/one-point paths cannot bypass admission. Input byte caps additionally bound JSON allocation, not kernel-private RSS.

Native sections use the admitted BREP definitions and rigid occurrence transforms. Same-material sections are unioned; holes/islands are retained. Walls are inward offsets; solid clipped hatch alternates axes. No mesh repair, generated supports, silently removed thin features or synthetic triangle fallback. Unsupported geometry produces diagnostics. Source/native sampling, offset and output quantization are separately declared numerical budgets, not physical dimensional accuracy claims.

For exactly world-vertical analytic carriers, XY geometry is invariant in open slabs between horizontal faces. Compilation natively samples and unions each slab once, checking support at transitions and reusing both hatch-parity path shapes while assigning each layer its explicit Z. Residual floating tilt admitted by carrier tolerances prevents reuse: every layer is natively sampled instead, so reuse cannot accumulate an undeclared XY error. Horizontal-face alignment uses `min(0.000001 mm, height*0.000001)`; midpoint fields describe the layer's sample plane, not a native-query log. Every emitted layer still charges the aggregate segment budget.

## Provenance, reports and immutable bundles

`ManufacturingProvenance` contains `project_id`, captured `input_revision`, `input_hash`, `definitions:StoredDefinition[]`, `occurrences:OccurrenceRecord[]`, explicit `intent`, `kernel:{name,version,revision}`, and explicit `planner_version`, `emitter_version`, `verifier_version`. Captured persisted source labels/units/uncertainty remain authoritative; native reimport labels must not replace them. Definitions/occurrences obey shared identity/reference/pose/provenance caps and every definition is referenced.

`input_fingerprint(project_id,definitions,occurrences,intent)` validates these inputs, sorts definitions by definition ID and occurrences by occurrence ID, then SHA-256 hashes the UTF-8 compact JSON emitted by Rust serde_json in this field order:

```text
{schema_version:1,project_id,units:"mm",frame:"right_handed",definitions,occurrences,intent}
```

Every record/intent field uses its Rust declaration order and serde spelling, without whitespace. Source bytes enter through source SHA-256 and definition identity; source units, retained labels and declared uncertainty are included. Revision/allocator/history/artifacts/timing are excluded. Algorithm/kernel provenance is additional bundle content identity. This is a Rust-defined canonical serialization algorithm, not a promise that arbitrary JavaScript JSON number formatting is interchangeable. Get/status does not rehash geometry on its hot path.

`VerificationReport` fields are `verified`, bounded `coverage:String[]`, `limitations:String[]`, `deposition_segments`, `travel_segments`, `deposited_volume_mm3`, `filament_length_mm`, `max_position_error_mm`, `max_extrusion_error_mm`. Measurements are finite nonnegative; description arrays are at most 64 entries of 256 bytes each. Stored `verified:true` is **not trust**: reopening/inspecting must independently decode/replay the actual program before reporting verified.

`ManufacturingSummary` contains `layers`, `paths`, `deposition_segments`, `deposited_volume_mm3`, `filament_length_mm`, `software_only:true`; counts/positive measurements are validated against resource limits.

`ManufacturingBundle` contains exactly `schema_version:1`, `provenance`, `plan`, `program:String`, `verification`. The immutable asset hash covers exact compact JSON UTF-8 bytes, with no self-reference. `ManufacturingArtifactRecord {hash,input_hash,byte_count,summary}` is manifest metadata; hash/input hash are canonical lowercase SHA-256 strings. Byte count is nonzero and at most 16 MiB. Each retained bundle Vec allocation is also at most 16 MiB; the separate 64 MiB active/staging/history budget charges actual allocated capacity and deduplicates shared assets. Disk/wire byte counts are serialized length, not capacity. Source storage limits remain separate. Assets are written/synchronized before manifest replacement; recovery checkpoints include coherent intent/artifact references; no asset GC. See [project format](project.md).

Public schema APIs are printer/recipe/component/capability/intent `validate()`, plan `validate(&intent)`, provenance/report/summary/artifact/bundle `validate()`, `decode_bundle(bytes)`, bundle `summary()` and `validate_record(&record)`. Summary/record matching require an already validated bundle. Record matching checks semantic input hash and summary; caller separately checks exact actual-byte hash and byte count. Schema validation does not parse/replay the program or confer replay success.

## Emitted program and independent replay

Program is conspicuously labeled `SOFTWARE VALIDATION ONLY, NOT MACHINE READY`. The supported dialect sets G21, G90, M82, G92 E0, then uses only G0/G1 absolute XYZ/E/F. There are no home/heat/start/end firmware macros, machine submission, retries or execution connectors. Emit XYZ with six fractional digits, E with eight and F with six; rounding must retain positive segments/extrusion and remain within declared bounds.

An independent verifier decodes command state from emitted bytes, not emitter helpers or a copied move buffer. It checks modes/sequence, malformed/duplicate/nonfinite fields, envelope, speed, nominal flow, positive deposition versus travel, expected path ordering/counts, positions and extrusion. Missing/extra/tampered commands fail verification. Replay coverage explicitly excludes physics, collisions, component swept volumes, startup/homing, temperature, firmware behavior, calibration and physical printer support. A successful program replay proves only the declared software semantics.

## Native services and immutable retrieval

The generated `spiling.engine.Manufacturing` gRPC service owns native control. `Execute` accepts a typed `ManufacturingRequest` oneof for Get or SetIntent; replies contain a typed status. `Compile` and `Verify` return standard `google.longrunning.Operation` resources. Both capture an explicit `ProjectRevision`, session parent and optional admission request UUID. These are not framed JSON requests, and no binary frame channel remains.

The Rust/TypeScript shell adapter uses `ManufacturingCommand` for `get`, `set_intent`, `compile` and `inspect`; `inspect` maps to Verify with the captured project revision. Status contains project info, optional intent, optional artifact record and an optional immutable resource descriptor. The adapter reports accepted work as `operation_accepted {operation:NativeOperationView}`, with a Google operation name, never a public numeric job handle.

Native metadata and result use the protobuf `NativeOperationMetadata` and `NativeOperationResult` Any types. Compiled results contain project info, record and resource; verified results contain record, fresh report and resource. Standard Operations Get/Wait/Cancel and Engine WatchOperation provide observation and explicit cancellation. Dropping or timing out a unary RPC does not cancel accepted work. Unfinished retained operations become interrupted/aborted on engine restart; old session results are historical, not active native handles.

Compilation captures immutable project inputs and their persistent revision. Core remains sole writer. After compile/replay, ACK publication checks project/session/revision/cancellation again before the normal artifact transaction. Intent/geometry changes invalidate artifact in the same transaction; undo restores coherent inputs+artifact. Publication advances persistent revision, but provenance keeps the original compiler input revision. Read-only allows inspect/replay, never intent edit/compile publication. Durable operation output and success are published only after native promotion ACK/core commit.

Artifacts.GetArtifact and standard Google ByteStream.Read retrieve immutable bundle bytes by the returned resource descriptor. Descriptor size/hash must match the manufacturing record; bound the total before allocation and validate streamed size, exact actual-byte SHA-256, strict UTF-8/schema, captured semantic input hash and summary. Bundle size remains at most 16 MiB. There is no ReadChunk command, correlation frame header or kind-4 payload. ByteStream flow control is separate from job observation/cancellation.

Canonical gRPC/Google Status carries typed domain error details. Domain errors remain recoverable; transport/contract failures do not fabricate success or retry uncertain mutations. Saved bundle JSON and the printer-authoring JSON are domain/storage formats, deliberately distinct from protobuf RPC messages.

Manufacturing errors are `invalid_specification`, `unsupported_capability`, `unsupported_geometry`, `no_intent`, `empty_project`, `stale_revision`, `verification_failed`, `resource_limit`, `cancelled`, `io`, `corrupt_artifact`, `read_only`, `busy`. Messages are bounded to 2,048 UTF-8 bytes. Invalid specifications are not replaced by defaults or fake fallbacks.
