package go_sample

import (
	. "math"
	h "net/http"
	_ "net/http/pprof"
)

func Serve() { _ = h.StatusOK; _ = Pi }
