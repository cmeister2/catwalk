# F3D thumbnail format coverage

Catwalk has Rust thumbnail readers for every published F3D format family. Each importer supports the encoding and geometry described below; **full valid-file parity with F3D remains unfinished**. An extension listed as a subset means the stated encoding and geometry can be previewed, not that every valid file with that extension can be read.

The baseline below covers every reader and extension in the [F3D 3.5 supported-format table](https://f3d.app/docs/user/SUPPORTED_FORMATS/), checked on 11 October 2026. Individual F3D builds vary with their plugins; the baseline here includes the whole published table. Reader identifiers and extension aliases are compatibility targets, not promises made by the CLI.

The CLI recognizes **all 76 published extension aliases**, covering all 57 reader rows below. Every row has a Rust reader for actual geometry or image data. Recognizing an alias does not mean full support for every encoding, entity or scene feature in that format.

Each section corresponds to a reader row in the summary table. Extension aliases sharing one format remain together, and shared renderer or codec policies are linked from the relevant section.

The seven additional document extensions described below are outside the published F3D baseline. Fresh geometry readers support Blender, LightBurn and a DWG drawing subset. Inventor `.ipt`/`.iam` and Fusion `.f3d` support saved-image extraction only; their geometry rendering is unsupported in Catwalk. Saved-image extraction does not regenerate geometry or establish compatibility with every application release.

| F3D reader | Extensions | Current Rust preview coverage |
| --- | --- | --- |
| [VTKLegacy](#vtk-legacy) | `.vtk` | Subset: ASCII/big-endian binary, classic/version-5 topology, polygons/lines, linear and structured grid boundaries. |
| [VTKXMLVTP](#vtk-polydata-xml) | `.vtp` | Subset: ASCII/binary/appended XML, zlib/LZ4; polygons, strips, vertices and polylines. |
| [VTKXMLVTU](#vtk-unstructuredgrid-xml) | `.vtu` | Subset: ASCII/binary/appended XML, zlib/LZ4; linear cells and closed convex polyhedra. |
| [VTKXMLVTR](#vtk-rectilineargrid-xml) | `.vtr` | Subset: ASCII/binary/appended, zlib/LZ4; single-piece rectilinear grid boundary geometry. |
| [VTKXMLVTI](#vtk-imagedata-xml) | `.vti` | Subset: scalar image data, ASCII/base64/zlib, voxel density surface. |
| [VTKXMLVTS](#vtk-structuredgrid-xml) | `.vts` | Subset: ASCII/binary/appended, zlib/LZ4; single-piece structured grid boundary geometry. |
| [VTKXMLVTM](#vtk-multiblock-xml) | `.vtm` | Subset: nested multiblock hierarchy and supported relative child datasets. |
| [VTKHDF](#vtkhdf) | `.vtkhdf` | Subset: static polygon/linear-cell/convex-polyhedron meshes, structured grids, scalar images and local composite assemblies. |
| [ExodusII](#exodus-ii) | `.e`, `.ex2`, `.exo`, `.g` | Subset: CDF1/2/5 and NetCDF4 undeformed linear element-block boundary geometry. |
| [NetCDF](#netcdf) | `.nc`, `.cdf`, `.ncdf` | Subset: rectilinear 2D scalar sheets, 3D scalar density and first time sample; Exodus schema detection. |
| [PLYReader](#ply) | `.ply` | Subset: ASCII/binary polygons, standard edges and point geometry with RGB. |
| [STL](#stl) | `.stl` | Implemented: ASCII and binary triangle geometry. |
| [DICOM](#dicom) | `.dcm` | Subset: first scalar/RGB image frame with native, deflated, JPEG or RLE pixel decoding. |
| [Nrrd](#nrrd) | `.nrrd`, `.nhdr` | Subset: raw/ASCII/hex/gzip scalar density, attached or one detached payload. |
| [MetaImage](#metaimage) | `.mhd`, `.mha` | Subset: raw/ASCII/zlib scalar density, attached or one detached payload. |
| [TIFF](#tiff) | `.tif`, `.tiff` | Subset: 2D image preview; no 3D stack rendering. |
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
| [STEP](#step) | `.step`, `.stp`, `.stpnc`, `.p21`, `.210` | Subset: supported Part 21 B-rep shells, axis-based assemblies/mappings and length contexts tessellated in Rust. |
| [IGES](#iges) | `.iges`, `.igs` | Subset: supported manifold/constructive solids and independent trimmed/bounded sheets, curved surface tessellation. |
| [BREP](#brep) | `.brep` | Subset: Open CASCADE ASCII topology, supported analytic/NURBS geometry, occurrence placements and curved tessellation. |
| [XBF](#xbf) | `.xbf` | Subset: native binary XCAF static B-rep geometry, free shapes, instance placements and diffuse colors. |
| [Alembic](#alembic) | `.abc` | Subset: Ogawa/HDF5 first-sample meshes, subdivision cages, points, sampled curves, transforms, instances and RGB. |
| [OBJ](#obj) | `.obj` | Subset: static polygons, polylines and points, vertex RGB and local MTL diffuse colors. |
| [GLTF, GLTFDraco](#gltf) | `.gltf` | Subset: textured triangle/point/line scenes, alpha, normals, first poses, skins/morphs, GPU instances and Rust Draco/quantized decoding. |
| [GLB, GLBDraco](#glb) | `.glb` | Subset: textured triangle/point/line scenes, alpha, normals, first poses, skins/morphs, GPU instances and Rust Draco/quantized decoding. |
| [Draco](#draco) | `.drc` | Subset: sequential/Edgebreaker meshes and sequential/kd-tree point clouds, positions and colors. |
| [3DS](#3ds) | `.3ds` | Subset: static triangle meshes, diffuse colors and earliest-key object hierarchy. |
| [VRMLReader](#vrml) | `.wrl`, `.vrml` | Subset: VRML97 text, static shapes, lines, terrain, extrusions, DEF/USE, transforms and colors. |
| [FBX](#fbx) | `.fbx` | Subset: ASCII/binary static polygon scenes, weighted linear skins, progressive morphs, hierarchy, pivots, units, first keys and colors. |
| [COLLADA](#collada) | `.dae` | Subset: static indexed mesh scenes, instancing, TRS/matrices, units and up axes. |
| [OFF](#off) | `.off` | Subset: polygon geometry and colors. |
| [DXF](#dxf) | `.dxf` | Subset: ASCII/binary drawing faces, points, lines, sampled curves/splines, meshes and nested block instances. |
| [DirectX](#directx) | `.x` | Subset: text/binary/MSZIP mesh scenes, static frames, bind geometry and colors. |
| [3MF](#3mf) | `.3mf` | Subset: core/production multipart meshes, placements, units, colors and packaged PNG/JPEG textures. |
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

## Shared preview policies

The renderer uses an automatically fitted orthographic view, a depth buffer, simple lighting, colored triangles, points and line segments. Optional triangle appearances support base-color texture sampling, interpolated linear RGBA and normals, unlit shading, sidedness, alpha masks and per-sample depth-sorted transparency. glTF/GLB and textured 3MF populate these appearances; each reader retains its stated material subset. LightBurn drawings use an XY view. Static scene importers select the authored pose or earliest supported transform sample. Animation playback, authored camera views, scene lighting, full physically based materials, volume transfer functions and Gaussian opacity/covariance are outside the current preview policy. Supporting a first static view permits omission of interactive viewer features, but does not remove the need to parse each model format correctly.

All current importers ignore non-geometric fields that do not affect their supported static geometry. Exact encoding restrictions are enforced by each reader. New variants need representative fixtures and thumbnail checks before this matrix changes to implemented.

### Raster-image rendering

Raster images are decoded by the enabled Rust codecs, fitted to the requested canvas with aspect ratio preserved, resized with Lanczos filtering and converted to RGBA. Transparent output preserves supported source alpha. Decoding is limited to 256 MiB; primary files remain bounded by the shared 512 MiB input limit. These readers produce 2D previews rather than reconstructed model geometry.

### Scientific arrays and cell limits

XML mesh arrays accept ASCII, inline base64, base64 appended and raw appended bytes, either byte order, UInt32/UInt64 headers, zlib and LZ4 compression. Binary lengths, block headers, decompressed sizes, connectivity offsets and indices are checked, with an aggregate 512 MiB decoded-array limit.

Scalar color maps, higher-order cells, legacy polyhedron face streams, platform-dependent legacy `long` words and XML LZMA compression remain unsupported; mixed unsupported cell types fail explicitly. Thirteen regression tests include pixel-identical results across 36 binary XML encoding combinations and malformed payload checks. Thirteen independent fixtures from [Kitware VTK](https://github.com/Kitware/VTK/tree/master/Testing/Data) and [vtkio](https://github.com/elrnv/vtkio/tree/master/assets) verify legacy binary grids, inline/appended arrays, compression and polyhedra against the primary [VTK format specification](https://docs.vtk.org/en/latest/vtk_file_formats/index.html) and [VTK cell identifiers](https://github.com/Kitware/VTK/blob/master/Common/DataModel/vtkCellType.h).

### Scalar-volume rendering

Scalar volume previews extract exposed faces of actual foreground voxels. Values are max-pooled to at most 32 cells per axis so a small bright feature survives reduction. The threshold is halfway through the finite scalar range, using zero as the lower baseline when the range crosses zero. Uniform nonzero data fills the extent; uniform zero data has no foreground. Coordinates, spacing, origin, direction and reflected axes are preserved. This produces a bounded density surface, including interior cavity walls, rather than an empty volume's bounding box. It is a fixed preview policy rather than F3D volume-transfer-function parity. Scalar payloads are capped at 16,777,216 samples and 128 MiB decoded data.

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

**Coverage:** Subset: ASCII/big-endian binary, classic/version-5 topology, polygons/lines, linear and structured grid boundaries.

The legacy VTK reader accepts ASCII and standard big-endian binary geometry, classic cell lists and version-5 offsets/connectivity sections. POLYDATA supports polygons, triangle strips, vertices and polylines; linear UNSTRUCTURED_GRID and structured/rectilinear/structured-point boundaries are rendered.

Grid boundaries are extracted for tetrahedra, voxels, hexahedra, wedges and pyramids, removing shared faces. Vertex/polyvertex, triangle, triangle-strip, polygon, pixel and quadrilateral cells are also accepted. Polygons use concave-aware triangulation. Legacy polyhedron face streams remain unsupported.

See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for validation, shared limits and reference checks.

## VTK PolyData XML

**Extensions:** `.vtp`.

**Coverage:** Subset: ASCII/binary/appended XML, zlib/LZ4; polygons, strips, vertices and polylines.

PolyData XML preserves polygons, triangle strips, vertices and polylines. Polygons use concave-aware triangulation. See [scientific arrays and cell limits](#scientific-arrays-and-cell-limits) for supported encodings, compression, validation and reference checks.

## VTK UnstructuredGrid XML

**Extensions:** `.vtu`.

**Coverage:** Subset: ASCII/binary/appended XML, zlib/LZ4; linear cells and closed convex polyhedra.

Grid boundaries are extracted for tetrahedra, voxels, hexahedra, wedges and pyramids, removing shared faces. XML type-42 convex polyhedra require closed face topology. Vertex/polyvertex, triangle, triangle-strip, polygon, pixel and quadrilateral cells are also accepted. Polygons use concave-aware triangulation.

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

**Coverage:** Subset: static polygon/linear-cell/convex-polyhedron meshes, structured grids, scalar images and local composite assemblies.

VTKHDF accepts supported static version-1/2 PolyData, UnstructuredGrid, ImageData, RectilinearGrid and StructuredGrid schemas, and local MultiBlockDataSet/PartitionedDataSetCollection assemblies. Polygon/strip/polyvertex/polyline geometry and supported linear cells are rendered; convex type-42 polyhedra require closed, consistent face topology. Partition-local indices and offsets are validated; shared interior faces are removed across partitions and mixed cell kinds. Scalar ImageData uses the density policy above, including cell-centered arrays and direction matrices. Local assembly links and repeated blocks are followed with depth/cycle/count bounds. Temporal Steps, AMR/HyperTreeGrid, nonconvex polyhedra, higher-order cells, nonzero ghosts and other schemas fail. Thirteen container tests and four independent [Kitware VTKHDF fixtures](https://github.com/Kitware/VTK/tree/master/Testing/Data/vtkHDF) cover real polyhedra, composite spheres, PolyData and Mandelbrot image density. The schema follows the [VTKHDF specification](https://docs.vtk.org/en/latest/vtk_file_formats/vtkhdf_file_format/vtkhdf_specifications.html).

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

**Coverage:** Subset: 2D image preview; no 3D stack rendering.

TIFF produces a 2D image preview. Reconstructing volume geometry from a TIFF stack remains unsupported.

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

**Coverage:** Subset: supported Part 21 B-rep shells, axis-based assemblies/mappings and length contexts tessellated in Rust.

STEP uses `truck-stepio` 0.3 and Truck's robust constrained surface tessellation. Supported open/closed B-rep shells become actual triangles, including supported curved surfaces. Full spheres and ring tori with vertex-loop bounds use direct analytic surface evaluation, retaining placement and checking that the loop vertex lies on the surface. The reader checks resolved topology, face/boundary preservation and per-face tessellation so incomplete geometry fails explicitly. Constrained tessellation uses 0.3% of each shell's estimated diameter from vertices and sampled boundary curves, with a kernel-tolerance floor; full analytic sphere/torus surfaces use a 64-by-64 parameter grid.

Supported shape representations retain nested `REPRESENTATION_MAP`/`MAPPED_ITEM` instances with 3D axis frames, and assembly occurrences linked through `NEXT_ASSEMBLY_USAGE_OCCURRENCE`, `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` and axis-based `ITEM_DEFINED_TRANSFORMATION` relationships. Product ownership determines transformed relationship direction; plain shape relationships group equivalent representations in either endpoint order and reject ambiguous links between different products. Nonzero mapping origins are subtracted before rotation and translation. Shell meshes are cached and reused for each placed occurrence. Representation length contexts, including SI prefixes and supported simple/complex conversion-based units, normalize geometry and frame translations to millimeters. Bare shells without representation contexts retain the existing coordinate convention. Unowned shells, missing placements, unsupported required representation items, cycles and excessive hierarchy expansion fail explicitly.

Tests generate a translated rectangular solid, cylinder, sphere and torus, verify surface areas/bounds, render a thumbnail and exercise all five extension aliases. Assembly regressions independently compare every placed vertex for repeated and nested parts, reversed relationship direction, mixed metre/millimeter contexts and inch conversion. [Authored assembly snapshots and a retained Apache-2.0 CadQuery export](../tests/fixtures/formats/scientific_cad/README.md) provide portable image comparisons. The independent export contains two placed bodies with curved geometry and conversion-based metre units; it does not establish real-exporter assembly hierarchy coverage. A wide, thin cylinder checks that boundary-curve extent controls tessellation density. Pinned upstream [OCCT cube, cone, cylinder, sphere and torus fixtures](https://github.com/ricosjp/resources4truck/tree/9bf9de1426c5fc2f2b8d63f501dca0d3c53ebd91/step) also render successfully.

Plain context-dependent identity occurrences without transformed relationships, additional transformation operators, unsupported entities, faceted schemas outside Truck's supported topology, unsupported/mixed vertex-loop topology, horn/spindle tori, wire-only files and non-geometric STEP-NC/AP210 contents remain unsupported. Materials and angular-unit conversion are not evaluated. Entity count, topology/representation depth, face and output triangle limits apply. Alias recognition does not establish full schema parity.

## IGES

**Extensions:** `.iges`, `.igs`.

**Coverage:** Subset: supported manifold/constructive solids and independent trimmed/bounded sheets, curved surface tessellation.

IGES supports the kernel's manifold B-rep and constructive solids and independent trimmed/bounded sheets, converting recognized global units to millimeters. The [IGES specification](https://nvlpubs.nist.gov/nistpubs/Legacy/IR/nbsir86-3359.pdf) defines global parameter 19 as the distance below which authored coordinates should be coincident. Its declared resolution is read with Hollerith strings/custom delimiters and converted to millimeters, then retained through import, bounds and tessellation. Positive resolutions up to 0.1 mm are accepted; omitted resolution retains the kernel default. Fixed-column sequences, directory/parameter ownership and termination counts are checked before import. Recognized logical groups retain their owned solid/sheet geometry; physically subordinate surfaces retain their geometric owner. A project-owned adapter builds independent trimmed faces through the published kernel geometry/topology APIs and retains every authored face in a compound without sewing or inventing connectivity. Ordinary manifold/constructive solids and subfigure instances use the unmodified published IGES reader. Repeated geometry is bounded at two million expanded numeric parameters and 100,000 topology nodes before allocation. Type-142 boundaries use their authored 3D trim where supplied, while owned alternate parameter-space curve definitions are validated and accounted for; independent/unowned curves remain explicit errors. A regression compares authored, imported and rendered face counts, area and bounds, and rejects unowned or malformed alternate definitions. Genuine boundary ranges crossing singular chart poles are split at verified surface points using the public curve projector before chart construction; a hand-authored great-circle hemisphere regression preserves both sphere poles, one complete face, analytic area and bounds. Geometry omissions and approximation warnings fail explicitly; non-geometric drafting metadata can be omitted. Bare curves/untrimmed surfaces and unsupported entity forms remain outside this subset.

The original OCCT [bearing.iges](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/iges/bearing.iges) retains all 213 authored trimmed faces and imports 3703 triangles without geometry warnings; ordinary sewing had returned zero shells. `hammer.iges` still fails a projected pcurve fit at approximately 0.689 mm, well beyond its declared 0.001 mm precision; geometry compromises remain explicit errors. Broader OCCT/IGES compatibility remains unfinished.

See [IGES and BREP tessellation](#iges-and-brep-tessellation) for the shared Rust kernel, deflection policy, bounds and regression checks.

## BREP

**Extensions:** `.brep`.

**Coverage:** Subset: Open CASCADE ASCII topology, supported analytic/NURBS geometry, occurrence placements and curved tessellation.

BREP reads genuine Open CASCADE ASCII placement/geometry/topology tables and supports the kernel's analytic/NURBS geometry. A Catwalk loader preserves all three authored analytic frame axes, including left-handed chart parameterizations, by rebuilding analytic objects through public geometry constructors; cylinder/native-circle regressions check parametric points, area and placement. Shape occurrence locations are applied. Rigid curve/surface carrier placements, including reflections, are restored from native metadata and materialized into actual analytic/NURBS geometry before tessellation. Equivalent native placements share transformed surface identities. A pcurve carrier is composed with its edge occurrence and matched to its face's composed carrier, preserving separately placed boundary edges. Scaled carriers fail explicitly; occurrence scaling remains supported. Translation/reflection, repeated carriers, opposing edge/carrier placements and separate occurrence regressions check exact area and bounds. As specified by [OCCT BRep_Tool](https://occt3d.com/dev/doc/refman/html/class_b_rep___tool.html), omitted planar pcurves are generated through exact closed-form projection; boundaries are first proved coplanar from analytic geometry or positive-weight spline control points. Off-plane boundaries and omitted pcurves on curved surfaces fail. Cached polygon/triangle tables are validated then discarded; faces are remeshed from their surfaces and trimming curves. Valid legacy fused index/continuity tokens are separated. Unsupported cached-data layouts and geometry records fail. Files beginning with the bare CASCADE topology header are also accepted. Compounds preserve free 3D curves and isolated vertices alongside faces.

Original OCCT [Room.brep](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/occ/Room.brep), [face1.brep](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/occ/face1.brep) and [solid.brep](https://github.com/Open-Cascade-SAS/OCCT/blob/master/data/occ/solid.brep) import with 5, 285 and 972 triangles respectively. Other checked OCCT files still fail missing pcurves or the bounded mesh deflection check; `Ball.brep` reaches genuine face tessellation but still exceeds that numeric tolerance.

See [IGES and BREP tessellation](#iges-and-brep-tessellation) for the shared Rust kernel, deflection policy, bounds and regression checks.

## XBF

**Extensions:** `.xbf`.

**Coverage:** Subset: native binary XCAF static B-rep geometry, free shapes, instance placements and diffuse colors.

XBF decodes genuine little-endian `BINFILE`/BinXCAF documents, including supported version-6–11 native shape tables and version-12 inline shape records. Analytic curves/surfaces, supported spline geometry, topology and document free-shape selection are interpreted before Rust tessellation; the file is not treated as ASCII BREP. Native backward references, hierarchy placements, rigid curve/surface carrier placements, mirroring and root-instance uniform scaling are preserved. Carrier geometry is actually transformed before tessellation because the kernel's ASCII bridge does not retain those placements. Diffuse RGB references and supported per-face colors are rendered. Input/conversion size, hierarchy depth, geometry counts and output are bounded. External parts, big-endian documents, unknown required sections/geometry, polygon-only or cached-triangulation representations and scaled curve/surface carriers fail explicitly. Shader styles, textures, alpha and unit metadata are omitted. Six regression tests check native serialization, actual area/bounds/colors, transformed shared face/pcurve geometry, point tables, mirroring and malformed input. Independent [F3D XBF fixtures](https://github.com/f3d-app/f3d/tree/master/testing/data) `f3d.xbf` and `xcaf-colors.xbf`, plus the [OCCT test assembly](https://github.com/a-betenev/opencascade-dataset/tree/master/xbf) `as1-no-ass-name.xbf`, render successfully. Some other real XBF documents still fail explicitly when the CAD kernel lacks required pcurves or cannot meet its bounded preview tessellation tolerance.

## Alembic

**Extensions:** `.abc`.

**Coverage:** Subset: Ogawa/HDF5 first-sample meshes, subdivision cages, points, sampled curves, transforms, instances and RGB.

Alembic reads Ogawa version-1 archives and supported HDF5 archives in Rust, selecting sample zero from each supported property. Polygon meshes, subdivision control cages with holes, points and sampled linear/cubic curves produce geometry. Transform operations, inheritance resets, nested object instances, visibility and supported RGB/RGBA arbitrary geometry parameters are applied; face/vertex colors become diffuse triangle or point colors. Authored coordinates are retained. Subdivision evaluation, curve widths, shader materials, alpha blending and later samples are omitted. NURBS patches, rational or variable-order curves and unknown/custom geometry schemas fail explicitly; layered archives are not composed. Node depth, counts, numeric sample sizes and aggregate decoded bytes are bounded. Nine regression tests and eight independent geometry archives from [Alembic](https://github.com/alembic/alembic/tree/master/prman/Tests/testdata) and [Pixar OpenUSD](https://github.com/PixarAnimationStudios/OpenUSD/tree/dev/pxr/usd/plugin/usdAbc/testenv) check Ogawa/HDF5 meshes, colors, instancing, first poses and curves against the primary [Alembic storage and geometry implementations](https://github.com/alembic/alembic/tree/master/lib/Alembic).

## OBJ

**Extensions:** `.obj`.

**Coverage:** Subset: static polygons, polylines and points, vertex RGB and local MTL diffuse colors.

OBJ loads static positions, faces, polylines and explicit points, with concave-aware triangulation. Positive and negative polyline indices, including vertex/texture-coordinate references, retain each consecutive segment. Unreferenced vertices are not added to a mesh or wire preview. Vertex RGB and optional local MTL diffuse `Kd` colors influence the thumbnail; texture coordinates, texture images and imported normals do not. Free-form curve and surface declarations fail explicitly rather than substituting their control points for geometry.

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

**Coverage:** Subset: sequential/Edgebreaker meshes and sequential/kd-tree point clouds, positions and colors.

Standalone Draco accepts sequential and Edgebreaker meshes, including valence and supported legacy streams, and sequential/kd-tree point clouds. Positions and mapped numeric colors are rendered; limits cap decoded geometry at five million points/faces and expanded attributes at 512 MiB. Generated tests and ten [Google Draco reference fixtures](https://github.com/google/draco/tree/main/testdata) exercise different encodings.

## 3DS

**Extensions:** `.3ds`.

**Coverage:** Subset: static triangle meshes, diffuse colors and earliest-key object hierarchy.

3DS reads binary triangle meshes, per-face diffuse colors, master scale and the earliest object translation/rotation/scale keys. Editor vertices, inverse mesh matrices, pivots, instance/dummy nodes, node-ID parent hierarchies and mirrored transforms are accounted for. Camera/light ancestors and the earliest camera roll are evaluated for mesh-child placement; their views and illumination are not rendered. Textures, material alpha, smoothing groups and later animation keys are omitted. Morph/visibility tracks, prescale, unlinked parent axes, legacy editor or parent-name hierarchies and unknown geometry chunks fail explicitly. The conventions are checked against the primary [Assimp](https://github.com/assimp/assimp/tree/master/code/AssetLib/3DS) and [Blender](https://github.com/blender/blender-addons/blob/main/io_scene_3ds/import_3ds.py) importers; six upstream Assimp models also render successfully.

## VRML

**Extensions:** `.wrl`, `.vrml`.

**Coverage:** Subset: VRML97 text, static shapes, lines, terrain, extrusions, DEF/USE, transforms and colors.

VRML accepts VRML97/V2.0 text with `DEF`/`USE`, groups, transforms, Anchor/Collision children, the selected Switch choice and the first LOD level. IndexedFaceSet, IndexedLineSet, PointSet, ElevationGrid, Extrusion, Box, Sphere, Cylinder and Cone produce geometry. Extrusion evaluates the specified spine-aligned frames, scales, orientations, open/closed cross-sections and spines, coincident spine sections, and concave caps. Diffuse surface materials, emissive line/point materials, and authored face/vertex colors are evaluated; vertex colors are averaged for the preview's flat triangle/line colors. Center, rotation, scale orientation and Y-up conversion are applied. Nine geometry tests and seven primary [Web3D sourcebook examples](https://www.web3d.org/x3d/content/examples/Vrml2Sourcebook/) pass, including terrain, colored lines, a torus and twisted/concave extrusions; [Assimp's Wuson fixture](https://github.com/assimp/assimp/tree/master/test/models/WRL) also renders. VRML1, prototypes, scripts, Inline, Billboard, Text and unknown scene/geometry nodes fail explicitly. Textures, camera/lighting nodes and event evaluation are omitted from the static preview.

## FBX

**Extensions:** `.fbx`.

**Coverage:** Subset: ASCII/binary static polygon scenes, weighted linear skins, progressive morphs, hierarchy, pivots, units, first keys and colors.

FBX accepts ASCII, including supported legacy version-6 inline meshes, and binary 7.x with 32/64-bit node records and zlib arrays. Static polygons, points, lines, instance hierarchies, transform pivots, pre/post rotations, geometric transforms, all three supported scale-inheritance modes, units, up axes, visibility and first transform keys are evaluated. Linear/rigid skins use authored mesh-to-bone bind matrices, current bone hierarchies and genuine control-point weights; Normalize and TotalOne link modes retain uninfluenced points and geometric offsets. Sparse blend shapes use default DeformPercent or the earliest authored key values, interpolate progressive full weights, support negative/extrapolated weights and accumulate parallel channels before skinning. Recognized Blender painted target weights are applied per vertex. Diffuse materials, per-face assignments and the first vertex-color layer influence the preview. Unknown deformers, dual-quaternion/hybrid skins, additive/associate-model skin modes, multiple skins on one mesh, constraints, NURBS, spherical rotations and layered controlling curves fail explicitly. Deformation work is bounded to five million control points/influences/offsets, with ID, index, weight and affine-matrix validation. Texture images, imported normals and later animation are omitted; embedded image byte metadata can be parsed without sampling those textures. Sixteen unit tests include analytic weighted positions and compressed/uncompressed binary deformation regressions. Independent [Assimp FBX fixtures](https://github.com/assimp/assimp/tree/master/test/models/FBX) cover rigid scenes plus skeleton animation and huesitos skins; [ufbx fixtures](https://github.com/ufbx/ufbx/tree/master/data) cover half-skinned Blender geometry, static blend shapes, Maya skinned pivots, transformed skin and progressive morphs. Every emitted vertex from seven skin/morph fixtures matched an independent ufbx static/earliest-sample evaluation within 2.2e-14 metres; the reference harness is only a development check, with no native dependency in Catwalk.

## COLLADA

**Extensions:** `.dae`.

**Coverage:** Subset: static indexed mesh scenes, instancing, TRS/matrices, units and up axes.

COLLADA accepts version 1.4/1.5 static indexed triangles, polygons, polylist, triangle fans and strips; float-array accessors, repeated geometry/node instances, nested affine matrices/TRS, global unit scaling, X/Y/Z up axes and diffuse material colors are evaluated. The importer validates source indices, accessor ranges and instance cycles. Skin/morph controllers, lines, polygon holes, spline/BREP geometry, `lookat`/`skew`, references to other documents and local asset coordinate-system overrides fail explicitly. Textures and animations are not evaluated. Matrix XML is read row by row, as illustrated in the [Khronos COLLADA specification](https://www.khronos.org/files/collada_spec_1_4.pdf).

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

USD uses the pure Rust [openusd](https://github.com/mxpv/openusd) 0.6 importer for USDA, USDC and USDZ. Supported local sublayers, references, payloads, variants and native instances are composed. Authored polygons, points, common geometric primitives, PointInstancer prototypes/placements and sampled linear/cubic BasisCurves produce geometry. Earliest supported time samples, xform/reset stacks, visibility, purpose and Y/Z up axes are applied. Display colors and PreviewSurface diffuse colors, including face subsets, are rendered. Subdivision surfaces retain their authored polygon cage. Textures and custom shader appearance use the available diffuse fallback. USDC versions below 0.4, nested USDZ/point instancers, NURBS, skeletal deformation, volumes and unknown geometry schemas fail explicitly. Resources remain inside the input directory, combined layer reads are capped at 512 MiB and geometry/prim counts are bounded.

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

Autodesk defines F3D as a single-workspace archive; distributed F3Z archives contain one or more F3D files and require separate qualification. Fusion's `.f3d` is unrelated to the F3D viewer whose format list defines the main matrix. See [Autodesk archive documentation](https://help.autodesk.com/view/fusion360/ENU/?caas=caas%2Fsfdcarticles%2Fsfdcarticles%2FHow-to-make-a-local-archive-back-up-file-in-Fusion-360.html).

The Rust reader selects exactly `FusionAssetName[Active]/Previews/small.png` in single-disk ZIP32 archives, validates manifest tag `3-2-0-0`, document type `FusionDocType` and the active-asset manifest, and checks selected-member lengths and CRCs. Stored, Deflate and single-frame Zstandard members are supported. An absent preview returns `None`; malformed or unsupported carriers error. Other image blobs are not candidates. Limits cap the source at 512 MiB, the ZIP directory at 4 MiB and 4096 entries, selected members at 8 MiB, decoded images at 64 MiB and image axes at 4096 pixels, with a cooperative five-second budget.

Authored regression tests exercise member selection, RGBA preservation, compression, absence, invalid carriers and limits. The format corpus includes Stored/Deflate carrier fixtures with intentionally different active-design and simulation images to check selection; these fixtures contain no Fusion geometry.

The `3-2-0-0` container tag does not establish Fusion application-version coverage. ZIP64, split archives, encrypted selected members, alternate manifests/assets and nested F3Z remain unsupported.

## AutoCAD drawings

**Extension:** `.dwg`. **Status:** qualified addressed DIB/PNG extraction and a native drawing geometry subset.

Fresh geometry decoding uses pinned Rust `acadrust=0.6.3` records and the in-memory [static DXF reader](#dxf), without converter subprocesses or native codecs. Incomplete or recovered decoding returns an error. Input and expanded DXF are limited to 512 MiB, and drawings to 500,000 decoded records. The record and expanded-DXF ceilings apply after upstream decoding and do not bound every decoder allocation; additional section/allocation/work bounds remain unfinished. Broader entities and ACIS solids remain unsupported. Five line drawings from 2000, 2004, 2007, 2013 and 2018 have independent coordinate checks; the 2018 fixture has no saved image and renders automatically. See [geometry fixture details](../tests/fixtures/native_geometry/dwg/README.md).

[LibreDWG documents drawing previews](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/doc/LibreDWG.texi) as optional BMP/WMF/PNG carriers; block and proxy previews must not be mistaken for the drawing thumbnail. Its [header schema](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/header.spec) places a little-endian thumbnail address at byte `0x0d` for R13-family and later containers. The supported carrier has a 16-byte sentinel, a size/count header and nine-byte type/address/size entries with absolute file offsets.

The [payload parser](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/dwg.c) distinguishes auxiliary header data (type 1), DIB bitmap data (type 2), WMF (type 3) and PNG (type 6). The Rust reader supports type-2 DIB and type-6 PNG, validates the declared pointer, begin/end sentinels, directory bounds and non-overlapping payload ranges, then selects the largest decoded raster candidate. A zero pointer returns `None`; malformed layouts, encrypted previews and WMF-only carriers error. Limits cap input at 256 MiB, the preview section at 64 MiB, axes at 8192 pixels and images at sixteen million pixels.

Saved-preview extraction accepts R13/R14/2000/2004/2007/2010/2013/2018 signatures AC1012/AC1014/AC1015/AC1018/AC1021/AC1024/AC1027/AC1032. [Carrier fixture details](../tests/fixtures/saved-previews/blend_dwg/README.md) describe authored AC1032 DIB, PNG and absent-preview carriers with pixel snapshots. These incomplete drawings test extraction; separate native drawing fixtures test geometry loading.

The saved-preview reader does not parse geometry, verify whole-file CRCs or recover previews through section maps. LibreDWG handles [R2004/R2010+ section maps](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/decode.c) and [R2007 page/section maps](https://github.com/LibreDWG/libredwg/blob/a3d51f13f7afc143c2adb87d4b0e858e24cdac95/src/decode_r2007.c) differently; addressed preview support does not establish those broader layouts. Other producers, service packs, encrypted or multiple-image previews and beta formats remain unqualified; AC1009, AC1013 and other signatures are explicitly rejected.

## Remaining compatibility work

1. Broaden CAD compatibility against independent files. Remaining cases include missing face pcurves, difficult trimming/tessellation, additional STEP assembly operators and independent exporter coverage, and additional Open CASCADE document versions and cached geometry. Preserve genuine topology, placement and curved geometry when adding these variants.
2. Broaden implemented encodings and geometry. Priorities include additional compression codecs, partial structured pieces, blanking, additional VTKHDF/Exodus/NetCDF schemas, broader CAD trimming and assembly operators, further USD/FBX/VRML/CityGML scene constructs, and supported scalar/DICOM/image codecs. Existing density and first-frame policies can stay fixed while missing valid encodings and geometry are added. Lidar schema/CRS coverage and CAD units also need corpus checks.
3. Establish parity with a pinned F3D build and corpus. Record its reader list, gather valid examples for every reader and encoding, and compare geometry bounds, supported first-frame content and reference thumbnails. Include malformed/truncated files, external resources, transforms, holes/concave polygons, mixed blocks, large inputs and compressed variants. Current generated regression tests and independent reference files establish the documented subsets; they do not establish universal valid-file equivalence.

Matching all 76 aliases is necessary but insufficient for F3D file compatibility. Remaining work covers valid encodings and geometry outside the documented subsets. The implementation uses no subprocess converters or C/C++ model/codec bindings.
