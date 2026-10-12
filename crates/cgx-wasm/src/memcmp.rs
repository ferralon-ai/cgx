//! A word-at-a-time `memcmp` for the wasm build.
//!
//! wasi-libc's `memcmp` compares one byte per iteration, and every `str`/`[u8]`
//! ordering and equality in the engine goes through it (string-keyed
//! `BTreeMap`s, sorts, dedups). The wasm build overrides the symbol at link
//! time with [`memcmp`], which compares eight bytes per iteration; on a large
//! repository that took about 11% off the instructions of a whole index.
//! Callers use only the sign of the result, and [`compare`] returns the same
//! sign as a byte-wise comparison (tested below on every length 0–64 with a
//! difference at every position; the host's test suite also calls the
//! exported symbol inside the built module).

/// Compare `a` and `b` (equal lengths) as `memcmp` does: the sign of the
/// first differing byte's difference, or 0.
pub fn compare(a: &[u8], b: &[u8]) -> i32 {
    debug_assert_eq!(a.len(), b.len());
    let words = a.chunks_exact(8).zip(b.chunks_exact(8));
    let mut done = 0;
    for (x, y) in words {
        let x = u64::from_le_bytes(x.try_into().expect("chunk of 8"));
        let y = u64::from_le_bytes(y.try_into().expect("chunk of 8"));
        if x != y {
            // Little-endian load: the first differing byte is the lowest
            // differing one.
            let shift = (x ^ y).trailing_zeros() & !7;
            return i32::from((x >> shift) as u8) - i32::from((y >> shift) as u8);
        }
        done += 8;
    }
    for (x, y) in a[done..].iter().zip(&b[done..]) {
        if x != y {
            return i32::from(*x) - i32::from(*y);
        }
    }
    0
}

/// The C `memcmp`, overriding wasi-libc's in the wasm build.
///
/// # Safety
///
/// As for C `memcmp`: `a` and `b` are readable for `n` bytes.
#[cfg(target_family = "wasm")]
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    if n == 0 {
        // C allows null (or dangling) pointers with n == 0; slices do not.
        return 0;
    }
    // SAFETY: the caller guarantees both are readable for `n` bytes, and
    // neither is null since n > 0.
    let (a, b) = unsafe { (std::slice::from_raw_parts(a, n), std::slice::from_raw_parts(b, n)) };
    compare(a, b)
}

#[cfg(test)]
mod tests {
    use super::compare;

    /// xorshift64*: deterministic, dependency-free.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn byte(&mut self) -> u8 {
            (self.next() >> 56) as u8
        }
    }

    fn reference(a: &[u8], b: &[u8]) -> i32 {
        a.cmp(b) as i32
    }

    #[test]
    fn sign_matches_bytewise_for_every_length_and_position() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for len in 0..=64usize {
            for round in 0..32 {
                // Offsets into a larger buffer exercise unaligned starts.
                let off = round % 8;
                let a: Vec<u8> = (0..len + off).map(|_| rng.byte()).collect();
                let a = &a[off..];
                assert_eq!(compare(a, a), 0, "len {len}");
                for pos in 0..len {
                    let mut b = a.to_vec();
                    // Any different byte, high bit set or not; then later
                    // bytes randomised so they cannot decide the result.
                    b[pos] = a[pos].wrapping_add(1 + rng.byte() % 255);
                    for byte in &mut b[pos + 1..] {
                        *byte = rng.byte();
                    }
                    let want = reference(a, &b);
                    let got = compare(a, &b);
                    assert_eq!(got.signum(), want, "len {len} pos {pos}: {a:?} vs {b:?}");
                    assert_eq!(compare(&b, a).signum(), -want, "len {len} pos {pos} swapped");
                }
            }
        }
    }

    #[test]
    fn result_is_the_first_differing_bytes_difference() {
        // What C memcmp implementations (musl's included) return; some C
        // callers print it, none depend on it, but keep it the same anyway.
        let a = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 0xff];
        let mut b = a;
        b[9] = 0x01;
        assert_eq!(compare(&a, &b), 0xff - 0x01);
        b = a;
        b[3] = 0x80;
        assert_eq!(compare(&a, &b), 3 - 0x80);
    }
}
