//! Curved fixed-node VTK cells, cross-encoding witnesses and an unchanged Kitware model.

use base64::{Engine, engine::general_purpose::STANDARD};
use catwalk3d::Result;
use flate2::{Compression, write::ZlibEncoder};
use hdf5_writer::{AttributeBuilder, DatasetBuilder, Hdf5Builder, WriteOptions};
use std::{fmt::Write as _, fs, io::Write, path::Path};

/// Corpus subdirectory containing the retained and authored curved-cell inputs.
const DIR: &str = "vtk_quadratic";
/// Authored geometry uses the project's MIT license, separately from the retained Kitware asset.
const AUTHORED: &str = "Author-created deterministic Rust fixture; MIT, matching the project license. Role-ordered curved fixed-node VTK cells and exact Float64 cross-encoding geometry; fixture details in vtk_quadratic/README.md.";
/// Exact original dataset, repository revision, external content identity and retained copyright.
const ORIGINAL: &str = "Unmodified Kitware VTK Testing/Data/quadraticTetra01.vtu, resolved from the SHA-512 pointer at commit 4b0325a981d1f5cfad9d4da4909a8230881b2bf4. BSD-3-Clause; original VTK-Copyright.txt retained unchanged. SHA-256 6bebfc84f0c342a867e6c50f849aa532f075d99b6ea980c0140966fb96d520ad. Contains one curved quadratic tetrahedron and two curved quadratic surface triangles; scalar arrays are not preview colors. https://github.com/Kitware/VTK/blob/4b0325a981d1f5cfad9d4da4909a8230881b2bf4/Testing/Data/quadraticTetra01.vtu.sha512";

/// One role-ordered authored cell with an isolated image case.
struct Cell {
    /// Standard fixed-node VTK cell identifier.
    kind: u8,
    /// Stable cell-name suffix shared by input and image identifiers.
    name: &'static str,
    /// Short human-readable label shown by the example gallery.
    title: &'static str,
    /// Literal physical nodes in the standard role order, including bowed extra nodes.
    points: &'static [[f64; 3]],
}

/// Literal curved nodes for VTK type 21, quadratic edge.
const QUADRATIC_EDGE: &[[f64; 3]] = &[[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [1.5, 0.9, 0.45]];

/// Literal curved nodes for VTK type 22, quadratic triangle.
const QUADRATIC_TRIANGLE: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, -0.35, 0.65],
    [1.75, 1.25, 0.65],
    [-0.35, 1.0, 0.65],
];

/// Literal curved nodes for VTK type 23, quadratic quad.
const QUADRATIC_QUAD: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, -0.5, 0.6],
    [3.5, 1.0, 0.6],
    [1.5, 2.5, 0.6],
    [-0.5, 1.0, 0.6],
];

/// Literal curved nodes for VTK type 24, quadratic tetra.
const QUADRATIC_TETRA: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [1.799_999_999_999_999_8, -0.199_999_999_999_999_96, -0.25],
    [1.799_999_999_999_999_8, 1.2, -0.25],
    [-0.299_999_999_999_999_8, 1.2, -0.25],
    [-0.299_999_999_999_999_8, -0.199_999_999_999_999_96, 1.5],
    [1.799_999_999_999_999_8, -0.199_999_999_999_999_96, 1.5],
    [-0.299_999_999_999_999_8, 1.2, 1.5],
];

/// Literal curved nodes for VTK type 25, quadratic hexahedron.
const QUADRATIC_HEXAHEDRON: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [3.0, 0.0, 2.5],
    [3.0, 2.0, 2.5],
    [0.0, 2.0, 2.5],
    [1.5, -0.399_999_999_999_999_9, -0.5],
    [3.599_999_999_999_999_6, 1.0, -0.5],
    [1.5, 2.4, -0.5],
    [-0.599_999_999_999_999_6, 1.0, -0.5],
    [1.5, -0.399_999_999_999_999_9, 3.0],
    [3.599_999_999_999_999_6, 1.0, 3.0],
    [1.5, 2.4, 3.0],
    [-0.599_999_999_999_999_6, 1.0, 3.0],
    [-0.599_999_999_999_999_6, -0.399_999_999_999_999_9, 1.25],
    [3.599_999_999_999_999_6, -0.399_999_999_999_999_9, 1.25],
    [3.599_999_999_999_999_6, 2.4, 1.25],
    [-0.599_999_999_999_999_6, 2.4, 1.25],
];

/// Literal curved nodes for VTK type 26, quadratic wedge.
const QUADRATIC_WEDGE: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [3.0, 0.0, 2.5],
    [0.0, 2.0, 2.5],
    [1.7, -0.266_666_666_666_666_6, -0.5],
    [1.7, 1.133_333_333_333_333_3, -0.5],
    [-0.399_999_999_999_999_9, 1.133_333_333_333_333_3, -0.5],
    [1.7, -0.266_666_666_666_666_6, 3.0],
    [1.7, 1.133_333_333_333_333_3, 3.0],
    [-0.399_999_999_999_999_9, 1.133_333_333_333_333_3, 3.0],
    [-0.399_999_999_999_999_9, -0.266_666_666_666_666_6, 1.25],
    [3.8, -0.266_666_666_666_666_6, 1.25],
    [-0.399_999_999_999_999_9, 2.533_333_333_333_333, 1.25],
];

/// Literal curved nodes for VTK type 27, quadratic pyramid.
const QUADRATIC_PYRAMID: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, 1.0, 2.5],
    [1.5, -0.399_999_999_999_999_9, -0.199_999_999_999_999_96],
    [3.599_999_999_999_999_6, 1.0, -0.199_999_999_999_999_96],
    [1.5, 2.4, -0.199_999_999_999_999_96],
    [-0.599_999_999_999_999_6, 1.0, -0.199_999_999_999_999_96],
    [
        0.450_000_000_000_000_2,
        0.300_000_000_000_000_04,
        1.549_999_999_999_999_8,
    ],
    [2.55, 0.300_000_000_000_000_04, 1.549_999_999_999_999_8],
    [2.55, 1.7, 1.549_999_999_999_999_8],
    [0.450_000_000_000_000_2, 1.7, 1.549_999_999_999_999_8],
];

/// Literal curved nodes for VTK type 28, biquadratic quad.
const BIQUADRATIC_QUAD: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, -0.5, 0.6],
    [3.5, 1.0, 0.6],
    [1.5, 2.5, 0.6],
    [-0.5, 1.0, 0.6],
    [1.5, 1.0, 1.3],
];

/// Literal curved nodes for VTK type 29, triquadratic hexahedron.
const TRIQUADRATIC_HEXAHEDRON: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [3.0, 0.0, 2.5],
    [3.0, 2.0, 2.5],
    [0.0, 2.0, 2.5],
    [1.5, -0.399_999_999_999_999_9, -0.5],
    [3.599_999_999_999_999_6, 1.0, -0.5],
    [1.5, 2.4, -0.5],
    [-0.599_999_999_999_999_6, 1.0, -0.5],
    [1.5, -0.399_999_999_999_999_9, 3.0],
    [3.599_999_999_999_999_6, 1.0, 3.0],
    [1.5, 2.4, 3.0],
    [-0.599_999_999_999_999_6, 1.0, 3.0],
    [-0.599_999_999_999_999_6, -0.399_999_999_999_999_9, 1.25],
    [3.599_999_999_999_999_6, -0.399_999_999_999_999_9, 1.25],
    [3.599_999_999_999_999_6, 2.4, 1.25],
    [-0.599_999_999_999_999_6, 2.4, 1.25],
    [-1.5, 1.0, 1.25],
    [4.5, 1.0, 1.25],
    [1.5, -1.0, 1.25],
    [1.5, 3.0, 1.25],
    [1.5, 1.0, -1.25],
    [1.5, 1.0, 3.75],
    [1.5, 1.0, 1.25],
];

/// Literal curved nodes for VTK type 30, quadratic linear quad.
const QUADRATIC_LINEAR_QUAD: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, -0.5, 0.8],
    [1.5, 2.5, 0.8],
];

/// Literal curved nodes for VTK type 31, quadratic linear wedge.
const QUADRATIC_LINEAR_WEDGE: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [3.0, 0.0, 2.5],
    [0.0, 2.0, 2.5],
    [1.7, -0.266_666_666_666_666_6, -0.5],
    [1.7, 1.133_333_333_333_333_3, -0.5],
    [-0.399_999_999_999_999_9, 1.133_333_333_333_333_3, -0.5],
    [1.7, -0.266_666_666_666_666_6, 3.0],
    [1.7, 1.133_333_333_333_333_3, 3.0],
    [-0.399_999_999_999_999_9, 1.133_333_333_333_333_3, 3.0],
];

/// Literal curved nodes for VTK type 32, biquadratic quadratic wedge.
const BIQUADRATIC_QUADRATIC_WEDGE: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [3.0, 0.0, 2.5],
    [0.0, 2.0, 2.5],
    [1.7, -0.266_666_666_666_666_6, -0.5],
    [1.7, 1.133_333_333_333_333_3, -0.5],
    [-0.399_999_999_999_999_9, 1.133_333_333_333_333_3, -0.5],
    [1.7, -0.266_666_666_666_666_6, 3.0],
    [1.7, 1.133_333_333_333_333_3, 3.0],
    [-0.399_999_999_999_999_9, 1.133_333_333_333_333_3, 3.0],
    [-0.399_999_999_999_999_9, -0.266_666_666_666_666_6, 1.25],
    [3.8, -0.266_666_666_666_666_6, 1.25],
    [-0.399_999_999_999_999_9, 2.533_333_333_333_333, 1.25],
    [2.0, -0.666_666_666_666_666_6, 1.25],
    [2.0, 1.333_333_333_333_333_5, 1.25],
    [-1.0, 1.333_333_333_333_333_5, 1.25],
];

/// Literal curved nodes for VTK type 33, biquadratic quadratic hexahedron.
const BIQUADRATIC_QUADRATIC_HEXAHEDRON: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, 0.0, 2.5],
    [3.0, 0.0, 2.5],
    [3.0, 2.0, 2.5],
    [0.0, 2.0, 2.5],
    [1.5, -0.399_999_999_999_999_9, -0.5],
    [3.599_999_999_999_999_6, 1.0, -0.5],
    [1.5, 2.4, -0.5],
    [-0.599_999_999_999_999_6, 1.0, -0.5],
    [1.5, -0.399_999_999_999_999_9, 3.0],
    [3.599_999_999_999_999_6, 1.0, 3.0],
    [1.5, 2.4, 3.0],
    [-0.599_999_999_999_999_6, 1.0, 3.0],
    [-0.599_999_999_999_999_6, -0.399_999_999_999_999_9, 1.25],
    [3.599_999_999_999_999_6, -0.399_999_999_999_999_9, 1.25],
    [3.599_999_999_999_999_6, 2.4, 1.25],
    [-0.599_999_999_999_999_6, 2.4, 1.25],
    [-1.5, 1.0, 1.25],
    [4.5, 1.0, 1.25],
    [1.5, -1.0, 1.25],
    [1.5, 3.0, 1.25],
];

/// Literal curved nodes for VTK type 34, biquadratic triangle.
const BIQUADRATIC_TRIANGLE: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, -0.35, 0.65],
    [1.75, 1.25, 0.65],
    [-0.35, 1.0, 0.65],
    [0.9, 0.6, 1.3],
];

/// Literal curved nodes for VTK type 35, cubic line.
const CUBIC_LINE: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [1.0, 0.9, 0.7],
    [2.0, 0.9, -0.7],
];

/// Literal curved nodes for VTK type 37, triquadratic pyramid.
const TRIQUADRATIC_PYRAMID: &[[f64; 3]] = &[
    [0.0, 0.0, 0.0],
    [3.0, 0.0, 0.0],
    [3.0, 2.0, 0.0],
    [0.0, 2.0, 0.0],
    [1.5, 1.0, 2.5],
    [1.5, -0.399_999_999_999_999_9, -0.199_999_999_999_999_96],
    [3.599_999_999_999_999_6, 1.0, -0.199_999_999_999_999_96],
    [1.5, 2.4, -0.199_999_999_999_999_96],
    [-0.599_999_999_999_999_6, 1.0, -0.199_999_999_999_999_96],
    [
        0.450_000_000_000_000_2,
        0.300_000_000_000_000_04,
        1.549_999_999_999_999_8,
    ],
    [2.55, 0.300_000_000_000_000_04, 1.549_999_999_999_999_8],
    [2.55, 1.7, 1.549_999_999_999_999_8],
    [0.450_000_000_000_000_2, 1.7, 1.549_999_999_999_999_8],
    [1.5, 1.0, -0.5],
    [1.5, -0.333_333_333_333_333_5, 1.166_666_666_666_666_7],
    [3.5, 1.0, 1.166_666_666_666_666_7],
    [1.5, 2.333_333_333_333_333_5, 1.166_666_666_666_666_7],
    [-0.5, 1.0, 1.166_666_666_666_666_7],
    [1.5, 1.0, 0.5],
];

/// All sixteen fixed-node cell diagnostics; volume interior nodes stay separate from boundary nodes.
const CELLS: &[Cell] = &[
    Cell {
        kind: 21,
        name: "quadratic-edge",
        title: "VTK quadratic edge",
        points: QUADRATIC_EDGE,
    },
    Cell {
        kind: 22,
        name: "quadratic-triangle",
        title: "VTK quadratic triangle",
        points: QUADRATIC_TRIANGLE,
    },
    Cell {
        kind: 23,
        name: "quadratic-quad",
        title: "VTK quadratic quad",
        points: QUADRATIC_QUAD,
    },
    Cell {
        kind: 24,
        name: "quadratic-tetra",
        title: "VTK quadratic tetra",
        points: QUADRATIC_TETRA,
    },
    Cell {
        kind: 25,
        name: "quadratic-hexahedron",
        title: "VTK quadratic hexahedron",
        points: QUADRATIC_HEXAHEDRON,
    },
    Cell {
        kind: 26,
        name: "quadratic-wedge",
        title: "VTK quadratic wedge",
        points: QUADRATIC_WEDGE,
    },
    Cell {
        kind: 27,
        name: "quadratic-pyramid",
        title: "VTK quadratic pyramid",
        points: QUADRATIC_PYRAMID,
    },
    Cell {
        kind: 28,
        name: "biquadratic-quad",
        title: "VTK biquadratic quad",
        points: BIQUADRATIC_QUAD,
    },
    Cell {
        kind: 29,
        name: "triquadratic-hexahedron",
        title: "VTK triquadratic hexahedron",
        points: TRIQUADRATIC_HEXAHEDRON,
    },
    Cell {
        kind: 30,
        name: "quadratic-linear-quad",
        title: "VTK quadratic linear quad",
        points: QUADRATIC_LINEAR_QUAD,
    },
    Cell {
        kind: 31,
        name: "quadratic-linear-wedge",
        title: "VTK quadratic linear wedge",
        points: QUADRATIC_LINEAR_WEDGE,
    },
    Cell {
        kind: 32,
        name: "biquadratic-quadratic-wedge",
        title: "VTK biquadratic quadratic wedge",
        points: BIQUADRATIC_QUADRATIC_WEDGE,
    },
    Cell {
        kind: 33,
        name: "biquadratic-quadratic-hexahedron",
        title: "VTK biquadratic quadratic hexahedron",
        points: BIQUADRATIC_QUADRATIC_HEXAHEDRON,
    },
    Cell {
        kind: 34,
        name: "biquadratic-triangle",
        title: "VTK biquadratic triangle",
        points: BIQUADRATIC_TRIANGLE,
    },
    Cell {
        kind: 35,
        name: "cubic-line",
        title: "VTK cubic line",
        points: CUBIC_LINE,
    },
    Cell {
        kind: 37,
        name: "triquadratic-pyramid",
        title: "VTK triquadratic pyramid",
        points: TRIQUADRATIC_PYRAMID,
    },
];

/// Common unstructured-grid values before any container encoding.
struct Grid {
    /// Flattened XYZ nodes; authored inputs always use `Float64` coordinates.
    points: Vec<f64>,
    /// Ordered cells, each starting at the next entry of the shared point table.
    cells: Vec<Vec<i64>>,
    /// Standard type for each cell in file order.
    types: Vec<u8>,
}

impl Grid {
    /// Place each isolated authored cell on an eight-unit four-column grid when requested.
    ///
    /// # Errors
    /// A fixed catalog index cannot fit the portable integer fields used by the fixture encoders.
    fn from_cells(cells: &[Cell], placed: bool) -> Result<Self> {
        let mut grid = Self {
            points: Vec::new(),
            cells: Vec::new(),
            types: Vec::new(),
        };
        for (index, cell) in cells.iter().enumerate() {
            let x = if placed {
                8. * f64::from(u32::try_from(index % 4)?)
            } else {
                0.
            };
            let y = if placed {
                8. * f64::from(u32::try_from(index / 4)?)
            } else {
                0.
            };
            let start = i64::try_from(grid.points.len() / 3)?;
            for point in cell.points {
                grid.points.extend([point[0] + x, point[1] + y, point[2]]);
            }
            let end = start + i64::try_from(cell.points.len())?;
            grid.cells.push((start..end).collect());
            grid.types.push(cell.kind);
        }
        Ok(grid)
    }

    /// Flatten connectivity while preserving file-order cell boundaries.
    fn connectivity(&self) -> Vec<i64> {
        self.cells.iter().flatten().copied().collect()
    }

    /// Produce VTK offsets including the leading zero needed by VTKHDF.
    ///
    /// # Errors
    /// The bounded catalog's accumulated connectivity cannot fit an `Int64` offset.
    fn offsets(&self) -> Result<Vec<i64>> {
        let mut offsets = vec![0];
        let mut end = 0_i64;
        for cell in &self.cells {
            end += i64::try_from(cell.len())?;
            offsets.push(end);
        }
        Ok(offsets)
    }
}

/// Serialize display-independent values with round-trip `Float64` spellings and ordered connectivity.
///
/// # Errors
/// Formatting or portable connectivity offset conversion can fail.
fn ascii_vtu(grid: &Grid) -> Result<Vec<u8>> {
    let mut points = String::new();
    for value in &grid.points {
        write!(points, "{value} ")?;
    }
    let mut connectivity = String::new();
    for value in grid.connectivity() {
        write!(connectivity, "{value} ")?;
    }
    let mut offsets = String::new();
    for value in grid.offsets()?.iter().skip(1) {
        write!(offsets, "{value} ")?;
    }
    let mut types = String::new();
    for value in &grid.types {
        write!(types, "{value} ")?;
    }
    Ok(format!(r#"<?xml version="1.0"?>
<VTKFile type="UnstructuredGrid" version="0.1" byte_order="LittleEndian"><UnstructuredGrid><Piece NumberOfPoints="{}" NumberOfCells="{}"><Points><DataArray type="Float64" NumberOfComponents="3" format="ascii">{points}</DataArray></Points><Cells><DataArray type="Int64" Name="connectivity" format="ascii">{connectivity}</DataArray><DataArray type="Int64" Name="offsets" format="ascii">{offsets}</DataArray><DataArray type="UInt8" Name="types" format="ascii">{types}</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>
"#, grid.points.len() / 3, grid.cells.len()).into_bytes())
}

/// Serialize a full curved-cell family using classic legacy ASCII or standard big-endian binary.
///
/// # Errors
/// Formatting or a catalog index conversion to the classic `Int32` words can fail.
fn legacy(grid: &Grid, binary: bool) -> Result<Vec<u8>> {
    let mode = if binary { "BINARY" } else { "ASCII" };
    let mut output = format!("# vtk DataFile Version 3.0\nCurved fixed-node cell family\n{mode}\nDATASET UNSTRUCTURED_GRID\nPOINTS {} double\n", grid.points.len() / 3).into_bytes();
    if binary {
        for value in &grid.points {
            output.extend(value.to_be_bytes());
        }
    } else {
        for (index, value) in grid.points.iter().enumerate() {
            if index != 0 {
                output.push(b' ');
            }
            write!(output, "{value}")?;
        }
    }
    write!(
        output,
        "\nCELLS {} {}\n",
        grid.cells.len(),
        grid.connectivity().len() + grid.cells.len()
    )?;
    for cell in &grid.cells {
        if binary {
            output.extend(i32::try_from(cell.len())?.to_be_bytes());
            for value in cell {
                output.extend(i32::try_from(*value)?.to_be_bytes());
            }
        } else {
            write!(output, "{}", cell.len())?;
            for value in cell {
                write!(output, " {value}")?;
            }
            output.push(b'\n');
        }
    }
    write!(output, "\nCELL_TYPES {}\n", grid.types.len())?;
    for (index, value) in grid.types.iter().enumerate() {
        if binary {
            output.extend(i32::from(*value).to_be_bytes());
        } else {
            if index != 0 {
                output.push(b' ');
            }
            write!(output, "{value}")?;
        }
    }
    output.push(b'\n');
    Ok(output)
}

/// One independently typed XML array before binary packaging.
struct Array {
    /// XML array name, including the conventional Points label.
    name: &'static str,
    /// XML scalar datatype of the encoded bytes.
    datatype: &'static str,
    /// Scalar components in one logical tuple.
    components: u8,
    /// Little-endian encoded values, before a header or compressor is applied.
    raw: Vec<u8>,
}

/// Encode the four ordered XML arrays without coupling to the production decoder.
///
/// # Errors
/// Connectivity offsets cannot fit their portable `Int64` fields.
fn arrays(grid: &Grid) -> Result<Vec<Array>> {
    Ok(vec![
        Array {
            name: "Points",
            datatype: "Float64",
            components: 3,
            raw: grid.points.iter().flat_map(|v| v.to_le_bytes()).collect(),
        },
        Array {
            name: "connectivity",
            datatype: "Int64",
            components: 1,
            raw: grid
                .connectivity()
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        },
        Array {
            name: "offsets",
            datatype: "Int64",
            components: 1,
            raw: grid
                .offsets()?
                .iter()
                .skip(1)
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        },
        Array {
            name: "types",
            datatype: "UInt8",
            components: 1,
            raw: grid.types.clone(),
        },
    ])
}

/// Build inline `UInt64`-header binary or raw-appended `UInt32`-header zlib XML arrays.
///
/// # Errors
/// Headers cannot represent the bounded arrays, compression fails, or formatting fails.
fn binary_vtu(grid: &Grid, appended: bool) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut tags = Vec::new();
    for array in arrays(grid)? {
        let tag = if appended {
            let offset = data.len();
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(&array.raw)?;
            let compressed = encoder.finish()?;
            for word in [
                1,
                u32::try_from(array.raw.len())?,
                u32::try_from(array.raw.len())?,
                u32::try_from(compressed.len())?,
            ] {
                data.extend(word.to_le_bytes());
            }
            data.extend(compressed);
            format!(
                r#"<DataArray Name="{}" type="{}" NumberOfComponents="{}" format="appended" offset="{offset}"/>"#,
                array.name, array.datatype, array.components
            )
        } else {
            let mut bytes = u64::try_from(array.raw.len())?.to_le_bytes().to_vec();
            bytes.extend(array.raw);
            format!(
                r#"<DataArray Name="{}" type="{}" NumberOfComponents="{}" format="binary">{}</DataArray>"#,
                array.name,
                array.datatype,
                array.components,
                STANDARD.encode(bytes)
            )
        };
        tags.push(tag);
    }
    let header = if appended {
        r#"header_type="UInt32" compressor="vtkZLibDataCompressor""#
    } else {
        r#"header_type="UInt64""#
    };
    let mut output = format!(r#"<?xml version="1.0"?>
<VTKFile type="UnstructuredGrid" version="0.1" byte_order="LittleEndian" {header}><UnstructuredGrid><Piece NumberOfPoints="{}" NumberOfCells="{}"><Points>{}</Points><Cells>{}{}{}</Cells></Piece></UnstructuredGrid>"#,grid.points.len()/3,grid.cells.len(),tags[0],tags[1],tags[2],tags[3]).into_bytes();
    if appended {
        output.extend(b"<AppendedData encoding=\"raw\">_");
        output.extend(data);
        output.extend(b"</AppendedData>");
    }
    output.extend(b"</VTKFile>\n");
    Ok(output)
}

/// Build one typed VTKHDF dataset from the same authored grid values.
///
/// # Errors
/// Dataset datatype, shape or value serialization cannot be represented by the Rust writer.
fn dataset<T: hdf5_writer::H5WriteElement>(
    name: &str,
    values: &[T],
    shape: &[u64],
) -> Result<DatasetBuilder> {
    Ok(DatasetBuilder::typed_data(name, shape.to_vec(), values)?)
}

/// Encode the full family as one static VTKHDF `UnstructuredGrid` partition.
///
/// # Errors
/// Dataset/attribute creation, header counts or HDF5 serialization can fail.
fn vtkhdf(grid: &Grid) -> Result<Vec<u8>> {
    let points = u64::try_from(grid.points.len() / 3)?;
    let cells = u64::try_from(grid.cells.len())?;
    let connectivity = grid.connectivity();
    let offsets = grid.offsets()?;
    let ids = u64::try_from(connectivity.len())?;
    let builder = Hdf5Builder::new()
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::fixed_string("Type", "UnstructuredGrid"),
        )
        .group_attribute("VTKHDF", AttributeBuilder::vector("Version", &[2_i32, 0])?)
        .dataset(dataset("VTKHDF/Points", &grid.points, &[points, 3])?)
        .dataset(dataset(
            "VTKHDF/NumberOfPoints",
            &[i64::try_from(points)?],
            &[1],
        )?)
        .dataset(dataset(
            "VTKHDF/NumberOfCells",
            &[i64::try_from(cells)?],
            &[1],
        )?)
        .dataset(dataset(
            "VTKHDF/NumberOfConnectivityIds",
            &[i64::try_from(ids)?],
            &[1],
        )?)
        .dataset(dataset("VTKHDF/Offsets", &offsets, &[cells + 1])?)
        .dataset(dataset("VTKHDF/Connectivity", &connectivity, &[ids])?)
        .dataset(dataset("VTKHDF/Types", &grid.types, &[cells])?);
    Ok(builder.into_plan()?.encode(WriteOptions::default())?)
}

/// Register only inputs whose genuine or authored curved geometry can be loaded.
///
/// # Errors
/// Reading a required input or decoding nonempty geometry fails.
fn register(
    root: &Path,
    id: &str,
    extension: &str,
    name: &str,
    title: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{name}");
    let scene = catwalk3d::formats::load(&root.join(&input))?;
    if scene.triangles.is_empty() && scene.points.is_empty() && scene.lines.is_empty() {
        return Err(format!("curved VTK fixture {input} has no geometry").into());
    }
    Ok(crate::fixture(
        id, extension, title, &input, "model", provenance,
    ))
}

/// Emit the five whole-family representations after the individual cell image cases.
///
/// # Errors
/// Portable encoding, resource writes or geometry registration can fail.
fn family(root: &Path, cases: &mut Vec<crate::Fixture>) -> Result<()> {
    let grid = Grid::from_cells(CELLS, true)?;
    for (id, extension, title, bytes) in [
        (
            "vtk-quadratic-family-ascii",
            "vtk",
            "VTK curved cell family, legacy ASCII",
            legacy(&grid, false)?,
        ),
        (
            "vtk-quadratic-family-binary",
            "vtk",
            "VTK curved cell family, big-endian binary",
            legacy(&grid, true)?,
        ),
        (
            "vtu-quadratic-family-binary",
            "vtu",
            "VTK curved cell family, XML UInt64 inline binary",
            binary_vtu(&grid, false)?,
        ),
        (
            "vtu-quadratic-family-appended-zlib",
            "vtu",
            "VTK curved cell family, raw appended zlib",
            binary_vtu(&grid, true)?,
        ),
        (
            "vtkhdf-quadratic-family",
            "vtkhdf",
            "VTKHDF curved cell family, one static partition",
            vtkhdf(&grid)?,
        ),
    ] {
        let name = format!("{id}.{extension}");
        fs::write(root.join(DIR).join(&name), bytes)?;
        cases.push(register(root, id, extension, &name, title, AUTHORED)?);
    }
    Ok(())
}

/// Copy the original Kitware bytes and write sixteen cell and five cross-encoding image cases.
///
/// # Errors
/// Encoding, resource writes or decoding the required original/authored geometry can fail.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    fs::create_dir_all(root.join(DIR))?;
    for (name, bytes) in [
        (
            "quadraticTetra01.vtu",
            include_bytes!("../../tests/fixtures/formats/vtk_quadratic/quadraticTetra01.vtu")
                .as_slice(),
        ),
        (
            "VTK-Copyright.txt",
            include_bytes!("../../tests/fixtures/formats/vtk_quadratic/VTK-Copyright.txt")
                .as_slice(),
        ),
    ] {
        fs::write(root.join(DIR).join(name), bytes)?;
    }
    let mut cases = vec![register(
        root,
        "vtu-kitware-quadratic-tetra",
        "vtu",
        "quadraticTetra01.vtu",
        "Kitware original curved tetrahedron and surface triangles",
        ORIGINAL,
    )?];
    for cell in CELLS {
        let grid = Grid::from_cells(std::slice::from_ref(cell), false)?;
        let id = format!("vtu-{}", cell.name);
        let name = format!("{id}.vtu");
        fs::write(root.join(DIR).join(&name), ascii_vtu(&grid)?)?;
        cases.push(register(root, &id, "vtu", &name, cell.title, AUTHORED)?);
    }
    family(root, &mut cases)?;
    Ok(cases)
}
