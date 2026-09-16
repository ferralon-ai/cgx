// cgx-fixture: interface dispatch (README example)
// Covers: interface-typed parameter -> calls:virtual (possible confidence),
//         self-contained — no imports of other fixture files.
// `vocalize` is a globally unique method name (cgx resolves TS method calls
// by name across the whole index, so a common name like `speak` pulls in
// unrelated fixtures' same-named methods).

interface Vocalizer {
  vocalize(): string;
}

class Dog implements Vocalizer {
  vocalize(): string {
    return "woof";
  }
}

class Cat implements Vocalizer {
  vocalize(): string {
    return "meow";
  }
}

/// Interface-typed dispatcher — calls:virtual on v.vocalize(), possible confidence.
/// Candidate set: [Dog.vocalize, Cat.vocalize]
export function dispatchVocal(v: Vocalizer): string {
  return v.vocalize();
}
