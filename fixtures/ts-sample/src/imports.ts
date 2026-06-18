// cgx-fixture: ESM import/re-export chains (TypeScript)
// Covers: named imports, default imports, re-exports, aliased imports,
//         cross-file call edges resolved via import graph -> probable confidence

// Named import from direct.ts
import { add, Counter } from "./direct";

// Named import under an alias
import { add as sumTwo } from "./direct";

// Re-export: makes direct::add available from this module
export { add as reexportedAdd } from "./direct";

// Re-export everything from errors.ts
export * from "./errors";

// Named import from virtual_dispatch.ts
import type { Speakable } from "./virtual_dispatch";
import { Dog, makeSpeak } from "./virtual_dispatch";

/// Uses a named import — callee resolves to direct::add via import graph.
export function doubleViaImport(n: number): number {
    return sumTwo(n, n);
}

/// Cross-module type usage.
export function makeDogSpeak(): string {
    const dog = new Dog();
    return makeSpeak(dog);   // probable: import resolves to virtual_dispatch::makeSpeak
}

/// Uses an imported class.
export function useCounter(): number {
    const c = new Counter(0);
    c.increment();
    return c.get();
}

/// Re-exported function used here — tests re-export resolution.
export function useReexported(n: number): number {
    return reexportedAdd(n, 10);  // resolves through re-export chain
}

/// Module that re-exports from a re-export chain.
export const utils = {
    double: (n: number) => add(n, n),
};

/// Type-only import used in a function signature (no runtime call).
export function speak(animal: Speakable): string {
    return animal.speak();  // calls:virtual, possible (interface type)
}
