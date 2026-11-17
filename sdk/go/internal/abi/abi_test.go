package abi

import (
	"bytes"
	"encoding/binary"
	"errors"
	"testing"
)

func TestFrameRoundTrip(t *testing.T) {
	cases := []struct {
		name    string
		entries [][]byte
	}{
		{"empty frame", nil},
		{"one empty entry", [][]byte{{}}},
		{"mixed", [][]byte{[]byte(`{"mode":"head"}`), {0xff, 0xfe, 'a'}, {}, bytes.Repeat([]byte{7}, 1<<16)}},
		{"invalid utf8 path bytes", [][]byte{{'s', 'r', 'c', '/', 0xc3, 0x28, '.', 'g', 'o'}}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, err := DecodeFrame(EncodeFrame(tc.entries...))
			if err != nil {
				t.Fatal(err)
			}
			if len(got) != len(tc.entries) {
				t.Fatalf("got %d entries, want %d", len(got), len(tc.entries))
			}
			for i := range got {
				if !bytes.Equal(got[i], tc.entries[i]) {
					t.Fatalf("entry %d differs", i)
				}
			}
		})
	}
}

func TestFrameLayout(t *testing.T) {
	got := EncodeFrame([]byte("ab"), []byte("c"))
	want := []byte{2, 0, 0, 0, 2, 0, 0, 0, 'a', 'b', 1, 0, 0, 0, 'c'}
	if !bytes.Equal(got, want) {
		t.Fatalf("layout = %v, want %v", got, want)
	}
}

func TestDecodeFrameRejectsMalformed(t *testing.T) {
	huge := binary.LittleEndian.AppendUint32(nil, 1<<30)
	cases := []struct {
		name string
		in   []byte
	}{
		{"short header", []byte{1, 0}},
		{"count beyond bytes", huge},
		{"truncated length", []byte{1, 0, 0, 0, 5, 0}},
		{"length beyond bytes", []byte{1, 0, 0, 0, 5, 0, 0, 0, 'a'}},
		{"trailing bytes", append(EncodeFrame([]byte("a")), 9)},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := DecodeFrame(tc.in); !errors.Is(err, ErrMalformed) {
				t.Fatalf("err = %v, want ErrMalformed", err)
			}
		})
	}
}

func TestDecodeFrameEntriesDoNotOverrun(t *testing.T) {
	entries, err := DecodeFrame(EncodeFrame([]byte("ab"), []byte("cd")))
	if err != nil {
		t.Fatal(err)
	}
	// Appending to one entry must not clobber the next (capacity is clipped).
	_ = append(entries[0], 'X')
	if string(entries[1]) != "cd" {
		t.Fatalf("entry 1 = %q after append to entry 0", entries[1])
	}
}

func TestPackUnpack(t *testing.T) {
	for _, tc := range []struct{ ptr, n uint32 }{{0, 0}, {1, 2}, {0xffffffff, 0xffffffff}, {0x10000, 0x7fffffff}} {
		p, n := Unpack(Pack(tc.ptr, tc.n))
		if p != tc.ptr || n != tc.n {
			t.Fatalf("Unpack(Pack(%d,%d)) = %d,%d", tc.ptr, tc.n, p, n)
		}
	}
}

func TestParseResponse(t *testing.T) {
	body, err := ParseResponse(OK([]byte(`{"a":1}`)))
	if err != nil || string(body) != `{"a":1}` {
		t.Fatalf("OK: body=%q err=%v", body, err)
	}

	_, err = ParseResponse(Err(KindResolve, "no symbol matched `x`"))
	var ge *GuestError
	if !errors.As(err, &ge) || ge.Kind != KindResolve || ge.Message != "no symbol matched `x`" {
		t.Fatalf("error envelope: %v", err)
	}

	for _, bad := range [][]byte{nil, {2}, {1, '{'}} {
		if _, err := ParseResponse(bad); !errors.Is(err, ErrMalformed) {
			t.Fatalf("ParseResponse(%v) = %v, want ErrMalformed", bad, err)
		}
	}
}

func TestIndexedAndSubmitEntries(t *testing.T) {
	ib, err := DecodeIndexed(EncodeIndexed(42, []byte("ctx")))
	if err != nil || ib.Index != 42 || string(ib.Payload) != "ctx" {
		t.Fatalf("indexed: %+v %v", ib, err)
	}
	if _, err := DecodeIndexed([]byte{1, 2}); !errors.Is(err, ErrMalformed) {
		t.Fatalf("short indexed: %v", err)
	}

	f := Fragment{Index: 7, FragmentVersion: 3, Facts: []byte{0, 1, 2}}
	got, err := DecodeSubmitEntry(EncodeSubmitEntry(f))
	if err != nil || got.Index != 7 || got.FragmentVersion != 3 || !bytes.Equal(got.Facts, f.Facts) {
		t.Fatalf("submit: %+v %v", got, err)
	}
	if _, err := DecodeSubmitEntry(make([]byte, 7)); !errors.Is(err, ErrMalformed) {
		t.Fatalf("short submit: %v", err)
	}
}

func FuzzDecodeFrame(f *testing.F) {
	f.Add(EncodeFrame([]byte("a"), nil, []byte("bc")))
	f.Add([]byte{0xff, 0xff, 0xff, 0xff})
	f.Fuzz(func(t *testing.T, b []byte) {
		entries, err := DecodeFrame(b)
		if err != nil {
			return
		}
		if !bytes.Equal(EncodeFrame(entries...), b) {
			t.Fatalf("re-encode differs")
		}
	})
}
