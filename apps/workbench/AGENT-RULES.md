<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native workbench

- `host` exclusively owns EngineClient on its Tokio runtime. UI frames read coalesced checked snapshots and enqueue bounded actions. Drawing can run more than once per frame; dispatch side effects only in `App::logic`. Never block drawing on RPC, jobs or dialogs.
- Accepted native operations retain their Google operation name until an observed terminal outcome. Observation failure permits Resume or explicit Cancel; it does not authorize resubmitting the mutation. Project snapshots and pending routes must remain distinct. Attach a route only from that operation's appropriate Scene/ProjectSaved receipt, including a receipt-owned uncertain save.
- Paths come from parented native pickers and retain NativePath encoding. Preferences contain theme/license assent, never project authority. Dirty replacement/close confirmation binds project identity/revision and is rechecked before discard. Normal close waits for child cleanup; Drop also reaps.
- `ui` is the production component/theme layer shared with the gallery binary. Command enablement is shared across buttons, menus and shortcuts; text input owns editing shortcuts. The inspection feature is development-only and disabled by default.
- This increment manages projects; it explicitly lacks 3D rendering/picking and manufacturing controls. Do not imply scene display or invent objects. Keep the legacy workbench usable during migration; [ADR006](../../docs/adr/006-native-egui-workbench.md) owns the cutover gates.
- `pnpm dev:native` builds matching engine/window binaries. Actual-child host checks use `SPILING_TEST_ENGINE=/absolute/path cargo test --locked -p spiling-workbench --lib -- --ignored --test-threads=1`; actual-window/picker acceptance is separate, as documented in [development](../../docs/quality/development.md#native-egui-workbench).
