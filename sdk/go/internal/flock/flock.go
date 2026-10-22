// Package flock takes the exclusive advisory lock cgx's object store uses for
// writers: flock(2) on unix and LockFileEx on windows, the primitives the
// native store's fd-lock uses, so an SDK writer and a native `cgx` writer on
// the same repository exclude each other through one lock file.
package flock

import (
	"context"
	"errors"
	"os"
	"time"
)

// Defaults match the native store (crates/cgx-store/src/lock.rs).
const (
	DefaultTimeout = 2 * time.Second
	DefaultPoll    = 20 * time.Millisecond
)

// ErrTimeout is returned when the lock stays held for the whole timeout.
var ErrTimeout = errors.New("timed out waiting for the cgx store write lock")

// Lock is a held lock. Unlock releases it and closes the file.
type Lock struct{ f *os.File }

// Acquire opens (creating if needed) path and polls for an exclusive lock
// every poll until timeout elapses or ctx is done.
func Acquire(ctx context.Context, path string, timeout, poll time.Duration) (*Lock, error) {
	f, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o644)
	if err != nil {
		return nil, err
	}
	deadline := time.Now().Add(timeout)
	for {
		ok, err := tryLock(f)
		if err != nil {
			f.Close()
			return nil, err
		}
		if ok {
			return &Lock{f: f}, nil
		}
		if !time.Now().Before(deadline) {
			f.Close()
			return nil, ErrTimeout
		}
		t := time.NewTimer(poll)
		select {
		case <-ctx.Done():
			t.Stop()
			f.Close()
			return nil, ctx.Err()
		case <-t.C:
		}
	}
}

// Unlock releases the lock. It is safe to call more than once.
func (l *Lock) Unlock() error {
	if l == nil || l.f == nil {
		return nil
	}
	err := unlock(l.f)
	if cerr := l.f.Close(); err == nil {
		err = cerr
	}
	l.f = nil
	return err
}
