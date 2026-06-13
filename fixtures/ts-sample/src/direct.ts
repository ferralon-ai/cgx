// cgx-fixture: direct function calls (TypeScript)
// Covers: calls (probable with SCIP, possible without),
//         method calls on typed class instances (certain with type info),
//         static method calls

export function add(a: number, b: number): number {
    return innerAdd(a, b);
}

function innerAdd(a: number, b: number): number {
    return a + b;
}

export function identity<T>(x: T): T {
    return cloneValue(x);
}

function cloneValue<T>(x: T): T {
    return x;
}

export class Counter {
    private value: number;

    constructor(start: number) {
        this.value = start;
    }

    increment(): void {
        this.value = this.addOne(this.value);  // certain: typed receiver
    }

    private addOne(n: number): number {
        return n + 1;
    }

    get(): number {
        return this.value;
    }

    static create(start: number): Counter {
        return new Counter(start);
    }
}

export function chain(n: number): number {
    const a = stepA(n);
    const b = stepB(a);
    return stepC(b);
}

function stepA(n: number): number { return n + 1; }
function stepB(n: number): number { return n * 2; }
function stepC(n: number): number { return n - 3; }

export function useCounter(): number {
    const c = Counter.create(0);  // direct static method call
    c.increment();
    c.increment();
    return c.get();
}
