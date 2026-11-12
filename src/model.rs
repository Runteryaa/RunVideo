//! Core project/timeline data model. Everything here is plain serializable data.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Hard requirement of the editor: a project contains at most 100 sections/clips.
pub const MAX_CLIPS: usize = 100;
/// Base layer + up to 7 overlay layers.
pub const MAX_LAYERS: usize = 8;
pub const MIN_CLIP: f64 = 0.05;
/// Version 2 moved the transform from the layer onto each section/clip.
pub const PROJECT_VERSION: u32 = 3;
pub const IMAGE_DEFAULT_DURATION: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Image,
    /// Generated title/caption, rendered by FFmpeg's `drawtext`.
    Text,
}

impl MediaKind {
    pub fn is_video(self) -> bool {
        matches!(self, MediaKind::Video)
    }

    pub fn is_text(self) -> bool {
        matches!(self, MediaKind::Text)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub kind: MediaKind,
    /// Offset into the source video where this section starts (seconds).
    #[serde(default)]
    pub in_point: f64,
    /// Length of this section on the timeline (seconds).
    pub duration: f64,
    /// Offset of this section from the start of its layer (seconds).
    ///
    /// Sections are placed freely: they do not have to be back-to-back, so a
    /// layer can contain gaps (and even overlaps) at any position.
    #[serde(default)]
    pub start: f64,
    /// Full duration of the source video, when known.
    #[serde(default)]
    pub source_duration: Option<f64>,
    #[serde(default)]
    pub has_audio: bool,
    /// Position/scale/opacity of this section inside the output frame.
    ///
    /// Every section carries its own transform, so two clips on the same track
    /// can sit in different corners. For older (version 1) projects the value is
    /// seeded from the layer transform in [`Project::sanitize`].
    #[serde(default)]
    pub transform: Transform,
    /// Animated transform. When non-empty these override `transform` and are
    /// interpolated over the section; times are relative to the section start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<Keyframe>,
    /// Linear audio gain for this section (1.0 = unchanged).
    #[serde(default = "default_gain")]
    pub volume: f64,
    /// Fade-in length in seconds, applied to video and audio.
    #[serde(default)]
    pub fade_in: f64,
    /// Fade-out length in seconds, applied to video and audio.
    #[serde(default)]
    pub fade_out: f64,
    /// Playback speed of the section. 1.0 = normal, 0.5 = half speed.
    #[serde(default = "default_gain")]
    pub speed: f64,
    /// Optional end speed for a speed ramp. When set, the section starts at
    /// `speed` and reaches this rate at its end (linear ramp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_end: Option<f64>,
    /// Play the section backwards.
    #[serde(default)]
    pub reverse: bool,
    /// Hold one frame (taken at `in_point`) for the whole section.
    #[serde(default)]
    pub freeze: bool,
    /// Text content when `kind` is [`MediaKind::Text`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextSpec>,
    /// Brightness adjustment, -1.0 ..= 1.0 (0 = unchanged).
    #[serde(default)]
    pub brightness: f64,
    /// Contrast multiplier, 0.0 ..= 3.0 (1 = unchanged).
    #[serde(default = "default_gain")]
    pub contrast: f64,
    /// Saturation multiplier, 0.0 ..= 3.0 (1 = unchanged).
    #[serde(default = "default_gain")]
    pub saturation: f64,
    /// Gaussian blur sigma in output pixels (0 = off).
    #[serde(default)]
    pub blur: f64,
    /// Rotation in degrees around the section centre (-180 ..= 180).
    #[serde(default)]
    pub rotation: f64,
    /// Crop fractions of the source, each 0.0 ..= 0.45.
    #[serde(default)]
    pub crop_l: f64,
    #[serde(default)]
    pub crop_r: f64,
    #[serde(default)]
    pub crop_t: f64,
    #[serde(default)]
    pub crop_b: f64,
    /// Pixelate block size in output pixels (0 = off). Hides faces/details.
    #[serde(default)]
    pub pixelate: f64,
    /// Sharpen amount, 0.0 ..= 1.5 (0 = off).
    #[serde(default)]
    pub sharpen: f64,
    /// Vignette strength, 0.0 ..= 1.0 (0 = off).
    #[serde(default)]
    pub vignette: f64,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
}

fn default_gain() -> f64 {
    1.0
}

fn default_speed() -> f64 {
    1.0
}

impl Default for Clip {
    fn default() -> Self {
        Self {
            id: 0,
            path: PathBuf::new(),
            name: String::new(),
            kind: MediaKind::Video,
            in_point: 0.0,
            duration: IMAGE_DEFAULT_DURATION,
            start: 0.0,
            source_duration: None,
            has_audio: false,
            transform: Transform::default(),
            keyframes: Vec::new(),
            volume: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            speed: 1.0,
            speed_end: None,
            reverse: false,
            freeze: false,
            text: None,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            blur: 0.0,
            rotation: 0.0,
            crop_l: 0.0,
            crop_r: 0.0,
            crop_t: 0.0,
            crop_b: 0.0,
            pixelate: 0.0,
            sharpen: 0.0,
            vignette: 0.0,
            flip_h: false,
            flip_v: false,
        }
    }
}

/// A single animated transform key.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Seconds from the start of the section.
    pub time: f64,
    #[serde(flatten)]
    pub transform: Transform,
}

/// Horizontal alignment of a text section inside the output frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// Vertical alignment of a text section inside the output frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Styling for a text section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextSpec {
    pub content: String,
    #[serde(default = "default_font_size")]
    pub size: f64,
    /// `#rrggbb` text colour.
    #[serde(default = "default_text_color")]
    pub color: String,
    /// `#rrggbb` box colour behind the text (`None` = no box).
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub bold: bool,
    /// Outline thickness in pixels at 1080p (0 = no outline).
    #[serde(default)]
    pub outline: f64,
    /// `#rrggbb` outline colour.
    #[serde(default = "default_outline_color")]
    pub outline_color: String,
    /// Drop-shadow offset in pixels at 1080p (0 = no shadow).
    #[serde(default)]
    pub shadow: f64,
    /// `#rrggbb` drop-shadow colour.
    #[serde(default = "default_shadow_color")]
    pub shadow_color: String,
    #[serde(default)]
    pub align: TextAlign,
    #[serde(default)]
    pub valign: VAlign,
}

fn default_outline_color() -> String {
    "#000000".to_owned()
}

fn default_shadow_color() -> String {
    "#000000".to_owned()
}

fn default_font_size() -> f64 {
    64.0
}

fn default_text_color() -> String {
    "#ffffff".to_owned()
}

impl TextSpec {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            size: default_font_size(),
            color: default_text_color(),
            background: None,
            bold: false,
            outline: 0.0,
            outline_color: default_outline_color(),
            shadow: 0.0,
            shadow_color: default_shadow_color(),
            align: TextAlign::Center,
            valign: VAlign::Middle,
        }
    }
}

impl Clip {
    /// Transform at `local` seconds into the section, honouring keyframes.
    pub fn transform_at(&self, local: f64) -> Transform {
        if self.keyframes.is_empty() {
            return self.transform;
        }
        let mut keys = self.keyframes.clone();
        keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        let t = local.max(0.0);
        if t <= keys[0].time {
            return keys[0].transform;
        }
        let last = *keys.last().unwrap();
        if t >= last.time {
            return last.transform;
        }
        for pair in keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t >= a.time && t <= b.time {
                let span = (b.time - a.time).max(1e-9);
                let f = ((t - a.time) / span).clamp(0.0, 1.0);
                return lerp_transform(a.transform, b.transform, f);
            }
        }
        self.transform
    }

    /// Seconds of source media this section consumes (accounts for speed and
    /// for a linear speed ramp).
    pub fn source_span(&self) -> f64 {
        let s0 = self.speed.clamp(0.05, 8.0);
        let s1 = self.speed_end.unwrap_or(s0).clamp(0.05, 8.0);
        self.duration * (s0 + s1) * 0.5
    }

    /// Average playback rate over the section (used for trims).
    pub fn avg_speed(&self) -> f64 {
        let s0 = self.speed.clamp(0.05, 8.0);
        let s1 = self.speed_end.unwrap_or(s0).clamp(0.05, 8.0);
        (s0 + s1) * 0.5
    }

    #[allow(dead_code)]
    pub fn end_in_source(&self) -> Option<f64> {
        self.source_duration
            .map(|d| (self.in_point + self.source_span()).min(d))
    }

    /// True when this section renders as a generated title.
    pub fn is_text(&self) -> bool {
        self.kind.is_text()
    }
}

fn lerp(a: f64, b: f64, f: f64) -> f64 {
    a + (b - a) * f
}

fn lerp_transform(a: Transform, b: Transform, f: f64) -> Transform {
    Transform {
        x: lerp(a.x, b.x, f),
        y: lerp(a.y, b.y, f),
        scale: lerp(a.scale, b.scale, f),
        opacity: lerp(a.opacity, b.opacity, f),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Top-left position of the scaled layer, as a fraction of the output frame.
    pub x: f64,
    pub y: f64,
    /// Scale relative to the output frame.
    pub scale: f64,
    pub opacity: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 1.0, opacity: 1.0 }
    }
}

impl Transform {
    pub fn webcam_default() -> Self {
        Self { x: 0.62, y: 0.62, scale: 0.35, opacity: 1.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub transform: Transform,
    /// Timeline offset where this layer begins (seconds). Useful for overlays that
    /// should only appear part-way through the base footage.
    #[serde(default)]
    pub start: f64,
    /// Whether this track is shown in the preview/export (the eye toggle).
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Audio-only mute (the eye also hides video).
    #[serde(default)]
    pub muted: bool,
    /// Solo: when any track is soloed, only soloed tracks are heard.
    #[serde(default)]
    pub solo: bool,
    /// Locked tracks cannot be edited or dragged until unlocked.
    #[serde(default)]
    pub locked: bool,
    /// Track output gain applied to the whole layer's audio (1.0 = unchanged).
    #[serde(default = "default_gain")]
    pub gain: f64,
}

fn default_true() -> bool {
    true
}

impl Layer {
    /// Duration of the layer's own content, ignoring its start offset.
    pub fn content_duration(&self) -> f64 {
        self.clips
            .iter()
            .map(|c| c.start.max(0.0) + c.duration)
            .fold(0.0, f64::max)
    }

    pub fn duration(&self) -> f64 {
        self.start.max(0.0) + self.content_duration()
    }

    /// Absolute start of the clip at `index` within this layer.
    pub fn clip_start(&self, index: usize) -> f64 {
        self.start.max(0.0)
            + self
                .clips
                .get(index)
                .map(|c| c.start.max(0.0))
                .unwrap_or(0.0)
    }

    /// Locate the clip which is playing at absolute timeline time `t`.
    /// When sections overlap the topmost (last added) one wins.
    pub fn locate(&self, t: f64) -> Option<(usize, f64)> {
        let origin = self.start.max(0.0);
        let mut found = None;
        for (i, c) in self.clips.iter().enumerate() {
            let abs = origin + c.start.max(0.0);
            if t >= abs - 1e-9 && t < abs + c.duration - 1e-9 {
                found = Some((i, t - abs));
            }
        }
        found
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[serde(default)]
    pub next_id: u64,
    pub layers: Vec<Layer>,
    /// Optional playback start marker (seconds). Set with `F`, cleared with `T`.
    /// When set, playback begins here unless the playhead is already past it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_marker: Option<f64>,
    /// Free timeline markers (seconds), in ascending order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<f64>,
    /// Cumulative project-wide playback rate (1.0 = normal). Each section's own
    /// speed is multiplied by this value when the whole project is retimed.
    #[serde(default = "default_speed")]
    pub speed: f64,
    /// Apply EBU R128 loudness normalisation to the final mix on export.
    #[serde(default)]
    pub normalize_audio: bool,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    pub fn new() -> Self {
        Self {
            version: PROJECT_VERSION,
            name: "Untitled".to_owned(),
            width: 1920,
            height: 1080,
            fps: 30,
            next_id: 1,
            layers: vec![Layer {
                name: "Base".to_owned(),
                clips: Vec::new(),
                transform: Transform::default(),
                start: 0.0,
                visible: true,
                muted: false,
                solo: false,
                locked: false,
                gain: 1.0,
            }],
            start_marker: None,
            markers: Vec::new(),
            speed: 1.0,
            normalize_audio: false,
            path: None,
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub fn duration(&self) -> f64 {
        self.layers.iter().map(|l| l.duration()).fold(0.0, f64::max)
    }

    pub fn total_clips(&self) -> usize {
        self.layers.iter().map(|l| l.clips.len()).sum()
    }

    /// Add a free timeline marker at `t`, keeping the list sorted and unique.
    pub fn add_marker(&mut self, t: f64) -> bool {
        let total = self.duration();
        if total <= MIN_CLIP {
            return false;
        }
        let t = t.clamp(0.0, total);
        if self.markers.iter().any(|m| (*m - t).abs() < 0.05) {
            return false;
        }
        self.markers.push(t);
        self.markers
            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        true
    }

    /// Remove the marker nearest to `t` when it is within `tolerance` seconds.
    pub fn remove_marker_near(&mut self, t: f64, tolerance: f64) -> bool {
        let Some((idx, _)) = self
            .markers
            .iter()
            .enumerate()
            .map(|(i, m)| (i, (*m - t).abs()))
            .filter(|(_, d)| *d <= tolerance)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        else {
            return false;
        };
        self.markers.remove(idx);
        true
    }

    pub fn find_clip(&self, id: u64) -> Option<(usize, usize)> {
        for (li, layer) in self.layers.iter().enumerate() {
            if let Some(ci) = layer.clips.iter().position(|c| c.id == id) {
                return Some((li, ci));
            }
        }
        None
    }

    #[allow(dead_code)]
    pub fn clip(&self, id: u64) -> Option<&Clip> {
        self.find_clip(id).map(|(li, ci)| &self.layers[li].clips[ci])
    }

    #[allow(dead_code)]
    pub fn clip_mut(&mut self, id: u64) -> Option<&mut Clip> {
        let (li, ci) = self.find_clip(id)?;
        self.layers.get_mut(li)?.clips.get_mut(ci)
    }

    /// Split the clip that the playhead is over on `layer`.
    /// Returns the id of the newly created right-hand section.
    pub fn split_at(&mut self, layer: usize, t: f64) -> Result<u64, String> {
        if self.total_clips() >= MAX_CLIPS {
            return Err(format!(
                "Section limit reached: a project can contain at most {MAX_CLIPS} sections."
            ));
        }
        let some_layer = self
            .layers
            .get(layer)
            .ok_or_else(|| "No layer selected.".to_owned())?;
        let (ci, offset) = some_layer
            .locate(t)
            .ok_or_else(|| "Move the playhead over a clip on the selected layer to split it.".to_owned())?;
        let clip = &some_layer.clips[ci];
        if offset < MIN_CLIP || clip.duration - offset < MIN_CLIP {
            return Err("Too close to the clip edge to split.".to_owned());
        }
        let original_duration = clip.duration;
        let source_in_point = clip.in_point;
        let start = clip.start.max(0.0);
        let mut right = clip.clone();

        let new_id = self.alloc_id();
        right.id = new_id;
        right.duration = original_duration - offset;
        right.start = start + offset;
        if right.kind.is_video() {
            right.in_point = source_in_point + offset;
        }
        {
            let layer_mut = &mut self.layers[layer];
            layer_mut.clips[ci].duration = offset;
            layer_mut.clips.insert(ci + 1, right);
        }
        Ok(new_id)
    }

    pub fn delete_clip(&mut self, id: u64) -> bool {
        if let Some((li, ci)) = self.find_clip(id) {
            self.layers[li].clips.remove(ci);
            true
        } else {
            false
        }
    }

    /// Trim a section's left edge, moving it in time.
    ///
    /// `delta` is in timeline seconds; positive shortens the section from the
    /// left (its start moves right), negative extends it earlier. The returned
    /// value is the delta actually applied after clamping to the available
    /// source material and the minimum clip length.
    pub fn trim_left(&mut self, id: u64, delta: f64, min: f64) -> Option<f64> {
        let (li, ci) = self.find_clip(id)?;
        let speed = self.layers[li].clips[ci].avg_speed();
        let (duration, in_point, kind) = {
            let c = &self.layers[li].clips[ci];
            (c.duration, c.in_point, c.kind)
        };
        let can_shrink = (duration - min).max(0.0);
        // Text and images have no source material, so they can extend freely.
        let can_grow = if kind.is_video() {
            (in_point / speed).max(0.0)
        } else {
            3600.0
        };
        // Positive delta shortens from the left: the section starts later, so
        // its duration shrinks and the source in-point advances.
        let applied = delta.clamp(-can_grow, can_shrink);
        if applied.abs() < 1e-9 {
            return Some(0.0);
        }
        // The left edge moves with the trim, so the section's placement shifts
        // too (sections are no longer packed back-to-back).
        if kind.is_video() {
            let c = &mut self.layers[li].clips[ci];
            c.in_point = (c.in_point + applied * speed).max(0.0);
            c.duration = (c.duration - applied).max(min);
            c.start = (c.start + applied).max(0.0);
        } else {
            let c = &mut self.layers[li].clips[ci];
            c.duration = (duration - applied).max(min);
            c.start = (c.start + applied).max(0.0);
        }
        Some(applied)
    }

    /// Trim a section's right edge. `delta` is in timeline seconds; positive
    /// extends the section, negative shortens it. Clamped to the source length.
    pub fn trim_right(&mut self, id: u64, delta: f64, min: f64) -> Option<f64> {
        let (li, ci) = self.find_clip(id)?;
        let (duration, in_point, speed, source, kind) = {
            let c = &self.layers[li].clips[ci];
            (
                c.duration,
                c.in_point,
                c.avg_speed(),
                c.source_duration,
                c.kind,
            )
        };        let max_dur = if kind.is_video() {
            source
                .map(|sd| ((sd - in_point) / speed).max(min))
                .unwrap_or(3600.0)
        } else {
            3600.0
        };
        let target = (duration + delta).clamp(min, max_dur);
        let applied = target - duration;
        self.layers[li].clips[ci].duration = target;
        Some(applied)
    }

    /// Apply a fade transition between `id` and the next section on the same
    /// track: the outgoing section fades out while the incoming one fades in
    /// over `duration` seconds.
    ///
    /// "Next" means the section that begins after this one ends, so it still
    /// works when sections are not packed back-to-back. The returned duration is
    /// clamped to both neighbours' lengths.
    pub fn fade_transition(&mut self, id: u64, duration: f64) -> Option<f64> {
        let (li, ci) = self.find_clip(id)?;
        let this_start = self.layers[li].clips[ci].start.max(0.0);
        let this_end = this_start + self.layers[li].clips[ci].duration;
        let next = self.layers[li]
            .clips
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != ci && c.start.max(0.0) >= this_end - 1e-3)
            .min_by(|a, b| {
                a.1.start
                    .partial_cmp(&b.1.start)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)?;
        let out_len = self.layers[li].clips[ci].duration;
        let in_len = self.layers[li].clips[next].duration;
        let d = duration.clamp(0.0, (out_len.min(in_len) * 0.9).max(0.0));
        if d <= 0.0 {
            return None;
        }
        self.layers[li].clips[ci].fade_out = d;
        self.layers[li].clips[next].fade_in = d;
        Some(d)
    }

    /// Move a section to `to_layer`, placing its left edge at `start` seconds
    /// from that layer's origin. Other sections are not disturbed.
    pub fn move_clip_to(&mut self, id: u64, to_layer: usize, start: f64) -> bool {
        if to_layer >= self.layers.len() {
            return false;
        }
        let Some((from_layer, from_index)) = self.find_clip(id) else {
            return false;
        };
        let mut clip = self.layers[from_layer].clips.remove(from_index);
        clip.start = start.max(0.0);
        self.layers[to_layer].clips.push(clip);
        true
    }

    /// Retime the whole project to the cumulative playback rate `target`
    /// (2.0 = twice as fast).
    ///
    /// Sections keep the same source range: their timeline length and start
    /// shrink by the ratio while their own speed grows by it. Gaps, markers and
    /// keyframes are scaled the same way, so the edit keeps its arrangement.
    /// Returns the factor that was actually applied (`1.0` = nothing changed).
    pub fn set_global_speed(&mut self, target: f64) -> f64 {
        let target = target.clamp(0.1, 4.0);
        let current = self.speed.clamp(0.1, 4.0);
        let mut ratio = target / current;
        // Do not push any single section outside the supported speed range.
        let mut max_speed: f64 = 0.0;
        let mut min_speed = f64::INFINITY;
        for layer in &self.layers {
            for c in &layer.clips {
                max_speed = max_speed.max(c.speed);
                min_speed = min_speed.min(c.speed);
            }
        }
        if max_speed > 0.0 {
            ratio = ratio.min((8.0 / max_speed).max(1.0));
        }
        if min_speed.is_finite() && min_speed > 0.0 {
            ratio = ratio.max((0.05 / min_speed).min(1.0));
        }
        if (ratio - 1.0).abs() < 1e-9 {
            return 1.0;
        }
        let inv = 1.0 / ratio;
        for layer in &mut self.layers {
            layer.start = (layer.start * inv).max(0.0);
            for c in &mut layer.clips {
                c.start = (c.start * inv).max(0.0);
                c.duration = (c.duration * inv).max(MIN_CLIP);
                c.speed = (c.speed * ratio).clamp(0.05, 8.0);
                c.fade_in = (c.fade_in * inv).clamp(0.0, c.duration);
                c.fade_out = (c.fade_out * inv).clamp(0.0, c.duration);
                for key in &mut c.keyframes {
                    key.time = (key.time * inv).clamp(0.0, c.duration);
                }
            }
        }
        if let Some(m) = &mut self.start_marker {
            *m = (*m * inv).max(0.0);
        }
        for m in &mut self.markers {
            *m = (*m * inv).max(0.0);
        }
        self.markers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        self.speed = (self.speed * ratio).clamp(0.1, 4.0);
        let last = (self.duration() - MIN_CLIP).max(0.0);
        if let Some(m) = &mut self.start_marker {
            *m = m.min(last);
        }
        ratio
    }

    /// Append a clip to a layer, directly after everything already on it.
    #[allow(dead_code)]
    pub fn push_clip(&mut self, layer: usize, mut clip: Clip) -> bool {
        if let Some(l) = self.layers.get_mut(layer) {
            clip.start = l.content_duration();
            l.clips.push(clip);
            true
        } else {
            false
        }
    }

    /// Insert `clip` so that its left edge lands at absolute timeline time `at`.
    ///
    /// Sections are placed freely, so this never shifts or splits anything: the
    /// new section simply starts where the playhead is.
    pub fn insert_clip_at(&mut self, layer: usize, at: f64, mut clip: Clip) -> Result<(), String> {
        let Some(target) = self.layers.get(layer) else {
            return Err("No layer selected.".to_owned());
        };
        if self.total_clips() >= MAX_CLIPS {
            return Err(format!(
                "Section limit reached: a project can contain at most {MAX_CLIPS} sections."
            ));
        }
        let layer_start = target.start.max(0.0);
        clip.start = (at - layer_start).max(0.0);
        self.layers[layer].clips.push(clip);
        Ok(())
    }

    pub fn add_overlay_layer(&mut self) -> Result<usize, String> {
        if self.layers.len() >= MAX_LAYERS {
            return Err(format!("Layer limit reached: at most {MAX_LAYERS} layers."));
        }
        let n = self.layers.len();
        self.layers.push(Layer {
            name: format!("Overlay {n}"),
            clips: Vec::new(),
            transform: Transform::webcam_default(),
            start: 0.0,
            visible: true,
            muted: false,
            solo: false,
            locked: false,
            gain: 1.0,
        });
        Ok(self.layers.len() - 1)
    }

    /// Remove an overlay layer. The base layer (index 0) can never be removed.
    pub fn remove_layer(&mut self, index: usize) -> Result<(), String> {
        if index == 0 {
            return Err("The base layer cannot be removed.".to_owned());
        }
        if index >= self.layers.len() {
            return Err("Invalid layer.".to_owned());
        }
        self.layers.remove(index);
        Ok(())
    }

    /// Raise (dir > 0) or lower (dir < 0) an overlay layer in compositing order.
    pub fn move_layer(&mut self, index: usize, dir: i32) -> bool {
        if index == 0 {
            return false;
        }
        let target = index as i32 + dir;
        if target < 1 || target as usize >= self.layers.len() {
            return false;
        }
        self.layers.swap(index, target as usize);
        true
    }

    /// Move an overlay layer to an absolute compositing index (drag-to-reorder).
    /// The base layer (index 0) always stays at the bottom.
    pub fn move_layer_to(&mut self, from: usize, to: usize) -> bool {
        let len = self.layers.len();
        if from == 0 || from >= len || to == 0 || to >= len || from == to {
            return false;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        true
    }

    /// Remove gaps on `layer`, packing its sections back-to-back in start order.
    /// Returns how many sections moved.
    pub fn close_gaps(&mut self, layer: usize) -> usize {
        let Some(l) = self.layers.get_mut(layer) else {
            return 0;
        };
        let mut order: Vec<usize> = (0..l.clips.len()).collect();
        order.sort_by(|a, b| {
            l.clips[*a]
                .start
                .partial_cmp(&l.clips[*b].start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut cursor = 0.0;
        let mut moved = 0;
        for i in order {
            let c = &mut l.clips[i];
            if (c.start - cursor).abs() > 1e-6 {
                moved += 1;
            }
            c.start = cursor.max(0.0);
            cursor += c.duration;
        }
        moved
    }

    /// Delete several sections and shift later sections on the same layer left
    /// to close the holes they leave (ripple delete). Returns removed count.
    pub fn ripple_delete(&mut self, ids: &[u64]) -> usize {
        let mut removed = 0;
        for layer in &mut self.layers {
            let mut holes: Vec<(f64, f64)> = Vec::new();
            layer.clips.retain(|c| {
                if ids.contains(&c.id) {
                    holes.push((c.start.max(0.0), c.duration));
                    false
                } else {
                    true
                }
            });
            if holes.is_empty() {
                continue;
            }
            removed += holes.len();
            for c in &mut layer.clips {
                let start = c.start.max(0.0);
                let shift: f64 = holes
                    .iter()
                    .filter(|(hs, hd)| hs + hd <= start + 1e-6)
                    .map(|(_, hd)| *hd)
                    .sum();
                c.start = (start - shift).max(0.0);
            }
        }
        removed
    }

    /// Replace `id` with the given timeline-local `keep` ranges (silence removal).
    /// Returns how many sections the original was split into.
    pub fn cut_silence(&mut self, id: u64, keep: &[(f64, f64)]) -> Result<usize, String> {
        let (li, ci) = self
            .find_clip(id)
            .ok_or_else(|| "That section no longer exists.".to_owned())?;
        let base = self.layers[li].clips[ci].clone();
        let extra = keep.len().saturating_sub(1);
        if self.total_clips() + extra > MAX_CLIPS {
            return Err(format!(
                "Section limit reached: a project can contain at most {MAX_CLIPS} sections."
            ));
        }
        let speed = base.avg_speed();
        let mut pieces: Vec<Clip> = Vec::new();
        for (i, (s, e)) in keep.iter().enumerate() {
            let s = s.clamp(0.0, base.duration);
            let e = e.clamp(0.0, base.duration);
            if e - s < MIN_CLIP {
                continue;
            }
            let mut c = base.clone();
            if i > 0 {
                c.id = self.alloc_id();
            }
            c.start = base.start.max(0.0) + s;
            c.duration = e - s;
            if base.kind.is_video() {
                c.in_point = base.in_point + s * speed;
            }
            c.fade_in = c.fade_in.min(c.duration);
            c.fade_out = c.fade_out.min(c.duration);
            if !base.keyframes.is_empty() {
                c.keyframes = base
                    .keyframes
                    .iter()
                    .filter(|k| k.time >= s && k.time <= e)
                    .map(|k| Keyframe {
                        time: k.time - s,
                        transform: k.transform,
                    })
                    .collect();
            }
            pieces.push(c);
        }
        if pieces.is_empty() {
            return Err("The section is entirely silent.".to_owned());
        }
        let n = pieces.len();
        self.layers[li].clips.remove(ci);
        for (k, p) in pieces.into_iter().enumerate() {
            self.layers[li].clips.insert(ci + k, p);
        }
        Ok(n)
    }

    pub fn sanitize(&mut self) -> Result<(), String> {
        if self.layers.is_empty() {
            self.layers.push(Layer {
                name: "Base".to_owned(),
                clips: Vec::new(),
                transform: Transform::default(),
                start: 0.0,
                visible: true,
                muted: false,
                solo: false,
                locked: false,
                gain: 1.0,
            });
        }
        if self.layers.len() > MAX_LAYERS {
            return Err(format!("Project has more than {MAX_LAYERS} layers."));
        }
        if self.total_clips() > MAX_CLIPS {
            return Err(format!("Project has more than {MAX_CLIPS} sections."));
        }
        self.width = self.width.max(16) & !1;
        self.height = self.height.max(16) & !1;
        if self.fps == 0 {
            self.fps = 30;
        }
        self.speed = self.speed.clamp(0.1, 4.0);
        let mut max_id = 0;
        // Version 1 stored a single transform per layer; copy it onto every
        // section so older projects keep looking the same after upgrading.
        let migrate_transforms = self.version < 2;
        let mutate_start = self.version < 3;
        for layer in &mut self.layers {
            layer.start = layer.start.max(0.0);
            layer.gain = layer.gain.clamp(0.0, 4.0);
            layer.transform.scale = layer.transform.scale.clamp(0.02, 4.0);
            layer.transform.opacity = layer.transform.opacity.clamp(0.0, 1.0);
            // Version 2 and older packed sections back-to-back, so their start
            // offset was implied. Give each section an explicit start now.
            if mutate_start {
                let mut acc = 0.0;
                for clip in &mut layer.clips {
                    clip.start = acc;
                    acc += clip.duration.max(MIN_CLIP);
                }
            }
            for clip in &mut layer.clips {
                if !clip.start.is_finite() {
                    clip.start = 0.0;
                }
                clip.start = clip.start.max(0.0);
                if migrate_transforms {
                    clip.transform = layer.transform;
                }
                clip.transform.scale = clip.transform.scale.clamp(0.02, 4.0);
                clip.transform.opacity = clip.transform.opacity.clamp(0.0, 1.0);
                clip.speed = clip.speed.clamp(0.05, 8.0);
                clip.speed_end = clip.speed_end.map(|s| s.clamp(0.05, 8.0));
                if clip
                    .speed_end
                    .map(|s| (s - clip.speed).abs() < 1e-4)
                    .unwrap_or(false)
                {
                    clip.speed_end = None;
                }
                if clip.freeze {
                    // A held frame has no motion to reverse or ramp.
                    clip.reverse = false;
                    clip.speed_end = None;
                }
                clip.volume = clip.volume.clamp(0.0, 4.0);
                clip.brightness = clip.brightness.clamp(-1.0, 1.0);
                clip.contrast = clip.contrast.clamp(0.0, 3.0);
                clip.saturation = clip.saturation.clamp(0.0, 3.0);
                clip.blur = clip.blur.clamp(0.0, 50.0);
                clip.rotation = clip.rotation.clamp(-180.0, 180.0);
                clip.crop_l = clip.crop_l.clamp(0.0, 0.45);
                clip.crop_r = clip.crop_r.clamp(0.0, 0.45);
                clip.crop_t = clip.crop_t.clamp(0.0, 0.45);
                clip.crop_b = clip.crop_b.clamp(0.0, 0.45);
                // Never crop away the entire frame.
                if clip.crop_l + clip.crop_r > 0.9 {
                    let k = 0.9 / (clip.crop_l + clip.crop_r);
                    clip.crop_l *= k;
                    clip.crop_r *= k;
                }
                if clip.crop_t + clip.crop_b > 0.9 {
                    let k = 0.9 / (clip.crop_t + clip.crop_b);
                    clip.crop_t *= k;
                    clip.crop_b *= k;
                }
                clip.pixelate = clip.pixelate.clamp(0.0, 64.0);
                clip.sharpen = clip.sharpen.clamp(0.0, 1.5);
                clip.vignette = clip.vignette.clamp(0.0, 1.0);
                clip.duration = clip.duration.max(MIN_CLIP);
                clip.fade_in = clip.fade_in.clamp(0.0, clip.duration);
                clip.fade_out = clip.fade_out.clamp(0.0, clip.duration);
                // Keep keyframes ordered and inside the section.
                for key in &mut clip.keyframes {
                    key.time = key.time.clamp(0.0, clip.duration);
                    key.transform.scale = key.transform.scale.clamp(0.02, 4.0);
                    key.transform.opacity = key.transform.opacity.clamp(0.0, 1.0);
                }
                clip.keyframes.sort_by(|a, b| {
                    a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal)
                });
                clip.in_point = clip.in_point.max(0.0);
                if let Some(sd) = clip.source_duration {
                    let span = clip.source_span();
                    clip.in_point = clip.in_point.min((sd - MIN_CLIP).max(0.0));
                    // Do not run off the end of the source.
                    let max_span = (sd - clip.in_point).max(MIN_CLIP);
                    if span > max_span {
                        clip.duration = (max_span / clip.speed.max(0.05)).max(MIN_CLIP);
                    }
                    clip.fade_in = clip.fade_in.clamp(0.0, clip.duration);
                    clip.fade_out = clip.fade_out.clamp(0.0, clip.duration);
                }
                if clip.is_text() && clip.text.is_none() {
                    clip.text = Some(TextSpec::new("Text"));
                }
                if let Some(text) = &mut clip.text {
                    text.size = text.size.clamp(8.0, 400.0);
                    text.outline = text.outline.clamp(0.0, 80.0);
                    text.shadow = text.shadow.clamp(0.0, 80.0);
                }
                max_id = max_id.max(clip.id);
            }
        }
        self.version = PROJECT_VERSION;
        self.next_id = self.next_id.max(max_id + 1);
        let total = self.duration();
        self.start_marker = self
            .start_marker
            .map(|m| m.clamp(0.0, (total - MIN_CLIP).max(0.0)))
            .filter(|_| total > MIN_CLIP);
        // Markers: drop anything outside the project, dedupe and sort.
        self.markers.retain(|m| *m >= 0.0 && *m <= total);
        self.markers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        self.markers.dedup_by(|a, b| (*a - *b).abs() < 0.001);
        if self.name.trim().is_empty() {
            self.name = "Untitled".to_owned();
        }
        Ok(())
    }
}

pub fn is_image_path(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" | "tif" | "tiff" | "avif"
    )
}

pub fn is_media_path(path: &std::path::Path) -> bool {
    if is_image_path(path) {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "mts" | "m2ts" | "mpg" | "mpeg"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn clip(id: u64, duration: f64) -> Clip {
        Clip {
            id,
            path: PathBuf::from(format!("c{id}.mp4")),
            name: format!("clip{id}"),
            kind: MediaKind::Video,
            in_point: 0.0,
            duration,
            source_duration: Some(100.0),
            has_audio: true,
            ..Default::default()
        }
    }

    fn photo(id: u64, duration: f64) -> Clip {
        Clip {
            kind: MediaKind::Image,
            source_duration: None,
            has_audio: false,
            ..clip(id, duration)
        }
    }

    #[test]
    fn split_creates_two_sections_and_trims_source_in_point() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 10.0));
        let new_id = p.split_at(0, 4.0).unwrap();
        assert_eq!(p.layers[0].clips.len(), 2);
        assert_eq!(p.layers[0].clips[0].duration, 4.0);
        assert_eq!(p.layers[0].clips[1].duration, 6.0);
        assert_eq!(p.layers[0].clips[1].in_point, 4.0);
        assert_eq!(p.layers[0].clips[1].id, new_id);
    }

    #[test]
    fn split_rejects_edges_and_empty_timeline() {
        let mut p = Project::new();
        assert!(p.split_at(0, 0.0).is_err());
        p.push_clip(0, clip(1, 10.0));
        assert!(p.split_at(0, 0.001).is_err());
        assert!(p.split_at(0, 9.999).is_err());
        assert!(p.split_at(0, 5.0).is_ok());
    }

    #[test]
    fn project_enforces_100_section_limit() {
        let mut p = Project::new();
        for i in 0..MAX_CLIPS {
            p.push_clip(0, clip(i as u64 + 1, 1.0));
        }
        p.next_id = MAX_CLIPS as u64 + 1;
        assert_eq!(p.total_clips(), 100);
        assert!(p.split_at(0, 0.5).is_err());
    }

    #[test]
    fn delete_removes_the_selected_section() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 2.0));
        p.push_clip(0, clip(2, 3.0));
        assert!(p.delete_clip(1));
        assert_eq!(p.total_clips(), 1);
        assert_eq!(p.layers[0].clips[0].id, 2);
        assert!(!p.delete_clip(999));
    }

    #[test]
    fn reorder_within_and_across_layers() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 1.0));
        p.push_clip(0, clip(2, 1.0));
        p.push_clip(0, clip(3, 1.0));
        // Move clip 3 to the front of the timeline.
        assert!(p.move_clip_to(3, 0, 0.0));
        let placed: Vec<(u64, f64)> = p.layers[0]
            .clips
            .iter()
            .map(|c| (c.id, c.start))
            .collect();
        assert_eq!(placed, vec![(1, 0.0), (2, 1.0), (3, 0.0)]);

        let overlay = p.add_overlay_layer().unwrap();
        assert!(p.move_clip_to(2, overlay, 4.0));
        assert_eq!(p.layers[overlay].clips.len(), 1);
        assert!((p.layers[overlay].clips[0].start - 4.0).abs() < 1e-9);
        assert_eq!(p.layers[0].clips.len(), 2);
    }

    #[test]
    fn layers_are_limited_and_base_is_protected() {
        let mut p = Project::new();
        for _ in 1..MAX_LAYERS {
            assert!(p.add_overlay_layer().is_ok());
        }
        assert!(p.add_overlay_layer().is_err());
        assert!(p.remove_layer(0).is_err());
        assert!(p.remove_layer(1).is_ok());
    }

    #[test]
    fn drag_reorder_keeps_base_at_the_bottom() {
        let mut p = Project::new();
        p.add_overlay_layer().unwrap();
        p.add_overlay_layer().unwrap();
        p.layers[1].name = "A".into();
        p.layers[2].name = "B".into();
        assert!(p.move_layer_to(2, 1));
        assert_eq!(p.layers[1].name, "B");
        assert_eq!(p.layers[2].name, "A");
        assert!(p.move_layer_to(1, 2));
        assert_eq!(p.layers[1].name, "A");
        // Base can never move and nothing can be moved onto index 0.
        assert!(!p.move_layer_to(0, 1));
        assert!(!p.move_layer_to(1, 0));
        assert_eq!(p.layers[0].name, "Base");
    }

    #[test]
    fn layer_start_offsets_timeline_positions() {
        let mut c2 = clip(2, 3.0);
        c2.start = 2.0;
        let mut layer = Layer {
            name: "Overlay".into(),
            clips: vec![clip(1, 2.0), c2],
            transform: Transform::webcam_default(),
            start: 5.0,
            visible: true,
            muted: false,
            solo: false,
            locked: false,
            gain: 1.0,
        };
        assert_eq!(layer.duration(), 10.0);
        assert_eq!(layer.clip_start(1), 7.0);
        assert!(layer.locate(4.0).is_none());
        assert_eq!(layer.locate(5.5).unwrap(), (0, 0.5));
        assert_eq!(layer.locate(8.0).unwrap(), (1, 1.0));
        layer.start = -3.0;
        let mut p = Project::new();
        p.layers.push(layer);
        p.sanitize().unwrap();
        assert_eq!(p.layers[1].start, 0.0);
    }

    #[test]
    fn total_duration_uses_the_longest_layer() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 10.0));
        let o = p.add_overlay_layer().unwrap();
        p.push_clip(o, clip(2, 4.0));
        assert!((p.duration() - 10.0).abs() < 1e-9);
        p.push_clip(o, clip(3, 20.0));
        assert!((p.duration() - 24.0).abs() < 1e-9);
    }

    #[test]
    fn photos_and_videos_round_trip_through_json() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 4.0));
        p.push_clip(0, photo(2, IMAGE_DEFAULT_DURATION));
        let json = serde_json::to_string(&p).unwrap();
        let mut back: Project = serde_json::from_str(&json).unwrap();
        back.sanitize().unwrap();
        assert_eq!(back.layers[0].clips[1].kind, MediaKind::Image);
        assert_eq!(back.total_clips(), 2);
        assert!(back.total_clips() <= MAX_CLIPS);
    }

    #[test]
    fn sanitize_repairs_bad_values() {
        let mut p = Project::new();
        p.width = 0;
        p.height = 1;
        p.fps = 0;
        p.layers[0].clips.push(Clip {
            duration: -5.0,
            ..clip(1, 1.0)
        });
        p.sanitize().unwrap();
        assert!(p.width >= 16 && p.width % 2 == 0);
        assert!(p.height >= 16 && p.height % 2 == 0);
        assert_eq!(p.fps, 30);
        assert!(p.layers[0].clips[0].duration >= MIN_CLIP);
    }

    #[test]
    fn sanitize_clamps_effects() {
        let mut p = Project::new();
        let mut c = clip(1, 2.0);
        c.brightness = -9.0;
        c.contrast = 99.0;
        c.saturation = -3.0;
        c.blur = 1000.0;
        p.push_clip(0, c);
        p.sanitize().unwrap();
        let c = &p.layers[0].clips[0];
        assert!((c.brightness + 1.0).abs() < 1e-9);
        assert!((c.contrast - 3.0).abs() < 1e-9);
        assert!(c.saturation.abs() < 1e-9);
        assert!((c.blur - 50.0).abs() < 1e-9);
    }

    #[test]
    fn sanitize_clamps_crop_and_stylise() {
        let mut p = Project::new();
        let mut c = clip(1, 2.0);
        c.crop_l = 0.8;
        c.crop_r = 0.8;
        c.crop_t = -1.0;
        c.crop_b = 0.3;
        c.pixelate = 999.0;
        c.sharpen = 9.0;
        c.vignette = -2.0;
        p.push_clip(0, c);
        p.sanitize().unwrap();
        let c = &p.layers[0].clips[0];
        // Horizontal crop is rescaled so it can never remove the whole frame.
        assert!(c.crop_l + c.crop_r <= 0.900_001);
        assert!(c.crop_l > 0.0 && c.crop_r > 0.0);
        assert!(c.crop_t.abs() < 1e-9);
        assert!((c.crop_b - 0.3).abs() < 1e-9);
        assert!((c.pixelate - 64.0).abs() < 1e-9);
        assert!((c.sharpen - 1.5).abs() < 1e-9);
        assert!(c.vignette.abs() < 1e-9);
    }

    #[test]
    fn markers_are_sorted_clamped_and_deduped() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 6.0));

        assert!(p.add_marker(4.0));
        assert!(p.add_marker(1.0));
        assert!(p.add_marker(2.5));
        // Duplicate marker is rejected.
        assert!(!p.add_marker(2.5));
        p.markers.push(-4.0);
        p.markers.push(500.0);
        p.sanitize().unwrap();
        assert_eq!(p.markers, vec![1.0, 2.5, 4.0]);
        assert!(p.remove_marker_near(2.52, 0.2));
        assert_eq!(p.markers, vec![1.0, 4.0]);
    }

    #[test]
    fn media_path_detection() {
        assert!(is_image_path(Path::new("a.PNG")));
        assert!(!is_image_path(Path::new("a.mp4")));
        assert!(is_media_path(Path::new("a.mov")));
        assert!(!is_media_path(Path::new("a.txt")));
    }

    #[test]
    fn start_marker_is_clamped_and_cleared_for_empty_projects() {
        let mut p = Project::new();
        // No media: a marker cannot be meaningful.
        p.start_marker = Some(5.0);
        p.sanitize().unwrap();
        assert_eq!(p.start_marker, None);

        p.push_clip(0, clip(1, 10.0));
        p.start_marker = Some(4.0);
        p.sanitize().unwrap();
        assert_eq!(p.start_marker, Some(4.0));

        // Past the end: clamped to the last legal split-ish point.
        p.start_marker = Some(999.0);
        p.sanitize().unwrap();
        let marker = p.start_marker.unwrap();
        assert!(marker <= p.duration() - MIN_CLIP + 1e-9);

        // Negative markers are clamped to zero.
        p.start_marker = Some(-3.0);
        p.sanitize().unwrap();
        assert_eq!(p.start_marker, Some(0.0));

        // The marker survives a JSON round-trip.
        let json = serde_json::to_string(&p).unwrap();
        let mut back: Project = serde_json::from_str(&json).unwrap();
        back.sanitize().unwrap();
        assert_eq!(back.start_marker, Some(0.0));
        p.start_marker = None;
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("start_marker"));
    }

    #[test]
    fn insert_at_playhead_places_media_without_moving_others() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 10.0));
        p.next_id = 3;
        p.insert_clip_at(0, 4.0, clip(2, 2.0)).unwrap();
        let ids: Vec<u64> = p.layers[0].clips.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 2]);
        // The existing section is untouched; the new one simply starts at 4 s.
        assert_eq!(p.layers[0].clips[0].duration, 10.0);
        assert_eq!(p.layers[0].clips[0].start, 0.0);
        assert_eq!(p.layers[0].clips[1].start, 4.0);
        assert_eq!(p.layers[0].clips[1].duration, 2.0);
    }

    #[test]
    fn sections_can_be_placed_freely_with_gaps() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 2.0));
        p.push_clip(0, clip(2, 3.0));
        // Move the second section to 10 s: the layer now has a 8 s gap.
        assert!(p.move_clip_to(2, 0, 10.0));
        let l = &p.layers[0];
        assert_eq!(l.content_duration(), 13.0);
        assert!(l.locate(5.0).is_none(), "gap must not contain a section");
        assert_eq!(l.locate(10.5).unwrap(), (1, 0.5));
        assert_eq!(p.duration(), 13.0);
    }

    #[test]
    fn overlapping_sections_keep_the_later_one_on_top() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 4.0));
        p.next_id = 3;
        p.insert_clip_at(0, 2.0, clip(2, 4.0)).unwrap();
        // Inside the overlap the most recently added section wins.
        assert_eq!(p.layers[0].locate(3.0).unwrap().0, 1);
        assert_eq!(p.layers[0].locate(0.5).unwrap().0, 0);
        assert_eq!(p.layers[0].locate(5.5).unwrap().0, 1);
    }

    #[test]
    fn version_2_projects_migrate_to_sequential_starts() {
        let mut p = Project::new();
        p.version = 2;
        p.layers[0].clips.push(clip(1, 2.0));
        p.layers[0].clips.push(clip(2, 3.0));
        p.layers[0].clips.push(clip(3, 1.0));
        p.sanitize().unwrap();
        let starts: Vec<f64> = p.layers[0].clips.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![0.0, 2.0, 5.0]);
        assert_eq!(p.version, PROJECT_VERSION);
    }

    #[test]
    fn insert_on_an_empty_layer_starts_at_the_playhead() {
        let mut p = Project::new();
        let overlay = p.add_overlay_layer().unwrap();
        p.insert_clip_at(overlay, 36.79, clip(9, 3.0)).unwrap();
        assert_eq!(p.layers[overlay].start, 0.0);
        assert_eq!(p.layers[overlay].clips[0].id, 9);
        assert!((p.layers[overlay].clip_start(0) - 36.79).abs() < 1e-9);
        assert!((p.duration() - 39.79).abs() < 1e-9);
    }

    #[test]
    fn fade_transition_touches_both_neighbours() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 4.0));
        p.push_clip(0, clip(2, 2.0));
        let d = p.fade_transition(1, 1.5).unwrap();
        assert!((d - 1.5).abs() < 1e-9);
        assert!((p.layers[0].clips[0].fade_out - 1.5).abs() < 1e-9);
        assert!((p.layers[0].clips[1].fade_in - 1.5).abs() < 1e-9);
        // No following section -> no transition.
        assert!(p.fade_transition(2, 1.0).is_none());
        // A duration longer than either clip is clamped to 90% of the shorter.
        let d = p.fade_transition(1, 99.0).unwrap();
        assert!((d - 1.8).abs() < 1e-6);
    }

    #[test]
    fn trim_left_moves_the_section_and_keeps_source_in_point() {
        let mut p = Project::new();
        let mut c = clip(1, 10.0);
        c.in_point = 4.0;
        c.source_duration = Some(100.0);
        p.push_clip(0, c);

        // Shorten from the left by 2s: duration 8, in-point 6.
        let applied = p.trim_left(1, 2.0, MIN_CLIP).unwrap();
        assert!((applied - 2.0).abs() < 1e-9);
        let c = &p.layers[0].clips[0];
        assert!((c.duration - 8.0).abs() < 1e-9);
        assert!((c.in_point - 6.0).abs() < 1e-9);

        // Extending back past the source start is clamped to in_point = 0.
        let applied = p.trim_left(1, -100.0, MIN_CLIP).unwrap();
        assert!((applied + 6.0).abs() < 1e-6);
        let c = &p.layers[0].clips[0];
        assert!(c.in_point.abs() < 1e-9);
        assert!((c.duration - 14.0).abs() < 1e-6);

        // Shrinking can never go below the minimum length.
        let applied = p.trim_left(1, 100.0, MIN_CLIP).unwrap();
        assert!((applied - (14.0 - MIN_CLIP)).abs() < 1e-6);
        assert!((p.layers[0].clips[0].duration - MIN_CLIP).abs() < 1e-9);
    }

    #[test]
    fn trim_right_clamps_to_available_source() {
        let mut p = Project::new();
        let mut c = clip(1, 6.0);
        c.in_point = 2.0;
        c.source_duration = Some(10.0);
        p.push_clip(0, c);

        // Extend by 2s: 8s long, and that is exactly all remaining source.
        let applied = p.trim_right(1, 2.0, MIN_CLIP).unwrap();
        assert!((applied - 2.0).abs() < 1e-9);
        assert!((p.layers[0].clips[0].duration - 8.0).abs() < 1e-9);

        // Asking for more is clamped (source ends at 10s, in-point at 2s).
        p.trim_right(1, 50.0, MIN_CLIP);
        assert!((p.layers[0].clips[0].duration - 8.0).abs() < 1e-6);

        // Shorten by 3s.
        let applied = p.trim_right(1, -3.0, MIN_CLIP).unwrap();
        assert!((applied + 3.0).abs() < 1e-9);
        assert!((p.layers[0].clips[0].duration - 5.0).abs() < 1e-9);
    }

    #[test]
    fn trim_helpers_respect_speed() {
        let mut p = Project::new();
        let mut c = clip(1, 4.0);
        c.in_point = 2.0;
        c.source_duration = Some(100.0);
        c.speed = 2.0;
        p.push_clip(0, c);
        // At 2x, 4s of timeline uses 8s of source. Extending 2s needs 4s more
        // source, which is available.
        p.trim_right(1, 2.0, MIN_CLIP);
        assert!((p.layers[0].clips[0].duration - 6.0).abs() < 1e-9);
        // Trimming the left by 1s advances the in-point by 2s (speed * delta).
        p.trim_left(1, 1.0, MIN_CLIP);
        assert!((p.layers[0].clips[0].in_point - 4.0).abs() < 1e-9);
        assert!((p.layers[0].clips[0].duration - 5.0).abs() < 1e-9);
    }

    #[test]
    fn global_speed_retimes_the_whole_timeline() {
        let mut p = Project::new();
        let mut a = clip(1, 4.0);
        a.keyframes.push(Keyframe { time: 2.0, transform: Transform::default() });
        p.push_clip(0, a);
        let mut b = clip(2, 2.0);
        b.start = 6.0;
        p.layers[0].clips.push(b);
        p.markers.push(5.0);
        p.start_marker = Some(1.0);
        assert!((p.duration() - 8.0).abs() < 1e-9);

        // 2x: everything is half as long and plays twice as fast.
        let ratio = p.set_global_speed(2.0);
        assert!((ratio - 2.0).abs() < 1e-9);
        let l = &p.layers[0];
        assert!((l.clips[0].duration - 2.0).abs() < 1e-9);
        assert!((l.clips[0].speed - 2.0).abs() < 1e-9);
        assert!((l.clips[1].start - 3.0).abs() < 1e-9);
        assert!((l.clips[1].duration - 1.0).abs() < 1e-9);
        assert!((l.clips[0].keyframes[0].time - 1.0).abs() < 1e-9);
        assert!((p.markers[0] - 2.5).abs() < 1e-9);
        assert!((p.start_marker.unwrap() - 0.5).abs() < 1e-9);
        assert!((p.duration() - 4.0).abs() < 1e-9);
        assert!((p.speed - 2.0).abs() < 1e-9);

        // Back to normal restores the original arrangement.
        assert!((p.set_global_speed(1.0) - 0.5).abs() < 1e-9);
        assert!((p.duration() - 8.0).abs() < 1e-9);
        assert!((p.layers[0].clips[0].speed - 1.0).abs() < 1e-9);
        assert!((p.speed - 1.0).abs() < 1e-9);
    }

    #[test]
    fn global_speed_never_pushes_sections_out_of_range() {
        let mut p = Project::new();
        let mut c = clip(1, 4.0);
        c.speed = 6.0;
        p.push_clip(0, c);
        // 4x would need a 24x section speed, so the retime stops at 8x.
        let ratio = p.set_global_speed(4.0);
        assert!((ratio - (8.0 / 6.0)).abs() < 1e-9);
        assert!((p.layers[0].clips[0].speed - 8.0).abs() < 1e-9);
        assert!((p.speed - (8.0 / 6.0)).abs() < 1e-9);
    }

    #[test]
    fn moving_sections_between_layers_keeps_their_place() {
        let mut p = Project::new();
        p.push_clip(0, clip(1, 5.0));
        p.insert_clip_at(0, 6.0, clip(2, 1.0)).unwrap();
        let overlay = p.add_overlay_layer().unwrap();
        assert!(p.move_clip_to(2, overlay, 2.5));
        assert_eq!(p.layers[0].clips.len(), 1);
        assert_eq!(p.layers[overlay].clips.len(), 1);
        assert!((p.layers[overlay].clips[0].start - 2.5).abs() < 1e-9);
    }

    #[test]
    fn clip_transforms_are_per_section_and_migrate_from_the_layer() {
        // A version 1 project stores the transform on the layer only.
        let mut p = Project::new();
        p.version = 1;
        p.layers[0].transform = Transform::webcam_default();
        p.layers[0].clips.push(clip(1, 2.0));
        p.layers[0].clips.push(clip(2, 2.0));
        p.sanitize().unwrap();
        assert_eq!(p.version, PROJECT_VERSION);
        assert_eq!(p.layers[0].clips[0].transform, Transform::webcam_default());
        assert_eq!(p.layers[0].clips[1].transform, Transform::webcam_default());

        // Changing one section no longer affects the other.
        p.layers[0].clips[0].transform = Transform::default();
        assert_eq!(p.layers[0].clips[1].transform, Transform::webcam_default());
    }

    #[test]
    fn clip_transform_survives_a_json_round_trip() {
        let mut p = Project::new();
        let mut c = clip(1, 2.0);
        c.transform = Transform::webcam_default();
        p.push_clip(0, c);
        let json = serde_json::to_string(&p).unwrap();
        let mut back: Project = serde_json::from_str(&json).unwrap();
        back.sanitize().unwrap();
        assert_eq!(back.layers[0].clips[0].transform, Transform::webcam_default());
    }

    #[test]
    fn close_gaps_packs_sections_back_to_back() {
        let mut p = Project::new();
        let mut a = clip(1, 2.0);
        a.start = 0.0;
        let mut b = clip(2, 3.0);
        b.start = 5.0;
        let mut c = clip(3, 1.0);
        c.start = 12.0;
        p.layers[0].clips = vec![c, a, b];
        assert_eq!(p.close_gaps(0), 2);
        let mut starts: Vec<f64> = p.layers[0].clips.iter().map(|c| c.start).collect();
        starts.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert_eq!(starts, vec![0.0, 2.0, 5.0]);
    }

    #[test]
    fn ripple_delete_closes_the_holes() {
        let mut p = Project::new();
        for (i, start) in [(1u64, 0.0), (2, 2.0), (3, 4.0), (4, 6.0)] {
            let mut c = clip(i, 2.0);
            c.start = start;
            p.layers[0].clips.push(c);
        }
        assert_eq!(p.ripple_delete(&[2, 3]), 2);
        let starts: Vec<f64> = p.layers[0].clips.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![0.0, 2.0]);
        assert_eq!(p.layers[0].clips[1].id, 4);
    }

    #[test]
    fn speed_ramp_averages_source_consumption() {
        let mut c = clip(1, 4.0);
        c.speed = 1.0;
        c.speed_end = Some(3.0);
        assert!((c.avg_speed() - 2.0).abs() < 1e-9);
        assert!((c.source_span() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn cut_silence_splits_a_section_into_kept_ranges() {
        let mut p = Project::new();
        let mut c = clip(1, 10.0);
        c.start = 0.0;
        p.layers[0].clips.push(c);
        let n = p.cut_silence(1, &[(0.0, 3.0), (7.0, 10.0)]).unwrap();
        assert_eq!(n, 2);
        assert_eq!(p.layers[0].clips.len(), 2);
        assert!((p.layers[0].clips[0].duration - 3.0).abs() < 1e-9);
        assert!((p.layers[0].clips[1].start - 7.0).abs() < 1e-9);
        assert!((p.layers[0].clips[1].in_point - 7.0).abs() < 1e-9);
    }
}

