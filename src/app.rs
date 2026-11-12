//! The RunVideo desktop UI.

use crate::ffmpeg::{self, ExportMsg, EXPORT_PROFILES};
use crate::model::{
    is_media_path, Clip, Keyframe, Layer, MediaKind, Project, TextAlign, TextSpec, Transform,
    VAlign, IMAGE_DEFAULT_DURATION, MAX_CLIPS, MAX_LAYERS, MIN_CLIP,
};
use crate::preview::{PreviewCmd, PreviewFrame, PreviewMsg, PreviewWorker};
use crate::subs;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Key, Layout, Modifiers, Pos2, Rect,
    RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(255, 166, 48);
const ACCENT_DIM: Color32 = Color32::from_rgb(105, 72, 24);
/// Width of the track header column (name + visibility toggle).
const ROW_HEADER: f32 = 156.0;
const RULER_H: f32 = 26.0;
const VIDEO_BG: Color32 = Color32::from_rgb(42, 96, 116);
const VIDEO_BG_HOVER: Color32 = Color32::from_rgb(54, 118, 141);
const IMAGE_BG: Color32 = Color32::from_rgb(116, 86, 38);
const TEXT_BG: Color32 = Color32::from_rgb(88, 60, 116);
const OK_COLOR: Color32 = Color32::from_rgb(122, 205, 140);
const ERR_COLOR: Color32 = Color32::from_rgb(240, 112, 112);
const PANEL_BG: Color32 = Color32::from_rgb(24, 24, 28);
const ROW_BG: Color32 = Color32::from_rgb(31, 31, 37);
const ROW_BG_ALT: Color32 = Color32::from_rgb(27, 27, 32);
const ROW_BG_SELECTED: Color32 = Color32::from_rgb(42, 38, 29);

struct Status {
    text: String,
    error: bool,
}

struct ExportState {
    rx: Receiver<ExportMsg>,
    cancel: Arc<AtomicBool>,
    progress: f32,
    finished: Option<Result<PathBuf, String>>,
    reported: bool,
}

#[derive(Clone, Debug)]
struct ClipView {
    id: u64,
    name: String,
    path: PathBuf,
    start: f64,
    duration: f64,
    kind: MediaKind,
    volume: f64,
    has_audio: bool,
}

#[derive(Clone, Debug)]
struct LayerView {
    index: usize,
    name: String,
    visible: bool,
    muted: bool,
    solo: bool,
    locked: bool,
    clips: Vec<ClipView>,
}

struct RowInfo {
    layer: usize,
    rect: Rect,
    header: Rect,
    clips: Vec<(u64, Rect)>,
}

/// Which part of the selected layer's preview frame is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewGrab {
    Move,
    Nw,
    Ne,
    Sw,
    Se,
}

struct PreviewGesture {
    grab: PreviewGrab,
    /// Pointer position when the gesture started (screen space).
    origin: egui::Pos2,
    /// Transform when the gesture started.
    start: Transform,
    /// Undo snapshot is only taken once the pointer actually moves.
    committed: bool,
}

/// Trimming a section by dragging its left or right edge in the timeline.
#[derive(Clone, Copy, Debug)]
struct TrimGesture {
    clip: u64,
    /// True when the left (in) edge is being dragged.
    left: bool,
    /// Original duration / in-point when the drag started.
    start_duration: f64,
    start_in: f64,
    /// Pointer x at gesture start.
    start_x: f32,
    committed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelTab {
    Section,
    Layer,
    Project,
}

pub struct RunVideoApp {
    project: Project,
    selected_clip: Option<u64>,
    /// Extra sections selected with Ctrl+click, for multi-select edits.
    selected_clips: BTreeSet<u64>,
    selected_layer: usize,
    position: f64,
    playing: bool,
    /// Replay the timeline from the start when the playhead reaches the end.
    loop_playback: bool,
    /// Timeline position the current preview stream was rendered from.
    stream_base: f64,
    /// Timestamp of the last streamed frame (seconds from `stream_base`).
    stream_time: Option<f64>,
    pps: f32,
    dirty: bool,

    worker: PreviewWorker,
    gen: u64,
    preview_tex: Option<TextureHandle>,
    preview_dims: (u32, u32),
    last_frame_key: Option<(u64, u64, u32, u32)>,
    pending_frame: Option<((u64, u64, u32, u32), Instant)>,
    /// A static preview render is in flight; keep repainting so its result is
    /// polled even when the user is idle.
    preview_inflight: bool,

    revision: u64,
    undo: Vec<Project>,
    redo: Vec<Project>,
    thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    drag_clip: Option<u64>,
    /// Seconds between the dragged section's left edge and the pointer.
    drag_grab: f64,
    /// Layer under the mouse in the timeline, used as the target for file drops.
    hovered_layer: Option<usize>,
    panel_tab: PanelTab,
    /// Timeline scroll offsets (pixels).
    h_offset: f32,
    v_offset: f32,
    /// Width of the timeline content area last frame, used by zoom-to-fit.
    timeline_width: f32,
    /// Inline track rename state.
    renaming_layer: Option<usize>,
    rename_buf: String,
    /// True right after a rename starts, so the text gets selected once.
    rename_fresh: bool,
    /// Active drag-to-reorder gesture in the track headers.
    drag_layer: Option<usize>,
    /// Active move/resize gesture in the preview.
    preview_drag: Option<PreviewGesture>,
    /// Active trim gesture in the timeline.
    trim_drag: Option<TrimGesture>,
    /// Copied section, ready to paste. Holds a clone so edits to the original
    /// after copying don't affect the clipboard.
    clipboard: Option<Clip>,
    /// Copied styling only, applied with "Paste settings".
    settings_clipboard: Option<Clip>,
    /// Cached audio peak data per media file.
    waveforms: HashMap<PathBuf, Vec<f32>>,
    waveform_pending: BTreeSet<PathBuf>,
    wave_tx: Sender<(PathBuf, Vec<f32>)>,
    wave_rx: Receiver<(PathBuf, Vec<f32>)>,
    /// Background silence-detection job: (clip id, silent ranges).
    silence_job: Option<Receiver<(u64, Result<Vec<(f64, f64)>, String>)>>,
    /// Background frame-capture job.
    capture_job: Option<Receiver<Result<PathBuf, String>>>,
    /// Window fullscreen state toggled with F11.
    fullscreen: bool,
    /// Live soundtrack for the preview (muted export-only no longer).
    audio: crate::audio::AudioPlayer,
    /// Snap dragged/edited times to nearby section edges and the playhead.
    snapping: bool,

    status: Option<Status>,
    ffmpeg_version: Option<String>,
    export: Option<ExportState>,
    show_export: bool,
    export_preset: String,
    export_profile: String,

    /// In-app shortcut/help sheet.
    show_help: bool,
    /// Last time an autosave recovery file was written.
    last_autosave: Instant,
    /// Set when a recovery project is found at startup.
    recovery: Option<PathBuf>,

    last_title: String,
}

impl RunVideoApp {
    #[allow(dead_code)]
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_project(cc, None)
    }

    pub fn new_with_project(
        cc: &eframe::CreationContext<'_>,
        open: Option<PathBuf>,
    ) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = PANEL_BG;
        visuals.window_fill = Color32::from_rgb(29, 29, 34);
        visuals.selection.bg_fill = ACCENT_DIM;
        visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 48, 56);
        visuals.widgets.active.bg_fill = Color32::from_rgb(60, 54, 40);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        });

        let ffmpeg_version = ffmpeg::ffmpeg_version();
        let (wave_tx, wave_rx) = channel();
        let mut app = Self {
            project: Project::new(),
            selected_clip: None,
            selected_clips: BTreeSet::new(),
            selected_layer: 0,
            position: 0.0,
            playing: false,
            loop_playback: false,
            stream_base: 0.0,
            stream_time: None,
            pps: 60.0,
            dirty: false,
            worker: PreviewWorker::new(),
            gen: 0,
            preview_tex: None,
            preview_dims: (0, 0),
            last_frame_key: None,
            pending_frame: None,
            preview_inflight: false,
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            thumbs: HashMap::new(),
            drag_clip: None,
            drag_grab: 0.0,
            hovered_layer: None,
            panel_tab: PanelTab::Section,
            h_offset: 0.0,
            v_offset: 0.0,
            timeline_width: 900.0,
            renaming_layer: None,
            rename_buf: String::new(),
            rename_fresh: false,
            drag_layer: None,
            preview_drag: None,
            trim_drag: None,
            clipboard: None,
            settings_clipboard: None,
            waveforms: HashMap::new(),
            waveform_pending: BTreeSet::new(),
            wave_tx,
            wave_rx,
            silence_job: None,
            capture_job: None,
            fullscreen: false,
            audio: crate::audio::AudioPlayer::new(),
            snapping: true,
            status: None,
            ffmpeg_version,
            export: None,
            show_export: false,
            export_preset: "medium".to_owned(),
            export_profile: "mp4".to_owned(),
            show_help: false,
            last_autosave: Instant::now(),
            recovery: None,
            last_title: String::new(),
        };
        if app.ffmpeg_version.is_none() {
            app.set_status(
                "FFmpeg was not found on PATH. Install FFmpeg to preview and export video.",
                true,
            );
        } else {
            app.set_status("Ready. Add media or drop files onto the window.", false);
        }
        if let Some(path) = open {
            match app.load_project_path(&path) {
                Ok(()) => {
                    app.refresh_thumbs(&cc.egui_ctx);
                    app.set_status(format!("Opened {}", path.display()), false);
                }
                Err(e) => app.set_status(e, true),
            }
        } else {
            app.check_recovery();
        }
        app
    }

    /// Load a project from disk, resetting selection and preview state.
    fn load_project_path(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read project: {e}"))?;
        let mut project: Project = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("Could not parse project: {e}"))?;
        project.sanitize()?;
        project.path = Some(path.to_path_buf());
        self.project = project;
        self.selected_clip = None;
        self.selected_clips.clear();
        self.selected_layer = 0;
        self.panel_tab = PanelTab::Section;
        self.position = 0.0;
        self.playing = false;
        self.dirty = false;
        self.undo.clear();
        self.redo.clear();
        self.thumbs.clear();
        self.revision = self.revision.wrapping_add(1);
        self.last_frame_key = None;
        self.pending_frame = None;
        self.gen += 1;
        self.worker.cancel_older(self.gen);
        self.preview_inflight = false;
        self.preview_tex = None;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Status, undo and edit bookkeeping
    // ------------------------------------------------------------------

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(Status {
            text: text.into(),
            error,
        });
    }

    fn snapshot(&mut self) {
        self.undo.push(self.project.clone());
        if self.undo.len() > 80 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Retime the whole timeline so it plays at `target`x (1.0 = normal).
    fn apply_project_speed(&mut self, target: f64) {
        if (target - self.project.speed).abs() < 1e-9 {
            return;
        }
        self.snapshot();
        let ratio = self.project.set_global_speed(target);
        if (ratio - 1.0).abs() < 1e-9 {
            self.undo.pop();
            return;
        }
        self.position = self.position.min(self.project.duration());
        self.after_edit();
        self.set_status(
            format!(
                "Project speed {:.2}x  -  timeline is now {}.",
                self.project.speed,
                fmt_time(self.project.duration())
            ),
            false,
        );
    }

    fn after_edit(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        self.playing = false;
        self.last_frame_key = None;
        self.pending_frame = None;
        self.gen += 1;
        self.worker.cancel_older(self.gen);
        self.preview_inflight = false;
        if self.project.duration() <= 0.001 {
            self.preview_tex = None;
        }
        if let Some(id) = self.selected_clip {
            if self.project.find_clip(id).is_none() {
                self.selected_clip = None;
            }
        }
        self.selected_clips
            .retain(|id| self.project.find_clip(*id).is_some());
        if let Some(id) = self.selected_clip {
            self.selected_clips.insert(id);
        }
        if self.selected_layer >= self.project.layers.len() {
            self.selected_layer = self.project.layers.len().saturating_sub(1);
        }
        let total = self.project.duration();
        if let Some(marker) = self.project.start_marker {
            if total <= MIN_CLIP {
                self.project.start_marker = None;
            } else {
                self.project.start_marker = Some(marker.clamp(0.0, total - MIN_CLIP));
            }
        }
    }

    fn undo(&mut self, ctx: &egui::Context) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.project.clone());
            self.project = prev;
            self.after_restore(ctx);
            self.set_status("Undo", false);
        }
    }

    fn redo(&mut self, ctx: &egui::Context) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.project.clone());
            self.project = next;
            self.after_restore(ctx);
            self.set_status("Redo", false);
        }
    }

    fn after_restore(&mut self, ctx: &egui::Context) {
        self.dirty = true;
        self.playing = false;
        self.position = self.position.min(self.project.duration());
        self.selected_clip = None;
        self.selected_clips.clear();
        self.selected_layer = self.selected_layer.min(self.project.layers.len().saturating_sub(1));
        self.revision = self.revision.wrapping_add(1);
        self.last_frame_key = None;
        self.pending_frame = None;
        self.gen += 1;
        self.worker.cancel_older(self.gen);
        self.preview_inflight = false;
        self.refresh_thumbs(ctx);
    }

    fn new_project(&mut self, ctx: &egui::Context) {
        self.project = Project::new();
        self.selected_clip = None;
        self.selected_clips.clear();
        self.selected_layer = 0;
        self.position = 0.0;
        self.playing = false;
        self.dirty = false;
        self.undo.clear();
        self.redo.clear();
        self.thumbs.clear();
        self.revision = self.revision.wrapping_add(1);
        self.last_frame_key = None;
        self.pending_frame = None;
        self.gen += 1;
        self.worker.cancel_older(self.gen);
        self.preview_inflight = false;
        self.preview_tex = None;
        self.set_status("New project created.", false);
        let _ = ctx;
    }

    // ------------------------------------------------------------------
    // Media import / project files
    // ------------------------------------------------------------------

    fn add_media_dialog(&mut self, ctx: &egui::Context) {
        self.add_media_dialog_for(ctx, self.selected_layer);
    }

    /// Open the file picker and add the chosen media to a specific layer.
    fn add_media_dialog_for(&mut self, ctx: &egui::Context, layer: usize) {
        let files = rfd::FileDialog::new()
            .add_filter(
                "Video and photos",
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mts", "m2ts", "mpg", "mpeg",
                    "jpg", "jpeg", "png", "bmp", "gif", "webp", "tif", "tiff",
                ],
            )
            .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi", "m4v", "mts", "m2ts", "mpg", "mpeg"])
            .add_filter("Photos", &["jpg", "jpeg", "png", "bmp", "gif", "webp", "tif", "tiff"])
            .pick_files();
        if let Some(files) = files {
            self.add_paths_to_layer(files, layer, ctx);
        }
    }

    /// Files dropped on the window go to the layer under the cursor, or to the
    /// selected layer when the drop is outside the timeline.
    fn add_paths(&mut self, files: Vec<PathBuf>, ctx: &egui::Context) {
        let layer = self.hovered_layer.unwrap_or(self.selected_layer);
        self.add_paths_to_layer(files, layer, ctx);
    }

    fn add_paths_to_layer(
        &mut self,
        files: Vec<PathBuf>,
        layer: usize,
        ctx: &egui::Context,
    ) {
        if self.project.total_clips() >= MAX_CLIPS {
            self.set_status(
                format!("Section limit reached (maximum {MAX_CLIPS} sections per project)."),
                true,
            );
            return;
        }
        let before = self.project.clone();
        let mut added = 0usize;
        let mut errors: Vec<String> = Vec::new();
        let mut added_paths: Vec<PathBuf> = Vec::new();
        let target_layer = layer.min(self.project.layers.len().saturating_sub(1));
        // New media is inserted at the playhead, not appended.
        let mut insert_at = self.position.max(0.0);

        for path in files {
            if self.project.total_clips() >= MAX_CLIPS {
                errors.push(format!(
                    "Stopped at the {MAX_CLIPS}-section project limit."
                ));
                break;
            }
            if !is_media_path(&path) {
                errors.push(format!("Unsupported file: {}", path.display()));
                continue;
            }
            match ffmpeg::probe(&path) {
                Ok(info) => {
                    let duration = match info.kind {
                        MediaKind::Video => info.duration.unwrap_or(0.0),
                        MediaKind::Image => IMAGE_DEFAULT_DURATION,
                        MediaKind::Text => IMAGE_DEFAULT_DURATION,
                    };
                    if info.kind == MediaKind::Video && duration < MIN_CLIP {
                        errors.push(format!("{}: could not read a usable duration", path.display()));
                        continue;
                    }
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("clip")
                        .to_owned();
                    let id = self.project.alloc_id();
                    // New media lands at the playhead. Its transform is seeded
                    // from the section already there (or the track's default) so
                    // a webcam keeps its corner.
                    let transform = self.seed_section_transform(target_layer, insert_at);
                    let clip = Clip {
                        id,
                        path: path.clone(),
                        name,
                        kind: info.kind,
                        in_point: 0.0,
                        duration,
                        source_duration: if info.kind == MediaKind::Video {
                            Some(duration)
                        } else {
                            None
                        },
                        has_audio: info.has_audio,
                        transform,
                        ..Default::default()
                    };
                    match self.project.insert_clip_at(target_layer, insert_at, clip) {
                        Ok(()) => {
                            added += 1;
                            self.selected_clip = Some(id);
                            added_paths.push(path);
                            // Multiple files dropped together are inserted one
                            // after another, starting at the playhead.
                            insert_at += duration;
                        }
                        Err(e) => errors.push(e),
                    }
                }
                Err(e) => errors.push(e.to_string()),
            }
        }

        if added > 0 {
            self.undo.push(before);
            if self.undo.len() > 80 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.selected_layer = target_layer;
            self.after_edit();
            for path in added_paths {
                self.ensure_thumb(ctx, &path);
            }
            let layer_name = self.project.layers[target_layer].name.clone();
            let msg = if errors.is_empty() {
                format!("Added {added} clip(s) to {layer_name}.")
            } else {
                format!("Added {added} clip(s) to {layer_name}. {}", errors.join(" "))
            };
            self.set_status(msg, false);
        } else if !errors.is_empty() {
            self.set_status(errors.join(" "), true);
        }
    }

    /// Insert a generated title/caption section at the playhead.
    fn add_text_section(&mut self, _ctx: &egui::Context) {
        if self.project.total_clips() >= MAX_CLIPS {
            self.set_status(
                format!("Section limit reached (maximum {MAX_CLIPS} sections per project)."),
                true,
            );
            return;
        }
        let layer = self.hovered_layer.unwrap_or(self.selected_layer);
        let target = layer.min(self.project.layers.len().saturating_sub(1));
        let at = self.position.max(0.0);
        let transform = self.seed_section_transform(target, at);
        let id = self.project.alloc_id();
        let mut clip = Clip {
            id,
            name: "Text".to_owned(),
            kind: MediaKind::Text,
            duration: 3.0,
            source_duration: None,
            has_audio: false,
            transform,
            text: Some(TextSpec::new("Your text here")),
            ..Default::default()
        };
        clip.speed = 1.0;
        clip.volume = 1.0;
        self.snapshot();
        match self.project.insert_clip_at(target, at, clip) {
            Ok(()) => {
                self.selected_clip = Some(id);
                self.selected_layer = target;
                self.panel_tab = PanelTab::Section;
                self.after_edit();
                self.set_status("Text section added at the playhead.", false);
            }
            Err(e) => {
                self.undo.pop();
                self.set_status(e, true);
            }
        }
    }

    /// Transform to give a newly inserted section: match the section already at
    /// `at` on that track, or fall back to the track's default.
    fn seed_section_transform(&self, layer: usize, at: f64) -> Transform {
        let Some(l) = self.project.layers.get(layer) else {
            return Transform::default();
        };
        if let Some((ci, _)) = l.locate(at) {
            l.clips[ci].transform
        } else {
            l.transform
        }
    }

    fn ensure_thumb(&mut self, ctx: &egui::Context, path: &Path) {
        if self.thumbs.contains_key(path) {
            return;
        }
        let texture = ffmpeg::thumbnail(path)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                ctx.load_texture(
                    format!("thumb:{}", path.display()),
                    image,
                    TextureOptions::LINEAR,
                )
            });
        self.thumbs.insert(path.to_path_buf(), texture);
    }

    fn refresh_thumbs(&mut self, ctx: &egui::Context) {
        let paths: BTreeSet<PathBuf> = self
            .project
            .layers
            .iter()
            .flat_map(|l| l.clips.iter().map(|c| c.path.clone()))
            .collect();
        for p in paths {
            self.ensure_thumb(ctx, &p);
        }
    }

    fn save_project(&mut self, _ctx: &egui::Context, save_as: bool) {
        let mut path = self.project.path.clone();
        if save_as || path.is_none() {
            let default_name = format!("{}.rvproj", safe_file_name(&self.project.name));
            let mut dialog = rfd::FileDialog::new()
                .add_filter("RunVideo project", &["rvproj", "json"])
                .set_file_name(default_name);
            if let Some(dir) = self.project.path.as_ref().and_then(|p| p.parent()) {
                dialog = dialog.set_directory(dir);
            }
            path = dialog.save_file();
        }
        let Some(mut path) = path else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("rvproj");
        }
        match serde_json::to_string_pretty(&self.project) {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(()) => {
                    self.project.path = Some(path.clone());
                    self.dirty = false;
                    // The recovery file is no longer needed.
                    let _ = std::fs::remove_file(Self::autosave_path());
                    self.recovery = None;
                    self.set_status(format!("Saved {}", path.display()), false);
                }
                Err(e) => self.set_status(format!("Could not save project: {e}"), true),
            },
            Err(e) => self.set_status(format!("Could not serialize project: {e}"), true),
        }
    }

    fn open_project(&mut self, ctx: &egui::Context) {
        let path = rfd::FileDialog::new()
            .add_filter("RunVideo project", &["rvproj", "json"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        match self.load_project_path(&path) {
            Ok(()) => {
                self.refresh_thumbs(ctx);
                self.set_status(format!("Opened {}", path.display()), false);
            }
            Err(e) => self.set_status(e, true),
        }
    }

    // ------------------------------------------------------------------
    // Timeline operations
    // ------------------------------------------------------------------

    fn split(&mut self) {
        let dur = self.project.duration();
        if dur <= 0.0 {
            self.set_status("Add media before splitting.", true);
            return;
        }
        let layer = self
            .selected_clip
            .and_then(|id| self.project.find_clip(id).map(|(l, _)| l))
            .unwrap_or(self.selected_layer);
        self.snapshot();
        match self.project.split_at(layer, self.position) {
            Ok(new_id) => {
                self.selected_clip = Some(new_id);
                self.selected_layer = layer;
                self.after_edit();
                self.set_status("Section split at playhead.", false);
            }
            Err(e) => {
                self.undo.pop();
                self.set_status(e, true);
            }
        }
    }

    fn delete_selected(&mut self) {
        // A multi-selection deletes as one ripple edit so no holes are left.
        if self.selected_clips.len() > 1 {
            self.ripple_delete_selected();
            return;
        }
        let Some(id) = self.selected_clip else {
            self.set_status("Select a section first, then press X to delete it.", true);
            return;
        };
        if let Some((li, _)) = self.project.find_clip(id) {
            if self.layer_locked(li) {
                self.set_status("That track is locked. Unlock it to edit.", true);
                return;
            }
        }
        self.snapshot();
        if self.project.delete_clip(id) {
            self.selected_clip = None;
            self.after_edit();
            self.set_status("Section deleted.", false);
        } else {
            self.undo.pop();
            self.set_status("That section no longer exists.", true);
        }
    }

    fn layer_locked(&self, layer: usize) -> bool {
        self.project
            .layers
            .get(layer)
            .map(|l| l.locked)
            .unwrap_or(false)
    }

    /// Copy the selected section into the internal clipboard.
    fn copy_selected(&mut self) {
        let Some(id) = self.selected_clip else {
            self.set_status("Select a section to copy.", true);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            return;
        };
        self.clipboard = Some(self.project.layers[li].clips[ci].clone());
        self.set_status("Section copied.", false);
    }

    /// Paste the clipboard section at the playhead on the selected track.
    fn paste_clipboard(&mut self) {
        let Some(mut clip) = self.clipboard.clone() else {
            self.set_status("Nothing to paste. Copy a section first.", true);
            return;
        };
        if self.project.total_clips() >= MAX_CLIPS {
            self.set_status(
                format!("Section limit reached (maximum {MAX_CLIPS} sections per project)."),
                true,
            );
            return;
        }
        let layer = self.hovered_layer.unwrap_or(self.selected_layer);
        let target = layer.min(self.project.layers.len().saturating_sub(1));
        if self.layer_locked(target) {
            self.set_status("That track is locked. Unlock it to paste.", true);
            return;
        }
        let at = self.position.max(0.0);
        clip.id = self.project.alloc_id();
        let new_id = clip.id;
        self.snapshot();
        match self.project.insert_clip_at(target, at, clip) {
            Ok(()) => {
                self.selected_clip = Some(new_id);
                self.selected_layer = target;
                self.after_edit();
                self.set_status("Section pasted at the playhead.", false);
            }
            Err(e) => {
                self.undo.pop();
                self.set_status(e, true);
            }
        }
    }

    /// Duplicate the selected section right after itself.
    fn duplicate_selected(&mut self) {
        let Some(id) = self.selected_clip else {
            self.set_status("Select a section to duplicate.", true);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            return;
        };
        if self.layer_locked(li) {
            self.set_status("That track is locked. Unlock it to edit.", true);
            return;
        }
        if self.project.total_clips() >= MAX_CLIPS {
            self.set_status(
                format!("Section limit reached (maximum {MAX_CLIPS} sections per project)."),
                true,
            );
            return;
        }
        let mut copy = self.project.layers[li].clips[ci].clone();
        copy.id = self.project.alloc_id();
        copy.name = format!("{} copy", copy.name);
        // The duplicate lands right after the original, not on top of it.
        copy.start = self.project.layers[li].clips[ci].start.max(0.0) + copy.duration;
        let new_id = copy.id;
        self.snapshot();
        self.project.layers[li].clips.insert(ci + 1, copy);
        self.selected_clip = Some(new_id);
        self.selected_layer = li;
        self.after_edit();
        self.set_status("Section duplicated.", false);
    }

    // ------------------------------------------------------------------
    // Section settings clipboard, multi-select and structural edits
    // ------------------------------------------------------------------

    /// Copy every visual/audio setting (but not identity or timing).
    fn copy_settings(&mut self) {
        let Some(id) = self.selected_clip else {
            self.set_status("Select a section to copy its settings.", true);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            return;
        };
        self.settings_clipboard = Some(self.project.layers[li].clips[ci].clone());
        self.set_status("Section settings copied.", false);
    }

    fn paste_settings(&mut self) {
        let Some(src) = self.settings_clipboard.clone() else {
            self.set_status("No section settings copied yet.", true);
            return;
        };
        let Some(id) = self.selected_clip else {
            self.set_status("Select a section to paste settings onto.", true);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            return;
        };
        if self.layer_locked(li) {
            self.set_status("That track is locked. Unlock it to edit.", true);
            return;
        }
        self.snapshot();
        let dst = &mut self.project.layers[li].clips[ci];
        dst.transform = src.transform;
        dst.keyframes = src.keyframes.clone();
        dst.speed = src.speed;
        dst.speed_end = src.speed_end;
        dst.reverse = src.reverse;
        dst.freeze = src.freeze;
        dst.volume = src.volume;
        dst.fade_in = src.fade_in.min(dst.duration);
        dst.fade_out = src.fade_out.min(dst.duration);
        dst.brightness = src.brightness;
        dst.contrast = src.contrast;
        dst.saturation = src.saturation;
        dst.blur = src.blur;
        dst.rotation = src.rotation;
        dst.crop_l = src.crop_l;
        dst.crop_r = src.crop_r;
        dst.crop_t = src.crop_t;
        dst.crop_b = src.crop_b;
        dst.pixelate = src.pixelate;
        dst.sharpen = src.sharpen;
        dst.vignette = src.vignette;
        dst.flip_h = src.flip_h;
        dst.flip_v = src.flip_v;
        if dst.kind.is_text() {
            if let Some(mut text) = src.text.clone() {
                if let Some(current) = &dst.text {
                    text.content = current.content.clone();
                }
                dst.text = Some(text);
            }
        }
        if dst.kind.is_video() {
            let max_dur = dst
                .source_duration
                .map(|sd| ((sd - dst.in_point) / dst.avg_speed().max(0.05)).max(MIN_CLIP))
                .unwrap_or(3600.0);
            dst.duration = dst.duration.clamp(MIN_CLIP, max_dur);
        }
        self.after_edit();
        self.set_status("Section settings pasted.", false);
    }

    /// Pack the selected track's sections back-to-back, closing every gap.
    fn close_gaps_selected(&mut self) {
        let layer = self.selected_layer.min(self.project.layers.len().saturating_sub(1));
        if self.layer_locked(layer) {
            self.set_status("That track is locked. Unlock it to edit.", true);
            return;
        }
        self.snapshot();
        let moved = self.project.close_gaps(layer);
        if moved == 0 {
            self.undo.pop();
            self.set_status("That track has no gaps to close.", false);
            return;
        }
        self.after_edit();
        self.set_status(format!("Closed {moved} gap(s) on the selected track."), false);
    }

    /// Ripple-delete every selected section and close the holes left behind.
    fn ripple_delete_selected(&mut self) {
        let mut ids: Vec<u64> = self.selected_clips.iter().copied().collect();
        if ids.is_empty() {
            if let Some(id) = self.selected_clip {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            self.set_status("Select one or more sections to ripple delete.", true);
            return;
        }
        for id in &ids {
            if let Some((li, _)) = self.project.find_clip(*id) {
                if self.layer_locked(li) {
                    self.set_status("A selected section is on a locked track.", true);
                    return;
                }
            }
        }
        self.snapshot();
        let removed = self.project.ripple_delete(&ids);
        if removed == 0 {
            self.undo.pop();
            self.set_status("Those sections no longer exist.", true);
            return;
        }
        self.selected_clip = None;
        self.selected_clips.clear();
        self.after_edit();
        self.set_status(
            format!("Ripple-deleted {removed} section(s); later sections moved left."),
            false,
        );
    }

    /// Insert the clipboard section at the playhead, shifting later sections on
    /// the target track to the right so nothing is overwritten.
    fn insert_edit_clipboard(&mut self) {
        let Some(mut clip) = self.clipboard.clone() else {
            self.set_status("Copy a section first (Ctrl+C), then insert-edit it.", true);
            return;
        };
        if self.project.total_clips() >= MAX_CLIPS {
            self.set_status(
                format!("Section limit reached (maximum {MAX_CLIPS} sections per project)."),
                true,
            );
            return;
        }
        let target = self
            .hovered_layer
            .unwrap_or(self.selected_layer)
            .min(self.project.layers.len().saturating_sub(1));
        if self.layer_locked(target) {
            self.set_status("That track is locked. Unlock it to edit.", true);
            return;
        }
        let at = self.position.max(0.0);
        let span = clip.duration;
        clip.id = self.project.alloc_id();
        let new_id = clip.id;
        self.snapshot();
        for c in &mut self.project.layers[target].clips {
            if c.start.max(0.0) >= at - 1e-6 {
                c.start += span;
            }
        }
        match self.project.insert_clip_at(target, at, clip) {
            Ok(()) => {
                self.selected_clip = Some(new_id);
                self.selected_layer = target;
                self.after_edit();
                self.set_status("Insert edit: later sections moved right.", false);
            }
            Err(e) => {
                self.undo.pop();
                self.set_status(e, true);
            }
        }
    }

    // ------------------------------------------------------------------
    // Frame capture, fullscreen, subtitles, silence, waveforms
    // ------------------------------------------------------------------

    fn capture_current_frame(&mut self) {
        if self.ffmpeg_version.is_none() {
            self.set_status("FFmpeg is required to capture frames.", true);
            return;
        }
        if self.project.total_clips() == 0 {
            self.set_status("Add media before capturing a frame.", true);
            return;
        }
        let default_name = format!(
            "{}_frame_{}.png",
            safe_file_name(&self.project.name),
            (self.position * 1000.0).round() as i64
        );
        let mut dialog = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name(default_name);
        if let Some(dir) = self.project.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("png");
        }
        let project = self.project.clone();
        let t = self.position;
        let (tx, rx) = channel();
        let out = path.clone();
        std::thread::Builder::new()
            .name("runvideo-capture".to_owned())
            .spawn(move || {
                let _ = tx.send(
                    ffmpeg::capture_frame(&project, t, &out)
                        .map(|()| out)
                        .map_err(|e| e.to_string()),
                );
            })
            .expect("could not start frame capture thread");
        self.capture_job = Some(rx);
        self.set_status(format!("Capturing frame to {}...", path.display()), false);
    }

    fn poll_capture(&mut self) {
        let Some(rx) = &self.capture_job else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(path)) => {
                self.capture_job = None;
                self.set_status(format!("Frame saved: {}", path.display()), false);
            }
            Ok(Err(e)) => {
                self.capture_job = None;
                self.set_status(format!("Frame capture failed: {e}"), true);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.capture_job = None;
                self.set_status("Frame capture failed.", true);
            }
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        ctx.request_repaint();
    }

    fn import_srt(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SubRip subtitles", &["srt"])
            .pick_file()
        else {
            return;
        };
        let subs = match subs::read_srt(&path) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(format!("Could not import subtitles: {e}"), true);
                return;
            }
        };
        if self.project.total_clips() + subs.len() > MAX_CLIPS {
            self.set_status(
                format!("Importing {} subtitles would exceed the {MAX_CLIPS}-section limit.", subs.len()),
                true,
            );
            return;
        }
        let layer = self.selected_layer.min(self.project.layers.len().saturating_sub(1));
        if self.layer_locked(layer) {
            self.set_status("That track is locked. Unlock it to import subtitles.", true);
            return;
        }
        self.snapshot();
        let added = subs::import_into(&mut self.project, layer, &subs);
        if added == 0 {
            self.undo.pop();
            self.set_status("No subtitles could be imported.", true);
            return;
        }
        self.panel_tab = PanelTab::Section;
        self.after_edit();
        self.set_status(
            format!("Imported {added} subtitle(s) as text sections (burned in on export)."),
            false,
        );
    }

    fn export_srt(&mut self) {
        let entries = subs::collect_from(&self.project);
        if entries.is_empty() {
            self.set_status("No text sections to export as subtitles.", true);
            return;
        }
        let default_name = format!("{}.srt", safe_file_name(&self.project.name));
        let mut dialog = rfd::FileDialog::new()
            .add_filter("SubRip subtitles", &["srt"])
            .set_file_name(default_name);
        if let Some(dir) = self.project.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("srt");
        }
        match subs::save_srt(&path, &entries) {
            Ok(()) => self.set_status(
                format!("Exported {} subtitle(s) to {}", entries.len(), path.display()),
                false,
            ),
            Err(e) => self.set_status(format!("Could not export subtitles: {e}"), true),
        }
    }

    fn cut_silence_selected(&mut self) {
        let Some(id) = self.selected_clip else {
            self.set_status("Select a video section to cut its silence.", true);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            return;
        };
        if self.layer_locked(li) {
            self.set_status("That track is locked. Unlock it to edit.", true);
            return;
        }
        let clip = &self.project.layers[li].clips[ci];
        if !clip.kind.is_video() {
            self.set_status("Silence detection only works on video sections.", true);
            return;
        }
        let path = clip.path.clone();
        let start = clip.in_point.max(0.0);
        let duration = clip.source_span().max(MIN_CLIP);
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("runvideo-silence".to_owned())
            .spawn(move || {
                let result = ffmpeg::detect_silence(&path, start, duration, -35.0, 0.4)
                    .map_err(|e| e.to_string());
                let _ = tx.send((id, result));
            })
            .expect("could not start silence detection thread");
        self.silence_job = Some(rx);
        self.set_status("Detecting silence...", false);
    }

    fn poll_silence(&mut self) {
        let Some(rx) = &self.silence_job else {
            return;
        };
        match rx.try_recv() {
            Ok((id, Ok(ranges))) => {
                self.silence_job = None;
                if ranges.is_empty() {
                    self.set_status("No silence longer than 0.4 s was found.", false);
                    return;
                }
                let Some((li, ci)) = self.project.find_clip(id) else {
                    self.set_status("That section no longer exists.", true);
                    return;
                };
                let speed = self.project.layers[li].clips[ci].avg_speed().max(0.05);
                let duration = self.project.layers[li].clips[ci].duration;
                // Build the complement of the silent ranges (timeline-local).
                let mut keep: Vec<(f64, f64)> = Vec::new();
                let mut cursor = 0.0;
                for (a, b) in ranges {
                    let a = (a / speed).clamp(0.0, duration);
                    let b = (b / speed).clamp(0.0, duration);
                    if a > cursor + 0.01 {
                        keep.push((cursor, a));
                    }
                    cursor = cursor.max(b);
                }
                if cursor < duration - 0.01 {
                    keep.push((cursor, duration));
                }
                if keep.is_empty() {
                    self.set_status("The whole section is silent; nothing to keep.", false);
                    return;
                }
                self.snapshot();
                match self.project.cut_silence(id, &keep) {
                    Ok(n) => {
                        self.selected_clip = None;
                        self.after_edit();
                        self.set_status(
                            format!("Silence removed: section split into {n} kept piece(s)."),
                            false,
                        );
                    }
                    Err(e) => {
                        self.undo.pop();
                        self.set_status(e, true);
                    }
                }
            }
            Ok((_, Err(e))) => {
                self.silence_job = None;
                self.set_status(format!("Silence detection failed: {e}"), true);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.silence_job = None;
                self.set_status("Silence detection failed.", true);
            }
        }
    }

    /// Request waveform data for every video file in the project.
    fn ensure_waveforms(&mut self) {
        let paths: BTreeSet<PathBuf> = self
            .project
            .layers
            .iter()
            .flat_map(|l| l.clips.iter())
            .filter(|c| c.kind.is_video() && c.has_audio)
            .map(|c| c.path.clone())
            .collect();
        for path in paths {
            if self.waveforms.contains_key(&path) || self.waveform_pending.contains(&path) {
                continue;
            }
            let tx = self.wave_tx.clone();
            let p = path.clone();
            self.waveform_pending.insert(path);
            std::thread::Builder::new()
                .name("runvideo-waveform".to_owned())
                .spawn(move || {
                    if let Ok(peaks) = ffmpeg::waveform(&p, None, 600) {
                        let _ = tx.send((p, peaks));
                    }
                })
                .ok();
        }
    }

    fn poll_waveforms(&mut self) {
        while let Ok((path, peaks)) = self.wave_rx.try_recv() {
            self.waveform_pending.remove(&path);
            self.waveforms.insert(path, peaks);
        }
    }

    fn move_selected_in_layer(&mut self, direction: i32) {
        let Some(id) = self.selected_clip else {
            self.set_status("Select a section to reorder.", true);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            return;
        };
        let len = self.project.layers[li].clips.len();
        let target = ci as i32 + direction;
        if target < 0 || target as usize >= len {
            return;
        }
        self.snapshot();
        // Sections have free positions, so "reorder" swaps their places on the
        // timeline instead of their list index.
        let a = self.project.layers[li].clips[ci].start.max(0.0);
        let b = self.project.layers[li].clips[target as usize].start.max(0.0);
        self.project.layers[li].clips[ci].start = b;
        self.project.layers[li].clips[target as usize].start = a;
        self.after_edit();
        self.set_status("Section moved.", false);
    }

    fn add_overlay_layer(&mut self) {
        self.snapshot();
        match self.project.add_overlay_layer() {
            Ok(idx) => {
                self.selected_layer = idx;
                self.selected_clip = None;
                self.panel_tab = PanelTab::Layer;
                self.after_edit();
                self.set_status(
                    format!(
                        "Added {}. Put a webcam clip/photo on it and position it in the inspector.",
                        self.project.layers[idx].name
                    ),
                    false,
                );
            }
            Err(e) => {
                self.undo.pop();
                self.set_status(e, true);
            }
        }
    }

    fn remove_layer(&mut self, index: usize) {
        self.snapshot();
        match self.project.remove_layer(index) {
            Ok(()) => {
                if self.selected_layer >= self.project.layers.len() {
                    self.selected_layer = self.project.layers.len().saturating_sub(1);
                }
                self.selected_clip = None;
                self.after_edit();
                self.set_status("Layer removed.", false);
            }
            Err(e) => {
                self.undo.pop();
                self.set_status(e, true);
            }
        }
    }

    fn move_layer(&mut self, index: usize, dir: i32) {
        self.snapshot();
        if self.project.move_layer(index, dir) {
            let target = (index as i32 + dir) as usize;
            if self.selected_layer == index {
                self.selected_layer = target;
            } else if self.selected_layer == target {
                self.selected_layer = index;
            }
            self.after_edit();
        } else {
            self.undo.pop();
        }
    }

    // ------------------------------------------------------------------
    // Playback & preview
    // ------------------------------------------------------------------

    fn preview_dims(&self) -> (u32, u32) {
        let w = self.project.width.max(2) as f32;
        let h = self.project.height.max(2) as f32;
        let scale = (960.0 / w).min(540.0 / h).min(1.0);
        let pw = ((w * scale).round() as u32).max(2) & !1;
        let ph = ((h * scale).round() as u32).max(2) & !1;
        (pw.max(2), ph.max(2))
    }

    fn preview_key(&self) -> (u64, u64, u32, u32) {
        let (w, h) = self.preview_dims();
        (
            self.revision,
            (self.position.max(0.0) * 1000.0).round() as u64,
            w,
            h,
        )
    }

    fn request_static(&mut self) {
        let dur = self.project.duration();
        if dur <= 0.001 {
            self.gen += 1;
            self.worker.cancel_older(self.gen);
            self.preview_inflight = false;
            self.preview_tex = None;
            return;
        }
        let (pw, ph) = self.preview_dims();
        let last_frame = 1.0 / self.project.fps.max(1) as f64;
        let t = self.position.clamp(0.0, (dur - last_frame).max(0.0));
        match ffmpeg::build_preview_plan(&self.project, t, pw, ph) {
            Some(plan) => {
                self.gen += 1;
                self.worker.cancel_older(self.gen);
                self.worker.send(PreviewCmd::Frame {
                    gen: self.gen,
                    plan,
                });
                self.preview_inflight = true;
            }
            None => {
                self.gen += 1;
                self.worker.cancel_older(self.gen);
                self.preview_inflight = false;
                self.preview_tex = None;
            }
        }
    }

    fn request_stream(&mut self) {
        let dur = self.project.duration();
        if dur <= 0.001 {
            self.playing = false;
            return;
        }
        let (pw, ph) = self.preview_dims();
        let last_frame = 1.0 / self.project.fps.max(1) as f64;
        let t = self.position.clamp(0.0, (dur - last_frame).max(0.0));
        if let Some(plan) = ffmpeg::build_preview_plan(&self.project, t, pw, ph) {
            self.gen += 1;
            self.stream_base = t;
            self.stream_time = None;
            self.worker.cancel_older(self.gen);
            self.worker.send(PreviewCmd::Stream {
                gen: self.gen,
                plan,
            });
        } else {
            self.playing = false;
        }
    }

    fn force_static(&mut self) {
        self.last_frame_key = Some(self.preview_key());
        self.pending_frame = None;
        self.request_static();
    }

    fn play(&mut self) {
        // Space / Play: with a start marker set, always begin there.
        self.start_playback(true);
    }

    /// Resume from the current playhead (Enter), ignoring the start marker.
    fn resume(&mut self) {
        self.start_playback(false);
    }

    fn start_playback(&mut self, from_marker: bool) {
        if self.export.as_ref().is_some_and(|s| s.finished.is_none()) {
            self.set_status("Wait for the export to finish before playing.", true);
            return;
        }
        let dur = self.project.duration();
        if dur <= 0.001 {
            self.set_status("Add media before playing.", true);
            return;
        }
        let last = (dur - 0.001).max(0.0);
        if from_marker {
            if let Some(marker) = self.project.start_marker {
                self.position = marker.clamp(0.0, last);
            } else if self.position >= dur - 0.001 {
                self.position = 0.0;
            }
        } else if self.position >= dur - 0.001 {
            // At the end: resume is meaningless, start over.
            self.position = self
                .project
                .start_marker
                .map(|m| m.clamp(0.0, last))
                .unwrap_or(0.0);
        }
        self.playing = true;
        self.last_frame_key = None;
        self.pending_frame = None;
        self.request_stream();
    }

    fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.playing = false;
        self.force_static();
    }

    fn toggle_play(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    fn stop(&mut self) {
        self.playing = false;
        self.position = self.project.start_marker.unwrap_or(0.0);
        self.force_static();
    }

    /// Set the playback start marker at the playhead (F).
    fn set_start_marker(&mut self) {
        let dur = self.project.duration();
        if dur <= MIN_CLIP {
            self.set_status("Add media before setting a start marker.", true);
            return;
        }
        let at = self.position.clamp(0.0, (dur - MIN_CLIP).max(0.0));
        self.snapshot();
        self.project.start_marker = Some(at);
        self.dirty = true;
        self.set_status(
            format!(
                "Start marker set at {}. Space/Play begins here; Enter resumes from the playhead (T to clear).",
                fmt_time(at)
            ),
            false,
        );
    }

    /// Clear the playback start marker (T).
    fn clear_start_marker(&mut self) {
        if self.project.start_marker.is_none() {
            self.set_status("There is no start marker to clear.", true);
            return;
        }
        self.snapshot();
        self.project.start_marker = None;
        self.dirty = true;
        self.set_status("Start marker cleared.", false);
    }

    /// Add a timeline marker at the playhead (M).
    fn add_marker(&mut self) {
        let at = self.position;
        if self.project.duration() <= MIN_CLIP {
            self.set_status("Add media before adding a marker.", true);
            return;
        }
        self.snapshot();
        if !self.project.add_marker(at) {
            self.undo.pop();
            self.set_status("There is already a marker here.", true);
            return;
        }
        self.dirty = true;
        self.set_status(format!("Marker added at {}.", fmt_time(at)), false);
    }

    /// Remove the marker nearest the playhead (Alt+M).
    fn remove_marker(&mut self) {
        let tol = (12.0 / self.pps.max(1.0) as f64).max(0.25);
        self.snapshot();
        if !self.project.remove_marker_near(self.position, tol) {
            self.undo.pop();
            self.set_status("No marker near the playhead.", true);
            return;
        }
        self.dirty = true;
        self.set_status("Marker removed.", false);
    }

    fn seek(&mut self, t: f64) {
        let dur = self.project.duration();
        if dur <= 0.0 {
            self.position = 0.0;
            return;
        }
        self.position = t.clamp(0.0, dur);
        if self.playing {
            self.playing = false;
            self.gen += 1;
            self.worker.cancel_older(self.gen);
            self.preview_inflight = false;
        }
        self.last_frame_key = None;
        self.pending_frame = None;
    }

    fn nudge(&mut self, delta: f64) {
        let dur = self.project.duration();
        if dur <= 0.0 {
            return;
        }
        self.position = (self.position + delta).clamp(0.0, dur);
        if self.playing {
            self.request_stream();
        } else {
            self.last_frame_key = None;
            self.pending_frame = None;
        }
    }

    /// Fit the whole project into the visible timeline width.
    fn zoom_to_fit(&mut self) {
        let dur = self.project.duration().max(MIN_CLIP);
        let avail = (self.timeline_width - ROW_HEADER - 16.0).max(120.0);
        self.pps = (avail / dur as f32).clamp(8.0, 300.0);
        self.h_offset = 0.0;
    }

    /// Snap a candidate time to nearby section edges, the playhead and zero.
    /// Returns the original time when snapping is off or nothing is close.
    fn snap_time(&self, t: f64, threshold_px: f32) -> f64 {
        if !self.snapping {
            return t;
        }
        let thresh = (threshold_px / self.pps.max(1.0)) as f64;
        let mut best = t;
        let mut best_d = thresh;
        let mut consider = |cand: f64| {
            let d = (cand - t).abs();
            if d < best_d {
                best_d = d;
                best = cand;
            }
        };
        consider(0.0);
        consider(self.position);
        if let Some(m) = self.project.start_marker {
            consider(m);
        }
        for layer in &self.project.layers {
            let origin = layer.start.max(0.0);
            consider(origin);
            for c in &layer.clips {
                consider(origin + c.start.max(0.0));
                consider(origin + c.start.max(0.0) + c.duration);
            }
        }
        best
    }

    fn tick_playback(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        // Drive the playhead from the frames that are actually on screen. The
        // preview stream needs a moment to start (input seek + graph setup), and
        // advancing by wall clock instead would leave the picture permanently
        // behind the timeline by that startup time.
        if let Some(t) = self.stream_time {
            let dur = self.project.duration();
            self.position = (self.stream_base + t).clamp(0.0, dur);
        }
        ctx.request_repaint();
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        loop {
            let msg = match self.worker.rx.try_recv() {
                Ok(m) => m,
                Err(_) => break,
            };
            match msg {
                PreviewMsg::Frame(frame) => {
                    if frame.gen == self.gen {
                        self.preview_inflight = false;
                        self.set_preview_texture(ctx, frame);
                        // A freshly arrived frame must be painted even if the user
                        // is idle (e.g. right after a preview drag).
                        ctx.request_repaint();
                    }
                }
                PreviewMsg::StreamFrame(frame) => {
                    if frame.gen == self.gen {
                        self.preview_inflight = false;
                        self.stream_time = Some(frame.time);
                        self.set_preview_texture(ctx, frame);
                        ctx.request_repaint();
                    }
                }
                PreviewMsg::StreamEnd(gen) => {
                    if gen == self.gen && self.playing {
                        let dur = self.project.duration();
                        if self.loop_playback && dur > 0.01 {
                            // Wrap around to the beginning of the timeline.
                            self.position = 0.0;
                            self.last_frame_key = None;
                            self.pending_frame = None;
                            self.request_stream();
                        } else {
                            self.position = dur.max(0.0);
                            self.playing = false;
                            self.force_static();
                        }
                    }
                }
                PreviewMsg::Error(gen, err) => {
                    if gen == self.gen {
                        self.preview_inflight = false;
                        self.playing = false;
                        let short: String = err.chars().take(280).collect();
                        self.set_status(short, true);
                    }
                }
            }
        }
    }

    fn set_preview_texture(&mut self, ctx: &egui::Context, frame: PreviewFrame) {
        if frame.data.len() < frame.width as usize * frame.height as usize * 4 {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            &frame.data,
        );
        if self.preview_dims == (frame.width, frame.height) {
            if let Some(tex) = &mut self.preview_tex {
                tex.set(image, TextureOptions::LINEAR);
                return;
            }
        }
        self.preview_dims = (frame.width, frame.height);
        self.preview_tex = Some(ctx.load_texture("preview", image, TextureOptions::LINEAR));
    }

    fn maybe_refresh_preview(&mut self, ctx: &egui::Context) {
        if self.playing || self.project.duration() <= 0.001 {
            return;
        }
        // Suppress preview rendering while an export is running; the two
        // pipelines would otherwise compete for all four cores and RAM.
        if self.export.as_ref().is_some_and(|s| s.finished.is_none()) {
            return;
        }
        let key = self.preview_key();
        if self.last_frame_key == Some(key) {
            return;
        }
        let now = Instant::now();
        match self.pending_frame {
            Some((pending_key, since)) if pending_key == key => {
                if now.duration_since(since) >= Duration::from_millis(90) {
                    self.last_frame_key = Some(key);
                    self.pending_frame = None;
                    self.request_static();
                } else {
                    ctx.request_repaint_after(Duration::from_millis(40));
                }
            }
            _ => {
                self.pending_frame = Some((key, now));
                ctx.request_repaint_after(Duration::from_millis(95));
            }
        }
    }

    // ------------------------------------------------------------------
    // Export
    // ------------------------------------------------------------------

    fn export_dialog(&mut self) {
        if self.project.duration() <= 0.001 {
            self.set_status("Add media before exporting.", true);
            return;
        }
        let (ext, filter_name) = match self.export_profile.as_str() {
            "gif" => ("gif", "Animated GIF"),
            "audio" => ("m4a", "M4A audio"),
            _ => ("mp4", "MP4 video"),
        };
        let default_name = format!("{}.{ext}", safe_file_name(&self.project.name));
        let mut dialog = rfd::FileDialog::new()
            .add_filter(filter_name, &[ext])
            .set_file_name(default_name);
        if let Some(dir) = self.project.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension(ext);
        }
        self.start_export(path);
    }

    fn start_export(&mut self, path: PathBuf) {
        // Never run the preview pipeline (another FFmpeg) alongside the export:
        // two 1080p filter graphs at once is what exhausts memory on small PCs.
        self.playing = false;
        self.gen += 1;
        self.worker.cancel_older(self.gen);
        self.preview_inflight = false;
        // Do not kick off a replacement frame either; just mark the preview
        // stale so it refreshes once the export has finished.
        self.last_frame_key = None;
        self.pending_frame = None;

        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = cancel.clone();
        let project = self.project.clone();
        let out = path.clone();
        let preset = self.export_preset.clone();
        let profile = self.export_profile.clone();
        std::thread::Builder::new()
            .name("runvideo-export".to_owned())
            .spawn(move || ffmpeg::run_export(project, out, preset, profile, cancel_thread, tx))
            .expect("could not start export thread");
        self.export = Some(ExportState {
            rx,
            cancel,
            progress: 0.0,
            finished: None,
            reported: false,
        });
        self.show_export = true;
        self.set_status(format!("Exporting to {}...", path.display()), false);
    }

    fn poll_export(&mut self) {
        let mut done_now: Option<Result<PathBuf, String>> = None;
        if let Some(state) = &mut self.export {
            while let Ok(msg) = state.rx.try_recv() {
                match msg {
                    ExportMsg::Progress(p) => state.progress = p,
                    ExportMsg::Done(result) => {
                        state.progress = 1.0;
                        state.finished = Some(result);
                    }
                }
            }
            if !state.reported {
                if let Some(result) = state.finished.clone() {
                    state.reported = true;
                    done_now = Some(result);
                }
            }
        }
        if let Some(result) = done_now {
            match result {
                Ok(path) => self.set_status(format!("Export complete: {}", path.display()), false),
                Err(e) => {
                    let short: String = e.lines().next().unwrap_or("Export failed").chars().take(240).collect();
                    self.set_status(short, true);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Input handling
    // ------------------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let wants_keyboard = ctx.egui_wants_keyboard_input();
        if wants_keyboard {
            return;
        }
        let (split, delete, space, set_start, clear_start, add_layer, stop_shift, resume) =
            ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::S),
                    i.consume_key(Modifiers::NONE, Key::X),
                    i.consume_key(Modifiers::NONE, Key::Space),
                    i.consume_key(Modifiers::NONE, Key::F),
                    i.consume_key(Modifiers::NONE, Key::T),
                    i.consume_key(Modifiers::NONE, Key::L),
                    i.consume_key(Modifiers::NONE, Key::ShiftRight),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
        if split {
            self.split();
        }
        if delete {
            self.delete_selected();
        }
        if space {
            self.toggle_play();
        }
        if set_start {
            self.set_start_marker();
        }
        if clear_start {
            self.clear_start_marker();
        }
        if add_layer {
            self.add_overlay_layer();
        }
        if stop_shift {
            self.stop();
        }
        if resume {
            if self.playing {
                self.pause();
            } else {
                self.resume();
            }
        }

        let ctrl = ctx.input(|i| i.modifiers.ctrl || i.modifiers.command);
        let shift = ctx.input(|i| i.modifiers.shift);
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.show_help = !self.show_help;
        }
        if ctx.input(|i| i.key_pressed(Key::F11)) {
            self.toggle_fullscreen(ctx);
        }
        if ctrl {
            if ctx.input(|i| i.key_pressed(Key::Z)) {
                if shift {
                    self.redo(&ctx);
                } else {
                    self.undo(&ctx);
                }
            }
            if ctx.input(|i| i.key_pressed(Key::Y)) {
                self.redo(&ctx);
            }
            if ctx.input(|i| i.key_pressed(Key::C)) {
                self.copy_selected();
            }
            if ctx.input(|i| i.key_pressed(Key::V)) {
                self.paste_clipboard();
            }
            if ctx.input(|i| i.key_pressed(Key::D)) {
                self.duplicate_selected();
            }
            if ctx.input(|i| i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus)) {
                self.zoom_to_fit();
            }
            if ctx.input(|i| i.key_pressed(Key::S)) {
                self.save_project(&ctx, false);
            }
            if ctx.input(|i| i.key_pressed(Key::O)) {
                self.open_project(ctx);
            }
            if ctx.input(|i| i.key_pressed(Key::E)) {
                self.export_dialog();
            }
            if ctx.input(|i| i.key_pressed(Key::N)) {
                self.new_project(ctx);
            }
        } else {
            if ctx.input(|i| i.key_pressed(Key::Delete)) {
                self.delete_selected();
            }
            let alt = ctx.input(|i| i.modifiers.alt);
            if alt {
                if ctx.input(|i| i.key_pressed(Key::M)) {
                    self.remove_marker();
                }
            } else {
                if ctx.input(|i| i.key_pressed(Key::M)) {
                    self.add_marker();
                }
            }
            // Frame stepping: comma/period step one frame, Shift makes it 10.
            let frame = 1.0 / self.project.fps.max(1) as f64;
            if ctx.input(|i| i.key_pressed(Key::Comma)) {
                self.nudge(-frame * if shift { 10.0 } else { 1.0 });
            }
            if ctx.input(|i| i.key_pressed(Key::Period)) {
                self.nudge(frame * if shift { 10.0 } else { 1.0 });
            }
            let step = if shift { 5.0 } else { 1.0 };
            if ctx.input(|i| i.key_pressed(Key::ArrowLeft)) {
                self.nudge(-step);
            }
            if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
                self.nudge(step);
            }
            if ctx.input(|i| i.key_pressed(Key::Home)) {
                self.seek(0.0);
            }
            if ctx.input(|i| i.key_pressed(Key::End)) {
                self.seek(self.project.duration());
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.fullscreen {
                self.toggle_fullscreen(&ctx);
            }
            self.drag_clip = None;
        }
    }

    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| Some(f.path().to_path_buf()))
                .collect()
        });
        if !files.is_empty() {
            self.add_paths(files, ctx);
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            ctx.request_repaint();
        }
    }

    // ------------------------------------------------------------------
    // Top bar
    // ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top_bar")
            .frame(egui::Frame::new().fill(PANEL_BG).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RunVideo").strong().size(18.0).color(ACCENT));
                    ui.separator();

                    if ui.button("New").on_hover_text("New project (Ctrl+N)").clicked() {
                        self.new_project(&ctx);
                    }
                    if ui.button("Open").on_hover_text("Open project (Ctrl+O)").clicked() {
                        self.open_project(&ctx);
                    }
                    if ui.button("Save").on_hover_text("Save project (Ctrl+S)").clicked() {
                        self.save_project(&ctx, false);
                    }
                    if ui.button("Save As").clicked() {
                        self.save_project(&ctx, true);
                    }
                    if ui
                        .button("Help")
                        .on_hover_text("Shortcuts and help (F1)")
                        .clicked()
                    {
                        self.show_help = true;
                    }
                    if ui
                        .add_enabled(
                            self.ffmpeg_version.is_some() && self.project.total_clips() > 0,
                            egui::Button::new("Capture"),
                        )
                        .on_hover_text("Save the frame at the playhead as a PNG")
                        .clicked()
                    {
                        self.capture_current_frame();
                    }
                    if ui
                        .button(if self.fullscreen { "Windowed" } else { "Fullscreen" })
                        .on_hover_text("Toggle fullscreen preview (F11)")
                        .clicked()
                    {
                        self.toggle_fullscreen(&ctx);
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.ffmpeg_version.is_some(),
                            egui::Button::new("+ Add Media"),
                        )
                        .on_hover_text(format!(
                            "Add video or photos to the selected track ({}). Drop files on a track row to target that track.",
                            self.project.layers[self
                                .selected_layer
                                .min(self.project.layers.len() - 1)]
                            .name
                        ))
                        .clicked()
                    {
                        self.add_media_dialog(&ctx);
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                        .on_hover_text("Undo (Ctrl+Z)")
                        .clicked()
                    {
                        self.undo(&ctx);
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                        .on_hover_text("Redo (Ctrl+Y)")
                        .clicked()
                    {
                        self.redo(&ctx);
                    }

                    ui.separator();

		    if ui
                        .add_enabled(
                            self.ffmpeg_version.is_some() && self.project.duration() > 0.0,
                            egui::Button::new("Export"),
                        )
                        .on_hover_text("Export MP4 (Ctrl+E)")
                        .clicked()
                    {
                        self.export_dialog();
                    }

                    let title = format!(
                        "{}  -  {}x{} @ {} fps  -  {} / {}",
                        self.project.name,
                        self.project.width,
                        self.project.height,
                        self.project.fps,
                        self.project.total_clips(),
                        MAX_CLIPS
                    );
                    ui.add(egui::Label::new(RichText::new(title).small().weak()).truncate());

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match &self.ffmpeg_version {
                            Some(v) => {
                                ui.label(RichText::new("FFmpeg ready").weak().size(11.0))
                                    .on_hover_text(v.clone());
                            }
                            None => {
                                ui.label(
                                    RichText::new("FFmpeg missing")
                                        .color(ERR_COLOR)
                                        .strong()
                                        .size(12.0),
                                );
                            }
                        }
                        if let Some(status) = &self.status {
                            ui.separator();

                            let text = truncate(&status.text, 56);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text)
                                        .small()
                                        .color(if status.error { ERR_COLOR } else { OK_COLOR }),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&status.text);
                        }
                    });
                });
            });
    }

    // ------------------------------------------------------------------
    // Timeline
    // ------------------------------------------------------------------

    fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(340.0)
            .frame(egui::Frame::new().fill(PANEL_BG).inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| {
                self.timeline_ui(ui);
            });
    }

    fn timeline_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Timeline");
            ui.separator();
            ui.label(RichText::new("Zoom").small().weak());
            ui.add(
                egui::Slider::new(&mut self.pps, 8.0..=300.0)
                    .logarithmic(true)
                    .show_value(false),
            )
            .on_hover_text("Timeline zoom");
            ui.separator();
            if ui
                .add_enabled(
                    self.project.layers.len() < MAX_LAYERS,
                    egui::Button::new("+ Track"),
                )
                .on_hover_text("Add another video track (L)")
                .clicked()
            {
                self.add_overlay_layer();
            }
            ui.separator();
            if ui
                .add_enabled(
                    self.project.total_clips() < MAX_CLIPS,
                    egui::Button::new("+ Text"),
                )
                .on_hover_text("Insert a title/caption section at the playhead")
                .clicked()
            {
                self.add_text_section(ui.ctx());
            }
            ui.separator();
            ui.checkbox(&mut self.snapping, "Snap")
                .on_hover_text("Snap seeking and dragging to section edges and the playhead");
            if ui
                .button("Fit")
                .on_hover_text("Zoom the timeline to fit the whole project (Ctrl+Plus)")
                .clicked()
            {
                self.zoom_to_fit();
            }
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "{} sections  -  {}",
                    self.project.total_clips(),
                    fmt_time(self.project.duration())
                ))
                .small()
                .weak(),
            );
            if ui.available_width() > 520.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(
                            "S split - X delete - Ctrl+D duplicate - Ctrl+C/V copy/paste - L/Y new track - Space play - RShift stop",
                        )
                        .small()
                        .weak(),
                    );
                });
            }
        });
        ui.add_space(4.0);

        let pps = self.pps;
        let duration = self.project.duration().max(1.0);
        let position = self.position;
        let selected_clip = self.selected_clip;
        let selected_layer = self.selected_layer;
        let start_marker = self.project.start_marker;
        let drag_clip = self.drag_clip;
        let dragged_name = drag_clip
            .and_then(|id| self.project.find_clip(id))
            .map(|(l, c)| self.project.layers[l].clips[c].name.clone());

        // Immutable snapshots for drawing.
        let layers: Vec<LayerView> = self
            .project
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let origin = l.start.max(0.0);
                let clips = l
                    .clips
                    .iter()
                    .map(|c| ClipView {
                        id: c.id,
                        name: c.name.clone(),
                        path: c.path.clone(),
                        start: origin + c.start.max(0.0),
                        duration: c.duration,
                        kind: c.kind,
                        volume: c.volume,
                        has_audio: c.has_audio,
                    })
                    .collect();
                LayerView {
                    index: i,
                    name: l.name.clone(),
                    visible: l.visible,
                    muted: l.muted,
                    solo: l.solo,
                    locked: l.locked,
                    clips,
                }
            })
            .collect();

        let mut thumb_map: HashMap<PathBuf, TextureHandle> = HashMap::new();
        for lv in &layers {
            for cv in &lv.clips {
                if let Some(Some(tex)) = self.thumbs.get(&cv.path) {
                    thumb_map.entry(cv.path.clone()).or_insert_with(|| tex.clone());
                }
            }
        }

        let area = ui.available_rect_before_wrap();
        if area.width() < 80.0 || area.height() < 50.0 {
            return;
        }
        let (area_rect, _) = ui.allocate_exact_size(area.size(), Sense::hover());
        let painter = ui.painter_at(area_rect);
        painter.rect_filled(area_rect, CornerRadius::ZERO, Color32::from_rgb(17, 17, 21));

        let header_w = ROW_HEADER.min(area_rect.width() * 0.42);
        let content_left = area_rect.left() + header_w;
        let ruler_top = area_rect.top();
        let rows_top = ruler_top + RULER_H;
        let rows_height = (area_rect.bottom() - rows_top).max(1.0);
        let ruler_rect = Rect::from_min_max(area_rect.min, egui::pos2(area_rect.right(), rows_top));
        let content_view = Rect::from_min_max(
            egui::pos2(content_left, rows_top),
            area_rect.max,
        );
        self.timeline_width = area_rect.width();

        let n = layers.len().max(1) as f32;
        let row_h = (rows_height / n).clamp(30.0, 58.0);
        let total_rows_h = row_h * n;
        let max_v = (total_rows_h - rows_height).max(0.0);
        self.v_offset = self.v_offset.clamp(0.0, max_v);

        let span = duration as f32 * pps + 160.0;
        let max_h = (span - content_view.width()).max(0.0);
        self.h_offset = self.h_offset.clamp(0.0, max_h);

        // Wheel: horizontal over the timeline, vertical over the track headers.
        // Ctrl+wheel zooms around the pointer.
        if let Some(p) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            if area_rect.contains(p) {
                let (wheel, shift, ctrl) = ui.ctx().input(|i| {
                    (i.smooth_scroll_delta, i.modifiers.shift, i.modifiers.ctrl || i.modifiers.command)
                });
                if ctrl && wheel.y.abs() > 0.01 && p.x >= content_left {
                    // Keep the time under the cursor fixed while zooming.
                    let t_at = ((p.x - content_left + self.h_offset) / self.pps) as f64;
                    let factor = (wheel.y * 0.004).exp();
                    let new_pps = (self.pps * factor).clamp(8.0, 300.0);
                    self.pps = new_pps;
                    self.h_offset = ((t_at as f32 * new_pps) - (p.x - content_left)).max(0.0);
                } else if p.x >= content_left {
                    let dx = wheel.x + wheel.y;
                    if dx.abs() > 0.01 {
                        self.h_offset = (self.h_offset - dx).clamp(0.0, max_h);
                    }
                } else if wheel.y.abs() > 0.01 {
                    self.v_offset = (self.v_offset - wheel.y).clamp(0.0, max_v);
                } else if shift && wheel.x.abs() > 0.01 {
                    self.v_offset = (self.v_offset - wheel.x).clamp(0.0, max_v);
                }
            }
        }
        let h_off = self.h_offset;
        let v_off = self.v_offset;
        let x_of = |t: f64| content_left - h_off + t as f32 * pps;
        let time_of =
            |x: f32| (((x - content_left + h_off) / pps) as f64).clamp(0.0, duration);

        // ---------------- layout ----------------
        let mut rows: Vec<RowInfo> = Vec::new();
        for (nth, lv) in layers.iter().rev().enumerate() {
            let top = rows_top - v_off + nth as f32 * row_h;
            let row = Rect::from_min_max(
                egui::pos2(content_left, top),
                egui::pos2(area_rect.right(), top + row_h),
            );
            let header = Rect::from_min_max(
                egui::pos2(area_rect.left(), top),
                egui::pos2(content_left, top + row_h),
            );
            let mut clips = Vec::new();
            for cv in &lv.clips {
                let left = x_of(cv.start) + 1.0;
                let right = x_of(cv.start + cv.duration) - 1.0;
                let rect = Rect::from_min_max(
                    egui::pos2(left, row.top() + 3.0),
                    egui::pos2(right.max(left + 6.0), row.bottom() - 3.0),
                );
                clips.push((cv.id, rect));
            }
            rows.push(RowInfo {
                layer: lv.index,
                rect: row,
                header,
                clips,
            });
        }

        // ---------------- interaction ----------------
        let mut seek_to: Option<f64> = None;
        let mut clicked_clip: Option<(u64, usize)> = None;
        let mut clip_menu_action: Option<(u64, &'static str)> = None;
        let mut clip_menu_volume: Option<(u64, f64)> = None;
        let mut trim_start: Option<TrimGesture> = None;
        let mut drag_start: Option<(u64, f64)> = None;
        let mut toggle_visibility: Option<usize> = None;
        let mut rename_start: Option<usize> = None;
        let mut header_delete: Option<usize> = None;
        let mut header_move: Option<(usize, i32)> = None;
        let mut header_add_media: Option<usize> = None;
        let mut layer_drag_start: Option<usize> = None;
        let mut toggle_mute: Option<usize> = None;
        let mut toggle_solo: Option<usize> = None;
        let mut toggle_lock: Option<usize> = None;
        let layer_count = layers.len();
        let mut selected_layer_action: Option<usize> = None;
        let mut moved_marker: Option<f64> = None;
        let mut marker_drag_started = false;

        // Scratch area for seeking (clips are registered afterwards and win the hit test).
        let seek_area = Rect::from_min_max(egui::pos2(content_left, ruler_top), area_rect.max);
        if seek_area.width() > 1.0 && seek_area.height() > 1.0 {
            let resp = ui.interact(
                seek_area,
                ui.make_persistent_id("timeline_seek"),
                Sense::click_and_drag(),
            );
            if resp.clicked() || resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    seek_to = Some(self.snap_time(time_of(p.x), 7.0));
                    if p.y > rows_top {
                        if let Some(r) = rows
                            .iter()
                            .find(|r| p.y >= r.rect.top() && p.y <= r.rect.bottom())
                        {
                            selected_layer_action = Some(r.layer);
                        }
                    }
                }
            }
        }

        // Track headers: select, double-click to rename, eye to show/hide.
        for r in &rows {
            let visible_header = r.header.intersect(area_rect);
            if visible_header.width() < 2.0 || visible_header.height() < 2.0 {
                continue;
            }
            let hresp = ui.interact(
                visible_header,
                ui.make_persistent_id(("track_header", r.layer)),
                Sense::click_and_drag(),
            );
            if hresp.clicked() {
                selected_layer_action = Some(r.layer);
            }
            if hresp.double_clicked() {
                rename_start = Some(r.layer);
            }
            if hresp.drag_started() && r.layer > 0 {
                layer_drag_start = Some(r.layer);
            }
            if hresp.hovered() {
                ui.ctx().set_cursor_icon(if r.layer > 0 {
                    egui::CursorIcon::Grab
                } else {
                    egui::CursorIcon::Default
                });
            }
            hresp.context_menu(|ui| {
                ui.set_min_width(150.0);
                if ui.button("Rename track").clicked() {
                    rename_start = Some(r.layer);
                    ui.close();
                }
                if ui.button("Add media here").clicked() {
                    header_add_media = Some(r.layer);
                    ui.close();
                }
                ui.separator();
                let muted = layers
                    .iter()
                    .find(|l| l.index == r.layer)
                    .map(|l| l.muted)
                    .unwrap_or(false);
                if ui
                    .button(if muted { "Unmute track" } else { "Mute track" })
                    .clicked()
                {
                    toggle_mute = Some(r.layer);
                    ui.close();
                }
                let soloed = layers
                    .iter()
                    .find(|l| l.index == r.layer)
                    .map(|l| l.solo)
                    .unwrap_or(false);
                if ui
                    .button(if soloed { "Unsolo track" } else { "Solo track" })
                    .clicked()
                {
                    toggle_solo = Some(r.layer);
                    ui.close();
                }
                let locked = layers
                    .iter()
                    .find(|l| l.index == r.layer)
                    .map(|l| l.locked)
                    .unwrap_or(false);
                if ui
                    .button(if locked { "Unlock track" } else { "Lock track" })
                    .clicked()
                {
                    toggle_lock = Some(r.layer);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(r.layer + 1 < layer_count, egui::Button::new("Move up"))
                    .clicked()
                {
                    header_move = Some((r.layer, 1));
                    ui.close();
                }
                if ui
                    .add_enabled(r.layer > 0, egui::Button::new("Move down"))
                    .clicked()
                {
                    header_move = Some((r.layer, -1));
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(r.layer > 0, egui::Button::new("Delete track"))
                    .clicked()
                {
                    header_delete = Some(r.layer);
                    ui.close();
                }
            });
            let eye_rect = Rect::from_center_size(
                egui::pos2(r.header.left() + 20.0, r.header.center().y),
                egui::vec2(22.0, 22.0),
            );
            if eye_rect.intersects(area_rect) {
                let eresp = ui.interact(
                    eye_rect,
                    ui.make_persistent_id(("track_eye", r.layer)),
                    Sense::click(),
                );
                if eresp.clicked() {
                    toggle_visibility = Some(r.layer);
                }
                eresp.on_hover_text("Show or hide this track");
            }
            // Mute / solo / lock toggles, right-aligned in the header.
            if r.header.width() >= 110.0 {
                let state = layers.iter().find(|l| l.index == r.layer);
                let muted = state.map(|l| l.muted).unwrap_or(false);
                let soloed = state.map(|l| l.solo).unwrap_or(false);
                let locked = state.map(|l| l.locked).unwrap_or(false);
                for (rect, id, tip) in [
                    (track_hdr_button(r.header, 2.5), "track_mute", "Mute audio"),
                    (track_hdr_button(r.header, 1.5), "track_solo", "Solo (hear only soloed tracks)"),
                    (track_hdr_button(r.header, 0.5), "track_lock", "Lock (block edits)"),
                ] {
                    if !rect.intersects(area_rect) {
                        continue;
                    }
                    let resp = ui.interact(
                        rect,
                        ui.make_persistent_id((id, r.layer)),
                        Sense::click(),
                    );
                    if resp.clicked() {
                        if id == "track_mute" {
                            toggle_mute = Some(r.layer);
                        } else if id == "track_solo" {
                            toggle_solo = Some(r.layer);
                        } else {
                            toggle_lock = Some(r.layer);
                        }
                    }
                    resp.on_hover_text(tip);
                    let _ = (muted, soloed, locked);
                }
            }
        }

        // Clips.
        for r in &rows {
            if r.rect.bottom() < rows_top || r.rect.top() > area_rect.bottom() {
                continue;
            }
            for (id, rect) in &r.clips {
                let hit = rect.intersect(content_view);
                if hit.width() < 2.0 || hit.height() < 2.0 {
                    continue;
                }
                let resp = ui.interact(
                    hit,
                    ui.make_persistent_id(("timeline_clip", *id)),
                    Sense::click_and_drag(),
                );
                if resp.clicked() {
                    clicked_clip = Some((*id, r.layer));
                }
                let row_locked = layers
                    .iter()
                    .find(|l| l.index == r.layer)
                    .map(|l| l.locked)
                    .unwrap_or(false);
                // Edge zones trim instead of moving the section.
                let edge = (rect.width() * 0.22).clamp(4.0, 14.0);
                let pointer_x = ui.ctx().input(|i| i.pointer.hover_pos()).map(|p| p.x);
                let near_left = pointer_x.map(|x| x <= rect.left() + edge).unwrap_or(false);
                let near_right = pointer_x.map(|x| x >= rect.right() - edge).unwrap_or(false);
                if resp.hovered() && !row_locked && (near_left || near_right) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if resp.drag_started() && !row_locked {
                    clicked_clip = Some((*id, r.layer));
                    if near_left || near_right {
                        if let Some((li, ci)) = self.project.find_clip(*id) {
                            let clip = &self.project.layers[li].clips[ci];
                            trim_start = Some(TrimGesture {
                                clip: *id,
                                left: near_left,
                                start_duration: clip.duration,
                                start_in: clip.in_point,
                                start_x: pointer_x.unwrap_or(rect.left()),
                                committed: false,
                            });
                        }
                    } else {
                        // Remember where inside the section the pointer grabbed
                        // it, so free dragging keeps that point under the cursor.
                        let grab = pointer_x
                            .map(|x| ((x - rect.left()) / pps) as f64)
                            .unwrap_or(0.0)
                            .clamp(0.0, rect.width() as f64 / pps as f64);
                        drag_start = Some((*id, grab));
                    }
                }
                let name = layers
                    .iter()
                    .find(|l| l.index == r.layer)
                    .and_then(|l| l.clips.iter().find(|c| c.id == *id))
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                if row_locked {
                    resp.clone()
                        .on_hover_text(format!("{name}\nTrack is locked - unlock it to edit"));
                } else {
                    resp.clone().on_hover_text(format!(
                        "{name}\nDrag to move it freely (hold Alt to ignore snapping); drop on another track to move it there"
                    ));
                }
                resp.context_menu(|ui| {
                    ui.set_min_width(170.0);
                    ui.label(RichText::new(truncate(&name, 24)).strong());
                    ui.separator();
                    if ui.button("Split at playhead (S)").clicked() {
                        clip_menu_action = Some((*id, "split"));
                        ui.close();
                    }
                    if ui.button("Duplicate (Ctrl+D)").clicked() {
                        clip_menu_action = Some((*id, "duplicate"));
                        ui.close();
                    }
                    if ui.button("Copy (Ctrl+C)").clicked() {
                        clip_menu_action = Some((*id, "copy"));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Delete (X)").clicked() {
                        clip_menu_action = Some((*id, "delete"));
                        ui.close();
                    }
                    ui.separator();
                    let mut gain: f32 = layers
                        .iter()
                        .find(|l| l.index == r.layer)
                        .and_then(|l| l.clips.iter().find(|c| c.id == *id))
                        .map(|c| c.volume as f32)
                        .unwrap_or(1.0);
                    ui.label(RichText::new("Volume").small().weak());
                    if ui.add(egui::Slider::new(&mut gain, 0.0..=2.0)).changed() {
                        clip_menu_volume = Some((*id, gain as f64));
                    }
                });
            }
        }

        // Start marker handle (drag to move).
        if let Some(marker) = start_marker {
            let mx = x_of(marker);
            if mx >= content_left - 2.0 && mx <= area_rect.right() + 2.0 {
                let handle = ui.interact(
                    Rect::from_min_max(
                        egui::pos2(mx - 6.0, ruler_top),
                        egui::pos2(mx + 58.0, ruler_top + 16.0),
                    ),
                    ui.make_persistent_id("start_marker_handle"),
                    Sense::click_and_drag(),
                );
                if handle.drag_started() {
                    marker_drag_started = true;
                }
                if (handle.dragged() || handle.clicked()) && !handle.drag_stopped() {
                    if let Some(p) = handle.interact_pointer_pos() {
                        let time = self
                            .snap_time(time_of(p.x), 7.0)
                            .clamp(0.0, (duration - MIN_CLIP).max(0.0));
                        moved_marker = Some(time);
                    }
                }
                handle.on_hover_text("Playback start marker (drag to move, T to clear)");
            }
        }

        let pointer = ui.ctx().input(|i| i.pointer.interact_pos());
        let released = ui.ctx().input(|i| i.pointer.any_released());

        // Track-header drag: reorder layers on release.
        if let Some(layer) = layer_drag_start {
            self.drag_layer = Some(layer);
            self.selected_layer = layer;
        }
        if released {
            if let Some(from) = self.drag_layer.take() {
                if let Some(p) = pointer {
                    if let Some(r) = rows
                        .iter()
                        .find(|r| p.y >= r.rect.top() && p.y <= r.rect.bottom())
                    {
                        let to = r.layer.max(1);
                        if from != to && from > 0 {
                            self.snapshot();
                            if self.project.move_layer_to(from, to) {
                                self.selected_layer = to;
                                self.after_edit();
                                self.set_status("Track order changed.", false);
                            } else {
                                self.undo.pop();
                            }
                        }
                    }
                }
            }
        }

        // Remember which track is under the pointer so file drops land there.
        self.hovered_layer = pointer.and_then(|p| {
            rows.iter()
                .find(|r| p.y >= r.rect.top() && p.y <= r.rect.bottom())
                .map(|r| r.layer)
        });

        if marker_drag_started {
            self.snapshot();
        }
        if let Some(t) = moved_marker {
            self.project.start_marker = Some(t);
            self.dirty = true;
        }
        if let Some(layer) = toggle_visibility {
            self.snapshot();
            if let Some(l) = self.project.layers.get_mut(layer) {
                l.visible = !l.visible;
                let shown = l.visible;
                self.after_edit();
                self.set_status(
                    if shown {
                        "Track shown."
                    } else {
                        "Track hidden from preview and export."
                    },
                    false,
                );
            } else {
                self.undo.pop();
            }
        }
        if let Some(layer) = toggle_mute {
            self.snapshot();
            if let Some(l) = self.project.layers.get_mut(layer) {
                l.muted = !l.muted;
                let muted = l.muted;
                self.after_edit();
                self.set_status(
                    if muted { "Track muted." } else { "Track unmuted." },
                    false,
                );
            } else {
                self.undo.pop();
            }
        }
        if let Some(layer) = toggle_solo {
            self.snapshot();
            if let Some(l) = self.project.layers.get_mut(layer) {
                l.solo = !l.solo;
                let solo = l.solo;
                self.after_edit();
                self.set_status(
                    if solo { "Track soloed." } else { "Track un-soloed." },
                    false,
                );
            } else {
                self.undo.pop();
            }
        }
        if let Some(layer) = toggle_lock {
            self.snapshot();
            if let Some(l) = self.project.layers.get_mut(layer) {
                l.locked = !l.locked;
                let locked = l.locked;
                self.after_edit();
                self.set_status(
                    if locked { "Track locked." } else { "Track unlocked." },
                    false,
                );
            } else {
                self.undo.pop();
            }
        }
        if let Some(layer) = rename_start {
            self.renaming_layer = Some(layer);
            self.rename_buf = self.project.layers[layer].name.clone();
            self.rename_fresh = true;
            self.selected_layer = layer;
        }
        if let Some(layer) = header_add_media {
            self.selected_layer = layer;
            self.add_media_dialog_for(ui.ctx(), layer);
        }
        if let Some((layer, dir)) = header_move {
            self.move_layer(layer, dir);
        }
        if let Some(layer) = header_delete {
            self.remove_layer(layer);
        }
        if let Some(t) = seek_to {
            self.seek(t);
        }
        if let Some((id, layer)) = clicked_clip {
            let additive = ui.ctx().input(|i| i.modifiers.ctrl || i.modifiers.command);
            if additive {
                if self.selected_clips.contains(&id) {
                    self.selected_clips.remove(&id);
                } else {
                    self.selected_clips.insert(id);
                }
            } else {
                self.selected_clips.clear();
                self.selected_clips.insert(id);
            }
            if self.selected_clips.is_empty() {
                self.selected_clip = None;
            } else if self.selected_clips.contains(&id) {
                self.selected_clip = Some(id);
                self.selected_layer = layer;
            } else if let Some(next) = self.selected_clips.iter().next().copied() {
                self.selected_clip = Some(next);
                if let Some((li, _)) = self.project.find_clip(next) {
                    self.selected_layer = li;
                }
            }
        }
        if let Some(layer) = selected_layer_action {
            self.selected_layer = layer;
            if let Some(id) = self.selected_clip {
                if self.project.find_clip(id).map(|(l, _)| l) != Some(layer) {
                    self.selected_clip = None;
                }
            }
        }
        if let Some((id, grab)) = drag_start {
            self.drag_clip = Some(id);
            self.drag_grab = grab;
        }
        if let Some(g) = trim_start {
            self.trim_drag = Some(g);
        }

        // Apply an active trim while the pointer moves.
        if let Some(mut g) = self.trim_drag {
            let dragging = ui.ctx().input(|i| i.pointer.any_down());
            let ptr_x = ui.ctx().input(|i| i.pointer.interact_pos()).map(|p| p.x);
            if dragging {
                if let Some(x) = ptr_x {
                    if (x - g.start_x).abs() > 1.5 && !g.committed {
                        self.snapshot();
                        g.committed = true;
                    }
                    if g.committed {
                        // Work from the gesture's original values so the trim
                        // does not accumulate error across frames.
                        if let Some((li, ci)) = self.project.find_clip(g.clip) {
                            let c = &mut self.project.layers[li].clips[ci];
                            c.duration = g.start_duration;
                            c.in_point = g.start_in;
                        }
                        let dt = ((x - g.start_x) / self.pps) as f64;
                        if g.left {
                            self.project.trim_left(g.clip, dt, MIN_CLIP);
                        } else {
                            self.project.trim_right(g.clip, dt, MIN_CLIP);
                        }
                        self.revision = self.revision.wrapping_add(1);
                        self.dirty = true;
                        self.playing = false;
                        self.last_frame_key = None;
                        self.pending_frame = None;
                        self.gen += 1;
                        self.worker.cancel_older(self.gen);
                        self.preview_inflight = false;
                    }
                }
                self.trim_drag = Some(g);
            } else {
                if g.committed {
                    self.after_edit();
                    self.set_status("Section trimmed.", false);
                }
                self.trim_drag = None;
            }
        }

        // Clip context-menu actions.
        if let Some((id, action)) = clip_menu_action {
            if let Some((li, _)) = self.project.find_clip(id) {
                self.selected_clip = Some(id);
                self.selected_layer = li;
                match action {
                    "split" => self.split(),
                    "duplicate" => self.duplicate_selected(),
                    "copy" => self.copy_selected(),
                    "delete" => self.delete_selected(),
                    _ => {}
                }
            }
        }
        if let Some((id, gain)) = clip_menu_volume {
            if let Some((li, ci)) = self.project.find_clip(id) {
                if self.layer_locked(li) {
                    self.set_status("That track is locked. Unlock it to edit.", true);
                } else {
                    self.snapshot();
                    self.project.layers[li].clips[ci].volume = gain.clamp(0.0, 4.0);
                    self.after_edit();
                    self.set_status(format!("Section volume set to {gain:.2}."), false);
                }
            }
        }

        // Commit a clip drop onto the track under the pointer.
        if released && self.drag_clip.is_some() {
            let dragged = self.drag_clip.unwrap();
            if let Some(p) = pointer {
                if let Some(r) = rows
                    .iter()
                    .find(|r| p.y >= r.rect.top() && p.y <= r.rect.bottom())
                {
                    // Free placement: the left edge follows the pointer, keeping
                    // the grabbed point under the cursor. Snapping applies
                    // unless Alt is held.
                    let snap = ui.ctx().input(|i| !i.modifiers.alt);
                    let raw = time_of(p.x) - self.drag_grab;
                    let start_abs = if snap {
                        self.snap_time(raw, 7.0).max(0.0)
                    } else {
                        raw.max(0.0)
                    };
                    if let Some((from_layer, from_idx)) = self.project.find_clip(dragged) {
                        let layer_origin =
                            self.project.layers.get(r.layer).map(|l| l.start.max(0.0)).unwrap_or(0.0);
                        let start = (start_abs - layer_origin).max(0.0);
                        let current = self.project.layers[from_layer].clips[from_idx].start.max(0.0);
                        let no_op = from_layer == r.layer && (start - current).abs() < 1e-3;
                        if !no_op {
                            self.snapshot();
                            if self.project.move_clip_to(dragged, r.layer, start) {
                                self.selected_layer = r.layer;
                                self.selected_clip = Some(dragged);
                                self.after_edit();
                                self.set_status("Section moved.", false);
                            } else {
                                self.undo.pop();
                            }
                        }
                    }
                }
            }
            self.drag_clip = None;
        }

        // ---------------- paint ----------------
        let content_painter = ui.painter_at(content_view);

        // Ruler.
        painter.rect_filled(ruler_rect, CornerRadius::ZERO, Color32::from_rgb(22, 22, 27));
        painter.line_segment(
            [
                egui::pos2(area_rect.left(), rows_top),
                egui::pos2(area_rect.right(), rows_top),
            ],
            Stroke::new(1.0, Color32::from_rgb(12, 12, 15)),
        );
        painter.text(
            egui::pos2(area_rect.left() + 10.0, ruler_top + 8.0),
            Align2::LEFT_TOP,
            format!("{} tracks", layers.len()),
            FontId::proportional(10.0),
            Color32::from_gray(110),
        );
        let step = choose_tick_step(pps);
        let mut t = ((h_off as f64 / pps as f64) / step).floor() * step;
        if t < 0.0 {
            t = 0.0;
        }
        while t <= duration + step {
            let x = x_of(t);
            if x > area_rect.right() + 1.0 {
                break;
            }
            if x >= content_left - 1.0 {
                painter.line_segment(
                    [
                        egui::pos2(x, rows_top - 7.0),
                        egui::pos2(x, rows_top),
                    ],
                    Stroke::new(1.0, Color32::from_gray(95)),
                );
                painter.text(
                    egui::pos2(x + 3.0, ruler_top + 4.0),
                    Align2::LEFT_TOP,
                    fmt_ruler(t),
                    FontId::monospace(10.0),
                    Color32::from_gray(150),
                );
                content_painter.line_segment(
                    [egui::pos2(x, rows_top), egui::pos2(x, area_rect.bottom())],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 10)),
                );
            }
            t += step;
        }

        for r in &rows {
            if r.rect.bottom() < rows_top || r.rect.top() > area_rect.bottom() {
                continue;
            }
            let lv = match layers.iter().find(|l| l.index == r.layer) {
                Some(l) => l,
                None => continue,
            };
            let selected = lv.index == selected_layer;

            // Header.
            let hbg = if selected { ROW_BG_SELECTED } else { ROW_BG };
            painter.rect_filled(r.header, CornerRadius::ZERO, hbg);
            painter.line_segment(
                [
                    egui::pos2(r.header.left(), r.header.top()),
                    egui::pos2(r.header.right(), r.header.top()),
                ],
                Stroke::new(1.0, Color32::from_rgb(14, 14, 17)),
            );
            if selected {
                painter.rect_filled(
                    Rect::from_min_max(
                        r.header.left_top(),
                        egui::pos2(r.header.left() + 3.0, r.header.bottom()),
                    ),
                    CornerRadius::ZERO,
                    ACCENT,
                );
            }
            let eye_color = if lv.visible {
                Color32::from_gray(215)
            } else {
                Color32::from_gray(105)
            };
            paint_eye(
                &painter,
                Rect::from_center_size(
                    egui::pos2(r.header.left() + 20.0, r.header.center().y),
                    egui::vec2(21.0, 21.0),
                )
                .shrink(3.0),
                lv.visible,
                eye_color,
            );
            // Mute / solo / lock indicator buttons (right side of the header).
            if r.header.width() >= 110.0 {
                let specs = [
                    (2.5, "M", lv.muted, Color32::from_rgb(220, 120, 70)),
                    (1.5, "S", lv.solo, Color32::from_rgb(220, 190, 70)),
                    (0.5, "L", lv.locked, Color32::from_rgb(130, 175, 230)),
                ];
                for (slot, label, on, on_color) in specs {
                    let rect = track_hdr_button(r.header, slot);
                    if !on {
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::monospace(10.0),
                            Color32::from_gray(100),
                        );
                    } else {
                        painter.rect_filled(rect, CornerRadius::same(3), on_color);
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::monospace(10.5),
                            Color32::from_rgb(20, 20, 24),
                        );
                    }
                }
            }
            if self.renaming_layer == Some(lv.index) {
                let edit_rect = Rect::from_min_max(
                    egui::pos2(r.header.left() + 34.0, r.header.top() + 6.0),
                    egui::pos2(r.header.right() - 6.0, r.header.bottom() - 6.0),
                );
                let edit_id = ui.make_persistent_id(("track_rename", lv.index));
                let resp = ui.put(
                    edit_rect,
                    egui::TextEdit::singleline(&mut self.rename_buf)
                        .id(edit_id)
                        .font(FontId::proportional(12.5))
                        .desired_width(edit_rect.width()),
                );
                if self.rename_fresh {
                    self.rename_fresh = false;
                    resp.request_focus();
                    let mut state =
                        egui::text_edit::TextEditState::default();
                    state.cursor.set_char_range(Some(
                        egui::text::CCursorRange::select_all_str(&self.rename_buf),
                    ));
                    state.store(ui.ctx(), edit_id);
                }
                let escape = ui.ctx().input(|i| i.key_pressed(Key::Escape));
                let enter = ui.ctx().input(|i| i.key_pressed(Key::Enter));
                if escape {
                    self.renaming_layer = None;
                } else if enter || resp.lost_focus() {
                    if !self.rename_buf.trim().is_empty() {
                        let name = self.rename_buf.trim().to_owned();
                        self.project.layers[lv.index].name = name;
                        self.after_edit();
                    }
                    self.renaming_layer = None;
                }
            } else {
                let name = lv.name.clone();
                // Leave room for the M/S/L toggles on the right of the header.
                let name_right = if r.header.width() >= 110.0 {
                    r.header.right() - 5.0 - 18.0 * 3.0 - 2.0 * 2.0 - 6.0
                } else {
                    r.header.right() - 6.0
                };
                let name_chars = (((name_right - (r.header.left() + 34.0)) / 7.0).floor())
                    .clamp(4.0, 24.0) as usize;
                painter.text(
                    egui::pos2(r.header.left() + 34.0, r.header.center().y),
                    Align2::LEFT_CENTER,
                    truncate(&name, name_chars),
                    FontId::proportional(12.5),
                    if !lv.visible {
                        Color32::from_gray(120)
                    } else if selected {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(210)
                    },
                );
            }

            // Row background.
            let rbg = if selected {
                ROW_BG_SELECTED.gamma_multiply(0.55)
            } else if lv.index % 2 == 0 {
                ROW_BG
            } else {
                ROW_BG_ALT
            };
            content_painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(content_left, r.rect.top()),
                    egui::pos2(area_rect.right(), r.rect.bottom()),
                ),
                CornerRadius::ZERO,
                rbg,
            );
            // Locked tracks get a subtle hatch so it is obvious edits are blocked.
            if lv.locked {
                let lock_rect = Rect::from_min_max(
                    egui::pos2(content_left, r.rect.top()),
                    egui::pos2(area_rect.right(), r.rect.bottom()),
                );
                content_painter.rect_filled(
                    lock_rect,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(12, 12, 16, 90),
                );
            }
            content_painter.line_segment(
                [
                    egui::pos2(content_left, r.rect.bottom()),
                    egui::pos2(area_rect.right(), r.rect.bottom()),
                ],
                Stroke::new(1.0, Color32::from_rgb(14, 14, 17)),
            );

            if lv.clips.is_empty() {
                content_painter.text(
                    egui::pos2(content_left + 12.0, r.rect.center().y),
                    Align2::LEFT_CENTER,
                    "Drop media here",
                    FontId::proportional(11.0),
                    Color32::from_gray(105),
                );
            }

            for cv in &lv.clips {
                let left = x_of(cv.start) + 1.0;
                let right = x_of(cv.start + cv.duration) - 1.0;
                if right < content_left || left > area_rect.right() {
                    continue;
                }
                let rect = Rect::from_min_max(
                    egui::pos2(left, r.rect.top() + 3.0),
                    egui::pos2(right.max(left + 6.0), r.rect.bottom() - 3.0),
                );
                let is_selected = Some(cv.id) == selected_clip
                    || self.selected_clips.contains(&cv.id);
                let base = match cv.kind {
                    MediaKind::Video => VIDEO_BG,
                    MediaKind::Image => IMAGE_BG,
                    MediaKind::Text => TEXT_BG,
                };
                let base = if !lv.visible {
                    base.gamma_multiply(0.35)
                } else if is_selected {
                    base
                } else {
                    base.gamma_multiply(0.85)
                };
                content_painter.rect_filled(rect, CornerRadius::same(4), base);
                content_painter.rect_stroke(
                    rect,
                    CornerRadius::same(4),
                    if is_selected {
                        Stroke::new(2.0, ACCENT)
                    } else {
                        Stroke::new(1.0, Color32::from_black_alpha(90))
                    },
                    StrokeKind::Inside,
                );

                let clipped = content_painter.with_clip_rect(rect.shrink(1.0));
                // Audio waveform (cached per media file).
                if cv.kind.is_video() && cv.has_audio {
                    if let Some(peaks) = self.waveforms.get(&cv.path) {
                        if !peaks.is_empty() {
                            let inner = rect.shrink2(egui::vec2(3.0, 5.0));
                            let mid = inner.center().y;
                            let amp = inner.height() * 0.44;
                            let n = inner.width().max(1.0) as usize;
                            let color = if lv.visible {
                                Color32::from_rgba_unmultiplied(210, 235, 255, 150)
                            } else {
                                Color32::from_rgba_unmultiplied(160, 160, 170, 90)
                            };
                            for i in 0..n {
                                let idx = (i * peaks.len() / n).min(peaks.len() - 1);
                                let v = peaks[idx].clamp(0.0, 1.0);
                                let x = inner.left() + i as f32;
                                clipped.line_segment(
                                    [
                                        egui::pos2(x, mid - v * amp),
                                        egui::pos2(x, mid + v * amp),
                                    ],
                                    Stroke::new(1.0, color),
                                );
                            }
                        }
                    }
                }
                let mut text_left = rect.left() + 6.0;
                if let Some(tex) = thumb_map.get(&cv.path) {
                    if rect.width() > 62.0 {
                        let th = (rect.height() - 6.0).clamp(12.0, 30.0);
                        let thumb_rect = Rect::from_min_size(
                            egui::pos2(rect.left() + 4.0, rect.center().y - th / 2.0),
                            egui::vec2(th * 1.4, th),
                        );
                        clipped.image(
                            tex.id(),
                            thumb_rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            if lv.visible {
                                Color32::WHITE
                            } else {
                                Color32::from_gray(120)
                            },
                        );
                        text_left = thumb_rect.right() + 5.0;
                    }
                }
                let name_room = ((rect.right() - text_left - 8.0) / 6.2).max(2.0) as usize;
                clipped.text(
                    egui::pos2(text_left, rect.center().y),
                    Align2::LEFT_CENTER,
                    truncate(&cv.name, name_room),
                    FontId::proportional(11.0),
                    if lv.visible {
                        Color32::from_rgb(235, 235, 240)
                    } else {
                        Color32::from_gray(140)
                    },
                );
                if rect.width() > 110.0 {
                    clipped.text(
                        egui::pos2(rect.right() - 5.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        format!("{:.1}s", cv.duration),
                        FontId::monospace(9.0),
                        Color32::from_rgb(225, 225, 230).gamma_multiply(0.75),
                    );
                }
            }
        }

        // Layer drag: highlight the row being reordered.
        if let Some(from) = self.drag_layer {
            if let Some(p) = pointer {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(row) = rows
                    .iter()
                    .find(|r| p.y >= r.rect.top() && p.y <= r.rect.bottom())
                {
                    painter.rect_filled(
                        Rect::from_min_max(
                            egui::pos2(area_rect.left(), row.rect.top()),
                            egui::pos2(area_rect.right(), row.rect.bottom()),
                        ),
                        CornerRadius::ZERO,
                        Color32::from_rgba_unmultiplied(255, 166, 48, 26),
                    );
                    painter.line_segment(
                        [
                            egui::pos2(area_rect.left(), row.rect.top()),
                            egui::pos2(area_rect.right(), row.rect.top()),
                        ],
                        Stroke::new(2.0, ACCENT),
                    );
                }
                if let Some(lv) = layers.iter().find(|l| l.index == from) {
                    let label = truncate(&lv.name, 24);
                    let galley = painter.layout_no_wrap(
                        label,
                        FontId::proportional(12.0),
                        Color32::WHITE,
                    );
                    let pos = egui::pos2(p.x + 14.0, p.y - 18.0);
                    let ghost = Rect::from_min_size(
                        egui::pos2(pos.x - 6.0, pos.y - 4.0),
                        egui::vec2(galley.size().x + 12.0, galley.size().y + 8.0),
                    );
                    painter.rect_filled(
                        ghost,
                        CornerRadius::same(4),
                        Color32::from_rgba_unmultiplied(30, 34, 44, 235),
                    );
                    painter.rect_stroke(
                        ghost,
                        CornerRadius::same(4),
                        Stroke::new(1.0, ACCENT),
                        StrokeKind::Inside,
                    );
                    painter.galley(pos, galley, Color32::WHITE);
                }
            }
        }

        // Free timeline markers.
        for m in &self.project.markers {
            let mx = x_of(*m);
            if mx < content_left - 4.0 || mx > area_rect.right() + 4.0 {
                continue;
            }
            let c = Color32::from_rgb(240, 196, 92);
            content_painter.line_segment(
                [egui::pos2(mx, rows_top), egui::pos2(mx, area_rect.bottom())],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(240, 196, 92, 90)),
            );
            let cy = ruler_top + RULER_H - 3.0;
            painter.add(Shape::convex_polygon(
                vec![
                    egui::pos2(mx - 4.0, cy),
                    egui::pos2(mx, cy - 5.0),
                    egui::pos2(mx + 4.0, cy),
                    egui::pos2(mx, cy + 4.0),
                ],
                c,
                Stroke::NONE,
            ));
        }

        // Start marker.
        if let Some(marker) = self.project.start_marker {
            let mx = x_of(marker);
            if mx >= content_left - 2.0 && mx <= area_rect.right() + 2.0 {
                let bottom = area_rect.bottom();
                let marker_color = Color32::from_rgb(96, 214, 156);
                content_painter.line_segment(
                    [egui::pos2(mx, rows_top), egui::pos2(mx, bottom)],
                    Stroke::new(2.0, marker_color),
                );
                let flag = Rect::from_min_size(egui::pos2(mx, ruler_top), egui::vec2(54.0, 14.0));
                painter.rect_filled(flag, CornerRadius::same(3), marker_color);
                painter.text(
                    egui::pos2(flag.left() + 4.0, flag.center().y),
                    Align2::LEFT_CENTER,
                    "START",
                    FontId::proportional(9.5),
                    Color32::from_rgb(12, 30, 20),
                );
            }
        }

        // Playhead.
        let px = x_of(position);
        if px >= content_left - 2.0 && px <= area_rect.right() + 2.0 {
            let top = ruler_top;
            let bottom = area_rect.bottom();
            content_painter.line_segment(
                [egui::pos2(px, rows_top), egui::pos2(px, bottom)],
                Stroke::new(1.5, Color32::from_rgb(255, 92, 92)),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    egui::pos2(px - 5.0, top),
                    egui::pos2(px + 5.0, top),
                    egui::pos2(px, top + 8.0),
                ],
                Color32::from_rgb(255, 92, 92),
                Stroke::NONE,
            ));
        }

        // Drag preview: a translucent outline where the section will land.
        if let Some(dragged) = self.drag_clip {
            if let Some(p) = pointer {
                if let Some(row) = rows
                    .iter()
                    .find(|r| p.y >= r.rect.top() && p.y <= r.rect.bottom())
                {
                    let dur = self
                        .project
                        .find_clip(dragged)
                        .map(|(li, ci)| self.project.layers[li].clips[ci].duration)
                        .unwrap_or(1.0);
                    let origin = self
                        .project
                        .layers
                        .get(row.layer)
                        .map(|l| l.start.max(0.0))
                        .unwrap_or(0.0);
                    let snap = ui.ctx().input(|i| !i.modifiers.alt);
                    let raw = time_of(p.x) - self.drag_grab;
                    let start_abs = (if snap {
                        self.snap_time(raw, 7.0)
                    } else {
                        raw
                    })
                    .max(origin);
                    let left = x_of(start_abs).max(content_left);
                    let right = (x_of(start_abs + dur) - 1.0).max(left + 6.0);
                    let preview = Rect::from_min_max(
                        egui::pos2(left, row.rect.top() + 3.0),
                        egui::pos2(right.min(area_rect.right()), row.rect.bottom() - 3.0),
                    );
                    content_painter.rect_filled(
                        preview.intersect(content_view),
                        CornerRadius::same(4),
                        Color32::from_rgba_unmultiplied(120, 170, 255, 46),
                    );
                    content_painter.rect_stroke(
                        preview.intersect(content_view),
                        CornerRadius::same(4),
                        Stroke::new(2.0, ACCENT),
                        StrokeKind::Inside,
                    );
                }
                let label = dragged_name.clone().unwrap_or_else(|| "Section".to_owned());
                let text_pos = egui::pos2(p.x + 14.0, p.y - 18.0);
                let galley = painter.layout_no_wrap(
                    truncate(&label, 26),
                    FontId::proportional(12.0),
                    Color32::WHITE,
                );
                let ghost = Rect::from_min_size(
                    egui::pos2(text_pos.x - 6.0, text_pos.y - 4.0),
                    egui::vec2(galley.size().x + 12.0, galley.size().y + 8.0),
                );
                painter.rect_filled(
                    ghost,
                    CornerRadius::same(4),
                    Color32::from_rgba_unmultiplied(30, 34, 44, 235),
                );
                painter.rect_stroke(
                    ghost,
                    CornerRadius::same(4),
                    Stroke::new(1.0, ACCENT),
                    StrokeKind::Inside,
                );
                painter.galley(text_pos, galley, Color32::WHITE);
            }
        }
    }

    // ------------------------------------------------------------------
    // Right panel: section / layer / project inspector
    // ------------------------------------------------------------------

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::right("right_panel")
            .resizable(true)
            .default_size(320.0)
            .frame(egui::Frame::new().fill(PANEL_BG).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.panel_tab, PanelTab::Section, "Section");
                    ui.selectable_value(&mut self.panel_tab, PanelTab::Layer, "Layer");
                    ui.selectable_value(&mut self.panel_tab, PanelTab::Project, "Project");
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.panel_tab {
                        PanelTab::Section => self.inspector_ui(&ctx, ui),
                        PanelTab::Layer => self.layer_tab_ui(&ctx, ui),
                        PanelTab::Project => self.project_ui(ui),
                    });
            });
    }


    fn layer_tab_ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let li = self.selected_layer.min(self.project.layers.len().saturating_sub(1));
        let layer = &self.project.layers[li];
        ui.horizontal(|ui| {
            ui.label(RichText::new(&layer.name).strong());
            ui.label(
                RichText::new(format!("{} clips", layer.clips.len()))
                    .small()
                    .weak(),
            );
        });
        ui.label(
            RichText::new(if li == 0 {
                "The base layer is the background canvas."
            } else {
                "This layer is composited over the layers below it."
            })
            .small()
            .weak(),
        );
        ui.add_space(6.0);
        if ui
            .button(format!("+ Add media to {}", self.project.layers[li].name))
            .on_hover_text("Add video or photos to this layer")
            .clicked()
        {
            self.add_media_dialog_for(ctx, li);
        }
        ui.add_space(6.0);
        if ui
            .button("+ Text section here")
            .on_hover_text("Insert a title/caption section at the playhead on this track")
            .clicked()
        {
            self.selected_layer = li;
            self.add_text_section(ctx);
        }
        ui.add_space(8.0);
        // Track state toggles (kept in sync with the timeline headers).
        ui.label(RichText::new("Track").small().strong());
        let mut track_change = false;
        ui.horizontal(|ui| {
            let mut visible = self.project.layers[li].visible;
            if ui
                .checkbox(&mut visible, "Visible")
                .on_hover_text("Show this track in the preview and export")
                .changed()
            {
                self.project.layers[li].visible = visible;
                track_change = true;
            }
            let has_audio = self.project.layers[li]
                .clips
                .iter()
                .any(|c| c.has_audio);
            let mut muted = self.project.layers[li].muted;
            if ui
                .add_enabled(has_audio, egui::Checkbox::new(&mut muted, "Mute"))
                .on_hover_text("Silence this track's audio")
                .changed()
            {
                self.project.layers[li].muted = muted;
                track_change = true;
            }
            let mut solo = self.project.layers[li].solo;
            if ui
                .add_enabled(has_audio, egui::Checkbox::new(&mut solo, "Solo"))
                .on_hover_text("When any track is soloed, only soloed tracks are heard")
                .changed()
            {
                self.project.layers[li].solo = solo;
                track_change = true;
            }
            let mut locked = self.project.layers[li].locked;
            if ui
                .checkbox(&mut locked, "Lock")
                .on_hover_text("Block edits and drags on this track")
                .changed()
            {
                self.project.layers[li].locked = locked;
                track_change = true;
            }
        });
        if track_change {
            self.after_edit();
        }
        ui.add_space(6.0);
        // Whole-track audio gain, applied on top of each section's own volume.
        let has_audio = self.project.layers[li].clips.iter().any(|c| c.has_audio);
        let mut gain = self.project.layers[li].gain as f32;
        if ui
            .add_enabled(
                has_audio,
                egui::Slider::new(&mut gain, 0.0..=4.0)
                    .text("Track gain")
                    .fixed_decimals(2),
            )
            .on_hover_text("Applies to every section on this track")
            .changed()
        {
            self.project.layers[li].gain = gain as f64;
            self.after_edit();
        }
        ui.add_space(6.0);
        self.layer_start_ui(ui);
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Position, scale and opacity belong to each section, not to the track. \
                 Select a section (or move the playhead onto one) to edit them in the \
                 Section tab, or drag it directly in the preview.",
            )
            .small()
            .weak(),
        );
    }

    /// Track start offset. The tracks themselves no longer carry a transform.
    fn layer_start_ui(&mut self, ui: &mut egui::Ui) {
        let li = self.selected_layer.min(self.project.layers.len().saturating_sub(1));
        let total = self.project.duration().max(60.0);
        let mut layer_start = self.project.layers[li].start;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Track start");
            let resp = ui
                .add(
                    egui::DragValue::new(&mut layer_start)
                        .range(0.0..=total)
                        .speed(0.05)
                        .suffix(" s"),
                )
                .on_hover_text("When this track first appears on the timeline");
            if resp.changed() {
                self.project.layers[li].start = layer_start;
                changed = true;
            }
        });
        ui.label(
            RichText::new("Delays every section on this track. The playhead position is unaffected.")
                .small()
                .weak(),
        );
        if changed {
            self.after_edit();
        }
    }

    fn inspector_ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let layer = self.selected_layer.min(self.project.layers.len().saturating_sub(1));
        let Some(id) = self.selected_clip else {
            ui.label(
                RichText::new("No section selected").strong(),
            );
            ui.label(
                RichText::new("Click a section on the timeline to trim it, split it, or change its length.")
                    .small()
                    .weak(),
            );
            ui.add_space(6.0);
            if ui
                .button(format!("+ Add media to {}", self.project.layers[layer].name))
                .clicked()
            {
                self.add_media_dialog_for(ctx, layer);
            }
            // The section under the playhead can still be transformed even when
            // nothing was clicked explicitly.
            self.section_transform_ui(ui);
            return;
        };
        let Some((li, ci)) = self.project.find_clip(id) else {
            self.selected_clip = None;
            return;
        };

        let kind = self.project.layers[li].clips[ci].kind;
        let name = self.project.layers[li].clips[ci].name.clone();
        let path = self.project.layers[li].clips[ci].path.clone();
        let has_audio = self.project.layers[li].clips[ci].has_audio;
        ui.horizontal(|ui| {
            ui.label(RichText::new(truncate(&name, 28)).strong());
            let (tag, color) = match kind {
                MediaKind::Video => ("VIDEO", VIDEO_BG_HOVER),
                MediaKind::Image => ("PHOTO", ACCENT),
                MediaKind::Text => ("TEXT", TEXT_BG),
            };
            ui.label(RichText::new(tag).small().color(color));
        });
        if !kind.is_text() {
            ui.add(
                egui::Label::new(RichText::new(path.display().to_string()).small().weak())
                    .truncate(),
            )
            .on_hover_text(path.display().to_string());
        }
        if kind == MediaKind::Video {
            ui.label(
                RichText::new(format!(
                    "Audio: {}",
                    if has_audio { "yes (mixed on export)" } else { "none" }
                ))
                .small()
                .weak(),
            );
        }

        let mut changed = false;
        let layer_origin = self.project.layers[li].start.max(0.0);
        let mut new_start: Option<f64> = None;
        let mut want_silence = false;
        {
            let clip = &mut self.project.layers[li].clips[ci];
            // ---- Position on the timeline ----------------------------------
            // Sections are placed freely, so their start can be typed in or
            // dragged here as well as on the timeline.
            ui.horizontal(|ui| {
                ui.label("Start");
                let mut start_abs = layer_origin + clip.start.max(0.0);
                if ui
                    .add(
                        egui::DragValue::new(&mut start_abs)
                            .range(0.0..=3600.0)
                            .speed(0.02)
                            .suffix(" s"),
                    )
                    .on_hover_text("Start of this section on the timeline")
                    .changed()
                {
                    new_start = Some((start_abs - layer_origin).max(0.0));
                }
            });
            if kind.is_text() {
                // ---- Text section -------------------------------------------
                let mut spec = clip.text.clone().unwrap_or_else(|| TextSpec::new("Text"));
                let mut text_changed = false;
                text_changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut spec.content)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("Caption text"),
                    )
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("Size");
                    text_changed |= ui
                        .add(
                            egui::DragValue::new(&mut spec.size)
                                .range(8.0..=400.0)
                                .speed(0.5),
                        )
                        .changed();
                    ui.checkbox(&mut spec.bold, "Bold");
                });
                ui.horizontal(|ui| {
                    ui.label("Colour");
                    text_changed |= color_edit(ui, &mut spec.color);
                    ui.label("Box");
                    let mut boxed = spec.background.is_some();
                    if ui.checkbox(&mut boxed, "").changed() {
                        spec.background = if boxed {
                            Some("#000000".to_owned())
                        } else {
                            None
                        };
                        text_changed = true;
                    }
                    if let Some(bg) = &mut spec.background {
                        text_changed |= color_edit(ui, bg);
                    }
                });
                ui.add_space(2.0);
                ui.label(RichText::new("Style").small().strong());
                ui.horizontal(|ui| {
                    ui.label("Outline");
                    text_changed |= ui
                        .add(
                            egui::DragValue::new(&mut spec.outline)
                                .range(0.0..=80.0)
                                .speed(0.2),
                        )
                        .changed();
                    if spec.outline > 0.01 {
                        text_changed |= color_edit(ui, &mut spec.outline_color);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Shadow");
                    text_changed |= ui
                        .add(
                            egui::DragValue::new(&mut spec.shadow)
                                .range(0.0..=80.0)
                                .speed(0.2),
                        )
                        .changed();
                    if spec.shadow > 0.01 {
                        text_changed |= color_edit(ui, &mut spec.shadow_color);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Align");
                    egui::ComboBox::from_id_salt(("text_align", clip.id))
                        .selected_text(format!("{:?} / {:?}", spec.align, spec.valign))
                        .show_ui(ui, |ui| {
                            for a in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
                                for v in [VAlign::Top, VAlign::Middle, VAlign::Bottom] {
                                    if ui
                                        .selectable_label(
                                            spec.align == a && spec.valign == v,
                                            format!("{a:?} / {v:?}"),
                                        )
                                        .clicked()
                                    {
                                        spec.align = a;
                                        spec.valign = v;
                                        text_changed = true;
                                    }
                                }
                            }
                        });
                });
                ui.label(RichText::new("Title templates").small().strong());
                ui.horizontal_wrapped(|ui| {
                    for (label, size, outline, shadow, align, valign, boxed, bold) in [
                        ("Title", 96.0, 4.0, 8.0, TextAlign::Center, VAlign::Middle, false, true),
                        ("Lower third", 46.0, 2.0, 3.0, TextAlign::Left, VAlign::Bottom, true, true),
                        ("Caption", 40.0, 3.0, 3.0, TextAlign::Center, VAlign::Bottom, false, true),
                        ("Big centre", 130.0, 6.0, 10.0, TextAlign::Center, VAlign::Middle, false, true),
                        ("Top banner", 54.0, 2.0, 3.0, TextAlign::Center, VAlign::Top, true, true),
                    ] {
                        if ui
                            .add(egui::Button::new(label).small())
                            .on_hover_text("Apply a ready-made title style")
                            .clicked()
                        {
                            spec.size = size;
                            spec.outline = outline;
                            spec.shadow = shadow;
                            spec.align = align;
                            spec.valign = valign;
                            spec.bold = bold;
                            spec.color = "#ffffff".to_owned();
                            spec.background = if boxed {
                                Some("#000000".to_owned())
                            } else {
                                None
                            };
                            text_changed = true;
                        }
                    }
                });
                if text_changed {
                    clip.text = Some(spec);
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Length");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut clip.duration)
                                .range(MIN_CLIP..=3600.0)
                                .speed(0.05)
                                .suffix(" s"),
                        )
                        .changed();
                });
                clip.in_point = 0.0;
            } else if kind == MediaKind::Video {
                ui.horizontal(|ui| {
                    ui.label("Source in");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut clip.in_point)
                                .range(0.0..=clip.source_duration.map(|d| d.max(0.0)).unwrap_or(3600.0))
                                .speed(0.02)
                                .suffix(" s"),
                        )
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Length");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut clip.duration)
                                .range(MIN_CLIP..=3600.0)
                                .speed(0.02)
                                .suffix(" s"),
                        )
                        .changed();
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label("Length");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut clip.duration)
                                .range(MIN_CLIP..=3600.0)
                                .speed(0.05)
                                .suffix(" s"),
                        )
                        .changed();
                });
                clip.in_point = 0.0;
            }

            // ---- Speed ------------------------------------------------------
            ui.horizontal(|ui| {
                ui.label("Speed");
                let mut speed = clip.speed;
                let resp = ui
                    .add(
                        egui::DragValue::new(&mut speed)
                            .range(0.05..=8.0)
                            .speed(0.01)
                            .suffix("x"),
                    )
                    .on_hover_text(
                        "Playback rate of this section. 2x uses twice as much source in the same length (clamped to the source available).",
                    );
                if resp.changed() {
                    clip.speed = speed.clamp(0.05, 8.0);
                    changed = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                for preset in [0.25f64, 0.5, 0.75, 1.0, 1.15, 1.25, 1.5, 2.0, 4.0] {
                    if ui
                        .add(egui::Button::new(format!("{preset}x")).small())
                        .on_hover_text("Set this section's playback speed")
                        .clicked()
                    {
                        clip.speed = preset;
                        changed = true;
                    }
                }
            });
            if kind == MediaKind::Video {
                ui.horizontal(|ui| {
                    let mut ramped = clip.speed_end.is_some();
                    if ui
                        .checkbox(&mut ramped, "Speed ramp to")
                        .on_hover_text("Linearly change speed from this section's start rate to the end rate")
                        .changed()
                    {
                        clip.speed_end = if ramped { Some(clip.speed) } else { None };
                        changed = true;
                    }
                    if let Some(end) = clip.speed_end {
                        let mut end = end;
                        if ui
                            .add(
                                egui::DragValue::new(&mut end)
                                    .range(0.05..=8.0)
                                    .speed(0.01)
                                    .suffix("x"),
                            )
                            .changed()
                        {
                            clip.speed_end = Some(end.clamp(0.05, 8.0));
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    changed |= ui
                        .add_enabled(
                            !clip.freeze,
                            egui::Checkbox::new(&mut clip.reverse, "Reverse"),
                        )
                        .on_hover_text("Play this section backwards (video and audio)")
                        .changed();
                    if ui
                        .checkbox(&mut clip.freeze, "Freeze frame")
                        .on_hover_text("Hold the first frame of the section for its whole length")
                        .changed()
                    {
                        if clip.freeze {
                            clip.reverse = false;
                            clip.speed_end = None;
                        }
                        changed = true;
                    }
                });
                if has_audio && !clip.freeze {
                    if ui
                        .button("Cut silence")
                        .on_hover_text("Detect quiet passages and keep only the audible parts")
                        .clicked()
                    {
                        want_silence = true;
                    }
                }
            }

            // ---- Audio (video only) -----------------------------------------
            if kind == MediaKind::Video && has_audio {
                ui.add_space(2.0);
                ui.label(RichText::new("Audio").small().strong());
                let mut vol = clip.volume;
                if ui
                    .add(
                        egui::Slider::new(&mut vol, 0.0..=2.0)
                            .text("Volume")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    clip.volume = vol.clamp(0.0, 4.0);
                    changed = true;
                }
                let max_fade = clip.duration.max(0.01);
                ui.horizontal(|ui| {
                    ui.label("Fade in");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut clip.fade_in)
                                .range(0.0..=max_fade)
                                .speed(0.01)
                                .suffix(" s"),
                        )
                        .changed();
                    ui.label("out");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut clip.fade_out)
                                .range(0.0..=max_fade)
                                .speed(0.01)
                                .suffix(" s"),
                        )
                        .changed();
                });
            }

            // ---- Adjustments (colour / blur / flip) --------------------------
            if !kind.is_text() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Adjust").small().strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("Reset")
                            .on_hover_text("Reset colour, blur and flip for this section")
                            .clicked()
                        {
                            clip.brightness = 0.0;
                            clip.contrast = 1.0;
                            clip.saturation = 1.0;
                            clip.blur = 0.0;
                            clip.rotation = 0.0;
                            clip.crop_l = 0.0;
                            clip.crop_r = 0.0;
                            clip.crop_t = 0.0;
                            clip.crop_b = 0.0;
                            clip.pixelate = 0.0;
                            clip.sharpen = 0.0;
                            clip.vignette = 0.0;
                            clip.flip_h = false;
                            clip.flip_v = false;
                            changed = true;
                        }
                    });
                });
                let mut bright = clip.brightness as f32;
                if ui
                    .add(egui::Slider::new(&mut bright, -1.0..=1.0).text("Brightness"))
                    .changed()
                {
                    clip.brightness = bright as f64;
                    changed = true;
                }
                let mut contrast = clip.contrast as f32;
                if ui
                    .add(egui::Slider::new(&mut contrast, 0.0..=3.0).text("Contrast"))
                    .changed()
                {
                    clip.contrast = contrast as f64;
                    changed = true;
                }
                let mut sat = clip.saturation as f32;
                if ui
                    .add(egui::Slider::new(&mut sat, 0.0..=3.0).text("Saturation"))
                    .changed()
                {
                    clip.saturation = sat as f64;
                    changed = true;
                }
                let mut blur = clip.blur as f32;
                if ui
                    .add(egui::Slider::new(&mut blur, 0.0..=50.0).text("Blur"))
                    .changed()
                {
                    clip.blur = blur as f64;
                    changed = true;
                }
                let mut rot = clip.rotation as f32;
                if ui
                    .add(
                        egui::Slider::new(&mut rot, -180.0..=180.0)
                            .text("Rotation")
                            .suffix(" deg"),
                    )
                    .changed()
                {
                    clip.rotation = rot as f64;
                    changed = true;
                }
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut clip.flip_h, "Flip H").changed();
                    changed |= ui.checkbox(&mut clip.flip_v, "Flip V").changed();
                });
                // ---- Crop (fractions of the source frame) -------------------
                ui.add_space(2.0);
                ui.label(RichText::new("Crop").small().strong());
                let mut cl = clip.crop_l as f32;
                let mut cr = clip.crop_r as f32;
                if ui
                    .add(egui::Slider::new(&mut cl, 0.0..=0.45).text("Left"))
                    .changed()
                {
                    clip.crop_l = cl as f64;
                    changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut cr, 0.0..=0.45).text("Right"))
                    .changed()
                {
                    clip.crop_r = cr as f64;
                    changed = true;
                }
                let mut ct = clip.crop_t as f32;
                let mut cb = clip.crop_b as f32;
                if ui
                    .add(egui::Slider::new(&mut ct, 0.0..=0.45).text("Top"))
                    .changed()
                {
                    clip.crop_t = ct as f64;
                    changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut cb, 0.0..=0.45).text("Bottom"))
                    .changed()
                {
                    clip.crop_b = cb as f64;
                    changed = true;
                }
                // ---- Stylise -------------------------------------------------
                ui.add_space(2.0);
                ui.label(RichText::new("Stylise").small().strong());
                let mut pix = clip.pixelate as f32;
                if ui
                    .add(
                        egui::Slider::new(&mut pix, 0.0..=64.0)
                            .text("Pixelate")
                            .suffix(" px"),
                    )
                    .on_hover_text("Block size; 0 turns pixelation off")
                    .changed()
                {
                    clip.pixelate = pix as f64;
                    changed = true;
                }
                let mut sharp = clip.sharpen as f32;
                if ui
                    .add(egui::Slider::new(&mut sharp, 0.0..=1.5).text("Sharpen"))
                    .changed()
                {
                    clip.sharpen = sharp as f64;
                    changed = true;
                }
                let mut vig = clip.vignette as f32;
                if ui
                    .add(egui::Slider::new(&mut vig, 0.0..=1.0).text("Vignette"))
                    .changed()
                {
                    clip.vignette = vig as f64;
                    changed = true;
                }
            }

            // ---- Trim guardrails --------------------------------------------
            clip.in_point = clip.in_point.max(0.0);
            if let Some(sd) = clip.source_duration {
                let max_in = (sd - MIN_CLIP).max(0.0);
                clip.in_point = clip.in_point.min(max_in);
                let max_dur = ((sd - clip.in_point) / clip.speed.max(0.05)).max(MIN_CLIP);
                clip.duration = clip.duration.clamp(MIN_CLIP, max_dur);
            }
            clip.fade_in = clip.fade_in.clamp(0.0, clip.duration);
            clip.fade_out = clip.fade_out.clamp(0.0, clip.duration);
        }
        if let Some(start) = new_start {
            self.project.layers[li].clips[ci].start = start;
            changed = true;
        }
        if want_silence {
            self.cut_silence_selected();
        }

        ui.horizontal(|ui| {
            let mut split = false;
            let mut delete = false;
            let mut left = false;
            let mut right = false;
            if ui.button("Split (S)").clicked() {
                split = true;
            }
            if ui.button("Delete (X)").clicked() {
                delete = true;
            }
            if ui.button("<").on_hover_text("Move section left").clicked() {
                left = true;
            }
            if ui.button(">").on_hover_text("Move section right").clicked() {
                right = true;
            }
            if ui
                .button("Trans")
                .on_hover_text("Fade this section out and the next one in (0.5 s)")
                .clicked()
            {
                let target = self.selected_clip;
                if let Some(id) = target {
                    if let Some((li, _)) = self.project.find_clip(id) {
                        if self.layer_locked(li) {
                            self.set_status("That track is locked. Unlock it to edit.", true);
                        } else {
                            self.snapshot();
                            match self.project.fade_transition(id, 0.5) {
                                Some(d) => {
                                    self.after_edit();
                                    self.set_status(
                                        format!("Transition applied ({d:.2} s)."),
                                        false,
                                    );
                                }
                                None => {
                                    self.undo.pop();
                                    self.set_status(
                                        "There is no following section to transition into.",
                                        true,
                                    );
                                }
                            }
                        }
                    }
                } else {
                    self.set_status("Select a section first.", true);
                }
            }
            if split {
                self.split();
            }
            if delete {
                self.delete_selected();
            }
            if left {
                self.move_selected_in_layer(-1);
            }
            if right {
                self.move_selected_in_layer(1);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Copy settings")
                .on_hover_text("Copy this section's style, speed, transform and effects")
                .clicked()
            {
                self.copy_settings();
            }
            if ui
                .button("Paste settings")
                .on_hover_text("Apply the copied settings to this section")
                .clicked()
            {
                self.paste_settings();
            }
            if ui
                .button("Ripple delete")
                .on_hover_text("Delete the selection and move later sections left")
                .clicked()
            {
                self.ripple_delete_selected();
            }
            if ui
                .button("Insert edit")
                .on_hover_text("Insert the copied section at the playhead, shifting later sections right")
                .clicked()
            {
                self.insert_edit_clipboard();
            }
            if ui
                .button("Close gaps")
                .on_hover_text("Pack this track's sections back-to-back")
                .clicked()
            {
                self.close_gaps_selected();
            }
        });
        if self.selected_clips.len() > 1 {
            ui.label(
                RichText::new(format!(
                    "{} sections selected - click with Ctrl to add/remove, X ripple-deletes them.",
                    self.selected_clips.len()
                ))
                .small()
                .weak(),
            );
        }
        if changed {
            self.after_edit();
        }
        self.section_transform_ui(ui);
    }

    /// The section whose transform the inspector/preview currently edits.
    /// Prefers the section under the playhead on the selected track, then the
    /// explicitly selected section.
    fn transform_target(&self) -> Option<(usize, usize)> {
        let li = self.selected_layer.min(self.project.layers.len().checked_sub(1)?);
        let layer = self.project.layers.get(li)?;
        if let Some((ci, _)) = layer.locate(self.position) {
            return Some((li, ci));
        }
        self.project.find_clip(self.selected_clip?)
    }

    fn section_transform_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let Some((li, ci)) = self.transform_target() else {
            ui.label(RichText::new("Section transform").strong());
            ui.label(
                RichText::new(
                    "Move the playhead over a section to change its position, scale or opacity.",
                )
                .small()
                .weak(),
            );
            return;
        };
        let name = truncate(&self.project.layers[li].clips[ci].name, 24);
        ui.label(RichText::new(format!("Section transform - {name}")).strong());
        ui.label(
            RichText::new(
                "Applies to this section only. Other sections on the track keep their own settings.",
            )
            .small()
            .weak(),
        );

        // Local time of the playhead inside this section (0 = section start).
        let section_start = self.project.layers[li].clip_start(ci);
        let local = (self.position - section_start).clamp(0.0, {
            let d = self.project.layers[li].clips[ci].duration;
            d
        });
        let animated = !self.project.layers[li].clips[ci].keyframes.is_empty();
        let mut anim_action: Option<&str> = None;
        ui.horizontal(|ui| {
            if !animated {
                if ui
                    .button("Animate transform")
                    .on_hover_text("Record the current position/scale/opacity as a keyframe")
                    .clicked()
                {
                    anim_action = Some("start");
                }
            } else {
                if ui.button("+ Key").on_hover_text("Add a keyframe at the playhead").clicked() {
                    anim_action = Some("add");
                }
                if ui.button("Del key").on_hover_text("Remove the keyframe at the playhead").clicked() {
                    anim_action = Some("del");
                }
                if ui.button("Clear").on_hover_text("Remove all keyframes").clicked() {
                    anim_action = Some("clear");
                }
            }
        });
        if animated {
            let keys = self.project.layers[li].clips[ci].keyframes.len();
            let on_key = self.project.layers[li].clips[ci]
                .keyframes
                .iter()
                .any(|k| (k.time - local).abs() < 0.05);
            ui.label(
                RichText::new(format!(
                    "{keys} keyframes - playhead at {local:.2}s{}",
                    if on_key { " (on a key)" } else { "" }
                ))
                .small()
                .weak(),
            );
        }
        match anim_action {
            Some("start") | Some("add") => {
                let current = self.project.layers[li].clips[ci].transform_at(local);
                self.snapshot();
                let clip = &mut self.project.layers[li].clips[ci];
                if clip.keyframes.is_empty() {
                    // Start from the section's static transform at time 0 so the
                    // first drag clearly becomes a movement.
                    clip.keyframes.push(Keyframe {
                        time: 0.0,
                        transform: current,
                    });
                }
                clip.keyframes.retain(|k| (k.time - local).abs() > 0.01);
                clip.keyframes.push(Keyframe {
                    time: local,
                    transform: current,
                });
                clip.keyframes
                    .sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
                self.after_edit();
                self.set_status("Keyframe added at the playhead.", false);
            }
            Some("del") => {
                self.snapshot();
                let clip = &mut self.project.layers[li].clips[ci];
                let before = clip.keyframes.len();
                clip.keyframes.retain(|k| (k.time - local).abs() > 0.05);
                if clip.keyframes.len() != before {
                    self.after_edit();
                    self.set_status("Keyframe removed.", false);
                } else {
                    self.undo.pop();
                }
            }
            Some("clear") => {
                self.snapshot();
                self.project.layers[li].clips[ci].keyframes.clear();
                self.after_edit();
                self.set_status("Animation cleared.", false);
            }
            _ => {}
        }

        let mut changed = false;
        {
            // When animated, the sliders edit the key at the playhead so repeated
            // edits record motion; otherwise they edit the static transform.
            let clip = &mut self.project.layers[li].clips[ci];
            let mut t = if clip.keyframes.is_empty() {
                clip.transform
            } else {
                clip.transform_at(local)
            };
            ui.horizontal(|ui| {
                ui.label("X");
                changed |= ui
                    .add(egui::Slider::new(&mut t.x, -1.0..=1.5).fixed_decimals(2))
                    .changed();
                ui.label("Y");
                changed |= ui
                    .add(egui::Slider::new(&mut t.y, -1.0..=1.5).fixed_decimals(2))
                    .changed();
            });
            changed |= ui
                .add(egui::Slider::new(&mut t.scale, 0.02..=2.0).text("Scale"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut t.opacity, 0.0..=1.0).text("Opacity"))
                .changed();
            ui.horizontal(|ui| {
                if ui
                    .button("Fill frame")
                    .on_hover_text("Make this section fill the whole output frame")
                    .clicked()
                {
                    t = Transform::default();
                    changed = true;
                }
                if ui
                    .button("Webcam preset")
                    .on_hover_text("Small section in the bottom-right corner")
                    .clicked()
                {
                    t = Transform::webcam_default();
                    changed = true;
                }
            });
            if changed {
                if clip.keyframes.is_empty() {
                    clip.transform = t;
                } else {
                    clip.keyframes.retain(|k| (k.time - local).abs() > 0.01);
                    clip.keyframes.push(Keyframe { time: local, transform: t });
                    clip.keyframes.sort_by(|a, b| {
                        a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }
            ui.label(
                RichText::new("X/Y are fractions of the frame (0.62 = 62%).")
                    .small()
                    .weak(),
            );
        }
        if changed {
            self.after_edit();
        }
    }

    fn project_ui(&mut self, ui: &mut egui::Ui) {
        let mut dirty = false;
        ui.label(RichText::new("Project settings").strong());
        ui.horizontal(|ui| {
            ui.label("Name");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.project.name).desired_width(180.0),
            );
            if resp.changed() {
                dirty = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Resolution");
            let current = format!("{} x {}", self.project.width, self.project.height);
            let mut selected: Option<(u32, u32)> = None;
            egui::ComboBox::from_id_salt("project_resolution")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (w, h, label) in [
                        (1920u32, 1080u32, "1080p - 1920 x 1080"),
                        (1280, 720, "720p - 1280 x 720"),
                        (3840, 2160, "4K - 3840 x 2160"),
                        (1080, 1920, "Vertical - 1080 x 1920"),
                        (1080, 1080, "Square - 1080 x 1080"),
                    ] {
                        if ui
                            .selectable_label(self.project.width == w && self.project.height == h, label)
                            .clicked()
                        {
                            selected = Some((w, h));
                        }
                    }
                });
            if let Some((w, h)) = selected {
                self.project.width = w;
                self.project.height = h;
                self.after_edit();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Frame rate");
            let mut selected: Option<u32> = None;
            egui::ComboBox::from_id_salt("project_fps")
                .selected_text(format!("{} fps", self.project.fps))
                .show_ui(ui, |ui| {
                    for fps in [24u32, 25, 30, 50, 60] {
                        if ui
                            .selectable_label(self.project.fps == fps, format!("{fps} fps"))
                            .clicked()
                        {
                            selected = Some(fps);
                        }
                    }
                });
            if let Some(fps) = selected {
                self.project.fps = fps;
                self.after_edit();
            }
        });
        ui.label(
            RichText::new(format!(
                "{} sections of {}  -  {} layers of {}",
                self.project.total_clips(),
                MAX_CLIPS,
                self.project.layers.len(),
                MAX_LAYERS
            ))
            .small()
            .weak(),
        );
        ui.separator();
        ui.label(RichText::new("Playback speed (whole project)").strong());
        let mut apply_speed: Option<f64> = None;
        ui.horizontal(|ui| {
            ui.label("Speed");
            let mut speed = self.project.speed;
            let resp = ui
                .add(
                    egui::DragValue::new(&mut speed)
                        .range(0.1..=4.0)
                        .speed(0.01)
                        .suffix("x"),
                )
                .on_hover_text(
                    "Retimes every section and the whole timeline. 2x makes the project twice as fast.",
                );
            // Commit once the drag/typing finishes instead of on every pixel.
            if resp.drag_stopped() || (resp.lost_focus() && resp.changed()) {
                apply_speed = Some(speed);
            }
        });
        ui.horizontal_wrapped(|ui| {
            for preset in [0.5f64, 0.75, 1.0, 1.15, 1.25, 1.5, 2.0, 4.0] {
                if ui
                    .add(egui::Button::new(format!("{preset}x")).small())
                    .on_hover_text("Retime the whole project")
                    .clicked()
                {
                    apply_speed = Some(preset);
                }
            }
        });
        if let Some(target) = apply_speed {
            self.apply_project_speed(target);
        }
        ui.label(
            RichText::new(format!(
                "Timeline length {}. Each section can still have its own speed in the Section tab.",
                fmt_time(self.project.duration())
            ))
            .small()
            .weak(),
        );
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Export speed");
            let mut selected: Option<&'static str> = None;
            egui::ComboBox::from_id_salt("export_preset")
                .selected_text(self.export_preset.clone())
                .show_ui(ui, |ui| {
                    for (preset, hint) in [
                        ("ultrafast", "fastest, largest file"),
                        ("superfast", "very fast"),
                        ("veryfast", "fast"),
                        ("faster", "fast"),
                        ("fast", "quick"),
                        ("medium", "balanced (default)"),
                        ("slow", "smaller file"),
                        ("slower", "much smaller file"),
                        ("veryslow", "smallest file, slowest"),
                    ] {
                        if ui
                            .selectable_label(self.export_preset == preset, preset)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            selected = Some(preset);
                        }
                    }
                })
                .response
                .on_hover_text("FFmpeg x264 preset used when exporting");
            if let Some(preset) = selected {
                self.export_preset = preset.to_owned();
            }
            ui.label(
                RichText::new(format!("x264 {preset}", preset = self.export_preset))
                    .small()
                    .weak(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Export format");
            let current = EXPORT_PROFILES
                .iter()
                .find(|(_, id)| *id == self.export_profile)
                .map(|(label, _)| *label)
                .unwrap_or("MP4 - project size");
            let mut selected: Option<&'static str> = None;
            egui::ComboBox::from_id_salt("export_profile")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (label, id) in EXPORT_PROFILES {
                        if ui
                            .selectable_label(self.export_profile == *id, *label)
                            .clicked()
                        {
                            selected = Some(id);
                        }
                    }
                })
                .response
                .on_hover_text("Output size/format; vertical and square are made for social video");
            if let Some(id) = selected {
                self.export_profile = id.to_owned();
            }
        });
        ui.checkbox(&mut self.project.normalize_audio, "Normalize loudness (-16 LUFS)")
            .on_hover_text("EBU R128 loudness normalisation and peak limiting on export");
        ui.separator();
        ui.label(RichText::new("Subtitles").small().strong());
        ui.horizontal(|ui| {
            if ui
                .button("Import SRT")
                .on_hover_text("Turn SRT cues into text sections on the selected track (burned in on export)")
                .clicked()
            {
                self.import_srt();
            }
            if ui
                .button("Export SRT")
                .on_hover_text("Write every text section to an SRT file")
                .clicked()
            {
                self.export_srt();
            }
        });
        if let Some(path) = &self.project.path {
            ui.add(
                egui::Label::new(RichText::new(path.display().to_string()).small().weak())
                    .truncate(),
            )
            .on_hover_text(path.display().to_string());
        }
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new("Markers").small().strong());
        if self.project.markers.is_empty() {
            ui.label(
                RichText::new("No markers. Press M at the playhead to add one.")
                    .small()
                    .weak(),
            );
        } else {
            let snapshot = self.project.markers.clone();
            let mut jump: Option<f64> = None;
            let mut drop: Option<f64> = None;
            for m in &snapshot {
                ui.horizontal(|ui| {
                    if ui
                        .small_button(format!("-> {}", fmt_time(*m)))
                        .on_hover_text("Jump the playhead to this marker")
                        .clicked()
                    {
                        jump = Some(*m);
                    }
                    if ui.small_button("x").on_hover_text("Delete marker").clicked() {
                        drop = Some(*m);
                    }
                });
            }
            if let Some(t) = jump {
                self.seek(t);
            }
            if let Some(t) = drop {
                self.snapshot();
                self.project.markers.retain(|m| (*m - t).abs() > 1e-9);
                self.after_edit();
            }
        }
        if dirty {
            self.dirty = true;
        }
    }

    // ------------------------------------------------------------------
    // Preview / transport
    // ------------------------------------------------------------------

    fn preview_ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let controls_h = 42.0;
        let avail = ui.available_size();
        let preview_h = (avail.y - controls_h).max(80.0);
        let (area, _) =
            ui.allocate_exact_size(egui::vec2(avail.x, preview_h), Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::same(6), Color32::from_rgb(12, 12, 15));

        let aspect = self.project.width.max(1) as f32 / self.project.height.max(1) as f32;
        let mut w = area.width().min(area.height() * aspect);
        let mut h = w / aspect;
        if h > area.height() {
            h = area.height();
            w = h * aspect;
        }
        let rect = Rect::from_center_size(area.center(), egui::vec2(w, h));

        if self.ffmpeg_version.is_none() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "FFmpeg not found.\nInstall FFmpeg and make sure ffmpeg.exe is on PATH.",
                FontId::proportional(16.0),
                ERR_COLOR,
            );
        } else if let Some(tex) = &self.preview_tex {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                if self.project.total_clips() == 0 {
                    "No media yet\n\nDrop video/photo files onto the window\nor use  + Add Media"
                } else if self.project.layers.iter().all(|l| !l.visible) {
                    "All tracks are hidden\n\nClick an eye in the track headers to show one."
                } else {
                    "Rendering preview..."
                },
                FontId::proportional(16.0),
                Color32::from_gray(130),
            );
        }
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::from_gray(52)),
            StrokeKind::Inside,
        );

        if self.ffmpeg_version.is_some() {
            self.preview_transform_ui(ui, rect);
            // Explain why nothing can be dragged right now.
            if !self.playing && !self.selected_layer_active() {
                if let Some(layer) = self.project.layers.get(self.selected_layer) {
                    if !layer.clips.is_empty() {
                        let hint = if !layer.visible {
                            format!(
                                "{} is hidden. Click its eye in the track header to edit it here.",
                                truncate(&layer.name, 22)
                            )
                        } else {
                            format!(
                                "{} is not on the timeline at {}. Move the playhead onto it to edit it here.",
                                truncate(&layer.name, 22),
                                fmt_time(self.position)
                            )
                        };
                        let g = painter.layout(
                            hint,
                            FontId::proportional(11.5),
                            Color32::from_gray(235),
                            520.0,
                        );
                        let pad = 8.0;
                        let box_rect = Rect::from_min_size(
                            egui::pos2(
                                rect.center().x - (g.size().x + pad * 2.0) * 0.5,
                                rect.bottom() - g.size().y - pad * 2.0 - 10.0,
                            ),
                            egui::vec2(g.size().x + pad * 2.0, g.size().y + pad * 2.0),
                        );
                        painter.rect_filled(
                            box_rect,
                            CornerRadius::same(4),
                            Color32::from_rgba_unmultiplied(18, 20, 26, 225),
                        );
                        painter.galley(
                            egui::pos2(box_rect.left() + pad, box_rect.top() + pad),
                            g,
                            Color32::WHITE,
                        );
                    }
                }
            }
        }

        if self.playing {
            painter.circle_filled(
                egui::pos2(rect.left() + 16.0, rect.top() + 16.0),
                5.0,
                Color32::from_rgb(255, 80, 80),
            );
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let play_label = if self.playing { "Pause" } else { "Play" };
            if ui
                .add_sized([76.0, 28.0], egui::Button::new(play_label))
                .on_hover_text("Space - play from the start marker (or beginning). Right Shift also stops.")
                .clicked()
            {
                self.toggle_play();
            }
            if ui
                .button("Resume")
                .on_hover_text("Enter - continue from the current playhead")
                .clicked()
            {
                if self.playing {
                    self.pause();
                } else {
                    self.resume();
                }
            }
            if ui
                .button("Stop")
                .on_hover_text("Return to the start marker (or beginning)")
                .clicked()
            {
                self.stop();
            }
            let loop_label = if self.loop_playback { "Loop: on" } else { "Loop: off" };
            if ui
                .selectable_label(self.loop_playback, loop_label)
                .on_hover_text("Replay the whole timeline when playback ends")
                .clicked()
            {
                self.loop_playback = !self.loop_playback;
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    fmt_time(self.position),
                    fmt_time(self.project.duration())
                ))
                .monospace(),
            );
            ui.separator();
            if ui.small_button("Split (S)").clicked() {
                self.split();
            }
            if ui.small_button("Delete (X)").clicked() {
                self.delete_selected();
            }
            ui.separator();
            let marker_label = match self.project.start_marker {
                Some(t) => format!("Start: {}", fmt_time(t)),
                None => "Start: none".to_owned(),
            };
            if ui
                .small_button(marker_label)
                .on_hover_text("Set playback start marker here (F)")
                .clicked()
            {
                self.set_start_marker();
            }
            if self.project.start_marker.is_some() && ui.small_button("Clear (T)").clicked() {
                self.clear_start_marker();
            }
            let room = ui.available_width();
            if room > 300.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(if self.fullscreen {
                            "Fullscreen preview - F11 or Esc to exit"
                        } else {
                            "Live preview mix - export matches this soundtrack"
                        })
                        .small()
                        .weak(),
                    );
                });
            }
        });
        if self.playing {
            ctx.request_repaint();
        }
    }

    // ------------------------------------------------------------------
    // Preview manipulation
    // ------------------------------------------------------------------

    /// True when the selected layer has visible content at the playhead, so it
    /// can be manipulated in the preview.
    fn selected_layer_active(&self) -> bool {
        let Some(layer) = self.project.layers.get(self.selected_layer) else {
            return false;
        };
        !layer.clips.is_empty() && layer.visible && layer.locate(self.position).is_some()
    }

    /// Move/resize the selected layer by dragging directly in the preview.
    /// `frame` is the on-screen rect of the composited video.
    fn preview_transform_ui(&mut self, ui: &mut egui::Ui, frame: Rect) {
        if self.playing || self.project.layers.is_empty() {
            return;
        }
        let li = self.selected_layer.min(self.project.layers.len() - 1);
        let layer = &self.project.layers[li];
        if !layer.visible {
            return;
        }
        let Some((ci, _)) = layer.locate(self.position) else {
            return;
        };
        // Each section has its own transform; manipulate the one under the playhead.
        let clip_id = layer.clips[ci].id;
        let layer_start = layer.clip_start(ci);
        // Honour keyframes: the box follows the animated transform at this time.
        let local = (self.position - layer_start).max(0.0);
        let t = layer.clips[ci].transform_at(local);
        let fw = frame.width().max(1.0);
        let fh = frame.height().max(1.0);
        let lw = (fw * t.scale as f32).max(8.0);
        let lh = (fh * t.scale as f32).max(8.0);
        let lrect = Rect::from_min_size(
            egui::pos2(frame.left() + t.x as f32 * fw, frame.top() + t.y as f32 * fh),
            egui::vec2(lw, lh),
        );

        let painter = ui.painter_at(ui.max_rect());
        painter.rect_stroke(
            lrect,
            CornerRadius::ZERO,
            Stroke::new(1.5, ACCENT),
            StrokeKind::Inside,
        );

        // Body first: the corner handles are registered afterwards so they sit
        // on top of it and win the hit test near the corners.
        let body = ui.interact(
            lrect,
            ui.make_persistent_id(("preview_body", clip_id)),
            Sense::drag(),
        );
        if body.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
        if body.drag_started() {
            if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                self.preview_drag = Some(PreviewGesture {
                    grab: PreviewGrab::Move,
                    origin: p,
                    start: t.clone(),
                    committed: false,
                });
            }
        }

        // Corner handles (scale, keeping the opposite corner fixed).
        let handles = [
            (PreviewGrab::Nw, lrect.left_top(), egui::CursorIcon::ResizeNwSe),
            (PreviewGrab::Ne, lrect.right_top(), egui::CursorIcon::ResizeNeSw),
            (PreviewGrab::Sw, lrect.left_bottom(), egui::CursorIcon::ResizeNeSw),
            (PreviewGrab::Se, lrect.right_bottom(), egui::CursorIcon::ResizeNwSe),
        ];
        for (grab, pos, cursor) in handles {
            // Generous grab area, smaller drawn handle.
            let hr = Rect::from_center_size(pos, egui::vec2(26.0, 26.0));
            let resp = ui.interact(
                hr,
                ui.make_persistent_id(("preview_handle", clip_id, grab as u8)),
                Sense::drag(),
            );
            painter.rect_filled(
                Rect::from_center_size(pos, egui::vec2(10.0, 10.0)),
                CornerRadius::same(2),
                ACCENT,
            );
            if resp.hovered() {
                ui.ctx().set_cursor_icon(cursor);
            }
            if resp.drag_started() {
                if let Some(p) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                    self.preview_drag = Some(PreviewGesture {
                        grab,
                        origin: p,
                        start: t.clone(),
                        committed: false,
                    });
                }
            }
        }

        if self.preview_drag.is_some() {
            let pointer = ui.ctx().input(|i| i.pointer.interact_pos());
            let released = ui.ctx().input(|i| i.pointer.any_released());
            let mut gesture = self.preview_drag.take().unwrap();
            let delta = pointer.map(|p| egui::vec2(p.x - gesture.origin.x, p.y - gesture.origin.y));
            if let Some(delta) = delta {
                let moved = delta.x.abs() + delta.y.abs() > 0.75;
                if moved {
                    if !gesture.committed {
                        self.snapshot();
                        gesture.committed = true;
                    }
                    let dx = delta.x as f64 / fw as f64;
                    let dy = delta.y as f64 / fh as f64;
                    let s0 = gesture.start.x;
                    let t0 = gesture.start.y;
                    let scale0 = gesture.start.scale;
                    let mut next = gesture.start.clone();
                    match gesture.grab {
                        PreviewGrab::Move => {
                            next.x = s0 + dx;
                            next.y = t0 + dy;
                        }
                        _ => {
                            // Fixed anchor at the opposite corner, in frame fractions.
                            let (ax, ay) = match gesture.grab {
                                PreviewGrab::Nw => (s0 + scale0, t0 + scale0),
                                PreviewGrab::Ne => (s0, t0 + scale0),
                                PreviewGrab::Sw => (s0 + scale0, t0),
                                _ => (s0, t0),
                            };
                            let px = s0 + dx;
                            let py = t0 + dy;
                            let (ex, ey) = match gesture.grab {
                                PreviewGrab::Nw => (ax - px, ay - py),
                                PreviewGrab::Ne => (px - ax, ay - py),
                                PreviewGrab::Sw => (ax - px, py - ay),
                                _ => (px - ax, py - ay),
                            };
                            let scale = ex.max(ey).clamp(0.02, 2.0);
                            let (nx, ny) = match gesture.grab {
                                PreviewGrab::Nw => (ax - scale, ay - scale),
                                PreviewGrab::Ne => (ax, ay - scale),
                                PreviewGrab::Sw => (ax - scale, ay),
                                _ => (ax, ay),
                            };
                            next.scale = scale;
                            next.x = nx;
                            next.y = ny;
                        }
                    }
                    next.x = next.x.clamp(-1.5, 2.0);
                    next.y = next.y.clamp(-1.5, 2.0);
                    if let Some(clip) = self
                        .project
                        .layers
                        .get_mut(li)
                        .and_then(|l| l.clips.get_mut(ci))
                    {
                        if clip.keyframes.is_empty() {
                            clip.transform = next;
                        } else {
                            // Animated section: dragging records a key at the
                            // playhead instead of clobbering the whole animation.
                            let local = (self.position - layer_start).max(0.0);
                            clip.keyframes.retain(|k| (k.time - local).abs() > 0.01);
                            clip.keyframes.push(Keyframe {
                                time: local,
                                transform: next,
                            });
                            clip.keyframes.sort_by(|a, b| {
                                a.time
                                    .partial_cmp(&b.time)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                    }
                    self.dirty = true;
                    ui.ctx().request_repaint();
                }
            }
            if released {
                if gesture.committed {
                    // Re-render the preview once at the final position.
                    self.after_edit();
                    self.force_static();
                    self.set_status("Section position updated.", false);
                }
                ui.ctx().request_repaint();
            } else {
                self.preview_drag = Some(gesture);
            }
        }
    }

    // ------------------------------------------------------------------
    // Export window
    // ------------------------------------------------------------------

    fn export_window(&mut self, ctx: &egui::Context) {
        if !self.show_export {
            return;
        }
        let mut open = true;
        let mut close_requested = false;
        egui::Window::new("Export video")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let Some(state) = &self.export else {
                    ui.label("No export running.");
                    return;
                };
                match &state.finished {
                    None => {
                        ui.label(format!(
                            "Rendering your timeline with FFmpeg (x264 preset: {})...",
                            self.export_preset
                        ));
                        ui.add_space(6.0);
                        ui.add(
                            egui::ProgressBar::new(state.progress)
                                .desired_width(340.0)
                                .show_percentage(),
                        );
                        ui.add_space(6.0);
                        if ui.button("Cancel export").clicked() {
                            state.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    Some(Ok(path)) => {
                        ui.label(RichText::new("Export complete.").color(OK_COLOR).strong());
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(path.display().to_string())
                                .truncate(),
                        )
                        .on_hover_text(path.display().to_string());
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            close_requested = true;
                        }
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new("Export failed.").color(ERR_COLOR).strong());
                        ui.add_space(4.0);
                        ui.label(err.as_str());
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            close_requested = true;
                        }
                    }
                }
            });
        if close_requested {
            self.show_export = false;
            self.export = None;
        } else if !open {
            self.show_export = false;
            let finished = self
                .export
                .as_ref()
                .map(|s| s.finished.is_some())
                .unwrap_or(true);
            if finished {
                self.export = None;
            }
        }
        if self.export.as_ref().and_then(|s| s.finished.as_ref()).is_none() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = format!(
            "RunVideo - {}{}",
            self.project.name,
            if self.dirty { " *" } else { "" }
        );
        if title != self.last_title {
            self.last_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    // ------------------------------------------------------------------
    // Help window
    // ------------------------------------------------------------------

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = true;
        egui::Window::new("Shortcuts & help")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
                    let sections: &[(&str, &[(&str, &str)])] = &[
                        (
                            "Playback",
                            &[
                                ("Space", "Play from the start marker (or the beginning)"),
                                ("Enter", "Resume from the current playhead"),
                                ("Right Shift", "Stop playback"),
                                ("Click / drag timeline", "Jump the playhead there"),
                                (",  /  .", "Step one frame (Shift = 10 frames)"),
                                ("<- / ->", "Step 1 s (Shift = 5 s)"),
                                ("Home / End", "Jump to the start / end"),
                            ],
                        ),
                        (
                            "Editing",
                            &[
                                ("S", "Split the section at the playhead"),
                                ("X / Delete", "Delete the selected section"),
                                ("Ctrl+D", "Duplicate the selected section"),
                                ("Ctrl+C / Ctrl+V", "Copy / paste a section at the playhead"),
                                ("Ctrl+click", "Select several sections (X ripple-deletes them)"),
                            ],
                        ),
                        (
                            "Sections & tracks",
                            &[
                                ("L / Y", "Add an overlay track"),
                                ("F", "Set the playback start marker"),
                                ("T", "Clear the start marker"),
                                ("M", "Add a timeline marker at the playhead"),
                                ("Alt+M", "Remove the nearest marker"),
                                ("Drag clip edge", "Trim the section"),
                                ("Drag clip", "Move within / between tracks"),
                                ("Drag track header", "Reorder tracks"),
                            ],
                        ),
                        (
                            "Project",
                            &[
                                ("Ctrl+S / Ctrl+O", "Save / open a project"),
                                ("Ctrl+E", "Export MP4"),
                                ("Ctrl+N", "New project"),
                                ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                                ("Ctrl+Plus", "Zoom the timeline to fit"),
                                ("Ctrl+wheel", "Zoom around the pointer"),
                                ("F11 / Esc", "Fullscreen preview on / off"),
                                ("F1", "Show this help"),
                            ],
                        ),
                    ];
                    for (title, rows) in sections {
                        ui.add_space(4.0);
                        ui.label(RichText::new(*title).strong().color(ACCENT));
                        egui::Grid::new(format!("help_{title}"))
                            .num_columns(2)
                            .spacing([18.0, 4.0])
                            .show(ui, |ui| {
                                for (key, what) in *rows {
                                    ui.label(RichText::new(*key).monospace().strong());
                                    ui.label(*what);
                                    ui.end_row();
                                }
                            });
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Tip: right-click a track header for rename, add media, move and delete. \
                             Right-click a section for split, duplicate, copy, delete and volume.",
                        )
                        .small()
                        .weak(),
                    );
                });
            });
        self.show_help = open;
    }

    // ------------------------------------------------------------------
    // Autosave / crash recovery
    // ------------------------------------------------------------------

    fn autosave_path() -> PathBuf {
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        base.join("RunVideo").join("autosave.rvproj")
    }

    /// Write the recovery file every 15 s while the project has unsaved edits.
    fn maybe_autosave(&mut self, ctx: &egui::Context) {
        if !self.dirty || self.project.total_clips() == 0 {
            return;
        }
        if self.last_autosave.elapsed() < Duration::from_secs(15) {
            return;
        }
        self.last_autosave = Instant::now();
        let path = Self::autosave_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&self.project) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        ctx.request_repaint_after(Duration::from_secs(15));
    }

    /// Offer to restore an autosave left behind by a crash.
    fn check_recovery(&mut self) {
        let path = Self::autosave_path();
        if !path.exists() {
            return;
        }
        // Only offer recovery when the file is newer than the last clean save.
        self.recovery = Some(path);
    }

    fn recover_autosave(&mut self, ctx: &egui::Context) {
        let Some(path) = self.recovery.clone() else {
            return;
        };
        match self.load_project_path(&path) {
            Ok(()) => {
                self.project.path = None;
                self.dirty = true;
                self.refresh_thumbs(ctx);
                self.set_status(
                    "Recovered the autosaved project. Save it to keep changes.",
                    false,
                );
            }
            Err(e) => self.set_status(e, true),
        }
        self.recovery = None;
    }

    fn discard_recovery(&mut self) {
        let Some(path) = self.recovery.take() else {
            return;
        };
        let _ = std::fs::remove_file(path);
        self.set_status("Discarded the autosave.", false);
    }

    /// Small banner offering to restore an autosave after a crash.
    fn recovery_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if self.recovery.is_none() {
            return;
        }
        egui::Panel::top("recovery_bar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(60, 48, 20))
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("An autosaved project from a previous session was found.")
                            .color(Color32::from_rgb(240, 210, 120)),
                    );
                    if ui.button("Recover").clicked() {
                        self.recover_autosave(ctx);
                    }
                    if ui.button("Discard").clicked() {
                        self.discard_recovery();
                    }
                });
            });
    }
}

impl eframe::App for RunVideoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_preview(&ctx);
        self.poll_export();
        self.poll_capture();
        self.poll_silence();
        self.poll_waveforms();
        self.ensure_waveforms();
        self.handle_dropped(&ctx);
        self.handle_shortcuts(&ctx);
        self.tick_playback(&ctx);
        // Drive the live soundtrack from the same state as the preview stream.
        // Wait for the first frame: it tells us the stream is really running, so
        // the sound does not start before the picture is on screen.
        self.audio.sync(
            self.playing && self.stream_time.is_some(),
            self.gen,
            &self.project,
            self.stream_base,
        );
        self.maybe_autosave(&ctx);

        if !self.fullscreen {
            self.recovery_bar(&ctx, ui);
            self.top_bar(ui);
            self.timeline_panel(ui);
            self.right_panel(ui);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PANEL_BG).inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| {
                self.preview_ui(&ctx, ui);
            });

        self.maybe_refresh_preview(&ctx);
        // A finished render lands in the worker's channel; keep polling while one
        // is in flight so an idle app still picks it up.
        if self.preview_inflight {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.export_window(&ctx);
        self.help_window(&ctx);
        self.update_title(&ctx);
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    let hours = (t / 3600.0).floor() as u64;
    let minutes = ((t % 3600.0) / 60.0).floor() as u64;
    let seconds = t % 60.0;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:05.2}")
    } else {
        format!("{minutes:02}:{seconds:05.2}")
    }
}

fn fmt_ruler(t: f64) -> String {
    let total = t.max(0.0);
    let minutes = (total / 60.0).floor() as u64;
    let seconds = total % 60.0;
    if minutes > 0 {
        format!("{minutes}:{seconds:02.0}")
    } else {
        format!("{seconds:.0}s")
    }
}

fn paint_eye(painter: &egui::Painter, rect: Rect, visible: bool, color: Color32) {
    let c = rect.center();
    let w = (rect.width() * 0.5).max(5.0);
    let h = (rect.height() * 0.30).max(2.0);
    let mut pts: Vec<Pos2> = Vec::with_capacity(25);
    for i in 0..=24 {
        let a = i as f32 / 24.0 * std::f32::consts::TAU;
        pts.push(egui::pos2(c.x + w * a.cos(), c.y + h * a.sin()));
    }
    painter.add(Shape::closed_line(pts, Stroke::new(1.3, color)));
    painter.circle_filled(c, (h * 0.9).max(1.5), color);
    if !visible {
        painter.line_segment(
            [
                egui::pos2(c.x - w, c.y - h * 1.5),
                egui::pos2(c.x + w, c.y + h * 1.5),
            ],
            Stroke::new(1.6, color),
        );
    }
}

/// Square M/S/L toggle button in a track header. `slot` is 2.5 (mute),
/// 1.5 (solo) or 0.5 (lock). Shared by the paint and interaction passes so they
/// can never drift apart.
fn track_hdr_button(header: Rect, slot: f32) -> Rect {
    let btn = 18.0;
    let gap = 2.0;
    let right = header.right() - 5.0;
    Rect::from_center_size(
        egui::pos2(
            right - btn * slot - gap * (slot * 2.0 - 1.0),
            header.center().y,
        ),
        egui::vec2(btn, btn),
    )
}

fn choose_tick_step(pps: f32) -> f64 {
    for step in [
        0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
    ] {
        if step as f32 * pps >= 64.0 {
            return step;
        }
    }
    7200.0
}

/// Small `#rrggbb` colour swatch + picker.
fn color_edit(ui: &mut egui::Ui, hex: &mut String) -> bool {
    let mut rgb = parse_hex(hex);
    let mut c = Color32::from_rgb(rgb.0, rgb.1, rgb.2);
    let resp = ui.color_edit_button_srgba(&mut c);
    if resp.changed() {
        rgb = (c.r(), c.g(), c.b());
        *hex = format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2);
        return true;
    }
    false
}

fn parse_hex(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim().trim_start_matches('#');
    if h.len() >= 6 {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(255);
        (r, g, b)
    } else {
        (255, 255, 255)
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push_str("...");
    out
}

fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('_').to_owned();
    if trimmed.is_empty() {
        "project".to_owned()
    } else {
        trimmed
    }
}

#[allow(dead_code)]
fn layer_clip_count(layer: &Layer) -> usize {
    layer.clips.len()
}

#[allow(dead_code)]
fn imported_audio(clip: &Clip) -> bool {
    clip.has_audio
}
