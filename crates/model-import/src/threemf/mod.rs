// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Practical 3MF: package relationships, model graphs and slicer part roles.
mod model;
mod package;
use crate::{Affine, Mesh, Result, check, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectKey {
    pub part: String,
    pub id: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Model,
    Negative,
    Modifier,
    SupportBlocker,
    SupportEnforcer,
}
#[derive(Debug)]
pub struct Object {
    pub name: String,
    pub mesh: Option<Arc<Mesh>>,
    pub components: Vec<Instance>,
    pub role: Role,
    pub volumes: Vec<(Arc<Mesh>, Role)>,
}
#[derive(Debug, Clone)]
pub struct Instance {
    pub object: ObjectKey,
    pub transform: Affine,
    pub printable: bool,
}
#[derive(Debug)]
pub struct Model {
    pub scale_to_mm: f64,
    pub objects: BTreeMap<u32, Object>,
    pub build: Vec<Instance>,
}
#[derive(Debug)]
pub struct Package {
    pub root: String,
    pub models: BTreeMap<String, Model>,
    pub omitted: BTreeSet<String>,
    pub plates: BTreeMap<u32, Vec<(u32, u32)>>,
    pub roles: BTreeMap<(ObjectKey, ObjectKey), Role>,
}
#[derive(Debug)]
pub struct Leaf {
    pub object: ObjectKey,
    pub mesh: Arc<Mesh>,
    pub volume_index: usize,
    pub transform_mm: Affine,
    pub source_scale_to_mm: f64,
    pub role: Role,
    pub name: String,
    /// Root build index keeps distinct logical assemblies separate.
    pub build_index: usize,
}
impl Package {
    pub fn decode(bytes: &[u8], live_bytes: usize, cancel: &AtomicBool) -> Result<Self> {
        package::decode(bytes, live_bytes, cancel)
    }
    /// Select one physical source plate. No coordinate-offset heuristic and no
    /// accidental assembly of several beds into the current layout.
    pub fn select_plate(&mut self, requested: Option<u32>) -> Result<u32> {
        if self.plates.is_empty() {
            if requested.is_some_and(|p| p != 1) {
                return Err(invalid("3MF has only its default source plate"));
            }
            return Ok(1);
        }
        let selected = requested.unwrap_or(*self.plates.keys().next().unwrap());
        let members = self
            .plates
            .get(&selected)
            .ok_or_else(|| invalid(format!("3MF source plate {selected} is absent")))?;
        let root = self.models.get_mut(&self.root).unwrap();
        let mut ordinal = BTreeMap::<u32, u32>::new();
        let mut paths = BTreeMap::<u32, String>::new();
        for item in &mut root.build {
            if let Some(old) = paths.insert(item.object.id, item.object.part.clone())
                && old != item.object.part
            {
                return Err(invalid(
                    "3MF plate metadata cannot distinguish equal IDs in different model parts",
                ));
            }
            let instance = ordinal.entry(item.object.id).or_default();
            item.printable &= members.contains(&(item.object.id, *instance));
            *instance += 1;
        }
        for &(object, instance) in members {
            if ordinal.get(&object).is_none_or(|n| instance >= *n) {
                return Err(invalid("3MF plate refers to a missing build instance"));
            }
        }
        Ok(selected)
    }
    /// Resolve component DAGs iteratively: no guessed recursion limit or
    /// global visited set that would discard repeated instances.
    pub fn leaves(&self, cancel: &AtomicBool) -> Result<Vec<Leaf>> {
        let root = self
            .models
            .get(&self.root)
            .ok_or_else(|| invalid("missing root model"))?;
        enum Visit<'a> {
            Enter(&'a ObjectKey, Affine, Role, usize),
            Exit(&'a ObjectKey),
        }
        let mut stack = Vec::new();
        let mut active = BTreeSet::new();
        let mut leaves = Vec::new();
        for (index, item) in root.build.iter().enumerate().rev() {
            if item.printable {
                stack.push(Visit::Enter(
                    &item.object,
                    item.transform,
                    Role::Model,
                    index,
                ));
            }
        }
        while let Some(visit) = stack.pop() {
            check(cancel)?;
            match visit {
                Visit::Exit(key) => {
                    active.remove(key);
                }
                Visit::Enter(key, transform, inherited, build_index) => {
                    if !active.insert(key) {
                        return Err(invalid("3MF component reference cycle"));
                    }
                    let model = self
                        .models
                        .get(&key.part)
                        .ok_or_else(|| invalid("missing referenced 3MF model part"))?;
                    let object = model
                        .objects
                        .get(&key.id)
                        .ok_or_else(|| invalid("missing referenced 3MF object"))?;
                    let mut scoped = None;
                    for owner in &active {
                        if let Some(&role) = self.roles.get(&((*owner).clone(), key.clone())) {
                            if scoped.is_some_and(|previous| previous != role) {
                                return Err(invalid("conflicting nested 3MF material roles"));
                            }
                            scoped = Some(role);
                        }
                    }
                    let role = if inherited == Role::Model {
                        scoped.unwrap_or(object.role)
                    } else {
                        inherited
                    };
                    if object.mesh.is_some() || !object.volumes.is_empty() {
                        let mut scale = Affine::IDENTITY;
                        for (axis, row) in scale.0.iter_mut().take(3).enumerate() {
                            row[axis] = model.scale_to_mm;
                        }
                        let volumes = if object.volumes.is_empty() {
                            vec![(
                                Arc::clone(object.mesh.as_ref().expect("mesh or volumes")),
                                role,
                            )]
                        } else {
                            object
                                .volumes
                                .iter()
                                .map(|(mesh, volume_role)| {
                                    (
                                        Arc::clone(mesh),
                                        if role == Role::Model {
                                            *volume_role
                                        } else {
                                            role
                                        },
                                    )
                                })
                                .collect()
                        };
                        for (volume_index, (mesh, role)) in volumes.into_iter().enumerate() {
                            leaves.push(Leaf {
                                source_scale_to_mm: model.scale_to_mm,
                                object: key.clone(),
                                mesh,
                                volume_index,
                                transform_mm: scale.then(transform).validate()?,
                                role,
                                name: object.name.clone(),
                                build_index,
                            });
                        }
                    }
                    stack.push(Visit::Exit(key));
                    for component in object.components.iter().rev() {
                        if !component.printable {
                            continue;
                        }
                        let mut local = component.transform;
                        local.0[3] = local.0[3].map(|v| v * model.scale_to_mm);
                        stack.push(Visit::Enter(
                            &component.object,
                            local.then(transform).validate()?,
                            role,
                            build_index,
                        ));
                    }
                }
            }
        }
        Ok(leaves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    fn package(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/root.model"/></Relationships>"#;
    const MESH: &str = r#"<object id="2"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#;
    #[test]
    fn production_part_graph_units_affines_and_repeated_instances() {
        let root = r#"<model unit="inch" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p"><resources/><build><item objectid="1" p:path="/3D/Objects/sub.model" transform="1 0 0 0 1 0 0 0 1 2 0 0"/><item objectid="1" p:path="/3D/Objects/sub.model" transform="1 0 0 0 1 0 0 0 1 4 0 0"/></build></model>"#;
        let sub = format!(
            r#"<model unit="centimeter"><resources><object id="1"><components><component objectid="2" transform="-2 0 0 0 1 0 0 0 1 1 0 0"/></components></object>{MESH}</resources></model>"#
        );
        let bytes = package(&[
            ("_rels/.rels", RELS),
            ("3D/root.model", root),
            ("3D/Objects/sub.model", &sub),
        ]);
        let cancel = AtomicBool::new(false);
        let decoded = Package::decode(&bytes, usize::MAX, &cancel).unwrap();
        let leaves = decoded.leaves(&cancel).unwrap();
        assert_eq!(leaves.len(), 2);
        assert!(Arc::ptr_eq(&leaves[0].mesh, &leaves[1].mesh));
        assert_eq!(leaves[0].transform_mm.apply([1., 0., 0.]), [40.8, 0., 0.]);
        assert_eq!(leaves[1].transform_mm.apply([1., 0., 0.]), [91.6, 0., 0.]);
        assert!(leaves[0].transform_mm.determinant() < 0.);
    }
    #[test]
    fn cycles_unknown_required_extensions_and_bad_transforms_refuse() {
        let cancel = AtomicBool::new(false);
        for body in [
            r#"<model><resources><object id="1"><components><component objectid="1"/></components></object></resources><build><item objectid="1"/></build></model>"#,
            r#"<model requiredextensions="x" xmlns:x="unknown"><resources/><build/></model>"#,
            r#"<model xmlns:cura="http://software.ultimaker.com/xml/cura/3mf/2015/10"><resources/><cura:setting/><build/></model>"#,
            r#"<model><resources/><build><item objectid="1" transform="bad"/></build></model>"#,
        ] {
            let bytes = package(&[("_rels/.rels", RELS), ("3D/root.model", body)]);
            assert!(
                Package::decode(&bytes, usize::MAX, &cancel)
                    .and_then(|p| p.leaves(&cancel))
                    .is_err()
            );
        }
    }
    #[test]
    fn negatives_are_never_positive_material_and_modifiers_are_excluded() {
        let body = format!(
            "<model><resources>{MESH}</resources><build><item objectid=\"2\"/></build></model>"
        );
        let meta =
            r#"<config><object id="2"><part id="2" subtype="negative_part"/></object></config>"#;
        let bytes = package(&[
            ("_rels/.rels", RELS),
            ("3D/root.model", &body),
            ("Metadata/model_settings.config", meta),
        ]);
        let cancel = AtomicBool::new(false);
        assert_eq!(
            Package::decode(&bytes, usize::MAX, &cancel)
                .unwrap()
                .leaves(&cancel)
                .unwrap()[0]
                .role,
            Role::Negative
        );
        let prusa = r#"<config><object id="2"><volume firstid="0" lastid="0"><metadata key="volume_type" value="NegativeVolume"/></volume></object></config>"#;
        let bytes = package(&[
            ("_rels/.rels", RELS),
            ("3D/root.model", &body),
            ("Metadata/Slic3r_PE_model.config", prusa),
        ]);
        assert_eq!(
            Package::decode(&bytes, usize::MAX, &cancel)
                .unwrap()
                .leaves(&cancel)
                .unwrap()[0]
                .role,
            Role::Negative
        );
    }
}

#[cfg(test)]
mod plate_tests {
    use super::*;
    use std::io::{Cursor, Write};
    #[test]
    fn plate_selection_keeps_instance_membership_and_printable_flags() {
        let root = r#"<model><resources><object id="2"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="2"/><item objectid="2" transform="1 0 0 0 1 0 0 0 1 10 0 0"/><item objectid="2" printable="0"/></build></model>"#;
        let rels = r#"<Relationships><Relationship Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/root.model"/></Relationships>"#;
        let settings = r#"<config><plate><metadata key="plater_id" value="1"/><model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="0"/></model_instance></plate><plate><metadata key="plater_id" value="2"/><model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="1"/></model_instance></plate></config>"#;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in [
            ("_rels/.rels", rels),
            ("3D/root.model", root),
            ("Metadata/model_settings.config", settings),
        ] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let cancel = AtomicBool::new(false);
        let mut first = Package::decode(&bytes, usize::MAX, &cancel).unwrap();
        assert_eq!(first.select_plate(None).unwrap(), 1);
        let leaves = first.leaves(&cancel).unwrap();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].transform_mm.apply([0.; 3]), [0.; 3]);
        let mut second = Package::decode(&bytes, usize::MAX, &cancel).unwrap();
        second.select_plate(Some(2)).unwrap();
        let leaves = second.leaves(&cancel).unwrap();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].transform_mm.apply([0.; 3]), [10., 0., 0.]);
        assert!(
            Package::decode(&bytes, usize::MAX, &cancel)
                .unwrap()
                .select_plate(Some(3))
                .is_err()
        );
    }
}

#[cfg(test)]
mod role_scope_tests {
    use super::*;
    #[test]
    fn reused_resource_roles_belong_to_their_logical_parent() {
        let key = |id| ObjectKey {
            part: "/root.model".into(),
            id,
        };
        let instance = |id| Instance {
            object: key(id),
            transform: Affine::IDENTITY,
            printable: true,
        };
        let part = Object {
            name: "shared".into(),
            mesh: Some(Arc::new(Mesh {
                vertices: vec![[0.; 3], [1., 0., 0.], [0., 1., 0.]],
                triangles: vec![[0, 1, 2]],
            })),
            components: vec![],
            role: Role::Model,
            volumes: vec![],
        };
        let group = || Object {
            name: "assembly".into(),
            mesh: None,
            components: vec![instance(3)],
            role: Role::Model,
            volumes: vec![],
        };
        let package = Package {
            root: "/root.model".into(),
            models: BTreeMap::from([(
                "/root.model".into(),
                Model {
                    scale_to_mm: 1.,
                    objects: BTreeMap::from([(1, group()), (2, group()), (3, part)]),
                    build: vec![instance(1), instance(2)],
                },
            )]),
            omitted: BTreeSet::new(),
            plates: BTreeMap::new(),
            roles: BTreeMap::from([((key(1), key(3)), Role::Negative)]),
        };
        let leaves = package.leaves(&AtomicBool::new(false)).unwrap();
        assert_eq!(leaves[0].role, Role::Negative);
        assert_eq!(leaves[1].role, Role::Model);
        assert!(Arc::ptr_eq(&leaves[0].mesh, &leaves[1].mesh));
    }
}
