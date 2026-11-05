//! Alert tones, synthesised at startup so the game ships no audio files.

use crate::sim::scoring::SimEvent;
use macroquad::audio::{Sound, load_sound_from_bytes, play_sound_once};

const SAMPLE_RATE: u32 = 22050;

/// A 16-bit mono WAV holding the given tones: `(frequency Hz, seconds)`, 0 Hz for silence.
fn tone_wav(tones: &[(f32, f32)]) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::new();
    for &(frequency, seconds) in tones {
        let count = (seconds * SAMPLE_RATE as f32) as usize;
        for n in 0..count {
            let t = n as f32 / SAMPLE_RATE as f32;
            // Short fades at each end stop the tone clicking.
            let fade = (n.min(count - n) as f32 / 200.0).min(1.0);
            let wave = (t * frequency * std::f32::consts::TAU).sin();
            samples.push((wave * fade * 0.35 * i16::MAX as f32) as i16);
        }
    }

    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Sounds are optional: a machine with no audio device still gets a playable game.
pub struct Sounds {
    conflict: Option<Sound>,
    separation: Option<Sound>,
    done: Option<Sound>,
    pub muted: bool,
}

impl Sounds {
    pub async fn load() -> Sounds {
        async fn make(tones: &[(f32, f32)]) -> Option<Sound> {
            load_sound_from_bytes(&tone_wav(tones)).await.ok()
        }
        Sounds {
            conflict: make(&[(880.0, 0.12), (0.0, 0.06), (880.0, 0.12)]).await,
            separation: make(&[(1200.0, 0.18), (700.0, 0.18), (1200.0, 0.18), (700.0, 0.18)]).await,
            done: make(&[(660.0, 0.08), (990.0, 0.12)]).await,
            muted: false,
        }
    }

    pub fn play_for(&self, events: &[SimEvent]) {
        if self.muted {
            return;
        }
        let sound = |s: &Option<Sound>| {
            if let Some(s) = s {
                play_sound_once(s);
            }
        };
        let any = |test: fn(&SimEvent) -> bool| events.iter().any(test);

        if any(|e| {
            matches!(
                e,
                SimEvent::LossOfSeparation { .. }
                    | SimEvent::NearCollision { .. }
                    | SimEvent::Msaw { .. }
            )
        }) {
            sound(&self.separation);
        } else if any(|e| {
            matches!(e, SimEvent::ConflictAlert { .. } | SimEvent::WakeViolation { .. })
        }) {
            sound(&self.conflict);
        } else if any(|e| matches!(e, SimEvent::Landed { .. } | SimEvent::Exited { .. })) {
            sound(&self.done);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_describes_the_samples() {
        let wav = tone_wav(&[(440.0, 0.1)]);
        let samples = (0.1 * SAMPLE_RATE as f32) as usize;
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + samples * 2);
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len as usize, samples * 2);
    }
}
