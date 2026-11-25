use std::path::Path;

use crate::plugin_sources::ResourceIdentity;

pub(super) fn clean_staging(root: &Path) {
    let Ok(entries) = root.read_dir() else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else { continue };
        // Leave recent jobs alone: another Player instance may still be using them.
        if metadata.is_dir() && metadata.modified().ok().and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age >= std::time::Duration::from_secs(86400)) {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                tracing::warn!(%error, path = %path.display(), "Could not clean YouTube staging");
            }
        }
    }
}

fn youtube_directory(root: &Path, id: &str) -> Result<std::path::PathBuf, String> {
    if id.len() != 11 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        return Err("Invalid YouTube video ID".into());
    }
    let key = blake3::hash(id.as_bytes()).to_hex();
    Ok(root.join("YouTube").join(key.as_str()))
}

pub(super) fn youtube_target(root: &Path, id: &str) -> Result<Option<std::path::PathBuf>, String> {
    let directory = youtube_directory(root, id)?;
    let entries = match directory.read_dir() {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !directory.canonicalize().map_err(|e| e.to_string())?.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
        return Err("YouTube destination escapes the library".into());
    }
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file()
            && matches!(entry.path().extension().and_then(|ext| ext.to_str()), Some("webm" | "mp4")) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

pub(super) fn save_youtube(root: &Path, source: &Path, resource: &ResourceIdentity) -> Result<std::path::PathBuf, String> {
    if let Some(existing) = youtube_target(root, &resource.resource_id)? {
        return Ok(existing);
    }
    let directory = youtube_directory(root, &resource.resource_id)?;
    let extension = match source.extension().and_then(|ext| ext.to_str()) {
        Some(ext @ ("webm" | "mp4")) => ext,
        _ => return Err("Unsupported YouTube media container".into()),
    };
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !directory.canonicalize().map_err(|e| e.to_string())?.starts_with(root) {
        return Err("YouTube destination escapes the library".into());
    }
    let title: String = resource.title.chars().take(48).collect();
    let title = crate::import_paths::sanitize_path_segment(&title);
    let target = directory.join(format!("{title}.{extension}"));
    // The temporary extension keeps the watcher from indexing an incomplete file.
    let temp = directory.join(".import.partial");
    // Remove a leftover from an interrupted import without following symlinks.
    match std::fs::remove_file(&temp) {
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e.to_string()),
    }
    let result = write_tags(source, &temp, extension, resource)
        .and_then(|()| std::fs::rename(&temp, &target).map_err(|e| e.to_string()));
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result.map(|()| target)
}

#[cfg(feature = "native-video")]
fn write_tags(source: &Path, target: &Path, extension: &str, resource: &ResourceIdentity) -> Result<(), String> {
    use ffmpeg_next as ff;
    let remux = || -> Result<(), ff::Error> {
        ff::init()?;
        let mut options = ff::Dictionary::new();
        options.set("protocol_whitelist", "file");
        let mut input = ff::format::input_with_dictionary(source, options)?;
        let mut output = ff::format::output_as(target, extension)?;
        let mut streams = vec![None; input.nb_streams() as usize];
        for stream in input.streams() {
            if !matches!(stream.parameters().medium(), ff::media::Type::Audio | ff::media::Type::Video) {
                continue;
            }
            let mut dest = output.add_stream(ff::encoder::find(ff::codec::Id::None))?;
            dest.set_parameters(stream.parameters());
            unsafe { (*dest.parameters().as_mut_ptr()).codec_tag = 0; }
            streams[stream.index()] = Some(dest.index());
        }
        let mut tags = input.metadata().to_owned();
        for (key, value) in [("title", &resource.title), ("artist", &resource.artist), ("album", &resource.album)] {
            if !value.trim().is_empty() {
                tags.set(key, &value.replace('\0', ""));
            }
        }
        output.set_metadata(tags);
        output.write_header()?;
        for (stream, mut packet) in input.packets() {
            if let Some(index) = streams[stream.index()] {
                packet.rescale_ts(stream.time_base(), output.stream(index).unwrap().time_base());
                packet.set_stream(index);
                packet.set_position(-1);
                packet.write_interleaved(&mut output)?;
            }
        }
        output.write_trailer()
    };
    remux().map_err(|error| format!("Could not write YouTube media tags: {error}"))
}

#[cfg(not(feature = "native-video"))]
fn write_tags(_: &Path, _: &Path, _: &str, _: &ResourceIdentity) -> Result<(), String> {
    Err("Saving tagged YouTube media requires a Player build with native media support".into())
}

#[cfg(test)]
#[test]
fn youtube_presence_tracks_the_saved_file() {
    let root = std::env::temp_dir().join(format!("player-youtube-presence-{}", std::process::id()));
    let id = "dQw4w9WgXcQ";
    let directory = youtube_directory(&root, id).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("renamed.webm");
    std::fs::write(&file, []).unwrap();
    assert_eq!(youtube_target(&root, id).unwrap(), Some(file.clone()));
    assert_eq!(youtube_target(&root, "abcdefghijk").unwrap(), None);
    std::fs::remove_file(file).unwrap();
    assert_eq!(youtube_target(&root, id).unwrap(), None);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(test, feature = "native-video"))]
mod tests {
    use super::*;

    #[test]
    fn youtube_import_preserves_tags_and_reuses_video_identity() {
        let root = std::env::temp_dir().join(format!("player-youtube-import-{}", std::process::id()));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mp4-opus.mp4");
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("preview.webm");
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opus-mono.opus"), &input).unwrap();
        let mut resource = ResourceIdentity {
            plugin_id: "player.youtube".into(), resource_id: "dQw4w9WgXcQ".into(),
            title: "A title".into(), artist: "An artist".into(), album: "An album".into(),
            duration_seconds: None, cover_hash: None,
        };
        let saved = save_youtube(&root, &input, &resource).unwrap();
        let (title, artist, album, _) = crate::video::native::container_metadata(&saved).unwrap();
        assert_eq!((title.as_deref(), artist.as_deref(), album.as_deref()), (Some("A title"), Some("An artist"), Some("An album")));
        resource.title = "Changed title".into();
        assert_eq!(save_youtube(&root, &source, &resource).unwrap(), saved);
        std::fs::remove_dir_all(root).unwrap();
    }
}
