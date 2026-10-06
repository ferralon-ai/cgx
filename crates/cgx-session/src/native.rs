//! The native side of a session: git enumeration and in-process extraction, and
//! the `cgx session` NDJSON server.
//!
//! ## Protocol (`PROTOCOL_VERSION` 1)
//!
//! The server's first line is the handshake
//! `{"cgx_session":1,"abi":1,"version":"<cgx>","schema_hash":"<sha1>"}`. Then one
//! JSON request per line on stdin, one response per line on stdout, in order;
//! diagnostics go to stderr. Requests:
//!
//! - `{"id":n,"op":"open"}` — warm-open the persisted index for the current
//!   `HEAD` (as `cgx_session_open`).
//! - `{"id":n,"op":"index","mode":"head"|"worktree","dataflow":bool|null,"scip":path|null,"force":bool}`
//!   — index and hold. `head` without `force` returns `up_to_date` when the
//!   persisted graph is already `HEAD`'s.
//! - `{"id":n,"op":"call","tool":"…","args":{…}}` — an MCP tool or a session op
//!   on the resident graph.
//! - `{"id":n,"op":"close"}` — answer, then exit.
//!
//! Responses: `{"id":n,"ok":true,"result":…}` or
//! `{"id":n,"ok":false,"error":{"kind":…,"message":…}}`, with `id` echoed.

use std::io::{BufRead, Write};
use std::path::Path;

use cgx_index::{extract_and_link, workdir_key, IndexOpts, Repo};
use cgx_mcp::session::overlay_difference;
use cgx_store::FactStore;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Result, SessionError};
use crate::session::{IndexReport, Mode, OpenRequest, OpenResponse, OpenState, Session, Target};
use crate::store_loc::dataflow_default;
use crate::{schema_hash, ABI_VERSION, PROTOCOL_VERSION};

fn git_err(e: cgx_index::IndexError) -> SessionError {
    SessionError::index(e.to_string())
}

impl<S: FactStore> Session<S> {
    fn cgx_toml(&self) -> Option<String> {
        std::fs::read_to_string(self.root().join("cgx.toml")).ok()
    }

    fn head_tree(&self) -> Option<String> {
        Repo::discover(self.root())
            .and_then(|r| r.head_tree_oid())
            .ok()
    }

    /// [`open`](Session::open) with `HEAD`'s tree and `cgx.toml` read from the
    /// repository at the session root.
    pub fn open_native(&mut self, dataflow: Option<bool>) -> Result<OpenResponse> {
        self.open(OpenRequest {
            head_tree: self.head_tree(),
            cgx_toml: self.cgx_toml(),
            dataflow,
        })
    }

    /// Index the repository at the session root in-process and hold the graph:
    /// `HEAD`'s tree (stored, pointer advanced — `cgx index` semantics) or the
    /// working directory (fragments cached, graph held only). Extraction runs on
    /// the native composition; everything after linking is shared with the
    /// host-driven path.
    pub fn index_native(
        &mut self,
        mode: Mode,
        dataflow: Option<bool>,
        scip: Option<&Path>,
        force: bool,
    ) -> Result<IndexReport> {
        let repo = Repo::discover(self.root()).map_err(git_err)?;
        let head = repo.head_tree_oid().map_err(git_err)?;
        let toml = self.cgx_toml();
        let dataflow = dataflow.unwrap_or_else(|| dataflow_default(toml.as_deref()));

        if mode == Mode::Head && !force {
            let opened = self.open(OpenRequest {
                head_tree: Some(head.clone()),
                cgx_toml: toml,
                dataflow: Some(dataflow),
            })?;
            if opened.state == OpenState::Fresh {
                return Ok(IndexReport {
                    graph_key: head,
                    mode,
                    up_to_date: true,
                    dataflow,
                    stats: None,
                });
            }
        }

        let scip = match scip {
            Some(path) => Some(std::fs::read(path).map_err(|e| {
                SessionError::index(format!("reading SCIP index {path:?}: {e}"))
            })?),
            None => None,
        };
        let (sources, target) = match mode {
            Mode::Head => (
                repo.enumerate_tree().map_err(git_err)?,
                Target {
                    mode,
                    graph_key: head.clone(),
                    head_tree: Some(head),
                    overlay: Vec::new(),
                    dataflow,
                },
            ),
            Mode::Worktree => {
                let workdir = repo
                    .workdir()
                    .ok_or_else(|| SessionError::index("repository has no working directory"))?
                    .to_path_buf();
                let committed = repo.tree_blob_oids(&head).map_err(git_err)?;
                // Walk before the store opens so the walk sees the same `.cgx/` a
                // host-driven worktree index sees.
                let working = repo.enumerate_workdir(&workdir).map_err(git_err)?;
                let target = Target {
                    mode,
                    graph_key: workdir_key(&working),
                    head_tree: Some(head),
                    overlay: overlay_difference(&committed, &working),
                    dataflow,
                };
                (working, target)
            }
        };

        let mut store = self.store()?;
        let opts = IndexOpts {
            dataflow,
            ..IndexOpts::default()
        };
        let (graph, stats) = extract_and_link(&sources, self.registry(), &mut store, &opts)?;
        drop(sources);
        self.complete(&mut store, graph, stats, scip.as_deref(), target)
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    op: String,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default)]
    mode: Option<Mode>,
    #[serde(default)]
    dataflow: Option<bool>,
    #[serde(default)]
    scip: Option<String>,
    #[serde(default)]
    force: Option<bool>,
}

/// Serve the `cgx session` protocol (module docs) over `input`/`output` until
/// `close` or end of input. Only I/O errors on the streams end it early.
pub fn serve<S: FactStore>(
    session: &mut Session<S>,
    input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    let handshake = json!({
        "cgx_session": PROTOCOL_VERSION,
        "abi": ABI_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "schema_hash": schema_hash(),
    });
    writeln!(output, "{handshake}")?;
    output.flush()?;

    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let (id, close, outcome) = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let close = req.op == "close";
                let id = req.id.clone();
                (id, close, handle(session, req))
            }
            Err(e) => (
                Value::Null,
                false,
                Err(SessionError::invalid_params(format!("malformed request: {e}"))),
            ),
        };
        let response = match outcome {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(error) => json!({"id": id, "ok": false, "error": error}),
        };
        writeln!(output, "{response}")?;
        output.flush()?;
        if close {
            break;
        }
    }
    Ok(())
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("session response serializes")
}

fn handle<S: FactStore>(session: &mut Session<S>, req: Request) -> Result<Value> {
    match req.op.as_str() {
        "open" => Ok(to_value(&session.open_native(req.dataflow)?)),
        "index" => {
            let mode = req.mode.unwrap_or(Mode::Head);
            let report = session.index_native(
                mode,
                req.dataflow,
                req.scip.as_deref().map(Path::new),
                req.force.unwrap_or(false),
            )?;
            Ok(to_value(&report))
        }
        "call" => {
            let tool = req
                .tool
                .ok_or_else(|| SessionError::invalid_params("call without `tool`"))?;
            let mut args = req.args.unwrap_or_else(|| json!({}));
            // Tools that read git history rather than the graph (`coupling`,
            // `impacted_tests`) take the repository from `root`; the resident
            // graph tools ignore it.
            if let (Value::Object(map), Some(root)) = (&mut args, session.root().to_str()) {
                map.entry("root").or_insert_with(|| Value::String(root.to_string()));
            }
            session.call(&tool, &args)
        }
        "close" => Ok(json!({})),
        other => Err(SessionError::invalid_params(format!("unknown op `{other}`"))),
    }
}

