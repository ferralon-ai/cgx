// cgx-fixture: direct calls (README example)
// Covers: ordinary typed calls -> calls:direct (certain)

function stepA(n: number): number {
  return n + 1;
}

function stepB(n: number): number {
  return n * 2;
}

function stepC(n: number): number {
  return n - 3;
}

/// Chained direct calls — every edge resolves certain.
export function chain(n: number): number {
  const a = stepA(n);
  const b = stepB(a);
  return stepC(b);
}
