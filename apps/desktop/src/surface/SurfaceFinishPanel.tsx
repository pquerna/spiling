/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useState, type FormEvent } from "react";
import type { FaceRef, SurfaceFinish, SurfaceScopeKind } from "@spiling/protocol";
import type { SurfaceFinishEdit, WorkbenchState } from "../useWorkbench.ts";
import "./surface.css";

const fields = ["depth_mm", "pitch_mm", "edge_margin_mm", "phase_mm"] as const;
const labels = [
  "Depth (mm)",
  "Pitch (mm)",
  "Protected edge margin (mm)",
  "Source-local phase (mm)",
];

export function SurfaceFinishPanel({
  state,
  inspect,
  edit,
}: {
  state: WorkbenchState;
  inspect(reference: FaceRef | null): Promise<void>;
  edit(command: SurfaceFinishEdit): Promise<void>;
}) {
  const [occurrenceId, setOccurrenceId] = useState<number | null>(null);
  const [scope, setScope] = useState<SurfaceScopeKind>("occurrence");
  const [kind, setKind] = useState<"vertical_ribs" | "smooth">("vertical_ribs");
  const [values, setValues] = useState(["0.1", "2", "1", "0"]);
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");
  const project = state.project;
  const reference = state.inspection?.reference ?? null;
  useEffect(() => {
    if (reference) setOccurrenceId(reference.occurrence_id);
  }, [reference]);
  useEffect(() => {
    setOccurrenceId(null);
    setMessage("");
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
  function apply(event: FormEvent) {
    event.preventDefault();
    if (!writable || !currentFace) return;
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
        Vertical ribs require a native certified full-height rectangular exterior vertical planar
        face, upright local Z and bed contact. No cylindrical, sloped, partial-height or
        hole-trimmed target faces. Existing protected holes in eligible bodies remain native.
        Compile checks bead-width/depth/pitch/margin, topology and nominal clearance limits;
        authoring alone does not certify eligibility.
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
              <option value="vertical_ribs">Vertical ribs</option>
              <option value="smooth">Smooth override</option>
            </select>
          </label>
          {kind === "vertical_ribs" && (
            <div className="surface-numbers">
              {fields.map((field, index) => (
                <label key={field}>
                  {labels[index]}
                  <input
                    data-testid={`surface-${field}`}
                    type="number"
                    step="any"
                    value={values[index]}
                    onChange={(event) =>
                      setValues((old) =>
                        old.map((value, i) => (i === index ? event.target.value : value)),
                      )
                    }
                  />
                </label>
              ))}
            </div>
          )}
          <button type="submit" data-testid="surface-apply" disabled={!writable || !currentFace}>
            Apply — append layer
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
              Layer {layer.layer_id}: {layer.finish.kind === "smooth" ? "Smooth" : "Vertical ribs"}{" "}
              · {layer.enabled ? "enabled" : "disabled"}
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
            },
            null,
            2,
          )}
        </pre>
      </details>
    </section>
  );
}
