//! Static IFC Part 21 geometry and product placements, independent of native BIM kernels.
#[path = "ifc_csg.rs"]
mod csg;
use super::transform::Transform;
use crate::scene::{Line, Point, Scene, Triangle, Vec3};
use crate::{Error, Result};
use ruststep::ast::{EntityInstance, Name, Parameter};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::f64::consts::{PI, TAU};
use std::path::Path;
use truck_stepio::r#in::ruststep;

const MAX_ENTITIES: usize = 500_000;
const MAX_GEOMETRY: usize = 5_000_000;
const MAX_DEPTH: usize = 64;
const CURVE_SEGMENTS: usize = 96;
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
fn list(p: &Parameter) -> Result<&[Parameter]> {
    if let Parameter::List(v) = p {
        Ok(v)
    } else {
        Err(error("IFC parameter requires a list"))
    }
}
fn reference(p: &Parameter) -> Result<u64> {
    if let Parameter::Ref(Name::Entity(id)) = p {
        Ok(*id)
    } else {
        Err(error("IFC parameter requires an entity reference"))
    }
}
fn optional_reference(p: &Parameter) -> Result<Option<u64>> {
    if matches!(p, Parameter::NotProvided | Parameter::Omitted) {
        Ok(None)
    } else {
        reference(p).map(Some)
    }
}
fn number(p: &Parameter) -> Result<f64> {
    let n = match p {
        Parameter::Real(n) => *n,
        Parameter::Integer(n) => *n as f64,
        Parameter::Typed { parameter, .. } => return number(parameter),
        _ => return Err(error("IFC parameter requires a finite number")),
    };
    if n.is_finite() {
        Ok(n)
    } else {
        Err(error("non-finite IFC number"))
    }
}
fn positive(p: &Parameter) -> Result<f64> {
    let n = number(p)?;
    if n > 0. {
        Ok(n)
    } else {
        Err(error("IFC dimension must be positive"))
    }
}
fn integer(p: &Parameter) -> Result<usize> {
    if let Parameter::Integer(n) = p {
        usize::try_from(*n).map_err(Into::into)
    } else {
        Err(error("IFC index requires an integer"))
    }
}
fn text(p: &Parameter) -> Option<&str> {
    match p {
        Parameter::String(s) | Parameter::Enumeration(s) => Some(s),
        _ => None,
    }
}
fn boolean(p: &Parameter) -> Result<bool> {
    match text(p) {
        Some("T" | "TRUE") => Ok(true),
        Some("F" | "FALSE") => Ok(false),
        _ => Err(error("IFC parameter requires a boolean")),
    }
}
fn field(e: &Entity, i: usize) -> Result<&Parameter> {
    e.fields
        .get(i)
        .ok_or_else(|| error(format!("truncated IFC {} attribute list", e.kind)))
}
fn refs(p: &Parameter) -> Result<Vec<u64>> {
    list(p)?.iter().map(reference).collect()
}
fn vector(p: &Parameter) -> Result<Vec3> {
    let values = list(p)?;
    if !(2..=3).contains(&values.len()) {
        return Err(error("IFC coordinate needs 2 or 3 components"));
    }
    Ok(Vec3::new(
        number(&values[0])?,
        number(&values[1])?,
        values.get(2).map(number).transpose()?.unwrap_or(0.),
    ))
}
fn normalized(v: Vec3) -> Result<Vec3> {
    let length = v.length();
    if !length.is_finite() || length <= 1e-15 {
        return Err(error("invalid IFC direction"));
    }
    Ok(v / length)
}
fn basis(origin: Vec3, z: Vec3, x: Vec3) -> Result<Transform> {
    let z = normalized(z)?;
    let x = normalized(x - z * x.dot(z))?;
    let y = z.cross(x);
    Ok(Transform([
        [x.x, x.y, x.z, 0.],
        [y.x, y.y, y.z, 0.],
        [z.x, z.y, z.z, 0.],
        [origin.x, origin.y, origin.z, 1.],
    ]))
}
fn column(t: Transform, i: usize) -> Vec3 {
    Vec3::new(t.0[i][0], t.0[i][1], t.0[i][2])
}
fn determinant(t: Transform) -> f64 {
    column(t, 0).dot(column(t, 1).cross(column(t, 2)))
}
fn checked_ring(mut ring: Vec<Vec3>) -> Result<Vec<Vec3>> {
    if ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() < 3 || ring.len() > MAX_GEOMETRY || ring.iter().any(|p| !p.is_finite()) {
        return Err(error("IFC polygon requires at least three finite vertices"));
    }
    Ok(ring)
}
fn signed_area(ring: &[Vec3]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.
}
fn triangles(rings: &[Vec<Vec3>]) -> Result<Vec<[Vec3; 3]>> {
    super::citygml::triangulate_rings(rings)
        .map_err(|e| error(format!("IFC face triangulation: {e}")))
}
struct Entity {
    kind: String,
    fields: Vec<Parameter>,
}
struct Model {
    entities: HashMap<u64, Entity>,
    colors: HashMap<u64, [u8; 3]>,
    face_colors: HashMap<u64, Vec<[u8; 3]>>,
    length_scale: f64,
    angle_scale: f64,
    visits: Cell<usize>,
}
impl Model {
    fn visit(&self) -> Result<()> {
        let visits = self.visits.get() + 1;
        if visits > MAX_GEOMETRY {
            return Err(error("IFC reference traversal exceeds its work limit"));
        }
        self.visits.set(visits);
        Ok(())
    }
    fn entity(&self, id: u64) -> Result<&Entity> {
        self.entities
            .get(&id)
            .ok_or_else(|| error(format!("missing IFC entity #{id}")))
    }
    fn expected(&self, id: u64, kind: &str) -> Result<&Entity> {
        let e = self.entity(id)?;
        if e.kind == kind {
            Ok(e)
        } else {
            Err(error(format!(
                "IFC #{id} requires {kind}, found {}",
                e.kind
            )))
        }
    }
    fn point(&self, id: u64) -> Result<Vec3> {
        vector(field(self.expected(id, "IFCCARTESIANPOINT")?, 0)?)
    }
    fn direction(&self, id: u64) -> Result<Vec3> {
        normalized(vector(field(self.expected(id, "IFCDIRECTION")?, 0)?)?)
    }
    fn optional_direction(&self, p: &Parameter, default: Vec3) -> Result<Vec3> {
        optional_reference(p)?
            .map(|id| self.direction(id))
            .transpose()
            .map(|v| v.unwrap_or(default))
    }
    fn axis(&self, id: Option<u64>) -> Result<Transform> {
        let Some(id) = id else {
            return Ok(Transform::IDENTITY);
        };
        let e = self.entity(id)?;
        let origin = self.point(reference(field(e, 0)?)?)?;
        match e.kind.as_str() {
            "IFCAXIS2PLACEMENT3D" => {
                let z = self.optional_direction(field(e, 1)?, Vec3::new(0., 0., 1.))?;
                let default = if z.x.abs() > 0.999999 {
                    Vec3::new(0., 1., 0.)
                } else {
                    Vec3::new(1., 0., 0.)
                };
                basis(origin, z, self.optional_direction(field(e, 2)?, default)?)
            }
            "IFCAXIS2PLACEMENT2D" => {
                let x = self.optional_direction(field(e, 1)?, Vec3::new(1., 0., 0.))?;
                if x.z.abs() > 1e-12 {
                    return Err(error("IFC 2D placement direction is not 2D"));
                }
                basis(origin, Vec3::new(0., 0., 1.), x)
            }
            "IFCAXIS1PLACEMENT" => {
                let z = self.optional_direction(field(e, 1)?, Vec3::new(0., 0., 1.))?;
                basis(
                    origin,
                    z,
                    if z.x.abs() > 0.999999 {
                        Vec3::new(0., 1., 0.)
                    } else {
                        Vec3::new(1., 0., 0.)
                    },
                )
            }
            _ => Err(error(format!("unsupported IFC axis placement {}", e.kind))),
        }
    }
    fn placement(&self, id: Option<u64>, depth: usize) -> Result<Transform> {
        self.visit()?;
        if depth > MAX_DEPTH {
            return Err(error("IFC placement chain is cyclic or too deep"));
        }
        let Some(id) = id else {
            return Ok(Transform::IDENTITY);
        };
        let e = self.expected(id, "IFCLOCALPLACEMENT")?;
        Ok(self
            .placement(optional_reference(field(e, 0)?)?, depth + 1)?
            .then(self.axis(Some(reference(field(e, 1)?)?))?))
    }
    fn operator(&self, id: u64) -> Result<Transform> {
        let e = self.entity(id)?;
        if !matches!(
            e.kind.as_str(),
            "IFCCARTESIANTRANSFORMATIONOPERATOR3D"
                | "IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM"
                | "IFCCARTESIANTRANSFORMATIONOPERATOR2D"
                | "IFCCARTESIANTRANSFORMATIONOPERATOR2DNONUNIFORM"
        ) {
            return Err(error(format!(
                "unsupported IFC mapping operator {}",
                e.kind
            )));
        }
        let is_3d = e.kind.contains("3D");
        let z = if is_3d {
            self.optional_direction(field(e, 4)?, Vec3::new(0., 0., 1.))?
        } else {
            Vec3::new(0., 0., 1.)
        };
        let default = if z.x.abs() > 0.999999 {
            Vec3::new(0., 1., 0.)
        } else {
            Vec3::new(1., 0., 0.)
        };
        let x = self.optional_direction(field(e, 0)?, default)?;
        let mut matrix = basis(self.point(reference(field(e, 2)?)?)?, z, x)?;
        if let Some(y) = optional_reference(field(e, 1)?)?
            && self.direction(y)?.dot(column(matrix, 1)) < 0.
        {
            for n in &mut matrix.0[1][..3] {
                *n = -*n;
            }
        }
        let default_scale = if matches!(field(e, 3)?, Parameter::NotProvided) {
            1.
        } else {
            positive(field(e, 3)?)?
        };
        let mut scales = [default_scale; 3];
        if e.kind.ends_with("NONUNIFORM") {
            for (axis, i) in [(1, if is_3d { 5 } else { 4 }), (2, 6)] {
                if axis == 2 && !is_3d {
                    continue;
                }
                if !matches!(field(e, i)?, Parameter::NotProvided) {
                    scales[axis] = positive(field(e, i)?)?;
                }
            }
        }
        if !is_3d {
            scales[2] = 1.;
        }
        for (axis, scale) in scales.into_iter().enumerate() {
            for n in &mut matrix.0[axis][..3] {
                *n *= scale;
            }
        }
        Ok(matrix)
    }
    fn coordinates(&self, id: u64) -> Result<Vec<Vec3>> {
        let e = self.entity(id)?;
        if !matches!(
            e.kind.as_str(),
            "IFCCARTESIANPOINTLIST3D" | "IFCCARTESIANPOINTLIST2D"
        ) {
            return Err(error("IFC tessellation requires a Cartesian point list"));
        }
        let values = list(field(e, 0)?)?;
        if values.len() > MAX_GEOMETRY {
            return Err(error("IFC coordinate count exceeds limit"));
        }
        values.iter().map(vector).collect()
    }
    fn curve(&self, id: u64, depth: usize) -> Result<Vec<Vec3>> {
        self.visit()?;
        if depth > MAX_DEPTH {
            return Err(error("IFC curve references are cyclic or too deep"));
        }
        let e = self.entity(id)?;
        match e.kind.as_str() {
            "IFCPOLYLINE" | "IFCPOLYLOOP" => refs(field(e, 0)?)?
                .into_iter()
                .map(|id| self.point(id))
                .collect(),
            "IFCINDEXEDPOLYCURVE" => {
                let points = self.coordinates(reference(field(e, 0)?)?)?;
                if matches!(field(e, 1)?, Parameter::NotProvided) {
                    return Ok(points);
                }
                let mut result = Vec::new();
                for segment in list(field(e, 1)?)? {
                    let Parameter::Typed { keyword, parameter } = segment else {
                        return Err(error("invalid IFC indexed curve segment"));
                    };
                    let indices = list(parameter)?;
                    let get = |p: &Parameter| -> Result<Vec3> {
                        let index = integer(p)?
                            .checked_sub(1)
                            .ok_or_else(|| error("IFC indices are one-based"))?;
                        points
                            .get(index)
                            .copied()
                            .ok_or_else(|| error("IFC curve index out of bounds"))
                    };
                    let path = match keyword.as_str() {
                        "IFCLINEINDEX" if indices.len() >= 2 => {
                            indices.iter().map(get).collect::<Result<Vec<_>>>()?
                        }
                        "IFCARCINDEX" if indices.len() == 3 => {
                            arc(get(&indices[0])?, get(&indices[1])?, get(&indices[2])?)?
                        }
                        _ => return Err(error("unsupported IFC indexed curve segment")),
                    };
                    append_path(&mut result, path)?;
                }
                Ok(result)
            }
            "IFCCOMPOSITECURVE" => {
                let mut result = Vec::new();
                for segment in refs(field(e, 0)?)? {
                    let segment = self.expected(segment, "IFCCOMPOSITECURVESEGMENT")?;
                    let mut path = self.curve(reference(field(segment, 2)?)?, depth + 1)?;
                    if !boolean(field(segment, 1)?)? {
                        path.reverse();
                    }
                    append_path(&mut result, path)?;
                }
                Ok(result)
            }
            "IFCCIRCLE" | "IFCELLIPSE" => {
                let t = self.axis(Some(reference(field(e, 0)?)?))?;
                let a = positive(field(e, 1)?)?;
                let b = if e.kind == "IFCELLIPSE" {
                    positive(field(e, 2)?)?
                } else {
                    a
                };
                Ok((0..=CURVE_SEGMENTS)
                    .map(|i| {
                        let angle = i as f64 / CURVE_SEGMENTS as f64 * TAU;
                        t.apply(Vec3::new(a * angle.cos(), b * angle.sin(), 0.))
                    })
                    .collect())
            }
            _ => Err(error(format!("unsupported IFC curve {}", e.kind))),
        }
    }
    fn profile(&self, id: u64, depth: usize) -> Result<Vec<Vec<Vec3>>> {
        self.visit()?;
        if depth > MAX_DEPTH {
            return Err(error("IFC profile references are cyclic or too deep"));
        }
        let e = self.entity(id)?;
        if text(field(e, 0)?) != Some("AREA") {
            return Err(error("IFC solid profile must define an area"));
        }
        let mut rings = match e.kind.as_str() {
            "IFCARBITRARYCLOSEDPROFILEDEF" => {
                vec![checked_ring(self.curve(reference(field(e, 2)?)?, 0)?)?]
            }
            "IFCARBITRARYPROFILEDEFWITHVOIDS" => {
                let mut rings = vec![checked_ring(self.curve(reference(field(e, 2)?)?, 0)?)?];
                for id in refs(field(e, 3)?)? {
                    rings.push(checked_ring(self.curve(id, 0)?)?);
                }
                rings
            }
            "IFCDERIVEDPROFILEDEF" | "IFCMIRROREDPROFILEDEF" => {
                let rings = self.profile(reference(field(e, 2)?)?, depth + 1)?;
                let t = self.operator(reference(field(e, 3)?)?)?;
                rings
                    .into_iter()
                    .map(|ring| ring.into_iter().map(|p| t.apply(p)).collect())
                    .collect()
            }
            "IFCRECTANGLEPROFILEDEF"
            | "IFCROUNDEDRECTANGLEPROFILEDEF"
            | "IFCRECTANGLEHOLLOWPROFILEDEF" => {
                let x = positive(field(e, 3)?)? / 2.;
                let y = positive(field(e, 4)?)? / 2.;
                let mut rings = vec![rectangle(x, y)];
                if e.kind == "IFCROUNDEDRECTANGLEPROFILEDEF" {
                    let radius = positive(field(e, 5)?)?;
                    rings[0] = fillets(
                        rectangle(x, y),
                        &[(0, radius), (1, radius), (2, radius), (3, radius)],
                    )?;
                }
                if e.kind == "IFCRECTANGLEHOLLOWPROFILEDEF" {
                    let thickness = positive(field(e, 5)?)?;
                    if thickness >= x.min(y) {
                        return Err(error("IFC hollow rectangle wall is too thick"));
                    }
                    let inner = e
                        .fields
                        .get(6)
                        .filter(|p| !matches!(p, Parameter::NotProvided))
                        .map(number)
                        .transpose()?
                        .unwrap_or(0.);
                    let outer = e
                        .fields
                        .get(7)
                        .filter(|p| !matches!(p, Parameter::NotProvided))
                        .map(number)
                        .transpose()?
                        .unwrap_or(0.);
                    rings[0] = fillets(
                        rectangle(x, y),
                        &[(0, outer), (1, outer), (2, outer), (3, outer)],
                    )?;
                    rings.push(fillets(
                        rectangle(x - thickness, y - thickness),
                        &[(0, inner), (1, inner), (2, inner), (3, inner)],
                    )?);
                }
                rings
            }
            "IFCCIRCLEPROFILEDEF" | "IFCCIRCLEHOLLOWPROFILEDEF" | "IFCELLIPSEPROFILEDEF" => {
                let a = positive(field(e, 3)?)?;
                let b = if e.kind == "IFCELLIPSEPROFILEDEF" {
                    positive(field(e, 4)?)?
                } else {
                    a
                };
                let ellipse = |a: f64, b: f64| {
                    (0..CURVE_SEGMENTS)
                        .map(|i| {
                            let angle = i as f64 / CURVE_SEGMENTS as f64 * TAU;
                            Vec3::new(a * angle.cos(), b * angle.sin(), 0.)
                        })
                        .collect()
                };
                let mut rings = vec![ellipse(a, b)];
                if e.kind == "IFCCIRCLEHOLLOWPROFILEDEF" {
                    let thickness = positive(field(e, 4)?)?;
                    if thickness >= a {
                        return Err(error("IFC hollow circle wall is too thick"));
                    }
                    rings.push(ellipse(a - thickness, b - thickness));
                }
                rings
            }
            "IFCISHAPEPROFILEDEF" => {
                let w = positive(field(e, 3)?)? / 2.;
                let h = positive(field(e, 4)?)? / 2.;
                let web = positive(field(e, 5)?)? / 2.;
                let flange = positive(field(e, 6)?)?;
                if web >= w || flange >= h {
                    return Err(error("invalid IFC I-section dimensions"));
                }
                for p in e.fields.iter().skip(8) {
                    if !matches!(p, Parameter::NotProvided) && number(p)? != 0. {
                        return Err(error("IFC I-section edge rounding/slopes are unsupported"));
                    }
                }
                let ring = vec![
                    (-w, -h),
                    (w, -h),
                    (w, -h + flange),
                    (web, -h + flange),
                    (web, h - flange),
                    (w, h - flange),
                    (w, h),
                    (-w, h),
                    (-w, h - flange),
                    (-web, h - flange),
                    (-web, -h + flange),
                    (-w, -h + flange),
                ]
                .into_iter()
                .map(|(x, y)| Vec3::new(x, y, 0.))
                .collect::<Vec<_>>();
                let radius = e
                    .fields
                    .get(7)
                    .filter(|p| !matches!(p, Parameter::NotProvided))
                    .map(number)
                    .transpose()?
                    .unwrap_or(0.);
                vec![fillets(
                    ring,
                    &[(3, radius), (4, radius), (9, radius), (10, radius)],
                )?]
            }
            _ => {
                return Err(error(format!(
                    "unsupported IFC extrusion profile {}",
                    e.kind
                )));
            }
        };
        if matches!(
            e.kind.as_str(),
            "IFCRECTANGLEPROFILEDEF"
                | "IFCROUNDEDRECTANGLEPROFILEDEF"
                | "IFCRECTANGLEHOLLOWPROFILEDEF"
                | "IFCCIRCLEPROFILEDEF"
                | "IFCCIRCLEHOLLOWPROFILEDEF"
                | "IFCELLIPSEPROFILEDEF"
                | "IFCISHAPEPROFILEDEF"
        ) {
            let t = self.axis(optional_reference(field(e, 2)?)?)?;
            for ring in &mut rings {
                for p in ring {
                    *p = t.apply(*p);
                }
            }
        }
        for (i, ring) in rings.iter_mut().enumerate() {
            if ring.iter().any(|p| p.z.abs() > 1e-8) {
                return Err(error("IFC extrusion profile is not in its XY plane"));
            }
            let area = signed_area(ring);
            if !area.is_finite() || area == 0. {
                return Err(error("IFC profile is degenerate"));
            }
            if (area > 0.) != (i == 0) {
                ring.reverse();
            }
        }
        Ok(rings)
    }
    fn color(&self, id: u64) -> Result<[u8; 3]> {
        let e = self.expected(id, "IFCCOLOURRGB")?;
        let mut color = [0; 3];
        for (i, c) in color.iter_mut().enumerate() {
            let n = number(field(e, i + 1)?)?;
            if !(0. ..=1.).contains(&n) {
                return Err(error("IFC RGB channel out of range"));
            }
            *c = (n * 255.).round() as u8;
        }
        Ok(color)
    }
    fn style(&self, id: u64, depth: usize) -> Result<Option<[u8; 3]>> {
        self.visit()?;
        if depth > MAX_DEPTH {
            return Err(error("IFC style references are cyclic or too deep"));
        }
        let e = self.entity(id)?;
        match e.kind.as_str() {
            "IFCPRESENTATIONSTYLEASSIGNMENT" | "IFCSURFACESTYLE" => {
                for child in refs(field(e, if e.kind == "IFCSURFACESTYLE" { 2 } else { 0 })?)? {
                    if let Some(color) = self.style(child, depth + 1)? {
                        return Ok(Some(color));
                    }
                }
                Ok(None)
            }
            "IFCSURFACESTYLESHADING" | "IFCSURFACESTYLERENDERING" => {
                let mut color = self.color(reference(field(e, 0)?)?)?;
                if e.kind == "IFCSURFACESTYLERENDERING" {
                    match field(e, 2)? {
                        Parameter::Ref(_) => color = self.color(reference(field(e, 2)?)?)?,
                        Parameter::NotProvided => {}
                        n => {
                            let ratio = number(n)?;
                            if !(0. ..=1.).contains(&ratio) {
                                return Err(error("IFC diffuse color factor out of range"));
                            }
                            color = color.map(|c| (c as f64 * ratio).round() as u8);
                        }
                    }
                }
                Ok(Some(color))
            }
            "IFCCURVESTYLE" => optional_reference(field(e, 3)?)?
                .map(|id| self.color(id))
                .transpose(),
            // Texture/specular/light/refraction styles do not change geometry.
            _ => Ok(None),
        }
    }
    fn collect_styles(&mut self) -> Result<()> {
        for (&id, e) in &self.entities {
            if e.kind == "IFCSTYLEDITEM" {
                let Some(item) = optional_reference(field(e, 0)?)? else {
                    continue;
                };
                for style in refs(field(e, 1)?)? {
                    if let Some(color) = self.style(style, 0)? {
                        if let Some(previous) = self.colors.insert(item, color)
                            && previous != color
                        {
                            return Err(error(format!("conflicting IFC styles on #{item}")));
                        }
                        break;
                    }
                }
            } else if e.kind == "IFCINDEXEDCOLOURMAP" {
                let mapped = reference(field(e, 0)?)?;
                let colors = self.expected(reference(field(e, 2)?)?, "IFCCOLOURRGBLIST")?;
                let colors = list(field(colors, 0)?)?
                    .iter()
                    .map(|value| {
                        let v = list(value)?;
                        if v.len() != 3 {
                            return Err(error("IFC color list requires RGB triples"));
                        }
                        let mut c = [0; 3];
                        for i in 0..3 {
                            let n = number(&v[i])?;
                            if !(0. ..=1.).contains(&n) {
                                return Err(error("IFC RGB channel out of range"));
                            }
                            c[i] = (n * 255.).round() as u8;
                        }
                        Ok(c)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let indices = list(field(e, 3)?)?;
                let values = indices
                    .iter()
                    .map(|p| {
                        let index = integer(p)?
                            .checked_sub(1)
                            .ok_or_else(|| error("IFC color indices are one-based"))?;
                        colors
                            .get(index)
                            .copied()
                            .ok_or_else(|| error("IFC color index out of range"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if self.face_colors.insert(mapped, values).is_some() {
                    return Err(error(format!("duplicate IFC color map #{id}")));
                }
            }
        }
        Ok(())
    }
    fn emit(
        &self,
        scene: &mut Scene,
        vertices: [Vec3; 3],
        t: Transform,
        color: Option<[u8; 3]>,
    ) -> Result<()> {
        if scene.triangles.len() >= MAX_GEOMETRY {
            return Err(error("IFC triangle count exceeds limit"));
        }
        let mut vertices = vertices.map(|p| t.apply(p));
        if vertices.iter().any(|p| !p.is_finite()) {
            return Err(error("IFC transform overflows its coordinates"));
        }
        if determinant(t) < 0. {
            vertices.swap(1, 2);
        }
        if (vertices[1] - vertices[0])
            .cross(vertices[2] - vertices[0])
            .length()
            > 1e-15
        {
            scene.triangles.push(Triangle { vertices, color });
        }
        Ok(())
    }
    fn product(&self, id: u64) -> Result<Scene> {
        let p = self.entity(id)?;
        let Some(representation) = optional_reference(field(p, 6)?)? else {
            return Ok(Scene::default());
        };
        let placement = self.placement(optional_reference(field(p, 5)?)?, 0)?;
        let shape = self.entity(representation)?;
        let reps = refs(field(shape, 2)?)?;
        let has_body = reps.iter().any(|id| {
            self.entities
                .get(id)
                .and_then(|e| e.fields.get(1))
                .and_then(text)
                == Some("Body")
        });
        let mut scene = Scene::default();
        for id in reps {
            let r = self.entity(id)?;
            if r.kind != "IFCSHAPEREPRESENTATION" {
                return Err(error("unsupported IFC product representation"));
            }
            let identifier = text(field(r, 1)?);
            if has_body && identifier != Some("Body") {
                continue;
            }
            if matches!(
                identifier,
                Some("Axis" | "FootPrint" | "Box" | "Clearance" | "SurveyPoints")
            ) {
                continue;
            }
            self.geometry(id, placement, None, 0, &mut scene)?;
        }
        Ok(scene)
    }
    fn halfspace(&self, id: u64, operand: &Scene, color: Option<[u8; 3]>) -> Result<Scene> {
        let e = self.expected(id, "IFCHALFSPACESOLID")?;
        let plane = self.expected(reference(field(e, 0)?)?, "IFCPLANE")?;
        let placement = self.axis(Some(reference(field(plane, 0)?)?))?;
        let origin = column(placement, 3);
        let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in operand.triangles.iter().flat_map(|t| t.vertices) {
            let p = p - origin;
            let p = Vec3::new(
                p.dot(column(placement, 0)),
                p.dot(column(placement, 1)),
                p.dot(column(placement, 2)),
            );
            minimum = Vec3::new(minimum.x.min(p.x), minimum.y.min(p.y), minimum.z.min(p.z));
            maximum = Vec3::new(maximum.x.max(p.x), maximum.y.max(p.y), maximum.z.max(p.z));
        }
        let negative = boolean(field(e, 1)?)?;
        if operand.triangles.is_empty()
            || (negative && minimum.z >= 0.)
            || (!negative && maximum.z <= 0.)
        {
            return Ok(Scene::default());
        }
        let extent = (maximum - minimum).length().max(1e-8);
        if !extent.is_finite() {
            return Err(error("IFC half-space bounds overflow"));
        }
        minimum = minimum - Vec3::new(extent, extent, extent);
        maximum = maximum + Vec3::new(extent, extent, extent);
        if negative {
            maximum.z = 0.;
        } else {
            minimum.z = 0.;
        }
        let p = [
            Vec3::new(minimum.x, minimum.y, minimum.z),
            Vec3::new(maximum.x, minimum.y, minimum.z),
            Vec3::new(maximum.x, maximum.y, minimum.z),
            Vec3::new(minimum.x, maximum.y, minimum.z),
            Vec3::new(minimum.x, minimum.y, maximum.z),
            Vec3::new(maximum.x, minimum.y, maximum.z),
            Vec3::new(maximum.x, maximum.y, maximum.z),
            Vec3::new(minimum.x, maximum.y, maximum.z),
        ];
        let mut scene = Scene::default();
        for [a, b, c, d] in [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ] {
            self.emit(&mut scene, [p[a], p[b], p[c]], placement, color)?;
            self.emit(&mut scene, [p[a], p[c], p[d]], placement, color)?;
        }
        Ok(scene)
    }
    fn face(&self, id: u64, t: Transform, color: Option<[u8; 3]>, scene: &mut Scene) -> Result<()> {
        let e = self.entity(id)?;
        if !matches!(
            e.kind.as_str(),
            "IFCFACE" | "IFCFACESURFACE" | "IFCADVANCEDFACE"
        ) {
            return Err(error(format!("unsupported IFC shell face {}", e.kind)));
        }
        if e.kind != "IFCFACE" {
            self.expected(reference(field(e, 1)?)?, "IFCPLANE")?;
        }
        let mut rings = Vec::new();
        let mut outer = None;
        for bound in refs(field(e, 0)?)? {
            let b = self.entity(bound)?;
            if !matches!(b.kind.as_str(), "IFCFACEBOUND" | "IFCFACEOUTERBOUND") {
                return Err(error("unsupported IFC face bound"));
            }
            let loop_id = reference(field(b, 0)?)?;
            self.expected(loop_id, "IFCPOLYLOOP")?;
            let mut ring = checked_ring(self.curve(loop_id, 0)?)?;
            if !boolean(field(b, 1)?)? {
                ring.reverse();
            }
            if b.kind == "IFCFACEOUTERBOUND" && outer.replace(rings.len()).is_some() {
                return Err(error("IFC face has multiple outer bounds"));
            }
            rings.push(ring);
        }
        if let Some(index) = outer {
            rings.swap(0, index);
        }
        for triangle in triangles(&rings)? {
            self.emit(scene, triangle, t, color)?;
        }
        Ok(())
    }
    fn geometry(
        &self,
        id: u64,
        t: Transform,
        inherited: Option<[u8; 3]>,
        depth: usize,
        scene: &mut Scene,
    ) -> Result<()> {
        self.visit()?;
        if depth > MAX_DEPTH {
            return Err(error("IFC geometry references are cyclic or too deep"));
        }
        let e = self.entity(id)?;
        let color = self.colors.get(&id).copied().or(inherited);
        match e.kind.as_str() {
            "IFCSHAPEREPRESENTATION" | "IFCTOPOLOGYREPRESENTATION" => {
                for child in refs(field(e, 3)?)? {
                    self.geometry(child, t, color, depth + 1, scene)?;
                }
            }
            "IFCMAPPEDITEM" => {
                let map = self.expected(reference(field(e, 0)?)?, "IFCREPRESENTATIONMAP")?;
                // MappingOrigin defines the source coordinate frame; target transforms it.
                let map_transform = self
                    .operator(reference(field(e, 1)?)?)?
                    .then(self.axis(Some(reference(field(map, 0)?)?))?);
                self.geometry(
                    reference(field(map, 1)?)?,
                    t.then(map_transform),
                    color,
                    depth + 1,
                    scene,
                )?;
            }
            "IFCEXTRUDEDAREASOLID" => {
                let rings = self.profile(reference(field(e, 0)?)?, 0)?;
                let placement = t.then(self.axis(optional_reference(field(e, 1)?)?)?);
                let direction = self.direction(reference(field(e, 2)?)?)?;
                let delta = direction * positive(field(e, 3)?)?;
                if direction.z.abs() < 1e-12 {
                    return Err(error("IFC extrusion direction lies in the profile plane"));
                }
                let reverse = direction.z < 0.;
                for mut triangle in triangles(&rings)? {
                    if reverse {
                        triangle.swap(1, 2);
                    }
                    self.emit(scene, triangle.map(|p| p + delta), placement, color)?;
                    triangle.swap(1, 2);
                    self.emit(scene, triangle, placement, color)?;
                }
                for ring in &rings {
                    for i in 0..ring.len() {
                        let a = ring[i];
                        let b = ring[(i + 1) % ring.len()];
                        for mut tri in [[a, b, b + delta], [a, b + delta, a + delta]] {
                            if reverse {
                                tri.swap(1, 2);
                            }
                            self.emit(scene, tri, placement, color)?;
                        }
                    }
                }
            }
            "IFCREVOLVEDAREASOLID" => {
                let rings = self.profile(reference(field(e, 0)?)?, 0)?;
                let placement = t.then(self.axis(optional_reference(field(e, 1)?)?)?);
                let axis = self.axis(Some(reference(field(e, 2)?)?))?;
                let origin = column(axis, 3);
                let direction = column(axis, 2);
                if origin.z.abs() > 1e-8 || direction.z.abs() > 1e-8 {
                    return Err(error("IFC revolution axis must lie in the profile plane"));
                }
                let angle = positive(field(e, 3)?)? * self.angle_scale;
                if !angle.is_finite() || angle > TAU + 1e-6 {
                    return Err(error("IFC revolution angle exceeds a full turn"));
                }
                let angle = angle.min(TAU);
                let count = (angle / TAU * CURVE_SEGMENTS as f64).ceil().max(2.) as usize;
                let center = rings[0].iter().copied().fold(Vec3::default(), |a, b| a + b)
                    / rings[0].len() as f64;
                let forward = direction.cross(center - origin).z;
                if forward.abs() < 1e-12 {
                    return Err(error("IFC revolution axis crosses the profile center"));
                }
                let reverse = forward < 0.;
                let rotated = |p: Vec3, theta: f64| {
                    let v = p - origin;
                    origin
                        + v * theta.cos()
                        + direction.cross(v) * theta.sin()
                        + direction * (direction.dot(v) * (1. - theta.cos()))
                };
                if angle < TAU - 1e-6 {
                    for mut tri in triangles(&rings)? {
                        if reverse {
                            tri.swap(1, 2);
                        }
                        self.emit(scene, tri.map(|p| rotated(p, angle)), placement, color)?;
                        tri.swap(1, 2);
                        self.emit(scene, tri, placement, color)?;
                    }
                }
                for j in 0..count {
                    let start = angle * j as f64 / count as f64;
                    let end = angle * (j + 1) as f64 / count as f64;
                    for ring in &rings {
                        for i in 0..ring.len() {
                            let a = ring[i];
                            let b = ring[(i + 1) % ring.len()];
                            let a0 = rotated(a, start);
                            let b0 = rotated(b, start);
                            let a1 = rotated(a, end);
                            let b1 = rotated(b, end);
                            for mut tri in [[a0, b0, b1], [a0, b1, a1]] {
                                if reverse {
                                    tri.swap(1, 2);
                                }
                                self.emit(scene, tri, placement, color)?;
                            }
                        }
                    }
                }
            }
            "IFCBOOLEANRESULT" | "IFCBOOLEANCLIPPINGRESULT" => {
                let mut a = Scene::default();
                let mut b = Scene::default();
                self.geometry(
                    reference(field(e, 1)?)?,
                    Transform::IDENTITY,
                    color,
                    depth + 1,
                    &mut a,
                )?;
                let second = reference(field(e, 2)?)?;
                let operator =
                    text(field(e, 0)?).ok_or_else(|| error("invalid IFC Boolean operator"))?;
                if self.entity(second)?.kind == "IFCHALFSPACESOLID" {
                    if operator == "UNION" {
                        return Err(error("IFC union with an infinite half-space is unbounded"));
                    }
                    b = self.halfspace(second, &a, color)?;
                } else {
                    self.geometry(second, Transform::IDENTITY, color, depth + 1, &mut b)?;
                }
                if !a.lines.is_empty()
                    || !a.points.is_empty()
                    || !b.lines.is_empty()
                    || !b.points.is_empty()
                {
                    return Err(error("IFC Boolean operands must be solids"));
                }
                let triangles = csg::boolean(a.triangles, b.triangles, operator)?;
                if scene.triangles.len() + triangles.len() > MAX_GEOMETRY {
                    return Err(error("IFC triangle count exceeds limit"));
                }
                for triangle in triangles {
                    self.emit(scene, triangle.vertices, t, triangle.color)?;
                }
            }
            "IFCCSGSOLID" => self.geometry(reference(field(e, 0)?)?, t, color, depth + 1, scene)?,
            "IFCBLOCK" => {
                let placement = t.then(self.axis(Some(reference(field(e, 0)?)?))?);
                let x = positive(field(e, 1)?)?;
                let y = positive(field(e, 2)?)?;
                let z = positive(field(e, 3)?)?;
                let p = [
                    Vec3::new(0., 0., 0.),
                    Vec3::new(x, 0., 0.),
                    Vec3::new(x, y, 0.),
                    Vec3::new(0., y, 0.),
                    Vec3::new(0., 0., z),
                    Vec3::new(x, 0., z),
                    Vec3::new(x, y, z),
                    Vec3::new(0., y, z),
                ];
                for [a, b, c, d] in [
                    [0, 3, 2, 1],
                    [4, 5, 6, 7],
                    [0, 1, 5, 4],
                    [1, 2, 6, 5],
                    [2, 3, 7, 6],
                    [3, 0, 4, 7],
                ] {
                    self.emit(scene, [p[a], p[b], p[c]], placement, color)?;
                    self.emit(scene, [p[a], p[c], p[d]], placement, color)?;
                }
            }
            "IFCRIGHTCIRCULARCYLINDER" | "IFCRIGHTCIRCULARCONE" => {
                let placement = t.then(self.axis(Some(reference(field(e, 0)?)?))?);
                let height = positive(field(e, 1)?)?;
                let radius = positive(field(e, 2)?)?;
                let conical = e.kind == "IFCRIGHTCIRCULARCONE";
                for i in 0..CURVE_SEGMENTS {
                    let a = i as f64 / CURVE_SEGMENTS as f64 * TAU;
                    let b = (i + 1) as f64 / CURVE_SEGMENTS as f64 * TAU;
                    let a0 = Vec3::new(radius * a.cos(), radius * a.sin(), 0.);
                    let b0 = Vec3::new(radius * b.cos(), radius * b.sin(), 0.);
                    let a1 = if conical {
                        Vec3::new(0., 0., height)
                    } else {
                        a0 + Vec3::new(0., 0., height)
                    };
                    let b1 = if conical {
                        a1
                    } else {
                        b0 + Vec3::new(0., 0., height)
                    };
                    self.emit(scene, [a0, b0, b1], placement, color)?;
                    self.emit(scene, [a0, b1, a1], placement, color)?;
                    self.emit(scene, [Vec3::default(), b0, a0], placement, color)?;
                    if !conical {
                        self.emit(scene, [Vec3::new(0., 0., height), a1, b1], placement, color)?;
                    }
                }
            }
            "IFCSPHERE" => {
                let placement = t.then(self.axis(Some(reference(field(e, 0)?)?))?);
                let radius = positive(field(e, 1)?)?;
                let point = |i: usize, j: usize| {
                    let lon = i as f64 / CURVE_SEGMENTS as f64 * TAU;
                    let lat = j as f64 / (CURVE_SEGMENTS / 2) as f64 * PI - PI / 2.;
                    Vec3::new(
                        radius * lat.cos() * lon.cos(),
                        radius * lat.cos() * lon.sin(),
                        radius * lat.sin(),
                    )
                };
                for j in 0..CURVE_SEGMENTS / 2 {
                    for i in 0..CURVE_SEGMENTS {
                        let a = point(i, j);
                        let b = point(i + 1, j);
                        let c = point(i + 1, j + 1);
                        let d = point(i, j + 1);
                        self.emit(scene, [a, b, c], placement, color)?;
                        self.emit(scene, [a, c, d], placement, color)?;
                    }
                }
            }
            "IFCTRIANGULATEDFACESET" | "IFCPOLYGONALFACESET" => {
                let points = self.coordinates(reference(field(e, 0)?)?)?;
                let triangular = e.kind == "IFCTRIANGULATEDFACESET";
                let index_field = if triangular { 3 } else { 2 };
                let pn = e
                    .fields
                    .get(if triangular { 4 } else { 3 })
                    .map(|p| {
                        if matches!(p, Parameter::NotProvided) {
                            Ok(None)
                        } else {
                            list(p)?
                                .iter()
                                .map(integer)
                                .collect::<Result<Vec<_>>>()
                                .map(Some)
                        }
                    })
                    .transpose()?
                    .flatten();
                let get = |p: &Parameter| -> Result<Vec3> {
                    let index = integer(p)?
                        .checked_sub(1)
                        .ok_or_else(|| error("IFC coordinate indices are one-based"))?;
                    let index = if let Some(pn) = &pn {
                        pn.get(index)
                            .copied()
                            .ok_or_else(|| error("IFC PnIndex is out of range"))?
                            .checked_sub(1)
                            .ok_or_else(|| error("IFC PnIndex entries are one-based"))?
                    } else {
                        index
                    };
                    points
                        .get(index)
                        .copied()
                        .ok_or_else(|| error("IFC face coordinate index is out of range"))
                };
                let faces = list(field(e, index_field)?)?;
                if let Some(colors) = self.face_colors.get(&id)
                    && colors.len() != faces.len()
                {
                    return Err(error("IFC indexed colors differ from face count"));
                }
                for (i, face) in faces.iter().enumerate() {
                    let face_color = self.face_colors.get(&id).map(|v| v[i]).or(color);
                    if triangular {
                        let face = list(face)?;
                        if face.len() != 3 {
                            return Err(error("IFC triangulated face requires three indices"));
                        }
                        self.emit(
                            scene,
                            [get(&face[0])?, get(&face[1])?, get(&face[2])?],
                            t,
                            face_color,
                        )?;
                    } else {
                        let f = self.entity(reference(face)?)?;
                        if !matches!(
                            f.kind.as_str(),
                            "IFCINDEXEDPOLYGONALFACE" | "IFCINDEXEDPOLYGONALFACEWITHVOIDS"
                        ) {
                            return Err(error("unsupported IFC polygonal face"));
                        }
                        let mut rings = vec![checked_ring(
                            list(field(f, 0)?)?.iter().map(get).collect::<Result<_>>()?,
                        )?];
                        if f.kind == "IFCINDEXEDPOLYGONALFACEWITHVOIDS" {
                            for inner in list(field(f, 1)?)? {
                                rings.push(checked_ring(
                                    list(inner)?.iter().map(get).collect::<Result<_>>()?,
                                )?);
                            }
                        }
                        for triangle in triangles(&rings)? {
                            self.emit(scene, triangle, t, face_color)?;
                        }
                    }
                }
            }
            "IFCFACETEDBREP"
            | "IFCSHELLBASEDSURFACEMODEL"
            | "IFCFACEBASEDSURFACEMODEL"
            | "IFCCLOSEDSHELL"
            | "IFCOPENSHELL"
            | "IFCCONNECTEDFACESET"
            | "IFCGEOMETRICSET"
            | "IFCGEOMETRICCURVESET" => {
                let children = if e.kind == "IFCFACETEDBREP" {
                    vec![reference(field(e, 0)?)?]
                } else {
                    refs(field(e, 0)?)?
                };
                for child in children {
                    self.geometry(child, t, color, depth + 1, scene)?;
                }
            }
            "IFCFACE" | "IFCFACESURFACE" | "IFCADVANCEDFACE" => self.face(id, t, color, scene)?,
            "IFCPOLYLINE"
            | "IFCINDEXEDPOLYCURVE"
            | "IFCCOMPOSITECURVE"
            | "IFCCIRCLE"
            | "IFCELLIPSE" => {
                let path = self.curve(id, 0)?;
                if scene.lines.len() + path.len() > MAX_GEOMETRY {
                    return Err(error("IFC line count exceeds limit"));
                }
                for pair in path.windows(2) {
                    scene.lines.push(Line {
                        vertices: [t.apply(pair[0]), t.apply(pair[1])],
                        color,
                    });
                }
            }
            "IFCCARTESIANPOINT" => {
                if scene.points.len() >= MAX_GEOMETRY {
                    return Err(error("IFC point count exceeds limit"));
                }
                scene.points.push(Point {
                    position: t.apply(self.point(id)?),
                    color,
                });
            }
            _ => {
                return Err(error(format!(
                    "unsupported IFC required geometry {} (#{id})",
                    e.kind
                )));
            }
        }
        Ok(())
    }
}
fn rectangle(x: f64, y: f64) -> Vec<Vec3> {
    [(-x, -y), (x, -y), (x, y), (-x, y)]
        .map(|(x, y)| Vec3::new(x, y, 0.))
        .to_vec()
}
fn fillets(ring: Vec<Vec3>, corners: &[(usize, f64)]) -> Result<Vec<Vec3>> {
    let mut result = Vec::new();
    for i in 0..ring.len() {
        let radius = corners
            .iter()
            .find(|(index, _)| *index == i)
            .map(|(_, r)| *r)
            .unwrap_or(0.);
        if radius == 0. {
            result.push(ring[i]);
            continue;
        }
        if radius < 0. || !radius.is_finite() {
            return Err(error("invalid IFC profile fillet radius"));
        }
        let a = ring[(i + ring.len() - 1) % ring.len()];
        let b = ring[i];
        let c = ring[(i + 1) % ring.len()];
        let u = normalized(a - b)?;
        let v = normalized(c - b)?;
        let angle = u.dot(v).clamp(-1., 1.).acos();
        let distance = radius / (angle / 2.).tan();
        if !distance.is_finite()
            || distance > (a - b).length() / 2.
            || distance > (c - b).length() / 2.
        {
            return Err(error("IFC profile fillet radius exceeds adjacent edges"));
        }
        let center = b + normalized(u + v)? * (radius / (angle / 2.).sin());
        let start = b + u * distance;
        let end = b + v * distance;
        let theta = (start.y - center.y).atan2(start.x - center.x);
        let mut sweep =
            ((end.y - center.y).atan2(end.x - center.x) - theta + PI).rem_euclid(TAU) - PI;
        let turn = (b - a).cross(c - b).z;
        if sweep * turn < 0. {
            sweep += if turn > 0. { TAU } else { -TAU };
        }
        let steps = (sweep.abs() / TAU * CURVE_SEGMENTS as f64).ceil().max(2.) as usize;
        for j in 0..=steps {
            let angle = theta + sweep * j as f64 / steps as f64;
            result.push(center + Vec3::new(radius * angle.cos(), radius * angle.sin(), 0.));
        }
    }
    Ok(result)
}
fn append_path(result: &mut Vec<Vec3>, mut next: Vec<Vec3>) -> Result<()> {
    if next.is_empty() {
        return Err(error("empty IFC curve segment"));
    }
    if let Some(last) = result.last() {
        if (*last - next[0]).length() > 1e-7 * last.length().max(next[0].length()).max(1.) {
            return Err(error("IFC composite curve segments are disconnected"));
        }
        next.remove(0);
    }
    if result
        .len()
        .checked_add(next.len())
        .is_none_or(|n| n > MAX_GEOMETRY)
    {
        return Err(error("IFC curve length exceeds limit"));
    }
    result.extend(next);
    Ok(())
}
fn arc(a: Vec3, b: Vec3, c: Vec3) -> Result<Vec<Vec3>> {
    let ab = b - a;
    let ac = c - a;
    let cross = ab.cross(ac);
    let d = 2. * cross.dot(cross);
    if !d.is_finite() || d <= 1e-24 {
        return Err(error("IFC circular arc points are collinear"));
    }
    let center = a + (ac.cross(cross) * ab.dot(ab) + cross.cross(ab) * ac.dot(ac)) / d;
    let x = normalized(a - center)?;
    let z = normalized(cross)?;
    let y = z.cross(x);
    let angle = |p: Vec3| {
        let p = p - center;
        p.dot(y).atan2(p.dot(x)).rem_euclid(TAU)
    };
    let end = angle(c);
    let mid = angle(b);
    let sweep = if mid <= end { end } else { end - TAU };
    let steps = (sweep.abs() / TAU * CURVE_SEGMENTS as f64).ceil().max(2.) as usize;
    let radius = (a - center).length();
    let mut points: Vec<_> = (0..=steps)
        .map(|i| {
            let theta = sweep * i as f64 / steps as f64;
            center + (x * theta.cos() + y * theta.sin()) * radius
        })
        .collect();
    points[0] = a;
    points[steps] = c;
    Ok(points)
}

fn nesting(text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let (mut i, mut depth) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                loop {
                    let b = *bytes
                        .get(i)
                        .ok_or_else(|| error("unterminated IFC string"))?;
                    i += 1;
                    if b == b'\'' {
                        if bytes.get(i) == Some(&b'\'') {
                            i += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while bytes.get(i..i + 2) != Some(b"*/") {
                    if i >= bytes.len() {
                        return Err(error("unterminated IFC comment"));
                    }
                    i += 1;
                }
                i += 2;
            }
            b'(' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(error("IFC parameter nesting exceeds limit"));
                }
                i += 1;
            }
            b')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| error("unbalanced IFC parentheses"))?;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return Err(error("unbalanced IFC parentheses"));
    }
    Ok(())
}
fn unit(model: &Model, id: u64, depth: usize) -> Result<f64> {
    if depth > MAX_DEPTH {
        return Err(error("IFC unit references are cyclic or too deep"));
    }
    let e = model.entity(id)?;
    match e.kind.as_str() {
        "IFCSIUNIT" => {
            if !matches!(text(field(e, 3)?), Some("METRE" | "RADIAN")) {
                return Err(error("unsupported IFC SI geometry unit"));
            }
            match text(field(e, 2)?) {
                None => Ok(1.),
                Some("MILLI") => Ok(1e-3),
                Some("CENTI") => Ok(1e-2),
                Some("DECI") => Ok(1e-1),
                Some("KILO") => Ok(1e3),
                Some("MICRO") => Ok(1e-6),
                Some("NANO") => Ok(1e-9),
                Some("MEGA") => Ok(1e6),
                Some(_) => Err(error("unsupported IFC SI geometry unit prefix")),
            }
        }
        "IFCCONVERSIONBASEDUNIT" => {
            let factor = model.expected(reference(field(e, 3)?)?, "IFCMEASUREWITHUNIT")?;
            let result = positive(field(factor, 0)?)?
                * unit(model, reference(field(factor, 1)?)?, depth + 1)?;
            if !result.is_finite() {
                return Err(error("IFC unit conversion overflow"));
            }
            Ok(result)
        }
        _ => Err(error("unsupported IFC geometry unit")),
    }
}
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let source = std::str::from_utf8(&bytes)?.trim_start_matches('\u{feff}');
    parse(source)
}
fn parse(source: &str) -> Result<Scene> {
    nesting(source)?;
    let exchange = ruststep::parser::parse(source)
        .map_err(|_| error("invalid IFC STEP Part 21 exchange file"))?;
    let schema = exchange
        .header
        .iter()
        .find(|r| r.name.as_str() == "FILE_SCHEMA")
        .and_then(|r| list(&r.parameter).ok())
        .and_then(|p| p.first())
        .and_then(|p| list(p).ok())
        .and_then(|p| p.first())
        .and_then(text)
        .ok_or_else(|| error("IFC exchange requires FILE_SCHEMA"))?;
    if schema != "IFC2X3" && !schema.starts_with("IFC4") {
        return Err(error(format!("unsupported IFC schema {schema}")));
    }
    if !exchange.anchor.is_empty() || !exchange.reference.is_empty() {
        return Err(error("IFC external STEP references are unsupported"));
    }
    let mut model = Model {
        entities: HashMap::new(),
        colors: HashMap::new(),
        face_colors: HashMap::new(),
        length_scale: 1.,
        angle_scale: 1.,
        visits: Cell::new(0),
    };
    for entity in exchange
        .data
        .into_iter()
        .flat_map(|section| section.entities)
    {
        if model.entities.len() >= MAX_ENTITIES {
            return Err(error("IFC entity count exceeds limit"));
        }
        let EntityInstance::Simple { id, record } = entity else {
            return Err(error("IFC complex STEP entities are unsupported"));
        };
        let Parameter::List(fields) = record.parameter else {
            return Err(error("invalid IFC entity attribute list"));
        };
        if model
            .entities
            .insert(
                id,
                Entity {
                    kind: record.name,
                    fields,
                },
            )
            .is_some()
        {
            return Err(error("duplicate IFC entity ID"));
        }
    }
    for e in model.entities.values().filter(|e| e.kind == "IFCPROJECT") {
        if let Some(units) = optional_reference(field(e, 8)?)? {
            for id in refs(field(model.expected(units, "IFCUNITASSIGNMENT")?, 0)?)? {
                let u = model.entity(id)?;
                let kind = text(field(u, 1)?).map(str::to_owned);
                if kind.as_deref() == Some("LENGTHUNIT") {
                    model.length_scale = unit(&model, id, 0)?;
                }
                if kind.as_deref() == Some("PLANEANGLEUNIT") {
                    model.angle_scale = unit(&model, id, 0)?;
                }
            }
        }
    }
    model.collect_styles()?;
    let mut products: Vec<_> = model
        .entities
        .iter()
        .filter_map(|(&id, e)| {
            let p = e
                .fields
                .get(6)
                .and_then(|p| optional_reference(p).ok())
                .flatten()?;
            model.entities.get(&p).filter(|e| {
                matches!(
                    e.kind.as_str(),
                    "IFCPRODUCTDEFINITIONSHAPE" | "IFCPRODUCTREPRESENTATION"
                )
            })?;
            Some((id, p))
        })
        .collect();
    products.sort_unstable();
    let mut openings = HashMap::<u64, Vec<u64>>::new();
    let mut voids = HashSet::new();
    for e in model
        .entities
        .values()
        .filter(|e| e.kind == "IFCRELVOIDSELEMENT")
    {
        let host = reference(field(e, 4)?)?;
        let opening = reference(field(e, 5)?)?;
        model.entity(host)?;
        model.entity(opening)?;
        if host == opening {
            return Err(error("IFC product cannot void itself"));
        }
        if !voids.insert(opening) {
            return Err(error("IFC opening is assigned to multiple products"));
        }
        openings.entry(host).or_default().push(opening);
    }
    let mut scene = Scene::default();
    for (product, _) in products {
        let p = model.entity(product)?;
        if voids.contains(&product)
            || matches!(
                p.kind.as_str(),
                "IFCOPENINGELEMENT" | "IFCOPENINGSTANDARDCASE" | "IFCSPACE"
            )
        {
            continue;
        }
        let mut item = model.product(product)?;
        if let Some(openings) = openings.get(&product) {
            if !item.lines.is_empty() || !item.points.is_empty() {
                return Err(error(
                    "IFC products with openings require solid body geometry",
                ));
            }
            let color = item.triangles.first().and_then(|t| t.color);
            for &opening in openings {
                let mut cut = model.product(opening)?;
                if !cut.lines.is_empty() || !cut.points.is_empty() {
                    return Err(error("IFC openings require solid body geometry"));
                }
                for t in &mut cut.triangles {
                    t.color = color;
                }
                item.triangles = csg::boolean(item.triangles, cut.triangles, "DIFFERENCE")?;
            }
        }
        if scene
            .triangles
            .len()
            .checked_add(item.triangles.len())
            .is_none_or(|n| n > MAX_GEOMETRY)
            || scene
                .lines
                .len()
                .checked_add(item.lines.len())
                .is_none_or(|n| n > MAX_GEOMETRY)
            || scene
                .points
                .len()
                .checked_add(item.points.len())
                .is_none_or(|n| n > MAX_GEOMETRY)
        {
            return Err(error("IFC scene geometry count exceeds limit"));
        }
        scene.triangles.extend(item.triangles);
        scene.lines.extend(item.lines);
        scene.points.extend(item.points);
    }
    for tri in &mut scene.triangles {
        for p in &mut tri.vertices {
            *p = *p * model.length_scale;
        }
    }
    for line in &mut scene.lines {
        for p in &mut line.vertices {
            *p = *p * model.length_scale;
        }
    }
    for p in &mut scene.points {
        p.position = p.position * model.length_scale;
    }
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(geometry: &str) -> String {
        format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;
            #1=IFCCARTESIANPOINT((0.,0.,0.));#2=IFCAXIS2PLACEMENT3D(#1,$,$);
            #3=IFCLOCALPLACEMENT($,#2);
            #4=IFCBUILDINGELEMENTPROXY('id',$,'shape',$,$,#3,#5,$,$);
            #5=IFCPRODUCTDEFINITIONSHAPE($,$,(#6));
            #6=IFCSHAPEREPRESENTATION($,'Body','SolidModel',(#20));
            {geometry}ENDSEC;END-ISO-10303-21;"
        )
    }
    fn volume(scene: &Scene) -> f64 {
        scene
            .triangles
            .iter()
            .map(|t| t.vertices[0].dot(t.vertices[1].cross(t.vertices[2])) / 6.)
            .sum()
    }
    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    }
    #[test]
    fn hollow_extrusion_has_real_caps_cavity_and_volume() {
        let scene = parse(&fixture(
            "#20=IFCEXTRUDEDAREASOLID(#21,$,#22,2.);
            #21=IFCRECTANGLEHOLLOWPROFILEDEF(.AREA.,$,$,4.,4.,1.,$,$);
            #22=IFCDIRECTION((0.,0.,1.));",
        ))
        .unwrap();
        near(volume(&scene), 24.);
        let cap_area: f64 = scene
            .triangles
            .iter()
            .filter(|t| t.vertices.iter().all(|p| p.z == 2.))
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        near(cap_area, 12.);
        assert!(crate::render::render(&scene, &Default::default()).is_ok());
    }
    #[test]
    fn mirrored_derived_profile_retains_outward_winding() {
        let scene = parse(&fixture(
            "#20=IFCEXTRUDEDAREASOLID(#21,$,#26,2.);
            #21=IFCDERIVEDPROFILEDEF(.AREA.,$,#22,#23,$);
            #22=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,2.,2.);
            #23=IFCCARTESIANTRANSFORMATIONOPERATOR2D($,#24,#25,1.);
            #24=IFCDIRECTION((0.,-1.));#25=IFCCARTESIANPOINT((0.,0.));
            #26=IFCDIRECTION((0.,0.,1.));",
        ))
        .unwrap();
        near(volume(&scene), 8.);
    }
    #[test]
    fn mapped_geometry_uses_source_frame_and_nonuniform_mirroring() {
        let scene = parse(&fixture(
            "#20=IFCMAPPEDITEM(#21,#25);
            #21=IFCREPRESENTATIONMAP(#22,#23);#22=IFCAXIS2PLACEMENT3D(#24,$,$);
            #24=IFCCARTESIANPOINT((10.,0.,0.));
            #23=IFCSHAPEREPRESENTATION($,'Body','CSG',(#30));
            #25=IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM($,#26,#27,2.,$,3.,4.);
            #26=IFCDIRECTION((0.,-1.,0.));#27=IFCCARTESIANPOINT((5.,0.,0.));
            #30=IFCBLOCK(#2,1.,1.,1.);",
        ))
        .unwrap();
        near(volume(&scene), 24.);
        let points: Vec<_> = scene.triangles.iter().flat_map(|t| t.vertices).collect();
        near(
            points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
            25.,
        );
        near(
            points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
            27.,
        );
        near(
            points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
            -3.,
        );
        near(
            points.iter().map(|p| p.z).fold(f64::NEG_INFINITY, f64::max),
            4.,
        );
    }
    #[test]
    fn tessellation_pn_index_and_indexed_colors_are_validated() {
        let source = fixture(
            "#20=IFCTRIANGULATEDFACESET(#21,$,.F.,((1,2,3)),(3,1,2));
            #21=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)),$);
            #22=IFCINDEXEDCOLOURMAP(#20,$,#23,(1));#23=IFCCOLOURRGBLIST(((1.,0.,0.)));",
        );
        let scene = parse(&source).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.triangles[0].vertices[0], Vec3::new(0., 1., 0.));
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert!(parse(&source.replace("(3,1,2)", "(3,1,9)")).is_err());
        assert!(parse(&source.replace("((1,2,3))", "((0,2,3))")).is_err());
    }
    #[test]
    fn solid_boolean_operations_preserve_analytic_box_volumes() {
        for (operator, expected) in [("UNION", 1.5), ("INTERSECTION", 0.5), ("DIFFERENCE", 0.5)] {
            let scene = parse(&fixture(&format!(
                "#20=IFCBOOLEANRESULT(.{operator}.,#21,#22);
                #21=IFCBLOCK(#2,1.,1.,1.);#22=IFCBLOCK(#23,1.,1.,1.);
                #23=IFCAXIS2PLACEMENT3D(#24,$,$);#24=IFCCARTESIANPOINT((0.5,0.,0.));"
            )))
            .unwrap();
            near(volume(&scene), expected);
        }
    }
    #[test]
    fn halfspace_clipping_obeys_ifc_agreement_flag_and_caps_the_cut() {
        for (agreement, minimum, maximum) in [("T", 0.5, 1.), ("F", 0., 0.5)] {
            let scene = parse(&fixture(&format!(
                "#20=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#21,#22);
                #21=IFCBLOCK(#2,1.,1.,1.);#22=IFCHALFSPACESOLID(#23,.{agreement}.);
                #23=IFCPLANE(#24);#24=IFCAXIS2PLACEMENT3D(#25,$,$);
                #25=IFCCARTESIANPOINT((0.,0.,0.5));"
            )))
            .unwrap();
            near(volume(&scene), 0.5);
            let positions: Vec<_> = scene.triangles.iter().flat_map(|t| t.vertices).collect();
            near(
                positions.iter().map(|p| p.z).fold(f64::INFINITY, f64::min),
                minimum,
            );
            near(
                positions
                    .iter()
                    .map(|p| p.z)
                    .fold(f64::NEG_INFINITY, f64::max),
                maximum,
            );
        }
    }
    #[test]
    fn revolution_converts_project_length_and_angle_units() {
        let scene = parse(&fixture(
            "#20=IFCREVOLVEDAREASOLID(#21,$,#24,360.);
            #21=IFCRECTANGLEPROFILEDEF(.AREA.,$,#22,1000.,1000.);
            #22=IFCAXIS2PLACEMENT2D(#23,$);#23=IFCCARTESIANPOINT((2000.,0.));
            #24=IFCAXIS1PLACEMENT(#1,#25);#25=IFCDIRECTION((0.,1.,0.));
            #90=IFCPROJECT('project',$,$,$,$,$,$,$,#91);
            #91=IFCUNITASSIGNMENT((#92,#93));#92=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);
            #93=IFCCONVERSIONBASEDUNIT(#99,.PLANEANGLEUNIT.,'degree',#94);
            #94=IFCMEASUREWITHUNIT(IFCPLANEANGLEMEASURE(0.017453292519943295),#95);
            #95=IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);
            #99=IFCDIMENSIONALEXPONENTS(0,0,0,0,0,0,0);",
        ))
        .unwrap();
        assert!((volume(&scene) - 4. * PI).abs() < 0.01);
        let radius = scene
            .triangles
            .iter()
            .flat_map(|t| t.vertices)
            .map(|p| (p.x * p.x + p.z * p.z).sqrt())
            .fold(0_f64, f64::max);
        near(radius, 2.5);
    }
    #[test]
    fn relational_opening_is_subtracted_in_absolute_product_placement() {
        let scene = parse(&fixture(
            "#20=IFCBLOCK(#2,4.,4.,1.);
            #30=IFCOPENINGELEMENT('cut',$,$,$,$,#31,#32,$,$);
            #31=IFCLOCALPLACEMENT(#3,#33);#33=IFCAXIS2PLACEMENT3D(#34,$,$);
            #34=IFCCARTESIANPOINT((1.,1.,-1.));
            #32=IFCPRODUCTDEFINITIONSHAPE($,$,(#35));
            #35=IFCSHAPEREPRESENTATION($,'Body','CSG',(#36));#36=IFCBLOCK(#2,2.,2.,3.);
            #40=IFCRELVOIDSELEMENT('relation',$,$,$,#4,#30);",
        ))
        .unwrap();
        near(volume(&scene), 12.);
        assert!(
            scene
                .triangles
                .iter()
                .any(|t| t.vertices.iter().all(|p| p.x == 1.)
                    && t.vertices.iter().all(|p| (1. ..=3.).contains(&p.y)))
        );
    }
    #[test]
    fn invalid_and_unsupported_geometry_returns_errors() {
        assert!(parse("not IFC").is_err());
        assert!(parse(&fixture("#20=IFCSPHERE(#2,-1.);")).is_err());
        assert!(parse(&fixture("#20=IFCSECTIONEDSOLIDHORIZONTAL($,$,$,$);")).is_err());
        let cyclic = fixture("#20=IFCBLOCK(#2,1.,1.,1.);")
            .replace("IFCLOCALPLACEMENT($,#2)", "IFCLOCALPLACEMENT(#3,#2)");
        assert!(parse(&cyclic).is_err());
        assert!(
            parse(&fixture(
                "#20=IFCTRIANGULATEDFACESET(#999,$,.F.,((1,2,3)),$);"
            ))
            .is_err()
        );
    }
}
