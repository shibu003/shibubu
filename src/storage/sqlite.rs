//! SQLite の正本（CORE-009）。open のたびに WAL・foreign keys ON・migration。
//! 行の中身は JSON 文字列、親子は FK（ON DELETE CASCADE）。時刻は SQLite が RFC3339（UTC・ms）で入れる。
//! `Connection` は Send / !Sync なので、thread 間で共有する側が `Mutex<Store>` で包む。
use super::log::redact_value;
use super::migrations;
use rusqlite::{
    params, params_from_iter, Connection, OptionalExtension, Row, Transaction, TransactionBehavior,
};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// updated_at の更新式。
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    /// 親を持たない table に parent_id を渡した等、呼び方の誤り。
    Usage(String),
    /// W2c: DB が この binary より新しい版で保存されている（`found` = DB の schema の版・`supported` = `MIGRATIONS.len()`）。
    /// 何も書かずに拒否する。画面の 1 行は Display（英語・FX1 の反証の直し: Home の 1 行と閉じる時の 1 行にそのまま入る）。送出は `Store::init`（migration を当てる前）。
    NewerSchema { found: i64, supported: i64 },
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Usage(s) => write!(f, "usage: {s}"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "saved by a newer Shibubu (version {found}, this one reads up to {supported}) · update Shibubu to open it"
            ),
        }
    }
}
impl std::error::Error for StoreError {}
impl StoreError {
    /// RQ3（RQ-011）: DB の file そのものが壊れている（SQLite の `SQLITE_CORRUPT`・`SQLITE_NOTADB`）。呼ぶ側の例（`Generated::open`）:
    /// `Err(e) if e.is_damaged() => { /* backups/<UTC>-damaged/ へ移して空から開き、Restore の 1 行 */ }`・それ以外の Err は今まで通り memory の store。
    /// 保証する: lock（別の writer）・権限・`NewerSchema`・io の失敗は false（file は壊れていないので動かさない）。保証しない: 開けた DB の中に壊れた page が無い事（読むまで分からない）。
    /// Wave: RQ3
    pub fn is_damaged(&self) -> bool {
        matches!(self, Self::Sqlite(rusqlite::Error::SqliteFailure(e, _)) if matches!(e.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase))
    }
}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// `id TEXT PRIMARY KEY` + `json` を持つ table。親の列は `parent_column`。
/// 複合 key の table（app_revisions / ui_state / run_events）は専用の fn。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Apps,
    Projects,
    Sessions,
    Runs,
    Suggestions,
    Permissions,
    Evidence,
    /// W4: Terminal の session の実行の記録（id = session_id・親 = project_id・json = `terminal::SessionInfo`）。
    TerminalSessions,
}
impl Table {
    pub const ALL: [Table; 8] = [
        Table::Apps,
        Table::Projects,
        Table::Sessions,
        Table::Runs,
        Table::Suggestions,
        Table::Permissions,
        Table::Evidence,
        Table::TerminalSessions,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Table::Apps => "apps",
            Table::Projects => "projects",
            Table::Sessions => "sessions",
            Table::Runs => "runs",
            Table::Suggestions => "suggestions",
            Table::Permissions => "permissions",
            Table::Evidence => "evidence",
            Table::TerminalSessions => "terminal_sessions",
        }
    }
    /// 親を指す列。apps は無し。permissions の app_id だけ NULL 可（app に紐付かない許可）。
    pub fn parent_column(self) -> Option<&'static str> {
        match self {
            Table::Apps | Table::Projects => None,
            Table::Sessions | Table::Permissions => Some("app_id"),
            Table::Runs | Table::Suggestions => Some("session_id"),
            Table::Evidence => Some("run_id"),
            Table::TerminalSessions => Some("project_id"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub parent_id: Option<String>,
    pub json: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    pub app_id: String,
    pub revision: i64,
    pub json: Value,
    /// 呼び出し側が渡した「元にした内容の hash」。store は計算も照合もしない。
    pub base_hash: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunEvent {
    pub run_id: String,
    pub seq: i64,
    pub kind: String,
    /// 保存時に `redact_value` を通した payload。
    pub payload: Value,
    pub recorded_at: String,
}

/// project_revisions の 1 行。revision の番号は定義の `revision` と同じ物を呼ぶ側が渡す（store は採番しない・重なれば PK で落ちる）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRevision {
    pub project_id: String,
    pub revision: i64,
    pub json: Value,
    pub base_hash: Option<String>,
    pub ops: Value,
    pub origin: String,
    pub created_at: String,
}

/// W2c: surface_receipts の 1 行。`json` は `domain::project::SurfaceReceipt` の serde そのまま（storage は domain を引かない）。
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceReceiptRow {
    pub seq: i64,
    pub project_id: String,
    pub surface_id: String,
    pub page_id: String,
    pub kind: String,
    pub state: String,
    pub json: Value,
    pub recorded_at: String,
}

/// W11: run_receipts の 1 行。`json` は `domain::terminal::RunReceipt` の serde そのまま（storage は domain を引かない）。
#[derive(Debug, Clone, PartialEq)]
pub struct RunReceiptRow {
    pub run_id: String,
    pub project_id: String,
    pub session_id: String,
    pub json: Value,
    pub recorded_at: String,
}

type RecordRow = (String, Option<String>, String, String, String);
type RevisionRow = (String, i64, String, Option<String>, String);
type EventRow = (String, i64, String, String, String);

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        Self::init(Connection::open(path)?)
    }

    /// RQ3（RQ-011・migration の前の写し）: `open` と同じだが、既に在る DB に当てていない migration が在る時だけ、当てる前に `before_update`（file の path・
    /// 例 `<data dir>/backups/<UTC>-before-update/kangae.sqlite3`）へ `back_up_to` で写し、migration が失敗したらその写しを元の path に戻してから Err を返す
    /// （前の版の Shibubu でそのまま開ける）。新しい DB（version 0）と migration の無い起動は写さない。例（`Generated::open`）:
    /// `Store::open_backing_up(&dir.join("kangae.sqlite3"), &dir.join(BACKUPS_DIR).join(format!("{stamp}-before-update")).join("kangae.sqlite3"))`
    /// 保証する: 写しが書けなければ migration を当てずに Err（写しの無い migration をしない）・`NewerSchema` は写さずに拒否（今と同じ）。
    /// 保証しない: 世代の数の上限（`Generated::prune_records` が消す）。
    /// Wave: RQ3
    pub fn open_backing_up(path: &Path, before_update: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let store = Self { conn: Self::prepare(Connection::open(path)?)? };
        let found = Self::found_version(&store.conn)?;
        let behind = found > 0 && found < migrations::MIGRATIONS.len() as i64;
        if behind {
            store.back_up_to(before_update)?;
        }
        match migrations::apply(&store.conn) {
            Ok(_) => Ok(store),
            Err(e) if behind => {
                // 当てかけの migration（前の版で読めない形）を写しで置き換える。-wal・-shm を残すと SQLite が写しの上に古い WAL を当て直す
                drop(store);
                for side in ["-wal", "-shm"] {
                    let _ = std::fs::remove_file(PathBuf::from(format!("{}{side}", path.display())));
                }
                std::fs::copy(before_update, path)?;
                Err(e.into())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// RQ3（RQ-011・`Back up now` と毎日の世代）: 今の DB の一貫した写しを `dest` に書く（`VACUUM INTO`・WAL の中の書きかけも含めた 1 時点・読むだけの接続でも可）。
    /// rusqlite の `backup` の feature を使わない（Cargo.toml を変えない）。`dest` の dir は無ければ作る・`dest` が既に在れば Err（上書きしない）。
    /// 例: `store.back_up_to(&dir.join(BACKUPS_DIR).join(format!("{stamp}-manual")).join("kangae.sqlite3"))?`
    /// 保証する: 元の DB を変えない・写しは `Store::open` で開ける。保証しない: 速さ（DB の大きさに比例・呼ぶ側が裏の thread で呼ぶ）・memory の store の写し（Err `Usage`）。
    /// Wave: RQ3
    pub fn back_up_to(&self, dest: &Path) -> Result<(), StoreError> {
        if self.conn.path().is_none_or(str::is_empty) {
            return Err(StoreError::Usage("memory の store は写せない".into()));
        }
        if dest.exists() {
            return Err(StoreError::Usage(format!("{} が既に在る（上書きしない）", dest.display())));
        }
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let to = dest.to_str().ok_or_else(|| StoreError::Usage("写しの path が UTF-8 でない".into()))?;
        self.conn.execute("VACUUM INTO ?1", [to])?;
        Ok(())
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    /// W9: 別 process（Terminal の `shibubu project …`）が読むだけで開く。file が無ければ Err（dir も file も作らない）・migration を当てない・
    /// 行を書かない・DB の主 file を変えない（`SQLITE_OPEN_READ_ONLY`）・DB がこの binary より新しい版なら `NewerSchema`。busy_timeout は `open` と同じ 5 秒。
    /// 例: `let store = Store::open_read_only(&data_dir.join("kangae.sqlite3"))?; let row = store.get(Table::Projects, &project_id)?;`
    /// 保証しない: 古い版の DB（Shibubu が開いた後は同じ版）の表が揃っている事・書く口（`put` 等）を呼んだ時の振る舞い（SQLite の readonly の誤り）・
    /// WAL の付属 file を作らない事（誰も開いていない DB〈-wal / -shm が無い〉を読むと、SQLite が -shm〈32 KiB の索引〉と空の -wal を作って残す。
    /// 行は変わらず、次に Shibubu が開いた時にそのまま使う。`immutable=1` で開けば作らないが、Shibubu が同時に書く間に古い page を読みうるので使わない）。
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        // SQLITE_OPEN_CREATE を渡さないので file は作らない（無ければ SQLite の CANTOPEN より先に、在処を言う 1 行で返す）
        if !path.is_file() {
            return Err(StoreError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} がありません", path.display()))));
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Self::refuse_newer(&conn)?;
        Ok(Self { conn })
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        let conn = Self::prepare(conn)?;
        migrations::apply(&conn)?;
        Ok(Self { conn })
    }

    /// `init` の migration の前まで（`open_backing_up` が間に写しを挟む）。
    fn prepare(conn: Connection) -> Result<Connection, StoreError> {
        // journal_mode は file に残るが foreign_keys は接続ごとに切れる。毎回両方。
        // file なら "wal"、memory なら "memory" が返る（値は見ない）。
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
        conn.busy_timeout(Duration::from_secs(5))?; // 別 process の writer を 5 秒待つ
        Self::refuse_newer(&conn)?;
        Ok(conn)
    }

    /// 新しい binary が保存した DB を古い binary が開いた時は、何も当てずに拒否する（deny_unknown_fields の代償・W2c）。`init` と `open_read_only` の共通。
    fn refuse_newer(conn: &Connection) -> Result<(), StoreError> {
        let found = Self::found_version(conn)?;
        let supported = migrations::MIGRATIONS.len() as i64;
        if found > supported {
            return Err(StoreError::NewerSchema { found, supported });
        }
        Ok(())
    }

    /// 保存した schema の版（`schema_migrations` が無い = 新しい DB = 0）。migration を当てる前に読む。
    fn found_version(conn: &Connection) -> Result<i64, StoreError> {
        let has_versions: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |r| r.get(0),
        )?;
        Ok(if has_versions { conn.query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |r| r.get(0))? } else { 0 })
    }

    /// `PRAGMA <name>` の値を文字列で（診断・test 用）。
    pub fn pragma(&self, name: &str) -> Result<String, StoreError> {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Usage(format!("pragma 名が不正: {name}")));
        }
        let v: rusqlite::types::Value =
            self.conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))?;
        Ok(match v {
            rusqlite::types::Value::Integer(i) => i.to_string(),
            rusqlite::types::Value::Text(t) => t,
            other => format!("{other:?}"),
        })
    }

    /// 適用済み migration の最大 version。
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?)
    }

    /// `BEGIN IMMEDIATE` の中で f を動かす: 読んでから書くまでの間に、同じ file を開いた別の接続（別 process）の writer を
    /// 入れない（process 内の Mutex は別の接続を止めない）。f が Ok なら COMMIT、Err か panic なら ROLLBACK（Transaction の drop）。
    /// 書く lock が空くまで busy_timeout（5 秒）待つ。入れ子にしない（f の中で呼ぶと SQLite が拒否する）。
    pub fn immediate<T, E>(
        &self,
        map_err: impl Fn(StoreError) -> E,
        f: impl FnOnce(&Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| map_err(e.into()))?;
        let out = f(self)?;
        tx.commit().map_err(|e| map_err(e.into()))?;
        Ok(out)
    }

    // ---- id を主キーに持つ table の put / get / list / delete ----

    /// 無ければ insert、あれば json と親を置き換える（created_at は残る）。
    pub fn put(
        &self,
        table: Table,
        id: &str,
        parent_id: Option<&str>,
        json: &Value,
    ) -> Result<(), StoreError> {
        let text = serde_json::to_string(json)?;
        let t = table.name();
        match table.parent_column() {
            Some(col) => {
                let sql = format!(
                    "INSERT INTO {t}(id, {col}, json) VALUES (?1, ?2, ?3) \
                     ON CONFLICT(id) DO UPDATE SET {col}=excluded.{col}, json=excluded.json, updated_at={NOW}"
                );
                self.conn.execute(&sql, params![id, parent_id, text])?;
            }
            None => {
                if parent_id.is_some() {
                    return Err(StoreError::Usage(format!("{t} に親は無い")));
                }
                let sql = format!(
                    "INSERT INTO {t}(id, json) VALUES (?1, ?2) \
                     ON CONFLICT(id) DO UPDATE SET json=excluded.json, updated_at={NOW}"
                );
                self.conn.execute(&sql, params![id, text])?;
            }
        }
        Ok(())
    }

    pub fn get(&self, table: Table, id: &str) -> Result<Option<Record>, StoreError> {
        let sql = format!("{} WHERE id=?1", Self::select(table));
        self.conn
            .query_row(&sql, [id], Self::record_row)
            .optional()?
            .map(Self::record)
            .transpose()
    }

    /// `parent_id` を渡すとその親の行だけ。None なら全行。created_at, id 順。
    pub fn list(&self, table: Table, parent_id: Option<&str>) -> Result<Vec<Record>, StoreError> {
        let base = Self::select(table);
        let (sql, args): (String, Vec<&str>) = match (table.parent_column(), parent_id) {
            (Some(col), Some(p)) => (format!("{base} WHERE {col}=?1 ORDER BY created_at, id"), vec![p]),
            (None, Some(_)) => return Err(StoreError::Usage(format!("{} に親は無い", table.name()))),
            _ => (format!("{base} ORDER BY created_at, id"), vec![]),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), Self::record_row)?;
        rows.map(|r| -> Result<Record, StoreError> { Self::record(r?) }).collect()
    }

    /// 消したら true。子の行は FK の CASCADE で一緒に消える。
    pub fn delete(&self, table: Table, id: &str) -> Result<bool, StoreError> {
        let n = self
            .conn
            .execute(&format!("DELETE FROM {} WHERE id=?1", table.name()), [id])?;
        Ok(n > 0)
    }

    /// RQ2（RQ-005・Delete）: Project の id で引く FK の無い表（`terminal_sessions`・`run_receipts`）のその Project の行を消す。消した行の数。
    /// `projects` の行の子（`project_revisions`・`surface_receipts`）は `delete(Table::Projects, id)` の CASCADE・app の子は `delete(Table::Apps, id)` の CASCADE。
    pub fn delete_project_rows(&self, project_id: &str) -> Result<usize, StoreError> {
        let sessions = self.conn.execute("DELETE FROM terminal_sessions WHERE project_id=?1", [project_id])?;
        let receipts = self.conn.execute("DELETE FROM run_receipts WHERE project_id=?1", [project_id])?;
        Ok(sessions + receipts)
    }

    fn select(table: Table) -> String {
        let parent = table.parent_column().unwrap_or("NULL");
        format!(
            "SELECT id, {parent}, json, created_at, updated_at FROM {}",
            table.name()
        )
    }
    fn record_row(r: &Row<'_>) -> rusqlite::Result<RecordRow> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    }
    fn record(t: RecordRow) -> Result<Record, StoreError> {
        Ok(Record {
            id: t.0,
            parent_id: t.1,
            json: serde_json::from_str(&t.2)?,
            created_at: t.3,
            updated_at: t.4,
        })
    }

    // ---- app_revisions ----

    /// revision 番号は store が採番する（その app の max + 1。1 文で atomic）。
    pub fn put_revision(
        &self,
        app_id: &str,
        json: &Value,
        base_hash: Option<&str>,
    ) -> Result<Revision, StoreError> {
        let text = serde_json::to_string(json)?;
        let (revision, created_at): (i64, String) = self.conn.query_row(
            "INSERT INTO app_revisions(app_id, revision, json, base_hash) \
             SELECT ?1, COALESCE(MAX(revision), 0) + 1, ?2, ?3 FROM app_revisions WHERE app_id=?1 \
             RETURNING revision, created_at",
            params![app_id, text, base_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(Revision {
            app_id: app_id.into(),
            revision,
            json: json.clone(),
            base_hash: base_hash.map(String::from),
            created_at,
        })
    }

    pub fn get_revision(&self, app_id: &str, revision: i64) -> Result<Option<Revision>, StoreError> {
        self.conn
            .query_row(
                "SELECT app_id, revision, json, base_hash, created_at FROM app_revisions \
                 WHERE app_id=?1 AND revision=?2",
                params![app_id, revision],
                Self::revision_row,
            )
            .optional()?
            .map(Self::revision)
            .transpose()
    }

    pub fn latest_revision(&self, app_id: &str) -> Result<Option<Revision>, StoreError> {
        self.conn
            .query_row(
                "SELECT app_id, revision, json, base_hash, created_at FROM app_revisions \
                 WHERE app_id=?1 ORDER BY revision DESC LIMIT 1",
                [app_id],
                Self::revision_row,
            )
            .optional()?
            .map(Self::revision)
            .transpose()
    }

    pub fn list_revisions(&self, app_id: &str) -> Result<Vec<Revision>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT app_id, revision, json, base_hash, created_at FROM app_revisions \
             WHERE app_id=?1 ORDER BY revision",
        )?;
        let rows = stmt.query_map([app_id], Self::revision_row)?;
        rows.map(|r| -> Result<Revision, StoreError> { Self::revision(r?) }).collect()
    }

    fn revision_row(r: &Row<'_>) -> rusqlite::Result<RevisionRow> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    }
    fn revision(t: RevisionRow) -> Result<Revision, StoreError> {
        Ok(Revision {
            app_id: t.0,
            revision: t.1,
            json: serde_json::from_str(&t.2)?,
            base_hash: t.3,
            created_at: t.4,
        })
    }

    // ---- project_revisions ----

    /// Wave: W1
    /// G86: 会話の列に 1 つ足す（追記だけ・json = `domain::chat::Turn`）。
    pub fn append_chat_turn(&self, project_id: &str, json: &Value) -> Result<(), StoreError> {
        self.conn.execute("INSERT INTO chat_turns(project_id, json) VALUES (?1, ?2)", params![project_id, serde_json::to_string(json)?])?;
        Ok(())
    }

    /// G86: Project の会話（古い順）。
    pub fn list_chat_turns(&self, project_id: &str) -> Result<Vec<Value>, StoreError> {
        let mut stmt = self.conn.prepare("SELECT json FROM chat_turns WHERE project_id=?1 ORDER BY seq")?;
        let rows = stmt.query_map([project_id], |r| r.get::<_, String>(0))?;
        rows.map(|r| -> Result<Value, StoreError> { Ok(serde_json::from_str(&r?)?) }).collect()
    }

    /// SESSIONS-LEFT-1101: 会話ごとの頭（key = Project の id か `chat::HOME_CHAT` の key・最初の人の一言〈無ければ最初の行〉の json・最後に足した時刻）。
    /// 並びは最後に足した順（新しい順）。左の sidebar の会話の一覧が読む。
    pub fn list_chats(&self) -> Result<Vec<(String, Value, String)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.project_id, \
               (SELECT f.json FROM chat_turns f WHERE f.project_id = c.project_id ORDER BY (json_extract(f.json, '$.who') = 'you') DESC, f.seq LIMIT 1), \
               MAX(c.recorded_at) \
             FROM chat_turns c GROUP BY c.project_id ORDER BY MAX(c.seq) DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        rows.map(|r| -> Result<(String, Value, String), StoreError> {
            let (key, json, at) = r?;
            Ok((key, serde_json::from_str(&json)?, at))
        })
        .collect()
    }

    /// SESSIONS-LEFT-1101: 1 つの会話の行を全部消す（Home の会話が Project になった = 会話は Project の列へ写った時）。消した行の数。
    pub fn delete_chat_turns(&self, project_id: &str) -> Result<usize, StoreError> {
        Ok(self.conn.execute("DELETE FROM chat_turns WHERE project_id=?1", [project_id])?)
    }

    pub fn put_project_revision(
        &self,
        project_id: &str,
        revision: i64,
        json: &Value,
        base_hash: Option<&str>,
        ops: &Value,
        origin: &str,
    ) -> Result<ProjectRevision, StoreError> {
        let created_at: String = self.conn.query_row(
            "INSERT INTO project_revisions(project_id, revision, json, base_hash, ops_json, origin) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING created_at",
            params![project_id, revision, serde_json::to_string(json)?, base_hash, serde_json::to_string(ops)?, origin],
            |r| r.get(0),
        )?;
        Ok(ProjectRevision {
            project_id: project_id.into(),
            revision,
            json: json.clone(),
            base_hash: base_hash.map(String::from),
            ops: ops.clone(),
            origin: origin.into(),
            created_at,
        })
    }

    /// 作った順。W8: Undo の後の最後の行は今の Project ではない（Redo の行き先）。今は `projects` の行の json の `revision` で見る。
    pub fn list_project_revisions(&self, project_id: &str) -> Result<Vec<ProjectRevision>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT project_id, revision, json, base_hash, ops_json, origin, created_at FROM project_revisions \
             WHERE project_id=?1 ORDER BY revision",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|r| -> Result<ProjectRevision, StoreError> {
            let t = r?;
            Ok(ProjectRevision {
                project_id: t.0,
                revision: t.1,
                json: serde_json::from_str(&t.2)?,
                base_hash: t.3,
                ops: serde_json::from_str(&t.4)?,
                origin: t.5,
                created_at: t.6,
            })
        })
        .collect()
    }

    /// W8: `revision > after` の行を消す（Undo の後に別の編集を当てる時の Redo の行き先）。消した行の数を返す。最後の行に居る時は 0。
    /// 使い方: `apply_project` の BEGIN IMMEDIATE の中で、新しい行を `put_project_revision` する直前に `store.truncate_project_revisions(&p.id, p.revision as i64)?`。
    /// 保証する: `after` 以下の行・`projects`・他の表（surface_receipts・ui_state・terminal_sessions）は触らない。保証しない: 呼ぶ側の transaction の外での原子性。
    /// Wave: W8
    pub fn truncate_project_revisions(&self, project_id: &str, after: i64) -> Result<usize, StoreError> {
        Ok(self.conn.execute("DELETE FROM project_revisions WHERE project_id=?1 AND revision>?2", params![project_id, after])?)
    }

    // ---- surface_receipts（W2c・追記だけ）----

    /// 読み戻しの結果を 1 行足す。照合はしない: 呼ぶ側が先に `SurfaceReceipt::validate` を通す（拒否はそこ 1 か所）。
    /// 例: `store.put_surface_receipt("project.x", "docs", "page.docs", "native_app", "verified", &serde_json::to_value(&r)?)?`
    /// 保証する: 行は消さず書き換えない（履歴が残る）・project を消すと消える（FK）。保証しない: json の中身が SurfaceReceipt の形である事。
    /// Wave: W2c
    pub fn put_surface_receipt(
        &self,
        project_id: &str,
        surface_id: &str,
        page_id: &str,
        kind: &str,
        state: &str,
        json: &Value,
    ) -> Result<SurfaceReceiptRow, StoreError> {
        let seq = self.conn.query_row(
            "INSERT INTO surface_receipts(project_id, surface_id, page_id, kind, state, json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING seq",
            params![project_id, surface_id, page_id, kind, state, serde_json::to_string(json)?],
            |r| r.get::<_, i64>(0),
        )?;
        self.surface_receipts_where("seq=?1", &[&seq])?
            .pop()
            .ok_or_else(|| StoreError::Usage(format!("surface_receipts: 足した行 {seq} が読めない")))
    }

    /// project の receipt を足した順に。surface_id を渡すとその surface だけ。再起動の後の復元（W8）と画面の状態の表示が使う。
    pub fn list_surface_receipts(
        &self,
        project_id: &str,
        surface_id: Option<&str>,
    ) -> Result<Vec<SurfaceReceiptRow>, StoreError> {
        match surface_id {
            Some(s) => self.surface_receipts_where("project_id=?1 AND surface_id=?2", &[&project_id, &s]),
            None => self.surface_receipts_where("project_id=?1", &[&project_id]),
        }
    }

    fn surface_receipts_where(
        &self,
        cond: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<SurfaceReceiptRow>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT seq, project_id, surface_id, page_id, kind, state, json, recorded_at FROM surface_receipts \
             WHERE {cond} ORDER BY seq"
        ))?;
        let rows = stmt.query_map(args, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|r| -> Result<SurfaceReceiptRow, StoreError> {
            let t = r?;
            Ok(SurfaceReceiptRow {
                seq: t.0,
                project_id: t.1,
                surface_id: t.2,
                page_id: t.3,
                kind: t.4,
                state: t.5,
                json: serde_json::from_str(&t.6)?,
                recorded_at: t.7,
            })
        })
        .collect()
    }

    // ---- run_receipts（W11・追記だけ。同じ run_id は拒否 = PRIMARY KEY）----

    /// command の記録を 1 行足す。照合はしない: 呼ぶ側が先に `RunReceipt::validate` を通す（拒否はそこ 1 か所）。
    ///
    /// 使い方: `r.validate()?; store.put_run_receipt(&r.run_id, &r.project_id, &r.session_id, &serde_json::to_value(&r)?)?`
    /// 保証する（実装の約束）: 行は消さず書き換えない。同じ run_id は Sqlite の UNIQUE で失敗。
    /// 保証しない: json の中身が RunReceipt の形である事・失敗を成功と出さない事（それは `RunReceipt::status_line`）。
    pub fn put_run_receipt(
        &self,
        run_id: &str,
        project_id: &str,
        session_id: &str,
        json: &Value,
    ) -> Result<RunReceiptRow, StoreError> {
        let recorded_at: String = self.conn.query_row(
            "INSERT INTO run_receipts(run_id, project_id, session_id, json) VALUES (?1, ?2, ?3, ?4) RETURNING recorded_at",
            params![run_id, project_id, session_id, serde_json::to_string(json)?],
            |r| r.get(0),
        )?;
        Ok(RunReceiptRow {
            run_id: run_id.to_owned(),
            project_id: project_id.to_owned(),
            session_id: session_id.to_owned(),
            json: json.clone(),
            recorded_at,
        })
    }

    /// project の receipt を新しい順に。session_id を渡すとその session だけ。
    ///
    /// 使い方: `store.list_run_receipts("project.x", Some("dev"))?`
    /// 保証する（実装の約束）: 空なら空の vec（表が無い事にはしない・V5）。
    pub fn list_run_receipts(
        &self,
        project_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<RunReceiptRow>, StoreError> {
        match session_id {
            Some(s) => self.run_receipts_where("project_id=?1 AND session_id=?2", &[&project_id, &s]),
            None => self.run_receipts_where("project_id=?1", &[&project_id]),
        }
    }

    fn run_receipts_where(
        &self,
        cond: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<RunReceiptRow>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT run_id, project_id, session_id, json, recorded_at FROM run_receipts \
             WHERE {cond} ORDER BY recorded_at DESC, rowid DESC"
        ))?;
        let rows = stmt.query_map(args, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|r| -> Result<RunReceiptRow, StoreError> {
            let t = r?;
            Ok(RunReceiptRow {
                run_id: t.0,
                project_id: t.1,
                session_id: t.2,
                json: serde_json::from_str(&t.3)?,
                recorded_at: t.4,
            })
        })
        .collect()
    }

    // ---- ui_state（session ごとの key → json）----

    pub fn put_ui_state(&self, session_id: &str, key: &str, json: &Value) -> Result<(), StoreError> {
        let sql = format!(
            "INSERT INTO ui_state(session_id, key, json) VALUES (?1, ?2, ?3) \
             ON CONFLICT(session_id, key) DO UPDATE SET json=excluded.json, updated_at={NOW}"
        );
        self.conn
            .execute(&sql, params![session_id, key, serde_json::to_string(json)?])?;
        Ok(())
    }

    /// 1 行を消す（RQ6: Mirror の同意の取り消し）。在って消したら true。
    pub fn delete_ui_state(&self, session_id: &str, key: &str) -> Result<bool, StoreError> {
        Ok(self.conn.execute("DELETE FROM ui_state WHERE session_id=?1 AND key=?2", params![session_id, key])? > 0)
    }

    pub fn get_ui_state(&self, session_id: &str, key: &str) -> Result<Option<Value>, StoreError> {
        let text: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM ui_state WHERE session_id=?1 AND key=?2",
                [session_id, key],
                |r| r.get(0),
            )
            .optional()?;
        text.map(|t| serde_json::from_str(&t).map_err(StoreError::from))
            .transpose()
    }

    pub fn list_ui_state(&self, session_id: &str) -> Result<Vec<(String, Value)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, json FROM ui_state WHERE session_id=?1 ORDER BY key")?;
        let rows = stmt.query_map([session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|r| -> Result<(String, Value), StoreError> {
            let (k, t) = r?;
            Ok((k, serde_json::from_str(&t)?))
        })
        .collect()
    }

    // ---- run_events（追記のみ。seq は store が採番）----

    /// payload は `redact_value` を通してから保存する（DATA_CONTRACTS §7 の redacted_payload）。
    pub fn append_run_event(
        &self,
        run_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<RunEvent, StoreError> {
        let payload = redact_value(payload);
        let text = serde_json::to_string(&payload)?;
        let (seq, recorded_at): (i64, String) = self.conn.query_row(
            "INSERT INTO run_events(run_id, seq, kind, json) \
             SELECT ?1, COALESCE(MAX(seq), 0) + 1, ?2, ?3 FROM run_events WHERE run_id=?1 \
             RETURNING seq, recorded_at",
            params![run_id, kind, text],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(RunEvent {
            run_id: run_id.into(),
            seq,
            kind: kind.into(),
            payload,
            recorded_at,
        })
    }

    pub fn list_run_events(&self, run_id: &str) -> Result<Vec<RunEvent>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, seq, kind, json, recorded_at FROM run_events WHERE run_id=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([run_id], |r| -> rusqlite::Result<EventRow> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.map(|r| -> Result<RunEvent, StoreError> {
            let t = r?;
            Ok(RunEvent {
                run_id: t.0,
                seq: t.1,
                kind: t.2,
                payload: serde_json::from_str(&t.3)?,
                recorded_at: t.4,
            })
        })
        .collect()
    }
}
