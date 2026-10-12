# COLLADA static primitive fixtures

## Unchanged Khronos conformance models

The following XML files are retained without modification at
[Khronos COLLADA-CTS commit `61f2a560cbb2a06ee62da8025241f6b08d06bfd9`](https://github.com/KhronosGroup/COLLADA-CTS/tree/61f2a560cbb2a06ee62da8025241f6b08d06bfd9/StandardDataSets/collada/library_visual_scenes/visual_scene/node).
They preserve their original Maya 8.5, ColladaMaya v3.02 and FCollada v3.2 metadata
and the complete embedded Khronos 2012 copyright/permission notice. They are
published conformance resources, rather than newly created exports or authored
Catwalk inputs. The unchanged upstream license is `KHRONOS-CTS-LICENSE`.
Catwalk's MIT license applies separately to its code and authored diagnostics.

| Resource | Original CTS path below `node` | SHA-256 |
| --- | --- | --- |
| `node_lookat4.dae` | `lookat/node_lookat4/node_lookat4.dae` | `48f2ac8f317f591a4af93910896f967dc7541fb5582342f5e4178a1028a8d429` |
| `node_skew_cube.dae` | `skew/node_skew_cube/node_skew_cube.dae` | `6b5f6a0b3b9e859e3a20aada2efc12adfaa779369ba2d7f87fc0889f4aa24c6b` |
| `KHRONOS-CTS-LICENSE` | [Root `LICENSE.md`](https://github.com/KhronosGroup/COLLADA-CTS/blob/61f2a560cbb2a06ee62da8025241f6b08d06bfd9/LICENSE.md) | `0c416509cf218accdbc54afc342271935b0ecdc1398283129a32375aa7bcc9a4` |

Independent geometry expectations derive from the original position/index arrays,
centimetre units, Y-up conversion and published node-transform conventions:

- Look-at places a five-vertex, six-triangle pyramid. Bounds are
  `[-.75, -.5, -.5]` to `[1.25, .5, .5]`; surface area is
  `(1 + sqrt(33)) / 2`. The original inward winding has signed algebraic volume
  `-1/3`. The local look-at basis points forward along positive X.
- Skew transforms the twelve-triangle unit cube as
  `z' = z + (x+y)/sqrt(2)` before up-axis conversion. Final bounds are
  `±[.5, .5+sqrt(.5), .5]`, surface area is `4+2*sqrt(2)`, and signed volume is
  one. Rotation and translation axes are used in their original XML order.

The complete XML is retained, including its cameras, lights and material
metadata. Thumbnails use Catwalk's fixed view and documented diffuse-color
subset. These two conformance models establish the tested static affine cases;
they do not establish broad exporter, controller or shader compatibility.

## Authored geometry controls

`examples/fixtures/collada_primitives.rs` generates four deterministic
MIT-licensed documents with fixed timestamps:

- `dae-holed-plane.dae`: a tilted four-by-three rectangle with two disjoint
  rectangular holes of areas one and one half in XY. The plane is
  `z=x/4+y/8`. Expected output is fourteen consistently wound triangles with
  area `10.5*sqrt(69)/8`; the holes must remain open.
- `dae-wire-instances.dae`: eight colored indexed segments and one closed
  four-edge strip, instantiated twice with a translation and mirrored,
  nonuniform scale. Expected output is twenty-four segments. Color inputs use
  arbitrary parameter names in component order, independently indexed from
  positions. Units are decimetres and positions remain Z up.
- `dae-lookat-placement.dae`: a tetrahedron placed by nested translation,
  nonorthogonal-up look-at and nonuniform scale, using millimetre units and Y up.
  Its accessor has two prefix scalars, an unnamed skipped slot and three
  arbitrarily named component slots. Expected bounds are
  `[.001, -.008, 0]` to `[.005, -.004, .009]` and signed volume is `24e-9`.
- `dae-nonorthogonal-skew.dae`: a tetrahedron with rotation axis `(1,1,0)`,
  translation axis `(1,0,0)` and angular shear `18.43494882292201` degrees.
  Independently expected placement is `x'=x+y`, with Y/Z unchanged; signed
  volume remains one. This case checks the general angular convention.

Independent tests inspect source integrity, every selected CTS triangle corner,
surface area and winding, wire endpoint/count/color/placement properties, hole
exclusion, ordered accessor slots and affine placement. Invalid or unsupported
controls check counts, indices, zero-area/nonplanar holes and singular transforms.
The format corpus separately compares complete rendered PNGs for all six models.
Regeneration requires only Rust and the retained source files; no upstream
exporter, external renderer or network access is needed.
