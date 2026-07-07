// Intraprocedural dataflow fixture for the ts_e2e smoke.
// Transform chain: a -> b (copy) -> c (arith) -> return.

export function transform(a: number): number {
  const b = a;
  const c = b + 1;
  return c;
}
