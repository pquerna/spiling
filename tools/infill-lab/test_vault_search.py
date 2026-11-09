# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Search retains small wins and weak-axis diversity without accepting stale evidence."""

from dataclasses import asdict
import math

import pytest

from model import Material, digest
from vault_search import frontier, geometric_mean, proposals
from vault_mechanics import patch_response


def row(key, material, response, **changes):
    return dict(
        capture_id=key,
        candidate=dict(
            family="uniform",
            fraction=0.1,
            pattern="web_roof",
            axis="alternating_xy",
            layer_height=0.2,
            combine_height=0,
        ),
        volume_mm3=material,
        motion_s=material,
        stiffness_n_mm=dict(zip("xyz", response)),
        status="ok",
        **changes,
    )


def test_small_improvements_and_directional_tradeoffs_are_retained():
    baseline = row("parent", 100, [10, 10, 10])
    small = row("small", 99.99, [10.001, 10, 10])
    directional = row("directional", 99, [11, 11, 9])
    assert {r["capture_id"] for r in frontier([baseline, small, directional])} == {
        "small",
        "directional",
    }
    assert geometric_mean([1, 8, 8]) == pytest.approx(4)
    assert geometric_mean([0, 8, 8]) == 0
    with pytest.raises(ValueError):
        geometric_mean([math.nan, 8, 8])


def test_fifty_unique_vault_proposals_keep_controls_and_parentage():
    seeds = proposals(0, [])
    assert sum(r["candidate"]["pattern"] == "web_roof" for r in seeds) == 10
    assert sum(r["reason"].endswith("control") for r in seeds) == 7
    history = []
    for n, proposal in enumerate(seeds):
        history.append(
            dict(row(str(n), 100 + n, [10, 10, 10]), candidate=proposal["candidate"])
        )
    all_proposals = list(seeds)
    for round_number in range(1, 5):
        batch = proposals(round_number, history)
        assert len(batch) == 10
        if round_number < 4:
            assert all(p["parent"] in {r["capture_id"] for r in history} for p in batch)
        for n, proposal in enumerate(batch):
            history.append(
                dict(
                    row(f"{round_number}-{n}", 99 + n, [10, 10, 10]),
                    candidate=proposal["candidate"],
                )
            )
        all_proposals += batch
    assert len({digest(p["candidate"]) for p in all_proposals}) == 57
    assert sum(p["reason"] == "local_mutation" for p in all_proposals) == 30


def test_real_nominal_roof_patch_matches_homogeneous_weak_z_solution():
    # Two overlapping deposited lanes fill the finite 1 mm square patch; five
    # physical intervals make a 1 mm height. No synthetic lid is inserted.
    roads = []
    for layer in range(5):
        for y in (21.25, 21.75):
            roads.append(
                dict(
                    start=[20, y, 1.2 + 0.2 * layer],
                    end=[24, y, 1.2 + 0.2 * layer],
                    width=0.5,
                    lower=1 + 0.2 * layer,
                    upper=1.2 + 0.2 * layer,
                )
            )
    material = Material(z_ratio=0.25)
    scenario = dict(
        bounds=[[21, 21, 1], [22, 22, 2]],
        material=asdict(material),
        solver="p_amg",
        refinement=dict(max_xy=1, max_z=0.2, divisions=1),
    )
    result = patch_response(dict(roads=roads, size_mm=[4, 4, 2]), scenario)
    # The inherited tensor applies sqrt(z_ratio) on each Z strain factor.
    assert result["stiffness_n_mm"]["x"] == pytest.approx(material.young_mpa, rel=1e-6)
    assert result["stiffness_n_mm"]["y"] == pytest.approx(material.young_mpa, rel=1e-6)
    assert result["stiffness_n_mm"]["z"] == pytest.approx(
        material.young_mpa * material.z_ratio, rel=1e-6
    )
    assert all(c["force_balance"] < 1e-5 for c in result["components"])
    scenario["solver"] = "superlu_mmd"
    direct = patch_response(dict(roads=roads, size_mm=[4, 4, 2]), scenario)
    assert direct["stiffness_n_mm"] == pytest.approx(result["stiffness_n_mm"], rel=1e-7)
    scenario["bounds"][1][2] = 1.9
    with pytest.raises(ValueError, match="actual top skin"):
        patch_response(dict(roads=roads, size_mm=[4, 4, 2]), scenario)


def test_collector_retains_parentage_and_refusals_but_rejects_bad_residuals(tmp_path):
    import json
    from types import SimpleNamespace
    from model import file_hash
    from vault_search import collect

    candidate = proposals(0, [])[7]["candidate"]
    case = dict(
        candidate=candidate,
        judge={"immutable": "judge"},
        scenario={"fixed": "fixture"},
        source_sha256="source",
        roads_sha256="roads",
        binaries={"engine": "binary"},
    )
    key = digest(case)
    inputs, results = tmp_path / "input", tmp_path / "results"
    (inputs / key).mkdir(parents=True)
    (results / key).mkdir(parents=True)

    def save(path, data):
        path.write_text(json.dumps(data))

    save(inputs / key / "case.json", case)
    save(
        inputs / "jobs.json",
        dict(
            judge=case["judge"],
            jobs=[dict(id=key, capture_id="native", candidate=candidate)],
            refused=[dict(id="refused", candidate=candidate, error="unsupported roof")],
        ),
    )
    save(
        results / key / "launch.json",
        dict(removed=True, docker_exit=0, image=dict(id="image")),
    )
    save(results / key / "dispatch.json", dict(exit_code=0))
    response = dict(
        input_sha256=file_hash(inputs / key / "case.json"),
        judge=case["judge"],
        scenario=case["scenario"],
        candidate=candidate,
        roads_sha256="roads",
        source_sha256="source",
        status="ok",
        components=[
            dict(relative_residual=1e-9, energy_error=1e-9, force_balance=1e-9)
        ],
        stiffness_n_mm=dict(x=10, y=10, z=10),
        volume_mm3=10,
        motion_s=10,
    )
    save(results / key / "trials.jsonl", response)
    provenance = tmp_path / "proposals.json"
    save(
        provenance,
        dict(
            proposals=[
                dict(candidate=candidate, parent="previous", reason="local_mutation")
            ]
        ),
    )
    args = SimpleNamespace(
        input=inputs,
        results=results,
        proposals=provenance,
        history=None,
        output=tmp_path / "archive.json",
    )
    collect(args)
    rows = json.loads(args.output.read_text())["rows"]
    assert rows[0]["parent"] == "previous"
    assert rows[1]["stage"] == "native" and rows[1]["status"] == "failed"
    response["components"][0]["energy_error"] = 0.1
    save(results / key / "trials.jsonl", response)
    args.output = tmp_path / "bad.json"
    with pytest.raises(ValueError, match="numerical checks"):
        collect(args)
    assert not args.output.exists()


def test_resource_prefix_shards_cover_each_job_once():
    from campaign import shard_jobs

    jobs = list(range(57))
    shards = [shard_jobs(jobs, slot, 3) for slot in range(3)]
    assert sorted(sum(shards, [])) == jobs
    assert all(
        not set(a) & set(b) for i, a in enumerate(shards) for b in shards[i + 1 :]
    )
    with pytest.raises(ValueError):
        shard_jobs(jobs, 3, 3)
    with pytest.raises(ValueError):
        shard_jobs(jobs, 0, 0)
