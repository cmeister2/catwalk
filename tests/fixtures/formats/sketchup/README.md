# SketchUp producer geometry and thumbnail fixtures

Unchanged retail SketchUp documents from [hew3d/openskp](https://github.com/hew3d/openskp/tree/a6788afa6386cfd4bc781998dca99c591b3fb069/corpus), pinned at `a6788afa6386cfd4bc781998dca99c591b3fb069`.
`legacy-box-v2013` through `legacy-box-v2016` are `corpus/legacy/box-vYEAR.skp`; `2017-*` and `2026-*` are the same-named files in those corpus directories. The 2026 files were saved by SketchUp Web as version 26.2.0; the 2017 files declare 17.3.116, and the earlier copies declare 13.0.1–16.0.1.

The upstream authored geometry is GPL-3.0-only. The original license and original corpus notice are retained alongside these separate test assets. Bundled Trimble content (including the default scale figure and watermark) remains Trimble's; the upstream notice describes that distinction. No upstream parser code is compiled or copied into Catwalk's MIT-licensed runtime.

The independent PNGs are the first document-model CDib payload in legacy files, and ZIP member `meta/model_thumbnail.png` in 2026 files. Python `struct` and `zipfile` extracted them before implementing the reader. All are 256 × 256 pixels. The later legacy preview, scene/component thumbnails, material images and modern `meta/preview_thumbnail.png` are deliberately excluded. The unchanged files contain those other images, so they also guard against selecting arbitrary PNG signatures. The independent PNGs qualify saved renders; geometry has separate producer-export witnesses below.

| File | Bytes | Source SHA-256 | Independent encoded PNG SHA-256 |
|---|---:|---|---|
| `2017-box-two-materials.skp` | 136920 | `a76ead95df577a786b8766f2e231f6da3b6c7a79d51b51017820dd0b219073ac` | `f6395835a3788903976d5ced13cf297d8b182aeb3234791b5c636beb71f358d2` |
| `2017-box.skp` | 136715 | `ac4064eb4d52bd06fd3392fc16bdd146533f2e1291ce2a544b74a2e0da5c7ce9` | `a3c39cc707f6db8ccbda5ec347bdbdd27d992b3f9ba402ac72c85ae3a8d1db89` |
| `2026-box-two-materials.skp` | 181868 | `e884ec08a9a9d21f3fa7e222dd0f07915ee6783faf909b0fe7403460b910ad69` | `8a2500083a93bf3d966ee3bf7525292e84c5e8c12f6b9267f461b6215b0d793a` |
| `2026-box.skp` | 180592 | `2026acb0b49edb78a63c279c867937ee3ee1e53813459e23dd37666f0d6217e5` | `f57f6b448482a9219c6609acf21a8410dab56c7fd33943d91533cdf84825d538` |
| `legacy-box-v2013.skp` | 71178 | `b2bc194ccee1bba12cf2a4371a4f2ef42f90ad81b55fd294feb71a46fb33f83a` | `a3c39cc707f6db8ccbda5ec347bdbdd27d992b3f9ba402ac72c85ae3a8d1db89` |
| `legacy-box-v2014.skp` | 131113 | `72f41969ff4e08cf68e5d43c0fdaa7480c8c342fb94d418e0d334fbdcc293de4` | `a3c39cc707f6db8ccbda5ec347bdbdd27d992b3f9ba402ac72c85ae3a8d1db89` |
| `legacy-box-v2015.skp` | 131113 | `fb9bc7147754dd4bc472d618c6b04c37b05739f7784816397ff0dfb49ddcbef3` | `a3c39cc707f6db8ccbda5ec347bdbdd27d992b3f9ba402ac72c85ae3a8d1db89` |
| `legacy-box-v2016.skp` | 131113 | `4e055123353b77d3e75d10023939f50811055f5a0bc1dfcc9a3231f8311278d9` | `a3c39cc707f6db8ccbda5ec347bdbdd27d992b3f9ba402ac72c85ae3a8d1db89` |

The referenced producer set qualifies only the documented framing and PNG subtype. It does not establish every SketchUp release or file variant. 2018–2025, alternate VFF headers and JPEG thumbnails remain outside saved-preview qualification. Legacy extraction walks the declared version map and fixed document prefix; modern extraction checks the metadata model-image descriptor. No signature scan selects an image.

## Geometry witnesses

Fresh geometry reads the native model records, skips saved-image bytes, and expands the visible model's component tree. The additional `2026-*.skp` files and `*-export.dae` witnesses are unchanged files from the same pinned upstream revision and retain the original corpus license and bundled-content exclusions described above.

`tools/measure_sketchup_exports.py` measures the original COLLADA exports using Python's standard XML parser, independently of Catwalk and the native SKP reader. `geometry-witnesses.json` retains world-space bounds in millimetres and one-sided surface areas, including hole subtraction. SketchUp's COLLADA exporter writes both sides of each face; witness areas divide the exported area by two. Exported line segments are included in bounds. The tests compare nine native producer models with these measurements, covering holes, concave faces, nested definitions, shared occurrences, translations, rotations, anisotropic scale and mirroring. The 1 m cube separately checks twelve triangles, area and outward signed volume.

The two-materials box has authored dimensions 34.5 × 74.875 × 26.5 inches; it is a distinct model from the 1 m cube. Its named materials are decoded from addressed material XML and assigned to the native front faces.

| Geometry/witness file | SHA-256 |
|---|---|
| `2026-arc.skp` | `920d1ba2d8260f98e6b83926d2a77e6b9513d0c36c602b9bb882214d8b814cd0` |
| `2026-box-component-two-instances.skp` | `03fff4b0060246cff633df719b045ac4f77745e374ed38da096f9ddfb461dc6e` |
| `2026-box-two-materials.skp` | `e884ec08a9a9d21f3fa7e222dd0f07915ee6783faf909b0fe7403460b910ad69` |
| `2026-box.skp` | `2026acb0b49edb78a63c279c867937ee3ee1e53813459e23dd37666f0d6217e5` |
| `2026-component-move.skp` | `4bbbded6ff356d39f5f6be7ddbcf29ca8d19efa4074d62df041a0d082d2da905` |
| `2026-component-rotate.skp` | `48a394d12654474dd79219677101c8425e30cfa178766817e2224e4c51549150` |
| `2026-curve.skp` | `1f1930c8cf09bbe921564ed0f648091e7988d18f9e29a2a07b2493ce746a1ea2` |
| `2026-face-with-hole.skp` | `00736422873c1d4f8c962acfc40407f627788d5037337825fbea077d6d698d4b` |
| `2026-hidden-entities.skp` | `81d07aa758413334ad084d3e987c9f4002a5d2559574ba1f8d40c50f27d5e615` |
| `2026-instance-scaled.skp` | `06f628229eb8388c7c2bb305fe19365f0c7e1ec45fbde99c36da89fa513e7f5f` |
| `2026-layers.skp` | `18c52c86fbf0ec40e2976e02cee0a5f968d2d7396f8f81fb5a19f30c09b93707` |
| `2026-nested-3-deep.skp` | `318e5a9e527c8f9f5ef65187fe55b4b148cce4e872a78ec38452b21bc5d2b7e5` |
| `2026-ngon-face.skp` | `d10be7c356d6eef9eb4eb38850323ca5c0d222c1d056ba8b318d7682bfeefca6` |
| `arc-export.dae` | `161846aaccced05ddfee3503afd2a5d3289f55d3b6b89c6ce7462184623380ea` |
| `box-component-two-instances-export.dae` | `f942264cd419f783f88d66bc66de23b3a1bb3a2dc80b23b6066a5500255aae5c` |
| `box-export.dae` | `3d38dc4753526b440471e650d65e70110bd003a47a4c239cffe7432fb9d0fbc8` |
| `box-two-materials-export.dae` | `a13430116be6dec56d55d32eefdbd18ca850e62c7b3427de22a744ccfc7fbac9` |
| `component-move-export.dae` | `d90fb4b73be327a1a4ed761b27516115357fe30a913a6d8b9dd95fc9ca89b957` |
| `component-rotate-export.dae` | `b119df298569ed18424a56d9ce81eadafbd6923162ffb84b8960cded61fe4212` |
| `curve-export.dae` | `38d2b07d6e4e966b1fdc463c21e835db625fb14f4f0589b39b5bb3ed9c3d1a48` |
| `face-with-hole-export.dae` | `10ade360548fd04f5ea6d6ceefd34ebe89a55a69f03438cf27d0b3ed73d7de4d` |
| `geometry-witnesses.json` | `c5900c0b493a50e11b441c0c4e0572b0a01798ee627392aab135b2527275e1f9` |
| `hidden-entities-export.dae` | `f91b7ed9f0488b08daf310dacf9ffce8ba4ce1b4fef09ecf0705b26d90b2b813` |
| `instance-scaled-export.dae` | `335b906fbdb5ac8637fb90d0f770629f842c9e55e726804cff1d437fa1ef0787` |
| `layers-export.dae` | `f270b094b662febe8ac0a573f378c5dbf5dfd2211b27070920fb193f429d27d9` |
| `nested-3-deep-export.dae` | `f81b5eca6d6f71f5b930d0a8e1da3fe1deaf57d23a4b6fe2d046b45fcfcaefe0` |
| `ngon-face-export.dae` | `f95606172d6cd4a7fef054e71906d7c70923302931a615dac0de4415ce0209c0` |

Legacy geometry additionally qualifies all six original 2013–2017 box documents, two repeated component occurrences, rotation and holes. Native model geometry is unchanged when the addressed CDib image payload is removed from temporary copies while retaining its archive slot. Prefix reference and non-finite vertex controls fail explicitly.

| Additional unchanged legacy geometry file | SHA-256 |
|---|---|
| `2017-box-component-two-instances.skp` | `8daa7e6751e90b0e10d06aad6a2c6ef32f53ee30933cc0b776060f453ebdbf07` |
| `2017-component-rotate.skp` | `e9c5c4249b94375479866bb7eed18d922250dd01c5304b3f1f3bc6f6f451a37c` |
| `2017-face-with-hole.skp` | `ba455819b7be02c441a062ace8d87aaf140e5b4fab5b8a5cf8a75d962f6d9f2b` |

## Visibility and selected annotations

The unchanged `2026-layers.skp` contains 1 m, 0.5 m and 0.25 m cubes. Its Medium tag is hidden, so the visible native surface area is 6,375,000 mm². The original COLLADA export includes the hidden cube and measures 7,875,000 mm²; it is not a visibility oracle. Explicit native controls check the hidden current tag remains displayed, omitted layer refs obey the default tag, geometry containers do not acquire drawing-entity tag semantics, unresolved current tags fail, and hidden default tags do not hide explicitly tagged placed geometry.

The original dimension fixture checks that selected visible annotations fail explicitly, while hidden annotations and annotation-bearing unplaced definitions do not block selected model geometry. Source from the same pinned upstream `corpus/2026/dimension.skp`; the original corpus license and notice apply.

| Additional annotation source | SHA-256 |
|---|---|
| `2026-dimension.skp` | `b31541f2686fd2565b4b41d4c2ab19f2e11a40be93279abb6760594300e76e9c` |
