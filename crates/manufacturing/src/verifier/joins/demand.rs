// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{cancelled, failed};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn verify(
    frame: &FunctionalMaterialFrame,
    request: &FunctionalMaterialConstruction,
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    let transition_end = request.minimum_pure_depth_mm
        + request.join.reach_into_region_mm
        + request.join.reach_into_base_mm;
    let end = transition_end + request.minimum_solid_base_root_depth_mm;
    let axis = usize::from(frame.outward_normal[1] != 0.0);
    let sign = -frame.outward_normal[axis];
    if program_coordinates::enclosed_vertical_gap_mm(
        sign * (frame.world_origin_mm[axis] + sign * end),
        sign * (frame.world_origin_mm[axis] + sign * transition_end),
    )? < request.minimum_solid_base_root_depth_mm
    {
        return Err(failed(
            "functional solid demand cannot establish the requested minimum root depth after coordinate rounding",
        ));
    }
    let horizon = recipe
        .bottom_skin_thickness_mm
        .max(recipe.top_skin_thickness_mm)
        .max(recipe.layer_height_mm);
    let expected = [
        FunctionalBaseSolidZone {
            kind: FunctionalBaseSolidZoneKind::Root,
            inward_depth_mm: [request.minimum_pure_depth_mm, end],
            physical_z_mm: frame.region_z_mm,
        },
        FunctionalBaseSolidZone {
            kind: FunctionalBaseSolidZoneKind::LowerLanding,
            inward_depth_mm: [0.0, end],
            physical_z_mm: [
                (frame.region_z_mm[0] - horizon).max(frame.stock_bounds_mm.min[2]),
                frame.region_z_mm[0],
            ],
        },
        FunctionalBaseSolidZone {
            kind: FunctionalBaseSolidZoneKind::UpperCover,
            inward_depth_mm: [0.0, end],
            physical_z_mm: [
                frame.region_z_mm[1],
                (frame.region_z_mm[1] + horizon).min(frame.stock_bounds_mm.max[2]),
            ],
        },
    ];
    if !frame.solid_base_zones.iter().eq(expected
        .iter()
        .filter(|zone| zone.physical_z_mm[0] < zone.physical_z_mm[1]))
    {
        return Err(failed(
            "functional solid demand omits or changes its source/intent/recipe-derived root, lower landing or upper cover",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::{AabbMm, DefinitionId, FaceId, OccurrenceId, SourceHash};

    fn fixture() -> (
        FunctionalMaterialFrame,
        FunctionalMaterialConstruction,
        PlanarPrintRecipe,
    ) {
        let (intent, _) = super::super::super::tests::fixture();
        let definition_id = DefinitionId::from_source_sha256(&[0; 32]);
        let face_id = FaceId::parse("step:1").unwrap();
        let occurrence_id = OccurrenceId::new(1).unwrap();
        let source_hash = SourceHash::from_bytes(b"source rectangle");
        let request = FunctionalMaterialConstruction {
            interface_policy: FunctionalInterfacePolicy::MechanicalCapture,
            occurrence_id,
            definition_id: definition_id.clone(),
            source_hash: source_hash.clone(),
            face_id: face_id.clone(),
            face_patch: None,
            material_id: "region".into(),
            base_material_id: "base".into(),
            minimum_pure_depth_mm: 5.0,
            minimum_solid_base_root_depth_mm: 0.8,
            join: PlanarTKeyJoin {
                reach_into_region_mm: 1.6,
                reach_into_base_mm: 1.6,
                pitch_mm: 6.4,
                stem_width_mm: 1.6,
                head_width_mm: 4.8,
                head_depth_mm: 1.2,
                edge_margin_mm: 1.2,
                end_collar_height_mm: 1.2,
                collar_head_width_mm: 3.2,
            },
        };
        let end = request.minimum_pure_depth_mm
            + request.join.reach_into_region_mm
            + request.join.reach_into_base_mm
            + request.minimum_solid_base_root_depth_mm;
        let frame = FunctionalMaterialFrame {
            occurrence_id,
            definition_id,
            source_hash,
            face_id,
            origin_mm: [0.0; 3],
            world_origin_mm: [0.0; 3],
            stock_bounds_mm: AabbMm::new([0.0; 3], [20.0, 10.0, 8.0]).unwrap(),
            horizontal_direction: [0.0, 1.0, 0.0],
            outward_normal: [-1.0, 0.0, 0.0],
            width_mm: 10.0,
            source_z_mm: [0.0, 8.0],
            selected_u_mm: [0.0, 10.0],
            region_z_mm: [0.0, 8.0],
            key_middle_z_mm: [1.2, 6.8],
            key_centers_u_mm: vec![5.0],
            solid_base_zones: vec![FunctionalBaseSolidZone {
                kind: FunctionalBaseSolidZoneKind::Root,
                inward_depth_mm: [request.minimum_pure_depth_mm, end],
                physical_z_mm: [0.0, 8.0],
            }],
        };
        (frame, request, intent.recipe)
    }

    #[test]
    fn functional_sparse_parent_validates_zone_demand_independent_of_density() {
        let (frame, request, mut recipe) = fixture();
        recipe.infill_fraction = 0.2;
        verify(&frame, &request, &recipe, &AtomicBool::new(false)).unwrap();
    }

    #[test]
    fn functional_sparse_parent_root_cannot_lose_requested_depth_to_rounding() {
        let (mut frame, mut request, recipe) = fixture();
        request.minimum_solid_base_root_depth_mm += 2.0_f64.powi(-50);
        frame.solid_base_zones[0].inward_depth_mm[1] = request.minimum_pure_depth_mm
            + request.join.reach_into_region_mm
            + request.join.reach_into_base_mm
            + request.minimum_solid_base_root_depth_mm;
        let failure = verify(&frame, &request, &recipe, &AtomicBool::new(false)).unwrap_err();
        assert!(failure.message.contains("minimum root depth"));
    }

    #[test]
    fn functional_sparse_parent_zones_cannot_omit_shrink_or_move_the_demand() {
        let (frame, request, recipe) = fixture();
        verify(&frame, &request, &recipe, &AtomicBool::new(false)).unwrap();
        for mutation in 0..4 {
            let mut bad = frame.clone();
            match mutation {
                0 => bad.solid_base_zones.clear(),
                1 => bad.solid_base_zones[0].inward_depth_mm[1] -= 0.4,
                2 => {
                    for value in &mut bad.solid_base_zones[0].inward_depth_mm {
                        *value += 1.0;
                    }
                }
                _ => bad.solid_base_zones[0].physical_z_mm[1] -= 0.4,
            }
            assert!(verify(&bad, &request, &recipe, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn functional_sparse_parent_localized_zones_need_exact_physical_landings_and_covers() {
        let (mut frame, request, recipe) = fixture();
        frame.region_z_mm = [1.13, 6.87];
        frame.solid_base_zones[0].physical_z_mm = frame.region_z_mm;
        let end = frame.solid_base_zones[0].inward_depth_mm[1];
        frame.solid_base_zones.extend([
            FunctionalBaseSolidZone {
                kind: FunctionalBaseSolidZoneKind::LowerLanding,
                inward_depth_mm: [0.0, end],
                physical_z_mm: [frame.region_z_mm[0] - recipe.layer_height_mm, 1.13],
            },
            FunctionalBaseSolidZone {
                kind: FunctionalBaseSolidZoneKind::UpperCover,
                inward_depth_mm: [0.0, end],
                physical_z_mm: [6.87, 7.87],
            },
        ]);
        verify(&frame, &request, &recipe, &AtomicBool::new(false)).unwrap();
        frame.solid_base_zones[1].physical_z_mm[0] =
            frame.solid_base_zones[1].physical_z_mm[0].next_up();
        assert!(verify(&frame, &request, &recipe, &AtomicBool::new(false)).is_err());
    }
}
