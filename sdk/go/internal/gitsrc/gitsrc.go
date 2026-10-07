// Package gitsrc enumerates the files cgx indexes, through the real `git`
// binary for the committed tree and a plain filesystem walk for the working
// directory. It reproduces the native rules in crates/cgx-index/src/git.rs and
// nothing more: which entries are indexed, which bytes are their content, and
// which OID names them. Paths stay raw bytes; the engine converts them.
package gitsrc

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
)

// ErrNotGitRepo is returned for a path outside a git repository, or one whose
// HEAD is unborn.
var ErrNotGitRepo = errors.New("not a git repository with a committed HEAD")

// ErrGitTooOld is returned for a git older than 2.25 (the first with
// `rev-parse --show-object-format`), and for a git older than 2.45 (the first
// that can disable lazy fetching) on a partial clone, where reading a missing
// blob would fetch it from the promisor remote.
var ErrGitTooOld = errors.New("git is too old")

// Git versions the SDK depends on.
var (
	minGit         = gitVersion{2, 25} // rev-parse --show-object-format
	noLazyFetchGit = gitVersion{2, 45} // --no-lazy-fetch, GIT_NO_LAZY_FETCH
)

type gitVersion struct{ major, minor int }

func (v gitVersion) atLeast(w gitVersion) bool {
	return v.major > w.major || v.major == w.major && v.minor >= w.minor
}

func (v gitVersion) String() string { return fmt.Sprintf("%d.%d", v.major, v.minor) }

// parseGitVersion reads `git version` output ("git version 2.50.1 (Apple
// Git-155)", "git version 2.39.5.windows.1").
func parseGitVersion(out string) (gitVersion, bool) {
	f := strings.Fields(out)
	if len(f) < 3 || f[0] != "git" || f[1] != "version" {
		return gitVersion{}, false
	}
	parts := strings.SplitN(f[2], ".", 3)
	if len(parts) < 2 {
		return gitVersion{}, false
	}
	major, err1 := strconv.Atoi(parts[0])
	minor, err2 := strconv.Atoi(parts[1])
	if err1 != nil || err2 != nil {
		return gitVersion{}, false
	}
	return gitVersion{major, minor}, true
}

// ErrMissingObject is returned when `git cat-file` cannot produce a blob, for
// example a partial clone's promisor blob (lazy fetch is disabled).
var ErrMissingObject = errors.New("git object missing")

// Entry is one indexed file: raw path bytes relative to the repository root,
// and the hex blob OID.
type Entry struct {
	Path []byte
	OID  string
}

// Repo runs git plumbing against one repository.
type Repo struct {
	// Dir is the canonical (absolute, symlink-resolved) path the caller gave.
	// The index directory `.cgx` lives here, not at the discovered top level.
	Dir string
	git string
	env []string
	// noLazyFetch: this git understands --no-lazy-fetch, so every call
	// passes it.
	noLazyFetch bool
}

// Open canonicalizes path and checks it is inside a git work tree.
func Open(ctx context.Context, path string) (*Repo, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return nil, err
	}
	dir, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return nil, err
	}
	git, err := exec.LookPath("git")
	if err != nil {
		return nil, fmt.Errorf("git binary: %w", err)
	}
	r := &Repo{Dir: dir, git: git, env: Env(os.Environ())}
	cmd := exec.CommandContext(ctx, git, "version")
	cmd.Env = r.env
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("git version: %w", err)
	}
	v, ok := parseGitVersion(string(out))
	if !ok {
		return nil, fmt.Errorf("git version: unrecognised output %q", bytes.TrimSpace(out))
	}
	if !v.atLeast(minGit) {
		return nil, fmt.Errorf("%w: git %s found, %s or later is required", ErrGitTooOld, v, minGit)
	}
	if _, err := r.run(ctx, "rev-parse", "--git-dir"); err != nil {
		return nil, fmt.Errorf("%w: %s: %v", ErrNotGitRepo, dir, err)
	}
	if err := r.checkLazyFetch(ctx, v); err != nil {
		return nil, err
	}
	return r, nil
}

// checkLazyFetch makes reads fail rather than fetch. git 2.45 and later take
// --no-lazy-fetch on every call. An older git cannot be stopped from lazily
// fetching, which can happen only in a partial clone, so such a repository
// is refused and any other is read as is.
func (r *Repo) checkLazyFetch(ctx context.Context, v gitVersion) error {
	if v.atLeast(noLazyFetchGit) {
		r.noLazyFetch = true
		return nil
	}
	partial, err := r.isPartialClone(ctx)
	if err != nil {
		return err
	}
	if partial {
		return fmt.Errorf("%w: %s is a partial clone and git %s cannot disable fetching its missing objects from the promisor remote; git %s or later is required", ErrGitTooOld, r.Dir, v, noLazyFetchGit)
	}
	return nil
}

// partialCloneConfig matches the configuration that makes a repository a
// partial clone: extensions.partialClone, or a remote marked promisor or
// given a partial-clone filter. git config reports keys lower-cased.
const partialCloneConfig = `^(extensions\.partialclone|remote\..+\.promisor|remote\..+\.partialclonefilter)$`

// isPartialClone reports whether the repository could lazily fetch objects.
func (r *Repo) isPartialClone(ctx context.Context) (bool, error) {
	var stderr bytes.Buffer
	cmd := exec.CommandContext(ctx, r.git, "-C", r.Dir, "config", "--get-regexp", partialCloneConfig)
	cmd.Env = r.env
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	var ee *exec.ExitError
	if errors.As(err, &ee) && ee.ExitCode() == 1 {
		return false, nil // no matching key
	}
	if err != nil {
		return false, fmt.Errorf("git config: %w: %s", err, bytes.TrimSpace(stderr.Bytes()))
	}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		key, value, _ := strings.Cut(line, " ")
		if strings.HasSuffix(key, ".promisor") && strings.EqualFold(strings.TrimSpace(value), "false") {
			continue
		}
		return true, nil
	}
	return false, nil
}

// repoLocationVars are the variables that point git at a repository other
// than the one named by -C: git's own "local" variables (`git rev-parse
// --local-env-vars`, git 2.50), the ones it resets when it switches
// repositories, plus the discovery controls. They are set inside git hooks,
// so an SDK embedded in a hook would otherwise index another repository. The
// native engine opens repositories with gix::discover, which applies no
// environment overrides; dropping these matches it. GIT_NO_REPLACE_OBJECTS
// and GIT_REPLACE_REF_BASE are kept: replace refs are honoured natively too.
var repoLocationVars = []string{
	"GIT_DIR", "GIT_WORK_TREE", "GIT_IMPLICIT_WORK_TREE", "GIT_PREFIX",
	"GIT_COMMON_DIR", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES",
	"GIT_SHALLOW_FILE", "GIT_GRAFT_FILE", "GIT_NAMESPACE",
	"GIT_CONFIG", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT",
	"GIT_CEILING_DIRECTORIES", "GIT_DISCOVERY_ACROSS_FILESYSTEM",
}

// Env returns base with the repository-location variables removed (see
// repoLocationVars) and the variables every git invocation needs: lazy fetch
// disabled (zero egress; a missing promisor blob is an error, as it is for the
// native reader) and credential prompts off.
func Env(base []string) []string {
	out := make([]string, 0, len(base)+2)
	for _, kv := range base {
		name, _, _ := strings.Cut(kv, "=")
		if name == "GIT_NO_LAZY_FETCH" || name == "GIT_TERMINAL_PROMPT" || dropped(name) {
			continue
		}
		out = append(out, kv)
	}
	return append(out, "GIT_NO_LAZY_FETCH=1", "GIT_TERMINAL_PROMPT=0")
}

func dropped(name string) bool {
	for _, v := range repoLocationVars {
		if name == v {
			return true
		}
	}
	// GIT_CONFIG_KEY_<n> / GIT_CONFIG_VALUE_<n> pair with GIT_CONFIG_COUNT.
	return strings.HasPrefix(name, "GIT_CONFIG_KEY_") || strings.HasPrefix(name, "GIT_CONFIG_VALUE_")
}

// command runs git against the repository, with --no-lazy-fetch when this
// git supports it (see checkLazyFetch).
func (r *Repo) command(ctx context.Context, args ...string) *exec.Cmd {
	pre := []string{"-C", r.Dir}
	if r.noLazyFetch {
		pre = append([]string{"--no-lazy-fetch"}, pre...)
	}
	cmd := exec.CommandContext(ctx, r.git, append(pre, args...)...)
	cmd.Env = r.env
	return cmd
}

func (r *Repo) run(ctx context.Context, args ...string) ([]byte, error) {
	var stderr bytes.Buffer
	cmd := r.command(ctx, args...)
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		return nil, fmt.Errorf("git %s: %w: %s", strings.Join(args, " "), err, bytes.TrimSpace(stderr.Bytes()))
	}
	return out, nil
}

// HeadTree returns the OID of HEAD's tree, the key the committed graph is
// stored under.
func (r *Repo) HeadTree(ctx context.Context) (string, error) {
	out, err := r.run(ctx, "rev-parse", "--verify", "--quiet", "HEAD^{tree}")
	if err != nil {
		if ctx.Err() != nil {
			return "", ctx.Err()
		}
		return "", fmt.Errorf("%w: %s", ErrNotGitRepo, r.Dir)
	}
	return string(bytes.TrimSpace(out)), nil
}

// TopLevel returns the work tree's root, the base of worktree-mode paths.
func (r *Repo) TopLevel(ctx context.Context) (string, error) {
	out, err := r.run(ctx, "rev-parse", "--show-toplevel")
	if err != nil {
		return "", err
	}
	return string(bytes.TrimRight(out, "\n")), nil
}

// ObjectFormat returns "sha1" or "sha256".
func (r *Repo) ObjectFormat(ctx context.Context) (string, error) {
	out, err := r.run(ctx, "rev-parse", "--show-object-format")
	if err != nil {
		return "", err
	}
	return string(bytes.TrimSpace(out)), nil
}

// ListTree returns every regular-file blob in tree, recursively, over the
// whole tree whatever subdirectory the repository was opened at. Symlinks
// (mode 120000) and submodule gitlinks are excluded; executables are kept.
func (r *Repo) ListTree(ctx context.Context, tree string) ([]Entry, error) {
	out, err := r.run(ctx, "ls-tree", "-r", "-z", "--full-tree", tree)
	if err != nil {
		return nil, err
	}
	return parseLsTree(out)
}

// parseLsTree parses `<mode> SP <type> SP <oid> TAB <path> NUL` records.
func parseLsTree(out []byte) ([]Entry, error) {
	var entries []Entry
	for len(out) > 0 {
		end := bytes.IndexByte(out, 0)
		if end < 0 {
			return nil, fmt.Errorf("ls-tree: unterminated record")
		}
		rec := out[:end]
		out = out[end+1:]
		tab := bytes.IndexByte(rec, '\t')
		if tab < 0 {
			return nil, fmt.Errorf("ls-tree: record without a tab: %q", rec)
		}
		fields := bytes.Fields(rec[:tab])
		if len(fields) != 3 {
			return nil, fmt.Errorf("ls-tree: malformed header %q", rec[:tab])
		}
		mode, typ, oid := string(fields[0]), string(fields[1]), string(fields[2])
		if typ != "blob" || mode == "120000" {
			continue
		}
		entries = append(entries, Entry{Path: bytes.Clone(rec[tab+1:]), OID: oid})
	}
	return entries, nil
}

// Batch is one long-lived `git cat-file --batch` process: raw object
// contents, with no smudge, eol or textconv filters applied.
type Batch struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	stdout *bufio.Reader
	stderr lockedBuffer
	mu     sync.Mutex
	closed bool
}

// lockedBuffer is written by exec's stderr copier while readOne may read it.
type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.buf.Len() < 64<<10 {
		b.buf.Write(p)
	}
	return len(p), nil
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return strings.TrimSpace(b.buf.String())
}

// NewBatch starts the cat-file process. Close it when done.
func (r *Repo) NewBatch(ctx context.Context) (*Batch, error) {
	b := &Batch{cmd: r.command(ctx, "cat-file", "--batch")}
	b.cmd.Stderr = &b.stderr
	stdin, err := b.cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := b.cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := b.cmd.Start(); err != nil {
		return nil, err
	}
	b.stdin = stdin
	b.stdout = bufio.NewReaderSize(stdout, 1<<16)
	return b, nil
}

// Each streams the contents of oids, in order, to fn. Requests are written
// from a separate goroutine so git's output never waits on our input. fn's
// slice is owned by fn. Each stops at the first error from git or fn.
func (b *Batch) Each(ctx context.Context, oids []string, fn func(i int, content []byte) error) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.closed {
		return errors.New("cat-file batch is closed")
	}
	writeErr := make(chan error, 1)
	stop := make(chan struct{})
	defer close(stop)
	go func() {
		w := bufio.NewWriterSize(b.stdin, 1<<16)
		for _, oid := range oids {
			select {
			case <-stop:
				writeErr <- nil
				return
			default:
			}
			if _, err := w.WriteString(oid + "\n"); err != nil {
				writeErr <- err
				return
			}
		}
		writeErr <- w.Flush()
	}()
	for i, oid := range oids {
		if err := ctx.Err(); err != nil {
			b.poison()
			return err
		}
		content, err := b.readOne(oid)
		if err != nil {
			b.poison()
			// A cancelled context kills the process; report the cause, not
			// the broken pipe it leaves.
			if cerr := ctx.Err(); cerr != nil {
				return cerr
			}
			return err
		}
		if err := fn(i, content); err != nil {
			// Requests already written would desynchronize the stream; the
			// process cannot be reused.
			b.poison()
			return err
		}
	}
	return <-writeErr
}

// Get reads one object.
func (b *Batch) Get(ctx context.Context, oid string) ([]byte, error) {
	var out []byte
	err := b.Each(ctx, []string{oid}, func(_ int, c []byte) error { out = c; return nil })
	return out, err
}

func (b *Batch) readOne(want string) ([]byte, error) {
	header, err := b.stdout.ReadString('\n')
	if err != nil {
		return nil, fmt.Errorf("cat-file: reading header for %s: %w: %s", want, err, b.stderr.String())
	}
	fields := strings.Fields(header)
	if len(fields) == 2 && fields[1] == "missing" {
		return nil, fmt.Errorf("%w: %s", ErrMissingObject, want)
	}
	if len(fields) != 3 {
		return nil, fmt.Errorf("cat-file: unexpected header %q", header)
	}
	if fields[1] != "blob" {
		return nil, fmt.Errorf("cat-file: %s is a %s, not a blob", want, fields[1])
	}
	size, err := strconv.ParseInt(fields[2], 10, 64)
	if err != nil || size < 0 {
		return nil, fmt.Errorf("cat-file: bad size in %q", header)
	}
	// The engine takes at most 2 GiB per call; refuse before allocating.
	if size > math.MaxInt32 {
		return nil, fmt.Errorf("cat-file: %s is %d bytes, larger than the engine accepts", want, size)
	}
	content := make([]byte, size+1)
	if _, err := io.ReadFull(b.stdout, content); err != nil {
		return nil, fmt.Errorf("cat-file: reading %s: %w", want, err)
	}
	if content[size] != '\n' {
		return nil, fmt.Errorf("cat-file: %s not newline-terminated", want)
	}
	return content[:size:size], nil
}

func (b *Batch) poison() {
	if !b.closed {
		b.closed = true
		_ = b.stdin.Close()
		_ = b.cmd.Process.Kill()
		_ = b.cmd.Wait()
	}
}

// Close ends the process.
func (b *Batch) Close() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.closed {
		return nil
	}
	b.closed = true
	_ = b.stdin.Close()
	return b.cmd.Wait()
}
