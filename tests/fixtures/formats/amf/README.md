# Original Slic3r AMF references

These separately licensed test assets are unchanged files from the official
[Slic3r repository at commit 026c1380e0ad12176dd2fc8cdf8f6f181deeb071](https://github.com/slic3r/Slic3r/tree/026c1380e0ad12176dd2fc8cdf8f6f181deeb071).
The repository's complete original GNU Affero General Public License version 3
is retained as `SLIC3R-LICENSE`. Catwalk's MIT license applies to its code and
original authored tests, rather than these external assets.

| Corpus filename | Original upstream path | SHA-256 |
| --- | --- | --- |
| `amf-slic3r-plain.amf` | `src/test/inputs/test_amf/20mmbox.amf` | `ff3be11c5dc4f31e683afeab1008400c95170ea99008e37c6dfab4be30e157c3` |
| `amf-slic3r-deflated.amf` | `src/test/inputs/test_amf/20mmbox_deflated.amf` | `f707308b505efe4572b80febc707bdc0b8dac03a014d4a85e5f73b9f8bf8944f` |

The XML identifies its exporter as `Slic3r 1.3.1-dev`. The ZIP is the official
compressed-reader fixture; its sole `20mmbox.amf` member is byte-identical to
the plain export. This does not claim that Slic3r created the ZIP container.
Only corpus filenames changed; neither XML nor compressed bytes were edited.

Despite its upstream filename, the actual object is a 20 × 20 × 10 mm box:
eight vertices, twelve triangles, surface area 1600 mm² and signed volume
4000 mm³. Its constellation places it at bounds (57.5, 25, 0) to (77.5, 45, 10)
mm, with zero Z rotation and uniform scale one. Independent tests require
those measurements and matching pixels between XML and ZIP.

These are geometry previews, not saved-image extraction. The two files
qualify this exporter example and compression form; they do not establish
all AMF features or compatibility with every slicer version. Regeneration
registers retained files without a slicer executable or network request.
