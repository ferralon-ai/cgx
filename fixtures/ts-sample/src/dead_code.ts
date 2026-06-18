// cgx-fixture: dead code (TypeScript)
// Covers: exported symbols never called from any entrypoint,
//         transitively dead helper functions

/// Exported but never imported or called from any entrypoint.
export function orphanComputation(x: number): number {
    return orphanHelper(x) * 2;
}

function orphanHelper(x: number): number {
    return x + 7;
}

export class UnusedService {
    private data: number[];

    constructor(data: number[]) {
        this.data = data;
    }

    /// Never called — dead method.
    process(): number {
        return this.data.reduce((a, b) => a + b, 0);
    }

    /// Also never called.
    reset(): void {
        this.data = [];
    }

    private internalCheck(): boolean {
        return this.data.length > 0;
    }
}

/// Called only by UnusedService.process — transitively dead.
function computeAverage(items: number[]): number {
    const sum = items.reduce((a, b) => a + b, 0);
    return sum / items.length;
}

/// Dead constant.
export const UNUSED_LIMIT = 42;

/// Dead function referencing dead constant.
export function checkLimit(x: number): boolean {
    return x < UNUSED_LIMIT;
}
