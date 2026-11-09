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

use atcgame::ui::audio::{SAMPLE_RATE, wav};
use atcgame::ui::voice::{Voices, measure, samples};
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/radio_samples/current"));
    std::fs::create_dir_all(&out).expect("make the output folder");
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
    println!("written to {}", out.display());
}
