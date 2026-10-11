<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable project contract vision

Provide one Rust-defined language for engine-owned recoverable authoring: immutable source-backed shared definitions, rigid occurrences, persistent identity, monotonic transactions, saved checkpoints, bounded undo and read-only snapshots. CLI and desktop consume generated contracts rather than invent project models.

The format is deliberately pre-alpha and changes through clean cutovers. Current authoring covers import, occurrence edits and explicit software-only manufacturing intent/artifact references, not general CAD topology changes or printer execution. Core owns persistence; compiler/replay behavior and real save/open/native-client evidence—not schema declarations—establish support.
