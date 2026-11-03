//! unit R（native renderer）の AC test: UI-005 / 006 / 007 / 009 の最小 / 014。egui の headless `Context::run` で描く（窓は要らない）。
//! bin crate に lib target が無いので `#[path]` で domain と ui を test crate の root に置く（renderer は `crate::domain` / `crate::ui` で引く）。
//! 画面の確かめは AccessKit の出力（cua が読む物と同じ木）と、描いた shape（card の入れ子）で行う。
#![allow(dead_code, unused_imports)]
#[path = "../src/domain/mod.rs"]
mod domain;
// ui::agent_rail は agent を crate root の sibling として引く（src/main.rs と同じ並び）
#[path = "../src/agent/mod.rs"]
mod agent;
#[allow(dead_code)] // ui::portal_frame が PortalState / SyncMode を再輸出する（W2c）
#[path = "../src/surface/mod.rs"]
mod surface;
// shell の Intent が model::AiAccount を運ぶ。model は時刻だけ google を、google は crypto を引く（adapters と同じ 3 つ）
#[allow(dead_code)]
#[path = "../src/crypto.rs"]
mod crypto;
#[allow(dead_code)]
#[path = "../src/google/mod.rs"]
mod google;
#[allow(dead_code)]
#[path = "../src/model.rs"]
mod model;
#[path = "../src/ui/mod.rs"]
mod ui;

use domain::action::{ActionInvocation, ActionReadiness, InvocationOrigin};
use domain::app::AppDefinitionV3;
use domain::binding::DataBinding;
use domain::ui::{UiNode, BUILTIN_COMPONENTS};
use eframe::egui::{self, epaint, vec2, Pos2, Rect};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use ui::renderer::{self, inspector_rows, ActionHost, AppState};
use ui::states::RegionState;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn load(rel: &str) -> AppDefinitionV3 {
    let path = format!("{ROOT}/docs/examples/{rel}.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).expect(&path)
}

/// 設計サンプル 3 本 + corpus 10 本（docs/examples/v3 を読む: 本数は file 数で決まる）。
fn fixtures() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(format!("{ROOT}/docs/examples/v3"))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json").map(|n| format!("v3/{n}")))
        .collect();
    names.sort();
    names.extend(["app_v3_timer", "app_v3_research", "app_v3_messages"].map(String::from));
    names
}

/// runtime の fake: data を返し、invoke を記録する。`ready` の action は runtime が Ready と言う（接続後の上書き）。
#[derive(Default)]
struct Fake {
    data: HashMap<(String, String), Value>,
    ready: Vec<String>,
    regions: HashMap<String, (RegionState, String)>,
    invoked: Vec<ActionInvocation>,
    /// 送った時に runtime がその node に置く region（保存が通らなかった）。None = 通した（region を置かない）。
    fails_with: Option<(RegionState, String)>,
    /// W3: 面ごとの最新の読み戻し（`surface_receipt` が返す）。
    receipts: HashMap<String, domain::project::SurfaceReceipt>,
    /// W5: Page の Terminal（None = Project の外）と、送った行・起こし直した ref
    terminal: Option<renderer::TerminalPane>,
    sent: Vec<(String, String)>,
    restarted: Vec<String>,
    /// G94: `terminal_restart` が返す Err（None = 起こせた）
    restart_err: Option<String>,
    /// W10: grid に focus の間に送った byte と、頼まれた大きさ（行・列）。
    keys: Vec<(String, Vec<u8>)>,
    resized: Vec<(String, u16, u16)>,
    /// W11: 入力欄の focus で呼んだ touch / release
    touched: Vec<(String, domain::terminal::InputView)>,
    released: Vec<(String, domain::terminal::InputView)>,
    /// W12W-1: Page に置く面（`embeds` が true）と、`place_surface` で受けた矩形
    embeds: Vec<String>,
    placed: Vec<(String, Rect)>,
    /// W12W-B: Browser の上の欄と履歴・frame
    go: Vec<(String, String)>,
    back: Vec<String>,
    forward: Vec<String>,
    reload: Vec<String>,
    frames: Vec<(String, f64, f64, f64, f64)>,
    urls: HashMap<String, String>,
    /// UI-6a（60.1）: 押し直すと 2 回届く node（`locked`）
    locked: Vec<String>,
    /// G88b: 録音の帯の今と、押された手
    take: Option<renderer::TakePane>,
    take_acts: Vec<(String, renderer::TakeAct)>,
    /// L7-ORCA #9: grid の右 click の menu で選んだ panel の手
    panel_acts: Vec<(String, ui::terminal_panel::PanelAct)>,
}
impl Fake {
    fn with(mut self, source: &str, path: &str, v: Value) -> Self {
        self.data.insert((source.into(), path.into()), v);
        self
    }
}
impl ActionHost for Fake {
    fn readiness(&self, id: &str) -> Option<ActionReadiness> {
        self.ready.iter().any(|r| r == id).then_some(ActionReadiness::Ready)
    }
    fn data(&self, b: &DataBinding) -> Option<Value> {
        self.data.get(&(b.source_ref.clone(), b.path.clone())).cloned()
    }
    fn region(&self, key: &str) -> Option<(RegionState, String)> {
        self.regions.get(key).cloned()
    }
    fn locked(&self, key: &str) -> bool {
        self.locked.iter().any(|k| k == key)
    }
    fn surface_receipt(&self, id: &str) -> Option<domain::project::SurfaceReceipt> {
        self.receipts.get(id).cloned()
    }
    fn embeds(&self, id: &str) -> bool {
        self.embeds.iter().any(|e| e == id)
    }
    fn place_surface(&mut self, id: &str, slot: Rect, _shown: Rect) {
        self.placed.push((id.into(), slot));
    }
    fn browser_go(&mut self, surface_id: &str, input: &str) {
        self.go.push((surface_id.into(), input.into()));
    }
    fn browser_back(&mut self, surface_id: &str) {
        self.back.push(surface_id.into());
    }
    fn browser_forward(&mut self, surface_id: &str) {
        self.forward.push(surface_id.into());
    }
    fn browser_reload(&mut self, surface_id: &str) {
        self.reload.push(surface_id.into());
    }
    fn browser_frame(&mut self, surface_id: &str, x: f64, y: f64, w: f64, h: f64) {
        self.frames.push((surface_id.into(), x, y, w, h));
    }
    fn browser_url(&self, surface_id: &str) -> Option<String> {
        self.urls.get(surface_id).cloned()
    }
    fn invoke(&mut self, inv: ActionInvocation) {
        if let (Some(r), InvocationOrigin::Button { node_key }) = (self.fails_with.clone(), &inv.origin) {
            self.regions.insert(node_key.clone(), r);
        }
        self.invoked.push(inv);
    }
    fn terminal(&mut self, _r: &str) -> Option<renderer::TerminalPane> {
        self.terminal.clone()
    }
    fn terminal_send(&mut self, r: &str, line: &str) -> Result<(), String> {
        self.sent.push((r.into(), line.into()));
        Ok(())
    }
    fn terminal_restart(&mut self, r: &str) -> Result<(), String> {
        self.restarted.push(r.into());
        self.restart_err.clone().map_or(Ok(()), Err)
    }
    fn take(&mut self, _r: &str) -> Option<renderer::TakePane> {
        self.take.clone()
    }
    fn take_act(&mut self, r: &str, act: renderer::TakeAct) -> Result<(), String> {
        self.take_acts.push((r.into(), act));
        Ok(())
    }
    fn terminal_keys(&mut self, r: &str, bytes: &[u8]) -> Result<(), String> {
        self.keys.push((r.into(), bytes.to_vec()));
        Ok(())
    }
    fn terminal_resize(&mut self, r: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.resized.push((r.into(), rows, cols));
        Ok(())
    }
    fn terminal_touch_human(&mut self, r: &str, view: domain::terminal::InputView) -> Result<(), String> {
        self.touched.push((r.into(), view));
        Ok(())
    }
    fn terminal_release_human(&mut self, r: &str, view: &domain::terminal::InputView) -> Result<(), String> {
        self.released.push((r.into(), view.clone()));
        Ok(())
    }
    fn terminal_panel_act(&mut self, r: &str, act: ui::terminal_panel::PanelAct) -> Result<(), String> {
        self.panel_acts.push((r.into(), act));
        Ok(())
    }
}

/// AccessKit の 1 要素（cua が見る物と同じ: role・名前・値・位置・押せるか）。
#[derive(Debug, Clone)]
struct Ax {
    role: String,
    text: String,
    rect: Rect,
    disabled: bool,
    /// AccessKit の toggled（egui は WidgetInfo の selected をここへ出す: list / table の行・tab は True / False、値の無い checkbox は Mixed）。
    toggled: Option<String>,
}

struct Harness {
    ctx: egui::Context,
    width: f32,
    /// 窓の高さ（既定 900）。これより下に描いた物は押せない（主領域の scroll の外）。
    height: f32,
}
impl Harness {
    fn new(width: f32) -> Self {
        let ctx = egui::Context::default();
        ui::theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        Harness { ctx, width, height: 900.0 }
    }
    /// 1 frame。殻の `draw_app_region` の中に render する（本番と同じ入れ物・同じ gutter と最大幅）。
    fn frame(&self, app: &AppDefinitionV3, state: &mut AppState, host: &mut Fake, events: Vec<egui::Event>) -> egui::FullOutput {
        self.frame_focused(app, state, host, events, true)
    }
    /// `focused` = 窓が前（false = 後ろの Shibubu に pid 宛ての key が届く場面）。
    fn frame_focused(&self, app: &AppDefinitionV3, state: &mut AppState, host: &mut Fake, events: Vec<egui::Event>, focused: bool) -> egui::FullOutput {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(self.width, self.height))), events, focused, ..Default::default() };
        self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui::shell::draw_app_region(ui, &mut |ui| renderer::render(ui, app, state, host));
            });
        })
    }
    /// 2 frame（1 frame 目で測った大きさが 2 frame 目の配置に効く物があるので、見るのは 2 frame 目）。
    fn settle(&self, app: &AppDefinitionV3, state: &mut AppState, host: &mut Fake) -> egui::FullOutput {
        self.frame(app, state, host, vec![]);
        self.frame(app, state, host, vec![])
    }
    /// 押して離す（押す frame・離す frame = click が立つ・もう 1 frame で結果を描く）。`secondary` は右 click。
    fn click_with(&self, app: &AppDefinitionV3, state: &mut AppState, host: &mut Fake, pos: Pos2, secondary: bool) -> egui::FullOutput {
        let modifiers = egui::Modifiers::default();
        let button = if secondary { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
        self.frame(app, state, host, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton { pos, button, pressed: true, modifiers }]);
        self.frame(app, state, host, vec![egui::Event::PointerButton { pos, button, pressed: false, modifiers }]);
        self.settle(app, state, host)
    }
    fn click(&self, app: &AppDefinitionV3, state: &mut AppState, host: &mut Fake, pos: Pos2) -> egui::FullOutput {
        self.click_with(app, state, host, pos, false)
    }
    /// W12W-B: Page の木 + `surface_specs` で描く（chrome の AX は `render` では出ない）。
    fn frame_page(&self, app: &AppDefinitionV3, page: &domain::project::PageDefinition, state: &mut AppState, host: &mut Fake, specs: &[domain::project::SurfaceSpec], events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(self.width, self.height))), events, ..Default::default() };
        self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui::shell::draw_app_region(ui, &mut |ui| renderer::render_page(ui, app, page, state, host, renderer::PageSurfaces { specs, ..Default::default() }));
            });
        })
    }
    fn click_page(&self, app: &AppDefinitionV3, page: &domain::project::PageDefinition, state: &mut AppState, host: &mut Fake, specs: &[domain::project::SurfaceSpec], pos: Pos2) -> egui::FullOutput {
        let modifiers = egui::Modifiers::default();
        let button = egui::PointerButton::Primary;
        self.frame_page(app, page, state, host, specs, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton { pos, button, pressed: true, modifiers }]);
        self.frame_page(app, page, state, host, specs, vec![egui::Event::PointerButton { pos, button, pressed: false, modifiers }]);
        self.frame_page(app, page, state, host, specs, vec![])
    }
    /// 人の手の間（egui は 0.3 秒以内の 2 回目の click を double click、0.6 秒以内の 3 回目を triple click と読み、入力欄では語や行を選ぶ）。
    /// frame の時刻は 1 frame 1/60 秒で進むので、40 frame（約 0.67 秒）空ける。入力欄を押す前、直前に別の物を押していたら挟む。
    fn pause(&self, app: &AppDefinitionV3, state: &mut AppState, host: &mut Fake) {
        for _ in 0..40 {
            self.frame(app, state, host, vec![]);
        }
    }
}

fn ax(out: &egui::FullOutput) -> Vec<Ax> {
    let Some(update) = &out.platform_output.accesskit_update else { return Vec::new() };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            Some(Ax {
                role: format!("{:?}", n.role()),
                text: n.label().or(n.value()).unwrap_or("").to_owned(),
                rect: Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32)),
                disabled: n.is_disabled(),
                toggled: n.toggled().map(|t| format!("{t:?}")),
            })
        })
        .collect()
}

/// 名前（label か value）が完全一致する AX 要素。無ければ panic（木を出して）。
fn find(out: &egui::FullOutput, text: &str) -> Ax {
    let all = ax(out);
    all.iter().find(|a| a.text == text).cloned().unwrap_or_else(|| panic!("AX に '{text}' が無い: {:?}", all.iter().map(|a| &a.text).collect::<Vec<_>>()))
}

fn has_text(out: &egui::FullOutput, part: &str) -> bool {
    ax(out).iter().any(|a| a.text.contains(part))
}

/// 描かれるべき node: `has_selection` が偽の間の条件付き node（と子）と、選ばれていない tab の子（と子）を除いた全部。
fn expected_keys<'a>(root: &'a UiNode, state: &AppState) -> BTreeSet<&'a str> {
    fn walk<'a>(n: &'a UiNode, state: &AppState, out: &mut BTreeSet<&'a str>) {
        let when = n.visibility.as_ref().and_then(|v| v.get("when")).and_then(Value::as_str);
        if when == Some("has_selection") && state.rows.is_empty() {
            return;
        }
        out.insert(&n.key);
        let kids: Vec<&UiNode> = if n.component == "tabs" {
            n.children.get(state.tabs.get(&n.key).copied().unwrap_or(0)).into_iter().collect()
        } else {
            n.children.iter().collect()
        };
        for c in kids {
            walk(c, state, out);
        }
    }
    let mut out = BTreeSet::new();
    walk(root, state, &mut out);
    out
}

/// fill と stroke を両方持つ大きめの面（= card）。button（36）・1 行の入力欄（38）は外す。
fn cards(out: &egui::FullOutput) -> Vec<Rect> {
    fn walk(s: &epaint::Shape, out: &mut Vec<Rect>) {
        match s {
            epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            epaint::Shape::Rect(r) if r.fill.a() > 0 && r.stroke.width > 0.0 && r.rect.height() > 44.0 && r.rect.width() > 44.0 => out.push(r.rect),
            _ => {}
        }
    }
    let mut v = Vec::new();
    for c in &out.shapes {
        walk(&c.shape, &mut v);
    }
    v
}

fn nested_cards(out: &egui::FullOutput) -> Vec<(Rect, Rect)> {
    let c = cards(out);
    let mut pairs = Vec::new();
    for (i, a) in c.iter().enumerate() {
        for (j, b) in c.iter().enumerate() {
            if i != j && a.expand(0.5).contains_rect(*b) && a != b {
                pairs.push((*a, *b));
            }
        }
    }
    pairs
}

/// corpus の data 一式（行が在る時の描画・card 検査用）。中身は test が作った物で、renderer は data を作らない。
fn rich(app: &AppDefinitionV3) -> Fake {
    let mut fake = Fake::default();
    for n in app.composition.root.nodes() {
        let Some(b) = &n.binding else { continue };
        let v = match n.component.as_str() {
            "list" | "chat" => json!([{"title": "一件目", "text": "本文", "source": "local"}, {"title": "二件目", "text": "本文 2"}]),
            "table" => json!([{"name": "一件目", "quantity": 3, "title": "一件目", "recipient": "A"}]),
            "detail" => json!({"title": "一件目", "status": "ok", "note": "メモ"}),
            "calendar" => json!([{"start": "2026-10-17T10:00", "title": "打合せ"}]),
            "timeline" => json!([{"title": "切る", "at": "00:10"}]),
            "media" | "source_surface" => json!({"title": "clip.mov"}),
            "run_progress" => json!({"status": "verifying", "steps": [{"title": "test", "status": "verified"}]}),
            "waveform" => json!([0.1, -0.4, 0.8, -0.2]),
            "editor" => json!("本文の下書き"),
            "text" => json!("12:00"),
            "form" => json!({"quantity": 3}),
            _ => continue,
        };
        fake.data.insert((b.source_ref.clone(), b.path.clone()), v);
    }
    fake
}

/// UI-005 / 014: 13 本の定義（設計サンプル 3 + corpus 10）が同じ code を通って、見えるはずの node が全部描かれる。
/// 1440 と 960（最小の段）の 2 幅。描いただけで invoke しない。
#[test]
fn corpus_13_render_every_visible_node_without_panic() {
    let names = fixtures();
    assert_eq!(names.len(), 13, "{names:?}");
    for width in [1440.0, 960.0] {
        let h = Harness::new(width);
        for name in &names {
            let app = load(name);
            for mut host in [Fake::default(), rich(&app)] {
                let mut state = AppState::default();
                h.settle(&app, &mut state, &mut host);
                let drawn: BTreeSet<&str> = state.rects.keys().map(String::as_str).collect();
                assert_eq!(drawn, expected_keys(&app.composition.root, &state), "{name} @ {width}");
                assert!(host.invoked.is_empty(), "{name}: 描いただけで invoke した");
            }
        }
    }
}

/// UI-005 AC「app 名を hard-code しない」: renderer の source に 13 本の id も title も出てこない。
#[test]
fn renderer_source_names_no_fixture_app() {
    let dir = format!("{ROOT}/src/ui/renderer");
    let source: String = std::fs::read_dir(&dir).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect();
    for name in fixtures() {
        let app = load(&name);
        assert!(!source.contains(&app.id), "renderer が app id '{}' を名指ししている", app.id);
        let title = app.title.unwrap();
        assert!(!source.contains(&title), "renderer が app 名 '{title}' を名指ししている");
    }
}

fn synthetic(root: Value, actions: Value, sources: Value) -> AppDefinitionV3 {
    serde_json::from_value(json!({
        "schema_version": "3.0", "id": "test.synthetic", "version": 1, "title": "部品の一覧",
        "goal": {"literal_goal": "全部の部品を描く", "outcomes": ["描ける"]},
        "composition": {"root": root},
        "capabilities": [],
        "actions": actions,
        "sources": sources,
        "agent": {"suggestions_enabled": true, "autonomy_level": "suggest_only", "entrypoint": "rail"},
        "state": {"readiness": "ready", "execution_mode": "local_ready"}
    }))
    .unwrap()
}

/// W15-c1（r20 の AX: 本物の AI の Project の全 Page に `◐ Partly shown` だけの行）: Project の殻の app は readiness Preview・文も action も取得元も無い。
/// 記号と状態語だけの行は何が欠けているかも次の一手も言わないので描かない。文の在る Preview / Partial は今のまま（欠けている事と次の一手を言う）。
/// 負の対照: `render_with` の `said.is_empty() && Preview` の門を外す → 1 本目が赤。
#[test]
fn w15c1_a_shell_app_with_nothing_to_say_draws_no_partly_shown_line() {
    use domain::app::AppReadiness;
    let root = json!({"key": "root", "component": "column", "children": [{"key": "t", "component": "text", "label": "hello"}]});
    let drawn = |readiness: AppReadiness, said: Option<&str>| {
        let mut app = synthetic(root.clone(), json!([]), json!([]));
        app.state.readiness = readiness;
        app.state.status_message = said.map(Into::into);
        let out = Harness::new(1440.0).settle(&app, &mut AppState::default(), &mut Fake::default());
        ax(&out).iter().map(|a| a.text.clone()).collect::<Vec<_>>()
    };
    let shell = drawn(AppReadiness::Preview, None);
    assert!(!shell.iter().any(|t| t.contains("Partly shown")), "殻の app が記号だけの行を描いた: {shell:?}");
    assert!(shell.iter().any(|t| t == "hello"), "正の対照: 本文は描く: {shell:?}");
    let said = drawn(AppReadiness::Partial, Some("No folder chosen · Choose folder"));
    assert!(said.iter().any(|t| t.contains("Partly shown") && t.contains("No folder chosen · Choose folder")), "欠けている事と次の一手の行は残す: {said:?}");
    let migrated = drawn(AppReadiness::Preview, Some("Moved from v2 · not checked yet"));
    assert!(migrated.iter().any(|t| t.contains("Partly shown") && t.contains("Moved from v2")), "文の在る Preview は残す: {migrated:?}");
}

fn ready_action() -> Value {
    json!([{"id": "local.do", "effect": "local_write", "readiness": "ready", "input_schema": {"type": "object"}, "required_scopes": []}])
}

/// domain の BUILTIN_COMPONENTS 25 種（recorder / source_surface / waveform / W5 の terminal / AI-COMPOSE-1101 の canvas を含む。Project の外の terminal は 1 行）を 1 つずつ描く。表に無い component は「未対応の部品」。
#[test]
fn every_builtin_component_draws() {
    assert_eq!(BUILTIN_COMPONENTS.len(), 25);
    let mut children = Vec::new();
    let mut fake = Fake::default();
    for c in BUILTIN_COMPONENTS {
        let key = format!("n-{c}");
        let mut node = json!({"key": key, "component": c, "label": format!("部品 {c}")});
        if matches!(*c, "column" | "row" | "split" | "tabs") {
            node["children"] = json!([{"key": format!("{key}-child"), "component": "text", "label": "子"}]);
        }
        if matches!(*c, "button" | "form" | "command_input" | "chat" | "recorder" | "source_surface" | "media") {
            node["action_ref"] = json!("local.do");
        }
        let data = match *c {
            "list" => Some(json!([{"title": "行"}])),
            "table" => Some(json!([{"name": "行", "qty": 1}])),
            "detail" => Some(json!({"title": "詳細", "x": "y"})),
            "chat" => Some(json!([{"author": "A", "text": "やあ"}])),
            "calendar" => Some(json!([{"start": "2026-10-17T10:00", "title": "予定"}])),
            "timeline" => Some(json!([{"title": "段", "status": "verified"}])),
            "media" | "source_surface" => Some(json!({"title": "clip"})),
            "chart" => Some(json!([{"k": "a", "v": 1}])),
            "run_progress" => Some(json!({"status": "running", "steps": [{"title": "s", "status": "verified"}]})),
            "waveform" => Some(json!([0.1, -0.5, 0.9])),
            "editor" => Some(json!("本文")),
            "text" => Some(json!("12:00")),
            "input" => Some(json!("初期値")),
            _ => None,
        };
        if let Some(v) = data {
            node["binding"] = json!({"source_ref": "local.s", "path": c});
            fake.data.insert(("local.s".into(), c.to_string()), v);
        }
        if *c == "form" {
            node["props"] = json!({"fields": [{"name": "q", "label": "項目", "input": "text"}]});
        }
        children.push(node);
    }
    children.push(json!({"key": "n-unknown", "component": "foo"}));
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": children}),
        ready_action(),
        json!([{"id": "local.s", "kind": "local_state", "readiness": "local"}]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut fake);
    for c in BUILTIN_COMPONENTS {
        assert!(state.rects.contains_key(&format!("n-{c}")), "'{c}' を描いていない");
    }
    assert_eq!(state.rects.len(), BUILTIN_COMPONENTS.len() + 4 + 1 + 1, "子 4 + 未知 1 + root 1");
    assert!(has_text(&out, "This version can't show one part (foo)"));
    assert!(state.gates.is_empty(), "ready な action は全部押せる: {:?}", state.gates);
    assert!(nested_cards(&out).is_empty(), "{:?}", nested_cards(&out));
    assert!(fake.invoked.is_empty());
}

/// DESIGN §4.1 / CD-02: 押せない button は理由つき disabled、押せる button は host.invoke に ActionInvocation（origin = その node）を渡す。
#[test]
fn buttons_are_gated_with_reasons_and_ready_ones_invoke() {
    let h = Harness::new(1440.0);

    // preview_only（設計サンプル）: 押せない・押しても何も呼ばない
    let timer = load("app_v3_timer");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&timer, &mut state, &mut host);
    assert_eq!(state.gates["timer-start"].reason, "Not run in preview");
    let start = find(&out, "開始 / 一時停止");
    assert!(start.disabled && start.role == "Button", "{start:?}");
    h.click(&timer, &mut state, &mut host, start.rect.center());
    assert!(host.invoked.is_empty(), "disabled を押して invoke した");

    // 接続が必要: 定義の作者が書いた理由（states.disabled）をそのまま出す
    let coding = load("v3/03_coding_workspace");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&coding, &mut state, &mut host);
    let gate = &state.gates["coding-issues-refresh"];
    assert_eq!(gate.state, Some(RegionState::Disconnected));
    assert_eq!(gate.reason, "取得元を繋ぐまで読めない。繋いでも読み取りだけ");
    assert!(find(&out, "Issue を読み込む").disabled);
    // row の中の button なので、理由は row の下に「button 名: 理由」で出る
    assert!(has_text(&out, "Not connected · Issue を読み込む: 取得元を繋ぐまで読めない"), "理由が画面に出ていない");

    // 未 bind（action_ref 無し）・定義に無い action = 未実装の Action は描かない（G100 FX-060）。理由は inspector のために gates に残す
    let unbound = synthetic(json!({"key": "root", "component": "column", "children": [
        {"key": "b", "component": "button", "label": "押す"},
        {"key": "c", "component": "button", "label": "無い手", "action_ref": "local.nowhere"},
    ]}), json!([]), json!([]));
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&unbound, &mut state, &mut host);
    assert_eq!(state.gates["b"].reason, "No action is set for this button");
    assert_eq!(state.gates["c"].reason, "The action “local.nowhere” isn't defined");
    assert!(!has_text(&out, "押す") && !has_text(&out, "無い手"), "未実装の Action を描いた");

    // 押せる: 01 の timer-toggle
    let timer = load("v3/01_timer_local");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&timer, &mut state, &mut host);
    let toggle = find(&out, "開始 / 一時停止");
    assert!(!toggle.disabled);
    h.click(&timer, &mut state, &mut host, toggle.rect.center());
    assert_eq!(
        host.invoked,
        vec![ActionInvocation { action_ref: "local.timer.toggle".into(), origin: InvocationOrigin::Button { node_key: "timer-toggle".into() }, input: json!({}) }]
    );

    // runtime が「接続した」と言えば押せる（定義は preview_only）
    let messages = load("app_v3_messages");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    h.settle(&messages, &mut state, &mut host);
    assert!(state.gates.contains_key("connect-button"));
    host.ready.push("local.connection.open".into());
    h.settle(&messages, &mut state, &mut host);
    assert!(!state.gates.contains_key("connect-button"), "{:?}", state.gates);

    // 実行中の node は押せない（二重に送らない）
    let (mut state, mut host) = (AppState::default(), Fake::default());
    host.regions.insert("timer-toggle".into(), (RegionState::Running, "残り 3 分".into()));
    h.settle(&timer, &mut state, &mut host);
    assert_eq!(state.gates["timer-toggle"], renderer::bindings::Gate { state: Some(RegionState::Running), reason: "残り 3 分".into() });

    // UI-6a（60.1）: 送ったか分からない node は語が Stale でも押せない（locked）。Stale だけ（disk と違う窓など）は押せない理由にしない
    // 負の対照（commit message）: gate の `host.locked(..)` を外す → 1 本目が赤
    let (mut state, mut host) = (AppState::default(), Fake { locked: vec!["timer-toggle".into()], ..Fake::default() });
    host.regions.insert("timer-toggle".into(), (RegionState::Stale, "Can't tell if it arrived · Open Runs".into()));
    h.settle(&timer, &mut state, &mut host);
    assert_eq!(state.gates.get("timer-toggle"), Some(&renderer::bindings::Gate { state: Some(RegionState::Stale), reason: "Can't tell if it arrived · Open Runs".into() }), "locked を押せる");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    host.regions.insert("timer-toggle".into(), (RegionState::Stale, "May be out of date".into()));
    h.settle(&timer, &mut state, &mut host);
    assert!(state.gates.get("timer-toggle").is_none_or(|g| g.state != Some(RegionState::Stale)), "locked でない Stale まで止めた: {:?}", state.gates.get("timer-toggle"));
}

/// 描いた文字の glyph の色（text が完全一致する TextShape の頂点色）。
fn glyph_colors(out: &egui::FullOutput, text: &str) -> Vec<egui::Color32> {
    fn walk(s: &epaint::Shape, text: &str, out: &mut Vec<egui::Color32>) {
        match s {
            epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, out)),
            epaint::Shape::Text(t) if t.galley.text() == text => out.extend(t.galley.rows.iter().flat_map(|r| r.visuals.mesh.vertices.iter().map(|v| v.color))),
            _ => {}
        }
    }
    let mut v = Vec::new();
    for c in &out.shapes {
        walk(&c.shape, text, &mut v);
    }
    v
}

/// DESIGN §12.1「読めない disabled は不可」: 押せない button の文字は token の text.tertiary のまま描かれる（白へ寄せる fade を通さない）。
/// 実測（2026-10-17 の写真）: egui の `add_enabled_ui(false)` に theme の button を入れると文字がほぼ白になり、白い button の上で消えた。
#[test]
fn disabled_button_labels_stay_readable() {
    let h = Harness::new(1440.0);
    let app = load("v3/03_coding_workspace");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    for label in ["Issue を読み込む", "test を実行"] {
        assert!(find(&out, label).disabled, "{label}");
        let colors = glyph_colors(&out, label);
        assert!(!colors.is_empty(), "{label} を描いていない");
        assert!(colors.iter().all(|c| *c == ui::theme::LIGHT.text_tertiary), "{label}: {:?}", colors.first());
    }
}

/// UI-006「button と label は使える形で残る」: 13 本 × 960 / 1440 × data 有無で、押せる物（button・入力欄）が主領域の右端の外へ出ない。
/// 実測（2026-10-17 の写真）: 08 の 5 つの button の row で、押せない button の理由を横に並べた分だけ後ろの button が窓の外へ押し出された。
#[test]
fn nothing_actionable_is_pushed_past_the_right_edge() {
    for (width, gutter) in [(960.0, 20.0), (1440.0, 180.0)] {
        let h = Harness::new(width);
        let edge = width - gutter + 0.5;
        for name in fixtures() {
            let app = load(&name);
            for mut host in [Fake::default(), rich(&app)] {
                let mut state = AppState::default();
                let out = h.settle(&app, &mut state, &mut host);
                let over: Vec<String> = ax(&out)
                    .into_iter()
                    .filter(|a| matches!(a.role.as_str(), "Button" | "TextInput" | "MultilineTextInput" | "SpinButton") && a.rect.right() > edge)
                    .map(|a| format!("{} '{}' right {}", a.role, a.text, a.rect.right()))
                    .collect();
                assert!(over.is_empty(), "{name} @ {width}: 右端 {edge} の外 {over:?}");
            }
        }
        // corpus の button の row は 960 でも 1 行に入る（折り返しの道に届かない）ので、合成の row（長い名前の button 8 つ）で通す
        let buttons: Vec<Value> = (0..8).map(|i| json!({"key": format!("b{i}"), "component": "button", "label": format!("とても長い名前の操作ボタン {i}"), "action_ref": "local.do"})).collect();
        let app = synthetic(json!({"key": "root", "component": "column", "children": [{"key": "r", "component": "row", "children": buttons}]}), ready_action(), json!([]));
        let (mut state, mut host) = (AppState::default(), Fake::default());
        let out = h.settle(&app, &mut state, &mut host);
        let placed: Vec<Ax> = ax(&out).into_iter().filter(|a| a.role == "Button" && a.text.starts_with("とても長い")).collect();
        assert_eq!(placed.len(), 8);
        assert!(placed.iter().all(|a| a.rect.right() <= edge), "@ {width}: {:?}", placed.iter().map(|a| a.rect.right()).collect::<Vec<_>>());
        let rows: BTreeSet<i32> = placed.iter().map(|a| a.rect.top().round() as i32).collect();
        assert!(rows.len() >= 2, "@ {width}: 8 つが 1 行に詰められた");
    }
}

/// 1 行の入力（command_input）は 1 行の高さで終わり、後ろの部品を押し出さない。
/// 実測（2026-10-17 の写真）: right_to_left(Center) の layout が残りの高さを全部取り、09 の予定・メール以下が窓の外へ出た。
#[test]
fn command_input_takes_one_row() {
    let h = Harness::new(1440.0);
    let app = load("v3/09_owner_test2_calendar_mail_chat");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    h.settle(&app, &mut state, &mut host);
    let cmd = state.rects["cm-command"];
    assert!(cmd.height() < 100.0, "見出し + 38 の 1 行のはず: {cmd:?}");
    assert!(state.rects["cm-split"].top() - cmd.bottom() < 40.0, "次の部品が離れている: {cmd:?} / {:?}", state.rects["cm-split"]);
}

/// CD-11 (4): `visibility.when = has_selection` の node は行を選ぶまで出ない。選ぶと出て、選び直すと消える。
#[test]
fn conditional_node_follows_row_selection() {
    let h = Harness::new(1440.0);
    let app = load("v3/02_inventory");
    let mut host = Fake::default().with("local.inventory", "items", json!([{"name": "りんご", "quantity": 3, "updated_at": "2026-10-17"}, {"name": "みかん", "quantity": 1}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!state.rects.contains_key("inventory-detail"));
    let row = find(&out, "りんご");
    h.click(&app, &mut state, &mut host, row.rect.center());
    assert_eq!(state.rows.get("inventory-table").map(|p| &p.record), Some(&json!({"name": "りんご", "quantity": 3, "updated_at": "2026-10-17"})), "選んだのは行番号でなく record");
    for key in ["inventory-detail", "inventory-item-form", "inventory-delete"] {
        assert!(state.rects.contains_key(key), "選んだのに '{key}' が出ない");
    }
    let out = h.settle(&app, &mut state, &mut host);
    let row = find(&out, "りんご");
    h.click(&app, &mut state, &mut host, row.rect.center());
    assert!(state.rows.is_empty() && !state.rects.contains_key("inventory-detail"), "選び直したら消える");
    assert!(host.invoked.is_empty(), "行を選ぶのは action ではない");
}

/// DESIGN §3: card の中に card を置かない。13 本（data 無し / 有り）と全部品の画面で、fill + 枠の面が入れ子にならない。
#[test]
fn no_card_inside_a_card() {
    let h = Harness::new(1440.0);
    for name in fixtures() {
        let app = load(&name);
        for mut host in [Fake::default(), rich(&app)] {
            let mut state = AppState::default();
            let out = h.settle(&app, &mut state, &mut host);
            assert!(nested_cards(&out).is_empty(), "{name}: card の入れ子 {:?}", nested_cards(&out));
        }
    }
}

/// UI-007: region ごとに 10 の状態（DESIGN §11 の文言・states.rs）。中身は描いてよい状態の時だけ描く。
#[test]
fn every_region_state_has_wording_and_content_only_when_allowed() {
    let h = Harness::new(1440.0);
    let app = load("v3/03_coding_workspace");
    // 定義だけで決まる物: 未接続の取得元 → 接続が必要 + 作者の文、端末内で空 → まだデータがありません + 作者の文
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(state.regions["coding-issues"], (RegionState::Disconnected, "Issue の取得元を Connections で選ぶと表示する。候補: GitHub（未接続）".into()));
    assert_eq!(state.regions["coding-tasks"], (RegionState::Empty, "まだやる事がありません。「追加する」で 1 つ目を書く".into()));
    assert!(has_text(&out, RegionState::Disconnected.wording()) && has_text(&out, RegionState::Empty.wording()));
    // runtime が知っている状態: 10 語を 1 つずつ。行が在っても、読み込み中・失敗・確認待ち 等の間は行を描かない
    for s in RegionState::ALL {
        let mut host = Fake::default().with("local.tasks", "items", json!([{"title": "一件目"}]));
        host.regions.insert("coding-tasks".into(), (s, "補足".into()));
        let mut state = AppState::default();
        let out = h.settle(&app, &mut state, &mut host);
        assert_eq!(state.regions["coding-tasks"].0, s);
        if s != RegionState::Ready {
            assert!(has_text(&out, s.wording()), "{s:?} の文言が出ていない");
        }
        // 期待は spec から直に書く（renderer の shows_content を oracle にしない: 同じ関数同士だと壊しても一致する）
        let shows = matches!(s, RegionState::Ready | RegionState::Partial | RegionState::Stale | RegionState::Running | RegionState::Verified);
        assert_eq!(has_text(&out, "一件目"), shows, "{s:?}: 中身の出し方");
    }
    // 取得元の readiness → 状態語（接続済み・確認中 / 一部使えない / 対応外 / 定義に無い）
    let root = json!({"key": "root", "component": "column", "children": [
        {"key": "a", "component": "list", "binding": {"source_ref": "s.unverified", "path": "x"}},
        {"key": "b", "component": "list", "binding": {"source_ref": "s.degraded", "path": "x"}},
        {"key": "c", "component": "list", "binding": {"source_ref": "s.unsupported", "path": "x"}},
        {"key": "d", "component": "list", "binding": {"source_ref": "s.missing", "path": "x"}}
    ]});
    let sources = json!([
        {"id": "s.unverified", "kind": "external_api", "readiness": "connected_unverified"},
        {"id": "s.degraded", "kind": "external_api", "readiness": "degraded"},
        {"id": "s.unsupported", "kind": "external_api", "readiness": "unsupported"}
    ]);
    let app = synthetic(root, json!([]), sources);
    let (mut state, mut host) = (AppState::default(), Fake::default());
    h.settle(&app, &mut state, &mut host);
    let got: Vec<RegionState> = ["a", "b", "c", "d"].iter().map(|k| state.regions[*k].0).collect();
    assert_eq!(got, vec![RegionState::Loading, RegionState::Stale, RegionState::Error, RegionState::Error]);
}

/// UI-009 の最小: node の余白を click → 1px の outline と inspector（name / role / binding / capability / 状態 / 権限 / source）。
/// disabled の button も選べる（なぜ押せないかを見る）。押せる button は右 click で選ぶ。Esc で外れる。
#[test]
fn clicking_a_node_outlines_it_and_inspector_lists_its_facts() {
    let h = Harness::new(1440.0);
    let app = load("v3/03_coding_workspace");
    // G87: 利用者の画面（`inspect` が false・env 無し）は node を選ぶ click 面も inspector も作らない = 押しても選ばない
    let (mut user, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut user, &mut host);
    let out = h.click(&app, &mut user, &mut host, find(&out, "やる事").rect.center());
    assert!(user.selected.is_none() && !has_text(&out, "Selected part"), "利用者の画面で node の選択と inspector が出た");
    let (mut state, mut host) = (AppState { inspect: true, ..AppState::default() }, Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let heading = find(&out, "やる事");
    let out = h.click(&app, &mut state, &mut host, heading.rect.center());
    assert_eq!(state.selected.as_deref(), Some("coding-tasks"));
    let rows: HashMap<&str, String> = inspector_rows(&app, &state, "coding-tasks").into_iter().collect();
    assert_eq!(rows.len(), 7);
    assert!(rows["name"].contains("やる事") && rows["name"].contains("list"), "{rows:?}");
    assert!(rows["binding"].starts_with("local.tasks · items"), "{rows:?}");
    assert!(rows["source"].starts_with("local.tasks · local_collection · local"), "{rows:?}");
    assert!(rows["State"].contains(RegionState::Empty.wording()), "{rows:?}");
    assert!(has_text(&out, "Selected part"), "inspector が出ていない");
    let want = state.rects["coding-tasks"].expand(4.0);
    let outlined = out.shapes.iter().any(|c| matches!(&c.shape, epaint::Shape::Rect(r) if r.fill.a() == 0 && r.stroke.color == ui::theme::LIGHT.text_primary && (r.rect.min - want.min).length() < 1.0));
    assert!(outlined, "1px の outline が無い");

    // disabled の button: 押せない理由・capability・権限
    let refresh = find(&out, "Issue を読み込む");
    h.click(&app, &mut state, &mut host, refresh.rect.center());
    assert_eq!(state.selected.as_deref(), Some("coding-issues-refresh"));
    let rows: HashMap<&str, String> = inspector_rows(&app, &state, "coding-issues-refresh").into_iter().collect();
    assert!(rows["capability"].contains("issues.read · requires_connection"), "{rows:?}");
    assert!(rows["Permissions"].starts_with("external_read"), "{rows:?}");
    assert!(rows["State"].contains("Can't press: Not connected · 取得元を繋ぐまで読めない"), "{rows:?}");
    assert!(host.invoked.is_empty());

    // 押せる button は右 click で選ぶ（左 click は実行）。「追加する」は必須の title を決める欄が無く押せないので、入力の要らない「取得元を選ぶ」で
    let out = h.settle(&app, &mut state, &mut host);
    let connect = find(&out, "取得元を選ぶ");
    assert!(!connect.disabled);
    h.click_with(&app, &mut state, &mut host, connect.rect.center(), true);
    assert_eq!(state.selected.as_deref(), Some("coding-issues-connect"));
    assert!(host.invoked.is_empty(), "右 click で実行した");

    // Esc で外れる
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::default() }]);
    assert_eq!(state.selected, None);
}

/// DESIGN §1.2: app を切り替えても入力の draft が残る（AppState は app ごと・入力欄の Id は app id + node key）。
#[test]
fn drafts_survive_switching_apps() {
    let h = Harness::new(1440.0);
    let inventory = load("v3/02_inventory");
    let research = load("v3/04_research_notes");
    let (mut a, mut b, mut host) = (AppState::default(), AppState::default(), Fake::default());
    h.settle(&inventory, &mut a, &mut host);
    let field = egui::Id::new(("kangae-field", &inventory.id, "inventory-search", "query"));
    h.ctx.memory_mut(|m| m.request_focus(field));
    h.frame(&inventory, &mut a, &mut host, vec![]);
    h.frame(&inventory, &mut a, &mut host, vec![egui::Event::Text("りん".into())]);
    assert_eq!(a.draft("inventory-search", None).and_then(|d| d.get("query")), Some(&json!("りん")));
    // 別の app を描く（同じ field 名 query を持つ: Id が app ごとに違うので混ざらない）
    h.settle(&research, &mut b, &mut host);
    h.frame(&research, &mut b, &mut host, vec![egui::Event::Text("x".into())]);
    assert!(b.draft("research-search", None).is_none_or(|d| d.get("query").is_none()), "前の app の焦点で別の app に打ち込んだ");
    // 戻る
    let out = h.settle(&inventory, &mut a, &mut host);
    assert_eq!(a.draft("inventory-search", None).and_then(|d| d.get("query")), Some(&json!("りん")));
    assert!(ax(&out).iter().any(|e| e.role == "TextInput" && e.text == "りん"), "戻った画面に draft が出ていない");
    // form を送ると draft が input になる（data の上に draft を重ねた object）
    let out = h.settle(&inventory, &mut a, &mut host);
    let submit = find(&out, "Run");
    h.click(&inventory, &mut a, &mut host, submit.rect.center());
    assert_eq!(host.invoked.last().map(|i| (i.action_ref.as_str(), i.input.clone())), Some(("local.inventory.filter", json!({"query": "りん"}))));
}

/// UI-006: 表が幅に入らない時は横 scroll を 1 行で明示する。入る時は出さない。button は縮めない。
#[test]
fn table_overflow_is_explicit_and_buttons_keep_their_size() {
    let h = Harness::new(1440.0);
    let wide: Vec<String> = (0..10).map(|i| format!("列{i}")).collect();
    for (columns, overflow) in [(wide.clone(), true), (wide[..3].to_vec(), false)] {
        let row: serde_json::Map<String, Value> = columns.iter().map(|c| (c.clone(), json!("値"))).collect();
        let root = json!({"key": "root", "component": "column", "children": [
            {"key": "t", "component": "table", "label": "表", "props": {"columns": columns}, "binding": {"source_ref": "local.s", "path": "rows"}},
            {"key": "r", "component": "row", "children": [
                {"key": "b1", "component": "button", "label": "とても長い名前の操作ボタンその一", "action_ref": "local.do"},
                {"key": "b2", "component": "button", "label": "とても長い名前の操作ボタンその二", "action_ref": "local.do"}
            ]}
        ]});
        let app = synthetic(root, ready_action(), json!([{"id": "local.s", "kind": "local_collection", "readiness": "local"}]));
        let mut host = Fake::default().with("local.s", "rows", json!([row]));
        let mut state = AppState::default();
        let out = h.settle(&app, &mut state, &mut host);
        assert_eq!(has_text(&out, "Scroll sideways to see all"), overflow, "{} 列", columns.len());
        for name in ["とても長い名前の操作ボタンその一", "とても長い名前の操作ボタンその二"] {
            let b = find(&out, name);
            assert!(b.rect.height() >= ui::theme::BUTTON_H && b.rect.width() > 200.0, "{name}: {:?}", b.rect);
        }
    }
}

/// UI-006: 横に並べると min が入らない split は縦に畳み、主役（role primary）を先に置く。入る split は横に並べる。
#[test]
fn split_collapses_primary_first_when_mins_do_not_fit() {
    let h = Harness::new(1440.0);
    let columns: Vec<String> = (0..8).map(|i| format!("c{i}")).collect();
    let root = json!({"key": "root", "component": "column", "children": [
        {"key": "tight", "component": "split", "layout": {"ratio": [0.5, 0.5]}, "children": [
            {"key": "side", "component": "list", "role": "context", "label": "横"},
            {"key": "main", "component": "table", "role": "primary", "label": "主", "props": {"columns": columns}}
        ]},
        {"key": "roomy", "component": "split", "children": [
            {"key": "left", "component": "list", "label": "左"},
            {"key": "right", "component": "list", "label": "右"}
        ]}
    ]});
    let app = synthetic(root, json!([]), json!([]));
    let (mut state, mut host) = (AppState::default(), Fake::default());
    h.settle(&app, &mut state, &mut host);
    let r = |k: &str| state.rects[k];
    assert!(r("main").bottom() <= r("side").top() + 0.5, "畳んだら主役が上: main {:?} side {:?}", r("main"), r("side"));
    assert!((r("left").top() - r("right").top()).abs() < 0.5 && r("left").right() < r("right").left(), "入る split は横並び");
}

/// DESIGN §4.4: source_surface は埋め込まず、外の window を開く button（handoff）と説明 1 行。黒い空の面も card も出さない。
#[test]
fn source_surface_is_a_handoff_not_an_embed() {
    let h = Harness::new(1440.0);
    let app = load("v3/10_owner_test3_music_workspace");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    state.tabs.insert("music-sources".into(), 1);
    let out = h.settle(&app, &mut state, &mut host);
    assert!(state.rects.contains_key("yt-player"));
    assert!(has_text(&out, "Opens in the original app's window instead of embedding"));
    let surface = state.rects["yt-player"];
    assert!(cards(&out).iter().all(|c| !surface.contains_rect(*c)), "source_surface の中に面を描いた");
    // youtube.player.open は必須の video_id を決める欄が画面に無い: 押せない（{} を送って runtime に落とされる形を出さない・R1-01）
    let open = ax(&out).into_iter().find(|a| a.text == "Open window" && surface.contains_rect(a.rect)).expect("Open window が無い");
    assert!(open.disabled);
    h.click(&app, &mut state, &mut host, open.rect.center());
    assert!(host.invoked.is_empty());

    // 入力の要らない handoff（09 の claude.ai）は押せて、action を invoke する
    let chat = load("v3/09_owner_test2_calendar_mail_chat");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&chat, &mut state, &mut host);
    let surface = state.rects["cm-claude"];
    assert!(cards(&out).iter().all(|c| !surface.contains_rect(*c)), "source_surface の中に面を描いた");
    let open = ax(&out).into_iter().find(|a| a.text == "Open window" && surface.contains_rect(a.rect)).expect("Open window が無い");
    assert!(!open.disabled, "{:?}", state.gates);
    h.click(&chat, &mut state, &mut host, open.rect.center());
    assert_eq!(host.invoked.iter().map(|i| i.action_ref.as_str()).collect::<Vec<_>>(), vec!["claude_ai.open"]);

    // 編集 app が未接続の handoff は押せない（作者の理由つき）
    let editor = load("v3/05_creative_editor");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    h.settle(&editor, &mut state, &mut host);
    assert_eq!(state.gates["edit-surface"].state, Some(RegionState::Disconnected));
}

// ---------------------------------------------------------------------------
// round 1 の確定した指摘（R1-01 / R1-03 / R1-04 / R1-05 / R-SEC-1）の再現 test
// ---------------------------------------------------------------------------

/// R1-01: 押せる button は、runtime の `validate_invocation`（domain）が通す input を送る（{} を送って毎回落とされる button を残さない・DESIGN §4.1）。
/// 13 本 × data 有りで、押せる AX の Button を 1 つずつ（毎回まっさらな状態から）押し、出た ActionInvocation を全部 validate に通す。
#[test]
fn every_enabled_button_sends_an_input_its_action_accepts() {
    let h = Harness::new(1440.0);
    let mut pressed = 0;
    for name in fixtures() {
        let app = load(&name);
        let targets: Vec<(String, Pos2)> = {
            let (mut state, mut host) = (AppState::default(), rich(&app));
            ax(&h.settle(&app, &mut state, &mut host)).into_iter().filter(|a| a.role == "Button" && !a.disabled).map(|a| (a.text, a.rect.center())).collect()
        };
        for (text, at) in targets {
            let (mut state, mut host) = (AppState::default(), rich(&app));
            let out = h.settle(&app, &mut state, &mut host);
            let Some(a) = ax(&out).into_iter().find(|a| a.text == text && (a.rect.center() - at).length() < 1.0) else { continue };
            h.click(&app, &mut state, &mut host, a.rect.center());
            for inv in &host.invoked {
                pressed += 1;
                let verdict = domain::action::validate_invocation(&app, inv);
                assert!(verdict.is_ok(), "{name}: 「{text}」が送った input {} を runtime が拒む: {:?}", inv.input, verdict.err());
            }
        }
    }
    assert!(pressed >= 5, "押して invoke まで届いた button が {pressed} 個しかない（測っていない）");
}

/// R1-01: 必須の入力を決める欄が画面に無い button は押せない（理由に足りない入力の名前）。
/// v3 の UiNode には「どの node の値を input にするか」を書く field が無いので、editor の文や選んだ行を推して詰めない。
#[test]
fn a_button_whose_required_input_has_no_field_is_disabled_with_the_field_named() {
    let h = Harness::new(1440.0);
    for (name, key, field, tab) in [
        ("v3/03_coding_workspace", "coding-notes-save", "body", None),
        ("v3/04_research_notes", "research-save", "note_id", None),
        ("v3/09_owner_test2_calendar_mail_chat", "cm-approve", "proposal_id", None),
        ("v3/10_owner_test3_music_workspace", "yt-player", "video_id", Some(("music-sources", 1))),
    ] {
        let app = load(name);
        let (mut state, mut host) = (AppState::default(), rich(&app));
        if let Some((tabs, i)) = tab {
            state.tabs.insert(tabs.into(), i);
        }
        h.settle(&app, &mut state, &mut host);
        let gate = state.gates.get(key).unwrap_or_else(|| panic!("{name}: '{key}' が押せる（必須の {field} を決める欄が無いのに）"));
        assert!(gate.reason.contains(field), "{name}: {gate:?}");
    }
}

/// R-SEC-1: form が送るのは props.fields に宣言した欄の値だけ。bind 先 data の、画面に欄の無い key（to / bcc）は送らない。
#[test]
fn form_sends_only_its_declared_fields_not_hidden_keys_of_the_bound_data() {
    let root = json!({"key": "root", "component": "column", "children": [{
        "key": "reply", "component": "form", "label": "返信", "action_ref": "mail.send",
        "props": {"fields": [{"name": "subject", "label": "件名", "input": "text"}, {"name": "body", "label": "本文", "input": "multiline"}]},
        "binding": {"source_ref": "inbox", "path": "selected"}
    }]});
    let actions = json!([{"id": "mail.send", "effect": "external_send", "readiness": "ready", "required_scopes": [],
        "input_schema": {"type": "object", "required": ["subject", "body"],
            "properties": {"to": {"type": "string"}, "bcc": {"type": "string"}, "subject": {"type": "string"}, "body": {"type": "string"}}}}]);
    let app = synthetic(root, actions, json!([{"id": "inbox", "kind": "external_api", "readiness": "ready"}]));
    let mut host = Fake::default().with("inbox", "selected", json!({"to": "attacker@example.com", "bcc": "x@example.com", "subject": "件名", "body": "本文"}));
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let send = find(&out, "Send");
    assert!(!send.disabled, "{:?}", state.gates);
    h.click(&app, &mut state, &mut host, send.rect.center());
    let sent: Vec<Value> = host.invoked.iter().map(|i| i.input.clone()).collect();
    assert_eq!(sent, vec![json!({"subject": "件名", "body": "本文"})]);
}

/// R1-05: 数・on/off の欄は、data も入力も無い間 0 / off を出さない（送らない値を見せない）。打った値がそのまま送られる。
#[test]
fn number_and_toggle_fields_show_no_value_until_one_is_entered() {
    let h = Harness::new(1440.0);
    let app = load("v3/01_timer_local");
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!ax(&out).iter().any(|a| a.text == "0"), "data も入力も無いのに 0 を出している");
    let gate = state.gates.get("timer-settings").expect("時間が無いのに 保存 が押せる");
    assert!(gate.reason.contains("時間（分）"), "{gate:?}");
    let field = egui::Id::new(("kangae-field", &app.id, "timer-settings", "duration_minutes"));
    h.ctx.memory_mut(|m| m.request_focus(field));
    h.frame(&app, &mut state, &mut host, vec![]);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("25".into())]);
    assert_eq!(state.draft("timer-settings", None).and_then(|d| d.get("duration_minutes")), Some(&json!(25)));
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!state.gates.contains_key("timer-settings"), "{:?}", state.gates);
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    let inv = host.invoked.last().expect("保存 が invoke しない");
    assert_eq!(inv.input, json!({"duration_minutes": 25}));
    assert!(domain::action::validate_invocation(&app, inv).is_ok());

    // on / off: 値が無い間は off と言わない（AX の toggled は Mixed = どちらでもない）。押すと on が draft に入る
    let root = json!({"key": "root", "component": "column", "children": [
        {"key": "f", "component": "form", "action_ref": "local.do", "props": {"fields": [{"name": "notify", "label": "通知", "input": "toggle"}]}}
    ]});
    let app = synthetic(root, ready_action(), json!([]));
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let check = ax(&out).into_iter().find(|a| a.role == "CheckBox").expect("checkbox が無い");
    assert_eq!(check.toggled.as_deref(), Some("Mixed"), "値が無いのに on / off のどちらかを言っている");
    h.click(&app, &mut state, &mut host, check.rect.center());
    assert_eq!(state.draft("f", None).and_then(|d| d.get("notify")), Some(&json!(true)));
}

/// R1-03: 行の選択は record に付く（先頭に行が増えても別の record に移らない）。一覧が空になれば外れ、has_selection の node も消える。
#[test]
fn row_selection_follows_the_record_and_clears_when_the_list_empties() {
    let h = Harness::new(1440.0);
    let app = load("v3/02_inventory");
    let apple = json!({"name": "りんご", "quantity": 3, "updated_at": "2026-10-17"});
    let orange = json!({"name": "みかん", "quantity": 1});
    let mut host = Fake::default().with("local.inventory", "items", json!([apple, orange]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let out = h.click(&app, &mut state, &mut host, find(&out, "りんご").rect.center());
    assert_eq!(find(&out, "りんご").toggled.as_deref(), Some("True"), "押しても選ばれていない");
    // runtime の更新で先頭に行が入る
    host.data.insert(("local.inventory".into(), "items".into()), json!([{"name": "ぶどう", "quantity": 9}, apple, orange]));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(find(&out, "りんご").toggled.as_deref(), Some("True"), "選んだ record から選択が外れた");
    assert_eq!(find(&out, "ぶどう").toggled.as_deref(), Some("False"), "先頭に入った別の record に選択が移った");
    assert!(state.rects.contains_key("inventory-detail"));
    // 一覧が空になる
    host.data.insert(("local.inventory".into(), "items".into()), json!([]));
    h.settle(&app, &mut state, &mut host);
    assert!(state.rows.is_empty(), "空の一覧に選択が残っている");
    assert!(!state.rects.contains_key("inventory-detail"), "何も選べないのに has_selection の node が出ている");
}

/// R1-04: runtime が Ready（idle の run 等）と言っても data が無ければ空白にしない: 「まだデータがありません」+ 作者の文。
#[test]
fn a_ready_region_without_data_says_so_instead_of_staying_blank() {
    let h = Harness::new(1440.0);
    let app = load("v3/03_coding_workspace");
    let mut host = Fake::default();
    host.regions.insert("coding-tasks".into(), (RegionState::Ready, String::new()));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(state.regions["coding-tasks"].0, RegionState::Empty);
    assert!(has_text(&out, "まだやる事がありません"), "作者の空の文が出ていない");
    // binding の無い run_progress に runtime が idle の run（Ready）を渡した時も
    let root = json!({"key": "root", "component": "column", "children": [{"key": "runs", "component": "run_progress", "label": "実行"}]});
    let app = synthetic(root, json!([]), json!([]));
    let mut host = Fake::default();
    host.regions.insert("runs".into(), (RegionState::Ready, String::new()));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(state.regions["runs"].0, RegionState::Empty);
    assert!(has_text(&out, RegionState::Empty.wording()));
    // 取り消した run（run_state(Cancelled) = Ready + 「Undone」）: runtime の言葉は消さない
    let (cancelled, detail) = renderer::bindings::run_state(domain::run::RunStatus::Cancelled);
    host.regions.insert("runs".into(), (cancelled, detail.to_owned()));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(state.regions["runs"], (RegionState::Empty, "Undone".to_owned()));
    assert!(has_text(&out, "Undone"), "runtime の補足が消えた");
}

// ---------------------------------------------------------------------------
// round 2 の確定した指摘（R2-01 / R2-02）の再現 test。今ある API（AX・invoke・rects・rows.is_empty）だけで見る
// ---------------------------------------------------------------------------

/// 主領域の中で、role が一致する最初の入力欄（`within` の node の rect の中だけ）。
fn input_in(out: &egui::FullOutput, state: &AppState, within: &str, role: &str) -> Ax {
    let area = state.rects[within];
    ax(out).into_iter().find(|e| e.role == role && area.contains_rect(e.rect)).unwrap_or_else(|| panic!("'{within}' の中に {role} が無い"))
}

/// R2-01: form の draft は打った相手の record に付く。bind 先が別の record に替わったら、その record の値を出して送り、前の record で打った文を乗せない。
/// 前の record に戻れば打ちかけの文が戻る。送った draft は、bind 先が送った時の data から変われば消える（反映された後に古い文で record を覆わない）。
#[test]
fn form_draft_follows_the_bound_record_and_never_lands_on_another() {
    // 09 の「返信の下書き」の 保存 は 900 の窓では y 901 に来るので、窓を縦に伸ばして押す
    let h = Harness { height: 1400.0, ..Harness::new(1440.0) };
    let app = load("v3/09_owner_test2_calendar_mail_chat");
    let a = json!({"id": "draft-a", "to": "a@example.com", "subject": "A の件", "body": "A の元の本文"});
    let b = json!({"id": "draft-b", "to": "b@example.com", "subject": "B の件", "body": "B の元の本文"});
    let mut host = Fake::default().with("local.drafts", "selected", a.clone()).with("local.proposals", "pending", json!([{"id": "p1", "title": "提案"}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    // cm-reply は行を選んでいる間だけ出る
    let out = h.click(&app, &mut state, &mut host, find(&out, "提案").rect.center());
    let body = |out: &egui::FullOutput, state: &AppState| input_in(out, state, "cm-reply", "MultilineTextInput");
    assert_eq!(body(&out, &state).text, "A の元の本文");
    // A の本文に打つ
    let at = body(&out, &state).rect.center();
    h.pause(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("（A 宛の返事）".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    let typed = body(&out, &state).text;
    assert!(typed.contains("A 宛の返事"), "{typed}");
    // runtime の選択が B に替わる（runtime の選択・Agent・別の行）
    host.data.insert(("local.drafts".into(), "selected".into()), b.clone());
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(body(&out, &state).text, "B の元の本文", "A で打った文が B の本文の欄に出ている");
    assert!(has_text(&out, "b@example.com") && has_text(&out, "B の件"));
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    assert_eq!(
        host.invoked.last().map(|i| i.input.clone()),
        Some(json!({"to": "b@example.com", "subject": "B の件", "body": "B の元の本文"})),
        "A で打った文を B に保存した"
    );
    // A に戻ると打ちかけの文が戻り、送るのは A の文
    host.data.insert(("local.drafts".into(), "selected".into()), a.clone());
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(body(&out, &state).text, typed, "A に戻ったのに打ちかけの文が消えた");
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    assert_eq!(host.invoked.last().map(|i| i.input.clone()), Some(json!({"to": "a@example.com", "subject": "A の件", "body": typed})));
    // 反映される前（data がまだ送った時のまま）は打った文を出し続ける（保存が通らなくても文を失わない）
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(body(&out, &state).text, typed);
    // runtime が反映した後に、別の手（Agent 等）が本文を直す: 送り済みの古い draft で覆わない
    let mut saved = a.clone();
    saved["body"] = json!(typed);
    host.data.insert(("local.drafts".into(), "selected".into()), saved.clone());
    h.settle(&app, &mut state, &mut host);
    let mut fixed = saved;
    fixed["body"] = json!("Agent が直した本文");
    host.data.insert(("local.drafts".into(), "selected".into()), fixed);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(body(&out, &state).text, "Agent が直した本文", "送って反映された後も、古い draft が record の値を覆っている");
}

/// R2-01: 送った後に runtime が確認待ち（承認）や読み込み中で bind 先を隠しても、送った draft は消さない
/// （data が見えない間は「送った時から変わった」と言えない）。承認を断られた・保存が通らなかった（data が送った時のまま戻る）時に、打った文が残っている。
#[test]
fn a_sent_draft_survives_while_the_bound_record_is_hidden() {
    let h = Harness { height: 1400.0, ..Harness::new(1440.0) };
    let app = load("v3/09_owner_test2_calendar_mail_chat");
    let a = json!({"id": "draft-a", "to": "a@example.com", "subject": "A の件", "body": "A の元の本文"});
    let mut host = Fake::default().with("local.drafts", "selected", a).with("local.proposals", "pending", json!([{"id": "p1", "title": "提案"}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let out = h.click(&app, &mut state, &mut host, find(&out, "提案").rect.center());
    let body = |out: &egui::FullOutput, state: &AppState| input_in(out, state, "cm-reply", "MultilineTextInput");
    let at = body(&out, &state).rect.center();
    h.pause(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("（返事）".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    let typed = body(&out, &state).text;
    assert!(typed.contains("返事"), "{typed}");
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    assert_eq!(host.invoked.last().map(|i| i.input["body"].clone()), Some(json!(typed)));
    for hidden in [RegionState::Blocked, RegionState::Loading] {
        host.regions.insert("cm-reply".into(), (hidden, "確認しています".into()));
        h.settle(&app, &mut state, &mut host);
        host.regions.remove("cm-reply");
        let out = h.settle(&app, &mut state, &mut host);
        assert_eq!(body(&out, &state).text, typed, "{hidden:?} の間に送った draft を消した（通らなかった時に打った文が無い）");
    }
}

/// R2-01: 02 の「選択した商品」も同じ形: 選ぶ品が替わったら、前の品で打った数量を出さない（戻れば打ちかけが戻る）。
#[test]
fn item_form_shows_the_selected_items_own_quantity() {
    let h = Harness::new(1440.0);
    let app = load("v3/02_inventory");
    let apple = json!({"name": "りんご", "quantity": 3});
    let orange = json!({"name": "みかん", "quantity": 1});
    let mut host = Fake::default().with("local.inventory", "items", json!([apple, orange])).with("local.inventory", "selected", apple.clone());
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let out = h.click(&app, &mut state, &mut host, find(&out, "りんご").rect.center());
    let quantity = |out: &egui::FullOutput, state: &AppState| input_in(out, state, "inventory-item-form", "TextInput");
    assert_eq!(quantity(&out, &state).text, "3");
    let at = quantity(&out, &state).rect.center();
    h.pause(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("0".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(quantity(&out, &state).text, "30");
    // 欄に焦点を置いたまま、runtime（Agent 等）が selected を みかん に替える: 打ちかけの "30" を みかん の欄に持ち越さない
    host.data.insert(("local.inventory".into(), "selected".into()), orange.clone());
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(quantity(&out, &state).text, "1", "焦点のあった欄が、りんご で打ちかけた数量を みかん の欄に出している");
    // 表の行も みかん にする
    let out = h.click(&app, &mut state, &mut host, find(&out, "みかん").rect.center());
    assert_eq!(quantity(&out, &state).text, "1", "りんご で打った数量が みかん の欄に出ている");
    // りんご に戻ると打ちかけが戻る
    host.data.insert(("local.inventory".into(), "selected".into()), apple);
    let out = h.click(&app, &mut state, &mut host, find(&out, "りんご").rect.center());
    assert_eq!(quantity(&out, &state).text, "30");
}

/// R2-01: editor（04 の本文・bind 先は selected.body）も同じ: 選んだノートが替われば、そのノートの本文を出す（戻れば打ちかけが戻る）。
#[test]
fn editor_draft_follows_the_bound_record() {
    let h = Harness::new(1440.0);
    let app = load("v3/04_research_notes");
    let mut host = Fake::default().with("local.notes", "selected.body", json!("A のノート"));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let editor = |out: &egui::FullOutput, state: &AppState| input_in(out, state, "research-editor", "MultilineTextInput");
    assert_eq!(editor(&out, &state).text, "A のノート");
    let at = editor(&out, &state).rect.center();
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("に足した文".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    let typed = editor(&out, &state).text;
    assert!(typed.contains("に足した文"), "{typed}");
    host.data.insert(("local.notes".into(), "selected.body".into()), json!("B のノート"));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(editor(&out, &state).text, "B のノート", "A で打った文が B の本文に出ている");
    host.data.insert(("local.notes".into(), "selected.body".into()), json!("A のノート"));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(editor(&out, &state).text, typed, "A に戻ったのに打ちかけの文が消えた");
}

/// R2-01: bind 先が中身を出せない間（未接続・読み込み中 等）は、どの record への入力か決められないので、欄は打てず送れない（editor と同じ）。
/// 送れない理由はその状態（「本文を入れると押せます」と打てない欄を指さない）。
#[test]
fn form_fields_lock_while_the_bound_record_is_unknown() {
    let h = Harness::new(1440.0);
    let root = json!({"key": "root", "component": "column", "children": [{
        "key": "f", "component": "form", "label": "返信", "action_ref": "local.save",
        "props": {"fields": [{"name": "body", "label": "本文", "input": "multiline"}]},
        "binding": {"source_ref": "remote", "path": "selected"}
    }]});
    let actions = json!([{"id": "local.save", "effect": "local_write", "readiness": "ready", "required_scopes": [],
        "input_schema": {"type": "object", "required": ["body"], "properties": {"body": {"type": "string"}}}}]);
    let app = synthetic(root, actions, json!([{"id": "remote", "kind": "external_api", "readiness": "requires_connection"}]));
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let field = input_in(&out, &state, "f", "MultilineTextInput");
    assert!(field.disabled, "未接続の bind 先の欄に打てる");
    assert_eq!(state.gates.get("f").map(|g| g.state), Some(Some(RegionState::Disconnected)), "{:?}", state.gates.get("f"));
    h.click(&app, &mut state, &mut host, field.rect.center());
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("打てないはず".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!has_text(&out, "打てないはず"), "未接続の間に打てた");
}

/// 外部 key（`*_id`）だけを持つ行の list と table、行を選んでいる間だけ出る node。
fn fk_app() -> AppDefinitionV3 {
    synthetic(
        json!({"key": "root", "component": "column", "children": [
            {"key": "facts", "component": "list", "label": "根拠", "binding": {"source_ref": "local.s", "path": "facts"}},
            {"key": "msgs", "component": "table", "label": "メッセージ", "props": {"columns": ["body"]}, "binding": {"source_ref": "local.s", "path": "msgs"}},
            {"key": "picked", "component": "text", "label": "選んでいる時だけ", "visibility": {"when": "has_selection"}}
        ]}),
        json!([]),
        json!([{"id": "local.s", "kind": "local_collection", "readiness": "local"}]),
    )
}

/// R2-02: 外部 key（`*_id`）だけが同じ行は、それぞれ選べる（2 行目を押すと 2 行目が選ばれ、もう 1 度押すと外れる）。list と table の両方。
#[test]
fn rows_sharing_only_a_foreign_key_are_separately_selectable() {
    let h = Harness::new(1440.0);
    let app = fk_app();
    let mut host = Fake::default()
        .with("local.s", "facts", json!([{"recipient_id": "r1", "text": "一つ目の根拠"}, {"recipient_id": "r1", "text": "二つ目の根拠"}]))
        .with("local.s", "msgs", json!([{"thread_id": "t1", "body": "一通目"}, {"thread_id": "t1", "body": "二通目"}]));
    let mut state = AppState::default();
    for (first, second) in [("一つ目の根拠", "二つ目の根拠"), ("一通目", "二通目")] {
        let out = h.settle(&app, &mut state, &mut host);
        let out = h.click(&app, &mut state, &mut host, find(&out, second).rect.center());
        assert_eq!(find(&out, second).toggled.as_deref(), Some("True"), "{second}: 2 行目を押したのに選ばれていない");
        assert_eq!(find(&out, first).toggled.as_deref(), Some("False"), "{second}: 外部 key が同じ 1 行目に選択が移った");
        assert!(state.rects.contains_key("picked"));
        let out = h.click(&app, &mut state, &mut host, find(&out, second).rect.center());
        assert_eq!(find(&out, second).toggled.as_deref(), Some("False"), "{second}: もう 1 度押しても外れない");
    }
    assert!(state.rows.is_empty(), "{:?}", state.rows);
}

/// R2-02: 外部 key が同じ兄弟がいる行を選び、その行が消えたら選択は外れる（残った兄弟へ移さない）。
/// 鍵が 1 行だけを指す間だけ鍵で追い、2 行以上に当たった鍵は以後使わない（[x] → [x, y] → [y] でも y に移さない）。
#[test]
fn a_vanished_row_is_never_replaced_by_a_sibling_with_the_same_foreign_key() {
    let h = Harness::new(1440.0);
    let app = fk_app();
    let x = json!({"thread_id": "t1", "body": "一通目"});
    let y = json!({"thread_id": "t1", "body": "二通目"});
    // 1) 兄弟がいる所で y を押し、押した直後（同じ data の frame を挟まず）に y が消える: 押した時に鍵が 1 行に決まらない事を見ている
    let mut host = Fake::default().with("local.s", "msgs", json!([x, y]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let at = find(&out, "二通目").rect.center();
    let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    h.frame(&app, &mut state, &mut host, vec![egui::Event::PointerMoved(at), press(true)]);
    h.frame(&app, &mut state, &mut host, vec![press(false)]);
    assert!(!state.rows.is_empty(), "押したのに選ばれていない");
    host.data.insert(("local.s".into(), "msgs".into()), json!([x]));
    let out = h.settle(&app, &mut state, &mut host);
    assert!(state.rows.is_empty(), "消えた行の選択が、外部 key が同じ兄弟に移った: {:?}", state.rows);
    assert_eq!(find(&out, "一通目").toggled.as_deref(), Some("False"));
    assert!(!state.rects.contains_key("picked"), "選んだ行が無いのに has_selection の node が出ている");
    // 2) 一人っ子の時に選び（鍵で追う）、兄弟が増え（鍵が 2 行に当たる）、元の行が消える
    let mut host = Fake::default().with("local.s", "msgs", json!([x]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, find(&out, "一通目").rect.center());
    host.data.insert(("local.s".into(), "msgs".into()), json!([x, y]));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(find(&out, "一通目").toggled.as_deref(), Some("True"));
    assert_eq!(find(&out, "二通目").toggled.as_deref(), Some("False"));
    host.data.insert(("local.s".into(), "msgs".into()), json!([y]));
    let out = h.settle(&app, &mut state, &mut host);
    assert!(state.rows.is_empty(), "鍵が兄弟にも当たった後で、消えた行の選択が兄弟に移った: {:?}", state.rows);
    assert_eq!(find(&out, "二通目").toggled.as_deref(), Some("False"));
}

/// R2-02 の裏（R1-03 の追跡を残す）: 鍵が 1 行だけを指す行は、中身が変わっても選択が付いて行く。`id` が在れば、`*_id`（外部 key）が変わっても同じ record。
#[test]
fn a_row_with_a_unique_key_keeps_its_selection_when_its_content_changes() {
    let h = Harness::new(1440.0);
    let app = fk_app();
    let mut host = Fake::default()
        .with("local.s", "msgs", json!([{"item_id": "i1", "body": "りんご", "quantity": 3}, {"item_id": "i2", "body": "みかん", "quantity": 1}]))
        .with("local.s", "facts", json!([{"id": "m1", "thread_id": "t1", "text": "本文"}, {"id": "m2", "thread_id": "t1", "text": "別の本文"}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let out = h.click(&app, &mut state, &mut host, find(&out, "りんご").rect.center());
    h.click(&app, &mut state, &mut host, find(&out, "本文").rect.center());
    // runtime が数量を書き換え、m1 を別の thread に移す
    host.data.insert(("local.s".into(), "msgs".into()), json!([{"item_id": "i1", "body": "りんご", "quantity": 5}, {"item_id": "i2", "body": "みかん", "quantity": 1}]));
    host.data.insert(("local.s".into(), "facts".into()), json!([{"id": "m1", "thread_id": "t2", "text": "本文"}, {"id": "m2", "thread_id": "t1", "text": "別の本文"}]));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(find(&out, "りんご").toggled.as_deref(), Some("True"), "中身が変わった item_id の行から選択が外れた");
    assert_eq!(find(&out, "本文").toggled.as_deref(), Some("True"), "thread_id が変わった id の行から選択が外れた");
}

// ---------------------------------------------------------------------------
// round 3 の確定した指摘（R3-01 / R3-02）の再現 test
// ---------------------------------------------------------------------------

/// R3-01（DESIGN.md:213・CD-03 の retryable）: 保存が失敗した（runtime が form の region を Error と言う）後も、打った文は欄に残り、
/// 欄は打て、保存 はもう 1 度押せる（失敗の 1 行は出す）。Error で欄が空・読み取り専用・送れないと、利用者は抜け出せない。
#[test]
fn a_failed_save_keeps_the_typed_text_and_can_be_resent() {
    let h = Harness { height: 1400.0, ..Harness::new(1440.0) };
    let app = load("v3/09_owner_test2_calendar_mail_chat");
    let a = json!({"id": "draft-a", "to": "a@example.com", "subject": "A の件", "body": "A の元の本文"});
    let mut host = Fake::default().with("local.drafts", "selected", a).with("local.proposals", "pending", json!([{"id": "p1", "title": "提案"}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    let out = h.click(&app, &mut state, &mut host, find(&out, "提案").rect.center());
    let body = |out: &egui::FullOutput, state: &AppState| input_in(out, state, "cm-reply", "MultilineTextInput");
    let at = body(&out, &state).rect.center();
    h.pause(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("（返事）".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    let typed = body(&out, &state).text;
    assert!(typed.contains("返事"), "{typed}");
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    let sent = host.invoked.len();
    // 保存が失敗した（Timeout / ConnectionLost 等）
    host.regions.insert("cm-reply".into(), (RegionState::Error, "保存できませんでした".into()));
    let out = h.settle(&app, &mut state, &mut host);
    assert!(has_text(&out, "保存できませんでした"), "失敗を知らせていない");
    let field = body(&out, &state);
    assert_eq!(field.text, typed, "失敗した後に打った文が欄から消えた");
    assert!(!field.disabled, "失敗した後に欄が打てない");
    let save = find(&out, "Save");
    assert!(!save.disabled, "失敗した後に 保存 を押せない: {:?}", state.gates.get("cm-reply"));
    h.click(&app, &mut state, &mut host, save.rect.center());
    assert_eq!(host.invoked.len(), sent + 1, "失敗した後に送り直せない");
    assert_eq!(host.invoked.last().map(|i| i.input["body"].clone()), Some(json!(typed)));
}

/// 主領域の AX の中で `text` の名前を持つ i 番目の要素。
fn nth(out: &egui::FullOutput, text: &str, i: usize) -> Ax {
    ax(out).into_iter().filter(|e| e.text == text).nth(i).unwrap_or_else(|| panic!("'{text}' の {i} 番目が無い"))
}

/// R3-02: 中身まで同じ 2 行（id も無い）でも、2 行目を押せば 2 行目が選ばれ、もう 1 度押せば外れる（1 行目に選択を移さない）。list と table の両方。
#[test]
fn twin_rows_with_identical_content_are_separately_selectable() {
    let h = Harness::new(1440.0);
    let app = fk_app();
    let mut host = Fake::default()
        .with("local.s", "facts", json!([{"recipient_id": "r1", "text": "同じ根拠"}, {"recipient_id": "r1", "text": "同じ根拠"}]))
        .with("local.s", "msgs", json!([{"thread_id": "t1", "body": "同じ本文"}, {"thread_id": "t1", "body": "同じ本文"}]));
    let mut state = AppState::default();
    for twin in ["同じ根拠", "同じ本文"] {
        let out = h.settle(&app, &mut state, &mut host);
        let out = h.click(&app, &mut state, &mut host, nth(&out, twin, 1).rect.center());
        assert_eq!(nth(&out, twin, 1).toggled.as_deref(), Some("True"), "{twin}: 2 行目を押したのに選ばれていない");
        assert_eq!(nth(&out, twin, 0).toggled.as_deref(), Some("False"), "{twin}: 中身が同じ 1 行目に選択が移った");
        let out = h.click(&app, &mut state, &mut host, nth(&out, twin, 1).rect.center());
        assert_eq!(nth(&out, twin, 1).toggled.as_deref(), Some("False"), "{twin}: もう 1 度押しても外れない");
        assert_eq!(nth(&out, twin, 0).toggled.as_deref(), Some("False"), "{twin}");
    }
    assert!(state.rows.is_empty(), "{:?}", state.rows);
}

/// W2 Done 3: Project の Page は Page の木を描く（app の木ではない）。見出しは Page の名前。部品の表に無い部品は「未対応の部品」の 1 行で、偽の描画をしない。
#[test]
fn render_page_draws_the_page_tree_and_names_unsupported_parts() {
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "app-only", "component": "text", "label": "app の木"}]}),
        ready_action(),
        json!([]),
    );
    let page: domain::project::PageDefinition = serde_json::from_value(json!({
        "id": "page.b", "route": "/b", "title": "Tests の Page", "revision": 1,
        "root": {"key": "b-root", "component": "column", "children": [
            {"key": "b-text", "component": "text", "label": "Page の文"},
            {"key": "b-unknown", "component": "foo"}
        ]}
    }))
    .unwrap();
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut fake = Fake::default();
    let mut out = None;
    for _ in 0..2 {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))), ..Default::default() };
        out = Some(h.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui::shell::draw_app_region(ui, &mut |ui| renderer::render_page(ui, &app, &page, &mut state, &mut fake, Default::default()));
            });
        }));
    }
    let out = out.unwrap();
    assert!(state.rects.contains_key("b-text") && state.rects.contains_key("b-unknown"), "{:?}", state.rects.keys());
    assert!(!state.rects.contains_key("app-only"), "app の木を描かない");
    assert!(!has_text(&out, "Tests の Page"), "Page の名前は tab が言う（W15-c2）ので Page の頭に重ねない");
    assert!(has_text(&out, "This version can't show one part (foo)"));
    assert!(!has_text(&out, "app の木"));
}

// ---------------------------------------------------------------------------
// W2b holdout（実装を見ない検査役の的。契約は src/ui/ax.rs・AppState::clear_entry_after_save・components.rs list_meta）。
// interface の commit（2b8a678）では #[ignore] で 3 本とも赤。実装の commit で外した。
// ---------------------------------------------------------------------------

/// 読書記録の「足す」form と同じ形: bind 先は path `draft`（data 無し）・欄は題名と著者。
fn w2b_entry_app() -> AppDefinitionV3 {
    synthetic(
        json!({"key": "root", "component": "column", "children": [{
            "key": "entry", "component": "form", "label": "本を足す", "action_ref": "local.do",
            "props": {"fields": [{"name": "title", "label": "題名", "input": "text"}, {"name": "author", "label": "著者", "input": "text"}]},
            "binding": {"source_ref": "local.s", "path": "draft"}
        }]}),
        ready_action(),
        json!([{"id": "local.s", "kind": "local_collection", "readiness": "local"}]),
    )
}

/// AccessKit の木の中で、`within` の領域に在る `role` の、上から i 番目の (node id, 今の文)。
fn w2b_field(out: &egui::FullOutput, state: &AppState, within: &str, role: &str, i: usize) -> (egui::accesskit::NodeId, Rect, String) {
    let area = state.rects[within];
    let update = out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
    let mut found: Vec<_> = update
        .nodes
        .iter()
        .filter(|(_, n)| format!("{:?}", n.role()) == role)
        .filter_map(|(id, n)| {
            let b = n.bounds()?;
            let r = Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32));
            area.contains_rect(r).then(|| (*id, r, n.value().unwrap_or("").to_owned()))
        })
        .collect();
    found.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    found.into_iter().nth(i).unwrap_or_else(|| panic!("'{within}' の中に {role} の {i} 番目が無い"))
}

/// src/ui/ax.rs の契約: AXPress（accesskit `Click`）を入力欄に送ると focus が入り、次の keyboard の文字がその欄に入る。
#[test]
fn w2b_ax_press_on_a_field_focuses_it() {
    let h = Harness::new(1440.0);
    let app = w2b_entry_app();
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let (target, _, _) = w2b_field(&out, &state, "entry", "TextInput", 0);
    let press = egui::accesskit::ActionRequest { action: egui::accesskit::Action::Click, target, data: None };
    h.frame(&app, &mut state, &mut host, vec![egui::Event::AccessKitActionRequest(press)]);
    h.settle(&app, &mut state, &mut host);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("吾輩は猫である".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 0).2, "吾輩は猫である", "AXPress の後に打った文が欄に入らない（focus が来ていない）");
}

/// AppState::clear_entry_after_save の契約: bind 先の record が無い「足す」form は、runtime が通した（region が無い）保存の後に欄が空に戻る。
#[test]
fn w2b_entry_form_is_empty_after_a_verified_save() {
    let h = Harness::new(1440.0);
    let app = w2b_entry_app();
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let at = w2b_field(&out, &state, "entry", "TextInput", 0).1.center();
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("坊っちゃん".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 0).2, "坊っちゃん", "正の対照: pointer の click では打てる");
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    assert_eq!(host.invoked.last().map(|i| i.input.clone()), Some(json!({"title": "坊っちゃん"})));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 0).2, "", "保存が通った後も題名の欄に前の文が残っている");
}

/// components.rs list_meta の契約: list の `columns` の 2 つ目以降（著者）が行の右に出る。
#[test]
fn w2b_list_shows_the_columns_after_the_title() {
    let h = Harness::new(1440.0);
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [
            {"key": "books", "component": "list", "label": "本", "props": {"columns": ["title", "author"]}, "binding": {"source_ref": "local.s", "path": "items"}}
        ]}),
        json!([]),
        json!([{"id": "local.s", "kind": "local_collection", "readiness": "local"}]),
    );
    let mut host = Fake::default().with("local.s", "items", json!([{"id": "b1", "title": "吾輩は猫である", "author": "夏目漱石"}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!glyph_colors(&out, "吾輩は猫である").is_empty(), "正の対照: 題名は描かれている");
    assert!(!glyph_colors(&out, "夏目漱石").is_empty(), "著者が一覧に出ていない");
}

/// ax.rs の契約: `SetValue` の要求で欄の文が置き換わり、draft に入る（macOS からは今は届かない＝unit だけの口）。
#[test]
fn w2b_set_value_replaces_the_field_text() {
    let h = Harness::new(1440.0);
    let app = w2b_entry_app();
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let (target, _, before) = w2b_field(&out, &state, "entry", "TextInput", 1);
    assert_eq!(before, "");
    let set = egui::accesskit::ActionRequest { action: egui::accesskit::Action::SetValue, target, data: Some(egui::accesskit::ActionData::Value("夏目漱石".into())) };
    h.frame(&app, &mut state, &mut host, vec![egui::Event::AccessKitActionRequest(set)]);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 1).2, "夏目漱石", "SetValue の値が欄に入らない");
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 0).2, "", "SetValue が別の欄に入った");
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    assert_eq!(host.invoked.last().map(|i| i.input.clone()), Some(json!({"author": "夏目漱石"})), "SetValue の値が送る値に入らない");
}

/// egui が扱う `Focus`（= macOS の AXFocused の set）でも focus が入り、打った文が入る（W2b で壊していない事の確認）。
#[test]
fn w2b_ax_focus_on_a_field_focuses_it() {
    let h = Harness::new(1440.0);
    let app = w2b_entry_app();
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    let (target, _, _) = w2b_field(&out, &state, "entry", "TextInput", 1);
    let focus = egui::accesskit::ActionRequest { action: egui::accesskit::Action::Focus, target, data: None };
    h.frame(&app, &mut state, &mut host, vec![egui::Event::AccessKitActionRequest(focus)]);
    h.settle(&app, &mut state, &mut host);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("夏目漱石".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 1).2, "夏目漱石");
}

/// clear_entry_after_save の契約の裏: 保存が通らなかった（runtime が region を置いた）時は打った文を消さない。
#[test]
fn w2b_entry_form_keeps_the_text_when_the_save_fails() {
    let h = Harness::new(1440.0);
    let app = w2b_entry_app();
    let (mut state, mut host) = (AppState::default(), Fake { fails_with: Some((RegionState::Error, "保存できませんでした".into())), ..Fake::default() });
    let out = h.settle(&app, &mut state, &mut host);
    let at = w2b_field(&out, &state, "entry", "TextInput", 0).1.center();
    h.click(&app, &mut state, &mut host, at);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("坊っちゃん".into())]);
    let out = h.settle(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, find(&out, "Save").rect.center());
    assert_eq!(host.invoked.len(), 1);
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(w2b_field(&out, &state, "entry", "TextInput", 0).2, "坊っちゃん", "保存が通らなかったのに打った文が消えた");
}

/// list_meta の契約の細部: 空・null の欄は飛ばし、`*_id` は出さず、columns の順に " · " で繋ぐ。
#[test]
fn w2b_list_meta_skips_empty_and_id_columns() {
    let h = Harness::new(1440.0);
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [
            {"key": "books", "component": "list", "label": "本", "props": {"columns": ["title", "shelf_id", "author", "note", "year"]}, "binding": {"source_ref": "local.s", "path": "items"}}
        ]}),
        json!([]),
        json!([{"id": "local.s", "kind": "local_collection", "readiness": "local"}]),
    );
    let mut host = Fake::default().with("local.s", "items", json!([{"title": "こころ", "shelf_id": "s9", "author": "夏目漱石", "note": "", "year": 1914}]));
    let mut state = AppState::default();
    let out = h.settle(&app, &mut state, &mut host);
    // 文の一致は完全一致: `s9`（*_id）や空の note が混ざれば別の文になって赤
    assert!(!glyph_colors(&out, "夏目漱石 · 1914").is_empty(), "補足が「夏目漱石 · 1914」でない（*_id・空の欄・順）");
}

// ---------------------------------------------------------------------------
// W3: 面の状態の 1 行（`surface-status:<id>`・YC_ACCEPTANCE §4 の L1）。
// ---------------------------------------------------------------------------

const SUCCESS_WORDS: [&str; 10] = ["接続済み", "保存しました", "保存済み", "済み", "完了", "✓", "Connected", "Saved", "Done", "Verified"];

fn receipt(state: domain::project::ReceiptState) -> domain::project::SurfaceReceipt {
    domain::project::SurfaceReceipt {
        surface_id: "docs".into(),
        page_id: "page.docs".into(),
        kind: domain::project::ReceiptKind::NativeApp,
        state,
        oracle: "find_app: この Mac に入っていません".into(),
        app_id: "com.apple.TextEdit".into(),
        app_version: "1.19".into(),
        os_version: "14.5 (23F79)".into(),
        operation: "1 行足して保存".into(),
        verified_at: "2026-10-20T00:00:00.000Z".into(),
        evidence_ref: "docs/evidence/W3/x.json".into(),
    }
}

/// 画面に出る文（glyph・状態語・1 行）を全部つないで見る。verified 以外（読み戻しが無いを含む）は成功の語を 1 つも含まない。
#[test]
fn w3_surface_status_line_says_success_only_when_verified() {
    use domain::project::ReceiptState as S;
    let shown = |r: Option<&domain::project::SurfaceReceipt>| {
        let (state, line) = renderer::surface_status_line(r);
        format!("{} · {line}", state.wording())
    };
    for r in [None, Some(receipt(S::Unverified)), Some(receipt(S::Unavailable)), Some(receipt(S::Ended))] {
        let text = shown(r.as_ref());
        assert!(!SUCCESS_WORDS.iter().any(|w| text.contains(w)), "成功の語: {text}");
        assert!(text.chars().count() > 8, "次の一手の 1 行が無い: {text}");
    }
    assert!(shown(Some(&receipt(S::Unavailable))).contains("isn't installed"), "unavailable は理由を言う");
    assert!(shown(Some(&receipt(S::Verified))).contains('✓'));
    // W16-2: 窓の文書が disk と違う（保存していない）・面の file を disk で読めない。どちらも成功の語なしで次の一手
    let not_on_disk = domain::project::SurfaceReceipt { oracle: domain::project::NOT_ON_DISK.into(), ..receipt(S::Unverified) };
    // disk を読めない receipt の oracle は NativeError::FileUnreadable の 1 行そのまま（74ec7fb の口・頭を足さない）
    let unreadable = domain::project::SurfaceReceipt {
        oracle: "README.md を読めません（ありません）。repo の中に README.md が在るか確かめてから、この Page に戻ってください".into(),
        ..receipt(S::Unavailable)
    };
    // 保存せずに Undo して窓が disk と同じに戻った（IN_SYNC）: W3 の「まだ読み戻していません」に落とさない
    let in_sync = domain::project::SurfaceReceipt { oracle: domain::project::IN_SYNC.into(), ..receipt(S::Unverified) };
    // W16-3 / W16-2-b: 外で書き換わった。保存させる 1 行に落とさない
    let changed_outside = domain::project::SurfaceReceipt { oracle: domain::project::CHANGED_OUTSIDE.into(), ..receipt(S::Unverified) };
    for (r, want) in [
        (&not_on_disk, "TextEdit has unsaved changes. Save in TextEdit and Shibubu reads them back"),
        (&unreadable, "README.md を読めません（ありません）。repo の中に README.md が在るか確かめてから"),
        (&in_sync, "The document in TextEdit's window matches what's on disk"),
        (&changed_outside, "The TextEdit document was changed outside. Reload it in TextEdit, or check with Open in TextEdit"),
    ] {
        let text = shown(Some(r));
        assert!(text.contains(want), "{text}");
        assert!(!text.contains("Not checked by Shibubu") && !text.contains("Couldn't find TextEdit"), "W3 の 1 行に落ちた: {text}");
        assert!(!SUCCESS_WORDS.iter().any(|w| text.contains(w)) && !text.contains("file_sha") && !text.contains("ax_text"), "成功の語か内部の語: {text}");
    }
}

/// source_surface は `surface-status:<surface_ref>` の AX node（値 = state）を出し、状態の文字はその子に置く。読み戻しが無い面は `unchecked`。
#[test]
fn w3_source_surface_draws_its_status_under_the_surface_status_identifier() {
    let app = synthetic(json!({"key": "root", "component": "column", "children": []}), ready_action(), json!([]));
    let page: domain::project::PageDefinition = serde_json::from_value(json!({
        "id": "page.docs", "route": "/docs", "title": "Docs", "revision": 1,
        "root": {"key": "docs-root", "component": "column", "children": [
            {"key": "docs-surface", "component": "source_surface", "label": "README.md（TextEdit）", "props": {"surface_ref": "docs"}},
            {"key": "other-surface", "component": "source_surface", "label": "別", "props": {"surface_ref": "other"}},
            {"key": "unverified-surface", "component": "source_surface", "label": "Web", "props": {"surface_ref": "unverified"}}
        ]}
    }))
    .unwrap();
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut fake = Fake::default();
    fake.receipts.insert("docs".into(), receipt(domain::project::ReceiptState::Unavailable));
    fake.receipts.insert("unverified".into(), receipt(domain::project::ReceiptState::Unverified));
    let mut out = None;
    for _ in 0..2 {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))), ..Default::default() };
        out = Some(h.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui::shell::draw_app_region(ui, &mut |ui| renderer::render_page(ui, &app, &page, &mut state, &mut fake, Default::default()));
            });
        }));
    }
    let out = out.unwrap();
    let nodes: HashMap<_, _> = out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().cloned().collect();
    let status = |sid: &str| {
        nodes.iter().find(|(_, n)| n.author_id() == Some(&format!("surface-status:{sid}"))).unwrap_or_else(|| panic!("surface-status:{sid} が無い"))
    };
    fn texts(nodes: &HashMap<egui::accesskit::NodeId, egui::accesskit::Node>, id: egui::accesskit::NodeId, out: &mut Vec<String>) {
        let n = &nodes[&id];
        out.extend(n.label().or(n.value()).map(String::from));
        for c in n.children() {
            texts(nodes, *c, out);
        }
    }
    let (id, n) = status("docs");
    assert_eq!(n.value(), Some("unavailable"));
    let mut under = Vec::new();
    for c in n.children() {
        texts(&nodes, *c, &mut under);
    }
    assert!(under.iter().any(|t| t.contains("isn't installed")), "状態の文字が surface-status の子に無い: {under:?} ({id:?})");
    assert_eq!(status("other").1.value(), Some("unchecked"), "読み戻しの無い面は unchecked");
    // UI-1 反証の直し: 承認の button が無い帯（読み戻しの無い面・確かめていない面）に「Needs approval」を出さない。label と子の文字の全部で見る
    for sid in ["other", "unverified"] {
        let (id, n) = status(sid);
        let mut shown = vec![n.label().unwrap_or_default().to_string()];
        for c in n.children() {
            texts(&nodes, *c, &mut shown);
        }
        assert!(shown.iter().all(|t| !t.contains("Needs approval") && !t.contains('▣')), "{sid}: 承認の無い帯に Needs approval: {shown:?} ({id:?})");
        assert!(shown.iter().any(|t| t.contains("Not checked")), "{sid}: 確かめていない事実の 1 行が無い: {shown:?}");
        // G87: 画面には文でなく印（hover で同じ文）。AX の label は 1 行のまま（上）
        let mut kids = Vec::new();
        for c in n.children() {
            texts(&nodes, *c, &mut kids);
        }
        assert!(!kids.is_empty() && kids.iter().all(|t| !t.contains("Not checked")), "{sid}: 確かめていない事実を画面に 1 行で描いた（印と hover にする）: {kids:?}");
    }
}

/// 反証の直し（W3）: Web の unverified は誰も確かめていないので「在ります」と言わない（Diagram の Page）。
#[test]
fn w3_web_unverified_line_does_not_say_it_exists() {
    let mut r = receipt(domain::project::ReceiptState::Unverified);
    r.kind = domain::project::ReceiptKind::Web;
    r.app_id = "https://excalidraw.com".into();
    r.oracle = "none: WebView の中身は自己申告だけ（操作とは別の道が無い）".into();
    let (_, line) = renderer::surface_status_line(Some(&r));
    // 表の EN は tooltip の `Not checked by Shibubu`（site の名前を言わない）
    assert!(!line.contains("is on this Mac") && !line.contains(" exists"), "確かめていない site を在ると言う: {line}");
    assert_eq!(line, "Not checked by Shibubu");
    assert!(!line.contains("https://"), "{line}");
}

/// 反証の直し（W3）: 面の 1 行に内部の語（bundle id・find_app・bridge・oracle の原文）を出さず app の名前で言う。入っていない app を開けとは言わない。
#[test]
fn w3_status_line_speaks_app_names_and_doable_next_steps() {
    use domain::project::ReceiptState as S;
    let internal = ["com.apple.", "find_app", "bridge", "none:"];
    let mut bridge_down = receipt(S::Unavailable);
    bridge_down.oracle = "bridge が起きていないので、app が在るかを確かめていません".into();
    let mut find_err = receipt(S::Unavailable);
    find_err.oracle = "find_app: timeout".into();
    for r in [receipt(S::Unavailable), bridge_down.clone(), find_err, receipt(S::Ended)] {
        let (_, line) = renderer::surface_status_line(Some(&r));
        assert!(!internal.iter().any(|w| line.contains(w)), "内部の語: {line}");
        assert!(line.contains("TextEdit"), "app の名前: {line}");
    }
    // 読み戻しの無い unverified は表の EN（tooltip の `Not checked by Shibubu`）: 名前も「在る」も言わない
    let (_, unverified) = renderer::surface_status_line(Some(&receipt(S::Unverified)));
    assert_eq!(unverified, "Not checked by Shibubu");
    let (_, missing) = renderer::surface_status_line(Some(&receipt(S::Unavailable)));
    assert!(missing.contains("isn't installed") && !missing.contains("Open"), "入っていない app を開けとは言えない: {missing}");
    let (_, unsure) = renderer::surface_status_line(Some(&bridge_down));
    assert!(!unsure.contains("isn't installed"), "確かめていないのに入っていないと言う: {unsure}");
}

/// 反証の直し（W3）: 押す button の無い source_surface は「開きます」と約束しない（W3 の Docs・Diagram の Page の面は action を持たない）。
#[test]
fn w3_source_surface_without_an_action_promises_no_opening() {
    let h = Harness::new(1440.0);
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [
            {"key": "docs-surface", "component": "source_surface", "label": "README.md（TextEdit）", "props": {"surface_ref": "docs"}}
        ]}),
        json!([]),
        json!([]),
    );
    let (mut state, mut host) = (AppState::default(), Fake::default());
    let out = h.settle(&app, &mut state, &mut host);
    assert!(state.rects.contains_key("docs-surface"));
    assert!(!has_text(&out, "Opens"), "button の無い面が開くと約束する");
    assert!(!ax(&out).iter().any(|a| a.text == "Open window"));
}

/// TERM-ONE-1101（owner 181「terminalごとにchat barがあるのがおかしい」）: terminal ごとの入力欄（`Type in the terminal`・`Send`）は無い。動いている grid も、
/// 画面を写せない Terminal（`screen` が None）も欄を出さない（打つのは grid に直に）。負の対照: terminal_body に欄の腕を戻す → 赤。
#[test]
fn term_one_terminal_has_no_input_field() {
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    use domain::terminal::{TermCursor, TermScreen};
    let screen = TermScreen { rows: 24, cols: 80, lines: vec![vec![]; 24], cursor: Some(TermCursor { row: 0, col: 0 }), ..TermScreen::default() };
    let pane = renderer::TerminalPane { text: screen.text(), screen: Some(screen), pid: Some(4242), accepts_input: true, status_line: "zsh · pid 4242".into(), view: None };
    let mut host = Fake { terminal: Some(pane.clone()), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    find(&out, "zsh · pid 4242");
    assert!(!has_text(&out, "Type in the terminal") && !has_text(&out, "Send"), "動いている grid に入力欄を出した");
    host.terminal = Some(renderer::TerminalPane { screen: None, ..pane });
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!has_text(&out, "Type in the terminal") && !has_text(&out, "Send"), "画面を写せない Terminal に入力欄を出した");
}

/// TERM-ONE-1101（CHAT-ONE-1101 の 3）: IME の変換中の字（preedit）は grid の cursor の所に描かれ、PTY には送らない。変換の間の Enter は IME の物で送らない。
/// 確定（`Ime(Commit)`）で確定の字だけが PTY に届き、変換中の字は消える。
/// 負の対照: terminal_grid の preedit を描く腕を外す → 「にほん」が描かれず赤・Key を落とす filter を外す → Enter の `\r` が届いて赤。
#[test]
fn term_one_ime_preedit_is_drawn_on_the_grid_and_only_the_commit_is_sent() {
    use domain::terminal::{TermCursor, TermRun, TermScreen};
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut lines = vec![vec![]; 24];
    lines[0] = vec![TermRun { text: "% ".into(), ..TermRun::default() }];
    let screen = TermScreen { rows: 24, cols: 80, lines, cursor: Some(TermCursor { row: 0, col: 2 }), ..TermScreen::default() };
    let pane = renderer::TerminalPane { text: screen.text(), screen: Some(screen), pid: Some(4242), accepts_input: true, status_line: "zsh · pid 4242".into(), view: None };
    let mut host = Fake { terminal: Some(pane), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    let t = out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
    let b = t.nodes.iter().find(|(_, n)| n.author_id() == Some("terminal:t1")).and_then(|(_, n)| n.bounds()).expect("grid の terminal:t1 が無い");
    h.click(&app, &mut state, &mut host, Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32));
    let drawn = |out: &egui::FullOutput, s: &str| out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == s));
    let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::default() };
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Ime(egui::ImeEvent::Preedit("にほん".into()))]);
    let out = h.frame(&app, &mut state, &mut host, vec![]);
    assert!(drawn(&out, "にほん"), "変換中の字が grid に描かれない");
    h.frame(&app, &mut state, &mut host, vec![enter]);
    assert!(host.keys.is_empty(), "変換中の字か Enter を PTY に送った: {:?}", host.keys);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Ime(egui::ImeEvent::Commit("日本".into()))]);
    let out = h.frame(&app, &mut state, &mut host, vec![]);
    let sent: Vec<u8> = host.keys.iter().flat_map(|(_, b)| b.clone()).collect();
    assert_eq!(sent, "日本".as_bytes().to_vec(), "確定の字だけが届く");
    assert!(!drawn(&out, "にほん"), "確定の後も変換中の字が残る");
}

/// L7-ORCA #9: grid の右 click（two-finger click）で押した所に menu。`Copy all` は画面の全部を clipboard へ・`Paste` は clipboard を頼む
/// （egui-winit が次の frame に `Event::Paste` を入れる）・`Clear` は ⌃L を送る。`Split Right` / `Split Down` / `Terminate` は panel の Terminal（席 = Drawer）だけで、
/// Page の Terminal（plan が置いた物）には出さない。選ぶと閉じる。
/// 負の対照: `secondary_clicked` で開く行を外す → menu が無く赤・Clear の byte を `\r` にする → 赤・`panel` の判定を外す → Page に Split が出て赤・
/// Split Down に SplitRight を渡す → 赤。
#[test]
fn l7_grid_right_click_menu_copies_pastes_clears_and_splits_the_panel() {
    use domain::terminal::{InputView, TermCursor, TermRun, TermScreen};
    use ui::terminal_panel::PanelAct;
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut lines = vec![vec![]; 24];
    lines[0] = vec![TermRun { text: "% ls".into(), ..TermRun::default() }];
    let screen = TermScreen { rows: 24, cols: 80, lines, cursor: Some(TermCursor { row: 0, col: 4 }), ..TermScreen::default() };
    let pane = renderer::TerminalPane { text: screen.text(), screen: Some(screen), pid: Some(4242), accepts_input: true, status_line: "zsh · pid 4242".into(), view: Some(InputView::Page { page_id: "p1".into() }) };
    let mut host = Fake { terminal: Some(pane.clone()), ..Fake::default() };
    let grid = |out: &egui::FullOutput| -> Rect {
        let t = out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
        let b = t.nodes.iter().find(|(_, n)| n.author_id() == Some("terminal:t1")).and_then(|(_, n)| n.bounds()).expect("grid の terminal:t1 が無い");
        Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32))
    };
    let at = grid(&h.settle(&app, &mut state, &mut host)).center();
    // Page の Terminal: 写す・貼る・消すだけ
    let out = h.click_with(&app, &mut state, &mut host, at, true);
    for name in ["Copy all", "Paste", "Clear"] {
        find(&out, name);
    }
    assert!(!["Split Right", "Split Down", "Terminate"].iter().any(|n| has_text(&out, n)), "Page の Terminal に panel の手を出した");
    let out = h.click(&app, &mut state, &mut host, find(&out, "Clear").rect.center());
    assert_eq!(host.keys.last(), Some(&("t1".to_owned(), b"\x0c".to_vec())), "Clear が ⌃L を送らない");
    assert!(!has_text(&out, "Copy all"), "選んでも menu が閉じない");
    // Copy all / Paste は離した frame の出力に出る
    let release = |h: &Harness, state: &mut AppState, host: &mut Fake, name: &str| -> egui::FullOutput {
        h.pause(&app, state, host);
        let out = h.click_with(&app, state, host, at, true);
        let p = find(&out, name).rect.center();
        let modifiers = egui::Modifiers::default();
        let button = egui::PointerButton::Primary;
        h.frame(&app, state, host, vec![egui::Event::PointerMoved(p), egui::Event::PointerButton { pos: p, button, pressed: true, modifiers }]);
        h.frame(&app, state, host, vec![egui::Event::PointerButton { pos: p, button, pressed: false, modifiers }])
    };
    let out = release(&h, &mut state, &mut host, "Copy all");
    assert!(out.platform_output.commands.contains(&egui::OutputCommand::CopyText(pane.text.clone())), "Copy all が画面を写さない: {:?}", out.platform_output.commands);
    let out = release(&h, &mut state, &mut host, "Paste");
    let asked = out.viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::RequestPaste));
    assert!(asked, "Paste が clipboard を頼まない");
    // panel の Terminal: 割る・止めるも出す
    host.terminal = Some(renderer::TerminalPane { view: Some(InputView::Drawer), ..pane.clone() });
    h.pause(&app, &mut state, &mut host);
    let out = h.click_with(&app, &mut state, &mut host, at, true);
    for name in ["Copy all", "Paste", "Clear", "Split Right", "Split Down", "Terminate"] {
        find(&out, name);
    }
    h.click(&app, &mut state, &mut host, find(&out, "Split Down").rect.center());
    assert_eq!(host.panel_acts, vec![("t1".to_owned(), PanelAct::SplitDown { tab: "t1".into() })]);
    // 終わった Terminal: 写すだけ（貼る・消すは打てない）
    host.terminal = Some(renderer::TerminalPane { accepts_input: false, view: None, ..pane });
    h.pause(&app, &mut state, &mut host);
    let out = h.click_with(&app, &mut state, &mut host, at, true);
    find(&out, "Copy all");
    assert!(!has_text(&out, "Paste") && !has_text(&out, "Clear"), "終わった Terminal に Paste / Clear");
}

/// W5: Terminal の部品。動いている間は 状態の 1 行・出力（AX の identifier `terminal:<ref>`）で、終わった Terminal は「新しく起こす」だけ（D-e）。
/// TERM-ONE-1101: 入力欄は無い（打つのは grid・`term_one_terminal_has_no_input_field`）。
#[test]
fn w5_terminal_shows_output_under_its_ax_id_and_restarts() {
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let pane = renderer::TerminalPane { text: "% echo hi\nhi\n% ".into(), screen: None, pid: Some(4242), accepts_input: true, status_line: "zsh · pid 4242".into(), view: None };
    let mut host = Fake { terminal: Some(pane.clone()), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    let update = out.platform_output.accesskit_update.as_ref().expect("AccessKit の木が無い");
    let shown: Vec<String> = update.nodes.iter().filter(|(_, n)| n.author_id() == Some("terminal:t1")).map(|(_, n)| n.label().or(n.value()).unwrap_or("").to_owned()).collect();
    assert_eq!(shown, vec![pane.text.clone()], "出力は terminal:t1 の 1 要素で、その名前が出力そのもの");
    // 状態の 1 行は印（● 準備完了 = L1 の成功の語「完了」）を付けない素の行で、AX の要素に完全一致で出る
    find(&out, "zsh · pid 4242");
    assert!(!has_text(&out, "Restart"), "動いている間は起こし直す button を出さない");
    // 終わった Terminal: 「新しく起こす」で host に頼む
    host.terminal = Some(renderer::TerminalPane { text: "bye\n".into(), screen: None, pid: None, accepts_input: false, status_line: "終わりました（終了 code 3）。新しく起こせます".into(), view: None });
    let out = h.settle(&app, &mut state, &mut host);
    h.click(&app, &mut state, &mut host, find(&out, "Restart").rect.center());
    assert_eq!(host.restarted, vec!["t1".to_owned()]);
}

/// G94 ①: 終わった Terminal の `Restart` が Err（消えた Terminal 等）を返したら、捨てずに Terminal の下の 1 行に出す。起こせれば消える。
/// 負の対照: components.rs の `Err(e) => ui.data_mut(..insert_temp..)` を `let _ =` に戻す → 赤。
#[test]
fn g94_restart_refusal_is_shown_in_the_terminal() {
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let pane = renderer::TerminalPane { text: "bye\n".into(), screen: None, pid: None, accepts_input: false, status_line: "Ended".into(), view: None };
    let line = "This terminal is no longer in the project · Add terminal";
    let mut host = Fake { terminal: Some(pane), restart_err: Some(line.into()), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    assert!(!has_text(&out, line), "押す前に理由");
    h.click(&app, &mut state, &mut host, find(&out, "Restart").rect.center());
    let out = h.settle(&app, &mut state, &mut host);
    assert!(has_text(&out, line), "Restart の拒否が Terminal の 1 行に出ない");
    host.restart_err = None;
    h.click(&app, &mut state, &mut host, find(&out, "Restart").rect.center());
    assert!(!has_text(&h.settle(&app, &mut state, &mut host), line), "起こせたのに前の理由が残る");
}

/// W12a: binding を持つ text と list の値は AX の identifier `data:<node key>` の Group の中（試験の道具の git_seen が引く所）。
/// 状態の 1 行（読めていない・空）はその外（L1 の検査に残す）。値が無い時も入れ物は出る（0 行・null を測れる）。
#[test]
fn w12a_bound_text_and_list_draw_their_values_under_the_data_identifier() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [
            {"key": "dev-git-sha", "component": "text", "label": "SHA", "props": {"style": "mono"},
             "binding": {"source_ref": "resource.repo", "path": "head.sha"}, "states": {"empty": "まだ commit がありません"}},
            {"key": "dev-git-changes", "component": "list", "label": "変更した file", "props": {"columns": ["path", "status"]},
             "binding": {"source_ref": "resource.repo", "path": "changes"}, "states": {"empty": "変更した file はありません"}}
        ]}),
        ready_action(),
        json!([{"id": "resource.repo", "kind": "local", "readiness": "local"}]),
    );
    fn under(out: &egui::FullOutput, ident: &str) -> Option<Vec<String>> {
        let nodes: HashMap<_, _> = out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().cloned().collect();
        let (id, _) = nodes.iter().find(|(_, n)| n.author_id() == Some(ident))?;
        fn walk(nodes: &HashMap<egui::accesskit::NodeId, egui::accesskit::Node>, id: egui::accesskit::NodeId, out: &mut Vec<String>) {
            for c in nodes[&id].children() {
                out.extend(nodes[c].label().or(nodes[c].value()).map(String::from));
                walk(nodes, *c, out);
            }
        }
        let mut texts = Vec::new();
        walk(&nodes, *id, &mut texts);
        Some(texts)
    }
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut host = Fake::default()
        .with("resource.repo", "head.sha", json!(sha))
        .with("resource.repo", "changes", json!([{"status": " M", "path": "README.md"}, {"status": "R ", "path": "b.txt", "orig_path": "a.txt"}]));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(under(&out, "data:dev-git-sha"), Some(vec![sha.to_owned()]), "SHA の 40 桁がそのまま data:dev-git-sha の中");
    assert_eq!(under(&out, "data:dev-git-changes"), Some(vec!["README.md".to_owned(), "b.txt".to_owned()]), "行の名前 = path（columns の先頭）");
    // 読めていない（runtime の region が Error）: 入れ物は出るが中は空・状態の 1 行は外
    let mut host = Fake::default().with("resource.repo", "head.sha", json!(sha));
    host.regions.insert("dev-git-sha".into(), (RegionState::Error, "読めていません".into()));
    host.regions.insert("dev-git-changes".into(), (RegionState::Error, "読めていません".into()));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(under(&out, "data:dev-git-sha"), Some(vec![]), "読めていない時は古い SHA を描かない");
    assert_eq!(under(&out, "data:dev-git-changes"), Some(vec![]));
    assert!(has_text(&out, "読めていません") && !has_text(&out, "変更した file はありません"), "読めていない時に空の文を言わない");
    // 読めて 0 行: 入れ物は出て行は 0・空の文は外
    let mut host = Fake::default().with("resource.repo", "changes", json!([]));
    let out = h.settle(&app, &mut state, &mut host);
    assert_eq!(under(&out, "data:dev-git-changes"), Some(vec![]));
    assert!(has_text(&out, "変更した file はありません"));
}

/// W12W-1: host が `embeds` と言う面だけ、残りの幅 × 最小 `EMBED_MIN_HEIGHT` の矩形を予約して毎 frame 1 回 `place_surface` に渡す。
/// 置かない面は今まで通り（矩形を渡さない）。置く面は「後の版です」と言わない（置いたのに嘘を言わない・D-e）。
#[test]
fn w12w1_source_surface_reserves_a_slot_only_for_embedded_surfaces() {
    let h = Harness::new(1440.0);
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [
            {"key": "docs-surface", "component": "source_surface", "label": "README.md（TextEdit）", "props": {"surface_ref": "docs"}},
            {"key": "diagram-surface", "component": "source_surface", "label": "構成図（Excalidraw）", "props": {"surface_ref": "diagram"}}
        ]}),
        json!([]),
        json!([]),
    );
    let (mut state, mut host) = (AppState::default(), Fake { embeds: vec!["diagram".into()], ..Fake::default() });
    let frames = 3;
    let mut out = None;
    for _ in 0..frames {
        out = Some(h.frame(&app, &mut state, &mut host, vec![]));
    }
    let out = out.unwrap();
    assert_eq!(host.placed.len(), frames, "毎 frame 1 回: {:?}", host.placed);
    assert!(host.placed.iter().all(|(id, _)| id == "diagram"), "置かない面の矩形を渡した: {:?}", host.placed);
    let (_, slot) = host.placed.last().unwrap();
    assert!(slot.height() >= renderer::EMBED_MIN_HEIGHT - 0.5, "最小の高さ: {slot:?}");
    assert!(state.rects["diagram-surface"].contains_rect(*slot), "矩形は面の node の中");
    assert!(slot.width() > 200.0, "残りの幅: {slot:?}");
    let diagram_texts: Vec<String> = ax(&out).into_iter().filter(|a| state.rects["diagram-surface"].contains_rect(a.rect)).map(|a| a.text).collect();
    assert!(!diagram_texts.iter().any(|t| t.contains("後の版です")), "置いた面が「後の版です」と言う: {diagram_texts:?}");
    let ids: Vec<String> = out.platform_output.accesskit_update.as_ref().unwrap().nodes.iter().filter_map(|(_, n)| n.author_id().map(str::to_owned)).collect();
    assert!(!ids.iter().any(|id| id.starts_with("web-bar:")), "Diagram に Browser の欄を出さない: {ids:?}");
}

/// W12W-B: Browser の面は題の帯・戻る / 進む / 読み直し・URL 欄の AX（`web-bar:<sid>` 等）を出し、押すと Host に届く。Diagram は出さない。
#[test]
fn w12wb_browser_chrome_has_ax_ids_and_clicks_reach_the_host() {
    let app = synthetic(json!({"key": "root", "component": "column", "children": []}), ready_action(), json!([]));
    let page: domain::project::PageDefinition = serde_json::from_value(json!({
        "id": "page.b", "route": "/b", "title": "Web", "revision": 1,
        "root": {"key": "b-root", "component": "column", "children": [
            {"key": "browser-surface", "component": "source_surface", "label": "Browser", "props": {"surface_ref": "b1"}}
        ]}
    }))
    .unwrap();
    let spec = domain::project::SurfaceSpec {
        id: "b1".into(),
        backend: domain::project::SurfaceBackendKind::WebView,
        target: domain::project::SurfaceTarget::Url { url: "https://www.google.com/".into() },
        keep_alive: true,
        frame: None,
        browser: true,
    };
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut host = Fake { embeds: vec!["b1".into()], urls: HashMap::from([("b1".into(), "https://www.google.com/".into())]), ..Fake::default() };
    let specs = [spec];
    let mut out = None;
    for _ in 0..2 {
        out = Some(h.frame_page(&app, &page, &mut state, &mut host, &specs, vec![]));
    }
    let out = out.unwrap();
    let nodes = out.platform_output.accesskit_update.as_ref().expect("AX の木");
    let id_of = |want: &str| {
        nodes.nodes.iter().find(|(_, n)| n.author_id() == Some(want)).unwrap_or_else(|| panic!("{want} が無い"))
    };
    for want in ["web-bar:b1", "web-back:b1", "web-forward:b1", "web-reload:b1"] {
        let _ = id_of(want);
    }
    assert!(has_text(&out, "Browser"));
    assert!(has_text(&out, "Search or enter website") || host.urls.contains_key("b1"));
    assert!(!host.placed.is_empty(), "WebView の矩形を毎 frame 渡す: {:?}", host.placed);
    assert!(host.placed.iter().all(|(id, _)| id == "b1"), "{:?}", host.placed);
    let bounds = |want: &str| {
        let n = &id_of(want).1;
        let b = n.bounds().unwrap_or_else(|| panic!("{want} に bounds が無い"));
        Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32))
    };
    h.click_page(&app, &page, &mut state, &mut host, &specs, bounds("web-back:b1").center());
    h.click_page(&app, &page, &mut state, &mut host, &specs, bounds("web-forward:b1").center());
    h.click_page(&app, &page, &mut state, &mut host, &specs, bounds("web-reload:b1").center());
    assert_eq!(host.back, vec!["b1".to_string()]);
    assert_eq!(host.forward, vec!["b1".to_string()]);
    assert_eq!(host.reload, vec!["b1".to_string()]);
}
/// W10 holdout（interface の commit b1cc7fe では `terminal_input_bytes` が todo!() で赤）: grid に focus がある間の入力を xterm の byte にする。
#[test]
fn w10_terminal_input_bytes_follow_xterm() {
    use domain::terminal::TermScreen;
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let none = egui::Modifiers::default();
    let plain = TermScreen::default();
    let modes = TermScreen { app_cursor: true, bracketed_paste: true, ..TermScreen::default() };
    let b = |ev: egui::Event, s: &TermScreen| renderer::terminal_input_bytes(&ev, s);
    assert_eq!(b(egui::Event::Text("é".into()), &plain), Some("é".as_bytes().to_vec()));
    assert_eq!(b(egui::Event::Ime(egui::ImeEvent::Commit("日本".into())), &plain), Some("日本".as_bytes().to_vec()));
    assert_eq!(b(key(egui::Key::Enter, none), &plain), Some(b"\r".to_vec()));
    assert_eq!(b(key(egui::Key::Backspace, none), &plain), Some(vec![0x7f]));
    assert_eq!(b(key(egui::Key::Escape, none), &plain), Some(vec![0x1b]));
    assert_eq!(b(key(egui::Key::C, egui::Modifiers::CTRL), &plain), Some(vec![0x03]));
    assert_eq!(b(key(egui::Key::ArrowUp, none), &plain), Some(b"\x1b[A".to_vec()));
    assert_eq!(b(key(egui::Key::ArrowUp, none), &modes), Some(b"\x1bOA".to_vec()));
    assert_eq!(b(egui::Event::Paste("ls".into()), &plain), Some(b"ls".to_vec()));
    assert_eq!(b(egui::Event::Paste("ls".into()), &modes), Some(b"\x1b[200~ls\x1b[201~".to_vec()));
    assert_eq!(b(key(egui::Key::A, none), &plain), None, "文字は Text の側で送る（2 度送らない）");
    assert_eq!(b(key(egui::Key::V, egui::Modifiers::MAC_CMD), &plain), None, "⌘ は Shibubu の short cut");
    let released = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: none };
    assert_eq!(b(released, &plain), None, "離した key は送らない");
}

/// W10: 修飾つきの矢印・Shift+Tab・Ctrl+[ ・Alt+Ctrl・F 鍵・貼り付けの改行と ESC（xterm と alacritty の列）。
#[test]
fn w10_terminal_input_bytes_modifiers_paste_and_function_keys() {
    use domain::terminal::TermScreen;
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let s = TermScreen::default();
    let modes = TermScreen { app_cursor: true, bracketed_paste: true, ..TermScreen::default() };
    let b = |ev: egui::Event, s: &TermScreen| renderer::terminal_input_bytes(&ev, s);
    assert_eq!(b(key(egui::Key::ArrowLeft, egui::Modifiers::CTRL), &modes), Some(b"\x1b[1;5D".to_vec()), "修飾があれば app_cursor でも CSI 1;m");
    assert_eq!(b(key(egui::Key::ArrowRight, egui::Modifiers::SHIFT), &s), Some(b"\x1b[1;2C".to_vec()));
    assert_eq!(b(key(egui::Key::Tab, egui::Modifiers::SHIFT), &s), Some(b"\x1b[Z".to_vec()));
    assert_eq!(b(key(egui::Key::Tab, egui::Modifiers::NONE), &s), Some(b"\t".to_vec()));
    assert_eq!(b(key(egui::Key::OpenBracket, egui::Modifiers::CTRL), &s), Some(vec![0x1b]));
    assert_eq!(b(key(egui::Key::D, egui::Modifiers::CTRL | egui::Modifiers::ALT), &s), Some(vec![0x1b, 0x04]), "Alt は ESC を前に");
    assert_eq!(b(key(egui::Key::Home, egui::Modifiers::NONE), &modes), Some(b"\x1bOH".to_vec()));
    assert_eq!(b(key(egui::Key::PageDown, egui::Modifiers::NONE), &s), Some(b"\x1b[6~".to_vec()));
    assert_eq!(b(key(egui::Key::Delete, egui::Modifiers::SHIFT), &s), Some(b"\x1b[3;2~".to_vec()));
    assert_eq!(b(key(egui::Key::F1, egui::Modifiers::NONE), &s), Some(b"\x1bOP".to_vec()));
    assert_eq!(b(key(egui::Key::F12, egui::Modifiers::NONE), &s), Some(b"\x1b[24~".to_vec()));
    assert_eq!(b(egui::Event::Paste("a\nb\r\nc".into()), &s), Some(b"a\rb\rc".to_vec()), "貼り付けの改行は CR");
    assert_eq!(b(egui::Event::Paste("x\x1b[201~rm".into()), &modes), Some(b"\x1b[200~x[201~rm\x1b[201~".to_vec()), "貼った文の ESC で包みを閉じさせない");
    assert_eq!(b(key(egui::Key::Space, egui::Modifiers::NONE), &s), None, "空白は Text で送る");
    assert_eq!(b(key(egui::Key::Num1, egui::Modifiers::SHIFT), &s), None);
    assert_eq!(b(egui::Event::Text(String::new()), &s), None, "空の Text は送らない");
    assert_eq!(b(egui::Event::Copy, &s), None);
}

/// W10: `screen` の在る Terminal は VT の grid を描く: AX の `terminal:<ref>` は 1 要素で value = `screen.text()`・
/// 描ける大きさ（24 行 × 幅の列）が screen と違えば host に頼む・grid を click した後の打鍵は host の `terminal_keys` に xterm の byte で渡り、
/// 入力欄「Terminal に打つ」には入らない。終わった Terminal は大きさも打鍵も頼まない。
#[test]
fn w10_terminal_grid_draws_screen_resizes_and_sends_keys() {
    use domain::terminal::{TermColor, TermCursor, TermRun, TermScreen, TermStyle};
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let red = TermStyle { fg: TermColor::Indexed(1), ..TermStyle::default() };
    let screen = TermScreen {
        rows: 2,
        cols: 10,
        history: 1,
        lines: vec![vec![TermRun { col: 0, text: "old".into(), style: red }], vec![TermRun { col: 0, text: "% ls 全角".into(), style: TermStyle::default() }], vec![]],
        cursor: Some(TermCursor { row: 0, col: 9 }),
        ..TermScreen::default()
    };
    let pane = renderer::TerminalPane { text: screen.text(), screen: Some(screen.clone()), pid: Some(7), accepts_input: true, status_line: "zsh · pid 7".into(), view: Some(domain::terminal::InputView::Page { page_id: "page.dev".into() }) };
    let h = Harness::new(1440.0);
    // G87: node の選択は開発用（`inspect`）の画面だけが持つ。Esc が選択を外さない事を測るので inspector の在る画面で回す
    let mut state = AppState { inspect: true, ..AppState::default() };
    let mut host = Fake { terminal: Some(pane.clone()), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    let update = out.platform_output.accesskit_update.as_ref().expect("AccessKit の木");
    let grid: Vec<_> = update.nodes.iter().filter(|(_, n)| n.author_id() == Some("terminal:t1")).collect();
    assert_eq!(grid.len(), 1, "terminal:t1 は 1 要素");
    assert_eq!(grid[0].1.value(), Some("old\n% ls 全角\n"), "AX の value は screen.text()");
    let b = grid[0].1.bounds().unwrap();
    let rect = Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32));
    let asked = host.resized.last().cloned().expect("screen と違う大きさを頼む");
    assert_eq!(asked.0.as_str(), "t1");
    assert!(asked.1 > 24, "行数は Page の残りの高さから（W15-c3・前は 24 固定）: {asked:?}");
    assert!(asked.2 > 40 && asked.2 < 400, "列は描ける幅から: {asked:?}");
    assert!(host.keys.is_empty(), "focus の前は送らない");
    h.click(&app, &mut state, &mut host, rect.center());
    let k = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::default() };
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("ls".into()), k(egui::Key::Enter), k(egui::Key::ArrowUp), k(egui::Key::Tab)]);
    // W10 反証: grid に focus の間の Esc は Terminal の物で、app の Esc（選択を外す・overlay を閉じる = renderer::escape_for_app）には使わない
    state.selected = Some("terminal.t1".into());
    h.frame(&app, &mut state, &mut host, vec![k(egui::Key::Escape)]);
    assert_eq!(state.selected.as_deref(), Some("terminal.t1"), "vim の Esc で node の選択が外れた");
    let sent: Vec<u8> = host.keys.iter().flat_map(|(r, b)| { assert_eq!(r, "t1"); b.clone() }).collect();
    assert_eq!(sent, b"ls\r\x1b[A\t\x1b".to_vec(), "Tab と Esc も focus を動かさず Terminal へ");
    state.selected = None;
    assert!(host.sent.is_empty() && state.drafts.get("terminal.t1").is_none(), "入力欄には入らない");
    // 終わった Terminal: 大きさも打鍵も頼まない（画面は最後の物を描く）
    let (asks, typed) = (host.resized.len(), host.keys.len());
    host.terminal = Some(renderer::TerminalPane { accepts_input: false, pid: None, status_line: "終わりました（終了 code 0）。新しく起こせます".into(), ..pane });
    h.settle(&app, &mut state, &mut host);
    h.frame(&app, &mut state, &mut host, vec![egui::Event::Text("x".into())]);
    assert_eq!((host.resized.len(), host.keys.len()), (asks, typed));
}

/// W15-c3（r26 の Dev terminal: 欄が窓の半分で止まり入力欄の下が空いた）: 欄の行数は Page の残りの高さから決まり、欄の下端は窓の下端の近く（TERM-ONE-1101 から入力欄は無い）。
/// 負の対照: `term_rows` を `TERM_ROWS` 固定に戻す → 赤（欄が 24 行で止まり、下が空く）。
#[test]
fn w15c3_terminal_grid_fills_the_page_down_to_the_bottom() {
    use domain::terminal::{TermCursor, TermScreen};
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "terminal.t1", "component": "terminal", "props": {"terminal_ref": "t1"}}]}),
        ready_action(),
        json!([]),
    );
    let pane = |rows: u16, cols: u16| {
        let screen = TermScreen { rows, cols, lines: vec![vec![]; usize::from(rows)], cursor: Some(TermCursor { row: 0, col: 0 }), ..TermScreen::default() };
        renderer::TerminalPane { text: screen.text(), screen: Some(screen), pid: Some(7), accepts_input: true, status_line: "zsh · pid 7".into(), view: Some(domain::terminal::InputView::Drawer) }
    };
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let mut host = Fake { terminal: Some(pane(24, 80)), ..Fake::default() };
    h.settle(&app, &mut state, &mut host);
    let (_, rows, cols) = host.resized.last().cloned().expect("大きさを頼む");
    assert!(rows > 24, "窓の高さ分まで行を頼む: {rows}");
    host.terminal = Some(pane(rows, cols));
    let out = h.settle(&app, &mut state, &mut host);
    let update = out.platform_output.accesskit_update.as_ref().expect("AccessKit の木");
    let rect_of = |n: &egui::accesskit::Node| n.bounds().map(|b| Rect::from_min_max(Pos2::new(b.x0 as f32, b.y0 as f32), Pos2::new(b.x1 as f32, b.y1 as f32))).unwrap();
    let grid = update.nodes.iter().find(|(_, n)| n.author_id() == Some("terminal:t1")).map(|(_, n)| rect_of(n)).expect("grid");
    // ponytail: 48 = 1 行（約 15）＋ grid の枠の余白と Page の下の余白の見込み（未測・cargo の後に実測で詰める）
    assert!(grid.max.y <= 900.0 && 900.0 - grid.max.y < 48.0, "欄の下端 {grid:?} と窓の下端の間が空きすぎ");
}

/// W11: 既定の ActionHost は Agent の入力を送ったと出さず、RunReceipt も空（偽の成功を出さない）。
#[test]
fn w11_default_host_refuses_agent_write_and_has_no_receipts() {
    use domain::terminal::{InputOwner, InputView};
    let mut host = Fake::default();
    let view = InputView::Drawer;
    let err = "No terminal on this page";
    assert_eq!(host.terminal_write("t1", InputOwner::Agent, &view, b"x").unwrap_err(), err);
    host.terminal_touch_human("t1", view.clone()).unwrap();
    host.terminal_release_human("t1", &view).unwrap();
    assert_eq!(host.touched.len(), 1);
    assert_eq!(host.released.len(), 1);
    assert!(host.terminal_lease("t1").is_none());
    assert!(host.run_receipts("project.x").is_empty());
}

/// UI-6a（反証の直し）: 状態の行の「1 pass に 1 回」は node の中身に効かない。run_progress の全体の行が今の段の名前を出し、段の一覧にも同じ段が在っても
/// 段の行は減らない。同じ状態と文の欄が 2 つ（Git の branch と SHA の型）でも 2 つ目の欄の 1 行は消えない。
/// 負の対照（commit message）: `draw_node` の `states::each_its_own` を外す → 赤。
#[test]
fn ui6a_same_line_in_separate_nodes_is_not_merged() {
    let data = json!({"status": "running", "step": "Build", "steps": [{"title": "Checkout", "status": "verified"}, {"title": "Build", "status": "running"}]});
    let root = json!({"key": "root", "component": "column", "children": [
        {"key": "runs", "component": "run_progress", "label": "Runs", "binding": {"source_ref": "local.s", "path": "run_progress"}},
        {"key": "branch", "component": "text", "label": "branch", "binding": {"source_ref": "local.s", "path": "branch"}},
        {"key": "sha", "component": "text", "label": "SHA", "binding": {"source_ref": "local.s", "path": "sha"}},
    ]});
    let app = synthetic(root, json!([]), json!([{"id": "local.s", "kind": "local_state", "readiness": "local"}]));
    let mut host = Fake::default().with("local.s", "run_progress", data);
    for k in ["branch", "sha"] {
        host.regions.insert(k.into(), (RegionState::Error, "Not read yet".into()));
    }
    let mut state = AppState::default();
    let out = Harness::new(1440.0).settle(&app, &mut state, &mut host);
    let texts: Vec<String> = ax(&out).into_iter().map(|a| a.text).collect();
    assert_eq!(texts.iter().filter(|t| t.contains("Build")).count(), 2, "全体の行と同じ段の行を消した: {texts:?}");
    assert_eq!(texts.iter().filter(|t| t.contains("Not read yet")).count(), 2, "2 つ目の欄の 1 行を消した: {texts:?}");
}

/// G88b: 録った後の録音の帯。波形・`Play`・`Add take to REAPER`（押す前に何が起きるかを下に 1 行）・`Discard`。押すと host に手が渡る。
/// 面が Ready でない時は `Add take to REAPER` が押せず理由の 1 行。mic が off なら `Open System Settings`。入れる先が無ければ `Reveal in Finder` だけ。
/// 負の対照（commit message）: `blocked` を見ない → 押せて赤
#[test]
fn g88b_the_recorder_band_plays_adds_and_discards_a_take() {
    let app = synthetic(
        json!({"key": "root", "component": "column", "children": [{"key": "hum-recorder", "component": "recorder", "props": {"label": "Record humming", "max_seconds": 60, "recorder_ref": "hum", "feeds": "reaper"}}]}),
        ready_action(),
        json!([]),
    );
    let h = Harness::new(1440.0);
    let mut state = AppState::default();
    let recorded = |blocked: Option<&str>| renderer::TakePane {
        stage: renderer::TakeStage::Recorded { seconds: 3.0, peaks: vec![0.1, 0.8, 0.3, 0.6] },
        add: Some(renderer::AddTake { app: "REAPER".into(), blocked: blocked.map(str::to_string) }),
        line: String::new(),
    };
    let mut host = Fake { take: Some(recorded(None)), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    for b in ["Play", "Add take to REAPER", "Discard"] {
        assert!(!find(&out, b).disabled, "{b} が押せない");
    }
    find(&out, "Goes to REAPER's cursor on the selected track");
    let add = find(&out, "Add take to REAPER");
    h.click(&app, &mut state, &mut host, add.rect.center());
    assert_eq!(host.take_acts, [("hum".to_owned(), renderer::TakeAct::Add)], "押した手が host に渡らない");

    let mut host = Fake { take: Some(recorded(Some("Open REAPER first"))), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    assert!(find(&out, "Add take to REAPER").disabled, "面が Ready でないのに押せる");
    find(&out, "Open REAPER first");

    let mut host = Fake { take: Some(renderer::TakePane { stage: renderer::TakeStage::MicOff, ..Default::default() }), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    find(&out, "Microphone access is off");
    find(&out, "Open System Settings");

    let mut host = Fake { take: Some(renderer::TakePane { add: None, ..recorded(None) }), ..Fake::default() };
    let out = h.settle(&app, &mut state, &mut host);
    find(&out, "Reveal in Finder");
    assert!(!has_text(&out, "Add take to"), "入れる先が無いのに Add take を出した");
}

/// FX6（FX-031・UIUX G07）: 外へ送る手の button は、押す前に相手と戻せるかの 1 行を持つ。読むだけの手・この Mac の中の手には出さない。
/// 負の対照（commit message）: `gated_button` の `external_effect_line` の呼びを外す → 1 つ目の assert が赤。
#[test]
fn fx6_external_send_says_who_it_goes_to_and_that_it_cannot_be_undone_before_the_button() {
    let make = |effect: &str| {
        let root = json!({"key": "root", "component": "column", "children": [{
            "key": "reply", "component": "form", "label": "Reply", "action_ref": "mail.send",
            "props": {"fields": [{"name": "body", "label": "Body", "input": "multiline"}]},
            "binding": {"source_ref": "inbox", "path": "selected"}
        }]});
        let actions = json!([{"id": "mail.send", "effect": effect, "readiness": "ready", "required_scopes": [],
            "input_schema": {"type": "object", "required": ["body"], "properties": {"body": {"type": "string"}}}}]);
        synthetic(root, actions, json!([{"id": "inbox", "kind": "external_api", "readiness": "ready"}]))
    };
    let shown = |effect: &str| {
        let app = make(effect);
        let mut host = Fake::default().with("inbox", "selected", json!({"body": "hi"}));
        let (h, mut state) = (Harness::new(1440.0), AppState::default());
        let out = h.settle(&app, &mut state, &mut host);
        ax(&out).into_iter().map(|a| a.text).collect::<Vec<_>>()
    };
    let send = shown("external_send");
    assert!(send.iter().any(|t| t.starts_with("Sends to ") && t.contains("Can't be undone")), "送る手の前に相手と戻せるかの 1 行が無い: {send:?}");
    let read = shown("external_read");
    assert!(!read.iter().any(|t| t.starts_with("Sends to ") || t.starts_with("Writes to ")), "読むだけの手に作用の行: {read:?}");
}
