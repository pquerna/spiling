/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useMemo, useState } from "react";
import {
  MAX_REPLAY_DISPLAY_BYTES,
  MAX_REPLAY_EVENTS,
  MAX_PRINT_LAYERS,
  MAX_DEPOSITION_SEGMENTS,
  REPLAY_KIND_LABELS,
  REPLAY_LIFECYCLE_LAYER,
  type PreflightCheck,
} from "@spiling/protocol";
import type { RectilinearAxisPolicy, SeamPolicy } from "@spiling/protocol";
import type { ManufacturingHost } from "../useWorkbench.ts";
import { scalarFields, policyScalarFields } from "./draft.ts";
import type { ManufacturingController } from "./useManufacturing.ts";
import { nextManufacturingStep } from "./workflow.ts";
import "./manufacturing.css";

export function ManufacturingPanel({
  host,
  manufacturing,
  cancel,
  diagnosticImport,
}: {
  host: ManufacturingHost;
  manufacturing: ManufacturingController;
  cancel: () => void;
  diagnosticImport: (source: string) => Promise<void>;
}) {
  const [exportPath, setExportPath] = useState("manufacturing-export");
  const [intentPath, setIntentPath] = useState(
    "fixtures/manufacturing/complete-sparse.intent.json",
  );
  const [bedOccurrence, setBedOccurrence] = useState("");
  const [diagnosticSource, setDiagnosticSource] = useState("");
  const { status, draft, inspection, fresh, available, writable } = manufacturing;
  const result = inspection?.result,
    replay = inspection?.replay;
  const sparseMetrics = useMemo(() => {
    if (!replay || !inspection) return null;
    const schedule = new Map(inspection.layers.map(row => [row.index, row.lower_z_mm]));
    let strokes = 0, combined = 0, min = Infinity, max = 0;
    for (let index = 0; index < replay.eventCount; index++) {
      if (replay.kind(index) !== 5) continue;
      const association = replay.association(index)!;
      const height = association.physicalZMm[1] - association.physicalZMm[0];
      strokes++;
      if (association.physicalZMm[0] < (schedule.get(replay.layer(index)) ?? -Infinity)) combined++;
      min = Math.min(min, height);
      max = Math.max(max, height);
    }
    return { strokes, combined, min: strokes ? min : 0, max };
  }, [replay, inspection]);
  const wallMetrics = useMemo(() => {
    if (!replay) return null;
    let strokes = 0, min = Infinity, max = 0;
    for (let index = 0; index < replay.eventCount; index++) {
      const association = replay.association(index);
      if (association?.regionalOwner !== "wall") continue;
      strokes++;
      min = Math.min(min, association.beadWidthMm);
      max = Math.max(max, association.beadWidthMm);
    }
    return strokes ? { strokes, min, max } : null;
  }, [replay]);
  const sparseBands = useMemo(() => inspection?.layers.filter(row => row.sparse_band) ?? [], [inspection]);
  const diaphragms = useMemo(() => {
    const plates = new Map<string, NonNullable<typeof inspection>["layers"][number]["diaphragms"][number]>();
    for (const row of inspection?.layers ?? []) for (const plate of row.diaphragms) {
      const key = `${plate.occurrence_id}:${plate.provenance.operation_id}`;
      const existing = plates.get(key);
      if (existing) existing.nominal_path_volume_mm3 += plate.nominal_path_volume_mm3;
      else plates.set(key, { ...plate });
    }
    return [...plates.values()];
  }, [inspection]);
  const selectedSparseBand = manufacturing.layer === null ? null : sparseBands.find(row =>
    row.sparse_band!.first_layer <= manufacturing.layer! && manufacturing.layer! <= row.index);
  const emptyLayer =
    manufacturing.layer !== null &&
    inspection?.layers.find((row) => row.index === manufacturing.layer)?.deposition_segments === 0;
  const event =
    replay && manufacturing.eventCount > 0 && !emptyLayer ? manufacturing.eventCount - 1 : null;
  const job = host.state.activeJob;
  const nextStep = nextManufacturingStep(host.state, manufacturing);
  async function runNextAction() {
    switch (nextStep.action) {
      case "load_intent": return manufacturing.selectIntent();
      case "apply": return manufacturing.apply();
      case "reload": return manufacturing.reloadDraft();
      case "preflight": return manufacturing.preflightIntent();
      case "compile": return manufacturing.compile();
      case "inspect": return manufacturing.inspect("report_and_motion");
      case "export": return manufacturing.exportBundle();
    }
  }
  return (
    <section
      className="manufacturing-controls geometry-controls"
      aria-label="Manufacturing review"
      data-testid="manufacturing-panel"
    >
      <h2>Manufacturing review</h2>
      <strong>Software-only — not machine-ready</strong>
      <section className="manufacturing-next-step" data-testid="manufacturing-next-step"
        data-state={nextStep.id} aria-labelledby="manufacturing-next-heading">
        <small>Next step · project revision {host.state.project?.revision ?? "—"}
          {host.state.project?.read_only ? " · read-only" : ""}</small>
        <h3 id="manufacturing-next-heading" aria-live="polite">{nextStep.title}</h3>
        <p>{nextStep.detail}</p>
        {nextStep.action && (
          <button data-testid="manufacturing-next-action" disabled={!available}
            onClick={() => { void runNextAction(); }}>{nextStep.label}</button>
        )}
        {nextStep.href && <a href={nextStep.href}>{nextStep.label}</a>}
      </section>
      {manufacturing.failure && (
        <section id="manufacturing-operation-failure" role="alert" className="manufacturing-failure">
          <h3>{manufacturing.failure.action === "inspect" ? "Verification" : manufacturing.failure.action} operation stopped</h3>
          <p>No completed result is claimed. Review the native diagnostic, then correct the input
            or placement, or explicitly retry using the operation controls.</p>
          <details>
            <summary>Full operation diagnostic</summary>
            <pre data-testid="manufacturing-message">{manufacturing.failure.message}</pre>
          </details>
        </section>
      )}
      <p>
        Review explicit requirements, compile an immutable bundle, and independently replay the
        actual emitted program. No Print or machine connection is provided.
      </p>
      <p>
        Independent replay admission limits: {MAX_REPLAY_DISPLAY_BYTES / (1024 * 1024)} MiB packed
        bytes,
        {MAX_REPLAY_EVENTS.toLocaleString()} total events,{" "}
        {MAX_DEPOSITION_SEGMENTS.toLocaleString()} deposition and travel events each,{" "}
        {MAX_PRINT_LAYERS} layers. Display shares the existing combined 128 MiB active/staged
        geometry/section/replay budget; these limits do not establish printer support.
      </p>
      {host.state.project?.read_only && (
        <p>
          Read-only snapshot: fresh Verify, inspection and Export are permitted; Apply and Compile
          are disabled. Saved verification flags are never accepted as a fresh inspection.
        </p>
      )}
      {host.state.diagnosticSession && (
        <p role="status" data-testid="manufacturing-diagnostic-label">
          Explicit native diagnostic session — no GPU display, picking, visual acceptance or
          hardware claim.
        </p>
      )}
      {host.state.diagnosticSession && host.state.runtime?.geometry_debug_enabled && (
        <div className="geometry-buttons">
          <button
            data-testid="manufacturing-diagnostic-import"
            disabled={!writable || !!host.state.occurrences.length}
            onClick={() => {
              void diagnosticImport("box-mm.step");
            }}
          >
            Admit box-mm STEP fixture through native tokens
          </button>
          <button
            data-testid="manufacturing-diagnostic-cylinder-import"
            disabled={!writable || !!host.state.occurrences.length}
            onClick={() => {
              void diagnosticImport("independent-exporters/occt-cylinder-mm.step");
            }}
          >
            Admit full-face OCCT cylinder fixture through native tokens
          </button>
          <label>
            Diagnostic STEP source relative to the native fixture root
            <input
              data-testid="manufacturing-diagnostic-source"
              value={diagnosticSource}
              disabled={!writable || !!host.state.occurrences.length}
              onChange={(event) => setDiagnosticSource(event.target.value)}
            />
          </label>
          <button
            data-testid="manufacturing-diagnostic-source-import"
            disabled={!writable || !!host.state.occurrences.length || !diagnosticSource}
            onClick={() => { void diagnosticImport(diagnosticSource); }}
          >
            Diagnostic native token import — no viewport or hardware claim
          </button>
        </div>
      )}
      <button
        data-testid="manufacturing-load-intent"
        disabled={!writable}
        onClick={() => {
          void manufacturing.selectIntent();
        }}
      >
        Load full ManufacturingIntent JSON…
      </button>
      <p>
        Native dialog only; inert JSON limited to 32 KiB and validated in Rust. No TypeScript
        execution or renderer file access.
      </p>
      {host.state.runtime?.geometry_debug_enabled && (
        <details>
          <summary>Confined inert intent fixture admission</summary>
          <input
            data-testid="manufacturing-intent-path"
            value={intentPath}
            onChange={(event) => setIntentPath(event.target.value)}
          />
          <button
            disabled={!writable}
            data-testid="manufacturing-debug-intent"
            onClick={() => {
              void manufacturing.selectIntent(intentPath);
            }}
          >
            Load confined JSON
          </button>
        </details>
      )}
      <button
        data-testid="manufacturing-select-software"
        disabled={!writable}
        onClick={manufacturing.selectSoftwareIntent}
      >
        Select bundled diagnostic body-only software reference intent
      </button>
      {!draft && (
        <p>
          No intent selected. Selection is explicit; there is no default printer permission or
          profile.
        </p>
      )}
      {draft && (
        <>
          <h3>{draft.intent.printer.name}</h3>
          <p>
            {draft.intent.printer.id} / {draft.intent.printer.revision}; {draft.intent.recipe.id} /{" "}
            {draft.intent.recipe.revision}
          </p>
          <p data-testid="manufacturing-workflow">
            {draft.intent.printer.workflow.kind === "diagnostic_body_only"
              ? "Diagnostic body-only program — no executable startup or shutdown lifecycle."
              : "Complete Marlin 2 single-extruder job — software-verified lifecycle, physically unqualified."}{" "}
            Dialect: {draft.intent.printer.capabilities.output_dialect}.
          </p>
          {draft.intent.printer.workflow.kind === "complete_marlin" && (
            <details>
              <summary>
                Explicit job declarations and provenance — not physical qualification
              </summary>
              <p>
                Machine, firmware, configuration and calibration hashes are supplied declarations.
                Recipe edits preserve this complete job unchanged; no machine values are invented.
                Rust validates the full intent on Apply. Homing trajectories are firmware-owned and
                never displayed as interpolated paths.
              </p>
              <pre data-testid="manufacturing-job-declarations">
                {JSON.stringify(draft.intent.printer.workflow.job, null, 2)}
              </pre>
            </details>
          )}
          <p>
            Frame: {draft.intent.printer.coordinate_frame}. Nozzle{" "}
            {draft.intent.printer.nozzle_diameter_mm} mm; maximum feed{" "}
            {draft.intent.printer.max_feed_mm_s} mm/s; nominal flow limit{" "}
            {draft.intent.printer.max_volumetric_flow_mm3_s} mm³/s.
          </p>
          <fieldset disabled={!writable} className="manufacturing-draft">
            <legend>Planar recipe scalars — physical-thickness skins and interior fill</legend>
            {scalarFields.map(([key, label]) => (
              <label key={key}>
                {label}
                <input
                  type="number"
                  step={key === "perimeter_count" ? "1" : "any"}
                  data-testid={`manufacturing-${key}`}
                  value={draft.values[key]}
                  onChange={(event) => manufacturing.editScalar(key, event.target.value)}
                />
              </label>
            ))}
            <p>Automatic source-associated walls choose local widths/counts within the captured
              software envelope; preferred width and contour depth are needs, not an algorithm
              switch or a promise of complete closed loops. Finite transitions, source/void
              protection and actual-command qualification remain mandatory.</p>
            {(["minimum", "maximum"] as const).map(bound => <label key={bound}>
              {bound === "minimum" ? "Minimum" : "Maximum"} software bead width (mm)
              <input type="number" step="any" data-testid={`manufacturing-width-${bound}`}
                value={draft.widthLimits[bound]}
                onChange={event => manufacturing.editWidthLimit(bound, event.target.value)} />
            </label>)}
            <label>
              Sparse infill schedule
              <select data-testid="manufacturing-sparse-infill"
                value={draft.sparseInfillKind}
                onChange={(event) => manufacturing.editSparseInfill(event.target.value as "fine" | "combine_eligible", draft.sparseMaxHeight)}>
                <option value="fine">Fine — every eligible schedule layer</option>
                <option value="combine_eligible">Combine eligible — fixed-width sparse only</option>
              </select>
            </label>
            {draft.sparseInfillKind === "combine_eligible" && <label>
              Maximum associated sparse stroke height (mm)
              <input type="number" step="any" data-testid="manufacturing-sparse-max-height"
                value={draft.sparseMaxHeight}
                onChange={(event) => manufacturing.editSparseInfill(draft.sparseInfillKind, event.target.value)} />
            </label>}
            <p>The global fine schedule, walls and skins remain protected. Eligible SparseFill
              spans may combine; only a genuinely anchored coarse gap becomes regional Bridge.
              Actual band-base/landing proofs and retained-fine reasons remain mandatory.
              Limits are software envelopes, not physical qualification.</p>
            {policyScalarFields(draft.intent).map(([key, label]) => (
              <label key={key}>
                {label}
                <input
                  type="number"
                  step={key.includes("fan_") ? "1" : "any"}
                  data-testid={`manufacturing-${key.replaceAll(".", "-")}`}
                  value={draft.policyValues[key]}
                  onChange={(event) => manufacturing.editPolicyScalar(key, event.target.value)}
                />
              </label>
            ))}
          </fieldset>
          <fieldset disabled={!writable} className="manufacturing-draft">
            <legend>Perimeter starts and rectilinear orientation — placed build/machine XY</legend>
            <label>
              Perimeter seam
              <select
                data-testid="manufacturing-seam-policy"
                value={draft.seamPolicyKind}
                onChange={(event) => manufacturing.editSeamPolicy(event.target.value as SeamPolicy["kind"])}
              >
                <option value="nearest">Nearest</option>
                <option value="preferred_build_direction">Preferred build direction</option>
              </select>
            </label>
            {draft.seamPolicyKind === "preferred_build_direction" && (
              <>
                <p>
                  Enter a finite nonzero XY direction in placed build/machine coordinates, not
                  source or camera coordinates. Apply normalizes it to a unit direction.
                </p>
                {([0, 1] as const).map((axis) => (
                  <label key={axis}>
                    Preferred build direction {axis === 0 ? "X" : "Y"}
                    <input
                      type="number"
                      step="any"
                      data-testid={`manufacturing-seam-direction-${axis === 0 ? "x" : "y"}`}
                      value={draft.preferredDirection[axis]}
                      onChange={(event) => manufacturing.editPreferredDirection(axis, event.target.value)}
                    />
                  </label>
                ))}
              </>
            )}
            <label>
              Rectilinear axis
              <select
                data-testid="manufacturing-rectilinear-axis"
                value={draft.intent.recipe.rectilinear_axis}
                onChange={(event) => manufacturing.editRectilinearAxis(event.target.value as RectilinearAxisPolicy)}
              >
                <option value="x">X</option>
                <option value="y">Y</option>
                <option value="alternating_xy">Alternating X/Y</option>
                <option value="alternating_yx">Alternating Y/X</option>
              </select>
            </label>
            <label>
              Top-skin rectilinear axis
              <select
                data-testid="manufacturing-top-skin-axis"
                value={draft.intent.recipe.top_skin_axis}
                onChange={(event) => manufacturing.editTopSkinAxis(event.target.value as RectilinearAxisPolicy)}
              >
                <option value="x">X</option>
                <option value="y">Y</option>
                <option value="alternating_xy">Alternating X/Y</option>
                <option value="alternating_yx">Alternating Y/X</option>
              </select>
            </label>
            <p>
              Preferred direction chooses perimeter starts at existing vertices; it does not add
              seam geometry. Rectilinear axes are in placed build/machine XY. Top-skin orientation
              is independent of regional sparse fill. Bridge and support paths can retain admitted
              orientation exceptions.
            </p>
          </fieldset>
          <p>
            Native event-aligned layers can have nonnominal final or shelf thickness; bead/flow uses
            actual thickness. Physical-thickness skins and sparse interior preserve object
            ownership. Bridge and breakaway support policies are explicit bounded software
            declarations, not physical-printability or collision certification.
          </p>
          <details>
            <summary>First-layer, cooling, retraction, bridge and support policies</summary>
            <pre data-testid="manufacturing-process-policy">
              {JSON.stringify(
                { process: draft.intent.recipe.process, supports: draft.intent.recipe.supports },
                null,
                2,
              )}
            </pre>
            <p>
              Scalar edits preserve every machine, configuration, calibration and process
              declaration. Apply revalidates the entire intent natively. Fan PWM is commanded state,
              not measured cooling.
            </p>
          </details>
          {manufacturing.conflict && (
            <p role="alert">
              The authoritative intent changed while this draft was edited. Reload the current
              intent before applying. The local draft has not been silently discarded.
            </p>
          )}
          <div className="geometry-buttons">
            <button
              data-testid="manufacturing-apply"
              disabled={!writable || !manufacturing.changed || manufacturing.conflict}
              onClick={() => {
                void manufacturing.apply();
              }}
            >
              Apply full intent once
            </button>
            <button
              data-testid="manufacturing-reset-draft"
              disabled={!available}
              onClick={manufacturing.reloadDraft}
            >
              Reload current intent / discard draft
            </button>
          </div>
          <p>
            Draft changes are transient until Apply. Failed Apply retains the draft. Project Undo /
            Redo / Save / Reopen use the existing Core controls, not a manufacturing-only history.
          </p>
        </>
      )}
      <fieldset
        disabled={!writable || !status?.intent || manufacturing.changed || host.state.stale}
      >
        <legend>Prepare an occurrence on the declared native bed</legend>
        <select
          data-testid="manufacturing-bed-occurrence"
          value={bedOccurrence}
          onChange={(event) => {
            setBedOccurrence(event.target.value);
            manufacturing.clearBedProposal();
          }}
        >
          <option value="">Select occurrence explicitly</option>
          {host.state.occurrences.map((row) => (
            <option key={row.occurrence_id} value={row.occurrence_id}>
              Occurrence {row.occurrence_id} / definition {row.definition_id}
            </option>
          ))}
        </select>
        <button
          data-testid="manufacturing-place-bed"
          disabled={!bedOccurrence}
          onClick={() => {
            void manufacturing.prepareBed(Number(bedOccurrence), false);
          }}
        >
          Prepare place-on-bed proposal
        </button>
        <button
          data-testid="manufacturing-center-bed"
          disabled={!bedOccurrence}
          onClick={() => {
            void manufacturing.prepareBed(Number(bedOccurrence), true);
          }}
        >
          Prepare center-and-place proposal
        </button>
        <p>
          Read-only native contact analysis supports the current arbitrary rigid orientation,
          preserves its quaternion and uses original-source native bounds, never display meshes.
          Review first; only explicit Apply commits one revision-bound Core pose transaction.
          Software contact evidence is not physical printability or machine qualification.
        </p>
      </fieldset>
      {manufacturing.bedProposal && (
        <section id="manufacturing-bed-review" aria-label="Review native bed proposal" data-testid="manufacturing-bed-review">
          <h3>Review native bed pose — not yet applied</h3>
          {manufacturing.bedProposalStale && (
            <p role="alert" data-testid="manufacturing-bed-stale">
              Proposal invalidated by changed scene, project, intent or session. Clear or prepare
              again; this captured pose cannot be applied.
            </p>
          )}
          <p>
            Occurrence {manufacturing.bedProposal.occurrence_id}; captured project revision{" "}
            {manufacturing.bedProposal.project_revision}, scene revision{" "}
            {manufacturing.bedProposal.scene_revision}.
          </p>
          <dl>
            <dt>Original translation (mm)</dt>
            <dd>{manufacturing.bedProposal.original_pose.translation_mm.join(", ")}</dd>
            <dt>Proposed translation (mm)</dt>
            <dd>{manufacturing.bedProposal.pose.translation_mm.join(", ")}</dd>
            <dt>Original orientation quaternion (X, Y, Z, W)</dt>
            <dd>{manufacturing.bedProposal.original_pose.rotation_xyzw.join(", ")}</dd>
            <dt>Proposed orientation quaternion (X, Y, Z, W), preserved</dt>
            <dd>{manufacturing.bedProposal.pose.rotation_xyzw.join(", ")}</dd>
            <dt>Native contact tolerance / maximum certified gap (mm)</dt>
            <dd>{manufacturing.bedProposal.contact_tolerance_mm} / {manufacturing.bedProposal.maximum_contact_gap_mm}</dd>
            <dt>Declared bed envelope (mm, minimum → maximum)</dt>
            <dd>{manufacturing.bedProposal.envelope_mm.min.join(", ")} → {manufacturing.bedProposal.envelope_mm.max.join(", ")}</dd>
            <dt>Original native minimum support bracket (mm)</dt>
            <dd>{manufacturing.bedProposal.minimum_lower_mm.join(", ")} → {manufacturing.bedProposal.minimum_upper_mm.join(", ")}</dd>
            <dt>Original native maximum support bracket (mm)</dt>
            <dd>{manufacturing.bedProposal.maximum_lower_mm.join(", ")} → {manufacturing.bedProposal.maximum_upper_mm.join(", ")}</dd>
          </dl>
          <button
            data-testid="manufacturing-bed-apply"
            disabled={!writable || host.state.stale || manufacturing.bedProposalStale}
            onClick={() => { void manufacturing.applyBed(); }}
          >
            Apply reviewed bed pose once
          </button>
          <button
            data-testid="manufacturing-bed-clear"
            disabled={!available}
            onClick={manufacturing.clearBedProposal}
          >
            Clear bed proposal without editing project
          </button>
        </section>
      )}
      <div className="geometry-buttons">
        <button
          data-testid="manufacturing-preflight"
          disabled={!available || !status?.intent || manufacturing.changed}
          onClick={() => {
            void manufacturing.preflightIntent();
          }}
        >
          Run input Preflight
        </button>
        <button
          data-testid="manufacturing-compile"
          disabled={!writable || !status?.intent || manufacturing.changed}
          onClick={() => {
            void manufacturing.compile();
          }}
        >
          Compile software bundle
        </button>
        <button
          data-testid="manufacturing-verify-report"
          disabled={!available || !status?.artifact}
          onClick={() => {
            void manufacturing.inspect("report_only");
          }}
        >
          Fresh report only
        </button>
        <button
          data-testid="manufacturing-verify-motion"
          disabled={!available || !status?.artifact}
          onClick={() => {
            void manufacturing.inspect("report_and_motion");
          }}
        >
          Fresh report and motion
        </button>
      </div>
      {manufacturing.preflight && (
        <section id="manufacturing-preflight-review" data-testid="manufacturing-preflight-report">
          <h3>Preflight — input admission, not Compile success</h3>
          <p>
            Revision {manufacturing.preflight.input_revision};
            {manufacturing.preflight.input_revision === host.state.project?.revision
              ? " current"
              : " stale — rerun for current input"}
            . Disposition: <strong>{manufacturing.preflight.disposition}</strong>.
            Required unresolved evidence is not passed. Software only; physical qualification is absent.
          </p>
          <h4>Required input admission</h4>
          <PreflightChecks checks={manufacturing.preflight.checks.filter(check => check.required)} />
          <details>
            <summary>Compile-only analysis and other advisories</summary>
            <p>These checks are not input-admission authority. Pending, partial or failed optional
              analysis does not itself refuse admitted input; Compile must establish its own result.</p>
            <PreflightChecks checks={manufacturing.preflight.checks.filter(check => !check.required)} />
          </details>
        </section>
      )}
      {job && manufacturing.working && (
        <div data-testid="manufacturing-progress" role="status">
          {job.status}: {job.stage} — {job.completed_units} / {job.total_units}
          <progress max={Math.max(1, job.total_units)} value={job.completed_units} />
          <button
            data-testid="manufacturing-cancel"
            disabled={job.status === "cancelling"}
            onClick={cancel}
          >
            Cancel native operation
          </button>
        </div>
      )}
      {manufacturing.admittingReplay && !job && (
        <div role="status" data-testid="manufacturing-transfer-progress">
          Immutable replay transfer, validation or GPU staging in progress.
          <button data-testid="manufacturing-cancel-transfer" onClick={manufacturing.cancelPreview}>
            Cancel preview admission
          </button>
        </div>
      )}
      {!manufacturing.failure && (
        <output data-testid="manufacturing-message" role="status">
          {manufacturing.message}
        </output>
      )}
      {status?.artifact && (
        <details open>
          <summary>Server-selected immutable bundle</summary>
          <p>
            Bundle SHA-256: <code>{status.artifact.hash}</code>
          </p>
          <p>
            Input SHA-256: <code>{status.artifact.input_hash}</code>
          </p>
          <p>
            {status.artifact.byte_count} B; {status.artifact.summary.layers} layers;
            {status.artifact.summary.paths} paths; {status.artifact.summary.deposition_segments}{" "}
            deposition segments.
          </p>
          {status.artifact.summary.process_resolution.length > 0 && (
            <section data-testid="manufacturing-process-resolution">
              <h4>Compiled physical process regions</h4>
              <ul>
                {status.artifact.summary.process_resolution.map(record => (
                  <li key={`${record.occurrence_id}:${record.provenance.operation_id}`}>
                    Body {record.occurrence_id}, operation {record.provenance.operation_id}:{" "}
                    <strong>{record.status === "no_eligible_interior" ? "NoEligibleInterior" : record.status === "no_density_change" ? "NoDensityChange" : "Applied"}</strong>
                    {record.status === "no_eligible_interior"
                      ? " — the effective selection owns no eligible finished interior; no separator or fill was invented."
                      : record.status === "no_density_change"
                        ? " — selected interior already has this fraction; ordinary hatch phase is unchanged and no separator is added."
                        : " — inspect actual separator_wall motion and the adjoining density rows in fresh replay."}
                  </li>
                ))}
              </ul>
              <p>Separator deposition is a separate permanent role, never ordinary fill or an automatically authorized Bridge.
                This software contact check is not measured bonding strength or machine readiness.</p>
            </section>
          )}
          <dl data-testid="manufacturing-native-geometry-coverage">
            <dt>Actual occurrence-query / slab coverage evidence</dt>
            <dd>{status.artifact.summary.native_geometry.evidence}</dd>
            <dt>Accepted disjoint / invariant-union / native-interface-complex layers</dt>
            <dd>
              {status.artifact.summary.native_geometry.source_feature_coverage.disjoint_source_layer_count} /{" "}
              {status.artifact.summary.native_geometry.source_feature_coverage.invariant_source_union_layer_count} /{" "}
              {status.artifact.summary.native_geometry.source_feature_coverage.native_interface_complex_layer_count}
            </dd>
            <dt>Owned critical source slabs</dt>
            <dd>
              {status.artifact.summary.native_geometry.source_feature_coverage.owned_critical_slab_count}
            </dd>
            <dt>Required / achieved native shape error (mm)</dt>
            <dd>
              {status.artifact.summary.native_geometry.requested_shape_error_mm} /{" "}
              {status.artifact.summary.native_geometry.achieved_shape_error_mm}
            </dd>
            <dt>Height analyses / slabs / events / geometry work</dt>
            <dd>
              {status.artifact.summary.native_geometry.height_analysis_count} /{" "}
              {status.artifact.summary.native_geometry.slab_count} /{" "}
              {status.artifact.summary.native_geometry.event_count} /{" "}
              {status.artifact.summary.native_geometry.geometry_work}
            </dd>
            <dt>Skin coverage scope</dt>
            <dd>{status.artifact.summary.native_geometry.skin_coverage}</dd>
            <dt>Support clearance scope</dt>
            <dd>{status.artifact.summary.native_geometry.support_clearance}</dd>
          </dl>
          <p>
            Native evidence covers the actual occurrence queries and slabs used by this schedule,
            not every possible query. Source feature composition counts accepted disjoint-source,
            invariant-union and native-interface-complex layers separately; a proof for one
            interval does not make the entire plan invariant. Endpoint-owned contributions
            bind certified adjacent closed strata, not an empty-to-contact distance certificate.
            Per-occurrence certificates alone do not certify a global composed union boundary.
            Requested and achieved bounds are separate from sampled image resolution below.
            Skin coverage measures distance in scheduled realized material D. Support clearance
            uses continuous target outer occupancy, not deposition anchors or proof of physical
            collision freedom. Actual deposited strokes alone establish anchors.
          </p>
          <small>
            Compiled summary is not a fresh actual-program report. Revision bookkeeping is not
            content identity.
          </small>
        </details>
      )}
      {status?.artifact && (
        <section className="surface-resolution-advisory" data-testid="manufacturing-surface-resolution">
          <h3>Sampled image feature resolution — advisory</h3>
          <dl>
            <dt>Evaluated witnesses</dt>
            <dd>{status.artifact.summary.surface_resolution.evaluated_witnesses}</dd>
            <dt>Resolved / ambiguous / below nominal bead resolution / unavailable</dt>
            <dd>
              {status.artifact.summary.surface_resolution.resolved} /{" "}
              {status.artifact.summary.surface_resolution.ambiguous} /{" "}
              {status.artifact.summary.surface_resolution.below_nominal_resolution} /{" "}
              {status.artifact.summary.surface_resolution.unavailable}
            </dd>
            <dt>Sampled diagnostic coverage</dt>
            <dd>{status.artifact.summary.surface_resolution.coverage_complete ? "Complete for admitted witnesses" : "Incomplete — unavailable or work-limited samples"}</dd>
          </dl>
          <p>
            Bounded first-wall distance samples and nominal bead width can reveal lost fine details.
            Correct erosion can remove small features. This is not universal Hausdorff coverage,
            physical accuracy or guaranteed feature survival, and is separate from fresh independent
            emitted-program verification below. Field, chord, retention, Z error and support failures
            remain hard errors, never downgraded by this advisory.
          </p>
        </section>
      )}
      {result && (
        <div
          className={fresh ? "manufacturing-inspection" : "manufacturing-inspection historical"}
          data-testid="manufacturing-inspection"
          data-active={fresh ? "true" : "false"}
        >
          <h3>
            {fresh ? "Fresh independent inspection" : "Historical / stale inspection — inactive"}
          </h3>
          {!fresh && (
            <p>
              No active replay or export authority. Verify the server-selected bundle in this
              session again.
            </p>
          )}
          <p>
            Verified: {String(result.report.verified)}. Bundle {result.record.hash}; input{" "}
            {result.record.input_hash}.
          </p>
          {diaphragms.length > 0 && (
            <section data-testid="manufacturing-diaphragms">
              <h4>Permanent supported transition plates</h4>
              <ul>{diaphragms.map(plate => (
                <li key={`${plate.occurrence_id}:${plate.provenance.operation_id}`}>
                  Body {plate.occurrence_id}, operation {plate.provenance.operation_id}: source-Z plane{" "}
                  {plate.source_plane_mm} mm, centered thickness {plate.requested_thickness_mm} mm.
                  {" "}Actual world slab [{plate.physical_z_mm.join(", ")}] mm.
                  {" "}Reserved interior cross-section {plate.area_mm2.toFixed(3)} mm².
                  {" "}Nominal path material cost: {plate.nominal_path_volume_mm3.toFixed(3)} mm³
                  {" "}(explicit bead dimensions and Bridge flow; not measured deposited plastic).
                </li>
              ))}</ul>
              <p>SolidFill / genuine Bridge strokes retain explicit permanent plate ownership.
                The plate preserves native holes and exterior walls/skins; its first layer and
                upper landing use actual lower strokes, never hidden temporary scaffolding.
                Inspect the plate layer with source bodies hidden. These are verified nominal
                program dimensions, not measured thickness or qualified bonding strength.</p>
            </section>
          )}
          <h4>Independent verifier coverage</h4>
          <ul>
            {result.report.coverage.map((item, index) => (
              <li key={index}>{item}</li>
            ))}
          </ul>
          <h4>Exclusions and limitations</h4>
          <ul>
            {result.report.limitations.map((item, index) => (
              <li key={index}>{item}</li>
            ))}
          </ul>
          <dl>
            <dt>Object deposition / travel events</dt>
            <dd>
              {result.report.deposition_segments} / {result.report.travel_segments}
            </dd>
            <dt>Nominal object deposited volume from actual paths and layer heights</dt>
            <dd>{result.report.deposited_volume_mm3} mm³</dd>
            <dt>Object filament length</dt>
            <dd>{result.report.filament_length_mm} mm</dd>
            <dt>Support deposition events / volume / filament (separate from object and prime)</dt>
            <dd>
              {result.report.support_deposition_segments} /{" "}
              {result.report.support_deposited_volume_mm3} mm³ /{" "}
              {result.report.support_filament_length_mm} mm
            </dd>
            <dt>Retractions / withdrawn filament (recovery is not deposition)</dt>
            <dd>
              {result.report.retraction_count} / {result.report.retracted_filament_mm} mm
            </dd>
            <dt>Lifecycle motion events</dt>
            <dd>{result.report.lifecycle_motion_count}</dd>
            <dt>Priming filament length (separate from object)</dt>
            <dd>{result.report.priming_filament_length_mm} mm</dd>
            <dt>Maximum position deviation</dt>
            <dd>{result.report.max_position_error_mm} mm</dd>
            <dt>Maximum extrusion deviation</dt>
            <dd>{result.report.max_extrusion_error_mm} mm</dd>
          </dl>
          <p>
            Surface ribs and mapped image relief add material before ordinary path/support planning,
            not an edited BREP or physical grip qualification. Exported plan coverage separates source section area and surface
            added area; their sum is realized material area. Added geometric volume is construction
            accounting, not measured extrusion. Inspect fresh actual emitted replay for changed
            paths.
          </p>
          <details>
            <summary>Actual native layer thickness and section heights (mm)</summary>
            <table data-testid="manufacturing-layer-accounting">
              <thead>
                <tr>
                  <th>Layer</th>
                  <th>Z</th>
                  <th>Actual thickness</th>
                  <th>Native section Z</th>
                  <th>Sparse-band admission ending here</th>
                </tr>
              </thead>
              <tbody>
                {inspection?.layers.map((row) => (
                  <tr key={row.index}>
                    <td>{row.index}</td>
                    <td>{row.z_mm}</td>
                    <td>{row.z_mm - row.lower_z_mm}</td>
                    <td>{row.section_z_mm}</td>
                    <td>{row.sparse_band && <>
                      Layers {row.sparse_band.first_layer}–{row.index}: combined regional owners{" "}
                      {row.sparse_band.combined_regions.join(", ") || (row.sparse_band.combined_global ? "whole interior" : "none")};
                      {" "}retained fine: {row.sparse_band.retained_fine.map(([reason, count]) =>
                        `${reason.replaceAll("_", " ")} (${count})`).join("; ") || "none"}.
                    </>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
          <details>
            <summary>Captured provenance</summary>
            <pre data-testid="manufacturing-provenance">
              {JSON.stringify(result.provenance, null, 2)}
            </pre>
          </details>
          {replay && (
            <fieldset disabled={!fresh || manufacturing.working}>
              <legend>Actual emitted-program replay — endpoint coordinates in mm</legend>
              <p>
                {replay.eventCount} events, {replay.depositionCount} object + support deposition,{" "}
                {replay.travelCount} object travel, {replay.lifecycleCount} lifecycle,{" "}
                {replay.processCount} stationary retraction/recovery;
                {replay.layerCount} object layers. Positions, E and F come from decoded program
                bytes, not planned paths.
              </p>
              {wallMetrics && <p data-testid="manufacturing-wall-association-metrics">
                Automatic Wall strokes: {wallMetrics.strokes}; nominal associated width range{" "}
                {wallMetrics.min.toFixed(6)}–{wallMetrics.max.toFixed(6)} mm.
                {wallMetrics.min === wallMetrics.max
                  ? " These inspected commands share one locally constant width."
                  : " Constant-section commands realize a variable local allocation; they are not ideal continuous tapers."}
                {" "}Wall ownership survives genuine Bridge roles. Dimensions are verified
                plan associations, not measured physical beads.
              </p>}
              {sparseMetrics && <p data-testid="manufacturing-sparse-association-metrics">
                Sparse strokes: {sparseMetrics.strokes}; associated height exceeds owning fine layer:
                {" "}{sparseMetrics.combined}; nominal associated height range {sparseMetrics.min.toFixed(6)}
                –{sparseMetrics.max.toFixed(6)} mm. Sparse colors progress green to amber with associated
                height/width; walls and skins retain their separate role colors. Inspect each event’s
                nominal width, physical-Z interval and regional owner below. These are verified
                expected-plan associations, not measured physical bead dimensions.
              </p>}
              {sparseBands.length > 0 && <p data-testid="manufacturing-sparse-band-decisions">
                {selectedSparseBand ? <>
                  This physical band spans layers {selectedSparseBand.sparse_band!.first_layer}–{selectedSparseBand.index}.
                  {" "}Combined regional owners: {selectedSparseBand.sparse_band!.combined_regions.join(", ") ||
                    (selectedSparseBand.sparse_band!.combined_global ? "whole interior" : "none")}.
                  {" "}Retained fine: {selectedSparseBand.sparse_band!.retained_fine.map(([reason, count]) =>
                    `${reason.replaceAll("_", " ")} (${count})`).join("; ") || "none"}.
                </> : <>Select a layer to inspect its band decision; the layer-accounting table lists all bands.</>}
                {" "}Decisions describe admitted software construction, not physical support or adhesion.
              </p>}
              <p>
                Packed roles distinguish object walls/skins/fill/bridge, support/interface, travel,
                lifecycle and prime. Retract/recover are stationary process events, not deposited
                material or fictional XYZ motion. Lifecycle events have no object layer and appear
                only with all layers selected; travel toggle excludes prime and stationary process
                events.
              </p>
              {!inspection?.displayReady && (
                <p>
                  No active GPU replay display. Validated transport/coordinate inspection only;
                  diagnostic evidence does not certify WebGPU rendering or picking.
                </p>
              )}
              {emptyLayer && (
                <p data-testid="manufacturing-empty-layer">
                  Selected schedule layer is a verified support contact gap: no deposition or motion
                  events. Actual thickness is retained; no neighbouring layer is substituted.
                </p>
              )}
              <label>
                Layer (zero-based; blank means all)
                <input
                  data-testid="manufacturing-layer"
                  type="number"
                  min="0"
                  max={replay.layerCount - 1}
                  value={manufacturing.layer ?? ""}
                  onChange={(event) => {
                    void manufacturing.updateView(
                      event.target.value === "" ? null : Number(event.target.value),
                      manufacturing.eventCount,
                      manufacturing.showTravel,
                      manufacturing.showSource,
                    );
                  }}
                />
              </label>
              <label>
                Event prefix / cursor
                <input
                  data-testid="manufacturing-events"
                  type="range"
                  min="0"
                  max={replay.eventCount}
                  value={manufacturing.eventCount}
                  onChange={(event) => {
                    void manufacturing.updateView(
                      manufacturing.layer,
                      Number(event.target.value),
                      manufacturing.showTravel,
                      manufacturing.showSource,
                    );
                  }}
                />
                <output>
                  {manufacturing.eventCount} / {replay.eventCount}
                </output>
              </label>
              <label>
                <input
                  data-testid="manufacturing-travel"
                  type="checkbox"
                  checked={manufacturing.showTravel}
                  onChange={(event) => {
                    void manufacturing.updateView(
                      manufacturing.layer,
                      manufacturing.eventCount,
                      event.target.checked,
                      manufacturing.showSource,
                    );
                  }}
                />
                Show travel / unknown-start endpoint
              </label>
              <label>
                <input
                  data-testid="manufacturing-source-bodies"
                  type="checkbox"
                  checked={manufacturing.showSource}
                  disabled={!inspection?.displayReady}
                  onChange={(event) => {
                    void manufacturing.updateView(
                      manufacturing.layer,
                      manufacturing.eventCount,
                      manufacturing.showTravel,
                      event.target.checked,
                    );
                  }}
                />
                Show source bodies (hide to inspect interior deposition)
              </label>
              {event !== null && (
                <pre data-testid="manufacturing-event">
                  {JSON.stringify(
                    {
                      event_index: event,
                      source_line: replay.sourceLine(event),
                      layer:
                        replay.layer(event) === REPLAY_LIFECYCLE_LAYER ? null : replay.layer(event),
                      kind: REPLAY_KIND_LABELS[replay.kind(event)],
                      known_start: replay.knownStart(event),
                      position_mm: replay.position(event),
                      absolute_extrusion_mm: replay.extrusion(event),
                      feed_mm_min: replay.feed(event),
                      nominal_plan_association_not_measurement: replay.association(event),
                    },
                    null,
                    2,
                  )}
                </pre>
              )}
              <p>
                The first commanded travel is an endpoint only: its prior position, distance and
                duration are unknown. No homing trajectory is displayed. Coincident travels remain
                valid events. Display projection error is bounded to 0.001 mm; float display is not
                native measurement or material calibration.
              </p>
            </fieldset>
          )}
          <button
            data-testid="manufacturing-export"
            disabled={!available || !fresh}
            onClick={() => {
              void manufacturing.exportBundle();
            }}
          >
            Export four files to a new directory…
          </button>
          <p>
            Native dialog grants a single-use destination token. Export never overwrites or retries,
            and rechecks this fresh bundle before committing plan.json, program.gcode,
            provenance.json and verification.json.
          </p>
          {host.state.runtime?.geometry_debug_enabled && (
            <details>
              <summary>Startup-gated native export fixture admission</summary>
              <input
                data-testid="manufacturing-export-path"
                value={exportPath}
                onChange={(event) => setExportPath(event.target.value)}
              />
              <button
                data-testid="manufacturing-debug-export"
                disabled={!available || !fresh}
                onClick={() => {
                  void manufacturing.exportBundle(exportPath);
                }}
              >
                Export to confined new fixture directory
              </button>
            </details>
          )}
        </div>
      )}
      <details>
        <summary>Manufacturing state / diagnostics</summary>
        <pre data-testid="manufacturing-state">
          {JSON.stringify(
            {
              session_id: host.state.phase === "running" ? host.state.hello?.session_id : null,
              project_id: host.state.project?.project_id,
              project_revision: host.state.project?.revision,
              read_only: host.state.project?.read_only,
              selected_artifact: status?.artifact ?? null,
              selected_intent: status?.intent ?? null,
              fresh,
              working: manufacturing.working,
              draft_changed: manufacturing.changed,
              preflight: manufacturing.preflight ?? null,
              bed_proposal: manufacturing.bedProposal,
              bed_proposal_stale: manufacturing.bedProposalStale,
              replay: replay
                ? {
                    event_count: replay.eventCount,
                    lifecycle_count: replay.lifecycleCount,
                    deposition_count: replay.depositionCount,
                    process_count: replay.processCount,
                    travel_count: replay.travelCount,
                    layer_count: replay.layerCount,
                    byte_count: replay.bytes.byteLength,
                    display_ready: inspection?.displayReady,
                  }
                : null,
              report: result?.report ?? null,
              provenance: result?.provenance ?? null,
              diagnostic_no_gpu: host.state.diagnosticSession,
            },
            null,
            2,
          )}
        </pre>
      </details>
    </section>
  );
}

function PreflightChecks({ checks }: { checks: PreflightCheck[] }) {
  return <ul>{checks.map((check, index) => (
    <li key={index}>
      <strong>{check.severity}: {check.code}</strong> — {check.message};
      evidence: {check.evidence}; required: {String(check.required)}
      {check.occurrence_id !== null && <>; occurrence {check.occurrence_id}</>}
      {check.face_id !== null && <>; face {check.face_id}</>}
    </li>
  ))}</ul>;
}
