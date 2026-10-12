/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  BufferAttribute,
  BufferGeometry,
  Group,
  LineBasicMaterial,
  LineSegments,
  Points,
  PointsMaterial,
  Vector3,
} from "three/webgpu";
import { REPLAY_HEADER_BYTES, REPLAY_RECORD_BYTES } from "@spiling/protocol";
import type { DecodedReplayDisplay, ReplayDisplayDescriptor } from "@spiling/protocol";

export interface ReplayView {
  layer: number | null;
  eventCount: number;
  showTravel: boolean;
}
interface Batch {
  lines: LineSegments;
  events: Uint32Array;
  kind: number;
}
export interface ReplayDisplay {
  readonly root: Group;
  readonly replay: DecodedReplayDisplay;
  readonly origin: Vector3;
  readonly rawBytes: number;
  readonly gpuBytes: number;
  setView(view: ReplayView): void;
  rebase(origin: Vector3): void;
  dispose(): void;
}
// Per-axis allowance ensures the full Euclidean projection error stays <= 0.001 mm.
const PRECISION_MM = 0.001 / Math.sqrt(3);
const reject = (message: string): never => {
  throw new Error(`Replay display: ${message}`);
};

/** Computes all retained raw/CPU/GPU buffer costs before any projection allocation. */
export function replayBudget(replay: DecodedReplayDisplay) {
  const segments = replay.eventCount - 1;
  const vertexBytes = segments * 24 + 12;
  return { rawBytes: replay.bytes.byteLength + vertexBytes + segments * 4, gpuBytes: vertexBytes };
}

/** One packed batch per kind, plus an explicit unknown-start endpoint (never an origin segment). */
export function createReplayDisplay(
  descriptor: ReplayDisplayDescriptor,
  replay: DecodedReplayDisplay,
  renderOrigin: Vector3 | null,
): ReplayDisplay {
  if (
    replay.eventCount !== descriptor.info.event_count ||
    replay.depositionCount !== descriptor.info.deposition_count ||
    replay.travelCount !== descriptor.info.travel_count ||
    replay.lifecycleCount !== descriptor.info.lifecycle_count ||
    replay.layerCount !== descriptor.info.layer_count ||
    replay.bytes.byteLength !== Number(descriptor.resource.size_bytes)
  )
    reject("descriptor mismatch");
  const data = new DataView(replay.bytes);
  const origin =
    renderOrigin?.clone() ??
    new Vector3(
      replay.boundsMm.min[0] / 2 + replay.boundsMm.max[0] / 2,
      replay.boundsMm.min[1] / 2 + replay.boundsMm.max[1] / 2,
      replay.boundsMm.min[2] / 2 + replay.boundsMm.max[2] / 2,
    );
  const root = new Group();
  const batches: Batch[] = [];
  const materials: (LineBasicMaterial | PointsMaterial)[] = [];
  const geometries: BufferGeometry[] = [];
  const counts = new Uint32Array(8);
  for (let event = 1; event < replay.eventCount; event++) {
    const kind = data.getUint8(REPLAY_HEADER_BYTES + event * REPLAY_RECORD_BYTES + 46);
    counts[kind] = counts[kind]! + 1;
  }
  const project = (event: number, axis: number): number => {
    const absolute = data.getFloat64(
      REPLAY_HEADER_BYTES + event * REPLAY_RECORD_BYTES + axis * 8,
      true,
    );
    const relative = absolute - origin.getComponent(axis);
    const value = Math.fround(relative);
    if (!Number.isFinite(value) || Math.abs(value - relative) > PRECISION_MM)
      reject("coordinates cannot be projected within 0.001 mm");
    return value;
  };
  const dispose = () => {
    root.removeFromParent();
    geometries.forEach((geometry) => geometry.dispose());
    materials.forEach((material) => material.dispose());
    root.clear();
  };
  try {
    const colors = [
      "#8eacbd",
      "#ffbb66",
      "#6cdec7",
      "#b693ff",
      "#ff799b",
      "#8ed05a",
      "#7b91eb",
      "#fff07a",
    ];
    for (let kind = 0; kind < counts.length; kind++) {
      if (!counts[kind]) continue;
      const positions = new Float32Array(counts[kind]! * 6);
      const events = new Uint32Array(counts[kind]!);
      let cursor = 0;
      for (let event = 1; event < replay.eventCount; event++) {
        if (data.getUint8(REPLAY_HEADER_BYTES + event * REPLAY_RECORD_BYTES + 46) !== kind)
          continue;
        events[cursor] = event;
        for (let endpoint = 0; endpoint < 2; endpoint++)
          for (let axis = 0; axis < 3; axis++)
            positions[cursor * 6 + endpoint * 3 + axis] = project(event - 1 + endpoint, axis);
        cursor++;
      }
      const geometry = new BufferGeometry();
      geometries.push(geometry);
      geometry.setAttribute("position", new BufferAttribute(positions, 3));
      const material = new LineBasicMaterial({ color: colors[kind], depthTest: false });
      materials.push(material);
      const lines = new LineSegments(geometry, material);
      lines.frustumCulled = false;
      lines.renderOrder = 3;
      root.add(lines);
      batches.push({ lines, events, kind });
    }
    const geometry = new BufferGeometry();
    geometries.push(geometry);
    const point = new Float32Array(3);
    for (let axis = 0; axis < 3; axis++) point[axis] = project(0, axis);
    geometry.setAttribute("position", new BufferAttribute(point, 3));
    const material = new PointsMaterial({
      color: "#ff77b7",
      size: 7,
      sizeAttenuation: false,
      depthTest: false,
    });
    materials.push(material);
    const marker = new Points(geometry, material);
    marker.frustumCulled = false;
    marker.renderOrder = 4;
    root.add(marker);
    const layerAt = (event: number) =>
      data.getUint16(REPLAY_HEADER_BYTES + event * REPLAY_RECORD_BYTES + 44, true);
    const lowerBound = (events: Uint32Array, predicate: (event: number) => boolean): number => {
      let low = 0,
        high = events.length;
      while (low < high) {
        const middle = (low + high) >>> 1;
        if (predicate(events[middle]!)) high = middle;
        else low = middle + 1;
      }
      return low;
    };
    const setView = (view: ReplayView) => {
      if (
        !Number.isInteger(view.eventCount) ||
        view.eventCount < 0 ||
        view.eventCount > replay.eventCount ||
        typeof view.showTravel !== "boolean" ||
        (view.layer !== null &&
          (!Number.isInteger(view.layer) || view.layer < 0 || view.layer >= replay.layerCount))
      )
        reject("invalid draw state");
      for (const batch of batches) {
        const lifecycle = batch.kind >= 6;
        const start =
          view.layer === null || lifecycle
            ? 0
            : lowerBound(batch.events, (event) => layerAt(event) >= view.layer!);
        const ordinalEnd = lowerBound(batch.events, (event) => event >= view.eventCount);
        const layerEnd =
          view.layer === null || lifecycle
            ? batch.events.length
            : lowerBound(batch.events, (event) => layerAt(event) > view.layer!);
        const end = Math.min(ordinalEnd, layerEnd);
        batch.lines.geometry.setDrawRange(start * 2, Math.max(0, end - start) * 2);
        batch.lines.visible =
          (!lifecycle || view.layer === null) &&
          ((batch.kind !== 0 && batch.kind !== 6) || view.showTravel);
      }
      marker.visible =
        view.showTravel &&
        view.eventCount > 0 &&
        (view.layer === null || view.layer === layerAt(0));
    };
    const rebase = (nextOrigin: Vector3) => {
      const offset = origin.clone().sub(nextOrigin);
      // Include f32 model translation and addition, not only vertex quantization.
      for (let event = 0; event < replay.eventCount; event++) {
        for (let axis = 0; axis < 3; axis++) {
          const absolute = data.getFloat64(
            REPLAY_HEADER_BYTES + event * REPLAY_RECORD_BYTES + axis * 8,
            true,
          );
          const projected = Math.fround(
            project(event, axis) + Math.fround(offset.getComponent(axis)),
          );
          const expected = absolute - nextOrigin.getComponent(axis);
          if (!Number.isFinite(projected) || Math.abs(projected - expected) > PRECISION_MM)
            reject("overlay origin exceeds the 0.001 mm projection bound");
        }
      }
      root.position.copy(offset);
    };
    setView({ layer: null, eventCount: replay.eventCount, showTravel: true });
    return { root, replay, origin, ...replayBudget(replay), setView, rebase, dispose };
  } catch (error) {
    dispose();
    throw error;
  }
}
