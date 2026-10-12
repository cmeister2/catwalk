//! AMF exporter qualification and independently calculated placement regressions.
#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    formats,
    render::{RenderOptions, render},
    scene::{AlphaMode, Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique temporary-input identifier for concurrent tests.
static NEXT: AtomicU64 = AtomicU64::new(0);
/// One temporary model directory, removed after each test.
struct Input(PathBuf);
impl Input {
    /// Store one isolated model without editing retained exporter assets.
    fn new(bytes: &[u8]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "catwalk-amf-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("model.amf");
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}
/// Find one byte-identical third-party reference in the corpus.
fn reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/amf")
        .join(name)
}
/// Author a closed asymmetric tetrahedron from known coordinates and winding.
fn tetrahedron(
    root_attributes: &str,
    before: &str,
    after: &str,
    object_color: &str,
    volume_attributes: &str,
    volume_color: &str,
    triangle_color: &str,
) -> String {
    format!(
        "<amf {root_attributes}>{before}<object id=\"0\">{object_color}<mesh><vertices><vertex><coordinates><x>0</x><y>0</y><z>0</z></coordinates></vertex><vertex><coordinates><x>2</x><y>0</y><z>0</z></coordinates></vertex><vertex><coordinates><x>0</x><y>1</y><z>0</z></coordinates></vertex><vertex><coordinates><x>0</x><y>0</y><z>3</z></coordinates></vertex></vertices><volume {volume_attributes}>{volume_color}<triangle>{triangle_color}<v1>0</v1><v2>2</v2><v3>1</v3></triangle><triangle><v1>0</v1><v2>1</v2><v3>3</v3></triangle><triangle><v1>1</v1><v2>2</v2><v3>3</v3></triangle><triangle><v1>2</v1><v2>0</v2><v3>3</v3></triangle></volume></mesh></object>{after}</amf>"
    )
}
/// Produce one ordinary mesh without additional appearance fields.
fn mesh(before: &str, after: &str) -> String {
    tetrahedron("unit=\"millimeter\"", before, after, "", "", "", "")
}
/// Load an authored XML source through the public dispatcher.
fn load(xml: &str) -> Scene {
    let input = Input::new(xml.as_bytes());
    formats::load(&input.0).unwrap()
}
/// Calculate bounds independently from emitted points.
fn bounds(scene: &Scene) -> (Vec3, Vec3) {
    let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for position in scene.positions() {
        minimum = Vec3::new(
            minimum.x.min(position.x),
            minimum.y.min(position.y),
            minimum.z.min(position.z),
        );
        maximum = Vec3::new(
            maximum.x.max(position.x),
            maximum.y.max(position.y),
            maximum.z.max(position.z),
        );
    }
    (minimum, maximum)
}
/// Require three independently expected coordinates within numeric rotation roundoff.
fn near(actual: Vec3, expected: [f64; 3]) {
    for (value, expected) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        assert!((value - expected).abs() < 1e-9, "{value} != {expected}");
    }
}
/// Calculate triangle surface area and oriented enclosed volume from output geometry.
fn measurements(scene: &Scene) -> (f64, f64) {
    let mut area = 0.0;
    let mut volume = 0.0;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        area += (b - a).cross(c - a).length() / 2.0;
        volume += a.dot(b.cross(c)) / 6.0;
    }
    (area, volume)
}
/// Build a single-file or deliberately ambiguous archive with actual ZIP encodings.
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for &(name, data) in files {
        archive
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        archive.write_all(data).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

#[test]
fn original_slic3r_export_and_compressed_reference_have_exact_independent_geometry() {
    let plain = std::fs::read(reference("amf-slic3r-plain.amf")).unwrap();
    let compressed = std::fs::read(reference("amf-slic3r-deflated.amf")).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&plain))),
        "ff3be11c5dc4f31e683afeab1008400c95170ea99008e37c6dfab4be30e157c3"
    );
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&compressed))),
        "f707308b505efe4572b80febc707bdc0b8dac03a014d4a85e5f73b9f8bf8944f"
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(&compressed)).unwrap();
    assert_eq!(archive.len(), 1);
    let mut member = archive.by_index(0).unwrap();
    assert_eq!(member.name().unwrap(), "20mmbox.amf");
    let mut expanded = Vec::new();
    member.read_to_end(&mut expanded).unwrap();
    assert_eq!(expanded, plain);
    let options = RenderOptions {
        width: 128,
        height: 128,
        ..RenderOptions::default()
    };
    let a = formats::load(&reference("amf-slic3r-plain.amf")).unwrap();
    let b = formats::load(&reference("amf-slic3r-deflated.amf")).unwrap();
    for scene in [&a, &b] {
        assert_eq!(scene.triangles.len(), 12);
        let (minimum, maximum) = bounds(scene);
        near(minimum, [57.5, 25.0, 0.0]);
        near(maximum, [77.5, 45.0, 10.0]);
        let (area, volume) = measurements(scene);
        assert!((area - 1600.0).abs() < 1e-9);
        assert!((volume - 4000.0).abs() < 1e-9);
    }
    assert_eq!(render(&a, &options).unwrap(), render(&b, &options).unwrap());
}

#[test]
fn all_document_units_normalize_positions_and_instance_translation() {
    for (unit, factor) in [
        ("millimeter", 1.0),
        ("inch", 25.4),
        ("feet", 304.8),
        ("meter", 1000.0),
        ("micron", 0.001),
    ] {
        let xml = tetrahedron(
            &format!("unit=\"{unit}\""),
            "",
            "<constellation id=\"1\"><instance objectid=\"0\"><deltax>4</deltax><deltay>5</deltay><deltaz>6</deltaz></instance></constellation>",
            "",
            "",
            "",
            "",
        );
        let scene = load(&xml);
        assert_eq!(scene.triangles.len(), 4);
        let (minimum, maximum) = bounds(&scene);
        near(minimum, [4.0 * factor, 5.0 * factor, 6.0 * factor]);
        near(maximum, [6.0 * factor, 6.0 * factor, 9.0 * factor]);
    }
}

#[test]
fn standard_degree_rotations_and_nested_constellations_preserve_placement() {
    let xml = mesh(
        "",
        "<constellation id=\"10\"><instance objectid=\"0\"><deltax>10</deltax><rz>90</rz></instance></constellation><constellation id=\"20\"><instance objectid=\"10\"><deltay>10</deltay><ry>90</ry></instance></constellation>",
    );
    let scene = load(&xml);
    assert_eq!(scene.triangles.len(), 4);
    let (minimum, maximum) = bounds(&scene);
    near(minimum, [0.0, 10.0, -10.0]);
    near(maximum, [3.0, 12.0, -9.0]);
    assert!((measurements(&scene).1 - 1.0).abs() < 1e-9);
    let x = load(&mesh(
        "",
        "<constellation id=\"1\"><instance objectid=\"0\"><rx>90</rx></instance></constellation>",
    ));
    let (minimum, maximum) = bounds(&x);
    near(minimum, [0.0, -3.0, 0.0]);
    near(maximum, [2.0, 0.0, 1.0]);
}

#[test]
fn qualified_slic3r_radians_and_uniform_scale_are_explicit() {
    let xml = mesh(
        "<metadata type=\"cad\">Slic3r 1.3.1-dev</metadata>",
        &format!(
            "<constellation id=\"1\"><instance objectid=\"0\"><deltax>10</deltax><deltay>20</deltay><deltaz>30</deltaz><rz>{}</rz><scale>2</scale></instance></constellation>",
            std::f64::consts::FRAC_PI_2
        ),
    );
    let scene = load(&xml);
    let (minimum, maximum) = bounds(&scene);
    near(minimum, [8.0, 20.0, 30.0]);
    near(maximum, [10.0, 24.0, 36.0]);
    assert!((measurements(&scene).1 - 8.0).abs() < 1e-9);
}

#[test]
fn constant_colors_follow_triangle_volume_object_material_precedence() {
    let blue = "<color><r>0</r><g>0</g><b>1</b></color>";
    let green = "<color><r>0</r><g>1</g><b>0</b></color>";
    let yellow = "<color><r>1</r><g>1</g><b>0</b></color>";
    let red = "<color><r>1</r><g>0</g><b>0</b><a>0.5</a></color>";
    let xml = tetrahedron(
        "",
        &format!("<material id=\"1\">{blue}</material>"),
        "",
        green,
        "materialid=\"1\"",
        yellow,
        red,
    );
    let scene = load(&xml);
    assert_eq!(scene.appearances.len(), 4);
    assert_eq!(scene.appearances[&0].base_color, [1.0, 0.0, 0.0, 0.5]);
    assert_eq!(scene.appearances[&0].alpha_mode, AlphaMode::Blend);
    assert_eq!(scene.appearances[&1].base_color, [1.0, 1.0, 0.0, 1.0]);
    let object = load(&tetrahedron(
        "",
        &format!("<material id=\"1\">{blue}</material>"),
        "",
        green,
        "materialid=\"1\"",
        "",
        "",
    ));
    assert_eq!(object.appearances[&0].base_color, [0.0, 1.0, 0.0, 1.0]);
    let material = load(&tetrahedron(
        "",
        &format!("<material id=\"1\">{blue}</material>"),
        "",
        "",
        "materialid=\"1\"",
        "",
        "",
    ));
    assert_eq!(material.appearances[&0].base_color, [0.0, 0.0, 1.0, 1.0]);
    let midtone = load(&tetrahedron(
        "",
        "",
        "",
        "<color><r>0.5</r><g>0.5</g><b>0.5</b><a>0.25</a></color>",
        "",
        "",
        "",
    ));
    let color = midtone.appearances[&0].base_color;
    for channel in &color[..3] {
        assert!((channel - 0.214_041_140_482_232_55).abs() < 1e-14);
    }
    assert_eq!(color[3], 0.25);
    assert_eq!(midtone.appearances[&0].alpha_mode, AlphaMode::Blend);
}

#[test]
fn geometry_features_that_cannot_be_rendered_are_explicit_errors() {
    let ordinary = mesh("", "");
    for invalid in [
        ordinary.replace("<vertices>", "<vertices><edge><v1>0</v1><v2>1</v2></edge>"),
        ordinary.replacen(
            "</coordinates>",
            "</coordinates><normal><nx>0</nx><ny>0</ny><nz>1</nz></normal>",
            1,
        ),
        ordinary.replacen(
            "</coordinates>",
            "</coordinates><color><r>1</r><g>0</g><b>0</b></color>",
            1,
        ),
        ordinary.replace("</amf>", "<texture id=\"1\">AA==</texture></amf>"),
        ordinary.replace(
            "<volume >",
            "<volume ><metadata type=\"slic3r.modifier\">1</metadata>",
        ),
        ordinary.replace("<volume >", "<volume type=\"support\">"),
        ordinary.replace("<volume >", "<volume materialid=\"0\">"),
        ordinary.replace(
            "</amf>",
            "<material id=\"1\"><composite materialid=\"2\">0.5</composite></material></amf>",
        ),
        tetrahedron(
            "",
            "",
            "",
            "<color><r>x/10</r><g>0</g><b>0</b></color>",
            "",
            "",
            "",
        ),
        mesh(
            "",
            "<constellation id=\"1\"><instance objectid=\"0\"><rx>20</rx><rz>30</rz></instance></constellation>",
        ),
        mesh(
            "",
            "<constellation id=\"1\"><instance objectid=\"0\"><scale>2</scale></instance></constellation>",
        ),
    ] {
        let input = Input::new(invalid.as_bytes());
        assert!(
            formats::load(&input.0).is_err(),
            "accepted unsupported geometry: {invalid}"
        );
    }
}

#[test]
fn malformed_ids_indices_scalars_and_unreachable_cycles_fail() {
    let ordinary = mesh("", "");
    for invalid in [
        ordinary.replace("<x>2</x>", "<x>NaN</x>"),
        ordinary.replace("<v3>1</v3>", "<v3>99</v3>"),
        ordinary.replace("<v3>1</v3>", "<v3>-1</v3>"),
        ordinary.replace("<v3>1</v3>", "<v3>0</v3>"),
        ordinary.replace("<x>2</x>", "<x>2</x><x>3</x>"),
        ordinary.replace("<volume >", "<volume materialid=\"999\">"),
        ordinary.replace("unit=\"millimeter\"", "unit=\"parsec\""),
        ordinary.replace(
            "</amf>",
            "<constellation id=\"0\"><instance objectid=\"0\"/></constellation></amf>",
        ),
        mesh(
            "",
            "<constellation id=\"1\"><instance objectid=\"999\"/></constellation>",
        ),
        mesh(
            "",
            "<constellation id=\"1\"><instance objectid=\"2\"/></constellation><constellation id=\"2\"><instance objectid=\"1\"/></constellation>",
        ),
        format!("<!DOCTYPE amf [<!ENTITY x '2'>]>{ordinary}"),
        ordinary.replace("<amf ", "<amf xmlns=\"https://example.invalid/other\" "),
    ] {
        let input = Input::new(invalid.as_bytes());
        assert!(
            formats::load(&input.0).is_err(),
            "accepted malformed source: {invalid}"
        );
    }
}

#[test]
fn archive_member_selection_is_unambiguous_and_paths_are_never_extracted() {
    let xml = mesh("", "");
    let accepted = Input::new(&zip(&[("nested/model.amf", xml.as_bytes())]));
    assert_eq!(formats::load(&accepted.0).unwrap().triangles.len(), 4);
    for files in [
        vec![("model.amf", xml.as_bytes()), ("other.amf", xml.as_bytes())],
        vec![("../model.amf", xml.as_bytes())],
        vec![("/model.amf", xml.as_bytes())],
        vec![("model.txt", xml.as_bytes())],
    ] {
        let input = Input::new(&zip(&files));
        assert!(formats::load(&input.0).is_err());
    }
    let mut duplicate = zip(&[("model.amf", xml.as_bytes()), ("other.amf", xml.as_bytes())]);
    let offsets: Vec<_> = duplicate
        .windows(9)
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == b"other.amf").then_some(index))
        .collect();
    assert_eq!(offsets.len(), 2);
    for offset in offsets {
        duplicate[offset..offset + 9].copy_from_slice(b"model.amf");
    }
    assert!(formats::load(&Input::new(&duplicate).0).is_err());
    let mut bad_crc = zip(&[("model.amf", xml.as_bytes())]);
    let central = bad_crc
        .windows(4)
        .rposition(|bytes| bytes == b"PK\x01\x02")
        .unwrap();
    bad_crc[central + 16] ^= 1;
    assert!(formats::load(&Input::new(&bad_crc).0).is_err());
    let mut truncated = zip(&[("model.amf", xml.as_bytes())]);
    truncated.truncate(truncated.len() / 2);
    assert!(formats::load(&Input::new(&truncated).0).is_err());
}

#[test]
fn xml_comments_preserve_numeric_text_and_reused_graph_depth_is_bounded() {
    let scene = load(&mesh("", "").replace("<x>2</x>", "<x>1<!--digit separator-->0</x>"));
    let (_, maximum) = bounds(&scene);
    near(maximum, [10.0, 1.0, 3.0]);
    let ordinary = mesh("", "");
    let zero_modifier = ordinary.replace(
        "<volume >",
        "<volume ><metadata type=\"slic3r.modifier\">0<!--split text-->0</metadata>",
    );
    assert_eq!(load(&zero_modifier).triangles.len(), 4);
    for value in [
        "0<!--split text-->1",
        "<value>0</value>",
        "",
        "false",
        "-1",
        "1",
    ] {
        let invalid = ordinary.replace(
            "<volume >",
            &format!("<volume ><metadata type=\"slic3r.modifier\">{value}</metadata>"),
        );
        assert!(formats::load(&Input::new(invalid.as_bytes()).0).is_err());
    }
    let mut nodes = String::new();
    for id in 1..=80 {
        std::fmt::Write::write_fmt(
            &mut nodes,
            format_args!(
                "<constellation id=\"{id}\"><instance objectid=\"{}\"/></constellation>",
                id - 1
            ),
        )
        .unwrap();
    }
    let xml = mesh("", &nodes);
    let error = formats::load(&Input::new(xml.as_bytes()).0)
        .unwrap_err()
        .to_string();
    assert!(error.contains("depth limit"), "{error}");
}
