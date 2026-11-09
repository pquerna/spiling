# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Strict trial identity and independent finite-fixture controls."""

import json
from pathlib import Path

import pytest

from cathedral import fixture, seeds
from model import Candidate
from vault_mechanics import web_response


def test_architecture_lineage_roundtrips_without_changing_old_candidate_identity():
    proposed = list(seeds())
    assert [p["name"] for p in proposed] == ["flat", "windows", "pier_ribs", "windows_and_ribs"]
    assert len({Candidate(**p["candidate"]).identity for p in proposed}) == 4
    for p in proposed:
        c = Candidate(**p["candidate"])
        assert Candidate(**json.loads(json.dumps(c.data()))).data() == c.data()
        a = c.sparse_policy()["architecture"]
        assert (a is None) == (p["name"] == "flat")
    assert "web_opening" not in Candidate().data()
    assert "pier_rib_depth_mm" not in Candidate().data()
    with pytest.raises(ValueError, match="Vault"):
        Candidate(web_opening=(2,10,4,4,1.8))
    with pytest.raises(ValueError, match="cap"):
        Candidate(pattern="web_roof", web_opening=(2,10,4,3,1.8))


def test_actual_control_freezes_a_complete_window_and_two_pier_crop():
    roads = []
    for fixed in (0, 1):
        for c in (24.,40.,56.):
            a,b = [20.,20.,1.],[68.,68.,1.]
            a[fixed] = b[fixed] = c
            roads.append(dict(start=a,end=b,kind=5,owner=0,width=0.45,upper=1.))
    bounds = fixture(dict(roads=roads))
    assert bounds[0][0] < 24 < 40 < bounds[1][0]
    assert bounds[0][2] < 2 < 12 < bounds[1][2]
    assert bounds[0][1] < 40 < bounds[1][1]


def test_web_fixture_reuses_one_assembly_and_keeps_physical_gates(monkeypatch):
    monkeypatch.syspath_prepend(str(Path(__file__).parent / "remote"))
    from dataclasses import asdict
    from model import Material

    # Homogeneous nominal material, independently known axial stiffness EA/L.
    roads = [dict(start=[20.,y,1.],end=[24.,y,1.],width=.5,lower=0.,upper=1.,kind=5,owner=0)
             for y in (21.25,21.75)]
    scenario = dict(bounds=[[21.,21.,0.],[23.,22.,1.]],
                    cases=["compression_x","compression_z","cantilever_z_y"],
                    material=asdict(Material(z_ratio=.25)),solver="superlu_mmd",
                    refinement=dict(max_xy=1.,max_z=.5,divisions=1))
    result = web_response(dict(roads=roads),scenario)
    assert result["stiffness_n_mm"]["compression_x"] == pytest.approx(2300./2., rel=1e-7)
    assert result["stiffness_n_mm"]["compression_z"] == pytest.approx(2300.*.25*2., rel=1e-7)
    assert not result["nonspanning_cases"]
    assert all(r["matrix_reused"] and r["energy_error"] < 1e-7 for r in result["components"])


def test_collection_keeps_timeout_unknown_and_rejects_cleanup_and_bad_residual(tmp_path):
    from cathedral import CASES, collect
    from model import digest, file_hash

    def write(path, data):
        path.write_text(json.dumps(data))

    inputs, results = tmp_path/"inputs", tmp_path/"results"
    inputs.mkdir(); results.mkdir()
    candidate = next(seeds())["candidate"]
    scenario = dict(bounds=[[0,0,0],[1,1,1]], cases=list(CASES), refinement=dict(divisions=1))
    data = dict(report=dict(deposited_volume_mm3=1.), motion_seconds_lower_bound=2.)
    temp = tmp_path/"roads.json"; write(temp,data)
    case = dict(kind="cathedral_web_bay", judge={}, scenario=scenario, candidate=candidate,
                roads_sha256=file_hash(temp), source_sha256="source", native_capture_id="capture")
    key = digest(case)
    source, dest = inputs/key, results/key
    source.mkdir(); dest.mkdir()
    write(source/"case.json",case); write(source/"roads.json",data)
    write(inputs/"jobs.json",dict(jobs=[dict(id=key,candidate=candidate)]))
    write(inputs/"cohort.json",dict(judge={},bounds=scenario["bounds"],divisions=1))
    image = tmp_path/"image.json"; write(image,dict(id="image"))
    launch = dict(removed=False,image=dict(id="image"),docker_exit=124)
    write(dest/"launch.json",launch); write(dest/"dispatch.json",dict(job=key,exit_code=124))
    with pytest.raises(ValueError, match="container"):
        collect(inputs,results,image,tmp_path/"bad.json")
    launch["removed"] = True; write(dest/"launch.json",launch)
    collect(inputs,results,image,tmp_path/"failed.json")
    archive=json.loads((tmp_path/"failed.json").read_text())
    assert archive["frontier"] == []
    assert archive["rows"][0]["status"] == "failed"
    assert "stiffness_n_mm" not in archive["rows"][0]
    bad = dict(case,status="ok",input_sha256=file_hash(source/"case.json"),
               stiffness_n_mm=dict.fromkeys(CASES,1.),nonspanning_cases=[],
               components=[dict(case=c,stiffness_n_mm=1.,relative_residual=.1,
                                energy_error=0.,force_balance=0.) for c in CASES])
    (dest/"trials.jsonl").write_text(json.dumps(bad)+"\n")
    launch["docker_exit"]=0; write(dest/"launch.json",launch)
    write(dest/"dispatch.json",dict(job=key,exit_code=0))
    with pytest.raises(ValueError, match="numerical gate"):
        collect(inputs,results,image,tmp_path/"bad-numerics.json")


def test_nested_cap_resolution_has_declared_error_and_preserves_default_geometry():
    import math
    import numpy as np
    import shapely
    from geometry import footprint_slabs, prism_mesh

    roads = [dict(start=[0.,0.,.2],end=[2.,0.,.2],width=.45,lower=0.,upper=.2)]
    previous = None
    for q in (4,8,16,32):
        _,_,domains,_ = footprint_slabs(roads,cap_segments=q)
        shape = domains[0]
        if previous is not None:
            assert shapely.area(shapely.difference(previous,shape)) < 1e-9
            assert shape.area > previous.area
        previous = shape
        _,info = prism_mesh(roads,cap_segments=q)
        assert info["maximum_cap_sagitta_mm"] == pytest.approx(.225*(1-math.cos(math.pi/(4*q))))
    ordinary,info = prism_mesh(roads)
    explicit,explicit_info = prism_mesh(roads,cap_segments=32)
    assert np.array_equal(ordinary.points,explicit.points)
    assert np.array_equal(ordinary.cells,explicit.cells)
    assert info == explicit_info
    with pytest.raises(ValueError,match="resolution"):
        prism_mesh(roads,cap_segments=3)
