These files exercise saved thumbnail extraction, independently of Catwalk's geometry renderers.

`blend-cube.blend`, `blend-280.blend` and `blend-extras.blend` are unchanged fixtures from [Khronos glTF-Blender-IO](https://github.com/KhronosGroup/glTF-Blender-IO/tree/ccc2cc6ea93f7618df42b56f6001538a8366f41c/tests/scenes), redistributed with [LICENSE-Khronos.txt](LICENSE-Khronos.txt). Their actual headers identify Blender 2.78, 2.80 and 5.2. The last file uses seekable Zstandard compression and the new 17-byte header / 32-byte block-header layout. Corresponding PNGs are mechanically extracted saved scene thumbnails, with Blender's bottom-up RGBA rows flipped for PNG display.

`blend-gzip-252.blend` and `blend-gzip-no-preview.blend` are unchanged [Assimp BLEND fixtures](https://github.com/assimp/assimp/tree/master/test/models/BLEND), redistributed with [LICENSE-Assimp.txt](LICENSE-Assimp.txt). Original names are `MirroredCube_252.blend` and `BlenderDefault_250_Compressed.blend`; both actual headers identify Blender 2.52. The former contains a saved thumbnail; the latter has a valid `REND` followed by `GLOB` without a `TEST` carrier, and returns `None`. These files are under `test/models`, outside Assimp's explicitly excluded `test/models-nonbsd` directory. The PNG is a mechanical saved-thumbnail extraction.

The `authored-*` files and their PNGs are original MIT-licensed fixtures. They contain minimal structurally valid thumbnail carriers rather than complete scene/drawing geometry. They are intentionally unsuitable for testing whether Blender or AutoCAD can open a complete model. Their asymmetric 64×48 image makes row order, palette order, opacity and transparent PNG preservation visible:

- `authored-legacy-32.blend`: Blender 2.80 header, 20-byte blocks and raw RGBA `TEST`.
- `authored-legacy-64.blend`: Blender 2.80 header, 24-byte blocks and raw RGBA `TEST`.
- `authored-format1.blend`: Blender 5.1 header, 32-byte blocks and raw RGBA `TEST`.
- `authored-dib.dwg`: AC1032 header with an explicitly addressed, sentinel-delimited preview table, type 1 metadata and type 2 8-bit palette DIB. Reserved palette alpha is ignored, as required by DIB.
- `authored-png.dwg`: the same preview table with type 6 RGBA PNG.
- `authored-no-preview.blend` and `authored-no-preview.dwg`: recognized containers without saved thumbnail carriers. `authored-metadata-only.dwg` has a valid preview section with type 1 metadata and no image.

[manifest.json](manifest.json) records every input and image hash. Supported carriers and limitations are documented in the [format coverage matrix](../../../../docs/format-coverage.md).

The committed images are exact decoded-pixel baselines. Updating them requires intentional review; the tests do not overwrite them. No Blender, AutoCAD, native DWG library or native image converter runs during extraction or comparison.
