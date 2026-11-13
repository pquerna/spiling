// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use quick_xml::{
    NsReader,
    events::{BytesStart, Event},
    name::ResolveResult,
};

pub(super) fn attrs(start: &BytesStart<'_>) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for a in start.attributes() {
        let a = a.map_err(|e| invalid(format!("3MF XML attribute: {e}")))?;
        let key = std::str::from_utf8(a.key.as_ref())
            .map_err(|_| invalid("invalid XML name"))?
            .to_owned();
        let value = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| invalid(format!("3MF XML value: {e}")))?
            .into_owned();
        if result.insert(key, value).is_some() {
            return Err(invalid("duplicate 3MF attribute"));
        }
    }
    Ok(result)
}
fn required<'a>(a: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    a.get(key)
        .map(String::as_str)
        .ok_or_else(|| invalid(format!("missing 3MF {key}")))
}
fn num(a: &BTreeMap<String, String>, key: &str) -> Result<f64> {
    required(a, key)?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| invalid(format!("invalid 3MF {key}")))
}
fn id(a: &BTreeMap<String, String>, key: &str) -> Result<u32> {
    required(a, key)?
        .parse::<u32>()
        .map_err(|_| invalid(format!("invalid 3MF {key}")))
}
fn transform(a: &BTreeMap<String, String>) -> Result<Affine> {
    let Some(text) = a.get("transform") else {
        return Ok(Affine::IDENTITY);
    };
    let values = text
        .split_whitespace()
        .map(|v| v.parse::<f64>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| invalid("invalid 3MF transform"))?;
    let values: [f64; 12] = values
        .try_into()
        .map_err(|_| invalid("3MF transform needs twelve values"))?;
    Affine(std::array::from_fn(|r| {
        std::array::from_fn(|c| values[r * 3 + c])
    }))
    .validate()
}
fn instance(a: &BTreeMap<String, String>, part: &str, path: Option<&str>) -> Result<Instance> {
    let part = match path {
        Some(path) => package::resolve(part, path)?,
        None => part.to_owned(),
    };
    let printable = match a.get("printable").map(String::as_str) {
        None | Some("1" | "true") => true,
        Some("0" | "false") => false,
        _ => return Err(invalid("invalid printable flag")),
    };
    Ok(Instance {
        object: ObjectKey {
            part,
            id: id(a, "objectid")?,
        },
        transform: transform(a)?,
        printable,
    })
}
pub(super) fn decode(bytes: &[u8], part: &str, cancel: &AtomicBool) -> Result<Model> {
    let mut reader = NsReader::from_reader(bytes);
    let mut result = Model {
        scale_to_mm: 1.,
        objects: BTreeMap::new(),
        build: Vec::new(),
    };
    let mut object: Option<(u32, Object)> = None;
    let mut stack = Vec::<Vec<u8>>::new();
    let mut root = false;
    let mut ignored_depth = 0usize;
    loop {
        check(cancel)?;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|e| invalid(format!("3MF XML: {e}")))?;
        let cura_content = matches!(&namespace, ResolveResult::Bound(uri) if String::from_utf8_lossy(uri.as_ref()).contains("/cura/"));
        if cura_content {
            return Err(invalid(
                "Cura workspace elements require explicit manufacturing-role interpretation",
            ));
        }
        let core = match namespace {
            ResolveResult::Unbound => true,
            ResolveResult::Bound(uri) => matches!(
                uri.as_ref(),
                b"http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                    | b"http://schemas.microsoft.com/3dmanufacturing/2013/01"
            ),
            ResolveResult::Unknown(_) => return Err(invalid("unbound 3MF XML namespace")),
        };
        if ignored_depth > 0 || !core {
            match event {
                Event::Start(_) => ignored_depth += 1,
                Event::End(_) => ignored_depth = ignored_depth.saturating_sub(1),
                Event::Eof => break,
                _ => {}
            }
            continue;
        }
        let (start, empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                let name = e.local_name();
                if name.as_ref() == b"object" {
                    let (id, object) = object
                        .take()
                        .ok_or_else(|| invalid("unmatched 3MF object"))?;
                    if object.mesh.is_some() == !object.components.is_empty() {
                        return Err(invalid("3MF object needs either mesh or components"));
                    }
                    if result.objects.insert(id, object).is_some() {
                        return Err(invalid("duplicate 3MF object ID"));
                    }
                }
                stack.pop();
                continue;
            }
            Event::Eof => break,
            Event::DocType(_) => {
                return Err(invalid("3MF external/DTD entities are not supported"));
            }
            _ => continue,
        };
        let local = start.local_name();
        let name = local.as_ref();
        let a = attrs(&start)?;
        let mut production_path = None;
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|e| invalid(format!("3MF path attribute: {e}")))?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            if local.as_ref() != b"path" {
                continue;
            }
            match namespace {
                ResolveResult::Bound(uri)
                    if uri.as_ref()
                        == b"http://schemas.microsoft.com/3dmanufacturing/production/2015/06" =>
                {
                    let key = std::str::from_utf8(attribute.key.as_ref())
                        .map_err(|_| invalid("invalid path attribute"))?;
                    if production_path
                        .replace(a.get(key).map(String::as_str).unwrap())
                        .is_some()
                    {
                        return Err(invalid("duplicate production path attribute"));
                    }
                }
                ResolveResult::Unknown(_) => return Err(invalid("unbound 3MF path namespace")),
                _ => {}
            }
        }
        if name == b"metadata"
            && a.get("name").is_some_and(|key| {
                matches!(
                    key.as_str(),
                    "cura:mesh_type"
                        | "cura:infill_mesh"
                        | "cura:cutting_mesh"
                        | "cura:support_mesh"
                        | "cura:anti_overhang_mesh"
                )
            })
        {
            return Err(invalid(
                "Cura workspace mesh-role metadata is not applied; export model geometry or remove this control",
            ));
        }
        match name {
            b"model" => {
                if root || !stack.is_empty() {
                    return Err(invalid("invalid 3MF model root"));
                }
                root = true;
                result.scale_to_mm = match a.get("unit").map(String::as_str).unwrap_or("millimeter")
                {
                    "micron" => 0.001,
                    "millimeter" => 1.,
                    "centimeter" => 10.,
                    "inch" => 25.4,
                    "foot" => 304.8,
                    "meter" => 1000.,
                    _ => return Err(invalid("unsupported 3MF units")),
                };
                if let Some(required) = a.get("requiredextensions") {
                    for prefix in required.split_whitespace() {
                        let uri = a
                            .get(&format!("xmlns:{prefix}"))
                            .map(String::as_str)
                            .unwrap_or("");
                        if !uri.contains("3dmanufacturing/production/") {
                            return Err(invalid(format!(
                                "unsupported required 3MF extension {prefix}"
                            )));
                        }
                    }
                }
            }
            b"object" => {
                if object.is_some() || stack.last().map(Vec::as_slice) != Some(b"resources") {
                    return Err(invalid("invalid nested 3MF object"));
                }
                let role = match a.get("type").map(String::as_str).unwrap_or("model") {
                    "model" => Role::Model,
                    "support" => Role::SupportEnforcer,
                    "solidsupport" => Role::SupportEnforcer,
                    "other" => Role::Modifier,
                    _ => return Err(invalid("unsupported 3MF object type")),
                };
                let oid = id(&a, "id")?;
                if oid == 0 {
                    return Err(invalid("3MF object ID must be positive"));
                }
                object = Some((
                    oid,
                    Object {
                        name: a
                            .get("name")
                            .filter(|name| !name.trim().is_empty())
                            .cloned()
                            .unwrap_or_else(|| format!("Object {oid}")),
                        mesh: None,
                        components: Vec::new(),
                        role,
                        volumes: Vec::new(),
                    },
                ));
            }
            b"mesh" => {
                let o = &mut object
                    .as_mut()
                    .ok_or_else(|| invalid("mesh outside object"))?
                    .1;
                if o.mesh.is_some() {
                    return Err(invalid("duplicate mesh"));
                }
                o.mesh = Some(Arc::new(Mesh::default()));
            }
            b"vertex" => {
                let mesh = mesh_mut(&mut object)?;
                mesh.vertices
                    .push([num(&a, "x")?, num(&a, "y")?, num(&a, "z")?]);
            }
            b"triangle" => {
                let mesh = mesh_mut(&mut object)?;
                mesh.triangles
                    .push([id(&a, "v1")?, id(&a, "v2")?, id(&a, "v3")?]);
            }
            b"component" => {
                object
                    .as_mut()
                    .ok_or_else(|| invalid("component outside object"))?
                    .1
                    .components
                    .push(instance(&a, part, production_path)?);
            }
            b"item" if stack.last().map(Vec::as_slice) == Some(b"build") => {
                result.build.push(instance(&a, part, production_path)?);
            }
            _ => {}
        }
        if !empty {
            stack.push(name.to_vec());
        }
    }
    if !root || !stack.is_empty() {
        return Err(invalid("incomplete 3MF model"));
    }
    for object in result.objects.values() {
        if let Some(mesh) = &object.mesh {
            mesh.validate()?;
        }
    }
    for item in &mut result.build {
        item.transform.0[3] = item.transform.0[3].map(|v| v * result.scale_to_mm);
    }
    Ok(result)
}
fn mesh_mut(object: &mut Option<(u32, Object)>) -> Result<&mut Mesh> {
    object
        .as_mut()
        .and_then(|(_, o)| o.mesh.as_mut())
        .and_then(Arc::get_mut)
        .ok_or_else(|| invalid("mesh data outside mesh"))
}
