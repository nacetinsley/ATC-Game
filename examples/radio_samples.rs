//! Renders the radio voices' listening set to WAV files, to compare by ear
//! before and after a change to the voices:
//!
//!     cargo run --release --example radio_samples -- [out-dir]
//!
//! Writes one file per transmission, `busy.wav` with them all one after the
//! other as on a busy frequency, and `report.txt` with what's measured
//! without listening. The default folder is `target/radio_samples/current`;
//! render into another one (`target/radio_samples/before`) to keep a set to
//! compare against.
//!
//!     cargo run --release --example radio_samples -- --vocabulary [out-dir]
//!
//! Instead says every clip in the vocabulary, one WAV per voice
//! (`vocabulary-<voice>.wav`, default folder `target/radio_samples/vocabulary`),
//! with `vocabulary.txt` giving the time each clip starts, to listen through
//! for words said wrong.

use atcgame::ui::audio::{SAMPLE_RATE, wav};
use atcgame::ui::voice::{VOICE_NAMES, Voices, measure, samples};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let vocabulary = args.iter().position(|a| a == "--vocabulary").map(|i| args.remove(i)).is_some();
    let default = if vocabulary { "target/radio_samples/vocabulary" } else { "target/radio_samples/current" };
    let out = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(default));
    std::fs::create_dir_all(&out).expect("make the output folder");
    if vocabulary {
        write_vocabulary(&out);
    } else {
        write_samples(&out);
    }
    println!("written to {}", out.display());
}

fn write_vocabulary(out: &Path) {
    let voices = Voices::load();
    let keys: Vec<&str> = include_str!("../assets/sounds/voice/words.txt")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let between = vec![0i16; (SAMPLE_RATE as f32 * 0.4) as usize];
    let mut index = String::new();
    for (n, name) in VOICE_NAMES.iter().enumerate() {
        let mut all = Vec::new();
        for key in &keys {
            let Some(clip) = voices.clip(n, key) else {
                eprintln!("{name}: no clip for {key:?}");
                continue;
            };
            if n == 0 {
                let secs = all.len() as f32 / SAMPLE_RATE as f32;
                let _ = writeln!(index, "{:>3}:{:05.2}  {key}", (secs / 60.0) as u32, secs % 60.0);
            }
            all.extend_from_slice(&clip);
            all.extend_from_slice(&between);
        }
        std::fs::write(out.join(format!("vocabulary-{name}.wav")), wav(&all)).expect("write a voice's vocabulary");
        println!("{name}: {} clips, {:.0} minutes", keys.len(), all.len() as f32 / SAMPLE_RATE as f32 / 60.0);
    }
    // The times are the first voice's; the others run a little longer or
    // shorter, in the same order.
    std::fs::write(out.join("vocabulary.txt"), index).expect("write the index");
}

fn write_samples(out: &Path) {
    let voices = Voices::load();
    let callsigns = ["NVA101", "BRZ682", "ORN481", "BRZ652", "N613WW", "ORN640", "TRL932", "MRD291", "ORN938", "MRD757", "MRD464", "MRD987", "FDX1407", "BRZ622"];
    let between = vec![0i16; (SAMPLE_RATE as f32 * 0.35) as usize];
    let mut busy = Vec::new();
    let mut report = String::from("name                  secs  words  wpm  longest gap  clipped\n");
    let (mut words, mut speech_s) = (0usize, 0.0f32);
    for (n, sample) in samples().iter().enumerate() {
        let Some((audio, said)) = voices.render(sample.speaker, &sample.talker, sample.text, &callsigns) else {
            eprintln!("{}: nothing rendered (no voice packs?)", sample.name);
            continue;
        };
        let m = measure(&audio, said);
        words += m.words;
        speech_s += m.words as f32 / m.words_per_minute * 60.0;
        let _ = writeln!(
            report,
            "{:<20} {:5.2} {:6} {:4.0} {:10.2} s {:8}",
            sample.name, m.seconds, m.words, m.words_per_minute, m.longest_gap_s, m.clipped
        );
        std::fs::write(out.join(format!("{:02}-{}.wav", n + 1, sample.name)), wav(&audio)).expect("write a sample");
        busy.extend_from_slice(&audio);
        busy.extend_from_slice(&between);
    }
    let _ = writeln!(report, "\nall: {words} words at {:.0} words a minute", words as f32 / speech_s * 60.0);
    std::fs::write(out.join("busy.wav"), wav(&busy)).expect("write busy.wav");
    std::fs::write(out.join("report.txt"), &report).expect("write the report");
    print!("{report}");
}
