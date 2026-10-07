//! Determinism: canonical facts are a pure function of the source bytes, so two
//! extractions of the same file are byte-identical (the blob-OID fragment-cache
//! soundness property, architecture §3).

mod common;

use cgx_core::codec;
use common::extract;

const SRC: &str = r#"
#include <memory>
#include "shapes.hpp"
#define MAX 32
#define LOG(x) real_log(x)
namespace geo {
using std::unique_ptr;
class Shape {
public:
  virtual double area() const = 0;
  virtual ~Shape();
};
class Circle : public Shape {
public:
  double area() const override;
  Circle(double r);
private:
  double r_;
};
double Circle::area() const {
  if (r_ > 0) { return 3.14 * r_ * r_; }
  return 0;
}
int f(int x) { return x; }
int f(double x) { return 0; }
template <typename T> T max_of(T a, T b) { return a > b ? a : b; }
void run(Shape* s) {
  try {
    s->area();
  } catch (const std::exception& e) {
    LOG("err");
  }
  for (int i = 0; i < MAX; ++i) { f(i); }
}
}
"#;

#[test]
fn two_extractions_are_structurally_equal() {
    assert_eq!(extract("src/m.cpp", SRC), extract("src/m.cpp", SRC));
}

#[test]
fn two_extractions_encode_byte_identically() {
    let a = extract("src/m.cpp", SRC);
    let b = extract("src/m.cpp", SRC);
    let ba = codec::encode(&a).expect("encode a");
    let bb = codec::encode(&b).expect("encode b");
    assert_eq!(ba, bb, "canonical postcard bytes must be identical");
}

#[test]
fn canonicalize_is_idempotent() {
    let mut a = extract("src/m.cpp", SRC);
    let before = codec::encode(&a).unwrap();
    a.canonicalize();
    let after = codec::encode(&a).unwrap();
    assert_eq!(before, after);
}

#[test]
fn empty_source_is_well_formed_not_an_error() {
    let f = extract("src/empty.cpp", "");
    assert!(f.defs.is_empty() && f.refs.is_empty());
    assert_eq!(f.scopes.scopes.len(), 1, "just the root scope");
}

#[test]
fn comment_only_source_has_no_defs() {
    let f = extract("src/c.cpp", "/* just a comment */\n// another\n");
    assert!(f.is_degraded_empty());
}
