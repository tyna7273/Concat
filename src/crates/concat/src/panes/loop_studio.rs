// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer and Concat contributors

//! Loop Studio pane — wizard that generates looping video clips and places
//! them on Concat's timeline.

use slint::SharedString;

use crate::loop_studio::{
    CancelToken, CrossfadeType, LoopConfig, LoopProgress, LoopResult, SourceMode,
};
use crate::ui::{LoopPhase, LoopStudioData};

// ── Messages ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LoopStudioMsg {
    // Dialog lifecycle
    Open,
    Close,
    Next,
    Back,

    // Step 0 – Source
    BrowseSource(Vec<String>),
    SourceModeChanged(i32),

    // Step 1 – Audio
    BrowseMusic(Vec<String>),
    BrowseAmbient(Vec<String>),
    MusicVolumeChanged(f32),
    AmbientVolumeChanged(f32),
    AudioEnabledChanged(bool),

    // Step 2 – Loop Settings
    DurationChanged(f32),
    RandomnessChanged(f32),
    CrossfadeTypeChanged(i32),
    CrossfadeDurationChanged(f32),

    // Step 3 – Generate
    Generate,
    CancelGenerate,
    /// Worker finished — success
    Done(LoopResult),
    /// Worker finished — failure
    Failed(String),
    /// In-progress tick from the worker
    Progress(LoopProgress),

    /// User clicked "Add to Timeline" after generation
    AddToTimeline,
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct LoopStudioPane {
    data: LoopStudioData,
    cancel: Option<CancelToken>,
    last_output_path: Option<std::path::PathBuf>,
}

impl LoopStudioPane {
    pub fn new() -> Self {
        Self {
            data: LoopStudioData {
                open: false,
                step: 0,
                source_path: SharedString::default(),
                source_mode: 0,
                music_path: SharedString::default(),
                music_volume: 0.8,
                ambient_path: SharedString::default(),
                ambient_volume: 0.5,
                audio_enabled: false,
                target_duration: 60.0,
                randomness: 3.0,
                crossfade_type: 0,
                crossfade_duration: 1.0,
                phase: LoopPhase::Idle,
                progress: 0.0,
                progress_label: SharedString::default(),
                error: SharedString::default(),
            },
            cancel: None,
            last_output_path: None,
        }
    }

    /// The data the Slint UI reads.
    pub fn data(&self) -> &LoopStudioData {
        &self.data
    }

    /// Handle a message. Returns an optional action for the caller to perform
    /// (e.g. "place these paths on the timeline").
    pub fn handle(&mut self, msg: LoopStudioMsg) -> Option<LoopStudioAction> {
        match msg {
            // ── Lifecycle ─────────────────────────────────────────────────
            LoopStudioMsg::Open => {
                self.data.open = true;
                self.data.step = 0;
                self.data.phase = LoopPhase::Idle;
                self.data.progress = 0.0;
                self.data.error = SharedString::default();
            }
            LoopStudioMsg::Close => {
                self.abort_generation();
                self.data.open = false;
                self.data.step = 0;
                self.data.phase = LoopPhase::Idle;
            }

            // ── Wizard navigation ─────────────────────────────────────────
            LoopStudioMsg::Next => {
                if self.data.step < 3 {
                    self.data.step += 1;
                } else {
                    // On step 3, "Next" is the same as "Generate"
                    return self.handle(LoopStudioMsg::Generate);
                }
            }
            LoopStudioMsg::Back => {
                if self.data.step > 0 {
                    self.data.step -= 1;
                    // Clear any previous failure so the wizard feels fresh
                    if self.data.phase == LoopPhase::Failed {
                        self.data.phase = LoopPhase::Idle;
                        self.data.error = SharedString::default();
                    }
                }
            }

            // ── Step 0 – Source ───────────────────────────────────────────
            LoopStudioMsg::BrowseSource(paths) => {
                if let Some(p) = paths.into_iter().next() {
                    self.data.source_path = p.into();
                }
            }
            LoopStudioMsg::SourceModeChanged(i) => {
                self.data.source_mode = i;
            }

            // ── Step 1 – Audio ────────────────────────────────────────────
            LoopStudioMsg::BrowseMusic(paths) => {
                if let Some(p) = paths.into_iter().next() {
                    self.data.music_path = p.into();
                    self.data.audio_enabled = true;
                }
            }
            LoopStudioMsg::BrowseAmbient(paths) => {
                if let Some(p) = paths.into_iter().next() {
                    self.data.ambient_path = p.into();
                }
            }
            LoopStudioMsg::MusicVolumeChanged(v) => {
                self.data.music_volume = v;
            }
            LoopStudioMsg::AmbientVolumeChanged(v) => {
                self.data.ambient_volume = v;
            }
            LoopStudioMsg::AudioEnabledChanged(on) => {
                self.data.audio_enabled = on;
            }

            // ── Step 2 – Loop Settings ────────────────────────────────────
            LoopStudioMsg::DurationChanged(s) => {
                self.data.target_duration = s;
            }
            LoopStudioMsg::RandomnessChanged(s) => {
                self.data.randomness = s;
            }
            LoopStudioMsg::CrossfadeTypeChanged(i) => {
                self.data.crossfade_type = i;
            }
            LoopStudioMsg::CrossfadeDurationChanged(s) => {
                self.data.crossfade_duration = s;
            }

            // ── Step 3 – Generate ─────────────────────────────────────────
            LoopStudioMsg::Generate => {
                self.data.phase = LoopPhase::Generating;
                self.data.progress = 0.0;
                self.data.progress_label = "Starting…".into();
                self.data.error = SharedString::default();

                let config = self.build_config();
                let cancel = CancelToken::new();
                self.cancel = Some(cancel.clone());
                return Some(LoopStudioAction::SpawnGenerate { config, cancel });
            }
            LoopStudioMsg::CancelGenerate => {
                self.abort_generation();
                self.data.phase = LoopPhase::Idle;
                self.data.progress = 0.0;
                self.data.progress_label = SharedString::default();
            }

            // ── Worker callbacks ──────────────────────────────────────────
            LoopStudioMsg::Progress(p) => {
                self.data.progress = p.fraction;
                self.data.progress_label = p.label.into();
            }
            LoopStudioMsg::Done(result) => {
                self.cancel = None;
                self.data.phase = LoopPhase::Done;
                self.data.progress = 1.0;
                self.data.progress_label = format!(
                    "Loop ready — {:.0}s",
                    result.duration_seconds
                )
                .into();
                self.last_output_path = Some(result.output_path.clone());
                return Some(LoopStudioAction::ReadyForTimeline {
                    path: result.output_path.to_string_lossy().into_owned(),
                    duration: result.duration_seconds,
                });
            }
            LoopStudioMsg::Failed(e) => {
                self.cancel = None;
                self.data.phase = LoopPhase::Failed;
                self.data.error = e.into();
            }

            // ── Post-generation ───────────────────────────────────────────
            LoopStudioMsg::AddToTimeline => {
                self.data.open = false;
                self.data.step = 0;
                self.data.phase = LoopPhase::Idle;
                if let Some(path) = self.last_output_path.take() {
                    return Some(LoopStudioAction::ImportToProject(path));
                }
            }
        }

        None
    }

    // ── Private helpers ───────────────────────────────────────────────────

    fn build_config(&self) -> LoopConfig {
        LoopConfig {
            source_path: self.data.source_path.as_str().into(),
            source_mode: SourceMode::from_index(self.data.source_mode),
            music_path: if self.data.audio_enabled && !self.data.music_path.is_empty() {
                Some(self.data.music_path.as_str().into())
            } else {
                None
            },
            music_volume: self.data.music_volume,
            ambient_path: if self.data.audio_enabled && !self.data.ambient_path.is_empty() {
                Some(self.data.ambient_path.as_str().into())
            } else {
                None
            },
            ambient_volume: self.data.ambient_volume,
            target_duration: self.data.target_duration as f64,
            randomness: self.data.randomness as f64,
            crossfade_type: CrossfadeType::from_index(self.data.crossfade_type),
            crossfade_duration: self.data.crossfade_duration as f64,
        }
    }

    fn abort_generation(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

// ── Actions the pane can ask the caller to perform ────────────────────────────

#[allow(dead_code)]
pub enum LoopStudioAction {
    /// Kick off the FFmpeg worker on a background thread.
    SpawnGenerate {
        config: LoopConfig,
        cancel: CancelToken,
    },
    /// Generation finished; the path is ready to be imported to the timeline.
    ReadyForTimeline { path: String, duration: f64 },
    /// Import the rendered video into the active project/timeline.
    ImportToProject(std::path::PathBuf),
}
