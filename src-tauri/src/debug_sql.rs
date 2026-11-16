//! Trusted local SQL console; deliberately not exposed through plugin permissions.
use crate::library::LibraryState;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{ffi, hooks::{AuthAction, AuthContext, Authorization}, Connection};
use serde::Serialize;
use std::{cell::Cell, ffi::{CStr, CString}, ptr, sync::{Arc, Mutex, TryLockError, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tauri::Emitter;

const TIME_LIMIT: Duration = Duration::from_secs(5);
const MAX_ROWS: usize = 1000;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SQL: usize = 256 * 1024;
thread_local! { static BUSY_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) }; }

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob { r#type: &'static str, base64: String },
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlResult {
    columns: Vec<String>,
    rows: Vec<Vec<SqlValue>>,
    affected_rows: u64,
    readonly: bool,
    truncated: bool,
    elapsed_ms: f64,
}

#[tauri::command]
pub async fn debug_execute_sql(sql: String, allow_write: bool, state: tauri::State<'_, LibraryState>, app: tauri::AppHandle) -> Result<SqlResult, String> {
    let conn = state.debug_connection();
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || execute_locked(conn, &sql, allow_write, started)).await.map_err(|e| e.to_string())??;
    if !result.readonly { let _ = app.emit("library-changed", ()); }
    Ok(result)
}

fn execute_locked(conn: Arc<Mutex<Connection>>, sql: &str, allow_write: bool, started: Instant) -> Result<SqlResult, String> {
    let deadline = started + TIME_LIMIT;
    loop {
        if Instant::now() >= deadline { return Err("SQL time limit exceeded while waiting for database".into()); }
        match conn.try_lock() {
            Ok(conn) => return execute(&conn, sql, allow_write, started, deadline),
            Err(TryLockError::Poisoned(_)) => return Err("Library database lock is poisoned".into()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn busy_wait(_: i32) -> bool {
    let keep_waiting = BUSY_DEADLINE.with(|d| d.get().is_some_and(|deadline| Instant::now() < deadline));
    if keep_waiting { std::thread::sleep(Duration::from_millis(2)); }
    keep_waiting
}

// All connection-local policy is restored before releasing the shared mutex.
struct Policy<'a> { conn: &'a Connection, old_length: i32 }
impl Drop for Policy<'_> {
    fn drop(&mut self) {
        let _ = self.conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
        if !self.conn.is_autocommit() { let _ = self.conn.execute_batch("ROLLBACK"); }
        unsafe { ffi::sqlite3_limit(self.conn.handle(), ffi::SQLITE_LIMIT_LENGTH, self.old_length); }
        let _ = self.conn.busy_timeout(Duration::from_secs(5));
        BUSY_DEADLINE.with(|d| d.set(None));
    }
}
struct Statement(*mut ffi::sqlite3_stmt);
impl Drop for Statement { fn drop(&mut self) { unsafe { ffi::sqlite3_finalize(self.0); } } }
fn db_error(conn: &Connection) -> String {
    unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(conn.handle())).to_string_lossy().into_owned() }
}

fn execute(conn: &Connection, sql: &str, allow_write: bool, started: Instant, deadline: Instant) -> Result<SqlResult, String> {
    if sql.len() > MAX_SQL { return Err("SQL exceeds 256 KiB limit".into()); }
    let sql = CString::new(sql).map_err(|_| "SQL contains a NUL byte")?;
    if !conn.is_autocommit() { return Err("Library database already has an active transaction".into()); }
    let old_length = unsafe { ffi::sqlite3_limit(conn.handle(), ffi::SQLITE_LIMIT_LENGTH, MAX_BYTES as i32) };
    let _policy = Policy { conn, old_length };
    BUSY_DEADLINE.with(|d| d.set(Some(deadline)));
    conn.busy_handler(Some(busy_wait)).map_err(|e| e.to_string())?;
    conn.progress_handler(1000, Some(move || Instant::now() >= deadline)).map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN DEFERRED").map_err(|e| e.to_string())?;
    let dml = Arc::new(AtomicBool::new(false));
    let saw_dml = Arc::clone(&dml);
    conn.authorizer(Some(move |ctx: AuthContext<'_>| {
        use AuthAction::*;
        if ctx.database_name.is_some_and(|name| name != "main" && name != "temp") { return Authorization::Deny; }
        match ctx.action {
            Attach { .. } | Detach { .. } | Transaction { .. } | Savepoint { .. } | CreateVtable { .. } | DropVtable { .. } | Unknown { .. } => Authorization::Deny,
            // Only introspection pragmas: setters and connection/file policy are not SQL-console operations.
            Pragma { pragma_name, .. } if !["table_info", "table_xinfo", "index_info", "index_xinfo", "index_list", "foreign_key_list", "database_list", "compile_options"].iter().any(|p| pragma_name.eq_ignore_ascii_case(p)) => Authorization::Deny,
            Function { function_name } if ["load_extension", "readfile", "writefile", "fts3_tokenizer"].iter().any(|f| function_name.eq_ignore_ascii_case(f)) => Authorization::Deny,
            Insert { table_name } | Delete { table_name } | Update { table_name, .. } => {
                if ctx.accessor.is_none() && !table_name.starts_with("sqlite_") { saw_dml.store(true, Ordering::Relaxed); }
                Authorization::Allow
            }
            _ => Authorization::Allow,
        }
    })).map_err(|e| e.to_string())?;
    unsafe {
        let db = conn.handle();
        let mut raw = ptr::null_mut();
        let mut tail = ptr::null();
        let rc = ffi::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut raw, &mut tail);
        let statement = Statement(raw);
        if rc != ffi::SQLITE_OK { return Err(db_error(conn)); }
        if raw.is_null() { return Err("Enter exactly one SQL statement".into()); }
        // Ask SQLite to parse the remainder; comments and semicolons inside literals remain valid.
        let mut extra = ptr::null_mut();
        let rc = ffi::sqlite3_prepare_v2(db, tail, -1, &mut extra, ptr::null_mut());
        let _extra = Statement(extra);
        if rc != ffi::SQLITE_OK || !extra.is_null() { return Err("Exactly one SQL statement is allowed".into()); }
        let readonly = ffi::sqlite3_stmt_readonly(raw) != 0;
        if !readonly && !allow_write { return Err("WRITE_CONFIRMATION_REQUIRED: Confirm this SQL mutation before executing it".into()); }
        let count = ffi::sqlite3_column_count(raw) as usize;
        let mut bytes = 0usize;
        let mut columns = Vec::with_capacity(count);
        for i in 0..count {
            let name = CStr::from_ptr(ffi::sqlite3_column_name(raw, i as i32)).to_string_lossy();
            bytes += name.len() * 6 + 8;
            if bytes > MAX_BYTES { return Err("SQL column names exceed result size limit".into()); }
            columns.push(name.into_owned());
        }
        let before = ffi::sqlite3_total_changes64(db);
        let mut rows = Vec::new();
        let mut truncated = false;
        loop {
            if Instant::now() >= deadline { return Err("SQL time limit exceeded".into()); }
            match ffi::sqlite3_step(raw) {
                ffi::SQLITE_DONE => break,
                ffi::SQLITE_ROW => {},
                _ => return Err(db_error(conn)),
            }
            if rows.len() == MAX_ROWS { truncated = true; if readonly { break; } else { continue; } }
            if truncated { continue; }
            let mut row = Vec::with_capacity(count);
            let mut row_bytes = count.saturating_mul(8);
            for i in 0..count {
                let i = i as i32;
                let kind = ffi::sqlite3_column_type(raw, i);
                let len = if kind == ffi::SQLITE_TEXT || kind == ffi::SQLITE_BLOB { ffi::sqlite3_column_bytes(raw, i) as usize } else { 0 };
                // Budget JSON escaping before allocating text or base64.
                row_bytes = row_bytes.saturating_add(if kind == ffi::SQLITE_BLOB { len.saturating_add(2) / 3 * 4 + 32 } else { len.saturating_mul(6) + 32 });
                if bytes.saturating_add(row_bytes) > MAX_BYTES { truncated = true; break; }
                row.push(match kind {
                    ffi::SQLITE_NULL => SqlValue::Null,
                    ffi::SQLITE_INTEGER => {
                        let n = ffi::sqlite3_column_int64(raw, i);
                        if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&n) { SqlValue::Text(n.to_string()) } else { SqlValue::Integer(n) }
                    }
                    ffi::SQLITE_FLOAT => SqlValue::Real(ffi::sqlite3_column_double(raw, i)),
                    ffi::SQLITE_BLOB => {
                        let data = if len == 0 { &[] } else { std::slice::from_raw_parts(ffi::sqlite3_column_blob(raw, i).cast::<u8>(), len) };
                        SqlValue::Blob { r#type: "blob", base64: STANDARD.encode(data) }
                    }
                    _ => {
                        let data = if len == 0 { &[] } else { std::slice::from_raw_parts(ffi::sqlite3_column_text(raw, i), len) };
                        SqlValue::Text(String::from_utf8_lossy(data).into_owned())
                    }
                });
            }
            if truncated { if readonly { break; } } else { bytes += row_bytes; rows.push(row); }
        }
        let affected_rows = if !readonly && dml.load(Ordering::Relaxed) && ffi::sqlite3_total_changes64(db) != before { ffi::sqlite3_changes64(db) as u64 } else { 0 };
        drop(statement);
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>).map_err(|e| e.to_string())?;
        if Instant::now() >= deadline { return Err("SQL time limit exceeded".into()); }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        Ok(SqlResult { columns, rows, affected_rows, readonly, truncated, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(conn: &Connection, sql: &str, write: bool) -> Result<SqlResult, String> {
        let now = Instant::now(); execute(conn, sql, write, now, now + TIME_LIMIT)
    }
    #[test]
    fn statement_boundaries_and_types() {
        let conn = Connection::open_in_memory().unwrap();
        let r = run(&conn, "SELECT ';', NULL, x'00ff', 9223372036854775807 UNION ALL SELECT 'two',NULL,x'',1; -- tail", false).unwrap();
        assert_eq!(r.rows.len(), 2);
        assert!(matches!(r.rows[0][1], SqlValue::Null));
        assert!(matches!(&r.rows[0][2], SqlValue::Blob { base64, .. } if base64 == "AP8="));
        assert!(matches!(&r.rows[0][3], SqlValue::Text(s) if s == "9223372036854775807"));
        assert!(run(&conn, "SELECT 1; SELECT 2", false).is_err());
        assert!(run(&conn, "-- empty", false).is_err());
    }
    #[test]
    fn writes_confirmation_returning_and_ddl_counts() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(run(&conn, "CREATE TABLE t(n INTEGER UNIQUE)", false).unwrap_err().starts_with("WRITE_CONFIRMATION_REQUIRED"));
        run(&conn, "CREATE TABLE t(n INTEGER UNIQUE)", true).unwrap();
        let sql = "WITH v(n) AS (VALUES(1),(2)) INSERT INTO t SELECT n FROM v RETURNING n";
        assert!(run(&conn, sql, false).unwrap_err().starts_with("WRITE_CONFIRMATION_REQUIRED"));
        let r = run(&conn, sql, true).unwrap();
        assert!(!r.readonly); assert_eq!(r.affected_rows, 2); assert_eq!(r.rows.len(), 2);
        assert_eq!(run(&conn, "CREATE INDEX idx ON t(n)", true).unwrap().affected_rows, 0);
        assert!(run(&conn, "UPDATE t SET n=3", true).is_err());
        assert_eq!(conn.query_row("SELECT sum(n) FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        assert_eq!(run(&conn, "DELETE FROM t WHERE n=99", true).unwrap().affected_rows, 0);
        for sql in ["BEGIN", "COMMIT", "ATTACH ':memory:' AS other", "PRAGMA writable_schema=ON", "VACUUM INTO '/tmp/debug-sql-test.db'"] { assert!(run(&conn, sql, true).is_err(), "{sql}"); }
        assert!(conn.is_autocommit());
    }
    #[test]
    fn limits_timeout_rollback_and_connection_reuse() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, "CREATE TABLE t(n)", true).unwrap();
        let now = Instant::now();
        let r = execute(&conn, "WITH RECURSIVE v(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM v WHERE n<100000000) INSERT INTO t SELECT n FROM v", true, now, now + Duration::from_millis(10));
        assert!(r.is_err());
        assert_eq!(conn.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(run(&conn, "SELECT 1", false).unwrap().rows.len(), 1);
        assert!(run(&conn, "SELECT zeroblob(99999999)", false).is_err());
        let r = run(&conn, "WITH RECURSIVE v(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM v WHERE n<1002) SELECT n FROM v", false).unwrap();
        assert!(r.truncated); assert_eq!(r.rows.len(), MAX_ROWS);
        let r = run(&conn, "WITH RECURSIVE v(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM v WHERE n<1002) INSERT INTO t SELECT n FROM v RETURNING n", true).unwrap();
        assert!(r.truncated); assert_eq!(r.rows.len(), MAX_ROWS); assert_eq!(r.affected_rows, 1002);
        assert_eq!(conn.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 1002);
        assert!(run(&conn, "SELECT printf('%.*c', 1000000, 'x')", false).unwrap().truncated);
    }
}
