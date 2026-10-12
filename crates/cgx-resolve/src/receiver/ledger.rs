//! Per-site outcome recording and the node counters it yields.
//!
//! Forward counters (`typed_out`) are kept per caller node. The backward count
//! `typed_away_in(d)` is, for a method def `d` with short name `m` in language
//! `l`, the number of narrowed sites calling `m` in `l` minus those whose kept
//! set contains `d`: same-name sites elsewhere that receiver typing bound
//! without `d`. Proven-external dangles count as narrowed sites that kept
//! nothing. Everything iterated is a `BTreeMap`; the fan-out memo is lookup-only.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cgx_core::id::NodeId;
use cgx_core::node::{NarrowingCounts, SymbolKind};

use crate::symtab::{DefEntry, SymbolTable};

use super::stats::{trace, Cat, PrecisionStats};
use super::{LegacyReason, Site};

#[derive(Debug, Default)]
pub(crate) struct Ledger {
    stats: PrecisionStats,
    counts: BTreeMap<NodeId, NarrowingCounts>,
    narrowed: BTreeMap<(String, String), u32>,
    kept: BTreeMap<NodeId, u32>,
    fanout: HashMap<(String, String), usize>,
    trace: bool,
}

impl Ledger {
    pub(crate) fn new(trace: bool) -> Self {
        Ledger {
            trace,
            ..Ledger::default()
        }
    }

    pub(crate) fn tracing(&self) -> bool {
        self.trace
    }

    pub(crate) fn stats_mut(&mut self) -> &mut PrecisionStats {
        &mut self.stats
    }

    /// The same-name method fan-out of `m` in `lang` without narrowing:
    /// exactly the Step-3 candidate set's size.
    pub(crate) fn fanout(&mut self, table: &SymbolTable, lang: &str, m: &str) -> usize {
        let key = (lang.to_owned(), m.to_owned());
        *self.fanout.entry(key).or_insert_with(|| {
            table
                .defs_by_short(m)
                .iter()
                .filter(|d| is_method_candidate(d, lang))
                .count()
        })
    }

    pub(crate) fn typed(
        &mut self,
        table: &SymbolTable,
        site: &Site<'_>,
        cat: Cat,
        rule: &str,
        hits: &[&DefEntry],
    ) {
        let fanout = self.site_fanout(table, site);
        let c = self.stats.cat(cat);
        c.sites += 1;
        c.edges_before += fanout;
        c.edges_after += hits.len();
        self.counts.entry(site.caller_id).or_default().typed_out += 1;
        self.note_narrowed(site);
        let kept: BTreeSet<NodeId> = hits.iter().map(|d| d.node_id).collect();
        for k in kept {
            *self.kept.entry(k).or_default() += 1;
        }
        if self.trace {
            let targets: Vec<&str> = hits.iter().map(|h| h.fqn.as_str()).collect();
            let tail = format!("{fanout}->{}\t{}", hits.len(), targets.join(","));
            self.trace_site(rule, site, &tail);
        }
    }

    pub(crate) fn external(&mut self, table: &SymbolTable, site: &Site<'_>, cat: Cat, label: &str) {
        let fanout = self.site_fanout(table, site);
        let c = self.stats.cat(cat);
        c.dangling += 1;
        c.dangling_edges_before += fanout;
        self.note_narrowed(site);
        if self.trace {
            self.trace_site(&format!("dangle:{label}"), site, &format!("{fanout}->0"));
        }
    }

    /// A classified site handed back to the same-name set.
    pub(crate) fn fallback(
        &mut self,
        table: &SymbolTable,
        site: &Site<'_>,
        cat: Cat,
        reason: LegacyReason,
    ) {
        let fanout = self.site_fanout(table, site);
        let c = match reason {
            LegacyReason::UnknownBase => &mut self.stats.unknown_base,
            _ => self.stats.cat(cat),
        };
        c.fallback += 1;
        c.fallback_edges += fanout;
        if self.trace {
            let label = match reason {
                LegacyReason::UnknownBase => "fallback:unknown-base",
                _ => "fallback:closed",
            };
            self.trace_site(label, site, &fanout.to_string());
        }
    }

    /// A site that reached the same-name step unclassified, by receiver shape.
    pub(crate) fn legacy(&mut self, site: &Site<'_>, cat: Cat, label: &str, fanout: usize) {
        let c = self.stats.cat(cat);
        c.sites += 1;
        c.edges_before += fanout;
        c.edges_after += fanout;
        if self.trace {
            self.trace_site(label, site, &fanout.to_string());
        }
    }

    /// The stats and the per-node counters, with each method def's
    /// `typed_away_in` computed from the narrowed-site totals.
    pub(crate) fn finish(
        mut self,
        table: &SymbolTable,
    ) -> (PrecisionStats, BTreeMap<NodeId, NarrowingCounts>) {
        for ((lang, m), n) in &self.narrowed {
            for d in table
                .defs_by_short(m)
                .iter()
                .filter(|d| is_method_candidate(d, lang))
            {
                let kept = self.kept.get(&d.node_id).copied().unwrap_or(0);
                let away = n.saturating_sub(kept);
                if away > 0 {
                    self.counts.entry(d.node_id).or_default().typed_away_in += away;
                }
            }
        }
        (self.stats, self.counts)
    }

    fn note_narrowed(&mut self, site: &Site<'_>) {
        let key = (site.file.lang.clone(), site.method().to_owned());
        *self.narrowed.entry(key).or_default() += 1;
    }

    fn site_fanout(&mut self, table: &SymbolTable, site: &Site<'_>) -> usize {
        self.fanout(table, &site.file.lang, site.method())
    }

    fn trace_site(&self, label: &str, site: &Site<'_>, tail: &str) {
        trace(
            label,
            &site.file.path,
            site.raw.span.line,
            &site.raw.name_path,
            tail,
        );
    }
}

/// Membership of the Step-3 same-name method candidate set.
pub(crate) fn is_method_candidate(d: &DefEntry, lang: &str) -> bool {
    matches!(d.kind, SymbolKind::Method | SymbolKind::Function) && d.lang == lang
}
