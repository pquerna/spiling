/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  ArtifactId,
  GeometryCommand,
  GeometryResponse,
  Hello,
  ProjectCommand,
  ProjectResponse,
  ProjectPathIntent,
  SelectedProjectPath,
  RigidPoseMm,
  SceneRevision,
  SelectedSource,
  SessionId,
} from "@spiling/protocol";

export interface RuntimeInfo {
  app_build: string;
  runtime: string;
  cef_api_version: number;
  geometry_debug_enabled: boolean;
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
  restart: (discardChanges: boolean) => invoke<Hello>("engine_restart", { discardChanges }),
  status: () => invoke<EngineStatus>("engine_status"),
  stop: () => invoke<void>("engine_stop"),
  interrupt: () => invoke<void>("engine_interrupt"),
  selectSources: () => invoke<SelectedSource[]>("geometry_select_sources"),
  debugSelectSources: (relativePaths: string[]) =>
    invoke<SelectedSource[]>("geometry_debug_select_sources", { relativePaths }),
  project: (command: ProjectCommand) => invoke<ProjectResponse>("project_control", { command }),
  selectProject: (intent: ProjectPathIntent) =>
    invoke<SelectedProjectPath | null>("project_select_path", { intent }),
  debugSelectProject: (relativePath: string, intent: ProjectPathIntent) =>
    invoke<SelectedProjectPath>("project_debug_select_path", { relativePath, intent }),
  openProject: (
    selection: SelectedProjectPath,
    baseRevision: SceneRevision,
    readOnly: boolean,
    recoverPrevious: boolean,
    discardChanges: boolean,
  ) =>
    invoke<ProjectResponse>("project_open", {
      token: selection.token,
      sessionId: selection.session_id,
      baseRevision,
      readOnly,
      recoverPrevious,
      discardChanges,
    }),
  saveProject: (sessionId: SessionId, baseRevision: SceneRevision, token: string | null) =>
    invoke<ProjectResponse>("project_save", { sessionId, baseRevision, token }),
  reopenProject: (sessionId: SessionId, baseRevision: SceneRevision) =>
    invoke<ProjectResponse>("project_reopen", { sessionId, baseRevision }),
  importSource: (source: SelectedSource, baseRevision: SceneRevision, initialPose: RigidPoseMm) =>
    invoke<GeometryResponse>("geometry_import_source", {
      token: source.token,
      sessionId: source.session_id,
      baseRevision,
      initialPose,
    }),
  geometry: (command: GeometryCommand) => invoke<GeometryResponse>("geometry_control", { command }),
  chunk: (sessionId: SessionId, artifactId: ArtifactId, chunkIndex: number) =>
    invoke<ArrayBuffer>("geometry_chunk", { sessionId, artifactId, chunkIndex }),
};
