// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::analysis::source_incidence::{
    boundary_loop, face_orientation, native_coedge, native_face,
};

fn assert_original_vertex_anchor(
    source: crate::analysis::material_source::MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence<'_>,
    vertex: solidcore::VertexId,
    available: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) {
    let canonical = source
        .trim_source()
        .source_vertex_point(vertex, work, cancel)
        .unwrap();
    let mut source_vertex = None;
    let mut occurrences = 0;
    for edge in incidence.vertex_edges(vertex) {
        let pair = incidence.get(edge).unwrap();
        for use_ in std::iter::once(pair.first).chain(pair.second) {
            let domain = source
                .face_domain(use_.face, available, work, cancel)
                .unwrap();
            let proof = domain.original_coedge_correspondence(use_.coedge).unwrap();
            let binding = proof
                .binding()
                .expect("verified original endpoint ownership");
            assert_eq!(binding.endpoints, pair.vertices);
            let original = source
                .definition()
                .original_source()
                .coedge(binding.source.source_index)
                .unwrap();
            for (native_end, &endpoint) in pair.vertices.iter().enumerate() {
                if endpoint != vertex {
                    continue;
                }
                work.charge(1, cancel).unwrap();
                // The sealed query/native association supplies the original ID.
                // Gauge-B parameter images may reverse the literal basis, so
                // neither same_sense nor coordinate equality selects an endpoint.
                let id = binding.source_vertex_ids[native_end];
                if let Some(previous) = source_vertex {
                    assert_eq!(
                        id, previous,
                        "the complete selected star has one original VertexPoint identity"
                    );
                } else {
                    source_vertex = Some(id);
                }
                let point = binding.source_vertex_points_mm[native_end];
                let mut literal_matches = 0;
                for (&literal_id, &literal_point) in
                    original.vertex_ids.iter().zip(&original.vertex_points_mm)
                {
                    if literal_id != id {
                        continue;
                    }
                    assert_eq!(
                        point, literal_point,
                        "the sealed ID retains its literal original coordinates"
                    );
                    literal_matches += 1;
                }
                assert!(
                    literal_matches > 0,
                    "the sealed canonical ID belongs to this actual original EDGE_CURVE"
                );
                assert_eq!(
                    canonical, point,
                    "the material apex is the captured original VertexPoint, not a healed native vertex"
                );
                occurrences += 1;
            }
        }
    }
    assert!(
        source_vertex.is_some() && occurrences > 0,
        "complete actual source vertex star"
    );
}

#[test]
fn nonowner_collar_bubble_contracts_at_original_native_endpoints() {
    let whole = [10.0, 12.0];
    let first =
        spatial_endpoint_bubble(whole, Interval::new(10.0, 10.0 + 1.0 / 1024.0).unwrap()).unwrap();
    let last =
        spatial_endpoint_bubble(whole, Interval::new(12.0 - 1.0 / 1024.0, 12.0).unwrap()).unwrap();
    assert!(first.lo >= 0.0 && first.hi < 1.0 / 1024.0);
    assert!(last.lo >= 0.0 && last.hi < 1.0 / 1024.0);
    let center = spatial_endpoint_bubble(whole, Interval::point(11.0)).unwrap();
    assert!(center.lo <= 0.25 && center.hi == 0.25);
    let complete = spatial_endpoint_bubble(whole, Interval::new(10.0, 12.0).unwrap()).unwrap();
    assert_eq!((complete.lo, complete.hi), (0.0, 0.25));
    assert_eq!(
        spatial_endpoint_bubble(whole, Interval::point(9.0))
            .unwrap_err()
            .code,
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
    );
}

#[test]
fn actual_fitting_complete_paired_source_collars_have_local_geometry() {
    let cancel = AtomicBool::new(false);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step");
    let definition =
        crate::from_step_bytes_with_cancel(&std::fs::read(path).unwrap(), &cancel).unwrap();
    let mut work = GeometryWorkBudget::new(20_000_000);
    let prepared =
        crate::prepare_solid(&definition, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
    let source = prepared
        .source_view()
        .expect("one immutable original-domain owner");
    let available = MAX_SCRATCH_BYTES - prepared.retained_bytes();
    let mut incidence =
        SourceEdgeIncidence::new(&definition, available, &mut work, &cancel).unwrap();
    incidence
        .prepare_vertices(available, &mut work, &cancel)
        .unwrap();
    let available = available - incidence.owned_bytes();

    // Visit ALL paired incidences in source-occurrence order, not hash or
    // allocation-ID order. No observed failing parameter/face selects scope.
    let mut pairs = incidence.values().collect::<Vec<_>>();
    pairs.sort_unstable_by_key(|pair| (pair.first.face, pair.first.wire, pair.first.index));
    let mut vertices = Vec::new();
    for pair in &pairs {
        assert!(
            pair.second.is_some(),
            "complete original opposed-use inventory"
        );
        for vertex in pair.vertices {
            if !vertices.contains(&vertex) {
                vertices.push(vertex);
            }
        }
    }
    let mut exits = Vec::new();
    for vertex in vertices {
        assert_original_vertex_anchor(source, &incidence, vertex, available, &mut work, &cancel);
        let plan = vertex_transport::SourceVertexTransportPlan::prepare(
            source, &incidence, vertex, available, &mut work, &cancel,
        )
        .unwrap()
        .expect("the unchanged canonical source-only NativeV plan");
        plan.visit_spatial_exits(
            &mut work,
            &cancel,
            &mut |edge, source_high, parameter, _work| {
                if let Some(&(_, _, previous)) = exits
                    .iter()
                    .find(|&&(candidate, end, _)| candidate == edge && end == source_high)
                {
                    assert_eq!(
                        previous, parameter,
                        "same actual source-spatial endpoint germ"
                    );
                } else {
                    exits.push((edge, source_high, parameter));
                }
                Ok(())
            },
        )
        .unwrap();
    }

    let mut complete_pairs = 0;
    for pair in pairs {
        let first = pair.first;
        let second = pair.second.unwrap();
        let first_owner = [first.face, first.wire, first.index];
        let second_owner = [second.face, second.wire, second.index];
        let a = source
            .face_domain(first.face, available, &mut work, &cancel)
            .unwrap();
        let b = source
            .face_domain(second.face, available, &mut work, &cancel)
            .unwrap();
        let (edge, _) = native_coedge(&definition, first.face, first.wire, first.index).unwrap();
        let proof = a.original_coedge_correspondence(first.coedge).unwrap();
        let endpoint = |source_high| {
            exits
                .iter()
                .find(|&&(candidate, end, _)| candidate == edge.edge && end == source_high)
                .map(|&(_, _, parameter)| parameter)
                .expect("actual original NativeV exit")
        };
        let central = [endpoint(false), endpoint(true)];
        let whole = proof.spatial_range();
        assert!(
            whole[0] < central[0] && central[0] < central[1] && central[1] < whole[1],
            "fixed source cutouts: {first_owner:?} {second_owner:?} {whole:?} {central:?}",
        );
        let link = PairedMaterialLink::new(
            &definition,
            a,
            first,
            b,
            second,
            central,
            available,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap_or_else(|| {
            panic!("whole used paired UV embedding: {first_owner:?} {second_owner:?} {central:?}")
        });
        // The actual atlas asks for the COMPLETE central support before any
        // local FIRST-matrix subdivision. Its auxiliary rectangle can extend
        // beyond the selected trim envelope; this must remain a coefficient-
        // sheet positional query, not a whole-collar Jacobian qualification.
        for collar in [&link.first, &link.second] {
            let gamma = chart_jet(
                collar.domain,
                collar.use_.coedge,
                central,
                &mut work,
                &cancel,
            )
            .unwrap();
            let support = collar.source_relative_support(
                central, &gamma, link.allowance, &mut work, &cancel,
            ).unwrap_or_else(|error| panic!(
                "complete source-relative positional support: owner={:?} central={central:?}: {error:?}",
                [collar.use_.face, collar.use_.wire, collar.use_.index],
            )).expect("the literal fitting trace has retained whole-forward authority");
            for component in support {
                assert!(component.lo.is_finite() && component.hi.is_finite());
                assert!(component.lo <= component.hi);
            }
        }
        // Prepare original knots before borrowing the same physical edge.
        let cuts = original_coedge_parameter_cuts(
            [(a, first.coedge), (b, second.coedge)],
            available,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut covered = 0;
        let mut current_range = central;
        let mut current_owner = first_owner;
        let mut current_phase = "original spatial preparation";
        let mut last_refusal = None;
        a.with_original_spatial_curve(first.coedge, available, &mut work, &cancel, |curve, work, cancel| {
            for span in crate::parametric::curve_parameter_spans(curve, whole, work, cancel)? {
                let start = span[0].max(central[0]);
                let finish = span[1].min(central[1]);
                if start > finish {
                    continue;
                }
                let first_cut = cuts.partition_point(|cut| *cut <= start);
                let last_cut = cuts.partition_point(|cut| *cut < finish).max(first_cut);
                let mut start = start;
                for finish in cuts[first_cut..last_cut].iter().copied()
                    .chain(std::iter::once(finish))
                {
                    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = ([start, finish], 0);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (range, depth) = stack[pending];
                        current_range = range;
                        current_phase = "original spatial enclosure";
                        work.charge(1, cancel)?;
                        let jet = crate::parametric::curve_enclosure(curve, (range[0], range[1]), work, cancel)?;
                        let mut refused = None;
                        for (collar, embedding) in [
                            (&link.first, &link.first_embedding),
                            (&link.second, &link.second_embedding),
                        ] {
                            current_owner = [
                                collar.use_.face, collar.use_.wire, collar.use_.index,
                            ];
                            current_phase = "source collar local geometry";
                            if let Err(proof) = collar.geometry_cell_proof(
                                embedding, range, &jet, link.allowance, work, cancel,
                            )? {
                                refused = Some((collar.use_, proof));
                                break;
                            }
                        }
                        if let Some((owner, proof)) = refused {
                            let owner = [owner.face, owner.wire, owner.index];
                            let mid = range[0] * 0.5 + range[1] * 0.5;
                            assert!(
                                depth < MAX_DEPTH && mid > range[0] && mid < range[1],
                                "complete paired original-source local geometry: owner={owner:?} pair={first_owner:?}/{second_owner:?} central={central:?} native={range:?} proof={proof:?} work={}",
                                work.consumed(),
                            );
                            last_refusal = Some((owner, range, proof));
                            stack[pending] = ([mid, range[1]], depth + 1);
                            stack[pending + 1] = ([range[0], mid], depth + 1);
                            pending += 2;
                        } else {
                            covered += 1;
                        }
                    }
                    start = finish;
                }
            }
            Ok(())
        })
        .map_err(|mut error| {
            if error.code == GeometryErrorCode::ResourceLimit {
                error.message = format!(
                    "{}; direct paired source cover phase={current_phase} owner={current_owner:?} current_native={current_range:?} last_local_refusal={last_refusal:?}",
                    error.message,
                );
            }
            error
        })
        .unwrap_or_else(|error| panic!(
            "whole original paired collar: {first_owner:?}/{second_owner:?} central={central:?} work={}: {error:?}",
            work.consumed(),
        ));
        assert!(
            covered > 0,
            "every whole original paired central range is covered"
        );
        complete_pairs += 1;
    }
    assert!(complete_pairs > 0);
}

#[test]
fn actual_fitting_circular_rim_normal_embedding_covers_unchanged_used_ribbon() {
    let cancel = AtomicBool::new(false);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step");
    let definition =
        crate::from_step_bytes_with_cancel(&std::fs::read(path).unwrap(), &cancel).unwrap();
    let mut work = GeometryWorkBudget::new(20_000_000);
    let prepared =
        crate::prepare_solid(&definition, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
    let source = prepared
        .source_view()
        .expect("one owner for the unchanged original trim domains");
    let available = MAX_SCRATCH_BYTES - prepared.retained_bytes();
    let mut incidence =
        SourceEdgeIncidence::new(&definition, available, &mut work, &cancel).unwrap();
    incidence
        .prepare_vertices(available, &mut work, &cancel)
        .unwrap();
    let available = available - incidence.owned_bytes();

    // Both annular wires, including every occurrence produced by periodic
    // splitting. The flat slot counts ALL preceding wires, just as the
    // original incidence inventory does; wire-local index is not that slot.
    let face_index = 2;
    let (_, face) = native_face(&definition, face_index).unwrap();
    assert_eq!(face.loops.len(), 2);
    let material_forward = face_orientation(&definition, face_index, &mut work, &cancel).unwrap();
    let annulus = source
        .face_domain(face_index, available, &mut work, &cancel)
        .unwrap();
    let mut uses = Vec::new();
    let mut vertices = Vec::new();
    let mut flat = 0;
    for wire_index in 0..face.loops.len() {
        let wire = boundary_loop(&definition, face_index, wire_index).unwrap();
        assert!(
            !wire.edges.is_empty(),
            "each actual annular rim must be exercised"
        );
        for (index, edge) in wire.edges.iter().enumerate() {
            let expected = CoedgeUse {
                face: face_index,
                coedge: flat,
                wire: wire_index,
                index,
                orientation: edge.forward == material_forward,
                edgeuse: annulus.coedges()[flat].edgeuse,
            };
            flat += 1;
            let pair = incidence.get(edge.edge).expect("actual original rim edge");
            let second = pair.second.expect("opposed actual original rim use");
            assert!(pair.first == expected || second == expected);
            // Exercise the paired original occurrence as well: its chart,
            // gauge and orientation need not match the annular plane's.
            uses.extend([pair.first, second]);
            for vertex in pair.vertices {
                if !vertices.contains(&vertex) {
                    vertices.push(vertex);
                }
            }
        }
    }
    assert_eq!(uses.len(), 2 * flat);

    // Literal exits from the SAME complete source-only NativeV geometry
    // plans as production: no coordinate lookup, uniform fraction, UV
    // qualification or query-dependent shortening.
    let mut exits = Vec::new();
    for vertex in vertices {
        assert_original_vertex_anchor(source, &incidence, vertex, available, &mut work, &cancel);
        let plan = vertex_transport::SourceVertexTransportPlan::prepare(
            source, &incidence, vertex, available, &mut work, &cancel,
        )
        .unwrap()
        .expect("actual rim NativeV has its unchanged source-only geometry plan");
        plan.visit_spatial_exits(
            &mut work,
            &cancel,
            &mut |edge, source_high, parameter, _work| {
                if let Some(&(_, _, previous)) = exits
                    .iter()
                    .find(|&&(candidate, end, _)| candidate == edge && end == source_high)
                {
                    assert_eq!(
                        previous, parameter,
                        "one literal exit per original endpoint germ"
                    );
                } else {
                    exits.push((edge, source_high, parameter));
                }
                Ok(())
            },
        )
        .unwrap();
    }
    let mut nonzero_collars = 0;
    for use_ in uses {
        let domain = source
            .face_domain(use_.face, available, &mut work, &cancel)
            .unwrap();
        let (face_id, _) = native_face(&definition, use_.face).unwrap();
        let (edge, native_edge) =
            native_coedge(&definition, use_.face, use_.wire, use_.index).unwrap();
        // The receipt seals the verified selected branch. Independently check
        // its IDs against the retained original STEP genealogy; the raw census
        // alone must never supply the correspondence authority.
        let proof = domain.original_coedge_correspondence(use_.coedge).unwrap();
        let binding = proof.binding().expect("final actual original-use binding");
        let original = definition.original_source();
        let source_face = original.face(face_id).unwrap();
        let source_use = original.coedge(binding.source.source_index).unwrap();
        assert!(
            source_face
                .wires
                .iter()
                .any(|wire| wire.coedges.contains(&binding.source.source_index))
        );
        assert_eq!(source_use.source_face, source_face.source_face);
        assert_eq!(binding.face, face_id);
        assert_eq!(binding.edge, edge.edge);
        assert_eq!(binding.owner, [use_.face, use_.wire, use_.index]);
        assert_eq!(binding.edgeuse, use_.edgeuse);
        assert_eq!(binding.source.source_edge, Some(source_use.edge_curve_id.0));
        assert_eq!(
            binding.source.source_coedge,
            Some(source_use.oriented_edge_id.0)
        );
        assert_eq!(binding.source_face, Some(source_face.source_face.0));
        assert_eq!(binding.source.native_orientation, edge.forward);
        assert_eq!(binding.endpoints, native_edge.vertices);
        assert_eq!(
            binding.native_interval,
            [native_edge.interval.min(), native_edge.interval.max()]
        );
        let slope = proof.uv_parameter_derivative();
        let context = format!(
            "face={} coedge={} wire={} index={} source={source_use:?} spatial={:?} uv={:?} map_derivative={:?} margin={:?} physical_error_mm={}",
            use_.face,
            use_.coedge,
            use_.wire,
            use_.index,
            proof.spatial_range(),
            proof.uv_range(),
            [slope.lo, slope.hi],
            proof.margin_uv(),
            proof.position_error_mm(),
        );
        assert!(
            proof.position_error_mm() <= source_allowance(&definition).unwrap(),
            "{context}"
        );
        let uv_range = proof.uv_range();
        assert!(
            uv_range[0] < uv_range[1] && (slope.lo > 0.0 || slope.hi < 0.0),
            "retained original parameter map must be strictly oriented: {context}"
        );
        let range = proof.spatial_range();
        let width =
            (range[1] - range[0]) * domain.material_realization().collar_parameter_fraction();
        assert!(
            width.is_finite() && width > 0.0,
            "source-only collar width: {context}"
        );
        let collar = OriginalCollar::for_enclosure(&domain, use_, &mut work, &cancel)
            .unwrap_or_else(|error| panic!("original collar evaluation: {context}: {error:?}"))
            .unwrap_or_else(|| panic!("source-only inward classification refused after actual owner/map/width audit: {context}"));
        let has_collar = matches!(collar.transport, CarrierTransport::Collar { .. });
        nonzero_collars += usize::from(has_collar);
        let find_exit = |source_high| {
            exits
                .iter()
                .find(|&&(candidate, end, _)| candidate == edge.edge && end == source_high)
                .map(|&(_, _, parameter)| parameter)
                .unwrap_or_else(|| {
                    panic!("missing original spatial endpoint germ source_high={source_high}: {context}")
                })
        };
        let central = [find_exit(false), find_exit(true)];
        assert!(
            range[0] < central[0] && central[0] < central[1] && central[1] < range[1],
            "actual NativeV exits partition the whole original rim: {context} central={central:?}"
        );
        let midpoint = range[0] * 0.5 + range[1] * 0.5;
        let (curve, _) = domain
            .original_coedge_jets(use_.coedge, [midpoint; 2], &mut work, &cancel)
            .unwrap();
        let before = collar
            .boundary_jet(midpoint, [0.0, 1.0], &curve, &mut work, &cancel)
            .unwrap();
        let embedding = collar
            .embedding(central, [0.0, 1.0], &mut work, &cancel)
            .unwrap_or_else(|error| panic!("scoped original UV proof: {context} central={central:?}: {error:?}"))
            .unwrap_or_else(|| panic!("whole USED original normal ribbon is not proved injective: {context} central={central:?}"));
        assert!(embedding.covers(&collar, central, [0.0, 1.0]), "{context}");
        assert!(
            !embedding.covers(&collar, range, [0.0, 1.0]),
            "a central receipt must never claim the collapsed NativeV quotient: {context}"
        );
        assert!(
            !embedding.covers(&collar, central, [0.0, 1.0001]),
            "{context}"
        );
        let after = collar
            .boundary_jet(midpoint, [0.0, 1.0], &curve, &mut work, &cancel)
            .unwrap();
        assert_eq!(
            before.position.map(|v| [v.lo, v.hi]),
            after.position.map(|v| [v.lo, v.hi]),
            "{context}"
        );
        assert_eq!(
            before.derivative.map(|v| [v.lo, v.hi]),
            after.derivative.map(|v| [v.lo, v.hi]),
            "{context}"
        );
        if has_collar {
            assert!(
                collar
                    .embedding(range, [0.0, 1.0], &mut work, &cancel)
                    .unwrap()
                    .is_none(),
                "collapsed original endpoints are not a strict UV chart: {context}"
            );
        }

        // Right geometry, wrong original occurrence: neither constructor
        // ownership nor a borrowed receipt may accept it.
        let pair = incidence.get(edge.edge).unwrap();
        let other = if pair.first == use_ {
            pair.second.unwrap()
        } else {
            pair.first
        };
        let wrong_use = CoedgeUse {
            coedge: use_.coedge,
            ..other
        };
        assert!(
            OriginalCollar::for_enclosure(&domain, wrong_use, &mut work, &cancel)
                .unwrap()
                .is_none(),
            "actual geometry cannot grant foreign owner authority: {context}"
        );
        let owned = collar
            .owned_embedding(central, available, &mut work, &cancel)
            .unwrap()
            .expect("whole boundary-owned original ribbon");
        let wrong_owner = OriginalCollar {
            use_: wrong_use,
            ..collar
        };
        assert!(
            !embedding.covers(&wrong_owner, central, [0.0, 1.0]),
            "{context}"
        );
        assert!(
            wrong_owner
                .geometry_cell(
                    &owned,
                    central,
                    &curve,
                    source_allowance(&definition).unwrap(),
                    &mut work,
                    &cancel,
                )
                .is_err(),
            "geometry consumer must reject another owner's receipt: {context}"
        );
        let narrow = collar
            .owned_embedding([midpoint; 2], available, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(!narrow.covers(&collar, central, [0.0, 1.0]), "{context}");
        assert!(
            collar
                .geometry_cell(
                    &narrow,
                    central,
                    &curve,
                    source_allowance(&definition).unwrap(),
                    &mut work,
                    &cancel,
                )
                .is_err(),
            "every cell must consume its full native scope: {context}"
        );
        let narrow_across = collar
            .embedding([midpoint; 2], [0.0, 0.5], &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(
            !narrow_across.covers(&collar, [midpoint; 2], [0.0, 1.0]),
            "a partial PURE across proof cannot grant whole owned-G authority: {context}"
        );
    }
    assert!(
        nonzero_collars > 0,
        "actual rims must exercise nonzero original transport"
    );
}

#[test]
fn normal_coordinate_pmatrix_refuses_a_genuine_curvature_fold() {
    // At t=0 of a unit circle, U=(1-k*r)*gamma. k=2 and
    // r in [0,1] includes an actual collapsed circle and orientation fold.
    let jet = ChartCurveEnclosure {
        position: [1.0, 0.0].map(Interval::point),
        derivative: [0.0, 1.0].map(Interval::point),
        second_derivative: [-1.0, 0.0].map(Interval::point),
    };
    assert!(normal_uv_pmatrix(
        &jet,
        jet.derivative,
        0.25,
        1.0,
        Interval { lo: 0.0, hi: 1.0 }
    ));
    assert!(!normal_uv_pmatrix(
        &jet,
        jet.derivative,
        2.0,
        1.0,
        Interval { lo: 0.0, hi: 1.0 }
    ));
}

fn pocket_floor() -> (Definition, usize) {
    let definition = crate::analysis::test_source::recipe("native-parity-blind-pocket.step");
    let mut work = GeometryWorkBudget::new(20_000_000);
    let functional = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    let floor = (0..definition.face_ids().len())
        .find(|&face| {
            super::super::super::slab::constant_source_face_height(
                &definition,
                face,
                functional,
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap()
            .is_some_and(|height| height.lo == 4.0 && height.hi == 4.0)
        })
        .expect("coefficient-backed interior floor");
    (definition, floor)
}

#[test]
fn actual_generic_transport_floor_preserves_complete_exact_scalar_law() {
    let (definition, floor) = pocket_floor();
    let functional = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    let mut work = GeometryWorkBudget::new(20_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        MAX_SCRATCH_BYTES,
        &mut work,
        &AtomicBool::new(false),
    )
    .unwrap();
    let source = prepared.source_view().unwrap();
    assert!(
        !definition
            .material_transport_is_proved_zero(&mut work, &AtomicBool::new(false),)
            .unwrap()
    );
    assert_eq!(
        constant_material_face_height(
            source,
            floor,
            functional,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap(),
        Some(4.0)
    );
}

#[test]
fn complete_material_scalar_law_rejects_sub_roundoff_functional_tilt() {
    let (definition, floor) = pocket_floor();
    let functional = HeightFunctional {
        direction: [1.0e-18, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    let mut work = GeometryWorkBudget::new(20_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        MAX_SCRATCH_BYTES,
        &mut work,
        &AtomicBool::new(false),
    )
    .unwrap();
    let source = prepared.source_view().unwrap();
    assert_eq!(
        constant_material_face_height(
            source,
            floor,
            functional,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap(),
        None,
        "rounded point heights cannot establish a scalar identity"
    );
}
