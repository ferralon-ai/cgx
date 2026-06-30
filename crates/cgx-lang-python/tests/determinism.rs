mod common;

use cgx_core::node::SymbolKind;
use common::{extract, extract_fixture};

#[test]
fn extraction_is_byte_identical_across_runs() {
    let a = extract_fixture("shapes.py");
    let b = extract_fixture("shapes.py");
    assert_eq!(a, b, "canonical facts must be identical across extractions");
}

#[test]
fn inline_extraction_is_byte_identical_across_runs() {
    let src = "MAX = 1\nclass C:\n    def m(self):\n        pass\ndef f():\n    pass\n";
    let a = extract("app/store.py", src);
    let b = extract("app/store.py", src);
    assert_eq!(a, b);
}

#[test]
fn fixture_def_set_is_complete() {
    let facts = extract_fixture("shapes.py");
    let fqns: Vec<&str> = facts.defs.iter().map(|d| d.fqn.as_str()).collect();
    for expect in [
        "shapes::MAX_SIDES",
        "shapes::default_name",
        "shapes::WIDTH",
        "shapes::HEIGHT",
        "shapes::area",
        "shapes::_internal_helper",
        "shapes::Shape",
        "shapes::Shape::__init__",
        "shapes::Shape::describe",
        "shapes::Shape::render",
        "shapes::Shape::sides",
        "shapes::decorated_fn",
        "shapes::Circle",
        "shapes::Circle::unit",
    ] {
        assert!(fqns.contains(&expect), "missing {expect}; have {fqns:?}");
    }

    // The abstract method must be flagged.
    assert!(
        facts
            .defs
            .iter()
            .any(|d| d.fqn.ends_with("Shape::render") && d.is_abstract),
        "Shape::render must be abstract"
    );
    // The class-body assignment is a Field; the module constant is a Constant.
    assert_eq!(
        facts
            .defs
            .iter()
            .find(|d| d.fqn.ends_with("Shape::sides"))
            .unwrap()
            .kind,
        SymbolKind::Field
    );
}
