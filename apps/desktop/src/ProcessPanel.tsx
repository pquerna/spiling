/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useState, type FormEvent } from "react";
import type {
  OccurrenceRecord,
  ProcessEdit,
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
}: {
  state: WorkbenchState;
  selected: OccurrenceRecord | null;
  edit(edit: ProcessEdit): Promise<void>;
}) {
  const [scopeKind, setScopeKind] = useState<SurfaceScopeKind>("occurrence");
  const [fraction, setFraction] = useState("0.20");
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
    setFraction(scopeFraction === undefined ? "0.20" : String(scopeFraction));
    setMessage("");
  }, [authority, selected?.occurrence_id, scopeKind, scopeFraction]);

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
  async function apply(event: FormEvent) {
    event.preventDefault();
    const value = Number(fraction);
    if (fraction.trim() === "" || !Number.isFinite(value) || value <= 0 || value > 1) {
      setMessage(
        "Sparse fraction must be greater than 0 and at most 1 (for example, 0.20 or 0.60).",
      );
      return;
    }
    await submit({ op: "set_sparse_fraction", scope: target(), fraction: value });
  }
  async function submit(change: ProcessEdit) {
    setWorking(true);
    setMessage("");
    try {
      await edit(change);
      setMessage(
        change.op === "reset"
          ? "Local layers in this scope removed. The engine's inherited value is shown below."
          : "Local sparse layer applied. Compile and fresh Verify are required before export.",
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
        Whole owned source volume only. This changes sparse fill, not source geometry, wall or skin
        recipes.
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
                onChange={(event) => setScopeKind(event.target.value as SurfaceScopeKind)}
              >
                <option value="occurrence">Occurrence — only this instance</option>
                <option value="definition">Definition — all instances of this source</option>
              </select>
            </label>
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
            Apply updates the engine-resolved local override, allocating a layer only when needed.
            Reset removes every layer in the chosen scope and reveals definition inheritance or the
            job recipe. Definition edits are shared; occurrence intent still wins. Project Undo /
            Redo restores edits.
          </p>
          <output className="project-controls" data-testid="process-effective" aria-live="polite">
            {state.stale || state.phase !== "running" ? (
              <strong>
                Captured process state — resume the native project for current effective values.
              </strong>
            ) : resolved ? (
              <>
                <strong>Effective sparse fraction for occurrence: {resolved.value.fraction}</strong>
                <span>
                  Selected {scopeKind} scope fraction: {scopeFraction}
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
          <details data-testid="process-layers">
            <summary>Source-bound layers ({relatedLayers.length})</summary>
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
                          Operation {operation.operation_id}: whole owned volume ·{" "}
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
