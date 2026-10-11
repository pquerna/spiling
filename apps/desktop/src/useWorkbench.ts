/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import {
  decodeMesh,
  decodeSection,
  validateMeshManifest,
  validateSectionManifest,
  JOB_POLL_INTERVAL_MS,
  MAX_SCENE_MESH_BYTES,
  MAX_SECTION_BYTES,
  type ArtifactId,
  type DefinitionRecord,
  type FaceIndexRow,
  type FaceInspection,
  type FaceRef,
  type GeometryCommand,
  type NativeOperationView,
  type OperationView,
  type ArtifactView,
  decodeTriangle,
  type GeometryResponse,
  type ProjectCommand,
  type ProjectResponse,
  type ProjectInfo,
  type SelectedProjectPath,
  type Hello,
  type MeshChunkMetadata,
  type OccurrenceRecord,
  type PlaneMm,
  type RigidPoseMm,
  type SceneSummary,
  type SectionChunkMetadata,
  type SectionLoopMetadata,
  type SectionSummary,
  type SelectedSource,
} from "@spiling/protocol";
import {
  createGeometryViewport,
  createDiagnosticViewport,
  type DiagnosticViewport,
  WebGPUUnavailableError,
  type GeometryViewport,
  type GeometryDisplayDefinition,
} from "@spiling/viewport";
import { native, type RuntimeInfo } from "./native.ts";

export type Phase =
  | "desktop-required"
  | "probing"
  | "starting"
  | "running"
  | "stopped"
  | "interrupted"
  | "unsupported"
  | "error";
export interface Transfer {
  bytes: number;
  requestMs: number;
  decodeMs: number;
  renderMs: number;
  totalMs: number;
}
export interface WorkbenchState {
  phase: Phase;
  busy: boolean;
  message: string;
  gpu: string | null;
  runtime: RuntimeInfo | null;
  hello: Hello | null;
  transfer: Transfer | null;
  scene: SceneSummary | null;
  definitions: GeometryDisplayDefinition[];
  occurrences: OccurrenceRecord[];
  project: ProjectInfo | null;
  activeJob: NativeOperationView | null;
  operation: OperationView | null;
  diagnosticTransfer: Transfer | null;
  transferring: "scene" | "section" | null;
  stale: boolean;
  inspection: FaceInspection | null;
  section: SectionSummary | null;
  bufferUsage: GeometryViewport["bufferUsage"];
}
class DomainError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(`${code}: ${message}`);
  }
}
class DisplayPaused extends Error {}
class ObsoleteOperation extends Error {}
const text = (error: unknown) => (error instanceof Error ? error.message : String(error));
function unexpectedJob(_value: never): never {
  throw new Error("unexpected shared job metadata");
}
const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));
export const identityPose: RigidPoseMm = { translation_mm: [0, 0, 0], rotation_xyzw: [0, 0, 0, 1] };
const initial: WorkbenchState = {
  phase: native.available() ? "probing" : "desktop-required",
  busy: false,
  message: "",
  gpu: null,
  runtime: null,
  hello: null,
  transfer: null,
  scene: null,
  definitions: [],
  occurrences: [],
  project: null,
  activeJob: null,
  operation: null,
  diagnosticTransfer: null,
  transferring: null,
  stale: false,
  inspection: null,
  section: null,
  bufferUsage: { rawBytes: 0, gpuBytes: 0, ownedBytes: 0 },
};

export function useWorkbench(
  canvas: RefObject<HTMLCanvasElement | null>,
  diagnosticCanvas: RefObject<HTMLCanvasElement | null>,
) {
  const [state, setState] = useState(initial);
  const stateRef = useRef(state);
  stateRef.current = state;
  const mounted = useRef(false),
    generation = useRef(0),
    inCommand = useRef(false);
  const workflow = useRef(false),
    pendingCancel = useRef(false),
    pauseDisplay = useRef(false);
  const activeJob = useRef<NativeOperationView | null>(null);
  const diagnosticOperation = useRef<string | null>(null);
  const diagnosticViewport = useRef<DiagnosticViewport | null>(null);
  const helloRef = useRef<Hello | null>(null),
    sceneRef = useRef<SceneSummary | null>(null);
  const viewport = useRef<GeometryViewport | null>(null);
  const viewportDisposal = useRef<Promise<void> | null>(null);
  const current = (token: number) => mounted.current && generation.current === token;
  const ensure = (token: number) => {
    if (!current(token)) throw new ObsoleteOperation();
  };
  const patch = (value: Partial<WorkbenchState>) => {
    if (mounted.current) setState((previous) => ({ ...previous, ...value }));
  };

  const disposeViewport = useCallback(() => {
    const previous = viewport.current;
    const diagnostic = diagnosticViewport.current;
    viewport.current = null;
    diagnosticViewport.current = null;
    diagnosticOperation.current = null;
    if (previous || diagnostic) {
      const pending = (viewportDisposal.current ?? Promise.resolve())
        .then(async () => {
          await previous?.dispose();
          await diagnostic?.dispose();
        })
        .finally(() => {
          if (viewportDisposal.current === pending) viewportDisposal.current = null;
          if (mounted.current && !viewport.current) {
            setState((state) => ({
              ...state,
              bufferUsage: { rawBytes: 0, gpuBytes: 0, ownedBytes: 0 },
            }));
          }
        });
      viewportDisposal.current = pending;
    }
    return viewportDisposal.current ?? Promise.resolve();
  }, []);

  // Only this function owns an invoke exchange. Waiting on a native job, hashing,
  // upload and frame yields never hold the pipe gate. No pipe read is aborted.
  async function exchange<T>(operation: () => Promise<T>, token: number): Promise<T> {
    while (inCommand.current) {
      await sleep(10);
      ensure(token);
    }
    ensure(token);
    inCommand.current = true;
    try {
      const result = await operation();
      ensure(token);
      return result;
    } finally {
      inCommand.current = false;
    }
  }
  async function control(command: GeometryCommand, token: number): Promise<GeometryResponse> {
    const response = await exchange(() => native.geometry(command), token);
    if (response.type === "error" || response.type === "project_error")
      throw new DomainError(response.error.code, response.error.message);
    return response;
  }
  async function getProject(token: number) {
    const response = await exchange(
      () =>
        native.project({
          op: "get",
          session_id: helloRef.current!.session_id,
        }),
      token,
    );
    if (response.type === "error")
      throw new DomainError(response.error.code, response.error.message);
    if (response.type !== "status") throw new Error("unexpected project status response");
    patch({ project: response.info });
    return response.info;
  }
  async function failure(error: unknown, token: number) {
    if (!current(token) || error instanceof ObsoleteOperation) return;
    if (error instanceof DisplayPaused) {
      patch({ message: error.message });
      return;
    }
    if (error instanceof DomainError) {
      patch({ message: text(error) });
      if (error.code !== "stale_revision") return;
      try {
        const latest = await getScene(token);
        if (latest.revision !== sceneRef.current?.revision) {
          sceneRef.current = latest;
          patch({ scene: latest, stale: true, inspection: null, section: null });
          await viewport.current?.beginScene(latest);
          await viewport.current?.discardScene();
        }
        return;
      } catch (refreshError) {
        if (!current(token)) return;
        error = refreshError;
      }
    }
    // Layout/hash admission failure retains the last valid display; transport
    // failures have already disconnected the shell client and are distinguished
    // through the actual supervisor, never by parsing exception strings.
    try {
      const status = await exchange(() => native.status(), token);
      if (status.state === "running") {
        patch({ message: `Synchronization failed: ${text(error)}` });
        return;
      }
    } catch {
      /* A failed supervision exchange is also fatal. */
    }
    if (!current(token)) return;
    ++generation.current;
    activeJob.current = null;
    helloRef.current = null;
    await disposeViewport();
    patch({
      phase: "interrupted",
      gpu: null,
      activeJob: null,
      transferring: null,
      message: text(error),
      stale: false,
    });
  }
  async function getScene(token: number) {
    const hello = helloRef.current;
    if (!hello) throw new ObsoleteOperation();
    const response = await control({ op: "get_scene", session_id: hello.session_id }, token);
    if (response.type !== "scene") throw new Error("unexpected scene response");
    return response.summary;
  }
  async function scenePages(summary: SceneSummary, token: number) {
    const definitions: DefinitionRecord[] = [],
      occurrences: OccurrenceRecord[] = [];
    for (const kind of ["definitions", "occurrences"] as const) {
      let offset: number | null = 0;
      while (offset !== null) {
        const response = await control(
          {
            op: "get_scene_page",
            session_id: summary.session_id,
            revision: summary.revision,
            kind,
            offset,
          },
          token,
        );
        if (response.type !== "scene_page") throw new Error("unexpected scene page");
        const rows = kind === "definitions" ? response.definitions : response.occurrences;
        if (rows.length > 64 || (response.next_offset !== null && response.next_offset <= offset))
          throw new Error("invalid scene page progression");
        definitions.push(...response.definitions);
        occurrences.push(...response.occurrences);
        if (
          definitions.length > summary.definition_count ||
          occurrences.length > summary.occurrence_count
        )
          throw new Error("scene page exceeds summary");
        offset = response.next_offset;
      }
    }
    if (
      definitions.length !== summary.definition_count ||
      occurrences.length !== summary.occurrence_count
    )
      throw new Error("incomplete scene pages");
    return { definitions, occurrences };
  }
  async function faces(definition: DefinitionRecord, session: string, token: number) {
    const rows: FaceIndexRow[] = [];
    let offset: number | null = 0;
    while (offset !== null) {
      const response = await control(
        {
          op: "get_face_index_page",
          session_id: session,
          definition_id: definition.definition_id,
          offset,
        },
        token,
      );
      if (response.type !== "face_index_page" || response.faces.length > 256)
        throw new Error("invalid face page");
      for (const row of response.faces) {
        if (row.ordinal !== rows.length) throw new Error("unordered face index");
        rows.push(row);
      }
      if (
        rows.length > definition.face_count ||
        (response.next_offset !== null && response.next_offset <= offset)
      )
        throw new Error("face page exceeds definition");
      offset = response.next_offset;
    }
    if (
      rows.length !== definition.face_count ||
      new Set(rows.map((row) => row.face_id)).size !== rows.length
    )
      throw new Error("incomplete face index");
    return rows;
  }
  async function artifactPages(
    session: string,
    artifact: ArtifactId,
    kind: "chunks" | "loops",
    token: number,
  ) {
    const chunks: Array<MeshChunkMetadata | SectionChunkMetadata> = [],
      loops: SectionLoopMetadata[] = [];
    const resources: ArtifactView[] = [];
    let offset: number | null = 0,
      summary: Extract<GeometryResponse, { type: "artifact_page" }>["summary"] | null = null;
    while (offset !== null) {
      const response = await control(
        { op: "get_artifact_page", session_id: session, artifact_id: artifact, kind, offset },
        token,
      );
      if (
        response.type !== "artifact_page" ||
        response.chunks.length > 64 ||
        response.loops.length > 64
      )
        throw new Error("invalid artifact page");
      if (summary && JSON.stringify(summary) !== JSON.stringify(response.summary))
        throw new Error("artifact changed during paging");
      summary = response.summary;
      const totals = summary.kind === "mesh" ? summary : summary.summary;
      const bytes = summary.kind === "mesh" ? summary.total_bytes : summary.summary.total_bytes;
      const loopsCap = summary.kind === "section" ? summary.summary.total_loop_count : 0;
      const byteCap = summary.kind === "mesh" ? MAX_SCENE_MESH_BYTES : MAX_SECTION_BYTES;
      if (
        !Number.isSafeInteger(bytes) ||
        bytes < 0 ||
        bytes > byteCap ||
        !Number.isSafeInteger(totals.chunk_count) ||
        totals.chunk_count < 0 ||
        totals.chunk_count > bytes / 64 ||
        !Number.isSafeInteger(loopsCap) ||
        loopsCap < 0 ||
        loopsCap > bytes / 96
      )
        throw new Error("artifact summary exceeds supported layout budgets");
      if (response.resources.length !== response.chunks.length)
        throw new Error("artifact resource count differs from packed descriptors");
      for (let index = 0; index < response.chunks.length; index++) {
        const metadata = response.chunks[index]!.metadata;
        const resource = response.resources[index]!;
        if (
          resource.size_bytes !== String(metadata.byte_count) ||
          resource.sha256 !== metadata.sha256
        )
          throw new Error("artifact resource size/hash differs from packed metadata");
        resources.push(resource);
      }
      for (const chunk of response.chunks) {
        if (chunk.kind !== summary.kind) throw new Error("artifact descriptor kind mismatch");
        chunks.push(chunk.metadata);
      }
      loops.push(...response.loops);
      if (
        chunks.length > totals.chunk_count ||
        loops.length > loopsCap ||
        (response.next_offset !== null && response.next_offset <= offset)
      )
        throw new Error("invalid artifact page progression");
      offset = response.next_offset;
    }
    if (!summary) throw new Error("missing artifact summary");
    return { summary, chunks, loops, resources };
  }
  async function release(session: string, artifact: ArtifactId, token: number) {
    if (current(token))
      await control({ op: "release_artifact", session_id: session, artifact_id: artifact }, token);
  }
  function checkDisplay(token: number) {
    ensure(token);
    if (pauseDisplay.current)
      throw new DisplayPaused(
        "Display transfer paused. Resume the current scene without reimporting.",
      );
  }
  async function syncScene(summary: SceneSummary, token: number) {
    const view = viewport.current;
    if (!view) throw new ObsoleteOperation();
    sceneRef.current = summary;
    patch({ scene: summary, stale: true, inspection: null, section: null, transferring: "scene" });
    await view.beginScene(summary);
    ensure(token);
    const metrics: Transfer = { bytes: 0, requestMs: 0, decodeMs: 0, renderMs: 0, totalMs: 0 },
      started = performance.now();
    try {
      const pages = await scenePages(summary, token),
        definitions: GeometryDisplayDefinition[] = [];
      for (const definition of pages.definitions) {
        checkDisplay(token);
        const faceRows = await faces(definition, summary.session_id, token);
        definitions.push({ ...definition, faces: faceRows });
        if (view.hasDefinition(definition.definition_id, definition.mesh_artifact_id)) continue;
        try {
          const page = await artifactPages(
            summary.session_id,
            definition.mesh_artifact_id,
            "chunks",
            token,
          );
          if (
            page.summary.kind !== "mesh" ||
            page.summary.definition_id !== definition.definition_id ||
            page.summary.artifact_id !== definition.mesh_artifact_id
          )
            throw new Error("invalid definition artifact");
          const descriptors: MeshChunkMetadata[] = page.chunks.map((row) => {
            if (!("definition_id" in row)) throw new Error("section descriptor in mesh artifact");
            return row;
          });
          validateMeshManifest(descriptors, {
            session_id: summary.session_id,
            artifact_id: definition.mesh_artifact_id,
            definition_id: definition.definition_id,
          });
          if (
            descriptors.length !== page.summary.chunk_count ||
            descriptors.reduce((sum, row) => sum + row.byte_count, 0) !== page.summary.total_bytes
          )
            throw new Error("incomplete mesh artifact");
          for (const metadata of descriptors) {
            checkDisplay(token);
            const began = performance.now();
            const bytes = await exchange(
              () =>
                native.chunk(
                  page.resources[metadata.chunk_index]!,
                  { kind: "mesh", metadata },
                  page.summary,
                ),
              token,
            );
            const received = performance.now();
            checkDisplay(token);
            const mesh = await decodeMesh(bytes, metadata, {
              session_id: summary.session_id,
              artifact_id: definition.mesh_artifact_id,
              definition_id: definition.definition_id,
              chunk_index: metadata.chunk_index,
            });
            const decoded = performance.now();
            checkDisplay(token);
            await view.addDefinitionChunk(metadata, mesh);
            ensure(token);
            const rendered = performance.now();
            metrics.bytes += bytes.byteLength;
            metrics.requestMs += received - began;
            metrics.decodeMs += decoded - received;
            metrics.renderMs += rendered - decoded;
            patch({
              transfer: { ...metrics, totalMs: rendered - started },
              bufferUsage: view.bufferUsage,
            });
            await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
          }
        } finally {
          await release(summary.session_id, definition.mesh_artifact_id, token);
        }
      }
      checkDisplay(token);
      const latest = await getScene(token);
      if (latest.revision !== summary.revision)
        throw new DomainError(
          "stale_revision",
          "scene changed during display admission; resume its current revision",
        );
      checkDisplay(token);
      await view.commitScene(definitions, pages.occurrences);
      ensure(token);
      metrics.totalMs = performance.now() - started;
      patch({
        scene: summary,
        definitions,
        occurrences: pages.occurrences,
        stale: false,
        transfer: metrics,
        bufferUsage: view.bufferUsage,
      });
    } catch (error) {
      await view.discardScene();
      if (current(token)) patch({ bufferUsage: view.bufferUsage });
      throw error;
    } finally {
      if (current(token)) patch({ transferring: null });
    }
  }
  async function showSection(artifact: ArtifactId, token: number) {
    const summary = sceneRef.current,
      view = viewport.current;
    if (!summary || !view) throw new ObsoleteOperation();
    patch({ transferring: "section" });
    try {
      const page = await artifactPages(summary.session_id, artifact, "chunks", token);
      const loopPage = await artifactPages(summary.session_id, artifact, "loops", token);
      if (
        page.summary.kind !== "section" ||
        loopPage.summary.kind !== "section" ||
        JSON.stringify(page.summary) !== JSON.stringify(loopPage.summary)
      )
        throw new Error("invalid section artifact");
      const section = page.summary.summary,
        chunks: SectionChunkMetadata[] = page.chunks.map((row) => {
          if ("definition_id" in row) throw new Error("mesh descriptor in section artifact");
          return row;
        });
      validateSectionManifest(section, chunks, loopPage.loops, {
        session_id: summary.session_id,
        artifact_id: artifact,
        revision: summary.revision,
      });
      if (chunks.length === 0) {
        checkDisplay(token);
        await view.showSection(section, null);
      }
      for (const metadata of chunks) {
        checkDisplay(token);
        const bytes = await exchange(
          () =>
            native.chunk(
              page.resources[metadata.chunk_index]!,
              { kind: "section", metadata },
              page.summary,
            ),
          token,
        );
        checkDisplay(token);
        const rows = loopPage.loops.slice(
          metadata.first_loop_ordinal,
          metadata.first_loop_ordinal + metadata.loop_count,
        );
        const decoded = await decodeSection(bytes, metadata, section, rows, {
          session_id: summary.session_id,
          artifact_id: artifact,
          revision: summary.revision,
          chunk_index: metadata.chunk_index,
        });
        checkDisplay(token);
        await view.showSection(section, decoded);
        ensure(token);
        if (metadata.chunk_index + 1 < metadata.chunk_count) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        }
      }
      ensure(token);
      patch({
        section,
        bufferUsage: view.bufferUsage,
        message: "Native section preview ready; display lines are approximate.",
      });
    } catch (error) {
      await view.discardSection();
      if (error instanceof DisplayPaused)
        throw new DisplayPaused(
          "Section transfer cancelled; previous overlay and interactive scene retained.",
        );
      throw error;
    } finally {
      await release(summary.session_id, artifact, token);
      if (current(token)) patch({ transferring: null });
    }
  }
  async function waitJob(response: GeometryResponse | ProjectResponse, token: number) {
    if (response.type === "error" || response.type === "project_error")
      throw new DomainError(response.error.code, response.error.message);
    if (response.type !== "operation_accepted")
      throw new Error("expected accepted native operation");
    const session = helloRef.current!.session_id;
    activeJob.current = response.operation;
    patch({ activeJob: activeJob.current });
    try {
      for (;;) {
        await sleep(JOB_POLL_INTERVAL_MS);
        ensure(token);
        const job = await exchange(() => native.nativeOperation(response.operation.name), token);
        if (job.session_id !== session)
          throw new DomainError("stale_revision", "operation belongs to an old session");
        activeJob.current = job;
        patch({ activeJob: job });
        if (job.status === "completed") {
          if (!job.result) throw new Error("completed operation has no result");
          const result = job.result;
          switch (result.kind) {
            case "scene":
              await getProject(token);
              return result;
            case "section":
            case "project_saved":
              return result;
            case "manufacturing_compiled":
              patch({ project: result.info });
              throw new DomainError(
                "unsupported_capability",
                "Manufacturing job results are not exposed by the desktop workbench.",
              );
            case "manufacturing_verified":
              throw new DomainError(
                "unsupported_capability",
                "Manufacturing job results are not exposed by the desktop workbench.",
              );
            default:
              return unexpectedJob(result);
          }
        }
        if (job.status === "failed" || job.status === "cancelled" || job.status === "interrupted") {
          const error = job.error;
          if (!error) {
            await getProject(token);
            throw new DomainError(
              job.status === "interrupted" ? "interrupted" : "cancelled",
              "Operation ended without replacing the prior project.",
            );
          }
          switch (error.domain) {
            case "project":
              await getProject(token);
              throw new DomainError(error.error.code, error.error.message);
            case "geometry":
            case "manufacturing":
              throw new DomainError(error.error.code, error.error.message);
            default:
              return unexpectedJob(error);
          }
        }
        switch (job.status) {
          case "queued":
          case "running":
          case "cancelling":
            break;
          default:
            unexpectedJob(job.status);
        }
      }
    } finally {
      if (current(token)) {
        activeJob.current = null;
        pendingCancel.current = false;
        patch({ activeJob: null });
      }
    }
  }
  async function run(operation: (token: number) => Promise<void>) {
    if (workflow.current || !helloRef.current || stateRef.current.phase !== "running") return;
    workflow.current = true;
    pauseDisplay.current = false;
    const token = generation.current;
    patch({ busy: true, message: "" });
    try {
      await operation(token);
    } catch (error) {
      await failure(error, token);
    } finally {
      workflow.current = false;
      if (current(token)) patch({ busy: false, transferring: null });
    }
  }
  const runDiagnostic = () =>
    run(async (token) => {
      if (!diagnosticCanvas.current) throw new Error("Diagnostic canvas is missing");
      if (!diagnosticViewport.current) {
        const next = await createDiagnosticViewport(diagnosticCanvas.current, (message) => {
          if (!current(token)) return;
          patch({ message, phase: "unsupported", gpu: null });
          ++generation.current;
          void native
            .stop()
            .finally(() => disposeViewport())
            .catch(() => undefined);
        });
        if (!current(token)) {
          await next.dispose();
          throw new ObsoleteOperation();
        }
        diagnosticViewport.current = next;
      }
      const view = diagnosticViewport.current;
      const began = performance.now();
      let operation = await exchange(() => native.runDiagnostic(crypto.randomUUID()), token);
      diagnosticOperation.current = operation.name;
      let displayed = 0;
      try {
        for (;;) {
          ensure(token);
          patch({ operation });
          if (operation.outputs.length > displayed) {
            const artifact = operation.outputs[operation.outputs.length - 1]!;
            const binary = await exchange(() => native.readArtifact(artifact.name), token);
            const received = performance.now();
            const triangle = decodeTriangle(binary);
            const decoded = performance.now();
            await view.showTriangle(triangle);
            ensure(token);
            const rendered = performance.now();
            displayed = operation.outputs.length;
            patch({
              diagnosticTransfer: {
                bytes: triangle.byteLength,
                requestMs: received - began,
                decodeMs: decoded - received,
                renderMs: rendered - decoded,
                totalMs: rendered - began,
              },
            });
          }
          if (operation.done) {
            if (operation.error_code !== null && operation.state !== "cancelled")
              throw new DomainError(
                String(operation.error_code),
                operation.error_message ?? "Diagnostic failed",
              );
            patch({
              message:
                operation.state === "cancelled"
                  ? "Diagnostic cancelled. Completed partial output remains inspectable."
                  : "Synthetic diagnostic complete; not authoritative geometry.",
            });
            return;
          }
          await sleep(100);
          operation = await exchange(() => native.getOperation(operation.name), token);
        }
      } finally {
        if (current(token)) diagnosticOperation.current = null;
      }
    });
  const cancelDiagnostic = () => {
    const name = diagnosticOperation.current,
      token = generation.current;
    if (name) void native.cancelOperation(name).catch((error: unknown) => failure(error, token));
  };

  async function applyProjectResponse(response: ProjectResponse, token: number) {
    if (response.type === "error") {
      await getProject(token);
      throw new DomainError(response.error.code, response.error.message);
    }
    if (response.type === "status") {
      // Intent/artifact-only undo/redo does not change geometry or display state.
      patch({ project: response.info });
      return;
    }
    if (response.type === "scene_changed") {
      patch({ project: response.info });
      await syncScene(response.summary, token);
    } else if (response.type === "operation_accepted") {
      const result = await waitJob(response, token);
      switch (result.kind) {
        case "project_saved":
          // Checkpoint only: no scene invalidation, mesh transfer or history reset.
          patch({
            project: result.info,
            message: "Project checkpoint saved; session undo is retained.",
          });
          break;
        case "scene":
          await syncScene(result.summary, token);
          break;
        case "section":
          throw new Error("unexpected section result for a project job");
        default:
          unexpectedJob(result);
      }
    } else throw new Error("unexpected project response");
  }
  const projectControl = (op: "new" | "undo" | "redo", discardChanges = false) =>
    run(async (token) => {
      const summary = sceneRef.current!;
      const command: ProjectCommand =
        op === "new"
          ? {
              op,
              session_id: summary.session_id,
              base_revision: summary.revision,
              discard_changes: discardChanges,
            }
          : { op, session_id: summary.session_id, base_revision: summary.revision };
      await applyProjectResponse(await exchange(() => native.project(command), token), token);
    });
  const openProject = (
    readOnly: boolean,
    recoverPrevious: boolean,
    discardChanges: boolean,
    relativePath?: string,
  ) =>
    run(async (token) => {
      const selection = await exchange(
        () =>
          relativePath === undefined
            ? native.selectProject("open")
            : native.debugSelectProject(relativePath, "open"),
        token,
      );
      if (!selection) return;
      await applyProjectResponse(
        await exchange(
          () =>
            native.openProject(
              selection,
              sceneRef.current!.revision,
              readOnly,
              recoverPrevious,
              discardChanges,
            ),
          token,
        ),
        token,
      );
    });
  const saveProject = (relativePath?: string) =>
    run(async (token) => {
      if (stateRef.current.project?.read_only)
        throw new DomainError("read_only", "Read-only projects cannot be saved.");
      let selection: SelectedProjectPath | null = null;
      if (stateRef.current.project?.path_label === null || relativePath !== undefined) {
        selection = await exchange(
          () =>
            relativePath === undefined
              ? native.selectProject("save")
              : native.debugSelectProject(relativePath, "save"),
          token,
        );
        if (!selection) return;
      }
      const summary = sceneRef.current!;
      await applyProjectResponse(
        await exchange(
          () => native.saveProject(summary.session_id, summary.revision, selection?.token ?? null),
          token,
        ),
        token,
      );
    });
  async function importSources(sources: SelectedSource[], token: number) {
    if (stateRef.current.project?.read_only)
      throw new DomainError("read_only", "Read-only projects cannot import sources.");
    for (const source of sources) {
      ensure(token);
      const summary = sceneRef.current!;
      patch({ message: `Importing ${source.label}` });
      try {
        const accepted = await exchange(
          () => native.importSource(source, summary.revision, identityPose),
          token,
        );
        const result = await waitJob(accepted, token);
        if (result.kind !== "scene") throw new Error("import did not return a scene");
        await syncScene(result.summary, token);
      } catch (error) {
        if (error instanceof DomainError)
          throw new DomainError(error.code, `${source.label}: ${error.message}`);
        throw error;
      }
    }
  }
  const importParts = () =>
    run(async (token) => importSources(await exchange(() => native.selectSources(), token), token));
  const debugImport = (relativePaths: string[]) =>
    run(async (token) =>
      importSources(await exchange(() => native.debugSelectSources(relativePaths), token), token),
    );
  const resume = () =>
    run(async (token) => {
      for (;;) {
        try {
          await syncScene(await getScene(token), token);
          return;
        } catch (error) {
          if (error instanceof DomainError && error.code === "stale_revision") {
            checkDisplay(token);
            continue;
          }
          throw error;
        }
      }
    });
  const mutate = (
    command: "add" | "remove" | "pose",
    occurrence: OccurrenceRecord,
    pose = occurrence.pose,
  ) =>
    run(async (token) => {
      if (stateRef.current.project?.read_only)
        throw new DomainError("read_only", "Read-only projects cannot edit instances.");
      if (stateRef.current.stale)
        throw new DomainError("stale_revision", "Resume display before editing instances.");
      const summary = sceneRef.current!;
      const shared = { session_id: summary.session_id, base_revision: summary.revision };
      const request: GeometryCommand =
        command === "add"
          ? { op: "add_instance", ...shared, definition_id: occurrence.definition_id, pose }
          : command === "remove"
            ? { op: "remove_instance", ...shared, occurrence_id: occurrence.occurrence_id }
            : { op: "set_instance_pose", ...shared, occurrence_id: occurrence.occurrence_id, pose };
      const response = await control(request, token);
      if (response.type !== "scene_changed")
        throw new Error("unexpected instance mutation response");
      const next = response.summary;
      await getProject(token);
      sceneRef.current = next;
      patch({ scene: next, stale: true, inspection: null, section: null });
      const view = viewport.current!;
      try {
        await view.beginScene(next);
        const pages = await scenePages(next, token);
        // No geometry bytes are requested for pure occurrence mutations.
        const definitions = stateRef.current.definitions.filter((row) =>
          pages.definitions.some((definition) => definition.definition_id === row.definition_id),
        );
        await view.updateOccurrences(next, pages.occurrences);
        ensure(token);
        patch({
          scene: next,
          definitions,
          occurrences: pages.occurrences,
          stale: false,
          transfer: { bytes: 0, requestMs: 0, decodeMs: 0, renderMs: 0, totalMs: 0 },
          bufferUsage: view.bufferUsage,
        });
      } catch (error) {
        await view.discardScene();
        throw error;
      }
    });
  const section = (plane: PlaneMm) =>
    run(async (token) => {
      if (stateRef.current.stale)
        throw new DomainError("stale_revision", "Resume display before sectioning.");
      const summary = sceneRef.current!;
      const result = await waitJob(
        await control(
          {
            op: "start_section",
            session_id: summary.session_id,
            base_revision: summary.revision,
            plane,
          },
          token,
        ),
        token,
      );
      if (result.kind !== "section") throw new Error("section job did not return an artifact");
      await showSection(result.artifact_id, token);
    });
  const cancel = () => {
    if (activeJob.current && !pendingCancel.current) {
      const name = activeJob.current.name,
        token = generation.current;
      pendingCancel.current = true;
      // Standard cancellation is an independent RPC, not queued behind a ByteStream download.
      void native
        .cancelOperation(name)
        .catch((error: unknown) => failure(error, token))
        .finally(() => {
          if (current(token)) pendingCancel.current = false;
        });
    } else if (stateRef.current.transferring) pauseDisplay.current = true;
  };
  const inspect = (reference: FaceRef | null) =>
    run(async (token) => {
      if (stateRef.current.stale) return;
      await viewport.current?.setSelection(reference);
      ensure(token);
      if (!reference) {
        patch({ inspection: null });
        return;
      }
      const response = await control({ op: "inspect_face", reference }, token);
      if (response.type !== "face_inspection")
        throw new Error("unexpected face inspection response");
      patch({ inspection: response.inspection });
    });
  const pick = (x: number, y: number) => inspect(viewport.current?.pick(x, y) ?? null);
  const setView = (view: "front" | "isometric") =>
    viewport.current?.setView(view).catch((error: unknown) => failure(error, generation.current));
  const fit = () =>
    viewport.current?.fitToScene().catch((error: unknown) => failure(error, generation.current));

  const execute = useCallback(
    async (action: "start" | "restart" | "stop" | "interrupt", discardChanges = false) => {
      if (!native.available() || !mounted.current) return;
      if (stateRef.current.project?.dirty && !discardChanges) {
        patch({
          message:
            "Unsaved changes: explicitly confirm discard before ending or restarting the session.",
        });
        return;
      }
      const reopenSaved = stateRef.current.project?.path_label != null;
      const token = ++generation.current;
      activeJob.current = null;
      pendingCancel.current = false;
      pauseDisplay.current = true;
      helloRef.current = null;
      sceneRef.current = null;
      patch({
        busy: true,
        activeJob: null,
        operation: null,
        diagnosticTransfer: null,
        transferring: null,
        inspection: null,
        section: null,
      });
      try {
        if (action === "stop" || action === "interrupt") {
          await exchange(() => (action === "stop" ? native.stop() : native.interrupt()), token);
          await disposeViewport();
          ensure(token);
          patch({
            phase: action === "stop" ? "stopped" : "interrupted",
            gpu: null,
            scene: null,
            definitions: [],
            occurrences: [],
            stale: false,
            bufferUsage: { rawBytes: 0, gpuBytes: 0, ownedBytes: 0 },
            message:
              "Session ended. Start explicitly reopens the attached checkpoint; unsaved edits are never replayed.",
          });
          return;
        }
        await disposeViewport();
        ensure(token);
        patch({
          phase: "probing",
          gpu: null,
          hello: null,
          scene: null,
          definitions: [],
          occurrences: [],
          transfer: null,
          stale: false,
          message: "",
        });
        if (!canvas.current) throw new Error("Viewport canvas is missing");
        let next: GeometryViewport | undefined;
        next = await createGeometryViewport(canvas.current, (message) => {
          if (!current(token) || (next && viewport.current !== next)) return;
          const lostToken = ++generation.current;
          activeJob.current = null;
          helloRef.current = null;
          patch({ phase: "unsupported", gpu: null, activeJob: null, transferring: null, message });
          void (async () => {
            try {
              await exchange(() => native.stop(), lostToken);
            } catch (error) {
              patch({ message: `${message}; cleanup failed: ${text(error)}` });
            } finally {
              await disposeViewport();
            }
          })();
        });
        if (!current(token)) {
          await next.dispose();
          return;
        }
        viewport.current = next;
        patch({ gpu: next.adapter, phase: "starting" });
        const runtime = await exchange(() => native.runtime(), token);
        const hello = await exchange(
          () => (action === "restart" ? native.restart(discardChanges) : native.start()),
          token,
        );
        helloRef.current = hello;
        patch({ hello, runtime });
        pauseDisplay.current = false;
        const summary = await getScene(token);
        if (reopenSaved) {
          await applyProjectResponse(
            await exchange(() => native.reopenProject(hello.session_id, summary.revision), token),
            token,
          );
        } else {
          await getProject(token);
          await syncScene(summary, token);
        }
        patch({
          phase: "running",
          message: reopenSaved
            ? "Attached project checkpoint reopened. Unsaved edits and prior session undo were not replayed."
            : "New unattached project. Import STEP parts, then save to a new project directory.",
        });
      } catch (error) {
        if (!current(token)) return;
        let message = text(error);
        try {
          await exchange(() => native.stop(), token);
        } catch (cleanup) {
          message += `; engine cleanup: ${text(cleanup)}`;
        }
        await disposeViewport();
        ensure(token);
        patch({
          phase: error instanceof WebGPUUnavailableError ? "unsupported" : "error",
          gpu: null,
          message,
        });
      } finally {
        if (current(token)) patch({ busy: false });
      }
    },
    [canvas, disposeViewport],
  );

  useEffect(() => {
    mounted.current = true;
    void execute("start");
    return () => {
      mounted.current = false;
      ++generation.current;
      pauseDisplay.current = true;
      // Native mutex waits for any complete exchange before stopping; no future
      // is aborted. GPU teardown is retained and awaited before any replacement.
      void (async () => {
        try {
          if (native.available()) await native.stop();
        } finally {
          await disposeViewport();
        }
      })().catch(() => undefined);
    };
  }, [execute, disposeViewport]);
  useEffect(() => {
    if (state.phase !== "running") return;
    let active = true;
    const timer = window.setInterval(() => {
      if (!active || inCommand.current || pendingCancel.current) return;
      const token = generation.current;
      void exchange(() => native.status(), token)
        .then(async (status) => {
          if (!active || !current(token) || status.state === "running") return;
          ++generation.current;
          activeJob.current = null;
          helloRef.current = null;
          await disposeViewport();
          patch({
            phase: status.state,
            gpu: null,
            activeJob: null,
            transferring: null,
            message:
              status.message ??
              "Engine session ended; restart explicitly reopens the attached checkpoint.",
          });
        })
        .catch((error: unknown) => failure(error, token));
    }, 750);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [state.phase, disposeViewport]);

  return {
    state,
    execute,
    runDiagnostic,
    cancelDiagnostic,
    importParts,
    debugImport,
    projectControl,
    openProject,
    saveProject,
    resume,
    mutate,
    section,
    cancel,
    pick,
    inspect,
    setView,
    fit,
  };
}
