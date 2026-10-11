//! A part's shape as the exchange writers see it: solids, and the sheet
//! bodies beside them.

use ogeom_core::{OgeomResult, ogeom_bail};
use ogeom_topo::{Model, Shape, ShapeType};

/// The bodies under a part's shape, each with its composed placement and
/// sense, in the shape's own order.
pub(crate) struct Bodies {
    /// Every solid.
    pub solids: Vec<Shape>,
    /// Every shell no solid owns.
    pub shells: Vec<Shape>,
    /// Every face no shell owns.
    pub faces: Vec<Shape>,
}

/// Split a part's shape into its solids, free shells and free faces,
/// looking through compounds and compound solids.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// shape holds a wire, edge or vertex outside every face (wireframe, which
/// `format` is not written with here), or holds nothing to write at all.
pub(crate) fn bodies_of(model: &Model, shape: &Shape, format: &str) -> OgeomResult<Bodies> {
    let mut bodies = Bodies {
        solids: Vec::new(),
        shells: Vec::new(),
        faces: Vec::new(),
    };
    let mut stack = vec![shape.clone()];
    while let Some(shape) = stack.pop() {
        match model.kind_of(&shape)? {
            ShapeType::Compound | ShapeType::CompSolid => {
                let children = model.children_of(&shape)?;
                stack.extend(children.into_iter().rev());
            }
            ShapeType::Solid => bodies.solids.push(shape),
            ShapeType::Shell => bodies.shells.push(shape),
            ShapeType::Face => bodies.faces.push(shape),
            kind @ (ShapeType::Wire | ShapeType::Edge | ShapeType::Vertex) => ogeom_bail!(
                Construction,
                "a part's shape holds a free {}, which {format} export does not write: \
                 it writes solids, shells and faces, not wireframe",
                format!("{kind:?}").to_lowercase()
            ),
        }
    }
    if bodies.solids.is_empty() && bodies.shells.is_empty() && bodies.faces.is_empty() {
        ogeom_bail!(
            Construction,
            "a part's shape holds no solid, shell or face to write as {format}"
        );
    }
    Ok(bodies)
}
