//! W19-1: 利用者の data の recipe（`domain::capability::Recipe`・`<data dir>/extensions/<id>/recipe.json`）を registry の `Adapter` にする。
//! Core に app 専用の code を足さずに、Engine が作った・利用者が入れた app 固有の能力を registry に載せる（owner 2026-10-21 16・18 通目）。
//! 手段（CLI・MCP・AX・DOM・code・HTTP）を実際に呼ぶのは注入された `sdk::CallRunner`（Core の配線が mcp・WebView・codegen・bridge を知っている）。
//! Core はこの module を名指さず、registry の `AdapterRegistry::add_recipes` だけを呼ぶ（`scripts/check_app_agnostic.py` の direct）。
//! この module 配下は `crate::` を書かない（tests/adapters.rs が `#[path]` で取り込む）。
//! Wave: W19-1
use super::super::domain::{
    action::Effect,
    binding::Source,
    capability::{CapabilityReadiness, CapabilityRecord, Origin, Recipe, RecipeAction, RecipeCall, Verification, CAPABILITY_SCHEMA, RECIPE_FILE, RECIPE_SCHEMA},
    run::{Observation, Outcome},
};
use super::sdk::{now, source, Adapter, AdapterKind, CallRunner, Route, SourceSync, Spec};
use serde_json::{json, Value};
use std::{fmt, path::Path};

/// recipe 1 つを包んだ Adapter。
/// Wave: W19-1
pub struct RecipeAdapter {
    recipe: Recipe,
    /// recipe の actions を `Spec` にした物（`Adapter::specs` は `&'static` を返す）
    specs: &'static [Spec],
    run: CallRunner,
    /// `Adapter::id` / `provider` は `&'static`（new で 1 回だけ leak する）
    id: &'static str,
    provider: &'static str,
    kind: AdapterKind,
    /// 手段の最後の結果（readiness を下げる向きにだけ効く）
    sync: SourceSync,
}

/// recipe を読めない・registry に載らない理由。Display は画面に出せる 1 行。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeError {
    /// dir / file を読めない
    Io { path: String, reason: String },
    /// JSON でない・未知の欄・`schema` が `RECIPE_SCHEMA` でない
    Parse { path: String, reason: String },
    /// action が registry に載らない（id が namespaced でない・input が object でない・id が重なる・外に書く / 送る / 消すのに承認なし）
    BadAction { action: String, reason: String },
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, reason } => write!(f, "追加の機能 {path} を読めません（{reason}）"),
            Self::Parse { path, reason } => write!(f, "追加の機能 {path} の形が違います（{reason}）。作り直すか消してください"),
            Self::BadAction { action, reason } => write!(f, "追加の機能の操作 {action} を使えません（{reason}）"),
        }
    }
}
impl std::error::Error for RecipeError {}

impl RecipeAdapter {
    /// Wave: W19-1
    /// 保証する: specs は recipe の actions と 1 対 1（順も同じ）・`Spec` の文字列は `&'static` なので作った adapter の分は process の終わりまで残す
    /// （ponytail: 読み直すたびに増える。数が問題になったら `Adapter::specs` を所有型に替える）。保証しない: action が registry に載る事（`register` が見る）。
    pub fn new(recipe: Recipe, run: CallRunner) -> Result<RecipeAdapter, RecipeError> {
        if recipe.schema != RECIPE_SCHEMA {
            return Err(RecipeError::Parse {
                path: format!("{}/{RECIPE_FILE}", recipe.id),
                reason: format!("schema が {RECIPE_SCHEMA} ではなく「{}」", recipe.schema),
            });
        }
        let bad = |a: &RecipeAction, reason: &str| RecipeError::BadAction { action: a.id.clone(), reason: reason.into() };
        for (i, a) in recipe.actions.iter().enumerate() {
            if !super::namespaced(&a.id) {
                return Err(bad(a, "id は dot を含む小文字の名前です"));
            }
            if !a.input.is_object() {
                return Err(bad(a, "input が JSON の object ではありません"));
            }
            if recipe.actions[..i].iter().any(|o| o.id == a.id) {
                return Err(bad(a, "同じ id の操作が 2 つあります"));
            }
            // registry の gate は spec.approval しか見ないので、外に書く・送る・消す操作の承認はここで縛る
            // （RecipeAction::approval の doc「external_write 以上」・domain の V5）。手元の書き込みの要否は作り手が決める
            if matches!(a.effect, Effect::ExternalWrite | Effect::ExternalSend | Effect::Destructive) && !a.approval {
                return Err(bad(a, "外に書く・送る・消す操作は承認（approval: true）が要ります"));
            }
        }
        let specs: Vec<Spec> = recipe
            .actions
            .iter()
            .map(|a| Spec {
                id: leak(&a.id),
                effect: a.effect,
                approval: a.approval,
                scopes: Box::leak(a.scopes.iter().map(|s| leak(s)).collect::<Vec<_>>().into_boxed_slice()),
                input: leak(&a.input.to_string()),
                verifier: leak(&a.verifier),
            })
            .collect();
        let mut kinds = recipe.actions.iter().map(|a| via(&a.call));
        let first = kinds.next();
        let kind = match first {
            Some(k) if kinds.all(|o| o == k) => k,
            _ => AdapterKind::Extension,
        };
        Ok(RecipeAdapter {
            id: leak(&recipe.id),
            provider: leak(&recipe.provider),
            kind,
            specs: Box::leak(specs.into_boxed_slice()),
            sync: SourceSync::default(),
            recipe,
            run,
        })
    }
    fn action(&self, id: &str) -> Option<&RecipeAction> {
        self.recipe.actions.iter().find(|a| a.id == id)
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// 手段 → CD-07 の種類（`CapabilityRecord` の doc の表）。
fn via(c: &RecipeCall) -> AdapterKind {
    match c {
        RecipeCall::Cli { .. } => AdapterKind::Repo,
        RecipeCall::Mcp { .. } => AdapterKind::Mcp,
        RecipeCall::Gui { .. } => AdapterKind::MacGui,
        RecipeCall::Dom { .. } => AdapterKind::OriginalSurface,
        RecipeCall::Code { .. } => AdapterKind::LocalCore,
        RecipeCall::Http { .. } => AdapterKind::Api,
    }
}

/// `dir/<id>/recipe.json` を全部読む（読めない file は Err で並べ、他の file は読む・dir が無ければ空）。
/// Wave: W19-1
pub fn load_dir(dir: &Path) -> Vec<Result<Recipe, RecipeError>> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut dirs: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| {
            let path = d.join(RECIPE_FILE);
            let shown = path.display().to_string();
            let text = std::fs::read_to_string(&path).map_err(|e| RecipeError::Io { path: shown.clone(), reason: e.to_string() })?;
            let recipe: Recipe = serde_json::from_str(&text).map_err(|e| RecipeError::Parse { path: shown.clone(), reason: e.to_string() })?;
            if recipe.schema != RECIPE_SCHEMA {
                return Err(RecipeError::Parse { path: shown, reason: format!("schema が {RECIPE_SCHEMA} ではなく「{}」", recipe.schema) });
            }
            Ok(recipe)
        })
        .collect()
}

/// 手段は `CallRunner` に任せ、gate（登録・readiness・承認）は registry が済ませる（`sdk::Adapter` の約束）。
/// kind は actions の手段が 1 つならその手段の kind（`CapabilityRecord` の doc の表）・混ざれば `Extension`。route は Origin::Official なら OfficialApi・他は OsAutomation。
impl Adapter for RecipeAdapter {
    fn id(&self) -> &'static str {
        self.id
    }
    fn kind(&self) -> AdapterKind {
        self.kind
    }
    fn route(&self) -> Route {
        match self.recipe.origin {
            Origin::Official => Route::OfficialApi,
            _ => Route::OsAutomation,
        }
    }
    fn provider(&self) -> &'static str {
        self.provider
    }
    fn specs(&self) -> &'static [Spec] {
        self.specs
    }
    /// action の verification（`CapabilityRecord::readiness` と同じ規則: Ready は Verified だけ）。最後の結果は下げる向きにだけ効く
    /// （手段が無い・切れた・認証を拒まれた = `SourceSync` の fault）。成功しても Unverified の action を Ready に上げない。
    fn readiness(&self, action: &str) -> CapabilityReadiness {
        let Some(a) = self.action(action) else { return CapabilityReadiness::Unknown };
        let base = match a.verification {
            Verification::Verified { .. } => CapabilityReadiness::Ready,
            Verification::Unverified => CapabilityReadiness::Discoverable,
            Verification::Unsupported { .. } => CapabilityReadiness::Unsupported,
        };
        match self.sync.capability(true) {
            CapabilityReadiness::Ready | CapabilityReadiness::ConnectedUnverified => base,
            fault if base == CapabilityReadiness::Ready => fault,
            _ => base,
        }
    }
    fn sources(&self) -> Vec<Source> {
        vec![source(self.id, self.kind, self.provider, &self.sync, true)]
    }
    /// 確かめ済み（Verified）で承認の要らない読みの action のうち、必須の入力が無い最初の 1 つを `{}` で 1 回呼ぶ。無ければ何もしない
    /// （確かめていない手順・書く手順を check の名目で走らせない）。
    fn check(&mut self) {
        let probe = self.recipe.actions.iter().find(|a| {
            matches!(a.verification, Verification::Verified { .. })
                && !a.approval
                && matches!(a.effect, Effect::LocalRead | Effect::ExternalRead)
                && a.input["required"].as_array().map_or(true, |r| r.is_empty())
        });
        let Some(call) = probe.map(|a| a.call.clone()) else { return };
        let (o, _) = (self.run)(&call, &json!({}));
        self.sync.record_check(now(), &o);
    }
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        let Some(call) = self.action(action).map(|a| a.call.clone()) else {
            return (Outcome::Rejected { reason: format!("追加の機能 {} に操作 {action} はありません", self.id) }, None);
        };
        let (o, obs) = (self.run)(&call, input);
        self.sync.record(now(), &o, obs.as_ref());
        (o, obs)
    }
    fn cleanup(&mut self) {
        self.sync = SourceSync::default();
    }
    /// recipe の reuse・origin・versions と、action ごとの手段の kind・verification・scope（根拠は Verified の evidence）。
    fn records(&self) -> Vec<CapabilityRecord> {
        self.recipe
            .actions
            .iter()
            .map(|a| CapabilityRecord {
                schema: CAPABILITY_SCHEMA.into(),
                id: a.id.clone(),
                reuse: self.recipe.reuse.clone(),
                origin: self.recipe.origin.clone(),
                kind: via(&a.call),
                verification: a.verification.clone(),
                versions: self.recipe.versions.clone(),
                permissions: a.scopes.clone(),
                evidence: match &a.verification {
                    Verification::Verified { evidence, .. } => vec![evidence.clone()],
                    _ => vec![],
                },
                constraints: vec![],
            })
            .collect()
    }
}

/// W19-1 の署名を compile で縛る。
#[cfg(test)]
mod w19_1_interface {
    use super::super::AdapterRegistry;
    use super::*;
    /// 偽の Adapter: 手書きの Adapter が `Adapter::records` を上書きできる事と、その口の形を本物の impl と独立に縛る（trait を変えるとここが compile で赤）。
    struct Fake;
    impl Adapter for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::Extension
        }
        fn route(&self) -> Route {
            Route::Handoff
        }
        fn provider(&self) -> &'static str {
            "fake"
        }
        fn specs(&self) -> &'static [Spec] {
            &[]
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Unknown
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, _action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
            (Outcome::Rejected { reason: "偽".into() }, None)
        }
        fn cleanup(&mut self) {}
        fn records(&self) -> Vec<CapabilityRecord> {
            vec![]
        }
    }

    #[test]
    fn w19_1_fake_adapter_pins_records_shape() {
        let a: Box<dyn Adapter> = Box::new(Fake);
        assert!(a.records().is_empty());
    }

    /// `records` を上書きしない手書きの Adapter（src/adapters/ の既定）: Spec ごとに 1 件・公式・provider の app 固有・kind は Adapter の物・
    /// 確かめていない（Unverified → Discoverable）。Ready に見せるのは registry が最後の結果を見た時だけ。
    struct Plain;
    const PLAIN: &[Spec] = &[Spec {
        id: "plain.doc.read",
        effect: super::super::super::domain::action::Effect::LocalRead,
        approval: false,
        scopes: &["plain.read"],
        input: r#"{"type":"object"}"#,
        verifier: "read_back",
    }];
    impl Adapter for Plain {
        fn id(&self) -> &'static str {
            "plain"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::Api
        }
        fn route(&self) -> Route {
            Route::OfficialApi
        }
        fn provider(&self) -> &'static str {
            "plain"
        }
        fn specs(&self) -> &'static [Spec] {
            PLAIN
        }
        fn readiness(&self, _action: &str) -> CapabilityReadiness {
            CapabilityReadiness::Ready
        }
        fn sources(&self) -> Vec<Source> {
            vec![]
        }
        fn check(&mut self) {}
        fn invoke(&mut self, _action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
            (Outcome::Rejected { reason: "偽".into() }, None)
        }
        fn cleanup(&mut self) {}
    }

    #[test]
    fn w19_1_default_records_are_official_and_unverified() {
        use super::super::super::domain::capability::{Origin, Reuse, Verification, CAPABILITY_SCHEMA};
        let r = Plain.records();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].schema.as_str(), r[0].id.as_str(), r[0].kind), (CAPABILITY_SCHEMA, "plain.doc.read", AdapterKind::Api));
        assert_eq!((&r[0].origin, &r[0].reuse), (&Origin::Official, &Reuse::AppSpecific { app: "plain".into() }));
        assert_eq!(r[0].permissions, ["plain.read"]);
        assert_eq!(r[0].verification, Verification::Unverified, "Adapter の readiness が Ready でも、確かめた結果が無ければ Verified にしない");
        assert_eq!(r[0].readiness(), CapabilityReadiness::Discoverable);
    }

    #[test]
    fn w19_1_recipe_adapter_and_registry_signatures() {
        let _: fn(Recipe, CallRunner) -> Result<RecipeAdapter, RecipeError> = RecipeAdapter::new;
        let _: fn(&Path) -> Vec<Result<Recipe, RecipeError>> = load_dir;
        fn is_adapter<T: Adapter>() {}
        is_adapter::<RecipeAdapter>();
        let _: fn(&mut AdapterRegistry, &str) -> Option<Box<dyn Adapter>> = AdapterRegistry::unregister;
        let _: fn(&AdapterRegistry) -> Vec<CapabilityRecord> = AdapterRegistry::records;
        let _: fn(&mut AdapterRegistry, &Path, &mut dyn FnMut() -> CallRunner) -> Vec<String> = AdapterRegistry::add_recipes;
    }
}

/// W19-1 の本体: recipe → Adapter・registry で足す / 外す・Capability Registry の形（偽の手段 = 呼ばれた手段と入力を残す CallRunner）。
#[cfg(test)]
mod w19_1_recipe {
    use super::super::super::domain::{
        action::Effect,
        capability::{Origin, RecipeAction, RecipeCall, Reuse, Verification, CAPABILITY_SCHEMA, RECIPE_FILE, RECIPE_SCHEMA},
        capability::DisplayTier,
        permission::{ConsentDecision, ConsentReceipt, ConsentSubject},
        run::{ErrorCode, Observation, Outcome},
    };
    use super::super::{sdk, AdapterRegistry, Caller};
    use super::*;
    use serde_json::json;
    use std::{cell::RefCell, path::PathBuf, rc::Rc};

    type Calls = Rc<RefCell<Vec<(RecipeCall, Value)>>>;
    /// 偽の手段: 呼ばれた (手段, 入力) を残し、`answer` を返す
    fn runner(calls: &Calls, answer: Outcome) -> CallRunner {
        let calls = calls.clone();
        Box::new(move |c: &RecipeCall, input: &Value| {
            calls.borrow_mut().push((c.clone(), input.clone()));
            (answer.clone(), None::<Observation>)
        })
    }
    fn verified() -> Verification {
        Verification::Verified { at: "2026-10-23T00:00:00Z".into(), evidence: "disk の sha".into() }
    }
    fn action(id: &str, effect: Effect, approval: bool, call: RecipeCall, verification: Verification) -> RecipeAction {
        RecipeAction { id: id.into(), effect, approval, scopes: vec!["example.write".into()], input: json!({ "type": "object" }), verifier: "read_back".into(), call, verification }
    }
    fn cli(arg: &str) -> RecipeCall {
        RecipeCall::Cli { argv: vec!["tool".into(), arg.into()] }
    }
    /// 読む（確かめ済み・CLI）・題を置く（調査しただけ・AX の手順・承認つき）・保存（確かめ済み・CLI・承認つき）
    fn example() -> Recipe {
        Recipe {
            schema: RECIPE_SCHEMA.into(),
            id: "example".into(),
            provider: "example".into(),
            reuse: Reuse::AppSpecific { app: "org.example.app".into() },
            origin: Origin::Generated { artifact: "example".into() },
            versions: vec!["1.0".into()],
            actions: vec![
                action("example.doc.read", Effect::LocalRead, false, cli("read"), verified()),
                action("example.doc.title.set", Effect::ExternalWrite, true, RecipeCall::Gui { steps: vec![json!({ "op": "set_value", "find": { "role": "AXTextField", "label": "題" } })] }, Verification::Unverified),
                action("example.doc.save", Effect::ExternalWrite, true, cli("save"), verified()),
            ],
        }
    }
    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-w19-1-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn put(dir: &Path, id: &str, text: &str) {
        std::fs::create_dir_all(dir.join(id)).unwrap();
        std::fs::write(dir.join(id).join(RECIPE_FILE), text).unwrap();
    }

    #[test]
    fn w19_1_recipe_actions_become_specs_and_the_runner_does_the_call() {
        let calls: Calls = Rc::default();
        let mut a = RecipeAdapter::new(example(), runner(&calls, Outcome::Verified { evidence: vec!["読めた".into()] })).unwrap();
        let ids: Vec<_> = a.specs().iter().map(|s| (s.id, s.effect, s.approval, s.scopes.to_vec(), s.input)).collect();
        assert_eq!(ids[0], ("example.doc.read", Effect::LocalRead, false, vec!["example.write"], r#"{"type":"object"}"#));
        assert_eq!(ids.len(), 3, "actions と 1 対 1");
        assert_eq!((a.id(), a.provider(), a.kind(), a.route()), ("example", "example", AdapterKind::Extension, Route::OsAutomation), "手段が混ざれば Extension・生成した物は公式の経路にしない");
        let only_cli = Recipe { origin: Origin::Official, actions: example().actions.into_iter().filter(|x| matches!(x.call, RecipeCall::Cli { .. })).collect(), ..example() };
        let b = RecipeAdapter::new(only_cli, runner(&calls, Outcome::Verified { evidence: vec![] })).unwrap();
        assert_eq!((b.kind(), b.route()), (AdapterKind::Repo, Route::OfficialApi), "CLI だけ = Repo（CD-07）");
        // 手段は CallRunner が呼ぶ（Adapter は中身を知らない）・入力はそのまま渡す
        let (o, _) = a.invoke("example.doc.read", &json!({ "path": "a.txt" }));
        assert!(matches!(o, Outcome::Verified { .. }), "{o:?}");
        assert_eq!(calls.borrow().as_slice(), &[(cli("read"), json!({ "path": "a.txt" }))]);
        assert!(matches!(a.invoke("example.nope", &json!({})).0, Outcome::Rejected { .. }));
        assert_eq!(calls.borrow().len(), 1, "知らない操作は呼ばない");
        // readiness = action の verification（調査しただけ = Discoverable）
        assert_eq!(a.readiness("example.doc.read"), CapabilityReadiness::Ready);
        assert_eq!(a.readiness("example.doc.title.set"), CapabilityReadiness::Discoverable);
        // 手段が無い（NotReady）と分かったら確かめ済みでも下げる・成功しても未確認を上げない
        let gone: Calls = Rc::default();
        let mut c = RecipeAdapter::new(example(), runner(&gone, Outcome::Failed { error: ErrorCode::NotReady })).unwrap();
        c.invoke("example.doc.read", &json!({}));
        assert_eq!(c.readiness("example.doc.read"), CapabilityReadiness::RequiresConnection);
        assert_eq!(c.readiness("example.doc.title.set"), CapabilityReadiness::Discoverable);
        c.cleanup();
        assert_eq!(c.readiness("example.doc.read"), CapabilityReadiness::Ready);
        // Capability Registry の形: recipe の reuse・origin・versions と action ごとの手段・verification
        let rec = a.records();
        assert_eq!(rec.iter().map(|r| (r.id.as_str(), r.kind, r.readiness())).collect::<Vec<_>>(), vec![
            ("example.doc.read", AdapterKind::Repo, CapabilityReadiness::Ready),
            ("example.doc.title.set", AdapterKind::MacGui, CapabilityReadiness::Discoverable),
            ("example.doc.save", AdapterKind::Repo, CapabilityReadiness::Ready),
        ]);
        assert!(rec.iter().all(|r| r.schema == CAPABILITY_SCHEMA && r.origin == Origin::Generated { artifact: "example".into() } && r.versions == ["1.0"]));
        assert_eq!(rec[0].evidence, ["disk の sha"]);
    }

    /// check は確かめ済みの読み（必須の入力なし）を 1 回だけ呼ぶ。調査しただけの手順・書く手順は呼ばない
    #[test]
    fn w19_1_recipe_check_runs_only_a_verified_read() {
        let calls: Calls = Rc::default();
        let mut a = RecipeAdapter::new(example(), runner(&calls, Outcome::Verified { evidence: vec![] })).unwrap();
        a.check();
        assert_eq!(calls.borrow().as_slice(), &[(cli("read"), json!({}))]);
        let unread = Recipe { actions: example().actions.into_iter().filter(|x| x.id != "example.doc.read").collect(), ..example() };
        let calls: Calls = Rc::default();
        RecipeAdapter::new(unread, runner(&calls, Outcome::Verified { evidence: vec![] })).unwrap().check();
        assert!(calls.borrow().is_empty(), "読みが無ければ何も呼ばない: {:?}", calls.borrow());
    }

    #[test]
    fn w19_1_recipe_new_refuses_what_registry_cannot_carry() {
        let calls: Calls = Rc::default();
        let new = |r: Recipe| RecipeAdapter::new(r, runner(&calls, Outcome::Verified { evidence: vec![] })).err().map(|e| e.to_string());
        assert!(new(Recipe { schema: "shibubu.recipe/0".into(), ..example() }).unwrap().contains("形が違います"));
        let mut bad_input = example();
        bad_input.actions[0].input = json!([1]);
        assert!(new(bad_input).unwrap().contains("object"));
        let mut nodot = example();
        nodot.actions[0].id = "Read".into();
        assert!(new(nodot).unwrap().contains("Read"));
        let mut dup = example();
        dup.actions[1].id = "example.doc.read".into();
        assert!(new(dup).unwrap().contains("2 つ"));
        // 書く・送る・消す action は承認なしでは載せない（registry の gate は spec.approval しか見ないので、ここで止めないと承認なしで手段まで届く）
        for effect in [Effect::ExternalWrite, Effect::ExternalSend, Effect::Destructive] {
            let mut no_approval = example();
            no_approval.actions[2] = action("example.doc.delete", effect, false, cli("delete"), verified());
            let line = new(no_approval).unwrap_or_else(|| panic!("{effect:?} + approval:false を載せた"));
            assert!(line.contains("example.doc.delete") && line.contains("承認"), "{line}");
        }
        let mut local = example();
        local.actions[2] = action("example.doc.cache", Effect::LocalWrite, false, cli("cache"), verified());
        assert!(new(local).is_none(), "手元の書き込みは承認なしでも載る（判断は recipe の作り手）");
        assert!(new(example()).is_none());
    }

    #[test]
    fn w19_1_load_dir_reads_each_recipe_and_reports_the_bad_ones() {
        let d = tmp("load");
        put(&d, "a", &serde_json::to_string(&example()).unwrap());
        put(&d, "b", "{ 壊れた");
        put(&d, "c", &serde_json::to_string(&Recipe { schema: "x".into(), ..example() }).unwrap());
        std::fs::create_dir_all(d.join("d")).unwrap();
        std::fs::write(d.join("note.txt"), "dir でない物は読まない").unwrap();
        let got = load_dir(&d);
        assert_eq!(got.len(), 4, "{got:?}");
        assert_eq!(got[0].as_ref().unwrap(), &example());
        assert!(matches!(&got[1], Err(RecipeError::Parse { .. })), "{:?}", got[1]);
        assert!(matches!(&got[2], Err(RecipeError::Parse { reason, .. }) if reason.contains("schema")), "{:?}", got[2]);
        assert!(matches!(&got[3], Err(RecipeError::Io { .. })), "{:?}", got[3]);
        assert!(load_dir(&d.join("無い")).is_empty(), "dir が無ければ空");
    }

    fn consent(action: &str, input: &Value) -> ConsentReceipt {
        ConsentReceipt {
            id: format!("c-{action}"),
            app_id: "app".into(),
            subject: ConsentSubject::Action { action_ref: action.into(), run_id: None },
            scopes: vec!["example.write".into()],
            payload_digest: Some(sdk::payload_digest(action, input)),
            decision: ConsentDecision::Approved,
            decided_at: "2026-10-23T00:00:00Z".into(),
        }
    }
    const APP: Caller = Caller { app_id: "app", run_id: None };

    /// 19 通目 §7 W19-1「Adapter を追加・削除しても Core の変更が不要」: recipe を置く → add_recipes で載る → 外すと「登録されていない」・段は Unavailable。
    /// 19 通目 §5「調査しただけの能力を Ready にしない」: 調査しただけの action は承認があっても gate で止まり、手段を呼ばない（負の対照の相手）。
    #[test]
    fn w19_1_add_and_remove_a_recipe_without_core_changes() {
        let d = tmp("add");
        put(&d, "example", &serde_json::to_string(&example()).unwrap());
        put(&d, "broken", "{");
        let calls: Calls = Rc::default();
        let mut r = AdapterRegistry::default();
        let lines = r.add_recipes(&d, &mut || runner(&calls, Outcome::Verified { evidence: vec!["読み戻して一致".into()] }));
        assert_eq!(lines.len(), 1, "壊れた recipe だけが 1 行: {lines:?}");
        assert!(lines[0].contains("broken") && lines[0].contains("形が違います"), "{lines:?}");
        assert_eq!(r.actions().len(), 3, "壊れた recipe があっても他は載る");
        // 調査しただけ: 承認があっても送らない
        let title = json!({ "text": "Shibubu Demo" });
        let (o, _) = r.invoke(APP, "example.doc.title.set", &title, Some(&consent("example.doc.title.set", &title)));
        assert!(matches!(&o, Outcome::Rejected { reason } if reason.contains("Discoverable")), "{o:?}");
        assert!(calls.borrow().is_empty(), "調査しただけの手順を呼んだ");
        // 確かめ済みの読みと、承認つきの保存は通る
        assert!(matches!(r.invoke(APP, "example.doc.read", &json!({}), None).0, Outcome::Verified { .. }));
        let save = json!({});
        let ok = consent("example.doc.save", &save);
        assert!(matches!(r.invoke(APP, "example.doc.save", &save, Some(&ok)).0, Outcome::Verified { .. }));
        assert_eq!(calls.borrow().len(), 2);
        assert_eq!(r.tier("example.doc.read"), DisplayTier::Verified);
        // 同じ id をもう一度足すと 1 行（登録済み）・二重に載らない
        let again = r.add_recipes(&d, &mut || runner(&calls, Outcome::Verified { evidence: vec![] }));
        assert!(again.iter().any(|l| l.contains("example") && l.contains("登録済み")), "{again:?}");
        assert_eq!(r.actions().len(), 3);
        // 外す: 登録されていない・Unavailable・records から消える
        let gone = r.unregister("example").expect("登録した id");
        assert_eq!(gone.id(), "example");
        assert!(r.unregister("example").is_none());
        assert!(matches!(r.invoke(APP, "example.doc.read", &json!({}), None).0, Outcome::Rejected { reason } if reason.contains("登録されていない")));
        assert_eq!(r.tier("example.doc.read"), DisplayTier::Unavailable);
        assert!(r.records().is_empty());
        // 入れ直しても、使った承認でもう一度は実行しない（used は消さない）
        assert!(r.add_recipes(&d, &mut || runner(&calls, Outcome::Verified { evidence: vec![] })).len() == 1);
        assert!(matches!(r.invoke(APP, "example.doc.save", &save, Some(&ok)).0, Outcome::Rejected { reason } if reason.contains("使用済み")));
        assert_eq!(calls.borrow().len(), 2);
    }

    /// 手書きの Adapter の records: 最後の試行が Verified の action だけ Verified（at = 試行の時刻・evidence = その根拠）。受理だけ（Unverified）・
    /// 試していない action は Unverified のまま。外すと最後の結果も消える（入れ直した直後は Unverified）
    #[test]
    fn w19_1_registry_records_raise_only_actions_whose_last_try_was_verified() {
        struct Echo;
        const ECHO: &[Spec] = &[
            Spec { id: "echo.doc.read", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "echo.doc.poke", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "echo.doc.idle", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "echo.doc.bare", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
            Spec { id: "echo.doc.blank", effect: Effect::LocalRead, approval: false, scopes: &[], input: r#"{"type":"object"}"#, verifier: "read_back" },
        ];
        impl Adapter for Echo {
            fn id(&self) -> &'static str { "echo" }
            fn kind(&self) -> AdapterKind { AdapterKind::LocalCore }
            fn route(&self) -> Route { Route::OfficialApi }
            fn provider(&self) -> &'static str { "echo" }
            fn specs(&self) -> &'static [Spec] { ECHO }
            fn readiness(&self, _action: &str) -> CapabilityReadiness { CapabilityReadiness::Ready }
            fn sources(&self) -> Vec<Source> { vec![] }
            fn check(&mut self) {}
            fn invoke(&mut self, action: &str, _input: &Value) -> (Outcome, Option<Observation>) {
                match action {
                    "echo.doc.read" => (Outcome::Verified { evidence: vec!["読み戻した".into()] }, None),
                    // 根拠の無い Verified（自己申告）: 空・空白だけ
                    "echo.doc.bare" => (Outcome::Verified { evidence: vec![] }, None),
                    "echo.doc.blank" => (Outcome::Verified { evidence: vec!["".into(), " ".into()] }, None),
                    _ => (Outcome::Unverified { reason: "送っただけ".into() }, None),
                }
            }
            fn cleanup(&mut self) {}
        }
        let mut r = AdapterRegistry::default();
        r.register(Box::new(Echo)).unwrap();
        assert!(r.records().iter().all(|x| x.verification == Verification::Unverified), "試す前に Verified");
        r.invoke(APP, "echo.doc.read", &json!({}), None);
        r.invoke(APP, "echo.doc.poke", &json!({}), None);
        r.invoke(APP, "echo.doc.bare", &json!({}), None);
        r.invoke(APP, "echo.doc.blank", &json!({}), None);
        let rec = r.records();
        match &rec[0].verification {
            Verification::Verified { at, evidence } => assert!(evidence == "読み戻した" && at.starts_with("20") && at.ends_with('Z'), "{at} {evidence}"),
            other => panic!("最後の試行が Verified の action が上がらない: {other:?}"),
        }
        assert_eq!((&rec[1].verification, &rec[2].verification), (&Verification::Unverified, &Verification::Unverified), "受理だけ・試していない action を上げた");
        assert_eq!(rec[0].readiness(), CapabilityReadiness::Ready);
        for x in &rec[3..] {
            assert_eq!((x.verification.clone(), x.readiness()), (Verification::Unverified, CapabilityReadiness::Discoverable), "{}: 根拠の無い Verified を上げた", x.id);
        }
        // 画面の段も同じ規則（証拠つきの Verified だけ Verified の段）
        assert_eq!((r.tier("echo.doc.read"), r.tier("echo.doc.bare"), r.tier("echo.doc.blank")), (DisplayTier::Verified, DisplayTier::Ready, DisplayTier::Ready));
        r.unregister("echo").unwrap();
        r.register(Box::new(Echo)).unwrap();
        assert!(r.records().iter().all(|x| x.verification == Verification::Unverified), "外した Adapter の最後の結果が残った");
    }
}
