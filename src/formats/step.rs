//! STEP Part 21 boundary-representation tessellation through the Rust Truck kernel.
use crate::Result;
use crate::scene::{Scene, Triangle, Vec3};
use ruststep::ast::{EntityInstance, Name, Parameter, Record};
use ruststep::tables::EntityTable;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use truck_meshalgo::prelude::*;
use truck_stepio::r#in::{
    FaceAny, ShellHolder, SphericalSurfaceHolder, Table, ToroidalSurfaceHolder, VertexPointHolder,
    ruststep,
};

const MAX_ENTITIES: usize = 250_000;
const MAX_FACES: usize = 100_000;
const MAX_TRIANGLES: usize = 5_000_000;
const MAX_REFERENCE_DEPTH: usize = 64;

pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let text = std::str::from_utf8(&bytes)?.trim_start_matches('\u{feff}');
    // Truck exposes infallible geometry constructors; convert their panics to importer errors.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse(text)))
        .map_err(|_| "STEP geometry could not be tessellated by the Rust CAD kernel")?
}

fn records(entity: &EntityInstance) -> (u64, &[Record]) {
    match entity {
        EntityInstance::Simple { id, record } => (*id, std::slice::from_ref(record)),
        EntityInstance::Complex { id, subsuper } => (*id, &subsuper.0),
    }
}

/// Cap nesting before ruststep recursively parses parameter lists.
fn check_nesting(text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let (mut index, mut depth) = (0usize, 0usize);
    while index < bytes.len() {
        match bytes[index] {
            b'\'' => {
                index += 1;
                loop {
                    let Some(&byte) = bytes.get(index) else {
                        return Err("unterminated STEP string".into());
                    };
                    index += 1;
                    if byte == b'\'' {
                        if bytes.get(index) == Some(&b'\'') {
                            index += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while bytes.get(index..index + 2) != Some(b"*/") {
                    if index >= bytes.len() {
                        return Err("unterminated STEP comment".into());
                    }
                    index += 1;
                }
                index += 2;
            }
            b'(' => {
                depth += 1;
                if depth > MAX_REFERENCE_DEPTH {
                    return Err("STEP parameter nesting exceeds importer limit".into());
                }
                index += 1;
            }
            b')' => {
                depth = depth.checked_sub(1).ok_or("unbalanced STEP parentheses")?;
                index += 1;
            }
            _ => index += 1,
        }
    }
    if depth != 0 {
        return Err("unbalanced STEP parentheses".into());
    }
    Ok(())
}

fn collect_references(parameter: &Parameter, references: &mut Vec<u64>) -> Result<()> {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => references.push(*id),
        Parameter::Ref(_) => return Err("STEP named/value references are not implemented".into()),
        Parameter::Real(value) if !value.is_finite() => {
            return Err("STEP numeric value is not finite".into());
        }
        Parameter::List(values) => {
            for value in values {
                collect_references(value, references)?;
            }
        }
        Parameter::Typed { parameter, .. } => collect_references(parameter, references)?,
        _ => {}
    }
    Ok(())
}

/// Reject dependency cycles before Truck's recursive owned-entity conversion.
fn check_graph(
    id: u64,
    graph: &HashMap<u64, Vec<u64>>,
    visiting: &mut HashSet<u64>,
    done: &mut HashMap<u64, usize>,
    depth: usize,
) -> Result<usize> {
    if let Some(&remaining_depth) = done.get(&id) {
        if depth + remaining_depth > MAX_REFERENCE_DEPTH {
            return Err("STEP geometry reference depth exceeds importer limit".into());
        }
        return Ok(remaining_depth);
    }
    if depth > MAX_REFERENCE_DEPTH {
        return Err("STEP geometry reference depth exceeds importer limit".into());
    }
    if !visiting.insert(id) {
        return Err("STEP geometry contains cyclic entity references".into());
    }
    let references = graph
        .get(&id)
        .ok_or_else(|| format!("STEP references missing entity #{id}"))?;
    let mut remaining_depth = 0usize;
    for &next in references {
        remaining_depth =
            remaining_depth.max(check_graph(next, graph, visiting, done, depth + 1)? + 1);
    }
    visiting.remove(&id);
    done.insert(id, remaining_depth);
    Ok(remaining_depth)
}

fn list(parameter: &Parameter) -> Result<&[Parameter]> {
    match parameter {
        Parameter::List(values) => Ok(values),
        _ => Err("invalid STEP entity parameters".into()),
    }
}

fn reference(parameter: &Parameter) -> Result<u64> {
    match parameter {
        Parameter::Ref(Name::Entity(id)) => Ok(*id),
        _ => Err("STEP entity must reference a geometry record".into()),
    }
}

fn simple_record<'a>(entities: &HashMap<u64, &'a EntityInstance>, id: u64) -> Result<&'a Record> {
    match entities.get(&id) {
        Some(EntityInstance::Simple { record, .. }) => Ok(record),
        _ => Err(format!("STEP entity #{id} is not a supported simple record").into()),
    }
}

fn positive_radius(parameter: &Parameter) -> Result<f64> {
    let value = match parameter {
        Parameter::Real(value) => *value,
        Parameter::Integer(value) => *value as f64,
        _ => return Err("invalid STEP analytic radius".into()),
    };
    if !value.is_finite() || value <= TOLERANCE {
        return Err("STEP analytic radius must be finite and exceed kernel tolerance".into());
    }
    Ok(value)
}

fn finite_parameter_bound(value: std::ops::Bound<f64>) -> Result<f64> {
    match value {
        std::ops::Bound::Included(value) | std::ops::Bound::Excluded(value)
            if value.is_finite() =>
        {
            Ok(value)
        }
        _ => Err("STEP geometry has unbounded or non-finite parameters".into()),
    }
}

fn append_closed_surface<S: ParametricSurface3D + SearchParameter<D2, Point = Point3>>(
    surface: &S,
    invert: bool,
    vertex: Point3,
    scene: &mut Scene,
) -> Result<()> {
    if surface.search_parameter(vertex, None, 100).is_none() {
        return Err("STEP vertex loop point does not lie on its analytic surface".into());
    }
    let ((u0, u1), (v0, v1)) = surface.parameter_range();
    let (u0, u1, v0, v1) = (
        finite_parameter_bound(u0)?,
        finite_parameter_bound(u1)?,
        finite_parameter_bound(v0)?,
        finite_parameter_bound(v1)?,
    );
    for i in 0..64 {
        for j in 0..64 {
            let u = |i: usize| u0 + (u1 - u0) * i as f64 / 64.0;
            let v = |j: usize| v0 + (v1 - v0) * j as f64 / 64.0;
            let points = [
                surface.subs(u(i), v(j)),
                surface.subs(u(i + 1), v(j)),
                surface.subs(u(i + 1), v(j + 1)),
                surface.subs(u(i), v(j + 1)),
            ]
            .map(|point| Vec3::new(point.x, point.y, point.z));
            if points.iter().any(|p| !p.is_finite()) {
                return Err("STEP analytic surface produced non-finite points".into());
            }
            let normal = surface.normal((u(i) + u(i + 1)) * 0.5, (v(j) + v(j + 1)) * 0.5);
            let normal = Vec3::new(normal.x, normal.y, normal.z);
            for mut vertices in [
                [points[0], points[1], points[2]],
                [points[0], points[2], points[3]],
            ] {
                let cross = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]);
                if cross.length() == 0.0 {
                    continue;
                }
                if (cross.dot(normal) < 0.0) != invert {
                    vertices.swap(1, 2);
                }
                if scene.triangles.len() == MAX_TRIANGLES {
                    return Err("STEP tessellation exceeds triangle limit".into());
                }
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
        }
    }
    Ok(())
}

/// A vertex loop on a full sphere/torus marks a closed surface, not a polygonal trim.
/// Truck 0.3 only accepts edge loops; evaluate these analytic surfaces directly.
fn closed_analytic_shell(
    id: u64,
    entities: &HashMap<u64, &EntityInstance>,
    table: &Table,
) -> Result<Option<Scene>> {
    let shell = simple_record(entities, id)?;
    let shell_params = list(&shell.parameter)?;
    let faces = list(shell_params.get(1).ok_or("missing STEP shell faces")?)?;
    if faces.is_empty() {
        return Ok(None);
    }
    let mut analytic_faces = Vec::new();
    for face in faces {
        let face_id = reference(face)?;
        let face_record = simple_record(entities, face_id)?;
        if !matches!(face_record.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE") {
            return Ok(None);
        }
        let face_params = list(&face_record.parameter)?;
        let bounds = list(face_params.get(1).ok_or("missing STEP face bounds")?)?;
        if bounds.len() != 1 {
            return Ok(None);
        }
        let bound_record = simple_record(entities, reference(&bounds[0])?)?;
        let bound_params = list(&bound_record.parameter)?;
        let loop_record = simple_record(
            entities,
            reference(bound_params.get(1).ok_or("missing STEP face loop")?)?,
        )?;
        if loop_record.name.as_str() != "VERTEX_LOOP" {
            return Ok(None);
        }
        let loop_params = list(&loop_record.parameter)?;
        let vertex_id = reference(loop_params.get(1).ok_or("missing STEP vertex loop point")?)?;
        let vertex = <Table as EntityTable<VertexPointHolder>>::get_owned(table, vertex_id)
            .map_err(|error| format!("cannot resolve STEP vertex loop: {error}"))?;
        let vertex = Point3::from(&vertex.vertex_geometry);
        let surface_id = reference(face_params.get(2).ok_or("missing STEP face surface")?)?;
        let face = table
            .face_surface
            .get(&face_id)
            .ok_or("missing STEP analytic face")?;
        analytic_faces.push((surface_id, !face.same_sense, vertex));
    }
    let mut scene = Scene::default();
    for (surface_id, invert, vertex) in analytic_faces {
        let surface_record = simple_record(entities, surface_id)?;
        let surface_params = list(&surface_record.parameter)?;
        match surface_record.name.as_str() {
            "SPHERICAL_SURFACE" => {
                positive_radius(surface_params.get(2).ok_or("missing STEP sphere radius")?)?;
                let source =
                    <Table as EntityTable<SphericalSurfaceHolder>>::get_owned(table, surface_id)
                        .map_err(|error| format!("cannot resolve STEP sphere: {error}"))?;
                let surface = truck_stepio::r#in::alias::SphericalSurface::from(&source);
                append_closed_surface(&surface, invert, vertex, &mut scene)?;
            }
            "TOROIDAL_SURFACE" => {
                let major = positive_radius(
                    surface_params
                        .get(2)
                        .ok_or("missing STEP torus major radius")?,
                )?;
                let minor = positive_radius(
                    surface_params
                        .get(3)
                        .ok_or("missing STEP torus minor radius")?,
                )?;
                if major <= minor {
                    return Err("STEP horn/spindle torus surfaces are not implemented".into());
                }
                let source =
                    <Table as EntityTable<ToroidalSurfaceHolder>>::get_owned(table, surface_id)
                        .map_err(|error| format!("cannot resolve STEP torus: {error}"))?;
                let surface = truck_stepio::r#in::alias::ToroidalSurface::from(&source);
                append_closed_surface(&surface, invert, vertex, &mut scene)?;
            }
            _ => {
                return Err(
                    "STEP vertex-loop surface requires a supported full sphere or torus".into(),
                );
            }
        }
    }
    scene.validate()?;
    Ok(Some(scene))
}

fn parse(text: &str) -> Result<Scene> {
    check_nesting(text)?;
    let exchange =
        ruststep::parser::parse(text).map_err(|_| "invalid STEP Part 21 exchange file")?;
    if exchange.data.is_empty() {
        return Err("STEP file has no DATA section".into());
    }
    let mut table = Table::default();
    let mut graph = HashMap::new();
    let mut entities = HashMap::new();
    for entity in exchange.data.iter().flat_map(|section| &section.entities) {
        if graph.len() >= MAX_ENTITIES {
            return Err("STEP file exceeds entity limit".into());
        }
        let (id, records) = records(entity);
        let mut references = Vec::new();
        for record in records {
            let name = record.name.as_str();
            if matches!(
                name,
                "MAPPED_ITEM"
                    | "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"
                    | "ITEM_DEFINED_TRANSFORMATION"
                    | "NEXT_ASSEMBLY_USAGE_OCCURRENCE"
                    | "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"
            ) {
                return Err(
                    "STEP assemblies and mapped shape instances are not implemented".into(),
                );
            }
            collect_references(&record.parameter, &mut references)?;
        }
        if graph.insert(id, references).is_some() {
            return Err(format!("duplicate STEP entity #{id}").into());
        }
        entities.insert(id, entity);
        // Avoid Table::from_data_section, which logs and skips malformed entities.
        table
            .push_instance(entity)
            .map_err(|error| format!("cannot decode STEP entity #{id}: {error}"))?;
    }
    if table.shell.is_empty() {
        return Err("STEP file has no supported open/closed B-rep shells".into());
    }
    if table.face_surface.len() > MAX_FACES {
        return Err("STEP file exceeds face limit".into());
    }
    let mut shells: Vec<_> = table.shell.iter().collect();
    shells.sort_unstable_by_key(|&(id, _)| *id);
    let mut scene = Scene::default();
    let mut done = HashMap::new();
    for (&id, shell_holder) in shells {
        check_graph(id, &graph, &mut HashSet::new(), &mut done, 0)?;
        if let Some(analytic) = closed_analytic_shell(id, &entities, &table)? {
            if scene.triangles.len() + analytic.triangles.len() > MAX_TRIANGLES {
                return Err("STEP tessellation exceeds triangle limit".into());
            }
            scene.triangles.extend(analytic.triangles);
            continue;
        }
        let owned_shell = <Table as EntityTable<ShellHolder>>::get_owned(&table, id)
            .map_err(|error| format!("cannot resolve STEP shell #{id}: {error}"))?;
        if owned_shell.cfs_faces.is_empty() {
            return Err(format!("STEP shell #{id} has no faces").into());
        }
        let shell = table
            .to_compressed_shell(shell_holder)
            .map_err(|error| format!("cannot convert STEP shell #{id}: {error}"))?;
        if shell.faces.len() != owned_shell.cfs_faces.len() {
            return Err(format!("STEP shell #{id} contains unsupported faces").into());
        }
        for (converted, original) in shell.faces.iter().zip(&owned_shell.cfs_faces) {
            let face = match original {
                FaceAny::FaceSurface(face) => face,
                FaceAny::OrientedFace(face) => &face.face_element,
            };
            if face.bounds.is_empty() || face.bounds.len() != converted.boundaries.len() {
                return Err(
                    format!("STEP shell #{id} contains unsupported face boundaries").into(),
                );
            }
            for (boundary, original) in converted.boundaries.iter().zip(&face.bounds) {
                if boundary.is_empty() || boundary.len() != original.bound.edge_list.len() {
                    return Err(
                        format!("STEP shell #{id} contains unsupported boundary edges").into(),
                    );
                }
            }
        }
        let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        {
            let mut include = |point: Point3| -> Result<()> {
                let point = Vec3::new(point.x, point.y, point.z);
                if !point.is_finite() {
                    return Err("STEP geometry contains non-finite points".into());
                }
                minimum = Vec3::new(
                    minimum.x.min(point.x),
                    minimum.y.min(point.y),
                    minimum.z.min(point.z),
                );
                maximum = Vec3::new(
                    maximum.x.max(point.x),
                    maximum.y.max(point.y),
                    maximum.z.max(point.z),
                );
                Ok(())
            };
            for point in &shell.vertices {
                include(*point)?;
            }
            // Seam vertices do not describe the extent of a full circle. Sample
            // boundary geometry so wide, thin CAD solids use a useful tolerance.
            for edge in &shell.edges {
                let (start, end) = edge.curve.parameter_range();
                let (start, end) = (finite_parameter_bound(start)?, finite_parameter_bound(end)?);
                for sample in 0..=16 {
                    include(edge.curve.subs(start + (end - start) * sample as f64 / 16.))?;
                }
            }
        }
        let diameter = (maximum - minimum).length();
        if !diameter.is_finite() || diameter <= 0.0 {
            return Err("STEP shell has no finite geometric extent".into());
        }
        let tolerance = (diameter * 0.003).max(TOLERANCE * 10.0);
        let tessellated = shell.robust_triangulation(tolerance);
        if tessellated.faces.iter().any(|face| {
            face.surface
                .as_ref()
                .is_none_or(|mesh| mesh.face_iter().next().is_none())
        }) {
            return Err(
                format!("Rust CAD tessellation failed for a face of STEP shell #{id}").into(),
            );
        }
        let mesh = tessellated.to_polygon();
        for face in mesh.face_iter() {
            let polygon = face
                .iter()
                .map(|vertex| {
                    mesh.positions()
                        .get(vertex.pos)
                        .map(|point| Vec3::new(point.x, point.y, point.z))
                        .ok_or_else(|| "STEP tessellation returned an invalid vertex index".into())
                })
                .collect::<Result<Vec<_>>>()?;
            for vertices in super::mesh::triangulate_polygon(&polygon)? {
                if scene.triangles.len() >= MAX_TRIANGLES {
                    return Err("STEP tessellation exceeds triangle limit".into());
                }
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
        }
    }
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use truck_modeling::{Point3, Vector3, builder};
    use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

    fn cube_step() -> String {
        let origin = builder::vertex(Point3::new(2.0, 3.0, 4.0));
        let edge = builder::tsweep(&origin, Vector3::new(2.0, 0.0, 0.0));
        let face = builder::tsweep(&edge, Vector3::new(0.0, 3.0, 0.0));
        let cube = builder::tsweep(&face, Vector3::new(0.0, 0.0, 4.0)).compress();
        CompleteStepDisplay::new(StepModel::from(&cube), StepHeaderDescriptor::default())
            .to_string()
    }

    #[test]
    fn solid_step_tessellates_real_faces_and_preserves_bounds() {
        let scene = parse(&cube_step()).unwrap();
        assert!(scene.triangles.len() >= 12);
        assert!(scene.points.is_empty());
        for (component, expected) in [(0, (2.0, 4.0)), (1, (3.0, 6.0)), (2, (4.0, 8.0))] {
            let values: Vec<_> = scene
                .positions()
                .map(|p| [p.x, p.y, p.z][component])
                .collect();
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            assert!((min - expected.0).abs() < 1e-8);
            assert!((max - expected.1).abs() < 1e-8);
        }
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 52.0).abs() < 1e-7);
        let thumbnail = crate::render::render(
            &scene,
            &crate::render::RenderOptions {
                width: 64,
                height: 64,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(thumbnail.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
    }

    #[test]
    fn malformed_reference_and_assembly_inputs_fail() {
        let valid = cube_step();
        assert!(parse("not a STEP file").is_err());
        let point_only = "ISO-10303-21; HEADER; FILE_DESCRIPTION(('test'),'2;1'); FILE_NAME('','',(''),(''),'','',''); FILE_SCHEMA(('AUTOMOTIVE_DESIGN')); ENDSEC; DATA; #1=CARTESIAN_POINT('',(0.,0.,0.)); ENDSEC; END-ISO-10303-21;";
        assert!(parse(point_only).is_err());
        let assembly = valid.replace(
            "ENDSEC;\nEND-ISO",
            "#999999=MAPPED_ITEM('',#1,#1);\nENDSEC;\nEND-ISO",
        );
        assert!(parse(&assembly).is_err());
        let broken = valid.replacen("CARTESIAN_POINT", "UNKNOWN_GEOMETRY", 1);
        assert!(parse(&broken).is_err());
    }

    #[test]
    fn curved_step_cylinder_is_tessellated_as_a_surface() {
        let vertex = builder::vertex(Point3::new(1.0, 0.0, 0.0));
        let circle = builder::rsweep(
            &vertex,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::unit_z(),
            truck_modeling::Rad(std::f64::consts::TAU),
        );
        let disk = builder::try_attach_plane(&[circle]).unwrap();
        let cylinder = builder::tsweep(&disk, Vector3::new(0.0, 0.0, 2.0)).compress();
        let text =
            CompleteStepDisplay::new(StepModel::from(&cylinder), StepHeaderDescriptor::default())
                .to_string();
        let scene = parse(&text).unwrap();
        assert!(scene.triangles.len() > 40);
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 6.0 * std::f64::consts::PI).abs() < 0.3);
        assert!(scene.positions().all(|p| p.x.abs() <= 1.00001
            && p.y.abs() <= 1.00001
            && p.z >= -1e-6
            && p.z <= 2.000001));
    }

    #[test]
    fn wide_thin_cylinder_uses_boundary_geometry_to_scale_tessellation() {
        let radius = 10_000.;
        let vertex = builder::vertex(Point3::new(radius, 0., 0.));
        let circle = builder::rsweep(
            &vertex,
            Point3::origin(),
            Vector3::unit_z(),
            truck_modeling::Rad(std::f64::consts::TAU),
        );
        let disk = builder::try_attach_plane(&[circle]).unwrap();
        let cylinder = builder::tsweep(&disk, Vector3::unit_z()).compress();
        let text =
            CompleteStepDisplay::new(StepModel::from(&cylinder), StepHeaderDescriptor::default())
                .to_string();
        let scene = parse(&text).unwrap();
        assert!(scene.triangles.len() < 2000);
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        let expected = std::f64::consts::TAU * radius * (radius + 1.);
        assert!((area - expected).abs() / expected < 0.03);
    }

    fn vertex_loop_step(surface: &str, vertex: [f64; 3]) -> String {
        format!(
            "ISO-10303-21; HEADER; FILE_DESCRIPTION(('analytic surface'),'2;1'); FILE_SCHEMA(('AUTOMOTIVE_DESIGN')); ENDSEC; DATA;\n#1=CLOSED_SHELL('',(#2));\n#2=ADVANCED_FACE('',(#3),#7,.T.);\n#3=FACE_BOUND('',#4,.T.);\n#4=VERTEX_LOOP('',#5);\n#5=VERTEX_POINT('',#6);\n#6=CARTESIAN_POINT('',({}, {}, {}));\n#7={surface};\n#8=AXIS2_PLACEMENT_3D('',#9,#10,#11);\n#9=CARTESIAN_POINT('',(1.,2.,3.));\n#10=DIRECTION('',(0.,0.,-1.));\n#11=DIRECTION('',(0.,1.,0.));\nENDSEC; END-ISO-10303-21;",
            vertex[0], vertex[1], vertex[2]
        )
    }

    #[test]
    fn sphere_and_torus_vertex_loops_preserve_surface_and_placement() {
        for (surface, vertex, area, bounds) in [
            (
                "SPHERICAL_SURFACE('',#8,5.)",
                [1.0, 2.0, 8.0],
                100.0 * std::f64::consts::PI,
                [(-4.0, 6.0), (-3.0, 7.0), (-2.0, 8.0)],
            ),
            (
                "TOROIDAL_SURFACE('',#8,3.,1.)",
                [1.0, 6.0, 3.0],
                12.0 * std::f64::consts::PI.powi(2),
                [(-3.0, 5.0), (-2.0, 6.0), (2.0, 4.0)],
            ),
        ] {
            let scene = parse(&vertex_loop_step(surface, vertex)).unwrap();
            assert!(scene.triangles.len() >= 4000);
            assert!(scene.points.is_empty());
            let observed_area: f64 = scene
                .triangles
                .iter()
                .map(|t| {
                    (t.vertices[1] - t.vertices[0])
                        .cross(t.vertices[2] - t.vertices[0])
                        .length()
                        * 0.5
                })
                .sum();
            assert!((observed_area - area).abs() / area < 0.005);
            for (axis, (expected_min, expected_max)) in bounds.into_iter().enumerate() {
                let values: Vec<_> = scene.positions().map(|p| [p.x, p.y, p.z][axis]).collect();
                let observed_min = values.iter().copied().fold(f64::INFINITY, f64::min);
                let observed_max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                assert!((observed_min - expected_min).abs() < 1e-8);
                assert!((observed_max - expected_max).abs() < 1e-8);
            }
        }
        assert!(
            parse(&vertex_loop_step(
                "TOROIDAL_SURFACE('',#8,1.,2.)",
                [1.0, 5.0, 3.0]
            ))
            .is_err()
        );
        assert!(
            parse(&vertex_loop_step(
                "SPHERICAL_SURFACE('',#8,5.)",
                [1.0, 2.0, 3.0]
            ))
            .is_err()
        );
    }

    #[test]
    fn cyclic_and_deep_references_are_rejected_without_recursing_forever() {
        let mut graph = HashMap::from([(1, vec![2]), (2, vec![1])]);
        assert!(check_graph(1, &graph, &mut HashSet::new(), &mut HashMap::new(), 0).is_err());
        graph = (0..100)
            .map(|id| (id, if id == 99 { vec![] } else { vec![id + 1] }))
            .collect();
        assert!(check_graph(0, &graph, &mut HashSet::new(), &mut HashMap::new(), 0).is_err());
        assert!(check_nesting(&format!("{}{}", "(".repeat(100), ")".repeat(100))).is_err());
    }
}
