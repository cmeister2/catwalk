//! Appearance and placement qualification against authored controls and
//! unchanged independently exported Khronos sample assets.
use catwalk::{
    formats,
    render::{RenderOptions, render},
    scene::{AlphaMode, Scene, TextureFilter, TextureWrap, Vec3},
};
use image::RgbaImage;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/mesh_scene")
        .join(name)
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

fn position(actual: Vec3, expected: [f64; 3]) {
    for (actual, expected) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        close(actual, expected, 2e-6);
    }
}

fn thumbnail(scene: &Scene) -> RgbaImage {
    render(
        scene,
        &RenderOptions {
            width: 192,
            height: 192,
            transparent: true,
            supersampling: 1,
            ..RenderOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn authored_external_and_buffer_view_images_preserve_colors_uvs_normals_and_placement() {
    let external = formats::load(&fixture("gltf-textured-external-cube.gltf")).unwrap();
    let embedded = formats::load(&fixture("glb-textured-buffer-view-cube.glb")).unwrap();
    assert_eq!(external.triangles.len(), 12);
    assert_eq!(external.appearances.len(), 12);
    assert_eq!(embedded.appearances.len(), 12);
    let original = image::open(fixture("gltf-authored-quadrants.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(original.dimensions(), (8, 8));
    assert_eq!(original.get_pixel(0, 0).0, [240, 55, 40, 255]);
    assert_eq!(original.get_pixel(7, 0).0, [40, 220, 65, 255]);
    assert_eq!(original.get_pixel(0, 7).0, [45, 65, 230, 255]);
    assert_eq!(original.get_pixel(7, 7).0, [245, 215, 40, 255]);
    for scene in [&external, &embedded] {
        // (-1,-1,1), scaled (1.5,.75,1.25), rotated 30 degrees around
        // glTF Z, translated (1,2,3), then converted from Y up to Z up.
        position(
            scene.triangles[0].vertices[0],
            [
                1.375 - 0.75 * 3f64.sqrt(),
                -4.25,
                1.25 - 0.375 * 3f64.sqrt(),
            ],
        );
        let appearance = &scene.appearances[&0];
        for (actual, expected) in appearance.base_color.into_iter().zip([0.8, 1., 0.85, 1.]) {
            close(actual, expected, 1e-7);
        }
        assert_eq!(appearance.texcoords, Some([[0., 1.], [1., 1.], [1., 0.]]));
        let colors = appearance.colors.unwrap();
        assert_eq!(colors[0], [1.; 4]);
        close(colors[1][0], 192. / 255., 1e-7);
        close(colors[2][1], 192. / 255., 1e-7);
        for normal in appearance.normals.unwrap() {
            position(normal, [0., -1., 0.]);
        }
        let texture = appearance.texture.as_ref().unwrap();
        assert_eq!(*texture.image, original);
        assert_eq!(texture.wrap_s, TextureWrap::ClampToEdge);
        assert_eq!(texture.wrap_t, TextureWrap::ClampToEdge);
        assert_eq!(texture.filter, TextureFilter::Nearest);
        assert_eq!(texture.min_filter, TextureFilter::Nearest);
        assert_eq!(appearance.alpha_mode, AlphaMode::Opaque);
        assert!(appearance.double_sided);
    }
    for (a, b) in external.triangles.iter().zip(&embedded.triangles) {
        for (a, b) in a.vertices.into_iter().zip(b.vertices) {
            position(a, [b.x, b.y, b.z]);
        }
    }
    assert_eq!(thumbnail(&external), thumbnail(&embedded));
}

#[test]
fn selected_uv_set_and_texture_transform_override_have_independent_values() {
    let scene = formats::load(&fixture("gltf-uv1-sampler-wrapping.gltf")).unwrap();
    assert_eq!(scene.triangles.len(), 6);
    for (index, mode) in [
        TextureWrap::Repeat,
        TextureWrap::MirroredRepeat,
        TextureWrap::ClampToEdge,
    ]
    .into_iter()
    .enumerate()
    {
        let appearance = &scene.appearances[&(2 * index)];
        let texture = appearance.texture.as_ref().unwrap();
        assert_eq!(texture.wrap_s, mode);
        assert_eq!(texture.wrap_t, mode);
        assert!(appearance.unlit);
        let uv = appearance.texcoords.unwrap();
        for (actual, expected) in uv
            .into_iter()
            .flatten()
            .zip([-0.4, 1.75, 2.4, 1.75, 2.4, -0.25])
        {
            close(actual, expected, 1e-6);
        }
    }
    let transformed = formats::load(&fixture("gltf-texture-transform-uv1.gltf")).unwrap();
    assert_eq!(transformed.triangles.len(), 2);
    let uv = transformed.appearances[&0].texcoords.unwrap();
    // offset + R(90deg) * S(-.75,.5) * UV1; info.texCoord is deliberately
    // UV0, so extension override omission produces completely different UVs.
    for (actual, expected) in uv
        .into_iter()
        .flatten()
        .zip([-0.525, 0.5, -0.525, -1.6, 0.475, -1.6])
    {
        close(actual, expected, 1e-6);
    }
}

#[test]
fn authored_smooth_normals_use_inverse_transpose_of_nonuniform_node_scale() {
    let scene = formats::load(&fixture("gltf-normal-interpolation.gltf")).unwrap();
    let normals = scene.appearances[&0].normals.unwrap();
    position(normals[0], [0., -1., 0.]);
    // Native normal (.6,0,.8), inverse scale (1/2,2,2/3), then
    // rotation 30deg around glTF Z. Rotation of the position matrix itself
    // would give a different direction and is not a valid normal transform.
    let x: f64 = 0.3;
    let z: f64 = 0.8 / 1.5;
    let length = x.hypot(z);
    position(
        normals[1],
        [
            x * 3f64.sqrt() / (2. * length),
            -z / length,
            x / (2. * length),
        ],
    );
    for normal in normals {
        close(normal.length(), 1., 1e-7);
    }
}

#[test]
fn alpha_mask_discard_preserves_behind_geometry_and_opaque_ignores_alpha() {
    let scene = formats::load(&fixture("gltf-alpha-mask-backed.gltf")).unwrap();
    assert_eq!(scene.triangles.len(), 4);
    assert_eq!(
        scene.appearances[&0].alpha_mode,
        AlphaMode::Mask { cutoff: 0.75 }
    );
    assert_eq!(scene.appearances[&2].alpha_mode, AlphaMode::Opaque);
    assert_eq!(scene.appearances[&2].base_color[3], 0.);
    let image = thumbnail(&scene);
    let visible: Vec<_> = image.pixels().filter(|p| p[3] == 255).collect();
    assert!(
        visible.iter().any(|p| p[2] > 150 && p[0] < 90 && p[1] < 90),
        "alpha192 blue quadrant was discarded"
    );
    assert!(
        visible
            .iter()
            .any(|p| p[0] > 150 && p[1] > 150 && p[2] < 90),
        "opaque yellow quadrant is absent"
    );
    assert!(
        visible
            .iter()
            .any(|p| p[0] > 180 && p[2] > 180 && (90..160).contains(&p[1])),
        "discarded texels wrote depth or zero-alpha opaque backing was blended away"
    );
    assert!(
        !visible.iter().any(|p| p[0] > 150 && p[1] < 90 && p[2] < 90),
        "alpha0 red texels were rendered"
    );
    assert!(
        !visible.iter().any(|p| p[1] > 150 && p[0] < 90 && p[2] < 90),
        "alpha64 green texels were rendered"
    );
}

#[test]
fn translucent_depth_composition_is_independent_of_triangle_submission_order() {
    let scene = formats::load(&fixture("glb-alpha-blend-overlaps.glb")).unwrap();
    assert_eq!(scene.triangles.len(), 6);
    assert_eq!(scene.appearances[&0].alpha_mode, AlphaMode::Blend);
    assert_eq!(scene.appearances[&2].alpha_mode, AlphaMode::Opaque);
    assert_eq!(scene.appearances[&4].alpha_mode, AlphaMode::Blend);
    let expected = thumbnail(&scene);
    let mut reordered = scene.clone();
    let count = scene.triangles.len();
    reordered.triangles.reverse();
    reordered.appearances = scene
        .appearances
        .iter()
        .map(|(i, appearance)| (count - 1 - i, appearance.clone()))
        .collect();
    assert_eq!(
        thumbnail(&reordered),
        expected,
        "BLEND follows submission order or transparent fragments suppress later opaque geometry"
    );
    assert!(
        expected.pixels().any(|p| p[3] > 0 && p[3] < 255),
        "standalone translucent regions lost output alpha"
    );
    assert!(
        expected
            .pixels()
            .any(|p| p[3] == 255 && p[0] > 100 && p[1] > 100 && p[2] > 100),
        "overlapping translucent colors were not composited over opaque geometry"
    );
}

#[test]
fn authored_gpu_instances_compose_instance_then_node_then_ancestor_trs() {
    let native = [[0., 0., 0.], [2., 0., 0.], [0., 1.6, 0.], [0.35, 0.45, 2.3]];
    let faces = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
    let scene = formats::load(&fixture("gltf-gpu-instanced-hierarchy.gltf")).unwrap();
    assert_eq!(
        scene.triangles.len(),
        12,
        "extension added or omitted the original base mesh"
    );
    for instance in 0..3 {
        for (face_index, face) in faces.into_iter().enumerate() {
            for (actual, index) in scene.triangles[instance * 4 + face_index]
                .vertices
                .into_iter()
                .zip(face)
            {
                let [x, y, z] = native[index];
                // Hand-composed ancestor Rz90*S(2,1,1), node Ry90*S(1,1,2)
                // and the three independent instance TRS definitions.
                let expected = match instance {
                    0 => [2. - y, x - 1., 4. * z + 3.],
                    1 => [1. - 0.5 * x, 2. - 1.5 * y, 4. * z + 3.],
                    _ => [4. - 0.75 * y, 1. - 1.25 * x, 7. - 2. * z],
                };
                position(actual, expected);
            }
        }
    }
    let translation = formats::load(&fixture("glb-gpu-instanced-translation.glb")).unwrap();
    assert_eq!(translation.triangles.len(), 12);
    for (instance, offset) in [-3., 0., 3.].into_iter().enumerate() {
        for (face_index, face) in faces.into_iter().enumerate() {
            for (actual, index) in translation.triangles[instance * 4 + face_index]
                .vertices
                .into_iter()
                .zip(face)
            {
                let [x, y, z] = native[index];
                position(actual, [x + offset - 1., -z - 3., y + 3.]);
            }
        }
    }
}

fn bounds(scene: &Scene) -> ([f64; 3], [f64; 3]) {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in scene.positions() {
        for (axis, coordinate) in [p.x, p.y, p.z].into_iter().enumerate() {
            low[axis] = low[axis].min(coordinate);
            high[axis] = high[axis].max(coordinate);
        }
    }
    (low, high)
}

fn area(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.vertices;
            (b - a).cross(c - a).length() * 0.5
        })
        .sum()
}

#[test]
fn unchanged_khronos_models_match_exact_hashes_and_independent_geometry_references() {
    // Independently decoded from original JSON/accessors and native node TRS
    // with Python struct/math. No importer or snapshot supplied these numbers.
    let references = [
        (
            "gltf-khronos-texture-coordinates.gltf",
            "424f373518a4451c612d952290dc3aa217de8f17008c4030e378da0d582ab1da",
            10,
            [
                -1.200_000_166_893_005_4,
                -5.255_118_367_131_217e-7,
                -1.200_000_166_893_005_4,
            ],
            [
                1.200_000_166_893_005_4,
                0.052_591_152_489_185_33,
                1.200_000_166_893_005_4,
            ],
            7.999_999_642_372_61,
        ),
        (
            "glb-khronos-texture-settings.glb",
            "44013a0602cbbdcaf703905ed47cc6b6fb49f067d7de0e1a0a568a5bbfa9d769",
            72,
            [
                -5.161_674_022_674_560_5,
                -0.100_000_299_513_34,
                -5.618_556_022_644_043,
            ],
            [
                5.161_674_022_674_560_5,
                0.150_000_408_291_816_7,
                4.453_536_987_304_687_5,
            ],
            248.605_174_349_896_88,
        ),
        (
            "glb-khronos-alpha-modes.glb",
            "37c3577d143071b42dd46e9d942b157837eb25c6340112171d7faecaa987b14e",
            61,
            [
                -4.300_000_190_734_863,
                -0.550_000_011_920_929,
                -0.100_000_023_841_857_91,
            ],
            [4.300_000_190_734_863, 0.75, 2.299_533_367_156_982_4],
            74.556_513_769_365_99,
        ),
        (
            "glb-khronos-simple-instancing.glb",
            "1c9425627481346a8c226118f7000db7cd7f80198e9f54331f8b664d16726f3f",
            1500,
            [-5. / 3., -522. / 41., -5. / 3.],
            [522. / 41., 5. / 3., 522. / 41.],
            1687.5,
        ),
    ];
    for (name, hash, triangles, low, high, expected_area) in references {
        let original = fs::read(fixture(name)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&original)), hash);
        let scene = formats::load(&fixture(name)).unwrap();
        assert_eq!(scene.triangles.len(), triangles, "{name}");
        assert!(scene.points.is_empty() && scene.lines.is_empty());
        let (actual_low, actual_high) = bounds(&scene);
        for (actual, expected) in actual_low
            .into_iter()
            .chain(actual_high)
            .zip(low.into_iter().chain(high))
        {
            close(actual, expected, 2e-6);
        }
        close(area(&scene), expected_area, expected_area * 2e-6);
        assert_eq!(fs::read(fixture(name)).unwrap(), original);
    }
    for (name, hash) in [
        (
            "TextureCoordinateTest.bin",
            "428cf36c6431d3252568295ea9bd5301813c96bb9cdd0f141de7a40a5abc9858",
        ),
        (
            "TextureCoordinateTemplate.png",
            "07728c817d1f3d0b86d1fcc4610ed5ae4cf5064cb7f245bb891e84fb95fe28c8",
        ),
    ] {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(fixture(name)).unwrap())),
            hash
        );
    }
}

fn original_document(path: &Path) -> (Value, Vec<u8>) {
    let original = fs::read(path).unwrap();
    if original.starts_with(b"glTF") {
        let container = gltf::binary::Glb::from_slice(&original).unwrap();
        (
            serde_json::from_slice(&container.json).unwrap(),
            container.bin.unwrap().into_owned(),
        )
    } else {
        let document: Value = serde_json::from_slice(&original).unwrap();
        let buffer = fs::read(
            path.parent()
                .unwrap()
                .join(document["buffers"][0]["uri"].as_str().unwrap()),
        )
        .unwrap();
        (document, buffer)
    }
}

fn original_image(document: &Value, buffer: &[u8], index: usize) -> RgbaImage {
    let image = &document["images"][index];
    if let Some(uri) = image["uri"].as_str() {
        image::open(fixture(uri)).unwrap().to_rgba8()
    } else {
        let view = &document["bufferViews"]
            [usize::try_from(image["bufferView"].as_u64().unwrap()).unwrap()];
        let offset = usize::try_from(view["byteOffset"].as_u64().unwrap_or(0)).unwrap();
        let length = usize::try_from(view["byteLength"].as_u64().unwrap()).unwrap();
        image::load_from_memory(&buffer[offset..offset + length])
            .unwrap()
            .to_rgba8()
    }
}

fn wrap(value: Option<u64>) -> TextureWrap {
    match value.unwrap_or(10497) {
        10497 => TextureWrap::Repeat,
        33071 => TextureWrap::ClampToEdge,
        33648 => TextureWrap::MirroredRepeat,
        other => panic!("unexpected upstream sampler {other}"),
    }
}

fn collect_materials(document: &Value, node: usize, output: &mut Vec<usize>) {
    let value = &document["nodes"][node];
    if let Some(mesh) = value["mesh"].as_u64() {
        for primitive in document["meshes"][usize::try_from(mesh).unwrap()]["primitives"]
            .as_array()
            .unwrap()
        {
            let count = if let Some(indices) = primitive["indices"].as_u64() {
                document["accessors"][usize::try_from(indices).unwrap()]["count"]
                    .as_u64()
                    .unwrap()
            } else {
                document["accessors"][usize::try_from(
                    primitive["attributes"]["POSITION"].as_u64().unwrap(),
                )
                .unwrap()]["count"]
                    .as_u64()
                    .unwrap()
            };
            output.extend(std::iter::repeat_n(
                usize::try_from(primitive["material"].as_u64().unwrap()).unwrap(),
                (count / 3) as usize,
            ));
        }
    }
    for child in value["children"].as_array().into_iter().flatten() {
        collect_materials(
            document,
            usize::try_from(child.as_u64().unwrap()).unwrap(),
            output,
        );
    }
}

#[test]
fn original_exported_materials_images_alpha_cutoffs_and_sidedness_are_retained() {
    for name in [
        "gltf-khronos-texture-coordinates.gltf",
        "glb-khronos-texture-settings.glb",
        "glb-khronos-alpha-modes.glb",
    ] {
        let path = fixture(name);
        let (document, buffer) = original_document(&path);
        assert!(
            document["asset"]["generator"]
                .as_str()
                .unwrap()
                .contains("GLTF")
                || document["asset"]["generator"]
                    .as_str()
                    .unwrap()
                    .contains("glTF")
        );
        let scene = formats::load(&path).unwrap();
        let mut material_ids = Vec::new();
        for root in document["scenes"]
            [usize::try_from(document["scene"].as_u64().unwrap_or(0)).unwrap()]["nodes"]
            .as_array()
            .unwrap()
        {
            collect_materials(
                &document,
                usize::try_from(root.as_u64().unwrap()).unwrap(),
                &mut material_ids,
            );
        }
        assert_eq!(material_ids.len(), scene.triangles.len());
        let mut images = BTreeMap::new();
        for (index, material_id) in material_ids.into_iter().enumerate() {
            let original = &document["materials"][material_id];
            let imported = &scene.appearances[&index];
            assert_eq!(
                imported.double_sided,
                original["doubleSided"].as_bool().unwrap_or(false),
                "{name}: material {material_id}"
            );
            let expected_mode = match original["alphaMode"].as_str().unwrap_or("OPAQUE") {
                "OPAQUE" => AlphaMode::Opaque,
                "BLEND" => AlphaMode::Blend,
                "MASK" => AlphaMode::Mask {
                    cutoff: original["alphaCutoff"].as_f64().unwrap_or(0.5),
                },
                value => panic!("unknown alpha mode {value}"),
            };
            assert_eq!(imported.alpha_mode, expected_mode);
            let pbr = &original["pbrMetallicRoughness"];
            let factor = pbr["baseColorFactor"]
                .as_array()
                .map(|factor| {
                    factor
                        .iter()
                        .map(|v| v.as_f64().unwrap())
                        .collect::<Vec<_>>()
                })
                .unwrap_or(vec![1.; 4]);
            for (actual, expected) in imported.base_color.into_iter().zip(factor) {
                close(actual, expected, 1e-7);
            }
            if let Some(texture) = pbr["baseColorTexture"]["index"].as_u64() {
                let texture = &document["textures"][usize::try_from(texture).unwrap()];
                let image_id = usize::try_from(texture["source"].as_u64().unwrap()).unwrap();
                let expected = images
                    .entry(image_id)
                    .or_insert_with(|| original_image(&document, &buffer, image_id));
                let imported_texture = imported.texture.as_ref().unwrap();
                assert_eq!(
                    &*imported_texture.image, expected,
                    "{name}: image {image_id}"
                );
                let sampler = texture["sampler"]
                    .as_u64()
                    .map(|i| &document["samplers"][usize::try_from(i).unwrap()]);
                assert_eq!(
                    imported_texture.wrap_s,
                    wrap(sampler.and_then(|s| s["wrapS"].as_u64()))
                );
                assert_eq!(
                    imported_texture.wrap_t,
                    wrap(sampler.and_then(|s| s["wrapT"].as_u64()))
                );
                assert!(imported.texcoords.is_some());
            } else {
                assert!(imported.texture.is_none());
            }
        }
        assert!(!images.is_empty());
    }
}
