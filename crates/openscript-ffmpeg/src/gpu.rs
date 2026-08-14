//! NVENC/NVDEC hardware acceleration for the ffmpeg render path.
//!
//! The video **filter graph is unchanged** — it stays a software filter graph
//! (scale/overlay/subtitles/loudnorm are CPU filters). GPU acceleration is
//! layered around it in two safe, transparent ways:
//!
//! 1. **NVDEC decode**: `-hwaccel cuda` is added before each `-i`. Frames are
//!    decoded on the GPU and downloaded to system memory automatically, so the
//!    existing software filter graph consumes them unchanged. (Do NOT add
//!    `-hwaccel_output_format cuda` — that keeps frames on the device and
//!    requires per-filter hwdownload management that would break the graph.)
//! 2. **NVENC encode**: `h264_nvenc` replaces `libx264` with a preset mapping
//!    (x264 names → p1..p7) and `-cq` (constant quality) in place of `-crf`.
//!    FFmpeg auto-uploads the filtered system-memory frames to the device.
//!
//! Controlled by `OPENSCRIPT_FFMPEG_GPU` (default `auto`):
//! - `auto`  → use NVENC/NVDEC when this ffmpeg build has BOTH `h264_nvenc`
//!             and the `cuda` hwaccel, else fall back to CPU libx264
//! - `cpu`   → force CPU libx264, never touch the GPU
//! - `nvenc` → force NVENC; falls back to CPU with a loud warning if the
//!             encoder is missing (same fail-soft contract as the sidecar
//!             `OPENSCRIPT_DEVICE=cuda` behavior)
//!
//! Quality/size note: NVENC spends roughly 2x the bits of x264 at equal
//! CQ/CRF numbers, so `-cq = crf + 2` is applied to land near x264's file
//! size. CPU and GPU renders of the same `crf` are therefore NOT bit-identical
//! in quality or size — compare them knowing that difference is expected.

use std::sync::OnceLock;
use tokio::process::Command;

/// GPU acceleration mode for the ffmpeg render path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuMode {
    /// Auto-detect: use NVENC/NVDEC when the encoder + cuda hwaccel exist, else CPU.
    Auto,
    /// Force CPU (libx264) encoding. Never touches the GPU.
    Cpu,
    /// Force NVENC encoding + CUDA decode.
    Nvenc,
}

impl GpuMode {
    /// Parse from the `OPENSCRIPT_FFMPEG_GPU` env var.
    ///
    /// Values: `auto` (default), `cpu`, `nvenc`. Unknown values log a warning
    /// and fall back to `auto` so a typo can never silently disable or force
    /// a mode the caller didn't intend.
    pub fn from_env() -> Self {
        let raw = std::env::var("OPENSCRIPT_FFMPEG_GPU").unwrap_or_default();
        Self::from_value(&raw)
    }

    /// Pure string parsing — separate from `from_env` so unit tests don't
    /// mutate process-global env vars (which is flaky under `--test-threads`).
    pub fn from_value(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cpu" => GpuMode::Cpu,
            "nvenc" | "gpu" | "cuda" => GpuMode::Nvenc,
            "" | "auto" => GpuMode::Auto,
            other => {
                tracing::warn!(
                    "[gpu] Unknown OPENSCRIPT_FFMPEG_GPU value '{}' — falling back to auto",
                    other
                );
                GpuMode::Auto
            }
        }
    }
}

/// Probe once per process whether the NVIDIA driver actually works at RUNTIME.
///
/// `ffmpeg -encoders`/`-hwaccels` only prove the BINARY was compiled with
/// NVENC/NVDEC support. A driver/library mismatch (nvidia-smi exits non-zero
/// with "Failed to initialize NVML: Driver/library version mismatch") makes
/// every `-hwaccel cuda` decode fail mid-render with
/// "Hardware device setup failed for decoder: device type cuda needed for
/// codec h264" — AFTER all the TTS/broll prep work. Probe the live driver:
/// `nvidia-smi --query-gpu=name` must exit 0 AND return a non-empty name.
/// If nvidia-smi is missing or fails, treat the GPU as unusable → CPU fallback.
fn cuda_driver_usable() -> bool {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let name = String::from_utf8_lossy(&o.stdout);
            !name.trim().is_empty()
        }
        Ok(_) => false, // exit != 0 (e.g. NVML driver/library mismatch)
        Err(_) => false, // nvidia-smi not installed
    }
}

/// Minimum free VRAM (MiB) required before `auto` mode enables the hardware
/// paths. On small cards (e.g. 8 GB), a resident TTS sidecar (IndexTTS /
/// Audio8 / VoiceDesign, ~2-4 GB) plus NVDEC decoder surface pools and the
/// NVENC encoder can exceed available VRAM. NVDEC decoder failures degrade
/// SOFT (ffmpeg falls back to software decode), but h264_nvenc ENCODER init
/// failure is FATAL — the render dies after all the TTS/broll prep work.
/// Gate `auto` on headroom so the render degrades to CPU libx264 BEFORE
/// spawning ffmpeg instead of crashing mid-render.
const GPU_MIN_FREE_VRAM_MIB: u64 = 2048;

/// Probe current free VRAM (MiB) via nvidia-smi. `None` when the query fails
/// (no GPU / driver mismatch / nvidia-smi absent) — callers treat `None` as
/// "unknown, assume OK" so machines where the query is unavailable keep the
/// existing behavior.
fn cuda_free_vram_mib() -> Option<u64> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next()?.trim().parse::<u64>().ok()
}

/// Pure decision helper: enough free VRAM headroom for NVDEC+NVENC?
/// `None` (probe unavailable) → true, preserving the old behavior.
fn vram_headroom_ok(free_mib: Option<u64>) -> bool {
    match free_mib {
        Some(free) => free >= GPU_MIN_FREE_VRAM_MIB,
        None => true,
    }
}

/// True when a failed ffmpeg stderr indicates the NVENC encoder failed to
/// initialize due to GPU memory pressure — the trigger for a one-shot CPU
/// retry in the render entry points. Decoder failures never match (they are
/// soft); only the fatal encoder-init OOM does.
pub fn nvenc_oom_failure(stderr: &str) -> bool {
    stderr.contains("h264_nvenc")
        && (stderr.contains("out of memory")
            || stderr.contains("Cannot allocate memory")
            || stderr.contains("InitializeEncoder failed"))
}

/// Probe once per process whether this ffmpeg build can actually accelerate:
/// the `h264_nvenc` encoder AND the `cuda` hwaccel must both exist AND the
/// live NVIDIA driver must be functional.
///
/// Checking all three matters: an ffmpeg compiled with `--enable-nvenc` on a
/// machine with a broken/absent NVIDIA driver passes the encoder grep but
/// `-hwaccel cuda` fails at render time. Requiring the live-driver probe makes
/// `auto` mode degrade BEFORE spawning ffmpeg instead of mid-render.
fn nvenc_available_once() -> &'static bool {
    static AVAIL: OnceLock<bool> = OnceLock::new();
    AVAIL.get_or_init(|| {
        let has_encoder = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"))
            .unwrap_or(false);
        let has_cuda_hwaccel = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-hwaccels"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("cuda"))
            .unwrap_or(false);
        has_encoder && has_cuda_hwaccel && cuda_driver_usable()
    })
}

/// True when this ffmpeg build has the NVIDIA H.264 encoder AND cuda hwaccel.
pub fn h264_nvenc_available() -> bool {
    *nvenc_available_once()
}

/// Resolved per-render GPU configuration.
///
/// Resolve once per render and pass `&GpuConfig` to the input/encoder helpers —
/// probing is cached, so repeated resolution is free, but keeping one instance
/// per command keeps the args consistent (a `cpu` render never touches the GPU).
#[derive(Debug, Clone, Copy)]
pub struct GpuConfig {
    /// Use `-hwaccel cuda` (NVDEC) before each input.
    pub decode: bool,
    /// Use `h264_nvenc` for the video encoder.
    pub encode_nvenc: bool,
}

impl GpuConfig {
    /// Resolve the effective config from the mode + environment.
    ///
    /// `Nvenc` (explicit) fails SOFT: if the encoder is missing, warn loudly
    /// and fall back to CPU rather than crashing the render with a cryptic
    /// "Unknown encoder h264_nvenc" after all the TTS/broll prep work.
    pub fn resolve() -> Self {
        let mode = GpuMode::from_env();
        let cfg = match mode {
            GpuMode::Cpu => GpuConfig {
                decode: false,
                encode_nvenc: false,
            },
            GpuMode::Auto => {
                let avail = h264_nvenc_available();
                let headroom = cuda_free_vram_mib();
                let headroom_ok = vram_headroom_ok(headroom);
                if !avail {
                    tracing::info!("[gpu] h264_nvenc/cuda not available — using CPU libx264");
                } else if !headroom_ok {
                    // VRAM-pressure guard (e.g. a resident TTS sidecar on a
                    // small card). NVENC encoder init OOM is fatal; degrade
                    // BEFORE spawning ffmpeg rather than mid-render.
                    tracing::warn!(
                        "[gpu] Free VRAM below {:.1} GiB minimum ({}) — degrading to CPU libx264 to avoid NVENC OOM (OPENSCRIPT_FFMPEG_GPU=nvenc to force)",
                        GPU_MIN_FREE_VRAM_MIB / 1024,
                        headroom
                            .map(|m| format!("{} MiB free", m))
                            .unwrap_or_else(|| "unknown".to_string())
                    );
                } else {
                    tracing::info!("[gpu] h264_nvenc + cuda available — NVENC encode + CUDA decode");
                }
                GpuConfig {
                    decode: avail && headroom_ok,
                    encode_nvenc: avail && headroom_ok,
                }
            }
            GpuMode::Nvenc => {
                if h264_nvenc_available() {
                    GpuConfig {
                        decode: true,
                        encode_nvenc: true,
                    }
                } else {
                    tracing::warn!(
                        "[gpu] OPENSCRIPT_FFMPEG_GPU=nvenc but h264_nvenc/cuda is not available in this ffmpeg build — falling back to CPU libx264"
                    );
                    GpuConfig {
                        decode: false,
                        encode_nvenc: false,
                    }
                }
            }
        };
        tracing::info!(
            "[gpu] mode={:?} decode={} encode_nvenc={}",
            mode,
            cfg.decode,
            cfg.encode_nvenc
        );
        cfg
    }

    /// True when any hardware acceleration is active (for logging/telemetry).
    pub fn active(&self) -> bool {
        self.decode || self.encode_nvenc
    }

    /// The pure args for the NVDEC input prelude — unit-testable.
    pub fn input_args(&self) -> Vec<&'static str> {
        if self.decode {
            vec!["-hwaccel", "cuda"]
        } else {
            Vec::new()
        }
    }

    /// Append NVDEC args before an input. Call immediately before every `-i`.
    ///
    /// Non-h264 inputs (GIF stickers, PNGs, audio concat, SFX WAVs) have no
    /// CUDA decoder — ffmpeg logs a harmless "hardware acceleration failed"
    /// warning and falls back to software for that input. That fallback is
    /// expected and safe; the decode acceleration targets the background
    /// stock clips, which are h264.
    pub fn add_input(&self, cmd: &mut Command) {
        for a in self.input_args() {
            cmd.arg(a);
        }
    }

    /// The pure video-encoder args — unit-testable.
    ///
    /// `profile_high` adds `-profile:v high` (used by the NLE/timeline paths;
    /// the from-scratch paths let the encoder pick its default profile).
    pub fn encoder_args(
        &self,
        preset: &str,
        crf: u32,
        fps: u32,
        profile_high: bool,
    ) -> Vec<String> {
        let mut out = Vec::new();
        if self.encode_nvenc {
            let nv_preset = map_preset(preset);
            tracing::info!(
                "[gpu] Encoding with h264_nvenc (preset {} <- '{}', cq {})",
                nv_preset,
                preset,
                crf + 2
            );
            out.push("-c:v".to_string());
            out.push("h264_nvenc".to_string());
            out.push("-preset".to_string());
            out.push(nv_preset.to_string());
            // Constant-quality VBR: -cq is NVENC's analogue of x264's -crf,
            // offset by +2 to land near x264's file size (see module docs).
            // `-b:v 0` keeps the rate control in pure CQ mode so the bitrate
            // cap doesn't override the quality target.
            out.push("-rc:v".to_string());
            out.push("vbr".to_string());
            out.push("-cq".to_string());
            out.push(crf.saturating_add(2).to_string());
            out.push("-b:v".to_string());
            out.push("0".to_string());
        } else {
            out.push("-c:v".to_string());
            out.push("libx264".to_string());
            out.push("-preset".to_string());
            out.push(preset.to_string());
            out.push("-crf".to_string());
            out.push(crf.to_string());
        }
        if profile_high {
            out.push("-profile:v".to_string());
            out.push("high".to_string());
        }
        out.push("-pix_fmt".to_string());
        out.push("yuv420p".to_string());
        out.push("-r".to_string());
        out.push(fps.to_string());
        out
    }

    /// Append the video encoder args, choosing NVENC or libx264.
    pub fn add_encoder(
        &self,
        cmd: &mut Command,
        preset: &str,
        crf: u32,
        fps: u32,
        profile_high: bool,
    ) {
        for a in self.encoder_args(preset, crf, fps, profile_high) {
            cmd.arg(a);
        }
    }
}

/// Map an x264-style preset name to the closest NVENC preset (p1..p7).
///
/// NVENC's p1 is fastest/lowest quality, p7 is slowest/highest quality —
/// the same axis as x264's ultrafast..veryslow. Defaults to p4 (medium) for
/// unknown names so a future custom preset string can't crash the render.
pub fn map_preset(preset: &str) -> &'static str {
    match preset {
        "ultrafast" | "superfast" | "veryfast" => "p1",
        "faster" | "fast" => "p2",
        "medium" => "p4",
        "slow" => "p5",
        "slower" => "p6",
        "veryslow" | "placebo" => "p7",
        _ => "p4",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_preset_covers_x264_axis() {
        // Fastest → slowest maps onto p1 → p7 monotonically.
        assert_eq!(map_preset("ultrafast"), "p1");
        assert_eq!(map_preset("superfast"), "p1");
        assert_eq!(map_preset("veryfast"), "p1");
        assert_eq!(map_preset("faster"), "p2");
        assert_eq!(map_preset("fast"), "p2");
        assert_eq!(map_preset("medium"), "p4");
        assert_eq!(map_preset("slow"), "p5");
        assert_eq!(map_preset("slower"), "p6");
        assert_eq!(map_preset("veryslow"), "p7");
        assert_eq!(map_preset("placebo"), "p7");
    }

    #[test]
    fn map_preset_unknown_defaults_to_medium() {
        assert_eq!(map_preset(""), "p4");
        assert_eq!(map_preset("turbo-max"), "p4");
        assert_eq!(map_preset("P7"), "p4"); // case-sensitive on purpose — callers pass x264 names
    }

    #[test]
    fn gpu_mode_from_value_parses_env_names() {
        assert_eq!(GpuMode::from_value(""), GpuMode::Auto);
        assert_eq!(GpuMode::from_value("auto"), GpuMode::Auto);
        assert_eq!(GpuMode::from_value("cpu"), GpuMode::Cpu);
        assert_eq!(GpuMode::from_value("nvenc"), GpuMode::Nvenc);
        assert_eq!(GpuMode::from_value("gpu"), GpuMode::Nvenc);
        assert_eq!(GpuMode::from_value("cuda"), GpuMode::Nvenc);
        assert_eq!(GpuMode::from_value("  CPU  "), GpuMode::Cpu);
        assert_eq!(GpuMode::from_value("NVENC"), GpuMode::Nvenc);
    }

    #[test]
    fn gpu_mode_unknown_value_falls_back_to_auto() {
        assert_eq!(GpuMode::from_value("quantum"), GpuMode::Auto);
    }

    #[test]
    fn cpu_config_disables_both_acceleration_paths() {
        let cfg = GpuConfig {
            decode: false,
            encode_nvenc: false,
        };
        assert!(!cfg.active());
        assert!(cfg.input_args().is_empty());
    }

    #[test]
    fn gpu_config_active_when_either_path_enabled() {
        assert!(GpuConfig {
            decode: true,
            encode_nvenc: false,
        }
        .active());
        assert!(GpuConfig {
            decode: false,
            encode_nvenc: true,
        }
        .active());
    }

    #[test]
    fn nvenc_config_emits_hwaccel_prelude() {
        let cfg = GpuConfig {
            decode: true,
            encode_nvenc: true,
        };
        assert_eq!(cfg.input_args(), vec!["-hwaccel", "cuda"]);
    }

    #[test]
    fn nvenc_encoder_args_are_exact_and_use_cq_offset() {
        let cfg = GpuConfig {
            decode: true,
            encode_nvenc: true,
        };
        let args = cfg.encoder_args("slow", 18, 30, true);
        assert_eq!(
            args,
            vec![
                "-c:v", "h264_nvenc",
                "-preset", "p5",          // map_preset("slow")
                "-rc:v", "vbr",
                "-cq", "20",              // crf 18 + 2
                "-b:v", "0",
                "-profile:v", "high",
                "-pix_fmt", "yuv420p",
                "-r", "30",
            ]
        );
    }

    #[test]
    fn cpu_encoder_args_match_legacy_libx264_shape() {
        let cfg = GpuConfig {
            decode: false,
            encode_nvenc: false,
        };
        let args = cfg.encoder_args("fast", 18, 30, false);
        assert_eq!(
            args,
            vec![
                "-c:v", "libx264",
                "-preset", "fast",
                "-crf", "18",
                "-pix_fmt", "yuv420p",
                "-r", "30",
            ]
        );
    }

    #[test]
    fn cuda_driver_usable_returns_bool_without_panicking() {
        // Pure function — must never panic regardless of driver state.
        // On CI (no GPU) it should be false; on a working box it may be true.
        let _ = cuda_driver_usable();
    }

    #[test]
    fn nvenc_available_once_is_cached_and_does_not_panic() {
        // First call computes; subsequent calls hit the OnceLock cache.
        let a = h264_nvenc_available();
        let b = h264_nvenc_available();
        assert_eq!(a, b);
    }

    #[test]
    fn nvenc_encoder_without_profile_high_omits_profile_arg() {
        let cfg = GpuConfig {
            decode: true,
            encode_nvenc: true,
        };
        let args = cfg.encoder_args("medium", 20, 30, false);
        assert!(!args.contains(&"-profile:v".to_string()));
        assert_eq!(args[0], "-c:v");
        assert_eq!(args[1], "h264_nvenc");
    }

    #[test]
    fn vram_headroom_gates_on_minimum() {
        // Above the minimum → OK.
        assert!(vram_headroom_ok(Some(4096)));
        assert!(vram_headroom_ok(Some(GPU_MIN_FREE_VRAM_MIB)));
        // Below the minimum → degrade to CPU.
        assert!(!vram_headroom_ok(Some(1024)));
        assert!(!vram_headroom_ok(Some(0)));
        // Unknown (probe failed) → assume OK, preserve old behavior.
        assert!(vram_headroom_ok(None));
    }

    #[test]
    fn nvenc_oom_failure_matches_fatal_encoder_oom() {
        let fatal = "[h264_nvenc @ 0x55c0a6167000] InitializeEncoder failed: out of memory (10): \n[vost#0:0/h264_nvenc @ 0x55c0a5902300] Error while opening encoder";
        assert!(nvenc_oom_failure(fatal));
        // Full-stderr shape from a real OOM render: the encoder init error
        // (which carries the h264_nvenc name) plus the later frame-pump OOM.
        let fatal2 = "[h264_nvenc @ 0x55c0a6167000] InitializeEncoder failed: out of memory (10): \n[fc#0 @ 0x55c0a58f72c0] Error sending frames to consumers: Cannot allocate memory";
        assert!(nvenc_oom_failure(fatal2));
    }

    #[test]
    fn nvenc_oom_failure_ignores_soft_decode_failures_and_other_errors() {
        // NVDEC decoder hwaccel failure is SOFT (ffmpeg falls back to
        // software decode) — must NOT trigger a retry.
        let soft_decode = "[h264 @ 0x55c0a8bb8540] decoder->cvdl->cuvidCreateDecoder failed -> CUDA_ERROR_OUT_OF_MEMORY: out of memory\n[h264 @ 0x55c0a8bb8540] Failed setup for format cuda: hwaccel initialisation returned error.";
        assert!(!nvenc_oom_failure(soft_decode));
        // A non-GPU failure must never trigger the retry.
        assert!(!nvenc_oom_failure("No such file or directory"));
        assert!(!nvenc_oom_failure(""));
        // Encoder OOM words without the nvenc codec name → not ours.
        assert!(!nvenc_oom_failure("libx264: Cannot allocate memory"));
    }
}
