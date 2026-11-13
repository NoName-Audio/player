//! Bounded-memory waveform extraction shared by all CUE segments of a source.
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

type DecodeError = Box<dyn std::error::Error + Send + Sync>;
const BINS: usize = 256;

fn open(path: &Path) -> Result<Box<dyn FormatReader>, DecodeError> {
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    Ok(symphonia::default::get_probe().format(
        &hint,
        MediaSourceStream::new(Box::new(File::open(path)?), Default::default()),
        &FormatOptions { enable_gapless: true, ..Default::default() },
        &MetadataOptions::default(),
    )?.format)
}

// Each pass walks the source once, never once per segment. Unknown container
// duration needs a counting pass before energy can be assigned to fixed bins.
fn stream(
    format: &mut dyn FormatReader,
    mut frame: impl FnMut(f64, f32),
) -> Result<f64, DecodeError> {
    let track = format.default_track().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "no audio track")
    })?;
    let track_id = track.id;
    let mut decoder = crate::audio_codecs::registry().make(&track.codec_params, &DecoderOptions::default())?;
    let mut samples = None;
    let mut elapsed = 0.0;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != track_id { continue; }
        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if channels == 0 || spec.rate == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid audio format").into());
        }
        let buffer = samples.get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        if buffer.capacity() < decoded.frames() * channels {
            *buffer = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        buffer.copy_interleaved_ref(decoded);
        let packet_start = elapsed;
        let mut count = 0_u64;
        for channels in buffer.samples().chunks_exact(channels) {
            // Average channel energy, not signed samples: stereo cannot cancel.
            let energy = channels.iter().map(|sample| {
                if sample.is_finite() { sample.clamp(-1.0, 1.0).powi(2) } else { 0.0 }
            }).sum::<f32>() / channels.len() as f32;
            frame(packet_start + count as f64 / f64::from(spec.rate), energy);
            count += 1;
        }
        elapsed = packet_start + count as f64 / f64::from(spec.rate);
    }
    Ok(elapsed)
}

pub(crate) fn decode(path: &Path, segments: &[(f64, Option<f64>)]) -> Result<Vec<Vec<u8>>, DecodeError> {
    if segments.is_empty() { return Ok(Vec::new()); }
    for &(start, end) in segments {
        if !start.is_finite() || start < 0.0 || end.is_some_and(|end| !end.is_finite() || end < start) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid waveform segment").into());
        }
    }
    let mut format = open(path)?;
    let known_duration = format.default_track().and_then(|track| {
        let params = &track.codec_params;
        let frames = params.n_frames?;
        let rate = params.sample_rate?;
        (rate > 0 && frames > 0).then_some(frames as f64 / f64::from(rate))
    });
    let duration = match known_duration {
        Some(duration) => duration,
        None => {
            let duration = stream(format.as_mut(), |_, _| {})?;
            format = open(path)?;
            duration
        }
    };
    let bounds: Vec<_> = segments.iter().map(|&(start, end)| {
        (start.min(duration), end.unwrap_or(duration).min(duration))
    }).collect();
    let mut energies = vec![[0.0_f64; BINS]; segments.len()];
    let mut counts = vec![[0_u64; BINS]; segments.len()];
    stream(format.as_mut(), |time, energy| {
        for ((&(start, end), bins), counts) in bounds.iter().zip(&mut energies).zip(&mut counts) {
            if time >= start && time < end {
                let bin = (((time - start) / (end - start)) * BINS as f64) as usize;
                let bin = bin.min(BINS - 1);
                bins[bin] += f64::from(energy);
                counts[bin] += 1;
            }
        }
    })?;
    Ok(energies.into_iter().zip(counts).map(|(bins, counts)| {
        bins.into_iter().zip(counts).map(|(energy, count)| {
            if count == 0 { 0 } else { ((energy / count as f64).sqrt() * 255.0).round() as u8 }
        }).collect()
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct PeakSpool { file: Option<File>, path: PathBuf }
    impl PeakSpool {
        fn new() -> std::io::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let path = std::env::temp_dir().join(format!("player-waveform-test-{}-{}.wav", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => return Ok(Self { file: Some(file), path }),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    impl Drop for PeakSpool {
        fn drop(&mut self) {
            drop(self.file.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }


    fn wav(channels: u16, samples: &[i16]) -> PeakSpool {
        let mut file = PeakSpool::new().unwrap();
        let bytes = (samples.len() * 2) as u32;
        file.file.as_mut().unwrap().write_all(b"RIFF").unwrap();
        file.file.as_mut().unwrap().write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.file.as_mut().unwrap().write_all(b"WAVEfmt \x10\0\0\0\x01\0").unwrap();
        file.file.as_mut().unwrap().write_all(&channels.to_le_bytes()).unwrap();
        file.file.as_mut().unwrap().write_all(&1024_u32.to_le_bytes()).unwrap();
        file.file.as_mut().unwrap().write_all(&(1024 * u32::from(channels) * 2).to_le_bytes()).unwrap();
        file.file.as_mut().unwrap().write_all(&(channels * 2).to_le_bytes()).unwrap();
        file.file.as_mut().unwrap().write_all(&16_u16.to_le_bytes()).unwrap();
        file.file.as_mut().unwrap().write_all(b"data").unwrap();
        file.file.as_mut().unwrap().write_all(&bytes.to_le_bytes()).unwrap();
        for sample in samples {
            file.file.as_mut().unwrap().write_all(&sample.to_le_bytes()).unwrap();
        }
        file.file.as_mut().unwrap().flush().unwrap();
        file
    }

    #[test]
    fn silence_has_exactly_256_zero_bins() {
        let source = wav(1, &[0; 1024]);
        assert_eq!(decode(&source.path, &[(0.0, None)]).unwrap(), vec![vec![0; BINS]]);
    }

    #[test]
    fn preserves_absolute_amplitude_without_normalization() {
        let source = wav(1, &[8192; 1024]);
        assert_eq!(decode(&source.path, &[(0.0, None)]).unwrap(), vec![vec![64; BINS]]);
    }

    #[test]
    fn opposite_channels_do_not_cancel() {
        let samples: Vec<_> = (0..1024).flat_map(|_| [16384, -16384]).collect();
        let source = wav(2, &samples);
        assert_eq!(decode(&source.path, &[(0.0, None)]).unwrap(), vec![vec![128; BINS]]);
    }

    #[test]
    fn transient_energy_is_lower_than_sustained_at_the_same_peak() {
        let samples: Vec<_> = (0..8192).map(|frame| {
            if frame < 4096 && frame % 32 != 0 { 0 } else { 32767 }
        }).collect();
        let source = wav(1, &samples);
        let result = decode(&source.path, &[(0.0, None)]).unwrap();
        assert!(result[0][..128].iter().all(|&value| value == 45));
        assert!(result[0][128..].iter().all(|&value| value == 255));
    }

    #[test]
    fn silent_channel_contributes_zero_energy() {
        let samples: Vec<_> = (0..1024).flat_map(|_| [16384, 0]).collect();
        let source = wav(2, &samples);
        assert_eq!(decode(&source.path, &[(0.0, None)]).unwrap(), vec![vec![90; BINS]]);
    }

    #[test]
    fn cue_segments_use_exclusive_bounds_and_actual_source_end() {
        let samples: Vec<_> = (0..2048).map(|frame| if frame < 1024 { 8192 } else { -24576 }).collect();
        let source = wav(1, &samples);
        let result = decode(&source.path, &[(0.0, Some(1.0)), (1.0, None), (1.0, Some(9.0)), (3.0, None)]).unwrap();
        assert_eq!(result, vec![vec![64; BINS], vec![191; BINS], vec![191; BINS], vec![0; BINS]]);
    }
}
