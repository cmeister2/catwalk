These independent decoder fixtures come from Google's Apache-2.0 licensed
[Draco testdata](https://github.com/google/draco/tree/eb7c044bfc75302b4d1e87f6bd9bb1f670c82343/testdata).

| Local name | Upstream name |
| --- | --- |
| cube.drc | cube_att.drc |
| cube-valence.drc | cube_att.obj.edgebreaker.cl4.2.2.drc |
| cube-sequential.drc | cube_att.obj.sequential.cl3.2.2.drc |
| cube-points.drc | cube_pc.drc |
| color-points.drc | pc_kd_color.drc |

The three unchanged mesh streams were compared byte for byte against Google
revision `eb7c044bfc75302b4d1e87f6bd9bb1f670c82343`. Their decoded geometry matches
the producer's [cube_att.obj](https://github.com/google/draco/blob/eb7c044bfc75302b4d1e87f6bd9bb1f670c82343/testdata/cube_att.obj):
eight unit-cube corners, 12 outward triangles and six authored axis directions.
Independent tests qualify all 12 triangle-normal associations, surface area 6
and signed volume 1. The legacy stream assigns unique ID zero to its position,
UV and normal attributes; its semantic mappings remain independently usable.

| Mesh fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| cube.drc | 301 | `327d95888f62a45f9579a9020701afc29d0d6c74e69406ed8b5154ba4a307f70` |
| cube-valence.drc | 199 | `f90c73777a7d0b2d74c4128364428014527f2fce54998d220be360a466ff7caa` |
| cube-sequential.drc | 220 | `a97cf55c8d41e3401564d47ffc636dbeb37d6859c59b16e55e71e6111657a433` |

Copyright 2016 Google Inc. Licensed under the Apache License, Version 2.0.
See [the upstream license](https://github.com/google/draco/blob/main/LICENSE).
