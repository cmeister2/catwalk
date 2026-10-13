# Original Autodesk 3ds Max saved thumbnails

`Combine Test.max` and `Spherify Test.max` are unchanged native scene files from
[Autodesk ADN's 3ds Max MCG samples](https://github.com/ADN-DevTech/3dsMax-MCG-Samples)
at revision `820d8a32badf916849282081ca42416a88d2f6b7`. The upstream README describes
these `Scenes/Tests` documents as examples for the supplied Max Creation Graph
tools. They qualify saved images only; Catwalk does not interpret their scene
geometry or execute MCG/plugin code.

The original MIT license is retained byte-for-byte as `LICENSE-Autodesk.txt`
(SHA-256 `55815a27053bafdefdb8b14fe79536f3af21f1b087fe3352f3aa6eeb6212ab74`).
`provenance.json` records exact pinned producer URLs, source paths, Git blob IDs,
file sizes, source SHA-256 checksums and independently decoded RGBA checksums.
The documents are not reduced, reconstructed or resaved.

| Original document | Bytes | Saved dimensions | Saved RGBA SHA-256 |
| --- | ---: | --- | --- |
| Combine Test.max | 630784 | 512 × 256 | `c06792b6a85f99a7eaf3991cacd2a37fe75833678b67f02b874ffc4b3580ab91` |
| Spherify Test.max | 630784 | 512 × 268 | `43f61b0fdefb6cfb3a5b682363ab032a6cb7b8c840d08d305563309483610023` |

Both sources have CFB v4 root `Scene`, `ClassData`, `ClassDirectory3` and
`\u0005SummaryInformation` streams. The saved image is SummaryInformation PID 17,
`VT_CF`, clipboard format `CF_METAFILEPICT` (3). Its legacy mapping header specifies
`MM_TEXT` (1) and the image dimensions. A standard WMF header addresses one
`META_STRETCHDIB` record with `SRCCOPY`, zero offsets and equal source/destination
sizes, followed by `META_EOF`. Its embedded bitmap has a 40-byte header,
positive-height, 24-bit `BI_RGB` pixels and no color table.

Independent inspection used Python `olefile` to read the exact named property
stream, Python `struct` to resolve its property table and WMF records, and Pillow
to decode the explicitly addressed DIB's BGR bottom-up rows. The recorded RGBA
hashes include opaque alpha for every pixel. Rust public-API tests compare these
literal hashes and verify that source bytes remain unchanged. Committed corpus
snapshots additionally qualify fitting these saved viewport images to a 256 × 256
transparent canvas.

The original upstream application release is not explicitly declared by these
fixtures. No release range, arbitrary WMF renderer or native MAX geometry support
is claimed. Authored tests qualify additional CFB v3/mini-stream framing, image
row padding/orientation and malformed carriers. Non-sector-aligned MAX files
and other thumbnail carriers remain unsupported; an upstream `Bunny Demo.max`
was observed with trailing non-CFB bytes and was not retained as a positive case.
