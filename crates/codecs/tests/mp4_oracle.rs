//! MP4 audio against ffmpeg: ffmpeg writes an AAC track (with the edit list that skips the encoder
//! priming), and reads at another sample rate must land on the same source samples as the native
//! decode, however the requests are cut.

mod common;

use common::*;

/// A source whose rate differs from the sequence's is resampled per request. An AAC track in MP4
/// carries an edit list that skips the encoder priming (ffmpeg: 1024 samples), and every request
/// must still land on the same source samples: reading at 44.1 kHz in video-frame-sized pieces (as
/// an export does) equals the 48 kHz decode interpolated at the same times.
#[test]
fn aac_mp4_resampled_reads_follow_the_edit_list() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let Some(f) = fixture(&ff, "tone_aac_48k.mp4", &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:d=3", "-c:a", "aac", "-b:a", "128k"]) else {
        panic!("could not write the AAC fixture")
    };
    let src = filmcraft_codecs::open_bytes("tone_aac_48k.mp4", bytes(&f)).unwrap();
    let native = src.audio(0, 2 * 48_000, 48_000).unwrap().channels[0].clone();
    let ratio = 48_000.0 / 44_100.0;
    // 30 fps at 44.1 kHz, starting mid-file and off the 48 kHz grid
    let (mut start, piece) = (22_051i64, 1470usize);
    let (mut worst, mut peak) = (0f32, 0f32);
    for _ in 0..20 {
        let got = src.audio(start, piece, 44_100).unwrap();
        for (k, &v) in got.channels[0].iter().enumerate() {
            let pos = (start + k as i64) as f64 * ratio;
            let i0 = pos.floor() as usize;
            let want = native[i0] + (native[i0 + 1] - native[i0]) * (pos - i0 as f64) as f32;
            worst = worst.max((v - want).abs());
            peak = peak.max(v.abs());
        }
        start += piece as i64;
    }
    let level = native.iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(peak <= level * 1.01, "resampled peak {peak} above the source's {level}");
    assert!(worst < 1e-4, "resampled read differs from the source by {worst}");
}

/// ffmpeg's MP4 muxer skips the B-frame reorder delay with an edit list, so the media (`mdhd`) runs
/// one frame past what plays. The clip must be as long as ffmpeg decodes it: 60 frames, not 61.
#[test]
fn bframe_mp4_duration_is_the_decoded_frame_count() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let args = ["-f", "lavfi", "-i", "testsrc2=size=160x96:rate=30:duration=2", "-c:v", "libx264", "-bf", "2", "-pix_fmt", "yuv420p"];
    let Some(f) = fixture(&ff, "bframes_x264.mp4", &args) else {
        eprintln!("SKIPPED: ffmpeg without libx264");
        return;
    };
    let n = ffmpeg_frames(&ff, &f, "yuv420p").len() / (160 * 96 * 3 / 2);
    assert_eq!(n, 60);
    let src = filmcraft_codecs::open_bytes("bframes_x264.mp4", bytes(&f)).unwrap();
    assert_eq!(src.info().duration, filmcraft_time::FrameRate::FPS_30.tick_of(n as i64));
}

/// Any file's first audio stream, decoded by us and by ffmpeg, compared sample by sample; prints
/// the error, where it is worst and how it repeats (a diagnostic for user reports):
/// `FILMCRAFT_AUDIO_ORACLE=<file> cargo test -p filmcraft-codecs --test mp4_oracle audio_oracle -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: needs FILMCRAFT_AUDIO_ORACLE=<file>"]
fn audio_oracle_for_any_file() {
    let (Some(ff), Ok(path)) = (filmcraft_testkit::oracle::ffmpeg(), std::env::var("FILMCRAFT_AUDIO_ORACLE")) else { return };
    let path = std::path::PathBuf::from(path);
    let src = filmcraft_codecs::open_bytes(path.file_name().and_then(|n| n.to_str()).unwrap_or("x.mp4"), bytes(&path)).unwrap();
    let info = src.info().clone();
    let a = info.audio_streams.first().expect("an audio stream").clone();
    let rate = a.sample_rate;
    let ch = a.channels as usize;
    eprintln!("ours: {} Hz, {} ch, codec {}", rate, ch, a.codec);
    let want = ffmpeg_audio_f32(&ff, &path, &[]);
    let frames = want.len() / ch.max(1);
    let mut got: Vec<Vec<f32>> = vec![Vec::new(); ch];
    let mut pos = 0;
    while pos < frames {
        let n = 512.min(frames - pos);
        let b = src.audio(pos as i64, n, rate).unwrap();
        for (o, c) in got.iter_mut().zip(&b.channels) {
            o.extend_from_slice(c);
        }
        pos += n;
    }
    // best alignment within ±4096 samples (decoder delay / priming differences), measured over
    // the loudest two seconds
    let win = (rate as usize * 2).min(frames / 2).max(1);
    let loudest = (0..frames.saturating_sub(win)).step_by(rate as usize / 4 + 1).max_by(|a, b| {
        let e = |s: usize| (s..s + win).step_by(5).map(|i| f64::from(want.get(i * ch).copied().unwrap_or(0.0)).powi(2)).sum::<f64>();
        e(*a).total_cmp(&e(*b))
    });
    let from = loudest.unwrap_or(0).max(4096);
    let err_at = |shift: i64| -> f64 {
        let mut e = 0.0;
        let mut k = 0usize;
        for i in (from..(from + win).min(frames.saturating_sub(4096))).step_by(3) {
            let j = i as i64 + shift;
            if let (Some(g), Some(w)) = (got[0].get(i), want.get(j as usize * ch)) {
                e += f64::from(g - w).powi(2);
                k += 1;
            }
        }
        e / k.max(1) as f64
    };
    let shift = (-4096i64..=4096).min_by(|a, b| err_at(*a).total_cmp(&err_at(*b))).unwrap_or(0);
    let mut worst = (0.0f32, 0usize);
    let mut sum = 0.0f64;
    let mut sig = 0.0f64;
    let mut hist = vec![0u32; 2048];
    for i in 0..frames {
        let j = i as i64 + shift;
        if j < 0 || j as usize >= frames {
            continue;
        }
        for c in 0..ch {
            let (Some(g), Some(w)) = (got[c].get(i), want.get(j as usize * ch + c)) else { continue };
            let d = (g - w).abs();
            sum += f64::from(d * d);
            sig += f64::from(w * w);
            if d > worst.0 {
                worst = (d, i);
            }
            if d > 0.01 {
                hist[i % 2048] += 1;
            }
        }
    }
    let snr = 10.0 * (sig / sum.max(1e-30)).log10();
    let bad: u32 = hist.iter().sum();
    let mut phases: Vec<(usize, u32)> = hist.iter().copied().enumerate().filter(|(_, n)| *n > 0).collect();
    phases.sort_by_key(|p| std::cmp::Reverse(p.1));
    eprintln!("alignment shift {shift}; SNR vs ffmpeg {snr:.1} dB; worst |error| {:.4} at sample {}; samples off by > 0.01: {bad}", worst.0, worst.1);
    eprintln!("most frequent positions mod 2048 of those: {:?}", phases.iter().take(8).collect::<Vec<_>>());
}
