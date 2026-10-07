//go:build unix

package main

import (
	"runtime"
	"syscall"
)

// maxRSS returns the peak resident set size in bytes of this process and of
// its largest waited-for child. For the native transport the engine is the
// child; Close waits for it, so its peak is known once the Graph is closed.
func maxRSS() (self, children uint64) {
	return rusage(syscall.RUSAGE_SELF), rusage(syscall.RUSAGE_CHILDREN)
}

func rusage(who int) uint64 {
	var ru syscall.Rusage
	if syscall.Getrusage(who, &ru) != nil {
		return 0
	}
	if runtime.GOOS == "darwin" {
		return uint64(ru.Maxrss) // bytes on darwin
	}
	return uint64(ru.Maxrss) * 1024 // KiB elsewhere
}
