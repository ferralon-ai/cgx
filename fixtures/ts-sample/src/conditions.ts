// cgx-fixture: conditional and loop edges (TypeScript)
// Covers: conditional (if/switch/ternary), loop (for/while/forEach body)

function logInfo(msg: string): void { console.log(`[INFO] ${msg}`); }
function logWarn(msg: string): void { console.warn(`[WARN] ${msg}`); }
function logError(msg: string): void { console.error(`[ERROR] ${msg}`); }
function processItem(x: number): number { return x * 2; }
function cleanup(): void { logInfo("cleanup"); }
function validate(x: number): boolean { return x > 0; }

/// Simple if/else — conditional edges to logInfo, logWarn.
export function maybeLog(flag: boolean): void {
    if (flag) {
        logInfo("flag is true");    // conditional: if-branch
    } else {
        logWarn("flag is false");   // conditional: else-branch
    }
}

/// switch/case — each case produces a conditional edge.
export function dispatch(code: number): string {
    switch (code) {
        case 0:
            logInfo("zero"); return "zero";    // conditional
        case 1:
            logWarn("one"); return "one";      // conditional
        default:
            logError("other"); return "other"; // conditional
    }
}

/// for loop — loop edge inside the body.
export function processAll(items: number[]): number[] {
    const out: number[] = [];
    for (const x of items) {
        out.push(processItem(x));  // loop edge
    }
    return out;
}

/// while loop.
export function countdown(n: number): number[] {
    const v: number[] = [];
    while (n > 0) {
        processItem(n);  // loop edge
        v.push(n);
        n--;
    }
    return v;
}

/// forEach — loop edge inside the callback body.
export function forEachProcess(items: number[]): void {
    items.forEach(x => {
        processItem(x);  // loop edge (inside forEach callback)
    });
}

/// Ternary expression — conditional edges.
export function clamp(x: number, lo: number, hi: number): number {
    return x < lo ? lo : x > hi ? hi : x;  // conditional
}

/// Conditional inside loop.
export function conditionalLoop(items: number[], threshold: number): void {
    if (items.length > 0) {                // conditional
        for (const x of items) {
            if (x > threshold) {           // conditional inside loop
                processItem(x);            // loop + conditional
            }
        }
        cleanup();                         // conditional (after loop, inside outer if)
    }
}

/// Optional chaining — conditional call if value is non-null.
export function maybeValidate(x: number | null): boolean {
    return x !== null && validate(x);  // conditional: only calls validate if x != null
}
