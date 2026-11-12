// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use quick_xml::{Reader, events::Event};
use std::io::{Cursor, Read};

/// Canonical package URI, never a host filesystem path. Parent traversals may
/// resolve relative package references, but cannot leave the package root.
pub(super) fn resolve(base: &str, target: &str) -> Result<String> {
    if target.contains(['\\', '?', '#', '\0']) || target.contains("://") || target.contains('%') {
        return Err(invalid("unsupported 3MF package URI"));
    }
    let joined = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!(
            "{}/{}",
            base.rsplit_once('/').map_or("", |(dir, _)| dir),
            target
        )
    };
    let mut parts = Vec::new();
    for p in joined.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("3MF URI leaves package"));
                }
            }
            _ => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Err(invalid("empty 3MF part URI"));
    }
    Ok(format!("/{}", parts.join("/")))
}
pub(super) fn decode(bytes: &[u8], mut remaining: usize, cancel: &AtomicBool) -> Result<Package> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| invalid(format!("3MF ZIP: {e}")))?;
    let mut names = BTreeMap::new();
    for index in 0..zip.len() {
        check(cancel)?;
        let file = zip
            .by_index(index)
            .map_err(|e| invalid(format!("3MF ZIP entry: {e}")))?;
        if file.is_dir() {
            continue;
        }
        let canonical = resolve("/", file.name())?;
        if canonical.trim_start_matches('/') != file.name()
            || names
                .insert(canonical.to_ascii_lowercase(), (canonical, index))
                .is_some()
        {
            return Err(invalid("ambiguous or duplicate 3MF ZIP member"));
        }
    }
    let mut read = |name: &str| -> Result<Vec<u8>> {
        check(cancel)?;
        let (_, index) = names
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| invalid(format!("missing 3MF part {name}")))?;
        let mut file = zip
            .by_index(*index)
            .map_err(|e| invalid(format!("3MF ZIP member: {e}")))?;
        let size = usize::try_from(file.size()).map_err(|_| invalid("3MF member size overflow"))?;
        if size > remaining {
            return Err(invalid(
                "3MF decompression exceeds current live-memory admission",
            ));
        }
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| invalid("3MF member allocation failed"))?;
        let mut block = [0u8; 64 * 1024];
        loop {
            check(cancel)?;
            let n = file
                .read(&mut block)
                .map_err(|e| invalid(format!("3MF member/CRC: {e}")))?;
            if n == 0 {
                break;
            }
            if data.len().checked_add(n).is_none_or(|v| v > size) {
                return Err(invalid("3MF member exceeds declared size"));
            }
            data.extend_from_slice(&block[..n]);
        }
        if data.len() != size {
            return Err(invalid("truncated 3MF ZIP member"));
        }
        remaining -= size;
        Ok(data)
    };
    let rels = read("/_rels/.rels")?;
    let mut reader = Reader::from_reader(rels.as_slice());
    let mut root = None;
    loop {
        match reader
            .read_event()
            .map_err(|e| invalid(format!("3MF relationships: {e}")))?
        {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                let a = model::attrs(&e)?;
                if a.get("Type").is_some_and(|v| v.ends_with("/3dmodel")) {
                    if a.get("TargetMode").is_some_and(|v| v == "External") {
                        return Err(invalid("external network 3MF model"));
                    }
                    let target = a
                        .get("Target")
                        .ok_or_else(|| invalid("3MF relationship lacks target"))?;
                    if root.replace(resolve("/", target)?).is_some() {
                        return Err(invalid("multiple root 3MF models"));
                    }
                }
            }
            Event::DocType(_) => return Err(invalid("DTD in 3MF relationships")),
            Event::Eof => break,
            _ => {}
        }
    }
    let root = root.ok_or_else(|| invalid("3MF has no editable root model"))?;
    let mut pending = vec![root.clone()];
    let mut models = BTreeMap::new();
    while let Some(part) = pending.pop() {
        if models.contains_key(&part) {
            continue;
        }
        let data = read(&part)?;
        let model = model::decode(&data, &part, cancel)?;
        for instance in model
            .build
            .iter()
            .chain(model.objects.values().flat_map(|o| o.components.iter()))
        {
            if !models.contains_key(&instance.object.part) {
                pending.push(instance.object.part.clone());
            }
        }
        models.insert(part, model);
    }
    let mut omitted = BTreeSet::new();
    for (name, _) in names.values() {
        if name.starts_with("/Metadata/") {
            omitted.insert(name.clone());
        }
    }
    let mut package = Package {
        root,
        models,
        omitted,
        plates: BTreeMap::new(),
    };
    // Bambu/Orca model_settings records are material roles, not presets.
    if names.contains_key("/metadata/model_settings.config") {
        let data = read("/Metadata/model_settings.config")?;
        roles(&data, &mut package)?;
        plates(&data, &mut package)?;
    }
    if names.contains_key("/metadata/slic3r_pe_model.config") {
        let data = read("/Metadata/Slic3r_PE_model.config")?;
        prusa_roles(&data, &mut package)?;
    }
    Ok(package)
}
fn roles(data: &[u8], package: &mut Package) -> Result<()> {
    let mut reader = Reader::from_reader(data);
    let mut object = None;
    let mut part = None;
    loop {
        match reader
            .read_event()
            .map_err(|e| invalid(format!("3MF model settings: {e}")))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let a = model::attrs(&e)?;
                match e.local_name().as_ref() {
                    b"object" => object = a.get("id").and_then(|v| v.parse::<u32>().ok()),
                    b"part" => {
                        part = a.get("id").and_then(|v| v.parse::<u32>().ok());
                        if let Some(kind) = a.get("subtype") {
                            set_role(package, object, part, kind)?;
                        }
                    }
                    b"metadata"
                        if a.get("key")
                            .is_some_and(|k| matches!(k.as_str(), "part_type" | "volume_type")) =>
                    {
                        let kind = a
                            .get("value")
                            .ok_or_else(|| invalid("3MF role metadata lacks value"))?;
                        set_role(package, object, part, kind)?;
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"object" => object = None,
                b"part" => part = None,
                _ => {}
            },
            Event::DocType(_) => return Err(invalid("DTD in 3MF metadata")),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}
fn set_role(
    package: &mut Package,
    object: Option<u32>,
    part: Option<u32>,
    kind: &str,
) -> Result<()> {
    let role = match kind {
        "normal_part" | "model_part" | "0" => Role::Model,
        "negative_part" | "negative_volume" | "2" => Role::Negative,
        "modifier_part" | "parameter_modifier" | "1" => Role::Modifier,
        "support_blocker" | "3" => Role::SupportBlocker,
        "support_enforcer" | "4" => Role::SupportEnforcer,
        _ => return Err(invalid(format!("unrecognized material role {kind}"))),
    };
    let parent = object.ok_or_else(|| invalid("3MF role has no owning object"))?;
    let root = package.models.get(&package.root).unwrap();
    let starts: Vec<_> = if root.objects.contains_key(&parent) {
        vec![ObjectKey {
            part: package.root.clone(),
            id: parent,
        }]
    } else {
        root.build
            .iter()
            .filter(|i| i.object.id == parent)
            .map(|i| i.object.clone())
            .collect()
    };
    let mut pending = starts;
    let mut seen = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    while let Some(key) = pending.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        let target = package
            .models
            .get(&key.part)
            .and_then(|m| m.objects.get(&key.id))
            .ok_or_else(|| invalid("3MF role has unresolved parent/component"))?;
        if key.id == part.unwrap_or(parent) {
            candidates.insert(key.clone());
        }
        for child in &target.components {
            pending.push(child.object.clone());
        }
    }
    if candidates.len() != 1 {
        return Err(invalid(
            "3MF role reference is missing or ambiguous across model parts",
        ));
    }
    let key = candidates.pop_first().unwrap();
    let target = package
        .models
        .get_mut(&key.part)
        .unwrap()
        .objects
        .get_mut(&key.id)
        .unwrap();
    target.role = role;
    Ok(())
}

/// Prusa stores semantic volumes as inclusive triangle ranges in one mesh.
/// They must be interpreted before the resulting object becomes model material.
fn prusa_roles(data: &[u8], package: &mut Package) -> Result<()> {
    let mut reader = Reader::from_reader(data);
    let mut object = None;
    let mut volume: Option<(usize, usize, Role)> = None;
    let mut excluded = BTreeMap::<u32, Vec<(usize, usize)>>::new();
    loop {
        match reader
            .read_event()
            .map_err(|e| invalid(format!("Prusa 3MF volumes: {e}")))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let a = model::attrs(&e)?;
                match e.local_name().as_ref() {
                    b"object" => {
                        object = Some(
                            a.get("id")
                                .and_then(|v| v.parse::<u32>().ok())
                                .ok_or_else(|| invalid("invalid Prusa object ID"))?,
                        )
                    }
                    b"volume" => {
                        volume = Some((
                            a.get("firstid")
                                .and_then(|v| v.parse().ok())
                                .ok_or_else(|| invalid("invalid Prusa volume range"))?,
                            a.get("lastid")
                                .and_then(|v| v.parse().ok())
                                .ok_or_else(|| invalid("invalid Prusa volume range"))?,
                            Role::Model,
                        ));
                    }
                    b"metadata" if a.get("key").is_some_and(|v| v == "volume_type") => {
                        let role = match a.get("value").map(String::as_str) {
                            Some("ModelPart") => Role::Model,
                            Some("NegativeVolume") => Role::Negative,
                            Some("ParameterModifier") => Role::Modifier,
                            Some("SupportBlocker") => Role::SupportBlocker,
                            Some("SupportEnforcer") => Role::SupportEnforcer,
                            _ => return Err(invalid("unknown Prusa material volume role")),
                        };
                        volume
                            .as_mut()
                            .ok_or_else(|| invalid("Prusa volume role outside volume"))?
                            .2 = role;
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"object" => object = None,
                b"volume" => {
                    let (first, last, role) = volume
                        .take()
                        .ok_or_else(|| invalid("unmatched Prusa volume"))?;
                    if first > last {
                        return Err(invalid("reversed Prusa volume range"));
                    }
                    if role == Role::Negative {
                        return Err(invalid(
                            "Prusa 3MF contains a negative volume; subtractive composition is required",
                        ));
                    }
                    if role != Role::Model {
                        excluded
                            .entry(object.ok_or_else(|| invalid("Prusa volume outside object"))?)
                            .or_default()
                            .push((first, last));
                    }
                }
                _ => {}
            },
            Event::DocType(_) => return Err(invalid("DTD in Prusa 3MF metadata")),
            Event::Eof => break,
            _ => {}
        }
    }
    let model = package.models.get_mut(&package.root).unwrap();
    for (id, ranges) in excluded {
        let object = model
            .objects
            .get_mut(&id)
            .ok_or_else(|| invalid("Prusa volume refers to missing object"))?;
        let original = object
            .mesh
            .as_ref()
            .ok_or_else(|| invalid("Prusa volume refers to missing mesh"))?;
        if ranges
            .iter()
            .any(|&(_, last)| last >= original.triangles.len())
        {
            return Err(invalid("Prusa volume range exceeds mesh"));
        }
        let mut mesh = (**original).clone();
        mesh.triangles = mesh
            .triangles
            .into_iter()
            .enumerate()
            .filter(|(i, _)| {
                !ranges
                    .iter()
                    .any(|&(first, last)| *i >= first && *i <= last)
            })
            .map(|(_, t)| t)
            .collect();
        if mesh.triangles.is_empty() {
            object.role = Role::Modifier;
        } else {
            mesh.validate()?;
            object.mesh = Some(Arc::new(mesh));
        }
    }
    Ok(())
}

fn plates(data: &[u8], package: &mut Package) -> Result<()> {
    let mut reader = Reader::from_reader(data);
    let mut plate = None;
    let mut entries = Vec::new();
    let mut instance = None;
    loop {
        match reader
            .read_event()
            .map_err(|e| invalid(format!("3MF plate metadata: {e}")))?
        {
            Event::Start(e) | Event::Empty(e) => {
                let a = model::attrs(&e)?;
                match e.local_name().as_ref() {
                    b"plate" => {
                        plate = Some(0);
                        entries.clear();
                    }
                    b"model_instance" if plate.is_some() => instance = Some((None, None)),
                    b"metadata" if plate.is_some() => {
                        let key = a.get("key").map(String::as_str).unwrap_or("");
                        if matches!(key, "object_id" | "instance_id" | "plater_id") {
                            let value = a
                                .get("value")
                                .and_then(|v| v.parse::<u32>().ok())
                                .ok_or_else(|| invalid("invalid 3MF plate reference"))?;
                            match key {
                                "plater_id" => plate = Some(value),
                                "object_id" => {
                                    if let Some((object, _)) = &mut instance {
                                        *object = Some(value);
                                    }
                                }
                                "instance_id" => {
                                    if let Some((_, id)) = &mut instance {
                                        *id = Some(value);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"model_instance" if plate.is_some() => {
                    let (object, id) = instance
                        .take()
                        .ok_or_else(|| invalid("unmatched plate instance"))?;
                    entries.push((
                        object.ok_or_else(|| invalid("plate instance lacks object"))?,
                        id.ok_or_else(|| invalid("plate instance lacks index"))?,
                    ));
                }
                b"plate" => {
                    let id = plate
                        .take()
                        .ok_or_else(|| invalid("unmatched source plate"))?;
                    if id == 0
                        || package
                            .plates
                            .insert(id, std::mem::take(&mut entries))
                            .is_some()
                    {
                        return Err(invalid("invalid or duplicate source plate ID"));
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            Event::DocType(_) => return Err(invalid("DTD in plate metadata")),
            _ => {}
        }
    }
    Ok(())
}
