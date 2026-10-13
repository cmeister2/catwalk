# SketchUp producer thumbnail fixtures

Unchanged retail SketchUp documents from [hew3d/openskp](https://github.com/hew3d/openskp/tree/a6788afa6386cfd4bc781998dca99c591b3fb069/corpus), pinned at `a6788afa6386cfd4bc781998dca99c591b3fb069`.
`legacy-box-v2013` through `legacy-box-v2016` are `corpus/legacy/box-vYEAR.skp`; `2017-*` and `2026-*` are the same-named files in those corpus directories. The 2026 files were saved by SketchUp Web as version 26.2.0; the 2017 files declare 17.3.116, and the earlier copies declare 13.0.1–16.0.1.

The upstream authored geometry is GPL-3.0-only. The original license and original corpus notice are retained alongside these separate test assets. Bundled Trimble content (including the default scale figure and watermark) remains Trimble's; the upstream notice describes that distinction. No upstream parser code is compiled or copied into Catwalk's MIT-licensed runtime.

The independent PNGs are the first document-model CDib payload in legacy files, and ZIP member `meta/model_thumbnail.png` in 2026 files. Python `struct` and `zipfile` extracted them before implementing the reader. All are 256 × 256 pixels. The later legacy preview, scene/component thumbnails, material images and modern `meta/preview_thumbnail.png` are deliberately excluded. The unchanged files contain those other images, so they also guard against selecting arbitrary PNG signatures. These are saved renders, not fresh Catwalk geometry.

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

The referenced producer set qualifies only the documented framing and PNG subtype. It does not establish every SketchUp release or file variant. 2018–2025, alternate VFF headers, JPEG thumbnails and model geometry remain unsupported. Legacy extraction walks the declared version map and fixed document prefix; modern extraction checks the metadata model-image descriptor. No signature scan selects an image.
