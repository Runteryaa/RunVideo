//! FFmpeg integration: media probing, filter graph construction, thumbnails and export.
//!
//! RunVideo deliberately shells out to FFmpeg instead of linking libav*: it keeps the
//! application tiny, avoids unsafe decoding code, and makes a crash in a decoder unable
//! to take the editor down with it.

use crate::model::{is_image_path, Clip, Keyframe, MediaKind, Project, Transform};
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
    speed: f64,
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
                let source_off = trim_front * speed;
                args.push("-ss".into());
                args.push(fmt(clip.in_point + source_off));
                args.push("-t".into());
                args.push(fmt(remain * speed + 0.05));
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
                let mut dt = format!(
                    "drawtext={font}text='{content}':fontsize={:.1}:fontcolor={color}:\
                     x=(w-text_w)/2:y=(h-text_h)/2",
                    spec.size * (height as f64 / 1080.0).max(0.25)
                );
                if let Some(bg) = &spec.background {
                    dt.push_str(&format!(
                        ":box=1:boxcolor={}@0.55:boxborderw={:.0}",
                        ff_color(bg),
                        (spec.size * 0.22).max(6.0)
                    ));
                }
                filters.push(format!(
                    "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba,{dt}[n{idx}]",
                    fmt(remain)
                ));
            } else {
                // Speed change before fps conversion so the output keeps the
                // project frame rate.
                let speed_filter = if (speed - 1.0).abs() > 1e-6 {
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
                has_audio: is_video && clip.has_audio,
                is_video,
                volume: clip.volume,
                // Audio also begins at `trim_front` into the section (the input
                // is seeked before decoding), so a partially elapsed fade must
                // resume rather than restart.
                fade_in: fade_in_left,
                fade_out,
                speed,
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

fn build_audio_filters(graph: &BuiltGraph, duration: f64) -> Option<(Vec<String>, String)> {
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
                // `speed`× the timeline rate.
                let tempo = if (clip.speed - 1.0).abs() > 1e-6 {
                    format!("atempo={:.6},", clip.speed.clamp(0.5, 2.0))
                } else {
                    String::new()
                };
                // Trim to the timeline duration after the speed change, so the
                // audio segment matches its video segment exactly.
                filters.push(format!(
                    "[{}:a]atrim=start=0:end={},asetpts=PTS-STARTPTS,{tempo}aresample=48000,\
                     aformat=sample_fmts=fltp:channel_layouts=stereo,\
                     atrim=start=0:end={},asetpts=PTS-STARTPTS[{}]",
                    clip.input,
                    fmt(clip.duration * clip.speed + 0.05),
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
    if padded.len() == 1 {
        filters.push(format!("{}alimiter=limit=0.95[{}]", padded[0], final_label));
    } else {
        filters.push(format!(
            "{}amix=inputs={}:duration=longest:normalize=0,alimiter=limit=0.95[{}]",
            padded.join(""),
            padded.len(),
            final_label
        ));
    }
    Some((filters, final_label))
}

/// Full export command arguments (excluding -progress). Returns (args, duration, has_audio).
pub fn build_export_args(
    project: &Project,
    output: &Path,
    preset: &str,
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
    let mut graph = build_graph(project, 0.0, project.width, project.height)
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

    let audio = build_audio_filters(&graph, duration);
    if let Some((audio_filters, _)) = &audio {
        graph.filters.extend(audio_filters.iter().cloned());
    }
    args.push("-filter_complex".into());
    args.push(graph.filters.join(";"));
    args.push("-map".into());
    args.push(format!("[{}]", graph.final_video));
    let has_audio = if let Some((_, label)) = &audio {
        args.push("-map".into());
        args.push(format!("[{label}]"));
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("192k".into());
        true
    } else {
        args.push("-an".into());
        false
    };
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
        "20".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv420p".to_owned(),
        // No `+faststart`: it rewrites the whole file a second time at the end,
        // which doubles disk churn and makes the export look frozen at ~99%.
        "-progress".to_owned(),
        "pipe:1".to_owned(),
        "-nostats".to_owned(),
    ]);
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
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: std::sync::mpsc::Sender<ExportMsg>,
) {
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::Ordering;

    let (args, duration) = match build_export_args(&project, &output, &preset) {
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
            build_export_args(&p, Path::new("out.mp4"), "ultrafast").unwrap();
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
        let (audio, _) = build_audio_filters(&graph, graph.duration).expect("audio");
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
}
