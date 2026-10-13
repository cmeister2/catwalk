//! Bounded native Fusion design geometry, independent of embedded preview images.

use crate::{Result, scene::Scene};
use cadmpeg_codec_f3d::F3dCodec;
use cadmpeg_core::decode::DecodePolicy;
use cadmpeg_ir::report::{
    Severity,
    loss::{LossCategory, LossTaxonomy},
};
use cadmpeg_ir::{
    CadIr, Codec, DecodeOptions,
    features::{FeatureOperation, NativeFeatureKind},
};
use std::{fs::File, path::Path};

/// Decode the active native Fusion bodies and tessellate their placed geometry.
///
/// # Errors
///
/// Invalid archives, exceeded decode budgets, incomplete geometry or placements,
/// and unsupported exact-shape tessellation fail before a scene is returned.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let mut source = File::open(path)?;
    let options = DecodeOptions {
        policy: DecodePolicy::service(),
        ..DecodeOptions::default()
    };
    let decoded = F3dCodec.decode(&mut source, &options)?;
    for loss in &decoded.report().losses {
        if loss.severity >= Severity::Warning
            && (matches!(
                loss.code.category(),
                LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
            ) || loss.code.taxonomy() == LossTaxonomy::SourceDialectUnverified)
        {
            return Err(format!(
                "Fusion native geometry is incomplete ({}): {}",
                loss.code, loss.message
            )
            .into());
        }
        if loss.severity >= Severity::Error {
            return Err(format!(
                "Fusion native document decode failed ({}): {}",
                loss.code, loss.message
            )
            .into());
        }
    }
    check_placement_features(decoded.ir())?;
    super::native_cad::scene_from_ir(
        decoded.ir(),
        super::native_cad::SurfaceNormalConvention::NativeAsm,
    )
}

/// Native-only assembly operations do not qualify final body placements.
///
/// Their loss is classified as design history by the decoder, but these exact
/// operation families may move already evaluated bodies. Display names and
/// ordinary shape-history omissions do not select this guard.
pub(super) fn check_placement_features(ir: &CadIr) -> Result<()> {
    for feature in &ir.model.features {
        if feature.suppressed == Some(true) {
            continue;
        }
        if let FeatureOperation::Native {
            kind: NativeFeatureKind::Other(kind),
            ..
        } = feature.evaluation.definition().operation()
            && matches!(kind.as_str(), "Assemble" | "GeometricRelationship")
        {
            return Err(format!(
                "Fusion native placement is incomplete: active {kind} operation {} has no neutral placement definition",
                feature.id,
            ).into());
        }
    }
    Ok(())
}
