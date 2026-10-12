//! Scientific containers decoded without HDF5 or NetCDF C libraries.
//! Schemas: <https://docs.vtk.org/en/latest/vtk_file_formats/vtkhdf_file_format/vtkhdf_specifications.html>
//! Exodus storage: <https://sandialabs.github.io/seacas-docs/html/exodusII__int_8h_source.html>
//! NetCDF coordinates: <https://docs.unidata.ucar.edu/netcdf-c/current/attribute_conventions.html>
use super::{extension, hdf5_owned as hdf, read_file, scientific, volume};
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use hdf::{Dataset, Hdf5File, NcFile};
use hdf5_reader::{Attribute, Datatype};
use netcdf_reader::{NcSliceInfo, NcSliceInfoElem, NcType, NcVariable};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Maximum scalar sample count accepted before array expansion.
const MAX_SAMPLES: usize = 16_777_216;
/// Maximum declared point count accepted before geometry allocation.
const MAX_POINTS: usize = 1_000_000;
/// Maximum scientific cell count accepted before topology expansion.
const MAX_CELLS: usize = 1_000_000;
/// Maximum container partitions visited during composite traversal.
const MAX_PARTS: usize = 1024;
/// Aggregate decoded numeric-array byte limit.
const MAX_DECODED: usize = 512 * 1024 * 1024;

/// Read a bounded scientific HDF5, Exodus and `NetCDF` container and decode its preview geometry.
///
/// # Errors
/// Returns an error for unreadable or unsupported containers, invalid array schemas/topology,
/// malformed compressed data or resource limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = read_file(path)?;
    match extension(path)?.as_str() {
        "vtkhdf" => vtkhdf(&bytes),
        "e" | "ex2" | "exo" | "g" => exodus(&bytes),
        "nc" | "cdf" | "ncdf" => netcdf(&bytes),
        _ => Err("unrecognized scientific container extension".into()),
    }
}

/// Aggregate decoded-array budget shared by container partitions.
struct Budget {
    /// Expanded scalar bytes already charged to this container.
    used: usize,
}
impl Budget {
    /// Start aggregate decoded-array byte accounting for a container.
    fn new() -> Self {
        Self { used: 0 }
    }
    /// Validate an array shape and charge its expanded double-width storage to the byte budget.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The sample count is bounded to 16,777,216 before conversion to a native allocation size."
    )]
    fn reserve(&mut self, shape: &[u64]) -> Result<usize> {
        let count = shape
            .iter()
            .try_fold(1u64, |n, &d| n.checked_mul(d))
            .ok_or("scientific array shape overflows")?;
        if count > MAX_SAMPLES as u64 {
            return Err("scientific array exceeds 16777216 samples".into());
        }
        let count = count as usize;
        self.used = self
            .used
            .checked_add(
                count
                    .checked_mul(8)
                    .ok_or("scientific array byte size overflows")?,
            )
            .ok_or("scientific decoded byte count overflows")?;
        if self.used > MAX_DECODED {
            return Err("scientific arrays exceed aggregate 512 MiB decoded limit".into());
        }
        Ok(count)
    }
}

/// An HDF5 file viewed through a rebased VTKHDF group.
struct HdfSource<'a> {
    /// Opened in-memory HDF5 container.
    file: &'a Hdf5File,
    /// Actual group path replacing the logical /VTKHDF prefix.
    base: String,
}
impl HdfSource<'_> {
    /// Rebase a logical VTKHDF path into the selected dataset group.
    fn path(&self, path: &str) -> Result<String> {
        let suffix = path
            .strip_prefix("/VTKHDF")
            .ok_or("invalid internal VTKHDF path")?;
        Ok(format!("{}{suffix}", self.base))
    }
    /// Open a rebased HDF5 dataset.
    fn dataset(&self, path: &str) -> Result<Dataset> {
        self.file.dataset(&self.path(path)?)
    }
    /// Open a rebased HDF5 group.
    fn group(&self, path: &str) -> Result<hdf5_reader::group::Group> {
        Ok(self.file.group(&self.path(path)?)?)
    }
}

// HDF5 typed reads assert the stored type, so dispatch instead of coercing IDs
// through floating point (which would lose precision for 64-bit connectivity).
/// Dispatch integer HDF5 reads by stored width and signedness without lossy floating-point
/// conversion.
macro_rules! hdf_dispatch {
    ($datatype:expr, $read:ident, $object:expr, $convert:expr) => {{
        match $datatype {
            Datatype::FixedPoint {
                size: 1,
                signed: true,
                ..
            } => $object.$read::<i8>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 1,
                signed: false,
                ..
            } => $object.$read::<u8>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 2,
                signed: true,
                ..
            } => $object.$read::<i16>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 2,
                signed: false,
                ..
            } => $object.$read::<u16>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 4,
                signed: true,
                ..
            } => $object.$read::<i32>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 4,
                signed: false,
                ..
            } => $object.$read::<u32>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 8,
                signed: true,
                ..
            } => $object.$read::<i64>()?.iter().map($convert).collect(),
            Datatype::FixedPoint {
                size: 8,
                signed: false,
                ..
            } => $object.$read::<u64>()?.iter().map($convert).collect(),
            _ => {
                return Err(
                    "HDF connectivity/count attribute must use a standard integer type".into(),
                );
            }
        }
    }};
}

/// Validate HDF5 dataset dimensions and individual decoded chunk sizes before reading.
fn hdf_dataset(file: &HdfSource, name: &str, budget: &mut Budget) -> Result<Dataset> {
    let dataset = file.dataset(name)?;
    budget.reserve(dataset.shape())?;
    if let Some(chunks) = dataset.chunks() {
        let count = chunks
            .iter()
            .try_fold(1u64, |n, &d| n.checked_mul(u64::from(d)))
            .ok_or("HDF chunk dimensions overflow")?;
        let size = hdf5_reader::dtype_element_size(dataset.dtype())?;
        if count
            .checked_mul(size as u64)
            .is_none_or(|n| n > 128 * 1024 * 1024)
        {
            return Err("HDF chunk exceeds 128 MiB decoded limit".into());
        }
    }
    Ok(dataset)
}

/// Read one-dimensional HDF5 integer connectivity or count arrays with checked native-index
/// conversion.
fn hdf_ids(file: &HdfSource, name: &str, budget: &mut Budget) -> Result<Vec<usize>> {
    let ds = hdf_dataset(file, name, budget)?;
    if ds.ndim() != 1 {
        return Err(format!("{name} must be one-dimensional").into());
    }
    hdf_dispatch!(ds.dtype(), read_array, ds, |&n| usize::try_from(
        i128::from(n)
    )
    .map_err(|_| "HDF integer is negative or overflows".into()))
}

/// Read supported HDF5 numeric dataset scalars as double-precision geometry values.
#[allow(
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    reason = "One integer-dispatch macro covers all widths; 64-bit scalar values intentionally approximate to f64 for preview geometry."
)]
fn hdf_values(ds: &Dataset) -> Result<Vec<f64>> {
    match ds.dtype() {
        Datatype::FloatingPoint { size: 4, .. } => Ok(ds
            .read_array::<f32>()?
            .iter()
            .map(|&n| f64::from(n))
            .collect()),
        Datatype::FloatingPoint { size: 8, .. } => {
            Ok(ds.read_array::<f64>()?.iter().copied().collect())
        }
        _ => {
            let values: Vec<f64> = hdf_dispatch!(ds.dtype(), read_array, ds, |&n| n as f64);
            Ok(values)
        }
    }
}

/// Read a supported numeric HDF5 attribute as geometry metadata.
#[allow(
    clippy::cast_lossless,
    clippy::cast_precision_loss,
    reason = "One integer-dispatch macro covers all widths; numeric placement attributes intentionally use f64, including 64-bit integers."
)]
fn hdf_attribute_values(attr: &Attribute) -> Result<Vec<f64>> {
    if attr.num_elements()? > 16 {
        return Err("HDF geometry attribute exceeds 16 values".into());
    }
    match &attr.datatype {
        Datatype::FloatingPoint { size: 4, .. } => Ok(attr
            .read_1d::<f32>()?
            .iter()
            .map(|&n| f64::from(n))
            .collect()),
        Datatype::FloatingPoint { size: 8, .. } => Ok(attr.read_1d::<f64>()?),
        _ => {
            let values: Vec<f64> = hdf_dispatch!(&attr.datatype, read_1d, attr, |&n| n as f64);
            Ok(values)
        }
    }
}

/// Open an in-memory HDF5 file and decode its root VTKHDF dataset.
fn vtkhdf(bytes: &[u8]) -> Result<Scene> {
    let file = Hdf5File::from_bytes(bytes)?;
    hdf_source(
        &HdfSource {
            file: &file,
            base: "/VTKHDF".into(),
        },
        &mut Budget::new(),
    )
}

/// Dispatch the selected VTKHDF type, rejecting time-dependent or unsupported schemas.
fn hdf_source(file: &HdfSource, budget: &mut Budget) -> Result<Scene> {
    let root = file.group("/VTKHDF")?;
    let version_attr = root.attribute("Version")?;
    if !matches!(version_attr.datatype, Datatype::FixedPoint { .. }) {
        return Err("VTKHDF Version must use an integer attribute".into());
    }
    let version = hdf_attribute_values(&version_attr)?;
    if version.len() != 2
        || !matches!(version[0], 1. | 2.)
        || version[1] < 0.
        || version[1].fract() != 0.
    {
        return Err("unsupported or malformed VTKHDF version".into());
    }
    if root
        .groups()?
        .iter()
        .any(|g| g.name().rsplit('/').next() == Some("Steps"))
    {
        return Err("temporal VTKHDF Steps schema is not implemented".into());
    }
    let kind = root.attribute("Type")?.read_string()?;
    match kind.as_str() {
        "UnstructuredGrid" | "PolyData" => hdf_mesh(file, &kind, budget),
        "ImageData" => hdf_image(file, budget),
        "RectilinearGrid" | "StructuredGrid" => hdf_structured(file, &kind, budget),
        "MultiBlockDataSet" | "PartitionedDataSetCollection" => hdf_composite(file, budget),
        _ => Err(format!("VTKHDF dataset type {kind} is not implemented").into()),
    }
}

/// Resolve VTKHDF composite assembly links and combine their child datasets.
fn hdf_composite(file: &HdfSource, budget: &mut Budget) -> Result<Scene> {
    let root = file.group("/VTKHDF")?;
    let mut blocks = BTreeMap::new();
    for group in root.groups()? {
        if group.name() == "Assembly" || group.name() == "FieldData" {
            continue;
        }
        let name = group.name().to_owned();
        let kind = group
            .attribute("Type")
            .ok()
            .map(|a| a.read_string())
            .transpose()?;
        if kind.as_ref().is_some_and(|s| {
            matches!(
                s.as_str(),
                "MultiBlockDataSet" | "PartitionedDataSetCollection"
            )
        }) {
            return Err("VTKHDF composite blocks must be non-composite datasets".into());
        }
        blocks.insert(group.address(), (name, kind.is_some()));
    }
    if blocks.len() > MAX_PARTS {
        return Err("VTKHDF composite exceeds 1024 blocks".into());
    }
    let assembly = root.group("Assembly")?;
    let mut scene = Scene::default();
    let mut stack = Vec::new();
    let mut nodes = 0;
    walk_assembly(
        file, &assembly, &blocks, budget, &mut scene, &mut stack, &mut nodes,
    )?;
    scene.validate()?;
    Ok(scene)
}

/// Walk assembly groups without revisiting cyclic or duplicate dataset links.
fn walk_assembly(
    file: &HdfSource,
    group: &hdf5_reader::group::Group,
    blocks: &BTreeMap<u64, (String, bool)>,
    budget: &mut Budget,
    scene: &mut Scene,
    stack: &mut Vec<u64>,
    nodes: &mut usize,
) -> Result<()> {
    *nodes += 1;
    if *nodes > 4096 || stack.len() > 32 {
        return Err("VTKHDF assembly exceeds depth32 or4096 nodes".into());
    }
    if let Some((name, present)) = blocks.get(&group.address()) {
        if *present {
            let child = HdfSource {
                file: file.file,
                base: format!("{}/{name}", file.base),
            };
            merge(scene, hdf_source(&child, budget)?)?;
        }
        return Ok(());
    }
    if stack.contains(&group.address()) {
        return Err("VTKHDF assembly contains a cyclic group reference".into());
    }
    stack.push(group.address());
    let (groups, datasets) = group.members()?;
    if !datasets.is_empty() {
        return Err("VTKHDF assembly nodes must reference dataset groups".into());
    }
    for child in groups {
        walk_assembly(file, &child, blocks, budget, scene, stack, nodes)?;
    }
    stack.pop();
    Ok(())
}

/// Convert finite coordinate triples into point positions.
fn points(values: &[f64]) -> Result<Vec<Vec3>> {
    if !values.len().is_multiple_of(3) || values.len() / 3 > MAX_POINTS {
        return Err("scientific points must be triplets, with at most 1000000 points".into());
    }
    if values.iter().any(|n| !n.is_finite()) {
        return Err("scientific point contains non-finite coordinates".into());
    }
    Ok(values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
        .collect())
}

/// Sum partition counts while enforcing a caller-provided record maximum.
fn total(counts: &[usize], maximum: usize) -> Result<usize> {
    let n = counts
        .iter()
        .try_fold(0usize, |sum, &n| sum.checked_add(n))
        .ok_or("scientific count overflows")?;
    if n > maximum {
        return Err("scientific topology count exceeds preview limit".into());
    }
    Ok(n)
}

/// Split partition-local offsets/connectivity into validated individual cells.
fn partition_cells(
    offsets: &[usize],
    connectivity: &[usize],
    point_count: usize,
) -> Result<Vec<Vec<usize>>> {
    if offsets.first() != Some(&0)
        || offsets.last() != Some(&connectivity.len())
        || offsets.windows(2).any(|w| w[0] > w[1])
    {
        return Err("scientific connectivity offsets are not monotonic from zero to length".into());
    }
    if connectivity.iter().any(|&i| i >= point_count) {
        return Err("scientific connectivity point index out of range".into());
    }
    Ok(offsets
        .windows(2)
        .map(|w| connectivity[w[0]..w[1]].to_vec())
        .collect())
}

/// Append a child container scene while enforcing aggregate geometry limits.
fn merge(scene: &mut Scene, child: Scene) -> Result<()> {
    scene.triangles.extend(child.triangles);
    scene.points.extend(child.points);
    scene.lines.extend(child.lines);
    if scene.triangles.len() + scene.points.len() + scene.lines.len() > 5_000_000 {
        return Err("scientific scene exceeds five million primitives".into());
    }
    Ok(())
}

/// Extract an unstructured boundary with standard linear cell types.
fn grid_scene(positions: &[Vec3], cells: &[Vec<usize>], types: &[usize]) -> Result<Scene> {
    grid_scene_poly(positions, cells, types, &BTreeMap::new())
}

/// Use the shared VTK point, ordered-line and exterior-face extractor with an aggregate budget.
fn grid_scene_poly(
    positions: &[Vec3],
    cells: &[Vec<usize>],
    types: &[usize],
    polyhedra: &BTreeMap<usize, Vec<Vec<usize>>>,
) -> Result<Scene> {
    let scene = if polyhedra.is_empty() {
        scientific::grid_scene(positions, cells, types)?
    } else {
        scientific::grid_scene_with_polyhedra(positions, cells, types, polyhedra)?
    };
    if scene.triangles.len() + scene.points.len() + scene.lines.len() > 5_000_000 {
        return Err("scientific scene exceeds five million primitives".into());
    }
    Ok(scene)
}

/// Decode partitioned HDF5 polydata or unstructured meshes with local point indices.
fn hdf_mesh(file: &HdfSource, kind: &str, budget: &mut Budget) -> Result<Scene> {
    let counts = hdf_ids(file, "/VTKHDF/NumberOfPoints", budget)?;
    if counts.is_empty() || counts.len() > MAX_PARTS {
        return Err("VTKHDF requires 1..1024 partitions".into());
    }
    let point_total = total(&counts, MAX_POINTS)?;
    let dataset = hdf_dataset(file, "/VTKHDF/Points", budget)?;
    if dataset.shape() != [point_total as u64, 3] {
        return Err("VTKHDF Points shape disagrees with partition counts".into());
    }
    let positions = points(&hdf_values(&dataset)?)?;
    reject_ghosts(file, budget)?;
    let categories: Vec<(&str, Option<usize>)> = if kind == "PolyData" {
        vec![
            ("Vertices", Some(2)),
            ("Lines", Some(4)),
            ("Polygons", Some(7)),
            ("Strips", Some(6)),
        ]
    } else {
        vec![("", None)]
    };
    let mut all_cells = vec![Vec::new(); counts.len()];
    let mut all_types = vec![Vec::new(); counts.len()];
    let mut all_polyhedra = vec![BTreeMap::new(); counts.len()];
    let mut aggregate_cells = 0;
    for (category, fixed_kind) in categories {
        let prefix = if category.is_empty() {
            "/VTKHDF".to_string()
        } else {
            format!("/VTKHDF/{category}")
        };
        // VTK writers normally include empty categories; also permit absent categories.
        if fixed_kind.is_some() && file.group(&prefix).is_err() {
            continue;
        }
        let nconn = hdf_ids(file, &format!("{prefix}/NumberOfConnectivityIds"), budget)?;
        let ncells = hdf_ids(file, &format!("{prefix}/NumberOfCells"), budget)?;
        if nconn.len() != counts.len() || ncells.len() != counts.len() {
            return Err("VTKHDF partition count arrays disagree".into());
        }
        let conn = hdf_ids(file, &format!("{prefix}/Connectivity"), budget)?;
        let offsets = hdf_ids(file, &format!("{prefix}/Offsets"), budget)?;
        let cell_total = total(&ncells, MAX_CELLS)?;
        aggregate_cells = total(&[aggregate_cells, cell_total], MAX_CELLS)?;
        if total(&nconn, MAX_SAMPLES)? != conn.len() || offsets.len() != cell_total + counts.len() {
            return Err("VTKHDF concatenated topology sizes disagree".into());
        }
        let types = if let Some(kind) = fixed_kind {
            vec![kind; cell_total]
        } else {
            hdf_ids(file, "/VTKHDF/Types", budget)?
        };
        if types.len() != cell_total {
            return Err("VTKHDF cell type count disagrees".into());
        }
        if types.contains(&42) {
            all_polyhedra = hdf_polyhedra(file, &counts, &ncells, &types, budget)?;
        }
        let (mut ci, mut oi, mut ti) = (0, 0, 0);
        for i in 0..counts.len() {
            let cells = partition_cells(
                &offsets[oi..=(oi + ncells[i])],
                &conn[ci..ci + nconn[i]],
                counts[i],
            )?;
            all_cells[i].extend(cells);
            all_types[i].extend_from_slice(&types[ti..ti + ncells[i]]);
            ci += nconn[i];
            oi += ncells[i] + 1;
            ti += ncells[i];
        }
    }
    let mut scene = Scene::default();
    let mut start = 0;
    for (((&n, cells), types), polyhedra) in counts
        .iter()
        .zip(&all_cells)
        .zip(&all_types)
        .zip(&all_polyhedra)
    {
        if !cells.is_empty() {
            merge(
                &mut scene,
                grid_scene_poly(&positions[start..start + n], cells, types, polyhedra)?,
            )?;
        }
        start += n;
    }
    scene.validate()?;
    Ok(scene)
}

/// Decode HDF5 polyhedron face/connectivity streams for one cell partition.
fn hdf_polyhedra(
    file: &HdfSource,
    npoints: &[usize],
    ncells: &[usize],
    types: &[usize],
    budget: &mut Budget,
) -> Result<Vec<BTreeMap<usize, Vec<Vec<usize>>>>> {
    let nfaces = hdf_ids(file, "/VTKHDF/NumberOfFaces", budget)?;
    let nconn = hdf_ids(file, "/VTKHDF/NumberOfFaceConnectivityIds", budget)?;
    let nrefs = hdf_ids(file, "/VTKHDF/NumberOfPolyhedronToFaceIds", budget)?;
    if [nfaces.len(), nconn.len(), nrefs.len()]
        .iter()
        .any(|&n| n != npoints.len())
    {
        return Err("VTKHDF polyhedron partition counts disagree".into());
    }
    let faces = hdf_ids(file, "/VTKHDF/FaceConnectivity", budget)?;
    let offsets = hdf_ids(file, "/VTKHDF/FaceOffsets", budget)?;
    let refs = hdf_ids(file, "/VTKHDF/PolyhedronToFaces", budget)?;
    let poly_offsets = hdf_ids(file, "/VTKHDF/PolyhedronOffsets", budget)?;
    if total(&nfaces, MAX_CELLS)? + npoints.len() != offsets.len()
        || total(&nconn, MAX_SAMPLES)? != faces.len()
        || total(&nrefs, MAX_SAMPLES)? != refs.len()
        || total(ncells, MAX_CELLS)? + npoints.len() != poly_offsets.len()
    {
        return Err("VTKHDF polyhedron topology sizes disagree".into());
    }
    let (mut fi, mut oi, mut ri, mut pi, mut ti) = (0, 0, 0, 0, 0);
    let mut result = Vec::new();
    for part in 0..npoints.len() {
        let face_cells = partition_cells(
            &offsets[oi..=(oi + nfaces[part])],
            &faces[fi..fi + nconn[part]],
            npoints[part],
        )?;
        let poly_cells = partition_cells(
            &poly_offsets[pi..=(pi + ncells[part])],
            &refs[ri..ri + nrefs[part]],
            nfaces[part],
        )?;
        let mut polys = BTreeMap::new();
        for (i, face_ids) in poly_cells.iter().enumerate() {
            if types[ti + i] == 42 {
                polys.insert(i, face_ids.iter().map(|&i| face_cells[i].clone()).collect());
            } else if !face_ids.is_empty() {
                return Err("VTKHDF non-polyhedron cell has polyhedron faces".into());
            }
        }
        result.push(polys);
        fi += nconn[part];
        oi += nfaces[part] + 1;
        ri += nrefs[part];
        pi += ncells[part] + 1;
        ti += ncells[part];
    }
    Ok(result)
}

/// Reject hidden or duplicate ghost-cell metadata that would change visible topology.
fn reject_ghosts(file: &HdfSource, budget: &mut Budget) -> Result<()> {
    for name in [
        "/VTKHDF/PointData/vtkGhostType",
        "/VTKHDF/CellData/vtkGhostType",
    ] {
        if file.dataset(name).is_ok() {
            let ds = hdf_dataset(file, name, budget)?;
            if hdf_values(&ds)?.iter().any(|&v| v != 0.) {
                return Err("VTKHDF ghost/blanked geometry is not implemented".into());
            }
        }
    }
    Ok(())
}

/// Read a fixed-size finite numeric HDF5 placement attribute.
fn geometry_attribute<const N: usize>(file: &HdfSource, name: &str) -> Result<[f64; N]> {
    let values = hdf_attribute_values(&file.group("/VTKHDF")?.attribute(name)?)?;
    let values: [f64; N] = values
        .try_into()
        .map_err(|_| format!("VTKHDF {name} attribute has wrong length"))?;
    if values.iter().any(|n| !n.is_finite()) {
        return Err(format!("VTKHDF {name} attribute is not finite").into());
    }
    Ok(values)
}

/// Decode HDF5 image scalars and preserve direction, origin, spacing and scalar masks.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Validated extents and sample budgets bound integer dimensions; their spatial coordinates intentionally use f64."
)]
fn hdf_image(file: &HdfSource, budget: &mut Budget) -> Result<Scene> {
    let extent = geometry_attribute::<6>(file, "WholeExtent")?;
    let mut dimensions = [0; 3];
    for axis in 0..3 {
        let (low, high) = (extent[2 * axis], extent[2 * axis + 1]);
        if low.fract() != 0. || high.fract() != 0. || high < low || high - low >= MAX_SAMPLES as f64
        {
            return Err("VTKHDF WholeExtent must contain bounded ordered integers".into());
        }
        dimensions[axis] = (high - low + 1.) as usize;
    }
    let origin = geometry_attribute::<3>(file, "Origin")?;
    let spacing = geometry_attribute::<3>(file, "Spacing")?;
    let direction = geometry_attribute::<9>(file, "Direction")?;
    let axes = std::array::from_fn(|i| {
        Vec3::new(direction[i], direction[i + 3], direction[i + 6]) * spacing[i]
    });
    let mut origin = Vec3::new(origin[0], origin[1], origin[2])
        + axes[0] * extent[0]
        + axes[1] * extent[2]
        + axes[2] * extent[4];
    reject_ghosts(file, budget)?;
    for (name, cell_data) in [("PointData", false), ("CellData", true)] {
        let Ok(group) = file.group(&format!("/VTKHDF/{name}")) else {
            continue;
        };
        let mut candidates = group.datasets()?;
        candidates.sort_by_key(|d| {
            (
                !d.attribute("Attribute")
                    .ok()
                    .and_then(|a| a.read_string().ok())
                    .is_some_and(|s| s.eq_ignore_ascii_case("Scalars")),
                d.name().to_owned(),
            )
        });
        let dims = if cell_data {
            dimensions.map(|n| n.saturating_sub(1).max(1))
        } else {
            dimensions
        };
        let shape = [dims[2] as u64, dims[1] as u64, dims[0] as u64];
        for ds in candidates {
            if ds.name().rsplit('/').next() == Some("vtkGhostType") {
                continue;
            }
            if ds.shape() != shape && ds.shape() != [shape[0], shape[1], shape[2], 1] {
                continue;
            }
            let ds = hdf_dataset(file, &format!("/VTKHDF/{name}/{}", ds.name()), budget)?;
            if cell_data {
                for axis in 0..3 {
                    if dimensions[axis] > 1 {
                        origin = origin + axes[axis] * 0.5;
                    }
                }
            }
            return volume::density_scene(dims, origin, axes, hdf_values(&ds)?);
        }
    }
    Err("VTKHDF ImageData contains no supported scalar density array".into())
}

/// Decode HDF5 structured or rectilinear grid coordinates and extract outer faces.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Validated extents and point budgets bound integer dimensions before native indexing and f64 placement."
)]
fn hdf_structured(file: &HdfSource, kind: &str, budget: &mut Budget) -> Result<Scene> {
    let dims = geometry_attribute::<3>(file, "Dimensions")?;
    if dims
        .iter()
        .any(|&n| n < 1. || n.fract() != 0. || n > MAX_POINTS as f64)
    {
        return Err("invalid VTKHDF structured dimensions".into());
    }
    let dims = dims.map(|n| n as usize);
    let count = total(
        &[dims
            .iter()
            .try_fold(1usize, |n, &d| n.checked_mul(d))
            .ok_or("structured size overflows")?],
        MAX_POINTS,
    )?;
    reject_ghosts(file, budget)?;
    let positions = if kind == "StructuredGrid" {
        let ds = hdf_dataset(file, "/VTKHDF/Points", budget)?;
        if ds.shape() != [dims[2] as u64, dims[1] as u64, dims[0] as u64, 3] {
            return Err("VTKHDF structured Points shape disagrees with dimensions".into());
        }
        points(&hdf_values(&ds)?)?
    } else {
        let mut coordinates = Vec::new();
        for (name, n) in ["XCoordinates", "YCoordinates", "ZCoordinates"]
            .iter()
            .zip(dims)
        {
            let ds = hdf_dataset(file, &format!("/VTKHDF/{name}"), budget)?;
            if ds.shape() != [n as u64] {
                return Err("VTKHDF coordinate shape disagrees with dimensions".into());
            }
            coordinates.push(hdf_values(&ds)?);
        }
        let mut positions = Vec::with_capacity(count);
        for &z in &coordinates[2] {
            for &y in &coordinates[1] {
                for &x in &coordinates[0] {
                    positions.push(Vec3::new(x, y, z));
                }
            }
        }
        positions
    };
    scientific::structured_scene(&positions, dims)
}

/// Open an in-memory classic or HDF5-backed `NetCDF` file.
fn open_netcdf(bytes: &[u8]) -> Result<NcFile> {
    NcFile::from_bytes_with_options(
        bytes,
        netcdf_reader::NcOpenOptions {
            chunk_cache_bytes: 8 * 1024 * 1024,
            chunk_cache_slots: 257,
            ..Default::default()
        },
    )
}

/// Read a bounded `NetCDF` numeric variable as scalar values.
fn nc_values(file: &NcFile, name: &str, budget: &mut Budget) -> Result<Vec<f64>> {
    let var = file.variable(name)?;
    budget.reserve(&var.shape())?;
    Ok(file.read_variable_as_f64(name)?.iter().copied().collect())
}

/// Read exact `NetCDF` integer connectivity without floating-point index conversion.
fn nc_ids(file: &NcFile, name: &str, budget: &mut Budget) -> Result<Vec<usize>> {
    let var = file.variable(name)?;
    budget.reserve(&var.shape())?;
    macro_rules! read {
        ($ty:ty) => {
            file.read_variable::<$ty>(name)?
                .iter()
                .map(|&n| {
                    usize::try_from(i128::from(n))
                        .map_err(|_| "NetCDF index is negative or overflows".into())
                })
                .collect()
        };
    }
    match var.dtype() {
        NcType::Byte => read!(i8),
        NcType::UByte => read!(u8),
        NcType::Short => read!(i16),
        NcType::UShort => read!(u16),
        NcType::Int => read!(i32),
        NcType::UInt => read!(u32),
        NcType::Int64 => read!(i64),
        NcType::UInt64 => read!(u64),
        _ => Err("NetCDF connectivity/count must use an integer type".into()),
    }
}

/// Require a named `NetCDF` dimension within its record limit.
fn dimension(file: &NcFile, name: &str, maximum: usize) -> Result<usize> {
    let n = usize::try_from(file.dimension(name)?.size)?;
    if n == 0 || n > maximum {
        return Err(format!("NetCDF {name} dimension must be in 1..{maximum}").into());
    }
    Ok(n)
}

/// Open an Exodus `NetCDF` container and extract its placed visible mesh boundary.
fn exodus(bytes: &[u8]) -> Result<Scene> {
    exodus_file(&open_netcdf(bytes)?)
}

/// Decode Exodus coordinates and active element-block topology, dropping shared interior faces.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Exodus dimensions and block IDs are checked against allocation limits before native indexing."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep Exodus coordinate and element-block validation together with its shared allocation budget."
)]
fn exodus_file(file: &NcFile) -> Result<Scene> {
    let dimensions = dimension(file, "num_dim", 3)?;
    let nodes = dimension(file, "num_nodes", MAX_POINTS)?;
    let block_count = dimension(file, "num_el_blk", MAX_PARTS)?;
    let element_count = dimension(file, "num_elem", MAX_CELLS)?;
    let mut budget = Budget::new();
    let coordinates = if file.variable("coord").is_ok() {
        if file.variable("coord")?.shape() != [dimensions as u64, nodes as u64] {
            return Err("Exodus coord shape disagrees with dimensions".into());
        }
        let values = nc_values(file, "coord", &mut budget)?;
        (0..dimensions)
            .map(|i| values[i * nodes..(i + 1) * nodes].to_vec())
            .collect::<Vec<_>>()
    } else {
        let mut coordinates = Vec::new();
        for name in ["coordx", "coordy", "coordz"].iter().take(dimensions) {
            if file.variable(name)?.shape() != [nodes as u64] {
                return Err("Exodus coordinate shape disagrees with node count".into());
            }
            coordinates.push(nc_values(file, name, &mut budget)?);
        }
        coordinates
    };
    let positions: Vec<_> = (0..nodes)
        .map(|i| {
            Vec3::new(
                coordinates[0][i],
                if dimensions > 1 {
                    coordinates[1][i]
                } else {
                    0.
                },
                if dimensions > 2 {
                    coordinates[2][i]
                } else {
                    0.
                },
            )
        })
        .collect();
    if positions.iter().any(|p| !p.is_finite()) {
        return Err("Exodus coordinates must be finite".into());
    }
    let statuses = if file.variable("eb_status").is_ok() {
        let statuses = nc_ids(file, "eb_status", &mut budget)?;
        if statuses.len() != block_count || statuses.iter().any(|&n| n > 1) {
            return Err("invalid Exodus element-block status".into());
        }
        statuses
    } else {
        vec![1; block_count]
    };
    let mut cells = Vec::new();
    let mut types = Vec::new();
    for (i, status) in statuses.into_iter().enumerate() {
        if status == 0 {
            continue;
        }
        let name = format!("connect{}", i + 1);
        let var = file.variable(&name)?;
        let shape = var.shape();
        if shape.len() != 2
            || shape[0] == 0
            || shape[0] > MAX_CELLS as u64
            || shape[1] > 8
            || shape[1] == 0
        {
            return Err(
                "Exodus connectivity requires fixed linear elements with 1..8 nodes".into(),
            );
        }
        let kind = var
            .attribute("elem_type")
            .and_then(|a| a.value.as_string())
            .ok_or("Exodus connectivity has no elem_type string")?;
        let kind = kind.trim_end_matches('\0').trim().to_ascii_uppercase();
        let vtk = match (kind.as_str(), shape[1]) {
            ("HEX" | "HEX8" | "HEXAHEDRON", 8) => 12,
            ("TETRA" | "TETRA4" | "TET4" | "TET", 4) => 10,
            ("WEDGE" | "WEDGE6" | "PRISM" | "PRISM6", 6) => 13,
            ("PYRAMID" | "PYRAMID5", 5) => 14,
            ("TRI" | "TRI3" | "TRIANGLE" | "TRIANGLE3" | "SHELL3" | "TRISHELL", 3) => 5,
            ("QUAD" | "QUAD4" | "SHELL" | "SHELL4", 4) => 9,
            ("BAR" | "BAR2" | "BEAM" | "BEAM2" | "TRUSS" | "TRUSS2" | "EDGE2", 2) => 3,
            ("SPHERE" | "CIRCLE" | "POINT", 1) => 1,
            _ => {
                return Err(format!(
                    "Exodus element type {kind} with {} nodes is not implemented",
                    shape[1]
                )
                .into());
            }
        };
        let ids = nc_ids(file, &name, &mut budget)?;
        for element in ids.chunks_exact(shape[1] as usize) {
            let element: Vec<_> = element
                .iter()
                .map(|&n| {
                    n.checked_sub(1)
                        .filter(|&n| n < nodes)
                        .ok_or("Exodus one-based node index out of range")
                })
                .collect::<std::result::Result<_, _>>()?;
            cells.push(element);
            types.push(vtk);
            if cells.len() > MAX_CELLS {
                return Err("Exodus mesh exceeds one million elements".into());
            }
        }
    }
    if cells.len() != element_count {
        return Err("Exodus active block element counts disagree with num_elem".into());
    }
    grid_scene(&positions, &cells, &types)
}

/// Recognize `NetCDF` primitive numeric storage types.
fn is_numeric(kind: &NcType) -> bool {
    matches!(
        kind,
        NcType::Byte
            | NcType::Short
            | NcType::Int
            | NcType::Float
            | NcType::Double
            | NcType::UByte
            | NcType::UShort
            | NcType::UInt
            | NcType::Int64
            | NcType::UInt64
    )
}

/// Recognize a `NetCDF` time coordinate from its dimension and metadata.
fn is_time(file: &NcFile, name: &str) -> bool {
    name.eq_ignore_ascii_case("time")
        || file.variable(name).is_ok_and(|var| {
            ["axis", "standard_name", "units"].iter().any(|&attr| {
                var.attribute(attr)
                    .and_then(|a| a.value.as_string())
                    .is_some_and(|s| {
                        s.eq_ignore_ascii_case("T")
                            || s.eq_ignore_ascii_case("time")
                            || (attr == "units" && s.contains(" since "))
                    })
            })
        })
}

/// Select the earliest time slice and retain the supported scalar spatial axes.
fn scalar_selection(file: &NcFile, var: &NcVariable) -> Result<Option<(NcSliceInfo, Vec<usize>)>> {
    if !is_numeric(var.dtype()) || var.is_coordinate_variable() {
        return Ok(None);
    }
    let mut shape = Vec::new();
    let mut selections = Vec::new();
    for d in var.dimensions() {
        if is_time(file, &d.name) {
            if d.size == 0 {
                return Err("NetCDF time dimension is empty".into());
            }
            selections.push(NcSliceInfoElem::Index(0));
        } else {
            shape.push(usize::try_from(d.size)?);
            selections.push(NcSliceInfoElem::Slice {
                start: 0,
                end: d.size,
                step: 1,
            });
        }
    }
    if !(2..=3).contains(&shape.len()) || shape.contains(&0) {
        return Ok(None);
    }
    Ok(Some((NcSliceInfo { selections }, shape)))
}

/// Decode supported coordinate-associated `NetCDF` scalar sheets or density volumes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "NetCDF axis dimensions are bounded before native allocation and conversion to preview coordinates."
)]
fn netcdf(bytes: &[u8]) -> Result<Scene> {
    let file = open_netcdf(bytes)?;
    if file.dimension("num_nodes").is_ok() && file.dimension("num_el_blk").is_ok() {
        return exodus_file(&file);
    }
    if file
        .variables()?
        .iter()
        .any(|v| v.attribute("cf_role").is_some())
    {
        return Err("NetCDF CF unstructured mesh topology is not implemented".into());
    }
    let mut metadata = BTreeSet::new();
    for var in file.variables()? {
        for name in [
            "bounds",
            "climatology",
            "grid_mapping",
            "coordinates",
            "ancillary_variables",
        ] {
            if let Some(names) = var.attribute(name).and_then(|a| a.value.as_string()) {
                for name in names.split_whitespace() {
                    metadata.insert(name.to_owned());
                }
            }
        }
    }
    let mut budget = Budget::new();
    for var in file.variables()? {
        if metadata.contains(var.name()) {
            continue;
        }
        let Some((selection, shape)) = scalar_selection(&file, var)? else {
            continue;
        };
        if let Some(coords) = var
            .attribute("coordinates")
            .and_then(|a| a.value.as_string())
        {
            for name in coords.split_whitespace() {
                if file.variable(name)?.ndim() > 1 {
                    return Err(
                        "NetCDF curvilinear auxiliary coordinates are not implemented".into(),
                    );
                }
            }
        }
        budget.reserve(&shape.iter().map(|&n| n as u64).collect::<Vec<_>>())?;
        let values = file.read_variable_slice_unpacked_masked(var.name(), &selection)?;
        let values: Vec<f64> = values.iter().copied().collect();
        if values.iter().any(|v| v.is_infinite()) {
            return Err("NetCDF scalar field contains infinite values".into());
        }
        let mut coords = Vec::new();
        for d in var.dimensions().iter().filter(|d| !is_time(&file, &d.name)) {
            let axis: Vec<f64> = if let Ok(coord) = file.coordinate_variable(&d.name) {
                budget.reserve(&coord.shape())?;
                file.read_variable_unpacked(coord.name())?
                    .iter()
                    .copied()
                    .collect()
            } else {
                (0..d.size).map(|i| i as f64).collect()
            };
            if axis.len() != d.size as usize || axis.iter().any(|n| !n.is_finite()) {
                return Err("NetCDF axis coordinate is invalid".into());
            }
            coords.push(axis);
        }
        // NetCDF uses C array order: the last axis changes fastest (world X).
        coords.reverse();
        if shape.len() == 2 {
            return scalar_sheet(&coords[0], &coords[1], &values);
        }
        let (mut minimum, mut maximum) = (f64::INFINITY, f64::NEG_INFINITY);
        for &value in values.iter().filter(|n| n.is_finite()) {
            minimum = minimum.min(value);
            maximum = maximum.max(value);
        }
        if !minimum.is_finite() {
            return Err("NetCDF scalar field is entirely missing".into());
        }
        let missing = minimum - (maximum - minimum).abs().max(minimum.abs()).max(1.);
        if !missing.is_finite() {
            return Err("NetCDF scalar range overflows missing-value baseline".into());
        }
        let values = values
            .into_iter()
            .map(|n| if n.is_nan() { missing } else { n })
            .collect();
        let coords: [Vec<f64>; 3] = coords
            .try_into()
            .map_err(|_| "NetCDF spatial axis count disagrees")?;
        return volume::density_rectilinear_scene(&coords, values);
    }
    Err("NetCDF file contains no supported 2D or 3D numeric scalar grid".into())
}

/// Triangulate a scalar-colored nonuniform coordinate sheet while retaining masked holes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Scalar color channels are clamped to the byte range and intentionally rounded before quantization."
)]
fn scalar_sheet(x: &[f64], y: &[f64], values: &[f64]) -> Result<Scene> {
    if x.len() < 2 || y.len() < 2 || x.len().checked_mul(y.len()) != Some(values.len()) {
        return Err("NetCDF scalar sheet needs two axes with at least two samples".into());
    }
    if (x.len() - 1)
        .checked_mul(y.len() - 1)
        .is_none_or(|n| n > MAX_CELLS)
    {
        return Err("NetCDF scalar sheet exceeds one million cells".into());
    }
    for axis in [x, y] {
        let increasing = axis[1] > axis[0];
        if axis.iter().any(|v| !v.is_finite())
            || axis.windows(2).any(|w| {
                if increasing {
                    w[1] <= w[0]
                } else {
                    w[1] >= w[0]
                }
            })
        {
            return Err(
                "NetCDF scalar sheet coordinates must be finite and strictly monotonic".into(),
            );
        }
    }
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for &value in values.iter().filter(|n| n.is_finite()) {
        low = low.min(value);
        high = high.max(value);
    }
    let mut scene = Scene::default();
    for j in 0..y.len() - 1 {
        for i in 0..x.len() - 1 {
            let ids = [
                i + x.len() * j,
                i + 1 + x.len() * j,
                i + 1 + x.len() * (j + 1),
                i + x.len() * (j + 1),
            ];
            if ids.iter().any(|&k| !values[k].is_finite()) {
                continue;
            }
            let corners = [
                Vec3::new(x[i], y[j], 0.),
                Vec3::new(x[i + 1], y[j], 0.),
                Vec3::new(x[i + 1], y[j + 1], 0.),
                Vec3::new(x[i], y[j + 1], 0.),
            ];
            let value = ids.iter().map(|&k| values[k] * 0.25).sum::<f64>();
            let v = if high > low {
                ((value - low) / (high - low)).clamp(0., 1.)
            } else {
                0.5
            };
            let color = Some([
                (40. + 210. * v) as u8,
                (75. + 155. * v) as u8,
                (220. - 160. * v) as u8,
            ]);
            for vertices in [[0, 1, 2], [0, 2, 3]] {
                scene.triangles.push(Triangle {
                    vertices: vertices.map(|i| corners[i]),
                    color,
                });
            }
        }
    }
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hdf5_writer::{AttributeBuilder, DatasetBuilder, Hdf5Builder, WriteOptions};
    use netcdf_writer::{NcAttrValue, NcFileBuilder, NcWriteFormat, NcWriteOptions};

    fn hdf_builder(kind: &str) -> Hdf5Builder {
        Hdf5Builder::new()
            .group_attribute("VTKHDF", AttributeBuilder::fixed_string("Type", kind))
            .group_attribute(
                "VTKHDF",
                AttributeBuilder::vector("Version", &[2i32, 0]).unwrap(),
            )
    }
    fn hdf_bytes(builder: Hdf5Builder) -> Vec<u8> {
        builder
            .into_plan()
            .unwrap()
            .encode(WriteOptions::default())
            .unwrap()
    }
    fn hdf<T: hdf5_writer::H5WriteElement>(
        name: &str,
        shape: &[u64],
        data: &[T],
    ) -> DatasetBuilder {
        DatasetBuilder::typed_data(name, shape.to_vec(), data).unwrap()
    }
    fn tetrahedra_hdf(bad_index: bool) -> Vec<u8> {
        let conn = [
            0i64,
            1,
            2,
            3,
            0,
            2,
            1,
            4,
            0,
            1,
            2,
            if bad_index { 99 } else { 3 },
        ];
        let points = [
            0f32, 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., -1., 3., 0., 0., 4., 0., 0.,
            3., 1., 0., 3., 0., 1.,
        ];
        let compressed = DatasetBuilder::typed::<f32>("VTKHDF/Points", vec![9, 3])
            .data(&points)
            .unwrap()
            .chunked(vec![3, 3])
            .filter(hdf5_writer::FilterDescription {
                id: 2,
                name: None,
                client_data: vec![4],
            })
            .filter(hdf5_writer::FilterDescription {
                id: 1,
                name: None,
                client_data: vec![6],
            });
        hdf_bytes(
            hdf_builder("UnstructuredGrid")
                .dataset(compressed)
                .dataset(hdf("VTKHDF/NumberOfPoints", &[2], &[5i64, 4]))
                .dataset(hdf("VTKHDF/NumberOfCells", &[2], &[2i32, 1]))
                .dataset(hdf("VTKHDF/NumberOfConnectivityIds", &[2], &[8u64, 4]))
                .dataset(hdf("VTKHDF/Offsets", &[5], &[0i64, 4, 8, 0, 4]))
                .dataset(hdf("VTKHDF/Connectivity", &[12], &conn))
                .dataset(hdf("VTKHDF/Types", &[3], &[10u8, 10, 10])),
        )
    }

    #[test]
    fn hdf_partition_local_ids_compression_and_shared_faces() {
        let scene = vtkhdf(&tetrahedra_hdf(false)).unwrap();
        assert_eq!(scene.triangles.len(), 10);
        assert_eq!(
            scene
                .positions()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max),
            4.
        );
        assert!(
            vtkhdf(&tetrahedra_hdf(true))
                .unwrap_err()
                .to_string()
                .contains("index")
        );
    }

    #[test]
    fn hdf_big_endian_numeric_geometry_and_metadata() {
        let order = hdf5_writer::ByteOrder::BigEndian;
        let version = AttributeBuilder::new(
            "Version",
            Datatype::FixedPoint {
                size: 4,
                signed: true,
                byte_order: order,
            },
            vec![2],
            [2i32.to_be_bytes(), 0i32.to_be_bytes()].concat(),
        );
        let mut b = Hdf5Builder::new()
            .group_attribute("VTKHDF", version)
            .group_attribute(
                "VTKHDF",
                AttributeBuilder::fixed_string("Type", "UnstructuredGrid"),
            )
            .dataset(
                DatasetBuilder::typed_with_order::<f64>("VTKHDF/Points", vec![3, 3], order)
                    .data(&[0., 0., 0., 2., 0., 0., 0., 3., 0.])
                    .unwrap(),
            );
        for (name, values) in [
            ("NumberOfPoints", vec![3i64]),
            ("NumberOfCells", vec![1]),
            ("NumberOfConnectivityIds", vec![3]),
            ("Offsets", vec![0, 3]),
            ("Connectivity", vec![0, 1, 2]),
            ("Types", vec![5]),
        ] {
            b = b.dataset(
                DatasetBuilder::typed_with_order::<i64>(
                    format!("VTKHDF/{name}"),
                    vec![values.len() as u64],
                    order,
                )
                .data(&values)
                .unwrap(),
            );
        }
        let bytes = b
            .into_plan()
            .unwrap()
            .encode(WriteOptions {
                byte_order: order,
                ..Default::default()
            })
            .unwrap();
        let scene = vtkhdf(&bytes).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(
            (scene.triangles[0].vertices[1] - scene.triangles[0].vertices[0])
                .cross(scene.triangles[0].vertices[2] - scene.triangles[0].vertices[0])
                .length(),
            6.
        );
    }

    #[test]
    #[allow(
        clippy::cast_sign_loss,
        reason = "The authored chunk fixture uses small nonnegative array positions for its byte-count records."
    )]
    fn hdf_filtered_512_byte_chunks_use_variable_size_fields_in_fixed_and_tree_indexes() {
        let values: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        for max_shape in [
            vec![16, 16, 16],
            vec![u64::MAX, 16, 16],
            vec![u64::MAX, u64::MAX, 16],
        ] {
            let ds = DatasetBuilder::typed_data("density", vec![16, 16, 16], &values)
                .unwrap()
                .max_shape(max_shape)
                .chunked(vec![8, 8, 8])
                .filter(hdf5_writer::FilterDescription {
                    id: 1,
                    name: None,
                    client_data: vec![6],
                })
                .filter(hdf5_writer::FilterDescription {
                    id: 3,
                    name: None,
                    client_data: vec![],
                });
            let bytes = hdf_bytes(Hdf5Builder::new().dataset(ds));
            let file = Hdf5File::from_bytes(&bytes).unwrap();
            let decoded: Vec<u8> = file
                .dataset("density")
                .unwrap()
                .read_array::<u8>()
                .unwrap()
                .iter()
                .copied()
                .collect();
            assert_eq!(decoded, values);
        }
    }

    #[test]
    #[allow(
        clippy::cast_possible_wrap,
        reason = "The authored topology has fewer than a dozen entries, so its i32 connectivity casts are exact."
    )]
    fn hdf_polydata_includes_actual_polygons_lines_strips_points() {
        let mut b = hdf_builder("PolyData")
            .dataset(hdf(
                "VTKHDF/Points",
                &[5, 3],
                &[0f64, 0., 0., 2., 0., 0., 2., 2., 0., 1., 1., 0., 0., 2., 0.],
            ))
            .dataset(hdf("VTKHDF/NumberOfPoints", &[1], &[5i64]));
        for (category, ids) in [
            ("Polygons", vec![0i32, 1, 2, 3, 4]),
            ("Lines", vec![0, 1, 2, 3]),
            ("Strips", vec![0, 1, 4, 3]),
            ("Vertices", vec![3]),
        ] {
            b = b
                .dataset(hdf(
                    &format!("VTKHDF/{category}/NumberOfCells"),
                    &[1],
                    &[1i64],
                ))
                .dataset(hdf(
                    &format!("VTKHDF/{category}/NumberOfConnectivityIds"),
                    &[1],
                    &[ids.len() as i64],
                ))
                .dataset(hdf(
                    &format!("VTKHDF/{category}/Offsets"),
                    &[2],
                    &[0i64, ids.len() as i64],
                ))
                .dataset(hdf(
                    &format!("VTKHDF/{category}/Connectivity"),
                    &[ids.len() as u64],
                    &ids,
                ));
        }
        let scene = vtkhdf(&hdf_bytes(b)).unwrap();
        assert_eq!(
            (scene.triangles.len(), scene.lines.len(), scene.points.len()),
            (4, 3, 1)
        );
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "The test verifies exact integral-coordinate output from deterministic authored topology."
    )]
    fn hdf_polyhedron_and_linear_cells_share_interior_faces_and_require_closed_shells() {
        let points = [
            0f64, 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0., 0., 0., 1., 1., 0., 1., 1., 1., 1.,
            0., 1., 1., 2., 0., 0., 2., 1., 0., 2., 0., 1., 2., 1., 1.,
        ];
        let faces = [
            0i64, 3, 2, 1, 4, 5, 6, 7, 0, 1, 5, 4, 1, 2, 6, 5, 2, 3, 7, 6, 3, 0, 4, 7,
        ];
        for bad in [false, true] {
            let refs = [0i64, 1, 2, 3, 4, if bad { 4 } else { 5 }];
            let mut b =
                hdf_builder("UnstructuredGrid").dataset(hdf("VTKHDF/Points", &[12, 3], &points));
            for (name, values) in [
                ("NumberOfPoints", vec![12i64]),
                ("NumberOfCells", vec![2]),
                ("NumberOfConnectivityIds", vec![16]),
                ("Offsets", vec![0, 8, 16]),
                (
                    "Connectivity",
                    vec![0, 1, 2, 3, 4, 5, 6, 7, 1, 8, 9, 2, 5, 10, 11, 6],
                ),
                ("Types", vec![42, 12]),
                ("NumberOfFaces", vec![6]),
                ("NumberOfFaceConnectivityIds", vec![24]),
                ("NumberOfPolyhedronToFaceIds", vec![6]),
                ("FaceOffsets", vec![0, 4, 8, 12, 16, 20, 24]),
                ("PolyhedronOffsets", vec![0, 6, 6]),
                ("PolyhedronToFaces", refs.to_vec()),
                ("FaceConnectivity", faces.to_vec()),
            ] {
                b = b.dataset(hdf(
                    &format!("VTKHDF/{name}"),
                    &[values.len() as u64],
                    &values,
                ));
            }
            let result = vtkhdf(&hdf_bytes(b));
            if bad {
                assert!(result.is_err());
            } else {
                let scene = result.unwrap();
                assert_eq!(scene.triangles.len(), 20);
                assert!(
                    scene
                        .triangles
                        .iter()
                        .all(|t| t.vertices.iter().any(|p| p.x != 1.))
                );
            }
        }
    }

    #[test]
    fn hdf_image_uses_nonuniform_scalar_mask_and_oriented_origin() {
        let bytes = hdf_bytes(
            hdf_builder("ImageData")
                .group_attribute(
                    "VTKHDF",
                    AttributeBuilder::vector("WholeExtent", &[-2i32, 0, 1, 2, 4, 5]).unwrap(),
                )
                .group_attribute(
                    "VTKHDF",
                    AttributeBuilder::vector("Origin", &[10f64, 20., 30.]).unwrap(),
                )
                .group_attribute(
                    "VTKHDF",
                    AttributeBuilder::vector("Spacing", &[-3f64, 2., 1.]).unwrap(),
                )
                .group_attribute(
                    "VTKHDF",
                    AttributeBuilder::vector("Direction", &[1f64, 0., 0., 0., 1., 0., 0., 0., 1.])
                        .unwrap(),
                )
                .dataset(
                    hdf(
                        "VTKHDF/PointData/Density",
                        &[2, 2, 3],
                        &[0u16, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    )
                    .attribute(AttributeBuilder::fixed_string("Attribute", "Scalars")),
                ),
        );
        let scene = vtkhdf(&bytes).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        let xs: Vec<_> = scene.positions().map(|p| p.x).collect();
        assert_eq!(xs.iter().copied().fold(f64::INFINITY, f64::min), 11.5);
        assert_eq!(xs.iter().copied().fold(f64::NEG_INFINITY, f64::max), 14.5);
        assert_eq!(
            scene.positions().map(|p| p.z).fold(f64::INFINITY, f64::min),
            33.5
        );
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "The test verifies exact integral-coordinate output from deterministic authored topology."
    )]
    fn hdf_rectilinear_actual_boundary_preserves_intermediate_coordinates() {
        let bytes = hdf_bytes(
            hdf_builder("RectilinearGrid")
                .group_attribute(
                    "VTKHDF",
                    AttributeBuilder::vector("Dimensions", &[3i64, 2, 2]).unwrap(),
                )
                .dataset(hdf("VTKHDF/XCoordinates", &[3], &[0f64, 1., 4.]))
                .dataset(hdf("VTKHDF/YCoordinates", &[2], &[0f64, 2.]))
                .dataset(hdf("VTKHDF/ZCoordinates", &[2], &[0f64, 3.])),
        );
        let scene = vtkhdf(&bytes).unwrap();
        assert_eq!(scene.triangles.len(), 20);
        assert!(scene.positions().any(|p| p.x == 1.));
    }

    fn exodus_bytes(format: NcWriteFormat, bad_index: bool) -> Vec<u8> {
        let mut b = NcFileBuilder::new();
        b.add_dimension("num_dim", 3).unwrap();
        let nodes = b.add_dimension("num_nodes", 5).unwrap();
        b.add_dimension("num_elem", 2).unwrap();
        let blocks = b.add_dimension("num_el_blk", 2).unwrap();
        let status = b.add_variable::<i32>("eb_status", &[blocks]).unwrap();
        b.write_variable(status, &[1, 1]).unwrap();
        for (name, values) in [
            ("coordx", [0f64, 1., 0., 0., 0.]),
            ("coordy", [0., 0., 1., 0., 0.]),
            ("coordz", [0., 0., 0., 1., -1.]),
        ] {
            let v = b.add_variable::<f64>(name, &[nodes]).unwrap();
            b.write_variable(v, &values).unwrap();
        }
        for (i, conn) in [[1i32, 2, 3, 4], [1, 3, 2, if bad_index { 99 } else { 5 }]]
            .iter()
            .enumerate()
        {
            let elems = b
                .add_dimension(format!("num_el_in_blk{}", i + 1), 1)
                .unwrap();
            let corners = b
                .add_dimension(format!("num_nod_per_el{}", i + 1), 4)
                .unwrap();
            let v = b
                .add_variable::<i32>(format!("connect{}", i + 1), &[elems, corners])
                .unwrap();
            b.add_variable_attribute(v, "elem_type", NcAttrValue::Chars("TETRA4".into()))
                .unwrap();
            b.write_variable(v, conn).unwrap();
        }
        b.to_vec(NcWriteOptions { format }).unwrap().1
    }

    #[test]
    fn exodus_cdf1_cdf2_cdf5_nc4_and_internal_block_faces() {
        for format in [
            NcWriteFormat::Classic,
            NcWriteFormat::Offset64,
            NcWriteFormat::Cdf5,
            NcWriteFormat::Nc4,
        ] {
            let scene = exodus(&exodus_bytes(format, false)).unwrap();
            assert_eq!(scene.triangles.len(), 6);
            assert!(
                scene
                    .triangles
                    .iter()
                    .all(|t| t.vertices.iter().any(|p| p.z != 0.))
            );
            assert!(
                exodus(&exodus_bytes(format, true))
                    .unwrap_err()
                    .to_string()
                    .contains("index")
            );
        }
    }

    fn scalar_bytes(format: NcWriteFormat) -> Vec<u8> {
        let mut builder = NcFileBuilder::new();
        let time = builder.add_dimension("time", 2).unwrap();
        let z_axis = builder.add_dimension("z", 2).unwrap();
        let y_axis = builder.add_dimension("y", 2).unwrap();
        let x_axis = builder.add_dimension("x", 3).unwrap();
        for (name, dim, values) in [
            ("x", x_axis, vec![0f64, 3., 9.]),
            ("y", y_axis, vec![0., 2.]),
            ("z", z_axis, vec![4., 1.]),
        ] {
            let variable = builder.add_variable::<f64>(name, &[dim]).unwrap();
            builder.write_variable(variable, &values).unwrap();
        }
        let variable = builder
            .add_variable::<i16>("density", &[time, z_axis, y_axis, x_axis])
            .unwrap();
        builder
            .add_variable_attribute(variable, "scale_factor", NcAttrValue::Doubles(vec![2.]))
            .unwrap();
        builder
            .set_variable_fill_value(variable, -32767i16)
            .unwrap();
        let mut values = vec![0i16; 24];
        values[1] = 5;
        values[3] = -32767;
        values[12..].fill(5);
        builder.write_variable(variable, &values).unwrap();
        if matches!(format, NcWriteFormat::Nc4) {
            builder
                .set_variable_chunking(variable, vec![1, 2, 2, 3])
                .unwrap();
            builder
                .set_variable_deflate(variable, Some(6), true)
                .unwrap();
        }
        builder.to_vec(NcWriteOptions { format }).unwrap().1
    }

    #[test]
    fn netcdf_density_first_time_packing_fill_nonuniform_descending_axes() {
        for format in [
            NcWriteFormat::Classic,
            NcWriteFormat::Offset64,
            NcWriteFormat::Cdf5,
            NcWriteFormat::Nc4,
        ] {
            let scene = netcdf(&scalar_bytes(format)).unwrap();
            assert_eq!(scene.triangles.len(), 12);
            assert_eq!(
                scene.positions().map(|p| p.x).fold(f64::INFINITY, f64::min),
                1.5
            );
            assert_eq!(
                scene
                    .positions()
                    .map(|p| p.x)
                    .fold(f64::NEG_INFINITY, f64::max),
                6.
            );
            let center = Vec3::new(3.75, 0., 4.);
            for t in &scene.triangles {
                let normal = (t.vertices[1] - t.vertices[0]).cross(t.vertices[2] - t.vertices[0]);
                let face = (t.vertices[0] + t.vertices[1] + t.vertices[2]) * (1. / 3.);
                assert!(
                    normal.dot(face - center) > 0.,
                    "reflected density face points inward"
                );
            }
        }
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "The test verifies exact integral-coordinate output from deterministic authored topology."
    )]
    fn netcdf_sheet_keeps_holes_and_scalar_colors() {
        let scene = scalar_sheet(
            &[0., 1., 3.],
            &[0., 2., 4.],
            &[0., 1., 2., 3., 4., 5., f64::NAN, 7., 8.],
        )
        .unwrap();
        assert_eq!(scene.triangles.len(), 6);
        assert!(scene.positions().any(|p| p.x == 3.));
        assert_ne!(scene.triangles[0].color, scene.triangles[4].color);
    }

    #[test]
    fn netcdf_coordinate_bounds_are_metadata_and_curvilinear_axes_fail() {
        let mut b = NcFileBuilder::new();
        let x = b.add_dimension("x", 2).unwrap();
        let y = b.add_dimension("y", 2).unwrap();
        let nv = b.add_dimension("nv", 2).unwrap();
        let c = b.add_variable::<f64>("x", &[x]).unwrap();
        b.add_variable_attribute(c, "bounds", NcAttrValue::Chars("x_bounds".into()))
            .unwrap();
        b.write_variable(c, &[0., 3.]).unwrap();
        let bounds = b.add_variable::<f64>("x_bounds", &[x, nv]).unwrap();
        b.write_variable(bounds, &[-1., 1., 2., 4.]).unwrap();
        let data = b.add_variable::<f64>("temperature", &[y, x]).unwrap();
        b.write_variable(data, &[1., 2., 3., 4.]).unwrap();
        let bytes = b.to_vec(NcWriteOptions::default()).unwrap().1;
        let scene = netcdf(&bytes).unwrap();
        assert_eq!(scene.triangles.len(), 2);
        assert_eq!(
            scene
                .positions()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max),
            3.
        );
        let latitude = b.add_variable::<f64>("latitude", &[y, x]).unwrap();
        b.write_variable(latitude, &[10., 10., 20., 20.]).unwrap();
        b.add_variable_attribute(data, "coordinates", NcAttrValue::Chars("latitude".into()))
            .unwrap();
        let bytes = b.to_vec(NcWriteOptions::default()).unwrap().1;
        assert!(
            netcdf(&bytes)
                .unwrap_err()
                .to_string()
                .contains("curvilinear")
        );
    }

    #[test]
    fn container_schema_bounds_and_malformed_inputs_fail() {
        assert!(vtkhdf(b"bad header").is_err());
        assert!(netcdf(b"CDF\x01").is_err());
        assert!(partition_cells(&[0, 4, 3], &[0, 1, 2], 3).is_err());
        assert!(Budget::new().reserve(&[u64::MAX, 2]).is_err());
        assert!(Budget::new().reserve(&[MAX_SAMPLES as u64 + 1]).is_err());
        let b = hdf_builder("HyperTreeGrid").dataset(hdf("VTKHDF/Unused", &[1], &[0u8]));
        assert!(
            vtkhdf(&hdf_bytes(b))
                .unwrap_err()
                .to_string()
                .contains("not implemented")
        );
        assert!(
            volume::density_rectilinear_scene(
                &[vec![0., 0.], vec![0., 1.], vec![0., 1.]],
                vec![1.; 8]
            )
            .is_err()
        );
    }

    #[test]
    fn independent_reference_fixtures_when_downloaded() {
        let root = Path::new("/tmp/catwalk-container-fixtures");
        for (name, hdf) in [
            ("hexahedron.vtkhdf", true),
            ("sphere_onepiece.vtkhdf", true),
            ("test_poly_data.vtkhdf", true),
            ("mandelbrot-vti.vtkhdf", true),
            ("ADeDA.e", false),
            ("block_crush_1.ex2", false),
        ] {
            let Ok(bytes) = std::fs::read(root.join(name)) else {
                continue;
            };
            let result = if hdf { vtkhdf(&bytes) } else { exodus(&bytes) };
            let scene = result.unwrap_or_else(|error| {
                if hdf {
                    let f = Hdf5File::from_bytes(&bytes).unwrap();
                    for group in f.group("VTKHDF").unwrap().groups().unwrap() {
                        eprintln!("GROUP {} {:?}", group.name(), group.attributes().unwrap());
                    }
                    for ds in f.group("VTKHDF").unwrap().datasets().unwrap() {
                        let ds = f.adapt_dataset(ds);
                        let values = if ds.num_elements().unwrap_or(u64::MAX) < 100 {
                            hdf_values(&ds).ok()
                        } else {
                            None
                        };
                        eprintln!("{} {:?} {:?}", ds.name(), ds.shape(), values);
                    }
                }
                panic!("reference fixture {name}: {error}")
            });
            scene.validate().unwrap();
        }
    }
}
