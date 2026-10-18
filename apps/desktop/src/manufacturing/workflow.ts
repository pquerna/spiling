/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import type { ManufacturingHost } from "../useWorkbench.ts";
import type { ManufacturingReviewState } from "./useManufacturing.ts";
type Action = "load_intent" | "apply" | "reload" | "preflight" | "compile" | "inspect" | "export";
interface NextStep {
  id: string;
  title: string;
  detail: string;
  action?: Action;
  label?: string;
  href?: string;
}

// Presentation only: no stored stage, guessed placement, or new manufacturing authority.
export function nextManufacturingStep(state: ManufacturingHost["state"], review: ManufacturingReviewState): NextStep {
  const { status, preflight } = review;
  const project = state.project;
  const readOnly = project?.read_only;
  if (state.phase !== "running") return {
    id: "inactive", title: "Connect the native engine",
    detail: "Manufacturing requires a running desktop session. Use the session controls to start or reconnect.",
  };
  if (state.busy || review.working || review.admittingReplay) return {
    id: "working", title: "Native operation in progress",
    detail: "Wait for its result or use Cancel below. No further manufacturing action is submitted automatically.",
  };
  if (state.stale || !project || !status || status.info.project_id !== project.project_id ||
      status.info.revision !== project.revision) return {
    id: "syncing", title: "Refreshing current project authority",
    detail: "Wait for the current project and manufacturing status to agree; stale reports cannot select a next action.",
  };
  if (review.conflict) return {
    id: "conflict", title: "Resolve the intent draft conflict",
    detail: "The project intent changed while this draft was edited. Reload explicitly before making another change.",
    action: "reload", label: "Reload current intent / discard draft",
  };
  if (review.changed) return readOnly ? {
    id: "readonly", title: "Read-only project",
    detail: "Draft changes cannot be applied here. Open a writable project to edit or compile.",
  } : {
    id: "draft", title: "Apply the intent draft",
    detail: "These settings are not project authority yet. Apply once before Preflight or Compile; this is undoable.",
    action: "apply", label: "Apply full intent once",
  };
  if (!state.occurrences.length) return {
    id: "source", title: "Import a native part",
    detail: "Use Import parts in the source controls. Source geometry, not a display mesh, owns manufacturing.",
  };
  if (!status.intent) return readOnly ? {
    id: "readonly", title: "No applied intent in this read-only project",
    detail: "Open a writable project to load and apply a full printer and recipe intent.",
  } : {
    id: "intent", title: "Choose a manufacturing intent",
    detail: "Load full intent JSON containing the printer, recipe and process declarations, then review and Apply it.",
    action: "load_intent", label: "Load full ManufacturingIntent JSON…",
  };
  if (review.failure) return {
    id: "failed", title: `${review.failure.action === "inspect" ? "Verification" : review.failure.action === "preflight" ? "Preflight" : review.failure.action === "compile" ? "Compile" : "Apply"} did not complete`,
    detail: "Review the operation failure below before changing the input or retrying. No successful result is inferred from an earlier report.",
    href: "#manufacturing-operation-failure", label: "Review operation failure",
  };
  if (status.artifact) {
    // Compile/save bookkeeping may stale Preflight without changing the selected bundle.
    if (!review.fresh) return {
      id: "verify", title: "Verify the selected program freshly",
      detail: "A compiled bundle or a saved verified flag is not fresh inspection authority. Decode the actual emitted bytes in this session.",
      action: "inspect", label: "Fresh report and motion",
    };
    if (!review.inspection?.result.report.verified) return {
      id: "failed", title: "Independent verification did not pass",
      detail: "Review the verifier report. This program is not ready for export.",
    };
    if (!review.inspection.replay) return {
      id: "inspect", title: "Inspect actual emitted motion",
      detail: "The report is fresh. Load actual-program motion to inspect it; report-only export remains available below.",
      action: "inspect", label: "Fresh report and motion",
    };
    return {
      id: "ready", title: "Fresh program ready for review and export",
      detail: "Inspect the actual motion and coverage, then choose a new export directory. Software verification is not machine readiness.",
      action: "export", label: "Export verified bundle…",
    };
  }
  if (review.bedProposal && !review.bedProposalStale && !readOnly) return {
    id: "placement", title: "Review the proposed bed placement",
    detail: "The proposal has not moved the part. Review the original and proposed poses before explicitly applying it.",
    href: "#manufacturing-bed-review", label: "Review bed proposal",
  };
  if (!preflight || preflight.input_revision !== project.revision) return {
    id: "preflight", title: preflight ? "Preflight is stale — check current input" : "Check the current input",
    detail: "Preflight checks applied intent and native placement without changing the project. Use native bed preparation below if placement needs changing.",
    action: "preflight", label: "Run input Preflight",
  };
  if (preflight.disposition !== "admitted") return {
    id: preflight.disposition, title: preflight.disposition === "refused" ? "Input admission refused" : "Input admission unresolved",
    detail: "Review required checks and their native scope below. Correct the input or placement before rerunning; unresolved evidence is not success.",
    href: "#manufacturing-preflight-review", label: "Review required admission checks",
  };
  return readOnly ? {
    id: "readonly", title: "Input admitted; no compiled bundle in this snapshot",
    detail: "Read-only Preflight is permitted, but Compile requires a writable project. Admission is not a manufacturing result.",
  } : {
    id: "compile", title: "Input admitted — Compile is next",
    detail: "Native material, topology, support and resource analysis still run during Compile. Admission alone does not prove they will pass.",
    action: "compile", label: "Compile software bundle",
  };
}
