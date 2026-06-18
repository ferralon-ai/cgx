// cgx-fixture: reflection and eval cut markers (TypeScript)
// Covers: eval() -> reflective cut marker,
//         Function() constructor -> reflective cut marker,
//         dynamic property access -> type-confidence boundary (GM-14.6),
//         string-literal getattr-equivalent

/// eval() — reflective cut marker; target is the entire expression.
export function evalCode(code: string): unknown {
    // eslint-disable-next-line no-eval
    return eval(code);  // cut-marker: reflective
}

/// new Function() — reflective cut marker.
export function buildFunction(body: string): (...args: unknown[]) => unknown {
    // eslint-disable-next-line @typescript-eslint/no-implied-eval
    return new Function(body) as (...args: unknown[]) => unknown;  // cut-marker: reflective
}

/// Dynamic property access on 'any' typed object — type-confidence boundary.
export function callDynamic(obj: Record<string, unknown>, methodName: string): unknown {
    const fn = obj[methodName];  // type-confidence boundary: obj[computed] -> possible
    if (typeof fn === "function") {
        return fn();  // calls:virtual, possible (dynamic dispatch via string key)
    }
    return null;
}

/// String-literal property access — cgx can resolve with literal-pedigree.
export function callByLiteralName(obj: Record<string, unknown>): unknown {
    const METHOD = "process";
    const fn = obj[METHOD];  // literal-pedigree: probable resolution
    if (typeof fn === "function") {
        return fn();
    }
    return null;
}

/// Prototype manipulation — reflective pattern.
export function addMethod(proto: object, name: string, fn: Function): void {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (proto as any)[name] = fn;  // reflective: dynamic property write
}

/// JSON.parse result used as method dispatch target — type-confidence boundary.
export function parseAndCall(json: string): unknown {
    const obj = JSON.parse(json) as Record<string, unknown>;  // type erased after parse
    return callDynamic(obj, "handle");  // calls callDynamic which has reflective cut marker
}
