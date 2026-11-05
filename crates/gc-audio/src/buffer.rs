use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Bounded FIFO of interleaved stereo f32 samples shared between a capture
/// thread (producer) and the pump (consumer). When the consumer falls
/// behind, the oldest samples are dropped so latency stays bounded.
#[derive(Clone)]
pub struct SampleRing {
    inner: Arc<Mutex<VecDeque<f32>>>,
    capacity: usize,
}

impl SampleRing {
    /// `capacity` in samples (frames × channels).
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub fn push(&self, samples: &[f32]) {
        let mut q = self.inner.lock();
        q.extend(samples.iter().copied());
        let excess = q.len().saturating_sub(self.capacity);
        if excess > 0 {
            // Keep frame alignment (stereo).
            q.drain(..excess + (excess & 1));
        }
    }

    pub fn push_silence(&self, samples: usize) {
        let mut q = self.inner.lock();
        q.extend(std::iter::repeat_n(0.0, samples));
        let excess = q.len().saturating_sub(self.capacity);
        if excess > 0 {
            q.drain(..excess + (excess & 1));
        }
    }

    /// Adds up to `out.len()` samples into `out` (mixing); returns how many
    /// were available. Missing samples contribute silence.
    pub fn mix_into(&self, out: &mut [f32]) -> usize {
        let mut q = self.inner.lock();
        let n = out.len().min(q.len());
        for (o, s) in out.iter_mut().zip(q.drain(..n)) {
            *o += s;
        }
        n
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
    }
}

/// Live, lock-free per-track controls shared with the UI.
#[derive(Debug)]
pub struct TrackControl {
    volume_bits: AtomicU32,
    muted: AtomicBool,
    peak_bits: AtomicU32,
    /// Noise-gate threshold (linear amplitude); 0 = gate off.
    gate_bits: AtomicU32,
    gate_open: AtomicBool,
}

impl TrackControl {
    pub fn new(volume: f32, muted: bool) -> Arc<Self> {
        Arc::new(Self {
            volume_bits: AtomicU32::new(volume.to_bits()),
            muted: AtomicBool::new(muted),
            peak_bits: AtomicU32::new(0f32.to_bits()),
            gate_bits: AtomicU32::new(0f32.to_bits()),
            gate_open: AtomicBool::new(true),
        })
    }
    /// Sets the noise-gate threshold in dBFS, `None` to disable the gate.
    pub fn set_gate_db(&self, db: Option<f32>) {
        let lin = db
            .map(|d| 10f32.powf(d.clamp(-100.0, 0.0) / 20.0))
            .unwrap_or(0.0);
        self.gate_bits.store(lin.to_bits(), Ordering::Relaxed);
    }
    /// Gate threshold as linear amplitude (0 = off).
    pub fn gate_threshold(&self) -> f32 {
        f32::from_bits(self.gate_bits.load(Ordering::Relaxed))
    }
    pub fn gate_open(&self) -> bool {
        self.gate_open.load(Ordering::Relaxed)
    }
    pub fn set_gate_open(&self, open: bool) {
        self.gate_open.store(open, Ordering::Relaxed);
    }
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }
    pub fn set_volume(&self, v: f32) {
        self.volume_bits
            .store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }
    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }
    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }
    /// Effective gain (0 when muted).
    pub fn gain(&self) -> f32 {
        if self.muted() {
            0.0
        } else {
            self.volume()
        }
    }
    /// Records the peak of the latest block (max-hold until read).
    pub fn update_peak(&self, p: f32) {
        // Peaks are non-negative, and non-negative IEEE-754 floats order the
        // same as their bit patterns, so an integer max is a float max.
        if p.is_nan() || p <= 0.0 {
            return;
        }
        self.peak_bits.fetch_max(p.to_bits(), Ordering::Relaxed);
    }
    /// Returns and resets the held peak.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak_bits.swap(0f32.to_bits(), Ordering::Relaxed))
    }
}

/// Downward noise gate with hold and smoothed gain (no clicks).
#[derive(Debug, Clone)]
pub struct NoiseGate {
    env: f32,
    gain: f32,
    hold_left: u32,
}

impl Default for NoiseGate {
    fn default() -> Self {
        Self {
            env: 0.0,
            gain: 1.0,
            hold_left: 0,
        }
    }
}

impl NoiseGate {
    const RATE: f32 = 48_000.0;
    /// Envelope release (~50 ms), gate hold (150 ms), gain attack (2 ms)
    /// and gain release (80 ms).
    const ENV_RELEASE_S: f32 = 0.05;
    const HOLD_S: f32 = 0.15;
    const ATTACK_S: f32 = 0.002;
    const RELEASE_S: f32 = 0.08;

    /// Gates interleaved stereo `samples` in place. Returns whether the gate
    /// is open at the end of the block. `threshold` is linear; 0 bypasses.
    pub fn process(&mut self, samples: &mut [f32], threshold: f32) -> bool {
        if threshold <= 0.0 {
            self.gain = 1.0;
            return true;
        }
        let env_coef = (-1.0 / (Self::ENV_RELEASE_S * Self::RATE)).exp();
        let up = 1.0 / (Self::ATTACK_S * Self::RATE);
        let down = 1.0 / (Self::RELEASE_S * Self::RATE);
        let hold = (Self::HOLD_S * Self::RATE) as u32;
        for frame in samples.chunks_exact_mut(2) {
            let level = frame[0].abs().max(frame[1].abs());
            self.env = if level > self.env {
                level
            } else {
                self.env * env_coef
            };
            let open = if self.env >= threshold {
                self.hold_left = hold;
                true
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
                true
            } else {
                false
            };
            self.gain = if open {
                (self.gain + up).min(1.0)
            } else {
                (self.gain - down).max(0.0)
            };
            frame[0] *= self.gain;
            frame[1] *= self.gain;
        }
        self.hold_left > 0 || self.env >= threshold
    }
}

/// Multiplies in place and hard-clips to [-1, 1].
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if (gain - 1.0).abs() < f32::EPSILON {
        for s in samples.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        return;
    }
    for s in samples.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_bounds_and_mixes() {
        let r = SampleRing::new(6);
        r.push(&[1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0]);
        assert_eq!(r.len(), 6, "oldest frame dropped");
        let mut out = vec![0.5; 8];
        assert_eq!(r.mix_into(&mut out), 6);
        assert_eq!(out, [2.5, 2.5, 3.5, 3.5, 4.5, 4.5, 0.5, 0.5]);
        assert!(r.is_empty());
    }

    #[test]
    fn noise_gate_silences_quiet_signal() {
        let c = TrackControl::new(1.0, false);
        c.set_gate_db(Some(-30.0)); // ≈ 0.0316
        let thr = c.gate_threshold();
        assert!((thr - 0.0316).abs() < 0.001);
        let mut g = NoiseGate::default();
        // 0.5 s of quiet hiss (-40 dBFS) → gate closes, output fades to 0.
        let mut quiet = vec![0.01f32; 48_000];
        let open = g.process(&mut quiet, thr);
        assert!(!open);
        assert!(quiet[quiet.len() - 2].abs() < 1e-6);
        // Speech (-12 dBFS) opens it again within a couple of ms.
        let mut loud = vec![0.25f32; 4_800];
        assert!(g.process(&mut loud, thr));
        assert!((loud[loud.len() - 2] - 0.25).abs() < 1e-6);
        // Threshold 0 bypasses.
        let mut x = vec![0.001f32; 10];
        assert!(g.process(&mut x, 0.0));
        assert_eq!(x[0], 0.001);
        c.set_gate_db(None);
        assert_eq!(c.gate_threshold(), 0.0);
    }

    #[test]
    fn gain_mute_and_peaks() {
        let c = TrackControl::new(0.5, false);
        let mut s = [0.8, -1.6, 0.2];
        apply_gain(&mut s, c.gain());
        assert_eq!(s, [0.4, -0.8, 0.1]);
        c.update_peak(peak(&s));
        c.update_peak(0.1);
        assert!((c.take_peak() - 0.8).abs() < 1e-6);
        assert_eq!(c.take_peak(), 0.0);
        c.set_muted(true);
        assert_eq!(c.gain(), 0.0);
        c.set_volume(5.0);
        assert_eq!(c.volume(), 2.0);
        let mut s = [3.0f32];
        apply_gain(&mut s, 1.0);
        assert_eq!(s, [1.0]);
    }
}
