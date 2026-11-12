#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod ffmpeg;
mod model;
mod preview;
mod subs;

use app::RunVideoApp;
use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A bare path argument opens that project in the GUI.
    let open_path = match args.first() {
        Some(a) if !a.starts_with('-') => Some(PathBuf::from(a)),
        _ => None,
    };
    if open_path.is_none() {
        if let Some(code) = cli(&args) {
            std::process::exit(code);
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RunVideo")
            .with_inner_size([1120.0, 620.0])
            .with_min_inner_size([900.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RunVideo",
        options,
        Box::new(move |cc| Ok(Box::new(RunVideoApp::new_with_project(cc, open_path.clone())))),
    )
}

/// Minimal headless interface so timelines can be rendered from scripts/CI:
///   runvideo --export project.rvproj out.mp4
fn cli(args: &[String]) -> Option<i32> {
    if args.is_empty() {
        return None;
    }
    match args[0].as_str() {
        "--version" | "-V" => {
            println!("RunVideo {}", env!("CARGO_PKG_VERSION"));
            if let Some(v) = ffmpeg::ffmpeg_version() {
                println!("{v}");
            }
            Some(0)
        }
        "--help" | "-h" => {
            println!("RunVideo {} — layered video editor\n", env!("CARGO_PKG_VERSION"));
            println!("Usage:");
            println!("  runvideo");
            println!("  runvideo --export <project.rvproj> <output.mp4> [preset] [profile]");
            println!("  runvideo --frame <project.rvproj> <frame.png> [time]");
            println!("  runvideo --srt-import <project.rvproj> <in.srt> <out.rvproj>");
            println!("  runvideo --srt-export <project.rvproj> <out.srt>");
            println!();
            println!(
                "Presets: ultrafast | superfast | veryfast | faster | fast | medium | slow | slower | veryslow"
            );
            println!();
            println!("Requires ffmpeg/ffprobe on PATH.");
            Some(0)
        }
        "--export" => {
            let Some(input) = args.get(1) else {
                eprintln!("error: --export needs a project file");
                return Some(2);
            };
            let Some(output) = args.get(2) else {
                eprintln!("error: --export needs an output file");
                return Some(2);
            };
            let project_path = PathBuf::from(input);
            let text = match std::fs::read_to_string(&project_path) {
                Ok(t) => t.trim_start_matches('\u{feff}').to_owned(),
                Err(e) => {
                    eprintln!("error: could not read {}: {e}", project_path.display());
                    return Some(1);
                }
            };
            let mut project: model::Project = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: invalid project: {e}");
                    return Some(1);
                }
            };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let output_path = PathBuf::from(output);
            let preset = args.get(3).map(String::as_str).unwrap_or("medium");
            let profile = args.get(4).map(String::as_str).unwrap_or("mp4");
            let (ff_args, duration, has_audio) =
                match ffmpeg::build_export_args(&project, &output_path, preset, profile) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: {e}");
                    return Some(1);
                }
            };
            eprintln!(
                "Rendering {:.2}s {}x{} @ {} fps, audio: {} -> {}",
                duration,
                project.width,
                project.height,
                project.fps,
                if has_audio { "yes" } else { "no" },
                output_path.display()
            );
            let mut cmd = ffmpeg::command("ffmpeg");
            cmd.args(&ff_args);
            match ffmpeg::spawn_quiet(&mut cmd).and_then(|mut c| c.wait()) {
                Ok(status) if status.success() => {
                    eprintln!("Done.");
                    Some(0)
                }
                Ok(status) => {
                    eprintln!("error: ffmpeg exited with {status}");
                    Some(1)
                }
                Err(e) => {
                    eprintln!("error: could not run ffmpeg: {e}");
                    Some(1)
                }
            }
        }
        // Print the exact FFmpeg argument vector for a project/time. Useful for
        // diagnosing filter-graph problems without the GUI.
        "--graph" => {
            let Some(input) = args.get(1) else {
                eprintln!("error: --graph needs <project> [time]");
                return Some(2);
            };
            let text = match std::fs::read_to_string(input) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: could not read {input}: {e}");
                    return Some(1);
                }
            };
            let mut project: model::Project = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: invalid project: {e}");
                    return Some(1);
                }
            };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let t: f64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let Some(plan) = ffmpeg::build_preview_plan(&project, t, 1280, 720) else {
                eprintln!("error: no plan");
                return Some(1);
            };
            for (i, a) in plan.ffmpeg_args().iter().enumerate() {
                println!("{i:>3}: {a}");
            }
            Some(0)
        }
        // Subtitles: import an SRT into a project as burn-in text sections, or
        // write every text section back out as an SRT.
        "--srt-import" => {
            let (Some(input), Some(srt), Some(output)) =
                (args.get(1), args.get(2), args.get(3))
            else {
                eprintln!("error: --srt-import needs <project.rvproj> <in.srt> <out.rvproj>");
                return Some(2);
            };
            let text = match std::fs::read_to_string(input) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: could not read {input}: {e}");
                    return Some(1);
                }
            };
            let mut project: model::Project =
                match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: invalid project: {e}");
                        return Some(1);
                    }
                };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let cues = match subs::read_srt(std::path::Path::new(srt)) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: could not read subtitles: {e}");
                    return Some(1);
                }
            };
            let added = subs::import_into(&mut project, 0, &cues);
            let json = match serde_json::to_string_pretty(&project) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: {e}");
                    return Some(1);
                }
            };
            if let Err(e) = std::fs::write(output, json) {
                eprintln!("error: could not write {output}: {e}");
                return Some(1);
            }
            println!("Imported {added} subtitle(s) into {output}");
            Some(0)
        }
        "--srt-export" => {
            let (Some(input), Some(output)) = (args.get(1), args.get(2)) else {
                eprintln!("error: --srt-export needs <project.rvproj> <out.srt>");
                return Some(2);
            };
            let text = match std::fs::read_to_string(input) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: could not read {input}: {e}");
                    return Some(1);
                }
            };
            let mut project: model::Project =
                match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: invalid project: {e}");
                        return Some(1);
                    }
                };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let cues = subs::collect_from(&project);
            if let Err(e) = subs::save_srt(std::path::Path::new(output), &cues) {
                eprintln!("error: {e}");
                return Some(1);
            }
            println!("Wrote {} subtitle(s) to {output}", cues.len());
            Some(0)
        }
        // Print the exact FFmpeg arguments used for live preview audio.
        "--audio-args" => {
            let Some(input) = args.get(1) else {
                eprintln!("error: --audio-args needs <project.rvproj> [time]");
                return Some(2);
            };
            let text = match std::fs::read_to_string(input) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: could not read {input}: {e}");
                    return Some(1);
                }
            };
            let mut project: model::Project =
                match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: invalid project: {e}");
                        return Some(1);
                    }
                };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let t: f64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
            match ffmpeg::build_audio_preview_args(&project, t) {
                Some((list, duration)) => {
                    println!("duration: {duration:.3}s");
                    for (i, a) in list.iter().enumerate() {
                        println!("{i:>3}: {a}");
                    }
                    Some(0)
                }
                None => {
                    eprintln!("error: this project has no audio to preview");
                    Some(1)
                }
            }
        }
        // Diagnostic: prove the live preview audio reaches a real device.
        "--audio-selftest" => {
            let Some(input) = args.get(1) else {
                eprintln!("error: --audio-selftest needs <project.rvproj> [seconds]");
                return Some(2);
            };
            let text = match std::fs::read_to_string(input) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: could not read {input}: {e}");
                    return Some(1);
                }
            };
            let mut project: model::Project =
                match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: invalid project: {e}");
                        return Some(1);
                    }
                };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let seconds: f64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(3.0);
            match audio::selftest(&project, 0.0, seconds) {
                Ok((bytes, samples)) => {
                    println!(
                        "Audio preview: {bytes} PCM bytes written, {samples} samples played ({:.2}s)",
                        samples as f64 / 48_000.0
                    );
                    Some(0)
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    Some(1)
                }
            }
        }
        // Render a single preview frame with the exact graph the editor uses.
        // Handy for diagnosing "Preview render failed" without opening the GUI.
        "--frame" => {
            let (Some(input), Some(output)) = (args.get(1), args.get(2)) else {
                eprintln!("error: --frame needs <project> <out.png> [time]");
                return Some(2);
            };
            let text = match std::fs::read_to_string(input) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: could not read {input}: {e}");
                    return Some(1);
                }
            };
            let mut project: model::Project = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: invalid project: {e}");
                    return Some(1);
                }
            };
            if let Err(e) = project.sanitize() {
                eprintln!("error: {e}");
                return Some(1);
            }
            let t: f64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let plan = match ffmpeg::build_preview_plan(&project, t, 1280, 720) {
                Some(p) => p,
                None => {
                    eprintln!("error: no plan");
                    return Some(1);
                }
            };
            let mut cmd_args = plan.ffmpeg_args();
            cmd_args.extend(["-frames:v".into(), "1".into(), "-y".into(), output.clone()]);
            let status = ffmpeg::command("ffmpeg").args(&cmd_args).status();
            match status {
                Ok(s) if s.success() => {
                    println!("Wrote {output}");
                    Some(0)
                }
                Ok(s) => {
                    eprintln!("error: ffmpeg exited with {s}");
                    Some(1)
                }
                Err(e) => {
                    eprintln!("error: could not run ffmpeg: {e}");
                    Some(1)
                }
            }
        }
        other => {
            eprintln!("error: unknown argument {other:?} (try --help)");
            Some(2)
        }
    }
}
