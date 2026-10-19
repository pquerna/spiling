/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
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
  NativeOperationView,
  OperationView,
  ArtifactView,
  ArtifactChunkMetadata,
  ArtifactSummary,
  ManufacturingCommand,
  ManufacturingResponse,
  ManufacturingIntent,
  ManufacturingArtifactRecord,
  ReplayDisplayDescriptor,
  SparseBandVeto,
  ProjectRevision,
  FaceRef,
  SurfaceScopeKind,
  ImageReliefParameters,
  SurfaceImageRecord,
} from "@spiling/protocol";

export interface RuntimeInfo {
  app_build: string;
  runtime: string;
  cef_api_version: number;
  geometry_debug_enabled: boolean;
  manufacturing_diagnostic_enabled: boolean;
}
export interface EngineStatus {
  state: "stopped" | "running" | "interrupted";
  message: string | null;
  hello: Hello | null;
}

export interface SelectedManufacturingExport {
  token: string;
  session_id: SessionId;
  label: string;
}
export interface ManufacturingExportReceipt {
  label: string;
  files: string[];
}
export interface SelectedManufacturingIntent {
  token: string;
  session_id: SessionId;
  label: string;
}
export interface ManufacturingLayer {
  index: number;
  z_mm: number;
  lower_z_mm: number;
  height_mm: number;
  section_z_mm: number;
  deposition_segments: number;
  sparse_band: {
    first_layer: number;
    combined_regions: number[];
    combined_global: boolean;
    retained_fine: [SparseBandVeto, number][];
  } | null;
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
  selectSurfaceImage: () => invoke<SelectedSource | null>("surface_image_select"),
  debugSelectSurfaceImage: (relativePath: string) =>
    invoke<SelectedSource>("surface_image_debug_select", { relativePath }),
  importSurfaceImage: (
    selection: SelectedSource,
    baseRevision: ProjectRevision,
    reference: FaceRef,
    scope: SurfaceScopeKind,
    parameters: ImageReliefParameters,
  ) =>
    invoke<ProjectResponse>("surface_image_import", {
      token: selection.token,
      sessionId: selection.session_id,
      baseRevision,
      reference,
      scope,
      parameters,
    }),
  surfaceImageBytes: (record: SurfaceImageRecord, resource: ArtifactView) =>
    invoke<ArrayBuffer>("surface_image_bytes", { record, resource }),
  importSource: (source: SelectedSource, baseRevision: SceneRevision, initialPose: RigidPoseMm) =>
    invoke<GeometryResponse>("geometry_import_source", {
      token: source.token,
      sessionId: source.session_id,
      baseRevision,
      initialPose,
    }),
  geometry: (command: GeometryCommand) => invoke<GeometryResponse>("geometry_control", { command }),
  nativeOperation: (name: string) =>
    invoke<NativeOperationView>("engine_native_operation", { name }),
  cancelOperation: (name: string) => invoke<void>("engine_cancel_operation", { name }),
  runDiagnostic: (requestId: string) =>
    invoke<OperationView>("engine_run_diagnostic", { requestId }),
  getOperation: (name: string) => invoke<OperationView>("engine_get_operation", { name }),
  readArtifact: (name: string) => invoke<ArrayBuffer>("engine_read_artifact", { name }),
  chunk: (resource: ArtifactView, expected: ArtifactChunkMetadata, summary: ArtifactSummary) =>
    invoke<ArrayBuffer>("geometry_chunk", { resource, expected, summary }),
  manufacturing: (command: ManufacturingCommand) =>
    invoke<ManufacturingResponse>("manufacturing_control", { command }),
  selectManufacturingIntent: () =>
    invoke<SelectedManufacturingIntent | null>("manufacturing_select_intent"),
  debugSelectManufacturingIntent: (relativePath: string) =>
    invoke<SelectedManufacturingIntent>("manufacturing_debug_select_intent", { relativePath }),
  loadManufacturingIntent: (selection: SelectedManufacturingIntent) =>
    invoke<ManufacturingIntent>("manufacturing_load_intent", {
      token: selection.token, sessionId: selection.session_id,
    }),
  manufacturingLayers: (record: ManufacturingArtifactRecord) =>
    invoke<ManufacturingLayer[]>("manufacturing_layers", { record }),
  prepareManufacturingBed: (sessionId: SessionId, baseRevision: SceneRevision, occurrenceId: number, center: boolean) =>
    invoke<GeometryResponse>("manufacturing_prepare_bed", { sessionId, baseRevision, occurrenceId, center }),
  replay: (descriptor: ReplayDisplayDescriptor, record: ManufacturingArtifactRecord) =>
    invoke<ArrayBuffer>("manufacturing_replay", { descriptor, record }),
  selectManufacturingExport: () =>
    invoke<SelectedManufacturingExport | null>("manufacturing_select_export"),
  debugSelectManufacturingExport: (relativePath: string) =>
    invoke<SelectedManufacturingExport>("manufacturing_debug_select_export", { relativePath }),
  exportManufacturing: (
    selection: SelectedManufacturingExport,
    record: ManufacturingArtifactRecord,
  ) =>
    invoke<ManufacturingExportReceipt>("manufacturing_export", {
      token: selection.token,
      sessionId: selection.session_id,
      record,
    }),
  attachManufacturingDiagnostic: () => invoke<Hello>("manufacturing_diagnostic_attach"),
  triangle: () => invoke<ArrayBuffer>("engine_triangle"),
};
