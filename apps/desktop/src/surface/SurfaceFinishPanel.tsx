/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useRef, useState, type FormEvent } from "react";
import type {
  FaceRef, ImageReliefParameters, SurfaceFinish, SurfaceImageRecord, SurfaceScopeKind,
} from "@spiling/protocol";
import type { SurfaceFinishEdit, WorkbenchState } from "../useWorkbench.ts";
import "./surface.css";

const fields = ["depth_mm", "pitch_mm", "edge_margin_mm", "phase_mm"] as const;
const labels = [
  "Depth (mm)",
  "Pitch (mm)",
  "Protected edge margin (mm)",
  "Source-local phase (mm)",
];
const cylinderFields = ["depth_mm", "rib_count", "phase_fraction"] as const;
const cylinderLabels = [
  "Depth (mm)",
  "Ribs around circumference",
  "Phase (fraction of one rib cycle)",
];
const imageFields = [
  "depth_mm", "u_repeats", "u_phase_fraction", "z_period_mm", "z_phase_mm",
  "edge_margin_mm", "vertical_error_mm",
] as const satisfies readonly (keyof ImageReliefParameters)[];
const imageLabels = [
  "Maximum additive depth (mm)", "Repeats across source U (integer)",
  "U phase (fraction of an image cycle)", "Source Z period (mm)",
  "Source Z phase (mm)", "Protected planar edge margin (mm; cylinder: 0)",
  "Layer approximation displacement error (mm)",
];
const finishLabels: Record<SurfaceFinish["kind"], string> = {
  smooth: "Smooth",
  vertical_ribs: "Planar vertical ribs",
  cylindrical_ribs: "Cylindrical vertical ribs",
  image_relief: "Image relief",
};

export function SurfaceFinishPanel({
  state,
  inspect,
  edit,
  selectImage,
  importImage,
  previewImage,
  cancel,
}: {
  state: WorkbenchState;
  inspect(reference: FaceRef | null): Promise<void>;
  edit(command: SurfaceFinishEdit): Promise<void>;
  selectImage(relativePath?: string): Promise<void>;
  importImage(reference: FaceRef, scope: SurfaceScopeKind, parameters: ImageReliefParameters): Promise<SurfaceImageRecord>;
  previewImage(imageHash: string): Promise<void>;
  cancel(): void;
}) {
  const [occurrenceId, setOccurrenceId] = useState<number | null>(null);
  const [scope, setScope] = useState<SurfaceScopeKind>("occurrence");
  const [kind, setKind] = useState<SurfaceFinish["kind"]>("vertical_ribs");
  const [values, setValues] = useState(["0.1", "2", "1", "0"]);
  const [cylinderValues, setCylinderValues] = useState(["0.1", "8", "0"]);
  const [imageValues, setImageValues] = useState(["0.5", "1", "0", "10", "0", "1", "0.05"]);
  const [imagePath, setImagePath] = useState("");
  const [storedImageHash, setStoredImageHash] = useState("");
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");
  const project = state.project;
  const storedImage = project?.surface_overlays.images.find((image) => image.image_hash === storedImageHash);
  const lastImageHash = project?.surface_overlays.layers.reduce<string | null>(
    (hash, layer) => layer.finish.kind === "image_relief" ? layer.finish.image_hash : hash, null,
  ) ?? null;
  const reference = state.inspection?.reference ?? null;
  useEffect(() => {
    if (reference) setOccurrenceId(reference.occurrence_id);
  }, [reference]);
  useEffect(() => {
    setOccurrenceId(null);
    setMessage("");
    setStoredImageHash("");
  }, [state.scene?.session_id, state.scene?.revision, state.projectEpoch]);
  const occurrence = state.occurrences.find((row) => row.occurrence_id === occurrenceId);
  const definition = state.definitions.find(
    (row) => row.definition_id === occurrence?.definition_id,
  );
  const available = state.phase === "running" && !state.busy && !state.stale && !working;
  const writable = available && !!project && !project.read_only;
  const currentFace =
    reference &&
    state.scene &&
    occurrence &&
    reference.session_id === state.scene.session_id &&
    reference.scene_revision === state.scene.revision &&
    reference.occurrence_id === occurrence.occurrence_id &&
    reference.definition_id === occurrence.definition_id
      ? reference
      : null;
  const analytic = currentFace ? state.inspection?.face.geometry.analytic : null;
  const finishFamilySupported = kind === "smooth" ||
    (kind === "vertical_ribs" && analytic?.kind === "plane") ||
    (kind === "cylindrical_ribs" && analytic?.kind === "cylinder") ||
    (kind === "image_relief" && (analytic?.kind === "plane" || analytic?.kind === "cylinder"));
  async function transact(command: SurfaceFinishEdit) {
    setWorking(true);
    setMessage("");
    try {
      await edit(command);
      setMessage(
        "Surface finish committed. Compile, then inspect fresh emitted-program replay to see manufactured paths.",
      );
    } catch (error) {
      setMessage(
        `Surface finish rejected: ${error instanceof Error ? error.message : String(error)}`,
      );
    } finally {
      setWorking(false);
    }
  }
  async function chooseImage(relativePath?: string) {
    setWorking(true);
    setMessage("");
    try { await selectImage(relativePath); }
    catch (error) { setMessage(`Image selection failed: ${error instanceof Error ? error.message : String(error)}`); }
    finally { setWorking(false); }
  }
  async function showImage(imageHash: string) {
    setWorking(true);
    setMessage("");
    try {
      await previewImage(imageHash);
      setMessage("Canonical captured height image loaded. This is source image data, not a manufactured-shape preview.");
    } catch (error) {
      setMessage(`Image preview failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally { setWorking(false); }
  }
  async function applyImage(parameters: ImageReliefParameters) {
    if (!currentFace) return;
    setWorking(true);
    setMessage("");
    let committed = false;
    try {
      const record = await importImage(currentFace, scope, parameters);
      committed = true;
      setStoredImageHash(record.image_hash);
      await previewImage(record.image_hash);
      setMessage(`Image layer committed using authoritative asset ${record.source_name}. Canonical captured preview loaded; undo is available before Compile.`);
    } catch (error) {
      setMessage(`${committed ? "Image layer committed, but preview failed" : "Image import rejected"}: ${error instanceof Error ? error.message : String(error)}`);
    } finally { setWorking(false); }
  }
  function apply(event: FormEvent) {
    event.preventDefault();
    if (!writable || !currentFace) return;
    if (!finishFamilySupported) {
      setMessage("Unsupported finish family: planar ribs require an exact plane; cylindrical ribs require an exact cylinder; image relief requires an exact plane or cylinder. Other analytic or generic surfaces are not finish-certified.");
      return;
    }
    if (kind === "image_relief") {
      if (!storedImage && !state.selectedImage) { setMessage("Choose a new image or an existing captured image first."); return; }
      if (imageValues.some((value) => value.trim() === "" || !Number.isFinite(Number(value)))) {
        setMessage("Enter finite values for all seven image mapping controls."); return;
      }
      const [depth, repeats, uPhase, period, zPhase, margin, error] = imageValues.map(Number);
      if (depth! <= 0 || !Number.isInteger(repeats) || repeats! < 1 || repeats! > 256 ||
          uPhase! < 0 || uPhase! >= 1 || period! <= 0 || zPhase! < 0 || zPhase! >= period! ||
          margin! < 0 || error! <= 0) {
        setMessage("Depth, Z period and error must be positive; repeats integer 1–256; U phase [0,1), Z phase [0,period), margin nonnegative.");
        return;
      }
      const parameters: ImageReliefParameters = {
        depth_mm: depth!, u_repeats: repeats!, u_phase_fraction: uPhase!,
        z_period_mm: period!, z_phase_mm: zPhase!, edge_margin_mm: margin!, vertical_error_mm: error!,
      };
      if (storedImage) {
        void transact({
          op: "add_surface_finish", face_ref: currentFace, scope,
          finish: {kind: "image_relief", image_hash: storedImage.image_hash, parameters},
        });
      } else void applyImage(parameters);
      return;
    }
    let finish: SurfaceFinish = { kind: "smooth" };
    if (kind === "vertical_ribs") {
      if (values.some((value) => value.trim() === "" || !Number.isFinite(Number(value)))) {
        setMessage("Enter finite millimetre values for every rib control.");
        return;
      }
      const [depth, pitch, margin, phase] = values.map(Number);
      if (depth! <= 0 || pitch! <= 0 || margin! <= 0 || phase! < 0 || phase! >= pitch!) {
        setMessage(
          "Depth, pitch and margin must be positive; phase must be at least zero and less than pitch.",
        );
        return;
      }
      finish = {
        kind: "vertical_ribs",
        depth_mm: depth!,
        pitch_mm: pitch!,
        edge_margin_mm: margin!,
        phase_mm: phase!,
      };
    }
    if (kind === "cylindrical_ribs") {
      if (cylinderValues.some((value) => value.trim() === "" || !Number.isFinite(Number(value)))) {
        setMessage("Enter finite values for depth, rib count and phase fraction.");
        return;
      }
      const [depth, count, phase] = cylinderValues.map(Number);
      if (
        depth! <= 0 ||
        !Number.isInteger(count) ||
        count! < 2 ||
        count! > 256 ||
        phase! < 0 ||
        phase! >= 1
      ) {
        setMessage(
          "Depth must be positive; rib count an integer from 2 to 256; phase fraction at least zero and less than one.",
        );
        return;
      }
      finish = {
        kind: "cylindrical_ribs",
        depth_mm: depth!,
        rib_count: count!,
        phase_fraction: phase!,
      };
    }
    void transact({ op: "add_surface_finish", face_ref: currentFace, scope, finish });
  }
  return (
    <section
      className="surface-panel geometry-controls"
      aria-label="Surface Finish"
      data-testid="surface-panel"
    >
      <h3>Surface Finish</h3>
      <p>
        Source-backed whole-face finish; not a shader or edited BREP. Software-only construction,
        with no grip or physical-print qualification.
      </p>
      <p>
        Planar ribs require a native certified full-height rectangular exterior vertical wall.
        Cylindrical ribs require one full-turn exterior cylinder face, not an inward bore or
        separate half-faces. Both require upright local Z and bed contact. Protected native holes
        remain unchanged. Compile checks process, topology, work and nominal clearance; authoring
        alone does not certify eligibility.
      </p>
      <label>
        Native occurrence
        <select
          data-testid="surface-occurrence"
          aria-label="Surface occurrence"
          disabled={!available}
          value={occurrence?.occurrence_id ?? ""}
          onChange={(event) => {
            setOccurrenceId(event.target.value ? Number(event.target.value) : null);
            void inspect(null);
          }}
        >
          <option value="">Select an occurrence</option>
          {state.occurrences.map((row) => (
            <option key={row.occurrence_id} value={row.occurrence_id}>
              Occurrence {row.occurrence_id} / definition {row.definition_id}
            </option>
          ))}
        </select>
      </label>
      <label>
        Native source face
        <select
          data-testid="surface-face"
          aria-label="Surface source face"
          disabled={!available || !occurrence}
          value={currentFace?.face_id ?? ""}
          onChange={(event) => {
            if (!occurrence || !state.scene) return;
            void inspect(
              event.target.value
                ? {
                    session_id: state.scene.session_id,
                    scene_revision: state.scene.revision,
                    occurrence_id: occurrence.occurrence_id,
                    definition_id: occurrence.definition_id,
                    face_id: event.target.value,
                  }
                : null,
            );
          }}
        >
          <option value="">Select a native source face</option>
          {definition?.faces.map((face) => (
            <option key={face.face_id} value={face.face_id}>
              {face.face_id}
            </option>
          ))}
        </select>
      </label>
      {currentFace && (
        <p>
          Selected occurrence {currentFace.occurrence_id}, definition {currentFace.definition_id},
          face {currentFace.face_id}. Project revision {project?.revision}.
        </p>
      )}
      {currentFace && !finishFamilySupported && (
        <p role="alert" data-testid="surface-unsupported-family">
          Unsupported finish dispatch for {state.inspection?.face.geometry.family}: this finish
          requires an exact analytic plane or cylinder of the selected kind. Generic family
          classification is descriptive only; Compile must still certify eligible carriers.
        </p>
      )}
      <form onSubmit={apply}>
        <fieldset disabled={!writable}>
          <legend>Append whole-face layer</legend>
          <label>
            Scope
            <select
              data-testid="surface-scope"
              value={scope}
              onChange={(event) => setScope(event.target.value as SurfaceScopeKind)}
            >
              <option value="occurrence">Occurrence — only this instance</option>
              <option value="definition">Definition — every instance of this source</option>
            </select>
          </label>
          <label>
            Finish
            <select
              data-testid="surface-kind"
              value={kind}
              onChange={(event) => setKind(event.target.value as typeof kind)}
            >
              <option value="vertical_ribs">Planar vertical ribs</option>
              <option value="cylindrical_ribs">Cylindrical vertical ribs</option>
              <option value="smooth">Smooth override</option>
              <option value="image_relief">Image relief — PNG / canonical PGM</option>
            </select>
          </label>
          {kind !== "smooth" && kind !== "image_relief" && (
            <div className="surface-numbers">
              {(kind === "cylindrical_ribs" ? cylinderFields : fields).map((field, index) => (
                <label key={field}>
                  {(kind === "cylindrical_ribs" ? cylinderLabels : labels)[index]}
                  <input
                    data-testid={`surface-${field}`}
                    type="number"
                    step={field === "rib_count" ? "1" : "any"}
                    value={(kind === "cylindrical_ribs" ? cylinderValues : values)[index]}
                    onChange={(event) =>
                      (kind === "cylindrical_ribs" ? setCylinderValues : setValues)((old) =>
                        old.map((value, i) => (i === index ? event.target.value : value)),
                      )
                    }
                  />
                </label>
              ))}
            </div>
          )}
          {kind === "cylindrical_ribs" && (
            <p>
              Phase is measured from source-local +X about +Z, independent of the native seam.
              Integer rib count closes the circumference. This is straight grip ribbing, not crossed
              or helical knurling.
            </p>
          )}
          {kind === "image_relief" && (
            <>
              <label>
                Height image
                <select data-testid="surface-image-asset" value={storedImage?.image_hash ?? ""}
                  onChange={(event) => setStoredImageHash(event.target.value)}>
                  <option value="">Import a new image file</option>
                  {project?.surface_overlays.images.map((image) => (
                    <option key={image.image_hash} value={image.image_hash}>
                      Captured: {image.source_name} ({image.width} × {image.height})
                    </option>
                  ))}
                </select>
              </label>
              {storedImage && <p>Reuse captured samples with these mapping parameters on the selected face. This appends an undoable metadata-only layer; the original image file is not required.</p>}
              {!storedImage && (
              <>
              <div className="geometry-buttons">
                <button type="button" data-testid="surface-image-select" onClick={() => void chooseImage()}>
                  Choose PNG / PGM image
                </button>
                <span data-testid="surface-image-selection">
                  {state.selectedImage ? state.selectedImage.label : "No image selected"}
                </span>
              </div>
              {state.runtime?.geometry_debug_enabled && (
                <details>
                  <summary>Confined height image fixture admission</summary>
                  <input data-testid="surface-image-path" value={imagePath}
                    aria-label="Height image fixture relative path"
                    onChange={(event) => setImagePath(event.target.value)} />
                  <button type="button" data-testid="surface-image-debug-select"
                    disabled={!imagePath} onClick={() => void chooseImage(imagePath)}>
                    Select fixture image using production token
                  </button>
                </details>
              )}
              </>
              )}
              <div className="surface-numbers">
                {imageFields.map((field, index) => (
                  <label key={field}>
                    {imageLabels[index]}
                    <input type="number" step={field === "u_repeats" ? "1" : "any"}
                      data-testid={`surface-${field}`} value={imageValues[index]}
                      onChange={(event) => setImageValues((old) => old.map(
                        (value, i) => i === index ? event.target.value : value,
                      ))} />
                  </label>
                ))}
              </div>
              <p>
                Black is zero added height; white is maximum depth. Import image and apply captures
                normalized immutable samples and appends one undoable layer; its canonical preview
                appears after authoring, before Compile. No original image path enters the workbench.
              </p>
              <p>
                Mapping is fixed in the source frame, including flipped source Z. Cylinder U starts
                at source +X and turns right-handed about +Z, independent of native UV/seams.
                Planar U follows the certified tangent from its canonical lower endpoint.
                Both U and source Z wrap bilinearly from last to first pixel nodes with no half-pixel
                shift. Image top points toward +source Z; positive phase advances the sampled coordinate.
              </p>
              <p>
                Z period and phase are physical source millimetres relative to the certified minimum Z.
                Vertical error bounds layer-approximation displacement, separately from the 0.001 mm XY
                chord certificate and physical accuracy. Planar margin must be at least two bead widths,
                with face width greater than four margins; protected bands and smooth end ramps preserve
                native edges. For a cylinder the unused margin must be zero. Compile owns these hard
                carrier, process, schedule and support checks; no silent untextured fallback.
              </p>
            </>
          )}
          <button type="submit" data-testid="surface-apply"
            disabled={!writable || !currentFace || (kind === "image_relief" && !storedImage && !state.selectedImage)}>
            {kind === "image_relief" ? storedImage ? "Apply captured image — append layer" : "Import image and apply" : "Apply — append layer"}
          </button>
        </fieldset>
      </form>
      {!writable && (
        <p>
          {project?.read_only
            ? "Read-only snapshot: finish edits disabled."
            : "Finish edits require a current native scene and idle writable project."}
        </p>
      )}
      {message && (
        <p role="status" data-testid="surface-message">
          {message}
        </p>
      )}
      {state.activeJob && (
        <div role="status" data-testid="surface-image-progress">
          {state.activeJob.stage}: {state.activeJob.completed_units}/{state.activeJob.total_units} · {state.activeJob.status}
          <button type="button" data-testid="surface-image-cancel" onClick={cancel}
            disabled={state.activeJob.status === "cancelling"}>Cancel native operation</button>
        </div>
      )}
      <h4>Captured height image assets</h4>
      <p>Preview reads canonical Gray8 bytes from the current project, including fresh read-only opens without the original PNG. It does not preview displaced geometry; Compile and fresh actual-program replay do that.</p>
      <button type="button" data-testid="surface-image-preview"
        disabled={!available || !lastImageHash}
        onClick={() => { if (lastImageHash) void showImage(lastImageHash); }}>
        Preview last authored captured height image
      </button>
      <ul data-testid="surface-images">
        {project?.surface_overlays.images.map((image) => (
          <li key={image.image_hash}>
            <strong>{image.source_name}</strong> · {image.width} × {image.height} · {image.byte_count} B
            <code>{image.image_hash}</code>
            <span>Original source SHA-256: {image.source_hash}</span>
            <button type="button" data-testid={`surface-image-preview-${image.image_hash}`}
              disabled={!available} onClick={() => void showImage(image.image_hash)}>
              Preview captured height image
            </button>
          </li>
        ))}
      </ul>
      {state.imagePreview && <HeightImagePreview preview={state.imagePreview} />}
      <h4>Authoritative ordered layers</h4>
      <p>
        Enabled definition layers apply first, then enabled occurrence layers; last matching layer
        wins. Smooth overrides ribs. Remove or disable reveals the lower finish. Stored order below
        is project authority, not a local preview.
      </p>
      <ol className="surface-layers" data-testid="surface-layers">
        {project?.surface_overlays.layers.map((layer) => (
          <li key={layer.layer_id}>
            <strong>
              Layer {layer.layer_id}: {finishLabels[layer.finish.kind]} ·{" "}
              {layer.enabled ? "enabled" : "disabled"}
            </strong>
            <span>
              {layer.scope.kind === "definition"
                ? `Definition ${layer.scope.definition_id} — all instances`
                : `Occurrence ${layer.scope.occurrence_id} — definition ${layer.scope.expected_definition_id}`}{" "}
              / face {layer.face_id}
            </span>
            {layer.finish.kind === "vertical_ribs" && (
              <span>
                Depth {layer.finish.depth_mm} mm · pitch {layer.finish.pitch_mm} mm · margin{" "}
                {layer.finish.edge_margin_mm} mm · phase {layer.finish.phase_mm} mm
              </span>
            )}
            {layer.finish.kind === "cylindrical_ribs" && (
              <span>
                Depth {layer.finish.depth_mm} mm · {layer.finish.rib_count} ribs · phase{" "}
                {layer.finish.phase_fraction} of one rib cycle
              </span>
            )}
            {layer.finish.kind === "image_relief" && (
              <span>
                Image {layer.finish.image_hash} · depth {layer.finish.parameters.depth_mm} mm · U repeats{" "}
                {layer.finish.parameters.u_repeats} · U phase {layer.finish.parameters.u_phase_fraction} · Z period{" "}
                {layer.finish.parameters.z_period_mm} mm · Z phase {layer.finish.parameters.z_phase_mm} mm · margin{" "}
                {layer.finish.parameters.edge_margin_mm} mm · vertical error {layer.finish.parameters.vertical_error_mm} mm
              </span>
            )}
            <div className="geometry-buttons">
              <button
                data-testid={`surface-enabled-${layer.layer_id}`}
                disabled={!writable}
                onClick={() =>
                  void transact({
                    op: "set_surface_finish_enabled",
                    layer_id: layer.layer_id,
                    enabled: !layer.enabled,
                  })
                }
              >
                {layer.enabled ? "Disable" : "Enable"}
              </button>
              <button
                data-testid={`surface-remove-${layer.layer_id}`}
                disabled={!writable}
                onClick={() =>
                  void transact({ op: "remove_surface_finish", layer_id: layer.layer_id })
                }
              >
                Remove
              </button>
            </div>
          </li>
        ))}
      </ol>
      {project?.surface_overlays.layers.length === 0 && <p>No authored finish layers.</p>}
      <details>
        <summary>Current surface authority</summary>
        <pre data-testid="surface-state">
          {JSON.stringify(
            {
              selectedFaceRef: currentFace,
              revision: project?.revision ?? null,
              surface_overlays: project?.surface_overlays ?? null,
              read_only: project?.read_only ?? null,
              working: working || state.busy,
              stale: state.stale,
              selectedImage: state.selectedImage,
              previewImageHash: state.imagePreview?.record.image_hash ?? null,
              imagePreview: state.imagePreview ? {
                record: state.imagePreview.record,
                width: state.imagePreview.image.width,
                height: state.imagePreview.image.height,
                pixelCount: state.imagePreview.image.pixels.length,
              } : null,
            },
            null,
            2,
          )}
        </pre>
      </details>
    </section>
  );
}

function HeightImagePreview({ preview }: { preview: NonNullable<WorkbenchState["imagePreview"]> }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [canvasError, setCanvasError] = useState("");
  useEffect(() => {
    const context = canvas.current?.getContext("2d");
    if (!context) {
      setCanvasError("Canonical image decoded, but Canvas 2D is unavailable; no preview was rendered.");
      return;
    }
    setCanvasError("");
    const { image } = preview;
    const rgba = context.createImageData(image.width, image.height);
    for (let i = 0; i < image.pixels.length; i++) {
      rgba.data[i * 4] = image.pixels[i]!;
      rgba.data[i * 4 + 1] = image.pixels[i]!;
      rgba.data[i * 4 + 2] = image.pixels[i]!;
      rgba.data[i * 4 + 3] = 255;
    }
    context.putImageData(rgba, 0, 0);
  }, [preview]);
  return (
    <figure data-testid="surface-image-display">
      <canvas ref={canvas} width={preview.image.width} height={preview.image.height}
        data-testid="surface-image-preview-canvas" aria-label={`Canonical height image ${preview.record.source_name}`} />
      {canvasError && <p role="alert" data-testid="surface-image-canvas-error">{canvasError}</p>}
      <figcaption>Captured canonical height samples: {preview.record.source_name}; source image top is +source Z, black 0 / white 255. Not manufactured geometry or a printability certificate.</figcaption>
    </figure>
  );
}
