//! STEP Part 21 analytic, faceted and AP242 geometry with shared assembly placements.
use super::{
    step_budget::Budget, step_faceted, step_parameters, step_tessellation, transform::Transform,
};
use crate::Result;
use crate::scene::{Scene, Triangle, Vec3};
use ruststep::ast::{EntityInstance, Name, Parameter, Record};
use ruststep::tables::EntityTable;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use truck_meshalgo::prelude::*;
use truck_stepio::r#in::{
    FaceAny, ShellHolder, SphericalSurfaceHolder, Table, ToroidalSurfaceHolder, VertexPointHolder,
    ruststep,
};

/// Maximum parsed schema entity count.
const MAX_ENTITIES: usize = 250_000;
/// Maximum CAD face count accepted for a preview body.
const MAX_FACES: usize = 100_000;
/// Maximum source or expanded preview triangle count.
const MAX_TRIANGLES: usize = 5_000_000;
/// Maximum parameter nesting or entity dependency depth.
const MAX_REFERENCE_DEPTH: usize = 64;

/// Read STEP geometry and convert Rust-kernel panics into importer errors.
///
/// # Errors
///
/// File/UTF-8 parsing, unsupported topology, placements or tessellation fail; kernel panics are caught.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let text = std::str::from_utf8(&bytes)?.trim_start_matches('\u{feff}');
    // Truck exposes infallible geometry constructors; convert their panics to importer errors.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse(text)))
        .map_err(|_| "STEP geometry could not be tessellated by the Rust CAD kernel")?
}

/// Expose an entity identifier and its simple or complex record list.
fn records(entity: &EntityInstance) -> (u64, &[Record]) {
    match entity {
        EntityInstance::Simple { id, record } => (*id, std::slice::from_ref(record)),
        EntityInstance::Complex { id, subsuper } => (*id, &subsuper.0),
    }
}

/// Cap nesting before ruststep recursively parses parameter lists.
fn check_nesting(text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let (mut index, mut depth) = (0usize, 0usize);
    while index < bytes.len() {
        match bytes[index] {
            b'\'' => {
                index += 1;
                loop {
                    let Some(&byte) = bytes.get(index) else {
                        return Err("unterminated STEP string".into());
                    };
                    index += 1;
                    if byte == b'\'' {
                        if bytes.get(index) == Some(&b'\'') {
                            index += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while bytes.get(index..index + 2) != Some(b"*/") {
                    if index >= bytes.len() {
                        return Err("unterminated STEP comment".into());
                    }
                    index += 1;
                }
                index += 2;
            }
            b'(' => {
                depth += 1;
                if depth > MAX_REFERENCE_DEPTH {
                    return Err("STEP parameter nesting exceeds importer limit".into());
                }
                index += 1;
            }
            b')' => {
                depth = depth.checked_sub(1).ok_or("unbalanced STEP parentheses")?;
                index += 1;
            }
            _ => index += 1,
        }
    }
    if depth != 0 {
        return Err("unbalanced STEP parentheses".into());
    }
    Ok(())
}

/// Collect entity references recursively while rejecting non-finite numbers.
///
/// # Errors
///
/// A reference uses an unsupported name kind or a numeric parameter is non-finite.
fn collect_references(parameter: &Parameter, references: &mut Vec<u64>) -> Result<()> {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => references.push(*id),
        Parameter::Ref(_) => return Err("STEP named/value references are not implemented".into()),
        Parameter::Real(value) if !value.is_finite() => {
            return Err("STEP numeric value is not finite".into());
        }
        Parameter::List(values) => {
            for value in values {
                collect_references(value, references)?;
            }
        }
        Parameter::Typed { parameter, .. } => collect_references(parameter, references)?,
        _ => {}
    }
    Ok(())
}

/// Reject dependency cycles before Truck's recursive owned-entity conversion.
fn check_graph(
    id: u64,
    graph: &HashMap<u64, Vec<u64>>,
    visiting: &mut HashSet<u64>,
    done: &mut HashMap<u64, usize>,
    depth: usize,
) -> Result<usize> {
    if let Some(&remaining_depth) = done.get(&id) {
        if depth + remaining_depth > MAX_REFERENCE_DEPTH {
            return Err("STEP geometry reference depth exceeds importer limit".into());
        }
        return Ok(remaining_depth);
    }
    if depth > MAX_REFERENCE_DEPTH {
        return Err("STEP geometry reference depth exceeds importer limit".into());
    }
    if !visiting.insert(id) {
        return Err("STEP geometry contains cyclic entity references".into());
    }
    let references = graph
        .get(&id)
        .ok_or_else(|| format!("STEP references missing entity #{id}"))?;
    let mut remaining_depth = 0usize;
    for &next in references {
        remaining_depth =
            remaining_depth.max(check_graph(next, graph, visiting, done, depth + 1)? + 1);
    }
    visiting.remove(&id);
    done.insert(id, remaining_depth);
    Ok(remaining_depth)
}

/// Require a STEP parameter-list value.
///
/// # Errors
///
/// The parameter is not a list.
fn list(parameter: &Parameter) -> Result<&[Parameter]> {
    match parameter {
        Parameter::List(values) => Ok(values),
        _ => Err("invalid STEP entity parameters".into()),
    }
}

/// Require a STEP entity reference.
///
/// # Errors
///
/// The parameter is not a numeric entity reference.
fn reference(parameter: &Parameter) -> Result<u64> {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => Ok(*id),
        _ => Err("STEP entity must reference a geometry record".into()),
    }
}

/// Resolve an identifier to a supported simple entity record.
///
/// # Errors
///
/// The entity is missing or is a complex record.
fn simple_record<'a>(entities: &HashMap<u64, &'a EntityInstance>, id: u64) -> Result<&'a Record> {
    match entities.get(&id) {
        Some(EntityInstance::Simple { record, .. }) => Ok(record),
        _ => Err(format!("STEP entity #{id} is not a supported simple record").into()),
    }
}

/// Read an analytic radius larger than the kernel's numerical tolerance.
///
/// # Errors
///
/// The parameter is nonnumeric, non-finite or too small for the kernel.
#[allow(
    clippy::cast_precision_loss,
    reason = "STEP integer dimensions intentionally use the geometry kernel's f64 representation, matching real parameters."
)]
fn positive_radius(parameter: &Parameter) -> Result<f64> {
    let value = match parameter {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        _ => return Err("invalid STEP analytic radius".into()),
    };
    if !value.is_finite() || value <= TOLERANCE {
        return Err("STEP analytic radius must be finite and exceed kernel tolerance".into());
    }
    Ok(value)
}

/// Extract a finite included or excluded surface-parameter bound.
///
/// # Errors
///
/// The parameter is unbounded or non-finite.
fn finite_parameter_bound(value: std::ops::Bound<f64>) -> Result<f64> {
    match value {
        std::ops::Bound::Included(value) | std::ops::Bound::Excluded(value)
            if value.is_finite() =>
        {
            Ok(value)
        }
        _ => Err("STEP geometry has unbounded or non-finite parameters".into()),
    }
}

/// Tessellate a closed analytic surface represented by a vertex loop.
///
/// # Errors
///
/// The vertex is off-surface, parameters are unbounded or geometry/triangle limits fail.
#[allow(
    clippy::cast_precision_loss,
    reason = "Validated dimensions and bounded subdivision indices intentionally retain existing f64 evaluation and rounding."
)]
#[allow(
    clippy::manual_midpoint,
    reason = "Finite analytic surface bounds are limited; preserve the established sampling and rounding order."
)]
fn append_closed_surface<S: ParametricSurface3D + SearchParameter<D2, Point = Point3>>(
    surface: &S,
    invert: bool,
    vertex: Point3,
    scene: &mut Scene,
    budget: &mut Budget,
) -> Result<()> {
    if surface.search_parameter(vertex, None, 100).is_none() {
        return Err("STEP vertex loop point does not lie on its analytic surface".into());
    }
    let ((u0, u1), (v0, v1)) = surface.parameter_range();
    let (u0, u1, v0, v1) = (
        finite_parameter_bound(u0)?,
        finite_parameter_bound(u1)?,
        finite_parameter_bound(v0)?,
        finite_parameter_bound(v1)?,
    );
    for i in 0..64 {
        for j in 0..64 {
            let u = |i: usize| u0 + (u1 - u0) * i as f64 / 64.0;
            let v = |j: usize| v0 + (v1 - v0) * j as f64 / 64.0;
            let points = [
                surface.subs(u(i), v(j)),
                surface.subs(u(i + 1), v(j)),
                surface.subs(u(i + 1), v(j + 1)),
                surface.subs(u(i), v(j + 1)),
            ]
            .map(|point| Vec3::new(point.x, point.y, point.z));
            if points.iter().any(|p| !p.is_finite()) {
                return Err("STEP analytic surface produced non-finite points".into());
            }
            let normal = surface.normal((u(i) + u(i + 1)) * 0.5, (v(j) + v(j + 1)) * 0.5);
            let normal = Vec3::new(normal.x, normal.y, normal.z);
            for mut vertices in [
                [points[0], points[1], points[2]],
                [points[0], points[2], points[3]],
            ] {
                let cross = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]);
                if cross.length() == 0.0 {
                    continue;
                }
                if (cross.dot(normal) < 0.0) != invert {
                    vertices.swap(1, 2);
                }
                if scene.triangles.len() == MAX_TRIANGLES {
                    return Err("STEP tessellation exceeds triangle limit".into());
                }
                budget.triangles(1, false)?;
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
        }
    }
    Ok(())
}

/// A vertex loop on a full sphere/torus marks a closed surface, not a polygonal trim.
/// Truck 0.3 only accepts edge loops; evaluate these analytic surfaces directly.
fn closed_analytic_shell(
    id: u64,
    entities: &HashMap<u64, &EntityInstance>,
    table: &Table,
    budget: &mut Budget,
) -> Result<Option<Scene>> {
    let shell = simple_record(entities, id)?;
    let shell_params = list(&shell.parameter)?;
    let faces = list(shell_params.get(1).ok_or("missing STEP shell faces")?)?;
    if faces.is_empty() {
        return Ok(None);
    }
    let mut analytic_faces = Vec::new();
    for face in faces {
        let face_id = reference(face)?;
        let face_record = simple_record(entities, face_id)?;
        if !matches!(face_record.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE") {
            return Ok(None);
        }
        let face_params = list(&face_record.parameter)?;
        let bounds = list(face_params.get(1).ok_or("missing STEP face bounds")?)?;
        if bounds.len() != 1 {
            return Ok(None);
        }
        let bound_record = simple_record(entities, reference(&bounds[0])?)?;
        let bound_params = list(&bound_record.parameter)?;
        let loop_record = simple_record(
            entities,
            reference(bound_params.get(1).ok_or("missing STEP face loop")?)?,
        )?;
        if loop_record.name.as_str() != "VERTEX_LOOP" {
            return Ok(None);
        }
        let loop_params = list(&loop_record.parameter)?;
        let vertex_id = reference(loop_params.get(1).ok_or("missing STEP vertex loop point")?)?;
        let vertex = <Table as EntityTable<VertexPointHolder>>::get_owned(table, vertex_id)
            .map_err(|error| format!("cannot resolve STEP vertex loop: {error}"))?;
        let vertex = Point3::from(&vertex.vertex_geometry);
        let surface_id = reference(face_params.get(2).ok_or("missing STEP face surface")?)?;
        let face = table
            .face_surface
            .get(&face_id)
            .ok_or("missing STEP analytic face")?;
        analytic_faces.push((surface_id, !face.same_sense, vertex));
    }
    let mut scene = Scene::default();
    for (surface_id, invert, vertex) in analytic_faces {
        let surface_record = simple_record(entities, surface_id)?;
        let surface_params = list(&surface_record.parameter)?;
        match surface_record.name.as_str() {
            "SPHERICAL_SURFACE" => {
                positive_radius(surface_params.get(2).ok_or("missing STEP sphere radius")?)?;
                let source =
                    <Table as EntityTable<SphericalSurfaceHolder>>::get_owned(table, surface_id)
                        .map_err(|error| format!("cannot resolve STEP sphere: {error}"))?;
                let surface = truck_stepio::r#in::alias::SphericalSurface::from(&source);
                append_closed_surface(&surface, invert, vertex, &mut scene, budget)?;
            }
            "TOROIDAL_SURFACE" => {
                let major = positive_radius(
                    surface_params
                        .get(2)
                        .ok_or("missing STEP torus major radius")?,
                )?;
                let minor = positive_radius(
                    surface_params
                        .get(3)
                        .ok_or("missing STEP torus minor radius")?,
                )?;
                if major <= minor {
                    return Err("STEP horn/spindle torus surfaces are not implemented".into());
                }
                let source =
                    <Table as EntityTable<ToroidalSurfaceHolder>>::get_owned(table, surface_id)
                        .map_err(|error| format!("cannot resolve STEP torus: {error}"))?;
                let surface = truck_stepio::r#in::alias::ToroidalSurface::from(&source);
                append_closed_surface(&surface, invert, vertex, &mut scene, budget)?;
            }
            _ => {
                return Err(
                    "STEP vertex-loop surface requires a supported full sphere or torus".into(),
                );
            }
        }
    }
    scene.validate()?;
    Ok(Some(scene))
}

/// Read a required positional entity field.
///
/// # Errors
///
/// The entity's parameter list lacks the requested field.
fn field(record: &Record, index: usize) -> Result<&Parameter> {
    list(&record.parameter)?
        .get(index)
        .ok_or_else(|| "truncated STEP assembly entity".into())
}

/// Find an entity's record with the requested schema name, if present.
///
/// # Errors
///
/// The referenced entity is missing; an absent matching record returns `None`.
fn named_record<'a>(
    entities: &HashMap<u64, &'a EntityInstance>,
    id: u64,
    name: &str,
) -> Result<Option<&'a Record>> {
    let entity = entities
        .get(&id)
        .ok_or_else(|| format!("STEP references missing entity #{id}"))?;
    Ok(records(entity)
        .1
        .iter()
        .find(|record| record.name.as_str() == name))
}

/// Read a finite integer or real STEP parameter.
///
/// # Errors
///
/// The value is not numeric or is non-finite.
#[allow(
    clippy::cast_precision_loss,
    reason = "STEP integer dimensions intentionally use the geometry kernel's f64 representation, matching real parameters."
)]
fn number(parameter: &Parameter) -> Result<f64> {
    let value = match parameter {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        Parameter::Typed { parameter, .. } => return number(parameter),
        _ => return Err("STEP assembly requires a numeric value".into()),
    };
    if !value.is_finite() {
        return Err("STEP assembly numeric value is non-finite".into());
    }
    Ok(value)
}

/// Resolve supported SI and conversion-based length units to millimetres.
///
/// # Errors
///
/// Units are missing, unsupported, nonpositive or recursively exceed the unit-depth limit.
fn unit_factor(entities: &HashMap<u64, &EntityInstance>, id: u64, depth: usize) -> Result<f64> {
    if depth > 32 {
        return Err("STEP unit definitions are cyclic or too deep".into());
    }
    let factor = if let Some(unit) = named_record(entities, id, "SI_UNIT")? {
        if !matches!(field(unit, 1)?, Parameter::Enumeration(name) if name == "METRE") {
            return Err("STEP assembly length unit must be a supported metre unit".into());
        }
        let exponent = match field(unit, 0)? {
            Parameter::NotProvided => 0,
            Parameter::Enumeration(prefix) => match prefix.as_str() {
                "EXA" => 18,
                "PETA" => 15,
                "TERA" => 12,
                "GIGA" => 9,
                "MEGA" => 6,
                "KILO" => 3,
                "HECTO" => 2,
                "DECA" => 1,
                "DECI" => -1,
                "CENTI" => -2,
                "MILLI" => -3,
                "MICRO" => -6,
                "NANO" => -9,
                "PICO" => -12,
                "FEMTO" => -15,
                "ATTO" => -18,
                _ => return Err("unsupported STEP SI length prefix".into()),
            },
            _ => return Err("invalid STEP SI length prefix".into()),
        };
        1000. * 10f64.powi(exponent)
    } else if let Some(unit) = named_record(entities, id, "CONVERSION_BASED_UNIT")? {
        let value_id = reference(field(unit, 1)?)?;
        // Complex STEP instances put inherited values in MEASURE_WITH_UNIT;
        // LENGTH_MEASURE_WITH_UNIT() is only an empty subtype marker there.
        let value = named_record(entities, value_id, "MEASURE_WITH_UNIT")?
            .or(named_record(
                entities,
                value_id,
                "LENGTH_MEASURE_WITH_UNIT",
            )?)
            .ok_or("unsupported STEP conversion-based length measure")?;
        number(field(value, 0)?)? * unit_factor(entities, reference(field(value, 1)?)?, depth + 1)?
    } else {
        return Err("unsupported STEP assembly length unit".into());
    };
    if !factor.is_finite() || factor <= 0. {
        return Err("invalid STEP assembly unit scale".into());
    }
    Ok(factor)
}

/// Read the length-unit scale attached to a representation context.
///
/// # Errors
///
/// The context/unit structure is missing, ambiguous or unsupported.
#[allow(
    clippy::float_cmp,
    reason = "Representation dimensionality must equal the exact schema value three."
)]
fn context_factor(entities: &HashMap<u64, &EntityInstance>, id: u64) -> Result<f64> {
    if let Some(context) = named_record(entities, id, "GEOMETRIC_REPRESENTATION_CONTEXT")?
        && number(field(context, 0)?)? != 3.
    {
        return Err("STEP assembly requires 3D representation contexts".into());
    }
    let Some(units) = named_record(entities, id, "GLOBAL_UNIT_ASSIGNED_CONTEXT")? else {
        return Ok(1.);
    };
    let mut scale = None;
    for unit in list(field(units, 0)?)? {
        let unit = reference(unit)?;
        if named_record(entities, unit, "LENGTH_UNIT")?.is_some() {
            if scale.is_some() {
                return Err("STEP context has multiple length units".into());
            }
            scale = Some(unit_factor(entities, unit, 0)?);
        }
    }
    scale.ok_or_else(|| "STEP unit context lacks a supported length unit".into())
}

/// Read three finite Cartesian or direction components from an entity.
///
/// # Errors
///
/// The entity kind, dimensionality or numeric components are invalid.
fn coordinate(entities: &HashMap<u64, &EntityInstance>, id: u64, kind: &str) -> Result<Vec3> {
    let record = named_record(entities, id, kind)?
        .ok_or_else(|| format!("STEP placement requires {kind}"))?;
    let values = list(field(record, 1)?)?;
    if values.len() != 3 {
        return Err("STEP placement coordinates must be three-dimensional".into());
    }
    Ok(Vec3::new(
        number(&values[0])?,
        number(&values[1])?,
        number(&values[2])?,
    ))
}

/// Resolve an optional normalized direction, using the supplied default when omitted.
///
/// # Errors
///
/// The referenced direction is missing, invalid or has zero length.
fn direction(
    entities: &HashMap<u64, &EntityInstance>,
    parameter: &Parameter,
    default: Vec3,
) -> Result<Vec3> {
    let value = if matches!(parameter, Parameter::NotProvided) {
        default
    } else {
        coordinate(entities, reference(parameter)?, "DIRECTION")?
    };
    let length = value.length();
    if !length.is_finite() || length <= 0. {
        return Err("STEP placement has an invalid direction".into());
    }
    Ok(value / length)
}

/// Build a rigid frame from an axis placement in the representation's units.
///
/// # Errors
///
/// The placement, coordinates or axis directions cannot define a finite orthogonal frame.
fn axis_frame(entities: &HashMap<u64, &EntityInstance>, id: u64, scale: f64) -> Result<Transform> {
    let record = named_record(entities, id, "AXIS2_PLACEMENT_3D")?
        .ok_or("STEP assembly supports 3D axis placements only")?;
    let origin = coordinate(entities, reference(field(record, 1)?)?, "CARTESIAN_POINT")? * scale;
    let z = direction(entities, field(record, 2)?, Vec3::new(0., 0., 1.))?;
    let default_x = if z.cross(Vec3::new(1., 0., 0.)).length() > 1e-12 {
        Vec3::new(1., 0., 0.)
    } else {
        Vec3::new(0., 1., 0.)
    };
    let reference = direction(entities, field(record, 3)?, default_x)?;
    let x = reference - z * reference.dot(z);
    if x.length() < 1e-12 {
        return Err("STEP placement axis and reference direction are parallel".into());
    }
    let x = x / x.length();
    let y = z.cross(x);
    let transform = Transform([
        [x.x, x.y, x.z, 0.],
        [y.x, y.y, y.z, 0.],
        [z.x, z.y, z.z, 0.],
        [origin.x, origin.y, origin.z, 1.],
    ]);
    if transform.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("STEP placement is non-finite".into());
    }
    Ok(transform)
}

/// Frames are orthonormal, so inverse rotation is its transpose.
fn inverse_frame(transform: Transform) -> Transform {
    let mut inverse = Transform::IDENTITY;
    for column in 0..3 {
        for row in 0..3 {
            inverse.0[column][row] = transform.0[row][column];
        }
    }
    let origin = Vec3::new(transform.0[3][0], transform.0[3][1], transform.0[3][2]);
    let origin = inverse.apply(origin) * -1.;
    inverse.0[3] = [origin.x, origin.y, origin.z, 1.];
    inverse
}

/// Identify supported representation schema names.
fn is_shape_representation(name: &str) -> bool {
    (name.ends_with("SHAPE_REPRESENTATION") && name != "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
        || name == "TESSELLATED_SHAPE_REPRESENTATION_WITH_ACCURACY_PARAMETERS"
}

/// Shape items and authored unit scale belonging to one representation.
struct Representation {
    /// Entity identifiers for geometry or mapped items owned by this representation.
    items: Vec<u64>,
    /// Authored representation length scale in millimetres.
    scale: f64,
}
/// A placed child representation owned by an assembly occurrence.
#[derive(Clone, Copy)]
struct Placement {
    /// Product-occurrence relationship identifier owning this placement.
    occurrence: u64,
    /// Entity identifier of the placed child representation.
    child: u64,
    /// Local-to-parent affine placement in normalized scene units.
    transform: Transform,
}
/// Indexed representation ownership, equivalent-shape groups and assembly roots.
struct Assembly<'a> {
    /// Indexed source entities used to resolve typed geometry and placements.
    entities: &'a HashMap<u64, &'a EntityInstance>,
    /// Shape representations indexed by their source entity identifiers.
    representations: BTreeMap<u64, Representation>,
    /// Placed child occurrences grouped by parent representation identifier.
    children: HashMap<u64, Vec<Placement>>,
    /// Equivalent representation groups for shared product-shape ownership.
    groups: BTreeMap<u64, Vec<u64>>,
    /// Representation identifiers selected as assembly roots.
    roots: Vec<u64>,
}
/// Mutable assembly expansion state and shared tessellated item cache.
struct Expansion<'a> {
    /// Cached tessellated item scenes indexed by source geometry identifier.
    meshes: &'a HashMap<u64, Scene>,
    /// Owned AP242 decoder loading only geometry selected by shape ownership.
    decoder: step_tessellation::Decoder<'a>,
    /// Owned planar polygon decoder loading only selected topology.
    faceted: step_faceted::Decoder<'a>,
    /// Decoded owned item scenes reused across repeated part occurrences.
    owned: HashMap<u64, Scene>,
    /// Source dependency graph used to validate selected polygonal geometry.
    graph: &'a HashMap<u64, Vec<u64>>,
    /// Dependency depths already checked for selected polygonal items.
    dependencies: HashMap<u64, usize>,
    /// Cumulative geometry, allocation and processing budgets across every loader.
    budget: Budget,
    /// Scene receiving all expanded assembly occurrences.
    scene: Scene,
    /// Active representation identifiers used to reject expansion cycles.
    visiting: HashSet<u64>,
    /// Geometry item identifiers consumed by successful expansion.
    used: HashSet<u64>,
    /// Accumulated processing work charged against the preview budget.
    work: usize,
}

impl<'a> Assembly<'a> {
    /// Resolve an assembly representation by its entity identifier.
    ///
    /// # Errors
    ///
    /// The identifier does not name a known representation.
    fn representation(&self, id: u64) -> Result<&Representation> {
        self.representations.get(&id).ok_or_else(|| {
            format!("STEP assembly references unsupported shape representation #{id}").into()
        })
    }
    /// Build representation ownership, placement relationships and assembly roots.
    ///
    /// # Errors
    ///
    /// Representation/unit/product relationships are missing, ambiguous, unsupported or inconsistent.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    fn new(entities: &'a HashMap<u64, &'a EntityInstance>) -> Result<Self> {
        let mut assembly = Self {
            entities,
            representations: BTreeMap::new(),
            children: HashMap::new(),
            groups: BTreeMap::new(),
            roots: Vec::new(),
        };
        let mut items_count = 0usize;
        for (&id, entity) in entities {
            let entity_records = records(entity).1;
            if !entity_records
                .iter()
                .any(|r| is_shape_representation(r.name.as_str()))
            {
                continue;
            }
            let record = match entity {
                EntityInstance::Simple { record, .. } => record,
                EntityInstance::Complex { .. } => entity_records
                    .iter()
                    .find(|r| r.name.as_str() == "REPRESENTATION")
                    .ok_or("unsupported complex STEP shape representation")?,
            };
            let items = list(field(record, 1)?)?
                .iter()
                .map(reference)
                .collect::<Result<Vec<_>>>()?;
            items_count = items_count
                .checked_add(items.len())
                .ok_or("STEP assembly item count overflow")?;
            if items_count > MAX_ENTITIES {
                return Err("STEP assembly exceeds representation-item limit".into());
            }
            assembly.representations.insert(
                id,
                Representation {
                    items,
                    scale: context_factor(entities, reference(field(record, 2)?)?)?,
                },
            );
        }
        if assembly.representations.is_empty() {
            return Err("STEP assembly has no supported shape representations".into());
        }
        let mut owners = HashMap::new();
        let mut relationships = BTreeMap::new();
        for (&id, entity) in entities {
            for record in records(entity).1 {
                if record.name.as_str() == "SHAPE_DEFINITION_REPRESENTATION" {
                    let rep = reference(field(record, 1)?)?;
                    let definition = reference(field(record, 0)?)?;
                    if let Some(shape) =
                        named_record(entities, definition, "PRODUCT_DEFINITION_SHAPE")?
                    {
                        let product = reference(field(shape, 2)?)?;
                        if named_record(entities, product, "PRODUCT_DEFINITION")?.is_some() {
                            assembly.representation(rep)?;
                            if owners.insert(rep, product).is_some_and(|p| p != product) {
                                return Err(
                                    "STEP representation has ambiguous product ownership".into()
                                );
                            }
                        }
                    }
                }
                if record.name.as_str() == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION" {
                    let relationship = named_record(entities, id, "REPRESENTATION_RELATIONSHIP")?
                        .unwrap_or(record);
                    let parameters = list(&record.parameter)?;
                    let operator = if parameters.len() == 1 {
                        reference(&parameters[0])?
                    } else {
                        reference(field(record, 4)?)?
                    };
                    let first = reference(field(relationship, 2)?)?;
                    let second = reference(field(relationship, 3)?)?;
                    assembly.representation(first)?;
                    assembly.representation(second)?;
                    let transform = assembly.relationship_transform(operator, first, second)?;
                    relationships.insert(id, (first, second, transform));
                }
            }
        }
        let mut equivalent = Vec::new();
        for &id in entities.keys() {
            if !relationships.contains_key(&id)
                && let Some(record) =
                    named_record(entities, id, "SHAPE_REPRESENTATION_RELATIONSHIP")?
            {
                let record =
                    named_record(entities, id, "REPRESENTATION_RELATIONSHIP")?.unwrap_or(record);
                let first = reference(field(record, 2)?)?;
                let second = reference(field(record, 3)?)?;
                assembly.representation(first)?;
                assembly.representation(second)?;
                equivalent.push((first, second));
            }
        }
        // A producer may attach an occurrence to the B-rep representation,
        // while the product owner is attached to its equivalent axis-only
        // representation. Propagate ownership before resolving occurrences.
        assembly.group_equivalent_representations(&equivalent, &mut owners)?;
        for (&anchor, members) in &assembly.groups {
            if let Some(&owner) = owners.get(&anchor) {
                for &member in members {
                    owners.insert(member, owner);
                }
            }
        }
        let mut usages = HashSet::new();
        let mut consumed_relationships = HashSet::new();
        // Product ownership determines direction even when a producer reverses
        // the relationship. See STEPConstruct_Assembly::CheckSRRReversesNAUO.
        for entity in entities.values() {
            for record in records(entity).1 {
                if record.name.as_str() != "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" {
                    continue;
                }
                let relationship = reference(field(record, 0)?)?;
                let &(first, second, transform) = relationships.get(&relationship).ok_or(
                    "STEP assembly occurrence lacks a supported relationship transformation",
                )?;
                let shape_id = reference(field(record, 1)?)?;
                let shape = named_record(entities, shape_id, "PRODUCT_DEFINITION_SHAPE")?
                    .ok_or("STEP assembly occurrence lacks a product-definition shape")?;
                let usage_id = reference(field(shape, 2)?)?;
                let usage = named_record(entities, usage_id, "NEXT_ASSEMBLY_USAGE_OCCURRENCE")?
                    .ok_or("unsupported STEP assembly usage relationship")?;
                let parent = reference(field(usage, 3)?)?;
                let child = reference(field(usage, 4)?)?;
                let first_owner = owners
                    .get(&first)
                    .ok_or("STEP assembly representation lacks product ownership")?;
                let second_owner = owners
                    .get(&second)
                    .ok_or("STEP assembly representation lacks product ownership")?;
                let (parent_rep, placement) = if *first_owner == child && *second_owner == parent {
                    (
                        second,
                        Placement {
                            occurrence: records(entity).0,
                            child: first,
                            transform,
                        },
                    )
                } else if *first_owner == parent && *second_owner == child {
                    (
                        first,
                        Placement {
                            occurrence: records(entity).0,
                            child: second,
                            transform: inverse_frame(transform),
                        },
                    )
                } else {
                    return Err("STEP representation relationship disagrees with assembly product ownership".into());
                };
                assembly
                    .children
                    .entry(parent_rep)
                    .or_default()
                    .push(placement);
                usages.insert(usage_id);
                consumed_relationships.insert(relationship);
            }
        }
        for (&id, (first, second, transform)) in &relationships {
            if !consumed_relationships.contains(&id) {
                assembly
                    .children
                    .entry(*second)
                    .or_default()
                    .push(Placement {
                        occurrence: id,
                        child: *first,
                        transform: *transform,
                    });
            }
        }
        for (&id, entity) in entities {
            for record in records(entity).1 {
                match record.name.as_str() {
                    "NEXT_ASSEMBLY_USAGE_OCCURRENCE" if !usages.contains(&id) => {
                        return Err(
                            "STEP assembly usage has no supported component placement".into()
                        );
                    }
                    _ => {}
                }
            }
        }
        // Mapping-origin subtraction matters when the referenced part is not
        // authored at the origin. Frames and geometry use each context's units.
        let mut mapped_owners = HashMap::<u64, Vec<u64>>::new();
        let mut presentation_items = HashSet::new();
        for (&id, entity) in entities {
            if records(entity)
                .1
                .iter()
                .any(|record| record.name.as_str() == "DRAUGHTING_MODEL")
            {
                let record = named_record(entities, id, "REPRESENTATION")?
                    .or(named_record(entities, id, "DRAUGHTING_MODEL")?)
                    .ok_or("STEP draughting model lacks representation attributes")?;
                for item in list(field(record, 1)?)? {
                    presentation_items.insert(reference(item)?);
                }
            }
        }
        for (&parent, representation) in &assembly.representations {
            for &item in &representation.items {
                if named_record(entities, item, "MAPPED_ITEM")?.is_some() {
                    mapped_owners.entry(item).or_default().push(parent);
                }
            }
        }
        for (&id, entity) in entities {
            if records(entity)
                .1
                .iter()
                .any(|r| r.name.as_str() == "MAPPED_ITEM")
                && !mapped_owners.contains_key(&id)
                && !presentation_items.contains(&id)
            {
                return Err("STEP mapped item has no owning shape representation".into());
            }
        }
        for (item, parents) in mapped_owners {
            let record = named_record(entities, item, "MAPPED_ITEM")?.unwrap();
            let mapping = reference(field(record, 1)?)?;
            let mapping = named_record(entities, mapping, "REPRESENTATION_MAP")?
                .ok_or("STEP mapped item lacks a representation map")?;
            let origin = reference(field(mapping, 0)?)?;
            let child = reference(field(mapping, 1)?)?;
            let source = axis_frame(entities, origin, assembly.representation(child)?.scale)?;
            for parent in parents {
                let target = axis_frame(
                    entities,
                    reference(field(record, 2)?)?,
                    assembly.representation(parent)?.scale,
                )?;
                assembly
                    .children
                    .entry(parent)
                    .or_default()
                    .push(Placement {
                        occurrence: item,
                        child,
                        transform: target.then(inverse_frame(source)),
                    });
            }
        }
        assembly.group_equivalent_representations(&equivalent, &mut owners)?;
        // Check every representation, including disconnected cycles, before
        // instance expansion. Shared parts are legal; cyclic maps are not.
        let representation_graph: HashMap<_, _> = assembly
            .groups
            .keys()
            .map(|&id| {
                (
                    id,
                    assembly
                        .children
                        .get(&id)
                        .map(|children| children.iter().map(|p| p.child).collect())
                        .unwrap_or_default(),
                )
            })
            .collect();
        let mut checked = HashMap::new();
        for &id in assembly.groups.keys() {
            check_graph(
                id,
                &representation_graph,
                &mut HashSet::new(),
                &mut checked,
                0,
            )?;
        }
        let incoming: HashSet<_> = assembly
            .children
            .values()
            .flatten()
            .map(|p| p.child)
            .collect();
        assembly.roots = assembly
            .groups
            .keys()
            .filter(|id| !incoming.contains(id) && (owners.is_empty() || owners.contains_key(id)))
            .copied()
            .collect();
        if assembly.roots.is_empty() {
            return Err("STEP assembly has no roots or contains a cycle".into());
        }
        // Deterministic hierarchy order makes repeated instances stable across
        // platforms while keeping separate coincident authored occurrences.
        for children in assembly.children.values_mut() {
            children.sort_by(|a, b| {
                a.child.cmp(&b.child).then_with(|| {
                    a.transform
                        .0
                        .iter()
                        .flatten()
                        .zip(b.transform.0.iter().flatten())
                        .find_map(|(a, b)| {
                            let order = a.total_cmp(b);
                            (!order.is_eq()).then_some(order)
                        })
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            });
        }
        Ok(assembly)
    }
    /// Plain shape relationships link equivalent representations, with no
    /// directional occurrence transform. Both endpoint orders occur in AP203
    /// exporters. Preserve every member's context while grouping the geometry.
    fn group_equivalent_representations(
        &mut self,
        equivalent: &[(u64, u64)],
        owners: &mut HashMap<u64, u64>,
    ) -> Result<()> {
        self.groups.clear();
        let mut adjacency = HashMap::<u64, Vec<u64>>::new();
        for &(first, second) in equivalent {
            adjacency.entry(first).or_default().push(second);
            adjacency.entry(second).or_default().push(first);
        }
        let mut aliases = HashMap::new();
        let mut grouped_owners = HashMap::new();
        for &id in self.representations.keys() {
            if aliases.contains_key(&id) {
                continue;
            }
            let mut pending = vec![id];
            let mut members = Vec::new();
            // Mark on insertion, so redundant undirected links remain bounded.
            aliases.insert(id, id);
            while let Some(next) = pending.pop() {
                members.push(next);
                for &neighbor in adjacency.get(&next).into_iter().flatten() {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        aliases.entry(neighbor)
                    {
                        entry.insert(id);
                        pending.push(neighbor);
                    }
                }
            }
            members.sort_unstable();
            let owned: Vec<_> = members
                .iter()
                .filter_map(|id| owners.get(id).map(|&owner| (*id, owner)))
                .collect();
            if owned.iter().any(|&(_, owner)| owner != owned[0].1) {
                return Err(
                    "STEP plain shape relationship ambiguously links different products".into(),
                );
            }
            let anchor = owned.first().map_or(members[0], |&(id, _)| id);
            if let Some(&(_, owner)) = owned.first() {
                grouped_owners.insert(anchor, owner);
            }
            for &member in &members {
                aliases.insert(member, anchor);
            }
            self.groups.insert(anchor, members);
        }
        let mut grouped_children = HashMap::<u64, Vec<Placement>>::new();
        // A mapped item shared by equivalent representations denotes the same
        // occurrence. Distinct authored occurrence IDs remain distinct even
        // when their placements coincide.
        let mut occurrences = HashMap::<(u64, u64), Placement>::new();
        for (parent, children) in std::mem::take(&mut self.children) {
            let parent = aliases[&parent];
            for mut child in children {
                child.child = aliases[&child.child];
                if let Some(previous) = occurrences.insert((parent, child.occurrence), child) {
                    if previous.child != child.child || previous.transform.0 != child.transform.0 {
                        return Err(
                            "STEP equivalent representations disagree on occurrence placement"
                                .into(),
                        );
                    }
                    continue;
                }
                grouped_children.entry(parent).or_default().push(child);
            }
        }
        self.children = grouped_children;
        *owners = grouped_owners;
        Ok(())
    }
    /// Resolve the source-to-target placement operator between two representations.
    ///
    /// # Errors
    ///
    /// The operator or axis frames are missing, unsupported or invalid.
    fn relationship_transform(&self, operator: u64, first: u64, second: u64) -> Result<Transform> {
        let record = named_record(self.entities, operator, "ITEM_DEFINED_TRANSFORMATION")?
            .ok_or("STEP supports axis-based item-defined assembly transformations only")?;
        let mut origin = reference(field(record, 2)?)?;
        let mut target = reference(field(record, 3)?)?;
        let source_rep = self.representation(first)?;
        let target_rep = self.representation(second)?;
        if !source_rep.items.contains(&origin)
            && source_rep.items.contains(&target)
            && target_rep.items.contains(&origin)
            && !target_rep.items.contains(&target)
        {
            std::mem::swap(&mut origin, &mut target);
        }
        let origin = axis_frame(self.entities, origin, source_rep.scale)?;
        let target = axis_frame(self.entities, target, target_rep.scale)?;
        // OCCT STEPControl_ActorRead::ComputeTransformation converts both
        // frames to common units before mapping the source frame to target.
        Ok(target.then(inverse_frame(origin)))
    }
    /// Expand assembly roots using cached, tessellated item geometry.
    ///
    /// # Errors
    ///
    /// Expansion encounters invalid references, missing geometry, cycles or occurrence/triangle limits.
    fn render(
        &self,
        meshes: &HashMap<u64, Scene>,
        graph: &HashMap<u64, Vec<u64>>,
        budget: Budget,
    ) -> Result<Scene> {
        let mut expansion = Expansion {
            meshes,
            decoder: step_tessellation::Decoder::new(self.entities),
            faceted: step_faceted::Decoder::new(self.entities),
            owned: HashMap::new(),
            graph,
            dependencies: HashMap::new(),
            budget,
            scene: Scene::default(),
            visiting: HashSet::new(),
            used: HashSet::new(),
            work: 0,
        };
        for &root in &self.roots {
            self.expand(root, Transform::IDENTITY, 0, &mut expansion)?;
        }
        if meshes.keys().any(|id| !expansion.used.contains(id)) {
            return Err("STEP assembly contains unowned or unsupported required shells".into());
        }
        expansion.scene.validate()?;
        Ok(expansion.scene)
    }
    /// Expand a representation's items and placed child occurrences recursively.
    ///
    /// # Errors
    ///
    /// Representations are missing, cyclic or exceed the expansion budget.
    fn expand(
        &self,
        id: u64,
        transform: Transform,
        depth: usize,
        expansion: &mut Expansion<'_>,
    ) -> Result<()> {
        expansion.work += 1;
        if depth > MAX_REFERENCE_DEPTH || expansion.work > MAX_ENTITIES {
            return Err("STEP assembly exceeds hierarchy depth or expansion limit".into());
        }
        if !expansion.visiting.insert(id) {
            return Err("STEP assembly contains a representation cycle".into());
        }
        let mut exact_items = HashSet::new();
        for &member in self
            .groups
            .get(&id)
            .ok_or("missing STEP representation group")?
        {
            let representation = self.representation(member)?;
            for &item in &representation.items {
                if let Some(shell) = step_faceted::solid_outer(self.entities[&item])? {
                    exact_items.insert((item, representation.scale.to_bits()));
                    exact_items.insert((shell, representation.scale.to_bits()));
                } else if let Some(record) =
                    named_record(self.entities, item, "MANIFOLD_SOLID_BREP")?
                {
                    exact_items.insert((item, representation.scale.to_bits()));
                    exact_items.insert((
                        reference(field(record, 1)?)?,
                        representation.scale.to_bits(),
                    ));
                } else if let Some(record) =
                    named_record(self.entities, item, "SHELL_BASED_SURFACE_MODEL")?
                {
                    for shell in list(field(record, 1)?)? {
                        exact_items.insert((reference(shell)?, representation.scale.to_bits()));
                    }
                } else if records(self.entities[&item])
                    .1
                    .iter()
                    .any(|record| matches!(record.name.as_str(), "OPEN_SHELL" | "CLOSED_SHELL"))
                {
                    exact_items.insert((item, representation.scale.to_bits()));
                }
            }
        }
        let mut included = HashSet::new();
        for &member in self
            .groups
            .get(&id)
            .ok_or("missing STEP representation group")?
        {
            let representation = self.representation(member)?;
            let mut scale = Transform::IDENTITY;
            for i in 0..3 {
                scale.0[i][i] = representation.scale;
            }
            let geometry_transform = transform.then(scale);
            if geometry_transform
                .0
                .iter()
                .flatten()
                .any(|v| !v.is_finite())
            {
                return Err("STEP assembled placement is non-finite".into());
            }
            for &item in &representation.items {
                // An explicit top-level geometric link names the same exact body
                // in this equivalent representation group. Preserve separate
                // bodies and occurrences; matching coordinates are insufficient.
                if let Some(link) =
                    step_tessellation::exact_body_link(self.entities[&item], self.entities)?
                    && exact_items.contains(&(link, representation.scale.to_bits()))
                {
                    continue;
                }
                if included.insert((item, representation.scale.to_bits())) {
                    self.append_item(item, geometry_transform, 0, expansion)?;
                }
            }
        }
        if let Some(children) = self.children.get(&id) {
            for child in children {
                self.expand(
                    child.child,
                    transform.then(child.transform),
                    depth + 1,
                    expansion,
                )?;
            }
        }
        expansion.visiting.remove(&id);
        Ok(())
    }
    /// Append one representation item, mapped item or cached shell under its world placement.
    ///
    /// # Errors
    ///
    /// The item is missing, unsupported, cyclic or exceeds expansion/geometry limits.
    fn append_item(
        &self,
        id: u64,
        transform: Transform,
        depth: usize,
        expansion: &mut Expansion<'_>,
    ) -> Result<()> {
        expansion.work += 1;
        if depth > MAX_REFERENCE_DEPTH || expansion.work > MAX_ENTITIES {
            return Err("STEP assembly exceeds geometry-item expansion limit".into());
        }
        if !expansion.owned.contains_key(&id) {
            if step_faceted::handles(self.entities[&id])
                || step_faceted::owns_shell(self.entities, id, &mut expansion.budget)?
            {
                check_graph(
                    id,
                    expansion.graph,
                    &mut HashSet::new(),
                    &mut expansion.dependencies,
                    0,
                )?;
            }
            let mesh = if let Some(mesh) = expansion.faceted.decode(id, &mut expansion.budget)? {
                Some(mesh)
            } else {
                expansion.decoder.decode(id, &mut expansion.budget)?
            };
            if let Some(mesh) = mesh {
                expansion.owned.insert(id, mesh);
            }
        }
        if let Some(mesh) = expansion.owned.get(&id) {
            append_mesh(mesh, transform, &mut expansion.scene, &mut expansion.budget)?;
            expansion.used.insert(id);
            return Ok(());
        }
        let record = simple_record(self.entities, id)?;
        match record.name.as_str() {
            "AXIS2_PLACEMENT_3D" | "MAPPED_ITEM" => {}
            "MANIFOLD_SOLID_BREP" => self.append_item(
                reference(field(record, 1)?)?,
                transform,
                depth + 1,
                expansion,
            )?,
            "SHELL_BASED_SURFACE_MODEL" => {
                for child in list(field(record, 1)?)? {
                    self.append_item(reference(child)?, transform, depth + 1, expansion)?;
                }
            }
            "OPEN_SHELL" | "CLOSED_SHELL" => {
                let mesh = expansion
                    .meshes
                    .get(&id)
                    .ok_or("STEP component references an unsupported shell")?;
                append_mesh(mesh, transform, &mut expansion.scene, &mut expansion.budget)?;
                expansion.used.insert(id);
            }
            _ => {
                return Err(format!(
                    "unsupported required STEP assembly geometry {} (#{id})",
                    record.name.as_str()
                )
                .into());
            }
        }
        Ok(())
    }
}

/// Append cached triangles and rotate their authored normals under a rigid frame and positive scale.
///
/// # Errors
///
/// The expanded geometry exceeds the triangle limit or a transformed normal is invalid.
fn append_mesh(
    mesh: &Scene,
    transform: Transform,
    scene: &mut Scene,
    budget: &mut Budget,
) -> Result<()> {
    if scene.triangles.len() + mesh.triangles.len() > MAX_TRIANGLES {
        return Err("STEP assembled preview exceeds triangle limit".into());
    }
    budget.storage(mesh.triangles.len(), std::mem::size_of::<Triangle>())?;
    budget.storage(
        mesh.appearances.len(),
        std::mem::size_of::<crate::scene::TriangleAppearance>() + 4 * std::mem::size_of::<usize>(),
    )?;
    budget.work(
        mesh.triangles
            .len()
            .checked_mul(3)
            .ok_or("STEP expanded corner work overflows")?,
    )?;
    scene.triangles.try_reserve_exact(mesh.triangles.len())?;
    let offset = scene.triangles.len();
    for triangle in &mesh.triangles {
        scene.triangles.push(Triangle {
            vertices: triangle.vertices.map(|point| transform.apply(point)),
            color: triangle.color,
        });
    }
    for (&index, appearance) in &mesh.appearances {
        let mut appearance = appearance.clone();
        if let Some(normals) = appearance.normals {
            let mut transformed = normals;
            for normal in &mut transformed {
                let components = [normal.x, normal.y, normal.z];
                let result: [f64; 3] = std::array::from_fn(|row| {
                    (0..3)
                        .map(|column| transform.0[column][row] * components[column])
                        .sum()
                });
                let direction = Vec3::new(result[0], result[1], result[2]);
                let length = direction.length();
                if !length.is_finite() || length <= 0. {
                    return Err("STEP placement produces an invalid tessellated normal".into());
                }
                *normal = direction / length;
            }
            appearance.normals = Some(transformed);
        }
        scene.appearances.insert(offset + index, appearance);
    }
    Ok(())
}

/// Identify outer-shell hints without requiring unused faceted geometry to be valid.
fn faceted_outer_hint(entity: &EntityInstance) -> Option<u64> {
    let (_, parts) = records(entity);
    if !parts.iter().any(|record| record.name == "FACETED_BREP") {
        return None;
    }
    let (record, index) = match entity {
        EntityInstance::Simple { record, .. } => (record, 1),
        EntityInstance::Complex { .. } => (
            parts
                .iter()
                .find(|record| record.name == "MANIFOLD_SOLID_BREP")?,
            0,
        ),
    };
    list(&record.parameter)
        .ok()?
        .get(index)
        .and_then(|value| reference(value).ok())
}

/// Partition polygon shells and explicitly faceted outer shells before kernel loading.
///
/// # Errors
///
/// Bounded shell classification exceeds its reference ceiling.
fn polygon_shells(
    entities: &HashMap<u64, &EntityInstance>,
    budget: &mut Budget,
) -> Result<HashSet<u64>> {
    let mut owned: HashSet<_> = entities
        .values()
        .filter_map(|entity| faceted_outer_hint(entity))
        .collect();
    for (&id, entity) in entities {
        if records(entity)
            .1
            .iter()
            .any(|record| matches!(record.name.as_str(), "OPEN_SHELL" | "CLOSED_SHELL"))
            && step_faceted::owns_shell(entities, id, budget)?
        {
            owned.insert(id);
        }
    }
    Ok(owned)
}

/// Collect existing geometry dependencies once per loader family with bounded storage and work.
///
/// # Errors
///
/// Metadata traversal or retained lookup storage exceeds the shared preview budget.
fn dependency_closure(
    roots: &HashSet<u64>,
    graph: &HashMap<u64, Vec<u64>>,
    budget: &mut Budget,
) -> Result<HashSet<u64>> {
    let mut seen = HashSet::new();
    let mut pending = Vec::new();
    for &id in roots {
        if graph.contains_key(&id) && seen.insert(id) {
            budget.storage(1, 4 * std::mem::size_of::<u64>())?;
            pending.push(id);
        }
    }
    while let Some(id) = pending.pop() {
        let references = &graph[&id];
        budget.work(references.len())?;
        for &next in references {
            if graph.contains_key(&next) && seen.insert(next) {
                budget.storage(1, 4 * std::mem::size_of::<u64>())?;
                pending.push(next);
            }
        }
    }
    Ok(seen)
}

/// Parse Part 21 entities, tessellate required shells and instantiate assembly geometry.
///
/// # Errors
///
/// Syntax, entity dependencies, CAD surfaces, placements or preview limits fail validation.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn parse(text: &str) -> Result<Scene> {
    check_nesting(text)?;
    let exchange = step_parameters::parse(text)?;
    if exchange.data.is_empty() {
        return Err("STEP file has no DATA section".into());
    }
    let mut table = Table::default();
    let mut graph = HashMap::new();
    let mut entities = HashMap::new();
    for entity in exchange.data.iter().flat_map(|section| &section.entities) {
        if graph.len() >= MAX_ENTITIES {
            return Err("STEP file exceeds entity limit".into());
        }
        let (id, records) = records(entity);
        let mut references = Vec::new();
        for record in records {
            collect_references(&record.parameter, &mut references)?;
        }
        if graph.insert(id, references).is_some() {
            return Err(format!("duplicate STEP entity #{id}").into());
        }
        entities.insert(id, entity);
    }
    let mut budget = Budget::default();
    let owned_shells = polygon_shells(&entities, &mut budget)?;
    let kernel_shells: HashSet<_> = entities
        .iter()
        .filter(|(id, entity)| {
            !owned_shells.contains(id)
                && records(entity)
                    .1
                    .iter()
                    .any(|record| matches!(record.name.as_str(), "OPEN_SHELL" | "CLOSED_SHELL"))
        })
        .map(|(&id, _)| id)
        .collect();
    let owned_geometry = dependency_closure(&owned_shells, &graph, &mut budget)?;
    let kernel_geometry = dependency_closure(&kernel_shells, &graph, &mut budget)?;
    for entity in exchange.data.iter().flat_map(|section| &section.entities) {
        let (id, _) = records(entity);
        // Keep shared ordinary BREP dependencies in Truck's public table, while
        // facet-only records are validated lazily by the selected owned loader.
        if !(step_tessellation::handles(entity)
            || step_faceted::handles(entity)
            || (owned_geometry.contains(&id) && !kernel_geometry.contains(&id)))
        {
            table
                .push_instance(entity)
                .map_err(|error| format!("cannot decode STEP entity #{id}: {error}"))?;
        }
    }
    if table.shell.is_empty()
        && !entities
            .values()
            .any(|entity| step_tessellation::handles(entity) || step_faceted::handles(entity))
        && owned_shells.is_empty()
    {
        return Err("STEP file has no supported open/closed B-rep shells".into());
    }
    if table.face_surface.len() > MAX_FACES {
        return Err("STEP file exceeds face limit".into());
    }
    budget.faces(table.face_surface.len())?;
    let mut shells: Vec<_> = table
        .shell
        .iter()
        .filter(|(id, _)| !owned_shells.contains(id))
        .collect();
    shells.sort_unstable_by_key(|&(id, _)| *id);
    let mut meshes = HashMap::new();
    let mut total_triangles = 0usize;
    let mut done = HashMap::new();
    for &(id, shell_holder) in &shells {
        let id = *id;
        let mut scene = Scene::default();
        check_graph(id, &graph, &mut HashSet::new(), &mut done, 0)?;
        if let Some(analytic) = closed_analytic_shell(id, &entities, &table, &mut budget)? {
            if total_triangles + analytic.triangles.len() > MAX_TRIANGLES {
                return Err("STEP tessellation exceeds triangle limit".into());
            }
            total_triangles += analytic.triangles.len();
            meshes.insert(id, analytic);
            continue;
        }
        let owned_shell = <Table as EntityTable<ShellHolder>>::get_owned(&table, id)
            .map_err(|error| format!("cannot resolve STEP shell #{id}: {error}"))?;
        if owned_shell.cfs_faces.is_empty() {
            return Err(format!("STEP shell #{id} has no faces").into());
        }
        let shell = table
            .to_compressed_shell(shell_holder)
            .map_err(|error| format!("cannot convert STEP shell #{id}: {error}"))?;
        if shell.faces.len() != owned_shell.cfs_faces.len() {
            return Err(format!("STEP shell #{id} contains unsupported faces").into());
        }
        for (converted, original) in shell.faces.iter().zip(&owned_shell.cfs_faces) {
            let face = match original {
                FaceAny::FaceSurface(face) => face,
                FaceAny::OrientedFace(face) => &face.face_element,
            };
            if face.bounds.is_empty() || face.bounds.len() != converted.boundaries.len() {
                return Err(
                    format!("STEP shell #{id} contains unsupported face boundaries").into(),
                );
            }
            for (boundary, original) in converted.boundaries.iter().zip(&face.bounds) {
                if boundary.is_empty() || boundary.len() != original.bound.edge_list.len() {
                    return Err(
                        format!("STEP shell #{id} contains unsupported boundary edges").into(),
                    );
                }
            }
        }
        let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        {
            let mut include = |point: Point3| -> Result<()> {
                let point = Vec3::new(point.x, point.y, point.z);
                if !point.is_finite() {
                    return Err("STEP geometry contains non-finite points".into());
                }
                minimum = Vec3::new(
                    minimum.x.min(point.x),
                    minimum.y.min(point.y),
                    minimum.z.min(point.z),
                );
                maximum = Vec3::new(
                    maximum.x.max(point.x),
                    maximum.y.max(point.y),
                    maximum.z.max(point.z),
                );
                Ok(())
            };
            for point in &shell.vertices {
                include(*point)?;
            }
            // Seam vertices do not describe the extent of a full circle. Sample
            // boundary geometry so wide, thin CAD solids use a useful tolerance.
            for edge in &shell.edges {
                let (start, end) = edge.curve.parameter_range();
                let (start, end) = (finite_parameter_bound(start)?, finite_parameter_bound(end)?);
                for sample in 0..=16 {
                    include(
                        edge.curve
                            .subs(start + (end - start) * f64::from(sample) / 16.),
                    )?;
                }
            }
        }
        let diameter = (maximum - minimum).length();
        if !diameter.is_finite() || diameter <= 0.0 {
            return Err("STEP shell has no finite geometric extent".into());
        }
        let tolerance = (diameter * 0.003).max(TOLERANCE * 10.0);
        let tessellated = shell.robust_triangulation(tolerance);
        if tessellated.faces.iter().any(|face| {
            face.surface
                .as_ref()
                .is_none_or(|mesh| mesh.face_iter().next().is_none())
        }) {
            return Err(
                format!("Rust CAD tessellation failed for a face of STEP shell #{id}").into(),
            );
        }
        let mesh = tessellated.to_polygon();
        for face in mesh.face_iter() {
            let polygon = face
                .iter()
                .map(|vertex| {
                    mesh.positions()
                        .get(vertex.pos)
                        .map(|point| Vec3::new(point.x, point.y, point.z))
                        .ok_or_else(|| "STEP tessellation returned an invalid vertex index".into())
                })
                .collect::<Result<Vec<_>>>()?;
            for vertices in super::mesh::triangulate_polygon(&polygon)? {
                if scene.triangles.len() >= MAX_TRIANGLES {
                    return Err("STEP tessellation exceeds triangle limit".into());
                }
                budget.triangles(1, false)?;
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
        }
        total_triangles += scene.triangles.len();
        if total_triangles > MAX_TRIANGLES {
            return Err("STEP tessellation exceeds triangle limit".into());
        }
        meshes.insert(id, scene);
    }
    if entities.values().any(|entity| {
        records(entity).1.iter().any(|record| {
            is_shape_representation(record.name.as_str())
                || matches!(
                    record.name.as_str(),
                    "MAPPED_ITEM"
                        | "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"
                        | "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"
                        | "NEXT_ASSEMBLY_USAGE_OCCURRENCE"
                )
        })
    }) {
        return Assembly::new(&entities)?.render(&meshes, &graph, budget);
    }
    let mut scene = Scene::default();
    for (&id, _) in shells {
        scene
            .triangles
            .extend(meshes.remove(&id).unwrap().triangles);
    }
    let mut decoder = step_faceted::Decoder::new(&entities);
    let mut polygon_shells: Vec<_> = owned_shells.into_iter().collect();
    polygon_shells.sort_unstable();
    for id in polygon_shells {
        check_graph(id, &graph, &mut HashSet::new(), &mut done, 0)?;
        let mesh = decoder
            .decode(id, &mut budget)?
            .ok_or("STEP polygon shell is unsupported")?;
        append_mesh(&mesh, Transform::IDENTITY, &mut scene, &mut budget)?;
    }
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use truck_modeling::{Point3, Vector3, builder};
    use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

    fn cube_step() -> String {
        let origin = builder::vertex(Point3::new(2.0, 3.0, 4.0));
        let edge = builder::tsweep(&origin, Vector3::new(2.0, 0.0, 0.0));
        let face = builder::tsweep(&edge, Vector3::new(0.0, 3.0, 0.0));
        let cube = builder::tsweep(&face, Vector3::new(0.0, 0.0, 4.0)).compress();
        CompleteStepDisplay::new(StepModel::from(&cube), StepHeaderDescriptor::default())
            .to_string()
    }

    #[test]
    fn solid_step_tessellates_real_faces_and_preserves_bounds() {
        let scene = parse(&cube_step()).unwrap();
        assert!(scene.triangles.len() >= 12);
        assert!(scene.points.is_empty());
        for (component, expected) in [(0, (2.0, 4.0)), (1, (3.0, 6.0)), (2, (4.0, 8.0))] {
            let values: Vec<_> = scene
                .positions()
                .map(|p| [p.x, p.y, p.z][component])
                .collect();
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            assert!((min - expected.0).abs() < 1e-8);
            assert!((max - expected.1).abs() < 1e-8);
        }
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
        assert!((area - 52.0).abs() < 1e-7);
        let thumbnail = crate::render::render(
            &scene,
            &crate::render::RenderOptions {
                width: 64,
                height: 64,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(thumbnail.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
    }

    #[test]
    fn malformed_reference_and_assembly_inputs_fail() {
        let valid = cube_step();
        assert!(parse("not a STEP file").is_err());
        let point_only = "ISO-10303-21; HEADER; FILE_DESCRIPTION(('test'),'2;1'); FILE_NAME('','',(''),(''),'','',''); FILE_SCHEMA(('AUTOMOTIVE_DESIGN')); ENDSEC; DATA; #1=CARTESIAN_POINT('',(0.,0.,0.)); ENDSEC; END-ISO-10303-21;";
        assert!(parse(point_only).is_err());
        let assembly = valid.replace(
            "ENDSEC;\nEND-ISO",
            "#999999=MAPPED_ITEM('',#1,#1);\nENDSEC;\nEND-ISO",
        );
        assert!(parse(&assembly).is_err());
        let broken = valid.replacen("CARTESIAN_POINT", "UNKNOWN_GEOMETRY", 1);
        assert!(parse(&broken).is_err());
    }

    #[test]
    fn curved_step_cylinder_is_tessellated_as_a_surface() {
        let vertex = builder::vertex(Point3::new(1.0, 0.0, 0.0));
        let circle = builder::rsweep(
            &vertex,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::unit_z(),
            truck_modeling::Rad(std::f64::consts::TAU),
        );
        let disk = builder::try_attach_plane(&[circle]).unwrap();
        let cylinder = builder::tsweep(&disk, Vector3::new(0.0, 0.0, 2.0)).compress();
        let text =
            CompleteStepDisplay::new(StepModel::from(&cylinder), StepHeaderDescriptor::default())
                .to_string();
        let scene = parse(&text).unwrap();
        assert!(scene.triangles.len() > 40);
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
        assert!((area - 6.0 * std::f64::consts::PI).abs() < 0.3);
        assert!(scene.positions().all(|p| p.x.abs() <= 1.00001
            && p.y.abs() <= 1.00001
            && p.z >= -1e-6
            && p.z <= 2.000_001));
    }

    #[test]
    fn wide_thin_cylinder_uses_boundary_geometry_to_scale_tessellation() {
        let radius = 10_000.;
        let vertex = builder::vertex(Point3::new(radius, 0., 0.));
        let circle = builder::rsweep(
            &vertex,
            Point3::origin(),
            Vector3::unit_z(),
            truck_modeling::Rad(std::f64::consts::TAU),
        );
        let disk = builder::try_attach_plane(&[circle]).unwrap();
        let cylinder = builder::tsweep(&disk, Vector3::unit_z()).compress();
        let text =
            CompleteStepDisplay::new(StepModel::from(&cylinder), StepHeaderDescriptor::default())
                .to_string();
        let scene = parse(&text).unwrap();
        assert!(scene.triangles.len() < 2000);
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
        let expected = std::f64::consts::TAU * radius * (radius + 1.);
        assert!((area - expected).abs() / expected < 0.03);
    }

    fn vertex_loop_step(surface: &str, vertex: [f64; 3]) -> String {
        format!(
            "ISO-10303-21; HEADER; FILE_DESCRIPTION(('analytic surface'),'2;1'); FILE_SCHEMA(('AUTOMOTIVE_DESIGN')); ENDSEC; DATA;\n#1=CLOSED_SHELL('',(#2));\n#2=ADVANCED_FACE('',(#3),#7,.T.);\n#3=FACE_BOUND('',#4,.T.);\n#4=VERTEX_LOOP('',#5);\n#5=VERTEX_POINT('',#6);\n#6=CARTESIAN_POINT('',({}, {}, {}));\n#7={surface};\n#8=AXIS2_PLACEMENT_3D('',#9,#10,#11);\n#9=CARTESIAN_POINT('',(1.,2.,3.));\n#10=DIRECTION('',(0.,0.,-1.));\n#11=DIRECTION('',(0.,1.,0.));\nENDSEC; END-ISO-10303-21;",
            vertex[0], vertex[1], vertex[2]
        )
    }

    #[test]
    fn sphere_and_torus_vertex_loops_preserve_surface_and_placement() {
        for (surface, vertex, area, bounds) in [
            (
                "SPHERICAL_SURFACE('',#8,5.)",
                [1.0, 2.0, 8.0],
                100.0 * std::f64::consts::PI,
                [(-4.0, 6.0), (-3.0, 7.0), (-2.0, 8.0)],
            ),
            (
                "TOROIDAL_SURFACE('',#8,3.,1.)",
                [1.0, 6.0, 3.0],
                12.0 * std::f64::consts::PI.powi(2),
                [(-3.0, 5.0), (-2.0, 6.0), (2.0, 4.0)],
            ),
        ] {
            let scene = parse(&vertex_loop_step(surface, vertex)).unwrap();
            assert!(scene.triangles.len() >= 4000);
            assert!(scene.points.is_empty());
            let observed_area: f64 = scene
                .triangles
                .iter()
                .map(|t| {
                    (t.vertices[1] - t.vertices[0])
                        .cross(t.vertices[2] - t.vertices[0])
                        .length()
                        * 0.5
                })
                .sum();
            assert!((observed_area - area).abs() / area < 0.005);
            for (axis, (expected_min, expected_max)) in bounds.into_iter().enumerate() {
                let values: Vec<_> = scene.positions().map(|p| [p.x, p.y, p.z][axis]).collect();
                let observed_min = values.iter().copied().fold(f64::INFINITY, f64::min);
                let observed_max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                assert!((observed_min - expected_min).abs() < 1e-8);
                assert!((observed_max - expected_max).abs() < 1e-8);
            }
        }
        assert!(
            parse(&vertex_loop_step(
                "TOROIDAL_SURFACE('',#8,1.,2.)",
                [1.0, 5.0, 3.0]
            ))
            .is_err()
        );
        assert!(
            parse(&vertex_loop_step(
                "SPHERICAL_SURFACE('',#8,5.)",
                [1.0, 2.0, 3.0]
            ))
            .is_err()
        );
    }

    #[test]
    fn cyclic_and_deep_references_are_rejected_without_recursing_forever() {
        let mut graph = HashMap::from([(1, vec![2]), (2, vec![1])]);
        assert!(check_graph(1, &graph, &mut HashSet::new(), &mut HashMap::new(), 0).is_err());
        graph = (0..100)
            .map(|id| (id, if id == 99 { vec![] } else { vec![id + 1] }))
            .collect();
        assert!(check_graph(0, &graph, &mut HashSet::new(), &mut HashMap::new(), 0).is_err());
        assert!(check_nesting(&format!("{}{}", "(".repeat(100), ")".repeat(100))).is_err());
    }
}
