//! Byte-identical extraction across runs — the canonical-bytes property (arch §3)
//! that makes blob-OID fragment caching sound.

mod common;

use common::extract;

const SRC: &str = "\
import * as fs from 'fs';
export function pipeline(a: number): number {
  const b = a;
  const c = b + 1;
  fs.readFileSync('/x');
  setTimeout(() => {}, 0);
  return c;
}
class S {
  m(x: number) { const y = x; return y; }
}
";

#[test]
fn extraction_is_byte_identical_across_runs() {
    let a = extract("src/pipeline.ts", SRC);
    let b = extract("src/pipeline.ts", SRC);
    assert_eq!(a, b, "canonical facts must be identical across extractions");
}

#[test]
fn effects_and_dataflow_present() {
    use cgx_core::effect::Effect;
    let facts = extract("src/pipeline.ts", SRC);
    let pipeline = facts
        .effects
        .iter()
        .find(|e| e.fqn.ends_with("::pipeline"))
        .expect("pipeline effects");
    assert!(pipeline.effects.contains(Effect::IoFile));
    assert!(pipeline.effects.contains(Effect::Spawns));
    assert!(
        !facts.data_flows.is_empty(),
        "pipeline must emit dataflow facts"
    );
}
