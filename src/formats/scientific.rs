//! Geometry subsets of the documented VTK formats; unsupported encodings fail explicitly.
use crate::Result;
use crate::scene::{Line, Point, Scene, Triangle, Vec3};
use roxmltree::Node;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Maximum geometry or attribute item count accepted by this importer.
const MAX_ITEMS: usize = 5_000_000;

#[path = "scientific_arrays.rs"]
mod arrays;
#[path = "scientific_legacy.rs"]
mod legacy;
#[path = "scientific_quadratic.rs"]
mod quadratic;
use arrays::XmlData;

/// Read a VTK dataset and resolve nested package-local multiblock children.
///
/// # Errors
/// Returns an error for malformed or unsupported datasets, invalid topology, nonfinite
/// coordinates, unsafe/cyclic references or resource limits.
pub(crate) fn load(path: &Path) -> Result<Scene> {
    ReadContext::default().load(path, 0)
}

#[derive(Default)]
/// Recursive VTK multiblock traversal state and aggregate resource accounting.
struct ReadContext {
    /// Canonical dataset paths in the current recursion chain.
    active: HashSet<PathBuf>,
    /// Aggregate bytes read from referenced datasets.
    total_bytes: u64,
    /// Number of dataset files visited so far.
    files: usize,
}

impl ReadContext {
    /// Read a VTK dataset and resolve nested package-local multiblock children.
    fn load(&mut self, path: &Path, depth: usize) -> Result<Scene> {
        if depth > 32 || self.files >= 256 {
            return Err("VTK composite dataset exceeds nesting or reference limit".into());
        }
        let path = path.canonicalize()?;
        if !self.active.insert(path.clone()) {
            return Err("VTK composite dataset contains cyclic file references".into());
        }
        self.files += 1;
        let result = (|| {
            let bytes = super::read_file(&path)?;
            self.total_bytes = self
                .total_bytes
                .checked_add(bytes.len() as u64)
                .ok_or("VTK composite byte count overflow")?;
            if self.total_bytes > super::MAX_INPUT_BYTES {
                return Err(
                    "VTK composite dataset exceeds the aggregate 512 MiB input limit".into(),
                );
            }
            match super::extension(&path)?.as_str() {
                "vtk" => legacy::parse(&bytes),
                "vtp" | "vtu" | "vtr" | "vts" => parse_vtk_xml_bytes(&bytes),
                "vti" => super::volume::vti_scene(&bytes),
                "vtm" => self.multiblock(
                    &path,
                    std::str::from_utf8(&bytes)?.trim_start_matches('\u{feff}'),
                    depth + 1,
                ),
                _ => Err("VTK composite references an unsupported geometry extension".into()),
            }
        })();
        self.active.remove(&path);
        result
    }

    /// Parse the root VTK multiblock document and combine its child datasets.
    fn multiblock(&mut self, path: &Path, text: &str, depth: usize) -> Result<Scene> {
        let document = roxmltree::Document::parse(text)?;
        let root = document.root_element();
        if !root.has_tag_name("VTKFile") || root.attribute("type") != Some("vtkMultiBlockDataSet") {
            return Err("expected a VTK XML multiblock dataset".into());
        }
        let mut scene = Scene::default();
        self.blocks(
            child(root, "vtkMultiBlockDataSet")?,
            path,
            depth,
            &mut scene,
        )?;
        scene.validate()?;
        Ok(scene)
    }

    /// Recursively visit multiblock entries while enforcing path, depth and geometry budgets.
    fn blocks(
        &mut self,
        node: Node<'_, '_>,
        path: &Path,
        depth: usize,
        scene: &mut Scene,
    ) -> Result<()> {
        if depth > 32 {
            return Err("VTK composite block hierarchy exceeds nesting limit".into());
        }
        for entry in node.children().filter(Node::is_element) {
            match entry.tag_name().name() {
                "Block" => self.blocks(entry, path, depth + 1, scene)?,
                "DataSet" => {
                    let Some(file) = entry.attribute("file").filter(|value| !value.is_empty())
                    else {
                        continue; // VTK represents null dataset slots without a filename.
                    };
                    let parent = path.parent().ok_or("VTK composite path has no directory")?;
                    let resource = super::relative_resource(parent, file)?;
                    let mut child = self.load(&resource, depth)?;
                    if scene.triangles.len() + child.triangles.len() > MAX_ITEMS
                        || scene.points.len() + child.points.len() > MAX_ITEMS
                        || scene.lines.len() + child.lines.len() > MAX_ITEMS
                    {
                        return Err("VTK composite output exceeds geometry limit".into());
                    }
                    scene.triangles.append(&mut child.triangles);
                    scene.points.append(&mut child.points);
                    scene.lines.append(&mut child.lines);
                }
                "FieldData" => {} // Dataset metadata does not alter child placement/topology.
                _ => return Err("unsupported VTK composite hierarchy element".into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn parse_vtk(text: &str) -> Result<Scene> {
    legacy::parse(text.as_bytes())
}

/// Require the named direct XML child element.
fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Result<Node<'a, 'input>> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .ok_or_else(|| format!("missing VTK XML {name}").into())
}

/// Read a bounded nonnegative XML geometry count.
fn count_attribute(node: Node<'_, '_>, name: &str) -> Result<usize> {
    let count: usize = node
        .attribute(name)
        .ok_or_else(|| format!("missing VTK XML {name}"))?
        .parse()?;
    if count > MAX_ITEMS {
        return Err("VTK XML count exceeds geometry limit".into());
    }
    Ok(count)
}

/// Require a named VTK `DataArray` child.
fn named_array<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Result<Node<'a, 'input>> {
    node.children()
        .find(|n| n.has_tag_name("DataArray") && n.attribute("Name") == Some(name))
        .ok_or_else(|| format!("missing VTK XML {name} array").into())
}

/// Validate XML offsets and split connectivity into individual cells.
fn xml_cells(node: Node<'_, '_>, count: usize, data: &XmlData<'_>) -> Result<Vec<Vec<usize>>> {
    let connectivity = data.indices(named_array(node, "connectivity")?, MAX_ITEMS)?;
    let offsets = data.indices(named_array(node, "offsets")?, count)?;
    if offsets.len() != count {
        return Err("VTK XML offsets disagree with cell count".into());
    }
    let mut start = 0usize;
    let mut cells = Vec::new();
    for end in offsets {
        if end < start || end > connectivity.len() {
            return Err("invalid VTK XML connectivity offset".into());
        }
        cells.push(connectivity[start..end].to_vec());
        start = end;
    }
    if start != connectivity.len() {
        return Err("unused VTK XML connectivity indices".into());
    }
    Ok(cells)
}

/// Decode the declared point tuples and reject count or component mismatches.
fn xml_positions(piece: Node<'_, '_>, count: usize, data: &XmlData<'_>) -> Result<Vec<Vec3>> {
    let points_node = child(piece, "Points")?;
    let mut arrays = points_node
        .children()
        .filter(|n| n.has_tag_name("DataArray"));
    let points_array = arrays.next().ok_or("missing VTK XML point DataArray")?;
    if arrays.next().is_some() {
        return Err("VTK XML Points must contain one coordinate array".into());
    }
    if points_array.attribute("NumberOfComponents").unwrap_or("3") != "3" {
        return Err("VTK XML point array must have three components".into());
    }
    let values = data.floats(
        points_array,
        count.checked_mul(3).ok_or("VTK point count overflow")?,
    )?;
    if values.len() != count * 3 {
        return Err("VTK XML point count disagrees with coordinates".into());
    }
    Ok(values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
        .collect())
}

/// Parse ordered six-value structured-grid extents.
fn xml_extent(node: Node<'_, '_>, attribute: &str) -> Result<[i64; 6]> {
    let values = node
        .attribute(attribute)
        .ok_or("missing VTK structured extent")?
        .split_whitespace()
        .map(str::parse::<i64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let extent: [i64; 6] = values
        .try_into()
        .map_err(|_| "VTK structured extent needs six integers")?;
    for axis in 0..3 {
        if extent[axis * 2] > extent[axis * 2 + 1] {
            return Err("VTK structured extent is reversed or empty".into());
        }
    }
    Ok(extent)
}

/// Convert inclusive extents into checked nonzero point dimensions.
fn structured_dimensions(extent: [i64; 6]) -> Result<[usize; 3]> {
    let mut dimensions = [0usize; 3];
    let mut count = 1usize;
    for axis in 0..3 {
        let size = extent[axis * 2 + 1]
            .checked_sub(extent[axis * 2])
            .and_then(|n| n.checked_add(1))
            .ok_or("VTK structured extent overflow")?;
        dimensions[axis] = usize::try_from(size)?;
        count = count
            .checked_mul(dimensions[axis])
            .ok_or("VTK structured point count overflow")?;
        if count > MAX_ITEMS {
            return Err("VTK structured point count exceeds geometry limit".into());
        }
    }
    Ok(dimensions)
}

/// Expand rectilinear or explicitly positioned structured-grid points.
#[allow(
    clippy::float_cmp,
    reason = "Exact duplicate axis coordinates violate strict monotonicity and must not be hidden by a tolerance."
)]
fn xml_structured(root: Node<'_, '_>, dataset: &str, data: &XmlData<'_>) -> Result<Scene> {
    let grid = child(root, dataset)?;
    let extent = xml_extent(grid, "WholeExtent")?;
    let dimensions = structured_dimensions(extent)?;
    let mut pieces = grid.children().filter(|n| n.has_tag_name("Piece"));
    let piece = pieces.next().ok_or("VTK structured grid has no piece")?;
    if pieces.next().is_some() || xml_extent(piece, "Extent")? != extent {
        return Err(
            "VTK structured reader currently requires one piece covering WholeExtent".into(),
        );
    }
    for array in piece
        .descendants()
        .filter(|n| n.has_tag_name("DataArray") && n.attribute("Name") == Some("vtkGhostType"))
    {
        if data
            .indices(array, MAX_ITEMS)?
            .iter()
            .any(|&value| value != 0)
        {
            return Err("VTK structured blanking and ghost cells are not implemented".into());
        }
    }
    let positions = if dataset == "StructuredGrid" {
        xml_positions(piece, dimensions.iter().product(), data)?
    } else {
        let coordinates = child(piece, "Coordinates")?;
        let arrays: Vec<_> = coordinates
            .children()
            .filter(|n| n.has_tag_name("DataArray"))
            .collect();
        if arrays.len() != 3 {
            return Err("VTK rectilinear grid needs three coordinate arrays".into());
        }
        let mut axes = Vec::new();
        for (array, &size) in arrays.into_iter().zip(&dimensions) {
            if array.attribute("NumberOfComponents").unwrap_or("1") != "1" {
                return Err("VTK rectilinear coordinate array must have one component".into());
            }
            let values = data.floats(array, size)?;
            if values.len() != size {
                return Err("VTK rectilinear coordinate count disagrees with extent".into());
            }
            if values.len() > 1 {
                let increasing = values[1] > values[0];
                if values
                    .windows(2)
                    .any(|pair| pair[0] == pair[1] || (pair[1] > pair[0]) != increasing)
                {
                    return Err("VTK rectilinear coordinates must be strictly monotonic".into());
                }
            }
            axes.push(values);
        }
        let mut positions = Vec::new();
        for &z in &axes[2] {
            for &y in &axes[1] {
                for &x in &axes[0] {
                    positions.push(Vec3::new(x, y, z));
                }
            }
        }
        positions
    };
    let mut scene = Scene::default();
    append_structured(&positions, dimensions, &mut scene)?;
    scene.validate()?;
    Ok(scene)
}

/// Extract outer quadrilateral cell faces while preserving every boundary grid point.
///
/// # Errors
/// Returns an error for invalid lattice dimensions, a mismatched or oversized position array,
/// nonfinite points or boundary extraction limits.
pub(super) fn structured_scene(positions: &[Vec3], dimensions: [usize; 3]) -> Result<Scene> {
    let count = dimensions.iter().try_fold(1usize, |n, &d| n.checked_mul(d));
    if dimensions.contains(&0) || count != Some(positions.len()) {
        return Err("structured grid point count disagrees with dimensions".into());
    }
    if positions.iter().any(|p| !p.is_finite()) {
        return Err("structured grid contains non-finite coordinates".into());
    }
    let mut scene = Scene::default();
    append_structured(positions, dimensions, &mut scene)?;
    scene.validate()?;
    Ok(scene)
}

/// Append boundary quads or planar grid faces without interior structured-cell surfaces.
fn append_structured(positions: &[Vec3], dimensions: [usize; 3], scene: &mut Scene) -> Result<()> {
    let active = dimensions.iter().filter(|&&n| n > 1).count();
    if active == 0 {
        append_point(positions[0], scene)?;
        return Ok(());
    }
    if active == 1 {
        return Err("VTK one-dimensional structured line geometry is not implemented".into());
    }
    let index = |coordinates: [usize; 3]| {
        coordinates[0] + dimensions[0] * (coordinates[1] + dimensions[1] * coordinates[2])
    };
    for axis in 0..3 {
        if active == 2 && dimensions[axis] != 1 {
            continue;
        }
        let other: Vec<_> = (0..3).filter(|&a| a != axis).collect();
        let ends = if active == 2 {
            vec![0]
        } else {
            vec![0, dimensions[axis] - 1]
        };
        for end in ends {
            for j in 0..dimensions[other[1]] - 1 {
                for i in 0..dimensions[other[0]] - 1 {
                    let mut corners = Vec::new();
                    for (i, j) in [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)] {
                        let mut coordinates = [0usize; 3];
                        coordinates[axis] = end;
                        coordinates[other[0]] = i;
                        coordinates[other[1]] = j;
                        corners.push(index(coordinates));
                    }
                    if (end == 0) != (axis == 1) {
                        corners.reverse();
                    }
                    append_polygon(positions, &corners, scene)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn parse_vtk_xml(text: &str) -> Result<Scene> {
    parse_vtk_xml_bytes(text.as_bytes())
}

/// Validate the XML envelope and decode supported dataset pieces into one scene.
fn parse_vtk_xml_bytes(bytes: &[u8]) -> Result<Scene> {
    let (text, raw) = arrays::xml_envelope(bytes)?;
    let document = roxmltree::Document::parse(&text)?;
    let root = document.root_element();
    if !root.has_tag_name("VTKFile") {
        return Err("missing VTKFile XML root".into());
    }
    let data = XmlData::new(root, raw)?;
    let dataset = root
        .attribute("type")
        .ok_or("missing VTK XML dataset type")?;
    if dataset == "RectilinearGrid" || dataset == "StructuredGrid" {
        return xml_structured(root, dataset, &data);
    }
    if dataset != "PolyData" && dataset != "UnstructuredGrid" {
        return Err(format!("VTK XML dataset {dataset} is not implemented").into());
    }
    let mut scene = Scene::default();
    for piece in child(root, dataset)?
        .children()
        .filter(|n| n.has_tag_name("Piece"))
    {
        let point_count = count_attribute(piece, "NumberOfPoints")?;
        let positions = xml_positions(piece, point_count, &data)?;
        for array in piece
            .descendants()
            .filter(|n| n.has_tag_name("DataArray") && n.attribute("Name") == Some("vtkGhostType"))
        {
            if data.indices(array, MAX_ITEMS)?.iter().any(|&n| n != 0) {
                return Err("VTK ghost cells and hidden points are not implemented".into());
            }
        }
        if dataset == "PolyData" {
            let mut sections = Vec::new();
            for (tag, attr) in [
                ("Polys", "NumberOfPolys"),
                ("Strips", "NumberOfStrips"),
                ("Verts", "NumberOfVerts"),
            ] {
                let count = piece
                    .attribute(attr)
                    .map(str::parse::<usize>)
                    .transpose()?
                    .unwrap_or(0);
                if count > MAX_ITEMS {
                    return Err("VTK XML count exceeds geometry limit".into());
                }
                let section = piece.children().find(|n| n.has_tag_name(tag));
                sections.push(match section {
                    Some(section) => xml_cells(section, count, &data)?,
                    None if count == 0 => Vec::new(),
                    None => return Err(format!("missing VTK XML {tag}").into()),
                });
            }
            let lines = piece
                .attribute("NumberOfLines")
                .map(str::parse::<usize>)
                .transpose()?
                .unwrap_or(0);
            if lines > MAX_ITEMS {
                return Err("VTK XML line count exceeds geometry limit".into());
            }
            if lines != 0 {
                for line in xml_cells(child(piece, "Lines")?, lines, &data)? {
                    append_line(&positions, &line, &mut scene)?;
                }
            }
            let mut sections = sections.into_iter();
            append_polydata(
                &positions,
                sections.next().unwrap(),
                sections.next().unwrap(),
                sections.next().unwrap(),
                &mut scene,
            )?;
        } else {
            let count = count_attribute(piece, "NumberOfCells")?;
            let cell_node = child(piece, "Cells")?;
            let cells = xml_cells(cell_node, count, &data)?;
            let types = data.indices(named_array(cell_node, "types")?, count)?;
            let polyhedra = xml_polyhedra(cell_node, &types, &data)?;
            append_grid_with_polyhedra(&positions, &cells, &types, &mut scene, Some(&polyhedra))?;
        }
    }
    scene.validate()?;
    Ok(scene)
}

/// Validate polyhedron face offsets and expand per-cell face vertex lists.
fn xml_polyhedra(
    node: Node<'_, '_>,
    types: &[usize],
    data: &XmlData<'_>,
) -> Result<BTreeMap<usize, Vec<Vec<usize>>>> {
    let mut polyhedra = BTreeMap::new();
    if !types.contains(&42) {
        return Ok(polyhedra);
    }
    let faces = data.indices(named_array(node, "faces")?, MAX_ITEMS)?;
    let offsets = data.integers(named_array(node, "faceoffsets")?, types.len())?;
    if offsets.len() != types.len() {
        return Err("VTK polyhedron faceoffset count disagrees with cells".into());
    }
    let mut start = 0usize;
    for (i, (&ty, end)) in types.iter().zip(offsets).enumerate() {
        if ty != 42 {
            if end != -1 {
                return Err("VTK non-polyhedron face offset must be -1".into());
            }
            continue;
        }
        let end = usize::try_from(end)?;
        let values = faces
            .get(start..end)
            .ok_or("invalid VTK polyhedron face offset")?;
        let count = *values.first().ok_or("empty VTK polyhedron face stream")?;
        let mut at = 1usize;
        let mut cell_faces = Vec::new();
        for _ in 0..count {
            let n = *values
                .get(at)
                .ok_or("truncated VTK polyhedron face stream")?;
            at += 1;
            let stop = at.checked_add(n).ok_or("VTK face size overflow")?;
            cell_faces.push(
                values
                    .get(at..stop)
                    .ok_or("truncated VTK polyhedron face indices")?
                    .to_vec(),
            );
            at = stop;
        }
        if at != values.len() {
            return Err("unused VTK polyhedron face indices".into());
        }
        polyhedra.insert(i, cell_faces);
        start = end;
    }
    if start != faces.len() {
        return Err("unused VTK polyhedron face stream".into());
    }
    Ok(polyhedra)
}

/// Validate a polyline and append its consecutive finite segments.
fn append_line(positions: &[Vec3], indices: &[usize], scene: &mut Scene) -> Result<()> {
    line_segment_count(indices.len(), scene.lines.len())?;
    let points = checked_positions(positions, indices)?;
    for pair in points.windows(2) {
        scene.lines.push(Line {
            vertices: [pair[0], pair[1]],
            color: None,
        });
    }
    Ok(())
}

/// Preflight a complete ordered line cell against the shared output segment budget.
///
/// # Errors
/// A cell has fewer than two vertices, its output count overflows, or its segments exceed the limit.
fn line_segment_count(vertices: usize, existing: usize) -> Result<usize> {
    let segments = vertices
        .checked_sub(1)
        .filter(|&count| count != 0)
        .ok_or("VTK polyline needs at least two vertices")?;
    if existing
        .checked_add(segments)
        .is_none_or(|count| count > MAX_ITEMS)
    {
        return Err("VTK output line count exceeds geometry limit".into());
    }
    Ok(segments)
}

/// Resolve one cell reference without allocating an endpoint buffer.
///
/// # Errors
/// Returns an error for an out-of-bounds reference or nonfinite coordinate.
fn checked_position(positions: &[Vec3], index: usize) -> Result<Vec3> {
    let position = positions
        .get(index)
        .copied()
        .ok_or("VTK cell index is out of bounds")?;
    if !position.x.is_finite() || !position.y.is_finite() || !position.z.is_finite() {
        return Err("VTK cell coordinate is nonfinite".into());
    }
    Ok(position)
}

/// Resolve cell indices into finite point coordinates.
fn checked_positions(positions: &[Vec3], indices: &[usize]) -> Result<Vec<Vec3>> {
    indices
        .iter()
        .map(|&index| checked_position(positions, index))
        .collect()
}

/// Triangulate a validated polygon and enforce the scene triangle budget.
fn append_polygon(positions: &[Vec3], indices: &[usize], scene: &mut Scene) -> Result<()> {
    let vertices = checked_positions(positions, indices)?;
    for vertices in super::mesh::triangulate_polygon(&vertices)? {
        if scene.triangles.len() >= MAX_ITEMS {
            return Err("VTK output triangle count exceeds geometry limit".into());
        }
        scene.triangles.push(Triangle {
            vertices,
            color: None,
        });
    }
    Ok(())
}

/// Append one finite point within the scene item budget.
fn append_point(position: Vec3, scene: &mut Scene) -> Result<()> {
    if scene.points.len() >= MAX_ITEMS {
        return Err("VTK output point count exceeds geometry limit".into());
    }
    scene.points.push(Point {
        position,
        color: None,
    });
    Ok(())
}

/// Expand an alternating-winding triangle strip into valid faces.
fn append_strip(positions: &[Vec3], indices: &[usize], scene: &mut Scene) -> Result<()> {
    if indices.len() < 3 {
        return Err("VTK triangle strip has fewer than three vertices".into());
    }
    let points = checked_positions(positions, indices)?;
    for i in 2..points.len() {
        let vertices = if i % 2 == 0 {
            [points[i - 2], points[i - 1], points[i]]
        } else {
            [points[i - 1], points[i - 2], points[i]]
        };
        if (vertices[1] - vertices[0])
            .cross(vertices[2] - vertices[0])
            .length()
            > 0.0
        {
            if scene.triangles.len() >= MAX_ITEMS {
                return Err("VTK output triangle count exceeds geometry limit".into());
            }
            scene.triangles.push(Triangle {
                vertices,
                color: None,
            });
        }
    }
    Ok(())
}

/// Combine polygon, strip, line and vertex cells from a VTK piece.
fn append_polydata(
    positions: &[Vec3],
    polygons: Vec<Vec<usize>>,
    strips: Vec<Vec<usize>>,
    vertices: Vec<Vec<usize>>,
    scene: &mut Scene,
) -> Result<()> {
    for polygon in polygons {
        append_polygon(positions, &polygon, scene)?;
    }
    for strip in strips {
        append_strip(positions, &strip, scene)?;
    }
    for cell in vertices {
        for position in checked_positions(positions, &cell)? {
            append_point(position, scene)?;
        }
    }
    Ok(())
}

/// Preserve point and ordered line cells, and discard internal volume faces before triangulation.
///
/// # Errors
/// Returns an error for mismatched cell/type arrays, unsupported cell types, invalid indices,
/// malformed topology or exposed-geometry limits.
pub(super) fn grid_scene(
    positions: &[Vec3],
    cells: &[Vec<usize>],
    types: &[usize],
) -> Result<Scene> {
    let mut scene = Scene::default();
    append_grid(positions, cells, types, &mut scene)?;
    scene.validate()?;
    Ok(scene)
}

/// Extract unstructured points, ordered lines and exposed faces, including closed polyhedra.
///
/// # Errors
/// Returns an error for mismatched cell/type/face arrays, unsupported cells, invalid vertex
/// references, open or malformed polyhedra, or exposed-geometry limits.
pub(super) fn grid_scene_with_polyhedra(
    positions: &[Vec3],
    cells: &[Vec<usize>],
    types: &[usize],
    polyhedra: &BTreeMap<usize, Vec<Vec<usize>>>,
) -> Result<Scene> {
    let mut scene = Scene::default();
    append_grid_with_polyhedra(positions, cells, types, &mut scene, Some(polyhedra))?;
    scene.validate()?;
    Ok(scene)
}

/// Append points, ordered lines and visible faces from standard unstructured-cell topology.
fn append_grid(
    positions: &[Vec3],
    cells: &[Vec<usize>],
    types: &[usize],
    scene: &mut Scene,
) -> Result<()> {
    append_grid_with_polyhedra(positions, cells, types, scene, None)
}

/// Full mixed visible-output counts checked before scene mutation or primitive reservation.
#[derive(Default)]
struct GridOutput {
    /// Maximum exposed triangle count, including preexisting scene triangles.
    triangles: usize,
    /// Maximum line count, including preexisting scene lines.
    lines: usize,
    /// Point count, including preexisting scene points.
    points: usize,
}

impl GridOutput {
    /// Initialize aggregate counts from an existing output scene.
    fn new(scene: &Scene) -> Self {
        Self {
            triangles: scene.triangles.len(),
            lines: scene.lines.len(),
            points: scene.points.len(),
        }
    }

    /// Charge bounded visible output without allocating its primitive vectors.
    ///
    /// # Errors
    /// Count arithmetic or any independent primitive ceiling is exceeded.
    fn add(&mut self, triangles: usize, lines: usize, points: usize) -> Result<()> {
        for (count, additional) in [
            (&mut self.triangles, triangles),
            (&mut self.lines, lines),
            (&mut self.points, points),
        ] {
            *count = count
                .checked_add(additional)
                .ok_or("VTK output count overflow")?;
            if *count > MAX_ITEMS {
                return Err("VTK mixed output exceeds geometry limit".into());
            }
        }
        Ok(())
    }

    /// Preflight complete primitive storage and reserve all growth before emission.
    ///
    /// # Errors
    /// Payload arithmetic, conservative storage accounting or allocation fails.
    fn reserve(&self, scene: &mut Scene, storage: &mut quadratic::Storage) -> Result<()> {
        if self
            .triangles
            .checked_add(self.lines)
            .and_then(|n| n.checked_add(self.points))
            .is_none_or(|n| n > MAX_ITEMS)
        {
            return Err("VTK mixed output exceeds five million total primitives".into());
        }
        let triangles = self
            .triangles
            .max(scene.triangles.capacity())
            .checked_mul(std::mem::size_of::<Triangle>())
            .ok_or("VTK triangle storage overflow")?;
        let lines = self
            .lines
            .max(scene.lines.capacity())
            .checked_mul(std::mem::size_of::<Line>())
            .ok_or("VTK line storage overflow")?;
        let points = self
            .points
            .max(scene.points.capacity())
            .checked_mul(std::mem::size_of::<Point>())
            .ok_or("VTK point storage overflow")?;
        let bytes = triangles
            .checked_add(lines)
            .and_then(|n| n.checked_add(points))
            .and_then(|n| n.checked_mul(2))
            .ok_or("VTK primitive storage overflow")?;
        storage.charge(bytes)?;
        scene
            .triangles
            .try_reserve_exact(self.triangles - scene.triangles.len())?;
        scene
            .lines
            .try_reserve_exact(self.lines - scene.lines.len())?;
        scene
            .points
            .try_reserve_exact(self.points - scene.points.len())?;
        Ok(())
    }
}

/// Precharge variable node-list copies and a conservative linear-face map entry before allocation.
///
/// # Errors
/// Node-list size arithmetic or the common importer storage ceiling is exceeded.
fn linear_face_storage(nodes: usize, storage: &mut quadratic::Storage) -> Result<()> {
    let payload = nodes
        .checked_mul(2 * std::mem::size_of::<usize>())
        .and_then(|n| n.checked_add(256))
        .ok_or("VTK face storage overflow")?;
    storage.charge(payload)
}

/// Count shared cell faces and append only exterior geometry, accounting for preexisting lines/points.
///
/// # Errors
/// Invalid mixed topology, references, nonfinite nodes or aggregate output/storage limits fail.
#[allow(
    clippy::cast_precision_loss,
    reason = "Cell vertex counts are bounded by supported topology; f64 averaging computes an approximate face center."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep cell-type topology and shared-face cancellation together with their orientation/count checks."
)]
pub(super) fn append_grid_with_polyhedra(
    positions: &[Vec3],
    cells: &[Vec<usize>],
    types: &[usize],
    scene: &mut Scene,
    polyhedra: Option<&BTreeMap<usize, Vec<Vec<usize>>>>,
) -> Result<()> {
    if cells.len() != types.len() {
        return Err("VTK cell and type counts disagree".into());
    }
    if positions.len() > MAX_ITEMS || cells.len() > MAX_ITEMS {
        return Err("VTK grid arrays exceed geometry limit".into());
    }
    let mut storage = quadratic::Storage::default();
    let mut output = GridOutput::new(scene);
    let mut curved_faces = quadratic::Faces::default();
    let mut volume_faces: BTreeMap<Vec<usize>, (usize, Vec<usize>)> = BTreeMap::new();
    let mut connectivity = 0_usize;
    for (cell_index, (indices, &cell_type)) in cells.iter().zip(types).enumerate() {
        connectivity = connectivity
            .checked_add(indices.len())
            .ok_or("VTK connectivity count overflow")?;
        if connectivity > MAX_ITEMS {
            return Err("VTK grid connectivity exceeds geometry limit".into());
        }
        storage.charge(
            indices
                .len()
                .checked_mul(std::mem::size_of::<Vec3>())
                .ok_or("VTK validation storage overflow")?,
        )?;
        // Ordered lines preflight their complete output before validating
        // references without allocating another endpoint buffer.
        if !matches!(cell_type, 3 | 4) {
            checked_positions(positions, indices)?;
        }
        let expected = match cell_type {
            0 => Some(0),
            1 => Some(1),
            3 => Some(2),
            5 => Some(3),
            8..=10 => Some(4),
            11 | 12 => Some(8),
            13 => Some(6),
            14 => Some(5),
            _ => quadratic::node_count(cell_type),
        };
        if expected.is_some_and(|n| indices.len() != n) {
            return Err(format!("invalid vertex count for VTK cell type {cell_type}").into());
        }
        if cell_type == 2 && indices.is_empty() {
            return Err("VTK poly-vertex cell is empty".into());
        }
        if matches!(cell_type, 5..=7 | 9) && indices.len() < 3 {
            return Err("VTK surface cell has fewer than three vertices".into());
        }
        if quadratic::node_count(cell_type).is_some()
            && indices
                .iter()
                .enumerate()
                .any(|(i, node)| indices[..i].contains(node))
        {
            return Err("VTK higher-order cell repeats a node ID".into());
        }
        if (10..=14).contains(&cell_type) {
            let mut unique = indices.clone();
            unique.sort_unstable();
            if unique.windows(2).any(|w| w[0] == w[1]) {
                return Err("VTK volume cell has duplicate vertices".into());
            }
        }
        match cell_type {
            0 => {}
            1 | 2 => output.add(0, 0, indices.len())?,
            3 | 4 => {
                let segments = line_segment_count(indices.len(), output.lines)?;
                output.add(0, segments, 0)?;
                for &index in indices {
                    let _ = checked_position(positions, index)?;
                }
            }
            5..=9 => output.add(indices.len() - 2, 0, 0)?,
            21 | 35 => output.add(0, quadratic::line_segments(), 0)?,
            22 | 23 | 28 | 30 | 34 => {
                let kind = quadratic::surface_kind(cell_type)
                    .ok_or("VTK curved surface basis is missing")?;
                output.add(kind.triangles(), 0, 0)?;
            }
            24..=27 | 29 | 31..=33 | 37 => {
                for &(kind, roles) in quadratic::volume_layout(cell_type) {
                    curved_faces.insert(
                        quadratic::Surface::local(kind, indices, roles)?,
                        &mut storage,
                    )?;
                }
            }
            42 => {
                let faces = polyhedra
                    .and_then(|p| p.get(&cell_index))
                    .ok_or("VTK polyhedron cell has no face topology")?;
                if indices.len() < 4 || faces.len() < 4 {
                    return Err("VTK polyhedron requires at least four vertices and faces".into());
                }
                let mut unique = indices.clone();
                unique.sort_unstable();
                unique.dedup();
                if unique.len() != indices.len() {
                    return Err("VTK polyhedron cell repeats a vertex".into());
                }
                let used_count = faces.iter().try_fold(0_usize, |count, face| {
                    count
                        .checked_add(face.len())
                        .ok_or("VTK polyhedron face storage overflow")
                })?;
                if used_count > MAX_ITEMS {
                    return Err("VTK polyhedron face lists exceed geometry limit".into());
                }
                storage.charge(
                    used_count
                        .checked_mul(4 * std::mem::size_of::<usize>())
                        .ok_or("VTK polyhedron topology storage overflow")?,
                )?;
                let mut used: Vec<_> = faces.iter().flatten().copied().collect();
                used.sort_unstable();
                used.dedup();
                if used != unique {
                    return Err("VTK polyhedron faces do not cover its cell vertices".into());
                }
                let center = indices
                    .iter()
                    .map(|&i| positions[i] * (1. / indices.len() as f64))
                    .fold(Vec3::new(0., 0., 0.), |a, b| a + b);
                let scale = indices
                    .iter()
                    .map(|&i| (positions[i] - center).length())
                    .fold(0., f64::max);
                let mut edges = BTreeMap::<(usize, usize), usize>::new();
                for face in faces {
                    if face.len() < 3 || face.iter().any(|i| !indices.contains(i)) {
                        return Err(
                            "VTK polyhedron face references a vertex outside its cell".into()
                        );
                    }
                    linear_face_storage(face.len(), &mut storage)?;
                    let mut face = face.clone();
                    let mut unique = face.clone();
                    unique.sort_unstable();
                    unique.dedup();
                    if unique.len() != face.len() {
                        return Err("VTK polyhedron face repeats a vertex".into());
                    }
                    let normal = (0..face.len())
                        .map(|i| {
                            (positions[face[i]] - center)
                                .cross(positions[face[(i + 1) % face.len()]] - center)
                        })
                        .fold(Vec3::new(0., 0., 0.), |a, b| a + b)
                        .normalized();
                    if normal.length() == 0. {
                        return Err("VTK polyhedron face is degenerate".into());
                    }
                    let outward = if normal.dot(positions[face[0]] - center) < 0. {
                        face.reverse();
                        normal * -1.
                    } else {
                        normal
                    };
                    if indices
                        .iter()
                        .any(|&i| outward.dot(positions[i] - positions[face[0]]) > scale * 1e-8)
                    {
                        return Err("nonconvex VTK polyhedron cells are not implemented".into());
                    }
                    for i in 0..face.len() {
                        let (a, b) = (face[i], face[(i + 1) % face.len()]);
                        *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                    }
                    let mut key = face.clone();
                    key.sort_unstable();
                    let entry = volume_faces.entry(key).or_insert((0, face));
                    entry.0 += 1;
                    if entry.0 > 2 {
                        return Err("VTK grid contains a non-manifold volume face".into());
                    }
                }
                if edges.values().any(|&n| n != 2) {
                    return Err("VTK polyhedron face topology is not closed".into());
                }
            }
            10..=14 => {
                let local_faces: Vec<Vec<usize>> = match cell_type {
                    10 => vec![vec![0, 2, 1], vec![0, 1, 3], vec![1, 2, 3], vec![2, 0, 3]],
                    11 => vec![
                        vec![0, 2, 3, 1],
                        vec![4, 5, 7, 6],
                        vec![0, 1, 5, 4],
                        vec![1, 3, 7, 5],
                        vec![3, 2, 6, 7],
                        vec![2, 0, 4, 6],
                    ],
                    12 => vec![
                        vec![0, 3, 2, 1],
                        vec![4, 5, 6, 7],
                        vec![0, 1, 5, 4],
                        vec![1, 2, 6, 5],
                        vec![2, 3, 7, 6],
                        vec![3, 0, 4, 7],
                    ],
                    13 => vec![
                        vec![0, 2, 1],
                        vec![3, 4, 5],
                        vec![0, 1, 4, 3],
                        vec![1, 2, 5, 4],
                        vec![2, 0, 3, 5],
                    ],
                    14 => vec![
                        vec![0, 3, 2, 1],
                        vec![0, 1, 4],
                        vec![1, 2, 4],
                        vec![2, 3, 4],
                        vec![3, 0, 4],
                    ],
                    _ => unreachable!(),
                };
                for face in local_faces {
                    linear_face_storage(face.len(), &mut storage)?;
                    let face: Vec<_> = face.into_iter().map(|i| indices[i]).collect();
                    let mut key = face.clone();
                    key.sort_unstable();
                    let entry = volume_faces.entry(key).or_insert((0, face));
                    entry.0 += 1;
                    if entry.0 > 2 {
                        return Err("VTK grid contains a non-manifold volume face".into());
                    }
                }
            }
            _ => return Err(format!("VTK cell type {cell_type} is not implemented").into()),
        }
    }
    for &(count, ref face) in volume_faces.values() {
        if count == 1 {
            output.add(face.len() - 2, 0, 0)?;
        }
    }
    for face in curved_faces.visible() {
        output.add(face.kind.triangles(), 0, 0)?;
    }
    output.reserve(scene, &mut storage)?;
    for (indices, &cell_type) in cells.iter().zip(types) {
        match cell_type {
            1 | 2 => {
                for &index in indices {
                    append_point(positions[index], scene)?;
                }
            }
            3 | 4 => append_line(positions, indices, scene)?,
            5 | 7 | 9 => append_polygon(positions, indices, scene)?,
            6 => append_strip(positions, indices, scene)?,
            8 => append_polygon(
                positions,
                &[indices[0], indices[1], indices[3], indices[2]],
                scene,
            )?,
            21 | 35 => quadratic::append_curve(cell_type, positions, indices, scene)?,
            22 | 23 | 28 | 30 | 34 => {
                let kind = quadratic::surface_kind(cell_type)
                    .ok_or("VTK curved surface basis is missing")?;
                quadratic::Surface::new(kind, indices)?.append(positions, scene)?;
            }
            _ => {}
        }
    }
    for (_, (count, face)) in volume_faces {
        if count == 1 {
            append_polygon(positions, &face, scene)?;
        }
    }
    for face in curved_faces.visible() {
        face.append(positions, scene)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "# vtk DataFile Version 3.0\ntest geometry\nASCII\n";

    #[test]
    fn fixed_nonlinear_cells_validate_all_nodes_before_emitting_any_geometry() {
        let mut positions: Vec<_> = (0_u32..27)
            .map(|i| Vec3::new(f64::from(i), f64::from(i % 3), f64::from(i % 5)))
            .collect();
        for (kind, count) in [
            (21, 3),
            (22, 6),
            (23, 8),
            (24, 10),
            (25, 20),
            (26, 15),
            (27, 13),
            (28, 9),
            (29, 27),
            (30, 6),
            (31, 12),
            (32, 18),
            (33, 24),
            (34, 7),
            (35, 4),
            (37, 19),
        ] {
            let good: Vec<_> = (0..count).collect();
            let mut duplicate = good.clone();
            duplicate[count - 1] = 0;
            let mut invalid_index = good.clone();
            invalid_index[count - 1] = 27;
            for invalid in [
                good[..count - 1].to_vec(),
                (0..=count).collect(),
                duplicate,
                invalid_index,
            ] {
                let mut scene = Scene::default();
                assert!(
                    append_grid(&positions, &[vec![0], invalid], &[1, kind], &mut scene).is_err()
                );
                assert!(scene.points.is_empty());
                assert!(scene.triangles.is_empty());
                assert!(scene.lines.is_empty());
            }
        }
        for (kind, count, interior) in [(29, 27, 26), (37, 19, 18)] {
            positions[interior].x = f64::NAN;
            let mut scene = Scene::default();
            assert!(
                append_grid(
                    &positions,
                    &[vec![0], (0..count).collect()],
                    &[1, kind],
                    &mut scene
                )
                .is_err()
            );
            assert!(scene.points.is_empty());
            positions[interior].x = f64::from(u32::try_from(interior).unwrap());
        }
        assert!(grid_scene(&positions, &[vec![0, 1, 2]], &[36]).is_err());
    }

    #[test]
    fn curved_tetrahedra_cancel_only_the_complete_shared_face_roles() {
        let positions = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(0., 2., 0.),
            Vec3::new(0., 0., 2.),
            Vec3::new(1., 0., 0.3),
            Vec3::new(1., 1., 0.4),
            Vec3::new(0., 1., 0.1),
            Vec3::new(0., 0., 1.),
            Vec3::new(1., 0., 1.),
            Vec3::new(0., 1., 1.),
            Vec3::new(0., 0., -2.),
            Vec3::new(0., 0., -1.),
            Vec3::new(0., 1., -1.),
            Vec3::new(1., 0., -1.),
        ];
        let first: Vec<_> = (0..10).collect();
        let second = vec![0, 2, 1, 10, 6, 5, 4, 11, 12, 13];
        assert_eq!(
            grid_scene(&positions, std::slice::from_ref(&first), &[24])
                .unwrap()
                .triangles
                .len(),
            144
        );
        assert_eq!(
            grid_scene(&positions, &[first.clone(), second.clone()], &[24, 24])
                .unwrap()
                .triangles
                .len(),
            216
        );
        let mut wrong_roles = second.clone();
        wrong_roles.swap(4, 5);
        assert_eq!(
            grid_scene(&positions, &[first.clone(), wrong_roles], &[24, 24])
                .unwrap()
                .triangles
                .len(),
            288
        );
        assert!(grid_scene(&positions, &[first.clone(), second, first], &[24, 24, 24]).is_err());
    }

    #[test]
    fn mixed_output_budget_preflights_aggregate_limits_without_large_allocations() {
        let mut output = GridOutput {
            triangles: MAX_ITEMS - 1,
            lines: 1,
            points: 0,
        };
        output.add(0, 0, 1).unwrap();
        let mut scene = Scene::default();
        let mut storage = quadratic::Storage::default();
        assert!(output.reserve(&mut scene, &mut storage).is_err());
        assert_eq!(scene.triangles.capacity(), 0);
        assert_eq!(scene.lines.capacity(), 0);
        assert_eq!(scene.points.capacity(), 0);
        assert!(
            GridOutput {
                triangles: usize::MAX,
                lines: 0,
                points: 0
            }
            .add(1, 0, 0)
            .is_err()
        );
    }

    #[test]
    fn legacy_polydata_renders_polygon_strip_and_vertices() {
        let text = format!(
            "{HEADER}DATASET POLYDATA\nPOINTS 4 float\n0 0 0 1 0 0 1 1 0 0 1 0\nPOLYGONS 1 5\n4 0 1 2 3\nTRIANGLE_STRIPS 1 5\n4 0 1 3 2\nVERTICES 1 2\n1 0\n"
        );
        let scene = parse_vtk(&text).unwrap();
        assert_eq!(scene.triangles.len(), 4);
        assert_eq!(scene.points.len(), 1);
    }

    #[test]
    fn shared_tetrahedron_face_is_removed() {
        let text = format!(
            "{HEADER}DATASET UNSTRUCTURED_GRID\nPOINTS 5 double\n0 0 0 1 0 0 0 1 0 0 0 1 0 0 -1\nCELLS 2 10\n4 0 1 2 3\n4 0 2 1 4\nCELL_TYPES 2\n10 10\n"
        );
        let scene = parse_vtk(&text).unwrap();
        assert_eq!(scene.triangles.len(), 6);
        assert!(
            scene
                .triangles
                .iter()
                .all(|t| t.vertices.iter().any(|p| p.z != 0.0))
        );
    }

    #[test]
    fn unstructured_lines_preserve_order_closed_paths_repeats_and_signed_zero() {
        let positions = [
            Vec3::new(0., -0.0, 1.),
            Vec3::new(2., 1., 0.),
            Vec3::new(-1., 3., 2.),
            Vec3::new(5., -2., 4.),
        ];
        let cells = [vec![3, 1], vec![1, 0, 0, 2, 1], vec![2, 3]];
        let scene = grid_scene(&positions, &cells, &[3, 4, 3]).unwrap();
        let expected = [[3, 1], [1, 0], [0, 0], [0, 2], [2, 1], [2, 3]]
            .map(|pair| pair.map(|index| positions[index]));
        assert_eq!(scene.lines.len(), 6);
        for (line, vertices) in scene.lines.iter().zip(expected) {
            assert_eq!(line.vertices, vertices);
            assert!(line.color.is_none());
        }
        assert_eq!(scene.lines[1].vertices[1].y.to_bits(), (-0.0_f64).to_bits());
        assert_eq!(scene.lines[2].vertices[0], scene.lines[2].vertices[1]);
        assert!(scene.triangles.is_empty());
        assert!(scene.points.is_empty());
        let two_point_polyline = grid_scene(&positions, &[vec![1, 2]], &[4]).unwrap();
        assert_eq!(two_point_polyline.lines.len(), 1);
        assert_eq!(
            two_point_polyline.lines[0].vertices,
            [positions[1], positions[2]]
        );
    }

    #[test]
    fn unstructured_mixed_primitives_retain_lines_and_cancel_only_volume_faces() {
        let positions = [
            Vec3::new(0., 0., 0.),
            Vec3::new(1., 0., 0.),
            Vec3::new(0., 1., 0.),
            Vec3::new(0., 0., 1.),
            Vec3::new(0., 0., -1.),
            Vec3::new(7., 4., 2.),
            Vec3::new(8., 3., 1.),
            Vec3::new(9., 5., 0.),
            Vec3::new(7., 8., 1.),
        ];
        let cells = [
            vec![5],
            vec![5, 6],
            vec![0, 1],
            vec![1, 2, 0],
            vec![5, 7, 8],
            vec![0, 1, 2, 3],
            vec![0, 2, 1, 4],
        ];
        let scene = grid_scene(&positions, &cells, &[1, 2, 3, 4, 5, 10, 10]).unwrap();
        assert_eq!(scene.points.len(), 3);
        assert_eq!(scene.lines.len(), 3);
        assert_eq!(scene.triangles.len(), 7);
        assert_eq!(scene.points[0].position, positions[5]);
        assert_eq!(scene.points[1].position, positions[5]);
        assert_eq!(scene.points[2].position, positions[6]);
        for (line, pair) in scene.lines.iter().zip([[0, 1], [1, 2], [2, 0]]) {
            assert_eq!(line.vertices, pair.map(|index| positions[index]));
        }
        assert_eq!(
            scene.triangles[0].vertices,
            [positions[5], positions[7], positions[8]]
        );
        assert!(
            scene.triangles[1..]
                .iter()
                .all(|triangle| { triangle.vertices.iter().any(|position| position.z != 0.) })
        );
    }

    #[test]
    fn unstructured_line_siblings_preserve_original_polyhedron_cell_indices() {
        let positions = [
            Vec3::new(0., 0., 0.),
            Vec3::new(1., 0., 0.),
            Vec3::new(0., 1., 0.),
            Vec3::new(0., 0., 1.),
            Vec3::new(4., 5., 6.),
        ];
        let cells = [vec![0, 1], vec![], vec![0, 1, 2, 3], vec![4]];
        let faces = BTreeMap::from([(
            2,
            vec![vec![0, 2, 1], vec![0, 1, 3], vec![1, 2, 3], vec![2, 0, 3]],
        )]);
        let scene = grid_scene_with_polyhedra(&positions, &cells, &[3, 0, 42, 1], &faces).unwrap();
        assert_eq!(scene.triangles.len(), 4);
        assert_eq!(scene.lines.len(), 1);
        assert_eq!(scene.points.len(), 1);
        assert_eq!(scene.lines[0].vertices, [positions[0], positions[1]]);
        assert_eq!(scene.points[0].position, positions[4]);
        assert!(grid_scene(&positions, &[vec![0], vec![0, 1]], &[0, 3]).is_err());
    }

    #[test]
    fn unstructured_lines_reject_bad_cardinalities_indices_and_nonfinite_geometry() {
        let positions = [
            Vec3::new(0., 0., 0.),
            Vec3::new(1., 0., 0.),
            Vec3::new(0., 1., 0.),
        ];
        for cell in [vec![], vec![0], vec![0, 1, 2]] {
            let error = grid_scene(&positions, &[cell], &[3])
                .unwrap_err()
                .to_string();
            assert!(error.contains("invalid vertex count"));
        }
        for cell in [vec![], vec![0]] {
            let error = grid_scene(&positions, &[cell], &[4])
                .unwrap_err()
                .to_string();
            assert!(error.contains("at least two"));
        }
        for (cell, kind) in [(vec![0, 3], 3), (vec![0, 1, usize::MAX], 4)] {
            assert!(grid_scene(&positions, &[cell], &[kind]).is_err());
        }
        let mut scene = grid_scene(&positions, &[vec![0, 1]], &[3]).unwrap();
        assert!(append_line(&positions, &[0, 1, 9], &mut scene).is_err());
        assert_eq!(scene.lines.len(), 1);
        let nonfinite = [positions[0], Vec3::new(f64::NAN, 0., 0.)];
        assert!(grid_scene(&nonfinite, &[vec![0, 1]], &[3]).is_err());
        assert!(grid_scene(&positions, &[vec![0, 1]], &[]).is_err());
        assert!(grid_scene(&positions, &[vec![0, 1]], &[21]).is_err());
    }

    #[test]
    fn ordered_line_output_budget_is_checked_before_endpoint_expansion() {
        assert_eq!(line_segment_count(2, MAX_ITEMS - 1).unwrap(), 1);
        assert_eq!(line_segment_count(3, MAX_ITEMS - 2).unwrap(), 2);
        assert_eq!(line_segment_count(MAX_ITEMS + 1, 0).unwrap(), MAX_ITEMS);
        assert!(line_segment_count(2, MAX_ITEMS).is_err());
        assert!(line_segment_count(3, MAX_ITEMS - 1).is_err());
        assert!(line_segment_count(MAX_ITEMS + 2, 0).is_err());
        assert!(line_segment_count(2, usize::MAX).is_err());
        assert!(line_segment_count(usize::MAX, 2).is_err());
    }

    #[test]
    fn xml_polydata_concave_polygon() {
        let scene = parse_vtk_xml(r#"<VTKFile type="PolyData"><PolyData><Piece NumberOfPoints="5" NumberOfPolys="1"><Points><DataArray NumberOfComponents="3">0 0 0 2 0 0 2 2 0 1 1 0 0 2 0</DataArray></Points><Polys><DataArray Name="connectivity">0 1 2 3 4</DataArray><DataArray Name="offsets">5</DataArray></Polys></Piece></PolyData></VTKFile>"#).unwrap();
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-9);
    }

    #[test]
    fn xml_grid_extracts_tetrahedron() {
        let scene = parse_vtk_xml(r#"<VTKFile type="UnstructuredGrid"><UnstructuredGrid><Piece NumberOfPoints="4" NumberOfCells="1"><Points><DataArray>0 0 0 1 0 0 0 1 0 0 0 1</DataArray></Points><Cells><DataArray Name="connectivity">0 1 2 3</DataArray><DataArray Name="offsets">4</DataArray><DataArray Name="types">10</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>"#).unwrap();
        assert_eq!(scene.triangles.len(), 4);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "The test verifies exact integral-coordinate output from deterministic authored topology."
    )]
    fn rectilinear_grid_extracts_outer_cells_without_internal_faces() {
        let scene = parse_vtk_xml(r#"<VTKFile type="RectilinearGrid"><RectilinearGrid WholeExtent="-1 1 4 5 0 1"><Piece Extent="-1 1 4 5 0 1"><Coordinates><DataArray>1 2 4</DataArray><DataArray>2 5</DataArray><DataArray>3 7</DataArray></Coordinates></Piece></RectilinearGrid></VTKFile>"#).unwrap();
        assert_eq!(scene.triangles.len(), 20);
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 66.).abs() < 1e-9);
        assert!(
            !scene
                .triangles
                .iter()
                .any(|t| t.vertices.iter().all(|p| p.x == 2.))
        );
    }

    #[test]
    fn structured_grid_preserves_deformation_and_two_dimensional_surfaces() {
        use std::fmt::Write as _;
        let mut points = String::new();
        for z in [0., 4.] {
            for y in [0., 3.] {
                for x in [0., 2.] {
                    write!(points, "{} {y} {z} ", x + z * 0.25).unwrap();
                }
            }
        }
        let text = format!(
            r#"<VTKFile type="StructuredGrid"><StructuredGrid WholeExtent="0 1 0 1 0 1"><Piece Extent="0 1 0 1 0 1"><Points><DataArray>{points}</DataArray></Points></Piece></StructuredGrid></VTKFile>"#
        );
        let scene = parse_vtk_xml(&text).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 2. * (6. + 8. + 153f64.sqrt())).abs() < 1e-9);
        assert_eq!(
            scene
                .positions()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max),
            3.
        );

        let sheet = parse_vtk_xml(r#"<VTKFile type="StructuredGrid"><StructuredGrid WholeExtent="0 2 0 2 0 0"><Piece Extent="0 2 0 2 0 0"><Points><DataArray>0 0 0 1 0 0 2 0 0 0 1 0 1 1 0.7 2 1 0 0 2 0 1 2 0 2 2 0</DataArray></Points></Piece></StructuredGrid></VTKFile>"#).unwrap();
        assert_eq!(sheet.triangles.len(), 8);
        assert!(sheet.positions().any(|p| p == Vec3::new(1., 1., 0.7)));
        assert!(crate::render::render(&sheet, &crate::render::RenderOptions::default()).is_ok());
    }

    #[test]
    fn structured_grid_rejects_bad_extents_payloads_and_blanking() {
        let valid = r#"<VTKFile type="RectilinearGrid"><RectilinearGrid WholeExtent="0 1 0 1 0 1"><Piece Extent="0 1 0 1 0 1"><Coordinates><DataArray>0 1</DataArray><DataArray>0 1</DataArray><DataArray>0 1</DataArray></Coordinates></Piece></RectilinearGrid></VTKFile>"#;
        for bad in [
            valid.replace("WholeExtent=\"0 1 0 1 0 1\"", "WholeExtent=\"0 9223372036854775807 0 1 0 1\""),
            valid.replace("WholeExtent=\"0 1 0 1 0 1\"", "WholeExtent=\"1 0 0 1 0 1\""),
            valid.replace("Extent=\"0 1 0 1 0 1\"><Coordinates>", "Extent=\"0 0 0 1 0 1\"><Coordinates>"),
            valid.replacen("<DataArray>0 1</DataArray>", "<DataArray>0</DataArray>", 1),
            valid.replacen("<DataArray>0 1</DataArray>", "<DataArray>0 0</DataArray>", 1),
            valid.replace("<Coordinates>", "<PointData><DataArray Name=\"vtkGhostType\">1</DataArray></PointData><Coordinates>"),
            valid.replacen("<DataArray>", "<DataArray format=\"binary\">", 1),
            valid.replace("</RectilinearGrid>", "<Piece Extent=\"0 1 0 1 0 1\"/></RectilinearGrid>"),
        ] { assert!(parse_vtk_xml(&bad).is_err(), "accepted {bad}"); }
        assert!(parse_vtk_xml(r#"<VTKFile type="StructuredGrid"><StructuredGrid WholeExtent="0 1 0 0 0 0"><Piece Extent="0 1 0 0 0 0"><Points><DataArray>0 0 0 1 0 0</DataArray></Points></Piece></StructuredGrid></VTKFile>"#).is_err());
    }

    #[test]
    fn multiblock_reads_nested_relative_datasets_and_rejects_cycles() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "catwalk-vtm-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let triangle = r#"<VTKFile type="PolyData"><PolyData><Piece NumberOfPoints="3" NumberOfPolys="1"><Points><DataArray>0 0 0 1 0 0 0 1 0</DataArray></Points><Polys><DataArray Name="connectivity">0 1 2</DataArray><DataArray Name="offsets">3</DataArray></Polys></Piece></PolyData></VTKFile>"#;
        std::fs::write(directory.join("part one.vtp"), triangle).unwrap();
        let root = directory.join("main.vtm");
        let nested = directory.join("nested.vtm");
        std::fs::write(&nested, r#"<VTKFile type="vtkMultiBlockDataSet"><vtkMultiBlockDataSet><DataSet index="0" file="part%20one.vtp"/><DataSet index="1"/></vtkMultiBlockDataSet></VTKFile>"#).unwrap();
        std::fs::write(&root, r#"<VTKFile type="vtkMultiBlockDataSet"><vtkMultiBlockDataSet><Block index="0"><DataSet index="0" file="part%20one.vtp"/><Block index="1"><DataSet index="0" file="nested.vtm"/></Block></Block></vtkMultiBlockDataSet></VTKFile>"#).unwrap();
        let scene = load(&root).unwrap();
        assert_eq!(scene.triangles.len(), 2);
        assert!(crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok());

        for file in [
            "main.vtm",
            "../outside.vtp",
            "https://example.com/model.vtp",
            "missing.vtp",
        ] {
            std::fs::write(&nested, format!(r#"<VTKFile type="vtkMultiBlockDataSet"><vtkMultiBlockDataSet><DataSet file="{file}"/></vtkMultiBlockDataSet></VTKFile>"#)).unwrap();
            assert!(load(&root).is_err(), "accepted {file}");
        }
        let deep = format!(
            "<VTKFile type=\"vtkMultiBlockDataSet\"><vtkMultiBlockDataSet>{}{}{}{}</vtkMultiBlockDataSet></VTKFile>",
            "<Block>".repeat(40),
            "<DataSet file=\"part%20one.vtp\"/>",
            "</Block>".repeat(40),
            ""
        );
        std::fs::write(&root, deep).unwrap();
        assert!(load(&root).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_and_unsupported_geometry_is_rejected() {
        for body in [
            "DATASET POLYDATA POINTS 3 float 0 0 0 1 0 0 0 1 0 POLYGONS 1 4 3 0 1 9",
            "DATASET POLYDATA POINTS 3 float 0 0 0 1 0 0 0 1 0 POLYGONS 1 5 3 0 1 2",
            "DATASET UNSTRUCTURED_GRID POINTS 4 float 0 0 0 1 0 0 0 1 0 0 0 1 CELLS 1 4 3 0 1 2 CELL_TYPES 1 10",
            "DATASET UNSTRUCTURED_GRID POINTS 4 float 0 0 0 1 0 0 0 1 0 0 0 1 CELLS 1 5 4 0 1 2 3 CELL_TYPES 1 24",
            "DATASET POLYDATA POINTS 1 float NaN 0 0 VERTICES 1 2 1 0",
        ] {
            assert!(
                parse_vtk(&format!("{HEADER}{body}")).is_err(),
                "accepted {body}"
            );
        }
        assert!(parse_vtk_xml(r#"<VTKFile type="PolyData"><PolyData><Piece NumberOfPoints="1"><Points><DataArray format="binary">AAAA</DataArray></Points></Piece></PolyData></VTKFile>"#).is_err());
    }

    fn numeric(values: &[f64], big: bool) -> Vec<u8> {
        values
            .iter()
            .flat_map(|&n| {
                if big {
                    n.to_be_bytes()
                } else {
                    n.to_le_bytes()
                }
            })
            .collect()
    }
    fn integer_words(values: &[u64], width: usize, big: bool) -> Vec<u8> {
        values
            .iter()
            .flat_map(|&n| {
                let bytes = if big {
                    n.to_be_bytes()
                } else {
                    n.to_le_bytes()
                };
                if big {
                    bytes[8 - width..].to_vec()
                } else {
                    bytes[..width].to_vec()
                }
            })
            .collect()
    }
    fn encoded_payload(
        data: &[u8],
        header: usize,
        big: bool,
        compressor: Option<&str>,
    ) -> (Vec<u8>, String) {
        use base64::Engine;
        use std::io::Write;
        let (prefix, payload) = if let Some(compressor) = compressor {
            let size = 17;
            let blocks: Vec<_> = data
                .chunks(size)
                .map(|block| {
                    if compressor == "vtkZLibDataCompressor" {
                        let mut writer = flate2::write::ZlibEncoder::new(
                            Vec::new(),
                            flate2::Compression::fast(),
                        );
                        writer.write_all(block).unwrap();
                        writer.finish().unwrap()
                    } else {
                        lz4_flex::block::compress(block)
                    }
                })
                .collect();
            let mut words = vec![blocks.len() as u64, size as u64, (data.len() % size) as u64];
            words.extend(blocks.iter().map(|b| b.len() as u64));
            (integer_words(&words, header, big), blocks.concat())
        } else {
            (
                integer_words(&[data.len() as u64], header, big),
                data.to_vec(),
            )
        };
        let bytes = [prefix.as_slice(), payload.as_slice()].concat();
        let encoded = if compressor.is_some() {
            format!(
                "{}{}",
                base64::engine::general_purpose::STANDARD.encode(&prefix),
                base64::engine::general_purpose::STANDARD.encode(&payload)
            )
        } else {
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        };
        (bytes, encoded)
    }
    fn binary_tetrahedron(
        mode: &str,
        header: usize,
        big: bool,
        compressor: Option<&str>,
    ) -> Vec<u8> {
        let arrays = [
            (
                "Points",
                "Float64",
                3,
                numeric(&[0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1.], big),
            ),
            (
                "connectivity",
                "Int64",
                1,
                integer_words(&[0, 1, 2, 3], 8, big),
            ),
            ("offsets", "Int64", 1, integer_words(&[4], 8, big)),
            ("types", "UInt8", 1, vec![10]),
        ];
        let mut tags = Vec::new();
        let mut appended = Vec::new();
        for (name, ty, components, bytes) in arrays {
            let (raw, encoded) = encoded_payload(&bytes, header, big, compressor);
            let placement = if mode == "binary" {
                format!("format=\"binary\">{encoded}")
            } else {
                let offset = appended.len();
                if mode == "raw" {
                    appended.extend(raw);
                } else {
                    appended.extend(encoded.as_bytes());
                }
                format!("format=\"appended\" offset=\"{offset}\">")
            };
            tags.push(format!("<DataArray Name=\"{name}\" type=\"{ty}\" NumberOfComponents=\"{components}\" {placement}</DataArray>"));
        }
        let compression = compressor
            .map(|c| format!(" compressor=\"{c}\""))
            .unwrap_or_default();
        let mut document = format!(
            "<VTKFile type=\"UnstructuredGrid\" byte_order=\"{}\" header_type=\"UInt{}\"{compression}><UnstructuredGrid><Piece NumberOfPoints=\"4\" NumberOfCells=\"1\"><Points>{}</Points><Cells>{}{}{}</Cells></Piece></UnstructuredGrid>",
            if big { "BigEndian" } else { "LittleEndian" }, header*8, tags[0], tags[1], tags[2], tags[3]
        ).into_bytes();
        if mode != "binary" {
            document.extend(
                format!(
                    "<AppendedData encoding=\"{}\">\n_",
                    if mode == "raw" { "raw" } else { "base64" }
                )
                .as_bytes(),
            );
            document.extend(appended);
            document.extend(b"\n</AppendedData>");
        }
        document.extend(b"</VTKFile>");
        document
    }
    #[test]
    fn xml_binary_encodings_match_ascii_topology_and_rendering() {
        let ascii = parse_vtk_xml(r#"<VTKFile type="UnstructuredGrid"><UnstructuredGrid><Piece NumberOfPoints="4" NumberOfCells="1"><Points><DataArray>0 0 0 1 0 0 0 1 0 0 0 1</DataArray></Points><Cells><DataArray Name="connectivity">0 1 2 3</DataArray><DataArray Name="offsets">4</DataArray><DataArray Name="types">10</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>"#).unwrap();
        let expected: Vec<_> = ascii.positions().collect();
        let image =
            crate::render::render(&ascii, &crate::render::RenderOptions::default()).unwrap();
        for mode in ["binary", "raw", "base64"] {
            for header in [4, 8] {
                for big in [false, true] {
                    for compressor in [
                        None,
                        Some("vtkZLibDataCompressor"),
                        Some("vtkLZ4DataCompressor"),
                    ] {
                        let scene =
                            parse_vtk_xml_bytes(&binary_tetrahedron(mode, header, big, compressor))
                                .unwrap();
                        assert_eq!(
                            scene.positions().collect::<Vec<_>>(),
                            expected,
                            "{mode} {header} {big} {compressor:?}"
                        );
                        assert_eq!(
                            crate::render::render(&scene, &crate::render::RenderOptions::default())
                                .unwrap()
                                .as_raw(),
                            image.as_raw()
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn binary_arrays_reject_truncation_lengths_compression_and_raw_envelopes() {
        let valid = binary_tetrahedron("raw", 4, false, Some("vtkZLibDataCompressor"));
        let underscore = valid.windows(2).position(|s| s == b"\n_").unwrap() + 1;
        for n in [1, 2, 3, 4, 5, 7, 8, 12, 16] {
            let mut bad = valid.clone();
            bad.drain(underscore + 1..underscore + 1 + n);
            assert!(
                parse_vtk_xml_bytes(&bad).is_err(),
                "accepted truncation of {n} bytes"
            );
        }
        let mut bad = valid.clone();
        bad[underscore + 1..underscore + 5].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_vtk_xml_bytes(&bad).is_err());
        let mut bad = valid.clone();
        bad[underscore + 5..underscore + 9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_vtk_xml_bytes(&bad).is_err());
        let mut bad = valid.clone();
        bad[underscore + 9..underscore + 13].copy_from_slice(&18u32.to_le_bytes());
        assert!(parse_vtk_xml_bytes(&bad).is_err());
        let inline = String::from_utf8(binary_tetrahedron("binary", 4, false, None)).unwrap();
        for bad in [
            inline.replace("NumberOfPoints=\"4\"", "NumberOfPoints=\"3\""),
            inline.replace("UInt8", "Float32"),
            inline.replace("offsets", "missing-offsets"),
            inline.replace("LittleEndian", "unknown"),
            inline.replace("UInt32", "UInt16"),
        ] {
            assert!(parse_vtk_xml(&bad).is_err());
        }
        for bad in [
            b"</AppendedData><AppendedData encoding=\"raw\">_".as_slice(),
            b"<VTKFile><AppendedData encoding=\"raw\">".as_slice(),
        ] {
            assert!(arrays::xml_envelope(bad).is_err());
        }
    }
    #[test]
    fn legacy_binary_and_v5_split_offsets_match_ascii() {
        let coordinates = [0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1.];
        for version in ["3.0", "5.1"] {
            for binary in [false, true] {
                let mut data = format!("# vtk DataFile Version {version}\nindependent geometry\n{}\nDATASET UNSTRUCTURED_GRID\nPOINTS 4 double\n", if binary {"BINARY"} else {"ASCII"}).into_bytes();
                if binary {
                    data.extend(numeric(&coordinates, true));
                } else {
                    data.extend(b"0 0 0 1 0 0 0 1 0 0 0 1");
                }
                if version == "5.1" {
                    data.extend(b"\nCELLS 2 4\nOFFSETS vtktypeint64\n");
                    if binary {
                        data.extend(integer_words(&[0, 4], 8, true));
                    } else {
                        data.extend(b"0 4");
                    }
                    data.extend(b"\nCONNECTIVITY vtktypeint64\n");
                    if binary {
                        data.extend(integer_words(&[0, 1, 2, 3], 8, true));
                    } else {
                        data.extend(b"0 1 2 3");
                    }
                } else {
                    data.extend(b"\nCELLS 1 5\n");
                    if binary {
                        data.extend(integer_words(&[4, 0, 1, 2, 3], 4, true));
                    } else {
                        data.extend(b"4 0 1 2 3");
                    }
                }
                data.extend(b"\nCELL_TYPES 1\n");
                if binary {
                    data.extend(10i32.to_be_bytes());
                } else {
                    data.extend(b"10");
                }
                let scene = legacy::parse(&data).unwrap();
                assert_eq!(scene.triangles.len(), 4);
                let volume: f64 = scene
                    .triangles
                    .iter()
                    .map(|t| t.vertices[0].dot(t.vertices[1].cross(t.vertices[2])) / 6.)
                    .sum();
                assert!((volume - 1. / 6.).abs() < 1e-12);
                if binary {
                    data.pop();
                    assert!(legacy::parse(&data).is_err());
                }
            }
        }
    }
    #[test]
    fn xml_polyhedron_topology_extracts_closed_boundary_and_rejects_bad_faces() {
        let text = r#"<VTKFile type="UnstructuredGrid"><UnstructuredGrid><Piece NumberOfPoints="4" NumberOfCells="1"><Points><DataArray>0 0 0 1 0 0 0 1 0 0 0 1</DataArray></Points><Cells><DataArray Name="connectivity">0 1 2 3</DataArray><DataArray Name="offsets">4</DataArray><DataArray Name="types">42</DataArray><DataArray Name="faces">4 3 0 2 1 3 0 1 3 3 1 2 3 3 2 0 3</DataArray><DataArray Name="faceoffsets">17</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>"#;
        assert_eq!(parse_vtk_xml(text).unwrap().triangles.len(), 4);
        assert!(parse_vtk_xml(&text.replace(">17<", ">16<")).is_err());
        assert!(parse_vtk_xml(&text.replace("3 2 0 3", "3 0 0 3")).is_err());
    }
}
