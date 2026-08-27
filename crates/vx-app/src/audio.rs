//! Sound, synthesized. There are no audio assets in this repository and there
//! never will be for the base game: every sound is a waveform computed from
//! numbers, the same way the terrain is. Nothing to license, nothing to load,
//! nothing for a mod to be missing.
//!
//! The output device is optional equipment. A headless capture, a CI run and
//! a machine with no sound card all get a silent [`Audio`] that swallows every
//! cue without complaint — sound must never be the reason the game cannot
//! start.

use rodio::buffer::SamplesBuffer;
use rodio::mixer::Mixer;
use rodio::{ChannelCount, MixerDeviceSink, SampleRate};

/// Synthesis sample rate. Modest on purpose: a boom has no content above a
/// few kilohertz worth keeping, and short buffers are built per shot.
const RATE: u32 = 22_050;

/// One cue the game can play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// The launcher going off right next to your head: a sub-bass sweep under
    /// a hard noise burst, long tail. Loud is the point.
    Boom,
    /// The same weapon heard from somewhere else in town.
    DistantBoom,
    /// A slug meeting a wall.
    Thud,
    /// The trigger on an empty satchel.
    Click,
    /// Somebody you have walked straight into. The `usize` is the villager's
    /// variant, which picks the pitch — a town of nine should not be one
    /// voice recorded nine times.
    Grunt(usize),
}

/// The speaker, if the machine has one.
pub struct Audio {
    sink: Option<MixerDeviceSink>,
}

impl Audio {
    /// Open the default output. Failure is quiet and final: the game plays
    /// on without sound rather than retrying a device that is not there.
    pub fn open() -> Audio {
        let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(mut sink) => {
                sink.log_on_drop(false);
                Some(sink)
            }
            Err(error) => {
                log::info!("no audio output ({error}); the game will be silent");
                None
            }
        };
        Audio { sink }
    }

    /// A player with no speaker at all, for tests and paths that never want
    /// sound. Unused by the game binary itself, which is fine and said here
    /// so dead-code analysis is being overridden knowingly.
    #[allow(dead_code)]
    pub fn silent() -> Audio {
        Audio { sink: None }
    }

    /// Play one cue, at a volume where 1.0 is the cue as authored.
    pub fn play(&self, cue: Cue, volume: f32) {
        let Some(sink) = &self.sink else { return };
        if volume <= 0.0 {
            return;
        }
        play_into(sink.mixer(), cue, volume);
    }
}

fn play_into(mixer: &Mixer, cue: Cue, volume: f32) {
    let samples = match cue {
        Cue::Boom => boom(0.9, 1.0),
        Cue::DistantBoom => boom(1.2, 0.25),
        Cue::Thud => thud(),
        Cue::Click => click(),
        Cue::Grunt(variant) => grunt(variant),
    };
    let gain = volume.clamp(0.0, 2.0);
    let scaled: Vec<f32> = samples.into_iter().map(|sample| sample * gain).collect();
    let channels = ChannelCount::new(1).expect("one is not zero");
    let rate = SampleRate::new(RATE).expect("the rate is not zero");
    mixer.add(SamplesBuffer::new(channels, rate, scaled));
}

/// Deterministic noise for the synth: the same splitmix-style hash the
/// worldgen jitter uses, mapped to -1..1. No RNG state, no `rand`.
fn noise(index: u32) -> f32 {
    let mut hash = (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    ((hash >> 40) as f32) / ((1u32 << 23) as f32) - 1.0
}

/// The launcher: an exponentially decaying noise burst over a sine that
/// sweeps from a punch-in-the-chest 120 Hz down to sub-bass, soft-clipped so
/// it sounds overdriven rather than polite.
fn boom(seconds: f32, brightness: f32) -> Vec<f32> {
    let count = (RATE as f32 * seconds) as usize;
    let mut samples = Vec::with_capacity(count);
    let mut phase = 0.0f32;
    for index in 0..count {
        let t = index as f32 / RATE as f32;
        let envelope = (-t * 6.0).exp();
        // The sweep: 120 Hz falling to 35 Hz over the first half second.
        let frequency = 35.0 + 85.0 * (-t * 4.0).exp();
        phase += std::f32::consts::TAU * frequency / RATE as f32;
        let body = phase.sin() * envelope;
        let crack = noise(index as u32) * (-t * 18.0).exp() * brightness;
        // Soft clip: tanh keeps the sum loud without wrapping.
        samples.push((1.6 * (body + 0.8 * crack)).tanh() * 0.9);
    }
    samples
}

/// A slug striking something solid: a short, dark knock.
fn thud() -> Vec<f32> {
    let count = (RATE as f32 * 0.18) as usize;
    let mut samples = Vec::with_capacity(count);
    let mut phase = 0.0f32;
    for index in 0..count {
        let t = index as f32 / RATE as f32;
        let envelope = (-t * 30.0).exp();
        phase += std::f32::consts::TAU * 70.0 / RATE as f32;
        let knock = phase.sin() * envelope;
        let grit = noise(index as u32 ^ 0x5eed) * (-t * 60.0).exp() * 0.4;
        samples.push((knock + grit) * 0.6);
    }
    samples
}

/// A short, closed-mouth "hmf" — somebody registering that you are standing
/// on their feet.
///
/// Synthesized like everything else here: a low fundamental with two
/// harmonics under a fast attack and a quick decay, plus a breath of noise so
/// it reads as a body rather than a beep. The harmonics are what make it a
/// voice; a bare sine at this pitch is a fog horn.
fn grunt(variant: usize) -> Vec<f32> {
    // Three voices in the town, three pitches. Low enough to sound like a
    // chest rather than a whistle.
    let fundamental = match variant % 3 {
        0 => 104.0,
        1 => 128.0,
        _ => 92.0,
    };
    let count = (RATE as f32 * 0.19) as usize;
    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        let t = index as f32 / RATE as f32;
        // Fast in, slower out, and a slight downward drift in pitch across
        // the sound — a grunt falls away, it does not hold a note.
        let attack = (1.0 - (-t * 90.0).exp()).clamp(0.0, 1.0);
        let decay = (-t * 13.0).exp();
        let envelope = attack * decay;
        let sag = 1.0 - t * 0.55;
        let angle = std::f32::consts::TAU * fundamental * sag * t;
        let voice = angle.sin() + (angle * 2.0).sin() * 0.45 + (angle * 3.0).sin() * 0.2;
        let breath = noise(index as u32 ^ 0x9c00) * (-t * 26.0).exp() * 0.12;
        samples.push((voice * 0.34 + breath) * envelope);
    }
    samples
}

/// A dry metallic click for an empty weapon.
fn click() -> Vec<f32> {
    let count = (RATE as f32 * 0.05) as usize;
    (0..count)
        .map(|index| {
            let t = index as f32 / RATE as f32;
            noise(index as u32 ^ 0xc11c) * (-t * 200.0).exp() * 0.35
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_synthesizes_finite_bounded_audio() {
        for samples in [
            boom(0.9, 1.0),
            boom(1.2, 0.25),
            thud(),
            click(),
            grunt(0),
            grunt(1),
            grunt(2),
        ] {
            assert!(!samples.is_empty());
            for sample in &samples {
                assert!(sample.is_finite());
                assert!(sample.abs() <= 1.0, "clipping sample {sample}");
            }
        }
    }

    #[test]
    fn the_boom_is_loud_then_gone() {
        let samples = boom(0.9, 1.0);
        let peak_early = samples[..RATE as usize / 10]
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let peak_late = samples[samples.len() - RATE as usize / 10..]
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak_early > 0.5, "the boom must actually be loud: {peak_early}");
        assert!(peak_late < 0.05, "the boom must die away: {peak_late}");
    }

    #[test]
    fn a_machine_with_no_speaker_swallows_cues() {
        let audio = Audio::silent();
        audio.play(Cue::Boom, 1.0);
        audio.play(Cue::Click, 0.0);
    }

    #[test]
    fn the_synthesis_noise_is_deterministic_and_centred() {
        let first: Vec<f32> = (0..1000).map(noise).collect();
        let second: Vec<f32> = (0..1000).map(noise).collect();
        assert_eq!(first, second);
        let mean = first.iter().sum::<f32>() / first.len() as f32;
        assert!(mean.abs() < 0.1, "noise is biased: {mean}");
    }

    #[test]
    fn the_town_grunts_in_three_voices() {
        // One recording played nine times is what a town of clones sounds
        // like. Three pitches is not many, but it is more than one.
        let voices: Vec<Vec<f32>> = (0..3).map(grunt).collect();
        assert_ne!(voices[0], voices[1]);
        assert_ne!(voices[1], voices[2]);
        // And it is a grunt, not a note held: the tail is far quieter than
        // the front of it.
        for samples in &voices {
            let front: f32 = samples[..samples.len() / 6]
                .iter()
                .map(|sample| sample.abs())
                .sum();
            let tail: f32 = samples[samples.len() * 5 / 6..]
                .iter()
                .map(|sample| sample.abs())
                .sum();
            assert!(front > tail * 4.0, "the grunt does not fall away");
        }
    }
}
