//! STEP surface, mapped placement, assembly ownership and unit-conversion qualification.

use catwalk3d::formats;
use std::{fs, path::PathBuf, process::Command};
use truck_modeling::{Point3, Vector3, builder};
use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

struct FixtureDirectory(PathBuf);

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_f3d_step_aliases_render_a_cad_solid_without_a_display() {
    let directory =
        FixtureDirectory(std::env::temp_dir().join(format!("catwalk-step-{}", std::process::id())));
    fs::create_dir(&directory.0).unwrap();
    let vertex = builder::vertex(Point3::new(0.0, 0.0, 0.0));
    let edge = builder::tsweep(&vertex, Vector3::new(2.0, 0.0, 0.0));
    let face = builder::tsweep(&edge, Vector3::new(0.0, 1.0, 0.0));
    let solid = builder::tsweep(&face, Vector3::new(0.0, 0.0, 3.0)).compress();
    let source = CompleteStepDisplay::new(StepModel::from(&solid), StepHeaderDescriptor::default())
        .to_string();
    let mut reference = None;
    for extension in ["step", "stp", "stpnc", "p21", "210"] {
        let input = directory.0.join(format!("solid.{extension}"));
        let output = directory.0.join(format!("{extension}.png"));
        fs::write(&input, &source).unwrap();
        let scene = formats::load(&input).unwrap();
        assert!(scene.triangles.len() >= 12);
        assert!(scene.points.is_empty());
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .args(["--output"])
            .arg(&output)
            .args(["--size", "64"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{extension}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stderr, [] as [u8; 0]);
        let bytes = fs::read(output).unwrap();
        let thumbnail = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(thumbnail.dimensions(), (64, 64));
        assert!(thumbnail.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
        if let Some(reference) = &reference {
            assert_eq!(&bytes, reference);
        } else {
            reference = Some(bytes);
        }
    }
}

fn cuboid_step() -> String {
    let vertex = builder::vertex(Point3::new(2., 3., 4.));
    let edge = builder::tsweep(&vertex, Vector3::new(2., 0., 0.));
    let face = builder::tsweep(&edge, Vector3::new(0., 3., 0.));
    let solid = builder::tsweep(&face, Vector3::new(0., 0., 4.)).compress();
    CompleteStepDisplay::new(StepModel::from(&solid), StepHeaderDescriptor::default()).to_string()
}

fn add_entities(source: &str, entities: &str) -> String {
    let end = source.rfind("ENDSEC;").unwrap();
    format!("{}{entities}\n{}", &source[..end], &source[end..])
}

/// The original part is offset (2,3,4); the map origin must be subtracted.
/// A reusable subassembly contains two rotated/transposed occurrences, and
/// the top assembly places that same subassembly twice.
fn nested_mapped_step() -> String {
    add_entities(
        &cuboid_step().replace("(#16), #11", "(#16, #1003), #11"),
        r"
#1000=CARTESIAN_POINT('',(2.,3.,4.));
#1001=DIRECTION('',(0.,0.,1.));
#1002=DIRECTION('',(1.,0.,0.));
#1003=AXIS2_PLACEMENT_3D('',#1000,#1001,#1002);
#1004=REPRESENTATION_MAP(#1003,#10);
#1010=CARTESIAN_POINT('',(10.,0.,0.));
#1011=DIRECTION('',(0.,1.,0.));
#1012=AXIS2_PLACEMENT_3D('',#1010,#1001,#1011);
#1013=MAPPED_ITEM('',#1004,#1012);
#1020=CARTESIAN_POINT('',(0.,8.,1.));
#1021=DIRECTION('',(0.,0.,-1.));
#1022=AXIS2_PLACEMENT_3D('',#1020,#1021,#1002);
#1023=MAPPED_ITEM('',#1004,#1022);
#1030=SHAPE_REPRESENTATION('repeated asymmetric parts',(#1013,#1023,#1032),#11);
#1031=CARTESIAN_POINT('',(5.,2.,1.));
#1032=AXIS2_PLACEMENT_3D('',#1031,#1001,#1002);
#1033=REPRESENTATION_MAP(#1032,#1030);
#1040=CARTESIAN_POINT('',(0.,0.,0.));
#1041=AXIS2_PLACEMENT_3D('',#1040,#1001,#1002);
#1042=MAPPED_ITEM('',#1033,#1041);
#1050=CARTESIAN_POINT('',(0.,20.,5.));
#1051=AXIS2_PLACEMENT_3D('',#1050,#1001,#1011);
#1052=MAPPED_ITEM('',#1033,#1051);
#1060=SHAPE_REPRESENTATION('nested repeated assembly',(#1042,#1052),#11);
#1070=SHAPE_DEFINITION_REPRESENTATION(#1071,#1060);
#1071=PRODUCT_DEFINITION_SHAPE('','',#1072);
#1072=PRODUCT_DEFINITION('assembly','',#1073,#9);
#1073=PRODUCT_DEFINITION_FORMATION('','',#1074);
#1074=PRODUCT('nested mapped assembly','','',(#8));
",
    )
}

fn load_text(source: &str, name: &str) -> catwalk3d::scene::Scene {
    static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = FixtureDirectory(std::env::temp_dir().join(format!(
        "catwalk-step-{name}-{}-{serial}",
        std::process::id()
    )));
    fs::create_dir(&directory.0).unwrap();
    let path = directory.0.join("model.step");
    fs::write(&path, source).unwrap();
    formats::load(&path).unwrap()
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Small authored reference coordinates are intentionally rounded to integer micro-units for comparison"
)]
fn transformed_vertices(
    scene: &catwalk3d::scene::Scene,
    transform: impl Fn(catwalk3d::scene::Vec3) -> catwalk3d::scene::Vec3,
) -> Vec<[i64; 3]> {
    let mut points: Vec<_> = scene
        .positions()
        .map(|p| {
            let p = transform(p);
            [p.x, p.y, p.z].map(|v| (v * 1e6).round() as i64)
        })
        .collect();
    points.sort_unstable();
    points
}

#[test]
fn nested_mapped_instances_preserve_each_authored_surface_and_frame() {
    let original = load_text(&cuboid_step(), "original");
    let assembled = load_text(&nested_mapped_step(), "mapped");
    assert_eq!(assembled.triangles.len(), original.triangles.len() * 4);
    let mut expected = Vec::new();
    for outer in 0..2 {
        for inner in 0..2 {
            expected.extend(transformed_vertices(&original, |p| {
                let q = p - catwalk3d::scene::Vec3::new(2., 3., 4.);
                let q = if inner == 0 {
                    catwalk3d::scene::Vec3::new(10. - q.y, q.x, q.z)
                } else {
                    catwalk3d::scene::Vec3::new(q.x, 8. - q.y, 1. - q.z)
                };
                let q = q - catwalk3d::scene::Vec3::new(5., 2., 1.);
                if outer == 0 {
                    q
                } else {
                    catwalk3d::scene::Vec3::new(-q.y, 20. + q.x, 5. + q.z)
                }
            }));
        }
    }
    expected.sort_unstable();
    assert_eq!(transformed_vertices(&assembled, |p| p), expected);
    let area: f64 = assembled
        .triangles
        .iter()
        .map(|t| {
            (t.vertices[1] - t.vertices[0])
                .cross(t.vertices[2] - t.vertices[0])
                .length()
                / 2.
        })
        .sum();
    assert!((area - 208.).abs() < 1e-7);
}

fn occurrence_step(reversed: bool) -> String {
    // Parent assembly has two occurrences of the same product definition.
    let links = if reversed {
        r"#1110=(REPRESENTATION_RELATIONSHIP('','',#1100,#10) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#1111) SHAPE_REPRESENTATION_RELATIONSHIP());
#1111=ITEM_DEFINED_TRANSFORMATION('','',#1012,#1003);"
    } else {
        r"#1110=(REPRESENTATION_RELATIONSHIP('','',#10,#1100) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#1111) SHAPE_REPRESENTATION_RELATIONSHIP());
#1111=ITEM_DEFINED_TRANSFORMATION('','',#1003,#1012);"
    };
    let entities = format!(
        r"
#1000=CARTESIAN_POINT('',(2.,3.,4.));
#1001=DIRECTION('',(0.,0.,1.));
#1002=DIRECTION('',(1.,0.,0.));
#1003=AXIS2_PLACEMENT_3D('',#1000,#1001,#1002);
#1010=CARTESIAN_POINT('',(10.,20.,30.));
#1011=DIRECTION('',(0.,1.,0.));
#1012=AXIS2_PLACEMENT_3D('',#1010,#1001,#1011);
#1020=CARTESIAN_POINT('',(-10.,-20.,-30.));
#1022=AXIS2_PLACEMENT_3D('',#1020,#1001,#1002);
#1100=SHAPE_REPRESENTATION('parent',(#1012,#1022),#11);
#1101=SHAPE_DEFINITION_REPRESENTATION(#1102,#1100);
#1102=PRODUCT_DEFINITION_SHAPE('','',#1103);
#1103=PRODUCT_DEFINITION('assembly','',#1104,#9);
#1104=PRODUCT_DEFINITION_FORMATION('','',#1105);
#1105=PRODUCT('assembly','','',(#8));
{links}
#1112=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#1110,#1113);
#1113=PRODUCT_DEFINITION_SHAPE('','',#1114);
#1114=NEXT_ASSEMBLY_USAGE_OCCURRENCE('first','','',#1103,#5,$);
#1120=(REPRESENTATION_RELATIONSHIP('','',#10,#1100) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#1121) SHAPE_REPRESENTATION_RELATIONSHIP());
#1121=ITEM_DEFINED_TRANSFORMATION('','',#1003,#1022);
#1122=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#1120,#1123);
#1123=PRODUCT_DEFINITION_SHAPE('','',#1124);
#1124=NEXT_ASSEMBLY_USAGE_OCCURRENCE('second','','',#1103,#5,$);
"
    );
    // Explicitly include the local frame in the part's representation, as exporters do.
    add_entities(
        &cuboid_step().replace("(#16), #11", "(#16, #1003), #11"),
        &entities,
    )
}

#[test]
fn assembly_occurrences_and_reversed_relationships_use_product_ownership() {
    let original = load_text(&cuboid_step(), "part");
    let normal = load_text(&occurrence_step(false), "occurrences");
    let reversed = load_text(&occurrence_step(true), "reversed");
    assert_eq!(normal.triangles.len(), original.triangles.len() * 2);
    assert_eq!(
        transformed_vertices(&normal, |p| p),
        transformed_vertices(&reversed, |p| p)
    );
    let mut expected = transformed_vertices(&original, |p| {
        let q = p - catwalk3d::scene::Vec3::new(2., 3., 4.);
        catwalk3d::scene::Vec3::new(10. - q.y, 20. + q.x, 30. + q.z)
    });
    expected.extend(transformed_vertices(&original, |p| {
        p - catwalk3d::scene::Vec3::new(12., 23., 34.)
    }));
    expected.sort_unstable();
    assert_eq!(transformed_vertices(&normal, |p| p), expected);
}

#[test]
fn length_contexts_apply_to_flat_and_mapped_geometry_and_placements() {
    let source = cuboid_step();
    let millimeters = load_text(&source, "millimeters");
    let meters = load_text(
        &source.replace("SI_UNIT(.MILLI.,.METRE.)", "SI_UNIT($,.METRE.)"),
        "meters",
    );
    assert_eq!(
        transformed_vertices(&meters, |p| p),
        transformed_vertices(&millimeters, |p| p * 1000.)
    );
    // Child context is metres; parent's mapping target is millimetres.
    let mixed = add_entities(
        &nested_mapped_step()
            .replace("SI_UNIT(.MILLI.,.METRE.)", "SI_UNIT($,.METRE.)")
            .replace("#11);\n#1031", "#2000);\n#1031")
            .replace("#11);\n#1070", "#2000);\n#1070"),
        r"
#2000=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#2001)) REPRESENTATION_CONTEXT('',''));
#2001=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
",
    );
    let scene = load_text(&mixed, "mixed-units");
    let mut expected = Vec::new();
    for outer in 0..2 {
        for inner in 0..2 {
            expected.extend(transformed_vertices(&millimeters, |p| {
                let q = (p - catwalk3d::scene::Vec3::new(2., 3., 4.)) * 1000.;
                let q = if inner == 0 {
                    catwalk3d::scene::Vec3::new(10. - q.y, q.x, q.z)
                } else {
                    catwalk3d::scene::Vec3::new(q.x, 8. - q.y, 1. - q.z)
                };
                let q = q - catwalk3d::scene::Vec3::new(5., 2., 1.);
                if outer == 0 {
                    q
                } else {
                    catwalk3d::scene::Vec3::new(-q.y, 20. + q.x, 5. + q.z)
                }
            }));
        }
    }
    expected.sort_unstable();
    assert_eq!(transformed_vertices(&scene, |p| p), expected);
}

#[test]
fn incomplete_or_unsupported_assembly_placements_fail_cleanly() {
    static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let directory = FixtureDirectory(std::env::temp_dir().join(format!(
        "catwalk-step-invalid-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    fs::create_dir(&directory.0).unwrap();
    for (index, source) in [
        nested_mapped_step().replace(
            "#1033=REPRESENTATION_MAP(#1032,#1030)",
            "#1033=REPRESENTATION_MAP(#1032,#1060)",
        ),
        nested_mapped_step().replace(
            "#1012=AXIS2_PLACEMENT_3D('',#1010,#1001,#1011)",
            "#1012=AXIS2_PLACEMENT_3D('',#1010,#1001,#1001)",
        ),
        nested_mapped_step().replace(
            "#1004=REPRESENTATION_MAP(#1003,#10)",
            "#1004=REPRESENTATION_MAP(#1003,#999999)",
        ),
        occurrence_step(false).replace(
            "ITEM_DEFINED_TRANSFORMATION('','',#1003,#1012)",
            "CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',#1002,#1011,#1000,1.,#1001)",
        ),
        occurrence_step(false).replace(
            "#1112=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#1110,#1113);",
            "",
        ),
        add_entities(
            &nested_mapped_step().replace("(#1042,#1052)", "(#1042,#1052,#2000)"),
            "#2000=GEOMETRIC_SET('',());",
        ),
    ]
    .iter()
    .enumerate()
    {
        let input = directory.0.join(format!("invalid-{index}.step"));
        let output = directory.0.join(format!("invalid-{index}.png"));
        fs::write(&input, source).unwrap();
        assert!(formats::load(&input).is_err(), "case {index}");
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success(), "case {index}");
        assert!(!output.exists(), "case {index}");
    }
}

#[test]
fn independently_exported_cadquery_bodies_preserve_metres_and_curved_surfaces() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/scientific_cad/step-cadquery-placed-bodies.step");
    let scene = formats::load(&path).unwrap();
    assert!(scene.triangles.len() > 40);
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for p in scene.positions() {
        for (i, value) in [p.x, p.y, p.z].iter().copied().enumerate() {
            minimum[i] = minimum[i].min(value);
            maximum[i] = maximum[i].max(value);
        }
    }
    // Actual cube: x=-10..0, y=0..10, z=0..10 mm. Cylinder:
    // centre=(10,5), radius 5 mm, height 10 mm; circular seam points
    // alone must not determine its extent.
    for (actual, expected) in minimum.into_iter().zip([-10., 0., 0.]) {
        assert!(
            (actual - expected).abs() < 0.1,
            "min actual {actual}, expected {expected}"
        );
    }
    for (actual, expected) in maximum.into_iter().zip([15., 10., 10.]) {
        assert!(
            (actual - expected).abs() < 0.1,
            "max actual {actual}, expected {expected}"
        );
    }
    let area: f64 = scene
        .triangles
        .iter()
        .map(|t| {
            (t.vertices[1] - t.vertices[0])
                .cross(t.vertices[2] - t.vertices[0])
                .length()
                / 2.
        })
        .sum();
    let expected = 600. + 150. * std::f64::consts::PI;
    assert!(
        (area - expected).abs() / expected < 0.015,
        "area {area}, expected {expected}"
    );
}

#[test]
fn simple_and_complex_inherited_conversion_units_have_identical_geometry() {
    let source = cuboid_step().replace(
        "#12 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );",
        "#12 = (CONVERSION_BASED_UNIT('INCH',#3000) LENGTH_UNIT() NAMED_UNIT(#3002));",
    );
    let simple = add_entities(
        &source,
        r"
#3000=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#3001);
#3001=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
#3002=DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);
",
    );
    let complex = simple.replace(
        "#3000=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#3001);",
        "#3000=(LENGTH_MEASURE_WITH_UNIT() MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#3001));",
    );
    let base = load_text(&cuboid_step(), "base-unit");
    let simple = load_text(&simple, "simple-inches");
    let complex = load_text(&complex, "complex-inches");
    let expected = transformed_vertices(&base, |p| p * 25.4);
    assert_eq!(transformed_vertices(&simple, |p| p), expected);
    assert_eq!(transformed_vertices(&complex, |p| p), expected);
}

#[test]
fn nonidentity_mapping_origin_rotation_is_inverted_before_target_placement() {
    let original = load_text(&cuboid_step(), "origin-rotation-base");
    let source = nested_mapped_step().replace(
        "#1003=AXIS2_PLACEMENT_3D('',#1000,#1001,#1002)",
        "#1003=AXIS2_PLACEMENT_3D('',#1000,#1001,#1011)",
    );
    let assembled = load_text(&source, "origin-rotation");
    let mut expected = Vec::new();
    for outer in 0..2 {
        for inner in 0..2 {
            expected.extend(transformed_vertices(&original, |p| {
                let q = p - catwalk3d::scene::Vec3::new(2., 3., 4.);
                let q = catwalk3d::scene::Vec3::new(q.y, -q.x, q.z); // inverse +90° source frame
                let q = if inner == 0 {
                    catwalk3d::scene::Vec3::new(10. - q.y, q.x, q.z)
                } else {
                    catwalk3d::scene::Vec3::new(q.x, 8. - q.y, 1. - q.z)
                };
                let q = q - catwalk3d::scene::Vec3::new(5., 2., 1.);
                if outer == 0 {
                    q
                } else {
                    catwalk3d::scene::Vec3::new(-q.y, 20. + q.x, 5. + q.z)
                }
            }));
        }
    }
    expected.sort_unstable();
    assert_eq!(transformed_vertices(&assembled, |p| p), expected);
}

#[test]
fn checked_in_assembly_cases_match_independent_authored_vertex_expectations() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/scientific_cad");
    for (name, source) in [
        ("step-nested-mapped-assembly.step", nested_mapped_step()),
        (
            "step-repeated-product-assembly.step",
            occurrence_step(false),
        ),
    ] {
        let retained = formats::load(&root.join(name)).unwrap();
        let authored = load_text(&source, name);
        assert_eq!(
            transformed_vertices(&retained, |p| p),
            transformed_vertices(&authored, |p| p),
            "{name}"
        );
    }
}

fn plain_link_step(reversed: bool, mapped: bool) -> String {
    let source = cuboid_step().replace(
        "SHAPE_DEFINITION_REPRESENTATION(#4, #10)",
        "SHAPE_DEFINITION_REPRESENTATION(#4, #4000)",
    );
    let link = if reversed {
        "#4010=SHAPE_REPRESENTATION_RELATIONSHIP('','',#10,#4000);"
    } else {
        "#4010=SHAPE_REPRESENTATION_RELATIONSHIP('','',#4000,#10);"
    };
    let extra = if mapped {
        r"
#4020=REPRESENTATION_MAP(#4003,#4000);
#4021=CARTESIAN_POINT('',(10.,20.,30.));
#4022=DIRECTION('',(0.,1.,0.));
#4023=AXIS2_PLACEMENT_3D('',#4021,$,#4022);
#4024=MAPPED_ITEM('',#4020,#4023);
#4025=SHAPE_REPRESENTATION('parent',(#4024),#11);
#4030=SHAPE_DEFINITION_REPRESENTATION(#4031,#4025);
#4031=PRODUCT_DEFINITION_SHAPE('','',#4032);
#4032=PRODUCT_DEFINITION('parent','',#4033,#9);
#4033=PRODUCT_DEFINITION_FORMATION('','',#4034);
#4034=PRODUCT('parent','','',(#8));
"
    } else {
        ""
    };
    add_entities(
        &source,
        &format!(
            r"
#4000=SHAPE_REPRESENTATION('product-owned axis representation',(#4003),#11);
#4001=CARTESIAN_POINT('',(2.,3.,4.));
#4003=AXIS2_PLACEMENT_3D('',#4001,$,$);
{link}
{extra}
"
        ),
    )
}

#[test]
fn equivalent_geometry_representation_links_accept_both_endpoint_orders() {
    let original = load_text(&cuboid_step(), "plain-base");
    for mapped in [false, true] {
        let first = load_text(&plain_link_step(false, mapped), "plain-owned-first");
        let second = load_text(&plain_link_step(true, mapped), "plain-owned-second");
        assert_eq!(first.triangles.len(), original.triangles.len());
        assert_eq!(
            transformed_vertices(&first, |p| p),
            transformed_vertices(&second, |p| p)
        );
        let expected = transformed_vertices(&original, |p| {
            if mapped {
                let q = p - catwalk3d::scene::Vec3::new(2., 3., 4.);
                catwalk3d::scene::Vec3::new(10. - q.y, 20. + q.x, 30. + q.z)
            } else {
                p
            }
        });
        assert_eq!(transformed_vertices(&first, |p| p), expected);
    }
}
