//! Original TIFF voxel facts, literal orientation geometry and bounded dispatch controls.

#[path = "support/tiff_fixture.rs"]
/// Build small authored carriers independently of the production directory reader.
mod carrier;
#[path = "support/digest_hex.rs"]
/// Format immutable original source and notice checksums.
mod digest_hex;

use carrier::{Frame, Layout};
use catwalk3d::{
    formats, geometry_thumbnail,
    render::{RenderOptions, render},
    scene::{Scene, Vec3},
    thumbnail,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use tiff::encoder::{Compression, TiffEncoder, colortype, compression::DeflateLevel};

/// Unique suffix for independent malformed-input files.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary input directory removed when its owner leaves scope.
struct Input {
    /// Directory containing this test's immutable TIFF file.
    directory: PathBuf,
    /// Input path with a supported TIFF extension.
    path: PathBuf,
}

impl Input {
    /// Store one test input with a process-unique directory and the chosen extension.
    fn new(bytes: &[u8], extension: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "catwalk-tiff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("input.{extension}"));
        fs::write(&path, bytes).unwrap();
        Self { directory, path }
    }
}

impl Drop for Input {
    /// Remove temporary files without hiding a test panic with a cleanup panic.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Create two nonadjacent lit voxels at explicitly different corners and depths.
fn two_voxels() -> Vec<Frame> {
    vec![
        Frame::gray8(3, 2, vec![200, 0, 0, 0, 0, 0]),
        Frame::gray8(3, 2, vec![0, 0, 0, 0, 0, 200]),
    ]
}

/// Require a scalar or coordinate to match its independent literal reference.
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

/// Compute Euclidean surface area and signed enclosed volume from emitted triangles.
fn area_volume(scene: &Scene) -> [f64; 2] {
    scene
        .triangles
        .iter()
        .fold([0., 0.], |[area, volume], triangle| {
            let [a, b, c] = triangle.vertices;
            [
                area + (b - a).cross(c - a).length() / 2.,
                volume + a.dot(b.cross(c)) / 6.,
            ]
        })
}

/// Accumulate independent world-space bounds over the emitted positions.
fn bounds(scene: &Scene) -> [Vec3; 2] {
    scene.positions().fold(
        [
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        ],
        |[low, high], p| {
            [
                Vec3::new(low.x.min(p.x), low.y.min(p.y), low.z.min(p.z)),
                Vec3::new(high.x.max(p.x), high.y.max(p.y), high.z.max(p.z)),
            ]
        },
    )
}

/// Verify the exact two box corner sets and every outward-facing triangle.
fn boxes(scene: &Scene, centers: [[f64; 3]; 2], half: [f64; 3]) {
    assert_eq!(scene.triangles.len(), 24);
    let centers = centers.map(|p| Vec3::new(p[0], p[1], p[2]));
    let mut expected = Vec::new();
    for center in centers {
        for x in [-half[0], half[0]] {
            for y in [-half[1], half[1]] {
                for z in [-half[2], half[2]] {
                    expected.push(center + Vec3::new(x, y, z));
                }
            }
        }
    }
    let mut seen = [false; 16];
    for triangle in &scene.triangles {
        for vertex in triangle.vertices {
            let index = expected
                .iter()
                .position(|p| (*p - vertex).length() < 1e-9)
                .expect("every corner must belong to a literal occupied voxel");
            seen[index] = true;
        }
        let centroid = (triangle.vertices[0] + triangle.vertices[1] + triangle.vertices[2]) / 3.;
        let center = centers
            .into_iter()
            .min_by(|a, b| {
                (*a - centroid)
                    .length()
                    .total_cmp(&(*b - centroid).length())
            })
            .unwrap();
        let normal = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0]);
        assert!(
            normal.dot(centroid - center) > 0.,
            "TIFF orientation changed outward winding"
        );
    }
    assert!(seen.into_iter().all(|v| v));
    let [area, volume] = area_volume(scene);
    near(
        area,
        16. * (half[0] * half[1] + half[0] * half[2] + half[1] * half[2]),
    );
    near(volume, 16. * half[0] * half[1] * half[2]);
}

#[test]
fn original_libtiff_pages_match_all_voxels_and_independent_surface_facts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/tiff_volumes");
    let path = root.join("seq-1c-8b-multipage-adeefdcc.tiff");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 1262);
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
        "f9dd5eb92e193f121b2afcb3575d2be5566adb6198afcb5009516a77972d6428"
    );
    let notice = fs::read(root.join("IMAGE-TIFF-LICENSE")).unwrap();
    assert_eq!(notice.len(), 1073);
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&notice))),
        "20c097d0de27e2adc179140a3d542091951a030699c7e1c78655f2a2d08d4e3e"
    );
    for (z, offset) in [8, 426, 844].into_iter().enumerate() {
        for (index, &sample) in bytes[offset..offset + 256].iter().enumerate() {
            assert_eq!(usize::from(sample), (index + 50 * z) % 256);
        }
    }
    let scene = formats::load(&path).unwrap();
    assert_eq!(scene.triangles.len(), 1208);
    let [area, volume] = area_volume(&scene);
    near(area, 604.);
    near(volume, 384.);
    for (actual, expected) in bounds(&scene)
        .into_iter()
        .zip([Vec3::new(-0.5, -0.5, -0.5), Vec3::new(15.5, 14.5, 2.5)])
    {
        near(actual.x, expected.x);
        near(actual.y, expected.y);
        near(actual.z, expected.z);
    }
}

#[test]
fn all_eight_orientations_and_both_byte_orders_preserve_literal_voxels() {
    let centers = [
        [[0., 1., 0.], [2., 0., 1.]],
        [[2., 1., 0.], [0., 0., 1.]],
        [[2., 0., 0.], [0., 1., 1.]],
        [[0., 0., 0.], [2., 1., 1.]],
        [[0., 2., 0.], [1., 0., 1.]],
        [[1., 2., 0.], [0., 0., 1.]],
        [[1., 0., 0.], [0., 2., 1.]],
        [[0., 0., 0.], [1., 2., 1.]],
    ];
    for big in [false, true] {
        for big_endian in [false, true] {
            for (orientation, expected) in (1..=8).zip(centers) {
                let mut frames = two_voxels();
                for frame in &mut frames {
                    frame.orientation = orientation;
                }
                let input = Input::new(
                    &carrier::encode(&frames, Layout { big, big_endian }).unwrap(),
                    "tiff",
                );
                boxes(&formats::load(&input.path).unwrap(), expected, [0.5; 3]);
            }
        }
    }
}

#[test]
fn rational_physical_spacing_transposes_with_orientation_and_sets_stack_depth() {
    for unit in [2, 3] {
        let mut frames = two_voxels();
        for frame in &mut frames {
            frame.orientation = 6;
            frame.resolution_unit = unit;
            frame.x_resolution = if unit == 2 { [127, 10] } else { [5, 1] };
            frame.y_resolution = if unit == 2 { [127, 15] } else { [10, 3] };
        }
        let input = Input::new(&carrier::encode(&frames, Layout::default()).unwrap(), "tif");
        boxes(
            &formats::load(&input.path).unwrap(),
            [[3., 4., 0.], [0., 0., 2.]],
            [1.5, 1., 1.],
        );
    }
}

#[test]
fn every_supported_scalar_width_preserves_the_literal_occupied_voxels() {
    let representations = [
        (8, 1, vec![0_u8], vec![200_u8]),
        (
            8,
            2,
            (-40_i8).to_le_bytes().to_vec(),
            80_i8.to_le_bytes().to_vec(),
        ),
        (
            16,
            1,
            0_u16.to_le_bytes().to_vec(),
            200_u16.to_le_bytes().to_vec(),
        ),
        (
            16,
            2,
            (-40_i16).to_le_bytes().to_vec(),
            80_i16.to_le_bytes().to_vec(),
        ),
        (
            32,
            1,
            0_u32.to_le_bytes().to_vec(),
            200_u32.to_le_bytes().to_vec(),
        ),
        (
            32,
            2,
            (-40_i32).to_le_bytes().to_vec(),
            80_i32.to_le_bytes().to_vec(),
        ),
        (
            64,
            1,
            0_u64.to_le_bytes().to_vec(),
            200_u64.to_le_bytes().to_vec(),
        ),
        (
            64,
            2,
            (-40_i64).to_le_bytes().to_vec(),
            80_i64.to_le_bytes().to_vec(),
        ),
        (
            16,
            3,
            0_u16.to_le_bytes().to_vec(),
            0x3c00_u16.to_le_bytes().to_vec(),
        ),
        (
            32,
            3,
            0_f32.to_le_bytes().to_vec(),
            1_f32.to_le_bytes().to_vec(),
        ),
        (
            64,
            3,
            0_f64.to_le_bytes().to_vec(),
            1_f64.to_le_bytes().to_vec(),
        ),
    ];
    for (bits, sample_format, low, high) in representations {
        let mut frames = two_voxels();
        for frame in &mut frames {
            frame.bits = bits;
            frame.sample_format = sample_format;
            frame.data = frame
                .data
                .iter()
                .flat_map(|&v| if v == 0 { low.clone() } else { high.clone() })
                .collect();
        }
        let input = Input::new(&carrier::encode(&frames, Layout::default()).unwrap(), "tif");
        boxes(
            &formats::load(&input.path).unwrap(),
            [[0., 1., 0.], [2., 0., 1.]],
            [0.5; 3],
        );
    }
}

#[test]
fn whiteiszero_is_normalized_once_and_nonfinite_or_signed_white_samples_fail() {
    for float in [false, true] {
        let mut frames = two_voxels();
        for frame in &mut frames {
            frame.white_is_zero = true;
            if float {
                frame.bits = 32;
                frame.sample_format = 3;
                frame.data = frame
                    .data
                    .iter()
                    .flat_map(|&v| (if v == 0 { 1_f32 } else { 0_f32 }).to_le_bytes())
                    .collect();
            } else {
                frame.data = frame.data.iter().map(|&v| u8::MAX - v).collect();
            }
        }
        let input = Input::new(&carrier::encode(&frames, Layout::default()).unwrap(), "tif");
        boxes(
            &formats::load(&input.path).unwrap(),
            [[0., 1., 0.], [2., 0., 1.]],
            [0.5; 3],
        );
    }
    let mut frames = two_voxels();
    for frame in &mut frames {
        frame.bits = 32;
        frame.sample_format = 3;
        frame.data = frame
            .data
            .iter()
            .flat_map(|&v| f32::from(v).to_le_bytes())
            .collect();
    }
    frames[1].data[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    rejected(&frames, "finite");
    let mut frames = two_voxels();
    for frame in &mut frames {
        frame.sample_format = 2;
        frame.white_is_zero = true;
    }
    rejected(&frames, "signed");
}

#[test]
fn full_resolution_selection_ignores_reduced_pages_and_page_flags_preserve_chain_order() {
    let original = two_voxels();
    let reference = Input::new(
        &carrier::encode(&original, Layout::default()).unwrap(),
        "tif",
    );
    let expected = formats::load(&reference.path).unwrap();
    let mut reduced = Frame::gray8(1, 1, vec![0]);
    reduced.subfile_type = 1;
    let frames = vec![original[0].clone(), reduced, original[1].clone()];
    let input = Input::new(
        &carrier::encode(&frames, Layout::default()).unwrap(),
        "tiff",
    );
    assert_eq!(
        formats::load(&input.path)
            .unwrap()
            .positions()
            .collect::<Vec<_>>(),
        expected.positions().collect::<Vec<_>>()
    );
    let mut pages = original;
    for page in &mut pages {
        page.subfile_type = 2;
    }
    let input = Input::new(&carrier::encode(&pages, Layout::default()).unwrap(), "tiff");
    assert_eq!(
        formats::load(&input.path)
            .unwrap()
            .positions()
            .collect::<Vec<_>>(),
        expected.positions().collect::<Vec<_>>()
    );
}

#[test]
fn deflate_lzw_and_packbits_pages_preserve_literal_density() {
    for (name, compression) in [
        ("Deflate", Compression::Deflate(DeflateLevel::Balanced)),
        ("LZW", Compression::Lzw),
        ("PackBits", Compression::Packbits),
    ] {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut encoder = TiffEncoder::new(&mut bytes)
                .unwrap()
                .with_compression(compression);
            for frame in two_voxels() {
                encoder
                    .write_image::<colortype::Gray8>(frame.width, frame.height, &frame.data)
                    .unwrap();
            }
        }
        let input = Input::new(bytes.get_ref(), "tif");
        boxes(
            &formats::load(&input.path).unwrap_or_else(|error| panic!("{name}: {error}")),
            [[0., 1., 0.], [2., 0., 1.]],
            [0.5; 3],
        );
    }
}

#[test]
fn automatic_and_forced_library_and_headless_cli_render_the_whole_stack() {
    let input = Input::new(
        &carrier::encode(&two_voxels(), Layout::default()).unwrap(),
        "tif",
    );
    let options = RenderOptions {
        width: 96,
        height: 96,
        transparent: true,
        ..RenderOptions::default()
    };
    let expected = render(&formats::load(&input.path).unwrap(), &options).unwrap();
    assert_eq!(thumbnail(&input.path, &options).unwrap(), expected);
    assert_eq!(geometry_thumbnail(&input.path, &options).unwrap(), expected);
    for force in [false, true] {
        let output = input.directory.join(format!("output-{force}.png"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_catwalk"));
        if force {
            command.arg("--render-geometry");
        }
        let result = command
            .arg(&input.path)
            .args(["--size", "96", "--transparent"])
            .arg("--output")
            .arg(&output)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty() && result.stderr.is_empty());
        assert_eq!(image::open(output).unwrap().to_rgba8(), expected);
    }
}

#[test]
fn single_page_images_keep_their_existing_fitted_preview() {
    let frame = Frame::gray8(3, 2, vec![0, 40, 80, 120, 160, 200]);
    let input = Input::new(
        &carrier::encode(&[frame], Layout::default()).unwrap(),
        "tiff",
    );
    let options = RenderOptions {
        width: 96,
        height: 64,
        transparent: true,
        ..RenderOptions::default()
    };
    let actual = thumbnail(&input.path, &options).unwrap();
    assert_eq!(actual.dimensions(), (96, 64));
    assert!(actual.get_pixel(8, 8)[0] < actual.get_pixel(88, 56)[0]);
    assert!(geometry_thumbnail(&input.path, &options).is_err());
    for subfile_type in [1, 4] {
        let frame = Frame {
            subfile_type,
            ..Frame::gray8(3, 2, vec![0, 40, 80, 120, 160, 200])
        };
        let input = Input::new(
            &carrier::encode(&[frame], Layout::default()).unwrap(),
            "tif",
        );
        assert_eq!(thumbnail(&input.path, &options).unwrap(), actual);
    }
}

#[test]
fn unsupported_volume_compression_and_rgb_pages_fail_before_decoding() {
    let mut bytes = carrier::encode(&two_voxels(), Layout::default()).unwrap();
    for page in 0..2 {
        let value = 8 + page * 186 + 2 + 4 * 12 + 8;
        bytes[value..value + 2].copy_from_slice(&7_u16.to_le_bytes());
    }
    let input = Input::new(&bytes, "tiff");
    assert!(
        thumbnail(&input.path, &RenderOptions::default())
            .unwrap_err()
            .to_string()
            .contains("compression")
    );
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut bytes).unwrap();
        for frame in two_voxels() {
            let rgb: Vec<_> = frame.data.into_iter().flat_map(|v| [v, 10, 0]).collect();
            encoder
                .write_image::<colortype::RGB8>(frame.width, frame.height, &rgb)
                .unwrap();
        }
    }
    let input = Input::new(bytes.get_ref(), "tiff");
    assert!(
        thumbnail(&input.path, &RenderOptions::default())
            .unwrap_err()
            .to_string()
            .contains("grayscale")
    );
}

/// Require explicit automatic-preview errors rather than a silent first-page image.
fn rejected(frames: &[Frame], expected: &str) {
    let input = Input::new(&carrier::encode(frames, Layout::default()).unwrap(), "tif");
    let error = thumbnail(&input.path, &RenderOptions::default())
        .unwrap_err()
        .to_string();
    assert!(error.to_ascii_lowercase().contains(expected), "{error}");
}

#[test]
fn heterogeneous_dimensions_orientation_and_spacing_do_not_become_unrelated_slices() {
    let mut frames = two_voxels();
    frames[1] = Frame::gray8(2, 2, vec![0; 4]);
    rejected(&frames, "dimension");
    let mut frames = two_voxels();
    frames[1].orientation = 2;
    rejected(&frames, "orientation");
    let mut frames = two_voxels();
    frames[1].resolution_unit = 3;
    frames[1].x_resolution = [2, 1];
    rejected(&frames, "resolution");
    let mut frames = two_voxels();
    for frame in &mut frames {
        frame.subfile_type = 1;
    }
    rejected(&frames, "reduced");
    let frames = [
        Frame {
            subfile_type: 1,
            ..Frame::gray8(1, 1, vec![0])
        },
        two_voxels().remove(0),
    ];
    rejected(&frames, "first");
}

#[test]
fn multidimensional_descriptions_and_subifd_pyramids_do_not_become_scalar_z_stacks() {
    for description in [
        "<OME xmlns=\"http://www.openmicroscopy.org/Schemas/OME/2016-06\"><Image><Pixels DimensionOrder=\"XYCZT\" SizeC=\"2\" SizeT=\"1\"/></Image></OME>",
        "<?xml version=\"1.0\"?><ome:OME xmlns:ome=\"http://www.openmicroscopy.org/Schemas/OME/2016-06\"/>",
        "ImageJ=1.54\nimages=2\nchannels=2\nframes=1\n",
        "ImageJ=1.54\nimages=2\nchannels=1\nframes=2\n",
    ] {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut encoder = TiffEncoder::new(&mut bytes).unwrap();
            for frame in two_voxels() {
                let mut page = encoder
                    .new_image::<colortype::Gray8>(frame.width, frame.height)
                    .unwrap();
                page.encoder()
                    .write_tag(tiff::tags::Tag::ImageDescription, description)
                    .unwrap();
                page.write_data(&frame.data).unwrap();
            }
        }
        let input = Input::new(bytes.get_ref(), "tiff");
        assert!(
            thumbnail(&input.path, &RenderOptions::default()).is_err(),
            "{description}"
        );
    }
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut bytes).unwrap();
        for frame in two_voxels() {
            let mut page = encoder
                .new_image::<colortype::Gray8>(frame.width, frame.height)
                .unwrap();
            page.encoder()
                .write_tag(tiff::tags::Tag::SubIfd, 0_u32)
                .unwrap();
            page.write_data(&frame.data).unwrap();
        }
    }
    let input = Input::new(bytes.get_ref(), "tiff");
    assert!(thumbnail(&input.path, &RenderOptions::default()).is_err());
}

#[test]
fn malformed_classic_and_bigtiff_chains_fail_before_pixel_expansion() {
    for big in [false, true] {
        let layout = Layout {
            big,
            big_endian: false,
        };
        let valid = carrier::encode(&two_voxels(), layout).unwrap();
        let first = if big { 16 } else { 8 };
        let count_width = if big { 8 } else { 2 };
        let entry_width = if big { 20 } else { 12 };
        let offset_width = if big { 8 } else { 4 };
        let next = first + count_width + 15 * entry_width;
        let mut mutations = Vec::new();
        let mut huge_count = valid.clone();
        huge_count[first..first + count_width].fill(255);
        mutations.push(huge_count);
        let mut cycle = valid.clone();
        cycle[next..next + offset_width]
            .copy_from_slice(&(first as u64).to_le_bytes()[..offset_width]);
        mutations.push(cycle);
        let mut duplicate = valid.clone();
        duplicate[first + count_width + entry_width..first + count_width + entry_width + 2]
            .copy_from_slice(&254_u16.to_le_bytes());
        mutations.push(duplicate);
        let mut bad_offset = valid.clone();
        let width_entry = first + count_width + entry_width;
        let value = width_entry + if big { 12 } else { 8 };
        bad_offset[value..value + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        mutations.push(bad_offset);
        mutations.push(valid[..next + offset_width - 1].to_vec());
        for bytes in mutations {
            let input = Input::new(&bytes, "tif");
            assert!(thumbnail(&input.path, &RenderOptions::default()).is_err());
        }
    }
}

/// Change each classic little-endian single-strip byte count without touching pixels.
fn shorten_strips(bytes: &mut [u8]) {
    let mut directory =
        usize::try_from(u32::from_le_bytes(bytes[4..8].try_into().unwrap())).unwrap();
    while directory != 0 {
        let count = usize::from(u16::from_le_bytes(
            bytes[directory..directory + 2].try_into().unwrap(),
        ));
        for index in 0..count {
            let entry = directory + 2 + index * 12;
            if u16::from_le_bytes(bytes[entry..entry + 2].try_into().unwrap()) == 279 {
                assert_eq!(
                    u32::from_le_bytes(bytes[entry + 4..entry + 8].try_into().unwrap()),
                    1
                );
                bytes[entry + 8..entry + 12].copy_from_slice(&1_u32.to_le_bytes());
            }
        }
        let next = directory + 2 + count * 12;
        directory = usize::try_from(u32::from_le_bytes(
            bytes[next..next + 4].try_into().unwrap(),
        ))
        .unwrap();
    }
}

#[test]
fn uncompressed_and_deflate_cannot_consume_pixels_outside_their_declared_strips() {
    let original = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/tiff_volumes/seq-1c-8b-multipage-adeefdcc.tiff");
    let mut deflate = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut deflate)
            .unwrap()
            .with_compression(Compression::Deflate(DeflateLevel::Balanced));
        for frame in two_voxels() {
            encoder
                .write_image::<colortype::Gray8>(frame.width, frame.height, &frame.data)
                .unwrap();
        }
    }
    for mut bytes in [fs::read(original).unwrap(), deflate.into_inner()] {
        let valid = Input::new(&bytes, "tif");
        assert!(formats::load(&valid.path).is_ok());
        shorten_strips(&mut bytes);
        let short = Input::new(&bytes, "tif");
        assert!(thumbnail(&short.path, &RenderOptions::default()).is_err());
        assert!(geometry_thumbnail(&short.path, &RenderOptions::default()).is_err());
    }
}

#[test]
fn cumulative_padded_tile_work_fails_before_any_large_pixel_expansion() {
    let frames: Vec<_> = (0..9).map(|_| Frame::gray8(1, 1, vec![1])).collect();
    let mut bytes = carrier::encode(&frames, Layout::default()).unwrap();
    for page in 0..frames.len() {
        let directory = 8 + page * 186;
        for (index, tag) in [(6, 324_u16), (8, 322), (9, 323), (10, 325)] {
            let entry = directory + 2 + index * 12;
            bytes[entry..entry + 2].copy_from_slice(&tag.to_le_bytes());
            if matches!(tag, 322 | 323) {
                bytes[entry + 8..entry + 12].copy_from_slice(&8192_u32.to_le_bytes());
            }
        }
    }
    let input = Input::new(&bytes, "tiff");
    let error = thumbnail(&input.path, &RenderOptions::default())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("cumulative") && error.contains("work"),
        "{error}"
    );
}
