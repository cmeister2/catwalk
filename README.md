# Catwalk

Turn 3D models into PNG thumbnails from your browser, the command line or a Rust application. Catwalk renders on the CPU with automatic framing, a fixed isometric view, simple lighting and antialiasing. Native builds run headlessly on Linux, macOS and Windows.

The Rust crate is **`catwalk3d`**; the command is **`catwalk`**.

[Create a thumbnail](https://cmeister2.github.io/catwalk3d/upload/) · [Browse example thumbnails](https://cmeister2.github.io/catwalk3d/) · [Check format coverage](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md) · [Rust API reference](https://docs.rs/catwalk3d/)

## Install and make a thumbnail

Download a platform ZIP from [GitHub Releases](https://github.com/cmeister2/catwalk3d/releases), extract it, and open a terminal in the extracted directory. On Linux or macOS:

```sh
./catwalk model.stl --output thumbnail.png --size 256
```

On Windows, run `.\catwalk.exe model.stl --output thumbnail.png --size 256` in PowerShell.

Linux builds are available for x86-64 and ARM64 with GNU or musl. Choose musl for a statically linked executable, including Alpine and systems without glibc. GNU builds are compiled and tested on Ubuntu 24.04; glibc 2.39 or newer is the supported baseline. macOS builds cover Intel and Apple Silicon; Windows builds cover x86-64. The archives include license notices and release metadata, with SHA-256 checksums beside the downloads. macOS and Windows downloads are not signed or notarized.

To install with a current stable Rust toolchain and Cargo:

```sh
cargo install catwalk3d --locked
catwalk model.stl --output thumbnail.png --size 256
```

This writes a 256 × 256 PNG and fits the model into the frame. The default output is a 512 × 512 PNG on a light background.

![Example gear thumbnail rendered by Catwalk](examples/gear.png)

To build from source and preview an included model:

```sh
git clone https://github.com/cmeister2/catwalk3d.git
cd catwalk3d
cargo run --release --locked -- examples/models/gear.obj --output thumbnail.png --size 256
```

The importers and renderer use Rust implementations. Native Autodesk decoding uses Cadmpeg with bundled Zstandard compression; building from source requires a C compiler for that compression library. Running Catwalk requires no graphics driver, display server, CAD installation or subprocess converter.

## Create a thumbnail in your browser

The [browser tool](https://cmeister2.github.io/catwalk3d/upload/) reads a selected model, image or document and lets you download its PNG thumbnail. Selected files stay on your device. Choose automatic saved-preview preference or fresh geometry, output size and transparency. Add materials, textures or assembly parts stored beside the primary file as companions; the picker currently accepts files from one folder.

The page loads the compressed renderer when you generate a thumbnail and runs it in a worker. Browser limits are 64 MiB per file, 128 MiB total, 2,048 selected files and two minutes per job. Cancellation stops the worker; a failed input can be followed by another render. Format versions and encodings follow the compatibility matrix below.

For local development, install Clang with WebAssembly support and the Rust WASI target, then build the static page:

```sh
rustup target add wasm32-wasip1
npm ci --ignore-scripts
npm run build:web
python3 -m http.server 4173 --bind 127.0.0.1 --directory target
```

Open `http://127.0.0.1:4173/web/`. `npm run test:web:node` exercises the WASI runner; `npx playwright install chromium` and `npm run test:web:browser` exercise uploads and downloads in Chromium. CI supplies runtime notices, verifies the page, and publishes it beside the existing gallery at `/upload/` on successful main builds.

## Which files can I preview?

The matrix covers all 91 advertised extensions, sorted alphabetically by type with aliases grouped together.

**✅ Supported for a qualified subset · ❌ Unsupported · — Not applicable.** Coverage follows the versions, encodings and features in each linked entry. Geometry includes meshes, points, lines and surfaces generated from volume data. Saved preview means extracting a document image stored in or referenced by a model file.

Ordinary raster and DICOM inputs are fitted directly to the canvas, so both document modes are —. TIFF also supports ✅ geometry previews from qualified scalar page stacks.

| Type | Extensions | Saved preview | Geometry |
| --- | --- | :---: | :---: |
| [3DS](docs/format-coverage.md#3ds) | `.3ds` | — | ✅ |
| [3ds Max](docs/format-coverage.md#3ds-max) | `.max` | ✅ | ✅ |
| [3MF](docs/format-coverage.md#3mf) | `.3mf` | ❌ | ✅ |
| [Alembic](docs/format-coverage.md#alembic) | `.abc` | — | ✅ |
| [AMF](docs/format-coverage.md#amf) | `.amf` | — | ✅ |
| [`AutoCAD` drawings](docs/format-coverage.md#autocad-drawings) | `.dwg` | ✅ | ✅ |
| [Autodesk Fusion](docs/format-coverage.md#autodesk-fusion) | `.f3d` | ✅ | ✅ |
| [Autodesk Inventor](docs/format-coverage.md#autodesk-inventor) | `.ipt`, `.iam` | ✅ | ✅ |
| [Blender](docs/format-coverage.md#blender) | `.blend` | ✅ | ✅ |
| [BMP](docs/format-coverage.md#bmp) | `.bmp` | — | — |
| [BPF](docs/format-coverage.md#bpf) | `.bpf` | — | ✅ |
| [BREP](docs/format-coverage.md#brep) | `.brep` | — | ✅ |
| [`CityGML`](docs/format-coverage.md#citygml) | `.gml` | — | ✅ |
| [COLLADA](docs/format-coverage.md#collada) | `.dae` | — | ✅ |
| [DICOM](docs/format-coverage.md#dicom) | `.dcm` | — | — |
| [DirectX](docs/format-coverage.md#directx) | `.x` | — | ✅ |
| [Draco](docs/format-coverage.md#draco) | `.drc` | — | ✅ |
| [DXF](docs/format-coverage.md#dxf) | `.dxf` | ❌ | ✅ |
| [Exodus II](docs/format-coverage.md#exodus-ii) | `.e`, `.ex2`, `.exo`, `.g` | — | ✅ |
| [EXR](docs/format-coverage.md#exr) | `.exr` | — | — |
| [FBX](docs/format-coverage.md#fbx) | `.fbx` | ❌ | ✅ |
| [`FreeCAD`](docs/format-coverage.md#freecad) | `.fcstd` | ✅ | ✅ |
| [GLB](docs/format-coverage.md#glb) | `.glb` | — | ✅ |
| [glTF](docs/format-coverage.md#gltf) | `.gltf` | — | ✅ |
| [HDR](docs/format-coverage.md#hdr) | `.hdr` | — | — |
| [IFC](docs/format-coverage.md#ifc) | `.ifc` | — | ✅ |
| [IGES](docs/format-coverage.md#iges) | `.iges`, `.igs` | — | ✅ |
| [JPEG](docs/format-coverage.md#jpeg) | `.jpg`, `.jpeg` | — | — |
| [LAS](docs/format-coverage.md#las) | `.las`, `.laz` | — | ✅ |
| [`LightBurn`](docs/format-coverage.md#lightburn) | `.lbrn`, `.lbrn2` | ✅ | ✅ |
| [Lychee](docs/format-coverage.md#lychee) | `.lys` | ✅ | ✅ |
| [`MetaImage`](docs/format-coverage.md#metaimage) | `.mhd`, `.mha` | — | ✅ |
| [`NetCDF`](docs/format-coverage.md#netcdf) | `.nc`, `.cdf`, `.ncdf` | — | ✅ |
| [NRRD](docs/format-coverage.md#nrrd) | `.nrrd`, `.nhdr` | — | ✅ |
| [OBJ](docs/format-coverage.md#obj) | `.obj` | — | ✅ |
| [OFF](docs/format-coverage.md#off) | `.off` | — | ✅ |
| [Optech CSD](docs/format-coverage.md#optech-csd) | `.csd` | — | ✅ |
| [PCD](docs/format-coverage.md#pcd) | `.pcd` | — | ✅ |
| [PLY](docs/format-coverage.md#ply) | `.ply` | — | ✅ |
| [PNG](docs/format-coverage.md#png) | `.png` | — | — |
| [PTS](docs/format-coverage.md#pts) | `.pts` | — | ✅ |
| [PTX](docs/format-coverage.md#ptx) | `.ptx` | — | ✅ |
| [QFIT](docs/format-coverage.md#qfit) | `.qi` | — | ✅ |
| [Quake MDL](docs/format-coverage.md#quake-mdl) | `.mdl` | — | ✅ |
| [Rhino](docs/format-coverage.md#rhino) | `.3dm` | ✅ | ✅ |
| [SBET](docs/format-coverage.md#sbet) | `.sbet` | — | ✅ |
| [`SketchUp`](docs/format-coverage.md#sketchup) | `.skp` | ✅ | ✅ |
| [SLPK](docs/format-coverage.md#slpk) | `.slpk` | — | ✅ |
| [`SolidWorks`](docs/format-coverage.md#solidworks) | `.sldprt`, `.sldasm` | ✅ | ✅ |
| [Splat](docs/format-coverage.md#splat) | `.splat` | — | ✅ |
| [SPZ](docs/format-coverage.md#spz) | `.spz` | — | ✅ |
| [STEP](docs/format-coverage.md#step) | `.step`, `.stp`, `.stpnc`, `.p21`, `.210` | — | ✅ |
| [STL](docs/format-coverage.md#stl) | `.stl` | — | ✅ |
| [`TerraScan` BIN](docs/format-coverage.md#terrascan-bin) | `.bin` | — | ✅ |
| [TGA](docs/format-coverage.md#tga) | `.tga` | — | — |
| [TIFF](docs/format-coverage.md#tiff) | `.tif`, `.tiff` | — | ✅ |
| [USD](docs/format-coverage.md#usd) | `.usd`, `.usda`, `.usdc`, `.usdz` | ❌ | ✅ |
| [VDB](docs/format-coverage.md#vdb) | `.vdb` | — | ✅ |
| [VRML](docs/format-coverage.md#vrml) | `.wrl`, `.vrml` | — | ✅ |
| [VTK `ImageData` XML](docs/format-coverage.md#vtk-imagedata-xml) | `.vti` | — | ✅ |
| [VTK legacy](docs/format-coverage.md#vtk-legacy) | `.vtk` | — | ✅ |
| [VTK `MultiBlock` XML](docs/format-coverage.md#vtk-multiblock-xml) | `.vtm` | — | ✅ |
| [VTK `PolyData` XML](docs/format-coverage.md#vtk-polydata-xml) | `.vtp` | — | ✅ |
| [VTK `RectilinearGrid` XML](docs/format-coverage.md#vtk-rectilineargrid-xml) | `.vtr` | — | ✅ |
| [VTK `StructuredGrid` XML](docs/format-coverage.md#vtk-structuredgrid-xml) | `.vts` | — | ✅ |
| [VTK `UnstructuredGrid` XML](docs/format-coverage.md#vtk-unstructuredgrid-xml) | `.vtu` | — | ✅ |
| [VTKHDF](docs/format-coverage.md#vtkhdf) | `.vtkhdf` | — | ✅ |
| [WebP](docs/format-coverage.md#webp) | `.webp` | — | — |
| [XBF](docs/format-coverage.md#xbf) | `.xbf` | — | ✅ |

The ❌ saved-preview entries have documented image carriers that Catwalk does not extract: [3MF thumbnails](https://github.com/3MFConsortium/spec_core/blob/master/3MF%20Core%20Specification.md), [DXF `THUMBNAILIMAGE`](https://help.autodesk.com/cloudhelp/2017/ENU/AutoCAD-DXF/files/GUID-792F79DC-0D5D-43B5-AB0E-212E0EDF6BAE.htm), [FBX thumbnail objects](https://help.autodesk.com/cloudhelp/2020/ENU/FBX-Developer-Help/files/cpp_ref/FBX_Developer_Help_cpp_ref_class_fbx_thumbnail_html.html) and [USD `AssetPreviewsAPI`](https://openusd.org/dev/user_guides/schemas/usdMedia/AssetPreviewsAPI.html). Other mesh, point and scientific-volume inputs use — for saved-preview mode.

Run `catwalk --list-formats` for the complete extension list. Each reader supports a documented subset; the [coverage matrix](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md) lists supported encodings, scene features and limitations.

Use `catwalk --list-formats --json` for a versioned catalog with every extension alias, human-readable format descriptions and support categories. See the [discovery contract](docs/format-catalog.md) and [JSON Schema](docs/format-catalog.schema.json) for downstream CI checks and settings tables.

Models use a fitted static view. Supported colors and materials are preserved; OBJ, glTF/GLB and 3MF additionally support base-color textures. Animation files use a supported static pose or sample. Raster images are fitted to the canvas with their proportions preserved.

TIFF (`.tif` / `.tiff`) supports fitted single-page images and fresh 3D previews of homogeneous grayscale page stacks. Volume previews use thresholded density surfaces, TIFF orientation and supported physical XY resolution; slice spacing defaults to X spacing. RGB stacks, heterogeneous image series and OME hyperstack axes remain unsupported. See [the TIFF coverage entry](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md#tiff).

COLLADA (`.dae`) previews include static surfaces, colored wire geometry, planar polygon holes and supported node placements, including `lookat` and angular `skew`. Controllers, textures and animation remain outside this reader's static subset; see [its coverage entry](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md#collada) for limits and tested examples.

Open CASCADE `.brep` previews accept supported ASCII and little-endian binary topology tables, including curved surfaces, free wires, isolated points and stored placements. See [the BREP coverage entry](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md#brep) for supported versions, cache exclusions and limits.

## Native documents and saved previews

By default, a supported saved image is used when available. Native documents with geometry readers fall back to fresh rendering when that image is missing or unreadable. `--render-geometry` bypasses saved images. A saved thumbnail can be blank or out of date; reader versions and supported geometry follow the linked coverage entries.

**`SolidWorks` supports native geometry and saved images.** Fresh rendering decodes the active solved part state, using qualified complete native 3D mesh caches or supported B-rep tessellation. Assemblies use authored component placements and require their native companion files in the supplied project directory. Saved-image extraction uses root `PreviewPNG` or supported `Preview` DIB streams and needs no companions; absent images return code 3. See [the coverage entry](docs/format-coverage.md#solidworks) and [producer qualification](tests/fixtures/formats/solidworks/README.md) for supported variants and limits.

**`LightBurn` supports fresh vector rendering:** paths, cubic Bézier curves, shared instances, nested groups, rectangles, rounded corners, ellipses and the first 16 standard cut-layer colors. Text, bitmaps, laser fills and multipage geometry remain unsupported. Nine projects without saved thumbnails have geometry checks and PNG snapshots.

**Inventor previews use stored solved geometry.** Fresh rendering decodes native part bodies and supported assembly occurrences, retaining their authored placement and suppression. Assemblies require their referenced `.ipt` / `.iam` documents beside the input; missing or ambiguous companions fail. Assembly features with nonempty local result bodies and unsupported document variants fail explicitly. The saved-image reader remains available without companions. Tests use unchanged native producer files; compatibility with every Inventor release and Model State remains unfinished.

**Fusion previews can render native geometry.** Supported `.f3d` archives transfer active solved bodies and supported stored carriers without using their saved images. Unresolved native assembly operations are refused until their final placements can be verified. The decoder also validates native document records; older design histories and complex archives can exceed its supported grammar or service budgets. These variants and `.f3z` archives remain unsupported. See [the Fusion coverage entry](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md#autodesk-fusion) for qualified examples and limits.

**Lychee previews prefer the saved scene PNG.** Fresh geometry renders the original stored model meshes with their saved scale, rotation, XY position and Z lift. Supports, rafts, hollowing and holes preparation are intentionally excluded from fresh geometry; the saved PNG preserves the prepared image. Original Lychee 5.4.3 keyboard scenes qualify flat Mango/ASAR manifest 3.1.0, scene-state 2 and mesh 2 encodings. Legacy ZIP scenes, `.lyt`, other stored encodings and missing external/library model geometry remain unsupported; see [the Lychee coverage entry](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md#lychee).

**`FreeCAD` previews use stored document shapes.** Fresh geometry respects supported visibility, part containers, local links and model arrays without opening `FreeCAD`. The reader supports qualified schema-4 documents with ASCII BREP members. Binary shape members, external links and unsupported link modes return an error; see [the FreeCAD coverage entry](https://github.com/cmeister2/catwalk3d/blob/main/docs/format-coverage.md#freecad).

See [native geometry thumbnail examples](docs/native-geometry/README.md) and [native Autodesk geometry examples and source credits](docs/native-autodesk/README.md).

Choose the preview mode explicitly when needed:

```sh
# Regenerate a supported document's preview from stored geometry.
catwalk project.lbrn2 --render-geometry --output fresh.png --size 256
catwalk scene.blend --render-geometry --output fresh.png --size 256
catwalk scene.lys --render-geometry --output models.png --size 256
catwalk part.FCStd --render-geometry --output fresh.png --size 256
catwalk part.ipt --render-geometry --output fresh.png --size 256
catwalk assembly.iam --render-geometry --output fresh.png --size 256
catwalk design.f3d --render-geometry --output fresh.png --size 256
catwalk model.skp --render-geometry --output fresh.png --size 256
catwalk model.3dm --render-geometry --output fresh.png --size 256
catwalk scene.max --render-geometry --output fresh.png --size 256
catwalk part.sldprt --render-geometry --output fresh.png --size 256
catwalk assembly.sldasm --render-geometry --output fresh.png --size 256

# Extract a saved image at its original dimensions.
catwalk part.ipt --extract-saved-preview --output saved.png
catwalk scene.lys --extract-saved-preview --output saved.png
catwalk model.3dm --extract-saved-preview --output saved.png
```

`--render-geometry` ignores embedded images and requires a supported geometry reader. `--extract-saved-preview` uses the saved image's original dimensions and cannot be combined with canvas-size, transparency, background, color or sampling options. For Inventor PNGs, extraction preserves the native bit depth. When the reader returns no saved image, extraction exits with code 3; corrupt or unsupported carriers can return an error.

## Output options

```sh
catwalk model.glb --output thumbnail.png --resolution 640,360
catwalk model.3mf --output thumbnail.png --transparent
catwalk model.obj --output - > thumbnail.png
```

On PowerShell versions before 7.4, use `--output thumbnail.png`; [binary stdout redirection is preserved from PowerShell 7.4 onward](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_redirection?view=powershell-7.5#redirecting-output-from-native-commands).

| Option | Effect |
| --- | --- |
| `--size 256` | Square PNG; default is 512 pixels |
| `--resolution 640,360` | Explicit width and height; `640x360` also works |
| `--transparent` | Transparent background with antialiased edges |
| `--background '#f5f7fa'` | Background color; shown value is the default |
| `--color '#64a0dc'` | Fallback color for geometry without material or vertex colors |
| `--samples 1` | Supersampling factor from 1 to 4; default is 2 |
| `--output -` | Write PNG bytes to stdout |

Use `catwalk --help` for all options. `--no-background` aliases `--transparent`, and `--list-readers` aliases `--list-formats`.

CLI dimensions are 1–4096 pixels per axis. Geometry rendering allows at most 16 million supersampled pixels; reduce dimensions or use `--samples 1` for a very large thumbnail. Inputs and referenced resources have size limits, with additional limits for individual readers documented in the coverage matrix.

Errors are written to stderr and return a nonzero exit code. Decode failures occur before the destination is opened, preserving an existing thumbnail. For an unsupported file variant, check its coverage entry and export to a supported encoding or geometry format. Unsupported native document variants may still need a new saved thumbnail from the authoring application.

## Use from Rust

Add the dependency:

```toml
[dependencies]
catwalk3d = "2"
```

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

`thumbnail` follows the same saved-image preference and geometry fallback as the CLI. `geometry_thumbnail` forces stored-geometry rendering. For supported documents, `saved_preview::extract` returns an image at its original dimensions, or `None` when the reader finds no saved image; malformed or unsupported carriers can return an error. `saved_preview::inventor::extract_png` preserves native PNG bit depth and strips ancillary metadata.

For custom scenes, `formats::load` produces a `scene::Scene` containing triangles, points, line segments and supported appearances. Pass it to `render::render`, or construct your own scene. `write_png` accepts any `std::io::Write`, including buffers and stdout. See the [API reference](https://docs.rs/catwalk3d/) for the types and functions.

When migrating Rust code to 2.x, include a wildcard arm in matches on the now non-exhaustive `scene::TextureWrap`. Its new `ClampToBorderWhite` mode preserves the underlying material outside a texture's unit square. OBJ material `Kd` now uses linear diffuse reflectivity consistently, so older OBJ material thumbnails can become brighter. OBJ previews also retain imported normals, per-corner UVs and RGB, local diffuse image maps and dissolve opacity; see the [OBJ coverage entry](docs/format-coverage.md#obj) for supported options and limits.

## Tests and example gallery

Published Cargo packages exclude test fixture files and snapshots. Small library unit-test inputs are authored in memory; real-file checks run as integration tests from a Git checkout. CI also runs the library tests from the extracted crate to verify they work without repository fixtures.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

CI checks formatting, strict Clippy, documentation and the publishable crate, then tests on Linux, macOS and Windows. It also compiles all seven release targets, runs every-extension image comparisons using each optimized executable, and tests the executable extracted from each archive. The aggregate job is named `Success`; it passes only when the checks, platform tests, gallery and binary preparation/builds pass. Linux tests run with display-related environment variables unset.

The [format corpus](tests/fixtures/formats/manifest.json) includes an encoded input for every advertised extension, plus additional encoding and scene examples. `cargo test --locked --test format_snapshots` renders each case through the CLI and compares decoded pixels with [committed PNG snapshots](tests/snapshots/formats). Missing fixtures, decoding failures and visual changes fail CI. Transparent pixels use premultiplied RGBA comparison, with a maximum per-channel tolerance of 2/255 for platform rounding.

CI retains actual, expected and amplified difference images for each platform. Successful runs publish the tested images to the [gallery](https://cmeister2.github.io/catwalk3d/), where you can search formats and compare snapshots. Saved-image fixtures include licensed original projects and authored image carriers; an image-carrier test alone establishes no geometry-rendering coverage.

For a local gallery after testing, run `cargo run --locked --example gallery -- --report target/format-snapshots` and serve `target/gallery` with a static HTTP server. To intentionally update baselines, use `cargo run --locked --example gallery -- --update-snapshots`, inspect the images and commit the PNG changes. Normal tests never update baselines. Authored inputs can be regenerated with `cargo run --locked --example generate_fixtures`.

## Releases

Successful CI on `main` automatically publishes `catwalk3d` to crates.io and creates a matching GitHub release. Publication waits for `Success`. Conventional commits select the version: `fix:` produces a patch, `feat:` a minor release, and a breaking change a major release. Documentation-only commits produce no release.

Each release includes `catwalk3d-VERSION-TARGET.zip` archives for these targets:

| Platform | Target |
| --- | --- |
| Linux x86-64, GNU | `x86_64-unknown-linux-gnu` |
| Linux ARM64, GNU | `aarch64-unknown-linux-gnu` |
| Linux x86-64, static musl | `x86_64-unknown-linux-musl` |
| Linux ARM64, static musl | `aarch64-unknown-linux-musl` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| Windows x86-64 | `x86_64-pc-windows-msvc` |

Every archive contains the executable, this README, Catwalk's license, generated dependency and Rust runtime license notices, and `release.json` with its version, source commit, compiler and target. Static musl archives also include the musl copyright notice. The release includes individual `.zip.sha256` files, provenance JSON sidecars and a combined `SHA256SUMS` file.

`semantic-release-cargo` prepares the actual release version before the platform builds and publishes the crate; the GitHub plugin attaches the verified archives. Publication checks the complete target set, versions, source commits and archive/member checksums before tagging. Main workflows are serialized to keep version planning and publication together. Pull requests have one CI run, with superseded runs canceled. Node dependencies are confined to release tooling. Dependencies are published crates with no local Cargo patches.

Maintainers supply a crates.io token through the GitHub secret `CARGO_REGISTRY_TOKEN`; `PUBLISH_SECRET` is also accepted. Publication skips while neither is configured. Local `.env` files are excluded from Git and Cargo packages.

## License

Catwalk's code and authored fixtures are MIT licensed. Retained upstream test assets keep their separately documented licenses and notices, including the AGPL-3.0 slicer examples. Catwalk is an independent thumbnail implementation.
