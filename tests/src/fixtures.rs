// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::path::{Path, PathBuf};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests directory must have parent")
        .join("fixtures")
        .join("geometry")
}

pub fn fixture_path(rel_path: &str) -> PathBuf {
    fixtures_dir().join(rel_path)
}

pub fn fixture_bytes(rel_path: &str) -> Vec<u8> {
    let p = fixture_path(rel_path);
    std::fs::read(&p).unwrap_or_else(|e| panic!("failed to read fixture at {:?}: {e}", p))
}

pub fn fixture_str(rel_path: &str) -> String {
    let p = fixture_path(rel_path);
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("failed to read fixture as string at {:?}: {e}", p))
}

pub const ALL_OCCT_FIXTURES: &[&str] = &[
    "independent-exporters/occt-box-mm.step",
    "independent-exporters/occt-cylinder-mm.step",
    "independent-exporters/occt-through-hole-mm.step",
    "independent-exporters/occt-narrow-pocket-mm.step",
    "independent-exporters/occt-tapered-fitting-mm.step",
    "independent-exporters/occt-ribbed-bracket-mm.step",
    "independent-exporters/occt-rounded-housing-mm.step",
    "independent-exporters/occt-spline-housing-mm.step",
];

pub const ALL_ADVERSARIAL_FIXTURES: &[(&str, &str)] = &[
    ("missing-units.step", "unsupported_units"),
    ("conflicting-units.step", "unsupported_units"),
    ("missing-reference.step", "invalid_geometry"),
    ("open-shell.step", "invalid_geometry"),
    ("non-manifold-shell.step", "invalid_geometry"),
    ("reachable-unsupported.step", "unsupported_geometry"),
    ("assembly.step", "unsupported_geometry"),
    ("nonfinite-coordinate.step", "invalid_geometry"),
    ("truncated.step", "invalid_geometry"),
    ("wrong-conversion-dimension.step", "unsupported_units"),
    ("recursive-units.step", "unsupported_units"),
    ("degrees.step", "unsupported_units"),
    ("duplicate-face-use.step", "invalid_geometry"),
];
