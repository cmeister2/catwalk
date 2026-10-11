//! Headless CLI input, output, resource isolation and saved-preview behavior tests.

use base64::Engine;
use catwalk::{formats, render::write_png, scene::Vec3};
use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        loop {
            let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("catwalk cli {} {id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create fixture directory: {error}"),
            }
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_catwalk"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR");
    command
}

fn run(input: &Path, output: &Path, extra: &[&str]) -> Output {
    let mut command = command();
    command.arg(input).arg("--output").arg(output);
    if !extra.contains(&"--size") {
        command.args(["--size", "64"]);
    }
    command.args(extra).output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, [] as [u8; 0]);
}

fn decode(bytes: &[u8]) -> RgbaImage {
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .unwrap()
        .to_rgba8()
}

fn assert_model_thumbnail(image: &RgbaImage, width: u32, height: u32) {
    assert_eq!(image.dimensions(), (width, height));
    assert_eq!(image.get_pixel(0, 0)[3], 0);
    assert_eq!(image.get_pixel(width - 1, height - 1)[3], 0);
    let occupied = image.pixels().filter(|pixel| pixel[3] > 0).count();
    assert!(occupied > 30, "thumbnail has insufficient visible geometry");
    assert!(occupied < (width * height) as usize * 9 / 10);
}

const OBJ: &str = "v 0 0 0\nv 2 0 0\nv 0 1 1\nf 1 2 3\n";

#[test]
fn obj_and_ascii_and_binary_stl_render_without_a_display() {
    let fixtures = FixtureDirectory::new();
    let ascii_stl = "solid fixture\nfacet normal 0 -1 1\nouter loop\nvertex 0 0 0\nvertex 2 0 0\nvertex 0 1 1\nendloop\nendfacet\nendsolid fixture\n";
    let mut binary_stl = vec![0; 80];
    // A binary STL is allowed to start with 'solid'; detection must not rely on that prefix.
    binary_stl[..5].copy_from_slice(b"solid");
    binary_stl.extend_from_slice(&1_u32.to_le_bytes());
    for value in [
        0.0_f32, -1.0, 1.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 1.0,
    ] {
        binary_stl.extend_from_slice(&value.to_le_bytes());
    }
    binary_stl.extend_from_slice(&0_u16.to_le_bytes());
    for (name, contents) in [
        ("model.OBJ", OBJ.as_bytes()),
        ("ascii.stl", ascii_stl.as_bytes()),
        ("binary.stl", binary_stl.as_slice()),
    ] {
        let input = fixtures.write(name, contents);
        let destination = fixtures.path(&format!("{name}.png"));
        let output = run(
            &input,
            &destination,
            &["--transparent", "--resolution", "96x64"],
        );
        assert_success(&output);
        assert_eq!(output.stdout, [] as [u8; 0]);
        assert_model_thumbnail(&decode(&fs::read(destination).unwrap()), 96, 64);
    }
}

fn positions_buffer() -> Vec<u8> {
    [0.0_f32, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect()
}

fn gltf_document(uri: Option<&str>) -> Value {
    let mut buffer = json!({"byteLength": 36});
    if let Some(uri) = uri {
        buffer["uri"] = json!(uri);
    }
    json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [
            {"children": [1], "translation": [10, 20, 30], "scale": [2, 3, 4]},
            {"mesh": 0, "translation": [1, 2, 3], "scale": [0.5, 1, 2],
             "rotation": [0, 0, std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2]}
        ],
        "buffers": [buffer],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3,
            "type": "VEC3", "min": [0, 0, 0], "max": [2, 1, 1]}],
        "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [0.25, 0.5, 0.75, 1]}}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}]
    })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Generated test JSON and binary buffers are small and fit GLB 32-bit chunk lengths"
)]
fn glb(document: &Value, buffer: &[u8]) -> Vec<u8> {
    let mut json = serde_json::to_vec(document).unwrap();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut binary = buffer.to_vec();
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let length = 12 + 8 + json.len() + 8 + binary.len();
    let mut bytes = Vec::with_capacity(length);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&(length as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"JSON");
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"BIN\0");
    bytes.extend_from_slice(&binary);
    bytes
}

fn assert_positions(actual: &[Vec3; 3], expected: [Vec3; 3]) {
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (*actual - expected).length() < 1e-5,
            "incorrect transformed vertex: {actual:?}; expected {expected:?}"
        );
    }
}

#[test]
fn gltf_inline_external_and_glb_compose_scene_transforms_and_convert_y_up() {
    let fixtures = FixtureDirectory::new();
    let buffer = positions_buffer();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&buffer);
    let inline = gltf_document(Some(&format!(
        "data:application/octet-stream;base64,{encoded}"
    )));
    fixtures.write("geometry data.bin", &buffer);
    let external = gltf_document(Some("geometry%20data.bin"));
    let paths = [
        fixtures.write("inline.gltf", serde_json::to_vec(&inline).unwrap()),
        fixtures.write("external.gltf", serde_json::to_vec(&external).unwrap()),
        fixtures.write("binary.glb", glb(&gltf_document(None), &buffer)),
    ];
    let mut reference = None;
    for path in paths {
        let scene = formats::load(&path).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        // Child rotation/scale then translation; parent nonuniform scale then
        // translation; finally glTF Y-up becomes the renderer's Z-up.
        assert_positions(
            &scene.triangles[0].vertices,
            [
                Vec3::new(12.0, -42.0, 26.0),
                Vec3::new(12.0, -42.0, 29.0),
                Vec3::new(10.0, -50.0, 26.0),
            ],
        );
        assert_eq!(scene.triangles[0].color, Some([137, 188, 225]));
        let destination = path.with_extension("png");
        assert_success(&run(&path, &destination, &["--transparent"]));
        let thumbnail = decode(&fs::read(destination).unwrap());
        assert_model_thumbnail(&thumbnail, 64, 64);
        if let Some(reference) = &reference {
            assert_eq!(
                &thumbnail, reference,
                "buffer packaging changed the thumbnail"
            );
        } else {
            reference = Some(thumbnail);
        }
    }
}

#[test]
fn required_gltf_unlit_is_accepted_and_unknown_extensions_fail() {
    let fixtures = FixtureDirectory::new();
    let buffer = positions_buffer();
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buffer)
    );
    let mut model = gltf_document(Some(&uri));
    model["extensionsUsed"] = serde_json::json!(["KHR_materials_unlit"]);
    model["extensionsRequired"] = serde_json::json!(["KHR_materials_unlit"]);
    model["materials"][0]["extensions"] = serde_json::json!({ "KHR_materials_unlit": {} });
    let input = fixtures.write("unlit.gltf", serde_json::to_vec(&model).unwrap());
    let output = fixtures.path("unlit.png");
    assert_success(&run(&input, &output, &["--transparent"]));
    assert_model_thumbnail(&decode(&fs::read(&output).unwrap()), 64, 64);
    model["extensionsRequired"] = serde_json::json!(["CUSTOM_unsupported"]);
    let input = fixtures.write(
        "unsupported-extension.gltf",
        serde_json::to_vec(&model).unwrap(),
    );
    let output = fixtures.path("unsupported-extension.png");
    assert!(!run(&input, &output, &[]).status.success());
    assert!(!output.exists());
}

fn write_3mf(path: &Path, model: &str) {
    let file = fs::File::create(path).unwrap();
    let mut package = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        (
            "[Content_Types].xml",
            "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>",
        ),
        (
            "_rels/.rels",
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/model%20part.model\" Id=\"model\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>",
        ),
        ("3D/model part.model", model),
    ] {
        package.start_file(name, options).unwrap();
        package.write_all(data.as_bytes()).unwrap();
    }
    package.finish().unwrap();
}

#[test]
fn packaged_3mf_resolves_model_relationship_and_composes_build_components() {
    let fixtures = FixtureDirectory::new();
    let input = fixtures.path("assembly.3mf");
    write_3mf(
        &input,
        r##"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <resources>
    <basematerials id="10"><base name="green" displaycolor="#28B45A"/></basematerials>
    <object id="1" pid="10" pindex="0"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="2" y="0" z="0"/><vertex x="0" y="1" z="1"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
    </mesh></object>
    <object id="2"><components><component objectid="1" transform="0 0.5 0 -1 0 0 0 0 2 1 2 3"/></components></object>
  </resources>
  <build><item objectid="2" transform="2 0 0 0 3 0 0 0 4 10 20 30"/></build>
</model>"##,
    );
    let scene = formats::load(&input).unwrap();
    assert_eq!(scene.triangles.len(), 1);
    assert_positions(
        &scene.triangles[0].vertices,
        [
            Vec3::new(12.0, 26.0, 42.0),
            Vec3::new(12.0, 29.0, 42.0),
            Vec3::new(10.0, 26.0, 50.0),
        ],
    );
    assert_eq!(scene.triangles[0].color, Some([40, 180, 90]));
    let destination = fixtures.path("assembly.png");
    assert_success(&run(&input, &destination, &["--transparent"]));
    let image = decode(&fs::read(destination).unwrap());
    assert_model_thumbnail(&image, 64, 64);
    assert!(
        image
            .pixels()
            .filter(|pixel| pixel[3] > 0)
            .all(|pixel| pixel[1] > pixel[0] && pixel[1] > pixel[2])
    );
}

#[test]
fn png_input_preserves_aspect_and_alpha_and_composites_background() {
    let fixtures = FixtureDirectory::new();
    let input = fixtures.path("source.png");
    let source = RgbaImage::from_pixel(40, 20, Rgba([220, 40, 80, 128]));
    write_png(&source, fs::File::create(&input).unwrap()).unwrap();
    let destination = fixtures.path("transparent.png");
    assert_success(&run(
        &input,
        &destination,
        &["--transparent", "--resolution", "80,80"],
    ));
    let thumbnail = decode(&fs::read(destination).unwrap());
    assert_eq!(thumbnail.dimensions(), (80, 80));
    assert_eq!(thumbnail.get_pixel(40, 19)[3], 0);
    assert_eq!(thumbnail.get_pixel(40, 60)[3], 0);
    assert_eq!(thumbnail.get_pixel(40, 20).0, [220, 40, 80, 128]);
    assert_eq!(thumbnail.get_pixel(40, 59).0, [220, 40, 80, 128]);
    let destination = fixtures.path("opaque.png");
    assert_success(&run(&input, &destination, &["--background", "#123456"]));
    let thumbnail = decode(&fs::read(destination).unwrap());
    assert_eq!(thumbnail.get_pixel(0, 0).0, [0x12, 0x34, 0x56, 255]);
    assert_eq!(thumbnail.get_pixel(32, 32)[3], 255);
    assert!(thumbnail.get_pixel(32, 32)[0] > 0x12);
}

#[test]
fn stdout_contains_only_png_bytes_and_file_output_is_deterministic() {
    let fixtures = FixtureDirectory::new();
    let input = fixtures.write("model.obj", OBJ);
    let stdout = run(&input, Path::new("-"), &["--no-background"]);
    assert_success(&stdout);
    assert_model_thumbnail(&decode(&stdout.stdout), 64, 64);
    let destination = fixtures.path("thumbnail.png");
    let file = run(&input, &destination, &["--transparent"]);
    assert_success(&file);
    assert_eq!(file.stdout, [] as [u8; 0]);
    assert_eq!(stdout.stdout, fs::read(destination).unwrap());
}

fn assert_clean_failure(output: &Output, destination: &Path) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "failure wrote to stdout");
    assert!(!output.stderr.is_empty(), "failure had no diagnostic");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(!message.contains("panicked"), "failure panicked: {message}");
    assert!(!destination.exists(), "failure created an output file");
}

#[test]
fn malformed_missing_and_unsupported_inputs_fail_without_creating_outputs() {
    let fixtures = FixtureDirectory::new();
    let malformed_obj = fixtures.write("bad.obj", "v 0 0 0\nv 1 0 0\nf 1 2 999\n");
    let invalid_numbers = fixtures.write("nonfinite.obj", "v NaN 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
    let degenerate = fixtures.write("degenerate.obj", "v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 3\n");
    let truncated_stl = fixtures.write("bad.stl", [0; 12]);
    let malformed_gltf = fixtures.write("bad.gltf", b"{not json}");
    let malformed_png = fixtures.write("bad.png", b"not a PNG");
    let unsupported = fixtures.write("model.unsupported", b"unrecognized model format");
    let missing = fixtures.path("missing.obj");
    for (index, input) in [
        malformed_obj,
        invalid_numbers,
        degenerate,
        truncated_stl,
        malformed_gltf,
        malformed_png,
        unsupported,
        missing,
    ]
    .into_iter()
    .enumerate()
    {
        let destination = fixtures.path(&format!("failure-{index}.png"));
        assert_clean_failure(&run(&input, &destination, &[]), &destination);
    }
}

#[test]
fn invalid_dimensions_and_options_fail_before_creating_output() {
    let fixtures = FixtureDirectory::new();
    let input = fixtures.write("model.obj", OBJ);
    let cases: &[&[&str]] = &[
        &["--resolution", "invalid"],
        &["--resolution", "0x64"],
        &["--resolution", "64,0"],
        &["--resolution", "4097x64"],
        &["--resolution", "64xfoo"],
        &["--resolution", "64x64x64"],
        &["--resolution", "4096x4096", "--samples", "4"],
        &["--size", "0"],
        &["--samples", "0"],
        &["--samples", "5"],
        &["--color", "💙💙"],
        &["--background", "zzzzzz"],
    ];
    for (index, args) in cases.iter().enumerate() {
        let destination = fixtures.path(&format!("invalid-option-{index}.png"));
        assert_clean_failure(&run(&input, &destination, args), &destination);
    }
}

#[test]
fn failures_preserve_existing_output_and_input_cannot_be_overwritten() {
    let fixtures = FixtureDirectory::new();
    let input = fixtures.write("bad.obj", "not an OBJ model");
    let destination = fixtures.write("existing.png", "keep this output");
    let output = run(&input, &destination, &[]);
    assert!(!output.status.success());
    assert_eq!(output.stdout, [] as [u8; 0]);
    assert_eq!(fs::read(&destination).unwrap(), b"keep this output");
    let input = fixtures.write("model.obj", OBJ);
    let output = run(&input, &input, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("input and output must be different"));
    assert_eq!(fs::read(input).unwrap(), OBJ.as_bytes());
}

#[test]
fn no_arguments_has_clean_error_and_format_listing_is_available() {
    let no_args = command().output().unwrap();
    assert!(!no_args.status.success());
    assert_eq!(no_args.stdout, [] as [u8; 0]);
    assert!(String::from_utf8_lossy(&no_args.stderr).contains("provide an input"));
    let formats = command().arg("--list-formats").output().unwrap();
    assert_success(&formats);
    let formats = String::from_utf8(formats.stdout).unwrap();
    assert!(formats.contains("obj") && formats.contains("gltf") && formats.contains("3mf"));
    assert!(formats.contains("limitations"));
    for extension in catwalk::saved_preview::EXTENSIONS {
        assert!(formats.contains(extension));
    }
}

#[test]
fn saved_previews_use_the_thumbnail_canvas_and_png_stdout_contract() {
    let fixtures = FixtureDirectory::new();
    let saved = RgbaImage::from_pixel(4, 2, Rgba([240, 30, 60, 255]));
    let mut encoded = Vec::new();
    write_png(&saved, &mut encoded).unwrap();
    let source = base64::engine::general_purpose::STANDARD.encode(encoded);
    let input = fixtures.write(
        "wide.lbrn2",
        format!("<LightBurnProject FormatVersion=\"1\"><Thumbnail Source=\"{source}\"/></LightBurnProject>"),
    );
    let original = fs::read(&input).unwrap();
    let result = run(
        &input,
        Path::new("-"),
        &["--resolution", "80x80", "--transparent"],
    );
    assert_success(&result);
    let image = decode(&result.stdout);
    assert_eq!(image.dimensions(), (80, 80));
    assert_eq!(*image.get_pixel(40, 10), Rgba([245, 247, 250, 0]));
    assert_eq!(*image.get_pixel(40, 40), Rgba([240, 30, 60, 255]));
    let output = fixtures.path("opaque.png");
    assert_success(&run(
        &input,
        &output,
        &["--resolution", "80x80", "--background", "112233"],
    ));
    let opaque = decode(&fs::read(output).unwrap());
    assert_eq!(*opaque.get_pixel(40, 10), Rgba([17, 34, 51, 255]));
    assert_eq!(*opaque.get_pixel(40, 40), Rgba([240, 30, 60, 255]));
    let extracted = command()
        .arg(&input)
        .args(["--extract-saved-preview", "--output", "-"])
        .output()
        .unwrap();
    assert_success(&extracted);
    assert_eq!(decode(&extracted.stdout), saved);
    assert_eq!(fs::read(input).unwrap(), original);
}

#[test]
fn absent_saved_preview_has_a_distinct_exit_status_and_preserves_old_output() {
    let fixtures = FixtureDirectory::new();
    let input = fixtures.write("absent.lbrn2", "<LightBurnProject FormatVersion=\"1\"/>");
    let destination = fixtures.write("existing.png", "keep previous usable preview");
    let result = run(&input, &destination, &[]);
    assert_eq!(result.status.code(), Some(3));
    assert_eq!(result.stdout, [] as [u8; 0]);
    assert!(String::from_utf8_lossy(&result.stderr).contains("no supported saved preview"));
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"keep previous usable preview"
    );
    let malformed = fixtures.write(
        "corrupt.lbrn2",
        "<LightBurnProject FormatVersion=\"1\"><Thumbnail Source=\"not base64!\"/></LightBurnProject>",
    );
    let result = run(&malformed, &destination, &[]);
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(result.stdout, [] as [u8; 0]);
    assert_eq!(
        fs::read(destination).unwrap(),
        b"keep previous usable preview"
    );
}
