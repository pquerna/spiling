<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing inputs rules

- Own original, redistributable printer-specification and intent inputs for the software compiler/replay gate, not physical printer calibration or supported-machine declarations.
- Author printer data in TypeScript against generated protocol types; the same Rust contracts validate inert intent JSON in the engine. Never maintain a second domain schema here.
- Keep profile, material and recipe identity/revision, units, frames and limits explicit. Changes to frozen measured inputs require a new revision and evidence hashes.
- All printer/intent metadata uses manufacturing schema10 without old-schema readers. Diagnostic profile revision10 and complete Marlin profile revision9 identify this cutover. Reference deposition limits declare fixed 0.45mm width and 0.000001–0.30mm height, not physical qualification. Recipes retain positive native_shape_error_mm, explicit sparse-infill policy, nearest seam, ordinary hatch axis and independently required top_skin_axis. Existing fixtures explicitly retain their prior hatch axis for top skins. `housing-two-zone.intent.ts` derives complete-sparse physical settings with fixed-X hatch/top skins for the admitted separator construction; ordinary complete sparse remains alternating XY. Paired `housing-h1-fine`/`housing-h1-combined` intents share the provisional 0.15mm nominal schedule, fixed-X ordinary hatch and declared Y top-skin finish, changing only sparse_infill (fine versus combine_eligible up to 0.30mm). Their presence is not admission evidence. Whole-owned and finite source-box process overrides, including explicit separator permission, are project-owned, not extra recipe fields. Source STEP corpora and configuration/calibration bytes remain unchanged.
- Component shapes are descriptive metadata, not proof of collision checking. Material/flow/temperature values are declared software inputs, not measurements. Diagnostic and complete Marlin profiles remain explicitly unqualified; configuration/calibration record hashes identify exact published bytes, not firmware approval. Revise records and their profile hashes together.
- Compile trusted TypeScript with `pnpm manufacturing:intent SOURCE.ts NEW.json`, then exercise actual CLI compile, independent replay, checkpoint and fresh-process reopen. A type check alone does not establish manufacturing correctness.
