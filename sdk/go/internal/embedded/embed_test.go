package embedded

import (
	"bytes"
	"testing"
)

func TestModuleIsWasmWhenPresent(t *testing.T) {
	b, ok := Module()
	if !ok {
		t.Skip("no embedded module in this build")
	}
	if !bytes.HasPrefix(b, []byte("\x00asm")) {
		t.Fatalf("embedded module lacks the wasm magic: % x", b[:min(8, len(b))])
	}
}
