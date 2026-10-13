# Official Rhino saved viewport fixtures

These are unchanged example 3DM files distributed in the public McNeel openNURBS SDK, pinned to commit [`eb92af3ba1806b0a34a99aba0d3bda83e3d46083`](https://github.com/mcneel/opennurbs/tree/eb92af3ba1806b0a34a99aba0d3bda83e3d46083/example_files).

[sources.json](sources.json) records each upstream path, complete SHA-256, native saved-image dimensions and decoded RGBA SHA-256. No source archive was re-saved, synthesized or modified. The Rhino 3, 4, 5, 6, 7 and 8 files contain saved viewport images. The Rhino 2 DeformSpheres and openNURBS 6 `my_mesh` examples contain no addressed preview and exercise absence.

McNeel distributes the example files with its openNURBS SDK. The original repository [license notice](LICENSE-openNURBS.txt) and the official [unrestricted use and redistribution grant](LICENSE-openNURBS-permission.txt) are retained. The grant is published on the [openNURBS Initiative page](https://www.rhino3d.com/features/developer/opennurbs/), whose SDK download explicitly includes sample 3DM files. These sources and derived saved-image references retain McNeel attribution and are not relicensed as Catwalk-authored assets.

The PNG references were independently reconstructed outside Catwalk using Python `struct`, `zlib` and Pillow. The extractor walks the start comment and leading properties table, selects type `0x20008025`, reads its serialized 40-byte DIB header and native compressed-buffer fields, verifies the declared decoded length and buffer CRC, inflates the addressed anonymous child, then converts padded bottom-up BGR rows into RGBA. The original saved pixels are preserved; no geometry or new camera view is rendered. The checked-in RGBA digests make those independent pixels explicit. Six 256-pixel corpus baselines additionally check Catwalk's ordinary aspect-preserving canvas placement.

The carrier interpretation follows McNeel's pinned [`opennurbs_3dm_properties.cpp`](https://github.com/mcneel/opennurbs/blob/eb92af3ba1806b0a34a99aba0d3bda83e3d46083/opennurbs_3dm_properties.cpp), [`opennurbs_bitmap.cpp`](https://github.com/mcneel/opennurbs/blob/eb92af3ba1806b0a34a99aba0d3bda83e3d46083/opennurbs_bitmap.cpp), and [official thumbnail extraction guide](https://developer.rhino3d.com/en/guides/cpp/extracting-thumbnail-preview-images/). No arbitrary image-signature scan is used.

These fixtures qualify document-property previews only. They do not establish native 3DM mesh, NURBS, B-rep, annotation, instance, material or plugin geometry support.
