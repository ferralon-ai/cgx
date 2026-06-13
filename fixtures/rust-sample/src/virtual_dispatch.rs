// cgx-fixture: virtual/trait dispatch
// Covers: dyn Trait -> calls:virtual (probable), impl Trait for T,
//         super calls via default trait methods

pub trait Speak {
    fn speak(&self) -> String;

    // Default method — override available in impls
    fn introduce(&self) -> String {
        format!("I say: {}", self.speak())
    }
}

pub struct Dog;
pub struct Cat;

impl Speak for Dog {
    fn speak(&self) -> String {
        "woof".to_string()
    }
}

impl Speak for Cat {
    fn speak(&self) -> String {
        "meow".to_string()
    }

    // Overrides default — this is a super-call chain scenario
    fn introduce(&self) -> String {
        format!("Cat says: {}", self.speak())
    }
}

/// dyn Trait dispatch — calls:virtual on `animal.speak()`, probable confidence.
/// Candidate set: [Dog::speak, Cat::speak]
pub fn make_speak(animal: &dyn Speak) -> String {
    animal.speak()
}

/// dyn Trait dispatch via box — same semantics.
pub fn make_speak_boxed(animal: Box<dyn Speak>) -> String {
    animal.introduce()
}

/// Generic static dispatch — monomorphized, calls are certain.
pub fn make_speak_static<T: Speak>(animal: &T) -> String {
    animal.speak()
}

pub trait Transform {
    fn transform(&self, input: i32) -> i32;
}

pub struct Doubler;
pub struct Adder(i32);

impl Transform for Doubler {
    fn transform(&self, input: i32) -> i32 {
        input * 2
    }
}

impl Transform for Adder {
    fn transform(&self, input: i32) -> i32 {
        input + self.0
    }
}

/// Virtual dispatch through a trait object stored in a Vec.
pub fn apply_all(transforms: &[&dyn Transform], value: i32) -> i32 {
    let mut result = value;
    for t in transforms {
        result = t.transform(result);
    }
    result
}
