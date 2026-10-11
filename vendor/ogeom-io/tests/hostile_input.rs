//! Malformed and hostile files: every reader answers with an error, never
//! a panic, a stack overflow or an allocation sized by a number in the file.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use ogeom_core::Tolerances;

const T: Tolerances = Tolerances::millimetres();

fn step(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
         FILE_NAME('','',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
         ENDSEC;\nDATA;\n{body}\nENDSEC;\nEND-ISO-10303-21;\n"
    )
}

/// A solid whose face, edge and vertex entities each stop short of the
/// arguments they need.
#[test]
fn a_step_entity_with_too_few_arguments_is_refused() {
    let body = "#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=VERTEX_POINT('');\n\
                #7=EDGE_CURVE('',#2);\n#8=ORIENTED_EDGE('',*,*,#7,.T.);\n\
                #9=EDGE_LOOP('',(#8));\n#10=FACE_OUTER_BOUND('',#9,.T.);\n\
                #11=PLANE('');\n#12=ADVANCED_FACE('',(#10),#11,.T.);\n\
                #13=CLOSED_SHELL('',(#12));\n#14=MANIFOLD_SOLID_BREP('',#13);";
    assert!(ogeom_io::read_step(&step(body), T).is_err());
}

#[test]
fn a_step_shell_oriented_after_itself_is_refused() {
    let body = "#5=ORIENTED_CLOSED_SHELL('',*,#5,.T.);\n#6=MANIFOLD_SOLID_BREP('',#5);";
    assert!(ogeom_io::read_step(&step(body), T).is_err());
}

#[test]
fn a_step_offset_surface_of_itself_is_refused() {
    let body = "#1=OFFSET_SURFACE('',#1,1.,.F.);\n\
                #2=CARTESIAN_POINT('',(0.,0.,0.));\n#3=VERTEX_POINT('',#2);\n\
                #9=EDGE_LOOP('',());\n#10=FACE_OUTER_BOUND('',#9,.T.);\n\
                #12=ADVANCED_FACE('',(#10),#1,.T.);\n\
                #13=CLOSED_SHELL('',(#12));\n#14=MANIFOLD_SOLID_BREP('',#13);";
    assert!(ogeom_io::read_step(&step(body), T).is_err());
}

#[test]
fn deeply_nested_step_arguments_are_refused() {
    let deep = format!(
        "#1=CARTESIAN_POINT('',{}0{});",
        "(".repeat(100_000),
        ")".repeat(100_000)
    );
    assert!(ogeom_io::read_step(&step(&deep), T).is_err());
}

/// One instance numbered in the billions: the reader's bookkeeping must
/// not be sized by the number.
#[test]
fn a_step_id_in_the_billions_is_read_without_a_table_that_size() {
    let body = "#999999999999=CARTESIAN_POINT('',(0.,0.,0.));";
    assert!(ogeom_io::read_step(&step(body), T).is_err());
}

#[test]
fn a_negative_step_id_is_refused() {
    let body = "#-1=CARTESIAN_POINT('',(0.,0.,0.));";
    assert!(ogeom_io::read_step(&step(body), T).is_err());
}

/// A knot repeated a billion billion times, and a degree past any real
/// spline's.
#[test]
fn step_spline_counts_past_any_real_spline_are_refused() {
    let points = "#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=CARTESIAN_POINT('',(1.,0.,0.));\n";
    let knots = format!(
        "{points}#3=B_SPLINE_CURVE_WITH_KNOTS('',1,(#1,#2),.UNSPECIFIED.,.F.,.F.,\
         (1000000000000000000,2),(0.,1.),.UNSPECIFIED.);\n\
         #4=VERTEX_POINT('',#1);\n#5=VERTEX_POINT('',#2);\n\
         #6=EDGE_CURVE('',#4,#5,#3,.T.);\n#7=ORIENTED_EDGE('',*,*,#6,.T.);\n\
         #8=EDGE_LOOP('',(#7));\n#9=FACE_OUTER_BOUND('',#8,.T.);\n\
         #10=AXIS2_PLACEMENT_3D('',#1,$,$);\n#11=PLANE('',#10);\n\
         #12=ADVANCED_FACE('',(#9),#11,.T.);\n#13=CLOSED_SHELL('',(#12));\n\
         #14=MANIFOLD_SOLID_BREP('',#13);"
    );
    assert!(ogeom_io::read_step(&step(&knots), T).is_err());
    let degree = knots.replace("('',1,(#1,#2)", "('',1.E30,(#1,#2)");
    assert!(ogeom_io::read_step(&step(&degree), T).is_err());
}
