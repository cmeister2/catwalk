//! Native SolidWorks solved parts and explicitly placed assembly companions.
//!
//! Parts transfer their active Parasolid B-rep through the bounded cadmpeg
//! decoder. Legacy compound assemblies resolve the addressed component-instance
//! XML, including configuration identity, visibility and metre-valued matrices.

/// Exact retained native bridge/carrier orientation restoration.
mod brep;

use crate::{
    Result,
    scene::{AlphaMode, Scene, Triangle, TriangleAppearance, Vec3},
};
use cadmpeg_codec_sldprt::SldprtCodec;
use cadmpeg_core::decode::DecodePolicy;
use cadmpeg_ir::{
    CadIr, Codec, DecodeOptions,
    appearance::AppearanceTarget,
    geometry::SolvedCurveGeometry,
    report::{
        Severity,
        loss::{LossCategory, LossTaxonomy},
    },
    topology::Sense,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Bound recursive assemblies independently of the native codec's limits.
const MAX_DEPTH: usize = 32;
/// Bound distinct source documents within one relocated project.
const MAX_FILES: usize = 128;
/// Bound the cumulative physical project input size.
const MAX_PROJECT_BYTES: u64 = 256 * 1024 * 1024;
/// Bound complete assembly occurrence expansion.
const MAX_OCCURRENCES: usize = 4096;
/// Bound retained cached geometry and its expanded uses.
const MAX_PRIMITIVES: usize = 5_000_000;
/// Bound conservative cumulative scene storage.
const MAX_SCENE_BYTES: usize = 512 * 1024 * 1024;
/// Bound filesystem directory searches for relocated authored filenames.
const MAX_DIRECTORY_ENTRIES: usize = 16_384;
/// Bound the addressed assembly document independently of the containing file.
const MAX_XML_BYTES: usize = 8 * 1024 * 1024;
/// Bound compound-directory and XML identity graphs before further allocations.
const MAX_ENTRIES: usize = 100_000;
/// The producer-qualified component-instance XML namespace.
const NAMESPACE: &str = "http://www.solidworks.com/sw2003/schema";
/// Native zlib-wrapper signature shared by legacy `SolidWorks` compound streams.
const ZLB_MAGIC: [u8; 16] = [
    0x23, 0x1d, 0xd5, 0x71, 0xda, 0x81, 0x48, 0xa2, 0xa8, 0x58, 0x98, 0xb2, 0x1b, 0x89, 0xef, 0x99,
];

/// Decode native geometry independently of the saved image streams.
///
/// # Errors
/// Incomplete geometry, unsupported assembly variants, missing companions,
/// invalid configuration/placement identity, cycles and resource limits fail.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let path = fs::canonicalize(path)?;
    let mut project = Project {
        root: path
            .parent()
            .ok_or("SolidWorks input has no parent directory")?
            .to_owned(),
        active: HashSet::new(),
        cached: HashMap::new(),
        input_bytes: 0,
        occurrences: 0,
        primitives: 0,
        scene_bytes: 0,
        directory_entries: 0,
    };
    let scene = project.document(&path, None, 0)?;
    drop(project);
    Ok(Arc::try_unwrap(scene).unwrap_or_else(|scene| (*scene).clone()))
}

/// Project-local decode cache and cumulative resource accounting.
struct Project {
    /// Canonical directory beneath which all supplied companions must remain.
    root: PathBuf,
    /// Active file/configuration pairs, detecting recursive reference cycles.
    active: HashSet<(PathBuf, Option<String>)>,
    /// Complete geometry, keyed by canonical file and requested configuration.
    cached: HashMap<(PathBuf, Option<String>), Arc<Scene>>,
    /// Physical bytes admitted before reading source documents.
    input_bytes: u64,
    /// Placed component expansions admitted before recursion.
    occurrences: usize,
    /// Cached and expanded geometry primitives charged before appending.
    primitives: usize,
    /// Conservative cached and expanded scene storage allocation charge.
    scene_bytes: usize,
    /// Directory entries inspected during case-insensitive reference resolution.
    directory_entries: usize,
}

impl Project {
    /// Decode a native source and completely resolve its selected geometry.
    ///
    /// # Errors
    /// Missing or escaped references, cycles, incomplete geometry and limits fail.
    fn document(
        &mut self,
        path: &Path,
        configuration: Option<&str>,
        depth: usize,
    ) -> Result<Arc<Scene>> {
        if depth > MAX_DEPTH {
            return Err("SolidWorks assembly nesting exceeds 32 levels".into());
        }
        if !path.starts_with(&self.root) {
            return Err("SolidWorks component escapes the supplied project directory".into());
        }
        let key = (path.to_owned(), configuration.map(str::to_owned));
        if self.active.contains(&key) {
            return Err("cyclic SolidWorks assembly reference".into());
        }
        if let Some(scene) = self.cached.get(&key) {
            return Ok(Arc::clone(scene));
        }
        if self.active.len() + self.cached.len() >= MAX_FILES {
            return Err("SolidWorks project exceeds 128 document/configuration pairs".into());
        }
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() {
            return Err("SolidWorks component is not a regular file".into());
        }
        self.input_bytes = self
            .input_bytes
            .checked_add(metadata.len())
            .ok_or("SolidWorks input size overflow")?;
        if self.input_bytes > MAX_PROJECT_BYTES {
            return Err("SolidWorks project inputs exceed 256 MiB".into());
        }
        self.active.insert(key.clone());
        let data = super::read_file(path)?;
        if data.len() as u64 != metadata.len() {
            return Err("SolidWorks input changed while being read".into());
        }
        let scene = match super::extension(path)?.as_str() {
            "sldprt" => part(&data, configuration)?,
            "sldasm" => self.assembly(&data, path, configuration, depth)?,
            _ => return Err("SolidWorks companions must be .sldprt or .sldasm documents".into()),
        };
        scene.validate()?;
        self.reserve_scene(&scene)?;
        self.active.remove(&key);
        let scene = Arc::new(scene);
        self.cached.insert(key, Arc::clone(&scene));
        Ok(scene)
    }

    /// Resolve the native component table and append every visible placement.
    ///
    /// # Errors
    /// Incomplete joins, unsupported component states and invalid placements fail.
    fn assembly(
        &mut self,
        data: &[u8],
        path: &Path,
        configuration: Option<&str>,
        depth: usize,
    ) -> Result<Scene> {
        let assembly = assembly_records(data, configuration)?;
        let mut scene = Scene::default();
        for occurrence in assembly {
            if !occurrence.visible {
                continue;
            }
            self.occurrences = self
                .occurrences
                .checked_add(1)
                .ok_or("SolidWorks occurrence count overflow")?;
            if self.occurrences > MAX_OCCURRENCES {
                return Err("SolidWorks project exceeds 4096 placed occurrences".into());
            }
            let target = self.resolve(path, &occurrence.path)?;
            if super::extension(&target)? != occurrence.extension {
                return Err("SolidWorks component kind does not match its filename".into());
            }
            let child = self.document(&target, Some(&occurrence.configuration), depth + 1)?;
            self.reserve_scene(&child)?;
            append_placed(&mut scene, &child, occurrence.transform)?;
        }
        Ok(scene)
    }

    /// Charge cached and expanded scene allocations before vector growth.
    ///
    /// # Errors
    /// Overflow and cumulative geometry resource limits fail.
    fn reserve_scene(&mut self, scene: &Scene) -> Result<()> {
        let count = scene
            .triangles
            .len()
            .checked_add(scene.points.len())
            .and_then(|n| n.checked_add(scene.lines.len()))
            .ok_or("SolidWorks primitive count overflow")?;
        self.primitives = self
            .primitives
            .checked_add(count)
            .ok_or("SolidWorks primitive count overflow")?;
        let bytes = scene
            .triangles
            .len()
            .checked_mul(std::mem::size_of::<crate::scene::Triangle>())
            .and_then(|n| {
                scene
                    .points
                    .len()
                    .checked_mul(std::mem::size_of::<crate::scene::Point>())
                    .and_then(|m| n.checked_add(m))
            })
            .and_then(|n| {
                scene
                    .lines
                    .len()
                    .checked_mul(std::mem::size_of::<crate::scene::Line>())
                    .and_then(|m| n.checked_add(m))
            })
            .and_then(|n| {
                scene
                    .appearances
                    .len()
                    .checked_mul(std::mem::size_of::<crate::scene::TriangleAppearance>() + 64)
                    .and_then(|m| n.checked_add(m))
            })
            .ok_or("SolidWorks scene size overflow")?;
        self.scene_bytes = self
            .scene_bytes
            .checked_add(bytes)
            .ok_or("SolidWorks scene size overflow")?;
        if self.primitives > MAX_PRIMITIVES || self.scene_bytes > MAX_SCENE_BYTES {
            return Err(
                "SolidWorks geometry exceeds its 5-million primitive or 512 MiB limit".into(),
            );
        }
        Ok(())
    }

    /// Relocate old absolute paths to supplied siblings, preserving exact names.
    ///
    /// # Errors
    /// Traversal, ambiguous names, missing companions and escaped symlinks fail.
    fn resolve(&mut self, owner: &Path, reference: &str) -> Result<PathBuf> {
        if reference.is_empty() || reference.len() > 4096 || reference.contains('\0') {
            return Err("invalid SolidWorks component reference".into());
        }
        let normalized = reference.replace('\\', "/");
        let absolute = normalized.starts_with('/')
            || (normalized.as_bytes().get(1) == Some(&b':')
                && normalized.as_bytes()[0].is_ascii_alphabetic());
        let components = normalized
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if components.iter().any(|part| matches!(*part, "." | "..")) {
            return Err("SolidWorks component reference contains traversal".into());
        }
        let requested = if absolute {
            &components[components
                .len()
                .checked_sub(1)
                .ok_or("SolidWorks reference has no filename")?..]
        } else {
            components.as_slice()
        };
        if requested.iter().any(|part| part.contains(':')) {
            return Err("invalid SolidWorks component reference path".into());
        }
        let mut target = owner
            .parent()
            .ok_or("SolidWorks assembly has no parent directory")?
            .to_owned();
        for component in requested {
            let mut matched = None;
            for entry in fs::read_dir(&target)? {
                self.directory_entries += 1;
                if self.directory_entries > MAX_DIRECTORY_ENTRIES {
                    return Err("SolidWorks component directory search limit exceeded".into());
                }
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(component))
                    && matched.replace(entry.path()).is_some()
                {
                    return Err("ambiguous SolidWorks component filename".into());
                }
            }
            target = fs::canonicalize(matched.ok_or_else(|| format!("missing SolidWorks companion {component}; provide its native file beside the assembly"))?)?;
            if !target.starts_with(&self.root) {
                return Err("SolidWorks component escapes the supplied project directory".into());
            }
        }
        Ok(target)
    }
}

/// Decode the active solved B-rep without including alternate configurations.
///
/// # Errors
/// Geometry loss, unresolved/alternate configuration identity and stale caches fail.
fn part(data: &[u8], requested: Option<&str>) -> Result<Scene> {
    // Native display-to-B-rep validation compares many authored trim segments.
    // Keep the service allocation/decompression ceilings while allowing one
    // billion explicitly charged work units and eight million collection
    // visits for these supported part graphs.
    let mut policy = DecodePolicy::service();
    policy.limits.max_work_units = 1_000_000_000;
    policy.limits.max_collection_items = 8_000_000;
    let decoded = SldprtCodec.decode(
        &mut Cursor::new(data),
        &DecodeOptions {
            policy,
            ..DecodeOptions::default()
        },
    )?;
    for loss in &decoded.report().losses {
        // Display-cache face ownership is independent of the solved B-rep.
        // This reader tessellates that B-rep and excludes every display cache.
        if loss.code.namespace() == "sldprt"
            && loss.code.local_code() == "tessellation.face-ownership-unresolved"
        {
            continue;
        }
        if loss.severity >= Severity::Error
            || (loss.severity >= Severity::Warning
                && (matches!(
                    loss.code.category(),
                    LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
                ) || loss.code.taxonomy() == LossTaxonomy::SourceDialectUnverified))
        {
            return Err(format!(
                "SolidWorks native geometry is incomplete ({}): {}",
                loss.code, loss.message
            )
            .into());
        }
    }
    check_part_configuration(decoded.ir(), requested)?;
    if let Some(scene) = cached_part_geometry(decoded.ir())? {
        return Ok(scene);
    }
    let (mut geometry, _, fidelity) = decoded.into_parts();
    brep::restore_face_senses(&mut geometry, &fidelity)?;
    drop(fidelity);
    geometry.native = cadmpeg_ir::Native::default();
    geometry.model.tessellations.clear();
    // A complete Parasolid graph includes boundary-free exterior regions.
    // The STEP boundary writer rejects empty regions, so omit only regions
    // without a listed or independently owned shell after the topology gate.
    let empty = geometry
        .model
        .regions
        .iter()
        .filter(|region| region.shells.is_empty())
        .map(|region| region.id.clone())
        .collect::<HashSet<_>>();
    if geometry
        .model
        .shells
        .iter()
        .any(|shell| empty.contains(&shell.region))
    {
        return Err("SolidWorks boundary-free region unexpectedly owns a shell".into());
    }
    geometry
        .model
        .regions
        .retain(|region| !empty.contains(&region.id));
    for body in &mut geometry.model.bodies {
        body.regions.retain(|region| !empty.contains(region));
        if body.regions.is_empty() {
            return Err("SolidWorks body has no transferred boundary regions".into());
        }
    }
    orient_line_edges(&mut geometry)?;
    super::native_cad::scene_from_ir(
        &geometry,
        super::native_cad::SurfaceNormalConvention::Parametric,
    )
}

/// Transfer the complete native 3D display cache of one saved solved state.
///
/// A single configuration and body establish whole-part ownership independently
/// of optional face-to-table joins. Every source table and native mesh count must
/// agree with the complete solved face inventory. Geometry coordinates remain in
/// canonical millimetres; the source's triangle winding and normals are retained.
///
/// # Errors
/// Incoherent cache counts, foreign ownership, invalid normals and limits fail.
fn cached_part_geometry(ir: &CadIr) -> Result<Option<Scene>> {
    if ir.model.bodies.len() != 1
        || ir.model.configurations.len() != 1
        || ir.model.tessellations.is_empty()
        || ir.model.tessellations.len() != ir.model.faces.len()
    {
        return Ok(None);
    }
    let body = &ir.model.bodies[0];
    if body.transform.is_some() || body.visible == Some(false) {
        return Ok(None);
    }
    let state = &ir.model.configurations[0];
    let Some(index) = state.source_index else {
        return Ok(None);
    };
    let attributes = &ir
        .source
        .as_ref()
        .ok_or("SolidWorks display cache has no source state")?
        .attributes;
    if attributes
        .get(format!("sw_configuration_{index}_needs_update").as_str())
        .map(String::as_str)
        != Some("NO")
    {
        return Ok(None);
    }
    let faces = ir
        .model
        .faces
        .iter()
        .map(|face| &face.id)
        .collect::<HashSet<_>>();
    let mut vertex_count = 0_usize;
    let mut triangle_count = 0_usize;
    let mut ids = HashSet::new();
    for mesh in &ir.model.tessellations {
        if mesh.triangle_count() == 0 {
            return Ok(None);
        }
        if !ids.insert(&mesh.id)
            || mesh.body.as_ref().is_some_and(|owner| owner != &body.id)
            || mesh.faces.iter().any(|face| !faces.contains(face))
        {
            return Err("SolidWorks display cache has inconsistent native ownership".into());
        }
        vertex_count = vertex_count
            .checked_add(mesh.vertex_count())
            .ok_or("SolidWorks cache vertex count overflow")?;
        triangle_count = triangle_count
            .checked_add(mesh.triangle_count())
            .ok_or("SolidWorks cache triangle count overflow")?;
    }
    for (key, count) in [
        ("displaylist_vertices", vertex_count),
        ("displaylist_triangles", triangle_count),
    ] {
        if attributes
            .get(key)
            .and_then(|text| text.parse::<usize>().ok())
            != Some(count)
        {
            return Err(
                "SolidWorks display cache is incomplete against its native source inventory".into(),
            );
        }
    }
    let bytes = triangle_count
        .checked_mul(
            std::mem::size_of::<Triangle>() + std::mem::size_of::<TriangleAppearance>() + 64 + 12,
        )
        .and_then(|bytes| {
            vertex_count
                .checked_mul(48)
                .and_then(|vertices| bytes.checked_add(vertices))
        })
        .ok_or("SolidWorks display geometry allocation overflow")?;
    if triangle_count > MAX_PRIMITIVES || vertex_count > MAX_PRIMITIVES || bytes > MAX_SCENE_BYTES {
        return Err("SolidWorks native display geometry exceeds the scene resource limits".into());
    }
    let colors = cached_colors(ir)?;
    let mut scene = Scene::default();
    scene.triangles.try_reserve_exact(triangle_count)?;
    for mesh in &ir.model.tessellations {
        let color = colors
            .get(mesh.id.as_str())
            .copied()
            .flatten()
            .or(body.color);
        append_cached_mesh(&mut scene, mesh, color)?;
    }
    Ok(Some(scene))
}

/// Resolve explicit native per-table diffuse appearance assignments.
///
/// # Errors
/// Unresolved appearance identities and duplicate assignments fail.
fn cached_colors(ir: &CadIr) -> Result<HashMap<&str, Option<cadmpeg_ir::topology::Color>>> {
    let appearances = ir
        .model
        .appearances
        .iter()
        .map(|appearance| (&appearance.id, appearance.base_color))
        .collect::<HashMap<_, _>>();
    let mut colors = HashMap::new();
    for binding in &ir.model.appearance_bindings {
        if let AppearanceTarget::Tessellation(target) = &binding.target {
            let color = *appearances
                .get(&binding.appearance)
                .ok_or("SolidWorks display appearance is unresolved")?;
            if colors.insert(target.as_str(), color).is_some() {
                return Err("SolidWorks display appearance assignment is ambiguous".into());
            }
        }
    }
    Ok(colors)
}

/// Transfer one complete source mesh after its allocation admission.
///
/// # Errors
/// Incomplete source shading normals and non-finite or zero normal vectors fail.
fn append_cached_mesh(
    scene: &mut Scene,
    mesh: &cadmpeg_ir::tessellation::Tessellation,
    color: Option<cadmpeg_ir::topology::Color>,
) -> Result<()> {
    let positions = mesh.vertices();
    let normals = mesh.vertex_normals();
    let corners = mesh.per_corner_normals();
    if (!normals.is_empty() && normals.len() != positions.len())
        || (!corners.is_empty() && corners.len() != mesh.triangle_count() * 3)
    {
        return Err("SolidWorks native display normals are incomplete".into());
    }
    for (ordinal, triangle) in mesh.triangles().into_iter().enumerate() {
        let indices = [
            usize::try_from(triangle[0])?,
            usize::try_from(triangle[1])?,
            usize::try_from(triangle[2])?,
        ];
        let positions = [0, 1, 2].map(|corner| positions[indices[corner]].get());
        let vertices = positions.map(|point| Vec3::new(point.x, point.y, point.z));
        let index = scene.triangles.len();
        scene.triangles.push(Triangle {
            vertices,
            color: color.map(rgb8),
        });
        let source_normals = if !normals.is_empty() {
            Some([0, 1, 2].map(|corner| normals[indices[corner]].get()))
        } else if !corners.is_empty() {
            Some([0, 1, 2].map(|corner| corners[ordinal * 3 + corner].get()))
        } else {
            None
        };
        let mut appearance = TriangleAppearance::default();
        if let Some(source_normals) = source_normals {
            let normals = source_normals.map(|normal| Vec3::new(normal.x, normal.y, normal.z));
            if normals
                .iter()
                .any(|normal| !normal.length().is_finite() || normal.length() == 0.)
            {
                return Err("SolidWorks native display normal is invalid".into());
            }
            appearance.normals = Some(normals.map(Vec3::normalized));
        }
        if let Some(color) = color {
            let linear = |value: f32| {
                let value = f64::from(value);
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            appearance.base_color = [
                linear(color.r()),
                linear(color.g()),
                linear(color.b()),
                f64::from(color.a()),
            ];
            if color.a() < 1. {
                appearance.alpha_mode = AlphaMode::Blend;
            }
        }
        scene.appearances.insert(index, appearance);
    }
    Ok(())
}

/// Convert already admitted native RGB unit channels to source color bytes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgb8(color: cadmpeg_ir::topology::Color) -> [u8; 3] {
    [color.r(), color.g(), color.b()].map(|channel| (channel * 255.).round() as u8)
}

/// Align straight carrier endpoints while preserving every oriented vertex walk.
///
/// The native kernel chooses a canonical edge use independently of the line's
/// parameter direction. STEP's neutral writer assumes increasing parameters.
/// Swapping descending endpoints and reversing every associated use preserves
/// each face's ordered vertex walk, its physical segment and its pcurve chart.
///
/// # Errors
/// Missing line endpoint identities or non-finite interval tests fail.
fn orient_line_edges(ir: &mut CadIr) -> Result<()> {
    let points = ir
        .model
        .points
        .iter()
        .map(|point| (&point.id, point.position().get()))
        .collect::<HashMap<_, _>>();
    let vertices = ir
        .model
        .vertices
        .iter()
        .map(|vertex| (&vertex.id, &vertex.point))
        .collect::<HashMap<_, _>>();
    let curves = ir
        .model
        .curves
        .iter()
        .map(|curve| (&curve.id, &curve.geometry))
        .collect::<HashMap<_, _>>();
    let mut reversed = HashSet::new();
    for edge in &mut ir.model.edges {
        let Some(SolvedCurveGeometry::Line(line)) = edge
            .curve()
            .and_then(|id| curves.get(id))
            .and_then(|curve| curve.solved())
        else {
            continue;
        };
        let start = vertices
            .get(&edge.start)
            .and_then(|point| points.get(point))
            .ok_or("SolidWorks line edge start vertex is unresolved")?;
        let end = vertices
            .get(&edge.end)
            .and_then(|point| points.get(point))
            .ok_or("SolidWorks line edge end vertex is unresolved")?;
        let direction = *line.direction().as_raw();
        let interval = (end.x - start.x) * direction.x
            + (end.y - start.y) * direction.y
            + (end.z - start.z) * direction.z;
        if !interval.is_finite() {
            return Err("SolidWorks line edge interval is non-finite".into());
        }
        if interval < 0. {
            std::mem::swap(&mut edge.start, &mut edge.end);
            reversed.insert(edge.id.clone());
        }
    }
    for coedge in &mut ir.model.coedges {
        if reversed.contains(&coedge.edge) {
            coedge.sense = match coedge.sense {
                Sense::Forward => Sense::Reversed,
                Sense::Reversed => Sense::Forward,
            };
        }
    }
    Ok(())
}

/// Prove the transferred body graph is exactly the selected native configuration.
///
/// # Errors
/// Missing/ambiguous identities, stale data and multiple transferred states fail.
fn check_part_configuration(ir: &CadIr, requested: Option<&str>) -> Result<()> {
    let mut active = ir.model.configurations.iter().filter(|state| state.active);
    let selected = active
        .next()
        .ok_or("SolidWorks part has no resolved active configuration")?;
    if active.next().is_some() {
        return Err("SolidWorks part has ambiguous active configurations".into());
    }
    if requested.is_some_and(|name| selected.name.as_deref() != Some(name)) {
        return Err("SolidWorks component requests a configuration other than its transferred active geometry".into());
    }
    let name = selected
        .name
        .as_deref()
        .ok_or("SolidWorks active configuration has no native name")?;
    if name.is_empty()
        || ir
            .source
            .as_ref()
            .and_then(|source| source.attributes.get("sw_configuration_name"))
            .map(String::as_str)
            != Some(name)
    {
        return Err("SolidWorks active geometry configuration identity is unresolved".into());
    }
    let bodies = selected
        .bodies
        .as_ref()
        .ok_or("SolidWorks active configuration has unresolved body membership")?;
    let body_ids = ir
        .model
        .bodies
        .iter()
        .map(|body| &body.id)
        .collect::<HashSet<_>>();
    if bodies.len() != body_ids.len() || bodies.iter().any(|body| !body_ids.contains(body)) {
        return Err("SolidWorks part contains geometry from unselected configurations".into());
    }
    if let Some(index) = selected.source_index
        && ir
            .source
            .as_ref()
            .and_then(|source| {
                source
                    .attributes
                    .get(format!("sw_configuration_{index}_needs_update").as_str())
            })
            .is_some_and(|value| value != "NO")
    {
        return Err("SolidWorks part configuration requires a native rebuild".into());
    }
    Ok(())
}

/// One exact model/file join and its authored selected-configuration placement.
struct Occurrence {
    /// Persisted native file path, relocated to the supplied project.
    path: String,
    /// Part or assembly file extension qualified by swDocType.
    extension: &'static str,
    /// Native configuration name requested by this occurrence.
    configuration: String,
    /// Visibility after suppression and hiding are both applied.
    visible: bool,
    /// Affine matrix in column-major order, with translation in millimetres.
    transform: [[f64; 4]; 4],
}

/// Decode only the addressed legacy assembly component-instance stream.
///
/// # Errors
/// Unsupported envelopes, stale/missing XML, ambiguous identities and states fail.
fn assembly_records(data: &[u8], requested: Option<&str>) -> Result<Vec<Occurrence>> {
    let xml = assembly_xml(data)?;
    let text = std::str::from_utf8(&xml)?;
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: u32::try_from(MAX_ENTRIES)?,
            ..roxmltree::ParsingOptions::default()
        },
    )?;
    let root = document.root_element();
    if !is_tag(root, "swSolidWorks") || attribute(root, "swVersion")? != "7000" {
        return Err("unsupported SolidWorks assembly component-instance XML dialect".into());
    }
    let file_list = child(root, "swHeader")?;
    let model_list = child(root, "swModelList")?;
    let configurations = child(root, "swConfigurationList")?;
    let extension_features = child(root, "swExtFeatureList")?;
    if extension_features.children().any(|node| node.is_element()) {
        return Err("SolidWorks assembly has unsupported external feature operations".into());
    }
    check_count(file_list)?;
    check_count(model_list)?;
    check_count(configurations)?;
    check_count(extension_features)?;
    let mut files = HashMap::new();
    for file in file_list.children().filter(roxmltree::Node::is_element) {
        if !is_tag(file, "swFile") || files.insert(attribute(file, "id")?, file).is_some() {
            return Err("SolidWorks assembly has duplicate or unsupported file identities".into());
        }
    }
    let mut models = HashMap::new();
    for model in model_list.children().filter(roxmltree::Node::is_element) {
        if !is_tag(model, "swModel") || models.insert(attribute(model, "id")?, model).is_some() {
            return Err("SolidWorks assembly has duplicate or unsupported model identities".into());
        }
    }
    let configuration = selected_configuration(configurations, requested)?;
    let model = *models
        .get(attribute(configuration, "swModelRef")?)
        .ok_or("SolidWorks assembly selected model reference is unresolved")?;
    let root_file = *files
        .get(attribute(model, "swFileRef")?)
        .ok_or("SolidWorks assembly root file reference is unresolved")?;
    if attribute(root_file, "swDocType")? != "ASSEMBLY"
        || attribute(model, "swConfigurationName")? != attribute(configuration, "swName")?
    {
        return Err(
            "SolidWorks assembly selected configuration has inconsistent root identity".into(),
        );
    }
    if !attribute(model, "swAssemblyFeatureEffectedComponents")?.is_empty() {
        return Err("SolidWorks assembly features alter component result geometry".into());
    }
    occurrence_records(model, configuration, &files, &models)
}

/// Exact document-local identity maps from framed native XML records.
type NativeNodes<'a, 'input> = HashMap<&'a str, roxmltree::Node<'a, 'input>>;

/// Join all selected component records without inventing absent state.
///
/// # Errors
/// Incomplete file/model/configuration joins and unsupported placements fail.
fn occurrence_records(
    model: roxmltree::Node<'_, '_>,
    selected: roxmltree::Node<'_, '_>,
    files: &NativeNodes<'_, '_>,
    models: &NativeNodes<'_, '_>,
) -> Result<Vec<Occurrence>> {
    let display = display_visibility(selected)?;
    let mut references = HashSet::new();
    let mut component_ids = HashSet::new();
    let mut result = Vec::new();
    for reference in model.children().filter(roxmltree::Node::is_element) {
        if !is_tag(reference, "swReference")
            || !references.insert(attribute(reference, "id")?)
            || !component_ids.insert(attribute(reference, "swID")?)
        {
            return Err(
                "SolidWorks assembly has duplicate or unsupported component identities".into(),
            );
        }
        if yes_no(reference, "swIsVirtualComponent")? || yes_no(reference, "swFlexible")? {
            return Err(
                "SolidWorks virtual or flexible assembly components are unsupported".into(),
            );
        }
        let prototype = *models
            .get(attribute(reference, "swModelRef")?)
            .ok_or("SolidWorks component model reference is unresolved")?;
        let file = *files
            .get(attribute(prototype, "swFileRef")?)
            .ok_or("SolidWorks component file reference is unresolved")?;
        let configuration = attribute(reference, "swConfigurationName")?;
        if configuration != attribute(prototype, "swConfigurationName")?
            || attribute(reference, "swConfigurationId")?
                != attribute(prototype, "swConfigurationId")?
        {
            return Err("SolidWorks component configuration joins disagree".into());
        }
        let extension = match attribute(file, "swDocType")? {
            "PART" => "sldprt",
            "ASSEMBLY" => "sldasm",
            _ => return Err("unsupported SolidWorks component document kind".into()),
        };
        let shown = if let Some(states) = &display {
            *states
                .get(attribute(reference, "swID")?)
                .ok_or("SolidWorks display-state component identity is unresolved")?
        } else {
            true
        };
        let visible =
            !yes_no(reference, "swSuppressed")? && !yes_no(reference, "swHidden")? && shown;
        result.push(Occurrence {
            path: attribute(file, "swPath")?.to_owned(),
            extension,
            configuration: configuration.to_owned(),
            visible,
            transform: placement(attribute(reference, "swTransform")?)?,
        });
        if result.len() > MAX_OCCURRENCES {
            return Err("SolidWorks assembly exceeds 4096 direct occurrences".into());
        }
    }
    if display
        .as_ref()
        .is_some_and(|states| states.len() != component_ids.len())
    {
        return Err("SolidWorks display-state component coverage is incomplete".into());
    }
    if result.is_empty() {
        return Err("SolidWorks assembly contains no selected component occurrences".into());
    }
    Ok(result)
}

/// Validate declared native record counts against their complete addressed lists.
///
/// # Errors
/// Malformed counts or omitted/extra records fail.
fn check_count(node: roxmltree::Node<'_, '_>) -> Result<()> {
    let declared: usize = attribute(node, "swObjCount")?.parse()?;
    if declared != node.children().filter(roxmltree::Node::is_element).count() {
        return Err("SolidWorks assembly native record count does not match its list".into());
    }
    Ok(())
}

/// Resolve the selected display state's explicit component visibility table.
///
/// # Errors
/// Missing, duplicate or unqualified visibility states fail.
fn display_visibility<'a>(
    configuration: roxmltree::Node<'a, '_>,
) -> Result<Option<HashMap<&'a str, bool>>> {
    let children = configuration
        .children()
        .filter(roxmltree::Node::is_element)
        .collect::<Vec<_>>();
    if children.is_empty() {
        return Ok(None);
    }
    if children.len() != 1 || !is_tag(children[0], "swDisplayStateList") {
        return Err("SolidWorks configuration has unsupported display-state structures".into());
    }
    let list = children[0];
    let selected = attribute(configuration, "swLinkDS")?.trim_end_matches(',');
    if selected.is_empty() || selected.contains(',') {
        return Err("SolidWorks selected display-state identity is ambiguous".into());
    }
    let mut state = None;
    let mut names = HashSet::new();
    for candidate in list.children().filter(roxmltree::Node::is_element) {
        if !is_tag(candidate, "swDisplayState")
            || !names.insert(attribute(candidate, "swDisplayStateName")?)
        {
            return Err("SolidWorks display-state identities are unsupported or duplicated".into());
        }
        if attribute(candidate, "swDisplayStateName")? == selected
            && state.replace(candidate).is_some()
        {
            return Err("SolidWorks selected display state is ambiguous".into());
        }
    }
    let state = state.ok_or("SolidWorks selected display state is unresolved")?;
    let ids = attribute(list, "swCompNameList")?
        .split_terminator(',')
        .collect::<Vec<_>>();
    let visible = attribute(state, "swShowState")?
        .split_terminator(',')
        .collect::<Vec<_>>();
    if ids.len() != visible.len() {
        return Err("SolidWorks display-state visibility count is incomplete".into());
    }
    let mut states = HashMap::new();
    for (id, token) in ids.into_iter().zip(visible) {
        let visible = match token {
            "Yes" => true,
            "No" => false,
            _ => return Err("unsupported SolidWorks display visibility state".into()),
        };
        if id.is_empty() || states.insert(id, visible).is_some() {
            return Err("SolidWorks display-state component identities are duplicated".into());
        }
    }
    Ok(Some(states))
}

/// Select one saved configuration by its exact native name or active flag.
///
/// # Errors
/// Duplicate, absent, ambiguous or stale selected configuration identities fail.
fn selected_configuration<'a, 'input>(
    configurations: roxmltree::Node<'a, 'input>,
    requested: Option<&str>,
) -> Result<roxmltree::Node<'a, 'input>> {
    let mut selected = None;
    let mut config_ids = HashSet::new();
    let mut config_names = HashSet::new();
    for configuration in configurations
        .children()
        .filter(roxmltree::Node::is_element)
    {
        if !is_tag(configuration, "swConfiguration")
            || !config_ids.insert(attribute(configuration, "id")?)
            || !config_names.insert(attribute(configuration, "swName")?)
        {
            return Err(
                "SolidWorks assembly has duplicate or unsupported configuration identities".into(),
            );
        }
        let choose = if let Some(name) = requested {
            attribute(configuration, "swName")? == name
        } else {
            yes_no(configuration, "swMostRecentConfiguration")?
        };
        if choose && selected.replace(configuration).is_some() {
            return Err("SolidWorks assembly has ambiguous selected configurations".into());
        }
    }
    let configuration =
        selected.ok_or("SolidWorks assembly has no matching saved configuration")?;
    if yes_no(configuration, "swConfigurationNeedsUpdate")? {
        return Err("SolidWorks assembly configuration requires a native rebuild".into());
    }
    Ok(configuration)
}

/// Admit and read the uniquely addressed compound component-instance stream.
///
/// # Errors
/// Excessive containers, unsupported local bodies and ambiguous streams fail.
fn assembly_xml(data: &[u8]) -> Result<Vec<u8>> {
    let mut compound = cfb::CompoundFile::open(Cursor::new(data))?;
    let entries = compound.walk().take(MAX_ENTRIES + 1).collect::<Vec<_>>();
    if entries.len() > MAX_ENTRIES {
        return Err("SolidWorks assembly exceeds 100,000 compound entries".into());
    }
    // Solved assembly-result bodies need their own qualified ownership; component
    // XML cannot establish the effect of assembly features on those bodies.
    if entries.iter().any(|entry| {
        entry.is_stream()
            && entry
                .path()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("-partition")
            || entry.is_stream()
                && entry
                    .path()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with("-partition__zlb")
    }) {
        return Err("SolidWorks assembly contains unsupported local result geometry".into());
    }
    let bare = "/swXmlContents/COMPINSTANCETREE";
    let wrapped = "/swXmlContents/COMPINSTANCETREE__ZLB";
    let address = match (compound.exists(bare), compound.exists(wrapped)) {
        (true, false) => bare,
        (false, true) => wrapped,
        (false, false) => {
            return Err("SolidWorks assembly has no addressed component-instance tree".into());
        }
        (true, true) => {
            return Err("SolidWorks assembly has ambiguous component-instance streams".into());
        }
    };
    let mut xml = Vec::new();
    compound
        .open_stream(address)?
        .take((MAX_XML_BYTES + 33) as u64)
        .read_to_end(&mut xml)?;
    if xml.len() > MAX_XML_BYTES + 32 {
        return Err("SolidWorks assembly component-instance stream exceeds 8 MiB".into());
    }
    if address == wrapped {
        xml = inflate_xml(&xml)?;
    }
    Ok(xml)
}

/// Decode the native zlib envelope with exact physical and expanded lengths.
///
/// # Errors
/// Invalid framing, trailing members or expanded resource limits fail.
fn inflate_xml(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 32 || data[..16] != ZLB_MAGIC {
        return Err("malformed SolidWorks assembly zlib wrapper".into());
    }
    let expanded = u32::from_le_bytes(data[16..20].try_into()?) as usize;
    let compressed = u32::from_le_bytes(data[20..24].try_into()?) as usize;
    if expanded > MAX_XML_BYTES || compressed.checked_add(32) != Some(data.len()) {
        return Err("SolidWorks assembly zlib wrapper has invalid lengths".into());
    }
    let mut decoder = flate2::read::ZlibDecoder::new(&data[24..24 + compressed]);
    let mut output = Vec::new();
    (&mut decoder)
        .take((expanded + 1) as u64)
        .read_to_end(&mut output)?;
    if output.len() != expanded || decoder.total_in() != compressed as u64 {
        return Err("SolidWorks assembly XML inflation does not match its framed length".into());
    }
    Ok(output)
}

/// Require an exact qualified native XML element name.
fn is_tag(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node.tag_name().namespace() == Some(NAMESPACE)
}

/// Require one uniquely addressed immediate XML child.
///
/// # Errors
/// Missing or duplicated addressed structures fail.
fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Result<roxmltree::Node<'a, 'input>> {
    let mut children = node.children().filter(|child| is_tag(*child, name));
    let result = children
        .next()
        .ok_or_else(|| format!("SolidWorks assembly is missing {name}"))?;
    if children.next().is_some() {
        return Err(format!("SolidWorks assembly has duplicate {name} structures").into());
    }
    Ok(result)
}

/// Read a required native attribute without silently inventing absent state.
///
/// # Errors
/// Missing or excessive attributes fail.
fn attribute<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Result<&'a str> {
    let value = node
        .attribute(name)
        .ok_or_else(|| format!("SolidWorks assembly is missing {name}"))?;
    if value.len() > 4096 {
        return Err("SolidWorks assembly attribute exceeds 4096 bytes".into());
    }
    Ok(value)
}

/// Read the native two-valued state fields explicitly.
///
/// # Errors
/// Missing or unknown state tokens fail.
fn yes_no(node: roxmltree::Node<'_, '_>, name: &str) -> Result<bool> {
    match attribute(node, name)? {
        "YES" => Ok(true),
        "NO" => Ok(false),
        _ => Err(format!("SolidWorks assembly {name} has an unsupported state").into()),
    }
}

/// Parse the producer's column-major rigid matrix and convert metres to millimetres.
///
/// # Errors
/// Non-finite, projective, singular, reflected or non-rigid matrices fail.
#[allow(clippy::float_cmp)] // Homogeneous affine sentinel fields are exact native constants.
fn placement(text: &str) -> Result<[[f64; 4]; 4]> {
    let values = text
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if values.len() != 16 || values.iter().any(|number| !number.is_finite()) {
        return Err("SolidWorks placement needs 16 finite matrix values".into());
    }
    let mut matrix: [[f64; 4]; 4] =
        std::array::from_fn(|column| std::array::from_fn(|row| values[column * 4 + row]));
    if [matrix[0][3], matrix[1][3], matrix[2][3], matrix[3][3]] != [0., 0., 0., 1.] {
        return Err("SolidWorks assembly placement is not affine".into());
    }
    let columns =
        [0, 1, 2].map(|index| Vec3::new(matrix[index][0], matrix[index][1], matrix[index][2]));
    if columns
        .iter()
        .any(|vector| (vector.dot(*vector) - 1.).abs() > 1e-8)
        || columns[0].dot(columns[1]).abs() > 1e-8
        || columns[1].dot(columns[2]).abs() > 1e-8
        || columns[0].dot(columns[2]).abs() > 1e-8
        || (columns[0].dot(columns[1].cross(columns[2])) - 1.).abs() > 1e-8
    {
        return Err("unsupported non-rigid SolidWorks assembly placement".into());
    }
    for value in &mut matrix[3][..3] {
        *value *= 1000.;
    }
    if matrix[3].iter().any(|number| !number.is_finite()) {
        return Err("SolidWorks placement unit conversion overflow".into());
    }
    Ok(matrix)
}

/// Append a complete scene under one qualified rigid native placement.
///
/// # Errors
/// Transformed non-finite coordinates or invalid normals fail.
fn append_placed(destination: &mut Scene, source: &Scene, matrix: [[f64; 4]; 4]) -> Result<()> {
    let apply = |point: Vec3, translate: bool| -> Result<Vec3> {
        let values = [point.x, point.y, point.z, if translate { 1. } else { 0. }];
        let output: [f64; 3] = std::array::from_fn(|row| {
            (0..4)
                .map(|column| matrix[column][row] * values[column])
                .sum()
        });
        let result = Vec3::new(output[0], output[1], output[2]);
        if !result.is_finite() {
            return Err("SolidWorks placed geometry is non-finite".into());
        }
        Ok(result)
    };
    let first = destination.triangles.len();
    for triangle in &source.triangles {
        let mut triangle = triangle.clone();
        for vertex in &mut triangle.vertices {
            *vertex = apply(*vertex, true)?;
        }
        destination.triangles.push(triangle);
    }
    for (index, appearance) in &source.appearances {
        let mut appearance = appearance.clone();
        if let Some(normals) = &mut appearance.normals {
            for normal in normals {
                let transformed = apply(*normal, false)?;
                let length = transformed.length();
                if !length.is_finite() || length == 0. {
                    return Err("SolidWorks placed normal is invalid".into());
                }
                *normal = transformed / length;
            }
        }
        destination.appearances.insert(first + index, appearance);
    }
    for point in &source.points {
        let mut point = point.clone();
        point.position = apply(point.position, true)?;
        destination.points.push(point);
    }
    for line in &source.lines {
        let mut line = line.clone();
        for vertex in &mut line.vertices {
            *vertex = apply(*vertex, true)?;
        }
        destination.lines.push(line);
    }
    Ok(())
}
