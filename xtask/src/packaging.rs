// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::{BINARIES, Context, Result, read_json, write_json};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

pub fn hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn notice_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "license",
        "licence",
        "copying",
        "notice",
        "authors",
        "attribution",
    ]
    .iter()
    .any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(['.', '_', '-']))
    })
}
fn notices(source: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let lower = name.to_string_lossy().to_ascii_lowercase();
        let kind = entry.file_type()?;
        if kind.is_file() && notice_name(&lower) {
            fs::copy(entry.path(), output.join(&name))?;
        }
        if kind.is_dir() && ["license", "licenses", "notice", "notices"].contains(&lower.as_str()) {
            copy_tree(&entry.path(), &output.join(name))?;
        }
    }
    let name = source.file_name().unwrap_or_default().to_string_lossy();
    if name.starts_with("epaint_default_fonts-") {
        for entry in fs::read_dir(source.join("fonts"))? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|x| x == "txt") {
                fs::create_dir_all(output.join("fonts"))?;
                fs::copy(entry.path(), output.join("fonts").join(entry.file_name()))?;
            }
        }
    }
    if name.starts_with("zstd-sys-") {
        let native = source.join("zstd");
        let target = output.join("zstd");
        notices(&native, &target)?;
        for file in ["lib/common/xxhash.h", "lib/dictBuilder/divsufsort.c"] {
            let text = fs::read_to_string(native.join(file))?;
            let end = text
                .strip_prefix("/*")
                .and_then(|s| s.find("*/").map(|n| n + 4))
                .ok_or("missing native source notice")?;
            let notice = &text[..end];
            if !notice.contains("Copyright") {
                return Err(format!("missing native copyright in {file}").into());
            }
            fs::write(
                target.join(format!(
                    "{}.notice",
                    Path::new(file)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                )),
                format!("{notice}\n"),
            )?;
        }
    }
    Ok(())
}
pub fn workspace_version(context: &Context) -> Result<String> {
    let metadata: Value = serde_json::from_str(&context.runner.capture(&mut context.cargo(&[
        "metadata",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
    ]))?)?;
    Ok(metadata["packages"]
        .as_array()
        .ok_or("missing workspace packages")?
        .iter()
        .find(|p| p["name"] == "spiling-workbench")
        .and_then(|p| p["version"].as_str())
        .ok_or("missing workbench version")?
        .to_owned())
}
fn catalogs(root: &Path, output: &Path) -> Result<Vec<Value>> {
    let path = root.join("data/material-catalogs");
    if !fs::symlink_metadata(&path)?.file_type().is_dir()
        || !fs::symlink_metadata(path.join("manifest.json"))?
            .file_type()
            .is_file()
    {
        return Err("catalog snapshot/manifest must be ordinary files, not symlinks".into());
    }
    let manifest = read_json(&path.join("manifest.json"))?;
    let mut result = Vec::new();
    let revision = manifest["revision"]
        .as_str()
        .ok_or("missing catalog revision")?;
    let target = output.join(format!("prusa-material-catalogs-{revision}"));
    fs::create_dir_all(&target)?;
    fs::copy(path.join("manifest.json"), target.join("manifest.json"))?;
    for key in ["notices", "catalogs"] {
        for item in manifest[key].as_array().ok_or("missing catalog entries")? {
            let file = item["file"].as_str().ok_or("missing catalog filename")?;
            if Path::new(file).file_name().is_none_or(|n| n != file) {
                return Err("catalog filename must be a basename".into());
            }
            let source = path.join(file);
            if !fs::symlink_metadata(&source)?.file_type().is_file() {
                return Err(format!("catalog/notice must be an ordinary file: {file}").into());
            }
            if hash(&source)? != item["sha256"].as_str().ok_or("missing catalog digest")? {
                return Err(format!("catalog/notice checksum mismatch: {file}").into());
            }
            if key == "notices" {
                fs::copy(&source, target.join(file))?;
            } else {
                if source.metadata()?.len()
                    != item["bytes"].as_u64().ok_or("missing catalog size")?
                {
                    return Err("catalog size mismatch".into());
                }
                result.push(json!({"name":format!("prusa-material-catalog-{}",item["source"].as_str().ok_or("missing catalog source")?),"source":manifest["repository"],"revision":revision,"license":"MIT","path":format!("data/material-catalogs/{file}"),"sha256":item["sha256"],"bytes":item["bytes"],"modifications":"None; exact upstream aggregate bytes. Separate declared reference data, not calibrated evidence or inventory."}));
            }
        }
    }
    Ok(result)
}
pub fn provenance(context: &Context) -> Result<PathBuf> {
    let directory = context.root.join(".cache/release");
    if directory.exists() {
        fs::remove_dir_all(&directory)?;
    }
    fs::create_dir_all(&directory)?;
    let metadata = context.metadata()?;
    let packages = metadata["packages"].as_array().ok_or("missing packages")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("missing resolved dependencies")?;
    let workspace: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .ok_or("missing workspace members")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let nodes: BTreeMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| n["id"].as_str().map(|id| (id, n)))
        .collect();
    let mut closure = BTreeSet::new();
    let mut pending: Vec<&str> = packages
        .iter()
        .filter(|p| BINARIES.iter().any(|name| p["name"] == *name))
        .filter_map(|p| p["id"].as_str())
        .collect();
    while let Some(id) = pending.pop() {
        if !closure.insert(id) {
            continue;
        }
        for dep in nodes.get(id).ok_or("missing dependency node")?["deps"]
            .as_array()
            .ok_or("missing dependency edges")?
        {
            if dep["dep_kinds"]
                .as_array()
                .ok_or("missing dependency kinds")?
                .iter()
                .any(|k| k["kind"] != "dev")
            {
                pending.push(dep["pkg"].as_str().ok_or("missing dependency id")?);
            }
        }
    }
    let mut dependencies = Vec::new();
    for pkg in packages {
        let id = pkg["id"].as_str().ok_or("missing package id")?;
        if !closure.contains(id) || (pkg["source"].is_null() && workspace.contains(id)) {
            continue;
        }
        let name = pkg["name"].as_str().ok_or("missing package name")?;
        let version = pkg["version"].as_str().ok_or("missing version")?;
        let manifest = Path::new(
            pkg["manifest_path"]
                .as_str()
                .ok_or("missing dependency manifest")?,
        );
        notices(
            manifest.parent().ok_or("invalid dependency path")?,
            &directory
                .join("third-party/rust")
                .join(format!("{name}-{version}")),
        )?;
        dependencies.push(
            json!({"name":name,"version":version,"license":pkg["license"],"source":pkg["source"]}),
        );
    }
    let source = directory.join("third-party/source");
    let mut components = catalogs(&context.root, &source)?;
    components.extend([
        json!({"name":"xkcd-color-survey","source":"https://xkcd.com/color/rgb.txt","revision":"sha256:450cca88fa6fa9a1e79c969969e05e6900b41a94f0a3a5f134e3d0b79077f890","license":"CC0-1.0","path":"crates/appearance/data/xkcd-rgb.txt","modifications":"None; 949 original survey samples, names and source license declaration retained."}),
        json!({"name":"prusa-fdm-mixer","source":"https://github.com/prusa3d/prusa-fdm-mixer","revision":"007bb922994cdf985c324cb734f6504e805f6790","license":"MIT","path":"crates/appearance/src/prusa_v7.rs","modifications":"Allocation-free Rust port of the pinned TypeScript v7 reference functions; no measurement dataset imported."})
    ]);
    for (from, to) in [
        (
            "crates/appearance/LICENSE-prusa-fdm-mixer.txt",
            "prusa-fdm-mixer-007bb922/LICENSE-prusa-fdm-mixer.txt",
        ),
        (
            "crates/appearance/LICENSE-xkcd-CC0.txt",
            "xkcd-color-survey-450cca88/LICENSE-CC0.txt",
        ),
        (
            "crates/appearance/data/xkcd-rgb.txt.license",
            "xkcd-color-survey-450cca88/PROVENANCE.txt",
        ),
    ] {
        let target = source.join(to);
        fs::create_dir_all(target.parent().ok_or("missing notice parent")?)?;
        fs::copy(context.root.join(from), target)?;
    }
    copy_tree(
        &context.root.join("apps/workbench/licenses/egui"),
        &source.join("egui-0.37.0"),
    )?;
    for (path, name) in [
        ("solidcraft", "solidcraft-geometry-step"),
        ("cpp_map", "cpp_map-0.2.0"),
        ("tracel-llvm-bundler", "tracel-llvm-bundler"),
        ("boolmesh", "boolmesh"),
    ] {
        let origin = context.root.join("vendor").join(path);
        let target = source.join(name);
        notices(&origin, &target)?;
        fs::copy(origin.join("UPSTREAM.toml"), target.join("UPSTREAM.toml"))?;
    }
    let record = json!({"app":"Spiling","version":workspace_version(context)?,"license":"OSL-3.0","source":"https://github.com/pquerna/spiling","commit":context.capture("git",&["rev-parse","HEAD"])?,"dirty":!context.capture("git",&["status","--porcelain"])?.is_empty(),"platform":env::consts::OS,"arch":env::consts::ARCH,"rust":context.capture("rustc",&["--version"])?,"cargo":context.capture("cargo",&["--version"])?,"lockHashes":{"Cargo.lock":hash(&context.root.join("Cargo.lock"))?},"rustDependencies":dependencies,"sourceComponents":components});
    write_json(&directory.join("provenance.json"), &record)?;
    for name in ["LICENSE.md", "NOTICE.md"] {
        fs::copy(context.root.join(name), directory.join(name))?;
    }
    Ok(directory)
}
pub fn package(context: &Context, args: &[OsString]) -> Result<()> {
    let profile = match args {
        [] => "release",
        [flag, value] if flag == "--profile" && (value == "dev" || value == "release") => {
            value.to_str().ok_or("invalid profile")?
        }
        _ => return Err("usage: cargo xtask package [--profile dev|release]".into()),
    };
    if cfg!(all(target_os = "macos", not(target_arch = "aarch64"))) {
        return Err("macOS packaging requires Apple Silicon".into());
    }
    if !context
        .capture("git", &["status", "--porcelain"])?
        .is_empty()
    {
        return Err("commit the package source first; packaging requires a clean worktree".into());
    }
    context.contracts(true)?;
    let built = context.build(profile == "release", None, &BTreeMap::new())?;
    let notices = provenance(context)?;
    let mut record = read_json(&notices.join("provenance.json"))?;
    record["profile"] = profile.into();
    record["purpose"] = if profile == "dev" {
        "unsigned development validation"
    } else {
        "unsigned package; requires platform/release qualification"
    }
    .into();
    let name = format!(
        "spiling-{}-{}",
        workspace_version(context)?,
        context.host()?
    );
    let output = context
        .target
        .join(if profile == "release" {
            "release"
        } else {
            "debug"
        })
        .join("bundle");
    let stage = output.join(&name);
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir_all(&stage)?;
    let contents = if cfg!(target_os = "macos") {
        stage.join("Spiling.app/Contents")
    } else {
        stage.clone()
    };
    let binaries = if cfg!(target_os = "macos") {
        contents.join("MacOS")
    } else {
        contents.clone()
    };
    fs::create_dir_all(&binaries)?;
    let mut identities = BTreeMap::new();
    for binary in BINARIES {
        let file = format!("{binary}{}", env::consts::EXE_SUFFIX);
        let from = &built[binary];
        let target = binaries.join(&file);
        let source_hash = hash(from)?;
        fs::copy(from, &target)?;
        executable(&target)?;
        if hash(&target)? != source_hash {
            return Err("binary changed during packaging".into());
        }
        identities.insert(binary, source_hash);
    }
    record["binaries"] = serde_json::to_value(identities)?;
    write_json(&notices.join("provenance.json"), &record)?;
    let resources = if cfg!(target_os = "macos") {
        contents.join("Resources")
    } else {
        contents.clone()
    };
    copy_tree(&notices, &resources.join("notices"))?;
    copy_tree(
        &context.root.join("apps/workbench/icons"),
        &resources.join("icons"),
    )?;
    fs::copy(
        context.root.join("docs/quality/SOURCE.md"),
        resources.join("notices/SOURCE.md"),
    )?;
    context.runner.run(
        context
            .command("git")
            .args(["archive", "--format=tar.gz", "--prefix=spiling-source/"])
            .arg(format!(
                "--output={}",
                resources.join("notices/source.tar.gz").display()
            ))
            .arg("HEAD"),
    )?;
    if cfg!(target_os = "macos") {
        for binary in BINARIES {
            crate::macos::audit(&binaries.join(binary))?;
        }
        crate::macos::metadata(&context.root, &contents, &workspace_version(context)?)?;
    }
    // A concurrent source edit cannot silently change the committed package identity.
    if !context
        .capture("git", &["status", "--porcelain"])?
        .is_empty()
        || context.capture("git", &["rev-parse", "HEAD"])? != record["commit"]
    {
        return Err("source changed while packaging; retry from clean committed source".into());
    }
    let archive = output.join(format!("{name}.tar.gz"));
    context.runner.run(
        context
            .command("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&output)
            .arg(&name),
    )?;
    if cfg!(target_os = "linux") {
        debian(context, &output, &stage, &name)?;
    }
    write_json(
        &output.join(format!("{name}.tar.gz.sha256.json")),
        &json!({"archive":archive.file_name(),"sha256":hash(&archive)?,"source":record["commit"],"profile":profile}),
    )?;
    println!("Packaged {}", stage.display());
    Ok(())
}
fn debian(context: &Context, output: &Path, stage: &Path, name: &str) -> Result<()> {
    let deb = output.join(format!("{name}-deb"));
    if deb.exists() {
        fs::remove_dir_all(&deb)?;
    }
    copy_tree(stage, &deb.join("opt/spiling"))?;
    for binary in BINARIES {
        executable(&deb.join("opt/spiling").join(binary))?;
    }
    fs::create_dir_all(deb.join("DEBIAN"))?;
    let arch = context.capture("dpkg", &["--print-architecture"])?;
    let deps = output.join(format!("{name}-dependencies"));
    fs::create_dir_all(deps.join("debian"))?;
    fs::write(
        deps.join("debian/control"),
        "Source: spiling\nSection: graphics\nPriority: optional\nMaintainer: Spiling contributors <spiling@pquerna.com>\n\nPackage: spiling\nArchitecture: any\nDescription: Experimental native workbench\n",
    )?;
    let mut shlibs = context.command("dpkg-shlibdeps");
    shlibs.current_dir(&deps).arg("-O");
    for binary in BINARIES {
        shlibs.arg(format!(
            "-e{}",
            deb.join("opt/spiling").join(binary).display()
        ));
    }
    let linked = context.runner.capture(&mut shlibs)?;
    let mut dependencies: Vec<String> = linked
        .lines()
        .find_map(|l| l.strip_prefix("shlibs:Depends="))
        .ok_or("dpkg-shlibdeps omitted native ABI dependencies")?
        .split(',')
        .map(|s| s.trim().to_owned())
        .collect();
    for library in [
        "libwayland-client0",
        "libx11-6",
        "libxkbcommon0",
        "libvulkan1",
        "xdg-desktop-portal",
    ] {
        if !dependencies
            .iter()
            .any(|d| d.split([' ', '(']).next() == Some(library))
        {
            dependencies.push(library.into());
        }
    }
    fs::write(
        deb.join("DEBIAN/control"),
        format!(
            "Package: spiling\nVersion: {}\nSection: graphics\nPriority: optional\nArchitecture: {arch}\nMaintainer: Spiling contributors <spiling@pquerna.com>\nDepends: {}\nDescription: Experimental native BREP manufacturing workbench\n Rust engine, egui workbench and CLI with source and dependency notices.\n",
            workspace_version(context)?,
            dependencies.join(", ")
        ),
    )?;
    let bin = deb.join("usr/bin");
    fs::create_dir_all(&bin)?;
    #[cfg(unix)]
    {
        for (link, target) in [
            ("spiling", "/opt/spiling/spiling-workbench"),
            ("spiling-cli", "/opt/spiling/spiling-cli"),
        ] {
            std::os::unix::fs::symlink(target, bin.join(link))?;
        }
    }
    let desktop = deb.join("usr/share/applications");
    fs::create_dir_all(&desktop)?;
    let desktop_file = desktop.join("spiling.desktop");
    fs::write(
        &desktop_file,
        "[Desktop Entry]\nType=Application\nName=Spiling\nComment=Experimental BREP manufacturing workbench\nExec=/opt/spiling/spiling-workbench\nIcon=spiling\nCategories=Graphics;Engineering;\nTerminal=false\n",
    )?;
    let icons = deb.join("usr/share/icons/hicolor/512x512/apps");
    fs::create_dir_all(&icons)?;
    fs::copy(
        context.root.join("apps/workbench/icons/icon.png"),
        icons.join("spiling.png"),
    )?;
    context
        .runner
        .run(context.command("desktop-file-validate").arg(desktop_file))?;
    context.runner.run(
        context
            .command("dpkg-deb")
            .args(["--root-owner-group", "--build"])
            .arg(deb)
            .arg(output.join(format!("{name}.deb"))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_zstandard_and_embedded_source_notices_keep_original_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("zstd-sys-test");
        let output = temp.path().join("notices");
        fs::create_dir_all(source.join("zstd/lib/common")).unwrap();
        fs::create_dir_all(source.join("zstd/lib/dictBuilder")).unwrap();
        fs::write(source.join("LICENSE"), "Rust binding\n").unwrap();
        fs::write(source.join("zstd/LICENSE"), "Native license\n").unwrap();
        fs::write(source.join("zstd/COPYING"), "Alternative license\n").unwrap();
        for file in ["lib/common/xxhash.h", "lib/dictBuilder/divsufsort.c"] {
            fs::write(
                source.join("zstd").join(file),
                "/* Copyright original author; upstream license. */\ncode",
            )
            .unwrap();
        }
        notices(&source, &output).unwrap();
        assert_eq!(
            fs::read_to_string(output.join("LICENSE")).unwrap(),
            "Rust binding\n"
        );
        assert_eq!(
            fs::read_to_string(output.join("zstd/LICENSE")).unwrap(),
            "Native license\n"
        );
        assert_eq!(
            fs::read_to_string(output.join("zstd/COPYING")).unwrap(),
            "Alternative license\n"
        );
        for file in ["xxhash.h.notice", "divsufsort.c.notice"] {
            assert_eq!(
                fs::read_to_string(output.join("zstd").join(file)).unwrap(),
                "/* Copyright original author; upstream license. */\n"
            );
        }
        fs::write(
            source.join("zstd/lib/common/xxhash.h"),
            "code without notice",
        )
        .unwrap();
        assert!(notices(&source, &output).is_err());
    }
}
