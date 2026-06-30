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
fn refs_imports_hints_are_identical_across_runs() {
    // A source exercising Cycle-2 facts: imports, a call ref, an instantiate, an
    // export, an entrypoint, and a cut hint.
    let src = "from a.b import c\nimport os.path as p\n__all__ = [\"run\"]\nclass W:\n    pass\ndef run(x):\n    w = W()\n    if x:\n        getattr(w, \"go\")()\nif __name__ == \"__main__\":\n    run(1)\n";
    let a = extract("app/store.py", src);
    let b = extract("app/store.py", src);
    assert_eq!(a.refs, b.refs);
    assert_eq!(a.imports, b.imports);
    assert_eq!(a.exports, b.exports);
    assert_eq!(a.entrypoint_hints, b.entrypoint_hints);
    assert_eq!(a.cut_hints, b.cut_hints);
    // Non-empty: the facts actually emit.
    assert!(!a.refs.is_empty() && !a.imports.is_empty());
    assert!(!a.exports.is_empty() && !a.entrypoint_hints.is_empty());
    assert!(!a.cut_hints.is_empty());
}

#[test]
fn inheritance_relations_are_identical_across_runs() {
    let a = extract_fixture("lattice.py");
    let b = extract_fixture("lattice.py");
    assert_eq!(a.impl_relations, b.impl_relations);
    assert!(
        !a.impl_relations.is_empty(),
        "lattice fixture must emit inherits/override relations"
    );
}

#[test]
fn effects_and_concurrency_are_identical_across_runs() {
    // A source exercising Cycle-4 facts: own-effects (io.net, io.proc,
    // nondeterministic, blocking), a spawn site, an await suspension, and a lock
    // `with` block — all of which feed `effects` and the spawn/async refs.
    let src = "import os, asyncio, threading, subprocess\nasync def run(u):\n    data = await fetch(u)\n    subprocess.run(['ls'])\n    _ = os.getenv('X')\n    asyncio.create_task(work())\n    threading.Thread(target=work)\n    with threading.Lock():\n        pass\n    return data\n";
    let a = extract("app/store.py", src);
    let b = extract("app/store.py", src);
    assert_eq!(a.effects, b.effects);
    assert_eq!(a.refs, b.refs);
    assert!(!a.effects.is_empty(), "effects must emit for this source");
}

#[test]
fn data_flows_are_identical_across_runs() {
    // A source exercising Cycle-5 dataflow: copy, arith, augmented, projection,
    // an opaque call, a tuple unpack, and a return — every fact must serialize
    // byte-stable across extractions.
    let src = "def g(a, b):\n    c = a + b\n    c += a\n    n = a.field\n    r = h(a)\n    p, q = make()\n    y = c\n    return y\n";
    let a = extract("app/df.py", src);
    let b = extract("app/df.py", src);
    assert_eq!(a.data_flows, b.data_flows);
    assert!(
        !a.data_flows.is_empty(),
        "data_flows must emit for this source"
    );
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
