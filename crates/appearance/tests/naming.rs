// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use sha2::{Digest, Sha256};
use spiling_appearance::naming::*;
use spiling_contracts::color_naming::*;
#[test]
fn pinned_survey_exact_lookup_preserves_every_rgb_and_attribution() {
    let data = include_str!("../data/xkcd-rgb.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(data.as_bytes())),
        COLOR_NAMING_DATASET_SHA256
    );
    assert!(
        data.lines()
            .next()
            .unwrap()
            .contains("creativecommons.org/publicdomain/zero/1.0/")
    );
    assert_eq!(data.lines().skip(1).count(), 949);
    for row in data.lines().skip(1) {
        let rgb = parse_srgb8_hex(row.split('\t').nth(1).unwrap()).unwrap();
        let v = describe(rgb, ColorNamingMode::Exact);
        let n = v.name.unwrap();
        assert_eq!(v.srgb8, rgb);
        assert_eq!(n.reference_srgb8, rgb);
        assert_eq!(n.distance_oklab, 0.0);
        assert!(!n.generated);
    }
    assert!(
        describe([0x12, 0x34, 0x56], ColorNamingMode::Exact)
            .name
            .is_none()
    );
    let coral = describe([0xcf, 0x52, 0x4e], ColorNamingMode::Descriptive)
        .name
        .unwrap();
    assert_eq!(coral.label, "Dark coral");
    assert!(!coral.generated);
}
#[test]
fn perceptual_coordinates_match_published_primary_values() {
    for (rgb, want) in [
        ([255, 0, 0], [0.6279553606, 0.2248630611, 0.1258462985]),
        ([0, 0, 0], [0.0; 3]),
        ([255, 255, 255], [1.0, 0.0, 0.0]),
    ] {
        let got = srgb8_to_oklab(rgb);
        for i in 0..3 {
            assert!((got[i] - want[i]).abs() < 1e-7, "{rgb:?} {got:?}");
        }
    }
}
#[test]
fn descriptive_grid_is_bounded_and_qualifiers_describe_actual_differences() {
    let mut modifiers = [false; 5];
    for r in (0..=255).step_by(31) {
        for g in (0..=255).step_by(31) {
            for b in (0..=255).step_by(31) {
                let rgb = [r, g, b];
                let response = name_colors(&ColorNamingRequest {
                    mode: ColorNamingMode::Descriptive,
                    colors: vec![rgb],
                })
                .unwrap();
                response.validate().unwrap();
                let named = &response.colors[0];
                assert_eq!(named.srgb8, rgb);
                let n = named.name.as_ref().unwrap();
                let a = srgb8_to_oklab(rgb);
                let base = srgb8_to_oklab(n.reference_srgb8);
                let c = a[1].hypot(a[2]);
                let bc = base[1].hypot(base[2]);
                if let Some(m) = n.modifier {
                    match m {
                        ColorModifier::Dark => {
                            modifiers[0] = true;
                            assert!(a[0] < base[0] - 0.05)
                        }
                        ColorModifier::Light => {
                            modifiers[1] = true;
                            assert!(a[0] > base[0] + 0.05)
                        }
                        ColorModifier::Muted => {
                            modifiers[2] = true;
                            assert!(c < bc * 0.8)
                        }
                        ColorModifier::Pale => {
                            modifiers[3] = true;
                            assert!(a[0] > base[0] + 0.05 && c < bc * 0.8)
                        }
                        ColorModifier::Vivid => {
                            modifiers[4] = true;
                            assert!(c > bc * 1.15)
                        }
                    }
                }
            }
        }
    }
    assert!(
        modifiers.into_iter().all(|v| v),
        "all five modifiers need exercised positive examples"
    );
}
#[test]
fn neutral_names_do_not_invent_a_hue_or_chroma_modifier() {
    for n in 0..=255 {
        let name = describe([n; 3], ColorNamingMode::Descriptive).name.unwrap();
        assert!(["black", "white", "grey"].contains(&name.reference_name.as_str()));
        assert!(!matches!(
            name.modifier,
            Some(ColorModifier::Muted | ColorModifier::Pale | ColorModifier::Vivid)
        ));
    }
}
#[test]
fn batches_preserve_order_duplicates_modes_and_checked_wire_provenance() {
    let samples = vec![[12, 34, 56], [255, 255, 255], [12, 34, 56]];
    for mode in [
        ColorNamingMode::Exact,
        ColorNamingMode::Nearest,
        ColorNamingMode::Descriptive,
    ] {
        let response = name_colors(&ColorNamingRequest {
            colors: samples.clone(),
            mode,
        })
        .unwrap();
        response.validate().unwrap();
        assert_eq!(response.colors[0], response.colors[2]);
        let wire = spiling_contracts::rpc::NameColorsReply::try_from(&response).unwrap();
        assert_eq!(ColorNamingResponse::try_from(wire).unwrap(), response);
        assert_eq!(
            response.colors.iter().map(|v| v.srgb8).collect::<Vec<_>>(),
            samples
        );
    }
}
