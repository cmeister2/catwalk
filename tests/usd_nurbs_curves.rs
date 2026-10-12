//! Independent Bernstein geometry and retained-export qualification for USD NURBS.

#[path = "support/digest_hex.rs"]
/// Lowercase checksum formatting for sha2's byte-array digest output.
mod digest_hex;

use catwalk3d::{
    Result, formats,
    scene::{Line, Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// Unique temporary directory suffix for concurrently running public-loader tests.
static SERIAL: AtomicUsize = AtomicUsize::new(0);
/// Independently calculated IEC sRGB bytes for the authored constant colors.
const ORANGE: [u8; 3] = [249, 137, 97];
/// Independently calculated IEC sRGB bytes for the authored green color.
const GREEN: [u8; 3] = [124, 231, 149];
/// Independently calculated IEC sRGB bytes for the authored blue color.
const BLUE: [u8; 3] = [118, 170, 249];
/// Complete quadratic attributes, suitable for independently authored edge cases.
const QUADRATIC: &str = r"
    int[] curveVertexCounts = [3]
    int[] order = [3]
    double[] knots = [0,0,0,1,1,1]
    double2[] ranges = [(0,1)]
    point3f[] points = [(2,0,0),(2,2,0),(0,2,0)]
";

/// Temporary stage storage kept alive throughout a public importer call.
struct InputDirectory {
    /// Directory containing the authored root layer and any local dependencies.
    path: PathBuf,
}

impl Drop for InputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl InputDirectory {
    /// Write an authored stage without touching retained corpus resources.
    ///
    /// # Errors
    ///
    /// Temporary directory creation or stage writing fails.
    fn new(source: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "catwalk-usd-nurbs-test-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path)?;
        let directory = Self { path };
        fs::write(directory.model(), source)?;
        Ok(directory)
    }

    /// Return the public-loader input path.
    fn model(&self) -> PathBuf {
        self.path.join("model.usda")
    }
}

/// Load an authored stage through the same public API as model thumbnails.
///
/// # Errors
///
/// Writing or importing the stage fails.
fn load_text(source: &str) -> Result<Scene> {
    let directory = InputDirectory::new(source)?;
    formats::load(&directory.model())
}

/// Obtain an actual public error instead of accepting incomplete geometry.
///
/// # Errors
///
/// Writing the stage fails or the unsupported input is unexpectedly accepted.
fn rejection(source: &str) -> Result<String> {
    let directory = InputDirectory::new(source)?;
    match formats::load(&directory.model()) {
        Ok(_) => Err("required invalid or unsupported NURBS geometry was accepted".into()),
        Err(error) => Ok(error.to_string()),
    }
}

/// Make one complete Z-up, unit-scale NURBS stage from literal attributes.
fn stage(attributes: &str) -> String {
    format!(
        "#usda 1.0\n(upAxis = \"Z\"; metersPerUnit = 1)\n\
         def NurbsCurves \"Curve\" {{\n{attributes}\n}}\n",
    )
}

/// Locate a retained authored or original exporter fixture.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/usd_nurbs")
        .join(name)
}

/// Compare world positions below any retained source-coordinate detail.
fn close(actual: Vec3, expected: Vec3, tolerance: f64) -> bool {
    (actual - expected).length() <= tolerance
}

/// Append the independently chosen thumbnail sampling of a closed-form curve.
fn strip(
    lines: &mut Vec<Line>,
    evaluate: impl Fn(f64) -> Vec3,
    first: f64,
    last: f64,
    color: Option<[u8; 3]>,
) {
    let mut previous = evaluate(first);
    for step in 1..=24 {
        let parameter = first + (last - first) * f64::from(step) / 24.;
        let position = evaluate(parameter);
        if position != previous {
            lines.push(Line {
                vertices: [previous, position],
                color,
            });
        }
        previous = position;
    }
}

/// Compare every segment and color without depending on stage traversal order.
fn assert_lines(scene: &Scene, expected: &[Line]) {
    assert!(scene.triangles.is_empty());
    assert!(scene.points.is_empty());
    assert_eq!(scene.lines.len(), expected.len());
    let mut unmatched: Vec<_> = (0..scene.lines.len()).collect();
    for line in expected {
        let matching = unmatched.iter().position(|&index| {
            let actual = &scene.lines[index];
            actual.color == line.color
                && ((close(actual.vertices[0], line.vertices[0], 1e-10)
                    && close(actual.vertices[1], line.vertices[1], 1e-10))
                    || (close(actual.vertices[0], line.vertices[1], 1e-10)
                        && close(actual.vertices[1], line.vertices[0], 1e-10)))
        });
        let Some(matching) = matching else {
            panic!(
                "missing independently expected segment: {line:?}; first imported segments: {:?}",
                scene.lines.get(..3)
            );
        };
        unmatched.swap_remove(matching);
    }
    assert_eq!(unmatched, [] as [usize; 0]);
}

/// Evaluate the literal radius-two rational quadratic in Bernstein form.
fn rational_quarter(parameter: f64, middle_weight: f64) -> Vec3 {
    let a = (1. - parameter).powi(2);
    let b = 2. * middle_weight * parameter * (1. - parameter);
    let c = parameter.powi(2);
    Vec3::new(2. * (a + b) / (a + b + c), 2. * (b + c) / (a + b + c), 0.)
}

/// Expected segments for three independently weighted diagnostic curves.
fn rational_lines() -> Vec<Line> {
    let mut lines = Vec::new();
    strip(
        &mut lines,
        |t| rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2),
        0.,
        1.,
        Some(ORANGE),
    );
    strip(
        &mut lines,
        |t| rational_quarter(t, 0.) + Vec3::new(3., 0., 0.),
        0.,
        1.,
        Some(GREEN),
    );
    strip(
        &mut lines,
        |t| rational_quarter(t, -0.25) + Vec3::new(6., 0., 0.),
        0.,
        1.,
        Some(BLUE),
    );
    lines
}

/// Closed-form linear, quadratic and cubic polynomials for the literal topology.
fn topology_lines() -> Vec<Line> {
    let mut lines = Vec::new();
    for (first, last, branch) in [(0.5, 1., 0), (1., 3., 1), (3., 3.5, 2)] {
        strip(
            &mut lines,
            |u| {
                Vec3::new(
                    u,
                    match branch {
                        0 => 2. * u,
                        1 => 2.5 - u / 2.,
                        _ => 2. * u - 5.,
                    },
                    0.,
                )
            },
            first,
            last,
            Some([249, 160, 97]),
        );
    }
    strip(
        &mut lines,
        |u| Vec3::new(5. + 2. * u, 4. * u - 2. * u * u, u - u * u / 2.),
        0.5,
        1.5,
        Some(GREEN),
    );
    strip(
        &mut lines,
        |t| {
            Vec3::new(
                10. + 6. * t,
                12. * t - 30. * t * t + 19. * t * t * t,
                6. * t - 9. * t * t + 3. * t * t * t,
            )
        },
        0.25,
        0.75,
        Some(BLUE),
    );
    strip(
        &mut lines,
        |t| Vec3::new(2. * t, 6. * t * (1. - t), 2.),
        0.,
        1.,
        Some([225, 149, 237]),
    );
    strip(
        &mut lines,
        |t| Vec3::new(5. + 2. * t, -4. * t + 5. * t * t, 2. + 2. * t * (1. - t)),
        0.,
        1.,
        Some([225, 149, 237]),
    );
    lines
}

/// Expected placements expanded by literal Cartesian equations, not transform helpers.
fn placed_lines() -> Vec<Line> {
    let mut lines = Vec::new();
    strip(
        &mut lines,
        |t| {
            let p = rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2);
            Vec3::new(p.x - 5., 3., 0.75 * p.y)
        },
        0.,
        1.,
        Some([124, 170, 249]),
    );
    strip(
        &mut lines,
        |t| {
            let p = rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2);
            Vec3::new(1. - p.y, 2. * p.x - 8., 1.)
        },
        0.,
        1.,
        Some(ORANGE),
    );
    strip(
        &mut lines,
        |t| {
            let p = rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2);
            Vec3::new(1. - p.y / 2., 3. * p.x + 4., 4.)
        },
        0.,
        1.,
        Some(GREEN),
    );
    lines
}

/// Expected shared stage, preserving every authored diagnostic and placement.
fn family_lines() -> Vec<Line> {
    let mut lines = rational_lines();
    for (group, translation) in [
        (topology_lines(), Vec3::new(0., 7., 0.)),
        (placed_lines(), Vec3::new(0., 17., 0.)),
    ] {
        lines.extend(group.into_iter().map(|mut line| {
            line.vertices = line.vertices.map(|point| point + translation);
            line
        }));
    }
    lines
}

#[test]
fn rational_export_controls_preserve_every_sample_and_distinct_weight_semantics() -> Result<()> {
    let scene = formats::load(&fixture("usda-nurbs-rational-weights.usda"))?;
    let expected = rational_lines();
    assert_eq!(expected.len(), 72);
    assert_lines(&scene, &expected);
    for line in scene.lines.iter().filter(|line| line.color == Some(ORANGE)) {
        for p in line.vertices {
            assert!((p.x.hypot(p.y) - 2.).abs() < 1e-10);
        }
    }
    for (position, color) in [
        (
            Vec3::new(std::f64::consts::SQRT_2, std::f64::consts::SQRT_2, 0.),
            ORANGE,
        ),
        (Vec3::new(4., 1., 0.), GREEN),
        (Vec3::new(6. + 2. / 3., 2. / 3., 0.), BLUE),
    ] {
        assert!(scene.lines.iter().any(|line| {
            line.color == Some(color)
                && line
                    .vertices
                    .iter()
                    .any(|&point| close(point, position, 1e-10))
        }));
    }
    Ok(())
}

#[test]
fn mixed_orders_trim_each_knot_span_and_never_connect_discontinuous_pieces() -> Result<()> {
    let scene = formats::load(&fixture("usda-nurbs-trimmed-discontinuous.usda"))?;
    let expected = topology_lines();
    assert_eq!(expected.len(), 168);
    assert_lines(&scene, &expected);
    let linear_length: f64 = scene
        .lines
        .iter()
        .filter(|line| line.color == Some([249, 160, 97]))
        .map(|line| (line.vertices[1] - line.vertices[0]).length())
        .sum();
    assert!((linear_length - 2. * 5_f64.sqrt()).abs() < 1e-10);
    for line in scene
        .lines
        .iter()
        .filter(|line| line.color == Some([225, 149, 237]))
    {
        assert!(
            line.vertices.iter().all(|point| point.x <= 2.)
                || line.vertices.iter().all(|point| point.x >= 5.)
        );
    }
    Ok(())
}

#[test]
fn curve_prototypes_parent_frames_and_per_instance_colors_have_literal_placements() -> Result<()> {
    let scene = formats::load(&fixture("usda-nurbs-placed-instances.usda"))?;
    let expected = placed_lines();
    assert_eq!(expected.len(), 72);
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn four_retained_encodings_preserve_the_full_independent_geometry_family() -> Result<()> {
    let binary = fs::read(fixture("usdc-nurbs-family.usdc"))?;
    assert!(binary.starts_with(b"PXR-USDC"));
    assert_eq!(binary, fs::read(fixture("usd-nurbs-family.usd"))?);
    let mut package =
        zip::ZipArchive::new(Cursor::new(fs::read(fixture("usdz-nurbs-family.usdz"))?))?;
    assert_eq!(package.len(), 1);
    let mut root = package.by_name("root.usdc")?;
    assert_eq!(root.compression(), zip::CompressionMethod::Stored);
    assert_eq!(
        root.data_start()
            .ok_or("USDZ member has no payload offset")?
            % 64,
        0
    );
    let mut packaged = Vec::new();
    root.read_to_end(&mut packaged)?;
    assert_eq!(packaged, binary);
    let expected = family_lines();
    assert_eq!(expected.len(), 312);
    for filename in [
        "usda-nurbs-family.usda",
        "usdc-nurbs-family.usdc",
        "usd-nurbs-family.usd",
        "usdz-nurbs-family.usdz",
    ] {
        assert_lines(&formats::load(&fixture(filename))?, &expected);
    }
    Ok(())
}

/// Independently evaluate one original uniform cubic span using Bernstein control points.
#[allow(
    clippy::excessive_precision,
    reason = "Retain the producer's decimal spellings before their declared float32 conversion."
)]
fn maya_span(span: usize, parameter: f64) -> Vec3 {
    let controls: [[f32; 3]; 11] = [
        [0.783_611_6, 0.783_611_6, 0.],
        [6.785_732_5e-17, 1.108_194_2, 0.],
        [-0.783_611_6, 0.783_611_6, 0.],
        [-1.108_194_2, 5.744_898_4e-17, 0.],
        [-0.783_611_6, -0.783_611_6, 0.],
        [-1.110_085_7e-16, -1.108_194_2, 0.],
        [0.783_611_6, -0.783_611_6, 0.],
        [1.108_194_2, -1.511_240_5e-16, 0.],
        [0.783_611_6, 0.783_611_6, 0.],
        [6.785_732_5e-17, 1.108_194_2, 0.],
        [-0.783_611_6, 0.783_611_6, 0.],
    ];
    let [previous, left, right, following] =
        std::array::from_fn(|i| Vec3::from(controls[span + i]));
    let first = (previous + left * 4. + right) / 6.;
    let second = (left * 2. + right) / 3.;
    let third = (left + right * 2.) / 3.;
    let fourth = (left + right * 4. + following) / 6.;
    let complement = 1. - parameter;
    first * complement.powi(3)
        + second * (3. * parameter * complement * complement)
        + third * (3. * parameter * parameter * complement)
        + fourth * parameter.powi(3)
}

#[test]
fn unchanged_maya_export_preserves_periodic_uniform_cubic_traces_and_placement() -> Result<()> {
    for (filename, length, hash) in [
        (
            "usda-maya-nurbs-circles.usda",
            1638,
            "17b9eab67f027d314e4de32cd01d1c383068768d8b9a49c7a72ea5abc0b6057b",
        ),
        (
            "MAYA-USD-LICENSE.md",
            10850,
            "8173d5c29b4f956d532781d2b86e4e30f83e6b7878dce18c919451d6ba707c90",
        ),
    ] {
        let bytes = fs::read(fixture(filename))?;
        assert_eq!(bytes.len(), length);
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
            hash
        );
    }
    let notice = fs::read_to_string(fixture("MAYA-USD-NOTICE.txt"))?;
    assert!(notice.contains("48eae123cabec8e671f0779b03c22fa30631a781"));
    assert!(notice.contains("nurbsCircleExport.usda"));
    assert!(notice.contains("Apache"));
    let mut expected = Vec::new();
    for height in [0., 3.] {
        for span in 0..8 {
            strip(
                &mut expected,
                |t| maya_span(span, t) + Vec3::new(0., 0., height),
                0.,
                1.,
                None,
            );
        }
    }
    assert_eq!(expected.len(), 384);
    let scene = formats::load(&fixture("usda-maya-nurbs-circles.usda"))?;
    assert_lines(&scene, &expected);
    let (mut minimum, mut maximum) = (
        Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for line in &scene.lines {
        for p in line.vertices {
            minimum.x = minimum.x.min(p.x);
            minimum.y = minimum.y.min(p.y);
            minimum.z = minimum.z.min(p.z);
            maximum.x = maximum.x.max(p.x);
            maximum.y = maximum.y.max(p.y);
            maximum.z = maximum.z.max(p.z);
        }
    }
    assert!(close(
        minimum,
        Vec3::new(-1.000_000_019_868_215, -1.000_000_019_868_215, 0.),
        1e-10
    ));
    assert!(close(
        maximum,
        Vec3::new(1.000_000_019_868_215, 1.000_000_019_868_215, 3.),
        1e-10
    ));
    Ok(())
}

#[test]
fn common_negative_weight_scaling_does_not_change_a_finite_rational_trace() -> Result<()> {
    let source = stage(&format!(
        "{QUADRATIC}\ndouble[] pointWeights = [-1,-0.7071067811865476,-1]"
    ));
    let scene = load_text(&source)?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |t| rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2),
        0.,
        1.,
        None,
    );
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn zero_endpoint_weights_use_removable_limits_instead_of_unweighted_control_points() -> Result<()> {
    for (weights, at_start) in [("[0,1,1]", true), ("[1,1,0]", false)] {
        let scene = load_text(&stage(&format!(
            "{QUADRATIC}\ndouble[] pointWeights = {weights}"
        )))?;
        let mut expected = Vec::new();
        strip(
            &mut expected,
            |t| {
                if at_start {
                    Vec3::new(4. * (1. - t) / (2. - t), 2., 0.)
                } else {
                    Vec3::new(2., 4. * t / (1. + t), 0.)
                }
            },
            0.,
            1.,
            None,
        );
        assert_lines(&scene, &expected);
    }
    Ok(())
}

#[test]
fn order_one_emits_discrete_constant_span_points_without_connecting_jumps() -> Result<()> {
    let scene = load_text(&stage(
        r"
        int[] curveVertexCounts = [5]
        int[] order = [1]
        double[] knots = [0,0,1,2,3,3]
        double2[] ranges = [(0.5,2.5)]
        point3f[] points = [(99,99,99),(0,0,0),(10,0,0),(20,0,0),(99,99,99)]
        double[] pointWeights = [0,-2,3,-4,0]
        color3f[] primvars:displayColor = [(1,0,0)]
    ",
    ))?;
    assert!(scene.lines.is_empty() && scene.triangles.is_empty());
    assert_eq!(scene.points.len(), 3);
    for expected in [
        Vec3::new(0., 0., 0.),
        Vec3::new(10., 0., 0.),
        Vec3::new(20., 0., 0.),
    ] {
        assert_eq!(
            scene
                .points
                .iter()
                .filter(|point| close(point.position, expected, 1e-12))
                .count(),
            1
        );
    }
    assert!(
        scene
            .points
            .iter()
            .all(|point| point.color == Some([255, 0, 0]))
    );
    Ok(())
}

#[test]
fn batched_order_one_and_quadratic_curves_keep_separate_offsets_and_uniform_colors() -> Result<()> {
    let mut scene = load_text(&stage(
        r#"
        int[] curveVertexCounts = [5,3]
        int[] order = [1,3]
        double[] knots = [0,0,1,2,3,3, 0,0,0,1,1,1]
        double2[] ranges = [(0.5,2.5),(0,1)]
        point3f[] points = [(99,99,99),(0,0,0),(10,0,0),(20,0,0),(99,99,99),
                            (32,0,0),(32,2,0),(30,2,0)]
        double[] pointWeights = [0,-2,3,-4,0, 1,0.7071067811865476,1]
        color3f[] primvars:displayColor = [(1,0,0),(0,0,1)] (
            interpolation = "uniform"
        )
    "#,
    ))?;
    assert_eq!(scene.points.len(), 3);
    for expected in [
        Vec3::new(0., 0., 0.),
        Vec3::new(10., 0., 0.),
        Vec3::new(20., 0., 0.),
    ] {
        assert_eq!(
            scene
                .points
                .iter()
                .filter(|point| close(point.position, expected, 1e-12))
                .count(),
            1,
        );
    }
    assert!(
        scene
            .points
            .iter()
            .all(|point| point.color == Some([255, 0, 0]))
    );
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |t| rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2) + Vec3::new(30., 0., 0.),
        0.,
        1.,
        Some([0, 0, 255]),
    );
    scene.points.clear();
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn a_single_control_point_order_one_curve_has_one_finite_point() -> Result<()> {
    let scene = load_text(&stage(
        r"
        int[] curveVertexCounts = [1]
        int[] order = [1]
        double[] knots = [0,1]
        double2[] ranges = [(0,1)]
        point3f[] points = [(2,3,4)]
    ",
    ))?;
    assert!(scene.lines.is_empty() && scene.triangles.is_empty());
    assert_eq!(scene.points.len(), 1);
    assert!(close(scene.points[0].position, Vec3::new(2., 3., 4.), 0.));
    assert!(scene.points[0].color.is_none());
    Ok(())
}

#[test]
fn earliest_authored_time_world_translation_and_y_up_mapping_are_applied() -> Result<()> {
    let scene = load_text(
        r#"#usda 1.0
        (upAxis = "Y"; startTimeCode = 2; endTimeCode = 9)
        def Xform "Parent" {
            double3 xformOp:translate = (10,20,30)
            uniform token[] xformOpOrder = ["xformOp:translate"]
            color3f[] primvars:displayColor = [(0,1,0)]
            def NurbsCurves "Animated" {
                int[] curveVertexCounts = [3]
                int[] order = [3]
                double[] knots = [0,0,0,1,1,1]
                double2[] ranges = [(0,1)]
                point3f[] points.timeSamples = {
                    2: [(2,0,0),(2,0,2),(0,0,2)],
                    9: [(200,0,0),(200,0,200),(0,0,200)]
                }
                double[] pointWeights = [1,0.7071067811865476,1]
                color3f[] primvars:displayColor.timeSamples = {
                    2: [(1,0,0)], 9: [(0,0,1)]
                }
            }
        }
    "#,
    )?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |t| {
            let p = rational_quarter(t, std::f64::consts::FRAC_1_SQRT_2);
            // Positive source Y remains positive renderer Z under the existing
            // right-handed +90-degree X rotation: (x,y,z) -> (x,-z,y).
            Vec3::new(10. + p.x, -30. - p.y, 20.)
        },
        0.,
        1.,
        Some([255, 0, 0]),
    );
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn a_bound_preview_material_overrides_curve_display_color() -> Result<()> {
    let source = format!(
        r#"#usda 1.0
        (upAxis = "Z")
        def Material "Paint" {{
            token outputs:surface.connect = </Paint/Shader.outputs:surface>
            def Shader "Shader" {{
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor = (0,0,1)
                token outputs:surface
            }}
        }}
        def NurbsCurves "Curve" {{
            {QUADRATIC}
            color3f[] primvars:displayColor = [(1,0,0)]
            rel material:binding = </Paint>
        }}
    "#
    );
    let scene = load_text(&source)?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |t| Vec3::new(2. - 2. * t * t, 4. * t - 2. * t * t, 0.),
        0.,
        1.,
        Some([0, 0, 255]),
    );
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn rational_poles_include_off_grid_even_roots_and_identically_zero_denominators() -> Result<()> {
    for weights in ["[1,-1,1]", "[1,-4,16]", "[0,0,0]"] {
        let error = rejection(&stage(&format!(
            "{QUADRATIC}\ndouble[] pointWeights = {weights}"
        )))?;
        let lower = error.to_ascii_lowercase();
        assert!(
            lower.contains("denominator")
                || lower.contains("pole")
                || lower.contains("singular")
                || lower.contains("weights"),
            "unexpected rational-geometry error: {error}"
        );
    }
    // For [1,-4,16], the quadratic Bernstein denominator is (1-5t)^2.
    // Its interior pole at t=0.2 has no sign change and lies between samples.
    Ok(())
}

#[test]
fn schema_valid_ranges_beyond_the_active_spline_domain_fail_explicitly() -> Result<()> {
    let source = QUADRATIC
        .replace("[0,0,0,1,1,1]", "[0,0,0,1,2,2]")
        .replace("[(0,1)]", "[(0,2)]");
    let error = rejection(&stage(&source))?;
    let lower = error.to_ascii_lowercase();
    assert!(
        lower.contains("unsupported") || lower.contains("not supported"),
        "{error}"
    );
    assert!(
        lower.contains("range") || lower.contains("domain"),
        "{error}"
    );
    Ok(())
}

#[test]
fn malformed_required_curve_arrays_knots_ranges_and_weights_are_rejected() -> Result<()> {
    for (label, old, new) in [
        (
            "nonpositive count",
            "curveVertexCounts = [3]",
            "curveVertexCounts = [0]",
        ),
        ("nonpositive order", "order = [3]", "order = [0]"),
        ("order exceeds count", "order = [3]", "order = [4]"),
        ("missing per-curve order", "order = [3]", "order = []"),
        ("extra per-curve order", "order = [3]", "order = [3,3]"),
        (
            "wrong point count",
            "curveVertexCounts = [3]",
            "curveVertexCounts = [4]",
        ),
        ("too few knots", "[0,0,0,1,1,1]", "[0,0,0,1,1]"),
        ("too many knots", "[0,0,0,1,1,1]", "[0,0,0,1,1,1,1]"),
        ("decreasing knots", "[0,0,0,1,1,1]", "[0,0,1,0,1,1]"),
        ("empty range", "[(0,1)]", "[(0.5,0.5)]"),
        ("reversed range", "[(0,1)]", "[(1,0)]"),
        ("below knot domain", "[(0,1)]", "[(-1,1)]"),
        ("above all knots", "[(0,1)]", "[(0,2)]"),
        ("missing range", "[(0,1)]", "[]"),
        ("extra per-curve range", "[(0,1)]", "[(0,1),(0,1)]"),
    ] {
        let error = rejection(&stage(&QUADRATIC.replace(old, new)))?;
        assert!(!error.is_empty(), "missing public error for {label}");
    }
    for weights in ["[1,1]", "[1,1,1,1]", "[1,nan,1]", "[1,inf,1]"] {
        assert_ne!(
            rejection(&stage(&format!(
                "{QUADRATIC}\ndouble[] pointWeights = {weights}"
            )))?,
            ""
        );
    }
    Ok(())
}

#[test]
fn an_interior_common_factor_preserves_every_finite_rational_sample() -> Result<()> {
    let scene = load_text(&stage(&format!(
        "{}\ndouble[] pointWeights = [-1,-0.5,2]",
        QUADRATIC.replace("[(2,0,0),(2,2,0),(0,2,0)]", "[(0,1,0),(1,2,0),(0.5,1.5,0)]"),
    )))?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |t| Vec3::new(t / (t + 1.), (2. * t + 1.) / (t + 1.), 0.),
        0.,
        1.,
        None,
    );
    assert_lines(&scene, &expected);
    assert!(
        scene
            .lines
            .iter()
            .flat_map(|line| line.vertices)
            .any(|p| close(p, Vec3::new(1. / 3., 4. / 3., 0.), 1e-12))
    );
    Ok(())
}

#[test]
fn a_factor_shared_by_only_one_coordinate_still_has_a_genuine_pole() -> Result<()> {
    let source = QUADRATIC.replace("[(2,0,0),(2,2,0),(0,2,0)]", "[(0,0,0),(1,0,0),(0.5,1,0)]");
    let error = rejection(&stage(&format!(
        "{source}\ndouble[] pointWeights = [-1,-0.5,2]"
    )))?;
    assert!(error.to_ascii_lowercase().contains("pole"), "{error}");
    Ok(())
}

#[test]
fn a_removable_even_root_with_constant_geometry_emits_one_finite_point() -> Result<()> {
    let source = QUADRATIC.replace("[(2,0,0),(2,2,0),(0,2,0)]", "[(2,3,4),(2,3,4),(2,3,4)]");
    let scene = load_text(&stage(&format!(
        "{source}\ndouble[] pointWeights = [1,-1,1]"
    )))?;
    assert!(scene.lines.is_empty() && scene.triangles.is_empty());
    assert_eq!(scene.points.len(), 1);
    assert!(close(scene.points[0].position, Vec3::new(2., 3., 4.), 0.));
    Ok(())
}

#[test]
fn a_strictly_positive_close_minimum_is_not_misclassified_as_a_pole() -> Result<()> {
    let middle = -1. + 2_f64.powi(-40);
    let scene = load_text(&stage(&format!(
        "{QUADRATIC}\ndouble[] pointWeights = [1,{middle:.17},1]"
    )))?;
    assert_eq!(scene.lines.len(), 24);
    let midpoint = Vec3::new(-2_f64.powi(40) + 2., -2_f64.powi(40) + 2., 0.);
    assert!(
        scene
            .lines
            .iter()
            .flat_map(|line| line.vertices)
            .any(|p| close(p, midpoint, 0.))
    );
    assert!(
        scene
            .lines
            .iter()
            .flat_map(|line| line.vertices)
            .all(Vec3::is_finite)
    );
    Ok(())
}

#[test]
fn trimming_away_an_even_pole_accepts_the_complete_remaining_span() -> Result<()> {
    let source = QUADRATIC.replace("[(0,1)]", "[(0.25,1)]");
    let scene = load_text(&stage(&format!(
        "{source}\ndouble[] pointWeights = [1,-4,16]"
    )))?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |t| {
            let a = (1. - t).powi(2);
            let b = -8. * t * (1. - t);
            let c = 16. * t * t;
            Vec3::new(2. * (a + b) / (a + b + c), 2. * (b + c) / (a + b + c), 0.)
        },
        0.25,
        1.,
        None,
    );
    assert_lines(&scene, &expected);
    assert!(close(
        scene.lines[0].vertices[0],
        Vec3::new(-30., -16., 0.),
        0.
    ));
    assert!(close(
        scene.lines[23].vertices[1],
        Vec3::new(0., 2., 0.),
        0.
    ));
    Ok(())
}

#[test]
fn accepted_order_limit_and_rejected_higher_order_are_explicit() -> Result<()> {
    for (order, accepted) in [(64_usize, true), (65, false)] {
        let points = vec!["(0,0,0)"; order].join(",");
        let knots = [vec!["0"; order], vec!["1"; order]].concat().join(",");
        let source = stage(&format!(
            "int[] curveVertexCounts = [{order}]\nint[] order = [{order}]\ndouble[] knots = [{knots}]\ndouble2[] ranges = [(0,1)]\npoint3f[] points = [{points}]"
        ));
        if accepted {
            let scene = load_text(&source)?;
            assert_eq!(scene.points.len(), 1);
            assert!(close(scene.points[0].position, Vec3::default(), 0.));
        } else {
            let error = rejection(&source)?;
            assert!(error.contains("order") && error.contains("64"), "{error}");
        }
    }
    Ok(())
}

#[test]
fn evaluation_work_is_cumulative_across_separate_stage_prims() -> Result<()> {
    let points = vec!["(0,0,0)"; 64 * 13].join(",");
    let curve_knots = [vec!["0"; 64], vec!["1"; 64]].concat().join(",");
    let knots = vec![curve_knots.as_str(); 13].join(",");
    let counts = vec!["64"; 13].join(",");
    let ranges = vec!["(0,1)"; 13].join(",");
    let attributes = format!(
        "int[] curveVertexCounts = [{counts}]\nint[] order = [{counts}]\ndouble[] knots = [{knots}]\ndouble2[] ranges = [{ranges}]\npoint3f[] points = [{points}]"
    );
    let source = format!(
        "#usda 1.0\n(upAxis = \"Z\")\ndef NurbsCurves \"First\" {{\n{attributes}\n}}\ndef NurbsCurves \"Second\" {{\n{attributes}\n}}\n"
    );
    let error = rejection(&source)?;
    assert!(
        error.contains("work") && error.contains("budget"),
        "{error}"
    );
    Ok(())
}

#[test]
fn overflowing_floating_knot_width_uses_exact_trimmed_geometry() -> Result<()> {
    let scene = load_text(&stage(
        r"
        int[] curveVertexCounts = [2]
        int[] order = [2]
        double[] knots = [-1e308,-1e308,1e308,1e308]
        double2[] ranges = [(-1,1)]
        point3f[] points = [(0,0,0),(2,0,0)]
    ",
    ))?;
    assert!(scene.lines.is_empty() && scene.triangles.is_empty());
    assert_eq!(scene.points.len(), 1);
    assert!(close(scene.points[0].position, Vec3::new(1., 0., 0.), 0.));
    Ok(())
}

#[test]
fn a_homogeneous_product_underflow_preserves_the_original_endpoint() -> Result<()> {
    let scene = load_text(&stage(
        r"
        int[] curveVertexCounts = [2]
        int[] order = [2]
        double[] knots = [0,0,1,1]
        double2[] ranges = [(0,1)]
        point3f[] points = [(1e-30,0,0),(2e-30,0,0)]
        double[] pointWeights = [1e-300,1]
    ",
    ))?;
    assert!(!scene.lines.is_empty());
    assert!(close(
        scene.lines[0].vertices[0],
        Vec3::new(f64::from(1e-30_f32), 0., 0.),
        0.
    ));
    assert!(close(
        scene
            .lines
            .last()
            .ok_or("missing finite sampled line")?
            .vertices[1],
        Vec3::new(f64::from(2e-30_f32), 0., 0.),
        0.
    ));
    Ok(())
}

#[test]
fn subnormal_homogeneous_products_preserve_a_scaled_float32_endpoint() -> Result<()> {
    let scene = load_text(&stage(
        r#"
        int[] curveVertexCounts = [2]
        int[] order = [2]
        double[] knots = [0,0,1,1]
        double2[] ranges = [(0,1)]
        point3f[] points = [(1e-30,0,0),(2e-30,0,0)]
        double[] pointWeights = [1e-290,1]
        double3 xformOp:scale = (1e30,1,1)
        uniform token[] xformOpOrder = ["xformOp:scale"]
    "#,
    ))?;
    let expected = f64::from(1e-30_f32) * 1e30;
    assert!(!scene.lines.is_empty());
    assert!(close(
        scene.lines[0].vertices[0],
        Vec3::new(expected, 0., 0.),
        0.
    ));
    assert!((scene.lines[0].vertices[0].x - 1.000_000_003_171_077).abs() < 1e-15);
    Ok(())
}

#[test]
fn normalized_bernstein_samples_survive_one_and_twenty_five_ulp_knot_intervals() -> Result<()> {
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |u| Vec3::new(2. * u, 4. * u * (1. - u), 0.),
        0.,
        1.,
        None,
    );
    for ulps in [1_u64, 25] {
        let end = f64::from_bits(1_f64.to_bits() + ulps);
        let scene = load_text(&stage(&format!(
            r"
            int[] curveVertexCounts = [3]
            int[] order = [3]
            double[] knots = [1,1,1,{end},{end},{end}]
            double2[] ranges = [(1,{end})]
            point3f[] points = [(0,0,0),(1,2,0),(2,0,0)]
        "
        )))?;
        assert_lines(&scene, &expected);
        assert!(
            scene
                .lines
                .iter()
                .flat_map(|line| line.vertices)
                .any(|p| close(p, Vec3::new(1., 1., 0.), 1e-12))
        );
    }
    Ok(())
}

#[test]
fn an_underflowing_parameter_ratio_preserves_the_visible_rational_trace() -> Result<()> {
    let scene = load_text(&stage(
        r#"
        int[] curveVertexCounts = [2]
        int[] order = [2]
        double[] knots = [0,0,1e308,1e308]
        double2[] ranges = [(1e-300,2e-300)]
        point3f[] points = [(0,0,0),(2,0,0)]
        double[] pointWeights = [1e-300,1]
        double3 xformOp:scale = (1e308,1,1)
        uniform token[] xformOpOrder = ["xformOp:scale"]
    "#,
    ))?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |u| Vec3::new(2. + 2. * u, 0., 0.),
        0.,
        1.,
        None,
    );
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn a_lost_parameter_complement_preserves_the_oppositely_directed_trace() -> Result<()> {
    let scene = load_text(&stage(
        r#"
        int[] curveVertexCounts = [2]
        int[] order = [2]
        double[] knots = [-1e308,-1e308,2e-300,2e-300]
        double2[] ranges = [(0,1e-300)]
        point3f[] points = [(2,0,0),(0,0,0)]
        double[] pointWeights = [1,1e-300]
        double3 xformOp:scale = (1e308,1,1)
        uniform token[] xformOpOrder = ["xformOp:scale"]
    "#,
    ))?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |u| Vec3::new(4. - 2. * u, 0., 0.),
        0.,
        1.,
        None,
    );
    assert_lines(&scene, &expected);
    Ok(())
}

#[test]
fn exactly_constant_controls_preserve_one_point_with_positive_or_negative_weights() -> Result<()> {
    let source = QUADRATIC.replace("[(2,0,0),(2,2,0),(0,2,0)]", "[(3,4,5),(3,4,5),(3,4,5)]");
    for weights in ["[1,0.7071067811865476,1]", "[-1,-0.7071067811865476,-1]"] {
        let scene = load_text(&stage(&format!(
            "{source}\ndouble[] pointWeights = {weights}"
        )))?;
        assert!(scene.lines.is_empty() && scene.triangles.is_empty());
        assert_eq!(scene.points.len(), 1);
        assert!(close(scene.points[0].position, Vec3::new(3., 4., 5.), 0.));
    }
    Ok(())
}

#[test]
fn a_small_authored_rational_trace_is_preserved_instead_of_tolerance_collapsing() -> Result<()> {
    let delta = 2_f64.powi(-20);
    let source = QUADRATIC.replace(
        "[(2,0,0),(2,2,0),(0,2,0)]",
        &format!("[(3,4,5),({},4,5),(3,4,5)]", 3. + delta),
    );
    let scene = load_text(&stage(&format!(
        "{source}\ndouble[] pointWeights = [1,0.7071067811865476,1]"
    )))?;
    let mut expected = Vec::new();
    strip(
        &mut expected,
        |u| {
            let middle = 2. * std::f64::consts::FRAC_1_SQRT_2 * u * (1. - u);
            let denominator = (1. - u).powi(2) + middle + u * u;
            Vec3::new(3. + delta * middle / denominator, 4., 5.)
        },
        0.,
        1.,
        None,
    );
    assert_lines(&scene, &expected);
    Ok(())
}
