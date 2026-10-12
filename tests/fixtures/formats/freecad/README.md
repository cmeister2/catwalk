# Original FreeCAD reference documents

These 26 `.FCStd` files are unchanged original F3D test fixtures from
[`f3d-app/f3d` at `efb898d0943b4d73b8ef707fb333292493151ef8`](https://github.com/f3d-app/f3d/tree/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data).
The native documents declare schema 4 / file version 1 and contain original
OCCT ASCII V1 BREP shapes. Ordinary examples record FreeCAD 0.21; `bracket_1x`
records FreeCAD 1.1. No FreeCAD binary BREP export is represented here.

The original corpus includes real saved documents and upstream malformed or
modified controls. Filenames beginning `malformed_`, plus `broken_links`,
`corrupt_brp`, `cyclic_groups`, `invalid_content`, and `malformed_gui`
exercise admission/error behavior. `no_version` qualifies the primary reader's
legacy appearance fallback when the application version is absent. They are not all untouched
application exports. Saved PNGs can be reused across different documents and
are tested separately from freshly rendered native geometry.

The complete original [BSD-3-Clause license](LICENSE.md) and
[dataset-license mapping](DATA_LICENSES.md) are retained byte-for-byte.
The mapping states that datasets outside its named exceptions use the F3D
BSD-3-Clause license; none of these FCStd filenames is a listed exception.
Upstream licensing sources are
[LICENSE.md](https://github.com/f3d-app/f3d/blob/efb898d0943b4d73b8ef707fb333292493151ef8/LICENSE.md) and
[DATA_LICENSES.md](https://github.com/f3d-app/f3d/blob/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/DATA_LICENSES.md).

`tests/freecad.rs` pins the full source bytes, uses independent literal native
box corners/bounds and planar area/volume checks, checks selected objects and
face-position colors, and distinguishes saved PNG extraction from geometry.
Its additional ZIP/XML/list controls are authored in memory at test time and
are clearly separate from the original upstream files. They reuse licensed
native members to test ownership, placement, corruption and resource bounds;
the original files are never rewritten.

## Original-byte inventory

Each link addresses the original Git LFS media object at the pinned commit.

| Original filename | Bytes | SHA-256 |
| --- | ---: | --- |
| [arch_floor.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/arch_floor.FCStd) | 6030 | `4207b1ddba0fe46ce17c885d153c2b11ec07737fec6e2c0cc0c184737858a5c6` |
| [bracket_1x.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/bracket_1x.FCStd) | 9743 | `42ba1f1d5dcb8f476cd09021120d8fae818ea6c4162d15e85006738d1e2c3fa4` |
| [broken_links.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/broken_links.FCStd) | 8524 | `c92bb55e07a11f963647a071b5a827fbaa39f99e69372cfb23dfb2ffbe89a93b` |
| [colored_visibility.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/colored_visibility.FCStd) | 7282 | `f95408d17f86c20b740839ca5503d74762785e63c7d3559827c7ed5a4b30070e` |
| [corrupt_brp.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/corrupt_brp.FCStd) | 5325 | `77217f79eff5c1c03f1ddf66344ea3ec148d26bb0c4b0ddc20f327357e94e866` |
| [cyclic_groups.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/cyclic_groups.FCStd) | 6600 | `40bdabb89a03ae5a839440428ad1d2cb7b825b7fd18bebdfd872070cfe0f449a` |
| [draft_wireframe.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/draft_wireframe.FCStd) | 7332 | `f99cf3e1822495059817fe4aef2d862112505338a6d904a53593454ee969715f` |
| [face_colors.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/face_colors.FCStd) | 5647 | `20a1f2e604abd022e497fe44854b6e60443ede69f5e7156be1a854583b177567` |
| [gui_without_view_providers.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/gui_without_view_providers.FCStd) | 2966 | `353b1ffcc41d8528ad30290ab8338b3541d659381f90ec8cb72e3dcc577efd52` |
| [headless.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/headless.FCStd) | 3158 | `fd15ed96ae0becbcc7c3ebd8df7bdb0359bd5167173f7c7c2650d188a2b830f3` |
| [hidden_container.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/hidden_container.FCStd) | 4362 | `38942232df7288c214b87d6ddc28bae9d41836a98b5caa0b29a4b2ffcd0ca00a` |
| [invalid_content.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/invalid_content.FCStd) | 190 | `3c4f15612918060ed6231cd69173ba87892a1b23337c1781d879b2693ad3edc2` |
| [legacy_visibility.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/legacy_visibility.FCStd) | 6671 | `cbf92504cc84fa8a0b9bf8ae8067c4a35d23f0ae4ae6f5d6c5017612084378c0` |
| [link.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/link.FCStd) | 6734 | `2cf08804b1102ab29100feaa53723ed9db4af14f740700db8beb09c22ff632f5` |
| [link_array.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/link_array.FCStd) | 4454 | `ef1bfd1c07a557be5da555ca273f1db5a66de2b1c5bfe64eb39f603594d0dea0` |
| [link_array_transform.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/link_array_transform.FCStd) | 4529 | `1bbc816a9763b87cfef23f35de333d80ddd3d951117200485200f7b06931ef61` |
| [link_external.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/link_external.FCStd) | 3035 | `8aa6aa78fc22620c9a9d21c9660e76842d6309b7d6bdd5a5a56e978f8cc8370b` |
| [link_transform.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/link_transform.FCStd) | 3165 | `eed63e674233e45293629ec844bcdd9ac6e1dec372cae25bee39d4799ef31017` |
| [malformed_elements.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/malformed_elements.FCStd) | 6915 | `2025b55f4185100749840811336dad19f01a6f62334c7fa58cd3e29e43aa6f6c` |
| [malformed_gui.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/malformed_gui.FCStd) | 5235 | `e6083d149b901e2bf519c73876eb42b4516ffa2a5dce1f3c859bc0123f20fc73` |
| [malformed_no_objectdata.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/malformed_no_objectdata.FCStd) | 6505 | `1af013a5e748be7ae68d50a93df19709d7dd1ef0b9f5a6812a9ae6d1b65a3251` |
| [malformed_no_objects.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/malformed_no_objects.FCStd) | 6506 | `d05c0a8c3bbc2420b2d3ac833f7b670cb21e8724bfdced77ebfb348f1e236a90` |
| [no_version.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/no_version.FCStd) | 5784 | `bd0b33ff3cb7464b0ae394d1a1f711a7ff1cf9b629a37f32a992fe9136a800dc` |
| [part_container.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/part_container.FCStd) | 7174 | `65c19806aa367049bffafbfd5705721a8ba476d851be22817cd101b8caeb882b` |
| [partdesign_body.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/partdesign_body.FCStd) | 9733 | `fb2f45af0a9fb488ee315b0995a46ff834c0f4a395bf3caf2451ef3f6aea4158` |
| [transparency.FCStd](https://media.githubusercontent.com/media/f3d-app/f3d/efb898d0943b4d73b8ef707fb333292493151ef8/testing/data/transparency.FCStd) | 8771 | `8d989b597a447ed232bbaadd63eef7160cc7213068a350e0d7fca9d9aefff6af` |

## Qualified geometry and limits

The original controls cover visibility, legacy GUI fallback, placed parts, local
links, arrays, headless documents, aggregate/body result selection and legacy or
1.1 appearance lists. Tests inspect actual world geometry rather than accepting
a nonempty render or a saved screenshot as native-geometry evidence.

The original `bracket_1x` tube shell visits the outer cylinder, top, bottom and
inner cylinder in that order. Its material list assigns red, green, blue at half
opacity, and yellow respectively. The tests associate those values with the
actual surface positions and retain the original shell occurrence order.

The original `partdesign_body` tip unites a 10 × 10 × 10 box in the positive
quadrant with a cylinder of radius 3 and height 20 at the origin. The native
chart exposes the lower 270° and upper 360° of the cylinder wall. Its analytic
area is `540 + 118.5π` and signed volume is `1000 + 157.5π`; a separate 4 × 4 × 4
side box adds area 96 and volume 64. Tests retain exact selection and placement
checks and allow 2% relative error for these curved, tessellated measurements.

External links, subelement links, scale extensions, inline shape carriers and
FreeCAD binary BREP carriers require separately qualified support. Unsupported
selected geometry and malformed ownership fail explicitly rather than yielding
a partial local scene. General FreeCAD feature-tree recomputation, textures and
specular material semantics are outside this stored-shape preview subset.
