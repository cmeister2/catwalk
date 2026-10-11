# ogeom

A B-rep solid modelling kernel written from scratch in Rust. It does not wrap
or link any other CAD kernel.

- Guide: <https://gilbertorconde.github.io/ogeom-rs/>
- API reference: <https://gilbertorconde.github.io/ogeom-rs/api/ogeom/>
- Changes: [`CHANGELOG.md`](CHANGELOG.md)

## What it does

| Area | Included |
|---|---|
| Modelling | primitives, extrusions, revolutions, sweeps, lofts, booleans, fillets and chamfers, offsets and shells, drafts, face removal |
| Geometry | lines, conics, B-splines and NURBS, elementary and freeform surfaces, curve and surface intersection |
| Analysis | volume, area, mass properties, distances, validity checks, healing |
| Meshing | tessellation at a stated deflection, per-face meshes that agree along shared edges |
| Drawings | hidden line removal, sections, DXF output |
| Exchange | STEP (assemblies, colours, PMI, views) and IGES in both directions; STL, OBJ, PLY, glTF, 3MF, VRML, DXF, and a native format |
| Meshes to solids | `solid_from_mesh` turns a triangle mesh into a solid, merging flat regions into planar faces and rebuilding cylinders, cones, spheres and tori |

## Scope and completeness

The target is parity with the modelling modules of the established reference
kernel, plus turning meshes into solids. [`docs/SCOPE.md`](docs/SCOPE.md)
defines the boundary.

[`docs/PARITY.md`](docs/PARITY.md) checks every public class of those modules
against 98 named capabilities. Each verdict cites the code and tests that back
it, and the build fails if the two drift apart. Rows marked `partial` state
their limits.

When the kernel cannot do something, it returns an error that says so. It does
not return a guessed result.

## Crates

| Crate | Contents |
|---|---|
| `ogeom` | the public API, re-exporting everything below |
| `ogeom-core` | arenas, identity, errors, tolerances, predicates |
| `ogeom-math` | points, vectors, transforms, B-spline basis, solvers |
| `ogeom-geom` | curves and surfaces |
| `ogeom-topo` | the B-rep data model |
| `ogeom-algo` | construction, measurement, classification, mesh to solid |
| `ogeom-mesh` | tessellation |
| `ogeom-intersect` | curve and surface intersection |
| `ogeom-bool` | booleans and face removal |
| `ogeom-heal` | healing and simplification |
| `ogeom-fillet` | fillets and chamfers |
| `ogeom-offset` | offsets, shells, sweeps, lofts, drafts |
| `ogeom-hlr` | hidden line removal and drawings |
| `ogeom-io` | file formats |
| `ogeom-doc` | assemblies, colours, names, PMI |

`tools/` holds a command-line tool, a software renderer, benchmarks and the
parity scripts. `outside/` holds working code that is out of scope (constraint
solving, feature recognition, picking) in a separate workspace.

## Design

- A shape is a handle: a topology node, a chain of placements and an
  orientation. The same node can be placed many times without copying.
- Every entity keeps its own tolerance, which only grows.
- Every entity records where it came from, so references survive a rebuild.
- Every operation returns a history of what became of its inputs.

[`docs/DATA_MODEL.md`](docs/DATA_MODEL.md) has the details.

## Building

```sh
cargo build --workspace
cargo test --workspace
```

The full check that CI runs (formatting, lints, tests, docs, the guide and the
parity audit):

```sh
bash tools/check.sh   # needs mdbook: cargo install mdbook --locked
```

## License

MIT or Apache-2.0, at your option.
