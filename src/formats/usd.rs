//! Static USD stages, composed by the MIT-licensed, entirely Rust openusd reader.
//! USD row-vector transforms are transposed into our column-vector convention.
use super::{MAX_INPUT_BYTES, mesh::triangulate_polygon, read_file, transform::Transform};
use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use openusd::{
    ar::{self, Resolver},
    gf,
    schemas::{
        geom::{Imageable, Xformable},
        shade,
    },
    sdf,
    usd::{self, Prim, SchemaBase},
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    io::{self, Cursor, Read},
    path::{Component, Path, PathBuf},
};

const MAX_ELEMENTS: usize = 5_000_000;
const MAX_PRIMS: usize = 100_000;

/// Every layer and every decompressed ZIP member passes through this resolver.
/// In particular, opening USDZ's root through `package[root.usdc]` prevents the
/// upstream package-format decoder from performing an unbounded ZIP read.
struct LocalResolver {
    root: PathBuf,
    total: Cell<u64>,
}

impl LocalResolver {
    fn parts(&self, identifier: &str) -> Option<(PathBuf, Option<String>)> {
        let (outer, inner) = if ar::is_package_relative_path(identifier) {
            let (outer, inner) = ar::split_package_relative_path_outer(identifier)?;
            if !safe_member(&inner) {
                return None;
            }
            (outer, Some(inner))
        } else {
            (identifier.to_owned(), None)
        };
        let path = Path::new(&outer).canonicalize().ok()?;
        if !path.starts_with(&self.root)
            || !path.is_file()
            || path.metadata().ok()?.len() > MAX_INPUT_BYTES
        {
            return None;
        }
        Some((path, inner))
    }
}

fn safe_member(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0', '[', ']'])
        && !Path::new(name).is_absolute()
        && !Path::new(name).components().any(|p| {
            matches!(
                p,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

impl Resolver for LocalResolver {
    fn create_identifier(&self, asset: &str, anchor: Option<&ar::ResolvedPath>) -> String {
        ar::DefaultResolver::new().create_identifier(asset, anchor)
    }
    fn resolve(&self, identifier: &str) -> Option<ar::ResolvedPath> {
        let (outer, mut inner) = self.parts(identifier)?;
        if inner.is_none()
            && outer
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("usdz"))
        {
            inner = Some(package_root(&outer).ok()?);
        }
        if let Some(inner) = inner {
            let mut archive = zip::ZipArchive::new(std::fs::File::open(&outer).ok()?).ok()?;
            let file = archive.by_name(&inner).ok()?;
            if file.size() > MAX_INPUT_BYTES || !file.is_file() {
                return None;
            }
            Some(ar::ResolvedPath::new(format!(
                "{}[{inner}]",
                outer.to_str()?
            )))
        } else {
            if outer.metadata().ok()?.len() > MAX_INPUT_BYTES {
                return None;
            }
            Some(ar::ResolvedPath::new(outer))
        }
    }
    fn resolve_for_new_asset(&self, _: &str) -> Option<ar::ResolvedPath> {
        None
    }
    fn open_asset(&self, path: &ar::ResolvedPath) -> io::Result<Box<dyn ar::Asset>> {
        let (outer, inner) = self.parts(&path.to_string_lossy()).ok_or_else(|| {
            io::Error::other("USD dependency must stay within the model directory")
        })?;
        let data = if let Some(inner) = inner {
            let mut archive =
                zip::ZipArchive::new(std::fs::File::open(outer)?).map_err(io::Error::other)?;
            let member = archive.by_name(&inner).map_err(io::Error::other)?;
            if member.size() > MAX_INPUT_BYTES {
                return Err(io::Error::other("USD package member exceeds 512 MiB"));
            }
            let mut bytes = Vec::new();
            member.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
            bytes
        } else {
            read_file(&outer).map_err(io::Error::other)?
        };
        let total = self.total.get().saturating_add(data.len() as u64);
        if data.len() as u64 > MAX_INPUT_BYTES || total > MAX_INPUT_BYTES {
            return Err(io::Error::other("USD layer reads exceed the 512 MiB limit"));
        }
        self.total.set(total);
        preflight_layer(&data).map_err(io::Error::other)?;
        Ok(Box::new(Cursor::new(data)))
    }
    fn identity(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

fn package_root(path: &Path) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    if archive.is_empty() {
        return Err("USDZ package is empty".into());
    }
    let member = archive.by_index(0)?;
    let name = member.name();
    if !safe_member(name)
        || !matches!(
            Path::new(name).extension().and_then(|e| e.to_str()),
            Some("usd" | "usda" | "usdc")
        )
    {
        return Err("USDZ's first member must be a safe USD root layer".into());
    }
    if member.size() > MAX_INPUT_BYTES {
        return Err("USDZ root layer exceeds 512 MiB".into());
    }
    Ok(name.to_owned())
}

fn preflight_layer(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(b"PXR-USDC") {
        let version = bytes.get(8..11).ok_or("truncated USDC bootstrap")?;
        if version[0] != 0 || version[1] < 4 {
            return Err(format!(
                "USDC {}.{}.{} is unsupported; binary USD needs version 0.4.0 or newer",
                version[0], version[1], version[2]
            )
            .into());
        }
        let read_u64 = |offset: usize| -> Result<u64> {
            Ok(u64::from_le_bytes(
                bytes
                    .get(offset..offset.checked_add(8).ok_or("USDC offset overflow")?)
                    .ok_or("truncated USDC section")?
                    .try_into()?,
            ))
        };
        let toc = usize::try_from(read_u64(16)?)?;
        let count = usize::try_from(read_u64(toc)?)?;
        if !(1..64).contains(&count) {
            return Err("invalid USDC section count".into());
        }
        for i in 0..count {
            let entry = toc.checked_add(8 + i * 32).ok_or("USDC TOC overflow")?;
            let name = bytes.get(entry..entry + 16).ok_or("truncated USDC TOC")?;
            let name = std::str::from_utf8(name.split(|&b| b == 0).next().unwrap_or_default())?;
            let start = usize::try_from(read_u64(entry + 16)?)?;
            let size = usize::try_from(read_u64(entry + 24)?)?;
            let end = start.checked_add(size).ok_or("USDC section overflow")?;
            if end > bytes.len() {
                return Err("USDC section lies outside the input".into());
            }
            if matches!(
                name,
                "TOKENS" | "STRINGS" | "FIELDS" | "FIELDSETS" | "PATHS" | "SPECS"
            ) && read_u64(start)? > MAX_ELEMENTS as u64
            {
                return Err("USDC section exceeds five million entries".into());
            }
            if name == "TOKENS" && read_u64(start + 8)? > MAX_INPUT_BYTES {
                return Err("USDC decoded token table exceeds 512 MiB".into());
            }
            let (mut cursor, streams) = match name {
                "TOKENS" => (start + 16, 1),
                "FIELDS" => (start + 8, 2),
                "FIELDSETS" => (start + 8, 1),
                "PATHS" => {
                    if read_u64(start + 8)? > MAX_ELEMENTS as u64 {
                        return Err("USDC encoded path table exceeds five million entries".into());
                    }
                    (start + 16, 3)
                }
                "SPECS" => (start + 8, 3),
                _ => (start, 0),
            };
            for _ in 0..streams {
                let len = usize::try_from(read_u64(cursor)?)?;
                cursor = cursor
                    .checked_add(8)
                    .ok_or("USDC compression offset overflow")?;
                let next = cursor
                    .checked_add(len)
                    .ok_or("USDC compression length overflow")?;
                if next > end || len == 0 {
                    return Err("invalid USDC compressed section length".into());
                }
                if bytes[cursor] != 0 {
                    return Err("USDC chunked LZ4 sections are unsupported".into());
                }
                cursor = next;
            }
        }
    }
    Ok(())
}

struct View(Prim);
impl SchemaBase for View {
    const KIND: usd::SchemaKind = usd::SchemaKind::AbstractTyped;
    fn prim(&self) -> &Prim {
        &self.0
    }
}
impl Imageable for View {}
impl Xformable for View {}

#[derive(Clone)]
struct State {
    world: Transform,
    hidden: bool,
    purpose: String,
}

pub(super) fn load(path: &Path) -> Result<Scene> {
    let path = path.canonicalize()?;
    let root = path
        .parent()
        .ok_or("USD input has no parent directory")?
        .to_owned();
    let mut identifier = path
        .to_str()
        .ok_or("USD paths must be valid UTF-8")?
        .to_owned();
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("usdz"))
    {
        if path.metadata()?.len() > MAX_INPUT_BYTES {
            return Err("USDZ input exceeds 512 MiB".into());
        }
        let name = package_root(&path)?;
        identifier = format!("{identifier}[{name}]");
    }
    let stage = usd::Stage::builder()
        .resolver(LocalResolver {
            root,
            total: Cell::new(0),
        })
        .open(&identifier)
        .map_err(|e| e.to_string())?;
    let time = if stage.has_authored_time_code_range() {
        usd::TimeCode::new(stage.start_time_code())
    } else {
        usd::TimeCode::EARLIEST
    };
    let up = stage
        .stage_metadata("upAxis")
        .map_err(|e| e.to_string())?
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "Y".into());
    let basis = match up.as_str() {
        "Z" => Transform::IDENTITY,
        "Y" => Transform([
            [1., 0., 0., 0.],
            [0., 0., 1., 0.],
            [0., -1., 0., 0.],
            [0., 0., 0., 1.],
        ]),
        _ => return Err(format!("unsupported USD upAxis {up}").into()),
    };
    let mut paths = Vec::new();
    let mut overflow = false;
    stage
        .traverse(usd::PrimPredicate::DEFAULT_PROXIES, |p| {
            if paths.len() >= MAX_PRIMS {
                overflow = true;
            } else {
                paths.push(p.clone());
            }
        })
        .map_err(|e| e.to_string())?;
    if overflow {
        return Err("USD stage exceeds 100,000 prims".into());
    }
    let mut prototypes = HashSet::new();
    for path in &paths {
        let prim = stage.prim(path.clone());
        if prim
            .type_name()
            .map_err(|e| e.to_string())?
            .is_some_and(|t| t.as_str() == "PointInstancer")
        {
            prototypes.extend(
                prim.relationship("prototypes")
                    .targets()
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    let mut states: HashMap<sdf::Path, State> = HashMap::new();
    let mut scene = Scene::default();
    for path in paths {
        let prim = stage.prim(path.clone());
        let view = View(prim.clone());
        let parent = path
            .parent()
            .and_then(|p| states.get(&p))
            .cloned()
            .unwrap_or(State {
                world: Transform::IDENTITY,
                hidden: false,
                purpose: "default".into(),
            });
        validate_ops(&view, time)?;
        let local = matrix(
            view.local_to_parent_transform(time)
                .map_err(|e| e.to_string())?,
        )?;
        let world = if view.resets_xform_stack().map_err(|e| e.to_string())? {
            local
        } else {
            parent.world.then(local)
        };
        let hidden =
            parent.hidden || token(&prim, "visibility", time)?.as_deref() == Some("invisible");
        let purpose = token(&prim, "purpose", time)?.unwrap_or(parent.purpose);
        states.insert(
            path.clone(),
            State {
                world,
                hidden,
                purpose: purpose.clone(),
            },
        );
        if hidden || purpose == "guide" || prototypes.iter().any(|p| under(&path, p)) {
            continue;
        }
        let kind = prim
            .type_name()
            .map_err(|e| e.to_string())?
            .map(|t| t.to_string())
            .unwrap_or_default();
        let world = basis.then(world);
        if kind == "PointInstancer" {
            instancer(&prim, time, world, &mut scene)?;
        } else {
            append_geometry(&prim, &kind, time, world, &mut scene)?;
        }
        geometry_limit(&scene)?;
    }
    let errors = stage.composition_errors();
    if !errors.is_empty() {
        return Err(format!("USD composition failed: {:?}", errors[0]).into());
    }
    scene.validate()?;
    Ok(scene)
}

fn under(path: &sdf::Path, root: &sdf::Path) -> bool {
    path == root
        || path
            .as_str()
            .strip_prefix(root.as_str())
            .is_some_and(|s| s.starts_with('/'))
}
fn geometry_limit(scene: &Scene) -> Result<()> {
    if scene
        .triangles
        .len()
        .saturating_add(scene.points.len())
        .saturating_add(scene.lines.len())
        > MAX_ELEMENTS
    {
        return Err("USD geometry exceeds five million elements".into());
    }
    Ok(())
}
fn append_geometry(
    prim: &Prim,
    kind: &str,
    time: usd::TimeCode,
    world: Transform,
    scene: &mut Scene,
) -> Result<()> {
    match kind {
        "Mesh" => mesh(prim, time, world, scene)?,
        "Points" => points(prim, time, world, scene)?,
        "BasisCurves" => curves(prim, time, world, scene)?,
        "Cube" | "Sphere" | "Cylinder" | "Cone" | "Capsule" | "Plane" => {
            primitive(prim, kind, time, world, scene)?
        }
        "" | "Xform" | "Scope" | "Material" | "Shader" | "NodeGraph" | "GeomSubset" | "Camera" => {}
        other if other.ends_with("Light") || other.ends_with("LightFilter") => {}
        other => {
            return Err(format!("unsupported USD prim type {other} at {}", prim.path()).into());
        }
    }
    geometry_limit(scene)
}

fn value(prim: &Prim, attr: &str, time: usd::TimeCode) -> Result<Option<sdf::Value>> {
    prim.attribute(attr)
        .get_at::<sdf::Value>(time)
        .map_err(|e| e.to_string().into())
}
fn token(prim: &Prim, attr: &str, time: usd::TimeCode) -> Result<Option<String>> {
    value(prim, attr, time)?
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("USD {attr} must be a token").into())
        })
        .transpose()
}
fn number(prim: &Prim, attr: &str, default: f64, time: usd::TimeCode) -> Result<f64> {
    let n = match value(prim, attr, time)? {
        Some(v) => v.cast::<f64>().map_err(|e| e.to_string())?,
        None => default,
    };
    if !n.is_finite() {
        return Err(format!("USD {attr} must be finite").into());
    }
    Ok(n)
}
fn vectors(prim: &Prim, attr: &str, time: usd::TimeCode) -> Result<Vec<Vec3>> {
    let vectors = match value(prim, attr, time)? {
        None => Vec::new(),
        Some(sdf::Value::Vec3fVec(v)) => v
            .into_iter()
            .map(|v| Vec3::new(v.x.into(), v.y.into(), v.z.into()))
            .collect(),
        Some(sdf::Value::Vec3dVec(v)) => v.into_iter().map(|v| Vec3::new(v.x, v.y, v.z)).collect(),
        Some(sdf::Value::Vec3hVec(v)) => v
            .into_iter()
            .map(|v| Vec3::new(v.x.to_f64(), v.y.to_f64(), v.z.to_f64()))
            .collect(),
        Some(_) => return Err(format!("USD {attr} must be a three-component vector array").into()),
    };
    if vectors.len() > MAX_ELEMENTS || vectors.iter().any(|v| !v.is_finite()) {
        return Err(format!("USD {attr} has invalid or excessive values").into());
    }
    Ok(vectors)
}
fn ints(prim: &Prim, attr: &str, time: usd::TimeCode) -> Result<Vec<i32>> {
    match value(prim, attr, time)? {
        None => Ok(Vec::new()),
        Some(sdf::Value::IntVec(v)) if v.len() <= MAX_ELEMENTS => Ok(v),
        _ => Err(format!("USD {attr} must be a bounded int array").into()),
    }
}
fn matrix(m: gf::Matrix4d) -> Result<Transform> {
    if m.0.iter().any(|n| !n.is_finite())
        || [3, 7, 11].iter().any(|&i| m.0[i] != 0.)
        || m.0[15] != 1.
    {
        return Err("USD transform must be finite and affine".into());
    }
    Ok(Transform(std::array::from_fn(|c| {
        std::array::from_fn(|r| m.0[c * 4 + r])
    })))
}
fn validate_ops(view: &View, time: usd::TimeCode) -> Result<()> {
    for op in view
        .xform_op_order()
        .map_err(|e| e.to_string())?
        .unwrap_or_default()
    {
        if op == "!resetXformStack!" {
            continue;
        }
        let name = op.strip_prefix("!invert!").unwrap_or(&op);
        let kind = name
            .strip_prefix("xformOp:")
            .and_then(|n| n.split(':').next())
            .ok_or("invalid USD xform operation")?;
        let raw = value(&view.0, name, time)?
            .ok_or_else(|| format!("missing USD transform attribute {name}"))?;
        let valid = match kind {
            "translate" | "scale" | "rotateXYZ" | "rotateXZY" | "rotateYXZ" | "rotateYZX"
            | "rotateZXY" | "rotateZYX" => matches!(
                raw,
                sdf::Value::Vec3h(_) | sdf::Value::Vec3f(_) | sdf::Value::Vec3d(_)
            ),
            "translateX" | "translateY" | "translateZ" | "scaleX" | "scaleY" | "scaleZ"
            | "rotateX" | "rotateY" | "rotateZ" => matches!(
                raw,
                sdf::Value::Half(_) | sdf::Value::Float(_) | sdf::Value::Double(_)
            ),
            "orient" => matches!(
                raw,
                sdf::Value::Quath(_) | sdf::Value::Quatf(_) | sdf::Value::Quatd(_)
            ),
            "transform" => matches!(raw, sdf::Value::Matrix4d(_)),
            _ => return Err(format!("unsupported USD transform operation {op}").into()),
        };
        if !valid {
            return Err(format!("invalid USD transform value for {op}").into());
        }
    }
    Ok(())
}

fn srgb(v: f64) -> u8 {
    let v = v.clamp(0., 1.);
    ((if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }) * 255.)
        .round() as u8
}
fn rgb(v: Vec3) -> [u8; 3] {
    [srgb(v.x), srgb(v.y), srgb(v.z)]
}

fn material(prim: &Prim, time: usd::TimeCode) -> Result<Option<[u8; 3]>> {
    let stage = prim.stage();
    let mut bound = if let Some(binding) =
        shade::MaterialBindingAPI::get(stage, prim.path().clone()).map_err(|e| e.to_string())?
    {
        binding
            .compute_bound_material("preview")
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    // Older USD exporters author bindings without applying MaterialBindingAPI.
    if bound.is_none() {
        for rel_name in ["material:binding:preview", "material:binding"] {
            let mut path = Some(prim.path().clone());
            while let Some(p) = path {
                let rel = stage.prim(p.clone()).relationship(rel_name);
                let targets = rel.targets().map_err(|e| e.to_string())?;
                if targets.len() > 1 {
                    return Err("USD direct material binding has multiple targets".into());
                }
                if let Some(target) = targets.first() {
                    let strength = stage
                        .attribute(rel.path().clone())
                        .get_metadata::<sdf::Value>("bindMaterialAs")
                        .map_err(|e| e.to_string())?;
                    if bound.is_none()
                        || strength.as_ref().and_then(sdf::Value::as_str)
                            == Some("strongerThanDescendants")
                    {
                        bound = Some(target.clone());
                    }
                }
                path = p.parent();
            }
            if bound.is_some() {
                break;
            }
        }
    }
    let Some(bound) = bound else {
        return Ok(None);
    };
    let Some(mat) = shade::Material::get(stage, bound).map_err(|e| e.to_string())? else {
        return Err("USD material binding targets a non-material prim".into());
    };
    let Some(shader) = mat.compute_surface_source().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if shader.id().map_err(|e| e.to_string())?.as_deref() != Some("UsdPreviewSurface") {
        return Ok(None);
    }
    let color = value(shader.prim(), "inputs:diffuseColor", time)?
        .map(|v| v.cast::<gf::Vec3f>().map_err(|e| e.to_string()))
        .transpose()?;
    if color
        .as_ref()
        .is_some_and(|v| ![v.x, v.y, v.z].iter().all(|n| n.is_finite()))
    {
        return Err("USD shader diffuseColor must be finite".into());
    }
    Ok(color.map(|v| rgb(Vec3::new(v.x.into(), v.y.into(), v.z.into()))))
}

struct Colors {
    values: Vec<Vec3>,
    indices: Vec<i32>,
    interpolation: String,
    fallback: Option<[u8; 3]>,
}
impl Colors {
    fn new(prim: &Prim, time: usd::TimeCode) -> Result<Self> {
        let mut owner = prim.clone();
        if value(prim, "primvars:displayColor", time)?.is_none() {
            let mut ancestor = prim.path().parent();
            while let Some(path) = ancestor {
                let candidate = prim.stage().prim(path.clone());
                if value(&candidate, "primvars:displayColor", time)?.is_some() {
                    let interpolation = candidate
                        .attribute("primvars:displayColor")
                        .get_metadata::<sdf::Value>("interpolation")
                        .map_err(|e| e.to_string())?;
                    if interpolation
                        .as_ref()
                        .and_then(sdf::Value::as_str)
                        .unwrap_or("constant")
                        == "constant"
                    {
                        owner = candidate;
                        break;
                    }
                }
                ancestor = path.parent();
            }
        }
        let values = vectors(&owner, "primvars:displayColor", time)?;
        let indices = ints(&owner, "primvars:displayColor:indices", time)?;
        let interpolation = owner
            .attribute("primvars:displayColor")
            .get_metadata::<sdf::Value>("interpolation")
            .map_err(|e| e.to_string())?
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "constant".into());
        if !matches!(
            interpolation.as_str(),
            "constant" | "uniform" | "vertex" | "varying" | "faceVarying"
        ) {
            return Err(
                format!("unsupported USD displayColor interpolation {interpolation}").into(),
            );
        }
        Ok(Self {
            values,
            indices,
            interpolation,
            fallback: material(prim, time)?,
        })
    }
    fn at(&self, vertex: usize, face: usize, corner: usize) -> Result<Option<Vec3>> {
        if self.values.is_empty() {
            return Ok(None);
        }
        let mut i = match self.interpolation.as_str() {
            "constant" => 0,
            "uniform" => face,
            "faceVarying" => corner,
            _ => vertex,
        };
        if !self.indices.is_empty() {
            i = usize::try_from(
                *self
                    .indices
                    .get(i)
                    .ok_or("USD displayColor index array is too short")?,
            )?;
        }
        Ok(Some(
            *self
                .values
                .get(i)
                .ok_or("USD displayColor array is too short")?,
        ))
    }
    fn face(&self, vertices: &[usize], face: usize, corner: usize) -> Result<Option<[u8; 3]>> {
        if self.fallback.is_some() {
            return Ok(self.fallback);
        }
        let mut sum = Vec3::default();
        for (j, &v) in vertices.iter().enumerate() {
            let Some(c) = self.at(v, face, corner + j)? else {
                return Ok(None);
            };
            sum = sum + c;
        }
        Ok(Some(rgb(sum / (vertices.len() as f64))))
    }
}

fn check_skin(prim: &Prim) -> Result<()> {
    if prim
        .property_names()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|n| n.as_str().starts_with("skel:"))
    {
        return Err(format!("USD skeletal deformation is unsupported at {}", prim.path()).into());
    }
    Ok(())
}

fn mesh(prim: &Prim, time: usd::TimeCode, world: Transform, scene: &mut Scene) -> Result<()> {
    check_skin(prim)?;
    let positions = vectors(prim, "points", time)?;
    let counts = ints(prim, "faceVertexCounts", time)?;
    let indices = ints(prim, "faceVertexIndices", time)?;
    let mut holes = HashSet::new();
    for i in ints(prim, "holeIndices", time)? {
        let i = usize::try_from(i)?;
        if i >= counts.len() {
            return Err("USD hole face index is out of range".into());
        }
        holes.insert(i);
    }
    let colors = Colors::new(prim, time)?;
    let mut subsets = HashMap::new();
    for child in prim.children().map_err(|e| e.to_string())? {
        if child
            .type_name()
            .map_err(|e| e.to_string())?
            .is_none_or(|t| t.as_str() != "GeomSubset")
        {
            continue;
        }
        if token(&child, "elementType", time)?
            .as_deref()
            .unwrap_or("face")
            != "face"
        {
            return Err("USD mesh subset must select faces".into());
        }
        if let Some(color) = material(&child, time)? {
            for i in ints(&child, "indices", time)? {
                let i = usize::try_from(i)?;
                if i >= counts.len() {
                    return Err("USD material subset face index is out of range".into());
                }
                if subsets.insert(i, color).is_some() {
                    return Err("overlapping USD material subsets".into());
                }
            }
        }
    }
    let mut offset = 0_usize;
    for (face, &count) in counts.iter().enumerate() {
        let count = usize::try_from(count)?;
        if !(3..=65_536).contains(&count) {
            return Err("USD polygon needs 3..65536 vertices".into());
        }
        let end = offset.checked_add(count).ok_or("USD face index overflow")?;
        let ids = indices
            .get(offset..end)
            .ok_or("USD face counts exceed index array")?
            .iter()
            .map(|&i| usize::try_from(i))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let polygon = ids
            .iter()
            .map(|&i| {
                positions
                    .get(i)
                    .copied()
                    .ok_or("USD face vertex index is out of range")
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let color = subsets
            .get(&face)
            .copied()
            .or(colors.face(&ids, face, offset)?);
        if !holes.contains(&face) {
            for vertices in triangulate_polygon(&polygon)? {
                scene.triangles.push(Triangle {
                    vertices: vertices.map(|v| world.apply(v)),
                    color,
                });
            }
        }
        offset = end;
    }
    if offset != indices.len() {
        return Err("USD face index array has trailing entries".into());
    }
    Ok(())
}

fn points(prim: &Prim, time: usd::TimeCode, world: Transform, scene: &mut Scene) -> Result<()> {
    check_skin(prim)?;
    let colors = Colors::new(prim, time)?;
    for (i, position) in vectors(prim, "points", time)?.into_iter().enumerate() {
        scene.points.push(Point {
            position: world.apply(position),
            color: colors.fallback.or(colors.at(i, i, i)?.map(rgb)),
        });
    }
    Ok(())
}

fn curves(prim: &Prim, time: usd::TimeCode, world: Transform, scene: &mut Scene) -> Result<()> {
    check_skin(prim)?;
    let positions = vectors(prim, "points", time)?;
    let counts = ints(prim, "curveVertexCounts", time)?;
    let kind = token(prim, "type", time)?.unwrap_or_else(|| "cubic".into());
    let basis = token(prim, "basis", time)?.unwrap_or_else(|| "bezier".into());
    let wrap = token(prim, "wrap", time)?.unwrap_or_else(|| "nonperiodic".into());
    if !matches!(wrap.as_str(), "nonperiodic" | "periodic") {
        return Err(format!("unsupported USD curve wrap {wrap}").into());
    }
    let colors = Colors::new(prim, time)?;
    let mut offset = 0_usize;
    for (curve, &count) in counts.iter().enumerate() {
        let count = usize::try_from(count)?;
        let end = offset
            .checked_add(count)
            .ok_or("USD curve count overflow")?;
        let p = positions
            .get(offset..end)
            .ok_or("USD curve counts exceed point array")?;
        let ids: Vec<_> = (offset..end).collect();
        let color = colors.face(&ids, curve, offset)?;
        let mut add = |a: Vec3, b: Vec3| -> Result<()> {
            if a != b {
                scene.lines.push(Line {
                    vertices: [world.apply(a), world.apply(b)],
                    color,
                });
            }
            geometry_limit(scene)
        };
        if kind == "linear" {
            if p.len() < 2 {
                return Err("USD linear curve needs at least two points".into());
            }
            for pair in p.windows(2) {
                add(pair[0], pair[1])?;
            }
            if wrap == "periodic" {
                add(*p.last().unwrap(), p[0])?;
            }
        } else if kind == "cubic" {
            if p.len() < 4 || !matches!(basis.as_str(), "bezier" | "bspline" | "catmullRom") {
                return Err(
                    "USD cubic curves need a supported basis and at least four points".into(),
                );
            }
            let step = if basis == "bezier" { 3 } else { 1 };
            let periodic = wrap == "periodic";
            if basis == "bezier"
                && if periodic {
                    p.len() % 3 != 0
                } else {
                    (p.len() - 1) % 3 != 0
                }
            {
                return Err("USD Bezier curve has an invalid point count".into());
            }
            let segments = if periodic {
                p.len() / step
            } else {
                (p.len() - 4) / step + 1
            };
            for segment in 0..segments {
                let q: [Vec3; 4] = std::array::from_fn(|i| p[(segment * step + i) % p.len()]);
                let evaluate = |t: f64| {
                    let t2 = t * t;
                    let t3 = t2 * t;
                    let weights = match basis.as_str() {
                        "bezier" => [
                            (1. - t).powi(3),
                            3. * t * (1. - t).powi(2),
                            3. * t2 * (1. - t),
                            t3,
                        ],
                        "bspline" => [
                            (1. - 3. * t + 3. * t2 - t3) / 6.,
                            (4. - 6. * t2 + 3. * t3) / 6.,
                            (1. + 3. * t + 3. * t2 - 3. * t3) / 6.,
                            t3 / 6.,
                        ],
                        _ => [
                            (-t + 2. * t2 - t3) / 2.,
                            (2. - 5. * t2 + 3. * t3) / 2.,
                            (t + 4. * t2 - 3. * t3) / 2.,
                            (-t2 + t3) / 2.,
                        ],
                    };
                    (0..4)
                        .map(|i| q[i] * weights[i])
                        .fold(Vec3::default(), |sum, p| sum + p)
                };
                let mut previous = evaluate(0.);
                for j in 1..=24 {
                    let next = evaluate(j as f64 / 24.);
                    add(previous, next)?;
                    previous = next;
                }
            }
        } else {
            return Err(format!("unsupported USD curve type {kind}").into());
        }
        offset = end;
    }
    if offset != positions.len() {
        return Err("USD curve points have trailing entries".into());
    }
    Ok(())
}

fn long_ints(prim: &Prim, attr: &str, time: usd::TimeCode) -> Result<Vec<i64>> {
    match value(prim, attr, time)? {
        None => Ok(Vec::new()),
        Some(sdf::Value::Int64Vec(v)) if v.len() <= MAX_ELEMENTS => Ok(v),
        _ => Err(format!("USD {attr} must be a bounded int64 array").into()),
    }
}

fn prototype_scene(stage: &usd::Stage, root: &sdf::Path, time: usd::TimeCode) -> Result<Scene> {
    let mut paths = Vec::new();
    stage
        .traverse(usd::PrimPredicate::DEFAULT_PROXIES, |p| {
            if under(p, root) && paths.len() < MAX_PRIMS {
                paths.push(p.clone());
            }
        })
        .map_err(|e| e.to_string())?;
    let mut states = HashMap::<sdf::Path, State>::new();
    let mut scene = Scene::default();
    for path in paths {
        let prim = stage.prim(path.clone());
        let view = View(prim.clone());
        let parent = path
            .parent()
            .and_then(|p| states.get(&p))
            .cloned()
            .unwrap_or(State {
                world: Transform::IDENTITY,
                hidden: false,
                purpose: "default".into(),
            });
        validate_ops(&view, time)?;
        let local = matrix(
            view.local_to_parent_transform(time)
                .map_err(|e| e.to_string())?,
        )?;
        let world = if view.resets_xform_stack().map_err(|e| e.to_string())? {
            local
        } else {
            parent.world.then(local)
        };
        let hidden =
            parent.hidden || token(&prim, "visibility", time)?.as_deref() == Some("invisible");
        let purpose = token(&prim, "purpose", time)?.unwrap_or(parent.purpose);
        states.insert(
            path,
            State {
                world,
                hidden,
                purpose: purpose.clone(),
            },
        );
        if hidden || purpose == "guide" {
            continue;
        }
        let kind = prim
            .type_name()
            .map_err(|e| e.to_string())?
            .map(|t| t.to_string())
            .unwrap_or_default();
        append_geometry(&prim, &kind, time, world, &mut scene)?;
    }
    if scene.triangles.is_empty() && scene.points.is_empty() && scene.lines.is_empty() {
        return Err(format!("USD prototype {root} contains no supported geometry").into());
    }
    Ok(scene)
}

fn instancer(prim: &Prim, time: usd::TimeCode, world: Transform, scene: &mut Scene) -> Result<()> {
    let targets = prim
        .relationship("prototypes")
        .targets()
        .map_err(|e| e.to_string())?;
    if targets.is_empty() || targets.len() > 1024 {
        return Err("USD point instancer needs 1..1024 prototypes".into());
    }
    if targets.iter().any(|p| under(prim.path(), p)) {
        return Err("cyclic USD point instancer prototypes".into());
    }
    let indices = ints(prim, "protoIndices", time)?;
    let positions = vectors(prim, "positions", time)?;
    let scales = vectors(prim, "scales", time)?;
    let orientations = value(prim, "orientationsf", time)?.or(value(prim, "orientations", time)?);
    let orientations = match orientations {
        None => Vec::new(),
        Some(sdf::Value::QuatfVec(v)) => v
            .into_iter()
            .map(|v| sdf::Value::Quatf(v).cast::<[f64; 4]>())
            .collect::<std::result::Result<Vec<_>, _>>()?,
        Some(sdf::Value::QuathVec(v)) => v
            .into_iter()
            .map(|v| sdf::Value::Quath(v).cast::<[f64; 4]>())
            .collect::<std::result::Result<Vec<_>, _>>()?,
        _ => return Err("USD point instancer orientations must be quaternion arrays".into()),
    };
    let ids = long_ints(prim, "ids", time)?;
    let mut hidden: HashSet<_> = long_ints(prim, "invisibleIds", time)?.into_iter().collect();
    if let Some(v) = prim
        .stage()
        .attribute(prim.path().clone())
        .get_metadata::<sdf::Value>("inactiveIds")
        .map_err(|e| e.to_string())?
    {
        match v {
            sdf::Value::Int64ListOp(op) if op.explicit => hidden.extend(op.explicit_items),
            sdf::Value::Int64Vec(v) => hidden.extend(v),
            _ => {
                return Err(
                    "USD point instancer inactiveIds requires an explicit int64 list".into(),
                );
            }
        }
    }
    let n = indices.len();
    if n != positions.len()
        || (!scales.is_empty() && scales.len() != n)
        || (!orientations.is_empty() && orientations.len() != n)
        || (!ids.is_empty() && ids.len() != n)
    {
        return Err("USD point instancer arrays have different lengths".into());
    }
    let colors = Colors::new(prim, time)?;
    let mut cache = HashMap::new();
    for (i, &index) in indices.iter().enumerate() {
        let prototype = usize::try_from(index)?;
        let target = targets
            .get(prototype)
            .ok_or("USD point instancer prototype index out of range")?;
        if hidden.contains(&ids.get(i).copied().unwrap_or(i as i64)) {
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(prototype) {
            entry.insert(prototype_scene(prim.stage(), target, time)?);
        }
        let source = &cache[&prototype];
        let total = scene.triangles.len()
            + scene.points.len()
            + scene.lines.len()
            + source.triangles.len()
            + source.points.len()
            + source.lines.len();
        if total > MAX_ELEMENTS {
            return Err("USD instancing exceeds five million geometry elements".into());
        }
        let p = positions[i];
        let s = scales.get(i).copied().unwrap_or(Vec3::new(1., 1., 1.));
        let q = orientations.get(i).copied().unwrap_or([1., 0., 0., 0.]);
        if q.iter().any(|v| !v.is_finite()) || q.iter().map(|v| v * v).sum::<f64>() < 1e-24 {
            return Err("invalid USD point instancer quaternion".into());
        }
        let translate = Transform([
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [p.x, p.y, p.z, 1.],
        ]);
        let scale = Transform([
            [s.x, 0., 0., 0.],
            [0., s.y, 0., 0.],
            [0., 0., s.z, 0.],
            [0., 0., 0., 1.],
        ]);
        let transform = world
            .then(translate)
            .then(matrix(gf::Matrix4d::from_quat(q))?)
            .then(scale);
        let color = colors.at(i, i, i)?.map(rgb).or(colors.fallback);
        scene
            .triangles
            .extend(source.triangles.iter().map(|t| Triangle {
                vertices: t.vertices.map(|v| transform.apply(v)),
                color: color.or(t.color),
            }));
        scene.points.extend(source.points.iter().map(|p| Point {
            position: transform.apply(p.position),
            color: color.or(p.color),
        }));
        scene.lines.extend(source.lines.iter().map(|l| Line {
            vertices: l.vertices.map(|v| transform.apply(v)),
            color: color.or(l.color),
        }));
    }
    Ok(())
}

fn primitive(
    prim: &Prim,
    kind: &str,
    time: usd::TimeCode,
    world: Transform,
    scene: &mut Scene,
) -> Result<()> {
    let colors = Colors::new(prim, time)?;
    let color = colors.fallback.or(colors.at(0, 0, 0)?.map(rgb));
    let axis = token(prim, "axis", time)?.unwrap_or_else(|| "Z".into());
    let orient = |v: Vec3| -> Result<Vec3> {
        Ok(world.apply(match axis.as_str() {
            "X" => Vec3::new(v.z, v.x, v.y),
            "Y" => Vec3::new(v.y, v.z, v.x),
            "Z" => v,
            _ => return Err("USD primitive axis must be X, Y, or Z".into()),
        }))
    };
    let mut add = |v: [Vec3; 3]| -> Result<()> {
        scene.triangles.push(Triangle {
            vertices: [orient(v[0])?, orient(v[1])?, orient(v[2])?],
            color,
        });
        Ok(())
    };
    if kind == "Cube" {
        let h = number(prim, "size", 2., time)? / 2.;
        if h <= 0. {
            return Err("USD cube size must be positive".into());
        }
        let p: [Vec3; 8] = std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 == 0 { -h } else { h },
                if i & 2 == 0 { -h } else { h },
                if i & 4 == 0 { -h } else { h },
            )
        });
        for [a, b, c, d] in [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ] {
            add([p[a], p[b], p[c]])?;
            add([p[a], p[c], p[d]])?;
        }
    } else if kind == "Plane" {
        let w = number(prim, "width", 2., time)? / 2.;
        let h = number(prim, "length", 2., time)? / 2.;
        if w <= 0. || h <= 0. {
            return Err("USD plane dimensions must be positive".into());
        }
        let p = [
            Vec3::new(-w, -h, 0.),
            Vec3::new(w, -h, 0.),
            Vec3::new(w, h, 0.),
            Vec3::new(-w, h, 0.),
        ];
        add([p[0], p[1], p[2]])?;
        add([p[0], p[2], p[3]])?;
    } else {
        let radius = number(prim, "radius", 1., time)?;
        let height = number(prim, "height", 2., time)?;
        if radius <= 0. || height < 0. {
            return Err("USD primitive dimensions must be positive".into());
        }
        let mut rings = Vec::new();
        if kind == "Sphere" || kind == "Capsule" {
            for j in 0..=24 {
                let a = std::f64::consts::PI * (j as f64) / 24.;
                let offset = if kind == "Capsule" {
                    if j <= 12 { height / 2. } else { -height / 2. }
                } else {
                    0.
                };
                rings.push((radius * a.sin(), radius * a.cos() + offset));
            }
            if kind == "Capsule" {
                rings.insert(13, (radius, -height / 2.));
            }
        } else {
            rings.extend([
                (0., -height / 2.),
                (radius, -height / 2.),
                (if kind == "Cone" { 0. } else { radius }, height / 2.),
                (0., height / 2.),
            ]);
        }
        let p = |ring: (f64, f64), i: usize| {
            let a = std::f64::consts::TAU * (i as f64) / 32.;
            Vec3::new(ring.0 * a.cos(), ring.0 * a.sin(), ring.1)
        };
        for pair in rings.windows(2) {
            for i in 0..32 {
                let a = p(pair[0], i);
                let b = p(pair[0], i + 1);
                let c = p(pair[1], i + 1);
                let d = p(pair[1], i);
                if pair[0].0 > 1e-15 {
                    add([a, b, c])?;
                }
                if pair[1].0 > 1e-15 {
                    add([a, c, d])?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "catwalk-usd-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, text).unwrap();
            p
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    const TRIANGLE: &str = r#"def Mesh "Triangle" {
        point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0,1,2]
        uniform token subdivisionScheme = "none"
    }"#;

    #[test]
    fn hierarchy_rotation_scale_earliest_sample_and_reset_are_applied() {
        let temp = Temp::new();
        let input = temp.write(
            "hierarchy.usda",
            &format!(
                r#"#usda 1.0
(upAxis = "Z")
def Xform "Root" {{
    double3 xformOp:translate = (10,0,0)
    float xformOp:rotateZ = 90
    token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateZ"]
    def Xform "Child" {{
        double3 xformOp:translate.timeSamples = {{2: (2,0,0), 4: (20,0,0)}}
        float3 xformOp:scale = (2,3,1)
        token[] xformOpOrder = ["xformOp:translate", "xformOp:scale"]
        {TRIANGLE}
    }}
    def Cube "Reset" {{
        double size = 2
        double3 xformOp:translate = (0,0,5)
        token[] xformOpOrder = ["!resetXformStack!", "xformOp:translate"]
    }}
}}"#
            ),
        );
        let s = load(&input).unwrap();
        assert_eq!(s.triangles.len(), 13);
        let [a, b, c] = s.triangles[0].vertices;
        assert!((a - Vec3::new(10., 2., 0.)).length() < 1e-10);
        assert!((b - Vec3::new(10., 4., 0.)).length() < 1e-10);
        assert!((c - Vec3::new(7., 2., 0.)).length() < 1e-10);
        for t in &s.triangles[1..] {
            for v in t.vertices {
                assert!(v.x.abs() <= 1. && v.y.abs() <= 1. && (4. ..=6.).contains(&v.z));
            }
        }
    }

    #[test]
    fn y_up_visibility_and_point_colors_are_preserved() {
        let temp = Temp::new();
        let input = temp.write(
            "points.usda",
            r#"#usda 1.0
def Points "Visible" {
 point3f[] points = [(1,2,3),(4,5,6)]
 color3f[] primvars:displayColor = [(1,0,0),(0,1,0)] (interpolation = "vertex")
}
def Xform "Hidden" {
 token visibility = "invisible"
 def Cube "Child" {}
}
def Cube "Guide" { token purpose = "guide" }
"#,
        );
        let s = load(&input).unwrap();
        assert!(s.triangles.is_empty());
        assert_eq!(s.points.len(), 2);
        assert_eq!(s.points[0].position, Vec3::new(1., -3., 2.));
        assert_eq!(s.points[0].color, Some([255, 0, 0]));
        assert_eq!(s.points[1].color, Some([0, 255, 0]));
    }

    #[test]
    fn indexed_uniform_colors_concave_polygons_and_holes() {
        let temp = Temp::new();
        let input = temp.write(
            "polygon.usda",
            r#"#usda 1.0
(upAxis = "Z")
def Mesh "Shape" {
 point3f[] points = [(0,0,0),(2,0,0),(2,2,0),(1,1,0),(0,2,0),(8,0,0),(9,0,0),(8,1,0)]
 int[] faceVertexCounts = [5,3]
 int[] faceVertexIndices = [0,1,2,3,4,5,6,7]
 int[] holeIndices = [1]
 color3f[] primvars:displayColor = [(0,1,0),(1,0,0)] (interpolation = "uniform")
 int[] primvars:displayColor:indices = [1,0]
}"#,
        );
        let s = load(&input).unwrap();
        assert_eq!(s.triangles.len(), 3);
        let area: f64 = s
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        assert!((area - 3.).abs() < 1e-10);
        assert!(s.triangles.iter().all(|t| t.color == Some([255, 0, 0])));
    }

    #[test]
    fn preview_materials_and_face_subsets_override_display_color() {
        let temp = Temp::new();
        let input = temp.write(
            "materials.usda",
            r#"#usda 1.0
(upAxis = "Z")
def Material "Green" {
 token outputs:surface.connect = </Green/Surface.outputs:surface>
 def Shader "Surface" {
  uniform token info:id = "UsdPreviewSurface"
  color3f inputs:diffuseColor = (0,1,0)
  token outputs:surface
 }
}
def Material "Blue" {
 token outputs:surface.connect = </Blue/Surface.outputs:surface>
 def Shader "Surface" {
  uniform token info:id = "UsdPreviewSurface"
  color3f inputs:diffuseColor = (0,0,1)
  token outputs:surface
 }
}
def Mesh "Square" (prepend apiSchemas = ["MaterialBindingAPI"]) {
 point3f[] points = [(0,0,0),(1,0,0),(1,1,0),(0,1,0)]
 int[] faceVertexCounts = [3,3]
 int[] faceVertexIndices = [0,1,2,0,2,3]
 color3f[] primvars:displayColor = [(1,0,0)]
 rel material:binding = </Green>
 def GeomSubset "BlueFace" {
  uniform token elementType = "face"
  uniform token familyName = "materialBind"
  int[] indices = [1]
  rel material:binding = </Blue>
 }
}"#,
        );
        let s = load(&input).unwrap();
        assert_eq!(s.triangles.len(), 2);
        assert_eq!(s.triangles[0].color, Some([0, 255, 0]));
        assert_eq!(s.triangles[1].color, Some([0, 0, 255]));
    }

    #[test]
    fn local_references_native_instances_and_variants_compose() {
        let temp = Temp::new();
        temp.write(
            "part.usda",
            &format!("#usda 1.0\n(defaultPrim = \"Part\")\ndef Xform \"Part\" {{\n{TRIANGLE}\n}}"),
        );
        let input = temp.write(
            "assembly.usda",
            r#"#usda 1.0
(upAxis = "Z")
def Xform "One" (prepend references = @part.usda@; instanceable = true) {
 double3 xformOp:translate = (2,0,0)
 token[] xformOpOrder = ["xformOp:translate"]
}
def Xform "Two" (prepend references = @part.usda@; instanceable = true) {
 double3 xformOp:translate = (-2,0,0)
 token[] xformOpOrder = ["xformOp:translate"]
}
def Xform "Variant" (
 variants = { string shape = "cube" }
 prepend variantSets = "shape"
) {
 variantSet "shape" = {
  "cube" { def Cube "Chosen" {} }
  "sphere" { def Sphere "Other" {} }
 }
}"#,
        );
        let s = load(&input).unwrap();
        assert_eq!(s.triangles.len(), 14);
        assert_eq!(s.triangles[0].vertices[0], Vec3::new(2., 0., 0.));
        assert_eq!(s.triangles[1].vertices[0], Vec3::new(-2., 0., 0.));
    }

    #[test]
    fn binary_usd_generic_usd_and_package_references_render_identically() {
        let temp = Temp::new();
        let ascii = temp.write(
            "triangle.usda",
            &format!("#usda 1.0\n(upAxis = \"Z\"; defaultPrim = \"Triangle\")\n{TRIANGLE}"),
        );
        let stage = usd::Stage::open(ascii.to_str().unwrap()).unwrap();
        let binary = temp.0.join("triangle.usdc");
        stage.root_layer().export(binary.to_str().unwrap()).unwrap();
        assert!(std::fs::read(&binary).unwrap().starts_with(b"PXR-USDC"));
        let generic = temp.0.join("triangle.usd");
        std::fs::copy(&binary, &generic).unwrap();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("root.usda", opts).unwrap();
        zip.write_all(b"#usda 1.0\n(upAxis = \"Z\"; defaultPrim = \"Part\")\ndef Mesh \"Part\" (prepend references = @parts/triangle.usdc@) {}\n").unwrap();
        zip.start_file("parts/triangle.usdc", opts).unwrap();
        zip.write_all(&std::fs::read(&binary).unwrap()).unwrap();
        let package = temp.0.join("triangle.usdz");
        std::fs::write(&package, zip.finish().unwrap().into_inner()).unwrap();
        let expected = load(&ascii).unwrap();
        let assembly=temp.write("assembly.usda","#usda 1.0\n(upAxis = \"Z\")\ndef Mesh \"Packaged\" (prepend references = @triangle.usdz@) {}\n");
        for input in [binary, generic, package, assembly] {
            let s = load(&input).unwrap();
            assert_eq!(s.triangles.len(), 1);
            assert_eq!(s.triangles[0].vertices, expected.triangles[0].vertices);
        }
    }

    #[test]
    fn malformed_topology_unsupported_geometry_and_missing_references_fail() {
        let temp = Temp::new();
        for (i,text) in [
            "#usda 1.0\ndef Mesh \"Bad\" { point3f[] points = [(0,0,0)]\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0,1,2] }",
            "#usda 1.0\ndef NurbsPatch \"Unsupported\" {}",
            "#usda 1.0\ndef Cube \"Missing\" (prepend references = @missing.usda@) {}",
            "#usda 1.0\ndef Cube \"BadTransform\" { token[] xformOpOrder = [\"xformOp:shear\"]\n double xformOp:shear = 1 }",
            "#usda 1.0\ndef Cube \"Skinned\" { double size = -2 }",
        ].into_iter().enumerate() {let input=temp.write(&format!("bad{i}.usda"),text);assert!(load(&input).is_err(),"unexpected success: {text}");}
    }

    #[test]
    fn resolver_confines_external_and_packaged_dependencies() {
        let temp = Temp::new();
        let outside = temp.0.with_extension("usda");
        std::fs::write(&outside, format!("#usda 1.0\n{TRIANGLE}")).unwrap();
        let input = temp.write(
            "escape.usda",
            &format!(
                "#usda 1.0\ndef Cube \"Part\" (prepend references = @{}@) {{}}",
                outside.display()
            ),
        );
        assert!(load(&input).is_err());
        std::fs::remove_file(outside).unwrap();
        for name in [
            "../escape.usda",
            "/absolute.usda",
            "dir\\escape.usda",
            "nested.usdz[root.usda]",
            "https://model.usda",
        ] {
            assert!(!safe_member(name));
        }
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("../root.usda", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(format!("#usda 1.0\n{TRIANGLE}").as_bytes())
            .unwrap();
        let package = temp.0.join("bad.usdz");
        std::fs::write(&package, zip.finish().unwrap().into_inner()).unwrap();
        assert!(load(&package).is_err());
    }

    #[test]
    fn analytic_primitives_have_expected_bounds_and_positive_area() {
        let temp = Temp::new();
        for kind in ["Cube", "Sphere", "Cylinder", "Cone", "Capsule", "Plane"] {
            let input = temp.write(
                &format!("{kind}.usda"),
                &format!("#usda 1.0\n(upAxis = \"Z\")\ndef {kind} \"Shape\" {{}}"),
            );
            let s = load(&input).unwrap();
            assert!(!s.triangles.is_empty());
            for t in &s.triangles {
                assert!(
                    (t.vertices[1] - t.vertices[0])
                        .cross(t.vertices[2] - t.vertices[0])
                        .length()
                        > 1e-15,
                    "degenerate {kind} face"
                );
            }
            let max_z = s.positions().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
            let expected = match kind {
                "Capsule" => 2.,
                "Plane" => 0.,
                _ => 1.,
            };
            assert!((max_z - expected).abs() < 1e-10);
        }
    }

    #[test]
    fn point_instancers_apply_instance_and_prototype_transforms_and_masks() {
        let temp = Temp::new();
        let input = temp.write(
            "instances.usda",
            &format!(
                r#"#usda 1.0
(upAxis = "Z")
def PointInstancer "Cloud" {{
 rel prototypes = </Cloud/Prototypes/Shape>
 int[] protoIndices = [0,0,0]
 point3f[] positions = [(10,0,0),(50,0,0),(100,0,0)]
 float3[] scales = [(2,3,1),(1,1,1),(1,1,1)]
 quatf[] orientationsf = [(0.70710677,0,0,0.70710677),(1,0,0,0),(1,0,0,0)]
 int64[] ids = [101,202,303]
 int64[] invisibleIds = [202,303]
 color3f[] primvars:displayColor = [(1,0,0),(0,1,0),(0,0,1)] (interpolation = "vertex")
 def Scope "Prototypes" {{
  def Xform "Shape" {{
   double3 xformOp:translate = (1,0,0)
   token[] xformOpOrder = ["xformOp:translate"]
   {TRIANGLE}
  }}
 }}
}}"#
            ),
        );
        let s = load(&input).unwrap();
        assert_eq!(s.triangles.len(), 1);
        assert_eq!(s.triangles[0].color, Some([255, 0, 0]));
        let [a, b, c] = s.triangles[0].vertices;
        assert!((a - Vec3::new(10., 2., 0.)).length() < 1e-6);
        assert!((b - Vec3::new(10., 4., 0.)).length() < 1e-6);
        assert!((c - Vec3::new(7., 2., 0.)).length() < 1e-6);
    }

    #[test]
    fn linear_and_bezier_curves_use_inherited_color_and_exact_endpoints() {
        let temp = Temp::new();
        let input = temp.write(
            "curves.usda",
            r#"#usda 1.0
(upAxis = "Z")
def Xform "Root" {
 color3f[] primvars:displayColor = [(0,1,0)]
 def BasisCurves "Line" {
  uniform token type = "linear"
  uniform token wrap = "periodic"
  int[] curveVertexCounts = [3]
  point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]
 }
 def BasisCurves "Arc" {
  uniform token type = "cubic"
  uniform token basis = "bezier"
  int[] curveVertexCounts = [4]
  point3f[] points = [(2,0,0),(2,1,0),(3,1,0),(3,0,0)]
 }
}"#,
        );
        let s = load(&input).unwrap();
        assert_eq!(s.lines.len(), 27);
        assert!(s.lines.iter().all(|l| l.color == Some([0, 255, 0])));
        assert_eq!(s.lines[3].vertices[0], Vec3::new(2., 0., 0.));
        assert_eq!(s.lines.last().unwrap().vertices[1], Vec3::new(3., 0., 0.));
        let peak = s
            .lines
            .iter()
            .flat_map(|l| l.vertices)
            .filter(|v| v.x >= 2.)
            .map(|v| v.y)
            .fold(0., f64::max);
        assert!((peak - 0.75).abs() < 1e-10);
    }

    #[test]
    fn old_binary_layers_return_compatibility_errors_without_unwinding() {
        let temp = Temp::new();
        let mut bytes = b"PXR-USDC".to_vec();
        bytes.extend([0, 3, 0, 0, 0, 0, 0, 0]);
        let input = temp.0.join("old.usdc");
        std::fs::write(&input, bytes).unwrap();
        let error = load(&input).unwrap_err().to_string();
        assert!(error.contains("0.4.0"));
    }
}
