//! unit C（composer）の test: GEN-001〜009・011・012・014、TEST-001〜004・007 の生成側。
//! domain と composer は bin crate の module なので `#[path]` で取り込む（lib target が無い）。
//! 生成した 10 本は docs/examples/generated/ に置き、毎回いま生成した物と比べる（`KANGAE_UPDATE_GENERATED=1` で書き直す）。
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/composer/mod.rs"]
mod composer;

use composer::goal::{GoalPlan, PlanError, SourcePolicy};
use composer::{apply_edit, component_set, compose, edit, undo, validate_plan, verify, BuildPlan, ComposeContext, ComposeError, EditError, ModelProvider};
use domain::action::{validate_invocation, ActionDefinition, ActionInvocation, ActionReadiness, Effect, InvocationError, InvocationOrigin};
use domain::app::{AppDefinitionV3, AppReadiness, ExecutionMode};
use domain::binding::SourceReadiness;
use domain::capability::{CapabilityDescriptor, CapabilityReadiness};
use domain::patch::AppRevision;
use domain::ui::UiNode;
use domain::validate::{validate, validate_generated, ValidationError};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// 先頭 4 本は corpus の prompt（TEST-001〜004）。後ろ 6 本は corpus に無い文。
const PROMPTS: [&str; 10] = [
    "タイマーを作って",
    "在庫を整理したい",
    "研究ノートを作りたい",
    "仕事の状況を把握したい",
    "毎朝の英単語を覚えたい",
    "家計を見える化したい",
    "読書記録をつけたい",
    "ジムの記録を残したい",
    "旅行の計画を立てたい",
    "バグの報告をまとめたい",
];

fn plan(p: &str) -> BuildPlan {
    compose(p, &ComposeContext::default()).unwrap_or_else(|e| panic!("{p}: {e}"))
}
fn app(p: &str) -> AppDefinitionV3 {
    plan(p).into_app()
}
fn has(a: &AppDefinitionV3, component: &str) -> bool {
    a.composition.root.nodes().iter().any(|n| n.component == component)
}
fn first<'a>(a: &'a AppDefinitionV3, component: &str) -> &'a UiNode {
    a.composition.root.nodes().into_iter().find(|n| n.component == component).unwrap_or_else(|| panic!("{}: {component} が無い", a.id))
}
fn errs<T: ToString>(e: &[T]) -> String {
    e.iter().map(ToString::to_string).collect::<Vec<_>>().join(" | ")
}
fn read(rel: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(format!("{ROOT}/{rel}")).expect(rel)).expect(rel)
}

// ---------- 通し: 10 文 → 互いに違う構成・全部 validate を通る（GEN-001 / GEN-006 / GEN-013 / GEN-009） ----------

#[test]
fn ten_goals_compose_valid_apps_with_distinct_shapes() {
    let mut shapes: Vec<(&str, BTreeSet<String>)> = Vec::new();
    let mut ids = HashSet::new();
    for p in PROMPTS {
        let bp = plan(p);
        let a = bp.app().clone();
        validate(&a).unwrap_or_else(|e| panic!("{p}: validate: {}", errs(&e)));
        validate_generated(&a).unwrap_or_else(|e| panic!("{p}: validate_generated: {}", errs(&e)));
        AppRevision::new(a.clone()).unwrap_or_else(|e| panic!("{p}: revision: {}", errs(&e)));
        let v = verify(&a, &[]);
        assert!(v.blockers().is_empty(), "{p}: {:?}", v.blockers());
        assert_eq!(v.readiness, a.state.readiness, "{p}: verifier と state が食い違う");
        assert_eq!(v.smoke.len(), a.actions.iter().filter(|x| x.readiness == ActionReadiness::Ready).count(), "{p}: ready な action は全部 smoke を通る");
        assert_eq!(a.goal.literal_goal, p);
        assert!(ids.insert(a.id.clone()), "{p}: app id '{}' が他と重複", a.id);
        assert!(bp.solutions.len() >= 2 && !bp.rejected.is_empty(), "{p}: GEN-002 は 2 案以上と落とした理由");
        assert!(!a.composition.root.nodes().iter().any(|n| matches!(n.component.as_str(), "kpi" | "metric" | "stat" | "card")), "{p}: KPI 0・card 0（DESIGN §3）");
        shapes.push((p, component_set(&a)));
    }
    let distinct: HashSet<&BTreeSet<String>> = shapes.iter().map(|(_, s)| s).collect();
    assert_eq!(distinct.len(), PROMPTS.len(), "component の集合が重なった:\n{shapes:#?}");
}

#[test]
fn each_shape_follows_the_goal_not_a_template() {
    // タイマー: 一覧を置かない。action は corpus 01 の 3 本と同じ id（runtime の local action）
    let timer = app("タイマーを作って");
    assert!(has(&timer, "text") && !has(&timer, "list") && !has(&timer, "table") && !has(&timer, "timeline"));
    let corpus01: BTreeSet<String> = read("docs/examples/v3/01_timer_local.json")["actions"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
    let ids: BTreeSet<String> = timer.actions.iter().map(|a| a.id.clone()).collect();
    assert_eq!(ids, corpus01, "タイマーの action は corpus 01 の id");
    // 在庫: table + 検索（CD-11 規則 1）+ 数量の変更 + 承認つき削除 + CSV
    let inv = app("在庫を整理したい");
    assert!(has(&inv, "table"));
    assert!(inv.composition.root.nodes().iter().any(|n| n.component == "form" && n.binding.as_ref().map(|b| b.path == "filter").unwrap_or(false)), "在庫には検索");
    for id in ["local.inventory.add", "local.inventory.filter", "local.inventory.update_quantity", "local.inventory.import_csv"] {
        assert!(inv.action(id).is_some(), "在庫に {id}");
    }
    let del = inv.action("local.inventory.delete").unwrap();
    assert!(del.effect == Effect::Destructive && del.requires_approval, "local の削除も承認つき");
    // 研究ノート: editor が主役・資料と引用（出典）
    let notes = app("研究ノートを作りたい");
    assert!(has(&notes, "editor") && notes.action("local.notes.cite").is_some() && notes.source("local.imports").is_some());
    // 英単語: 1 枚の detail と「覚えた」（表は出さない）。毎朝 → 今日の残り
    let words = app("毎朝の英単語を覚えたい");
    assert!(has(&words, "detail") && has(&words, "tabs") && !has(&words, "table"));
    assert!(words.action("local.words.toggle_done").is_some());
    assert!(words.composition.root.nodes().iter().any(|n| n.label.as_deref() == Some("Left today")), "毎朝 → 今日の残り");
    // 家計: chart は見える化の時だけ。理由が metadata に残り、同じ数字の table も在る
    let ledger = app("家計を見える化したい");
    let chart = first(&ledger, "chart");
    assert!(ledger.metadata.as_ref().unwrap()["composer"]["rationale"][&chart.key].as_str().map(|s| s.contains("見える化")).unwrap_or(false), "chart の理由");
    assert!(has(&ledger, "table"));
    for p in PROMPTS.iter().filter(|p| !p.contains("見える化")) {
        assert!(!has(&app(p), "chart"), "{p}: 頼まれていない chart を置かない");
    }
    // 読書記録: 日付の timeline + 感想の editor + 状態の変更
    let books = app("読書記録をつけたい");
    assert!(has(&books, "timeline") && has(&books, "editor") && books.action("local.books.update_status").is_some());
    // ジム: 入力 form を常設（数の欄）+ timeline
    let gym = app("ジムの記録を残したい");
    let entry = gym.composition.root.nodes().into_iter().find(|n| n.key == "workouts-entry").expect("ジムの入力 form");
    assert!(entry.props["fields"].as_array().unwrap().iter().any(|f| f["input"] == "number"), "重さは数の欄");
    assert!(has(&gym, "timeline"));
    // 旅行: calendar + 持ち物・準備の list（押すと済み）
    let trip = app("旅行の計画を立てたい");
    assert!(has(&trip, "calendar"));
    let check = trip.composition.root.nodes().into_iter().find(|n| n.key == "checklist-list").expect("持ち物・準備");
    assert_eq!(check.action_ref.as_deref(), Some("local.checklist.toggle_done"));
    // バグ: table + 内容の editor + 状態の変更
    let bugs = app("バグの報告をまとめたい");
    assert!(has(&bugs, "table") && has(&bugs, "editor") && bugs.action("local.reports.update_status").is_some());
    // 仕事の状況: 状態で束ねた timeline + 任意の取得元（Slack / GitHub を推定しない: TEST-004）
    let work = app("仕事の状況を把握したい");
    assert_eq!(first(&work, "timeline").props["group_by"], json!("status"));
    let raw = serde_json::to_string(&work).unwrap().to_lowercase();
    for svc in ["slack", "github", "google", "android", "iphone", "gmail"] {
        assert!(!raw.contains(svc), "仕事の状況に '{svc}' を推定した");
    }
}

// ---------- 接続が要る物の正直さ（GEN-003 / TEST-004 / TEST-005 の生成側） ----------

#[test]
fn unconnected_actions_are_never_ready_and_say_why() {
    for p in ["仕事の状況を把握したい", "複数サービスのチャットをまとめたい"] {
        let a = app(p);
        let externals: Vec<&ActionDefinition> = a.actions.iter().filter(|x| !x.id.starts_with("local.")).collect();
        assert!(!externals.is_empty(), "{p}: 接続が要る action が在る");
        for x in &externals {
            assert_eq!(x.readiness, ActionReadiness::RequiresConnection, "{p}: {}", x.id);
            let cap = a.capability(x.capability_ref.as_deref().unwrap()).unwrap();
            assert_ne!(cap.readiness, CapabilityReadiness::Ready, "{p}: {}", cap.id);
            assert!(cap.provider.is_none(), "{p}: 名指しされていない provider を決めた: {:?}", cap.provider);
            let inv = ActionInvocation { action_ref: x.id.clone(), origin: InvocationOrigin::Agent { suggestion_id: "s1".into() }, input: json!({}) };
            assert!(matches!(validate_invocation(&a, &inv), Err(InvocationError::NotReady { .. })), "{p}: {} は実行できない", x.id);
        }
        for n in a.composition.root.nodes() {
            if let Some(x) = n.action_ref.as_deref().and_then(|id| a.action(id)) {
                if x.readiness != ActionReadiness::Ready {
                    assert!(n.states.as_ref().and_then(|s| s.get("disabled")).and_then(Value::as_str).map(|t| !t.is_empty()).unwrap_or(false), "{p}: {} に押せない理由", n.key);
                }
            }
            if let Some(src) = n.binding.as_ref().and_then(|b| a.source(&b.source_ref)) {
                if src.readiness != SourceReadiness::Local {
                    assert!(n.states.as_ref().and_then(|s| s.get("requires_connection")).is_some(), "{p}: {} は未接続と言う", n.key);
                }
            }
        }
        assert_eq!(a.state.readiness, AppReadiness::Partial, "{p}: 端末内だけ動く = partial");
        assert!(a.composition.root.nodes().iter().any(|n| n.action_ref.as_deref() == Some("local.connection.open")), "{p}: 繋ぐ候補への入口（GEN-003）");
    }
    let chat = app("複数サービスのチャットをまとめたい");
    let send = chat.actions.iter().find(|x| x.effect == Effect::ExternalSend).expect("送信");
    assert!(send.requires_approval, "送信は承認つき");
    assert!(chat.workflows.iter().any(|w| w.action_refs.contains(&send.id) && w.approval.as_deref() == Some("exact_payload")));
}

#[test]
fn registry_readiness_is_taken_as_is_never_invented() {
    let cap = |r| CapabilityDescriptor { id: "tasks.read".into(), provider: Some("tracker".into()), readiness: r, required_scopes: vec!["tasks.read".into()], adapter: Some("api".into()) };
    // 何も繋いでいない: unknown（connector を捏造しない）
    let none = app("仕事の状況を把握したい");
    assert_eq!(none.capability("tasks.read").unwrap().readiness, CapabilityReadiness::Unknown);
    // registry が ready と言った時だけ ready・integrated
    let ctx = ComposeContext { registry: vec![cap(CapabilityReadiness::Ready)], ..Default::default() };
    let ready = compose("仕事の状況を把握したい", &ctx).unwrap().into_app();
    assert_eq!(ready.action("tasks.read").unwrap().readiness, ActionReadiness::Ready);
    assert_eq!(ready.source("optional.tasks").unwrap().readiness, SourceReadiness::Ready);
    assert_eq!((ready.state.readiness, ready.state.execution_mode), (AppReadiness::Ready, ExecutionMode::Integrated));
    validate_generated(&ready).unwrap_or_else(|e| panic!("{}", errs(&e)));
    assert!(verify(&ready, &ctx.registry).blockers().is_empty());
    // registry が接続待ちと言えば、そのまま接続待ち
    let ctx = ComposeContext { registry: vec![cap(CapabilityReadiness::RequiresConnection)], ..Default::default() };
    let waiting = compose("仕事の状況を把握したい", &ctx).unwrap().into_app();
    assert_eq!(waiting.action("tasks.read").unwrap().readiness, ActionReadiness::RequiresConnection);
    assert_eq!(waiting.capability("tasks.read").unwrap().provider.as_deref(), Some("tracker"));
}

#[test]
fn local_goals_run_without_connections_or_questions() {
    for p in PROMPTS.iter().filter(|p| !p.contains("仕事")) {
        let bp = plan(p);
        let a = bp.app();
        assert_eq!(bp.goal_plan.source_policy, SourcePolicy::LocalOnly, "{p}");
        assert!(bp.goal_plan.questions.is_empty(), "{p}: local で済む目的に質問しない（GEN-007）");
        assert!(a.capabilities.is_empty() && a.actions.iter().all(|x| x.readiness == ActionReadiness::Ready && !x.effect.is_external()), "{p}");
        assert_eq!((a.state.readiness, a.state.execution_mode), (AppReadiness::Ready, ExecutionMode::LocalReady), "{p}");
        assert!(a.sources.iter().all(|s| s.readiness == SourceReadiness::Local), "{p}");
    }
}

// ---------- 質問の予算（GEN-008 / CD-05: 0 問・1 問・目的そのものが読めない） ----------

#[test]
fn clarification_budget_is_zero_one_or_one_blocking() {
    assert!(plan("タイマーを作って").goal_plan.questions.is_empty());
    let chat = plan("複数サービスのチャットをまとめたい");
    assert_eq!(chat.goal_plan.questions.len(), 1, "どのアカウントか、だけ");
    assert!(!chat.goal_plan.questions[0].is_blocking(), "既定つき = preview を止めない");
    match compose("便利にして", &ComposeContext::default()) {
        Err(ComposeError::Clarify(p)) => {
            validate_plan(&p).unwrap_or_else(|e| panic!("{}", errs(&e)));
            assert_eq!(p.questions.iter().filter(|q| q.is_blocking()).count(), 1, "blocking は 1 つだけ");
            assert!(p.ui_intents.len() >= 2, "選べる候補を出す");
        }
        other => panic!("便利にして: 1 問返すはず: {:?}", other.map(|b| b.app().id.clone())),
    }
    assert!(matches!(compose("検索を追加", &ComposeContext::default()), Err(ComposeError::EditRequest(_))), "変更の依頼は新しい app にしない");
    assert!(matches!(compose("  ", &ComposeContext::default()), Err(ComposeError::Empty)));
}

// ---------- 一般化: corpus にも 10 文にも無い文（holdout は見ていない。ここは自分で足した形の違う文） ----------

#[test]
fn unseen_goals_still_compose_valid_apps() {
    let prompts = [
        "植物の水やりを記録したい",
        "ペットの健康を管理したい",
        "体重を見える化したい",
        "引っ越しの準備をしたい",
        "議事録を書きたい",
        "Slack と GitHub をまとめたい",
        "スマホの画面を Mac で見て操作したい",
        "動画を編集したい",
        "鼻歌を録音して Reaper に入れたい",
        "make a pomodoro timer",
        "顧客の連絡先を整理したい",
        "子どもの宿題の進捗を把握したい",
    ];
    for p in prompts {
        let a = app(p);
        validate_generated(&a).unwrap_or_else(|e| panic!("{p}: {}", errs(&e)));
        let v = verify(&a, &[]);
        assert!(v.blockers().is_empty(), "{p}: {:?}", v.blockers());
        assert!(!a.goal.outcomes.is_empty() && a.title.as_deref().map(|t| !t.is_empty()).unwrap_or(false), "{p}");
    }
    // 表に無い物でも、述語で形が決まり、利用者の言葉が label になる
    let water = app("植物の水やりを記録したい");
    assert!(has(&water, "timeline"));
    assert!(water.composition.root.nodes().iter().any(|n| n.label.as_deref().map(|l| l.contains("植物の水やり")).unwrap_or(false)));
    // 名指しされたサービスだけ provider に残る
    let named = app("Slack と GitHub をまとめたい");
    let providers: BTreeSet<String> = named.capabilities.iter().filter_map(|c| c.provider.clone()).collect();
    assert_eq!(providers, BTreeSet::from(["github".to_string(), "slack".to_string()]));
    assert!(named.capabilities.iter().all(|c| c.readiness == CapabilityReadiness::RequiresConnection), "名指し = 選択済み・未認証");
    assert!(has(&named, "chat") && !named.composition.root.nodes().iter().any(|n| n.label.as_deref() == Some("Slack と GitHub")), "サービス名を物の名前にしない（会話をまとめる）");
    // 「準備」→ 日付と持ち物・準備の確認
    let moving = app("引っ越しの準備をしたい");
    assert!(has(&moving, "calendar") && moving.source("local.checklist").is_some());
    // 見える化の値は利用者の言葉で呼ぶ
    assert!(first(&app("体重を見える化したい"), "chart").label.as_deref().map(|l| l.contains("体重")).unwrap_or(false));
    // 端末は OS を決め打ちしない・入力は承認つき
    let mirror = app("スマホの画面を Mac で見て操作したい");
    assert_eq!(first(&mirror, "source_surface").props["display"], json!("mirror"));
    assert!(mirror.action("device.input.send").unwrap().requires_approval);
}

// ---------- 編集: 該当 subtree だけ（GEN-011 / GEN-014 / TEST-007） ----------

fn flat(root: &UiNode) -> BTreeMap<String, Value> {
    root.nodes()
        .into_iter()
        .map(|n| {
            let mut v = serde_json::to_value(n).unwrap();
            v.as_object_mut().unwrap().remove("children");
            (n.key.clone(), v)
        })
        .collect()
}

/// edit を当てて、affected_keys の外の node が 1 つも変わらない・action / source / data の定義が変わらない・version + 1・
/// undo で元の composition に戻る、を確かめる。当てた後の revision を返す。
fn apply_and_check(a: &AppDefinitionV3, instruction: &str, selected: Option<&str>) -> (composer::Edit, AppDefinitionV3) {
    let base = AppRevision::new(a.clone()).unwrap();
    let e = edit(a, instruction, selected).unwrap_or_else(|err| panic!("{instruction}: {err}"));
    let next = apply_edit(&base, &e).unwrap_or_else(|err| panic!("{instruction}: {err}"));
    let after = next.definition().clone();
    assert_eq!(after.version, a.version + 1, "{instruction}: version + 1");
    assert_eq!((&after.actions, &after.sources, &after.capabilities, &after.state), (&a.actions, &a.sources, &a.capabilities, &a.state), "{instruction}: data / action は触らない");
    let (before_nodes, after_nodes) = (flat(&a.composition.root), flat(&after.composition.root));
    // 消した node の子孫は一緒に消えてよい（消したのはその subtree だけ）
    let mut affected: HashSet<String> = e.affected_keys.iter().cloned().collect();
    for k in &e.affected_keys {
        if let (Some(n), false) = (a.composition.root.find(k), after_nodes.contains_key(k)) {
            affected.extend(n.nodes().into_iter().map(|d| d.key.clone()));
        }
    }
    for (k, v) in &before_nodes {
        if affected.contains(k) {
            continue;
        }
        match after_nodes.get(k) {
            Some(w) => assert_eq!(v, w, "{instruction}: 対象外の '{k}' が変わった"),
            None => panic!("{instruction}: 対象外の '{k}' が消えた"),
        }
    }
    for k in after_nodes.keys() {
        assert!(before_nodes.contains_key(k) || affected.contains(k), "{instruction}: 対象外の node '{k}' が増えた");
    }
    let back = undo(&next, &e).unwrap_or_else(|err| panic!("{instruction}: undo: {err}"));
    assert_eq!(back.definition().composition, a.composition, "{instruction}: undo で元の構成に戻る");
    (e, after)
}

#[test]
fn edits_touch_only_the_referenced_subtree_and_undo() {
    // 「検索を追加」: 検索の無い一覧（ジムの timeline）に、split の比率を崩さず上に足す
    let gym = app("ジムの記録を残したい");
    let (e, after) = apply_and_check(&gym, "検索を追加", None);
    let search = after.composition.root.find(e.affected_keys.last().unwrap()).unwrap();
    assert_eq!((search.component.as_str(), search.action_ref.as_deref()), ("form", Some("local.workouts.filter")));
    assert_eq!(e.affected_keys.len(), 2, "足した node と、その親だけ");
    // GEN-014「この表を検索可能に」: 表だけが対象（家計の明細）
    let ledger = app("家計を見える化したい");
    let (e, _) = apply_and_check(&ledger, "この表を検索可能に", None);
    assert!(e.summary.contains("details"), "{}", e.summary);
    // 「この表を並び替え可能に」: 選んだ table の props だけ
    let inv = app("在庫を整理したい");
    let (e, after) = apply_and_check(&inv, "この表を並び替え可能に", Some("inventory-table"));
    assert_eq!(e.affected_keys, vec!["inventory-table".to_string()]);
    assert_eq!(after.composition.root.find("inventory-table").unwrap().props["sortable"], json!(true));
    // 「この欄を消して」: 「この」= 選んでいる部品。選んだ検索欄だけが消える
    let (e, after) = apply_and_check(&inv, "この欄を消して", Some("inventory-search"));
    assert!(after.composition.root.find("inventory-search").is_none());
    assert_eq!(e.affected_keys, vec!["inventory-root".to_string(), "inventory-search".to_string()]);
    // 選んでいない「この欄」は推測で消さない・既に在る物は足さない
    assert!(matches!(edit(&inv, "この欄を消して", None), Err(EditError::NoTarget(_))));
    assert!(matches!(edit(&inv, "検索を追加", None), Err(EditError::AlreadyApplied(_))));
    // split の子を外したら比率も詰める（DESIGN §7 の検査を通る）
    let (_, after) = apply_and_check(&inv, "これを消して", Some("inventory-selected"));
    assert_eq!(after.composition.root.find("inventory-split").unwrap().layout.as_ref().unwrap()["ratio"], json!([1.0]));
    assert!(verify(&after, &[]).blockers().is_empty());
}

// ---------- model の口（GEN-012）: 幻覚の action id は拒否・既定は off ----------

struct Fixed(Result<Value, String>);
impl ModelProvider for Fixed {
    fn complete_json(&self, _prompt: &str, schema: &Value) -> Result<Value, String> {
        assert!(schema["properties"]["schema_version"]["const"] == "3.0", "schema は app v3");
        self.0.clone()
    }
}

fn with_model(goal: &str, reply: Result<Value, String>) -> BuildPlan {
    let m = Fixed(reply);
    compose(goal, &ComposeContext { model: Some(&m), ..Default::default() }).unwrap()
}

#[test]
fn model_output_is_used_only_after_it_passes_every_check() {
    let goal = "在庫を整理したい";
    let base = app(goal);
    let base_json = serde_json::to_value(&base).unwrap();
    // (a) 外部の幻覚: 繋いでいない GitHub を ready で
    let mut fake = base_json.clone();
    fake["capabilities"] = json!([{ "id": "github.issues.read", "provider": "github", "readiness": "ready", "required_scopes": [], "adapter": "api" }]);
    fake["actions"].as_array_mut().unwrap().push(json!({ "id": "github.issues.read", "capability_ref": "github.issues.read", "effect": "external_read", "readiness": "ready", "input_schema": { "type": "object" }, "required_scopes": [] }));
    let p = with_model(goal, Ok(fake));
    assert_eq!(p.interpreter, "rule_based");
    assert!(p.errors.iter().any(|e| e.contains("github.issues.read")), "{:?}", p.errors);
    assert!(p.app().action("github.issues.read").is_none(), "幻覚は採らない");
    assert_eq!(p.app().metadata.as_ref().unwrap()["composer"]["errors"][0].as_str().map(|e| e.contains("幻覚")), Some(true), "捨てた理由が残る（GEN-010）");
    // (b) local の幻覚: runtime に無い verb
    let mut fake = base_json.clone();
    fake["actions"].as_array_mut().unwrap().push(json!({ "id": "local.inventory.teleport", "effect": "local_write", "readiness": "ready", "input_schema": { "type": "object" }, "required_scopes": [] }));
    let p = with_model(goal, Ok(fake));
    assert!(p.interpreter == "rule_based" && p.errors.iter().any(|e| e.contains("local.inventory.teleport")), "{:?}", p.errors);
    // (b') 繋いでいない新しい接続先を、ready にはせず足す（readiness の検査は通るが、discover していない connector の捏造）
    let mut fake = base_json.clone();
    fake["capabilities"] = json!([{ "id": "notion.pages.read", "provider": "notion", "readiness": "requires_connection", "required_scopes": [] }]);
    fake["actions"].as_array_mut().unwrap().push(json!({ "id": "notion.pages.read", "capability_ref": "notion.pages.read", "effect": "external_read", "readiness": "requires_connection", "input_schema": { "type": "object" }, "required_scopes": [] }));
    fake["state"] = json!({ "readiness": "partial", "execution_mode": "local_ready" });
    let p = with_model(goal, Ok(fake));
    assert!(p.interpreter == "rule_based" && p.errors.iter().any(|e| e.contains("notion.pages.read")), "{:?}", p.errors);
    // (c) JSON でない・schema に合わない
    assert_eq!(with_model(goal, Err("timeout".into())).interpreter, "rule_based");
    assert_eq!(with_model(goal, Ok(json!({ "schema_version": "3.0", "id": "x" }))).interpreter, "rule_based");
    // (d) 規則を全部通る案は採る（題名だけ変えた案）
    let mut ok = base_json.clone();
    ok["title"] = json!("倉庫の在庫");
    let p = with_model(goal, Ok(ok));
    assert_eq!((p.interpreter.as_str(), p.app().title.as_deref()), ("model", Some("倉庫の在庫")));
    assert_eq!(p.app().metadata.as_ref().unwrap()["composer"]["interpreter"], json!("model"));
}

// ---------- BuildVerifier（GEN-009）: 未完成の app は ready にならない ----------

#[test]
fn incomplete_apps_are_never_ready() {
    let inv = app("在庫を整理したい");
    assert!(verify(&inv, &[]).is_ready());
    // (a) runtime に無い verb の action を ready で、button から押せる
    let mut a = inv.clone();
    a.actions.push(ActionDefinition { id: "local.inventory.teleport".into(), capability_ref: None, effect: Effect::LocalWrite, readiness: ActionReadiness::Ready, input_schema: Map::new(), output_schema: None, required_scopes: vec![], verifier: None, requires_approval: false });
    a.composition.root.children.push(UiNode { key: "teleport".into(), component: "button".into(), action_ref: Some("local.inventory.teleport".into()), ..Default::default() });
    let v = verify(&a, &[]);
    assert!(!v.is_ready() && v.blockers().iter().any(|f| f.subject == "local.inventory.teleport"), "{:?}", v.findings);
    // (b) 一覧の空の文を消す（DESIGN §11）
    let mut b = inv.clone();
    b.composition.root.find_mut("inventory-table").unwrap().states = None;
    assert!(validate_generated(&b).unwrap_err().contains(&ValidationError::MissingState { key: "inventory-table".into(), state: "empty" }));
    assert!(!verify(&b, &[]).is_ready());
    // (c) 繋いでいない取得元を ready と名乗る（registry は接続待ちと言っている）
    let work = app("仕事の状況を把握したい");
    let mut c = work.clone();
    c.capabilities.iter_mut().for_each(|x| x.readiness = CapabilityReadiness::Ready);
    c.actions.iter_mut().for_each(|x| x.readiness = ActionReadiness::Ready);
    c.sources.iter_mut().filter(|s| s.readiness != SourceReadiness::Local).for_each(|s| s.readiness = SourceReadiness::Ready);
    c.state = composer::compiler::state_for(&c.actions, &c.sources);
    let registry = vec![CapabilityDescriptor { id: "tasks.read".into(), provider: None, readiness: CapabilityReadiness::RequiresConnection, required_scopes: vec![], adapter: None }];
    let v = verify(&c, &registry);
    assert!(!v.is_ready() && v.blockers().iter().any(|f| f.subject == "tasks.read"), "{:?}", v.findings);
    // (d) 主役が 60% 未満の split（DESIGN §7）
    let mut d = inv.clone();
    d.composition.root.find_mut("inventory-split").unwrap().layout = Some([("ratio".to_string(), json!([0.5, 0.5]))].into_iter().collect());
    assert!(!verify(&d, &[]).is_ready());
}

// ---------- GoalPlan（CD-10 / CD-05） ----------

/// 数の表し方（1 と 1.0）を揃えて比べる。
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        x => x.clone(),
    }
}

#[test]
fn goal_plan_is_the_schema_one_to_one_and_cd05_is_enforced() {
    let mut n = 0;
    for entry in fs::read_dir(format!("{ROOT}/docs/examples")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if !(name.starts_with("goal_") && name.ends_with(".json")) {
            continue;
        }
        n += 1;
        let raw = read(&format!("docs/examples/{name}"));
        let p: GoalPlan = serde_json::from_value(raw.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(norm(&serde_json::to_value(&p).unwrap()), norm(&raw), "{name}: round-trip で field が欠けた / 増えた");
        validate_plan(&p).unwrap_or_else(|e| panic!("{name}: {}", errs(&e)));
    }
    assert_eq!(n, 5, "goal_*.json は 5 本");
    // 負の対照（CD-05 の検査の仕方）
    let mut m: GoalPlan = serde_json::from_value(read("docs/examples/goal_messages.json")).unwrap();
    m.questions[0].suggested_default = None;
    let mut extra = m.questions[0].clone();
    extra.id = "second".into();
    m.questions.push(extra);
    assert!(validate_plan(&m).unwrap_err().contains(&PlanError::BlockingQuestions(2)));
    let mut t: GoalPlan = serde_json::from_value(read("docs/examples/goal_timer_local.json")).unwrap();
    let mut q = m.questions[1].clone();
    q.blocking_need_id = t.needs[0].id.clone();
    t.questions.push(q);
    assert!(validate_plan(&t).unwrap_err().contains(&PlanError::LocalOnlyHasQuestions));
    let mut r: GoalPlan = serde_json::from_value(read("docs/examples/goal_messages.json")).unwrap();
    r.questions.clear();
    r.candidate_sources.clear();
    assert!(validate_plan(&r).unwrap_err().contains(&PlanError::RequestedWithoutSource));
    let mut bad = read("docs/examples/goal_inventory.json");
    bad["requirements"][0]["origin"] = json!("UserConfirmed");
    assert!(serde_json::from_value::<GoalPlan>(bad).is_err(), "schema に無い origin は読めない");
    let mut unknown = read("docs/examples/goal_inventory.json");
    unknown["blocking"] = json!(true);
    assert!(serde_json::from_value::<GoalPlan>(unknown).is_err(), "schema に無い field は読めない");
}

// ---------- validate_generated の 2 段（CD-14） ----------

#[test]
fn corpus_passes_the_generated_gate_and_design_samples_stay_loose() {
    let dir = format!("{ROOT}/docs/examples/v3");
    let mut names: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".json")).collect();
    names.sort();
    assert_eq!(names.len(), 10);
    for n in names {
        let def: AppDefinitionV3 = serde_json::from_value(read(&format!("docs/examples/v3/{n}"))).unwrap();
        validate_generated(&def).unwrap_or_else(|e| panic!("{n}: {}", errs(&e)));
    }
    // 設計サンプルは緩い validate だけ（README §1）。messages の props.readiness は V6 で名指しされる
    let messages: AppDefinitionV3 = serde_json::from_value(read("docs/examples/app_v3_messages.json")).unwrap();
    assert!(validate(&messages).is_ok());
    assert!(validate_generated(&messages).unwrap_err().iter().any(|e| matches!(e, ValidationError::ForbiddenProp { prop, .. } if prop == "readiness")));
}

// ---------- 生成した 10 本の保存（docs/examples/generated/） ----------

#[test]
fn generated_examples_on_disk_match_what_compose_returns() {
    let dir = format!("{ROOT}/docs/examples/generated");
    let update = std::env::var("KANGAE_UPDATE_GENERATED").is_ok();
    fs::create_dir_all(&dir).unwrap();
    let mut stale = Vec::new();
    for (i, p) in PROMPTS.iter().enumerate() {
        let a = app(p);
        let path = format!("{dir}/{:02}_{}.json", i + 1, a.id.trim_start_matches("generated."));
        let now = serde_json::to_value(&a).unwrap();
        match fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            Some(saved) if saved == now => {}
            Some(_) if !update => stale.push(path),
            _ => fs::write(&path, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap(),
        }
    }
    assert!(stale.is_empty(), "保存した生成物が古い（KANGAE_UPDATE_GENERATED=1 で書き直す）: {stale:?}");
}

// ---------- 審査 R1 の指摘（C-R1-01〜08・SEC-C1〜3）: 直す前に赤くなる再現 ----------

fn ready_caps(ids: &[&str]) -> Vec<CapabilityDescriptor> {
    ids.iter().map(|id| CapabilityDescriptor { id: (*id).into(), provider: None, readiness: CapabilityReadiness::Ready, required_scopes: vec![(*id).into()], adapter: Some("api".into()) }).collect()
}

fn with_model_ctx(goal: &str, reply: Value, registry: &[CapabilityDescriptor], app_id: Option<&str>) -> BuildPlan {
    let m = Fixed(Ok(reply));
    compose(goal, &ComposeContext { app_id: app_id.map(Into::into), registry: registry.to_vec(), model: Some(&m), ..Default::default() }).unwrap_or_else(|e| panic!("{goal}: {e}"))
}

/// C-R1-01: model の案の理由（metadata.composer.rationale）を上書きで消さない。返す app ＝ 検査した app。
#[test]
fn r1_01_model_rationale_is_kept_and_the_returned_app_is_the_verified_one() {
    let goal = "在庫を整理したい";
    let mut reply = serde_json::to_value(app(goal)).unwrap();
    let chart = json!({ "key": "stock-chart", "component": "chart", "label": "在庫の数量", "binding": { "source_ref": "local.inventory", "path": "aggregate" },
        "props": { "kind": "bar", "x": "name", "y": "quantity", "aggregate": "sum" }, "states": { "empty": "在庫を足すと、ここに数量が出る" } });
    reply["composition"]["root"]["children"].as_array_mut().unwrap().push(chart);
    reply["metadata"]["composer"]["rationale"]["stock-chart"] = json!("在庫の数量を品目ごとに比べる（model の案）");
    let p = with_model(goal, Ok(reply.clone()));
    assert_eq!(p.interpreter, "model", "{:?}", p.errors);
    validate_generated(p.app()).unwrap_or_else(|e| panic!("返した app が規則に落ちる: {}", errs(&e)));
    assert_eq!(p.app().metadata.as_ref().unwrap()["composer"]["rationale"]["stock-chart"], json!("在庫の数量を品目ごとに比べる（model の案）"));
    assert_eq!(p.app().metadata.as_ref().unwrap()["composer"]["interpreter"], json!("model"));
    assert_eq!(p.verdict, verify(p.app(), &[]), "verdict は返した app の物");
    // 理由の無い chart は採らない（理由を捏造して通さない）。捨てた時も返す app は規則を通る
    reply["metadata"]["composer"]["rationale"].as_object_mut().unwrap().remove("stock-chart");
    let p = with_model(goal, Ok(reply));
    assert_eq!(p.interpreter, "rule_based");
    validate_generated(p.app()).unwrap_or_else(|e| panic!("{}", errs(&e)));
    assert_eq!(p.verdict, verify(p.app(), &[]));
}

/// C-R1-02 / SEC-C3: model の返答で app の id と version を変えさせない（id で保存する呼び側が別の app を上書きしない）。
#[test]
fn r1_02_model_cannot_change_the_app_id_or_version() {
    let goal = "在庫を整理したい";
    let base = compose(goal, &ComposeContext { app_id: Some("user.app1".into()), ..Default::default() }).unwrap().into_app();
    assert_eq!((base.id.as_str(), base.version), ("user.app1", 1));
    for (id, version, says) in [("someone.else", 1, "someone.else"), ("user.app1", 99, "version"), ("generated.messages_monitor", 1, "generated.messages_monitor")] {
        let mut reply = serde_json::to_value(&base).unwrap();
        reply["id"] = json!(id);
        reply["version"] = json!(version);
        let p = with_model_ctx(goal, reply, &[], Some("user.app1"));
        assert_eq!((p.interpreter.as_str(), p.app().id.as_str(), p.app().version), ("rule_based", "user.app1", 1), "{id} v{version}: {:?}", p.errors);
        assert!(p.errors.iter().any(|e| e.contains(says)), "{id} v{version}: {:?}", p.errors);
    }
}

/// Slack の送信も GitHub の読み込みも繋いだ（registry で ready）状態の下書き。
fn slack_github() -> (&'static str, Vec<CapabilityDescriptor>, AppDefinitionV3) {
    let goal = "Slack と GitHub をまとめたい";
    let registry = ready_caps(&["slack.messages.read", "slack.messages.send", "github.issues.read"]);
    let base = compose(goal, &ComposeContext { registry: registry.clone(), ..Default::default() }).unwrap().into_app();
    let send = base.action("slack.messages.send").expect("前提: Slack の送信");
    assert!(send.readiness == ActionReadiness::Ready && send.effect == Effect::ExternalSend && send.requires_approval, "前提: {send:?}");
    assert_eq!(base.action("github.issues.read").map(|a| a.readiness), Some(ActionReadiness::Ready), "前提: GitHub の読み込み");
    (goal, registry, base)
}

/// SEC-C1: 外への action の effect・承認は下書きのまま（送信を local_read・承認なしと自己申告させない）。
#[test]
fn r1_sec_c1_model_cannot_lower_the_effect_of_a_send() {
    let (goal, registry, base) = slack_github();
    // 送信の effect を local_read・承認なしと自己申告し、承認の workflow も消す
    let mut laundered = serde_json::to_value(&base).unwrap();
    for a in laundered["actions"].as_array_mut().unwrap() {
        if a["id"] == "slack.messages.send" {
            a["effect"] = json!("local_read");
            a["requires_approval"] = json!(false);
        }
    }
    laundered["workflows"] = json!([]);
    let p = with_model_ctx(goal, laundered, &registry, None);
    assert_eq!(p.interpreter, "rule_based", "effect を下げた案を採った: {:?}", p.errors);
    assert!(p.errors.iter().any(|e| e.contains("slack.messages.send")), "{:?}", p.errors);
    let send = p.app().action("slack.messages.send").unwrap();
    assert!(send.effect == Effect::ExternalSend && send.requires_approval && !p.app().workflows.is_empty());
}

/// C-R1-03: 許した capability の下でも、下書きに無い外への action（架空の id）は採らない。
#[test]
fn r1_03_model_cannot_add_a_made_up_action_under_an_allowed_capability() {
    let (goal, registry, base) = slack_github();
    // 読むだけの能力の下に、架空の destructive な action を ready で足す
    let mut fabricated = serde_json::to_value(&base).unwrap();
    fabricated["actions"].as_array_mut().unwrap().push(json!({ "id": "github.repo.delete", "capability_ref": "github.issues.read", "effect": "destructive", "readiness": "ready", "input_schema": { "type": "object" }, "required_scopes": [], "requires_approval": true }));
    let p = with_model_ctx(goal, fabricated, &registry, None);
    assert!(p.interpreter == "rule_based" && p.errors.iter().any(|e| e.contains("github.repo.delete")), "{:?}", p.errors);
    assert!(p.app().action("github.repo.delete").is_none());
}

/// SEC-C2: model に許す capability はこの目的に discover した物だけ（registry に ready で在っても、計画に無い能力を足させない）。
#[test]
fn r1_sec_c2_model_cannot_add_other_registry_capabilities() {
    // local だけの目的に、registry で ready な別の能力（Gmail）を足す
    let ledger_goal = "家計を見える化したい";
    let gmail = ready_caps(&["gmail.mail.read"]);
    let mut widened = serde_json::to_value(app(ledger_goal)).unwrap();
    widened["capabilities"] = json!([{ "id": "gmail.mail.read", "provider": "google", "readiness": "ready", "required_scopes": ["gmail.mail.read"], "adapter": "api" }]);
    widened["actions"].as_array_mut().unwrap().push(json!({ "id": "gmail.mail.read", "capability_ref": "gmail.mail.read", "effect": "external_read", "readiness": "ready", "input_schema": { "type": "object" }, "required_scopes": ["gmail.mail.read"] }));
    let p = with_model_ctx(ledger_goal, widened, &gmail, None);
    assert!(p.interpreter == "rule_based" && p.errors.iter().any(|e| e.contains("gmail.mail.read")), "{:?}", p.errors);
    assert!(p.app().capabilities.is_empty() && p.app().action("gmail.mail.read").is_none(), "local だけの目的に外の能力を足さない");
}

/// SEC-C1（verifier 側）: 外の能力を通る action が local の effect を名乗ると承認の gate を素通りする → Blocker。組み込み（mic・handoff）は除く。
#[test]
fn r1_sec_c1_verifier_blocks_a_local_effect_through_an_external_capability() {
    let (_, registry, mut a) = slack_github();
    assert!(verify(&a, &registry).blockers().is_empty());
    let send = a.actions.iter_mut().find(|x| x.id == "slack.messages.send").unwrap();
    send.effect = Effect::LocalRead;
    send.requires_approval = false;
    a.workflows.clear();
    validate_generated(&a).unwrap_or_else(|e| panic!("前提: domain の検査は通ってしまう形: {}", errs(&e)));
    let v = verify(&a, &registry);
    assert!(!v.is_ready() && v.blockers().iter().any(|f| f.subject == "slack.messages.send" && f.check == "permissions"), "{:?}", v.findings);
    for p in ["鼻歌を録音して Reaper に入れたい", "動画を編集したい"] {
        assert!(verify(&app(p), &[]).blockers().is_empty(), "{p}: 組み込みの能力（mic・editor）は local の effect のままでよい");
    }
}

/// C-R1-04: action の required の欄は form から外さない（外すと、その form は以後必ず MissingInput で落ちる）。
#[test]
fn r1_04_a_field_the_action_requires_is_not_removed() {
    let books = app("読書記録をつけたい");
    let err = edit(&books, "この題名の欄を消して", Some("books-entry")).expect_err("題名は local.books.add の required");
    assert!(err.to_string().contains("Title"), "{err}");
    let ledger = app("家計を見える化したい");
    let err = edit(&ledger, "金額の欄を消して", Some("ledger-entry")).expect_err("金額は local.ledger.add の required");
    assert!(err.to_string().contains("Amount"), "{err}");
    // required でない欄は今までどおり外せる
    let (_, after) = apply_and_check(&books, "この著者の欄を消して", Some("books-entry"));
    let fields = after.composition.root.find("books-entry").unwrap().props["fields"].as_array().unwrap().clone();
    assert!(fields.iter().all(|f| f["name"] != "author") && fields.iter().any(|f| f["name"] == "title"));
}

/// C-R1-05: 名前で指した「消して」は、文に含まれる一番長い label の node（「Selected 在庫」を「在庫」と読まない）。
#[test]
fn r1_05_remove_by_name_takes_the_longest_label_in_the_sentence() {
    let inv = app("在庫を整理したい");
    let e = edit(&inv, "Selected 在庫を消して", None).unwrap();
    assert_eq!(e.patches[0].target_key, "inventory-selected", "{}", e.summary);
    let (_, after) = apply_and_check(&inv, "Selected 在庫を消して", None);
    assert!(after.composition.root.find("inventory-table").is_some(), "主役の一覧を消さない");
}

/// C-R1-05 と同じ読み方を、一覧を名前で指す編集（並び替え・検索）にも。
#[test]
fn r1_05b_a_collection_by_name_takes_the_longest_label_too() {
    let inv = app("在庫を整理したい");
    let mut two = inv.clone();
    let mut history = two.composition.root.find("inventory-table").unwrap().clone();
    history.key = "inventory-history".into();
    history.component = "list".into();
    history.label = Some("在庫の履歴".into());
    two.composition.root.children.push(history);
    assert_eq!(edit(&two, "在庫の履歴を並び替え可能に", None).unwrap().target_key, "inventory-history");
}

/// C-R1-06: 固定（user_locked）した部品は、親を消す編集でも消さない。
#[test]
fn r1_06_remove_does_not_take_locked_parts_inside_the_subtree() {
    let mut inv = app("在庫を整理したい");
    inv.composition.root.find_mut("inventory-quantity-form").unwrap().user_locked = true;
    match edit(&inv, "この欄を消して", Some("inventory-selected")) {
        Err(EditError::Locked(k)) => assert_eq!(k, "inventory-quantity-form"),
        other => panic!("固定した form ごと消した: {:?}", other.map(|e| e.summary)),
    }
}

/// C-R1-07: 「〜できるようにしたい」は新しい目的。変更の語だけの文は今までどおり編集。
#[test]
fn r1_07_can_do_goals_are_new_apps_and_edits_stay_edits() {
    let water = compose("植物の水やりを記録できるようにしたい", &ComposeContext::default()).unwrap_or_else(|e| panic!("{e}")).into_app();
    assert_eq!(component_set(&water), component_set(&app("植物の水やりを記録したい")), "「記録したい」と同じ形");
    for g in ["植物の水やりを管理可能にしたい", "読書を記録できるようにしたい"] {
        compose(g, &ComposeContext::default()).unwrap_or_else(|e| panic!("{g}: {e}"));
    }
    for g in ["検索を追加", "一覧に検索を追加して", "一覧を並び替え可能にして", "一覧を検索できるようにして"] {
        assert!(matches!(compose(g, &ComposeContext::default()), Err(ComposeError::EditRequest(_))), "{g}: 変更の依頼は新しい app にしない");
    }
}

/// C-R1-08: registry の simulated_in_preview は、そのまま写して接続待ちの action にする（compose を落とさない）。
#[test]
fn r1_08_a_simulated_registry_entry_is_not_a_compose_failure() {
    let sim = |id: &str| CapabilityDescriptor { id: id.into(), provider: None, readiness: CapabilityReadiness::SimulatedInPreview, required_scopes: vec![id.into()], adapter: None };
    for (goal, cap) in [("仕事の状況を把握したい", "tasks.read"), ("Slack と GitHub をまとめたい", "slack.messages.read")] {
        let registry = vec![sim(cap)];
        let a = compose(goal, &ComposeContext { registry: registry.clone(), ..Default::default() }).unwrap_or_else(|e| panic!("{goal}: {e}")).into_app();
        assert_eq!(a.capability(cap).unwrap().readiness, CapabilityReadiness::SimulatedInPreview, "{goal}: registry の値のまま");
        assert_eq!(a.action(cap).unwrap().readiness, ActionReadiness::RequiresConnection, "{goal}: preview の能力では実行しない");
        assert!(verify(&a, &registry).blockers().is_empty(), "{goal}");
    }
}

// ---------- 審査 R2 の指摘（C-R2-01〜03・SEC-C2-01）: 直す前に赤くなる再現 ----------

/// JSON の UI tree から key の node を外す（model の案を作る用）。
fn drop_nodes(n: &mut Value, keys: &[&str]) {
    if let Some(cs) = n.get_mut("children").and_then(Value::as_array_mut) {
        cs.retain(|c| !keys.iter().any(|k| c["key"] == *k));
        for c in cs {
            drop_nodes(c, keys);
        }
    }
}

/// C-R2-01 / SEC-C2-01: local かどうかは下書きの側で決める。mic の録音から capability を外して ready にした案も、録音と関係の無い app に
/// 足した録音も採らない。BuildVerifier も、mic の許可（local.microphone）を通らない録音を ready と言わせない。
#[test]
fn r2_01_the_mic_gate_cannot_be_stripped_and_no_recorder_can_be_added() {
    let goal = "鼻歌を録音したい";
    let base = app(goal);
    let rec = base.action("local.audio.record").expect("前提: 録音の action");
    assert_eq!((rec.capability_ref.as_deref(), rec.readiness), (Some("local.microphone"), ActionReadiness::RequiresConnection), "前提");
    // (1) 同じ id の録音から capability を外し、ready・local_ready を名乗る
    let mut stripped = serde_json::to_value(&base).unwrap();
    for a in stripped["actions"].as_array_mut().unwrap() {
        if a["id"] == "local.audio.record" {
            a.as_object_mut().unwrap().remove("capability_ref");
            a["readiness"] = json!("ready");
        }
    }
    stripped["state"] = json!({ "readiness": "ready", "execution_mode": "local_ready" });
    // 下書きとの照合で落ちる（BuildVerifier より前の段）
    let drifted = |p: &BuildPlan, id: &str| p.errors.iter().any(|e| e.contains("下書きの外") && e.contains(id));
    let p = with_model(goal, Ok(stripped.clone()));
    assert_eq!(p.interpreter, "rule_based", "mic の許可を外した録音を採った: {:?}", p.errors);
    assert!(drifted(&p, "local.audio.record"), "{:?}", p.errors);
    let rec = p.app().action("local.audio.record").unwrap();
    assert_eq!((rec.capability_ref.as_deref(), rec.readiness), (Some("local.microphone"), ActionReadiness::RequiresConnection));
    // model の口の外から来た定義でも、BuildVerifier が止める
    let def: AppDefinitionV3 = serde_json::from_value(stripped).unwrap();
    validate_generated(&def).unwrap_or_else(|e| panic!("前提: domain の検査は通ってしまう形: {}", errs(&e)));
    let v = verify(&def, &[]);
    assert!(!v.is_ready() && v.blockers().iter().any(|f| f.subject == "local.audio.record"), "{:?}", v.findings);
    // 録音を変えない案（題名だけ）は今までどおり採る
    let mut ok = serde_json::to_value(&base).unwrap();
    ok["title"] = json!("鼻歌のメモ");
    let p = with_model(goal, Ok(ok));
    assert_eq!(p.interpreter, "model", "{:?}", p.errors);
    // mic を許可済み（registry で local.microphone が ready）なら、local.microphone を通る録音は gate を通って ready のまま
    let mic = ready_caps(&["local.microphone"]);
    let granted = compose(goal, &ComposeContext { registry: mic.clone(), ..Default::default() }).unwrap_or_else(|e| panic!("許可済みの mic で作れない: {e}")).into_app();
    assert_eq!(granted.action("local.audio.record").map(|a| a.readiness), Some(ActionReadiness::Ready));
    assert!(verify(&granted, &mic).blockers().is_empty());
    // (2) 録音と関係の無い app に、組み込みの namespace の録音（照合で落ちる）や、自分の source の下の録音（mic を通らないので
    // BuildVerifier で落ちる）を ready・承認なしで足す
    let inv_goal = "在庫を整理したい";
    for (id, by_drift) in [("local.audio.record", true), ("local.inventory.record", false)] {
        let mut added = serde_json::to_value(app(inv_goal)).unwrap();
        added["actions"].as_array_mut().unwrap().push(json!({ "id": id, "effect": "local_write", "readiness": "ready", "input_schema": { "type": "object" }, "required_scopes": [] }));
        added["composition"]["root"]["children"].as_array_mut().unwrap().push(json!({ "key": "inventory-record", "component": "button", "label": "録音する", "action_ref": id }));
        let p = with_model(inv_goal, Ok(added));
        assert!(p.interpreter == "rule_based" && p.errors.iter().any(|e| e.contains(id)), "{id}: {:?}", p.errors);
        assert!(!by_drift || drifted(&p, id), "{id}: {:?}", p.errors);
        assert!(p.app().action(id).is_none(), "{id}");
    }
}

/// C-R2-02: 下書きの接続先（capability・source）と local でない action は、案から消しても採らない（目的に要る取得元を落として
/// 「接続は要らない」Ready にしない）。消してよいのは local の action だけ。
#[test]
fn r2_02_connections_and_non_local_actions_cannot_be_dropped() {
    let goal = "仕事の状況を把握したい";
    let base = app(goal);
    assert!(base.action("tasks.read").is_some() && base.capability("tasks.read").is_some() && base.source("optional.tasks").is_some(), "前提");
    assert_eq!(base.state.readiness, AppReadiness::Partial, "前提");
    let mut dropped = serde_json::to_value(&base).unwrap();
    dropped["actions"].as_array_mut().unwrap().retain(|a| a["id"] != "tasks.read");
    dropped["capabilities"] = json!([]);
    dropped["sources"].as_array_mut().unwrap().retain(|s| s["id"] != "optional.tasks");
    drop_nodes(&mut dropped["composition"]["root"], &["sources-items", "sources-read"]);
    dropped["state"] = json!({ "readiness": "ready", "execution_mode": "local_ready" });
    let p = with_model(goal, Ok(dropped));
    assert_eq!(p.interpreter, "rule_based", "接続先を消した案を採った: {:?}", p.errors);
    assert!(p.errors.iter().any(|e| e.contains("tasks.read") && e.contains("optional.tasks")), "{:?}", p.errors);
    assert!(p.app().action("tasks.read").is_some() && p.app().state.readiness == AppReadiness::Partial);
    // 使っていない local の action（filter）を消しただけの案は採る
    let mut slim = serde_json::to_value(&base).unwrap();
    slim["actions"].as_array_mut().unwrap().retain(|a| a["id"] != "local.tasks.filter");
    let p = with_model(goal, Ok(slim));
    assert_eq!(p.interpreter, "model", "{:?}", p.errors);
}

/// C-R2-03: 名前で指した一覧は「一覧」「表」の語より先（「取り込んだ資料の一覧」をノートの一覧と読まない）。「表示」は表と読まない。
#[test]
fn r2_03_a_collection_named_by_label_wins_over_the_generic_word() {
    let notes = app("研究ノートを作りたい");
    assert_eq!(edit(&notes, "取り込んだ資料の一覧を並び替え可能にして", None).unwrap().target_key, "imports-list");
    let (e, after) = apply_and_check(&notes, "取り込んだ資料の一覧に検索を追加", None);
    let form = after.composition.root.find(e.affected_keys.last().unwrap()).unwrap();
    assert_eq!((form.binding.as_ref().map(|b| b.source_ref.as_str()), form.action_ref.as_deref()), (Some("local.imports"), Some("local.imports.filter")));
    // 「表示」の「表」は表ではない: 表と一覧が両方在る時、「一覧の表示」は一覧を指す
    let mut two = app("在庫を整理したい");
    let mut history = two.composition.root.find("inventory-table").unwrap().clone();
    history.key = "inventory-history".into();
    history.component = "list".into();
    history.label = Some("履歴".into());
    history.role = None;
    two.composition.root.children.push(history);
    assert_eq!(edit(&two, "一覧の表示を並び替え可能にして", None).unwrap().target_key, "inventory-history");
    assert_eq!(edit(&two, "この表を並び替え可能にして", None).unwrap().target_key, "inventory-table", "「表」は今までどおり表");
}

/// C-R3-01: 小文字にすると文字数が変わる文字（'İ' は "i\u{307}" の 2 文字になる）が前に在っても、名指しのサービス名と物の label は
/// 利用者の書いた形のまま切り出す（1 文字ずれた「lack 」を接続先の名前にしない）。
#[test]
fn r3_01_a_char_whose_lowercase_is_longer_does_not_shift_the_users_words() {
    let reading = |p: &str| match composer::goal::interpret(p) {
        composer::goal::Interpretation::Goal(r) => r,
        _ => panic!("{p}: Goal を期待"),
    };
    let r = reading("İzmir の Slack をまとめたい");
    let named: Vec<String> = r.externals.iter().filter_map(|x| x.named.clone()).collect();
    assert!(!named.is_empty() && named.iter().all(|n| n == "Slack"), "{named:?}");
    let r = reading("İ の英単語を覚えたい");
    assert!(r.entities.iter().any(|e| e.label.contains("英単語")), "{:?}", r.entities.iter().map(|e| &e.label).collect::<Vec<_>>());
}

/// FX6（FX-032・UIUX G06）: AI 無しで英語の一言から作った物の欄の名前と選択肢の値は英語（日本語の表の語は照合の terms にだけ残る）。
/// 負の対照（commit message）: `books` の `pick("status", "Status", …)` の選択肢を `"読んだ"` に戻す → 赤。
#[test]
fn fx6_a_reading_log_made_without_an_ai_has_english_field_names_and_values() {
    fn collect(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if matches!(k.as_str(), "label" | "options") {
                        match x {
                            Value::String(s) => out.push(s.clone()),
                            Value::Array(a) => out.extend(a.iter().filter_map(|o| o.as_str().map(str::to_owned))),
                            _ => {}
                        }
                    }
                    collect(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
            _ => {}
        }
    }
    let def = serde_json::to_value(plan("a reading log").into_app()).unwrap();
    let mut words = Vec::new();
    collect(&def["composition"], &mut words);
    let cjk = |s: &str| s.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c));
    let bad: Vec<&String> = words.iter().filter(|w| cjk(w)).collect();
    assert!(bad.is_empty(), "英語の一言から作ったのに日本語の欄の名前・選択肢: {bad:?}");
    for want in ["Title", "Author", "Status", "Want to read", "Reading", "Read"] {
        assert!(words.iter().any(|w| w == want), "{want} が無い: {words:?}");
    }
}
