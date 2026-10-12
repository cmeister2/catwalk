//! Independent texture, geometry, packaging, and retained-source qualification for 3MF.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    Result, formats, render,
    scene::{AlphaMode, Scene, TextureFilter, TextureInterpolationSpace, TextureWrap, Vec3},
};
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicUsize, atomic::Ordering},
};

/// Core namespace used by authored package controls.
const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
/// Materials namespace used by authored texture resources and groups.
const MATERIAL: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
/// Production namespace used by the multipart ownership control.
const PRODUCTION: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
/// OPC relationship namespace used by authored model and image references.
const OPC: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// Required relationship type for an owning model's texture image.
const TEXTURE_RELATIONSHIP: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture";
/// Required relationship type for a package's root or production child model.
const MODEL_RELATIONSHIP: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
/// Default authored texture coordinates, expressed from the source image's lower left.
const COORDINATES: &str = "<m:tex2coord u=\"0\" v=\"0\"/><m:tex2coord u=\"1\" v=\"0\"/><m:tex2coord u=\"0\" v=\"1\"/><m:tex2coord u=\"1\" v=\"1\"/>";
/// One authored geometric triangle with three different texture-coordinate indices.
const TRIANGLE: &str =
    "<triangle v1=\"0\" v2=\"1\" v3=\"2\" pid=\"3\" p1=\"0\" p2=\"1\" p3=\"2\"/>";
/// Unique temporary filename sequence for concurrently executed package tests.
static NEXT_PACKAGE: AtomicUsize = AtomicUsize::new(0);

/// Temporary package path removed after its scene has been loaded.
struct TemporaryPackage {
    /// Unique file path including the native 3MF extension.
    path: PathBuf,
}

impl Drop for TemporaryPackage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Write a temporary native ZIP package and evaluate it without touching retained fixtures.
///
/// # Errors
/// Returns an error if package writing or the supplied loading operation fails.
fn with_package<T>(entries: &[(&str, &[u8])], load: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let package = TemporaryPackage {
        path: std::env::temp_dir().join(format!(
            "catwalk-3mf-texture-{}-{}.3mf",
            std::process::id(),
            NEXT_PACKAGE.fetch_add(1, Ordering::Relaxed)
        )),
    };
    let mut archive = zip::ZipWriter::new(fs::File::create(&package.path)?);
    for &(name, bytes) in entries {
        archive.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        archive.write_all(bytes)?;
    }
    archive.finish()?;
    load(&package.path)
}

/// Encode the four source-image quadrants, including deliberately nonopaque alpha.
fn quadrant_png() -> Vec<u8> {
    let image = RgbaImage::from_raw(
        2,
        2,
        vec![
            255, 0, 0, 0, 0, 255, 0, 64, 0, 0, 255, 128, 255, 255, 255, 255,
        ],
    )
    .unwrap();
    let mut bytes = Vec::new();
    render::write_png(&image, &mut bytes).unwrap();
    bytes
}

/// Build one direct-texture model with independently selectable source properties.
fn textured_model(sampler: &str, coordinates: &str, properties: &str, triangles: &str) -> String {
    format!(
        "<model xmlns=\"{CORE}\" xmlns:m=\"{MATERIAL}\" requiredextensions=\"m\"><resources><m:texture2d id=\"2\" path=\"/3D/Textures/image.png\" contenttype=\"image/png\" {sampler}/><m:texture2dgroup id=\"3\" texid=\"2\">{coordinates}</m:texture2dgroup><object id=\"1\" {properties}><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"4\" y=\"0\" z=\"0\"/><vertex x=\"0\" y=\"4\" z=\"0\"/></vertices><triangles>{triangles}</triangles></mesh></object></resources><build><item objectid=\"1\"/></build></model>"
    )
}

/// Build a single typed relationship with an optional target-mode declaration.
fn relationship(kind: &str, target: &str, mode: &str) -> String {
    format!(
        "<Relationships xmlns=\"{OPC}\"><Relationship Id=\"texture\" Type=\"{kind}\" Target=\"{target}\" {mode}/></Relationships>"
    )
}

/// Load an authored model with an optional owning-model image relationship part.
///
/// # Errors
/// Returns an error for malformed authored package controls or filesystem failures.
fn authored(model: &str, relationships: Option<&str>, image: &[u8]) -> Result<Scene> {
    let root = relationship(MODEL_RELATIONSHIP, "/3D/main.model", "");
    let mut entries = vec![
        ("_rels/.rels", root.as_bytes()),
        ("3D/main.model", model.as_bytes()),
        ("3D/Textures/image.png", image),
    ];
    if let Some(relationships) = relationships {
        entries.push(("3D/_rels/main.model.rels", relationships.as_bytes()));
    }
    with_package(&entries, formats::load)
}

/// Resolve one retained official archive in the portable format corpus.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/mesh_scene")
        .join(name)
}

/// Check analytic vertex bounds and total triangle area against independent references.
fn assert_geometry(scene: &Scene, triangles: usize, low: [f64; 3], high: [f64; 3], area: f64) {
    assert_eq!(scene.triangles.len(), triangles);
    let mut actual_low = [f64::INFINITY; 3];
    let mut actual_high = [f64::NEG_INFINITY; 3];
    let mut actual_area = 0.;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        actual_area += (b - a).cross(c - a).length() * 0.5;
        for vertex in triangle.vertices {
            for (axis, value) in [vertex.x, vertex.y, vertex.z].into_iter().enumerate() {
                actual_low[axis] = actual_low[axis].min(value);
                actual_high[axis] = actual_high[axis].max(value);
            }
        }
    }
    for axis in 0..3 {
        assert!((actual_low[axis] - low[axis]).abs() < 1e-6);
        assert!((actual_high[axis] - high[axis]).abs() < 1e-6);
    }
    assert!((actual_area - area).abs() < 1e-4);
}

/// Independently read the unchanged model XML and verify every imported corner's UV association.
fn assert_source_uvs(scene: &Scene, bytes: &[u8]) {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    archive
        .by_name("3D/3dmodel.model")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    let document = roxmltree::Document::parse(&xml).unwrap();
    let mut groups = HashMap::new();
    for group in document
        .descendants()
        .filter(|n| n.has_tag_name((MATERIAL, "texture2dgroup")))
    {
        let id: u32 = group.attribute("id").unwrap().parse().unwrap();
        let source_uvs: Vec<[f64; 2]> = group
            .children()
            .filter(roxmltree::Node::is_element)
            .map(|uv| {
                [
                    uv.attribute("u").unwrap().parse().unwrap(),
                    uv.attribute("v").unwrap().parse().unwrap(),
                ]
            })
            .collect();
        groups.insert(id, source_uvs);
    }
    let source_triangles: Vec<_> = document
        .descendants()
        .filter(|n| n.has_tag_name((CORE, "triangle")))
        .collect();
    assert_eq!(source_triangles.len(), scene.triangles.len());
    for (index, triangle) in source_triangles.into_iter().enumerate() {
        let object = triangle
            .ancestors()
            .find(|n| n.has_tag_name((CORE, "object")))
            .unwrap();
        let pid: u32 = triangle
            .attribute("pid")
            .or_else(|| object.attribute("pid"))
            .unwrap()
            .parse()
            .unwrap();
        let source = &groups[&pid];
        let first: usize = triangle
            .attribute("p1")
            .or_else(|| object.attribute("pindex"))
            .unwrap_or("0")
            .parse()
            .unwrap();
        let indices = [
            first,
            triangle
                .attribute("p2")
                .map_or(first, |v| v.parse().unwrap()),
            triangle
                .attribute("p3")
                .map_or(first, |v| v.parse().unwrap()),
        ];
        let appearance = &scene.appearances[&index];
        let imported = appearance.texcoords.unwrap();
        for (actual, source_index) in imported.into_iter().zip(indices) {
            assert!((actual[0] - source[source_index][0]).abs() < 1e-12);
            assert!((actual[1] + source[source_index][1] - 1.).abs() < 1e-12);
        }
        assert_eq!(appearance.alpha_mode, AlphaMode::Opaque);
        assert_eq!(
            appearance.texture.as_ref().unwrap().interpolation_space,
            TextureInterpolationSpace::Srgb
        );
    }
}

#[test]
fn retained_official_png_texture_preserves_source_hash_geometry_uvs_and_pixels() {
    let path = fixture("3mf-lib3mf-png-texture.3mf");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
        "52f787357062da8bfbb14c1b5258a11513717b4beb072b00aa0161aa76a34b36"
    );
    let scene = formats::load(&path).unwrap();
    assert_geometry(&scene, 1346, [0.; 3], [100.; 3], 42_597.116_861_316_31);
    assert_eq!(scene.appearances.len(), 1346);
    assert_source_uvs(&scene, &bytes);
    let image = &scene.appearances[&0].texture.as_ref().unwrap().image;
    assert_eq!(image.dimensions(), (87, 93));
    // Pixel bytes independently decoded with Pillow from the retained PNG.
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(image.as_raw()))),
        "fe4bc33c5da98314bfc72291f3c245bf38f69f22e46802001c73a7eb22b3fd7f"
    );
    assert!(
        scene
            .appearances
            .values()
            .all(|appearance| Arc::ptr_eq(image, &appearance.texture.as_ref().unwrap().image))
    );
}

#[test]
fn retained_escaped_image_names_keep_all_six_distinct_png_textures() {
    let path = fixture("3mf-lib3mf-escaped-textures.3mf");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
        "55fe819e98ab5377a9cfeaf295f30ae8a046259a022349cde86110b8b03c095e"
    );
    let scene = formats::load(&path).unwrap();
    assert_geometry(&scene, 12, [0.; 3], [100.; 3], 60_000.);
    assert_source_uvs(&scene, &bytes);
    for (index, dimensions, hash) in [
        (
            0,
            (223, 218),
            "4f709c9db80c4ca96755e1575c701d59872b5b6ffec18bcc966c8523d76c4798",
        ),
        (
            2,
            (186, 221),
            "f406fe728c79bb2938437062b7402de88b5e4ac61a36c4ce9853c0f048211d49",
        ),
        (
            4,
            (165, 223),
            "6d6eebc12f78ed98bcaded86b9148041b75193f68b75f8bcb58cf71a20549fd3",
        ),
        (
            6,
            (187, 215),
            "53d010202b7addef04618e3ec680c6cd4d51804800c821e292e28ef94678ee6a",
        ),
        (
            8,
            (225, 225),
            "9e4fd062cbc4409fad55a506caea7422b369c8e933db01fda06329bc515a4ebf",
        ),
        (
            10,
            (246, 205),
            "9178f49f535828eebf09b2bc7ad1dbf7b8efdb23e1350d508bf8d1831e33ed5c",
        ),
    ] {
        let image = &scene.appearances[&index].texture.as_ref().unwrap().image;
        assert_eq!(image.dimensions(), dimensions);
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(image.as_raw()))),
            hash
        );
        assert!(Arc::ptr_eq(
            image,
            &scene.appearances[&(index + 1)]
                .texture
                .as_ref()
                .unwrap()
                .image
        ));
    }
}

#[test]
fn retained_two_jpeg_textures_preserve_independent_geometry_and_source_samples() {
    let path = fixture("3mf-consortium-jpeg-textures.3mf");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
        "1024c33276043c5ef25bf111496056d87bf5a816ae2f6eb6a6804d75ced02d59"
    );
    let scene = formats::load(&path).unwrap();
    assert_geometry(
        &scene,
        96_692,
        [4.946, -0.002, 0.],
        [145.05, 149.998, 99.644],
        73_300.363_826_818_68,
    );
    assert_source_uvs(&scene, &bytes);
    let mut images = HashMap::new();
    for appearance in scene.appearances.values() {
        let image = &appearance.texture.as_ref().unwrap().image;
        if let Some(previous) = images.insert(image.dimensions(), image) {
            assert!(Arc::ptr_eq(previous, image));
        }
    }
    assert_eq!(images.len(), 2);
    // Independent Pillow samples permit only the usual JPEG reconstruction rounding.
    for (dimensions, samples) in [
        (
            (1280, 640),
            [[37_u8, 28, 91, 255], [37, 28, 91, 255], [108, 104, 95, 255]],
        ),
        (
            (3100, 3100),
            [[33_u8, 33, 31, 255], [30, 25, 19, 255], [12, 19, 11, 255]],
        ),
    ] {
        let image = images[&dimensions];
        for (position, expected) in [
            (0, 0),
            (dimensions.0 / 2, dimensions.1 / 2),
            (dimensions.0 - 1, dimensions.1 - 1),
        ]
        .into_iter()
        .zip(samples)
        {
            for (actual, expected) in image
                .get_pixel(position.0, position.1)
                .0
                .into_iter()
                .zip(expected)
            {
                assert!(actual.abs_diff(expected) <= 2);
            }
        }
    }
}

#[test]
fn source_uv_origin_defaults_and_direct_alpha_are_preserved() {
    let relation = relationship(TEXTURE_RELATIONSHIP, "Textures/image.png", "");
    let png = quadrant_png();
    let model = textured_model("filter=\"nearest\"", COORDINATES, "", TRIANGLE);
    let scene = authored(&model, Some(&relation), &png).unwrap();
    let appearance = &scene.appearances[&0];
    let uv = appearance.texcoords.unwrap();
    for (actual, expected) in uv.into_iter().zip([[0., 1.], [1., 1.], [0., 0.]]) {
        assert!((actual[0] - expected[0]).abs() < 1e-12);
        assert!((actual[1] - expected[1]).abs() < 1e-12);
    }
    let image = &appearance.texture.as_ref().unwrap().image;
    assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 0]);
    assert_eq!(image.get_pixel(0, 1).0, [0, 0, 255, 128]);
    assert_eq!(appearance.alpha_mode, AlphaMode::Opaque);
    let default_model = textured_model(
        "filter=\"nearest\"",
        COORDINATES,
        "pid=\"3\" pindex=\"2\"",
        "<triangle v1=\"0\" v2=\"1\" v3=\"2\"/>",
    );
    let mut default_scene = authored(&default_model, Some(&relation), &png).unwrap();
    for corner in default_scene.appearances[&0].texcoords.unwrap() {
        assert!(corner.into_iter().all(|value| value.abs() < 1e-12));
    }
    default_scene.appearances.get_mut(&0).unwrap().unlit = true;
    let rendered = render::render(
        &default_scene,
        &render::RenderOptions {
            width: 32,
            height: 32,
            transparent: true,
            supersampling: 1,
            ..render::RenderOptions::default()
        },
    )
    .unwrap();
    // Default UV selects the red texel with source alpha zero, which must still
    // cover the model as an opaque direct material. No transparent holes arise.
    assert!(
        rendered
            .pixels()
            .filter(|p| p[3] != 0)
            .all(|p| p.0 == [255, 0, 0, 255])
    );
    assert!(rendered.pixels().filter(|p| p[3] == 255).count() > 20);
}

#[test]
fn direct_texture_sampler_controls_and_unbounded_finite_uvs_are_imported() {
    let relation = relationship(TEXTURE_RELATIONSHIP, "/3D/Textures/image.png", "");
    let png = quadrant_png();
    for (tile, wrapping) in [
        ("wrap", TextureWrap::Repeat),
        ("mirror", TextureWrap::MirroredRepeat),
        ("clamp", TextureWrap::ClampToEdge),
        ("none", TextureWrap::ClampToEdge),
    ] {
        for (filter, expected) in [
            ("nearest", TextureFilter::Nearest),
            ("linear", TextureFilter::Linear),
            ("auto", TextureFilter::Linear),
        ] {
            let coordinates = "<m:tex2coord u=\"-2.5\" v=\"3.75\"/>";
            let model = textured_model(
                &format!("tilestyleu=\"{tile}\" tilestylev=\"{tile}\" filter=\"{filter}\""),
                coordinates,
                "pid=\"3\"",
                "<triangle v1=\"0\" v2=\"1\" v3=\"2\"/>",
            );
            let scene = authored(&model, Some(&relation), &png).unwrap();
            let appearance = &scene.appearances[&0];
            for uv in appearance.texcoords.unwrap() {
                assert!((uv[0] + 2.5).abs() < 1e-12);
                assert!((uv[1] + 2.75).abs() < 1e-12);
            }
            let texture = appearance.texture.as_ref().unwrap();
            assert_eq!(texture.wrap_s, wrapping);
            assert_eq!(texture.wrap_t, wrapping);
            assert_eq!(texture.filter, expected);
            assert_eq!(texture.min_filter, expected);
        }
    }
}

#[test]
fn texture_relationships_mime_namespaces_and_references_fail_explicitly() {
    let png = quadrant_png();
    let model = textured_model("", COORDINATES, "", TRIANGLE);
    let valid = relationship(TEXTURE_RELATIONSHIP, "Textures/image.png", "");
    assert!(authored(&model, None, &png).is_err());
    for relationships in [
        relationship(MODEL_RELATIONSHIP, "Textures/image.png", ""),
        relationship(TEXTURE_RELATIONSHIP, "Textures/absent.png", ""),
        relationship(
            TEXTURE_RELATIONSHIP,
            "https://example.invalid/image.png",
            "TargetMode=\"External\"",
        ),
        relationship(TEXTURE_RELATIONSHIP, "../../image.png", ""),
    ] {
        assert!(authored(&model, Some(&relationships), &png).is_err());
    }
    for malformed in [
        model.replace("image/png", "image/jpeg"),
        model.replace("image/png", "image/webp"),
        model.replace(MATERIAL, "urn:unknown"),
        model.replace("<m:texture2d ", "<texture2d "),
        model.replace("texid=\"2\"", "texid=\"99\""),
        model.replace("p3=\"2\"", "p3=\"99\""),
        model.replace("u=\"0\"", "u=\"NaN\""),
        model.replace("v=\"0\"", "v=\"inf\""),
        model.replace("/3D/Textures/image.png", "/3D/%2e%2e/image.png"),
        model.replace(
            "<m:texture2dgroup ",
            "<m:texture2dgroup displaypropertiesid=\"7\" ",
        ),
    ] {
        assert!(authored(&malformed, Some(&valid), &png).is_err());
    }
    for sampler in [
        "filter=\"cubic\"",
        "tilestyleu=\"invalid\"",
        "tilestylev=\"invalid\"",
    ] {
        assert!(
            authored(
                &textured_model(sampler, COORDINATES, "", TRIANGLE),
                Some(&valid),
                &png
            )
            .is_err()
        );
    }
    assert!(authored(&model, Some(&valid), b"not an image").is_err());
}

#[test]
fn multipart_images_are_shared_but_each_owning_part_requires_its_relationship() {
    let png = quadrant_png();
    let child = textured_model("filter=\"nearest\"", COORDINATES, "", TRIANGLE)
        .replace("<model ", "<model unit=\"inch\" ");
    let local = textured_model("", COORDINATES, "", TRIANGLE);
    let root = local.replace("<model ", &format!("<model xmlns:p=\"{PRODUCTION}\" ")).replace("</resources>", "<object id=\"8\"><components><component p:path=\"/3D/Objects/child.model\" objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 10 0 0\"/></components></object></resources>").replace("</build>", "<item objectid=\"8\"/><item objectid=\"8\" transform=\"1 0 0 0 1 0 0 0 1 0 20 0\"/></build>");
    let root_rel = format!(
        "<Relationships xmlns=\"{OPC}\"><Relationship Id=\"image\" Type=\"{TEXTURE_RELATIONSHIP}\" Target=\"Textures/image.png\"/><Relationship Id=\"model\" Type=\"{MODEL_RELATIONSHIP}\" Target=\"Objects/child.model\"/></Relationships>"
    );
    let child_rel = relationship(TEXTURE_RELATIONSHIP, "../Textures/image.png", "");
    let initial = relationship(MODEL_RELATIONSHIP, "/3D/main.model", "");
    let entries = [
        ("_rels/.rels", initial.as_bytes()),
        ("3D/main.model", root.as_bytes()),
        ("3D/Objects/child.model", child.as_bytes()),
        ("3D/_rels/main.model.rels", root_rel.as_bytes()),
        ("3D/Objects/_rels/child.model.rels", child_rel.as_bytes()),
        ("3D/Textures/image.png", png.as_slice()),
    ];
    let scene = with_package(&entries, formats::load).unwrap();
    assert_eq!(scene.triangles.len(), 3);
    assert_eq!(
        scene.appearances[&0].texture.as_ref().unwrap().filter,
        TextureFilter::Linear
    );
    assert_eq!(
        scene.appearances[&1].texture.as_ref().unwrap().filter,
        TextureFilter::Nearest
    );
    let image = &scene.appearances[&0].texture.as_ref().unwrap().image;
    assert!(
        scene
            .appearances
            .values()
            .all(|appearance| Arc::ptr_eq(image, &appearance.texture.as_ref().unwrap().image))
    );
    let expected = [
        Vec3::new(10., 0., 0.),
        Vec3::new(111.6, 0., 0.),
        Vec3::new(10., 101.6, 0.),
    ];
    for (actual, expected) in scene.triangles[1].vertices.into_iter().zip(expected) {
        assert!((actual - expected).length() < 1e-9);
    }
    assert!((scene.triangles[2].vertices[0] - Vec3::new(10., 20., 0.)).length() < 1e-9);
    let missing: Vec<_> = entries
        .into_iter()
        .filter(|(name, _)| !name.contains("Objects/_rels"))
        .collect();
    assert!(with_package(&missing, formats::load).is_err());
}

#[test]
fn unqualified_original_composite_materials_remain_rejected() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/threemf-materials/lib3mf-composite-materials.3mf");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
        "0c6aebf96558689a3ea8d40d5f62ea03278b89178ff97dce6cd25a96b180af36"
    );
    let error = formats::load(&path).unwrap_err().to_string();
    assert!(error.contains("composite"));
}

/// Create a valid PNG header with a tiny payload, so oversized images must fail predecode.
fn oversized_header(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    drop(encoder.write_header().unwrap());
    // Both writers produce an eight-byte signature and 25-byte IHDR chunk.
    // The smaller source's remaining IDAT/IEND chunks are intentionally copied
    // without allocating pixels for the declared oversized dimensions.
    bytes.truncate(33);
    bytes.extend_from_slice(&quadrant_png()[33..]);
    bytes
}

#[test]
fn oversized_texture_axes_and_pixel_counts_fail_before_pixel_decoding() {
    let model = textured_model("", COORDINATES, "", TRIANGLE);
    let relation = relationship(TEXTURE_RELATIONSHIP, "Textures/image.png", "");
    for (width, height) in [(8193, 1), (1, 8193), (8192, 2049)] {
        let error = authored(&model, Some(&relation), &oversized_header(width, height))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("dimension") || error.contains("pixel budget"),
            "unexpected postdecode error: {error}"
        );
    }
}
