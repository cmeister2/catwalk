//! Verify generated product placements before a tolerant STEP reader can default them.

use crate::{Result, scene::Vec3};
use std::collections::{HashMap, HashSet};
use truck_stepio::r#in::ruststep::ast::{EntityInstance, Name, Parameter, Record};

/// Match the bounded Part 21 parser's entity ceiling.
const MAX_ENTITIES: usize = 250_000;
/// Match the native renderer's placed-part occurrence ceiling.
const MAX_PLACEMENTS: usize = 4096;
/// Allow normal floating-point frame roundoff while refusing a changed rigid frame.
const FRAME_ERROR: f64 = 1e-8;

/// Require a complete, unique generated usage-to-representation placement chain.
///
/// # Errors
/// Invalid syntax, ambiguous ownership, absent frames, degenerate axes, unsupported
/// mapped body placements, and malformed transformation attributes are refused.
pub(super) fn validate(text: &str) -> Result<()> {
    let exchange = super::super::step_parameters::parse(text)?;
    if exchange.data.len() != 1 || !exchange.anchor.is_empty() || !exchange.reference.is_empty() {
        return Err("native CAD projection requires one local STEP data section".into());
    }
    let mut entities = HashMap::new();
    for entity in &exchange.data[0].entities {
        let (id, _) = records(entity);
        if entities.len() >= MAX_ENTITIES || entities.insert(id, entity).is_some() {
            return Err("native CAD STEP entity identities are repeated or excessive".into());
        }
    }
    let index = Index { entities };
    let mut representations = HashMap::new();
    let mut placements = HashMap::new();
    let mut relationships = HashSet::new();
    for (&id, entity) in &index.entities {
        for record in records(entity).1 {
            match record.name.as_str() {
                "MAPPED_ITEM"
                | "REPRESENTATION_MAP"
                | "CARTESIAN_TRANSFORMATION_OPERATOR_3D"
                | "CARTESIAN_TRANSFORMATION_OPERATOR_3D_NON_UNIFORM" => {
                    return Err(
                        "native CAD mapped body placements are not qualified for this tessellator"
                            .into(),
                    );
                }
                "AXIS2_PLACEMENT_3D" => index.frame(id)?,
                "ITEM_DEFINED_TRANSFORMATION" => index.transformation(id)?,
                "SHAPE_DEFINITION_REPRESENTATION" => {
                    let args = parameters(record, 2)?;
                    let owner = index.definition(reference(&args[0])?)?;
                    let representation = reference(&args[1])?;
                    index.representation(representation)?;
                    if index.named(owner, "PRODUCT_DEFINITION")?.is_some()
                        && representations.insert(owner, representation).is_some()
                    {
                        return Err("native CAD product has ambiguous shape representations".into());
                    }
                }
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" => {
                    let args = parameters(record, 2)?;
                    let owner = index.definition(reference(&args[1])?)?;
                    index.required(owner, "NEXT_ASSEMBLY_USAGE_OCCURRENCE")?;
                    let relationship = reference(&args[0])?;
                    if placements.len() >= MAX_PLACEMENTS
                        || placements.insert(owner, relationship).is_some()
                        || !relationships.insert(relationship)
                    {
                        return Err("native CAD usage has repeated placement relationships".into());
                    }
                }
                _ => {}
            }
        }
    }
    for (&id, entity) in &index.entities {
        if let Some(usage) = index.named(id, "NEXT_ASSEMBLY_USAGE_OCCURRENCE")? {
            let args = parameters(usage, 6)?;
            let (parent, child) = (reference(&args[3])?, reference(&args[4])?);
            index.required(parent, "PRODUCT_DEFINITION")?;
            index.required(child, "PRODUCT_DEFINITION")?;
            if parent == child {
                return Err("native CAD usage references its own product".into());
            }
            let parent_representation = *representations
                .get(&parent)
                .ok_or("native CAD parent product has no shape representation")?;
            let child_representation = *representations
                .get(&child)
                .ok_or("native CAD child product has no shape representation")?;
            let relationship = *placements
                .get(&id)
                .ok_or("native CAD usage has no explicit placement relationship")?;
            index.relationship(relationship, parent_representation, child_representation)?;
        }
        // Every relationship must be attached to a verified source usage; the
        // importer otherwise ignores an unowned transform without reporting it.
        if records(entity)
            .1
            .iter()
            .any(|r| r.name == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
            && !relationships.contains(&id)
        {
            return Err("native CAD transformation relationship has no source usage".into());
        }
    }
    Ok(())
}

/// An index over the bounded, original Part 21 AST; no source text is rewritten.
struct Index<'a> {
    /// Entity identities cannot alias or come from auxiliary data sections.
    entities: HashMap<u64, &'a EntityInstance>,
}

impl<'a> Index<'a> {
    /// Resolve one subtype while refusing repeated subtype records.
    ///
    /// # Errors
    /// Missing identities and repeated subtype attributes fail.
    fn named(&self, id: u64, name: &str) -> Result<Option<&'a Record>> {
        let entity = self
            .entities
            .get(&id)
            .ok_or("native CAD placement reference is missing")?;
        let mut matching = records(entity)
            .1
            .iter()
            .filter(|record| record.name == name);
        let record = matching.next();
        if matching.next().is_some() {
            return Err("native CAD placement has repeated subtype attributes".into());
        }
        Ok(record)
    }

    /// Require the exact attribute family used by the generated source.
    ///
    /// # Errors
    /// Missing entities, unsupported types and repeated attributes fail.
    fn required(&self, id: u64, name: &str) -> Result<&'a Record> {
        self.named(id, name)?
            .ok_or_else(|| format!("native CAD placement #{id} is not {name}").into())
    }

    /// Generated frames and item transforms have one complete simple schema record.
    ///
    /// # Errors
    /// Missing identities, complex schemas and unexpected record types fail.
    fn single(&self, id: u64, name: &str) -> Result<&'a Record> {
        let entity = self
            .entities
            .get(&id)
            .ok_or("native CAD placement reference is missing")?;
        match records(entity).1 {
            [record] if record.name == name => Ok(record),
            _ => Err("native CAD placement has an unsupported simple schema".into()),
        }
    }

    /// Resolve the ownership recorded by a product-definition shape.
    ///
    /// # Errors
    /// Missing ownership records, malformed attributes and nonlocal references fail.
    fn definition(&self, id: u64) -> Result<u64> {
        let args = parameters(self.required(id, "PRODUCT_DEFINITION_SHAPE")?, 3)?;
        reference(&args[2])
    }

    /// Require one of the native writer's supported shape representations.
    ///
    /// # Errors
    /// Missing or unsupported representations and invalid item/context references fail.
    fn representation(&self, id: u64) -> Result<()> {
        let entity = self
            .entities
            .get(&id)
            .ok_or("native CAD shape representation is missing")?;
        let record = match records(entity).1 {
            [record]
                if matches!(
                    record.name.as_str(),
                    "SHAPE_REPRESENTATION"
                        | "MANIFOLD_SURFACE_SHAPE_REPRESENTATION"
                        | "ADVANCED_BREP_SHAPE_REPRESENTATION"
                ) =>
            {
                record
            }
            _ => {
                return Err("native CAD placement references an unsupported representation".into());
            }
        };
        let args = parameters(record, 3)?;
        let Parameter::List(items) = &args[1] else {
            return Err("native CAD shape representation has no item list".into());
        };
        if items.is_empty() {
            return Err("native CAD shape representation contains no items".into());
        }
        for item in items {
            reference(item)?;
        }
        reference(&args[2])?;
        Ok(())
    }

    /// Verify a rigid Cartesian frame rather than accepting fallback axes.
    ///
    /// # Errors
    /// Invalid point or direction references, nonunit or nonorthogonal axes fail.
    fn frame(&self, id: u64) -> Result<()> {
        let args = parameters(self.single(id, "AXIS2_PLACEMENT_3D")?, 4)?;
        let _ = self.coordinates(reference(&args[1])?, "CARTESIAN_POINT")?;
        let axis = self.direction(&args[2], Vec3::new(0., 0., 1.))?;
        let reference = self.direction(&args[3], Vec3::new(1., 0., 0.))?;
        if axis.dot(reference).abs() > FRAME_ERROR {
            return Err("native CAD frame axes are parallel or not orthogonal".into());
        }
        Ok(())
    }

    /// Validate finite three-dimensional point or direction coordinates.
    ///
    /// # Errors
    /// Missing coordinate records, incorrect dimensions and invalid numbers fail.
    fn coordinates(&self, id: u64, name: &str) -> Result<Vec3> {
        let args = parameters(self.required(id, name)?, 2)?;
        let Parameter::List(values) = &args[1] else {
            return Err("native CAD frame coordinates are not an aggregate".into());
        };
        let [x, y, z] = values.as_slice() else {
            return Err("native CAD frame coordinates are not three dimensional".into());
        };
        Ok(Vec3::new(number(x)?, number(y)?, number(z)?))
    }

    /// Admit the writer's unit axes, including explicit STEP default axes.
    ///
    /// # Errors
    /// Unsupported default fields, missing directions and nonunit vectors fail.
    fn direction(&self, value: &Parameter, default: Vec3) -> Result<Vec3> {
        let direction = if matches!(value, Parameter::NotProvided) {
            default
        } else {
            self.coordinates(reference(value)?, "DIRECTION")?
        };
        if (direction.length() - 1.).abs() > FRAME_ERROR {
            return Err("native CAD frame direction is zero or not a unit vector".into());
        }
        Ok(direction)
    }

    /// Require both source and target frames for an item-defined transformation.
    ///
    /// # Errors
    /// Missing frame references, incorrect attribute schemas and invalid axes fail.
    fn transformation(&self, id: u64) -> Result<()> {
        let args = parameters(self.single(id, "ITEM_DEFINED_TRANSFORMATION")?, 4)?;
        self.frame(reference(&args[2])?)?;
        self.frame(reference(&args[3])?)
    }

    /// Check the complete generated complex relationship and its two products.
    ///
    /// # Errors
    /// Unsupported schemas, mismatched product representations and invalid transforms fail.
    fn relationship(&self, id: u64, parent: u64, child: u64) -> Result<()> {
        let entity = self
            .entities
            .get(&id)
            .ok_or("native CAD relationship is missing")?;
        if records(entity).1.len() != 3 {
            return Err("native CAD placement has an unsupported relationship schema".into());
        }
        parameters(self.required(id, "SHAPE_REPRESENTATION_RELATIONSHIP")?, 0)?;
        let args = parameters(self.required(id, "REPRESENTATION_RELATIONSHIP")?, 4)?;
        let (first, second) = (reference(&args[2])?, reference(&args[3])?);
        if !((first == child && second == parent) || (first == parent && second == child)) {
            return Err(
                "native CAD relationship does not bind its parent and child representations".into(),
            );
        }
        let args = parameters(
            self.required(id, "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")?,
            1,
        )?;
        self.transformation(reference(&args[0])?)
    }
}

/// Read an entity's simple or complex records without flattening inherited fields.
fn records(entity: &EntityInstance) -> (u64, &[Record]) {
    match entity {
        EntityInstance::Simple { id, record } => (*id, std::slice::from_ref(record)),
        EntityInstance::Complex { id, subsuper } => (*id, &subsuper.0),
    }
}

/// Attribute arity is exact because this consumer accepts only generated AP242.
///
/// # Errors
/// Nonaggregate or incorrect attribute arity fails.
fn parameters(record: &Record, length: usize) -> Result<&[Parameter]> {
    match &record.parameter {
        Parameter::List(values) if values.len() == length => Ok(values),
        _ => Err("native CAD placement has missing or unsupported attributes".into()),
    }
}

/// Resolve a local entity reference, refusing omitted, external, and value objects.
///
/// # Errors
/// Any parameter other than a local numbered entity reference fails.
fn reference(value: &Parameter) -> Result<u64> {
    match value {
        Parameter::Ref(Name::Entity(id)) => Ok(*id),
        _ => Err("native CAD placement requires an explicit local entity reference".into()),
    }
}

/// Coordinate integers use the geometry representation's existing finite f64 domain.
///
/// # Errors
/// Nonnumeric or nonfinite coordinate parameters fail.
#[allow(
    clippy::cast_precision_loss,
    reason = "STEP integer coordinates use the same f64 representation as real geometry coordinates."
)]
fn number(value: &Parameter) -> Result<f64> {
    let value = match value {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        _ => return Err("native CAD frame coordinate is not numeric".into()),
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err("native CAD frame coordinate is not finite".into())
    }
}

#[cfg(test)]
/// Serialized controls for missing and ambiguous placement information.
#[path = "native_step_parameters_tests.rs"]
mod tests;
