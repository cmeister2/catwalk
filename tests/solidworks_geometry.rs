//! Original producer geometry, placed assembly and refusal qualification.

#[path = "support/digest_hex.rs"]
mod digest_hex;

use catwalk3d::{formats, geometry_thumbnail, render::RenderOptions, scene::Scene};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

/// Unique temporary project labels, independent of parallel test execution.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
/// Share the expensive unchanged native producer assembly across qualifications.
static ASSEMBLY: OnceLock<Scene> = OnceLock::new();
/// Native files addressed by the original assembly's exact component table.
const COMPONENTS: &[&str] = &[
    "frontface.SLDPRT",
    "松下38AH.SLDPRT",
    "空开.SLDPRT",
    "空开支架.SLDPRT",
    "稳压器.SLDPRT",
    "车载电源接口.SLDPRT",
    "显示器电源接口.SLDPRT",
];

/// Locate immutable producer files and their independent native witnesses.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/solidworks")
        .join(name)
}

/// Temporary relocated project, removed even after a failed qualification.
struct Directory(PathBuf);

impl Directory {
    /// Create an isolated directory for controlled native document mutations.
    fn new() -> Self {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "catwalk-solidworks-geometry-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    /// Supply the full original assembly with its exact authored companion names.
    fn project(&self) -> PathBuf {
        for name in COMPONENTS
            .iter()
            .copied()
            .chain(["solidworks-powerpackage.sldasm"])
        {
            std::fs::copy(fixture(name), self.0.join(name)).unwrap();
        }
        self.0.join("solidworks-powerpackage.sldasm")
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compute model extents from all transferred geometry positions.
fn bounds(scene: &Scene) -> [[f64; 3]; 2] {
    let mut range = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for point in scene.positions() {
        for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
            range[0][axis] = range[0][axis].min(value);
            range[1][axis] = range[1][axis].max(value);
        }
    }
    range
}

/// Compare analytical/native producer witnesses in physical millimetres.
fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} differs from {expected} by more than {tolerance}"
    );
}

/// Verify the original box against the producer's sketch dimensions and extrusion.
fn qualify_box(scene: &Scene) {
    assert_eq!(scene.triangles.len(), 12);
    let expected = [[-145., 0., -115.], [145., 200., 115.]];
    for (actual, expected) in bounds(scene)
        .into_iter()
        .flatten()
        .zip(expected.into_iter().flatten())
    {
        close(actual, expected, 0.000_02);
    }
    let area: f64 = scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            (b - a).cross(c - a).length() * 0.5
        })
        .sum();
    let volume: f64 = scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            a.dot(b.cross(c)) / 6.
        })
        .sum();
    close(area, 341_400., 0.1);
    close(volume, 13_340_000., 5.);
    for (index, triangle) in scene.triangles.iter().enumerate() {
        if let Some(normals) = scene
            .appearances
            .get(&index)
            .and_then(|appearance| appearance.normals)
        {
            let [a, b, c] = triangle.vertices;
            let normal = (b - a).cross(c - a).normalized();
            assert!(normals.iter().all(|source| normal.dot(*source) > 0.99));
        }
    }
}

/// Remove both addressed raster streams while retaining all native 3D records.
fn remove_images(path: &Path) {
    let mut compound = cfb::open_rw(path).unwrap();
    for name in ["/Preview", "/PreviewPNG"] {
        if compound.exists(name) {
            compound.remove_stream(name).unwrap();
        }
    }
}

/// Change one addressed native XML tree with exact native zlib framing preserved.
fn modify_xml(path: &Path, tree: &str, mutate: impl FnOnce(String) -> String) {
    let address = format!("/swXmlContents/{tree}__ZLB");
    let mut compound = cfb::open_rw(path).unwrap();
    let mut wrapper = Vec::new();
    compound
        .open_stream(&address)
        .unwrap()
        .read_to_end(&mut wrapper)
        .unwrap();
    let mut xml = String::new();
    flate2::read::ZlibDecoder::new(&wrapper[24..wrapper.len() - 8])
        .read_to_string(&mut xml)
        .unwrap();
    let xml = mutate(xml);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(xml.as_bytes()).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut replacement = wrapper[..16].to_vec();
    replacement.extend(u32::try_from(xml.len()).unwrap().to_le_bytes());
    replacement.extend(u32::try_from(compressed.len()).unwrap().to_le_bytes());
    replacement.extend(compressed);
    replacement.extend(&wrapper[wrapper.len() - 8..]);
    compound
        .create_stream(&address)
        .unwrap()
        .write_all(&replacement)
        .unwrap();
}

#[test]
fn original_native_box_has_physical_geometry_without_any_saved_image() {
    let source = fixture("solidworks-about.sldprt");
    let original = std::fs::read(&source).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&original))),
        "08a8ec2afc112a27b1b8f4fad92224080b5c0d8cda8fc095b423cf91ca062e34"
    );
    let scene = formats::load(&source).unwrap();
    qualify_box(&scene);
    let directory = Directory::new();
    let path = directory.0.join("box.SLDPRT");
    std::fs::copy(&source, &path).unwrap();
    remove_images(&path);
    assert!(catwalk3d::saved_preview::extract(&path).unwrap().is_none());
    let without_images = formats::load(&path).unwrap();
    assert_eq!(
        scene.positions().collect::<Vec<_>>(),
        without_images.positions().collect::<Vec<_>>()
    );
    qualify_box(&without_images);
    let image = geometry_thumbnail(
        &path,
        &RenderOptions {
            width: 96,
            height: 64,
            transparent: true,
            ..RenderOptions::default()
        },
    )
    .unwrap();
    assert_eq!(image.dimensions(), (96, 64));
    assert!(image.pixels().any(|pixel| pixel.0[3] != 0));
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn native_brep_fallback_preserves_box_when_every_display_cache_is_absent() {
    let directory = Directory::new();
    let path = directory.0.join("box.sldprt");
    std::fs::copy(fixture("solidworks-about.sldprt"), &path).unwrap();
    remove_images(&path);
    cfb::open_rw(&path)
        .unwrap()
        .remove_stream("/Contents/DisplayLists__ZLB")
        .unwrap();
    qualify_box(&formats::load(&path).unwrap());
}

#[test]
fn original_placed_assembly_matches_native_bbox_and_all_nine_occurrences() {
    let source = fixture("solidworks-powerpackage.sldasm");
    let original = std::fs::read(&source).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&original))),
        "2e71fb43b889a5d9585eb7606266a1dd35ef685244df6ee65782a7f7b2f19788"
    );
    let scene = ASSEMBLY.get_or_init(|| formats::load(&source).unwrap());
    let expected = [
        [
            -181.444_410_459_368_2,
            -109.147_613_096_922_4,
            207.000_000_000_000_1,
        ],
        [
            175.309_759_384_937_8,
            90.852_436_903_077_61,
            437.000_000_000_000_1,
        ],
    ];
    for (actual, expected) in bounds(scene)
        .into_iter()
        .flatten()
        .zip(expected.into_iter().flatten())
    {
        close(actual, expected, 0.000_1);
    }
    let mut triangles = 0;
    for (name, count) in COMPONENTS.iter().zip([1, 1, 1, 1, 2, 2, 1]) {
        triangles += formats::load(&fixture(name)).unwrap().triangles.len() * count;
    }
    assert_eq!(scene.triangles.len(), triangles);
    assert!(
        scene
            .appearances
            .values()
            .any(|appearance| appearance.normals.is_some())
    );
    let directory = Directory::new();
    let path = directory.project();
    remove_images(&path);
    for name in COMPONENTS {
        remove_images(&directory.0.join(name));
    }
    let without_images = formats::load(&path).unwrap();
    assert_eq!(
        scene.positions().collect::<Vec<_>>(),
        without_images.positions().collect::<Vec<_>>()
    );
    let image = geometry_thumbnail(
        &path,
        &RenderOptions {
            width: 80,
            height: 60,
            transparent: true,
            ..RenderOptions::default()
        },
    )
    .unwrap();
    assert_eq!(image.dimensions(), (80, 60));
    assert!(image.pixels().any(|pixel| pixel.0[3] != 0));
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn exact_companion_sources_and_license_are_retained() {
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture("geometry-provenance.json")).unwrap())
            .unwrap();
    assert_eq!(
        provenance["files"].as_array().unwrap().len(),
        COMPONENTS.len()
    );
    for file in provenance["files"].as_array().unwrap() {
        let bytes = std::fs::read(fixture(file["file"].as_str().unwrap())).unwrap();
        assert_eq!(bytes.len() as u64, file["bytes"].as_u64().unwrap());
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(bytes))),
            file["sha256"].as_str().unwrap()
        );
    }
    assert!(
        std::fs::read_to_string(fixture("POWER-PACKAGE-LICENSE.txt"))
            .unwrap()
            .contains("Copyright (c) 2022 GuoXiong")
    );
}

#[test]
fn missing_native_companion_does_not_return_saved_preview_pixels() {
    let directory = Directory::new();
    let path = directory.0.join("missing.sldasm");
    std::fs::copy(fixture("solidworks-powerpackage.sldasm"), &path).unwrap();
    assert!(catwalk3d::saved_preview::extract(&path).unwrap().is_some());
    assert!(
        formats::load(&path)
            .unwrap_err()
            .to_string()
            .contains("missing SolidWorks companion")
    );
}

#[test]
fn stale_configs_bad_joins_and_nonrigid_placements_fail_explicitly() {
    for (from, to, message) in [
        (
            "swConfigurationNeedsUpdate=\"NO\"",
            "swConfigurationNeedsUpdate=\"YES\"",
            "requires a native rebuild",
        ),
        (
            "swModelRef=\"5\"",
            "swModelRef=\"999\"",
            "model reference is unresolved",
        ),
        ("swFlexible=\"NO\"", "swFlexible=\"YES\"", "flexible"),
        (
            "swTransform=\"1 0 0 0 0 1 0 0 0 0 1 0",
            "swTransform=\"2 0 0 0 0 1 0 0 0 0 1 0",
            "non-rigid",
        ),
        (
            "swPath=\"C:\\Users\\123\\Desktop\\恒润实习\\SW\\电池包\\frontface.SLDPRT\"",
            "swPath=\"../frontface.SLDPRT\"",
            "traversal",
        ),
        (
            "swModelRef=\"5\"",
            "swModelRef=\"8\" swConfigurationId=\"1\"",
            "already defined",
        ),
    ] {
        let directory = Directory::new();
        let path = directory.0.join("bad.sldasm");
        std::fs::copy(fixture("solidworks-powerpackage.sldasm"), &path).unwrap();
        modify_xml(&path, "COMPINSTANCETREE", |xml| {
            assert!(xml.contains(from));
            xml.replacen(from, to, 1)
        });
        let error = formats::load(&path).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
    }
    let directory = Directory::new();
    let path = directory.0.join("stale.sldprt");
    std::fs::copy(fixture("solidworks-about.sldprt"), &path).unwrap();
    modify_xml(&path, "Features", |xml| {
        xml.replace(
            "swConfigurationNeedsUpdate=\"NO\"",
            "swConfigurationNeedsUpdate=\"YES\"",
        )
    });
    assert!(
        formats::load(&path)
            .unwrap_err()
            .to_string()
            .contains("requires a native rebuild")
    );
}

#[test]
fn component_reference_cycles_and_escaping_symlinks_are_refused() {
    let directory = Directory::new();
    let path = directory.0.join("cycle.sldasm");
    std::fs::copy(fixture("solidworks-powerpackage.sldasm"), &path).unwrap();
    modify_xml(&path, "COMPINSTANCETREE", |xml| {
        xml.replacen("frontface.SLDPRT", "cycle.sldasm", 1)
            .replacen("swDocType=\"PART\"", "swDocType=\"ASSEMBLY\"", 1)
    });
    assert!(
        formats::load(&path)
            .unwrap_err()
            .to_string()
            .contains("cyclic SolidWorks")
    );
    #[cfg(unix)]
    {
        let directory = Directory::new();
        let path = directory.0.join("escape.sldasm");
        std::fs::copy(fixture("solidworks-powerpackage.sldasm"), &path).unwrap();
        std::os::unix::fs::symlink(
            fixture("frontface.SLDPRT"),
            directory.0.join("frontface.SLDPRT"),
        )
        .unwrap();
        assert!(
            formats::load(&path)
                .unwrap_err()
                .to_string()
                .contains("escapes the supplied project")
        );
    }
}

#[test]
fn hidden_components_need_no_companion_and_keep_all_remaining_placements() {
    let directory = Directory::new();
    let path = directory.project();
    std::fs::remove_file(directory.0.join("frontface.SLDPRT")).unwrap();
    modify_xml(&path, "COMPINSTANCETREE", |xml| {
        xml.replacen("swHidden=\"NO\"", "swHidden=\"YES\"", 1)
    });
    let scene = formats::load(&path).unwrap();
    let original =
        ASSEMBLY.get_or_init(|| formats::load(&fixture("solidworks-powerpackage.sldasm")).unwrap());
    let hidden_triangles = 7396;
    assert_eq!(
        scene.triangles.len(),
        original.triangles.len() - hidden_triangles
    );
    assert_eq!(
        scene.positions().collect::<Vec<_>>(),
        original.triangles[hidden_triangles..]
            .iter()
            .flat_map(|triangle| triangle.vertices)
            .collect::<Vec<_>>()
    );
}
