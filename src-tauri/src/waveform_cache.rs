//! Durable, background-only waveform jobs. Failed identities are cached too.
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::{Arc, Mutex}, time::Duration};
use tauri::Emitter;
use std::sync::atomic::{AtomicI64, Ordering};
static SELECTED_TRACK: AtomicI64 = AtomicI64::new(0);
type Error = Box<dyn std::error::Error + Send + Sync>;
const VERSION: i64 = 2; // RMS envelope; invalidate legacy maximum-peak bins.

pub(crate) fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS track_waveforms (
        track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
        version INTEGER NOT NULL, identity TEXT NOT NULL, samples BLOB);
        CREATE TABLE IF NOT EXISTS waveform_jobs (
        track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE);
        CREATE TRIGGER IF NOT EXISTS waveform_track_insert AFTER INSERT ON tracks BEGIN
          INSERT OR IGNORE INTO waveform_jobs VALUES (NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS waveform_track_update AFTER UPDATE OF file_hash, modified_secs,
          source_path, segment_start_secs, segment_end_secs ON tracks
        WHEN OLD.file_hash IS NOT NEW.file_hash OR OLD.modified_secs IS NOT NEW.modified_secs
          OR OLD.source_path IS NOT NEW.source_path OR OLD.segment_start_secs IS NOT NEW.segment_start_secs
          OR OLD.segment_end_secs IS NOT NEW.segment_end_secs BEGIN
          INSERT OR IGNORE INTO waveform_jobs VALUES (NEW.id); END;
        DELETE FROM waveform_jobs WHERE track_id NOT IN (SELECT id FROM tracks);
        DELETE FROM track_waveforms WHERE track_id NOT IN (SELECT id FROM tracks);")?;
    conn.execute("INSERT OR IGNORE INTO waveform_jobs SELECT t.id FROM tracks t
        LEFT JOIN track_waveforms w ON w.track_id=t.id WHERE w.track_id IS NULL OR w.version != ?1", [VERSION])?;
    Ok(())
}

#[derive(Clone)]
struct SourceTrack { id: i64, source: String, hash: Option<String>, modified: i64, start: f64, end: Option<f64> }
impl SourceTrack {
    fn identity(&self, root: &Path) -> String {
        let stat = std::fs::metadata(root.join(&self.source)).ok();
        let size = stat.as_ref().map(|s| s.len());
        let nanos = stat.and_then(|s| s.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos());
        format!("{:?}|{}|{:?}|{:?}|{:?}|{}|{:?}", self.hash, self.modified, size, nanos, self.source, self.start, self.end)
    }
}

/// Reads stored data only; this never opens or decodes an audio file.
pub(crate) fn read(conn: &Connection, id: i64) -> rusqlite::Result<Option<Vec<u8>>> {
    SELECTED_TRACK.store(id, Ordering::Relaxed);
    conn.query_row("SELECT w.samples FROM track_waveforms w
        WHERE w.track_id=?1 AND w.version=?2 AND NOT EXISTS
        (SELECT 1 FROM waveform_jobs j WHERE j.track_id=w.track_id)", params![id, VERSION], |r| r.get(0)).optional().map(Option::flatten)
}

/// Detects sub-second/size changes even when the legacy indexer skips same-second files.
pub(crate) fn source_changed(conn: &Connection, root: &Path, source: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("SELECT id, COALESCE(NULLIF(source_path,''),path), file_hash, COALESCE(modified_secs,0), COALESCE(segment_start_secs,0), segment_end_secs FROM tracks WHERE COALESCE(NULLIF(source_path,''),path)=?1")?;
    let rows = stmt.query_map([source], |r| Ok(SourceTrack { id:r.get(0)?, source:r.get(1)?, hash:r.get(2)?, modified:r.get(3)?, start:r.get(4)?, end:r.get(5)? }))?;
    for row in rows {
        let track = row?;
        let identity = track.identity(root);
        conn.execute("INSERT OR IGNORE INTO waveform_jobs SELECT ?1 WHERE NOT EXISTS (SELECT 1 FROM track_waveforms WHERE track_id=?1 AND version=?2 AND identity=?3)", params![track.id,VERSION,identity])?;
    }
    Ok(())
}

/// Processes one physical source without holding the SQLite lock during decoding.
pub(crate) fn process_one(conn: &Arc<Mutex<Connection>>, root: &Path) -> Result<bool, Error> {
    process_one_selected(conn, root, SELECTED_TRACK.load(Ordering::Relaxed))
}

fn process_one_selected(conn: &Arc<Mutex<Connection>>, root: &Path, selected: i64) -> Result<bool, Error> {
    let tracks = {
        let db = conn.lock().unwrap();
        let source: Option<String> = db.query_row("SELECT COALESCE(NULLIF(t.source_path,''), t.path)
            FROM waveform_jobs j JOIN tracks t ON t.id=j.track_id
            ORDER BY (j.track_id=?1) DESC, j.track_id LIMIT 1", [selected], |r| r.get(0)).optional()?;
        let Some(source) = source else { return Ok(false) };
        let mut stmt = db.prepare("SELECT t.id, COALESCE(NULLIF(t.source_path,''),t.path), t.file_hash,
            COALESCE(t.modified_secs,0), COALESCE(t.segment_start_secs,0), t.segment_end_secs
            FROM tracks t WHERE COALESCE(NULLIF(t.source_path,''),t.path)=?1")?;
        let rows = stmt.query_map([source], |r| Ok(SourceTrack { id:r.get(0)?, source:r.get(1)?, hash:r.get(2)?, modified:r.get(3)?, start:r.get(4)?, end:r.get(5)? }))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut pending = Vec::new();
    for track in tracks {
        let identity = track.identity(root);
        let db = conn.lock().unwrap();
        let cached: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM track_waveforms WHERE track_id=?1 AND version=?2 AND identity=?3)", params![track.id, VERSION, identity], |r| r.get(0))?;
        if cached { db.execute("DELETE FROM waveform_jobs WHERE track_id=?1", [track.id])?; }
        else { pending.push((track, identity)); }
    }
    if pending.is_empty() { return Ok(true); }
    let segments: Vec<_> = pending.iter().map(|(t,_)| (t.start,t.end)).collect();
    let decoded = crate::waveform::decode(&root.join(&pending[0].0.source), &segments);
    if let Err(error) = &decoded { eprintln!("[library] waveform unavailable for {}: {error}", pending[0].0.source); }
    let db = conn.lock().unwrap();
    let tx = db.unchecked_transaction()?;
    for (index, (track, identity)) in pending.iter().enumerate() {
        // An edit while decoding must leave its job pending, never publish stale data.
        let current: Option<(Option<String>,i64,f64,Option<f64>,String)> = tx.query_row("SELECT file_hash, COALESCE(modified_secs,0), COALESCE(segment_start_secs,0), segment_end_secs, COALESCE(NULLIF(source_path,''),path) FROM tracks WHERE id=?1", [track.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if current != Some((track.hash.clone(),track.modified,track.start,track.end,track.source.clone())) || track.identity(root) != *identity { continue; }
        let samples = decoded.as_ref().ok().and_then(|v| v.get(index));
        tx.execute("INSERT INTO track_waveforms(track_id,version,identity,samples) VALUES (?1,?2,?3,?4)
            ON CONFLICT(track_id) DO UPDATE SET version=excluded.version,identity=excluded.identity,samples=excluded.samples", params![track.id,VERSION,identity,samples])?;
        tx.execute("DELETE FROM waveform_jobs WHERE track_id=?1", [track.id])?;
    }
    tx.commit()?;
    Ok(true)
}

pub(crate) fn start(conn: Arc<Mutex<Connection>>, root: std::path::PathBuf, app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // Stat sources off the startup thread to detect edits made while closed.
        let sources = {
            let db = conn.lock().unwrap();
            let mut stmt = db.prepare("SELECT DISTINCT COALESCE(NULLIF(source_path,''),path) FROM tracks").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_,String>(0)).unwrap();
            rows.filter_map(Result::ok).collect::<Vec<_>>()
        };
        for source in sources { let _ = source_changed(&conn.lock().unwrap(), &root, &source); }
        loop {
            match process_one(&conn, &root) {
                Ok(true) => { let _ = app.emit("waveform-ready", ()); }
                Ok(false) => std::thread::sleep(Duration::from_secs(2)),
                Err(error) => { eprintln!("[library] waveform cache: {error}"); std::thread::sleep(Duration::from_secs(10)); }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav(amplitude: i16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(b"RIFF"); data.extend( (36u32 + 2048).to_le_bytes()); data.extend(b"WAVEfmt ");
        data.extend(16u32.to_le_bytes()); data.extend(1u16.to_le_bytes()); data.extend(1u16.to_le_bytes());
        data.extend(1024u32.to_le_bytes()); data.extend(2048u32.to_le_bytes()); data.extend(2u16.to_le_bytes()); data.extend(16u16.to_le_bytes());
        data.extend(b"data"); data.extend(2048u32.to_le_bytes());
        for _ in 0..1024 { data.extend(amplitude.to_le_bytes()); }
        data
    }
    #[test]
    fn selected_track_precedes_backfill_without_decoding_on_read() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE tracks(id INTEGER PRIMARY KEY,path TEXT,source_path TEXT,file_hash TEXT,modified_secs INTEGER,segment_start_secs REAL,segment_end_secs REAL);").unwrap();
        init(&db).unwrap();
        db.execute_batch("INSERT INTO tracks(id,path) VALUES (1,'missing-first.wav'),(2,'missing-selected.wav');").unwrap();
        let conn = Arc::new(Mutex::new(db));
        assert!(read(&conn.lock().unwrap(), 2).unwrap().is_none());
        assert_eq!(conn.lock().unwrap().query_row("SELECT count(*) FROM track_waveforms", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(process_one_selected(&conn, Path::new("/nonexistent-waveform-test"), 2).unwrap());
        let db = conn.lock().unwrap();
        assert_eq!(db.query_row("SELECT track_id FROM track_waveforms", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert_eq!(db.query_row("SELECT track_id FROM waveform_jobs", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }
    #[test]
    fn waveform_cache_real_wav_reuse_invalidation_and_failed_resume() {
        let root = std::env::temp_dir().join(format!("player-waveform-cache-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("test.wav"), wav(16384)).unwrap();
        let dbpath = root.join("cache.db");
        let db = Connection::open(&dbpath).unwrap();
        db.execute_batch("CREATE TABLE tracks(id INTEGER PRIMARY KEY,path TEXT,source_path TEXT,file_hash TEXT,modified_secs INTEGER,segment_start_secs REAL,segment_end_secs REAL);").unwrap();
        init(&db).unwrap();
        db.execute("INSERT INTO tracks(id,path,file_hash,modified_secs) VALUES (1,'test.wav','first',1)", []).unwrap();
        let conn = Arc::new(Mutex::new(db));
        assert!(process_one(&conn,&root).unwrap());
        let first = read(&conn.lock().unwrap(),1).unwrap().unwrap();
        assert_eq!(first.len(),256); assert!(first.iter().all(|&v| (127..=129).contains(&v)));
        assert!(!process_one(&conn,&root).unwrap());
        drop(conn);
        let db = Connection::open(&dbpath).unwrap(); init(&db).unwrap();
        let conn = Arc::new(Mutex::new(db));
        assert!(!process_one(&conn,&root).unwrap());
        assert_eq!(read(&conn.lock().unwrap(),1).unwrap(),Some(first));
        std::fs::write(root.join("test.wav"),wav(0)).unwrap();
        conn.lock().unwrap().execute("UPDATE tracks SET file_hash='second',modified_secs=2 WHERE id=1",[]).unwrap();
        assert!(read(&conn.lock().unwrap(),1).unwrap().is_none());
        assert!(process_one(&conn,&root).unwrap());
        assert!(read(&conn.lock().unwrap(),1).unwrap().unwrap().iter().all(|&v|v==0));
        let mut changed = wav(8192); changed.extend([0; 16]);
        std::fs::write(root.join("test.wav"),changed).unwrap();
        source_changed(&conn.lock().unwrap(),&root,"test.wav").unwrap();
        assert!(read(&conn.lock().unwrap(),1).unwrap().is_none());
        assert!(process_one(&conn,&root).unwrap());
        assert!(read(&conn.lock().unwrap(),1).unwrap().unwrap().iter().all(|&v|v==64));
        conn.lock().unwrap().execute("INSERT INTO tracks(id,path,file_hash) VALUES (2,'missing.wav','missing')",[]).unwrap();
        assert!(process_one(&conn,&root).unwrap());
        init(&conn.lock().unwrap()).unwrap();
        assert!(!process_one(&conn,&root).unwrap());
        assert!(read(&conn.lock().unwrap(),2).unwrap().is_none());
        drop(conn); std::fs::remove_dir_all(root).unwrap();
    }
}
