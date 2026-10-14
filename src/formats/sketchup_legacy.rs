//! Independently framed legacy SketchUp MFC archives and native half-edge surfaces.

use super::{citygml::triangulate_rings, transform::Transform};
use crate::{
    Result,
    scene::{AlphaMode, Line, Scene, Triangle, TriangleAppearance, Vec3},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

/// Maximum archive bytes admitted before object-map allocation.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Maximum archive slots, list elements and emitted primitives.
const MAX_ITEMS: usize = 1_000_000;
/// Maximum recursive object, attribute and placement depth.
const MAX_DEPTH: usize = 128;

/// Visibility and paint references in the native drawing-element base.
#[derive(Clone, Copy, Default)]
struct Drawing {
    /// Native front material reference, absent for default paint.
    material: Option<usize>,
    /// Native layer reference, absent for the default layer.
    layer: Option<usize>,
    /// Native hidden flag.
    hidden: bool,
}

/// Addressed contents of the model or one shared component definition.
#[derive(Clone)]
struct Container {
    /// Visible container state shared by its child elements.
    drawing: Drawing,
    /// Explicit root or definition entity-list entries.
    entities: Vec<usize>,
    /// Layer selected for drawing, which remains visible despite its hidden flag.
    active_layer: Option<usize>,
}

/// Geometry and referenced resources decoded at one native archive slot.
#[derive(Clone)]
enum Object {
    /// Framed non-geometric metadata retained in the global archive map.
    Auxiliary,
    /// Native vertex coordinates in inches.
    Vertex(Vec3),
    /// Native endpoints and drawing flags.
    Edge([usize; 2], Drawing),
    /// Half-edge object, reversal flag, and its parent-loop back-reference.
    Use(usize, bool, usize),
    /// Ordered half-edge boundary and native outer-loop marker.
    Loop(Vec<usize>, bool),
    /// Surface loops, plane normal and native paint/visibility.
    Face(Vec<usize>, Vec3, Drawing),
    /// Layer visibility.
    Layer(bool),
    /// Diffuse material in linear color, with opacity.
    Material([f64; 4]),
    /// Shared definition contents and camera-facing behavior flag.
    Definition(Container, bool),
    /// Definition placement and inherited paint.
    Instance(usize, Transform, Drawing),
}

/// One shared MFC map stores both runtime-class declarations and objects.
enum Slot {
    /// Class name and its independently declared schema number.
    Class(String, u16),
    /// Reserved object slot during recursive decoding of its children.
    Pending,
    /// Completed object body.
    Object(Object),
}

/// Cumulative output and work accounting across every placed definition visit.
struct Expansion {
    /// Final placed model surfaces and visible wires.
    scene: Scene,
    /// Definitions currently on the occurrence stack, for cycle detection.
    stack: BTreeSet<usize>,
    /// Definition visits, including those emitting no primitives.
    visits: usize,
}

/// Forward-only bounded archive and its native global reference map.
struct Archive<'a> {
    /// Original admitted bytes.
    data: &'a [u8],
    /// Exact next byte offset, with no signature search or resynchronization.
    offset: usize,
    /// Producer-declared schema inventory from the first archive object.
    versions: BTreeMap<String, u32>,
    /// Native 1-based MFC store map, including runtime classes.
    slots: Vec<Slot>,
    /// Whether this archive stores presence-masked entity persistent ids.
    pids: bool,
    /// Original monotonic admission time.
    started: Instant,
}

/// Load addressed legacy geometry after walking its complete native archive prefix.
///
/// # Errors
/// Unsupported versions/classes/schemas, malformed native references or topology,
/// unresolved placements, active sections, and exceeded budgets return errors.
pub(super) fn load(data: &[u8]) -> Result<Scene> {
    if data.len() > MAX_BYTES {
        return Err("legacy SketchUp archive exceeds 64 MiB".into());
    }
    let mut archive = Archive {
        data,
        offset: 0,
        versions: BTreeMap::new(),
        slots: vec![Slot::Pending],
        pids: false,
        started: Instant::now(),
    };
    if archive.string()? != "SketchUp Model" {
        return Err("invalid legacy SketchUp document identity".into());
    }
    let version = archive.string()?;
    let major = version
        .strip_prefix('{')
        .and_then(|v| v.split('.').next())
        .ok_or("invalid legacy SketchUp version")?
        .parse::<u32>()?;
    if !(13..=17).contains(&major) {
        return Err("unsupported legacy SketchUp version".into());
    }
    archive.pids = major >= 17;
    archive.take(16)?;
    archive.string()?;
    archive.u32()?;
    archive.expect_object("CVersionMap", 0)?;
    if archive.versions.get("CEntity").copied() != Some(if archive.pids { 5 } else { 3 }) {
        return Err("unsupported legacy SketchUp entity schema".into());
    }
    archive.take(if archive.pids { 20 } else { 12 })?;
    archive.expect_object("CDib", 0)?;
    archive.take(if archive.pids { 15 } else { 14 })?;
    archive.string()?;
    if archive.u32()? != 2 {
        return Err("unsupported legacy SketchUp option framing".into());
    }
    for _ in 0..archive.count()? {
        archive.string()?;
        archive.dictionary(0)?;
    }
    archive.expect_object("CAttributeContainer", 0)?;
    archive.expect_object("CDib", 0)?;
    archive.expect_object("CCamera", 0)?;
    archive.preamble(0)?;
    archive.take(206)?;
    if archive.take(8)? != [0, 0, 0, 0, 1, 0, 0, 0] {
        return Err("invalid legacy SketchUp model boundary".into());
    }
    let root = archive.container(0)?;
    let mut expansion = Expansion {
        scene: Scene::default(),
        stack: BTreeSet::new(),
        visits: 0,
    };
    archive.emit(&root, Transform::IDENTITY, None, &mut expansion, 0)?;
    let scene = expansion.scene;
    if scene.triangles.is_empty() && scene.lines.is_empty() {
        return Err("legacy SketchUp model has no visible geometry".into());
    }
    Ok(scene)
}

impl<'a> Archive<'a> {
    /// Enforce cumulative archive work, object depth and elapsed time.
    fn check(&self, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH || self.slots.len() > MAX_ITEMS {
            return Err("legacy SketchUp archive exceeds object/depth limits".into());
        }
        if self.started.elapsed() > Duration::from_secs(30) {
            return Err("legacy SketchUp import exceeds 30 second limit".into());
        }
        Ok(())
    }

    /// Consume an exact checked extent from the native archive.
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or("legacy SketchUp offset overflow")?;
        let data = self
            .data
            .get(self.offset..end)
            .ok_or("truncated legacy SketchUp archive")?;
        self.offset = end;
        Ok(data)
    }

    /// Read a single native byte.
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    /// Read a little-endian native word.
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    /// Read a little-endian native integer.
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    /// Read one finite little-endian native coordinate.
    fn float(&mut self) -> Result<f64> {
        let value = f64::from_le_bytes(self.take(8)?.try_into()?);
        if !value.is_finite() {
            return Err("non-finite legacy SketchUp coordinate".into());
        }
        Ok(value)
    }
    /// Read a bounded native list or payload element count.
    fn count(&mut self) -> Result<usize> {
        let count = usize::try_from(self.u32()?)?;
        if count > MAX_ITEMS {
            return Err("legacy SketchUp list exceeds element limit".into());
        }
        Ok(count)
    }
    /// Read a finite native three-component point.
    fn point(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.float()?, self.float()?, self.float()?))
    }

    /// Decode a native MFC Unicode string with its escalating length encoding.
    fn string(&mut self) -> Result<String> {
        if self.take(3)? != [0xff, 0xfe, 0xff] {
            return Err("invalid legacy SketchUp Unicode string".into());
        }
        let first = self.u8()?;
        let count = if first == 255 {
            let word = self.u16()?;
            if word == u16::MAX {
                usize::try_from(self.u32()?)?
            } else {
                usize::from(word)
            }
        } else {
            usize::from(first)
        };
        if count > MAX_ITEMS {
            return Err("legacy SketchUp string exceeds limit".into());
        }
        let units = self
            .take(count * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes(*v))
            .collect::<Vec<_>>();
        Ok(String::from_utf16(&units)?)
    }

    /// Require one completed native resource by its recorded archive slot.
    fn object_at(&self, id: usize) -> Result<&Object> {
        match self.slots.get(id) {
            Some(Slot::Object(object)) => Ok(object),
            _ => Err("unresolved or mistyped legacy SketchUp object reference".into()),
        }
    }

    /// Require a non-null object pointer at a qualified resource position.
    fn required(&mut self, depth: usize) -> Result<usize> {
        self.object(depth)?
            .ok_or_else(|| "null legacy SketchUp required object reference".into())
    }

    /// Read an expected metadata object, checking its declared runtime class.
    fn expect_object(&mut self, name: &str, depth: usize) -> Result<()> {
        let position = self.offset;
        let class_name = self.pointer_class()?;
        self.offset = position;
        if class_name != name {
            return Err(format!("legacy SketchUp expected {name}, found {class_name}").into());
        }
        self.required(depth)?;
        Ok(())
    }

    /// Peek the class identity of a new metadata pointer without assigning map slots.
    fn pointer_class(&mut self) -> Result<String> {
        let tag = self.u16()?;
        if tag == 0xffff {
            self.u16()?;
            let length = usize::from(self.u16()?);
            return Ok(String::from_utf8(self.take(length)?.to_vec())?);
        }
        if tag & 0x8000 == 0 {
            return Err("legacy SketchUp expected a new metadata object".into());
        }
        match self.slots.get(usize::from(tag & 0x7fff)) {
            Some(Slot::Class(name, _)) => Ok(name.clone()),
            _ => Err("legacy SketchUp metadata class reference is invalid".into()),
        }
    }

    /// Decode an object pointer with the single native map used by the entire archive.
    fn object(&mut self, depth: usize) -> Result<Option<usize>> {
        self.check(depth)?;
        let short = self.u16()?;
        if short == 0 {
            return Ok(None);
        }
        let (tag, class_bit) = if short == 0x7fff {
            (self.u32()?, 0x8000_0000)
        } else {
            (u32::from(short), 0x8000)
        };
        let (name, schema) = if short == 0xffff {
            let schema = self.u16()?;
            let length = usize::from(self.u16()?);
            if !(1..=128).contains(&length) {
                return Err("invalid legacy SketchUp runtime class name".into());
            }
            let name = String::from_utf8(self.take(length)?.to_vec())?;
            let definition_exception = name == "CComponentDefinition"
                && schema == 11
                && self.versions.get(&name).copied() == Some(10);
            if name != "CVersionMap"
                && !definition_exception
                && self.versions.get(&name).copied() != Some(u32::from(schema))
            {
                return Err(format!("unqualified legacy SketchUp schema for {name}").into());
            }
            self.slots.push(Slot::Class(name.clone(), schema));
            (name, schema)
        } else if tag & class_bit != 0 {
            match self.slots.get(usize::try_from(tag & !class_bit)?) {
                Some(Slot::Class(name, schema)) => (name.clone(), *schema),
                _ => {
                    return Err(
                        "legacy SketchUp class reference is out of range or mistyped".into(),
                    );
                }
            }
        } else {
            let index = usize::try_from(tag)?;
            if !matches!(self.slots.get(index), Some(Slot::Object(_) | Slot::Pending)) {
                return Err("legacy SketchUp object reference is out of range or mistyped".into());
            }
            return Ok(Some(index));
        };
        let id = self.slots.len();
        self.slots.push(Slot::Pending);
        let object = self.body(&name, schema, id, depth + 1)?;
        self.slots[id] = Slot::Object(object);
        Ok(Some(id))
    }

    /// Consume entity attributes and the version-qualified presence-masked persistent id.
    fn preamble(&mut self, depth: usize) -> Result<()> {
        self.object(depth + 1)?;
        self.pid()
    }

    /// Consume only the version-qualified persistent-id encoding.
    fn pid(&mut self) -> Result<()> {
        if self.pids {
            let mask = self.u8()?;
            self.take(usize::try_from(mask.count_ones())?)?;
        }
        Ok(())
    }

    /// Decode the native paint, visibility and layer reference fields.
    fn drawing(&mut self, depth: usize) -> Result<Drawing> {
        let material = self.object(depth + 1)?;
        let flags = self.take(6)?;
        if flags.iter().any(|v| *v > 1) {
            return Err("invalid legacy SketchUp drawing flags".into());
        }
        let layer = self.object(depth + 1)?;
        Ok(Drawing {
            material,
            layer,
            hidden: flags[0] != 0,
        })
    }

    /// Frame all manager lists, shared definitions and direct entities of one container.
    fn container(&mut self, depth: usize) -> Result<Container> {
        self.check(depth)?;
        self.preamble(depth)?;
        let drawing = self.drawing(depth)?;
        self.take(if self.pids { 3 } else { 2 })?;
        for _ in 0..self.count()? {
            self.required(depth + 1)?;
        }
        self.object(depth + 1)?;
        self.take(if self.pids { 3 } else { 2 })?;
        for _ in 0..self.count()? {
            self.required(depth + 1)?;
        }
        let active_layer = self.object(depth + 1)?;
        for _ in 0..self.count()? {
            self.required(depth + 1)?;
        }
        let count = self.count()?;
        let mut entities = Vec::with_capacity(count);
        for _ in 0..count {
            entities.push(self.required(depth + 1)?);
        }
        for _ in 0..self.count()? {
            self.required(depth + 1)?;
        }
        if self.object(depth + 1)?.is_some() {
            return Err("legacy SketchUp active section clipping is unsupported".into());
        }
        Ok(Container {
            drawing,
            entities,
            active_layer,
        })
    }

    /// Walk a named dictionary's typed key/value entries through its empty key terminator.
    fn dictionary(&mut self, depth: usize) -> Result<()> {
        self.check(depth)?;
        for _ in 0..MAX_ITEMS {
            if self.string()?.is_empty() {
                return Ok(());
            }
            let kind = self.u8()?;
            self.variant(kind, depth + 1)?;
        }
        Err("legacy SketchUp dictionary exceeds its entry limit".into())
    }

    /// Frame bounded native attribute variants, including recursively typed arrays.
    fn variant(&mut self, kind: u8, depth: usize) -> Result<()> {
        self.check(depth)?;
        match kind {
            0 => {}
            4 | 5 | 9 => {
                self.take(4)?;
            }
            6 | 12 => {
                self.take(8)?;
            }
            7 => {
                self.take(1)?;
            }
            10 => {
                self.string()?;
            }
            11 => {
                for _ in 0..self.count()? {
                    let kind = self.u8()?;
                    self.variant(kind, depth + 1)?;
                }
            }
            17 | 18 => {
                self.take(24)?;
            }
            _ => {
                return Err(format!("unsupported legacy SketchUp attribute variant {kind}").into());
            }
        }
        Ok(())
    }

    /// Dispatch only schema-qualified native bodies with known framing.
    #[allow(
        clippy::too_many_lines,
        reason = "Native class body framing stays in one checked dispatch"
    )]
    fn body(&mut self, name: &str, schema: u16, id: usize, depth: usize) -> Result<Object> {
        match (name, schema) {
            ("CVersionMap", 0) => {
                for _ in 0..256 {
                    let name = self.string()?;
                    let version = self.u32()?;
                    if name == "End-Of-Version-Map" {
                        return Ok(Object::Auxiliary);
                    }
                    if self.versions.insert(name, version).is_some() {
                        return Err("duplicate legacy SketchUp schema inventory class".into());
                    }
                }
                return Err("legacy SketchUp version inventory exceeds limit".into());
            }
            ("CDib", 3) => {
                let kind = self.u32()?;
                let length = usize::try_from(self.u32()?)?;
                if length > MAX_BYTES {
                    return Err("legacy SketchUp image metadata extent exceeds limit".into());
                }
                self.take(length)?;
                if kind == 1 {
                    self.u32()?;
                } else if !matches!(kind, 2 | 3 | 4 | 7) {
                    return Err("unsupported legacy SketchUp embedded image subtype".into());
                }
            }
            ("CAttributeContainer", 0) => {
                self.preamble(depth)?;
                while self.object(depth + 1)?.is_some() {
                    self.check(depth)?;
                }
            }
            ("CAttributeNamed", 1) => {
                self.preamble(depth)?;
                self.take(4)?;
                self.string()?;
                self.dictionary(depth)?;
                self.u32()?;
            }
            ("CCamera", 5) => {
                self.take(137)?;
                self.u16()?;
                self.string()?;
                self.take(33)?;
            }
            ("CMaterial", 12) => return self.material(depth),
            ("CLayer", 2) => {
                self.preamble(depth)?;
                self.string()?;
                let hidden = self.u8()?;
                self.u16()?;
                self.pid()?;
                self.string()?;
                self.take(6)?;
                self.string()?;
                self.take(21)?;
                if hidden > 1 {
                    return Err("invalid legacy SketchUp layer visibility".into());
                }
                return Ok(Object::Layer(hidden != 0));
            }
            ("CComponentDefinition", 10 | 11) => {
                let container = self.container(depth)?;
                self.take(16)?;
                self.string()?;
                self.string()?;
                self.string()?;
                self.take(4 + 1 + 24 + if self.pids { 3 } else { 2 })?;
                self.take(2 + 4)?;
                let behavior = self.u8()?;
                self.take(8)?;
                self.expect_object("CThumbnail", depth)?;
                return Ok(Object::Definition(container, behavior & 1 != 0));
            }
            ("CThumbnail", 1) => {
                self.preamble(depth)?;
                self.expect_object("CCamera", depth)?;
                self.object(depth + 1)?;
            }
            ("CVertex", 0) => {
                self.preamble(depth)?;
                return Ok(Object::Vertex(self.point()?));
            }
            ("CEdge", 2) => {
                self.preamble(depth)?;
                let drawing = self.drawing(depth)?;
                let vertices = [self.required(depth + 1)?, self.required(depth + 1)?];
                self.object(depth + 1)?;
                return Ok(Object::Edge(vertices, drawing));
            }
            ("CCurve", 4) => {
                self.preamble(depth)?;
                self.take(1)?;
                self.count()?;
            }
            ("CArcCurve", 3) => {
                self.preamble(depth)?;
                self.take(1)?;
                self.count()?;
                for _ in 0..14 {
                    self.float()?;
                }
            }
            ("CFace", 3) => {
                self.preamble(depth)?;
                let drawing = self.drawing(depth)?;
                let normal = self.point()?;
                self.float()?;
                if (normal.length() - 1.).abs() > 1e-5 {
                    return Err("invalid legacy SketchUp face normal".into());
                }
                let count = self.count()?;
                let mut loops = Vec::with_capacity(count);
                for _ in 0..count {
                    loops.push(self.required(depth + 1)?);
                }
                self.object(depth + 1)?;
                return Ok(Object::Face(loops, normal, drawing));
            }
            ("CLoop", 1) => {
                self.preamble(depth)?;
                let outer = self.u8()?;
                let convex = self.u8()?;
                if outer > 1 || convex > 1 {
                    return Err("invalid legacy SketchUp loop flags".into());
                }
                let mut uses = Vec::new();
                while let Some(id) = self.object(depth + 1)? {
                    uses.push(id);
                    if uses.len() > MAX_ITEMS {
                        return Err("excessive legacy SketchUp loop size".into());
                    }
                }
                return Ok(Object::Loop(uses, outer != 0));
            }
            ("CEdgeUse", 1) => {
                self.preamble(depth)?;
                let edge = self.required(depth + 1)?;
                let direction = self.u8()?;
                let parent = self.required(depth + 1)?;
                if direction > 1 {
                    return Err("invalid legacy SketchUp half-edge direction".into());
                }
                if !matches!(
                    self.slots.get(parent),
                    Some(Slot::Pending | Slot::Object(Object::Loop(_, _)))
                ) {
                    return Err("invalid legacy SketchUp half-edge parent".into());
                }
                return Ok(Object::Use(edge, direction != 0, parent));
            }
            ("CFaceTextureCoords", 4) => {
                self.preamble(depth)?;
                self.u32()?;
                for _ in 0..24 {
                    self.float()?;
                }
                for _ in 0..2 {
                    let count = self.count()?;
                    self.take(count * 32)?;
                }
                self.take(8)?;
            }
            ("CComponentInstance", 5) | ("CGroup", 1) => {
                self.preamble(depth)?;
                let drawing = self.drawing(depth)?;
                let definition = self.required(depth + 1)?;
                let mut transform = Transform::IDENTITY;
                for row in 0..3 {
                    for column in 0..3 {
                        transform.0[column][row] = self.float()?;
                    }
                }
                let p = self.point()?;
                transform.0[3] = [p.x, p.y, p.z, 1.];
                if self.float()?.to_bits() != 1_f64.to_bits() {
                    return Err("unsupported legacy SketchUp transform weight".into());
                }
                self.string()?;
                self.take(16)?;
                return Ok(Object::Instance(definition, transform, drawing));
            }
            ("CRelationship", 0) => {
                self.preamble(depth)?;
                self.required(depth + 1)?;
                self.required(depth + 1)?;
            }
            _ => {
                return Err(format!(
                    "unsupported legacy SketchUp native class {name} schema {schema} at slot {id}"
                )
                .into());
            }
        }
        Ok(Object::Auxiliary)
    }

    /// Decode solid or textured material framing while retaining its authored diffuse paint.
    fn material(&mut self, depth: usize) -> Result<Object> {
        self.preamble(depth)?;
        self.string()?;
        let texture = self.u8()?;
        let color = if texture == 1 {
            self.object(depth + 1)?;
            self.u8()?;
            self.object(depth + 1)?;
            self.float()?;
            self.float()?;
            self.string()?;
            let color = self.take(4)?.try_into()?;
            self.take(5)?;
            color
        } else if texture == 0 {
            self.u8()?;
            self.take(4)?.try_into()?
        } else {
            return Err("invalid legacy SketchUp material texture flag".into());
        };
        self.string()?;
        self.take(8)?;
        let transparency = self.float()?;
        let use_transparency = self.u8()?;
        if !(0. ..=1.).contains(&transparency) || use_transparency > 1 {
            return Err("invalid legacy SketchUp material transparency".into());
        }
        let [red, green, blue, _]: [u8; 4] = color;
        let linear = |v: u8| {
            let encoded = f64::from(v) / 255.;
            if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        };
        Ok(Object::Material([
            linear(red),
            linear(green),
            linear(blue),
            if use_transparency == 1 {
                1. - transparency
            } else {
                1.
            },
        ]))
    }

    /// Resolve paint and layer types before deciding whether a native element is visible.
    fn visible(&self, drawing: Drawing, active_layer: Option<usize>) -> Result<bool> {
        let hidden = if let Some(layer) = drawing.layer {
            match self.object_at(layer)? {
                Object::Layer(hidden) => *hidden && Some(layer) != active_layer,
                _ => return Err("mistyped legacy SketchUp layer reference".into()),
            }
        } else {
            false
        };
        Ok(!drawing.hidden && !hidden)
    }

    /// Resolve a native vertex pointer in the original model's inch coordinate system.
    fn vertex(&self, id: usize) -> Result<Vec3> {
        match self.object_at(id)? {
            Object::Vertex(v) => Ok(*v),
            _ => Err("mistyped legacy SketchUp vertex reference".into()),
        }
    }

    /// Expand only placed native entities, preserving boundaries and per-instance transforms.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep native half-edge cycle and instance expansion checks together"
    )]
    fn emit(
        &self,
        container: &Container,
        transform: Transform,
        inherited: Option<usize>,
        expansion: &mut Expansion,
        depth: usize,
    ) -> Result<()> {
        self.check(depth)?;
        expansion.visits += 1;
        if expansion.visits > MAX_ITEMS {
            return Err("legacy SketchUp instance expansion exceeds visit limit".into());
        }
        if !self.visible(container.drawing, container.active_layer)? {
            return Ok(());
        }
        let mut surface_edges = BTreeSet::new();
        for &entity in &container.entities {
            self.check(depth)?;
            if let Object::Face(loops, normal, drawing) = self.object_at(entity)? {
                let mut rings = Vec::new();
                for (index, &loop_id) in loops.iter().enumerate() {
                    let Object::Loop(uses, outer) = self.object_at(loop_id)? else {
                        return Err("mistyped legacy SketchUp face loop".into());
                    };
                    if *outer != (index == 0) || uses.len() < 3 {
                        return Err("invalid legacy SketchUp face loop order".into());
                    }
                    let mut endpoints = Vec::new();
                    let mut points = Vec::new();
                    for &use_id in uses {
                        let Object::Use(edge_id, reversed, parent) = self.object_at(use_id)? else {
                            return Err("mistyped legacy SketchUp half-edge reference".into());
                        };
                        if *parent != loop_id {
                            return Err("legacy SketchUp half-edge has wrong loop parent".into());
                        }
                        let Object::Edge(vertices, _) = self.object_at(*edge_id)? else {
                            return Err("mistyped legacy SketchUp face edge".into());
                        };
                        surface_edges.insert(*edge_id);
                        let [start, end] = if *reversed {
                            [vertices[1], vertices[0]]
                        } else {
                            *vertices
                        };
                        endpoints.push([start, end]);
                        points.push(self.vertex(start)?);
                    }
                    if !(0..endpoints.len())
                        .all(|i| endpoints[i][1] == endpoints[(i + 1) % endpoints.len()][0])
                    {
                        return Err(
                            "legacy SketchUp half-edge boundary is not a closed cycle".into()
                        );
                    }
                    rings.push(points);
                }
                if !self.visible(*drawing, container.active_layer)? {
                    continue;
                }
                let material = drawing
                    .material
                    .or(inherited)
                    .map(|id| match self.object_at(id)? {
                        Object::Material(color) => Ok::<_, crate::Error>(*color),
                        _ => Err("mistyped legacy SketchUp material reference".into()),
                    })
                    .transpose()?;
                let columns: [Vec3; 3] = std::array::from_fn(|column| {
                    Vec3::new(
                        transform.0[column][0],
                        transform.0[column][1],
                        transform.0[column][2],
                    )
                });
                let mirrored = columns[0].cross(columns[1]).dot(columns[2]) < 0.;
                for mut vertices in triangulate_rings(&rings)? {
                    if (vertices[1] - vertices[0])
                        .cross(vertices[2] - vertices[0])
                        .dot(*normal)
                        < 0.
                    {
                        vertices.swap(1, 2);
                    }
                    if mirrored {
                        vertices.swap(1, 2);
                    }
                    expansion.scene.triangles.push(Triangle {
                        vertices: vertices.map(|v| transform.apply(v) * 25.4),
                        color: None,
                    });
                    if let Some(color) = material {
                        expansion.scene.appearances.insert(
                            expansion.scene.triangles.len() - 1,
                            TriangleAppearance {
                                base_color: color,
                                alpha_mode: if color[3] < 1. {
                                    AlphaMode::Blend
                                } else {
                                    AlphaMode::Opaque
                                },
                                ..TriangleAppearance::default()
                            },
                        );
                    }
                    Self::output(&expansion.scene)?;
                }
            }
        }
        for &entity in &container.entities {
            self.check(depth)?;
            match self.object_at(entity)? {
                Object::Edge(vertices, drawing)
                    if !surface_edges.contains(&entity)
                        && self.visible(*drawing, container.active_layer)? =>
                {
                    expansion.scene.lines.push(Line {
                        vertices: [self.vertex(vertices[0])?, self.vertex(vertices[1])?]
                            .map(|v| transform.apply(v) * 25.4),
                        color: None,
                    });
                    Self::output(&expansion.scene)?;
                }
                Object::Instance(definition, local, drawing)
                    if self.visible(*drawing, container.active_layer)? =>
                {
                    if !expansion.stack.insert(*definition) {
                        return Err("cyclic legacy SketchUp component placement".into());
                    }
                    let Object::Definition(child, billboard) = self.object_at(*definition)? else {
                        return Err("mistyped legacy SketchUp component definition".into());
                    };
                    if *billboard {
                        return Err("legacy SketchUp camera-facing component is unsupported".into());
                    }
                    self.emit(
                        child,
                        transform.then(*local),
                        drawing.material.or(inherited),
                        expansion,
                        depth + 1,
                    )?;
                    expansion.stack.remove(definition);
                }
                Object::Face(_, _, _) | Object::Edge(_, _) | Object::Instance(_, _, _) => {}
                _ => return Err("unsupported legacy SketchUp root/placed entity".into()),
            }
        }
        Ok(())
    }

    /// Bound final geometry expansion independently of archive pool size.
    fn output(scene: &Scene) -> Result<()> {
        if scene.triangles.len() + scene.lines.len() > MAX_ITEMS {
            return Err("legacy SketchUp placed geometry exceeds primitive limit".into());
        }
        Ok(())
    }
}
