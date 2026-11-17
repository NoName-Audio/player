//! Packages remain native-side between review and permission approval.
use super::model::{InstallSource, Manifest, Package, MAX_PACKAGE_BYTES};
use serde::Serialize;
use std::sync::{atomic::{AtomicU64, Ordering}, Mutex};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static STAGED: Mutex<Option<(String, StagedPackage)>> = Mutex::new(None);

pub struct StagedPackage {
    pub package: Package,
    pub fingerprint: String,
    pub source: Option<InstallSource>,
}

#[derive(Serialize)]
pub struct Preview {
    pub manifest: Manifest,
    pub fingerprint: String,
    pub source: Option<InstallSource>,
    pub token: String,
}

pub fn stage(package: Package, source: Option<InstallSource>) -> Result<Preview, String> {
    let fingerprint = package.fingerprint();
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed).to_string();
    let preview = Preview { manifest: package.manifest.clone(), fingerprint: fingerprint.clone(), source: source.clone(), token: token.clone() };
    let previous = STAGED.lock().map_err(|e| e.to_string())?.replace((token, StagedPackage { package, fingerprint, source }));
    drop(previous);
    Ok(preview)
}

pub fn read_path(path: String) -> Result<Preview, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_PACKAGE_BYTES as u64 {
        return Err("Plugin package exceeds 768 MiB".into());
    }
    let mut content = String::new();
    file.take(MAX_PACKAGE_BYTES as u64 + 1).read_to_string(&mut content).map_err(|e| e.to_string())?;
    let package = Package::parse(&content)?;
    drop(content);
    stage(package, None)
}

pub fn take(token: &str, fingerprint: &str) -> Result<StagedPackage, String> {
    let mut staged = STAGED.lock().map_err(|e| e.to_string())?;
    let Some((stored_token, package)) = staged.as_ref() else { return Err("Plugin preview expired; select the package again".into()); };
    if stored_token != token || package.fingerprint != fingerprint {
        return Err("Plugin preview does not match the approved package".into());
    }
    Ok(staged.take().expect("checked staged package").1)
}

pub fn discard(token: &str) -> Result<(), String> {
    let mut staged = STAGED.lock().map_err(|e| e.to_string())?;
    let removed = if staged.as_ref().is_some_and(|(stored, _)| stored == token) { staged.take() } else { None };
    drop(staged);
    drop(removed);
    Ok(())
}
