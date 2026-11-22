use ffmpeg_next as ff;
use std::{path::Path, sync::Mutex};
use tauri::ipc::Response;

#[derive(Default)]
pub struct NativeVideo(Mutex<Option<(String, Decoder)>>);

struct Decoder {
    input: ff::format::context::Input,
    codec: ff::decoder::Video,
    stream: usize,
    tick: f64,
    origin: f64,
    pending: Option<(f64, ff::frame::Video)>,
    last_target: Option<f64>,
    eof: bool,
}

fn open_local_input(path: &Path) -> Result<ff::format::context::Input, ff::Error> {
    ff::init()?;
    let mut options = ff::Dictionary::new();
    options.set("protocol_whitelist", "file");
    ff::format::input_with_dictionary(path, options)
}

pub(crate) fn container_metadata(path: &Path) -> Option<(Option<String>, Option<String>, Option<String>, Option<f64>)> {
    let input = open_local_input(path).ok()?;
    let tags = input.metadata();
    let tag = |key| tags.get(key).filter(|value| !value.trim().is_empty()).map(str::to_owned);
    let duration = input.duration();
    Some((tag("title"), tag("ARTIST"), tag("ALBUM"), (duration > 0).then_some(duration as f64 / 1_000_000.0)))
}

pub(super) fn has_video_stream(path: &Path) -> bool {
    open_local_input(path).is_ok_and(|input| input.streams().any(|stream| {
        stream.parameters().medium() == ff::media::Type::Video
            && !stream.disposition().contains(ff::format::stream::Disposition::ATTACHED_PIC)
    }))
}

impl Decoder {
    fn open(path: &Path) -> Result<Self, ff::Error> {
        let input = open_local_input(path)?;
        let stream = input
            .streams()
            .best(ff::media::Type::Video)
            .ok_or(ff::Error::StreamNotFound)?;
        let tick = f64::from(stream.time_base());
        let origin = if stream.start_time() == ff::ffi::AV_NOPTS_VALUE {
            0.0
        } else {
            stream.start_time() as f64 * tick
        };
        let mut context = ff::codec::context::Context::from_parameters(stream.parameters())?;
        let mut threading = ff::codec::threading::Config::default();
        threading.kind = ff::codec::threading::Type::Frame;
        threading.count = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
        context.set_threading(threading);
        let codec = context.decoder().video()?;
        Ok(Self {
            stream: stream.index(),
            input,
            codec,
            tick,
            origin,
            pending: None,
            last_target: None,
            eof: false,
        })
    }

    fn next(&mut self) -> Result<Option<(f64, ff::frame::Video)>, ff::Error> {
        loop {
            let mut frame = ff::frame::Video::empty();
            match self.codec.receive_frame(&mut frame) {
                Ok(()) => {
                    if let Some(ts) = frame.timestamp() {
                        return Ok(Some((ts as f64 * self.tick - self.origin, frame)));
                    }
                    continue;
                }
                Err(ff::Error::Eof) => return Ok(None),
                Err(ff::Error::Other { errno }) if errno == ff::error::EAGAIN => (),
                Err(error) => return Err(error),
            }
            if self.eof {
                return Ok(None);
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => self.codec.send_packet(&packet)?,
                Ok(()) => (),
                Err(ff::Error::Eof) => {
                    self.codec.send_eof()?;
                    self.eof = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn frame(&mut self, target: f64) -> Result<Vec<u8>, ff::Error> {
        if self
            .last_target
            .map_or(target > 1.0, |last| target < last || target - last > 1.0)
        {
            let ts = ((target + self.origin) * 1_000_000.0) as i64;
            self.input.seek(ts, ..ts)?;
            self.codec.flush();
            self.pending = None;
            self.eof = false;
        }
        self.last_target = Some(target);
        let mut latest = None;
        loop {
            let next = match self.pending.take() {
                Some(frame) => Some(frame),
                None => self.next()?,
            };
            let Some((pts, frame)) = next else { break };
            if pts > target {
                self.pending = Some((pts, frame));
                break;
            }
            latest = Some(frame);
        }
        let Some(frame) = latest else {
            return Ok(Vec::new());
        };
        rgba_bytes(&frame, 1280, 720)
    }
}

pub(crate) fn cover(path: &Path) -> Option<Vec<u8>> {
    let mut decoder = Decoder::open(path).ok()?;
    let mut selected = None;
    for _ in 0..60 {
        let Some((pts, frame)) = decoder.next().ok()? else { break };
        selected = Some(frame);
        if pts >= 0.5 { break; }
    }
    let bytes = rgba_bytes(&selected?, 640, 640).ok()?;
    let width = u32::from_le_bytes(bytes[..4].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let mut png = Vec::new();
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&bytes[8..], width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(png)
}

fn rgba_bytes(frame: &ff::frame::Video, max_width: u32, max_height: u32) -> Result<Vec<u8>, ff::Error> {
    let (w, h) = (frame.width(), frame.height());
    let scale = (f64::from(max_width) / f64::from(w))
        .min(f64::from(max_height) / f64::from(h)).min(1.0);
    let width = ((f64::from(w) * scale) as u32).max(1);
    let height = ((f64::from(h) * scale) as u32).max(1);
    let mut scaler = ff::software::scaling::Context::get(
        frame.format(), w, h, ff::format::Pixel::RGBA, width, height,
        ff::software::scaling::Flags::BILINEAR,
    )?;
    let mut rgba = ff::frame::Video::empty();
    scaler.run(frame, &mut rgba)?;
    let row = width as usize * 4;
    let mut bytes = Vec::with_capacity(8 + row * height as usize);
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    for line in rgba.data(0).chunks_exact(rgba.stride(0)).take(height as usize) {
        bytes.extend_from_slice(&line[..row]);
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn native_video_frame(
    app: tauri::AppHandle,
    source_id: String,
) -> Result<Response, String> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        let audio = app.state::<crate::playback::PlaybackState>();
        let Some(path) = audio
            .video_path(&source_id)
            .filter(|path| super::supported(path))
        else {
            return Ok(Response::new(Vec::<u8>::new()));
        };
        let video = app.state::<NativeVideo>();
        let mut decoder = video
            .0
            .lock()
            .map_err(|_| "video decoder unavailable".to_string())?;
        if decoder.as_ref().is_none_or(|(id, _)| *id != source_id) {
            *decoder = Some((
                source_id.clone(),
                Decoder::open(&path).map_err(|e| e.to_string())?,
            ));
        }
        let frame = decoder
            .as_mut()
            .unwrap()
            .1
            .frame(audio.position_secs())
            .map_err(|e| e.to_string())?;
        Ok(Response::new(if audio.video_path(&source_id).is_some() {
            frame
        } else {
            Vec::new()
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_only_webm_is_not_video() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        assert!(!crate::video::has_video(&fixtures.join("audio-only.webm")));
        assert!(crate::video::has_video(&fixtures.join("mp4-opus.mp4")));
    }

    #[test]
    fn mp4_video_follows_audio_position_and_backward_seek() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mp4-opus.mp4");
        let mut decoder = Decoder::open(&path).unwrap();
        let first = decoder.frame(0.0).unwrap();
        assert_eq!(&first[..8], &[32, 0, 0, 0, 32, 0, 0, 0]);
        assert!(decoder.frame(0.05).unwrap().is_empty());
        assert!(!decoder.frame(0.21).unwrap().is_empty());
        assert_eq!(decoder.frame(0.0).unwrap(), first);
    }

    #[test]
    #[ignore = "requires a local video path in PLAYER_VIDEO_TEST_FILE"]
    fn local_video_decodes_after_forward_and_backward_seeks() {
        let path = std::env::var_os("PLAYER_VIDEO_TEST_FILE").unwrap();
        let mut decoder = Decoder::open(Path::new(&path)).unwrap();
        for target in [0.2, 20.0, 5.0] {
            let frame = decoder.frame(target).unwrap();
            assert!(frame.len() > 8, "no frame at {target}s");
            let width = u32::from_le_bytes(frame[..4].try_into().unwrap());
            let height = u32::from_le_bytes(frame[4..8].try_into().unwrap());
            assert_eq!(frame.len(), 8 + (width * height * 4) as usize);
        }
    }
}
