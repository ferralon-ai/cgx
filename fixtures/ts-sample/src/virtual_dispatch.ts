// cgx-fixture: virtual/duck-typed dispatch (TypeScript)
// Covers: duck-typed method calls -> calls:virtual (possible confidence),
//         interface-typed calls -> calls:virtual,
//         extends/implements override chains

export interface Speakable {
    speak(): string;
    introduce?(): string;
}

export class Dog implements Speakable {
    speak(): string {
        return "woof";
    }

    introduce(): string {
        return `Dog says: ${this.speak()}`;  // certain: typed receiver on class method
    }
}

export class Cat implements Speakable {
    speak(): string {
        return "meow";
    }
    // introduce() not overridden — missing from Cat
}

/// Duck-typed parameter — calls:virtual on animal.speak(), possible confidence.
/// Candidate set: all types with a speak() method visible in scope.
export function makeSpeak(animal: { speak(): string }): string {
    return animal.speak();  // calls:virtual, possible (duck type, no type narrowing)
}

/// Interface-typed parameter — calls:virtual, possible confidence.
export function introduceAnimal(animal: Speakable): string {
    return animal.introduce ? animal.introduce() : animal.speak();  // calls:virtual, conditional
}

/// Typed array iteration — calls:virtual on each element.
export function chorus(animals: Speakable[]): string[] {
    return animals.map(a => a.speak());  // calls:virtual, loop (inside map)
}

export abstract class Shape {
    abstract area(): number;

    describe(): string {
        return `shape with area ${this.area()}`;  // calls:virtual: abstract method
    }
}

export class Circle extends Shape {
    constructor(private radius: number) {
        super();
    }

    area(): number {
        return Math.PI * this.radius * this.radius;  // calls Math.PI (reference, not call)
    }
}

export class Rectangle extends Shape {
    constructor(private w: number, private h: number) {
        super();
    }

    area(): number {
        return this.w * this.h;
    }
}

export function totalArea(shapes: Shape[]): number {
    return shapes.reduce((sum, s) => sum + s.area(), 0);  // calls:virtual inside reduce callback
}
