<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Reference filament catalogs

`data/material-catalogs/` retains exact JSON snapshots from [Prusa FDM Mixer](https://github.com/prusa3d/prusa-fdm-mixer/tree/main/data). The engine embeds these snapshots for offline native/CLI discovery through the bounded Libraries SearchCatalog service. Search is read-only; selecting a row opens an editable declaration before an explicit personal-library write. Discovery does not create inventory or add project materials. [Library contracts](../protocol/library.md) continue to govern those writes.

## Refresh and integrity

```sh
pnpm materials:sync                       # resolve upstream main once, then fetch at its exact commit
pnpm materials:sync --revision COMMIT     # reproduce a reviewed exact 40-character commit
pnpm materials:check                      # offline schema, bytes, hashes, counts and notices
pnpm test:material-catalogs               # failure, identity, bounds and publication regressions
```

[`tools/material-catalogs.mjs`](../../tools/material-catalogs.mjs) owns admission and publication. Both catalogs and Prusa's MIT notice come from one exact commit. The separate OpenPrintTag MIT notice retains its own notice-owner pin; this is **not** the original data revision consumed by Prusa, which its aggregate does not record. `manifest.json` records these distinctions, exact hashes, byte sizes, counts and source URLs. Preserve raw files; do not hand-edit or reformat them. `pnpm check` includes the offline check; ordinary checks need no network.

A refresh downloads and validates the complete candidate before changing the accepted directory. Files are bounded to 8 MiB, catalogs to 32,768 rows each, notices to 16 KiB and each network request to 30 seconds. Unknown fields, duplicate source IDs, invalid RGB/TD, count mismatch and missing notices refuse. Disappearing IDs or brands refuse unless an operator has reviewed the diff and deliberately uses `--allow-removals`; scheduled updates must never use that override. Value changes are reported separately. A timestamp-only refresh keeps the prior accepted bytes and revision.

Review the generated diff and summary, run the check and tests, then explicitly stage the catalog directory and publish by normal Git updates. Periodic refresh uses this same command; it has no runtime updater or dependency on a catalog service while slicing.

A sibling `.lock` prevents concurrent writers. Publication stages a checked directory, retains the previous directory as `.previous`, renames the staged directory into place, then removes the backup. This maintenance tool does not provide PTAC durability or uninterrupted concurrent reads. After a crash, preserve the directories, establish that no writer is running and run the offline validator on each retained candidate (its exported `checkSnapshot(path)` accepts another location). Restore a complete validated snapshot before removing a stale lock or backup. Never delete recovery files merely to make a refresh pass. Unrecognized local files or changed hashes also require explicit reconciliation.

## Identity and interpretation

Keep HueForge and OpenPrintTag catalogs separate. A row is identified by its catalog source plus upstream `id`; this is neither a durable product SKU nor proof that another source's similarly named row is the same filament. Upstream slugging/deduplication can collapse distinct vendor records, and its synchronizer can publish a partial vendor set. Our disappearance guard protects the accepted baseline; it cannot reconstruct records already absent from that baseline. No cross-source joins or automatic conflict winners are introduced.

`hex` is a declared sRGB color. Optional `td` is HueForge-style transmission distance in millimeters. Missing TD remains unknown. Do not convert it into a physical attenuation length, promote it to measured/calibrated evidence or derive machine/process limits. Names, colors and TD do not establish spool ownership, available stock, physical color qualification or a printable material profile. Selected definitions retain provider-local identity, exact snapshot/content hashes and original declarations alongside editable current fields. Diameter remains unknown unless entered. Catalog updates in the native inspector compare the exact captured source key with the bundled record and require review/save; they never change saved definitions automatically. A saved definition has no inferred process profile; spool registration and project capture remain explicit independent actions.

## Distribution

Retain both original MIT notices beside the raw data. [NOTICE.md](../../NOTICE.md) and [licensing](../license.md) distinguish these imports from the separately pinned Prusa v7 math port. Release provenance validates the snapshots, records each catalog as a separate source component and copies both notices and the complete manifest. The tracked public source archive includes the data and engine binaries embed the admitted snapshots.

## Native acquisition checks

Use Prepare → Material → Choose filament → Choose from My library, or Project
hardware → Add printer. Exercise search, selected-result keyboard review, manual
entry and explicit return at normal and narrow window sizes in Graphite/Paper.
Check a saved imported definition against the bundled catalog, then inspect its
self-contained project copy after save/reopen. The Acquisition gallery uses
illustrative rows through the production controls; it does not establish engine
or storage behavior.

For local debug builds, `SPILING_CATALOG_TIMINGS=1` records query-edit to matching
results-layout latency, including debounce and RPC observation. Measure the first
index load separately from warm samples. This ends at egui layout, not GPU
presentation; it is not a display latency or platform performance certification.
