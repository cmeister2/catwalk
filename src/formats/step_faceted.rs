//! Selected planar STEP polygon topology decoded from unchanged Part 21 records.
//!
//! The verified family uses `FACETED_BREP`, planar `FACE_SURFACE`, and `POLY_LOOP`
//! with Cartesian-point references. Attribute inheritance follows the merged EXPRESS
//! declarations at <https://www.steptools.com/stds/stp_aim/html/t_faceted_brep.html>.
//! Product selection, units and occurrence placements belong to the parent importer.

use super::step_budget::Budget;
use crate::Result;
use crate::scene::{Scene, Triangle, TriangleAppearance, Vec3};
use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;
use truck_stepio::r#in::ruststep::ast::{EntityInstance, Name, Parameter, Record};

/// Maximum polygon vertices accumulated in one selected planar face.
const MAX_FACE_VERTICES: usize = 4096;
/// Maximum container references or shallow topology-classification operations.
const MAX_ITEMS: usize = 250_000;
/// Maximum nested oriented faces, consistent with the STEP dependency depth.
const MAX_DEPTH: usize = 64;

/// Concrete schema layouts used by the owned planar topology reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A solid naming a closed outer shell.
    Solid,
    /// A nonoriented, connected open or closed face set.
    Shell,
    /// A planar face with its authored surface orientation.
    Face,
    /// A face occurrence reversing its underlying face when requested.
    OrientedFace,
    /// An exterior or interior boundary with a loop traversal orientation.
    Bound,
    /// An implicitly closed polygon of Cartesian-point references.
    Loop,
    /// Three finite position components.
    Point,
    /// Three finite, nonzero direction ratios.
    Direction,
    /// A plane defined by an orthonormal three-dimensional frame.
    Plane,
    /// A frame location and optional axis/reference direction.
    Axis,
}

impl Kind {
    /// Identify the supported most-specific schema name.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "FACETED_BREP" => Self::Solid,
            "OPEN_SHELL" | "CLOSED_SHELL" => Self::Shell,
            "FACE_SURFACE" => Self::Face,
            "ORIENTED_FACE" => Self::OrientedFace,
            "FACE_OUTER_BOUND" | "FACE_BOUND" => Self::Bound,
            "POLY_LOOP" => Self::Loop,
            "CARTESIAN_POINT" => Self::Point,
            "DIRECTION" => Self::Direction,
            "PLANE" => Self::Plane,
            "AXIS2_PLACEMENT_3D" => Self::Axis,
            _ => return None,
        })
    }

    /// Expected inherited attribute count for the internal simple mapping.
    fn count(self) -> usize {
        match self {
            Self::Solid
            | Self::Shell
            | Self::Loop
            | Self::Point
            | Self::Direction
            | Self::Plane => 2,
            Self::Bound => 3,
            Self::Face | Self::OrientedFace | Self::Axis => 4,
        }
    }

    /// Whether an external complex partial record is a known empty ancestor.
    fn empty_ancestor(self, name: &str) -> bool {
        match name {
            "GEOMETRIC_REPRESENTATION_ITEM" => matches!(
                self,
                Self::Solid
                    | Self::Face
                    | Self::Loop
                    | Self::Point
                    | Self::Direction
                    | Self::Plane
                    | Self::Axis
            ),
            "TOPOLOGICAL_REPRESENTATION_ITEM" => matches!(
                self,
                Self::Shell | Self::Face | Self::OrientedFace | Self::Bound | Self::Loop
            ),
            "SOLID_MODEL" => self == Self::Solid,
            "LOOP" => self == Self::Loop,
            "POINT" => self == Self::Point,
            "SURFACE" => self == Self::Plane,
            _ => false,
        }
    }
}

/// Known attributes assembled by reference without copying large aggregates.
struct Fields<'a> {
    /// Supported most-specific schema layout.
    kind: Kind,
    /// Most-specific record name, distinguishing shell and bound subtypes.
    name: &'a str,
    /// Parameters in inherited simple-mapping order.
    values: Vec<&'a Parameter>,
}

/// Return the partial records of either Part 21 instance mapping.
fn records(entity: &EntityInstance) -> &[Record] {
    match entity {
        EntityInstance::Simple { record, .. } => std::slice::from_ref(record),
        EntityInstance::Complex { subsuper, .. } => &subsuper.0,
    }
}

/// Identify faceted records that must bypass Truck's unsupported-record formatting.
pub(super) fn handles(entity: &EntityInstance) -> bool {
    records(entity)
        .iter()
        .any(|record| record.name == "POLY_LOOP" || record.name.contains("FACETED_BREP"))
}

/// Require a typed parameter aggregate without conflating omission and empty lists.
fn list(value: &Parameter) -> Result<&[Parameter]> {
    match value {
        Parameter::List(values) => Ok(values),
        _ => Err("STEP faceted topology requires an aggregate parameter".into()),
    }
}

/// Require an ordinary numeric entity reference.
fn reference(value: &Parameter) -> Result<u64> {
    match value {
        Parameter::Ref(Name::Entity(id)) => Ok(*id),
        _ => Err("STEP faceted topology requires an entity reference".into()),
    }
}

/// Require an EXPRESS Boolean rather than unknown logical or numeric values.
fn boolean(value: &Parameter) -> Result<bool> {
    match value {
        Parameter::Enumeration(value) if value == "T" => Ok(true),
        Parameter::Enumeration(value) if value == "F" => Ok(false),
        _ => Err("STEP faceted orientation must be Boolean".into()),
    }
}

/// Convert finite numeric coordinates without accepting strings or typed measures.
#[allow(
    clippy::cast_precision_loss,
    reason = "STEP integer and real geometry components intentionally use the scene's f64 representation."
)]
fn number(value: &Parameter) -> Result<f64> {
    let value = match value {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        _ => return Err("STEP faceted vector component is not numeric".into()),
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err("STEP faceted vector component is non-finite".into())
    }
}

/// Decode exactly three finite position or direction components.
fn vector(value: &Parameter) -> Result<Vec3> {
    let [x, y, z] = list(value)? else {
        return Err("STEP faceted positions and directions require three components".into());
    };
    Ok(Vec3::new(number(x)?, number(y)?, number(z)?))
}

/// Normalize a finite nonzero direction, rejecting overflow and zero ratios.
fn normalized(value: Vec3) -> Result<Vec3> {
    let length = value.length();
    if !length.is_finite() || length <= 0. {
        return Err("STEP faceted direction is zero or non-finite".into());
    }
    Ok(value / length)
}

/// Read exactly the local attributes of a required external complex partial record.
fn partial<'a>(all: &'a [Record], name: &str, count: usize) -> Result<&'a [Parameter]> {
    let record = all
        .iter()
        .find(|record| record.name == name)
        .ok_or("STEP faceted complex instance lacks inherited attributes")?;
    let values = list(&record.parameter)?;
    if values.len() != count {
        return Err("STEP faceted complex instance has invalid local attribute count".into());
    }
    Ok(values)
}

/// Check a supported entity's simple or external-complex inherited layout.
#[allow(
    clippy::too_many_lines,
    reason = "One schema-layout gate keeps every permitted complex ancestor and inherited attribute count visible together."
)]
fn fields(entity: &EntityInstance) -> Result<Option<Fields<'_>>> {
    let all = records(entity);
    if all.iter().any(|record| {
        record.name.contains("BREP_WITH_VOIDS")
            || (record.name.contains("FACETED_BREP") && record.name != "FACETED_BREP")
    }) {
        return Err("STEP faceted void or specialized solid combinations are unsupported".into());
    }
    // FACE_BOUND is a partial ancestor of FACE_OUTER_BOUND, not a second concrete subtype.
    let outer = all.iter().any(|record| record.name == "FACE_OUTER_BOUND");
    let mut kinds = all.iter().filter_map(|record| {
        if outer && record.name == "FACE_BOUND" {
            return None;
        }
        Kind::from_name(record.name.as_str()).map(|kind| (kind, record))
    });
    let Some((kind, concrete)) = kinds.next() else {
        return Ok(None);
    };
    if kinds.next().is_some() {
        return Err("STEP faceted complex instance has conflicting concrete subtypes".into());
    }
    let values = if matches!(entity, EntityInstance::Simple { .. }) {
        let values = list(&concrete.parameter)?;
        if values.len() != kind.count() {
            return Err("STEP faceted entity has invalid inherited attribute count".into());
        }
        values.iter().collect::<Vec<_>>()
    } else {
        let base = match kind {
            Kind::Solid => Some("MANIFOLD_SOLID_BREP"),
            Kind::Shell => Some("CONNECTED_FACE_SET"),
            Kind::Face | Kind::OrientedFace => Some("FACE"),
            Kind::Bound if outer => Some("FACE_BOUND"),
            Kind::Plane => Some("ELEMENTARY_SURFACE"),
            Kind::Axis => Some("PLACEMENT"),
            _ => None,
        };
        let mut seen = HashSet::new();
        for record in all {
            let name = record.name.as_str();
            if !seen.insert(name) {
                return Err("STEP faceted complex instance repeats a partial record".into());
            }
            if name != concrete.name.as_str()
                && name != "REPRESENTATION_ITEM"
                && Some(name) != base
                && !kind.empty_ancestor(name)
            {
                return Err("unsupported STEP faceted external-complex subtype".into());
            }
            if kind.empty_ancestor(name) && !list(&record.parameter)?.is_empty() {
                return Err("STEP faceted complex ancestor has unexpected attributes".into());
            }
        }
        let mut values = vec![&partial(all, "REPRESENTATION_ITEM", 1)?[0]];
        if let Some(base) = base {
            let count = if kind == Kind::Bound { 2 } else { 1 };
            values.extend(partial(all, base, count)?);
        }
        let local_count = kind.count() - values.len();
        values.extend(partial(all, concrete.name.as_str(), local_count)?);
        values
    };
    if values.len() != kind.count() || !matches!(values[0], Parameter::String(_)) {
        return Err("STEP faceted entity has invalid inherited name or attributes".into());
    }
    Ok(Some(Fields {
        kind,
        name: concrete.name.as_str(),
        values,
    }))
}

/// Read a faceted solid's outer reference before assembly identity/deduplication.
///
/// # Errors
/// Malformed inherited attributes, a nonreference outer shell, or any supported-name
/// void/specialized combination is rejected. Target existence/type is checked by decoding.
pub(super) fn solid_outer(entity: &EntityInstance) -> Result<Option<u64>> {
    if !records(entity)
        .iter()
        .any(|record| record.name.contains("FACETED_BREP"))
    {
        return Ok(None);
    }
    let fields = fields(entity)?.ok_or("unsupported STEP faceted solid subtype")?;
    if fields.kind != Kind::Solid {
        return Err("unsupported STEP faceted solid subtype".into());
    }
    Ok(Some(reference(fields.values[1])?))
}

/// Find an aggregate attribute permissively during topology ownership classification.
fn raw_attribute<'a>(
    entity: &'a EntityInstance,
    simple: &[&str],
    base: &str,
    index: usize,
) -> Option<&'a Parameter> {
    match entity {
        EntityInstance::Simple { record, .. } if simple.contains(&record.name.as_str()) => {
            if let Parameter::List(values) = &record.parameter {
                values.get(index)
            } else {
                None
            }
        }
        EntityInstance::Complex { subsuper, .. } => subsuper
            .0
            .iter()
            .find(|record| record.name == base)
            .and_then(|record| {
                if let Parameter::List(values) = &record.parameter {
                    values.first()
                } else {
                    None
                }
            }),
        EntityInstance::Simple { .. } => None,
    }
}

/// Detect polygon-loop shell ownership without triangulating or validating unused face geometry.
///
/// # Errors
/// The shallow classifier exceeds its local reference ceiling or shared work/storage limits.
/// Missing or malformed optional paths remain unresolved here and are validated if selected.
pub(super) fn owns_shell(
    entities: &HashMap<u64, &EntityInstance>,
    id: u64,
    budget: &mut Budget,
) -> Result<bool> {
    let Some(entity) = entities.get(&id) else {
        return Ok(false);
    };
    budget.work(records(entity).len())?;
    let Some(Parameter::List(faces)) = raw_attribute(
        entity,
        &["OPEN_SHELL", "CLOSED_SHELL"],
        "CONNECTED_FACE_SET",
        1,
    ) else {
        return Ok(false);
    };
    if faces.len() > MAX_ITEMS {
        return Err("STEP polygon shell classification exceeds reference limit".into());
    }
    let mut pending = Vec::new();
    queue_references(faces, &mut pending, budget)?;
    let mut seen = HashSet::new();
    let mut work = 0usize;
    while let Some(id) = pending.pop() {
        budget.work(1)?;
        work += 1;
        if work > MAX_ITEMS {
            return Err("STEP polygon shell classification exceeds reference limit".into());
        }
        if seen.contains(&id) {
            continue;
        }
        budget.storage(1, 4 * size_of::<u64>())?;
        seen.insert(id);
        let Some(entity) = entities.get(&id) else {
            continue;
        };
        budget.work(records(entity).len())?;
        if records(entity)
            .iter()
            .any(|record| record.name == "POLY_LOOP")
        {
            return Ok(true);
        }
        let names = records(entity);
        let value = if names.iter().any(|record| record.name == "ORIENTED_FACE") {
            raw_attribute(entity, &["ORIENTED_FACE"], "ORIENTED_FACE", 2)
        } else if names.iter().any(|record| {
            matches!(
                record.name.as_str(),
                "FACE" | "FACE_SURFACE" | "ADVANCED_FACE"
            )
        }) {
            raw_attribute(
                entity,
                &["FACE", "FACE_SURFACE", "ADVANCED_FACE"],
                "FACE",
                1,
            )
        } else if names
            .iter()
            .any(|record| matches!(record.name.as_str(), "FACE_BOUND" | "FACE_OUTER_BOUND"))
        {
            raw_attribute(entity, &["FACE_BOUND", "FACE_OUTER_BOUND"], "FACE_BOUND", 1)
        } else {
            None
        };
        match value {
            Some(Parameter::List(values)) => {
                if values.len() > MAX_ITEMS.saturating_sub(work).saturating_sub(pending.len()) {
                    return Err("STEP polygon shell classification exceeds reference limit".into());
                }
                queue_references(values, &mut pending, budget)?;
            }
            Some(value) => {
                queue_references(std::slice::from_ref(value), &mut pending, budget)?;
            }
            None => {}
        }
    }
    Ok(false)
}

/// Charge aggregate traversal and pending capacity before retaining shallow numeric references.
fn queue_references(
    values: &[Parameter],
    pending: &mut Vec<u64>,
    budget: &mut Budget,
) -> Result<()> {
    budget.work(values.len())?;
    // Capacity slack and repeated temporary growth are conservatively charged cumulatively.
    budget.storage(values.len(), 2 * size_of::<u64>())?;
    pending.try_reserve_exact(values.len())?;
    pending.extend(values.iter().filter_map(|value| reference(value).ok()));
    Ok(())
}

/// An orthonormal plane frame retaining its authored surface normal.
#[derive(Clone, Copy)]
struct Plane {
    /// Origin used to validate the source supporting plane.
    origin: Vec3,
    /// Unit direction along plane-local X.
    x: Vec3,
    /// Unit direction along plane-local Y.
    y: Vec3,
    /// Unit authored axis, independent of the face's `same_sense` flag.
    normal: Vec3,
}

/// Cached unplaced face triangles with their constant authored face normal.
struct FaceMesh {
    /// Triangles oriented consistently with the face surface.
    triangles: Vec<[Vec3; 3]>,
    /// Normal of the underlying face before oriented-face occurrences.
    normal: Vec3,
}

/// Selected planar topology decoder sharing caches but never predecoding unrelated solids.
pub(super) struct Decoder<'a> {
    /// Borrowed Part 21 entity records indexed by numeric identity.
    entities: &'a HashMap<u64, &'a EntityInstance>,
    /// Finite Cartesian positions cached by their actual schema entity identity.
    points: HashMap<u64, Vec3>,
    /// Normalized authored direction ratios cached separately from point positions.
    directions: HashMap<u64, Vec3>,
    /// Shared triangulated underlying faces reused by shell and orientation occurrences.
    faces: HashMap<u64, Arc<FaceMesh>>,
}

impl<'a> Decoder<'a> {
    /// Construct an empty selected-only decoder without allocating source geometry.
    pub(super) fn new(entities: &'a HashMap<u64, &'a EntityInstance>) -> Self {
        Self {
            entities,
            points: HashMap::new(),
            directions: HashMap::new(),
            faces: HashMap::new(),
        }
    }

    /// Require the selected target and flatten its verified inherited schema attributes.
    fn target(&self, id: u64, kind: Kind) -> Result<Fields<'a>> {
        let entity = self
            .entities
            .get(&id)
            .ok_or_else(|| format!("STEP faceted geometry references missing entity #{id}"))?;
        let fields =
            fields(entity)?.ok_or("STEP faceted geometry references an unsupported schema type")?;
        if fields.kind != kind {
            return Err("STEP faceted geometry reference has the wrong schema type".into());
        }
        Ok(fields)
    }

    /// Decode one position or direction and charge storage before growing its cache.
    fn coordinate(&mut self, id: u64, direction: bool, budget: &mut Budget) -> Result<Vec3> {
        let cached = if direction {
            self.directions.get(&id)
        } else {
            self.points.get(&id)
        };
        if let Some(&value) = cached {
            return Ok(value);
        }
        let fields = self.target(
            id,
            if direction {
                Kind::Direction
            } else {
                Kind::Point
            },
        )?;
        budget.vectors(1)?;
        budget.storage(1, 4 * size_of::<(u64, Vec3)>())?;
        let value = vector(fields.values[1])?;
        let value = if direction { normalized(value)? } else { value };
        if direction {
            self.directions.insert(id, value);
        } else {
            self.points.insert(id, value);
        }
        Ok(value)
    }

    /// Decode a supporting plane and project the optional reference direction onto its axis.
    fn plane(&mut self, id: u64, budget: &mut Budget) -> Result<Plane> {
        let fields = self.target(id, Kind::Plane)?;
        let axis = self.target(reference(fields.values[1])?, Kind::Axis)?;
        budget.work(2)?;
        let origin = self.coordinate(reference(axis.values[1])?, false, budget)?;
        let normal = if matches!(axis.values[2], Parameter::NotProvided) {
            Vec3::new(0., 0., 1.)
        } else {
            self.coordinate(reference(axis.values[2])?, true, budget)?
        };
        let reference = if matches!(axis.values[3], Parameter::NotProvided) {
            if normal.cross(Vec3::new(1., 0., 0.)).length() > 1e-12 {
                Vec3::new(1., 0., 0.)
            } else {
                Vec3::new(0., 1., 0.)
            }
        } else {
            self.coordinate(reference(axis.values[3])?, true, budget)?
        };
        let x = normalized(reference - normal * reference.dot(normal))?;
        let y = normalized(normal.cross(x))?;
        Ok(Plane {
            origin,
            x,
            y,
            normal,
        })
    }

    /// Decode a complete unique Cartesian-point polygon with its explicit bound traversal.
    fn ring(&mut self, bound: &Fields<'_>, budget: &mut Budget) -> Result<Vec<Vec3>> {
        let polygon = self.target(reference(bound.values[1])?, Kind::Loop)?;
        let refs = list(polygon.values[1])?;
        if refs.len() < 3 || refs.len() > MAX_FACE_VERTICES {
            return Err("STEP POLY_LOOP requires 3..4096 unique Cartesian points".into());
        }
        budget.indices(refs.len())?;
        budget.storage(refs.len(), 4 * size_of::<u64>() + size_of::<Vec3>())?;
        let mut seen = HashSet::new();
        let mut points = Vec::new();
        points.try_reserve_exact(refs.len())?;
        for value in refs {
            let id = reference(value)?;
            if !seen.insert(id) {
                return Err("STEP POLY_LOOP repeats a Cartesian-point reference".into());
            }
            points.push(self.coordinate(id, false, budget)?);
        }
        if !boolean(bound.values[2])? {
            points.reverse();
        }
        Ok(points)
    }

    /// Triangulate one underlying face once, preserving its supporting plane and hole boundaries.
    fn face(&mut self, id: u64, budget: &mut Budget) -> Result<Arc<FaceMesh>> {
        if let Some(face) = self.faces.get(&id) {
            return Ok(Arc::clone(face));
        }
        let fields = self.target(id, Kind::Face)?;
        let bounds = list(fields.values[1])?;
        if bounds.is_empty() || bounds.len() > MAX_FACE_VERTICES / 3 {
            return Err("STEP faceted face has an empty or excessive bound set".into());
        }
        budget.faces(1)?;
        budget.indices(bounds.len())?;
        budget.storage(bounds.len(), size_of::<Vec<Vec3>>() + 4 * size_of::<u64>())?;
        let plane = self.plane(reference(fields.values[2])?, budget)?;
        let normal = if boolean(fields.values[3])? {
            plane.normal
        } else {
            -plane.normal
        };
        let mut seen = HashSet::new();
        let mut rings = Vec::new();
        let mut outer = None;
        let mut vertices = 0usize;
        for value in bounds {
            let id = reference(value)?;
            if !seen.insert(id) {
                return Err("STEP faceted face repeats a bound reference".into());
            }
            let bound = self.target(id, Kind::Bound)?;
            let ring = self.ring(&bound, budget)?;
            vertices = vertices
                .checked_add(ring.len())
                .ok_or("STEP polygon vertex count overflows")?;
            if vertices > MAX_FACE_VERTICES {
                return Err("STEP faceted face exceeds 4096 polygon vertices".into());
            }
            if bound.name == "FACE_OUTER_BOUND" && outer.replace(rings.len()).is_some() {
                return Err("STEP faceted face has more than one exterior bound".into());
            }
            rings.push(ring);
        }
        let exterior = outer.ok_or("STEP faceted face lacks an explicit exterior bound")?;
        rings.swap(0, exterior);
        let mesh = Arc::new(triangulate(&rings, plane, normal, budget)?);
        budget.storage(
            1,
            size_of::<FaceMesh>() + 4 * size_of::<(u64, Arc<FaceMesh>)>(),
        )?;
        self.faces.insert(id, Arc::clone(&mesh));
        Ok(mesh)
    }

    /// Resolve nested face orientation wrappers while guarding cycles and depth.
    fn oriented_face(&mut self, mut id: u64, budget: &mut Budget) -> Result<(Arc<FaceMesh>, bool)> {
        let mut reversed = false;
        let mut seen = HashSet::new();
        for _ in 0..=MAX_DEPTH {
            budget.work(1)?;
            if !seen.insert(id) {
                return Err("STEP oriented face contains a reference cycle".into());
            }
            let entity = self
                .entities
                .get(&id)
                .ok_or("STEP shell references a missing face")?;
            let fields = fields(entity)?
                .ok_or("STEP polygon shell contains unsupported required face geometry")?;
            if fields.kind == Kind::Face {
                return Ok((self.face(id, budget)?, reversed));
            }
            if fields.kind != Kind::OrientedFace {
                return Err(
                    "STEP polygon shell contains unsupported required face geometry".into(),
                );
            }
            if !matches!(fields.values[1], Parameter::Omitted) {
                return Err("STEP ORIENTED_FACE must derive its bounds with *".into());
            }
            reversed ^= !boolean(fields.values[3])?;
            id = reference(fields.values[2])?;
        }
        Err("STEP oriented face exceeds reference depth limit".into())
    }

    /// Decode a selected faceted solid or polygon shell into an unplaced scene.
    ///
    /// # Errors
    /// Required topology is missing, mixed, unsupported or malformed; ring/plane geometry
    /// violates the verified planar contract; orientation, triangulation or shared limits fail.
    /// Unrelated item types return `None` without decoding their source geometry.
    pub(super) fn decode(&mut self, id: u64, budget: &mut Budget) -> Result<Option<Scene>> {
        let entity = self
            .entities
            .get(&id)
            .ok_or("STEP geometry references a missing item")?;
        let facet = records(entity)
            .iter()
            .any(|record| record.name.contains("FACETED_BREP"));
        let shell = records(entity)
            .iter()
            .any(|record| matches!(record.name.as_str(), "OPEN_SHELL" | "CLOSED_SHELL"));
        if !facet && (!shell || !owns_shell(self.entities, id, budget)?) {
            return Ok(None);
        }
        let id = if facet {
            let outer = solid_outer(entity)?.ok_or("unsupported STEP faceted solid subtype")?;
            let fields = self.target(outer, Kind::Shell)?;
            if fields.name != "CLOSED_SHELL" {
                return Err("STEP FACETED_BREP outer must be a nonoriented CLOSED_SHELL".into());
            }
            outer
        } else {
            id
        };
        let fields = self.target(id, Kind::Shell)?;
        let refs = list(fields.values[1])?;
        if refs.is_empty() || refs.len() > MAX_ITEMS {
            return Err("STEP polygon shell has an empty or excessive face set".into());
        }
        budget.indices(refs.len())?;
        budget.storage(refs.len(), 4 * size_of::<u64>())?;
        let mut seen = HashSet::new();
        let mut scene = Scene::default();
        for value in refs {
            let id = reference(value)?;
            if !seen.insert(id) {
                return Err("STEP polygon shell repeats a face reference".into());
            }
            let (face, reversed) = self.oriented_face(id, budget)?;
            budget.triangles(face.triangles.len(), true)?;
            scene.triangles.try_reserve_exact(face.triangles.len())?;
            let normal = if reversed { -face.normal } else { face.normal };
            for &vertices in &face.triangles {
                let mut vertices = vertices;
                if reversed {
                    vertices.swap(1, 2);
                }
                scene.appearances.insert(
                    scene.triangles.len(),
                    TriangleAppearance {
                        normals: Some([normal; 3]),
                        ..TriangleAppearance::default()
                    },
                );
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
        }
        Ok(Some(scene))
    }
}

/// Signed double area of one projected ring, anchored to limit cancellation.
fn signed_area(points: &[[f64; 2]]) -> f64 {
    let origin = points[0];
    (0..points.len())
        .map(|i| {
            let a = [points[i][0] - origin[0], points[i][1] - origin[1]];
            let b = [
                points[(i + 1) % points.len()][0] - origin[0],
                points[(i + 1) % points.len()][1] - origin[1],
            ];
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f64>()
        / 2.
}

/// Signed planar cross product of two vectors originating at a.
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Test collinear point containment using the same scale-relative ring tolerance.
fn on_segment(a: [f64; 2], b: [f64; 2], point: [f64; 2], distance: f64, area: f64) -> bool {
    orient(a, b, point).abs() <= area
        && point[0] >= a[0].min(b[0]) - distance
        && point[0] <= a[0].max(b[0]) + distance
        && point[1] >= a[1].min(b[1]) - distance
        && point[1] <= a[1].max(b[1]) + distance
}

/// Detect proper crossings and touching/overlapping projected polygon segments.
fn intersects(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
    distance: f64,
    area: f64,
) -> bool {
    let ab_c = orient(a, b, c);
    let ab_d = orient(a, b, d);
    let cd_a = orient(c, d, a);
    let cd_b = orient(c, d, b);
    (((ab_c > area && ab_d < -area) || (ab_c < -area && ab_d > area))
        && ((cd_a > area && cd_b < -area) || (cd_a < -area && cd_b > area)))
        || on_segment(a, b, c, distance, area)
        || on_segment(a, b, d, distance, area)
        || on_segment(c, d, a, distance, area)
        || on_segment(c, d, b, distance, area)
}

/// Test strict polygon inclusion after a separate crossing/touching check.
fn inside(points: &[[f64; 2]], point: [f64; 2]) -> bool {
    let mut inside = false;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        if (a[1] > point[1]) != (b[1] > point[1])
            && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

/// Reject degenerate, self-intersecting and touching edges within one ring.
fn simple_ring(points: &[[f64; 2]], distance: f64, area: f64) -> Result<()> {
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        if (a[0] - b[0]).hypot(a[1] - b[1]) <= distance {
            return Err("STEP polygon has a zero-length edge".into());
        }
        let c = points[(i + 2) % points.len()];
        if orient(a, b, c).abs() <= area
            && (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]) < 0.
        {
            return Err("STEP polygon has overlapping adjacent edges".into());
        }
        for j in i + 1..points.len() {
            if j == i + 1 || (i == 0 && j + 1 == points.len()) {
                continue;
            }
            if intersects(
                a,
                b,
                points[j],
                points[(j + 1) % points.len()],
                distance,
                area,
            ) {
                return Err("STEP polygon ring crosses or touches itself".into());
            }
        }
    }
    Ok(())
}

/// Validate disjoint interior rings and their strict containment within one exterior.
fn holes(rings: &[Vec<[f64; 2]>], distance: f64, area: f64) -> Result<()> {
    for first in 0..rings.len() {
        for second in first + 1..rings.len() {
            for i in 0..rings[first].len() {
                for j in 0..rings[second].len() {
                    if intersects(
                        rings[first][i],
                        rings[first][(i + 1) % rings[first].len()],
                        rings[second][j],
                        rings[second][(j + 1) % rings[second].len()],
                        distance,
                        area,
                    ) {
                        return Err("STEP polygon bounds cross or touch".into());
                    }
                }
            }
            if first == 0 {
                if !inside(&rings[0], rings[second][0]) {
                    return Err("STEP polygon hole lies outside its exterior".into());
                }
            } else if inside(&rings[first], rings[second][0])
                || inside(&rings[second], rings[first][0])
            {
                return Err("STEP polygon holes overlap or contain one another".into());
            }
        }
    }
    Ok(())
}

/// Project and validate a bounded planar polygon before earcutting its exterior and holes.
#[allow(
    clippy::too_many_lines,
    reason = "Planarity, ring topology, resource preflight and triangle conservation form one checked polygon conversion."
)]
fn triangulate(
    rings: &[Vec<Vec3>],
    plane: Plane,
    normal: Vec3,
    budget: &mut Budget,
) -> Result<FaceMesh> {
    let total = rings.iter().map(Vec::len).sum::<usize>();
    budget.work(
        total
            .checked_mul(total)
            .and_then(|n| n.checked_mul(2))
            .ok_or("STEP polygon validation work overflows")?,
    )?;
    // Earcut keeps node links and index scratch; account a conservative per-vertex estimate.
    budget.storage(
        total,
        256 + size_of::<Vec3>() + 2 * size_of::<f64>() + size_of::<usize>(),
    )?;
    let origin = rings[0][0];
    let mut scale = 0_f64;
    let mut magnitude = 0_f64;
    let mut positions = Vec::new();
    let mut flat = Vec::new();
    let mut projected = Vec::new();
    let mut offsets = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        if i != 0 {
            offsets.push(positions.len());
        }
        let mut points = Vec::new();
        for &point in ring {
            let local = point - origin;
            let value = [local.dot(plane.x), local.dot(plane.y)];
            if !local.is_finite() || value.iter().any(|n| !n.is_finite()) {
                return Err("STEP polygon projection overflows".into());
            }
            scale = scale.max(local.length());
            magnitude = magnitude
                .max(point.x.abs())
                .max(point.y.abs())
                .max(point.z.abs());
            points.push(value);
            flat.extend_from_slice(&value);
            positions.push(point);
        }
        projected.push(points);
    }
    if !scale.is_finite() || scale <= 0. {
        return Err("STEP polygon is degenerate".into());
    }
    let distance = scale * 1e-12;
    let area_tolerance = scale * distance;
    if !area_tolerance.is_finite() || area_tolerance <= 0. {
        return Err("STEP polygon scale is outside representable precision".into());
    }
    let plane_tolerance = scale * 1e-8 + magnitude * (8. * f64::EPSILON);
    for &point in &positions {
        let distance = (point - plane.origin).dot(plane.normal);
        if !distance.is_finite() || distance.abs() > plane_tolerance {
            return Err("STEP POLY_LOOP is not on its supporting plane".into());
        }
    }
    let sense = normal.dot(plane.normal);
    let mut expected_area = 0.;
    for (i, ring) in projected.iter().enumerate() {
        simple_ring(ring, distance, area_tolerance)?;
        let area = signed_area(ring);
        if !area.is_finite() || area.abs() <= area_tolerance {
            return Err("STEP polygon ring has zero or non-finite area".into());
        }
        if (i == 0 && area * sense <= 0.) || (i != 0 && area * sense >= 0.) {
            return Err("STEP polygon bound winding disagrees with its face orientation".into());
        }
        expected_area += if i == 0 { area.abs() } else { -area.abs() };
    }
    if !expected_area.is_finite() || expected_area <= area_tolerance {
        return Err("STEP polygon holes consume its exterior".into());
    }
    holes(&projected, distance, area_tolerance)?;
    let triangle_limit = total
        .checked_add(2 * offsets.len())
        .and_then(|n| n.checked_sub(2))
        .ok_or("STEP polygon triangle bound overflows")?;
    budget.storage(triangle_limit, size_of::<[Vec3; 3]>())?;
    budget.indices(
        triangle_limit
            .checked_mul(3)
            .ok_or("STEP polygon triangle indices overflow")?,
    )?;
    let indices = earcutr::earcut(&flat, &offsets, 2)
        .map_err(|error| format!("STEP polygon triangulation failed: {error}"))?;
    let (triples, remainder) = indices.as_chunks::<3>();
    if !remainder.is_empty() || triples.is_empty() || triples.len() > triangle_limit {
        return Err("STEP polygon triangulation has invalid triangle count".into());
    }
    let mut triangles = Vec::new();
    triangles.try_reserve_exact(triples.len())?;
    let mut actual_area = 0.;
    for &[a, b, c] in triples {
        let mut vertices = [
            *positions.get(a).ok_or("STEP earcut index is invalid")?,
            *positions.get(b).ok_or("STEP earcut index is invalid")?,
            *positions.get(c).ok_or("STEP earcut index is invalid")?,
        ];
        let cross = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]);
        let alignment = cross.dot(normal);
        if !alignment.is_finite() {
            return Err("STEP polygon triangle geometry overflows".into());
        }
        if alignment < 0. {
            vertices.swap(1, 2);
        }
        let area = alignment.abs() / 2.;
        if area <= area_tolerance {
            return Err("STEP polygon triangulation produced a degenerate triangle".into());
        }
        actual_area += area;
        triangles.push(vertices);
    }
    if !actual_area.is_finite() || (actual_area - expected_area).abs() > expected_area * 1e-7 {
        return Err("STEP polygon triangulation did not preserve its exterior and holes".into());
    }
    Ok(FaceMesh { triangles, normal })
}

#[cfg(test)]
#[path = "step_faceted_tests.rs"]
mod tests;
