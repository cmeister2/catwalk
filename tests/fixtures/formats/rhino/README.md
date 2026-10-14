# Rhino native geometry and saved viewport fixtures

These are unchanged example 3DM files distributed in the public McNeel openNURBS SDK, pinned to commit [`eb92af3ba1806b0a34a99aba0d3bda83e3d46083`](https://github.com/mcneel/opennurbs/tree/eb92af3ba1806b0a34a99aba0d3bda83e3d46083/example_files).

[sources.json](sources.json) records each upstream path, complete SHA-256, native saved-image dimensions and decoded RGBA SHA-256. No source archive was re-saved, synthesized or modified. The Rhino 3, 4, 5, 6, 7 and 8 files contain saved viewport images. The Rhino 2 DeformSpheres and openNURBS 6 `my_mesh` examples contain no addressed preview and exercise absence.

McNeel distributes the example files with its openNURBS SDK. The original repository [license notice](LICENSE-openNURBS.txt) and the official [unrestricted use and redistribution grant](LICENSE-openNURBS-permission.txt) are retained. The grant is published on the [openNURBS Initiative page](https://www.rhino3d.com/features/developer/opennurbs/), whose SDK download explicitly includes sample 3DM files. These sources and derived saved-image references retain McNeel attribution and are not relicensed as Catwalk-authored assets.

The PNG references were independently reconstructed outside Catwalk using Python `struct`, `zlib` and Pillow. The extractor walks the start comment and leading properties table, selects type `0x20008025`, reads its serialized 40-byte DIB header and native compressed-buffer fields, verifies the declared decoded length and buffer CRC, inflates the addressed anonymous child, then converts padded bottom-up BGR rows into RGBA. The original saved pixels are preserved; no geometry or new camera view is rendered. The checked-in RGBA digests make those independent pixels explicit. Six 256-pixel corpus baselines additionally check Catwalk's ordinary aspect-preserving canvas placement.

The carrier interpretation follows McNeel's pinned [`opennurbs_3dm_properties.cpp`](https://github.com/mcneel/opennurbs/blob/eb92af3ba1806b0a34a99aba0d3bda83e3d46083/opennurbs_3dm_properties.cpp), [`opennurbs_bitmap.cpp`](https://github.com/mcneel/opennurbs/blob/eb92af3ba1806b0a34a99aba0d3bda83e3d46083/opennurbs_bitmap.cpp), and [official thumbnail extraction guide](https://developer.rhino3d.com/en/guides/cpp/extracting-thumbnail-preview-images/). No arbitrary image-signature scan is used.

The saved-image cases continue to qualify document properties independently of geometry. The native geometry cases below additionally qualify freshly rendered geometry.

## Native geometry provenance and independent witnesses

[geometry-witnesses.json](geometry-witnesses.json) records exact archive SHA-256 values and measurements made independently with McNeel's `rhino3dm` Python distribution, version **8.17.0**. [qualify_geometry.py](qualify_geometry.py) reads each original through that SDK, recording native triangle/quad faces, vertex positions, curve samples, points and point clouds, units, visibility, B-rep vertices, native face-cache triangle counts, and block transforms, native mesh normals, and bottom/top profile samples for all seven V5 extrusion objects. It never invokes Catwalk or writes a source archive. The native runtime uses Rust and has no dependency on this Python SDK.

Three additional unchanged SDK examples come from the same pinned openNURBS revision and redistribution grant as the saved-image sources:

| Original upstream path | SHA-256 |
|---|---|
| `example_files/V6/my_curves.3dm` | `45a8afedd0518614e23d804f5d4370a1aa44cbbc832b7b524f4f8075c3c8fb37` |
| `example_files/V6/my_points.3dm` | `79c6119cd0cc61d9fbee1a7554e085358c4354ce925e812a5d44feeaf19f06cb` |
| `example_files/V6/my_trimmed_surface.3dm` | `39d357d8dcf774e8a7dff1ea59f6c6d6dbcb8b3d40e4ca4a64d4b606bee3f557` |

The existing original `my_mesh.3dm`, `v4_MechPartB.3dm` and `v5_extrusions.3dm` are independently qualified for geometry too. `my_mesh` has nine stored vertices, four triangles and one quad, no saved preview, and SDK-decoded source normals. `my_curves` declares inches; `my_points` declares metres and includes a point cloud; the trimmed surface is a triangular region on a larger supporting surface. The V4 mechanical part has complete native per-face render meshes. The V5 source includes seven native extrusion objects and ten separate source curves.

The `producer_*.3dm` files are small Catwalk-authored models written through the public `rhino3dm` 8.17.0 document and geometry APIs. [generate_geometry.py](generate_geometry.py) retains their authoring recipe. Its optional execution creates new UUIDs and timestamps and intentionally reauthors the fixtures; it is never part of test setup. Reauthoring requires rerunning the independent qualifier and reviewing every changed source hash and measurement. The committed original archives remain immutable during tests.

| Authored producer archive | Qualified native behavior |
|---|---|
| `producer_geometry_v5.3dm` through `producer_geometry_v8.3dm` | Native archive versions 50, 60, 70 and 80; a box B-rep without a stored mesh, a triangle/quad mesh, a line, rational circle and cubic NURBS curve, source centimetres, object colors, visible/hidden points. The physical box has surface area 6200 mm² and enclosed volume 30000 mm³. |
| `producer_instances.3dm` | One native triangle definition placed at two translated occurrences; definition geometry is excluded from scene roots, member object color is retained. |
| `producer_point.3dm` | A point-only document without triangles, curves or saved images. |
| `producer_surface.3dm` | A native free NURBS patch between two quadratic ribs; SDK samples establish its parameter domain and positions. |

`tests/rhino_geometry.rs` checks these independent measurements, exact hashes, original mesh triangle winding and shading normals, no-cache B-rep area and volume, physical unit normalization, trimmed area, SDK curve and patch samples, source extrusion profile coverage, native block placements, visibility and source preservation. It also renders the no-preview mesh headlessly, rejects incomplete/unsupported object families and truncated archives without replacing an existing output, and rejects sparse oversized sources before reading their payload.

## Runtime geometry scope and limits

The native reader uses pinned `cadmpeg-codec-rhino` 0.6.1 for bounded archive/chunk framing, CRC verification, typed native geometry, effective object/layer visibility and colors, physical units, and block expansion. It renders source meshes with their double-precision coordinates, native winding and shading normals; source points and point clouds; finite lines, conics, polylines, polycurves and NURBS curves; independent NURBS patches; and supported B-rep/extrusion boundaries. Geometry enters a Z-up scene in millimetres.

Complete B-rep render caches are admitted only when each native face has one nonempty, uniquely indexed, contiguous `.render-N` slot. Analysis caches cannot replace missing render slots or add duplicate output. Other supported exact boundaries pass through Catwalk's existing in-memory Rust STEP projection and CAD tessellator with its geometry, placement and numerical-report checks. Independently displayed curves and surfaces are retained while supporting carriers and point topology remain internal. Saved property images never supply geometry.

Curves use 64 subdivisions per nonempty nonlinear knot span; free NURBS patches use 32 per nonlinear span along each axis. Linear spans retain exact endpoints. These are bounded display approximations. Bodies without complete cached meshes must satisfy the shared CAD kernel's stricter supported topology and numerical limits. Unsupported carriers or omitted native geometry are reported explicitly. Text, annotations, unresolved inherited attributes, plugin geometry and other unimplemented object families prevent whole-scene geometry loading; the V2/V3 sphere documents include text dots and therefore remain saved-preview controls. A native reader does not imply complete Rhino modeling/API or arbitrary material/texture support.

The codec's service policy bounds the native input to 256 MiB, each expanded buffer to 256 MiB, materialized/retained decode storage to 512 MiB, typed entities to eight million, each collection to one million, recursion to 128 and decode work to 256 million units. Catwalk additionally bounds scene storage to 512 MiB, emitted geometry to five million primitives, copied native mesh vertices to five million, aggregate curve/surface evaluations to four million, curve nesting to 64, boundary graph nodes to one million and tessellated body count to 4096. Allocation accounting can reject a model before its primitive ceiling is reached. The explicit shared source-size guard rejects files above 512 MiB before codec admission.
