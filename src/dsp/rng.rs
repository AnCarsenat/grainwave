/// Small allocation-free xorshift32 RNG for audio-thread randomness.
#[derive(Debug, Clone, Copy)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in [-1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// True with probability `p`.
    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let v = r.next_f32();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
