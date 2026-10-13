//! Native boundary qualification includes incomplete-report and allocation controls.

use super::*;
use ogeom_io::WarningSummary;
use ogeom_math::{Axis, Direction, Frame, Point, Transform, Vector};

/// One bounded numerical warning in the pinned kernel's complete prose grammar.
fn report(distance: f64) -> StepReport {
    StepReport {
        scale_mm: 1.0,
        warnings: vec![format!(
            "face #17: a projected pcurve fit stopped at {distance:.2e}; the face's mesh may sit that far off along this edge"
        )],
        summary: vec![WarningSummary {
            kind: "fit-short",
            count: 1,
            worst: distance,
            exemplar: 17,
        }],
        ..StepReport::default()
    }
}

#[test]
fn numerical_warning_requires_exact_grammar_complete_counts_and_bounded_measurement() {
    assert!(validate_report(&report(0.0001), 0.001).is_ok());
    assert!(validate_report(&report(0.002), 0.001).is_err());
    let mut missing = report(0.0001);
    missing.summary.clear();
    assert!(validate_report(&missing, 0.001).is_err());
    let mut wrong_count = report(0.0001);
    wrong_count.summary[0].count = 2;
    assert!(validate_report(&wrong_count, 0.001).is_err());
    let mut fake = report(0.0001);
    fake.warnings[0] = "face #17: a required edge was dropped; fit stopped at 1e-4".to_owned();
    assert!(validate_report(&fake, 0.001).is_err());
    let mut unknown = report(0.0001);
    unknown.summary[0].kind = "geometry-lost";
    assert!(validate_report(&unknown, 0.001).is_err());
    let mut nonfinite = report(0.0001);
    nonfinite.summary[0].worst = f64::NAN;
    assert!(validate_report(&nonfinite, 0.001).is_err());
}

#[test]
fn skipped_geometry_and_visibility_are_refused_even_with_complete_faces() {
    for name in [
        "GEOMETRIC_CURVE_SET",
        "TRIANGULATED_FACE",
        "INVISIBILITY",
        "MAPPED_ITEM",
    ] {
        let mut value = report(0.0001);
        value.skipped.insert(name.to_owned(), 1);
        assert!(validate_report(&value, 0.001).is_err(), "{name}");
    }
    let mut metadata = report(0.0001);
    metadata.skipped.insert("APPLICATION_CONTEXT".to_owned(), 1);
    assert!(validate_report(&metadata, 0.001).is_ok());
}

#[test]
fn trim_window_event_requires_its_own_bounded_endpoint_projection_budget() {
    let mut value = report(0.0001);
    value.warnings.push("#1084: edge endpoints sit 2.39e-1 and 5.38e-1 from its curve's ends; the window between the vertices' feet on the curve was taken".to_owned());
    value.summary.push(WarningSummary {
        kind: "vertex-window",
        count: 1,
        worst: 0.538_338_117_113_939_9,
        exemplar: 1084,
    });
    assert!(validate_report(&value, 0.002).is_ok());
    assert!(validate_report(&value, 0.0005).is_err());
    value.warnings[1] = "#1084: edge endpoints sit NaN and 5.38e-1 from its curve's ends; the window between the vertices' feet on the curve was taken".to_owned();
    assert!(validate_report(&value, 0.002).is_err());
}

#[test]
fn native_output_and_input_limits_refuse_before_large_allocations() {
    let mut scene = Scene::default();
    let mut bytes = 0;
    assert!(reserve_output(&mut scene, &mut bytes, MAX_TRIANGLES + 1, false).is_err());
    assert_eq!(scene.triangles.capacity(), 0);
    bytes = MAX_SCENE_BYTES;
    assert!(reserve_output(&mut scene, &mut bytes, 1, true).is_err());
    assert_eq!(scene.triangles.capacity(), 0);
    assert!(load_bytes(&vec![b' '; MAX_INPUT_BYTES + 1]).is_err());
}

/// A genuine box topology under a body placement and two different product uses.
fn placed_document() -> Document {
    let mut model = Model::new();
    let box_shape = ogeom_algo::make_box(&mut model, Frame::WORLD, (1., 2., 3.), TOLERANCES)
        .unwrap()
        .shape;
    let body = model.placed(&box_shape, Transform::translation(Vector::new(2., 3., 5.)));
    let mut document = Document::over(model);
    let part = document.add_part("offset body", body);
    document
        .set_product_colour(part, Colour::rgb(0.2, 0.4, 0.6))
        .unwrap();
    let assembly = document.add_assembly("two instances");
    document
        .add_instance(assembly, part, Transform::IDENTITY, None)
        .unwrap();
    let rotation = Transform::rotation(
        Axis::new(Point::ORIGIN, Direction::Z),
        std::f64::consts::FRAC_PI_2,
    );
    let placement = Transform::translation(Vector::new(10., 0., 0.)) * rotation;
    document
        .add_instance(assembly, part, placement, None)
        .unwrap();
    document
}

#[test]
fn product_walk_and_mesh_retain_nested_rotation_translation_and_colour() {
    let document = placed_document();
    let occurrences = placed_parts(&document).unwrap();
    assert_eq!(occurrences.len(), 2);
    let mut scene = Scene::default();
    let mut bytes = 0;
    for occurrence in &occurrences {
        append_occurrence(&mut scene, &mut bytes, &document, occurrence, 6, 0.001).unwrap();
    }
    let minimum = scene.positions().fold(
        Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        |a, b| Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
    );
    let maximum = scene.positions().fold(
        Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        |a, b| Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
    );
    assert!((minimum - Vec3::new(2., 2., 5.)).length() < 1e-9);
    assert!((maximum - Vec3::new(7., 5., 8.)).length() < 1e-9);
    let volume: f64 = scene
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.vertices;
            a.dot(b.cross(c)) / 6.
        })
        .sum();
    assert!((volume - 12.).abs() < 1e-9);
    assert!(
        scene
            .triangles
            .iter()
            .all(|t| t.color == Some([51, 102, 153]))
    );
    assert!(
        scene
            .appearances
            .values()
            .all(|appearance| appearance.normals.is_some())
    );
    scene.validate().unwrap();
}

#[test]
fn deep_or_excessive_product_expansion_is_refused() {
    let mut deep = Document::new();
    let mut child = deep.add_assembly("leaf");
    for depth in 0..=MAX_DEPTH {
        let parent = deep.add_assembly(format!("level {depth}"));
        deep.add_instance(parent, child, Transform::IDENTITY, None)
            .unwrap();
        child = parent;
    }
    let error = placed_parts(&deep).err().unwrap().to_string();
    assert!(error.contains("depth"), "{error}");
    let mut many = placed_document();
    let part = many
        .products()
        .find_map(|(id, product)| matches!(product.kind, ProductKind::Part { .. }).then_some(id))
        .unwrap();
    let assembly = many.add_assembly("excessive repeated body");
    for _ in 0..=MAX_OCCURRENCES {
        many.add_instance(assembly, part, Transform::IDENTITY, None)
            .unwrap();
    }
    let error = placed_parts(&many).err().unwrap().to_string();
    assert!(error.contains("4096"), "{error}");
}
