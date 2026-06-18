// cgx-fixture: dynamic import and cut markers (TypeScript)
// Covers: dynamic import() with non-literal specifier -> dynamic cut marker,
//         string-literal dynamic import -> probable resolution

/// Dynamic import with a string literal — statically analyzable.
/// cgx can resolve the target: probable confidence.
export async function loadDirect(): Promise<unknown> {
    const mod = await import("./direct");  // resolvable: probable confidence
    return mod;
}

/// Dynamic import with a variable specifier — NOT statically resolvable.
/// cgx emits a cut marker: dynamic.
export async function loadDynamic(moduleName: string): Promise<unknown> {
    const mod = await import(moduleName);  // cut-marker: dynamic (non-literal specifier)
    return mod;
}

/// Dynamic import conditioned on a variable — conditional + dynamic.
export async function conditionalLoad(flag: boolean, name: string): Promise<unknown | null> {
    if (flag) {
        return import(name);  // cut-marker: dynamic, conditional edge
    }
    return null;
}

/// Dynamic import in a loop — dynamic cut marker, loop edge.
export async function loadAll(names: string[]): Promise<unknown[]> {
    const mods: unknown[] = [];
    for (const name of names) {
        const mod = await import(name);  // cut-marker: dynamic, loop edge
        mods.push(mod);
    }
    return mods;
}

/// require() with variable — non-static require also gets dynamic cut marker.
export function requireDynamic(name: string): unknown {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    return require(name);  // cut-marker: dynamic
}

/// require() with a string literal — statically resolvable.
export function requireStatic(): unknown {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    return require("./direct");  // probable confidence
}
