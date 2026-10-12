# VRML local prototype fixtures

## Unchanged Web3D originals

The two original VRML97 documents below are retained byte for byte from the
Web3D Consortium example archive. Both preserve their original author, date,
generator and explicit license metadata. They are published examples translated
from X3D to VRML97, rather than newly authored Catwalk files.

| Input | Original source | SHA-256 |
| --- | --- | --- |
| `Figure31_9SpinGroupPrototype.wrl` | [VRML 2 Sourcebook, Chapter 31](https://www.web3d.org/x3d/content/examples/Vrml2Sourcebook/Chapter31Prototypes/Figure31_9SpinGroupPrototype.wrl) | `a054724500cace784c4e04af48d3fe283496eedb84236d978ab775b1c8a6c4d1` |
| `ProtoInstanceExample.wrl` | [Basic development examples](https://www.web3d.org/x3d/content/examples/Basic/development/ProtoInstanceExample.wrl) | `cfe5ea5413f73180f2d2501c9d6d5d408e861a9ba2b3fb46ba5fe6a8c572df4d` |
| `WEB3D-LICENSE.txt` | [Original Web3D license text](https://www.web3d.org/x3d/content/examples/license.txt) | `0c90db51ca37d07075567650ac50d8f338d462c4b0f6ae449b6403b6eb237e4c` |

SpinGroup credits Andrea L. Ames, David R. Nadeau and John L. Moreland's 1997
VRML 2.0 Sourcebook example, translated by Don Brutzman. ProtoInstanceExample
credits Don Brutzman and was created in June 2000. Both record a modification
date of 20 October 2019 and explicitly reference the Web3D Consortium open-source
license. That license permits source/binary redistribution while requiring the
author copyright notice, conditions and disclaimer to be retained. Its exact
text is kept as [WEB3D-LICENSE.txt](WEB3D-LICENSE.txt); the original embedded
metadata remains unchanged. Catwalk's MIT license applies separately to its
code and authored controls.

Independent initial-geometry expectations:

- SpinGroup creates three rectangular boxes with source dimensions `(25,2,2)`,
  `(2,25,2)` and `(2,2.05,25)`. Nested MFNode overrides, local body definitions
  and shared caller appearance nodes must retain all three. Expected output is
  36 triangles, two green boxes `[0,255,77]` and one blue box `[0,77,255]`.
  Final bounds are `[-12.5,-12.5,-12.5]` to `[12.5,12.5,12.5]`; authored surface
  area is `626.7`, and summed signed volume is `302.5`. The boxes overlap;
  this volume sums their separate surfaces and is not a boolean union volume.
  Timer/ROUTE animation is not evaluated by the static preview.
- ProtoInstanceExample overrides its red appearance with green and changes
  the cylinder's radius to four while retaining height three. Typed SFNode and
  SFFloat overrides are applied through `IS`. It contains a default-Y-axis
  Billboard; Catwalk's fixed-view facing rotates this case by 45 degrees around
  that axis. The existing 32-sector cylinder is invariant at that sampled
  rotation. Expected output is 128 triangles, color `[0,255,0]`, bounds
  `[-4,-4,-1.5]` to `[4,4,1.5]`, polygonal surface area `175.16340864536022`
  and signed volume `149.8293673083865`. Those mesh measures differ from the
  ideal continuous cylinder's area `56*pi` and volume `48*pi`.

These originals qualify their static prototype features. They do not establish
Script execution, external prototypes, animation or arbitrary X3D compatibility.

## Four authored scope, type and facing controls

`examples/fixtures/vrml_prototypes.rs` writes four deterministic MIT-licensed
inputs in Rust. No Web3D download, browser or native parser is needed during
regeneration. Literal coordinate and color expectations are independently
checked through Catwalk's public loader; the corpus also compares full PNGs.

- `vrml-proto-typed-arrays.vrml`: MFVec3f/MFInt32/MFColor defaults and overrides
  populate actual coordinate, index and color arrays. The default red triangle
  has final vertices `(-3,0,0)`, `(-1,0,0)`, `(-3,0,3)` and area three. The
  overridden triangle has final vertices `(2,0,1)`, `(3,0,1)`, `(2,-2,1)`,
  area one and averaged RGB `[85,85,85]`. Expected total is two triangles with
  area four and bounds `[-3,-2,0]` to `[3,0,3]`.
- `wrl-proto-first-body.wrl`: GeometryPiece inherits Box and Paint inherits
  Material. Each later auxiliary Shape contains a huge box and must remain
  outside the transformation hierarchy. The two actual boxes have final
  centers `(-3,0,0)` and `(3,-2,1)`, dimensions `(2,3,1)` and `(1,1,2)`, and
  colors `[64,128,255]` and `[255,128,64]`. Expected total is 24 triangles,
  area 32, signed volume eight and bounds `[-4,-2.5,-.5]` to `[3.5,1.5,2]`.
- `wrl-proto-nested-scopes.wrl`: Pair has a local LocalTriangle prototype,
  body-local DEF/USE and an MFNode extras field. Two instances create four red
  triangles without leaking or colliding their private definition names. A
  green unit box supplied with caller DEF CallerShape remains available to an
  outside USE, producing two box instances at final centers `(-4,0,0)` and
  `(-4,-2,0)`. Expected total is 28 triangles, area 14 and bounds
  `[-4.5,-2.5,-.5]` to `[5,.5,2]`. Open triangles are not assigned a whole-scene
  physical volume. Pair's later auxiliary huge box is excluded.

The fourth control, `wrl-billboard-occurrences.wrl`, contains an asymmetric
triangle and an out-of-plane point marker. It checks default-Y constrained
facing, free-axis alignment and reuse under a translated, rotated and
nonuniformly scaled parent. Red geometry uses `[255,64,64]`, green geometry
uses `[64,255,64]`, and the reused occurrence remains red. The literal final
triangle vertices are:

| Occurrence | A | B | C |
| --- | --- | --- | --- |
| Constrained | `(-4,0,0)` | `(-4+sqrt(2),sqrt(2),0)` | `(-4,0,3)` |
| Free-axis | `(3,0,0)` | `(3+sqrt(2),sqrt(2),0)` | `(3-3/sqrt(6),3/sqrt(6),6/sqrt(6))` |
| Rotated/stretched reuse | `(-4,-4,0)` | `(-4+2/sqrt(10),-4+18/sqrt(10),0)` | `(-4,-4,6)` |

The three markers are `(-4+sqrt(2),-sqrt(2),0)`,
`(3+2/sqrt(3),-2/sqrt(3),2/sqrt(3))` and
`(-4+6/sqrt(10),-4-6/sqrt(10),0)`. Expected output is three triangles and
three points, total area `6+6*sqrt(8.2)`, and bounds
`[-4,-4-6/sqrt(10),0]` to `[3+sqrt(2),-4+18/sqrt(10),6]`.
The marker points ensure the renderer selects its primary reference view.
Billboard orientation is frozen at import against that view; an entirely
edge-on surface-only scene may later select an alternate renderer view without
reevaluating Billboard orientation. No authored camera or event animation is
used.

Prototype type/category links follow the directional rules in
[VRML97 Table 4.4](https://www.web3d.org/documents/specifications/14772/V2.0/part1/concepts.html#4.8).
An interface field can bind a body's field or exposedField; an interface
exposedField cannot bind a body's field such as Box.size. Defaults, overrides,
node identity/scope and first-body semantics are tested separately from ignored
runtime events. Regeneration never changes PNG baselines.
