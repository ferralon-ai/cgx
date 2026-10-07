package abi

import "encoding/json"

// Info is cgx_info's response.
type Info struct {
	ABI         int      `json:"abi"`
	CgxVersion  string   `json:"cgx_version"`
	StoreFormat uint32   `json:"store_format"`
	SchemaHash  string   `json:"schema_hash"`
	Tools       []string `json:"tools"`
}

// SessionOpenRequest is cgx_session_open's request. HeadTree is the hex tree
// OID of HEAD (nil outside a git repository); CgxToml is the repo-root
// cgx.toml text (nil when absent or not valid UTF-8, which the engine treats
// as "no config"); Dataflow overrides the cgx.toml default when non-nil.
type SessionOpenRequest struct {
	HeadTree *string `json:"head_tree"`
	CgxToml  *string `json:"cgx_toml"`
	Dataflow *bool   `json:"dataflow"`
}

// Freshness states reported by an open.
const (
	StateFresh   = "fresh"
	StateStale   = "stale"
	StateMissing = "missing"
)

// SessionOpenResponse is the open result on both transports.
type SessionOpenResponse struct {
	State    string          `json:"state"`
	GraphKey *string         `json:"graph_key"`
	Pointer  json.RawMessage `json:"pointer,omitempty"`
}

// Index modes.
const (
	ModeHead     = "head"
	ModeWorktree = "worktree"
)

// IndexBeginOpts is the first entry of cgx_index_begin's frame. Head mode
// sets GraphKey; worktree mode sets HeadTree and Committed, the number of
// trailing (path, oid) pairs that are HEAD's tree rather than working files.
// CgxToml carries the repo-root cgx.toml so a begin without a prior open
// still honours its dataflow setting.
type IndexBeginOpts struct {
	Mode      string  `json:"mode"`
	GraphKey  *string `json:"graph_key,omitempty"`
	HeadTree  *string `json:"head_tree,omitempty"`
	Committed int     `json:"committed,omitempty"`
	Dataflow  *bool   `json:"dataflow,omitempty"`
	CgxToml   *string `json:"cgx_toml,omitempty"`
}

// IndexBeginResponse lists the working entries whose contents the planner
// needs, in the order cgx_index_plan expects them, and the graph key.
type IndexBeginResponse struct {
	ManifestIndices []uint32 `json:"manifest_indices"`
	GraphKey        string   `json:"graph_key"`
}

// PlanSummary is the first entry of cgx_index_plan's response frame. The host
// relies only on the frame's miss list; the summary is carried into stats.
type PlanSummary = json.RawMessage

// ExtractMeta is the first entry of cgx_extract's response frame.
type ExtractMeta struct {
	Lang            string `json:"lang"`
	FragmentVersion uint32 `json:"fragment_version"`
}

// QueryRequest is cgx_query's request, and the body of a native `call`.
type QueryRequest struct {
	Tool string          `json:"tool"`
	Args json.RawMessage `json:"args"`
}
