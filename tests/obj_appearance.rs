//! Independent source, corner-attribute and rendered appearance qualification for OBJ.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    formats,
    render::{RenderOptions, render},
    scene::{AlphaMode, Scene, TextureFilter, TextureInterpolationSpace, TextureWrap, Vec3},
};
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

/// Original source hashes independently checked against the pinned Assimp commit.
const RETAINED: [(&str, &str); 3] = [
    (
        "regr01.obj",
        "35bff9dd9dced2282ff333be4cff907ea09679c3cded491a4ad261f3b3592cd8",
    ),
    (
        "regr01.mtl",
        "9c7301cc37f2718d882f69cabedf998b6064442b8b00c49a22fa5ac00f17fbb7",
    ),
    (
        "ASSIMP-LICENSE",
        "21195d410708b82757d3e34c3a40c427f6e59339da862e9480d4fcdc79b7a1bd",
    ),
];

/// One independently authored concave-polygon corner and its separate attributes.
struct Corner {
    /// Position in the tilted plane z=x/4+y/8.
    position: [f64; 3],
    /// Expected top-left-origin UV after the Wavefront V conversion.
    uv: [f64; 2],
    /// Authored normal stream value before renderer normalization.
    normal: [f64; 3],
    /// Quantized sRGB extension color, retained separately from linear MTL Kd.
    rgb: [u8; 3],
}

/// Corner expectations depend on source position rather than triangulation order.
const CONCAVE_CORNERS: [Corner; 6] = [
    Corner {
        position: [0., 0., 0.],
        uv: [0., 1.],
        normal: [-0.25, -0.125, 2.],
        rgb: [255, 255, 255],
    },
    Corner {
        position: [2., 0., 0.5],
        uv: [1., 1.],
        normal: [0.75, -0.125, 1.],
        rgb: [191, 255, 255],
    },
    Corner {
        position: [2., 1., 0.625],
        uv: [1., 0.5],
        normal: [0.75, -0.125, 1.],
        rgb: [255, 191, 255],
    },
    Corner {
        position: [1., 1., 0.375],
        uv: [0.5, 0.5],
        normal: [-0.25, -0.125, 2.],
        rgb: [255, 255, 191],
    },
    Corner {
        position: [1., 2., 0.5],
        uv: [0.5, 0.],
        normal: [-0.25, 0.875, 1.],
        rgb: [191, 191, 255],
    },
    Corner {
        position: [0., 2., 0.25],
        uv: [0., 0.],
        normal: [-0.25, 0.875, 1.],
        rgb: [255, 191, 191],
    },
];

/// Locate an input or dependency within the dedicated OBJ corpus directory.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/obj_appearance")
        .join(name)
}

/// Compare independent numeric expectations with a specified absolute tolerance.
fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

/// Compare a decoded position or authored direction without normalizing it.
fn vector(actual: Vec3, expected: [f64; 3]) {
    for (a, e) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        close(a, e, 1e-9);
    }
}

/// Independent sRGB transfer applied to the fixed extension RGB byte expectations.
fn linear(value: u8) -> f64 {
    let encoded = f64::from(value) / 255.;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Sum surface area and signed algebraic volume from emitted triangle geometry.
fn area_and_volume(scene: &Scene) -> (f64, f64) {
    let mut area = 0.;
    let mut volume = 0.;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        area += (b - a).cross(c - a).length() / 2.;
        volume += a.dot(b.cross(c)) / 6.;
    }
    (area, volume)
}

/// Render a small fixed-view image for attribute-sensitive comparisons.
fn thumbnail(scene: &Scene) -> RgbaImage {
    render(
        scene,
        &RenderOptions {
            width: 128,
            height: 128,
            transparent: true,
            supersampling: 1,
            ..RenderOptions::default()
        },
    )
    .unwrap()
}

/// Count differing decoded RGBA pixels, without relying on PNG encoding bytes.
fn changed_pixels(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels().zip(b.pixels()).filter(|(a, b)| a != b).count()
}

/// Isolate one congruent two-triangle card and remap its appearance keys.
fn card(scene: &Scene, first: usize) -> Scene {
    let mut selected = Scene::default();
    for index in first..first + 2 {
        selected.triangles.push(scene.triangles[index].clone());
        selected
            .appearances
            .insert(index - first, scene.appearances[&index].clone());
    }
    selected
}

#[test]
fn genuine_exporter_source_bytes_and_geometry_match_independent_expectations() {
    for (name, expected) in RETAINED {
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(bytes))),
            expected,
            "{name}"
        );
    }
    let source = fs::read(fixture("regr01.obj")).unwrap();
    assert!(source.starts_with(b"# Max2Obj Version 4.0 Mar 10th, 2001"));
    for (prefix, count) in [
        (b"v ".as_slice(), 2108),
        (b"vt ".as_slice(), 688),
        (b"vn ".as_slice(), 0),
        (b"f ".as_slice(), 2710),
        (b"g".as_slice(), 111),
        (b"usemtl ".as_slice(), 38),
    ] {
        assert_eq!(
            source
                .split(|byte| *byte == b'\n')
                .filter(|line| line.starts_with(prefix))
                .count(),
            count
        );
    }
    let legacy_name = b"usemtl Terraind\xe6k";
    assert_eq!(
        source
            .windows(legacy_name.len())
            .filter(|part| *part == legacy_name)
            .count(),
        2
    );
    assert!(std::str::from_utf8(&source).is_err());
    let material = fs::read(fixture("regr01.mtl")).unwrap();
    assert!(material.starts_with(b"# Max2Mtl Version 4.0 Mar 10th, 2001"));
    assert!(
        material
            .windows(b"newmtl Terraind\xe6k".len())
            .any(|part| part == b"newmtl Terraind\xe6k")
    );
    let scene = formats::load(&fixture("regr01.obj")).unwrap();
    assert_eq!(scene.triangles.len(), 2706);
    assert_eq!(scene.appearances.len(), 2706);
    // Independent source-index decoding fixes every emitted vertex and its winding.
    let mut geometry = Sha256::new();
    for triangle in &scene.triangles {
        for position in triangle.vertices {
            for coordinate in [position.x, position.y, position.z] {
                geometry.update(coordinate.to_le_bytes());
            }
        }
    }
    assert_eq!(
        format!("{:x}", digest_hex::Hex(geometry.finalize())),
        "ec2c17ab5f5f9daecd2963a852615bdd9ca8bd56ff5995ccf5131522b5740760"
    );
    let minimum = scene.positions().fold([f64::INFINITY; 3], |mut bounds, p| {
        for (axis, value) in [p.x, p.y, p.z].into_iter().enumerate() {
            bounds[axis] = bounds[axis].min(value);
        }
        bounds
    });
    let maximum = scene
        .positions()
        .fold([f64::NEG_INFINITY; 3], |mut bounds, p| {
            for (axis, value) in [p.x, p.y, p.z].into_iter().enumerate() {
                bounds[axis] = bounds[axis].max(value);
            }
            bounds
        });
    for (a, e) in minimum
        .into_iter()
        .zip([-194.199_508_67, -204.511_566_16, 0.])
    {
        close(a, e, 1e-9);
    }
    for (a, e) in maximum
        .into_iter()
        .zip([1_442.085_571_29, 967.615_295_41, 337.509_033_2])
    {
        close(a, e, 1e-9);
    }
    let (area, algebraic_volume) = area_and_volume(&scene);
    close(area, 9_677_888.403_887_872, 1e-6);
    close(algebraic_volume, 24_342_521.616_665_15, 1e-5);
    assert!(scene.triangles.iter().all(|t| {
        (t.vertices[1] - t.vertices[0])
            .cross(t.vertices[2] - t.vertices[0])
            .length()
            > 0.
    }));
}

#[test]
fn genuine_material_uv_and_transparency_attributes_preserve_original_streams() {
    let scene = formats::load(&fixture("regr01.obj")).unwrap();
    let first = &scene.appearances[&0];
    assert_eq!(first.base_color, [0.2, 0.2, 0.2, 1.]);
    assert!(first.texcoords.is_none());
    for (position, expected) in
        scene.triangles[0]
            .vertices
            .into_iter()
            .zip([[20., 845., 8.], [5., 0., 8.], [5., 860., 8.]])
    {
        vector(position, expected);
    }
    // Two pairs of named materials share diffuse values: Base/Sill and Ridging/Terraindæk.
    let expected_materials = [
        ([0.2, 0.2, 0.2, 1.], 108),
        ([0.5, 0.7, 0.4, 1.], 40),
        ([0.8, 0.6, 0.4, 1.], 96),
        ([0.9, 0.9, 0.7, 1.], 1336),
        ([0.8, 0.5, 0.2, 1.], 96),
        ([0.4, 0.3, 0.3, 1.], 556),
        ([0.9, 0.9, 0.6, 1.], 68),
        ([0.8, 0.7, 0.5, 1.], 56),
        ([0.5, 0., 0., 1.], 290),
        ([0.6, 0.7, 0.8, 0.5], 60),
    ];
    for (rgba, count) in expected_materials {
        assert_eq!(
            scene
                .appearances
                .values()
                .filter(|appearance| {
                    appearance.base_color.map(f64::to_bits) == rgba.map(f64::to_bits)
                })
                .count(),
            count
        );
    }
    assert_eq!(
        scene
            .appearances
            .values()
            .filter(|appearance| appearance.texcoords.is_some())
            .count(),
        668
    );
    for appearance in scene.appearances.values() {
        assert!(expected_materials.iter().any(|(rgba, _)| {
            rgba.map(f64::to_bits) == appearance.base_color.map(f64::to_bits)
        }));
        assert!(appearance.normals.is_none());
        assert!(appearance.colors.is_none());
        assert!(appearance.texture.is_none());
        assert_eq!(
            appearance.alpha_mode,
            if appearance.base_color[3].to_bits() == 0.5_f64.to_bits() {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            }
        );
    }
    let first_uv = &scene.appearances[&1496];
    assert_eq!(first_uv.base_color, [0.9, 0.9, 0.7, 1.]);
    assert_eq!(first_uv.texcoords, Some([[1., 1.], [1., 0.], [0., 0.]]));
    for (position, expected) in scene.triangles[1496].vertices.into_iter().zip([
        [171.803_710_94, 291.724_304_2, 314.793_396],
        [171.803_710_94, 609.724_304_2, 314.793_396],
        [179.803_710_94, 609.724_304_2, 314.793_396],
    ]) {
        vector(position, expected);
    }
    // This face's legacy material name must match its original MTL definition.
    assert_eq!(scene.appearances[&2232].base_color, [0.4, 0.3, 0.3, 1.]);
    let windows = &scene.appearances[&2646];
    assert_eq!(windows.base_color, [0.6, 0.7, 0.8, 0.5]);
    assert_eq!(windows.alpha_mode, AlphaMode::Blend);
    for (position, expected) in scene.triangles[2646].vertices.into_iter().zip([
        [415., 370.5, 68.],
        [555., 364.5, 68.],
        [415., 364.5, 68.],
    ]) {
        vector(position, expected);
    }
}

#[test]
fn concave_triangulation_keeps_winding_and_separate_corner_attributes() {
    let scene = formats::load(&fixture("obj-textured-concave.obj")).unwrap();
    assert_eq!(scene.triangles.len(), 4);
    assert_eq!(scene.appearances.len(), 4);
    close(area_and_volume(&scene).0, 3. * 69f64.sqrt() / 8., 1e-12);
    for (index, triangle) in scene.triangles.iter().enumerate() {
        let appearance = &scene.appearances[&index];
        for (a, e) in appearance.base_color.into_iter().zip([0.5, 0.8, 1., 1.]) {
            close(a, e, 1e-12);
        }
        let [a, b, c] = triangle.vertices;
        assert!((b - a).cross(c - a).dot(Vec3::new(-0.25, -0.125, 1.)) > 0.);
        let centroid = (a + b + c) / 3.;
        assert!(centroid.x <= 1. + 1e-12 || centroid.y <= 1. + 1e-12);
        for (corner, actual) in triangle.vertices.into_iter().enumerate() {
            let expected = CONCAVE_CORNERS
                .iter()
                .find(|source| {
                    [actual.x, actual.y, actual.z]
                        .into_iter()
                        .zip(source.position)
                        .all(|(a, e)| (a - e).abs() < 1e-12)
                })
                .unwrap();
            vector(actual, expected.position);
            for (a, e) in appearance.texcoords.unwrap()[corner]
                .into_iter()
                .zip(expected.uv)
            {
                close(a, e, 1e-12);
            }
            vector(appearance.normals.unwrap()[corner], expected.normal);
            for (a, e) in appearance.colors.unwrap()[corner][..3]
                .iter()
                .copied()
                .zip(expected.rgb.map(linear))
            {
                close(a, e, 1e-12);
            }
        }
    }
    let texture = scene.appearances[&0].texture.as_ref().unwrap();
    assert_eq!(texture.wrap_s, TextureWrap::Repeat);
    assert_eq!(texture.wrap_t, TextureWrap::Repeat);
    assert_eq!(texture.filter, TextureFilter::Linear);
    assert_eq!(
        texture.interpolation_space,
        TextureInterpolationSpace::Linear
    );
    let image = &texture.image;
    assert_eq!(image.dimensions(), (8, 8));
    for (x, y, expected) in [
        (0, 0, [240, 55, 40, 255]),
        (7, 0, [40, 220, 65, 255]),
        (0, 7, [45, 65, 230, 255]),
        (7, 7, [245, 215, 40, 255]),
    ] {
        assert_eq!(image.get_pixel(x, y).0, expected);
    }
}

#[test]
fn authored_normal_and_color_interpolation_change_decoded_thumbnail_pixels() {
    let scene = formats::load(&fixture("obj-textured-concave.obj")).unwrap();
    let expected = thumbnail(&scene);
    let mut flat = scene.clone();
    for appearance in flat.appearances.values_mut() {
        appearance.normals = None;
    }
    assert!(changed_pixels(&expected, &thumbnail(&flat)) > 100);
    let mut uncolored = scene;
    for appearance in uncolored.appearances.values_mut() {
        appearance.colors = None;
    }
    assert!(changed_pixels(&expected, &thumbnail(&uncolored)) > 100);
}

#[test]
fn diffuse_white_map_and_outside_clamp_preserve_plain_material_rendering() {
    let scene = formats::load(&fixture("obj-texture-transform-border.obj")).unwrap();
    assert_eq!(scene.triangles.len(), 8);
    let plain = card(&scene, 0);
    let white = card(&scene, 2);
    for appearance in white.appearances.values() {
        let image = &appearance.texture.as_ref().unwrap().image;
        assert_eq!(image.dimensions(), (8, 8));
        assert!(image.pixels().all(|pixel| pixel.0 == [255; 4]));
    }
    assert_eq!(thumbnail(&plain), thumbnail(&white));
    let mut outside = card(&scene, 6);
    for appearance in outside.appearances.values_mut() {
        appearance.texcoords = Some([[-2., 0.5]; 3]);
        assert_eq!(
            appearance.texture.as_ref().unwrap().wrap_s,
            TextureWrap::ClampToBorderWhite
        );
    }
    assert_eq!(thumbnail(&plain), thumbnail(&outside));
    let repeated = card(&scene, 4);
    assert!(changed_pixels(&thumbnail(&plain), &thumbnail(&repeated)) > 100);
    for (index, triangle) in scene.triangles.iter().enumerate().skip(4) {
        let offset = if index < 6 { 0. } else { 1.5 };
        let appearance = &scene.appearances[&index];
        for (a, e) in appearance.base_color.into_iter().zip([0.5, 0.4, 0.25, 1.]) {
            close(a, e, 1e-12);
        }
        for (corner, p) in triangle.vertices.into_iter().enumerate() {
            let source_u = if p.x - offset < 0.5 { -0.5 } else { 0.75 };
            let source_v = if p.y < 2. { 0. } else { 1. };
            let expected = [2. * source_u - 0.25, 1. - (0.5 * source_v + 0.125)];
            for (a, e) in appearance.texcoords.unwrap()[corner]
                .into_iter()
                .zip(expected)
            {
                close(a, e, 1e-12);
            }
        }
    }
}

#[test]
fn dissolve_and_inverse_transparency_have_distinct_depth_composited_opacity() {
    let scene = formats::load(&fixture("obj-alpha-overlap.obj")).unwrap();
    assert_eq!(scene.triangles.len(), 6);
    for (first, alpha, mode) in [
        (0, 1., AlphaMode::Opaque),
        (2, 0.5, AlphaMode::Blend),
        (4, 0.25, AlphaMode::Blend),
    ] {
        for index in first..first + 2 {
            close(scene.appearances[&index].base_color[3], alpha, 1e-12);
            assert_eq!(scene.appearances[&index].alpha_mode, mode);
        }
    }
    let expected = thumbnail(&scene);
    let mut opaque = scene;
    for appearance in opaque.appearances.values_mut() {
        appearance.alpha_mode = AlphaMode::Opaque;
    }
    assert!(changed_pixels(&expected, &thumbnail(&opaque)) > 100);
}
