# Standalone binary and ASCII BREP fixtures

## Unchanged F3D counterpart files

`f3d.bin.brep` and `f3d.brep` are unmodified files from
[f3d-app/f3d commit `89464637f12acaed870c46cf5cdeda3b8411841b`](https://github.com/f3d-app/f3d/tree/89464637f12acaed870c46cf5cdeda3b8411841b/testing/data).
The binary file uses standalone Open CASCADE native topology version 4; its ASCII
counterpart uses CASCADE topology version 3. Both describe the same compound of
two closed tetrahedral shells. These are original upstream test assets, rather
than newly generated Catwalk files.

The exact upstream [F3D-LICENSE.md](F3D-LICENSE.md) and
[F3D-DATA_LICENSES.md](F3D-DATA_LICENSES.md) are retained without modification.
The data mapping states that all datasets without another listed license use
F3D's BSD-3-Clause license; neither of these BREP files has an exception. Retain
the original copyright notices, conditions and disclaimer when redistributing
the inputs. Catwalk's MIT license applies separately to its code and authored
diagnostic inputs.

| Retained file | Original pinned source path | SHA-256 |
| --- | --- | --- |
| `f3d.bin.brep` | `testing/data/f3d.bin.brep` | `0c1788d106be3473758bfc81013667f976d4b319378aab2e79e77eae91912265` |
| `f3d.brep` | `testing/data/f3d.brep` | `f1a8028786df34e897e9b8cdb0cddb3d9d4e39e7431e3126092075d98a3e5201` |
| `F3D-LICENSE.md` | `LICENSE.md` | `42e90e43576bfcc8945239179c49141c387c407a3a3c3696789d9f061cc4afb4` |
| `F3D-DATA_LICENSES.md` | `testing/data/DATA_LICENSES.md` | `4229d1453e125ace50ca1825d420340bbc6b2ee4f2e6ffabde8709052534e12a` |

Independent source expectations are eight Cartesian vertices, twelve edges,
eight triangular wires/faces, two shells and one compound: 39 native topology
records. Both previews must retain all eight oriented triangles and their exact
corner associations. Bounds are `[-2, -1.75, -4.5]` to `[2, 1.75, 4.5]`;
surface area is `55.018917559102576`, and summed signed volume is `12.5`.
The integration test hardcodes the original vertex coordinates and face loops
independently of Catwalk's binary decoder, then checks both inputs and their
fixed-view rendered images. The corpus gives each format its own PNG baseline.

## Authored native table controls

`examples/fixtures/binary_brep.rs` writes four deterministic MIT-licensed inputs
directly in Rust, following the published
[OCCT BinTools layout](https://github.com/Open-Cascade-SAS/OCCT/tree/V7_9_0/src/BinTools).
Headers, elementary locations, binary64 scalars, signed 32-bit indexes, shape
flags, backward references and the final root occurrence are encoded explicitly
in little-endian order. No CAD application or native writer is required.
These are authored layout controls, not additional exporter originals.

- `brep-binary-v1-points.brep`: three isolated points translated by the root to
  `(3,-2,1)`, `(5,-2,1)` and `(3,-1,1)`.
- `brep-binary-v2-wire.brep`: an L-shaped free wire whose curve carrier rotates
  and translates the actual analytic line geometry. Expected endpoints are
  `(1,2,3)` to `(1,4,3)` to `(0,4,3)`, with two segments and total length three.
- `brep-binary-v3-placed-wire-points.brep`: a reflected curve carrier followed
  by a separate rotated/translated root. The wire runs `(2,-1,4)` to `(2,-3,4)`
  to `(1,-3,4)`; the isolated point is `(4,5,3)`. It retains two segments of
  total length three and the point in the same compound.
- `brep-binary-v4-repeated-wire.brep`: two occurrences of one wire have
  different child placements and share a uniformly scaled/translated root.
  Expected endpoints are `(12,4,5)` to `(16,4,5)` to `(16,6,5)` and
  `(20,4,7)` to `(24,4,7)` to `(24,6,7)`. Four segments have total length twelve.

The wire and point controls have no cached polygons, triangulations or optional
point-on-curve representations. They do not establish compatibility with all
historical version-1/2 optional vertex encodings. The genuine counterpart pair
additionally exercises planar surfaces, trimming pcurves and complete shell
topology. Format-specific exclusions and bounded tessellation still apply.

Regeneration copies the original bytes and writes the authored inputs using
Rust only. It never changes PNG baselines; rendered results require separate
review before a snapshot update.
