//! Owned local VRML97 interfaces and immutable prototype-instance expansion.
//! Primary rules: <https://www.web3d.org/documents/specifications/14772/V2.0/part1/concepts.html#4.8>.
use super::{MAX_ITEMS, MAX_NODES, Node, Parser, Token, Value, error};
use crate::Result;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Conservative cumulative storage and logical-work accounting for templates and their copies.
#[derive(Default)]
pub(super) struct Budget {
    /// Parsed tokens, owned values, templates and copied values accounted for so far.
    items: usize,
    /// Parsed nodes/templates and structurally expanded nodes accounted for so far.
    pub(super) nodes: usize,
    /// Conservative retained token/template/copy payload and allocation-overhead bytes.
    bytes: usize,
}

impl Budget {
    /// Preflight and reserve complete geometric vector growth rather than relying on uncharged push capacity.
    ///
    /// # Errors
    /// Size arithmetic, logical item limits, byte accounting or allocation fails.
    pub(super) fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<()> {
        let needed = values
            .len()
            .checked_add(additional)
            .ok_or("VRML vector count overflow")?;
        if needed > MAX_ITEMS {
            return Err(error("VRML vector item limit exceeded"));
        }
        if needed > values.capacity() {
            let capacity = needed
                .max(values.capacity().saturating_mul(2))
                .clamp(4, MAX_ITEMS);
            let bytes = (capacity - values.capacity())
                .checked_mul(std::mem::size_of::<T>())
                .ok_or("VRML vector storage overflow")?;
            self.charge(0, 0, bytes)?;
            values.try_reserve_exact(capacity - values.len())?;
            if values.capacity() > capacity {
                self.charge(
                    0,
                    0,
                    (values.capacity() - capacity)
                        .checked_mul(std::mem::size_of::<T>())
                        .ok_or("VRML vector storage overflow")?,
                )?;
            }
        }
        Ok(())
    }

    /// Preflight cumulative counts before allocating or cloning owned payloads.
    ///
    /// # Errors
    /// Arithmetic overflow or the item, node or existing 512 MiB byte ceiling is exceeded.
    pub(super) fn charge(&mut self, items: usize, nodes: usize, bytes: usize) -> Result<()> {
        let items = self
            .items
            .checked_add(items)
            .ok_or("VRML item count overflow")?;
        let nodes = self
            .nodes
            .checked_add(nodes)
            .ok_or("VRML node count overflow")?;
        let bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or("VRML allocation size overflow")?;
        if items > MAX_ITEMS || nodes > MAX_NODES || bytes as u64 > super::super::MAX_INPUT_BYTES {
            return Err(error(
                "VRML template/expansion item, node or byte limit exceeded",
            ));
        }
        self.items = items;
        self.nodes = nodes;
        self.bytes = bytes;
        Ok(())
    }
}

/// One standard single-value VRML97 element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Element {
    /// A TRUE/FALSE value.
    Bool,
    /// A bounded RGB triple.
    Color,
    /// A finite single-precision number.
    Float,
    /// Integer image dimensions/components and unsigned packed pixels.
    Image,
    /// A signed 32-bit integer, including hexadecimal syntax.
    Int,
    /// One node or NULL.
    Node,
    /// Axis and angle numeric components.
    Rotation,
    /// A quoted Unicode string.
    String,
    /// A finite double-precision time.
    Time,
    /// Two finite single-precision vector components.
    Vec2,
    /// Three finite single-precision vector components.
    Vec3,
}

/// Exact standard field type, retaining the distinction between SF and MF declarations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FieldType {
    /// Element type shared by a single field or each entry of a multiple field.
    element: Element,
    /// Whether this is a multiple-valued field.
    multiple: bool,
}

impl FieldType {
    /// Resolve one of the twenty normative VRML97 field names.
    ///
    /// # Errors
    /// The type is not a standard VRML97 single or multiple field.
    fn parse(name: &str) -> Result<Self> {
        let (multiple, suffix) = if let Some(suffix) = name.strip_prefix("SF") {
            (false, suffix)
        } else if let Some(suffix) = name.strip_prefix("MF") {
            (true, suffix)
        } else {
            return Err(error("invalid VRML prototype field type"));
        };
        let element = match suffix {
            "Bool" if !multiple => Element::Bool,
            "Image" if !multiple => Element::Image,
            "Color" => Element::Color,
            "Float" => Element::Float,
            "Int32" => Element::Int,
            "Node" => Element::Node,
            "Rotation" => Element::Rotation,
            "String" => Element::String,
            "Time" => Element::Time,
            "Vec2f" => Element::Vec2,
            "Vec3f" => Element::Vec3,
            _ => return Err(error("unsupported VRML97 prototype field type")),
        };
        Ok(Self { element, multiple })
    }
}

/// Normative field or event access category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Access {
    /// Persistent initialize-only field.
    Field,
    /// Persistent field together with its input and output event channels.
    Exposed,
    /// Input event without a file-stored initial value.
    Input,
    /// Output event without a file-stored initial value.
    Output,
}

impl Access {
    /// Resolve an interface declaration keyword.
    ///
    /// # Errors
    /// The keyword is not one of the four standard access categories.
    fn parse(name: &str) -> Result<Self> {
        match name {
            "field" => Ok(Self::Field),
            "exposedField" => Ok(Self::Exposed),
            "eventIn" => Ok(Self::Input),
            "eventOut" => Ok(Self::Output),
            _ => Err(error("invalid VRML prototype access declaration")),
        }
    }

    /// Whether a declaration accepts a persistent initial value in file syntax.
    pub(super) fn initializes(self) -> bool {
        matches!(self, Self::Field | Self::Exposed)
    }

    /// Whether a body member may associate with this interface declaration, per directional Table 4.4.
    fn accepts(self, declaration: Self) -> bool {
        self == Self::Exposed || self == declaration
    }

    /// Individual persistent/input/output channels occupied by an association.
    fn channels(self) -> &'static [u8] {
        match self {
            Self::Field => &[0],
            Self::Input => &[1],
            Self::Output => &[2],
            Self::Exposed => &[0, 1, 2],
        }
    }
}

/// One typed local prototype interface declaration.
#[derive(Debug)]
pub(super) struct Declaration {
    /// Exact standard field or event type.
    pub(super) ty: FieldType,
    /// Persistent or event access category.
    pub(super) access: Access,
    /// Owned default template for persistent fields; events have no file default.
    default: Option<Value>,
}

/// Uniquely named declarations belonging to one prototype interface.
pub(super) type Interface = HashMap<String, Declaration>;

/// Expanded nodes retaining their source `Rc` so pointer identity keys cannot be recycled.
pub(super) type Memo = HashMap<usize, (Rc<Node>, Rc<Node>)>;

/// Complete immutable local prototype with its own node templates and lexical references.
#[derive(Debug)]
pub(super) struct Prototype {
    /// Typed declarations used for defaults, overrides and IS validation.
    pub(super) interface: Rc<Interface>,
    /// Ordered body roots; only the first participates in the transformation hierarchy.
    body: Vec<Rc<Node>>,
}

/// A node member signature and the persistent name shared by exposed-field aliases.
struct Member {
    /// Exact declared type.
    ty: FieldType,
    /// Resolved access category, including explicit exposed-field event aliases.
    access: Access,
    /// Persistent member name used to detect overlapping IS channels.
    canonical: String,
}

impl Parser {
    /// Initialize scopes and account for existing lexical buffers before owned template parsing.
    ///
    /// # Errors
    /// Token storage overflows or exceeds the cumulative input/item byte ceilings.
    pub(super) fn new(tokens: Vec<Token>) -> Result<Self> {
        let mut budget = Budget::default();
        let token_bytes = tokens
            .capacity()
            .checked_mul(std::mem::size_of::<Token>())
            .ok_or("VRML token storage overflow")?;
        budget.charge(tokens.len(), 0, token_bytes)?;
        for token in &tokens {
            if let Token::Atom(text) | Token::String(text) = token {
                budget.charge(0, 0, text.capacity())?;
            }
        }
        Ok(Self {
            tokens,
            offset: 0,
            definitions: HashMap::new(),
            prototypes: vec![HashMap::new()],
            interface: None,
            declaring: Vec::new(),
            budget,
            expanded: HashMap::new(),
        })
    }

    /// Compare the current token without allocating an identifier copy.
    pub(super) fn is_atom(&self, name: &str) -> bool {
        matches!(self.tokens.get(self.offset), Some(Token::Atom(value)) if value == name)
    }

    /// Consume one quoted string after a checked allocation preflight.
    ///
    /// # Errors
    /// The next token is not quoted or its copied storage exceeds a budget.
    pub(super) fn string(&mut self) -> Result<String> {
        let Some(Token::String(text)) = self.tokens.get(self.offset) else {
            return Err(error("VRML typed field needs a quoted string"));
        };
        self.budget
            .charge(1, 0, text.len() + 2 * std::mem::size_of::<String>())?;
        let text = text.clone();
        self.offset += 1;
        Ok(text)
    }

    /// Find the closest completed prototype declaration in lexical scope.
    pub(super) fn prototype(&self, name: &str) -> Option<Rc<Prototype>> {
        self.prototypes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    /// Parse a local declaration or static ROUTE statement wherever statements are permitted.
    ///
    /// # Errors
    /// A declaration/route is malformed, unsupported or exceeds parser limits.
    pub(super) fn statement_keyword(&mut self, depth: usize) -> Result<bool> {
        if self.is_atom("PROTO") {
            self.offset += 1;
            self.prototype_definition(depth)?;
        } else if self.is_atom("EXTERNPROTO") {
            return Err(error("VRML external prototypes are not supported"));
        } else if self.is_atom("ROUTE") {
            self.offset += 1;
            self.atom()?;
            if self.atom()? != "TO" {
                return Err(error("invalid VRML ROUTE"));
            }
            self.atom()?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Parse a complete local declaration inside its separate DEF/USE and nested-PROTO scopes.
    ///
    /// # Errors
    /// Names, interface values, body syntax, scope references or resource limits are invalid.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep declaration, interface and lexical scope construction together."
    )]
    fn prototype_definition(&mut self, depth: usize) -> Result<()> {
        if depth > 256 {
            return Err(error("VRML prototype nesting limit exceeded"));
        }
        let name = self.atom()?;
        identifier(&name)?;
        if builtin(&name)
            || self
                .prototypes
                .last()
                .is_some_and(|scope| scope.contains_key(&name))
        {
            return Err(error("duplicate or built-in VRML prototype name"));
        }
        self.budget
            .charge(1, 1, std::mem::size_of::<Prototype>() + name.len() + 128)?;
        self.symbol('[')?;
        let caller_definitions = std::mem::take(&mut self.definitions);
        let caller_interface = self.interface.take();
        self.prototypes.push(HashMap::new());
        self.declaring.push(name.clone());
        let mut interface = Interface::new();
        while self.tokens.get(self.offset) != Some(&Token::Symbol(']')) {
            let access = Access::parse(&self.atom()?)?;
            let ty = FieldType::parse(&self.atom()?)?;
            let member = self.atom()?;
            identifier(&member)?;
            self.budget
                .charge(1, 0, 4 * std::mem::size_of::<(String, Declaration)>())?;
            let default = if access.initializes() {
                Some(self.typed_value(ty, depth + 1)?)
            } else {
                None
            };
            if interface
                .insert(
                    member,
                    Declaration {
                        ty,
                        access,
                        default,
                    },
                )
                .is_some()
            {
                return Err(error("duplicate VRML prototype interface declaration"));
            }
        }
        self.symbol(']')?;
        validate_interface_names(&interface, &mut self.budget)?;
        self.symbol('{')?;
        let interface = Rc::new(interface);
        self.interface = Some(interface.clone());
        let mut body = Vec::new();
        while self.tokens.get(self.offset) != Some(&Token::Symbol('}')) {
            if self.statement_keyword(depth + 1)? {
                continue;
            }
            if body.is_empty() && self.is_atom("USE") {
                return Err(error("VRML prototype first body node cannot be USE"));
            }
            self.budget
                .charge(1, 0, 2 * std::mem::size_of::<Rc<Node>>())?;
            self.budget.reserve(&mut body, 1)?;
            body.push(self.node(depth + 1)?);
        }
        self.symbol('}')?;
        if body.is_empty() {
            return Err(error("VRML prototype needs a first body node"));
        }
        self.prototypes.pop();
        self.declaring.pop();
        self.definitions = caller_definitions;
        self.interface = caller_interface;
        let prototype = Rc::new(Prototype { interface, body });
        self.prototypes
            .last_mut()
            .ok_or("VRML prototype scope is missing")?
            .insert(name, prototype);
        Ok(())
    }

    /// Read a standard typed field with exactly one element or an ordered MF list.
    ///
    /// # Errors
    /// Element syntax, declared cardinality, numeric ranges or parser limits are invalid.
    pub(super) fn typed_value(&mut self, ty: FieldType, depth: usize) -> Result<Value> {
        if depth > 256 {
            return Err(error("VRML typed value nesting limit exceeded"));
        }
        if ty.multiple {
            let bracketed = self.tokens.get(self.offset) == Some(&Token::Symbol('['));
            if bracketed {
                self.offset += 1;
            }
            let mut values = Vec::new();
            loop {
                if bracketed && self.tokens.get(self.offset) == Some(&Token::Symbol(']')) {
                    self.offset += 1;
                    break;
                }
                self.budget.charge(1, 0, 2 * std::mem::size_of::<Value>())?;
                self.budget.reserve(&mut values, 1)?;
                values.push(self.typed_single(ty.element, depth + 1)?);
                if !bracketed {
                    break;
                }
            }
            Ok(Value::List(values))
        } else {
            self.typed_single(ty.element, depth + 1)
        }
    }

    /// Read one exactly sized elemental value while validating its scalar representation.
    ///
    /// # Errors
    /// Scalar syntax, type range, referenced node or allocation budget is invalid.
    fn typed_single(&mut self, element: Element, depth: usize) -> Result<Value> {
        if element == Element::Node {
            return if self.is_atom("NULL") {
                self.offset += 1;
                Ok(Value::Null)
            } else {
                self.node(depth).map(Value::Node)
            };
        }
        if element == Element::String {
            return self.string().map(|text| Value::Strings(vec![text]));
        }
        if element == Element::Image {
            return self.image();
        }
        let count = match element {
            Element::Color | Element::Vec3 => 3,
            Element::Rotation => 4,
            Element::Vec2 => 2,
            _ => 1,
        };
        self.budget
            .charge(1, 0, count * std::mem::size_of::<String>())?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            let text = self.atom()?;
            if element == Element::Bool {
                if !matches!(text.as_str(), "TRUE" | "FALSE") {
                    return Err(error("VRML SFBool must be TRUE or FALSE"));
                }
                values.push(text);
            } else if element == Element::Int {
                values.push(integer(&text)?.to_string());
            } else {
                let value: f64 = text.parse()?;
                if !value.is_finite()
                    || (element != Element::Time && !text.parse::<f32>()?.is_finite())
                {
                    return Err(error(
                        "VRML prototype number is non-finite or outside its field type",
                    ));
                }
                if element == Element::Color && !(0.0..=1.0).contains(&value) {
                    return Err(error("VRML prototype colour is outside zero to one"));
                }
                values.push(text);
            }
        }
        Ok(Value::Atoms(values))
    }

    /// Read bounded `SFImage` dimensions and exactly their required unsigned packed pixels.
    ///
    /// # Errors
    /// Dimensions, components, pixel values, required data or storage limits are invalid.
    fn image(&mut self) -> Result<Value> {
        let mut values = Vec::new();
        self.budget.reserve(&mut values, 3)?;
        let mut dimensions = [0_usize; 3];
        for dimension in &mut dimensions {
            let text = self.atom()?;
            *dimension = usize::try_from(integer(&text)?)?;
            values.push(text);
        }
        let [width, height, components] = dimensions;
        let count = width
            .checked_mul(height)
            .ok_or("VRML SFImage pixel count overflow")?;
        if count > MAX_ITEMS
            || (count != 0 && !(1..=4).contains(&components))
            || (count == 0 && components > 4)
        {
            return Err(error(
                "invalid or oversized VRML SFImage dimensions/components",
            ));
        }
        self.budget.charge(count, 0, 0)?;
        self.budget.reserve(&mut values, count)?;
        let maximum = if components == 4 {
            u32::MAX
        } else {
            (1_u32 << (components * 8)) - 1
        };
        for _ in 0..count {
            let text = self.atom()?;
            let pixel = unsigned_integer(&text)?;
            if pixel > u64::from(maximum) {
                return Err(error("VRML SFImage pixel exceeds its component width"));
            }
            values.push(text);
        }
        Ok(Value::Atoms(values))
    }

    /// Validate exact type/access-direction associations and record their occupied member channels.
    ///
    /// # Errors
    /// The interface/member is missing, its type/access mismatches, associations overlap or limits fail.
    pub(super) fn binding(
        &mut self,
        kind: &str,
        prototype: Option<&Prototype>,
        name: &str,
        associated: &mut HashSet<(String, u8)>,
    ) -> Result<Value> {
        self.offset += 1; // IS is already recognized by the node parser.
        let target = self.atom()?;
        let interface = self
            .interface
            .as_ref()
            .ok_or("VRML IS outside a prototype implementation")?;
        let (ty, access) = interface_signature(interface, &target)
            .ok_or("VRML IS references an unknown interface declaration")?;
        self.budget.charge(1, 0, name.len() + 64)?;
        let member = member(kind, prototype, name)?;
        if member.ty != ty || !member.access.accepts(access) {
            return Err(error(
                "VRML IS type or access category does not match its declaration",
            ));
        }
        let channels = if member.access == Access::Exposed {
            access
        } else {
            member.access
        };
        for &channel in channels.channels() {
            self.budget.charge(1, 0, member.canonical.len() + 64)?;
            if !associated.insert((member.canonical.clone(), channel)) {
                return Err(error("VRML IS associations overlap on one member channel"));
            }
        }
        Ok(Value::Binding {
            name: target,
            initialize: member.access.initializes() && access.initializes(),
        })
    }
}

/// Resolve declared interface names before their implicit exposed-field input/output aliases.
fn interface_signature(interface: &Interface, name: &str) -> Option<(FieldType, Access)> {
    if let Some(declaration) = interface.get(name) {
        return Some((declaration.ty, declaration.access));
    }
    for (base, access) in [
        (name.strip_prefix("set_"), Access::Input),
        (name.strip_suffix("_changed"), Access::Output),
    ] {
        if let Some(declaration) = base.and_then(|base| interface.get(base))
            && declaration.access == Access::Exposed
        {
            return Some((declaration.ty, access));
        }
    }
    None
}

/// Check that explicit declarations and implicit exposed-field events have unique interface names.
///
/// # Errors
/// A generated event name overlaps another name or its bounded registry storage cannot be reserved.
fn validate_interface_names(interface: &Interface, budget: &mut Budget) -> Result<()> {
    let mut names: HashSet<Cow<'_, str>> = HashSet::new();
    for (name, declaration) in interface {
        for (prefix, suffix, generated) in
            [("", "", false), ("set_", "", true), ("", "_changed", true)]
        {
            if generated && declaration.access != Access::Exposed {
                continue;
            }
            let length = prefix
                .len()
                .checked_add(name.len())
                .and_then(|length| length.checked_add(suffix.len()))
                .ok_or("VRML prototype interface name size overflow")?;
            // Four entry slots plus metadata cover geometric hash-table growth before reservation.
            let storage = (4 * std::mem::size_of::<Cow<'_, str>>() + 128)
                .checked_add(if generated { length } else { 0 })
                .ok_or("VRML prototype interface registry size overflow")?;
            budget.charge(1, 0, storage)?;
            names.try_reserve(1)?;
            let occupied = if generated {
                let mut alias = String::new();
                alias.try_reserve_exact(length)?;
                alias.push_str(prefix);
                alias.push_str(name);
                alias.push_str(suffix);
                Cow::Owned(alias)
            } else {
                Cow::Borrowed(name.as_str())
            };
            if !names.insert(occupied) {
                return Err(error("VRML prototype exposed-field event names overlap"));
            }
        }
    }
    Ok(())
}

/// Validate the normative identifier restrictions on local interface and type names.
///
/// # Errors
/// The identifier is empty, reserved or contains a prohibited character.
fn identifier(name: &str) -> Result<()> {
    let invalid = |c: char| {
        c.is_control()
            || matches!(
                c,
                ' ' | '"' | '\'' | '#' | ',' | '.' | '[' | ']' | '\\' | '{' | '}'
            )
    };
    if name.is_empty()
        || name.chars().any(invalid)
        || name.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-'))
        || matches!(
            name,
            "DEF"
                | "EXTERNPROTO"
                | "FALSE"
                | "IS"
                | "NULL"
                | "PROTO"
                | "ROUTE"
                | "TO"
                | "TRUE"
                | "USE"
                | "eventIn"
                | "eventOut"
                | "exposedField"
                | "field"
        )
    {
        return Err(error("invalid VRML prototype identifier"));
    }
    Ok(())
}

/// Read an unsigned decimal or hexadecimal magnitude without accepting floating syntax.
///
/// # Errors
/// The magnitude is malformed, negative or exceeds the unsigned integer range.
fn unsigned_integer(text: &str) -> Result<u64> {
    let text = text.strip_prefix('+').unwrap_or(text);
    if let Some(digits) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Ok(u64::from_str_radix(digits, 16)?)
    } else {
        Ok(text.parse()?)
    }
}

/// Read signed decimal/hexadecimal `SFInt32`, including full unsigned hexadecimal bit patterns.
///
/// # Errors
/// Integer syntax or magnitude lies outside a signed decimal or 32-bit hexadecimal representation.
fn integer(text: &str) -> Result<i32> {
    let negative = text.starts_with('-');
    let magnitude = unsigned_integer(text.strip_prefix('-').unwrap_or(text))?;
    if negative {
        let value = i64::try_from(magnitude)?
            .checked_neg()
            .ok_or("VRML integer overflow")?;
        Ok(i32::try_from(value)?)
    } else if text.trim_start_matches('+').starts_with("0x")
        || text.trim_start_matches('+').starts_with("0X")
    {
        Ok(i32::from_ne_bytes(u32::try_from(magnitude)?.to_ne_bytes()))
    } else {
        Ok(i32::try_from(magnitude)?)
    }
}

/// Whether this name is one of the standard built-in node types.
pub(super) fn builtin(kind: &str) -> bool {
    kind == "Script" || BUILTIN_FIELDS.iter().any(|&(node, _, _, _)| node == kind)
}

/// Resolve an exact body member, including synthesized exposed-field event aliases.
///
/// # Errors
/// The node member is unknown or its standard type/access signature is unsupported.
fn member(kind: &str, prototype: Option<&Prototype>, name: &str) -> Result<Member> {
    let lookup = |field: &str| -> Result<Option<(FieldType, Access)>> {
        if let Some(prototype) = prototype {
            return Ok(prototype
                .interface
                .get(field)
                .map(|value| (value.ty, value.access)));
        }
        BUILTIN_FIELDS
            .iter()
            .find(|&&(node, _, _, field_name)| node == kind && field_name == field)
            .map(|&(_, access, ty, _)| Ok((FieldType::parse(ty)?, Access::parse(access)?)))
            .transpose()
    };
    if let Some((ty, access)) = lookup(name)? {
        return Ok(Member {
            ty,
            access,
            canonical: name.to_owned(),
        });
    }
    for (base, access) in [
        (name.strip_prefix("set_"), Access::Input),
        (name.strip_suffix("_changed"), Access::Output),
    ] {
        if let Some(base) = base
            && let Some((ty, Access::Exposed)) = lookup(base)?
        {
            return Ok(Member {
                ty,
                access,
                canonical: base.to_owned(),
            });
        }
    }
    Err(error(format!(
        "unknown VRML {kind} member {name} in IS association"
    )))
}

/// Copies already resolved field payloads after preflighting their complete deep allocation cost.
///
/// # Errors
/// An unresolved association, excessive nesting, count overflow or allocation budget prevents copying.
fn copy_value(value: &Value, budget: &mut Budget, depth: usize) -> Result<Value> {
    let (items, bytes) = value_cost(value, depth)?;
    budget.charge(items, 0, bytes)?;
    Ok(value.clone())
}

/// Compute plain-value deep-copy cost without copying shared node references or allocating.
///
/// # Errors
/// A value has unresolved IS data, excessive depth or overflowing size/count arithmetic.
fn value_cost(value: &Value, depth: usize) -> Result<(usize, usize)> {
    if depth > 256 {
        return Err(error("VRML value-copy nesting limit exceeded"));
    }
    let mut items = 1_usize;
    let mut bytes = std::mem::size_of::<Value>();
    match value {
        Value::Atoms(values) | Value::Strings(values) => {
            for text in values {
                items = items
                    .checked_add(1)
                    .ok_or("VRML value-copy count overflow")?;
                bytes = bytes
                    .checked_add(text.len())
                    .and_then(|n| n.checked_add(std::mem::size_of::<String>()))
                    .ok_or("VRML value-copy size overflow")?;
            }
        }
        Value::List(values) => {
            for value in values {
                let (count, size) = value_cost(value, depth + 1)?;
                items = items
                    .checked_add(count)
                    .ok_or("VRML value-copy count overflow")?;
                bytes = bytes
                    .checked_add(size)
                    .ok_or("VRML value-copy size overflow")?;
            }
        }
        Value::Binding { .. } => return Err(error("unresolved VRML IS value cannot be copied")),
        Value::Node(_) | Value::Null => {}
    }
    Ok((items, bytes))
}

/// Structurally expand one caller graph while preserving its DEF/USE identities.
///
/// # Errors
/// Template references, static bindings, body recursion or cumulative expansion limits are invalid.
pub(super) fn expand(node: &Rc<Node>, memo: &mut Memo, budget: &mut Budget) -> Result<Rc<Node>> {
    expand_node(node, &HashMap::new(), memo, budget, &mut Vec::new(), 0)
}

/// Expand an ordinary node or one prototype using an instance-local copy memo and binding map.
///
/// # Errors
/// Fields cannot be resolved or their copied nodes/values exceed cumulative expansion limits.
fn expand_node(
    node: &Rc<Node>,
    bindings: &HashMap<String, Value>,
    memo: &mut Memo,
    budget: &mut Budget,
    active: &mut Vec<usize>,
    depth: usize,
) -> Result<Rc<Node>> {
    if depth > 256 {
        return Err(error("VRML prototype expansion depth limit exceeded"));
    }
    let identity = Rc::as_ptr(node) as usize;
    if let Some((_, node)) = memo.get(&identity) {
        return Ok(node.clone());
    }
    let expanded = if let Some(prototype) = &node.prototype {
        expand_instance(node, prototype, bindings, memo, budget, active, depth + 1)?
    } else {
        let bytes = node
            .fields
            .len()
            .checked_mul(4 * std::mem::size_of::<(String, Value)>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Node>() + node.kind.len() + 128))
            .ok_or("VRML node-copy size overflow")?;
        budget.charge(1, 1, bytes)?;
        let mut fields = HashMap::with_capacity(node.fields.len());
        for (name, value) in &node.fields {
            if matches!(
                value,
                Value::Binding {
                    initialize: false,
                    ..
                }
            ) {
                continue;
            }
            budget.charge(1, 0, name.len())?;
            let value = expand_value(value, bindings, memo, budget, active, depth + 1)?;
            fields.insert(name.clone(), value);
        }
        Rc::new(Node {
            kind: node.kind.clone(),
            fields,
            prototype: None,
        })
    };
    budget.charge(
        1,
        0,
        4 * std::mem::size_of::<(usize, (Rc<Node>, Rc<Node>))>() + 16,
    )?;
    memo.insert(identity, (node.clone(), expanded.clone()));
    Ok(expanded)
}

/// Resolve caller overrides before entering the body, so finite caller nesting is not body recursion.
///
/// # Errors
/// Overrides/defaults cannot be resolved, the body is recursive/empty or resource limits fail.
#[allow(
    clippy::too_many_arguments,
    reason = "Keep caller identity, lexical binding, recursion and resource state distinct."
)]
fn expand_instance(
    node: &Node,
    prototype: &Rc<Prototype>,
    bindings: &HashMap<String, Value>,
    caller: &mut Memo,
    budget: &mut Budget,
    active: &mut Vec<usize>,
    depth: usize,
) -> Result<Rc<Node>> {
    let mut overrides = HashMap::new();
    budget.charge(
        node.fields.len(),
        0,
        node.fields
            .len()
            .checked_mul(4 * std::mem::size_of::<(String, Value)>())
            .ok_or("VRML interface-copy size overflow")?,
    )?;
    for (name, value) in &node.fields {
        if matches!(
            value,
            Value::Binding {
                initialize: false,
                ..
            }
        ) {
            continue;
        }
        let value = expand_value(value, bindings, caller, budget, active, depth + 1)?;
        budget.charge(1, 0, name.len())?;
        overrides.insert(name.clone(), value);
    }
    let identity = Rc::as_ptr(prototype) as usize;
    if active.contains(&identity) {
        return Err(error("recursive VRML prototype body expansion"));
    }
    active.push(identity);
    let mut body_memo = HashMap::new();
    let mut resolved = HashMap::new();
    budget.charge(
        prototype.interface.len(),
        0,
        prototype
            .interface
            .len()
            .checked_mul(4 * std::mem::size_of::<(String, Value)>())
            .ok_or("VRML prototype defaults size overflow")?,
    )?;
    for (name, declaration) in prototype.interface.iter() {
        if !declaration.access.initializes() {
            continue;
        }
        let value = if let Some(value) = overrides.remove(name) {
            value
        } else {
            expand_value(
                declaration
                    .default
                    .as_ref()
                    .ok_or("VRML field default is missing")?,
                &HashMap::new(),
                &mut body_memo,
                budget,
                active,
                depth + 1,
            )?
        };
        budget.charge(1, 0, name.len())?;
        resolved.insert(name.clone(), value);
    }
    let mut first = None;
    for body in &prototype.body {
        let node = expand_node(body, &resolved, &mut body_memo, budget, active, depth + 1)?;
        if first.is_none() {
            first = Some(node);
        }
    }
    active.pop();
    first.ok_or_else(|| error("VRML prototype has no first body node"))
}

/// Resolve static bindings and deep-copy plain values while expanding template-owned node graphs.
///
/// # Errors
/// A static association is missing, an event has no initial value or copy/expansion limits fail.
fn expand_value(
    value: &Value,
    bindings: &HashMap<String, Value>,
    memo: &mut Memo,
    budget: &mut Budget,
    active: &mut Vec<usize>,
    depth: usize,
) -> Result<Value> {
    if depth > 256 {
        return Err(error("VRML prototype value expansion depth limit exceeded"));
    }
    match value {
        Value::Binding {
            name,
            initialize: true,
        } => copy_value(
            bindings
                .get(name)
                .ok_or("VRML static IS value is missing")?,
            budget,
            depth,
        ),
        Value::Binding {
            initialize: false, ..
        } => Err(error("VRML event binding has no static field value")),
        Value::Node(node) => {
            expand_node(node, bindings, memo, budget, active, depth + 1).map(Value::Node)
        }
        Value::List(values) => {
            budget.charge(
                values.len(),
                0,
                values
                    .len()
                    .checked_mul(std::mem::size_of::<Value>())
                    .ok_or("VRML list-copy size overflow")?,
            )?;
            let mut copied = Vec::with_capacity(values.len());
            for value in values {
                copied.push(expand_value(
                    value,
                    bindings,
                    memo,
                    budget,
                    active,
                    depth + 1,
                )?);
            }
            Ok(Value::List(copied))
        }
        Value::Atoms(_) | Value::Strings(_) | Value::Null => copy_value(value, budget, depth),
    }
}

/// Normative VRML97 built-in interfaces; strings identify types/access, not executable templates.
/// <https://www.web3d.org/documents/specifications/14772/V2.0/part1/nodesRef.html>
const BUILTIN_FIELDS: &[(&str, &str, &str, &str)] = &[
    ("Anchor", "eventIn", "MFNode", "addChildren"),
    ("Anchor", "eventIn", "MFNode", "removeChildren"),
    ("Anchor", "exposedField", "MFNode", "children"),
    ("Anchor", "exposedField", "SFString", "description"),
    ("Anchor", "exposedField", "MFString", "parameter"),
    ("Anchor", "exposedField", "MFString", "url"),
    ("Anchor", "field", "SFVec3f", "bboxCenter"),
    ("Anchor", "field", "SFVec3f", "bboxSize"),
    ("Appearance", "exposedField", "SFNode", "material"),
    ("Appearance", "exposedField", "SFNode", "texture"),
    ("Appearance", "exposedField", "SFNode", "textureTransform"),
    ("AudioClip", "exposedField", "SFString", "description"),
    ("AudioClip", "exposedField", "SFBool", "loop"),
    ("AudioClip", "exposedField", "SFFloat", "pitch"),
    ("AudioClip", "exposedField", "SFTime", "startTime"),
    ("AudioClip", "exposedField", "SFTime", "stopTime"),
    ("AudioClip", "exposedField", "MFString", "url"),
    ("AudioClip", "eventOut", "SFTime", "duration_changed"),
    ("AudioClip", "eventOut", "SFBool", "isActive"),
    ("Background", "eventIn", "SFBool", "set_bind"),
    ("Background", "exposedField", "MFFloat", "groundAngle"),
    ("Background", "exposedField", "MFColor", "groundColor"),
    ("Background", "exposedField", "MFString", "backUrl"),
    ("Background", "exposedField", "MFString", "bottomUrl"),
    ("Background", "exposedField", "MFString", "frontUrl"),
    ("Background", "exposedField", "MFString", "leftUrl"),
    ("Background", "exposedField", "MFString", "rightUrl"),
    ("Background", "exposedField", "MFString", "topUrl"),
    ("Background", "exposedField", "MFFloat", "skyAngle"),
    ("Background", "exposedField", "MFColor", "skyColor"),
    ("Background", "eventOut", "SFBool", "isBound"),
    ("Billboard", "eventIn", "MFNode", "addChildren"),
    ("Billboard", "eventIn", "MFNode", "removeChildren"),
    ("Billboard", "exposedField", "SFVec3f", "axisOfRotation"),
    ("Billboard", "exposedField", "MFNode", "children"),
    ("Billboard", "field", "SFVec3f", "bboxCenter"),
    ("Billboard", "field", "SFVec3f", "bboxSize"),
    ("Box", "field", "SFVec3f", "size"),
    ("Collision", "eventIn", "MFNode", "addChildren"),
    ("Collision", "eventIn", "MFNode", "removeChildren"),
    ("Collision", "exposedField", "MFNode", "children"),
    ("Collision", "exposedField", "SFBool", "collide"),
    ("Collision", "field", "SFVec3f", "bboxCenter"),
    ("Collision", "field", "SFVec3f", "bboxSize"),
    ("Collision", "field", "SFNode", "proxy"),
    ("Collision", "eventOut", "SFTime", "collideTime"),
    ("Color", "exposedField", "MFColor", "color"),
    ("ColorInterpolator", "eventIn", "SFFloat", "set_fraction"),
    ("ColorInterpolator", "exposedField", "MFFloat", "key"),
    ("ColorInterpolator", "exposedField", "MFColor", "keyValue"),
    ("ColorInterpolator", "eventOut", "SFColor", "value_changed"),
    ("Cone", "field", "SFFloat", "bottomRadius"),
    ("Cone", "field", "SFFloat", "height"),
    ("Cone", "field", "SFBool", "side"),
    ("Cone", "field", "SFBool", "bottom"),
    ("Coordinate", "exposedField", "MFVec3f", "point"),
    (
        "CoordinateInterpolator",
        "eventIn",
        "SFFloat",
        "set_fraction",
    ),
    ("CoordinateInterpolator", "exposedField", "MFFloat", "key"),
    (
        "CoordinateInterpolator",
        "exposedField",
        "MFVec3f",
        "keyValue",
    ),
    (
        "CoordinateInterpolator",
        "eventOut",
        "MFVec3f",
        "value_changed",
    ),
    ("Cylinder", "field", "SFBool", "bottom"),
    ("Cylinder", "field", "SFFloat", "height"),
    ("Cylinder", "field", "SFFloat", "radius"),
    ("Cylinder", "field", "SFBool", "side"),
    ("Cylinder", "field", "SFBool", "top"),
    ("CylinderSensor", "exposedField", "SFBool", "autoOffset"),
    ("CylinderSensor", "exposedField", "SFFloat", "diskAngle"),
    ("CylinderSensor", "exposedField", "SFBool", "enabled"),
    ("CylinderSensor", "exposedField", "SFFloat", "maxAngle"),
    ("CylinderSensor", "exposedField", "SFFloat", "minAngle"),
    ("CylinderSensor", "exposedField", "SFFloat", "offset"),
    ("CylinderSensor", "eventOut", "SFBool", "isActive"),
    (
        "CylinderSensor",
        "eventOut",
        "SFRotation",
        "rotation_changed",
    ),
    (
        "CylinderSensor",
        "eventOut",
        "SFVec3f",
        "trackPoint_changed",
    ),
    (
        "DirectionalLight",
        "exposedField",
        "SFFloat",
        "ambientIntensity",
    ),
    ("DirectionalLight", "exposedField", "SFColor", "color"),
    ("DirectionalLight", "exposedField", "SFVec3f", "direction"),
    ("DirectionalLight", "exposedField", "SFFloat", "intensity"),
    ("DirectionalLight", "exposedField", "SFBool", "on"),
    ("ElevationGrid", "eventIn", "MFFloat", "set_height"),
    ("ElevationGrid", "exposedField", "SFNode", "color"),
    ("ElevationGrid", "exposedField", "SFNode", "normal"),
    ("ElevationGrid", "exposedField", "SFNode", "texCoord"),
    ("ElevationGrid", "field", "MFFloat", "height"),
    ("ElevationGrid", "field", "SFBool", "ccw"),
    ("ElevationGrid", "field", "SFBool", "colorPerVertex"),
    ("ElevationGrid", "field", "SFFloat", "creaseAngle"),
    ("ElevationGrid", "field", "SFBool", "normalPerVertex"),
    ("ElevationGrid", "field", "SFBool", "solid"),
    ("ElevationGrid", "field", "SFInt32", "xDimension"),
    ("ElevationGrid", "field", "SFFloat", "xSpacing"),
    ("ElevationGrid", "field", "SFInt32", "zDimension"),
    ("ElevationGrid", "field", "SFFloat", "zSpacing"),
    ("Extrusion", "eventIn", "MFVec2f", "set_crossSection"),
    ("Extrusion", "eventIn", "MFRotation", "set_orientation"),
    ("Extrusion", "eventIn", "MFVec2f", "set_scale"),
    ("Extrusion", "eventIn", "MFVec3f", "set_spine"),
    ("Extrusion", "field", "SFBool", "beginCap"),
    ("Extrusion", "field", "SFBool", "ccw"),
    ("Extrusion", "field", "SFBool", "convex"),
    ("Extrusion", "field", "SFFloat", "creaseAngle"),
    ("Extrusion", "field", "MFVec2f", "crossSection"),
    ("Extrusion", "field", "SFBool", "endCap"),
    ("Extrusion", "field", "MFRotation", "orientation"),
    ("Extrusion", "field", "MFVec2f", "scale"),
    ("Extrusion", "field", "SFBool", "solid"),
    ("Extrusion", "field", "MFVec3f", "spine"),
    ("Fog", "exposedField", "SFColor", "color"),
    ("Fog", "exposedField", "SFString", "fogType"),
    ("Fog", "exposedField", "SFFloat", "visibilityRange"),
    ("Fog", "eventIn", "SFBool", "set_bind"),
    ("Fog", "eventOut", "SFBool", "isBound"),
    ("FontStyle", "field", "MFString", "family"),
    ("FontStyle", "field", "SFBool", "horizontal"),
    ("FontStyle", "field", "MFString", "justify"),
    ("FontStyle", "field", "SFString", "language"),
    ("FontStyle", "field", "SFBool", "leftToRight"),
    ("FontStyle", "field", "SFFloat", "size"),
    ("FontStyle", "field", "SFFloat", "spacing"),
    ("FontStyle", "field", "SFString", "style"),
    ("FontStyle", "field", "SFBool", "topToBottom"),
    ("Group", "eventIn", "MFNode", "addChildren"),
    ("Group", "eventIn", "MFNode", "removeChildren"),
    ("Group", "exposedField", "MFNode", "children"),
    ("Group", "field", "SFVec3f", "bboxCenter"),
    ("Group", "field", "SFVec3f", "bboxSize"),
    ("ImageTexture", "exposedField", "MFString", "url"),
    ("ImageTexture", "field", "SFBool", "repeatS"),
    ("ImageTexture", "field", "SFBool", "repeatT"),
    ("IndexedFaceSet", "eventIn", "MFInt32", "set_colorIndex"),
    ("IndexedFaceSet", "eventIn", "MFInt32", "set_coordIndex"),
    ("IndexedFaceSet", "eventIn", "MFInt32", "set_normalIndex"),
    ("IndexedFaceSet", "eventIn", "MFInt32", "set_texCoordIndex"),
    ("IndexedFaceSet", "exposedField", "SFNode", "color"),
    ("IndexedFaceSet", "exposedField", "SFNode", "coord"),
    ("IndexedFaceSet", "exposedField", "SFNode", "normal"),
    ("IndexedFaceSet", "exposedField", "SFNode", "texCoord"),
    ("IndexedFaceSet", "field", "SFBool", "ccw"),
    ("IndexedFaceSet", "field", "MFInt32", "colorIndex"),
    ("IndexedFaceSet", "field", "SFBool", "colorPerVertex"),
    ("IndexedFaceSet", "field", "SFBool", "convex"),
    ("IndexedFaceSet", "field", "MFInt32", "coordIndex"),
    ("IndexedFaceSet", "field", "SFFloat", "creaseAngle"),
    ("IndexedFaceSet", "field", "MFInt32", "normalIndex"),
    ("IndexedFaceSet", "field", "SFBool", "normalPerVertex"),
    ("IndexedFaceSet", "field", "SFBool", "solid"),
    ("IndexedFaceSet", "field", "MFInt32", "texCoordIndex"),
    ("IndexedLineSet", "eventIn", "MFInt32", "set_colorIndex"),
    ("IndexedLineSet", "eventIn", "MFInt32", "set_coordIndex"),
    ("IndexedLineSet", "exposedField", "SFNode", "color"),
    ("IndexedLineSet", "exposedField", "SFNode", "coord"),
    ("IndexedLineSet", "field", "MFInt32", "colorIndex"),
    ("IndexedLineSet", "field", "SFBool", "colorPerVertex"),
    ("IndexedLineSet", "field", "MFInt32", "coordIndex"),
    ("Inline", "exposedField", "MFString", "url"),
    ("Inline", "field", "SFVec3f", "bboxCenter"),
    ("Inline", "field", "SFVec3f", "bboxSize"),
    ("LOD", "exposedField", "MFNode", "level"),
    ("LOD", "field", "SFVec3f", "center"),
    ("LOD", "field", "MFFloat", "range"),
    ("Material", "exposedField", "SFFloat", "ambientIntensity"),
    ("Material", "exposedField", "SFColor", "diffuseColor"),
    ("Material", "exposedField", "SFColor", "emissiveColor"),
    ("Material", "exposedField", "SFFloat", "shininess"),
    ("Material", "exposedField", "SFColor", "specularColor"),
    ("Material", "exposedField", "SFFloat", "transparency"),
    ("MovieTexture", "exposedField", "SFBool", "loop"),
    ("MovieTexture", "exposedField", "SFFloat", "speed"),
    ("MovieTexture", "exposedField", "SFTime", "startTime"),
    ("MovieTexture", "exposedField", "SFTime", "stopTime"),
    ("MovieTexture", "exposedField", "MFString", "url"),
    ("MovieTexture", "field", "SFBool", "repeatS"),
    ("MovieTexture", "field", "SFBool", "repeatT"),
    ("MovieTexture", "eventOut", "SFTime", "duration_changed"),
    ("MovieTexture", "eventOut", "SFBool", "isActive"),
    ("NavigationInfo", "eventIn", "SFBool", "set_bind"),
    ("NavigationInfo", "exposedField", "MFFloat", "avatarSize"),
    ("NavigationInfo", "exposedField", "SFBool", "headlight"),
    ("NavigationInfo", "exposedField", "SFFloat", "speed"),
    ("NavigationInfo", "exposedField", "MFString", "type"),
    (
        "NavigationInfo",
        "exposedField",
        "SFFloat",
        "visibilityLimit",
    ),
    ("NavigationInfo", "eventOut", "SFBool", "isBound"),
    ("Normal", "exposedField", "MFVec3f", "vector"),
    ("NormalInterpolator", "eventIn", "SFFloat", "set_fraction"),
    ("NormalInterpolator", "exposedField", "MFFloat", "key"),
    ("NormalInterpolator", "exposedField", "MFVec3f", "keyValue"),
    ("NormalInterpolator", "eventOut", "MFVec3f", "value_changed"),
    (
        "OrientationInterpolator",
        "eventIn",
        "SFFloat",
        "set_fraction",
    ),
    ("OrientationInterpolator", "exposedField", "MFFloat", "key"),
    (
        "OrientationInterpolator",
        "exposedField",
        "MFRotation",
        "keyValue",
    ),
    (
        "OrientationInterpolator",
        "eventOut",
        "SFRotation",
        "value_changed",
    ),
    ("PixelTexture", "exposedField", "SFImage", "image"),
    ("PixelTexture", "field", "SFBool", "repeatS"),
    ("PixelTexture", "field", "SFBool", "repeatT"),
    ("PlaneSensor", "exposedField", "SFBool", "autoOffset"),
    ("PlaneSensor", "exposedField", "SFBool", "enabled"),
    ("PlaneSensor", "exposedField", "SFVec2f", "maxPosition"),
    ("PlaneSensor", "exposedField", "SFVec2f", "minPosition"),
    ("PlaneSensor", "exposedField", "SFVec3f", "offset"),
    ("PlaneSensor", "eventOut", "SFBool", "isActive"),
    ("PlaneSensor", "eventOut", "SFVec3f", "trackPoint_changed"),
    ("PlaneSensor", "eventOut", "SFVec3f", "translation_changed"),
    ("PointLight", "exposedField", "SFFloat", "ambientIntensity"),
    ("PointLight", "exposedField", "SFVec3f", "attenuation"),
    ("PointLight", "exposedField", "SFColor", "color"),
    ("PointLight", "exposedField", "SFFloat", "intensity"),
    ("PointLight", "exposedField", "SFVec3f", "location"),
    ("PointLight", "exposedField", "SFBool", "on"),
    ("PointLight", "exposedField", "SFFloat", "radius"),
    ("PointSet", "exposedField", "SFNode", "color"),
    ("PointSet", "exposedField", "SFNode", "coord"),
    ("PositionInterpolator", "eventIn", "SFFloat", "set_fraction"),
    ("PositionInterpolator", "exposedField", "MFFloat", "key"),
    (
        "PositionInterpolator",
        "exposedField",
        "MFVec3f",
        "keyValue",
    ),
    (
        "PositionInterpolator",
        "eventOut",
        "SFVec3f",
        "value_changed",
    ),
    ("ProximitySensor", "exposedField", "SFVec3f", "center"),
    ("ProximitySensor", "exposedField", "SFVec3f", "size"),
    ("ProximitySensor", "exposedField", "SFBool", "enabled"),
    ("ProximitySensor", "eventOut", "SFBool", "isActive"),
    ("ProximitySensor", "eventOut", "SFVec3f", "position_changed"),
    (
        "ProximitySensor",
        "eventOut",
        "SFRotation",
        "orientation_changed",
    ),
    ("ProximitySensor", "eventOut", "SFTime", "enterTime"),
    ("ProximitySensor", "eventOut", "SFTime", "exitTime"),
    ("ScalarInterpolator", "eventIn", "SFFloat", "set_fraction"),
    ("ScalarInterpolator", "exposedField", "MFFloat", "key"),
    ("ScalarInterpolator", "exposedField", "MFFloat", "keyValue"),
    ("ScalarInterpolator", "eventOut", "SFFloat", "value_changed"),
    ("Shape", "exposedField", "SFNode", "appearance"),
    ("Shape", "exposedField", "SFNode", "geometry"),
    ("Sound", "exposedField", "SFVec3f", "direction"),
    ("Sound", "exposedField", "SFFloat", "intensity"),
    ("Sound", "exposedField", "SFVec3f", "location"),
    ("Sound", "exposedField", "SFFloat", "maxBack"),
    ("Sound", "exposedField", "SFFloat", "maxFront"),
    ("Sound", "exposedField", "SFFloat", "minBack"),
    ("Sound", "exposedField", "SFFloat", "minFront"),
    ("Sound", "exposedField", "SFFloat", "priority"),
    ("Sound", "exposedField", "SFNode", "source"),
    ("Sound", "field", "SFBool", "spatialize"),
    ("Sphere", "field", "SFFloat", "radius"),
    ("SphereSensor", "exposedField", "SFBool", "autoOffset"),
    ("SphereSensor", "exposedField", "SFBool", "enabled"),
    ("SphereSensor", "exposedField", "SFRotation", "offset"),
    ("SphereSensor", "eventOut", "SFBool", "isActive"),
    ("SphereSensor", "eventOut", "SFRotation", "rotation_changed"),
    ("SphereSensor", "eventOut", "SFVec3f", "trackPoint_changed"),
    ("SpotLight", "exposedField", "SFFloat", "ambientIntensity"),
    ("SpotLight", "exposedField", "SFVec3f", "attenuation"),
    ("SpotLight", "exposedField", "SFFloat", "beamWidth"),
    ("SpotLight", "exposedField", "SFColor", "color"),
    ("SpotLight", "exposedField", "SFFloat", "cutOffAngle"),
    ("SpotLight", "exposedField", "SFVec3f", "direction"),
    ("SpotLight", "exposedField", "SFFloat", "intensity"),
    ("SpotLight", "exposedField", "SFVec3f", "location"),
    ("SpotLight", "exposedField", "SFBool", "on"),
    ("SpotLight", "exposedField", "SFFloat", "radius"),
    ("Switch", "exposedField", "MFNode", "choice"),
    ("Switch", "exposedField", "SFInt32", "whichChoice"),
    ("Text", "exposedField", "MFString", "string"),
    ("Text", "exposedField", "SFNode", "fontStyle"),
    ("Text", "exposedField", "MFFloat", "length"),
    ("Text", "exposedField", "SFFloat", "maxExtent"),
    ("TextureCoordinate", "exposedField", "MFVec2f", "point"),
    ("TextureTransform", "exposedField", "SFVec2f", "center"),
    ("TextureTransform", "exposedField", "SFFloat", "rotation"),
    ("TextureTransform", "exposedField", "SFVec2f", "scale"),
    ("TextureTransform", "exposedField", "SFVec2f", "translation"),
    ("TimeSensor", "exposedField", "SFTime", "cycleInterval"),
    ("TimeSensor", "exposedField", "SFBool", "enabled"),
    ("TimeSensor", "exposedField", "SFBool", "loop"),
    ("TimeSensor", "exposedField", "SFTime", "startTime"),
    ("TimeSensor", "exposedField", "SFTime", "stopTime"),
    ("TimeSensor", "eventOut", "SFTime", "cycleTime"),
    ("TimeSensor", "eventOut", "SFFloat", "fraction_changed"),
    ("TimeSensor", "eventOut", "SFBool", "isActive"),
    ("TimeSensor", "eventOut", "SFTime", "time"),
    ("TouchSensor", "exposedField", "SFBool", "enabled"),
    ("TouchSensor", "eventOut", "SFVec3f", "hitNormal_changed"),
    ("TouchSensor", "eventOut", "SFVec3f", "hitPoint_changed"),
    ("TouchSensor", "eventOut", "SFVec2f", "hitTexCoord_changed"),
    ("TouchSensor", "eventOut", "SFBool", "isActive"),
    ("TouchSensor", "eventOut", "SFBool", "isOver"),
    ("TouchSensor", "eventOut", "SFTime", "touchTime"),
    ("Transform", "eventIn", "MFNode", "addChildren"),
    ("Transform", "eventIn", "MFNode", "removeChildren"),
    ("Transform", "exposedField", "SFVec3f", "center"),
    ("Transform", "exposedField", "MFNode", "children"),
    ("Transform", "exposedField", "SFRotation", "rotation"),
    ("Transform", "exposedField", "SFVec3f", "scale"),
    (
        "Transform",
        "exposedField",
        "SFRotation",
        "scaleOrientation",
    ),
    ("Transform", "exposedField", "SFVec3f", "translation"),
    ("Transform", "field", "SFVec3f", "bboxCenter"),
    ("Transform", "field", "SFVec3f", "bboxSize"),
    ("Viewpoint", "eventIn", "SFBool", "set_bind"),
    ("Viewpoint", "exposedField", "SFFloat", "fieldOfView"),
    ("Viewpoint", "exposedField", "SFBool", "jump"),
    ("Viewpoint", "exposedField", "SFRotation", "orientation"),
    ("Viewpoint", "exposedField", "SFVec3f", "position"),
    ("Viewpoint", "field", "SFString", "description"),
    ("Viewpoint", "eventOut", "SFTime", "bindTime"),
    ("Viewpoint", "eventOut", "SFBool", "isBound"),
    ("VisibilitySensor", "exposedField", "SFVec3f", "center"),
    ("VisibilitySensor", "exposedField", "SFBool", "enabled"),
    ("VisibilitySensor", "exposedField", "SFVec3f", "size"),
    ("VisibilitySensor", "eventOut", "SFTime", "enterTime"),
    ("VisibilitySensor", "eventOut", "SFTime", "exitTime"),
    ("VisibilitySensor", "eventOut", "SFBool", "isActive"),
    ("WorldInfo", "field", "MFString", "info"),
    ("WorldInfo", "field", "SFString", "title"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse and structurally expand root nodes without imposing visible-geometry requirements.
    fn roots(source: &str) -> Result<Vec<Rc<Node>>> {
        let mut parser = Parser::new(super::super::tokenize(source)?)?;
        let mut nodes = Vec::new();
        while parser.offset < parser.tokens.len() {
            if parser.statement_keyword(0)? {
                continue;
            }
            let node = parser.node(0)?;
            nodes.push(expand(&node, &mut parser.expanded, &mut parser.budget)?);
        }
        Ok(nodes)
    }

    #[test]
    fn all_twenty_standard_interface_types_retain_typed_defaults() {
        let source = r#"PROTO All [
          field SFBool bool TRUE
          field SFColor color .1 .2 .3
          field SFFloat number .5
          field SFImage image 1 2 4 0xFF0000FF 0x00FF0080
          field SFInt32 integer -0xE20
          field SFNode node NULL
          field SFRotation rotation 0 1 0 1
          field SFString string "quoted \"word\" and \\ path"
          field SFTime time 1e100
          field SFVec2f vec2 1 2
          field SFVec3f vec3 1 2 3
          field MFColor colors [.1 .2 .3, .4 .5 .6]
          field MFFloat numbers [.5, 1]
          field MFInt32 integers [0xffffffff, -0X80000000]
          field MFNode nodes [NULL, Group {}]
          field MFRotation rotations [0 0 1 0, 0 1 0 1]
          field MFString strings ["a", "b"]
          field MFTime times [1e100, -1]
          field MFVec2f vec2s [1 2, 3 4]
          field MFVec3f vec3s [1 2 3, 4 5 6]
        ] { Group {} } All {}"#;
        let mut parser = Parser::new(super::super::tokenize(source).unwrap()).unwrap();
        assert!(parser.statement_keyword(0).unwrap());
        let prototype = parser.prototype("All").unwrap();
        assert_eq!(prototype.interface.len(), 20);
        assert!(matches!(prototype.interface["string"].default.as_ref(),
            Some(Value::Strings(values)) if values == &["quoted \"word\" and \\ path"]));
        assert!(matches!(prototype.interface["integer"].default.as_ref(),
            Some(Value::Atoms(values)) if values == &["-3616"]));
        assert!(matches!(prototype.interface["integers"].default.as_ref(),
            Some(Value::List(values)) if matches!(&values[0], Value::Atoms(items) if items == &["-1"])));
        assert_eq!(roots(source).unwrap()[0].kind, "Group");
        assert!(roots("PROTO A [field MFBool value []] {Group {}}").is_err());
        assert!(roots("PROTO A [field MFImage value []] {Group {}}").is_err());
    }

    #[test]
    fn defaults_overrides_and_first_body_effective_types_drive_real_geometry() {
        let source = r"#VRML V2.0 utf8
          PROTO Paint [field SFColor tone 1 0 0] {
            Material {diffuseColor IS tone}
            Shape {geometry Box {size 100 100 100}}
          }
          PROTO Geometry [field SFVec3f dimensions 2 2 2] {
            Box {size IS dimensions}
            Shape {geometry Box {size 100 100 100}}
          }
          Shape {
            appearance Appearance {material Paint {tone 0 1 0}}
            geometry Geometry {dimensions 4 2 6}
          }";
        let scene = super::super::vrml(source).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!(
            scene
                .triangles
                .iter()
                .all(|triangle| triangle.color == Some([0, 255, 0]))
        );
        let low = scene.positions().fold([f64::INFINITY; 3], |low, point| {
            [
                low[0].min(point.x),
                low[1].min(point.y),
                low[2].min(point.z),
            ]
        });
        let high = scene
            .positions()
            .fold([f64::NEG_INFINITY; 3], |high, point| {
                [
                    high[0].max(point.x),
                    high[1].max(point.y),
                    high[2].max(point.z),
                ]
            });
        assert_eq!(low.map(f64::to_bits), [-2_f64, -3., -1.].map(f64::to_bits));
        assert_eq!(high.map(f64::to_bits), [2_f64, 3., 1.].map(f64::to_bits));
    }

    #[test]
    fn instance_body_sharing_is_fresh_while_node_overrides_keep_caller_identity() {
        let source = r"
          PROTO Wrapper [field MFNode payload []] {
            Group {children [
              DEF Local Shape {geometry Box {size 1 1 1}}
              USE Local
              Group {children IS payload}
            ]}
          }
          DEF Caller Shape {geometry Box {size 2 2 2}}
          Wrapper {payload [DEF Override Shape {geometry Box {size 3 3 3}}]}
          Wrapper {payload [USE Caller]}
          USE Override";
        let nodes = roots(source).unwrap();
        let first = nodes[1].nodes("children").unwrap();
        let second = nodes[2].nodes("children").unwrap();
        assert!(Rc::ptr_eq(&first[0], &first[1]));
        assert!(Rc::ptr_eq(&second[0], &second[1]));
        assert!(!Rc::ptr_eq(&first[0], &second[0]));
        assert!(Rc::ptr_eq(
            &first[2].nodes("children").unwrap()[0],
            &nodes[3]
        ));
        assert!(Rc::ptr_eq(
            &second[2].nodes("children").unwrap()[0],
            &nodes[0]
        ));
        assert!(roots(&format!("{source} USE Local")).is_err());
        assert!(roots("DEF Outside Group {} PROTO P [] {Group {children USE Outside}}").is_err());
    }

    #[test]
    fn default_node_graphs_are_instance_owned_and_is_preserves_their_sharing() {
        let source = r"
          PROTO Pair [
            field SFNode single DEF Shared Shape {geometry Box {}}
            field MFNode many [USE Shared, USE Shared]
          ] {Group {children [Group {children IS many} Collision {proxy IS single}]}}
          Pair {} Pair {}";
        let nodes = roots(source).unwrap();
        let first = nodes[0].nodes("children").unwrap();
        let second = nodes[1].nodes("children").unwrap();
        let first_many = first[0].nodes("children").unwrap();
        let second_many = second[0].nodes("children").unwrap();
        assert!(Rc::ptr_eq(&first_many[0], &first_many[1]));
        assert!(Rc::ptr_eq(
            &first_many[0],
            &first[1].single("proxy").unwrap().unwrap()
        ));
        assert!(!Rc::ptr_eq(&first_many[0], &second_many[0]));
    }

    #[test]
    fn finite_same_prototype_caller_nesting_and_nested_lexical_definitions_are_valid() {
        let source = r"#VRML V2.0 utf8
          PROTO Wrap [exposedField MFNode children []] {
            Group {children IS children}
            TimeSensor {}
          }
          Wrap {children [Wrap {children [Shape {geometry Box {}}]}]}";
        assert_eq!(super::super::vrml(source).unwrap().triangles.len(), 12);
        let local = r"#VRML V2.0 utf8
          PROTO Outer [field SFVec3f dimensions 1 1 1] {
            PROTO Inner [field SFVec3f dimensions 2 2 2] {Box {size IS dimensions}}
            Inner {dimensions IS dimensions}
          }
          Shape {geometry Outer {dimensions 3 4 5}}";
        assert_eq!(super::super::vrml(local).unwrap().triangles.len(), 12);
        assert!(super::super::vrml(&format!("{local} Shape {{geometry Inner {{}}}}")).is_err());
        assert!(roots("PROTO P [] {P {}} P {}").is_err());
        assert!(roots("PROTO P [] {PROTO Q [] {P {}} Q {}}").is_err());
    }

    #[test]
    fn directional_is_rules_cover_all_sixteen_access_pairs_and_event_aliases() {
        let declarations = ["exposedField", "field", "eventIn", "eventOut"];
        let body = [
            "Transform {translation IS value}",
            "Box {size IS value}",
            "Transform {set_translation IS value}",
            "Transform {translation_changed IS value}",
        ];
        let expected = [
            [true, true, true, true],
            [false, true, false, false],
            [false, false, true, false],
            [false, false, false, true],
        ];
        for (row, body) in body.iter().enumerate() {
            for (column, declaration) in declarations.iter().enumerate() {
                let default = if column < 2 { "1 2 3" } else { "" };
                let source =
                    format!("PROTO P [{declaration} SFVec3f value {default}] {{{body}}} P {{}}");
                assert_eq!(roots(&source).is_ok(), expected[row][column], "{source}");
            }
        }
        let events = r"PROTO P [eventIn SFVec3f incoming eventOut SFVec3f outgoing] {
            Transform {translation IS incoming rotation 0 1 0 0 translation_changed IS outgoing}
          } P {}";
        assert!(!roots(events).unwrap()[0].fields.contains_key("translation"));
        assert!(
            roots("PROTO P [field SFColor value 1 0 0] {Transform {translation IS value}}")
                .is_err()
        );
        assert!(roots("PROTO P [field SFColor value 1 0 0] {Color {color IS value}}").is_err());
        assert!(roots("PROTO P [exposedField SFVec3f value 0 0 0 eventIn SFVec3f incoming] {Transform {translation IS value set_translation IS incoming}}").is_err());
        assert!(
            roots("PROTO P [eventIn SFFloat fraction] {ScalarInterpolator {fraction IS fraction}}")
                .is_err()
        );
        assert!(roots("Group {children IS anything}").is_err());
    }

    #[test]
    fn interface_event_aliases_validate_without_initializing_static_geometry() {
        let source = r"#VRML V2.0 utf8
          PROTO Motion [
            exposedField SFVec3f move 0 0 0
            exposedField SFFloat fraction .25
          ] {
            Transform {
              translation 3 4 5
              set_translation IS set_move
              translation_changed IS move_changed
              children [Shape {geometry Box {size 2 4 6}}]
            }
            ScalarInterpolator {
              set_fraction IS set_fraction
              value_changed IS fraction_changed
            }
          }
          Motion {move 100 200 300 fraction .75}";
        let nodes = roots(source).unwrap();
        assert_eq!(nodes[0].kind, "Transform");
        assert!(matches!(&nodes[0].fields["translation"],
            Value::Atoms(values) if values == &["3", "4", "5"]));
        assert!(!nodes[0].fields.contains_key("set_translation"));
        assert!(!nodes[0].fields.contains_key("translation_changed"));
        let scene = super::super::vrml(source).unwrap();
        let literal = super::super::vrml(
            r"#VRML V2.0 utf8
          Transform {translation 3 4 5 children [Shape {geometry Box {size 2 4 6}}]}",
        )
        .unwrap();
        assert_eq!(scene.triangles.len(), 12);
        let positions = |scene: &crate::scene::Scene| {
            scene
                .positions()
                .map(|point| [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()])
                .collect::<Vec<_>>()
        };
        assert_eq!(positions(&scene), positions(&literal));
        for invalid in [
            "ScalarInterpolator {set_fraction IS fraction_changed}",
            "ScalarInterpolator {value_changed IS set_fraction}",
            "Box {size IS set_move}",
            "ScalarInterpolator {set_fraction IS set_move}",
        ] {
            let source = format!(
                "PROTO P [exposedField SFVec3f move 0 0 0 exposedField SFFloat fraction 1] {{{invalid}}} P {{}}"
            );
            assert!(roots(&source).is_err(), "{source}");
        }
        assert!(roots("PROTO P [field SFFloat value 1] {ScalarInterpolator {set_fraction IS set_value}} P {}").is_err());
        assert!(
            roots("PROTO P [exposedField SFFloat value 1] {Group {}} P {set_value 2}").is_err()
        );
        assert!(roots("PROTO P [field SFFloat value 1 eventOut SFFloat set_value] {ScalarInterpolator {value_changed IS set_value}} P {}").is_ok());
        assert!(roots("PROTO P [exposedField SFFloat set_value 1] {ScalarInterpolator {value_changed IS set_value_changed}} P {}").is_ok());
        assert!(roots("PROTO Inner [exposedField SFFloat set_value 1] {ScalarInterpolator {set_fraction IS set_set_value value_changed IS set_value_changed}} PROTO Outer [eventOut SFFloat result] {Inner {set_value_changed IS result}} Outer {}").is_ok());
        assert!(roots("PROTO P [exposedField SFFloat set_value 1 exposedField SFFloat value_changed 2] {Group {}} P {}").is_err());
    }

    #[test]
    fn repeated_def_uses_closest_preceding_node_and_duplicate_fields_stay_undefined() {
        let nodes = roots("DEF Same Group {} USE Same DEF Same Transform {} USE Same").unwrap();
        assert!(Rc::ptr_eq(&nodes[0], &nodes[1]));
        assert!(Rc::ptr_eq(&nodes[2], &nodes[3]));
        assert!(!Rc::ptr_eq(&nodes[0], &nodes[2]));
        assert!(roots("DEF Same Group {children USE Same}").is_err());
        assert!(roots("Group {children [] children []}").is_err());
        assert!(roots("PROTO P [field SFFloat value 1 field SFFloat value 2] {Group {}}").is_err());
        assert!(
            roots("PROTO P [exposedField SFFloat value 1 eventIn SFFloat set_value] {Group {}}")
                .is_err()
        );
        assert!(roots("PROTO P [] {} P {}").is_err());
        assert!(roots("PROTO P [] {Group {}} PROTO P [] {Group {}}").is_err());
        assert!(roots("PROTO Box [] {Group {}}").is_err());
    }

    #[test]
    fn ordinary_node_body_statements_keep_the_enclosing_file_prototype_scope() {
        let source = r"#VRML V2.0 utf8
          Group {
            PROTO Geometry [field SFVec3f dimensions 2 2 2] {Box {size IS dimensions}}
            children [
              DEF Clock TimeSensor {}
              DEF Scalar ScalarInterpolator {}
              Shape {geometry Geometry {}}
            ]
            ROUTE Clock.fraction_changed TO Scalar.set_fraction
          }
          Shape {geometry Geometry {dimensions 3 4 5}}";
        assert_eq!(super::super::vrml(source).unwrap().triangles.len(), 24);
    }

    #[test]
    fn malformed_typed_values_unknown_overrides_and_unsupported_carriers_fail() {
        for (ty, value) in [
            ("SFBool", "1"),
            ("SFVec3f", "1 2"),
            ("SFColor", "1 2 3"),
            ("SFFloat", "1e100"),
            ("SFTime", "NaN"),
            ("SFInt32", "2147483648"),
            ("SFInt32", "0x100000000"),
            ("SFString", "unquoted"),
            ("MFVec2f", "[1 2 3]"),
            ("SFImage", "1 1 1 0x100"),
            ("SFImage", "20000000 20000000 4"),
        ] {
            assert!(
                roots(&format!(
                    "PROTO P [field {ty} value {value}] {{Group {{}}}}"
                ))
                .is_err(),
                "{ty} {value}"
            );
        }
        assert!(roots("PROTO P [field SFVec3f value 1 2 3] {Group {}} P {value 1 2}").is_err());
        assert!(roots("PROTO P [field SFFloat value 1] {Group {}} P {unknown 2}").is_err());
        assert!(roots("PROTO P [eventIn SFFloat value] {Group {}} P {value 2}").is_err());
        assert!(roots("EXTERNPROTO P [] \"remote.wrl\"").is_err());
        assert!(roots("PROTO P [] {Script {}}").is_err());
        assert!(roots("PROTO P [] {Unknown {}}").is_err());
    }

    #[test]
    fn copy_preflights_are_cumulative_checked_and_do_not_require_large_allocations() {
        let value = Value::Atoms(vec!["1".into(), "2".into(), "3".into()]);
        let mut budget = Budget {
            items: MAX_ITEMS,
            ..Default::default()
        };
        assert!(copy_value(&value, &mut budget, 0).is_err());
        assert_eq!(budget.items, MAX_ITEMS);
        let mut budget = Budget {
            bytes: usize::try_from(super::super::super::MAX_INPUT_BYTES).unwrap(),
            ..Default::default()
        };
        assert!(copy_value(&value, &mut budget, 0).is_err());
        let mut budget = Budget {
            nodes: MAX_NODES,
            ..Default::default()
        };
        let node = Rc::new(Node {
            kind: "Group".into(),
            fields: HashMap::new(),
            prototype: None,
        });
        assert!(expand(&node, &mut HashMap::new(), &mut budget).is_err());
        let mut budget = Budget::default();
        assert!(budget.charge(usize::MAX, 0, 0).is_err());
        assert!(budget.charge(0, 0, usize::MAX).is_err());
        let mut nested = Value::Null;
        for _ in 0..258 {
            nested = Value::List(vec![nested]);
        }
        assert!(copy_value(&nested, &mut Budget::default(), 0).is_err());
    }

    #[test]
    fn persistent_expansion_memo_retains_raw_nodes_until_identity_keys_are_removed() {
        let mut memo = Memo::new();
        let mut budget = Budget::default();
        let first = Rc::new(Node {
            kind: "Group".into(),
            fields: HashMap::new(),
            prototype: None,
        });
        let identity = Rc::as_ptr(&first) as usize;
        let source = Rc::downgrade(&first);
        let expanded = expand(&first, &mut memo, &mut budget).unwrap();
        drop(first);
        let retained = source
            .upgrade()
            .expect("memo must retain its raw source identity");
        assert!(Rc::ptr_eq(&memo[&identity].0, &retained));
        assert!(Rc::ptr_eq(&memo[&identity].1, &expanded));
        let nodes = roots("Group {} Transform {} WorldInfo {} Group {}").unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.kind.as_str())
                .collect::<Vec<_>>(),
            ["Group", "Transform", "WorldInfo", "Group"]
        );
        drop(retained);
        memo.clear();
        assert!(source.upgrade().is_none());
    }

    #[test]
    fn lexical_and_flat_value_capacity_limits_fail_before_buffer_growth() {
        let bytes = usize::try_from(super::super::super::MAX_INPUT_BYTES).unwrap();
        let mut storage = super::super::LexicalStorage { bytes };
        let mut tokens = Vec::new();
        assert!(storage.token(&mut tokens, Token::Symbol('[')).is_err());
        assert_eq!(tokens.capacity(), 0);
        let mut text = String::new();
        assert!(storage.character(&mut text, 'é').is_err());
        assert_eq!(text.capacity(), 0);
        let mut budget = Budget {
            bytes,
            ..Default::default()
        };
        let mut values: Vec<String> = Vec::new();
        assert!(budget.reserve(&mut values, 3).is_err());
        assert_eq!(values.capacity(), 0);
        assert!(budget.reserve(&mut values, MAX_ITEMS + 1).is_err());
        let tokens = super::super::tokenize(r#"Group {label "α \"β\" \\ path"}"#).unwrap();
        assert!(matches!(&tokens[3], Token::String(value) if value == "α \"β\" \\ path"));
    }
}
