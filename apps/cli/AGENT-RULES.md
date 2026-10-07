<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# CLI engine consumer rules

- Use the shared generated client exclusively. Diagnose observes engine readiness; triangle downloads the real diagnostic artifact; job watches a long-running operation and incrementally fetches its outputs.
- Optional --store retains operations across runs; default storage is temporary. --request-id follows AIP-155; do not invent automatic retry with a new ID.
- stdout is final machine-readable JSON after successful child shutdown. Progress/errors go to stderr. Failed output writes clean up the child. Native paths retain OS bytes; JSON labels use explicit display encoding.
- Run node --import tsx tools/smoke-cli.mjs against built binaries, plus real engine integration tests.
