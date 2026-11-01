// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Bind watched inputs to this checkout even when several worktrees share a target directory.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut prost = tonic_prost_build::Config::new();
    prost.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    // Keep bounded protobuf control oneofs inline rather than adding per-call boxes.
    for oneof in [
        ".spiling.engine.ArtifactSummary.kind",
        ".spiling.engine.ManufacturingRequest.command",
        ".spiling.engine.ManufacturingReply.result",
        ".spiling.engine.AppearancePaletteRow.evidence",
    ] {
        prost.type_attribute(
            oneof,
            "#[expect(clippy::large_enum_variant, reason = \"Bounded control oneofs stay inline without an extra allocation\")]",
        );
    }
    tonic_prost_build::configure().compile_with_config(
        prost,
        &[
            std::path::PathBuf::from("proto/spiling/engine.proto"),
            std::path::PathBuf::from("proto/spiling/native_common.proto"),
            std::path::PathBuf::from("proto/spiling/native_geometry.proto"),
            std::path::PathBuf::from("proto/spiling/native_project.proto"),
            std::path::PathBuf::from("proto/spiling/native_manufacturing.proto"),
            std::path::PathBuf::from("proto/spiling/native_appearance.proto"),
            std::path::PathBuf::from("proto/spiling/native_library.proto"),
            std::path::PathBuf::from("proto/spiling/native_material.proto"),
            std::path::PathBuf::from("proto/vendor/google/longrunning/operations.proto"),
            std::path::PathBuf::from("proto/vendor/google/bytestream/bytestream.proto"),
        ]
        .map(|path| manifest.join(path)),
        &[
            manifest.join("proto/vendor"),
            manifest.join("proto"),
            protoc_bin_vendored::include_path()?,
        ],
    )?;
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("proto").display()
    );
    Ok(())
}
