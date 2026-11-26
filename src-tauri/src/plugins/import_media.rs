use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::plugin_sources::ResourceIdentity;

pub(super) fn clean_legacy_download_staging(root: &Path) {
    let Ok(entries) = root.read_dir() else { return };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("youtube-") {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else { continue };
        // Leave recent jobs alone: another Player instance may still be using them.
        if metadata.is_dir() && metadata.modified().ok().and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age >= std::time::Duration::from_secs(86400)) {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                tracing::warn!(%error, path = %path.display(), "Could not clean media staging");
            }
        }
    }
}

fn legacy_youtube_directory(root: &Path, id: &str) -> Option<PathBuf> {
    if id.len() != 11 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        return None;
    }
    let key = blake3::hash(id.as_bytes()).to_hex();
    Some(root.join("YouTube").join(key.as_str()))
}

fn resource_directory(root: &Path, plugin_id: &str, id: &str) -> PathBuf {
    root.join("Plugin Imports")
        .join(blake3::hash(plugin_id.as_bytes()).to_hex().as_str())
        .join(blake3::hash(id.as_bytes()).to_hex().as_str())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 4000 {
        return Err("Invalid resource ID".into());
    }
    Ok(())
}

fn check_contained(root: &Path, directory: &Path) -> Result<(), String> {
    let relative = directory.strip_prefix(root).map_err(|e| e.to_string())?;
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canonical_dir = directory.canonicalize().map_err(|e| e.to_string())?;
    if relative.as_os_str().is_empty() || canonical_dir != canonical_root.join(relative) {
        return Err("Import directory escapes its root".into());
    }
    Ok(())
}

fn media_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    crate::library::is_audio_extension(&ext).then_some(ext)
}

fn find_media(root: &Path, directory: &Path, legacy: bool) -> Result<Option<PathBuf>, String> {
    let entries = match directory.read_dir() {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    check_contained(root, directory)?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file()
            && media_extension(&entry.path()).is_some_and(|ext| !legacy || matches!(ext.as_str(), "webm" | "mp4")) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

pub(super) fn target(root: &Path, plugin_id: &str, id: &str) -> Result<Option<PathBuf>, String> {
    validate_id(id)?;
    if let Some(path) = find_media(root, &resource_directory(root, plugin_id, id), false)? {
        return Ok(Some(path));
    }
    // Existing YouTube imports predate generic resource identity storage. Keep
    // their paths (and indexed track IDs) rather than moving or duplicating them.
    if plugin_id == "player.youtube" {
        if let Some(directory) = legacy_youtube_directory(root, id) {
            return find_media(root, &directory, true);
        }
    }
    Ok(None)
}

pub(super) struct StagingDir(PathBuf);

impl StagingDir {
    pub(super) fn new(root: &Path) -> Result<Self, String> {
        static NEXT_IMPORT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        loop {
            let serial = NEXT_IMPORT.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(".import-{}-{serial}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("Could not stage media import: {e}")),
            }
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn save(root: &Path, work: &Path, source: &Path, resource: &ResourceIdentity) -> Result<PathBuf, String> {
    let plugin_id = &resource.plugin_id;
    let id = &resource.resource_id;
    if let Some(existing) = target(root, plugin_id, id)? {
        return Ok(existing);
    }
    let extension = media_extension(source).ok_or("Unsupported media container")?;
    let directory = resource_directory(root, plugin_id, id);
    let parent = directory.parent().expect("resource directory has a parent");
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    check_contained(root, parent)?;
    let title = if resource.title.trim().is_empty() {
        source.file_stem().and_then(|name| name.to_str()).unwrap_or("media")
    } else {
        &resource.title
    };
    let title: String = title.chars().take(48).collect();
    let title = crate::import_paths::sanitize_path_segment(&title);
    let filename = format!("{title}.{extension}");

    // A library-internal staging directory keeps unfinished media out of the
    // scanner and on the same volume as its final path.
    let staging_root = root.join(".player-transactions").join("plugin-imports");
    fs::create_dir_all(&staging_root).map_err(|e| e.to_string())?;
    check_contained(root, &staging_root)?;
    let staging = StagingDir::new(&staging_root)?;
    let staged = staging.path().join(&filename);
    // Do not move the original; a decoder may still own it. Remux compatible
    // containers on seekable app-private storage before copying to the library.
    if cfg!(feature = "native-video") && matches!(extension.as_str(), "webm" | "mp4") {
        let private = StagingDir::new(work)?;
        let remuxed = private.path().join(&filename);
        write_tags(source, &remuxed, &extension, resource)?;
        fs::copy(&remuxed, &staged).map_err(|e| e.to_string())?;
    } else {
        fs::copy(source, &staged).map_err(|e| e.to_string())?;
    }

    // Publish the entire resource directory. A competing process that already
    // published a nonempty directory wins, even if it used a different title.
    // Remove an empty directory left by library deletion. This never removes
    // a competing import, and also allows publication on Windows.
    let _ = fs::remove_dir(&directory);
    match fs::rename(staging.path(), &directory) {
        Ok(()) => Ok(directory.join(filename)),
        Err(error) => target(root, plugin_id, id)?.ok_or_else(|| format!("Could not publish media import: {error}")),
    }
}

#[cfg(feature = "native-video")]
fn write_tags(source: &Path, target: &Path, extension: &str, resource: &ResourceIdentity) -> Result<(), String> {
    use ffmpeg_next as ff;
    let remux = || -> Result<(), (&'static str, ff::Error)> {
        ff::init().map_err(|e| ("initialize FFmpeg", e))?;
        let mut options = ff::Dictionary::new();
        options.set("protocol_whitelist", "file");
        let mut input = ff::format::input_with_dictionary(source, options).map_err(|e| ("open source", e))?;
        let mut output = ff::format::output_as(target, extension).map_err(|e| ("open staging output", e))?;
        let mut streams = vec![None; input.nb_streams() as usize];
        for stream in input.streams() {
            if !matches!(stream.parameters().medium(), ff::media::Type::Audio | ff::media::Type::Video) {
                continue;
            }
            let mut dest = output.add_stream(ff::encoder::find(ff::codec::Id::None)).map_err(|e| ("copy stream", e))?;
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
        output.write_header().map_err(|e| ("write container header", e))?;
        for (stream, mut packet) in input.packets() {
            if let Some(index) = streams[stream.index()] {
                packet.rescale_ts(stream.time_base(), output.stream(index).unwrap().time_base());
                packet.set_stream(index);
                packet.set_position(-1);
                packet.write_interleaved(&mut output).map_err(|e| ("write media packet", e))?;
            }
        }
        output.write_trailer().map_err(|e| ("finalize container", e))
    };
    remux().map_err(|(stage, error)| format!("Could not save {extension}: {stage}: {error}"))
}

#[cfg(not(feature = "native-video"))]
fn write_tags(_: &Path, _: &Path, _: &str, _: &ResourceIdentity) -> Result<(), String> {
    Err("Saving tagged media requires a Player build with native media support".into())
}

#[cfg(test)]
#[test]
fn legacy_youtube_presence_tracks_the_saved_file() {
    let root = std::env::temp_dir().join(format!("player-youtube-presence-{}", std::process::id()));
    let id = "dQw4w9WgXcQ";
    let directory = legacy_youtube_directory(&root, id).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("renamed.webm");
    std::fs::write(&file, []).unwrap();
    assert_eq!(target(&root, "player.youtube", id).unwrap(), Some(file.clone()));
    let resource = ResourceIdentity {
        plugin_id: "player.youtube".into(), resource_id: id.into(),
        title: "New title".into(), artist: String::new(), album: String::new(),
        duration_seconds: None, cover_hash: None,
    };
    assert_eq!(save(&root, &root, &root.join("new.webm"), &resource).unwrap(), file);
    assert_eq!(target(&root, "another.plugin", id).unwrap(), None);
    assert_eq!(target(&root, "player.youtube", "abcdefghijk").unwrap(), None);
    std::fs::remove_file(file).unwrap();
    assert_eq!(target(&root, "player.youtube", id).unwrap(), None);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(test)]
#[test]
fn copied_media_is_deduplicated_only_within_its_plugin_and_resource() {
    let root = StagingDir::new(&std::env::temp_dir()).unwrap();
    let source = root.path().join("download.FLAC");
    let library = root.path().join("library");
    let work = root.path().join("private");
    fs::write(&source, b"media").unwrap();
    let mut resource = ResourceIdentity {
        plugin_id: "test.first".into(), resource_id: "../../opaque:id".into(),
        title: "First title".into(), artist: String::new(), album: String::new(),
        duration_seconds: None, cover_hash: None,
    };
    let first = save(&library, &work, &source, &resource).unwrap();
    assert_eq!(fs::read(&first).unwrap(), b"media");
    assert_eq!(fs::read(&source).unwrap(), b"media");
    resource.title = "Other title".into();
    assert_eq!(save(&library, &work, &source, &resource).unwrap(), first);
    resource.plugin_id = "test.second".into();
    assert_eq!(target(&library, &resource.plugin_id, &resource.resource_id).unwrap(), None);
    let second = save(&library, &work, &source, &resource).unwrap();
    assert_ne!(second, first);
    let renamed = second.with_file_name("renamed.flac");
    fs::rename(second, &renamed).unwrap();
    assert_eq!(target(&library, &resource.plugin_id, &resource.resource_id).unwrap(), Some(renamed.clone()));
    fs::remove_file(renamed).unwrap();
    assert_eq!(target(&library, &resource.plugin_id, &resource.resource_id).unwrap(), None);
    assert!(save(&library, &work, &source, &resource).unwrap().is_file());
    let pending = library.join(".player-transactions/plugin-imports");
    assert!(pending.read_dir().unwrap().next().is_none());
}

#[cfg(all(test, feature = "native-video"))]
mod tests {
    use super::*;

    #[test]
    fn remux_preserves_tags_and_reuses_resource_identity() {
        let root = std::env::temp_dir().join(format!("player-media-import-{}", std::process::id()));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mp4-opus.mp4");
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("preview.webm");
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio-only.webm"), &input).unwrap();
        let mut resource = ResourceIdentity {
            plugin_id: "test.plugin".into(), resource_id: "resource-a".into(),
            title: "A title".into(), artist: "An artist".into(), album: "An album".into(),
            duration_seconds: None, cover_hash: None,
        };
        let saved = save(&root, &root, &input, &resource).unwrap();
        let (title, artist, album, _) = crate::video::native::container_metadata(&saved).unwrap();
        assert_eq!((title.as_deref(), artist.as_deref(), album.as_deref()), (Some("A title"), Some("An artist"), Some("An album")));
        resource.title = "Changed title".into();
        assert_eq!(save(&root, &root, &source, &resource).unwrap(), saved);
        resource.resource_id = "resource-b".into();
        // A previous job must not block or share the next import's staging path.
        let directory = root.join(".player-transactions/plugin-imports");
        std::fs::create_dir_all(directory.join(".import.partial")).unwrap();
        let video = save(&root, &root, &source, &resource).unwrap();
        let (title, _, _, _) = crate::video::native::container_metadata(&video).unwrap();
        assert_eq!(title.as_deref(), Some("Changed title"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
