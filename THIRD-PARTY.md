# Third-party software

RunVideo is MIT licensed (see `LICENSE`). The Windows installer and the
portable build also ship third-party binaries that RunVideo calls at runtime.

## FFmpeg (ffmpeg.exe, ffprobe.exe)

- Project: https://ffmpeg.org/
- Source code: https://github.com/FFmpeg/FFmpeg
- Windows builds: https://www.gyan.dev/ffmpeg/builds/ (essentials build)
- License: **GPL v3** (the build enables `--enable-gpl`, which RunVideo needs
  for the `libx264` encoder)

FFmpeg is a separate program. RunVideo invokes it as a child process and links
against none of its code, but the bundled binary itself is distributed under the
GNU General Public License version 3. A copy of that license is installed next
to the binaries and is available at
https://www.gnu.org/licenses/gpl-3.0.txt.

As required by the GPL, the complete corresponding source for the bundled build
can be obtained from the FFmpeg repository above. If you need a written offer or
a specific source snapshot, open an issue at
https://github.com/Runteryaa/RunVideo/issues.

RunVideo's own source is available at https://github.com/Runteryaa/RunVideo.
