//! Independent AP242 triangle, representation ownership and assembly qualification.

use catwalk::{
    Result, formats,
    scene::{Scene, Triangle, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use truck_modeling::{Point3, Vector3, builder};
use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

/// Unique suffix for authored STEP files created by concurrent integration tests.
static SERIAL: AtomicUsize = AtomicUsize::new(0);

/// Temporary authored input directory removed when its test finishes.
struct InputDirectory {
    /// Directory containing one authored exchange file.
    path: PathBuf,
}

impl Drop for InputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl InputDirectory {
    /// Write a fresh authored exchange without modifying retained fixtures.
    ///
    /// # Errors
    ///
    /// Creating the directory or writing its model fails.
    fn new(source: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "catwalk-ap242-test-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        let directory = Self { path };
        fs::write(directory.model(), source)?;
        Ok(directory)
    }

    /// Return the input path interpreted by the public format loader.
    fn model(&self) -> PathBuf {
        self.path.join("model.stp")
    }
}

/// Import an authored exchange through the same public path used for thumbnails.
///
/// # Errors
///
/// Temporary input creation or STEP import fails.
fn load_text(source: &str) -> Result<Scene> {
    let directory = InputDirectory::new(source)?;
    formats::load(&directory.model())
}

/// Obtain the concrete public error for an intentionally unsupported exchange.
///
/// # Errors
///
/// Temporary input creation fails or unsupported geometry is unexpectedly accepted.
fn rejection(source: &str) -> Result<String> {
    let directory = InputDirectory::new(source)?;
    match formats::load(&directory.model()) {
        Ok(_) => Err("unsupported authored STEP input was accepted".into()),
        Err(error) => Ok(error.to_string()),
    }
}

/// Resolve the unchanged official NIST model registered in the format corpus.
fn nist_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/scientific_cad/step-nist-ap242-tessellated.stp")
}

/// Independently calculate triangle area from its authored corner coordinates.
fn area(triangle: &Triangle) -> f64 {
    (triangle.vertices[1] - triangle.vertices[0])
        .cross(triangle.vertices[2] - triangle.vertices[0])
        .length()
        / 2.
}

/// Compare finite vectors using a tolerance below the fixture's coordinate precision.
fn close(actual: Vec3, expected: Vec3) -> bool {
    (actual - expected).length() < 1e-9
}

/// Preserve repeated corners while ignoring harmless triangle traversal order.
fn geometry(scene: &Scene) -> Vec<[u64; 3]> {
    let mut positions: Vec<_> = scene
        .positions()
        .map(|point| [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()])
        .collect();
    positions.sort_unstable();
    positions
}

#[test]
fn unchanged_nist_part_preserves_facets_bounds_area_and_selected_ownership() -> Result<()> {
    let path = nist_path();
    let bytes = fs::read(&path)?;
    assert_eq!(bytes.len(), 2_120_559);
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "40cd4e5e2e02b5742e0884ba5b132f2e37d2215c45f57afb48962a05a060c384"
    );
    let scene = formats::load(&path)?;
    // #106 owns #11436. The two construction triangles in #25216 and all
    // associated-data/drafting copies must remain outside the part preview.
    assert_eq!(scene.triangles.len(), 3_388);
    assert_eq!(scene.appearances.len(), 3_388);
    assert!(scene.points.is_empty());
    assert!(scene.lines.is_empty());
    assert_eq!(
        scene
            .triangles
            .iter()
            .filter(|triangle| area(triangle) <= f64::EPSILON)
            .count(),
        20
    );

    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for point in scene.positions() {
        for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    assert_eq!(
        minimum.map(f64::to_bits),
        [-155.797_256_f64, -111.347_252, 0.].map(f64::to_bits)
    );
    assert_eq!(
        maximum.map(f64::to_bits),
        [155.797_256_f64, 111.347_252, 48.259_998].map(f64::to_bits)
    );
    let imported_area: f64 = scene.triangles.iter().map(area).sum();
    // Independent direct expansion gives 191437.91224364479 mm². The
    // producer records 191437.91216 mm² at #11476 and #59306.
    assert!((imported_area - 191_437.912_243_644_8).abs() < 1e-7);
    assert!((imported_area - 191_437.912_16).abs() < 1e-4);
    let source = std::str::from_utf8(&bytes)?;
    assert!(source.contains("AREA_MEASURE(1.9143791216E+05)"));
    assert!(source.contains("#11466=INTEGER_REPRESENTATION_ITEM('number of facets',3388.);"));

    let construction = [
        Vec3::new(50.799_999, -102.457_253, 45.212_002),
        Vec3::new(50.799_999, -102.457_253, 11.684),
        Vec3::new(-50.799_999, -102.457_253, 45.212_002),
        Vec3::new(-50.799_999, -102.457_253, 11.684),
    ];
    assert!(scene.triangles.iter().all(|triangle| {
        !triangle
            .vertices
            .iter()
            .all(|&vertex| construction.iter().any(|&point| close(vertex, point)))
    }));
    Ok(())
}

#[test]
fn nist_face_621_retains_normal_indirection_and_forward_strip_orientation() -> Result<()> {
    let scene = formats::load(&nist_path())?;
    // #621 has pnindex=(895,883,53,55), strip=(1,2,3,4) and normal +Y.
    // These coordinates were independently read from shared list #111.
    let points = [
        Vec3::new(-71.882_004, -57.150_002, 48.259_998),
        Vec3::new(-48.768_002, -57.150_002, 48.259_998),
        Vec3::new(-71.882_004, -57.150_002, 44.831_001),
        Vec3::new(-48.768_002, -57.150_002, 44.831_001),
    ];
    for expected in [
        [points[0], points[1], points[2]],
        [points[2], points[1], points[3]],
    ] {
        let (index, triangle) = scene
            .triangles
            .iter()
            .enumerate()
            .find(|(_, triangle)| {
                expected
                    .iter()
                    .all(|&point| triangle.vertices.iter().any(|&vertex| close(vertex, point)))
            })
            .ok_or("missing independently identified NIST face #621 triangle")?;
        let normals = scene
            .appearances
            .get(&index)
            .and_then(|appearance| appearance.normals)
            .ok_or("NIST face #621 lost its authored normals")?;
        for normal in normals {
            assert!(close(normal, Vec3::new(0., 1., 0.)));
        }
        let normal_dot = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0])
            .dot(Vec3::new(0., 1., 0.));
        assert!((normal_dot - 79.257_843_515_994_03).abs() < 1e-9);
    }
    Ok(())
}

/// Wrap authored AP242 entities without deriving expected geometry from the importer.
fn exchange(entities: &str) -> String {
    format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));ENDSEC;DATA;{entities}ENDSEC;END-ISO-10303-21;"
    )
}

/// One metre-authored triangle mapped three times into a millimetre assembly.
/// Two distinct authored mappings deliberately occupy the same target frame.
fn mapped_triangle() -> String {
    exchange(
        r"
#1=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
#2=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#1)) REPRESENTATION_CONTEXT('',''));
#3=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.));
#4=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#3)) REPRESENTATION_CONTEXT('',''));
#111=COORDINATES_LIST('',3,((1.,2.,3.),(1.002,2.,3.),(1.,2.003,3.)));
#110=TRIANGULATED_FACE('',#111,3,((3.,0.,3.),(0.,0.,2.),(0.,2.,2.)),$,(3,1,2),((2,3,1)));
#120=TESSELLATED_SOLID('',(#110),$);
#200=CARTESIAN_POINT('',(1.,2.,3.));
#201=DIRECTION('',(0.,0.,1.));
#202=DIRECTION('',(1.,0.,0.));
#210=AXIS2_PLACEMENT_3D('',#200,#201,#202);
#100=TESSELLATED_SHAPE_REPRESENTATION('metre part',(#120,#210),#4);
#211=REPRESENTATION_MAP(#210,#100);
#212=CARTESIAN_POINT('',(10.,20.,30.));
#213=DIRECTION('',(0.,1.,0.));
#214=AXIS2_PLACEMENT_3D('',#212,#213,#202);
#220=CARTESIAN_POINT('',(-40.,50.,60.));
#221=DIRECTION('',(0.,0.,-1.));
#222=AXIS2_PLACEMENT_3D('',#220,#221,#202);
#230=MAPPED_ITEM('first',#211,#214);
#231=MAPPED_ITEM('coincident second',#211,#214);
#232=MAPPED_ITEM('third',#211,#222);
#240=SHAPE_REPRESENTATION('millimetre assembly',(#230,#231,#232),#2);
#300=APPLICATION_CONTEXT('mechanical');
#301=PRODUCT_CONTEXT('',#300,'mechanical');
#302=PRODUCT_DEFINITION_CONTEXT('',#300,'design');
#303=PRODUCT('assembly','','',(#301));
#304=PRODUCT_DEFINITION_FORMATION('','',#303);
#305=PRODUCT_DEFINITION('','',#304,#302);
#306=PRODUCT_DEFINITION_SHAPE('','',#305);
#307=SHAPE_DEFINITION_REPRESENTATION(#306,#240);
",
    )
}

#[test]
fn mixed_units_repeated_maps_preserve_per_corner_normals_and_each_occurrence() -> Result<()> {
    let scene = load_text(&mapped_triangle())?;
    assert_eq!(scene.triangles.len(), 3);
    assert_eq!(scene.appearances.len(), 3);
    let first = [
        Vec3::new(10., 20., 30.),
        Vec3::new(12., 20., 30.),
        Vec3::new(10., 20., 27.),
    ];
    let third = [
        Vec3::new(-40., 50., 60.),
        Vec3::new(-38., 50., 60.),
        Vec3::new(-40., 47., 60.),
    ];
    let first_normals = [
        Vec3::new(0., 1., 0.),
        Vec3::new(0., 1., -1.).normalized(),
        Vec3::new(1., 1., 0.).normalized(),
    ];
    let third_normals = [
        Vec3::new(0., 0., -1.),
        Vec3::new(0., -1., -1.).normalized(),
        Vec3::new(1., 0., -1.).normalized(),
    ];
    let mut first_count = 0;
    let mut third_count = 0;
    for (index, triangle) in scene.triangles.iter().enumerate() {
        let is_first = close(triangle.vertices[0], first[0]);
        let (expected, expected_normals) = if is_first {
            first_count += 1;
            (first, first_normals)
        } else {
            third_count += 1;
            (third, third_normals)
        };
        let normals = scene
            .appearances
            .get(&index)
            .and_then(|appearance| appearance.normals)
            .ok_or("mapped AP242 triangle lost per-corner normals")?;
        for corner in 0..3 {
            assert!(close(triangle.vertices[corner], expected[corner]));
            assert!(close(normals[corner], expected_normals[corner]));
        }
        let cross = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0]);
        assert!(cross.dot(normals[0]) > 0.);
    }
    assert_eq!((first_count, third_count), (2, 1));
    assert!((scene.triangles.iter().map(area).sum::<f64>() - 9.).abs() < 1e-9);
    Ok(())
}

/// Add entities before the final DATA terminator of an authored exchange.
///
/// # Errors
///
/// The helper input lacks its DATA terminator.
fn add_entities(source: &str, entities: &str) -> Result<String> {
    let end = source
        .rfind("ENDSEC;")
        .ok_or("authored STEP has no DATA terminator")?;
    Ok(format!("{}{entities}\n{}", &source[..end], &source[end..]))
}

#[test]
fn drafting_owned_mapped_views_are_ignored_but_actual_orphans_are_rejected() -> Result<()> {
    let source = mapped_triangle();
    let original = load_text(&source)?;
    let presentation = add_entities(
        &source,
        "#500=MAPPED_ITEM('drawing view',#211,#222);\n#501=DRAUGHTING_MODEL('drawing view',(#500),#2);",
    )?;
    let viewed = load_text(&presentation)?;
    assert_eq!(geometry(&original), geometry(&viewed));
    assert_eq!(viewed.appearances.len(), 3);
    let orphan = add_entities(&source, "#500=MAPPED_ITEM('orphan',#211,#222);")?;
    assert!(rejection(&orphan)?.contains("mapped item has no owning shape representation"));
    Ok(())
}

#[test]
fn unsupported_required_tessellated_item_does_not_return_a_partial_part() -> Result<()> {
    let source = mapped_triangle().replace("(#120,#210)", "(#120,#210,#500)");
    let source = add_entities(
        &source,
        "#500=TESSELLATED_GEOMETRIC_SET('required group',(#110));",
    )?;
    let error = rejection(&source)?;
    assert!(
        error.contains("unsupported required STEP assembly geometry TESSELLATED_GEOMETRIC_SET")
    );
    Ok(())
}

/// Independently export an offset 2×3×4 mm cuboid using public Truck builders.
fn cuboid() -> String {
    let vertex = builder::vertex(Point3::new(2., 3., 4.));
    let edge = builder::tsweep(&vertex, Vector3::new(2., 0., 0.));
    let face = builder::tsweep(&edge, Vector3::new(0., 3., 0.));
    let solid = builder::tsweep(&face, Vector3::new(0., 0., 4.)).compress();
    CompleteStepDisplay::new(StepModel::from(&solid), StepHeaderDescriptor::default()).to_string()
}

/// Build an independently enumerated twelve-triangle cuboid in an equivalent representation.
///
/// # Errors
///
/// Adding the authored entity records fails.
fn cuboid_tessellation(
    source: &str,
    container: &str,
    target: &str,
    reversed: bool,
) -> Result<String> {
    let relationship = if reversed { "#10,#2003" } else { "#2003,#10" };
    add_entities(
        source,
        &format!(
            r"
#2000=COORDINATES_LIST('',8,((2.,3.,4.),(4.,3.,4.),(4.,6.,4.),(2.,6.,4.),(2.,3.,8.),(4.,3.,8.),(4.,6.,8.),(2.,6.,8.)));
#2001=TRIANGULATED_FACE('',#2000,8,(),$,(),((1,3,2),(1,4,3),(5,6,7),(5,7,8),(1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,4,8),(3,8,7),(4,1,5),(4,5,8)));
#2002={container}('same cuboid',(#2001),{target});
#2003=TESSELLATED_SHAPE_REPRESENTATION('tessellated cuboid',(#2002),#11);
#2004=SHAPE_REPRESENTATION_RELATIONSHIP('','',{relationship});
"
        ),
    )
}

#[test]
fn explicitly_linked_solid_or_shell_preserves_exact_body_without_duplication() -> Result<()> {
    let source = cuboid();
    let original = load_text(&source)?;
    assert!((original.triangles.iter().map(area).sum::<f64>() - 52.).abs() < 1e-7);
    for (container, target) in [("TESSELLATED_SOLID", "#16"), ("TESSELLATED_SHELL", "#17")] {
        for reversed in [false, true] {
            let linked = cuboid_tessellation(&source, container, target, reversed)?;
            let linked = load_text(&linked)?;
            assert_eq!(linked.triangles.len(), original.triangles.len());
            assert_eq!(geometry(&linked), geometry(&original));
        }
    }
    // An unlinked body is independently authored, even when its coordinates coincide.
    let separate = cuboid_tessellation(&source, "TESSELLATED_SOLID", "$", false)?;
    let separate = load_text(&separate)?;
    assert_eq!(separate.triangles.len(), original.triangles.len() + 12);
    assert!((separate.triangles.iter().map(area).sum::<f64>() - 104.).abs() < 1e-7);
    Ok(())
}

#[test]
fn invalid_solid_and_shell_link_targets_are_rejected_before_duplicate_selection() -> Result<()> {
    let source = cuboid();
    // Both target IDs are present in the selected equivalent BREP group, but
    // TESSELLATED_SOLID requires a manifold body and TESSELLATED_SHELL a face set.
    for (container, target) in [("TESSELLATED_SOLID", "#17"), ("TESSELLATED_SHELL", "#16")] {
        let invalid = cuboid_tessellation(&source, container, target, false)?;
        assert!(rejection(&invalid)?.contains("exact-geometry link"));
    }
    Ok(())
}

/// Spell the inherited accuracy representation as a simple or external complex record.
fn accuracy_representation(complex: bool, context: u64) -> String {
    if complex {
        format!(
            "#100=(REPRESENTATION('part',(#120,#210),#{context}) SHAPE_REPRESENTATION() TESSELLATED_SHAPE_REPRESENTATION() TESSELLATED_SHAPE_REPRESENTATION_WITH_ACCURACY_PARAMETERS((CHORDAL_DEVIATION(0.001))));"
        )
    } else {
        format!(
            "#100=TESSELLATED_SHAPE_REPRESENTATION_WITH_ACCURACY_PARAMETERS('part',(#120,#210),#{context},(CHORDAL_DEVIATION(0.001)));"
        )
    }
}

/// Require equivalent public scenes to retain every triangle and authored corner normal.
///
/// # Errors
///
/// A previously present appearance or normal triple is absent from either scene.
fn same_geometry_and_normals(actual: &Scene, expected: &Scene) -> Result<()> {
    assert_eq!(geometry(actual), geometry(expected));
    assert_eq!(actual.appearances.len(), expected.appearances.len());
    for (&index, appearance) in &expected.appearances {
        let expected_normals = appearance.normals.ok_or("reference normals are absent")?;
        let actual_normals = actual
            .appearances
            .get(&index)
            .and_then(|appearance| appearance.normals)
            .ok_or("equivalent representation lost corner normals")?;
        for (actual_normal, expected_normal) in actual_normals.into_iter().zip(expected_normals) {
            assert!(close(actual_normal, expected_normal));
        }
    }
    Ok(())
}

#[test]
fn external_complex_containers_and_accuracy_representations_preserve_public_scenes() -> Result<()> {
    let source = mapped_triangle();
    let original = load_text(&source)?;
    for container in ["TESSELLATED_SHELL", "TESSELLATED_SOLID"] {
        let complex = source.replace(
            "#120=TESSELLATED_SOLID('',(#110),$);",
            &format!(
                "#120=(GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM('') TESSELLATED_ITEM() {container}((#110),$));"
            ),
        );
        same_geometry_and_normals(&load_text(&complex)?, &original)?;
        for complex_representation in [false, true] {
            let accuracy = complex.replace(
                "#100=TESSELLATED_SHAPE_REPRESENTATION('metre part',(#120,#210),#4);",
                &accuracy_representation(complex_representation, 4),
            );
            same_geometry_and_normals(&load_text(&accuracy)?, &original)?;
        }
    }
    // Qualify the same representation layouts without any mapped assembly parent.
    let flat = exchange(
        r"
#1=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
#2=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#1)) REPRESENTATION_CONTEXT('',''));
#111=COORDINATES_LIST('',3,((0.,0.,0.),(2.,0.,0.),(0.,3.,0.)));
#110=TRIANGULATED_FACE('',#111,3,((0.,0.,1.)),$,(),((1,2,3)));
#120=TESSELLATED_SOLID('',(#110),$);
#200=CARTESIAN_POINT('',(0.,0.,0.));
#210=AXIS2_PLACEMENT_3D('',#200,$,$);
#100=TESSELLATED_SHAPE_REPRESENTATION('part',(#120,#210),#2);
",
    );
    let original = load_text(&flat)?;
    assert_eq!(original.triangles.len(), 1);
    for complex_representation in [false, true] {
        let accuracy = flat.replace(
            "#100=TESSELLATED_SHAPE_REPRESENTATION('part',(#120,#210),#2);",
            &accuracy_representation(complex_representation, 2),
        );
        same_geometry_and_normals(&load_text(&accuracy)?, &original)?;
    }
    Ok(())
}
