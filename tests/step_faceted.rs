//! Independent planar faceted STEP geometry, topology and assembly qualification.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    Result, formats,
    scene::{Scene, Triangle, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering as Comparison,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use truck_modeling::{Point3, Vector3, builder};
use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

/// Unique suffix for authored files created by concurrent integration tests.
static SERIAL: AtomicUsize = AtomicUsize::new(0);

/// Temporary exchange directory removed after its public-loader qualification.
struct InputDirectory {
    /// Directory containing one authored STEP file.
    path: PathBuf,
}

impl Drop for InputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl InputDirectory {
    /// Write authored geometry without modifying the retained producer files.
    ///
    /// # Errors
    ///
    /// Creating the temporary directory or writing its exchange fails.
    fn new(source: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "catwalk-faceted-test-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        let directory = Self { path };
        fs::write(directory.model(), source)?;
        Ok(directory)
    }

    /// Return the path consumed by the public format loader.
    fn model(&self) -> PathBuf {
        self.path.join("model.stp")
    }
}

/// Import an authored exchange through the thumbnail engine's public loader.
///
/// # Errors
///
/// Creating the input or loading its required geometry fails.
fn load_text(source: &str) -> Result<Scene> {
    let directory = InputDirectory::new(source)?;
    formats::load(&directory.model())
}

/// Obtain the public error for an unsupported or malformed required body.
///
/// # Errors
///
/// Input creation fails or the invalid geometry is unexpectedly accepted.
fn rejection(source: &str) -> Result<String> {
    let directory = InputDirectory::new(source)?;
    match formats::load(&directory.model()) {
        Ok(_) => Err("invalid authored faceted STEP was accepted".into()),
        Err(error) => Ok(error.to_string()),
    }
}

/// Locate an unchanged scientific CAD qualification input.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/scientific_cad")
        .join(name)
}

/// Independently calculate triangle area from Cartesian corners.
fn area(triangle: &Triangle) -> f64 {
    (triangle.vertices[1] - triangle.vertices[0])
        .cross(triangle.vertices[2] - triangle.vertices[0])
        .length()
        / 2.
}

/// Calculate the oriented volume of a closed triangulated surface.
fn volume(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|triangle| {
            triangle.vertices[0].dot(triangle.vertices[1].cross(triangle.vertices[2])) / 6.
        })
        .sum()
}

/// Compare coordinates below the producer's smallest retained geometric detail.
fn close(actual: Vec3, expected: Vec3) -> bool {
    (actual - expected).length() < 1e-10
}

/// Lexicographically order finite Cartesian coordinates without quantization.
fn point_order(left: &Vec3, right: &Vec3) -> Comparison {
    left.x
        .total_cmp(&right.x)
        .then(left.y.total_cmp(&right.y))
        .then(left.z.total_cmp(&right.z))
}

/// Ignore traversal order while retaining every triangle and repeated corner.
fn canonical_triangles(triangles: &[Triangle]) -> Vec<[Vec3; 3]> {
    let mut result: Vec<_> = triangles
        .iter()
        .map(|triangle| {
            let mut corners = triangle.vertices;
            corners.sort_by(point_order);
            corners
        })
        .collect();
    result.sort_by(|left, right| {
        point_order(&left[0], &right[0])
            .then(point_order(&left[1], &right[1]))
            .then(point_order(&left[2], &right[2]))
    });
    result
}

/// Decode standard binary STL records independently of every model importer.
///
/// # Errors
///
/// The retained binary source has a truncated or inconsistent record layout.
fn binary_stl(bytes: &[u8]) -> Result<Vec<Triangle>> {
    let count = usize::try_from(u32::from_le_bytes(
        bytes
            .get(80..84)
            .ok_or("missing binary STL count")?
            .try_into()?,
    ))?;
    if count != 768 || bytes.len() != 84 + count * 50 {
        return Err("unexpected retained binary STL record count or size".into());
    }
    let mut triangles = Vec::with_capacity(count);
    for record in bytes[84..].as_chunks::<50>().0 {
        let mut corners = [Vec3::new(0., 0., 0.); 3];
        for (corner, point) in corners.iter_mut().enumerate() {
            let mut coordinates = [0.; 3];
            for (axis, value) in coordinates.iter_mut().enumerate() {
                let offset = 12 + corner * 12 + axis * 4;
                *value = f64::from(f32::from_le_bytes(record[offset..offset + 4].try_into()?));
            }
            *point = Vec3::new(coordinates[0], coordinates[1], coordinates[2]);
        }
        triangles.push(Triangle {
            vertices: corners,
            color: None,
        });
    }
    Ok(triangles)
}

/// Require every emitted planar normal to agree with its oriented triangle.
///
/// # Errors
///
/// An imported triangle lacks its authored flat corner normals.
fn aligned_flat_normals(scene: &Scene) -> Result<()> {
    assert_eq!(scene.appearances.len(), scene.triangles.len());
    for (index, triangle) in scene.triangles.iter().enumerate() {
        let cross = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0]);
        assert!(cross.length() > 0.);
        let expected = cross.normalized();
        let normals = scene
            .appearances
            .get(&index)
            .and_then(|appearance| appearance.normals)
            .ok_or("faceted face lost its authored corner normals")?;
        for normal in normals {
            assert!(close(normal, expected));
        }
    }
    Ok(())
}

#[test]
fn genuine_jumper_step_matches_unchanged_binary_stl_geometry_and_orientation() -> Result<()> {
    let path = fixture("step-jumper-faceted.stp");
    let step = fs::read(&path)?;
    assert_eq!(step.len(), 273_247);
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&step))),
        "2d31965483cc219cfdd3375816658fee90ac1eb01ccfb7d8539f35c25322826f"
    );
    let stl = fs::read(fixture("stl-jumper-source.stl"))?;
    assert_eq!(stl.len(), 38_484);
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&stl))),
        "955cae66249058e00a25dc913228766275803a1cc471c09581cb2984cf674733"
    );
    let reference = binary_stl(&stl)?;
    let scene = formats::load(&path)?;
    assert_eq!(scene.triangles.len(), 768);
    assert!(scene.lines.is_empty());
    assert!(scene.points.is_empty());
    for (actual, expected) in canonical_triangles(&scene.triangles)
        .into_iter()
        .zip(canonical_triangles(&reference))
    {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(close(actual, expected));
        }
    }
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for point in scene.positions() {
        for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    for (actual, expected) in minimum.into_iter().zip([
        -7.697_089_672_088_623,
        -11.977_172_851_562_5,
        -13.853_844_642_639_16,
    ]) {
        assert!((actual - expected).abs() < 1e-12);
    }
    for (actual, expected) in maximum.into_iter().zip([
        2.309_415_340_423_584,
        22.402_166_366_577_15,
        11.977_148_056_030_273,
    ]) {
        assert!((actual - expected).abs() < 1e-12);
    }
    assert!((scene.triangles.iter().map(area).sum::<f64>() - 889.016_523_211_688_9).abs() < 1e-9);
    assert!((volume(&scene) - 418.397_535_355_700_7).abs() < 1e-9);
    aligned_flat_normals(&scene)
}

/// Direction of a loop, bound, face surface or oriented-face wrapper.
#[derive(Clone, Copy)]
enum Sense {
    /// Retain the underlying traversal or normal direction.
    Forward,
    /// Reverse the underlying traversal or normal direction.
    Reversed,
}

impl Sense {
    /// Spell the corresponding STEP Boolean parameter.
    fn parameter(self) -> &'static str {
        match self {
            Self::Forward => ".T.",
            Self::Reversed => ".F.",
        }
    }

    /// Return the normal multiplier for this orientation.
    fn sign(self) -> f64 {
        match self {
            Self::Forward => 1.,
            Self::Reversed => -1.,
        }
    }
}

/// Entity inheritance syntax used by otherwise identical authored geometry.
#[derive(Clone, Copy)]
enum RecordForm {
    /// Ordinary simple entity with its flattened inherited attributes.
    Simple,
    /// External complex entity with attributes local to each supertype.
    External,
}

/// Independent switches for raw ring, bound, surface and wrapper orientation.
#[derive(Clone, Copy)]
struct FaceOrientation {
    /// Traversal order of every raw Cartesian-point polygon.
    traversal: Sense,
    /// Whether a face bound reverses that raw loop.
    bound: Sense,
    /// Whether the face agrees with its plane's normal.
    surface: Sense,
    /// Optional oriented-face wrapper applied after the underlying face.
    wrapper: Option<Sense>,
}

impl FaceOrientation {
    /// Author an ordinary face aligned with its plane and rings.
    const FORWARD: Self = Self {
        traversal: Sense::Forward,
        bound: Sense::Forward,
        surface: Sense::Forward,
        wrapper: None,
    };
}

/// Small independent Part 21 authoring helper; no importer geometry is reused.
struct Document {
    /// Complete records accumulated in their authored order.
    records: Vec<String>,
    /// Next unique identifier above the four fixed unit/context records.
    next: u64,
}

impl Document {
    /// Establish separate millimetre and metre contexts for geometry controls.
    fn new() -> Self {
        Self {
            records: vec![
                "#1=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));".into(),
                "#2=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#1)) REPRESENTATION_CONTEXT('',''));".into(),
                "#3=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.));".into(),
                "#4=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#3)) REPRESENTATION_CONTEXT('',''));".into(),
            ],
            next: 1_000,
        }
    }

    /// Retain one independently authored simple or complex entity expression.
    fn add(&mut self, mut expression: String) -> u64 {
        let id = self.next;
        self.next += 1;
        expression.insert_str(0, &format!("#{id}="));
        expression.push(';');
        self.records.push(expression);
        id
    }

    /// Author a finite Cartesian point with enough digits to preserve its value.
    fn point(&mut self, point: Vec3, form: RecordForm) -> u64 {
        let coordinates = format!("({:.17},{:.17},{:.17})", point.x, point.y, point.z);
        self.add(match form {
            RecordForm::Simple => format!("CARTESIAN_POINT('',{coordinates})"),
            RecordForm::External => format!(
                "(CARTESIAN_POINT({coordinates}) GEOMETRIC_REPRESENTATION_ITEM() POINT() REPRESENTATION_ITEM(''))"
            ),
        })
    }

    /// Author a direction without asking the production loader to infer it.
    fn direction(&mut self, direction: Vec3, form: RecordForm) -> u64 {
        let ratios = format!(
            "({:.17},{:.17},{:.17})",
            direction.x, direction.y, direction.z
        );
        self.add(match form {
            RecordForm::Simple => format!("DIRECTION('',{ratios})"),
            RecordForm::External => format!(
                "(DIRECTION({ratios}) GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM(''))"
            ),
        })
    }

    /// Author an explicit axis frame used by a plane or mapping.
    fn axis(&mut self, origin: Vec3, normal: Vec3, reference: Vec3, form: RecordForm) -> u64 {
        let point = self.point(origin, form);
        let z = self.direction(normal, form);
        let x = self.direction(reference, form);
        self.add(match form {
            RecordForm::Simple => format!("AXIS2_PLACEMENT_3D('',#{point},#{z},#{x})"),
            RecordForm::External => format!(
                "(AXIS2_PLACEMENT_3D(#{z},#{x}) GEOMETRIC_REPRESENTATION_ITEM() PLACEMENT(#{point}) REPRESENTATION_ITEM(''))"
            ),
        })
    }

    /// Author one plane-bounded polygon face, optionally using external inheritance.
    fn face(
        &mut self,
        rings: &[Vec<Vec3>],
        normal: Vec3,
        reference: Vec3,
        orientation: FaceOrientation,
        form: RecordForm,
    ) -> u64 {
        let origin = rings[0][0];
        let axis = self.axis(origin, normal, reference, form);
        let plane = self.add(match form {
            RecordForm::Simple => format!("PLANE('',#{axis})"),
            RecordForm::External => format!(
                "(ELEMENTARY_SURFACE(#{axis}) GEOMETRIC_REPRESENTATION_ITEM() PLANE() REPRESENTATION_ITEM('') SURFACE())"
            ),
        });
        let mut bounds = Vec::new();
        for (index, ring) in rings.iter().enumerate() {
            let mut points: Vec<_> = ring.iter().map(|&point| self.point(point, form)).collect();
            if matches!(orientation.traversal, Sense::Reversed) {
                points.reverse();
            }
            let polygon = self.add(match form {
                RecordForm::Simple => format!("POLY_LOOP('',({}))", references(&points)),
                RecordForm::External => format!(
                    "(GEOMETRIC_REPRESENTATION_ITEM() LOOP() POLY_LOOP(({})) REPRESENTATION_ITEM('') TOPOLOGICAL_REPRESENTATION_ITEM())",
                    references(&points)
                ),
            });
            let name = if index == 0 {
                "FACE_OUTER_BOUND"
            } else {
                "FACE_BOUND"
            };
            let flag = orientation.bound.parameter();
            bounds.push(self.add(match form {
                RecordForm::Simple => format!("{name}('',#{polygon},{flag})"),
                RecordForm::External => {
                    let subtype = if index == 0 { " FACE_OUTER_BOUND()" } else { "" };
                    format!(
                        "(FACE_BOUND(#{polygon},{flag}){subtype} REPRESENTATION_ITEM('') TOPOLOGICAL_REPRESENTATION_ITEM())"
                    )
                }
            }));
        }
        let face = self.add(match form {
            RecordForm::Simple => format!(
                "FACE_SURFACE('',({}),#{plane},{})",
                references(&bounds), orientation.surface.parameter()
            ),
            RecordForm::External => format!(
                "(FACE(({})) FACE_SURFACE(#{plane},{}) GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM('') TOPOLOGICAL_REPRESENTATION_ITEM())",
                references(&bounds), orientation.surface.parameter()
            ),
        });
        match orientation.wrapper {
            Some(sense) => self.add(format!("ORIENTED_FACE('',*,#{face},{})", sense.parameter())),
            None => face,
        }
    }

    /// Author an open surface container, avoiding a fictitious closed solid.
    fn surface(&mut self, face: u64) -> u64 {
        let shell = self.add(format!("OPEN_SHELL('',(#{face}))"));
        self.add(format!("SHELL_BASED_SURFACE_MODEL('',(#{shell}))"))
    }

    /// Add a representation with independently chosen items and unit context.
    fn representation(&mut self, kind: &str, items: &[u64], context: u64) -> u64 {
        self.add(format!(
            "{kind}('qualification',({}),#{context})",
            references(items)
        ))
    }

    /// Connect a selected representation to a real product definition.
    fn own(&mut self, representation: u64) {
        let application = self.add("APPLICATION_CONTEXT('mechanical')".into());
        let product_context = self.add(format!("PRODUCT_CONTEXT('',#{application},'mechanical')"));
        let definition_context = self.add(format!(
            "PRODUCT_DEFINITION_CONTEXT('',#{application},'design')"
        ));
        let product = self.add(format!(
            "PRODUCT('qualification','','',(#{product_context}))"
        ));
        let formation = self.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
        let definition = self.add(format!(
            "PRODUCT_DEFINITION('','',#{formation},#{definition_context})"
        ));
        let shape = self.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));
        self.add(format!(
            "SHAPE_DEFINITION_REPRESENTATION(#{shape},#{representation})"
        ));
    }

    /// Serialize the authored exchange with a standard AP242 schema header.
    fn text(&self) -> String {
        format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));ENDSEC;DATA;{}ENDSEC;END-ISO-10303-21;",
            self.records.join("\n")
        )
    }
}

/// Spell a STEP aggregate's entity references in authored order.
fn references(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Concave L-shaped exterior with one or two clockwise rectangular holes.
fn tilted_rings(holes: usize) -> Vec<Vec<Vec3>> {
    let rings = [
        vec![(0., 0.), (8., 0.), (8., 3.), (4., 3.), (4., 7.), (0., 7.)],
        vec![(1., 1.), (1., 2.), (3., 2.), (3., 1.)],
        vec![(1., 4.), (1., 6.), (2., 6.), (2., 4.)],
    ];
    rings
        .into_iter()
        .take(holes + 1)
        .map(|ring| ring.into_iter().map(|(u, v)| Vec3::new(u, v, u)).collect())
        .collect()
}

/// Build a selected open polygon surface with independently configured flags.
fn tilted_document(holes: usize, orientation: FaceOrientation, form: RecordForm) -> Document {
    let mut document = Document::new();
    let face = document.face(
        &tilted_rings(holes),
        Vec3::new(-1., 0., 1.),
        Vec3::new(1., 0., 1.),
        orientation,
        form,
    );
    let surface = document.surface(face);
    let representation =
        document.representation("MANIFOLD_SURFACE_SHAPE_REPRESENTATION", &[surface], 2);
    document.own(representation);
    document
}

/// Test XY-projected triangle coverage independently of polygon triangulation.
fn covers(triangle: &Triangle, x: f64, y: f64) -> bool {
    let [first, second, third] = triangle.vertices;
    let edge = |start: Vec3, end: Vec3| {
        (end.x - start.x) * (y - start.y) - (end.y - start.y) * (x - start.x)
    };
    let signs = [edge(first, second), edge(second, third), edge(third, first)];
    signs.iter().all(|&sign| sign >= -1e-12) || signs.iter().all(|&sign| sign <= 1e-12)
}

/// Verify analytic area, genuine openings and the selected planar normal sign.
///
/// # Errors
///
/// A triangle lacks the plane's authored normal metadata.
fn qualify_tilted(scene: &Scene, holes: usize, sign: f64) -> Result<()> {
    let expected_area = if holes == 1 { 38. } else { 36. } * 2_f64.sqrt();
    assert!((scene.triangles.iter().map(area).sum::<f64>() - expected_area).abs() < 1e-9);
    let normal = Vec3::new(-1., 0., 1.).normalized() * sign;
    for (index, triangle) in scene.triangles.iter().enumerate() {
        for point in triangle.vertices {
            assert!((point.z - point.x).abs() < 1e-12);
        }
        let cross = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0]);
        assert!(cross.dot(normal) > 0.);
        let normals = scene
            .appearances
            .get(&index)
            .and_then(|appearance| appearance.normals)
            .ok_or("tilted polygon lost its authored plane normals")?;
        assert!(normals.into_iter().all(|actual| close(actual, normal)));
    }
    for (u, v) in [(1.25, 1.25), (2., 1.5), (2.75, 1.75), (6., 5.)] {
        assert!(
            !scene
                .triangles
                .iter()
                .any(|triangle| covers(triangle, u, v))
        );
    }
    if holes == 2 {
        for (u, v) in [(1.25, 4.25), (1.5, 5.), (1.75, 5.75)] {
            assert!(
                !scene
                    .triangles
                    .iter()
                    .any(|triangle| covers(triangle, u, v))
            );
        }
    }
    for (u, v) in [(6., 1.), (2., 3.), (0.5, 6.)] {
        assert!(
            scene
                .triangles
                .iter()
                .any(|triangle| covers(triangle, u, v))
        );
    }
    Ok(())
}

#[test]
fn tilted_concave_surface_preserves_one_and_multiple_real_polygon_holes() -> Result<()> {
    for holes in [1, 2] {
        let scene = load_text(
            &tilted_document(holes, FaceOrientation::FORWARD, RecordForm::Simple).text(),
        )?;
        qualify_tilted(&scene, holes, 1.)?;
    }
    Ok(())
}

#[test]
fn raw_loop_bound_surface_and_wrapper_orientations_remain_independent() -> Result<()> {
    let pairs = [
        (Sense::Forward, Sense::Forward, Sense::Forward),
        (Sense::Reversed, Sense::Reversed, Sense::Forward),
        (Sense::Reversed, Sense::Forward, Sense::Reversed),
        (Sense::Forward, Sense::Reversed, Sense::Reversed),
    ];
    for (traversal, bound, surface) in pairs {
        for wrapper in [None, Some(Sense::Forward), Some(Sense::Reversed)] {
            let orientation = FaceOrientation {
                traversal,
                bound,
                surface,
                wrapper,
            };
            let scene = load_text(&tilted_document(2, orientation, RecordForm::Simple).text())?;
            let sign = surface.sign() * wrapper.map_or(1., Sense::sign);
            qualify_tilted(&scene, 2, sign)?;
        }
    }
    let inconsistent = FaceOrientation {
        bound: Sense::Reversed,
        ..FaceOrientation::FORWARD
    };
    assert!(
        rejection(&tilted_document(1, inconsistent, RecordForm::Simple).text())?
            .contains("orientation")
    );
    Ok(())
}

/// Coordinates of an offset 2×3×4 mm cuboid, independent of either decoder.
fn cuboid_points() -> [Vec3; 8] {
    [
        Vec3::new(2., 3., 4.),
        Vec3::new(4., 3., 4.),
        Vec3::new(4., 6., 4.),
        Vec3::new(2., 6., 4.),
        Vec3::new(2., 3., 8.),
        Vec3::new(4., 3., 8.),
        Vec3::new(4., 6., 8.),
        Vec3::new(2., 6., 8.),
    ]
}

/// Enumerated outward cuboid face rings, including both end caps.
const CUBOID_FACES: [[usize; 4]; 6] = [
    [0, 3, 2, 1],
    [4, 5, 6, 7],
    [0, 1, 5, 4],
    [1, 2, 6, 5],
    [2, 3, 7, 6],
    [3, 0, 4, 7],
];

/// Independently author a closed planar cuboid and return body/shell identities.
fn faceted_cuboid(document: &mut Document, points: [Vec3; 8], form: RecordForm) -> (u64, u64) {
    let mut faces = Vec::new();
    for ring in CUBOID_FACES {
        let vertices = ring.map(|index| points[index]);
        let reference = vertices[1] - vertices[0];
        let normal = reference.cross(vertices[2] - vertices[0]);
        faces.push(document.face(
            &[vertices.to_vec()],
            normal,
            reference,
            FaceOrientation::FORWARD,
            form,
        ));
    }
    let shell = document.add(match form {
        RecordForm::Simple => format!("CLOSED_SHELL('',({}))", references(&faces)),
        RecordForm::External => format!(
            "(CLOSED_SHELL() CONNECTED_FACE_SET(({})) REPRESENTATION_ITEM('') TOPOLOGICAL_REPRESENTATION_ITEM())", references(&faces)
        ),
    });
    let body = document.add(match form {
        RecordForm::Simple => format!("FACETED_BREP('cuboid',#{shell})"),
        RecordForm::External => format!(
            "(FACETED_BREP() GEOMETRIC_REPRESENTATION_ITEM() MANIFOLD_SOLID_BREP(#{shell}) REPRESENTATION_ITEM('cuboid') SOLID_MODEL())"
        ),
    });
    (body, shell)
}

#[test]
fn external_complex_faceted_entities_preserve_solid_and_holed_surface_geometry() -> Result<()> {
    for form in [RecordForm::Simple, RecordForm::External] {
        let mut document = Document::new();
        let (body, _) = faceted_cuboid(&mut document, cuboid_points(), form);
        let representation =
            document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &[body], 2);
        document.own(representation);
        let scene = load_text(&document.text())?;
        assert_eq!(scene.triangles.len(), 12);
        assert!((scene.triangles.iter().map(area).sum::<f64>() - 52.).abs() < 1e-10);
        assert!((volume(&scene) - 24.).abs() < 1e-10);
        aligned_flat_normals(&scene)?;
        qualify_tilted(
            &load_text(&tilted_document(2, FaceOrientation::FORWARD, form).text())?,
            2,
            1.,
        )?;
    }
    Ok(())
}

/// Build two rotated mapped occurrences and a distinct coincident occurrence.
fn mapped_cuboid() -> Document {
    let mut document = Document::new();
    let (body, _) = faceted_cuboid(
        &mut document,
        cuboid_points().map(|point| point / 1_000.),
        RecordForm::Simple,
    );
    let geometry = document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &[body], 4);
    let origin = document.axis(
        Vec3::new(0.002, 0.003, 0.004),
        Vec3::new(0., 0., 1.),
        Vec3::new(0., 1., 0.),
        RecordForm::Simple,
    );
    let source = document.representation("SHAPE_REPRESENTATION", &[origin], 4);
    document.add(format!(
        "SHAPE_REPRESENTATION_RELATIONSHIP('','',#{geometry},#{source})"
    ));
    let map = document.add(format!("REPRESENTATION_MAP(#{origin},#{source})"));
    let first = document.axis(
        Vec3::new(10., 20., 30.),
        Vec3::new(0., 1., 0.),
        Vec3::new(1., 0., 0.),
        RecordForm::Simple,
    );
    let second = document.axis(
        Vec3::new(-10., 5., 2.),
        Vec3::new(0., 0., -1.),
        Vec3::new(0., 1., 0.),
        RecordForm::Simple,
    );
    let mut occurrences = Vec::new();
    for target in [first, second, first] {
        occurrences.push(document.add(format!("MAPPED_ITEM('occurrence',#{map},#{target})")));
    }
    let assembly = document.representation("SHAPE_REPRESENTATION", &occurrences, 2);
    document.own(assembly);
    document
}

#[test]
fn mixed_units_equivalent_representations_keep_rotated_and_coincident_occurrences() -> Result<()> {
    let scene = load_text(&mapped_cuboid().text())?;
    assert_eq!(scene.triangles.len(), 36);
    assert!((scene.triangles.iter().map(area).sum::<f64>() - 156.).abs() < 1e-9);
    assert!((volume(&scene) - 72.).abs() < 1e-9);
    aligned_flat_normals(&scene)?;
    let relative = cuboid_points().map(|point| point - Vec3::new(2., 3., 4.));
    // Hand-composed target frames after subtracting and unrotating the metre source frame.
    let first = relative.map(|point| Vec3::new(10. + point.y, 20. + point.z, 30. + point.x));
    let second = relative.map(|point| Vec3::new(-10. - point.x, 5. + point.y, 2. - point.z));
    let mut counts = [0, 0];
    for triangle in &scene.triangles {
        if triangle
            .vertices
            .iter()
            .all(|&vertex| first.iter().any(|&point| close(vertex, point)))
        {
            counts[0] += 1;
        } else {
            assert!(
                triangle
                    .vertices
                    .iter()
                    .all(|&vertex| second.iter().any(|&point| close(vertex, point)))
            );
            counts[1] += 1;
        }
    }
    assert_eq!(counts, [24, 12]);
    Ok(())
}

/// Add an independently enumerated AP242 cuboid alternative in an equivalent group.
fn add_tessellated_cuboid(
    document: &mut Document,
    representation: u64,
    container: &str,
    target: Option<u64>,
    order: Sense,
) {
    let coordinates = document.add("COORDINATES_LIST('',8,((2.,3.,4.),(4.,3.,4.),(4.,6.,4.),(2.,6.,4.),(2.,3.,8.),(4.,3.,8.),(4.,6.,8.),(2.,6.,8.)))".into());
    let face = document.add(format!(
        "TRIANGULATED_FACE('',#{coordinates},8,(),$,(),((1,3,2),(1,4,3),(5,6,7),(5,7,8),(1,2,6),(1,6,5),(2,3,7),(2,7,6),(3,4,8),(3,8,7),(4,1,5),(4,5,8)))"
    ));
    let link = target.map_or_else(|| "$".into(), |id| format!("#{id}"));
    let body = document.add(format!("{container}('alternative',(#{face}),{link})"));
    let tessellation = document.representation("TESSELLATED_SHAPE_REPRESENTATION", &[body], 2);
    let endpoints = match order {
        Sense::Forward => format!("#{representation},#{tessellation}"),
        Sense::Reversed => format!("#{tessellation},#{representation}"),
    };
    document.add(format!(
        "SHAPE_REPRESENTATION_RELATIONSHIP('','',{endpoints})"
    ));
}

#[test]
fn explicit_ap242_body_and_shell_links_deduplicate_facets_in_both_endpoint_orders() -> Result<()> {
    for form in [RecordForm::Simple, RecordForm::External] {
        for container in ["TESSELLATED_SOLID", "TESSELLATED_SHELL"] {
            for order in [Sense::Forward, Sense::Reversed] {
                let mut document = Document::new();
                let (body, shell) = faceted_cuboid(&mut document, cuboid_points(), form);
                let representation =
                    document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &[body], 2);
                document.own(representation);
                let target = if container == "TESSELLATED_SOLID" {
                    body
                } else {
                    shell
                };
                add_tessellated_cuboid(
                    &mut document,
                    representation,
                    container,
                    Some(target),
                    order,
                );
                let scene = load_text(&document.text())?;
                assert_eq!(scene.triangles.len(), 12);
                assert!((scene.triangles.iter().map(area).sum::<f64>() - 52.).abs() < 1e-10);
                assert!((volume(&scene) - 24.).abs() < 1e-10);
                aligned_flat_normals(&scene)?;
            }
        }
    }
    let mut document = Document::new();
    let (body, _) = faceted_cuboid(&mut document, cuboid_points(), RecordForm::Simple);
    let representation = document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &[body], 2);
    document.own(representation);
    add_tessellated_cuboid(
        &mut document,
        representation,
        "TESSELLATED_SOLID",
        None,
        Sense::Forward,
    );
    let separate = load_text(&document.text())?;
    assert_eq!(separate.triangles.len(), 24);
    assert!((separate.triangles.iter().map(area).sum::<f64>() - 104.).abs() < 1e-10);
    Ok(())
}

#[test]
fn unselected_malformed_construction_is_ignored_but_selected_geometry_must_fail() -> Result<()> {
    for selected in [false, true] {
        let mut document = Document::new();
        let (body, _) = faceted_cuboid(&mut document, cuboid_points(), RecordForm::Simple);
        let malformed = document.face(
            &[vec![
                Vec3::new(0., 0., 0.),
                Vec3::new(1., 0., 0.),
                Vec3::new(0., 1., 1.),
            ]],
            Vec3::new(0., 0., 1.),
            Vec3::new(1., 0., 0.),
            FaceOrientation::FORWARD,
            RecordForm::Simple,
        );
        let shell = document.add(format!("CLOSED_SHELL('',(#{malformed}))"));
        let construction = document.add(format!("FACETED_BREP('construction',#{shell})"));
        document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &[construction], 2);
        let items = if selected {
            vec![body, construction]
        } else {
            vec![body]
        };
        let representation =
            document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &items, 2);
        document.own(representation);
        if selected {
            let error = rejection(&document.text())?;
            assert!(
                error.contains("supporting plane") || error.contains("coplanar"),
                "{error}"
            );
        } else {
            let scene = load_text(&document.text())?;
            assert_eq!(scene.triangles.len(), 12);
            assert!((volume(&scene) - 24.).abs() < 1e-10);
        }
    }
    Ok(())
}

/// Independently export an analytic cuboid whose first face can become a polygon.
fn analytic_cuboid() -> String {
    let vertex = builder::vertex(Point3::new(2., 3., 4.));
    let edge = builder::tsweep(&vertex, Vector3::new(2., 0., 0.));
    let face = builder::tsweep(&edge, Vector3::new(0., 3., 0.));
    let solid = builder::tsweep(&face, Vector3::new(0., 0., 4.)).compress();
    CompleteStepDisplay::new(StepModel::from(&solid), StepHeaderDescriptor::default()).to_string()
}

/// Add authored records immediately before the final DATA terminator.
///
/// # Errors
///
/// The independently authored exchange has no DATA terminator.
fn add_records(source: &str, records: &str) -> Result<String> {
    let end = source
        .rfind("ENDSEC;")
        .ok_or("authored exchange has no DATA terminator")?;
    Ok(format!("{}{records}\n{}", &source[..end], &source[end..]))
}

#[test]
fn required_mixed_loop_shells_and_faceted_voids_fail_without_partial_geometry() -> Result<()> {
    let analytic = analytic_cuboid();
    assert!(analytic.contains("#19 = FACE_BOUND('', #20, .F.);"));
    let mixed = analytic.replace(
        "#19 = FACE_BOUND('', #20, .F.);",
        "#19 = FACE_OUTER_BOUND('', #600, .F.);",
    );
    let mixed = add_records(&mixed, "#600=POLY_LOOP('',(#158,#159,#160,#161));")?;
    let error = rejection(&mixed)?;
    assert!(
        error.contains("mixed")
            || error.contains("EDGE_LOOP")
            || error.contains("unsupported schema"),
        "{error}"
    );

    let mut document = Document::new();
    let (_, outer) = faceted_cuboid(&mut document, cuboid_points(), RecordForm::Simple);
    let inner_points = cuboid_points()
        .map(|point| (point - Vec3::new(3., 4.5, 6.)) * 0.5 + Vec3::new(3., 4.5, 6.));
    let (_, inner) = faceted_cuboid(&mut document, inner_points, RecordForm::Simple);
    let void = document.add(format!("ORIENTED_CLOSED_SHELL('cavity',*,#{inner},.F.)"));
    let body = document.add(format!(
        "(BREP_WITH_VOIDS((#{void})) FACETED_BREP() GEOMETRIC_REPRESENTATION_ITEM() MANIFOLD_SOLID_BREP(#{outer}) REPRESENTATION_ITEM('body with cavity') SOLID_MODEL())"
    ));
    let representation = document.representation("FACETED_BREP_SHAPE_REPRESENTATION", &[body], 2);
    document.own(representation);
    assert!(rejection(&document.text())?.contains("void"));
    Ok(())
}

#[test]
fn outside_crossing_touching_and_nonplanar_polygon_boundaries_are_rejected() -> Result<()> {
    let mut cases = Vec::new();
    let mut outside = tilted_rings(1);
    outside[1] = vec![
        Vec3::new(5., 4., 5.),
        Vec3::new(5., 5., 5.),
        Vec3::new(6., 5., 6.),
        Vec3::new(6., 4., 6.),
    ];
    cases.push(outside);
    let mut crossing = tilted_rings(1);
    crossing[1].swap(1, 2);
    cases.push(crossing);
    let mut touching = tilted_rings(2);
    touching[2] = vec![
        Vec3::new(1., 2., 1.),
        Vec3::new(1., 3., 1.),
        Vec3::new(2., 3., 2.),
        Vec3::new(2., 2., 2.),
    ];
    cases.push(touching);
    let mut nonplanar = tilted_rings(1);
    nonplanar[1][0].z += 0.5;
    cases.push(nonplanar);
    for rings in cases {
        let mut document = Document::new();
        let face = document.face(
            &rings,
            Vec3::new(-1., 0., 1.),
            Vec3::new(1., 0., 1.),
            FaceOrientation::FORWARD,
            RecordForm::Simple,
        );
        let surface = document.surface(face);
        let representation =
            document.representation("MANIFOLD_SURFACE_SHAPE_REPRESENTATION", &[surface], 2);
        document.own(representation);
        let error = rejection(&document.text())?;
        assert_ne!(error, "");
    }
    Ok(())
}
