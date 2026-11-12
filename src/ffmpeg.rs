//! FFmpeg integration: media probing, filter graph construction, thumbnails and export.
//!
//! RunVideo deliberately shells out to FFmpeg instead of linking libav*: it keeps the
//! application tiny, avoids unsafe decoding code, and makes a crash in a decoder unable
//! to take the editor down with it.

use crate::model::{
    is_image_path, Clip, Keyframe, MediaKind, Project, TextAlign, Transform, VAlign,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Drop a child process to below-normal priority so an export never starves
/// the editor (or the rest of Windows) of CPU time.
#[cfg(windows)]
fn lower_process_priority(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetPriorityClass(process: *mut core::ffi::c_void, class: u32) -> i32;
    }
    // BELOW_NORMAL_PRIORITY_CLASS
    const BELOW_NORMAL: u32 = 0x0000_4000;
    let handle = child.as_raw_handle() as *mut core::ffi::c_void;
    // Best-effort: a failure here is harmless.
    unsafe {
        SetPriorityClass(handle, BELOW_NORMAL);
    }
}

/// Spawn a child at below-normal priority. Used for every FFmpeg render so it
/// can never starve the editor, the compositor or the rest of Windows.
pub fn spawn_quiet(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    let child = cmd.spawn()?;
    #[cfg(windows)]
    lower_process_priority(&child);
    Ok(child)
}

pub fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn ffmpeg_version() -> Option<String> {
    let out = command("ffmpeg")
        .args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().unwrap_or("ffmpeg").to_owned())
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub kind: MediaKind,
    pub duration: Option<f64>,
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
    pub height: u32,
    pub has_audio: bool,
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let is_image = is_image_path(path);
    let out = command("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("Could not run ffprobe. Is FFmpeg installed and on PATH?")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("ffprobe failed: {}", err.trim());
    }
    let json: Value = serde_json::from_slice(&out.stdout).context("Invalid ffprobe output")?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();

    let mut has_video = false;
    let mut has_audio = false;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut stream_duration: Option<f64> = None;
    for s in &streams {
        match s["codec_type"].as_str() {
            Some("video") => {
                // Attached cover art is not a real video stream.
                if s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 1 {
                    continue;
                }
                has_video = true;
                width = s["width"].as_u64().unwrap_or(0) as u32;
                height = s["height"].as_u64().unwrap_or(0) as u32;
                if stream_duration.is_none() {
                    stream_duration = s["duration"].as_str().and_then(|d| d.parse().ok());
                }
                // Respect rotation metadata for display aspect where possible.
                if let Some(rot) = s["tags"]["rotate"].as_str().and_then(|v| v.parse::<i64>().ok()) {
                    if rot.abs() % 180 == 90 {
                        std::mem::swap(&mut width, &mut height);
                    }
                }
                if let Some(side) = s["side_data_list"].as_array() {
                    for sd in side {
                        if let Some(rot) = sd["rotation"].as_f64() {
                            if rot.abs() % 180.0 > 45.0 {
                                std::mem::swap(&mut width, &mut height);
                            }
                        }
                    }
                }
            }
            Some("audio") => has_audio = true,
            _ => {}
        }
    }
    if !has_video {
        bail!("{}: no video stream found", path.display());
    }

    let format_duration = json["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok());
    let duration = if is_image {
        None
    } else {
        format_duration.or(stream_duration).filter(|d| *d > 0.0)
    };

    Ok(MediaInfo {
        kind: if is_image { MediaKind::Image } else { MediaKind::Video },
        duration,
        width,
        height,
        has_audio: has_audio && !is_image,
    })
}

/// Extract a small PNG thumbnail from any media file.
pub fn thumbnail(path: &Path) -> Result<Vec<u8>> {
    let make = |seek: bool| -> Result<Vec<u8>> {
        let mut cmd = command("ffmpeg");
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
        if seek {
            cmd.args(["-ss", "0.1"]);
        }
        cmd.arg("-i")
            .arg(path)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=160:-2:force_original_aspect_ratio=decrease",
                "-an",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-",
            ])
            .stdin(Stdio::null());
        let out = cmd.output().context("Could not run ffmpeg for thumbnail")?;
        if !out.status.success() || out.stdout.is_empty() {
            bail!("thumbnail extraction failed");
        }
        Ok(out.stdout)
    };
    if is_image_path(path) {
        make(false)
    } else {
        make(true).or_else(|_| make(false))
    }
}

// ---------------------------------------------------------------------------
// Audio helpers: waveform, silence detection, loudness
// ---------------------------------------------------------------------------

/// Peak-per-bucket waveform for a media file, normalised to 0..1.
///
/// Decoding at 2 kHz mono is tiny (8 KB per second) and plenty for a timeline
/// drawing. `duration_hint` caps very long files.
pub fn waveform(path: &Path, duration_hint: Option<f64>, buckets: usize) -> Result<Vec<f32>> {
    let buckets = buckets.clamp(64, 8192);
    let mut cmd = command("ffmpeg");
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
    if let Some(d) = duration_hint {
        let t = (d + 1.0).clamp(0.1, 3600.0);
        cmd.args(["-t", &format!("{t:.3}")]);
    }
    cmd.arg("-i")
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "2000", "-f", "f32le", "-"])
        .stdin(Stdio::null());
    let out = cmd.output().context("Could not run ffmpeg for the waveform")?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("waveform: no decodable audio");
    }
    let samples = out.stdout.len() / 4;
    if samples == 0 {
        bail!("waveform: no audio samples");
    }
    let mut peaks = vec![0f32; buckets];
    for (i, chunk) in out.stdout.chunks_exact(4).enumerate() {
        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]).abs();
        let bi = (i * buckets / samples).min(buckets - 1);
        if v > peaks[bi] {
            peaks[bi] = v;
        }
    }
    let max = peaks.iter().copied().fold(0.0f32, f32::max);
    if max > 1e-6 {
        for p in &mut peaks {
            *p = (*p / max).clamp(0.0, 1.0);
        }
    }
    Ok(peaks)
}

/// Detect silent ranges inside `start .. start+duration` of a media file.
/// Returned ranges are seconds relative to `start`.
pub fn detect_silence(
    path: &Path,
    start: f64,
    duration: f64,
    noise_db: f64,
    min_silence: f64,
) -> Result<Vec<(f64, f64)>> {
    let mut cmd = command("ffmpeg");
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "info"])
        .args(["-ss", &format!("{start:.6}")])
        .args(["-t", &format!("{duration:.6}")])
        .arg("-i")
        .arg(path)
        .args([
            "-vn",
            "-af",
            &format!("silencedetect=noise={noise_db:.1}dB:d={min_silence:.3}"),
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null());
    let out = cmd.output().context("Could not run silence detection")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("silence detection failed: {}", err.trim());
    }
    Ok(parse_silence(&String::from_utf8_lossy(&out.stderr), duration))
}

fn parse_silence(text: &str, duration: f64) -> Vec<(f64, f64)> {
    let mut ranges = Vec::new();
    let mut cur: Option<f64> = None;
    for line in text.lines() {
        if let Some(idx) = line.find("silence_start:") {
            if let Some(v) = line[idx + "silence_start:".len()..]
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok())
            {
                cur = Some(v.max(0.0));
            }
        } else if let Some(idx) = line.find("silence_end:") {
            let v = line[idx + "silence_end:".len()..]
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if let Some(s) = cur.take() {
                ranges.push((s.clamp(0.0, duration), v.clamp(0.0, duration)));
            }
        }
    }
    if let Some(s) = cur {
        ranges.push((s.clamp(0.0, duration), duration));
    }
    ranges
        .into_iter()
        .filter(|(a, b)| *b > *a + 1e-4)
        .collect()
}

/// Render one full-resolution PNG frame at timeline time `t`.
pub fn capture_frame(project: &Project, t: f64, out: &Path) -> Result<()> {
    let plan = build_preview_plan(project, t, project.width, project.height)
        .context("Nothing to capture at this position.")?;
    let mut args = plan.ffmpeg_args();
    args.extend([
        "-frames:v".into(),
        "1".into(),
        "-y".into(),
        out.to_string_lossy().to_string(),
    ]);
    let status = command("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .status()
        .context("Could not run ffmpeg for the frame capture")?;
    if !status.success() {
        bail!("frame capture failed");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Filter graph builder
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct CroppedClip {
    input: usize,
    duration: f64,
    /// Offset from the start of the rendered window, in seconds.
    offset: f64,
    has_audio: bool,
    is_video: bool,
    volume: f64,
    fade_in: f64,
    fade_out: f64,
    /// Average playback rate over the section (accounts for speed ramps).
    avg_speed: f64,
    reverse: bool,
}

#[derive(Clone, Debug)]
struct CroppedLayer {
    index: usize,
    label: String,
    clips: Vec<CroppedClip>,
    muted: bool,
    solo: bool,
    gain: f64,
}

struct BuiltGraph {
    /// FFmpeg input arguments (before -filter_complex).
    args: Vec<String>,
    /// filter_complex chain fragments joined with ';'.
    filters: Vec<String>,
    final_video: String,
    layers: Vec<CroppedLayer>,
    duration: f64,
    start: f64,
}

impl BuiltGraph {
    fn finalize(mut self) -> Self {
        // Nothing to do; convenient hook if the graph ever needs a final conversion.
        if self.final_video.is_empty() {
            self.final_video = "vout".to_owned();
        }
        self
    }
}

fn fmt(v: f64) -> String {
    format!("{v:.6}")
}

fn even(v: u32) -> u32 {
    (v.max(2) / 2) * 2
}


/// Colour for an FFmpeg filter option (`#rrggbb` → `0xRRGGBB`).
fn ff_color(hex: &str) -> String {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("0x{}", h.to_ascii_uppercase())
    } else {
        "0xFFFFFF".to_owned()
    }
}

/// Escape a string for use inside `drawtext=text='...'`.
fn drawtext_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' => out.push_str("'\\''"),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// `fontfile=...:` fragment for drawtext, or empty to use FFmpeg's default font.
fn font_arg(bold: bool) -> String {
    let candidates: [&str; 2] = if bold {
        [r"C:\Windows\Fonts\arialbd.ttf", r"C:\Windows\Fonts\seguisb.ttf"]
    } else {
        [r"C:\Windows\Fonts\arial.ttf", r"C:\Windows\Fonts\segoeui.ttf"]
    };
    for p in candidates {
        if Path::new(p).exists() {
            let esc = p.replace('\\', "/").replace(':', "\\:");
            return format!("fontfile='{esc}':");
        }
    }
    String::new()
}

/// Piecewise-linear expression over `var` for a keyframed value.
fn piecewise_expr(
    keys: &[Keyframe],
    offset: f64,
    var: &str,
    get: impl Fn(&Transform) -> f64,
) -> String {
    let mut keys: Vec<Keyframe> = keys.to_vec();
    keys.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if keys.is_empty() {
        return "0".to_owned();
    }
    if keys.len() == 1 {
        return format!("{:.6}", get(&keys[0].transform));
    }
    let last = *keys.last().unwrap();
    let mut expr = format!("{:.6}", get(&last.transform));
    for i in (0..keys.len() - 1).rev() {
        let (a, b) = (keys[i], keys[i + 1]);
        let v0 = get(&a.transform);
        let v1 = get(&b.transform);
        let t0 = a.time + offset;
        let t1 = b.time + offset;
        let span = (t1 - t0).max(1e-6);
        let interp = format!("({v0:.6}+({v1:.6}-{v0:.6})*({var}-{t0:.6})/{span:.6})");
        expr = format!("if(lt({var},{t1:.6}),{interp},{expr})");
        if i == 0 {
            expr = format!("if(lt({var},{t0:.6}),{v0:.6},{expr})");
        }
    }
    expr
}

/// Build the shared video graph for both preview and export.
fn build_graph(project: &Project, start: f64, width: u32, height: u32) -> Option<BuiltGraph> {
    let total = project.duration();
    if total <= 0.001 {
        return None;
    }
    let fps = project.fps.clamp(1, 120);
    let width = even(width);
    let height = even(height);
    let start = start.clamp(0.0, (total - 0.001).max(0.0));
    let duration = (total - start).max(0.0);
    if duration <= 0.001 {
        return None;
    }

    let mut args: Vec<String> = Vec::new();
    let mut filters: Vec<String> = Vec::new();
    let mut layers_out: Vec<CroppedLayer> = Vec::new();
    let mut input_count = 0usize;

    for (li, layer) in project.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        let layer_total = layer.duration();
        if layer_total <= 0.001 {
            continue;
        }
        let origin = layer.start.max(0.0);
        let mut cropped: Vec<CroppedClip> = Vec::new();
        let mut labels: Vec<String> = Vec::new();

        for clip in &layer.clips {
            // Sections are placed freely inside their layer; gaps and overlaps
            // are allowed and simply show through to the layers below.
            let clip_start = origin + clip.start.max(0.0);
            let clip_end = clip_start + clip.duration;
            if clip_end <= start + 1e-6 {
                continue;
            }
            let trim_front = (start - clip_start).max(0.0);
            let remain = clip.duration - trim_front;
            if remain <= 0.001 {
                continue;
            }

            let speed = clip.speed.clamp(0.05, 8.0);
            // A linear speed ramp (ignored for frozen frames).
            let ramp = clip.speed_end.filter(|_| !clip.freeze);
            let s_end = ramp.unwrap_or(speed).clamp(0.05, 8.0);
            let ramp_b = ramp
                .map(|_| (s_end - speed) / clip.duration.max(1e-9))
                .unwrap_or(0.0);
            // Source position (seconds into the source) after `t` timeline seconds.
            let src_at = |t: f64| speed * t + 0.5 * ramp_b * t * t;
            let source_off = src_at(trim_front);
            let source_span = (src_at(clip.duration) - source_off).max(0.0);
            let is_video = clip.kind.is_video();
            let is_text = clip.is_text();
            // Only real media files take an ffmpeg input slot. A text section is
            // generated by `color`/`drawtext` inside the filter graph, so it must
            // not consume an input index (otherwise every later `[N:v]` breaks).
            let idx = if is_text {
                input_count
            } else {
                let i = input_count;
                input_count += 1;
                i
            };

            if is_text {
                // Generated title: no media input, just a transparent canvas.
            } else if is_video {
                // `remain` is timeline time; the source consumes speed× as much.
                // A frozen section always holds the same source frame, no matter
                // where the playhead is inside it.
                let seek = if clip.freeze {
                    clip.in_point.max(0.0)
                } else {
                    clip.in_point + source_off
                };
                args.push("-ss".into());
                args.push(fmt(seek));
                args.push("-t".into());
                if clip.freeze {
                    // Only the first frame is needed; tpad holds it.
                    args.push("0.20".into());
                } else {
                    args.push(fmt(source_span + 0.20));
                }
                args.push("-i".into());
                args.push(clip.path.to_string_lossy().to_string());
            } else {
                args.push("-loop".into());
                args.push("1".into());
                args.push("-framerate".into());
                args.push(fps.to_string());
                args.push("-t".into());
                args.push(fmt(remain));
                args.push("-i".into());
                args.push(clip.path.to_string_lossy().to_string());
            }

            // --- Normalize to a full-frame RGBA image -----------------------
            if is_text {
                let spec = clip.text.clone().unwrap_or_else(|| crate::model::TextSpec::new("Text"));
                let content = drawtext_escape(&spec.content);
                let font = font_arg(spec.bold);
                let color = ff_color(&spec.color);
                // Text sizes are authored at 1080p; scale to the render size.
                let k = (height as f64 / 1080.0).max(0.25);
                let font_px = (spec.size * k).max(6.0);
                let (x, y) = match (spec.align, spec.valign) {
                    (TextAlign::Left, VAlign::Top) => ("w*0.05", "h*0.05"),
                    (TextAlign::Left, VAlign::Middle) => ("w*0.05", "(h-text_h)/2"),
                    (TextAlign::Left, VAlign::Bottom) => ("w*0.05", "h-text_h-h*0.05"),
                    (TextAlign::Center, VAlign::Top) => ("(w-text_w)/2", "h*0.05"),
                    (TextAlign::Center, VAlign::Middle) => ("(w-text_w)/2", "(h-text_h)/2"),
                    (TextAlign::Center, VAlign::Bottom) => ("(w-text_w)/2", "h-text_h-h*0.05"),
                    (TextAlign::Right, VAlign::Top) => ("w-text_w-w*0.05", "h*0.05"),
                    (TextAlign::Right, VAlign::Middle) => ("w-text_w-w*0.05", "(h-text_h)/2"),
                    (TextAlign::Right, VAlign::Bottom) => {
                        ("w-text_w-w*0.05", "h-text_h-h*0.05")
                    }
                };
                let mut dt = format!(
                    "drawtext={font}text='{content}':fontsize={font_px:.1}:fontcolor={color}:\
                     x={x}:y={y}"
                );
                if let Some(bg) = &spec.background {
                    dt.push_str(&format!(
                        ":box=1:boxcolor={}@0.55:boxborderw={:.0}",
                        ff_color(bg),
                        (font_px * 0.22).max(6.0)
                    ));
                }
                let outline = (spec.outline * k).max(0.0);
                if outline > 0.01 {
                    dt.push_str(&format!(
                        ":borderw={:.1}:bordercolor={}",
                        outline,
                        ff_color(&spec.outline_color)
                    ));
                }
                let shadow = (spec.shadow * k).max(0.0);
                if shadow > 0.01 {
                    let off = (shadow * 0.35).max(1.0);
                    dt.push_str(&format!(
                        ":shadowx={off:.1}:shadowy={off:.1}:shadowcolor={}@0.75",
                        ff_color(&spec.shadow_color)
                    ));
                }
                filters.push(format!(
                    "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba,{dt}[n{idx}]",
                    fmt(remain)
                ));
            } else {
                // Speed change before fps conversion so the output keeps the
                // project frame rate.
                let speed_filter = if clip.freeze {
                    // Hold the first frame of the window for the whole section.
                    format!(
                        "trim=end_frame=1,setpts=PTS-STARTPTS,\
                         tpad=stop_mode=clone:stop_duration={},",
                        fmt(remain)
                    )
                } else if clip.reverse {
                    if (speed - 1.0).abs() > 1e-6 {
                        format!("reverse,setpts=(PTS-STARTPTS)/{speed:.6},")
                    } else {
                        "reverse,setpts=PTS-STARTPTS,".to_owned()
                    }
                } else if ramp.is_some() {
                    // The input window starts at `source_off`, so T is the source
                    // time elapsed inside the window. Solve s0*t + b*t^2/2 = T
                    // for the output time t.
                    let s_at = speed + ramp_b * trim_front;
                    let out = if ramp_b.abs() < 1e-9 {
                        format!("(T/{s_at:.6})")
                    } else {
                        format!(
                            "((-{s_at:.6}+sqrt({s2:.6}+{b2:.6}*T))/{b:.6})",
                            s2 = s_at * s_at,
                            b2 = 2.0 * ramp_b,
                            b = ramp_b
                        )
                    };
                    format!("setpts=PTS-STARTPTS,setpts=({out})/TB,")
                } else if (speed - 1.0).abs() > 1e-6 {
                    format!("setpts=(PTS-STARTPTS)/{speed:.6},")
                } else {
                    "setpts=PTS-STARTPTS,".to_owned()
                };
                filters.push(format!(
                    "[{idx}:v]{speed_filter}fps={fps},\
                     scale={width}:{height}:force_original_aspect_ratio=decrease,\
                     format=rgba,\
                     pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black@0,\
                     setsar=1[n{idx}]",
                ));
            }

            // --- Colour / blur / flip effects -------------------------------
            // Applied to the normalized frame before fades and the transform, so
            // they act on the section's own pixels only.
            let mut effected = format!("n{idx}");
            {
                let mut chain: Vec<String> = Vec::new();
                // Crop, then stretch the remainder back over the full frame so
                // the rest of the graph keeps the same geometry.
                let (cl, cr, ct, cb) = (
                    clip.crop_l.clamp(0.0, 0.45),
                    clip.crop_r.clamp(0.0, 0.45),
                    clip.crop_t.clamp(0.0, 0.45),
                    clip.crop_b.clamp(0.0, 0.45),
                );
                if cl + cr + ct + cb > 0.001 {
                    let cw = (1.0 - cl - cr).clamp(0.05, 1.0);
                    let ch = (1.0 - ct - cb).clamp(0.05, 1.0);
                    chain.push(format!(
                        // `setsar=1` is required: scaling after a non-uniform crop
                        // otherwise keeps the input display aspect ratio by
                        // rewriting SAR (e.g. 19:20), which makes concat/overlay
                        // reject the stream.
                        "crop=iw*{cw:.6}:ih*{ch:.6}:iw*{cl:.6}:ih*{ct:.6},\
                         scale={width}:{height},setsar=1"
                    ));
                }
                if clip.pixelate > 0.5 {
                    let block = clip.pixelate.clamp(2.0, 64.0);
                    let pw = ((width as f64 / block).round() as i64).max(1);
                    let ph = ((height as f64 / block).round() as i64).max(1);
                    chain.push(format!("pixelize=width={pw}:height={ph}"));
                }
                if clip.sharpen > 0.01 {
                    let amount = clip.sharpen.clamp(0.0, 1.5);
                    chain.push(format!(
                        "unsharp=luma_msize_x=5:luma_msize_y=5:luma_amount={amount:.4}"
                    ));
                }
                if clip.flip_h {
                    chain.push("hflip".to_owned());
                }
                if clip.flip_v {
                    chain.push("vflip".to_owned());
                }
                if clip.blur > 0.05 {
                    chain.push(format!("gblur=sigma={:.3}", clip.blur.min(50.0)));
                }
                let bright = clip.brightness.clamp(-1.0, 1.0);
                let contrast = clip.contrast.clamp(0.0, 3.0);
                let sat = clip.saturation.clamp(0.0, 3.0);
                if bright.abs() > 1e-4 || (contrast - 1.0).abs() > 1e-4 || (sat - 1.0).abs() > 1e-4
                {
                    chain.push(format!(
                        "eq=brightness={bright:.4}:contrast={contrast:.4}:saturation={sat:.4}"
                    ));
                }
                if clip.vignette > 0.01 {
                    // Larger angle darkens the corners more strongly.
                    let angle = std::f64::consts::PI / 5.0 * (0.35 + clip.vignette.clamp(0.0, 1.0));
                    chain.push(format!("vignette=angle={angle:.4}"));
                }
                if !chain.is_empty() {
                    filters.push(format!(
                        "[{effected}]{}[fx{idx}]",
                        chain.join(",")
                    ));
                    effected = format!("fx{idx}");
                }
            }

            // --- Fades (alpha) ----------------------------------------------
            // The filters see a stream that starts `trim_front` into the section.
            // When a fade-in is still in progress at that point we must resume at
            // its real alpha. `fade` only accepts st >= 0, so shift the stream's
            // timestamps forward by `trim_front`, fade from zero, and shift them
            // back. That is far cheaper than a per-pixel `geq` (which used to
            // make 1080p projects take minutes per second of video).
            let fade_out = clip.fade_out.clamp(0.0, remain);
            let fade_in_left = (clip.fade_in - trim_front).clamp(0.0, remain);
            let mut faded = effected.clone();
            if fade_in_left > 0.01 {
                let fi = clip.fade_in.max(1e-6);
                if trim_front > 0.001 {
                    filters.push(format!(
                        "[{faded}]setpts=PTS+{}/TB[fs{idx}]",
                        fmt(trim_front)
                    ));
                    filters.push(format!(
                        "[fs{idx}]fade=t=in:st=0:d={}:alpha=1[ff{idx}]",
                        fmt(fi)
                    ));
                    filters.push(format!(
                        "[ff{idx}]setpts=PTS-{}/TB[f{idx}a]",
                        fmt(trim_front)
                    ));
                } else {
                    filters.push(format!(
                        "[{faded}]fade=t=in:st=0:d={}:alpha=1[f{idx}a]",
                        fmt(fi)
                    ));
                }
                faded = format!("f{idx}a");
            }
            if fade_out > 0.01 {
                filters.push(format!(
                    "[{faded}]fade=t=out:st={}:d={}:alpha=1[f{idx}]",
                    fmt((remain - fade_out).max(0.0)),
                    fmt(fade_out)
                ));
                faded = format!("f{idx}");
            }

            // --- Transform (static or keyframed) ----------------------------
            let positioned = format!("p{idx}");
            let animated = !clip.keyframes.is_empty();
            if animated {
                // Filter time starts at 0 for the trimmed window, so a keyframe
                // at section-local time `k.time` sits at `k.time - trim_front`.
                let off = -trim_front;
                let scale_e = piecewise_expr(&clip.keyframes, off, "t", |t| t.scale.max(0.001));
                let x_e = piecewise_expr(&clip.keyframes, off, "t", |t| t.x);
                let y_e = piecewise_expr(&clip.keyframes, off, "t", |t| t.y);
                filters.push(format!(
                    "[{faded}]scale=w='trunc({width}*({scale_e})/2)*2':\
                     h='trunc({height}*({scale_e})/2)*2':eval=frame[s{idx}]"
                ));
                filters.push(format!(
                    "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba[bg{idx}]",
                    fmt(remain)
                ));
                filters.push(format!(
                    "[bg{idx}][s{idx}]overlay=x='trunc({width}*({x_e}))':\
                     y='trunc({height}*({y_e}))':eval=frame:eof_action=pass:format=auto,\
                     format=rgba[k{idx}]"
                ));
                // Opacity: use the cheap colour-mixer whenever it is constant,
                // and only fall back to the expensive per-pixel `geq` when the
                // keyframes really animate alpha (a 1080p geq costs ~30x more).
                let op_static = {
                    let first = clip
                        .keyframes
                        .first()
                        .map(|k| k.transform.opacity)
                        .unwrap_or(clip.transform.opacity);
                    clip.keyframes
                        .iter()
                        .all(|k| (k.transform.opacity - first).abs() < 1e-6)
                };
                if op_static {
                    let op = clip
                        .keyframes
                        .first()
                        .map(|k| k.transform.opacity)
                        .unwrap_or(clip.transform.opacity)
                        .clamp(0.0, 1.0);
                    if (op - 1.0).abs() < 1e-6 {
                        filters.push(format!("[k{idx}]null[{positioned}]"));
                    } else {
                        filters.push(format!(
                            "[k{idx}]colorchannelmixer=aa={op:.4}[{positioned}]"
                        ));
                    }
                } else {
                    // Animate alpha with geq (drawtext/keyframes all end up in alpha).
                    // geq's time variable is capital `T`, unlike overlay/scale's `t`.
                    let op_e =
                        piecewise_expr(&clip.keyframes, off, "T", |t| t.opacity.clamp(0.0, 1.0));
                    filters.push(format!(
                        "[k{idx}]geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':\
                         a='alpha(X,Y)*({op_e})'[{positioned}]"
                    ));
                }
            } else {
                let t = clip.transform;
                if t.scale <= 0.001 || t.opacity <= 0.001 {
                    filters.push(format!(
                        "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba[{positioned}]",
                        fmt(remain)
                    ));
                } else if (t.x).abs() < 1e-9
                    && (t.y).abs() < 1e-9
                    && (t.scale - 1.0).abs() < 1e-9
                    && (t.opacity - 1.0).abs() < 1e-9
                    && clip.rotation.abs() < 1e-6
                {
                    filters.push(format!("[{faded}]null[{positioned}]"));
                } else {
                    let sw = ((width as f64 * t.scale).round() as i64).max(2);
                    let sh = ((height as f64 * t.scale).round() as i64).max(2);
                    let px = (t.x * width as f64).round() as i64;
                    let py = (t.y * height as f64).round() as i64;
                    let rot = clip.rotation.clamp(-180.0, 180.0);
                    if rot.abs() < 1e-6 {
                        filters.push(format!(
                            "[{faded}]scale={sw}:{sh},colorchannelmixer=aa={:.4}[s{idx}]",
                            t.opacity
                        ));
                        filters.push(format!(
                            "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba[bg{idx}]",
                            fmt(remain)
                        ));
                        filters.push(format!(
                            "[bg{idx}][s{idx}]overlay=x={px}:y={py}:eof_action=pass:format=auto,\
                             format=rgba[{positioned}]"
                        ));
                    } else {
                        // Rotate around the clip centre. Expand the frame to the
                        // rotated bounding box so no corners are cut off, then
                        // overlay it so the centre lands where it would have
                        // without rotation.
                        let a = rot.to_radians();
                        let rw = ((sw as f64 * a.cos().abs() + sh as f64 * a.sin().abs()).ceil()
                            as i64)
                            .max(2);
                        let rh = ((sw as f64 * a.sin().abs() + sh as f64 * a.cos().abs()).ceil()
                            as i64)
                            .max(2);
                        let cx = px + sw / 2;
                        let cy = py + sh / 2;
                        let ox = cx - rw / 2;
                        let oy = cy - rh / 2;
                        filters.push(format!(
                            "[{faded}]scale={sw}:{sh},rotate={a:.6}:ow={rw}:oh={rh}:c=none,\
                             colorchannelmixer=aa={:.4}[s{idx}]",
                            t.opacity
                        ));
                        filters.push(format!(
                            "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba[bg{idx}]",
                            fmt(remain)
                        ));
                        filters.push(format!(
                            "[bg{idx}][s{idx}]overlay=x={ox}:y={oy}:eof_action=pass:format=auto,\
                             format=rgba[{positioned}]"
                        ));
                    }
                }
            }
            labels.push(format!("[{positioned}]"));
            cropped.push(CroppedClip {
                input: idx,
                duration: remain,
                offset: (clip_start - start).max(0.0),
                has_audio: is_video && clip.has_audio && !clip.freeze,
                is_video,
                volume: clip.volume,
                // Audio also begins at `trim_front` into the section (the input
                // is seeked before decoding), so a partially elapsed fade must
                // resume rather than restart.
                fade_in: fade_in_left,
                fade_out,
                avg_speed: clip.avg_speed(),
                reverse: clip.reverse && !clip.freeze,
            });
        }

        if cropped.is_empty() {
            continue;
        }
        // Composite the layer onto a transparent canvas, positioning each
        // section at its own offset. Gaps stay transparent and overlapping
        // sections are drawn in list order (the later one on top).
        let canvas_len = (layer_total - start).clamp(0.002, duration);
        let layer_label = format!("l{li}");
        let canvas = format!("lc{li}");
        filters.push(format!(
            "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba[{canvas}]",
            fmt(canvas_len)
        ));
        let mut current = canvas;
        for (ci, label) in labels.iter().enumerate() {
            let offset = cropped[ci].offset;
            let shifted = format!("p{li}_{ci}");
            filters.push(format!(
                "{label}setpts=PTS-STARTPTS+{}/TB[{shifted}]",
                fmt(offset)
            ));
            let next = format!("m{li}_{ci}");
            filters.push(format!(
                "[{current}][{shifted}]overlay=x=0:y=0:eof_action=pass:format=auto[{next}]"
            ));
            current = next;
        }
        filters.push(format!("[{current}]null[{layer_label}]"));
        layers_out.push(CroppedLayer {
            index: li,
            label: layer_label,
            clips: cropped,
            muted: layer.muted,
            solo: layer.solo,
            gain: layer.gain.clamp(0.0, 4.0),
        });
    }

    // Base layer (index 0), extended to the end of the render window with its last
    // frame. When the base is empty or hidden there is nothing to draw at all and
    // the black canvas below is shown instead; emitting an unused [base] label
    // would make FFmpeg reject the whole graph.
    if let Some(base) = layers_out.iter().find(|l| l.index == 0) {
        let base_len = (project.layers[0].duration() - start).max(0.0);
        let pad = (duration - base_len).max(0.0);
        if pad > 0.002 {
            filters.push(format!(
                "[{}]tpad=stop_mode=clone:stop_duration={},format=rgba[base]",
                base.label,
                fmt(pad)
            ));
        } else {
            filters.push(format!("[{}]format=rgba[base]", base.label));
        }
    }

    // Black canvas; every layer is composited on top of it. Section transforms
    // were already applied per clip, so layers are placed at the origin.
    filters.push(format!(
        "color=c=black:s={width}x{height}:r={fps}:d={},format=rgba[canvas]",
        fmt(duration)
    ));
    let mut current = "canvas".to_owned();

    // Base first (index 0), then overlays in ascending order.
    for layer in &layers_out {
        let source = if layer.index == 0 {
            "base".to_owned()
        } else {
            layer.label.clone()
        };
        let li = layer.index;
        let next = format!("v{li}");
        filters.push(format!(
            "[{current}][{source}]overlay=x=0:y=0:eof_action=pass:format=auto[{next}]"
        ));
        current = next;
    }

    filters.push(format!("[{current}]format=rgba[vout]"));

    Some(
        BuiltGraph {
            args,
            filters,
            final_video: "vout".to_owned(),
            layers: layers_out,
            duration,
            start,
        }
        .finalize(),
    )
}

#[derive(Clone, Debug)]
pub struct RenderPlan {
    /// Full FFmpeg arguments except global flags and the output target.
    pub args: Vec<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[allow(dead_code)]
    pub duration: f64,
    #[allow(dead_code)]
    pub start: f64,
}

impl RenderPlan {
    pub fn ffmpeg_args(&self) -> Vec<String> {
        let mut v = vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-loglevel".into(),
            "error".into(),
        ];
        v.extend(self.args.iter().cloned());
        v
    }
}

/// Build a one-frame/streaming preview plan starting at `start`.
pub fn build_preview_plan(project: &Project, start: f64, width: u32, height: u32) -> Option<RenderPlan> {
    let graph = build_graph(project, start, width, height)?;
    let mut args = graph.args.clone();
    args.push("-filter_complex".into());
    args.push(graph.filters.join(";"));
    args.push("-map".into());
    args.push(format!("[{}]", graph.final_video));
    args.push("-an".into());
    args.push("-sn".into());
    args.push("-dn".into());
    args.push("-t".into());
    args.push(fmt(graph.duration));
    args.push("-r".into());
    args.push(project.fps.to_string());
    Some(RenderPlan {
        args,
        width: even(width),
        height: even(height),
        fps: project.fps,
        duration: graph.duration,
        start: graph.start,
    })
}

/// `atempo` filter chain for any playback rate in the editor's 0.05–8× range.
fn atempo_chain(speed: f64) -> String {
    let mut s = speed.clamp(0.05, 8.0);
    let mut parts: Vec<String> = Vec::new();
    while s > 2.0 + 1e-9 {
        parts.push("atempo=2.000000".to_owned());
        s /= 2.0;
    }
    while s < 0.5 - 1e-9 {
        parts.push("atempo=0.500000".to_owned());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-6 {
        parts.push(format!("atempo={s:.6}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{},", parts.join(","))
    }
}

fn build_audio_filters(
    graph: &BuiltGraph,
    duration: f64,
    normalize: bool,
) -> Option<(Vec<String>, String)> {
    let any_audio = graph
        .layers
        .iter()
        .any(|l| l.clips.iter().any(|c| c.has_audio));
    if !any_audio {
        return None;
    }
    // Solo overrides mute: when any track is soloed, only soloed tracks are heard.
    let any_solo = graph.layers.iter().any(|l| l.solo);
    let mut filters = Vec::new();
    let mut padded: Vec<String> = Vec::new();
    for layer in &graph.layers {
        if layer.clips.is_empty() {
            continue;
        }
        let audible = if any_solo { layer.solo } else { !layer.muted };
        let mut segments = Vec::new();
        for (si, clip) in layer.clips.iter().enumerate() {
            let label = format!("a{}_{}", layer.index, si);
            if clip.is_video && clip.has_audio {
                // Speed changes pitch-shift via atempo; the source is consumed at
                // `avg_speed`× the timeline rate. A single atempo only accepts
                // 0.5–2.0, so larger/smaller factors are chained. Reversed
                // sections are reversed before the speed change.
                let tempo = atempo_chain(clip.avg_speed);
                let reverse = if clip.reverse { "areverse," } else { "" };
                // Trim to the timeline duration after the speed change, so the
                // audio segment matches its video segment exactly.
                filters.push(format!(
                    "[{}:a]atrim=start=0:end={},asetpts=PTS-STARTPTS,{reverse}{tempo}aresample=48000,\
                     aformat=sample_fmts=fltp:channel_layouts=stereo,\
                     atrim=start=0:end={},asetpts=PTS-STARTPTS[{}]",
                    clip.input,
                    fmt(clip.duration * clip.avg_speed + 0.05),
                    fmt(clip.duration),
                    label
                ));
            } else {
                filters.push(format!(
                    "anullsrc=r=48000:cl=stereo,atrim=start=0:end={},asetpts=PTS-STARTPTS,\
                     aformat=sample_fmts=fltp:channel_layouts=stereo[{}]",
                    fmt(clip.duration),
                    label
                ));
            }
            // Per-section gain + fades. The tiny edge fades also remove the click
            // you get when a section starts or ends mid-waveform.
            let gain = if audible {
                clip.volume.clamp(0.0, 4.0) * layer.gain.clamp(0.0, 4.0)
            } else {
                0.0
            };
            let declick = 0.008f64.min(clip.duration / 4.0);
            let fade_in = clip.fade_in.max(declick).min(clip.duration);
            let fade_out = clip.fade_out.max(declick).min(clip.duration);
            let shaped = format!("v{}_{}", layer.index, si);
            let mut chain = format!("volume={gain:.4}");
            chain.push_str(&format!(",afade=t=in:st=0:d={}", fmt(fade_in)));
            chain.push_str(&format!(
                ",afade=t=out:st={}:d={}",
                fmt((clip.duration - fade_out).max(0.0)),
                fmt(fade_out)
            ));
            filters.push(format!("[{label}]{chain}[{shaped}]"));
            // Place the segment at its own timeline offset so gaps line up with
            // the video. Overlapping segments simply mix together.
            let delayed = format!("d{}_{}", layer.index, si);
            let ms = (clip.offset.max(0.0) * 1000.0).round() as i64;
            if ms > 0 {
                filters.push(format!("[{shaped}]adelay={ms}|{ms}[{delayed}]"));
            } else {
                filters.push(format!("[{shaped}]anull[{delayed}]"));
            }
            segments.push(format!("[{delayed}]"));
        }
        let joined_label = format!("al{}", layer.index);
        if segments.len() == 1 {
            filters.push(format!("{}anull[{}]", segments[0], joined_label));
        } else {
            filters.push(format!(
                "{}amix=inputs={}:duration=longest:normalize=0[{}]",
                segments.join(""),
                segments.len(),
                joined_label
            ));
        }
        let pad_label = format!("ap{}", layer.index);
        filters.push(format!(
            "[{}]apad=whole_dur={},atrim=start=0:end={}[{}]",
            joined_label,
            fmt(duration),
            fmt(duration),
            pad_label
        ));
        padded.push(format!("[{pad_label}]"));
    }
    if padded.is_empty() {
        return None;
    }
    let final_label = "aout".to_owned();
    // Loudness normalisation runs before the limiter so boosted peaks still
    // cannot clip. `aresample` undoes loudnorm's internal 192 kHz rate.
    let mastering = if normalize {
        "loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000,"
    } else {
        ""
    };
    if padded.len() == 1 {
        filters.push(format!(
            "{}{mastering}alimiter=limit=0.95[{}]",
            padded[0], final_label
        ));
    } else {
        filters.push(format!(
            "{}amix=inputs={}:duration=longest:normalize=0,{mastering}alimiter=limit=0.95[{}]",
            padded.join(""),
            padded.len(),
            final_label
        ));
    }
    Some((filters, final_label))
}

/// User-facing export profiles: `(label, id)`.
pub const EXPORT_PROFILES: &[(&str, &str)] = &[
    ("MP4 - project size", "mp4"),
    ("MP4 1080p", "mp4_1080"),
    ("MP4 720p (small file)", "mp4_720"),
    ("Vertical 1080x1920", "vertical"),
    ("Square 1080x1080", "square"),
    ("GIF 480 px", "gif"),
    ("Audio only (M4A)", "audio"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProfileKind {
    Mp4 { width: u32, height: u32, crf: u32 },
    Gif { width: u32 },
    Audio,
}

fn profile_kind(project: &Project, profile: &str) -> ProfileKind {
    match profile {
        "mp4_1080" => ProfileKind::Mp4 { width: 1920, height: 1080, crf: 20 },
        "mp4_720" => ProfileKind::Mp4 { width: 1280, height: 720, crf: 22 },
        "vertical" => ProfileKind::Mp4 { width: 1080, height: 1920, crf: 20 },
        "square" => ProfileKind::Mp4 { width: 1080, height: 1080, crf: 20 },
        "gif" => ProfileKind::Gif { width: 480 },
        "audio" => ProfileKind::Audio,
        _ => ProfileKind::Mp4 {
            width: project.width,
            height: project.height,
            crf: 20,
        },
    }
}

/// Full export command arguments (excluding -progress). Returns (args, duration, has_audio).
pub fn build_export_args(
    project: &Project,
    output: &Path,
    preset: &str,
    profile: &str,
) -> Result<(Vec<String>, f64, bool)> {
    const VALID_PRESETS: [&str; 9] = [
        "ultrafast",
        "superfast",
        "veryfast",
        "faster",
        "fast",
        "medium",
        "slow",
        "slower",
        "veryslow",
    ];
    let preset = if VALID_PRESETS.contains(&preset) {
        preset
    } else {
        "medium"
    };
    let kind = profile_kind(project, profile);
    let (out_w, out_h) = match kind {
        ProfileKind::Mp4 { width, height, .. } => (even(width), even(height)),
        ProfileKind::Gif { width } => {
            let w = even(width);
            let h = even((width as f64 * project.height as f64
                / project.width.max(1) as f64)
                .round()
                .max(2.0) as u32);
            (w, h)
        }
        ProfileKind::Audio => (project.width, project.height),
    };
    let mut graph = build_graph(project, 0.0, out_w, out_h)
        .context("The timeline is empty; add media before exporting.")?;
    let duration = graph.duration;
    // Keep a slice of the machine free so the UI stays responsive and the
    // system never runs out of memory. FFmpeg's defaults happily spawn one
    // encoder/filter thread per core, which on a 4-core/6 GB machine makes
    // 1080p multi-layer compositing lock Windows up.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let filter_threads = (cores / 2).clamp(1, 2);
    let encoder_threads = (cores - 1).max(1);
    let mut args = vec![
        "-y".to_owned(),
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-filter_complex_threads".to_owned(),
        filter_threads.to_string(),
    ];
    args.extend(graph.args.clone());

    let audio = build_audio_filters(&graph, duration, project.normalize_audio);
    if let Some((audio_filters, _)) = &audio {
        graph.filters.extend(audio_filters.iter().cloned());
    }

    // GIF needs a palette pass over the final RGBA frame.
    let mut video_label = graph.final_video.clone();
    if let ProfileKind::Gif { .. } = kind {
        graph.filters.push(format!(
            "[{video_label}]format=rgb24,split[gif_a][gif_b]"
        ));
        graph.filters
            .push("[gif_a]palettegen=stats_mode=diff[gif_p]".to_owned());
        graph.filters.push(
            "[gif_b][gif_p]paletteuse=dither=bayer:bayer_scale=3[gif_out]".to_owned(),
        );
        video_label = "gif_out".to_owned();
    }

    // Audio-only exports still build the full video graph. Consume its output
    // with a sink so FFmpeg does not reject the unconnected filter pad.
    if let ProfileKind::Audio = kind {
        graph.filters.push(format!("[{video_label}]nullsink"));
    }

    args.push("-filter_complex".into());
    args.push(graph.filters.join(";"));
    let mut has_audio = false;
    if let ProfileKind::Audio = kind {
        // Ignore the video stream entirely.
        if let Some((_, label)) = &audio {
            args.push("-map".into());
            args.push(format!("[{label}]"));
            has_audio = true;
        }
    } else {
        args.push("-map".into());
        args.push(format!("[{video_label}]"));
        if let ProfileKind::Gif { .. } = kind {
            args.push("-an".into());
        } else if let Some((_, label)) = &audio {
            args.push("-map".into());
            args.push(format!("[{label}]"));
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("192k".into());
            has_audio = true;
        } else {
            args.push("-an".into());
        }
    }

    match kind {
        ProfileKind::Audio => {
            if !has_audio {
                bail!("This timeline has no audio to export.");
            }
            args.extend([
                "-t".to_owned(),
                fmt(duration),
                "-vn".to_owned(),
                "-c:a".to_owned(),
                "aac".to_owned(),
                "-b:a".to_owned(),
                "192k".to_owned(),
                "-progress".to_owned(),
                "pipe:1".to_owned(),
                "-nostats".to_owned(),
            ]);
        }
        ProfileKind::Gif { .. } => {
            args.extend([
                "-t".to_owned(),
                fmt(duration),
                "-r".to_owned(),
                "15".to_owned(),
                "-c:v".to_owned(),
                "gif".to_owned(),
                "-loop".to_owned(),
                "0".to_owned(),
                "-progress".to_owned(),
                "pipe:1".to_owned(),
                "-nostats".to_owned(),
            ]);
        }
        ProfileKind::Mp4 { crf, .. } => {
            args.extend([
                "-t".to_owned(),
                fmt(duration),
                "-r".to_owned(),
                project.fps.to_string(),
                "-c:v".to_owned(),
                "libx264".to_owned(),
                "-threads".to_owned(),
                encoder_threads.to_string(),
                "-preset".to_owned(),
                preset.to_owned(),
                "-crf".to_owned(),
                crf.to_string(),
                "-pix_fmt".to_owned(),
                "yuv420p".to_owned(),
                // No `+faststart`: it rewrites the whole file a second time at the
                // end, which doubles disk churn and makes the export look frozen
                // at ~99%.
                "-progress".to_owned(),
                "pipe:1".to_owned(),
                "-nostats".to_owned(),
            ]);
        }
    }
    args.push(output.to_string_lossy().to_string());
    Ok((args, duration, has_audio))
}

#[derive(Debug)]
pub enum ExportMsg {
    Progress(f32),
    Done(Result<PathBuf, String>),
}

/// Runs an export on a background thread. Progress is parsed from FFmpeg's `-progress` stream.
pub fn run_export(
    project: Project,
    output: PathBuf,
    preset: String,
    profile: String,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: std::sync::mpsc::Sender<ExportMsg>,
) {
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::Ordering;

    let (args, duration) = match build_export_args(&project, &output, &preset, &profile) {
        Ok(v) => (v.0, v.1),
        Err(e) => {
            let _ = tx.send(ExportMsg::Done(Err(e.to_string())));
            return;
        }
    };

    let mut cmd = command("ffmpeg");
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match spawn_quiet(&mut cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(ExportMsg::Done(Err(format!("Could not start FFmpeg: {e}"))));
            return;
        }
    };

    let stderr = child.stderr.take();
    let err_handle = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                lines.push(line);
                if lines.len() > 40 {
                    lines.remove(0);
                }
            }
        }
        lines.join("\n")
    });

    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<f64>() {
                    if duration > 0.0 {
                        let p = (us / 1_000_000.0 / duration).clamp(0.0, 1.0) as f32;
                        let _ = tx.send(ExportMsg::Progress(p));
                    }
                }
            } else if let Some(v) = line.strip_prefix("out_time_ms=") {
                // Older FFmpeg versions report microseconds here as well.
                if let Ok(us) = v.trim().parse::<f64>() {
                    if duration > 0.0 {
                        let p = (us / 1_000_000.0 / duration).clamp(0.0, 1.0) as f32;
                        let _ = tx.send(ExportMsg::Progress(p));
                    }
                }
            }
        }
    }

    let status = child.wait();
    let stderr_text = err_handle.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        let _ = tx.send(ExportMsg::Done(Err("Export cancelled.".to_owned())));
        return;
    }
    match status {
        Ok(s) if s.success() => {
            let _ = tx.send(ExportMsg::Done(Ok(output)));
        }
        Ok(s) => {
            let mut msg = format!("FFmpeg exited with {s}.");
            if !stderr_text.trim().is_empty() {
                msg.push('\n');
                msg.push_str(stderr_text.trim());
            }
            let _ = tx.send(ExportMsg::Done(Err(msg)));
        }
        Err(e) => {
            let _ = tx.send(ExportMsg::Done(Err(format!("Could not wait for FFmpeg: {e}"))));
        }
    }
}

/// Convenience used by tests/tools: not called by the UI.
#[allow(dead_code)]
pub fn clip_label(clip: &Clip) -> String {
    if clip.kind == MediaKind::Video {
        format!("{} [video]", clip.name)
    } else {
        format!("{} [photo]", clip.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Clip;

    fn project_with_crop() -> Project {
        let mut p = Project::new();
        p.width = 1920;
        p.height = 1080;
        p.layers[0].clips.push(Clip {
            id: 1,
            path: PathBuf::from("does-not-exist.mp4"),
            name: "crop".to_owned(),
            kind: MediaKind::Video,
            in_point: 0.0,
            duration: 2.0,
            source_duration: Some(2.0),
            has_audio: true,
            crop_l: 0.1,
            crop_t: 0.1,
            rotation: 5.0,
            ..Clip::default()
        });
        p
    }

    #[test]
    fn crop_chain_resets_sar() {
        // A non-uniform crop followed by `scale` rewrites the sample aspect
        // ratio, which made concat/overlay reject the stream and aborted every
        // export that used crop. `setsar=1` must stay in the chain.
        let p = project_with_crop();
        let graph = build_graph(&p, 0.0, p.width, p.height).unwrap();
        let joined = graph.filters.join(";");
        assert!(
            joined.contains("crop=") && joined.contains("setsar=1"),
            "crop must reset SAR: {joined}"
        );
    }

    #[test]
    fn export_args_limit_resource_use() {
        let p = project_with_crop();
        let (args, duration, has_audio) =
            build_export_args(&p, Path::new("out.mp4"), "ultrafast", "mp4").unwrap();
        assert!((duration - 2.0).abs() < 1e-6);
        assert!(has_audio);
        assert!(args.iter().any(|a| a == "-filter_complex_threads"));
        // Encoder threads must be capped below the core count.
        let i = args.iter().position(|a| a == "-threads").expect("threads cap");
        let n: usize = args[i + 1].parse().unwrap();
        assert!(n >= 1 && n <= std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4));
        // faststart rewriting the file at the end adds significant disk churn.
        assert!(!args.iter().any(|a| a.contains("faststart")));
    }

    #[test]
    fn in_progress_fade_uses_cheap_filters() {
        let mut p = project_with_crop();
        let clip = &mut p.layers[0].clips[0];
        clip.crop_l = 0.0;
        clip.crop_t = 0.0;
        clip.rotation = 0.0;
        clip.fade_in = 2.0;
        // Ask for a frame mid-fade: the graph must not fall back to per-pixel geq.
        let graph = build_graph(&p, 1.0, p.width, p.height).unwrap();
        let joined = graph.filters.join(";");
        assert!(joined.contains("fade=t=in"), "{joined}");
        assert!(
            !joined.contains("geq="),
            "in-progress fades must not use geq: {joined}"
        );
    }

    #[test]
    fn audio_speed_filter_supports_the_whole_range() {
        assert_eq!(atempo_chain(1.0), "");
        assert_eq!(atempo_chain(1.5), "atempo=1.500000,");
        // Single atempo only accepts 0.5-2.0, so bigger factors are chained.
        assert_eq!(
            atempo_chain(4.0),
            "atempo=2.000000,atempo=2.000000,"
        );
        assert_eq!(
            atempo_chain(0.25),
            "atempo=0.500000,atempo=0.500000,"
        );
    }

    #[test]
    fn sections_with_gaps_are_placed_at_their_offsets() {
        let mut p = project_with_crop();
        let mut second = p.layers[0].clips[0].clone();
        second.id = 2;
        second.name = "second".to_owned();
        second.in_point = 0.5;
        second.start = 5.0;
        p.layers[0].clips.push(second);
        assert!((p.duration() - 7.0).abs() < 1e-6);

        let graph = build_graph(&p, 0.0, p.width, p.height).unwrap();
        let offsets: Vec<f64> = graph.layers[0].clips.iter().map(|c| c.offset).collect();
        assert_eq!(offsets, vec![0.0, 5.0]);
        // No concat: sections are overlaid at their own timestamps so the gap
        // stays empty instead of pulling the second section forward.
        let joined = graph.filters.join(";");
        assert!(!joined.contains("concat="), "{joined}");
        assert!(joined.contains("+5.000000/TB"), "{joined}");

        // Audio follows the same placement.
        let (audio, _) = build_audio_filters(&graph, graph.duration, false).expect("audio");
        let ajoined = audio.join(";");
        assert!(ajoined.contains("adelay=5000|5000"), "{ajoined}");
    }

    #[test]
    fn hidden_layer_is_skipped() {
        let mut p = project_with_crop();
        p.layers[0].visible = false;
        // A hidden track still yields a (black) canvas, but nothing is decoded.
        let graph = build_graph(&p, 0.0, p.width, p.height).unwrap();
        assert!(graph.layers.is_empty());
        assert!(!graph.args.iter().any(|a| a == "-i"));

        // Adding an empty overlay layer must not disturb the base layer.
        p.layers[0].visible = true;
        let mut overlay = p.layers[0].clone();
        overlay.name = "Overlay".to_owned();
        overlay.clips.clear();
        p.layers.push(overlay);
        let graph = build_graph(&p, 0.0, p.width, p.height).expect("graph");
        assert_eq!(graph.layers.len(), 1);
        assert!(!graph.final_video.is_empty());
    }

    // ------------------------------------------------------------------
    // Feature coverage: speed ramp / reverse / freeze, text styling,
    // export profiles, loudness, silence parsing.
    // ------------------------------------------------------------------

    fn single_video_project(mutate: impl Fn(&mut Clip)) -> Project {
        let mut p = Project::new();
        p.width = 640;
        p.height = 360;
        p.fps = 30;
        let mut c = Clip {
            id: 1,
            path: PathBuf::from("does-not-exist.mp4"),
            name: "clip".to_owned(),
            kind: MediaKind::Video,
            duration: 4.0,
            source_duration: Some(10.0),
            has_audio: true,
            ..Clip::default()
        };
        mutate(&mut c);
        p.layers[0].clips.push(c);
        p
    }

    #[test]
    fn freeze_holds_one_frame_without_audio() {
        let p = single_video_project(|c| c.freeze = true);
        let graph = build_graph(&p, 1.0, p.width, p.height).unwrap();
        let joined = graph.filters.join(";");
        assert!(joined.contains("tpad=stop_mode=clone"), "{joined}");
        let audio = build_audio_filters(&graph, graph.duration, false);
        assert!(audio.is_none(), "frozen sections must not contribute audio");
    }

    #[test]
    fn reverse_uses_reverse_filters_and_areverse() {
        let p = single_video_project(|c| c.reverse = true);
        let graph = build_graph(&p, 0.0, p.width, p.height).unwrap();
        let joined = graph.filters.join(";");
        assert!(joined.starts_with("[0:v]reverse"), "{joined}");
        let (audio, _) = build_audio_filters(&graph, graph.duration, false).expect("audio");
        assert!(audio.join(";").contains("areverse"), "{audio:?}");
    }

    #[test]
    fn speed_ramp_uses_a_timestamp_expression() {
        let p = single_video_project(|c| {
            c.speed = 0.5;
            c.speed_end = Some(2.0);
        });
        let graph = build_graph(&p, 0.0, p.width, p.height).unwrap();
        let joined = graph.filters.join(";");
        assert!(joined.contains("sqrt("), "{joined}");
        // The expression must not smuggle an option separator into the filter.
        let expr = joined.split("setpts=(").nth(1).unwrap_or("");
        let expr = expr.split(")/TB").next().unwrap_or("");
        assert!(!expr.contains(','), "expression leaked a comma: {expr}");
    }

    #[test]
    fn text_style_emits_outline_shadow_and_alignment() {
        let mut p = Project::new();
        p.width = 640;
        p.height = 360;
        let mut spec = crate::model::TextSpec::new("Hello");
        spec.outline = 4.0;
        spec.shadow = 6.0;
        spec.align = TextAlign::Right;
        spec.valign = VAlign::Bottom;
        p.layers[0].clips.push(Clip {
            id: 1,
            name: "title".to_owned(),
            kind: MediaKind::Text,
            duration: 2.0,
            text: Some(spec),
            ..Clip::default()
        });
        let graph = build_graph(&p, 0.0, p.width, p.height).unwrap();
        let joined = graph.filters.join(";");
        assert!(joined.contains("borderw="), "{joined}");
        assert!(joined.contains("shadowx="), "{joined}");
        assert!(joined.contains("w-text_w-w*0.05"), "{joined}");
        assert!(joined.contains("h-text_h-h*0.05"), "{joined}");
    }

    #[test]
    fn audio_profile_maps_only_audio_and_sinks_video() {
        let p = single_video_project(|_| {});
        let (args, _, has_audio) =
            build_export_args(&p, Path::new("out.m4a"), "ultrafast", "audio").unwrap();
        assert!(has_audio);
        assert!(args.iter().any(|a| a == "-vn"));
        let graph = args.iter().position(|a| a == "-filter_complex").unwrap() + 1;
        assert!(args[graph].contains("nullsink"), "{}", args[graph]);
        assert!(!args.iter().any(|a| a == "[vout]"));
    }

    #[test]
    fn gif_and_social_profiles_change_the_output_geometry() {
        let p = single_video_project(|_| {});
        let (args, _, _) = build_export_args(&p, Path::new("out.mp4"), "ultrafast", "vertical").unwrap();
        let graph = args.iter().position(|a| a == "-filter_complex").unwrap() + 1;
        assert!(args[graph].contains("s=1080x1920"), "{}", args[graph]);

        let (args, _, _) = build_export_args(&p, Path::new("out.mp4"), "ultrafast", "square").unwrap();
        let graph = args.iter().position(|a| a == "-filter_complex").unwrap() + 1;
        assert!(args[graph].contains("s=1080x1080"), "{}", args[graph]);

        let (args, _, _) =
            build_export_args(&p, Path::new("out.gif"), "ultrafast", "gif").unwrap();
        let graph = args.iter().position(|a| a == "-filter_complex").unwrap() + 1;
        assert!(args[graph].contains("palettegen"), "{}", args[graph]);
        assert!(args.iter().any(|a| a == "gif"));
        assert!(args.iter().any(|a| a == "-an"));
    }

    #[test]
    fn loudness_normalisation_is_optional() {
        let mut p = single_video_project(|_| {});
        p.normalize_audio = true;
        let (args, _, _) =
            build_export_args(&p, Path::new("out.mp4"), "ultrafast", "mp4").unwrap();
        let graph = args.iter().position(|a| a == "-filter_complex").unwrap() + 1;
        assert!(args[graph].contains("loudnorm=I=-16"), "{}", args[graph]);

        p.normalize_audio = false;
        let (args, _, _) =
            build_export_args(&p, Path::new("out.mp4"), "ultrafast", "mp4").unwrap();
        let graph = args.iter().position(|a| a == "-filter_complex").unwrap() + 1;
        assert!(!args[graph].contains("loudnorm"), "{}", args[graph]);
    }

    #[test]
    fn silence_ranges_are_parsed_from_ffmpeg_logs() {
        let log = "[silencedetect @ 0000] silence_start: 1.25\n\
                   [silencedetect @ 0000] silence_end: 3.5 | silence_duration: 2.25\n\
                   [silencedetect @ 0000] silence_start: 7\n";
        let ranges = parse_silence(log, 10.0);
        assert_eq!(ranges.len(), 2);
        assert!((ranges[0].0 - 1.25).abs() < 1e-6);
        assert!((ranges[0].1 - 3.5).abs() < 1e-6);
        // An unterminated final range extends to the end of the section.
        assert!((ranges[1].0 - 7.0).abs() < 1e-6);
        assert!((ranges[1].1 - 10.0).abs() < 1e-6);
    }

    #[test]
    fn waveform_and_silence_work_on_the_sample_media() {
        let base = Path::new("sample/base.mp4");
        if !base.exists() {
            return;
        }
        let peaks = waveform(base, Some(6.0), 200).expect("waveform");
        assert_eq!(peaks.len(), 200);
        assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));
        assert!(peaks.iter().any(|p| *p > 0.05), "waveform is flat: {peaks:?}");
        // The detector must run end-to-end even when it finds nothing.
        let ranges = detect_silence(base, 0.0, 4.0, -35.0, 0.3).expect("silence detection");
        for (a, b) in ranges {
            assert!(b > a);
            assert!(b <= 4.001);
        }
    }
}
