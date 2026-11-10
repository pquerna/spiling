import copy
import math

import pytest

from roof_support_judge import capsule_intervals, gaps, judge, prepare


def road(start, end, lower=0.0, upper=0.2, kind=5, optional=False, owner=0):
    return dict(start=[*start, upper], end=[*end, upper], lower=lower, upper=upper,
                width=0.45, kind=kind, optional=optional, owner=owner, layer=round(upper / 0.2) - 1,
                feed_mm_s=20.0, extruded_filament_mm=math.dist(start, end) / 30)


def context(roads):
    proofs, _ = prepare(roads, 10.0)
    return dict(roads=roads, proofs=proofs, bridge_limit_mm=10.0,
                filament_diameter_mm=1.75, travel_speed_mm_s=100.0)


def test_analytic_capsules_cross_parallel_and_tangent():
    current = road([0, 0], [10, 0])
    previous = road([5, -1], [5, 1])
    interval = capsule_intervals(current, previous)
    assert min(low for low, _ in interval) == pytest.approx((5 - 0.224998) / 10)
    assert max(high for _, high in interval) == pytest.approx((5 + 0.224998) / 10)
    assert not capsule_intervals(current, road([0, 0.225], [10, 0.225]))
    assert capsule_intervals(current, road([0, 0.224], [10, 0.224]))


def test_whole_interval_union_not_sum_of_contacts():
    proof = dict(intervals=[[0, 0, 0.3], [1, 0.2, 0.5], [2, 0.8, 1]], endpoints=[])
    assert gaps(proof, [True] * 3) == [(0.5, 0.8, True, True)]
    assert gaps(proof, [False, False, True]) == [(0, 0.8, False, True)]


def test_required_columns_and_identity_cannot_be_deleted_or_duplicated():
    data = context([road([0, 0], [10, 0]), road([0, 1], [10, 1], optional=True)])
    assert judge(data, [0])["deposition_commands"] == 1
    for selected in ([1], [0, 0], [0, 2], [-1], [True]):
        with pytest.raises(ValueError):
            judge(data, selected)


@pytest.mark.parametrize("owner", ["mandatory_skin", 0, "wall"])
def test_removed_predecessor_fails_every_affected_kind(owner):
    data = context([road([0, 0], [10, 0], optional=True),
                    road([0, 0], [10, 0], lower=0.2, upper=0.4, owner=owner, kind=4)])
    judge(data, [0, 1])
    with pytest.raises(ValueError, match="support/process"):
        judge(data, [1])


def test_short_floating_stroke_has_no_tiny_motion_exemption():
    data = context([road([0, 0], [0.03, 0], optional=True),
                    road([0, 0], [0.03, 0], lower=0.2, upper=0.4)])
    with pytest.raises(ValueError):
        judge(data, [1])


def test_existing_bridge_role_does_not_authorize_one_ended_roof():
    data = context([road([0, -1], [0, 1]), road([8, -1], [8, 1], optional=True),
                    road([0, 0], [8, 0], lower=0.2, upper=0.4, owner="mandatory_skin", kind=8)])
    assert judge(data, [0, 1, 2])["max_roof_bridge_mm"] < 8
    with pytest.raises(ValueError):
        judge(data, [0, 2])


def test_ordinary_road_cannot_silently_acquire_bridge_process():
    data = context([road([0, 0], [8, 0], optional=True),
                    road([0, -1], [0, 1]), road([8, -1], [8, 1]),
                    road([0, 0], [8, 0], lower=0.2, upper=0.4, kind=4, owner="mandatory_skin")])
    with pytest.raises(ValueError):
        judge(data, [1, 2, 3])


def test_actual_physical_adjacency_not_neighboring_nonempty_layer():
    data = context([road([0, 0], [10, 0]), road([0, 0], [10, 0], lower=0.4, upper=0.6)])
    assert data["proofs"][1]["intervals"] == []
    assert data["proofs"][1]["baseline_bad"] == [[0.0, 1.0]]
    same = copy.deepcopy(data)
    same["roads"][0]["optional"] = True
    assert judge(same, [1])["inherited_warning_roads"] == 1


def test_mixed_physical_heights_and_lower_ceiling_are_both_checked():
    data = context([road([0, 0], [10, 0], upper=0.16, optional=True),
                    road([0, 0], [10, 0], lower=0.16, upper=0.31, owner="mandatory_skin", kind=4),
                    road([0, 0], [10, 0], lower=0.31, upper=0.47, owner="mandatory_skin", kind=4)])
    assert data["proofs"][1]["intervals"]
    with pytest.raises(ValueError):
        judge(data, [1, 2])


def test_inherited_warning_is_not_permission_to_enlarge_unsupported_interval():
    data = context([road([0, -1], [0, 1], optional=True), road([8, -1], [8, 1]),
                    road([0, 0], [8, 0], lower=0.2, upper=0.4, kind=4, owner="mandatory_skin")])
    assert judge(data, [0, 1, 2])["inherited_warning_roads"] == 1
    with pytest.raises(ValueError):
        judge(data, [1, 2])


def test_analytic_intervals_match_independent_point_segment_distance():
    import random
    from roof_support_judge import EPS, point_distance

    randomizer = random.Random(2917)
    for _ in range(60):
        current = road([randomizer.uniform(-2, 2), randomizer.uniform(-2, 2)],
                       [randomizer.uniform(-2, 2), randomizer.uniform(-2, 2)])
        previous = road([randomizer.uniform(-2, 2), randomizer.uniform(-2, 2)],
                        [randomizer.uniform(-2, 2), randomizer.uniform(-2, 2)])
        intervals = capsule_intervals(current, previous)
        for sample in range(101):
            parameter = sample / 100
            point = [current["start"][axis] + parameter * (current["end"][axis] - current["start"][axis]) for axis in (0, 1)]
            distance = point_distance(point, previous)
            inside = any(low <= parameter <= high for low, high in intervals)
            assert inside == (distance < previous["width"] / 2 - EPS)


def test_pareto_cannot_hide_worse_starts_with_less_material():
    from roof_support_report import dominates
    baseline = dict(volume_ratio=1.0, motion_ratio=1.0, starts_ratio=1.0)
    candidate = dict(volume_ratio=0.9, motion_ratio=0.9, starts_ratio=1.01)
    assert not dominates(candidate, baseline)
    assert not dominates(baseline, candidate)
    candidate["starts_ratio"] = 1.0
    assert dominates(candidate, baseline)


def test_frozen_judge_change_is_refused(tmp_path, monkeypatch):
    import json
    import roof_support
    (tmp_path / "freeze.json").write_text(json.dumps(dict(sources={"judge.py": "old"}, contexts={})))
    monkeypatch.setattr(roof_support, "source_inventory", lambda: {"judge.py": "changed"})
    with pytest.raises(ValueError, match="frozen judge"):
        roof_support.checked(tmp_path)


def test_freeze_refuses_mixed_native_producers(tmp_path, monkeypatch):
    import json
    import roof_support
    monkeypatch.setattr(roof_support, "CASES", [("first", "train"), ("second", "holdout")])
    for name in ("first", "second"):
        folder = tmp_path / "captures" / name
        folder.mkdir(parents=True)
        (folder / "context.json").write_text(json.dumps(dict(baseline_native_replayed=True, engine_build="same",
            bin_sha256={"spiling-engine": name, "spiling-cli": "same"})))
    with pytest.raises(ValueError, match="mixed native"):
        roof_support.freeze(tmp_path)
