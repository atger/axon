//! SQLite-backed task store for the self-improvement pipeline.
//!
//! Tasks are the planner's output and the unit the human reviews. The DB at
//! `~/.axon/axon.db` is the single source of truth. Bodies are markdown.
//! Active vs history is a status filter:
//!   active  = proposed | accepted | implementing
//!   history = rejected | implemented | failed

use std::fs;
use std::sync::{Mutex, MutexGuard, OnceLock};

use color_eyre::eyre::{Result, WrapErr};
use rusqlite::{Connection, Row, params};
use serde::Serialize;

use crate::daemon::axon_data_dir;

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// Open (once) and migrate the database. Panics only if the data dir or DB file
/// cannot be created — without it the server cannot function.
fn conn() -> MutexGuard<'static, Connection> {
    DB.get_or_init(|| {
        let dir = axon_data_dir().expect("axon data dir");
        fs::create_dir_all(&dir).expect("create ~/.axon");
        let c = Connection::open(dir.join("axon.db")).expect("open axon.db");
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                 id          TEXT PRIMARY KEY,
                 title       TEXT NOT NULL,
                 description TEXT NOT NULL DEFAULT '',
                 body        TEXT NOT NULL DEFAULT '',
                 tags        TEXT NOT NULL DEFAULT '',
                 status      TEXT NOT NULL DEFAULT 'proposed',
                 source      TEXT NOT NULL DEFAULT 'planner',
                 created     TEXT NOT NULL,
                 updated     TEXT NOT NULL
             );",
        )
        .expect("migrate tasks table");
        Mutex::new(c)
    })
    .lock()
    .expect("task db mutex poisoned")
}

const ACTIVE: &str = "('proposed','accepted','implementing')";
const HISTORY: &str = "('rejected','implemented','failed')";

#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub tags: String,
    pub status: String,
    pub source: String,
    pub created: String,
    pub updated: String,
}

fn row_to_task(row: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        body: row.get(3)?,
        tags: row.get(4)?,
        status: row.get(5)?,
        source: row.get(6)?,
        created: row.get(7)?,
        updated: row.get(8)?,
    })
}

const COLS: &str = "id, title, description, body, tags, status, source, created, updated";

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() { "task".into() } else { s }
}

/// Insert a new `proposed` task; returns its id.
pub fn add_task(title: &str, description: &str, tags: &str, body: &str) -> Result<String> {
    let now = chrono::Local::now().to_rfc3339();
    let id = format!("{}-{}", slugify(title), chrono::Local::now().format("%Y%m%d%H%M%S"));
    conn()
        .execute(
            "INSERT INTO tasks (id,title,description,body,tags,status,source,created,updated)
             VALUES (?1,?2,?3,?4,?5,'proposed','planner',?6,?6)",
            params![id, title, description, body, tags, now],
        )
        .wrap_err("failed to insert task")?;
    Ok(id)
}

fn query(where_clause: &str, order: &str) -> Result<Vec<Task>> {
    let c = conn();
    let mut stmt = c.prepare(&format!(
        "SELECT {COLS} FROM tasks WHERE status IN {where_clause} ORDER BY created {order}"
    ))?;
    let rows = stmt.query_map([], row_to_task)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn list_active() -> Result<Vec<Task>> {
    // Oldest first so the longest-waiting task is at the top of the queue.
    query(ACTIVE, "ASC")
}

pub fn list_history() -> Result<Vec<Task>> {
    // Most recently resolved first.
    query(HISTORY, "DESC")
}

pub fn get(id: &str) -> Result<Task> {
    let c = conn();
    c.query_row(
        &format!("SELECT {COLS} FROM tasks WHERE id = ?1"),
        params![id],
        row_to_task,
    )
    .wrap_err_with(|| format!("task `{id}` not found"))
}

/// Human edit of a task's title + markdown body.
pub fn update(id: &str, title: &str, body: &str) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    let n = conn()
        .execute(
            "UPDATE tasks SET title=?2, body=?3, updated=?4 WHERE id=?1",
            params![id, title, body, now],
        )
        .wrap_err("failed to update task")?;
    if n == 0 {
        color_eyre::eyre::bail!("task `{id}` not found");
    }
    Ok(())
}

pub fn set_status(id: &str, status: &str) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    let n = conn()
        .execute(
            "UPDATE tasks SET status=?2, updated=?3 WHERE id=?1",
            params![id, status, now],
        )
        .wrap_err("failed to set task status")?;
    if n == 0 {
        color_eyre::eyre::bail!("task `{id}` not found");
    }
    Ok(())
}
