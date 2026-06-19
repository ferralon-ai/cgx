//! Tiny SCIP fixture crate (decision R7). Exercises every confidence path the
//! re-label pass cares about:
//!  - a free function (free, unique → certain-eligible),
//!  - an inherent method (free, unique → certain-eligible),
//!  - a trait + impl + a `dyn Trait` call (trait member → probable ceiling),
//!  - a macro.
//!
//! Regenerate the committed `index.scip` with the command in REGEN.md whenever
//! this file changes.

pub fn parse(input: &str) -> usize {
    input.len()
}

pub struct Reader {
    pub pos: usize,
}

impl Reader {
    pub fn new() -> Self {
        Reader { pos: 0 }
    }

    pub fn read(&mut self) -> usize {
        let p = self.pos;
        self.pos += 1;
        p
    }
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Shape {
    fn area(&self) -> f64;
}

pub struct Circle {
    pub r: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.r * self.r
    }
}

pub fn total_area(shapes: &[Box<dyn Shape>]) -> f64 {
    shapes.iter().map(|s| s.area()).sum()
}

pub fn driver() -> usize {
    let n = parse("hello");
    let mut r = Reader::new();
    let _ = r.read();
    let shapes: Vec<Box<dyn Shape>> = vec![Box::new(Circle { r: 1.0 })];
    let _ = total_area(&shapes);
    n
}
