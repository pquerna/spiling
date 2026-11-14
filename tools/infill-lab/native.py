# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Bounded native CLI trials. Search proposes inert public process edits only."""

from __future__ import annotations
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
from model import Candidate, file_hash

ROOT = Path(__file__).resolve().parents[2]
_TRIAL_GROUP = None


def enter_trial_group():
    """Only the fresh session-leading worker may own this cancellation group."""
    global _TRIAL_GROUP
    if os.getpgrp() != os.getpid():
        raise RuntimeError("trial worker must lead its own process group")
    _TRIAL_GROUP = os.getpid()


def run(command, *, timeout=120, cwd=ROOT, output: Path | None = None):
    env = {
        **os.environ,
        "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "1"),
        "OPENBLAS_NUM_THREADS": os.environ.get("OPENBLAS_NUM_THREADS", "1"),
        "OMP_NUM_THREADS": os.environ.get("OMP_NUM_THREADS", "1"),
    }
    started = time.monotonic()
    owns_group = _TRIAL_GROUP is None
    import tempfile

    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        with subprocess.Popen(
            list(map(str, command)),
            cwd=cwd,
            env=env,
            stdout=stdout,
            stderr=stderr,
            start_new_session=owns_group,
        ) as p:
            try:
                while p.poll() is None:
                    if time.monotonic() - started > timeout:
                        raise TimeoutError(f"command exceeded {timeout:.1f}s")
                    if (
                        os.fstat(stdout.fileno()).st_size
                        + os.fstat(stderr.fileno()).st_size
                        > 32 * 1024 * 1024
                    ):
                        raise RuntimeError("native receipt exceeds output budget")
                    time.sleep(0.025)
                if (
                    os.fstat(stdout.fileno()).st_size
                    + os.fstat(stderr.fileno()).st_size
                    > 32 * 1024 * 1024
                ):
                    raise RuntimeError("native receipt exceeds output budget")
            except BaseException as error:
                # The group includes the supervised sidecar; do not leave an orphan.
                if not owns_group:
                    print(
                        f"trial process group cancelled: {error}",
                        file=sys.stderr,
                        flush=True,
                    )
                    os.killpg(_TRIAL_GROUP, signal.SIGKILL)
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                p.wait()
                raise
            finally:
                # The worker's CLI/sidecar descendants share this group. Kill
                # remaining descendants even after a worker exits successfully.
                if owns_group:
                    try:
                        os.killpg(p.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
        stdout.seek(0)
        stderr.seek(0)
        out, err = stdout.read(), stderr.read()

    if output:
        output.with_suffix(".stdout.json").write_bytes(out)
        output.with_suffix(".stderr.txt").write_bytes(err)
    if p.returncode:
        detail = err.decode(errors="replace")
        try:
            diagnostic = json.loads(detail.splitlines()[-1])
            detail = json.dumps(
                {
                    k: diagnostic[k]
                    for k in ("domain", "message", "error")
                    if k in diagnostic
                }
            )
        except (ValueError, IndexError, TypeError):
            pass
        raise RuntimeError(f"command exit {p.returncode}: {detail[-5000:]}")
    return out, time.monotonic() - started


@dataclass
class Native:
    bin_dir: Path
    base_intent: Path
    timeout: int = 120

    def compile(self, source: Path, size, candidate: Candidate, folder: Path):
        folder.mkdir(parents=True, exist_ok=False)
        owned_source = folder / "source.step"
        shutil.copyfile(source, owned_source)
        source_hash = file_hash(owned_source)
        intent = json.loads(self.base_intent.read_text())
        recipe = intent["recipe"]
        recipe.update(
            infill_fraction=candidate.fraction,
            rectilinear_axis=candidate.axis,
            supports={"kind": "disabled"},
            layer_height_mm=candidate.layer_height,
            sparse_infill=candidate.sparse_policy(),
        )
        # Preserve walls/skins/process from the captured public base recipe.
        intent_file = folder / "intent.json"
        intent_file.write_text(json.dumps(intent))
        created, create_time = run(
            [
                self.bin_dir / "spiling-cli",
                "project",
                "create",
                folder / "project.spil",
                "--import",
                owned_source,
                "--pose",
                "1",
                json.dumps(
                    {"translation_mm": [20, 20, 0], "rotation_xyzw": [0, 0, 0, 1]}
                ),
                "--intent",
                intent_file,
                "--save",
                "--engine",
                self.bin_dir / "spiling-engine",
            ],
            timeout=self.timeout,
            output=folder / "create",
        )
        definition = json.loads(created)["definitions"][0]["record"]["definition_id"]
        scope = {
            "kind": "occurrence",
            "occurrence_id": 1,
            "expected_definition_id": definition,
        }
        args = ["project", "edit", folder / "project.spil"]
        edits = []

        def box(lo, hi, fraction, permission):
            if permission == "separator-wall":
                interface = {"kind": "separator_wall"}
            else:
                interface = {
                    "kind": "horizontal_diaphragm",
                    "thickness_mm": float(permission.split(":")[1]),
                }
            edit = {
                "op": "add_layer",
                "scope": scope,
                "selector": {"kind": "source_box_mm", "min": lo, "max": hi},
                "action": {
                    "kind": "replace",
                    "value": {"kind": "sparse_fraction", "fraction": fraction},
                },
                "interface": interface,
            }
            file = folder / f"edit-{len(edits)}.json"
            file.write_text(json.dumps(edit))
            edits.append(edit)
            return ["--process-edit", json.dumps(edit)]

        x, y, z = size
        if candidate.family in {"crown", "flanges"}:
            plane = round(z * candidate.cut / 0.2) * 0.2
            args += box(
                [-1, -1, plane],
                [x + 1, y + 1, z + 1],
                candidate.region_fraction,
                f"horizontal-diaphragm:{candidate.plate}",
            )
            if candidate.family == "flanges":
                low = round((z - plane) / 0.2) * 0.2
                args += box(
                    [-1, -1, -1],
                    [x + 1, y + 1, low],
                    candidate.region_fraction,
                    f"horizontal-diaphragm:{candidate.plate}",
                )
        elif candidate.family == "separator":
            axis = 0 if candidate.axis == "x" else 1
            lo, hi = [-1.0, -1.0, -1.0], [x + 1.0, y + 1.0, z + 1.0]
            lo[axis] = size[axis] * candidate.cut
            args += box(lo, hi, candidate.region_fraction, "separator-wall")
        args += [
            "--compile",
            "--verify",
            "--manufacturing-out",
            folder / "bundle",
            "--save",
            "--engine",
            self.bin_dir / "spiling-engine",
        ]
        out, elapsed = run(
            [self.bin_dir / "spiling-cli", *args],
            timeout=self.timeout,
            output=folder / "compile",
        )
        # Inspection is a new engine, with only the saved PTAC as authority.
        # Remove this trial's copies before opening, not after scoring/export.
        owned_source.unlink()
        intent_file.unlink()
        receipt = folder / "receipt.json"
        out, replay_time = run(
            [
                self.bin_dir / "spiling-cli",
                "project",
                "inspect",
                folder / "project.spil",
                "--verify",
                "--manufacturing-out",
                folder / "fresh-bundle",
                # Keep ReportAndMotion last: ReportOnly export deliberately
                # replaces the root receipt's transient display descriptor.
                "--replay-out",
                folder / "actual.splr",
                "--engine",
                self.bin_dir / "spiling-engine",
            ],
            timeout=self.timeout,
            output=folder / "inspect",
        )
        receipt.write_bytes(out)
        exports = ("program.gcode", "plan.json", "provenance.json", "verification.json")
        fresh_exports = {name: file_hash(folder / "fresh-bundle" / name) for name in exports}
        for name, digest in fresh_exports.items():
            if digest != file_hash(folder / "bundle" / name):
                raise RuntimeError(f"source-absent fresh export differs: {name}")
        fresh_program = fresh_exports["program.gcode"]
        run(
            [
                "node",
                "--experimental-strip-types",
                ROOT / "tools/infill-lab/export-replay.mjs",
                receipt,
                folder / "actual.splr",
                folder / "roads.json",
            ],
            timeout=30,
        )
        data = json.loads((folder / "roads.json").read_text())
        data.update(
            candidate=candidate.data(),
            source_sha256=source_hash,
            size_mm=size,
            compile_wall_s=elapsed + create_time + replay_time,
            process_edits=edits,
            source_independent_reopen=True,
            fresh_export_manifest_sha256=file_hash(folder / "fresh-bundle/manifest.json"),
            fresh_export_program_sha256=fresh_program,
            fresh_export_sha256=fresh_exports,
            bin_sha256={
                x.name: file_hash(x)
                for x in [self.bin_dir / "spiling-cli", self.bin_dir / "spiling-engine"]
            },
        )
        (folder / "roads.json").write_text(json.dumps(data))
        return data

    def reopen(self, folder: Path):
        # Destructive only to this trial's owned input copies; the project must stand alone.
        for name in ("source.step", "intent.json"):
            (folder / name).unlink(missing_ok=True)
        old = file_hash(folder / "actual.splr")
        run(
            [
                self.bin_dir / "spiling-cli",
                "project",
                "inspect",
                folder / "project.spil",
                "--verify",
                "--manufacturing-out",
                folder / "returned-bundle",
                "--replay-out",
                folder / "returned.splr",
                "--engine",
                self.bin_dir / "spiling-engine",
            ],
            timeout=self.timeout,
            output=folder / "reopen",
        )
        if file_hash(folder / "returned.splr") != old:
            raise RuntimeError("saved fresh replay differs from candidate")
        program = file_hash(folder / "bundle/program.gcode")
        if file_hash(folder / "returned-bundle/program.gcode") != program:
            raise RuntimeError("saved fresh export differs from candidate program")
        return {"source_independent_reopen": True, "replay_sha256": old,
                "program_sha256":program,
                "export_manifest_sha256":file_hash(folder / "returned-bundle/manifest.json")}
