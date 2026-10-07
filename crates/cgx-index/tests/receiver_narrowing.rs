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
    let (a, ia) = index(true);
    let (b, ib) = index(true);
    assert_eq!(a.graph_key, b.graph_key);
    assert_eq!(ia.edges, ib.edges);
    assert_eq!(ia.nodes, ib.nodes);
}

/// I2: every dangling ref is counted on exactly one node, with or without
/// narrowing, including two defs that share an FQN in one file.
#[test]
fn every_dangling_ref_is_counted_once() {
    for files in [MAIN_FILES, HONESTY_FILES, IDENTITY_FILES] {
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
    assert!(o.stats.precision.self_cone.fallback >= 1);
}

/// I5: every narrowed target is a same-language method or function with the
/// call's short name, so narrowing only removes same-name candidates.
#[test]
fn narrowed_targets_are_same_name_candidates() {
    for files in [MAIN_FILES, HONESTY_FILES, IDENTITY_FILES] {
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
        assert!(seen > 0 || files == HONESTY_FILES);
    }
    // Every narrowed site's targets share the site's method name: compare
    // against the flag-off same-name set at the same call site.
    let (_o, off) = index(false);
    let (_o, on) = index(true);
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
    let off_sites = sites(&off, false);
    for (site, targets) in sites(&on, true) {
        let legacy = &off_sites[&site];
        assert!(targets.is_subset(legacy), "{targets:?} ⊄ {legacy:?}");
    }
}

/// I8: `typed_away_in` counts the narrowed same-name sites that excluded a def.
#[test]
fn backward_counts_are_narrowed_sites_minus_those_that_kept_the_def() {
    let (_o, idx) = index(true);
    // Narrowed `validate` sites: Base.save (self), Mixin.run (self) and
    // Child.validate (super). None keeps Unrelated.validate; only Mixin.run
    // excludes Base.validate.
    assert_eq!(
        node(&idx, "pkg::widgets::Unrelated::validate")
            .narrowing
            .typed_away_in,
        3
    );
    assert_eq!(
        node(&idx, "pkg::base::Base::validate")
            .narrowing
            .typed_away_in,
        1
    );
    assert_eq!(node(&idx, "pkg::base::Base::save").narrowing.typed_out, 1);
    let (_o, off) = index(false);
    assert!(off.nodes.iter().all(|n| n.narrowing == Default::default()));
}

/// I7 (in-test half): with the flag off nothing is narrowed or counted.
#[test]
fn flag_off_produces_no_narrowing() {
    for files in [MAIN_FILES, HONESTY_FILES, IDENTITY_FILES] {
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
    for (root, files) in [("main", MAIN_FILES), ("honesty", HONESTY_FILES)] {
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
