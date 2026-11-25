//! Native Plugin API 2 dispatcher. Shared libraries and APKs use the same lifecycle/UI contract.
use super::{
    model::{native_target, NativePackage, Package},
    PluginState,
};
use player_plugin_sdk::{Buffer, Host, HostApi, Result as PluginResult};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::{c_void, CStr},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tauri::{Emitter, Manager};

type Create = unsafe extern "C" fn() -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Call = unsafe extern "C" fn(*mut c_void, *const u8, usize, HostApi) -> Buffer;
type Free = unsafe extern "C" fn(Buffer);
struct LibraryInstance {
    _library: Arc<libloading::Library>,
    directory: std::path::PathBuf,
    instance: *mut c_void,
    call: Call,
    free: Free,
    destroy: Destroy,
}
// Access to the ABI instance is serialized by its activation mutex.
unsafe impl Send for LibraryInstance {}
impl Drop for LibraryInstance {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.instance) }
    }
}
enum Backend {
    Library(LibraryInstance),
    Android,
}
struct Activation {
    generation: u64,
    backend: Backend,
    initialized: bool,
}
#[derive(Default)]
pub struct NativeRuntime {
    activations: Mutex<BTreeMap<String, Arc<Mutex<Activation>>>>,
    closing: AtomicBool,
}
#[derive(Clone)]
struct Context {
    app: tauri::AppHandle,
    id: String,
    generation: u64,
    directory: Option<std::path::PathBuf>,
    share_file: Option<Arc<Mutex<std::fs::File>>>,
}
struct Reply {
    id: String,
    generation: u64,
    send: std::sync::mpsc::Sender<PluginResult>,
}
// Library code remains mapped until process exit, including after window/runtime teardown.
static LIBRARIES: OnceLock<Mutex<Vec<Arc<libloading::Library>>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static REPLIES: OnceLock<Mutex<BTreeMap<u64, Reply>>> = OnceLock::new();
fn replies() -> &'static Mutex<BTreeMap<u64, Reply>> {
    REPLIES.get_or_init(Default::default)
}
#[cfg(target_os = "android")]
static ANDROID_CONTEXTS: OnceLock<Mutex<BTreeMap<u64, Context>>> = OnceLock::new();
impl Context {
    fn active(&self) -> Result<(), String> {
        if self
            .app
            .state::<NativeRuntime>()
            .closing
            .load(Ordering::Acquire)
        {
            return Err("Native runtime is closing".into());
        }
        let state = self.app.state::<PluginState>();
        let record = state.record(&self.id)?;
        if !record.enabled || record.generation != self.generation {
            return Err("DISABLED: plugin activation ended".into());
        }
        if !state
            .manifest(&self.id)?
            .platforms
            .iter()
            .any(|p| p == super::model::platform())
        {
            return Err("Unsupported native plugin platform".into());
        }
        Ok(())
    }
}
impl Host for Context {
    fn call(&self, method: &str, args: Value) -> PluginResult {
        self.active()?;
        if method == "share.read" {
            use base64::Engine;
            use std::io::{Read, Seek, SeekFrom};
            let file = self.share_file.as_ref().ok_or("No file in this share call")?;
            let offset = args["offset"].as_u64().ok_or("Invalid share offset")?;
            let length = args["length"].as_u64().filter(|n| (1..=65536).contains(n)).ok_or("Invalid share length")?;
            let mut file = file.lock().map_err(|e| e.to_string())?;
            file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            let mut chunk = vec![0; length as usize];
            let count = file.read(&mut chunk).map_err(|e| e.to_string())?;
            self.active()?;
            return Ok(json!({"data": base64::engine::general_purpose::STANDARD.encode(&chunk[..count])}));
        }
        if method == "runtime.directory" {
            let path = self.directory.as_ref().ok_or("Runtime directory is only available to native libraries")?;
            self.active()?;
            return Ok(json!({"path": path}));
        }
        if method == "host.invalidate" {
            let _ = self.app.emit(
                "plugin-invalidated",
                json!({"pluginId":self.id,"generation":self.generation}),
            );
            return Ok(Value::Null);
        }
        if method == "host.background" {
            let topic = args["topic"]
                .as_str()
                .ok_or("Missing background topic")?
                .to_string();
            let task_id = args["id"].clone();
            let method = args["method"]
                .as_str()
                .ok_or("Missing background method")?
                .to_string();
            if topic.len() > 100 || method.len() > 100 {
                return Err("Invalid background request".into());
            }
            let mut context = self.clone();
            context.share_file = None;
            let task_args = args["args"].clone();
            tauri::async_runtime::spawn(async move {
                let result = super::plugin_call(
                    context.id.clone(),
                    context.generation,
                    method,
                    task_args,
                    context.app.clone(),
                )
                .await;
                let payload = match result {
                    Ok(value) => json!({"id":task_id,"result":value}),
                    Err(error) => json!({"id":task_id,"error":error,"result":null}),
                };
                super::emit_generation_event(
                    &context.app,
                    &context.id,
                    context.generation,
                    &topic,
                    payload,
                );
            });
            return Ok(Value::Null);
        }
        if method.starts_with("host.") {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let (send, receive) = std::sync::mpsc::channel();
            replies().lock().map_err(|e| e.to_string())?.insert(
                id,
                Reply {
                    id: self.id.clone(),
                    generation: self.generation,
                    send,
                },
            );
            let emitted=self.app.emit("plugin-host-call",json!({"requestId":id,"pluginId":self.id,"generation":self.generation,"method":method,"args":args}));
            let result = if let Err(e) = emitted {
                Err(e.to_string())
            } else {
                receive
                    .recv_timeout(Duration::from_secs(45))
                    .map_err(|_| "Native host request timed out".to_string())
                    .and_then(|v| v)
            };
            replies().lock().map_err(|e| e.to_string())?.remove(&id);
            self.active()?;
            return result;
        }
        tauri::async_runtime::block_on(super::plugin_call(
            self.id.clone(),
            self.generation,
            method.into(),
            args,
            self.app.clone(),
        ))
    }
}
unsafe extern "C" fn host_call(context: *mut c_void, request: *const u8, len: usize) -> Buffer {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> PluginResult {
        if context.is_null() || request.is_null() || len > player_plugin_sdk::MAX_MESSAGE_BYTES {
            return Err("Invalid host request".into());
        }
        let value: Value = serde_json::from_slice(std::slice::from_raw_parts(request, len))
            .map_err(|e| e.to_string())?;
        (&*context.cast::<Context>()).call(
            value["method"].as_str().ok_or("Missing host method")?,
            value["args"].clone(),
        )
    }))
    .unwrap_or_else(|_| Err("Native host callback panicked".into()));
    Buffer::new(player_plugin_sdk::encode(result))
}

unsafe extern "C" fn deny_probe_host_call(_: *mut c_void, _: *const u8, _: usize) -> Buffer {
    Buffer::new(player_plugin_sdk::encode(Err("Host calls are unavailable during canShare".into())))
}
fn ensure_real_directory(path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if parent != path && !parent.as_os_str().is_empty() { ensure_real_directory(parent)?; }
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("Native runtime path is not a real directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound =>
            std::fs::create_dir(path).map_err(|e| e.to_string()),
        Err(error) => Err(error.to_string()),
    }
}
fn runtime_contents_match(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else { return false; };
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let length = remaining.len().min(buffer.len());
        if file.read_exact(&mut buffer[..length]).is_err() || buffer[..length] != remaining[..length] {
            return false;
        }
        remaining = &remaining[length..];
    }
    matches!(file.read(&mut buffer[..1]), Ok(0))
}
fn install_runtime_file(path: &std::path::Path, bytes: &[u8], executable: bool) -> Result<(), String> {
    use std::io::Write;
    let matches = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type().is_file() && !metadata.file_type().is_symlink()
            && metadata.len() == bytes.len() as u64 && runtime_contents_match(path, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    if !matches {
        let temporary = path.parent().ok_or("Missing runtime directory")?.join(format!(".runtime-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| e.to_string())?;
            file.write_all(bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            runtime_permissions(&temporary, executable)?;
            #[cfg(windows)]
            if let Ok(metadata) = std::fs::symlink_metadata(path) {
                if metadata.file_type().is_file() {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(false);
                    std::fs::set_permissions(path, permissions).map_err(|e| e.to_string())?;
                }
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            std::fs::rename(&temporary, path).map_err(|e| e.to_string())
        })();
        if result.is_err() { let _ = std::fs::remove_file(&temporary); }
        result?;
    }
    runtime_permissions(path, executable)
}
fn runtime_permissions(path: &std::path::Path, executable: bool) -> Result<(), String> {
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::Permissions::from_mode(if executable { 0o500 } else { 0o400 })
    };
    #[cfg(not(unix))]
    let permissions = {
        let _ = executable;
        let mut permissions = std::fs::metadata(path).map_err(|e| e.to_string())?.permissions();
        permissions.set_readonly(true);
        permissions
    };
    std::fs::set_permissions(path, permissions).map_err(|e| e.to_string())
}
pub(super) fn extract_artifact(root: &std::path::Path, artifact: &super::model::NativeArtifact) -> Result<(), String> {
    // Each payload is decoded and hash-verified below; only preflight names/sizes here.
    artifact.validate_file_layout()?;
    ensure_real_directory(root)?;
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    install_runtime_file(&root.join(format!("plugin.{}", std::env::consts::DLL_EXTENSION)), &artifact.decode()?, true)?;
    for (name, file) in &artifact.files {
        install_runtime_file(&root.join(name), &file.decode()?, file.executable)?;
    }
    Ok(())
}
fn library(
    app: &tauri::AppHandle,
    package: &Package,
    fingerprint: &str,
) -> Result<Backend, String> {
    let NativePackage::Library { artifacts } = &package.native else {
        return Err("Missing native artifacts".into());
    };
    let artifact = artifacts.get(&native_target())
        .ok_or("No native library for this platform and architecture")?;
    // The application data root is trusted; OS ancestors may legitimately be symlinks
    // (for example /var on macOS). Validate only the private native subtree below it.
    let base = std::fs::canonicalize(&app.state::<PluginState>().root).map_err(|e| e.to_string())?;
    let root = base.join("native").join(&package.manifest.id).join(fingerprint);
    extract_artifact(&root, artifact)?;
    let root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let path = root.join(format!("plugin.{}", std::env::consts::DLL_EXTENSION));
    // No library is opened during preview or installation: only after explicit enable.
    unsafe {
        let library = Arc::new(
            libloading::Library::new(&path)
                .map_err(|e| format!("Could not load native plugin: {e}"))?,
        );
        let version = *library
            .get::<unsafe extern "C" fn() -> u32>(b"player_plugin_abi_version\0")
            .map_err(|e| e.to_string())?;
        if version() != player_plugin_sdk::ABI_VERSION {
            return Err("Incompatible native ABI".into());
        }
        let identity = *library
            .get::<unsafe extern "C" fn() -> *const std::ffi::c_char>(b"player_plugin_id\0")
            .map_err(|e| e.to_string())?;
        let id = identity();
        if id.is_null() || CStr::from_ptr(id).to_bytes() != package.manifest.id.as_bytes() {
            return Err("Native plugin ID mismatch".into());
        }
        let create = *library
            .get::<Create>(b"player_plugin_create\0")
            .map_err(|e| e.to_string())?;
        let call = *library
            .get::<Call>(b"player_plugin_call\0")
            .map_err(|e| e.to_string())?;
        let free = *library
            .get::<Free>(b"player_plugin_free\0")
            .map_err(|e| e.to_string())?;
        let destroy = *library
            .get::<Destroy>(b"player_plugin_destroy\0")
            .map_err(|e| e.to_string())?;
        let instance = create();
        if instance.is_null() {
            return Err("Native plugin constructor failed".into());
        }
        LIBRARIES
            .get_or_init(Default::default)
            .lock()
            .map_err(|e| e.to_string())?
            .push(library.clone());
        Ok(Backend::Library(LibraryInstance {
            _library: library,
            directory: root,
            instance,
            call,
            free,
            destroy,
        }))
    }
}
fn backend(
    app: &tauri::AppHandle,
    package: &Package,
    fingerprint: &str,
) -> Result<Backend, String> {
    match &package.native {
        NativePackage::Library { .. } => library(app, package, fingerprint),
        NativePackage::AndroidApk => Ok(Backend::Android),
    }
}
// Attachment metadata is a host-owned scope, never an arbitrary plugin view selector.
fn validate_attachment_scope(
    manifest: &super::model::Manifest,
    method: &str,
    args: &Value,
) -> Result<(), String> {
    let scoped = method == "renderAttachment"
        || (method == "action"
            && ["attachmentId", "target", "instanceId"]
                .iter()
                .any(|key| args.get(key).is_some()));
    if !scoped {
        return Ok(());
    }
    if !manifest
        .permissions
        .iter()
        .any(|permission| permission == "ui")
    {
        return Err("PERMISSION_DENIED: ui".into());
    }
    let attachment = args["attachmentId"]
        .as_str()
        .and_then(|id| {
            manifest
                .attachments
                .iter()
                .find(|attachment| attachment.id == id)
        })
        .ok_or("Unknown attachment")?;
    if args["viewId"].as_str() != Some(attachment.view.as_str())
        || args["target"].as_str() != Some(attachment.target.as_str())
    {
        return Err("Attachment scope mismatch".into());
    }
    let instance = args["instanceId"]
        .as_str()
        .ok_or("Missing attachment instance")?;
    if instance.is_empty() || instance.len() > 256 || instance.chars().any(char::is_control) {
        return Err("Invalid attachment instance".into());
    }
    if !args.get("context").is_some_and(Value::is_object) {
        return Err("Missing attachment context".into());
    }
    Ok(())
}

fn dispatch(
    activation: &mut Activation,
    context: &mut Context,
    method: &str,
    args: Value,
) -> PluginResult {
    match &mut activation.backend {
        Backend::Library(plugin) => unsafe {
            let request = serde_json::to_vec(&json!({"method":method,"args":args}))
                .map_err(|e| e.to_string())?;
            let result = (plugin.call)(
                plugin.instance,
                request.as_ptr(),
                request.len(),
                HostApi {
                    context: (context as *mut Context).cast(),
                    call: if method == "canShare" { deny_probe_host_call } else { host_call },
                    free: Buffer::free,
                },
            );
            let value = result.read().and_then(player_plugin_sdk::decode);
            (plugin.free)(result);
            value
        },
        Backend::Android => {
            #[cfg(target_os = "android")]
            {
                let binding = context
                    .app
                    .state::<PluginState>()
                    .manifest(&context.id)?
                    .android_native
                    .ok_or("Missing APK binding")?;
                let token = NEXT.fetch_add(1, Ordering::Relaxed);
                ANDROID_CONTEXTS
                    .get_or_init(Default::default)
                    .lock()
                    .map_err(|e| e.to_string())?
                    .insert(token, context.clone());
                let response = super::android::mobile_call(
                    "call",
                    json!({"pluginId":context.id,"generation":context.generation,"binding":binding,"method":method,"argsJson":args.to_string(),"hostToken":token}),
                );
                ANDROID_CONTEXTS
                    .get_or_init(Default::default)
                    .lock()
                    .map_err(|e| e.to_string())?
                    .remove(&token);
                response.map(|v| v["result"].clone())
            }
            #[cfg(not(target_os = "android"))]
            {
                Err("APK plugins require Android".into())
            }
        }
    }
}
fn start_polling(app: tauri::AppHandle, id: String, generation: u64, interval: u64) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
            let current = app.state::<PluginState>().record(&id);
            if app.state::<NativeRuntime>().closing.load(Ordering::Acquire)
                || !current.is_ok_and(|r| r.enabled && r.generation == generation)
            {
                break;
            }
            if let Err(error) = plugin_native_call(
                app.clone(),
                id.clone(),
                generation,
                "poll".into(),
                json!({}),
            )
            .await
            {
                if error.contains("METHOD_NOT_FOUND")
                    || error.contains("DISABLED")
                    || error.contains("not active")
                {
                    break;
                }
                let _ = app.emit(
                    "plugin-runtime-error",
                    json!({"pluginId":id,"generation":generation,"error":error}),
                );
            }
        }
    });
}
#[tauri::command]
pub async fn plugin_native_call(
    app: tauri::AppHandle,
    plugin_id: String,
    generation: u64,
    method: String,
    args: Value,
) -> PluginResult {
    if ![
        "activate",
        "poll",
        "render",
        "renderAttachment",
        "action",
        "search",
        "renderSearch",
        "playback",
        "event",
    ]
    .contains(&method.as_str())
    {
        return Err("Unknown native operation".into());
    }
    if serde_json::to_vec(&args).map_err(|e| e.to_string())?.len()
        > player_plugin_sdk::MAX_MESSAGE_BYTES
    {
        return Err("Native request exceeds 1 MiB".into());
    }
    native_call(app, plugin_id, generation, method, args, None).await
}

async fn native_call(app: tauri::AppHandle, plugin_id: String, generation: u64, method: String,
    args: Value, share_file: Option<Arc<Mutex<std::fs::File>>>) -> PluginResult {
    tauri::async_runtime::spawn_blocking(move || {
        let mut context = Context {
            app: app.clone(),
            id: plugin_id.clone(),
            generation,
            directory: None,
            share_file,
        };
        context.active()?;
        if matches!(method.as_str(), "share" | "canShare") {
            let state = app.state::<PluginState>();
            state.authorize(&plugin_id, generation, "library.import")?;
            if !state.manifest(&plugin_id)?.contributions.iter().any(|c| c.kind == "share-handler") {
                return Err("No share-handler contribution".into());
            }
        }
        validate_attachment_scope(
            &app.state::<PluginState>().manifest(&plugin_id)?,
            &method,
            &args,
        )?;
        let runtime = app.state::<NativeRuntime>();
        let activation = {
            let mut activations = runtime.activations.lock().map_err(|e| e.to_string())?;
            if let Some(existing) = activations.get(&plugin_id) {
                existing.clone()
            } else {
                if method != "activate" {
                    return Err("Native plugin is not active".into());
                }
                let package = app.state::<PluginState>().package(&plugin_id)?;
                let fingerprint = app.state::<PluginState>().info(&plugin_id)?.fingerprint;
                let entry = Arc::new(Mutex::new(Activation {
                    generation,
                    backend: backend(&app, &package, &fingerprint)?,
                    initialized: false,
                }));
                activations.insert(plugin_id.clone(), entry.clone());
                entry
            }
        };
        let mut activation = activation.lock().map_err(|e| e.to_string())?;
        context.active()?;
        if activation.generation != generation {
            return Err("Stale native activation; disable and enable the plugin".into());
        }
        if let Backend::Library(library) = &activation.backend {
            context.directory = Some(library.directory.clone());
        }
        if matches!(method.as_str(), "share" | "canShare") && !activation.initialized {
            return Err("Native plugin is not active".into());
        }
        if method == "activate" && activation.initialized {
            return Ok(Value::Null);
        }
        let result = dispatch(&mut activation, &mut context, &method, args);
        if method == "activate" && result.is_ok() {
            activation.initialized = true;
            if let Some(interval) = app
                .state::<PluginState>()
                .manifest(&plugin_id)?
                .poll_interval_ms
            {
                start_polling(app.clone(), plugin_id.clone(), generation, interval);
            }
        }
        context.active()?;
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
/// Called only by Android intake with sanitized metadata and its private staged copy.
#[cfg(target_os = "android")]
pub async fn dispatch_share(app: &tauri::AppHandle, args: Value, staged: Option<&std::path::Path>) -> bool {
    let file = match staged {
        Some(path) => match std::fs::File::open(path) {
            Ok(f) if f.metadata().is_ok_and(|m| m.is_file()) => Some(Arc::new(Mutex::new(f))),
            _ => return false,
        },
        None => None,
    };
    let Ok(mut plugins) = app.state::<PluginState>().list() else { return false };
    plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    plugins.retain(|plugin| plugin.enabled && plugin.supported
        && plugin.manifest.contributions.iter().any(|c| c.kind == "share-handler"));
    let mut matching = Vec::new();
    for plugin in plugins {
        if native_call(app.clone(), plugin.manifest.id.clone(), plugin.generation,
            "canShare".into(), args.clone(), None).await.is_ok_and(|reply| reply == Value::Bool(true)) {
            matching.push(plugin);
        }
    }
    let mut plugins = matching;
    let explicit = plugins.len() > 1;
    if explicit {
        let mut choices: Vec<_> = plugins.iter().map(|p| format!("{} ({})", p.manifest.name, p.manifest.id)).collect();
        if staged.is_some() { choices.push("Import into library".into()); }
        match crate::share_inbox::choose_handler(app, choices).await {
            Some(index) if index < plugins.len() => {
                let selected = plugins.swap_remove(index);
                plugins.clear();
                plugins.push(selected);
            }
            Some(index) if index == plugins.len() && staged.is_some() => return false,
            _ => return true, // Cancel: do not dispatch or import.
        }
    }
    for plugin in plugins {
        let id = plugin.manifest.id;
        match native_call(app.clone(), id.clone(), plugin.generation, "share".into(), args.clone(), file.clone()).await {
            Ok(reply) if reply["handled"].as_bool() == Some(true) => {
                if staged.is_none() && args["url"].is_string() {
                    if let Some(query) = reply["searchQuery"].as_str() {
                        if let Some(provider) = plugin.manifest.contributions.iter().find(|c| c.kind == "search") {
                            let _ = app.emit("plugin-shared-search", json!({"pluginId": id, "providerId": provider.id, "query": query}));
                        }
                    }
                }
                return true;
            }
            Err(error) => {
                tracing::warn!(%id, %error, "Android share handler failed");
                let _ = app.emit("plugin-runtime-error", json!({"pluginId":id,"generation":plugin.generation,"error":error}));
            }
            _ => {}
        }
    }
    explicit // Never silently import after the user explicitly chose a plugin.
}

pub fn stop(app: &tauri::AppHandle, id: &str, generation: u64) {
    if let Ok(mut pending) = replies().lock() {
        pending.retain(|_, reply| {
            if reply.id == id {
                let _ = reply.send.send(Err("CANCELLED: plugin disabled".into()));
                false
            } else {
                true
            }
        });
    }

    let runtime = app.state::<NativeRuntime>();
    let activation = runtime
        .activations
        .lock()
        .ok()
        .and_then(|mut a| a.remove(id));
    if let Some(activation) = activation {
        if let Ok(mut activation) = activation.lock() {
            let mut context = Context {
                app: app.clone(),
                id: id.into(),
                generation,
                share_file: None,
                directory: match &activation.backend {
                    Backend::Library(library) => Some(library.directory.clone()),
                    Backend::Android => None,
                },
            };
            if !matches!(activation.backend, Backend::Android)
                || !runtime.closing.load(Ordering::Acquire)
            {
                let _ = dispatch(&mut activation, &mut context, "deactivate", json!({}));
            }
        }
    }
}
pub fn shutdown(app: &tauri::AppHandle) {
    let runtime = app.state::<NativeRuntime>();
    runtime.closing.store(true, Ordering::Release);
    let ids: Vec<_> = runtime
        .activations
        .lock()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    for id in ids {
        let generation = app
            .state::<PluginState>()
            .record(&id)
            .map(|r| r.generation)
            .unwrap_or(0);
        stop(app, &id, generation);
    }
}

#[tauri::command]
pub fn plugin_native_reply(
    request_id: u64,
    plugin_id: String,
    generation: u64,
    result: Option<Value>,
    error: Option<String>,
) -> Result<(), String> {
    let mut pending = replies().lock().map_err(|e| e.to_string())?;
    let Some(reply) = pending.get(&request_id) else {
        return Ok(());
    };
    if reply.id != plugin_id || reply.generation != generation {
        return Err("Invalid native callback owner".into());
    }
    let reply = pending.remove(&request_id).unwrap();
    let _ = reply.send.send(match error {
        Some(e) => Err(e),
        None => Ok(result.unwrap_or(Value::Null)),
    });
    Ok(())
}

#[cfg(test)]
mod runtime_file_tests {
    use super::*;
    use super::super::model::{NativeArtifact, RuntimeFile};

    fn artifact() -> NativeArtifact {
        let hash = "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb";
        NativeArtifact {
            data: "YQ==".into(), sha256: hash.into(),
            files: [("yt-dlp".into(), RuntimeFile { data: "YQ==".into(), sha256: hash.into(), executable: true }),
                ("config.json".into(), RuntimeFile { data: "YQ==".into(), sha256: hash.into(), executable: false })].into(),
        }
    }
    fn root() -> std::path::PathBuf {
        std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("player-runtime-files-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
    #[test]
    fn extracts_and_repairs_tampering_with_private_permissions() {
        let root = root();
        let artifact = artifact();
        extract_artifact(&root, &artifact).unwrap();
        assert_eq!(std::fs::read(root.join("yt-dlp")).unwrap(), b"a");
        assert_eq!(std::fs::read(root.join(format!("plugin.{}", std::env::consts::DLL_EXTENSION))).unwrap(), b"a");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(root.join("yt-dlp")).unwrap().permissions().mode() & 0o777, 0o500);
            assert_eq!(std::fs::metadata(root.join("config.json")).unwrap().permissions().mode() & 0o777, 0o400);
        }
        std::fs::remove_file(root.join("yt-dlp")).unwrap();
        std::fs::write(root.join("yt-dlp"), b"tampered").unwrap();
        extract_artifact(&root, &artifact).unwrap();
        assert_eq!(std::fs::read(root.join("yt-dlp")).unwrap(), b"a");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn replaces_file_symlinks_without_touching_target_and_rejects_directory_symlinks() {
        use std::os::unix::fs::symlink;
        let root = root();
        let outside = root.with_extension("outside");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::create_dir(&root).unwrap();
        symlink(&outside, root.join("yt-dlp")).unwrap();
        extract_artifact(&root, &artifact()).unwrap();
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert!(!std::fs::symlink_metadata(root.join("yt-dlp")).unwrap().file_type().is_symlink());
        let alias = root.with_extension("alias");
        symlink(&root, &alias).unwrap();
        assert!(extract_artifact(&alias, &artifact()).is_err());
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_dev_verncat_player_NativeHostBridge_call(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    token: jni::sys::jlong,
    method: jni::objects::JString,
    args: jni::objects::JString,
) -> jni::sys::jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> PluginResult {
        let method: String = env.get_string(&method).map_err(|e| e.to_string())?.into();
        let args: String = env.get_string(&args).map_err(|e| e.to_string())?.into();
        if args.len() > player_plugin_sdk::MAX_MESSAGE_BYTES {
            return Err("Host request exceeds 1 MiB".into());
        }
        let context = ANDROID_CONTEXTS
            .get_or_init(Default::default)
            .lock()
            .map_err(|e| e.to_string())?
            .get(&(token as u64))
            .cloned()
            .ok_or("Expired native context")?;
        context.call(
            &method,
            serde_json::from_str(&args).map_err(|e| e.to_string())?,
        )
    }))
    .unwrap_or_else(|_| Err("Android host callback panicked".into()));
    let response = String::from_utf8(player_plugin_sdk::encode(result)).unwrap_or_default();
    env.new_string(response)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiled_plugin_runs_through_c_abi_and_calls_host_without_an_interpreter() {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/Cargo.toml");
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/target");
        let output = std::process::Command::new("cargo")
            .args(["build", "--locked", "--offline", "--manifest-path"])
            .arg(&manifest)
            .args(["-p", "player-hello-plugin"])
            .env("CARGO_TARGET_DIR", &target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = target.join("debug").join(format!(
            "{}player_hello_plugin.{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_EXTENSION
        ));
        let packaged = std::env::temp_dir().join(format!(
            "player-native-abi-{}.player-plugin",
            std::process::id()
        ));
        let package_source = manifest.parent().unwrap().join("examples/hello");
        let output = std::process::Command::new("cargo")
            .args(["run", "--locked", "--offline", "--manifest-path"])
            .arg(&manifest)
            .args([
                "-p",
                "player-plugin-sdk",
                "--bin",
                "player-plugin-pack",
                "--",
            ])
            .arg(package_source)
            .arg(&packaged)
            .arg(format!("{}={}", native_target(), path.display()))
            .env("CARGO_TARGET_DIR", &target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let package = Package::parse(&std::fs::read_to_string(&packaged).unwrap()).unwrap();
        assert_eq!(package.manifest.api_version, 2);
        assert_eq!(package.manifest.id, "example.hello");
        std::fs::remove_file(packaged).unwrap();
        unsafe extern "C" fn callback(
            context: *mut c_void,
            request: *const u8,
            len: usize,
        ) -> Buffer {
            let state = &mut *context.cast::<Value>();
            let request: Value =
                serde_json::from_slice(std::slice::from_raw_parts(request, len)).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "settings.get" => Ok(state.clone()),
                "settings.update" => {
                    *state = json!({"revision":1,"values":request["args"]["values"]});
                    Ok(state.clone())
                }
                "host.invalidate" => Ok(Value::Null),
                _ => Err("Unexpected host method".into()),
            };
            Buffer::new(player_plugin_sdk::encode(result))
        }
        unsafe {
            let library = libloading::Library::new(path).unwrap();
            let version = library
                .get::<unsafe extern "C" fn() -> u32>(b"player_plugin_abi_version\0")
                .unwrap();
            assert_eq!(version(), 1);
            let identity = library
                .get::<unsafe extern "C" fn() -> *const std::ffi::c_char>(b"player_plugin_id\0")
                .unwrap();
            assert_eq!(
                CStr::from_ptr(identity()).to_str().unwrap(),
                "example.hello"
            );
            let create = library.get::<Create>(b"player_plugin_create\0").unwrap();
            let call = library.get::<Call>(b"player_plugin_call\0").unwrap();
            let free = library.get::<Free>(b"player_plugin_free\0").unwrap();
            let destroy = library.get::<Destroy>(b"player_plugin_destroy\0").unwrap();
            let instance = create();
            assert!(!instance.is_null());
            let mut settings = json!({"revision":0,"values":{}});
            let mut invoke = |method: &str, args: Value| {
                let request = json!({"method":method,"args":args}).to_string();
                let buffer = call(
                    instance,
                    request.as_ptr(),
                    request.len(),
                    HostApi {
                        context: (&mut settings as *mut Value).cast(),
                        call: callback,
                        free: Buffer::free,
                    },
                );
                let value = player_plugin_sdk::decode(buffer.read().unwrap());
                free(buffer);
                value
            };
            invoke("activate", json!({})).unwrap();
            invoke(
                "action",
                json!({"action":"save","fields":{"greeting":"Native ABI works"}}),
            )
            .unwrap();
            assert_eq!(
                invoke("render", json!({"viewId":"home"})).unwrap()["title"],
                "Native ABI works"
            );
            let mut scope = json!({"attachmentId":"album-greeting","viewId":"album-greeting",
                "target":"AlbumCard","instanceId":"native-card-1","context":{"album":{"album":"Native album"}}});
            validate_attachment_scope(&package.manifest, "renderAttachment", &scope).unwrap();
            assert_eq!(
                invoke("renderAttachment", scope.clone()).unwrap()["children"][0]["text"],
                "Hello, Native album!"
            );
            scope["context"] = json!({});
            assert_eq!(invoke("renderAttachment", scope).unwrap(), Value::Null);
            assert!(invoke("unknown", json!({})).is_err());
            invoke("deactivate", json!({})).unwrap();
            destroy(instance);
        }
    }
    #[test]
    fn callback_reply_is_bound_to_its_owner_and_generation() {
        let (send, receive) = std::sync::mpsc::channel();
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        replies().lock().unwrap().insert(
            id,
            Reply {
                id: "example.a".into(),
                generation: 5,
                send,
            },
        );
        assert!(plugin_native_reply(id, "example.b".into(), 5, Some(json!(1)), None).is_err());
        assert!(plugin_native_reply(id, "example.a".into(), 4, Some(json!(1)), None).is_err());
        plugin_native_reply(id, "example.a".into(), 5, Some(json!(1)), None).unwrap();
        assert_eq!(receive.recv().unwrap().unwrap(), json!(1));
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    #[test]
    fn attachment_renders_and_actions_require_declared_scope() {
        let mut manifest = super::super::model::fixture_manifest();
        manifest.permissions.push("ui".into());
        manifest.attachments = vec![serde_json::from_value(json!({
            "id":"extra", "view":"details", "target":"AlbumCard", "position":"after"
        }))
        .unwrap()];
        let args = json!({"attachmentId":"extra", "viewId":"details", "target":"AlbumCard",
            "instanceId":"host-instance-1", "context":{"album":{"album":"Example"}}});
        for method in ["renderAttachment", "action"] {
            validate_attachment_scope(&manifest, method, &args).unwrap();
            for (key, value) in [
                ("attachmentId", json!("unknown")),
                ("viewId", json!("other")),
                ("target", json!("OtherCard")),
                ("instanceId", json!("")),
                ("context", Value::Null),
            ] {
                let mut invalid = args.clone();
                invalid[key] = value;
                assert!(validate_attachment_scope(&manifest, method, &invalid).is_err());
            }
        }
        manifest.permissions.retain(|permission| permission != "ui");
        assert!(validate_attachment_scope(&manifest, "renderAttachment", &args).is_err());
        validate_attachment_scope(&manifest, "render", &json!({"viewId":"legacy"})).unwrap();
        validate_attachment_scope(&manifest, "action", &json!({"viewId":"legacy"})).unwrap();
    }
}
