//! Receiver-typing facts emitted by the Python frontend: the binding-form
//! matrix, parameters, annotation normalization, class bases, anonymous
//! receivers and the canonical module path.

mod common;

use cgx_frontend::{
    AnonRoot, BaseExpr, FileCtx, FileFacts, LanguageFrontend, LiteralKind, ManifestInfo,
    ManifestKind, ParamKind, RefKind, TypeExpr, TypeFact, ValueSource,
};
use cgx_lang_python::PythonFrontend;
use common::extract;
use smallvec::SmallVec;

fn named(path: &str) -> TypeExpr {
    TypeExpr::Named {
        path: path.split('.').map(str::to_string).collect(),
        indirect: false,
    }
}

fn segs(path: &str) -> SmallVec<[String; 2]> {
    path.split('.').map(str::to_string).collect()
}

/// Facts of the function `f` defined at module level in `m.py`, with `body`
/// as its (already indented) body.
fn facts_of_f(body: &str) -> Vec<TypeFact> {
    let src = format!("def f(self):\n{body}\n");
    let facts = extract("m.py", &src);
    let func = format!("{}::f", facts.module.as_deref().expect("module set"));
    facts
        .type_facts
        .into_iter()
        .filter(|t| match t {
            TypeFact::Param { func: f, .. }
            | TypeFact::Bind { func: f, .. }
            | TypeFact::FieldBind { func: f, .. } => *f == func,
            _ => false,
        })
        .collect()
}

/// The `Bind` sources of `var` in `f`, in canonical order.
fn binds(body: &str, var: &str) -> Vec<ValueSource> {
    facts_of_f(body)
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::Bind { var: v, src, .. } if v == var => Some(src),
            _ => None,
        })
        .collect()
}

fn field_binds(body: &str, field: &str) -> Vec<ValueSource> {
    facts_of_f(body)
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::FieldBind { field: f, src, .. } if f == field => Some(src),
            _ => None,
        })
        .collect()
}

fn bind_names(body: &str) -> Vec<String> {
    let mut names: Vec<String> = facts_of_f(body)
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::Bind { var, .. } => Some(var),
            _ => None,
        })
        .collect();
    names.dedup();
    names
}

fn declared_type(annotation: &str) -> ValueSource {
    let mut b = binds(&format!("    x: {annotation}"), "x");
    assert_eq!(b.len(), 1, "{annotation}: {b:?}");
    b.pop().unwrap()
}

// --- binding-form matrix ---

#[test]
fn identifier_assignment_binds_its_source() {
    assert_eq!(binds("    x = y", "x"), vec![ValueSource::Var("y".into())]);
    assert_eq!(
        binds("    x = pkg.Foo(1)", "x"),
        vec![ValueSource::Call(segs("pkg.Foo"))]
    );
    assert_eq!(binds("    x = None", "x"), vec![ValueSource::Null]);
    assert_eq!(
        binds("    x = (y)", "x"),
        vec![ValueSource::Var("y".into())]
    );
    assert_eq!(binds("    x = a.b", "x"), vec![ValueSource::Opaque]);
    assert_eq!(binds("    x = f()()", "x"), vec![ValueSource::Opaque]);
}

#[test]
fn literal_sources_carry_their_kind() {
    let cases = [
        ("True", LiteralKind::Bool),
        ("'s'", LiteralKind::Str),
        ("'a' 'b'", LiteralKind::Str),
        ("b'x'", LiteralKind::Bytes),
        ("1.5", LiteralKind::Num),
        ("[1]", LiteralKind::List),
        ("[i for i in y]", LiteralKind::List),
        ("{1: 2}", LiteralKind::Dict),
        ("{k: v for k, v in y}", LiteralKind::Dict),
        ("{1}", LiteralKind::Set),
        ("(1, 2)", LiteralKind::Tuple),
    ];
    for (rhs, kind) in cases {
        assert_eq!(
            binds(&format!("    x = {rhs}"), "x"),
            vec![ValueSource::Literal(kind)],
            "{rhs}"
        );
    }
}

#[test]
fn chained_assignment_binds_every_name_to_the_value() {
    assert_eq!(
        binds("    a = b = Foo()", "a"),
        vec![ValueSource::Call(segs("Foo"))]
    );
    assert_eq!(
        binds("    a = b = Foo()", "b"),
        vec![ValueSource::Call(segs("Foo"))]
    );
}

#[test]
fn annotated_assignment_binds_declared_type() {
    assert_eq!(
        binds("    x: Foo = make()", "x"),
        vec![ValueSource::Declared(named("Foo"))]
    );
    assert_eq!(
        binds("    x: Foo", "x"),
        vec![ValueSource::Declared(named("Foo"))]
    );
}

#[test]
fn tuple_star_and_augmented_targets_are_opaque() {
    for body in [
        "    x, y = f()",
        "    [x, y] = f()",
        "    *x, y = f()",
        "    (x, (y, z)) = f()",
    ] {
        assert_eq!(binds(body, "x"), vec![ValueSource::Opaque], "{body}");
        assert_eq!(binds(body, "y"), vec![ValueSource::Opaque], "{body}");
    }
    assert_eq!(binds("    x += 1", "x"), vec![ValueSource::Opaque]);
}

#[test]
fn subscript_and_foreign_attribute_targets_bind_no_local() {
    assert!(bind_names("    a[0] = 1\n    o.f = 1\n    a[0] += 1").is_empty());
}

#[test]
fn walrus_binds_its_value() {
    assert_eq!(
        binds("    if (x := Foo()):\n        pass", "x"),
        vec![ValueSource::Call(segs("Foo"))]
    );
}

#[test]
fn for_targets_bind_an_element_of_the_iterable() {
    assert_eq!(
        binds("    for x in xs:\n        pass", "x"),
        vec![ValueSource::Element(Box::new(ValueSource::Var(
            "xs".into()
        )))]
    );
    assert_eq!(
        binds("    ys = [x for x in make()]", "x"),
        vec![ValueSource::Element(Box::new(ValueSource::Call(segs(
            "make"
        ))))]
    );
    assert_eq!(
        binds("    for k, v in d.items():\n        pass", "k"),
        vec![ValueSource::Opaque]
    );
    assert_eq!(
        binds("    g = (v for k, v in d)", "v"),
        vec![ValueSource::Opaque]
    );
}

#[test]
fn with_alias_binds_the_entered_value() {
    assert_eq!(
        binds("    with open(p) as fh:\n        pass", "fh"),
        vec![ValueSource::Enter(Box::new(ValueSource::Call(segs(
            "open"
        ))))]
    );
    assert_eq!(
        binds("    with (a as b, c as d):\n        pass", "d"),
        vec![ValueSource::Enter(Box::new(ValueSource::Var("c".into())))]
    );
    assert_eq!(
        binds("    with cm() as (r, s):\n        pass", "r"),
        vec![ValueSource::Opaque]
    );
}

#[test]
fn except_alias_is_declared_as_the_caught_type() {
    assert_eq!(
        binds(
            "    try:\n        pass\n    except E as e:\n        pass",
            "e"
        ),
        vec![ValueSource::Declared(named("E"))]
    );
    assert_eq!(
        binds(
            "    try:\n        pass\n    except (A, b.B) as e:\n        pass",
            "e"
        ),
        vec![ValueSource::Declared(TypeExpr::Union(vec![
            named("A"),
            named("b.B")
        ]))]
    );
    assert_eq!(
        binds(
            "    try:\n        pass\n    except* G as e:\n        pass",
            "e"
        ),
        vec![ValueSource::Declared(named("G"))]
    );
}

#[test]
fn match_captures_are_opaque_and_value_patterns_are_not_bindings() {
    let body = "    match v:\n        case Foo(x=y) as w:\n            pass\n        case [m, *rest]:\n            pass\n        case {\"k\": kv, **dr}:\n            pass\n        case Color.RED:\n            pass\n        case cap:\n            pass\n        case _:\n            pass";
    let mut names = bind_names(body);
    names.sort();
    assert_eq!(names, ["cap", "dr", "kv", "m", "rest", "w", "y"]);
    assert_eq!(binds(body, "y"), vec![ValueSource::Opaque]);
}

#[test]
fn scope_statements_local_imports_and_del_are_opaque() {
    let body = "    global g\n    nonlocal n\n    import a.b.c, d.e as de\n    from m import p, q as r\n    del z, o.f, xs[0]";
    let mut names = bind_names(body);
    names.sort();
    assert_eq!(names, ["a", "de", "g", "n", "p", "r", "z"]);
    for n in names {
        assert_eq!(binds(body, &n), vec![ValueSource::Opaque], "{n}");
    }
}

#[test]
fn nested_definitions_are_opaque_names_and_not_descended() {
    let body = "    def inner(z):\n        w = Foo()\n    class K:\n        a = 1\n    @deco\n    def dec():\n        pass";
    let mut names = bind_names(body);
    names.sort();
    assert_eq!(names, ["K", "dec", "inner"]);
    assert_eq!(binds(body, "inner"), vec![ValueSource::Opaque]);
    assert_eq!(binds(body, "K"), vec![ValueSource::Opaque]);
}

#[test]
fn lambda_parameters_are_opaque_bindings_of_the_enclosing_callable() {
    let body = "    node = Node()\n    items.sort(key=lambda node, d=1, *a, k, **kw: node.priority())\n    g = lambda: (q := Foo())";
    let mut names = bind_names(body);
    names.sort();
    assert_eq!(names, ["a", "d", "g", "k", "kw", "node", "q"]);
    assert_eq!(
        binds(body, "node"),
        vec![ValueSource::Call(segs("Node")), ValueSource::Opaque]
    );
}

#[test]
fn nonlocal_in_a_nested_scope_poisons_the_enclosing_local() {
    let body = "    x = A()\n    def reset():\n        nonlocal x\n        x = B()\n    class K:\n        def m(self):\n            def deeper():\n                nonlocal y\n";
    assert_eq!(
        binds(body, "x"),
        vec![ValueSource::Call(segs("A")), ValueSource::Opaque]
    );
    assert_eq!(binds(body, "y"), vec![ValueSource::Opaque]);
}

#[test]
fn nested_def_defaults_and_class_bases_are_walked_in_this_scope() {
    let body = "    def inner(a=(w := Foo())):\n        pass\n    class K(Base, metaclass=(m := Meta())):\n        pass";
    assert_eq!(binds(body, "w"), vec![ValueSource::Call(segs("Foo"))]);
    assert_eq!(binds(body, "m"), vec![ValueSource::Call(segs("Meta"))]);
}

#[test]
fn nested_function_facts_live_under_its_own_fqn() {
    let facts = extract("m.py", "def f():\n    def inner(z):\n        w = Foo()\n");
    let inner = format!("{}::f::inner", facts.module.as_deref().unwrap());
    assert!(facts.type_facts.contains(&TypeFact::Bind {
        func: inner.clone(),
        var: "w".into(),
        src: ValueSource::Call(segs("Foo")),
    }));
    assert!(facts
        .type_facts
        .iter()
        .any(|t| matches!(t, TypeFact::Param { func, name, .. } if *func == inner && name == "z")));
}

#[test]
fn type_alias_statement_is_opaque() {
    assert_eq!(binds("    type T = int", "T"), vec![ValueSource::Opaque]);
}

#[test]
fn every_rebinding_site_is_its_own_fact() {
    assert_eq!(
        binds(
            "    x = Foo()\n    x = y\n    for x in z:\n        pass",
            "x"
        ),
        vec![
            ValueSource::Var("y".into()),
            ValueSource::Call(segs("Foo")),
            ValueSource::Element(Box::new(ValueSource::Var("z".into()))),
        ]
    );
}

// --- field binds ---

#[test]
fn self_field_binds() {
    assert_eq!(
        field_binds("    self.h = Handler()", "h"),
        vec![ValueSource::Call(segs("Handler"))]
    );
    assert_eq!(
        field_binds("    self.h: Handler = None", "h"),
        vec![ValueSource::Declared(named("Handler"))]
    );
    assert_eq!(
        field_binds("    self.n += 1", "n"),
        vec![ValueSource::Opaque]
    );
    assert_eq!(
        field_binds("    self.a, b = f()", "a"),
        vec![ValueSource::Opaque]
    );
    assert!(field_binds("    other.h = 1\n    self.a.b = 1", "h").is_empty());
    assert!(field_binds("    self.a.b = 1", "b").is_empty());
}

fn field_binds_in(src: &str, func: &str) -> Vec<(String, ValueSource)> {
    let facts = extract("m.py", src);
    let func = format!("{}::{func}", facts.module.as_deref().unwrap());
    facts
        .type_facts
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::FieldBind {
                func: f,
                field,
                src,
            } if f == func => Some((field, src)),
            _ => None,
        })
        .collect()
}

#[test]
fn field_binds_through_cls_and_any_first_parameter_name() {
    let src = "class C:\n    @classmethod\n    def configure(cls):\n        cls.h = B()\n    def init(s):\n        s.g = A()\n    def other(self, o):\n        o.z = 1\n        def cb():\n            self.k = 1\n            o.w = 1\n";
    assert_eq!(
        field_binds_in(src, "C::configure"),
        vec![("h".into(), ValueSource::Call(segs("B")))]
    );
    assert_eq!(
        field_binds_in(src, "C::init"),
        vec![("g".into(), ValueSource::Call(segs("A")))]
    );
    assert!(field_binds_in(src, "C::other").is_empty());
    assert_eq!(
        field_binds_in(src, "C::other::cb"),
        vec![("k".into(), ValueSource::Literal(LiteralKind::Num))]
    );
}

fn attr_stores_in(src: &str, func: &str) -> Vec<(ValueSource, String, ValueSource)> {
    let facts = extract("m.py", src);
    let module = facts.module.clone().unwrap();
    let func = if func.is_empty() {
        module
    } else {
        format!("{module}::{func}")
    };
    facts
        .type_facts
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::AttrStore {
                func: f,
                base,
                field,
                src,
            } if f == func => Some((base, field, src)),
            _ => None,
        })
        .collect()
}

#[test]
fn non_receiver_attribute_stores_are_attr_stores() {
    let src = "def f(self, other):\n    other.h = B()\n    self.a.b = 1\n    other.n += 1\n\n\nobj.g = A()\n";
    let var = |n: &str| ValueSource::Var(n.into());
    assert_eq!(
        attr_stores_in(src, "f"),
        vec![
            (var("other"), "h".into(), ValueSource::Call(segs("B"))),
            (var("other"), "n".into(), ValueSource::Opaque),
            (
                ValueSource::Opaque,
                "b".into(),
                ValueSource::Literal(LiteralKind::Num)
            ),
        ]
    );
    assert_eq!(
        attr_stores_in(src, ""),
        vec![(var("obj"), "g".into(), ValueSource::Call(segs("A")))]
    );
    // A store through the receiver stays a field bind.
    assert!(attr_stores_in("class C:\n    def m(self):\n        self.h = 1\n", "C::m").is_empty());
}

#[test]
fn nested_callables_store_through_the_enclosing_receiver() {
    let src = "class C:\n    def __init__(this):\n        def later():\n            this.h = B()\n        def shadow(this):\n            this.g = B()\n        def rebinds():\n            this = other()\n            this.k = B()\n";
    assert_eq!(
        field_binds_in(src, "C::__init__::later"),
        vec![("h".into(), ValueSource::Call(segs("B")))]
    );
    assert!(field_binds_in(src, "C::__init__::shadow").is_empty());
    assert_eq!(attr_stores_in(src, "C::__init__::shadow").len(), 1);
    assert!(field_binds_in(src, "C::__init__::rebinds").is_empty());
    assert_eq!(
        attr_stores_in(src, "C::__init__::rebinds"),
        vec![(
            ValueSource::Var("this".into()),
            "k".into(),
            ValueSource::Call(segs("B"))
        )]
    );
}

#[test]
fn pep695_type_parameters_are_unknown() {
    let src = "class A:\n    pass\n\n\ndef f[A, *Ts, **P](x: A, y: list[A]):\n    z: A = x\n\n\nclass G[T]:\n    def m(self, x: T, y: A):\n        pass\n";
    let facts = extract("m.py", src);
    let module = facts.module.clone().unwrap();
    let ty = |func: &str, name: &str| {
        facts.type_facts.iter().find_map(|t| match t {
            TypeFact::Param {
                func: f,
                name: n,
                ty,
                ..
            } if *f == format!("{module}::{func}") && n == name => ty.clone(),
            _ => None,
        })
    };
    assert_eq!(ty("f", "x"), Some(TypeExpr::Unknown));
    assert_eq!(
        ty("f", "y"),
        Some(TypeExpr::Generic {
            head: segs("list"),
            args: vec![TypeExpr::Unknown],
            indirect: false,
        })
    );
    assert_eq!(ty("G::m", "x"), Some(TypeExpr::Unknown));
    assert_eq!(ty("G::m", "y"), Some(named("A")));
    let z = facts.type_facts.iter().find_map(|t| match t {
        TypeFact::Bind { var, src, .. } if var == "z" => Some(src.clone()),
        _ => None,
    });
    assert_eq!(z, Some(ValueSource::Declared(TypeExpr::Unknown)));
}

#[test]
fn calling_a_type_parameter_is_opaque() {
    let src = "class A:\n    def m(self):\n        pass\n\n\ndef g[A]():\n    x = A()\n    A().m()\n    return x\n\n\ndef h():\n    y = A()\n    return y\n";
    let facts = extract("m.py", src);
    let bind = |var: &str| {
        facts.type_facts.iter().find_map(|t| match t {
            TypeFact::Bind { var: v, src, .. } if v == var => Some(src.clone()),
            _ => None,
        })
    };
    assert_eq!(bind("x"), Some(ValueSource::Opaque));
    assert_eq!(bind("y"), Some(ValueSource::Call(segs("A"))));
    let root = facts.type_facts.iter().find_map(|t| match t {
        TypeFact::AnonReceiver { method, root, .. } if method == "m" => Some(root.clone()),
        _ => None,
    });
    assert_eq!(root, Some(AnonRoot::Other));
}

#[test]
fn first_parameter_of_a_module_function_is_not_a_receiver() {
    let src = "def f(o):\n    o.z = 1\n";
    assert!(field_binds_in(src, "f").is_empty());
}

// --- parameters ---

#[test]
fn params_carry_kind_index_and_type() {
    let facts = extract(
        "m.py",
        "def f(self, a, b: int, c=1, d: \"pkg.Foo\" = None, /, e=2, *args: str, k, kd: Foo = x, **kw):\n    pass\ndef g(a, *, b):\n    pass\n",
    );
    let module = facts.module.clone().unwrap();
    let params = |func: &str| -> Vec<(String, Option<u8>, ParamKind, Option<TypeExpr>)> {
        let func = format!("{module}::{func}");
        facts
            .type_facts
            .iter()
            .filter_map(|t| match t {
                TypeFact::Param {
                    func: f,
                    name,
                    index,
                    kind,
                    ty,
                } if *f == func => Some((name.clone(), *index, *kind, ty.clone())),
                _ => None,
            })
            .collect()
    };
    let mut f = params("f");
    f.sort_by_key(|p| p.1);
    use ParamKind::*;
    assert_eq!(
        f,
        vec![
            ("self".into(), Some(0), Positional, None),
            ("a".into(), Some(1), Positional, None),
            ("b".into(), Some(2), Positional, Some(named("int"))),
            ("c".into(), Some(3), Positional, None),
            ("d".into(), Some(4), Positional, Some(named("pkg.Foo"))),
            ("e".into(), Some(5), Positional, None),
            ("args".into(), Some(6), VarArgs, Some(named("str"))),
            ("k".into(), Some(7), KeywordOnly, None),
            ("kd".into(), Some(8), KeywordOnly, Some(named("Foo"))),
            ("kw".into(), Some(9), VarKeywords, None),
        ]
    );
    let mut g = params("g");
    g.sort_by_key(|p| p.1);
    assert_eq!(
        g,
        vec![
            ("a".into(), Some(0), Positional, None),
            ("b".into(), Some(1), KeywordOnly, None),
        ]
    );
}

// --- annotation normalization ---

#[test]
fn optional_and_none_unions_drop_none() {
    for a in [
        "Optional[Foo]",
        "typing.Optional[Foo]",
        "Foo | None",
        "None | Foo",
        "Union[Foo, None]",
        "\"Foo\"",
        "\"Optional[Foo]\"",
    ] {
        assert_eq!(declared_type(a), ValueSource::Declared(named("Foo")), "{a}");
    }
}

#[test]
fn string_annotation_parses_dotted_names() {
    assert_eq!(
        declared_type("\"pkg.Foo\""),
        ValueSource::Declared(named("pkg.Foo"))
    );
    assert_eq!(
        declared_type("\"not valid ((\""),
        ValueSource::Declared(TypeExpr::Unknown)
    );
}

#[test]
fn unions_flatten() {
    let ab = TypeExpr::Union(vec![named("A"), named("B")]);
    for a in [
        "Union[A, B]",
        "A | B",
        "A | B | None",
        "Optional[Union[A, B]]",
        "Union[A, A | B]",
    ] {
        assert_eq!(declared_type(a), ValueSource::Declared(ab.clone()), "{a}");
    }
}

#[test]
fn other_subscripts_are_generic() {
    assert_eq!(
        declared_type("dict[str, Foo]"),
        ValueSource::Declared(TypeExpr::Generic {
            head: segs("dict"),
            args: vec![named("str"), named("Foo")],
            indirect: false,
        })
    );
    assert_eq!(
        declared_type("t.List[Foo]"),
        ValueSource::Declared(TypeExpr::Generic {
            head: segs("t.List"),
            args: vec![named("Foo")],
            indirect: false,
        })
    );
}

#[test]
fn any_and_unmodelled_annotations_are_unknown() {
    for a in [
        "Any",
        "typing.Any",
        "None",
        "Literal[1]",
        "Callable[[int], str]",
    ] {
        let got = declared_type(a);
        match a {
            "Literal[1]" | "Callable[[int], str]" => {
                assert!(
                    matches!(got, ValueSource::Declared(TypeExpr::Generic { .. })),
                    "{a}: {got:?}"
                );
            }
            _ => assert_eq!(got, ValueSource::Declared(TypeExpr::Unknown), "{a}"),
        }
    }
}

// --- class bases ---

fn class_bases(src: &str, class: &str) -> Vec<Vec<BaseExpr>> {
    let facts = extract("m.py", src);
    let fqn = format!("{}::{class}", facts.module.as_deref().unwrap());
    facts
        .type_facts
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::ClassBases { class, bases } if class == fqn => Some(bases),
            _ => None,
        })
        .collect()
}

#[test]
fn class_bases_keep_source_order_and_unnameable_bases() {
    assert_eq!(
        class_bases(
            "class K(Z, pkg.B, Generic[T], mk(), (lambda: 1)(), *more, metaclass=M):\n    pass\n",
            "K"
        ),
        vec![vec![
            BaseExpr::Type(named("Z")),
            BaseExpr::Type(named("pkg.B")),
            BaseExpr::Type(TypeExpr::Generic {
                head: segs("Generic"),
                args: vec![named("T")],
                indirect: false,
            }),
            BaseExpr::Call(segs("mk")),
            BaseExpr::Unknown,
            BaseExpr::Unknown,
        ]]
    );
}

#[test]
fn factory_bases_with_non_literal_arguments_are_unknown() {
    assert_eq!(
        class_bases(
            "class K(declarative_base(), mk(\"x\", n=1), with_metaclass(M, Core), f(cls=B)):\n    pass\n",
            "K"
        ),
        vec![vec![
            BaseExpr::Call(segs("declarative_base")),
            BaseExpr::Call(segs("mk")),
            BaseExpr::Unknown,
            BaseExpr::Unknown,
        ]]
    );
}

#[test]
fn module_level_bindings_are_binds_of_the_module() {
    let facts = extract(
        "m.py",
        "from x import Real, declarative_base\nCompat = Real\nBase = declarative_base()\nMixed = declarative_base(cls=Real)\n\ndef f():\n    y = 1\n",
    );
    let module = facts.module.clone().unwrap();
    let mut got: Vec<(String, ValueSource)> = facts
        .type_facts
        .iter()
        .filter_map(|t| match t {
            TypeFact::Bind { func, var, src } if *func == module => {
                Some((var.clone(), src.clone()))
            }
            _ => None,
        })
        .collect();
    got.retain(|(v, _)| ["Compat", "Base", "Mixed", "y"].contains(&v.as_str()));
    assert_eq!(
        got,
        vec![
            (
                "Base".to_owned(),
                ValueSource::Call(segs("declarative_base"))
            ),
            ("Compat".to_owned(), ValueSource::Var("Real".into())),
            // A module-level factory given a class may subclass it.
            ("Mixed".to_owned(), ValueSource::Opaque),
        ]
    );
    // Inside a callable a call keeps its callee whatever its arguments.
    assert_eq!(
        binds("    z = g(Real)", "z"),
        vec![ValueSource::Call(segs("g"))]
    );
}

#[test]
fn class_without_bases_still_gets_a_fact() {
    assert_eq!(class_bases("class K:\n    pass\n", "K"), vec![vec![]]);
    assert_eq!(class_bases("class K():\n    pass\n", "K"), vec![vec![]]);
}

#[test]
fn class_bases_leave_inheritance_relations_untouched() {
    let facts = extract("m.py", "class K(Generic[T], pkg.B):\n    pass\n");
    let objects: Vec<String> = facts
        .impl_relations
        .iter()
        .map(|r| r.object.join("::"))
        .collect();
    assert_eq!(objects, ["B"]);
}

// --- anonymous receivers ---

fn anon(src: &str) -> Vec<(u32, u32, String, u8, AnonRoot)> {
    let facts = extract("m.py", src);
    let anons: Vec<_> = facts
        .type_facts
        .iter()
        .filter_map(|t| match t {
            TypeFact::AnonReceiver {
                line,
                col,
                method,
                depth,
                root,
                ..
            } => Some((*line, *col, method.clone(), *depth, root.clone())),
            _ => None,
        })
        .collect();
    // Every fact joins to exactly one virtual-receiver call ref by span, with
    // the method as its last segment and `depth` as its path length.
    for (line, col, method, depth, _) in &anons {
        let refs: Vec<_> = facts
            .refs
            .iter()
            .filter(|r| r.span.line == *line && r.span.col == Some(*col))
            .collect();
        assert_eq!(refs.len(), 1, "line {line} col {col}");
        assert_eq!(refs[0].kind, RefKind::CallVirtualReceiver);
        assert_eq!(refs[0].name_path.last(), Some(method));
        assert_eq!(refs[0].name_path.len(), *depth as usize);
    }
    anons
}

#[test]
fn anon_receiver_root_kinds() {
    let src = "def f(xs):\n    ''.join(xs)\n    b'x'.decode()\n    (1).bit_length()\n    [].append(1)\n    {}.get(1)\n    {1}.add(2)\n    (1, 2).count(1)\n    super().save()\n    Foo().bar.baz()\n    xs[0].strip()\n    (a or b).run()\n    xs.strip()\n";
    let got = anon(src);
    let roots: Vec<(String, u8, AnonRoot)> =
        got.into_iter().map(|(_, _, m, d, r)| (m, d, r)).collect();
    use LiteralKind::*;
    assert_eq!(
        roots,
        vec![
            ("join".into(), 1, AnonRoot::Literal(Str)),
            ("decode".into(), 1, AnonRoot::Literal(Bytes)),
            ("bit_length".into(), 1, AnonRoot::Literal(Num)),
            ("append".into(), 1, AnonRoot::Literal(List)),
            ("get".into(), 1, AnonRoot::Literal(Dict)),
            ("add".into(), 1, AnonRoot::Literal(Set)),
            ("count".into(), 1, AnonRoot::Literal(Tuple)),
            ("save".into(), 1, AnonRoot::Super),
            ("baz".into(), 2, AnonRoot::Call(segs("Foo"))),
            ("strip".into(), 1, AnonRoot::Subscript),
            ("run".into(), 1, AnonRoot::Other),
        ]
    );
}

#[test]
fn super_naming_another_class_is_not_the_enclosing_super() {
    let src = "class B:\n    pass\n\nclass C(B):\n    def m(self):\n        super(C, self).m()\n        super(B, self).m()\n        def inner():\n            super(C, self).n()\n";
    let roots: Vec<(String, AnonRoot)> = anon(src)
        .into_iter()
        .map(|(_, _, m, _, r)| (m, r))
        .collect();
    assert_eq!(
        roots,
        vec![
            ("m".into(), AnonRoot::Super),
            ("m".into(), AnonRoot::Call(segs("super"))),
            ("n".into(), AnonRoot::Super),
        ]
    );
}

#[test]
fn anon_receiver_is_keyed_to_the_enclosing_callable() {
    let facts = extract("m.py", "class C:\n    def m(self):\n        ''.join([])\n");
    let func = format!("{}::C::m", facts.module.as_deref().unwrap());
    assert!(facts
        .type_facts
        .iter()
        .any(|t| matches!(t, TypeFact::AnonReceiver { func: f, .. } if *f == func)));
}

// --- module identity ---

fn extract_with(ctx: FileCtx, src: &str) -> FileFacts {
    let mut facts = PythonFrontend::new()
        .extract(src.as_bytes(), &ctx)
        .expect("extract");
    facts.canonicalize();
    facts
}

#[test]
fn module_is_the_def_prefix_for_a_flat_file() {
    let facts = extract("pkg/mod.py", "def f():\n    pass\n");
    assert_eq!(facts.module.as_deref(), Some("pkg::mod"));
    assert!(facts.defs.iter().any(|d| d.fqn == "pkg::mod::f"));
}

#[test]
fn module_is_the_def_prefix_for_a_src_layout_file() {
    let mut ctx = FileCtx::new("proj/src/pkg/mod.py", "oid");
    ctx.manifest = Some(ManifestInfo {
        kind: ManifestKind::PyProject,
        identity: None,
        root_dir: "proj/src".into(),
    });
    let facts = extract_with(ctx, "def f():\n    pass\n");
    assert_eq!(facts.module.as_deref(), Some("pkg::mod"));
    assert!(facts.defs.iter().any(|d| d.fqn == "pkg::mod::f"));
}
