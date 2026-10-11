<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Session deadline vision

Keep an authenticated gRPC engine authoring/evaluation session supervised even when no requests arrive, storage is syncing or an upstream native call does not yield. One deadline owner requests cooperative cancellation, shares the native/durable commit linearization boundary and interrupts an unresponsive worker with observable process exit. It supervises native execution only; the durable Jobs ledger owns public Google Operations. Clients must reap the child and explicitly begin a fresh session or reopen a committed project.

This is not a general scheduler, persistent job queue or alternate process supervisor. Actual native child cancellation/interruption evidence governs acceptance; allocator-level caps and upstream mid-call preemption are not claimed.
