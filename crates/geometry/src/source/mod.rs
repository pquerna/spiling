// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! One adopted material interpretation, with native or mixed surface storage.
mod composition;
mod csg;
mod deviation;
mod normalize;
mod packed;
mod topology;
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
    pub findings: Vec<ImportFinding>,
    pub placements: Vec<RigidPoseMm>,
}
/// Decode syntax and adopt geometry once; rigid translation stays occurrence
/// data. Repeated package instances with the same linear map share a source.
/// A rejected normalization attempt may retain checked facets for diagnostic
/// display only. It never becomes a SourceDefinition or material-query operand.
pub use spiling_model_import::Mesh as RejectedMesh;
#[derive(Debug)]
pub struct RejectedSource {
    pub error: GeometryError,
    pub mesh: Option<Box<spiling_model_import::Mesh>>,
}
impl From<GeometryError> for RejectedSource {
    fn from(error: GeometryError) -> Self {
        Self { error, mesh: None }
    }
}
impl RejectedSource {
    pub(super) fn object(mut self, name: String) -> Self {
        if let Some(info) = &mut self.error.import_diagnostic {
            info.object_name = bounded_text(&name, MAX_DISPLAY_LABEL_BYTES as usize);
        }
        self
    }
    fn material(
        mesh: spiling_model_import::Mesh,
        mut error: GeometryError,
        cancel: &AtomicBool,
    ) -> Self {
        if !matches!(
            error.code,
            GeometryErrorCode::UnsupportedGeometry | GeometryErrorCode::InvalidGeometry
        ) {
            return error.into();
        }
        let mut edges = std::collections::BTreeMap::<(u32, u32), usize>::new();
        for (i, t) in mesh.triangles.iter().enumerate() {
            if i % 1024 == 0 && crate::check_cancel(cancel).is_err() {
                return GeometryError::new(
                    GeometryErrorCode::Cancelled,
                    "import diagnostic cancelled",
                )
                .into();
            }
            for a in 0..3 {
                let (v, w) = (t[a], t[(a + 1) % 3]);
                *edges.entry((v.min(w), v.max(w))).or_default() += 1;
            }
        }
        let mut points = Vec::new();
        let mut kind = ImportFailureKind::UnresolvedMaterial;
        for ((a, b), uses) in edges {
            if uses != 2 {
                if uses > 2 {
                    kind = ImportFailureKind::NonManifold;
                } else if kind != ImportFailureKind::NonManifold {
                    kind = ImportFailureKind::OpenBoundary;
                }
                points.extend([mesh.vertices[a as usize], mesh.vertices[b as usize]]);
            }
        }
        for &t in &mesh.triangles {
            if normalize::normal(&mesh.vertices, t).is_none() {
                kind = ImportFailureKind::CollapsedFacet;
                points.extend(t.map(|v| mesh.vertices[v as usize]));
            }
        }
        error.import_diagnostic = Some(Box::new(ImportDiagnostic {
            kind,
            source_name: "Source".into(),
            object_name: String::new(),
            source_hash: None,
            bounds_mm: if points.is_empty() {
                None
            } else {
                topology::point_bounds(&points).ok()
            },
            preview: None,
            preview_definition: None,
        }));
        Self {
            error,
            mesh: Some(Box::new(mesh)),
        }
    }
}

pub fn decode_container(
    bytes: &[u8],
    label: &str,
    format: MeshSourceFormat,
    live_bytes: usize,
    options: &ModelImportOptions,
    cancel: &AtomicBool,
) -> Result<Vec<DecodedSource>, GeometryError> {
    decode_container_report(bytes, label, format, live_bytes, options, cancel)
        .map_err(|rejected| rejected.error)
}
pub fn decode_container_report(
    bytes: &[u8],
    label: &str,
    format: MeshSourceFormat,
    live_bytes: usize,
    options: &ModelImportOptions,
    cancel: &AtomicBool,
) -> Result<Vec<DecodedSource>, RejectedSource> {
    decode_container_inner(bytes, label, format, live_bytes, options, cancel).map_err(
        |mut rejected| {
            if let Some(info) = &mut rejected.error.import_diagnostic {
                info.source_name = bounded_text(label, MAX_DISPLAY_LABEL_BYTES as usize);
                info.source_hash = Some(SourceHash::from_bytes(bytes));
            }
            rejected
        },
    )
}
fn decode_container_inner(
    bytes: &[u8],
    label: &str,
    format: MeshSourceFormat,
    live_bytes: usize,
    options: &ModelImportOptions,
    cancel: &AtomicBool,
) -> Result<Vec<DecodedSource>, RejectedSource> {
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
            return Err(invalid("STL has no source-plate selection").into());
        }
        let mesh = spiling_model_import::stl::decode(bytes, cancel).map_err(decode_error)?;
        let (complex, mut findings) = normalize::build_report(
            mesh,
            options
                .stl_unit
                .unwrap_or(SourceUnit::Millimetre)
                .scale_to_mm(),
            Affine::IDENTITY,
            cancel,
        )?;
        findings.push(finding(
            ImportFindingKind::Information,
            ImportFindingCode::InterpretedUnits,
            1,
            "STL units",
            "Unitless source interpreted using the selected units.",
            None,
            None,
        ));
        return Ok(vec![DecodedSource {
            complex,
            selector: format!(
                "stl:{:?}:surface1",
                options.stl_unit.unwrap_or(SourceUnit::Millimetre)
            ),
            format: MeshSourceFormat::Stl,
            source_unit: options.stl_unit.unwrap_or(SourceUnit::Millimetre),
            name: label.into(),
            findings,
            placements: vec![RigidPoseMm::IDENTITY],
        }]);
    }
    if options.stl_unit.is_some() {
        return Err(
            invalid("unit override applies only to unitless STL; 3MF declares its units").into(),
        );
    }
    let mut package = Package::decode(bytes, live_bytes, cancel).map_err(decode_error)?;
    let selected_plate = package
        .select_plate(options.source_plate)
        .map_err(decode_error)?;
    let mut findings = Vec::new();
    if !package.omitted.is_empty() {
        findings.push(finding(
            ImportFindingKind::NotApplied,
            ImportFindingCode::ForeignSettings,
            package.omitted.len() as u32,
            "Package settings",
            &bounded_text(&format!("Not applied: {}. Current setup is retained; source scripts are never executed.",package.omitted.iter().cloned().collect::<Vec<_>>().join(", ")),MAX_DISPLAY_LABEL_BYTES as usize),
            None,
            None,
        ));
    }
    if package.plates.len() > 1 {
        findings.push(finding(
            ImportFindingKind::Information,
            ImportFindingCode::SourcePlate,
            package.plates.len() as u32,
            &format!("Plate {selected_plate}/{}", package.plates.len()),
            "Selected source plate imported; other source plates remain in the original package.",
            None,
            None,
        ));
    }
    let leaves = package.leaves(cancel).map_err(decode_error)?;
    if leaves.iter().any(|l| l.role == Role::Negative) {
        return composition::negative_objects(&package, leaves, &findings, selected_plate, cancel)
            .map_err(|mut failure| {
                if failure.error.import_diagnostic.is_none()
                    && matches!(
                        failure.error.code,
                        GeometryErrorCode::UnsupportedGeometry | GeometryErrorCode::InvalidGeometry
                    )
                {
                    failure.error.import_diagnostic = Some(Box::new(ImportDiagnostic {
                        kind: ImportFailureKind::Composition,
                        source_name: "Source".into(),
                        object_name: format!("{} · source plate {}", package.root, selected_plate),
                        source_hash: None,
                        bounds_mm: None,
                        preview: None,
                        preview_definition: None,
                    }));
                }
                failure
            });
    }
    let controls: Vec<_> = leaves
        .iter()
        .filter(|l| !matches!(l.role, Role::Model | Role::Negative))
        .collect();
    let mut results: Vec<DecodedSource> = Vec::new();
    let mut keys = std::collections::BTreeMap::<_, usize>::new();
    for leaf in &leaves {
        if leaf.role != Role::Model {
            continue;
        }
        crate::check_cancel(cancel)?;
        let (linear, pose) = split_placement(leaf.transform_mm)?;
        let key = (
            (
                package.models[&package.root].build[leaf.build_index]
                    .object
                    .clone(),
                leaf.object.clone(),
                leaf.volume_index,
            ),
            linear
                .0
                .map(|p| p.map(|v| if v == 0. { 0 } else { v.to_bits() })),
        );
        if let Some(&index) = keys.get(&key) {
            results[index].placements.push(pose);
            continue;
        }
        let owner = &package.models[&package.root].build[leaf.build_index].object;
        let selector = format!(
            "3mf:{}:{}:owner{}:{}:plate{}:volume{}:surface1",
            leaf.object.part,
            leaf.object.id,
            owner.part,
            owner.id,
            selected_plate,
            leaf.volume_index
        );
        let (complex, mut local_findings) =
            normalize::build_report((*leaf.mesh).clone(), 1., linear, cancel).map_err(
                |failure| {
                    failure.object(format!(
                        "{}:{} · {}",
                        leaf.object.part, leaf.object.id, leaf.name
                    ))
                },
            )?;
        local_findings.extend(findings.clone());
        for control in controls
            .iter()
            .filter(|l| l.build_index == leaf.build_index)
        {
            let transform = composition::relative_transform(control.transform_mm, pose);
            local_findings.push(composition::control_finding(control, transform)?);
        }
        keys.insert(key, results.len());
        results.push(DecodedSource {
            complex,
            selector,
            format: MeshSourceFormat::ThreeMf,
            source_unit: composition::source_unit(leaf.source_scale_to_mm),
            name: leaf.name.clone(),
            findings: local_findings,
            placements: vec![pose],
        });
    }
    if results.is_empty() {
        return Err(invalid("3MF contains no printable editable model geometry").into());
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
                    let mut normals: Vec<[f64; 3]> = Vec::new();
                    let mut ts = Vec::new();
                    let mut indices = std::collections::HashMap::<u32, u32>::new();
                    for (index, t) in triangles.into_iter().enumerate() {
                        if index % 1024 == 0 {
                            crate::check_cancel(cancel)?;
                        }
                        let p = t.map(|i| c.positions[i as usize]);
                        let n = normalize::normal(&c.positions, t)
                            .ok_or_else(|| invalid("source display has a degenerate facet"))?;
                        let local = std::array::from_fn(|slot| {
                            *indices.entry(t[slot]).or_insert_with(|| {
                                let next = positions.len() as u32;
                                positions.push(p[slot]);
                                normals.push([0.; 3]);
                                next
                            })
                        });
                        for &vertex in &local {
                            for a in 0..3 {
                                normals[vertex as usize][a] += n[a];
                            }
                        }
                        ts.push(local);
                    }
                    for n in &mut normals {
                        let length = normalize::dot(*n, *n).sqrt();
                        *n = if length > 0. {
                            n.map(|v| v / length)
                        } else {
                            [0., 0., 1.]
                        };
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
    pub(super) fn cube(lo: f64, hi: f64) -> Mesh {
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
                object_name: "Imported part".into(),
                findings: vec![],
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
        assert!(
            count
                .iter()
                .any(|f| f.code == ImportFindingCode::DuplicateFacets && f.count == 1)
        );
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

fn finding(
    kind: ImportFindingKind,
    code: ImportFindingCode,
    count: u32,
    label: &str,
    detail: &str,
    bounds_mm: Option<AabbMm>,
    displacement_mm: Option<f64>,
) -> ImportFinding {
    ImportFinding {
        kind,
        code,
        count,
        label: label.into(),
        detail: detail.into(),
        bounds_mm,
        displacement_mm,
    }
}

#[cfg(test)]
mod import_tests;
