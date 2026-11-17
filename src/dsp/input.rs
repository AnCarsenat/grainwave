//! OWNER: feat/grain-engine. Channel mode encode/decode around the wet chain.
//!
//! - Stereo: no-op both ways.
//! - Mono: encode `l = r = (l + r) / 2`; decode no-op.
//! - Mid/side: encode `l = M = (l + r) / 2`, `r = S = (l - r) / 2`; decode `L = M + S`, `R = M - S`.
use crate::params::{ChannelMode, Snapshot};

/// Called before the wet chain.
pub fn encode(l: &mut [f32], r: &mut [f32], snap: &Snapshot) {
    encode_mode(l, r, snap.channel_mode);
}

/// Called after the wet chain.
pub fn decode(l: &mut [f32], r: &mut [f32], snap: &Snapshot) {
    decode_mode(l, r, snap.channel_mode);
}

fn encode_mode(l: &mut [f32], r: &mut [f32], mode: ChannelMode) {
    match mode {
        ChannelMode::Stereo => {}
        ChannelMode::Mono => {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                let m = (*a + *b) * 0.5;
                *a = m;
                *b = m;
            }
        }
        ChannelMode::MidSide => {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                let m = (*a + *b) * 0.5;
                let s = (*a - *b) * 0.5;
                *a = m;
                *b = s;
            }
        }
    }
}

fn decode_mode(l: &mut [f32], r: &mut [f32], mode: ChannelMode) {
    match mode {
        ChannelMode::Stereo | ChannelMode::Mono => {}
        ChannelMode::MidSide => {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                let m = *a;
                let s = *b;
                *a = m + s;
                *b = m - s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::rng::Rng;

    fn signal(n: usize, seed: u32) -> (Vec<f32>, Vec<f32>) {
        let mut rng = Rng::new(seed);
        let l = (0..n).map(|_| rng.bipolar()).collect();
        let r = (0..n).map(|_| rng.bipolar()).collect();
        (l, r)
    }

    #[test]
    fn mid_side_round_trip() {
        let (l0, r0) = signal(512, 7);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        encode_mode(&mut l, &mut r, ChannelMode::MidSide);
        // Encoded values really are mid and side.
        assert!((l[3] - (l0[3] + r0[3]) * 0.5).abs() < 1e-7);
        assert!((r[3] - (l0[3] - r0[3]) * 0.5).abs() < 1e-7);
        decode_mode(&mut l, &mut r, ChannelMode::MidSide);
        for i in 0..l0.len() {
            assert!((l[i] - l0[i]).abs() < 1e-6, "L mismatch at {i}");
            assert!((r[i] - r0[i]).abs() < 1e-6, "R mismatch at {i}");
        }
    }

    #[test]
    fn stereo_is_transparent() {
        let (l0, r0) = signal(64, 3);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        encode_mode(&mut l, &mut r, ChannelMode::Stereo);
        decode_mode(&mut l, &mut r, ChannelMode::Stereo);
        assert_eq!(l, l0);
        assert_eq!(r, r0);
    }

    #[test]
    fn mono_sums() {
        let (l0, r0) = signal(64, 5);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        encode_mode(&mut l, &mut r, ChannelMode::Mono);
        decode_mode(&mut l, &mut r, ChannelMode::Mono);
        for i in 0..64 {
            assert_eq!(l[i], r[i]);
            assert!((l[i] - (l0[i] + r0[i]) * 0.5).abs() < 1e-7);
        }
    }
}
