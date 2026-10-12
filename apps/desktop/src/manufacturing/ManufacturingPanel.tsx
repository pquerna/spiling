/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useState } from "react";
import {
  MAX_REPLAY_DISPLAY_BYTES,
  MAX_REPLAY_EVENTS,
  MAX_PRINT_LAYERS,
  MAX_DEPOSITION_SEGMENTS,
  REPLAY_KIND_LABELS,
  REPLAY_LIFECYCLE_LAYER,
} from "@spiling/protocol";
import type { ManufacturingHost } from "../useWorkbench.ts";
import { scalarFields } from "./draft.ts";
import { useManufacturing } from "./useManufacturing.ts";
import "./manufacturing.css";

export function ManufacturingPanel({
  host,
  cancel,
  diagnosticImport,
}: {
  host: ManufacturingHost;
  cancel: () => void;
  diagnosticImport: () => Promise<void>;
}) {
  const manufacturing = useManufacturing(host);
  const [exportPath, setExportPath] = useState("manufacturing-export");
  const { status, draft, inspection, fresh, available, writable } = manufacturing;
  const result = inspection?.result,
    replay = inspection?.replay;
  const event = replay && manufacturing.eventCount > 0 ? manufacturing.eventCount - 1 : null;
  const job = host.state.activeJob;
  return (
    <section
      className="manufacturing-controls geometry-controls"
      aria-label="Manufacturing review"
      data-testid="manufacturing-panel"
    >
      <h2>Manufacturing review</h2>
      <strong>Software-only — not machine-ready</strong>
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
        <button
          data-testid="manufacturing-diagnostic-import"
          disabled={!writable || !!host.state.occurrences.length}
          onClick={() => {
            void diagnosticImport();
          }}
        >
          Admit box-mm STEP fixture through native tokens
        </button>
      )}
      <button
        data-testid="manufacturing-select-software"
        disabled={!writable}
        onClick={manufacturing.selectSoftwareIntent}
      >
        Select bundled software-only reference intent
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
          </fieldset>
          <p>
            Fixed-height vertical planar/cylindrical geometry; integral layer heights. Skin
            thickness conservatively rounds up to whole layers; interior fill is solid at fraction 1
            and sparse below 1. No generated supports, material calibration or physical-printability
            proof.
          </p>
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
      <div className="geometry-buttons">
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
      <output data-testid="manufacturing-message" role="status">
        {manufacturing.message}
      </output>
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
          <small>
            Compiled summary is not a fresh actual-program report. Revision bookkeeping is not
            content identity.
          </small>
        </details>
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
            <dt>Object deposited volume</dt>
            <dd>{result.report.deposited_volume_mm3} mm³</dd>
            <dt>Object filament length</dt>
            <dd>{result.report.filament_length_mm} mm</dd>
            <dt>Lifecycle motion events</dt>
            <dd>{result.report.lifecycle_motion_count}</dd>
            <dt>Priming filament length (separate from object)</dt>
            <dd>{result.report.priming_filament_length_mm} mm</dd>
            <dt>Maximum position deviation</dt>
            <dd>{result.report.max_position_error_mm} mm</dd>
            <dt>Maximum extrusion deviation</dt>
            <dd>{result.report.max_extrusion_error_mm} mm</dd>
          </dl>
          <details>
            <summary>Captured provenance</summary>
            <pre data-testid="manufacturing-provenance">
              {JSON.stringify(result.provenance, null, 2)}
            </pre>
          </details>
          {replay && (
            <fieldset disabled={!fresh}>
              <legend>Actual emitted-program replay — endpoint coordinates in mm</legend>
              <p>
                {replay.eventCount} events, {replay.depositionCount} deposition,{" "}
                {replay.travelCount} object travel, {replay.lifecycleCount} lifecycle;
                {replay.layerCount} object layers. Positions, E and F come from decoded program
                bytes, not planned paths.
              </p>
              <p>
                Packed path colors: perimeter orange, solid fill turquoise, bottom skin purple, top
                skin pink, sparse fill green, object travel blue-gray, lifecycle travel blue, prime
                yellow. Lifecycle events have no object layer and appear only with all layers
                selected; the travel toggle also controls lifecycle travel, not priming.
              </p>
              {!inspection?.displayReady && (
                <p>
                  No active GPU replay display. Validated transport/coordinate inspection only;
                  diagnostic evidence does not certify WebGPU rendering or picking.
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
                    );
                  }}
                />
                Show travel / unknown-start endpoint
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
              replay: replay
                ? {
                    event_count: replay.eventCount,
                    lifecycle_count: replay.lifecycleCount,
                    deposition_count: replay.depositionCount,
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
