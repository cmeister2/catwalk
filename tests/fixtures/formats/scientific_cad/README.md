# Scientific and CAD fixtures

Authored examples are generated deterministically by `examples/fixtures/scientific_cad.rs` and are MIT licensed with Catwalk. The STEP assembly examples use actual B-rep surfaces, repeated product occurrences and nested representation mappings, with nonzero source-frame origins and rotated/translated components. Geometry checks compare their placed vertices independently; PNG snapshots check the resulting thumbnails.

`step-cadquery-placed-bodies.step` is an unmodified upstream test asset, renamed only for the corpus, from [CadQuery at commit 5f5bc62b255f62e3512105740025361b6c31eddd](https://github.com/CadQuery/cadquery/blob/5f5bc62b255f62e3512105740025361b6c31eddd/tests/testdata/red_cube_blue_cylinder.step). It retains its Apache-2.0 license in `step-cadquery-LICENSE`; source provenance and the exact SHA-256 are recorded in `step-cadquery-NOTICE.txt`. The file contains two placed bodies and a conversion-based metre context. It checks curved surfaces and millimeter unit conversion, without claiming an independently exported assembly hierarchy. STEP material colors are not currently rendered.

Regeneration uses Rust and retained fixture bytes; no CAD application, native writer or network is required. The separately licensed upstream asset is not relabeled as an authored fixture.
