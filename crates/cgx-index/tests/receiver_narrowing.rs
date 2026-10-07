//! Receiver-kind narrowing of Python virtual call sites, end to end through
//! `index_path` (real Python extraction, then link).
//!
//! The main fixture plants a same-named method on an unrelated class next to
//! every narrowed site, so the same-name method set and the narrowed set
//! differ by exactly the infeasible targets.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use cgx_core::confidence::Confidence;
use cgx_core::node::SymbolKind;
use cgx_frontend::{FileCtx, LanguageFrontend};
use cgx_index::{index_path, IndexOpts, IndexOutcome};
use common::*;

const BASE_PY: &str = r#"
class Base:
    def save(self):
        return self.validate()

    def validate(self):
        return True


class Mixin:
    def run(self):
        return self.validate()
"#;

const WIDGETS_PY: &str = r#"
import os
from unittest import TestCase

from pkg.base import Base, Mixin


class Widget(Mixin, Base):
    def validate(self):
        return False

    def build(self):
        Widget.make()
        return os.path.join("a", "b")

    @classmethod
    def make(cls):
        return cls


class Unrelated:
    def validate(self):
        return 1

    def join(self, *parts):
        return 2

    def make(self):
        return 3

    def assertEqual(self, a, b):
        return a == b


class MyTest(TestCase):
    def test_x(self):
        self.assertEqual(1, 1)
"#;

const SVC_PY: &str = r#"
from typing import Optional

from pkg.base import Base
from pkg.widgets import Unrelated


def use_annotated(b: Optional[Base]):
    return b.validate()


def use_ctor():
    u = Unrelated()
    return u.validate()


class Child(Base):
    def __init__(self):
        super().__init__()
        self.helper = Unrelated()

    def validate(self):
        ", ".join([])
        return super().validate()

    def go(self):
        Unrelated().make()
        return self.helper.validate()


def use_rebound(items):
    x = Unrelated()
    for x in items:
        pass
    return x.validate()
"#;

/// Every fixture set, for the invariant checks.
const ALL_FIXTURES: &[&[(&str, &str)]] = &[
    MAIN_FILES,
    HONESTY_FILES,
    IDENTITY_FILES,
    TYPING_FILES,
    GO_FILES,
    SHAPES_FILES,
    ROOTS_FILES,
    FACTORY_FILES,
    ORDER_FILES,
    FACTORY_VALUE_FILES,
    REVIEW_PY_FILES,
    REVIEW_GO_FILES,
    STORES_FILES,
    CALLBACKS_FILES,
    CTOR_FILES,
];

const MAIN_FILES: &[(&str, &str)] = &[
    ("pkg/__init__.py", ""),
    ("pkg/base.py", BASE_PY),
    ("pkg/widgets.py", WIDGETS_PY),
    ("pkg/svc.py", SVC_PY),
];

/// Honesty cases: unresolvable and dynamic bases, an attribute holding a
/// callable, and two defs sharing one FQN.
const HONESTY_FILES: &[(&str, &str)] = &[
    ("pkg/__init__.py", ""),
    ("pkg/missing.py", "def helper():\n    return 1\n"),
    ("pkg/helpers.py", "def make_base():\n    return object\n"),
    (
        "pkg/bases.py",
        r#"
from sqlalchemy.orm import declarative_base

from pkg.helpers import make_base
from pkg.missing import *

Base = declarative_base()


class StarBase(Base2):
    def __init__(self):
        super().__init__()


class Dyn(make_base()):
    def __init__(self):
        super().__init__()


class Decl(declarative_base()):
    def __init__(self):
        super().__init__()


class Model(Base):
    def __init__(self):
        super().__init__()


def some_fn():
    return 1


class Holder:
    def fire(self):
        return self.handler()


def install(h):
    h.handler = some_fn


class Other:
    def __init__(self):
        pass

    def handler(self):
        return 2
"#,
    ),
    (
        "pkg/dupes.py",
        "def dup():\n    missing_one()\n\n\ndef dup():\n    missing_two()\n",
    ),
];

/// Module identity: package re-exports, a sys.path root below the repo root,
/// an in-repo module named like a stdlib one, and a dotted plain import.
const IDENTITY_FILES: &[(&str, &str)] = &[
    ("pyproject.toml", "[project]\nname = \"identity\"\n"),
    ("pkg/__init__.py", ""),
    ("pkg/forms/__init__.py", "from pkg.forms.fields import *\n"),
    (
        "pkg/forms/fields.py",
        "class Field:\n    def clean(self):\n        return 1\n",
    ),
    (
        "pkg/other.py",
        "class Field:\n    def clean(self):\n        return 2\n\n\nclass Q:\n    def m(self):\n        return 3\n\n    def dumps(self, x):\n        return x\n",
    ),
    ("pkg/json.py", "def dumps(x):\n    return x\n"),
    (
        "pkg/user.py",
        "from pkg import forms\nimport json\nimport os.path\n\n\ndef use():\n    forms.Field.clean(None)\n    json.dumps({})\n    os.path.join(\"a\")\n",
    ),
    ("tests/app/__init__.py", ""),
    (
        "tests/app/models.py",
        "class M:\n    @classmethod\n    def m(cls):\n        return 4\n",
    ),
    (
        "tests/app/test_models.py",
        "from app.models import M\n\n\ndef t():\n    return M.m()\n",
    ),
];

fn index_files(files: &[(&str, &str)], narrow: bool) -> (IndexOutcome, GraphIndex) {
    let (_t, repo) = init_empty_repo();
    for (path, text) in files {
        write_file(&repo, path, text);
    }
    commit_all(&repo, "receiver fixture");
    let registry = cgx_index::default_registry();
    let mut store = mem_store();
    let opts = IndexOpts {
        receiver_narrowing: narrow,
        ..IndexOpts::default()
    };
    let outcome = index_path(&repo, &registry, &mut store, &opts).unwrap();
    let g = read_graph(&store, &outcome.graph_key);
    (outcome, GraphIndex::new(&g))
}

fn index(narrow: bool) -> (IndexOutcome, GraphIndex) {
    index_files(MAIN_FILES, narrow)
}

fn callees(idx: &GraphIndex, caller: &str) -> BTreeSet<String> {
    idx.edges
        .iter()
        .filter(|e| e.kind.is_call() && idx.fqn_of(e.src) == caller)
        .map(|e| idx.fqn_of(e.dst).to_owned())
        .collect()
}

fn node<'a>(idx: &'a GraphIndex, fqn: &str) -> &'a cgx_core::node::NodeRecord {
    idx.nodes
        .iter()
        .find(|n| n.fqn == fqn)
        .unwrap_or_else(|| panic!("no node {fqn}"))
}

fn set(xs: &[&str]) -> BTreeSet<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn dangling_sum(idx: &GraphIndex) -> usize {
    idx.nodes
        .iter()
        .map(|n| (n.unresolved_calls + n.external_calls + n.narrowing.visible_dropped_out) as usize)
        .sum()
}

#[test]
fn legacy_links_every_same_named_method() {
    let (_o, idx) = index(false);
    assert!(callees(&idx, "pkg::base::Base::save").contains("pkg::widgets::Unrelated::validate"));
    assert!(callees(&idx, "pkg::widgets::Widget::build").contains("pkg::widgets::Unrelated::join"));
    assert!(callees(&idx, "pkg::widgets::MyTest::test_x")
        .contains("pkg::widgets::Unrelated::assertEqual"));
}

#[test]
fn self_call_is_bounded_by_the_class_cone() {
    let (o, idx) = index(true);
    // Base and its in-repo subclasses Widget and Child; never the unrelated class.
    assert_eq!(
        callees(&idx, "pkg::base::Base::save"),
        set(&[
            "pkg::base::Base::validate",
            "pkg::svc::Child::validate",
            "pkg::widgets::Widget::validate"
        ])
    );
    // A mixin's self-call resolves through the composing subclass's MRO.
    assert_eq!(
        callees(&idx, "pkg::base::Mixin::run"),
        set(&["pkg::widgets::Widget::validate"])
    );
    let single: Vec<_> = idx
        .edges
        .iter()
        .filter(|e| idx.fqn_of(e.src) == "pkg::base::Mixin::run")
        .collect();
    assert_eq!(single[0].confidence, Confidence::Probable);
    assert_eq!(single[0].rule, "recv-self");
    assert!(o.stats.precision.self_cone.sites >= 2);
}

#[test]
fn class_headed_call_uses_the_class_lookup() {
    let (_o, idx) = index(true);
    let got = callees(&idx, "pkg::widgets::Widget::build");
    assert!(got.contains("pkg::widgets::Widget::make"), "{got:?}");
    assert!(!got.contains("pkg::widgets::Unrelated::make"), "{got:?}");
}

#[test]
fn external_receivers_dangle_honestly() {
    let (o, idx) = index(true);
    // `os.path.join` and an inherited `unittest.TestCase.assertEqual` cannot
    // target an in-repo method: no edge, but a counted external call on the
    // caller so a forward negative is never reported exact.
    assert!(!callees(&idx, "pkg::widgets::Widget::build").contains("pkg::widgets::Unrelated::join"));
    assert!(callees(&idx, "pkg::widgets::MyTest::test_x").is_empty());
    assert!(node(&idx, "pkg::widgets::Widget::build").external_calls >= 1);
    assert!(node(&idx, "pkg::widgets::MyTest::test_x").external_calls >= 1);
    assert_eq!(o.stats.precision.external_module.dangling, 1);
    assert_eq!(o.stats.precision.self_cone.dangling, 1);
}

#[test]
fn super_and_literal_receivers() {
    let (o, idx) = index(true);
    // super().validate() → Base.validate only; the str literal's join dangles.
    let v = callees(&idx, "pkg::svc::Child::validate");
    assert_eq!(v, set(&["pkg::base::Base::validate"]), "{v:?}");
    assert_eq!(o.stats.precision.literal.dangling, 1);
    // super().__init__() reaches only `object.__init__`: out of repo. (The
    // remaining callee is the `Unrelated()` instantiation.)
    assert_eq!(
        callees(&idx, "pkg::svc::Child::__init__"),
        set(&["pkg::widgets::Unrelated"])
    );
    assert_eq!(node(&idx, "pkg::svc::Child::__init__").external_calls, 1);
}

#[test]
fn narrowing_is_deterministic() {
    for files in ALL_FIXTURES {
        let (a, ia) = index_files(files, true);
        let (b, ib) = index_files(files, true);
        assert_eq!(a.graph_key, b.graph_key);
        assert_eq!(ia.edges, ib.edges);
        assert_eq!(ia.nodes, ib.nodes);
    }
}

/// I2: every dangling ref is counted on exactly one node, with or without
/// narrowing, including two defs that share an FQN in one file.
#[test]
fn every_dangling_ref_is_counted_once() {
    for files in ALL_FIXTURES {
        for narrow in [false, true] {
            let (o, idx) = index_files(files, narrow);
            assert_eq!(dangling_sum(&idx), o.stats.unresolved, "narrow={narrow}");
        }
    }
    let (_o, idx) = index_files(HONESTY_FILES, false);
    let dups: Vec<_> = idx
        .nodes
        .iter()
        .filter(|n| n.fqn == "pkg::dupes::dup")
        .collect();
    assert_eq!(dups.len(), 2);
    assert_eq!(dups.iter().map(|n| n.unresolved_calls).sum::<u32>(), 2);
}

/// I1: a base that resolves to nothing (a star import that does not provide
/// it) keeps the same-name set and never dangles.
#[test]
fn unresolvable_base_keeps_the_legacy_set() {
    let (_o, off) = index_files(HONESTY_FILES, false);
    let (o, on) = index_files(HONESTY_FILES, true);
    let caller = "pkg::bases::StarBase::__init__";
    assert!(!callees(&off, caller).is_empty());
    assert_eq!(callees(&on, caller), callees(&off, caller));
    assert_eq!(node(&on, caller).external_calls, 0);
    assert!(o.stats.precision.unknown_base.fallback >= 1);
}

/// I9: an in-repo call base is unknown (legacy); an out-of-repo call base and
/// a base bound to a module-level value are out-of-repo method providers.
#[test]
fn dynamic_bases_follow_their_provenance() {
    let (_o, off) = index_files(HONESTY_FILES, false);
    let (o, on) = index_files(HONESTY_FILES, true);
    let dyn_init = "pkg::bases::Dyn::__init__";
    assert_eq!(callees(&on, dyn_init), callees(&off, dyn_init));
    assert_eq!(node(&on, dyn_init).external_calls, 0);
    for open in ["pkg::bases::Decl::__init__", "pkg::bases::Model::__init__"] {
        assert!(callees(&on, open).is_empty(), "{open}");
        assert_eq!(node(&on, open).external_calls, 1, "{open}");
    }
    assert_eq!(o.stats.precision.open_nonclass_bases, 2);
    assert_eq!(o.stats.precision.unknown_bases, 2);
}

/// I3: a closed lookup that finds nothing (an attribute set from outside the
/// class holding a function) keeps the same-name set.
#[test]
fn closed_lookup_with_no_target_keeps_the_legacy_set() {
    let (_o, off) = index_files(HONESTY_FILES, false);
    let (o, on) = index_files(HONESTY_FILES, true);
    let fire = "pkg::bases::Holder::fire";
    assert!(callees(&off, fire).contains("pkg::bases::Other::handler"));
    assert_eq!(callees(&on, fire), callees(&off, fire));
    // The store through `h` makes `handler` an instance attribute, so the
    // lookup is unknown rather than closed; either way it keeps the set.
    assert!(o.stats.precision.unknown_base.fallback >= 1);
}

/// I5: every narrowed target is a same-language method or function with the
/// call's short name, so narrowing only removes same-name candidates.
#[test]
fn narrowed_targets_are_same_name_candidates() {
    for files in ALL_FIXTURES {
        let (_o, idx) = index_files(files, true);
        let by_id: BTreeMap<_, _> = idx.nodes.iter().map(|n| (n.id, n)).collect();
        let mut seen = 0;
        for e in idx.edges.iter().filter(|e| e.rule.starts_with("recv-")) {
            let (src, dst) = (by_id[&e.src], by_id[&e.dst]);
            assert!(
                matches!(dst.kind, SymbolKind::Method | SymbolKind::Function),
                "{}",
                dst.fqn
            );
            assert_eq!(dst.lang, src.lang);
            seen += 1;
        }
        // The honesty and factory fixtures are all-legacy by design.
        assert!(
            seen > 0 || [HONESTY_FILES, FACTORY_FILES, CALLBACKS_FILES, CTOR_FILES].contains(files)
        );
    }
    // Every narrowed site's targets share the site's method name: compare
    // against the flag-off same-name set at the same call site.
    let sites = |idx: &GraphIndex, recv_only: bool| {
        let mut m: BTreeMap<_, BTreeSet<String>> = BTreeMap::new();
        for e in &idx.edges {
            if e.site_id.is_some() && (!recv_only || e.rule.starts_with("recv-")) {
                m.entry(e.site_id)
                    .or_default()
                    .insert(idx.fqn_of(e.dst).to_owned());
            }
        }
        m
    };
    for files in ALL_FIXTURES {
        let (_o, off) = index_files(files, false);
        let (_o, on) = index_files(files, true);
        let off_sites = sites(&off, false);
        for (site, targets) in sites(&on, true) {
            let legacy = &off_sites[&site];
            assert!(targets.is_subset(legacy), "{targets:?} ⊄ {legacy:?}");
        }
    }
}

/// I8: `typed_away_in` counts the narrowed same-name sites that excluded a def.
#[test]
fn backward_counts_are_narrowed_sites_minus_those_that_kept_the_def() {
    let (_o, idx) = index(true);
    // Narrowed `validate` sites: Base.save (self), Mixin.run (self),
    // Child.validate (super), use_annotated (local), use_ctor (local) and
    // Child.go (field). Four exclude Unrelated.validate; Mixin.run, use_ctor
    // and Child.go exclude Base.validate.
    assert_eq!(
        node(&idx, "pkg::widgets::Unrelated::validate")
            .narrowing
            .typed_away_in,
        4
    );
    assert_eq!(
        node(&idx, "pkg::base::Base::validate")
            .narrowing
            .typed_away_in,
        3
    );
    assert_eq!(node(&idx, "pkg::base::Base::save").narrowing.typed_out, 1);
    let (_o, off) = index(false);
    assert!(off.nodes.iter().all(|n| n.narrowing == Default::default()));
}

/// I7 (in-test half): with the flag off nothing is narrowed or counted.
#[test]
fn flag_off_produces_no_narrowing() {
    for files in ALL_FIXTURES {
        let (o, idx) = index_files(files, false);
        assert!(idx.edges.iter().all(|e| !e.rule.starts_with("recv-")));
        assert!(idx.nodes.iter().all(|n| n.external_calls == 0));
        assert_eq!(o.stats.precision, Default::default());
    }
}

/// D2: package re-exports, a sys.path root below the repo root, an in-repo
/// module shadowing a stdlib name, and `import a.b` binding `a`.
#[test]
fn module_identity_resolves_exactly() {
    let (_o, idx) = index_files(IDENTITY_FILES, true);
    let used = callees(&idx, "pkg::user::use");
    assert!(
        used.contains("pkg::forms::fields::Field::clean"),
        "{used:?}"
    );
    assert!(!used.contains("pkg::other::Field::clean"), "{used:?}");
    // Only `os.path.join` is proven external; `json` names the in-repo module.
    assert_eq!(node(&idx, "pkg::user::use").external_calls, 1);
    assert_eq!(
        callees(&idx, "tests::app::test_models::t"),
        set(&["tests::app::models::M::m"])
    );
    assert_eq!(node(&idx, "tests::app::test_models::t").external_calls, 0);
}

/// No facts, no claim: a language whose frontend emits no receiver-typing facts
/// is never narrowed.
#[test]
fn languages_without_receiver_facts_are_unchanged() {
    let ts = "class A {\n  m() { return 1; }\n}\nclass B {\n  m() { return 2; }\n}\nexport function f(x: A) {\n  return x.m();\n}\n";
    let files: &[(&str, &str)] = &[("src/app.ts", ts)];
    let (_o, off) = index_files(files, false);
    let (o, on) = index_files(files, true);
    assert_eq!(on.edges, off.edges);
    assert_eq!(on.nodes, off.nodes);
    assert_eq!(o.stats.precision, Default::default());
}

/// I6: the link is a pure function of the input set: reversing the file order
/// yields the same nodes (narrowing counters included), edges, dangling refs
/// and stats. Candidate-group ids are numbered in input order by the link
/// itself, with or without narrowing, so edges are compared without them.
#[test]
fn narrowing_is_independent_of_input_order() {
    let frontend = cgx_lang_python::PythonFrontend;
    let mut facts = Vec::new();
    for (root, files) in [
        ("main", MAIN_FILES),
        ("honesty", HONESTY_FILES),
        ("typing", TYPING_FILES),
    ] {
        for (path, text) in files.iter().filter(|(p, _)| p.ends_with(".py")) {
            let ctx = FileCtx::new(format!("{root}/{path}"), format!("oid{}", facts.len()));
            let mut f = frontend.extract(text.as_bytes(), &ctx).unwrap();
            f.canonicalize();
            facts.push((ctx.path.as_str().to_owned(), f));
        }
    }
    let inputs: Vec<_> = facts
        .iter()
        .enumerate()
        .map(|(i, (p, f))| cgx_resolve::FileInput::new(format!("oid{i}"), p.clone(), "python", f))
        .collect();
    let opts = cgx_resolve::LinkOpts {
        receiver_narrowing: true,
        ..Default::default()
    };
    let forward = cgx_resolve::link(&inputs, &opts);
    let mut reversed_inputs = inputs.clone();
    reversed_inputs.reverse();
    let reversed = cgx_resolve::link(&reversed_inputs, &opts);
    assert!(forward
        .edges
        .iter()
        .any(|e| e.edge.rule.starts_with("recv-")));
    let edges = |g: &cgx_resolve::ResolvedGraph| -> BTreeSet<_> {
        g.edges
            .iter()
            .map(|e| {
                let e = &e.edge;
                (
                    e.src,
                    e.dst,
                    e.kind,
                    e.confidence,
                    e.rule.clone(),
                    e.site_id,
                    e.candidate_group.is_some(),
                )
            })
            .collect()
    };
    assert_eq!(forward.nodes, reversed.nodes);
    assert_eq!(edges(&forward), edges(&reversed));
    assert_eq!(forward.edges.len(), reversed.edges.len());
    assert_eq!(forward.unresolved, reversed.unresolved);
    assert_eq!(forward.precision, reversed.precision);
}

const CLOSURE_PY: &str = r#"
class Base:
    def validate(self):
        return 1

    def outer(self):
        def inner():
            return self.validate()
        return inner()


class Unrelated:
    def validate(self):
        return 2


def factory(items):
    Base = items[0]

    def inner():
        return Base.validate()
    return inner


def make():
    class LocalBase:
        def validate(self):
            return 3

    class LocalChild(LocalBase):
        def go(self):
            return self.validate()

    return LocalChild
"#;

/// A name with no binding in the calling closure is looked up in the enclosing
/// callables first: an outer method's `self` is still the receiver, and an
/// outer function's local shadows the module-level class of that name.
#[test]
fn closures_see_the_enclosing_callables_bindings() {
    let files: &[(&str, &str)] = &[("pkg/__init__.py", ""), ("pkg/closure.py", CLOSURE_PY)];
    let (_o, idx) = index_files(files, true);
    assert_eq!(
        callees(&idx, "pkg::closure::Base::outer::inner"),
        set(&["pkg::closure::Base::validate"])
    );
    // A class defined in the function is a class, not an opaque local.
    assert_eq!(
        callees(&idx, "pkg::closure::make::LocalChild::go"),
        set(&["pkg::closure::make::LocalBase::validate"])
    );
    let shadowed = callees(&idx, "pkg::closure::factory::inner");
    assert!(
        shadowed.contains("pkg::closure::Unrelated::validate"),
        "{shadowed:?}"
    );
    assert!(
        shadowed.contains("pkg::closure::Base::validate"),
        "{shadowed:?}"
    );
}

/// Receiver shapes whose true targets narrowing must keep or must not claim.
const SHAPES_FILES: &[(&str, &str)] = &[
    ("app/__init__.py", ""),
    (
        "app/a_base.py",
        "class ABase:\n    def run(self):\n        return self.step()\n\n    def step(self):\n        return 0\n",
    ),
    (
        "app/a_impl.py",
        "try:\n    from app.a_base import ABase\nexcept ImportError:\n    from extlib import ABase\n\n\nclass AImpl(ABase):\n    def step(self):\n        return 1\n",
    ),
    (
        "app/a2.py",
        "class BaseMgr:\n    @classmethod\n    def from_qs(cls, qs):\n        return type(\"M\", (cls,), {})\n\n    def all(self):\n        return self.get_qs()\n\n    def get_qs(self):\n        return 0\n\n\nclass Mgr(BaseMgr.from_qs(object)):\n    pass\n\n\nclass PublishedMgr(Mgr):\n    def get_qs(self):\n        return 1\n\n\ndef make_related(superclass):\n    class RelatedMgr(superclass):\n        def get_qs(self):\n            return 2\n    return RelatedMgr\n",
    ),
    ("app/b_compat.py", "from app.b_real import Real\n\nCompat = Real\n"),
    (
        "app/b_real.py",
        "class Real:\n    def go(self):\n        return self.hook()\n\n    def hook(self):\n        return 0\n",
    ),
    (
        "app/b_user.py",
        "from app.b_compat import Compat\n\n\nclass User(Compat):\n    def hook(self):\n        return 1\n\n    def other(self):\n        return self.go()\n",
    ),
    (
        "app/c_super.py",
        "class A1:\n    def m(self):\n        return 1\n\n\nclass B1(A1):\n    def m(self):\n        return 2\n\n\nclass C1(B1):\n    def m(self):\n        return super(B1, self).m()\n",
    ),
    (
        "app/d_meta.py",
        "class Meta(type):\n    def __call__(cls, *a, **k):\n        cls.validate_args(a)\n        return super().__call__(*a, **k)\n\n\nclass Model(metaclass=Meta):\n    @classmethod\n    def validate_args(cls, a):\n        return a\n",
    ),
    (
        "app/e_proxy.py",
        "import threading\n\n\nclass Proxy(threading.local):\n    def __init__(self, target):\n        self._t = target\n\n    def __getattr__(self, n):\n        return getattr(self._t, n)\n\n    def call(self):\n        return self.process()\n\n\nclass Worker:\n    def process(self):\n        return 1\n",
    ),
    (
        "app/f_attr.py",
        "import threading\n\n\nclass Runner(threading.Thread):\n    def __init__(self, w):\n        super().__init__()\n        self.handle = w.handle\n\n    def run(self):\n        return self.handle()\n\n\nclass Job:\n    def handle(self):\n        return 1\n",
    ),
    ("app/fastjson.py", "def dumps(x):\n    return x\n"),
    (
        "app/g_scope.py",
        "class Inner:\n    def ping(self):\n        return 1\n\n\nclass Outer:\n    class Inner:\n        def ping(self):\n            return 2\n\n    def f(self):\n        return Inner.ping(self)\n",
    ),
    (
        "app/i_local.py",
        "import yaml\n\n\nclass Thing:\n    def dump(self):\n        return 0\n\n\ndef f():\n    from app import yaml_compat as yaml\n    return yaml.dump(1)\n",
    ),
    (
        "app/j_cond.py",
        "try:\n    import ujson as json\nexcept ImportError:\n    from app import fastjson as json\n\n\ndef f():\n    return json.dumps(1)\n",
    ),
    (
        "app/m_mro.py",
        "class Root:\n    def save(self):\n        return 0\n\n\nclass MixA(Root):\n    def save(self):\n        return super().save()\n\n\nclass MixB(Root):\n    def save(self):\n        return 1\n\n\nclass Final(MixA, MixB):\n    pass\n\n\nclass LogMixin:\n    def save(self):\n        return super().save()\n\n\nclass Logged(LogMixin, MixB):\n    pass\n",
    ),
    (
        "app/p_rebind.py",
        "def wrap(cls):\n    class Wrapped(cls):\n        def tick(self):\n            return 9\n    return Wrapped\n\n\n@wrap\nclass Clock:\n    def start(self):\n        return self.tick()\n\n    def tick(self):\n        return 0\n",
    ),
    (
        "app/q_kinds.py",
        "class Q:\n    @property\n    def size(self):\n        return 1\n\n    @staticmethod\n    def helper(x):\n        return x\n\n    @classmethod\n    def build(cls):\n        return cls.helper(1)\n\n    def use(self):\n        return Q.helper(2)\n",
    ),
    ("app/yaml_compat.py", "def dump(x):\n    return x\n"),
    ("tests/k_user.py", "import logging\n\n\ndef f():\n    return logging.getLogger(\"x\")\n"),
    ("tests/logging/__init__.py", "def getLogger(n):\n    return n\n"),
];

/// A nested project root, the super cone-exclusion case, and `import util`
/// next to `from . import util`.
const ROOTS_FILES: &[(&str, &str)] = &[
    ("app/__init__.py", ""),
    (
        "app/k2.py",
        "import util\nfrom . import util as u2\n\n\ndef f():\n    return util.ping() + u2.ping()\n",
    ),
    (
        "app/sx.py",
        "class Field:\n    def __init__(self):\n        pass\n\n\nclass BinaryField(Field):\n    def __init__(self):\n        super().__init__()\n\n\nclass Sub(BinaryField):\n    def __init__(self):\n        super().__init__()\n",
    ),
    ("app/util.py", "def ping():\n    return 1\n"),
    (
        "main.py",
        "import sub.lib.helper as hp\n\n\ndef f():\n    return hp.H.run(None)\n\n\nclass X(hp.H):\n    def go(self):\n        return self.run()\n",
    ),
    ("sub/lib/helper.py", "class H:\n    def run(self):\n        return 1\n"),
    ("sub/pyproject.toml", "[project]\nname = \"sub\"\n"),
];

const FACTORY_FILES: &[(&str, &str)] = &[
    ("app/__init__.py", ""),
    (
        "app/w.py",
        "from six import with_metaclass\n\n\nclass Meta(type):\n    pass\n\n\nclass Core:\n    def ping(self):\n        return 1\n\n\nclass Foo(with_metaclass(Meta, Core)):\n    def go(self):\n        return self.ping()\n",
    ),
];

fn assert_keeps(idx: &GraphIndex, caller: &str, targets: &[&str]) {
    let got = callees(idx, caller);
    for t in targets {
        assert!(got.contains(*t), "{caller} lost {t}: {got:?}");
    }
}

/// Subclasses reached through an unresolvable base (a factory call, a
/// parameter, a conflicting import) are still candidates of the cone.
#[test]
fn floating_subclasses_stay_in_the_cone() {
    let (o, idx) = index_files(SHAPES_FILES, true);
    assert_keeps(
        &idx,
        "app::a2::BaseMgr::all",
        &[
            "app::a2::BaseMgr::get_qs",
            "app::a2::PublishedMgr::get_qs",
            "app::a2::make_related::RelatedMgr::get_qs",
        ],
    );
    assert_keeps(
        &idx,
        "app::a_base::ABase::run",
        &["app::a_base::ABase::step", "app::a_impl::AImpl::step"],
    );
    assert_keeps(
        &idx,
        "app::p_rebind::Clock::start",
        &["app::p_rebind::wrap::Wrapped::tick"],
    );
    assert!(o.stats.precision.floating_classes >= 4);
}

/// A field assigned through the receiver, or a customised attribute lookup,
/// makes a miss unknown instead of proven external.
#[test]
fn receiver_fields_and_getattr_keep_the_legacy_set() {
    let (_o, off) = index_files(SHAPES_FILES, false);
    let (_o, on) = index_files(SHAPES_FILES, true);
    for caller in ["app::f_attr::Runner::run", "app::e_proxy::Proxy::call"] {
        assert!(!callees(&off, caller).is_empty(), "{caller}");
        assert_eq!(callees(&on, caller), callees(&off, caller), "{caller}");
        assert_eq!(node(&on, caller).external_calls, 0, "{caller}");
    }
}

/// A module-level alias of a class is that class.
#[test]
fn class_aliases_resolve_to_the_class() {
    let (_o, idx) = index_files(SHAPES_FILES, true);
    assert_eq!(
        callees(&idx, "app::b_user::User::other"),
        set(&["app::b_real::Real::go"])
    );
    assert_eq!(
        callees(&idx, "app::b_real::Real::go"),
        set(&["app::b_real::Real::hook", "app::b_user::User::hook"])
    );
}

/// `super(X, self)` naming another class, and `cls` in a metaclass, are not
/// narrowed as the enclosing class's receiver.
#[test]
fn explicit_super_and_metaclass_receivers_keep_the_legacy_set() {
    let (_o, off) = index_files(SHAPES_FILES, false);
    let (_o, on) = index_files(SHAPES_FILES, true);
    assert_keeps(&on, "app::c_super::C1::m", &["app::c_super::A1::m"]);
    assert_keeps(
        &on,
        "app::d_meta::Meta::__call__",
        &["app::d_meta::Model::validate_args"],
    );
    assert_eq!(
        callees(&on, "app::c_super::C1::m"),
        callees(&off, "app::c_super::C1::m")
    );
}

/// A method body does not see its class body's names.
#[test]
fn class_scope_names_do_not_leak_into_methods() {
    let (_o, idx) = index_files(SHAPES_FILES, true);
    assert_eq!(
        callees(&idx, "app::g_scope::Outer::f"),
        set(&["app::g_scope::Inner::ping"])
    );
}

/// Cooperative MRO, local and conditional imports, and method kinds.
#[test]
fn mro_local_imports_and_method_kinds() {
    let (_o, off) = index_files(SHAPES_FILES, false);
    let (_o, on) = index_files(SHAPES_FILES, true);
    assert_eq!(
        callees(&on, "app::m_mro::MixA::save"),
        set(&["app::m_mro::MixB::save", "app::m_mro::Root::save"])
    );
    assert_eq!(
        callees(&on, "app::m_mro::LogMixin::save"),
        set(&["app::m_mro::MixB::save"])
    );
    for caller in ["app::i_local::f", "app::j_cond::f"] {
        assert_eq!(callees(&on, caller), callees(&off, caller), "{caller}");
        assert_eq!(node(&on, caller).external_calls, 0, "{caller}");
    }
    assert_eq!(node(&on, "tests::k_user::f").external_calls, 0);
    assert_eq!(
        callees(&on, "app::q_kinds::Q::build"),
        set(&["app::q_kinds::Q::helper"])
    );
    assert_eq!(
        callees(&on, "app::q_kinds::Q::use"),
        set(&["app::q_kinds::Q::helper"])
    );
}

/// A nested project root is in repo; `super()` never reaches the class's own
/// subclasses; a plain and a relative import of one module both stay in repo.
#[test]
fn nested_roots_and_super_cone_exclusion() {
    let (_o, idx) = index_files(ROOTS_FILES, true);
    assert_eq!(callees(&idx, "main::f"), set(&["lib::helper::H::run"]));
    assert_eq!(callees(&idx, "main::X::go"), set(&["lib::helper::H::run"]));
    assert_eq!(
        callees(&idx, "app::sx::BinaryField::__init__"),
        set(&["app::sx::Field::__init__"])
    );
    assert_eq!(
        callees(&idx, "app::sx::Sub::__init__"),
        set(&["app::sx::BinaryField::__init__"])
    );
    assert_eq!(node(&idx, "app::k2::f").external_calls, 0);
}

/// A class factory given in-repo classes can splice them into the bases.
#[test]
fn factory_bases_with_class_arguments_keep_the_legacy_set() {
    let (_o, idx) = index_files(FACTORY_FILES, true);
    assert_keeps(&idx, "app::w::Foo::go", &["app::w::Core::ping"]);
    assert_eq!(node(&idx, "app::w::Foo::go").external_calls, 0);
}

/// Order-sensitive structures: same-FQN classes with different bases in two
/// files (one module, two files), and a cyclic lattice.
const ORDER_FILES: &[(&str, &str)] = &[
    (
        "pkg/dup.py",
        "class A:\n    def m(self):\n        return 1\n\n\nclass D(A):\n    def run(self):\n        return self.m()\n",
    ),
    (
        "src/pkg/dup.py",
        "class B:\n    def m(self):\n        return 2\n\n\nclass D(B):\n    def go(self):\n        return super().m()\n",
    ),
    (
        "pkg/cyc.py",
        "class P(Q):\n    def m(self):\n        return self.n()\n\n\nclass Q(P):\n    def n(self):\n        return super().m()\n",
    ),
];

#[test]
fn order_sensitive_lattices_are_order_independent() {
    let frontend = cgx_lang_python::PythonFrontend;
    let facts: Vec<(String, cgx_frontend::FileFacts)> = ORDER_FILES
        .iter()
        .chain(SHAPES_FILES.iter())
        .filter(|(p, _)| p.ends_with(".py"))
        .enumerate()
        .map(|(i, (path, text))| {
            let ctx = FileCtx::new(*path, format!("oid{i}"));
            let mut f = frontend.extract(text.as_bytes(), &ctx).unwrap();
            f.canonicalize();
            (path.to_string(), f)
        })
        .collect();
    let inputs: Vec<_> = facts
        .iter()
        .enumerate()
        .map(|(i, (p, f))| cgx_resolve::FileInput::new(format!("oid{i}"), p.clone(), "python", f))
        .collect();
    let opts = cgx_resolve::LinkOpts {
        receiver_narrowing: true,
        ..Default::default()
    };
    let edges = |g: &cgx_resolve::ResolvedGraph| -> BTreeSet<_> {
        g.edges
            .iter()
            .map(|e| {
                let e = &e.edge;
                (
                    e.src,
                    e.dst,
                    e.kind,
                    e.confidence,
                    e.rule.clone(),
                    e.site_id,
                )
            })
            .collect()
    };
    let forward = cgx_resolve::link(&inputs, &opts);
    for order in [inputs.iter().rev().cloned().collect::<Vec<_>>(), {
        let mut v = inputs.clone();
        v.rotate_left(1);
        v
    }] {
        let other = cgx_resolve::link(&order, &opts);
        assert_eq!(forward.nodes, other.nodes);
        assert_eq!(edges(&forward), edges(&other));
        assert_eq!(forward.unresolved, other.unresolved);
        assert_eq!(forward.precision, other.precision);
    }
    assert!(forward.precision.fqn_collisions >= 1);
}

/// Factory values given classes, a function-local class name reassigned in
/// the same function, and a floating sibling base after the class in the MRO.
const FACTORY_VALUE_FILES: &[(&str, &str)] = &[
    ("app/__init__.py", ""),
    (
        "app/v1.py",
        "from sqlalchemy.orm import declarative_base\n\n\nclass Core:\n    def ping(self):\n        return 1\n\n\nBase = declarative_base(cls=Core)\n\n\nclass M(Base):\n    def go(self):\n        return self.ping()\n",
    ),
    (
        "app/v2.py",
        "from six import with_metaclass\n\n\nclass Meta2(type):\n    pass\n\n\nclass Core2:\n    def pong(self):\n        return 1\n\n\nCompat2 = with_metaclass(Meta2, Core2)\n\n\nclass N(Compat2):\n    def go(self):\n        return self.pong()\n",
    ),
    (
        "app/v3.py",
        "class Other:\n    def hook(self):\n        return 9\n\n\ndef make(mixin):\n    class Base:\n        def hook(self):\n            return 0\n    if mixin:\n        Base = mixin\n    class Sub(Base):\n        def go(self):\n            return self.hook()\n    return Sub\n",
    ),
    (
        "app/v4.py",
        "def make_c():\n    return C\n\n\ndef make_mixin():\n    return object\n\n\nclass A:\n    def m(self):\n        return 1\n\n\nclass C(A):\n    def m(self):\n        return super().m()\n\n\nclass G(make_mixin()):\n    def m(self):\n        return 2\n\n\nclass F(make_c(), G):\n    pass\n",
    ),
];

#[test]
fn factory_values_reassigned_locals_and_floating_siblings_keep_their_targets() {
    let (_o, idx) = index_files(FACTORY_VALUE_FILES, true);
    for (caller, target) in [
        ("app::v1::M::go", "app::v1::Core::ping"),
        ("app::v2::N::go", "app::v2::Core2::pong"),
    ] {
        assert_keeps(&idx, caller, &[target]);
        assert_eq!(node(&idx, caller).external_calls, 0, "{caller}");
    }
    assert_keeps(&idx, "app::v3::make::Sub::go", &["app::v3::Other::hook"]);
    assert_keeps(&idx, "app::v4::C::m", &["app::v4::A::m", "app::v4::G::m"]);
}

// --- Intraprocedural receiver typing (Python) -------------------------------

#[test]
fn typed_locals_and_params_are_narrowed() {
    let (o, idx) = index(true);
    // Annotation: the declared class's cone.
    assert_eq!(
        callees(&idx, "pkg::svc::use_annotated"),
        set(&[
            "pkg::base::Base::validate",
            "pkg::svc::Child::validate",
            "pkg::widgets::Widget::validate"
        ])
    );
    // Constructor: the exact class.
    assert_eq!(
        callees(&idx, "pkg::svc::use_ctor"),
        set(&[
            "pkg::widgets::Unrelated",
            "pkg::widgets::Unrelated::validate"
        ])
    );
    // Rebound by a `for` target: poisoned, legacy set kept.
    assert!(callees(&idx, "pkg::svc::use_rebound").contains("pkg::base::Base::validate"));
    assert_eq!(o.stats.precision.typed_local.sites, 2);
}

#[test]
fn constructor_and_field_receivers() {
    let (o, idx) = index(true);
    let g = callees(&idx, "pkg::svc::Child::go");
    // `Unrelated().make()`: the constructed class's method only.
    assert!(g.contains("pkg::widgets::Unrelated::make"), "{g:?}");
    assert!(!g.contains("pkg::widgets::Widget::make"), "{g:?}");
    // `self.helper` is only ever `Unrelated()`.
    assert!(g.contains("pkg::widgets::Unrelated::validate"), "{g:?}");
    assert!(!g.contains("pkg::base::Base::validate"), "{g:?}");
    let rules: BTreeSet<_> = idx
        .edges
        .iter()
        .filter(|e| idx.fqn_of(e.src) == "pkg::svc::Child::go")
        .map(|e| e.rule.as_str())
        .collect();
    assert!(
        rules.contains("recv-ctor") && rules.contains("recv-field"),
        "{rules:?}"
    );
    assert_eq!(o.stats.precision.ctor_call.sites, 1);
    assert_eq!(o.stats.precision.self_field.sites, 1);
}

const MODEL_PY: &str = r#"
class Base:
    def validate(self):
        return 1


class Sub(Base):
    def validate(self):
        return 2


class Unrelated:
    def validate(self):
        return 3

    def update(self, x):
        return 4

    def get(self, k):
        return 5


class MyDict(dict):
    def get(self, k):
        return 6


def make():
    return Base()
"#;

/// One function per binding form. Each binds `x` to a constructor first, so
/// only the poisoning form keeps it from narrowing.
const FORMS_PY: &str = r#"
from pkg.model import Base, Unrelated, make


def f_for(xs):
    x = Unrelated()
    for x in xs:
        pass
    return x.validate()


def f_with():
    x = Unrelated()
    with make() as x:
        pass
    return x.validate()


def f_match(v):
    x = Unrelated()
    match v:
        case [x]:
            pass
    return x.validate()


def f_tuple(p):
    x = Unrelated()
    x, y = p
    return x.validate()


def f_star(p):
    x = Unrelated()
    *x, y = p
    return x.validate()


def f_aug(y):
    x = Unrelated()
    x += y
    return x.validate()


def f_walrus():
    x = Unrelated()
    if (x := make()):
        pass
    return x.validate()


def f_global():
    global x
    x = Unrelated()
    return x.validate()


def f_import():
    x = Unrelated()
    from pkg import model as x
    return x.validate()


def f_def():
    x = Unrelated()

    def x():
        return 0
    return x.validate()


def f_del():
    x = Unrelated()
    del x
    x = Unrelated()
    return x.validate()


def f_comp(xs):
    x = Unrelated()
    return [x.validate() for x in xs]


def f_annotated():
    x: Base = make()
    return x.validate()


def f_except():
    try:
        return 0
    except Base as x:
        return x.validate()


def f_join(flag):
    x = Base()
    if flag:
        x = Unrelated()
    return x.validate()


def f_join_call():
    x = Base()
    x = make()
    return x.validate()
"#;

const EXT_PY: &str = r#"
import structlog
from structlog.typing import EventDict


def collect():
    imports = set()
    imports.update([1])
    return imports


def log(e: EventDict):
    return e.update({})


def log_dotted(e: structlog.typing.EventDict):
    return e.update({})


def use_dict(d: dict):
    return d.get("k")


def use_literal():
    d = {}
    return d.get("k")
"#;

const FIELDS_PY: &str = r#"
from pkg.model import Base, Unrelated, make


def factory(base):
    return base


class Holder:
    def __init__(self):
        self.helper = Unrelated()

    def run(self):
        return self.helper.validate()


class ClassAttr:
    helper = None

    def __init__(self):
        self.helper = Unrelated()

    def run(self):
        return self.helper.validate()


class FromCall:
    def __init__(self):
        self.helper = make()

    def run(self):
        return self.helper.validate()


class Store:
    def __init__(self):
        self.helper = Unrelated()

    def run(self):
        return self.helper.validate()


class Dynamic(factory(Store)):
    def __init__(self):
        self.helper = Base()
"#;

const TYPING_FILES: &[(&str, &str)] = &[
    ("pkg/__init__.py", ""),
    ("pkg/model.py", MODEL_PY),
    ("pkg/forms.py", FORMS_PY),
    ("pkg/ext.py", EXT_PY),
    ("pkg/fields.py", FIELDS_PY),
];

const VALIDATES: &[&str] = &[
    "pkg::model::Base::validate",
    "pkg::model::Sub::validate",
    "pkg::model::Unrelated::validate",
];

fn validate_targets(idx: &GraphIndex, caller: &str) -> BTreeSet<String> {
    callees(idx, caller)
        .into_iter()
        .filter(|c| c.ends_with("::validate"))
        .collect()
}

/// I4: every binding form the facts do not type poisons the name, so the
/// site keeps exactly the flag-off same-name set.
#[test]
fn every_untyped_binding_form_keeps_the_legacy_set() {
    let (_o, off) = index_files(TYPING_FILES, false);
    let (_o, on) = index_files(TYPING_FILES, true);
    let forms = [
        "f_for",
        "f_with",
        "f_match",
        "f_tuple",
        "f_star",
        "f_aug",
        "f_walrus",
        "f_global",
        "f_import",
        "f_def",
        "f_del",
        "f_comp",
        "f_join_call",
    ];
    for f in forms {
        let caller = format!("pkg::forms::{f}");
        assert_eq!(validate_targets(&off, &caller), set(VALIDATES), "{f} off");
        assert_eq!(validate_targets(&on, &caller), set(VALIDATES), "{f} on");
        assert_eq!(callees(&on, &caller), callees(&off, &caller), "{f}");
        assert!(
            on.edges
                .iter()
                .filter(|e| on.fqn_of(e.src) == caller)
                .all(|e| !e.rule.starts_with("recv-")),
            "{f}"
        );
    }
}

/// Annotated locals and `except` targets are declared bounds: the declared
/// class's cone.
#[test]
fn declared_bindings_are_typed() {
    let (_o, idx) = index_files(TYPING_FILES, true);
    let cone = set(&["pkg::model::Base::validate", "pkg::model::Sub::validate"]);
    assert_eq!(validate_targets(&idx, "pkg::forms::f_annotated"), cone);
    assert_eq!(validate_targets(&idx, "pkg::forms::f_except"), cone);
}

/// Flow-insensitive join: every constructor that reaches `x`, each exact.
#[test]
fn rebinding_joins_the_constructed_classes() {
    let (_o, idx) = index_files(TYPING_FILES, true);
    assert_eq!(
        validate_targets(&idx, "pkg::forms::f_join"),
        set(&[
            "pkg::model::Base::validate",
            "pkg::model::Unrelated::validate"
        ])
    );
}

/// Builtin and out-of-repo receiver types have no in-repo target when no
/// in-repo class that can derive from an out-of-repo class defines the method
/// (an exact literal or builtin value: no subclass at all): a counted
/// external call, no edge.
#[test]
fn external_typed_receivers_dangle() {
    let (_o, off) = index_files(TYPING_FILES, false);
    let (o, on) = index_files(TYPING_FILES, true);
    // `use_literal`'s dict is exactly a dict, never an in-repo subclass.
    for (f, m) in [
        ("collect", "update"),
        ("log", "update"),
        ("log_dotted", "update"),
        ("use_literal", "get"),
    ] {
        let caller = format!("pkg::ext::{f}");
        assert!(callees(&off, &caller).contains(&format!("pkg::model::Unrelated::{m}")));
        assert!(callees(&on, &caller).is_empty(), "{f}");
        assert_eq!(node(&on, &caller).external_calls, 1, "{f}");
    }
    assert!(o.stats.precision.typed_local.dangling >= 4);
}

/// An out-of-repo type with an in-repo subclass: the subclass's override is
/// the in-repo target.
#[test]
fn external_type_keeps_in_repo_subclass_overrides() {
    let (_o, idx) = index_files(TYPING_FILES, true);
    assert_eq!(
        callees(&idx, "pkg::ext::use_dict"),
        set(&["pkg::model::MyDict::get"])
    );
    let e = idx
        .edges
        .iter()
        .find(|e| idx.fqn_of(e.src) == "pkg::ext::use_dict")
        .unwrap();
    assert_eq!(e.rule, "recv-local");
    // The lookup is open (a plain `dict` has no in-repo `get`), but the site
    // is not dangled; like every narrowed site it is reported through
    // `typed_out`, so its contract is `under` (`receiver-narrowed`). An open
    // narrowed site and a closed one are reported alike by design.
    let n = node(&idx, "pkg::ext::use_dict");
    assert_eq!(n.narrowing.typed_out, 1);
    assert_eq!(n.external_calls, 0);
}

/// Instance fields: constructor stores narrow, including those of a class
/// whose base cannot be resolved (it may be a subclass); a class-level
/// attribute of that name or an untyped store keeps the same-name set.
#[test]
fn field_receivers_follow_their_stores() {
    let (_o, off) = index_files(TYPING_FILES, false);
    let (_o, on) = index_files(TYPING_FILES, true);
    // `Dynamic`'s base is unresolvable, so it may subclass either class: its
    // `Base()` store joins every `helper` field's type.
    for c in ["Holder", "Store"] {
        assert_eq!(
            validate_targets(&on, &format!("pkg::fields::{c}::run")),
            set(&[
                "pkg::model::Base::validate",
                "pkg::model::Unrelated::validate"
            ]),
            "{c}"
        );
    }
    for c in ["ClassAttr", "FromCall"] {
        let caller = format!("pkg::fields::{c}::run");
        assert_eq!(validate_targets(&on, &caller), set(VALIDATES), "{c}");
        assert_eq!(callees(&on, &caller), callees(&off, &caller), "{c}");
    }
}

// --- Go: VTA-lite ------------------------------------------------------------

const READERS_GO: &str = r#"
package app

type Reader interface {
	Read(p []byte) int
}

type File struct{}

func (f *File) Read(p []byte) int { return 1 }

type Buffer struct{}

func (b *Buffer) Read(p []byte) int { return 2 }

type Socket struct{}

func (s *Socket) Read(p []byte) int { return 3 }

func (s *Socket) Pump() int { return s.Read(nil) }

// Unexported and only called directly, but a parameter is never typed from
// call arguments.
func consume(r Reader) int { return r.Read(nil) }

func Run() int {
	f := &File{}
	b := &Buffer{}
	return consume(f) + consume(b)
}

// Exported: callers outside the repository may pass any Reader.
func Exported(r Reader) int { return r.Read(nil) }

// Allocations, a copy and a declared struct variable.
func Local() int {
	f := &File{}
	g := f
	var b Buffer
	return f.Read(nil) + g.Read(nil) + b.Read(nil)
}

// A variable declared with an interface type holds any implementation.
func Declared() int {
	var r Reader = &File{}
	return r.Read(nil)
}
"#;

const GO_FILES: &[(&str, &str)] = &[
    ("go.mod", "module example.com/app\n\ngo 1.22\n"),
    ("app/readers.go", READERS_GO),
];

fn index_go(narrow: bool) -> (IndexOutcome, GraphIndex) {
    index_files(GO_FILES, narrow)
}

fn reads(idx: &GraphIndex, caller_suffix: &str) -> BTreeSet<String> {
    let caller = idx
        .nodes
        .iter()
        .find(|n| n.fqn.ends_with(caller_suffix))
        .unwrap_or_else(|| panic!("no {caller_suffix}"))
        .fqn
        .clone();
    callees(idx, &caller)
        .into_iter()
        .filter(|c| c.ends_with("::Read"))
        .map(|c| c.rsplit("::").nth(1).unwrap().to_owned())
        .collect()
}

const ALL_READERS: &[&str] = &["(*Buffer)", "(*File)", "(*Socket)"];

#[test]
fn go_legacy_binds_every_reader() {
    let (_o, idx) = index_go(false);
    for f in ["::consume", "::(*Socket)::Pump", "::Exported", "::Local"] {
        assert_eq!(reads(&idx, f), set(ALL_READERS), "{f}");
    }
}

/// Intraprocedural only: receivers, allocations, copies and declared struct
/// variables are typed; parameters and interface-typed variables are not.
#[test]
fn go_vta_types_receivers_and_locals_only() {
    let (_o, off) = index_go(false);
    let (o, on) = index_go(true);
    // A receiver has exactly its declared type.
    assert_eq!(reads(&on, "::(*Socket)::Pump"), set(&["(*Socket)"]));
    // `f := &File{}`, `g := f` and `var b Buffer`.
    assert_eq!(reads(&on, "::Local"), set(&["(*Buffer)", "(*File)"]));
    assert_eq!(o.stats.precision.go_vta.sites, 4);
    // Parameters keep the same-name set, even when only File and Buffer are
    // passed; so do an exported parameter and an interface-typed variable.
    for f in ["::consume", "::Exported", "::Declared"] {
        assert_eq!(reads(&on, f), set(ALL_READERS), "{f}");
        let caller = on
            .nodes
            .iter()
            .find(|n| n.fqn.ends_with(f))
            .unwrap()
            .fqn
            .clone();
        assert_eq!(callees(&on, &caller), callees(&off, &caller), "{f}");
    }
    let rules: BTreeSet<_> = on
        .edges
        .iter()
        .filter(|e| on.fqn_of(e.src).ends_with("::Local"))
        .map(|e| e.rule.as_str())
        .collect();
    assert_eq!(rules, BTreeSet::from(["recv-vta"]));
}

// --- Review regressions ------------------------------------------------------

const REVIEW_MODELS_PY: &str = r#"
import logging
from collections import OrderedDict, Counter
from dataclasses import dataclass
from enum import IntEnum


class A:
    def m(self):
        pass


class B:
    def m(self):
        pass


class AppError(ValueError):
    def render(self):
        pass


class MyCounter(Counter):
    def tally(self):
        pass


class MyOD(OrderedDict):
    def lookup1(self, k):
        pass


class Level(IntEnum):
    LOW = 1

    def label(self):
        pass


class H(logging.StreamHandler):
    def emit2(self, r):
        pass


def make_base():
    return object


class Floating(make_base()):
    def fget(self, k):
        pass


class MyList(list):
    def push(self, v):
        pass


class Holder:
    def __init__(self):
        self.h = A()

    def run(self):
        self.h.m()


class Holder2:
    def __init__(self):
        self.h = A()

    @classmethod
    def make(cls):
        o = cls()
        o.h = B()
        return o

    def run(self):
        self.h.m()


@dataclass
class DC:
    h: object

    def __post_init__(self):
        self.h = A()

    def run(self):
        self.h.m()


class Ann:
    h: "B"

    def __init__(self):
        self.h = A()

    def run(self):
        self.h.m()


class Storage:
    def __new__(cls, *a):
        return object.__new__(S3)

    def save(self):
        pass


class S3(Storage):
    def save(self):
        pass


class Mixin:
    def setup(self):
        self.h = B()


class P:
    def __init__(self):
        self.h = A()

    def run(self):
        self.h.m()


class Both(P, Mixin):
    pass


class Other:
    def __init__(self):
        self.h = A()

    def run(self):
        self.h.m()


class Nested:
    def __init__(this):
        this.h = A()

        def later():
            this.h = B()

        later()

    def run(self):
        self.h.m()
"#;

const REVIEW_CASES_PY: &str = r#"
import logging
from typing import cast, TypeVar, Dict

from fx.models import A, B, Storage

T = TypeVar("T", bound=A)


def exc_transitive():
    try:
        pass
    except Exception as e:
        e.render()


def dict_transitive_counter(d: dict):
    d.tally()


def dict_transitive_od(d: dict):
    d.lookup1(1)


def dict_floating(d: dict):
    d.fget(1)


def int_enum(x: int):
    x.label()


def handler_transitive(h: logging.Handler):
    h.emit2(None)


def ctor_new():
    Storage().save()


def ctor_new_local():
    s = Storage()
    s.save()


def nonlocal_case():
    x = A()

    def inner():
        nonlocal x
        x = B()

    inner()
    x.m()


def lambda_case():
    x = A()
    f = lambda x: x.m()
    return f


def forward_ref(x: "A"):
    x.m()


def cast_case(z):
    y = cast(A, z)
    y.m()


def typevar_case(x: T):
    x.m()


def union_case(x: A | B):
    x.m()


def lying(x: A = None):
    x.m()


def dict_alias(d: Dict[str, int]):
    d.tally()


def object_param(o: object):
    o.m()


def pep695[A](x: A):
    x.m()
"#;

const REVIEW_OTHER_PY: &str = r#"
from fx.models import Other, B


def poke(o: Other):
    o.h = B()
"#;

const REVIEW_SHADOW_PY: &str = r#"
from fx.models import MyList

list = MyList


def shadowed_builtin():
    x = list()
    x.push(1)
"#;

const REVIEW_GO: &str = r#"
package fx

type File struct{}

func (f *File) Read() {}

type Buf struct{}

func (b Buf) Read() {}

type S struct{}

func (S) M() {}

type T struct{ S }

type U struct{}

func (U) M() {}

type Box[K any] struct{}

func (b *Box[K]) Read() {}

type Reader interface{ Read() }

func emb() {
	t := T{}
	t.M()
}

func clo() {
	f := &File{}
	g := func() { f = nil }
	g()
	f.Read()
}

func clo2() {
	var r Reader = &File{}
	f := &File{}
	func() { r = &Buf{}; _ = r }()
	f.Read()
}

func tsw(v any) {
	switch x := v.(type) {
	case *File:
		x.Read()
	}
}

func assert(v any) {
	x := v.(*File)
	x.Read()
}

func gen() {
	b := &Box[int]{}
	b.Read()
}

func multi() {
	f, g := &File{}, Buf{}
	f.Read()
	g.Read()
}

func multi2() (*File, Buf) { return nil, Buf{} }

func multi3() {
	f, g := multi2()
	f.Read()
	g.Read()
}

func (f *File) Self() {
	f.Read()
}

func ptr() {
	f := &File{}
	p := &f
	_ = p
	f.Read()
}

func tparam[File Reader](x File) {
	var y File
	y.Read()
}
"#;

const REVIEW_PY_FILES: &[(&str, &str)] = &[
    ("pyproject.toml", "[project]\nname = \"fx\"\n"),
    ("fx/__init__.py", ""),
    ("fx/models.py", REVIEW_MODELS_PY),
    ("fx/cases.py", REVIEW_CASES_PY),
    ("fx/other.py", REVIEW_OTHER_PY),
    ("fx/shadow.py", REVIEW_SHADOW_PY),
];

const REVIEW_GO_FILES: &[(&str, &str)] = &[
    ("go.mod", "module example.com/fx\n\ngo 1.22\n"),
    ("a.go", REVIEW_GO),
];

/// Every site keeps the true target that an earlier version dropped: an
/// out-of-repo annotation reaching in-repo classes through out-of-repo
/// hierarchies or an unresolvable base, an in-repo `__new__`, a PEP 695 type
/// parameter shadowing a class, and fields stored through other names.
#[test]
fn review_cases_keep_their_true_targets() {
    let (_o, on) = index_files(REVIEW_PY_FILES, true);
    for (caller, target) in [
        ("fx::cases::exc_transitive", "fx::models::AppError::render"),
        (
            "fx::cases::dict_transitive_counter",
            "fx::models::MyCounter::tally",
        ),
        ("fx::cases::dict_transitive_od", "fx::models::MyOD::lookup1"),
        ("fx::cases::dict_alias", "fx::models::MyCounter::tally"),
        ("fx::cases::int_enum", "fx::models::Level::label"),
        ("fx::cases::handler_transitive", "fx::models::H::emit2"),
        ("fx::cases::dict_floating", "fx::models::Floating::fget"),
        ("fx::cases::ctor_new", "fx::models::S3::save"),
        ("fx::cases::ctor_new_local", "fx::models::S3::save"),
        ("fx::cases::pep695", "fx::models::B::m"),
        ("fx::models::Holder2::run", "fx::models::B::m"),
        ("fx::models::Other::run", "fx::models::B::m"),
        ("fx::models::Nested::run", "fx::models::B::m"),
    ] {
        let got = callees(&on, caller);
        assert!(got.contains(target), "{caller}: {got:?}");
        assert_eq!(node(&on, caller).external_calls, 0, "{caller}");
    }
    let (_o, off) = index_files(REVIEW_GO_FILES, false);
    let (_o, on) = index_files(REVIEW_GO_FILES, true);
    assert_eq!(reads(&on, "::tparam"), reads(&off, "::tparam"));
    assert!(reads(&on, "::tparam").contains("Buf"));
}

const STORES_PY: &str = r#"
class A:
    def m(self):
        return 1


class B:
    def m(self):
        return 2


class C:
    def m(self):
        return 3


class Other:
    def __init__(self):
        self.p = A()

    def run(self):
        return self.p.m()


def poke(o: Other):
    o.p = B()


class Poisoned:
    def __init__(self):
        self.q = A()

    def run(self):
        return self.q.m()


def anywhere(x):
    x.q = C()


class Nested:
    def __init__(this):
        this.r = A()

        def later():
            this.r = B()

        later()

    def run(self):
        return self.r.m()
"#;

const STORES_FILES: &[(&str, &str)] = &[("pkg/__init__.py", ""), ("pkg/stores.py", STORES_PY)];

/// Attribute stores through other names: a typed base joins that class's
/// field; an untyped base makes the field name untyped everywhere; a nested
/// callable's store through the enclosing method's receiver is a field store.
#[test]
fn attribute_stores_through_other_names() {
    let (_o, off) = index_files(STORES_FILES, false);
    let (_o, on) = index_files(STORES_FILES, true);
    let m = |c: &str| format!("pkg::stores::{c}::m");
    let ab: BTreeSet<String> = [m("A"), m("B")].into_iter().collect();
    assert_eq!(callees(&on, "pkg::stores::Other::run"), ab);
    assert_eq!(callees(&on, "pkg::stores::Nested::run"), ab);
    assert_eq!(
        callees(&on, "pkg::stores::Poisoned::run"),
        callees(&off, "pkg::stores::Poisoned::run")
    );
    assert!(callees(&on, "pkg::stores::Poisoned::run").contains(&m("C")));
}

const CALLBACKS_PY: &str = r#"
import threading


class Job:
    def handle(self):
        return 1

    def poll(self):
        return 2


class Typed(threading.Thread):
    def go(self):
        return self.handle()


def wire(t: Typed, job: Job):
    t.handle = job.handle


class Untyped(threading.Thread):
    def go(self):
        return self.poll()


def wire_any(x, job: Job):
    x.poll = job.poll
"#;

const CALLBACKS_FILES: &[(&str, &str)] =
    &[("pkg/__init__.py", ""), ("pkg/callbacks.py", CALLBACKS_PY)];

/// An attribute stored through another name is an instance attribute: a call
/// through it is not proven external even when the class's lookup ends at an
/// out-of-repo base, whether the store's base is typed as the class or of
/// unknown type.
#[test]
fn stored_callables_are_attributes_not_external_methods() {
    let (_o, off) = index_files(CALLBACKS_FILES, false);
    let (_o, on) = index_files(CALLBACKS_FILES, true);
    for (caller, target) in [
        ("pkg::callbacks::Typed::go", "pkg::callbacks::Job::handle"),
        ("pkg::callbacks::Untyped::go", "pkg::callbacks::Job::poll"),
    ] {
        assert_eq!(callees(&on, caller), callees(&off, caller), "{caller}");
        assert!(callees(&on, caller).contains(target), "{caller}");
        assert_eq!(node(&on, caller).external_calls, 0, "{caller}");
    }
}

const CTOR_PY: &str = r#"
class A:
    def m(self):
        return 0


class B:
    def m(self):
        return 2


class C:
    def __new__(cls):
        return B()

    def m(self):
        return 1


class D(C):
    pass


def use_new():
    return C().m()


def use_new_local():
    c = C()
    return c.m()


def use_inherited_new():
    return D().m()


def type_param_ctor[A]():
    x = A()
    return x.m()


def type_param_anon[A]():
    return A().m()
"#;

const CTOR_FILES: &[(&str, &str)] = &[("pkg/__init__.py", ""), ("pkg/ctor.py", CTOR_PY)];

/// A class whose MRO has an in-repo `__new__` can construct any class, and a
/// PEP 695 type parameter called as a constructor names no class: these
/// constructor results are untyped and keep the same-name set.
#[test]
fn untyped_constructor_results_keep_the_legacy_set() {
    let (_o, off) = index_files(CTOR_FILES, false);
    let (_o, on) = index_files(CTOR_FILES, true);
    for f in [
        "use_new",
        "use_new_local",
        "use_inherited_new",
        "type_param_ctor",
        "type_param_anon",
    ] {
        let caller = format!("pkg::ctor::{f}");
        assert_eq!(callees(&on, &caller), callees(&off, &caller), "{f}");
        assert!(callees(&on, &caller).contains("pkg::ctor::B::m"), "{f}");
    }
}
