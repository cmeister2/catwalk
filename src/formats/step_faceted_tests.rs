//! Selected polygon schema, invalid topology and shared-resource regressions.

use super::*;
use crate::formats::step_budget::{MAX_BYTES, MAX_FACES, MAX_WORK};
use std::fmt::Write;

/// Authored open rectangular face with separate reversed occurrence.
const RECTANGLE: &str = "
#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=CARTESIAN_POINT('',(4.,0.,0.));
#3=CARTESIAN_POINT('',(4.,3.,0.));
#4=CARTESIAN_POINT('',(0.,3.,0.));
#5=AXIS2_PLACEMENT_3D('',#1,$,$);
#6=PLANE('',#5);
#7=POLY_LOOP('',(#1,#2,#3,#4));
#8=FACE_OUTER_BOUND('',#7,.T.);
#9=FACE_SURFACE('',(#8),#6,.T.);
#10=OPEN_SHELL('',(#9));
#11=ORIENTED_FACE('',*,#9,.F.);
#12=OPEN_SHELL('',(#11));
";

/// Parse authored record syntax through the same public AST adapter as production.
fn entities(body: &str) -> Result<Vec<EntityInstance>> {
    let text = format!(
        "ISO-10303-21;HEADER;FILE_DESCRIPTION(('polygon regression'),'2;1');ENDSEC;DATA;{body}ENDSEC;END-ISO-10303-21;"
    );
    Ok(crate::formats::step_parameters::parse(&text)?
        .data
        .into_iter()
        .flat_map(|section| section.entities)
        .collect())
}

/// Retrieve the identity of either Part 21 entity mapping.
fn identity(entity: &EntityInstance) -> u64 {
    match entity {
        EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
    }
}

/// Index authored records without importing or interpreting unused geometry.
fn index(entities: &[EntityInstance]) -> HashMap<u64, &EntityInstance> {
    entities
        .iter()
        .map(|entity| (identity(entity), entity))
        .collect()
}

/// Decode one required authored polygon scene through a fresh shared budget.
fn decode(entities: &[EntityInstance], id: u64) -> Result<Scene> {
    Decoder::new(&index(entities))
        .decode(id, &mut Budget::default())?
        .ok_or_else(|| "authored polygon shell was unexpectedly delegated".into())
}

/// Mutate an authored simple-mapping attribute while preserving the surrounding AST.
fn change(
    entities: &mut [EntityInstance],
    id: u64,
    attribute: usize,
    value: Parameter,
) -> Result<()> {
    let entity = entities
        .iter_mut()
        .find(|entity| identity(entity) == id)
        .ok_or("authored mutation lacks its entity")?;
    let EntityInstance::Simple { record, .. } = entity else {
        return Err("authored mutation requires a simple mapping".into());
    };
    let Parameter::List(values) = &mut record.parameter else {
        return Err("authored mutation requires an aggregate".into());
    };
    *values
        .get_mut(attribute)
        .ok_or("authored mutation lacks its attribute")? = value;
    Ok(())
}

/// Obtain an import error without relying on the successful scene's debug representation.
fn failure(entities: &[EntityInstance], id: u64) -> Result<String> {
    match decode(entities, id) {
        Err(error) => Ok(error.to_string()),
        Ok(_) => Err("invalid authored polygon unexpectedly decoded".into()),
    }
}

/// Provide the unit supporting frame for independent projected-ring controls.
fn xy_plane() -> Plane {
    Plane {
        origin: Vec3::new(0., 0., 0.),
        x: Vec3::new(1., 0., 0.),
        y: Vec3::new(0., 1., 0.),
        normal: Vec3::new(0., 0., 1.),
    }
}

/// Author XY ring positions directly, independent of STEP reference decoding.
fn ring(points: &[[f64; 2]]) -> Vec<Vec3> {
    points.iter().map(|&[x, y]| Vec3::new(x, y, 0.)).collect()
}

#[test]
fn selected_faces_cache_geometry_and_preserve_occurrence_normals() -> Result<()> {
    let records = entities(RECTANGLE)?;
    let indexed = index(&records);
    let mut decoder = Decoder::new(&indexed);
    let mut budget = Budget::default();
    budget.faces(MAX_FACES - 1)?;
    let forward = decoder
        .decode(10, &mut budget)?
        .ok_or("missing forward face")?;
    let reverse = decoder
        .decode(12, &mut budget)?
        .ok_or("missing reversed face")?;
    assert_eq!(decoder.faces.len(), 1);
    assert_eq!(decoder.points.len(), 4);
    assert_eq!(forward.triangles.len(), 2);
    assert_eq!(reverse.triangles.len(), 2);
    let area = forward
        .triangles
        .iter()
        .map(|triangle| {
            (triangle.vertices[1] - triangle.vertices[0])
                .cross(triangle.vertices[2] - triangle.vertices[0])
                .length()
                / 2.
        })
        .sum::<f64>();
    assert!((area - 12.).abs() < 1e-12);
    for (scene, sign) in [(&forward, 1.), (&reverse, -1.)] {
        for (id, triangle) in scene.triangles.iter().enumerate() {
            let cross = (triangle.vertices[1] - triangle.vertices[0])
                .cross(triangle.vertices[2] - triangle.vertices[0]);
            assert!(cross.z * sign > 0.);
            let normals = scene.appearances[&id]
                .normals
                .ok_or("missing authored plane normals")?;
            for normal in normals {
                assert!((normal.z - sign).abs() < 1e-12);
                assert!(normal.x.abs() + normal.y.abs() < 1e-12);
            }
        }
    }
    // Reusing the face is permitted, while each emitted scene remains budgeted separately.
    budget
        .storage(MAX_BYTES, 1)
        .expect_err("retained scenes must already consume storage");
    Ok(())
}

#[test]
fn external_complex_layouts_flatten_only_verified_local_attributes() -> Result<()> {
    let records = entities("
#1=(CARTESIAN_POINT((0.,0.,0.)) GEOMETRIC_REPRESENTATION_ITEM() POINT() REPRESENTATION_ITEM('origin'));
#2=(DIRECTION((0.,0.,1.)) GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM('axis'));
#3=(AXIS2_PLACEMENT_3D(#2,$) GEOMETRIC_REPRESENTATION_ITEM() PLACEMENT(#1) REPRESENTATION_ITEM('frame'));
#4=(ELEMENTARY_SURFACE(#3) GEOMETRIC_REPRESENTATION_ITEM() PLANE() REPRESENTATION_ITEM('plane') SURFACE());
#5=(GEOMETRIC_REPRESENTATION_ITEM() LOOP() POLY_LOOP((#1,#6,#7)) REPRESENTATION_ITEM('loop') TOPOLOGICAL_REPRESENTATION_ITEM());
#6=CARTESIAN_POINT('',(2.,0.,0.));
#7=CARTESIAN_POINT('',(0.,3.,0.));
#8=(FACE_BOUND(#5,.T.) FACE_OUTER_BOUND() REPRESENTATION_ITEM('bound') TOPOLOGICAL_REPRESENTATION_ITEM());
#9=(FACE((#8)) FACE_SURFACE(#4,.T.) GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM('face') TOPOLOGICAL_REPRESENTATION_ITEM());
#10=(CONNECTED_FACE_SET((#9)) OPEN_SHELL() REPRESENTATION_ITEM('shell') TOPOLOGICAL_REPRESENTATION_ITEM());
#11=(FACE(*) ORIENTED_FACE(#9,.F.) REPRESENTATION_ITEM('reversed') TOPOLOGICAL_REPRESENTATION_ITEM());
#12=(CONNECTED_FACE_SET((#11)) OPEN_SHELL() REPRESENTATION_ITEM('reversed shell') TOPOLOGICAL_REPRESENTATION_ITEM());
#13=(CLOSED_SHELL() CONNECTED_FACE_SET((#9)) REPRESENTATION_ITEM('closed') TOPOLOGICAL_REPRESENTATION_ITEM());
#14=(FACETED_BREP() GEOMETRIC_REPRESENTATION_ITEM() MANIFOLD_SOLID_BREP(#13) REPRESENTATION_ITEM('solid') SOLID_MODEL());
")?;
    let indexed = index(&records);
    assert_eq!(solid_outer(indexed[&14])?, Some(13));
    assert!(owns_shell(&indexed, 10, &mut Budget::default())?);
    assert!(owns_shell(&indexed, 12, &mut Budget::default())?);
    let scene = decode(&records, 10)?;
    assert_eq!(scene.triangles.len(), 1);
    assert!(
        (scene.triangles[0].vertices[1] - scene.triangles[0].vertices[0])
            .cross(scene.triangles[0].vertices[2] - scene.triangles[0].vertices[0])
            .z
            > 0.
    );
    let reverse = decode(&records, 12)?;
    assert!(
        reverse.appearances[&0]
            .normals
            .ok_or("missing reversed normal")?[0]
            .z
            < 0.
    );
    Ok(())
}

#[test]
fn malformed_complex_inheritance_and_void_combinations_fail_explicitly() -> Result<()> {
    for body in [
        "#1=(FACETED_BREP() MANIFOLD_SOLID_BREP(#2) REPRESENTATION_ITEM('') REPRESENTATION_ITEM('duplicate') SOLID_MODEL());",
        "#1=(FACETED_BREP() MANIFOLD_SOLID_BREP(#2,#3) REPRESENTATION_ITEM('') SOLID_MODEL());",
        "#1=(FACETED_BREP() MANIFOLD_SOLID_BREP(#2) REPRESENTATION_ITEM('') SOLID_MODEL(1));",
        "#1=(FACETED_BREP() MANIFOLD_SOLID_BREP(#2) REPRESENTATION_ITEM('') UNKNOWN_SOLID());",
        "#1=(BREP_WITH_VOIDS((#3)) FACETED_BREP() MANIFOLD_SOLID_BREP(#2) REPRESENTATION_ITEM('') SOLID_MODEL());",
        "#1=FACETED_BREP_WITH_VOIDS('',#2,(#3));",
        "#1=FACETED_BREP('',$);",
    ] {
        let records = entities(body)?;
        assert!(solid_outer(&records[0]).is_err(), "{body}");
    }
    let ordinary = entities("#1=MANIFOLD_SOLID_BREP('',#2);")?;
    assert_eq!(solid_outer(&ordinary[0])?, None);
    Ok(())
}

#[test]
fn polygon_references_are_unique_cartesian_positions_and_boolean_orientations() -> Result<()> {
    let mut records = entities(RECTANGLE)?;
    change(
        &mut records,
        7,
        1,
        Parameter::List(vec![
            Parameter::Ref(Name::Entity(1)),
            Parameter::Ref(Name::Entity(2)),
            Parameter::Ref(Name::Entity(3)),
            Parameter::Ref(Name::Entity(1)),
        ]),
    )?;
    assert!(failure(&records, 10)?.contains("repeats a Cartesian-point"));
    let records = entities(&RECTANGLE.replace(
        "#2=CARTESIAN_POINT('',(4.,0.,0.));",
        "#2=VERTEX_POINT('',#1);",
    ))?;
    assert!(failure(&records, 10)?.contains("unsupported schema type"));
    let mut records = entities(RECTANGLE)?;
    change(&mut records, 8, 2, Parameter::Enumeration("U".to_owned()))?;
    assert!(failure(&records, 10)?.contains("Boolean"));
    let mut records = entities(RECTANGLE)?;
    change(&mut records, 9, 3, Parameter::Enumeration("U".to_owned()))?;
    assert!(failure(&records, 10)?.contains("Boolean"));
    Ok(())
}

#[test]
fn plane_coordinates_directions_and_derived_bounds_are_typed() -> Result<()> {
    let mut records = entities(RECTANGLE)?;
    change(
        &mut records,
        1,
        1,
        Parameter::List(vec![Parameter::Real(0.), Parameter::Real(0.)]),
    )?;
    assert!(failure(&records, 10)?.contains("three components"));
    let mut records = entities(RECTANGLE)?;
    change(
        &mut records,
        2,
        1,
        Parameter::List(vec![
            Parameter::Real(f64::INFINITY),
            Parameter::Real(0.),
            Parameter::Real(0.),
        ]),
    )?;
    assert!(failure(&records, 10)?.contains("non-finite"));
    for direction in ["(0.,0.,0.)", "(0.,0.,1.)"] {
        let text = RECTANGLE.replace(
            "#5=AXIS2_PLACEMENT_3D('',#1,$,$);",
            "#5=AXIS2_PLACEMENT_3D('',#1,#13,#14);",
        );
        let records = entities(&format!(
            "{text}#13=DIRECTION('',(0.,0.,1.));#14=DIRECTION('',{direction});"
        ))?;
        assert!(failure(&records, 10)?.contains("direction is zero or non-finite"));
    }
    let mut records = entities(RECTANGLE)?;
    change(&mut records, 11, 1, Parameter::NotProvided)?;
    assert!(failure(&records, 12)?.contains("derive its bounds"));
    Ok(())
}

#[test]
fn nonplanar_self_touching_backtracking_and_wrong_winding_rings_fail() -> Result<()> {
    let plane = xy_plane();
    let normal = plane.normal;
    for points in [
        vec![[0., 0.], [4., 3.], [4., 0.], [0., 3.]],
        vec![[0., 0.], [4., 0.], [2., 0.], [4., 3.], [0., 3.]],
        vec![[0., 0.], [4., 0.], [4., 3.], [0., 3.], [4., 0.]],
        vec![[0., 0.], [0., 3.], [4., 3.], [4., 0.]],
        vec![[0., 0.], [4., 0.], [4., 0.], [0., 3.]],
    ] {
        assert!(triangulate(&[ring(&points)], plane, normal, &mut Budget::default()).is_err());
    }
    let mut nonplanar = ring(&[[0., 0.], [4., 0.], [4., 3.], [0., 3.]]);
    nonplanar[2].z = 0.01;
    let error = triangulate(&[nonplanar], plane, normal, &mut Budget::default())
        .err()
        .ok_or("nonplanar ring unexpectedly triangulated")?;
    assert!(error.to_string().contains("supporting plane"));
    Ok(())
}

#[test]
fn outside_touching_overlapping_and_nested_holes_are_rejected() {
    let outer = ring(&[[0., 0.], [10., 0.], [10., 10.], [0., 10.]]);
    let inner = ring(&[[2., 2.], [2., 8.], [8., 8.], [8., 2.]]);
    for bounds in [
        vec![
            outer.clone(),
            ring(&[[11., 2.], [11., 3.], [12., 3.], [12., 2.]]),
        ],
        vec![
            outer.clone(),
            ring(&[[0., 2.], [0., 3.], [2., 3.], [2., 2.]]),
        ],
        vec![
            outer.clone(),
            inner.clone(),
            ring(&[[7., 3.], [7., 5.], [9., 5.], [9., 3.]]),
        ],
        vec![
            outer.clone(),
            inner.clone(),
            ring(&[[3., 3.], [3., 4.], [4., 4.], [4., 3.]]),
        ],
        vec![
            outer,
            inner,
            ring(&[[8., 3.], [8., 4.], [9., 4.], [9., 3.]]),
        ],
    ] {
        assert!(
            triangulate(
                &bounds,
                xy_plane(),
                xy_plane().normal,
                &mut Budget::default()
            )
            .is_err()
        );
    }
}

#[test]
fn required_mixed_topology_and_missing_outer_bounds_are_not_silently_skipped() -> Result<()> {
    let text = RECTANGLE.replace("#10=OPEN_SHELL('',(#9));", "#10=OPEN_SHELL('',(#9,#13));");
    let records = entities(&format!(
        "{text}#13=FACE('',(#14));#14=FACE_BOUND('',#15,.T.);#15=EDGE_LOOP('',());"
    ))?;
    assert!(owns_shell(&index(&records), 10, &mut Budget::default())?);
    assert!(failure(&records, 10)?.contains("unsupported required face geometry"));
    let records = entities(&RECTANGLE.replace("FACE_OUTER_BOUND", "FACE_BOUND"))?;
    assert!(failure(&records, 10)?.contains("explicit exterior bound"));
    let records = entities(&RECTANGLE.replace("#9=FACE_SURFACE", "#9=ADVANCED_FACE"))?;
    assert!(owns_shell(&index(&records), 10, &mut Budget::default())?);
    assert!(failure(&records, 10)?.contains("unsupported required face geometry"));
    Ok(())
}

#[test]
fn unused_malformed_geometry_stays_lazy_but_selected_wrappers_are_checked() -> Result<()> {
    let records = entities(&format!(
        "{RECTANGLE}#20=FACE_SURFACE('',(),#404,.U.);#21=CLOSED_SHELL('',(#20));#22=FACETED_BREP('',#21);"
    ))?;
    let indexed = index(&records);
    assert!(!owns_shell(&indexed, 21, &mut Budget::default())?);
    assert_eq!(decode(&records, 10)?.triangles.len(), 2);
    assert!(failure(&records, 22)?.contains("empty or excessive bound set"));
    let text = RECTANGLE
        .replace(
            "#11=ORIENTED_FACE('',*,#9,.F.);",
            "#11=ORIENTED_FACE('',*,#11,.F.);",
        )
        .replace("#12=OPEN_SHELL('',(#11));", "#12=CLOSED_SHELL('',(#11));");
    let records = entities(&format!("{text}#13=FACETED_BREP('',#12);"))?;
    assert!(failure(&records, 13)?.contains("reference cycle"));
    Ok(())
}

#[test]
fn polygon_limits_and_shared_work_are_checked_before_triangulation() -> Result<()> {
    let mut oversized = String::new();
    for id in 1..=MAX_FACE_VERTICES + 1 {
        if id != 1 {
            oversized.push(',');
        }
        write!(oversized, "#{id}")?;
    }
    let records = entities(&RECTANGLE.replace("(#1,#2,#3,#4)", &format!("({oversized})")))?;
    assert!(failure(&records, 10)?.contains("3..4096"));
    let records = entities(RECTANGLE)?;
    let indexed = index(&records);
    let mut budget = Budget::default();
    budget.work(MAX_WORK)?;
    let error = Decoder::new(&indexed)
        .decode(10, &mut budget)
        .err()
        .ok_or("exhausted shared work budget unexpectedly decoded")?;
    assert!(error.to_string().contains("processing work"));
    let mut budget = Budget::default();
    budget.storage(MAX_BYTES, 1)?;
    let error = Decoder::new(&indexed)
        .decode(10, &mut budget)
        .err()
        .ok_or("exhausted shared storage budget unexpectedly decoded")?;
    assert!(error.to_string().contains("512 MiB"));
    Ok(())
}

#[test]
fn repeated_shell_classification_obeys_cumulative_work_and_storage_limits() -> Result<()> {
    let records = entities(RECTANGLE)?;
    let indexed = index(&records);
    let mut budget = Budget::default();
    // A small traversal succeeds; repeated shared topology must reuse the same counter.
    budget.work(MAX_WORK - 15)?;
    assert!(owns_shell(&indexed, 10, &mut budget)?);
    let error = owns_shell(&indexed, 10, &mut budget)
        .err()
        .ok_or("repeated shared classification unexpectedly reset its work budget")?;
    assert!(error.to_string().contains("processing work"));
    let mut budget = Budget::default();
    budget.storage(MAX_BYTES - 1, 1)?;
    let error = owns_shell(&indexed, 10, &mut budget)
        .err()
        .ok_or("classifier pending references exceeded their storage preflight")?;
    assert!(error.to_string().contains("512 MiB"));
    Ok(())
}
