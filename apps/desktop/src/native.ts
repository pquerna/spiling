/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { Hello, OperationView } from "@spiling/protocol";

export interface RuntimeInfo {
  app_build: string;
  runtime: string;
  cef_api_version: number;
}
export interface EngineStatus {
  state: "stopped" | "running" | "interrupted";
  message: string | null;
  hello: Hello | null;
}

export const native = {
  available: isTauri,
  runtime: () => invoke<RuntimeInfo>("runtime_info"),
  start: () => invoke<Hello>("engine_start"),
  restart: () => invoke<Hello>("engine_restart"),
  status: () => invoke<EngineStatus>("engine_status"),
  stop: () => invoke<void>("engine_stop"),
  interrupt: () => invoke<void>("engine_interrupt"),
  runDiagnostic: (requestId: string) =>
    invoke<OperationView>("engine_run_diagnostic", { requestId }),
  getOperation: (name: string) => invoke<OperationView>("engine_get_operation", { name }),
  cancelOperation: (name: string) => invoke<void>("engine_cancel_operation", { name }),
  readArtifact: (name: string) => invoke<ArrayBuffer>("engine_read_artifact", { name }),
  triangle: () => invoke<ArrayBuffer>("engine_triangle"),
};
