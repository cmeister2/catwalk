//! Typed FreeCAD schema-four object ownership, visibility and rigid placements.

use super::super::{cad::ShapeStyle, fcstd_container};
use crate::Result;
use ogeom_math::{Matrix3, Transform, Vector};
use roxmltree::Node;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum persisted object declarations admitted by one document.
pub(super) const MAX_OBJECTS: usize = 10_000;
/// Maximum selected hierarchy depth.
pub(super) const MAX_DEPTH: usize = 64;
/// Maximum cumulative group references.
const MAX_REFERENCES: usize = 100_000;
/// Maximum interpreted translation or finite geometry-coordinate magnitude.
pub(super) const MAX_COORDINATE: f64 = 1e12;

/// Original object graph and its optional appearance-version interpretation.
pub(super) struct ObjectDocument {
    /// Object definitions in original declaration order.
    pub objects: Vec<Object>,
    /// Exact persisted object names mapped to their declaration indices.
    pub names: BTreeMap<String, usize>,
    /// Whether packed opacity uses pre-1.1 semantics, including the primary unknown-version fallback.
    pub legacy_alpha: Option<bool>,
}

/// Local or external persisted object association.
pub(super) struct Link {
    /// Exact object identifier within the selected source document.
    pub target: String,
    /// Whether the association addresses an external document or subelement.
    pub unsupported: bool,
}

/// Qualified persisted shape, visibility, container, link and appearance metadata.
pub(super) struct Object {
    /// Exact immutable persisted object name.
    pub name: String,
    /// Native object runtime type from its declaration.
    pub kind: String,
    /// Addressed native ASCII shape member, including a possible null member.
    pub shape: Option<String>,
    /// Whether the selected object requires unqualified geometry modes.
    pub unsupported: bool,
    /// Native feature placement, already present in ordinary BREP root locations.
    pub placement: Transform,
    /// Native link placement replacing a linked target's root location.
    pub link_placement: Transform,
    /// Whether target placement is included after link or array placement.
    pub link_transform: bool,
    /// Persisted linked target with its external/subelement classification.
    pub link: Option<Link>,
    /// Local Draft-array source object.
    pub base: Option<String>,
    /// Exact binary placement-list member.
    pub placements: Option<String>,
    /// Exact native per-instance vector scale member, interpreted only for active arrays.
    pub scales: Option<String>,
    /// Native optional instance visibility bits, with index zero at the rightmost character.
    pub visibility_bits: Option<String>,
    /// Persisted declared link-array instance count, when present.
    pub element_count: Option<usize>,
    /// Child object indices in the native group order.
    pub children: Vec<usize>,
    /// Unique group-parent index, excluding unresolved references.
    pub parent: Option<usize>,
    /// Document visibility taking precedence over GUI fallback.
    pub visible: Option<bool>,
    /// Optional source shape appearance and wireframe policy.
    pub style: ShapeStyle,
    /// Presence of actual GUI overrides, independently of their numeric values.
    pub authored: AuthoredAppearance,
}

/// Explicit GUI appearance overrides determining local array inheritance.
#[derive(Default)]
pub(super) struct AuthoredAppearance {
    /// Whether metadata contributes a uniform or per-face color override.
    pub color: bool,
    /// Whether metadata contributes an object opacity override.
    pub opacity: bool,
}

impl Object {
    /// Whether this type contributes an ancestor rigid placement and subtree visibility.
    pub(super) fn is_placement_group(&self) -> bool {
        self.kind == "App::Part"
    }
}

impl ObjectDocument {
    /// Interpret exact declaration/value associations without instantiating application classes.
    ///
    /// # Errors
    ///
    /// Required XML grammar, typed properties, object references or graph limits are invalid.
    pub(super) fn parse(text: &str) -> Result<Self> {
        let xml = fcstd_container::xml(text)?;
        let root = xml.root_element();
        let declarations = required_child(root, "Objects")?;
        let values = required_child(root, "ObjectData")?;
        let mut objects = Vec::new();
        let mut names = BTreeMap::new();
        for declaration in declarations
            .children()
            .filter(|node| node.has_tag_name("Object"))
        {
            if objects.len() >= MAX_OBJECTS {
                return Err("FCStd exceeds 10000 object declarations".into());
            }
            let name = identifier(attribute(declaration, "name")?)?;
            let kind = identifier(attribute(declaration, "type")?)?;
            let unsupported = kind.starts_with("Assembly::");
            if names.insert(name.clone(), objects.len()).is_some() {
                return Err("FCStd duplicate object declaration".into());
            }
            objects.push(Object {
                name,
                kind,
                shape: None,
                unsupported,
                placement: Transform::IDENTITY,
                link_placement: Transform::IDENTITY,
                link_transform: false,
                link: None,
                base: None,
                placements: None,
                scales: None,
                visibility_bits: None,
                element_count: None,
                children: Vec::new(),
                parent: None,
                visible: None,
                style: ShapeStyle {
                    color: [173, 181, 189],
                    alpha: 255,
                    faces: Vec::new(),
                    wireframe: false,
                },
                authored: AuthoredAppearance::default(),
            });
        }
        validate_count(declarations, "Count", objects.len())?;
        let mut seen = BTreeSet::new();
        let mut groups = Vec::new();
        let mut references = 0usize;
        for record in values.children().filter(|node| node.has_tag_name("Object")) {
            let name = attribute(record, "name")?;
            let &index = names
                .get(name)
                .ok_or("FCStd object values have no declaration")?;
            if !seen.insert(index) {
                return Err("FCStd duplicate object value record".into());
            }
            let properties = required_child(record, "Properties")?;
            let children = parse_properties(properties, &mut objects[index])?;
            references = references
                .checked_add(children.len())
                .ok_or("FCStd reference count overflow")?;
            if references > MAX_REFERENCES {
                return Err("FCStd exceeds 100000 group references".into());
            }
            groups.push((index, children));
        }
        if seen.len() != objects.len() {
            return Err("FCStd object declaration is missing its values".into());
        }
        validate_count(values, "Count", objects.len())?;
        resolve_groups(&mut objects, &names, groups)?;
        let legacy_alpha = Some(
            root.attribute("ProgramVersion")
                .and_then(program_version)
                .unwrap_or(true),
        );
        Ok(Self {
            objects,
            names,
            legacy_alpha,
        })
    }

    /// Check object visibility including only qualifying container ancestors.
    pub(super) fn visible(&self, index: usize) -> bool {
        let mut current = Some(index);
        while let Some(ancestor) = current {
            let object = &self.objects[ancestor];
            if (ancestor == index || object.is_placement_group()) && object.visible == Some(false) {
                return false;
            }
            current = object.parent;
        }
        true
    }

    /// Compose the qualifying parent's placements outside a stored feature shape.
    pub(super) fn ancestors(&self, index: usize) -> Transform {
        let mut result = Transform::IDENTITY;
        let mut current = self.objects[index].parent;
        while let Some(index) = current {
            let object = &self.objects[index];
            if object.is_placement_group() {
                result = object.placement * result;
            }
            current = object.parent;
        }
        result
    }

    /// Whether a container's visible descendants already provide stored result geometry.
    pub(super) fn redundant(&self, index: usize) -> bool {
        let mut pending = self.objects[index].children.clone();
        while let Some(child) = pending.pop() {
            if self.visible(child) && self.objects[child].shape.is_some() {
                return true;
            }
            pending.extend_from_slice(&self.objects[child].children);
        }
        false
    }
}

/// Resolve exact group ownership and prove bounded acyclic ancestor chains.
///
/// # Errors
///
/// Missing children, duplicate parents, cycles or excessive hierarchy depth are rejected.
fn resolve_groups(
    objects: &mut [Object],
    names: &BTreeMap<String, usize>,
    groups: Vec<(usize, Vec<String>)>,
) -> Result<()> {
    for (parent, children) in groups {
        let mut unique = BTreeSet::new();
        for name in children {
            let &index = names
                .get(&name)
                .ok_or("FCStd group references a missing object")?;
            if !unique.insert(index) || objects[index].parent.replace(parent).is_some() {
                return Err("FCStd object has duplicate or multiple group parents".into());
            }
            objects[parent].children.push(index);
        }
    }
    for index in 0..objects.len() {
        let mut ancestors = BTreeSet::new();
        let mut current = Some(index);
        while let Some(index) = current {
            if !ancestors.insert(index) || ancestors.len() > MAX_DEPTH {
                return Err("FCStd group cycle or hierarchy exceeds 64 levels".into());
            }
            current = objects[index].parent;
        }
    }
    Ok(())
}

/// Parse the qualified typed property carriers of one declared object.
///
/// # Errors
///
/// Duplicate properties, wrong carrier types, numeric values or associations are malformed.
fn parse_properties(properties: Node<'_, '_>, object: &mut Object) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    let mut children = Vec::new();
    let has_property = |name: &str| {
        properties
            .children()
            .any(|node| node.has_tag_name("Property") && node.attribute("name") == Some(name))
    };
    let instance = !has_property("Shape")
        && (object.kind == "App::Link"
            || has_property("LinkedObject")
            || has_property("PlacementList"));
    for property in properties
        .children()
        .filter(|node| node.has_tag_name("Property"))
    {
        let name = attribute(property, "name")?;
        if !names.insert(name) {
            return Err("FCStd duplicate object property".into());
        }
        if instance {
            parse_instance_property(name, property, object)?;
        }
        match name {
            "Shape" => parse_shape(property, object)?,
            "Placement" | "LinkPlacement" => {
                typed(property, "App::PropertyPlacement")?;
                let value = placement(required_child(property, "PropertyPlacement")?)?;
                if name == "Placement" {
                    object.placement = value;
                } else {
                    object.link_placement = value;
                }
            }
            "Visibility" | "LinkTransform" => {
                typed(property, "App::PropertyBool")?;
                let value = boolean(required_child(property, "Bool")?)?;
                if name == "Visibility" {
                    object.visible = Some(value);
                } else {
                    object.link_transform = value;
                }
            }
            "LinkedObject" => {
                typed(property, "App::PropertyXLink")?;
                let link = required_child(property, "XLink")?;
                let target = identifier(attribute(link, "name")?)?;
                let unsupported = link.attribute("file").is_some_and(|file| !file.is_empty())
                    || link.children().any(|child| child.is_element())
                    || link
                        .attributes()
                        .any(|attribute| !matches!(attribute.name(), "file" | "stamp" | "name"));
                object.link = Some(Link {
                    target,
                    unsupported,
                });
            }
            "Base" => {
                if property.attribute("type") == Some("App::PropertyLink") {
                    let value = attribute(required_child(property, "Link")?, "value")?;
                    object.base = (!value.is_empty()).then(|| value.to_owned());
                }
            }
            "PlacementList" => {
                typed(property, "App::PropertyPlacementList")?;
                let file = attribute(required_child(property, "PlacementList")?, "file")?;
                object.placements = (!file.is_empty()).then(|| file.to_owned());
            }
            "Group" => {
                typed(property, "App::PropertyLinkList")?;
                let list = required_child(property, "LinkList")?;
                for link in list.children().filter(|node| node.has_tag_name("Link")) {
                    children.push(identifier(attribute(link, "value")?)?);
                }
                validate_count(list, "count", children.len())?;
            }
            "Mesh" => object.unsupported = true,
            _ => {}
        }
    }
    Ok(children)
}

/// Bind an exact persisted native shape address or mark unqualified inline storage.
///
/// # Errors
///
/// Native shape property type, direct carrier or addressed member is absent or invalid.
fn parse_shape(property: Node<'_, '_>, object: &mut Object) -> Result<()> {
    typed(property, "Part::PropertyPartShape")?;
    let part = required_child(property, "Part")?;
    if part.attribute("binary").is_some() || part.attribute("brep").is_some() {
        object.unsupported = true;
    } else {
        let file = attribute(part, "file")?;
        if file.is_empty() {
            return Err("FCStd persisted Shape requires an addressed member".into());
        }
        object.shape = Some(file.to_owned());
    }
    Ok(())
}

/// Admit typed instance parameters only when no authoritative stored shape exists.
///
/// # Errors
///
/// Interpreted array counts, visibility grammar, scalar finiteness or native carrier types are invalid.
fn parse_instance_property(name: &str, property: Node<'_, '_>, object: &mut Object) -> Result<()> {
    match name {
        "ScaleList" => {
            typed(property, "App::PropertyVectorList")?;
            let file = attribute(required_child(property, "VectorList")?, "file")?;
            object.scales = (!file.is_empty()).then(|| file.to_owned());
        }
        "VisibilityList" => {
            typed(property, "App::PropertyBoolList")?;
            let bits = attribute(required_child(property, "BoolList")?, "value")?;
            if bits.len() > MAX_OBJECTS || bits.bytes().any(|bit| !matches!(bit, b'0' | b'1')) {
                return Err("FCStd invalid or excessive instance visibility bits".into());
            }
            object.visibility_bits = Some(bits.to_owned());
        }
        "ElementCount" => {
            typed(property, "App::PropertyIntegerConstraint")?;
            let value: usize = attribute(required_child(property, "Integer")?, "value")?.parse()?;
            if value > MAX_OBJECTS {
                return Err("FCStd array exceeds 10000 instances".into());
            }
            object.element_count = Some(value);
        }
        "Scale" => {
            typed(property, "App::PropertyFloat")?;
            if !identity_scale(number(required_child(property, "Float")?, "value")?) {
                object.unsupported = true;
            }
        }
        "ScaleVector" => {
            typed(property, "App::PropertyVector")?;
            let vector = required_child(property, "PropertyVector")?;
            for name in ["valueX", "valueY", "valueZ"] {
                if !identity_scale(number(vector, name)?) {
                    object.unsupported = true;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Return one direct unnamespaced child without accepting ambiguous duplicate carriers.
///
/// # Errors
///
/// Multiple direct elements with the selected name are rejected.
pub(super) fn child<'a, 'input>(
    node: Node<'a, 'input>,
    name: &str,
) -> Result<Option<Node<'a, 'input>>> {
    let mut values = node.children().filter(|child| child.has_tag_name(name));
    let first = values.next();
    if values.next().is_some() {
        return Err(format!("FCStd duplicate direct {name} carrier").into());
    }
    Ok(first)
}

/// Read one required direct typed XML carrier.
///
/// # Errors
///
/// The selected carrier is missing or duplicated.
pub(super) fn required_child<'a, 'input>(
    node: Node<'a, 'input>,
    name: &str,
) -> Result<Node<'a, 'input>> {
    child(node, name)?.ok_or_else(|| format!("FCStd missing direct {name} carrier").into())
}

/// Borrow a required original XML attribute.
///
/// # Errors
///
/// The required attribute is absent.
pub(super) fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str> {
    node.attribute(name)
        .ok_or_else(|| format!("FCStd missing {name} attribute").into())
}

/// Require an exact qualified native property runtime type.
///
/// # Errors
///
/// Another or missing property type cannot admit the selected carrier.
pub(super) fn typed(node: Node<'_, '_>, expected: &str) -> Result<()> {
    if node.attribute("type") != Some(expected) {
        return Err(format!("FCStd property requires type {expected}").into());
    }
    Ok(())
}

/// Decode a finite native scalar attribute before interpretation-specific bounds.
///
/// # Errors
///
/// Missing, invalid or nonfinite scalar values are rejected.
pub(super) fn number(node: Node<'_, '_>, name: &str) -> Result<f64> {
    let value: f64 = attribute(node, name)?.parse()?;
    if !value.is_finite() {
        return Err("FCStd nonfinite scalar".into());
    }
    Ok(value)
}

/// Decode an exact persisted boolean without guessing malformed text.
///
/// # Errors
///
/// Another or missing boolean spelling is invalid.
pub(super) fn boolean(node: Node<'_, '_>) -> Result<bool> {
    match attribute(node, "value")? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("FCStd invalid boolean".into()),
    }
}

/// Validate a bounded immutable object identifier.
///
/// # Errors
///
/// Empty, excessive or control-character identifiers are rejected.
fn identifier(value: &str) -> Result<String> {
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err("FCStd invalid object identifier".into());
    }
    Ok(value.to_owned())
}

/// Check optional persisted list counts against actual bounded records.
///
/// # Errors
///
/// Invalid or inconsistent declared counts are rejected.
fn validate_count(node: Node<'_, '_>, name: &str, actual: usize) -> Result<()> {
    if let Some(value) = node.attribute(name)
        && value.parse::<usize>()? != actual
    {
        return Err("FCStd declared record count is inconsistent".into());
    }
    Ok(())
}

/// Interpret the known pre-1.1 packed-opacity transition from the source program version.
fn program_version(value: &str) -> Option<bool> {
    let (major, tail) = value.split_once('.')?;
    let minor: String = tail.chars().take_while(char::is_ascii_digit).collect();
    Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?) < (1, 1))
}

/// Admit only literal identity scaling from qualified native instance metadata.
#[allow(
    clippy::float_cmp,
    reason = "Every authored nonidentity scale is unsupported, including values one floating-point step from identity."
)]
pub(super) fn identity_scale(value: f64) -> bool {
    value == 1.0
}

/// Decode native XML rigid placement, preserving `FreeCAD`'s axis-angle precedence.
///
/// # Errors
///
/// Missing, nonfinite, singular or invalid rigid components are rejected.
pub(super) fn placement(node: Node<'_, '_>) -> Result<Transform> {
    let translation = [
        number(node, "Px")?,
        number(node, "Py")?,
        number(node, "Pz")?,
    ];
    let quaternion = if node.attribute("A").is_some() {
        let angle = number(node, "A")?;
        let axis = [
            number(node, "Ox")?,
            number(node, "Oy")?,
            number(node, "Oz")?,
        ];
        let maximum = axis.iter().map(|value| value.abs()).fold(0.0_f64, f64::max);
        if maximum == 0.0 && angle != 0.0 {
            return Err("FCStd singular placement rotation axis".into());
        }
        let axis = axis.map(|value| if maximum == 0.0 { 0.0 } else { value / maximum });
        let length = axis.iter().map(|value| value * value).sum::<f64>().sqrt();
        let scale = if length == 0.0 {
            0.0
        } else {
            (angle / 2.0).sin() / length
        };
        [
            axis[0] * scale,
            axis[1] * scale,
            axis[2] * scale,
            (angle / 2.0).cos(),
        ]
    } else {
        [
            number(node, "Q0")?,
            number(node, "Q1")?,
            number(node, "Q2")?,
            number(node, "Q3")?,
        ]
    };
    rigid(translation, quaternion)
}

/// Construct a normalized rigid matrix from bounded translation and quaternion components.
///
/// # Errors
///
/// Nonfinite, excessive, zero-norm or nonorthogonal components are rejected.
pub(super) fn rigid(translation: [f64; 3], quaternion: [f64; 4]) -> Result<Transform> {
    if translation
        .iter()
        .any(|value| !value.is_finite() || value.abs() > MAX_COORDINATE)
        || quaternion.iter().any(|value| !value.is_finite())
    {
        return Err("FCStd nonfinite or excessive placement scalar".into());
    }
    let maximum = quaternion
        .iter()
        .map(|value| value.abs())
        .fold(0.0_f64, f64::max);
    if maximum == 0.0 {
        return Err("FCStd singular placement quaternion".into());
    }
    let quaternion = quaternion.map(|value| value / maximum);
    let norm = quaternion.iter().map(|value| value * value).sum::<f64>();
    if norm == 0.0 {
        return Err("FCStd singular placement quaternion".into());
    }
    let [x, y, z, w] = quaternion;
    let scale = 2.0 / norm;
    let rows = [
        [
            1.0 - scale * (y * y + z * z),
            scale * (x * y - z * w),
            scale * (x * z + y * w),
        ],
        [
            scale * (x * y + z * w),
            1.0 - scale * (x * x + z * z),
            scale * (y * z - x * w),
        ],
        [
            scale * (x * z - y * w),
            scale * (y * z + x * w),
            1.0 - scale * (x * x + y * y),
        ],
    ];
    Ok(Transform::from_parts(
        Matrix3::new(rows),
        1.0,
        Vector::new(translation[0], translation[1], translation[2]),
        1e-9,
    )?)
}
