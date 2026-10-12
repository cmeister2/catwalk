# VTK unstructured line cells

## Original F3D scientific dataset

`pdiag.vtu` is the unchanged
[F3D persistence-diagram test asset](https://github.com/f3d-app/f3d/blob/09ba978ad03eb40466548f340c0db7c18843a354/testing/data/pdiag.vtu)
at commit `09ba978ad03eb40466548f340c0db7c18843a354`. It contains 6,992
Float32 points and 3,497 type-3 line cells in a base64-appended, little-endian
UInt32/zlib XML UnstructuredGrid. The complete original input is retained,
including its simulation attributes and field metadata. Those attributes are
not RGB materials and are not interpreted as a scalar color map.

The exact upstream [F3D-LICENSE.md](F3D-LICENSE.md) and
[F3D-DATA_LICENSES.md](F3D-DATA_LICENSES.md) are retained. The mapping explicitly
licenses all unlisted datasets under F3D's BSD-3-Clause terms, and `pdiag.vtu`
has no exception. Retain these original copyright notices, redistribution
conditions and disclaimer when redistributing the input. Catwalk's MIT license
applies separately to its code and authored fixtures.

| Original resource | Pinned upstream path | SHA-256 |
| --- | --- | --- |
| `pdiag.vtu` | `testing/data/pdiag.vtu` | `61152cf6369593d3cb4d7213496cd0fc0da298e37de156f93f965e8585b2e342` |
| `F3D-LICENSE.md` | `LICENSE.md` | `42e90e43576bfcc8945239179c49141c387c407a3a3c3696789d9f061cc4afb4` |
| `F3D-DATA_LICENSES.md` | `testing/data/DATA_LICENSES.md` | `4229d1453e125ace50ca1825d420340bbc6b2ee4f2e6ffabde8709052534e12a` |

Independent topology expectations are 3,497 ordered line segments, including
282 coincident-endpoint segments, with no triangles or point cells. The total
segment length is `141.33120573828802`. Tests check the original source hash,
declared arrays, independently decoded endpoint associations, bounds and
length. The thumbnail snapshot checks the resulting fixed-view line geometry.
This qualifies a genuine scientific line dataset, rather than only recognizing
the `.vtu` extension.

## Four authored equivalent encodings

`examples/fixtures/vtk_lines.rs` generates four deterministic MIT-licensed
UnstructuredGrid inputs with the same literal coordinates and topology. It uses
Rust only, including the published `hdf5-writer` API for VTKHDF; regeneration
requires no VTK installation, external writer or network access.

| Point | X | Y | Z |
| --- | ---: | ---: | ---: |
| 0 | -2 | -1 | 0.5 |
| 1 | -1 | 2 | 1 |
| 2 | 2 | -2 | 3 |
| 3 | 0 | 0 | 0 |
| 4 | 3 | 1 | 2 |
| 5 | 4 | 3 | -1 |
| 6 | 1 | 5 | 1 |
| 7 | -1 | 1 | 2 |
| 8 | -2 | 3 | 2 |

The five cells are type-1 vertex `[0]`, type-2 polyvertex `[1,2]`, type-3 line
`[3,4]`, type-4 polyline `[4,5,6]`, and type-5 triangle `[3,7,8]`.
Expected preview geometry is three isolated points, three ordered segments and
one triangle. Segment lengths are `sqrt(14)`, `sqrt(14)` and `sqrt(17)`;
triangle area is `sqrt(21)/2`. Bounds are `[-2,-2,-1]` to `[4,5,3]`. No colors
are authored, so all four encodings should produce identical PNG pixels.

- `vtk-unstructured-lines-ascii.vtk`: legacy version-3 ASCII, with classic
  count-prefixed cell lists.
- `vtk-unstructured-lines-binary-v5.vtk`: standard big-endian binary Float64
  positions and version-5 Int64 offsets/connectivity; cell types use Int32.
- `vtu-unstructured-lines-ascii.vtu`: ASCII XML arrays with explicit cell kinds
  and connectivity offsets.
- `vtkhdf-unstructured-lines.vtkhdf`: static version-2 single-partition HDF
  UnstructuredGrid, with separate typed positions/connectivity/offset/type
  datasets. VTKHDF already supported line cells; this fixture checks equivalent
  geometry across the readers.

Line cells require exactly two vertices; polyline cells require at least two
ordered vertices. Repeated indices and explicit closing endpoints retain their
authored segments. Higher-order cells and scientific scalar coloring remain
outside this feature. Fixture generation does not update PNG baselines.
