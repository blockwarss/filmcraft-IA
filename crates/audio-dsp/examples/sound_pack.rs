//! Render every built-in sound effect and a short demo of every music style as WAV files, to
//! listen to them: `cargo run --release -p filmcraft-audio-dsp --example sound_pack -- <dir>`.

use filmcraft_audio_dsp::synth::Buf;
use filmcraft_audio_dsp::{music, sfx};

fn wav(b: &Buf) -> Vec<u8> {
    let data: Vec<u8> = b.interleaved().iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()).collect();
    let mut v = Vec::with_capacity(44 + data.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&b.sample_rate.to_le_bytes());
    v.extend_from_slice(&(b.sample_rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend(data);
    v
}

fn main() -> std::io::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "sound_pack".into()));
    std::fs::create_dir_all(dir.join("sfx"))?;
    std::fs::create_dir_all(dir.join("music"))?;
    for s in sfx::CATALOG {
        if let Some(b) = sfx::render(s.id, 0, 48_000) {
            std::fs::write(dir.join("sfx").join(format!("{}-{}.wav", s.category, s.id)), wav(&b))?;
        }
    }
    for s in music::STYLES {
        let p = music::MusicParams { style: s.id.into(), seconds: 24.0, bpm: None, key: None, seed: 1 };
        match music::generate(&p, 48_000) {
            Ok(t) => std::fs::write(dir.join("music").join(format!("{}-{}bpm.wav", s.id, t.bpm)), wav(&t.buf))?,
            Err(e) => eprintln!("{e}"),
        }
    }
    println!("wrote {}", dir.display());
    Ok(())
}
