//! Original Web3D local prototypes and authored expansion/scope/facing controls.

use catwalk3d::Result;
use std::{fs, path::Path};

/// Corpus directory containing the original and authored static prototype examples.
const DIR: &str = "vrml_prototypes";
/// Authored diagnostic documents use the same MIT license as Catwalk's code.
const AUTHORED: &str =
    "Author-created deterministic Rust fixture; MIT, matching the project license";
/// Explicit upstream licensing, original authorship and exact-byte qualification.
const ORIGINAL: &str = "Unmodified original Web3D Consortium VRML97 example, with its embedded author/date/license metadata preserved. Explicit Web3D Consortium open-source BSD-style redistribution terms retained as vrml_prototypes/WEB3D-LICENSE.txt; exact source URLs, original SHA-256 hashes and independent static geometry expectations in vrml_prototypes/README.md. https://www.web3d.org/x3d/content/examples/";

/// Encode typed coordinate/color defaults and independently placed instance overrides.
fn typed_arrays() -> &'static [u8] {
    br"#VRML V2.0 utf8
# Catwalk authored typed prototype control; MIT.
PROTO ColoredMesh [
  field MFVec3f positions [0 0 0, 2 0 0, 0 3 0]
  field MFInt32 indices [0 1 2 -1]
  field MFColor hues [1 0 0, 1 0 0, 1 0 0]
] {
  Shape {
    appearance Appearance { material Material { diffuseColor 1 1 1 } }
    geometry IndexedFaceSet {
      coord Coordinate { point IS positions }
      coordIndex IS indices
      color Color { color IS hues }
    }
  }
}
Transform { translation -3 0 0 children [ColoredMesh {}] }
Transform {
  translation 2 1 0
  children [
    ColoredMesh {
      positions [0 0 0, 1 0 0, 0 0 2]
      hues [1 0 0, 0 1 0, 0 0 1]
    }
  ]
}
"
}

/// Encode geometry/material inherited types while excluding later auxiliary scene graphs.
fn first_body() -> &'static [u8] {
    br"#VRML V2.0 utf8
# Catwalk authored first-body prototype control; MIT.
PROTO GeometryPiece [field SFVec3f dimensions 2 1 3] {
  Box { size IS dimensions }
  Shape { geometry Box { size 100 100 100 } }
}
PROTO Paint [field SFColor tone 0.25 0.5 1] {
  Material { diffuseColor IS tone }
  Shape { geometry Box { size 100 100 100 } }
}
Transform {
  translation -3 0 0
  children [
    Shape {
      appearance Appearance { material Paint {} }
      geometry GeometryPiece {}
    }
  ]
}
Transform {
  translation 3 1 2
  children [
    Shape {
      appearance Appearance { material Paint { tone 1 0.5 0.25 } }
      geometry GeometryPiece { dimensions 1 2 1 }
    }
  ]
}
"
}

/// Encode nested templates, per-instance body definitions and caller-scoped node overrides.
fn nested_scopes() -> &'static [u8] {
    br"#VRML V2.0 utf8
# Catwalk authored prototype scope and repeated-instance control; MIT.
PROTO Pair [
  field SFVec3f move 0 0 0
  field MFNode extras []
] {
  PROTO LocalTriangle [field SFColor tint 1 0 0] {
    Shape {
      appearance Appearance { material Material { diffuseColor IS tint } }
      geometry IndexedFaceSet {
        coord Coordinate { point [0 0 0, 1 0 0, 0 1 0] }
        coordIndex [0 1 2 -1]
      }
    }
  }
  Transform {
    translation IS move
    children [
      DEF LocalInstance LocalTriangle {}
      Transform { translation 2 0 0 children [USE LocalInstance] }
      Group { children IS extras }
    ]
  }
  Shape { geometry Box { size 100 100 100 } }
}
Pair {
  move -4 0 0
  extras [
    DEF CallerShape Shape {
      appearance Appearance { material Material { diffuseColor 0 1 0 } }
      geometry Box { size 1 1 1 }
    }
  ]
}
Pair { move 2 1 1 }
Transform { translation -4 0 2 children [USE CallerShape] }
"
}

/// Encode asymmetric constrained/free billboards and a reused transformed occurrence.
fn billboard_occurrences() -> &'static [u8] {
    br"#VRML V2.0 utf8
# Catwalk authored fixed-view billboard occurrence control; MIT.
Transform {
  translation -4 0 0
  children [
    DEF Facing Billboard {
      children [
        Shape {
          appearance Appearance { material Material { diffuseColor 1 0.25 0.25 } }
          geometry IndexedFaceSet {
            coord Coordinate { point [0 0 0, 2 0 0, 0 3 0] }
            coordIndex [0 1 2 -1]
          }
        }
        Shape {
          appearance Appearance { material Material { emissiveColor 1 0.25 0.25 } }
          geometry PointSet { coord Coordinate { point [0 0 2] } }
        }
      ]
    }
  ]
}
Transform {
  translation 3 0 0
  children [
    Billboard {
      axisOfRotation 0 0 0
      children [
        Shape {
          appearance Appearance { material Material { diffuseColor 0.25 1 0.25 } }
          geometry IndexedFaceSet {
            coord Coordinate { point [0 0 0, 2 0 0, 0 3 0] }
            coordIndex [0 1 2 -1]
          }
        }
        Shape {
          appearance Appearance { material Material { emissiveColor 0.25 1 0.25 } }
          geometry PointSet { coord Coordinate { point [0 0 2] } }
        }
      ]
    }
  ]
}
Transform {
  translation -4 0 4
  rotation 0 1 0 1.5707963267948966
  scale 3 2 1
  children [USE Facing]
}
"
}

/// Verify that a written input has geometry before registering its snapshot/gallery case.
fn register(
    root: &Path,
    name: &str,
    id: &str,
    extension: &str,
    title: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{name}");
    let scene = catwalk3d::formats::load(&root.join(&input))?;
    if scene.triangles.is_empty() && scene.lines.is_empty() && scene.points.is_empty() {
        return Err(format!("VRML prototype fixture {input} has no geometry").into());
    }
    Ok(crate::fixture(
        id, extension, title, &input, "model", provenance,
    ))
}

/// Copy exact licensed originals and write four deterministic prototype/facing controls.
///
/// # Errors
///
/// Writing inputs/notices or loading their required geometry can fail.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    for (name, bytes) in [
        (
            "Figure31_9SpinGroupPrototype.wrl",
            include_bytes!(
                "../../tests/fixtures/formats/vrml_prototypes/Figure31_9SpinGroupPrototype.wrl"
            )
            .as_slice(),
        ),
        (
            "ProtoInstanceExample.wrl",
            include_bytes!("../../tests/fixtures/formats/vrml_prototypes/ProtoInstanceExample.wrl")
                .as_slice(),
        ),
        (
            "WEB3D-LICENSE.txt",
            include_bytes!("../../tests/fixtures/formats/vrml_prototypes/WEB3D-LICENSE.txt")
                .as_slice(),
        ),
    ] {
        fs::write(directory.join(name), bytes)?;
    }
    let mut cases = Vec::new();
    for (name, id, title) in [
        (
            "Figure31_9SpinGroupPrototype.wrl",
            "wrl-web3d-spingroup",
            "Web3D original nested SpinGroup prototype",
        ),
        (
            "ProtoInstanceExample.wrl",
            "wrl-web3d-proto-instance",
            "Web3D original typed cylinder prototype and billboard",
        ),
    ] {
        cases.push(register(root, name, id, "wrl", title, ORIGINAL)?);
    }
    for (id, extension, title, bytes) in [
        (
            "vrml-proto-typed-arrays",
            "vrml",
            "VRML typed coordinate/color defaults and overrides",
            typed_arrays(),
        ),
        (
            "wrl-proto-first-body",
            "wrl",
            "VRML first-body geometry and material wrappers",
            first_body(),
        ),
        (
            "wrl-proto-nested-scopes",
            "wrl",
            "VRML nested templates, body-local definitions and caller nodes",
            nested_scopes(),
        ),
        (
            "wrl-billboard-occurrences",
            "wrl",
            "VRML constrained/free billboards and transformed shared instances",
            billboard_occurrences(),
        ),
    ] {
        let name = format!("{id}.{extension}");
        fs::write(directory.join(&name), bytes)?;
        cases.push(register(root, &name, id, extension, title, AUTHORED)?);
    }
    Ok(cases)
}
