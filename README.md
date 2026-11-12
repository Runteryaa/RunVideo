# RunVideo

A small, fast, layer-based video editor written in Rust.

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
| **Edge trimming** | Drag a section's left or right edge in the timeline to trim it |
| **Colour & effects** | Per-section brightness, contrast, saturation, blur, flip, rotation, crop, pixelate, sharpen and vignette |
| **Transitions** | One-click fade between adjacent sections, plus manual per-section fades |
| **Track gain** | A per-track volume control applied on top of every section's own volume |
| **Timeline markers** | Drop reference markers (`M`) and jump between them |
| **Loop playback** | Replay the whole timeline when playback ends |

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
| `Ctrl` + mouse wheel | Zoom the timeline around the pointer |

## Requirements

- Rust (stable)
- **FFmpeg** (`ffmpeg.exe` and `ffprobe.exe` on `PATH`)
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
you get.

## Project files

`.rvproj` is JSON: resolution, fps, layers, clips, source in-points, a per-section
start position and a per-section transform. Version 1 stored one transform per
layer; those are copied onto each section when the project is opened. Version 2
and older stored clips back-to-back with no explicit start; they are converted to
sequential starts, so old files look the same. Media files are referenced by
absolute path (nothing is copied or re-encoded until you export).
