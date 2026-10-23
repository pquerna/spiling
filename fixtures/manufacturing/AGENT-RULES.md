<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing inputs rules

- Own original, redistributable printer-specification and intent inputs for the software compiler/replay gate, not physical printer calibration or supported-machine declarations.
- Author printer data in TypeScript against generated protocol types; the same Rust contracts validate inert intent JSON in the engine. Never maintain a second domain schema here.
- Keep profile, material and recipe identity/revision, units, frames and limits explicit. Changes to frozen measured inputs require a new revision and evidence hashes.
- All printer/intent metadata uses manufacturing11 without old-schema readers. Diagnostic profile revision11 and complete Marlin profile revision10 declare software-only local wall widths0.30–0.75mm around the nominal0.45mm preference and height0.000001–0.30mm; these are not physical calibration. Source-authored inert JSON follows its TypeScript profile identity. Recipes retain explicit native-shape error, sparse policy, seam, ordinary axis and independent top-skin axis. Housing ordinary uses alternating XY; H1 fine/combined references retain their distinct fixed-X construction and source-bound density/plate/separator intent. Custom native diagnostic envelopes remain explicit.
- Component shapes are descriptive metadata, not proof of collision checking. Material/flow/temperature values are declared software inputs, not measurements. Diagnostic and complete Marlin profiles remain explicitly unqualified; configuration/calibration record hashes identify exact published bytes, not firmware approval. Revise records and their profile hashes together.
- Compile trusted TypeScript with `pnpm manufacturing:intent SOURCE.ts NEW.json`, then exercise actual CLI compile, independent replay, checkpoint and fresh-process reopen. A type check alone does not establish manufacturing correctness.
