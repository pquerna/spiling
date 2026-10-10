/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useRef, useState, type FormEvent } from "react";
import licenseDocument from "../../../LICENSE.md?raw";
import {
  IMPORT_PROFILE,
  PROTOCOL_VERSION,
  type OccurrenceRecord,
  type RigidPoseMm,
} from "@spiling/protocol";
import { useWorkbench, type Phase } from "./useWorkbench.ts";

const assentKey = "spiling:license-assent:OSL-3.0:b0-1";
const phaseDescriptions: Record<Phase, string> = {
  "desktop-required": "Desktop runtime required",
  probing: "Checking WebGPU",
  starting: "Connecting to native engine",
  running: "Engine connected",
  stopped: "Session stopped",
  interrupted: "Engine interrupted",
  unsupported: "WebGPU unavailable",
  error: "Session could not start",
};
function LicenseDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  return (
    <dialog ref={dialog} onCancel={onClose} onClose={onClose} className="license-dialog">
      <div className="section-heading">
        <h2>Open Software License 3.0</h2>
        <button onClick={onClose}>Close</button>
      </div>
      <pre className="license-copy">{licenseDocument}</pre>
    </dialog>
  );
}

function DiscardDialog({
  label,
  onCancel,
  onConfirm,
}: {
  label: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  return (
    <dialog
      ref={dialog}
      className="license-dialog"
      data-testid="project-discard-dialog"
      onCancel={onCancel}
      onClose={onCancel}
    >
      <h2>Discard unsaved project changes?</h2>
      <p>
        {label} discards edits since the last checkpoint. Unsaved edits and session undo are not
        replayed.
      </p>
      <button data-testid="project-discard-cancel" onClick={onCancel}>
        Keep editing
      </button>
      <button data-testid="project-discard-confirm" onClick={onConfirm}>
        Discard changes
      </button>
    </dialog>
  );
}

function PoseEditor({
  occurrence,
  disabled,
  apply,
}: {
  occurrence: OccurrenceRecord;
  disabled: boolean;
  apply: (pose: RigidPoseMm) => void;
}) {
  const [translation, setTranslation] = useState(occurrence.pose.translation_mm.map(String));
  const [rotation, setRotation] = useState(["0", "0", "0"]);
  const [message, setMessage] = useState("");
  useEffect(() => {
    setTranslation(occurrence.pose.translation_mm.map(String));
    const [x, y, z, w] = occurrence.pose.rotation_xyzw;
    const m13 = 2 * (x * z + w * y),
      m23 = 2 * (y * z - w * x);
    const m33 = 1 - 2 * (x * x + y * y),
      m12 = 2 * (x * y - w * z);
    const m11 = 1 - 2 * (y * y + z * z),
      m32 = 2 * (y * z + w * x),
      m22 = 1 - 2 * (x * x + z * z);
    const ry = Math.asin(Math.max(-1, Math.min(1, m13)));
    const rx = Math.abs(m13) < 0.9999999 ? Math.atan2(-m23, m33) : Math.atan2(m32, m22);
    const rz = Math.abs(m13) < 0.9999999 ? Math.atan2(-m12, m11) : 0;
    setRotation([rx, ry, rz].map((value) => String((value * 180) / Math.PI)));
    setMessage("");
  }, [occurrence]);
  function submit(event: FormEvent) {
    event.preventDefault();
    if (
      [...translation, ...rotation].some(
        (value) => value.trim() === "" || !Number.isFinite(Number(value)),
      )
    ) {
      setMessage("Enter finite numeric translation and rotation values.");
      return;
    }
    const [rx, ry, rz] = rotation.map((value) => (Number(value) * Math.PI) / 360);
    const c1 = Math.cos(rx!),
      c2 = Math.cos(ry!),
      c3 = Math.cos(rz!);
    const s1 = Math.sin(rx!),
      s2 = Math.sin(ry!),
      s3 = Math.sin(rz!);
    const quaternion: [number, number, number, number] = [
      s1 * c2 * c3 + c1 * s2 * s3,
      c1 * s2 * c3 - s1 * c2 * s3,
      c1 * c2 * s3 + s1 * s2 * c3,
      c1 * c2 * c3 - s1 * s2 * s3,
    ];
    const norm = Math.hypot(...quaternion),
      sign = quaternion[3] < 0 ? -1 : 1;
    apply({
      translation_mm: translation.map(Number) as [number, number, number],
      rotation_xyzw: quaternion.map((value) => (value * sign) / norm) as [
        number,
        number,
        number,
        number,
      ],
    });
  }
  return (
    <form className="geometry-form" onSubmit={submit}>
      <h3>Instance {occurrence.occurrence_id} placement</h3>
      <fieldset disabled={disabled}>
        <legend>Translation (mm)</legend>
        <div className="numeric-triple">
          {translation.map((value, axis) => (
            <label key={axis}>
              {"XYZ"[axis]}
              <input
                type="number"
                step="any"
                aria-label={`Translation ${"XYZ"[axis]}`}
                value={value}
                onChange={(event) =>
                  setTranslation((old) =>
                    old.map((item, i) => (i === axis ? event.target.value : item)),
                  )
                }
              />
            </label>
          ))}
        </div>
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Euler XYZ (degrees)</legend>
        <div className="numeric-triple">
          {rotation.map((value, axis) => (
            <label key={axis}>
              {"XYZ"[axis]}
              <input
                type="number"
                step="any"
                aria-label={`Rotation ${"XYZ"[axis]}`}
                value={value}
                onChange={(event) =>
                  setRotation((old) =>
                    old.map((item, i) => (i === axis ? event.target.value : item)),
                  )
                }
              />
            </label>
          ))}
        </div>
      </fieldset>
      {message && <p role="alert">{message}</p>}
      <button disabled={disabled} type="submit" data-testid="apply-pose">
        Apply placement
      </button>
    </form>
  );
}

function Workbench() {
  const canvas = useRef<HTMLCanvasElement>(null),
    pointer = useRef<[number, number] | null>(null);
  const workbench = useWorkbench(canvas),
    { state, execute } = workbench;
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [plane, setPlane] = useState(["0", "0", "4", "0", "0", "1"]),
    [planeError, setPlaneError] = useState("");
  const [debugPaths, setDebugPaths] = useState('["box-mm.step", "cylinder-mm.step"]');
  const [debugError, setDebugError] = useState("");
  const [readOnly, setReadOnly] = useState(false);
  const [projectFixture, setProjectFixture] = useState("desktop-project");
  const [discard, setDiscard] = useState<{
    label: string;
    action: (discardChanges: boolean) => void;
  } | null>(null);
  function replaceProject(label: string, action: (discardChanges: boolean) => void) {
    if (state.project?.dirty) setDiscard({ label, action });
    else action(false);
  }
  const running = state.phase === "running",
    interactive = running && !state.busy && !state.stale,
    editable = interactive && !state.project?.read_only;
  const selected = state.occurrences.find((row) => row.occurrence_id === selectedId) ?? null;
  const definition = state.definitions.find((row) => row.definition_id === selected?.definition_id);
  const greeting = state.hello,
    noObjects = !state.occurrences.length;
  useEffect(() => {
    if (state.inspection) setSelectedId(state.inspection.reference.occurrence_id);
  }, [state.inspection]);
  useEffect(() => {
    setSelectedId(null);
  }, [state.scene?.session_id, state.scene?.revision]);
  return (
    <main className="workbench">
      {discard && (
        <DiscardDialog
          label={discard.label}
          onCancel={() => setDiscard(null)}
          onConfirm={() => {
            const action = discard.action;
            setDiscard(null);
            action(true);
          }}
        />
      )}
      <aside className="outline panel">
        <div className="panel-title">
          NATIVE SCENE <span>{state.scene?.revision ?? 0}</span>
        </div>
        <div className="geometry-controls">
          <section
            className="project-controls"
            aria-label="Durable project"
            data-testid="project-controls"
          >
            <h2>Project</h2>
            {!running && state.project && (
              <small>
                Last session status; no live authoring authority until the checkpoint is reopened.
              </small>
            )}
            <output data-testid="project-state">
              {state.project ? (
                <>
                  <strong data-testid="project-id">{state.project.project_id}</strong>
                  <span data-testid="project-revision">Revision {state.project.revision}</span>
                  <span data-testid="project-dirty">
                    {state.project.dirty ? "Unsaved changes" : "Clean"}
                  </span>
                  <span data-testid="project-saved">
                    {state.project.path_label ??
                      "Unattached — choose a new directory for first save"}
                  </span>
                  <span data-testid="project-saved-revision">
                    Saved revision: {state.project.saved_revision ?? "none"}
                  </span>
                  <span data-testid="project-read-only-state">
                    {state.project.read_only ? "Read-only snapshot" : "Writable"}
                  </span>
                  {state.project.recovered_previous && (
                    <strong data-testid="project-recovered">
                      Recovered previous checkpoint — save explicitly to commit recovery
                    </strong>
                  )}
                  {state.project.save_uncertain && (
                    <strong data-testid="project-save-uncertain" role="alert">
                      Save durability is uncertain: committed storage is attached, but the
                      checkpoint is not confirmed. Inspect the outcome before explicitly saving
                      again; do not assume cancellation or a successful checkpoint.
                    </strong>
                  )}
                </>
              ) : (
                "No active project"
              )}
            </output>
            <div className="geometry-buttons">
              <button
                data-testid="project-new"
                disabled={!running || state.busy}
                onClick={() =>
                  replaceProject("New project", (discardChanges) => {
                    void workbench.projectControl("new", discardChanges);
                  })
                }
              >
                New
              </button>
              <button
                data-testid="project-open"
                disabled={!running || state.busy}
                onClick={() =>
                  replaceProject("Open project", (discardChanges) => {
                    void workbench.openProject(readOnly, false, discardChanges);
                  })
                }
              >
                Open…
              </button>
              <button
                data-testid="project-save"
                disabled={!editable}
                onClick={() => {
                  void workbench.saveProject();
                }}
              >
                Save
              </button>
              <button
                data-testid="project-undo"
                disabled={!editable || !state.project?.can_undo}
                onClick={() => {
                  void workbench.projectControl("undo");
                }}
              >
                Undo
              </button>
              <button
                data-testid="project-redo"
                disabled={!editable || !state.project?.can_redo}
                onClick={() => {
                  void workbench.projectControl("redo");
                }}
              >
                Redo
              </button>
              <button
                data-testid="project-recover"
                disabled={!running || state.busy}
                onClick={() =>
                  replaceProject("Recover previous checkpoint", (discardChanges) => {
                    void workbench.openProject(readOnly, true, discardChanges);
                  })
                }
              >
                Recover previous…
              </button>
            </div>
            <label>
              <input
                type="checkbox"
                data-testid="project-read-only"
                checked={readOnly}
                onChange={(event) => setReadOnly(event.target.checked)}
              />
              Open / recover read-only
            </label>
            <p>
              Save checkpoints exact imported sources and placements. Undo is session-only; restart
              reopens the attached checkpoint, never unsaved mutations.
            </p>
            {state.runtime?.geometry_debug_enabled && (
              <details className="geometry-debug">
                <summary>Privileged project fixture admission</summary>
                <input
                  data-testid="project-debug-path"
                  aria-label="Project fixture relative directory"
                  value={projectFixture}
                  onChange={(event) => setProjectFixture(event.target.value)}
                />
                <button
                  data-testid="project-debug-save"
                  disabled={!editable || state.project?.path_label != null}
                  onClick={() => {
                    void workbench.saveProject(projectFixture);
                  }}
                >
                  First save fixture directory
                </button>
                <button
                  data-testid="project-debug-open"
                  disabled={!running || state.busy}
                  onClick={() =>
                    replaceProject("Open fixture project", (discardChanges) => {
                      void workbench.openProject(readOnly, false, discardChanges, projectFixture);
                    })
                  }
                >
                  Open fixture directory
                </button>
                <button
                  data-testid="project-debug-recover"
                  disabled={!running || state.busy}
                  onClick={() =>
                    replaceProject("Recover fixture checkpoint", (discardChanges) => {
                      void workbench.openProject(readOnly, true, discardChanges, projectFixture);
                    })
                  }
                >
                  Recover fixture checkpoint
                </button>
              </details>
            )}
          </section>
          <button
            className="primary"
            data-testid="import-parts"
            disabled={!editable}
            onClick={() => {
              void workbench.importParts();
            }}
          >
            Import parts…
          </button>
          <p>
            Independent planar/cylindrical STEP parts. Multi-select imports sequentially; earlier
            successful parts remain if a later import fails.
          </p>
          {state.runtime?.geometry_debug_enabled && (
            <details className="geometry-debug">
              <summary>Privileged fixture admission</summary>
              <textarea
                data-testid="geometry-debug-paths"
                aria-label="Fixture relative paths JSON"
                value={debugPaths}
                onChange={(event) => setDebugPaths(event.target.value)}
              />
              <button
                data-testid="geometry-debug-import"
                disabled={!editable}
                onClick={() => {
                  try {
                    const paths: unknown = JSON.parse(debugPaths);
                    if (!Array.isArray(paths) || !paths.every((path) => typeof path === "string")) {
                      throw new Error("Expected a JSON array of relative fixture paths.");
                    }
                    setDebugError("");
                    void workbench.debugImport(paths);
                  } catch (error) {
                    setDebugError(error instanceof Error ? error.message : String(error));
                  }
                }}
              >
                Import fixture parts
              </button>
              {debugError && <p role="alert">{debugError}</p>}
            </details>
          )}
          {noObjects && <p>No objects. Import a supported STEP part to begin.</p>}
          <ul className="instance-tree" data-testid="instance-tree">
            {state.definitions.map((part) => (
              <li key={part.definition_id}>
                <strong>{part.provenance.source_name}</strong>
                <small>
                  {part.face_count} native faces · {part.provenance.source_unit}
                </small>
                <ul>
                  {state.occurrences
                    .filter((row) => row.definition_id === part.definition_id)
                    .map((row) => (
                      <li key={row.occurrence_id}>
                        <button
                          aria-pressed={selected?.occurrence_id === row.occurrence_id}
                          data-occurrence-id={row.occurrence_id}
                          onClick={() => {
                            setSelectedId(row.occurrence_id);
                            void workbench.inspect(null);
                          }}
                        >
                          Instance {row.occurrence_id}
                        </button>
                      </li>
                    ))}
                </ul>
              </li>
            ))}
          </ul>
          {selected && (
            <>
              <div className="geometry-buttons">
                <button
                  disabled={!editable}
                  data-testid="add-instance"
                  onClick={() => {
                    void workbench.mutate("add", selected);
                  }}
                >
                  Add instance
                </button>
                <button
                  disabled={!editable}
                  data-testid="remove-instance"
                  onClick={() => {
                    void workbench.mutate("remove", selected);
                  }}
                >
                  Remove instance
                </button>
              </div>
              <PoseEditor
                occurrence={selected}
                disabled={!editable}
                apply={(pose) => {
                  void workbench.mutate("pose", selected, pose);
                }}
              />
            </>
          )}
        </div>
        <div className="scope-note">
          <span className="eyebrow">SUPPORTED INPUT PROFILE</span>
          <strong>{IMPORT_PROFILE}</strong>
          <p>
            Native BREP is authoritative. Mesh surface deviation ≤0.05 mm relative to admitted
            carriers. Durable source-backed projects and bounded session undo; no general STEP
            assembly or manufacturing operations.
          </p>
        </div>
      </aside>
      <section className="viewport-panel panel" aria-label="Native geometry viewport">
        <div className="viewport-toolbar">
          <div>
            <span className="eyebrow">NATIVE / MM</span>
            <h1>Project authoring &amp; inspection</h1>
          </div>
          <span className={`gpu-tag ${state.gpu ? "ready" : ""}`}>WEBGPU ONLY</span>
        </div>
        <div className="view-actions">
          <button
            disabled={!running}
            onClick={() => {
              void workbench.fit();
            }}
          >
            Fit scene
          </button>
          <button
            disabled={!running}
            onClick={() => {
              void workbench.setView("front");
            }}
          >
            Front
          </button>
          <button
            disabled={!running}
            onClick={() => {
              void workbench.setView("isometric");
            }}
          >
            Isometric
          </button>
          <small>Drag to orbit · pan/zoom · click a face to inspect</small>
        </div>
        <div className={`canvas-stage ${!running ? "inactive" : ""}`}>
          <canvas
            ref={canvas}
            data-testid="viewport"
            aria-label="WebGPU display of native parts"
            onPointerDown={(event) => {
              pointer.current = [event.clientX, event.clientY];
            }}
            onPointerUp={(event) => {
              const start = pointer.current;
              pointer.current = null;
              if (
                !interactive ||
                !start ||
                Math.hypot(event.clientX - start[0], event.clientY - start[1]) > 4
              )
                return;
              const bounds = event.currentTarget.getBoundingClientRect();
              void workbench.pick(
                ((event.clientX - bounds.left) / bounds.width) * 2 - 1,
                1 - ((event.clientY - bounds.top) / bounds.height) * 2,
              );
            }}
          />
          {(!running || (noObjects && !state.stale)) && (
            <div className="viewport-empty" role="status">
              <h2>{running ? "Empty native scene" : phaseDescriptions[state.phase]}</h2>
              <p>
                {running
                  ? "Import parts or open a saved project. First save chooses a new project directory."
                  : state.phase === "desktop-required"
                    ? "Open the Tauri / CEF desktop; browsers cannot launch the engine."
                    : state.phase === "unsupported"
                      ? "A working WebGPU device is required. No WebGL fallback is used."
                      : state.message || "Connecting the engine after GPU admission."}
              </p>
            </div>
          )}
          {state.stale && (
            <div className="stale-display" data-testid="stale-display" role="status">
              Engine revision {state.scene?.revision} is not synchronized. Display is stale and
              noninteractive.
              <button
                disabled={!running || state.busy}
                data-testid="resume-display"
                onClick={() => {
                  void workbench.resume();
                }}
              >
                Resume display update
              </button>
            </div>
          )}
          <div className="artifact-caption">
            APPROXIMATE DISPLAY · SOURCE FACE IDENTITY · NATIVE MM
          </div>
        </div>
        <div className="transfer-strip" data-testid="transfer-metrics">
          <div>
            <span>Transferred this update</span>
            <strong>{state.transfer ? `${state.transfer.bytes} B` : "—"}</strong>
          </div>
          <div>
            <span>Engine → UI</span>
            <strong>{state.transfer?.requestMs.toFixed(2) ?? "—"} ms</strong>
          </div>
          <div>
            <span>Validation</span>
            <strong>{state.transfer?.decodeMs.toFixed(2) ?? "—"} ms</strong>
          </div>
          <div>
            <span>Upload/render</span>
            <strong>{state.transfer?.renderMs.toFixed(2) ?? "—"} ms</strong>
          </div>
        </div>
        <p className="measurement-note">
          Wall-clock intervals, not GPU timestamps. Shared definitions transfer once; placement
          changes do not retessellate.
        </p>
        {state.activeJob && (
          <div className="job-status" role="status" data-testid="geometry-job">
            Job {state.activeJob.job_id}: {state.activeJob.status} / {state.activeJob.stage}
            <button data-testid="cancel-job" onClick={workbench.cancel}>
              Cancel job
            </button>
          </div>
        )}
        {state.transferring && (
          <div className="job-status" role="status">
            Validating/uploading {state.transferring} chunks
            <button data-testid="cancel-transfer" onClick={workbench.cancel}>
              Pause transfer
            </button>
          </div>
        )}
      </section>
      <aside className="inspector panel">
        <div className="panel-title">
          INSPECTION <span>NATIVE</span>
        </div>
        <div className={`status-card status-${state.phase}`}>
          <span className="status-dot" />
          <div>
            <strong data-testid="engine-status">{state.phase}</strong>
            <small>{phaseDescriptions[state.phase]}</small>
          </div>
        </div>
        {state.message && (
          <div className="diagnostic-message" role="alert">
            {state.message}
          </div>
        )}
        {planeError && (
          <div className="diagnostic-message" role="alert">
            {planeError}
          </div>
        )}
        <div className="engine-actions">
          <button
            data-testid="restart-engine"
            disabled={
              state.phase === "desktop-required" ||
              state.phase === "probing" ||
              state.phase === "starting"
            }
            onClick={() => {
              replaceProject("Restart / reopen saved checkpoint", (discardChanges) => {
                void execute(running ? "restart" : "start", discardChanges);
              });
            }}
          >
            {running ? "Restart engine" : "Start engine"}
          </button>
          <button
            data-testid="stop-engine"
            disabled={!running}
            onClick={() => {
              replaceProject("Stop session", (discardChanges) => {
                void execute("stop", discardChanges);
              });
            }}
          >
            Stop session
          </button>
        </div>
        <div className="geometry-controls">
          <h3>Face inspector</h3>
          {definition && (
            <select
              aria-label="Native face"
              disabled={!interactive}
              value={state.inspection?.reference.face_id ?? ""}
              onChange={(event) => {
                if (!selected || !state.scene || !event.target.value) return;
                void workbench.inspect({
                  session_id: state.scene.session_id,
                  scene_revision: state.scene.revision,
                  occurrence_id: selected.occurrence_id,
                  definition_id: selected.definition_id,
                  face_id: event.target.value,
                });
              }}
            >
              <option value="">Select or click a source face</option>
              {definition.faces.map((face) => (
                <option key={face.face_id} value={face.face_id}>
                  {face.face_id}
                </option>
              ))}
            </select>
          )}
          {state.inspection ? (
            <div className="face-inspection" data-testid="face-inspection">
              <strong>
                Instance {state.inspection.reference.occurrence_id} /{" "}
                {state.inspection.face.face_id}
              </strong>
              <p>
                {state.inspection.face.source_entity_kind} · surface entity{" "}
                {state.inspection.face.surface_face_entity}
              </p>
              <p>
                Orientation: {state.inspection.face.orientation ? "forward" : "reversed"},
                independent of carrier
              </p>
              <pre>{JSON.stringify(state.inspection.face.carrier, null, 2)}</pre>
              <p>
                Source unit: {state.inspection.provenance.source_unit}. Declared uncertainty:{" "}
                {state.inspection.provenance.uncertainty_mm ?? "not provided"} mm.
              </p>
              <p className="hash-value">
                Source SHA-256: {state.inspection.provenance.source_hash}
              </p>
              <p>
                Carrier parameters are native definition-frame mm, not an exact GPU hit measurement.
              </p>
            </div>
          ) : (
            <p>Select an instance, then pick or select a native source face.</p>
          )}
          <form
            className="geometry-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (
                plane.some((value) => value.trim() === "" || !Number.isFinite(Number(value))) ||
                Math.hypot(...plane.slice(3).map(Number)) === 0
              ) {
                setPlaneError("Enter finite plane coordinates and a nonzero normal.");
                return;
              }
              setPlaneError("");
              void workbench.section({
                origin_mm: plane.slice(0, 3).map(Number) as [number, number, number],
                normal: plane.slice(3).map(Number) as [number, number, number],
              });
            }}
          >
            <h3>Native planar section</h3>
            {["Origin (scene mm)", "Normal"].map((label, group) => (
              <fieldset key={label} disabled={!interactive}>
                <legend>{label}</legend>
                <div className="numeric-triple">
                  {plane.slice(group * 3, group * 3 + 3).map((value, axis) => {
                    const index = group * 3 + axis;
                    return (
                      <label key={index}>
                        {"XYZ"[axis]}
                        <input
                          type="number"
                          step="any"
                          aria-label={`${label} ${"XYZ"[axis]}`}
                          value={value}
                          onChange={(event) =>
                            setPlane((old) =>
                              old.map((item, i) => (i === index ? event.target.value : item)),
                            )
                          }
                        />
                      </label>
                    );
                  })}
                </div>
              </fieldset>
            ))}
            <button data-testid="start-section" disabled={!interactive || noObjects}>
              Preview section
            </button>
          </form>
          {state.section && (
            <p data-testid="section-summary">
              Overlay: {state.section.total_loop_count} native loops at origin{" "}
              {state.section.plane.origin_mm.join(", ")} / normal{" "}
              {state.section.plane.normal.join(", ")}. Sampling ≤
              {state.section.sampling_tolerance_mm} mm.
            </p>
          )}
        </div>
        <dl className="session-facts">
          <div>
            <dt>Engine PID</dt>
            <dd data-testid="engine-pid">{greeting?.pid ?? "—"}</dd>
          </div>
          <div>
            <dt>Protocol</dt>
            <dd>
              {greeting?.protocol_version ?? PROTOCOL_VERSION} / {PROTOCOL_VERSION}
            </dd>
          </div>
          <div>
            <dt>Kernel</dt>
            <dd>
              {greeting ? `${greeting.kernel.name} ${greeting.kernel.version}` : "Not negotiated"}
            </dd>
          </div>
          <div>
            <dt>Kernel revision</dt>
            <dd className="hash-value">{greeting?.kernel.revision ?? "—"}</dd>
          </div>
          <div>
            <dt>Session</dt>
            <dd className="hash-value">{greeting?.session_id ?? "—"}</dd>
          </div>
          <div>
            <dt>Native scene</dt>
            <dd>
              {state.scene
                ? `${state.scene.definition_count} definitions / ${state.scene.occurrence_count} occurrences / ${state.scene.unique_mesh_bytes} unique mesh B`
                : "—"}
            </dd>
          </div>
          <div>
            <dt>Owned display bytes</dt>
            <dd>
              {state.bufferUsage.ownedBytes} (raw {state.bufferUsage.rawBytes}, submitted GPU{" "}
              {state.bufferUsage.gpuBytes})
            </dd>
          </div>
          <div>
            <dt>WebGPU</dt>
            <dd>{state.gpu ?? "No active device"}</dd>
          </div>
          <div>
            <dt>Desktop</dt>
            <dd>{state.runtime ? `${state.runtime.runtime} / ${state.runtime.app_build}` : "—"}</dd>
          </div>
        </dl>
        <details className="geometry-controls">
          <summary>Negotiated capabilities and limits</summary>
          <p>{greeting?.geometry_capabilities.join(", ") ?? "Not negotiated"}</p>
          <pre>{greeting ? JSON.stringify(greeting.geometry_limits, null, 2) : "No handshake"}</pre>
        </details>
        <details className="geometry-controls">
          <summary>Scene and display inspection metrics</summary>
          <output data-testid="geometry-state">
            <pre>
              {JSON.stringify(
                {
                  session_id: greeting?.session_id ?? null,
                  scene: state.scene,
                  definitions: state.definitions.map(({ faces: _faces, ...record }) => record),
                  occurrences: state.occurrences,
                  inspection: state.inspection,
                  section: state.section,
                  stale: state.stale,
                  transferring: state.transferring,
                  active_job: state.activeJob,
                  transfer: state.transfer,
                  buffer_usage: state.bufferUsage,
                },
                null,
                2,
              )}
            </pre>
          </output>
          <p>Owned buffer counters are not total GPU/driver memory measurements.</p>
        </details>
        <div className="diagnostic-tools">
          <button
            className="danger"
            data-testid="interrupt-engine"
            disabled={!running}
            onClick={() => {
              replaceProject("Interrupt engine", (discardChanges) => {
                void execute("interrupt", discardChanges);
              });
            }}
          >
            Interrupt engine
          </button>
        </div>
      </aside>
    </main>
  );
}

export function App() {
  const [accepted, setAccepted] = useState(() => {
    try {
      return localStorage.getItem(assentKey) === "accepted";
    } catch {
      return false;
    }
  });
  const [checked, setChecked] = useState(false),
    [showLicense, setShowLicense] = useState(false),
    [storageNotice, setStorageNotice] = useState("");
  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">S</span>
          <div>
            <strong>SPILING</strong>
            <span>Manufacturing workbench</span>
          </div>
        </div>
        <div className="milestone">
          <span className="milestone-dot" />
          GEOMETRY <span className="quiet">/ native inspection</span>
        </div>
      </header>
      {accepted ? (
        <Workbench />
      ) : (
        <main className="license-gate" data-testid="license-gate">
          <section className="license-welcome">
            <span className="eyebrow">A NATIVE WORKBENCH, BUILT IN THE OPEN</span>
            <h1>
              Inspect native
              <br />
              geometry and identity.
            </h1>
            <p>
              Spiling connects a supervised native BREP engine to WebGPU inspection. Import
              supported STEP parts, place instances and inspect source faces and planar sections.
              Persistence and manufacturing authoring are not available.
            </p>
            <div className="welcome-detail">
              <span>01</span>
              <div>
                <strong>Explicit license assent</strong>
                <small>
                  Review the full license before entering. No engine launches until you accept and
                  WebGPU initialization succeeds.
                </small>
              </div>
            </div>
          </section>
          <section className="license-review panel">
            <div className="section-heading">
              <div>
                <span className="eyebrow">OSL-3.0 / AVAILABLE OFFLINE</span>
                <h2>Open Software License 3.0</h2>
              </div>
            </div>
            <pre className="license-copy">{licenseDocument}</pre>
            <label className="assent-control">
              <input
                type="checkbox"
                data-testid="license-assent"
                checked={checked}
                onChange={(event) => setChecked(event.target.checked)}
              />
              <span>I have read and expressly accept the Open Software License version 3.0.</span>
            </label>
            <p className="assent-note">
              Your versioned acknowledgement is stored on this device only. No telemetry or network
              request is made.
            </p>
            <button
              className="primary enter-workbench"
              data-testid="accept-license"
              disabled={!checked}
              onClick={() => {
                if (!checked) return;
                try {
                  localStorage.setItem(assentKey, "accepted");
                } catch {
                  setStorageNotice(
                    "License acknowledgement applies to this session only because local storage is unavailable.",
                  );
                }
                setAccepted(true);
              }}
            >
              Accept license & enter workbench →
            </button>
          </section>
        </main>
      )}
      {storageNotice && (
        <p className="storage-notice" role="status">
          {storageNotice}
        </p>
      )}
      <footer className="app-footer">
        <span>
          © 2026 Spiling contributors / Licensed under the Open Software License version 3.0
        </span>
        <div>
          <button className="text-button" onClick={() => setShowLicense(true)}>
            Read OSL-3.0
          </button>
          {accepted && (
            <button
              className="text-button"
              onClick={() => {
                if (
                  !window.confirm(
                    "Reviewing assent ends the engine session. Discard any unsaved project changes?",
                  )
                )
                  return;
                try {
                  localStorage.removeItem(assentKey);
                } catch {
                  /* Revoke in-memory assent regardless. */
                }
                setChecked(false);
                setAccepted(false);
              }}
            >
              Review assent
            </button>
          )}
        </div>
      </footer>
      {showLicense && <LicenseDialog onClose={() => setShowLicense(false)} />}
    </div>
  );
}
