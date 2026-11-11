<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Hardware inventory and portable planning captures

Rust contracts in `crates/contracts/src/hardware.rs` own hardware identity and
validation. The typed native API is `native_hardware.proto`, Libraries and
Projects; the CLI and native workbench use the same engine services. Hardware
reuses the personal [library container](library.md), its location preference,
writer leases, revision comparison and request receipts. There is no second
hardware database or automatic project synchronization.

## Identity and ownership

| Record                  | Meaning                                                                                                                    | Storage                                                                        |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Printer reference       | Public manufacturer/model declarations and optional build size; no inferred mount or process settings                      | Built-in reference discovery; exact original row retained by personal identity |
| Setup                   | Complete existing `PrinterSpecification` and public model, with an inspectable content revision                            | Personal machine record; copied into project                                   |
| Owned machine           | Physical instance UUID, model identity, private nickname/serial/location and explicit unconfigured or complete setup state | Personal library only                                                          |
| Plate definition / face | Public mounting interface, thickness, installed surface datum, faces, usable bounds, keepouts and temperature limits       | Personal plate; copied into project                                            |
| Owned plate             | Physical sheet UUID, private nickname/condition and optional installed machine                                             | Personal library only                                                          |
| Accessory               | Public nozzle declaration or rectangular fixture; owned instance can be installed on a machine                             | Personal library; installed definitions copied into project                    |
| Route                   | Physical machine UUID, closed connection family, endpoint, expected peer, generation and optional credential reference     | Personal library only                                                          |
| Project capture         | Complete setup, plate, selected face and installed accessory definitions                                                   | Project snapshot, history, manifest and manufacturing intent/provenance        |
| Remembered selection    | Project UUID plus exact local attachment, capture hash and owned machine/plate/generation                                  | Personal library only                                                          |

Names are display values, never identity keys. Two identical machines or sheets
remain distinct physical records. Portable setup/plate labels are explicitly
public; private nicknames are never used to synthesize those labels. Custom
public definitions are operator declarations, not calibrated machine evidence.

`hardware-catalog.ts` is trusted TypeScript authoring data against the generated
Rust domain view. `pnpm contracts` generates inert `hardware-catalog.json`;
`pnpm check` checks drift and TypeScript. Rust validates the embedded catalog.
`printer-references.ts` separately authors small manufacturer-cited identity declarations; its generated inert JSON shares offline `SearchCatalog` with filament discovery. It imports no external slicer scripts or compilation. Unknown build size remains unknown. The setup catalog supplies the existing diagnostic Cartesian and complete Marlin
software configurations and a two-face reference sheet. H2C is a catalog-only
identity with unknown declared volume and no executable setup. Adding a model
record cannot authorize a dialect, firmware, connector or physical process.

## Selection, copying and invalidation

My Library's **My Hardware** view owns Machines, Build plates, Accessories and
Catalog. Users can find an exact model or enter manufacturer/model manually, record multiple physical instances, edit private instance information,
author custom setups and plate faces, record installation, and save connection
configuration. The project view and Prepare's **Choose hardware** action review
and explicitly apply a machine/plate/face. A captured setup and its accessories
can also be copied back into My Hardware, including from a read-only project.

An owned machine carries `PrinterIdentity` and tagged `MachineConfiguration`. `Unconfigured` is valid inventory and displays **Setup needed**. It supplies no mount, nozzle, dialect or process limits. Installing a plate/accessory or capturing it for a project requires a complete setup; a configured setup must name the same manufacturer/model. Selecting a reference never silently adopts diagnostic Marlin settings. Exact admitted configurations are deliberately selected; a manually entered model can instead start from explicit software settings with an operator-entered mount. Those declarations do not qualify physical hardware. Core advances installation generation when identity or configuration changes; nickname edits alone retain it.

Native acquisition uses the shared search/review page, then a bounded editor with visible Add/Save and discard actions. CLI `library catalog --kind printer --query TEXT` and `library add-reference RECORD.json` save source-only inventory; `library add-machine NAME --manufacturer BRAND --model MODEL` saves manual inventory. `--setup ID` explicitly starts a complete admitted configuration. Rich setup edits continue to use typed `replace-hardware`/receipt requests. Project captures remain complete and contain no private instance identity.

`Projects.SetHardware` works before a manufacturing recipe exists. Core commits
the complete capture and, when present, the recipe's printer/capture atomically.
Existing material-profile pins are validated; selecting hardware cannot silently
discard an incompatible profile. Captured single-tool material profiles own their resolved nozzle/bed operating targets; these may differ from the installed setup's initial thermal values. Every other installed printer field remains exact, and the material resolver validates the complete capture. Reselecting hardware preserves those bound process temperatures. Authoring history restores the exact closure.
An identical capture preserves revision and compiled artifact, allowing another
identical physical instance to be selected locally. A changed capture invalidates
the artifact; Undo restores its original coherent inputs/artifact. Removing the
capture retains the explicit recipe printer rather than inventing a default.

Private names, routes and credentials do not affect project inputs. Core advances
machine generation for installed setup changes and route generation for
endpoint/peer/family/auth changes. Changing an endpoint, expected peer, family or
machine refuses retention of the old credential reference. Two windows must
review the current library revision; stale writes do not silently merge.

A confirmed selection on an attached project records its private association.
The workbench carries an unsaved selection through confirmed Save and Save As.
CLI `--hardware-local` records the association after confirmed save. Failure to
publish this separate association does not undo or misreport a successful project
save. Reopen matches project UUID, print-layout UUID, exact local attachment, capture hash and
installed generation/content. A changed installation is shown as different;
opening never adopts it. A missing, locked or corrupt personal store cannot
prevent opening, compiling or independently replaying a standalone project.

## Consumed compatibility and explicit limits

Capture validation checks exact mount identity, selected face existence, installed
plate ownership, nozzle agreement and face-specific bed temperature. Equal plate
dimensions do not imply mount compatibility. The installed top surface datum must
equal the current backend's bed Z. Thickness remains a separate declaration;
unsupported datum offsets refuse instead of shifting geometry or applying a
second thickness correction. Fixture geometry is bounded axis-aligned boxes;
curved clips, transformed fixtures and arbitrary custom accessory behavior are
not supported.

Planner preflight checks conservative final occurrence bounds against the machine
volume, selected face's usable XY rectangle and 3D keepouts/fixtures. This can
conservatively refuse an AABB that spans a void; it does not claim exact fixture
collision analysis. Fresh independent replay checks actual decoded deposition
bead bounds, including width and physical height, against those same captured
constraints. Purge/travel remain governed by the machine workflow rather than the
part's plate placement rectangle. Swept travel/toolhead collision remains
unqualified. These semantics are bound by planner68/verifier29; emitter14 and the
existing numerical document/binary versions remain unchanged.

The capture is at most 24 KiB and the complete hardware inventory at most 48 KiB,
within the native control limit. Count limits are additional ceilings, not a
promise that every maximum fits the byte budget. Core charges distinct retained
capture allocations across current/history/prepared snapshots against 2 MiB;
loaded recipe and snapshot references share the validated immutable capture.

## Credentials and connection availability

`engine-client::hardware::credentials` is the native secret owner. New slots use
pinned `keyring` 4.2.0, with native Keychain, Windows or Secret Service providers.
If native store initialization is unavailable, a new slot uses a private plaintext
file. Access denial is explicit. An existing reference always uses its recorded
backend: missing/locked/deleted entries never switch stores or resurrect an old
file secret. Normal lookup never enumerates OS keyring entries.

The service is `org.spiling.hardware`; account names are
`v1.<canonical-nonnil-slot-uuid>.<closed-role>`. Roles are `lan-access-code`,
`api-key`, `password` and `refresh-token`. Private library records carry
`CredentialRef { slot_id, role, backend }`; project captures carry none. Secrets
are bounded to 8192 bytes, use redacted transient input and zeroizing native/UI
buffers, and never enter domain serialization or provider error text.

Fallback files live below the platform user-data directory at
`credentials/hardware/<slot-uuid>/<role>.secret`, independent of a movable
personal library. Unix directories/files require 0700/0600 permissions. Reads
refuse symlinks and unsafe modes; replacement uses a private temporary file,
atomic publication and directory sync. This is plaintext storage, and the UI
identifies it as such. Windows ACL behavior and native OS provider lifecycle still
require platform-specific acceptance; Linux file tests do not qualify them.

Credential rotation creates a new slot on the previous reference's backend,
commits the new reference with native library CAS, and retires the previous slot only after confirmation. Forget first
unlinks the reference, then deletes the exact slot. Before sending a credential-reference write, the workbench durably records its
exact library attachment/identity, original request, new slot and deferred deletion
in a bounded private journal. Restart recovers those operands for the matching
library; explicit Resolve repeats the same idempotent request. Observation never
resubmits. Confirmed or definitively refused writes retire only unreferenced slots
and remove the journal. A crash between creating a slot and journaling it may leave
an unreferenced secret; opening never grants it authority or automatically retries.

**Only existing verified file/media export is available.** A File route means a
local save through the shared export transaction, never remote acceptance or
start. Bambu Connect, Bambu developer LAN, Moonraker, OctoPrint and PrusaLink
records are explicitly labeled configuration-only and unsupported in this build.
No probe, upload, start or automatic mode-switch control is exposed. Development
simulator identity is separate and confers no production authority. Qualifying a
direct family requires an actual named device/API/firmware/backend cohort,
private durable submission journal, permissions, wrong-peer and timeout handling,
unknown-outcome reconciliation, and separately authorized start. These are not
established by an inert route or by software program verification.

## Reproduction

Run `pnpm test:hardware` for actual separate CLI/engine processes: create three
same-name machines and two plates, capture a 10 mm cube job, compile/replay/export,
remember private selection, remove source/intent/personal-library availability,
and reopen/replay/export the standalone project with exact equality for all four
saved export files. A further fresh compilation must reproduce G-code and
verification bytes, the semantic plan and provenance. Only the recorded geometry
work counter and the new compilation input revision are excluded from this second
comparison; their actual values are retained in evidence. Private sentinels are
checked in the project and exports. Commands/output/hashes live under ignored
`.cache/`.

Run native host acceptance after building the engine:

```sh
SPILING_TEST_ENGINE="$PWD/target/debug/spiling-engine" \
  RUST_TEST_THREADS=1 RUST_MIN_STACK=8388608 \
  cargo test -p spiling-workbench hardware_selection_save_as_restart -- --ignored
```

Contracts/Core/client tests exercise strict native round trips, invalid captures,
CAS/generation/credential-scope rules, history/save/load and file secret behavior.
The manufacturing verifier's independent numeric golden tests reject beads whose
centerlines fit but whose width intersects a face limit or fixture. Native host
acceptance covers Save As, identical-instance replacement, restart and missing
personal storage. These are software outcomes; programs remain **NOT MACHINE
READY**, with physical printing and production connectors unqualified.
