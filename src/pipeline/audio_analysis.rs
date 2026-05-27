use std::path::Path;
use anyhow::{Context, Result, bail};
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::domain::audio::{AudioAnalysis, AudioOnsetCurve};

/// Number of samples per analysis frame (hop size).
/// At 44100 Hz this is ~11.6 ms per frame — fine enough for onset detection.
const HOP_SIZE: usize = 512;

/// Window size for the STFT used to compute spectral flux.
/// Must be >= HOP_SIZE. Power of 2 for FFT efficiency.
const WINDOW_SIZE: usize = 1024;

/// Load an audio file and compute the onset-strength curve.
///
/// The onset-strength curve is computed using **spectral flux**: the sum of
/// positive differences in magnitude between successive STFT frames.
/// This is a simple, robust onset detector suitable for rhythmic content.
pub fn analyze_audio(path: &Path) -> Result<AudioAnalysis> {
    let samples = decode_audio_to_mono(path)?;
    let sample_rate = samples.sample_rate;
    let pcm = samples.samples;

    let onset_curve = compute_spectral_flux_onset_curve(&pcm, sample_rate, HOP_SIZE, WINDOW_SIZE);

    Ok(AudioAnalysis {
        source_audio: path.to_path_buf(),
        sample_rate,
        onset_curve,
        observed_onsets: Vec::new(), // filled by onset_detection pipeline step
    })
}

// ---------------------------------------------------------------------------
// Decoded mono audio
// ---------------------------------------------------------------------------

struct MonoAudio {
    samples: Vec<f32>,
    sample_rate: u32,
}

fn decode_audio_to_mono(path: &Path) -> Result<MonoAudio> {
    let src = std::fs::File::open(path)
        .with_context(|| format!("Cannot open audio file: {}", path.display()))?;

    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .context("Unsupported audio format")?;

    let mut format = probed.format;

    // Find the first audio track.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .context("No audio track found in file")?;

    let track_id = track.id;
    let sample_rate = track
        .codec_params
        .sample_rate
        .context("Audio track has no sample rate")?;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("Cannot create decoder")?;

    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e).context("Error reading audio packet"),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e).context("Decode error"),
        };

        append_to_mono(&decoded, channels, &mut samples);
    }

    if samples.is_empty() {
        bail!("Audio file decoded to zero samples");
    }

    Ok(MonoAudio { samples, sample_rate })
}

/// Mix a decoded buffer down to mono and append to `out`.
fn append_to_mono(buf: &AudioBufferRef, channels: usize, out: &mut Vec<f32>) {
    match buf {
        AudioBufferRef::F32(b) => {
            let planes = b.planes();
            let len = planes.planes()[0].len();
            for i in 0..len {
                let sum: f32 = (0..channels.min(planes.planes().len()))
                    .map(|c| planes.planes()[c][i])
                    .sum();
                out.push(sum / channels as f32);
            }
        }
        AudioBufferRef::S16(b) => {
            let planes = b.planes();
            let len = planes.planes()[0].len();
            for i in 0..len {
                let sum: f32 = (0..channels.min(planes.planes().len()))
                    .map(|c| planes.planes()[c][i] as f32 / 32768.0)
                    .sum();
                out.push(sum / channels as f32);
            }
        }
        AudioBufferRef::S32(b) => {
            let planes = b.planes();
            let len = planes.planes()[0].len();
            for i in 0..len {
                let sum: f32 = (0..channels.min(planes.planes().len()))
                    .map(|c| planes.planes()[c][i] as f32 / 2_147_483_648.0)
                    .sum();
                out.push(sum / channels as f32);
            }
        }
        // For other sample formats, convert via f64 path
        _ => {
            // Fallback: use F64 planes if available, otherwise skip
            if let AudioBufferRef::F64(b) = buf {
                let planes = b.planes();
                let len = planes.planes()[0].len();
                for i in 0..len {
                    let sum: f32 = (0..channels.min(planes.planes().len()))
                        .map(|c| planes.planes()[c][i] as f32)
                        .sum();
                    out.push(sum / channels as f32);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Spectral flux onset strength
// ---------------------------------------------------------------------------

/// Compute a spectral-flux onset strength curve from mono PCM samples.
///
/// Algorithm:
///   1. Divide samples into overlapping frames of `window_size` with hop `hop_size`.
///   2. Apply a Hann window.
///   3. Compute magnitude spectrum via a simple DFT (no FFT crate needed for now;
///      can be swapped for realfft later for performance).
///   4. Compute spectral flux = sum of positive differences in magnitude vs previous frame.
///   5. Normalise the curve to [0, 1].
fn compute_spectral_flux_onset_curve(
    samples: &[f32],
    sample_rate: u32,
    hop_size: usize,
    window_size: usize,
) -> AudioOnsetCurve {
    let n_frames = if samples.len() >= window_size {
        (samples.len() - window_size) / hop_size + 1
    } else {
        0
    };

    // Precompute Hann window coefficients.
    let hann: Vec<f32> = (0..window_size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos()))
        .collect();

    let fft_bins = window_size / 2 + 1;
    let mut prev_magnitudes = vec![0.0_f32; fft_bins];
    let mut flux_values: Vec<f32> = Vec::with_capacity(n_frames);

    for frame_idx in 0..n_frames {
        let start = frame_idx * hop_size;
        let frame = &samples[start..start + window_size];

        // Apply Hann window.
        let windowed: Vec<f32> = frame.iter().zip(hann.iter()).map(|(s, w)| s * w).collect();

        // Compute DFT magnitudes (real-to-complex, positive frequencies only).
        let magnitudes = real_dft_magnitudes(&windowed, fft_bins);

        // Spectral flux: sum of positive differences.
        let flux: f32 = magnitudes
            .iter()
            .zip(prev_magnitudes.iter())
            .map(|(mag, prev)| (mag - prev).max(0.0))
            .sum();

        flux_values.push(flux);
        prev_magnitudes = magnitudes;
    }

    // Normalise to [0, 1].
    let max_val = flux_values.iter().cloned().fold(0.0_f32, f32::max);
    if max_val > 0.0 {
        for v in flux_values.iter_mut() {
            *v /= max_val;
        }
    }

    AudioOnsetCurve {
        sample_rate,
        hop_size,
        values: flux_values,
    }
}

/// Compute magnitudes of the real DFT for the positive frequencies (0..=N/2).
/// This is O(N²) — fine for offline analysis; swap with realfft crate for speed.
fn real_dft_magnitudes(windowed: &[f32], fft_bins: usize) -> Vec<f32> {
    let n = windowed.len() as f32;
    (0..fft_bins)
        .map(|k| {
            let mut re = 0.0_f32;
            let mut im = 0.0_f32;
            for (i, &x) in windowed.iter().enumerate() {
                let angle = -2.0 * std::f32::consts::PI * k as f32 * i as f32 / n;
                re += x * angle.cos();
                im += x * angle.sin();
            }
            (re * re + im * im).sqrt()
        })
        .collect()
}
