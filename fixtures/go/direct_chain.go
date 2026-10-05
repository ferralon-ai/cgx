package go_sample

// Chained direct calls — every callee resolves exact/certain.
func Chain(n int) int {
	a := StepA(n)
	b := StepB(a)
	return StepC(b)
}

func StepA(n int) int { return n + 1 }
func StepB(n int) int { return n * 2 }
func StepC(n int) int { return n - 3 }
