//! unit ST（TODO.md CORE-009〜012）の受け入れ test。bin crate なので `#[path]` で module を取り込む。
//! secret の test は memory store。本物の Keychain は `--ignored` の 1 本だけ（operator が手で回す）。
#[allow(dead_code, unused_imports)]
#[path = "../src/storage/mod.rs"]
mod storage;

use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use storage::*;

/// test ごとに別 dir（cargo test は並列。pid + 連番 + 名前で一意）。
fn temp_db(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kangae-storage-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("kangae.sqlite3")
}

/// gitleaks に token と見られない様に、形だけの token は実行時に組む。
fn fake_github_token() -> String {
    format!("ghp_{}", "q".repeat(32))
}

fn seed(store: &Store) {
    store.put(Table::Apps, "app1", None, &json!({"name": "Outreach"})).unwrap();
    store.put_revision("app1", &json!({"v": 1}), None).unwrap();
    store.put(Table::Sessions, "s1", Some("app1"), &json!({"title": "Mon"})).unwrap();
    store.put_ui_state("s1", "layout", &json!({"x": 1})).unwrap();
    store.put(Table::Runs, "run1", Some("s1"), &json!({"status": "queued"})).unwrap();
    store.append_run_event("run1", "RunCreated", &json!({"input_digest": "abc"})).unwrap();
    store.put(Table::Evidence, "ev1", Some("run1"), &json!({"verified": true})).unwrap();
}

// ---- CORE-009 ----

#[test]
fn core009_reopen_restores_everything() {
    let path = temp_db("reopen");
    {
        let store = Store::open(&path).unwrap();
        seed(&store);
        let r2 = store.put_revision("app1", &json!({"v": 2}), Some("h1")).unwrap();
        assert_eq!(r2.revision, 2);
        store.put(Table::Suggestions, "sg1", Some("s1"), &json!({"kind": "reply"})).unwrap();
        store.put(Table::Permissions, "p1", Some("app1"), &json!({"scope": "github:write"})).unwrap();
        store.put(Table::Permissions, "p0", None, &json!({"scope": "global"})).unwrap();
        // 上書き: json は替わり created_at は残る
        store.put(Table::Apps, "app1", None, &json!({"name": "Outreach v2"})).unwrap();
    }
    let store = Store::open(&path).unwrap();
    let app = store.get(Table::Apps, "app1").unwrap().unwrap();
    assert_eq!(app.json, json!({"name": "Outreach v2"}));
    assert!(
        app.created_at.len() >= 20 && app.created_at.contains('T') && app.created_at.ends_with('Z'),
        "RFC3339 ではない: {}",
        app.created_at
    );
    assert_eq!(store.latest_revision("app1").unwrap().unwrap().revision, 2);
    assert_eq!(store.get_revision("app1", 2).unwrap().unwrap().base_hash.as_deref(), Some("h1"));
    assert_eq!(store.list_revisions("app1").unwrap().len(), 2);
    let s = store.get(Table::Sessions, "s1").unwrap().unwrap();
    assert_eq!(s.parent_id.as_deref(), Some("app1"));
    assert_eq!(store.get_ui_state("s1", "layout").unwrap(), Some(json!({"x": 1})));
    let evs = store.list_run_events("run1").unwrap();
    assert_eq!((evs.len(), evs[0].seq, evs[0].kind.as_str()), (1, 1, "RunCreated"));
    assert!(evs[0].recorded_at.ends_with('Z'));
    assert_eq!(store.list(Table::Suggestions, Some("s1")).unwrap().len(), 1);
    assert_eq!(store.list(Table::Permissions, Some("app1")).unwrap().len(), 1);
    assert_eq!(store.list(Table::Permissions, None).unwrap().len(), 2);
    assert_eq!(store.list(Table::Evidence, Some("run1")).unwrap().len(), 1);
    assert!(matches!(store.list(Table::Apps, Some("x")), Err(StoreError::Usage(_))));
}

#[test]
fn core009_migration_once_and_pragmas() {
    let path = temp_db("pragma");
    let a = Store::open(&path).unwrap();
    assert_eq!(a.pragma("journal_mode").unwrap(), "wal");
    assert_eq!(a.pragma("foreign_keys").unwrap(), "1");
    assert_eq!(a.schema_version().unwrap() as usize, storage::migrations::MIGRATIONS.len());
    drop(a);
    // V1 / V2 は CREATE TABLE（IF NOT EXISTS 無し）なので、2 回目の open が通る = 再適用していない
    let b = Store::open(&path).unwrap();
    assert_eq!(b.schema_version().unwrap() as usize, storage::migrations::MIGRATIONS.len());
    assert_eq!(b.pragma("foreign_keys").unwrap(), "1");
    let m = Store::open_in_memory().unwrap();
    assert_eq!(m.pragma("foreign_keys").unwrap(), "1");
    assert_eq!(m.schema_version().unwrap() as usize, storage::migrations::MIGRATIONS.len());
}

#[test]
fn core009_no_leak_across_sessions() {
    let store = Store::open_in_memory().unwrap();
    store.put(Table::Apps, "app1", None, &json!({})).unwrap();
    store.put(Table::Apps, "app2", None, &json!({})).unwrap();
    store.put(Table::Sessions, "s1", Some("app1"), &json!({})).unwrap();
    store.put(Table::Sessions, "s2", Some("app1"), &json!({})).unwrap();
    store.put(Table::Sessions, "s3", Some("app2"), &json!({})).unwrap();
    store.put_ui_state("s1", "layout", &json!({"who": "s1"})).unwrap();
    store.put_ui_state("s2", "layout", &json!({"who": "s2"})).unwrap();
    store.put(Table::Runs, "run1", Some("s1"), &json!({})).unwrap();
    store.put(Table::Runs, "run2", Some("s2"), &json!({})).unwrap();
    assert_eq!(store.list_ui_state("s2").unwrap(), vec![("layout".to_string(), json!({"who": "s2"}))]);
    assert_eq!(store.get_ui_state("s3", "layout").unwrap(), None);
    let ids = |t, p| store.list(t, p).unwrap().into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(ids(Table::Runs, Some("s1")), vec!["run1"]);
    assert_eq!(ids(Table::Sessions, Some("app2")), vec!["s3"]);
    assert_eq!(ids(Table::Sessions, None).len(), 3);
}

#[test]
fn core009_foreign_keys_reject_orphans_and_cascade() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.put(Table::Runs, "r", Some("no-such-session"), &json!({})).is_err());
    assert!(store.append_run_event("no-such-run", "X", &json!({})).is_err());
    assert!(store.put(Table::Sessions, "s", None, &json!({})).is_err(), "sessions.app_id は NOT NULL");
    seed(&store);
    assert!(store.delete(Table::Apps, "app1").unwrap());
    assert!(store.get(Table::Sessions, "s1").unwrap().is_none());
    assert!(store.get(Table::Runs, "run1").unwrap().is_none());
    assert!(store.get(Table::Evidence, "ev1").unwrap().is_none());
    assert!(store.list_run_events("run1").unwrap().is_empty());
    assert!(store.list_ui_state("s1").unwrap().is_empty());
    assert!(store.list_revisions("app1").unwrap().is_empty());
    assert!(!store.delete(Table::Apps, "app1").unwrap());
}

// ---- CORE-010 ----

#[test]
fn core010_token_never_reaches_db_bytes() {
    let token = fake_github_token();
    let path = temp_db("secret");
    let secrets = SecretStore::memory();
    secrets.put("github.token", &token).unwrap();
    {
        let store = Store::open(&path).unwrap();
        store.put(Table::Apps, "app1", None, &json!({})).unwrap();
        store
            .put(Table::Permissions, "p1", Some("app1"), &json!({"provider": "github", "secret_ref": "github.token"}))
            .unwrap();
        store.put(Table::Sessions, "s1", Some("app1"), &json!({})).unwrap();
        store.put(Table::Runs, "run1", Some("s1"), &json!({})).unwrap();
        // 統合層が誤って payload に token を入れても run_events は redact して保存する
        let ev = store
            .append_run_event(
                "run1",
                "ActionDispatched",
                &json!({"headers": {"Authorization": format!("Bearer {token}")}, "token": token, "note": format!("token={token}")}),
            )
            .unwrap();
        let saved = ev.payload.to_string();
        assert!(!saved.contains(&token), "{saved}");
        // secret 系の key は値を丸ごと伏せる（"Bearer" の scheme も残さない。log の field と同じ規則）
        assert_eq!(ev.payload["headers"]["Authorization"], "[redacted]");
        assert_eq!(ev.payload["token"], "[redacted]");
        assert_eq!(ev.payload["note"], "token=[redacted]");
    }
    // DB file（WAL / shm も）を bytes で見る。secret_ref は在る（= 本当に読めている）、値は無い
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", path.display()));
        if let Ok(b) = std::fs::read(&p) {
            bytes.extend_from_slice(&b);
        }
    }
    let haystack = String::from_utf8_lossy(&bytes);
    assert!(haystack.contains("github.token"), "DB を読めていない");
    assert!(!haystack.contains(&token), "token の値が DB に残っている");
    assert_eq!(secrets.get("github.token").unwrap().as_deref(), Some(token.as_str()));
}

#[test]
fn core010_memory_store_roundtrip_and_key_rules() {
    let s = SecretStore::memory();
    assert!(s.is_memory());
    assert_eq!(s.get("k").unwrap(), None);
    s.put("k", "v").unwrap();
    s.put("k", "v2").unwrap();
    assert_eq!(s.get("k").unwrap().as_deref(), Some("v2"));
    assert!(s.delete("k").unwrap());
    assert!(!s.delete("k").unwrap());
    assert!(matches!(s.put("bad key", "v"), Err(SecretError::BadKey(_))));
    assert!(matches!(s.get(""), Err(SecretError::BadKey(_))));
    assert!(s.put("provider:google/refresh_token@a.b-c", "ok").is_ok());
    // brief の差し替え口: env KANGAE_SECRET_STORE=memory で from_env が in-memory になる
    std::env::set_var("KANGAE_SECRET_STORE", "memory");
    assert!(SecretStore::from_env().is_memory());
}

/// 本物の Keychain に probe-<pid> を書いて読んで消す。operator が手で:
/// `cargo test --test storage -- --ignored keychain`
#[test]
#[ignore = "本物の Keychain に書く（service Kangae / account probe-<pid>。最後に消す）"]
fn keychain_roundtrip_real() {
    let s = SecretStore::keychain(KEYCHAIN_SERVICE);
    let key = format!("probe-{}", std::process::id());
    assert_eq!(s.get(&key).unwrap(), None);
    s.put(&key, "probe-value-1").unwrap();
    assert_eq!(s.get(&key).unwrap().as_deref(), Some("probe-value-1"));
    let tricky = "probe value 2 with spaces \"quotes\" 'and' $ymbols 日本語";
    s.put(&key, tricky).unwrap();
    assert_eq!(s.get(&key).unwrap().as_deref(), Some(tricky));
    assert!(s.delete(&key).unwrap());
    assert_eq!(s.get(&key).unwrap(), None);
    assert!(!s.delete(&key).unwrap());
}

// ---- CORE-011 ----

#[test]
fn core011_redelivery_does_not_duplicate() {
    let mut bus = EventBus::new(100);
    let count = Arc::new(Mutex::new(0));
    let c = count.clone();
    bus.subscribe("run.", move |e| {
        if e.topic == "run.completed" {
            *c.lock().unwrap() += 1;
        }
    });
    let first = bus.publish_with_id("evt-1".into(), "run.completed", json!({"run_id": "run1"}), Some("cid-1"));
    let again = bus.publish_with_id("evt-1".into(), "run.completed", json!({"run_id": "run1"}), Some("cid-1"));
    assert_eq!(first, Publish::Delivered { seq: 1, listeners: 1 });
    assert_eq!(again, Publish::Duplicate);
    assert_eq!(*count.lock().unwrap(), 1);
}

#[test]
fn core011_order_window_prefix_and_listener_lifecycle() {
    let mut bus = EventBus::new(2);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let id = bus.subscribe("", move |e| s.lock().unwrap().push((e.seq, e.topic.clone(), e.cid.clone())));
    bus.publish("a", json!(1), Some("c1"));
    bus.publish("b", json!(2), None);
    bus.publish("c", json!(3), None);
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![1, 2, 3], "seq は単調");
    assert_eq!((got[0].1.as_str(), got[0].2.as_deref()), ("a", Some("c1")));
    // 窓 2: x は 2 つ後に忘れられる（有界の代償。仕様）
    assert!(matches!(bus.publish_with_id("x".into(), "t", json!(0), None), Publish::Delivered { .. }));
    assert_eq!(bus.publish_with_id("x".into(), "t", json!(0), None), Publish::Duplicate);
    bus.publish_with_id("y".into(), "t", json!(0), None);
    bus.publish_with_id("z".into(), "t", json!(0), None);
    assert!(matches!(bus.publish_with_id("x".into(), "t", json!(0), None), Publish::Delivered { .. }));
    // prefix: "run." の listener は "ui.click" を受けない
    let n = Arc::new(Mutex::new(0));
    let n2 = n.clone();
    let run_id = bus.subscribe("run.", move |_| *n2.lock().unwrap() += 1);
    assert_eq!(bus.publish("ui.click", json!(0), None), Publish::Delivered { seq: 8, listeners: 1 });
    assert_eq!(bus.publish("run.started", json!(0), None), Publish::Delivered { seq: 9, listeners: 2 });
    assert_eq!(*n.lock().unwrap(), 1);
    // lifecycle
    assert!(bus.unsubscribe(id));
    assert!(!bus.unsubscribe(id));
    let before = seen.lock().unwrap().len();
    bus.publish("after", json!(0), None);
    assert_eq!(seen.lock().unwrap().len(), before);
    assert!(bus.unsubscribe(run_id));
    assert_eq!(bus.listener_count(), 0);
}

// ---- CORE-012 ----

#[test]
fn core012_log_carries_cid_and_hides_secret_and_content() {
    let token = fake_github_token();
    let (log, buf) = Logger::capture();
    let cid = new_id();
    log.info(&cid, "proposal.created", &[("proposal_id", "pr1"), ("prompt", "お客さまへの返信を書いて")]);
    log.info(
        &cid,
        "run.dispatched",
        &[("run_id", "run1"), ("authorization", &format!("Bearer {token}")), ("note", &format!("token={token} ok"))],
    );
    log.warn(&cid, "verification.unverified", &[("reason", "no observation")]);
    log.debug(&cid, "dropped", &[]); // 既定 min は Info
    let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 3, "{text}");
    for l in &lines {
        assert_eq!(l["cid"], cid);
        assert!(l["ts"].is_u64());
    }
    assert_eq!(lines[0]["event"], "proposal.created");
    assert_eq!(lines[1]["authorization"], "[redacted]");
    assert_eq!(lines[1]["note"], "token=[redacted] ok");
    assert_eq!(lines[2]["level"], "warn");
    assert!(!text.contains(&token), "{text}");
    assert!(!text.contains("お客さま"), "本文が既定で出ている: {text}");
    assert!(lines[0]["prompt"].as_str().unwrap().starts_with("[content "));
    // show_content(true) なら本文は出るが secret は出ない
    let (log, buf) = Logger::capture();
    let log = log.show_content(true).min_level(Level::Debug);
    log.debug(&cid, "x", &[("prompt", "本文"), ("api_key", &token)]);
    let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(text.contains("本文") && !text.contains(&token), "{text}");
}

#[test]
fn core012_redact_patterns() {
    let t = fake_github_token();
    assert_eq!(redact(&format!("push with {t} done")), "push with [redacted] done");
    assert_eq!(redact("Authorization: Bearer abc.def"), "Authorization: Bearer [redacted]");
    assert_eq!(redact("Bearer abc"), "Bearer [redacted]");
    assert_eq!(redact("password=hunter2 user=bob"), "password=[redacted] user=bob");
    assert_eq!(redact(r#"{"api_key":"k123","name":"x"}"#), r#"{"api_key":"[redacted]","name":"x"}"#);
    assert_eq!(redact("plain text, no secrets 2026-10-16 https://api.github.com/repos"), "plain text, no secrets 2026-10-16 https://api.github.com/repos");
    let sk = format!("sk-{}", "b".repeat(40));
    assert_eq!(redact(&format!("key {sk}")), "key [redacted]");
    let jwt = format!("eyJ{}.eyJ{}.{}", "a".repeat(20), "b".repeat(20), "c".repeat(20));
    assert_eq!(redact(&jwt), "[redacted]");
    assert_eq!(redact("ghp_short"), "ghp_short", "短い語は token と見なさない");
    // W19-3: 秘密鍵の PEM の塊（本文は既知の prefix を持たない base64）・END が無ければ尻まで・PRIVATE KEY でない PEM は残す
    let body = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmU";
    assert_eq!(redact(&format!("a -----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n-----END OPENSSH PRIVATE KEY----- b")), "a [redacted] b");
    assert_eq!(redact(&format!("-----BEGIN RSA PRIVATE KEY-----\n{body}")), "[redacted]");
    assert_eq!(redact("-----BEGIN CERTIFICATE-----\nMIIB\n"), "-----BEGIN CERTIFICATE-----\nMIIB\n");
    let v = redact_value(&json!({
        "token": "abc",
        "nested": {"client_secret": {"x": 1}, "ok": "fine"},
        "list": [format!("Bearer {t}"), 1, null]
    }));
    assert_eq!(
        v,
        json!({"token": "[redacted]", "nested": {"client_secret": "[redacted]", "ok": "fine"}, "list": ["Bearer [redacted]", 1, null]})
    );
}

/// W2c: surface_receipts は追記だけで、surface ごとに引け、project を消すと消える。
#[test]
fn w2c_surface_receipts_append_and_list_by_surface() {
    let path = temp_db("w2c-receipts");
    let store = Store::open(&path).unwrap();
    store.put(Table::Projects, "project.x", None, &json!({"id": "project.x"})).unwrap();
    let r1 = json!({"surface_id": "docs", "state": "unverified"});
    let a = store.put_surface_receipt("project.x", "docs", "page.docs", "native_app", "unverified", &r1).unwrap();
    let b = store.put_surface_receipt("project.x", "diagram", "page.diagram", "web", "unverified", &json!({"n": 2})).unwrap();
    let c = store.put_surface_receipt("project.x", "docs", "page.docs", "native_app", "verified", &json!({"n": 3})).unwrap();
    assert!(a.seq < b.seq && b.seq < c.seq);
    assert_eq!((a.project_id.as_str(), a.surface_id.as_str(), a.page_id.as_str(), a.kind.as_str(), a.state.as_str()), ("project.x", "docs", "page.docs", "native_app", "unverified"));
    assert_eq!(a.json, r1);
    assert!(a.recorded_at.ends_with('Z'), "{}", a.recorded_at);
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.list_surface_receipts("project.x", None).unwrap(), vec![a.clone(), b, c.clone()]);
    assert_eq!(store.list_surface_receipts("project.x", Some("docs")).unwrap(), vec![a, c]);
    assert!(store.list_surface_receipts("project.other", None).unwrap().is_empty());
    // 親の無い project・表に無い kind / state は DB が拒否
    assert!(store.put_surface_receipt("project.none", "docs", "p", "native_app", "verified", &json!({})).is_err());
    assert!(store.put_surface_receipt("project.x", "docs", "p", "desktop", "verified", &json!({})).is_err());
    assert!(store.put_surface_receipt("project.x", "docs", "p", "web", "connected", &json!({})).is_err());
    store.delete(Table::Projects, "project.x").unwrap();
    assert!(store.list_surface_receipts("project.x", None).unwrap().is_empty(), "project と一緒に消える");
}

/// W11: run_receipts は追記だけ・新しい順・同じ run_id は拒否。project の FK は無い（包んだだけの古い app）。
#[test]
fn w11_run_receipts_append_newest_first_and_reject_duplicate_id() {
    let path = temp_db("w11-receipts");
    let store = Store::open(&path).unwrap();
    let r1 = json!({"run_id": "run.a", "exit_code": 1});
    let a = store.put_run_receipt("run.a", "project.x", "dev", &r1).unwrap();
    let b = store.put_run_receipt("run.b", "project.x", "dev", &json!({"n": 2})).unwrap();
    let c = store.put_run_receipt("run.c", "project.x", "other", &json!({"n": 3})).unwrap();
    assert_eq!((a.run_id.as_str(), a.project_id.as_str(), a.session_id.as_str()), ("run.a", "project.x", "dev"));
    assert_eq!(a.json, r1);
    assert!(a.recorded_at.ends_with('Z'), "{}", a.recorded_at);
    assert!(store.put_run_receipt("run.a", "project.x", "dev", &json!({"again": true})).is_err(), "同じ run_id は拒否");
    drop(store);
    let store = Store::open(&path).unwrap();
    let all = store.list_run_receipts("project.x", None).unwrap();
    assert_eq!(all.iter().map(|r| r.run_id.as_str()).collect::<Vec<_>>(), vec!["run.c", "run.b", "run.a"]);
    assert_eq!(store.list_run_receipts("project.x", Some("dev")).unwrap().iter().map(|r| r.run_id.as_str()).collect::<Vec<_>>(), vec!["run.b", "run.a"]);
    assert!(store.list_run_receipts("project.other", None).unwrap().is_empty());
    let _ = (b, c);
}

/// W2c: この binary より新しい版で保存された DB は、何も当てずに拒否し「saved by a newer Shibubu」と出す（FX1 の反証の直しで英語に）。
#[test]
fn w2c_newer_schema_is_refused_without_touching_the_db() {
    let path = temp_db("w2c-newer");
    drop(Store::open(&path).unwrap());
    let supported = Store::open(&path).unwrap().schema_version().unwrap();
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute("INSERT INTO schema_migrations(version) VALUES (?1)", [supported + 1]).unwrap();
    }
    let e = Store::open(&path).err().expect("新しい版の DB を開いてしまった");
    assert!(matches!(e, StoreError::NewerSchema { found, supported: s } if found == supported + 1 && s == supported), "{e:?}");
    assert!(e.to_string().contains("saved by a newer Shibubu"), "{e}");
    let c = rusqlite::Connection::open(&path).unwrap();
    let max: i64 = c.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0)).unwrap();
    assert_eq!(max, supported + 1, "拒否した DB を書き換えた");
}

/// W9: Terminal の `shibubu project …` が読むだけで開く口。無い DB の file・dir を作らない・migration を当てない・行を書かない・主 file を変えない・新しい版は拒否。
/// Shibubu が開いている間（WAL の -wal / -shm が在る）と、閉じた後（最後の接続が -wal / -shm を消した）の両方で読める。
/// 閉じた後に読むと -shm と空の -wal は残る（SQLite の WAL の付属 file・`Store::open_read_only` の保証しない）。
#[test]
fn w9_read_only_reads_without_changing_the_db() {
    let missing = temp_db("w9-missing");
    let e = Store::open_read_only(&missing).err().expect("無い file を開いた");
    assert!(e.to_string().contains("がありません"), "{e}");
    assert!(!missing.parent().unwrap().exists(), "読むだけの口が dir を作った");

    let path = temp_db("w9-ro");
    let w = Store::open(&path).unwrap();
    w.put(Table::Projects, "project.a", None, &json!({ "n": 1 })).unwrap();
    let ro = Store::open_read_only(&path).unwrap();
    assert_eq!(ro.get(Table::Projects, "project.a").unwrap().unwrap().json, json!({ "n": 1 }), "Shibubu が開いている間に読める");
    w.put(Table::Projects, "project.a", None, &json!({ "n": 2 })).unwrap();
    assert_eq!(ro.get(Table::Projects, "project.a").unwrap().unwrap().json, json!({ "n": 2 }), "開いた後の書き込みも次の読みで出る");
    assert!(ro.put(Table::Projects, "project.b", None, &json!({})).is_err(), "読むだけの store に書けた");
    drop((ro, w));
    let before = std::fs::read(&path).unwrap();
    let wal = path.with_file_name("kangae.sqlite3-wal");
    assert!(!wal.exists(), "前提: 閉じると -wal は消える");
    let ro = Store::open_read_only(&path).unwrap();
    assert_eq!(ro.get(Table::Projects, "project.a").unwrap().unwrap().json, json!({ "n": 2 }), "Shibubu を閉じた後も読める");
    assert!(ro.get(Table::Projects, "project.b").unwrap().is_none());
    drop(ro);
    assert_eq!(std::fs::read(&path).unwrap(), before, "読むだけで DB の file が変わった");
    // ここで -wal / -shm が無い事は見ない: 誰も開いていない DB を読むと SQLite は WAL の索引 -shm と空の -wal を作って残す
    // （読むだけの接続は消せない・反証 W9 で `!wal.exists() && !shm.exists()` を当てて赤を見た）。約束は主 file と行が変わらない事（上の 2 つ）

    // migration を当てない: 表の無い（Shibubu が一度も開いていない）SQLite は開けるが、schema_migrations を作らない
    let bare = temp_db("w9-bare");
    std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
    rusqlite::Connection::open(&bare).unwrap().execute_batch("CREATE TABLE other(x)").unwrap();
    let ro = Store::open_read_only(&bare).unwrap();
    assert!(ro.get(Table::Projects, "project.a").is_err(), "projects の表は無いまま（作っていない）");
    drop(ro);
    let c = rusqlite::Connection::open(&bare).unwrap();
    let made: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='schema_migrations')", [], |r| r.get(0)).unwrap();
    assert!(!made, "読むだけの口が migration を当てた");
}

#[test]
fn w9_read_only_refuses_a_newer_schema() {
    let path = temp_db("w9-newer");
    let supported = Store::open(&path).unwrap().schema_version().unwrap();
    rusqlite::Connection::open(&path).unwrap().execute("INSERT INTO schema_migrations(version) VALUES (?1)", [supported + 1]).unwrap();
    let e = Store::open_read_only(&path).err().expect("新しい版の DB を読んだ");
    assert!(matches!(e, StoreError::NewerSchema { found, supported: s } if found == supported + 1 && s == supported), "{e:?}");
}

/// RQ3 interface（caller の形・本体の前は todo!() で赤 = 設計通り）: `Back up now` の写しは別の file で開け、同じ行を読める・元の DB は変わらない・
/// 既に在る写しの path には上書きしない。壊れた file（DB でない中身）を開いた誤りは `is_damaged`、lock の誤りは違う。
/// 負の対照（本体の commit で取る）: `back_up_to` の `VACUUM INTO` を空の file の作成に替える → 読み戻しが赤。
#[test]
fn rq3_back_up_to_writes_a_copy_that_opens_and_damage_is_told_apart() {
    let path = temp_db("rq3-backup");
    let store = Store::open(&path).unwrap();
    store.put(Table::Projects, "project.a", None, &json!({"name": "a"})).unwrap();
    let copy = path.parent().unwrap().join("backups").join("20261028T000000Z-manual").join("kangae.sqlite3");
    store.back_up_to(&copy).unwrap();
    assert!(store.back_up_to(&copy).is_err(), "在る写しを上書きした");
    let back = Store::open(&copy).unwrap();
    assert_eq!(back.get(Table::Projects, "project.a").unwrap().map(|r| r.json), Some(json!({"name": "a"})));
    let bad = path.parent().unwrap().join("bad.sqlite3");
    std::fs::write(&bad, b"this is not a database at all, just text that is long enough").unwrap();
    let err = Store::open(&bad).err().expect("DB でない file を開けた");
    assert!(err.is_damaged(), "{err}");
    assert!(!StoreError::Usage("x".into()).is_damaged());
}

/// RQ3（RQ-011・migration の前の写し）: 版の後ろの DB を開くと、当てる前に `before_update` へ写し、当てに失敗したら写しを元の path に戻して Err（前の版の Shibubu で開ける）。
/// 新しい DB と最新の DB は写さない。負の対照（commit message）: 失敗の腕の `std::fs::copy(before_update, path)` を外す → 版の assert が赤。
#[test]
fn rq3_open_backing_up_copies_before_a_migration_and_puts_it_back_on_failure() {
    let path = temp_db("rq3-before-update");
    let dir = path.parent().unwrap().to_path_buf();
    let copy = |n: &str| dir.join("backups").join(n).join("kangae.sqlite3");
    let store = Store::open_backing_up(&path, &copy("new")).unwrap();
    assert!(!copy("new").exists(), "新しい DB を写した");
    store.put(Table::Projects, "project.a", None, &json!({"name": "a"})).unwrap();
    let latest = store.schema_version().unwrap();
    drop(store);
    drop(Store::open_backing_up(&path, &copy("same")).unwrap());
    assert!(!copy("same").exists(), "最新の DB を写した");
    // 2 つ前の版の DB の形を作る: 最後から 2 つ目の migration（terminal_sessions）は表ごと消して当て直せる様に・最後の 1 つは印だけ消す
    // = 当て直すと 1 つ目は通って commit され、2 つ目が「表が既に在る」で失敗する（途中まで当たった DB を写しで戻すかを見る）
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(&format!("DROP TABLE terminal_sessions; DELETE FROM schema_migrations WHERE version>={};", latest - 1))
        .unwrap();
    let err = Store::open_backing_up(&path, &copy("old")).err().expect("失敗する migration が通った");
    assert!(!err.is_damaged(), "{err}");
    assert!(copy("old").exists(), "当てる前の写しが無い");
    let c = rusqlite::Connection::open(&path).unwrap();
    let v: i64 = c.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0)).unwrap();
    let t: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='terminal_sessions'", [], |r| r.get(0)).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap();
    assert_eq!((v, t, n), (latest - 2, 0, 1), "途中まで当たった DB を写しで戻していない");
}
