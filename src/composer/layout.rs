//! Layout（GEN-006）: DESIGN §7「data shape + job + 頻度 → component」で UiNode の木を組む。pattern の表は無い（CD-11 規則 2）。
//!
//! - 主役の component は job と欄の形で決まる: 計る → text(timer)、整理 → 欄が 3 つ以上なら table・少なければ list、
//!   記録 → 入力 form を上に常設（頻度が高い）＋ timeline、見える化 → chart（job が正当化する唯一の chart）＋ 同じ数字の table、
//!   計画 → calendar、覚える → 1 枚の detail ＋ 覚えた button（一覧は別 tab）、把握 → 状態で束ねた timeline、書く → editor、
//!   会話 → chat、端末 → source_surface(mirror)、動画 → source_surface(handoff)、録音 → recorder ＋ waveform。
//! - 欄の性質で足す: 作った後に変える欄 → 選択時の form、本文（multiline）→ editor、boolean → 済みにする button。
//! - CD-11 規則 1: 検索は主役が list / table で利用者が増やす collection の時だけ画面に出す（action は常に在る）。
//! - DESIGN §3: KPI 0・card 0・主操作は region に 1 つ。split は主役に 60% 以上。座標・色・px は書かない。
//! - README 規則 4・5（V17）: ready でない action を指す node は states.disabled、bind した一覧は states.empty、
//!   未接続の source に bind した node は states.requires_connection（`finish` が最後に全 node を見る）。
//! - W13c 配置の型（29 通目）: 部品を決めた**後**に、型が在れば**置き方だけ**型に従う（`place`）。部品の選び方・数は変えない（固定 template に戻さない）。
//!   型の区画（main / side / top / bottom / drawer）に、部品の種類（`PartKind`）で振り分ける。型が無い・その Page の型が無い時は今の並び。
//!   どの区画も受けない部品は落とさず main へ。型は data（`assets/templates/*.json`・schema は docs/schemas/placement_template_v1.schema.json）で、code に app の名前を書かない。図: docs/diagrams.md §34。
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::domain::action::{ActionDefinition, ActionReadiness, Effect};
use super::domain::binding::{DataBinding, SourceReadiness};
use super::domain::project::{PageDefinition, ProjectDefinitionV1};
use super::domain::ui::UiNode;
use super::goal::{Cadence, EntityKind, ExternalRole, FieldKind, FieldSpec, Job, Reading, UiIntent};
use super::planner::{Blueprint, Planned, PlannedExternal};

pub struct Scene {
    pub root: UiNode,
    /// node key → その部品が在る理由（GEN-010。chart は理由が無いと validate_generated が落とす: V18）
    pub rationale: Vec<(String, String)>,
    pub intents: Vec<UiIntent>,
}

/// bind した時に states.empty が要る component（README 規則 5）。
pub const COLLECTIONS: &[&str] = &["list", "table", "calendar", "timeline", "chat", "detail"];

/// 利用者に見せる「押せない理由」（DESIGN §4.1: disabled with explanation）。
pub fn disabled_reason(a: &ActionDefinition, label: &str) -> String {
    match (a.readiness, a.effect, a.capability_ref.as_deref()) {
        (ActionReadiness::Unsupported, _, _) => format!("{label} isn't available on this Mac"),
        (_, _, Some("local.microphone")) => "Can't record until you allow the microphone. Allow it in System Settings".into(),
        (_, _, Some("editor.open")) => "Can't open until you choose an editor. You edit in that app".into(),
        (_, Effect::ExternalSend, _) => "Connect first, then check the recipient and text and approve each one to send. Nothing sends until you connect".into(),
        (_, Effect::ExternalWrite, _) if a.requires_approval => format!("{label} needs your approval each time, in the app you connected. Unavailable until you connect"),
        (_, Effect::ExternalRead, _) => format!("{label} needs a source to be connected first. Even then it only reads"),
        _ => format!("{label} is unavailable until you connect"),
    }
}

fn node(key: &str, component: &str) -> UiNode {
    UiNode { key: key.into(), component: component.into(), ..Default::default() }
}

fn set(n: &mut UiNode, k: &str, v: Value) {
    n.props.insert(k.into(), v);
}

fn state(n: &mut UiNode, k: &str, text: String) {
    n.states.get_or_insert_with(Map::new).insert(k.into(), Value::String(text));
}

fn has_state(n: &UiNode, k: &str) -> bool {
    n.states.as_ref().and_then(|s| s.get(k)).and_then(Value::as_str).map(|t| !t.is_empty()).unwrap_or(false)
}

fn key(id: &str, part: &str) -> String {
    format!("{}-{part}", id.replace('_', "-"))
}

fn field_json(f: &FieldSpec) -> Value {
    let mut m = json!({ "name": f.name, "label": f.label, "input": f.input() });
    if !f.options.is_empty() {
        m["options"] = json!(f.options);
    }
    m
}

/// 一覧に並べる欄: 見出し → 本文と boolean 以外。
///
/// 契約（W2b・読書記録の一覧に著者が出ない）: entity の一覧（list・table・timeline）は、この欄の名前を `props.columns` として持つ。
/// 先頭が題名、2 つ目以降が題名以外の欄（例: 読書記録 → ["title", "author", "status", "finished_on"]）。
/// renderer はそれを出す（list・timeline は `components.rs` の `list_meta` で題名の後ろに、table は列に）。別の prop 名（`fields` 等）にしない。
fn columns(p: &Planned) -> Vec<&FieldSpec> {
    let e = &p.entity;
    let mut cols: Vec<&FieldSpec> = e.fields.iter().filter(|f| f.title).collect();
    cols.extend(e.fields.iter().filter(|f| !f.title && f.kind != FieldKind::Multiline && f.kind != FieldKind::Boolean));
    cols
}

fn entry_fields(p: &Planned) -> Vec<&FieldSpec> {
    p.entity.fields.iter().filter(|f| !f.auto && f.kind != FieldKind::Multiline && f.kind != FieldKind::Boolean).collect()
}

fn empty_text(label: &str, start: &str) -> String {
    format!("No {label} yet. Start with “{start}”")
}

struct Ui<'a> {
    bp: &'a Blueprint,
    r: &'a Reading,
    why: Vec<(String, String)>,
    intents: Vec<UiIntent>,
}

impl<'a> Ui<'a> {
    fn note(&mut self, key: &str, why: impl Into<String>) {
        self.why.push((key.into(), why.into()));
    }

    fn intent(&mut self, key: &str, purpose: String, primary: bool, hint: &str) {
        self.intents.push(UiIntent { id: key.replace('-', "_"), purpose, primary, representation_hint: Some(hint.into()) });
    }

    fn button(&mut self, key: &str, label: &str, variant: &str, action: &str, why: &str) -> UiNode {
        let mut n = node(key, "button");
        n.label = Some(label.into());
        set(&mut n, "variant", json!(variant));
        n.action_ref = Some(action.into());
        self.note(key, why);
        n
    }

    /// planned の verb が在る時だけ button。
    fn verb_button(&mut self, p: &Planned, verb: &str, part: &str, label: &str, variant: &str) -> Option<UiNode> {
        let id = p.action(verb)?;
        let why = format!("{}に「{label}」の操作がある（action {id}）", p.entity.label);
        Some(self.button(&key(&p.entity.id, part), label, variant, id, &why))
    }

    fn bound(&mut self, key: &str, component: &str, label: &str, source: &str, path: &str, empty: Option<String>, why: &str) -> UiNode {
        let mut n = node(key, component);
        n.label = Some(label.into());
        n.binding = Some(DataBinding { source_ref: source.into(), path: path.into(), extra: Map::new() });
        if let Some(e) = empty {
            state(&mut n, "empty", e);
        }
        self.note(key, why);
        n
    }

    fn form(&mut self, key: &str, label: &str, fields: &[&FieldSpec], source: &str, path: &str, action: &str, why: &str) -> UiNode {
        let mut n = node(key, "form");
        n.label = Some(label.into());
        set(&mut n, "fields", Value::Array(fields.iter().map(|f| field_json(f)).collect()));
        n.binding = Some(DataBinding { source_ref: source.into(), path: path.into(), extra: Map::new() });
        n.action_ref = Some(action.into());
        self.note(key, why);
        n
    }

    fn group(&mut self, key: &str, component: &str, role: Option<&str>, label: Option<&str>, children: Vec<UiNode>, why: &str) -> UiNode {
        let mut n = node(key, component);
        n.role = role.map(Into::into);
        n.label = label.map(Into::into);
        n.children = children;
        self.note(key, why);
        n
    }

    fn split(&mut self, key: &str, ratio: &[f64], children: Vec<UiNode>) -> UiNode {
        let mut n = node(key, "split");
        n.layout = Some([("ratio".to_string(), json!(ratio))].into_iter().collect());
        let main = ratio.iter().cloned().fold(0.0, f64::max);
        n.children = children;
        self.note(key, format!("主役に {}% を渡す split（DESIGN §7: main focus region >= 60%）", (main * 100.0).round()));
        n
    }

    fn row(&mut self, key: &str, children: Vec<UiNode>) -> Option<UiNode> {
        (!children.is_empty()).then(|| self.group(key, "row", None, None, children, "操作を 1 行に（主操作は region に 1 つ・副操作は同じ行: DESIGN §4.1）"))
    }

    /// CD-11 規則 1 の検索 form（主役の一覧の上）。
    fn search(&mut self, p: &Planned, verb: &str) -> Option<UiNode> {
        let id = p.action(verb)?;
        let q = FieldSpec { name: "query".into(), label: format!("Search {}", p.entity.label), kind: FieldKind::Text, options: vec![], mutable: false, title: false, auto: false };
        Some(self.form(&key(&p.entity.id, "search"), "Filter", &[&q], &p.source, "filter", id, "主役が利用者の増やす一覧なので検索を置く（CD-11 規則 1）"))
    }

    /// 主役の一覧（table か list）。
    fn collection(&mut self, p: &Planned, start: &str) -> UiNode {
        let cols = columns(p);
        let comp = if cols.len() >= 3 { "table" } else { "list" };
        let label = p.entity.label.clone();
        let why = if comp == "table" { "records で並べる欄が 3 つ以上 → table（DESIGN §7）" } else { "records で欄が少ない → list（DESIGN §7）" };
        let mut n = self.bound(&key(&p.entity.id, comp), comp, &label, &p.source, "items", Some(empty_text(&label, start)), why);
        set(&mut n, "columns", json!(cols.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()));
        let numeric: Vec<&str> = cols.iter().filter(|f| f.is_numeric()).map(|f| f.name.as_str()).collect();
        if comp == "table" && !numeric.is_empty() {
            set(&mut n, "numeric_columns", json!(numeric));
        }
        n
    }

    /// 頻度の高い入力（記録・見える化・覚える）は form を常設する。
    fn entry(&mut self, p: &Planned, label: &str) -> Option<UiNode> {
        let add = p.action("add")?;
        let fields = entry_fields(p);
        let mut n = self.form(&key(&p.entity.id, "entry"), label, &fields, &p.source, "draft", add, "入力の頻度が高いので form を上に常設（DESIGN §7: interaction frequency）");
        if self.r.cadence == Some(Cadence::Daily) {
            if let Some(d) = p.entity.date_field() {
                set(&mut n, "defaults", json!({ d.name.clone(): "today" }));
            }
        }
        Some(n)
    }

    /// 選択した 1 件の文脈（変える欄の form・本文の editor・済み・削除）。
    fn detail(&mut self, p: &Planned, with_delete: bool) -> Option<UiNode> {
        let e = &p.entity;
        let mut kids = Vec::new();
        for f in e.fields.iter().filter(|f| f.mutable && f.kind != FieldKind::Multiline && f.kind != FieldKind::Boolean) {
            if let Some(id) = p.action(&format!("update_{}", f.name)) {
                let why = format!("{}は作った後に変える欄 → 選択した 1 件の form", f.label);
                kids.push(self.form(&key(&e.id, &format!("{}-form", f.name.replace('_', "-"))), &format!("Change {}", f.label), &[f], &p.source, "selected", id, &why));
            }
        }
        for f in e.fields.iter().filter(|f| f.mutable && f.kind == FieldKind::Multiline) {
            if let Some(id) = p.action(&format!("update_{}", f.name)) {
                let k = key(&e.id, &format!("{}-editor", f.name.replace('_', "-")));
                let why = format!("{}は本文 → editor（DESIGN §7: text edit → editor）", f.label);
                kids.push(self.bound(&k, "editor", &f.label, &p.source, &format!("selected.{}", f.name), None, &why));
                kids.push(self.button(&format!("{k}-save"), &format!("Save {}", f.label), "secondary", id, "editor の内容は保存の操作で残す（local_write・取り消せる）"));
            }
        }
        if let Some(f) = e.fields.iter().find(|f| f.mutable && f.kind == FieldKind::Boolean) {
            let label = format!("Make it {}", f.label);
            if let Some(n) = self.verb_button(p, "toggle_done", "done", &label, "secondary") {
                kids.push(n);
            }
        }
        if with_delete {
            if let Some(n) = self.verb_button(p, "delete", "delete", "Delete", "danger") {
                kids.push(n);
            }
        }
        if kids.is_empty() {
            return None;
        }
        let label = if matches!(self.r.job, Job::Log | Job::Visualize) { "Selected record".to_string() } else { format!("Selected {}", e.label) };
        let mut n = self.group(&key(&e.id, "selected"), "column", Some("context"), Some(&label), kids, "選択した 1 件だけを扱う文脈（選んだ時だけ出す）");
        n.visibility = Some([("when".to_string(), json!("has_selection"))].into_iter().collect());
        Some(n)
    }

    fn measure(&mut self, p: &Planned) -> Vec<UiNode> {
        let mut out = Vec::new();
        let mut display = self.bound(&key(&p.entity.id, "display"), "text", "Time left", &p.source, "remaining_display", None, "残り時間（状態が 1 つ）→ 大きな text（DESIGN §7）");
        display.role = Some("primary".into());
        set(&mut display, "style", json!("timer"));
        self.intent(&display.key.clone(), "Show the time left in large type".into(), true, "text");
        out.push(display);
        let mut controls = Vec::new();
        if let Some(n) = self.verb_button(p, "toggle", "toggle", "Start / Pause", "primary") {
            controls.push(n);
        }
        if let Some(n) = self.verb_button(p, "reset", "reset", "Reset", "secondary") {
            controls.push(n);
        }
        out.extend(self.row(&key(&p.entity.id, "controls"), controls));
        if let Some(id) = p.action("save_settings") {
            let fields: Vec<&FieldSpec> = p.entity.fields.iter().collect();
            out.push(self.form(&key(&p.entity.id, "settings"), "Settings", &fields, &p.source, "settings", id, "設定（分）は閉じても残す"));
        }
        out
    }

    fn organize(&mut self, p: &Planned) -> Vec<UiNode> {
        let mut out = Vec::new();
        out.extend(self.search(p, "filter"));
        // DOGFOOD-X 周 7 #4: 足す手は欄つきの form（欄の無い `Add` の button は required の題を渡せず、押す前から「No field on this screen sets the input」）
        out.extend(self.entry(p, "Add"));
        let mut main = self.collection(p, "Add");
        main.role = Some("primary".into());
        self.intent(&main.key.clone(), format!("See {} in a list and filter it", p.entity.label), true, &main.component.clone());
        out.push(match self.detail(p, true) {
            Some(d) => self.split(&key(&p.entity.id, "split"), &[0.6, 0.4], vec![main, d]),
            None => main,
        });
        let mut acts = Vec::new();
        acts.extend(self.verb_button(p, "import_csv", "import", "Import CSV", "secondary"));
        out.extend(self.row(&key(&p.entity.id, "actions"), acts));
        out
    }

    fn log(&mut self, p: &Planned) -> Vec<UiNode> {
        let mut out = Vec::new();
        out.extend(self.entry(p, "Log it"));
        let label = p.entity.label.clone();
        let mut tl = self.bound(&key(&p.entity.id, "timeline"), "timeline", &format!("{label} log"), &p.source, "items", Some(empty_text(&format!("{label} log"), "Log it")), "日付つきの記録 → timeline（新しい順・日付で束ねる）");
        tl.role = Some("primary".into());
        if let Some(d) = p.entity.date_field() {
            set(&mut tl, "group_by", json!(d.name));
        }
        set(&mut tl, "order", json!("desc"));
        set(&mut tl, "columns", json!(columns(p).iter().map(|f| f.name.as_str()).collect::<Vec<_>>()));
        self.intent(&tl.key.clone(), format!("Look back at {label} by date"), true, "timeline");
        out.push(match self.detail(p, true) {
            Some(d) => self.split(&key(&p.entity.id, "split"), &[0.6, 0.4], vec![tl, d]),
            None => tl,
        });
        out
    }

    fn visualize(&mut self, p: &Planned) -> Vec<UiNode> {
        let e = &p.entity;
        let mut out = Vec::new();
        out.extend(self.entry(p, "Log it"));
        let num = e.numeric_field().map(|f| (f.name.clone(), f.label.clone())).unwrap_or_else(|| ("value".into(), "Value".into()));
        let cat = e.category_field().map(|f| (f.name.clone(), f.label.clone()));
        let date = e.date_field().map(|f| (f.name.clone(), f.label.clone()));
        let (x, xl) = cat.clone().or(date.clone()).unwrap_or_else(|| ("date".into(), "Date".into()));
        let chart_key = key(&e.id, "chart");
        let mut chart = self.bound(&chart_key, "chart", &format!("{} by {xl}", num.1), &p.source, "aggregate", Some(format!("Log something and {} by {xl} shows up here", num.1)), "");
        chart.role = Some("primary".into());
        set(&mut chart, "kind", json!(if cat.is_some() { "bar" } else { "line" }));
        set(&mut chart, "x", json!(x));
        set(&mut chart, "y", json!(num.0));
        set(&mut chart, "aggregate", json!("sum"));
        if cat.is_some() {
            if let Some((d, _)) = &date {
                set(&mut chart, "period", json!("month"));
                set(&mut chart, "period_field", json!(d));
            }
        }
        // V18: chart は理由が無ければ生成時に落ちる。理由 = 利用者の述語（見える化）とデータの形
        self.why.retain(|(k, _)| k != &chart_key);
        let verb = self.r.evidence.first().cloned().unwrap_or_default();
        self.note(&chart_key, format!("{verb}: 見える化を頼まれた → {xl}ごとの{}の合計を chart にする。同じ数字は隣の表にも出す（DESIGN §9: chart だけに頼らない）", num.1));
        self.intent(&chart_key, format!("See the flow of {} at a glance", e.label), true, "chart");
        let mut table = self.collection(p, "Log it");
        table.label = Some(format!("{} details", e.label));
        set(&mut table, "sortable", json!(true));
        out.push(self.split(&key(&e.id, "split"), &[0.6, 0.4], vec![chart, table]));
        let mut acts = Vec::new();
        acts.extend(self.verb_button(p, "import_csv", "import", "Import CSV", "secondary"));
        if let Some(mut d) = self.verb_button(p, "delete", "delete", "Delete selected row", "danger") {
            d.visibility = Some([("when".to_string(), json!("has_selection"))].into_iter().collect());
            acts.push(d);
        }
        out.extend(self.row(&key(&e.id, "actions"), acts));
        out
    }

    fn plan(&mut self, p: &Planned, checklist: Option<&Planned>) -> Vec<UiNode> {
        let e = &p.entity;
        let title = e.title_field().map(|f| f.name.clone()).unwrap_or_default();
        let date = e.date_field().map(|f| f.name.clone()).unwrap_or_default();
        let mut cal = self.bound(&key(&e.id, "calendar"), "calendar", &e.label, &p.source, "items", Some(empty_text(&e.label, "Add an event")), "日付の上に並べる → calendar（DESIGN §7: temporal → calendar）");
        set(&mut cal, "date_field", json!(date));
        set(&mut cal, "title_field", json!(title));
        self.intent(&cal.key.clone(), format!("See {} by date", e.label), true, "calendar");
        let mut acts = Vec::new();
        if let Some(mut d) = self.verb_button(p, "delete", "delete", "Delete selected event", "danger") {
            d.visibility = Some([("when".to_string(), json!("has_selection"))].into_iter().collect());
            acts.push(d);
        }
        // 周 7 #4: 予定を足す手は欄つきの form（organize と同じ・欄の無い button は題と日付を渡せない）
        let mut main_kids: Vec<UiNode> = self.entry(p, "Add an event").into_iter().collect();
        main_kids.push(cal);
        main_kids.extend(self.row(&key(&e.id, "actions"), acts));
        let main = self.group(&key(&e.id, "main"), "column", Some("primary"), None, main_kids, "主役: 日付の上の予定");
        let side = match checklist {
            Some(c) => {
                let mut list = self.bound(&key(&c.entity.id, "list"), "list", &c.entity.label, &c.source, "items", Some(empty_text(&c.entity.label, "Add")), "準備の確認 → 済みの印を付ける list");
                list.action_ref = c.action("toggle_done").map(String::from);
                let mut kids = vec![list];
                if let Some(add) = c.action("add") {
                    let fields: Vec<&FieldSpec> = c.entity.fields.iter().filter(|f| f.title).collect();
                    kids.push(self.form(&key(&c.entity.id, "add"), &format!("Add {}", c.entity.label), &fields, &c.source, "draft", add, "Add a prep item"));
                }
                self.intent(&key(&c.entity.id, "side"), format!("Check {}", c.entity.label), false, "list");
                Some(self.group(&key(&c.entity.id, "side"), "column", Some("context"), Some(&c.entity.label), kids, "準備は予定の横に（同時に見る）"))
            }
            None => self.detail(p, false),
        };
        match side {
            Some(s) => vec![self.split(&key(&e.id, "split"), &[0.6, 0.4], vec![main, s])],
            None => vec![main],
        }
    }

    fn memorize(&mut self, p: &Planned) -> Vec<UiNode> {
        let e = &p.entity;
        let front = e.title_field().map(|f| f.name.clone()).unwrap_or_default();
        let back = e.fields.iter().find(|f| !f.title && matches!(f.kind, FieldKind::Text | FieldKind::Multiline)).map(|f| f.name.clone()).unwrap_or_default();
        let daily = self.r.cadence.is_some();
        let mut remaining = self.bound(&key(&e.id, "remaining"), "text", if daily { "Left today" } else { "Left" }, &p.source, "due_count", None, "今日の残りの数は「今やるか」を決める数（DESIGN §3: 判断に効く数だけ）");
        set(&mut remaining, "style", json!("helper"));
        let empty = if daily { format!("Today's review is done. Whatever you add to {} will show up next time", e.label) } else { format!("No {} to review. Add some in “List”", e.label) };
        let mut card = self.bound(&key(&e.id, "card"), "detail", &e.label, &p.source, "next_due", Some(empty), "1 枚ずつ覚える → 1 件の detail（表を見せない）");
        set(&mut card, "front", json!(front));
        set(&mut card, "back", json!(back));
        set(&mut card, "reveal", json!("tap"));
        let flag = e.fields.iter().find(|f| f.kind == FieldKind::Boolean).map(|f| f.label.clone()).unwrap_or_else(|| "Got it".into());
        let mut review_kids = vec![remaining, card];
        let done: Vec<UiNode> = self.verb_button(p, "toggle_done", "done", &flag, "primary").into_iter().collect();
        review_kids.extend(self.row(&key(&e.id, "review-actions"), done));
        let review_label = if daily { "Today's review" } else { "Review" };
        let review = self.group(&key(&e.id, "review"), "column", Some("primary"), Some(review_label), review_kids, "主役: 1 枚ずつの復習");
        self.intent(&review.key.clone(), format!("Review {} one card at a time", e.label), true, "detail");
        let mut all_kids = Vec::new();
        all_kids.extend(self.entry(p, &format!("Add {}", e.label)));
        let list = {
            let mut n = self.bound(&key(&e.id, "list"), "list", &e.label, &p.source, "items", Some(empty_text(&e.label, &format!("Add {}", e.label))), "全部の一覧は別 tab（復習の邪魔をしない）");
            set(&mut n, "columns", json!([front, back]));
            n
        };
        all_kids.push(list);
        let mut acts = Vec::new();
        acts.extend(self.verb_button(p, "import_csv", "import", "Import CSV", "secondary"));
        if let Some(mut d) = self.verb_button(p, "delete", "delete", "Delete", "danger") {
            d.visibility = Some([("when".to_string(), json!("has_selection"))].into_iter().collect());
            acts.push(d);
        }
        all_kids.extend(self.row(&key(&e.id, "list-actions"), acts));
        let all = self.group(&key(&e.id, "all"), "column", None, Some("List"), all_kids, "Add · List · Import");
        self.intent(&all.key.clone(), format!("Add {} and see them in a list", e.label), false, "list");
        vec![self.group(&key(&e.id, "tabs"), "tabs", None, None, vec![review, all], "復習と一覧は同時に要らない → tabs")]
    }

    fn monitor(&mut self, p: &Planned) -> Vec<UiNode> {
        let e = &p.entity;
        let status = e.status_field().map(|f| (f.name.clone(), f.options.clone()));
        let mut board = self.bound(&key(&e.id, "board"), "timeline", &e.label, &p.source, "items", Some(empty_text(&e.label, "Add")), "状態のある物を 1 画面で → 状態で束ねた timeline（DESIGN §7: stages → timeline）");
        board.role = Some("primary".into());
        if let Some((name, stages)) = &status {
            set(&mut board, "group_by", json!(name));
            set(&mut board, "stages", json!(stages));
        }
        self.intent(&board.key.clone(), format!("See the status of {} on one screen", e.label), true, "timeline");
        let mut side = Vec::new();
        side.extend(self.detail(p, true));
        side.extend(self.sources_block());
        let body = if side.is_empty() {
            board
        } else {
            let s = self.group(&key(&e.id, "side"), "column", Some("context"), None, side, "The selected one, and a source you can connect (optional)");
            self.split(&key(&e.id, "split"), &[0.6, 0.4], vec![board, s])
        };
        // 周 7 #4: 足す手は欄つきの form（organize と同じ・欄の無い button は題を渡せない）
        let mut out: Vec<UiNode> = self.entry(p, "Add").into_iter().collect();
        out.push(body);
        out
    }

    fn author(&mut self, p: &Planned, imports: Option<&Planned>) -> Vec<UiNode> {
        let e = &p.entity;
        let body = e.fields.iter().find(|f| f.kind == FieldKind::Multiline).map(|f| (f.name.clone(), f.label.clone())).unwrap_or_else(|| ("body".into(), "Text".into()));
        let mut side = Vec::new();
        side.extend(self.search(p, "search"));
        side.push(self.bound(&key(&e.id, "list"), "list", &e.label, &p.source, "items", Some(empty_text(&e.label, "Create new")), "Your writing (choose on the left, write on the right)"));
        side.extend(self.verb_button(p, "create", "new", "Create new", "secondary"));
        if let Some(im) = imports {
            side.push(self.bound(&key(&im.entity.id, "list"), "list", &im.entity.label, &im.source, "items", Some(format!("No {} yet. Choose a file with “Import sources”", im.entity.label)), "Sources to cite (only files you chose)"));
            side.extend(self.verb_button(im, "add", "add", "Import sources", "secondary"));
        }
        let side = self.group(&key(&e.id, "side"), "column", Some("navigation"), None, side, "一覧と検索は左（主役の editor に 60%）");
        let mut main_kids = vec![self.bound(&key(&e.id, "editor"), "editor", &body.1, &p.source, &format!("selected.{}", body.0), None, "本文を書く → editor（DESIGN §7: text edit → editor）")];
        if imports.is_some() {
            main_kids.push(self.bound(&key(&e.id, "citations"), "list", &format!("Sources for this {}", e.label), &p.source, "selected.citations", Some("No sources yet. Choose a source and press “Insert citation”".into()), "引用は取り込んだ資料への参照だけ（出典の無い引用を作らない）"));
        }
        let mut acts = Vec::new();
        acts.extend(self.verb_button(p, "save", "save", "Save", "primary"));
        acts.extend(self.verb_button(p, "cite", "cite", "Insert citation", "secondary"));
        acts.extend(self.verb_button(p, "delete", "delete", "Delete", "danger"));
        main_kids.extend(self.row(&key(&e.id, "main-actions"), acts));
        let main = self.group(&key(&e.id, "main"), "column", Some("primary"), None, main_kids, "主役: 書く");
        self.intent(&main.key.clone(), format!("Write {}", e.label), true, "editor");
        vec![self.split(&key(&e.id, "split"), &[0.4, 0.6], vec![side, main])]
    }

    fn communicate(&mut self) -> Vec<UiNode> {
        let bp = self.bp;
        let mut out = Vec::new();
        let threads = bp.external(ExternalRole::Threads);
        if threads.map(|t| t.readiness != ActionReadiness::Ready).unwrap_or(false) {
            let mut hint = node("communicate-hint", "text");
            set(&mut hint, "style", json!("helper"));
            set(&mut hint, "text", json!("Choose an account to summarize and the list appears. Nothing is read until you choose."));
            self.note("communicate-hint", "繋ぐまで空である事を先に言う（DESIGN §6.2: 接続していない物を在るように見せない）");
            out.push(hint);
        }
        out.extend(self.sources_block());
        let mut list_kids = Vec::new();
        let mut thread_kids = Vec::new();
        if let Some(t) = threads {
            let src = t.need.source.clone().unwrap_or_default();
            let mut list = self.bound("communicate-threads", "list", "Conversations", &src, "threads", Some("No conversations yet".into()), "会話の一覧（出所の印を消さない: DESIGN §4.3）");
            set(&mut list, "show_source_badge", json!(true));
            list_kids.push(list);
            list_kids.push(self.button("communicate-read", &t.need.label, "ghost", &t.need.action.clone(), "読むだけの操作は ghost（DESIGN §4.5）"));
            let mut chat = self.bound("communicate-chat", "chat", "Conversations", &src, "selected.messages", Some("Choose a conversation to see its contents".into()), "会話 → chat（DESIGN §7）");
            set(&mut chat, "show_source_badge", json!(true));
            thread_kids.push(chat);
        }
        if let Some(d) = bp.entity(EntityKind::Drafts) {
            if let Some(save) = d.action("save") {
                let body: Vec<&FieldSpec> = d.entity.fields.iter().filter(|f| f.kind == FieldKind::Multiline).collect();
                thread_kids.push(self.form("communicate-reply", "Reply draft", &body, &d.source, "selected", save, "Drafts stay on this Mac. Sending is a separate step"));
            }
        }
        if let Some(s) = bp.external(ExternalRole::Send) {
            thread_kids.push(self.button("communicate-send", &s.need.label, "primary", &s.need.action.clone(), "送信は宛先と本文を確認してから 1 件ずつ（external_send・承認）"));
        }
        let list = self.group("communicate-list", "column", Some("navigation"), None, list_kids, "Conversation list");
        let thread = self.group("communicate-thread", "column", Some("primary"), None, thread_kids, "主役: 選んだ会話と返信");
        self.intent("communicate-thread", "Read the selected conversation and draft a reply".into(), true, "chat");
        out.push(self.split("communicate-split", &[0.4, 0.6], vec![list, thread]));
        out
    }

    fn mirror(&mut self) -> Vec<UiNode> {
        let bp = self.bp;
        let mut out = Vec::new();
        let mut side = Vec::new();
        let mut screen_node = None;
        if let Some(s) = bp.external(ExternalRole::Screen) {
            let src = s.need.source.clone().unwrap_or_default();
            let mut n = self.bound("device-screen", "source_surface", "Device screen", &src, "screen", None, "端末の画面 → source_surface（display = mirror。Shibubu の操作と混ぜない: DESIGN §4.4）");
            n.role = Some("primary".into());
            set(&mut n, "display", json!("mirror"));
            set(&mut n, "editable", json!(false));
            self.intent("device-screen", "See the screen of a connected device".into(), true, "source_surface");
            screen_node = Some(n);
            side.push(self.button("device-screen-read", &s.need.label, "ghost", &s.need.action.clone(), "The screen is shown after you agree to pairing"));
        }
        if let Some(p) = bp.entity(EntityKind::Pairing) {
            side.push(self.bound(&key(&p.entity.id, "list"), "list", &p.entity.label, &p.source, "items", Some("繋ぎ方の候補は Connections に並ぶ（OS を決め打ちしない）".into()), "繋ぎ方は候補から選ぶ（Android / iPhone を決め打ちしない）"));
        }
        let mut acts = Vec::new();
        if let Some(c) = bp.connection_action() {
            acts.push(self.button("device-connect", "Choose how to connect", "secondary", c, "Connecting starts only after you choose"));
        }
        if let Some(i) = bp.external(ExternalRole::Input) {
            acts.push(self.button("device-input", &i.need.label, "ghost", &i.need.action.clone(), "入力の送信は 1 回ずつ承認（external_write）"));
        }
        side.extend(self.row("device-actions", acts));
        let side = self.group("device-side", "column", Some("context"), None, side, "How to connect and what you can do");
        match screen_node {
            Some(s) => out.push(self.split("device-split", &[0.65, 0.35], vec![s, side])),
            None => out.push(side),
        }
        out
    }

    fn edit_media(&mut self) -> Vec<UiNode> {
        let bp = self.bp;
        let mut side = Vec::new();
        if let Some(c) = bp.entity(EntityKind::Clips) {
            side.push(self.bound(&key(&c.entity.id, "list"), "list", &c.entity.label, &c.source, "items", Some(format!("No {} yet. Choose a file with “Add footage”", c.entity.label)), "Your footage (only files you chose)"));
        }
        if let Some(m) = bp.entity(EntityKind::Markers) {
            let mut tl = self.bound(&key(&m.entity.id, "timeline"), "timeline", &m.entity.label, &m.source, "items", Some("No cut points yet".into()), "切る位置 → timeline（DESIGN §7: stages）");
            set(&mut tl, "group_by", json!("clip_id"));
            side.push(tl);
        }
        let mut acts = Vec::new();
        if let Some(c) = bp.entity(EntityKind::Clips) {
            acts.extend(self.verb_button(c, "add", "add", "Add footage", "primary"));
        }
        if let Some(m) = bp.entity(EntityKind::Markers) {
            acts.extend(self.verb_button(m, "add", "add", "Add a cut point", "secondary"));
        }
        if let Some(c) = bp.connection_action() {
            acts.push(self.button("media-connect", "Choose an editor", "secondary", c, "You choose which editor to use"));
        }
        side.extend(self.row("media-actions", acts));
        let side = self.group("media-side", "column", Some("context"), None, side, "Footage and cut points stay on this Mac");
        let mut surface = node("media-editor", "source_surface");
        surface.label = Some("editor".into());
        surface.role = Some("primary".into());
        set(&mut surface, "display", json!("handoff"));
        if let Some(x) = bp.external(ExternalRole::Editor) {
            surface.action_ref = Some(x.need.action.clone());
        }
        self.note("media-editor", "編集は既存の editor へ渡す（display = handoff。Shibubu は editor を作らない: DESIGN §12.2）");
        self.intent("media-editor", "Edit in the editor you chose".into(), true, "source_surface");
        vec![self.split("media-split", &[0.6, 0.4], vec![surface, side])]
    }

    fn record(&mut self) -> Vec<UiNode> {
        let bp = self.bp;
        let mut out = Vec::new();
        if let Some(mic) = bp.external(ExternalRole::Mic) {
            let mut rec = node("record-recorder", "recorder");
            rec.label = Some(mic.need.label.clone());
            rec.role = Some("primary".into());
            rec.action_ref = Some(mic.need.action.clone());
            set(&mut rec, "max_seconds", json!(600));
            self.note("record-recorder", "連続信号を録る → recorder（mic は OS の許可が要る）");
            self.intent("record-recorder", "Record from the microphone".into(), true, "recorder");
            out.push(rec);
        }
        if let Some(r) = bp.entity(EntityKind::Recordings) {
            let list = self.bound(&key(&r.entity.id, "list"), "list", &r.entity.label, &r.source, "items", Some(format!("No {} yet. Start with “{}”", r.entity.label, "Record")), "Recordings");
            let wave = self.bound(&key(&r.entity.id, "waveform"), "waveform", "Waveform", &r.source, "selected.waveform", None, "選んだ 1 本の波形 → waveform（DESIGN §7: continuous signal）");
            let mut kids = vec![wave];
            kids.extend(self.verb_button(r, "delete", "delete", "Delete", "danger"));
            let mut detail = self.group(&key(&r.entity.id, "detail"), "column", Some("context"), Some("Selected recording"), kids, "Only the selected one");
            detail.visibility = Some([("when".to_string(), json!("has_selection"))].into_iter().collect());
            out.push(self.split(&key(&r.entity.id, "split"), &[0.6, 0.4], vec![list, detail]));
        }
        out
    }

    /// 繋げる候補（runtime が Connections から埋める）＋ 接続を始める button。置くのは 1 回だけ。
    fn sources_block(&mut self) -> Vec<UiNode> {
        let bp = self.bp;
        let mut out = Vec::new();
        if self.why.iter().any(|(k, _)| k == "sources-candidates") {
            return out;
        }
        let Some(c) = bp.entity(EntityKind::Candidates) else { return out };
        out.push(self.bound("sources-candidates", "list", &c.entity.label, &c.source, "items", Some("The options are in Connections. Nothing is read until you connect".into()), "繋ぐ先は registry が出す候補から利用者が選ぶ（composer は列挙しない: GEN-003）"));
        let mut acts = Vec::new();
        if let Some(a) = bp.connection_action() {
            acts.push(self.button("sources-connect", "Choose a source", "secondary", a, "接続は利用者が選んでから（後で接続できる: DESIGN §6.1）"));
        }
        if let Some(x) = bp.external(ExternalRole::MonitorRead) {
            let src = x.need.source.clone().unwrap_or_default();
            out.push(self.bound("sources-items", "list", &x.need.source_label, &src, "items", Some("Nothing yet".into()), "Connected sources (empty for now; nothing is read)"));
            acts.push(self.button("sources-read", &x.need.label, "ghost", &x.need.action.clone(), "読むだけの操作は ghost（DESIGN §4.5）"));
        }
        out.extend(self.row("sources-actions", acts));
        out
    }

    /// job が消費しなかった名指しのサービス（Extra）を source ごとに 1 区画。
    fn extras(&mut self) -> Vec<UiNode> {
        let bp = self.bp;
        let mut out = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let extras: Vec<&PlannedExternal> = bp
            .externals
            .iter()
            .filter(|x| x.need.role == ExternalRole::Extra || (matches!(x.need.role, ExternalRole::Threads | ExternalRole::Send) && self.r.job != Job::Communicate))
            .collect();
        for x in &extras {
            let group = x.need.source.clone().unwrap_or_else(|| x.need.action.split('.').next().unwrap_or("web").to_string());
            if seen.contains(&group) {
                continue;
            }
            seen.push(group.clone());
            let base = group.replace(['_', '.'], "-");
            let mut kids = Vec::new();
            if let Some(src) = &x.need.source {
                kids.push(self.bound(&format!("{base}-items"), "list", &x.need.source_label, src, "items", Some("Nothing yet".into()), "Sources you named (empty until you connect)"));
            }
            let mut acts = Vec::new();
            for y in extras.iter().filter(|y| y.need.source.clone().unwrap_or_else(|| y.need.action.split('.').next().unwrap_or("web").to_string()) == group) {
                let variant = if y.need.effect.is_external() && y.need.effect != Effect::ExternalRead { "secondary" } else { "ghost" };
                let label = if y.need.source.is_none() { format!("{} {}", y.need.label, y.need.source_label) } else { y.need.label.clone() };
                acts.push(self.button(&format!("{base}-{}", y.need.action.rsplit('.').next().unwrap_or("run").replace('_', "-")), &label, variant, &y.need.action.clone(), "Actions of the service you named (connecting and approval happen when you run them)"));
            }
            kids.extend(self.row(&format!("{base}-actions"), acts));
            out.push(self.group(&format!("{base}-section"), "column", None, Some(&x.need.source_label), kids, "The service you named is kept apart and never mixed with your other data"));
        }
        out
    }
}

/// 最後に全 node を見て、README 規則 4・5 の文を必ず持たせる（各所で書いた文が優先）。
fn finish(n: &mut UiNode, bp: &Blueprint) {
    if let Some(a) = n.action_ref.as_deref().and_then(|id| bp.action(id)) {
        if a.readiness != ActionReadiness::Ready && !has_state(n, "disabled") {
            let label = n.label.clone().unwrap_or_else(|| "This action".into());
            state(n, "disabled", disabled_reason(a, &label));
        }
    }
    if let Some(b) = n.binding.clone() {
        if COLLECTIONS.contains(&n.component.as_str()) && !has_state(n, "empty") {
            state(n, "empty", "Nothing yet".into());
        }
        if let Some(s) = bp.source(&b.source_ref) {
            if !matches!(s.readiness, SourceReadiness::Local | SourceReadiness::Ready) && !has_state(n, "requires_connection") {
                let label = n.label.clone().unwrap_or_else(|| "Contents".into());
                state(n, "requires_connection", format!("{label} shows up once you connect a source (nothing is read until then)"));
            }
        }
    }
    for c in &mut n.children {
        finish(c, bp);
    }
}

pub fn scene(r: &Reading, bp: &Blueprint) -> Scene {
    let mut ui = Ui { bp, r, why: Vec::new(), intents: Vec::new() };
    let primary = bp.entity(EntityKind::Primary);
    let mut children = match (r.job, primary) {
        (Job::Measure, Some(p)) => ui.measure(p),
        (Job::Log, Some(p)) => ui.log(p),
        (Job::Visualize, Some(p)) => ui.visualize(p),
        (Job::Plan, Some(p)) => ui.plan(p, bp.entity(EntityKind::Checklist)),
        (Job::Memorize, Some(p)) => ui.memorize(p),
        (Job::Monitor, Some(p)) => ui.monitor(p),
        (Job::Author, Some(p)) => ui.author(p, bp.entity(EntityKind::Imports)),
        (Job::Communicate, _) => ui.communicate(),
        (Job::Mirror, _) => ui.mirror(),
        (Job::EditMedia, _) => ui.edit_media(),
        (Job::Record, _) => ui.record(),
        (_, Some(p)) => ui.organize(p),
        (_, None) => Vec::new(),
    };
    if r.externals.iter().any(|x| x.named.is_some()) {
        children.extend(ui.sources_block());
    }
    children.extend(ui.extras());
    let base = primary.map(|p| p.entity.id.clone()).unwrap_or_else(|| serde_json::to_value(r.job).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "app".into()));
    let root_key = key(&base, "root");
    let mut root = node(&root_key, "column");
    root.children = children;
    ui.note(&root_key, "縦に積む（ページ → 区画。card で包まない: DESIGN §3）");
    finish(&mut root, bp);
    if !ui.intents.iter().any(|i| i.primary) {
        ui.intent(&root_key, r.objective.clone(), true, "column");
    }
    Scene { root, rationale: ui.why, intents: ui.intents }
}

// ============================== W13c 配置の型（29 通目・3 つのお題） ==============================

/// W13c: 型の区画。並びは上から top → main と side（左右の split・main が 60% 以上）→ bottom → drawer（畳める引き出し）。
/// Wave: W13c
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Main,
    Side,
    Top,
    Bottom,
    Drawer,
}

/// W13c: 部品の種類（型の `accepts` の語彙）。置き場所を引くためだけに使い、部品の選び方は変えない。
/// 決め方は `part_kind`（component・面の backend・bind した resource の kind）。どれでもない部品は `Other`（どの区画も受けなければ main へ）。
/// Wave: W13c
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// Project の Terminal（component `terminal`）
    Terminal,
    /// 元の窓を映す面（`source_surface` で backend が mirror）
    Mirror,
    /// 本物の app の窓を重ねる面（backend が native_direct）
    NativeApp,
    /// Web の面（backend が webview）
    Web,
    /// repo の resource に bind した部品（Git の欄）
    Repo,
    /// file の resource に bind した部品（素材の一覧）
    File,
    /// 走らせた結果（RunReceipt の一覧）
    RunReceipt,
    /// 書きとめ（editor）
    Notes,
    /// G88: 録音の部品（component `recorder`）
    Recorder,
    /// G88: 録った音の波形（component `waveform`）
    Waveform,
    /// 保存先などの link
    Link,
    /// 状態の 1 行
    Status,
    /// 入力 form
    Form,
    /// timeline
    Timeline,
    Other,
}

/// W13c: 区画 1 つ。`size` は main と side の幅の割合（両方在る Page では足して 1・main ≥ 0.6）。他の区画は null。
/// Wave: W13c
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub slot: Slot,
    pub accepts: Vec<PartKind>,
    #[serde(default)]
    pub size: Option<f64>,
    /// G88a: この区画に置く部品の数の上限（無ければ無制限）。満ちたら次に受ける区画へ・どこも受けなければ main（`place`・落とさない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

/// W13c: 1 枚の Page の置き方。`page` は Page の title（大小文字を見ない）か `*`（title の合う entry が無い Page 全部）。
/// Wave: W13c
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagePlacement {
    pub page: String,
    pub regions: Vec<Region>,
}

/// W13c: 配置の型（JSON・`docs/schemas/placement_template_v1.schema.json`）。`goal_hint` は一言にこの語が在れば既定にする語（data の名前の表・code に置かない）。
/// 例: `{"id": "dev", "name": "開発環境", "goal_hint": ["開発環境", "repo"], "pages": [{"page": "Dev", "regions": [{"slot": "main", "accepts": ["terminal"], "size": 0.65}, …]}]}`
/// Wave: W13c
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementTemplate {
    pub id: String,
    pub name: String,
    pub goal_hint: Vec<String>,
    pub pages: Vec<PagePlacement>,
}

/// W13c: 同梱の型（`assets/templates/*.json` を build に埋め込む）。読めない・`validate_template` を通らない型は入れない（同梱の試験が赤にする）。
/// Wave: W13c
pub fn bundled_templates() -> Vec<PlacementTemplate> {
    static ALL: std::sync::OnceLock<Vec<PlacementTemplate>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        BUNDLED
            .iter()
            .filter_map(|(name, text)| match serde_json::from_str::<PlacementTemplate>(text) {
                Ok(t) if validate_template(&t).is_ok() => Some(t),
                other => {
                    let why = match other {
                        Ok(t) => validate_template(&t).err().unwrap_or_default().join(" / "),
                        Err(e) => e.to_string(),
                    };
                    eprintln!("W13c: 同梱の型 {name} を読めません（{why}）");
                    None
                }
            })
            .collect()
    })
    .clone()
}

/// 同梱の型の file（並び = 「配置」を替える順・同じ数だけ合った時に既定にする順）。
const BUNDLED: [(&str, &str); 3] = [
    ("dev.json", include_str!("../../assets/templates/dev.json")),
    ("music.json", include_str!("../../assets/templates/music.json")),
    ("tasks.json", include_str!("../../assets/templates/tasks.json")),
];

impl Slot {
    pub const fn name(self) -> &'static str {
        match self {
            Slot::Main => "main",
            Slot::Side => "side",
            Slot::Top => "top",
            Slot::Bottom => "bottom",
            Slot::Drawer => "drawer",
        }
    }
}

/// W13c: 型の形の検査（schema の enum と deny_unknown_fields は serde が見る・ここは意味）: id と name が空でない・Page ごとに main が 1 つ・同じ区画は 1 回・
/// main と side が両方在れば size が両方在って足して 1（±0.01）・main ≥ 0.6・他の区画の size は null・accepts が空でない。
/// Wave: W13c
pub fn validate_template(t: &PlacementTemplate) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();
    if t.id.trim().is_empty() || t.name.trim().is_empty() {
        errs.push("id か name が空".to_string());
    }
    if t.pages.is_empty() {
        errs.push("pages が空".into());
    }
    let mut seen_pages = std::collections::HashSet::new();
    for pg in &t.pages {
        let at = &pg.page;
        if !seen_pages.insert(pg.page.to_lowercase()) {
            errs.push(format!("Page '{at}' が 2 回"));
        }
        let count = |s: Slot| pg.regions.iter().filter(|r| r.slot == s).count();
        if count(Slot::Main) != 1 {
            errs.push(format!("Page '{at}': main が 1 つでない（{}）", count(Slot::Main)));
        }
        for s in [Slot::Side, Slot::Top, Slot::Bottom, Slot::Drawer] {
            if count(s) > 1 {
                errs.push(format!("Page '{at}': {} が 2 回以上", s.name()));
            }
        }
        for r in &pg.regions {
            if r.accepts.is_empty() {
                errs.push(format!("Page '{at}': {} の accepts が空", r.slot.name()));
            }
            if !matches!(r.slot, Slot::Main | Slot::Side) && r.size.is_some() {
                errs.push(format!("Page '{at}': {} に size（main と side だけ）", r.slot.name()));
            }
        }
        let size = |s: Slot| pg.regions.iter().find(|r| r.slot == s).and_then(|r| r.size);
        if count(Slot::Side) == 1 {
            match (size(Slot::Main), size(Slot::Side)) {
                (Some(m), Some(s)) if (m + s - 1.0).abs() > 0.01 => errs.push(format!("Page '{at}': main と side の size の和が 1 でない（{m} + {s}）")),
                (Some(m), Some(_)) if m < 0.6 => errs.push(format!("Page '{at}': main の size が 0.6 より小さい（{m}・DESIGN §7）")),
                (Some(_), Some(_)) => {}
                _ => errs.push(format!("Page '{at}': main と side が在るのに size が無い")),
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// W13c: 一言に合う既定の型（`goal_hint` の語が一言に多く在る型・同じ数なら並びの先・1 つも無ければ None = 型なし）。大小文字を見ない。
/// Wave: W13c
pub fn default_template<'a>(goal: &str, templates: &'a [PlacementTemplate]) -> Option<&'a PlacementTemplate> {
    let g = goal.to_lowercase();
    templates
        .iter()
        .enumerate()
        .map(|(i, t)| (t.goal_hint.iter().filter(|h| !h.trim().is_empty() && g.contains(&h.to_lowercase())).count(), std::cmp::Reverse(i), t))
        .filter(|(n, _, _)| *n > 0)
        .max_by_key(|(n, i, _)| (*n, *i))
        .map(|(_, _, t)| t)
}

/// W13c: 部品 1 つの種類（Page の root の直下の子に使う）。terminal → Terminal・source_surface → その面の backend・
/// `resource.<id>` に bind（子孫を含む）→ その resource の kind（repo → Repo・file → File）・form → Form・timeline → Timeline・editor → Notes・他 → Other。
/// Wave: W13c
pub fn part_kind(node: &UiNode, p: &ProjectDefinitionV1) -> PartKind {
    use super::domain::project::SurfaceBackendKind as B;
    // node そのものの種類（子を見ない）
    let own = |n: &UiNode| -> PartKind {
        let bound = n.binding.as_ref().and_then(|b| b.source_ref.strip_prefix("resource.")).and_then(|id| p.resources.iter().find(|r| r.id == id));
        match (n.component.as_str(), bound) {
            ("terminal", _) => PartKind::Terminal,
            ("source_surface", _) => match n.props.get("surface_ref").and_then(Value::as_str).and_then(|id| p.surface_specs.iter().find(|s| s.id == id)).map(|s| s.backend) {
                Some(B::Mirror) => PartKind::Mirror,
                Some(B::NativeDirect) => PartKind::NativeApp,
                Some(B::WebView) => PartKind::Web,
                None => PartKind::Other,
            },
            (_, Some(r)) if r.kind == "repo" => PartKind::Repo,
            (_, Some(r)) if r.kind == "file" => PartKind::File,
            ("form", _) => PartKind::Form,
            ("timeline", _) => PartKind::Timeline,
            ("editor", _) => PartKind::Notes,
            ("recorder", _) => PartKind::Recorder,
            ("waveform", _) => PartKind::Waveform,
            _ => PartKind::Other,
        }
    };
    match own(node) {
        PartKind::Other => {
            // 容れ物（column・row 等）: 子孫の種類が 1 つだけならそれ
            let mut kinds: Vec<PartKind> = node.nodes().into_iter().map(own).filter(|k| *k != PartKind::Other).collect();
            kinds.dedup();
            kinds.sort_by_key(|k| *k as u8);
            kinds.dedup();
            if kinds.len() == 1 {
                kinds[0]
            } else {
                PartKind::Other
            }
        }
        k => k,
    }
}

/// G94: 型が `page` に持つ行（Page の id・題名の大小文字を除いた一致・無ければ `*`）。無ければ `place` は木をそのまま返す。
fn entry_for<'a>(t: &'a PlacementTemplate, page: &PageDefinition) -> Option<&'a PagePlacement> {
    let title = page.title.to_lowercase();
    t.pages.iter().find(|e| e.page == page.id || e.page.to_lowercase() == title).or_else(|| t.pages.iter().find(|e| e.page == "*"))
}

/// G94: この型が `page` の並びに効くか（`place` が木を替える行を持つか）。効かない型へ `Layout: {name}` を回すと label だけ替わり画面が動かない。
/// 例: `composer::layout::bundled_templates().iter().any(|t| composer::layout::applies_to(t, &page))`
pub fn applies_to(t: &PlacementTemplate, page: &PageDefinition) -> bool {
    entry_for(t, page).is_some()
}

/// W13c: 置き方だけを型に従わせた Page の木（描く時に使う・保存した Project は変えない）。
/// 型が None・その Page の型が無い・slot の容れ物の key が既存の key と重なる時は `page.root` のまま（今の並び）。
/// root の直下の子を `part_kind` で区画へ振り分ける（見出しの text〈style heading〉は次の部品と同じ区画）。どの区画も受けない子は main へ。区画の中は元の順。
/// 返す木: root と同じ key の column の下に、top の column → main と side（両方在れば split・ratio は size）→ bottom → drawer（どれも空なら置かない）。
/// 容れ物の key は `slot.<区画>`・props `{"slot": "<区画>"}`（renderer が AX の `slot:<区画>` を付ける）。
/// 保証する: 返した木の slot の容れ物を除いた node の集合（key と中身）が `page.root` と同じ（部品を足さない・消さない・変えない）。
/// LAYOUT（owner 10-31 の 19 点の 3「録音のところが下に行きすぎて scroll」）: bottom に振った録音の帯（`PartKind::Recorder`）は、main に部品が在れば main の末尾
/// （主の app の真下）へ移す。bottom は split の下 = 背の高い side の列の後ろで、縦に畳む幅では窓の外だった。main が空（Web だけの Page）なら bottom のまま（全幅の Web を細くしない）。
/// side の区画の中は desktop app（Mirror・Native）が上・他（Web 等）が下（完成予想図の右上 Music・右下 YouTube・model の席の順に依らない・同じ種類の中は元の順）。
/// Wave: W13c, LAYOUT
pub fn place(page: &PageDefinition, p: &ProjectDefinitionV1, template: Option<&PlacementTemplate>) -> UiNode {
    let root = &page.root;
    let Some(entry) = template.and_then(|t| entry_for(t, page)) else { return root.clone() };
    if root.component != "column" || root.nodes().iter().any(|n| n.key.starts_with("slot.")) {
        return root.clone();
    }
    // 部品を並びの順に、受ける区画のうち空き（`max`）の在る最初の区画へ・どこも受けない / 空きが無い → main（落とさない）。
    // 見出し（text の style heading）は数えず次の部品と同じ区画（後ろから決めると見出しが続いても届く・末尾の見出しは自分の種類で）
    let heading = |n: &UiNode| n.component == "text" && n.props.get("style").and_then(Value::as_str) == Some("heading");
    let n = root.children.len();
    let mut used = vec![0u32; entry.regions.len()];
    let mut slots: Vec<Option<Slot>> = vec![None; n];
    for (i, c) in root.children.iter().enumerate() {
        if heading(c) && i + 1 < n {
            continue;
        }
        let k = part_kind(c, p);
        let at = (0..entry.regions.len()).find(|&j| entry.regions[j].accepts.contains(&k) && entry.regions[j].max.is_none_or(|m| used[j] < m));
        slots[i] = Some(at.map_or(Slot::Main, |j| {
            used[j] += 1;
            entry.regions[j].slot
        }));
    }
    for i in (0..n).rev() {
        if slots[i].is_none() {
            slots[i] = Some(slots.get(i + 1).copied().flatten().unwrap_or(Slot::Main));
        }
    }
    let mut buckets: Vec<(Slot, Vec<UiNode>)> = [Slot::Top, Slot::Main, Slot::Side, Slot::Bottom, Slot::Drawer].into_iter().map(|s| (s, Vec::new())).collect();
    for (c, s) in root.children.iter().zip(slots) {
        let s = s.unwrap_or(Slot::Main);
        if let Some((_, b)) = buckets.iter_mut().find(|(x, _)| *x == s) {
            b.push(c.clone());
        }
    }
    let take = |s: Slot, buckets: &mut Vec<(Slot, Vec<UiNode>)>| std::mem::take(&mut buckets.iter_mut().find(|(x, _)| *x == s).expect("5 区画").1);
    let (main, bottom, mut side) = (take(Slot::Main, &mut buckets), take(Slot::Bottom, &mut buckets), take(Slot::Side, &mut buckets));
    let (rec, bottom): (Vec<UiNode>, Vec<UiNode>) = bottom.into_iter().partition(|c| !main.is_empty() && part_kind(c, p) == PartKind::Recorder);
    // ponytail: 見出し（text）は app でないので下へ回る（side に見出しを置く席の型が出たら次の部品と組にして並べる）
    side.sort_by_key(|c| !matches!(part_kind(c, p), PartKind::Mirror | PartKind::NativeApp));
    for (s, b) in [(Slot::Main, [main, rec].concat()), (Slot::Bottom, bottom), (Slot::Side, side)] {
        buckets.iter_mut().find(|(x, _)| *x == s).expect("5 区画").1 = b;
    }
    let holder = |s: Slot, children: Vec<UiNode>| {
        let mut n = node(&format!("slot.{}", s.name()), "column");
        set(&mut n, "slot", json!(s.name()));
        n.children = children;
        n
    };
    let mut out = Vec::new();
    let top = take(Slot::Top, &mut buckets);
    if !top.is_empty() {
        out.push(holder(Slot::Top, top));
    }
    let (main, side) = (take(Slot::Main, &mut buckets), take(Slot::Side, &mut buckets));
    match (main.is_empty(), side.is_empty()) {
        (false, false) => {
            let size = |s: Slot| entry.regions.iter().find(|r| r.slot == s).and_then(|r| r.size);
            let (m, s) = match (size(Slot::Main), size(Slot::Side)) {
                (Some(m), Some(s)) => (m, s),
                _ => (0.65, 0.35),
            };
            let mut split = node("slot.split", "split");
            split.layout = Some([("ratio".to_string(), json!([m, s]))].into_iter().collect());
            split.children = vec![holder(Slot::Main, main), holder(Slot::Side, side)];
            out.push(split);
        }
        (false, true) => out.push(holder(Slot::Main, main)),
        (true, false) => out.push(holder(Slot::Side, side)),
        (true, true) => {}
    }
    for s in [Slot::Bottom, Slot::Drawer] {
        let b = take(s, &mut buckets);
        if !b.is_empty() {
            out.push(holder(s, b));
        }
    }
    UiNode { children: out, ..root.clone() }
}

/// G88a: 1 Page に席が 2 つ以上ある時、`project::plan_project` が席の node（root の直下の子）の props に置く model の置き場 `{"slot": "main" | "side" | "bottom", "why": "<1 行>"}`。
/// 席が 1 つの Page には置かない（録画の dev の Page の置き方は今の型のまま）。
pub const SEAT_PROP: &str = "seat";

/// G88a: model の置き場から作る型の id（`Generated::placement` が保存した選択の無い時に先に使う・「Layout:」の替え順の先頭）。同梱の型の id と重ならない。
pub const SEAT_TEMPLATE: &str = "seats";

/// G88a: model が席ごとに決めた置き場（`SEAT_PROP`）を、今の型の形（`PlacementTemplate` / `PagePlacement` / `Region`）にする。
/// `SEAT_PROP` の在る Page ごとに `PagePlacement{page: <Page の id>, regions}`: main → side → bottom の順に、その区画に置いた席の `part_kind` を `accepts`・
/// 席の数を `max`（`size` は null = `place` の 0.65 / 0.35）。後ろに `fallback`（一言に合う既定の型）の entry を足す（置き場の無い Page はそちらで置く）。
/// 例: `let t = layout::seat_template(&p, layout::default_template(&p.goal, &all)); layout::place(&page, &p, t.as_ref())`
/// 保証する: `SEAT_PROP` の在る Page が 1 つも無い → None（呼ぶ側は今の既定の型）。id = `SEAT_TEMPLATE`。`place` は Page の id でも entry を引く。
/// 保証しない: 後から足した部品（置き場の無い node）の置き場（どの区画も受けなければ main）。
/// Wave: M2
pub fn seat_template(p: &ProjectDefinitionV1, fallback: Option<&PlacementTemplate>) -> Option<PlacementTemplate> {
    let slot_of = |n: &UiNode| n.props.get(SEAT_PROP).and_then(|v| serde_json::from_value::<Slot>(v["slot"].clone()).ok());
    let pages: Vec<PagePlacement> = p
        .pages
        .iter()
        .filter_map(|pg| {
            let seats: Vec<(Slot, PartKind)> = pg.root.children.iter().filter_map(|c| Some((slot_of(c)?, part_kind(c, p)))).collect();
            let regions: Vec<Region> = [Slot::Main, Slot::Side, Slot::Bottom]
                .into_iter()
                .filter_map(|s| {
                    let here: Vec<PartKind> = seats.iter().filter(|(x, _)| *x == s).map(|(_, k)| *k).collect();
                    let mut accepts: Vec<PartKind> = Vec::new();
                    for k in &here {
                        if !accepts.contains(k) {
                            accepts.push(*k);
                        }
                    }
                    (!here.is_empty()).then(|| Region { slot: s, accepts, size: None, max: Some(here.len() as u32) })
                })
                .collect();
            (!regions.is_empty()).then(|| PagePlacement { page: pg.id.clone(), regions })
        })
        .collect();
    if pages.is_empty() {
        return None;
    }
    Some(PlacementTemplate {
        id: SEAT_TEMPLATE.into(),
        name: "Suggested".into(),
        goal_hint: vec![],
        pages: pages.into_iter().chain(fallback.into_iter().flat_map(|t| t.pages.clone())).collect(),
    })
}

#[cfg(test)]
mod w13c_interface {
    use super::*;

    #[test]
    fn w13c_signatures() {
        let _: fn() -> Vec<PlacementTemplate> = bundled_templates;
        let _: fn(&PlacementTemplate) -> Result<(), Vec<String>> = validate_template;
        let _: for<'a> fn(&str, &'a [PlacementTemplate]) -> Option<&'a PlacementTemplate> = default_template;
        let _: fn(&UiNode, &ProjectDefinitionV1) -> PartKind = part_kind;
        let _: fn(&PageDefinition, &ProjectDefinitionV1, Option<&PlacementTemplate>) -> UiNode = place;
    }

    /// 保存形式: 知らない欄は拒否・区画と種類は snake_case。
    #[test]
    fn w13c_template_round_trips_and_rejects_unknown_fields() {
        let v = json!({"id": "t", "name": "型", "goal_hint": ["x"], "pages": [{"page": "*", "regions": [{"slot": "main", "accepts": ["native_app", "run_receipt"], "size": 0.7}, {"slot": "drawer", "accepts": ["notes"], "size": null}]}]});
        let t: PlacementTemplate = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(t.pages[0].regions[0].accepts, [PartKind::NativeApp, PartKind::RunReceipt]);
        assert_eq!(serde_json::to_value(&t).unwrap(), v);
        let mut extra = v;
        extra["pages"][0]["regions"][0]["x"] = json!(1);
        assert!(serde_json::from_value::<PlacementTemplate>(extra).is_err(), "座標の欄（未知）は拒否");
    }
}

/// W13c の試験（composer だけ・画面は E2E）。負の対照: `place` で区画が受けない部品を落とす / 足す実装にすると
/// `w13c_place_keeps_every_part_and_only_moves_them` が赤（容れ物を除いた node の集合が page.root と違う）。
#[cfg(test)]
mod w13c_tests {
    use super::super::project::compose_project;
    use super::super::ComposeContext;
    use super::*;
    use std::collections::BTreeSet;

    fn demo() -> ProjectDefinitionV1 {
        compose_project("この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい", &ComposeContext::default(), Some(std::path::Path::new("/tmp")))
            .unwrap()
            .expect("主デモ")
            .project
    }

    /// 容れ物（slot.*）を除いた node（key と中身）の集合。中身は子を除いた JSON（容れ物の下に入っても同じに見える）。
    fn parts(root: &UiNode) -> BTreeSet<(String, String)> {
        root.nodes()
            .into_iter()
            .filter(|n| !n.key.starts_with("slot."))
            .map(|n| (n.key.clone(), serde_json::to_string(&UiNode { children: vec![], ..n.clone() }).unwrap()))
            .collect()
    }

    fn keys_under(root: &UiNode, slot: &str) -> Vec<String> {
        root.find(&format!("slot.{slot}")).map(|s| s.nodes().into_iter().skip(1).filter(|n| !n.key.starts_with("slot.")).map(|n| n.key.clone()).collect()).unwrap_or_default()
    }

    /// Dev の Page に Terminal を足した形（＋ Terminal の op と同じ: root の末尾）。
    fn dev_with_terminal(p: &ProjectDefinitionV1) -> PageDefinition {
        let mut dev = p.pages[0].clone();
        let mut t = node("terminal.dev", "terminal");
        set(&mut t, "terminal_ref", json!(p.terminal_specs[0].id));
        dev.root.children.push(t);
        dev
    }

    #[test]
    fn w13c_bundled_three_templates_validate_and_match_the_schema() {
        let all = bundled_templates();
        assert_eq!(all.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["dev", "music", "tasks"], "同梱の型が 3 つ読めない");
        for t in &all {
            validate_template(t).unwrap_or_else(|e| panic!("{}: {e:?}", t.id));
        }
        // schema の enum と Rust の型の語が同じ（片方だけ足すと赤）
        let schema: Value = serde_json::from_str(include_str!("../../docs/schemas/placement_template_v1.schema.json")).unwrap();
        let region = &schema["properties"]["pages"]["items"]["properties"]["regions"]["items"]["properties"];
        let names = |v: &Value| v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect::<BTreeSet<_>>();
        let slots: BTreeSet<String> = [Slot::Main, Slot::Side, Slot::Top, Slot::Bottom, Slot::Drawer].iter().map(|s| serde_json::to_value(s).unwrap().as_str().unwrap().to_owned()).collect();
        assert_eq!(names(&region["slot"]["enum"]), slots);
        use PartKind::*;
        let kinds: BTreeSet<String> = [Terminal, Mirror, NativeApp, Web, Repo, File, RunReceipt, Notes, Recorder, Waveform, Link, Status, Form, Timeline, Other].iter().map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_owned()).collect();
        assert_eq!(names(&region["accepts"]["items"]["enum"]), kinds);
    }

    #[test]
    fn w13c_validate_rejects_broken_templates() {
        let base = || bundled_templates().into_iter().next().unwrap();
        let mut t = base();
        t.pages[0].regions[0].size = Some(0.5);
        t.pages[0].regions[1].size = Some(0.5);
        assert!(validate_template(&t).is_err(), "main が 60% 未満");
        let mut t = base();
        t.pages[0].regions[1].size = Some(0.2);
        assert!(validate_template(&t).is_err(), "main と side の和が 1 でない");
        let mut t = base();
        t.pages[0].regions[1].slot = Slot::Main;
        assert!(validate_template(&t).is_err(), "main が 2 つ");
        let mut t = base();
        t.pages[0].regions[2].size = Some(0.3);
        assert!(validate_template(&t).is_err(), "bottom に size");
        let mut t = base();
        t.pages[0].regions[0].accepts.clear();
        assert!(validate_template(&t).is_err(), "accepts が空");
    }

    #[test]
    fn w13c_default_template_follows_the_goal() {
        let all = bundled_templates();
        let id = |g: &str| default_template(g, &all).map(|t| t.id.clone());
        assert_eq!(id("この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい").as_deref(), Some("dev"));
        assert_eq!(id("REAPER で作曲して、素材を並べて書き出したい").as_deref(), Some("music"));
        assert_eq!(id("Reminders と GitHub の Issues でタスクを管理したい").as_deref(), Some("tasks"));
        assert_eq!(id("読書記録をつけたい"), None, "それ以外は型なし");
    }

    /// 型あり / なしで部品の集合が同じ（置き方だけ違う）。主デモの Dev は main = Terminal（と区画の決まらない repo の行）・side = Git の欄（見出しも）。
    #[test]
    fn w13c_place_keeps_every_part_and_only_moves_them() {
        let p = demo();
        let dev = dev_with_terminal(&p);
        let t = bundled_templates().into_iter().find(|t| t.id == "dev").unwrap();
        assert_eq!(place(&dev, &p, None), dev.root, "型なしは今の並び");
        let placed = place(&dev, &p, Some(&t));
        assert_ne!(placed, dev.root, "型が効いていない");
        assert_eq!(parts(&placed), parts(&dev.root), "型で部品が増えた・減った・変わった");
        let main = keys_under(&placed, "main");
        let side = keys_under(&placed, "side");
        assert!(main.contains(&"terminal.dev".to_owned()), "main に Terminal が無い: {main:?}");
        assert_eq!(side, ["dev-git-title", "dev-git-branch", "dev-git-sha", "dev-git-changes"], "side が Git の欄（見出しは次の部品と同じ区画）");
        assert!(main.contains(&"dev-repo".to_owned()) && main.contains(&"dev-next".to_owned()), "区画の決まらない部品を main に置かない: {main:?}");
        // split は main に 65%（verifier の responsiveness を通る）
        let split = placed.find("slot.split").expect("main と side の split");
        assert_eq!(split.layout.as_ref().unwrap()["ratio"], json!([0.65, 0.35]));
        let mut f = Vec::new();
        super::super::verifier::walk(&placed, &mut f);
        assert!(f.is_empty(), "置いた木が responsiveness に落ちた: {f:?}");
        // Docs・Diagram は面が main（side の notes・link は無いので split にしない）
        for pg in &p.pages[1..] {
            let placed = place(pg, &p, Some(&t));
            assert_eq!(parts(&placed), parts(&pg.root));
            assert!(placed.find("slot.split").is_none() && !keys_under(&placed, "main").is_empty(), "{}: {placed:?}", pg.title);
        }
    }

    /// 型の区画が受けない部品は落とさず main へ。Page の entry が無い型は今の並び。
    #[test]
    fn w13c_unaccepted_parts_fall_into_main_and_unknown_pages_stay() {
        let p = demo();
        let dev = dev_with_terminal(&p);
        let only_web = PlacementTemplate { id: "w".into(), name: "W".into(), goal_hint: vec![], pages: vec![PagePlacement { page: "*".into(), regions: vec![Region { slot: Slot::Main, accepts: vec![PartKind::Web], size: None, max: None }, Region { slot: Slot::Top, accepts: vec![PartKind::Form], size: None, max: None }] }] };
        let placed = place(&dev, &p, Some(&only_web));
        assert_eq!(parts(&placed), parts(&dev.root));
        assert_eq!(keys_under(&placed, "main").len(), parts(&dev.root).len() - 1, "受けない部品が main に落ちていない（root を除く全部）");
        let no_dev = PlacementTemplate { pages: vec![PagePlacement { page: "Docs".into(), ..only_web.pages[0].clone() }], ..only_web };
        assert_eq!(place(&dev, &p, Some(&no_dev)), dev.root, "その Page の型が無いのに並べ替えた");
    }

    #[test]
    fn w13c_part_kinds_come_from_the_data_shape() {
        let p = demo();
        let dev = dev_with_terminal(&p);
        let kind = |key: &str, pg: &PageDefinition| part_kind(pg.root.find(key).unwrap(), &p);
        assert_eq!(kind("terminal.dev", &dev), PartKind::Terminal);
        assert_eq!(kind("dev-git-changes", &dev), PartKind::Repo);
        assert_eq!(kind("dev-repo", &dev), PartKind::Other);
        // M3: Docs の TextEdit の席は feature `mirror` の時は Mirror（`project::APP_SEAT_BACKEND`）・無い時は NativeDirect
        let app_part = if cfg!(feature = "mirror") { PartKind::Mirror } else { PartKind::NativeApp };
        assert_eq!(kind("docs-surface", &p.pages[1]), app_part);
        assert_eq!(kind("diagram-surface", &p.pages[2]), PartKind::Web);
    }

    /// G88a: 区画の `max` が満ちたら次に受ける区画へ・どこも受けなければ main（落とさない）。serde は `max` を書く（無ければ書かない = 前の型の JSON と同じ）。
    /// 負の対照（commit message）: `place` が `max` を見ない → 2 つ目と 3 つ目の Web も main で赤
    #[test]
    fn g88a_a_full_region_passes_parts_to_the_next_region() {
        let p = demo();
        // Web の面（Diagram）を 3 つ指す Page
        let three: PageDefinition = serde_json::from_value(json!({"id": "page.three", "route": "/three", "title": "Three", "revision": 1, "root": {"key": "three-root", "component": "column", "children": [
            {"key": "w1", "component": "source_surface", "props": {"surface_ref": "diagram"}},
            {"key": "w2", "component": "source_surface", "props": {"surface_ref": "diagram"}},
            {"key": "w3", "component": "source_surface", "props": {"surface_ref": "diagram"}}
        ]}}))
        .unwrap();
        let region = |slot, max: Option<u32>, size| Region { slot, accepts: vec![PartKind::Web], size, max };
        let two = PlacementTemplate { id: "w".into(), name: "W".into(), goal_hint: vec![], pages: vec![PagePlacement { page: "*".into(), regions: vec![region(Slot::Main, Some(1), Some(0.65)), region(Slot::Side, None, Some(0.35))] }] };
        validate_template(&two).expect("max の在る型が検査を通らない");
        let placed = place(&three, &p, Some(&two));
        assert_eq!(parts(&placed), parts(&three.root), "max で部品が増えた・減った");
        assert_eq!((keys_under(&placed, "main"), keys_under(&placed, "side")), (vec!["w1".to_owned()], vec!["w2".to_owned(), "w3".to_owned()]), "満ちた main の次の Web が side へ行かない");
        // 受ける区画が全部満ちた: 落とさず main（max を超えても）
        let one = PlacementTemplate { pages: vec![PagePlacement { page: "*".into(), regions: vec![region(Slot::Main, Some(1), None)] }], ..two.clone() };
        assert_eq!(keys_under(&place(&three, &p, Some(&one)), "main"), ["w1", "w2", "w3"], "満ちた後の部品を落とした");
        assert_eq!(serde_json::to_value(&two.pages[0].regions[0]).unwrap()["max"], json!(1));
        assert!(serde_json::to_value(&two.pages[0].regions[1]).unwrap().get("max").is_none(), "max の無い区画に max を書いた（前の型の JSON と違う）");
    }
}
