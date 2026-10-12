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
