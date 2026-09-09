use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};

use ffmpeg_next as ffmpeg;
use ffmpeg::format::{input, Pixel};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags};
use ffmpeg::util::frame::video::Video as VideoFrame;

pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct VideoStreamer {
    rx: Receiver<DecodedFrame>,
    stop: Arc<AtomicBool>,
    _thread: JoinHandle<()>,
}

impl VideoStreamer {
    /// `target` is the display's physical size. When present, the decoder
    /// scales frames to cover that size (capped at 3x upscale) using
    /// Lanczos for enlargement and AREA for reduction, so the GPU displays
    /// pixels 1:1 instead of stretching a low-res frame.
    pub fn open(path: PathBuf, target: Option<(u32, u32)>) -> Result<Self, String> {
        // Bounded channel: naturally back-pressures the decoder so we don't
        // buffer more than a couple frames ahead of the render loop.
        let (tx, rx) = bounded::<DecodedFrame>(2);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        let thread = thread::Builder::new()
            .name("video-decoder".into())
            .spawn(move || {
                if let Err(e) = decode_loop(&path, tx, stop_thread, target) {
                    eprintln!("[video] decoder terminated: {e}");
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(VideoStreamer {
            rx,
            stop,
            _thread: thread,
        })
    }

    pub fn try_next_frame(&self) -> Option<DecodedFrame> {
        self.rx.try_recv().ok()
    }
}

impl Drop for VideoStreamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Thread will exit when the channel is dropped or stop is checked.
    }
}

pub fn init() -> Result<(), String> {
    ffmpeg::init().map_err(|e| e.to_string())
}

fn compute_output_size(src_w: u32, src_h: u32, target: Option<(u32, u32)>) -> (u32, u32) {
    let Some((tw, th)) = target else {
        return (src_w, src_h);
    };
    if src_w == 0 || src_h == 0 {
        return (src_w, src_h);
    }
    // Cover-scale factor: enough pixels to fill the display in the largest dim.
    let scale = (tw as f32 / src_w as f32).max(th as f32 / src_h as f32);
    // Cap upscale so a tiny source doesn't produce a giant frame buffer.
    let effective = scale.min(3.0);
    let out_w = ((src_w as f32 * effective).round().max(2.0) as u32) & !1; // even
    let out_h = ((src_h as f32 * effective).round().max(2.0) as u32) & !1;
    (out_w, out_h)
}

fn decode_loop(
    path: &Path,
    tx: Sender<DecodedFrame>,
    stop: Arc<AtomicBool>,
    target: Option<(u32, u32)>,
) -> Result<(), String> {
    // Loop the video by reopening the input when we hit EOF.
    'restart: loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut ictx = input(&path).map_err(|e| format!("open input: {e}"))?;
        let video_stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or("no video stream found")?;
        let video_index = video_stream.index();

        // Frame pacing based on average frame rate. If unknown, default to 30fps.
        let avg = video_stream.avg_frame_rate();
        let frame_period = if avg.numerator() > 0 && avg.denominator() > 0 {
            Duration::from_secs_f64(avg.denominator() as f64 / avg.numerator() as f64)
        } else {
            Duration::from_secs_f64(1.0 / 30.0)
        };

        let codec_ctx = ffmpeg::codec::context::Context::from_parameters(video_stream.parameters())
            .map_err(|e| format!("codec ctx: {e}"))?;
        let mut decoder = codec_ctx
            .decoder()
            .video()
            .map_err(|e| format!("video decoder: {e}"))?;

        let (out_w, out_h) = compute_output_size(decoder.width(), decoder.height(), target);
        let flags = if out_w > decoder.width() || out_h > decoder.height() {
            Flags::LANCZOS
        } else if out_w < decoder.width() || out_h < decoder.height() {
            Flags::AREA
        } else {
            Flags::BILINEAR
        };
        let mut scaler = Scaler::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGBA,
            out_w,
            out_h,
            flags,
        )
        .map_err(|e| format!("scaler init: {e}"))?;

        let mut next_deadline = Instant::now();

        let send_frame = |scaler: &mut Scaler,
                          decoder: &mut ffmpeg::decoder::Video,
                          tx: &Sender<DecodedFrame>,
                          next_deadline: &mut Instant,
                          stop: &Arc<AtomicBool>|
         -> Result<bool, String> {
            let mut decoded = VideoFrame::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                if stop.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                let mut rgba = VideoFrame::empty();
                scaler
                    .run(&decoded, &mut rgba)
                    .map_err(|e| format!("scale: {e}"))?;

                let width = rgba.width();
                let height = rgba.height();
                let stride = rgba.stride(0);
                let row_bytes = (width as usize) * 4;
                let src = rgba.data(0);
                let mut packed = Vec::with_capacity((height as usize) * row_bytes);
                for row in 0..height as usize {
                    let start = row * stride;
                    packed.extend_from_slice(&src[start..start + row_bytes]);
                }

                let now = Instant::now();
                if *next_deadline > now {
                    thread::sleep(*next_deadline - now);
                }
                *next_deadline += frame_period;
                // Prevent drift catastrophe if we're way behind.
                if *next_deadline < Instant::now() {
                    *next_deadline = Instant::now();
                }

                if tx
                    .send(DecodedFrame {
                        width,
                        height,
                        rgba: packed,
                    })
                    .is_err()
                {
                    return Ok(false);
                }
            }
            Ok(true)
        };

        for (stream, packet) in ictx.packets() {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if stream.index() != video_index {
                continue;
            }
            decoder
                .send_packet(&packet)
                .map_err(|e| format!("send packet: {e}"))?;
            if !send_frame(&mut scaler, &mut decoder, &tx, &mut next_deadline, &stop)? {
                return Ok(());
            }
        }
        decoder
            .send_eof()
            .map_err(|e| format!("send eof: {e}"))?;
        send_frame(&mut scaler, &mut decoder, &tx, &mut next_deadline, &stop)?;

        // Loop
        continue 'restart;
    }
}
