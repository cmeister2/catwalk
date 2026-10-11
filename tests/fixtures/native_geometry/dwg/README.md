# Native DWG geometry investigation

The five `line-*.dwg` inputs are unchanged public LibreDWG test drawings, pinned
to commit `a3d51f13f7afc143c2adb87d4b0e858e24cdac95`. Exact source links and hashes
are in [provenance.json](provenance.json). Their GPL-3.0-or-later license is
retained as [LICENSE-LibreDWG.txt](LICENSE-LibreDWG.txt), separately from
Catwalk's original MIT code. The decoder is the MPL-2.0 Rust crate
[`acadrust` 0.6.3](https://github.com/hakanaktt/acadrust).

`line-2018.dwg` genuinely has no supported drawing image; it was not edited to
remove one. Catwalk's normal thumbnail path renders its one native line. All
five inputs are also rendered with `--render-geometry`, which never reads the
saved image. Tests check exact endpoints against the independent original
AutoCAD property dump `line-2018-reference.txt` and the full-precision 2007 DXF
`line-2007-reference.dxf`, rather than a writer/reader round trip.

The bridge decodes DWG records to a native document in memory and serializes
the document to bounded DXF bytes, then calls Catwalk's static DXF geometry
reader. Every stage runs in Rust; no converter executable or native geometry
library is used. Strict decoding rejects incomplete streams, skipped records,
recovered errors, unsupported model entities and missing geometry. The bridge
accepts the DXF reader's drawing subset, but genuine qualification here covers
these line drawings only.

Broader external probing rendered five generations of lines and a 135-segment
AutoCAD 2000 sample. Generic R13 through 2018 examples fail explicitly on
`XLINE`; an R11 example fails on `SHAPE`; a 2004 surface drawing fails a mesh
count check. These are unresolved compatibility cases, not successful models.
ACIS `3DSOLID`, `BODY` and `REGION` tessellation is not implemented in this
bridge. The decoder can expose SAT/SAB records; converting those records to
validated trimmed surfaces is the next substantial step for solid models.

The wrapper limits source and intermediate drawing bytes to 512 MiB and
rejects documents over 500,000 decoded records. Those checks do not establish
a complete memory/work bound inside the third-party DWG decoder; further
malformed-input qualification and section-decompression limits are needed
before claiming a hardened general-purpose DWG importer.

```sh
cargo test --locked --test dwg_geometry
cargo run --locked -- tests/fixtures/native_geometry/dwg/line-2018.dwg \
  --render-geometry --transparent --size 256 --output line.png
```
