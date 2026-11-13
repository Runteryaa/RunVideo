# RunVideo

A small, fast, layer-based video editor written in Rust.

**Download:** grab the latest Windows build from
[Releases](https://github.com/Runteryaa/RunVideo/releases).

### Installing

| Download | What you get |
| --- | --- |
| `RunVideo-<version>-setup.exe` **(recommended)** | A normal Windows installer. It bundles FFmpeg and ffprobe, so there is nothing else to install. Adds Start Menu (and optional desktop) shortcuts plus an uninstaller. |
| `RunVideo-<version>-portable.zip` | The same app with FFmpeg included, unpacked into a folder. Extract it anywhere and run `runvideo.exe`; delete the folder to remove it. |
| `runvideo.exe` | Only the editor binary. This one **does need FFmpeg**: put `ffmpeg.exe` and `ffprobe.exe` on your `PATH`, or create a `ffmpeg\bin` folder next to the executable with both files in it. |

RunVideo looks for FFmpeg in `ffmpeg\bin` beside its own executable first, then
falls back to `PATH`. Building from source also works with any FFmpeg on `PATH`.

If Windows shows a SmartScreen warning for the installer, it is unsigned:
choose **More info → Run anyway**.

RunVideo edits video and photos on a multi-layer timeline and renders the final
result with FFmpeg. The UI runs on [egui](https://github.com/emilk/egui); media
decoding/encoding is delegated to FFmpeg so the editor stays small and a broken
codec can never take the application down with it.

## Features

| Requirement | Where it lives |
| --- | --- |
| Up to **100 sections per project** | Hard limit in `src/model.rs` (`MAX_CLIPS`), enforced by import and split |
| **Split** a section with `S` | Splits the selected/current layer at the playhead, adjusting source in-points |
| **Delete** the selected section with `X` | `delete_selected` |
| **Photos** on the timeline | Any PNG/JPEG/GIF/WebP/BMP/TIFF becomes a still section with an editable duration |
| **Free placement** of videos and photos | Drag a section anywhere on its track (gaps and overlaps allowed) or type its **Start** in the inspector |
| **Layers** for compositing (webcam over gameplay) | Base layer + up to 7 overlay layers, each with a start offset |
| **Per-section transform** | Every section stores its own X/Y/scale/opacity, so clips on one track can sit in different corners |
| **Keyframed transforms** | Animate X/Y/scale/opacity inside one section; keys are interpolated linearly |
| **Text sections** | Generated titles/captions with content, size, bold, colour and an optional box |
| **Speed control** | Per-section 0.05x–8x and a whole-project playback speed; video and audio stay in sync |
| **Audio controls** | Per-section volume, fade in/out, track mute/solo, and a limiter that stops clipping/pops |
| **Live preview audio** | The editor plays the mixed soundtrack while previewing, exactly as the export will sound |
| **Edge trimming** | Drag a section's left or right edge in the timeline to trim it |
| **Colour & effects** | Per-section brightness, contrast, saturation, blur, flip, rotation, crop, pixelate, sharpen and vignette |
| **Transitions** | One-click fade between adjacent sections, plus manual per-section fades |
| **Track gain** | A per-track volume control applied on top of every section's own volume |
| **Timeline markers** | Drop reference markers (`M`) and jump between them |
| **Loop playback** | Replay the whole timeline when playback ends |
| **Vertical / square presets** | `vertical` (1080×1920) and `square` (1080×1080) export profiles |
| **Export presets** | MP4 (project size / 1080p / 720p), vertical, square, GIF and audio-only M4A |
| **Fullscreen preview** | `F11` (or the toolbar button) hides the panels and fills the window; `Esc` exits |
| **Frame capture** | **Capture** saves the exact frame at the playhead as a PNG |
| **Copy / paste section settings** | Copy style, speed, transform and effects from one section and apply them to another |
| **Close gaps** | Pack a track's sections back-to-back in one click |
| **Text outline, shadow and alignment** | Per-section outline/shadow with 9-way alignment plus 5 title templates |
| **Audio waveforms** | Waveforms drawn inside video sections on the timeline |
| **SRT import / export** | SRT cues become text sections (burned in on export); text sections export back to SRT |
| **Speed ramp / reverse / freeze** | Per-section linear speed ramp, reverse playback and freeze-frame |
| **Silence auto cut** | Detect quiet passages in a video section and keep only the audible parts |
| **Multi-select, ripple and insert editing** | `Ctrl+click` multiple sections, ripple-delete them, or insert-edit the clipboard at the playhead |
| **Loudness normalisation** | Optional EBU R128 loudness normalisation (−16 LUFS) plus peak limiting on export |

Also included: snapping, undo/redo, project save/load (`.rvproj`), thumbnails,
drag-and-drop import, frame-accurate preview, and MP4 export with mixed audio.

### Start marker

Press `F` to drop a green **START** flag on the timeline. Once set, pressing
`Space` (or the **Play** button) always begins playback at the marker, even
when the video was paused somewhere else. To continue from where playback was
paused instead, press `Enter` (or the **Resume** button). `T` removes the
marker. There is at most one start marker per project; drag the flag to move
it. Click or drag anywhere on the timeline to jump the playhead there.

### Layers and dragging

Press `L` or `Y` (or **+ Track**) to add a layer. Every layer is its own track row
in the timeline. The track header on the left shows only the layer name and an
eye toggle: click the eye to hide that track from the preview and the export, and
double-click the name to rename it. Drag files directly onto a track row to add
them to that layer; empty rows show a drop hint. Drag any section left or right
on its row to place it anywhere: sections do **not** have to sit back-to-back, so
you can leave gaps or overlap two sections (the later one draws on top). Hold
`Alt` while dragging to ignore snapping. Dragging onto another track moves the
section there and keeps the grabbed point under the cursor. A translucent
preview and a floating label show where it will land. The **Start** field in the
Section tab sets the same position numerically.

New media is added **at the playhead**, not at the end of the track. It simply
starts there — other sections are never shifted or split — so adding a clip at
`00:36.79` puts it exactly at that frame. Dropping several files at once inserts
them one after another starting at the playhead.

The right panel has three tabs: **Section** (trim/split/delete the selected
clip, set its position/scale/opacity, keyframes, text, speed, and audio),
**Layer** (add media/text to the track, rename it, toggle visible/mute/solo/lock,
and set its start offset), and **Project** (name, resolution, frame rate, export
speed).

### Track controls (M / S / L)

Every track header has three small buttons on the right:

- **M** — mute the track's audio (video still shows).
- **S** — solo; while any track is soloed only soloed tracks are heard.
- **L** — lock; locked tracks ignore clicks, drags, deletes and trims, and are
  shaded in the timeline until unlocked.

The same toggles are available in the **Layer** tab and in the track right-click
menu.

### Trimming on the timeline

Hover near a section's left or right edge and the cursor becomes a resize arrow.
Drag to trim. Video sections are clamped to the available source material and
never shorter than the minimum section length; photos and text can extend freely.

### Keyframes

In the **Section** tab press **Animate transform** to start an animation. Move the
playhead, change X/Y/scale/opacity (or drag the box in the preview), and a key is
recorded at that time. **+ Key** adds a key at the playhead, **Del key** removes
the nearest one, and **Clear** returns the section to a static transform. Keys
are interpolated linearly and preview immediately.

### Text sections

**+ Text** in the timeline toolbar inserts a generated title at the playhead
(select the track first, or hover the target row). Text sections are real
timeline sections: split, trim, move and animate them like media. They need no FFmpeg
input, so they never disturb the rest of the graph.

### Transitions

Select a section and click **Trans** in the Section tab to fade it out while the
next section on the same track (the first one that starts after it ends) fades in
over 0.5 s. This is a fade-through (out to black/transparent, then in); for a true
crossfade, overlap the two sections. Adjust the exact length afterwards with the
section's own **Fade out** / **Fade in** controls, which can differ per section.

### Colour, blur, flip and rotation

Each media section has its own **Adjust** block in the Section tab: brightness
(−1 … 1), contrast and saturation (0 … 3), Gaussian blur (0 … 50 px), rotation
(−180 … 180°), horizontal/vertical flip, plus a **Reset** button. Effects are
applied to that section's pixels only, before its fades and transform, so two
sections on the same track can look completely different. Rotation happens
around the section's centre and expands the frame to the rotated bounding box,
so corners are never clipped.

### Crop and stylise

Below **Adjust** the same tab has:

- **Crop** — left / right / top / bottom sliders that cut the source and stretch
  the remainder back over the frame (each up to 45 %, and the two sides can never
  remove the whole picture). Use it to remove letterboxing or reframe a shot.
- **Pixelate** — block size in pixels (0 = off). Hides faces or details; block
  sizes of 8–24 read well at 720p.
- **Sharpen** — an unsharp-mask amount (0 … 1.5) for soft footage.
- **Vignette** — darkens the corners (0 … 1) for a filmic look.

### Timeline markers

Press `M` to drop a gold timeline marker (a diamond on the ruler), `Alt+M` to
remove the nearest one. Markers are listed in the Project tab with jump and
delete buttons. They are saved with the project.

**Loop** in the transport bar replays the whole timeline each time playback
reaches the end.

### Track gain

The Layer tab has a **Track gain** slider for each track (0 … 4×), multiplied
into every section's own volume, so you can ride a whole music or commentary
track without touching each clip. It still passes through the limiter, so
boosting never clips.

### Autosave and crash recovery

While a project has unsaved edits, RunVideo writes a recovery file every 15
seconds to `%LOCALAPPDATA%\RunVideo\autosave.rvproj`. If the app is closed
without saving, the next launch shows a **Recover / Discard** banner. Saving the
project deletes the recovery file. Opening a project directly (`runvideo.exe
my.rvproj`) skips the prompt.

### Help

Press **F1** or click **Help** in the toolbar for an in-app shortcut sheet.

### Speed, audio and pops

Each section has its own playback speed (0.05x–8x, with 0.25/0.5/0.75/1/1.15/1.25/1.5/2/4
presets) in the **Section** tab. Changing a section's speed keeps its slot on the
timeline: at 2x it consumes twice as much source inside the same length (limited by
available source), and at 0.5x it stretches the source. The **Project** tab also has
a **Playback speed** control (0.1x–4x, same presets) that retimes the *whole*
timeline — sections, starts, gaps, keyframes, markers and fades all scale, while each
section's speed is multiplied so no source frames are lost. Set it back to 1x (or
undo) to restore the original arrangement.

Speed changes also affect audio: RunVideo chains `atempo` stages so any rate in the
editor's range keeps sync and pitch-corrected tempo. Every section also has a volume
slider plus fade-in/fade-out. Fades already in progress at the seek point resume at
the right alpha rather than restarting, and every audio mix passes through
`alimiter=limit=0.95`, so sections cut mid-waveform don't click or clip.


### Editing directly in the preview

Select a track and the preview shows an orange box around the section under the
playhead. Drag the box to move it and drag any corner to resize it — the
opposite corner stays put. This works for the base layer too, so any section can
be positioned or shrunk without touching the numeric sliders.

**The transform belongs to the section, not the track.** Two clips on the same
track keep independent positions, scales and opacities; moving one never affects
the other. To edit a different section, move the playhead onto it (or select it
on the timeline) and drag again. If the selected track is hidden or has no
content at the playhead, the preview explains why nothing can be dragged.

To change compositing order, drag a track header up or down, or right-click it
and pick **Move up/down**. The base track always stays at the bottom.

Right-click a track header for **Rename**, **Add media here**, **Move up/down**
and **Delete track**.

### Export formats and presets

The **Project** tab has two dropdowns:

- **Export speed** — FFmpeg's x264 preset (`ultrafast` … `veryslow`).
- **Export format** — the output size/container:
  - **MP4 – project size** keeps the project's resolution.
  - **MP4 – 1080p** / **MP4 – 720p** force a 16:9 size with letterboxing.
  - **Vertical 9:16** (1080×1920) for Shorts/Reels/TikTok.
  - **Square 1:1** (1080×1080) for feed posts.
  - **GIF** (480 px wide, with a palette pass) — no audio.
  - **Audio only (M4A/AAC)** exports the mixed soundtrack.

The save dialog picks the matching extension. The headless CLI takes the profile
as an optional fourth argument:

```bat
runvideo --export project.rvproj out.mp4 ultrafast vertical
runvideo --export project.rvproj out.gif ultrafast gif
runvideo --export project.rvproj out.m4a ultrafast audio
```

**Normalize loudness (−16 LUFS)** in the Project tab runs the mix through
`loudnorm` and a peak limiter on export, so dialogue and music land at a
consistent, streaming-friendly level.

When the export finishes, the progress window offers **Open video** (plays the
file with your default player) and **Open video location** (reveals it in File
Explorer with the file selected), so you don't have to hunt for the output in
the save folder.

### Fullscreen preview and frame capture

Press `F11` (or click **Fullscreen** in the toolbar) to hide the top bar, timeline
and side panels and fill the window with the preview; `Esc` or `F11` returns.
**Capture** in the toolbar writes the frame at the playhead to a PNG using the
same render graph as the preview, so what you save is exactly what you see.

### Copying section settings

Every section can carry its own look. **Copy settings** / **Paste settings** in the
Section tab copy the transform, keyframes, speed (including ramp/reverse/freeze),
volume, fades, colour adjustments, crops and text style — but keep the target's
text content, media and timing. Use it to apply one grade or title style to many
sections quickly.

### Multi-select, ripple delete and insert editing

`Ctrl+click` sections on the timeline to build a multi-selection (the count is
shown in the Section tab). With several selected:

- `X` / **Ripple delete** removes them and slides later sections on each track
  left to close the holes.
- **Close gaps** packs the selected track back-to-back without deleting anything.
- **Insert edit** pastes the copied section at the playhead and pushes later
  sections on the target track to the right, so nothing is overwritten.

### Speed ramps, reverse and freeze

In the Section tab, a video section can set:

- **Speed ramp to** — a second speed; playback rate changes linearly from the
  section's start rate to the end rate. Audio follows the average rate.
- **Reverse** — plays the section (and its audio) backwards.
- **Freeze frame** — holds the first frame of the section for its whole length
  (silent). Useful for a punch-in pause.

These are combined with the per-section speed and the project-wide playback speed.

### Audio waveforms and silence cutting

Video sections with audio draw their waveform directly on the timeline, so cuts
can be lined up with speech and beats. Click **Cut silence** on a selected video
section and RunVideo runs FFmpeg's silence detector over the section, keeps the
audible ranges and drops the rest as separate sections.

### Live preview audio

Playback in the editor is not silent: RunVideo renders the same mix the exporter
uses (section volume and fades, track gain/mute/solo, speed changes and the
limiter) and streams it to the default Windows output device. The mix starts at
the same timeline position as the picture, so seeking and restarting stay in
sync. Preview mastering skips only the loudness-normalisation pass, whose
lookahead would delay the first fraction of a second; the exported file still
gets that pass. The **Live preview mix** label under the preview shows the
soundtrack is active.

Live preview audio uses the system output through `winmm`, so no extra
dependency or setup is needed. On non-Windows builds the preview is silent while
export still contains the mixed audio. If no output device is available, the
editor falls back silently and playback continues.

### Subtitles (SRT)

**Import SRT** in the Project tab turns each cue into a text section at the cue's
time on the selected track, styled as a bottom-centre caption with outline and
shadow; they are burned into the picture on export and stay editable like any
other section. **Export SRT** writes every text section back out as cues on the
project timeline. The CLI mirrors both:

```bat
runvideo --srt-import project.rvproj subs.srt project_with_subs.rvproj
runvideo --srt-export project.rvproj out.srt
```

### Text styling and title templates

Text sections have an **Outline** (width and colour), a **Shadow** (offset and
colour) and 9-way **Align** (left/centre/right × top/middle/bottom). Five
one-click **title templates** (Title, Lower third, Caption, Big centre, Top
banner) set a good starting style that can still be tweaked per section.

### Export speed

The project panel has an **Export speed** dropdown mapping to FFmpeg's x264
preset (`ultrafast` … `veryslow`). `ultrafast` renders quickly but makes larger
files; `veryslow` makes the smallest files but is much slower. The headless CLI
accepts the same value as an optional third argument:

```bat
runvideo --export project.rvproj out.mp4 ultrafast
```

### Keeping the PC responsive while exporting

An export is a full FFmpeg render, and on a small machine (4 cores / 6 GB) a
1080p multi-layer timeline can saturate both CPU and RAM. RunVideo therefore:

- runs FFmpeg at **below-normal priority**, so the editor and the rest of
  Windows always get CPU time first;
- caps the filter-graph to **2 threads** and the x264 encoder to
  **one thread fewer than the core count**;
- **stops preview rendering** for the duration of the export, so two video
  pipelines never run at once (that was the cause of hard freezes on low-memory
  PCs);
- writes the MP4 directly without `+faststart`, avoiding a second full-file
  rewrite at the very end.

Exports are slower as a result (a heavy 1080p 3-layer timeline renders at about
a quarter of real time on 4 cores), but the progress dialog, Cancel button and
the rest of the desktop stay usable.

### Troubleshooting the preview

If the preview ever reports **Preview render failed**, render the same frame
headlessly to see FFmpeg's full error:

```bat
runvideo --frame project.rvproj frame.png 12.5
```

The third argument is the time in seconds (default 0). This uses the exact
filter graph the editor builds, so a failure here has the same cause. It is
also the quickest way to check a project whose layers were moved, scaled or
hidden.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `S` | Split the section under the playhead |
| `X` / `Delete` | Delete the selected section |
| `Ctrl+D` | Duplicate the selected section |
| `Ctrl+C` / `Ctrl+V` | Copy / paste a section at the playhead |
| `Ctrl+click` | Add/remove a section from the multi-selection |
| `Space` | Play — always from the start marker when one is set |
| `Enter` | Resume playback from the current playhead (ignores the start marker) |
| `Right Shift` | Stop playback and return to the start marker (or beginning) |
| `L` / `Y` | Add an overlay layer |
| `F` | Set the playback start marker at the playhead (one per project) |
| `T` | Clear the playback start marker |
| `M` / `Alt+M` | Add / remove a timeline marker |
| `,` / `.` | Step one frame back / forward (hold `Shift` for 10 frames) |
| `←` / `→` | Step 1 s (hold `Shift` for 5 s) |
| `Home` / `End` | Jump to start / end |
| `Ctrl+Plus` | Zoom the timeline to fit the project |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+S` / `Ctrl+O` | Save / open project |
| `Ctrl+E` | Export MP4 |
| `Ctrl+N` | New project |
| `F11` / `Esc` | Enter / leave fullscreen preview |
| `Ctrl` + mouse wheel | Zoom the timeline around the pointer |

## Requirements

- Rust (stable)
- **FFmpeg** (`ffmpeg.exe` and `ffprobe.exe` on `PATH`, or in `ffmpeg\bin` next
  to `runvideo.exe`)
- Windows GNU toolchain needs MinGW (see below). MSVC toolchains work directly.

## Run

```bat
run.cmd
```

`run.cmd` launches the newest build and builds one first if none exists yet.
`run.cmd --release` builds the optimized version, `run.cmd --rebuild` forces a
fresh debug build, and CLI modes work through it too:

```bat
run.cmd --export project.rvproj out.mp4 ultrafast
run.cmd --version
```

## Build

```bat
build.cmd
```

`build.cmd` finds a MinGW installation (or a WinLibs install from winget) and
builds the `x86_64-pc-windows-gnu` target. Release build:

```bat
build.cmd release
```

Manual equivalent:

```powershell
winget install BrechtSanders.WinLibs.POSIX.UCRT
$mingw = "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin"
$env:PATH = "$mingw;$env:PATH"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "$mingw\x86_64-w64-mingw32-gcc.exe"
cargo build --release --target x86_64-pc-windows-gnu
```

### Installer

`installer\build.ps1` produces both release packages in `dist\`: the Inno Setup
installer and the portable zip, each with FFmpeg bundled.

```powershell
winget install JRSoftware.InnoSetup
.\installer\build.ps1              # version comes from Cargo.toml
.\installer\build.ps1 -Version 0.2.0
```

It copies `ffmpeg.exe`/`ffprobe.exe` from `-FfmpegDir` (or from the FFmpeg on
`PATH`) into `vendor\ffmpeg`, builds the release binary, compiles
`installer\runvideo.iss` with `ISCC.exe`, and zips the portable layout. Pushing a
`v*` tag runs the same script on GitHub Actions and attaches the results to the
release.

## How layering works

The render graph is built in `src/ffmpeg.rs`:

1. Each layer's clips are normalized (scale to fit, letterboxed to a transparent
   full-frame canvas).
2. Each clip then gets its **own** transform: scaled, alpha-blended and placed on
   a transparent full-frame background, so neighbors on a track can differ.
3. Those positioned segments are stamped with their own timeline offset and
   overlaid onto a transparent layer canvas, so gaps stay empty and overlaps draw
   in order.
4. Layers are composited over a black canvas in layer order.
5. Audio from each section is delayed to its own offset and mixed.

The exact same graph feeds the preview and the exporter, so what you see is what
you get. Live preview audio reuses the same audio branching (and therefore the
same volume, fades, speed and limiter) but stops after the audio stage, so the
picture never has to be decoded twice while you listen.

## Project files

`.rvproj` is JSON: resolution, fps, layers, clips, source in-points, a per-section
start position and a per-section transform. Version 1 stored one transform per
layer; those are copied onto each section when the project is opened. Version 2
and older stored clips back-to-back with no explicit start; they are converted to
sequential starts, so old files look the same. Media files are referenced by
absolute path (nothing is copied or re-encoded until you export).
