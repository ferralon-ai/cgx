package flock

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

func TestAcquireExcludesSecondHolder(t *testing.T) {
	path := filepath.Join(t.TempDir(), "objects.lock")
	l, err := Acquire(context.Background(), path, time.Second, time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	_, err = Acquire(context.Background(), path, 60*time.Millisecond, 10*time.Millisecond)
	if !errors.Is(err, ErrTimeout) {
		t.Fatalf("second acquire: %v, want ErrTimeout", err)
	}
	if time.Since(start) < 60*time.Millisecond {
		t.Fatalf("gave up before the timeout")
	}
	if err := l.Unlock(); err != nil {
		t.Fatal(err)
	}
	if err := l.Unlock(); err != nil {
		t.Fatalf("second Unlock: %v", err)
	}
	l2, err := Acquire(context.Background(), path, time.Second, time.Millisecond)
	if err != nil {
		t.Fatalf("acquire after unlock: %v", err)
	}
	l2.Unlock()
}

func TestAcquireWaitsForRelease(t *testing.T) {
	path := filepath.Join(t.TempDir(), "objects.lock")
	l, err := Acquire(context.Background(), path, time.Second, time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		time.Sleep(50 * time.Millisecond)
		l.Unlock()
	}()
	l2, err := Acquire(context.Background(), path, 2*time.Second, 5*time.Millisecond)
	if err != nil {
		t.Fatalf("waiter: %v", err)
	}
	l2.Unlock()
}

func TestAcquireHonoursContext(t *testing.T) {
	path := filepath.Join(t.TempDir(), "objects.lock")
	l, err := Acquire(context.Background(), path, time.Second, time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	defer l.Unlock()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := Acquire(ctx, path, time.Second, 5*time.Millisecond); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
}

// The lock must exclude another process, not just another descriptor: that is
// the property that makes the SDK and a native cgx writer exclude each other.
func TestAcquireExcludesOtherProcess(t *testing.T) {
	if p := os.Getenv("CGX_FLOCK_HELPER_PATH"); p != "" {
		l, err := Acquire(context.Background(), p, 50*time.Millisecond, 5*time.Millisecond)
		if errors.Is(err, ErrTimeout) {
			os.Exit(3)
		}
		if err != nil {
			os.Exit(4)
		}
		l.Unlock()
		os.Exit(0)
	}
	path := filepath.Join(t.TempDir(), "objects.lock")
	helper := func() int {
		cmd := exec.Command(os.Args[0], "-test.run=^TestAcquireExcludesOtherProcess$")
		cmd.Env = append(os.Environ(), "CGX_FLOCK_HELPER_PATH="+path)
		err := cmd.Run()
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			return ee.ExitCode()
		}
		if err != nil {
			t.Fatal(err)
		}
		return 0
	}
	l, err := Acquire(context.Background(), path, time.Second, time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	if code := helper(); code != 3 {
		t.Fatalf("child acquiring a held lock exited %d, want 3 (timeout)", code)
	}
	l.Unlock()
	if code := helper(); code != 0 {
		t.Fatalf("child acquiring a free lock exited %d, want 0", code)
	}
}
