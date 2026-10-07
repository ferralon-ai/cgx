package wasmhost

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

type fakeExtractor struct {
	mem      uint64
	grow     uint64
	failOn   string
	poison   bool
	inflight *atomic.Int32
	maxSeen  *atomic.Int32
	closed   bool
}

func (f *fakeExtractor) extract(ctx context.Context, fileCtx, content []byte) (uint32, []byte, error) {
	n := f.inflight.Add(1)
	defer f.inflight.Add(-1)
	for {
		m := f.maxSeen.Load()
		if n <= m || f.maxSeen.CompareAndSwap(m, n) {
			break
		}
	}
	time.Sleep(time.Millisecond)
	f.mem += f.grow
	if f.failOn != "" && string(content) == f.failOn {
		f.poison = true
		return 0, nil, errors.New("trap")
	}
	return 7, append([]byte("facts:"), content...), nil
}
func (f *fakeExtractor) memSize() uint64       { return f.mem }
func (f *fakeExtractor) poisoned() bool        { return f.poison }
func (f *fakeExtractor) close(context.Context) { f.closed = true }

type fakeFactory struct {
	mu       sync.Mutex
	made     []*fakeExtractor
	grow     uint64
	failOn   string
	inflight atomic.Int32
	maxSeen  atomic.Int32
}

func (ff *fakeFactory) new(context.Context) (extractor, error) {
	ff.mu.Lock()
	defer ff.mu.Unlock()
	x := &fakeExtractor{mem: 1 << 20, grow: ff.grow, failOn: ff.failOn, inflight: &ff.inflight, maxSeen: &ff.maxSeen}
	ff.made = append(ff.made, x)
	return x, nil
}

func runPool(t *testing.T, p *pool, n int) (map[uint32]extractResult, error) {
	t.Helper()
	jobs := make(chan extractJob)
	out := make(chan extractResult)
	errc := make(chan error, 1)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go func() { errc <- p.run(ctx, jobs, out) }()
	go func() {
		defer close(jobs)
		for i := range n {
			select {
			case jobs <- extractJob{index: uint32(i), fileCtx: []byte("ctx"), content: []byte(fmt.Sprint(i))}:
			case <-ctx.Done():
				return
			}
		}
	}()
	got := map[uint32]extractResult{}
	for r := range out {
		got[r.index] = r
	}
	err := <-errc
	return got, err
}

func TestPoolRunsEveryJobWithinSize(t *testing.T) {
	ff := &fakeFactory{}
	p := newPool(4, 0, ff.new)
	got, err := runPool(t, p, 200)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 200 {
		t.Fatalf("got %d results", len(got))
	}
	for i := range uint32(200) {
		if r := got[i]; r.fragmentVersion != 7 || string(r.facts) != fmt.Sprint("facts:", i) {
			t.Fatalf("result %d = %+v", i, r)
		}
	}
	if m := ff.maxSeen.Load(); m > 4 || m < 2 {
		t.Fatalf("max concurrency %d, want 2..4", m)
	}
	peaks, started, _ := p.stats()
	if started > 4 || len(peaks) != 4 {
		t.Fatalf("started %d extractors, peaks %v", started, peaks)
	}
	// Slots keep their extractors across runs.
	if _, err := runPool(t, p, 50); err != nil {
		t.Fatal(err)
	}
	if _, again, _ := p.stats(); again != started {
		t.Fatalf("second run started %d more extractors", again-started)
	}
}

func TestPoolStartsLazily(t *testing.T) {
	ff := &fakeFactory{}
	p := newPool(8, 0, ff.new)
	if _, err := runPool(t, p, 1); err != nil {
		t.Fatal(err)
	}
	if _, started, _ := p.stats(); started != 1 {
		t.Fatalf("one job started %d extractors", started)
	}
	if _, err := runPool(t, p, 0); err != nil {
		t.Fatal(err)
	}
}

func TestPoolErrorIsFatal(t *testing.T) {
	ff := &fakeFactory{failOn: "17"}
	p := newPool(3, 0, ff.new)
	got, err := runPool(t, p, 500)
	if err == nil || err.Error() != "trap" {
		t.Fatalf("err = %v, want the extractor's trap", err)
	}
	if len(got) >= 500 {
		t.Fatal("run continued past a fatal error")
	}
	// The trapped extractor is dropped; the next run gets a fresh one.
	ff.failOn = ""
	for _, x := range ff.made {
		x.failOn = ""
	}
	if _, err := runPool(t, p, 20); err != nil {
		t.Fatalf("run after a trap: %v", err)
	}
	if _, started, _ := p.stats(); started < 2 {
		t.Fatalf("started = %d, want the poisoned slot refilled", started)
	}
}

func TestPoolRecyclesGrownExtractors(t *testing.T) {
	ff := &fakeFactory{grow: 1 << 20}
	p := newPool(1, 4<<20, ff.new)
	if _, err := runPool(t, p, 10); err != nil {
		t.Fatal(err)
	}
	peaks, started, recycled := p.stats()
	// Each extractor starts at 1 MiB and grows 1 MiB per job: it passes 4 MiB
	// on its 4th job, so 10 jobs need 3 extractors and recycle 2.
	if recycled != 2 || started != 3 || peaks[0] != 5<<20 {
		t.Fatalf("recycled %d, started %d, peaks %v", recycled, started, peaks)
	}
	for _, x := range ff.made[:2] {
		if !x.closed {
			t.Fatal("recycled extractor not closed")
		}
	}
}

func TestPoolHonoursCancel(t *testing.T) {
	ff := &fakeFactory{}
	p := newPool(2, 0, ff.new)
	ctx, cancel := context.WithCancel(context.Background())
	jobs := make(chan extractJob)
	out := make(chan extractResult)
	errc := make(chan error, 1)
	go func() { errc <- p.run(ctx, jobs, out) }()
	go func() {
		for range out {
		}
	}()
	cancel()
	select {
	case err := <-errc:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("err = %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("pool did not stop on cancel")
	}
}
