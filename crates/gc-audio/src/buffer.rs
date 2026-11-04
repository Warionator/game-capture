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
}

impl TrackControl {
    pub fn new(volume: f32, muted: bool) -> Arc<Self> {
        Arc::new(Self {
            volume_bits: AtomicU32::new(volume.to_bits()),
            muted: AtomicBool::new(muted),
            peak_bits: AtomicU32::new(0f32.to_bits()),
        })
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
        let _ = self
            .peak_bits
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                (p > f32::from_bits(old)).then_some(p.to_bits())
            });
    }
    /// Returns and resets the held peak.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak_bits.swap(0f32.to_bits(), Ordering::Relaxed))
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
