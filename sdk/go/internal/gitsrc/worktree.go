package gitsrc

import (
	"context"
	"crypto/sha1"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"hash"
	"os"
	"path/filepath"
	"strconv"
)

// WorkEntry is one working-directory file. Abs is where to re-read it.
type WorkEntry struct {
	Entry
	Abs string
}

// WalkWorktree reproduces the native working-directory walk
// (crates/cgx-index/src/git.rs walk_dir): every entry named exactly `.git` is
// skipped at every depth, file or directory; the directory named exactly
// `.cgx` directly under root (cgx's own index) is skipped, while a deeper
// `.cgx` is walked and a root file named `.cgx` is indexed; directories
// recurse (nested repositories included); regular files are indexed; symlinks
// and anything else are skipped without being followed. No ignore rules apply. OIDs are the
// repository's hash over `blob <len>\0<content>`.
func WalkWorktree(ctx context.Context, root, objectFormat string) ([]WorkEntry, error) {
	var out []WorkEntry
	err := walk(ctx, root, nil, objectFormat, &out)
	return out, err
}

func walk(ctx context.Context, dir string, rel []byte, objectFormat string, out *[]WorkEntry) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	for _, e := range entries {
		name := e.Name()
		if name == ".git" {
			continue
		}
		// cgx's own index directory at the walk root is never source.
		if len(rel) == 0 && name == ".cgx" && e.Type().IsDir() {
			continue
		}
		abs := filepath.Join(dir, name)
		child := make([]byte, 0, len(rel)+1+len(name))
		if len(rel) > 0 {
			child = append(append(child, rel...), '/')
		}
		child = append(child, name...)
		switch t := e.Type(); {
		case t.IsDir():
			if err := walk(ctx, abs, child, objectFormat, out); err != nil {
				return err
			}
		case t.IsRegular():
			content, err := os.ReadFile(abs)
			if err != nil {
				return err
			}
			oid, err := BlobOID(objectFormat, content)
			if err != nil {
				return err
			}
			*out = append(*out, WorkEntry{Entry: Entry{Path: child, OID: oid}, Abs: abs})
		}
		// Anything else (symlinks, sockets, devices) is neither indexed nor followed.
	}
	return nil
}

// BlobOID is git's object ID for a blob with content.
func BlobOID(objectFormat string, content []byte) (string, error) {
	var h hash.Hash
	switch objectFormat {
	case "sha1", "":
		h = sha1.New()
	case "sha256":
		h = sha256.New()
	default:
		return "", fmt.Errorf("unsupported object format %q", objectFormat)
	}
	h.Write([]byte("blob " + strconv.Itoa(len(content)) + "\x00"))
	h.Write(content)
	return hex.EncodeToString(h.Sum(nil)), nil
}

// ReadWorkEntry re-reads a walked file and checks it still has the walked OID.
func ReadWorkEntry(e WorkEntry, objectFormat string) ([]byte, error) {
	content, err := os.ReadFile(e.Abs)
	if err != nil {
		return nil, err
	}
	oid, err := BlobOID(objectFormat, content)
	if err != nil {
		return nil, err
	}
	if oid != e.OID {
		return nil, fmt.Errorf("%q changed while indexing", e.Path)
	}
	return content, nil
}
