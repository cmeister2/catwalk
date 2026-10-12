# Original Lychee saved-preview scenes

These are unmodified keyboard-case projects from [JRiggles/DELPHI-kbd](https://github.com/JRiggles/DELPHI-kbd/tree/a666598d5916b73f8a3fac796e569878de18bb79), pinned at `a666598d5916b73f8a3fac796e569878de18bb79`. The author supplies presupported scenes with the original keyboard designs. The repository's MIT license, copyright 2022–25 John Riggles, is retained unchanged in [DELPHI-LICENSE](DELPHI-LICENSE).

| Retained input | Original path | Bytes | Source SHA-256 |
| --- | --- | ---: | --- |
| `delphi-edge-case-lp.lys` | `cases/DELPHI - Edge Case LP.lys` | 805,288 | `b1f145b784f1705582f86f980e7f93e0ef849746fe0f0f1e3bd395ea9535ce90` |
| `delphi-edge-case-hp.lys` | `cases/DELPHI Edge Case HP.lys` | 1,015,246 | `efb540d3aa82c581dc7fbeb34db07a91a93e8230a652f591a35ef1e2cd2640ac` |

Both are genuine Lychee 5.4.3 saves with scene-state version 2 and flat Mango/ASAR manifest version 3.1.0. The manifest addresses `preview.png` by an offset relative to byte 832. The retained `.png` files contain those original selected member bytes, preserving an independent saved-image reference rather than a new render.

| Original saved PNG | Bytes | Encoded SHA-256 | Decoded 512 × 512 RGBA SHA-256 |
| --- | ---: | --- | --- |
| `delphi-edge-case-lp.png` | 21,320 | `1a2fe62db9dbc709cd1c6fb7e5a3ef7b3c484d6e996569d46803f3a061ef6c9a` | `627b5a44e15db1817513082c43eceb70e7e7129b4a21194d3ada588a1ea88d06` |
| `delphi-edge-case-hp.png` | 86,679 | `b99e0763a77248b2094f50a00b0e83b407fbba1cb96ed22290351ba3727ba1d6` | `f071d4f2f3742c4bcdbb5b54f841a95519ba8d4389af9c085149487a90f7a247` |

Integration tests assert these literal source/image hashes, original PNG pixels, public extraction and thumbnail behavior. Separate CLI image snapshots fit both saved images to the 256 × 256 transparent corpus canvas; the Pages gallery includes their original MIT notice. Fixture generation only registers these inputs and never rewrites their bytes or approves new image baselines.

The saved previews show the authoring application's prepared scene. Native geometry, object transforms and support generation are outside this reader's scope. These two cases do not establish compatibility with every Lychee release, legacy ZIP scenes, `.lyt`, multiple plates or linked library assets. All fixture assets are excluded from the published crate.
