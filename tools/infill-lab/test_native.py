# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""A fresh saved-project inspection must not depend on trial input copies."""

import json
import pytest
from pathlib import Path

from model import Candidate
from native import Native


@pytest.mark.parametrize("tamper", [None, "program.gcode", "plan.json", "provenance.json", "verification.json"])
def test_compile_removes_only_owned_inputs_before_saved_return_and_export(tmp_path,monkeypatch,tamper):
    import native

    source=tmp_path/'original.step';source.write_bytes(b'original source remains owned by caller')
    intent=tmp_path/'base.json'
    intent.write_text(json.dumps(dict(printer=dict(workflow=dict(job=dict(park_xy_mm=[0,0]))),recipe={})))
    binaries=tmp_path/'bin';binaries.mkdir()
    for name in ('spiling-cli','spiling-engine'):
        (binaries/name).write_bytes(name.encode())
    folder=tmp_path/'trial';seen=[]

    def run(command,**kwargs):
        if command[0]=='node':
            (folder/'roads.json').write_text(json.dumps(dict(report=dict(verified=True))))
            return b'',0.
        operation=command[2];seen.append(operation)
        if operation=='create':
            assert (folder/'source.step').exists() and (folder/'intent.json').exists()
            (folder/'project.spil').write_bytes(b'saved project')
            return json.dumps(dict(definitions=[dict(record=dict(definition_id='definition'))])).encode(),0.
        if operation=='inspect':
            assert command.index('--manufacturing-out') < command.index('--replay-out')
            assert not (folder/'source.step').exists()
            assert not (folder/'intent.json').exists()
            assert source.exists() and intent.exists()
        export=Path(command[command.index('--manufacturing-out')+1]);export.mkdir()
        for name in ("program.gcode", "plan.json", "provenance.json", "verification.json"):
            (export/name).write_bytes(b'changed' if operation == 'inspect' and name == tamper else name.encode())
        (export/'manifest.json').write_text('{}')
        return b'{}',0.

    monkeypatch.setattr(native,'run',run)
    if tamper:
        with pytest.raises(RuntimeError, match=f"fresh export differs: {tamper}"):
            Native(binaries,intent).compile(source,(48,48,48),Candidate(),folder)
        return
    result=Native(binaries,intent).compile(source,(48,48,48),Candidate(),folder)
    assert seen==['create','edit','inspect']
    assert result['source_independent_reopen']
    assert result['fresh_export_manifest_sha256']
    assert len(result['fresh_export_sha256']) == 4
    assert source.read_bytes()==b'original source remains owned by caller'
