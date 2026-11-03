//! 段階的な schema migration（CORE-009）。`MIGRATIONS[i]` が version `i+1`。
//! 適用済みの version は `schema_migrations` に残り、open のたびに未適用分だけが 1 transaction ずつ走る。
//! 変更は末尾に足す（過去の SQL を書き換えない）。
use rusqlite::Connection;

/// RFC3339（ms・UTC）。created_at / updated_at / recorded_at の既定値。
const V1: &str = "
CREATE TABLE apps (
  id         TEXT PRIMARY KEY,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE app_revisions (
  app_id     TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  revision   INTEGER NOT NULL,
  json       TEXT NOT NULL,
  base_hash  TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (app_id, revision)
);
CREATE TABLE sessions (
  id         TEXT PRIMARY KEY,
  app_id     TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX sessions_app ON sessions(app_id);
CREATE TABLE ui_state (
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  key        TEXT NOT NULL,
  json       TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (session_id, key)
);
CREATE TABLE runs (
  id         TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX runs_session ON runs(session_id, created_at);
CREATE TABLE run_events (
  run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  kind        TEXT NOT NULL,
  json        TEXT NOT NULL,
  recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (run_id, seq)
);
CREATE TABLE suggestions (
  id         TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX suggestions_session ON suggestions(session_id);
CREATE TABLE permissions (
  id         TEXT PRIMARY KEY,
  app_id     TEXT REFERENCES apps(id) ON DELETE CASCADE,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX permissions_app ON permissions(app_id);
CREATE TABLE evidence (
  id         TEXT PRIMARY KEY,
  run_id     TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX evidence_run ON evidence(run_id);
";

/// YC W1（YCM-010・016・043）: Project（複数 Page）と、その revision。revision は全量の snapshot（復元・Undo）と、当てた ops（差分の表示）を持つ。
/// base_hash = 元にした revision の json の digest（1 枚目は NULL）。apps / app_revisions は触らない（古い app は読む時に包む）。
const V2: &str = "
CREATE TABLE projects (
  id         TEXT PRIMARY KEY,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE project_revisions (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  revision   INTEGER NOT NULL,
  json       TEXT NOT NULL,
  base_hash  TEXT,
  ops_json   TEXT NOT NULL,
  origin     TEXT NOT NULL CHECK (origin IN ('generate','nl_edit','direct_edit','undo')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  PRIMARY KEY (project_id, revision)
);
";

/// YC W2c: surface の読み戻しの記録（`domain::project::SurfaceReceipt` 1 件 = 1 行・追記だけ）。最新は surface ごとに seq の最大。
/// 欄（app_id 等）は json に持ち、照合（verified なら 6 欄が空でない・local は verified を取らない）は保存する側の `SurfaceReceipt::validate` 1 か所で行う
/// （ここに CHECK を重ねると、validate を外しても拒否の試験が赤くならない）。列に出すのは引く時に使う物だけ。
const V3: &str = "
CREATE TABLE surface_receipts (
  seq         INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  surface_id  TEXT NOT NULL,
  page_id     TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('native_app','web','terminal','local')),
  state       TEXT NOT NULL CHECK (state IN ('verified','unverified','unavailable','ended')),
  json        TEXT NOT NULL,
  recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX surface_receipts_surface ON surface_receipts(project_id, surface_id, seq);
";

/// YC W4（YCM-020・028。W2c の V3 と同時に足したので V4 にした）: Terminal の session の実行の記録。1 行 = 1 session（id = session_id・json = `terminal::SessionInfo`）。
/// 起こし方（spec）は Project の定義が持つのでここには置かない。PTY の handle は保存しない（SPEC §6）。
/// project_id に FK を張らない: 包んだだけの古い app の Project は、初めての差分まで projects の行を持たない（generated.rs）。
const V4: &str = "
CREATE TABLE terminal_sessions (
  id         TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  json       TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX terminal_sessions_project ON terminal_sessions(project_id);
";

/// YC W11（YCM-027・065）: Terminal で走らせた command 1 回の記録（`domain::terminal::RunReceipt` 1 件 = 1 行・追記だけ）。
/// 欄（exit_code・時刻・入出力・repo・lease_holder・surface_id）は json に持ち、照合は保存する側の `RunReceipt::validate` 1 か所。
/// project_id に FK を張らない: `terminal_sessions` と同じ（包んだだけの古い app の Project は projects の行を持たない）。
const V5: &str = "
CREATE TABLE run_receipts (
  run_id      TEXT PRIMARY KEY,
  project_id  TEXT NOT NULL,
  session_id  TEXT NOT NULL,
  json        TEXT NOT NULL,
  recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX run_receipts_project ON run_receipts(project_id, recorded_at);
CREATE INDEX run_receipts_session ON run_receipts(session_id, recorded_at);
";

/// YC G86: Project の会話の列（`domain::chat::Turn` 1 件 = 1 行・追記だけ・古い順は seq）。
/// project_id に FK を張らない: `terminal_sessions` と同じ（包んだだけの古い app の Project は projects の行を持たない）。
const V6: &str = "
CREATE TABLE chat_turns (
  seq         INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id  TEXT NOT NULL,
  json        TEXT NOT NULL,
  recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX chat_turns_project ON chat_turns(project_id, seq);
";

/// 順番に適用する SQL。index + 1 が version。
pub const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6];

/// 未適用の migration を 1 本ずつ transaction で適用し、適用した本数を返す。
/// W2c: DB の版が `MIGRATIONS.len()` より新しい時は何も当てずに拒否し、
/// `Store::init` が apply の前に見て `StoreError::NewerSchema` にする（古い binary が新しい保存を壊さない）。
pub fn apply(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
           version    INTEGER PRIMARY KEY,
           applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    let mut applied = 0;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute("INSERT INTO schema_migrations(version) VALUES (?1)", [version])?;
        tx.commit()?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V5 が空の DB に `run_receipts` を作る（W11 の保存形式）。
    #[test]
    fn w11_v5_creates_run_receipts() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        assert_eq!(apply(&c).unwrap(), MIGRATIONS.len());
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='run_receipts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let version: i64 = c
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 6);
    }
}
