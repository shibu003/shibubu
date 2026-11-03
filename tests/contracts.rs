//! CORE-001..008 の契約 test（TODO.md §2 の AC をそのまま）。
//! domain は bin crate の module なので `#[path]` で取り込む（lib target が無い）。
//! fixture: docs/examples/*.json（正）と tests/fixtures/broken/*.json（1 箇所ずつ壊した物）。
#[path = "../src/domain/mod.rs"]
mod domain;

use domain::action::{validate_invocation, ActionInvocation, ActionReadiness, InvocationError, InvocationOrigin};
use domain::app::AppDefinitionV3;
use domain::binding::{BindingReadiness, DataBinding, SourceReadiness};
use domain::capability::{CapabilityDescriptor, CapabilityReadiness};
use domain::migrate::{to_v3, MigrateError};
use domain::patch::{apply, AppPatch, AppRevision, PatchError, PatchOp};
use domain::permission::PermissionGrant;
use domain::run::{Run, RunStatus, Step, RUN_TRANSITIONS};
use domain::suggestion::{Suggestion, SuggestionStatus};
use domain::validate::{validate, validate_grant, validate_suggestion, ValidationError};
use serde_json::{json, Value};
use std::fs;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn read(rel: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(format!("{ROOT}/{rel}")).expect(rel)).expect(rel)
}
fn app(name: &str) -> AppDefinitionV3 {
    serde_json::from_value(read(&format!("docs/examples/{name}.json"))).expect(name)
}
fn research() -> AppDefinitionV3 {
    app("app_v3_research")
}
fn broken(name: &str) -> Value {
    read(&format!("tests/fixtures/broken/{name}.json"))
}
fn proposal() -> Suggestion {
    serde_json::from_value(read("docs/examples/agent_proposal_v1.json")).unwrap()
}
fn text(errs: &[ValidationError]) -> String {
    errs.iter().map(ToString::to_string).collect::<Vec<_>>().join(" | ")
}
fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(v: &T, what: &str) {
    let again: T = serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap();
    assert_eq!(*v, again, "{what}: serialize → deserialize で field が欠けた");
}

// ---------- CORE-001 ----------

#[test]
fn v3_examples_deserialize_validate_and_roundtrip() {
    for name in ["app_v3_timer", "app_v3_research", "app_v3_messages"] {
        let def = app(name);
        validate(&def).unwrap_or_else(|e| panic!("{name}: {}", text(&e)));
        roundtrip(&def, name);
    }
    let p = proposal();
    assert_eq!(p.status, SuggestionStatus::Queued);
    roundtrip(&p, "agent_proposal_v1");
}

#[test]
fn legacy_v1_and_v2_examples_migrate_to_valid_v3() {
    let cases = [
        ("calendar_mail", "1.0"),
        ("issue_ide", "1.0"),
        ("release_monitor", "1.0"),
        ("video_ide", "1.0"),
        ("calendar_mail_v2", "2.0"),
        ("music_workspace_v2", "2.0"),
        ("outreach_workspace_v2", "2.0"),
    ];
    for (name, from) in cases {
        let raw = read(&format!("docs/examples/{name}.json"));
        let def = to_v3(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
        validate(&def).unwrap_or_else(|e| panic!("{name}: {}", text(&e)));
        roundtrip(&def, name);
        let meta = def.metadata.as_ref().expect("metadata");
        assert_eq!(meta["migrated_from"], json!(from), "{name}");
        assert_eq!(meta["legacy"], raw, "{name}: 元の文書が metadata.legacy に丸ごと残る");
        assert!(def.actions.iter().all(|a| a.readiness != ActionReadiness::Ready), "{name}: 移行で ready を名乗らない");
        assert!(!def.actions.is_empty() || name == "calendar_mail_v2", "{name}: actions が移る");
    }
    // v1: dataset → produced_by の capability → その source が binding の source_ref。component.action_id は action id。
    let cal = to_v3(&read("docs/examples/calendar_mail.json")).unwrap();
    let mail_list = cal.composition.root.find("mail-list").unwrap();
    let b = mail_list.binding.as_ref().unwrap();
    assert_eq!((b.source_ref.as_str(), b.path.as_str()), ("mail", "related-mail"));
    assert_eq!(mail_list.component, "list");
    assert_eq!(mail_list.visibility.as_ref().unwrap()["visible_in"], json!(["selected", "completed"]));
    assert_eq!(cal.composition.root.find("make-draft").unwrap().action_ref.as_deref(), Some("mail-draft"));
    assert_eq!(cal.action("mail-draft").unwrap().capability_ref.as_deref(), Some("mail-draft"));
    assert!(cal.action("mail-draft").unwrap().requires_approval);
    // v2: 知らない namespaced type_id は placeholder `extension`、元の id は props.type_id。entity は source（kind entity）。
    let music = to_v3(&read("docs/examples/music_workspace_v2.json")).unwrap();
    let wave = music.composition.root.find("audio-wave").unwrap();
    assert_eq!(wave.component, "extension");
    assert_eq!(wave.props["type_id"], json!("extension.vendor.waveform@1"));
    assert_eq!(wave.binding.as_ref().unwrap().source_ref, "recording-list");
    assert_eq!(music.source("recording-list").unwrap().kind, "entity");
    assert_eq!(music.source("recording-list").unwrap().readiness, SourceReadiness::Local, "録音は local microphone が作る");
    assert_eq!(music.composition.root.find("youtube-provider").unwrap().component, "extension");
    assert_eq!(music.action("record-mic").unwrap().readiness, ActionReadiness::PreviewOnly);
    assert_eq!(music.action("import-to-reaper").unwrap().readiness, ActionReadiness::RequiresConnection);
    // v2: when → visibility
    let outreach = to_v3(&read("docs/examples/outreach_workspace_v2.json")).unwrap();
    let vis = outreach.composition.root.find("selected-person").unwrap().visibility.clone().unwrap();
    assert_eq!((vis["state_key"].clone(), vis["equals"].clone()), (json!("has_selection"), json!(true)));
}

#[test]
fn unknown_schema_version_is_an_explicit_error() {
    let raw = broken("unknown_schema_version");
    assert_eq!(to_v3(&raw), Err(MigrateError::UnknownSchemaVersion("4.0".into())));
    let direct = serde_json::from_value::<AppDefinitionV3>(raw).unwrap_err().to_string();
    assert!(direct.contains("4.0") && direct.contains("3.0"), "{direct}");
    assert_eq!(to_v3(&json!({"id": "x"})), Err(MigrateError::MissingSchemaVersion));
}

#[test]
fn unknown_fields_are_rejected_not_ignored() {
    let err = serde_json::from_value::<AppDefinitionV3>(broken("unknown_field")).unwrap_err().to_string();
    assert!(err.contains("intent"), "{err}");
}

// ---------- CORE-002 ----------

#[test]
fn capability_lookup_is_exact_never_by_similarity() {
    let mut def = research();
    def.capabilities.push(CapabilityDescriptor {
        id: "local.notes".into(),
        provider: None,
        readiness: CapabilityReadiness::Ready,
        required_scopes: vec![],
        adapter: None,
    });
    assert!(def.capability("local.notes").is_some());
    for near in ["local.notes.save", "Local.Notes", "local.note", "local.notes ", "notes"] {
        assert!(def.capability(near).is_none(), "'{near}' が完全一致でないのに引けた");
    }
    def.actions[0].capability_ref = Some("local.note".into());
    let errs = validate(&def).unwrap_err();
    assert!(errs.contains(&ValidationError::MissingCapability { action: "local.notes.save".into(), capability_ref: "local.note".into() }), "{}", text(&errs));
    // ready な action の裏の capability が ready でなければ落ちる
    def.actions[0].capability_ref = Some("local.notes".into());
    def.actions[0].readiness = ActionReadiness::Ready;
    def.capabilities[0].readiness = CapabilityReadiness::RequiresConnection;
    let errs = validate(&def).unwrap_err();
    assert!(text(&errs).contains("RequiresConnection"), "{}", text(&errs));
}

// ---------- CORE-003 ----------

#[test]
fn button_palette_and_agent_share_one_action_validator() {
    let mut def = research();
    def.actions[0].readiness = ActionReadiness::Ready;
    def.actions[0].input_schema =
        json!({"type": "object", "required": ["note_id"], "properties": {"note_id": {"type": "string"}}}).as_object().unwrap().clone();
    let origins = [
        InvocationOrigin::Button { node_key: "research-save".into() },
        InvocationOrigin::CommandPalette,
        InvocationOrigin::Agent { suggestion_id: "proposal.research.search".into() },
    ];
    let inv = |origin: &InvocationOrigin, action: &str, input: Value| ActionInvocation { action_ref: action.into(), origin: origin.clone(), input };
    for origin in &origins {
        assert_eq!(validate_invocation(&def, &inv(origin, "local.notes.save", json!({"note_id": "n1"}))).map(|a| a.id.as_str()), Ok("local.notes.save"));
        assert_eq!(
            validate_invocation(&def, &inv(origin, "local.notes.save", json!({}))),
            Err(InvocationError::MissingInput { field: "note_id".into() })
        );
        assert_eq!(
            validate_invocation(&def, &inv(origin, "local.notes.save", json!({"note_id": 5}))),
            Err(InvocationError::InputType { field: "note_id".into(), expected: "string".into() })
        );
        assert_eq!(
            validate_invocation(&def, &inv(origin, "local.notes.export", json!({}))),
            Err(InvocationError::UnknownAction("local.notes.export".into()))
        );
    }
    // 見える node は action の id を指すだけで、定義は actions にある（別の型）
    let node = def.composition.root.find("research-save").unwrap();
    assert_eq!(def.action(node.action_ref.as_deref().unwrap()).unwrap().id, "local.notes.save");
    // preview_only は誰からも実行できない
    def.actions[0].readiness = ActionReadiness::PreviewOnly;
    assert!(matches!(
        validate_invocation(&def, &inv(&origins[1], "local.notes.save", json!({"note_id": "n1"}))),
        Err(InvocationError::NotReady { .. })
    ));
    roundtrip(&inv(&origins[0], "local.notes.save", json!({"note_id": "n1"})), "ActionInvocation");
}

// ---------- CORE-004 ----------

#[test]
fn binding_readiness_reports_missing_unconnected_and_stale_instead_of_content() {
    let def = app("app_v3_messages");
    let b = def.composition.root.find("message-list").unwrap().binding.clone().unwrap();
    assert_eq!(b.readiness(&def.sources, None), BindingReadiness::RequiresConnection);
    let orphan = DataBinding { source_ref: "optional.mail".into(), path: "threads".into(), extra: Default::default() };
    assert_eq!(orphan.readiness(&def.sources, None), BindingReadiness::MissingSource("optional.mail".into()));
    let mut sources = def.sources.clone();
    sources[0].readiness = SourceReadiness::Ready;
    sources[0].last_observed_at = Some("2026-10-16T00:00:00Z".into());
    assert_eq!(b.readiness(&sources, Some("2026-10-16T08:00:00Z")), BindingReadiness::Stale, "観測が期限より古い");
    assert_eq!(b.readiness(&sources, Some("2026-10-15T08:00:00Z")), BindingReadiness::Ready);
    sources[0].last_observed_at = None;
    assert_eq!(b.readiness(&sources, Some("2026-10-15T08:00:00Z")), BindingReadiness::Stale, "一度も観測していない");
    assert_eq!(b.readiness(&sources, None), BindingReadiness::Ready);
    sources[0].readiness = SourceReadiness::Degraded;
    assert_eq!(b.readiness(&sources, None), BindingReadiness::Stale);
    // binding の追加 key は schema が許すので捨てずに残す
    let with_extra: DataBinding = serde_json::from_value(json!({"source_ref": "s", "path": "p", "privacy": "personal"})).unwrap();
    assert_eq!(with_extra.extra["privacy"], json!("personal"));
    roundtrip(&with_extra, "DataBinding extra");
}

// ---------- CORE-005 / CORE-008 ----------

#[test]
fn broken_fixtures_fail_and_name_the_offender() {
    let cases = [
        ("dup_key", "research-list"),
        ("missing_action", "local.notes.sav"),
        ("missing_source", "local.note"),
        ("missing_capability", "local.notes.cap"),
        ("workflow_missing_action", "local.notes.export"),
        ("unsupported_component", "hologram"),
        ("phantom_ready", "external.mail.send"),
    ];
    for (name, offender) in cases {
        let def: AppDefinitionV3 = serde_json::from_value(broken(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let errs = validate(&def).err().unwrap_or_else(|| panic!("{name}: 壊れているのに通った"));
        let t = text(&errs);
        assert!(t.contains(offender), "{name}: error 文に '{offender}' が無い: {t}");
        assert!(AppRevision::new(def).is_err(), "{name}: revision にもなれない");
    }
    assert_eq!(validate(&serde_json::from_value(broken("phantom_ready")).unwrap()).unwrap_err(), vec![ValidationError::PhantomReady { action: "external.mail.send".into() }]);
    // 提案: 対象 node が無い → apply に進めない
    let def = research();
    let s: Suggestion = serde_json::from_value(broken("proposal_missing_target")).unwrap();
    let t = text(&validate_suggestion(&def, &s).unwrap_err());
    assert!(t.contains("research-rootz"), "{t}");
}

#[test]
fn example_proposal_validates_and_applies_then_goes_stale() {
    let def = research();
    let s = proposal();
    validate_suggestion(&def, &s).unwrap_or_else(|e| panic!("{}", text(&e)));
    let base = AppRevision::new(def).unwrap();
    let next = apply(&base, &s.change.operations).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((base.version(), next.version()), (1, 2));
    assert_eq!(next.definition().composition.root.find("research-search").unwrap().component, "input");
    let errs = validate_suggestion(next.definition(), &s).unwrap_err();
    assert!(errs.contains(&ValidationError::VersionMismatch { expected: 2, found: 1 }), "{}", text(&errs));
    assert!(errs.contains(&ValidationError::DuplicateKey("research-search".into())), "{}", text(&errs));
}

// ---------- CORE-006 ----------

#[test]
fn patch_batch_is_atomic_and_leaves_base_unchanged() {
    let base = AppRevision::new(research()).unwrap();
    let before = serde_json::to_string(base.definition()).unwrap();
    let insert = AppPatch { op: PatchOp::Insert, target_key: "research-root".into(), value: Some(json!({"key": "research-search", "component": "input"})) };
    let bad = AppPatch { op: PatchOp::Remove, target_key: "nope".into(), value: None };
    let err = apply(&base, &[insert.clone(), bad]).unwrap_err();
    assert_eq!(err, PatchError::TargetNotFound { op: PatchOp::Remove, key: "nope".into() });
    assert_eq!(serde_json::to_string(base.definition()).unwrap(), before, "却下された batch は元を変えない");
    assert!(base.definition().composition.root.find("research-search").is_none(), "通った方の insert も残らない");

    let p = |op: PatchOp, key: &str, value: Value| AppPatch { op, target_key: key.into(), value: Some(value) };
    // 参照切れになる bind は post-validate で落ちる
    assert!(matches!(apply(&base, &[p(PatchOp::Bind, "research-save", json!({"action_ref": "local.notes.nope"}))]), Err(PatchError::Invalid(_))));
    // root は外せない / key と children は update で変えない / 既にある key は insert できない
    assert!(matches!(apply(&base, &[AppPatch { op: PatchOp::Remove, target_key: "research-root".into(), value: None }]), Err(PatchError::RootImmovable { .. })));
    assert!(matches!(apply(&base, &[p(PatchOp::Update, "research-list", json!({"key": "x"}))]), Err(PatchError::InvalidValue { .. })));
    assert_eq!(apply(&base, &[p(PatchOp::Insert, "research-root", json!({"key": "research-list", "component": "list"}))]).unwrap_err(), PatchError::DuplicateKey("research-list".into()));
    // 自分の子孫の下へ move は循環（CORE-005 cyclic references）
    assert_eq!(
        apply(&base, &[p(PatchOp::Move, "research-content", json!({"parent": "research-editor"}))]).unwrap_err(),
        PatchError::Cycle { key: "research-content".into(), into: "research-editor".into() }
    );
    // 5 種類が全部通る道
    let next = apply(
        &base,
        &[
            insert,
            p(PatchOp::Move, "research-search", json!({"parent": "research-root", "index": 0})),
            p(PatchOp::Update, "research-search", json!({"label": "検索", "props": {"placeholder": "ノートを検索"}})),
            p(PatchOp::Bind, "research-search", json!({"binding": {"source_ref": "local.notes", "path": "query"}})),
            AppPatch { op: PatchOp::Remove, target_key: "research-save".into(), value: None },
        ],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let root = &next.definition().composition.root;
    assert_eq!(root.children[0].key, "research-search");
    assert_eq!(root.children[0].label.as_deref(), Some("検索"));
    assert_eq!(root.children[0].binding.as_ref().unwrap().path, "query");
    assert!(root.find("research-save").is_none());
    assert_eq!(next.version(), 2);
    assert_eq!(serde_json::to_string(base.definition()).unwrap(), before, "成功しても元の revision は不変");
    assert!(base.definition().composition.root.find("research-save").is_some());
}

// ---------- CORE-007 ----------

#[test]
fn run_and_suggestion_transitions_are_validated_centrally() {
    use RunStatus::*;
    let mut s = Idle;
    for to in [Planning, WaitingConnection, WaitingApproval, Running, Verifying, Uncertain, Verifying, Verified] {
        s.transition(to).unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(s.is_terminal());
    for (from, to) in [(Idle, Verified), (Verified, Running), (Verifying, Cancelled), (Cancelled, Idle), (Running, Verified), (Uncertain, Cancelled)] {
        let mut st = from;
        let err = st.transition(to).unwrap_err();
        assert_eq!((err.from, err.to), (from, to));
        assert_eq!(st, from, "落ちた遷移は状態を変えない");
        assert!(!from.can_transition(to));
    }
    for v in [Idle, Planning, WaitingConnection, WaitingApproval, Running, Verifying, Verified, Failed, Cancelled, Uncertain] {
        assert!(RUN_TRANSITIONS.iter().any(|(a, b)| *a == v || *b == v), "{v:?} が表に無い");
    }
    let mut run = Run {
        id: "run-1".into(),
        app_id: "example.research".into(),
        app_version: 1,
        workflow_ref: None,
        origin: InvocationOrigin::CommandPalette,
        status: Idle,
        steps: vec![Step { id: "s1".into(), action_ref: "local.notes.save".into(), status: Idle, attempts: vec![] }],
        created_at: "2026-10-16T00:00:00Z".into(),
        updated_at: "2026-10-16T00:00:00Z".into(),
    };
    run.status.transition(Planning).unwrap();
    run.steps[0].status.transition(Planning).unwrap();
    assert!(run.steps[0].status.transition(Verified).is_err());
    roundtrip(&run, "Run");

    use SuggestionStatus as P;
    let mut p = P::Candidate;
    for to in [P::Validated, P::Queued, P::Shown, P::Previewed, P::Snoozed, P::Queued, P::Shown, P::Approved, P::Applied, P::Verified, P::Reverted] {
        p.transition(to).unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(p.is_terminal());
    for (from, to) in [(P::Candidate, P::Applied), (P::Rejected, P::Queued), (P::Applied, P::Reverted), (P::Shown, P::Verified), (P::Queued, P::Approved)] {
        let mut st = from;
        assert!(st.transition(to).is_err(), "{from:?} -> {to:?} が通った");
        assert_eq!(st, from);
    }
}

#[test]
fn grants_and_proposal_scopes_must_be_declared_by_the_app() {
    let mut def = research();
    def.actions[0].required_scopes = vec!["notes.write".into()];
    let grant = PermissionGrant {
        id: "grant-1".into(),
        app_id: "example.research".into(),
        scope: "notes.write".into(),
        granted_at: "2026-10-16T00:00:00Z".into(),
        expires_at: Some("2026-10-17T00:00:00Z".into()),
        consent_ref: None,
    };
    validate_grant(&def, &grant).unwrap();
    assert!(grant.active_at("2026-10-16T12:00:00Z"));
    assert!(!grant.active_at("2026-10-17T00:00:00Z"));
    let other = PermissionGrant { scope: "mail.send".into(), ..grant.clone() };
    assert_eq!(validate_grant(&def, &other), Err(ValidationError::UndeclaredScope("mail.send".into())));
    let foreign = PermissionGrant { app_id: "example.timer".into(), ..grant.clone() };
    assert!(matches!(validate_grant(&def, &foreign), Err(ValidationError::AppMismatch { .. })));
    let mut s = proposal();
    s.permission_scopes = vec!["mail.send".into()];
    assert!(validate_suggestion(&def, &s).unwrap_err().contains(&ValidationError::UndeclaredScope("mail.send".into())));
    roundtrip(&grant, "PermissionGrant");
}

/// docs/examples/v3 の regression corpus 10 本（docs/examples/README.md）が domain の validate と AppRevision::new を通る。
/// unit D の但し書き「corpus が同じ branch に乗ったら commit する」を統合 1 で果たした物。file が減れば赤（一度も測らずに緑にならない）。
#[test]
fn v3_corpus_ten_apps_validate_and_become_revisions() {
    let dir = format!("{ROOT}/docs/examples/v3");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect(&dir)
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 10, "corpus は 10 本: {names:?}");
    let mut failed = Vec::new();
    let mut nodes = 0;
    for n in &names {
        let def: AppDefinitionV3 = match serde_json::from_value(read(&format!("docs/examples/v3/{n}"))) {
            Ok(d) => d,
            Err(e) => { failed.push(format!("{n}: deserialize {e}")); continue; }
        };
        if let Err(errs) = validate(&def) { failed.push(format!("{n}: {}", text(&errs))); continue; }
        nodes += def.composition.root.nodes().len();
        if let Err(errs) = AppRevision::new(def) { failed.push(format!("{n}: revision {}", text(&errs))); }
    }
    assert!(failed.is_empty(), "corpus が落ちた:\n{}", failed.join("\n"));
    assert!(nodes >= 100, "corpus の node 数が少なすぎる: {nodes}");
}

// ---- YC W1: ProjectDefinitionV1（YCM-010）と ProjectPatchOp ----
mod project_v1 {
    use super::domain::patch::{AppPatch, PatchOp};
    use super::domain::project::{apply, validate_project, PageDefinition, ProjectDefinitionV1, ProjectPatchError, ProjectPatchOp, RevisionOrigin};
    use super::domain::ui::UiNode;
    use super::research;
    use serde_json::json;

    fn node(key: &str, component: &str, children: Vec<UiNode>) -> UiNode {
        UiNode { key: key.into(), component: component.into(), children, ..Default::default() }
    }
    fn page(id: &str, root: UiNode) -> PageDefinition {
        PageDefinition { id: id.into(), route: format!("/{id}"), title: id.into(), app_ref: None, root, revision: 1 }
    }
    /// 2 枚: dev（root の下に editor と terminal の場所）・tests（空の column）
    fn two_pages() -> ProjectDefinitionV1 {
        let mut p = ProjectDefinitionV1::wrap(&research());
        p.pages = vec![
            page("dev", node("dev-root", "column", vec![node("code", "editor", vec![]), node("term", "text", vec![])])),
            page("tests", node("tests-root", "column", vec![])),
        ];
        p
    }

    #[test]
    fn serde_round_trip_and_unknown_fields_rejected() {
        let p = two_pages();
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(serde_json::from_value::<ProjectDefinitionV1>(v.clone()).unwrap(), p, "往復が一致");
        let mut extra = v.clone();
        extra["pages_v2"] = json!([]);
        assert!(serde_json::from_value::<ProjectDefinitionV1>(extra).is_err(), "Project の未知の field");
        let mut extra = v.clone();
        extra["pages"][0]["layout_mode"] = json!("grid");
        assert!(serde_json::from_value::<ProjectDefinitionV1>(extra).is_err(), "Page の未知の field");
        let op = ProjectPatchOp::MoveSurface { surface_key: "term".into(), to_page: "tests".into(), parent: "tests-root".into(), index: None };
        let ov = serde_json::to_value(&op).unwrap();
        assert_eq!(ov["op"], json!("move_surface"));
        assert_eq!(serde_json::from_value::<ProjectPatchOp>(ov.clone()).unwrap(), op);
        let mut bad = ov;
        bad["force"] = json!(true);
        assert!(serde_json::from_value::<ProjectPatchOp>(bad).is_err(), "op の未知の field");
        assert_eq!(serde_json::to_value(RevisionOrigin::NlEdit).unwrap(), json!(RevisionOrigin::NlEdit.as_str()));
    }

    #[test]
    fn wrap_is_one_page_holding_the_app_tree() {
        let a = research();
        let p = ProjectDefinitionV1::wrap(&a);
        assert_eq!(p.pages.len(), 1);
        assert_eq!(p.pages[0].root, a.composition.root);
        assert_eq!(p.pages[0].app_ref.as_deref(), Some(a.id.as_str()));
        assert_eq!(p, ProjectDefinitionV1::wrap(&a), "同じ app から同じ Project（id に乱数を使わない）");
        assert!(validate_project(&p, &[&a]).is_ok());
        assert!(validate_project(&p, &[]).is_err(), "指す app が無ければ照合できない");
    }

    #[test]
    fn ops_apply_and_bump_only_touched_pages() {
        let base = two_pages();
        let ops = vec![
            ProjectPatchOp::AddPage { page: page("git", node("git-root", "list", vec![])), index: Some(1) },
            ProjectPatchOp::MoveSurface { surface_key: "term".into(), to_page: "tests".into(), parent: "tests-root".into(), index: None },
            ProjectPatchOp::RenamePage { page_id: "tests".into(), title: "Tests".into() },
            ProjectPatchOp::Page { page_id: "dev".into(), patch: AppPatch { op: PatchOp::Update, target_key: "code".into(), value: Some(json!({ "label": "Code" })) } },
            ProjectPatchOp::MovePage { page_id: "tests".into(), index: 0 },
        ];
        let p = apply(&base, &ops, &[]).unwrap();
        assert_eq!(p.revision, base.revision + 1);
        assert_eq!(p.pages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec!["tests", "dev", "git"]);
        assert!(p.page("tests").unwrap().root.find("term").is_some() && p.page("dev").unwrap().root.find("term").is_none(), "Page をまたいで動いた");
        assert_eq!(p.page("dev").unwrap().root.find("code").unwrap().label.as_deref(), Some("Code"));
        assert_eq!((p.page("dev").unwrap().revision, p.page("tests").unwrap().revision, p.page("git").unwrap().revision), (2, 2, 1));
        assert_eq!(base, two_pages(), "元は変わらない");
        // 触らない Page は前後で同じ
        let q = apply(&p, &[ProjectPatchOp::RenamePage { page_id: "git".into(), title: "Git".into() }], &[]).unwrap();
        assert_eq!(q.page("dev"), p.page("dev"));
    }

    #[test]
    fn all_or_nothing_and_rejections_say_what_to_do() {
        let base = two_pages();
        let cases: Vec<(Vec<ProjectPatchOp>, fn(&ProjectPatchError) -> bool)> = vec![
            // 1 つ目は当たるが 2 つ目が落ちる → 何も変わらない
            (vec![ProjectPatchOp::AddPage { page: page("git", node("git-root", "list", vec![])), index: None }, ProjectPatchOp::RemovePage { page_id: "nope".into() }], |e| matches!(e, ProjectPatchError::PageNotFound(_))),
            (vec![ProjectPatchOp::AddPage { page: page("dev", node("x", "list", vec![])), index: None }], |e| matches!(e, ProjectPatchError::DuplicatePage(_))),
            (vec![ProjectPatchOp::RemovePage { page_id: "tests".into() }, ProjectPatchOp::RemovePage { page_id: "dev".into() }], |e| matches!(e, ProjectPatchError::LastPage(_))),
            (vec![ProjectPatchOp::MoveSurface { surface_key: "ghost".into(), to_page: "tests".into(), parent: "tests-root".into(), index: None }], |e| matches!(e, ProjectPatchError::SurfaceNotFound(_))),
            (vec![ProjectPatchOp::AddPage { page: page("bad", node("bad-root", "hologram", vec![])), index: None }], |e| matches!(e, ProjectPatchError::Invalid(_))),
            (vec![ProjectPatchOp::Page { page_id: "dev".into(), patch: AppPatch { op: PatchOp::Remove, target_key: "dev-root".into(), value: None } }], |e| matches!(e, ProjectPatchError::Page { .. })),
        ];
        for (ops, want) in cases {
            let err = apply(&base, &ops, &[]).unwrap_err();
            assert!(want(&err), "{ops:?} → {err:?}");
            assert!(!err.to_string().is_empty());
        }
        // 2 枚に同じ key があると、どれを動かすか決めない
        let mut dup = two_pages();
        dup.pages[1].root.children.push(node("term", "text", vec![]));
        let err = apply(&dup, &[ProjectPatchOp::MoveSurface { surface_key: "term".into(), to_page: "dev".into(), parent: "dev-root".into(), index: None }], &[]).unwrap_err();
        assert!(matches!(err, ProjectPatchError::AmbiguousSurface(_)), "{err:?}");
        assert!(ProjectPatchError::LastPage("p".into()).to_string().contains("先に別の Page を足して"));
        assert!(validate_project(&{ let mut p = two_pages(); p.pages[0].root.children.push(node("z", "hologram", vec![])); p }, &[]).unwrap_err().iter().any(|e| e.contains("unsupported")));
    }

    /// W1 反証: Page の action_ref / binding は、その Page が app_ref で指す app の actions / sources に在る物だけ（patch::apply の MissingAction / MissingSource と同じ）。
    #[test]
    fn page_refs_must_resolve_in_the_app_the_page_points_to() {
        let a = research();
        let base = ProjectDefinitionV1::wrap(&a);
        let pid = base.pages[0].id.clone();
        let key = base.pages[0].root.nodes().iter().find(|n| n.action_ref.is_some()).expect("research に action を持つ node").key.clone();
        let bind = |v: serde_json::Value| ProjectPatchOp::Page { page_id: pid.clone(), patch: AppPatch { op: PatchOp::Bind, target_key: key.clone(), value: Some(v) } };
        let invalid = |r: Result<ProjectDefinitionV1, ProjectPatchError>, what: &str| match r {
            Err(ProjectPatchError::Invalid(errs)) => assert!(errs.iter().any(|e| e.contains(what)), "{errs:?}"),
            other => panic!("{what} を拒否しない: {other:?}"),
        };
        invalid(apply(&base, &[bind(json!({ "action_ref": "no.such.action" }))], &[&a]), "no.such.action");
        invalid(apply(&base, &[bind(json!({ "binding": { "source_ref": "no.such.source", "path": "x" } }))], &[&a]), "no.such.source");
        // 在る action への付け替えは当たる
        let ok = a.actions.last().unwrap().id.clone();
        assert_eq!(apply(&base, &[bind(json!({ "action_ref": ok.clone() }))], &[&a]).unwrap().pages[0].root.find(&key).unwrap().action_ref.as_deref(), Some(ok.as_str()));
        // app を指さない Page へ action を持つ node を動かすと、参照が宙に浮く
        let ops = vec![
            ProjectPatchOp::AddPage { page: page("bare", node("bare-root", "column", vec![])), index: None },
            ProjectPatchOp::MoveSurface { surface_key: key.clone(), to_page: "bare".into(), parent: "bare-root".into(), index: None },
        ];
        invalid(apply(&base, &ops, &[&a]), "app を指さない");
        // Page が指す app を渡さなければ照合できないので拒否
        invalid(apply(&base, &[ProjectPatchOp::RenamePage { page_id: pid.clone(), title: "x".into() }], &[]), "開いていません");
    }
}

// ---- YC W2c: SurfaceSpec・SurfaceReceipt の契約（serde の往復・参照の照合・verified の欄の照合）----
mod surface_w2c {
    use super::domain::project::{validate_project, ProjectDefinitionV1, ReceiptError, ReceiptKind, ReceiptState, SurfaceBackendKind, SurfaceReceipt, SurfaceSpec, SurfaceTarget};
    use super::domain::ui::UiNode;
    use super::research;
    use serde_json::json;

    fn textedit() -> SurfaceSpec {
        SurfaceSpec {
            id: "docs".into(),
            backend: SurfaceBackendKind::NativeDirect,
            target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("notes.txt".into()) },
            keep_alive: true,
            frame: None,
            browser: false,
        }
    }

    /// app の木の root の子に source_surface を 1 つ足した Project（surface_ref = r）。
    fn with_ref(r: &str) -> ProjectDefinitionV1 {
        let mut p = ProjectDefinitionV1::wrap(&research());
        let mut n = UiNode { key: "docs-surface".into(), component: "source_surface".into(), ..Default::default() };
        n.props.insert("display".into(), json!("native"));
        n.props.insert("surface_ref".into(), json!(r));
        p.pages[0].root.children.push(n);
        p
    }

    #[test]
    fn surface_specs_round_trip_and_w1_json_still_reads() {
        let mut p = ProjectDefinitionV1::wrap(&research());
        p.surface_specs = vec![textedit(), SurfaceSpec { id: "diagram".into(), backend: SurfaceBackendKind::WebView, target: SurfaceTarget::Url { url: "https://excalidraw.com".into() }, keep_alive: false, frame: None, browser: false }];
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["surface_specs"][1]["backend"], json!("webview"));
        assert_eq!(v["surface_specs"][0]["target"]["kind"], json!("app"));
        assert_eq!(serde_json::from_value::<ProjectDefinitionV1>(v.clone()).unwrap(), p, "往復が一致");
        // W1 の保存（surface_specs の欄が無い json）も読める（#[serde(default)]）
        let mut w1 = v.clone();
        w1.as_object_mut().unwrap().remove("surface_specs");
        assert!(serde_json::from_value::<ProjectDefinitionV1>(w1).unwrap().surface_specs.is_empty());
        // handle（pid・窓 id）は持てない
        let mut extra = v;
        extra["surface_specs"][0]["pid"] = json!(123);
        assert!(serde_json::from_value::<ProjectDefinitionV1>(extra).is_err(), "SurfaceSpec の未知の field");
    }

    #[test]
    fn surface_ref_must_point_at_a_spec() {
        let a = research();
        let mut ok = with_ref("docs");
        ok.surface_specs.push(textedit());
        assert_eq!(validate_project(&ok, &[&a]), Ok(()));
        let errs = validate_project(&with_ref("docs"), &[&a]).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("surface_ref 'docs'")), "{errs:?}");
        let mut dup = ok.clone();
        dup.surface_specs.push(textedit());
        assert!(validate_project(&dup, &[&a]).unwrap_err().iter().any(|e| e.contains("重複")));
    }

    fn verified() -> SurfaceReceipt {
        SurfaceReceipt {
            surface_id: "docs".into(),
            page_id: "page.docs".into(),
            kind: ReceiptKind::NativeApp,
            state: ReceiptState::Verified,
            oracle: "ax_value".into(),
            app_id: "com.apple.TextEdit".into(),
            app_version: "1.19".into(),
            os_version: "14.5 (23F79)".into(),
            operation: "本文に nonce を入力".into(),
            verified_at: "2026-10-20T00:00:00Z".into(),
            evidence_ref: "docs/evidence/W2c/receipt.json".into(),
        }
    }

    #[test]
    fn verified_receipt_needs_all_six_fields_and_not_local() {
        assert_eq!(verified().validate(), Ok(()));
        for (name, clear) in [
            ("app_id", (|r: &mut SurfaceReceipt| r.app_id.clear()) as fn(&mut SurfaceReceipt)),
            ("app_version", |r| r.app_version.clear()),
            ("os_version", |r| r.os_version.clear()),
            ("operation", |r| r.operation = " ".into()),
            ("verified_at", |r| r.verified_at.clear()),
            ("evidence_ref", |r| r.evidence_ref.clear()),
        ] {
            let mut r = verified();
            clear(&mut r);
            assert_eq!(r.validate(), Err(ReceiptError::MissingField(name)), "{name}");
            r.state = ReceiptState::Unverified;
            assert_eq!(r.validate(), Ok(()), "unverified は欄が空でよい: {name}");
        }
        let mut local = verified();
        local.kind = ReceiptKind::Local;
        assert_eq!(local.validate(), Err(ReceiptError::LocalVerified));
        let v = serde_json::to_value(verified()).unwrap();
        assert_eq!((v["kind"].clone(), v["state"].clone()), (json!("native_app"), json!("verified")));
        assert_eq!(serde_json::from_value::<SurfaceReceipt>(v).unwrap(), verified());
    }
}

/// W5 interface: Page の `terminal` の席と `terminal_specs` の照合・`AddTerminal` の保存形式。
mod terminal_w5 {
    use super::domain::project::{validate_project, ProjectDefinitionV1, ProjectPatchOp, TerminalSessionSpec};
    use super::domain::ui::UiNode;
    use super::research;
    use serde_json::json;

    fn spec(p: &ProjectDefinitionV1) -> TerminalSessionSpec {
        TerminalSessionSpec { id: "t1".into(), project_id: p.id.clone(), shell: "/bin/zsh".into(), working_directory: "/tmp".into(), keep_alive_on_page_switch: true, command: None }
    }

    /// 先頭 Page の root の子に terminal を 1 つ足した Project（terminal_ref = r・None なら props 無し）。
    fn with_terminal(component: &str, r: Option<&str>) -> ProjectDefinitionV1 {
        let mut p = ProjectDefinitionV1::wrap(&research());
        let mut n = UiNode { key: "dev-terminal".into(), component: component.into(), ..Default::default() };
        if let Some(r) = r {
            n.props.insert("terminal_ref".into(), json!(r));
        }
        p.pages[0].root.children.push(n);
        p
    }

    #[test]
    fn terminal_ref_must_point_at_a_spec_of_this_project() {
        let a = research();
        let mut ok = with_terminal("terminal", Some("t1"));
        ok.terminal_specs.push(spec(&ok));
        assert_eq!(validate_project(&ok, &[&a]), Ok(()));
        let bad = |p: &ProjectDefinitionV1, needle: &str| {
            let errs = validate_project(p, &[&a]).unwrap_err();
            assert!(errs.iter().any(|e| e.contains(needle)), "{needle}: {errs:?}");
        };
        bad(&with_terminal("terminal", Some("t1")), "terminal_ref 't1'");
        bad(&with_terminal("terminal", None), "terminal_ref ''");
        let mut not_terminal = with_terminal("text", Some("t1"));
        not_terminal.terminal_specs.push(spec(&not_terminal));
        bad(&not_terminal, "terminal だけが持てる");
        let mut dup = ok.clone();
        dup.terminal_specs.push(spec(&ok));
        bad(&dup, "Terminal の id 't1' が空か重複");
        let mut other = ok.clone();
        other.terminal_specs[0].project_id = "project.other".into();
        bad(&other, "この Project ではない");
        // Page に置かれていない spec は許す（Page から外しても shell は止めない）
        let mut unplaced = ProjectDefinitionV1::wrap(&a);
        unplaced.terminal_specs.push(spec(&unplaced));
        assert_eq!(validate_project(&unplaced, &[&a]), Ok(()));
    }

    #[test]
    fn add_terminal_op_round_trips() {
        let p = ProjectDefinitionV1::wrap(&research());
        let op = ProjectPatchOp::AddTerminal { spec: spec(&p), page_id: p.pages[0].id.clone(), parent: p.pages[0].root.key.clone(), node_key: "terminal.t1".into(), index: None };
        let v = serde_json::to_value(&op).unwrap();
        assert_eq!(v["op"], json!("add_terminal"));
        assert!(v.get("index").is_none(), "index 無しは書かない");
        assert_eq!(serde_json::from_value::<ProjectPatchOp>(v).unwrap(), op);
    }
}
/// W16M-1: E0 の判定（試験の側の純関数・tests/common/readback.rs）が、黒い画面・止まった映像・遅い映像・写真 0 枚で赤になる事。
#[path = "common/readback.rs"]
mod readback;

mod mirror_e0_w16m1 {
    use super::readback::{e0_current, OcrShot};

    fn shot(at_ms: f64, lines: &[&str]) -> OcrShot {
        OcrShot { at_ms, lines: lines.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn w16m1_e0_passes_only_when_the_mirror_follows_the_app() {
        let (n1, n2, t) = ("e0-n1-7f3a", "e0-n2-c41d", 10_000.0);
        let before = [shot(9_000.0, &["notes.md", "e0-n1-", "7f3a"])]; // OCR が行を分けても拾う
        // 正の対照: 0.8 秒後の写真に nonce2・nonce1 は消えた
        assert_eq!(e0_current(&before, n1, &[shot(10_300.0, &["e0-n1-7f3a"]), shot(10_800.0, &["e0-n2-c41d"])], n2, t), Ok(()));
        // 黒い texture: 行 0
        assert!(e0_current(&[shot(9_000.0, &[])], n1, &[shot(10_500.0, &[])], n2, t).is_err());
        // 最初の frame で止めた stream: 入れ替えた後も nonce1 のまま
        assert!(e0_current(&before, n1, &[shot(10_500.0, &["e0-n1-7f3a"]), shot(11_900.0, &["e0-n1-7f3a"])], n2, t).is_err());
        // 遅い: nonce2 が 2.5 秒後
        assert!(e0_current(&before, n1, &[shot(12_500.0, &["e0-n2-c41d"])], n2, t).is_err());
        // 入れ替えの前に撮った写真の nonce2 は数えない
        assert!(e0_current(&before, n1, &[shot(9_900.0, &["e0-n2-c41d"])], n2, t).is_err());
        // 写真 0 枚は未測（緑にしない）
        assert!(e0_current(&[], n1, &[], n2, t).unwrap_err().contains("未測"));
    }

    /// W16M-1b: OCR と製品の log の両方が通った時だけ緑（log だけ・OCR だけ・log が 2 秒超・どちらかが未測 → 赤）
    #[test]
    fn w16m1b_e0_needs_both_the_ocr_and_the_product_log() {
        use super::readback::e0_verdict;
        assert_eq!(e0_verdict(Ok(()), Ok(800.0)), Ok(800.0));
        assert!(e0_verdict(Err("入れ替えた後も nonce2 が写らない".into()), Ok(800.0)).is_err(), "製品の log だけで緑にした");
        assert!(e0_verdict(Ok(()), Err("未測: 記録が無い".into())).unwrap_err().contains("未測"), "OCR だけで緑にした");
        assert!(e0_verdict(Ok(()), Ok(4_000.0)).is_err(), "log の遅れ 4 秒を緑にした");
    }
}

/// W16M-2: E1 の経路の判定（試験の側の純関数・tests/common/readback.rs）が、relay の無い変化（試験の側から直接 postToPid）・relay より前の変化・
/// 変化 0 件で赤になる事。
mod mirror_e1_w16m2 {
    use super::readback::{e1_path, relay_facts, RelayFact, ValueChange};

    fn ch(at_ms: f64, what: &str) -> ValueChange {
        ValueChange { at_ms, what: what.into() }
    }

    #[test]
    fn w16m2_e1_needs_a_relay_before_each_change() {
        // 製品の relay の記録（v 2）を JSONL のまま読む・空行は飛ばす
        let log = "{\"v\":2,\"seq\":1,\"at_ms\":1000.5,\"kind\":\"text\",\"route\":\"post_to_pid\",\"pid\":42,\"window_id\":7,\"pos\":null}\n\n\
                   {\"v\":2,\"seq\":2,\"at_ms\":3000.0,\"kind\":\"key\",\"route\":\"save_document\",\"pid\":42,\"window_id\":7,\"pos\":null}\n";
        let relays = relay_facts(log).unwrap();
        assert_eq!(relays, vec![RelayFact { seq: 1, at_ms: 1000.5 }, RelayFact { seq: 2, at_ms: 3000.0 }]);
        // MIRROR-HYBRID-1101 追記 235: v 3 の答えの行（送った後の時刻）は E1 の relay に数えない（負の対照〈未実行〉: 飛ばさない → 3 件で赤）
        let answered = format!("{log}{{\"v\":3,\"seq\":1,\"at_ms\":2500.0,\"kind\":\"text\",\"route\":\"post_to_pid\",\"pid\":42,\"window_id\":7,\"pos\":null,\"outcome\":\"delivered\",\"ms\":3.0}}\n");
        assert_eq!(relay_facts(&answered).unwrap(), relays, "答えの行を送った relay に数えた");
        // 正の対照: 打つ → AX value が変わる・⌘S → disk の sha が変わる（並びが逆でも時刻で並べる）
        assert_eq!(e1_path(&relays, &[ch(3400.0, "notes.md の sha"), ch(1200.0, "TextEdit の AX value")]), Ok(2));
        // 負の対照: relay 0 件で値が変わった（試験の側から TextEdit に直接 postToPid）
        assert!(e1_path(&[], &[ch(1200.0, "TextEdit の AX value")]).unwrap_err().contains("relay の記録の無い変化"));
        // relay 1 件の後に変化が 2 つ（2 つ目は Shibubu を通っていない）
        assert!(e1_path(&relays[..1], &[ch(1200.0, "AX value"), ch(1500.0, "AX value")]).is_err());
        // 変化が relay より前（時刻の順が変化 → relay）
        assert!(e1_path(&relays[1..], &[ch(2500.0, "AX value")]).is_err());
        // 変化 0 件は未測（緑にしない）
        assert!(e1_path(&relays, &[]).unwrap_err().contains("未測"));
        // seq・at_ms の無い行・JSON でない行は Err
        assert!(relay_facts("{\"at_ms\":1.0}").is_err());
        assert!(relay_facts("not json").is_err());
    }
}
