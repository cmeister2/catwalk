//! IGES exchange.
//!
//! *Elsewhere:* the IGES half of the exchange layer. The format is ANSI
//! Y14.26M / IGES 5.3, a fixed-column deck from the 1980s, read and written
//! here from the published specification.

mod parse;
mod read;
mod write;

pub use read::{IgesImport, IgesReport, read_iges, read_iges_unsewn};
pub use write::write_iges;

/// The entity types the reader and writer translate, by number and by the
/// name the specification gives each. The names are load-bearing twice over:
/// refusal messages quote them, and `tools/parity.py exchange` greps them to
/// keep the parity ledger's IGES coverage figure regenerable from this file
/// rather than asserted.
pub const SUPPORTED_ENTITIES: &[(i64, &str)] = &[
    (100, "CIRCULAR_ARC"),
    (102, "COMPOSITE_CURVE"),
    (104, "CONIC_ARC"),
    (108, "PLANE"),
    (110, "LINE"),
    (112, "SPLINE_CURVE"),
    (114, "PARAMETRIC_SPLINE_SURFACE"),
    (116, "POINT"),
    (118, "RULED_SURFACE"),
    (120, "SURFACE_OF_REVOLUTION"),
    (122, "TABULATED_CYLINDER"),
    (123, "DIRECTION"),
    (124, "TRANSFORMATION_MATRIX"),
    (126, "B_SPLINE_CURVE"),
    (128, "B_SPLINE_SURFACE"),
    (130, "OFFSET_CURVE"),
    (140, "OFFSET_SURFACE"),
    (141, "BOUNDARY"),
    (142, "CURVE_ON_SURFACE"),
    (143, "BOUNDED_SURFACE"),
    (144, "TRIMMED_SURFACE"),
    (150, "BLOCK"),
    (152, "RIGHT_ANGULAR_WEDGE"),
    (154, "RIGHT_CIRCULAR_CYLINDER"),
    (156, "RIGHT_CIRCULAR_CONE_FRUSTUM"),
    (158, "SPHERE"),
    (160, "TORUS"),
    (162, "SOLID_OF_REVOLUTION"),
    (164, "SOLID_OF_LINEAR_EXTRUSION"),
    (168, "ELLIPSOID"),
    (180, "BOOLEAN_TREE"),
    (184, "SOLID_ASSEMBLY"),
    (186, "MANIFOLD_SOLID"),
    (190, "PLANE_SURFACE"),
    (192, "RIGHT_CIRCULAR_CYLINDRICAL_SURFACE"),
    (194, "RIGHT_CIRCULAR_CONICAL_SURFACE"),
    (196, "SPHERICAL_SURFACE"),
    (198, "TOROIDAL_SURFACE"),
    (202, "ANGULAR_DIMENSION"),
    (206, "DIAMETER_DIMENSION"),
    (208, "FLAG_NOTE"),
    (210, "GENERAL_LABEL"),
    (212, "GENERAL_NOTE"),
    (214, "LEADER_ARROW"),
    (216, "LINEAR_DIMENSION"),
    (218, "ORDINATE_DIMENSION"),
    (220, "POINT_DIMENSION"),
    (222, "RADIUS_DIMENSION"),
    (228, "GENERAL_SYMBOL"),
    (308, "SUBFIGURE_DEFINITION"),
    (314, "COLOR"),
    (402, "GROUP"),
    (406, "DEFINITION_LEVELS"),
    (408, "SINGULAR_SUBFIGURE"),
    (430, "SOLID_INSTANCE"),
    (502, "VERTEX_LIST"),
    (504, "EDGE_LIST"),
    (508, "LOOP"),
    (510, "FACE"),
    (514, "SHELL"),
];

/// The specification's name for an entity type, where this module knows it.
#[must_use]
pub fn entity_name(kind: i64) -> Option<&'static str> {
    SUPPORTED_ENTITIES
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, n)| *n)
}
