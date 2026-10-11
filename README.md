# Catwalk

Headless model thumbnails rendered in Rust, with a fixed isometric camera, automatic framing, simple lighting and antialiasing. Catwalk provides a command-line program and a library. It renders on the CPU and needs no GPU, display server, F3D, VTK, Assimp, Open CASCADE or subprocess converters.

Catwalk has Rust thumbnail readers for all 76 extension aliases in F3D 3.5's published format list. Each reader supports a documented subset of its format; **full compatibility with every file F3D can read is unfinished**. [The coverage matrix](docs/format-coverage.md) records the supported encodings, geometry and known limitations. Unsupported inputs produce an error.

It also extracts saved thumbnails from Inventor `.ipt`/`.iam`, `LightBurn` `.lbrn`/`.lbrn2`, Blender `.blend`, Fusion `.f3d` and `AutoCAD` `.dwg`. These seven additional extensions use qualified image carriers. A saved image may be absent, blank or stale; extracting it does not regenerate model geometry.

Catwalk additionally renders stored Blender mesh scenes, `LightBurn` vector drawings and a qualified DWG drawing subset when their saved image is missing or unreadable. `--render-geometry` forces a fresh preview even when a saved image exists. [Native geometry evidence and limitations](docs/research/native-geometry.md) describe these readers and the separate experimental Inventor/Fusion solid bridge.

## Build and use

Install a current stable Rust toolchain, then:

```sh
cargo build --release --locked
./target/release/catwalk model.stl --output thumbnail.png --size 256
./target/release/catwalk model.glb --output thumbnail.png --resolution 640,360
./target/release/catwalk model.3mf --output thumbnail.png --transparent
./target/release/catwalk model.obj --output - > thumbnail.png
./target/release/catwalk project.lbrn2 --output thumbnail.png --size 256
./target/release/catwalk scene.blend --render-geometry --output fresh.png --size 256
./target/release/catwalk part.ipt --extract-saved-preview --output - > saved-preview.png
./target/release/catwalk --list-formats
```

Run `cargo run --example demo --locked` to generate an original gear OBJ and this preview:

![Procedural gear rendered by Catwalk](examples/gear.png)

`--resolution WIDTHxHEIGHT` also works. `--no-background` is an alias for `--transparent`, and `--list-readers` is an alias for `--list-formats`. These cover common F3D thumbnail invocation patterns; the rest of F3D's CLI and viewer are outside this thumbnail project.

Use `--background '#f5f7fa'`, `--color '#64a0dc'` and `--samples 1|2|3|4` to adjust the output. The default is a 512 × 512 PNG with 2 × 2 supersampling. Nonsquare images preserve the model's proportions. Transparent output keeps antialiased edge colors. Image inputs are fitted and centered on the same canvas.

Geometry uses Z up; scene importers convert formats with other axis conventions. The camera has a deterministic fallback for a model that lies entirely edge-on to the default view. There is no interactive viewer, animation playback or GPU initialization.

## Library

```rust,no_run
use catwalk::{render::{RenderOptions, write_png}, thumbnail};
use std::{fs::File, path::Path};

fn main() -> catwalk::Result<()> {
    let options = RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..Default::default()
    };
    let image = thumbnail(Path::new("model.stl"), &options)?;
    write_png(&image, File::create("thumbnail.png")?)?;
    Ok(())
}
```

`formats::load` returns triangles, points and line segments in a `scene::Scene`. Its optional triangle appearances carry linear RGBA factors, per-vertex colors, UVs, imported normals, shared textures, alpha mode and sidedness. Applications can supply their own scene to `render::render` without writing a model file. `write_png` accepts any `std::io::Write`, including an in-memory buffer or stdout.

`geometry_thumbnail` renders stored geometry without consulting an embedded image. `LightBurn` uses an XY plan view; the other geometry readers use the fitted model view. The native document readers support the static geometry described in the coverage matrix. Application feature evaluation remains unfinished.

`saved_preview::extract` returns `Result<Option<RgbaImage>>` at the saved image's original dimensions: `None` means no supported saved image. Inventor returns `None` for complete unsupported image encodings; malformed data errors. Other readers reject unsupported document versions and carriers. `saved_preview::inventor::extract_png` preserves native PNG bit depth and strips ancillary metadata. Normal `thumbnail` output fits the saved image onto an RGBA8 canvas.

Saved-preview qualification:

| Family | Qualified carrier | Evidence and limits |
| --- | --- | --- |
| Inventor | Root CFBv3/v4 property sets, native summary FMTID/PID17 `VT_CF`, legacy summary fallback; PNG/DIB/BMP | Generated CFBv3/v4 carriers exercise PNG and supported bitmap extraction. Blank images remain valid; full application-release compatibility remains unqualified. [Evidence](docs/research/autodesk-previews.json). |
| `LightBurn` | Direct XML `Thumbnail Source` base64 PNG; `FormatVersion` 0/1 | 43 distinct files: 21 previews extracted, 22 absent. Saved-preview producers range from 0.9.16 to 2.0.05; newer template/multipage layouts are unqualified. [Evidence](docs/research/lightburn-preview.json). |
| Blender | Early `TEST` RGBA block, vertically flipped; legacy and format-1 block layouts; raw/gzip/seekable Zstandard | Real scenes qualify both header generations and compressed carriers. Big-endian files and versions before 2.50 are unsupported. [Evidence](docs/research/blend-dwg-previews.json). |
| Fusion | ZIP32 manifest tag `3-2-0-0`, exact active-design `Previews/small.png`; Stored/Deflate/Zstandard method 93 | Five genuine designs including a Zstandard assembly. Container tag is not an application release number. ZIP64/split/encrypted archives remain unqualified. [Evidence](docs/research/autodesk-previews.json). |
| DWG | Declared preview address, validated sentinels and typed entry ranges; type-2 DIB/type-6 PNG | Real R13, R14, 2000, 2004, 2007, 2010, 2013 and 2018 layouts qualify. Earlier formats, encrypted sections and WMF-only previews are unsupported. [Evidence](docs/research/blend-dwg-previews.json). |

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Tests cover depth and winding, centering and aspect ratio, transparent edges, texture coordinates and filtering, alpha masks and blended overlaps, transformed normals, GPU instances, extreme coordinates, importer encodings and transforms, concave polygons, scientific grid boundaries, packaged 3MF files, the CLI and PNG output. The CI workflow is configured to test Linux, macOS and Windows; the Linux job unsets display-related environment variables.

The [format corpus](tests/fixtures/formats/manifest.json) supplies an encoded input for every advertised extension, plus additional encoding and scene examples. Saved-preview cases include licensed original projects and clearly identified authored carrier fixtures; minimal carriers contain no CAD geometry. `cargo test --locked --test format_snapshots` renders every case through the CLI and compares decoded RGBA pixels with [committed PNG snapshots](tests/snapshots/formats). Missing fixtures, missing snapshots, decoding failures and visual changes fail CI. Transparent pixels are compared in premultiplied RGBA; each channel may differ by at most 2/255 for platform rounding. PNG compression bytes are not compared.

CI retains actual, expected and amplified diff images as downloadable artifacts on every platform. Successful runs publish the same tested images to the [GitHub Pages gallery](https://cmeister2.github.io/catwalk/), with format search, model/image filters and an interactive snapshot comparison. Deployment waits for formatting, strict Clippy, documentation, all three operating-system test jobs and the separate experimental solid-geometry checks. Pull requests produce a gallery artifact for review.

To regenerate authored inputs, run `cargo run --locked --example generate_fixtures`. To explicitly update baselines and inspect the resulting gallery, run `cargo run --locked --example gallery -- --update-snapshots`. Review the images and commit the changed PNGs; normal tests never update them. For a local gallery from an existing test run, use `cargo run --locked --example gallery -- --report target/format-snapshots`, then serve `target/gallery` with a static HTTP server. These snapshots establish regression coverage for their fixtures, while the broader valid-file limitations in the coverage matrix still apply.

Input and referenced resources are limited to 512 MiB each. Raster image decoding is limited to 256 MiB. Importers apply format-specific count limits, and scene expansion is bounded for formats with instances. The DWG record ceiling is checked after decoding; additional upstream section and allocation bounds remain [unfinished](docs/research/native-geometry.md). CLI output dimensions are 1–4096 pixels per axis; model rendering additionally allows at most 16 million supersampled pixels, so very large thumbnails may need `--samples 1`. Decode failures occur before an output file is opened.

## Implementation

Rust importers produce a common triangle, point and line scene. The software renderer normalizes model coordinates, fits projected bounds, resolves opaque visibility with a depth buffer, applies simple lighting, downsamples premultiplied colors and encodes PNG. glTF/GLB triangle previews additionally sample PNG/JPEG base-color textures, interpolate linear vertex RGBA and transformed normals, apply material sidedness and alpha cutouts, and compose transparent fragments in depth order for each sample. Samplers support repeat, mirrored repeat, clamp, nearest and bilinear filtering; mipmap modes use the base-level fallback defined by glTF. Appearance shading, blending and supersampling operate in linear color. Required GPU instancing and texture-transform extensions are supported; skin combined with GPU instancing remains an explicit error.

3MF previews additionally sample packaged PNG/JPEG texture groups, preserving per-corner UVs, material defaults and repeat/mirror/clamp settings. Texture filtering uses encoded sRGB as specified by 3MF; glTF retains linear filtering. Direct 3MF texture layers are opaque. Composite and layered material resources remain explicit errors.

Material and vertex colors are used where implemented; glTF and supported FBX skins/morphs are evaluated in the first pose. Full physically based materials, normal/emissive/occlusion maps, textures outside glTF/3MF, Gaussian splatting and additional deformation variants remain tracked limitations. Scalar volumes use a coarse density surface; DICOM and raster images use a fitted image preview.

Compression and image codecs use Rust backends. The supported Linux, Windows and macOS dependency graphs do not contain native geometry or codec libraries. Rust's standard library and platform bindings still use normal operating-system facilities.

## License

Catwalk's code and authored fixtures are MIT licensed. Retained upstream test assets keep their separately documented licenses and notices, including the AGPL-3.0 slicer examples. Catwalk is an independent thumbnail implementation; no F3D source code is included.
