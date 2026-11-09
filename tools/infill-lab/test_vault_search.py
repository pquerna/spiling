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


def test_worker_admission_adapts_to_live_memory_and_cores():
    from vault_search_run import Workers, admitted_slots

    workers = Workers([], [], [], memory_gib=32, cpus=5)
    assert (
        admitted_slots([dict(memory_available=120 * 1024**3, cpus=20)], workers, 8) == 3
    )
    assert (
        admitted_slots([dict(memory_available=64 * 1024**3, cpus=16)], workers, 8) == 1
    )
    with pytest.raises(ValueError, match="headroom"):
        admitted_slots([dict(memory_available=30 * 1024**3, cpus=20)], workers, 8)


def test_roof_cohort_normalizes_only_the_declared_float_bound():
    from copy import deepcopy
    from vault_search import roof_cohort

    case = dict(
        judge={"immutable": "judge"},
        source_sha256="source",
        binaries={"engine": "binary"},
        scenario=dict(
            refinement=dict(max_xy=4, max_z=1, divisions=1),
            solver="superlu_mmd",
            bounds=[[40, 40, 44], [48, 48, 48]],
        ),
    )
    original = deepcopy(case)
    floating = deepcopy(case)
    floating["scenario"]["refinement"]["max_z"] = 1.0
    assert roof_cohort(case, "image") == roof_cohort(floating, "image")
    assert case == original
    assert digest(case) != digest(floating)  # Exact input identities are retained.
    floating["scenario"]["refinement"]["max_z"] = 0.5
    assert roof_cohort(case, "image") != roof_cohort(floating, "image")
    assert roof_cohort(case, "image") != roof_cohort(case, "other-image")
    floating = deepcopy(case)
    floating["scenario"]["solver"] = "p_amg"
    assert roof_cohort(case, "image") != roof_cohort(floating, "image")


def test_independent_pair_preserves_small_gains_and_zero_directions():
    from vault_pair import compare

    parent = dict(row("parent", 100, [10, 10, 0]), cohort="fixed")
    child = dict(row("child", 99.9, [10.001, 9.999, 1]), cohort="fixed")
    pair = dict(parent="parent", child="child")
    result = compare(dict(rows=[parent, child]), pair)
    assert result["material_ratio"] == pytest.approx(0.999)
    assert result["response_ratios"] == dict(x=1.0001, y=0.9999, z=None)
    child["cohort"] = "changed"
    with pytest.raises(ValueError, match="physical cohorts"):
        compare(dict(rows=[parent, child]), pair)
    child["status"] = "failed"
    assert compare(dict(rows=[parent, child]), pair)["status"] == "incomplete"


def test_pair_subset_rejects_unmatched_sources_and_producers(tmp_path, monkeypatch):
    import vault_pair

    roots = [tmp_path / "one", tmp_path / "two"]
    records = {
        root: (
            dict(identity="same"),
            [dict(id=key, status="ok", specimen="cube48", source_sha256="source")],
        )
        for root, key in zip(roots, ("parent", "child"), strict=True)
    }
    monkeypatch.setattr(vault_pair, "native_rows", lambda root: records[root])
    records[roots[1]][0]["identity"] = "different"
    with pytest.raises(ValueError, match="producers differ"):
        vault_pair.subset(roots, ["parent", "child"], tmp_path / "bad")
    records[roots[1]][0]["identity"] = "same"
    records[roots[1]][1][0]["source_sha256"] = "changed"
    with pytest.raises(ValueError, match="identical native source"):
        vault_pair.subset(roots, ["parent", "child"], tmp_path / "bad")
    assert not (tmp_path / "bad").exists()


def test_later_rounds_reuse_exact_original_source_bytes(tmp_path):
    from vault_search_run import freeze_specimens

    seed, later = tmp_path / "seed", tmp_path / "later"
    (seed / "specimens").mkdir(parents=True)
    original = seed / "specimens" / "cube.step"
    original.write_bytes(b"original exporter product identity\n")
    (seed / "specimens" / "cube.json").write_text('{"source":"frozen"}')
    freeze_specimens(seed, later)
    assert (later / "specimens" / "cube.step").read_bytes() == original.read_bytes()
    freeze_specimens(seed, later)  # Resume is idempotent, not a regeneration.
    (later / "specimens" / "cube.step").write_bytes(b"different product counter")
    with pytest.raises(ValueError, match="differs from frozen seed"):
        freeze_specimens(seed, later)
    assert (
        later / "specimens" / "cube.step"
    ).read_bytes() == b"different product counter"


def test_core_cohort_binds_solver_and_physics_but_groups_axes():
    from copy import deepcopy
    from vault_search import core_cohort

    case = dict(judge={"source": "frozen"}, solver="p_amg")
    target = dict(
        source_sha256="solid",
        scenarios=[
            dict(
                case="compression_x",
                material={"z_ratio": 0.25},
                refinement={"divisions": 1},
            )
        ],
    )
    base = core_cohort(case, target, "image")
    changed = deepcopy(target)
    changed["scenarios"][0]["case"] = "compression_z"
    assert core_cohort(case, changed, "image") == base
    assert core_cohort(dict(case, solver="superlu_mmd"), target, "image") != base
    assert core_cohort(case, target, "other-image") != base
    changed["scenarios"][0]["material"]["z_ratio"] = 1.0
    assert core_cohort(case, changed, "image") != base


def test_join_retains_all_failed_panel_and_rejects_wrong_solver(tmp_path):
    import json
    from types import SimpleNamespace as Args
    from model import file_hash
    from vault_search import join_core, write

    candidate = proposals(0, [])[7]["candidate"]
    judge = {"immutable": "judge"}
    scenario = {"case": "compression_x", "fixed": "fixture"}
    target = dict(
        candidate=candidate,
        roads_sha256="roads",
        source_sha256="source",
        scenarios=[scenario],
    )
    text = json.dumps(target) + "\n"
    reference = tmp_path / "reference.json"
    reference.write_text(text)
    case = dict(judge=judge, solver="p_amg", reference_case_sha256=file_hash(reference))
    key = digest(case)
    inputs, results = tmp_path / "inputs", tmp_path / "results"
    (inputs / key).mkdir(parents=True)
    (results / key).mkdir(parents=True)
    write(inputs / key / "case.json", case)
    (inputs / key / "reference-case.json").write_text(text)
    write(
        inputs / "jobs.json",
        dict(
            judge=judge,
            jobs=[dict(id=key, capture_id="native", candidate=candidate, axis="x")],
        ),
    )
    write(results / key / "launch.json", dict(removed=True, image={"id": "image"}))
    write(results / key / "dispatch.json", dict(exit_code=1))
    response = dict(
        input_sha256=file_hash(inputs / key / "case.json"),
        judge=judge,
        scenario=scenario,
        solver="p_amg",
        status="failed",
    )
    write(results / key / "trials.jsonl", response)
    roof = tmp_path / "roof.json"
    write(
        roof,
        dict(
            rows=[
                dict(
                    capture_id="native",
                    candidate=candidate,
                    roads_sha256="roads",
                    source_sha256="source",
                    status="failed",
                    stage="roof_mechanics",
                )
            ]
        ),
    )
    args = Args(
        roof=roof,
        input=inputs,
        results=results,
        history=None,
        output=tmp_path / "complete-but-unscored.json",
    )
    join_core(args)
    archive = json.loads(args.output.read_text())
    assert archive["frontier"] == []
    assert archive["rows"][0]["status"] == "failed"
    (results / key / "trials.jsonl").write_text(
        json.dumps(dict(response, solver="superlu_mmd"))
    )
    args.output = tmp_path / "wrong-solver.json"
    with pytest.raises(ValueError, match="response identity"):
        join_core(args)
    assert not args.output.exists()
