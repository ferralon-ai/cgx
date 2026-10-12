//! Determinism: canonical facts are a pure function of the source bytes, so two
//! extractions of the same file are byte-identical (the blob-OID fragment-cache
//! soundness property, architecture §3).

mod common;

use cgx_core::codec;
use common::extract;

const SRC: &str = r#"
#include <stdio.h>
#include "util.h"
#define MAX 32
#define LOG(x) real_log(x)
typedef int (*BinOp)(int, int);
enum Color { RED, GREEN, BLUE };
struct Point { int x; int y; };
static int g_count = 0;
static int helper(int x) { return x + 1; }
int apply(int (*op)(int, int), int a) { return op(a, 1); }
int main(void) {
    int y = helper(41);
    if (y > 0) { printf("%d", y); }
    for (int i = 0; i < MAX; i++) { helper(i); }
    LOG("hi");
    fopen("f", "r");
    return y;
}
"#;

#[test]
fn two_extractions_are_structurally_equal() {
    let a = extract("src/m.c", SRC);
    let b = extract("src/m.c", SRC);
    assert_eq!(a, b);
}

#[test]
fn two_extractions_encode_byte_identically() {
    let a = extract("src/m.c", SRC);
    let b = extract("src/m.c", SRC);
    let ba = codec::encode(&a).expect("encode a");
    let bb = codec::encode(&b).expect("encode b");
    assert_eq!(ba, bb, "canonical postcard bytes must be identical");
}

#[test]
fn canonicalize_is_idempotent() {
    let mut a = extract("src/m.c", SRC);
    let before = codec::encode(&a).unwrap();
    a.canonicalize();
    let after = codec::encode(&a).unwrap();
    assert_eq!(before, after);
}

#[test]
fn empty_source_is_well_formed_not_an_error() {
    let f = extract("src/empty.c", "");
    assert!(f.defs.is_empty() && f.refs.is_empty());
    assert_eq!(f.scopes.scopes.len(), 1, "just the root scope");
}

#[test]
fn comment_only_source_has_no_defs() {
    let f = extract("src/c.c", "/* just a comment */\n// another\n");
    assert!(f.is_degraded_empty());
}
