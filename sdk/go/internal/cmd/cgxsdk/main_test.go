package main

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

func TestUsageErrors(t *testing.T) {
	for _, args := range [][]string{nil, {"bogus", "-repo", t.TempDir()}, {"index", "-transport", "carrier-pigeon"}} {
		var out, errb bytes.Buffer
		if err := run(args, &out, &errb); err == nil {
			t.Fatalf("run(%q) succeeded", args)
		}
	}
}

// Every Stats field the perf harness reads is present in the JSON report.
func TestStatsJSONFields(t *testing.T) {
	b, err := json.Marshal(output{})
	if err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{
		"wall_ns", "open_ns", "compile_ns", "compile_cache_hit", "session_mem_peak_bytes",
		"extractor_mem_peaks_bytes", "extractors_started", "extractors_recycled",
		"enumerate_ns", "plan_ns", "extract_ns", "submit_ns", "finish_ns", "total_ns",
		"bytes_in", "bytes_out", "files_total", "files_extracted", "transport",
	} {
		if !strings.Contains(string(b), `"`+key+`"`) {
			t.Errorf("JSON report lacks %q: %s", key, b)
		}
	}
}
