// cgx-fixture: inheritance and override chains (TypeScript)
// Covers: extends (inherits edge), implements (implements edge),
//         method overrides (overrides edge), super calls, abstract methods

export interface Animal {
    name(): string;
    sound(): string;
    description?(): string;
}

export abstract class BaseAnimal implements Animal {
    private _name: string;

    constructor(name: string) {
        this._name = name;
    }

    name(): string {
        return this._name;  // concrete method, no override
    }

    abstract sound(): string;  // must be overridden

    // Default description — calls this.name() and this.sound() (virtual)
    description(): string {
        return `${this.name()} says ${this.sound()}`;  // calls:virtual on sound()
    }
}

export class Labrador extends BaseAnimal {
    constructor(name: string) {
        super(name);   // super call — calls BaseAnimal constructor
    }

    sound(): string {
        return "woof";  // overrides abstract method
    }
    // description() inherited — uses BaseAnimal::description
}

export class Poodle extends BaseAnimal {
    constructor(name: string) {
        super(name);
    }

    sound(): string {
        return "yip";
    }

    // Override: custom description, calls super.description()
    description(): string {
        const base = super.description();   // super call: certain -> BaseAnimal::description
        return `${base} (fancy)`;
    }
}

export interface Trainable {
    train(command: string): boolean;
    respond(): string;
}

export class TrainedLabrador extends Labrador implements Trainable {
    constructor(name: string) {
        super(name);
    }

    train(command: string): boolean {
        return command.length > 0;
    }

    respond(): string {
        return `${this.name()} sits!`;  // certain: this.name() on concrete class
    }
}

/// Virtual dispatch to overridden method.
export function describeAnimal(a: Animal): string {
    return a.description ? a.description() : a.sound();  // calls:virtual, conditional
}

/// Super chain: Poodle.description -> BaseAnimal.description.
export function poodleDescription(): string {
    const p = new Poodle("Fifi");
    return p.description();  // calls Poodle::description which super-calls BaseAnimal::description
}

/// Array of base type — virtual dispatch on each element.
export function allDescriptions(animals: BaseAnimal[]): string[] {
    return animals.map(a => a.description());  // calls:virtual inside map callback
}
