// cgx-fixture: inheritance and override chains
// Covers: impl Trait for T (implements edges), overrides of default methods,
//         super-trait calls, struct embedding pattern

pub trait Animal {
    fn name(&self) -> &str;

    // Default implementation — can be overridden
    fn description(&self) -> String {
        format!("{} is an animal", self.name())
    }

    fn sound(&self) -> String;
}

pub trait Trainable: Animal {
    fn train(&self, command: &str) -> bool;

    // Default: calls sound() from Animal
    fn respond(&self) -> String {
        format!("{} responds: {}", self.name(), self.sound())
    }
}

pub struct Labrador {
    pub name: String,
}

pub struct Poodle {
    pub name: String,
}

impl Animal for Labrador {
    fn name(&self) -> &str {
        &self.name
    }

    fn sound(&self) -> String {
        "woof".to_string()
    }
    // description() uses default implementation — calls self.name() via default
}

impl Animal for Poodle {
    fn name(&self) -> &str {
        &self.name
    }

    fn sound(&self) -> String {
        "yip".to_string()
    }

    // Override: does not call the default
    fn description(&self) -> String {
        format!("{} is a fancy poodle", self.name())
    }
}

impl Trainable for Labrador {
    fn train(&self, command: &str) -> bool {
        !command.is_empty()
    }
    // respond() uses default — calls self.name() and self.sound() via default
}

impl Trainable for Poodle {
    fn train(&self, command: &str) -> bool {
        command.len() < 10
    }

    // Overrides respond — custom implementation
    fn respond(&self) -> String {
        format!("{} does a trick!", self.name())
    }
}

/// Demonstrate virtual dispatch to overridden methods.
pub fn describe_animal(a: &dyn Animal) -> String {
    a.description()   // calls:virtual -> [Labrador::description(default), Poodle::description(override)]
}

pub fn get_response(t: &dyn Trainable) -> String {
    t.respond()       // calls:virtual -> [Labrador::respond(default), Poodle::respond(override)]
}

/// Check override chain: default calls through to overridden method.
pub fn default_description_chain() -> String {
    let lab = Labrador { name: "Rex".to_string() };
    lab.description()  // calls default, which calls self.name() (certain, Labrador::name)
}
