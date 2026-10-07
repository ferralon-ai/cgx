//go:build !unix

package main

func maxRSS() (self, children uint64) { return 0, 0 }
