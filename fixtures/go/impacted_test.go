package go_sample

import "testing"

// TestPlain calls Transform directly from the test body, so the reverse walk
// reaches it over an ordinary call edge.
func TestPlain(t *testing.T) {
	if Transform(1) != 2 {
		t.Fatal("Transform(1)")
	}
}

// TestSub's only call to Transform sits inside the t.Run subtest closure. The
// closure is a Lambda node whose FQN is `go_sample::TestSub::{func@L:C}`, and no
// call edge links TestSub to it (the closure-argument value-flow gap), so this
// test is reachable ONLY through the containment lift from the Lambda to its
// lexically enclosing named function.
func TestSub(t *testing.T) {
	t.Run("case", func(t *testing.T) {
		if Transform(2) != 3 {
			t.Fatal("Transform(2)")
		}
	})
}
