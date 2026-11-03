//! 永続化・secret・event bus・log（TODO.md CORE-009〜012）。
//! 保存する中身は JSON 文字列（`serde_json::Value`）。型付き層には依存しない（統合役が上に乗せる）。
//! 旧 MVP（`model.rs` の sessions.json）とは独立。中では `crate::` を使わない
//! （`tests/storage.rs` が `#[path]` でこの module を取り込むため、`super::` で参照する）。
pub mod migrations;
pub mod redact;
pub mod secret_store;
pub mod sqlite;

pub use event_bus::{Event, EventBus, EventId, ListenerId, Publish};
pub use log::{redact, redact_value, Level, Logger, REDACTED};
pub use secret_store::{SecretError, SecretStore, KEYCHAIN_SERVICE};
pub use sqlite::{ProjectRevision, Record, Revision, RunEvent, RunReceiptRow, Store, StoreError, SurfaceReceiptRow, Table};

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 時刻（ns）と process 内の連番から作る id。correlation id・event id・record id に使う。
pub fn new_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    format!("{nanos:x}-{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// epoch からの ms。log の `ts` に使う（DB 側の時刻は SQLite の strftime が RFC3339 で入れる）。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---- event_bus（旧 storage/event_bus.rs）----
pub mod event_bus {
    //! process 内の event bus（CORE-011）。EventId で再配達を弾き、seq で論理順序を付け、
    //! listener は id で登録 / 解除。配送は publish の中で同期に行う。
    // ponytail: 単一 thread 前提。worker thread から流すなら Mutex<EventBus> で包む（配送は lock を持つ側で起きる）。
    use serde_json::Value;
    use std::collections::{HashSet, VecDeque};

    pub type EventId = String;
    pub type ListenerId = u64;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Event {
        pub id: EventId,
        /// bus が付ける論理順序（1 から単調増加）。
        pub seq: u64,
        /// `run.completed` / `ui.click` のように `.` で区切る。listener は前方一致で受ける。
        pub topic: String,
        pub payload: Value,
        /// correlation id（log と同じ物を通す）。
        pub cid: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Publish {
        Delivered { seq: u64, listeners: usize },
        /// 同じ EventId を dedup 窓の中で見た。listener は呼ばれない。
        Duplicate,
    }

    struct Listener {
        id: ListenerId,
        prefix: String,
        f: Box<dyn FnMut(&Event) + Send>,
    }

    pub struct EventBus {
        next_seq: u64,
        next_listener: ListenerId,
        listeners: Vec<Listener>,
        seen: HashSet<EventId>,
        order: VecDeque<EventId>,
        window: usize,
    }

    impl EventBus {
        /// `window` = 覚えておく event id の数（有界）。窓から押し出された id の再配達は弾けない。0 なら dedup 無し。
        pub fn new(window: usize) -> Self {
            Self {
                next_seq: 0,
                next_listener: 0,
                listeners: Vec::new(),
                seen: HashSet::new(),
                order: VecDeque::new(),
                window,
            }
        }

        /// `topic_prefix` に前方一致する event を受ける。`""` なら全部。
        pub fn subscribe(
            &mut self,
            topic_prefix: &str,
            f: impl FnMut(&Event) + Send + 'static,
        ) -> ListenerId {
            self.next_listener += 1;
            self.listeners.push(Listener {
                id: self.next_listener,
                prefix: topic_prefix.into(),
                f: Box::new(f),
            });
            self.next_listener
        }

        /// 外したら true。二度目は false。
        pub fn unsubscribe(&mut self, id: ListenerId) -> bool {
            let before = self.listeners.len();
            self.listeners.retain(|l| l.id != id);
            self.listeners.len() != before
        }

        pub fn listener_count(&self) -> usize {
            self.listeners.len()
        }

        /// 新しい id で配る（常に Delivered）。
        pub fn publish(&mut self, topic: &str, payload: Value, cid: Option<&str>) -> Publish {
            self.publish_with_id(super::new_id(), topic, payload, cid)
        }

        /// 再配達され得る物はこちら。同じ id は窓の中なら Duplicate。
        pub fn publish_with_id(
            &mut self,
            id: EventId,
            topic: &str,
            payload: Value,
            cid: Option<&str>,
        ) -> Publish {
            if self.seen.contains(&id) {
                return Publish::Duplicate;
            }
            self.remember(id.clone());
            self.next_seq += 1;
            let event = Event {
                id,
                seq: self.next_seq,
                topic: topic.into(),
                payload,
                cid: cid.map(String::from),
            };
            let mut delivered = 0;
            for l in self.listeners.iter_mut().filter(|l| event.topic.starts_with(&l.prefix)) {
                (l.f)(&event);
                delivered += 1;
            }
            Publish::Delivered { seq: event.seq, listeners: delivered }
        }

        fn remember(&mut self, id: EventId) {
            if self.window == 0 {
                return;
            }
            while self.order.len() >= self.window {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
            self.order.push_back(id.clone());
            self.seen.insert(id);
        }
    }
} // mod event_bus

// ---- log（旧 storage/log.rs）----
pub mod log {
    //! 構造化 log と redact（CORE-012）。1 行 1 JSON。correlation id（cid）で
    //! proposal → patch → run → verification を追う。本文（content 系の key）と secret（key 名・値の形）は
    //! 既定で伏せる。`tracing` は足さない。
    use serde_json::{Map, Value};
    use std::io::Write;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    // W19-3: redact の本体は self-contained の `super::redact`（intel が `#[path]` で同じ file を取り込み、解析の結果からも同じ形で伏せる）
    pub use super::redact::{is_content_key, is_secret_key, redact, redact_value, REDACTED};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Level {
        Debug,
        Info,
        Warn,
        Error,
    }
    impl Level {
        fn name(self) -> &'static str {
            match self {
                Level::Debug => "debug",
                Level::Info => "info",
                Level::Warn => "warn",
                Level::Error => "error",
            }
        }
    }

    pub struct Logger {
        sink: Mutex<Box<dyn Write + Send>>,
        min: Level,
        show_content: bool,
        max_len: usize,
    }

    impl Logger {
        pub fn stderr() -> Self {
            Self::with_sink(Box::new(std::io::stderr()))
        }

        /// 追記で開く。親 dir は作る。
        pub fn file(path: &Path) -> std::io::Result<Self> {
            if let Some(dir) = path.parent() {
                if !dir.as_os_str().is_empty() {
                    std::fs::create_dir_all(dir)?;
                }
            }
            let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            Ok(Self::with_sink(Box::new(f)))
        }

        pub fn with_sink(sink: Box<dyn Write + Send>) -> Self {
            Self { sink: Mutex::new(sink), min: Level::Info, show_content: false, max_len: 512 }
        }

        /// test 用: 書いた物を buffer で受ける。
        pub fn capture() -> (Self, Arc<Mutex<Vec<u8>>>) {
            struct Buf(Arc<Mutex<Vec<u8>>>);
            impl Write for Buf {
                fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                    self.0
                        .lock()
                        .map_err(|_| std::io::Error::other("log buffer の lock が壊れている"))?
                        .extend_from_slice(b);
                    Ok(b.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let buf = Arc::new(Mutex::new(Vec::new()));
            (Self::with_sink(Box::new(Buf(buf.clone()))), buf)
        }

        pub fn min_level(mut self, level: Level) -> Self {
            self.min = level;
            self
        }

        /// 本文を伏せない（debug 用。既定 false）。secret は true でも伏せる。
        pub fn show_content(mut self, yes: bool) -> Self {
            self.show_content = yes;
            self
        }

        /// 1 行書く。`ts`（epoch ms）・`level`・`cid`・`event` は予約 key。fields は key 名と値で redact される。
        pub fn log(&self, level: Level, cid: &str, event: &str, fields: &[(&str, &str)]) {
            if level < self.min {
                return;
            }
            let mut m = Map::new();
            m.insert("ts".into(), Value::from(super::now_ms()));
            m.insert("level".into(), Value::from(level.name()));
            m.insert("cid".into(), Value::from(cid));
            m.insert("event".into(), Value::from(event));
            for (k, v) in fields {
                m.insert((*k).into(), Value::String(self.field(k, v)));
            }
            let line = Value::Object(m).to_string();
            if let Ok(mut s) = self.sink.lock() {
                let _ = writeln!(s, "{line}");
                let _ = s.flush();
            }
        }
        pub fn debug(&self, cid: &str, event: &str, fields: &[(&str, &str)]) {
            self.log(Level::Debug, cid, event, fields)
        }
        pub fn info(&self, cid: &str, event: &str, fields: &[(&str, &str)]) {
            self.log(Level::Info, cid, event, fields)
        }
        pub fn warn(&self, cid: &str, event: &str, fields: &[(&str, &str)]) {
            self.log(Level::Warn, cid, event, fields)
        }
        pub fn error(&self, cid: &str, event: &str, fields: &[(&str, &str)]) {
            self.log(Level::Error, cid, event, fields)
        }

        fn field(&self, key: &str, value: &str) -> String {
            if is_secret_key(key) {
                return REDACTED.into();
            }
            if is_content_key(key) && !self.show_content {
                return format!("[content {} chars]", value.chars().count());
            }
            let v = redact(value);
            if v.chars().count() > self.max_len {
                let mut cut: String = v.chars().take(self.max_len).collect();
                cut.push('…');
                cut
            } else {
                v
            }
        }
    }
} // mod log
