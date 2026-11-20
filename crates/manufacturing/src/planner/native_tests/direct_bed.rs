use super::*;
use crate::planner::support::{AnchorCandidate, AnchorWorkspace, GapKind, StrokeRuns, gap_kind};

const EXCLUSIVE: (f64, f64) = (60.0, 90.0);
const WEST: (f64, f64) = (-5.0, 25.0);
const EAST: (f64, f64) = (25.0, 55.0);
const EXCLUSIVE_BIRTH: f64 = 2.0;
const BLOCKER_BIRTH: f64 = 12.0;
const BLOCKER_TOP: f64 = 20.0;
const LEGACY_BIRTH: f64 = 22.0;

fn inside(path: &DepositionPath, range: (f64, f64)) -> bool {
    path.points_mm
        .iter()
        .all(|point| range.0 < point[0] && point[0] < range.1)
}

fn support_paths(
    layer: &PrintLayer,
    range: (f64, f64),
) -> impl Iterator<Item = &DepositionPath> + Clone {
    layer
        .paths
        .iter()
        .filter(move |path| path.kind.is_support() && inside(path, range))
}

fn model_paths(
    layer: &PrintLayer,
    range: (f64, f64),
) -> impl Iterator<Item = &DepositionPath> + Clone {
    layer
        .paths
        .iter()
        .filter(move |path| path.kind.is_model() && inside(path, range))
}

fn roof_index(layers: &[PrintLayer], birth: f64, range: (f64, f64)) -> usize {
    layers
        .iter()
        .position(|layer| {
            layer.lower_z_mm.to_bits() == birth.to_bits()
                && model_paths(layer, range).next().is_some()
        })
        .expect("missing roof model roads at source birth")
}

fn needs_contact<'a>(
    path: &DepositionPath,
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contact: impl Iterator<Item = &'a DepositionPath> + Clone,
    limit: f64,
    cancel: &AtomicBool,
) -> bool {
    let mut anchors = AnchorWorkspace::default();
    let mut work = 0;
    for (first, last, width) in StrokeRuns::for_path(path).unwrap() {
        let endpoints = [path.points_mm[first], path.points_mm[last]];
        let length = (endpoints[1][0] - endpoints[0][0]).hypot(endpoints[1][1] - endpoints[0][1]);
        let candidate = AnchorCandidate {
            endpoints_mm: endpoints,
            bead_width_mm: width,
        };
        let has_demand = |anchors: &mut AnchorWorkspace, work: &mut usize, use_contacts| {
            anchors
                .gaps(
                    candidate,
                    previous.clone(),
                    contact.clone().filter(|_| use_contacts),
                    work,
                    cancel,
                )
                .unwrap()
                .iter()
                .any(|gap| {
                    matches!(
                        gap_kind(gap, (gap.end - gap.start) * length, width, limit),
                        GapKind::Demand
                    )
                })
        };
        if has_demand(&mut anchors, &mut work, false) && !has_demand(&mut anchors, &mut work, true)
        {
            return true;
        }
    }
    false
}

fn roof_contact(
    layers: &[PrintLayer],
    target: usize,
    range: (f64, f64),
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> usize {
    let top = (0..target)
        .rev()
        .find(|&index| support_paths(&layers[index], range).next().is_some())
        .expect("missing installed contact course");
    let SupportPolicy::Breakaway { z_gap_mm, .. } = &intent.recipe.supports else {
        panic!("fixture requires breakaway support");
    };
    let interval = |layer: &PrintLayer| {
        program_coordinates::project_xyz6_bead_height_mm(layer.z_mm, layer.z_mm - layer.lower_z_mm)
            .unwrap()
    };
    assert!(
        program_coordinates::enclosed_vertical_gap_mm(
            interval(&layers[target])[0],
            interval(&layers[top])[1],
        )
        .unwrap()
            >= *z_gap_mm,
        "terminal does not meet the physical Z clearance"
    );
    let contacts = support_paths(&layers[top], range)
        .filter(|path| path.kind == DepositionKind::SupportInterface);
    assert!(
        contacts.clone().next().is_some(),
        "terminal is not interface"
    );
    let previous = model_paths(&layers[target - 1], range);
    assert!(
        model_paths(&layers[target], range).any(|path| needs_contact(
            path,
            previous.clone(),
            contacts.clone(),
            intent.recipe.process.max_bridge_span_mm,
            cancel,
        )),
        "roof has no actual run that needs and receives this contact"
    );
    assert_eq!(layers[target].region_coverage.unsupported_overhang_mm, 0.0);
    top
}

fn assert_bed_lifetime(layers: &[PrintLayer], top: usize, range: (f64, f64)) {
    assert_eq!(layers[0].lower_z_mm, 0.0);
    for layer in &layers[..=top] {
        assert!(
            support_paths(layer, range).next().is_some(),
            "missing bed chain at {}",
            layer.index
        );
    }
}

fn part_launch(layers: &[PrintLayer], top: usize, cancel: &AtomicBool) -> (usize, usize) {
    let launch = layers[..=top]
        .iter()
        .position(|layer| {
            layer.lower_z_mm >= BLOCKER_TOP && support_paths(layer, WEST).next().is_some()
        })
        .expect("missing elevated west launch");
    let donor = layers[..launch]
        .iter()
        .rposition(|layer| layer.z_mm == BLOCKER_TOP && model_paths(layer, WEST).next().is_some())
        .expect("missing actual model donor below west launch");
    let previous = support_paths(&layers[launch - 1], WEST);
    assert!(
        previous.clone().next().is_none(),
        "west launch could inherit a bed stack"
    );
    assert!(
        support_paths(&layers[launch], WEST).any(|path| {
            let blocked_below = path.checked_edges().unwrap().any(|edge| {
                let [start, end] = [edge.points_mm[0], edge.points_mm[1]];
                let x = 0.5 * (start[0] + end[0]);
                let y = 0.5 * (start[1] + end[1]);
                0.0 < x && x < 20.0 && 0.0 < y && y < 20.0 && (x - 10.0).hypot(y - 10.0) > 3.0
            });
            blocked_below
                && needs_contact(
                    path,
                    previous.clone(),
                    model_paths(&layers[donor], WEST),
                    0.0,
                    cancel,
                )
        }),
        "west launch lacks actual donor contact over the bed-blocking source slab"
    );
    assert!(layers.iter().any(|layer| layer.lower_z_mm >= BLOCKER_BIRTH
        && layer.z_mm <= BLOCKER_TOP
        && layer.region_coverage.protected_hole_area_mm2 > 0.0));
    (launch, donor)
}

#[test]
fn support_sweep_installs_mixed_native_roots_and_replays() {
    let mut scene = NativeScene::new(HOLE, &[translated(0.0, 0.0, BLOCKER_BIRTH)]);
    scene.add(
        BOX,
        &[
            translated(65.0, 0.0, EXCLUSIVE_BIRTH),
            translated(0.0, 0.0, LEGACY_BIRTH),
            translated(30.0, 0.0, LEGACY_BIRTH),
        ],
    );
    let intent = intent(true);
    let cancel = AtomicBool::new(false);
    let result = compile(scene.input(&intent), &cancel).unwrap();
    drop(scene);
    // One shared sweep serves an exclusive bed demand, a later bed demand and
    // an on-part landing over a bed-blocking slab, each with its real contact.
    let returned = reload(&encode(&result), &result.program);
    let report = crate::verify_bundle(&returned, &cancel).unwrap();
    assert!(report.verified, "{report:#?}");
    let layers = &returned.plan.layers;
    let target = roof_index(layers, EXCLUSIVE_BIRTH, EXCLUSIVE);
    let later = roof_index(layers, LEGACY_BIRTH, WEST);
    assert_eq!(roof_index(layers, LEGACY_BIRTH, EAST), later);
    assert_ne!(target, later);
    let direct_top = roof_contact(layers, target, EXCLUSIVE, &intent, &cancel);
    let bed_top = roof_contact(layers, later, EAST, &intent, &cancel);
    let part_top = roof_contact(layers, later, WEST, &intent, &cancel);
    assert!(direct_top < bed_top && direct_top < part_top);
    assert_bed_lifetime(layers, direct_top, EXCLUSIVE);
    assert_bed_lifetime(layers, bed_top, EAST);
    part_launch(layers, part_top, &cancel);
    for layer in layers {
        for path in layer.paths.iter().filter(|path| path.kind.is_support()) {
            assert!(
                inside(path, EXCLUSIVE) || inside(path, WEST) || inside(path, EAST),
                "support path crosses or escapes an ownership partition"
            );
        }
    }
}
