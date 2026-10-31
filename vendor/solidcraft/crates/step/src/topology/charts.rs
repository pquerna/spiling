//! Native spline chart proposals, checked by the completed BREP and downstream geometry queries.

use super::*;

/// Same source carrier and unchanged spline parameter law, not a projection or
/// nearest-surface association. Ambiguous periodic uses retain the ordinary path.
pub(super) fn declared_spline_pcurve(
    source: &Curve,
    surface: &std::sync::Arc<crate::geom::Surface>,
    native_surface: &BSurface,
    native_curve: &BCurve,
    interval: [f64; 2],
    tolerance: f64,
) -> R<Option<Curve2>> {
    let Curve::SurfaceCurve { curve_3d, associated, .. } = source else {
        return Ok(None);
    };
    if !matches!(&**curve_3d, Curve::BSpline(_)) {
        return Ok(None);
    }
    let mut selected = associated.iter().filter_map(|a| match a {
        crate::geom::Associated::Pcurve(pc) if std::sync::Arc::ptr_eq(&pc.surface, surface) => Some(pc),
        _ => None,
    });
    let (Some(pc), None) = (selected.next(), selected.next()) else {
        return Ok(None);
    };
    let Curve::BSpline(spline) = &*pc.curve else { return Ok(None) };
    let mapped = match map_bspline_curve(spline)? {
        BCurve::Spline { knots, points } => Curve2::Spline {
            knots,
            points: points.into_iter().map(|p| [p[0], p[1]]).collect(),
        },
        BCurve::RationalSpline { knots, homogeneous } => Curve2::RationalSpline {
            knots,
            homogeneous: homogeneous.into_iter().map(|p| [p[0], p[1], p[3]]).collect(),
        },
        _ => return Ok(None),
    };
    if mapped.interval() != Some(interval) {
        return Ok(None);
    }
    // Source association and equal gauges propose correspondence; neither is
    // its proof. Decline incompatible proposals before native construction.
    // The completed record and independent native queries retain their checks.
    let carrier = SurfaceEval::new(native_surface).map_err(|e| e.to_string())?;
    let edge = CurveEval::new(native_curve).map_err(|e| e.to_string())?;
    for i in 0..=32 {
        let t = interval[0] + (interval[1] - interval[0]) * i as f64 / 32.;
        let Some((a, b)) = mapped.eval(t).and_then(|uv| carrier.point(uv)).zip(edge.point(t)) else {
            return Ok(None);
        };
        if a.dist(b) > tolerance * 0.25 {
            return Ok(None);
        }
    }
    Ok(Some(mapped))
}

/// Construct native chart curves; these remain derived approximations, never
/// rewritten STEP declarations. Shared endpoints use the original ordered
/// vertex incidence. Geometry validation below still checks every lifted curve.
pub(super) fn construct_native_chart_boundaries(
    surface: &BSurface,
    loops: &[Vec<(u32, bool)>],
    edges: &[EdgeB],
    vertices: &[Vertex],
    curves: &mut [Vec<Curve2>],
    tolerance: f64,
) -> R<()> {
    let eval = SurfaceEval::new(surface).map_err(|e| e.to_string())?;
    let domain = eval.domain();
    for ring in curves.iter_mut() {
        for curve in ring {
            if let Curve2::Spline { points, .. } = curve {
                for axis in 0..2 {
                    if let Some((a, b)) = domain.range[axis] {
                        let scale = a.abs().max(b.abs()).max(1.0);
                        for bound in [a, b] {
                            if points.iter().all(|p| (p[axis] - bound).abs() <= 64.0 * f64::EPSILON * scale) {
                                for p in points.iter_mut() {
                                    p[axis] = bound;
                                }
                            }
                        }
                    }
                }
                for p in points {
                    for axis in 0..2 {
                        if let Some((a, b)) = domain.range[axis] {
                            let proposed = p[axis].clamp(a.min(b), a.max(b));
                            // Search only for a rounding-scale native chart
                            // construction; genuine outlying controls can
                            // describe an in-chart curve and must stay intact.
                            let scale = a.abs().max(b.abs()).max(1.0);
                            if (proposed - p[axis]).abs() <= 64.0 * f64::EPSILON * scale {
                                p[axis] = proposed;
                            }
                        }
                    }
                }
            }
        }
    }
    for (uses, curves) in loops.iter().zip(curves) {
        for i in 0..uses.len() {
            let j = (i + 1) % uses.len();
            let (left, lf) = uses[i];
            let (right, rf) = uses[j];
            let le = edges.get(left as usize).ok_or("chart endpoint edge")?;
            let re = edges.get(right as usize).ok_or("chart endpoint edge")?;
            let lv = le.vertices[usize::from(lf)];
            let rv = re.vertices[usize::from(!rf)];
            if lv != rv {
                return Err("native chart endpoints lack shared original vertex".into());
            }
            let endpoint = |curve: &Curve2, end: bool| -> Option<[f64; 2]> {
                let Curve2::Spline { knots, points } = curve else { return None };
                let degree = knots.len().checked_sub(points.len() + 1)?;
                if degree == 0 || points.is_empty() {
                    return None;
                }
                let index = if end { points.len() - 1 } else { 0 };
                let value = if end { *knots.last()? } else { *knots.first()? };
                let ends = if end { &knots[points.len()..] } else { &knots[..=degree] };
                ends.iter().all(|k| *k == value).then_some(points[index])
            };
            let (Some(a), Some(b)) = (endpoint(&curves[i], lf), endpoint(&curves[j], !rf)) else {
                continue;
            };
            if a == b {
                continue;
            }
            let mut uv = [a[0] * 0.5 + b[0] * 0.5, a[1] * 0.5 + b[1] * 0.5];
            for axis in 0..2 {
                if let Some((lo, hi)) = domain.range[axis] {
                    let scale = lo.abs().max(hi.abs()).max(1.0);
                    for bound in [lo, hi] {
                        if (a[axis] - bound).abs() <= 64.0 * f64::EPSILON * scale && (b[axis] - bound).abs() <= 64.0 * f64::EPSILON * scale
                        {
                            uv[axis] = bound;
                        }
                    }
                }
            }
            // A shared spatial vertex does not identify its UV branch at a
            // pole or seam. Only propose a rounding-scale common endpoint;
            // preserve genuinely distinct chart coordinates and their paths.
            if (0..2).any(|axis| {
                let scale = domain.range[axis]
                    .map_or(a[axis].abs().max(b[axis].abs()), |(x, y)| x.abs().max(y.abs()))
                    .max(1.0);
                (a[axis] - b[axis]).abs() > 64.0 * f64::EPSILON * scale
            }) {
                continue;
            }
            let vertex = vertices.get(lv as usize).ok_or("chart endpoint vertex")?;
            let Some(point) = eval.point(uv) else { continue };
            // Across a genuine covering-chart seam the UV average need not
            // represent the vertex; retain that separately handled branch.
            if point.dist(v3(vertex.point)) > tolerance {
                continue;
            }
            for (index, end) in [(i, lf), (j, !rf)] {
                if let Curve2::Spline { points, .. } = &mut curves[index] {
                    let at = if end { points.len() - 1 } else { 0 };
                    points[at] = uv;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Associated, CurveForm, KnotSpec, Logical, Pcurve, PreferredRep, SurfaceCurveKind};
    use std::sync::Arc;

    fn spline(points: Vec<P3>) -> Curve {
        Curve::BSpline(Box::new(BSplineCurve {
            degree: 1,
            points,
            weights: None,
            knots: vec![0., 1.],
            multiplicities: vec![2, 2],
            form: CurveForm::Unspecified,
            closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }))
    }
    fn plane() -> BSurface {
        BSurface::Spline {
            u_knots: vec![0., 0., 1., 1.],
            v_knots: vec![0., 0., 1., 1.],
            points: vec![vec![[0., 0., 0.], [0., 1., 0.]], vec![[1., 0., 0.], [1., 1., 0.]]],
        }
    }
    #[test]
    fn authored_proposal_requires_unique_original_carrier_gauge_and_forward_image() {
        // Association identity is independent of equal geometry values.
        let owner = Arc::new(crate::geom::Surface::Plane {
            position: crate::geom::Placement {
                origin: [0.; 3],
                axis: [0., 0., 1.],
                ref_direction: [1., 0., 0.],
            },
        });
        let edge = spline(vec![[0., 0., 0.], [1., 0., 0.]]);
        let native = map_bspline_curve(match &edge {
            Curve::BSpline(b) => b,
            _ => unreachable!(),
        })
        .unwrap();
        let pc = Pcurve {
            surface: owner.clone(),
            curve: Arc::new(edge.clone()),
        };
        let mut source = Curve::SurfaceCurve {
            kind: SurfaceCurveKind::Surface,
            curve_3d: Arc::new(edge),
            associated: vec![Associated::Pcurve(pc.clone())],
            master: PreferredRep::Curve3d,
        };
        let propose =
            |s: &Curve, o: &Arc<crate::geom::Surface>, range| declared_spline_pcurve(s, o, &plane(), &native, range, 1e-7).unwrap();
        assert!(propose(&source, &owner, [0., 1.]).is_some());
        assert!(propose(&source, &Arc::new((*owner).clone()), [0., 1.]).is_none());
        assert!(propose(&source, &owner, [2., 3.]).is_none());
        if let Curve::SurfaceCurve { associated, .. } = &mut source {
            associated.push(Associated::Pcurve(pc));
        }
        assert!(propose(&source, &owner, [0., 1.]).is_none());
        if let Curve::SurfaceCurve { associated, .. } = &mut source {
            associated.truncate(1);
            if let Associated::Pcurve(p) = &mut associated[0] {
                p.curve = Arc::new(spline(vec![[0., 0., 0.], [1., 0.1, 0.]]));
            }
        }
        assert!(propose(&source, &owner, [0., 1.]).is_none());
    }
    #[test]
    fn chart_construction_preserves_real_excursions_and_distinct_uv_branches() {
        let epsilon = f64::EPSILON;
        let mut curves = vec![vec![
            Curve2::Spline {
                knots: vec![0., 0., 0., 1., 1., 1.],
                points: vec![[0., -epsilon], [0.5, epsilon], [1., -epsilon]],
            },
            Curve2::Spline {
                knots: vec![0., 0., 0., 1., 1., 1.],
                // An exterior control can describe a valid interior curve. It must
                // not be dragged onto the chart just because it is outside.
                points: vec![[0., 0.1], [0.5, -0.1], [1., 0.1]],
            },
        ]];
        let original = curves[0][1].clone();
        construct_native_chart_boundaries(&plane(), &[], &[], &[], &mut curves, 1e-7).unwrap();
        let Curve2::Spline { points, .. } = &curves[0][0] else { panic!() };
        assert!(points.iter().all(|p| p[1] == 0.));
        assert_eq!(curves[0][1], original);

        let edge = |vertices| EdgeB {
            source_geometry: None,
            curve: BCurve::Spline {
                knots: vec![0., 0., 1., 1.],
                points: vec![[0.; 3], [1., 0., 0.]],
            },
            interval: [0., 1.],
            vertices,
            tolerance: 1e-7,
        };
        let edges = [edge([0, 1]), edge([1, 0])];
        let vertices = [
            Vertex {
                point: [0.; 3],
                tolerance: 1e-7,
            },
            Vertex {
                point: [1., 0.5, 0.],
                tolerance: 1e-7,
            },
        ];
        let line = |a, b| Curve2::Spline {
            knots: vec![0., 0., 1., 1.],
            points: vec![a, b],
        };
        let mut ring = vec![vec![line([0., 0.], [1., 0.5]), line([1., 0.5 + epsilon], [0., 0.])]];
        construct_native_chart_boundaries(&plane(), &[vec![(0, true), (1, true)]], &edges, &vertices, &mut ring, 1e-7).unwrap();
        assert_eq!(ring[0][0].eval(1.), ring[0][1].eval(0.));
        ring[0][1] = line([0.25, 0.5], [0., 0.]);
        let distinct = ring[0][1].clone();
        construct_native_chart_boundaries(&plane(), &[vec![(0, true), (1, true)]], &edges, &vertices, &mut ring, 1e-7).unwrap();
        assert_eq!(
            ring[0][1], distinct,
            "shared spatial incidence does not collapse a distinct UV branch"
        );
        assert!(construct_native_chart_boundaries(&plane(), &[vec![(0, true), (0, true)]], &edges, &vertices, &mut ring, 1e-7).is_err());
    }
}
