// cgx-fixture: throw paths (TypeScript) — panic-equivalent label
// Covers: throw new Error() -> panic label (unrecovered throw reaching exit),
//         throw inside condition -> conditional + panic,
//         throw inside loop -> loop + panic

function abort(msg: string): never {
    throw new Error(msg);  // panic: unrecoverable throw
}

function assertPositive(n: number): void {
    if (n <= 0) {
        abort(`expected positive, got ${n}`);  // panic: conditional path
    }
}

function getFirst<T>(items: T[]): T {
    if (items.length === 0) {
        throw new Error("items must not be empty");  // panic
    }
    return items[0];
}

/// mustPositive delegates to assertPositive which throws.
export function mustPositive(n: number): number {
    assertPositive(n);  // panic in callee
    return n;
}

/// Throwing inside a loop — panic edge with loop context.
export function throwInLoop(items: number[]): number[] {
    const out: number[] = [];
    for (const x of items) {
        if (x < 0) {
            throw new RangeError(`negative value: ${x}`);  // panic inside loop
        }
        out.push(x * 2);
    }
    return out;
}

/// Conditional throw — panic on the error branch.
export function divideOrThrow(a: number, b: number): number {
    if (b === 0) {
        throw new Error("division by zero");  // panic: conditional
    }
    return a / b;
}

/// extractAndParse: panic chain — calls functions that can throw.
export function extractAndParse(items: string[]): number {
    const first = getFirst(items);          // panic in callee
    assertPositive(parseInt(first, 10));    // panic in callee
    return parseInt(first, 10);
}

/// Combination: conditional path can panic, happy path returns normally.
export function safeOrPanic(items: number[], requirePositive: boolean): number {
    const first = getFirst(items);          // panic (callee)
    if (requirePositive) {
        assertPositive(first);              // panic: conditional path only
    }
    return Math.abs(first);
}
