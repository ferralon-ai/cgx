//! GM-12 effect-label type: bitset semantics, fixed iteration order, canonical
//! string round-trip, and deterministic encoding.

use cgx_core::codec::{decode, encode};
use cgx_core::effect::{Effect, EffectSet};

#[test]
fn iteration_order_is_fixed_regardless_of_insertion_order() {
    let a = EffectSet::from_iter_canonical([Effect::Nondeterministic, Effect::Blocking]);
    let b = EffectSet::from_iter_canonical([Effect::Blocking, Effect::Nondeterministic]);
    assert_eq!(a, b);
    assert_eq!(
        a.iter().collect::<Vec<_>>(),
        vec![Effect::Blocking, Effect::Nondeterministic],
    );
}

#[test]
fn empty_set_is_empty_and_has_zero_len() {
    let e = EffectSet::new();
    assert!(e.is_empty());
    assert_eq!(e.len(), 0);
    assert_eq!(e.iter().count(), 0);
}

#[test]
fn insert_is_idempotent() {
    let mut e = EffectSet::new();
    e.insert(Effect::IoFile);
    e.insert(Effect::IoFile);
    assert_eq!(e.len(), 1);
    assert!(e.contains(Effect::IoFile));
    assert!(!e.contains(Effect::IoNet));
}

#[test]
fn union_combines_both_sets() {
    let a = EffectSet::single(Effect::IoFile);
    let b = EffectSet::single(Effect::Spawns);
    let u = a.union(b);
    assert!(u.contains(Effect::IoFile));
    assert!(u.contains(Effect::Spawns));
    assert_eq!(u.len(), 2);
}

#[test]
fn canonical_strings_match_gm12_labels() {
    assert_eq!(Effect::Blocking.as_str(), "blocking");
    assert_eq!(Effect::Spawns.as_str(), "spawns");
    assert_eq!(Effect::IoFile.as_str(), "io.file");
    assert_eq!(Effect::IoNet.as_str(), "io.net");
    assert_eq!(Effect::IoProc.as_str(), "io.proc");
    assert_eq!(Effect::DynamicCode.as_str(), "dynamic-code");
    assert_eq!(Effect::Nondeterministic.as_str(), "nondeterministic");
}

#[test]
fn parse_is_inverse_of_as_str() {
    for e in Effect::ALL {
        assert_eq!(Effect::parse(e.as_str()), Some(e));
    }
    assert_eq!(Effect::parse("not-an-effect"), None);
}

#[test]
fn encode_round_trips_losslessly() {
    let e = EffectSet::from_iter_canonical([Effect::IoFile, Effect::Blocking, Effect::Spawns]);
    let bytes = encode(&e).unwrap();
    let back: EffectSet = decode(&bytes).unwrap();
    assert_eq!(e, back);
}

#[test]
fn equal_sets_encode_to_identical_bytes() {
    let a = EffectSet::from_iter_canonical([Effect::IoNet, Effect::IoFile]);
    let b = EffectSet::from_iter_canonical([Effect::IoFile, Effect::IoNet]);
    assert_eq!(encode(&a).unwrap(), encode(&b).unwrap());
}
