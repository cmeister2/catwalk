//! Placement controls exercise the tolerant reader's silent-identity cases.
use super::*;

/// Complete serialized generated-style parent/child placement, including a quarter turn.
const RECORDS: &str = "\
#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CARTESIAN_POINT('',(10.,20.,30.));
#6=DIRECTION('',(0.,1.,0.));
#7=AXIS2_PLACEMENT_3D('',#5,#2,#6);
#10=PRODUCT_DEFINITION('root','',#100,#100);
#11=PRODUCT_DEFINITION('part','',#100,#100);
#12=PRODUCT_DEFINITION_SHAPE('','',#10);
#13=PRODUCT_DEFINITION_SHAPE('','',#11);
#20=SHAPE_REPRESENTATION('root',(#4),#100);
#21=SHAPE_REPRESENTATION('part',(#4),#100);
#22=SHAPE_DEFINITION_REPRESENTATION(#12,#20);
#23=SHAPE_DEFINITION_REPRESENTATION(#13,#21);
#30=NEXT_ASSEMBLY_USAGE_OCCURRENCE('use','placed','',#10,#11,$);
#31=PRODUCT_DEFINITION_SHAPE('','',#30);
#32=ITEM_DEFINED_TRANSFORMATION('','',#4,#7);
#33=(REPRESENTATION_RELATIONSHIP('','',#21,#20) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#32) SHAPE_REPRESENTATION_RELATIONSHIP());
#34=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#33,#31);
#100=GEOMETRIC_REPRESENTATION_CONTEXT(3);
";

/// The shared bounded parser accepts a complete exchange, not individual snippets.
fn exchange(records: &str) -> String {
    format!(
        "ISO-10303-21;HEADER;FILE_DESCRIPTION(('placement control'),'2;1');\
        FILE_NAME('','',(''),(''),'','','');FILE_SCHEMA(('AP242'));ENDSEC;\
        DATA;{records}ENDSEC;END-ISO-10303-21;"
    )
}

#[test]
fn explicit_parent_child_transform_and_reversed_relationship_order_are_qualified() {
    validate(&exchange(RECORDS)).unwrap();
    let reversed = RECORDS.replace(
        "REPRESENTATION_RELATIONSHIP('','',#21,#20)",
        "REPRESENTATION_RELATIONSHIP('','',#20,#21)",
    );
    validate(&exchange(&reversed)).unwrap();
}

#[test]
fn missing_or_invalid_usage_transform_never_defaults_to_identity() {
    for modified in [
        RECORDS.replace("#34=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#33,#31);", ""),
        RECORDS.replace(
            "ITEM_DEFINED_TRANSFORMATION('','',#4,#7)",
            "ITEM_DEFINED_TRANSFORMATION('','',#4,#999)",
        ),
        RECORDS.replace(
            "AXIS2_PLACEMENT_3D('',#5,#2,#6)",
            "AXIS2_PLACEMENT_3D('',#5,#2,#2)",
        ),
        RECORDS.replace("DIRECTION('',(0.,1.,0.))", "DIRECTION('',(0.,0.,0.))"),
        RECORDS.replace("DIRECTION('',(0.,1.,0.))", "DIRECTION('',(0.,2.,0.))"),
        RECORDS.replace(
            "CARTESIAN_POINT('',(10.,20.,30.))",
            "CARTESIAN_POINT('',(1.e999,20.,30.))",
        ),
        RECORDS.replace(
            "REPRESENTATION_RELATIONSHIP('','',#21,#20)",
            "REPRESENTATION_RELATIONSHIP('','',#21,#21)",
        ),
        RECORDS.replace(
            "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#32)",
            "UNQUALIFIED_TRANSFORMATION(#32)",
        ),
    ] {
        assert!(
            validate(&exchange(&modified)).is_err(),
            "accepted {modified}"
        );
    }
}

#[test]
fn placement_and_product_ownership_must_be_unambiguous() {
    for extra in [
        "#200=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#33,#31);",
        "#200=SHAPE_DEFINITION_REPRESENTATION(#13,#20);",
        "#33=(REPRESENTATION_RELATIONSHIP('','',#21,#20) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#32) SHAPE_REPRESENTATION_RELATIONSHIP());",
    ] {
        assert!(validate(&exchange(&format!("{RECORDS}{extra}"))).is_err());
    }
    let missing_product = RECORDS.replace("#11=PRODUCT_DEFINITION('part','',#100,#100);", "");
    assert!(validate(&exchange(&missing_product)).is_err());
}

#[test]
fn mapped_body_and_scaled_operator_placements_are_explicitly_refused() {
    for extra in [
        "#200=REPRESENTATION_MAP(#4,#21);#201=MAPPED_ITEM('body',#200,#7);",
        "#200=CARTESIAN_TRANSFORMATION_OPERATOR_3D('',#3,#6,#1,2.,#2);",
    ] {
        let error = validate(&exchange(&format!("{RECORDS}{extra}"))).unwrap_err();
        assert!(error.to_string().contains("mapped body placements"));
    }
}
