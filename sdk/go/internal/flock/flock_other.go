//go:build !unix && !windows

package flock

import (
	"errors"
	"os"
)

func tryLock(*os.File) (bool, error) {
	return false, errors.New("flock: file locking is not supported on this platform")
}

func unlock(*os.File) error { return nil }
