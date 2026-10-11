//! Experimental `BRep` geometry bridge. Decoders, STEP conversion, tessellation and rendering run in Rust.
#![forbid(unsafe_code)]

use cadmpeg_codec_f3d::F3dCodec;
use cadmpeg_codec_inventor::InventorCodec;
use cadmpeg_codec_step::StepCodec;
use cadmpeg_core::decode::DecodePolicy;
use cadmpeg_ir::codec::write::{EncodeInput, Encoder, target::TargetRequest};
use cadmpeg_ir::report::{Severity, check::Check, loss::LossCategory};
use cadmpeg_ir::{Codec, DecodeOptions};
use catwalk::scene::Scene;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Cursor, Write},
    path::Path,
};

/// Exact native-decoder source revision recorded in every geometry report.
const UPSTREAM: &str = "9e8f6d6b5da9371bc791ce3924f364aea81be0ec";

/// Decode qualified self-contained part `BRep` through checked STEP and tessellate its surfaces.
///
/// The experimental decoders accept selected `.f3d` and `.ipt` producer variants. Assemblies
/// and geometric, topological or product losses are rejected. The returned report retains
/// decoder findings, neutral-geometry validation and STEP export and round-trip reports.
/// A temporary STEP file bridges the path-based tessellator; no converter process or saved
/// document image participates in this function.
///
/// # Errors
/// Returns an error for unsupported extensions or unresolved assemblies, unreadable inputs,
/// decoding failures, missing `BRep`, relevant loss or validation findings, failed STEP conversion,
/// changed body/shell/face counts, temporary-file I/O failures, or invalid or empty tessellation.
pub fn load_geometry(path: &Path) -> catwalk::Result<(Scene, Value)> {
    let extension = part_extension(path)?;
    let options = DecodeOptions {
        container_only: false,
        policy: DecodePolicy::service(),
    };
    let mut input = File::open(path)?;
    let decoded = match extension.as_str() {
        "f3d" => F3dCodec.decode(&mut input, &options)?,
        "ipt" => InventorCodec.decode(&mut input, &options)?,
        _ => {
            return Err(
                "experimental bridge accepts .f3d and .ipt; .iam is currently unresolved".into(),
            );
        }
    };
    let ir = decoded.ir();
    if ir.model.bodies.is_empty() || ir.model.faces.is_empty() {
        return Err("source contains no completely decoded solid/sheet BRep geometry".into());
    }
    for loss in &decoded.report().losses {
        if loss.severity >= Severity::Error
            || (loss.severity >= Severity::Warning
                && matches!(
                    loss.code.category(),
                    LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
                ))
        {
            return Err(format!("source geometry is incomplete: {}", loss.message).into());
        }
    }
    let check = cadmpeg_ir::validate::validate_neutral(ir, Vec::new())?;
    for finding in &check.findings {
        // Inventor's serialized parameter order does not affect its saved, fully evaluated BRep.
        let unrelated_parameter_order = finding.check == Check::ReferentialIntegrity
            && finding
                .entity
                .as_deref()
                .is_some_and(|id| id.starts_with("inventor:design:parameter#"))
            && finding.message.starts_with("parameter dependency ")
            && finding.message.contains(" does not precede its consumer");
        if finding.severity >= Severity::Error && !unrelated_parameter_order {
            return Err(format!("BRep validation failed: {}", finding.message).into());
        }
    }
    let step = StepCodec::default();
    let plan = step.plan(EncodeInput::new(ir, None), TargetRequest::Inherit)?;
    for loss in &plan.report().losses {
        if matches!(
            loss.code.category(),
            LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
        ) {
            return Err(format!(
                "geometry bridge would omit source geometry: {}",
                loss.message
            )
            .into());
        }
    }
    let mut bytes = Vec::new();
    let export_report = plan.write_to(&mut bytes)?;
    let roundtrip = step.decode(&mut Cursor::new(&bytes), &options)?;
    let back = &roundtrip.ir().model;
    if back.bodies.len() != ir.model.bodies.len()
        || back.faces.len() != ir.model.faces.len()
        || back.shells.len() != ir.model.shells.len()
    {
        return Err("STEP bridge changed the body, shell or face count".into());
    }
    if roundtrip.report().losses.iter().any(|loss| {
        loss.severity >= Severity::Warning
            && matches!(
                loss.code.category(),
                LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
            )
    }) {
        return Err("STEP bridge failed to preserve complete geometry".into());
    }
    let scene = tessellate_step(&bytes)?;
    let report = json!({
        "experimental": true,
        "upstream_commit": UPSTREAM,
        "source_bodies": ir.model.bodies.len(),
        "source_shells": ir.model.shells.len(),
        "source_faces": ir.model.faces.len(),
        "source_edges": ir.model.edges.len(),
        "source_vertices": ir.model.vertices.len(),
        "render_triangles": scene.triangles.len(),
        "render_lines": scene.lines.len(),
        "render_points": scene.points.len(),
        "geometry_sha256": geometry_fingerprint(&scene),
        "decode_report": decoded.report(),
        "validation_report": check,
        "step_export_report": export_report,
        "step_roundtrip_report": roundtrip.report(),
    });
    Ok((scene, report))
}

/// Read a lowercased source suffix and explicitly reject unresolved assemblies.
fn part_extension(path: &Path) -> catwalk::Result<String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "iam" {
        return Err("Inventor assemblies require resolved external prototypes and occurrence placements; this experimental bridge supports self-contained .ipt parts only".into());
    }
    Ok(extension)
}

/// Write checked STEP bytes to a temporary path and require a valid surface tessellation.
fn tessellate_step(bytes: &[u8]) -> catwalk::Result<Scene> {
    let mut temporary = tempfile::Builder::new().suffix(".step").tempfile()?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    let scene = catwalk::formats::load(temporary.path())?;
    scene.validate()?;
    if scene.triangles.is_empty() {
        return Err("BRep bridge produced no surface triangles".into());
    }
    Ok(scene)
}

/// Hash triangle vertex coordinates and established face colors in their original order.
fn geometry_fingerprint(scene: &Scene) -> String {
    let mut fingerprint = Sha256::new();
    for triangle in &scene.triangles {
        for vertex in triangle.vertices {
            for value in [vertex.x, vertex.y, vertex.z] {
                fingerprint.update(value.to_le_bytes());
            }
        }
        fingerprint.update(triangle.color.unwrap_or([100, 160, 220]));
    }
    format!("{:x}", fingerprint.finalize())
}
