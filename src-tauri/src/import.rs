//! Drag-and-drop import of audio files into the library.
//!
//! Dropped files/folders are copied (never moved) into `data_dir` following
//! the same path convention as Soulseek downloads:
//! `Artist/Album/filename`, `Artist/filename`, or `filename` when tags are
//! missing. Copied files are indexed immediately.

use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::Emitter;
use walkdir::WalkDir;

use crate::import_paths::{build_metadata_based_path, extract_audio_metadata};
use crate::library::{
    cue_audio_files, index_cue_file, index_file, is_audio_extension, is_cue_extension, rel_path,
    LibraryState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub imported_track_ids: Vec<i64>,
}

/// Matches the `index-progress` payload emitted by library indexing so the
/// existing frontend index log surfaces drag-and-drop imports too.
#[derive(Clone, Serialize)]
struct ImportProgress {
    current: usize,
    total: usize,
    status: &'static str,
    added: usize,
    track_name: Option<String>,
}

fn emit_progress(app: &tauri::AppHandle, progress: ImportProgress) {
    let _ = app.emit("index-progress", progress);
}

fn track_ids_for(conn: &Connection, rel: &str) -> Vec<i64> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id FROM tracks WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '#'",
    ) else {
        return Vec::new();
    };
    stmt.query_map([rel], |row| row.get::<_, i64>(0))
        .map(|rows| rows.filter_map(|row| row.ok()).collect())
        .unwrap_or_default()
}

#[tauri::command]
pub fn import_dropped_files(
    app: tauri::AppHandle,
    state: tauri::State<'_, LibraryState>,
    paths: Vec<String>,
) -> Result<ImportResult, String> {
    let data_dir = state.data_dir().to_path_buf();
    let canonical_data_dir = fs::canonicalize(&data_dir).unwrap_or_else(|_| data_dir.clone());

    let mut sources: Vec<PathBuf> = Vec::new();
    for raw in &paths {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            for entry in WalkDir::new(&path).into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    sources.push(entry.path().to_path_buf());
                }
            }
        } else if path.is_file() {
            sources.push(path);
        }
    }

    let extra_audio: Vec<PathBuf> = sources
        .iter()
        .filter(|source| {
            source
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| is_cue_extension(&value.to_lowercase()))
        })
        .flat_map(|source| cue_audio_files(source))
        .filter(|audio| audio.is_file() && !sources.contains(audio))
        .collect();
    for audio in extra_audio {
        if !sources.contains(&audio) {
            sources.push(audio);
        }
    }
    sources.sort_by_key(|source| {
        source
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| is_cue_extension(&value.to_lowercase()))
    });
    let mut audio_target_dirs: HashMap<PathBuf, PathBuf> = HashMap::new();

    let mut imported = 0usize;
    let mut imported_track_ids: Vec<i64> = Vec::new();
    let mut skipped = 0usize;
    let total = sources.len();

    if total > 0 {
        emit_progress(
            &app,
            ImportProgress {
                current: 0,
                total,
                status: "indexing",
                added: 0,
                track_name: None,
            },
        );
    }

    for source in sources {
        let ext = source
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_lowercase())
            .unwrap_or_default();
        if !is_audio_extension(&ext) && !is_cue_extension(&ext) {
            skipped += 1;
            continue;
        }

        // Skip files that already live inside the library.
        if let Ok(canonical) = fs::canonicalize(&source) {
            if canonical.starts_with(&canonical_data_dir) {
                let conn = state.conn();
                let conn = conn.lock().unwrap();
                imported_track_ids.extend(track_ids_for(&conn, &rel_path(&canonical_data_dir, &canonical)));
                skipped += 1;
                continue;
            }
        }

        let Some(filename) = source.file_name() else {
            skipped += 1;
            continue;
        };

        let (artist, album) = if is_audio_extension(&ext) {
            extract_audio_metadata(&source)
        } else {
            (None, None)
        };
        let target_dir = if is_cue_extension(&ext) {
            cue_audio_files(&source)
                .iter()
                .find_map(|audio| audio_target_dirs.get(audio).cloned())
                .unwrap_or_else(|| data_dir.clone())
        } else {
            build_metadata_based_path(&data_dir, artist.as_deref(), album.as_deref())
        };
        if let Err(e) = fs::create_dir_all(&target_dir) {
            eprintln!(
                "[import] failed to create directory {}: {}",
                target_dir.display(),
                e
            );
            skipped += 1;
            continue;
        }
        let target = target_dir.join(filename);
        if is_audio_extension(&ext) {
            audio_target_dirs.insert(source.clone(), target_dir.clone());
        }

        if target.exists() {
            let conn = state.conn();
            let conn = conn.lock().unwrap();
            imported_track_ids.extend(track_ids_for(&conn, &rel_path(&data_dir, &target)));
            skipped += 1;
            continue;
        }

        if let Err(e) = fs::copy(&source, &target) {
            eprintln!(
                "[import] failed to copy {} -> {}: {}",
                source.display(),
                target.display(),
                e
            );
            skipped += 1;
            continue;
        }

        {
            let conn = state.conn();
            let conn = conn.lock().unwrap();
            let indexed = if is_cue_extension(&ext) {
                index_cue_file(&conn, &data_dir, &target).map(|_| ())
            } else {
                index_file(&conn, &data_dir, &target).map(|_| ())
            };
            if let Err(e) = indexed {
                eprintln!("[import] failed to index {}: {}", target.display(), e);
            }
            imported_track_ids.extend(track_ids_for(&conn, &rel_path(&data_dir, &target)));
        }
        imported += 1;
        emit_progress(
            &app,
            ImportProgress {
                current: imported,
                total,
                status: "added",
                added: imported,
                track_name: target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
            },
        );
    }

    if total > 0 {
        emit_progress(
            &app,
            ImportProgress {
                current: total,
                total,
                status: "done",
                added: imported,
                track_name: None,
            },
        );
    }

    if imported > 0 {
        let _ = app.emit("library-changed", ());
    }

    Ok(ImportResult { imported, skipped, imported_track_ids })
}
