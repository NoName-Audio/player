//! Android owns the transient content grant; Rust only consumes private staged copies.
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use tauri::{plugin::PluginHandle, AppHandle, Manager, Wry};

pub fn mobile_plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri::plugin::Builder::new("player-share-inbox")
        .setup(|app, api| {
            let handle = api.register_android_plugin("dev.verncat.player", "ShareInboxPlugin")?;
            app.manage(Inbox(handle));
            Ok(())
        })
        .build()
}

struct Inbox(PluginHandle<Wry>);

pub(crate) async fn choose_handler(app: &AppHandle, choices: Vec<String>) -> Option<usize> {
    #[derive(Deserialize)]
    struct Choice { index: i32 }
    let choice = app.state::<Inbox>().0.run_mobile_plugin_async::<Choice>(
        "chooseHandler", json!({"choices": choices}),
    ).await.ok()?;
    usize::try_from(choice.index).ok()
}

#[derive(Deserialize)]
struct Batch {
    items: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Item {
    Url { url: String },
    File { name: String, mime: String, path: PathBuf },
}

// Never read or unlink a path outside the host's staging directory.
fn staged_file(root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.symlink_metadata().ok()?.is_file() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    canonical.strip_prefix(root.canonicalize().ok()?).ok()?;
    Some(canonical)
}

async fn consume(app: &AppHandle, root: &Path, item: Item) {
    match item {
        Item::Url { url } => {
            crate::plugins::dispatch_share(app, json!({"url": url}), None).await;
        }
        Item::File { name, mime, path } => {
            let Some(file) = staged_file(root, &path) else {
                tracing::warn!("Ignoring missing or invalid Android share staging file");
                return;
            };
            let handled = crate::plugins::dispatch_share(app, json!({"name": name, "mime": mime}), Some(&file)).await;
            if !handled {
                if let Err(error) = crate::share_import::import_shared(app, &file, &name).await {
                    tracing::warn!(%error, "Could not import shared file");
                }
            }
            if let Err(error) = std::fs::remove_file(file) {
                if error.kind() != std::io::ErrorKind::NotFound { tracing::warn!(%error, "Could not remove Android share staging file"); }
            }
        }
    }
}

pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let root = match app.path().app_cache_dir() {
            Ok(cache) => cache.join("share-inbox"),
            Err(error) => {
                tracing::error!(%error, "Cannot resolve Android share staging directory");
                return;
            }
        };
        let inbox = app.state::<Inbox>();
        loop {
            match inbox.0.run_mobile_plugin_async::<Batch>("awaitItems", json!({})).await {
                Ok(batch) => {
                    for item in batch.items {
                        consume(&app, &root, item).await;
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "Android share inbox unavailable");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    });
}
