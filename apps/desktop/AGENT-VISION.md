<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop vision

Provide thin durable native authoring and BREP inspection over protocol 4: exact imported STEP assets, shared repeated instances and rigid placement, bounded undo/redo, explicit save/open/recovery and read-only committed snapshots. Core owns project transactions/storage; engine owns native caches/revisions; React owns transient editing/synchronization state; WebGPU owns disposable approximations.

Start with a new unattached project; first save creates a new project directory, subsequent save checkpoints attached storage without clearing session undo. New/open/recover require explicit dirty-discard intent. Restart explicitly reopens the attached saved checkpoint, not unsaved edits; persistent references survive while session handles/display expire. Supported input remains `step-planar-cylindrical-v1`, not general STEP product-tree import, topology editing, slicing or manufacturing execution.

The shared engine protocol also describes software-only manufacturing capabilities and common manufacturing job results/errors. The desktop consumes those contracts exhaustively without exposing manufacturing controls, adding invokes or claiming machine-ready execution.

Cancellation remains responsive while native work proceeds. Display admission is transactional and bounded: a paused scene synchronization clearly distinguishes engine revision from stale displayed geometry and offers resume without reimport; a paused section transfer retains the prior labelled overlay and interactive scene. Repeated objects share geometry allocations, and source-face identity—not a float32 hit—is inspected natively.

Browser-only use and missing WebGPU are honest unsupported states. Privileged fixture admission is startup-gated, confined and uses the real production bridge. Implemented controls/typechecking/bridge calls do not certify visible authoring, physical picking or native dialog behavior: those gates remain unverified until exercised on declared WebGPU reference hardware. Packaging certification is separate.
