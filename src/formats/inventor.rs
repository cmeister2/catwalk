//! Native Inventor solved part geometry and locally resolved assembly occurrences.
//!
//! `cadmpeg` owns CFB/RSe/ASM decoding. Parts use their typed active kernel
//! carrier; assemblies use exact UFRx/AmDc/AmGraphics identity joins. Persisted
//! absolute paths are relocated only to siblings in the supplied project.

use crate::{Result, scene::Scene};
use cadmpeg_codec_inventor::InventorCodec;
use cadmpeg_core::decode::DecodePolicy;
use cadmpeg_ir::{
    CadIr, Codec, DecodeOptions,
    products::{ExternalDocument, Occurrence, OccurrenceParent, PrototypeReference},
    report::{
        Severity,
        decode::DecodeReport,
        loss::{LossCategory, LossTaxonomy},
    },
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Maximum distinct documents read while resolving one assembly.
const MAX_FILES: usize = 128;
/// Maximum nested assembly depth, independent of native record recursion.
const MAX_DEPTH: usize = 32;
/// Maximum placed occurrences expanded across the whole project.
const MAX_OCCURRENCES: usize = 4096;
/// Maximum physical bytes across all unique native documents.
const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum geometry primitives across cached and expanded scenes.
const MAX_PRIMITIVES: usize = 5_000_000;
/// Conservative cumulative allocation ceiling for retained project geometry.
const MAX_SCENE_BYTES: usize = 512 * 1024 * 1024;
/// Maximum directory entries examined while resolving relocated references.
const MAX_DIRECTORY_ENTRIES: usize = 16_384;

/// Resolve genuine Inventor document geometry without consulting saved images.
///
/// # Errors
/// Native decoding, incomplete geometry or placements, missing local references,
/// unsupported document variants, reference cycles or resource limits fail.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let path = fs::canonicalize(path)?;
    let root = path
        .parent()
        .ok_or("Inventor input has no parent directory")?
        .to_owned();
    let mut project = Project {
        root,
        active: HashSet::new(),
        cached: HashMap::new(),
        input_bytes: 0,
        occurrences: 0,
        primitives: 0,
        scene_bytes: 0,
        directory_entries: 0,
    };
    let scene = project.document(&path, 0)?;
    drop(project);
    // The final scene is shared only with the now-dropped project cache.
    Ok(Arc::try_unwrap(scene).unwrap_or_else(|scene| (*scene).clone()))
}

/// Bounded native documents, recursion stack and transformed scene allocations.
struct Project {
    /// Canonical directory containing the requested document and its companions.
    root: PathBuf,
    /// Documents currently being resolved, for cycle detection.
    active: HashSet<PathBuf>,
    /// Geometry shared by repeated uses of the same canonical document.
    cached: HashMap<PathBuf, Arc<Scene>>,
    /// Total bytes admitted before opening unique input documents.
    input_bytes: u64,
    /// Total occurrence expansions admitted before recursion.
    occurrences: usize,
    /// Total retained and expanded primitive count.
    primitives: usize,
    /// Conservative cumulative scene allocation accounting.
    scene_bytes: usize,
    /// Total filesystem directory entries examined for name matching.
    directory_entries: usize,
}

impl Project {
    /// Decode one unique document and completely resolve all its visible uses.
    ///
    /// # Errors
    /// Outside-root files, cycles, incomplete decode or limits are rejected.
    fn document(&mut self, path: &Path, depth: usize) -> Result<Arc<Scene>> {
        if depth > MAX_DEPTH {
            return Err("Inventor assembly nesting exceeds 32 levels".into());
        }
        if !path.starts_with(&self.root) {
            return Err("Inventor component escapes the supplied project directory".into());
        }
        if self.active.contains(path) {
            return Err("cyclic Inventor assembly reference".into());
        }
        if let Some(scene) = self.cached.get(path) {
            return Ok(Arc::clone(scene));
        }
        if self.cached.len() + self.active.len() >= MAX_FILES {
            return Err("Inventor assembly exceeds 128 distinct documents".into());
        }
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() {
            return Err("Inventor component is not a regular file".into());
        }
        self.input_bytes = self
            .input_bytes
            .checked_add(metadata.len())
            .ok_or("Inventor project input size overflow")?;
        if self.input_bytes > MAX_INPUT_BYTES {
            return Err("Inventor project inputs exceed 256 MiB".into());
        }
        let extension = super::extension(path)?;
        if !matches!(extension.as_str(), "ipt" | "iam") {
            return Err("Inventor component must be an .ipt or .iam document".into());
        }
        self.active.insert(path.to_owned());
        let decoded = InventorCodec.decode(
            &mut File::open(path)?,
            &DecodeOptions {
                container_only: false,
                policy: DecodePolicy::service(),
            },
        )?;
        let assembly = extension == "iam";
        validate_decode(decoded.report(), assembly)?;
        let ir = decoded.ir();
        if ir.source.as_ref().and_then(|source| {
            source.attributes.iter().find_map(|(key, value)| {
                (key.as_str() == "document_kind").then_some(value.as_str())
            })
        }) != Some(if assembly { "assembly" } else { "part" })
        {
            return Err("Inventor native document kind does not match its extension".into());
        }
        let scene = if assembly {
            self.assembly(ir, path, depth)?
        } else {
            if ir.model.bodies.is_empty() || ir.model.faces.is_empty() {
                return Err("Inventor part contains no transferred solved geometry".into());
            }
            let scene = super::native_cad::scene_from_ir(
                ir,
                super::native_cad::SurfaceNormalConvention::NativeAsm,
            )?;
            self.reserve_scene(&scene)?;
            scene
        };
        scene.validate()?;
        self.active.remove(path);
        let scene = Arc::new(scene);
        self.cached.insert(path.to_owned(), Arc::clone(&scene));
        Ok(scene)
    }

    /// Expand exact native occurrences, keeping suppression and every placement.
    ///
    /// # Errors
    /// Missing/unresolved prototypes, partial joins, unsupported parents or scales fail.
    fn assembly(&mut self, ir: &CadIr, path: &Path, depth: usize) -> Result<Scene> {
        validate_assembly_kernel(ir, path)?;
        let source_count = ir
            .native
            .namespace("inventor")
            .and_then(|native| native.arenas().get("ufrx_occurrences"))
            .map_or(0, Vec::len);
        if source_count != ir.model.occurrences.len() || source_count == 0 {
            return Err("Inventor assembly occurrence joins are missing or incomplete".into());
        }
        if !ir.model.bodies.is_empty() {
            return Err("Inventor assembly has unsupported embedded result geometry".into());
        }
        let mut scene = Scene::default();
        for occurrence in &ir.model.occurrences {
            if occurrence.visible == Some(false) {
                continue;
            }
            self.occurrences = self
                .occurrences
                .checked_add(1)
                .ok_or("Inventor occurrence count overflow")?;
            if self.occurrences > MAX_OCCURRENCES {
                return Err("Inventor project exceeds 4096 placed occurrences".into());
            }
            if !matches!(occurrence.parent, OccurrenceParent::Root {})
                || occurrence.linked_prototype.is_some()
            {
                return Err(
                    "Inventor occurrence has unsupported local parent or link state".into(),
                );
            }
            let PrototypeReference::External {
                document: ExternalDocument::Path { path: reference },
                object: None,
            } = &occurrence.prototype
            else {
                return Err("Inventor occurrence has no resolvable native file reference".into());
            };
            let child_path = self.resolve(path, reference.as_str())?;
            let child = self.document(&child_path, depth + 1)?;
            self.reserve_scene(&child)?;
            append_placed(&mut scene, &child, occurrence)?;
        }
        Ok(scene)
    }

    /// Charge geometry storage and primitive traversal before output allocation.
    ///
    /// # Errors
    /// Counter overflow or the aggregate scene limits fail before vector growth.
    fn reserve_scene(&mut self, scene: &Scene) -> Result<()> {
        let count = scene
            .triangles
            .len()
            .checked_add(scene.points.len())
            .and_then(|count| count.checked_add(scene.lines.len()))
            .ok_or("Inventor primitive count overflow")?;
        let primitives = self
            .primitives
            .checked_add(count)
            .ok_or("Inventor primitive count overflow")?;
        let bytes = scene
            .triangles
            .len()
            .checked_mul(std::mem::size_of::<crate::scene::Triangle>())
            .and_then(|bytes| {
                scene
                    .lines
                    .len()
                    .checked_mul(std::mem::size_of::<crate::scene::Line>())
                    .and_then(|lines| bytes.checked_add(lines))
            })
            .and_then(|bytes| {
                scene
                    .points
                    .len()
                    .checked_mul(std::mem::size_of::<crate::scene::Point>())
                    .and_then(|points| bytes.checked_add(points))
            })
            .and_then(|bytes| {
                scene
                    .appearances
                    .len()
                    .checked_mul(std::mem::size_of::<crate::scene::TriangleAppearance>() + 64)
                    .and_then(|appearances| bytes.checked_add(appearances))
            })
            .and_then(|bytes| self.scene_bytes.checked_add(bytes))
            .ok_or("Inventor scene allocation overflow")?;
        if primitives > MAX_PRIMITIVES || bytes > MAX_SCENE_BYTES {
            return Err(
                "Inventor project geometry exceeds its 5-million primitive or 512 MiB limit".into(),
            );
        }
        self.primitives = primitives;
        self.scene_bytes = bytes;
        Ok(())
    }

    /// Relocate an authored reference to the exact supplied local companion.
    ///
    /// # Errors
    /// Traversal, missing files, ambiguous case matches or escaped symlinks fail.
    fn resolve(&mut self, owner: &Path, reference: &str) -> Result<PathBuf> {
        if reference.is_empty() || reference.len() > 4096 || reference.contains('\0') {
            return Err("invalid Inventor component reference".into());
        }
        let normalized = reference.replace('\\', "/");
        let absolute = normalized.starts_with('/')
            || (normalized.as_bytes().get(1) == Some(&b':')
                && normalized.as_bytes()[0].is_ascii_alphabetic());
        let components = normalized
            .split('/')
            .filter(|component| !component.is_empty())
            .collect::<Vec<_>>();
        if components
            .iter()
            .any(|component| matches!(*component, "." | ".."))
        {
            return Err("Inventor component reference contains traversal".into());
        }
        let directory = owner
            .parent()
            .ok_or("Inventor assembly has no parent directory")?;
        let requested = if absolute {
            &components[components
                .len()
                .checked_sub(1)
                .ok_or("Inventor component reference has no filename")?..]
        } else {
            components.as_slice()
        };
        if requested.iter().any(|component| component.contains(':')) {
            return Err("invalid Inventor component reference path".into());
        }
        let mut target = directory.to_owned();
        for component in requested {
            let mut matched = None;
            for entry in fs::read_dir(&target)? {
                self.directory_entries += 1;
                if self.directory_entries > MAX_DIRECTORY_ENTRIES {
                    return Err("Inventor component directory search limit exceeded".into());
                }
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(component))
                    && matched.replace(entry.path()).is_some()
                {
                    return Err("ambiguous Inventor component filename".into());
                }
            }
            target = matched.ok_or_else(|| format!(
                "missing Inventor companion {component}; provide its .ipt/.iam beside the assembly"
            ))?;
            target = fs::canonicalize(&target)?;
            if !target.starts_with(&self.root) {
                return Err("Inventor component escapes the supplied project directory".into());
            }
        }
        Ok(target)
    }
}

/// Prove that local assembly result geometry is absent before composing parts.
///
/// The upstream codec transfers external occurrences but does not decode the
/// assembly's own `AmBRep` bodies. Its exact typed record ranges address the
/// already validated bulk carrier; the source fingerprint binds the second
/// read. A header-only ASM has one entity, the header itself.
///
/// # Errors
/// Nonempty, ambiguous, unqualified or changed assembly carriers are rejected.
fn validate_assembly_kernel(ir: &CadIr, path: &Path) -> Result<()> {
    let native = ir
        .native
        .namespace("inventor")
        .ok_or("Inventor assembly has no typed native inventory")?;
    let Some(metadata) = native.arenas().get("segment_meta") else {
        return Err("Inventor assembly has no segment inventory".into());
    };
    for segment in metadata {
        if segment
            .fields()
            .get("kind")
            .and_then(serde_json::Value::as_str)
            != Some("am_brep")
        {
            continue;
        }
        let token = segment
            .fields()
            .get("token")
            .and_then(serde_json::Value::as_str)
            .ok_or("Inventor assembly B-rep has no segment token")?;
        let records = native
            .arenas()
            .get("rse_records")
            .ok_or("Inventor assembly has no framed bulk records")?;
        let mut carriers = records.iter().filter(|record| {
            record
                .fields()
                .get("token")
                .and_then(serde_json::Value::as_str)
                == Some(token)
                && record
                    .fields()
                    .get("type_id")
                    .and_then(serde_json::Value::as_str)
                    == Some("5c5945f6d5113313100060a6bba647b5")
        });
        let carrier = carriers
            .next()
            .ok_or("Inventor assembly B-rep has no typed kernel carrier")?;
        if carriers.next().is_some() {
            return Err("Inventor assembly has ambiguous local kernel carriers".into());
        }
        let offset = carrier
            .fields()
            .get("payload_offset")
            .and_then(serde_json::Value::as_u64)
            .ok_or("Inventor assembly kernel has no payload offset")?;
        let length = carrier
            .fields()
            .get("payload_len")
            .and_then(serde_json::Value::as_u64)
            .ok_or("Inventor assembly kernel has no payload length")?;
        let bulk = native
            .arenas()
            .get("segment_bulk")
            .and_then(|records| {
                records.iter().find(|record| {
                    record
                        .fields()
                        .get("token")
                        .and_then(serde_json::Value::as_str)
                        == Some(token)
                })
            })
            .ok_or("Inventor assembly has no matching bulk stream")?;
        let expanded_len = bulk
            .fields()
            .get("expanded_len")
            .and_then(serde_json::Value::as_u64)
            .ok_or("Inventor assembly bulk stream has no expanded length")?;
        let expected_hash = bulk
            .fields()
            .get("expanded_sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or("Inventor assembly bulk stream has no source fingerprint")?;
        let expanded = read_assembly_bulk(path, token, expanded_len, expected_hash)?;
        let start = usize::try_from(
            offset
                .checked_add(14)
                .ok_or("Inventor assembly carrier offset overflow")?,
        )?;
        let end = usize::try_from(
            offset
                .checked_add(length)
                .ok_or("Inventor assembly carrier length overflow")?,
        )?;
        let kernel = expanded
            .get(start..end)
            .ok_or("Inventor assembly carrier lies outside its framed record")?;
        validate_empty_asm_kernel(kernel)?;
    }
    Ok(())
}

/// Read one bounded assembly stream and verify its decoded source fingerprint.
///
/// # Errors
/// Oversized, truncated, invalid compressed or changed streams are rejected.
fn read_assembly_bulk(
    path: &Path,
    token: &str,
    expanded_len: u64,
    expected_hash: &str,
) -> Result<Vec<u8>> {
    if expanded_len > 64 * 1024 * 1024 {
        return Err("Inventor assembly kernel carrier exceeds 64 MiB".into());
    }
    let mut compound = cfb::open(path)?;
    let mut stream = compound.open_stream(format!("/RSeStorage/B{token}"))?;
    let mut prefix = [0u8; 18];
    stream.read_exact(&mut prefix)?;
    let decoder = flate2::read::ZlibDecoder::new(stream);
    let mut expanded = Vec::new();
    decoder.take(expanded_len + 1).read_to_end(&mut expanded)?;
    if expanded.len() as u64 != expanded_len
        || cadmpeg_ir::hash::sha256_hex(&expanded) != expected_hash
    {
        return Err("Inventor assembly carrier changed after native decoding".into());
    }
    Ok(expanded)
}

/// Require an ASM header-only assembly carrier with no local result bodies.
///
/// # Errors
/// Unsupported, truncated or nonempty kernel carriers are rejected.
fn validate_empty_asm_kernel(kernel: &[u8]) -> Result<()> {
    let entity_count = if kernel.starts_with(b"ASM BinaryFile4") {
        Some(u64::from(u32::from_le_bytes(
            kernel
                .get(23..27)
                .ok_or("truncated Inventor assembly ASM header")?
                .try_into()?,
        )))
    } else if kernel.starts_with(b"ASM BinaryFile8") {
        Some(u64::from_le_bytes(
            kernel
                .get(31..39)
                .ok_or("truncated Inventor assembly ASM header")?
                .try_into()?,
        ))
    } else {
        None
    };
    if entity_count != Some(1) {
        return Err(
            "Inventor assembly contains unsupported local result geometry or assembly features"
                .into(),
        );
    }
    Ok(())
}

/// Refuse any decoder loss that could produce incomplete or invented geometry.
///
/// # Errors
/// Geometry/topology/source losses and incomplete assembly placements fail.
fn validate_decode(report: &DecodeReport, assembly: bool) -> Result<()> {
    for loss in &report.losses {
        let external = assembly
            && loss.code.namespace() == "inventor"
            && loss.code.local_code() == "assembly.component-external";
        let structural = loss.code.namespace() == "inventor"
            && matches!(
                loss.code.local_code(),
                "rse.metadata-stream-malformed"
                    | "rse.bulk-stream-malformed"
                    | "rse.stream-unpaired"
            );
        let assembly_structure = assembly
            && loss.code.namespace() == "inventor"
            && matches!(
                loss.code.local_code(),
                "assembly.record-malformed"
                    | "ufrx.table-malformed"
                    | "ufrx.schema-unsupported-assembly"
            );
        if !external
            && (loss.severity >= Severity::Error
                || matches!(
                    loss.code.category(),
                    LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
                )
                || loss.code.taxonomy() == LossTaxonomy::SourceDialectUnverified
                || structural
                || assembly_structure)
        {
            return Err(format!(
                "Inventor geometry is incomplete: {}: {}",
                loss.code, loss.message
            )
            .into());
        }
    }
    Ok(())
}

/// Append a placed scene with cofactor normals and reflected corner correspondence.
///
/// # Errors
/// Singular/non-finite transforms, coordinates or normals fail.
fn append_placed(destination: &mut Scene, source: &Scene, occurrence: &Occurrence) -> Result<()> {
    let mut rows = occurrence.transform.affine_rows();
    let scales = occurrence.scale.map(cadmpeg_ir::scalar::FiniteReal::get);
    for row in &mut rows {
        for column in 0..3 {
            row[column] *= scales[column];
        }
    }
    let [a, b, c] = rows.map(|row| crate::scene::Vec3::new(row[0], row[1], row[2]));
    let cofactors = [b.cross(c), c.cross(a), a.cross(b)];
    let determinant = a.dot(cofactors[0]);
    if !determinant.is_finite()
        || determinant == 0.0
        || rows.iter().flatten().any(|x| !x.is_finite())
    {
        return Err("Inventor occurrence has a singular or non-finite transform".into());
    }
    let apply = |point: crate::scene::Vec3| -> Result<crate::scene::Vec3> {
        let value = crate::scene::Vec3::new(
            a.dot(point) + rows[0][3],
            b.dot(point) + rows[1][3],
            c.dot(point) + rows[2][3],
        );
        if !value.is_finite() {
            return Err("Inventor placed geometry has non-finite coordinates".into());
        }
        Ok(value)
    };
    let first_triangle = destination.triangles.len();
    for triangle in &source.triangles {
        let mut triangle = triangle.clone();
        for vertex in &mut triangle.vertices {
            *vertex = apply(*vertex)?;
        }
        if determinant < 0.0 {
            triangle.vertices.swap(1, 2);
        }
        destination.triangles.push(triangle);
    }
    for (index, appearance) in &source.appearances {
        let mut appearance = appearance.clone();
        if let Some(normals) = &mut appearance.normals {
            for normal in normals.iter_mut() {
                let result = crate::scene::Vec3::new(
                    cofactors[0].dot(*normal),
                    cofactors[1].dot(*normal),
                    cofactors[2].dot(*normal),
                ) * determinant.signum();
                let length = result.length();
                if !length.is_finite() || length == 0.0 {
                    return Err("Inventor placed normal is invalid".into());
                }
                *normal = result / length;
            }
        }
        if determinant < 0.0 {
            if let Some(colors) = &mut appearance.colors {
                colors.swap(1, 2);
            }
            if let Some(texcoords) = &mut appearance.texcoords {
                texcoords.swap(1, 2);
            }
            if let Some(normals) = &mut appearance.normals {
                normals.swap(1, 2);
            }
        }
        destination
            .appearances
            .insert(first_triangle + index, appearance);
    }
    for point in &source.points {
        let mut point = point.clone();
        point.position = apply(point.position)?;
        destination.points.push(point);
    }
    for line in &source.lines {
        let mut line = line.clone();
        for vertex in &mut line.vertices {
            *vertex = apply(*vertex)?;
        }
        destination.lines.push(line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Triangle, TriangleAppearance, Vec3};

    /// A typed occurrence for independent affine-math and refusal controls.
    fn occurrence(transform: [[f64; 4]; 3]) -> Occurrence {
        serde_json::from_value(serde_json::json!({
            "id": "inventor:assembly:instance#1",
            "prototype": {"scope":"unresolved"},
            "parent": {"kind":"root"},
            "ordinal": 0,
            "transform": transform,
            "scale": [1,1,1]
        }))
        .unwrap()
    }

    #[test]
    fn reflected_nonuniform_placement_preserves_outward_normals_and_corner_attributes() {
        let source = Scene {
            triangles: vec![Triangle {
                vertices: [
                    Vec3::new(1., 0., 0.),
                    Vec3::new(0., 1., 0.),
                    Vec3::new(0., 0., 1.),
                ],
                color: None,
            }],
            appearances: [(
                0,
                TriangleAppearance {
                    normals: Some([Vec3::new(1., 1., 1.).normalized(); 3]),
                    texcoords: Some([[0., 0.], [1., 0.], [0., 1.]]),
                    ..TriangleAppearance::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Scene::default()
        };
        let placement = occurrence([[-2., 0., 0., 11.], [0., 3., 0., -7.], [0., 0., 0.5, 9.]]);
        let mut destination = Scene::default();
        append_placed(&mut destination, &source, &placement).unwrap();
        assert_eq!(
            destination.triangles[0].vertices,
            [
                Vec3::new(9., -7., 9.),
                Vec3::new(11., -7., 9.5),
                Vec3::new(11., -4., 9.)
            ]
        );
        assert_eq!(
            destination.appearances[&0].texcoords,
            Some([[0., 0.], [0., 1.], [1., 0.]])
        );
        let expected = Vec3::new(-0.5, 1. / 3., 2.).normalized();
        for normal in destination.appearances[&0].normals.unwrap() {
            assert!((normal - expected).length() < 1e-12);
        }
        let [a, b, c] = destination.triangles[0].vertices;
        assert!((b - a).cross(c - a).normalized().dot(expected) > 0.999_999);
    }

    #[test]
    fn singular_placement_cannot_be_drawn_as_complete_geometry() {
        let placement = occurrence([[0., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
        let error = append_placed(&mut Scene::default(), &Scene::default(), &placement)
            .unwrap_err()
            .to_string();
        assert!(error.contains("singular"));
    }

    #[test]
    fn project_scene_limits_are_checked_before_output_allocations() {
        let mut project = Project {
            root: PathBuf::new(),
            active: HashSet::new(),
            cached: HashMap::new(),
            input_bytes: 0,
            occurrences: 0,
            primitives: MAX_PRIMITIVES,
            scene_bytes: 0,
            directory_entries: 0,
        };
        let scene = Scene {
            triangles: vec![Triangle {
                vertices: [Vec3::default(); 3],
                color: None,
            }],
            ..Scene::default()
        };
        assert!(
            project
                .reserve_scene(&scene)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert_eq!(project.primitives, MAX_PRIMITIVES);
    }
}
