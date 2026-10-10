/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  AmbientLight,
  BufferAttribute,
  BufferGeometry,
  Color,
  DirectionalLight,
  DoubleSide,
  Group,
  InstancedMesh,
  LineBasicMaterial,
  LineSegments,
  Matrix4,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  OrthographicCamera,
  Quaternion,
  Raycaster,
  RenderTarget,
  Scene,
  Vector2,
  Vector3,
  WebGPURenderer,
} from "three/webgpu";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import {
  MAX_DEFINITIONS,
  MAX_DISPLAY_BUFFER_BYTES,
  MAX_OCCURRENCES,
  MAX_NATIVE_FACES,
  MAX_DISPLAY_VERTICES,
  MAX_DISPLAY_TRIANGLES,
  MAX_GEOMETRY_CHUNK_BYTES,
  MAX_SCENE_MESH_BYTES,
  POSE_NORM_TOLERANCE,
  validateSectionSummary,
} from "@spiling/protocol";
import type {
  ArtifactId,
  DefinitionId,
  DefinitionRecord,
  DisplayMesh,
  DisplaySection,
  FaceIndexRow,
  FaceRef,
  MeshChunkMetadata,
  OccurrenceRecord,
  SceneSummary,
  SectionSummary,
} from "@spiling/protocol";

/** Associates the engine's paged source-face table with its definition record. */
export type GeometryDisplayDefinition = DefinitionRecord & {
  readonly faces: readonly FaceIndexRow[];
};
export interface GeometryViewport {
  readonly adapter: string;
  readonly bufferUsage: { rawBytes: number; gpuBytes: number; ownedBytes: number };
  hasDefinition(definition: DefinitionId, artifact: ArtifactId): boolean;
  beginScene(summary: SceneSummary): Promise<void>;
  addDefinitionChunk(metadata: MeshChunkMetadata, mesh: DisplayMesh): Promise<void>;
  commitScene(
    definitions: readonly GeometryDisplayDefinition[],
    occurrences: readonly OccurrenceRecord[],
  ): Promise<void>;
  updateOccurrences(summary: SceneSummary, occurrences: readonly OccurrenceRecord[]): Promise<void>;
  discardScene(): Promise<void>;
  pick(ndcX: number, ndcY: number): FaceRef | null;
  setSelection(reference: FaceRef | null): Promise<void>;
  showSection(summary: SectionSummary, section: DisplaySection | null): Promise<void>;
  discardSection(): Promise<void>;
  fitToScene(): Promise<void>;
  setView(view: "front" | "isometric"): Promise<void>;
  dispose(): Promise<void>;
}
export class WebGPUUnavailableError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WebGPUUnavailableError";
  }
}
export class GeometrySynchronizationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "GeometrySynchronizationError";
  }
}
function reject(message: string): never {
  throw new GeometrySynchronizationError(message);
}
interface Chunk {
  metadata: MeshChunkMetadata;
  mesh: DisplayMesh;
  geometry: BufferGeometry;
  highlightGeometry: BufferGeometry;
  rawBytes: number;
  gpuBytes: number;
}
interface Definition {
  record: GeometryDisplayDefinition;
  chunks: Chunk[];
}
interface Batch {
  mesh: InstancedMesh;
  chunk: Chunk;
  definition: Definition;
  occurrences: readonly OccurrenceRecord[];
}
interface Display {
  summary: SceneSummary;
  definitions: Map<DefinitionId, Definition>;
  occurrences: readonly OccurrenceRecord[];
  root: Group;
  batches: Batch[];
  origin: Vector3;
  matrixBytes: number;
}
interface SectionDisplay {
  summary: SectionSummary;
  root: Group;
  chunks: DisplaySection[];
  rawBytes: number;
  gpuBytes: number;
  byteCount: number;
  loopCount: number;
}

/** A real WebGPU consumer. Native geometry and f64 identity stay outside Three. */
export async function createGeometryViewport(
  canvas: HTMLCanvasElement,
  onFailure: (message: string) => void,
): Promise<GeometryViewport> {
  if (!navigator.gpu)
    throw new WebGPUUnavailableError("WebGPU is unavailable. No WebGL fallback is used.");
  const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
  if (!adapter)
    throw new WebGPUUnavailableError(
      "No WebGPU adapter is available. Check the driver and CEF configuration.",
    );
  let device: GPUDevice;
  try {
    device = await adapter.requestDevice();
  } catch (error) {
    throw new WebGPUUnavailableError(`WebGPU device request failed: ${String(error)}`);
  }
  const renderer = new WebGPURenderer({
    canvas,
    device,
    forceWebGL: false,
    antialias: true,
    alpha: false,
  });
  // The pinned Three renderer otherwise silently tries WebGL on init failure.
  Object.defineProperty(renderer, "_getFallback", { value: null, writable: false });
  const scene = new Scene();
  scene.background = new Color("#101b22");
  scene.add(new AmbientLight(0xffffff, 2));
  const light = new DirectionalLight(0xffffff, 3);
  light.position.set(1, -2, 3);
  scene.add(light);
  const camera = new OrthographicCamera(-1, 1, 1, -1, 0.01, 100);
  camera.up.set(0, 0, 1);
  camera.position.set(3, -4, 3);
  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = false;
  controls.target.set(0, 0, 0);
  controls.update();
  const material = new MeshStandardMaterial({
    color: "#d3ae72",
    roughness: 0.8,
    metalness: 0,
    side: DoubleSide,
  });
  const selectionMaterial = new MeshBasicMaterial({
    color: "#55d9ff",
    side: DoubleSide,
    depthWrite: false,
    polygonOffset: true,
    polygonOffsetFactor: -2,
    polygonOffsetUnits: -2,
  });
  const lineMaterial = new LineBasicMaterial({ color: "#7ee9ce", depthTest: false });
  // One tiny offscreen target permits bounded staged uploads without publishing them.
  const uploadTarget = new RenderTarget(1, 1);
  const raycaster = new Raycaster();
  let active: Display | null = null;
  let stagedSummary: SceneSummary | null = null;
  const stagedChunks = new Map<DefinitionId, Chunk[]>();
  let candidate: Display | null = null;
  let overlay: SectionDisplay | null = null;
  let stagedSection: SectionDisplay | null = null;
  const highlights = new Group();
  scene.add(highlights);
  let synchronized = true;
  let disposed = false;
  let failed = false;
  let observer: ResizeObserver | null = null;
  let operation: Promise<void> | null = null;
  let rendering: Promise<void> | null = null;
  let redrawPending = false;
  let disposal: Promise<void> | null = null;
  let viewHeight = 2;
  let aspect = 1;

  function available() {
    if (disposed || failed) reject("The WebGPU viewport is no longer available.");
  }
  function usage() {
    const unique = new Set<Chunk>();
    active?.definitions.forEach((d) => d.chunks.forEach((c) => unique.add(c)));
    candidate?.definitions.forEach((d) => d.chunks.forEach((c) => unique.add(c)));
    stagedChunks.forEach((chunks) => chunks.forEach((c) => unique.add(c)));
    let rawBytes = 0,
      gpuBytes = 0;
    unique.forEach((c) => {
      rawBytes += c.rawBytes;
      gpuBytes += c.gpuBytes;
    });
    for (const display of [active, candidate]) {
      rawBytes += display?.matrixBytes ?? 0;
      gpuBytes += display?.matrixBytes ?? 0;
    }
    for (const lines of [overlay, stagedSection]) {
      rawBytes += lines?.rawBytes ?? 0;
      gpuBytes += lines?.gpuBytes ?? 0;
    }
    return { rawBytes, gpuBytes, ownedBytes: rawBytes + gpuBytes };
  }
  function reserve(raw: number, gpu: number) {
    if (
      !Number.isSafeInteger(raw) ||
      !Number.isSafeInteger(gpu) ||
      raw < 0 ||
      gpu < 0 ||
      usage().ownedBytes + raw + gpu > MAX_DISPLAY_BUFFER_BYTES
    )
      reject("Active plus staged display buffers exceed the 128 MiB support cap.");
  }
  function fail(message: string) {
    if (disposed || failed) return;
    failed = true;
    onFailure(message);
    void dispose().catch(() => undefined);
  }
  const uncaptured = (event: GPUUncapturedErrorEvent) =>
    fail(`WebGPU error: ${event.error.message}`);
  device.addEventListener("uncapturederror", uncaptured);
  void device.lost.then((info) => {
    if (info.reason !== "destroyed") fail(`WebGPU device lost: ${info.message || info.reason}`);
  });
  function redraw(): Promise<void> {
    available();
    redrawPending = true;
    if (rendering) return rendering;
    rendering = (async () => {
      while (redrawPending && !disposed && !failed) {
        redrawPending = false;
        await renderer.renderAsync(scene, camera);
        await device.queue.onSubmittedWorkDone();
      }
    })()
      .catch((error: unknown) => {
        fail(`Rendering failed: ${String(error)}`);
        throw error;
      })
      .finally(() => {
        rendering = null;
      });
    return rendering;
  }
  function perform(work: () => Promise<void>): Promise<void> {
    available();
    if (operation)
      return Promise.reject(
        new GeometrySynchronizationError("A viewport upload/update is already in progress."),
      );
    operation = work().finally(() => {
      operation = null;
    });
    return operation;
  }
  async function upload(root: Group) {
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await rendering;
    available();
    const stagingScene = new Scene();
    stagingScene.add(root);
    // Camera/frustum do not determine which staged buffers are admitted/uploaded.
    root.traverse((object) => {
      object.frustumCulled = false;
    });
    rendering = (async () => {
      renderer.setRenderTarget(uploadTarget);
      try {
        await renderer.renderAsync(stagingScene, camera);
        await device.queue.onSubmittedWorkDone();
      } catch (error) {
        fail(`Geometry upload failed: ${String(error)}`);
        throw error;
      } finally {
        renderer.setRenderTarget(null);
        stagingScene.remove(root);
      }
    })().finally(() => {
      rendering = null;
    });
    await rendering;
    available();
    if (redrawPending) await redraw();
  }
  function destroyChunk(chunk: Chunk) {
    // Both wrappers borrow the same attributes. They die together, never during selection changes.
    chunk.highlightGeometry.dispose();
    chunk.geometry.dispose();
  }
  function destroyDisplay(display: Display | null, retained: Set<Chunk>) {
    if (!display) return;
    scene.remove(display.root);
    display.batches.forEach((batch) => batch.mesh.dispose());
    display.definitions.forEach((d) =>
      d.chunks.forEach((chunk) => {
        if (!retained.has(chunk)) destroyChunk(chunk);
      }),
    );
  }
  function dropSection(section: SectionDisplay | null) {
    if (!section) return;
    scene.remove(section.root);
    section.root.children.forEach((object) => {
      if (object instanceof LineSegments) object.geometry.dispose();
    });
    section.root.clear();
  }
  function clearSelection() {
    highlights.clear();
  }
  function clearSections() {
    dropSection(overlay);
    overlay = null;
    dropSection(stagedSection);
    stagedSection = null;
  }
  function dropStaging() {
    const retained = new Set<Chunk>();
    active?.definitions.forEach((d) => d.chunks.forEach((c) => retained.add(c)));
    // Candidate references staged and active chunks; only its instance buffers die here.
    const candidateRetained = new Set(retained);
    stagedChunks.forEach((chunks) => chunks.forEach((chunk) => candidateRetained.add(chunk)));
    destroyDisplay(candidate, candidateRetained);
    candidate = null;
    stagedChunks.forEach((chunks) =>
      chunks.forEach((chunk) => {
        if (!retained.has(chunk)) destroyChunk(chunk);
      }),
    );
    stagedChunks.clear();
    stagedSummary = null;
  }
  function checkSummary(summary: SceneSummary) {
    if (
      !Number.isInteger(summary.revision) ||
      summary.revision < 0 ||
      summary.revision > 0xffffffff ||
      !Number.isInteger(summary.definition_count) ||
      summary.definition_count < 0 ||
      summary.definition_count > MAX_DEFINITIONS ||
      !Number.isInteger(summary.occurrence_count) ||
      summary.occurrence_count < 0 ||
      summary.occurrence_count > MAX_OCCURRENCES ||
      summary.definition_count > summary.occurrence_count ||
      (summary.definition_count === 0) !== (summary.occurrence_count === 0) ||
      (summary.occurrence_count === 0) !== (summary.bounds_mm === null) ||
      !Number.isInteger(summary.unique_mesh_bytes) ||
      summary.unique_mesh_bytes < 0 ||
      summary.unique_mesh_bytes > MAX_SCENE_MESH_BYTES
    )
      reject("Invalid scene support counts or revision.");
    if (summary.bounds_mm) {
      const { min, max } = summary.bounds_mm;
      for (let axis = 0; axis < 3; axis++)
        if (!Number.isFinite(min[axis]) || !Number.isFinite(max[axis]) || min[axis]! > max[axis]!)
          reject("Invalid scene bounds.");
    }
    if (
      active &&
      active.summary.session_id === summary.session_id &&
      summary.revision < active.summary.revision
    )
      reject("Cannot display a stale scene revision.");
  }
  function begin(summary: SceneSummary) {
    checkSummary(summary);
    dropStaging();
    stagedSummary = summary;
    synchronized = false;
    clearSelection();
    if (
      !active ||
      summary.session_id !== active.summary.session_id ||
      summary.revision !== active.summary.revision
    )
      clearSections();
  }
  function originFor(summary: SceneSummary): Vector3 {
    if (!summary.bounds_mm) return new Vector3();
    const { min, max } = summary.bounds_mm;
    return new Vector3(
      min[0] + (max[0] - min[0]) / 2,
      min[1] + (max[1] - min[1]) / 2,
      min[2] + (max[2] - min[2]) / 2,
    );
  }
  function matrixFor(occurrence: OccurrenceRecord, chunk: Chunk, origin: Vector3): Matrix4 {
    const { translation_mm: t, rotation_xyzw: r } = occurrence.pose;
    if (
      !t.every(Number.isFinite) ||
      !r.every(Number.isFinite) ||
      Math.abs(Math.hypot(...r) - 1) > POSE_NORM_TOLERANCE
    )
      reject("Invalid rigid occurrence pose.");
    const q = new Quaternion(...r);
    const p = new Vector3(...chunk.mesh.localOriginMm).applyQuaternion(q);
    // Subtract the f64 scene origin BEFORE Three converts matrices to f32.
    p.set(p.x + (t[0] - origin.x), p.y + (t[1] - origin.y), p.z + (t[2] - origin.z));
    const matrix = new Matrix4().compose(p, q, new Vector3(1, 1, 1));
    if (!matrix.elements.every((x) => Number.isFinite(x) && Number.isFinite(Math.fround(x))))
      reject("Occurrence exceeds finite GPU coordinates.");
    return matrix;
  }
  function buildDisplay(
    summary: SceneSummary,
    definitions: readonly GeometryDisplayDefinition[],
    occurrences: readonly OccurrenceRecord[],
  ): Display {
    if (
      definitions.length !== summary.definition_count ||
      occurrences.length !== summary.occurrence_count
    )
      reject("Incomplete scene pages.");
    const map = new Map<DefinitionId, Definition>();
    let meshBytes = 0,
      vertexCount = 0,
      triangleCount = 0,
      faceCount = 0;
    for (const record of definitions) {
      if (map.has(record.definition_id)) reject("Duplicate definition record.");
      if (
        record.faces.length !== record.face_count ||
        new Set(record.faces.map((row) => row.face_id)).size !== record.face_count ||
        record.faces.some(
          (row, ordinal) => row.ordinal !== ordinal || !/^step:[0-9]+$/.test(row.face_id),
        )
      )
        reject("Incomplete or reordered source-face table.");
      faceCount += record.face_count;
      const reused =
        active?.summary.session_id === summary.session_id
          ? active.definitions.get(record.definition_id)
          : undefined;
      const chunks =
        stagedChunks.get(record.definition_id) ??
        (reused?.record.mesh_artifact_id === record.mesh_artifact_id ? reused.chunks : undefined);
      if (!chunks?.length || chunks.length !== chunks[0]!.metadata.chunk_count)
        reject("Incomplete definition chunks.");
      chunks.forEach((chunk, index) => {
        if (
          chunk.metadata.chunk_index !== index ||
          chunk.metadata.artifact_id !== record.mesh_artifact_id ||
          chunk.metadata.face_count !== record.face_count ||
          chunk.metadata.session_id !== summary.session_id
        )
          reject("Definition chunk identity or face count mismatch.");
        meshBytes += chunk.metadata.byte_count;
        vertexCount += chunk.mesh.positions.length / 3;
        triangleCount += chunk.mesh.faceOrdinals.length;
      });
      map.set(record.definition_id, { record, chunks });
    }
    if (
      meshBytes !== summary.unique_mesh_bytes ||
      [...stagedChunks.keys()].some((id) => !map.has(id))
    )
      reject("Scene mesh byte count or definition set mismatch.");
    if (
      vertexCount > MAX_DISPLAY_VERTICES ||
      triangleCount > MAX_DISPLAY_TRIANGLES ||
      faceCount > MAX_NATIVE_FACES
    )
      reject("Unique scene vertices, triangles, or source faces exceed support limits.");
    const ids = new Set<number>();
    const grouped = new Map<DefinitionId, OccurrenceRecord[]>();
    for (const occurrence of occurrences) {
      if (
        !Number.isInteger(occurrence.occurrence_id) ||
        occurrence.occurrence_id <= 0 ||
        occurrence.occurrence_id > 0xffffffff ||
        ids.has(occurrence.occurrence_id) ||
        !map.has(occurrence.definition_id)
      )
        reject("Invalid occurrence identity.");
      ids.add(occurrence.occurrence_id);
      const group = grouped.get(occurrence.definition_id) ?? [];
      group.push(occurrence);
      grouped.set(occurrence.definition_id, group);
    }
    let matrixBytes = 0;
    map.forEach((definition, id) => {
      const group = grouped.get(id);
      if (!group?.length) reject("An unreferenced definition cannot own a display mesh.");
      matrixBytes += group.length * 64 * definition.chunks.length;
    });
    reserve(matrixBytes, matrixBytes);
    const display: Display = {
      summary,
      definitions: map,
      occurrences,
      root: new Group(),
      batches: [],
      origin: originFor(summary),
      matrixBytes,
    };
    try {
      map.forEach((definition, id) => {
        const group = grouped.get(id)!;
        for (const chunk of definition.chunks) {
          const mesh = new InstancedMesh(chunk.geometry, material, group.length);
          mesh.frustumCulled = false;
          const batch = { mesh, chunk, definition, occurrences: group };
          display.batches.push(batch);
          group.forEach((occurrence, index) =>
            mesh.setMatrixAt(index, matrixFor(occurrence, chunk, display.origin)),
          );
          mesh.instanceMatrix.needsUpdate = true;
          display.root.add(mesh);
        }
      });
      return display;
    } catch (error) {
      display.batches.forEach((batch) => batch.mesh.dispose());
      throw error;
    }
  }
  function projection() {
    camera.left = (-viewHeight * aspect) / 2;
    camera.right = (viewHeight * aspect) / 2;
    camera.top = viewHeight / 2;
    camera.bottom = -viewHeight / 2;
    camera.updateProjectionMatrix();
  }
  function fit(summary: SceneSummary | undefined) {
    const bounds = summary?.bounds_mm;
    const extent = bounds
      ? Math.max(...bounds.max.map((value, axis) => value - bounds.min[axis]!))
      : 1;
    const radius = Math.max((extent * Math.sqrt(3)) / 2, 0.01);
    viewHeight = (radius * 2.4) / Math.min(aspect, 1);
    camera.zoom = 1;
    camera.near = Math.max(radius / 10000, 0.00001);
    camera.far = radius * 20;
    const direction = camera.position.clone().sub(controls.target).normalize();
    controls.target.set(0, 0, 0);
    camera.position.copy(direction.multiplyScalar(radius * 4));
    controls.minZoom = 0.01;
    controls.maxZoom = 10000;
    controls.update();
    projection();
  }
  async function commit(
    definitions: readonly GeometryDisplayDefinition[],
    occurrences: readonly OccurrenceRecord[],
  ) {
    if (!stagedSummary) reject("Begin a scene update before committing.");
    await rendering;
    candidate = buildDisplay(stagedSummary, definitions, occurrences);
    const next = candidate;
    await upload(next.root);
    const previous = active;
    if (previous) scene.remove(previous.root);
    scene.add(next.root);
    if (!previous || previous.summary.occurrence_count === 0) fit(next.summary);
    await redraw();
    available();
    active = next;
    candidate = null;
    stagedChunks.clear();
    stagedSummary = null;
    const retained = new Set<Chunk>();
    next.definitions.forEach((d) => d.chunks.forEach((c) => retained.add(c)));
    destroyDisplay(previous, retained);
    synchronized = true;
  }
  const changed = () => {
    if (!disposed && !failed) void redraw().catch(() => undefined);
  };
  controls.addEventListener("change", changed);

  function dispose(): Promise<void> {
    if (disposal) return disposal;
    disposed = true;
    synchronized = false;
    observer?.disconnect();
    controls.removeEventListener("change", changed);
    controls.dispose();
    device.removeEventListener("uncapturederror", uncaptured);
    disposal = (async () => {
      await operation?.catch(() => undefined);
      await rendering?.catch(() => undefined);
      clearSelection();
      clearSections();
      dropStaging();
      destroyDisplay(active, new Set());
      active = null;
      material.dispose();
      selectionMaterial.dispose();
      lineMaterial.dispose();
      uploadTarget.dispose();
      try {
        await renderer.dispose();
      } finally {
        device.destroy();
      }
    })();
    return disposal;
  }
  try {
    await renderer.init();
    if (!("isWebGPUBackend" in renderer.backend) || renderer.backend.isWebGPUBackend !== true)
      throw new WebGPUUnavailableError(
        "The renderer did not initialize the required WebGPU backend.",
      );
    const resize = () => {
      if (disposed || failed) return;
      const { width, height } = canvas.getBoundingClientRect();
      if (width <= 0 || height <= 0) return;
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
      renderer.setSize(width, height, false);
      aspect = width / height;
      projection();
      changed();
    };
    observer = new ResizeObserver(resize);
    observer.observe(canvas);
    resize();
    await redraw();
    available();
    const info = adapter.info;
    return {
      adapter:
        [info?.vendor, info?.device, info?.description].filter(Boolean).join(" · ") ||
        "WebGPU adapter (identity not disclosed)",
      get bufferUsage() {
        return usage();
      },
      hasDefinition(id, artifact) {
        return (
          !disposed &&
          !!active &&
          (!stagedSummary || stagedSummary.session_id === active.summary.session_id) &&
          active.definitions.get(id)?.record.mesh_artifact_id === artifact
        );
      },
      beginScene(summary) {
        return perform(async () => {
          await rendering;
          begin(summary);
          await redraw();
        });
      },
      addDefinitionChunk(metadata, mesh) {
        return perform(async () => {
          if (
            !stagedSummary ||
            metadata.session_id !== stagedSummary.session_id ||
            mesh.chunkIndex !== metadata.chunk_index ||
            mesh.byteLength !== metadata.byte_count ||
            metadata.byte_count > MAX_GEOMETRY_CHUNK_BYTES
          )
            reject("Mesh chunk does not belong to the staged scene.");
          if (
            active?.summary.session_id === metadata.session_id &&
            active.definitions.get(metadata.definition_id)?.record.mesh_artifact_id ===
              metadata.artifact_id
          )
            reject("An already owned definition must be reused, not uploaded again.");
          const chunks = stagedChunks.get(metadata.definition_id) ?? [];
          if (
            metadata.chunk_index !== chunks.length ||
            (chunks.length &&
              (metadata.artifact_id !== chunks[0]!.metadata.artifact_id ||
                metadata.chunk_count !== chunks[0]!.metadata.chunk_count))
          )
            reject("Reordered or inconsistent mesh chunk.");
          if (
            !stagedChunks.has(metadata.definition_id) &&
            stagedChunks.size >= stagedSummary.definition_count
          )
            reject("Staged definitions exceed the scene declaration.");
          let stagedBytes = metadata.byte_count;
          let stagedVertices = mesh.positions.length / 3;
          let stagedTriangles = mesh.faceOrdinals.length;
          stagedChunks.forEach((entries) =>
            entries.forEach((entry) => {
              stagedBytes += entry.metadata.byte_count;
              stagedVertices += entry.mesh.positions.length / 3;
              stagedTriangles += entry.mesh.faceOrdinals.length;
            }),
          );
          if (
            stagedBytes > stagedSummary.unique_mesh_bytes ||
            stagedVertices > MAX_DISPLAY_VERTICES ||
            stagedTriangles > MAX_DISPLAY_TRIANGLES
          )
            reject("Staged mesh bytes, vertices, or triangles exceed the scene support limits.");
          const gpuBytes =
            mesh.positions.byteLength + mesh.normals.byteLength + mesh.indices.byteLength;
          const rawBytes = mesh.positions.buffer.byteLength;
          if (
            rawBytes > MAX_GEOMETRY_CHUNK_BYTES ||
            mesh.normals.buffer !== mesh.positions.buffer ||
            mesh.indices.buffer !== mesh.positions.buffer ||
            mesh.faceOrdinals.buffer !== mesh.positions.buffer
          )
            reject("Decoded mesh must borrow one bounded immutable chunk buffer.");
          reserve(rawBytes, gpuBytes);
          const geometry = new BufferGeometry();
          geometry.setAttribute("position", new BufferAttribute(mesh.positions, 3));
          geometry.setAttribute("normal", new BufferAttribute(mesh.normals, 3));
          geometry.setIndex(new BufferAttribute(mesh.indices, 1));
          const highlightGeometry = new BufferGeometry();
          highlightGeometry.setAttribute("position", geometry.getAttribute("position"));
          highlightGeometry.setIndex(geometry.index);
          const chunk: Chunk = { metadata, mesh, geometry, highlightGeometry, rawBytes, gpuBytes };
          chunks.push(chunk);
          stagedChunks.set(metadata.definition_id, chunks);
          const root = new Group();
          const uploadMesh = new Mesh(geometry, material);
          uploadMesh.frustumCulled = false;
          root.add(uploadMesh);
          await upload(root);
        });
      },
      commitScene(definitions, occurrences) {
        return perform(() => commit(definitions, occurrences));
      },
      updateOccurrences(summary, occurrences) {
        return perform(async () => {
          if (!active || active.summary.session_id !== summary.session_id)
            reject("No scene in the current session to update.");
          const records = [...active.definitions.values()]
            .map((definition) => definition.record)
            .filter((record) =>
              occurrences.some((occurrence) => occurrence.definition_id === record.definition_id),
            );
          await rendering;
          begin(summary);
          await commit(records, occurrences);
        });
      },
      async discardScene() {
        await operation?.catch(() => undefined);
        await rendering?.catch(() => undefined);
        available();
        const target = stagedSummary;
        dropStaging();
        if (target)
          synchronized =
            !!active &&
            target.session_id === active.summary.session_id &&
            target.revision === active.summary.revision;
        await redraw();
      },
      pick(ndcX, ndcY) {
        if (
          !synchronized ||
          !active ||
          disposed ||
          failed ||
          !Number.isFinite(ndcX) ||
          !Number.isFinite(ndcY) ||
          Math.abs(ndcX) > 1 ||
          Math.abs(ndcY) > 1
        )
          return null;
        scene.updateMatrixWorld(true);
        camera.updateMatrixWorld(true);
        raycaster.setFromCamera(new Vector2(ndcX, ndcY), camera);
        const hits = raycaster.intersectObjects(
          active.batches.map((batch) => batch.mesh),
          false,
        );
        const hit = hits[0];
        if (!hit || hit.instanceId === undefined || hit.faceIndex == null) return null;
        const batch = active.batches.find((entry) => entry.mesh === hit.object);
        const occurrence = batch?.occurrences[hit.instanceId];
        const ordinal = batch?.chunk.mesh.faceOrdinals[hit.faceIndex];
        const face = ordinal === undefined ? undefined : batch?.definition.record.faces[ordinal];
        if (!batch || !occurrence || !face) return null;
        return {
          session_id: active.summary.session_id,
          scene_revision: active.summary.revision,
          occurrence_id: occurrence.occurrence_id,
          definition_id: occurrence.definition_id,
          face_id: face.face_id,
        };
      },
      setSelection(reference) {
        return perform(async () => {
          await rendering;
          clearSelection();
          if (reference) {
            if (
              !synchronized ||
              !active ||
              reference.session_id !== active.summary.session_id ||
              reference.scene_revision !== active.summary.revision
            )
              reject("Selection is stale or the scene is paused.");
            const definition = active.definitions.get(reference.definition_id);
            const ordinal =
              definition?.record.faces.findIndex((row) => row.face_id === reference.face_id) ?? -1;
            const occurrence = active.occurrences.find(
              (entry) =>
                entry.occurrence_id === reference.occurrence_id &&
                entry.definition_id === reference.definition_id,
            );
            if (ordinal < 0 || !occurrence) reject("Unknown occurrence or source face.");
            for (const chunk of definition!.chunks) {
              const geometry = chunk.highlightGeometry;
              geometry.clearGroups();
              const faces = chunk.mesh.faceOrdinals;
              for (let start = 0; start < faces.length; ) {
                if (faces[start] !== ordinal) {
                  start++;
                  continue;
                }
                let end = start + 1;
                while (end < faces.length && faces[end] === ordinal) end++;
                geometry.addGroup(start * 3, (end - start) * 3, 0);
                start = end;
              }
              if (!geometry.groups.length) continue;
              // A material array selects only the existing index groups; no copied index/vertex buffer.
              const highlight = new Mesh(geometry, [selectionMaterial]);
              highlight.matrixAutoUpdate = false;
              highlight.matrix.copy(matrixFor(occurrence, chunk, active.origin));
              highlight.renderOrder = 1;
              highlight.frustumCulled = false;
              highlights.add(highlight);
            }
          }
          await redraw();
        });
      },
      showSection(summary, section) {
        return perform(async () => {
          if (
            !synchronized ||
            !active ||
            summary.session_id !== active.summary.session_id ||
            summary.revision !== active.summary.revision
          )
            reject("Section does not match the displayed scene revision.");
          validateSectionSummary(summary, {
            session_id: active.summary.session_id,
            artifact_id: summary.artifact_id,
            revision: active.summary.revision,
          });
          if (!section) {
            if (summary.chunk_count !== 0) reject("Nonempty section needs a decoded chunk.");
            await rendering;
            clearSections();
            await redraw();
            return;
          }
          if (section.chunkIndex === 0) {
            dropSection(stagedSection);
            stagedSection = {
              summary,
              root: new Group(),
              chunks: [],
              rawBytes: 0,
              gpuBytes: 0,
              byteCount: 0,
              loopCount: 0,
            };
          }
          const stage = stagedSection;
          if (
            !stage ||
            JSON.stringify(stage.summary) !== JSON.stringify(summary) ||
            section.chunkIndex !== stage.chunks.length ||
            section.chunkIndex >= summary.chunk_count ||
            section.firstLoopOrdinal !== stage.loopCount ||
            section.planeOriginMm.some((value, axis) => value !== summary.plane.origin_mm[axis])
          )
            reject("Reordered or inconsistent section chunks.");
          const rawBytes = section.pointsRelativeMm.buffer.byteLength;
          if (
            rawBytes > MAX_GEOMETRY_CHUNK_BYTES ||
            section.loopOffsets.buffer !== section.pointsRelativeMm.buffer
          )
            reject("Decoded section must borrow one bounded immutable chunk buffer.");
          let segments = 0;
          for (let loop = 0; loop + 1 < section.loopOffsets.length; loop++)
            segments += section.loopOffsets[loop + 1]! - section.loopOffsets[loop]! - 1;
          const gpuBytes = segments * 6 * 4;
          reserve(rawBytes + gpuBytes, gpuBytes);
          const positions = new Float32Array(segments * 6);
          const offset = new Vector3(...section.planeOriginMm).sub(active.origin);
          let cursor = 0;
          for (let loop = 0; loop + 1 < section.loopOffsets.length; loop++) {
            const first = section.loopOffsets[loop]!,
              end = section.loopOffsets[loop + 1]!;
            for (let point = first; point + 1 < end; point++) {
              for (const endpoint of [point, point + 1]) {
                for (let axis = 0; axis < 3; axis++) {
                  const value =
                    section.pointsRelativeMm[endpoint * 3 + axis]! + offset.getComponent(axis);
                  if (!Number.isFinite(value) || !Number.isFinite(Math.fround(value)))
                    reject("Section exceeds finite GPU coordinates.");
                  positions[cursor++] = value;
                }
              }
            }
          }
          const geometry = new BufferGeometry();
          geometry.setAttribute("position", new BufferAttribute(positions, 3));
          const lines = new LineSegments(geometry, lineMaterial);
          lines.renderOrder = 2;
          lines.frustumCulled = false;
          const root = new Group();
          root.add(lines);
          stage.chunks.push(section);
          stage.rawBytes += rawBytes + gpuBytes;
          stage.gpuBytes += gpuBytes;
          stage.byteCount += section.byteLength;
          stage.loopCount += section.loopOffsets.length - 1;
          try {
            await upload(root);
          } finally {
            // Even a failed upload is owned by staging and reclaimed during disposal.
            stage.root.add(lines);
          }
          if (stage.chunks.length === summary.chunk_count) {
            if (
              stage.byteCount !== summary.total_bytes ||
              stage.loopCount !== summary.total_loop_count
            )
              reject("Incomplete section manifest.");
            await rendering;
            if (overlay) scene.remove(overlay.root);
            scene.add(stage.root);
            await redraw();
            dropSection(overlay);
            overlay = stage;
            stagedSection = null;
          }
        });
      },
      async discardSection() {
        await operation?.catch(() => undefined);
        await rendering?.catch(() => undefined);
        available();
        dropSection(stagedSection);
        stagedSection = null;
        await redraw();
      },
      fitToScene() {
        return perform(async () => {
          await rendering;
          fit(active?.summary);
          await redraw();
        });
      },
      setView(view) {
        return perform(async () => {
          await rendering;
          const distance = Math.max(camera.position.distanceTo(controls.target), 1);
          camera.up.set(0, view === "front" ? 1 : 0, view === "front" ? 0 : 1);
          const direction =
            view === "front" ? new Vector3(0, 0, 1) : new Vector3(1, -1, 1).normalize();
          camera.position.copy(controls.target).addScaledVector(direction, distance);
          controls.update();
          await redraw();
        });
      },
      dispose,
    };
  } catch (error) {
    await dispose().catch(() => undefined);
    if (error instanceof WebGPUUnavailableError) throw error;
    throw new WebGPUUnavailableError(`WebGPU renderer initialization failed: ${String(error)}`);
  }
}
