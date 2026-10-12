//! Unchanged Google producer meshes qualify authored corner normals independently.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    Result, formats,
    scene::{Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// The upstream unit cube uses six outward axis directions in its OBJ normal records.
const DIRECTIONS: [Vec3; 6] = [
    Vec3::new(-1., 0., 0.),
    Vec3::new(1., 0., 0.),
    Vec3::new(0., -1., 0.),
    Vec3::new(0., 1., 0.),
    Vec3::new(0., 0., -1.),
    Vec3::new(0., 0., 1.),
];

/// Qualify the unit cube against producer coordinates, surface area, winding and normals.
fn qualify_cube(scene: &Scene, normal_order: [usize; 12]) {
    assert_eq!(scene.triangles.len(), 12);
    assert_eq!(scene.appearances.len(), 12);
    assert!(scene.lines.is_empty() && scene.points.is_empty());
    let mut area = 0.;
    let mut volume = 0.;
    let mut counts = [0; 6];
    for (index, triangle) in scene.triangles.iter().enumerate() {
        assert!(triangle.color.is_none());
        let [a, b, c] = triangle.vertices;
        for position in [a, b, c] {
            for coordinate in [position.x, position.y, position.z] {
                assert!(
                    coordinate.to_bits() == 0_f64.to_bits()
                        || coordinate.to_bits() == 1_f64.to_bits()
                );
            }
        }
        let normal = DIRECTIONS[normal_order[index]];
        let cross = (b - a).cross(c - a);
        assert!((cross.length() - 1.).abs() < 1e-12);
        assert!((cross - normal).length() < 1e-12);
        assert_eq!(scene.appearances[&index].normals, Some([normal; 3]));
        assert!(scene.appearances[&index].colors.is_none());
        assert!(scene.appearances[&index].texture.is_none());
        assert!(scene.appearances[&index].texcoords.is_none());
        // An outward unit-cube face lies at coordinate zero for a negative
        // normal and one for a positive normal, independently of point order.
        let plane = normal.x.max(normal.y).max(normal.z).max(0.);
        for position in [a, b, c] {
            assert!((position.dot(normal) - plane).abs() < 1e-12);
        }
        counts[normal_order[index]] += 1;
        area += cross.length() / 2.;
        volume += a.dot(b.cross(c)) / 6.;
    }
    assert_eq!(counts, [2; 6]);
    assert!((area - 6.).abs() < 1e-12);
    assert!((volume - 1.).abs() < 1e-12);
}

#[test]
fn unchanged_google_encodings_preserve_authored_normal_geometry_associations() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/draco");
    for (name, length, hash, normal_order) in [
        (
            "cube.drc",
            301,
            "327d95888f62a45f9579a9020701afc29d0d6c74e69406ed8b5154ba4a307f70",
            [3, 5, 5, 2, 0, 0, 4, 3, 1, 1, 2, 4],
        ),
        (
            "cube-valence.drc",
            199,
            "f90c73777a7d0b2d74c4128364428014527f2fce54998d220be360a466ff7caa",
            [5, 5, 2, 2, 1, 1, 3, 3, 0, 0, 4, 4],
        ),
        (
            "cube-sequential.drc",
            220,
            "a97cf55c8d41e3401564d47ffc636dbeb37d6859c59b16e55e71e6111657a433",
            [4, 4, 0, 0, 3, 3, 1, 1, 2, 2, 5, 5],
        ),
    ] {
        let path = root.join(name);
        let bytes = fs::read(&path)?;
        assert_eq!(bytes.len(), length);
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
            hash
        );
        qualify_cube(&formats::load(&path)?, normal_order);
    }
    // The existing gallery file remains the original Google valence stream.
    assert_eq!(
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/formats/mesh_scene/draco-google-cube.drc")
        )?,
        fs::read(root.join("cube-valence.drc"))?,
    );
    Ok(())
}
