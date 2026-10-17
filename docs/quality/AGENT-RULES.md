<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Quality reference rules

- Help readers navigate current Rust: ownership, entry points, authority/invariants, failure paths, support limits and minimal reproduction commands. Link owning modules and canonical protocols/ADRs; do not duplicate them.
- Rewrite stale sections in place. No checkpoint appendices, dated progress, per-run pass counts, machine inventories, timings, fixture-hash tables or relocated Markdown archives. Git preserves old text.
- Commit/PR notes summarize the changed behavior, exercised verification and known failures. CI or ignored artifacts retain detailed logs, producer/input/environment identities and measurements. Promote a result into Markdown only when it changes a durable support boundary, constraint or procedure.
- Distinguish implemented, exercised, refused and unqualified behavior. Software rendering, CI compilation and local hardware smokes do not establish packaged-platform or physical-machine qualification.
- Prefer one concise owner per fact. Current pins live in manifests/constants; fixture identities live in manifests; wire rules live in protocols; design rationale lives in ADRs. Source distribution and licensing must still describe actual packaged artifacts.
