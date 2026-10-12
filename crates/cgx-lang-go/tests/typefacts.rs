//! Receiver-typing facts emitted by the Go frontend: receivers, parameters,
//! binding forms, call arguments and the canonical module path.

mod common;

use cgx_frontend::{
    CallArg, FileCtx, LanguageFrontend, LiteralKind, ManifestInfo, ManifestKind, ParamKind,
    TypeExpr, TypeFact, ValueSource,
};
use cgx_lang_go::GoFrontend;
use common::extract;
use smallvec::SmallVec;

fn segs(path: &str) -> SmallVec<[String; 2]> {
    path.split('.').map(str::to_string).collect()
}

fn named(path: &str, indirect: bool) -> TypeExpr {
    TypeExpr::Named {
        path: segs(path),
        indirect,
    }
}

/// Facts of `func F(...)` in package `p` with the given parameter list and
/// body, keyed by that function's FQN.
fn facts_of_f(params: &str, body: &str) -> Vec<TypeFact> {
    let src = format!("package p\n\nfunc F({params}) {{\n{body}\n}}\n");
    let facts = extract("p/f.go", &src);
    let func = format!("{}::F", facts.module.as_deref().expect("module set"));
    facts
        .type_facts
        .into_iter()
        .filter(|t| match t {
            TypeFact::Param { func: f, .. }
            | TypeFact::Bind { func: f, .. }
            | TypeFact::CallArgs { func: f, .. } => *f == func,
            _ => false,
        })
        .collect()
}

fn binds(body: &str, var: &str) -> Vec<ValueSource> {
    facts_of_f("", body)
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::Bind { var: v, src, .. } if v == var => Some(src),
            _ => None,
        })
        .collect()
}

fn bind_names(body: &str) -> Vec<String> {
    let mut names: Vec<String> = facts_of_f("", body)
        .into_iter()
        .filter_map(|t| match t {
            TypeFact::Bind { var, .. } => Some(var),
            _ => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn pointer_receiver_is_an_indirect_named_param() {
    let facts = extract(
        "p/f.go",
        "package p\n\nfunc (r *T) M() {}\nfunc (v T) N() {}\nfunc (*T) O() {}\n",
    );
    let module = facts.module.clone().unwrap();
    let recv = |m: &str| -> Vec<(String, Option<TypeExpr>)> {
        facts
            .type_facts
            .iter()
            .filter_map(|t| match t {
                TypeFact::Param {
                    func,
                    name,
                    index: None,
                    kind: ParamKind::Receiver,
                    ty,
                } if func.ends_with(m) => Some((name.clone(), ty.clone())),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        recv("::(*T)::M"),
        vec![("r".into(), Some(named("T", true)))]
    );
    assert_eq!(recv("::T::N"), vec![("v".into(), Some(named("T", false)))]);
    assert!(recv("::(*T)::O").is_empty());
    assert!(facts
        .type_facts
        .iter()
        .all(|t| !matches!(t, TypeFact::Param { func, .. } if !func.starts_with(&module))));
}

#[test]
fn params_carry_index_kind_and_type() {
    let params: Vec<(String, Option<u8>, ParamKind, Option<TypeExpr>)> =
        facts_of_f("a, b int, _ string, c pkg.R, d *List[X], e ...*T", "")
            .into_iter()
            .filter_map(|t| match t {
                TypeFact::Param {
                    name,
                    index,
                    kind,
                    ty,
                    ..
                } => Some((name, index, kind, ty)),
                _ => None,
            })
            .collect();
    let mut params = params;
    params.sort_by_key(|p| p.1);
    use ParamKind::*;
    assert_eq!(
        params,
        vec![
            ("a".into(), Some(0), Positional, Some(named("int", false))),
            ("b".into(), Some(1), Positional, Some(named("int", false))),
            (
                "_".into(),
                Some(2),
                Positional,
                Some(named("string", false))
            ),
            ("c".into(), Some(3), Positional, Some(named("pkg.R", false))),
            (
                "d".into(),
                Some(4),
                Positional,
                Some(TypeExpr::Generic {
                    head: segs("List"),
                    args: vec![named("X", false)],
                    indirect: true,
                })
            ),
            ("e".into(), Some(5), VarArgs, Some(named("T", true))),
        ]
    );
}

#[test]
fn unnamed_params_bind_nothing() {
    // Go groups `a, b T`, so `F(int, s string)` names a param `int`; only an
    // all-unnamed list declares no names.
    assert!(facts_of_f("int, string", "").is_empty());
}

#[test]
fn composite_literals_are_allocations() {
    assert_eq!(
        binds("\tx := T{}", "x"),
        vec![ValueSource::New(named("T", false))]
    );
    assert_eq!(
        binds("\tx := &pkg.U{A: 1}", "x"),
        vec![ValueSource::New(named("pkg.U", true))]
    );
    assert_eq!(
        binds("\tx := (&T{})", "x"),
        vec![ValueSource::New(named("T", true))]
    );
}

#[test]
fn var_declarations() {
    assert_eq!(
        binds("\tvar v T", "v"),
        vec![ValueSource::Declared(named("T", false))]
    );
    assert_eq!(
        binds("\tvar v *pkg.T", "v"),
        vec![ValueSource::Declared(named("pkg.T", true))]
    );
    assert_eq!(
        binds("\tvar v []T", "v"),
        vec![ValueSource::Declared(TypeExpr::Unknown)]
    );
    assert_eq!(
        binds("\tvar v = w", "v"),
        vec![ValueSource::Var("w".into())]
    );
    assert_eq!(
        binds("\tvar v T = w", "v"),
        vec![ValueSource::Declared(named("T", false))]
    );
    assert_eq!(
        binds("\tvar r io.Reader = &fileReader{}", "r"),
        vec![ValueSource::Declared(named("io.Reader", false))]
    );
    assert_eq!(binds("\tvar (a, b = f())", "a"), vec![ValueSource::Opaque]);
}

#[test]
fn copies_calls_literals_and_nil() {
    assert_eq!(binds("\tx := y", "x"), vec![ValueSource::Var("y".into())]);
    assert_eq!(binds("\tx := nil", "x"), vec![ValueSource::Null]);
    assert_eq!(
        binds("\tx := \"s\"", "x"),
        vec![ValueSource::Literal(LiteralKind::Str)]
    );
    assert_eq!(
        binds("\tx := 1", "x"),
        vec![ValueSource::Literal(LiteralKind::Num)]
    );
    assert_eq!(
        binds("\tx := pkg.New()", "x"),
        vec![ValueSource::Call(segs("pkg.New"))]
    );
    assert_eq!(binds("\tx := f()()", "x"), vec![ValueSource::Opaque]);
    assert_eq!(binds("\tx := a.b", "x"), vec![ValueSource::Opaque]);
    let pair = "\tx, y := a, T{}";
    assert_eq!(binds(pair, "x"), vec![ValueSource::Var("a".into())]);
    assert_eq!(binds(pair, "y"), vec![ValueSource::New(named("T", false))]);
    assert_eq!(binds("\tx = y", "x"), vec![ValueSource::Var("y".into())]);
}

#[test]
fn multi_value_and_compound_assignments_are_opaque() {
    assert_eq!(binds("\tx, err := f()", "x"), vec![ValueSource::Opaque]);
    assert_eq!(binds("\tx, err := f()", "err"), vec![ValueSource::Opaque]);
    assert_eq!(binds("\tx += 1", "x"), vec![ValueSource::Opaque]);
    assert_eq!(binds("\tx++", "x"), vec![ValueSource::Opaque]);
    assert_eq!(binds("\tx--", "x"), vec![ValueSource::Opaque]);
}

#[test]
fn range_receive_and_type_switch_bindings_are_opaque() {
    assert_eq!(
        binds("\tfor i, v := range xs {}", "v"),
        vec![ValueSource::Opaque]
    );
    assert_eq!(
        binds("\tfor i, v := range xs {}", "i"),
        vec![ValueSource::Opaque]
    );
    assert_eq!(
        binds("\tselect {\n\tcase m, ok := <-ch:\n\t\t_ = m\n\t}", "m"),
        vec![ValueSource::Opaque]
    );
    assert_eq!(
        binds("\tswitch t := v.(type) {}", "t"),
        vec![ValueSource::Opaque]
    );
    assert_eq!(binds("\tconst c = 1", "c"), vec![ValueSource::Opaque]);
}

#[test]
fn closure_bindings_and_params_are_opaque() {
    let body =
        "\tx := T{}\n\tfn := func(k int) (r T) { x = T{}; var w = T{}; y := x; h(x) }\n\t_ = fn";
    assert_eq!(
        binds(body, "x"),
        vec![ValueSource::New(named("T", false)), ValueSource::Opaque]
    );
    for n in ["k", "r", "w", "y"] {
        assert_eq!(binds(body, n), vec![ValueSource::Opaque], "{n}");
    }
}

#[test]
fn blank_identifier_and_non_local_targets_bind_nothing() {
    assert!(
        bind_names("\t_ = f()\n\t_, _ = a, b\n\ts.f = 1\n\ta[0] = 1\n\t*p = 1\n\ts.n++").is_empty()
    );
}

#[test]
fn named_results_are_declared_locals() {
    let facts = extract(
        "p/f.go",
        "package p\n\nfunc F() (n int, err error) { return }\n",
    );
    let module = facts.module.clone().unwrap();
    let func = format!("{module}::F");
    for (name, ty) in [("n", "int"), ("err", "error")] {
        assert!(
            facts.type_facts.contains(&TypeFact::Bind {
                func: func.clone(),
                var: name.into(),
                src: ValueSource::Declared(named(ty, false)),
            }),
            "{name}"
        );
    }
}

/// `(line, col, callee, args)` of one `CallArgs` fact.
type CallSite = (u32, u32, SmallVec<[String; 2]>, Vec<CallArg>);

#[test]
fn call_args_carry_sources_and_the_call_span() {
    let src = "package p\n\nfunc F(x T) {\n\th(x, \"s\", 1, &T{}, nil, pkg.Make(), xs...)\n\tpkg.G(T{})\n\tg()\n\tf()(x)\n\tfunc() { h(x) }()\n}\n";
    let facts = extract("p/f.go", src);
    let func = format!("{}::F", facts.module.as_deref().unwrap());
    let calls: Vec<CallSite> = facts
        .type_facts
        .iter()
        .filter_map(|t| match t {
            TypeFact::CallArgs {
                func: f,
                line,
                col,
                callee,
                args,
            } if *f == func => Some((*line, *col, callee.clone(), args.clone())),
            _ => None,
        })
        .collect();
    let arg = |value| CallArg {
        keyword: None,
        value,
    };
    assert_eq!(
        calls,
        vec![
            (
                4,
                2,
                segs("h"),
                vec![
                    arg(ValueSource::Var("x".into())),
                    arg(ValueSource::Literal(LiteralKind::Str)),
                    arg(ValueSource::Literal(LiteralKind::Num)),
                    arg(ValueSource::New(named("T", true))),
                    arg(ValueSource::Null),
                    arg(ValueSource::Call(segs("pkg.Make"))),
                    arg(ValueSource::Opaque),
                ]
            ),
            (
                5,
                2,
                segs("pkg.G"),
                vec![arg(ValueSource::New(named("T", false)))]
            ),
            (8, 11, segs("h"), vec![arg(ValueSource::Opaque)]),
        ]
    );
    // Every fact joins to the call ref with the same span.
    for (line, col, callee, _) in &calls {
        assert!(
            facts.refs.iter().any(|r| r.span.line == *line
                && r.span.col == Some(*col)
                && r.name_path.last() == callee.last()),
            "no ref at {line}:{col}"
        );
    }
}

#[test]
fn module_is_the_go_mod_import_path() {
    let ctx = FileCtx::new("internal/store/db.go", "oid").with_manifest(Some(ManifestInfo {
        kind: ManifestKind::GoMod,
        identity: Some("example.com/app".into()),
        root_dir: String::new(),
    }));
    let mut facts = GoFrontend::new()
        .extract(b"package store\n\nfunc Open() {}\n", &ctx)
        .unwrap();
    facts.canonicalize();
    assert_eq!(
        facts.module.as_deref(),
        Some("example.com/app/internal/store")
    );
    assert!(facts
        .defs
        .iter()
        .any(|d| d.fqn == "example.com/app/internal/store::Open"));
}

#[test]
fn module_is_set_without_a_manifest() {
    let facts = extract("p/f.go", "package p\n\nfunc F() {}\n");
    let module = facts.module.clone().expect("module set");
    assert!(facts.defs.iter().any(|d| d.fqn == format!("{module}::F")));
}

#[test]
fn generic_types_keep_their_indirection() {
    let facts = extract(
        "p/f.go",
        "package p\n\nfunc (l *List[T]) M(v List[T], m map[K]V) {}\n",
    );
    let tys: Vec<(String, Option<TypeExpr>)> = facts
        .type_facts
        .iter()
        .filter_map(|t| match t {
            TypeFact::Param { name, ty, .. } => Some((name.clone(), ty.clone())),
            _ => None,
        })
        .collect();
    // `T` is the receiver's type parameter, so it is `Unknown`.
    let list = |indirect| TypeExpr::Generic {
        head: segs("List"),
        args: vec![TypeExpr::Unknown],
        indirect,
    };
    assert!(tys.contains(&("l".into(), Some(list(true)))), "{tys:?}");
    assert!(tys.contains(&("v".into(), Some(list(false)))), "{tys:?}");
    assert!(
        tys.contains(&("m".into(), Some(TypeExpr::Unknown))),
        "{tys:?}"
    );
}

#[test]
fn address_taken_locals_are_opaque() {
    let body = "\tvar r io.Reader = &fileReader{}\n\treopen(&r)\n\tvar cfg Config\n\tjson.Unmarshal(b, &(cfg))\n\tp := &s.f\n\t_ = p";
    assert_eq!(
        binds(body, "r"),
        vec![
            ValueSource::Declared(named("io.Reader", false)),
            ValueSource::Opaque
        ]
    );
    assert_eq!(
        binds(body, "cfg"),
        vec![
            ValueSource::Declared(named("Config", false)),
            ValueSource::Opaque
        ]
    );
    assert!(binds(body, "s").is_empty());
}

#[test]
fn type_parameters_are_unknown() {
    let src = "package p\n\ntype File struct{}\n\ntype Box[K any] struct{}\n\nfunc G[File any, V any](x File, y *V, z Box[File]) {\n\tvar w File\n\t_ = w\n}\n\nfunc (b *Box[K]) M(k K, f File) {}\n";
    let facts = extract("p/f.go", src);
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
    assert_eq!(ty("G", "x"), Some(TypeExpr::Unknown));
    assert_eq!(ty("G", "y"), Some(TypeExpr::Unknown));
    assert_eq!(
        ty("G", "z"),
        Some(TypeExpr::Generic {
            head: segs("Box"),
            args: vec![TypeExpr::Unknown],
            indirect: false,
        })
    );
    assert_eq!(ty("(*Box[K])::M", "k"), Some(TypeExpr::Unknown));
    assert_eq!(ty("(*Box[K])::M", "f"), Some(named("File", false)));
    let w = facts.type_facts.iter().find_map(|t| match t {
        TypeFact::Bind { var, src, .. } if var == "w" => Some(src.clone()),
        _ => None,
    });
    assert_eq!(w, Some(ValueSource::Declared(TypeExpr::Unknown)));
}
