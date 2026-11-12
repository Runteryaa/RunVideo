//! Live preview audio.
//!
//! The timeline mix is produced by FFmpeg exactly like the exporter produces it
//! (speed, fades, track gain, limiter), streamed as raw 48 kHz stereo PCM and
//! written to the default Windows output device through `winmm`. No extra crate
//! is needed: the waveOut API is small enough to bind directly.
//!
//! The device and a dedicated thread are opened lazily on the first playback
//! and then kept alive, so pausing and resuming does not re-open the device.

use crate::ffmpeg;
use crate::model::Project;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Generation-tagged playback request. `generation` changes whenever the
/// preview stream restarts (play, seek, nudge, loop), which tells the audio
/// thread to drop whatever it was playing and start at the new position.
#[derive(Debug)]
enum Cmd {
    Play(#[allow(dead_code)] u64, Project, f64),
    Stop,
}

pub struct AudioPlayer {
    tx: Sender<Cmd>,
    /// Generation of the request currently playing, or `u64::MAX` when idle.
    generation: u64,
    playing: bool,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        #[cfg(windows)]
        std::thread::Builder::new()
            .name("runvideo-audio".to_owned())
            .spawn(move || engine_loop(rx))
            .expect("could not start audio thread");
        #[cfg(not(windows))]
        drop(rx);
        Self {
            tx,
            generation: u64::MAX,
            playing: false,
        }
    }

    /// Keep the soundtrack in step with the video preview.
    ///
    /// Called every frame; only a change in `want` or `generation` touches the
    /// audio thread, so this is effectively free while playing.
    pub fn sync(&mut self, want: bool, generation: u64, project: &Project, start: f64) {
        if want {
            if !self.playing || self.generation != generation {
                self.playing = true;
                self.generation = generation;
                let _ = self.tx.send(Cmd::Play(generation, project.clone(), start));
            }
        } else if self.playing {
            self.playing = false;
            self.generation = u64::MAX;
            let _ = self.tx.send(Cmd::Stop);
        }
    }
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
fn engine_loop(rx: Receiver<Cmd>) {
    use win::{Device, JobEnd};

    let mut device: Option<Device> = None;
    let mut pending: Option<Cmd> = None;
    loop {
        let cmd = match pending.take() {
            Some(cmd) => cmd,
            None => match rx.recv() {
                Ok(cmd) => cmd,
                Err(_) => break,
            },
        };
        match cmd {
            Cmd::Stop => {
                if let Some(dev) = &mut device {
                    dev.reset();
                }
            }
            Cmd::Play(_, project, start) => {
                if device.is_none() {
                    device = Device::open();
                }
                let Some(dev) = &mut device else {
                    // No output device (or it disappeared). Stay silent and
                    // wait for the next request; playback itself keeps working.
                    continue;
                };
                if let JobEnd::Command(next) = play_job(dev, &rx, &project, start).0 {
                    pending = Some(next);
                }
            }
        }
    }
}

/// Diagnostic helper: play the mix for `seconds` and report how much audio
/// actually reached the device. Used by `--audio-selftest`.
#[cfg(windows)]
pub fn selftest(project: &Project, start: f64, seconds: f64) -> Result<(u64, u64), String> {
    use std::time::Duration;
    use win::Device;

    let mut device = Device::open().ok_or("no audio output device is available")?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(seconds.max(0.1)));
        let _ = tx.send(Cmd::Stop);
    });
    let (_, bytes, samples) = play_job(&mut device, &rx, project, start);
    Ok((bytes, samples))
}

#[cfg(not(windows))]
pub fn selftest(_project: &Project, _start: f64, _seconds: f64) -> Result<(u64, u64), String> {
    Err("live audio preview is only implemented on Windows".to_owned())
}

#[cfg(windows)]
fn play_job(
    dev: &mut win::Device,
    rx: &Receiver<Cmd>,
    project: &Project,
    start: f64,
) -> (win::JobEnd, u64, u64) {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc::{sync_channel, RecvTimeoutError};
    use std::time::Duration;
    use win::JobEnd;

    const CHUNK: usize = 4096; // ~21 ms at 48 kHz stereo s16

    let Some((args, _)) = ffmpeg::build_audio_preview_args(project, start) else {
        return (JobEnd::Finished, 0, 0);
    };
    // The picture is already around the first frame when this job starts (the UI
    // only starts audio once the stream is up). FFmpeg still needs a moment to
    // open inputs and fill the first chunk, so that delay is dropped from the
    // stream afterwards to stay locked to the video.
    let launched = std::time::Instant::now();
    let mut child = match ffmpeg::command("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return (JobEnd::Failed, 0, 0),
    };
    let stdout = child.stdout.take();
    // A small queue keeps the driver fed while still bounding how far ahead
    // FFmpeg (which decodes much faster than real time) can run: ~130 ms.
    let (chunk_tx, chunk_rx) = sync_channel::<Vec<u8>>(6);
    let reader = stdout.map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = vec![0u8; CHUNK];
            loop {
                match out.read_exact(&mut buf) {
                    Ok(()) => {
                        if chunk_tx.send(buf.clone()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
    });

    dev.reset();
    let mut end = JobEnd::Finished;
    let mut written: u64 = 0;
    let mut skip: Option<u64> = None;
    'outer: loop {
        match chunk_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(mut chunk) => {
                // On the very first chunk, drop what the video advanced while
                // FFmpeg was starting up (capped so a slow start cannot eat
                // noticeably into the soundtrack).
                let to_skip = *skip.get_or_insert_with(|| {
                    let lead = launched.elapsed().as_secs_f64();
                    ((lead * 192_000.0) as u64).min(96_000)
                });
                if to_skip >= chunk.len() as u64 {
                    skip = Some(to_skip - chunk.len() as u64);
                    continue;
                }
                let chunk = chunk.split_off(to_skip as usize);
                skip = Some(0);
                if !dev.push(&chunk) {
                    // Every buffer is still playing: wait for one to finish,
                    // but stay responsive to Stop/Play from the UI.
                    loop {
                        if let Ok(cmd) = rx.try_recv() {
                            end = JobEnd::Command(cmd);
                            break 'outer;
                        }
                        std::thread::sleep(Duration::from_millis(4));
                        if dev.push(&chunk) {
                            written += chunk.len() as u64;
                            break;
                        }
                    }
                } else {
                    written += chunk.len() as u64;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(cmd) = rx.try_recv() {
                    end = JobEnd::Command(cmd);
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if let Ok(cmd) = rx.try_recv() {
            end = JobEnd::Command(cmd);
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    // Dropping the receiver unblocks the reader if it is waiting to send, so
    // the join below cannot hang.
    drop(chunk_rx);
    if let Some(reader) = reader {
        let _ = reader.join();
    }
    // waveOutReset resets the device position, so read it first.
    let samples = dev.samples_played();
    dev.reset();
    (end, written, samples)
}

#[cfg(not(windows))]
fn engine_loop(rx: Receiver<Cmd>) {
    // Silence on other platforms; the video preview is unaffected.
    while rx.recv().is_ok() {}
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::ptr;

    type HWaveOut = *mut c_void;

    #[repr(C)]
    struct WaveFormatEx {
        format_tag: u16,
        channels: u16,
        samples_per_sec: u32,
        avg_bytes_per_sec: u32,
        block_align: u16,
        bits_per_sample: u16,
        cb_size: u16,
    }

    #[repr(C)]
    struct WaveHdr {
        data: *mut u8,
        buffer_length: u32,
        bytes_recorded: u32,
        user: usize,
        flags: u32,
        loops: u32,
        next: *mut WaveHdr,
        reserved: usize,
    }

    /// `MMTIME` with `wType = TIME_SAMPLES`; the union is two DWORDs wide.
    #[repr(C)]
    struct MmTime {
        time_type: u32,
        value: [u32; 2],
    }

    const TIME_SAMPLES: u32 = 0x0002;

    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
    const CALLBACK_NULL: u32 = 0;
    const WHDR_DONE: u32 = 0x0001;
    const WHDR_PREPARED: u32 = 0x0002;
    const MMSYSERR_NOERROR: u32 = 0;

    // The audio format is fixed: 48 kHz stereo 16-bit, which every Windows
    // audio device supports (the mixer resamples if the endpoint differs).
    const RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;
    const BITS: u16 = 16;
    const BUFFERS: usize = 4;

    #[link(name = "winmm")]
    extern "system" {
        fn waveOutOpen(
            handle: *mut HWaveOut,
            device_id: u32,
            format: *const WaveFormatEx,
            callback: usize,
            instance: usize,
            open_flags: u32,
        ) -> u32;
        fn waveOutClose(handle: HWaveOut) -> u32;
        fn waveOutPrepareHeader(handle: HWaveOut, header: *mut WaveHdr, size: u32) -> u32;
        fn waveOutUnprepareHeader(handle: HWaveOut, header: *mut WaveHdr, size: u32) -> u32;
        fn waveOutWrite(handle: HWaveOut, header: *mut WaveHdr, size: u32) -> u32;
        fn waveOutReset(handle: HWaveOut) -> u32;
        fn waveOutGetPosition(handle: HWaveOut, time: *mut MmTime, size: u32) -> u32;
    }

    struct Buffer {
        data: Vec<u8>,
        hdr: Box<WaveHdr>,
    }

    pub enum JobEnd {
        Command(super::Cmd),
        Finished,
        Failed,
    }

    pub struct Device {
        handle: HWaveOut,
        buffers: Vec<Buffer>,
    }

    fn header_size() -> u32 {
        std::mem::size_of::<WaveHdr>() as u32
    }

    impl Device {
        pub fn open() -> Option<Self> {
            let block_align = CHANNELS * BITS / 8;
            let format = WaveFormatEx {
                format_tag: WAVE_FORMAT_PCM,
                channels: CHANNELS,
                samples_per_sec: RATE,
                avg_bytes_per_sec: RATE * block_align as u32,
                block_align,
                bits_per_sample: BITS,
                cb_size: 0,
            };
            let mut handle: HWaveOut = ptr::null_mut();
            let rc = unsafe {
                waveOutOpen(&mut handle, WAVE_MAPPER, &format, 0, 0, CALLBACK_NULL)
            };
            if rc != MMSYSERR_NOERROR || handle.is_null() {
                return None;
            }
            let buffers = (0..BUFFERS)
                .map(|_| Buffer {
                    data: Vec::new(),
                    // Zeroed is a valid, unprepared WAVEHDR.
                    hdr: Box::new(unsafe { std::mem::zeroed::<WaveHdr>() }),
                })
                .collect();
            Some(Self { handle, buffers })
        }

        /// Hand one PCM chunk to the driver. Returns false when all buffers are
        /// still playing.
        pub fn push(&mut self, bytes: &[u8]) -> bool {
            let handle = self.handle;
            let index = self.buffers.iter().position(|b| {
                let flags = (*b.hdr).flags;
                flags & WHDR_PREPARED == 0 || flags & WHDR_DONE != 0
            });
            let Some(index) = index else {
                return false;
            };
            let buffer = &mut self.buffers[index];
            let flags = (*buffer.hdr).flags;
            if flags & WHDR_PREPARED != 0 {
                // Done buffers must be unprepared before they can be reused.
                unsafe {
                    waveOutUnprepareHeader(handle, &mut *buffer.hdr, header_size());
                }
            }
            buffer.data.clear();
            buffer.data.extend_from_slice(bytes);
            let header = &mut *buffer.hdr;
            header.data = buffer.data.as_mut_ptr();
            header.buffer_length = buffer.data.len() as u32;
            header.bytes_recorded = 0;
            header.flags = 0;
            header.loops = 0;
            if unsafe { waveOutPrepareHeader(handle, header, header_size()) } != MMSYSERR_NOERROR {
                return false;
            }
            if unsafe { waveOutWrite(handle, header, header_size()) } != MMSYSERR_NOERROR {
                return false;
            }
            true
        }

        /// Samples the device has actually rendered since the last reset.
        pub fn samples_played(&self) -> u64 {
            let mut time: MmTime = unsafe { std::mem::zeroed() };
            time.time_type = TIME_SAMPLES;
            let rc = unsafe {
                waveOutGetPosition(self.handle, &mut time, std::mem::size_of::<MmTime>() as u32)
            };
            if rc == MMSYSERR_NOERROR {
                time.value[0] as u64
            } else {
                0
            }
        }

        /// Stop immediately and release every queued buffer.
        pub fn reset(&mut self) {            unsafe {
                waveOutReset(self.handle);
            }
            for buffer in &mut self.buffers {
                if (*buffer.hdr).flags & WHDR_PREPARED != 0 {
                    unsafe {
                        waveOutUnprepareHeader(self.handle, &mut *buffer.hdr, header_size());
                    }
                }
            }
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            unsafe {
                waveOutClose(self.handle);
            }
        }
    }
}
