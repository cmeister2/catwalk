//! Self-contained analytic chart controls need no packaged fixture files.

use super::*;
use cadmpeg_ir::{
    eval::{admission::EvaluationAdmission, decode::surface_point_solved},
    geometry::{
        Surface,
        pcurve::{Pcurve, PcurveMetadata, PcurveNurbs},
    },
    ids::{CoedgeId, EdgeId, FaceId, LoopId, PcurveId, ShellId, SurfaceId, VertexId},
    math::{Point2, Point3, Vector3},
    topology::{
        AnchoredVertexUse, Coedge, Face, FaceLoops, Loop, LoopBoundary, LoopRing, PcurveUse,
    },
};

/// The native writer is independent of these mathematical chart identities.
fn assert_same_points(source: &SolvedSurfaceGeometry) {
    let (target, change) = canonical_surface(source).unwrap();
    let change = change.expect("signed chart must change");
    for u in [-2.5, -0.7, 0., 0.4, 2.8] {
        for v in [-1.25, -0.2, 0., 0.6, 1.4] {
            let before = surface_point_solved(EvaluationAdmission::Standard, source, u, v)
                .unwrap()
                .get();
            let after = surface_point_solved(
                EvaluationAdmission::Standard,
                &target,
                change.scale[0] * u + change.offset[0],
                change.scale[1] * v + change.offset[1],
            )
            .unwrap()
            .get();
            assert!((before.x - after.x).abs() < 1e-11);
            assert!((before.y - after.y).abs() < 1e-11);
            assert!((before.z - after.z).abs() < 1e-11);
        }
    }
}

#[test]
fn negative_sphere_is_identical_under_the_reflected_chart() {
    let source = SolvedSurfaceGeometry::Sphere(
        SphereSurface::try_new(
            Point3::new(3., -7., 9.),
            Vector3::new(0., 1., 0.),
            Vector3::new(1., 0., 0.),
            -4.,
        )
        .unwrap(),
    );
    assert_same_points(&source);
    assert!(canonical_surface(&source).unwrap().1.unwrap().reversed);
}

#[test]
fn negative_torus_tube_is_identical_under_the_shifted_chart() {
    let source = SolvedSurfaceGeometry::Torus(
        TorusSurface::try_new(
            Point3::new(-3., 2., 5.),
            Vector3::new(1., 0., 0.),
            Vector3::new(0., 0., 1.),
            8.,
            -2.,
        )
        .unwrap(),
    );
    assert_same_points(&source);
    assert!(!canonical_surface(&source).unwrap().1.unwrap().reversed);
}

#[test]
fn signed_spindle_tori_keep_the_geometry_refusal() {
    let source = SolvedSurfaceGeometry::Torus(
        TorusSurface::try_new(
            Point3::new(0., 0., 0.),
            Vector3::new(0., 0., 1.),
            Vector3::new(1., 0., 0.),
            2.,
            -3.,
        )
        .unwrap(),
    );
    assert!(canonical_surface(&source).is_err());
}

#[test]
fn rational_pcurve_chart_changes_preserve_knots_weights_and_parameter_order() {
    let arena = DecodeArena::new();
    let policy = DecodePolicy::service();
    let (ctx, _) = DecodeContext::from_root_bytes(&[], &arena, &policy).unwrap();
    let nurbs = PcurveNurbs::from_lanes(
        &ctx,
        2,
        vec![0., 0., 0., 1., 1., 1.],
        vec![
            Point2::new(1., 2.),
            Point2::new(3., 4.),
            Point2::new(5., 6.),
        ],
        Some(vec![1., 0.5, 2.]),
        false,
    )
    .unwrap()
    .unwrap();
    let knots = nurbs.knots().clone();
    let weights = nurbs.weights();
    let mut geometry = PcurveGeometry::Nurbs { nurbs };
    remap_pcurve(
        &mut geometry,
        ChartChange {
            scale: [-1., 1.],
            offset: [0., 0.],
            reversed: true,
        },
        &ctx,
    )
    .unwrap();
    let PcurveGeometry::Nurbs { nurbs } = geometry else {
        panic!("preserve NURBS carrier")
    };
    assert_eq!(nurbs.knots(), &knots);
    assert_eq!(nurbs.weights(), weights);
    assert_eq!(
        nurbs
            .control_points()
            .into_iter()
            .map(FinitePoint2::get)
            .collect::<Vec<_>>(),
        vec![
            Point2::new(-1., 2.),
            Point2::new(-3., 4.),
            Point2::new(-5., 6.)
        ]
    );
}

/// An analytic chart unit control, independent of any saved native thumbnail.
fn pole_chart_ir(anchored: bool, ctx: &DecodeContext<'_>) -> CadIr {
    let surface = SurfaceId::mint("test:chart:surface#sphere").unwrap();
    let face = FaceId::mint("test:chart:face#sphere").unwrap();
    let loop_id = LoopId::mint("test:chart:loop#pole").unwrap();
    let vertex = VertexId::mint("test:chart:vertex#pole").unwrap();
    let pcurve = PcurveId::mint("test:chart:pcurve#pole").unwrap();
    let mut ir = CadIr::empty();
    ir.model.surfaces.push(Surface {
        id: surface.clone(),
        geometry: SurfaceGeometry::Solved(SolvedSurfaceGeometry::Sphere(
            SphereSurface::try_new(
                Point3::new(0., 0., 0.),
                Vector3::new(0., 0., 1.),
                Vector3::new(1., 0., 0.),
                -4.,
            )
            .unwrap(),
        )),
        source_object: None,
    });
    ir.model.faces.push(Face {
        id: face.clone(),
        shell: ShellId::mint("test:chart:shell#sphere").unwrap(),
        surface,
        sense: Sense::Forward,
        loops: FaceLoops::Unspecified {
            loops: vec![loop_id.clone()],
        },
        name: None,
        color: None,
        tolerance: None,
    });
    let pcurve_use = PcurveUse {
        pcurve: pcurve.clone(),
        isoparametric: Some(true),
        parameter_range: None,
    };
    let boundary = if anchored {
        let coedge = CoedgeId::mint("test:chart:coedge#pole").unwrap();
        ir.model.coedges.push(Coedge {
            id: coedge.clone(),
            owner_loop: loop_id.clone(),
            edge: EdgeId::mint("test:chart:edge#pole").unwrap(),
            radial_next: coedge.clone(),
            sense: Sense::Forward,
            pcurves: Vec::new(),
            use_curve: None,
        });
        LoopBoundary::Ring(
            LoopRing::new(
                ctx,
                vec![coedge.clone()],
                vec![AnchoredVertexUse {
                    vertex,
                    after: coedge,
                    pcurves: vec![pcurve_use],
                }],
            )
            .unwrap()
            .unwrap(),
        )
    } else {
        LoopBoundary::Vertex {
            vertex,
            pcurves: vec![pcurve_use],
        }
    };
    ir.model.loops.push(Loop {
        id: loop_id,
        face,
        boundary,
    });
    ir.model.pcurves.push(Pcurve {
        id: pcurve,
        geometry: PcurveGeometry::Line(
            LinePcurve::try_new(
                Point2::new(0.7, std::f64::consts::FRAC_PI_2),
                Point2::new(1., 0.),
            )
            .unwrap(),
        ),
        metadata: PcurveMetadata::default(),
    });
    ir
}

#[test]
fn singular_and_anchored_pole_pcurves_follow_the_surface_chart() {
    let arena = DecodeArena::new();
    let policy = DecodePolicy::service();
    let (ctx, _) = DecodeContext::from_root_bytes(&[], &arena, &policy).unwrap();
    for anchored in [false, true] {
        let mut ir = pole_chart_ir(anchored, &ctx);
        let boundary = ir.model.loops[0].clone();
        let metadata = ir.model.pcurves[0].metadata.clone();
        canonicalize_signed_surfaces(&mut ir, &ctx).unwrap();
        let PcurveGeometry::Line(line) = &ir.model.pcurves[0].geometry else {
            panic!("retain parameter-space line carrier")
        };
        assert_eq!(
            line.origin().get(),
            Point2::new(-0.7, std::f64::consts::FRAC_PI_2)
        );
        assert_eq!(*line.direction().as_raw(), Point2::new(-1., 0.));
        assert_eq!(ir.model.faces[0].sense, Sense::Reversed);
        assert_eq!(ir.model.loops[0], boundary);
        assert_eq!(ir.model.pcurves[0].metadata, metadata);
    }
}

#[test]
fn pole_pcurves_shared_by_changed_and_unchanged_charts_are_refused() {
    let arena = DecodeArena::new();
    let policy = DecodePolicy::service();
    let (ctx, _) = DecodeContext::from_root_bytes(&[], &arena, &policy).unwrap();
    let mut ir = pole_chart_ir(false, &ctx);
    let mut surface = ir.model.surfaces[0].clone();
    surface.id = SurfaceId::mint("test:chart:surface#unchanged").unwrap();
    surface.geometry = SurfaceGeometry::Solved(SolvedSurfaceGeometry::Sphere(
        SphereSurface::try_new(
            Point3::new(0., 0., 0.),
            Vector3::new(0., 0., 1.),
            Vector3::new(1., 0., 0.),
            4.,
        )
        .unwrap(),
    ));
    let mut face = ir.model.faces[0].clone();
    face.id = FaceId::mint("test:chart:face#unchanged").unwrap();
    face.surface = surface.id.clone();
    let mut loop_ = ir.model.loops[0].clone();
    loop_.id = LoopId::mint("test:chart:loop#unchanged").unwrap();
    loop_.face = face.id.clone();
    face.loops = FaceLoops::Unspecified {
        loops: vec![loop_.id.clone()],
    };
    ir.model.surfaces.push(surface);
    ir.model.faces.push(face);
    ir.model.loops.push(loop_);
    let error = canonicalize_signed_surfaces(&mut ir, &ctx)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("shared by incompatible signed surface charts"),
        "{error}"
    );
}

#[test]
fn native_signed_torus_normal_correction_is_separate_from_its_parameter_chart() {
    let arena = DecodeArena::new();
    let policy = DecodePolicy::service();
    let (ctx, _) = DecodeContext::from_root_bytes(&[], &arena, &policy).unwrap();
    let mut source = pole_chart_ir(false, &ctx);
    source.model.surfaces[0].geometry = SurfaceGeometry::Solved(SolvedSurfaceGeometry::Torus(
        TorusSurface::try_new(
            Point3::new(0., 0., 8.),
            Vector3::new(0., 0., -1.),
            Vector3::new(-1., 0., 0.),
            5.5,
            -2.,
        )
        .unwrap(),
    ));
    let mut preview = source.clone();
    canonicalize_signed_surfaces(&mut preview, &ctx).unwrap();
    assert_eq!(preview.model.faces[0].sense, Sense::Forward);
    let mut generic = preview.clone();
    apply_surface_normal_convention(&source, &mut generic, SurfaceNormalConvention::Parametric);
    assert_eq!(generic.model.faces, preview.model.faces);

    let geometry = preview.model.surfaces[0].geometry.solved().unwrap();
    let partials = cadmpeg_ir::eval::surface_partials_solved(
        EvaluationAdmission::Standard,
        geometry,
        0.,
        std::f64::consts::FRAC_PI_2,
    )
    .unwrap()
    .into_raw();
    assert!((partials.point.z - 6.).abs() < 1e-10);
    // At the floor circle, the positive carrier's cross-product normal points
    // downward. Native ASM's signed carrier convention requires the +Z normal.
    let parameter_normal = partials.du.cross(partials.dv);
    assert!(parameter_normal.z < 0.);
    apply_surface_normal_convention(&source, &mut preview, SurfaceNormalConvention::NativeAsm);
    assert_eq!(preview.model.faces[0].sense, Sense::Reversed);
    assert!(-parameter_normal.z > 0.);
    assert_eq!(preview.model.surfaces, generic.model.surfaces);
    assert_eq!(preview.model.pcurves, generic.model.pcurves);
    assert_eq!(preview.model.loops, generic.model.loops);

    let mut positive = source.clone();
    positive.model.surfaces = generic.model.surfaces.clone();
    apply_surface_normal_convention(&positive, &mut generic, SurfaceNormalConvention::NativeAsm);
    assert_eq!(generic.model.faces[0].sense, Sense::Forward);

    // The neutral API's solved() also selects a preferred procedural cache.
    // Native carrier placement wrappers must retain the same sign convention.
    let signed = source.model.surfaces[0].geometry.solved().unwrap().clone();
    source.model.surfaces[0].geometry = SurfaceGeometry::Procedural {
        construction: cadmpeg_ir::ids::ProceduralSurfaceId::mint(
            "test:chart:surface-procedure#cached",
        )
        .unwrap(),
        cache: Some(SolvedSurfaceGeometry::Transformed(
            PlacedSurface::try_new(
                Box::new(signed),
                cadmpeg_ir::transform::Transform::identity(),
            )
            .unwrap(),
        )),
    };
    apply_surface_normal_convention(&source, &mut generic, SurfaceNormalConvention::NativeAsm);
    assert_eq!(generic.model.faces[0].sense, Sense::Reversed);
}
