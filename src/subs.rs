//! Minimal SubRip (`.srt`) subtitle parsing and writing.
//!
//! RunVideo does not need libass: imported subtitles become regular text
//! sections, so they are composited and burned in by the same `drawtext` path
//! as hand-made titles.

use anyhow::{bail, Context, Result};
use crate::model::Project;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Subtitle {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// `HH:MM:SS,mmm` (or `.`) to seconds.
fn parse_time(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    let mut parts = raw.split(':');
    let h: f64 = parts.next()?.trim().parse().ok()?;
    let m: f64 = parts.next()?.trim().parse().ok()?;
    let sec: f64 = parts.next()?.trim().replace(',', ".").parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

pub fn parse_srt(text: &str) -> Result<Vec<Subtitle>> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let lines: Vec<&str> = block.lines().collect();
        let Some(timing_idx) = lines.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        let (a, b) = lines[timing_idx]
            .split_once("-->")
            .context("malformed SRT timing line")?;
        // Some files append positioning after the end timestamp.
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (parse_time(a), parse_time(b)) else {
            continue;
        };
        let content = lines[timing_idx + 1..].join("\n").trim().to_owned();
        if content.is_empty() || end <= start {
            continue;
        }
        out.push(Subtitle {
            start,
            end,
            text: content,
        });
    }
    if out.is_empty() {
        bail!("No subtitles found in that SRT file.");
    }
    out.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    let total_ms = (t * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

pub fn write_srt(subs: &[Subtitle]) -> String {
    let mut out = String::new();
    for (i, s) in subs.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            fmt_time(s.start),
            fmt_time(s.end),
            s.text.replace('\r', "")
        ));
    }
    out
}

pub fn read_srt(path: &Path) -> Result<Vec<Subtitle>> {
    let bytes = std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    parse_srt(&text)
}

pub fn save_srt(path: &Path, subs: &[Subtitle]) -> Result<()> {
    std::fs::write(path, write_srt(subs))
        .with_context(|| format!("could not write {}", path.display()))
}

/// Turn subtitle cues into text sections on `layer`, keeping their timing.
/// Returns how many sections were actually inserted.
pub fn import_into(project: &mut Project, layer: usize, subs: &[Subtitle]) -> usize {
    use crate::model::{Clip, MediaKind, TextAlign, TextSpec, VAlign, MAX_CLIPS, MIN_CLIP};
    let mut added = 0usize;
    for sub in subs {
        if project.total_clips() >= MAX_CLIPS {
            break;
        }
        let mut spec = TextSpec::new(&sub.text);
        spec.size = 54.0;
        spec.align = TextAlign::Center;
        spec.valign = VAlign::Bottom;
        spec.outline = 3.0;
        spec.shadow = 3.0;
        let name: String = sub.text.replace('\n', " ").chars().take(30).collect();
        let mut clip = Clip {
            id: project.alloc_id(),
            name,
            kind: MediaKind::Text,
            start: sub.start.max(0.0),
            duration: (sub.end - sub.start).max(MIN_CLIP),
            text: Some(spec),
            ..Default::default()
        };
        clip.speed = 1.0;
        clip.volume = 1.0;
        if project
            .insert_clip_at(layer, sub.start.max(0.0), clip)
            .is_ok()
        {
            added += 1;
        }
    }
    added
}

/// Collect every text section in the project as subtitle cues, in timeline order.
pub fn collect_from(project: &Project) -> Vec<Subtitle> {
    let mut entries: Vec<Subtitle> = Vec::new();
    for layer in &project.layers {
        let origin = layer.start.max(0.0);
        for c in &layer.clips {
            if !c.kind.is_text() {
                continue;
            }
            let Some(spec) = &c.text else { continue };
            if spec.content.trim().is_empty() {
                continue;
            }
            entries.push(Subtitle {
                start: origin + c.start.max(0.0),
                end: origin + c.start.max(0.0) + c.duration,
                text: spec.content.clone(),
            });
        }
    }
    entries.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::VAlign;

    #[test]
    fn parses_typical_srt() {
        let text = "1\r\n00:00:01,500 --> 00:00:03,000\r\nHello\r\nworld\r\n\r\n\
                    2\r\n00:00:04,000 --> 00:00:05,250\r\nSecond line\r\n";
        let subs = parse_srt(text).unwrap();
        assert_eq!(subs.len(), 2);
        assert!((subs[0].start - 1.5).abs() < 1e-6);
        assert!((subs[0].end - 3.0).abs() < 1e-6);
        assert_eq!(subs[0].text, "Hello\nworld");
        assert!((subs[1].start - 4.0).abs() < 1e-6);
        assert!((subs[1].end - 5.25).abs() < 1e-6);
    }

    #[test]
    fn round_trips_through_write() {
        let subs = vec![
            Subtitle { start: 0.0, end: 1.25, text: "a".into() },
            Subtitle { start: 61.0, end: 62.5, text: "b".into() },
        ];
        let text = write_srt(&subs);
        assert!(text.contains("00:00:00,000 --> 00:00:01,250"));
        assert!(text.contains("00:01:01,000 --> 00:01:02,500"));
        let back = parse_srt(&text).unwrap();
        assert_eq!(back, subs);
    }

    #[test]
    fn rejects_empty_or_invalid() {
        assert!(parse_srt("").is_err());
        assert!(parse_srt("no timing here").is_err());
    }

    #[test]
    fn cues_become_text_sections_and_come_back_out() {
        let mut project = Project::new();
        let subs = vec![
            Subtitle { start: 0.5, end: 2.0, text: "first".into() },
            Subtitle { start: 3.0, end: 4.25, text: "second\nline".into() },
        ];
        let added = import_into(&mut project, 0, &subs);
        assert_eq!(added, 2);
        assert_eq!(project.layers[0].clips.len(), 2);
        // Timing is preserved and the text section is burn-in ready.
        assert!((project.layers[0].clips[0].start - 0.5).abs() < 1e-9);
        assert!((project.layers[0].clips[1].duration - 1.25).abs() < 1e-9);
        assert!(project.layers[0].clips[0].kind.is_text());
        let spec = project.layers[0].clips[1].text.as_ref().unwrap();
        assert_eq!(spec.valign, VAlign::Bottom);
        assert!(spec.outline > 0.0);

        let back = collect_from(&project);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].text, "first");
        assert!((back[1].start - 3.0).abs() < 1e-9);
        assert!((back[1].end - 4.25).abs() < 1e-9);
    }
}
