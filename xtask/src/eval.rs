// xtask eval — fixture eval harness
//
// Compares extracted FileFacts (from cgx's frontend) against hand-written golden
// YAML files. Prints per-label FP/FN rates. Exits 0 on full match, 1 on any mismatch.
//
// Usage:
//   cargo xtask eval --fixtures fixtures/ --goldens fixtures/goldens/ --lang rust
//   cargo xtask eval --fixtures fixtures/ --goldens fixtures/goldens/ --lang ts
//   cargo xtask eval --fixtures fixtures/ --goldens fixtures/goldens/  # both
//
// In Phase 1 (WP-02 bootstrap): the harness validates golden format and structure
// without calling a real frontend (that is WP-04/05). When --actual-facts <dir>
// is provided, it diffs actual extracted facts against the goldens.

use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Args)]
pub struct EvalArgs {
    /// Root of the fixtures directory (contains rust-sample/, ts-sample/)
    #[arg(long, default_value = "fixtures")]
    fixtures: PathBuf,

    /// Root of the goldens directory (contains goldens/rust-sample/, goldens/ts-sample/)
    #[arg(long, default_value = "fixtures/goldens")]
    goldens: PathBuf,

    /// Language to evaluate: rust, ts, or all
    #[arg(long, default_value = "all")]
    lang: String,

    /// Optional: directory with actual extracted facts (YAML) to diff against goldens.
    /// When absent, only validates golden structure.
    #[arg(long)]
    actual_facts: Option<PathBuf>,

    /// If set, fail on any FP or FN (default: print and continue)
    #[arg(long)]
    strict: bool,
}

// ---------------------------------------------------------------------------
// Golden schema — mirrors fixtures/README.md annotation format
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Golden {
    pub file: String,
    #[serde(default)]
    pub defs: Vec<DefRecord>,
    #[serde(default)]
    pub refs: Vec<RefRecord>,
    #[serde(default)]
    pub imports: Vec<ImportRecord>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct DefRecord {
    pub fqn: String,
    pub kind: String,
    pub line: u32,
    pub visibility: String,
    #[serde(default)]
    pub is_abstract: bool,
    #[serde(default)]
    pub entrypoint_kind: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct RefRecord {
    pub caller: String,
    pub callee: String,
    pub kind: String,
    pub edge_condition: String,
    pub confidence: String,
    pub line: u32,
    #[serde(default)]
    pub cut_marker: Option<String>,
    #[serde(default)]
    pub implicit: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct ImportRecord {
    pub from: String,
    pub name: String,
    #[serde(default)]
    pub re_export: bool,
    #[serde(default)]
    pub alias: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-label counters for FP/FN reporting
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
struct LabelStats {
    tp: usize,
    fp: usize,
    r#fn: usize,
}

impl LabelStats {
    fn precision(&self) -> f64 {
        let denom = self.tp + self.fp;
        if denom == 0 {
            1.0
        } else {
            self.tp as f64 / denom as f64
        }
    }

    fn recall(&self) -> f64 {
        let denom = self.tp + self.r#fn;
        if denom == 0 {
            1.0
        } else {
            self.tp as f64 / denom as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Eval logic
// ---------------------------------------------------------------------------

pub fn run(args: EvalArgs) -> Result<()> {
    let langs: Vec<&str> = match args.lang.as_str() {
        "rust" => vec!["rust"],
        "ts" => vec!["ts"],
        "all" => vec!["rust", "ts"],
        other => anyhow::bail!("unknown --lang: {}. Use rust, ts, or all", other),
    };

    let mut total_errors = 0usize;

    for lang in langs {
        let golden_dir = args.goldens.join(format!("{}-sample", lang));
        if !golden_dir.exists() {
            eprintln!("warning: golden dir not found: {}", golden_dir.display());
            continue;
        }

        println!("\n=== Evaluating: {} ===", lang);

        let goldens = load_goldens(&golden_dir)
            .with_context(|| format!("loading goldens from {}", golden_dir.display()))?;

        if goldens.is_empty() {
            println!("  (no goldens found)");
            continue;
        }

        if let Some(ref actual_dir) = args.actual_facts {
            let actual_subdir = actual_dir.join(format!("{}-sample", lang));
            if actual_subdir.exists() {
                let errors = diff_against_actual(&goldens, &actual_subdir, args.strict)?;
                total_errors += errors;
            } else {
                println!(
                    "  no actual facts found at {} — structure-only check",
                    actual_subdir.display()
                );
                let errors = validate_golden_structure(&goldens);
                total_errors += errors;
            }
        } else {
            let errors = validate_golden_structure(&goldens);
            total_errors += errors;
        }
    }

    if total_errors > 0 {
        anyhow::bail!("{} evaluation error(s) found", total_errors);
    }

    println!("\nAll golden checks passed.");
    Ok(())
}

fn load_goldens(dir: &Path) -> Result<HashMap<String, Golden>> {
    let mut map = HashMap::new();
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "yaml") {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let golden: Golden = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", path.display()))?;
            let key = path.file_stem().unwrap().to_string_lossy().into_owned();
            map.insert(key, golden);
        }
    }
    Ok(map)
}

fn validate_golden_structure(goldens: &HashMap<String, Golden>) -> usize {
    let mut errors = 0;
    for (name, golden) in goldens {
        // Validate: every ref has a caller that appears in defs (or is a well-known external)
        let def_fqns: std::collections::HashSet<&str> =
            golden.defs.iter().map(|d| d.fqn.as_str()).collect();

        for r in &golden.refs {
            // caller should be in defs (cross-file callers are allowed to be absent)
            let caller_in_defs = def_fqns.contains(r.caller.as_str());
            let is_cross_file = !r.caller.starts_with(
                &golden
                    .file
                    .replace("src/", "")
                    .replace(".rs", "")
                    .replace(".ts", ""),
            );

            if !caller_in_defs && !is_cross_file {
                eprintln!(
                    "  [WARN] {}: ref caller '{}' not found in defs",
                    name, r.caller
                );
                // Not counted as error — callers may be from other files
            }

            // Validate edge_condition is a known value
            let valid_conditions = ["always", "conditional", "loop", "exception", "panic"];
            if !valid_conditions.contains(&r.edge_condition.as_str()) {
                eprintln!(
                    "  [ERROR] {}: unknown edge_condition '{}' on ref {}->{}",
                    name, r.edge_condition, r.caller, r.callee
                );
                errors += 1;
            }

            // Validate confidence is a known value
            let valid_confidence = ["certain", "probable", "possible"];
            if !valid_confidence.contains(&r.confidence.as_str()) {
                eprintln!(
                    "  [ERROR] {}: unknown confidence '{}' on ref {}->{}",
                    name, r.confidence, r.caller, r.callee
                );
                errors += 1;
            }

            // Validate kind is a known value
            let valid_kinds = [
                "calls",
                "calls:virtual",
                "calls:closure",
                "calls:callback",
                "calls:async",
                "calls:indirect",
                "spawns",
            ];
            if !valid_kinds.contains(&r.kind.as_str()) {
                eprintln!(
                    "  [ERROR] {}: unknown ref kind '{}' on ref {}->{}",
                    name, r.kind, r.caller, r.callee
                );
                errors += 1;
            }
        }

        // Validate: every def has a known kind
        let valid_kinds = [
            "function",
            "method",
            "type",
            "field",
            "variable",
            "module",
            "constant",
            "macro",
            "lambda",
            "entrypoint",
        ];
        for d in &golden.defs {
            if !valid_kinds.contains(&d.kind.as_str()) {
                eprintln!(
                    "  [ERROR] {}: unknown def kind '{}' for {}",
                    name, d.kind, d.fqn
                );
                errors += 1;
            }
        }

        println!(
            "  {} : {} defs, {} refs, {} imports — structure OK",
            name,
            golden.defs.len(),
            golden.refs.len(),
            golden.imports.len()
        );
    }
    errors
}

fn diff_against_actual(
    goldens: &HashMap<String, Golden>,
    actual_dir: &Path,
    strict: bool,
) -> Result<usize> {
    let actuals = load_goldens(actual_dir)?;
    let mut total_errors = 0;

    // Per-label stats
    let mut edge_condition_stats: HashMap<String, LabelStats> = HashMap::new();
    let mut kind_stats: HashMap<String, LabelStats> = HashMap::new();
    let mut confidence_stats: HashMap<String, LabelStats> = HashMap::new();

    for (name, golden) in goldens {
        let actual = match actuals.get(name) {
            Some(a) => a,
            None => {
                eprintln!("  [MISSING] no actual facts for {}", name);
                total_errors += 1;
                continue;
            }
        };

        // Diff refs: use (caller, callee, kind, edge_condition) as identity
        let golden_refs: std::collections::HashSet<_> = golden
            .refs
            .iter()
            .map(|r| (&r.caller, &r.callee, &r.kind, &r.edge_condition))
            .collect();
        let actual_refs: std::collections::HashSet<_> = actual
            .refs
            .iter()
            .map(|r| (&r.caller, &r.callee, &r.kind, &r.edge_condition))
            .collect();

        // True positives
        for r in golden.refs.iter() {
            let key = (&r.caller, &r.callee, &r.kind, &r.edge_condition);
            let stats = edge_condition_stats
                .entry(r.edge_condition.clone())
                .or_default();
            let kstats = kind_stats.entry(r.kind.clone()).or_default();
            let cstats = confidence_stats.entry(r.confidence.clone()).or_default();

            if actual_refs.contains(&key) {
                stats.tp += 1;
                kstats.tp += 1;
                cstats.tp += 1;
            } else {
                // False negative: in golden, not in actual
                stats.r#fn += 1;
                kstats.r#fn += 1;
                cstats.r#fn += 1;
                eprintln!(
                    "  [FN] {}: missing ref {}->{} ({}, {})",
                    name, r.caller, r.callee, r.kind, r.edge_condition
                );
                total_errors += 1;
            }
        }

        // False positives: in actual, not in golden
        for r in actual.refs.iter() {
            let key = (&r.caller, &r.callee, &r.kind, &r.edge_condition);
            if !golden_refs.contains(&key) {
                let stats = edge_condition_stats
                    .entry(r.edge_condition.clone())
                    .or_default();
                let kstats = kind_stats.entry(r.kind.clone()).or_default();
                stats.fp += 1;
                kstats.fp += 1;
                eprintln!(
                    "  [FP] {}: extra ref {}->{} ({}, {})",
                    name, r.caller, r.callee, r.kind, r.edge_condition
                );
                if strict {
                    total_errors += 1;
                }
            }
        }
    }

    // Print per-label summary
    println!("\nEdge-condition precision/recall:");
    let mut cond_labels: Vec<_> = edge_condition_stats.keys().collect();
    cond_labels.sort();
    for label in cond_labels {
        let s = &edge_condition_stats[label];
        println!(
            "  {:12} | TP={:4} FP={:4} FN={:4} | P={:.2} R={:.2}",
            label,
            s.tp,
            s.fp,
            s.r#fn,
            s.precision(),
            s.recall()
        );
    }

    println!("\nRef kind precision/recall:");
    let mut kind_labels: Vec<_> = kind_stats.keys().collect();
    kind_labels.sort();
    for label in kind_labels {
        let s = &kind_stats[label];
        println!(
            "  {:20} | TP={:4} FP={:4} FN={:4} | P={:.2} R={:.2}",
            label,
            s.tp,
            s.fp,
            s.r#fn,
            s.precision(),
            s.recall()
        );
    }

    Ok(total_errors)
}
