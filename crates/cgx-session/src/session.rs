//! [`Session`]: one repository's resident graph and the index state machine that
//! builds it.
//!
//! A host drives an index through four calls — [`Session::index_begin`],
//! [`Session::index_plan`], [`Session::index_submit`] (any number of times) and
//! [`Session::index_finish`] — and runs [`extract`] for each file the plan asks
//! for, wherever it likes (a pool of wasm instances, threads, another process).
//! Every stage body is the shared pipeline code in `cgx-index`; the session only
//! sequences it. A native caller can instead index in-process with
//! [`Session::index_native`](crate::Session::index_native), which runs the native
//! composition (`extract_and_link`) and then the same tail.
//!
//! After any index the graph stays resident, built from the linked graph in the
//! store's canonical order — never read back from the store.

use std::cell::{Cell, OnceCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use cgx_core::codec::{decode, encode};
use cgx_core::NodeId;
use cgx_frontend::{FileCtx, FileFacts, FrontendRegistry};
use cgx_index::{
    apply_cha, apply_effects, apply_rta, apply_scip_bytes, apply_sig, default_registry,
    extract_one, is_manifest_path, link_prepared, plan, probe_fragments, put_extracted,
    workdir_key, ExtractedFile, IndexOpts, IndexStats, PlannedFile, PreparedFile, SourceFile,
};
use cgx_mcp::session::overlay_difference;
use cgx_mcp::tools::{SessionProvider, SessionRef};
use cgx_mcp::{GraphSession, ToolError};
use cgx_query::GraphView;
use cgx_resolve::ResolvedGraph;
use cgx_store::{FactStore, LinkedGraph, TreeOid};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, SessionError};
use crate::ops;
use crate::store_loc::{dataflow_default, read_pointer, write_pointer, IndexPointer};

/// Opens the session's store, given the directory that holds `.cgx/`.
pub type StoreOpener<S> = Box<dyn Fn(&Path) -> cgx_store::Result<S>>;

/// What an index builds: the committed `HEAD` tree (stored, pointer advanced) or
/// the working directory (held only; its fragments are cached, its graph never
/// persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Head,
    Worktree,
}

/// `open`'s request: the repository's `HEAD` tree as the host established it
/// (`None` outside a git repository), the text of its `cgx.toml` (`None` when
/// absent), and an explicit dataflow override.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenRequest {
    #[serde(default)]
    pub head_tree: Option<String>,
    #[serde(default)]
    pub cgx_toml: Option<String>,
    #[serde(default)]
    pub dataflow: Option<bool>,
}

/// Whether the persisted index can serve the repository as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenState {
    /// The pointer names `HEAD`'s tree and its graph is stored; it is now resident.
    Fresh,
    /// A pointer exists but names another tree, or its graph is not stored.
    Stale,
    /// No index has been written.
    Missing,
}

/// `open`'s response.
#[derive(Debug, Clone, Serialize)]
pub struct OpenResponse {
    pub state: OpenState,
    /// The pointer's graph key (`null` when there is no pointer).
    pub graph_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<IndexPointer>,
}

/// The options entry of `index_begin`.
#[derive(Debug, Clone, Deserialize)]
pub struct BeginOpts {
    pub mode: Mode,
    /// `head` mode: the tree OID being indexed (the Layer-2 key). Required.
    #[serde(default)]
    pub graph_key: Option<String>,
    /// `worktree` mode: `HEAD`'s tree OID, the base the overlay is measured
    /// from. Required.
    #[serde(default)]
    pub head_tree: Option<String>,
    /// Explicit dataflow setting; when absent, `cgx_toml` decides, then the
    /// setting from the last `open`, then the default (on).
    #[serde(default)]
    pub dataflow: Option<bool>,
    #[serde(default)]
    pub cgx_toml: Option<String>,
    /// `worktree` mode: how many trailing `(path, oid)` pairs are the committed
    /// tree's entries rather than working files.
    #[serde(default)]
    pub committed: u32,
}

/// `index_begin`'s response.
#[derive(Debug, Clone, Serialize)]
pub struct BeginResponse {
    /// Indices (into the host's working list) of the files whose content the
    /// planner needs, in the order `index_plan` expects them.
    pub manifest_indices: Vec<u32>,
    /// The key the graph will be built under.
    pub graph_key: String,
}

/// The summary entry of `index_plan`'s response.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    pub files: usize,
    pub planned: usize,
    pub unsupported: usize,
    pub cached: usize,
    pub to_extract: usize,
}

/// One file the plan wants extracted: the host's index for it and the postcard
/// [`FileCtx`] to hand to [`extract`] with its content.
pub type PlanMiss = (u32, Vec<u8>);

/// The metadata entry of an [`extract`] response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractMeta {
    pub lang: String,
    pub fragment_version: u32,
}

/// One extracted file handed back to [`Session::index_submit`].
#[derive(Debug, Clone)]
pub struct Submitted {
    pub index: u32,
    pub fragment_version: u32,
    /// The postcard `FileFacts` [`extract`] returned, which is also the Layer-1
    /// fragment stored for the file.
    pub facts: Vec<u8>,
}

/// What an index did (`index_finish`'s response and the native `index` result).
#[derive(Debug, Clone, Serialize)]
pub struct IndexReport {
    pub graph_key: String,
    pub mode: Mode,
    /// The persisted graph already matched `HEAD`; nothing was rebuilt.
    pub up_to_date: bool,
    /// Whether the dataflow layer was built; `null` when `up_to_date` (the
    /// pointer does not record it).
    pub dataflow: Option<bool>,
    /// Pipeline counters; `null` when `up_to_date`.
    pub stats: Option<ReportStats>,
}

/// The pipeline counters an index reports. The `dataflow_functions_*` split is
/// telemetry only: a store without a dataflow cache (the wasm build) recomputes
/// every function, so it differs between transports while the graph does not.
#[derive(Debug, Clone, Serialize)]
pub struct ReportStats {
    pub blobs_indexed: usize,
    pub blobs_extracted: usize,
    pub blobs_cached: usize,
    pub blobs_unsupported: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unresolved: usize,
    pub dataflow_functions_recomputed: usize,
    pub dataflow_functions_reused: usize,
}

impl From<&IndexStats> for ReportStats {
    fn from(s: &IndexStats) -> Self {
        ReportStats {
            blobs_indexed: s.blobs_indexed,
            blobs_extracted: s.blobs_extracted,
            blobs_cached: s.blobs_cached,
            blobs_unsupported: s.blobs_unsupported,
            nodes: s.nodes,
            edges: s.edges,
            unresolved: s.unresolved,
            dataflow_functions_recomputed: s.dataflow.functions_recomputed,
            dataflow_functions_reused: s.dataflow.functions_reused,
        }
    }
}

/// The graph a session holds and answers from.
///
/// Its session metadata — `graph_version`, `dirty` and the freshness envelope —
/// is established when the graph becomes resident (at `open` or `index`) and is
/// not re-checked per call: if the repository's `HEAD` moves while a session
/// holds a graph, answers keep describing the graph as of that moment until the
/// next `open` or `index`.
#[derive(Debug)]
pub struct Resident {
    pub graph_key: String,
    pub mode: Mode,
    pub session: GraphSession,
    /// Node ids by final FQN segment, built on the first `resolve` that can use it.
    by_final_segment: OnceCell<HashMap<String, Vec<NodeId>>>,
}

impl Resident {
    pub(crate) fn new(graph_key: String, mode: Mode, session: GraphSession) -> Self {
        Resident {
            graph_key,
            mode,
            session,
            by_final_segment: OnceCell::new(),
        }
    }

    /// The nodes whose FQN ends in the segment `last` (text after the final
    /// `::`), in ascending id order.
    pub(crate) fn nodes_ending_in(&self, last: &str) -> &[NodeId] {
        let index = self.by_final_segment.get_or_init(|| {
            let mut index: HashMap<String, Vec<NodeId>> = HashMap::new();
            for n in self.session.view.nodes() {
                let seg = n.fqn.rsplit("::").next().unwrap_or(&n.fqn);
                index.entry(seg.to_string()).or_default().push(n.id);
            }
            index
        });
        index.get(last).map_or(&[], Vec::as_slice)
    }
}

/// Where an index's graph goes once linked.
pub(crate) struct Target {
    pub(crate) mode: Mode,
    pub(crate) graph_key: String,
    /// `HEAD`'s tree: the freshness base for both modes.
    pub(crate) head_tree: Option<String>,
    /// `worktree` mode: the overlay difference from `head_tree`.
    pub(crate) overlay: Vec<String>,
    pub(crate) dataflow: bool,
}

struct Pending<S> {
    target: Target,
    /// The working files, sorted by path, with their host indices alongside.
    sources: Vec<SourceFile>,
    host_index: Vec<u32>,
    /// Positions in `sources` of the files whose content `plan` reads.
    manifests: Vec<usize>,
    planned: Option<Planned<S>>,
}

struct Planned<S> {
    store: S,
    stats: IndexStats,
    /// Cache hits, decoded.
    prepared: Vec<PreparedFile>,
    /// Misses not yet submitted, by host index.
    awaiting: BTreeMap<u32, PlannedFile>,
    extracted: Vec<ExtractedFile>,
}

/// One repository's resident session. See the module docs.
pub struct Session<S> {
    /// The directory holding `.cgx/`: the repository root natively, `/` in the
    /// wasm guest (where the host mounts `<repo>/.cgx` at `/.cgx`).
    root: PathBuf,
    open_store: StoreOpener<S>,
    registry: OnceCell<FrontendRegistry>,
    dataflow_default: Option<bool>,
    resident: Option<Resident>,
    pending: Option<Pending<S>>,
}

impl<S> std::fmt::Debug for Session<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("root", &self.root)
            .field("resident", &self.resident.as_ref().map(|r| &r.graph_key))
            .field("indexing", &self.pending.is_some())
            .finish()
    }
}

/// The message of the error a graph tool returns when no graph is resident.
const NO_GRAPH: &str = "no graph is resident: open a fresh index or index first";

impl<S: FactStore> Session<S> {
    /// A session over the `.cgx/` under `root`, opening its store with `open_store`.
    pub fn new(root: impl Into<PathBuf>, open_store: StoreOpener<S>) -> Self {
        Session {
            root: root.into(),
            open_store,
            registry: OnceCell::new(),
            dataflow_default: None,
            resident: None,
            pending: None,
        }
    }

    /// The directory holding `.cgx/`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The resident graph, if any.
    pub fn resident(&self) -> Option<&Resident> {
        self.resident.as_ref()
    }

    /// The frontend registry extraction runs with (built on first use).
    pub fn registry(&self) -> &FrontendRegistry {
        self.registry.get_or_init(default_registry)
    }

    pub(crate) fn store(&self) -> Result<S> {
        (self.open_store)(&self.root).map_err(|e| SessionError::index(format!("opening the store: {e}")))
    }

    /// Drop the resident graph and any index in progress.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn discard(&mut self) {
        self.pending = None;
        self.resident = None;
    }

    pub(crate) fn effective_dataflow(&self, explicit: Option<bool>, cgx_toml: Option<&str>) -> bool {
        explicit
            .or_else(|| cgx_toml.map(|t| dataflow_default(Some(t))))
            .or(self.dataflow_default)
            .unwrap_or(true)
    }

    /// Warm-open the persisted index: if the pointer names `HEAD`'s tree (the
    /// auto-index staleness rule) and its graph is stored, load it resident.
    /// Never indexes. Discards any resident graph and any index in progress.
    pub fn open(&mut self, req: OpenRequest) -> Result<OpenResponse> {
        self.pending = None;
        self.resident = None;
        self.dataflow_default = Some(
            req.dataflow
                .unwrap_or_else(|| dataflow_default(req.cgx_toml.as_deref())),
        );

        let Some(ptr) = read_pointer(&self.root).map_err(SessionError::index)? else {
            return Ok(OpenResponse {
                state: OpenState::Missing,
                graph_key: None,
                pointer: None,
            });
        };
        let stale = |ptr: IndexPointer| OpenResponse {
            state: OpenState::Stale,
            graph_key: Some(ptr.graph_key.clone()),
            pointer: Some(ptr),
        };
        if !ptr.is_fresh(req.head_tree.as_deref()) {
            return Ok(stale(ptr));
        }
        let store = self.store()?;
        let key = TreeOid::new(ptr.graph_key.clone());
        if !store.graph_for(&key)? {
            return Ok(stale(ptr));
        }
        let g = store.read_graph(&key)?;
        let view = GraphView::new(g.nodes, g.edges, g.candidates);
        self.resident = Some(Resident::new(
            ptr.graph_key.clone(),
            Mode::Head,
            GraphSession::committed(view, ptr.graph_key.clone(), req.head_tree),
        ));
        Ok(OpenResponse {
            state: OpenState::Fresh,
            graph_key: Some(ptr.graph_key.clone()),
            pointer: Some(ptr),
        })
    }

    /// Start a host-driven index. `working` is the host's file list as raw path
    /// bytes and blob OIDs, in any order; in `worktree` mode `committed` is
    /// `HEAD`'s tree the same way. Paths are converted lossily to UTF-8 here (as
    /// the native git layer does) and sorted. Discards any resident graph and any
    /// index in progress.
    pub fn index_begin(
        &mut self,
        opts: BeginOpts,
        working: &[(&[u8], &[u8])],
        committed: &[(&[u8], &[u8])],
    ) -> Result<BeginResponse> {
        self.pending = None;
        self.resident = None;

        let mut files: Vec<(SourceFile, u32)> = Vec::with_capacity(working.len());
        for (i, (path, oid)) in working.iter().enumerate() {
            files.push((
                SourceFile {
                    blob_oid: host_oid(oid)?,
                    rel_path: String::from_utf8_lossy(path).into_owned(),
                    content: Vec::new(),
                },
                i as u32,
            ));
        }
        files.sort_by(|a, b| a.0.rel_path.cmp(&b.0.rel_path));
        let (sources, host_index): (Vec<SourceFile>, Vec<u32>) = files.into_iter().unzip();

        let dataflow = self.effective_dataflow(opts.dataflow, opts.cgx_toml.as_deref());
        let target = match opts.mode {
            Mode::Head => {
                if !committed.is_empty() {
                    return Err(SessionError::invalid_params(
                        "head mode takes no committed entries",
                    ));
                }
                let key = opts.graph_key.ok_or_else(|| {
                    SessionError::invalid_params("head mode requires graph_key (HEAD's tree OID)")
                })?;
                let key = host_oid(key.as_bytes())?;
                Target {
                    mode: Mode::Head,
                    graph_key: key.clone(),
                    head_tree: Some(key),
                    overlay: Vec::new(),
                    dataflow,
                }
            }
            Mode::Worktree => {
                let head = opts.head_tree.ok_or_else(|| {
                    SessionError::invalid_params("worktree mode requires head_tree")
                })?;
                let head = host_oid(head.as_bytes())?;
                let mut base = BTreeMap::new();
                for (path, oid) in committed {
                    base.insert(String::from_utf8_lossy(path).into_owned(), host_oid(oid)?);
                }
                Target {
                    mode: Mode::Worktree,
                    graph_key: workdir_key(&sources),
                    head_tree: Some(head),
                    overlay: overlay_difference(&base, &sources),
                    dataflow,
                }
            }
        };

        let manifests: Vec<usize> = (0..sources.len())
            .filter(|&i| is_manifest_path(&sources[i].rel_path))
            .collect();
        let response = BeginResponse {
            manifest_indices: manifests.iter().map(|&i| host_index[i]).collect(),
            graph_key: target.graph_key.clone(),
        };
        self.pending = Some(Pending {
            target,
            sources,
            host_index,
            manifests,
            planned: None,
        });
        Ok(response)
    }

    /// Plan the index begun by [`index_begin`](Self::index_begin), given the
    /// contents of the files it named, in its order. Probes the Layer-1 fragment
    /// cache (pipeline pass 1) and returns only the misses to extract.
    pub fn index_plan(&mut self, contents: Vec<Vec<u8>>) -> Result<(PlanSummary, Vec<PlanMiss>)> {
        let mut pending = self
            .pending
            .take()
            .ok_or_else(|| SessionError::invalid_params("index_plan without index_begin"))?;
        if pending.planned.is_some() {
            return Err(SessionError::invalid_params("index_plan called twice"));
        }
        if contents.len() != pending.manifests.len() {
            return Err(SessionError::invalid_params(format!(
                "index_plan got {} contents for {} manifest files",
                contents.len(),
                pending.manifests.len()
            )));
        }
        for (&pos, content) in pending.manifests.iter().zip(contents) {
            pending.sources[pos].content = content;
        }

        let plan = plan(&pending.sources, self.registry());
        let store = self.store()?;
        let mut stats = IndexStats {
            blobs_unsupported: plan.unsupported,
            ..IndexStats::default()
        };
        let (prepared, misses) = probe_fragments(&plan, &store, &mut stats)?;
        let mut awaiting = BTreeMap::new();
        let mut out = Vec::with_capacity(misses.len());
        for file in misses {
            let host = pending.host_index[file.source];
            out.push((host, encode(&file.ctx).map_err(|e| SessionError::internal(e.to_string()))?));
            awaiting.insert(host, file.clone());
        }
        let summary = PlanSummary {
            files: pending.sources.len(),
            planned: plan.files.len(),
            unsupported: plan.unsupported,
            cached: prepared.len(),
            to_extract: out.len(),
        };
        pending.planned = Some(Planned {
            store,
            stats,
            prepared,
            awaiting,
            extracted: Vec::new(),
        });
        self.pending = Some(pending);
        Ok((summary, out))
    }

    /// Hand extracted files to the index in progress. Each must be a miss the
    /// plan returned and not yet submitted, with the fragment version its
    /// frontend reports and facts in canonical encoding (the bytes are stored as
    /// the file's Layer-1 fragment). Any rejected file aborts the whole index:
    /// the pending state is dropped, so no graph missing that file can be
    /// finished.
    pub fn index_submit(&mut self, files: Vec<Submitted>) -> Result<()> {
        let result = self.submit_all(files);
        if result.is_err() {
            self.pending = None;
        }
        result
    }

    fn submit_all(&mut self, files: Vec<Submitted>) -> Result<()> {
        let registry = self.registry.get_or_init(default_registry);
        let planned = self
            .pending
            .as_mut()
            .and_then(|p| p.planned.as_mut())
            .ok_or_else(|| SessionError::invalid_params("index_submit without index_plan"))?;
        for f in files {
            let file = planned.awaiting.get(&f.index).ok_or_else(|| {
                SessionError::invalid_params(format!(
                    "file {} was not asked for by the plan or was already submitted",
                    f.index
                ))
            })?;
            let expected = registry.frontend_for(&file.ctx.path).fragment_version();
            if f.fragment_version != expected {
                return Err(SessionError::invalid_params(format!(
                    "file {}: fragment version {} from a frontend at version {expected}",
                    f.index, f.fragment_version
                )));
            }
            let facts: FileFacts = decode(&f.facts).map_err(|e| {
                SessionError::invalid_params(format!("file {}: facts do not decode: {e}", f.index))
            })?;
            // Fragments are write-once per blob, so non-canonical bytes would
            // persist and break store byte identity for every later reader.
            let canonical = encode(&facts).map_err(|e| SessionError::internal(e.to_string()))?;
            if canonical != f.facts {
                return Err(SessionError::invalid_params(format!(
                    "file {}: facts are not in canonical encoding",
                    f.index
                )));
            }
            let file = planned.awaiting.remove(&f.index).expect("present: checked above");
            planned.extracted.push(ExtractedFile {
                prepared: PreparedFile {
                    blob_oid: file.ctx.blob_oid,
                    rel_path: file.ctx.path.as_str().to_string(),
                    lang: file.lang,
                    facts,
                },
                fragment_version: f.fragment_version,
                fragment: f.facts,
            });
        }
        Ok(())
    }

    /// Finish the index in progress: write the extracted fragments, link,
    /// run the post-link passes (SCIP first when `scip` is given), and — in
    /// `head` mode — store the graph, drop superseded graphs and advance the
    /// pointer. The graph becomes resident. The host holds the store's write lock
    /// around this call.
    pub fn index_finish(&mut self, scip: Option<&[u8]>) -> Result<IndexReport> {
        let pending = self
            .pending
            .take()
            .ok_or_else(|| SessionError::invalid_params("index_finish without index_begin"))?;
        let mut planned = pending
            .planned
            .ok_or_else(|| SessionError::invalid_params("index_finish without index_plan"))?;
        if !planned.awaiting.is_empty() {
            return Err(SessionError::index(format!(
                "{} planned files were never submitted",
                planned.awaiting.len()
            )));
        }
        let target = pending.target;
        planned.stats.blobs_extracted = planned.extracted.len();
        put_extracted(&mut planned.store, &planned.extracted)?;
        let mut prepared = planned.prepared;
        prepared.extend(planned.extracted.into_iter().map(|e| e.prepared));
        let opts = IndexOpts {
            dataflow: target.dataflow,
            ..IndexOpts::default()
        };
        let graph = link_prepared(prepared, &mut planned.store, &opts, &mut planned.stats)?;
        self.complete(&mut planned.store, graph, planned.stats, scip, target)
    }

    /// The tail every index shares once linked: post-link passes, persistence
    /// for `head` mode, and the resident graph.
    pub(crate) fn complete(
        &mut self,
        store: &mut S,
        mut graph: ResolvedGraph,
        mut stats: IndexStats,
        scip: Option<&[u8]>,
        target: Target,
    ) -> Result<IndexReport> {
        if let Some(bytes) = scip {
            apply_scip_bytes(&mut graph, &mut stats, bytes)?;
        }
        apply_cha(&mut graph, &mut stats);
        apply_rta(&mut graph, &mut stats);
        apply_sig(&mut graph, &mut stats);
        apply_effects(&mut graph, &mut stats);

        let (nodes, edges, candidates) = graph.into_linked();
        let mut linked = LinkedGraph::new(nodes, edges, candidates);
        if target.mode == Mode::Head {
            let key = TreeOid::new(target.graph_key.clone());
            store.put_graph(&key, None, &linked)?;
            // Retain exactly the graph just written (sparse-storage RFC P1), as
            // `cgx index` does.
            store.prune_graphs_except(&[&key])?;
            write_pointer(
                &self.root,
                &IndexPointer {
                    graph_key: target.graph_key.clone(),
                    total_files: Some(stats.blobs_indexed + stats.blobs_unsupported),
                    unsupported_files: Some(stats.blobs_unsupported),
                },
            )
            .map_err(SessionError::index)?;
        }

        linked.canonical_order();
        let view = GraphView::new(linked.nodes, linked.edges, linked.candidates);
        let session = match target.mode {
            Mode::Head => GraphSession::committed(view, target.graph_key.clone(), target.head_tree),
            Mode::Worktree => GraphSession::worktree(
                view,
                target.head_tree.unwrap_or_default(),
                &target.overlay,
                None,
                target.graph_key.clone(),
            ),
        };
        self.resident = Some(Resident::new(target.graph_key.clone(), target.mode, session));
        Ok(IndexReport {
            graph_key: target.graph_key,
            mode: target.mode,
            up_to_date: false,
            dataflow: Some(target.dataflow),
            stats: Some(ReportStats::from(&stats)),
        })
    }

    /// Run one tool on the resident graph: a session op ([`ops::SESSION_OPS`])
    /// or an MCP tool by name, with the MCP tool's arguments. `root` and
    /// `include_dirty` are ignored by graph tools: the resident graph answers.
    pub fn call(&self, tool: &str, args: &Value) -> Result<Value> {
        if let Some(op) = ops::session_op(tool) {
            let resident = self.resident.as_ref().ok_or_else(|| SessionError::stale(NO_GRAPH))?;
            return op(resident, args);
        }
        let held = Held {
            session: self.resident.as_ref().map(|r| &r.session),
            missing: Cell::new(false),
        };
        match cgx_mcp::tools::call_with(tool, args, &held) {
            Ok(v) => Ok(v),
            Err(_) if held.missing.get() => Err(SessionError::stale(NO_GRAPH)),
            Err(e) => Err(e.into()),
        }
    }
}

/// The provider a resident session hands the MCP tool handlers: the held graph,
/// whatever the call's `root`/`include_dirty`.
struct Held<'a> {
    session: Option<&'a GraphSession>,
    missing: Cell<bool>,
}

impl SessionProvider for Held<'_> {
    fn session(&self, _args: &Value) -> std::result::Result<SessionRef<'_>, ToolError> {
        match self.session {
            Some(s) => Ok(SessionRef::Borrowed(s)),
            None => {
                self.missing.set(true);
                Err(ToolError::index(NO_GRAPH))
            }
        }
    }
}

/// An object id (blob or tree OID) as the host sent it: exactly 40 (SHA-1) or 64
/// (SHA-256) lowercase hex digits, the only spellings git and the native walk
/// produce. Anything else would address store paths native never writes, or
/// none at all.
fn host_oid(bytes: &[u8]) -> Result<String> {
    let well_formed = matches!(bytes.len(), 40 | 64)
        && bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !well_formed {
        return Err(SessionError::invalid_params(format!(
            "object id {:?} is not 40 or 64 lowercase hex digits",
            String::from_utf8_lossy(bytes)
        )));
    }
    Ok(String::from_utf8(bytes.to_vec()).expect("hex is ASCII"))
}

/// Extract one file (pipeline pass 2): the postcard [`FileCtx`] from
/// [`Session::index_plan`] and the file's content in; the frontend's fragment
/// version and the postcard `FileFacts` out. Pure: no store, no session state.
pub fn extract(registry: &FrontendRegistry, ctx: &[u8], content: &[u8]) -> Result<(ExtractMeta, Vec<u8>)> {
    let ctx: FileCtx = decode(ctx)
        .map_err(|e| SessionError::invalid_params(format!("file context does not decode: {e}")))?;
    let out = extract_one(registry, &ctx, content)?;
    Ok((
        ExtractMeta {
            lang: out.prepared.lang,
            fragment_version: out.fragment_version,
        },
        out.fragment,
    ))
}

