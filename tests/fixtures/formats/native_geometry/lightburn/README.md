# LightBurn geometry without saved thumbnails

Every project in this directory has **no `Thumbnail` element**. The eight
external fixtures are unchanged source files, with pinned source URLs and
SHA-256 digests in [provenance.json](provenance.json). Their original MIT license
texts are retained alongside them. `classic-geometry.lbrn` is an authored MIT
fixture with a classic cubic path, rounded rectangle, ellipse, nested affine
placement and three layer colours.

| Fixture | Geometry exercised |
| --- | --- |
| `line.lbrn2` | Native 1.7.08 two-point path and nonuniform affine transform |
| `square.lbrn2` | Native 1.7.08 centred rectangle and translation |
| `circle.lbrn2` | Native 1.7.08 ellipse with independently checked circle radii |
| `rings.lbrn2` | Native 1.7.08 independent vertex/primitive resource IDs, reused paths, groups, line closures and Beziers |
| `crucifix.lbrn2` | Native 1.7.08 four curved instances using one shared vertex/primitive pair; independent placement and colour checks |
| `butterfly_vectorized.lbrn2` | External vectorized grouped cubic path; source does not record an application version |
| `board-with-soldermask-only.lbrn2` | External 1.7.03-labelled generated PCB contours: 72 nondegenerate line segments |
| `repro04-motor-controller.lbrn2` | External 1.7.03-labelled generated PCB: 662 paths, 29,953 nondegenerate segments and nested groups |
| `classic-geometry.lbrn` | Authored classic V/P syntax: Bezier arch, transformed group, ellipse and rounded rectangle |

The PNG files in `snapshots/` are newly rendered 256 × 256 XY plan previews of
these vectors, with transparent backgrounds. They contain no pixels extracted
from a document image. Black outlines are best inspected on a light background.
Integration tests check every source hash, reject any accidentally introduced
thumbnail carrier, check geometry independently, and compare premultiplied RGBA
against these images with a maximum per-channel tolerance of two. A separate
CLI test inserts a corrupt thumbnail and proves `--render-geometry` still
renders the correct geometry without a display server.

```sh
cargo test --locked --lib formats::lightburn::tests
cargo test --locked --test lightburn_geometry
cargo run --locked -- tests/fixtures/native_geometry/lightburn/rings.lbrn2 \
  --render-geometry --transparent --resolution 256,256 --output rings.png
```

The implementation reads millimetre coordinates from either classic `V`/`P`
nodes or compact `VertList`/`PrimList` strings. Cubic segments use the start
vertex's `c0` handle and the destination vertex's `c1` handle. Independent
`VertID` and `PrimID` caches preserve reuse, including definitions later in the
document. Parent and child affine transforms are composed in drawing order;
document `MirrorX`/`MirrorY` flags are applied before converting screen Y to
the renderer's upward Y axis. Beziers are subdivided adaptively, including
collinear overshoots and closed curves.

Producer explanations and original implementation research:

- [LightBurn staff explanation of classic V/P cubic geometry](https://forum.lightburnsoftware.com/t/lbrn-or-lbrn2-xml-file-docuumentation/42317).
- [LightBurn staff explanation of affine matrix order and origin mirroring](https://forum.lightburnsoftware.com/t/lbrn2-file-documentation/52174).
- [LightBurn file-management reference describing legacy and current project types](https://docs.lightburnsoftware.com/latest/Reference/FileManagement/).
- [Original lbrn2-to-svg compact-data parser](https://github.com/jlucaso1/lbrn2-to-svg/blob/7cea0bf68b7caf4ad8d9b16ea03bd8a90c2c9c49/src/lbrn2Parser.ts).
- [Original lbrnts rectangle model](https://github.com/tscircuit/lbrnts/blob/d21a7b29354cd782cabf533b1b8f0ade6a11c7dd/lib/classes/elements/shapes/ShapeRect.ts).

This is vector-outline support for a **2D drawing format**. Laser scan fills,
kerf compensation, tab/perforation toolpaths, bitmap composition, font shaping,
text backup outlines, variable text, multi-page projects and user-configured
display palettes are not implemented. Text, bitmap, unknown shape types and
unknown shape containers return explicit errors. The first sixteen standard
cut-layer colours are retained; other layer indices currently use black.
Explicit future format versions and namespaces are rejected. Input, XML-node,
shared-resource, output-segment and group-depth limits bound parsing and
tessellation. Supported projects require no external application or subprocess.
