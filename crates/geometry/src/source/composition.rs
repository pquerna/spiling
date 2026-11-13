// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_model_import::{
    Affine, Mesh,
    threemf::{Leaf, Package, Role},
};
use std::collections::BTreeMap;

/// Compose in logical build scope, never across unrelated object occurrences.
/// Resolve the build translation into occurrence data before adopting material.
pub(super) fn negative_objects(
    package: &Package,
    leaves: Vec<Leaf>,
    global_findings: &[ImportFinding],
    selected_plate: u32,
    cancel: &AtomicBool,
) -> Result<Vec<DecodedSource>, RejectedSource> {
    let mut groups = BTreeMap::<usize, Vec<Leaf>>::new();
    for leaf in leaves {
        groups.entry(leaf.build_index).or_default().push(leaf);
    }
    let mut results: Vec<DecodedSource> = Vec::new();
    let mut shared = BTreeMap::new();
    for (index, group) in groups {
        crate::check_cancel(cancel)?;
        let first = group
            .iter()
            .find(|l| l.role == Role::Model)
            .ok_or_else(|| {
                invalid(
                    "3MF logical object has controls or negative volumes without model material",
                )
            })?;
        let anchor = first.transform_mm.0[3];
        let root = &package.models[&package.root].build[index];
        let key = (
            root.object.clone(),
            root.transform.0[..3]
                .iter()
                .map(|p| p.map(f64::to_bits))
                .collect::<Vec<_>>(),
        );
        if let Some(&existing) = shared.get(&key) {
            let decoded: &mut DecodedSource = &mut results[existing];
            decoded.placements.push(RigidPoseMm {
                translation_mm: anchor,
                rotation_xyzw: [0., 0., 0., 1.],
            });
            continue;
        }
        let mut positive = None;
        let mut negative = Vec::new();
        let mut findings = global_findings.to_vec();
        for leaf in &group {
            let mut transform = leaf.transform_mm;
            transform.0[3] = std::array::from_fn(|a| transform.0[3][a] - anchor[a]);
            if !matches!(leaf.role, Role::Model | Role::Negative) {
                findings.push(control_finding(leaf, transform)?);
                continue;
            }
            let (complex, local) =
                normalize::build_report((*leaf.mesh).clone(), 1., transform, cancel).map_err(
                    |failure| {
                        failure.object(format!(
                            "{}:{} · {}",
                            leaf.object.part, leaf.object.id, leaf.name
                        ))
                    },
                )?;
            findings.extend(local);
            let mesh = Mesh {
                vertices: complex.positions,
                triangles: complex.triangles,
            };
            if leaf.role == Role::Negative {
                findings.push(finding(
                    ImportFindingKind::Information,
                    ImportFindingCode::NegativeVolumes,
                    1,
                    "Negative volume",
                    "Subtracted from this logical object only.",
                    Some(topology::point_bounds(&mesh.vertices)?),
                    None,
                ));
                negative.push(mesh);
            } else {
                positive = Some(match positive {
                    None => mesh,
                    Some(old) => csg::compose(old, mesh, false, cancel)?,
                });
            }
        }
        let mut material =
            positive.ok_or_else(|| invalid("3MF object has no positive material"))?;
        for cut in negative {
            material = csg::compose(material, cut, true, cancel)?;
        }
        let (complex, local) = normalize::build_report(material, 1., Affine::IDENTITY, cancel)?;
        findings.extend(local);
        let result = DecodedSource {
            complex,
            selector: format!(
                "3mf:{}:{}:plate{}:composed1",
                root.object.part, root.object.id, selected_plate
            ),
            format: MeshSourceFormat::ThreeMf,
            source_unit: source_unit(first.source_scale_to_mm),
            name: first.name.clone(),
            findings,
            placements: vec![RigidPoseMm {
                translation_mm: anchor,
                rotation_xyzw: [0., 0., 0., 1.],
            }],
        };
        shared.insert(key, results.len());
        results.push(result);
    }
    Ok(results)
}
pub(super) fn control_finding(
    leaf: &Leaf,
    transform: Affine,
) -> Result<ImportFinding, GeometryError> {
    let points: Vec<_> = leaf
        .mesh
        .triangles
        .iter()
        .flatten()
        .map(|&v| transform.apply(leaf.mesh.vertices[v as usize]))
        .collect();
    let label = match leaf.role {
        Role::Modifier => "Modifier",
        Role::SupportBlocker => "Support blocker",
        Role::SupportEnforcer => "Support enforcer",
        _ => return Err(invalid("not a process control")),
    };
    Ok(finding(
        ImportFindingKind::NotApplied,
        ImportFindingCode::ProcessControl,
        1,
        label,
        "Excluded from model material; source manufacturing control is not applied.",
        Some(topology::point_bounds(&points)?),
        None,
    ))
}
pub(super) fn source_unit(scale: f64) -> SourceUnit {
    match scale {
        0.001 => SourceUnit::Micrometre,
        10. => SourceUnit::Centimetre,
        25.4 => SourceUnit::Inch,
        304.8 => SourceUnit::Foot,
        1000. => SourceUnit::Metre,
        _ => SourceUnit::Millimetre,
    }
}

pub(super) fn relative_transform(transform: Affine, pose: RigidPoseMm) -> Affine {
    let [x, y, z, w] = pose.rotation_xyzw;
    let inverse = RigidPoseMm {
        rotation_xyzw: [-x, -y, -z, w],
        translation_mm: [0.; 3],
    };
    let mut relative = transform;
    for a in 0..3 {
        relative.0[a] = inverse.rotate(transform.0[a]);
    }
    relative.0[3] = inverse.rotate(std::array::from_fn(|a| {
        transform.0[3][a] - pose.translation_mm[a]
    }));
    relative
}
