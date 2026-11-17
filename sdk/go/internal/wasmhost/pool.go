package wasmhost

import (
	"context"
	"sync"
)

// extractor is one extraction worker's engine. The pool owns its lifecycle.
type extractor interface {
	extract(ctx context.Context, fileCtx, content []byte) (fragmentVersion uint32, facts []byte, err error)
	memSize() uint64
	poisoned() bool
	close(ctx context.Context)
}

// extractJob is one cache miss: the guest-planned file context and the blob.
type extractJob struct {
	index   uint32
	fileCtx []byte
	content []byte
}

type extractResult struct {
	index           uint32
	fragmentVersion uint32
	facts           []byte
}

// pool runs extraction over up to size extractors. Slots are filled lazily,
// keep their extractor across runs, and replace it when it trapped or its
// linear memory (which never shrinks) grew past recycle bytes.
type pool struct {
	size    int
	recycle uint64
	newFn   func(ctx context.Context) (extractor, error)

	mu       sync.Mutex
	slots    []extractor
	peaks    []uint64
	started  int
	recycled int
}

func newPool(size int, recycle uint64, newFn func(context.Context) (extractor, error)) *pool {
	if size < 1 {
		size = 1
	}
	return &pool{size: size, recycle: recycle, newFn: newFn, slots: make([]extractor, size), peaks: make([]uint64, size)}
}

// run extracts every job from jobs and sends results to out, closing out when
// done. The first error cancels the run; it is fatal to the whole index, as
// an extraction failure is natively. run returns after every worker stopped.
func (p *pool) run(ctx context.Context, jobs <-chan extractJob, out chan<- extractResult) error {
	ctx, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)
	var wg sync.WaitGroup
	for slot := range p.size {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := p.worker(ctx, slot, jobs, out); err != nil {
				cancel(err)
			}
		}()
	}
	wg.Wait()
	close(out)
	if err := context.Cause(ctx); err != nil && ctx.Err() != nil {
		return err
	}
	return nil
}

func (p *pool) worker(ctx context.Context, slot int, jobs <-chan extractJob, out chan<- extractResult) error {
	for {
		var job extractJob
		var ok bool
		select {
		case <-ctx.Done():
			return nil
		case job, ok = <-jobs:
			if !ok {
				return nil
			}
		}
		x, err := p.get(ctx, slot)
		if err != nil {
			return err
		}
		ver, facts, err := x.extract(ctx, job.fileCtx, job.content)
		p.after(ctx, slot, x)
		if err != nil {
			return err
		}
		select {
		case out <- extractResult{index: job.index, fragmentVersion: ver, facts: facts}:
		case <-ctx.Done():
			return nil
		}
	}
}

func (p *pool) get(ctx context.Context, slot int) (extractor, error) {
	p.mu.Lock()
	x := p.slots[slot]
	p.mu.Unlock()
	if x != nil {
		return x, nil
	}
	x, err := p.newFn(ctx)
	if err != nil {
		return nil, err
	}
	p.mu.Lock()
	p.slots[slot] = x
	p.started++
	p.peaks[slot] = max(p.peaks[slot], x.memSize())
	p.mu.Unlock()
	return x, nil
}

// after records the slot's peak and drops an extractor that trapped or grew
// past the recycle threshold, so the slot's next job starts a fresh one.
func (p *pool) after(ctx context.Context, slot int, x extractor) {
	size := x.memSize()
	p.mu.Lock()
	defer p.mu.Unlock()
	p.peaks[slot] = max(p.peaks[slot], size)
	switch {
	case x.poisoned():
		p.slots[slot] = nil
	case p.recycle > 0 && size > p.recycle:
		x.close(ctx)
		p.slots[slot] = nil
		p.recycled++
	}
}

func (p *pool) stats() (peaks []uint64, started, recycled int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]uint64(nil), p.peaks...), p.started, p.recycled
}

func (p *pool) close(ctx context.Context) {
	p.mu.Lock()
	defer p.mu.Unlock()
	for i, x := range p.slots {
		if x != nil {
			x.close(ctx)
			p.slots[i] = nil
		}
	}
}
