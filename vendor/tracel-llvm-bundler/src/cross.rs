// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Original Spiling target-LLVM build adapter; upstream files retain their licenses.
use std::path::{Path, PathBuf};

// Spiling: build scripts run on HOST; their native library output belongs to TARGET.
// Cross builds must supply target archives and captured llvm-config data, never
// execute a Darwin llvm-config on Linux or silently link host LLVM archives.
fn darwin_cross(host: &str, target: &str) -> bool {
    target.ends_with("-apple-darwin") && host != target
}

pub(crate) fn cross_darwin() -> bool {
    match (std::env::var("HOST"), std::env::var("TARGET")) {
        (Ok(host), Ok(target)) => darwin_cross(&host, &target),
        _ => false,
    }
}

pub(crate) fn cross_input(name: &str) -> std::io::Result<PathBuf> {
    let path = std::env::var_os(name).map(PathBuf::from).ok_or_else(|| {
        std::io::Error::other(format!(
            "Darwin cross-build requires {name}; see tools/macos/README.md"
        ))
    })?;
    if !path.is_absolute() || !path.is_dir() {
        return Err(std::io::Error::other(format!(
            "{name} must name an existing absolute directory"
        ))
        .into());
    }
    Ok(path)
}

pub(crate) fn captured_query(
    directory: &Path,
    prefix: &Path,
    target: &str,
    version: &str,
    argument: &str,
) -> std::io::Result<String> {
    let read = |name: &str| std::fs::read_to_string(directory.join(format!("{name}.txt")));
    if read("target")?.trim() != target || read("version")?.trim() != version {
        return Err(std::io::Error::other("target LLVM capture target/version mismatch").into());
    }
    let name = argument.strip_prefix("--").unwrap_or("");
    if ![
        "libs",
        "libnames",
        "system-libs",
        "libdir",
        "includedir",
        "version",
        "cflags",
        "cxxflags",
    ]
    .contains(&name)
    {
        return Err(std::io::Error::other("unsupported captured llvm-config query").into());
    }
    Ok(read(name)?
        .trim()
        .replace("${LLVM_PREFIX}", &prefix.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_darwin_cross_builds_override_native_provisioning() {
        assert!(darwin_cross(
            "x86_64-unknown-linux-gnu",
            "aarch64-apple-darwin"
        ));
        assert!(darwin_cross("aarch64-apple-darwin", "x86_64-apple-darwin"));
        assert!(!darwin_cross(
            "aarch64-apple-darwin",
            "aarch64-apple-darwin"
        ));
        assert!(!darwin_cross(
            "x86_64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu"
        ));
        assert!(!darwin_cross(
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu"
        ));
    }

    #[test]
    fn captured_target_queries_are_relocated_and_cannot_use_another_target() {
        let directory =
            std::env::temp_dir().join(format!("spiling-llvm-query-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("target.txt"), "aarch64-apple-darwin\n").unwrap();
        std::fs::write(directory.join("version.txt"), "23.1.0\n").unwrap();
        std::fs::write(directory.join("libdir.txt"), "${LLVM_PREFIX}/lib\n").unwrap();
        let prefix = Path::new("/private/target LLVM");
        assert_eq!(
            captured_query(
                &directory,
                prefix,
                "aarch64-apple-darwin",
                "23.1.0",
                "--libdir"
            )
            .unwrap(),
            "/private/target LLVM/lib"
        );
        assert!(
            captured_query(
                &directory,
                prefix,
                "x86_64-apple-darwin",
                "23.1.0",
                "--libdir"
            )
            .is_err()
        );
        assert!(
            captured_query(
                &directory,
                prefix,
                "aarch64-apple-darwin",
                "23.1.0",
                "--../../other"
            )
            .is_err()
        );
        std::fs::write(directory.join("version.txt"), "22.0.0\n").unwrap();
        assert!(
            captured_query(
                &directory,
                prefix,
                "aarch64-apple-darwin",
                "23.1.0",
                "--libdir"
            )
            .is_err()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
