# LightBurn saved-preview qualification fixtures

These are unmodified public project files and the exact PNG bytes already saved
in their direct `LightBurnProject/Thumbnail` element's `Source` attribute.
The PNGs are expected extraction snapshots, not newly rendered geometry. A saved
thumbnail may be stale. In particular, the legacy sample's saved image reads
"Test" even though its project includes a larger generated material-test layout.

| Fixture | Recorded application / format | Original source | License |
| --- | --- | --- | --- |
| `legacy-example1.lbrn`, `legacy-example1.png` | 0.9.16 / 0 | [MarcinZukowski/lightburn-tester example1](https://github.com/MarcinZukowski/lightburn-tester/blob/3f51e497f0d899209f762c8d507a2a08322a6a54/examples/example1.lbrn) | Apache-2.0; `LICENSE-lightburn-tester.txt` |
| `current-jig.lbrn2`, `current-jig.png` | 1.4.03 / 1 | [tscircuit/lbrnts jig sample](https://github.com/tscircuit/lbrnts/blob/d21a7b29354cd782cabf533b1b8f0ade6a11c7dd/tests/assets/printable_area_jig.lbrn2) | MIT, Copyright (c) 2025 tscircuit Inc.; `LICENSE-lbrnts.txt` |

Both carriers decode to 256 × 256, 8-bit RGBA PNGs. The supplied expected images
retain every saved pixel. Tests compare decoded RGBA exactly, so PNG encoder
compression differences do not change the snapshot result.

SHA-256:

```text
181072c4c881dbca5af7c510b63bf6d7dceb9c05bc8f1d2f4364d26eaa077552  legacy-example1.lbrn
b01e37b542519aa34a432d947179391fd95cc9655bb74ab2e04730a2dd3a39ab  legacy-example1.png
a13336c9c1af59c1e229cb4d9f2387e45a4ac84c06a067d079f14f1fad30f784  current-jig.lbrn2
61b41e71b380140a0decb980823611f5b98aa80d775cb1d35ddecfe9c3042a1e  current-jig.png
```

Run the extraction and robustness tests with:

```sh
cargo test --locked --lib saved_preview::lightburn
```

The larger qualification corpus is documented in
[`docs/research/lightburn-preview.json`](../../../../docs/research/lightburn-preview.json).
Those local checks are separate from the two retained CI fixtures. Most absent
cases are deliberately minimal third-party parser fixtures, so their count is
not an estimate of how often authoring-application saves contain a preview.
