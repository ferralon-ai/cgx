package gitsrc

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"
)

var ctx = context.Background()

func git(t *testing.T, dir string, stdin []byte, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(),
		"GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1",
		"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.com", "GIT_AUTHOR_DATE=2026-01-01T00:00:00Z",
		"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.com", "GIT_COMMITTER_DATE=2026-01-01T00:00:00Z")
	if stdin != nil {
		cmd.Stdin = bytes.NewReader(stdin)
	}
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func write(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func initRepo(t *testing.T, extra ...string) string {
	t.Helper()
	dir := t.TempDir()
	git(t, dir, nil, append([]string{"init", "-q", "-b", "main"}, extra...)...)
	return dir
}

func paths(entries []Entry) []string {
	var out []string
	for _, e := range entries {
		out = append(out, string(e.Path))
	}
	slices.Sort(out)
	return out
}

// T1–T4, T7, T10: the committed-tree rules on one repository.
func TestListTreeCommittedRules(t *testing.T) {
	dir := initRepo(t)
	write(t, filepath.Join(dir, "src/a.go"), "package a\n")
	write(t, filepath.Join(dir, "run.sh"), "#!/bin/sh\n")
	if err := os.Chmod(filepath.Join(dir, "run.sh"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("src/a.go", filepath.Join(dir, "link.go")); err != nil {
		t.Fatal(err)
	}
	// Tracked-but-ignored and attribute-filtered files are indexed with raw bytes.
	write(t, filepath.Join(dir, ".gitignore"), "ignored.py\n")
	write(t, filepath.Join(dir, "ignored.py"), "x = 1\n")
	write(t, filepath.Join(dir, ".gitattributes"), "*.txt text eol=crlf ident\n")
	write(t, filepath.Join(dir, "notes.txt"), "$Id$\nline\n")
	git(t, dir, nil, "add", "-A")
	git(t, dir, nil, "add", "-f", "ignored.py")
	// A submodule gitlink: excluded and never descended.
	git(t, dir, nil, "update-index", "--add", "--cacheinfo", "160000,"+strings.Repeat("ab", 20)+",vendor/sub")
	git(t, dir, nil, "commit", "-q", "-m", "c")

	r, err := Open(ctx, filepath.Join(dir, "src"))
	if err != nil {
		t.Fatal(err)
	}
	wantDir, _ := filepath.EvalSymlinks(filepath.Join(dir, "src"))
	if r.Dir != wantDir {
		t.Fatalf("Dir = %q, want the canonical path given %q", r.Dir, wantDir)
	}
	tree, err := r.HeadTree(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if want := git(t, dir, nil, "rev-parse", "HEAD^{tree}"); tree != want {
		t.Fatalf("HeadTree = %s, want %s", tree, want)
	}
	entries, err := r.ListTree(ctx, tree)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{".gitattributes", ".gitignore", "ignored.py", "notes.txt", "run.sh", "src/a.go"}
	if got := paths(entries); !slices.Equal(got, want) {
		t.Fatalf("paths = %q, want %q (whole tree from a subdirectory; symlink and gitlink excluded)", got, want)
	}
	for _, e := range entries {
		if want := git(t, dir, nil, "rev-parse", "HEAD:"+string(e.Path)); e.OID != want {
			t.Fatalf("%s oid = %s, want %s", e.Path, e.OID, want)
		}
	}

	b, err := r.NewBatch(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	for _, e := range entries {
		if string(e.Path) != "notes.txt" {
			continue
		}
		got, err := b.Get(ctx, e.OID)
		if err != nil {
			t.Fatal(err)
		}
		if string(got) != "$Id$\nline\n" {
			t.Fatalf("notes.txt content = %q, want the raw blob (no eol or ident filter)", got)
		}
	}
}

// T5: path bytes are passed through raw, never decoded.
func TestListTreeNonUTF8Path(t *testing.T) {
	dir := initRepo(t)
	write(t, filepath.Join(dir, "ok.go"), "package ok\n")
	git(t, dir, nil, "add", "-A")
	blob := git(t, dir, []byte("package bad\n"), "hash-object", "-w", "--stdin")
	bad := "bad\xc3\x28.go"
	git(t, dir, nil, "update-index", "--add", "--cacheinfo", "100644,"+blob+","+bad)
	git(t, dir, nil, "commit", "-q", "-m", "c")

	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	tree, _ := r.HeadTree(ctx)
	entries, err := r.ListTree(ctx, tree)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.ContainsFunc(entries, func(e Entry) bool { return bytes.Equal(e.Path, []byte(bad)) && e.OID == blob }) {
		t.Fatalf("raw non-UTF-8 path missing: %q", paths(entries))
	}
}

// T8: replace refs are honoured for content, as they are natively.
func TestBatchHonoursReplaceRefs(t *testing.T) {
	dir := initRepo(t)
	write(t, filepath.Join(dir, "a.py"), "original\n")
	git(t, dir, nil, "add", "-A")
	git(t, dir, nil, "commit", "-q", "-m", "c")
	orig := git(t, dir, nil, "rev-parse", "HEAD:a.py")
	repl := git(t, dir, []byte("replaced\n"), "hash-object", "-w", "--stdin")
	git(t, dir, nil, "replace", orig, repl)

	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	tree, _ := r.HeadTree(ctx)
	entries, _ := r.ListTree(ctx, tree)
	if len(entries) != 1 || entries[0].OID != orig {
		t.Fatalf("entries = %+v, want the tree's own oid %s", entries, orig)
	}
	b, err := r.NewBatch(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	got, err := b.Get(ctx, orig)
	if err != nil || string(got) != "replaced\n" {
		t.Fatalf("content = %q, %v; want the replacement", got, err)
	}
}

// T9: lazy fetch is disabled and prompts are off for every git call.
func TestEnvDisablesLazyFetch(t *testing.T) {
	env := Env([]string{"PATH=/bin", "GIT_NO_LAZY_FETCH=0", "GIT_TERMINAL_PROMPT=1", "GIT_NO_REPLACE_OBJECTS_X=1"})
	for _, want := range []string{"GIT_NO_LAZY_FETCH=1", "GIT_TERMINAL_PROMPT=0", "PATH=/bin"} {
		if !slices.Contains(env, want) {
			t.Fatalf("env %q lacks %s", env, want)
		}
	}
	for _, unwanted := range []string{"GIT_NO_LAZY_FETCH=0", "GIT_TERMINAL_PROMPT=1"} {
		if slices.Contains(env, unwanted) {
			t.Fatalf("env %q keeps caller's %s", env, unwanted)
		}
	}
	if slices.ContainsFunc(env, func(kv string) bool { return strings.HasPrefix(kv, "GIT_NO_REPLACE_OBJECTS=") }) {
		t.Fatal("GIT_NO_REPLACE_OBJECTS must not be set")
	}
}

func TestNotGitRepo(t *testing.T) {
	if _, err := Open(ctx, t.TempDir()); !errors.Is(err, ErrNotGitRepo) {
		t.Fatalf("plain dir: %v, want ErrNotGitRepo", err)
	}
	r, err := Open(ctx, initRepo(t))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := r.HeadTree(ctx); !errors.Is(err, ErrNotGitRepo) {
		t.Fatalf("unborn HEAD: %v, want ErrNotGitRepo", err)
	}
}

func TestBatchStreamsInOrderAndReportsMissing(t *testing.T) {
	dir := initRepo(t)
	var oids []string
	var want []string
	for i := range 60 {
		c := strings.Repeat(string(rune('a'+i%26)), i*1000)
		want = append(want, c)
		oids = append(oids, git(t, dir, []byte(c), "hash-object", "-w", "--stdin"))
	}
	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	b, err := r.NewBatch(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	n := 0
	if err := b.Each(ctx, oids, func(i int, c []byte) error {
		if i != n || string(c) != want[i] {
			t.Fatalf("object %d out of order or wrong content", i)
		}
		n++
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	if n != len(oids) {
		t.Fatalf("streamed %d of %d", n, len(oids))
	}
	// The process stays usable for a second batch.
	if c, err := b.Get(ctx, oids[5]); err != nil || string(c) != want[5] {
		t.Fatalf("reuse: %q %v", c, err)
	}

	_, err = b.Get(ctx, strings.Repeat("0", 40))
	if !errors.Is(err, ErrMissingObject) {
		t.Fatalf("missing object: %v", err)
	}
}

func TestBatchCallbackErrorStops(t *testing.T) {
	dir := initRepo(t)
	oid := git(t, dir, []byte("x"), "hash-object", "-w", "--stdin")
	r, _ := Open(ctx, dir)
	b, err := r.NewBatch(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	stop := errors.New("stop")
	if err := b.Each(ctx, []string{oid, oid, oid}, func(int, []byte) error { return stop }); !errors.Is(err, stop) {
		t.Fatalf("err = %v", err)
	}
}

// W1–W6: the working-directory walk.
func TestWalkWorktree(t *testing.T) {
	dir := initRepo(t)
	write(t, filepath.Join(dir, "a.go"), "package a\n")
	write(t, filepath.Join(dir, ".gitignore"), "target/\n")
	write(t, filepath.Join(dir, "target/gen.rs"), "fn g() {}\n") // ignored, still walked (W4)
	write(t, filepath.Join(dir, "nested/.git"), "gitdir: ../.git/modules/nested\n")
	write(t, filepath.Join(dir, "nested/b.py"), "b = 1\n") // nested repo descended (W2)
	sub := filepath.Join(dir, "inner")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	git(t, sub, nil, "init", "-q")
	write(t, filepath.Join(sub, "c.ts"), "export const c = 1;\n")
	if err := os.Symlink("a.go", filepath.Join(dir, "link.go")); err != nil { // W3
		t.Fatal(err)
	}
	if err := os.Symlink("nested", filepath.Join(dir, "linkdir")); err != nil {
		t.Fatal(err)
	}
	bad := "bad\xc3\x28.go"
	nonUTF8 := os.WriteFile(filepath.Join(dir, bad), []byte("package bad\n"), 0o644) == nil

	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	top, err := r.TopLevel(ctx)
	if err != nil {
		t.Fatal(err)
	}
	format, _ := r.ObjectFormat(ctx)
	entries, err := WalkWorktree(ctx, top, format)
	if err != nil {
		t.Fatal(err)
	}
	var plain []Entry
	for _, e := range entries {
		plain = append(plain, e.Entry)
		if want := git(t, dir, nil, "hash-object", "--no-filters", e.Abs); e.OID != want {
			t.Fatalf("%s oid = %s, want %s (W5)", e.Path, e.OID, want)
		}
	}
	want := []string{".gitignore", "a.go", "inner/c.ts", "nested/b.py", "target/gen.rs"}
	if nonUTF8 {
		want = append(want, bad)
		slices.Sort(want)
	} else {
		t.Logf("filesystem rejects non-UTF-8 names (%s); W6 raw-bytes case covered by TestListTreeNonUTF8Path", runtime.GOOS)
	}
	if got := paths(plain); !slices.Equal(got, want) {
		t.Fatalf("walk = %q, want %q", got, want)
	}

	// A changed file is caught on re-read.
	for _, e := range entries {
		if string(e.Path) == "a.go" {
			write(t, e.Abs, "package changed\n")
			if _, err := ReadWorkEntry(e, format); err == nil {
				t.Fatal("ReadWorkEntry accepted a changed file")
			}
		}
	}
}

func TestSHA256Repository(t *testing.T) {
	dir := initRepo(t, "--object-format=sha256")
	write(t, filepath.Join(dir, "a.go"), "package a\n")
	git(t, dir, nil, "add", "-A")
	git(t, dir, nil, "commit", "-q", "-m", "c")
	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	format, err := r.ObjectFormat(ctx)
	if err != nil || format != "sha256" {
		t.Fatalf("format = %q %v", format, err)
	}
	tree, _ := r.HeadTree(ctx)
	entries, err := r.ListTree(ctx, tree)
	if err != nil || len(entries) != 1 || len(entries[0].OID) != 64 {
		t.Fatalf("entries = %+v %v, want one 64-hex oid (T7)", entries, err)
	}
	walked, err := WalkWorktree(ctx, dir, format)
	if err != nil || len(walked) != 1 || walked[0].OID != entries[0].OID {
		t.Fatalf("walked = %+v %v, want the committed oid", walked, err)
	}
}

func TestParseLsTreeRejectsMalformed(t *testing.T) {
	for _, in := range []string{"100644 blob abc", "100644 blob\tx\x00", "100644 blob abc x\x00"} {
		if _, err := parseLsTree([]byte(in)); err == nil {
			t.Fatalf("parseLsTree(%q) accepted malformed input", in)
		}
	}
}

func TestBatchCancelReportsContextError(t *testing.T) {
	dir := initRepo(t)
	var oids []string
	for i := range 20 {
		oids = append(oids, git(t, dir, []byte(strings.Repeat("x", 1<<16+i)), "hash-object", "-w", "--stdin"))
	}
	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	cctx, cancel := context.WithCancel(ctx)
	b, err := r.NewBatch(cctx)
	if err != nil {
		t.Fatal(err)
	}
	defer b.Close()
	err = b.Each(cctx, oids, func(i int, _ []byte) error {
		if i == 2 {
			cancel()
			time.Sleep(50 * time.Millisecond) // let the kill land mid-stream
		}
		return nil
	})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
}

// The index directory at the walk root is not source; every other .cgx is.
func TestWalkWorktreeSkipsRootIndexDir(t *testing.T) {
	dir := initRepo(t)
	write(t, filepath.Join(dir, "a.go"), "package a\n")
	write(t, filepath.Join(dir, ".cgx", "HEAD.json"), "{}\n")
	write(t, filepath.Join(dir, ".cgx", "objects", "ab", "cd"), "x")
	write(t, filepath.Join(dir, "sub", ".cgx", "kept.go"), "package kept\n")
	entries, err := WalkWorktree(ctx, dir, "sha1")
	if err != nil {
		t.Fatal(err)
	}
	var plain []Entry
	for _, e := range entries {
		plain = append(plain, e.Entry)
	}
	if got, want := paths(plain), []string{"a.go", "sub/.cgx/kept.go"}; !slices.Equal(got, want) {
		t.Fatalf("walk = %q, want %q", got, want)
	}

	// A regular file named .cgx at the root is indexed; a symlink named .cgx
	// is skipped like every symlink.
	fileRepo := initRepo(t)
	write(t, filepath.Join(fileRepo, ".cgx"), "not a directory\n")
	entries, err = WalkWorktree(ctx, fileRepo, "sha1")
	if err != nil || len(entries) != 1 || string(entries[0].Path) != ".cgx" {
		t.Fatalf("root .cgx file: %+v %v", entries, err)
	}
	linkRepo := initRepo(t)
	write(t, filepath.Join(linkRepo, "real", "r.go"), "package r\n")
	if err := os.Symlink("real", filepath.Join(linkRepo, ".cgx")); err != nil {
		t.Fatal(err)
	}
	entries, err = WalkWorktree(ctx, linkRepo, "sha1")
	if err != nil || len(entries) != 1 || string(entries[0].Path) != "real/r.go" {
		t.Fatalf("root .cgx symlink: %+v %v", entries, err)
	}
}

// B2: repository-location variables from the caller (as inside a git hook)
// must not redirect git away from the opened repository.
func TestEnvIgnoresRepositoryLocationVars(t *testing.T) {
	dir := initRepo(t)
	write(t, filepath.Join(dir, "mine.go"), "package mine\n")
	git(t, dir, nil, "add", "-A")
	git(t, dir, nil, "commit", "-q", "-m", "c")
	other := initRepo(t)
	write(t, filepath.Join(other, "theirs.py"), "x = 1\n")
	git(t, other, nil, "add", "-A")
	git(t, other, nil, "commit", "-q", "-m", "c")

	t.Setenv("GIT_DIR", filepath.Join(other, ".git"))
	t.Setenv("GIT_WORK_TREE", other)
	t.Setenv("GIT_INDEX_FILE", filepath.Join(other, ".git", "index"))
	t.Setenv("GIT_OBJECT_DIRECTORY", filepath.Join(other, ".git", "objects"))
	t.Setenv("GIT_COMMON_DIR", filepath.Join(other, ".git"))
	t.Setenv("GIT_CEILING_DIRECTORIES", dir)
	t.Setenv("GIT_CONFIG_COUNT", "1")
	t.Setenv("GIT_CONFIG_KEY_0", "core.worktree")
	t.Setenv("GIT_CONFIG_VALUE_0", other)

	r, err := Open(ctx, dir)
	if err != nil {
		t.Fatal(err)
	}
	tree, err := r.HeadTree(ctx)
	if err != nil {
		t.Fatal(err)
	}
	entries, err := r.ListTree(ctx, tree)
	if err != nil {
		t.Fatal(err)
	}
	if got := paths(entries); !slices.Equal(got, []string{"mine.go"}) {
		t.Fatalf("hostile env redirected git: listed %q, want [mine.go]", got)
	}
	top, err := r.TopLevel(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if want, _ := filepath.EvalSymlinks(dir); top != want {
		t.Fatalf("toplevel = %s, want %s", top, want)
	}
	for _, kv := range r.env {
		name, _, _ := strings.Cut(kv, "=")
		if dropped(name) {
			t.Fatalf("env still carries %s", kv)
		}
	}
}

func TestCommandsDisableLazyFetch(t *testing.T) {
	r, err := Open(ctx, initRepo(t))
	if err != nil {
		t.Fatal(err)
	}
	if args := r.command(ctx, "status").Args; len(args) < 2 || args[1] != "--no-lazy-fetch" {
		t.Fatalf("git args = %q, want --no-lazy-fetch first", args)
	}
}

func TestParseGitVersion(t *testing.T) {
	for in, want := range map[string]gitVersion{
		"git version 2.50.1 (Apple Git-155)\n": {2, 50},
		"git version 2.39.5":                   {2, 39},
		"git version 2.45.0.windows.1":         {2, 45},
	} {
		if got, ok := parseGitVersion(in); !ok || got != want {
			t.Errorf("parseGitVersion(%q) = %v %v, want %v", in, got, ok, want)
		}
	}
	for _, bad := range []string{"", "hub version 2.1", "git version x.y"} {
		if _, ok := parseGitVersion(bad); ok {
			t.Errorf("parseGitVersion(%q) accepted", bad)
		}
	}
	if !(gitVersion{2, 45}).atLeast(noLazyFetchGit) || (gitVersion{2, 44}).atLeast(noLazyFetchGit) || !(gitVersion{3, 0}).atLeast(noLazyFetchGit) {
		t.Fatal("atLeast")
	}
}

// Below git 2.45 lazy fetching cannot be disabled, so a partial clone is
// refused and any other repository is read without --no-lazy-fetch.
func TestLazyFetchGate(t *testing.T) {
	plain := initRepo(t)
	partialByExtension := initRepo(t)
	git(t, partialByExtension, nil, "config", "remote.origin.url", "https://example.invalid/x.git")
	git(t, partialByExtension, nil, "config", "extensions.partialClone", "origin")
	partialByPromisor := initRepo(t)
	git(t, partialByPromisor, nil, "config", "remote.upstream.promisor", "true")
	notPromisor := initRepo(t)
	git(t, notPromisor, nil, "config", "remote.upstream.promisor", "false")
	byFilter := initRepo(t)
	git(t, byFilter, nil, "config", "remote.origin.partialclonefilter", "blob:none")

	old, current := gitVersion{2, 39}, gitVersion{2, 50}
	cases := []struct {
		name    string
		dir     string
		partial bool
	}{
		{"plain", plain, false},
		{"extensions.partialClone", partialByExtension, true},
		{"remote promisor", partialByPromisor, true},
		{"remote promisor=false", notPromisor, false},
		{"partial clone filter", byFilter, true},
	}
	for _, tc := range cases {
		r := &Repo{Dir: tc.dir, git: "git", env: Env(os.Environ())}
		got, err := r.isPartialClone(ctx)
		if err != nil || got != tc.partial {
			t.Errorf("%s: isPartialClone = %v, %v; want %v", tc.name, got, err, tc.partial)
		}
		err = r.checkLazyFetch(ctx, old)
		if tc.partial != errors.Is(err, ErrGitTooOld) {
			t.Errorf("%s with git %s: %v", tc.name, old, err)
		}
		if tc.partial && !strings.Contains(err.Error(), "partial clone") {
			t.Errorf("%s: error does not say why: %v", tc.name, err)
		}
		if r.noLazyFetch {
			t.Errorf("%s: git %s given --no-lazy-fetch", tc.name, old)
		}
		if args := r.command(ctx, "status").Args; slices.Contains(args, "--no-lazy-fetch") {
			t.Errorf("%s: old git called with %q", tc.name, args)
		}
		if err := r.checkLazyFetch(ctx, current); err != nil || !r.noLazyFetch {
			t.Errorf("%s with git %s: %v, noLazyFetch=%v", tc.name, current, err, r.noLazyFetch)
		}
	}
}
