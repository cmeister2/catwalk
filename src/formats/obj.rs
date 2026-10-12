//! Indexed Wavefront polygon attributes, points and polylines.

use std::path::Path;

use crate::{
    Result,
    scene::{AlphaMode, Line, Point, Scene, Triangle, TriangleAppearance, Vec3},
};

use super::obj_material::{Material, Materials, logical_lines, words};

/// Maximum coordinate records or emitted geometry items.
const MAX_ITEMS: usize = 20_000_000;
/// Maximum corners accepted in a polygon or polyline directive.
const MAX_CORNERS: usize = 100_000;
/// Maximum vector capacity bytes plus conservatively estimated appearance-map storage.
const MAX_STORAGE_BYTES: usize = 256 * 1024 * 1024;
/// Maximum material-name length.
const MAX_NAME_BYTES: usize = 4096;

/// Independent authored indices at one OBJ polygon corner.
#[derive(Clone, Copy)]
struct Corner {
    /// Geometric vertex index.
    position: usize,
    /// Optional texture-coordinate index.
    uv: Option<usize>,
    /// Optional normal index.
    normal: Option<usize>,
}

/// Evolving OBJ geometry, authored attributes and local material state.
struct Parser<'a> {
    /// Model root used to resolve local resource files.
    directory: &'a Path,
    /// Geometric vertex records.
    positions: Vec<Vec3>,
    /// Optional byte/sRGB vertex color extension records.
    colors: Vec<Option<[u8; 3]>>,
    /// Authored bottom-left texture coordinates.
    uvs: Vec<[f64; 2]>,
    /// Authored finite normals; zero records are allowed only when unused or on discarded faces.
    normals: Vec<Vec3>,
    /// Shared material resources and resource budgets.
    materials: Materials,
    /// Active material name, resolved when an element is emitted.
    material: Option<String>,
    /// Resulting fixed scene geometry and appearances.
    scene: Scene,
    /// Whether an explicit point, line or face element was authored, even if degenerate.
    authored_geometry: bool,
    /// Aggregate retained geometry and attribute storage estimate.
    storage: usize,
}

/// Decode bounded OBJ polygons and their independent authored corner attributes.
///
/// # Errors
/// Returns errors for malformed geometry, authored indices, existing material resources,
/// unsupported diffuse map options, nonfinite attributes or exceeded resource budgets.
pub(super) fn parse(source: &str, directory: &Path) -> Result<Scene> {
    if source.len() as u64 > super::MAX_INPUT_BYTES {
        return Err("OBJ input exceeds the 512 MiB limit".into());
    }
    let mut parser = Parser {
        directory,
        positions: Vec::new(),
        colors: Vec::new(),
        uvs: Vec::new(),
        normals: Vec::new(),
        materials: Materials::new(source.len(), directory)?,
        material: None,
        scene: Scene::default(),
        authored_geometry: false,
        storage: 0,
    };
    logical_lines(source, |number, line| {
        parser
            .line(line)
            .map_err(|error| format!("OBJ line {number}: {error}").into())
    })?;
    if !parser.authored_geometry {
        grow(
            &mut parser.scene.points,
            parser.positions.len(),
            &mut parser.storage,
        )?;
        parser.scene.points.extend(
            parser
                .positions
                .into_iter()
                .zip(parser.colors)
                .map(|(position, color)| Point { position, color }),
        );
    }
    Ok(parser.scene)
}

impl Parser<'_> {
    /// Charge retained allocations before growing a vector or appearance map.
    fn reserve(&mut self, count: usize, size: usize) -> Result<()> {
        self.storage = count
            .checked_mul(size)
            .and_then(|bytes| self.storage.checked_add(bytes))
            .ok_or("OBJ geometry storage overflow")?;
        if self.storage > MAX_STORAGE_BYTES {
            return Err("OBJ geometry and attributes exceed the 256 MiB storage limit".into());
        }
        Ok(())
    }

    /// Decode one bounded OBJ directive while preserving indexed face attribute seams.
    fn line(&mut self, line: &str) -> Result<()> {
        let words = words(line, MAX_CORNERS + 1)?;
        let Some(directive) = words.first().copied() else {
            return Ok(());
        };
        let values = &words[1..];
        match directive {
            "v" => self.position(values)?,
            "vt" => {
                if values.is_empty() || values.len() > 3 {
                    return Err("OBJ texture coordinate requires one to three components".into());
                }
                let u = finite(values[0])?;
                let v = values.get(1).map_or(Ok(0.0), |value| finite(value))?;
                if let Some(w) = values.get(2) {
                    finite(w)?;
                }
                self.record_limit()?;
                grow(&mut self.uvs, 1, &mut self.storage)?;
                self.uvs.push([u, v]);
            }
            "vn" => {
                if values.len() != 3 {
                    return Err("OBJ normal requires exactly three components".into());
                }
                let normal = vector(values)?;
                let length = normal.length();
                if !length.is_finite() {
                    return Err("OBJ normal exceeds numeric range".into());
                }
                self.record_limit()?;
                grow(&mut self.normals, 1, &mut self.storage)?;
                self.normals.push(normal);
            }
            "f" => {
                self.authored_geometry = true;
                self.face(values)?;
            }
            "l" => {
                self.authored_geometry = true;
                self.polyline(values)?;
            }
            "p" => {
                self.authored_geometry = true;
                self.points(values)?;
            }
            "curv" | "curv2" | "surf" => {
                return Err("OBJ free-form curves and surfaces are not implemented".into());
            }
            "usemtl" => {
                if values.len() != 1 || values[0].len() > MAX_NAME_BYTES {
                    return Err("OBJ usemtl requires one bounded material name".into());
                }
                self.material = Some(values[0].to_owned());
            }
            "mtllib" => {
                if values.is_empty() {
                    return Err("OBJ mtllib requires a material filename".into());
                }
                for name in values {
                    self.materials.library(self.directory, name)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Bound the combined coordinate record count before appending one attribute.
    fn record_limit(&self) -> Result<()> {
        if self
            .positions
            .len()
            .saturating_add(self.uvs.len())
            .saturating_add(self.normals.len())
            >= MAX_ITEMS
        {
            return Err("too many OBJ coordinate or attribute records".into());
        }
        Ok(())
    }

    /// Decode geometric or homogeneous vertices and the common RGB extension.
    fn position(&mut self, words: &[&str]) -> Result<()> {
        if !matches!(words.len(), 3 | 4 | 6 | 7) {
            return Err("OBJ vertex requires xyz, optional weight or RGB".into());
        }
        let mut position = vector(words)?;
        if matches!(words.len(), 4 | 7) {
            let weight = finite(words[3])?;
            if weight == 0.0 {
                return Err("OBJ homogeneous vertex has zero weight".into());
            }
            position = position / weight;
            if !position.is_finite() {
                return Err("OBJ homogeneous vertex exceeds numeric range".into());
            }
        }
        let color = if words.len() == 6 {
            Some(rgb(&words[3..6])?)
        } else if words.len() == 7 {
            Some(rgb(&words[4..7])?)
        } else {
            None
        };
        self.record_limit()?;
        grow(&mut self.positions, 1, &mut self.storage)?;
        grow(&mut self.colors, 1, &mut self.storage)?;
        self.positions.push(position);
        self.colors.push(color);
        Ok(())
    }

    /// Resolve v, v/vt, v//vn or v/vt/vn with independent positive/negative indexing.
    fn corner(&self, word: &str) -> Result<Corner> {
        let mut fields = word.split('/');
        let position = index(fields.next().unwrap_or(""), self.positions.len())?;
        let uv = fields.next();
        let normal = fields.next();
        if fields.next().is_some()
            || uv.is_some_and(str::is_empty) && normal.is_none()
            || normal.is_some_and(str::is_empty)
        {
            return Err("invalid OBJ face corner fields".into());
        }
        Ok(Corner {
            position,
            uv: uv
                .filter(|field| !field.is_empty())
                .map(|field| index(field, self.uvs.len()))
                .transpose()?,
            normal: normal
                .map(|field| index(field, self.normals.len()))
                .transpose()?,
        })
    }

    /// Resolve the currently named material without copying its image pixels.
    fn selected_material(&self) -> Option<&Material> {
        self.material
            .as_ref()
            .and_then(|name| self.materials.definitions.get(name))
    }

    /// Triangulate positions by corner indices and append their corresponding UVs and normals.
    fn face(&mut self, words: &[&str]) -> Result<()> {
        if words.len() < 3 || words.len() > MAX_CORNERS {
            return Err("OBJ face corner count is invalid".into());
        }
        let corners = words
            .iter()
            .map(|word| self.corner(word))
            .collect::<Result<Vec<_>>>()?;
        if corners.iter().any(|corner| {
            corner.uv.is_some() != corners[0].uv.is_some()
                || corner.normal.is_some() != corners[0].normal.is_some()
        }) {
            return Err("OBJ polygon corners must consistently reference UVs and normals".into());
        }
        let positions: Vec<_> = corners
            .iter()
            .map(|corner| self.positions[corner.position])
            .collect();
        let triangles = super::mesh::triangulate_polygon_indices(&positions)?;
        if self.scene.triangles.len().saturating_add(triangles.len()) > MAX_ITEMS {
            return Err("too much OBJ triangle geometry".into());
        }
        let material = self.selected_material().cloned();
        let color = material
            .as_ref()
            .and_then(|material| material.color)
            .or_else(|| mean_color(corners.iter().map(|corner| self.colors[corner.position])));
        let has_colors = corners
            .iter()
            .any(|corner| self.colors[corner.position].is_some());
        let has_appearance = material.is_some()
            || has_colors
            || corners[0].uv.is_some()
            || corners[0].normal.is_some();
        grow(
            &mut self.scene.triangles,
            triangles.len(),
            &mut self.storage,
        )?;
        if has_appearance && !triangles.is_empty() {
            // A BTreeMap node has spare value slots even for the first insertion.
            // Charge twelve initial slots and three slots per triangle, including
            // pointer/key overhead, rather than counting only occupied values.
            let slots = triangles
                .len()
                .checked_mul(3)
                .and_then(|slots| {
                    slots.checked_add(if self.scene.appearances.is_empty() {
                        12
                    } else {
                        0
                    })
                })
                .ok_or("OBJ appearance storage overflow")?;
            self.reserve(
                slots,
                std::mem::size_of::<TriangleAppearance>() + 4 * std::mem::size_of::<usize>(),
            )?;
        }
        for indices in triangles {
            let selected = indices.map(|index| corners[index]);
            let vertices = selected.map(|corner| self.positions[corner.position]);
            if !triangle_has_area(vertices) {
                continue;
            }
            if has_appearance {
                let appearance = self.appearance(selected, material.as_ref(), has_colors)?;
                self.scene
                    .appearances
                    .insert(self.scene.triangles.len(), appearance);
            }
            self.scene.triangles.push(Triangle {
                vertices,
                color: color.or_else(|| material.as_ref().map(|_| [255; 3])),
            });
        }
        Ok(())
    }

    /// Construct one triangle's diffuse multipliers without losing authored corner attributes.
    fn appearance(
        &self,
        corners: [Corner; 3],
        material: Option<&Material>,
        has_colors: bool,
    ) -> Result<TriangleAppearance> {
        let mut appearance = TriangleAppearance::default();
        if let Some(material) = material {
            let [r, g, b] = material.diffuse;
            appearance.base_color = [r, g, b, material.opacity];
            if material.opacity < 1.0 {
                appearance.alpha_mode = AlphaMode::Blend;
            }
        }
        if has_colors {
            appearance.colors = Some(corners.map(|corner| {
                let [r, g, b] = self.colors[corner.position]
                    .unwrap_or([255; 3])
                    .map(|value| srgb_to_linear(f64::from(value) / 255.0));
                [r, g, b, 1.0]
            }));
        }
        if corners[0].uv.is_some() {
            let mut coordinates = [[0.0; 2]; 3];
            for (out, corner) in coordinates.iter_mut().zip(corners) {
                let uv = self.uvs[corner
                    .uv
                    .ok_or("OBJ triangle is missing its texture coordinate")?];
                *out = if let Some(map) = material.and_then(|material| material.map.as_ref()) {
                    map.coordinates(uv)?
                } else {
                    [uv[0], 1.0 - uv[1]]
                };
                if out.iter().any(|value| !value.is_finite()) {
                    return Err("OBJ texture coordinate exceeds numeric range".into());
                }
            }
            appearance.texcoords = Some(coordinates);
            if let Some(map) = material.and_then(|material| material.map.as_ref()) {
                // OBJ diffuse maps are color maps; dissolve is controlled by d/Tr.
                appearance.texture = Some(map.texture.clone());
                if map.has_alpha {
                    appearance.alpha_mode = AlphaMode::Blend;
                }
            }
        }
        if corners[0].normal.is_some() {
            let mut normals = [Vec3::default(); 3];
            for (out, corner) in normals.iter_mut().zip(corners) {
                *out = self.normals[corner.normal.ok_or("OBJ triangle is missing its normal")?];
                if out.length() == 0.0 {
                    return Err("OBJ nondegenerate triangle references a zero normal".into());
                }
            }
            appearance.normals = Some(normals);
        }
        Ok(appearance)
    }

    /// Decode indexed polylines while validating their optional texture-coordinate references.
    fn polyline(&mut self, words: &[&str]) -> Result<()> {
        if words.len() < 2 || words.len() > MAX_CORNERS {
            return Err("OBJ polyline requires two to 100000 vertices".into());
        }
        let corners = words
            .iter()
            .map(|word| self.corner(word))
            .collect::<Result<Vec<_>>>()?;
        if corners.iter().any(|corner| corner.normal.is_some()) {
            return Err("OBJ polylines cannot reference normal records".into());
        }
        if self.scene.lines.len().saturating_add(corners.len() - 1) > MAX_ITEMS {
            return Err("too much OBJ line geometry".into());
        }
        grow(&mut self.scene.lines, corners.len() - 1, &mut self.storage)?;
        let color = self.selected_material().and_then(|material| material.color);
        for edge in corners.windows(2) {
            self.scene.lines.push(Line {
                vertices: [
                    self.positions[edge[0].position],
                    self.positions[edge[1].position],
                ],
                color: color
                    .or_else(|| mean_color(edge.iter().map(|corner| self.colors[corner.position]))),
            });
        }
        Ok(())
    }

    /// Decode explicit points, rejecting polygon-only slash attributes.
    fn points(&mut self, words: &[&str]) -> Result<()> {
        if words.is_empty() {
            return Err("OBJ point element requires a vertex".into());
        }
        if self.scene.points.len().saturating_add(words.len()) > MAX_ITEMS {
            return Err("too much OBJ point geometry".into());
        }
        grow(&mut self.scene.points, words.len(), &mut self.storage)?;
        let material_color = self
            .selected_material()
            .map(|material| material.color.unwrap_or([255; 3]));
        for word in words {
            if word.contains('/') {
                return Err("OBJ point elements require plain vertex references".into());
            }
            let index = index(word, self.positions.len())?;
            self.scene.points.push(Point {
                position: self.positions[index],
                color: material_color.or(self.colors[index]),
            });
        }
        Ok(())
    }
}

/// Determine noncollinearity without squaring tiny coordinates or multiplying huge edges.
/// Finite edges are normalized independently; overflowing subtraction scales positions first.
fn triangle_has_area(vertices: [Vec3; 3]) -> bool {
    let mut edges = [vertices[1] - vertices[0], vertices[2] - vertices[0]];
    if edges.iter().any(|edge| !edge.is_finite()) {
        let scale = vertices.iter().fold(0.0_f64, |scale, vertex| {
            scale
                .max(vertex.x.abs())
                .max(vertex.y.abs())
                .max(vertex.z.abs())
        });
        let vertices = vertices.map(|vertex| vertex / scale);
        edges = [vertices[1] - vertices[0], vertices[2] - vertices[0]];
    }
    let directions = edges.map(|edge| {
        let scale = edge.x.abs().max(edge.y.abs()).max(edge.z.abs());
        if scale == 0.0 {
            Vec3::default()
        } else {
            (edge / scale).normalized()
        }
    });
    directions[0].cross(directions[1]).length() != 0.0
}

/// Grow a vector geometrically after charging the complete requested capacity before allocation.
/// Existing spare capacity is reused, so exact reservation occurs only at geometric boundaries.
fn grow<T>(values: &mut Vec<T>, additional: usize, storage: &mut usize) -> Result<()> {
    let required = values
        .len()
        .checked_add(additional)
        .ok_or("OBJ vector length overflow")?;
    if required <= values.capacity() {
        return Ok(());
    }
    let capacity = required.max(values.capacity().saturating_mul(2)).max(4);
    let bytes = capacity
        .checked_sub(values.capacity())
        .and_then(|count| count.checked_mul(std::mem::size_of::<T>()))
        .ok_or("OBJ vector capacity overflow")?;
    let projected = storage
        .checked_add(bytes)
        .ok_or("OBJ vector storage overflow")?;
    if projected > MAX_STORAGE_BYTES {
        return Err("OBJ geometry and attributes exceed the 256 MiB storage limit".into());
    }
    let previous = values.capacity();
    values.try_reserve_exact(capacity - values.len())?;
    // The allocator may grant more capacity than requested; retain its actual
    // reported capacity in the budget rather than trusting a growth heuristic.
    *storage = storage
        .checked_add(
            (values.capacity() - previous)
                .checked_mul(std::mem::size_of::<T>())
                .ok_or("OBJ vector capacity overflow")?,
        )
        .ok_or("OBJ vector storage overflow")?;
    if *storage > MAX_STORAGE_BYTES {
        return Err("OBJ allocator capacity exceeds the storage budget".into());
    }
    Ok(())
}

/// Parse one finite coordinate or color component.
fn finite(word: &str) -> Result<f64> {
    let value: f64 = word.parse()?;
    if !value.is_finite() {
        return Err("non-finite OBJ coordinate or attribute".into());
    }
    Ok(value)
}

/// Decode the first three components of an already bounded vertex record.
fn vector(words: &[&str]) -> Result<Vec3> {
    if words.len() < 3 {
        return Err("OBJ coordinate requires three components".into());
    }
    Ok(Vec3::new(
        finite(words[0])?,
        finite(words[1])?,
        finite(words[2])?,
    ))
}

/// Convert normalized finite vertex RGB to the established byte/sRGB representation.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite clamped color components are rounded within the byte range."
)]
fn rgb(words: &[&str]) -> Result<[u8; 3]> {
    Ok([finite(words[0])?, finite(words[1])?, finite(words[2])?]
        .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8))
}

/// Resolve an independently counted one-based or relative-negative authored index.
fn index(word: &str, count: usize) -> Result<usize> {
    let value: i64 = word.parse()?;
    let count = i64::try_from(count)?;
    let offset = match value.cmp(&0) {
        std::cmp::Ordering::Greater => value - 1,
        std::cmp::Ordering::Less => count
            .checked_add(value)
            .ok_or("OBJ index arithmetic overflow")?,
        std::cmp::Ordering::Equal => {
            return Err("OBJ indices are one-based; zero is invalid".into());
        }
    };
    if offset < 0 || offset >= count {
        return Err("OBJ index out of bounds".into());
    }
    Ok(usize::try_from(offset)?)
}

/// Average available byte vertex colors for legacy polyline and triangle metadata.
#[allow(
    clippy::cast_possible_truncation,
    reason = "The mean of byte channels stays within the byte range."
)]
fn mean_color(colors: impl Iterator<Item = Option<[u8; 3]>>) -> Option<[u8; 3]> {
    let mut sum = [0_u64; 3];
    let mut count = 0;
    for color in colors.flatten() {
        count += 1;
        for (sum, channel) in sum.iter_mut().zip(color) {
            *sum += u64::from(channel);
        }
    }
    (count != 0).then(|| sum.map(|channel| (channel / count) as u8))
}

/// Decode encoded sRGB vertex color extensions into linear scene multipliers.
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "These controls compare exact authored integers and power-of-two fractions, not computed geometry approximations."
)]
/// Focused indexed-corner and malformed-attribute controls.
mod tests {
    use super::*;
    use std::fmt::Write;

    /// A concave polygon must retain the UV and normal belonging to each original corner.
    #[test]
    fn concave_faces_preserve_independent_corner_attributes() {
        let positions = [[0, 0], [2, 0], [2, 2], [1, 1], [0, 2]];
        let mut source = String::new();
        for [x, y] in positions {
            writeln!(source, "v {x} {y} 0").unwrap();
        }
        for index in 0..5 {
            writeln!(source, "vt {index} {}", index + 1).unwrap();
        }
        for index in 0..5 {
            writeln!(source, "vn {index} 1 1").unwrap();
        }
        // UV and normal indices deliberately differ from position and from each other.
        source.push_str("f -5/-1/-2 -4/-2/-3 -3/-3/-4 -2/-4/-5 -1/-5/-1\n");
        let scene = parse(&source, Path::new(".")).unwrap();
        assert_eq!(scene.triangles.len(), 3);
        for (index, triangle) in scene.triangles.iter().enumerate() {
            let appearance = &scene.appearances[&index];
            for corner in 0..3 {
                let authored = positions
                    .iter()
                    .position(|[x, y]| {
                        triangle.vertices[corner] == Vec3::new(f64::from(*x), f64::from(*y), 0.0)
                    })
                    .unwrap();
                let uv = [4, 3, 2, 1, 0][authored];
                let normal = [3, 2, 1, 0, 4][authored];
                assert_eq!(
                    appearance.texcoords.unwrap()[corner],
                    [f64::from(uv), -f64::from(uv)]
                );
                assert_eq!(
                    appearance.normals.unwrap()[corner],
                    Vec3::new(f64::from(normal), 1.0, 1.0)
                );
            }
        }
        scene.validate().unwrap();
    }

    /// The same geometric position can have different UVs and normals on adjacent faces.
    #[test]
    fn shared_positions_do_not_collapse_uv_or_normal_seams() {
        let scene = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 1\nvn 0 0 1\nvn 0 1 1\nf 1/1/1 2/1/1 3/1/1\nf 1/2/2 2/2/2 3/2/2\n", Path::new(".")).unwrap();
        assert_eq!(scene.triangles[0].vertices, scene.triangles[1].vertices);
        assert_eq!(scene.appearances[&0].texcoords, Some([[0.0, 1.0]; 3]));
        assert_eq!(scene.appearances[&1].texcoords, Some([[1.0, 0.0]; 3]));
        assert_ne!(scene.appearances[&0].normals, scene.appearances[&1].normals);
    }

    /// Bad authored indices must fail even when an optional texture is absent.
    #[test]
    fn invalid_independent_indices_and_inconsistent_corners_fail() {
        let prefix = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvn 0 0 1\n";
        for face in [
            "f 1/0 2/1 3/1",
            "f 1/2 2/1 3/1",
            "f 1/-2 2/1 3/1",
            "f 1//0 2//1 3//1",
            "f 1//2 2//1 3//1",
            "f 1//-2 2//1 3//1",
            "f 1/1/1/1 2/1/1 3/1/1",
            "f 1/ 2/1 3/1",
            "f 1/1/ 2/1/1 3/1/1",
            "f 1 2/1 3/1",
            "f 1//1 2 3//1",
        ] {
            assert!(
                parse(&format!("{prefix}{face}\n"), Path::new(".")).is_err(),
                "{face}"
            );
        }
    }

    /// Only unused or degenerate-face zero normals can be discarded safely.
    #[test]
    fn zero_normals_fail_for_visible_faces_but_degenerate_export_records_are_skipped() {
        let prefix = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 0\nvn 0 0 1\n";
        assert!(parse(&format!("{prefix}f 1//1 2//1 3//1\n"), Path::new(".")).is_err());
        let scene = parse(
            &format!("{prefix}f 1//1 1//1 1//1\nf 1//2 2//2 3//2\n"),
            Path::new("."),
        )
        .unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(
            scene.appearances[&0].normals,
            Some([Vec3::new(0.0, 0.0, 1.0); 3])
        );
    }

    /// Discarded authored faces cannot become a fabricated point cloud; vertex-only files can.
    #[test]
    fn degenerate_face_only_inputs_do_not_fall_back_to_points() {
        let vertices = "v 0 0 0\nv 1 0 0\nv 2 0 0\n";
        for face in ["f 1 2 3\n", "f 1 1 1\n"] {
            let scene = parse(&format!("{vertices}{face}"), Path::new(".")).unwrap();
            assert_eq!(
                [scene.triangles.len(), scene.lines.len(), scene.points.len()],
                [0; 3]
            );
            assert!(scene.validate().is_err());
        }
        let points = parse(vertices, Path::new(".")).unwrap();
        assert_eq!(points.points.len(), 3);
        points.validate().unwrap();
    }

    /// Nondegenerate triangles retain identical thumbnails across tiny, huge and subtraction-overflow scales.
    #[test]
    fn finite_triangle_scale_does_not_change_degeneracy_or_rendering() {
        let options = crate::render::RenderOptions {
            width: 32,
            height: 32,
            transparent: true,
            supersampling: 1,
            ..crate::render::RenderOptions::default()
        };
        let mut reference = None;
        for scale in [1.0_f64, 1e-200, 1e200, 1e308] {
            let source = format!("v -{scale:e} 0 0\nv {scale:e} 0 0\nv 0 {scale:e} 0\nf 1 2 3\n");
            let scene = parse(&source, Path::new(".")).unwrap();
            assert_eq!(scene.triangles.len(), 1, "scale {scale:e}");
            scene.validate().unwrap();
            let image = crate::render::render(&scene, &options).unwrap();
            assert!(image.pixels().any(|pixel| pixel[3] > 0));
            if let Some(reference) = &reference {
                assert_eq!(&image, reference, "scale {scale:e}");
            } else {
                reference = Some(image);
            }
            let collinear = format!("v -{scale:e} 0 0\nv {scale:e} 0 0\nv 0 0 0\nf 1 2 3\n");
            let scene = parse(&collinear, Path::new(".")).unwrap();
            assert_eq!(
                [scene.triangles.len(), scene.points.len()],
                [0; 2],
                "collinear scale {scale:e}"
            );
            assert!(scene.validate().is_err());
        }
    }

    /// Attribute and homogeneous arithmetic cannot introduce nonfinite geometry.
    #[test]
    fn nonfinite_coordinates_and_normal_overflow_are_rejected() {
        for record in [
            "v inf 0 0",
            "v 1e308 0 0 1e-308",
            "vt NaN",
            "vt 0 inf",
            "vt 0 0 NaN",
            "vn 1.79e308 1.79e308 1.79e308",
            "vn 0 inf 1",
        ] {
            assert!(parse(record, Path::new(".")).is_err(), "{record}");
        }
        let scene = parse(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 1e308 0 0\nf 1//1 2//1 3//1\n",
            Path::new("."),
        )
        .unwrap();
        scene.validate().unwrap();
        assert_eq!(
            scene.appearances[&0].normals.unwrap()[0],
            Vec3::new(1e308, 0.0, 0.0)
        );
    }

    /// Geometry storage reservations reject overflow before appending records.
    #[test]
    fn geometry_budget_rejects_before_growth() {
        let mut parser = Parser {
            directory: Path::new("."),
            positions: Vec::new(),
            colors: Vec::new(),
            uvs: Vec::new(),
            normals: Vec::new(),
            materials: Materials::new(0, Path::new(".")).unwrap(),
            material: None,
            scene: Scene::default(),
            authored_geometry: false,
            storage: MAX_STORAGE_BYTES,
        };
        assert!(parser.position(&["0", "0", "0"]).is_err());
        assert_eq!(parser.positions, []);
        assert!(
            parser
                .reserve(usize::MAX, std::mem::size_of::<Triangle>())
                .is_err()
        );
    }

    /// Vertex RGB extensions retain their independent sRGB corner multipliers.
    #[test]
    fn vertex_color_variation_is_preserved_in_linear_corner_multipliers() {
        let scene = parse(
            "v 0 0 0 1 0 0\nv 1 0 0 0 .5 0\nv 0 1 0 0 0 1\nf 1 2 3\n",
            Path::new("."),
        )
        .unwrap();
        let colors = scene.appearances[&0].colors.unwrap();
        assert_eq!(colors[0], [1.0, 0.0, 0.0, 1.0]);
        assert!((colors[1][1] - 0.215_860_500_113_899_26).abs() < 1e-15);
        assert_eq!(colors[2], [0.0, 0.0, 1.0, 1.0]);
    }
}
