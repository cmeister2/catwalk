//! Selected AP242 tessellated surfaces decoded directly from the Part 21 AST.
//!
//! Attribute layouts and point/normal indirection follow sections 5.1, 5.3 and
//! 5.4 of the CAx-IF 3D Tessellated Geometry recommendations, version 1.1:
//! <https://www.mbx-if.org/home/wp-content/uploads/2024/05/rec_prac_3dtess_geo_v11.pdf>.
//! Assembly selection, units and placements remain the STEP importer's responsibility.

use super::step_budget::{Budget, MAX_VECTORS};
use crate::Result;
use crate::scene::{Scene, Triangle, TriangleAppearance, Vec3};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use truck_stepio::r#in::ruststep::ast::{EntityInstance, Name, Parameter, Record};

/// Maximum structured-item references in one selected shell or solid.
const MAX_ITEMS: usize = 100_000;

/// Concrete triangle-bearing schema layouts and shared coordinate records.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A named counted list of three-dimensional points.
    Coordinates,
    /// Independent triangles describing a structured face.
    Face,
    /// Triangle strips and fans describing a structured face.
    ComplexFace,
    /// Independent triangles without an analytic face link.
    Surface,
    /// Triangle strips and fans without an analytic face link.
    ComplexSurface,
    /// A structured surface item set with an optional exact-shell link.
    Shell,
    /// A structured surface item set with an optional exact-solid link.
    Solid,
}

impl Kind {
    /// Recognize only layouts whose inherited attributes are decoded here.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "COORDINATES_LIST" => Self::Coordinates,
            "TRIANGULATED_FACE" => Self::Face,
            "COMPLEX_TRIANGULATED_FACE" => Self::ComplexFace,
            "TRIANGULATED_SURFACE_SET" => Self::Surface,
            "COMPLEX_TRIANGULATED_SURFACE_SET" => Self::ComplexSurface,
            "TESSELLATED_SHELL" => Self::Shell,
            "TESSELLATED_SOLID" => Self::Solid,
            _ => return None,
        })
    }

    /// Whether this entity stores strips and fans instead of independent triples.
    fn complex_triangles(self) -> bool {
        matches!(self, Self::ComplexFace | Self::ComplexSurface)
    }

    /// Whether the inherited face fields include an optional exact-geometry link.
    fn linked_face(self) -> bool {
        matches!(self, Self::Face | Self::ComplexFace)
    }

    /// Whether this entity directly supplies a triangle surface.
    fn surface(self) -> bool {
        matches!(
            self,
            Self::Face | Self::ComplexFace | Self::Surface | Self::ComplexSurface
        )
    }
}

/// Inherited simple-record fields assembled without copying their aggregate values.
struct Fields<'a> {
    /// Recognized concrete schema subtype.
    kind: Kind,
    /// Parameters in the subtype's simple-record attribute order.
    values: Vec<&'a Parameter>,
}

/// Return a simple record or the partial records of a complex entity instance.
fn records(entity: &EntityInstance) -> &[Record] {
    match entity {
        EntityInstance::Simple { record, .. } => std::slice::from_ref(record),
        EntityInstance::Complex { subsuper, .. } => &subsuper.0,
    }
}

/// Identify tessellated schema records that must bypass Truck's large dummy formatting.
pub(super) fn handles(entity: &EntityInstance) -> bool {
    records(entity).iter().any(|record| {
        record.name == "COORDINATES_LIST"
            || record.name.contains("TESSELLATED")
            || record.name.contains("TRIANGULATED")
    })
}

/// Require a typed aggregate without treating omitted or derived attributes as empty lists.
fn list(value: &Parameter) -> Result<&[Parameter]> {
    match value {
        Parameter::List(values) => Ok(values),
        _ => Err("AP242 tessellation requires an aggregate parameter".into()),
    }
}

/// Read a positive schema integer without rounding real-valued indices or counts.
fn positive(value: &Parameter) -> Result<usize> {
    match value {
        Parameter::Integer(value) if *value > 0 => Ok(usize::try_from(*value)?),
        _ => Err("AP242 tessellation requires a positive integer count or index".into()),
    }
}

/// Require a numeric entity identifier rather than a value or named reference.
fn reference(value: &Parameter) -> Result<u64> {
    match value {
        Parameter::Ref(Name::Entity(id)) => Ok(*id),
        _ => Err("AP242 tessellation requires an entity reference".into()),
    }
}

/// Convert finite STEP numeric vector components to the scene's double precision.
#[allow(
    clippy::cast_precision_loss,
    reason = "STEP integer coordinates intentionally share the scene's f64 representation with real coordinate parameters."
)]
fn number(value: &Parameter) -> Result<f64> {
    let value = match value {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        _ => return Err("AP242 coordinate or normal component is not numeric".into()),
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err("AP242 coordinate or normal component is not finite".into())
    }
}

/// Decode exactly three finite vector components.
fn vector(value: &Parameter) -> Result<Vec3> {
    let values = list(value)?;
    let [x, y, z] = values else {
        return Err("AP242 coordinates and normals require exactly three components".into());
    };
    Ok(Vec3::new(number(x)?, number(y)?, number(z)?))
}

/// Validate optional exact-geometry links without decoding or retessellating their targets.
fn link(value: &Parameter, entities: &HashMap<u64, &EntityInstance>, kind: Kind) -> Result<()> {
    if matches!(value, Parameter::NotProvided) {
        return Ok(());
    }
    let id = reference(value)?;
    let entity = entities
        .get(&id)
        .ok_or_else(|| format!("AP242 exact-geometry link references missing entity #{id}"))?;
    let valid = records(entity).iter().any(|record| match kind {
        Kind::Solid => matches!(
            record.name.as_str(),
            "MANIFOLD_SOLID_BREP" | "FACETED_BREP" | "BREP_WITH_VOIDS"
        ),
        Kind::Shell => matches!(
            record.name.as_str(),
            "CONNECTED_FACE_SET" | "OPEN_SHELL" | "CLOSED_SHELL"
        ),
        Kind::Face | Kind::ComplexFace => matches!(
            record.name.as_str(),
            "FACE"
                | "FACE_SURFACE"
                | "ADVANCED_FACE"
                | "PLANE"
                | "CYLINDRICAL_SURFACE"
                | "CONICAL_SURFACE"
                | "SPHERICAL_SURFACE"
                | "TOROIDAL_SURFACE"
                | "DEGENERATE_TOROIDAL_SURFACE"
                | "SURFACE_OF_LINEAR_EXTRUSION"
                | "SURFACE_OF_REVOLUTION"
                | "B_SPLINE_SURFACE"
                | "B_SPLINE_SURFACE_WITH_KNOTS"
                | "RATIONAL_B_SPLINE_SURFACE"
                | "BEZIER_SURFACE"
                | "UNIFORM_SURFACE"
                | "QUASI_UNIFORM_SURFACE"
                | "RECTANGULAR_TRIMMED_SURFACE"
                | "CURVE_BOUNDED_SURFACE"
                | "OFFSET_SURFACE"
        ),
        _ => false,
    });
    if valid {
        Ok(())
    } else {
        Err("AP242 exact-geometry link has an unsupported or invalid target type".into())
    }
}

/// Assemble known simple or complex-instance attributes and reject unknown inherited behavior.
fn fields(entity: &EntityInstance) -> Result<Option<Fields<'_>>> {
    let all = records(entity);
    let mut concrete = all
        .iter()
        .filter_map(|record| Kind::from_name(&record.name).map(|kind| (kind, record)));
    let Some((kind, record)) = concrete.next() else {
        return Ok(None);
    };
    if concrete.next().is_some() {
        return Err("AP242 complex instance has conflicting tessellated subtypes".into());
    }
    let expected = match kind {
        Kind::Coordinates | Kind::Shell | Kind::Solid => 3,
        Kind::Face | Kind::ComplexSurface => 7,
        Kind::ComplexFace => 8,
        Kind::Surface => 6,
    };
    let values = if matches!(entity, EntityInstance::Simple { .. }) {
        let values = list(&record.parameter)?;
        if values.len() != expected {
            return Err("AP242 tessellated entity has invalid inherited attribute layout".into());
        }
        values.iter().collect()
    } else {
        complex_fields(all, kind, record)?
    };
    if values.len() != expected || !matches!(values[0], Parameter::String(_)) {
        return Err("AP242 tessellated entity has invalid inherited attribute layout".into());
    }
    Ok(Some(Fields { kind, values }))
}

/// Read a tessellated shell or solid's optional exact-body link from either Part 21 mapping.
///
/// # Errors
/// Returns an error for malformed inherited attributes or a provided link with
/// a missing, unsupported or incorrect schema target. Validation precedes any
/// representation deduplication so an invalid link cannot suppress geometry.
pub(super) fn exact_body_link(
    entity: &EntityInstance,
    entities: &HashMap<u64, &EntityInstance>,
) -> Result<Option<u64>> {
    let Some(fields) = fields(entity)? else {
        return Ok(None);
    };
    if !matches!(fields.kind, Kind::Shell | Kind::Solid)
        || matches!(fields.values[2], Parameter::NotProvided)
    {
        return Ok(None);
    }
    link(fields.values[2], entities, fields.kind)?;
    Ok(Some(reference(fields.values[2])?))
}

/// Assemble the local attributes of a supported external complex mapping.
fn complex_fields<'a>(
    records: &'a [Record],
    kind: Kind,
    concrete: &'a Record,
) -> Result<Vec<&'a Parameter>> {
    let base = match kind {
        Kind::Face | Kind::ComplexFace => Some("TESSELLATED_FACE"),
        Kind::Surface | Kind::ComplexSurface => Some("TESSELLATED_SURFACE_SET"),
        _ => None,
    };
    let mut seen = HashSet::new();
    for record in records {
        if !seen.insert(record.name.as_str()) {
            return Err("AP242 complex instance repeats a partial entity".into());
        }
        let known_empty = matches!(
            record.name.as_str(),
            "GEOMETRIC_REPRESENTATION_ITEM" | "TESSELLATED_ITEM" | "TESSELLATED_STRUCTURED_ITEM"
        );
        if record.name != concrete.name
            && record.name != "REPRESENTATION_ITEM"
            && Some(record.name.as_str()) != base
            && !known_empty
        {
            return Err("unsupported AP242 tessellated complex-instance subtype".into());
        }
        if known_empty && !list(&record.parameter)?.is_empty() {
            return Err("AP242 complex-instance supertype has unexpected attributes".into());
        }
    }
    let name = records
        .iter()
        .find(|record| record.name == "REPRESENTATION_ITEM")
        .ok_or("AP242 complex instance lacks its representation-item name")?;
    let attributes = list(&name.parameter)?;
    if attributes.len() != 1 {
        return Err("AP242 complex instance has invalid representation-item attributes".into());
    }
    let mut values = vec![&attributes[0]];
    if let Some(base) = base {
        let record = records
            .iter()
            .find(|record| record.name == base)
            .ok_or("AP242 complex instance lacks inherited tessellated surface attributes")?;
        let attributes = list(&record.parameter)?;
        if attributes.len() != if kind.linked_face() { 4 } else { 3 } {
            return Err("AP242 complex instance has invalid inherited surface attributes".into());
        }
        values.extend(attributes);
    }
    let attributes = list(&concrete.parameter)?;
    let expected = match kind {
        Kind::Coordinates | Kind::Shell | Kind::Solid | Kind::Face | Kind::Surface => 2,
        Kind::ComplexFace | Kind::ComplexSurface => 3,
    };
    if attributes.len() != expected {
        return Err("AP242 complex instance has invalid concrete attributes".into());
    }
    values.extend(attributes);
    Ok(values)
}

/// Validated point/normal indirection for a single selected tessellated surface.
struct Face {
    /// Cached coordinates shared across faces.
    coordinates: Arc<Vec<Vec3>>,
    /// Point indices for each point/normal pair; empty means direct indexing.
    mapping: Vec<usize>,
    /// Zero, one or one-per-pair normalized surface normals.
    normals: Vec<Vec3>,
    /// Number of valid one-based primitive point/normal indices.
    pnmax: usize,
}

impl Face {
    /// Read a one-based primitive index and its coordinate and optional corner normal.
    fn corner(&self, value: &Parameter) -> Result<(Vec3, Option<Vec3>)> {
        let pair = positive(value)? - 1;
        if pair >= self.pnmax {
            return Err("AP242 triangle index exceeds pnmax".into());
        }
        let point = if self.mapping.is_empty() {
            pair
        } else {
            self.mapping[pair]
        };
        let position = *self
            .coordinates
            .get(point)
            .ok_or("AP242 triangle references a missing coordinate")?;
        let normal = match self.normals.len() {
            0 => None,
            1 => Some(self.normals[0]),
            _ => Some(self.normals[pair]),
        };
        Ok((position, normal))
    }

    /// Append three already validated corners, preserving per-pair rather than per-point normals.
    fn append(&self, corners: [(Vec3, Option<Vec3>); 3], scene: &mut Scene) {
        let index = scene.triangles.len();
        scene.triangles.push(Triangle {
            vertices: corners.map(|(point, _)| point),
            color: None,
        });
        if !self.normals.is_empty() {
            scene.appearances.insert(
                index,
                TriangleAppearance {
                    normals: Some(corners.map(|(_, normal)| normal.unwrap_or_default())),
                    ..Default::default()
                },
            );
        }
    }
}

/// Lazy AP242 decoder with shared coordinate caching and cumulative selected-geometry limits.
pub(super) struct Decoder<'a> {
    /// Complete AST entity lookup; unrelated geometry is never decoded eagerly.
    entities: &'a HashMap<u64, &'a EntityInstance>,
    /// Coordinates already decoded from selected faces.
    coordinates: HashMap<u64, Arc<Vec<Vec3>>>,
}

impl<'a> Decoder<'a> {
    /// Construct a lazy decoder without inspecting or allocating source geometry.
    pub fn new(entities: &'a HashMap<u64, &'a EntityInstance>) -> Self {
        Self {
            entities,
            coordinates: HashMap::new(),
        }
    }

    /// Decode a selected surface or structured shell/solid; delegate other schema items.
    ///
    /// # Errors
    /// Returns an error for malformed attributes, missing references, invalid indices,
    /// normals or declared counts, unsupported structured children, duplicate set items,
    /// or excessive selected geometry, memory and processing work.
    pub fn decode(&mut self, id: u64, budget: &mut Budget) -> Result<Option<Scene>> {
        budget.work(1)?;
        let entity = self
            .entities
            .get(&id)
            .ok_or_else(|| format!("AP242 tessellation references missing entity #{id}"))?;
        let Some(fields) = fields(entity)? else {
            return Ok(None);
        };
        let mut scene = Scene::default();
        if fields.kind.surface() {
            self.surface(&fields, &mut scene, budget)?;
        } else if matches!(fields.kind, Kind::Shell | Kind::Solid) {
            let items = list(fields.values[1])?;
            if items.is_empty() || items.len() > MAX_ITEMS {
                return Err("AP242 tessellated shell/solid item count is invalid".into());
            }
            link(fields.values[2], self.entities, fields.kind)?;
            budget.work(items.len())?;
            let mut seen = HashSet::new();
            for item in items {
                let child = reference(item)?;
                if !seen.insert(child) {
                    return Err("AP242 tessellated shell/solid repeats a structured item".into());
                }
                let child = self
                    .entities
                    .get(&child)
                    .ok_or("AP242 tessellated shell/solid references a missing item")?;
                let child = fields_for_face(child)?;
                self.surface(&child, &mut scene, budget)?;
            }
        } else {
            return Ok(None);
        }
        scene.validate()?;
        Ok(Some(scene))
    }

    /// Resolve and cache a counted coordinate list referenced by selected geometry.
    fn coordinates(&mut self, id: u64, budget: &mut Budget) -> Result<Arc<Vec<Vec3>>> {
        if let Some(points) = self.coordinates.get(&id) {
            return Ok(Arc::clone(points));
        }
        let entity = self
            .entities
            .get(&id)
            .ok_or("AP242 surface references a missing coordinate list")?;
        let fields =
            fields(entity)?.ok_or("AP242 surface references an unsupported coordinate list")?;
        if fields.kind != Kind::Coordinates {
            return Err("AP242 surface coordinates must reference COORDINATES_LIST".into());
        }
        let count = positive(fields.values[1])?;
        let values = list(fields.values[2])?;
        if values.len() != count {
            return Err("AP242 coordinate count does not match npoints".into());
        }
        budget.vectors(count)?;
        let points = Arc::new(values.iter().map(vector).collect::<Result<Vec<_>>>()?);
        self.coordinates.insert(id, Arc::clone(&points));
        Ok(points)
    }

    /// Validate a face's point/normal mapping and optional exact geometry link.
    fn face(&mut self, fields: &Fields<'_>, budget: &mut Budget) -> Result<Face> {
        let coordinates = self.coordinates(reference(fields.values[1])?, budget)?;
        let pnmax = positive(fields.values[2])?;
        if pnmax > MAX_VECTORS {
            return Err("AP242 pnmax exceeds vector limit".into());
        }
        let values = list(fields.values[3])?;
        if !matches!(values.len(), 0 | 1) && values.len() != pnmax {
            return Err("AP242 normal count must be zero, one or pnmax".into());
        }
        budget.vectors(values.len())?;
        let normals = values
            .iter()
            .map(|value| {
                let normal = vector(value)?;
                let length = normal.length();
                if length <= 0.0 || !length.is_finite() {
                    return Err("AP242 surface normal must have finite nonzero length".into());
                }
                Ok(normal / length)
            })
            .collect::<Result<Vec<_>>>()?;
        let map_field = if fields.kind.linked_face() {
            link(fields.values[4], self.entities, fields.kind)?;
            5
        } else {
            4
        };
        let values = list(fields.values[map_field])?;
        if (values.is_empty() && pnmax != coordinates.len())
            || (!values.is_empty() && values.len() != pnmax)
        {
            return Err("AP242 pnindex count is inconsistent with pnmax and coordinates".into());
        }
        budget.indices(values.len())?;
        budget.storage(values.len(), std::mem::size_of::<usize>())?;
        let mapping = values
            .iter()
            .map(|value| {
                let index = positive(value)? - 1;
                if index >= coordinates.len() {
                    return Err("AP242 pnindex references a missing coordinate".into());
                }
                Ok(index)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Face {
            coordinates,
            mapping,
            normals,
            pnmax,
        })
    }

    /// Expand direct triples or alternating strips and fixed-center fans into one scene.
    fn surface(
        &mut self,
        fields: &Fields<'_>,
        scene: &mut Scene,
        budget: &mut Budget,
    ) -> Result<()> {
        budget.faces(1)?;
        let face = self.face(fields, budget)?;
        let primitive = if fields.kind.linked_face() { 6 } else { 5 };
        if fields.kind.complex_triangles() {
            let strips = list(fields.values[primitive])?;
            let fans = list(fields.values[primitive + 1])?;
            if strips.is_empty() && fans.is_empty() {
                return Err("AP242 complex surface has no triangle strips or fans".into());
            }
            Self::sequences(&face, strips, false, scene, budget)?;
            Self::sequences(&face, fans, true, scene, budget)?;
        } else {
            let triangles = list(fields.values[primitive])?;
            if triangles.is_empty() {
                return Err("AP242 triangulated surface has no triangles".into());
            }
            budget.indices(
                triangles
                    .len()
                    .checked_mul(3)
                    .ok_or("AP242 triangle indices overflow")?,
            )?;
            budget.triangles(triangles.len(), !face.normals.is_empty())?;
            scene.triangles.try_reserve_exact(triangles.len())?;
            for triangle in triangles {
                let [a, b, c] = list(triangle)? else {
                    return Err("AP242 independent triangle requires exactly three indices".into());
                };
                face.append([face.corner(a)?, face.corner(b)?, face.corner(c)?], scene);
            }
        }
        Ok(())
    }

    /// Expand one family of strip or fan aggregates without retaining decoded index arrays.
    fn sequences(
        face: &Face,
        sequences: &[Parameter],
        fan: bool,
        scene: &mut Scene,
        budget: &mut Budget,
    ) -> Result<()> {
        budget.work(sequences.len())?;
        for sequence in sequences {
            let values = list(sequence)?;
            if values.len() < 3 {
                return Err("AP242 triangle strip/fan requires at least three indices".into());
            }
            budget.indices(values.len())?;
            budget.triangles(values.len() - 2, !face.normals.is_empty())?;
            scene.triangles.try_reserve_exact(values.len() - 2)?;
            let first = face.corner(&values[0])?;
            let mut previous = face.corner(&values[1])?;
            let mut previous_previous = first;
            for (offset, value) in values[2..].iter().enumerate() {
                let current = face.corner(value)?;
                let corners = if fan {
                    [first, previous, current]
                } else if offset % 2 == 0 {
                    [previous_previous, previous, current]
                } else {
                    [previous, previous_previous, current]
                };
                face.append(corners, scene);
                previous_previous = previous;
                previous = current;
            }
        }
        Ok(())
    }
}

/// Require a supported structured face rather than silently ignoring required edges or vertices.
fn fields_for_face(entity: &EntityInstance) -> Result<Fields<'_>> {
    let fields =
        fields(entity)?.ok_or("unsupported AP242 tessellated shell/solid child geometry")?;
    if !fields.kind.linked_face() {
        return Err("AP242 tessellated shell/solid requires supported structured faces".into());
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    //! Analytic AST controls independent of the Part 21 parser and CAD tessellation kernel.

    use super::*;
    use truck_stepio::r#in::ruststep::ast::SubSuperRecord;

    /// Construct an aggregate without relying on upstream empty-list parsing.
    fn aggregate(values: Vec<Parameter>) -> Parameter {
        Parameter::List(values)
    }

    /// Construct an authored schema integer from a bounded test index or count.
    fn integer(value: usize) -> Parameter {
        Parameter::Integer(i64::try_from(value).unwrap())
    }

    /// Construct an aggregate of positive one-based primitive indices.
    fn indices(values: &[usize]) -> Parameter {
        aggregate(values.iter().copied().map(integer).collect())
    }

    /// Construct three-dimensional coordinate or normal aggregates.
    fn vectors(values: &[[f64; 3]]) -> Parameter {
        aggregate(
            values
                .iter()
                .map(|values| aggregate(values.iter().copied().map(Parameter::Real).collect()))
                .collect(),
        )
    }

    /// Construct the empty inherited representation-item label.
    fn name() -> Parameter {
        Parameter::String(String::new())
    }

    /// Construct a numeric entity reference in the authored AST.
    fn entity_ref(id: u64) -> Parameter {
        Parameter::Ref(Name::Entity(id))
    }

    /// Construct a simple schema instance with named typed parameters.
    fn simple(id: u64, name: &str, values: Vec<Parameter>) -> EntityInstance {
        EntityInstance::Simple {
            id,
            record: Record {
                name: name.to_owned(),
                parameter: aggregate(values),
            },
        }
    }

    /// Provide an independently authored counted coordinate table at entity one.
    fn points(values: &[[f64; 3]]) -> EntityInstance {
        simple(
            1,
            "COORDINATES_LIST",
            vec![name(), integer(values.len()), vectors(values)],
        )
    }

    /// Provide a direct structured face at entity two with explicit pair indirection.
    fn face(pnmax: usize, normals: &[[f64; 3]], mapping: &[usize]) -> EntityInstance {
        simple(
            2,
            "TRIANGULATED_FACE",
            vec![
                name(),
                entity_ref(1),
                integer(pnmax),
                vectors(normals),
                Parameter::NotProvided,
                indices(mapping),
                aggregate(vec![indices(&[1, 2, 3])]),
            ],
        )
    }

    /// Resolve a selected item through a fresh decoder and require a surface result.
    fn decode(entities: &[EntityInstance], id: u64) -> Result<Scene> {
        let indexed: HashMap<_, _> = entities
            .iter()
            .map(|entity| {
                let id = match entity {
                    EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
                };
                (id, entity)
            })
            .collect();
        Decoder::new(&indexed)
            .decode(id, &mut Budget::default())?
            .ok_or_else(|| "authored tessellated surface was unexpectedly delegated".into())
    }

    /// Change one typed simple-record attribute to exercise its malformed contract.
    fn change(entity: &mut EntityInstance, index: usize, value: Parameter) {
        let EntityInstance::Simple { record, .. } = entity else {
            panic!("authored mutation requires a simple entity");
        };
        let Parameter::List(values) = &mut record.parameter else {
            panic!("authored mutation requires parameter fields");
        };
        values[index] = value;
    }

    #[test]
    fn direct_triangles_retain_absent_single_and_pair_normals() -> Result<()> {
        let coordinates = points(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]]);
        let no_normals = decode(&[coordinates.clone(), face(3, &[], &[])], 2)?;
        assert_eq!(no_normals.triangles.len(), 1);
        assert!(no_normals.appearances.is_empty());
        let single = decode(&[coordinates.clone(), face(3, &[[0.0, 0.0, 7.0]], &[])], 2)?;
        assert_eq!(
            single.appearances[&0].normals,
            Some([Vec3::new(0.0, 0.0, 1.0); 3])
        );
        let pairs = decode(
            &[
                coordinates,
                face(
                    3,
                    &[[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]],
                    &[3, 1, 2],
                ),
            ],
            2,
        )?;
        assert_eq!(
            pairs.triangles[0].vertices,
            [
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0)
            ]
        );
        assert_eq!(
            pairs.appearances[&0].normals,
            Some([
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0)
            ])
        );
        Ok(())
    }

    #[test]
    fn strips_alternate_and_fans_keep_their_fixed_center() -> Result<()> {
        let source = points(&[
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [2.0, 3.0, 0.0],
        ]);
        for (fan, mapping, expected) in [
            (false, vec![], [[1, 2, 3], [3, 2, 4]]),
            (true, vec![1, 2, 4, 3], [[1, 2, 4], [1, 4, 3]]),
        ] {
            let sequence = aggregate(vec![indices(&[1, 2, 3, 4])]);
            let empty = aggregate(vec![]);
            let fields = vec![
                name(),
                entity_ref(1),
                integer(4),
                vectors(&[[0.0, 0.0, 1.0]]),
                Parameter::NotProvided,
                indices(&mapping),
                if fan { empty.clone() } else { sequence.clone() },
                if fan { sequence } else { empty },
            ];
            let scene = decode(
                &[
                    source.clone(),
                    simple(2, "COMPLEX_TRIANGULATED_FACE", fields),
                ],
                2,
            )?;
            let positions = [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(2.0, 3.0, 0.0),
            ];
            assert_eq!(scene.triangles.len(), 2);
            for (triangle, expected) in scene.triangles.iter().zip(expected) {
                assert_eq!(
                    triangle.vertices,
                    expected.map(|index| positions[index - 1])
                );
                let [a, b, c] = triangle.vertices;
                assert!((b - a).cross(c - a).dot(Vec3::new(0.0, 0.0, 1.0)) > 0.0);
            }
        }
        Ok(())
    }

    #[test]
    fn repeated_points_keep_distinct_pair_normals_and_degenerate_triangles() -> Result<()> {
        let mut triangles = face(
            4,
            &[
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, -1.0],
            ],
            &[1, 2, 3, 1],
        );
        change(&mut triangles, 6, aggregate(vec![indices(&[1, 4, 2])]));
        let scene = decode(
            &[
                points(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                triangles,
            ],
            2,
        )?;
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(
            scene.triangles[0].vertices[0],
            scene.triangles[0].vertices[1]
        );
        let normals = scene.appearances[&0].normals.unwrap();
        assert_eq!(normals[0], Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(normals[1], Vec3::new(0.0, 0.0, -1.0));
        Ok(())
    }

    #[test]
    fn simple_and_external_complex_surface_attributes_match() -> Result<()> {
        let coordinates = points(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        let partials = [
            ("GEOMETRIC_REPRESENTATION_ITEM", vec![]),
            ("REPRESENTATION_ITEM", vec![name()]),
            ("TESSELLATED_ITEM", vec![]),
            (
                "TESSELLATED_SURFACE_SET",
                vec![entity_ref(1), integer(3), vectors(&[[0.0, 0.0, 1.0]])],
            ),
            (
                "TRIANGULATED_SURFACE_SET",
                vec![indices(&[]), aggregate(vec![indices(&[1, 2, 3])])],
            ),
        ];
        let complex = EntityInstance::Complex {
            id: 2,
            subsuper: SubSuperRecord(
                partials
                    .into_iter()
                    .map(|(name, values)| Record {
                        name: name.to_owned(),
                        parameter: aggregate(values),
                    })
                    .collect(),
            ),
        };
        let simple = simple(
            2,
            "TRIANGULATED_SURFACE_SET",
            vec![
                name(),
                entity_ref(1),
                integer(3),
                vectors(&[[0.0, 0.0, 1.0]]),
                indices(&[]),
                aggregate(vec![indices(&[1, 2, 3])]),
            ],
        );
        let external = decode(&[coordinates.clone(), complex], 2)?;
        let internal = decode(&[coordinates, simple], 2)?;
        assert_eq!(
            external.triangles[0].vertices,
            internal.triangles[0].vertices
        );
        assert_eq!(
            external.appearances[&0].normals,
            internal.appearances[&0].normals
        );
        Ok(())
    }

    #[test]
    fn selected_container_caches_coordinates_and_excludes_unselected_items() -> Result<()> {
        let coordinates = points(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]]);
        let mut other = face(3, &[], &[]);
        let EntityInstance::Simple { id, .. } = &mut other else {
            unreachable!()
        };
        *id = 3;
        let solid = simple(
            4,
            "TESSELLATED_SOLID",
            vec![
                name(),
                aggregate(vec![entity_ref(2)]),
                Parameter::NotProvided,
            ],
        );
        let entities = [coordinates, face(3, &[], &[]), other, solid];
        let indexed: HashMap<_, _> = entities
            .iter()
            .enumerate()
            .map(|(index, value)| (u64::try_from(index + 1).unwrap(), value))
            .collect();
        let mut decoder = Decoder::new(&indexed);
        let mut budget = Budget::default();
        assert_eq!(decoder.decode(4, &mut budget)?.unwrap().triangles.len(), 1);
        assert_eq!(decoder.coordinates.len(), 1);
        assert_eq!(budget.vectors, 3);
        assert_eq!(decoder.decode(3, &mut budget)?.unwrap().triangles.len(), 1);
        assert_eq!(budget.vectors, 3);
        Ok(())
    }

    #[test]
    fn malformed_counts_indices_normals_and_links_fail() {
        let coordinates = points(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        let mut wrong_count = coordinates.clone();
        change(&mut wrong_count, 1, integer(4));
        assert!(decode(&[wrong_count, face(3, &[], &[])], 2).is_err());
        for (field, value) in [
            (1, entity_ref(999)),
            (2, integer(0)),
            (2, integer(2)),
            (3, vectors(&[[0.0, 0.0, 0.0]])),
            (3, vectors(&[[0.0, 0.0, 1.0]; 2])),
            (3, vectors(&[[f64::NAN, 0.0, 1.0]])),
            (4, entity_ref(999)),
            (4, Parameter::List(vec![])),
            (5, indices(&[1, 2])),
            (5, indices(&[1, 2, 4])),
            (6, aggregate(vec![indices(&[0, 2, 3])])),
            (6, aggregate(vec![indices(&[1, 2, 4])])),
            (6, aggregate(vec![indices(&[1, 2])])),
            (6, aggregate(vec![])),
        ] {
            let mut malformed = face(3, &[], &[]);
            change(&mut malformed, field, value);
            assert!(
                decode(&[coordinates.clone(), malformed], 2).is_err(),
                "field {field}"
            );
        }
    }

    #[test]
    fn unsupported_required_children_and_duplicate_set_members_fail() {
        let coordinates = points(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        for children in [vec![entity_ref(2), entity_ref(2)], vec![entity_ref(999)]] {
            let solid = simple(
                3,
                "TESSELLATED_SOLID",
                vec![name(), aggregate(children), Parameter::NotProvided],
            );
            assert!(decode(&[coordinates.clone(), face(3, &[], &[]), solid], 3).is_err());
        }
        let edge = simple(2, "TESSELLATED_EDGE", vec![]);
        assert!(handles(&edge));
        let solid = simple(
            3,
            "TESSELLATED_SOLID",
            vec![
                name(),
                aggregate(vec![entity_ref(2)]),
                Parameter::NotProvided,
            ],
        );
        assert!(decode(&[coordinates, edge, solid], 3).is_err());
    }

    #[test]
    fn exact_geometry_links_require_the_correct_schema_target_types() -> Result<()> {
        let coordinates = points(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        let mut linked = face(3, &[], &[]);
        change(&mut linked, 4, entity_ref(3));
        let plane = simple(3, "PLANE", vec![]);
        assert_eq!(
            decode(&[coordinates.clone(), linked.clone(), plane], 2)?
                .triangles
                .len(),
            1
        );
        let point = simple(3, "CARTESIAN_POINT", vec![]);
        assert!(decode(&[coordinates.clone(), linked, point], 2).is_err());
        for (kind, valid, invalid) in [
            ("TESSELLATED_SHELL", "CLOSED_SHELL", "MANIFOLD_SOLID_BREP"),
            ("TESSELLATED_SOLID", "MANIFOLD_SOLID_BREP", "CLOSED_SHELL"),
        ] {
            let container = simple(
                4,
                kind,
                vec![name(), aggregate(vec![entity_ref(2)]), entity_ref(3)],
            );
            let entities = [
                coordinates.clone(),
                face(3, &[], &[]),
                simple(3, valid, vec![]),
                container.clone(),
            ];
            assert_eq!(decode(&entities, 4)?.triangles.len(), 1);
            let entities = [
                coordinates.clone(),
                face(3, &[], &[]),
                simple(3, invalid, vec![]),
                container,
            ];
            assert!(decode(&entities, 4).is_err());
        }
        Ok(())
    }

    #[test]
    fn simple_and_complex_body_link_positions_match() -> Result<()> {
        for kind in ["TESSELLATED_SHELL", "TESSELLATED_SOLID"] {
            let target_name = if kind == "TESSELLATED_SHELL" {
                "CLOSED_SHELL"
            } else {
                "MANIFOLD_SOLID_BREP"
            };
            let target = simple(13, target_name, vec![]);
            let targets = HashMap::from([(13, &target)]);
            for expected in [Some(13), None] {
                let link = expected.map_or(Parameter::NotProvided, entity_ref);
                let simple_entity = simple(
                    2,
                    kind,
                    vec![name(), aggregate(vec![entity_ref(1)]), link.clone()],
                );
                let complex = EntityInstance::Complex {
                    id: 2,
                    subsuper: SubSuperRecord(
                        [
                            ("GEOMETRIC_REPRESENTATION_ITEM", vec![]),
                            ("REPRESENTATION_ITEM", vec![name()]),
                            ("TESSELLATED_ITEM", vec![]),
                            (kind, vec![aggregate(vec![entity_ref(1)]), link]),
                        ]
                        .into_iter()
                        .map(|(name, values)| Record {
                            name: name.to_owned(),
                            parameter: aggregate(values),
                        })
                        .collect(),
                    ),
                };
                assert_eq!(exact_body_link(&simple_entity, &targets)?, expected);
                assert_eq!(exact_body_link(&complex, &targets)?, expected);
                if expected.is_some() {
                    assert!(exact_body_link(&simple_entity, &HashMap::new()).is_err());
                    let wrong_name = if kind == "TESSELLATED_SHELL" {
                        "MANIFOLD_SOLID_BREP"
                    } else {
                        "CLOSED_SHELL"
                    };
                    let wrong = simple(13, wrong_name, vec![]);
                    assert!(exact_body_link(&complex, &HashMap::from([(13, &wrong)])).is_err());
                }
            }
        }
        assert_eq!(exact_body_link(&face(3, &[], &[]), &HashMap::new())?, None);
        let mut malformed = simple(
            2,
            "TESSELLATED_SOLID",
            vec![name(), aggregate(vec![entity_ref(1)]), integer(13)],
        );
        assert!(exact_body_link(&malformed, &HashMap::new()).is_err());
        change(&mut malformed, 2, aggregate(vec![]));
        assert!(exact_body_link(&malformed, &HashMap::new()).is_err());
        Ok(())
    }
}
