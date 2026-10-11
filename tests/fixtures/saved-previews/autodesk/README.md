# Original Inventor thumbnail carriers

These two files were authored for Catwalk and are covered by the repository's
MIT license. They contain CFB/OLEPS property-thumbnail carriers, not Inventor
CAD geometry. They test preview extraction and snapshots, and cannot establish
that Inventor can open the files as complete designs.

| File | Container | Saved image |
| --- | --- | --- |
| `part-carrier.ipt` | CFB v3, root native-summary FMTID and PID17 VT_CF | 160 × 128 RGBA PNG showing Catwalk's original gear |
| `assembly-carrier.iam` | CFB v4, same thumbnail property carrier | 280 × 160 RGBA PNG composing two original gear images |

Reproduce them with:

```sh
cargo run --locked --example generate_inventor_fixtures
```

The source image is rendered from `examples/models/gear.obj`. The reusable
fixture writer is `examples/fixtures/inventor.rs`. Portable parser tests
generate smaller regular/mini-stream, PNG/DIB/BMP, missing, malformed and
resource-limit cases separately.

Model-specific source files and extracted images are not included in this portable fixture corpus.
