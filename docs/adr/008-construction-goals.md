<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# ADR008: construction preferences and protected requirements

Status: adopted for the bounded physical layer-height workflow.

A preferred construction value can be unrealizable under captured material and
machine declarations without making the part unusable. Preserve the request and
explain a valid alternative. A verified program need not meet every preference;
it must satisfy the policy actually captured with the job.

The recipe therefore distinguishes a preferred layer height from a protected
maximum. Contracts resolve the preference against the effective deposition
height range and bead width without rewriting either declaration. The planner
distributes admissible physical intervals between native/field boundaries;
preferred layers may be thicker than the selected nominal height when that is
necessary to avoid a subminimum remainder. Protected maxima retain strict upper
bounds. Neither policy waives native material, holes, shape error, anchors or
command/process admission. Fixed-dual physical slots retain their separate exact
height contract and require the protected policy.

The typed assessment records the original target, selected nominal, limiting
reason, command-projected physical range and affected layer count/index. Fresh
independent replay checks command correspondence and recomputes this assessment;
stored assessments are not proof. The existing report, artifact summary and four
export files carry it through Core-owned history and `.spil` recovery. No second
protocol, warning store or new crate is introduced.

The native editor offers immediate draft resolution through the pure shared
contract. Applying still commits once; geometry and program work remain async
operations. Review separates verified output from an adjusted preference and
lets the operator inspect the first differing physical layer without a warning
dialog. Stale results cannot authorize export.

Rejecting every unmet preference loses useful output. Silently clamping the
stored request loses user intent. Downgrading every failure to a warning loses
the distinction between a known compromise and unknown/invalid construction.
This decision adopts explicit policy and retained assessments instead.

Additional material is not a strength guarantee. Interior thickness, density,
cost and structural goals require their own measured scopes and consuming
workflows before adoption; this decision does not implement a general optimizer,
arbitrary feature-protection authoring or physical qualification. The executable
contract and [manufacturing protocol](../protocol/manufacturing.md) define the
implemented limits. Runtime acceptance remains an independent evidence gate.
