//! Portable authored point-cloud/raster fixtures, with an attributed I3S sample.
use catwalk::{Result, render::RenderOptions};
use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, Rgb, Rgba, RgbaImage};
use std::{
    collections::{HashMap, HashSet},
    f64::consts::TAU,
    fmt::Write as _,
    fs,
    io::{Cursor, Write},
    path::Path,
};

const DIR: &str = "images_points";
const AUTHORED: &str =
    "Author-created deterministic Rust fixture; MIT, matching the project license";
const ESRI: &str = "Unmodified Esri LEPCC testData/SMALL_AUTZEN_LAS_All.slpk; Apache-2.0; https://github.com/Esri/lepcc; original license and notice retained in images_points/ESRI-LICENSE.txt and ESRI-NOTICE.txt";
const ESRI_COLORED: &str = "Authored I3S 2.0 package and LEPCC RGB bands (MIT), containing unchanged Esri LEPCC SMALL_AUTZEN_LAS_All XYZ geometry (Apache-2.0); https://github.com/Esri/lepcc; license and notice retained in images_points/ESRI-LICENSE.txt and ESRI-NOTICE.txt";

struct Fixtures<'a> {
    root: &'a Path,
    cases: Vec<crate::Fixture>,
}

impl Fixtures<'_> {
    fn resource(&self, name: &str, bytes: &[u8]) -> Result<()> {
        fs::write(self.root.join(DIR).join(name), bytes)?;
        Ok(())
    }
    fn register(
        &mut self,
        id: &str,
        extension: &str,
        title: &str,
        kind: &str,
        provenance: &str,
    ) -> Result<()> {
        let input = format!("{DIR}/{id}.{extension}");
        if kind == "model" {
            verify_points(&self.root.join(&input), extension, id)?;
        }
        let thumbnail = catwalk::thumbnail(
            &self.root.join(&input),
            &RenderOptions {
                width: 256,
                height: 256,
                transparent: true,
                supersampling: 2,
                ..Default::default()
            },
        )?;
        if !thumbnail.pixels().any(|pixel| pixel[3] != 0) {
            return Err(format!("generated {input} has an empty thumbnail").into());
        }
        let distinct: HashSet<_> = thumbnail.pixels().map(|pixel| pixel.0).collect();
        if kind == "image" && distinct.len() < 32 {
            return Err(format!("generated {input} thumbnail lost its authored pattern").into());
        }
        self.cases.push(crate::fixture(
            id, extension, title, &input, kind, provenance,
        ));
        Ok(())
    }
    fn add(
        &mut self,
        id: &str,
        extension: &str,
        title: &str,
        kind: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.resource(&format!("{id}.{extension}"), bytes)?;
        self.register(id, extension, title, kind, AUTHORED)
    }
}

pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    fs::create_dir_all(root.join(DIR))?;
    let mut fixtures = Fixtures {
        root,
        cases: Vec::new(),
    };
    point_formats(&mut fixtures)?;
    image_formats(&mut fixtures)?;
    fixtures.resource(
        "slpk-autzen.slpk",
        include_bytes!("../../tests/fixtures/slpk/autzen.slpk"),
    )?;
    fixtures.resource(
        "ESRI-LICENSE.txt",
        include_bytes!("../../tests/fixtures/slpk/LICENSE.txt"),
    )?;
    fixtures.resource(
        "ESRI-NOTICE.txt",
        include_bytes!("../../tests/fixtures/slpk/NOTICE.txt"),
    )?;
    fixtures.register(
        "slpk-autzen",
        "slpk",
        "I3S original LEPCC geographic point cloud",
        "model",
        ESRI,
    )?;
    fixtures.resource("slpk-colored-autzen.slpk", &colored_slpk()?)?;
    fixtures.register(
        "slpk-colored-autzen",
        "slpk",
        "I3S 2.0 LEPCC reference terrain with authored RGB bands",
        "model",
        ESRI_COLORED,
    )?;
    fixtures.resource(
        "README.txt",
        b"Point and raster fixtures are deterministic authored examples, MIT, matching the project license. QFIT, SBET and CSD have no RGB field and preview survey geometry. DICOM inputs are synthetic secondary captures with no real patient data. slpk-autzen.slpk is unchanged Esri LEPCC testData/SMALL_AUTZEN_LAS_All.slpk. slpk-colored-autzen.slpk reuses its unchanged XYZ geometry in an authored I3S package with authored RGB bands. These Esri geometry bytes retain Apache-2.0 provenance; see ESRI-LICENSE.txt and ESRI-NOTICE.txt. Source: https://github.com/Esri/lepcc\n",
    )?;
    Ok(fixtures.cases)
}

/// Validate independently authored placement and RGB through the public loader,
/// so rendering a nonempty but incorrectly decoded cloud cannot approve input.
fn verify_points(path: &Path, extension: &str, id: &str) -> Result<()> {
    let scene = catwalk::formats::load(path)?;
    let expected_count = if extension == "slpk" {
        106
    } else if extension == "csd" {
        1760
    } else {
        880
    };
    if scene.points.len() != expected_count
        || !scene.triangles.is_empty()
        || !scene.lines.is_empty()
    {
        return Err(format!("generated {id} lost its authored point count").into());
    }
    let positions: HashSet<_> = scene
        .points
        .iter()
        .map(|point| [point.position.x, point.position.y, point.position.z].map(f64::to_bits))
        .collect();
    if positions.len() < 64 {
        return Err(format!("generated {id} collapsed distinct point geometry").into());
    }
    let has_rgb = !matches!(extension, "qi" | "sbet" | "csd") && id != "slpk-autzen";
    if has_rgb {
        let colors: HashSet<_> = scene
            .points
            .iter()
            .filter_map(|point| point.color)
            .collect();
        if colors.len() < 4 || scene.points.iter().any(|point| point.color.is_none()) {
            return Err(format!("generated {id} lost its authored RGB pattern").into());
        }
    }
    let expected = match extension {
        "ptx" => [8., -4., 0.4],
        "las" | "laz" | "bpf" => [1001., 2000., 8.4],
        "qi" | "sbet" | "csd" => [-121.9, 51., 9.84],
        "slpk" => return Ok(()),
        _ => [1., 0., -1.6],
    };
    let point = scene.points[0].position;
    if [point.x, point.y, point.z]
        .into_iter()
        .zip(expected)
        .any(|(actual, expected)| (actual - expected).abs() > 0.002)
    {
        return Err(format!("generated {id} changed the authored coordinate placement").into());
    }
    Ok(())
}

/// Package unchanged independently encoded XYZ with an independently authored
/// ClusterRGB raw stream. The original Esri package stays byte-for-byte intact.
fn colored_slpk() -> Result<Vec<u8>> {
    const COUNT: usize = 106;
    let palette = [
        [235u8, 75, 50],
        [35, 160, 220],
        [80, 190, 100],
        [245, 190, 45],
        [165, 90, 205],
        [235, 115, 165],
    ];
    let mut rgb = b"ClusterRGB".to_vec();
    rgb.extend(1u16.to_le_bytes());
    rgb.extend(0u32.to_le_bytes());
    rgb.extend((32u64 + COUNT as u64 * 3).to_le_bytes());
    rgb.extend((COUNT as u32).to_le_bytes());
    rgb.extend([0; 4]); // No palette/lookup/compression: lossless RGB triples.
    for index in 0..COUNT {
        rgb.extend(palette[index / 7 % palette.len()]);
    }
    // LEPCC Fletcher32 uses big-endian words, padding the final odd byte with 0.
    let mut first = 0u32;
    let mut second = 0u32;
    for word in rgb[16..].chunks(2) {
        let value = u16::from_be_bytes([word[0], word.get(1).copied().unwrap_or(0)]);
        first = (first + value as u32) % 65535;
        second = (second + first) % 65535;
    }
    if first == 0 {
        first = 65535;
    }
    if second == 0 {
        second = 65535;
    }
    rgb[12..16].copy_from_slice(&((second << 16) | first).to_le_bytes());
    let layer = serde_json::json!({
        "layerType":"PointCloud", "spatialReference":{"wkid":4326},
        "store":{"version":"2.0","index":{"nodesPerPage":64},
            "defaultGeometrySchema":{"encoding":"lepcc-xyz"}},
        "attributeStorageInfo":[{"name":"RGB","key":"color","encoding":"lepcc-rgb"}]
    });
    let nodes = serde_json::json!({"nodes":[{
        "childCount":0, "resourceId":0, "vertexCount":COUNT
    }]});
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in [
        ("3dSceneLayer.json", serde_json::to_vec(&layer)?),
        ("nodepages/0.json", serde_json::to_vec(&nodes)?),
        (
            "nodes/0/geometries/0.bin.pccxyz",
            include_bytes!("../../tests/fixtures/slpk/autzen.xyz").to_vec(),
        ),
        ("nodes/0/attributes/color.bin.pccrgb", rgb),
    ] {
        archive.start_file(name, options)?;
        archive.write_all(&bytes)?;
    }
    Ok(archive.finish()?.into_inner())
}

#[derive(Clone, Copy)]
struct ColoredPoint {
    position: [f64; 3],
    color: [u8; 3],
}

/// Two densely sampled helical rails and sixteen colored rungs. The asymmetric
/// red/blue rails make axis, color and placement regressions visible.
fn helix() -> Vec<ColoredPoint> {
    let mut points = Vec::new();
    for i in 0..256 {
        let t = i as f64 / 255.;
        let angle = t * TAU * 1.6;
        for strand in 0..2 {
            let phase = angle + strand as f64 * TAU / 2.;
            points.push(ColoredPoint {
                position: [phase.cos(), phase.sin(), (t - 0.5) * 3.2],
                color: if strand == 0 {
                    [235, 75 + (70. * t) as u8, 45]
                } else {
                    [40, 115 + (90. * t) as u8, 235]
                },
            });
        }
    }
    for rung in 0..16 {
        let t = (rung as f64 + 0.5) / 16.;
        let angle = t * TAU * 1.6;
        for i in 1..24 {
            let radius = 1. - i as f64 / 12.;
            points.push(ColoredPoint {
                position: [radius * angle.cos(), radius * angle.sin(), (t - 0.5) * 3.2],
                color: if rung % 2 == 0 {
                    [80, 195, 100]
                } else {
                    [245, 195, 50]
                },
            });
        }
    }
    points
}

fn point_formats(f: &mut Fixtures<'_>) -> Result<()> {
    let points = helix();
    let mut pts = format!("{}\n", points.len());
    for p in &points {
        let [x, y, z] = p.position;
        let [r, g, b] = p.color;
        writeln!(pts, "{x:.6} {y:.6} {z:.6} 0.8 {r} {g} {b}")?;
    }
    f.add(
        "pts-colored-helix",
        "pts",
        "PTS counted colored double helix",
        "model",
        pts.as_bytes(),
    )?;
    // Row-vector PTX transform rotates around Z and translates the scan.
    let mut ptx = format!(
        "{}\n1\n8 -3 2\n0 -1 0\n1 0 0\n0 0 1\n0 -1 0 0\n1 0 0 0\n0 0 1 0\n8 -3 2 1\n",
        points.len() + 1
    );
    for p in &points {
        let [x, y, z] = p.position;
        let [r, g, b] = p.color;
        writeln!(ptx, "{x:.6} {y:.6} {z:.6} 0.8 {r} {g} {b}")?;
    }
    ptx.push_str("0 0 0 0 0 0 0\n");
    f.add(
        "ptx-transformed-helix",
        "ptx",
        "PTX transformed scan and missing return",
        "model",
        ptx.as_bytes(),
    )?;
    for encoding in ["ascii", "binary", "binary_compressed"] {
        let id = match encoding {
            "binary_compressed" => "pcd-lzf-helix",
            "binary" => "pcd-binary-helix",
            _ => "pcd-ascii-helix",
        };
        let title = format!("PCD {encoding} colored double helix");
        f.add(id, "pcd", &title, "model", &pcd(&points, encoding))?;
    }
    let mut splat = Vec::new();
    for p in &points {
        for value in p.position {
            splat.extend((value as f32).to_le_bytes());
        }
        for value in [0.04f32, 0.03, 0.06] {
            splat.extend(value.to_le_bytes());
        }
        splat.extend(p.color);
        splat.push(255);
        splat.extend([255, 128, 128, 128]); // Quantized identity quaternion, WXYZ.
    }
    f.add(
        "splat-colored-helix",
        "splat",
        "Gaussian SPLAT colored centers",
        "model",
        &splat,
    )?;
    for version in 1..=4 {
        f.add(
            &format!("spz-v{version}-helix"),
            "spz",
            &format!(
                "SPZ v{version} {} colored centers",
                if version == 4 { "Zstandard" } else { "gzip" }
            ),
            "model",
            &spz(&points, version)?,
        )?;
    }
    for compressed in [false, true] {
        f.add(
            if compressed {
                "laz-colored-helix"
            } else {
                "las-colored-helix"
            },
            if compressed { "laz" } else { "las" },
            if compressed {
                "LAZ compressed 16-bit RGB points"
            } else {
                "LAS 16-bit RGB points"
            },
            "model",
            &las(&points, compressed)?,
        )?;
    }
    for (layout, name) in [(0, "dimension"), (1, "interleaved"), (2, "byte")] {
        for compressed in [false, true] {
            f.add(
                &format!("bpf-{name}-{}", if compressed { "zlib" } else { "raw" }),
                "bpf",
                &format!(
                    "BPF {name} layout with {} RGB points",
                    if compressed { "zlib" } else { "raw" }
                ),
                "model",
                &bpf(&points, layout, compressed)?,
            )?;
        }
    }
    for version in [20010712, 20020715] {
        f.add(
            &format!("bin-terrascan-{version}"),
            "bin",
            &format!("TerraScan {version} colored survey points"),
            "model",
            &terra(&points, version),
        )?;
    }
    for (size, big) in [(40, false), (48, false), (56, true)] {
        f.add(
            &format!(
                "qi-{}-{}",
                size / 4,
                if big { "big-endian" } else { "little-endian" }
            ),
            "qi",
            &format!(
                "NASA QFIT {}-word {} survey",
                size / 4,
                if big { "big endian" } else { "little endian" }
            ),
            "model",
            &qfit(&points, size, big),
        )?;
    }
    f.add(
        "sbet-survey-trajectory",
        "sbet",
        "SBET navigation trajectory in radians",
        "model",
        &sbet(&points),
    )?;
    f.add(
        "csd-survey-echoes",
        "csd",
        "Optech CSD range echoes and platform placement",
        "model",
        &csd(&points),
    )?;
    Ok(())
}

fn packed_rgb(p: ColoredPoint) -> u32 {
    (p.color[0] as u32) << 16 | (p.color[1] as u32) << 8 | p.color[2] as u32
}

fn pcd(points: &[ColoredPoint], encoding: &str) -> Vec<u8> {
    let mut data = format!("# .PCD v0.7\nVERSION 0.7\nFIELDS x y z rgb\nSIZE 4 4 4 4\nTYPE F F F U\nCOUNT 1 1 1 1\nWIDTH {}\nHEIGHT 1\nVIEWPOINT 0 0 0 1 0 0 0\nPOINTS {}\nDATA {encoding}\n", points.len(), points.len()).into_bytes();
    if encoding == "ascii" {
        for &p in points {
            let [x, y, z] = p.position;
            data.extend(format!("{x:.6} {y:.6} {z:.6} {}\n", packed_rgb(p)).bytes());
        }
    } else if encoding == "binary" {
        for &p in points {
            for value in p.position {
                data.extend((value as f32).to_le_bytes());
            }
            data.extend(packed_rgb(p).to_le_bytes());
        }
    } else {
        let mut fields = Vec::new();
        for axis in 0..3 {
            for p in points {
                fields.extend((p.position[axis] as f32).to_le_bytes());
            }
        }
        for &p in points {
            fields.extend(packed_rgb(p).to_le_bytes());
        }
        let lzf = lzf_encode(&fields);
        data.extend((lzf.len() as u32).to_le_bytes());
        data.extend((fields.len() as u32).to_le_bytes());
        data.extend(lzf);
    }
    data
}

/// Independent LZF writer with literal runs and overlapping back references.
fn lzf_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut literals = Vec::new();
    let mut previous = HashMap::<[u8; 3], usize>::new();
    let mut cursor = 0;
    let flush = |out: &mut Vec<u8>, literals: &mut Vec<u8>| {
        if !literals.is_empty() {
            out.push((literals.len() - 1) as u8);
            out.append(literals);
        }
    };
    while cursor < data.len() {
        let mut length = 0;
        let mut distance = 0;
        if cursor + 3 <= data.len() {
            let key: [u8; 3] = data[cursor..cursor + 3].try_into().unwrap();
            if let Some(&start) = previous.get(&key) {
                distance = cursor - start;
                if distance <= 8192 {
                    while length < 264
                        && cursor + length < data.len()
                        && data[start + length] == data[cursor + length]
                    {
                        length += 1;
                    }
                }
            }
        }
        if length >= 3 {
            flush(&mut out, &mut literals);
            let offset = distance - 1;
            let encoded_length = length - 2;
            out.push(((encoded_length.min(7) << 5) | (offset >> 8)) as u8);
            if encoded_length >= 7 {
                out.push((encoded_length - 7) as u8);
            }
            out.push(offset as u8);
            for position in cursor..cursor + length {
                if position + 3 <= data.len() {
                    previous.insert(data[position..position + 3].try_into().unwrap(), position);
                }
            }
            cursor += length;
        } else {
            if cursor + 3 <= data.len() {
                previous.insert(data[cursor..cursor + 3].try_into().unwrap(), cursor);
            }
            literals.push(data[cursor]);
            cursor += 1;
            if literals.len() == 32 {
                flush(&mut out, &mut literals);
            }
        }
    }
    flush(&mut out, &mut literals);
    out
}

/// Round f32 coordinates to IEEE binary16 for the original SPZ v1 layout.
fn half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) as u16 & 0x8000;
    let exponent = ((bits >> 23) & 255) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let shift = 14 - exponent;
        return sign | (((mantissa | 0x80_0000) + (1 << (shift - 1))) >> shift) as u16;
    }
    let rounded = (mantissa + 0x1000) >> 13;
    sign | (((exponent as u32) << 10) + rounded) as u16
}

fn spz_attributes(points: &[ColoredPoint], version: u32) -> Vec<Vec<u8>> {
    let mut attributes = vec![Vec::new(); 5];
    for p in points {
        // SPZ Right-Up-Back coordinates: the importer converts (x,y,z) to (x,-z,y).
        for value in [p.position[0], p.position[2], -p.position[1]] {
            if version == 1 {
                attributes[0].extend(half(value as f32).to_le_bytes());
            } else {
                attributes[0].extend(&((value * 4096.).round() as i32).to_le_bytes()[..3]);
            }
        }
        attributes[1].push(255);
        for channel in p.color {
            let encoded =
                (0.5 + ((channel as f64 / 255.) - 0.5) * 0.15 / 0.28209479177387814) * 255.;
            attributes[2].push(encoded.round() as u8);
        }
        attributes[3].extend([109, 104, 115]); // Quantized log scales for small Gaussians.
        if version >= 3 {
            attributes[4].extend(0xc0000000u32.to_le_bytes());
        } else {
            attributes[4].extend([128, 128, 128]);
        }
    }
    attributes
}

fn zstd_raw(data: &[u8]) -> Vec<u8> {
    // Single-segment frame with a 32-bit content size. Constant alpha planes
    // use a real RLE block; remaining attribute planes use a raw final block.
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
    frame.extend((data.len() as u32).to_le_bytes());
    let rle = data.iter().all(|&v| v == data[0]);
    frame.extend(&(((data.len() as u32) << 3) | if rle { 3 } else { 1 }).to_le_bytes()[..3]);
    if rle {
        frame.push(data[0]);
    } else {
        frame.extend(data);
    }
    frame
}

fn spz(points: &[ColoredPoint], version: u32) -> Result<Vec<u8>> {
    let attributes = spz_attributes(points, version);
    let mut bytes = b"NGSP".to_vec();
    bytes.extend(version.to_le_bytes());
    bytes.extend((points.len() as u32).to_le_bytes());
    bytes.extend([0, 12, 0, if version == 4 { 5 } else { 0 }]);
    if version == 4 {
        bytes.extend(32u32.to_le_bytes());
        bytes.extend([0; 12]);
        let frames: Vec<_> = attributes.iter().map(|a| zstd_raw(a)).collect();
        for (attribute, frame) in attributes.iter().zip(&frames) {
            bytes.extend((frame.len() as u64).to_le_bytes());
            bytes.extend((attribute.len() as u64).to_le_bytes());
        }
        for frame in frames {
            bytes.extend(frame);
        }
        Ok(bytes)
    } else {
        for attribute in attributes {
            bytes.extend(attribute);
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&bytes)?;
        Ok(encoder.finish()?)
    }
}

fn las(points: &[ColoredPoint], compressed: bool) -> Result<Vec<u8>> {
    let mut builder = las::Builder::from((1, 4));
    builder.point_format = las::point::Format::new(2)?;
    builder.point_format.is_compressed = compressed;
    builder.generating_software = "Catwalk Rust fixture generator".into();
    builder.system_identifier = "Authored double helix".into();
    builder.date = None;
    let mut writer = las::Writer::new(Cursor::new(Vec::new()), builder.into_header()?)?;
    for p in points {
        let [r, g, b] = p.color.map(|v| v as u16 * 257);
        writer.write_point(las::Point {
            x: p.position[0] + 1000.,
            y: p.position[1] + 2000.,
            z: p.position[2] + 10.,
            color: Some(las::Color::new(r, g, b)),
            return_number: 1,
            number_of_returns: 1,
            ..Default::default()
        })?;
    }
    Ok(writer.into_inner()?.into_inner())
}

fn bpf(points: &[ColoredPoint], layout: u8, compressed: bool) -> Result<Vec<u8>> {
    const DIMENSIONS: usize = 6;
    let mut bytes = b"BPF!0003".to_vec();
    bytes.extend((176u32 + 56 * DIMENSIONS as u32).to_le_bytes());
    bytes.extend([DIMENSIONS as u8, layout, if compressed { 3 } else { 0 }, 0]);
    bytes.extend((points.len() as u32).to_le_bytes());
    bytes.extend([0; 12]);
    for r in 0..4 {
        for c in 0..4 {
            bytes.extend((if r == c { 1f64 } else { 0. }).to_le_bytes());
        }
    }
    bytes.extend([0; 16]);
    for offset in [1000f64, 2000., 10., 0., 0., 0.] {
        bytes.extend(offset.to_le_bytes());
    }
    let values: Vec<[f32; DIMENSIONS]> = points
        .iter()
        .map(|p| {
            [
                p.position[0] as f32,
                p.position[1] as f32,
                p.position[2] as f32,
                p.color[0] as f32 * 257.,
                p.color[1] as f32 * 257.,
                p.color[2] as f32 * 257.,
            ]
        })
        .collect();
    // BPF stores all minima, then all maxima, in dimension order.
    for maximum in [false, true] {
        for d in 0..DIMENSIONS {
            let value = values
                .iter()
                .map(|p| p[d])
                .reduce(if maximum { f32::max } else { f32::min })
                .unwrap();
            let offset = [1000f64, 2000., 10., 0., 0., 0.][d];
            bytes.extend((value as f64 + offset).to_le_bytes());
        }
    }
    for label in ["X", "Y", "Z", "Red", "Green", "Blue"] {
        let mut field = [0; 32];
        field[..label.len()].copy_from_slice(label.as_bytes());
        bytes.extend(field);
    }
    let mut data = Vec::new();
    match layout {
        0 => {
            for d in 0..DIMENSIONS {
                for p in &values {
                    data.extend(p[d].to_le_bytes());
                }
            }
        }
        1 => {
            for p in &values {
                for v in p {
                    data.extend(v.to_le_bytes());
                }
            }
        }
        _ => {
            for d in 0..DIMENSIONS {
                for byte in 0..4 {
                    for p in &values {
                        data.push(p[d].to_le_bytes()[byte]);
                    }
                }
            }
        }
    }
    if compressed {
        // Multiple zlib blocks also exercise concatenated BPF block decoding.
        for block in data.chunks(4096) {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(block)?;
            let encoded = encoder.finish()?;
            bytes.extend((block.len() as u32).to_le_bytes());
            bytes.extend((encoded.len() as u32).to_le_bytes());
            bytes.extend(encoded);
        }
    } else {
        bytes.extend(data);
    }
    Ok(bytes)
}

fn terra(points: &[ColoredPoint], version: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [56, version, 970401] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(b"CXYZ");
    bytes.extend((points.len() as u32).to_le_bytes());
    bytes.extend(1000i32.to_le_bytes());
    for value in [1000000f64, 2000000., 10000.] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(1u32.to_le_bytes());
    for (i, p) in points.iter().enumerate() {
        if version == 20010712 {
            bytes.extend([1, 1, 0, 0]);
        }
        for (axis, value) in p.position.iter().enumerate() {
            bytes.extend(
                ((value * 1000.).round() as i32 + [1000000, 2000000, 10000][axis]).to_le_bytes(),
            );
        }
        if version == 20020715 {
            bytes.extend([1, 1, 0, 0, 0, 0, 0, 0]);
        }
        bytes.extend((i as u32 * 100).to_le_bytes());
        bytes.extend(p.color);
        bytes.push(255);
    }
    bytes
}

fn qfit(points: &[ColoredPoint], size: usize, big: bool) -> Vec<u8> {
    let mut bytes = vec![0; size * (points.len() + 2)];
    let put = |bytes: &mut [u8], offset: usize, value: i32| {
        bytes[offset..offset + 4].copy_from_slice(&if big {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        });
    };
    put(&mut bytes, 0, size as i32);
    put(&mut bytes, size, -9_000_000);
    put(&mut bytes, size + 4, (size * 2) as i32);
    for (i, p) in points.iter().enumerate() {
        let offset = size * (i + 2);
        // Store the authored pattern as a compact geographic survey; no CRS
        // reprojection is needed to exercise the QFIT record interpretation.
        for (word, value) in [
            i as i32 * 10,
            ((51. + p.position[1] * 0.1) * 1_000_000.).round() as i32,
            ((-122. + p.position[0] * 0.1) * 1_000_000.).round() as i32,
            ((10. + p.position[2] * 0.1) * 1000.).round() as i32,
        ]
        .into_iter()
        .enumerate()
        {
            put(&mut bytes, offset + word * 4, value);
        }
    }
    bytes
}

fn sbet(points: &[ColoredPoint]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let mut record = [0f64; 17];
        record[0] = i as f64 * 0.01;
        record[1] = (51. + p.position[1] * 0.1).to_radians();
        record[2] = (-122. + p.position[0] * 0.1).to_radians();
        record[3] = 10. + p.position[2] * 0.1;
        for value in record {
            bytes.extend(value.to_le_bytes());
        }
    }
    bytes
}

fn csd(points: &[ColoredPoint]) -> Vec<u8> {
    let mut bytes = vec![0; 2048];
    bytes[..4].copy_from_slice(b"CSD\0");
    bytes[100..104].copy_from_slice(&5.201f32.to_le_bytes());
    bytes[104..106].copy_from_slice(&2048u16.to_le_bytes());
    bytes[106..108].copy_from_slice(&2200u16.to_le_bytes());
    bytes[116..124].copy_from_slice(&(points.len() as f64 * 0.01).to_le_bytes());
    bytes[124..128].copy_from_slice(&(points.len() as u32).to_le_bytes());
    bytes[128..130].copy_from_slice(&1u16.to_le_bytes());
    for (i, p) in points.iter().enumerate() {
        bytes.extend((i as f64 * 0.01).to_le_bytes());
        bytes.push(2);
        for value in [10f32, 10.015, 0., 0.] {
            bytes.extend(value.to_le_bytes());
        }
        for value in [200u16, 120, 0, 0] {
            bytes.extend(value.to_le_bytes());
        }
        for value in [0f32; 4] {
            bytes.extend(value.to_le_bytes());
        } // Scan angle and platform attitude.
        bytes.extend((51. + p.position[1] * 0.1).to_radians().to_le_bytes());
        bytes.extend((-122. + p.position[0] * 0.1).to_radians().to_le_bytes());
        bytes.extend((20f32 + p.position[2] as f32 * 0.1).to_le_bytes());
    }
    bytes
}

/// Color chart with a chevron, ring and stepped bands. Each codec/alias gets a
/// different orientation/palette, while RGBA codecs retain a soft alpha edge.
fn pattern(seed: u32) -> RgbaImage {
    RgbaImage::from_fn(96, 64, |x, y| {
        let x = if seed.is_multiple_of(2) { x } else { 95 - x };
        let y = if seed.is_multiple_of(3) { 63 - y } else { y };
        let palette = [
            [235, 75, 50],
            [35, 160, 220],
            [80, 190, 100],
            [245, 190, 45],
            [165, 90, 205],
            [235, 115, 165],
        ];
        let mut color = palette[((x / 16 + seed) % 6) as usize];
        if y < 16 {
            color = [
                25 + x as u8 * 2,
                35 + y as u8 * 5,
                65 + (95 - x) as u8 + (seed % 9) as u8 * 5,
            ];
        } else if y > 47 {
            let level = 25 + (x / 12) as u8 * 30;
            color = [level; 3];
        }
        let dx = x as i32 - (23 + (seed % 5) as i32 * 2);
        let dy = y as i32 - (29 + (seed % 4) as i32 * 2);
        let distance = dx * dx + dy * dy;
        if (95..=175).contains(&distance) {
            color = [245, 248, 252];
        }
        if (x as i32 - (65 + (seed % 7) as i32)).abs() + (y as i32 - 32).abs()
            < 13 + (seed % 4) as i32
        {
            color = palette[((seed + 3) % 6) as usize];
        }
        if !(4..=91).contains(&x) || !(4..=59).contains(&y) {
            color = [15, 28, 45];
        }
        let edge = x.min(95 - x).min(y.min(63 - y));
        let alpha = match edge {
            0 => 0,
            1 => 65,
            2 => 150,
            3 => 225,
            _ => 255,
        };
        Rgba([color[0], color[1], color[2], alpha])
    })
}

fn encoded(image: DynamicImage, format: ImageFormat) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, format)?;
    Ok(cursor.into_inner())
}

fn image_formats(f: &mut Fixtures<'_>) -> Result<()> {
    for (seed, (extension, format, title)) in [
        ("png", ImageFormat::Png, "PNG color chart with alpha"),
        ("jpg", ImageFormat::Jpeg, "JPG patterned RGB color chart"),
        ("jpeg", ImageFormat::Jpeg, "JPEG mirrored RGB color chart"),
        ("bmp", ImageFormat::Bmp, "BMP patterned color chart"),
        ("tga", ImageFormat::Tga, "TGA RLE color chart with alpha"),
        ("webp", ImageFormat::WebP, "WebP lossless chart with alpha"),
        ("tif", ImageFormat::Tiff, "TIF RGBA chart with soft edge"),
        ("tiff", ImageFormat::Tiff, "TIFF mirrored RGBA chart"),
    ]
    .into_iter()
    .enumerate()
    {
        let rgba = pattern(seed as u32);
        let image = if matches!(format, ImageFormat::Jpeg) {
            DynamicImage::ImageRgba8(rgba).to_rgb8().into()
        } else {
            rgba.into()
        };
        f.add(
            &format!("{extension}-pattern"),
            extension,
            title,
            "image",
            &encoded(image, format)?,
        )?;
    }
    let hdr = pattern(8);
    let pixels: Vec<_> = hdr
        .pixels()
        .map(|p| Rgb([p[0] as f32 / 255., p[1] as f32 / 255., p[2] as f32 / 255.]))
        .collect();
    let mut bytes = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut bytes).encode(&pixels, 96, 64)?;
    f.add(
        "hdr-radiance-pattern",
        "hdr",
        "Radiance HDR float color chart",
        "image",
        &bytes,
    )?;
    let exr = DynamicImage::ImageRgba8(pattern(9)).to_rgba32f();
    f.add(
        "exr-float-alpha-pattern",
        "exr",
        "OpenEXR float RGBA chart",
        "image",
        &encoded(exr.into(), ImageFormat::OpenExr)?,
    )?;
    let raw_tga = pattern(10);
    let mut bytes = Vec::new();
    image::codecs::tga::TgaEncoder::new(&mut bytes)
        .disable_rle()
        .write_image(raw_tga.as_raw(), 96, 64, ExtendedColorType::Rgba8)?;
    f.add(
        "tga-uncompressed-pattern",
        "tga",
        "TGA uncompressed RGBA chart",
        "image",
        &bytes,
    )?;
    f.add(
        "dcm-native-rgb-pattern",
        "dcm",
        "DICOM native RGB secondary capture",
        "image",
        &dicom(true, false, false)?,
    )?;
    f.add(
        "dcm-rle-rgb-pattern",
        "dcm",
        "DICOM RLE three-plane RGB capture",
        "image",
        &dicom(true, false, true)?,
    )?;
    f.add(
        "dcm-native-gray16-pattern",
        "dcm",
        "DICOM native 16-bit grayscale capture",
        "image",
        &dicom(false, true, false)?,
    )?;
    f.add(
        "dcm-rle-gray-pattern",
        "dcm",
        "DICOM RLE 8-bit grayscale capture",
        "image",
        &dicom(false, false, true)?,
    )?;
    Ok(())
}

fn dicom_element(out: &mut Vec<u8>, group: u16, tag: u16, vr: &[u8; 2], mut value: Vec<u8>) {
    if !value.len().is_multiple_of(2) {
        value.push(if vr == b"UI" { 0 } else { b' ' });
    }
    out.extend(group.to_le_bytes());
    out.extend(tag.to_le_bytes());
    out.extend(vr);
    if [b"OB", b"OW", b"SQ", b"UN", b"UT"].contains(&vr) {
        out.extend([0; 2]);
        out.extend((value.len() as u32).to_le_bytes());
    } else {
        out.extend((value.len() as u16).to_le_bytes());
    }
    out.extend(value);
}

fn dicom(rgb: bool, gray16: bool, rle: bool) -> Result<Vec<u8>> {
    let mut meta = Vec::new();
    let syntax = if rle {
        "1.2.840.10008.1.2.5"
    } else {
        "1.2.840.10008.1.2.1"
    };
    let uid = format!(
        "2.25.123456789012345678901234567890123{}{}{}",
        rgb as u8, gray16 as u8, rle as u8
    );
    for (tag, vr, value) in [
        (1, b"OB", vec![0, 1]),
        (2, b"UI", b"1.2.840.10008.5.1.4.1.1.7".to_vec()),
        (3, b"UI", uid.as_bytes().to_vec()),
        (0x10, b"UI", syntax.as_bytes().to_vec()),
        (0x12, b"UI", b"2.25.123456789012345678901234567890".to_vec()),
    ] {
        dicom_element(&mut meta, 2, tag, vr, value);
    }
    let mut out = vec![0; 128];
    out.extend(b"DICM");
    dicom_element(
        &mut out,
        2,
        0,
        b"UL",
        (meta.len() as u32).to_le_bytes().to_vec(),
    );
    out.extend(meta);
    dicom_element(
        &mut out,
        8,
        0x16,
        b"UI",
        b"1.2.840.10008.5.1.4.1.1.7".to_vec(),
    );
    dicom_element(&mut out, 8, 0x18, b"UI", uid.as_bytes().to_vec());
    dicom_element(&mut out, 8, 0x60, b"CS", b"OT".to_vec());
    dicom_element(
        &mut out,
        0x10,
        0x10,
        b"PN",
        b"AUTHOR^CREATED^PATTERN".to_vec(),
    );
    dicom_element(
        &mut out,
        0x28,
        2,
        b"US",
        (if rgb { 3u16 } else { 1 }).to_le_bytes().to_vec(),
    );
    dicom_element(
        &mut out,
        0x28,
        4,
        b"CS",
        if rgb {
            b"RGB".to_vec()
        } else {
            b"MONOCHROME2".to_vec()
        },
    );
    if rgb {
        dicom_element(&mut out, 0x28, 6, b"US", 0u16.to_le_bytes().to_vec());
    }
    let bits = if gray16 { 16u16 } else { 8 };
    for (tag, value) in [
        (0x10, 64u16),
        (0x11, 96),
        (0x100, bits),
        (0x101, bits),
        (0x102, bits - 1),
        (0x103, 0),
    ] {
        dicom_element(&mut out, 0x28, tag, b"US", value.to_le_bytes().to_vec());
    }
    let mut pixels = Vec::new();
    for (x, y, p) in pattern(if rgb { 11 } else { 12 }).enumerate_pixels() {
        if rgb {
            pixels.extend(&p.0[..3]);
        } else {
            // Grayscale chart combines a ramp and distinct geometric regions.
            let value = if ((x as i32 - 32).pow(2) + (y as i32 - 31).pow(2)) < 180 {
                230
            } else if x > 58 && y > 15 && y < 48 {
                50
            } else {
                ((x * 180 / 95 + y * 70 / 63) as u8).min(250)
            };
            if gray16 {
                pixels.extend((value as u16 * 257).to_le_bytes());
            } else {
                pixels.push(value);
            }
        }
    }
    if !rle {
        dicom_element(
            &mut out,
            0x7fe0,
            0x10,
            if gray16 { b"OW" } else { b"OB" },
            pixels,
        );
    } else {
        let samples = if rgb { 3 } else { 1 };
        let mut frame = vec![0; 64];
        frame[..4].copy_from_slice(&(samples as u32).to_le_bytes());
        for component in 0..samples {
            let offset = frame.len() as u32;
            frame[4 + component * 4..8 + component * 4].copy_from_slice(&offset.to_le_bytes());
            let plane: Vec<_> = pixels
                .chunks_exact(samples)
                .map(|pixel| pixel[component])
                .collect();
            // DICOM RLE compresses every row independently, with PackBits
            // repeat runs as well as literals in each scalar plane.
            for row in plane.chunks(96) {
                frame.extend(packbits(row));
            }
        }
        if !frame.len().is_multiple_of(2) {
            frame.push(0);
        }
        out.extend(0x7fe0u16.to_le_bytes());
        out.extend(0x10u16.to_le_bytes());
        out.extend(b"OB\0\0");
        out.extend(u32::MAX.to_le_bytes());
        // Empty basic offset table, followed by one encapsulated frame.
        for payload in [Vec::new(), frame] {
            out.extend(0xfffeu16.to_le_bytes());
            out.extend(0xe000u16.to_le_bytes());
            out.extend((payload.len() as u32).to_le_bytes());
            out.extend(payload);
        }
        out.extend(0xfffeu16.to_le_bytes());
        out.extend(0xe0ddu16.to_le_bytes());
        out.extend(0u32.to_le_bytes());
    }
    Ok(out)
}

fn packbits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let run = |start: usize| {
        let mut length = 1;
        while start + length < row.len() && row[start + length] == row[start] && length < 128 {
            length += 1;
        }
        length
    };
    while cursor < row.len() {
        let length = run(cursor);
        if length >= 3 {
            out.push((1i16 - length as i16) as u8);
            out.push(row[cursor]);
            cursor += length;
        } else {
            let start = cursor;
            cursor += length;
            while cursor < row.len() && cursor - start < 128 && run(cursor) < 3 {
                cursor += run(cursor).min(128 - (cursor - start));
            }
            out.push((cursor - start - 1) as u8);
            out.extend(&row[start..cursor]);
        }
    }
    out
}
