package cgx_test

import (
	"context"
	"errors"
	"fmt"
	"log"

	"github.com/ferralon-ai/cgx/sdk/go/cgx"
)

func ExampleOpen() {
	ctx := context.Background()
	g, err := cgx.Open(ctx, ".")
	if errors.Is(err, cgx.ErrNoEmbeddedModule) {
		// No engine in this build: drive the cgx binary instead.
		g, err = cgx.Open(ctx, ".", cgx.WithTransport(cgx.Native("")))
	}
	if err != nil {
		log.Fatal(err)
	}
	defer g.Close()

	report, err := g.Index(ctx)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("graph", report.GraphKey, "up to date:", report.UpToDate)
}

func ExampleGraph_Callers() {
	ctx := context.Background()
	g, err := cgx.Open(ctx, ".")
	if err != nil {
		log.Fatal(err)
	}
	defer g.Close()

	res, err := g.Callers(ctx, cgx.CallersRequest{Symbol: "crate::store::put", Depth: cgx.Ptr(int64(2))})
	var te *cgx.ToolError
	if errors.As(err, &te) && te.Kind == cgx.KindResolve {
		log.Fatalf("no such symbol: %s", te.Message)
	}
	if err != nil {
		log.Fatal(err)
	}
	for _, r := range res.Results {
		fmt.Printf("%s (%s:%d) depth %d, %s\n", r.Name, r.File, r.Line, r.Depth, r.Confidence)
	}
	fmt.Println("approximation:", res.Approximation.Direction)
}

func ExampleGraph_Query() {
	ctx := context.Background()
	g, err := cgx.Open(ctx, ".")
	if err != nil {
		log.Fatal(err)
	}
	defer g.Close()

	res, err := g.Query(ctx, cgx.GraphQueryRequest{
		Query: `MATCH (a)-[:CALLS]->(b) WHERE b.fqn CONTAINS "store" RETURN a.fqn, b.fqn`,
	})
	if err != nil {
		log.Fatal(err)
	}
	if t := res.GraphQueryTableOutput; t != nil {
		fmt.Println(t.Columns, len(t.Rows), "rows")
	}
}
