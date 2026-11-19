use lofty::config::ParseOptions;
use lofty::file::AudioFile;
use lofty::ogg::OpusFile;
use lofty::prelude::{ItemKey, TaggedFileExt};
use lofty::probe::Probe;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use tauri::Emitter;

type Error = Box<dyn std::error::Error + Send + Sync>;

pub const TARGET_LUFS: f64 = -14.0;
const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;
const R128_REFERENCE_LUFS: f64 = -23.0;
const MAX_GAIN_DB: f64 = 12.0;
const MIN_GAIN_DB: f64 = -24.0;

static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Loudness {
    pub lufs: Option<f64>,
    pub gain_db: f64,
    pub peak: Option<f64>,
}

#[derive(Clone, serde::Serialize)]
struct AnalysisProgress {
    current: usize,
    total: usize,
    status: &'static str,
    added: usize,
    track_name: Option<String>,
}

pub(crate) fn init(conn: &Connection) -> rusqlite::Result<()> {
    let _ = conn.execute_batch("ALTER TABLE tracks ADD COLUMN loudness_lufs REAL");
    let _ = conn.execute_batch("ALTER TABLE tracks ADD COLUMN gain_db REAL");
    let _ = conn.execute_batch("ALTER TABLE tracks ADD COLUMN peak REAL");
    let _ = conn
        .execute_batch("ALTER TABLE device_config ADD COLUMN normalize_volume INTEGER NOT NULL DEFAULT 1");
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS loudness_track_update AFTER UPDATE OF file_hash, modified_secs,
           source_path, segment_start_secs, segment_end_secs ON tracks
         WHEN OLD.file_hash IS NOT NEW.file_hash OR OLD.modified_secs IS NOT NEW.modified_secs
           OR OLD.source_path IS NOT NEW.source_path OR OLD.segment_start_secs IS NOT NEW.segment_start_secs
           OR OLD.segment_end_secs IS NOT NEW.segment_end_secs
         BEGIN
           UPDATE tracks SET gain_db = NULL, loudness_lufs = NULL, peak = NULL WHERE id = NEW.id;
         END;",
    )
}

pub(crate) fn requeue_failed(conn: &Connection) {
    let _ = conn.execute(
        "UPDATE tracks SET gain_db = NULL
          WHERE gain_db = 0 AND loudness_lufs IS NULL AND peak IS NULL",
        [],
    );
}

pub(crate) fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub(crate) fn gain_db_for(lufs: f64) -> f64 {
    (TARGET_LUFS - lufs).clamp(MIN_GAIN_DB, MAX_GAIN_DB)
}

pub(crate) fn gain_factor(gain_db: f64) -> f32 {
    10f64.powf(gain_db / 20.0) as f32
}

pub(crate) fn playback_gain(gain_db: Option<f64>, peak: Option<f64>) -> f32 {
    let Some(gain_db) = gain_db.filter(|value| value.is_finite()) else {
        return 1.0;
    };
    let factor = gain_factor(gain_db.clamp(MIN_GAIN_DB, MAX_GAIN_DB));
    match peak.filter(|value| value.is_finite() && *value > 0.0) {
        Some(peak) => factor.min((1.0 / peak) as f32),
        None => factor.min(1.0),
    }
}

pub(crate) fn track_gain(conn: &Connection, id: i64) -> f32 {
    conn.query_row(
        "SELECT gain_db, peak FROM tracks WHERE id = ?1",
        params![id],
        |row| Ok(playback_gain(row.get(0)?, row.get(1)?)),
    )
    .unwrap_or(1.0)
}

pub(crate) fn parse_replaygain_db(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    let number = trimmed
        .strip_suffix("dB")
        .or_else(|| trimmed.strip_suffix("db"))
        .or_else(|| trimmed.strip_suffix("DB"))
        .unwrap_or(trimmed);
    number.trim().parse::<f64>().ok().filter(|gain| gain.is_finite())
}

pub(crate) fn parse_peak(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|peak| peak.is_finite() && *peak > 0.0)
}

pub(crate) fn parse_r128_gain_db(value: &str) -> Option<f64> {
    value.trim().parse::<i32>().ok().map(|q78| f64::from(q78) / 256.0)
}

fn from_tag_gain(tag_gain_db: f64, reference_lufs: f64, peak: Option<f64>) -> Loudness {
    let lufs = reference_lufs - tag_gain_db;
    Loudness {
        lufs: Some(lufs),
        gain_db: gain_db_for(lufs),
        peak,
    }
}

fn tagged_loudness(path: &Path) -> Option<Loudness> {
    let is_opus = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("opus"));
    if is_opus {
        let mut file = File::open(path).ok()?;
        let opus = OpusFile::read_from(&mut file, ParseOptions::new()).ok()?;
        let gain = opus
            .vorbis_comments()
            .get("R128_TRACK_GAIN")
            .and_then(parse_r128_gain_db);
        if let Some(gain) = gain {
            return Some(from_tag_gain(gain, R128_REFERENCE_LUFS, None));
        }
    }
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let gain = tag
        .get_string(ItemKey::ReplayGainTrackGain)
        .and_then(parse_replaygain_db)?;
    let peak = tag
        .get_string(ItemKey::ReplayGainTrackPeak)
        .and_then(parse_peak);
    Some(from_tag_gain(gain, REPLAYGAIN_REFERENCE_LUFS, peak))
}

pub(crate) fn measure(path: &Path, start_secs: f64, end_secs: Option<f64>) -> Result<Loudness, Error> {
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(File::open(path)?), Default::default()),
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?
        .format;
    let track = format.tracks().iter().find(|track| track.codec_params.sample_rate.is_some())
        .ok_or("no audio track")?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let source_rate = params.sample_rate.ok_or("unknown sample rate")?;
    let mut decoder = crate::audio_codecs::registry().make(&params, &DecoderOptions::default())?;
    if start_secs > 0.0 {
        let _ = format.seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time: Time::new(start_secs as u64, start_secs.fract()),
                track_id: Some(track_id),
            },
        );
        decoder.reset();
    }

    let mut meter: Option<ebur128::EbuR128> = None;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    let mut peak = 0f64;
    loop {
        if CANCEL.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::ResetRequired) => break,
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let packet_start = match params.time_base {
            Some(time_base) => {
                let time = time_base.calc_time(packet.ts());
                time.seconds as f64 + time.frac
            }
            None => packet.ts() as f64 / f64::from(source_rate),
        };
        if end_secs.is_some_and(|end| packet_start >= end) {
            break;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if channels == 0 || spec.rate == 0 {
            return Err("invalid audio format".into());
        }
        if meter.is_none() {
            meter = Some(ebur128::EbuR128::new(
                channels as u32,
                spec.rate,
                ebur128::Mode::I | ebur128::Mode::SAMPLE_PEAK,
            )?);
        }
        let samples = buffer.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if samples.capacity() < decoded.frames() * channels {
            *samples = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        samples.copy_interleaved_ref(decoded);
        let interleaved = samples.samples();
        let frames = interleaved.len() / channels;
        let rate = f64::from(spec.rate);
        let first = if packet_start < start_secs {
            (((start_secs - packet_start) * rate).ceil() as usize).min(frames)
        } else {
            0
        };
        let last = match end_secs {
            Some(end) => (((end - packet_start) * rate).ceil().max(0.0) as usize).min(frames),
            None => frames,
        };
        if first < last {
            let slice = &interleaved[first * channels..last * channels];
            if let Some(meter) = meter.as_mut() {
                meter.add_frames_f32(slice)?;
            }
            for sample in slice {
                peak = peak.max(f64::from(sample.abs()));
            }
        }
    }

    let lufs = meter.ok_or("no audio frames")?.loudness_global()?;
    if lufs.is_finite() {
        Ok(Loudness {
            lufs: Some(lufs),
            gain_db: gain_db_for(lufs),
            peak: Some(peak),
        })
    } else {
        Ok(Loudness {
            lufs: None,
            gain_db: 0.0,
            peak: Some(peak),
        })
    }
}

fn analyze(path: &Path, is_segment: bool, start_secs: f64, end_secs: Option<f64>) -> Result<Loudness, Error> {
    if !is_segment {
        if let Some(tagged) = tagged_loudness(path) {
            if tagged.peak.is_some() || tagged.gain_db <= 0.0 {
                return Ok(tagged);
            }
            return Ok(measure(path, start_secs, end_secs).unwrap_or(tagged));
        }
    }
    measure(path, start_secs, end_secs)
}

struct Pending {
    id: i64,
    source: String,
    is_segment: bool,
    start: f64,
    end: Option<f64>,
    hash: Option<String>,
    modified: i64,
}

fn remaining(conn: &Mutex<Connection>) -> rusqlite::Result<usize> {
    conn.lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM tracks WHERE gain_db IS NULL", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count as usize)
}

fn process_one(conn: &Mutex<Connection>, root: &Path) -> Result<bool, Error> {
    let pending = {
        let db = conn.lock().unwrap();
        db.query_row(
            "SELECT id,
                    COALESCE(NULLIF(source_path, ''), path),
                    COALESCE(source_kind, 'file') = 'cue',
                    COALESCE(segment_start_secs, 0),
                    segment_end_secs,
                    file_hash,
                    COALESCE(modified_secs, 0)
               FROM tracks
              WHERE gain_db IS NULL
              ORDER BY id
              LIMIT 1",
            [],
            |row| {
                Ok(Pending {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    is_segment: row.get(2)?,
                    start: row.get(3)?,
                    end: row.get(4)?,
                    hash: row.get(5)?,
                    modified: row.get(6)?,
                })
            },
        )
        .optional()?
    };
    let Some(pending) = pending else {
        return Ok(false);
    };
    let result = analyze(&root.join(&pending.source), pending.is_segment, pending.start, pending.end);
    if CANCEL.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let loudness = result.unwrap_or_else(|error| {
        eprintln!("[loudness] analysis failed for {}: {error}", pending.source);
        Loudness {
            lufs: None,
            gain_db: 0.0,
            peak: None,
        }
    });
    conn.lock().unwrap().execute(
        "UPDATE tracks
            SET loudness_lufs = ?1, gain_db = ?2, peak = ?3
          WHERE id = ?4 AND file_hash IS ?5 AND COALESCE(modified_secs, 0) = ?6
            AND COALESCE(NULLIF(source_path, ''), path) = ?7
            AND COALESCE(segment_start_secs, 0) = ?8
            AND segment_end_secs IS ?9",
        params![
            loudness.lufs,
            loudness.gain_db,
            loudness.peak,
            pending.id,
            pending.hash,
            pending.modified,
            pending.source,
            pending.start,
            pending.end,
        ],
    )?;
    Ok(true)
}

fn emit_progress(app: &tauri::AppHandle, current: usize, total: usize, status: &'static str) {
    let _ = app.emit(
        "index-progress",
        AnalysisProgress {
            current,
            total,
            status,
            added: 0,
            track_name: None,
        },
    );
}

pub(crate) fn start(conn: Arc<Mutex<Connection>>, root: PathBuf, app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("loudness-analysis".to_owned())
        .spawn(move || {
            let mut done = 0usize;
            while !CANCEL.load(Ordering::SeqCst) {
                match process_one(&conn, &root) {
                    Ok(true) => {
                        done += 1;
                        let left = remaining(&conn).unwrap_or(0);
                        emit_progress(&app, done, done + left, "analyzing");
                    }
                    Ok(false) => {
                        if done > 0 {
                            emit_progress(&app, done, done, "analyzed");
                            done = 0;
                        }
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    Err(error) => {
                        eprintln!("[loudness] {error}");
                        std::thread::sleep(Duration::from_secs(10));
                    }
                }
            }
        });
}

#[tauri::command]
pub fn set_normalize_volume(
    enabled: bool,
    library: tauri::State<'_, crate::library::LibraryState>,
    playback: tauri::State<'_, crate::playback::PlaybackState>,
) -> Result<(), String> {
    library
        .conn()
        .lock()
        .unwrap()
        .execute(
            "UPDATE device_config SET normalize_volume = ?1 WHERE id = 1",
            params![if enabled { 1 } else { 0 }],
        )
        .map_err(|error| error.to_string())?;
    playback.set_normalize(enabled);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn temp_wav(amplitude: f32) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let rate = 48_000u32;
        let frames = rate * 3;
        let mut data = Vec::new();
        data.extend(b"RIFF");
        data.extend((36 + frames * 2).to_le_bytes());
        data.extend(b"WAVEfmt ");
        data.extend(16u32.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(rate.to_le_bytes());
        data.extend((rate * 2).to_le_bytes());
        data.extend(2u16.to_le_bytes());
        data.extend(16u16.to_le_bytes());
        data.extend(b"data");
        data.extend((frames * 2).to_le_bytes());
        for index in 0..frames {
            let phase = 2.0 * std::f32::consts::PI * 1000.0 * index as f32 / rate as f32;
            data.extend(((phase.sin() * amplitude * 32767.0) as i16).to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!(
            "player-loudness-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn gain_db_converts_to_linear_factor() {
        assert!((gain_factor(0.0) - 1.0).abs() < 1e-6);
        assert!((gain_factor(20.0) - 10.0).abs() < 1e-4);
        assert!((gain_factor(-20.0) - 0.1).abs() < 1e-6);
        assert!((gain_factor(6.0206) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn gain_is_derived_from_target_and_clamped() {
        assert!((gain_db_for(-20.0) - 6.0).abs() < 1e-9);
        assert!((gain_db_for(-10.0) + 4.0).abs() < 1e-9);
        assert_eq!(gain_db_for(-60.0), MAX_GAIN_DB);
        assert_eq!(gain_db_for(10.0), MIN_GAIN_DB);
    }

    #[test]
    fn playback_gain_is_limited_by_peak() {
        assert_eq!(playback_gain(None, Some(0.5)), 1.0);
        assert_eq!(playback_gain(Some(f64::NAN), Some(0.5)), 1.0);
        assert!((playback_gain(Some(12.0), Some(0.5)) - 2.0).abs() < 1e-6);
        assert!(playback_gain(Some(12.0), Some(0.9)) * 0.9 <= 1.0 + 1e-6);
        assert!((playback_gain(Some(-6.0), Some(1.0)) - gain_factor(-6.0)).abs() < 1e-6);
        assert!((playback_gain(Some(40.0), Some(0.01)) - gain_factor(MAX_GAIN_DB)).abs() < 1e-5);
    }

    #[test]
    fn playback_gain_without_peak_never_boosts() {
        assert_eq!(playback_gain(Some(6.0), None), 1.0);
        assert!((playback_gain(Some(-6.0), None) - gain_factor(-6.0)).abs() < 1e-6);
    }

    #[test]
    fn parses_replaygain_tags() {
        assert_eq!(parse_replaygain_db("-6.50 dB"), Some(-6.5));
        assert_eq!(parse_replaygain_db("+3.2dB"), Some(3.2));
        assert_eq!(parse_replaygain_db(" 0.00 DB "), Some(0.0));
        assert_eq!(parse_replaygain_db("-4"), Some(-4.0));
        assert_eq!(parse_replaygain_db("loud"), None);
        assert_eq!(parse_peak("0.988553"), Some(0.988553));
        assert_eq!(parse_peak("0"), None);
        assert_eq!(parse_peak("nope"), None);
    }

    #[test]
    fn parses_r128_gain_as_q7_8() {
        assert_eq!(parse_r128_gain_db("-512"), Some(-2.0));
        assert_eq!(parse_r128_gain_db(" 256 "), Some(1.0));
        assert_eq!(parse_r128_gain_db("x"), None);
    }

    #[test]
    fn tag_gain_is_rebased_to_target_level() {
        let replaygain = from_tag_gain(-6.0, REPLAYGAIN_REFERENCE_LUFS, Some(0.9));
        assert_eq!(replaygain.lufs, Some(-12.0));
        assert!((replaygain.gain_db + 2.0).abs() < 1e-9);
        let r128 = from_tag_gain(0.0, R128_REFERENCE_LUFS, None);
        assert_eq!(r128.lufs, Some(-23.0));
        assert!((r128.gain_db - 9.0).abs() < 1e-9);
    }

    #[test]
    fn measures_relative_loudness_and_peak() {
        let quiet = temp_wav(0.1);
        let loud = temp_wav(0.2);
        let quiet_result = measure(&quiet, 0.0, None).unwrap();
        let loud_result = measure(&loud, 0.0, None).unwrap();
        let _ = std::fs::remove_file(&quiet);
        let _ = std::fs::remove_file(&loud);
        let difference = loud_result.lufs.unwrap() - quiet_result.lufs.unwrap();
        assert!((difference - 6.02).abs() < 0.1, "difference was {difference}");
        assert!((quiet_result.peak.unwrap() - 0.1).abs() < 0.01);
        assert!((loud_result.peak.unwrap() - 0.2).abs() < 0.01);
        assert!(quiet_result.gain_db > loud_result.gain_db);
    }

    #[test]
    fn measures_only_the_requested_range() {
        let path = temp_wav(0.1);
        let whole = measure(&path, 0.0, None).unwrap();
        let range = measure(&path, 0.5, Some(2.5)).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!((whole.lufs.unwrap() - range.lufs.unwrap()).abs() < 0.3);
    }

    fn migration_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tracks (id INTEGER PRIMARY KEY, file_hash TEXT, modified_secs INTEGER,
                source_path TEXT, segment_start_secs REAL, segment_end_secs REAL);
             CREATE TABLE device_config (id INTEGER PRIMARY KEY, emoji TEXT);
             INSERT INTO device_config VALUES (1, 'x');
             INSERT INTO tracks (id, file_hash, modified_secs) VALUES (1, 'a', 10);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn migration_adds_columns_and_is_repeatable() {
        let conn = migration_connection();
        init(&conn).unwrap();
        init(&conn).unwrap();
        let (lufs, gain, peak): (Option<f64>, Option<f64>, Option<f64>) = conn
            .query_row(
                "SELECT loudness_lufs, gain_db, peak FROM tracks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((lufs, gain, peak), (None, None, None));
        let normalize: i64 = conn
            .query_row("SELECT normalize_volume FROM device_config WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(normalize, 1);
    }

    #[test]
    fn changed_file_resets_stored_loudness() {
        let conn = migration_connection();
        init(&conn).unwrap();
        conn.execute(
            "UPDATE tracks SET loudness_lufs = -20, gain_db = 6, peak = 0.5 WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute("UPDATE tracks SET source_path = source_path WHERE id = 1", []).unwrap();
        let kept: Option<f64> = conn
            .query_row("SELECT gain_db FROM tracks WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, Some(6.0));
        conn.execute("UPDATE tracks SET modified_secs = 11 WHERE id = 1", []).unwrap();
        let reset: Option<f64> = conn
            .query_row("SELECT gain_db FROM tracks WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(reset, None);
    }

    #[test]
    fn failed_analyses_are_requeued_but_silence_is_not() {
        let conn = migration_connection();
        init(&conn).unwrap();
        conn.execute(
            "INSERT INTO tracks (id, file_hash, modified_secs, gain_db, peak) VALUES (2, 'b', 1, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute("UPDATE tracks SET gain_db = 0 WHERE id = 1", []).unwrap();
        requeue_failed(&conn);
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM tracks WHERE gain_db IS NULL")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(ids, vec![1]);
    }
}
