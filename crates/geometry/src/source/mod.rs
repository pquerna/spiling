// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! One adopted material interpretation, with native or mixed surface storage.
mod deviation;
mod normalize;
mod packed;
use crate::{
    Definition, GeometryError, GeometryErrorCode,
    tolerant::{TolerantModel, TolerantPolicy},
};
use spiling_contracts::geometry::*;
use std::sync::atomic::AtomicBool;

/// Consumers borrow one source. Native-only operations explicitly request the
/// native capability; they never receive a fabricated triangle BREP.
#[expect(
    clippy::large_enum_variant,
    reason = "One immutable definition is already Arc-owned; avoid another allocation around the native owner"
)]
pub enum SourceDefinition {
    Native(Definition),
    Hybrid(HybridDefinition),
}
pub struct HybridDefinition {
    pub id: DefinitionId,
    pub provenance: SourceProvenance,
    pub bounds: AabbMm,
    pub complex: SurfaceComplex,
}
#[derive(Debug)]
pub struct SurfaceComplex {
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub regions: Vec<u32>,
    pub faces: Vec<FaceInfo>,
    /// Maximum adopted analytic displacement from original facets.
    pub error_mm: f64,
}
impl From<Definition> for SourceDefinition {
    fn from(d: Definition) -> Self {
        Self::Native(d)
    }
}
impl SourceDefinition {
    pub fn native(&self) -> Result<&Definition, GeometryError> {
        match self {
            Self::Native(d) => Ok(d),
            _ => Err(invalid("operation requires a native BREP capability")),
        }
    }
    pub fn id(&self) -> &DefinitionId {
        match self {
            Self::Native(d) => d.id(),
            Self::Hybrid(d) => &d.id,
        }
    }
    pub fn provenance(&self) -> &SourceProvenance {
        match self {
            Self::Native(d) => d.provenance(),
            Self::Hybrid(d) => &d.provenance,
        }
    }
    pub fn faces(&self) -> &[FaceInfo] {
        match self {
            Self::Native(d) => d.faces(),
            Self::Hybrid(d) => &d.complex.faces,
        }
    }
    pub fn bounds_mm(&self) -> AabbMm {
        match self {
            Self::Native(d) => d.bounds_mm(),
            Self::Hybrid(d) => d.bounds,
        }
    }
    pub fn retained_query_proof_bytes(&self) -> usize {
        match self {
            Self::Native(d) => d.retained_query_proof_bytes(),
            Self::Hybrid(d) => d.complex.retained_bytes(),
        }
    }
    pub fn prepare(
        &self,
        policy: TolerantPolicy,
        cancel: &AtomicBool,
    ) -> Result<TolerantModel, GeometryError> {
        match self {
            Self::Native(d) => TolerantModel::build(d, policy, cancel),
            Self::Hybrid(d) => {
                crate::check_cancel(cancel)?;
                TolerantModel::from_complex(&d.complex, policy)
            }
        }
    }
    pub fn circles(&self, pose: &RigidPoseMm) -> crate::tolerant::CircleSource {
        crate::tolerant::CircleSource::from_faces(self.faces(), pose)
    }
    pub fn restore(
        provenance: SourceProvenance,
        bytes: &[u8],
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        crate::check_cancel(cancel)?;
        provenance.validate()?;
        if provenance
            .normalized
            .as_ref()
            .is_none_or(|record| record.hash != SourceHash::from_bytes(bytes))
        {
            return Err(invalid(
                "retained normalized geometry does not match its source descriptor",
            ));
        }
        let complex = SurfaceComplex::decode(bytes)?;
        let bounds = complex.bounds()?;
        Ok(Self::Hybrid(HybridDefinition {
            id: provenance.definition_id(),
            provenance,
            bounds,
            complex,
        }))
    }
}
impl SurfaceComplex {
    pub fn normalize(
        mesh: spiling_model_import::Mesh,
        scale: f64,
        transform: spiling_model_import::Affine,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        normalize::build(mesh, scale, transform, cancel).map(|(complex, _)| complex)
    }
    pub fn retained_bytes(&self) -> usize {
        self.positions.capacity() * 24
            + self.triangles.capacity() * 12
            + self.regions.capacity() * 4
            + self.faces.capacity() * std::mem::size_of::<FaceInfo>()
    }
    fn measured_deviation(&self) -> Result<f64, GeometryError> {
        self.triangles
            .iter()
            .zip(&self.regions)
            .try_fold(0_f64, |max, (triangle, region)| {
                match &self.faces[*region as usize].geometry.analytic {
                    Some(surface) => {
                        deviation::triangle(surface, triangle.map(|v| self.positions[v as usize]))
                            .map(|error| max.max(error))
                    }
                    None => Ok(max),
                }
            })
    }
    pub fn bounds(&self) -> Result<AabbMm, GeometryError> {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for p in &self.positions {
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        // Adopted analytic patches may extend beyond their facet chords.
        // Keep public source bounds conservative under that retained deviation.
        AabbMm::new(
            min.map(|v| (v - self.error_mm).next_down()),
            max.map(|v| (v + self.error_mm).next_up()),
        )
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        if self.positions.is_empty()
            || self.triangles.is_empty()
            || self.faces.is_empty()
            || self.regions.len() != self.triangles.len()
            || !self.error_mm.is_finite()
            || self.error_mm < 0.
            || self.error_mm > TolerantPolicy::default().profile_mm
            || self.positions.iter().flatten().any(|v| !v.is_finite())
            || self
                .triangles
                .iter()
                .flatten()
                .any(|&v| v as usize >= self.positions.len())
            || self.regions.iter().any(|&r| r as usize >= self.faces.len())
        {
            return Err(invalid("invalid retained surface complex"));
        }
        for (index, face) in self.faces.iter().enumerate() {
            face.validate()?;
            if face.face_id
                != FaceId::from_mesh_region(
                    u32::try_from(index + 1).map_err(|_| invalid("region identity overflow"))?,
                )?
            {
                return Err(invalid(
                    "retained region identity disagrees with its inventory",
                ));
            }
        }
        if self.measured_deviation()? > self.error_mm {
            return Err(invalid(
                "retained analytic interpretation exceeds its declared deviation",
            ));
        }
        self.bounds()?;
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message.into())
}

pub struct DecodedSource {
    pub complex: SurfaceComplex,
    pub selector: String,
    pub format: MeshSourceFormat,
    pub source_unit: SourceUnit,
    pub name: String,
    pub notes: Vec<String>,
    pub placements: Vec<RigidPoseMm>,
}
/// Decode syntax and adopt geometry once; rigid translation stays occurrence
/// data. Repeated package instances with the same linear map share a source.
pub fn decode_container(
    bytes: &[u8],
    label: &str,
    format: MeshSourceFormat,
    live_bytes: usize,
    options: &ModelImportOptions,
    cancel: &AtomicBool,
) -> Result<Vec<DecodedSource>, GeometryError> {
    use spiling_model_import::{
        Affine,
        threemf::{Package, Role},
    };
    let decode_error = |e: spiling_model_import::DecodeError| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            GeometryError::new(GeometryErrorCode::Cancelled, e.to_string())
        } else {
            invalid(e.to_string())
        }
    };
    if format == MeshSourceFormat::Stl {
        if options.source_plate.is_some() {
            return Err(invalid("STL has no source-plate selection"));
        }
        let mesh = spiling_model_import::stl::decode(bytes, cancel).map_err(decode_error)?;
        let (complex, duplicates) = normalize::build(
            mesh,
            options
                .stl_unit
                .unwrap_or(SourceUnit::Millimetre)
                .scale_to_mm(),
            Affine::IDENTITY,
            cancel,
        )?;
        let mut notes =
            vec!["STL is unitless; interpreted dimensions and units are retained.".into()];
        if duplicates > 0 {
            notes.push(format!(
                "Removed {duplicates} duplicate facets with identical winding."
            ));
        }
        return Ok(vec![DecodedSource {
            complex,
            selector: format!(
                "stl:{:?}:surface1",
                options.stl_unit.unwrap_or(SourceUnit::Millimetre)
            ),
            format: MeshSourceFormat::Stl,
            source_unit: options.stl_unit.unwrap_or(SourceUnit::Millimetre),
            name: label.into(),
            notes,
            placements: vec![RigidPoseMm::IDENTITY],
        }]);
    }
    if options.stl_unit.is_some() {
        return Err(invalid(
            "unit override applies only to unitless STL; 3MF declares its units",
        ));
    }
    let mut package = Package::decode(bytes, live_bytes, cancel).map_err(decode_error)?;
    package
        .select_plate(options.source_plate)
        .map_err(decode_error)?;
    let mut notes = vec!["Geometry imported; foreign printer/process settings, custom G-code and appearance metadata were not applied.".into()];
    if package.plates.len() > 1 {
        notes.push(format!(
            "Source contains {} plates; each import selects one source plate.",
            package.plates.len()
        ));
    }
    let leaves = package.leaves(cancel).map_err(decode_error)?;
    if leaves.iter().any(|leaf| leaf.role != Role::Model) {
        notes.push(
            "Process modifiers and support controls were excluded from model material.".into(),
        );
    }
    if leaves.iter().any(|l| l.role == Role::Negative) {
        return Err(invalid(
            "3MF contains negative volumes; subtractive component composition is not available for this construction",
        ));
    }
    let mut results: Vec<DecodedSource> = Vec::new();
    let mut keys = std::collections::BTreeMap::<_, usize>::new();
    for leaf in leaves {
        if leaf.role != Role::Model {
            continue;
        }
        crate::check_cancel(cancel)?;
        let (linear, pose) = split_placement(leaf.transform_mm)?;
        let key = (
            leaf.object.clone(),
            linear
                .0
                .map(|p| p.map(|v| if v == 0. { 0 } else { v.to_bits() })),
        );
        if let Some(&index) = keys.get(&key) {
            results[index].placements.push(pose);
            continue;
        }
        let selector = format!("3mf:{}:{}:surface1", leaf.object.part, leaf.object.id);
        let (complex, duplicates) = normalize::build((*leaf.mesh).clone(), 1., linear, cancel)?;
        let mut notes = notes.clone();
        if duplicates > 0 {
            notes.push(format!(
                "Removed {duplicates} duplicate facets with identical winding."
            ));
        }
        keys.insert(key, results.len());
        results.push(DecodedSource {
            complex,
            selector,
            format: MeshSourceFormat::ThreeMf,
            source_unit: match leaf.source_scale_to_mm {
                0.001 => SourceUnit::Micrometre,
                10. => SourceUnit::Centimetre,
                25.4 => SourceUnit::Inch,
                304.8 => SourceUnit::Foot,
                1000. => SourceUnit::Metre,
                _ => SourceUnit::Millimetre,
            },
            name: leaf.name,
            notes: notes.clone(),
            placements: vec![pose],
        });
    }
    if results.is_empty() {
        return Err(invalid("3MF contains no printable editable model geometry"));
    }
    Ok(results)
}
impl SourceDefinition {
    pub fn display(
        &self,
        profile: DisplayProfile,
        cancel: &AtomicBool,
        budget: crate::MeshBudget,
        work: &mut crate::GeometryWorkBudget,
        progress: &mut impl FnMut(crate::DisplayProgress),
    ) -> Result<crate::MeshArtifact, GeometryError> {
        match self {
            Self::Native(d) => {
                crate::tessellate_with_work(d, profile, cancel, budget, work, progress)
            }
            Self::Hybrid(d) => {
                let c = &d.complex;
                let mut builder = spiling_contracts::display::MeshChunkBuilder::with_budget(
                    d.id.clone(),
                    c.faces.len() as u32,
                    profile,
                    budget,
                )?;
                let mut by_region = vec![Vec::new(); c.faces.len()];
                for (t, r) in c.triangles.iter().zip(&c.regions) {
                    by_region[*r as usize].push(*t);
                }
                for (r, triangles) in by_region.into_iter().enumerate() {
                    crate::check_cancel(cancel)?;
                    let mut positions = Vec::new();
                    let mut normals = Vec::new();
                    let mut ts = Vec::new();
                    for t in triangles {
                        let p = t.map(|i| c.positions[i as usize]);
                        let a = std::array::from_fn::<_, 3, _>(|i| p[1][i] - p[0][i]);
                        let b = std::array::from_fn::<_, 3, _>(|i| p[2][i] - p[0][i]);
                        let n = [
                            a[1] * b[2] - a[2] * b[1],
                            a[2] * b[0] - a[0] * b[2],
                            a[0] * b[1] - a[1] * b[0],
                        ];
                        let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
                        let n = n.map(|v| v / length);
                        let i = positions.len() as u32;
                        positions.extend_from_slice(&p);
                        normals.extend_from_slice(&[n; 3]);
                        ts.push([i, i + 1, i + 2]);
                    }
                    builder.push_face(r as u32, &positions, &normals, &ts, c.error_mm, 0.)?;
                }
                builder.finish()
            }
        }
    }
}
impl SourceDefinition {
    pub fn vertical_wall_face_with_work(
        &self,
        face: &FaceId,
        work: &mut crate::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<crate::VerticalWallFace, GeometryError> {
        self.native()?
            .vertical_wall_face_with_work(face, work, cancel)
    }
    pub fn vertical_cylinder_face_with_work(
        &self,
        face: &FaceId,
        work: &mut crate::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<crate::VerticalCylinderFace, GeometryError> {
        self.native()?
            .vertical_cylinder_face_with_work(face, work, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_model_import::{Affine, Mesh};
    fn cube(lo: f64, hi: f64) -> Mesh {
        Mesh {
            vertices: vec![
                [lo, lo, lo],
                [hi, lo, lo],
                [hi, hi, lo],
                [lo, hi, lo],
                [lo, lo, hi],
                [hi, lo, hi],
                [hi, hi, hi],
                [lo, hi, hi],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        }
    }
    #[test]
    fn planar_box_and_retained_return() {
        let cancel = AtomicBool::new(false);
        let mut mesh = cube(0., 10.);
        mesh.vertices.push([1000., 1000., 1000.]);
        let c = SurfaceComplex::normalize(mesh, 1., Affine::IDENTITY, &cancel).unwrap();
        assert!(
            c.bounds()
                .unwrap()
                .max
                .iter()
                .all(|v| (*v - 10.).abs() < 1e-9)
        );
        assert_eq!(c.faces.len(), 6);
        assert!(
            c.faces
                .iter()
                .all(|f| f.geometry.family == SurfaceFamily::Plane)
        );
        let bytes = c.encode().unwrap();
        let restored = SurfaceComplex::decode(&bytes).unwrap();
        assert_eq!(restored.encode().unwrap(), bytes);
        let model = TolerantModel::from_complex(&restored, TolerantPolicy::default()).unwrap();
        let section = model.section(5.).unwrap();
        assert_eq!(section.loops.len(), 1);
        assert!((section.loops[0].area - 100.).abs() < 1e-10);
        assert!(SurfaceComplex::decode(&bytes[..bytes.len() - 1]).is_err());
    }
    #[test]
    fn placed_inspection_returns_closed_loops_on_an_oblique_plane() {
        let cancel = AtomicBool::new(false);
        let complex =
            SurfaceComplex::normalize(cube(0., 10.), 1., Affine::IDENTITY, &cancel).unwrap();
        let bytes = complex.encode().unwrap();
        let provenance = SourceProvenance {
            source_hash: SourceHash::from_bytes(b"original box"),
            source_name: "box.stl".into(),
            source_unit: SourceUnit::Millimetre,
            uncertainty_mm: None,
            source_solid: None,
            normalized: Some(NormalizedSource {
                hash: SourceHash::from_bytes(&bytes),
                format: MeshSourceFormat::Stl,
                selector: "box".into(),
                notes: vec![],
            }),
        };
        let source = SourceDefinition::restore(provenance, &bytes, &cancel).unwrap();
        let loops = source
            .section_placed(
                RigidPoseMm::IDENTITY,
                PlaneMm {
                    origin_mm: [5., 5., 5.],
                    normal: [1., 1., 1.],
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].points_mm.first(), loops[0].points_mm.last());
        for p in &loops[0].points_mm {
            assert!((p.iter().sum::<f64>() - 15.).abs() < 1e-9);
        }
    }

    #[test]
    fn cooriented_duplicate_cleanup_is_reported_but_opposed_facets_refuse() {
        let cancel = AtomicBool::new(false);
        let mut mesh = cube(0., 10.);
        mesh.triangles.push(mesh.triangles[0]);
        let (complex, count) = normalize::build(mesh, 1., Affine::IDENTITY, &cancel).unwrap();
        assert_eq!(count, 1);
        let section = TolerantModel::from_complex(&complex, TolerantPolicy::default())
            .unwrap()
            .section(5.)
            .unwrap();
        assert!((section.loops[0].area - 100.).abs() < 1e-10);
        let mut mesh = cube(0., 10.);
        let mut duplicate = mesh.triangles[0];
        duplicate.swap(1, 2);
        mesh.triangles.push(duplicate);
        assert!(normalize::build(mesh, 1., Affine::IDENTITY, &cancel).is_err());
    }

    #[test]
    fn nested_shell_keeps_a_void_after_winding_correction() {
        let cancel = AtomicBool::new(false);
        let mut mesh = cube(0., 10.);
        let inside = cube(2., 8.);
        mesh.triangles
            .extend(inside.triangles.into_iter().map(|t| t.map(|v| v + 8)));
        mesh.vertices.extend(inside.vertices);
        let c = SurfaceComplex::normalize(mesh, 1., Affine::IDENTITY, &cancel).unwrap();
        let model = TolerantModel::from_complex(&c, TolerantPolicy::default()).unwrap();
        let section = model.section(5.).unwrap();
        assert_eq!(section.loops.len(), 2);
        assert!((section.loops.iter().map(|l| l.area).sum::<f64>() - 64.).abs() < 1e-10);
        assert!(!model.support_voids(&cancel).unwrap().is_empty());
    }
    #[test]
    fn filleted_outer_wall_keeps_planar_caps_and_an_analytic_bore() {
        let n = 128u32;
        let mut profile = vec![(5., 0.)];
        for i in 1..=8 {
            let a = std::f64::consts::FRAC_PI_2 * i as f64 / 8.;
            profile.push((5. + a.sin(), 1. - a.cos()));
        }
        profile.push((6., 7.));
        for i in (0..8).rev() {
            let a = std::f64::consts::FRAC_PI_2 * i as f64 / 8.;
            profile.push((5. + a.sin(), 7. + a.cos()));
        }
        let rings = profile.len() as u32;
        let mut mesh = Mesh::default();
        for &(radius, z) in &profile {
            for i in 0..n {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                mesh.vertices.push([radius * a.cos(), radius * a.sin(), z]);
            }
        }
        for z in [0., 8.] {
            for i in 0..n {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                mesh.vertices.push([2. * a.cos(), 2. * a.sin(), z]);
            }
        }
        for ring in 0..rings - 1 {
            for i in 0..n {
                let j = (i + 1) % n;
                let a = ring * n;
                let b = a + n;
                mesh.triangles
                    .extend([[a + i, a + j, b + j], [a + i, b + j, b + i]]);
            }
        }
        let inner = rings * n;
        let top = (rings - 1) * n;
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.triangles.extend([
                [i, inner + j, j],
                [i, inner + i, inner + j],
                [top + i, top + j, inner + n + j],
                [top + i, inner + n + j, inner + n + i],
                [inner + i, inner + n + j, inner + j],
                [inner + i, inner + n + i, inner + n + j],
            ]);
        }
        let cancel = AtomicBool::new(false);
        let c = SurfaceComplex::normalize(mesh, 1., Affine::IDENTITY, &cancel).unwrap();
        for family in [
            SurfaceFamily::Plane,
            SurfaceFamily::Cylinder,
            SurfaceFamily::Faceted,
        ] {
            assert!(
                c.faces.iter().any(|f| f.geometry.family == family),
                "missing {family:?}"
            );
        }
        let section = TolerantModel::from_complex(&c, TolerantPolicy::default())
            .unwrap()
            .section(4.)
            .unwrap();
        assert_eq!(section.loops.len(), 2);
        assert!(
            section
                .circles
                .iter()
                .any(|circle| (circle.radius - 2.).abs() < 1e-10)
        );
    }

    #[test]
    fn cylindrical_recovery_bounds_chord_interiors() {
        fn cylinder(n: u32) -> Mesh {
            let mut mesh = Mesh::default();
            for z in [0., 8.] {
                for i in 0..n {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    mesh.vertices.push([4. * a.cos(), 4. * a.sin(), z]);
                }
            }
            mesh.vertices.extend([[0., 0., 0.], [0., 0., 8.]]);
            for i in 0..n {
                let j = (i + 1) % n;
                mesh.triangles.extend([
                    [i, j, n + j],
                    [i, n + j, n + i],
                    [2 * n, j, i],
                    [2 * n + 1, n + i, n + j],
                ]);
            }
            mesh
        }
        let cancel = AtomicBool::new(false);
        let fine = SurfaceComplex::normalize(cylinder(128), 1., Affine::IDENTITY, &cancel).unwrap();
        assert!(
            fine.faces
                .iter()
                .any(|f| f.geometry.family == SurfaceFamily::Cylinder)
        );
        let model = TolerantModel::from_complex(&fine, TolerantPolicy::default()).unwrap();
        let section = model.section(4.).unwrap();
        assert_eq!(section.circles.len(), 1);
        assert!((section.circles[0].radius - 4.).abs() < 1e-10);
        let coarse =
            SurfaceComplex::normalize(cylinder(16), 1., Affine::IDENTITY, &cancel).unwrap();
        assert!(
            coarse
                .faces
                .iter()
                .all(|f| f.geometry.family != SurfaceFamily::Cylinder)
        );
    }
}

/// Convert an orthonormal column matrix to a checked rigid pose. Used for
/// package placement and section frames; non-rigid maps stay baked geometry.
fn rigid_pose(columns: [[f64; 3]; 3], translation: [f64; 3]) -> Result<RigidPoseMm, GeometryError> {
    let m = |r: usize, c: usize| columns[c][r];
    let trace = m(0, 0) + m(1, 1) + m(2, 2);
    let q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (m(2, 1) - m(1, 2)) / s,
            (m(0, 2) - m(2, 0)) / s,
            (m(1, 0) - m(0, 1)) / s,
            s / 4.,
        ]
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1. + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.;
        [
            s / 4.,
            (m(0, 1) + m(1, 0)) / s,
            (m(0, 2) + m(2, 0)) / s,
            (m(2, 1) - m(1, 2)) / s,
        ]
    } else if m(1, 1) > m(2, 2) {
        let s = (1. + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.;
        [
            (m(0, 1) + m(1, 0)) / s,
            s / 4.,
            (m(1, 2) + m(2, 1)) / s,
            (m(0, 2) - m(2, 0)) / s,
        ]
    } else {
        let s = (1. + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.;
        [
            (m(0, 2) + m(2, 0)) / s,
            (m(1, 2) + m(2, 1)) / s,
            s / 4.,
            (m(1, 0) - m(0, 1)) / s,
        ]
    };
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let pose = RigidPoseMm {
        translation_mm: translation,
        rotation_xyzw: q.map(|v| v / n),
    };
    pose.validate()?;
    Ok(pose)
}
impl SourceDefinition {
    pub fn section_placed(
        &self,
        pose: RigidPoseMm,
        plane: PlaneMm,
        cancel: &AtomicBool,
    ) -> Result<Vec<crate::SectionLoopMm>, GeometryError> {
        let frame = PlaneFrameMm::from_plane(plane)?;
        let axes = [frame.x_axis, frame.y_axis, frame.z_axis];
        let columns = std::array::from_fn(|a| std::array::from_fn(|b| axes[b][a]));
        let translation =
            axes.map(|axis| -(0..3).map(|a| axis[a] * frame.origin_mm[a]).sum::<f64>());
        let inverse = rigid_pose(columns, translation)?;
        let model = self
            .prepare(TolerantPolicy::default(), cancel)?
            .placed(&pose)
            .placed(&inverse);
        let section = model.section(0.)?;
        crate::check_cancel(cancel)?;
        Ok(section
            .loops
            .into_iter()
            .map(|boundary| {
                let mut points_mm: Vec<_> = boundary
                    .points
                    .into_iter()
                    .map(|p| {
                        std::array::from_fn(|a| {
                            frame.origin_mm[a] + p[0] * frame.x_axis[a] + p[1] * frame.y_axis[a]
                        })
                    })
                    .collect();
                if let Some(first) = points_mm.first().copied() {
                    points_mm.push(first);
                }
                crate::SectionLoopMm {
                    is_hole: boundary.area < 0.,
                    points_mm,
                }
            })
            .collect())
    }
}

/// Factor the proper rigid part from a general affine. Scale, shear and
/// reflection remain in shared geometry, never in RigidPoseMm.
fn split_placement(
    affine: spiling_model_import::Affine,
) -> Result<(spiling_model_import::Affine, RigidPoseMm), GeometryError> {
    let dot = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f64>();
    let norm = |a: [f64; 3]| {
        let n = dot(a, a).sqrt();
        a.map(|v| v / n)
    };
    let x = norm(affine.0[0]);
    let projection = dot(x, affine.0[1]);
    let y = norm(std::array::from_fn(|a| affine.0[1][a] - projection * x[a]));
    let z = [
        x[1] * y[2] - x[2] * y[1],
        x[2] * y[0] - x[0] * y[2],
        x[0] * y[1] - x[1] * y[0],
    ];
    let axes = [x, y, z];
    let pose = rigid_pose(axes, affine.0[3])?;
    let mut residual = spiling_model_import::Affine::IDENTITY;
    for i in 0..3 {
        residual.0[i] = axes.map(|axis| dot(axis, affine.0[i]));
    }
    residual.validate().map_err(|e| invalid(e.to_string()))?;
    Ok((residual, pose))
}
