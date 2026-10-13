//! Genuine native documents qualify projection, loss handling and serialized placements.
//!
//! Repository integration tests own these producer fixtures. Published-crate
//! library tests retain self-contained mathematical controls without CAD assets.

pub use catwalk3d::{Result, scene};
#[path = "../src/formats/fusion.rs"]
mod fusion;
#[path = "../src/formats/native_cad.rs"]
mod native_cad;
#[path = "../src/formats/native_step.rs"]
mod native_step;
#[path = "../src/formats/step_parameters.rs"]
mod step_parameters;

use cadmpeg_codec_f3d::F3dCodec;
use cadmpeg_core::decode::DecodePolicy;
use cadmpeg_ir::{
    CadIr, Codec, DecodeOptions,
    features::{FeatureOperation, NativeFeatureKind},
    geometry::{
        Curve, CurveGeometry, ProceduralCurve, ProceduralCurveDefinition, SolvedCurveGeometry,
    },
    ids::{CurveId, OccurrenceId, ProceduralCurveId, ProductDefinitionId},
    products::{
        Occurrence, OccurrenceParent, ProductDefinition, ProductDefinitionKind, PrototypeReference,
    },
    scalar::FiniteReal,
    topology::{BodyKind, CoedgeUseCurve, ParameterInterval},
    transform::Transform,
};
use native_cad::{SurfaceNormalConvention, preview_geometry};
use scene::Scene;
use std::{collections::BTreeMap, fs::File, path::Path};

/// Real F3D documents use the explicit native ASM carrier normal convention.
fn scene_from_ir(ir: &CadIr) -> Result<Scene> {
    native_cad::scene_from_ir(ir, SurfaceNormalConvention::NativeAsm)
}

/// Inspect the same source-qualified projection that the native adapter renders.
fn projection_bytes(ir: &CadIr) -> Result<Vec<u8>> {
    native_cad::step_projection(ir, SurfaceNormalConvention::NativeAsm)
}

/// Read a real authored body; synthetic controls add only ownership relationships.
fn authored_ir() -> CadIr {
    native_ir("fusion-toy-block.f3d")
}

/// Decode an unmodified source created by the external Fusion author.
fn native_ir(filename: &str) -> CadIr {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/autodesk_geometry")
        .join(filename);
    F3dCodec
        .decode(
            &mut File::open(path).unwrap(),
            &DecodeOptions {
                policy: DecodePolicy::service(),
                ..DecodeOptions::default()
            },
        )
        .unwrap()
        .into_parts()
        .0
}

#[test]
fn auxiliary_cache_supports_are_pruned_but_topology_geometry_is_kept() {
    let mut ir = authored_ir();
    let original = ir.model.curves[0].clone();
    let support = CurveId::mint("test:model:curve#support").unwrap();
    let construction = ProceduralCurveId::mint("test:model:procedure#cached").unwrap();
    ir.model.curves.push(Curve {
        id: support.clone(),
        geometry: CurveGeometry::Solved(SolvedCurveGeometry::Unknown { record: None }),
        parameter_range: None,
        source_object: None,
    });
    ir.model.curves[0].geometry = CurveGeometry::Procedural {
        construction: construction.clone(),
        cache: original.geometry.solved().cloned(),
    };
    ir.model.procedural_curves.push(ProceduralCurve::new(
        construction,
        ProceduralCurveDefinition::Replica {
            source: support.clone(),
            transform: Transform::identity(),
        },
    ));
    let projected = preview_geometry(&ir).unwrap();
    assert!(
        !projected
            .model
            .curves
            .iter()
            .any(|curve| curve.id == support)
    );
    assert!(
        projected
            .model
            .curves
            .iter()
            .any(|curve| curve.id == original.id && curve.geometry == original.geometry)
    );
    let mut expected_bodies = ir.model.bodies.clone();
    for body in &mut expected_bodies {
        if body.kind == BodyKind::Solid {
            body.kind = BodyKind::Sheet;
        }
    }
    assert_eq!(projected.model.bodies, expected_bodies);
    assert_eq!(projected.model.regions, ir.model.regions);
    assert_eq!(projected.model.shells, ir.model.shells);
    assert_eq!(projected.model.faces, ir.model.faces);
    assert_eq!(projected.model.edges, ir.model.edges);
    assert_eq!(projected.model.tessellations, ir.model.tessellations);
    assert_eq!(projected.model.appearances, ir.model.appearances);
    assert_eq!(
        projected.model.appearance_bindings,
        ir.model.appearance_bindings
    );
    assert_eq!(
        projected.model.product_definitions,
        ir.model.product_definitions
    );
    assert_eq!(projected.model.occurrences, ir.model.occurrences);
}

#[test]
fn unsupported_standalone_geometry_still_blocks_the_export() {
    let mut ir = authored_ir();
    ir.model.curves.push(Curve {
        id: CurveId::mint("test:model:curve#independent").unwrap(),
        geometry: CurveGeometry::Solved(SolvedCurveGeometry::Unknown { record: None }),
        parameter_range: None,
        source_object: None,
    });
    let error = scene_from_ir(&ir).unwrap_err().to_string();
    assert!(error.contains("geometry.carrier-not-written"), "{error}");
}

#[test]
fn coedge_local_curve_association_cannot_disappear_as_informational_metadata() {
    let mut ir = authored_ir();
    let shared = ir
        .model
        .edges
        .iter()
        .find(|edge| edge.id == ir.model.coedges[0].edge)
        .unwrap()
        .curve();
    let other = ir
        .model
        .edges
        .iter()
        .find_map(|edge| edge.curve().filter(|curve| Some(*curve) != shared))
        .unwrap()
        .clone();
    ir.model.coedges[0].use_curve = Some(CoedgeUseCurve {
        curve: other,
        parameter_range: ParameterInterval::new([0., 1.]).unwrap(),
    });
    let error = scene_from_ir(&ir).unwrap_err().to_string();
    assert!(
        error.contains("topology.coedge-use-curve-not-represented"),
        "{error}"
    );
}

/// Only the product tree is constructed; every source face remains authored.
fn place_authored_part(mut ir: CadIr) -> CadIr {
    assert_eq!(ir.model.bodies.len(), 1);
    let root = ProductDefinitionId::mint("test:placement:product#assembly").unwrap();
    let part = ProductDefinitionId::mint("test:placement:product#hex-nut").unwrap();
    ir.model.product_definitions = vec![
        product_definition(root.clone(), Vec::new()),
        product_definition(part.clone(), vec![ir.model.bodies[0].id.clone()]),
    ];
    let parent = OccurrenceId::mint("test:placement:occurrence#root").unwrap();
    let child = OccurrenceId::mint("test:placement:occurrence#placed-nut").unwrap();
    ir.model.occurrences = vec![
        product_occurrence(
            parent.clone(),
            root,
            OccurrenceParent::Root {},
            Transform::identity(),
        ),
        product_occurrence(
            child,
            part,
            OccurrenceParent::Occurrence { occurrence: parent },
            Transform::affine([[0., -1., 0., 10.], [1., 0., 0., 20.], [0., 0., 1., 30.]]).unwrap(),
        ),
    ];
    ir.model
        .occurrences
        .sort_by(|left, right| left.id.cmp(&right.id));
    ir
}

/// AP242 has one reusable definition for both parts and assembly containers.
fn product_definition(
    id: ProductDefinitionId,
    bodies: Vec<cadmpeg_ir::ids::BodyId>,
) -> ProductDefinition {
    ProductDefinition {
        id,
        kind: ProductDefinitionKind::Part,
        source_name: None,
        label: None,
        description: None,
        part_number: None,
        bom_properties: BTreeMap::new(),
        bodies,
        native_ref: None,
    }
}

/// Rigid, unit-scale use of a local definition in the canonical occurrence tree.
fn product_occurrence(
    id: OccurrenceId,
    definition: ProductDefinitionId,
    parent: OccurrenceParent,
    transform: Transform,
) -> Occurrence {
    Occurrence {
        id,
        prototype: PrototypeReference::Local { definition },
        parent,
        ordinal: 0,
        transform,
        linked_prototype: None,
        scale: [FiniteReal::new(1.).unwrap(); 3],
        name: None,
        visible: Some(true),
        link: None,
        native_ref: None,
    }
}

/// Physical bounds and oriented volume use triangle coordinates directly.
fn scene_metrics(scene: &Scene) -> ([f64; 3], [f64; 3], f64) {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    let mut volume = 0.;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        volume += a.dot(b.cross(c)) / 6.;
        for point in triangle.vertices {
            for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
                low[axis] = low[axis].min(value);
                high[axis] = high[axis].max(value);
            }
        }
    }
    (low, high, volume)
}

#[test]
fn serialized_native_product_occurrence_preserves_rotation_translation_and_volume() {
    let original = native_ir("fusion-hex-nut.f3d");
    let baseline = scene_from_ir(&original).unwrap();
    let placed_ir = place_authored_part(original.clone());
    assert_eq!(placed_ir.model.bodies, original.model.bodies);
    assert_eq!(placed_ir.model.faces, original.model.faces);
    assert_eq!(placed_ir.model.surfaces, original.model.surfaces);
    let bytes = projection_bytes(&placed_ir).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"));
    assert!(text.contains("ITEM_DEFINED_TRANSFORMATION"));
    let placed = scene_from_ir(&placed_ir).expect("serialized rigid product placement");
    let (low, high, volume) = scene_metrics(&placed);
    // Source side planes specify 20 mm across flats and 10 mm height. An
    // independent quarter turn gives x'=-y+10, y'=x+20 and z'=z+30.
    let half_corner_width = 20. / 3_f64.sqrt();
    for (actual, expected) in low.into_iter().zip([0., 20. - half_corner_width, 30.]) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
    for (actual, expected) in high.into_iter().zip([20., 20. + half_corner_width, 40.]) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
    let (_, _, original_volume) = scene_metrics(&baseline);
    assert!(original_volume > 1000., "authored closed material volume");
    assert!((volume - original_volume).abs() < original_volume * 1e-7);
    assert_eq!(placed.triangles.len(), baseline.triangles.len());
}

#[test]
fn incomplete_serialized_native_occurrence_never_renders_at_identity() {
    let bytes = projection_bytes(&place_authored_part(native_ir("fusion-hex-nut.f3d"))).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let usage = text
        .lines()
        .find(|line| line.contains("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION("))
        .unwrap();
    let without_usage = text.replace(usage, "");
    assert!(
        native_step::load_bytes(without_usage.as_bytes()).is_err(),
        "a missing occurrence placement must refuse rendering"
    );

    let transformation = text
        .lines()
        .find(|line| line.contains("ITEM_DEFINED_TRANSFORMATION("))
        .unwrap();
    let target = transformation
        .rsplit(',')
        .next()
        .unwrap()
        .trim_end_matches(");");
    let frame = text
        .lines()
        .find(|line| {
            line.split_once('=').is_some_and(|(id, record)| {
                id.trim() == target && record.trim_start().starts_with("AXIS2_PLACEMENT_3D(")
            })
        })
        .unwrap();
    let mut fields = frame
        .split('(')
        .nth(1)
        .unwrap()
        .trim_end_matches(");")
        .split(',');
    let name = fields.next().unwrap();
    let point = fields.next().unwrap();
    let axis = fields.next().unwrap();
    let parallel = format!("{target}=AXIS2_PLACEMENT_3D({name},{point},{axis},{axis});");
    let malformed = text.replace(frame, &parallel);
    assert!(
        native_step::load_bytes(malformed.as_bytes()).is_err(),
        "parallel placement frame axes must refuse rendering"
    );
}

#[test]
fn generated_mapped_body_transform_stays_explicitly_unqualified() {
    let mut ir = native_ir("fusion-hex-nut.f3d");
    ir.model.bodies[0].transform =
        Some(Transform::affine([[1., 0., 0., 10.], [0., 1., 0., 20.], [0., 0., 1., 30.]]).unwrap());
    let error = scene_from_ir(&ir).unwrap_err().to_string();
    assert!(error.contains("mapped body placements"), "{error}");
}

#[test]
fn placement_guard_uses_active_source_semantics_and_ignores_display_names() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/autodesk_geometry/fusion-keylock.f3d");
    let error = fusion::load(&path).unwrap_err().to_string();
    assert!(
        error.contains("Fusion native placement is incomplete"),
        "{error}"
    );
    let mut ir = F3dCodec
        .decode(
            &mut File::open(path).unwrap(),
            &DecodeOptions {
                policy: DecodePolicy::service(),
                ..DecodeOptions::default()
            },
        )
        .unwrap()
        .into_parts()
        .0;
    assert!(fusion::check_placement_features(&ir).is_err());
    let mut guarded = 0;
    for feature in &mut ir.model.features {
        if let FeatureOperation::Native {
            kind: NativeFeatureKind::Other(kind),
            ..
        } = feature.evaluation.definition().operation()
            && matches!(kind.as_str(), "Assemble" | "GeometricRelationship")
        {
            feature.suppressed = Some(true);
            guarded += 1;
        }
        feature.name = Some("Assemble GeometricRelationship".to_owned());
    }
    assert_eq!(
        guarded, 2,
        "qualify both genuine native-only operation families"
    );
    fusion::check_placement_features(&ir)
        .expect("suppressed operations and arbitrary display names do not select guard");
}
