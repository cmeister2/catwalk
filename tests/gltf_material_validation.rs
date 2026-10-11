//! Public-loader validation of glTF material carriers, UV encodings and resource limits.

use base64::Engine;
use catwalk3d::{Result, formats, scene::Scene};
use image::{
    ExtendedColorType, ImageEncoder, Rgba, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
const PIXELS: [u8; 16] = [
    255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 255, 0,
];

fn png() -> Vec<u8> {
    let mut output = Vec::new();
    PngEncoder::new(&mut output)
        .write_image(&PIXELS, 2, 2, ExtendedColorType::Rgba8)
        .unwrap();
    output
}

fn jpeg() -> Vec<u8> {
    let mut output = Vec::new();
    JpegEncoder::new_with_quality(&mut output, 100)
        .encode(&[32u8, 64, 96].repeat(64), 8, 8, ExtendedColorType::Rgb8)
        .unwrap();
    output
}

struct Fixture {
    directory: PathBuf,
    document: Value,
    bytes: Vec<u8>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Fixture {
    fn new() -> Self {
        let directory = loop {
            let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "catwalk-material-validation-{}-{id}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => panic!("cannot create fixture directory: {error}"),
            }
        };
        std::fs::write(directory.join("color.png"), png()).unwrap();
        let mut fixture = Self {
            directory,
            document: json!({
                "asset":{"version":"2.0"},
                "bufferViews":[],"accessors":[],
                "meshes":[{"primitives":[{"attributes":{},"material":0}]}],
                "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0,
                "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}},"doubleSided":true}],
                "images":[{"uri":"color.png"}],"textures":[{"source":0,"sampler":0}],
                "samplers":[{"magFilter":9728,"minFilter":9728,"wrapS":33071,"wrapT":33648}]
            }),
            bytes: Vec::new(),
        };
        let positions = fixture.floats("VEC3", 3, &[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        fixture.document["accessors"][positions]["min"] = json!([0, 0, 0]);
        fixture.document["accessors"][positions]["max"] = json!([1, 1, 0]);
        let uvs = fixture.floats("VEC2", 3, &[0., 0., 1., 0., 0., 1.]);
        fixture.document["meshes"][0]["primitives"][0]["attributes"] =
            json!({"POSITION":positions,"TEXCOORD_0":uvs});
        fixture
    }

    fn view(&mut self, bytes: &[u8]) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let views = self.document["bufferViews"].as_array_mut().unwrap();
        let index = views.len();
        views.push(json!({"buffer":0,"byteOffset":self.bytes.len(),"byteLength":bytes.len()}));
        self.bytes.extend(bytes);
        index
    }

    fn accessor(
        &mut self,
        dimensions: &str,
        component_type: u32,
        count: usize,
        normalized: bool,
        bytes: &[u8],
    ) -> usize {
        let view = self.view(bytes);
        let accessors = self.document["accessors"].as_array_mut().unwrap();
        let index = accessors.len();
        accessors.push(json!({"bufferView":view,"componentType":component_type,"count":count,"type":dimensions,"normalized":normalized}));
        index
    }

    fn floats(&mut self, dimensions: &str, count: usize, values: &[f32]) -> usize {
        let bytes = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        self.accessor(dimensions, 5126, count, false, &bytes)
    }

    fn extension(&mut self, name: &str) {
        self.document["extensionsUsed"] = json!([name]);
        self.document["extensionsRequired"] = json!([name]);
    }

    fn load(&self) -> Result<Scene> {
        let mut document = self.document.clone();
        document["buffers"] = json!([{"byteLength":self.bytes.len(),"uri":format!("data:application/octet-stream;base64,{}",base64::engine::general_purpose::STANDARD.encode(&self.bytes))}]);
        let path = self.directory.join("model.gltf");
        std::fs::write(&path, serde_json::to_vec(&document)?)?;
        formats::load(&path)
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "The fixed test buffers fit GLB 32-bit chunk lengths"
    )]
    fn load_glb(&self) -> Result<Scene> {
        let mut document = self.document.clone();
        document["buffers"] = json!([{"byteLength":self.bytes.len()}]);
        let mut json = serde_json::to_vec(&document)?;
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut binary = self.bytes.clone();
        while !binary.len().is_multiple_of(4) {
            binary.push(0);
        }
        let size = 12 + 8 + json.len() + 8 + binary.len();
        let mut glb = Vec::with_capacity(size);
        glb.extend_from_slice(b"glTF");
        glb.extend(2u32.to_le_bytes());
        glb.extend((size as u32).to_le_bytes());
        glb.extend((json.len() as u32).to_le_bytes());
        glb.extend(0x4e4f_534a_u32.to_le_bytes());
        glb.extend(json);
        glb.extend((binary.len() as u32).to_le_bytes());
        glb.extend(0x004e_4942_u32.to_le_bytes());
        glb.extend(binary);
        let path = self.directory.join("model.glb");
        std::fs::write(&path, glb)?;
        formats::load(&path)
    }

    fn error(&self, expected: &str) {
        let error = self.load().unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error:?}"
        );
    }
}

fn uv(scene: &Scene) -> [[f64; 2]; 3] {
    scene.appearances[&0].texcoords.unwrap()
}

fn near_uv(actual: [[f64; 2]; 3], expected: [[f64; 2]; 3]) {
    for (actual, expected) in actual
        .into_iter()
        .flatten()
        .zip(expected.into_iter().flatten())
    {
        assert!(
            (actual - expected).abs() < 1e-6,
            "UV {actual} != {expected}"
        );
    }
}

#[test]
fn png_and_jpeg_external_data_uri_and_glb_view_images_preserve_pixels() {
    for (extension, mime, bytes) in [("png", "image/png", png()), ("jpg", "image/jpeg", jpeg())] {
        for carrier in ["external", "data", "view"] {
            let mut fixture = Fixture::new();
            fixture.document["images"][0] = match carrier {
                "external" => {
                    std::fs::write(fixture.directory.join(format!("image.{extension}")), &bytes)
                        .unwrap();
                    json!({"uri":format!("image.{extension}"),"mimeType":mime})
                }
                "data" => {
                    json!({"uri":format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(&bytes)),"mimeType":mime})
                }
                _ => {
                    let view = fixture.view(&bytes);
                    json!({"bufferView":view,"mimeType":mime})
                }
            };
            let scene = if carrier == "view" {
                fixture.load_glb()
            } else {
                fixture.load()
            }
            .unwrap();
            let texture = scene.appearances[&0].texture.as_ref().unwrap();
            if extension == "png" {
                assert_eq!(texture.image.dimensions(), (2, 2));
                assert_eq!(texture.image.as_raw(), &PIXELS);
                assert_eq!(texture.image.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
                assert_eq!(texture.image.get_pixel(0, 1), &Rgba([0, 0, 255, 64]));
            } else {
                assert_eq!(texture.image.dimensions(), (8, 8));
                for pixel in texture.image.pixels() {
                    for (&actual, expected) in pixel.0[..3].iter().zip([32, 64, 96]) {
                        assert!(actual.abs_diff(expected) <= 3);
                    }
                    assert_eq!(pixel[3], 255);
                }
            }
            assert_eq!(texture.wrap_s, catwalk3d::scene::TextureWrap::ClampToEdge);
            assert_eq!(
                texture.wrap_t,
                catwalk3d::scene::TextureWrap::MirroredRepeat
            );
            assert_eq!(texture.filter, catwalk3d::scene::TextureFilter::Nearest);
        }
    }
}

#[test]
fn materials_sharing_an_image_reuse_one_decoded_texture() {
    let mut fixture = Fixture::new();
    let mut second = fixture.document["materials"][0].clone();
    second["pbrMetallicRoughness"]["baseColorFactor"] = json!([0.5, 1, 0.25, 1]);
    fixture.document["materials"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let mut second = fixture.document["meshes"][0]["primitives"][0].clone();
    second["material"] = json!(1);
    fixture.document["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let scene = fixture.load().unwrap();
    assert_eq!(scene.triangles.len(), 2);
    let first = scene.appearances[&0].texture.as_ref().unwrap();
    let second = scene.appearances[&1].texture.as_ref().unwrap();
    assert!(Arc::ptr_eq(&first.image, &second.image));
    assert_eq!(scene.appearances[&1].base_color, [0.5, 1., 0.25, 1.]);
}

#[test]
fn escaped_local_image_names_work_and_paths_cannot_escape_the_model_directory() {
    let mut fixture = Fixture::new();
    std::fs::create_dir(fixture.directory.join("textures")).unwrap();
    std::fs::write(fixture.directory.join("textures/tile image%.png"), png()).unwrap();
    fixture.document["images"][0]["uri"] = json!("textures/tile%20image%25.png");
    assert_eq!(
        fixture.load().unwrap().appearances[&0]
            .texture
            .as_ref()
            .unwrap()
            .image
            .as_raw(),
        &PIXELS
    );
    for uri in ["../color.png", "%2e%2e/color.png"] {
        fixture.document["images"][0]["uri"] = json!(uri);
        fixture.error("model directory");
    }
    // Windows distinguishes root-relative paths from absolute drive paths.
    // Either kind must fail before an external image can enter the scene.
    fixture.document["images"][0]["uri"] = json!("%2Ftmp/color.png");
    assert!(fixture.load().is_err());
    for uri in [
        "https://example.invalid/image.png",
        "color.png?query=1",
        "color.png%00extra",
        "textures%5Ctile.png",
    ] {
        fixture.document["images"][0]["uri"] = json!(uri);
        fixture.error("relative local path");
    }
    fixture.document["images"][0]["uri"] = json!("color.png%");
    fixture.error("percent escape");
}

#[cfg(unix)]
#[test]
fn symlinked_images_outside_the_model_directory_are_rejected() {
    let mut fixture = Fixture::new();
    let outside = Fixture::new();
    std::os::unix::fs::symlink(
        outside.directory.join("color.png"),
        fixture.directory.join("escape.png"),
    )
    .unwrap();
    fixture.document["images"][0]["uri"] = json!("escape.png");
    fixture.error("outside the model directory");
}

#[test]
fn image_mime_declarations_and_data_uri_encoding_are_validated() {
    let mut fixture = Fixture::new();
    fixture.document["images"][0]["mimeType"] = json!("image/jpeg");
    fixture.error("MIME type");
    fixture.document["images"][0] = json!({"uri":format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(png())),"mimeType":"image/jpeg"});
    fixture.error("differs from its data URI");
    fixture.document["images"][0] = json!({"uri":"data:image/png,raw"});
    fixture.error("base64");
    fixture.document["images"][0] = json!({"uri":format!("data:image/webp;base64,{}",base64::engine::general_purpose::STANDARD.encode(png()))});
    fixture.error("PNG or JPEG");
    let view = fixture.view(&png());
    fixture.document["images"][0] = json!({"bufferView":view,"mimeType":"image/jpeg"});
    assert!(
        fixture
            .load_glb()
            .unwrap_err()
            .to_string()
            .contains("MIME type")
    );
    fixture.document["images"][0]["mimeType"] = json!("image/png");
    fixture.document["bufferViews"][view]["byteStride"] = json!(4);
    assert!(
        fixture
            .load_glb()
            .unwrap_err()
            .to_string()
            .contains("image view")
    );
}

#[test]
fn required_texture_transform_shape_fields_and_selected_uv_are_validated() {
    let mut fixture = Fixture::new();
    fixture.extension("KHR_texture_transform");
    for value in [Value::Null, json!(3), json!([0, 1])] {
        fixture.document["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
            json!({"KHR_texture_transform":value});
        fixture.error("must be an object");
    }
    for (value, message) in [
        (json!({"offset":[0]}), "two-number arrays"),
        (json!({"scale":[1,2,3]}), "two-number arrays"),
        (json!({"rotation":"90"}), "rotation"),
        (json!({"texCoord":-1}), "unsigned integer"),
    ] {
        fixture.document["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
            json!({"KHR_texture_transform":value});
        fixture.error(message);
    }
    fixture.document["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
        json!({"KHR_texture_transform":{"texCoord":1}});
    fixture.error("missing texture coordinates");
}

#[test]
fn normal_and_selected_texture_coordinate_counts_must_match_positions() {
    let mut fixture = Fixture::new();
    let uv = fixture.floats("VEC2", 2, &[0., 0., 1., 1.]);
    fixture.document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(uv);
    fixture.error("count does not match positions");
    fixture.document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(1);
    let normal = fixture.floats("VEC3", 2, &[0., 0., 1., 0., 0., 1.]);
    fixture.document["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
    fixture.error("count does not match positions");
}

#[test]
fn integer_uvs_normalize_only_when_declared_and_preserve_raw_negative_values() {
    for (component, normalized, bytes, expected, quantized) in [
        (
            5121,
            true,
            vec![0, 255, 64, 128, 255, 0],
            [[0., 1.], [64. / 255., 128. / 255.], [1., 0.]],
            false,
        ),
        (
            5123,
            true,
            [0u16, 65535, 16384, 32768, 65535, 0]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
            [[0., 1.], [16384. / 65535., 32768. / 65535.], [1., 0.]],
            false,
        ),
        (
            5120,
            false,
            vec![254, 3, 4, 251, 6, 7],
            [[-2., 3.], [4., -5.], [6., 7.]],
            true,
        ),
        (
            5122,
            false,
            [-300i16, 200, 400, -500, 600, 700]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect(),
            [[-300., 200.], [400., -500.], [600., 700.]],
            true,
        ),
        (
            5120,
            true,
            vec![128, 127, 192, 64, 0, 0],
            [[-1., 1.], [-64. / 127., 64. / 127.], [0., 0.]],
            true,
        ),
    ] {
        let mut fixture = Fixture::new();
        let accessor = fixture.accessor("VEC2", component, 3, normalized, &bytes);
        if component == 5120 || component == 5121 {
            // VEC2 byte vertex attributes occupy four-byte aligned elements.
            let padded = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|v| [v[0], v[1], 0, 0])
                .collect::<Vec<_>>();
            let view = fixture.view(&padded);
            fixture.document["accessors"][accessor]["bufferView"] = json!(view);
            fixture.document["bufferViews"][view]["byteStride"] = json!(4);
        }
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] =
            json!(accessor);
        if quantized {
            fixture.extension("KHR_mesh_quantization");
        }
        near_uv(uv(&fixture.load().unwrap()), expected);
        if quantized {
            fixture
                .document
                .as_object_mut()
                .unwrap()
                .remove("extensionsRequired");
            fixture.error("permitted integer components");
        }
    }
}

#[test]
fn selected_uv_morphs_precede_texture_transform_and_core_signed_deltas_need_no_extension() {
    let mut fixture = Fixture::new();
    let base = fixture.floats("VEC2", 3, &[0., 0., 1., 0., 0., 1.]);
    fixture.document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_1"] = json!(base);
    let delta = fixture.accessor(
        "VEC2",
        5120,
        3,
        true,
        &[127, 0, 0, 0, 0, 127, 0, 0, 128, 0, 0, 0],
    );
    let view = usize::try_from(
        fixture.document["accessors"][delta]["bufferView"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    fixture.document["bufferViews"][view]["byteStride"] = json!(4);
    fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"TEXCOORD_1":delta}]);
    fixture.document["meshes"][0]["weights"] = json!([0.5]);
    // Extension is used rather than required: no quantization extension exists.
    fixture.document["extensionsUsed"] = json!(["KHR_texture_transform"]);
    fixture.document["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] = json!({"KHR_texture_transform":{"texCoord":1,"offset":[2,3],"scale":[2,4],"rotation":std::f64::consts::FRAC_PI_2}});
    // (base + .5*delta), scale, rotate90, then translate.
    near_uv(
        uv(&fixture.load().unwrap()),
        [[2., 4.], [0., 5.], [-2., 2.]],
    );
    fixture.document["accessors"][delta]["count"] = json!(2);
    fixture.error("morph accessor count");
}

#[test]
fn raw_unsigned_uv_morphs_are_rejected_even_with_quantized_base_support() {
    let mut fixture = Fixture::new();
    fixture.extension("KHR_mesh_quantization");
    let delta = fixture.accessor(
        "VEC2",
        5123,
        3,
        false,
        &[1u16, 2, 3, 4, 5, 6]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"TEXCOORD_0":delta}]);
    fixture.document["meshes"][0]["weights"] = json!([1]);
    fixture.error("permitted integer components");
}

#[test]
fn expanded_instance_limits_reject_before_allocating_output_geometry() {
    for vertex_work in [false, true] {
        let mut fixture = Fixture::new();
        if vertex_work {
            let mut positions = vec![0f32; 501 * 3];
            positions[3] = 1.;
            positions[7] = 1.;
            let accessor = fixture.floats("VEC3", 501, &positions);
            fixture.document["accessors"][accessor]["min"] = json!([0, 0, 0]);
            fixture.document["accessors"][accessor]["max"] = json!([1, 1, 0]);
            fixture.document["meshes"][0]["primitives"][0]["attributes"]["POSITION"] =
                json!(accessor);
            let uvs = fixture.floats("VEC2", 501, &vec![0.; 501 * 2]);
            fixture.document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(uvs);
        }
        let indices = if vertex_work {
            vec![0u8, 1, 2]
        } else {
            [0u8, 1, 2].repeat(51)
        };
        let indices = fixture.accessor("SCALAR", 5121, indices.len(), false, &indices);
        fixture.document["meshes"][0]["primitives"][0]["indices"] = json!(indices);
        let translations = fixture.floats("VEC3", 100_000, &vec![0.; 300_000]);
        fixture.document["nodes"][0]["extensions"] =
            json!({"EXT_mesh_gpu_instancing":{"attributes":{"TRANSLATION":translations}}});
        fixture.extension("EXT_mesh_gpu_instancing");
        fixture.error(if vertex_work {
            "expanded vertex work limit"
        } else {
            "expanded geometry limit"
        });
    }
}
