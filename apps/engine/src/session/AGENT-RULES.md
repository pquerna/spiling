<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Session deadline rules

Own one bounded watchdog state and Condvar thread, independent of gRPC requests, the dedicated native dispatcher and kernel/storage execution. Accepted native work begins a 60-second deadline. First cancellation request begins one ten-second grace, never resets it. A missing terminal/promotion ACK at grace expiry emits `job_cancel_deadline` on stderr and exits 70; no successful cancellation is fabricated.

The watchdog mutex serializes cancellation against promotion and save's durable commit decision. A cloned job-bound CommitToken exposes that same lock to the native worker before manifest replacement. Before the decision, set the cooperative flag; afterward, report committing and retain the ACK deadline, never promise rollback. A disk commit receipt must be published. Completion clears the one active record. No timer/event queue, frontend native-work deadline or automatic replay.

Private native JobId correlation belongs to the engine geometry runtime; public session/domain DTOs belong to contracts. Scene publication follows geometry dispatcher ACK/core commit, then the single durable Jobs ledger publishes the operation outcome. Tests must observe actual child/PID failure and cleanup; mocked timers alone do not establish runtime responsiveness.
