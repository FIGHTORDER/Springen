//! Deterministic integer primitives, bit-identical to the JavaScript prototype.
//!
//! Every operation here mirrors a JS expression whose semantics are fixed by
//! ECMAScript's ToInt32/ToUint32 coercions. `Math.imul` is a wrapping 32-bit
//! multiply, and `>>>` is a logical shift on ToUint32 of its operand, so the
//! whole family maps onto Rust's `u32` wrapping ops with no loss.

/// ECMAScript `ToInt32`, as applied by every JS bitwise operator.
///
/// Non-finite inputs become 0; everything else truncates toward zero and wraps
/// modulo 2^32. Seeds arrive here as plain `f64` because they do in the
/// prototype, where `ctx.seed + p.seed * 7919` is ordinary float arithmetic.
pub fn to_i32(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let t = v.trunc();
    let m = t.rem_euclid(4294967296.0);
    (m as u32) as i32
}

/// `Math.imul(a, b)` — 32-bit wrapping multiply.
#[inline]
pub fn imul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// The prototype's `mulberry32`. Returns a stateful generator yielding
/// `[0, 1)` doubles.
#[derive(Clone, Debug)]
pub struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    pub fn new(seed: u32) -> Self {
        Mulberry32 { state: seed }
    }

    /// `mulberry32(seed)` from a JS number, which coerces with `>>> 0`.
    pub fn from_f64(seed: f64) -> Self {
        Mulberry32::new(to_i32(seed) as u32)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4294967296.0
    }
}

/// Spatial hash. The three products are summed as JS numbers and then folded
/// through `ToInt32`, which is exactly wrapping `u32` addition.
#[inline]
pub fn hash2i(x: i32, y: i32, seed: i32) -> u32 {
    let h = (imul(x, 374_761_393) as u32)
        .wrapping_add(imul(y, 668_265_263) as u32)
        .wrapping_add(imul(seed, 1_274_126_177) as u32);
    let h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_i32_wraps_like_ecmascript() {
        assert_eq!(to_i32(1.9), 1);
        assert_eq!(to_i32(-1.9), -1);
        assert_eq!(to_i32(4294967296.0), 0);
        assert_eq!(to_i32(4294967297.0), 1);
        assert_eq!(to_i32(2147483648.0), -2147483648);
        assert_eq!(to_i32(f64::NAN), 0);
    }

    #[test]
    fn mulberry32_is_in_unit_interval() {
        let mut r = Mulberry32::new(20250815);
        for _ in 0..64 {
            let v = r.next();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
