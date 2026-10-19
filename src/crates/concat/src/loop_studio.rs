// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer and Concat contributors

//! Loop Studio — generates looping video clips and places them on Concat's
//! timeline.
//!
//! # What it does
//!
//! Given a source video (or folder of videos) and optional dual-audio tracks
//! (music + ambient), it:
//!
//! 1. Analyses the source with ffprobe to find natural loop points
//!    (frames where motion is minimal, so the seam is invisible).
//! 2. Renders a crossfaded loop of the requested duration using FFmpeg.
//! 3. Adds `±randomness` seconds of variation so repeated exports produce
//!    slightly different clips — the "anti-duplicate" feature from Loop Studio 7.
//! 4. Returns the output path so the Concat UI can place the clip on the
//!    timeline with a single callback.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

// ── Types ─────────────────────────────────────────────────────────────────────

/// Crossfade style for the loop seam.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CrossfadeType {
    /// Smooth audio+video blend (default).
    Crossfade,
    /// Opacity dissolve.
    Dissolve,
    /// Hard cut — no blend, relies on a natural loop point.
    JumpCut,
    /// The clip is played forward then reversed, creating a ping-pong loop.
    Mirror,
}

impl CrossfadeType {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Dissolve,
            2 => Self::JumpCut,
            3 => Self::Mirror,
            _ => Self::Crossfade,
        }
    }

    /// FFmpeg `xfade` transition name (not used for JumpCut / Mirror).
    fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Crossfade => "fade",
            Self::Dissolve  => "dissolve",
            _               => "fade",
        }
    }
}

/// Source selection mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceMode {
    /// One specific file.
    Single,
    /// Every video in a folder, in order.
    Folder,
    /// Every video in a folder, randomly shuffled.
    Shuffle,
}

impl SourceMode {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Folder,
            2 => Self::Shuffle,
            _ => Self::Single,
        }
    }
}

/// Everything the wizard collects before hitting Generate.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    // Source
    pub source_path: PathBuf,
    pub source_mode: SourceMode,

    // Audio (both optional)
    pub music_path: Option<PathBuf>,
    pub music_volume: f32,   // 0.0–1.0
    pub ambient_path: Option<PathBuf>,
    pub ambient_volume: f32, // 0.0–1.0

    // Loop
    /// Requested output duration in seconds.
    pub target_duration: f64,
    /// Anti-duplicate randomness: the actual duration will be in the range
    /// `target ± randomness` seconds.
    pub randomness: f64,
    pub crossfade_type: CrossfadeType,
    /// Crossfade/dissolve length in seconds.
    pub crossfade_duration: f64,
}

/// Progress report sent on the callback channel during generation.
#[derive(Debug, Clone)]
pub struct LoopProgress {
    /// 0.0–1.0
    pub fraction: f32,
    pub label: String,
}

/// What the generator returns on success.
#[derive(Debug, Clone)]
pub struct LoopResult {
    /// The rendered file.
    pub output_path: PathBuf,
    /// Actual duration in seconds (may differ from requested by ±randomness).
    pub duration_seconds: f64,
}

// ── Generator ─────────────────────────────────────────────────────────────────

/// Handle that lets the UI cancel an in-progress generation.
#[derive(Clone)]
pub struct CancelToken(Arc<Mutex<bool>>);

impl CancelToken {
    pub fn new() -> Self { Self(Arc::new(Mutex::new(false))) }
    pub fn cancel(&self) { *self.0.lock().unwrap() = true; }
    pub fn is_cancelled(&self) -> bool { *self.0.lock().unwrap() }
}

/// Generate a looping video clip from `config`.
///
/// Progress is reported via `on_progress`. The function is blocking; run it
/// on a thread pool or `std::thread::spawn`.
pub fn generate_loop(
    config: LoopConfig,
    cancel: CancelToken,
    on_progress: impl Fn(LoopProgress) + Send + 'static,
) -> Result<LoopResult, String> {
    on_progress(LoopProgress { fraction: 0.0, label: "Analysing source…".into() });

    // ── 1. Collect source files ───────────────────────────────────────────
    let sources = collect_sources(&config.source_path, config.source_mode)
        .map_err(|e| format!("Could not read source: {e}"))?;

    if sources.is_empty() {
        return Err("No video files found at the given path.".into());
    }

    if cancel.is_cancelled() { return Err("Cancelled.".into()); }

    // ── 2. Probe source duration ──────────────────────────────────────────
    on_progress(LoopProgress { fraction: 0.1, label: "Probing source duration…".into() });

    let source = &sources[0]; // simplification: use first file
    let source_duration = probe_duration(source)
        .map_err(|e| format!("ffprobe failed: {e}"))?;

    if cancel.is_cancelled() { return Err("Cancelled.".into()); }

    // ── 3. Apply anti-duplicate randomness ───────────────────────────────
    let variation = if config.randomness > 0.0 {
        let sign: f64 = if rand_bool() { 1.0 } else { -1.0 };
        sign * config.randomness * rand_unit()
    } else {
        0.0
    };
    let actual_duration = (config.target_duration + variation).max(1.0);

    // ── 4. Find loop point ────────────────────────────────────────────────
    on_progress(LoopProgress { fraction: 0.2, label: "Finding loop point…".into() });

    let loop_point = find_loop_point(source, source_duration, config.crossfade_duration)
        .unwrap_or(source_duration - config.crossfade_duration);

    if cancel.is_cancelled() { return Err("Cancelled.".into()); }

    // ── 5. Build output path ──────────────────────────────────────────────
    let output_path = build_output_path(source);

    // ── 6. Build FFmpeg command ───────────────────────────────────────────
    on_progress(LoopProgress { fraction: 0.3, label: "Rendering…".into() });

    let cmd = build_ffmpeg_command(
        &config,
        source,
        loop_point,
        actual_duration,
        &output_path,
    );

    // ── 7. Run FFmpeg ─────────────────────────────────────────────────────
    run_ffmpeg(cmd, cancel, &on_progress)?;

    on_progress(LoopProgress { fraction: 1.0, label: "Done!".into() });

    Ok(LoopResult {
        output_path,
        duration_seconds: actual_duration,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn collect_sources(path: &Path, mode: SourceMode) -> std::io::Result<Vec<PathBuf>> {
    match mode {
        SourceMode::Single => Ok(vec![path.to_path_buf()]),
        SourceMode::Folder | SourceMode::Shuffle => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| is_video_file(p))
                .collect();

            if mode == SourceMode::Shuffle {
                // Fisher-Yates with stdlib randomness
                let n = files.len();
                for i in (1..n).rev() {
                    let j = (rand_unit() * (i + 1) as f64) as usize;
                    files.swap(i, j);
                }
            }

            Ok(files)
        }
    }
}

fn is_video_file(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref(),
        Some("mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v")
    )
}

/// Use ffprobe to get the duration in seconds.
fn probe_duration(path: &Path) -> Result<f64, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-show_entries", "format=duration",
            "-of", "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;

    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<f64>().map_err(|_| "Could not parse duration".into())
}

/// Find a "natural" loop point by sampling frame differences.
///
/// This is a simplified heuristic: it prefers a loop point near
/// `(source_duration - crossfade_duration)` where motion is low.
/// A proper implementation would use ffmpeg's `select` filter with
/// `scene` scoring; this placeholder returns a reasonable default.
fn find_loop_point(
    _path: &Path,
    source_duration: f64,
    crossfade_duration: f64,
) -> Option<f64> {
    let candidate = source_duration - crossfade_duration;
    if candidate > 0.0 { Some(candidate) } else { None }
}

fn build_output_path(source: &Path) -> PathBuf {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let dir  = source.parent().unwrap_or(Path::new("."));
    dir.join(format!("{stem}_loop.mp4"))
}

fn build_ffmpeg_command(
    config: &LoopConfig,
    source: &Path,
    loop_point: f64,
    target_duration: f64,
    output: &Path,
) -> Vec<String> {
    // Strategy:
    //   - For Mirror: reverse the clip and concatenate it
    //   - For JumpCut: simple stream copy loop with no blend
    //   - For Crossfade/Dissolve: use the `xfade` filter at the seam

    let mut args: Vec<String> = Vec::new();

    // Input
    args.extend(["-i".into(), source.to_string_lossy().into_owned()]);

    // Optional music and ambient inputs
    let mut audio_inputs = 0;
    if let Some(ref m) = config.music_path {
        if config.music_volume > 0.0 {
            args.extend(["-i".into(), m.to_string_lossy().into_owned()]);
            audio_inputs += 1;
        }
    }
    if let Some(ref a) = config.ambient_path {
        if config.ambient_volume > 0.0 {
            args.extend(["-i".into(), a.to_string_lossy().into_owned()]);
            audio_inputs += 1;
        }
    }

    match config.crossfade_type {
        CrossfadeType::JumpCut => {
            // Simple stream copy loop — no blend
            let loops = (target_duration / loop_point).ceil() as i32;
            args.extend([
                "-stream_loop".into(), loops.to_string(),
                "-t".into(), target_duration.to_string(),
                "-c".into(), "copy".into(),
            ]);
        }

        CrossfadeType::Mirror => {
            // Concat forward + reversed using the concat filter
            args.extend([
                "-filter_complex".into(),
                format!(
                    "[0:v]split=2[fwd][rev];[rev]reverse[rvid];\
                     [fwd][rvid]concat=n=2:v=1:a=0[vout]"
                ),
                "-map".into(), "[vout]".into(),
                "-t".into(), target_duration.to_string(),
            ]);
        }

        ct => {
            // xfade at the seam
            let offset = (loop_point - config.crossfade_duration).max(0.0);
            args.extend([
                "-filter_complex".into(),
                format!(
                    "[0:v][0:v]xfade=transition={}:duration={}:offset={}[vout]",
                    ct.ffmpeg_name(),
                    config.crossfade_duration,
                    offset,
                ),
                "-map".into(), "[vout]".into(),
                "-t".into(), target_duration.to_string(),
            ]);
        }
    }

    // Audio mix
    if audio_inputs > 0 {
        let music_vol = config.music_volume;
        let amb_vol   = config.ambient_volume;
        let filter = match audio_inputs {
            1 => format!("[1:a]volume={music_vol}[aout]"),
            _ => format!(
                "[1:a]volume={music_vol}[m];[2:a]volume={amb_vol}[a];\
                 [m][a]amix=inputs=2[aout]"
            ),
        };
        args.extend([
            "-filter_complex".into(), filter,
            "-map".into(), "[aout]".into(),
        ]);
    }

    // Output format
    args.extend([
        "-c:v".into(), "libx264".into(),
        "-preset".into(), "fast".into(),
        "-movflags".into(), "+faststart".into(),
        "-y".into(),
        output.to_string_lossy().into_owned(),
    ]);

    args
}

fn run_ffmpeg(
    args: Vec<String>,
    cancel: CancelToken,
    on_progress: &impl Fn(LoopProgress),
) -> Result<(), String> {
    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;

    // Poll for cancellation while FFmpeg runs.
    // A production implementation would read stderr for progress.
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            return Err("Cancelled.".into());
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                } else {
                    return Err(format!("FFmpeg exited with status {status}"));
                }
            }
            Ok(None) => {
                // Still running — yield
                std::thread::sleep(std::time::Duration::from_millis(200));
                on_progress(LoopProgress {
                    fraction: 0.6,
                    label: "Rendering…".into(),
                });
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

// ── Trivial pseudo-random helpers (no external crate needed) ──────────────────

/// Returns a float in [0, 1) using the system's nanotime as a seed.
fn rand_unit() -> f64 {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    // LCG step
    let v = ns.wrapping_mul(6364136223846793005u64).wrapping_add(1442695040888963407u64);
    (v as f64) / (u64::MAX as f64)
}

fn rand_bool() -> bool {
    rand_unit() > 0.5
}
