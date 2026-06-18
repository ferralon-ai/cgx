// cgx-fixture: TypeScript entrypoints
// Covers: top-level export default (entrypoint), jest test() (entrypoint:test),
//         named exports as entrypoints, module init

import { add } from "./direct";
import { tryParse } from "./errors";
import { makeSpeak } from "./virtual_dispatch";

// cgx:entrypoint kind=module-export
export default function main(args: string[]): void {
    const x = add(1, 2);
    console.log(x);

    const result = tryParse("42");
    if (result !== null) {
        console.log("parsed:", result);
    }

    const sound = makeSpeak({ speak: () => "hello" });
    console.log(sound);
}

// cgx:entrypoint kind=module-export
export function startServer(port: number): void {
    console.log(`server on port ${port}`);
    add(port, 0);
}

// Jest test entrypoints — auto-detected by test() / it() calls
// cgx:entrypoint kind=test
test("add returns sum", () => {
    expect(add(1, 2)).toBe(3);
});

// cgx:entrypoint kind=test
test("tryParse returns number", () => {
    expect(tryParse("5")).toBe(5);
});

// cgx:entrypoint kind=test
it("startServer calls add", () => {
    // just checking it doesn't throw
    startServer(8080);
});
