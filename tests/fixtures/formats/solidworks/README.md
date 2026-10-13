# Original SolidWorks document thumbnails

The native documents are copied byte-for-byte from [GuoXiong's Power-Package](https://github.com/guoxxiong/Power-Package) at commit `bc6b8d0ca0fe472a4f0ed91c9e64f986b3ddc7e6`. They are separately MIT licensed; the complete original notice is retained in `POWER-PACKAGE-LICENSE.txt`. Filename case is normalized locally without changing file contents.

| Local file | Original path | Source SHA-256 |
| --- | --- | --- |
| `solidworks-about.sldprt` | [`about.SLDPRT`](https://github.com/guoxxiong/Power-Package/blob/bc6b8d0ca0fe472a4f0ed91c9e64f986b3ddc7e6/about.SLDPRT) | `08a8ec2afc112a27b1b8f4fad92224080b5c0d8cda8fc095b423cf91ca062e34` |
| `solidworks-powerpackage.sldasm` | [`powerpackage.SLDASM`](https://github.com/guoxxiong/Power-Package/blob/bc6b8d0ca0fe472a4f0ed91c9e64f986b3ddc7e6/powerpackage.SLDASM) | `2e71fb43b889a5d9585eb7606266a1dd35ef685244df6ee65782a7f7b2f19788` |

Both files use CFB v3 with 512-byte sectors. Their exact root `PreviewPNG` streams are 640×480 RGBA PNG images. The `.preview.png` references are the original stream bytes extracted independently with Python `olefile`, without using Catwalk's extractor. The `.preview-dib.png` references are the original root `Preview` DIBs independently decoded with Pillow 10.2: strip the four-byte length, add a BMP file header with its palette-adjusted pixel offset, decode and save RGBA. The original DIBs are 1418×696 RLE8 images. `provenance.json` records document, PNG stream, DIB stream and decoded DIB pixel hashes.

The part shows a simple block. The assembly shows a power-package enclosure and components. No referenced assembly parts are retained: extracting its root image does not need them. These are actual native producer documents, including their geometry and metadata; the corpus renders their saved image and does not decode model geometry.

`tests/solidworks_saved_previews.rs` fixes source hashes, compares public extraction to the independent PNGs, and exercises the original RLE8 fallback by deleting only `PreviewPNG` in temporary copies. Unit tests author CFB v3/v4 carriers for selection, absent/corrupt images, original mini-FAT cycles and allocation limits. Authoring a carrier or renaming a source does not qualify a SolidWorks application version.

Modern compressed-section containers are a separate format envelope and remain unsupported. Preview extraction establishes neither Parasolid geometry support nor universal valid-file compatibility.
