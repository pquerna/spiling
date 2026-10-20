<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing inputs rules

- Own original, redistributable printer-specification and intent inputs for the software compiler/replay gate, not physical printer calibration or supported-machine declarations.
- Author printer data in TypeScript against generated protocol types; the same Rust contracts validate inert intent JSON in the engine. Never maintain a second domain schema here.
- Keep profile, material and recipe identity/revision, units, frames and limits explicit. Changes to frozen measured inputs require a new revision and evidence hashes.
- All printer/intent metadata uses manufacturing11 without old-schema readers. Diagnostic profile revision10 and complete Marlin profile revision9 retain software-only inputs. Reference deposition limits declare fixed0.45mm width and0.000001–0.30mm height, not physical qualification. Recipes retain explicit native-shape error, sparse policy, seam, ordinary axis and independent top-skin axis. `housing-two-zone` uses fixed-X hatch/top skins for the separator; complete sparse remains alternating XY. Paired `housing-h1-fine`/`housing-h1-combined` share nominal0.10mm, fixed-X ordinary hatch and alternating-XY top skins, differing only in fine versus combine_eligible up to0.30mm. They also serve the centered physical-thickness housing plate job. Source boxes and tagged separator/plate permission are project-owned, not recipe fields. Source STEP and configuration/calibration bytes remain unchanged. Fixture presence alone is not admission evidence.
- Component shapes are descriptive metadata, not proof of collision checking. Material/flow/temperature values are declared software inputs, not measurements. Diagnostic and complete Marlin profiles remain explicitly unqualified; configuration/calibration record hashes identify exact published bytes, not firmware approval. Revise records and their profile hashes together.
- Compile trusted TypeScript with `pnpm manufacturing:intent SOURCE.ts NEW.json`, then exercise actual CLI compile, independent replay, checkpoint and fresh-process reopen. A type check alone does not establish manufacturing correctness.
