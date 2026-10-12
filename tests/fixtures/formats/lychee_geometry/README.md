# Lychee stored-model witnesses

The native `.lys` contract renders explicitly addressed original stored model meshes with their scene placements. It excludes supports, rafts, hollowing, holes and operation geometry. Automatic thumbnails still prefer an addressed saved PNG; a qualified model without that PNG falls back to native rendering.

Two original STL files are retained unchanged from [JRiggles/DELPHI-kbd at a666598d5916b73f8a3fac796e569878de18bb79](https://github.com/JRiggles/DELPHI-kbd/tree/a666598d5916b73f8a3fac796e569878de18bb79/cases). Their original Lychee scenes remain in `../lychee`. Both share the original MIT notice in `../lychee/DELPHI-LICENSE`, copyright 2022–25 John Riggles.

| Original STL | Bytes | SHA-256 |
| --- | ---: | --- |
| `delphi-edge-case-lp.stl` | 318,084 | `59f649fc05a795d1d0705c5447a43803082b9486bc2b0269b38c563adc3a0661` |
| `delphi-edge-case-hp.stl` | 320,084 | `6a2da7a8e4ba6dfb00761c706ac6a26de554337cda215552d8a2cb890067303d` |

The integration tests compare every original STL triangle corner with the indexed native mesh after the single recorded float32 source-center subtraction: 57,240 low-profile and 57,600 high-profile scalar comparisons, all bitwise exact. They also compare every emitted placed corner against independently derived fixed matrices, bounds, surface area and signed volume. Stored native coordinates are already centered; neither `center` nor `formerCenter` is subtracted during placement.

Four neutral containers are authored by `examples/fixtures/lychee_geometry.rs` under the project MIT license. They use the qualified flat Mango manifest 3.1.0, scene-state version 2, resin kind `MSLA`, and indexed mesh version 2. They are format controls, not files saved by the Lychee application.

| Control | Independent assertion |
| --- | --- |
| `missing-preview.lys` | Four source triangles render automatically when no saved PNG exists. |
| `asymmetric-placement.lys` | Nonuniform scale `(2,3,0.5)`, global X/Y/Z rotations `(30,45,60)` and XY/lift `(11,-7,9)` match literal corners. Lift uses actual transformed vertices. |
| `shared-visibility.lys` | Two distinct visible placements share one source mesh; hidden and off-scene occurrences add no geometry. |
| `preparation-ignored.lys` | Support, raft, hollowing and hole metadata, hole/operation objects and an unrelated derived mesh leave the selected original triangles unchanged. A differing producer `appVersion` and preparatory `properties.lysHash` retain the original `properties.hash` association. Its distinct authored 16×8 PNG remains preferred by automatic thumbnails. |

The tests also frame private corrupt-PNG and incorrect-PNG-integrity variants without changing the valid addressed scene or source mesh. Automatic thumbnails and forced geometry must render the literal original model, while explicit saved-image extraction reports an error. The producer version string is metadata; qualified manifest, scene-state and mesh schema versions remain explicit.

The additive scene byte protocol constants follow the public MIT [Open-Resin-Alliance/df-plugin-lys parser at cc4e64805d2e31095b084e244ef884018c92d1ba](https://github.com/Open-Resin-Alliance/df-plugin-lys/blob/cc4e64805d2e31095b084e244ef884018c92d1ba/LysParser.ts). The unchanged original notice is retained as `DF-PLUGIN-LYS-LICENSE`, copyright 2026 Open Resin Alliance. No library asset lookup, external file resolution, preparation regeneration or compatibility with other scene/mesh versions is established by these fixtures.
