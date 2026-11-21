#[cfg(feature = "native-video")]
pub(crate) mod native;

#[tauri::command]
pub fn native_video_enabled() -> bool {
    cfg!(feature = "native-video")
}

use std::io::{Read, Seek, SeekFrom};
use tauri::{http::{Request, Response, StatusCode}, Manager, Runtime, UriSchemeContext, UriSchemeResponder};

pub fn handle<R: Runtime>(ctx: UriSchemeContext<'_, R>, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let response = app.try_state::<crate::playback::PlaybackState>()
            .and_then(|state| state.video_path(request.uri().path().trim_start_matches('/')))
            .map(|path| serve(&path, &request))
            .unwrap_or_else(|| empty(StatusCode::NOT_FOUND));
        responder.respond(response);
    });
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(status).body(Vec::new()).unwrap()
}

pub(crate) fn supported(path: &std::path::Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        ["mp4", "m4v", "webm"].iter().any(|candidate| ext.eq_ignore_ascii_case(candidate))
    })
}

fn serve(path: &std::path::Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let mime = match path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        _ => return empty(StatusCode::NOT_FOUND),
    };
    let Ok(mut file) = std::fs::File::open(path) else { return empty(StatusCode::NOT_FOUND) };
    let Ok(info) = file.metadata() else { return empty(StatusCode::NOT_FOUND) };
    if !info.is_file() { return empty(StatusCode::NOT_FOUND); }
    let size = info.len();
    let mut builder = Response::builder().header("Content-Type", mime).header("Accept-Ranges", "bytes").header("Cache-Control", "no-store");
    if request.method() == tauri::http::Method::HEAD {
        return builder.header("Content-Length", size).body(Vec::new()).unwrap();
    }
    let range = request.headers().get("Range");
    let bounds = match range {
        None => Some((0, size.saturating_sub(1))),
        Some(value) => value.to_str().ok().and_then(|s| bounds(s, size)),
    };
    let Some((start, end)) = bounds else {
        return builder.status(416).header("Content-Range", format!("bytes */{size}")).body(Vec::new()).unwrap();
    };
    // A 2 MiB response covers less than one second of high-bitrate local
    // video. Allow more read-ahead without allocating the entire file.
    const MAX_RANGE_BYTES: u64 = 8 * 1024 * 1024;
    let end = if range.is_some() { end.min(start.saturating_add(MAX_RANGE_BYTES - 1)) } else { end };
    let count = if size == 0 { 0 } else { end - start + 1 };
    if count > 64 * 1024 * 1024 { return empty(StatusCode::PAYLOAD_TOO_LARGE); }
    if range.is_some() {
        builder = builder.status(206).header("Content-Range", format!("bytes {start}-{end}/{size}"));
    }
    let mut data = Vec::with_capacity(count as usize);
    if file.seek(SeekFrom::Start(start)).is_err() || file.take(count).read_to_end(&mut data).is_err() || data.len() as u64 != count {
        return empty(StatusCode::SERVICE_UNAVAILABLE);
    }
    builder.header("Content-Length", count).body(data).unwrap()
}

fn bounds(range: &str, size: u64) -> Option<(u64, u64)> {
    let last = size.checked_sub(1)?;
    let (start, end) = range.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return (suffix > 0).then(|| (size.saturating_sub(suffix), last));
    }
    let start = start.parse().ok()?;
    let end = if end.is_empty() { last } else { last.min(end.parse().ok()?) };
    (start <= end).then_some((start, end))
}
