// Own-effects fixture: a function that performs network + spawn effects, and a
// pure companion the resolver must leave effect-free.

export async function loadRemote(url: string): Promise<unknown> {
  const res = await fetch(url);
  setTimeout(() => {}, 0);
  return res;
}

export function add(a: number, b: number): number {
  return a + b;
}
