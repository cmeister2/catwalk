# Curved fixed-node VTK fixtures

## Unchanged Kitware dataset

`quadraticTetra01.vtu` is the original Kitware VTK test dataset, retained
byte for byte. Its content-addressed [SHA-512 pointer at commit
4b0325a981d1f5cfad9d4da4909a8230881b2bf4](https://github.com/Kitware/VTK/blob/4b0325a981d1f5cfad9d4da4909a8230881b2bf4/Testing/Data/quadraticTetra01.vtu.sha512)
resolves to the [original external data object](https://www.vtk.org/files/ExternalData/SHA512/e92542fca9fdd19dd9b41f0750baf80f003e200cbe4960d51fdbddb275448408af42007586c0c74fb70f419e5c64dbb7c935005778611ac80c490be587a0cec8).
The 1,471-byte file has SHA-256
`6bebfc84f0c342a867e6c50f849aa532f075d99b6ea980c0140966fb96d520ad`.
It contains one ten-node quadratic tetrahedron and two six-node quadratic
surface triangles, with 22 total points. The tetrahedron's first midside
node is `(0.5,0,-0.2)`, visibly away from its corner-only straight edge.
The original scalar simulation arrays are retained; they are not preview
material colors.

The separately retained [VTK-Copyright.txt](VTK-Copyright.txt) is the exact
[BSD-3-Clause notice from the same pinned revision](https://github.com/Kitware/VTK/blob/4b0325a981d1f5cfad9d4da4909a8230881b2bf4/Copyright.txt),
including its original line endings. It is 1,761 bytes with SHA-256
`11232448be82e0ea2c2c66219c2e36389f42249894070ee10c549ff182fc08b6`.
The model and its separate terms are not relicensed under Catwalk's MIT license.

At the fixed six-subdivision face policy, this original produces 216 triangles:
144 from the tetrahedron boundary and 36 from each surface triangle. Independent
checks use its literal original node coordinates and the VTK shape functions,
rather than a corner-only tetrahedron approximation. The original ASCII file
declares Float32, while the current ASCII reader retains its decimal values in
Float64. Separate independent geometry checks also evaluate the Float32-rounded
native coordinates; those small precision differences are not treated as new
shape features.
This qualifies one genuine upstream test dataset, rather than every scientific
exporter or arbitrary-order finite element.

## Sixteen authored cell images

`examples/fixtures/vtk_quadratic.rs` writes deterministic MIT-licensed controls
using literal role-ordered physical nodes. Separate image cases expose every
supported fixed-node cell type. The extra edge nodes bow away from the corner
mesh; available face-center nodes also change the visible surface. Cubic line
interior nodes have distinct positive and negative Z positions. Volume cells
have asymmetric corners, edge mids displaced radially by a factor of 1.4 about
the mean corner center, and face centers by a factor of 2.0. Interior volume
nodes remain separate and do not enter the boundary-only preview.

| VTK ID | Input | Nodes | Fixed preview output |
| --- | --- | --- | --- |
| 21 | `vtu-quadratic-edge.vtu` | 3 | 12 segments |
| 22 | `vtu-quadratic-triangle.vtu` | 6 | 36 triangles |
| 23 | `vtu-quadratic-quad.vtu` | 8 | 72 triangles |
| 24 | `vtu-quadratic-tetra.vtu` | 10 | 144 triangles |
| 25 | `vtu-quadratic-hexahedron.vtu` | 20 | 432 triangles |
| 26 | `vtu-quadratic-wedge.vtu` | 15 | 288 triangles |
| 27 | `vtu-quadratic-pyramid.vtu` | 13 | 216 triangles |
| 28 | `vtu-biquadratic-quad.vtu` | 9 | 72 triangles |
| 29 | `vtu-triquadratic-hexahedron.vtu` | 27 | 432 triangles |
| 30 | `vtu-quadratic-linear-quad.vtu` | 6 | 12 triangles |
| 31 | `vtu-quadratic-linear-wedge.vtu` | 12 | 108 triangles |
| 32 | `vtu-biquadratic-quadratic-wedge.vtu` | 18 | 288 triangles |
| 33 | `vtu-biquadratic-quadratic-hexahedron.vtu` | 24 | 432 triangles |
| 34 | `vtu-biquadratic-triangle.vtu` | 7 | 36 triangles |
| 35 | `vtu-cubic-line.vtu` | 4 | 12 segments |
| 37 | `vtu-triquadratic-pyramid.vtu` | 19 | 216 triangles |

Node roles follow the primary [VTK cell identifiers](https://github.com/Kitware/VTK/blob/4b0325a981d1f5cfad9d4da4909a8230881b2bf4/Common/DataModel/vtkCellType.h)
and corresponding `Common/DataModel` cell definitions. Hexahedron face-center
order is X0, X1, Y0, Y1, followed by Z0/Z1 for the 27-node cell. The 18-node
wedge's three centers belong to its lateral faces. The 19-node pyramid retains
one base center, four side centers and a distinct interior point. Four-node
cubic line role order is endpoint 0, endpoint 1, first interior node, second
interior node; physical traversal is 0 → 2 → 3 → 1.

All quadratic triangle and quadrilateral surface edges use six subdivisions.
Quadratic-linear quadrilaterals use six along their quadratic direction and one
along the linear direction. Both line families use up to twelve segments, omitting coincident sampled endpoints. This is a
fixed thumbnail sampling policy, rather than adaptive error-controlled finite
element visualization. Output counts are checked after matching shared faces are
removed, with an aggregate five-million triangle/line/point ceiling and
conservative 512 MiB topology/temporary/output allocation accounting. Full scene geometry is checked independently, including
primitive orientation and use of curved extra nodes, and every image case has
a PNG snapshot comparison.

## Five full-family encodings

The following inputs contain the same sixteen disconnected cells in the same
order, using Float64 coordinates throughout. The 201 points are placed on a
four-column grid, with offsets `(8*(index%4),8*(index/4),0)`. Expected full-family
output is 2,784 triangles and 24 line segments. All five encodings compare the
same geometry and full PNG output.

| Input | Encoding |
| --- | --- |
| `vtk-quadratic-family-ascii.vtk` | Classic legacy ASCII |
| `vtk-quadratic-family-binary.vtk` | Classic legacy big-endian binary |
| `vtu-quadratic-family-binary.vtu` | XML inline binary, little-endian UInt64 headers |
| `vtu-quadratic-family-appended-zlib.vtu` | XML raw appended zlib arrays, little-endian UInt32 block headers |
| `vtkhdf-quadratic-family.vtkhdf` | VTKHDF static UnstructuredGrid, one partition |

Regeneration uses the checked-in Rust generator, existing Rust array/container
writers and the retained original bytes. It needs no native VTK runtime,
Python generator or network. Regeneration leaves snapshot baselines unchanged.
Variable-node quadratic polygons (type 36), arbitrary-order Lagrange/Bezier
cells and unsupported higher-order families remain outside this qualification.
