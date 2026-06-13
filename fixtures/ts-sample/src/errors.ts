// cgx-fixture: exception edges (TypeScript try/catch/finally)
// Covers: exception edge (catch block), always edge (finally block),
//         GM-3.1 JS/TS mapping, re-throw propagation

function readString(input: string): string {
    if (!input) throw new Error("empty input");
    return input;
}

function parseToInt(s: string): number {
    const n = parseInt(s, 10);
    if (isNaN(n)) throw new Error(`not a number: ${s}`);
    return n;
}

function checkRange(n: number): number {
    if (n < 0 || n > 100) throw new RangeError(`out of range: ${n}`);
    return n;
}

function formatResult(n: number): string {
    return `value=${n}`;
}

/// tryParse: throws on bad input — exception edges flow to callers' catch blocks.
export function tryParse(input: string): number | null {
    try {
        const s = readString(input);    // always (inside try body)
        const n = parseToInt(s);        // always
        return checkRange(n);           // always
    } catch (e) {
        logParseError(e);               // exception: catch body — GM-3.1
        return null;
    }
}

function logParseError(e: unknown): void {
    console.error("parse error:", e);
}

/// finally block — calls inside are always (edge_condition=always).
export function withCleanup(input: string): string {
    let result = "";
    try {
        result = formatResult(parseToInt(input));  // always
    } catch (e) {
        handleError(e);                            // exception: catch body
    } finally {
        releaseResources();                        // always: finally block — GM-3.1
    }
    return result;
}

function handleError(e: unknown): void {
    console.error("error:", e);
}

function releaseResources(): void {
    console.log("released");
}

/// Re-throw in catch — calls inside catch are exception edges; then throws again.
export function validateAndParse(input: string): number {
    try {
        return parseToInt(readString(input));  // always
    } catch (e) {
        logRethrow(e);    // exception: catch body
        throw e;          // re-throw: exception propagates further
    }
}

function logRethrow(e: unknown): void {
    console.error("rethrowing:", e);
}

/// Nested try/catch — inner exception caught, outer not.
export function nestedTry(input: string): string {
    try {
        let n: number;
        try {
            n = parseToInt(input);              // always (inner try)
        } catch (innerErr) {
            logInnerError(innerErr);            // exception (inner catch)
            n = 0;
        }
        return formatResult(checkRange(n));     // always (outer try, after inner)
    } catch (outerErr) {
        logOuterError(outerErr);                // exception (outer catch)
        return "error";
    }
}

function logInnerError(e: unknown): void { console.error("inner:", e); }
function logOuterError(e: unknown): void { console.error("outer:", e); }
