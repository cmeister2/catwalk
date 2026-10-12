# Thumbnail format coverage

Catwalk provides Rust thumbnail readers for the model, image and application-document formats listed here. Each importer supports the encodings and geometry described below. **A listed extension does not imply support for every encoding, entity or scene feature in that format.**

The summary table groups format families and extension aliases. Each format section describes supported features, limits and shared renderer or codec policies.

Fresh geometry readers support Blender, LightBurn, a DWG drawing subset, original stored Lychee models and qualified FreeCAD document shapes. Inventor `.ipt`/`.iam` and Fusion `.f3d` support saved-image extraction only; their geometry rendering is unsupported in Catwalk. Saved-image extraction does not regenerate geometry or establish compatibility with every application release.

| Format family | Extensions | Current Rust preview coverage |
| --- | --- | --- |
| [VTKLegacy](#vtk-legacy) | `.vtk` | Subset: ASCII/big-endian binary, classic/version-5 topology, polygons/lines, linear/fixed-node higher-order and structured grid boundaries. |
| [VTKXMLVTP](#vtk-polydata-xml) | `.vtp` | Subset: ASCII/binary/appended XML, zlib/LZ4; polygons, strips, vertices and polylines. |
| [VTKXMLVTU](#vtk-unstructuredgrid-xml) | `.vtu` | Subset: ASCII/binary/appended XML, zlib/LZ4; linear/fixed-node higher-order cells and closed convex polyhedra. |
| [VTKXMLVTR](#vtk-rectilineargrid-xml) | `.vtr` | Subset: ASCII/binary/appended, zlib/LZ4; single-piece rectilinear grid boundary geometry. |
| [VTKXMLVTI](#vtk-imagedata-xml) | `.vti` | Subset: scalar image data, ASCII/base64/zlib, voxel density surface. |
| [VTKXMLVTS](#vtk-structuredgrid-xml) | `.vts` | Subset: ASCII/binary/appended, zlib/LZ4; single-piece structured grid boundary geometry. |
| [VTKXMLVTM](#vtk-multiblock-xml) | `.vtm` | Subset: nested multiblock hierarchy and supported relative child datasets. |
| [VTKHDF](#vtkhdf) | `.vtkhdf` | Subset: static polygon/linear/fixed-node higher-order/convex-polyhedron meshes, structured grids, scalar images and local composite assemblies. |
| [ExodusII](#exodus-ii) | `.e`, `.ex2`, `.exo`, `.g` | Subset: CDF1/2/5 and NetCDF4 undeformed linear element-block boundary geometry. |
| [NetCDF](#netcdf) | `.nc`, `.cdf`, `.ncdf` | Subset: rectilinear 2D scalar sheets, 3D scalar density and first time sample; Exodus schema detection. |
| [PLYReader](#ply) | `.ply` | Subset: ASCII/binary polygons, standard edges and point geometry with RGB. |
| [STL](#stl) | `.stl` | Implemented: ASCII and binary triangle geometry. |
| [DICOM](#dicom) | `.dcm` | Subset: first scalar/RGB image frame with native, deflated, JPEG or RLE pixel decoding. |
| [Nrrd](#nrrd) | `.nrrd`, `.nhdr` | Subset: raw/ASCII/hex/gzip scalar density, attached or one detached payload. |
| [MetaImage](#metaimage) | `.mhd`, `.mha` | Subset: raw/ASCII/zlib scalar density, attached or one detached payload. |
| [TIFF](#tiff) | `.tif`, `.tiff` | Fitted 2D images and bounded scalar page-stack density surfaces. |
| [PNG](#png) | `.png` | Implemented: 2D image preview. |
| [JPEG](#jpeg) | `.jpg`, `.jpeg` | Implemented: 2D image preview. |
| [BMP](#bmp) | `.bmp` | Implemented: 2D image preview. |
| [TGA](#tga) | `.tga` | Implemented: 2D image preview. |
| [HDR](#hdr) | `.hdr` | Implemented: 2D image preview. |
| [WebP](#webp) | `.webp` | Implemented: 2D image preview. |
| [EXR](#exr) | `.exr` | Implemented: 2D image preview. |
| [QuakeMDL](#quake-mdl) | `.mdl` | Subset: Quake IDPO v6 triangle geometry, first frame, grouped frames. |
| [CityGML](#citygml) | `.gml` | Subset: highest feature LoD planar surfaces, holes, local XLinks and diffuse appearances. |
| [PTS](#pts) | `.pts` | Implemented: point positions and optional RGB. |
| [STEP](#step) | `.step`, `.stp`, `.stpnc`, `.p21`, `.210` | Subset: supported Part 21 B-rep shells, planar faceted solids and AP242 triangle geometry, axis-based assemblies/mappings and length contexts in Rust. |
| [IGES](#iges) | `.iges`, `.igs` | Subset: supported manifold/constructive solids and independent trimmed/bounded sheets, curved surface tessellation. |
| [BREP](#brep) | `.brep` | Subset: Open CASCADE ASCII and little-endian binary topology, supported analytic/NURBS geometry, occurrence placements and curved tessellation. |
| [XBF](#xbf) | `.xbf` | Subset: native binary XCAF static B-rep geometry, free shapes, instance placements and diffuse colors. |
| [FreeCAD](#freecad) | `.fcstd` | Subset: addressed saved PNG and native ASCII BREP document shapes with supported visibility, containers, local links and model arrays. |
| [Alembic](#alembic) | `.abc` | Subset: Ogawa/HDF5 first-sample meshes, subdivision cages, points, sampled curves, transforms, instances and RGB. |
| [OBJ](#obj) | `.obj` | Subset: static polygons/wires/points, corner RGB/normals/UVs, local MTL diffuse maps and dissolve. |
| [GLTF, GLTFDraco](#gltf) | `.gltf` | Subset: textured triangle/point/line scenes, alpha, normals, first poses, skins/morphs, GPU instances and Rust Draco/quantized decoding. |
| [GLB, GLBDraco](#glb) | `.glb` | Subset: textured triangle/point/line scenes, alpha, normals, first poses, skins/morphs, GPU instances and Rust Draco/quantized decoding. |
| [Draco](#draco) | `.drc` | Subset: sequential/Edgebreaker meshes with authored corner normals, sequential/kd-tree point clouds, positions and colors. |
| [3DS](#3ds) | `.3ds` | Subset: static triangle meshes, diffuse colors and earliest-key object hierarchy. |
| [VRMLReader](#vrml) | `.wrl`, `.vrml` | Subset: VRML97 static geometry, local typed PROTO/IS instances, DEF/USE, transforms, fixed-view billboards and colors. |
| [FBX](#fbx) | `.fbx` | Subset: ASCII/binary static polygon scenes, weighted linear skins, progressive morphs, hierarchy, pivots, units, first keys and colors. |
| [COLLADA](#collada) | `.dae` | Subset: static indexed surfaces and wires, planar polygon holes, instancing, affine transforms, units and up axes. |
| [OFF](#off) | `.off` | Subset: polygon geometry and colors. |
| [DXF](#dxf) | `.dxf` | Subset: ASCII/binary drawing faces, points, lines, sampled curves/splines, meshes and nested block instances. |
| [DirectX](#directx) | `.x` | Subset: text/binary/MSZIP mesh scenes, static frames, bind geometry and colors. |
| [3MF](#3mf) | `.3mf` | Subset: core/production multipart meshes, placements, units, colors and packaged PNG/JPEG textures. |
| [AMF](#amf) | `.amf` | Subset: XML/single-model ZIP triangle volumes, units, constant colors and qualified constellation placements. |
| [USD](#usd) | `.usd`, `.usda`, `.usdc`, `.usdz` | Subset: local composed stages, static meshes/primitives/points/curves, instances, transforms and diffuse colors. |
| [VDB](#vdb) | `.vdb` | Subset: scalar sparse 5/4/3 trees, half-float values, density/level-set surfaces and affine transforms. |
| [IFC](#ifc) | `.ifc` | Subset: Part 21 product placements, mapped/faceted/tessellated shapes, extruded/revolved profiles, primitives and bounded solid operations/openings. |
| [Splat](#splat) | `.splat` | Subset: positions and RGB rendered as points. |
| [SPZ](#spz) | `.spz` | Subset: gzip versions 1–3 and Zstandard version 4, centers and colors rendered as points. |
| [LAS](#las) | `.las`, `.laz` | Subset: LAS point records, Rust LAZ decompression and RGB. |
| [BinaryPointFile](#bpf) | `.bpf` | Subset: versions 1–3, supported layouts, zlib, homogeneous transforms and RGB. |
| [TerraScanBIN](#terrascan-bin) | `.bin` | Subset: versions 20010712/20020715, scaled coordinates and optional RGB. |
| [OptechCSD](#optech-csd) | `.csd` | Subset: 69-byte records, range echoes, platform attitude and boresight geometry. |
| [PCD](#pcd) | `.pcd` | Subset: ASCII, binary and LZF-compressed points with RGB. |
| [LeicaCyclonePTX](#ptx) | `.ptx` | Subset: scans, positions, RGB and scan transforms. |
| [QFIT](#qfit) | `.qi` | Subset: supported 10/12/14-word records and endianness, longitude/latitude/elevation points. |
| [SBET](#sbet) | `.sbet` | Subset: little-endian 17-double trajectory records rendered as geographic points. |
| [SLPK](#slpk) | `.slpk` | Subset: I3S point-cloud packages, hierarchy and Rust LEPCC XYZ/RGB decoding. |
| [Lychee](#lychee) | `.lys` | Subset: addressed saved PNG and original indexed source models with scene placements in qualified flat Mango containers; print preparation is excluded from fresh geometry. |

## Shared preview policies

The renderer uses an automatically fitted orthographic view, a depth buffer, simple lighting, colored triangles, points and line segments. Optional triangle appearances support base-color texture sampling, interpolated linear RGBA and normals, unlit shading, sidedness, alpha masks and per-sample depth-sorted transparency. glTF/GLB, textured 3MF and constant AMF colors populate these appearances; each reader retains its stated material subset. LightBurn drawings use an XY view. Static scene importers select the authored pose or earliest supported transform sample. Animation playback, authored camera views, scene lighting, full physically based materials, volume transfer functions and Gaussian opacity/covariance are outside the current preview policy. Supporting a first static view permits omission of interactive viewer features, but does not remove the need to parse each model format correctly.

All current importers ignore non-geometric fields that do not affect their supported static geometry. Exact encoding restrictions are enforced by each reader. New variants need representative fixtures and thumbnail checks before this matrix changes to implemented.

### Raster-image rendering

Raster images are decoded by the enabled Rust codecs, fitted to the requested canvas with aspect ratio preserved, resized with Lanczos filtering and converted to RGBA. Transparent output preserves supported source alpha. Decoding is limited to 256 MiB; primary files remain bounded by the shared 512 MiB input limit. These readers produce 2D previews rather than reconstructed model geometry.

### Scientific arrays and cell limits

XML mesh arrays accept ASCII, inline base64, base64 appended and raw appended bytes, either byte order, UInt32/UInt64 headers, zlib and LZ4 compression. Binary lengths, block headers, decompressed sizes, connectivity offsets and indices are checked, with an aggregate 512 MiB decoded-array limit. ASCII Float32 positions currently retain the decimal Float64 parse, while binary coordinates follow the encoded scalar width; cross-encoding exact-output fixtures therefore use Float64.

Scalar color maps, variable-node quadratic polygons (type 36), unsupported higher-order families including arbitrary-order Lagrange/Bezier cells, legacy polyhedron face streams, platform-dependent legacy `long` words and XML LZMA compression remain unsupported; mixed unsupported cell types fail explicitly. Thirteen regression tests include pixel-identical results across 36 binary XML encoding combinations and malformed payload checks. Thirteen independent fixtures from [Kitware VTK](https://github.com/Kitware/VTK/tree/master/Testing/Data) and [vtkio](https://github.com/elrnv/vtkio/tree/master/assets) verify legacy binary grids, inline/appended arrays, compression and polyhedra against the primary [VTK format specification](https://docs.vtk.org/en/latest/vtk_file_formats/index.html) and [VTK cell identifiers](https://github.com/Kitware/VTK/blob/master/Common/DataModel/vtkCellType.h).

Fixed-node higher-order cell types 21–35 and 37 use their authored extra nodes and standard interpolation functions rather than reducing them to corner-only polygons. The supported family includes quadratic edges, triangles, quadrilaterals, tetrahedra, hexahedra, wedges and pyramids; biquadratic triangles/quadrilaterals; quadratic-linear quadrilaterals/wedges; biquadratic-quadratic wedges/hexahedra; triquadratic hexahedra/pyramids; and cubic lines. Volume cells contribute their exposed boundary faces, not a volumetric scalar visualization. Triangle and quadrilateral faces use six subdivisions per quadratic edge; quadratic-linear quadrilaterals use six along the quadratic direction and one along the linear direction. Both edge families use up to twelve line segments, omitting coincident sampled endpoints. This fixed sampling policy is for preview thumbnails, without an adaptive geometric error guarantee. Boundary output counts are computed after matching shared faces are removed. The shared dispatcher caps combined visible triangles, lines and points at five million; conservative 512 MiB accounting preflights topology, curved face descriptors, temporary samples and emitted geometry. The shared grid limit also caps positions, cells and referenced connectivity indices at five million each; VTKHDF retains its stricter one-million point/cell limits.

A separate image case for each of the sixteen fixed-node types, the unchanged Kitware `quadraticTetra01.vtu`, and five full-family Float64 legacy/XML/VTKHDF encodings have [fixture details and retained licensing](../tests/fixtures/formats/vtk_quadratic/README.md). Independent regressions check source identity, literal curved geometry and primitive orientation; full-image comparisons qualify the static output. The original contributes one curved tetrahedron and two curved surface triangles. This covers the documented fixed-node subset rather than all higher-order VTK elements.

### Scalar-volume rendering

Scalar volume previews extract exposed faces of actual foreground voxels. Values are max-pooled to at most 32 cells per axis so a small bright feature survives reduction. The threshold is halfway through the finite scalar range, using zero as the lower baseline when the range crosses zero. Uniform nonzero data fills the extent; uniform zero data has no foreground. Coordinates, spacing, origin, direction and reflected axes are preserved. This produces a bounded density surface, including interior cavity walls, rather than an empty volume's bounding box. It is a fixed preview policy; interactive volume transfer functions are unsupported. Scalar payloads are capped at 16,777,216 samples and 128 MiB decoded data.

### HDF5 and NetCDF backends

VTKHDF, Exodus and NetCDF use audited `hdf5-reader`/`netcdf-reader` 0.9.1 from the pure Rust [netcdf-rust project](https://github.com/roteiro-gis/netcdf-rust). Numeric endianness, signed/unsigned integer IDs and supported compression are decoded without native HDF5/NetCDF libraries. Deflate, shuffle, Fletcher32 and the enabled Rust LZ4 codec are available; unknown filters, virtual/external datasets and external file resolution remain unsupported. Catwalk owns bounded loaders for variable-width filtered-chunk indexes, constructing upstream chunk and array objects while retaining upstream metadata, filters and numeric decoding. Affected layouts are checked before upstream data traversal, and allocated entry/coordinate storage is capped at 512 MiB. Regressions exercise deflate plus Fletcher32 across fixed-array, extensible-array and v2 B-tree indexes. Independent metadata cases follow the [HDF Group's format](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t4.html), checking direct and secondary extensible-array blocks, sparse physical pages, MSB-first initialization flags, metadata checksums, depth-two tree count fields, finite maximum-dimension strides and nonleading unlimited axes. Typed and sliced tests retain signed/endian values, padded edges, fill values, skipped filters, NetCDF masking before unpacking, and Alembic geometry/string rendering.

### Survey-point conventions

Survey point readers follow the corresponding [PDAL readers](https://github.com/PDAL/PDAL/tree/master/io).

QFIT/SBET/CSD previews retain longitude/latitude in degrees and elevation in stored linear units, as in these reader conventions; they do not provide metric CRS reprojection. Unsupported record versions/compression and malformed lengths fail. Point counts are bounded and deterministically thinned to at most one million.

### IGES and BREP tessellation

IGES and BREP use the pure Rust [ogeom-rs](https://github.com/gilbertorconde/ogeom-rs) 0.9.16 kernel.

Topology, placed vertices and nested geometry records are validated. Surface tessellation first requests 0.3% of actual curve/surface bounds with a 0.2-radian angular limit; bounded retries request 0.6%/0.3 radians and 1%/0.4 radians, with a 4096-segment cap. The numerical chord floor is 0.00001 mm, independent of authored coincidence precision; shapes too small for that floor to remain within 1% of their diameter fail explicitly. Every face must produce triangles and meet the requested deflection. Limits cap input at 16 MiB, table/topology counts and spline poles/expanded knots at 100,000, attached placed pcurves at 100,000 and their association work at ten million comparisons, topology/trimmed/offset nesting and location powers at 64, each body at 4096 faces and output at five million triangles/segments. Colors and drafting views are omitted. Generated box/cylinder/sphere and trimmed-sheet tests check surface area and placement; cache/legacy-token, mixed-wire and logical-group regressions exercise preserved geometry. A small sphere with coarse authored precision verifies geometric sag at the requested preview resolution, and a smaller box exercises the explicit numerical-scale error.

## VTK legacy

**Extensions:** `.vtk`.

**Coverage:** Subset: ASCII/big-endian binary, classic/version-5 topology, polygons/lines, linear/fixed-node higher-order and structured grid boundaries.

The legacy VTK reader accepts ASCII and standard big-endian binary geometry, classic cell lists and version-5 offsets/connectivity sections. POLYDATA supports polygons, triangle strips, vertices and polylines; supported linear and fixed-node higher-order UNSTRUCTURED_GRID cells and structured/rectilinear/structured-point boundaries are rendered.

Grid boundaries are extracted for tetrahedra, voxels, hexahedra, wedges and pyramids, removing shared faces. Vertex/polyvertex, line/polyline, triangle, triangle-strip, polygon, pixel and quadrilateral cells are also accepted. Type-3 lines require exactly two vertices, and type-4 polylines require at least two; ordered adjacent endpoint pairs become segments, including repeated or explicitly closing endpoints. Polygons use concave-aware triangulation. Legacy polyhedron face streams remain unsupported.

[Four equivalent authored mixed-cell grids](../tests/fixtures/formats/vtk_lines/README.md) check line, polyline, point and surface geometry across classic ASCII, version-5 big-endian offsets/connectivity, XML and the already-supported VTKHDF line path. Independent tests compare literal coordinates, cell associations, bounds, line lengths, triangle area and complete rendered pixels. Shared point/connectivity/output caps and decoded-array limits apply to line cells as well.

See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for validation, shared limits and reference checks.

## VTK PolyData XML

**Extensions:** `.vtp`.

**Coverage:** Subset: ASCII/binary/appended XML, zlib/LZ4; polygons, strips, vertices and polylines.

PolyData XML preserves polygons, triangle strips, vertices and polylines. Polygons use concave-aware triangulation. See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for supported encodings, compression, validation and reference checks.

## VTK UnstructuredGrid XML

**Extensions:** `.vtu`.

**Coverage:** Subset: ASCII/binary/appended XML, zlib/LZ4; linear/fixed-node higher-order cells and closed convex polyhedra.

Grid boundaries are extracted for tetrahedra, voxels, hexahedra, wedges and pyramids, removing shared faces. XML type-42 convex polyhedra require closed face topology. Vertex/polyvertex, line/polyline, triangle, triangle-strip, polygon, pixel and quadrilateral cells are also accepted. Type-3 lines require two vertices, and type-4 polylines expand adjacent endpoint pairs in their stored order; repeated/closing endpoints are retained. Polygons use concave-aware triangulation.

The unchanged [F3D `pdiag.vtu` dataset](../tests/fixtures/formats/vtk_lines/README.md) retains its original BSD-3-Clause notice and license mapping. It qualifies 3,497 ordered line cells, including 282 coincident-endpoint segments, from appended zlib-compressed arrays. Independent geometry checks preserve every endpoint association, bounds and total segment length `141.33120573828802`; its snapshot uses the stored geometry and fallback color. Simulation/scalar attributes are not rendered as a color map. Higher-order cells outside the fixed-node families listed above remain unsupported.

See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for supported encodings, compression and validation.

## VTK RectilinearGrid XML

**Extensions:** `.vtr`.

**Coverage:** Subset: ASCII/binary/appended, zlib/LZ4; single-piece rectilinear grid boundary geometry.

VTR accepts one piece covering the complete extent, with monotonic rectilinear coordinate axes. Actual boundary cell faces retain every boundary grid point; two-dimensional sheets and single points are also accepted. Multiple/partial pieces, one-dimensional structured lines, nonzero ghost/blanking flags and time-dependent geometry arrays fail explicitly.

See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for XML encoding and validation.

## VTK ImageData XML

**Extensions:** `.vti`.

**Coverage:** Subset: scalar image data, ASCII/base64/zlib, voxel density surface.

VTI supports ASCII, inline base64 and appended base64 arrays with UInt32/UInt64 length headers, endianness and multiblock zlib compression. One complete piece, active scalar PointData/CellData and direction matrices are supported. Raw appended XML, LZ4/LZMA compression, partial/multiple pieces, nonzero ghosts and vector/time arrays fail.

See [scalar-volume rendering](#scalar-volume-rendering) for density extraction and shared limits. Eleven volume unit tests and a five-alias CLI comparison check actual nonuniform density, compression, detached payloads, reflected directions and malformed inputs.

## VTK StructuredGrid XML

**Extensions:** `.vts`.

**Coverage:** Subset: ASCII/binary/appended, zlib/LZ4; single-piece structured grid boundary geometry.

VTS accepts one piece covering the complete extent, with explicit structured point coordinates. Actual boundary cell faces retain every boundary grid point, including deformed surfaces; two-dimensional sheets and single points are also accepted. Multiple/partial pieces, one-dimensional structured lines, nonzero ghost/blanking flags and time-dependent geometry arrays fail explicitly.

See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for XML encoding and validation.

## VTK MultiBlock XML

**Extensions:** `.vtm`.

**Coverage:** Subset: nested multiblock hierarchy and supported relative child datasets.

VTM traverses inline blocks and nested references to `.vtk`, `.vtp`, `.vtu`, `.vtr`, `.vts`, `.vti` and `.vtm`, preserving their stored coordinates. References must resolve inside each composite file's directory; cycles, unsupported child formats and excessive nesting/references fail. Composite input is capped at 512 MiB in aggregate, 256 file loads, 32 hierarchy levels and five million triangles/points/lines. Null dataset slots are skipped. The structure follows the [VTK XML specification](https://docs.vtk.org/en/latest/vtk_file_formats/vtkxml_file_format.html).

## VTKHDF

**Extensions:** `.vtkhdf`.

**Coverage:** Subset: static polygon/linear/fixed-node higher-order/convex-polyhedron meshes, structured grids, scalar images and local composite assemblies.

VTKHDF accepts supported static version-1/2 PolyData, UnstructuredGrid, ImageData, RectilinearGrid and StructuredGrid schemas, and local MultiBlockDataSet/PartitionedDataSetCollection assemblies. Polygon/strip/polyvertex/polyline geometry and supported linear/fixed-node higher-order cells are rendered; convex type-42 polyhedra require closed, consistent face topology. Partition-local indices and offsets are validated; matching shared interior faces are removed within each partition, including supported mixed cell kinds. Scalar ImageData uses the density policy above, including cell-centered arrays and direction matrices. Local assembly links and repeated blocks are followed with depth/cycle/count bounds. Temporal Steps, AMR/HyperTreeGrid, nonconvex polyhedra, unsupported higher-order cells, nonzero ghosts and other schemas fail. Thirteen container tests and four independent [Kitware VTKHDF fixtures](https://github.com/Kitware/VTK/tree/master/Testing/Data/vtkHDF) cover real polyhedra, composite spheres, PolyData and Mandelbrot image density. The schema follows the [VTKHDF specification](https://docs.vtk.org/en/latest/vtk_file_formats/vtkhdf_file_format/vtkhdf_specifications.html).

See [HDF5 and NetCDF backends](#hdf5-and-netcdf-backends) for codecs, validation and the local filtered-chunk fix.

## Exodus II

**Extensions:** `.e`, `.ex2`, `.exo`, `.g`.

**Coverage:** Subset: CDF1/2/5 and NetCDF4 undeformed linear element-block boundary geometry.

Exodus accepts CDF1/2/5 and NetCDF4, packed or separate coordinates, active element blocks and one-based integer connectivity for supported linear HEX8, TETRA4, WEDGE6, PYRAMID5, triangle/shell, quadrilateral/shell, bar/beam/truss and point elements. The undeformed mesh boundary is extracted jointly across blocks. Higher-order/polyhedral elements, Nemesis multi-file aggregation and result/deformation interpretation remain unsupported. Two independent [SEACAS Exodus fixtures](https://github.com/sandialabs/seacas) also render.

See [HDF5 and NetCDF backends](#hdf5-and-netcdf-backends) for codecs, validation and the local filtered-chunk fix.

## NetCDF

**Extensions:** `.nc`, `.cdf`, `.ncdf`.

**Coverage:** Subset: rectilinear 2D scalar sheets, 3D scalar density and first time sample; Exodus schema detection.

Generic NetCDF recognizes an Exodus schema when present; otherwise it selects the first supported numeric 2D/3D spatial variable and the first recognized time sample. Dimension coordinate variables supply rectilinear axes, or array indices are used when none exist. Packed scale/offset and fill/missing values are applied, and coordinate bounds/grid-mapping/ancillary variables are excluded from candidate data. Two-dimensional arrays become flat scalar-colored sheets with missing-data holes; three-dimensional arrays produce density surfaces with nonuniform/descending coordinates retained. CF UGRID, multidimensional curvilinear coordinates, unsupported spatial ranks and CRS reprojection remain unsupported. The [Unidata attribute conventions](https://docs.unidata.ucar.edu/netcdf-c/current/attribute_conventions.html) guide packing and masking. An eight-alias headless CLI test checks identical cube geometry across VTKHDF/Exodus and nonuniform compressed density across NetCDF aliases.

See [HDF5 and NetCDF backends](#hdf5-and-netcdf-backends) for codecs, validation and the local filtered-chunk fix.

## PLY

**Extensions:** `.ply`.

**Coverage:** Subset: ASCII/binary polygons, standard edges and point geometry with RGB.

PLY accepts ASCII, little-endian binary and big-endian binary arrays, standard polygon indices and the `edge` records described in the [original PLY specification](https://gamma-web.iacs.umd.edu/POWERPLANT/papers/ply.pdf). Edges require integer `vertex1`/`vertex2` references and preserve optional RGB; missing edge colors use available endpoint colors. Edges may precede vertices, and mesh faces and edges render together. Wire-only inputs retain their edges without adding all vertices as points. Additional PLY properties can be skipped, but Gaussian fields are not evaluated.

## STL

**Extensions:** `.stl`.

**Coverage:** Implemented: ASCII and binary triangle geometry.

ASCII and binary STL triangle geometry is decoded directly in Rust. Binary files may begin with a `solid` header; malformed or incomplete facets fail explicitly. The preview uses the shared fixed camera and surface renderer.

## DICOM

**Extensions:** `.dcm`.

**Coverage:** Subset: first scalar/RGB image frame with native, deflated, JPEG or RLE pixel decoding.

DICOM uses audited Rust [DICOM-rs](https://github.com/Enet4/dicom-rs) features plus bounded first-frame RLE handling. Native/deflated datasets, the enabled Rust JPEG codec and supported RLE scalar/RGB pixels produce a 2D preview with the library's modality/VOI conversion. Allocation is restricted to 8/16 bits and at most 16 million samples. Unsupported codecs such as JPEG-LS/JPEG2000, other pixel layouts, multi-file study assembly and 3D volume interpretation remain outside this subset. Multi-frame input selects frame zero. An independent CT sample and native/RLE test images render.

## NRRD

**Extensions:** `.nrrd`, `.nhdr`.

**Coverage:** Subset: raw/ASCII/hex/gzip scalar density, attached or one detached payload.

NRRD versions 1–5 support raw, ASCII, hex and gzip data, attached or in one relative detached file, with spatial directions/origin/spacings and supported byte/line skips. Bzip2, file lists/patterns and nonspatial axes fail.

No unit or world-coordinate-system conversion is performed. The layout follows the primary [NRRD specification](https://teem.sourceforge.net/nrrd/format.html).

See [scalar-volume rendering](#scalar-volume-rendering) for density extraction and shared limits. Eleven volume unit tests and a five-alias CLI comparison check actual nonuniform density, compression, detached payloads, reflected directions and malformed inputs.

## MetaImage

**Extensions:** `.mhd`, `.mha`.

**Coverage:** Subset: raw/ASCII/zlib scalar density, attached or one detached payload.

MetaImage supports 2D/3D scalar fixed-width numeric elements, raw/ASCII/zlib, attached or one detached file, supported header skips, spacing and column-major TransformMatrix directions. Platform-dependent `MET_LONG`/`MET_ULONG`, multiple channels and file lists/patterns fail.

No unit or world-coordinate-system conversion is performed. The layout follows the primary [MetaIO specification](https://insightsoftwareconsortium.github.io/ITKWikiArchive/Wiki/ITK/MetaIO/Documentation/).

See [scalar-volume rendering](#scalar-volume-rendering) for density extraction and shared limits. Eleven volume unit tests and a five-alias CLI comparison check actual nonuniform density, compression, detached payloads, reflected directions and malformed inputs.

## TIFF

**Extensions:** `.tif`, `.tiff`.

**Coverage:** Fitted 2D images and bounded scalar page-stack density surfaces.

Single-page TIFF produces an aspect-preserving 2D image preview. Homogeneous grayscale stacks in classic TIFF or BigTIFF produce a fitted 3D density surface using the same threshold and bounded pooling policy as the scientific volume readers. Automatic thumbnails use every selected addressed page; `--render-geometry` requires a supported multipage scalar stack.

Volume samples support unsigned/signed 8-, 16-, 32- and 64-bit integers and IEEE float16/32/64. WhiteIsZero supports unsigned integers and float32/64; signed and float16 WhiteIsZero volumes fail explicitly. Qualified volume compression is uncompressed, LZW, Deflate or PackBits. JPEG-compressed volumes remain unsupported. These scalar-volume restrictions do not replace the existing single-page raster codecs.

The reader traverses the actual linked image directories in order. When explicit `NewSubfileType=0` full-resolution pages exist, it selects those pages; otherwise ordinary/page-tagged grayscale directories form the stack. Reduced-image or mask-only chains fail. Dimensions, scalar representation, photometric interpretation, orientation and spacing must agree across selected pages. A heterogeneous image series fails rather than silently rendering its first frame. An isolated full-resolution page behind a reduced first directory is currently unsupported.

TIFF orientations 1–8 map stored columns and rows into a Cartesian XY frame, including reflected and transposed axes. Rational inch/centimetre resolution determines XY spacing in millimetres. Absent/unitless resolution uses unit spacing; Z spacing uses stored X spacing because these ordinary page chains contain no qualified slice-spacing field. This placement policy is explicit, rather than inferred from page numbers or image-description text. WhiteIsZero follows the codec's normalization once.

An unchanged MIT-licensed [libtiff-produced three-page file](https://github.com/image-rs/image-tiff/blob/3302936b08e2260d4c66f40f51170efbb1fded4c/tests/images/seq/seq-1c-8b-multipage-adeefdcc.tiff) supplies independent byte, voxel, surface area and volume checks. Authored nonsymmetric controls qualify scalar types, BigTIFF, byte order, orientation, resolution and compression, with committed PNG comparisons. Linked directory counts, cycles, duplicate tags and source spans are checked before initializing the codec. During pixel decoding, the owned reader restricts reads to each declared strip/tile span. Limits include 4,096 directories, 4,096 entries per directory, 65,536 entries total, 1 MiB per metadata value, 32 MiB of declared metadata, 16,777,216 scalar samples, 128 MiB expanded samples, a conservative 512 MiB aggregate source/decode/geometry allowance, and 512 MiB of cumulative encoded plus padded decode work, counting reused chunks each time.

RGB/multichannel stacks, palette volumes, heterogeneous image series, SubIFD pyramids, OME hyperstack axes and ImageJ planes without independently addressed directories remain unsupported. A TIFF filename alone does not establish compatibility with every scientific TIFF layout.

See [raster-image rendering](#raster-image-rendering) for sizing and decoding limits.

## PNG

**Extensions:** `.png`.

**Coverage:** Implemented: 2D image preview.

PNG inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## JPEG

**Extensions:** `.jpg`, `.jpeg`.

**Coverage:** Implemented: 2D image preview.

JPEG inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## BMP

**Extensions:** `.bmp`.

**Coverage:** Implemented: 2D image preview.

BMP inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## TGA

**Extensions:** `.tga`.

**Coverage:** Implemented: 2D image preview.

TGA inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## HDR

**Extensions:** `.hdr`.

**Coverage:** Implemented: 2D image preview.

HDR inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## WebP

**Extensions:** `.webp`.

**Coverage:** Implemented: 2D image preview.

WebP inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## EXR

**Extensions:** `.exr`.

**Coverage:** Implemented: 2D image preview.

EXR inputs produce a fitted 2D image preview through the enabled Rust image decoder. See [raster-image rendering](#raster-image-rendering) for sizing, transparency and decoding limits.

## Quake MDL

**Extensions:** `.mdl`.

**Coverage:** Subset: Quake IDPO v6 triangle geometry, first frame, grouped frames.

Quake MDL accepts IDPO version 6 and validates single/grouped skins and single/grouped frames. The first frame of the first group supplies indexed triangles after the stored scale and origin are applied. Skin images and supplied normal indices are not rendered, so the preview uses default shaded geometry. Other MDL dialects are unsupported. The binary layout follows the [original Quake header](https://github.com/id-Software/Quake/blob/master/WinQuake/modelgen.h).

## CityGML

**Extensions:** `.gml`.

**Coverage:** Subset: highest feature LoD planar surfaces, holes, local XLinks and diffuse appearances.

CityGML selects the highest supported LoD for each feature and triangulates actual planar Polygon/Triangle/Rectangle/PolygonPatch surfaces, including holes. `pos`, `posList`, legacy `coordinates`, local XLinks and diffuse X3DMaterial appearances are supported. Implicit geometry, curved surfaces, composite rings/curves and Tin schemas fail explicitly. Authored coordinates are retained without CRS reprojection; texture images are omitted. An independent [OGC CityGML sample](https://github.com/opengeospatial/CityGML-3.0Encodings) checks a building in EPSG:25832.

## PTS

**Extensions:** `.pts`.

**Coverage:** Implemented: point positions and optional RGB.

PTS previews render stored point positions and optional RGB. The reader supports the recognized count/header and point-record layouts; point rendering follows the shared fixed-view policy.

## STEP

**Extensions:** `.step`, `.stp`, `.stpnc`, `.p21`, `.210`.

**Coverage:** Subset: supported Part 21 B-rep shells, planar faceted solids and AP242 triangle geometry, axis-based assemblies/mappings and length contexts in Rust.

STEP uses `truck-stepio` 0.3 and Truck's robust constrained surface tessellation. Supported open/closed B-rep shells become actual triangles, including supported curved surfaces. Full spheres and ring tori with vertex-loop bounds use direct analytic surface evaluation, retaining placement and checking that the loop vertex lies on the surface. The reader checks resolved topology, face/boundary preservation and per-face tessellation so incomplete geometry fails explicitly. Constrained tessellation uses 0.3% of each shell's estimated diameter from vertices and sampled boundary curves, with a kernel-tolerance floor; full analytic sphere/torus surfaces use a 64-by-64 parameter grid.

An owned decoder reads selected `FACETED_BREP` solids with a non-oriented `CLOSED_SHELL`, and selected open/closed polygon shells. Their planar `FACE_SURFACE` faces use one explicit `FACE_OUTER_BOUND` and optional `FACE_BOUND` holes; each [POLY_LOOP](https://www.steptools.com/stds/stp_aim/html/t_poly_loop.html) references at least three distinct Cartesian points with implicit closure. Concave exteriors and multiple holes retain their openings. Bound orientation, `FACE_SURFACE.same_sense` and `ORIENTED_FACE` wrappers determine winding and flat plane normals. Supported simple and external-complex inheritance layouts are checked for bodies, shells, faces, bounds, loops, points and plane placements. Finite 3D points, valid plane axes, coplanarity, unique edges/vertices, ring simplicity, exterior/hole winding, hole containment and triangulated area are validated; touching, overlapping and nested holes fail. Each face is capped at 4,096 vertices. Faceted and AP242 loading share cumulative face, geometry, work and estimated allocation budgets. This implements the planar boundary subset of the [faceted representation schema](https://www.steptools.com/stds/stp_aim/html/t_faceted_brep_shape_representation.html), without changing upstream dependencies.

Owned loading functions also read AP242 `COORDINATES_LIST`, `TRIANGULATED_FACE`, `COMPLEX_TRIANGULATED_FACE`, triangle surface sets, and face-only `TESSELLATED_SHELL`/`TESSELLATED_SOLID` containers. They construct ordinary scene triangles directly, retaining optional per-corner normals and point/normal index indirection. Strips alternate winding and fans retain their center. Simple and supported complex Part 21 instances are checked against their inherited attribute layouts. Empty aggregates are preserved as empty lists in the unchanged upstream AST, separately from omitted values and empty subtype records; dependencies require no patches. Declared counts, finite vectors, one-based integer indices, exact-geometry links and cumulative allocation/work limits are validated.

Supported shape representations retain nested `REPRESENTATION_MAP`/`MAPPED_ITEM` instances with 3D axis frames, and assembly occurrences linked through `NEXT_ASSEMBLY_USAGE_OCCURRENCE`, `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` and axis-based `ITEM_DEFINED_TRANSFORMATION` relationships. Product ownership determines transformed relationship direction; plain shape relationships group equivalent representations in either endpoint order and reject ambiguous links between different products. Nonzero mapping origins are subtracted before rotation and translation. Shell meshes are cached and reused for each placed occurrence. Representation length contexts, including SI prefixes and supported simple/complex conversion-based units, normalize geometry and frame translations to millimeters. Bare shells without representation contexts retain the existing coordinate convention. Unowned shells, missing placements, unsupported required representation items, cycles and excessive hierarchy expansion fail explicitly.

AP242 triangles use the same ownership, units and placements, with normals rotated and normalized for each occurrence. Only selected shape items are decoded, excluding associated validation, construction and drafting-view geometry. An explicit solid/shell link to an exact body in the same representation group avoids drawing its tessellated alternative twice. Unlinked bodies and separate occurrences remain distinct. Tessellated representations with accuracy parameters are recognized; selecting between unlinked levels of detail is outside this subset.

Tests generate a translated rectangular solid, cylinder, sphere and torus, verify surface areas/bounds, render a thumbnail and exercise all five extension aliases. Assembly regressions independently compare every placed vertex for repeated and nested parts, reversed relationship direction, mixed metre/millimeter contexts and inch conversion. [Authored assembly snapshots and a retained Apache-2.0 CadQuery export](../tests/fixtures/formats/scientific_cad/README.md) provide portable image comparisons. The independent export contains two placed bodies with curved geometry and conversion-based metre units; it does not establish real-exporter assembly hierarchy coverage. A wide, thin cylinder checks that boundary-curve extent controls tessellation density. Pinned upstream [OCCT cube, cone, cylinder, sphere and torus fixtures](https://github.com/ricosjp/resources4truck/tree/9bf9de1426c5fc2f2b8d63f501dca0d3c53ebd91/step) also render successfully.

The unchanged [NIST FTC 08 AP242 tessellated export](../tests/fixtures/formats/scientific_cad/README.md) provides a real-file snapshot with 3,388 authored triangles, including 20 zero-area triangles, and optional surface normals. Tests verify its exact source checksum, selected-part bounds and embedded producer surface area, and exclude ancillary geometry and duplicate view copies. Authored controls separately qualify direct triangles, fans, surface sets, normal indirection/rotation, mixed-unit repeated placements and explicitly linked B-rep alternatives. This one export does not establish broad AP242 edition or exporter coverage.

The unchanged [Jumper faceted STEP and source STL](../tests/fixtures/formats/scientific_cad/README.md) provide separately licensed Apache-2.0 exporter snapshots. The mesh-derived AP214 component contains 768 triangular planar faces; independent binary-STL expectations check its count, placed bounds, surface area and signed volume. Authored controls separately exercise polygon holes, orientation and faceted assembly placement. The retained STEP is a replica producer's export of robot mesh geometry, rather than original manufacturer CAD, and this single export does not establish broad faceted schema or exporter compatibility.

Plain context-dependent identity occurrences without transformed relationships, additional transformation operators, unsupported entities, unsupported/mixed vertex-loop topology, horn/spindle tori, wire-only files and non-geometric STEP-NC/AP210 contents remain unsupported. The faceted decoder rejects required mixed `EDGE_LOOP`/`VERTEX_LOOP` boundaries, bare `FACE`, `ADVANCED_FACE` with polygon loops, nonplanar surfaces, void solids and oriented outer shells. AP242 tessellated edge/vertex children, repositioned items, curved/polygonal tessellation and tessellated B-rep face-surface schemas remain unsupported. Materials, PMI and angular-unit conversion are not evaluated. Entity count, topology/representation depth, face and output triangle limits apply. Alias recognition does not establish full schema parity.

## IGES

**Extensions:** `.iges`, `.igs`.

**Coverage:** Subset: supported manifold/constructive solids and independent trimmed/bounded sheets, curved surface tessellation.

IGES supports the kernel's manifold B-rep and constructive solids and independent trimmed/bounded sheets, converting recognized global units to millimeters. The [IGES specification](https://nvlpubs.nist.gov/nistpubs/Legacy/IR/nbsir86-3359.pdf) defines global parameter 19 as the distance below which authored coordinates should be coincident. Its declared resolution is read with Hollerith strings/custom delimiters and converted to millimeters, then retained through import, bounds and tessellation. Positive resolutions up to 0.1 mm are accepted; omitted resolution retains the kernel default. Fixed-column sequences, directory/parameter ownership and termination counts are checked before import. Recognized logical groups retain their owned solid/sheet geometry; physically subordinate surfaces retain their geometric owner. A project-owned adapter builds independent trimmed faces through the published kernel geometry/topology APIs and retains every authored face in a compound without sewing or inventing connectivity. Ordinary manifold/constructive solids and subfigure instances use the unmodified published IGES reader. Repeated geometry is bounded at two million expanded numeric parameters and 100,000 topology nodes before allocation. Type-142 boundaries use their authored 3D trim where supplied, while owned alternate parameter-space curve definitions are validated and accounted for; independent/unowned curves remain explicit errors. A regression compares authored, imported and rendered face counts, area and bounds, and rejects unowned or malformed alternate definitions. Genuine boundary ranges crossing singular chart poles are split at verified surface points using the public curve projector before chart construction; a hand-authored great-circle hemisphere regression preserves both sphere poles, one complete face, analytic area and bounds. Geometry omissions and approximation warnings fail explicitly; non-geometric drafting metadata can be omitted. Bare curves/untrimmed surfaces and unsupported entity forms remain outside this subset.

The original OCCT [bearing.iges](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/iges/bearing.iges) retains all 213 authored trimmed faces and imports 3703 triangles without geometry warnings; ordinary sewing had returned zero shells. `hammer.iges` still fails a projected pcurve fit at approximately 0.689 mm, well beyond its declared 0.001 mm precision; geometry compromises remain explicit errors. Broader OCCT/IGES compatibility remains unfinished.

See [IGES and BREP tessellation](#iges-and-brep-tessellation) for the shared Rust kernel, deflection policy, bounds and regression checks.

## BREP

**Extensions:** `.brep`.

**Coverage:** Subset: Open CASCADE ASCII and little-endian binary topology, supported analytic/NURBS geometry, occurrence placements and curved tessellation.

BREP reads genuine Open CASCADE ASCII placement/geometry/topology tables and supports the kernel's analytic/NURBS geometry. A Catwalk loader preserves all three authored analytic frame axes, including left-handed chart parameterizations, by rebuilding analytic objects through public geometry constructors; cylinder/native-circle regressions check parametric points, area and placement. Shape occurrence locations are applied. Rigid curve/surface carrier placements, including reflections, are restored from native metadata and materialized into actual analytic/NURBS geometry before tessellation. Equivalent native placements share transformed surface identities. A pcurve carrier is composed with its edge occurrence and matched to its face's composed carrier, preserving separately placed boundary edges. Scaled carriers fail explicitly; occurrence scaling remains supported. Translation/reflection, repeated carriers, opposing edge/carrier placements and separate occurrence regressions check exact area and bounds. As specified by [OCCT BRep_Tool](https://occt3d.com/dev/doc/refman/html/class_b_rep___tool.html), omitted planar pcurves are generated through exact closed-form projection; boundaries are first proved coplanar from analytic geometry or positive-weight spline control points. Off-plane boundaries and omitted pcurves on curved surfaces fail. Native oriented endpoint references restore connected wire traversal before meshing; source edge occurrences, placements and face identities are retained. Cached polygon/triangle tables are validated then discarded; faces are remeshed from their surfaces and trimming curves. Valid legacy fused index/continuity tokens are separated. Unsupported cached-data layouts and geometry records fail. Files beginning with the bare CASCADE topology header are also accepted. Compounds preserve free 3D curves and isolated vertices alongside faces.

Standalone little-endian binary BREP native topology versions 1–4 are decoded through the same owned native table reader used for supported XBF shape sections. The final root occurrence, backward references, topology orientation, occurrence placements and supported analytic/trimmed/offset/spline geometry are preserved before Rust tessellation. Rigid curve/surface carriers transform the actual geometry; uniform occurrence scaling remains supported. Nonzero binary polygon or triangulation tables, unknown versions/records and scaled carriers fail explicitly; binary cached meshes are not substituted for analytic geometry. Inputs and converted geometry text are capped at 16 MiB, native tables/expanded occurrences at 100,000, hierarchy depth at 64 and selected preview primitives at 4,096. The shared CAD tessellator retains its output and numerical tolerance limits.

The unchanged [F3D binary version-4 and ASCII counterpart files](../tests/fixtures/formats/binary_brep/README.md) retain the original BSD-3-Clause license and data-license mapping. Independent tests compare every emitted vertex and oriented triangle against the original eight-face source topology, bounds, surface area `55.018917559102576` and signed volume `12.5`, and compare their fixed-view renders. Four authored version-1–4 controls separately check free wires, isolated points, root/carrier reflection and rotation, repeated occurrences and uniform root scaling. These controls do not establish all historical optional vertex encodings or universal Open CASCADE compatibility.

Original OCCT [Room.brep](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/occ/Room.brep), [face1.brep](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/occ/face1.brep) and [solid.brep](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/occ/solid.brep) import with 5, 285 and 972 triangles respectively. Other checked OCCT files still fail missing pcurves or the bounded mesh deflection check; `Ball.brep` reaches genuine face tessellation but still exceeds that numeric tolerance.

See [IGES and BREP tessellation](#iges-and-brep-tessellation) for the shared Rust kernel, deflection policy, bounds and regression checks.

## XBF

**Extensions:** `.xbf`.

**Coverage:** Subset: native binary XCAF static B-rep geometry, free shapes, instance placements and diffuse colors.

XBF decodes genuine little-endian `BINFILE`/BinXCAF documents, including supported version-6–11 native shape tables and version-12 inline shape records. Analytic curves/surfaces, supported spline geometry, topology and document free-shape selection are interpreted before Rust tessellation; the file is not treated as ASCII BREP. Native backward references, hierarchy placements, rigid curve/surface carrier placements, mirroring and root-instance uniform scaling are preserved. Carrier geometry is actually transformed before tessellation because the kernel's ASCII bridge does not retain those placements. Diffuse RGB references and supported per-face colors are rendered. Input/conversion size, hierarchy depth, geometry counts and output are bounded. External parts, big-endian documents, unknown required sections/geometry, polygon-only or cached-triangulation representations and scaled curve/surface carriers fail explicitly. Shader styles, textures, alpha and unit metadata are omitted. Six regression tests check native serialization, actual area/bounds/colors, transformed shared face/pcurve geometry, point tables, mirroring and malformed input. Independent [F3D XBF fixtures](https://github.com/f3d-app/f3d/tree/master/testing/data) `f3d.xbf` and `xcaf-colors.xbf`, plus the [OCCT test assembly](https://github.com/a-betenev/opencascade-dataset/tree/master/xbf) `as1-no-ass-name.xbf`, render successfully. Some other real XBF documents still fail explicitly when the CAD kernel lacks required pcurves or cannot meet its bounded preview tessellation tolerance.

## FreeCAD

**Extensions:** `.fcstd` (case-insensitive, including `.FCStd`).

**Coverage:** Subset: addressed saved PNG and stored ASCII BREP document shapes with supported visibility, containers, local links and model arrays.

The default preview first extracts the exact ZIP member `thumbnails/Thumbnail.png`. Missing or unreadable saved images fall back to fresh geometry. `--render-geometry` bypasses the image; `--extract-saved-preview` returns the original image dimensions and does not load shapes. A saved image may be stale or differ from the document's current geometry.

Geometry loading admits schema-4, file-version-1 `Document.xml` and selects shapes through each object's typed `Shape` property and addressed `Part` member. It never chooses an arbitrary BREP from the archive. Stored shapes use the existing bounded Rust ASCII BREP import and tessellation pipeline. Normal object placement is already present in the BREP root occurrence; supported part ancestors add their placements without applying the object's placement twice. Local links and model arrays replace the target root occurrence and honor the qualified `LinkTransform` setting. Visibility and supported aggregate/body selection prevent hidden history or duplicate container results from being rendered. Optional GUI properties supply diffuse colors, qualified per-face colors, transparency and wireframe display.

Original [F3D FreeCAD references](../tests/fixtures/formats/freecad/README.md) retain their BSD-3-Clause notice and dataset-license mapping. They qualify FreeCAD 0.21 and 1.1 document records, ancestor transforms, local links, arrays, visibility, body selection, legacy colors and version-3 material lists. Independent geometry checks use literal original box corners, areas, volumes, bounds and native face positions; saved and fresh previews have separate PNG comparisons. These files include application exports and upstream protocol controls, and do not establish compatibility with every FreeCAD release or object family.

Archives are bounded before ZIP library indexing, with physical and normalized duplicate names rejected. Selected members are bounded and fully decoded for CRC checking. XML forbids DTDs and has finite byte, node and hierarchy limits; placement scalars and addressed lists are checked before geometry expansion. Source archives are capped at 64 MiB, individual XML/shape members at 16 MiB and cumulative selected decompression at 128 MiB. Existing per-shape CAD bounds remain in force, with additional document and instance budgets. Saved PNGs are capped at 8 MiB encoded, 4096 pixels per axis and sixteen million pixels.

External links, unresolved selected shapes, target subelements, link scaling, unsupported array layouts and ambiguous/cyclic ownership fail explicitly. FreeCAD-specific binary shape members, inline shape carriers and unsupported material layouts remain unqualified; they are not passed to the standalone binary BREP reader. Textures, material UUID behavior, procedural recomputation and unsupported document object modes are outside this stored-shape reader.

## Alembic

**Extensions:** `.abc`.

**Coverage:** Subset: Ogawa/HDF5 first-sample meshes, subdivision cages, points, sampled curves, transforms, instances and RGB.

Alembic reads Ogawa version-1 archives and supported HDF5 archives in Rust, selecting sample zero from each supported property. Polygon meshes, subdivision control cages with holes, points and sampled linear/cubic curves produce geometry. Transform operations, inheritance resets, nested object instances, visibility and supported RGB/RGBA arbitrary geometry parameters are applied; face/vertex colors become diffuse triangle or point colors. Authored coordinates are retained. Subdivision evaluation, curve widths, shader materials, alpha blending and later samples are omitted. NURBS patches, rational or variable-order curves and unknown/custom geometry schemas fail explicitly; layered archives are not composed. Node depth, counts, numeric sample sizes and aggregate decoded bytes are bounded. Nine regression tests and eight independent geometry archives from [Alembic](https://github.com/alembic/alembic/tree/master/prman/Tests/testdata) and [Pixar OpenUSD](https://github.com/PixarAnimationStudios/OpenUSD/tree/dev/pxr/usd/plugin/usdAbc/testenv) check Ogawa/HDF5 meshes, colors, instancing, first poses and curves against the primary [Alembic storage and geometry implementations](https://github.com/alembic/alembic/tree/master/lib/Alembic).

## OBJ

**Extensions:** `.obj`.

**Coverage:** Subset: static polygons, polylines and points, per-corner RGB/normals/UVs, local MTL diffuse maps and dissolve.

OBJ loads static positions, faces, polylines and explicit points, with concave-aware triangulation. Positive and negative position, texture-coordinate and normal indices are resolved independently; polygon corners retain their corresponding attributes after triangulation. Supplied normals and quantized sRGB vertex RGB are interpolated for shaded faces. Smoothing groups do not generate normals. Zero-area triangles are omitted; zero normals on surviving geometry fail explicitly. Polylines retain each consecutive segment and their existing byte RGB colors. Unreferenced vertices are not added to a mesh or wire preview; a point-only input can use its positions.

Local MTL `Kd` accepts scalar gray or RGB values in 0–1 as linear diffuse reflectivity multipliers. Scalar `d` and the common inverse-opacity `Tr` extension accept 0–1 values and control dissolve; spectral/XYZ reflectivity is unsupported. Diffuse `map_Kd` images retain authored UVs, with one bottom-left-to-top-left V conversion. PNG, JPEG, BMP, TGA, HDR, WebP, EXR and TIFF codecs are supported. `-s`, `-o` and `-clamp` options control scale, offset and addressing. Default addressing repeats; `-clamp on` retains the underlying material outside the unit square. Identity `-mm 0 1`, zero `-t`, `-cc off`, and default `-blendu on`/`-blendv on` options are accepted. Nonidentity unsupported modifiers, malformed existing materials/images and inconsistent corner attribute layouts fail. Missing optional appearance resources leave decoded geometry and material color available. Image paths resolve relative to their MTL directory, including normalized Windows separators. Spaces and percent signs are literal filename characters; a well-formed percent-decoded compatibility fallback is tried only when the literal file is missing. Parent-directory references are accepted when the canonical target remains inside the OBJ model directory. Absolute paths, URLs and lexical or symlink escapes from that root fail explicitly. The original [Wavefront 4.2 material documentation](https://github.com/Alhadis/language-wavefront/blob/master/docs/mtl-spec.rst) describes diffuse multiplication, dissolve and clamping. Ambient/specular effects, bump maps, reflection maps, halo dissolve, complete illumination models and free-form curve/surface geometry remain outside this thumbnail subset.

OBJ and MTL text preserves valid UTF-8; otherwise a lossless Windows-1252 fallback retains legacy names and preserves undefined bytes distinctly. Encoded input, decoded text expansion and accumulated appearance resources share a 512 MiB limit. Geometry/attribute storage is limited to 256 MiB, with 20 million coordinate/output records and 100,000 corners per face or polyline. Material references are limited to 1,024 and definitions to 65,536; logical lines are limited to 4 MiB. Textures have an 8,192-pixel axis limit, 16 million pixels per image and 64 million aggregate pixels. Other scene/render limits still apply.

The unchanged [Max2Obj-exported Assimp building and original MTL file](../tests/fixtures/formats/obj_appearance/README.md) provide licensed real-file coverage for independently indexed UVs, diffuse materials and translucent windows. Its original legacy material names are decoded without modifying source bytes. The 2,710 source triangles include four zero-area faces, yielding 2,706 rendered triangles; 668 retain UVs. This export has no supplied normals or texture maps. Independent tests check source hashes, all ordered triangle vertices, emitted bounds/area, material counts and literal corner attributes. Authored snapshots separately qualify decoded image pixels, a concave textured polygon, normal/RGB interpolation, white-map invariance, transformed repeat/border sampling and depth-composited `d`/`Tr` opacity. These examples do not establish every exporter, image variant or Wavefront shading feature.

## glTF

**Extensions:** `.gltf`.

**Coverage:** Subset: textured triangle/point/line scenes, alpha, normals, first poses, skins/morphs, GPU instances and Rust Draco/quantized decoding.

glTF accepts triangle, triangle-strip, triangle-fan, point and line primitives, scene transforms, base material factors and vertex colors, following the [glTF 2.0 specification](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html). It evaluates the first animation at its earliest active channel timestamp; other channels clamp to their first value, and cubic samplers select the value from the first tangent/value/tangent triple. Unanimated scenes retain their authored pose.

Morph position, normal, selected texture-coordinate and `COLOR_0` deltas, including sparse accessors and permitted normalized integer data, are applied before skinning. Node weights override mesh defaults; primitives may omit targets and contribute zero deltas. Joint globals and inverse bind matrices produce genuine weighted geometry across multiple influence sets; the skinned mesh node transform is excluded as required by glTF. Normals use the inverse transpose of the applicable affine transform, including the weighted joint matrix, then normalize. Singular transforms or cancelled normals fall back to geometric face normals.

Triangle materials sample actual PNG/JPEG base-color images from local relative paths, base64 data URIs or GLB buffer views. Selected `TEXCOORD_n`, `KHR_texture_transform` offset/rotation/scale and coordinate overrides are preserved. Shared images are decoded once. Repeat, mirrored repeat and clamp wrapping, distinct nearest/bilinear magnification and minification, and the specified base-level fallback for mipmap modes are supported. Texture RGB is decoded from sRGB before filtering and multiplication with linear base factors and interpolated vertex RGBA. Material `OPAQUE`, `MASK` and `BLEND`, alpha cutoffs above one, single-sided culling, mirrored winding, double-sided normal reversal and `KHR_materials_unlit` are evaluated. Blended fragments are sorted by depth for each raster sample and respect opaque geometry; appearance supersampling resolves linear premultiplied color.

`KHR_draco_mesh_compression` decodes positions, normals, selected UVs, colors, joints and weights with checked attribute IDs, counts, types and normalization. `KHR_mesh_quantization` supports permitted byte/short positions, normals and texture coordinates, retaining authored node or inverse-bind dequantization transforms. `EXT_mesh_gpu_instancing` expands actual local TRS instances before the node's global transform; omitted TRS attributes retain their defaults and the uninstanced base mesh is not added. Required extensions outside these three, `KHR_texture_transform` and `KHR_materials_unlit` fail explicitly. Skin combined with GPU instancing is explicitly unsupported. Full metallic/roughness shading, normal/emissive/occlusion maps, texture appearances on point/line primitives, later animation times and playback remain omitted.

Limits include 256 images/buffers, 1024 textures, 100,000 nodes/materials/instances per instancing batch, five million expanded geometry elements, fifty million expanded vertex transformations and 512 MiB aggregate buffer/external-image bytes. Each decoded texture is limited to 8192 pixels per axis and sixteen million pixels; distinct textures total at most sixty-four million pixels. Appearance rasterization caps bounding-box visits at 256 million and retained transparent fragments at four million. Exceeding a limit produces an error.

Importer/deformation/animation tests check weighted geometry and normals, sparse data, quantization, morphs, first poses, instance composition and malformed input. [Thirteen additional gallery cases](../tests/fixtures/formats/mesh_scene/README.md) include authored appearance controls and four unchanged, licensed Khronos models: TextureCoordinateTest, TextureSettingsTest, AlphaBlendModeTest and SimpleInstancing. Independent qualification checks source hashes, geometry counts/bounds/areas, decoded PNG/JPEG pixels, samplers, material metadata and hand-computed transforms; the genuine instancing asset retains all 125 instances and 1500 triangles. Earlier independent checks rendered 22 [Khronos sample-asset variants](https://github.com/KhronosGroup/glTF-Sample-Assets) and compared their emitted vertices against a separate accessor/TRS/morph/skin evaluator and Google Draco decoder. These checks establish the tested subset, not every extension or asset. See [GLB](#glb) for binary packaging.

## GLB

**Extensions:** `.glb`.

**Coverage:** Subset: textured triangle/point/line scenes, alpha, normals, first poses, skins/morphs, GPU instances and Rust Draco/quantized decoding.

GLB decodes the binary glTF container, its JSON chunk and binary buffer chunk. Geometry, textures, alpha, normals, transforms, first animation poses, skins, morphs and supported extensions follow the [glTF reader](#gltf). Relative external resources and buffer/accessor layouts are validated. Generated glTF/GLB pairs render identically, and the independent Khronos checks include binary-packaged variants.

## Draco

**Extensions:** `.drc`.

**Coverage:** Subset: sequential/Edgebreaker meshes with authored corner normals, sequential/kd-tree point clouds, positions and colors.

Standalone Draco accepts sequential and Edgebreaker meshes, including valence and supported legacy streams, and sequential/kd-tree point clouds. Positions, mapped numeric colors and three-component authored mesh normals are rendered. Normals follow point-to-attribute mappings and become interpolated unit directions at triangle corners; a zero corner uses its face direction, while a triangle with entirely zero normals retains flat shading. Nonfinite, ambiguous or incorrectly sized mesh normal attributes fail explicitly. Point-cloud normals are ignored, retaining the existing position/color rendering. Standalone UV textures remain unsupported.

Limits cap decoded geometry at five million points/faces and expanded attributes at 512 MiB. Meshes with normals also preflight their combined decoded attributes, expanded tuples, topology and conservatively estimated appearance storage against 512 MiB. [Three unchanged Google cube encodings](../tests/fixtures/draco/README.md) qualify producer geometry, all face-normal associations, area and volume. Authored controls check differing mappings, integer and extreme finite directions, zero/malformed normals, storage limits and interpolated lighting. Existing colors and custom fallback colors remain effective for appearances that supply only normals; explicit material and texture colors retain their own semantics.

## 3DS

**Extensions:** `.3ds`.

**Coverage:** Subset: static triangle meshes, diffuse colors and earliest-key object hierarchy.

3DS reads binary triangle meshes, per-face diffuse colors, master scale and the earliest object translation/rotation/scale keys. Editor vertices, inverse mesh matrices, pivots, instance/dummy nodes, node-ID parent hierarchies and mirrored transforms are accounted for. Camera/light ancestors and the earliest camera roll are evaluated for mesh-child placement; their views and illumination are not rendered. Textures, material alpha, smoothing groups and later animation keys are omitted. Morph/visibility tracks, prescale, unlinked parent axes, legacy editor or parent-name hierarchies and unknown geometry chunks fail explicitly. The conventions are checked against the primary [Assimp](https://github.com/assimp/assimp/tree/master/code/AssetLib/3DS) and [Blender](https://github.com/blender/blender-addons/blob/main/io_scene_3ds/import_3ds.py) importers; six upstream Assimp models also render successfully.

## VRML

**Extensions:** `.wrl`, `.vrml`.

**Coverage:** Subset: VRML97 static geometry, local typed PROTO/IS instances, DEF/USE, transforms, fixed-view billboards and colors.

VRML accepts VRML97/V2.0 text with `DEF`/`USE`, groups, transforms, Anchor/Collision children, the selected Switch choice and the first LOD level. IndexedFaceSet, IndexedLineSet, PointSet, ElevationGrid, Extrusion, Box, Sphere, Cylinder and Cone produce geometry. Extrusion evaluates the specified spine-aligned frames, scales, orientations, open/closed cross-sections and spines, coincident spine sections, and concave caps. Diffuse surface materials, emissive line/point materials, and authored face/vertex colors are evaluated; vertex colors are averaged for the preview's flat triangle/line colors. Center, rotation, scale orientation and Y-up conversion are applied. Existing geometry regressions and seven primary [Web3D sourcebook examples](https://www.web3d.org/x3d/content/examples/Vrml2Sourcebook/) cover terrain, colored lines, a torus and twisted/concave extrusions; [Assimp's Wuson fixture](https://github.com/assimp/assimp/tree/master/test/models/WRL) also renders.

Local `PROTO` declarations accept typed defaults and instance overrides, scalar/vector/color/index/node arrays, nested declarations and `IS` bindings. Interface types and directional field/event categories follow [VRML97 Table 4.4](https://www.web3d.org/documents/specifications/14772/V2.0/part1/concepts.html#4.8): an interface `field` can bind a body's `field` or `exposedField`, while an interface `exposedField` cannot bind a body's `field`. Implicit input/output event aliases of exposed fields are resolved on either side of `IS`; event-only associations do not supply a static initial value. The first body node supplies the inherited node type and scene graph; later body nodes do not become extra geometry. Each instance has its own body-local `DEF`/`USE` scope. Caller-supplied SFNode/MFNode overrides retain their caller scope, and nested prototype declarations remain local. The unchanged Web3D SpinGroup and ProtoInstanceExample originals, plus four authored typed-array, geometry/material-wrapper, nested-scope and asymmetric Billboard controls, have [fixture details and retained licensing](../tests/fixtures/formats/vrml_prototypes/README.md). Their regressions compare literal geometry and colors as well as complete PNG outputs. Token/value/template/copy work is limited cumulatively to 20 million items, parsed/copied nodes and templates to 100,000, nesting/expansion depth to 256, and retained token/template/copy payload to 512 MiB with allocation preflight.

Billboard children face the renderer's primary orthographic view using an actual rotation in each occurrence's local frame. Constrained-axis facing and zero-axis viewer alignment use the inverse parent linear transform; translation does not affect the orthographic viewing ray. Non-finite, non-affine or singular transforms and numerically undefined axis/view or up/view alignment fail explicitly. An axis parallel to local Z uses a deterministic zero-angle rotation because rotating about it cannot change the facing Z direction. Zero-axis alignment uses the pulled-back viewing ray and camera-up direction. A parent's nonuniform scale or shear remains in effect, so local up alignment does not promise exact world-space camera-up alignment. Billboard children retain the existing traversal and output bounds. Facing is frozen against the primary reference at import; the renderer can subsequently select an alternate view for an entirely edge-on surface-only scene without reevaluating Billboard orientation. The asymmetric regression includes point markers to fix its renderer view. The static preview does not use authored cameras or run subsequent events.

VRML1, `EXTERNPROTO`, scripts, Inline, Text and unknown scene/geometry nodes fail explicitly. Textures, camera/lighting nodes and event evaluation are omitted from the static preview. This qualifies local static prototype expansion rather than all VRML97 or X3D behavior.

## FBX

**Extensions:** `.fbx`.

**Coverage:** Subset: ASCII/binary static polygon scenes, weighted linear skins, progressive morphs, hierarchy, pivots, units, first keys and colors.

FBX accepts ASCII, including supported legacy version-6 inline meshes, and binary 7.x with 32/64-bit node records and zlib arrays. Static polygons, points, lines, instance hierarchies, transform pivots, pre/post rotations, geometric transforms, all three supported scale-inheritance modes, units, up axes, visibility and first transform keys are evaluated. Linear/rigid skins use authored mesh-to-bone bind matrices, current bone hierarchies and genuine control-point weights; Normalize and TotalOne link modes retain uninfluenced points and geometric offsets. Sparse blend shapes use default DeformPercent or the earliest authored key values, interpolate progressive full weights, support negative/extrapolated weights and accumulate parallel channels before skinning. Recognized Blender painted target weights are applied per vertex. Diffuse materials, per-face assignments and the first vertex-color layer influence the preview. Unknown deformers, dual-quaternion/hybrid skins, additive/associate-model skin modes, multiple skins on one mesh, constraints, NURBS, spherical rotations and layered controlling curves fail explicitly. Deformation work is bounded to five million control points/influences/offsets, with ID, index, weight and affine-matrix validation. Texture images, imported normals and later animation are omitted; embedded image byte metadata can be parsed without sampling those textures. Sixteen unit tests include analytic weighted positions and compressed/uncompressed binary deformation regressions. Independent [Assimp FBX fixtures](https://github.com/assimp/assimp/tree/master/test/models/FBX) cover rigid scenes plus skeleton animation and huesitos skins; [ufbx fixtures](https://github.com/ufbx/ufbx/tree/master/data) cover half-skinned Blender geometry, static blend shapes, Maya skinned pivots, transformed skin and progressive morphs. Every emitted vertex from seven skin/morph fixtures matched an independent ufbx static/earliest-sample evaluation within 2.2e-14 metres; the reference harness is only a development check, with no native dependency in Catwalk.

## COLLADA

**Extensions:** `.dae`.

**Coverage:** Subset: static indexed surfaces and wires, planar polygon holes, instancing, affine transforms, units and up axes.

COLLADA accepts version 1.4/1.5 static indexed triangles, polygons, polylist, triangle fans and strips, line pairs and line strips. Planar `<polygons><ph>` entries retain their exterior and hole rings; triangulation checks planarity and preserved projected area. Float-array accessors use named component slots in their declared order, skipping unnamed slots; the names themselves do not determine XYZ or RGB order. Repeated geometry/node instances, nested affine matrices/TRS, node-placement `lookat`, angular `skew`, global unit scaling and X/Y/Z up axes are evaluated. Diffuse materials and indexed vertex colors influence flat surface and segment colors; each segment averages its two endpoint colors. Textures, imported normals and animations are not evaluated.

The importer validates source indices, accessor ranges, primitive counts and instance cycles. Skin/morph controllers, spline/BREP geometry, references to other documents and local asset coordinate-system overrides fail explicitly. Zero-area hole rings are unsupported and return an error. Undefined look-at axes, zero/parallel skew axes and singular angular skew intervals also fail. Matrix XML is read row by row; `lookat` places local geometry at the eye position, and `skew` displaces along its second authored axis according to the general angular convention in the [Khronos COLLADA specification](https://www.khronos.org/files/collada_spec_1_4.pdf).

Limits allow at most 64 primitive inputs/tuple offsets, 100,000 polygon corners or line-strip vertices, 1,024 rings per holed polygon and 100,000 index lists in each wire or holed-polygon primitive. Scene traversal permits 100,000 node visits. Each numeric/index list and geometry count is limited to 20 million items; decoded float arrays and mesh/output vector allocations also share a cumulative 256 MiB budget. [Two unchanged Khronos conformance models and four authored controls](../tests/fixtures/formats/collada_primitives/README.md) have independent position, topology, color, surface-area and signed-volume checks plus gallery snapshots. The original XML files preserve their exporter metadata and Khronos license; these cases qualify static geometry, rather than full exporter or shader compatibility.

## OFF

**Extensions:** `.off`.

**Coverage:** Subset: polygon geometry and colors.

OFF accepts the supported polygon/color variants; format-specific higher-dimensional geometry is outside this subset.

## DXF

**Extensions:** `.dxf`.

**Coverage:** Subset: ASCII/binary drawing faces, points, lines, sampled curves/splines, meshes and nested block instances.

DXF accepts ASCII and modern 16-bit/legacy 8-bit binary group codes. Faces, solids, points, lines, sampled circles/arcs/ellipses, authored rational B-splines, bulged polylines, rectangular meshes, polyfaces and level-zero MESH geometry are converted. Splines of degree 1–32 use their stored WCS control points, knots and positive weights with bounded adaptive sampling; discontinuous knot spans stay separate. Fit-point-only splines fail explicitly. Object coordinate systems, extrusion thickness, nested/array INSERT blocks, block base points, layer visibility and ACI/true RGB/ByLayer/ByBlock colors are evaluated. Text/attributes, dimensions, leaders, tolerances, tables and viewport annotations are omitted from model geometry and framing. ACIS, hatches, nonzero line widths, fitted polylines, subdivision, MESH property overrides and unknown visible model entities fail explicitly. Nine tests pass, and five [Assimp DXF fixtures](https://github.com/assimp/assimp/tree/master/test/models/DXF), an [ezdxf authored spline fixture](https://github.com/mozman/ezdxf/blob/master/tests/test_02_dxf_graphics/test_226_spline.py), and its [nested-dimension drawing](https://github.com/mozman/ezdxf/blob/master/examples_dxf/dimension_in_nested_blocks.dxf) render successfully.

## DirectX

**Extensions:** `.x`.

**Coverage:** Subset: text/binary/MSZIP mesh scenes, static frames, bind geometry and colors.

DirectX X accepts text, binary 32/64-bit and MSZIP-compressed streams, including the rolling compression dictionary. Static Frame transforms, polygon meshes, material references and vertex colors are supported; left-handed Y-up coordinates become right-handed Z-up. Supported SkinWeights/header data are validated and the authored bind mesh is previewed without joint deformation. Optional normals/UVs/DeclData/FVFData are validated but not rendered; textures and later animation are omitted. Unknown required geometry fails. Five [Assimp X fixtures](https://github.com/assimp/assimp/tree/master/test/models/X), including binary, compressed and skinned bind geometry, render successfully.

## 3MF

**Extensions:** `.3mf`.

**Coverage:** Subset: core/production multipart meshes, placements, units, colors and packaged PNG/JPEG textures.

3MF loads core meshes, nested component instances, build transforms, base materials and optional material color groups. The [Production Extension 1.1.2](https://3mf.io/spec/production-v1-1-2/) permits the root model's components and build items to reference other model parts inside the package. These namespace-qualified `p:path` references are resolved through OPC model relationships; object IDs and material palettes stay local to each model part. Only the selected root build contributes geometry. Child builds are ignored, and child component references remain local as required by the extension. The six core length units normalize to millimeters, including translations in each referencing part's units; nested rotations and dimensionless scales are preserved.

Direct `texture2d`/`texture2dgroup` material resources load packaged PNG/JPEG images through the owning model part's typed OPC texture relationships. Part-local property IDs, object defaults and per-corner `p1`/`p2`/`p3` coordinates are preserved. Lower-left 3MF UVs convert to the renderer's image origin. Repeat, mirrored repeat and edge clamp are supported; `none` acts as clamp for these single-layer textures, and their alpha is ignored as specified by 3MF. Nearest and bilinear filtering operate in encoded sRGB before lighting in linear color; `auto` selects bilinear filtering. Shared image parts decode once across model parts.

Authored multipart fixtures exercise colliding IDs and palettes, nested placement, direct cross-part build references and mixed inch/centimeter units. Unchanged [Bambu Studio, OrcaSlicer, PrusaSlicer and 3MF Consortium packages](../tests/fixtures/formats/mesh_scene/README.md) add real exporter examples, with independent geometry, image, sampler, coordinate and source-hash checks. Tests reject missing relationships/parts/objects, ambiguous duplicate entries, malformed namespaces, cycles and excessive expansion. Limits cover 256 model parts, 512 MiB of aggregate consumed XML and encoded image data, five million source vertices/triangles/texture coordinates and output triangles, 100,000 resources/archive entries, one million references/instances and 256 levels of component nesting. Texture axes are limited to 8192 pixels, individual images to sixteen million pixels, and distinct images to sixty-four million pixels in aggregate before raster decoding.

Composite materials, layered multiproperties, physically based display properties and unknown required extensions remain unsupported. Remote model and texture resources are rejected. UUID traceability and slicer settings do not affect preview geometry; full manufacturing and print conformance remain unvalidated.

## USD

**Extensions:** `.usd`, `.usda`, `.usdc`, `.usdz`.

**Coverage:** Subset: local composed stages, static meshes/primitives/points/curves, instances, transforms and diffuse colors.

USD uses the pure Rust [openusd](https://github.com/mxpv/openusd) 0.8 importer and its published `openusd-schemas` geometry/material helpers for USDA, USDC and USDZ. Supported local sublayers, references, payloads, variants and native instances are composed. Authored polygons, points, common geometric primitives, PointInstancer prototypes/placements, sampled linear/cubic BasisCurves and rational NurbsCurves produce geometry. Earliest supported time samples, xform/reset stacks, visibility, purpose and Y/Z up axes are applied. Display colors and PreviewSurface diffuse colors, including face subsets, are rendered. Subdivision surfaces retain their authored polygon cage. Textures and custom shader appearance use the available diffuse fallback. USDC versions below 0.4, nested USDZ/point instancers, NURBS patches, skeletal deformation, volumes and unknown geometry schemas fail explicitly. Resources remain inside the input directory, combined layer reads are capped at 512 MiB and geometry/prim counts are bounded.

[NurbsCurves](https://openusd.org/release/api/class_usd_geom_nurbs_curves.html) retain unweighted control positions, independently authored orders, knot vectors, trimmed ranges and optional point weights. The reader evaluates rational B-spline centerlines with 24 intervals per nonempty trimmed knot span; full-multiplicity boundaries remain separate and order-1 spans become discrete points. Omitted weights are one. Finite curves with positive, negative, zero or mixed weights are supported, including removable endpoint and interior denominator zeros. For weights without a strict common sign, exact rational polynomials derived from the original binary attribute values cancel factors shared by all homogeneous components, then Sturm sequences certify the entire trimmed denominator interval. Genuine poles, including even roots between samples, fail explicitly. Schema-valid ranges extending beyond the active spline domain also fail explicitly. Widths do not produce ribbons or tubes. Numeric coordinates retain the existing USD policy, without applying `metersPerUnit`.

NURBS expansion is bounded across the stage and point-instancer prototype scenes: order at most 64, 250,000 nonempty spans, 5,250,000 samples, 100 million estimated evaluation operations, five million checked exact operations, and 8,192 bits per unreduced numerator or denominator. A shared 512 MiB budget charges NURBS attribute/sample storage, checked exact-arithmetic result payloads, and geometry-vector capacity throughout the stage. Ordinary meshes, points, BasisCurves and primitives, cached prototype scenes, and bulk instance copies all reserve bounded capacity before insertion. Reused capacity is charged once, and prototype charges remain in the stage budget when their local scene is dropped. The five-million-element scene limit is checked before output insertion. Normalized-span interpolation preserves samples even between adjacent floating-point knots; ill-conditioned parameter ratios use exact fallback, and mathematically constant controls retain a point without a geometry tolerance. Eight new corpus cases include one unchanged Apache-2.0 Maya exporter layer and equivalent authored USDA/USDC/USD/USDZ curves; independent closed-form geometry assertions qualify rational weights, mixed orders, trim intervals, discontinuities, placements and removable limits separately from image comparisons.

## VDB

**Extensions:** `.vdb`.

**Coverage:** Subset: scalar sparse 5/4/3 trees, half-float values, density/level-set surfaces and affine transforms.

OpenVDB reads file versions 222–224 with scalar float/double/int32/int64 5/4/3 trees, including half-float payloads. Active voxels, coarse tiles and negative inactive level-set interiors supply actual geometry. The first scalar field is selected, preferring a density field; density is max-pooled and level sets use their nonpositive interior, producing at most 32 cells per axis. Scale, translation, affine and unitary transforms preserve placement. Raw, zlib and supported Blosc1 blocks decode in Rust, including split streams, byte/bit shuffle, BloscLZ, LZ4, zlib and Zstandard. Vector/mask/point-data grids, instanced trees, older file versions, nonlinear frustum transforms and other Blosc codecs remain unsupported. Sparse output is capped at five million samples, 131,072 leaves and 256 MiB decoded scalar data. Four regressions check hollow density, reflected placement, full/half values, inactive backgrounds, codec layout and malformed blocks. Official [OpenVDB cube, sphere and smoke fixtures](https://www.openvdb.org/download/) render and were visually checked. The layout follows OpenVDB's own [compression](https://github.com/AcademySoftwareFoundation/openvdb/blob/master/openvdb/openvdb/io/Compression.h) and tree serializers; no native OpenVDB or Blosc dependency is used.

## IFC

**Extensions:** `.ifc`.

**Coverage:** Subset: Part 21 product placements, mapped/faceted/tessellated shapes, extruded/revolved profiles, primitives and bounded solid operations/openings.

IFC accepts IFC2x3 and supported IFC4-family Part 21 entities. Hierarchical local placements, mapped instances, mirroring, nonuniform scaling, SI/conversion units, diffuse styles and indexed face colors are applied. Faceted shells, planar faces and tessellated triangles/polygons include actual interior holes and `PnIndex` mapping. Arbitrary closed profiles with voids, rectangle/rounded/hollow rectangle, circle/hollow circle, ellipse and supported I-section profiles generate extrusions or revolutions; blocks, cylinders, cones and spheres are tessellated. Bounded BSP union/intersection/difference, planar half-space clipping and relational opening subtraction preserve solid cavities. Body representations take priority; space volumes and auxiliary axes/footprints/bounding boxes are omitted. Curved advanced B-reps, unsupported trimmed/spline curves and profiles, polygonally bounded half-spaces, tapered/sectioned/swept-disk solids, grid/linear placements and external/complex STEP entities fail explicitly. Texture sampling, transparency and appearance supplied only by material associations are omitted. Limits cap input at 512 MiB, entities at 500,000 and generated geometry at five million; individual solid operations cap polygons/triangles at 100,000, BSP depth at 256 and work at ten million. Nine regression tests and fourteen independent [buildingSMART examples](https://github.com/buildingSMART/IFC4.x-specification-models) verify extrusion, revolution, mappings, facets, colors, solid cavities and openings against the primary [IFC specification](https://standards.buildingsmart.org/IFC/DEV/IFC4_3/HTML/).

## Splat

**Extensions:** `.splat`.

**Coverage:** Subset: positions and RGB rendered as points.

Splat inputs retain stored centers and RGB and render them as points. Gaussian covariance, rotation and opacity compositing are outside the current preview policy.

## SPZ

**Extensions:** `.spz`.

**Coverage:** Subset: gzip versions 1–3 and Zstandard version 4, centers and colors rendered as points.

SPZ accepts gzip-compressed versions 1–3 and version-4 Zstandard attribute streams, preserving decoded centers, converting the stored coordinate convention and rendering the base spherical-harmonic color as RGB. Fully transparent centers are skipped. Point counts, compression output sizes, stream lengths and reserved header flags are checked. The preview uses at most one million uniformly selected centers; Gaussian rotation, covariance, opacity blending and higher-order view-dependent harmonics are not rendered. The binary conventions follow the primary [Niantic SPZ source](https://github.com/nianticlabs/spz).

## LAS

**Extensions:** `.las`, `.laz`.

**Coverage:** Subset: LAS point records, Rust LAZ decompression and RGB.

LAS/LAZ uses [las-rs](https://github.com/gadomski/las-rs) and [laz-rs](https://github.com/laz-rs/laz-rs) to decode supported point formats and decompress LAZ in Rust. RGB values are normalized from 16 bits; files using only 8-bit RGB retain those values. The declared point count must be between 1 and 20 million; deterministic uniform thinning caps previews at one million points. Truncated records and non-finite selected coordinates fail explicitly. Coordinates remain in the stored axis system, without CRS reprojection. Intensity/classification color maps and waveform rendering are not evaluated.

## BPF

**Extensions:** `.bpf`.

**Coverage:** Subset: versions 1–3, supported layouts, zlib, homogeneous transforms and RGB.

BPF versions 1–3 support the recognized point/dimension/byte layouts, uncompressed or zlib payloads, dimension offsets, homogeneous transforms and RGB fields.

See [survey-point conventions](#survey-point-conventions) for source conventions, coordinate units and shared limits.

## TerraScan BIN

**Extensions:** `.bin`.

**Coverage:** Subset: versions 20010712/20020715, scaled coordinates and optional RGB.

TerraScan supports versions 20010712 and 20020715 with stored scaling/origins and optional RGB.

See [survey-point conventions](#survey-point-conventions) for source conventions, coordinate units and shared limits.

## Optech CSD

**Extensions:** `.csd`.

**Coverage:** Subset: 69-byte records, range echoes, platform attitude and boresight geometry.

Optech CSD reads the supported 69-byte records, deriving up to four echoes from range, scan angle, platform position/attitude and boresight.

See [survey-point conventions](#survey-point-conventions) for source conventions, coordinate units and shared limits.

## PCD

**Extensions:** `.pcd`.

**Coverage:** Subset: ASCII, binary and LZF-compressed points with RGB.

PCD accepts ASCII, little-endian binary and LZF `binary_compressed` payloads, scalar XYZ fields and common packed/separate RGB fields.

## PTX

**Extensions:** `.ptx`.

**Coverage:** Subset: scans, positions, RGB and scan transforms.

PTX previews render scan points and RGB after applying their stored scan transforms. Empty scan points are excluded by the reader. The preview uses the shared point renderer.

## QFIT

**Extensions:** `.qi`.

**Coverage:** Subset: supported 10/12/14-word records and endianness, longitude/latitude/elevation points.

QFIT reads the supported 10/12/14-word records in either endianness and validates the history/data offset.

See [survey-point conventions](#survey-point-conventions) for source conventions, coordinate units and shared limits.

## SBET

**Extensions:** `.sbet`.

**Coverage:** Subset: little-endian 17-double trajectory records rendered as geographic points.

SBET reads complete little-endian 17-double records as trajectory points.

See [survey-point conventions](#survey-point-conventions) for source conventions, coordinate units and shared limits.

## SLPK

**Extensions:** `.slpk`.

**Coverage:** Subset: I3S point-cloud packages, hierarchy and Rust LEPCC XYZ/RGB decoding.

SLPK supports I3S PointCloud 1.6–1.9/2.0 packages, legacy and node-page hierarchies, resource IDs and leaf selection. ZIP/gzip and LEPCC XYZ/RGB streams are decoded in Rust, including supported raw, palette, constant and Huffman color layouts. Geographic WGS84/CGCS2000 points become a local tangent frame in meters; projected coordinates are retained. Mesh layers, terrain-relative elevations, unsupported geographic/custom-WKT systems and invalid hierarchies/checksums fail. Classification/stretch drawing styles are omitted; available RGB is rendered. A licensed [Esri I3S point-cloud sample](https://github.com/Esri/i3s-spec) is included under `tests/fixtures/slpk` with its notice. Aggregate decoded data and preview points are bounded.

## AMF

**Extension:** `.amf`. **Status:** Rust geometry reader; bounded XML/ZIP subset.

The reader loads UTF-8 AMF XML and a ZIP containing one regular `.amf` model member, with stored or deflated compression. It preserves indexed flat triangles from every supported object volume, validates references, and normalizes millimeter, meter, micron, inch and feet units to millimeters. Only top-level, unreferenced object/constellation definitions render; referenced definitions contribute their placed instances without an extra copy at the origin. Nested constellations retain translation and qualified single-axis rotations. Standard angles use degrees within ±360°; mixed-axis rotations within one instance fail explicitly. Constant sRGB material, object, volume and triangle colors follow that precedence from least to most specific and retain alpha through the shared renderer.

The [official ISO AMF schema](https://standards.iso.org/iso/52915/ISOASTM52915(2016)AMF.xsd) supplies the core fields and units. The separately qualified [Slic3r 1.3.1 exporter](https://github.com/slic3r/Slic3r/blob/026c1380e0ad12176dd2fc8cdf8f6f181deeb071/xs/src/libslic3r/IO/AMF.cpp) stores Z rotation in radians and adds dimensionless uniform scale. Documents identified by that application/version metadata use its convention, including positive uniform scale and Z rotations within one turn (approximately ±2π radians, allowing the exporter's numeric rounding). Other Slic3r rotation dialects and scale in standard AMF fail rather than guessing. Supported definitions use nonnegative integer IDs; unsupported namespaces, duplicated IDs or fields, missing references, cycles and invalid numeric fields fail.

Curved-patch vertex normals, edge tangents, vertex color gradients, textures, color/material expressions, composite materials, custom color profiles, void/support volumes and slicer modifier volumes are unsupported and rejected. Manufacturing metadata is not interpreted. This importer does not repair meshes or evaluate process settings, and does not claim full AMF specification or slicer-version compatibility.

The [unchanged original Slic3r XML export and compressed reference](../tests/fixtures/formats/amf/README.md) are separately licensed AGPL-3.0 assets with the full original license and source notice. Their XML payloads are byte-identical. Independent assertions check twelve triangles, placed bounds, surface area and signed volume; both encodings also have CLI PNG comparisons. The compressed reference is an upstream ZIP-reader test around exporter XML, rather than evidence that Slic3r wrote that ZIP container. Authored controls independently check units, individual rotation axes, nested placement, scale, color precedence and explicit malformed/unsupported errors.

Inputs and expanded XML are limited to 64 MiB, XML nodes to two million, scalar fields to 128 bytes, resource/instance records to 100,000, source vertices/source triangles/placed triangles to one million each, nested constellations to 64 levels, cumulative geometry accounting to 256 MiB and charged instance work to fifty million operations. ZIP paths are validated without writing extracted files. Multiple members, unsafe or nonregular members, unsupported compression and failed CRC/length validation are rejected.

## Lychee

**Extension:** `.lys`. **Status:** saved-PNG extraction and original stored-model geometry with qualified placements.

Automatic previews prefer a usable saved `preview.png`. When it is absent or unreadable, Catwalk renders the original stored model meshes. `--render-geometry` always selects those source models; supports, rafts, hollowing, holes and other print preparation are intentionally excluded. Preparation metadata and unreferenced derived meshes do not invalidate an otherwise valid addressed source model. The saved image preserves the prepared view and can be stale; it does not regenerate model geometry or preparation. `--extract-saved-preview` remains extraction only at the original image dimensions, returning code 3 when no saved carrier is present.

The shared owned reader validates Mango's ASAR/Pickle framing, exact JSON length and zero alignment padding, then checks every flat member name and nonoverlapping, source-bounded span. Member offsets are decimal strings relative to `8 + headerSize`, with checked integer sizes. Declared whole and contiguous block SHA-256 digests for each selected image, scene or source mesh are verified before decoding; optional integrity metadata can be absent. Arbitrary image signatures and unrelated note images are not selected. Malformed or unsupported containers and failed selected-member integrity checks return errors. A corrupt selected PNG can still fall back to valid independently addressed native scene/mesh members in automatic mode.

Native geometry accepts scene-state version **2** and stored mesh version **2** with the qualified 20-byte header `[2, 12, indexCount, coordinateScalarCount, 0]`, followed by little-endian `u32` triangle indices and float32 XYZ coordinates. Every stored coordinate must be finite, scalar/index counts describe complete triplets, the member length matches exactly, and every index is in range. Objects resolve their exact `properties.hash` source member; required missing external or library geometry errors rather than selecting the first or largest available mesh. Visible, on-scene file models are included, while hidden/off-scene and preparatory hole/operation objects are excluded. Shared source arrays are validated once and reused across independently placed occurrences.

Coordinates are already centered on import. Native placement applies component scale, world-axis X, Y and Z rotations in authored degrees, then XY translation and a Z translation that places the lowest transformed actual source vertex at `position.z`. The original `center`/`formerCenter` fields are not subtracted again. Scene producer strings are qualification metadata, not a replacement for schema-version checks. Native colors/materials, legacy ZIP `.lys`, `.lyt`, nested directory tables, linked/unpacked resources and other scene/mesh variants remain unsupported. The [official export documentation](https://doc.mango3d.io/doc/resin-documentation/resin-export/export-scene/) distinguishes self-contained `.lys` from `.lyt` scenes requiring original model files; Catwalk does not fetch those resources.

Two unchanged MIT-licensed Lychee **5.4.3** keyboard-case scenes qualify flat manifest **3.1.0** and the native stored encodings. Their exact original PNGs, source/pixel hashes and notice remain in the [saved-image corpus](../tests/fixtures/formats/lychee/README.md). The [native corpus](../tests/fixtures/formats/lychee_geometry/README.md) retains the author's original STL witnesses: all 114,840 source triangle-corner scalars match the stored indexed coordinates bitwise after the single recorded STL import-centering operation. Independent literal placed corners, bounds, surface areas and signed volumes check both models. Asymmetric authored controls cover nonuniform scale/noncommuting rotations, repeated meshes, visibility, ignored preparation, saved-image preference and absent/unreadable-image fallback. These fixtures do not establish compatibility with every Lychee release or stored-scene variant. Adapted public byte-transform constants retain the original [MIT notice](licenses/DF-PLUGIN-LYS-LICENSE), also included in the project license shipped with source and binary archives.

Primary inputs use the shared 512 MiB limit. The JSON index allows at most 2 MiB, 4096 flat members, eight nested levels, 50,000 cumulative values and 4096 encoded bytes per string. Saved PNGs are limited to 8 MiB encoded, 4096 pixels per axis, 16 million pixels and 64 MiB codec allocation. Native scene state is limited to 16 MiB, 64 MessagePack levels, one million cumulative values and 1 MiB UTF-8 strings; structural/count/allocation preflight precedes upstream borrowed-value decoding. Native association tables are limited to 10,000 objects, selected mesh members to 128 MiB, cumulative source vertices to five million, source indices to fifteen million and placed triangles to five million. Conservative source/metadata/temporary/reserved-output accounting is limited to 512 MiB and shared transform/decode/index work to fifty million operations. Duplicate decoded JSON/string map keys, unsafe aliases, malformed framing/spans, incomplete associations, invalid transforms, unsupported stored encodings and exceeded budgets fail explicitly. No desktop application, native dependency or subprocess converter is required.

## Autodesk Inventor

**Extensions:** `.ipt`, `.iam`. **Status:** Rust property-thumbnail extraction; qualified image carriers.

Geometry rendering for `.ipt` and `.iam` is unsupported in Catwalk. These readers extract an embedded image only.

The Rust extractor selects root-document CFB/OLE v3/v4 summary property sets by FMTID, prefers the native Inventor summary over the legacy summary, and validates the PID-17 `VT_CF` image. Saved PNG and supported DIB/BMP forms normalize to PNG; the source design remains untouched. A usable legacy summary can rescue an unusable native summary. This follows the structured-storage thumbnail mechanism described by [Autodesk ADN](https://images.autodesk.com/apac_japan_main/files/3538256_API_Thumbnail.pdf) and [Microsoft typed property values](https://learn.microsoft.com/en-us/openspecs/windows_protocols/MS-OLEPS/f122b9d7-e5cf-4484-8466-83f6fd94b3cc).

Regression tests cover CFB v3/v4 regular/mini streams, native/legacy selection, distractors, PNG precision/transparency, supported bitmap depths, malformed carriers, original FAT chain integrity and limits. Real legacy DIB/BMP carriers and full application-release compatibility remain unqualified. `extract_png` preserves native PNG bit depth; common thumbnail output uses RGBA8. Rust extraction bounds input at 512 MiB, summary/image payloads and output at 8 MiB, decoded images at 64 MiB and axes at 4096 pixels, with a cooperative five-second budget. Directory and miniFAT metadata are limited to 4 MiB each and the root mini-stream to 64 MiB. Catwalk's original [carrier fixture details](../tests/fixtures/saved-previews/autodesk/README.md) describe saved images without Inventor CAD geometry.

## LightBurn

**Extensions:** `.lbrn`, `.lbrn2`. **Status:** qualified saved-image extraction and fresh vector-outline rendering.

Fresh geometry support reads classic and compact paths, adaptive Beziers, shared vertex/primitive resources, nested affine groups, rectangles, rounded corners and ellipses. The view is XY. Text, bitmaps, laser fills, custom palettes and multipage layouts remain unsupported. Geometry input is limited to 64 MiB, XML nodes/shared resources/shapes/output segments to one million, and group nesting to 32. Nine no-thumbnail projects have independent geometry checks and PNG snapshots; see [geometry fixture details](../tests/fixtures/native_geometry/lightburn/README.md).

The [official file-management documentation](https://docs.lightburnsoftware.com/latest/Reference/FileManagement/) describes `.lbrn` as legacy and `.lbrn2` as the default; the [0.9.21 announcement](https://lightburnsoftware.com/blogs/news/lightburn-0-9-21-image-adjustment-window-image-compositing-measure-tool-new-file-format-memory-savings-and-much-more) introduced the newer format. The Rust reader accepts UTF-8, unnamespaced `LightBurnProject` XML with `FormatVersion` 0 or 1 and one direct-child `Thumbnail/@Source` containing a Base64 PNG. It preserves decoded RGBA pixels. Missing or empty previews return `None`; corrupt, ambiguous, namespaced and unsupported carriers error. DTDs are rejected.

Two licensed source/image pairs are retained as [saved-preview regression fixtures](../tests/fixtures/saved-previews/lightburn/README.md). Tests cover pixel preservation, absence, malformed carriers, limits, ambiguity and whitespace. These fixtures do not establish universal application-release compatibility.

Limits cap input at 64 MiB, XML nodes at 500,000, encoded previews at 12 MiB, PNG payloads at 8 MiB, axes at 8192 pixels and images at sixteen million pixels. Version 2.1 templates, 2.2 multi-page projects, other encodings/carriers remain unqualified.

## Blender

**Extension:** `.blend`. **Status:** qualified saved-image extraction and static mesh geometry rendering.

The fresh geometry reader uses BHead/SDNA layouts to load old and modern mesh arrays, active scene membership, static object/parent placements, collection instances, loose edges and vertices. Modifiers, shape keys, non-mesh surfaces, external libraries and material textures remain unsupported. Input and decompressed geometry are limited to 512 MiB, BHead blocks to two million, mesh arrays/output primitives to five million, and parent/collection nesting to 128. Six licensed no-thumbnail scenes from 2.52 through 5.2 have source integrity, measured topology/bounds/areas and PNG comparisons; see [geometry fixture details](../tests/fixtures/native_geometry/blend/README.md).

The Rust reader follows Blender's [thumbnail extractor](https://github.com/blender/blender/blob/main/source/blender/blendthumb/src/blendthumb_extract.cc): leading `REND` records followed by `TEST`, whose payload is two signed 32-bit dimensions and raw RGBA pixels. Exact payload length and dimensions are validated, and rows are flipped vertically for display. Historical 12-byte file headers with 20/24-byte blocks and new 17-byte headers with 32-byte blocks are supported; [Blender 5.0 introduced larger-buffer headers](https://developer.blender.org/docs/release_notes/5.0/core/). `GLOB`/`ENDB` without `TEST` returns `None`. Big-endian files, versions below 2.50 and malformed carriers error.

Extraction supports [raw, gzip and Zstandard containers](https://github.com/blender/blender/blob/main/source/blender/blenloader_core/intern/blo_core_file_reader.cc), including the multiple frames and trailing skippable seek table used by Blender's [Zstandard writer](https://github.com/blender/blender/blob/main/source/blender/blenloader/intern/writefile.cc). Limits cap input at 256 MiB, decompression at 512 MiB, previews at 64 MiB, axes at 8192 pixels, images at sixteen million pixels and Zstandard windows at 64 MiB.

Four licensed scenes with saved images and one gzip absence case are retained as [saved-preview regression fixtures](../tests/fixtures/saved-previews/blend_dwg/README.md). These include Blender 2.52, 2.78, 2.80 and 5.2 headers, raw/gzip compression and new-header seekable Zstandard compression. Authored fixtures exercise all three block layouts, other dimensions, alpha, compression and malformed/absent carriers. Retained real saved-preview files are 64-bit little-endian with 128 × 128 thumbnails; real 32-bit files and broader release/dimension coverage remain unqualified.

## Autodesk Fusion

**Extension:** `.f3d`. **Status:** implemented saved-PNG extraction; qualified archive subset.

Geometry rendering for `.f3d` is unsupported in Catwalk. This reader extracts an embedded PNG only.

Autodesk defines F3D as a single-workspace archive; distributed F3Z archives contain one or more F3D files and require separate qualification. See [Autodesk archive documentation](https://help.autodesk.com/view/fusion360/ENU/?caas=caas%2Fsfdcarticles%2Fsfdcarticles%2FHow-to-make-a-local-archive-back-up-file-in-Fusion-360.html).

The Rust reader selects exactly `FusionAssetName[Active]/Previews/small.png` in single-disk ZIP32 archives, validates manifest tag `3-2-0-0`, document type `FusionDocType` and the active-asset manifest, and checks selected-member lengths and CRCs. Stored, Deflate and single-frame Zstandard members are supported. An absent preview returns `None`; malformed or unsupported carriers error. Other image blobs are not candidates. Limits cap the source at 512 MiB, the ZIP directory at 4 MiB and 4096 entries, selected members at 8 MiB, decoded images at 64 MiB and image axes at 4096 pixels, with a cooperative five-second budget.

Authored regression tests exercise member selection, RGBA preservation, compression, absence, invalid carriers and limits. The format corpus includes Stored/Deflate carrier fixtures with intentionally different active-design and simulation images to check selection; these fixtures contain no Fusion geometry.

The `3-2-0-0` container tag does not establish Fusion application-version coverage. ZIP64, split archives, encrypted selected members, alternate manifests/assets and nested F3Z remain unsupported.

## AutoCAD drawings

**Extension:** `.dwg`. **Status:** qualified addressed DIB/PNG extraction and a native drawing geometry subset.

Fresh geometry decoding uses pinned Rust `acadrust=0.6.4` records and the in-memory [static DXF reader](#dxf), without converter subprocesses or native codecs. Incomplete or recovered decoding returns an error. Input and expanded DXF are limited to 512 MiB, and drawings to 500,000 decoded records. The record and expanded-DXF ceilings apply after upstream decoding and do not bound every decoder allocation; additional section/allocation/work bounds remain unfinished. Broader entities and ACIS solids remain unsupported. Five line drawings from 2000, 2004, 2007, 2013 and 2018 have independent coordinate checks; the 2018 fixture has no saved image and renders automatically. See [geometry fixture details](../tests/fixtures/native_geometry/dwg/README.md).

[LibreDWG documents drawing previews](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/doc/LibreDWG.texi) as optional BMP/WMF/PNG carriers; block and proxy previews must not be mistaken for the drawing thumbnail. Its [header schema](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/header.spec) places a little-endian thumbnail address at byte `0x0d` for R13-family and later containers. The supported carrier has a 16-byte sentinel, a size/count header and nine-byte type/address/size entries with absolute file offsets.

The [payload parser](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/dwg.c) distinguishes auxiliary header data (type 1), DIB bitmap data (type 2), WMF (type 3) and PNG (type 6). The Rust reader supports type-2 DIB and type-6 PNG, validates the declared pointer, begin/end sentinels, directory bounds and non-overlapping payload ranges, then selects the largest decoded raster candidate. A zero pointer returns `None`; malformed layouts, encrypted previews and WMF-only carriers error. Limits cap input at 256 MiB, the preview section at 64 MiB, axes at 8192 pixels and images at sixteen million pixels.

Saved-preview extraction accepts R13/R14/2000/2004/2007/2010/2013/2018 signatures AC1012/AC1014/AC1015/AC1018/AC1021/AC1024/AC1027/AC1032. [Carrier fixture details](../tests/fixtures/saved-previews/blend_dwg/README.md) describe authored AC1032 DIB, PNG and absent-preview carriers with pixel snapshots. These incomplete drawings test extraction; separate native drawing fixtures test geometry loading.

The saved-preview reader does not parse geometry, verify whole-file CRCs or recover previews through section maps. LibreDWG handles [R2004/R2010+ section maps](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/decode.c) and [R2007 page/section maps](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/decode_r2007.c) differently; addressed preview support does not establish those broader layouts. Other producers, service packs, encrypted or multiple-image previews and beta formats remain unqualified; AC1009, AC1013 and other signatures are explicitly rejected.

## Remaining compatibility work

1. Broaden CAD compatibility against independent files. Remaining cases include missing face pcurves, difficult trimming/tessellation, additional STEP assembly operators and independent exporter coverage, and additional Open CASCADE document versions and cached geometry. Preserve genuine topology, placement and curved geometry when adding these variants.
2. Broaden implemented encodings and geometry. Priorities include additional compression codecs, partial structured pieces, blanking, additional VTKHDF/Exodus/NetCDF schemas, broader CAD trimming and assembly operators, further USD/FBX/VRML/CityGML scene constructs, and supported scalar/DICOM/image codecs. Existing density and first-frame policies can stay fixed while missing valid encodings and geometry are added. Lidar schema/CRS coverage and CAD units also need corpus checks.
3. Expand independent format verification. Add examples from more exporters and encoding variants, and compare geometry bounds, supported static content and reference thumbnails. Include malformed/truncated files, external resources, transforms, holes/concave polygons, mixed blocks, large inputs and compressed variants. Current generated regression tests and independent reference files establish the documented subsets; they do not establish universal valid-file compatibility.

Remaining work covers valid encodings and geometry outside the documented subsets. The implementation uses no subprocess converters or C/C++ model/codec bindings.
