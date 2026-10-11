//! DXF: 2D drawings, the interchange the field expects.
//!
//! Reading takes a drawing's curves as written ([`read_dxf_entities`]: lines,
//! arcs, circles, ellipses, splines and bulged polylines, with the units and
//! which layers are dashed), or as polylines ([`read_dxf`]).
//!
//! R12 ASCII, the most widely readable dialect: a TABLES section declaring
//! the two linetypes and two layers a technical drawing needs, then one
//! `POLYLINE` per curve. Visible curves go on the `VISIBLE` layer with
//! continuous lines; hidden curves on `HIDDEN`, dashed. The writer takes
//! bare polylines rather than a drawing type, so anything that produces 2D
//! curves (the hidden-line projector, a section outline, a sketch) writes
//! without this crate knowing where they came from.

use ogeom_math::{Point2, Vector2};
use std::fmt::Write as _;

/// Write polylines as an R12 DXF document.
///
/// `visible` draws continuous on layer `VISIBLE`; `hidden` draws dashed on
/// layer `HIDDEN`. Polylines with fewer than two points are skipped; a
/// point is not a line in a drawing, here as everywhere.
#[must_use]
pub fn write_dxf(visible: &[Vec<Point2>], hidden: &[Vec<Point2>]) -> String {
    let mut out = String::new();
    // Header: R12 says almost nothing and needs almost nothing.
    push(
        &mut out,
        &[("0", "SECTION"), ("2", "HEADER"), ("0", "ENDSEC")],
    );

    // Tables: the linetypes first, because the layers name them.
    push(&mut out, &[("0", "SECTION"), ("2", "TABLES")]);
    push(&mut out, &[("0", "TABLE"), ("2", "LTYPE"), ("70", "2")]);
    push(
        &mut out,
        &[
            ("0", "LTYPE"),
            ("2", "CONTINUOUS"),
            ("70", "0"),
            ("3", "Solid line"),
            ("72", "65"),
            ("73", "0"),
            ("40", "0.0"),
        ],
    );
    push(
        &mut out,
        &[
            ("0", "LTYPE"),
            ("2", "DASHED"),
            ("70", "0"),
            ("3", "Dashed line"),
            ("72", "65"),
            ("73", "2"),
            ("40", "0.75"),
            ("49", "0.5"),
            ("49", "-0.25"),
        ],
    );
    push(&mut out, &[("0", "ENDTAB")]);
    push(&mut out, &[("0", "TABLE"), ("2", "LAYER"), ("70", "2")]);
    push(
        &mut out,
        &[
            ("0", "LAYER"),
            ("2", "VISIBLE"),
            ("70", "0"),
            ("62", "7"),
            ("6", "CONTINUOUS"),
        ],
    );
    push(
        &mut out,
        &[
            ("0", "LAYER"),
            ("2", "HIDDEN"),
            ("70", "0"),
            ("62", "8"),
            ("6", "DASHED"),
        ],
    );
    push(&mut out, &[("0", "ENDTAB"), ("0", "ENDSEC")]);

    push(&mut out, &[("0", "SECTION"), ("2", "ENTITIES")]);
    for (layer, curves) in [("VISIBLE", visible), ("HIDDEN", hidden)] {
        for curve in curves {
            if curve.len() < 2 {
                continue;
            }
            push(
                &mut out,
                &[("0", "POLYLINE"), ("8", layer), ("66", "1"), ("70", "0")],
            );
            for p in curve {
                push(&mut out, &[("0", "VERTEX"), ("8", layer)]);
                let _ = writeln!(out, "10\n{}\n20\n{}\n30\n0.0", real(p.x), real(p.y));
            }
            push(&mut out, &[("0", "SEQEND")]);
        }
    }
    push(&mut out, &[("0", "ENDSEC"), ("0", "EOF")]);
    out
}

/// A DXF real: shortest exact form, decimal point guaranteed.
fn real(v: f64) -> String {
    let s = format!("{v:?}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Append group-code/value pairs, one per line each.
fn push(out: &mut String, pairs: &[(&str, &str)]) {
    for (code, value) in pairs {
        let _ = writeln!(out, "{code}\n{value}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_drawing_writes_layers_polylines_and_exact_coordinates() {
        let visible = vec![vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 5.0),
        ]];
        let hidden = vec![vec![Point2::new(1.5, 2.25), Point2::new(3.0, 2.25)]];
        let text = write_dxf(&visible, &hidden);

        assert!(text.starts_with("0\nSECTION"));
        assert!(text.trim_end().ends_with("EOF"));
        assert_eq!(text.matches("POLYLINE").count(), 2);
        assert_eq!(text.matches("VERTEX").count(), 5);
        assert_eq!(text.matches("SEQEND").count(), 2);
        // Both layers declared and used, hidden dashed.
        assert!(text.contains("VISIBLE"));
        assert!(text.contains("HIDDEN"));
        assert!(text.contains("DASHED"));
        // Coordinates exact and point-carrying.
        assert!(text.contains("10\n10.0\n20\n5.0"));
        assert!(text.contains("10\n1.5\n20\n2.25"));
    }

    #[test]
    fn degenerate_polylines_are_dropped() {
        let text = write_dxf(&[vec![Point2::new(1.0, 1.0)]], &[vec![]]);
        assert_eq!(text.matches("POLYLINE").count(), 0);
    }
}

/// The polylines a DXF carries, by the layer they were drawn on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DxfDrawing {
    /// Curves on the `VISIBLE` layer, or on no named layer at all.
    pub visible: Vec<Vec<Point2>>,
    /// Curves on the `HIDDEN` layer.
    pub hidden: Vec<Vec<Point2>>,
}

/// Read the polylines out of an ASCII DXF.
///
/// `POLYLINE` and `LWPOLYLINE` become polylines, a closed one ending on
/// its first point again; `LINE` becomes a polyline of two points. A
/// polyline's bulges are read as straight chords here: the typed reader,
/// [`read_dxf_entities`], keeps them and every curve besides. Hidden
/// curves are those on the `HIDDEN` layer or drawn in a dashed or hidden
/// linetype.
///
/// # Errors
///
/// As [`read_dxf_entities`].
pub fn read_dxf(text: &str) -> ogeom_core::OgeomResult<DxfDrawing> {
    let mut out = DxfDrawing::default();
    for entity in read_dxf_entities(text)?.entities {
        let points = match entity.curve {
            DxfCurve::Line { start, end } => vec![start, end],
            DxfCurve::Polyline { vertices, closed } => {
                let mut points: Vec<Point2> = vertices.iter().map(|v| v.0).collect();
                if closed
                    && points.len() > 2
                    && let (Some(first), Some(last)) = (points.first(), points.last())
                    && first.distance(*last) > 0.0
                {
                    points.push(*first);
                }
                points
            }
            _ => continue,
        };
        if points.len() < 2 {
            continue;
        }
        if entity.hidden {
            out.hidden.push(points);
        } else {
            out.visible.push(points);
        }
    }
    Ok(out)
}

/// A DXF's drawing entities, typed, with the drawing's units.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DxfEntities {
    /// `$INSUNITS` from the HEADER, when present (0 is unitless).
    pub insunits: Option<i32>,
    /// Millimetres per drawing unit, when `insunits` names a length unit.
    pub unit_mm: Option<f64>,
    /// The ENTITIES section's curves, in file order.
    pub entities: Vec<DxfEntity>,
}

/// One entity: the curve, and where it was drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct DxfEntity {
    /// Group 8, as written (`"0"` when absent).
    pub layer: String,
    /// Whether the layer is `HIDDEN`, or the entity's or its layer's
    /// linetype is dashed or hidden.
    pub hidden: bool,
    /// The curve.
    pub curve: DxfCurve,
}

/// A DXF curve in the drawing's plane.
#[derive(Debug, Clone, PartialEq)]
pub enum DxfCurve {
    /// A segment.
    Line {
        /// Where it starts.
        start: Point2,
        /// Where it ends.
        end: Point2,
    },
    /// Counter-clockwise from `start_angle` to `end_angle`, in radians.
    Arc {
        /// The centre.
        centre: Point2,
        /// The radius.
        radius: f64,
        /// Where it starts, radians.
        start_angle: f64,
        /// Where it ends, radians.
        end_angle: f64,
    },
    /// A full circle.
    Circle {
        /// The centre.
        centre: Point2,
        /// The radius.
        radius: f64,
    },
    /// An ellipse or elliptic arc, parameters as DXF gives them: a full
    /// ellipse when they span two pi.
    Ellipse {
        /// The centre.
        centre: Point2,
        /// The major axis's end, relative to the centre.
        major: Vector2,
        /// Minor over major.
        ratio: f64,
        /// Start parameter.
        start_param: f64,
        /// End parameter.
        end_param: f64,
    },
    /// A B-spline as written: degree, knots, control points and weights.
    /// A spline given only by fit points carries them as its control
    /// points and no knots.
    Spline {
        /// The degree.
        degree: usize,
        /// The knot vector.
        knots: Vec<f64>,
        /// The control points, or the fit points where there are none.
        control_points: Vec<Point2>,
        /// The weights, for a rational spline.
        weights: Option<Vec<f64>>,
        /// Whether the spline is closed.
        closed: bool,
    },
    /// A `POLYLINE` or `LWPOLYLINE`: each vertex with the bulge of the
    /// segment that starts at it (the tangent of a quarter of the included
    /// angle, positive counter-clockwise).
    Polyline {
        /// The vertices and their bulges.
        vertices: Vec<(Point2, f64)>,
        /// Whether the last vertex joins the first.
        closed: bool,
    },
}

/// Millimetres per unit for a `$INSUNITS` code that names a length.
fn unit_mm(code: i32) -> Option<f64> {
    Some(match code {
        1 => 25.4,
        2 => 304.8,
        3 => 1_609_344.0,
        4 => 1.0,
        5 => 10.0,
        6 => 1000.0,
        7 => 1.0e6,
        8 => 2.54e-5,
        9 => 0.0254,
        10 => 914.4,
        11 => 1.0e-7,
        12 => 1.0e-6,
        13 => 1.0e-3,
        14 => 100.0,
        15 => 1.0e4,
        16 => 1.0e5,
        17 => 1.0e12,
        18 => 1.495_978_707e14,
        19 => 9.460_730_472_580_8e18,
        20 => 3.085_677_581_491_367e19,
        _ => return None,
    })
}

/// One group-coded record: the entity or table entry a `0` code opens, and
/// the pairs up to the next.
struct Record<'a> {
    kind: &'a str,
    pairs: Vec<(i32, &'a str)>,
}

impl Record<'_> {
    fn text(&self, code: i32) -> Option<&str> {
        self.pairs.iter().find(|p| p.0 == code).map(|p| p.1)
    }

    fn real(&self, code: i32) -> Option<f64> {
        self.text(code).and_then(|v| v.parse().ok())
    }

    fn int(&self, code: i32) -> Option<i64> {
        self.text(code).and_then(|v| v.parse().ok())
    }

    fn point(&self, x: i32, y: i32) -> Option<Point2> {
        Some(Point2::new(self.real(x)?, self.real(y)?))
    }

    fn reals(&self, code: i32) -> Vec<f64> {
        self.pairs
            .iter()
            .filter(|p| p.0 == code)
            .filter_map(|p| p.1.parse().ok())
            .collect()
    }

    /// Points given as repeated `x`/`y` pairs, in order.
    fn points(&self, x: i32, y: i32) -> Vec<Point2> {
        let mut out: Vec<Point2> = Vec::new();
        for (code, value) in &self.pairs {
            let Ok(v) = value.parse::<f64>() else {
                continue;
            };
            if *code == x {
                out.push(Point2::new(v, 0.0));
            } else if *code == y
                && let Some(last) = out.last_mut()
            {
                last.y = v;
            }
        }
        out
    }
}

/// Whether a linetype name draws hidden lines.
fn dashed(linetype: &str) -> bool {
    let name = linetype.to_ascii_uppercase();
    name.contains("HIDDEN") || name.contains("DASH")
}

/// Read the typed entities, the units and the layers' linetypes out of an
/// ASCII DXF.
///
/// Lines, arcs, circles, ellipses, splines, `POLYLINE` and `LWPOLYLINE`
/// are read from the ENTITIES section, with a polyline's closed flag and
/// bulges; a `POLYLINE` header's own point is its elevation, not a
/// vertex. An entity whose extrusion is `(0, 0, -1)` is seen from below,
/// and its planar coordinates are mirrored in x to the drawing's own view.
/// Blocks, inserts, text, dimensions and hatches are skipped.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// file is not group-coded in pairs, or an entity's extrusion is neither
/// up nor down, or an ellipse or spline is seen from below.
pub fn read_dxf_entities(text: &str) -> ogeom_core::OgeomResult<DxfEntities> {
    // A DXF is a stream of (code, value) pairs, one per line each.
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    if !lines.len().is_multiple_of(2) && !lines.last().is_some_and(|l| l.is_empty()) {
        ogeom_core::ogeom_bail!(
            Construction,
            "a DXF is group codes and values in pairs; this has an odd number of lines"
        );
    }
    let mut pairs: Vec<(i32, &str)> = Vec::with_capacity(lines.len() / 2);
    for [code, value] in lines.as_chunks::<2>().0 {
        let Ok(code) = code.parse::<i32>() else {
            ogeom_core::ogeom_bail!(
                Construction,
                "a DXF group code is an integer; found {code:?}"
            );
        };
        pairs.push((code, *value));
    }

    // Records within each section, by name.
    let mut sections: Vec<(&str, Vec<Record<'_>>)> = Vec::new();
    let mut k = 0;
    while k < pairs.len() {
        if pairs[k] == (0, "SECTION") && k + 1 < pairs.len() && pairs[k + 1].0 == 2 {
            let name = pairs[k + 1].1;
            k += 2;
            let mut records: Vec<Record<'_>> = Vec::new();
            // The pairs before the section's first 0 code (the HEADER's
            // variables) make a record of their own.
            let mut current = Record {
                kind: "",
                pairs: Vec::new(),
            };
            while k < pairs.len() && pairs[k] != (0, "ENDSEC") {
                if pairs[k].0 == 0 {
                    records.push(core::mem::replace(
                        &mut current,
                        Record {
                            kind: pairs[k].1,
                            pairs: Vec::new(),
                        },
                    ));
                } else {
                    current.pairs.push(pairs[k]);
                }
                k += 1;
            }
            records.push(current);
            sections.push((name, records));
        }
        k += 1;
    }

    let mut out = DxfEntities::default();
    let mut layer_linetype: ogeom_core::FastMap<String, String> = ogeom_core::FastMap::default();
    for (name, records) in &sections {
        match *name {
            "HEADER" => {
                for record in records {
                    let mut it = record.pairs.iter();
                    while let Some((code, value)) = it.next() {
                        if *code == 9
                            && *value == "$INSUNITS"
                            && let Some((70, units)) = it.next()
                            && let Ok(units) = units.parse::<i32>()
                        {
                            out.insunits = Some(units);
                            out.unit_mm = unit_mm(units);
                        }
                    }
                }
            }
            "TABLES" => {
                for record in records.iter().filter(|r| r.kind == "LAYER") {
                    if let Some(layer) = record.text(2) {
                        layer_linetype.insert(
                            layer.to_ascii_uppercase(),
                            record.text(6).unwrap_or("").to_string(),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    let Some((_, records)) = sections.iter().find(|(name, _)| *name == "ENTITIES") else {
        return Ok(out);
    };
    let mut k = 0;
    while k < records.len() {
        let record = &records[k];
        k += 1;
        let layer = record.text(8).unwrap_or("0").to_string();
        let hidden = layer.eq_ignore_ascii_case("HIDDEN")
            || record.text(6).is_some_and(dashed)
            || layer_linetype
                .get(&layer.to_ascii_uppercase())
                .is_some_and(|l| dashed(l));
        // The arbitrary axis: straight up reads as written, straight down
        // mirrors x; anything tilted is not a drawing's plane.
        let from_below = match record.real(230) {
            None => false,
            Some(z)
                if (record.real(210).unwrap_or(0.0).abs()
                    + record.real(220).unwrap_or(0.0).abs())
                    <= 1e-12 =>
            {
                z < 0.0
            }
            Some(_) => ogeom_core::ogeom_bail!(
                Construction,
                "a {} entity is extruded along a tilted axis; only drawings in the XY plane \
                 are read",
                record.kind
            ),
        };
        let flip = |p: Point2| {
            if from_below {
                Point2::new(-p.x, p.y)
            } else {
                p
            }
        };
        let curve = match record.kind {
            "LINE" => {
                let (Some(a), Some(b)) = (record.point(10, 20), record.point(11, 21)) else {
                    continue;
                };
                DxfCurve::Line {
                    start: flip(a),
                    end: flip(b),
                }
            }
            "CIRCLE" => {
                let (Some(centre), Some(radius)) = (record.point(10, 20), record.real(40)) else {
                    continue;
                };
                DxfCurve::Circle {
                    centre: flip(centre),
                    radius,
                }
            }
            "ARC" => {
                let (Some(centre), Some(radius)) = (record.point(10, 20), record.real(40)) else {
                    continue;
                };
                let start = record.real(50).unwrap_or(0.0).to_radians();
                let end = record.real(51).unwrap_or(360.0).to_radians();
                // Mirrored, the counter-clockwise run from start to end
                // becomes the one from the mirror of end to that of start.
                let (start_angle, end_angle) = if from_below {
                    (core::f64::consts::PI - end, core::f64::consts::PI - start)
                } else {
                    (start, end)
                };
                DxfCurve::Arc {
                    centre: flip(centre),
                    radius,
                    start_angle,
                    end_angle,
                }
            }
            "ELLIPSE" | "SPLINE" if from_below => ogeom_core::ogeom_bail!(
                Construction,
                "a {} seen from below (extrusion 0, 0, -1) is not read yet",
                record.kind
            ),
            "ELLIPSE" => {
                let (Some(centre), Some(major)) = (record.point(10, 20), record.point(11, 21))
                else {
                    continue;
                };
                DxfCurve::Ellipse {
                    centre,
                    major: Vector2::new(major.x, major.y),
                    ratio: record.real(40).unwrap_or(1.0),
                    start_param: record.real(41).unwrap_or(0.0),
                    end_param: record.real(42).unwrap_or(core::f64::consts::TAU),
                }
            }
            "SPLINE" => {
                let flags = record.int(70).unwrap_or(0);
                let control = record.points(10, 20);
                let weights = record.reals(41);
                let (control_points, knots) = if control.is_empty() {
                    (record.points(11, 21), Vec::new())
                } else {
                    (control, record.reals(40))
                };
                DxfCurve::Spline {
                    degree: usize::try_from(record.int(71).unwrap_or(3)).unwrap_or(3),
                    knots,
                    weights: (!weights.is_empty() && weights.len() == control_points.len())
                        .then_some(weights),
                    control_points,
                    closed: flags & 1 != 0,
                }
            }
            "LWPOLYLINE" => {
                let mut vertices: Vec<(Point2, f64)> = Vec::new();
                for (code, value) in &record.pairs {
                    let Ok(v) = value.parse::<f64>() else {
                        continue;
                    };
                    match code {
                        10 => vertices.push((Point2::new(v, 0.0), 0.0)),
                        20 => {
                            if let Some(last) = vertices.last_mut() {
                                last.0.y = v;
                            }
                        }
                        42 => {
                            if let Some(last) = vertices.last_mut() {
                                last.1 = v;
                            }
                        }
                        _ => {}
                    }
                }
                polyline(vertices, record.int(70).unwrap_or(0), from_below)
            }
            "POLYLINE" => {
                // The header's own 10/20 is the elevation; the vertices
                // follow as records of their own up to SEQEND.
                let mut vertices: Vec<(Point2, f64)> = Vec::new();
                while k < records.len() && records[k].kind == "VERTEX" {
                    let v = &records[k];
                    k += 1;
                    // A spline frame's control point is not on the curve.
                    if v.int(70).unwrap_or(0) & 16 != 0 {
                        continue;
                    }
                    if let Some(p) = v.point(10, 20) {
                        vertices.push((p, v.real(42).unwrap_or(0.0)));
                    }
                }
                if k < records.len() && records[k].kind == "SEQEND" {
                    k += 1;
                }
                polyline(vertices, record.int(70).unwrap_or(0), from_below)
            }
            _ => continue,
        };
        out.entities.push(DxfEntity {
            layer,
            hidden,
            curve,
        });
    }
    Ok(out)
}

/// A polyline from its vertices and flags, mirrored in x when seen from
/// below (a mirror turns every bulge the other way).
fn polyline(vertices: Vec<(Point2, f64)>, flags: i64, from_below: bool) -> DxfCurve {
    let vertices = if from_below {
        vertices
            .into_iter()
            .map(|(p, b)| (Point2::new(-p.x, p.y), -b))
            .collect()
    } else {
        vertices
    };
    DxfCurve::Polyline {
        vertices,
        closed: flags & 1 != 0,
    }
}
