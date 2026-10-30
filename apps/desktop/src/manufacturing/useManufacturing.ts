/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { useEffect, useRef, useState } from "react";
import {
  decodeReplayDisplay,
  type BedPosePreparation,
  type DecodedReplayDisplay,
  type JobResult,
  type ManufacturingResponse,
  type ManufacturingIntent,
  type VerificationOutput,
  type RectilinearAxisPolicy,
  type SeamPolicy,
} from "@spiling/protocol";
import { native, type ManufacturingLayer } from "../native.ts";
import type { ManufacturingHost } from "../useWorkbench.ts";
import softwareIntentJson from "../../../../fixtures/manufacturing/solid-fill.intent.json?raw";
import {
  draftChanged,
  draftIntent,
  inspectionIsFresh,
  intentIdentity,
  makeDraft,
  sameArtifact,
  type IntentDraft,
  type ScalarField,
} from "./draft.ts";

const softwareIntent: ManufacturingIntent = JSON.parse(softwareIntentJson);

type Status = Extract<ManufacturingResponse, { type: "status" }>;
type Verified = Extract<JobResult, { kind: "manufacturing_verified" }>;
type Preflight = Extract<JobResult, { kind: "manufacturing_preflight" }>["report"];
interface Inspection {
  result: Verified;
  session: string;
  request: number;
  replay: DecodedReplayDisplay | null;
  displayReady: boolean;
  diagnostic: boolean;
  authority: string;
  invalidated: boolean;
  layers: ManufacturingLayer[];
}

type ManufacturingAction = "apply" | "compile" | "inspect" | "preflight";
export interface ManufacturingReviewState {
  status: Status | null;
  changed: boolean;
  conflict: boolean;
  fresh: boolean;
  inspection: Inspection | null;
  working: boolean;
  admittingReplay: boolean;
  preflight: Preflight | null;
  bedProposal: BedPosePreparation | null;
  bedProposalStale: boolean;
  failure: { action: ManufacturingAction; message: string } | null;
}
export interface ManufacturingController extends ManufacturingReviewState {
  draft: IntentDraft | null;
  message: string;
  available: boolean;
  writable: boolean;
  layer: number | null;
  eventCount: number;
  showTravel: boolean;
  showSource: boolean;
  colorByMaterial: boolean;
  declaredToolColors: readonly [string, string] | null;
  selectIntent(relativePath?: string): Promise<void>;
  prepareBed(occurrenceId: number, center: boolean): Promise<void>;
  applyBed(): Promise<void>;
  clearBedProposal(): void;
  selectSoftwareIntent(): void;
  editScalar(key: ScalarField, value: string): void;
  editPolicyScalar(key: string, value: string): void;
  editWidthLimit(bound: "minimum" | "maximum", value: string): void;
  editSeamPolicy(value: SeamPolicy["kind"]): void;
  editSparseInfill(kind: "fine" | "combine_eligible", maxHeight: string): void;
  editPreferredDirection(axis: 0 | 1, value: string): void;
  editRectilinearAxis(value: RectilinearAxisPolicy): void;
  editTopSkinAxis(value: RectilinearAxisPolicy): void;
  reloadDraft(): void;
  apply(): Promise<void>;
  compile(): Promise<void>;
  preflightIntent(): Promise<void>;
  inspect(output: VerificationOutput): Promise<void>;
  updateView(layer: number | null, eventCount: number, showTravel: boolean, showSource: boolean, colorByMaterial?: boolean): Promise<void>;
  exportBundle(relativePath?: string): Promise<void>;
  cancelPreview(): void;
}
export function useManufacturing(host: ManufacturingHost): ManufacturingController {
  const hostRef = useRef(host);
  hostRef.current = host;
  const [status, setStatus] = useState<Status | null>(null);
  const statusRef = useRef<Status | null>(null);
  const [draft, setDraft] = useState<IntentDraft | null>(null);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const inspectionRef = useRef(inspection);
  inspectionRef.current = inspection;
  const [message, setMessage] = useState("");
  const [preflight, setPreflight] = useState<Preflight | null>(null);
  const [working, setWorking] = useState(false);
  const [failure, setFailure] = useState<{
    action: ManufacturingAction;
    message: string;
    revision: number | undefined;
    authority: string;
  } | null>(null);
  const [bedProposal, setBedProposal] = useState<{
    preparation: BedPosePreparation;
    authority: string;
    invalidated: boolean;
  } | null>(null);
  const bedRequest = useRef(0);
  const [admittingReplay, setAdmittingReplay] = useState(false);
  const [layer, setLayer] = useState<number | null>(null);
  const [eventCount, setEventCount] = useState(0);
  const [showTravel, setShowTravel] = useState(true);
  const [showSource, setShowSource] = useState(true);
  const [colorByMaterial, setColorByMaterial] = useState(false);
  const declaredToolColors = ((): readonly [string, string] | null => {
    const workflow = inspection?.result.provenance.workflow;
    if (workflow?.kind !== "fixed_dual_marlin") return null;
    const colors: [string | null, string | null] = [null, null];
    for (const construction of workflow.job.surface_constructions) {
      for (const capture of construction.optical_origin?.materials ?? []) {
        const tool = workflow.job.tools.findIndex(
          (tool) => tool.material.id === capture.loaded_material.id,
        );
        const evidence = capture.evidence.evidence;
        // Catalog display metadata only; the reference mixture is never painted
        // onto independently decoded tool strokes.
        if (tool < 0 || tool > 1 || evidence.kind !== "catalog_estimate") continue;
        const color =
          "#" +
          evidence.srgb
            .map((channel) =>
              Math.round(channel * 255)
                .toString(16)
                .padStart(2, "0"),
            )
            .join("");
        if (colors[tool] !== null && colors[tool] !== color) return null;
        colors[tool] = color;
      }
    }
    return colors[0] && colors[1] ? [colors[0], colors[1]] : null;
  })();
  const request = useRef(0),
    refreshSequence = useRef(0),
    mounted = useRef(true);
  const changingView = useRef(false);
  const authority = useRef("");
  const session = host.state.phase === "running" ? host.state.hello?.session_id : null;
  const projectId = host.state.project?.project_id;
  const revision = host.state.project?.revision;
  const currentAuthority = `${session ?? ""}:${projectId ?? ""}:${host.state.projectEpoch}`;
  const fresh = inspectionIsFresh(
    inspection
      ? {
          session: inspection.session,
          authority: inspection.authority,
          invalidated: inspection.invalidated,
          projectId: inspection.result.provenance.project_id,
          record: inspection.result.record,
        }
      : null,
    {
      session,
      authority: currentAuthority,
      projectId,
      statusProjectId: status?.info.project_id,
      currentRevision: revision,
      statusRevision: status?.info.revision,
      artifact: status?.artifact,
    },
  );
  const changed = draftChanged(draft);
  const conflict = !!draft && !!status && draft.base !== intentIdentity(status.intent);
  const available = host.state.phase === "running" && !host.state.busy && !working;
  const writable = available && !host.state.project?.read_only;
  const bedProposalStale = !!bedProposal && (
    bedProposal.invalidated || bedProposal.authority !== currentAuthority ||
    bedProposal.preparation.session_id !== session ||
    bedProposal.preparation.scene_revision !== host.state.scene?.revision ||
    bedProposal.preparation.project_revision !== revision ||
    host.state.stale || changed || conflict
  );
  useEffect(() => {
    if (bedProposalStale)
      setBedProposal(previous => previous && !previous.invalidated ? { ...previous, invalidated: true } : previous);
  }, [bedProposalStale]);

  function ensureRequest(token: number, id: number) {
    hostRef.current.ensure(token);
    if (!mounted.current || request.current !== id)
      throw new Error("Manufacturing request superseded.");
  }
  async function refresh(token: number): Promise<Status> {
    const sequence = ++refreshSequence.current;
    const owner = hostRef.current.snapshot().project?.project_id;
    const session = hostRef.current.snapshot().hello?.session_id;
    if (!session) throw new Error("No active native manufacturing session.");
    const response = await hostRef.current.exchange(
      () => native.manufacturing({ op: "get", session_id: session }),
      token,
    );
    if (response.type === "error")
      throw new Error(`${response.error.code}: ${response.error.message}`);
    if (response.type !== "status") throw new Error("Unexpected manufacturing status response.");
    const snapshot = hostRef.current.snapshot();
    if (
      !mounted.current ||
      snapshot.hello?.session_id !== session ||
      (owner && response.info.project_id !== owner)
    )
      throw new Error("Manufacturing status superseded.");
    if (sequence !== refreshSequence.current) return response;
    statusRef.current = response;
    setStatus(response);
    if (!draftChanged(draftRef.current)) {
      const next = response.intent ? makeDraft(response.intent, response.intent) : null;
      draftRef.current = next;
      setDraft(next);
    }
    return response;
  }
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      ++request.current;
      ++refreshSequence.current;
      ++bedRequest.current;
    };
  }, []);
  useEffect(() => {
    if (authority.current !== currentAuthority) {
      authority.current = currentAuthority;
      ++request.current;
      ++refreshSequence.current;
      ++bedRequest.current;
      statusRef.current = null;
      setStatus(null);
      draftRef.current = null;
      setDraft(null);
      setMessage("");
      setWorking(false);
      setPreflight(null);
      setFailure(null);
      if (inspectionRef.current) {
        const historical = {
          ...inspectionRef.current,
          replay: null,
          displayReady: false,
          invalidated: true,
        };
        inspectionRef.current = historical;
        setInspection(historical);
      }
      // Project replacement owns awaited GPU cleanup; session teardown disposes the viewport.
    }
    if (!session) return;
    const token = hostRef.current.token();
    void refresh(token).catch((error: unknown) => {
      if (mounted.current && hostRef.current.snapshot().hello?.session_id === session)
        setMessage(String(error));
    });
  }, [session, projectId, revision, currentAuthority]);
  useEffect(() => {
    if (
      inspection &&
      !inspection.invalidated &&
      inspection.authority === currentAuthority &&
      status &&
      !sameArtifact(status.artifact, inspection.result.record)
    ) {
      void hostRef.current.clearReplay().catch((error: unknown) => {
        if (mounted.current) setMessage(String(error));
      });
      const historical = { ...inspection, replay: null, displayReady: false, invalidated: true };
      inspectionRef.current = historical;
      setInspection(historical);
    }
  }, [status, inspection, currentAuthority]);

  async function selectIntent(relativePath?: string) {
    if (!writable) return;
    setFailure(null);
    setWorking(true);
    await hostRef.current.run(async (token) => {
      try {
        const selection = await hostRef.current.exchange(
          () => relativePath === undefined ? native.selectManufacturingIntent()
            : native.debugSelectManufacturingIntent(relativePath), token,
        );
        if (!selection) { setMessage("Intent selection cancelled; draft unchanged."); return; }
        const intent = await hostRef.current.exchange(() => native.loadManufacturingIntent(selection), token);
        const next = makeDraft(intent, statusRef.current?.intent ?? null);
        draftRef.current = next;
        setDraft(next);
        setMessage(`Loaded ${selection.label}: Rust-validated full inert intent. Apply commits it; physical qualification is absent.`);
      } catch (error) { setMessage(String(error)); throw error; }
    });
    if (mounted.current) setWorking(false);
  }
  function clearBedProposal() {
    ++bedRequest.current;
    setBedProposal(null);
    setMessage("Bed proposal cleared; no project edit was submitted.");
  }
  async function prepareBed(occurrenceId: number, center: boolean) {
    if (!writable || !statusRef.current?.intent || changed || conflict) return;
    setFailure(null);
    const id = ++bedRequest.current;
    const capturedAuthority = currentAuthority;
    const capturedEnvelope = statusRef.current.intent.printer.build_envelope;
    setBedProposal(null);
    setWorking(true);
    try {
      const preparation = await hostRef.current.prepareBed(occurrenceId, center);
      if (!mounted.current || bedRequest.current !== id) return;
      if (!preparation.envelope_mm.min.every((value, axis) => value === capturedEnvelope.min[axis]) ||
          !preparation.envelope_mm.max.every((value, axis) => value === capturedEnvelope.max[axis]))
        throw new Error("Bed proposal belongs to a different printer envelope; prepare again.");
      setBedProposal({ preparation, authority: capturedAuthority, invalidated: false });
      setMessage("Read-only native bed proposal ready. Review contact evidence and preserved orientation, then explicitly Apply; no project edit has occurred.");
    } catch (error) {
      if (mounted.current && bedRequest.current === id) setMessage(String(error));
    } finally {
      if (mounted.current && bedRequest.current === id) setWorking(false);
    }
  }
  async function applyBed() {
    if (!writable || !bedProposal || bedProposalStale) return;
    setFailure(null);
    const snapshot = hostRef.current.snapshot();
    const liveAuthority = `${snapshot.phase === "running" ? snapshot.hello?.session_id ?? "" : ""}:${snapshot.project?.project_id ?? ""}:${snapshot.projectEpoch}`;
    if (bedProposal.authority !== liveAuthority) {
      setBedProposal(previous => previous ? { ...previous, invalidated: true } : previous);
      setMessage("Bed proposal belongs to a replaced project/session; prepare again.");
      return;
    }
    const captured = bedProposal;
    const id = ++bedRequest.current;
    setWorking(true);
    try {
      await hostRef.current.applyBed(captured.preparation);
      if (!mounted.current || bedRequest.current !== id) return;
      setBedProposal(null);
      setMessage("Reviewed bed pose applied in one authoritative SetInstancePose transaction; Core owns revision and Undo/Redo.");
    } catch (error) {
      if (mounted.current && bedRequest.current === id) {
        setBedProposal(previous => previous ? { ...previous, invalidated: true } : previous);
        setMessage(`${String(error)} Proposal invalidated; prepare again before applying.`);
      }
    } finally {
      if (mounted.current && bedRequest.current === id) setWorking(false);
    }
  }
  function selectSoftwareIntent() {
    setFailure(null);
    const next = makeDraft(softwareIntent, statusRef.current?.intent ?? null);
    draftRef.current = next;
    setDraft(next);
    setMessage(
      "Software-only reference selected explicitly. Apply commits this full intent once; no printer operation is submitted.",
    );
  }
  function editScalar(key: ScalarField, value: string) {
    setFailure(null);
    setMessage("");
    setDraft((previous) =>
      previous ? { ...previous, values: { ...previous.values, [key]: value } } : null,
    );
  }
  function editPolicyScalar(key: string, value: string) {
    setFailure(null);
    setMessage("");
    setDraft(previous => previous ? { ...previous, policyValues: { ...previous.policyValues, [key]: value } } : null);
  }
  function editWidthLimit(bound: "minimum" | "maximum", value: string) {
    setFailure(null);
    setMessage("");
    setDraft(previous => previous ? { ...previous, widthLimits: { ...previous.widthLimits, [bound]: value } } : null);
  }
  function editSparseInfill(kind: "fine" | "combine_eligible", maxHeight: string) {
    setFailure(null);
    setMessage("");
    setDraft(previous => previous ? { ...previous, sparseInfillKind: kind, sparseMaxHeight: maxHeight } : null);
  }
  function editSeamPolicy(value: SeamPolicy["kind"]) {
    setFailure(null);
    setMessage("");
    setDraft(previous => previous ? { ...previous, seamPolicyKind: value } : null);
  }
  function editPreferredDirection(axis: 0 | 1, value: string) {
    setFailure(null);
    setMessage("");
    setDraft(previous => {
      if (!previous) return null;
      const preferredDirection: [string, string] = [...previous.preferredDirection];
      preferredDirection[axis] = value;
      return { ...previous, preferredDirection };
    });
  }
  function editRectilinearAxis(value: RectilinearAxisPolicy) {
    setFailure(null);
    setMessage("");
    setDraft(previous => previous ? {
      ...previous,
      intent: { ...previous.intent, recipe: { ...previous.intent.recipe, rectilinear_axis: value } },
    } : null);
  }
  function editTopSkinAxis(value: RectilinearAxisPolicy) {
    setFailure(null);
    setMessage("");
    setDraft(previous => previous ? {
      ...previous,
      intent: { ...previous.intent, recipe: { ...previous.intent.recipe, top_skin_axis: value } },
    } : null);
  }
  function reloadDraft() {
    const intent = statusRef.current?.intent;
    const next = intent ? makeDraft(intent, intent) : null;
    draftRef.current = next;
    setDraft(next);
    setMessage("");
    setFailure(null);
  }
  async function perform(action: ManufacturingAction, output?: VerificationOutput) {
    if (!available || ((action === "apply" || action === "compile") && !writable)) return;
    if (action === "apply" && (!draftRef.current || !draftChanged(draftRef.current) || conflict))
      return;
    let appliedIntent: ManufacturingIntent | undefined;
    try {
      if (action === "apply") appliedIntent = draftIntent(draftRef.current!);
    } catch (error) {
      setMessage(String(error));
      setFailure({ action, message: String(error), revision, authority: currentAuthority });
      return;
    }
    const id = ++request.current;
    setWorking(true);
    setMessage("");
    setFailure(null);
    await hostRef.current.run(async (token) => {
      try {
        const before = await refresh(token);
        ensureRequest(token, id);
        const session = hostRef.current.snapshot().hello!.session_id;
        if (action === "apply" && draftRef.current!.base !== intentIdentity(before.intent))
          throw new Error(
            "Current intent changed. Reload the current intent before applying a new draft.",
          );
        const command =
          action === "apply"
            ? {
                op: "set_intent" as const,
                session_id: session,
                base_revision: before.info.revision,
                intent: appliedIntent!,
              }
            : action === "compile"
              ? { op: "compile" as const, session_id: session, base_revision: before.info.revision }
              : action === "preflight"
                ? { op: "preflight" as const, session_id: session, base_revision: before.info.revision }
              : {
                  op: "inspect" as const,
                  session_id: session,
                  base_revision: before.info.revision,
                  output: output!,
                };
        const response = await hostRef.current.exchange(() => native.manufacturing(command), token);
        ensureRequest(token, id);
        if (response.type === "error")
          throw new Error(`${response.error.code}: ${response.error.message}`);
        if (action === "apply") {
          if (response.type !== "status")
            throw new Error("Intent Apply did not return committed status.");
          statusRef.current = response;
          setStatus(response);
          const next = response.intent ? makeDraft(response.intent, response.intent) : null;
          draftRef.current = next;
          setDraft(next);
          await hostRef.current.refreshProject(token);
          setMessage(
            "Full manufacturing intent applied once. Project Undo, Save and Reopen use Core history/checkpoints.",
          );
          return;
        }
        if (action === "preflight") {
          const result = response.type === "preflight"
            ? { kind: "manufacturing_preflight" as const, report: response.report }
            : await hostRef.current.poll(response, token);
          ensureRequest(token, id);
          if (result.kind !== "manufacturing_preflight")
            throw new Error("Preflight returned another operation kind.");
          if (hostRef.current.snapshot().project?.revision !== result.report.input_revision)
            throw new Error("Preflight revision changed; run it again.");
          setPreflight(result.report);
          setMessage(`Preflight ${result.report.disposition}: captured required evidence is reported separately from pending Compile-only analysis; software-only and physically unqualified.`);
          return;
        }
        if (inspectionRef.current) {
          const historical = {
            ...inspectionRef.current,
            replay: null,
            displayReady: false,
            invalidated: true,
          };
          inspectionRef.current = historical;
          setInspection(historical);
          await hostRef.current.clearReplay();
          ensureRequest(token, id);
        }
        const result =
          response.type === "verified"
            ? { kind: "manufacturing_verified" as const, ...response }
            : await hostRef.current.poll(response, token);
        ensureRequest(token, id);
        await hostRef.current.refreshProject(token);
        const selected = await refresh(token);
        ensureRequest(token, id);
        if (action === "compile") {
          if (result.kind !== "manufacturing_compiled")
            throw new Error("Compile returned another operation kind.");
          setMessage(
            "Immutable software-only bundle compiled. Verify freshly to inspect or export; stored verified flags are not trusted.",
          );
          return;
        }
        if (result.kind !== "manufacturing_verified")
          throw new Error("Verify returned another operation kind.");
        if (
          !sameArtifact(selected.artifact, result.record) ||
          result.provenance.project_id !== selected.info.project_id
        )
          throw new Error("Fresh verification completed for a bundle that is no longer active.");
        let replay: DecodedReplayDisplay | null = null;
        let displayReady = false;
        const diagnostic = hostRef.current.snapshot().diagnosticSession;
        if (result.replay_display) {
          setAdmittingReplay(true);
          const descriptor = result.replay_display;
          const wasteCount = result.report.routing?.tools.reduce((sum, tool) => sum + tool.waste_deposition_segments, 0) ?? 0;
          if (
            descriptor.info.bundle_hash !== result.record.hash ||
            descriptor.info.input_hash !== result.record.input_hash ||
            descriptor.info.input_hash !== result.provenance.input_hash ||
            descriptor.info.verifier_version !== result.provenance.verifier_version ||
            descriptor.info.layer_count !== result.record.summary.layers ||
            descriptor.info.deposition_count !== result.report.deposition_segments + result.report.support_deposition_segments + wasteCount ||
            descriptor.info.travel_count !== result.report.travel_segments ||
            descriptor.info.lifecycle_count !== result.report.lifecycle_motion_count ||
            descriptor.info.process_count !== result.report.retraction_count * 2 - (result.report.routing ? 2 : 0) ||
            descriptor.info.tool_epoch_count !== (result.report.routing?.selection_count ?? 0) ||
            descriptor.info.waste_count !== wasteCount
          )
            throw new Error(
              "Replay descriptor is not bound to the freshly verified active bundle.",
            );
          const bytes = await native.replay(descriptor, result.record);
          ensureRequest(token, id);
          replay = await decodeReplayDisplay(bytes, descriptor);
          ensureRequest(token, id);
          try {
            if (!diagnostic) await hostRef.current.showReplay(descriptor, replay);
            // Neither decode nor GPU staging confers freshness.
            ensureRequest(token, id);
            const after = await refresh(token);
            ensureRequest(token, id);
            if (!sameArtifact(after.artifact, result.record))
              throw new Error("Bundle changed during replay admission.");
            displayReady = !diagnostic;
          } catch (error) {
            await hostRef.current.clearReplay();
            throw error;
          }
        } else {
          await hostRef.current.clearReplay();
        }
        const layers = await native.manufacturingLayers(result.record);
        ensureRequest(token, id);
        const afterLayers = await refresh(token);
        ensureRequest(token, id);
        if (!sameArtifact(afterLayers.artifact, result.record))
          throw new Error("Bundle changed during native layer accounting retrieval.");
        const next: Inspection = {
          result,
          session,
          request: id,
          replay,
          displayReady,
          diagnostic,
          authority: currentAuthority,
          invalidated: false,
          layers,
        };
        inspectionRef.current = next;
        setInspection(next);
        setLayer(null);
        setEventCount(replay?.eventCount ?? 0);
        setShowTravel(true);
        setShowSource(true);
        setColorByMaterial(false);
        setMessage(
          diagnostic && replay
            ? "Fresh actual-program report and validated replay bytes ready. Native diagnostic only: no GPU display, picking or hardware claim."
            : replay
              ? "Fresh actual emitted-program replay ready; millimetres, software-only, not machine-ready."
              : "Fresh report ready. Report-only verification allocates no motion display; request Report and motion to inspect coordinates.",
        );
      } catch (error) {
        try {
          hostRef.current.ensure(token);
          if (action === "inspect" && request.current === id && !inspectionRef.current?.displayReady)
            await hostRef.current.clearReplay();
          await hostRef.current.refreshProject(token);
          await refresh(token);
        } catch {
          /* Session teardown/supersession has no draft authority. */
        }
        if (mounted.current && request.current === id) {
          setMessage(String(error));
          setFailure({
            action, message: String(error),
            revision: hostRef.current.snapshot().project?.revision,
            authority: currentAuthority,
          });
        }
        throw error;
      }
    });
    if (mounted.current && request.current === id) setWorking(false);
    if (mounted.current) setAdmittingReplay(false);
  }
  async function updateView(
    nextLayer: number | null,
    nextCount: number,
    nextTravel: boolean,
    nextSource: boolean,
    nextMaterial: boolean = colorByMaterial,
  ) {
    if (!fresh || !inspection?.replay || working || changingView.current) return;
    const replay = inspection.replay;
    if (
      !Number.isInteger(nextCount) ||
      nextCount < 0 ||
      nextCount > replay.eventCount ||
      (nextLayer !== null &&
        (!Number.isInteger(nextLayer) || nextLayer < 0 || nextLayer >= replay.layerCount))
    ) {
      setMessage("Replay control outside the validated layer/event bounds.");
      return;
    }
    changingView.current = true;
    setWorking(true);
    try {
      if (inspection.displayReady)
        await hostRef.current.setReplayView({
          layer: nextLayer,
          eventCount: nextCount,
          showTravel: nextTravel,
          showSource: nextSource,
          toolColors: nextMaterial ? declaredToolColors : null,
        });
      if (mounted.current && inspectionRef.current?.request === inspection.request && !inspectionRef.current.invalidated) {
        // Describe the admitted draw, not an optimistic request that the
        // viewport's single-flight ownership may have refused.
        setLayer(nextLayer);
        setEventCount(nextCount);
        setShowTravel(nextTravel);
        setShowSource(nextSource);
        setColorByMaterial(nextMaterial && declaredToolColors !== null);
      }
    } catch (error) {
      if (mounted.current) setMessage(String(error));
    } finally {
      changingView.current = false;
      if (mounted.current) setWorking(false);
    }
  }
  async function exportBundle(relativePath?: string) {
    if (!available || !fresh || !inspection) return;
    setFailure(null);
    const captured = inspection;
    setWorking(true);
    setMessage("");
    await hostRef.current.run(async (token) => {
      try {
        const current = await refresh(token);
        if (!sameArtifact(current.artifact, captured.result.record))
          throw new Error("Verify the currently selected bundle before exporting.");
        const selection = await hostRef.current.exchange(
          () =>
            relativePath === undefined
              ? native.selectManufacturingExport()
              : native.debugSelectManufacturingExport(relativePath),
          token,
        );
        if (!selection) {
          setMessage("Export selection cancelled; no files written.");
          return;
        }
        const selected = await refresh(token);
        if (
          !sameArtifact(selected.artifact, captured.result.record) ||
          selection.session_id !== captured.session ||
          inspectionRef.current?.request !== captured.request
        )
          throw new Error("Export selection superseded; verify again.");
        const receipt = await native.exportManufacturing(selection, captured.result.record);
        hostRef.current.ensure(token);
        setMessage(
          `Committed ${receipt.files.join(", ")} to ${receipt.label}. Software-only; no machine submission.`,
        );
      } catch (error) {
        if (mounted.current) setMessage(String(error));
        throw error;
      }
    });
    if (mounted.current) setWorking(false);
  }
  function cancelPreview() {
    ++request.current;
    setWorking(false);
    setAdmittingReplay(false);
    setMessage(
      "Replay transfer/admission cancelled. Completed immutable native output is historical, not an active preview.",
    );
    void hostRef.current.clearReplay().catch((error: unknown) => {
      if (mounted.current) setMessage(String(error));
    });
  }

  return {
    status,
    draft,
    inspection,
    preflight,
    failure: failure?.authority === currentAuthority && failure.revision === revision ? failure : null,
    selectIntent,
    prepareBed,
    bedProposal: bedProposal?.preparation ?? null,
    bedProposalStale,
    applyBed,
    clearBedProposal,
    fresh,
    message,
    working,
    changed,
    conflict,
    available,
    writable,
    layer,
    eventCount,
    showTravel,
    showSource,
    colorByMaterial,
    declaredToolColors,
    selectSoftwareIntent,
    editScalar,
    editPolicyScalar,
    editWidthLimit,
    editSeamPolicy,
    editSparseInfill,
    editPreferredDirection,
    editRectilinearAxis,
    editTopSkinAxis,
    reloadDraft,
    apply: () => perform("apply"),
    compile: () => perform("compile"),
    preflightIntent: () => perform("preflight"),
    inspect: (output: VerificationOutput) => perform("inspect", output),
    updateView,
    exportBundle,
    cancelPreview,
    admittingReplay,
  };
}
