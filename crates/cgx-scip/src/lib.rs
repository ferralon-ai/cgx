//! # cgx-scip
//!
//! Read-only SCIP (Sourcegraph Code Intelligence Protocol) ingestion for cgx,
//! Phase-2 part **P1**: a hand-rolled protobuf wire decoder ([`wire`]), a typed
//! [`ScipIndex`] model, and a SCIP-symbol → cgx-qualified-name mapper
//! ([`symbol`]).
//!
//! ## Why hand-rolled (decision R1)
//!
//! cgx forbids new dependencies; this crate adds **zero** (`prost`, `protobuf`,
//! and the official `scip` crate are all banned). The decoder reads only the
//! narrow field subset cgx needs (wire types 0 and 2) and is isolated behind the
//! typed [`ScipIndex`] so a future prost-backed reader could replace [`wire`]
//! without touching any caller. [`ScipIndex::parse`] is that swap point.
//!
//! ## What P1 delivers
//!
//! - [`ScipIndex`] / [`ScipDocument`] / [`ScipOccurrence`] / [`ScipSymbolInfo`]
//!   — the typed model, parsed from `&[u8]`.
//! - [`symbol::map_symbol`] — SCIP symbol string → cgx qname + `(pkg, version)`.
//! - [`symbol::classify`] — trait-member vs free/inherent (gotcha 1).
//! - [`ScipResolver`] — the read-side index the P2 re-label pass consumes:
//!   [`ScipResolver::def_sites`], [`ScipResolver::refs_in_doc`],
//!   [`ScipResolver::map_symbol`], [`ScipResolver::classify`]. All returned
//!   orders are deterministic (sorted).
//!
//! P1 does **not** implement the re-label merge, the CLI `--scip` flag, CHA, or
//! RTA — those are later phases.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod symbol;
pub mod wire;

use std::collections::BTreeMap;

use wire::{Reader, WireError, WireType};

pub use symbol::{MappedSymbol, SymClass};

/// `Occurrence.symbol_roles` bit for a definition site (`Definition = 0x1`).
pub const SYMBOL_ROLE_DEFINITION: i32 = 0x1;

/// Errors surfaced while parsing a `.scip` byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipError {
    /// The underlying protobuf wire stream was malformed.
    Wire(WireError),
}

impl std::fmt::Display for ScipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScipError::Wire(e) => write!(f, "scip wire error: {e}"),
        }
    }
}

impl std::error::Error for ScipError {}

impl From<WireError> for ScipError {
    fn from(e: WireError) -> Self {
        ScipError::Wire(e)
    }
}

/// A source-span range within a document. Mirrors SCIP's two range encodings
/// (single-line `[startLine, startChar, endChar]` and multi-line
/// `[startLine, startChar, endLine, endChar]`) normalized to four coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ScipRange {
    /// 0-based start line.
    pub start_line: i32,
    /// 0-based start character (UTF-16/UTF-8 per the document's encoding).
    pub start_char: i32,
    /// 0-based end line.
    pub end_line: i32,
    /// 0-based end character (exclusive).
    pub end_char: i32,
}

impl ScipRange {
    fn from_ints(ints: &[i32]) -> ScipRange {
        match ints {
            // single-line: [startLine, startChar, endChar]
            [sl, sc, ec] => ScipRange {
                start_line: *sl,
                start_char: *sc,
                end_line: *sl,
                end_char: *ec,
            },
            // multi-line: [startLine, startChar, endLine, endChar]
            [sl, sc, el, ec, ..] => ScipRange {
                start_line: *sl,
                start_char: *sc,
                end_line: *el,
                end_char: *ec,
            },
            _ => ScipRange::default(),
        }
    }
}

/// One occurrence (a definition or reference) of a symbol in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipOccurrence {
    /// The SCIP symbol string (see [`symbol`] for the grammar).
    pub symbol: String,
    /// Bitset of `SymbolRole`s; `& SYMBOL_ROLE_DEFINITION` marks a definition.
    pub symbol_roles: i32,
    /// The source span of this occurrence.
    pub range: ScipRange,
}

impl ScipOccurrence {
    /// True when this occurrence is the symbol's definition site.
    pub fn is_definition(&self) -> bool {
        self.symbol_roles & SYMBOL_ROLE_DEFINITION != 0
    }
}

/// Metadata about a symbol defined in (or referenced by) the index. Carries the
/// `kind` needed for trait-member classification (gotcha 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipSymbolInfo {
    /// The SCIP symbol string.
    pub symbol: String,
    /// SCIP `Kind` enum value (e.g. Method, Trait). 0 = `UnspecifiedKind`.
    pub kind: i32,
    /// The enclosing symbol string, when SCIP records one (post PR#18758).
    pub enclosing_symbol: String,
}

/// One source document and its occurrences + locally-defined symbols.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScipDocument {
    /// Path relative to the index's `project_root`.
    pub relative_path: String,
    /// Every definition/reference occurrence in document order.
    pub occurrences: Vec<ScipOccurrence>,
    /// Symbols defined in this document (post PR#18758, locals included).
    pub symbols: Vec<ScipSymbolInfo>,
}

/// Index-level metadata (project root + producing tool name).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScipMetadata {
    /// Absolute root the document `relative_path`s are relative to.
    pub project_root: String,
    /// Producing tool name (e.g. `rust-analyzer`).
    pub tool: String,
}

/// The typed, parsed `.scip` index — cgx's swap-point interface over the wire
/// format. A future prost-based reader would produce the same shape from
/// [`ScipIndex::parse`] without changing any consumer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScipIndex {
    /// Index metadata.
    pub metadata: ScipMetadata,
    /// All indexed documents.
    pub documents: Vec<ScipDocument>,
    /// Symbols defined in other crates/repos (cross-crate; GM-14 source).
    pub external_symbols: Vec<ScipSymbolInfo>,
}

// --- SCIP `.proto` field numbers (the narrow subset cgx decodes) -------------
// Index { metadata=1, documents=2, external_symbols=3 }
const F_INDEX_METADATA: u32 = 1;
const F_INDEX_DOCUMENTS: u32 = 2;
const F_INDEX_EXTERNAL_SYMBOLS: u32 = 3;
// Metadata { version=1, tool_info=2, project_root=3, text_document_encoding=4 }
const F_META_PROJECT_ROOT: u32 = 3;
const F_META_TOOL_INFO: u32 = 2;
// ToolInfo { name=1, version=2, arguments=3 }
const F_TOOL_NAME: u32 = 1;
// Document { relative_path=1, occurrences=2, symbols=3, language=4 }
const F_DOC_RELATIVE_PATH: u32 = 1;
const F_DOC_OCCURRENCES: u32 = 2;
const F_DOC_SYMBOLS: u32 = 3;
// Occurrence { range=1, symbol=2, symbol_roles=3, ..., single_line_range=8?,
//   ... } — rust-analyzer emits packed int32 range in field 1.
const F_OCC_RANGE: u32 = 1;
const F_OCC_SYMBOL: u32 = 2;
const F_OCC_SYMBOL_ROLES: u32 = 3;
// SymbolInformation { symbol=1, documentation=3, relationships=4, kind=5,
//   display_name=6, signature_documentation=7, enclosing_symbol=8 }
const F_SYM_SYMBOL: u32 = 1;
const F_SYM_KIND: u32 = 5;
const F_SYM_ENCLOSING: u32 = 8;

impl ScipIndex {
    /// Parse a `.scip` protobuf byte stream into the typed model.
    ///
    /// Unknown fields are skipped, so a newer SCIP emitter that adds fields is
    /// tolerated. Malformed wire data returns [`ScipError`] rather than panics.
    pub fn parse(bytes: &[u8]) -> Result<ScipIndex, ScipError> {
        let mut index = ScipIndex::default();
        let mut r = Reader::new(bytes);
        while let Some(h) = r.next_field()? {
            match (h.number, h.wire_type) {
                (F_INDEX_METADATA, WireType::Len) => {
                    index.metadata = parse_metadata(r.read_message()?)?;
                }
                (F_INDEX_DOCUMENTS, WireType::Len) => {
                    index.documents.push(parse_document(r.read_message()?)?);
                }
                (F_INDEX_EXTERNAL_SYMBOLS, WireType::Len) => {
                    index
                        .external_symbols
                        .push(parse_symbol_info(r.read_message()?)?);
                }
                (_, wt) => r.skip(wt)?,
            }
        }
        Ok(index)
    }
}

fn parse_metadata(mut r: Reader<'_>) -> Result<ScipMetadata, ScipError> {
    let mut meta = ScipMetadata::default();
    while let Some(h) = r.next_field()? {
        match (h.number, h.wire_type) {
            (F_META_PROJECT_ROOT, WireType::Len) => meta.project_root = r.read_str()?.to_string(),
            (F_META_TOOL_INFO, WireType::Len) => meta.tool = parse_tool_name(r.read_message()?)?,
            (_, wt) => r.skip(wt)?,
        }
    }
    Ok(meta)
}

fn parse_tool_name(mut r: Reader<'_>) -> Result<String, ScipError> {
    let mut name = String::new();
    while let Some(h) = r.next_field()? {
        match (h.number, h.wire_type) {
            (F_TOOL_NAME, WireType::Len) => name = r.read_str()?.to_string(),
            (_, wt) => r.skip(wt)?,
        }
    }
    Ok(name)
}

fn parse_document(mut r: Reader<'_>) -> Result<ScipDocument, ScipError> {
    let mut doc = ScipDocument::default();
    while let Some(h) = r.next_field()? {
        match (h.number, h.wire_type) {
            (F_DOC_RELATIVE_PATH, WireType::Len) => doc.relative_path = r.read_str()?.to_string(),
            (F_DOC_OCCURRENCES, WireType::Len) => {
                doc.occurrences.push(parse_occurrence(r.read_message()?)?);
            }
            (F_DOC_SYMBOLS, WireType::Len) => doc.symbols.push(parse_symbol_info(r.read_message()?)?),
            (_, wt) => r.skip(wt)?,
        }
    }
    Ok(doc)
}

fn parse_occurrence(mut r: Reader<'_>) -> Result<ScipOccurrence, ScipError> {
    let mut symbol = String::new();
    let mut symbol_roles = 0i32;
    let mut range_ints: Vec<i32> = Vec::new();
    while let Some(h) = r.next_field()? {
        match (h.number, h.wire_type) {
            (F_OCC_SYMBOL, WireType::Len) => symbol = r.read_str()?.to_string(),
            (F_OCC_SYMBOL_ROLES, WireType::Varint) => symbol_roles = r.read_i32()?,
            // Range is a packed `repeated int32` (wire type 2 = packed scalars).
            (F_OCC_RANGE, WireType::Len) => {
                let mut packed = r.read_message()?;
                while !packed.is_empty() {
                    range_ints.push(packed.read_i32()?);
                }
            }
            // Tolerate a non-packed range (each int32 as its own varint field).
            (F_OCC_RANGE, WireType::Varint) => range_ints.push(r.read_i32()?),
            (_, wt) => r.skip(wt)?,
        }
    }
    Ok(ScipOccurrence {
        symbol,
        symbol_roles,
        range: ScipRange::from_ints(&range_ints),
    })
}

fn parse_symbol_info(mut r: Reader<'_>) -> Result<ScipSymbolInfo, ScipError> {
    let mut info = ScipSymbolInfo {
        symbol: String::new(),
        kind: 0,
        enclosing_symbol: String::new(),
    };
    while let Some(h) = r.next_field()? {
        match (h.number, h.wire_type) {
            (F_SYM_SYMBOL, WireType::Len) => info.symbol = r.read_str()?.to_string(),
            (F_SYM_KIND, WireType::Varint) => info.kind = r.read_i32()?,
            (F_SYM_ENCLOSING, WireType::Len) => info.enclosing_symbol = r.read_str()?.to_string(),
            (_, wt) => r.skip(wt)?,
        }
    }
    Ok(info)
}

/// A resolved definition location: which document and where in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DefLoc {
    /// The defining document's `relative_path`.
    pub relative_path: String,
    /// The definition span.
    pub range: ScipRange,
    /// The raw SCIP symbol that produced this definition.
    pub symbol: String,
}

/// The read-side index the P2 re-label pass consumes.
///
/// Built once from a [`ScipIndex`]; all accessors return deterministically
/// sorted slices so downstream output stays byte-stable (IF-8).
///
/// Construction joins occurrences into:
/// - `def_sites`: cgx-qname → every definition location (multiplicity exposes
///   the #18772 collision — gotcha 2);
/// - `refs_by_doc`: relpath → every (range, symbol) reference occurrence;
/// - a symbol-kind table over local + external [`ScipSymbolInfo`] for
///   trait-member classification (gotcha 1).
#[derive(Debug, Clone)]
pub struct ScipResolver {
    index: ScipIndex,
    def_sites: BTreeMap<String, Vec<DefLoc>>,
    refs_by_doc: BTreeMap<String, Vec<(ScipRange, String)>>,
    /// symbol-string → SCIP `Kind` (joined from local + external symbol info).
    kind_by_symbol: BTreeMap<String, i32>,
    /// symbol-string → enclosing-symbol string.
    enclosing_by_symbol: BTreeMap<String, String>,
}

impl ScipResolver {
    /// Build the resolver from a parsed index. O(occurrences log) — sorts every
    /// accessor's backing vector once so reads are already canonical.
    pub fn new(index: ScipIndex) -> ScipResolver {
        let mut def_sites: BTreeMap<String, Vec<DefLoc>> = BTreeMap::new();
        let mut refs_by_doc: BTreeMap<String, Vec<(ScipRange, String)>> = BTreeMap::new();
        let mut kind_by_symbol: BTreeMap<String, i32> = BTreeMap::new();
        let mut enclosing_by_symbol: BTreeMap<String, String> = BTreeMap::new();

        for info in index
            .documents
            .iter()
            .flat_map(|d| d.symbols.iter())
            .chain(index.external_symbols.iter())
        {
            if !info.symbol.is_empty() {
                kind_by_symbol.insert(info.symbol.clone(), info.kind);
                if !info.enclosing_symbol.is_empty() {
                    enclosing_by_symbol.insert(info.symbol.clone(), info.enclosing_symbol.clone());
                }
            }
        }

        for doc in &index.documents {
            let refs = refs_by_doc.entry(doc.relative_path.clone()).or_default();
            for occ in &doc.occurrences {
                if occ.symbol.is_empty() || symbol::is_local(&occ.symbol) {
                    // Local symbols are intra-document only; still join refs so
                    // a same-doc lexical resolution is available, but never as a
                    // cross-file def site.
                }
                if occ.is_definition() {
                    if let Some(mapped) = symbol::map_symbol(&occ.symbol) {
                        def_sites.entry(mapped.qname).or_default().push(DefLoc {
                            relative_path: doc.relative_path.clone(),
                            range: occ.range,
                            symbol: occ.symbol.clone(),
                        });
                    }
                } else {
                    refs.push((occ.range, occ.symbol.clone()));
                }
            }
        }

        for locs in def_sites.values_mut() {
            locs.sort();
            locs.dedup();
        }
        for refs in refs_by_doc.values_mut() {
            refs.sort();
        }

        ScipResolver {
            index,
            def_sites,
            refs_by_doc,
            kind_by_symbol,
            enclosing_by_symbol,
        }
    }

    /// Parse-and-build in one step.
    pub fn from_bytes(bytes: &[u8]) -> Result<ScipResolver, ScipError> {
        Ok(ScipResolver::new(ScipIndex::parse(bytes)?))
    }

    /// The underlying parsed index.
    pub fn index(&self) -> &ScipIndex {
        &self.index
    }

    /// Every definition location for a cgx qname, sorted. Empty when the qname
    /// has no SCIP definition. **`len() > 1` flags the #18772 collision**
    /// (gotcha 2) — downstream must cap such a symbol at `probable`.
    pub fn def_sites(&self, qname: &str) -> &[DefLoc] {
        self.def_sites.get(qname).map_or(&[], |v| v.as_slice())
    }

    /// Every reference occurrence in a document, sorted by (range, symbol).
    /// Empty when the document is absent or has no references.
    pub fn refs_in_doc(&self, relpath: &str) -> &[(ScipRange, String)] {
        self.refs_by_doc.get(relpath).map_or(&[], |v| v.as_slice())
    }

    /// Map a SCIP symbol string to a cgx qname + `(pkg, version)`. `None` for an
    /// unparseable or `local` symbol (locals carry no cross-file qname).
    pub fn map_symbol(&self, symbol: &str) -> Option<MappedSymbol> {
        symbol::map_symbol(symbol)
    }

    /// Classify a SCIP symbol as trait-member vs free/inherent (gotcha 1). Uses
    /// the joined symbol-kind / enclosing-symbol tables when available, falling
    /// back to the descriptor grammar otherwise.
    pub fn classify(&self, symbol: &str) -> SymClass {
        symbol::classify(symbol, &self.kind_by_symbol, &self.enclosing_by_symbol)
    }
}
