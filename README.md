# Catwalk

Headless model thumbnails rendered in Rust, with a fixed isometric camera, automatic framing, simple lighting and antialiasing. Catwalk provides a command-line program and a library. It renders on the CPU and needs no GPU, display server, F3D, VTK, Assimp, Open CASCADE or subprocess converters.

Catwalk has Rust thumbnail readers for all 76 extension aliases in F3D 3.5's published format list. Each reader supports a documented subset of its format; **full compatibility with every file F3D can read is unfinished**. [The coverage matrix](docs/format-coverage.md) records the supported encodings, geometry and known limitations. Unsupported inputs produce an error.

It also extracts saved thumbnails from Inventor `.ipt`/`.iam`, `LightBurn` `.lbrn`/`.lbrn2`, Blender `.blend`, Fusion `.f3d` and `AutoCAD` `.dwg`. These seven additional extensions use qualified image carriers. A saved image may be absent, blank or stale; extracting it does not regenerate model geometry.

Catwalk additionally renders stored Blender mesh scenes, `LightBurn` vector drawings and a DWG drawing subset when their saved image is missing or unreadable. `--render-geometry` forces a fresh preview even when a saved image exists. The [coverage matrix](docs/format-coverage.md#autodesk-inventor) describes these document readers and their limits. Geometry rendering for `.ipt`, `.iam` and `.f3d` is unsupported in Catwalk.

## Build and use

The crate is named `catwalk3d`; its command-line executable is `catwalk`.

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
use catwalk3d::{render::{RenderOptions, write_png}, thumbnail};
use std::{fs::File, path::Path};

fn main() -> catwalk3d::Result<()> {
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

Saved-preview support:

| Family | Supported carrier | Limits and regression fixtures |
| --- | --- | --- |
| Inventor | Root CFBv3/v4 property sets, native summary FMTID/PID17 `VT_CF`, legacy summary fallback; PNG/DIB/BMP | Blank images remain valid. Full application-release compatibility remains unqualified. [Carrier fixture details](tests/fixtures/saved-previews/autodesk/README.md). |
| `LightBurn` | Direct XML `Thumbnail Source` base64 PNG; `FormatVersion` 0/1 | Newer template/multipage layouts and other carriers remain unqualified. [Fixture details](tests/fixtures/saved-previews/lightburn/README.md). |
| Blender | Early `TEST` RGBA block, vertically flipped; legacy and format-1 block layouts; raw/gzip/seekable Zstandard | Big-endian files and versions before 2.50 are unsupported. [Fixture details](tests/fixtures/saved-previews/blend_dwg/README.md). |
| Fusion | ZIP32 manifest tag `3-2-0-0`, exact active-design `Previews/small.png`; Stored/Deflate/Zstandard method 93 | Container tag is not an application release number. ZIP64/split/encrypted archives are unsupported. [Reader limits](docs/format-coverage.md#autodesk-fusion). |
| DWG | R13/R14/2000/2004/2007/2010/2013/2018 signatures; declared preview address, validated sentinels and typed entry ranges; type-2 DIB/type-6 PNG | Earlier formats, encrypted previews and WMF-only previews are unsupported. [Carrier fixture details](tests/fixtures/saved-previews/blend_dwg/README.md). |

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Tests cover depth and winding, centering and aspect ratio, transparent edges, texture coordinates and filtering, alpha masks and blended overlaps, transformed normals, GPU instances, extreme coordinates, importer encodings and transforms, concave polygons, scientific grid boundaries, packaged 3MF files, the CLI and PNG output. The CI workflow is configured to test Linux, macOS and Windows; the Linux job unsets display-related environment variables.

The [format corpus](tests/fixtures/formats/manifest.json) supplies an encoded input for every advertised extension, plus additional encoding and scene examples. Saved-preview cases include licensed original projects and clearly identified authored carrier fixtures; minimal carriers contain no CAD geometry. `cargo test --locked --test format_snapshots` renders every case through the CLI and compares decoded RGBA pixels with [committed PNG snapshots](tests/snapshots/formats). Missing fixtures, missing snapshots, decoding failures and visual changes fail CI. Transparent pixels are compared in premultiplied RGBA; each channel may differ by at most 2/255 for platform rounding. PNG compression bytes are not compared.

CI retains actual, expected and amplified diff images as downloadable artifacts on every platform. Successful runs publish the same tested images to the [GitHub Pages gallery](https://cmeister2.github.io/catwalk/), with format search, model/image filters and an interactive snapshot comparison. Deployment waits for formatting, strict Clippy, documentation and all three operating-system test jobs. Pull requests produce a gallery artifact for review.

To regenerate authored inputs, run `cargo run --locked --example generate_fixtures`. To explicitly update baselines and inspect the resulting gallery, run `cargo run --locked --example gallery -- --update-snapshots`. Review the images and commit the changed PNGs; normal tests never update them. For a local gallery from an existing test run, use `cargo run --locked --example gallery -- --report target/format-snapshots`, then serve `target/gallery` with a static HTTP server. These snapshots establish regression coverage for their fixtures, while the broader valid-file limitations in the coverage matrix still apply.

Input and referenced resources are limited to 512 MiB each. Raster image decoding is limited to 256 MiB. Importers apply format-specific count limits, and scene expansion is bounded for formats with instances. The DWG record ceiling is checked after decoding; additional upstream section and allocation bounds remain [unfinished](docs/format-coverage.md#autocad-drawings). CLI output dimensions are 1–4096 pixels per axis; model rendering additionally allows at most 16 million supersampled pixels, so very large thumbnails may need `--samples 1`. Decode failures occur before an output file is opened.

## Implementation

Rust importers produce a common triangle, point and line scene. The software renderer normalizes model coordinates, fits projected bounds, resolves opaque visibility with a depth buffer, applies simple lighting, downsamples premultiplied colors and encodes PNG. glTF/GLB triangle previews additionally sample PNG/JPEG base-color textures, interpolate linear vertex RGBA and transformed normals, apply material sidedness and alpha cutouts, and compose transparent fragments in depth order for each sample. Samplers support repeat, mirrored repeat, clamp, nearest and bilinear filtering; mipmap modes use the base-level fallback defined by glTF. Appearance shading, blending and supersampling operate in linear color. Required GPU instancing and texture-transform extensions are supported; skin combined with GPU instancing remains an explicit error.

3MF previews additionally sample packaged PNG/JPEG texture groups, preserving per-corner UVs, material defaults and repeat/mirror/clamp settings. Texture filtering uses encoded sRGB as specified by 3MF; glTF retains linear filtering. Direct 3MF texture layers are opaque. Composite and layered material resources remain explicit errors.

Material and vertex colors are used where implemented; glTF and supported FBX skins/morphs are evaluated in the first pose. Full physically based materials, normal/emissive/occlusion maps, textures outside glTF/3MF, Gaussian splatting and additional deformation variants remain tracked limitations. Scalar volumes use a coarse density surface; DICOM and raster images use a fitted image preview.

Compression and image codecs use Rust backends. The supported Linux, Windows and macOS dependency graphs do not contain native geometry or codec libraries. Rust's standard library and platform bindings still use normal operating-system facilities.

Dependencies are published crates without local patches. Owned adapters decode the affected HDF5 chunk indexes into upstream arrays, restore authored BREP frames through geometry builders, and construct independent IGES preview faces through public topology APIs.

## Releases

Successful CI runs on `main` publish the `catwalk3d` crate and a matching GitHub release through semantic-release. Conventional commits determine the version: `fix:` produces a patch, `feat:` a minor release, and a breaking change a major release. The first release is `1.0.0`. Documentation-only commits do not trigger a release.

Publication waits for formatting, strict Clippy, documentation and every operating-system test job. The release checkout stamps the selected version into `Cargo.toml` and its local `Cargo.lock` entry, verifies the packaged crate and its size, and publishes with locked dependencies before semantic-release creates a version tag. An existing non-yanked version is accepted only when the official crates.io checksum matches the exact verified archive. A failed first upload therefore leaves the version untagged; retrying an identical successful upload does not upload it twice. GitHub releases include the `.crate` source archive. Node dependencies are used only by the release tooling.

Registry publication, Git tagging and GitHub releases are separate operations. If GitHub publication fails after the version tag is pushed, maintainers must finish or repair that tagged GitHub release; rerunning semantic-release alone does not republish an existing tag. Keep the published crate and tag when recovering this partial state.

Maintainers configure a crates.io publishing token in the repository secret `CARGO_REGISTRY_TOKEN`; `PUBLISH_SECRET` is also accepted. Release automation skips publication while neither secret is configured. Local `.env` files are excluded from Git and Cargo packages.

## License

Catwalk's code and authored fixtures are MIT licensed. Retained upstream test assets keep their separately documented licenses and notices, including the AGPL-3.0 slicer examples. Catwalk is an independent thumbnail implementation; no F3D source code is included.
