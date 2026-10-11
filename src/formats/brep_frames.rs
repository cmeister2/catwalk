//! Restore authored BREP analytic coordinate frames through upstream geometry builders.
//!
//! The upstream reader supplies topology, spline data and geometry parameters. Native
//! BREP also stores an independent Y axis, which its reader reconstructs from X and Z.
//! Rebuild only framed analytic leaves, preserving nested trims, offsets and sweeps.

use crate::Result;
use ogeom_core::Tolerances;
use ogeom_geom::{
    CircleCurve, ConeSurface, Curve, Curve3d, CylinderSurface, EllipseCurve, ExtrusionSurface,
    HyperbolaCurve, OffsetCurve, OffsetSurface, ParabolaCurve, PlaneSurface, Reversible,
    RevolutionSurface, SphereSurface, Surface, SurfaceGeometry, TorusSurface, TrimmedCurve,
    TrimmedSurface,
};
use ogeom_math::{
    Circle, Cone, Cylinder, Ellipse, Frame, Hyperbola, Parabola, Plane, Sphere, Torus,
};
use ogeom_topo::{EdgeRepr, Model, NodeData, Shape};
use std::collections::HashMap;

/// Read validated, cache-normalized BREP while preserving every authored analytic axis.
///
/// # Errors
///
/// Geometry tables are malformed, upstream import fails, table ordinals do not match
/// the imported geometry store, or a reconstructed analytic primitive is invalid.
pub(super) fn read(text: &str, tolerances: Tolerances) -> Result<(Model, Shape)> {
    let tokens: Vec<_> = text.split_whitespace().collect();
    let section = |name: &str| -> Result<usize> {
        tokens
            .iter()
            .position(|token| *token == name)
            .ok_or_else(|| format!("BREP lacks {name} section").into())
    };
    let curves = section("Curves")?;
    let polygons = section("Polygon3D")?;
    let surfaces = section("Surfaces")?;
    let triangulations = section("Triangulations")?;
    if !(curves < polygons && polygons < surfaces && surfaces < triangulations) {
        return Err("BREP analytic geometry tables are out of order".into());
    }
    let curve_frames = super::validate_geometry_table(&tokens[curves + 1..polygons], 3)?;
    let surface_frames = super::validate_geometry_table(&tokens[surfaces + 1..triangulations], 0)?;
    let (mut model, root) = ogeom_io::brep::read(text, tolerances)?;
    let (curve_count, _, surface_count) = model.geometry().counts();
    if curve_count != curve_frames.len() || surface_count != surface_frames.len() {
        return Err("BREP analytic geometry table does not match imported ordinals".into());
    }
    // The public reader inserts each top-level table record in file order.
    // Keep surface handles stable for face and parameter-curve associations.
    let replacements = model
        .geometry()
        .surfaces()
        .zip(surface_frames)
        .filter_map(|((id, geometry), frame)| frame.map(|frame| (id, geometry, frame)))
        .map(|(id, geometry, frame)| Ok((id, restore_surface(geometry, frame, tolerances)?)))
        .collect::<Result<Vec<_>>>()?;
    for (id, geometry) in replacements {
        *model
            .geometry_mut()
            .surface_mut(id)
            .ok_or("BREP surface handle disappeared during frame restoration")? = geometry;
    }
    // GeometryStore exposes curve insertion rather than mutable curve access.
    // Replace edge references to newly built curves without changing topology.
    let replacements = model
        .geometry()
        .curves()
        .zip(curve_frames)
        .filter_map(|((id, geometry), frame)| frame.map(|frame| (id, geometry, frame)))
        .map(|(id, geometry, frame)| Ok((id, restore_curve(geometry, frame, tolerances)?)))
        .collect::<Result<Vec<_>>>()?;
    let mut curves = HashMap::with_capacity(replacements.len());
    for (id, geometry) in replacements {
        curves.insert(id, model.geometry_mut().add_curve(geometry));
    }
    let nodes: Vec<_> = model.nodes().map(|(id, _)| Shape::of(id)).collect();
    for shape in nodes {
        let node = model
            .node_mut(&shape)
            .ok_or("BREP topology handle disappeared during frame restoration")?;
        if let NodeData::Edge(edge) = node.data_mut() {
            for representation in &mut edge.representations {
                if let EdgeRepr::Curve3d { curve, .. } = representation
                    && let Some(replacement) = curves.get(curve)
                {
                    *curve = *replacement;
                }
            }
            edge.assert_same_parameter(false);
        }
    }
    Ok((model, root))
}

/// Rebuild one analytic curve leaf while preserving its nested parameterization.
///
/// # Errors
///
/// A framed table record unexpectedly imports as an unframed curve, or an
/// upstream primitive, trim or offset builder rejects its parameters.
fn restore_curve(curve: &Curve, frame: Frame, tolerances: Tolerances) -> Result<Curve> {
    let (start, end) = curve.domain();
    let (restored, reversed): (Curve, bool) = match curve {
        Curve::Circle(curve) => (
            CircleCurve::new(Circle::new(frame, curve.circle().radius(), tolerances)?).into(),
            curve.is_reversed(),
        ),
        Curve::Ellipse(curve) => {
            let ellipse = curve.ellipse();
            (
                EllipseCurve::new(Ellipse::new(
                    frame,
                    ellipse.major_radius(),
                    ellipse.minor_radius(),
                    tolerances,
                )?)
                .into(),
                curve.is_reversed(),
            )
        }
        Curve::Hyperbola(curve) => {
            let hyperbola = curve.hyperbola();
            (
                HyperbolaCurve::over(
                    Hyperbola::new(
                        frame,
                        hyperbola.major_radius(),
                        hyperbola.minor_radius(),
                        tolerances,
                    )?,
                    start,
                    end,
                )?
                .into(),
                curve.is_reversed(),
            )
        }
        Curve::Parabola(curve) => (
            ParabolaCurve::over(
                Parabola::new(frame, curve.parabola().focal(), tolerances)?,
                start,
                end,
            )?
            .into(),
            curve.is_reversed(),
        ),
        Curve::Trimmed(curve) => (
            TrimmedCurve::new(
                restore_curve(curve.basis(), frame, tolerances)?,
                start,
                end,
                tolerances,
            )?
            .into(),
            curve.is_reversed(),
        ),
        Curve::Offset(curve) => (
            Curve::Offset(Box::new(OffsetCurve::new(
                restore_curve(curve.basis(), frame, tolerances)?,
                curve.distance(),
                curve.reference(),
            )?)),
            false,
        ),
        _ => return Err("BREP authored frame does not match an analytic curve leaf".into()),
    };
    Ok(if reversed {
        restored.reversed()
    } else {
        restored
    })
}

/// Rebuild an analytic surface leaf or the framed basis of a nested sweep.
///
/// # Errors
///
/// A framed table record unexpectedly imports as an unframed surface, or an
/// upstream primitive, trim, offset or sweep builder rejects its parameters.
fn restore_surface(
    surface: &SurfaceGeometry,
    frame: Frame,
    tolerances: Tolerances,
) -> Result<SurfaceGeometry> {
    let (u, v) = surface.domain();
    Ok(match surface {
        SurfaceGeometry::Plane(_) => PlaneSurface::over(Plane::new(frame), u, v)?.into(),
        SurfaceGeometry::Cylinder(surface) => CylinderSurface::new(
            Cylinder::new(frame, surface.cylinder().radius(), tolerances)?,
            v,
        )?
        .into(),
        SurfaceGeometry::Cone(surface) => {
            let cone = surface.cone();
            ConeSurface::new(
                Cone::new(
                    frame,
                    cone.reference_radius(),
                    cone.half_angle(),
                    tolerances,
                )?,
                v,
            )?
            .into()
        }
        SurfaceGeometry::Sphere(surface) => {
            SphereSurface::new(Sphere::new(frame, surface.sphere().radius(), tolerances)?).into()
        }
        SurfaceGeometry::Torus(surface) => {
            let torus = surface.torus();
            TorusSurface::new(Torus::new(
                frame,
                torus.major_radius(),
                torus.minor_radius(),
                tolerances,
            )?)
            .into()
        }
        SurfaceGeometry::Extrusion(surface) => ExtrusionSurface::over(
            restore_curve(surface.curve(), frame, tolerances)?,
            surface.direction(),
            v,
        )?
        .into(),
        SurfaceGeometry::Revolution(surface) => RevolutionSurface::new(
            restore_curve(surface.curve(), frame, tolerances)?,
            surface.axis(),
            u.1,
        )?
        .into(),
        SurfaceGeometry::Trimmed(surface) => TrimmedSurface::new(
            restore_surface(surface.basis(), frame, tolerances)?,
            u,
            v,
            tolerances,
        )?
        .into(),
        SurfaceGeometry::Offset(surface) => SurfaceGeometry::Offset(Box::new(OffsetSurface::new(
            restore_surface(surface.basis(), frame, tolerances)?,
            surface.distance(),
        )?)),
        SurfaceGeometry::BSpline(_) => {
            return Err("BREP authored frame does not match an analytic surface leaf".into());
        }
    })
}

/// Analytic-coordinate checks independent of the native writer's frame serialization.
#[cfg(test)]
mod tests {
    use super::*;
    use ogeom_math::{Direction, Point};
    use std::{
        f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU},
        fmt::Write,
    };

    /// An authored left-handed chart with positive X/Y and negative Z.
    const LEFT_FRAME: &str = "7 -2 5 0 0 -1 1 0 0 0 1 0";

    /// Assemble native geometry records with minimal topology referencing every record.
    fn document(curves: &[String], surfaces: &[String]) -> String {
        let mut text = format!(
            "DBRep_DrawableShape\nCASCADE Topology V1, (c) Matra-Datavision\n\
             Locations 0\nCurve2ds 0\nCurves {}\n{}\n\
             Polygon3D 0\nPolygonOnTriangulations 0\nSurfaces {}\n{}\n\
             Triangulations 0\nTShapes {}\n",
            curves.len(),
            curves.join("\n"),
            surfaces.len(),
            surfaces.join("\n"),
            curves.len() + surfaces.len() + 1,
        );
        for index in 1..=curves.len() {
            writeln!(text, "Ed 1e-7 0 0 0 1 {index} 0 0 1 0 0001000 *").unwrap();
        }
        for index in 1..=surfaces.len() {
            writeln!(text, "Fa 0 1e-7 {index} 0 0001000 *").unwrap();
        }
        text.push_str("Co 0001000 ");
        for number in (2..=curves.len() + surfaces.len() + 1).rev() {
            write!(text, "+{number} 0 ").unwrap();
        }
        text.push_str("*\n+1 0\n");
        text
    }

    /// Obtain the actual remapped curves in their source edge order.
    fn edge_curves(model: &Model) -> Vec<&Curve> {
        model
            .nodes()
            .filter_map(|(_, node)| {
                let edge = node.data().as_edge()?;
                let EdgeRepr::Curve3d { curve, .. } = edge.curve3d()? else {
                    return None;
                };
                model.geometry().curve(*curve)
            })
            .collect()
    }

    /// Compare an evaluated coordinate with an independently specified Cartesian point.
    fn assert_point(actual: Point, expected: Point) {
        assert!(
            actual.distance(expected) < 1e-10,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn raw_conic_records_preserve_authored_axes_and_domains() {
        let records = [
            format!("2 {LEFT_FRAME} 2"),
            format!("3 {LEFT_FRAME} 3 1"),
            format!("4 {LEFT_FRAME} 2"),
            format!("5 {LEFT_FRAME} 3 1"),
            "1 7 -2 5 0 1 0".to_owned(),
            "7 0 0 1 2 2 7 -2 5 7 0 5 0 2 1 2".to_owned(),
            "2 7 -2 5 0 0 1 1 0 0 0 1 0 2".to_owned(),
        ];
        let (model, _) = read(&document(&records, &[]), super::super::TOLERANCES).unwrap();
        let curves = edge_curves(&model);
        let cases = [
            (0, FRAC_PI_2, Point::new(7., 0., 5.)),
            (1, FRAC_PI_2, Point::new(7., -1., 5.)),
            (2, 2., Point::new(7.5, 0., 5.)),
            (
                3,
                1.,
                Point::new(7. + 3. * 1f64.cosh(), -2. + 1f64.sinh(), 5.),
            ),
            (4, 2., Point::new(7., 0., 5.)),
            (5, 0.5, Point::new(7., -1., 5.)),
            (6, FRAC_PI_2, Point::new(7., 0., 5.)),
        ];
        for (index, parameter, expected) in cases {
            assert_point(
                curves[index]
                    .point_at(parameter, super::super::TOLERANCES)
                    .unwrap(),
                expected,
            );
        }
        assert_eq!(curves[0].domain(), (0., TAU));
        assert_eq!(curves[1].domain(), (0., TAU));
        assert_eq!(curves[2].domain(), (-1e6, 1e6));
        assert_eq!(curves[3].domain(), (-1e6, 1e6));
        assert_eq!(curves[4].domain(), (-1e6, 1e6));
        assert_eq!(curves[5].domain(), (0., 1.));
    }

    #[test]
    fn raw_analytic_surfaces_keep_coordinates_and_native_domains() {
        let records = [
            format!("1 {LEFT_FRAME}"),
            format!("2 {LEFT_FRAME} 2"),
            format!("3 {LEFT_FRAME} 2 {FRAC_PI_4}"),
            format!("4 {LEFT_FRAME} 2"),
            format!("5 {LEFT_FRAME} 3 1"),
            "9 0 0 0 0 1 1 2 2 2 2 7 -2 5 9 -2 5 7 0 5 9 0 5 0 2 1 2 0 2 1 2".to_owned(),
        ];
        let (model, _) = read(&document(&[], &records), super::super::TOLERANCES).unwrap();
        let surfaces: Vec<_> = model
            .geometry()
            .surfaces()
            .map(|(_, surface)| surface)
            .collect();
        let cases = [
            (0, 1., 2., Point::new(8., 0., 5.)),
            (1, FRAC_PI_2, 1., Point::new(7., 0., 4.)),
            (2, FRAC_PI_2, 1., Point::new(7., 1., 4.)),
            (3, FRAC_PI_2, 0., Point::new(7., 0., 5.)),
            (4, FRAC_PI_2, FRAC_PI_2, Point::new(7., 1., 4.)),
            (5, 0.5, 0.5, Point::new(8., -1., 5.)),
        ];
        for (index, u, v, expected) in cases {
            assert_point(
                surfaces[index]
                    .point_at(u, v, super::super::TOLERANCES)
                    .unwrap(),
                expected,
            );
        }
        assert_eq!(surfaces[0].domain(), ((-1e9, 1e9), (-1e9, 1e9)));
        assert_eq!(surfaces[1].domain(), ((0., TAU), (-1e9, 1e9)));
        assert_eq!(surfaces[2].domain(), ((0., TAU), (-1e9, 1e9)));
        assert_eq!(surfaces[3].domain(), ((0., TAU), (-FRAC_PI_2, FRAC_PI_2)));
        assert_eq!(surfaces[4].domain(), ((0., TAU), (0., TAU)));
        assert_eq!(surfaces[5].domain(), ((0., 1.), (0., 1.)));
    }

    #[test]
    fn nested_curve_trims_offsets_keep_independent_reference_directions() {
        let records = [
            format!("8 0 2 9 0.5 0 0 1 2 {LEFT_FRAME} 2"),
            "8 0 2 9 0.5 0 0 1 1 7 -2 5 0 1 0".to_owned(),
        ];
        let (model, _) = read(&document(&records, &[]), super::super::TOLERANCES).unwrap();
        let curves = edge_curves(&model);
        assert_eq!(curves[0].domain(), (0., 2.));
        assert_point(
            curves[0]
                .point_at(FRAC_PI_2, super::super::TOLERANCES)
                .unwrap(),
            Point::new(7., 0.5, 5.),
        );
        assert_point(
            curves[1].point_at(1., super::super::TOLERANCES).unwrap(),
            Point::new(7.5, -1., 5.),
        );
        let Curve::Trimmed(trim) = curves[0] else {
            panic!("expected trim")
        };
        let Curve::Offset(offset) = trim.basis() else {
            panic!("expected offset")
        };
        assert_eq!(offset.reference(), Direction::Z);
    }

    #[test]
    fn nested_surface_trims_offsets_and_sweeps_preserve_independent_axes() {
        let records = [
            format!("10 -1 2 -2 3 11 0.5 1 {LEFT_FRAME}"),
            format!("10 0 {TAU} 0 3 6 0 0 1 8 0 {TAU} 9 0.5 0 0 1 2 {LEFT_FRAME} 2"),
            format!("7 7 -2 5 0 1 0 8 0 2 2 {LEFT_FRAME} 2"),
            "6 0 0 1 1 7 -2 5 0 1 0".to_owned(),
        ];
        let (model, _) = read(&document(&[], &records), super::super::TOLERANCES).unwrap();
        let surfaces: Vec<_> = model
            .geometry()
            .surfaces()
            .map(|(_, surface)| surface)
            .collect();
        assert_eq!(surfaces[0].domain(), ((-1., 2.), (-2., 3.)));
        assert_point(
            surfaces[0]
                .point_at(1., 2., super::super::TOLERANCES)
                .unwrap(),
            Point::new(8., 0., 5.5),
        );
        assert_point(
            surfaces[1]
                .point_at(FRAC_PI_2, 2., super::super::TOLERANCES)
                .unwrap(),
            Point::new(7., 0.5, 7.),
        );
        assert_point(
            surfaces[2]
                .point_at(FRAC_PI_2, FRAC_PI_4, super::super::TOLERANCES)
                .unwrap(),
            Point::new(7., -2. + 2f64.sqrt(), 5. - 2f64.sqrt()),
        );
        assert_point(
            surfaces[3]
                .point_at(1., 2., super::super::TOLERANCES)
                .unwrap(),
            Point::new(7., -1., 7.),
        );
        let SurfaceGeometry::Trimmed(trim) = surfaces[1] else {
            panic!("expected trim")
        };
        let SurfaceGeometry::Extrusion(extrusion) = trim.basis() else {
            panic!("expected extrusion")
        };
        assert_eq!(extrusion.direction(), Direction::Z);
        let SurfaceGeometry::Revolution(revolution) = surfaces[2] else {
            panic!("expected revolution")
        };
        assert_eq!(revolution.axis().location, Point::new(7., -2., 5.));
        assert_eq!(revolution.axis().direction, Direction::Y);
        assert_eq!(surfaces[2].domain(), ((0., TAU), (0., 2.)));
        assert_eq!(surfaces[3].domain(), ((-1e6, 1e6), (0., 1e9)));
    }

    #[test]
    fn reversed_curves_keep_their_parameter_direction_during_reconstruction() {
        let tolerances = super::super::TOLERANCES;
        let frame = Frame::from_axes(
            Point::new(7., -2., 5.),
            Direction::X,
            Direction::Y,
            -Direction::Z,
            tolerances,
        )
        .unwrap();
        let curves: [Curve; 4] = [
            CircleCurve::new(Circle::new(Frame::WORLD, 2., tolerances).unwrap()).into(),
            EllipseCurve::new(Ellipse::new(Frame::WORLD, 3., 1., tolerances).unwrap()).into(),
            ParabolaCurve::over(
                Parabola::new(Frame::WORLD, 2., tolerances).unwrap(),
                -1.,
                3.,
            )
            .unwrap()
            .into(),
            HyperbolaCurve::over(
                Hyperbola::new(Frame::WORLD, 3., 1., tolerances).unwrap(),
                -1.,
                3.,
            )
            .unwrap()
            .into(),
        ];
        for curve in curves {
            let domain = curve.domain();
            let forward = restore_curve(&curve, frame, tolerances).unwrap();
            let reversed = restore_curve(&curve.reversed(), frame, tolerances).unwrap();
            assert_eq!(reversed.domain(), domain);
            let parameter = if matches!(curve, Curve::Circle(_) | Curve::Ellipse(_)) {
                TAU - 0.25
            } else {
                domain.0 + domain.1 - 0.25
            };
            assert_point(
                reversed.point_at(0.25, tolerances).unwrap(),
                forward.point_at(parameter, tolerances).unwrap(),
            );
            let offset = Curve::Offset(Box::new(
                OffsetCurve::new(curve.clone(), 0.5, Direction::Z).unwrap(),
            ));
            let offset_forward = restore_curve(&offset, frame, tolerances).unwrap();
            let offset_reversed = restore_curve(&offset.reversed(), frame, tolerances).unwrap();
            assert_eq!(offset_reversed.domain(), domain);
            assert_point(
                offset_reversed.point_at(0.25, tolerances).unwrap(),
                offset_forward.point_at(parameter, tolerances).unwrap(),
            );
            let trim: Curve = TrimmedCurve::new(curve, 0., 1., tolerances).unwrap().into();
            let restored = restore_curve(&trim.reversed(), frame, tolerances).unwrap();
            assert_eq!(restored.domain(), (0., 1.));
            assert_point(
                restored.point_at(0.25, tolerances).unwrap(),
                forward.point_at(0.75, tolerances).unwrap(),
            );
        }
    }

    #[test]
    fn malformed_authored_y_axes_are_rejected_before_upstream_import() {
        let source = document(&["2 7 -2 5 0 0 -1 1 0 0 1 0 0 2".to_owned()], &[]);
        assert!(read(&source, super::super::TOLERANCES).is_err());
        let source = document(&[format!("2 {LEFT_FRAME} 2")], &[]);
        assert!(
            read(
                &source.replace("Curves 1", "Curves 2"),
                super::super::TOLERANCES
            )
            .is_err()
        );
    }
}
