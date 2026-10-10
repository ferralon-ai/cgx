//! Grammar selection by extension: `.tsx`/`.jsx` parse with the TSX grammar,
//! everything else with the TypeScript grammar.

mod common;

use cgx_core::node::SymbolKind;
use cgx_frontend::{FileCtx, FileFacts, LanguageFrontend, RefKind};
use cgx_lang_ts::{FallbackStats, TypeScriptFrontend};
use common::extract;
use tree_sitter::{Node, Parser};

const COMPONENTS: &str = r#"
import React from 'react';

function helper(x: number): number { return x + 1; }

export function Button(props: { label: string; onClick: () => void }) {
  return <button onClick={() => props.onClick()}>{props.label}</button>;
}

export const List = ({ items }: { items: string[] }) => (
  <>
    <Button label="a" onClick={() => helper(1)} />
    {items.map((i) => <li key={i}>{format(i)}</li>)}
  </>
);

function format(s: string): string { return s.trim(); }
"#;

fn has_def(f: &FileFacts, suffix: &str, kind: SymbolKind) -> bool {
    f.defs
        .iter()
        .any(|d| d.fqn.ends_with(suffix) && d.kind == kind)
}

fn calls(f: &FileFacts, callee: &str) -> bool {
    f.refs.iter().any(|r| {
        r.name_path.last().map(String::as_str) == Some(callee)
            && matches!(
                r.kind,
                RefKind::Call | RefKind::CallClosure | RefKind::CallVirtualReceiver
            )
    })
}

fn count_errors(n: Node) -> usize {
    let mut c = n.walk();
    let own = usize::from(n.is_error() || n.is_missing());
    own + n.children(&mut c).map(count_errors).sum::<usize>()
}

fn errors_with(lang: tree_sitter::Language, src: &str) -> usize {
    let mut p = Parser::new();
    p.set_language(&lang).unwrap();
    count_errors(p.parse(src, None).unwrap().root_node())
}

#[test]
fn tsx_file_yields_defs_and_calls_inside_jsx() {
    let f = extract("src/List.tsx", COMPONENTS);
    for name in ["::helper", "::Button", "::format"] {
        assert!(has_def(&f, name, SymbolKind::Function), "def {name}: {f:?}");
    }
    // Arrow-function component bound to a const is a lambda def.
    assert!(has_def(&f, "::List", SymbolKind::Lambda), "{f:?}");
    // Calls that only exist as JSX attribute / child expressions.
    assert!(calls(&f, "helper"), "call in JSX attribute arrow: {f:?}");
    assert!(calls(&f, "format"), "call in JSX child expression: {f:?}");
    assert!(calls(&f, "onClick"), "call in JSX attribute arrow: {f:?}");
}

#[test]
fn jsx_file_works() {
    let src = "function helper(){ return 1; }\nexport function App(){ return <div onClick={() => helper()}>hi</div>; }\n";
    let f = extract("src/App.jsx", src);
    assert!(has_def(&f, "::App", SymbolKind::Function), "{f:?}");
    assert!(calls(&f, "helper"), "{f:?}");
}

#[test]
fn ts_file_keeps_angle_bracket_cast() {
    let src = "function id(x: unknown): number { return <number>x; }\nfunction g(){ return id(<any>1); }\n";
    for path in ["src/a.ts", "src/a.mts", "src/a.cts"] {
        let f = extract(path, src);
        assert!(has_def(&f, "::id", SymbolKind::Function), "{path}: {f:?}");
        assert!(has_def(&f, "::g", SymbolKind::Function), "{path}: {f:?}");
        assert!(calls(&f, "id"), "{path}: {f:?}");
    }
}

#[test]
fn grammars_split_on_jsx_versus_cast() {
    let ts: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let tsx: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let cast = "function id(x: unknown): number { return <number>x; }\n";
    assert_eq!(errors_with(ts.clone(), cast), 0);
    assert!(
        errors_with(tsx.clone(), cast) > 0,
        "TSX must reject <T>expr"
    );
    assert_eq!(errors_with(tsx, COMPONENTS), 0);
    assert!(
        errors_with(ts, COMPONENTS) > 0,
        "TS grammar must not parse JSX"
    );
}

const JSX_IN_JS: &str = "function helper(){ return 1; }\nexport function App(){ return <div onClick={() => helper()}>hi</div>; }\n";

#[test]
fn jsx_in_plain_js_is_recovered() {
    for path in ["src/App.js", "src/App.mjs", "src/App.cjs"] {
        let f = extract(path, JSX_IN_JS);
        assert!(has_def(&f, "::App", SymbolKind::Function), "{path}: {f:?}");
        assert!(calls(&f, "helper"), "{path}: {f:?}");
    }
}

/// Defs and refs by value, ignoring the file path carried in spans.
type Shape = (Vec<(String, SymbolKind)>, Vec<(Vec<String>, RefKind)>);

fn shape(f: &FileFacts) -> Shape {
    let defs = f.defs.iter().map(|d| (d.fqn.clone(), d.kind)).collect();
    let refs = f
        .refs
        .iter()
        .map(|r| (r.name_path.iter().cloned().collect(), r.kind))
        .collect();
    (defs, refs)
}

#[test]
fn plain_js_without_jsx_is_unchanged() {
    let src = "function f(a, b) { return a < b; }\nconst g = (x) => f(x, 1) > 2;\nclass K { m() { return g(1); } }\n";
    assert_eq!(
        shape(&extract("src/a.js", src)),
        shape(&extract("src/a.ts", src))
    );
}

#[test]
fn js_error_tie_keeps_typescript_tree() {
    // A syntax error that both grammars recover from identically: the TSX
    // re-parse is not strictly better, so the TypeScript tree is kept.
    let src = "function f(){ return 1; }\nfunction g(){ f( }\n";
    assert_eq!(
        shape(&extract("src/a.js", src)),
        shape(&extract("src/a.ts", src))
    );
}

#[test]
fn fallback_stats_count_reparses_and_kept_tsx_trees() {
    let fe = TypeScriptFrontend::new();
    let run = |path: &str, src: &str| {
        fe.extract(src.as_bytes(), &FileCtx::new(path, "oid"))
            .unwrap();
        fe.fallback_stats()
    };
    assert_eq!(run("a.js", "function f(){}\n"), FallbackStats::default());
    assert_eq!(
        run("b.ts", "function f(){ f( }\n"),
        FallbackStats::default()
    );
    assert_eq!(
        run("c.js", "function f(){ f( }\n"),
        FallbackStats {
            reparsed: 1,
            tsx_kept: 0
        }
    );
    assert_eq!(
        run("d.js", JSX_IN_JS),
        FallbackStats {
            reparsed: 2,
            tsx_kept: 1
        }
    );
}
