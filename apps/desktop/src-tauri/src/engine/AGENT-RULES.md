<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop engine supervision rules

- EngineClient owns the child; EngineRpc owns cloneable RPC access. Hold the supervisor mutex only for lifecycle/status ownership, never an operation observation or artifact transfer.
- Check the closing gate before and after acquiring lifecycle ownership. Shutdown/restart clean up the previous child; status observes OS exit. Preserve stopped/running/interrupted meanings.
- Store diagnostic operations in the application local-data directory. No authentication capability enters persistent storage.
- Shell operations expose generated OperationView metadata and bounded raw binary IPC; no JSON/base64 bytes or independent domain schema. Ordinary job/RPC errors do not disconnect the engine.
- Resolve the executable relative to installation, or explicit absolute development override. No protocol-version override or cross-version shims.
- Validate through native invokes, concurrent cancellation and process cleanup; software/headless GPU runs are not hardware acceptance.
