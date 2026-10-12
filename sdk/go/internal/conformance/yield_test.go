package conformance

import (
	"context"
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"runtime/metrics"
	"strings"
	"testing"
	"time"

	"github.com/ferralon-ai/cgx/sdk/go/cgx"
)

// TestCancelDuringStoreWrite indexes a generated repository of yieldFiles Go
// files of yieldPerFile functions each, which writes thousands of objects: the
// store write runs long enough to be cancelled part-way, and the index fits a
// unit-test budget.
const (
	yieldFiles   = 200
	yieldPerFile = 100
)

// gcFuncs is the size of TestIndexGCPause's single-file workload. Resolving
// the calls in one file scans the file's definitions, so one large file gives
// link a long stretch of allocation-free work with no WASI call in it: the
// stretch only the engine's yield points break. (Many small files would not
// do: their per-file work is short and the reads between them are host calls,
// where the Go runtime can stop the goroutine anyway.)
const gcFuncs = 6000

// genRepo writes and commits a one-package Go repository of files files of
// perFile functions each: F0…Fn-1, each calling the next two.
func genRepo(t *testing.T, files, perFile int) string {
	t.Helper()
	dir := t.TempDir()
	n := files * perFile
	for f := range files {
		var b strings.Builder
		b.WriteString("package big\n\n")
		for j := range perFile {
			i := f*perFile + j
			fmt.Fprintf(&b, "func F%d(x int) int {\n\tif x > %d {\n\t\treturn F%d(x-1) + F%d(x-2)\n\t}\n\treturn x\n}\n\n", i, i%7, (i+1)%n, (i+2)%n)
		}
		if err := os.WriteFile(filepath.Join(dir, fmt.Sprintf("f%03d.go", f)), []byte(b.String()), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/big\n\ngo 1.22\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	git(t, dir, "init", "-q", "-b", "main")
	git(t, dir, "add", "-A")
	git(t, dir, "commit", "-q", "-m", "workload")
	return dir
}

// gcStallBound is the longest a forced garbage collection (runtime.GC, start
// to finish) may take while the wasm engine indexes TestIndexGCPause's
// workload.
//
// The session instance runs without wazero's termination checks, so the Go
// runtime can preempt the goroutine running it only when the engine calls out
// to the host: at a yield or a WASI call. A collection must stop that
// goroutine (to stop the world, and to scan its stack), so a stretch of engine
// work with no yield point holds every collection in the process for its whole
// length. The runtime's stop-the-world histogram shows only part of that wait
// (the stack-scan wait is not a pause), so the test bounds the collection's
// wall time and logs the histogram's longest pause beside it.
//
// Measured on a 10-core Apple M-series Mac, four to six runs per row, at load
// averages 5 to 35 (the higher ones with ten extra busy-looping processes):
// with the engine's yield points, the longest collection took 22–53 ms; with
// the yield import kept but never called, 3.7–8.3 s; with only link's
// definition-scan yield points removed, 1.0–1.7 s. On a heavier 20,000-function
// workload under unrelated machine load, healthy collections reached 665 ms.
// The bound sits above all healthy runs seen and well below a total loss of
// yield points, so a loaded CI runner (or -race) does not trip it; a partial
// loss confined to one loop is caught only when it is as large as that one.
const gcStallBound = time.Second

const pauseMetric = "/sched/pauses/total/gc:seconds"

func gcPauses() *metrics.Float64Histogram {
	s := []metrics.Sample{{Name: pauseMetric}}
	metrics.Read(s)
	return s[0].Value.Float64Histogram()
}

// longestNewPause is the lower edge of the highest histogram bucket that
// gained a pause between before and after (0 if none did).
func longestNewPause(before, after *metrics.Float64Histogram) time.Duration {
	for i := len(after.Counts) - 1; i >= 0; i-- {
		if after.Counts[i] > before.Counts[i] {
			lo := after.Buckets[i]
			if math.IsInf(lo, -1) {
				return 0
			}
			return time.Duration(lo * float64(time.Second))
		}
	}
	return 0
}

// TestIndexGCPause (wasm only) indexes the single-file workload while a probe
// forces a garbage collection every 10 ms, and fails if any of them takes
// longer than gcStallBound: some engine work then runs without a yield point.
// The runtime's longest stop-the-world pause over the index is logged too.
func TestIndexGCPause(t *testing.T) {
	for _, e := range transports(t) {
		if e.name != "wasm" {
			continue
		}
		g := e.open(t, genRepo(t, 1, gcFuncs), cgx.WithAutoIndex(false))

		stop, worst := make(chan struct{}), make(chan time.Duration)
		go func() {
			var w time.Duration
			tick := time.NewTicker(10 * time.Millisecond)
			defer tick.Stop()
			for {
				select {
				case <-stop:
					worst <- w
					return
				case <-tick.C:
					start := time.Now()
					runtime.GC()
					w = max(w, time.Since(start))
				}
			}
		}()
		before := gcPauses()
		start := time.Now()
		_, err := g.Index(context.Background())
		elapsed := time.Since(start)
		after := gcPauses()
		close(stop)
		probe := <-worst
		if err != nil {
			t.Fatal(err)
		}
		var n uint64
		for i := range after.Counts {
			n += after.Counts[i] - before.Counts[i]
		}
		pause := longestNewPause(before, after)
		t.Logf("index %v; %d stop-the-world pauses, longest ≥ %v; longest runtime.GC %v", elapsed, n, pause, probe)
		if probe > gcStallBound {
			t.Fatalf("a garbage collection took %v during the index (bound %v): some engine work runs without a yield point", probe, gcStallBound)
		}
	}
}

// TestCancelDuringStoreWrite (wasm only) cancels a cold index as soon as the
// store write has begun (its first object appears) and checks that the call
// returns promptly with the context's error, that nothing names the partial
// write (no HEAD.json, no ref) so the store opens to no graph, and that a
// later index completes and writes the same store as an uninterrupted one.
func TestCancelDuringStoreWrite(t *testing.T) {
	for _, e := range transports(t) {
		if e.name != "wasm" {
			continue
		}
		repo := genRepo(t, yieldFiles, yieldPerFile)
		g := e.open(t, repo, cgx.WithAutoIndex(false))
		objects := filepath.Join(repo, ".cgx", "objects")

		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		cancelled := make(chan time.Time, 1)
		go func() {
			for ctx.Err() == nil {
				if ents, _ := os.ReadDir(objects); len(ents) > 0 {
					cancelled <- time.Now()
					cancel()
					return
				}
				time.Sleep(time.Millisecond)
			}
		}()
		_, err := g.Index(ctx)
		returned := time.Now()
		if err == nil {
			t.Fatal("the index completed before its store write was seen; enlarge the workload")
		}
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("err = %v, want context.Canceled", err)
		}
		latency := returned.Sub(<-cancelled)
		t.Logf("cancel → return %v", latency)
		// One shard's write per yield: milliseconds. The bound leaves room
		// for a loaded machine and -race.
		if latency > 2*time.Second {
			t.Fatalf("cancel during the store write took %v to return", latency)
		}
		if ents, _ := os.ReadDir(objects); len(ents) == 0 {
			t.Fatal("no objects written: the cancel did not land in the store write")
		}
		if _, err := os.Stat(filepath.Join(repo, ".cgx", "HEAD.json")); !errors.Is(err, os.ErrNotExist) {
			t.Fatalf("HEAD.json after a cancelled cold index: %v", err)
		}
		if refs, _ := os.ReadDir(filepath.Join(repo, ".cgx", "refs")); len(refs) != 0 {
			t.Fatalf("refs after a cancelled cold index: %v", refs)
		}
		g.Close()

		g2 := e.open(t, repo, cgx.WithAutoIndex(false))
		if _, err := g2.Index(context.Background()); err != nil {
			t.Fatalf("index after cancel: %v", err)
		}
		ref := genRepo(t, yieldFiles, yieldPerFile)
		gr := e.open(t, ref, cgx.WithAutoIndex(false))
		if _, err := gr.Index(context.Background()); err != nil {
			t.Fatal(err)
		}
		if d := diffStores(storeFiles(t, repo), storeFiles(t, ref)); len(d) != 0 {
			t.Fatalf("store after cancel + reindex differs from an uninterrupted index: %v", d)
		}
	}
}
