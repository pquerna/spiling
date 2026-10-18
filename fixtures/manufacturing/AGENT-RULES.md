<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing inputs rules

- Own original, redistributable printer-specification and intent inputs for the software compiler/replay gate, not physical printer calibration or supported-machine declarations.
- Author printer data in TypeScript against generated protocol types; the same Rust contracts validate inert intent JSON in the engine. Never maintain a second domain schema here.
- Keep profile, material and recipe identity/revision, units, frames and limits explicit. Changes to frozen measured inputs require a new revision and evidence hashes.
- All printer/intent metadata uses manufacturing schema7 without old-schema readers. Diagnostic profile revision7 and complete Marlin profile revision6 identify this contract cutover; every recipe declares a positive native_shape_error_mm budget (0.1 mm for these software inputs), explicit nearest seam and alternating_xy rectilinear-axis policies, never implicit defaults. Source STEP corpora and configuration/calibration record bytes remain unchanged. Original image-relief pattern data exercises spatial mapping through genuine file import, never production shape recognition.
- Component shapes are descriptive metadata, not proof of collision checking. Material/flow/temperature values are declared software inputs, not measurements. Diagnostic and complete Marlin profiles remain explicitly unqualified; configuration/calibration record hashes identify exact published bytes, not firmware approval. Revise records and their profile hashes together.
- Compile trusted TypeScript with `pnpm manufacturing:intent SOURCE.ts NEW.json`, then exercise actual CLI compile, independent replay, checkpoint and fresh-process reopen. A type check alone does not establish manufacturing correctness.
