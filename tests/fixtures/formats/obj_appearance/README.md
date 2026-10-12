# OBJ appearance fixtures

## Original Max2Obj building export

`regr01.obj` and `regr01.mtl` are unchanged files from
[Assimp commit `f6ee58e2489f3cbaee328655599f50c5c386a08e`](https://github.com/assimp/assimp/tree/f6ee58e2489f3cbaee328655599f50c5c386a08e/test/models/OBJ).
Their headers identify Max2Obj/Max2Mtl Version 4.0, dated March 10, 2001.
The upstream repository distributes `test/models` under its BSD-3-Clause license;
its separate model exception applies to `test/models-nonbsd`, which does not
contain these files. The complete original license is retained as
`ASSIMP-LICENSE`. These separately licensed assets are distinct from Catwalk's
MIT code and authored controls. No exporter, Assimp binary or network access is
needed to regenerate the corpus.

| Original resource | SHA-256 |
| --- | --- |
| `regr01.obj` | `35bff9dd9dced2282ff333be4cff907ea09679c3cded491a4ad261f3b3592cd8` |
| `regr01.mtl` | `9c7301cc37f2718d882f69cabedf998b6064442b8b00c49a22fa5ac00f17fbb7` |
| `ASSIMP-LICENSE` | `21195d410708b82757d3e34c3a40c427f6e59339da862e9480d4fcdc79b7a1bd` |

The original building has 2,108 positions, 688 texture coordinates, 2,710
explicit triangular faces, 111 group records and 12 diffuse materials. Four
zero-area faces are omitted, leaving 2,706 rendered triangles. Of those, 668
retain independently indexed UVs. No face supplies a normal and the materials
have no texture maps; this export qualifies geometry, diffuse materials, UVs and
window transparency. Authored controls below qualify supplied normals and image
sampling separately.

The original `Terraindæk` material name contains the legacy byte `0xe6` in both
OBJ and MTL. UTF-8-first, lossless Windows-1252 decoding preserves that name
without rewriting either source file. The 60 `Windows` triangles use diffuse
reflectivity `(0.6, 0.7, 0.8)` and opacity `0.5`; all other materials are opaque.
The emitted material counts are `Base=48`, `Site=40`, `Door=96`, `Floor=1336`,
`Rafter=96`, `Ridging=488`, `Sill=60`, `Roof=68`, `Terraindæk=68`,
`Wall-inner=56`, `Wall-out=290`, and `Windows=60`.

Independent source expectations are bounds
`[-194.19950867, -204.51156616, 0]` to
`[1442.08557129, 967.61529541, 337.5090332]`, surface area
`9677888.403887872`, and signed algebraic volume `24342521.61666515`.
The model has boundary and nonmanifold edges, so volume is a geometry checksum,
not a claim of watertight physical volume. Concatenating each ordered rendered
triangle's XYZ corners as little-endian IEEE-754 binary64 values gives SHA-256
`ec2c17ab5f5f9daecd2963a852615bdd9ca8bd56ff5995ccf5131522b5740760`.
The first UV-bearing triangle is emitted index 1496, with source position/UV
indices `937/10`, `939/12`, `940/11` and converted UVs
`[(1,1), (1,0), (0,0)]`. This one export does not establish compatibility with
every OBJ exporter or material feature.

## Authored appearance controls

`examples/fixtures/obj_appearance.rs` generates the following MIT-licensed
diagnostics, their MTL files, and deterministic Rust-encoded 8×8 PNG images:

- `obj-textured-concave.obj` is a tilted L with a first corner chosen so naive
  fan triangulation crosses the concavity. Its position, UV and normal indices
  differ, including negative position/UV indices. Six source RGB values remain
  per-corner multipliers. Independent area is `3 * sqrt(69) / 8`; the missing
  upper-right square must remain empty.
- `obj-texture-transform-border.obj` has four congruent cards: plain diffuse
  reflectivity, the same material with an opaque white map, transformed repeated
  quadrants, and transformed quadrants with `-clamp on`. The first two render
  identically when isolated. Coordinates outside the last map retain the
  underlying material, rather than extending its edge colors. Map scale is
  `(2, .5)`, offset is `(-.25, .125)`, and Wavefront's bottom-left V origin is
  converted once into the renderer's top-left origin.
- `obj-alpha-overlap.obj` contains opaque backing and two overlapping front
  cards. `d .5` supplies opacity .5; the common `Tr .75` extension supplies
  opacity .25. The snapshot includes both depth and dissolve effects.

The quadrant image is red/green on its top row and blue/yellow on its bottom row,
so a missing or repeated V inversion changes both attribute checks and the
snapshot. Tests separately inspect decoded authored image pixels, per-corner index
mapping and normals/color interpolation; PNG comparisons cover the complete
fixed-view output. Linear MTL diffuse reflectivity is distinct from the retained
sRGB vertex-color convention. Specular, ambient, bump and illumination-model
effects are outside this diffuse thumbnail subset.
