<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Supervised native client vision

Provide the same bounded protocol-v4 supervision to native CLI and desktop consumers: handshake, explicit diagnostics, shared geometry/project/manufacturing job control, bounded artifact pulls, observed status and awaited shutdown/interruption. Recoverable typed domain failures and fully received corrupt manufacturing bundles leave the session available; corrupt or interrupted transport never does. Manufacturing pulls establish content/schema/provenance identity, not independent emitted-program replay. The client owns no native definitions, project domain/storage, job scheduler, source cache, planner, replay or competing frontend watchdog.
