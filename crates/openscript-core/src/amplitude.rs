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
/// Reads the WAV, computes RMS amplitude in 30ms windows centered on each
/// frame, normalizes to 0.0–1.0, and applies a 3-frame moving average
/// smoothing pass.
pub fn extract_amplitude(wav_path: &str, fps: u32) -> Result<AmplitudeTrack, AmplitudeError> {
    let wav_bytes = std::fs::read(wav_path).map_err(|e| AmplitudeError::Io(e.to_string()))?;

    // Parse WAV header
    if wav_bytes.len() < 44 || &wav_bytes[..4] != b"RIFF" {
        return Err(AmplitudeError::InvalidWav("Not a valid WAV file".into()));
    }
    let sample_rate = u32::from_le_bytes(wav_bytes[24..28].try_into().unwrap());
    let num_channels = u16::from_le_bytes(wav_bytes[22..24].try_into().unwrap()) as usize;
    let bits_per_sample = u16::from_le_bytes(wav_bytes[34..36].try_into().unwrap());

    if bits_per_sample != 16 {
        return Err(AmplitudeError::InvalidWav(format!(
            "Only 16-bit PCM supported, got {}-bit",
            bits_per_sample
        )));
    }

    // Extract samples (16-bit signed PCM, starting at byte 44)
    let data = &wav_bytes[44..];
    let samples: Vec<i16> = data
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

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

        // Compute RMS for this window (mono: average channels)
        let mut sum_sq: f64 = 0.0;
        let mut count = 0;
        for i in (start..end).step_by(num_channels) {
            let sample = samples[i] as f64 / 32768.0;
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
        let original_var: f32 = amplitudes.iter().map(|a| (a - 0.5).powi(2)).sum::<f32>() / amplitudes.len() as f32;
        let smoothed_var: f32 = smoothed.iter().map(|a| (a - 0.5).powi(2)).sum::<f32>() / smoothed.len() as f32;
        assert!(smoothed_var <= original_var, "Smoothing should reduce variance");
    }

    #[test]
    fn test_smooth_amplitudes_empty() {
        let smoothed = smooth_amplitudes(&[], 3);
        assert!(smoothed.is_empty());
    }
}
