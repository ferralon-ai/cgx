//! SARIF 2.1.0 structural-validity tests for the `cgx-cli` output layer.
//!
//! These check the emitted document against the required shape of the OASIS
//! SARIF 2.1.0 schema (the fields a consumer like GitHub Advanced Security or the
//! VS Code SARIF viewer requires): `$schema`, `version`, a `runs` array, each run
//! with `tool.driver.name`, and each result with a `ruleId`, `message.text`, and
//! a `physicalLocation`. We assert structure (not byte content) so the test is
//! robust to additive property changes.

use cgx_core::{
    Confidence, EdgeCondition, EdgeId, EdgeKind, EdgeRecord, NodeId, NodeRecord, Tier, Visibility,
};
use cgx_query::{FreshnessEnvelope, NeighborResult};
use serde_json::Value;

use cgx_cli::output::{sarif_document, ResultSet};

/// A fixed, fully-populated freshness envelope: these tests assert SARIF
/// structure, not freshness semantics (that lives in `freshness.rs`/`cli.rs`).
fn fresh() -> FreshnessEnvelope {
    FreshnessEnvelope::new(
        Some("aaaa1111".into()),
        Some("aaaa1111".into()),
        Some(("aaaa1111".to_owned(), 0)),
    )
}

fn node(id: u32, fqn: &str, file: &str, line: u32) -> NodeRecord {
    NodeRecord {
        id: NodeId(id),
        kind: cgx_core::SymbolKind::Function,
        fqn: fqn.to_string(),
        file: file.to_string(),
        line_start: line,
        line_end: line,
        lang: "rust".to_string(),
        visibility: Visibility::Public,
        is_abstract: false,
        entrypoint_kind: None,
        signature: None,
        own_effects: cgx_core::EffectSet::new(),
        transitive_effects: cgx_core::EffectSet::new(),
        unresolved_calls: 0,
    }
}

fn edge(id: u32, src: u32, dst: u32, cond: EdgeCondition, conf: Confidence) -> EdgeRecord {
    EdgeRecord {
        id: EdgeId(id),
        src: NodeId(src),
        dst: NodeId(dst),
        kind: EdgeKind::Calls,
        condition: cond,
        confidence: conf,
        tier: Tier::ScopeGraph,
        rule: "import-ref".to_string(),
        site_id: None,
        stmt_index: None,
        cut_markers: Default::default(),
        implicit: None,
        candidate_group: None,
        established_by: None,
        cfg_condition: None,
        macro_origin: None,
        transform: None,
    }
}

fn sample_neighbors() -> ResultSet {
    let n = node(2, "auth::validate", "src/auth.rs", 42);
    let e = edge(0, 1, 2, EdgeCondition::Exception, Confidence::Probable);
    ResultSet::Neighbors {
        results: vec![NeighborResult {
            node: n,
            depth: 1,
            via: e,
            condition: EdgeCondition::Exception,
            confidence: Confidence::Probable,
            min_confidence_on_path: Confidence::Probable,
            exception_transient: true,
        }],
        forest: None,
    }
}

#[test]
fn sarif_has_required_top_level_shape() {
    let doc = sarif_document(
        "callers",
        &sample_neighbors(),
        false,
        &cgx_query::ApproximationContract::exact(),
        &fresh(),
    );
    assert_eq!(doc["version"], Value::String("2.1.0".into()));
    assert!(doc["$schema"].as_str().unwrap().contains("sarif"));
    let runs = doc["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    let driver = &runs[0]["tool"]["driver"];
    assert_eq!(driver["name"], Value::String("cgx".into()));
    assert!(driver["rules"].as_array().is_some());
}

#[test]
fn sarif_result_has_rule_message_and_physical_location() {
    let doc = sarif_document(
        "callers",
        &sample_neighbors(),
        false,
        &cgx_query::ApproximationContract::exact(),
        &fresh(),
    );
    let result = &doc["runs"][0]["results"][0];
    assert_eq!(result["ruleId"], Value::String("cgx/callers".into()));
    assert!(result["message"]["text"].is_string());
    let loc = &result["locations"][0]["physicalLocation"];
    assert_eq!(
        loc["artifactLocation"]["uri"],
        Value::String("src/auth.rs".into())
    );
    assert_eq!(loc["region"]["startLine"], Value::Number(42.into()));
}

#[test]
fn sarif_surfaces_confidence_and_condition_properties() {
    let doc = sarif_document(
        "callers",
        &sample_neighbors(),
        false,
        &cgx_query::ApproximationContract::exact(),
        &fresh(),
    );
    let props = &doc["runs"][0]["results"][0]["properties"];
    assert_eq!(props["confidence"], Value::String("probable".into()));
    assert_eq!(props["edgeCondition"], Value::String("exception".into()));
    assert_eq!(props["exceptionTransient"], Value::Bool(true));
}

#[test]
fn vacuous_pass_adds_note_level_result() {
    let doc = sarif_document(
        "paths",
        &ResultSet::Neighbors {
            results: vec![],
            forest: None,
        },
        true,
        &cgx_query::ApproximationContract::exact(),
        &fresh(),
    );
    let results = doc["runs"][0]["results"].as_array().unwrap();
    // The vacuous note plus the two always-present notes: the approximation
    // contract (A3/A4) and the index-freshness envelope, which is emitted the same
    // way for the same reason — an answer document states what it can be wrong
    // about and what state it describes.
    assert_eq!(
        results.len(),
        3,
        "vacuous note + approximation note + freshness note: {results:?}"
    );
    assert_eq!(
        results[0]["ruleId"],
        Value::String("cgx/vacuous-assertion".into())
    );
    assert_eq!(results[0]["level"], Value::String("note".into()));
    assert_eq!(
        results[1]["ruleId"],
        Value::String("cgx/approximation-contract".into())
    );
    assert_eq!(
        results[2]["ruleId"],
        Value::String("cgx/index-freshness".into())
    );
    assert_eq!(results[2]["level"], Value::String("note".into()));
    assert_eq!(results[2]["properties"]["stale"], Value::Bool(false));
}

#[test]
fn sarif_is_valid_json_and_serializes_stably() {
    // Two serializations of the same document are byte-identical (determinism).
    let a = sarif_document(
        "callees",
        &sample_neighbors(),
        false,
        &cgx_query::ApproximationContract::exact(),
        &fresh(),
    )
    .to_string();
    let b = sarif_document(
        "callees",
        &sample_neighbors(),
        false,
        &cgx_query::ApproximationContract::exact(),
        &fresh(),
    )
    .to_string();
    assert_eq!(a, b);
    // And it round-trips as JSON.
    let _: Value = serde_json::from_str(&a).expect("valid json");
}
