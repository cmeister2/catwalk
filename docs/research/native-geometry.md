# Native document geometry without embedded thumbnails

Catwalk generates fresh previews from the qualified Blender, LightBurn and DWG
geometry described below. Inventor and Fusion solid decoding remains in a
separately built experimental tool with retained qualification evidence.

| Documents | Proven result | Remaining work |
| --- | --- | --- |
| Blender `.blend` | Integrated static mesh reader; six real no-thumbnail scenes from 2.52 through 5.2 have independent geometry checks and image snapshots. | Modifiers, shape keys, non-mesh surfaces, external libraries, materials and broader visibility/rotation qualification. |
| LightBurn `.lbrn`, `.lbrn2` | Integrated XML vector reader; nine no-thumbnail projects render in XY, including shared Beziers and a 662-path PCB. | Text/fonts, bitmaps, fills, custom palettes and multipage projects. This is a 2D drawing format. |
| AutoCAD `.dwg` | Integrated native Rust record decoder feeding the static DXF reader; five genuine line drawings match independent AutoCAD coordinates. A 2018 drawing originally without an image renders automatically. | ACIS solid/region/body tessellation, additional drawing entities and section-limit qualification. Current genuine tests establish line drawing support, not general CAD solids. |
| Inventor `.ipt` | Separate experimental pure Rust solid bridge, including a current Zstandard-compressed cylinder and an older part after only the saved image is removed. | Normal CLI integration; broader shapes, schemas and release qualification. |
| Inventor `.iam` | Container investigation; complete assembly rendering remains unsupported. | External part resolution, occurrence placement, suppression/visibility and complete independent assembly fixtures. |
| Fusion `.f3d` | Separate experimental pure Rust solid bridge; a real preview-free 107-face extrusion renders through STEP tessellation. | Normal CLI integration, unresolved design contexts, assembly placement and additional surface/trim variants. |

## Integrated readers and tests

`--render-geometry` and `catwalk::geometry_thumbnail` never consult a saved
image. Normal thumbnails retain existing saved-image behavior when a valid
image is present; the four integrated native extensions attempt geometry when
that image is absent or unreadable. Explicit `--extract-saved-preview` keeps
the extraction-only contract and exit status 3 for absent images.

The CI corpus includes twenty fresh-geometry cases alongside the other model
and image cases. Every native geometry extension must have a fresh-render snapshot,
independently of saved-image coverage. The CLI forces geometry for these
cases, compares decoded PNG pixels, and supplies actual/expected/difference
images to the gallery's **Fresh geometry** filter. The workflow continues to
run on Linux, macOS and Windows. The experimental solid bridge has a separate
Linux CI job with its own geometry and image comparisons.

Blender's [manifest](../../tests/fixtures/native_geometry/blend/manifest.json)
records original, decompressed and derived hashes. One BSD Assimp 2.52 source
naturally has no thumbnail. The other Apache Khronos sources have only their
`TEST` image block removed; geometry bytes are retained. Original Rust code
reads BHead and SDNA, ID-local pointers, old `MFace`, `MPoly`/`MLoop` and modern
attribute arrays, shared meshes, static parent transforms, collection
instances and loose edges/points. Independently checked topology, bounds and
areas supplement snapshots. Format facts come from Blender's producer
[header definitions](https://github.com/blender/blender/blob/main/source/blender/blenloader_core/BLO_core_blend_header.hh)
and SDNA/mesh source; Blender's GPL implementation is not copied. Real
big-endian/32-bit geometry remains unqualified despite authored layout tests.
See [fixture details](../../tests/fixtures/native_geometry/blend/README.md).

Eight external MIT LightBurn projects are unchanged and contain no
`Thumbnail`; the ninth classic fixture is explicitly authored. The
[provenance](../../tests/fixtures/native_geometry/lightburn/provenance.json)
and [fixture notes](../../tests/fixtures/native_geometry/lightburn/README.md)
document independent coordinates, shared resources and adaptive Beziers.
Producer explanations establish
[cubic handles](https://forum.lightburnsoftware.com/t/lbrn-or-lbrn2-xml-file-docuumentation/42317)
and [affine order/mirroring](https://forum.lightburnsoftware.com/t/lbrn2-file-documentation/52174).

DWG pins MPL-2.0 `acadrust=0.6.3` with optional features disabled. Actual
Linux/macOS/Windows feature trees use Rust compression backends. Strict decode
statistics reject partial drawings. The separate GPL test-asset license,
independent coordinate sources, failed broader probes and remaining decoder
bounds are documented in the [DWG evidence](../../tests/fixtures/native_geometry/dwg/README.md).

## Inventor/Fusion reverse engineering

Apache-2.0
[`cadmpeg`](https://github.com/cadmpeg/cadmpeg/tree/9e8f6d6b5da9371bc791ce3924f364aea81be0ec)
decodes Inventor RSe streams, Fusion active-design graphs and ASM/SAB records
into typed boundary representations. Its original native Zstandard
dependencies prevent unchanged adoption. The isolated
[experimental bridge](../../tools/autodesk_geometry/README.md) pins that
revision, applies a reviewable patch using bounded `ruzstd`, and performs
decoding, validated STEP conversion and Catwalk rendering in one Rust
process. Setup fetches the large upstream source into an ignored checkout;
default Catwalk dispatch and dependencies remain separate.

Inspected older Inventor envelopes use compression tag 1/zlib; current 2027
RSe schema-31/MetaStream-8 envelopes use tag 2/Zstandard. Dispatching on that
tag recovers a current cylinder's one body, three faces, two circular edges
and two vertices. Derived no-image parts remove only summary PID17 `VT_CF`;
all RSeStorage streams stay byte-identical.

MIT Adafruit Fusion archives contain ASM `.smb`/`.smbh` BRep members without
an OGS mesh cache. Derived files remove only `/Previews/` members, preserving
all remaining member payloads. The extrusion decodes to one body, 107 faces,
314 edges and 210 vertices, validates and exports STEP without geometry
losses. Other probes expose a 19-face switch and a 37-body/680-face board;
three production archives fail unresolved component contexts. Counts alone
do not qualify every surface or assembly placement.

The bridge's images come from solid geometry. Its separate tests check
original-versus-image-stripped topology and pixels, dimensions, face
preservation and unsupported assemblies. The eight upstream crates contain
roughly half a million runtime source lines, so this reproducible isolated
build avoids importing that maintenance burden before a qualified integration
choice. Source licenses, hashes, retained geometry and exact current proof
results are recorded alongside the tool.

```sh
cargo test --locked --all-targets
cargo run --locked --example native_geometry_probe -- \
  tests/fixtures/native_geometry/blend/hierarchy-502-no-thumbnail.blend \
  tests/fixtures/native_geometry/lightburn/rings.lbrn2 \
  tests/fixtures/native_geometry/dwg/line-2018.dwg
cargo run --locked -- tests/fixtures/native_geometry/blend/default-252-no-thumbnail.blend \
  --render-geometry --transparent --size 256 --output fresh.png
cargo run --locked --example gallery -- --output target/gallery
```

The Autodesk tool has its own build instructions. Its research proof does not
advertise `.ipt`, `.iam` or `.f3d` as normal CLI geometry readers.
