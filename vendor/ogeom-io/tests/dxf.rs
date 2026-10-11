//! Reading DXF drawings as typed curves: arcs, circles, bulged and closed
//! polylines, units and dashed layers.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use core::f64::consts::{FRAC_PI_2, PI};
use ogeom_io::dxf::{DxfCurve, read_dxf, read_dxf_entities};

const REPRO: &str = "0\nSECTION\n2\nENTITIES\n0\nCIRCLE\n8\n0\n10\n0.0\n20\n0.0\n40\n5.0\n0\nARC\n8\n0\n10\n20.0\n20\n0.0\n40\n4.0\n50\n0.0\n51\n90.0\n0\nLWPOLYLINE\n8\n0\n90\n2\n70\n1\n10\n40.0\n20\n0.0\n42\n1.0\n10\n50.0\n20\n0.0\n42\n1.0\n0\nPOLYLINE\n8\n0\n66\n1\n10\n0.0\n20\n0.0\n30\n0.0\n70\n0\n0\nVERTEX\n8\n0\n10\n5.0\n20\n5.0\n0\nVERTEX\n8\n0\n10\n15.0\n20\n5.0\n0\nSEQEND\n0\nENDSEC\n0\nEOF\n";

#[test]
fn circles_arcs_and_bulged_closed_polylines_read_as_written() {
    let d = read_dxf_entities(REPRO).unwrap();
    assert_eq!(d.entities.len(), 4);
    assert!(matches!(d.entities[0].curve, DxfCurve::Circle { radius, .. } if radius == 5.0));
    assert!(matches!(d.entities[1].curve,
        DxfCurve::Arc { radius, start_angle, end_angle, .. }
            if radius == 4.0 && start_angle == 0.0 && (end_angle - FRAC_PI_2).abs() < 1e-12));
    let DxfCurve::Polyline { vertices, closed } = &d.entities[2].curve else {
        panic!("{:?}", d.entities[2]);
    };
    assert!(*closed);
    assert_eq!(
        vertices.iter().map(|v| v.1).collect::<Vec<_>>(),
        vec![1.0, 1.0]
    );
    let DxfCurve::Polyline { vertices, closed } = &d.entities[3].curve else {
        panic!("{:?}", d.entities[3]);
    };
    assert!(!*closed);
    assert_eq!(
        vertices.iter().map(|v| (v.0.x, v.0.y)).collect::<Vec<_>>(),
        vec![(5.0, 5.0), (15.0, 5.0)]
    );
    // The polyline reader does not take the header's point for a vertex.
    assert_eq!(read_dxf(REPRO).unwrap().visible.last().unwrap().len(), 2);
    assert_eq!(d.insunits, None);
}

#[test]
fn the_header_names_the_units() {
    let text = format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n1\n0\nENDSEC\n{REPRO}");
    let d = read_dxf_entities(&text).unwrap();
    assert_eq!(d.insunits, Some(1));
    assert_eq!(d.unit_mm, Some(25.4));
    assert_eq!(d.entities.len(), 4);
}

#[test]
fn a_dashed_layer_draws_hidden_lines() {
    let text = "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n0\nLAYER\n2\nBACK\n70\n0\n6\nDASHED\n0\nENDTAB\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nLINE\n8\nBACK\n10\n0.0\n20\n0.0\n11\n1.0\n21\n0.0\n0\nLINE\n8\nFRONT\n10\n0.0\n20\n1.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n0\nEOF\n";
    let d = read_dxf_entities(text).unwrap();
    assert!(d.entities[0].hidden);
    assert!(!d.entities[1].hidden);
    assert_eq!(d.entities[1].layer, "FRONT");
}

#[test]
fn an_arc_seen_from_below_is_mirrored_into_the_drawing() {
    let text = "0\nSECTION\n2\nENTITIES\n0\nARC\n8\n0\n10\n3.0\n20\n0.0\n40\n1.0\n50\n0.0\n51\n90.0\n210\n0.0\n220\n0.0\n230\n-1.0\n0\nENDSEC\n0\nEOF\n";
    let d = read_dxf_entities(text).unwrap();
    let DxfCurve::Arc {
        centre,
        start_angle,
        end_angle,
        ..
    } = d.entities[0].curve
    else {
        panic!("{:?}", d.entities[0]);
    };
    assert_eq!((centre.x, centre.y), (-3.0, 0.0));
    assert!((start_angle - FRAC_PI_2).abs() < 1e-12);
    assert!((end_angle - PI).abs() < 1e-12);
}
