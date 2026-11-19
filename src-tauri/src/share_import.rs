//! Import one privately staged Android share without consuming the caller's file.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use tauri::Manager;

use crate::import::import_dropped_files;
use crate::library::{is_audio_extension, LibraryState};

pub(crate) async fn import_shared(
    app: &tauri::AppHandle,
    path: &Path,
    name: &str,
) -> Result<(), String> {
    let filename = Path::new(name);
    if filename.file_name().and_then(|s| s.to_str()) != Some(name)
        || name.contains(['\\', '\0'])
        || !filename.extension().and_then(|s| s.to_str()).is_some_and(|ext| {
            is_audio_extension(&ext.to_ascii_lowercase())
        })
    {
        return Err("Expected a shared audio filename".into());
    }

    let app = app.clone();
    let path = path.to_owned();
    let name = name.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        if !fs::symlink_metadata(&path).map_err(|e| e.to_string())?.is_file() {
            return Err("Expected a regular staged file".into());
        }
        let import = |source: &Path| {
            let source = source.to_str().ok_or("Invalid staged file path")?;
            let state = app.try_state::<LibraryState>().ok_or("Library unavailable")?;
            let result = import_dropped_files(app.clone(), state, vec![source.to_owned()])?;
            // An already-indexed duplicate also returns its track ID. A copied
            // but unindexed file, on the other hand, isn't a successful import.
            if result.imported_track_ids.is_empty() {
                return Err(format!("Could not import {name} into the library"));
            }
            Ok(())
        };
        if path.file_name() == Some(name.as_ref()) {
            return import(&path);
        }

        // Move the private copy: Android SELinux forbids hard links here.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = path.parent().ok_or("Missing staging directory")?;
        let dir = loop {
            let dir = parent.join(format!(".share-import-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        };
        let source = dir.join(&name);
        let result = fs::rename(&path, &source).map_err(|e| e.to_string()).and_then(|()| import(&source));
        let cleanup = fs::remove_dir_all(dir).map_err(|e| e.to_string());
        result.and(cleanup)
    })
    .await
    .map_err(|e| e.to_string())?
}
