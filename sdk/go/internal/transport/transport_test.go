package transport

import (
	"bytes"
	"errors"
	"strings"
	"testing"

	"github.com/ferralon-ai/cgx/sdk/go/internal/abi"
)

func TestRingKeepsTail(t *testing.T) {
	var tee bytes.Buffer
	r := NewRing(&tee)
	r.Write([]byte("head-"))
	r.Write(bytes.Repeat([]byte("x"), DiagnosticsSize-2))
	r.Write([]byte("tail"))
	s := r.String()
	if len(s) != DiagnosticsSize || !strings.HasSuffix(s, "xxtail") || strings.HasPrefix(s, "head") {
		t.Fatalf("ring len %d, suffix %q", len(s), s[len(s)-8:])
	}
	if tee.Len() != 5+DiagnosticsSize-2+4 {
		t.Fatalf("tee got %d bytes", tee.Len())
	}
	r.Write(bytes.Repeat([]byte("y"), DiagnosticsSize+10))
	if s := r.String(); len(s) != DiagnosticsSize || strings.Contains(s, "x") {
		t.Fatal("oversized write did not replace the tail")
	}
	r.Reset()
	if r.String() != "" {
		t.Fatal("Reset kept bytes")
	}
}

func TestFromGuest(t *testing.T) {
	cases := []struct {
		kind  abi.ErrorKind
		check func(error) bool
	}{
		{abi.KindResolve, func(err error) bool {
			var te *ToolError
			return errors.As(err, &te) && te.Kind == Resolve && te.Tool == "callers"
		}},
		{abi.KindInvalidParams, func(err error) bool {
			var te *ToolError
			return errors.As(err, &te) && te.Kind == InvalidParams
		}},
		{abi.KindUnimplemented, func(err error) bool {
			var te *ToolError
			return errors.As(err, &te) && te.Kind == Unimplemented
		}},
		{abi.KindStale, func(err error) bool { return errors.Is(err, ErrNotIndexed) }},
		{abi.KindInternal, func(err error) bool {
			var ee *EngineError
			return errors.As(err, &ee) && ee.Kind == Internal && ee.Transport == "wasm"
		}},
	}
	for _, tc := range cases {
		err := FromGuest("wasm", "cgx_query", "callers", &abi.GuestError{Kind: tc.kind, Message: "m"})
		if !tc.check(err) {
			t.Fatalf("%s mapped to %T %v", tc.kind, err, err)
		}
	}
}

func TestEngineErrorWrapsSentinel(t *testing.T) {
	err := error(&EngineError{Transport: "wasm", Op: "cgx_index_finish", Kind: Trap, Err: ErrMemoryLimit})
	if !errors.Is(err, ErrMemoryLimit) {
		t.Fatal("errors.Is(ErrMemoryLimit) failed through EngineError")
	}
}
