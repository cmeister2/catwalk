# Experimental Inventor and Fusion geometry previews

This separate Rust tool generates new thumbnails from saved BRep geometry in selected `.ipt` and `.f3d` files whose saved preview has been removed. It is **not part of the default Catwalk CLI**, and it does not establish general Inventor or Fusion compatibility. `.iam` assemblies return an explicit unresolved-dependency error.

The runtime decodes native geometry, writes and checks STEP in memory, and uses Catwalk's STEP tessellator and renderer. No Autodesk installation, native codec, external converter process or saved image is used. A temporary STEP file bridges Catwalk's current path-based API. `bootstrap.sh` uses Git during setup to fetch pinned source; it is separate from the thumbnail runtime.

## Reproduce

Run from the repository root with Rust 1.99 or newer:

```sh
./tools/autodesk_geometry/bootstrap.sh
cargo test --locked --manifest-path tools/autodesk_geometry/Cargo.toml --all-targets
cargo run --locked --manifest-path tools/autodesk_geometry/Cargo.toml --bin autodesk-preview -- \
  tools/autodesk_geometry/evidence/fixtures/inventor-cylinder-without-preview.ipt \
  /tmp/inventor-cylinder.png /tmp/inventor-cylinder.json
```

The tool requires a PNG output path and rejects paths that would overwrite the input or put the PNG and report in the same file. Existing Unix hard-link aliases are also checked. The report records all native decoder findings and the body, face, edge and triangle counts. Sources are large and a first debug build takes several minutes; upstream source is downloaded into ignored `.upstream`, rather than vendored into this repository.

```sh
cargo fmt --manifest-path tools/autodesk_geometry/Cargo.toml -p catwalk-autodesk-geometry -- --check
cargo clippy --locked --manifest-path tools/autodesk_geometry/Cargo.toml --all-targets -- -D warnings
cargo tree --locked --manifest-path tools/autodesk_geometry/Cargo.toml --edges normal,build
```

## Verified, genuinely generated images

| Public model | Saved BRep | Rendered triangles | Generated PNG |
| --- | --- | ---: | --- |
| Adafruit 2020 extrusion `.f3d` | 1 body, 107 faces, 314 edges | 444 | [Extrusion](evidence/outputs/fusion-extrusion.png) |
| Adafruit slide switch `.f3d` | 1 body, 19 faces, 46 edges | 56 | [Switch](evidence/outputs/fusion-switch.png) |
| Inventor 2027 cylinder `.ipt` | 1 body, 3 faces, 2 edges | 92 | [Cylinder](evidence/outputs/inventor-cylinder.png) |
| Older Inventor rounded die `.ipt` | 1 body, 49 faces, 48 edges | 948 | [Die](evidence/outputs/inventor-dice.png) |

Each test checks that Catwalk can extract the original saved preview and cannot extract one from the derived file. It then decodes both files, requires identical geometry fingerprints and geometry counts, and renders identical pixels. The current cylinder additionally checks its independently observed 12.5 mm radius, 10 mm length and analytic surface area. The switch's history archive must not create duplicate active bodies.

PNG snapshots compare premultiplied RGBA with a maximum per-pixel channel difference of 2; fully transparent RGB is ignored. Fresh actual, expected and amplified difference PNGs, plus geometry reports, are written to `CATWALK_AUTODESK_ARTIFACTS`, or `target/real-geometry-snapshots` beneath this tool by default.

The bridge refuses geometric, topological or product losses, and checks that its STEP round trip preserves exact body, shell and face counts. It does not silently omit unsupported bodies. Two saved parameter ordering errors in the older die are explicitly tolerated because that fully evaluated BRep does not require feature replay; those findings remain in the report. Unknown metadata and non-geometric feature-history warnings are also retained.

## Dependency and format findings

The native decoders come from [cadmpeg at `9e8f6d6b5da9371bc791ce3924f364aea81be0ec`](https://github.com/cadmpeg/cadmpeg/tree/9e8f6d6b5da9371bc791ce3924f364aea81be0ec), version 0.6.1, with Apache-2.0 source. The corresponding format documentation has its own CC-BY-4.0 license. No documentation text is copied here. The tool's lockfile pins the separate build graph.

Unmodified upstream pulls in C-backed `zstd-sys` even with codec default features disabled. [Our retained patch](patches/pure-rust-zstd.patch) replaces that decoder with `ruzstd`, removes ZIP's native Zstandard feature and the unused container dependency, and adds the observed Inventor RSe compression-tag dispatch. The tested graph has **no `zstd-sys`, `zstd-safe`, `cc`, `cmake` or `bindgen`**; see [the dependency audit](evidence/dependencies.txt). This limited adapter accepts one exact ordinary Zstandard frame, enforces a 64 MiB window limit before allocating, validates content checksums and rejects trailing, concatenated or truncated data. It is an experimental patch, not an upstream production release; ZIP entries using Zstandard are unsupported in this build.

The [Fusion format reference](https://github.com/cadmpeg/cadmpeg/blob/9e8f6d6b5da9371bc791ce3924f364aea81be0ec/docs/formats/f3d.md) matches the observed ZIP members. The extrusion's `Breps.BlobParts/BREP.*.smb` starts with `ASM BinaryFile8`. The switch has both `.smb` active geometry and `.smbh` history. All six independently examined Adafruit archives contained binary BRep, but none contained an OGS mesh cache. Rendering their BRep therefore demonstrates more than cached-image or cached-mesh extraction.

The [Inventor format reference](https://github.com/cadmpeg/cadmpeg/blob/9e8f6d6b5da9371bc791ce3924f364aea81be0ec/docs/formats/inventor.md) and independently observed CFB streams locate paired `RSeStorage/M<token>` metadata and `RSeStorage/B<token>` bulk streams. At bulk byte 16, the old die uses bytes `04 01` and the current cylinder uses `04 02`: version 4, followed by a compression selector. Zlib or Zstandard data begins at byte 18. The current cylinder's expanded `Bx6bcdpeygze5t34bmwbo4iyoff` stream contains a framed `PmBRep` record with `ASM BinaryFile4`; the record type is `5c5945f6d5113313100060a6bba647b5`. The decoder follows typed segment records, rather than treating signature searches as valid carrier locations. Inventor's evaluated BRep can be used without recreating parametric feature history.

ACIS-family magic alone does not establish compatibility with a generic SAT/SAB parser. These proofs use the dedicated ASM decoder and native container record framing, not a blind ACIS signature scan.

## Remaining work and limits

- **Fusion:** three of six examined public files (two batteries and a breadboard) fail because active design components lack a context UUID binding understood by the decoder. A 37-body sensor board decodes but reports an untyped solved-geometry record; this conservative bridge refuses it. Component/context resolution and procedural surface qualification need additional real fixtures before wider support can be claimed.
- **Inventor parts:** the two proven producer families do not cover all versions, model states or kernel surfaces. Other older files decode BRep but currently hit unsupported elliptic-cone STEP export. An elliptic cylinder must not be replaced with a circular cylinder to make rendering pass. A sketch-only file has no solid BRep and is rejected explicitly. Additional native graphics-only or sheet cases need separate qualification.
- **Inventor assemblies:** `.iam` requires external part resolution, nested occurrence transforms and model-state selection. `UFRxDoc`, `AmDc` and `AmGraphics` contain relevant references and placements, but the examined standalone assembly does not contain a complete resolved set of component solids. Implement a bounded relative-path resolver and verify occurrence transforms against independent source examples before enabling assembly rendering. Replaying feature history is a different task.
- **Production integration:** audit the sizeable upstream parsers, qualify more versions and surface types, preserve detailed unsupported errors and keep thumbnail resource budgets before adding a default loader. Saved thumbnails may remain an explicit fallback, but must not masquerade as generated geometry.

## Input provenance and preview removal

[The manifest](evidence/manifest.json) records pinned public source URLs, Git revisions, source and derived SHA-256 digests, licenses and output geometry fingerprints. The Adafruit examples and the simple MetaReader cylinder/subassembly are covered by their repositories' MIT licenses. The InventorLoader die fixture and its derived copy are retained with that repository's GPL-2.0 license; none of its parser code is copied into this tool. Exact license texts are kept alongside the fixtures.

Fusion derived files remove only named `/Previews/` ZIP members. Every remaining uncompressed member payload is byte-identical. Inventor derived files remove only PID 17 with type `VT_CF` from the qualified native or legacy summary-information section. Every other property payload, every RSe stream and all other top-level CFB streams are preserved; [stream-by-stream audits](evidence/preview-removal.json) record before/after digests. Cached reference files inside `RSeStorage/RefdFiles` are not modified. These copies remove the document's recognized saved-preview carrier; the geometry runtime never reads an image carrier, including nested reference previews.

The fixture derivation helpers are independent research tools:

```sh
python3 tools/autodesk_geometry/scripts/strip_fusion_preview.py INPUT.f3d OUTPUT.f3d
cargo run --locked --manifest-path tools/autodesk_geometry/Cargo.toml --example strip_inventor_preview -- \
  INPUT.ipt OUTPUT.ipt > /tmp/stream-audit.json
```

The Inventor helper is deliberately limited to the qualified property-set layout and refuses malformed property tables. These helpers are not used by the thumbnail runtime.
