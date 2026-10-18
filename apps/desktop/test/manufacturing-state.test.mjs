/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  draftChanged,
  draftIntent,
  makeDraft,
  inspectionIsFresh,
} from "../src/manufacturing/draft.ts";
import { nextManufacturingStep } from "../src/manufacturing/workflow.ts";

const intent = JSON.parse(
  await readFile(
    new URL("../../../fixtures/manufacturing/solid-fill.intent.json", import.meta.url),
    "utf8",
  ),
);
test("unchanged full intent does not submit; failed scalar admission retains draft and source", () => {
  const draft = makeDraft(intent, intent);
  assert.equal(draftChanged(draft), false);
  draft.values.layer_height_mm = "";
  assert.equal(draftChanged(draft), true);
  assert.throws(() => draftIntent(draft), /finite positive/);
  assert.equal(draft.values.layer_height_mm, "");
  assert.equal(intent.recipe.layer_height_mm, 0.2);
  draft.values.layer_height_mm = "0.4";
  const applied = draftIntent(draft);
  assert.equal(applied.recipe.layer_height_mm, 0.4);
  assert.deepEqual(applied.printer, intent.printer);
  assert.equal(intent.recipe.layer_height_mm, 0.2);
});
test("freshness uses selected content, not captured revision; session, reopen and invalidation remain binding", () => {
  const record = { hash: "bundle", input_hash: "input" };
  const inspection = {
    session: "session-a",
    authority: "session-a:project:0",
    projectId: "project",
    invalidated: false,
    record,
  };
  const current = {
    session: "session-a",
    authority: "session-a:project:0",
    projectId: "project",
    statusProjectId: "project",
    currentRevision: 10,
    statusRevision: 10,
    artifact: record,
  };
  assert.equal(inspectionIsFresh(inspection, current), true);
  assert.equal(
    inspectionIsFresh(inspection, { ...current, currentRevision: 11, statusRevision: 11 }),
    true,
    "Checkpoint bookkeeping with unchanged selected hash remains fresh",
  );
  assert.equal(
    inspectionIsFresh(inspection, { ...current, currentRevision: 11 }),
    false,
    "Pending status is not active",
  );
  assert.equal(
    inspectionIsFresh(inspection, { ...current, artifact: { ...record, input_hash: "changed" } }),
    false,
  );
  assert.equal(inspectionIsFresh(inspection, { ...current, session: "session-b" }), false);
  assert.equal(
    inspectionIsFresh(inspection, { ...current, authority: "session-a:project:1" }),
    false,
    "Reopen needs fresh Verify",
  );
  assert.equal(
    inspectionIsFresh({ ...inspection, invalidated: true }, current),
    false,
    "Undo cannot reactivate history silently",
  );
  assert.equal(
    inspectionIsFresh(inspection, { ...current, statusProjectId: "another-project" }),
    false,
  );
});

test("full complete intent scalar edits preserve declaration identity and unedited policies", async () => {
  const complete = JSON.parse(await readFile(new URL(
    "../../../fixtures/manufacturing/complete-sparse.intent.json", import.meta.url), "utf8"));
  const original = structuredClone(complete);
  const draft = makeDraft(complete, complete);
  assert.equal(draftChanged(draft), false);
  draft.values.infill_fraction = "0.5";
  draft.policyValues["process.first_layer_speed_factor"] = "0.4";
  draft.policyValues["process.retraction.distance_mm"] = "1.1";
  const changed = draftIntent(draft);
  assert.deepEqual(changed.printer, original.printer);
  assert.equal(changed.recipe.process.first_layer_speed_factor, 0.4);
  assert.equal(changed.recipe.process.retraction.distance_mm, 1.1);
  assert.deepEqual(changed.recipe.supports, original.recipe.supports);
  assert.equal(changed.recipe.process.bridge_fan_pwm, original.recipe.process.bridge_fan_pwm);
  assert.deepEqual(complete, original, "Draft must not mutate the admitted intent");
  draft.policyValues["process.fan_first_layers"] = "256";
  assert.throws(() => draftIntent(draft), /policy range/);
  assert.equal(draft.policyValues["process.fan_first_layers"], "256", "Failed policy edit is retained");
});

test("explicit native shape policy survives scalar Apply and authoritative draft reload", () => {
  const original = structuredClone(intent);
  original.recipe.native_shape_error_mm = 0.075;
  const draft = makeDraft(original, original);
  draft.values.print_speed_mm_s = "36";
  const applied = draftIntent(draft);
  assert.equal(applied.recipe.native_shape_error_mm, 0.075);
  assert.deepEqual(applied.recipe.process, original.recipe.process);
  assert.deepEqual(applied.recipe.supports, original.recipe.supports);
  const reloaded = makeDraft(JSON.parse(JSON.stringify(applied)), applied);
  assert.equal(draftChanged(reloaded), false);
  assert.equal(reloaded.values.native_shape_error_mm, "0.075");
  reloaded.values.native_shape_error_mm = "0.05";
  assert.equal(draftIntent(reloaded).recipe.native_shape_error_mm, 0.05);
  for (const invalid of ["", "0", "-0.1", "NaN", "Infinity"]) {
    reloaded.values.native_shape_error_mm = invalid;
    assert.throws(() => draftIntent(reloaded), /Native shape error budget.*finite positive/);
    assert.equal(reloaded.values.native_shape_error_mm, invalid);
  }
  assert.equal(original.recipe.native_shape_error_mm, 0.075);
});

test("enabled support edits retain explicit zero gap and disabled policies are never invented", () => {
  const supported = structuredClone(intent);
  supported.recipe.supports = { kind: "breakaway", density: 0.25, xy_clearance_mm: 0.5,
    z_gap_mm: 0, interface_thickness_mm: 0.4 };
  const draft = makeDraft(supported, supported);
  draft.policyValues["supports.density"] = "0.5";
  const changed = draftIntent(draft);
  assert.equal(changed.recipe.supports.density, 0.5);
  assert.equal(changed.recipe.supports.z_gap_mm, 0);
  assert.deepEqual(changed.printer, supported.printer);
  const disabled = makeDraft(intent, intent);
  assert.equal("supports.density" in disabled.policyValues, false);
  assert.deepEqual(draftIntent(disabled).recipe.supports, intent.recipe.supports);
});

function guidance() {
  return {
    state: { phase: "running", busy: false, stale: false,
      project: { project_id: "project", revision: 9, read_only: false }, occurrences: [{}] },
    review: { status: { info: { project_id: "project", revision: 9 }, intent, artifact: null },
      changed: false, conflict: false, fresh: false, inspection: null, working: false,
      admittingReplay: false, preflight: null, bedProposal: null, bedProposalStale: false,
      failure: null },
  };
}

test("guidance requires current admission and does not promote optional Compile evidence", () => {
  const { state, review } = guidance();
  review.preflight = { input_revision: 8, disposition: "admitted", checks: [] };
  assert.equal(nextManufacturingStep(state, review).action, "preflight");
  review.preflight.input_revision = 9;
  review.preflight.checks = [{ required: false, severity: "error", evidence: "failed" }];
  assert.equal(nextManufacturingStep(state, review).action, "compile",
    "Optional Compile failure does not replace the engine's admission disposition");
  for (const disposition of ["refused", "unresolved"]) {
    review.preflight.disposition = disposition;
    const step = nextManufacturingStep(state, review);
    assert.equal(step.action, undefined, "A refused/unresolved input must not recommend Compile");
    assert.equal(step.href, "#manufacturing-preflight-review");
  }
});

test("read-only snapshots can preflight and freshly inspect/export but never mutate", () => {
  const { state, review } = guidance();
  state.project.read_only = true;
  assert.equal(nextManufacturingStep(state, review).action, "preflight");
  review.preflight = { input_revision: 9, disposition: "admitted", checks: [] };
  assert.equal(nextManufacturingStep(state, review).action, undefined);
  review.status.artifact = { hash: "selected" };
  assert.equal(nextManufacturingStep(state, review).action, "inspect");
  review.fresh = true;
  review.inspection = { result: { report: { verified: true } }, replay: {} };
  assert.equal(nextManufacturingStep(state, review).action, "export");
  review.changed = true;
  assert.equal(nextManufacturingStep(state, review).action, undefined);
});

test("drafts, conflicts and pending authority supersede an otherwise exportable program", () => {
  const { state, review } = guidance();
  review.status.artifact = { hash: "selected" };
  review.fresh = true;
  review.inspection = { result: { report: { verified: true } }, replay: {} };
  review.changed = true;
  assert.equal(nextManufacturingStep(state, review).action, "apply");
  review.conflict = true;
  assert.equal(nextManufacturingStep(state, review).action, "reload");
  review.status.info.revision = 8;
  assert.equal(nextManufacturingStep(state, review).action, undefined);
  review.status.info.revision = 9;
  state.stale = true;
  assert.equal(nextManufacturingStep(state, review).action, undefined);
});

test("fresh selected content survives stale Preflight but failed verification cannot export", () => {
  const { state, review } = guidance();
  review.preflight = { input_revision: 8, disposition: "refused", checks: [] };
  review.status.artifact = { hash: "selected" };
  assert.equal(nextManufacturingStep(state, review).action, "inspect");
  review.fresh = true;
  review.inspection = { result: { report: { verified: true } }, replay: null };
  assert.equal(nextManufacturingStep(state, review).action, "inspect");
  review.inspection.replay = {};
  assert.equal(nextManufacturingStep(state, review).action, "export");
  review.inspection.result.report.verified = false;
  assert.equal(nextManufacturingStep(state, review).action, undefined);
});

test("a current operation failure directs review instead of automatically retrying Compile", () => {
  const { state, review } = guidance();
  review.preflight = { input_revision: 9, disposition: "admitted", checks: [] };
  review.failure = { action: "compile", message: "A bounded native operation stopped." };
  const step = nextManufacturingStep(state, review);
  assert.equal(step.action, undefined);
  assert.equal(step.href, "#manufacturing-operation-failure");
  review.failure = null;
  assert.equal(nextManufacturingStep(state, review).action, "compile");
});
