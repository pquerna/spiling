/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useState, type FormEvent } from "react";
import type {
  OccurrenceRecord,
  ProcessEdit,
  ProcessInterfacePermission,
  SurfaceScope,
  SurfaceScopeKind,
} from "@spiling/protocol";
import type { WorkbenchState } from "./useWorkbench.ts";

function scopeLabel(scope: SurfaceScope): string {
  return scope.kind === "definition"
    ? `Definition ${scope.definition_id} — all its instances`
    : `Occurrence ${scope.occurrence_id} — definition ${scope.expected_definition_id}`;
}

export function ProcessPanel({
  state,
  selected,
  edit,
  beadWidthMm,
}: {
  state: WorkbenchState;
  selected: OccurrenceRecord | null;
  edit(edit: ProcessEdit): Promise<void>;
  beadWidthMm: number | null;
}) {
  const [scopeKind, setScopeKind] = useState<SurfaceScopeKind>("occurrence");
  const [fraction, setFraction] = useState("0.20");
  const [mode, setMode] = useState<"whole" | "box">("whole");
  const [min, setMin] = useState<[string, string, string]>(["", "", ""]);
  const [max, setMax] = useState<[string, string, string]>(["", "", ""]);
  const [interfaceKind, setInterfaceKind] = useState<
    "separator_wall" | "horizontal_diaphragm"
  >("separator_wall");
  const [diaphragmThickness, setDiaphragmThickness] = useState("");
  const [separatorApproved, setSeparatorApproved] = useState(false);
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");
  const project = state.project;
  const resolved = project?.resolved_process.find(
    (row) =>
      !!selected &&
      row.occurrence_id === selected.occurrence_id &&
      row.definition_id === selected.definition_id,
  );
  const layers = project?.process_document.layers ?? [];
  const scopeFraction =
    scopeKind === "definition" ? resolved?.inherited.fraction : resolved?.value.fraction;
  const provenance = (scopeKind === "definition" ? resolved?.inherited : resolved?.value)
    ?.provenance;
  const winningLayer = provenance
    ? layers.find((layer) => layer.layer_id === provenance.layer_id)
    : undefined;
  const relatedLayers = selected
    ? layers.filter((layer) =>
        layer.scope.kind === "definition"
          ? layer.scope.definition_id === selected.definition_id
          : layer.scope.occurrence_id === selected.occurrence_id &&
            layer.scope.expected_definition_id === selected.definition_id,
      )
    : [];
  const available = state.phase === "running" && !state.busy && !state.stale && !working;
  const writable = available && !!project && !project.read_only && !!selected;
  const authority = `${state.hello?.session_id ?? ""}:${project?.project_id ?? ""}:${state.projectEpoch}`;

  useEffect(() => {
    if (mode === "whole")
      setFraction(scopeFraction === undefined ? "0.20" : String(scopeFraction));
    setMessage("");
  }, [authority, selected?.occurrence_id, scopeKind, scopeFraction, mode]);

  useEffect(() => {
    setMin(["", "", ""]);
    setMax(["", "", ""]);
    setInterfaceKind("separator_wall");
    setDiaphragmThickness("");
    setSeparatorApproved(false);
  }, [authority, selected?.occurrence_id, scopeKind]);

  function target(): SurfaceScope {
    if (!selected) throw new Error("Select a source instance first.");
    return scopeKind === "definition"
      ? { kind: "definition", definition_id: selected.definition_id }
      : {
          kind: "occurrence",
          occurrence_id: selected.occurrence_id,
          expected_definition_id: selected.definition_id,
        };
  }
  function permission(): ProcessInterfacePermission | null {
    if (!separatorApproved) return null;
    return interfaceKind === "separator_wall"
      ? { kind: "separator_wall" }
      : { kind: "horizontal_diaphragm", thickness_mm: Number(diaphragmThickness) };
  }
  async function apply(event: FormEvent) {
    event.preventDefault();
    const value = Number(fraction);
    if (fraction.trim() === "" || !Number.isFinite(value) || value <= 0 || value > 1) {
      setMessage(
        "Sparse fraction must be greater than 0 and at most 1 (for example, 0.20 or 0.60).",
      );
      return;
    }
    if (mode === "whole") {
      await submit({ op: "set_sparse_fraction", scope: target(), fraction: value });
      return;
    }
    const lower = min.map(Number) as [number, number, number];
    const upper = max.map(Number) as [number, number, number];
    if (
      [...min, ...max].some((bound) => bound.trim() === "") ||
      lower.some((bound, axis) => !Number.isFinite(bound) || !Number.isFinite(upper[axis]) || bound >= upper[axis]!)
    ) {
      setMessage("Enter six finite definition-local mm bounds with min < max on X, Y and Z.");
      return;
    }
    if (separatorApproved && interfaceKind === "horizontal_diaphragm") {
      const thickness = Number(diaphragmThickness);
      if (
        diaphragmThickness.trim() === "" ||
        !Number.isFinite(thickness) ||
        thickness <= 0
      ) {
        setMessage(
          "Horizontal diaphragm thickness must be a finite positive number of millimetres.",
        );
        return;
      }
    }
    await submit({
      op: "set_box_sparse_fraction",
      scope: target(),
      min: lower,
      max: upper,
      fraction: value,
      interface: permission(),
    });
  }
  async function submit(change: ProcessEdit) {
    setWorking(true);
    setMessage("");
    try {
      await edit(change);
      setMessage(
        change.op === "reset"
          ? "All local layers in this scope removed, including boxes. The engine's base inherited value is shown below."
          : change.op === "set_box_sparse_fraction" && change.interface === null
            ? "Box intent saved without permanent interface authorization. Compile blocks nonempty joins. No joined artifact is claimed."
            : "Local sparse intent applied. Compile and fresh Verify are required before export.",
      );
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setWorking(false);
    }
  }

  return (
    <section
      className="project-controls geometry-form"
      data-testid="process-panel"
      aria-label="Local sparse process"
    >
      <h2>Local sparse process</h2>
      <p>
        Author whole owned source volume or a bounded physical box mask. Sparse fraction is not a
        strength guarantee. Source geometry and global wall/skin recipes are unchanged.
      </p>
      {!selected ? (
        <p>Select an instance above to inspect or override its sparse fraction.</p>
      ) : (
        <>
          <strong data-testid="process-target">
            Instance {selected.occurrence_id} · definition {selected.definition_id}
          </strong>
          <form className="geometry-form" onSubmit={(event) => void apply(event)}>
            <label>
              Override scope
              <select
                data-testid="process-scope"
                value={scopeKind}
                disabled={!writable}
                onChange={(event) => setScopeKind(event.target.value as SurfaceScopeKind)}
              >
                <option value="occurrence">Occurrence — only this instance</option>
                <option value="definition">Definition — all instances of this source</option>
              </select>
            </label>
            <label>
              Physical region
              <select
                data-testid="process-region-mode"
                value={mode}
                disabled={!writable}
                onChange={(event) => setMode(event.target.value as "whole" | "box")}
              >
                <option value="whole">Whole owned volume</option>
                <option value="box">Bounded box — definition-local mm</option>
              </select>
            </label>
            {mode === "box" && (
              <>
                <p>
                  Bounds use the immutable source definition frame, before occurrence placement.
                  Min is inclusive; max is exclusive. This mask is not new source geometry.
                </p>
                {(["min", "max"] as const).map((side) => (
                  <fieldset key={side} disabled={!writable}>
                    <legend>{side === "min" ? "Minimum" : "Maximum"} bounds (mm)</legend>
                    <div className="numeric-triple">
                      {(side === "min" ? min : max).map((bound, axis) => (
                        <label key={axis}>
                          {"XYZ"[axis]} {side} (mm)
                          <input
                            data-testid={`process-box-${side}-${"xyz"[axis]}`}
                            type="number"
                            step="any"
                            required
                            value={bound}
                            onChange={(event) => {
                              const update = side === "min" ? setMin : setMax;
                              update((previous) =>
                                previous.map((value, index) => index === axis ? event.target.value : value) as [string, string, string],
                              );
                            }}
                          />
                        </label>
                      ))}
                    </div>
                  </fieldset>
                ))}
                <label>
                  Permanent interface permission
                  <select
                    data-testid="process-interface-kind"
                    value={interfaceKind}
                    disabled={!writable}
                    onChange={(event) =>
                      setInterfaceKind(
                        event.target.value as "separator_wall" | "horizontal_diaphragm",
                      )
                    }
                  >
                    <option value="separator_wall">Separator wall — X/Y density interfaces</option>
                    <option value="horizontal_diaphragm">
                      Horizontal diaphragm — source-Z density boundary
                    </option>
                  </select>
                </label>
                {interfaceKind === "horizontal_diaphragm" && (
                  <>
                    <label>
                      Horizontal diaphragm thickness (mm, greater than 0)
                      <input
                        data-testid="process-diaphragm-thickness"
                        type="number"
                        min="0"
                        step="any"
                        value={diaphragmThickness}
                        disabled={!writable}
                        onChange={(event) => setDiaphragmThickness(event.target.value)}
                      />
                    </label>
                    <p data-testid="process-diaphragm-explanation">
                      The plate is centered on the box's single source-Z density boundary:
                      source heights at boundary ± thickness / 2 are mapped through the
                      occurrence placement to actual world heights. Its physical thickness is
                      permanent deposited material, not removable support and not a strength
                      qualification. The panel does not infer whether geometry is eligible; the
                      fresh Compile reports native acceptance or refusal.
                    </p>
                  </>
                )}
                <label>
                  <input
                    data-testid="process-separator-approval"
                    type="checkbox"
                    checked={separatorApproved}
                    disabled={!writable}
                    onChange={(event) => setSeparatorApproved(event.target.checked)}
                  />
                  {interfaceKind === "separator_wall"
                    ? "I authorize a permanent separator wall at density interfaces."
                    : "I authorize a permanent horizontal diaphragm across the selected source-Z density boundary."}
                </label>
                <p data-testid="process-separator-thickness">
                  Permanent separator rib width / thickness: {beadWidthMm === null
                    ? "unavailable — apply a job recipe and refresh current native state"
                    : `${beadWidthMm} mm — current applied recipe bead width`}.
                  {" "}Physical allowed overlap with the separator: {beadWidthMm === null
                    ? "unavailable until a current applied recipe is present"
                    : `${beadWidthMm / 2} mm — half the recipe bead width`}.
                  {" "}Representation error is accounted for separately by the compiler.
                  {" "}This is permanent deposited material, not removable support or a strength
                  qualification. Leaving approval unchecked saves explicitly unauthorized intent;
                  Compile blocks nonempty joins.
                </p>
              </>
            )}
            <label>
              Sparse fraction (0 &lt; fraction ≤ 1)
              <input
                data-testid="process-fraction"
                type="number"
                min="0"
                max="1"
                step="any"
                required
                value={fraction}
                disabled={!writable}
                onChange={(event) => setFraction(event.target.value)}
              />
            </label>
            <div className="geometry-buttons">
              <button type="submit" data-testid="process-apply" disabled={!writable}>
                Apply
              </button>
              <button
                type="button"
                data-testid="process-reset"
                disabled={!writable}
                onClick={() => void submit({ op: "reset", scope: target() })}
              >
                Reset to inherit
              </button>
            </div>
          </form>
          <p>
            Whole Apply updates only the engine's whole-volume winner. Box Apply updates the
            exact-scope box operation, including changed bounds, or allocates once. Reset removes
            every layer in the chosen scope, including boxes. Only the engine resolves spatial
            precedence. Definition edits are shared. Project Undo / Redo restores edits.
          </p>
          <output className="project-controls" data-testid="process-effective" aria-live="polite">
            {state.stale || state.phase !== "running" ? (
              <strong>
                Captured process state — resume the native project for current effective values.
              </strong>
            ) : resolved ? (
              <>
                <strong>Base whole-owned sparse fraction for occurrence: {resolved.value.fraction}</strong>
                <span>Box masks are excluded from these base values; authored boxes are listed below.</span>
                <span>
                  Selected {scopeKind} base whole-owned fraction: {scopeFraction}
                </span>
                <span data-testid="process-inherited-recipe">
                  Inherited job recipe: {resolved.global_fraction}
                </span>
                <span data-testid="process-definition-inheritance">
                  Definition inheritance: {resolved.inherited.fraction}
                  {resolved.inherited.provenance
                    ? ` · layer ${resolved.inherited.provenance.layer_id} / operation ${resolved.inherited.provenance.operation_id}`
                    : " · inherited job recipe"}
                </span>
                <span data-testid="process-provenance">
                  {provenance
                    ? `Selected ${scopeKind} scope · winning layer ${provenance.layer_id} / operation ${provenance.operation_id}${winningLayer ? ` · ${scopeLabel(winningLayer.scope)}` : ""}`
                    : `Selected ${scopeKind} scope inherits the job recipe — no local winning operation`}
                </span>
                {provenance && (
                  <span>The inherited recipe remains unchanged beneath local overrides.</span>
                )}
              </>
            ) : (
              <span>
                Apply a manufacturing job recipe to resolve effective and inherited sparse values.
                Local layers are preserved independently of that recipe.
              </span>
            )}
          </output>
          <details open data-testid="process-layers">
            <summary>Authored source-bound layers ({relatedLayers.length})</summary>
            {relatedLayers.length === 0 ? (
              <p>No local layers for this source or instance.</p>
            ) : (
              <ol>
                {relatedLayers.map((layer) => (
                  <li key={layer.layer_id} data-process-layer-id={layer.layer_id}>
                    <strong>
                      Layer {layer.layer_id} — {layer.enabled ? "enabled" : "disabled"}
                    </strong>
                    <p>{scopeLabel(layer.scope)}</p>
                    <ul>
                      {layer.operations.map((operation) => (
                        <li key={operation.operation_id}>
                          Operation {operation.operation_id}:{" "}
                          {operation.selector.kind === "source_box_mm" ? (
                            <>
                              box in definition-local mm · min [{operation.selector.min.join(", ")}]
                              {" "}inclusive · max [{operation.selector.max.join(", ")}] exclusive ·{" "}
                              <strong>
                                {operation.interface === null
                                  ? "UNAUTHORIZED interface — Compile blocks nonempty joins"
                                  : operation.interface.kind === "separator_wall"
                                    ? "permanent separator wall authorized (separator_wall)"
                                    : `permanent horizontal diaphragm authorized (horizontal_diaphragm) · ${operation.interface.thickness_mm} mm centered on the source-Z boundary`}
                              </strong>
                              {" · "}
                            </>
                          ) : "whole owned volume · "}
                          {operation.action.kind === "replace"
                            ? `replace sparse fraction ${operation.action.value.fraction}`
                            : "erase this layer's contribution (reveal inheritance)"}
                        </li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ol>
            )}
          </details>
        </>
      )}
      {project?.read_only && (
        <p>Read-only snapshot: inspect values and layers; mutation is disabled.</p>
      )}
      {message && (
        <p role="status" data-testid="process-message">
          {message}
        </p>
      )}
    </section>
  );
}
