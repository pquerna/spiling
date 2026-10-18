<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Process policy contract rules

Own required schema7 process, external-E retraction, breakaway support and bounded preflight data. No implicit serde defaults or machine execution. ProcessPolicy.max_bridge_span_mm is the sole bridge-span authority; support policy must not duplicate it.

Validate finite positive speed/flow/span factors, speed factors in (0,1], integral PWM bytes, explicit disabled/enabled retraction and disabled/breakaway supports. Recipe/printer validation owns scaled feed/flow and actual serialization admission; enabled supports require generated_supports capability. External retraction is stationary absolute E, not firmware G10/G11, deposition or priming.

Preflight has at most256 checks admitted before vector growth, bounded codes/messages, captured revision and software_only=true. Every check carries explicit required and evidence: not_evaluated/certified/unsupported/indeterminate/failed. Required unsupported/failed evidence refuses even if another required check is unresolved; otherwise required not_evaluated/indeterminate evidence yields unresolved; only all-certified required checks admit. Compile-only checks are not required. Severity is presentation, not evidence. There is no eligibility boolean. Adapters reapply validate(); disposition establishes neither native section/support/bridge success nor physical qualification.

Exercise strict missing-field, nonfinite, feed/flow, precision, capability and resource rejection through contracts tests. Parent owns verification commands.
