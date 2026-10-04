/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  BufferAttribute,
  BufferGeometry,
  Color,
  DoubleSide,
  GridHelper,
  Mesh,
  MeshBasicMaterial,
  OrthographicCamera,
  Scene,
  WebGPURenderer,
} from "three/webgpu";
import type { DiagnosticTriangle } from "@spiling/protocol";

export interface DiagnosticViewport {
  readonly adapter: string;
  showTriangle(triangle: DiagnosticTriangle): Promise<void>;
  dispose(): Promise<void>;
}

export class WebGPUUnavailableError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WebGPUUnavailableError";
  }
}

/** Probe and initialize a real WebGPU device before the caller starts an engine. */
export async function createDiagnosticViewport(
  canvas: HTMLCanvasElement,
  onFailure: (message: string) => void,
): Promise<DiagnosticViewport> {
  if (!navigator.gpu)
    throw new WebGPUUnavailableError(
      "WebGPU is unavailable in this runtime. No WebGL fallback is used.",
    );
  const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
  if (!adapter)
    throw new WebGPUUnavailableError(
      "No WebGPU adapter is available. Check the GPU driver and CEF configuration.",
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
  // Three's WebGPURenderer installs a WebGL fallback even with forceWebGL:false.
  // Disable that pinned implementation hook BEFORE init so an init failure is
  // an explicit unsupported-GPU state, never a successful WebGL substitution.
  Object.defineProperty(renderer, "_getFallback", { value: null, writable: false });
  let disposed = false;
  let failed = false;
  let renderPending = false;
  let renderPromise: Promise<void> | null = null;
  let disposal: Promise<void> | null = null;
  const scene = new Scene();
  scene.background = new Color("#101b22");
  const camera = new OrthographicCamera(-1.6, 1.6, 1.3, -1.3, 0.1, 20);
  camera.position.set(0, 0, 5);
  camera.lookAt(0, 0, 0);
  const grid = new GridHelper(8, 32, "#385461", "#21343e");
  grid.rotation.x = Math.PI / 2;
  grid.position.z = -0.02;
  scene.add(grid);
  const material = new MeshBasicMaterial({ color: "#eebc69", side: DoubleSide });
  let mesh: Mesh<BufferGeometry, MeshBasicMaterial> | null = null;
  let observer: ResizeObserver | null = null;

  const fail = (message: string) => {
    if (disposed || failed) return;
    failed = true;
    onFailure(message);
    void dispose().catch(() => undefined);
  };
  const uncaptured = (event: GPUUncapturedErrorEvent) =>
    fail(`WebGPU error: ${event.error.message}`);
  device.addEventListener("uncapturederror", uncaptured);
  void device.lost.then((info) => {
    if (info.reason !== "destroyed") fail(`WebGPU device lost: ${info.message || info.reason}`);
  });

  const redraw = (): Promise<void> => {
    if (disposed || failed) return Promise.resolve();
    renderPending = true;
    if (renderPromise) return renderPromise;
    renderPromise = (async () => {
      while (renderPending && !disposed && !failed) {
        renderPending = false;
        await renderer.renderAsync(scene, camera);
      }
    })().finally(() => {
      renderPromise = null;
    });
    return renderPromise;
  };
  const clear = () => {
    if (!mesh) return;
    scene.remove(mesh);
    mesh.geometry.dispose();
    mesh = null;
    if (!disposed)
      void redraw().catch((error: unknown) => fail(`Rendering failed: ${String(error)}`));
  };
  function dispose(): Promise<void> {
    if (disposal) return disposal;
    disposed = true;
    observer?.disconnect();
    device.removeEventListener("uncapturederror", uncaptured);
    disposal = (async () => {
      // No frame continues using buffers after they are released.
      await renderPromise?.catch(() => undefined);
      clear();
      material.dispose();
      grid.geometry.dispose();
      if (Array.isArray(grid.material)) grid.material.forEach((entry) => entry.dispose());
      else grid.material.dispose();
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
    if (!("isWebGPUBackend" in renderer.backend) || renderer.backend.isWebGPUBackend !== true) {
      throw new WebGPUUnavailableError(
        "The renderer did not initialize the required WebGPU backend.",
      );
    }
    const resize = () => {
      if (disposed) return;
      const { width, height } = canvas.getBoundingClientRect();
      if (width <= 0 || height <= 0) return;
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
      renderer.setSize(width, height, false);
      const aspect = width / height;
      camera.left = -1.3 * aspect;
      camera.right = 1.3 * aspect;
      camera.updateProjectionMatrix();
      void redraw().catch((error: unknown) => fail(`Rendering failed: ${String(error)}`));
    };
    observer = new ResizeObserver(resize);
    observer.observe(canvas);
    resize();
    await redraw();
    if (failed || disposed)
      throw new WebGPUUnavailableError("WebGPU failed during initial rendering.");
    const info = adapter.info;
    const label =
      [info?.vendor, info?.device, info?.description].filter(Boolean).join(" · ") ||
      "WebGPU adapter (identity not disclosed)";
    return {
      adapter: label,
      async showTriangle(triangle) {
        await renderPromise;
        if (disposed || failed) throw new Error("The WebGPU viewport is no longer available.");
        if (mesh) {
          scene.remove(mesh);
          mesh.geometry.dispose();
        }
        const geometry = new BufferGeometry();
        geometry.setAttribute("position", new BufferAttribute(triangle.positions, 3));
        geometry.setIndex(new BufferAttribute(triangle.indices, 1));
        mesh = new Mesh(geometry, material);
        scene.add(mesh);
        await redraw();
      },
      dispose,
    };
  } catch (error) {
    await dispose().catch(() => undefined);
    if (error instanceof WebGPUUnavailableError) throw error;
    throw new WebGPUUnavailableError(`WebGPU renderer initialization failed: ${String(error)}`);
  }
}
