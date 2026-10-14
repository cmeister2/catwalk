# Original 3ds Max geometry fixtures

These `.max` files and their independently exported `.babylon` geometry are
unchanged files from [BabylonJS/Exporters](https://github.com/BabylonJS/Exporters)
at commit `231c4f720609d7e93999e43dcd6865388d89b117`. The upstream Apache-2.0
license is retained as `LICENSE-BabylonJS.md`; exact original paths, URLs and
SHA-256 digests are recorded in `provenance.json`. The test exports were produced
by 3ds Max's Babylon exporter, not by Catwalk or its MAX parser.

`SimpleTriangle.max` contains an Editable Mesh with one indexed triangle,
translation and 100x scale. `Instances.max` contains three scene occurrences
sharing one native Box primitive, at x=0, 2 and 4. `DoubleSidedMaterial.max`
contains an Editable Poly quad with nonidentity Euler rotation, plus camera and
helper objects. Its exporter duplicates the surface for the two-sided material;
the native source has one quad, yielding two geometric triangles.

The importer reads addressed OLE Scene and ClassDirectory3 streams, resolves
native class identities and ordered/keyed scene references, and emits visible
Editable Mesh triangles, Editable Poly faces with authored diagonals (or planar
faces without stored diagonals) and constant unsegmented
Box primitives. Scene-node ancestors and object-offset pivot transforms are
applied, including anisotropic scale orientation and mirrored winding. Qualified
controllers are native PRS, Position XYZ, Euler XYZ, Scale XYZ, Bezier Float and
constant Bezier Scale. A sole Bezier Float key exactly at time zero is supported.
Coordinates remain in the scene's authored Z-up system units.

Unsupported procedural objects, modifier stacks, transform-controller families,
multiple/nonzero-time transform keys, segmented Box primitives and nonplanar
polygons without authored diagonals return errors. Appearance currently uses the native node's wire color;
material shaders and textures are not imported. The native geometry API does
not read saved-image streams. The existing saved-thumbnail API remains available.

Budgets: 256 MiB source, 64 MiB per selected encoded/decoded stream, 1 million
chunks cumulatively, nesting/ancestor depth 64, 1 million vertices per mesh,
1 million final placed triangles, polygon degree 4096, and a 30-second deadline.
Gzip native streams are bounded and checked for trailing data. Compound-file
tables are opened through the strict `cfb` reader before selected streams load.

Native layout facts were checked against Autodesk's
[ILoad](https://help.autodesk.com/cloudhelp/2025/ENU/MAXDEV-CPP-API-REF/class_i_load.html)
and controller documentation, the original producer bytes, and the independent
[Ryzom native MAX storage implementation](https://github.com/ryzom/ryzomcore/tree/core4/nel/tools/3d/pipeline_max).
The Catwalk implementation is original Rust code; no upstream parser code is
copied or executed at runtime.
