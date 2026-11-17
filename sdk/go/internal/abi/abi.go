// Package abi is the host side of the cgx wasm module's calling convention:
// the ABI version, the length-prefixed frame codec, the packed (ptr, len)
// return value, the status byte and error envelope, and the JSON shapes of the
// control ops. It is pure data handling with no wazero dependency, so both the
// wasm host and tests can use it.
package abi

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
)

// Version is the ABI this host speaks. The guest reports its own through
// cgx_abi_version; any difference is a hard failure.
const Version = 1

// Export names.
const (
	ExportABIVersion  = "cgx_abi_version"
	ExportAlloc       = "cgx_alloc"
	ExportFree        = "cgx_free"
	ExportInfo        = "cgx_info"
	ExportSessionOpen = "cgx_session_open"
	ExportIndexBegin  = "cgx_index_begin"
	ExportIndexPlan   = "cgx_index_plan"
	ExportExtract     = "cgx_extract"
	ExportIndexSubmit = "cgx_index_submit"
	ExportIndexFinish = "cgx_index_finish"
	ExportQuery       = "cgx_query"
	ExportInitialize  = "_initialize"
)

const (
	statusOK           = 0
	statusError        = 1
	frameHeaderLen     = 4
	maxFrameEntryCount = 1 << 28
)

// ErrorKind is the `kind` of a guest error envelope. The first four are the
// MCP tool error variants verbatim.
type ErrorKind string

const (
	KindInvalidParams ErrorKind = "invalid_params"
	KindResolve       ErrorKind = "resolve"
	KindIndex         ErrorKind = "index"
	KindUnimplemented ErrorKind = "unimplemented"
	KindStale         ErrorKind = "stale"
	KindInternal      ErrorKind = "internal"
)

// GuestError is a status-1 response body.
type GuestError struct {
	Kind    ErrorKind `json:"kind"`
	Message string    `json:"message"`
}

func (e *GuestError) Error() string { return fmt.Sprintf("%s: %s", e.Kind, e.Message) }

// ErrMalformed reports bytes from the guest that violate the ABI.
var ErrMalformed = errors.New("cgx abi: malformed response")

// Unpack splits an op export's i64 return into the response pointer and length.
func Unpack(packed uint64) (ptr, length uint32) {
	return uint32(packed >> 32), uint32(packed)
}

// Pack is the inverse of Unpack (used by test guests).
func Pack(ptr, length uint32) uint64 { return uint64(ptr)<<32 | uint64(length) }

// ParseResponse checks the status byte. On status 0 it returns the body; on
// status 1 it returns a *GuestError decoded from the body.
func ParseResponse(resp []byte) ([]byte, error) {
	if len(resp) == 0 {
		return nil, fmt.Errorf("%w: empty response", ErrMalformed)
	}
	switch resp[0] {
	case statusOK:
		return resp[1:], nil
	case statusError:
		var ge GuestError
		if err := json.Unmarshal(resp[1:], &ge); err != nil {
			return nil, fmt.Errorf("%w: error envelope: %v", ErrMalformed, err)
		}
		return nil, &ge
	default:
		return nil, fmt.Errorf("%w: status byte %d", ErrMalformed, resp[0])
	}
}

// OK prefixes body with the success status byte (used by test guests).
func OK(body []byte) []byte { return append([]byte{statusOK}, body...) }

// Err encodes an error envelope response (used by test guests).
func Err(kind ErrorKind, msg string) []byte {
	b, _ := json.Marshal(GuestError{Kind: kind, Message: msg})
	return append([]byte{statusError}, b...)
}

// EncodeFrame lays out entries as u32le count, then count × (u32le len, bytes).
func EncodeFrame(entries ...[]byte) []byte {
	n := frameHeaderLen
	for _, e := range entries {
		n += 4 + len(e)
	}
	out := make([]byte, 0, n)
	out = binary.LittleEndian.AppendUint32(out, uint32(len(entries)))
	for _, e := range entries {
		out = binary.LittleEndian.AppendUint32(out, uint32(len(e)))
		out = append(out, e...)
	}
	return out
}

// DecodeFrame is the inverse of EncodeFrame. Returned entries alias b.
func DecodeFrame(b []byte) ([][]byte, error) {
	if len(b) < frameHeaderLen {
		return nil, fmt.Errorf("%w: frame shorter than its header", ErrMalformed)
	}
	count := binary.LittleEndian.Uint32(b)
	rest := b[frameHeaderLen:]
	// Each entry needs at least its 4-byte length, which bounds count by the
	// remaining bytes before anything is allocated.
	if count > maxFrameEntryCount || uint64(count)*4 > uint64(len(rest)) {
		return nil, fmt.Errorf("%w: frame count %d exceeds %d remaining bytes", ErrMalformed, count, len(rest))
	}
	entries := make([][]byte, 0, count)
	for i := uint32(0); i < count; i++ {
		if len(rest) < 4 {
			return nil, fmt.Errorf("%w: frame entry %d truncated", ErrMalformed, i)
		}
		l := binary.LittleEndian.Uint32(rest)
		rest = rest[4:]
		if uint64(l) > uint64(len(rest)) {
			return nil, fmt.Errorf("%w: frame entry %d length %d exceeds %d remaining bytes", ErrMalformed, i, l, len(rest))
		}
		entries = append(entries, rest[:l:l])
		rest = rest[l:]
	}
	if len(rest) != 0 {
		return nil, fmt.Errorf("%w: %d trailing bytes after frame", ErrMalformed, len(rest))
	}
	return entries, nil
}

// IndexedBlob is one `u32le index ‖ payload` frame entry (plan misses).
type IndexedBlob struct {
	Index   uint32
	Payload []byte
}

// DecodeIndexed splits a `u32le index ‖ payload` entry.
func DecodeIndexed(e []byte) (IndexedBlob, error) {
	if len(e) < 4 {
		return IndexedBlob{}, fmt.Errorf("%w: indexed entry shorter than 4 bytes", ErrMalformed)
	}
	return IndexedBlob{Index: binary.LittleEndian.Uint32(e), Payload: e[4:]}, nil
}

// EncodeIndexed builds a `u32le index ‖ payload` entry.
func EncodeIndexed(index uint32, payload []byte) []byte {
	out := make([]byte, 0, 4+len(payload))
	out = binary.LittleEndian.AppendUint32(out, index)
	return append(out, payload...)
}

// Fragment is one extracted file handed to cgx_index_submit.
type Fragment struct {
	Index           uint32
	FragmentVersion uint32
	Facts           []byte // FileFacts postcard, opaque to the host
}

// EncodeSubmitEntry builds a `u32le index ‖ u32le fragment_version ‖ facts` entry.
func EncodeSubmitEntry(f Fragment) []byte {
	out := make([]byte, 0, 8+len(f.Facts))
	out = binary.LittleEndian.AppendUint32(out, f.Index)
	out = binary.LittleEndian.AppendUint32(out, f.FragmentVersion)
	return append(out, f.Facts...)
}

// DecodeSubmitEntry is the inverse of EncodeSubmitEntry (used by test guests).
func DecodeSubmitEntry(e []byte) (Fragment, error) {
	if len(e) < 8 {
		return Fragment{}, fmt.Errorf("%w: submit entry shorter than 8 bytes", ErrMalformed)
	}
	return Fragment{
		Index:           binary.LittleEndian.Uint32(e),
		FragmentVersion: binary.LittleEndian.Uint32(e[4:]),
		Facts:           e[8:],
	}, nil
}
