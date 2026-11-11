//! Background preview rendering. A single worker thread runs FFmpeg and streams raw RGBA
//! frames back to the UI, paced at the project frame rate so playback is smooth.
//!
//! Preview rendering never touches the UI thread, and every new request cancels the old one
//! via a generation counter. A crashing/hanging decoder can therefore never freeze the editor.

use crate::ffmpeg::{self, RenderPlan};
use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub gen: u64,
    pub width: u32,
    pub height: u32,
    /// Stream time in seconds since the first frame (0 for still frames).
    pub time: f64,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum PreviewMsg {
    Frame(PreviewFrame),
    StreamFrame(PreviewFrame),
    StreamEnd(u64),
    Error(u64, String),
}

#[derive(Debug)]
pub enum PreviewCmd {
    Frame { gen: u64, plan: RenderPlan },
    Stream { gen: u64, plan: RenderPlan },
}

pub struct PreviewWorker {
    tx: Sender<PreviewCmd>,
    pub rx: Receiver<PreviewMsg>,
    kill: Arc<AtomicU64>,
}

impl PreviewWorker {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = channel::<PreviewCmd>();
        let (msg_tx, msg_rx) = sync_channel::<PreviewMsg>(4);
        let kill = Arc::new(AtomicU64::new(0));
        let kill_thread = kill.clone();
        std::thread::Builder::new()
            .name("runvideo-preview".to_owned())
            .spawn(move || worker_loop(cmd_rx, msg_tx, kill_thread))
            .expect("could not start preview thread");
        Self {
            tx: cmd_tx,
            rx: msg_rx,
            kill,
        }
    }

    /// Cancel everything older than `gen` and make `gen` the only valid request.
    pub fn cancel_older(&self, gen: u64) {
        self.kill.store(gen, Ordering::SeqCst);
    }

    pub fn send(&self, cmd: PreviewCmd) {
        let _ = self.tx.send(cmd);
    }
}

impl Default for PreviewWorker {
    fn default() -> Self {
        Self::new()
    }
}

fn worker_loop(rx: Receiver<PreviewCmd>, tx: SyncSender<PreviewMsg>, kill: Arc<AtomicU64>) {
    while let Ok(first) = rx.recv() {
        // Collapse queued requests: only the newest one matters.
        let mut cmd = first;
        while let Ok(newer) = rx.try_recv() {
            cmd = newer;
        }
        match cmd {
            PreviewCmd::Frame { gen, plan } => run_frame(gen, plan, &tx, &kill),
            PreviewCmd::Stream { gen, plan } => run_stream(gen, plan, &tx, &kill),
        }
    }
}

fn run_frame(gen: u64, plan: RenderPlan, tx: &SyncSender<PreviewMsg>, kill: &Arc<AtomicU64>) {
    if kill.load(Ordering::SeqCst) != gen {
        return;
    }
    let frame_size = plan.width as usize * plan.height as usize * 4;
    let mut args = plan.ffmpeg_args();
    args.extend([
        "-frames:v".into(),
        "1".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-".into(),
    ]);
    let output = ffmpeg::command("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    if kill.load(Ordering::SeqCst) != gen {
        return;
    }
    match output {
        Ok(out) if out.status.success() && out.stdout.len() >= frame_size => {
            let data = out.stdout[..frame_size].to_vec();
            let _ = tx.send(PreviewMsg::Frame(PreviewFrame {
                gen,
                width: plan.width,
                height: plan.height,
                time: 0.0,
                data,
            }));
        }
        Ok(out) => {
            let mut err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            if err.len() > 1200 {
                err.truncate(1200);
            }
            let msg = if !err.is_empty() {
                format!("Preview render failed:\n{err}")
            } else if out.status.success() {
                // FFmpeg was happy but produced no full frame, e.g. the visible
                // layers have no content at this point in the timeline.
                format!(
                    "Preview render failed: FFmpeg returned {} of {} frame bytes.",
                    out.stdout.len(),
                    frame_size
                )
            } else {
                format!("Preview render failed: FFmpeg exited with {}.", out.status)
            };
            let _ = tx.send(PreviewMsg::Error(gen, msg));
        }
        Err(e) => {
            let _ = tx.send(PreviewMsg::Error(gen, format!("Could not start FFmpeg: {e}")));
        }
    }
}

fn run_stream(gen: u64, plan: RenderPlan, tx: &SyncSender<PreviewMsg>, kill: &Arc<AtomicU64>) {
    if kill.load(Ordering::SeqCst) != gen {
        return;
    }
    let frame_size = plan.width as usize * plan.height as usize * 4;
    let mut args = plan.ffmpeg_args();
    args.extend([
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-".into(),
    ]);
    let mut child = match ffmpeg::command("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(PreviewMsg::Error(gen, format!("Could not start FFmpeg: {e}")));
            return;
        }
    };

    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut lines: Vec<String> = Vec::new();
        if let Some(stderr) = stderr {
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    lines.push(line);
                    if lines.len() > 30 {
                        lines.remove(0);
                    }
                }
            }
        }
        lines.join("\n")
    });

    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            return;
        }
    };

    let frame_dur = 1.0 / plan.fps.max(1) as f64;
    // Start the clock on the first decoded frame, not on process spawn: opening
    // inputs and building the filter graph can take a while, and pacing from
    // spawn would then dump a burst of frames to catch up.
    let mut clock: Option<Instant> = None;
    let mut buf = vec![0u8; frame_size];
    let mut index: u64 = 0;
    let mut sent_any = false;

    loop {
        if kill.load(Ordering::SeqCst) != gen {
            let _ = child.kill();
            break;
        }
        match stdout.read_exact(&mut buf) {
            Ok(()) => {
                let time = index as f64 * frame_dur;
                if let Some(start) = clock {
                    let target = time;
                    let elapsed = start.elapsed().as_secs_f64();
                    if target > elapsed {
                        std::thread::sleep(Duration::from_secs_f64(target - elapsed));
                    }
                } else {
                    clock = Some(Instant::now());
                }
                if kill.load(Ordering::SeqCst) != gen {
                    let _ = child.kill();
                    break;
                }
                if tx
                    .send(PreviewMsg::StreamFrame(PreviewFrame {
                        gen,
                        width: plan.width,
                        height: plan.height,
                        time,
                        data: buf.clone(),
                    }))
                    .is_err()
                {
                    let _ = child.kill();
                    break;
                }
                index += 1;
                sent_any = true;
            }
            Err(_) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let err_text = err_thread.join().unwrap_or_default();

    if kill.load(Ordering::SeqCst) != gen {
        return;
    }
    if sent_any {
        let _ = tx.send(PreviewMsg::StreamEnd(gen));
    } else {
        let mut msg = String::from("Preview playback failed.");
        if !err_text.trim().is_empty() {
            msg.push('\n');
            msg.push_str(err_text.trim());
        }
        let _ = tx.send(PreviewMsg::Error(gen, msg));
    }
}
