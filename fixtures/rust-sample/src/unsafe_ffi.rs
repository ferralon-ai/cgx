// cgx-fixture: FFI and unsafe
// Covers: extern "C" calls -> via-FFI cut marker,
//         unsafe blocks -> unsafe_region=true on enclosing symbol

extern "C" {
    // Declared external C functions — calls to these produce via-FFI cut markers.
    fn strlen(s: *const std::os::raw::c_char) -> usize;
    fn abs(x: std::os::raw::c_int) -> std::os::raw::c_int;
}

/// Calls an extern "C" function — via-FFI cut marker, confidence: possible.
///
/// # Safety
/// `ptr` must be a valid null-terminated C string.
pub unsafe fn get_c_strlen(ptr: *const std::os::raw::c_char) -> usize {
    strlen(ptr)   // via-FFI cut marker
}

/// Wraps the unsafe FFI call in a safe interface.
pub fn safe_abs(x: i32) -> i32 {
    unsafe { abs(x) }  // via-FFI cut marker; unsafe_region=true on safe_abs
}

/// Pure Rust unsafe block (no FFI — raw pointer arithmetic).
/// unsafe_region=true on this function, no FFI cut marker.
pub unsafe fn offset_ptr(base: *const i32, offset: isize) -> i32 {
    *base.offset(offset)  // unsafe, no FFI
}

/// struct with unsafe impl — the impl block is flagged, not FFI.
pub struct RawBuffer {
    ptr: *mut u8,
    len: usize,
}

impl RawBuffer {
    /// # Safety
    /// `ptr` must point to `len` bytes of valid memory.
    pub unsafe fn new(ptr: *mut u8, len: usize) -> Self {
        RawBuffer { ptr, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// # Safety
    /// `idx` must be < self.len.
    pub unsafe fn read_byte(&self, idx: usize) -> u8 {
        *self.ptr.add(idx)  // unsafe raw ptr dereference, no FFI
    }
}
