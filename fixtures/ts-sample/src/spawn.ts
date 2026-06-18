// cgx-fixture: spawn and Promise patterns (TypeScript)
// Covers: Promise constructor -> spawns edge (detached execution),
//         setTimeout/setInterval -> spawns edge,
//         Promise.race/all -> calls:async,
//         detached error domain (GM-9)

async function backgroundWork(id: number): Promise<void> {
    await processTask(id);
}

async function processTask(id: number): Promise<void> {
    logTask(id);
}

function logTask(id: number): void {
    console.log(`task ${id}`);
}

function handleError(e: Error): void {
    console.error("task failed:", e.message);
}

/// Unconditional spawn via Promise + no await — spawns edge, always.
export function spawnOne(): void {
    void backgroundWork(1);   // spawns edge, always (fire-and-forget)
}

/// Conditional spawn — spawns edge, conditional.
export function spawnIf(condition: boolean): void {
    if (condition) {
        void backgroundWork(2);  // spawns edge, conditional
    }
}

/// Loop spawn — spawns edge, loop.
export function spawnMany(count: number): void {
    for (let i = 0; i < count; i++) {
        void backgroundWork(i);  // spawns edge, loop
    }
}

/// Spawn via setTimeout — spawns edge (detached timer callback).
export function spawnDelayed(ms: number): void {
    setTimeout(() => {
        void processTask(99);  // spawns edge, always (inside timer callback)
    }, ms);
}

/// setInterval — spawns edge, loop (repeated firing).
export function spawnInterval(ms: number): ReturnType<typeof setInterval> {
    return setInterval(() => {
        logTask(0);  // spawns edge via interval
    }, ms);
}

/// Spawn in error handling context — spawns edge, exception.
export function spawnOnError(result: Result<number, Error>): void {
    if (!result.ok) {
        void backgroundWork(0);  // spawns edge, conditional (error branch)
    }
}

type Result<T, E> = { ok: true; value: T } | { ok: false; error: E };

/// Awaited Promise — calls:async, NOT spawns.
export async function spawnAndJoin(): Promise<void> {
    await backgroundWork(99);   // calls:async (awaited — retains result)
}

/// Promise.all — awaited results from multiple concurrent tasks.
export async function allParallel(ids: number[]): Promise<void[]> {
    return Promise.all(ids.map(id => backgroundWork(id)));  // calls:async on the await
}
