//! docs/examples/v3/01〜04（timer / 在庫 / coding / 研究ノート）の local action を corpus と同じ id で登録する。composer が作る一覧の物
//! （読書記録・家計簿・英単語…）は entity ごとに書かず、app を開いた時に定義から導く（`derive`: `local.<source>.<verb>` の汎用の verb）。
//! 中身は `ui_state(session_id, <source id>)` の JSON。source id = action id の最後の段を除いた物（`local.inventory.add` →
//! `local.inventory`）で、corpus の binding（`source_ref` / `path`）がそのまま読める形: 在庫 → `{items, selected, filter}`、
//! timer → `{settings, running, remaining_ms, started_at_ms, remaining_display}`、03 のメモ → `{body}`。
//! verify は書いた値を読み直して比べる（corpus の verifier `local_state`）。ui.patch は domain の patch::apply。
//! 登録しない corpus の id: `local.inventory.import_csv`（file_ref を中身に解く picker が無い）・`issues.read` / `tests.run`（外部 adapter）。
use serde_json::{json, Value};

use super::domain::action::Effect;
use super::domain::app::AppDefinitionV3;
use super::domain::patch::{apply, AppPatch};
use super::domain::run::{ErrorCode, Outcome};
use super::permission::digest;
use super::registry::{ActionContract, ActionRegistry, Ctx, Handler, UI_PATCH};
use super::storage::{new_id, now_ms, Store, Table};
use super::{internal, now_rfc3339, revision_of};

/// timer の既定の長さ（分）。corpus 01 は既定を書いていない。
pub const DEFAULT_TIMER_MINUTES: u64 = 25;

#[derive(Clone, Copy)]
enum Op {
    TimerToggle,
    TimerReset,
    TimerSettings,
    /// items に 1 件足して selected にする。`required` は空にできない文字列 field。
    Create { required: Option<&'static str>, defaults: fn() -> Value },
    /// `id_key` で引いた 1 件に、入力の残りの field を書く。
    Update(&'static str),
    /// `id_key` で引いた 1 件の bool field を反転。
    Toggle(&'static str, &'static str),
    Delete(&'static str),
    /// items を query で絞って `filter` に結果の id を書く（読取。中身は変えない）。
    Filter(&'static [&'static str]),
    /// ノートと資料（local.imports）を検索して `filter` に書く。
    Search,
    /// `note_id` があればその 1 件の body、無ければ 03 の単一メモの body。
    SaveNote,
    /// 資料（local.imports）に在る物だけを引用に足す（出典の無い引用は作れない）。
    Cite,
    OpenConnection,
    Patch,
}

/// corpus の id 表。effect は corpus の宣言と同じ（違えば invoke が拒否する）。
pub fn register(reg: &mut ActionRegistry) {
    use Effect::{Destructive, LocalRead, LocalWrite};
    let table: [(&'static str, Effect, Op); 17] = [
        ("local.timer.toggle", LocalWrite, Op::TimerToggle),
        ("local.timer.reset", LocalWrite, Op::TimerReset),
        ("local.timer.save_settings", LocalWrite, Op::TimerSettings),
        ("local.inventory.filter", LocalRead, Op::Filter(&["name"])),
        ("local.inventory.add", LocalWrite, Op::Create { required: Some("name"), defaults: || json!({ "quantity": 0 }) }),
        ("local.inventory.update_quantity", LocalWrite, Op::Update("item_id")),
        ("local.inventory.delete", Destructive, Op::Delete("item_id")),
        ("local.tasks.add", LocalWrite, Op::Create { required: Some("title"), defaults: || json!({ "done": false }) }),
        ("local.tasks.toggle_done", LocalWrite, Op::Toggle("task_id", "done")),
        ("local.notes.search", LocalRead, Op::Search),
        ("local.notes.create", LocalWrite, Op::Create { required: None, defaults: || json!({ "title": "無題のノート", "body": "", "citations": [] }) }),
        ("local.notes.save", LocalWrite, Op::SaveNote),
        ("local.notes.cite", LocalWrite, Op::Cite),
        ("local.notes.delete", Destructive, Op::Delete("note_id")),
        ("local.imports.add", LocalWrite, Op::Create { required: Some("file_ref"), defaults: || json!({}) }),
        ("local.connection.open", LocalWrite, Op::OpenConnection),
        (UI_PATCH, LocalWrite, Op::Patch),
    ];
    for (id, effect, op) in table {
        let source = id.rsplit_once('.').map_or(id, |(s, _)| s);
        reg.register(id, ActionContract::local(effect), Local { source: source.into(), op });
    }
}

/// 一覧（`local_collection`）の汎用の verb → 効果と中身。input の形は composer の planner（`input_schema`）と同じ: 1 件は `item_id`、
/// 足すのは欄の値（必須の欄は app の input_schema が invoke の前に検査する）、済みの印は `done`（composer の boolean の欄は `done` だけ）。
/// ここに無い verb（import_csv・search・save …）は導かない = 登録されず、画面では Unsupported のまま。
fn generic(verb: &str) -> Option<(Effect, Op)> {
    use Effect::{Destructive, LocalRead, LocalWrite};
    Some(match verb {
        "add" => (LocalWrite, Op::Create { required: None, defaults: || json!({}) }),
        "filter" => (LocalRead, Op::Filter(&[])),
        "delete" => (Destructive, Op::Delete("item_id")),
        "toggle_done" => (LocalWrite, Op::Toggle("item_id", "done")),
        v if v.len() > "update_".len() && v.starts_with("update_") => (LocalWrite, Op::Update("item_id")),
        _ => return None,
    })
}

/// app の定義の `local.<ns>.<verb>` のうち、`local.<ns>` が定義の `local_collection` の source で verb が汎用の物を登録する
/// （起動時の corpus の登録と、先に導いた同じ id は置き換えない）。効果は registry の表が正（定義の宣言が違えば invoke が拒否する）。足した数を返す。
pub fn derive(reg: &ActionRegistry, def: &AppDefinitionV3) -> usize {
    let mut added = 0;
    for a in &def.actions {
        let Some((source, verb)) = a.id.rsplit_once('.') else { continue };
        let collection = source.starts_with("local.") && def.sources.iter().any(|s| s.id == source && s.kind == "local_collection");
        let Some((effect, op)) = generic(verb).filter(|_| collection) else { continue };
        if reg.register_if_absent(&a.id, ActionContract::local(effect), Local { source: source.into(), op }) {
            added += 1;
        }
    }
    added
}

struct Local {
    source: String,
    op: Op,
}

impl Handler for Local {
    fn execute(&self, ctx: &Ctx, input: &Value) -> Result<Value, ErrorCode> {
        if let Op::Patch = self.op {
            return patch(ctx, input);
        }
        // 読んで・変えて・書くのを 1 回の lock の中で（同じ process の同時の 2 つが互いの変更を上書きしない。ui_state は session の物で、
        // 別 process が同じ session を書く前提は無い。app ごとに共有する revision の ui.patch だけ DB transaction で別 process も止める）
        let store = ctx.store();
        let source = self.source.as_str();
        let mut doc = read(&store, &ctx.session_id, source)?;
        let claim = match self.op {
            Op::TimerToggle | Op::TimerReset | Op::TimerSettings => timer(self.op, &mut doc, input)?,
            Op::Create { required, defaults } => {
                if let Some(field) = required {
                    if text(input, field)?.trim().is_empty() {
                        return Err(ErrorCode::InvalidInput);
                    }
                }
                let now = now_rfc3339();
                let mut item = defaults();
                for (k, v) in input.as_object().into_iter().flatten() {
                    set(&mut item, k, v.clone());
                }
                let id = new_id();
                set(&mut item, "id", json!(id));
                set(&mut item, "created_at", json!(now));
                set(&mut item, "updated_at", json!(now));
                items_mut(&mut doc)?.push(item.clone());
                set(&mut doc, "selected", item.clone());
                claim_item(source, &id, item)
            }
            Op::Update(id_key) => {
                let id = item_id(input, id_key)?.to_string();
                let item = find_mut(&mut doc, &id)?;
                for (k, v) in input.as_object().into_iter().flatten().filter(|(k, _)| k.as_str() != "item_id" && k.as_str() != id_key) {
                    set(item, k, v.clone());
                }
                touched(&mut doc, source, &id)?
            }
            Op::Toggle(id_key, field) => {
                let id = item_id(input, id_key)?.to_string();
                let item = find_mut(&mut doc, &id)?;
                let flipped = !item[field].as_bool().unwrap_or(false);
                set(item, field, json!(flipped));
                touched(&mut doc, source, &id)?
            }
            Op::Delete(id_key) => {
                let id = item_id(input, id_key)?.to_string();
                let items = items_mut(&mut doc)?;
                let at = items.iter().position(|it| it["id"] == id.as_str()).ok_or(ErrorCode::TargetNotFound)?;
                items.remove(at);
                sync_selected(&mut doc, &id, Value::Null);
                claim_item(source, &id, Value::Null)
            }
            Op::Filter(fields) => {
                let raw = input.get("query").and_then(Value::as_str).unwrap_or_default();
                let ids = matching(&doc["items"], fields, raw);
                set(&mut doc, "filter", json!({ "query": raw, "ids": ids }));
                claim_fields(source, &doc, &["filter"])
            }
            Op::Search => {
                let raw = input.get("query").and_then(Value::as_str).unwrap_or_default();
                let imports = read(&store, &ctx.session_id, "local.imports")?;
                let filter = json!({
                    "query": raw,
                    "note_ids": matching(&doc["items"], &["title", "body"], raw),
                    "import_ids": matching(&imports["items"], &["title", "file_ref"], raw),
                });
                set(&mut doc, "filter", filter);
                claim_fields(source, &doc, &["filter"])
            }
            Op::SaveNote => {
                let body = text(input, "body")?.to_string();
                match input.get("note_id").and_then(Value::as_str) {
                    Some(id) => {
                        set(find_mut(&mut doc, id)?, "body", json!(body));
                        touched(&mut doc, source, id)?
                    }
                    None => {
                        set(&mut doc, "body", json!(body));
                        claim_fields(source, &doc, &["body"])
                    }
                }
            }
            Op::Cite => {
                let note_id = text(input, "note_id")?.to_string();
                let import_id = text(input, "import_id")?;
                let imports = read(&store, &ctx.session_id, "local.imports")?;
                if !imports["items"].as_array().is_some_and(|a| a.iter().any(|it| it["id"] == import_id)) {
                    return Err(ErrorCode::TargetNotFound); // 出典の無い引用は作れない
                }
                let mut citation = json!({ "id": new_id(), "import_id": import_id });
                if let Some(quote) = input.get("quote") {
                    set(&mut citation, "quote", quote.clone());
                }
                let note = find_mut(&mut doc, &note_id)?;
                match note.get_mut("citations").and_then(Value::as_array_mut) {
                    Some(list) => list.push(citation),
                    None => set(note, "citations", json!([citation])),
                }
                touched(&mut doc, source, &note_id)?
            }
            Op::OpenConnection => {
                set(&mut doc, "requested", input.get("capability_ref").cloned().unwrap_or(Value::Null));
                set(&mut doc, "requested_at", json!(now_rfc3339()));
                claim_fields(source, &doc, &["requested", "requested_at"])
            }
            Op::Patch => unreachable!("Patch は上で返した"),
        };
        if ctx.cancelled() {
            return Err(ErrorCode::Cancelled); // 書く直前の checkpoint（RUN-008）
        }
        store.put_ui_state(&ctx.session_id, source, &doc).map_err(internal)?;
        Ok(claim)
    }

    fn verify(&self, ctx: &Ctx, _input: &Value, claim: &Value) -> Outcome {
        if let Op::Patch = self.op {
            return verify_patch(ctx, claim);
        }
        let source = claim["source"].as_str().unwrap_or(&self.source);
        let doc = match read(&ctx.store(), &ctx.session_id, source) {
            Ok(doc) => doc,
            Err(error) => return Outcome::Failed { error },
        };
        let observed = match claim["item"].as_str() {
            Some(id) => doc["items"].as_array().and_then(|a| a.iter().find(|it| it["id"] == id)).cloned().unwrap_or(Value::Null),
            None => {
                let keys: Vec<&str> = claim["expect"].as_object().map(|m| m.keys().map(String::as_str).collect()).unwrap_or_default();
                pick(&doc, &keys)
            }
        };
        if observed != claim["expect"] {
            return Outcome::Unverified { reason: format!("{source} を読み直したが書いた値と違う") };
        }
        match ctx.observe(source, json!({ "item": claim["item"], "observed": observed })) {
            Ok(id) => Outcome::Verified { evidence: vec![id] },
            Err(error) => Outcome::Failed { error },
        }
    }
}

/// timer の 3 つ。動いている間の残りは started_at_ms から描く側が計算する（remaining_display は止めた時点の値）。
fn timer(op: Op, doc: &mut Value, input: &Value) -> Result<Value, ErrorCode> {
    let minutes = match op {
        Op::TimerSettings => input.get("duration_minutes").and_then(Value::as_u64).ok_or(ErrorCode::InvalidInput)?,
        _ => doc.pointer("/settings/duration_minutes").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMER_MINUTES),
    };
    // maximum は app 側の schema の宣言で、無い app もある。溢れる長さは入力の誤り（掛けると debug では panic・release では wrap）
    let full = minutes.checked_mul(60_000).ok_or(ErrorCode::InvalidInput)?;
    let now = now_ms();
    let running = doc["running"].as_bool().unwrap_or(false);
    let mut remaining = doc["remaining_ms"].as_u64().unwrap_or(full);
    let started = match op {
        Op::TimerToggle if running => {
            remaining = remaining.saturating_sub(now.saturating_sub(doc["started_at_ms"].as_u64().unwrap_or(now)));
            Value::Null
        }
        Op::TimerToggle => {
            if remaining == 0 {
                remaining = full;
            }
            json!(now)
        }
        Op::TimerReset => {
            remaining = full;
            Value::Null
        }
        _ => {
            if !running {
                remaining = full;
            }
            doc["started_at_ms"].clone()
        }
    };
    let now_running = !started.is_null();
    set(doc, "settings", json!({ "duration_minutes": minutes }));
    set(doc, "running", json!(now_running));
    set(doc, "remaining_ms", json!(remaining));
    set(doc, "started_at_ms", started);
    let secs = remaining.div_ceil(1000);
    set(doc, "remaining_display", json!(format!("{:02}:{:02}", secs / 60, secs % 60)));
    Ok(claim_fields("local.timer", doc, &["settings", "running", "remaining_ms", "started_at_ms", "remaining_display"]))
}

/// 時刻だけで中身が替わる唯一の source（`view` が今の時刻から残りを出し直す）。
pub const TIMER_SOURCE: &str = "local.timer";

/// `view(TIMER_SOURCE, ..)` の `remaining_display` が時刻だけで次に替わるまでの ms（動いている timer の残りの秒の替わり目）。
/// 止まっている・残り 0 は None。描く側はこれで次の frame を頼む（W2b-AX 直し: idle の窓は frame を頼まない・src/app.rs `next_repaint`）。
pub fn next_change_ms(timer_view: &Value) -> Option<u64> {
    let remaining = timer_view["remaining_ms"].as_u64().filter(|&r| r > 0 && timer_view["running"] == json!(true))?;
    Some(match remaining % 1000 {
        0 => 1000,
        r => r,
    })
}

/// 描く側に見せる source の中身。timer は未保存でも既定の長さで出し（押す前から 25:00）、動いている間は
/// `remaining_display` を started_at_ms と今の時刻から出し直す（保存した値は止めた時点の物）。他の source は保存した物そのまま（無ければ `{}`）。
pub fn view(source: &str, doc: Option<Value>, now: u64) -> Value {
    let mut doc = match doc {
        Some(d @ Value::Object(_)) => d,
        _ => json!({}),
    };
    if source != TIMER_SOURCE {
        return doc;
    }
    let minutes = doc.pointer("/settings/duration_minutes").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMER_MINUTES);
    let full = minutes.saturating_mul(60_000);
    let mut remaining = doc["remaining_ms"].as_u64().unwrap_or(full);
    let running = doc["running"].as_bool().unwrap_or(false);
    if running {
        let started = doc["started_at_ms"].as_u64().unwrap_or(now);
        remaining = remaining.saturating_sub(now.saturating_sub(started));
    }
    let secs = remaining.div_ceil(1000);
    set(&mut doc, "settings", json!({ "duration_minutes": minutes }));
    set(&mut doc, "running", json!(running));
    set(&mut doc, "remaining_ms", json!(remaining));
    set(&mut doc, "remaining_display", json!(format!("{:02}:{:02}", secs / 60, secs % 60)));
    doc
}

/// ui.patch: store の最新 revision（正本・CD-09）に AppPatch の列を原子的に当て（domain の patch::apply が validate まで通す）、
/// app_revisions に 1 行と apps の行を書き、共有の revision を入れ替える。lock の順は app → store。
/// 土台は手元の写しにしない: 読む・当てる・書くを 1 回の DB transaction（BEGIN IMMEDIATE）の中で行うので、同じ DB を開いた別の
/// process の patch もこの間は書けず、土台は常に store の最新（process 内の Mutex だけでは別の接続の writer を止められない・RT-R2-2）。
/// stale・cancel・書込の失敗は rollback して何も残さない。base_hash は渡さない（CD-18 (3) で捨てた）。
fn patch(ctx: &Ctx, input: &Value) -> Result<Value, ErrorCode> {
    let ops: Vec<AppPatch> = serde_json::from_value(input.get("operations").cloned().unwrap_or(Value::Null)).map_err(|_| ErrorCode::InvalidInput)?;
    if ops.is_empty() {
        return Err(ErrorCode::InvalidInput);
    }
    let mut app = ctx.app();
    let store = ctx.store();
    let app_id = app.definition().id.clone();
    let (next, json, rev) = store.immediate(internal, |store| {
        let latest = store.latest_revision(&app_id).map_err(internal)?.ok_or(ErrorCode::Internal)?; // open が必ず 1 行置く
        let base = revision_of(latest.json)?;
        if input.get("app_version").and_then(Value::as_u64).is_some_and(|v| v != u64::from(base.version())) {
            return Err(ErrorCode::InvalidInput); // 古い版に向けて作った patch は当てない（CD-18 (3) の stale）
        }
        let next = apply(&base, &ops).map_err(|_| ErrorCode::InvalidInput)?;
        if ctx.cancelled() {
            return Err(ErrorCode::Cancelled);
        }
        let json = serde_json::to_value(next.definition()).map_err(internal)?;
        let rev = store.put_revision(&app_id, &json, None).map_err(internal)?;
        store.put(Table::Apps, &app_id, None, &json).map_err(internal)?;
        Ok((next, json, rev))
    })?;
    let version = next.version();
    *app = next; // commit の後で入れ替える（rollback した patch は見せない）
    Ok(json!({ "source": "app_revisions", "app_id": app_id, "revision": rev.revision, "version": version, "digest": digest(&json) }))
}

/// 書いた revision を番号で引いて読み直し、中身の digest が書いた物と同じなら Verified（後から別の patch が積まれても自分の行は変わらない）。
fn verify_patch(ctx: &Ctx, claim: &Value) -> Outcome {
    let app_id = claim["app_id"].as_str().unwrap_or_default();
    let stored = claim["revision"].as_i64().and_then(|n| ctx.store().get_revision(app_id, n).ok().flatten());
    match stored {
        Some(rev) if claim["digest"] == digest(&rev.json).as_str() => {
            match ctx.observe("app_revisions", json!({ "app_id": app_id, "revision": rev.revision, "version": claim["version"] })) {
                Ok(id) => Outcome::Verified { evidence: vec![id] },
                Err(error) => Outcome::Failed { error },
            }
        }
        _ => Outcome::Unverified { reason: "保存した revision を読み直せない".into() },
    }
}

/// source の JSON（無ければ `{}`）。object でない物が入っていたら上書きせずに止める。
fn read(store: &Store, session_id: &str, source: &str) -> Result<Value, ErrorCode> {
    match store.get_ui_state(session_id, source).map_err(internal)? {
        Some(doc @ Value::Object(_)) => Ok(doc),
        Some(_) => Err(internal(format!("ui_state '{source}' が object でない"))),
        None => Ok(json!({})),
    }
}

fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str, ErrorCode> {
    input.get(key).and_then(Value::as_str).ok_or(ErrorCode::InvalidInput)
}

/// 1 件の id。corpus の key（`note_id` / `task_id`）か、composer の一覧の `item_id`（derive は corpus と同じ id を置き換えないので、
/// 「日記」の `local.notes.delete` は corpus の handler に composer の入力が来る）。
fn item_id<'a>(input: &'a Value, key: &str) -> Result<&'a str, ErrorCode> {
    text(input, key).or_else(|_| text(input, "item_id"))
}

fn set(v: &mut Value, key: &str, value: Value) {
    if let Some(m) = v.as_object_mut() {
        m.insert(key.into(), value);
    }
}

fn items_mut(doc: &mut Value) -> Result<&mut Vec<Value>, ErrorCode> {
    let m = doc.as_object_mut().ok_or(ErrorCode::Internal)?;
    m.entry("items").or_insert_with(|| json!([])).as_array_mut().ok_or(ErrorCode::Internal)
}

fn find_mut<'a>(doc: &'a mut Value, id: &str) -> Result<&'a mut Value, ErrorCode> {
    items_mut(doc)?.iter_mut().find(|it| it["id"] == id).ok_or(ErrorCode::TargetNotFound)
}

/// 書き換えた 1 件の updated_at を進め、selected が同じ物なら写し、claim を作る。
fn touched(doc: &mut Value, source: &str, id: &str) -> Result<Value, ErrorCode> {
    let item = find_mut(doc, id)?;
    set(item, "updated_at", json!(now_rfc3339()));
    let item = item.clone();
    sync_selected(doc, id, item.clone());
    Ok(claim_item(source, id, item))
}

/// selected は選んだ 1 件の写し（binding `selected.body` がそのまま読める）。元が変われば写し直し、消えれば null。
fn sync_selected(doc: &mut Value, id: &str, item: Value) {
    if doc["selected"]["id"] == id {
        set(doc, "selected", item);
    }
}

/// query（大文字小文字を区別しない）を fields のどれかに含む item の id。空の query は全部。
fn matching(items: &Value, fields: &[&str], raw: &str) -> Vec<Value> {
    let q = raw.to_lowercase();
    items
        .as_array()
        .into_iter()
        .flatten()
        // fields が空 = 文字の欄の全部（composer の一覧は欄の名前を runtime に渡さない）。Create が付ける id と時刻は人の書いた欄ではないので外す
        .filter(|it| {
            let hit = |s: &str| s.to_lowercase().contains(&q);
            q.is_empty()
                || if fields.is_empty() {
                    it.as_object().is_some_and(|m| m.iter().any(|(k, v)| !matches!(k.as_str(), "id" | "created_at" | "updated_at") && v.as_str().is_some_and(hit)))
                } else {
                    fields.iter().any(|f| it[*f].as_str().is_some_and(hit))
                }
        })
        .map(|it| it["id"].clone())
        .collect()
}

fn pick(doc: &Value, keys: &[&str]) -> Value {
    Value::Object(keys.iter().map(|k| (k.to_string(), doc.get(*k).cloned().unwrap_or(Value::Null))).collect())
}

/// verify に渡す「書いたはずの値」: 1 件（id で引く。null = 無いはず）か、doc の一部の key。
fn claim_item(source: &str, id: &str, item: Value) -> Value {
    json!({ "source": source, "item": id, "expect": item })
}

fn claim_fields(source: &str, doc: &Value, keys: &[&str]) -> Value {
    json!({ "source": source, "expect": pick(doc, keys) })
}
