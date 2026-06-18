// cgx-fixture: closures and higher-order functions (TypeScript)
// Covers: calls:closure (arrow function invocation), calls:callback (fn-value arg),
//         closure captures, .map/.filter/.reduce iterator callbacks

/// Arrow function stored in variable and invoked — calls:closure.
export function closureVariable(): number {
    const double = (x: number) => x * 2;
    return double(5);
}

/// .map() with arrow function — calls:closure on map body.
export function doubleAll(items: number[]): number[] {
    return items.map(x => x * 2);
}

/// .filter() — calls:closure (predicate).
export function filterEvens(items: number[]): number[] {
    return items.filter(x => x % 2 === 0);
}

/// .reduce() — calls:closure (reducer).
export function sumArray(items: number[]): number {
    return items.reduce((acc, x) => acc + x, 0);
}

/// Closure that captures outer variable — captured binding.
export function makeAdder(n: number): (x: number) => number {
    return (x: number) => x + n;  // captures n
}

export function useCapturedClosure(): number {
    const add5 = makeAdder(5);
    return add5(10);
}

/// Higher-order function — calls:callback on f(value).
export function apply(f: (x: number) => number, value: number): number {
    return f(value);
}

export function useCallback(): number {
    return apply(x => x + 1, 41);  // calls:callback (arrow fn as argument)
}

/// Function reference as argument — calls:callback.
function negate(x: number): number { return -x; }

export function transformList(items: number[], f: (x: number) => number): number[] {
    return items.map(f);  // calls:callback on f inside map
}

export function negateList(items: number[]): number[] {
    return transformList(items, negate);  // negate passed as callback
}

/// Nested closures.
export function nestedClosures(): number {
    const outer = (x: number) => {
        const inner = (y: number) => y * y;
        return inner(x) + x;
    };
    return outer(3);
}

/// Method reference as callback — calls:callback.
export class Formatter {
    prefix: string;

    constructor(prefix: string) {
        this.prefix = prefix;
    }

    format(s: string): string {
        return `${this.prefix}${s}`;
    }
}

export function formatAll(items: string[], fmt: Formatter): string[] {
    return items.map(s => fmt.format(s));  // calls:closure with captured fmt
}
