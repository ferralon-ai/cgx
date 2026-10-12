//! [`ChunkedVec`]: an append-mostly sequence stored in fixed-size chunks.
//!
//! The resolved edge list is the largest structure an index holds. In a single
//! `Vec`, every capacity doubling keeps the old buffer and one twice its size
//! live at once, and a post-pass `reserve` that outgrows the slack copies the
//! whole list again. A chunked list grows by adding a chunk: past the first
//! chunk no element is copied on growth, the slack is at most one chunk, and no
//! single allocation exceeds one chunk (which also keeps large graphs under a
//! 32-bit target's per-allocation limit).
//!
//! Every chunk except the last holds exactly [`CHUNK_LEN`] elements, so indexing
//! is a shift and a mask, and two lists with equal elements have equal layout.

use std::fmt;
use std::ops::{Index, IndexMut};

const CHUNK_SHIFT: u32 = 16;
/// Elements per chunk.
pub const CHUNK_LEN: usize = 1 << CHUNK_SHIFT;
const CHUNK_MASK: usize = CHUNK_LEN - 1;

/// A growable sequence stored as a list of fixed-capacity chunks. See the
/// module docs.
#[derive(Clone, PartialEq, Eq)]
pub struct ChunkedVec<T> {
    // Invariant: every chunk but the last has exactly CHUNK_LEN elements, the
    // last has 1..=CHUNK_LEN, and no chunk's capacity exceeds CHUNK_LEN.
    chunks: Vec<Vec<T>>,
    len: usize,
}

impl<T> Default for ChunkedVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for ChunkedVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T> ChunkedVec<T> {
    /// An empty list. Allocates nothing.
    pub const fn new() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the list has no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append an element. Never moves an element out of a full chunk.
    pub fn push(&mut self, value: T) {
        let last = match self.chunks.last_mut() {
            Some(c) if c.len() < CHUNK_LEN => c,
            prev => {
                // A list that already filled a chunk is large: give the next one
                // its full size at once rather than regrowing it from empty.
                let cap = if prev.is_some() { CHUNK_LEN } else { 0 };
                self.chunks.push(Vec::with_capacity(cap));
                self.chunks.last_mut().expect("chunk just pushed")
            }
        };
        if last.len() == last.capacity() {
            // Grow the first chunk by doubling, capped at CHUNK_LEN, so a small
            // list stays small and a full chunk is exactly CHUNK_LEN.
            let target = (last.capacity() * 2).clamp(4, CHUNK_LEN);
            last.reserve_exact(target - last.len());
        }
        last.push(value);
        self.len += 1;
    }

    /// The element at `i`, if any.
    pub fn get(&self, i: usize) -> Option<&T> {
        self.chunks
            .get(i >> CHUNK_SHIFT)
            .and_then(|c| c.get(i & CHUNK_MASK))
    }

    /// The element at `i` mutably, if any.
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.chunks
            .get_mut(i >> CHUNK_SHIFT)
            .and_then(|c| c.get_mut(i & CHUNK_MASK))
    }

    /// The last element, if any.
    pub fn last(&self) -> Option<&T> {
        self.chunks.last().and_then(|c| c.last())
    }

    /// The last element mutably, if any.
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.chunks.last_mut().and_then(|c| c.last_mut())
    }

    /// Iterate the elements in order.
    pub fn iter(&self) -> Iter<'_, T> {
        self.chunks.iter().flatten()
    }

    /// Iterate the elements mutably in order.
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        self.chunks.iter_mut().flatten()
    }

    /// Swap the elements at `a` and `b`. Panics if either is out of bounds.
    pub fn swap(&mut self, a: usize, b: usize) {
        let (ca, ia) = (a >> CHUNK_SHIFT, a & CHUNK_MASK);
        let (cb, ib) = (b >> CHUNK_SHIFT, b & CHUNK_MASK);
        if ca == cb {
            self.chunks[ca].swap(ia, ib);
        } else {
            let (lo, hi, ilo, ihi) = if ca < cb {
                (ca, cb, ia, ib)
            } else {
                (cb, ca, ib, ia)
            };
            let (head, tail) = self.chunks.split_at_mut(hi);
            std::mem::swap(&mut head[lo][ilo], &mut tail[0][ihi]);
        }
    }

    /// Shorten the list to `len` elements, freeing every chunk left empty.
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        let keep_chunks = len.div_ceil(CHUNK_LEN);
        self.chunks.truncate(keep_chunks);
        if let Some(last) = self.chunks.last_mut() {
            last.truncate(len - (keep_chunks - 1) * CHUNK_LEN);
        }
        self.len = len;
    }

    /// Keep only the elements for which `keep` returns true, preserving their
    /// order. `keep` sees every element once, in order. In place: survivors are
    /// swapped forward and the tail is truncated, so no second buffer is held.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        let mut write = 0usize;
        for read in 0..self.len {
            if keep(&self[read]) {
                if read != write {
                    self.swap(read, write);
                }
                write += 1;
            }
        }
        self.truncate(write);
    }

    /// Sort stably by a key computed once per element, exactly like
    /// [`slice::sort_by_cached_key`]: the result is the order of the
    /// `(key, original index)` pairs, applied by in-place swaps.
    pub fn sort_by_cached_key<K: Ord>(&mut self, f: impl FnMut(&T) -> K) {
        if self.len < 2 {
            return;
        }
        if u32::try_from(self.len).is_ok() {
            self.permute_by_key(f, |i| i as u32, |i| i as usize);
        } else {
            self.permute_by_key(f, |i| i, |i| i);
        }
    }

    fn permute_by_key<K: Ord, I: Copy + Ord>(
        &mut self,
        f: impl FnMut(&T) -> K,
        to: impl Fn(usize) -> I,
        from: impl Fn(I) -> usize,
    ) {
        let mut indices: Vec<(K, I)> = Vec::with_capacity(self.len);
        indices.extend(self.iter().map(f).enumerate().map(|(i, k)| (k, to(i))));
        // (key, index) pairs are unique, so an unstable sort is deterministic and
        // equals a stable sort by key.
        indices.sort_unstable();
        for i in 0..self.len {
            let mut index = indices[i].1;
            while from(index) < i {
                index = indices[from(index)].1;
            }
            indices[i].1 = index;
            self.swap(i, from(index));
        }
    }
}

impl<T> Index<usize> for ChunkedVec<T> {
    type Output = T;

    fn index(&self, i: usize) -> &T {
        &self.chunks[i >> CHUNK_SHIFT][i & CHUNK_MASK]
    }
}

impl<T> IndexMut<usize> for ChunkedVec<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.chunks[i >> CHUNK_SHIFT][i & CHUNK_MASK]
    }
}

impl<T> Extend<T> for ChunkedVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for v in iter {
            self.push(v);
        }
    }
}

impl<T> FromIterator<T> for ChunkedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut v = Self::new();
        v.extend(iter);
        v
    }
}

impl<T> From<Vec<T>> for ChunkedVec<T> {
    fn from(v: Vec<T>) -> Self {
        v.into_iter().collect()
    }
}

/// Borrowing iterator over a [`ChunkedVec`].
pub type Iter<'a, T> = std::iter::Flatten<std::slice::Iter<'a, Vec<T>>>;
/// Mutably borrowing iterator over a [`ChunkedVec`].
pub type IterMut<'a, T> = std::iter::Flatten<std::slice::IterMut<'a, Vec<T>>>;

impl<'a, T> IntoIterator for &'a ChunkedVec<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut ChunkedVec<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

/// Owning iterator over a [`ChunkedVec`]. Frees each chunk as soon as it has
/// been drained, and reports an exact length so a `collect` allocates once.
#[derive(Debug)]
pub struct IntoIter<T> {
    chunks: std::vec::IntoIter<Vec<T>>,
    current: std::vec::IntoIter<T>,
    remaining: usize,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        loop {
            if let Some(v) = self.current.next() {
                self.remaining -= 1;
                return Some(v);
            }
            self.current = self.chunks.next()?.into_iter();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}

impl<T> IntoIterator for ChunkedVec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter {
            chunks: self.chunks.into_iter(),
            current: Vec::new().into_iter(),
            remaining: self.len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(n: usize) -> ChunkedVec<usize> {
        (0..n).collect()
    }

    #[test]
    fn push_index_and_layout_cross_chunk_boundaries() {
        for n in [
            0,
            1,
            CHUNK_LEN - 1,
            CHUNK_LEN,
            CHUNK_LEN + 1,
            3 * CHUNK_LEN + 7,
        ] {
            let v = filled(n);
            assert_eq!(v.len(), n);
            assert!(v.iter().copied().eq(0..n));
            for i in [0, n / 2, n.saturating_sub(1)] {
                if i < n {
                    assert_eq!(v[i], i);
                }
            }
            assert_eq!(v.get(n), None);
            assert_eq!(v.chunks.len(), n.div_ceil(CHUNK_LEN));
            assert!(v.chunks.iter().all(|c| c.capacity() <= CHUNK_LEN));
            assert!(v.chunks.iter().skip(1).all(|c| c.capacity() == CHUNK_LEN));
            assert!(v.chunks.iter().rev().skip(1).all(|c| c.len() == CHUNK_LEN));
        }
    }

    #[test]
    fn retain_matches_vec_and_frees_tail_chunks() {
        let n = 2 * CHUNK_LEN + 11;
        let keep = |x: &usize| !x.is_multiple_of(3) && *x < CHUNK_LEN + 5;
        let mut v = filled(n);
        v.retain(keep);
        let mut expect: Vec<usize> = (0..n).collect();
        expect.retain(keep);
        assert!(v.iter().eq(expect.iter()));
        assert_eq!(v.len(), expect.len());
        assert_eq!(v.chunks.len(), expect.len().div_ceil(CHUNK_LEN));
    }

    #[test]
    fn truncate_to_chunk_boundary_drops_empty_chunk() {
        let mut v = filled(CHUNK_LEN + 3);
        v.truncate(CHUNK_LEN);
        assert_eq!(v.chunks.len(), 1);
        v.truncate(0);
        assert!(v.chunks.is_empty() && v.is_empty());
        v.push(9);
        assert_eq!(v[0], 9);
    }

    #[test]
    fn sort_by_cached_key_matches_slice_stable_order() {
        // Keys with many ties, across chunk boundaries: the payload records the
        // original index so stability is observable.
        let n = 2 * CHUNK_LEN + 101;
        let key = |x: &(u32, usize)| x.0;
        let mut expect: Vec<(u32, usize)> = (0..n).map(|i| (((i * 7919) % 13) as u32, i)).collect();
        let mut v: ChunkedVec<(u32, usize)> = expect.iter().copied().collect();
        expect.sort_by_cached_key(key);
        v.sort_by_cached_key(key);
        assert!(v.iter().eq(expect.iter()));
    }

    #[test]
    fn owned_iteration_is_exact_and_ordered() {
        let v = filled(CHUNK_LEN + 9);
        let it = v.into_iter();
        assert_eq!(it.len(), CHUNK_LEN + 9);
        let out: Vec<usize> = it.collect();
        assert_eq!(out.capacity(), CHUNK_LEN + 9);
        assert!(out.into_iter().eq(0..CHUNK_LEN + 9));
    }

    #[test]
    fn equality_and_swap() {
        let mut a = filled(CHUNK_LEN + 2);
        let b = filled(CHUNK_LEN + 2);
        assert_eq!(a, b);
        a.swap(1, CHUNK_LEN + 1);
        assert_eq!((a[1], a[CHUNK_LEN + 1]), (CHUNK_LEN + 1, 1));
        assert_ne!(a, b);
    }
}
