//! Key press sounds.
//!
//! The samples are synthesized at start, so no audio asset is shipped. The audio device is owned by
//! a dedicated thread which is started lazily by the first sound, a key press only sends a cue to it.

use std::sync::{
    OnceLock,
    atomic::{AtomicU32, AtomicU8, Ordering},
    mpsc::{SyncSender, sync_channel},
};

use rodio::{OutputStreamBuilder, Source, buffer::SamplesBuffer};

use crate::config::KeySound;

const SAMPLE_RATE: u32 = 44_100;

static KIND: AtomicU8 = AtomicU8::new(0);
static VOLUME_BITS: AtomicU32 = AtomicU32::new(0);
static CUES: OnceLock<Option<SyncSender<(KeySound, f32)>>> = OnceLock::new();

fn kind_to_u8(kind: KeySound) -> u8 {
    match kind {
        KeySound::Off => 0,
        KeySound::Click => 1,
        KeySound::Thock => 2,
    }
}

fn kind_from_u8(v: u8) -> KeySound {
    match v {
        1 => KeySound::Click,
        2 => KeySound::Thock,
        _ => KeySound::Off,
    }
}

pub fn set_kind(kind: KeySound) {
    KIND.store(kind_to_u8(kind), Ordering::Relaxed);
}

pub fn set_volume(volume: f32) {
    let volume = if volume.is_finite() { volume } else { 0. };
    VOLUME_BITS.store(volume.clamp(0., 1.).to_bits(), Ordering::Relaxed);
}

/// Play the sound of the configured kind, it does nothing if sounds are off.
pub fn play_key() {
    let kind = kind_from_u8(KIND.load(Ordering::Relaxed));
    if kind == KeySound::Off {
        return;
    }
    play(kind, f32::from_bits(VOLUME_BITS.load(Ordering::Relaxed)));
}

fn play(kind: KeySound, volume: f32) {
    let cues = CUES.get_or_init(|| {
        let (tx, rx) = sync_channel::<(KeySound, f32)>(32);
        let spawned = std::thread::Builder::new()
            .name("key-sound".to_string())
            .spawn(move || {
                let stream = match OutputStreamBuilder::open_default_stream() {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("key sounds are disabled, no audio output: {e}");
                        return;
                    }
                };
                let click = synth(KeySound::Click);
                let thock = synth(KeySound::Thock);
                while let Ok((kind, volume)) = rx.recv() {
                    let samples = match kind {
                        KeySound::Click => &click,
                        KeySound::Thock => &thock,
                        KeySound::Off => continue,
                    };
                    let source = SamplesBuffer::new(1, SAMPLE_RATE, samples.clone());
                    stream.mixer().add(source.amplify(volume));
                }
            });
        match spawned {
            Ok(_) => Some(tx),
            Err(e) => {
                tracing::warn!("failed to start the key sound thread: {e}");
                None
            }
        }
    });
    if let Some(cues) = cues {
        // drop the cue if the audio thread is behind, never queue up
        let _ = cues.try_send((kind, volume));
    }
}

/// A tiny deterministic noise generator, the sound must be the same every time.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.
    }
}

fn synth(kind: KeySound) -> Vec<f32> {
    let (duration, tone_hz, tone_decay, noise_decay, noise_level) = match kind {
        KeySound::Click => (0.025, 2600., 0.0018, 0.0012, 0.5),
        KeySound::Thock => (0.07, 150., 0.014, 0.003, 0.25),
        KeySound::Off => return vec![],
    };
    let len = (SAMPLE_RATE as f32 * duration) as usize;
    let mut noise = Noise(7);
    (0..len)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let tone = (std::f32::consts::TAU * tone_hz * t).sin() * (-t / tone_decay).exp();
            let hiss = noise.next() * noise_level * (-t / noise_decay).exp();
            // short fade-in avoids a pop at the start
            let fade_in = (t / 0.0004).min(1.);
            (tone * 0.8 + hiss) * fade_in * 0.7
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_bounded_and_not_silent() {
        for kind in [KeySound::Click, KeySound::Thock] {
            let s = synth(kind);
            assert!(!s.is_empty());
            assert!(s.iter().all(|v| v.abs() <= 1.));
            assert!(s.iter().any(|v| v.abs() > 0.05));
        }
    }
}
