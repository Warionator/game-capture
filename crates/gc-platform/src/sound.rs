//! Short notification chimes, synthesised at runtime (no audio assets) and
//! played with `PlaySoundW` from memory. Our own process is never part of a
//! game's process-loopback capture, so chimes don't end up in clips.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chime {
    /// Capture is ready for a game: soft rising fifth.
    Ready,
    /// Clip saved: quick bright arpeggio.
    Saved,
    /// Recording started: two firm notes.
    RecordStart,
    /// Recording stopped: falling pair.
    RecordStop,
    /// Bookmark/screenshot: single tick.
    Tick,
    /// Something failed: low double tone.
    Error,
}

const RATE: u32 = 44_100;

/// `(frequency Hz, start s, length s)` for each note of a chime.
fn notes(c: Chime) -> &'static [(f32, f32, f32)] {
    match c {
        Chime::Ready => &[(783.99, 0.0, 0.30), (1174.66, 0.11, 0.42)],
        Chime::Saved => &[
            (1046.5, 0.0, 0.22),
            (1318.5, 0.07, 0.22),
            (1568.0, 0.14, 0.40),
        ],
        Chime::RecordStart => &[(880.0, 0.0, 0.18), (880.0, 0.16, 0.30)],
        Chime::RecordStop => &[(1174.66, 0.0, 0.22), (783.99, 0.12, 0.38)],
        Chime::Tick => &[(1567.98, 0.0, 0.16)],
        Chime::Error => &[(329.63, 0.0, 0.22), (261.63, 0.16, 0.34)],
    }
}

/// Renders a chime as a 16-bit mono PCM WAV file in memory.
pub fn wav(chime: Chime, volume: f32) -> Vec<u8> {
    let ns = notes(chime);
    let total = ns.iter().map(|(_, s, l)| s + l).fold(0.0f32, f32::max) + 0.02;
    let n = (total * RATE as f32) as usize;
    let mut buf = vec![0f32; n];
    for &(f, start, len) in ns {
        let s0 = (start * RATE as f32) as usize;
        let len_n = (len * RATE as f32) as usize;
        for i in 0..len_n.min(n - s0) {
            let t = i as f32 / RATE as f32;
            let attack = (t / 0.006).min(1.0);
            let decay = (-t * 7.0 / len).exp();
            let tone = (std::f32::consts::TAU * f * t).sin()
                + 0.25 * (std::f32::consts::TAU * 2.0 * f * t).sin();
            buf[s0 + i] += tone * attack * decay;
        }
    }
    let peak = buf.iter().fold(0f32, |m, s| m.max(s.abs())).max(1e-6);
    let gain = volume.clamp(0.0, 1.0) * 0.7 / peak;
    let data: Vec<u8> = buf
        .iter()
        .flat_map(|s| (((s * gain).clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes())
        .collect();
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Plays a chime asynchronously (replacing one that is still playing).
pub fn play(chime: Chime, volume: f32) {
    if volume <= 0.0 {
        return;
    }
    #[cfg(windows)]
    {
        use parking_lot::Mutex;
        use windows::core::PCWSTR;
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
        // PlaySound reads the buffer while playing asynchronously, so it must
        // outlive the sound: keep the current one until the next call (which
        // stops the previous sound before returning).
        static CURRENT: Mutex<Option<Vec<u8>>> = Mutex::new(None);
        let data = wav(chime, volume);
        let mut cur = CURRENT.lock();
        unsafe {
            let _ = PlaySoundW(
                PCWSTR(data.as_ptr().cast()),
                None,
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
        *cur = Some(data);
    }
    #[cfg(not(windows))]
    {
        let _ = chime;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_valid_wav() {
        for c in [
            Chime::Ready,
            Chime::Saved,
            Chime::RecordStart,
            Chime::RecordStop,
            Chime::Tick,
            Chime::Error,
        ] {
            let w = wav(c, 0.6);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(&w[8..16], b"WAVEfmt ");
            let len = u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize;
            assert_eq!(w.len(), 44 + len);
            assert!(len > 2 * 44_100 / 10, "{c:?} too short");
            let peak = w[44..]
                .chunks(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
                .max()
                .unwrap();
            // volume 0.6 × 0.7 headroom ≈ 13 760
            assert!((13_000..=14_500).contains(&peak), "{c:?} peak {peak}");
        }
        assert!(wav(Chime::Tick, 0.0)[44..].iter().all(|&b| b == 0));
    }
}
