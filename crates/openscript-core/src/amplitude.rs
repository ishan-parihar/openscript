//! WAV amplitude extraction for lip-sync animation.
//!
//! Reads a WAV file and computes per-frame RMS amplitude, which is used
//! to drive the mouth animation (scaleY) for SVG sticker puppets.
//! The amplitude is normalized to 0.0–1.0 and smoothed to avoid jitter.

use serde::Serialize;

/// Per-frame amplitude data for a WAV file.
#[derive(Debug, Clone, Serialize)]
pub struct AmplitudeTrack {
    /// Normalized amplitude (0.0–1.0) per frame, at the target fps.
    pub frames: Vec<f32>,
    /// The fps the amplitudes were sampled at.
    pub fps: u32,
    /// Total duration in milliseconds.
    pub duration_ms: i64,
}

/// Extract per-frame amplitude from a WAV file.
///
/// Reads the WAV via `hound` (chunk-aware, supports 8/16/24/32-bit PCM and
/// IEEE float), computes RMS amplitude in 30ms windows centered on each
/// frame, normalizes to 0.0–1.0, and applies a 3-frame moving average
/// smoothing pass.
///
/// Prior versions hand-parsed the WAV header assuming the `data` chunk
/// starts at byte 44 — this breaks on WAVs with `LIST`/`fact`/`bext`
/// chunks between `fmt ` and `data`, and only supported 16-bit PCM.
/// `hound` handles all of these correctly.
pub fn extract_amplitude(wav_path: &str, fps: u32) -> Result<AmplitudeTrack, AmplitudeError> {
    let reader = hound::WavReader::open(wav_path)
        .map_err(|e| AmplitudeError::InvalidWav(format!("hound: {}", e)))?;

    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let num_channels = spec.channels as usize;
    let bits_per_sample = spec.bits_per_sample;

    // Read all samples as f32, normalised to [-1.0, 1.0].
    // hound gives us i16 for 16-bit, i32 for 24/32-bit, and f32 for float.
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            // For integer samples, divide by the max value of the bit depth.
            let max_val = (1u64 << (bits_per_sample - 1)) as f32;
            match bits_per_sample {
                8 => reader
                    .into_samples::<i16>()
                    // 8-bit WAV is unsigned (0..255), center around 0
                    .map(|s| s.map(|v| (v as f32 - 128.0) / 128.0).unwrap_or(0.0))
                    .collect(),
                16 => reader
                    .into_samples::<i16>()
                    .map(|s| s.map(|v| v as f32 / max_val).unwrap_or(0.0))
                    .collect(),
                24 | 32 => reader
                    .into_samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / max_val).unwrap_or(0.0))
                    .collect(),
                _ => {
                    return Err(AmplitudeError::InvalidWav(format!(
                        "Unsupported integer bit depth: {}",
                        bits_per_sample
                    )))
                }
            }
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
    };

    if samples.is_empty() {
        return Ok(AmplitudeTrack {
            frames: Vec::new(),
            fps,
            duration_ms: 0,
        });
    }

    let total_samples = samples.len();
    let duration_ms = ((total_samples as f64 / sample_rate as f64) * 1000.0).round() as i64;
    let total_frames = ((duration_ms as f64 / 1000.0) * fps as f64).round() as usize;

    // Compute RMS amplitude per frame
    let samples_per_frame = (sample_rate as f64 / fps as f64) as usize;
    let window_samples = (sample_rate as f64 * 0.030) as usize; // 30ms window

    let mut raw_amplitudes = Vec::with_capacity(total_frames);

    for frame in 0..total_frames {
        let center_sample = frame * samples_per_frame;
        let start = center_sample.saturating_sub(window_samples / 2);
        let end = (center_sample + window_samples / 2).min(total_samples);

        if start >= end {
            raw_amplitudes.push(0.0);
            continue;
        }

        // Compute RMS for this window (mono: average channels by stepping)
        let mut sum_sq: f64 = 0.0;
        let mut count = 0;
        for i in (start..end).step_by(num_channels.max(1)) {
            let sample = samples[i] as f64;
            sum_sq += sample * sample;
            count += 1;
        }

        let rms = if count > 0 {
            (sum_sq / count as f64).sqrt()
        } else {
            0.0
        };
        raw_amplitudes.push(rms as f32);
    }

    // Find peak for normalization
    let peak = raw_amplitudes.iter().cloned().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for amp in &mut raw_amplitudes {
            *amp /= peak;
        }
    }

    // Apply 3-frame moving average smoothing
    let smoothed = smooth_amplitudes(&raw_amplitudes, 3);

    Ok(AmplitudeTrack {
        frames: smoothed,
        fps,
        duration_ms,
    })
}

/// Apply a moving average smoothing pass to the amplitude track.
fn smooth_amplitudes(amplitudes: &[f32], window: usize) -> Vec<f32> {
    if amplitudes.is_empty() || window == 0 {
        return amplitudes.to_vec();
    }

    let half = window / 2;
    amplitudes
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let start = i.saturating_sub(half);
            let end = (i + half + 1).min(amplitudes.len());
            let slice = &amplitudes[start..end];
            slice.iter().sum::<f32>() / slice.len() as f32
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum AmplitudeError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Invalid WAV: {0}")]
    InvalidWav(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal 16-bit PCM WAV file for testing.
    fn make_test_wav(sample_rate: u32, duration_ms: i64, freq: f64) -> Vec<u8> {
        let num_samples = (sample_rate as f64 * duration_ms as f64 / 1000.0) as usize;
        let data_size = num_samples * 2; // 16-bit mono
        let mut wav = Vec::with_capacity(44 + data_size);

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_size as u32).to_le_bytes());

        // Generate sine wave samples
        for i in 0..num_samples {
            let t = i as f64 / sample_rate as f64;
            let sample = (t * freq * 2.0 * std::f64::consts::PI).sin() * 0.5;
            let sample_i16 = (sample * 32767.0) as i16;
            wav.extend_from_slice(&sample_i16.to_le_bytes());
        }

        wav
    }

    #[test]
    fn test_extract_amplitude_basic() {
        let wav = make_test_wav(24000, 1000, 440.0); // 1 second of 440Hz
        let path = std::env::temp_dir().join("test_amp.wav");
        std::fs::write(&path, &wav).unwrap();

        let track = extract_amplitude(path.to_str().unwrap(), 30).unwrap();

        assert_eq!(track.fps, 30);
        assert!(track.duration_ms >= 990 && track.duration_ms <= 1010);
        assert!(!track.frames.is_empty());
        // Amplitudes should be normalized to 0.0–1.0
        for &amp in &track.frames {
            assert!(amp >= 0.0 && amp <= 1.0, "Amplitude out of range: {}", amp);
        }
        // A 440Hz sine wave should have non-zero amplitude
        assert!(track.frames.iter().sum::<f32>() > 0.0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_extract_amplitude_silence() {
        let wav = make_test_wav(24000, 500, 0.0); // silence (0Hz = no variation)
        let path = std::env::temp_dir().join("test_amp_silence.wav");
        std::fs::write(&path, &wav).unwrap();

        let track = extract_amplitude(path.to_str().unwrap(), 30).unwrap();

        // Silence should produce all-zero amplitudes
        for &amp in &track.frames {
            assert!(amp < 0.01, "Expected near-zero amplitude, got {}", amp);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_extract_amplitude_invalid_wav() {
        let path = std::env::temp_dir().join("test_amp_invalid.wav");
        std::fs::write(&path, b"not a wav file").unwrap();

        let result = extract_amplitude(path.to_str().unwrap(), 30);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_smooth_amplitudes() {
        let amplitudes = vec![0.0, 1.0, 0.0, 1.0, 0.0];
        let smoothed = smooth_amplitudes(&amplitudes, 3);
        // Smoothing should reduce the variance
        let original_var: f32 =
            amplitudes.iter().map(|a| (a - 0.5).powi(2)).sum::<f32>() / amplitudes.len() as f32;
        let smoothed_var: f32 =
            smoothed.iter().map(|a| (a - 0.5).powi(2)).sum::<f32>() / smoothed.len() as f32;
        assert!(
            smoothed_var <= original_var,
            "Smoothing should reduce variance"
        );
    }

    #[test]
    fn test_smooth_amplitudes_empty() {
        let smoothed = smooth_amplitudes(&[], 3);
        assert!(smoothed.is_empty());
    }
}
