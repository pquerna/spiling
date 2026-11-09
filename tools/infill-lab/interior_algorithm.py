# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Editable research construction; frozen judge is outside this file."""
from copy import deepcopy
import math


def row(template, fixed, c, lo, hi):
    if hi - lo < 0.001:
        return None
    r = deepcopy(template)
    r["start"][fixed] = r["end"][fixed] = round(c, 6)
    r["start"][1 - fixed] = round(lo, 6)
    r["end"][1 - fixed] = round(hi, 6)
    return r


def merge(rows):
    protected = [r for r in rows if r["upper"] >= 12.0]
    groups = {}
    for r in rows:
        if r["upper"] >= 12.0:
            continue
        fixed = int(r["start"][0] != r["end"][0])
        k = (r["layer"], fixed, r["start"][fixed])
        groups.setdefault(k, []).append(r)
    result = []
    for (_, fixed, c), group in sorted(groups.items()):
        intervals = sorted(
            (
                min(r["start"][1 - fixed], r["end"][1 - fixed]),
                max(r["start"][1 - fixed], r["end"][1 - fixed]),
            )
            for r in group
        )
        lo, hi = intervals[0]
        for a, b in intervals[1:]:
            if a <= hi:
                hi = max(hi, b)
            else:
                result.append(row(group[0], fixed, c, lo, hi))
                lo, hi = a, b
        result.append(row(group[0], fixed, c, lo, hi))
    return result + protected


def base_rows(context):
    for r in context["sparse"]:
        if r["upper"] >= context["edit_top"]:
            continue
        fixed = int(r["start"][0] != r["end"][0])
        if r["start"][fixed] not in context["roots"][fixed]:
            continue
        yield r, fixed, r["start"][fixed], min(
            r["start"][1 - fixed], r["end"][1 - fixed]
        ), max(r["start"][1 - fixed], r["end"][1 - fixed])


def ribs(context, depth, stations):
    output = []
    for r, fixed, c, lo, hi in base_rows(context):
        d = depth(r["upper"])
        if d < 0.001:
            continue
        for p in stations(context["roots"][1 - fixed], r["upper"]):
            for sign in (-1, 1):
                extra = row(
                    r,
                    fixed,
                    c + sign * context["width"],
                    max(lo, p - d),
                    min(hi, p + d),
                )
                if extra is not None:
                    output.append(extra)
    return output


def cut_windows(context, holes):
    output = []
    for r in context["sparse"]:
        if r["upper"] >= context["edit_top"]:
            output.append(r)
            continue
        fixed = int(r["start"][0] != r["end"][0])
        c = r["start"][fixed]
        spans = [
            (
                min(r["start"][1 - fixed], r["end"][1 - fixed]),
                max(r["start"][1 - fixed], r["end"][1 - fixed]),
            )
        ]
        for a, b in zip(context["roots"][1 - fixed], context["roots"][1 - fixed][1:]):
            if a < spans[0][0] or b > spans[-1][1]:
                continue
            for x, y in holes(a, b, r["upper"], fixed):
                next_spans = []
                for lo, hi in spans:
                    if y <= lo or x >= hi:
                        next_spans.append((lo, hi))
                        continue
                    if x > lo:
                        next_spans.append((lo, x))
                    if y < hi:
                        next_spans.append((y, hi))
                spans = next_spans
        for lo, hi in spans:
            p = row(r, fixed, c, lo, hi)
            if p is not None:
                output.append(p)
    return output


def generate(context):
    extra = ribs(context, lambda z: 1.35, lambda roots, z: roots)
    # Keep one side per web; alternate handedness by web index for balance.
    selected = []
    for r in extra:
        fixed = int(r["start"][0] != r["end"][0])
        c = r["start"][fixed]
        root = min(context["roots"][fixed], key=lambda v: abs(v - c))
        sign = 1 if context["roots"][fixed].index(root) % 2 == 0 else -1
        if abs(c - root - sign * context["width"]) < 1e-6:
            selected.append(r)
    return merge(context["sparse"] + selected)
