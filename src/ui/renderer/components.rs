//! component ごとの描き方（UI-005）。domain の `BUILTIN_COMPONENTS` 25 種を全部ここで受け、表に無い物は「未対応の部品」の 1 行（CD-11 (3)）。
//! 行の中身は `ActionHost::data` が返した物だけ。無い・繋がっていない・古い時は §11 の状態語（`content`）。
//! card を作らない（DESIGN §3: 行・見出し・余白・1px 区切り）。枠を描くのは media の再生面と waveform の描画面（bounded canvas）だけ。

use crate::domain::icon::Icon;
use super::bindings::{self, Gate};
use super::layout;
use super::{draft_key, draw_node, terminal_input_bytes, visible, Cx, RowPick, TakeAct, TakePane, TakeStage, TerminalPane};
use crate::domain::terminal::{InputView, TermColor, TermRun, TermScreen, TermStyle};
use crate::domain::action::{ActionInvocation, Effect, InvocationOrigin};
use crate::domain::ui::UiNode;
use crate::ui::ax::AxTextInput;
use crate::ui::states::{self, RegionState};
use crate::ui::theme::{self, Tokens, Type, Variant};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Sense, StrokeKind, Vec2};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

pub fn draw(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    match node.component.as_str() {
        "column" => column(ui, node, cx),
        "row" => row(ui, node, cx),
        "split" => blocks(ui, node, cx, layout::ratio(node)),
        "tabs" => tabs(ui, node, cx),
        "text" => text(ui, node, cx),
        "button" => button(ui, node, cx),
        "form" => form(ui, node, cx),
        "input" => input(ui, node, cx),
        "editor" => editor(ui, node, cx),
        "command_input" => {
            let name = heading(ui, node, cx.t);
            composer(ui, node, cx, "Run", name);
        }
        "chat" => chat(ui, node, cx),
        "list" => list(ui, node, cx),
        "table" => {
            heading(ui, node, cx.t);
            if let Some(data) = content(ui, node, cx, true) {
                table(ui, node, cx, &data);
            }
        }
        "chart" => chart(ui, node, cx),
        "detail" => detail(ui, node, cx),
        "calendar" => calendar(ui, node, cx),
        "timeline" => timeline(ui, node, cx),
        "media" => media(ui, node, cx),
        "run_progress" => run_progress(ui, node, cx),
        "recorder" => recorder(ui, node, cx),
        "source_surface" => source_surface(ui, node, cx),
        "waveform" => waveform(ui, node, cx),
        "terminal" => terminal(ui, node, cx),
        "canvas" => canvas(ui, node, cx),
        "extension" => {
            let name = prop_str(node, "name").or(node.label.as_deref()).unwrap_or(&node.key);
            states::inline(ui, RegionState::Partial, &format!("This version can't show “{name}”"));
        }
        other => states::inline(ui, RegionState::Partial, &format!("This version can't show one part ({other})")),
    }
}

// ---------------------------------------------------------------------------
// 共通
// ---------------------------------------------------------------------------

/// 見出し（HeadingSm + 8）。戻り値は入力欄の accessible name に使う label の Id。
fn heading(ui: &mut egui::Ui, node: &UiNode, t: Tokens) -> Option<egui::Id> {
    let label = node.label.as_ref()?;
    let id = theme::label(ui, Type::HeadingSm, label, t.text_primary).id;
    ui.add_space(theme::SP_2);
    Some(id)
}

fn prop_str<'a>(node: &'a UiNode, key: &str) -> Option<&'a str> {
    node.props.get(key).and_then(Value::as_str)
}

fn variant(node: &UiNode) -> Variant {
    match prop_str(node, "variant") {
        Some("primary") => Variant::Primary,
        Some("danger") => Variant::Danger,
        _ => Variant::Ghost,
    }
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "—".into(),
        Value::Bool(true) => "✓".into(),
        Value::Bool(false) => "—".into(),
        other => other.to_string(),
    }
}

/// object なら最初に在る key の文字、object でなければその値の文字。
fn item_text(v: &Value, keys: &[&str]) -> Option<String> {
    match v.as_object() {
        Some(o) => keys.iter().find_map(|k| o.get(*k)).map(cell_text),
        None => Some(cell_text(v)),
    }
}

/// region の状態を描き、中身を描いてよい時だけ data を返す（UI-007）。
/// 中身が無い時: `big` は見出し + 説明（list / table 等の領域）、そうでなければ 1 行。中身を描く時の Ready 以外（一部・古い・実行中・検証済み）は上に 1 行。
/// 行を描かない時は、その node の行の選択も外す（見えていない行を選んだままにしない・`has_selection` を偽に戻す）。
fn content(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>, big: bool) -> Option<Value> {
    let (state, detail, data) = bindings::region(cx.app, node, &*cx.host);
    if state != RegionState::Ready {
        if big && data.is_none() {
            states::region(ui, state, &detail, None);
        } else {
            states::inline(ui, state, &detail);
        }
    }
    if data.is_none() {
        cx.state.rows.remove(&node.key);
    }
    cx.state.regions.insert(node.key.clone(), (state, detail));
    data
}

/// 入力の部品（form / input / editor）の bind 先。`record` は初期値で、draft を付ける相手（bind 先が無い・空なら None）。
struct Bound {
    record: Option<Value>,
    /// 打ってよいか（`bindings::editable`。bind 先の無い部品は常に打てる）。
    editable: bool,
}

/// 入力の部品の bind 先を読む。状態の 1 行は「空」以外の時だけ（入力欄の空は「まだ書いていない」で、知らせる事が無い。
/// 接続が必要・読み込み中・失敗 等は出す）。送った draft のうち、bind 先が送った時から変わった物はここで消す（R2-01）。
/// 見比べるのは bind 先の record が分かっている時だけ（data が在る・空）。確認待ち・読み込み中 等で data が隠れている間に消すと、
/// 承認を断られた・保存が通らなかった（data が送った時のまま戻る）時に、打った文が無くなる。
fn bound(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) -> Bound {
    if node.binding.is_none() {
        return Bound { record: None, editable: true };
    }
    let (state, detail, record, editable) = bindings::input_region(cx.app, node, &*cx.host);
    if !matches!(state, RegionState::Ready | RegionState::Empty) {
        states::inline(ui, state, &detail);
    }
    if record.is_some() || state == RegionState::Empty {
        cx.state.forget_sent(&node.key, record.as_ref());
    }
    cx.state.regions.insert(node.key.clone(), (state, detail));
    Bound { record, editable }
}

fn invoke(cx: &mut Cx<'_>, node: &UiNode, action_ref: String, input: Value) {
    cx.host.invoke(ActionInvocation { action_ref, origin: InvocationOrigin::Button { node_key: node.key.clone() }, input });
}

/// 押せる button か、理由つき disabled（DESIGN §4.1）。`input` は押した時に送る物で、gate が runtime と同じ検査を先にかける。
/// 右 click は inspector で選ぶ（押せる button は左 click が実行なので）。戻り値は invoke したか。
fn gated_button(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>, label: &str, variant: Variant, compact: bool, input: Value) -> bool {
    let draw = |ui: &mut egui::Ui| if compact { theme::button_compact(ui, label, variant) } else { theme::button(ui, label, variant) };
    match bindings::gate(cx.app, node, &*cx.host, &input) {
        Ok(action_ref) => {
            // FX-031: 外に出る手（送る・外へ書く）は押す前に、相手と作用の 1 行（✓ は read-back の後に runtime が付ける）
            if let Some(line) = cx.app.action(&action_ref).and_then(|a| external_effect_line(cx.app, a)) {
                theme::label(ui, Type::Helper, &line, cx.t.text_secondary);
            }
            let response = draw(ui);
            if response.secondary_clicked() && super::inspecting(cx.state) {
                cx.state.selected = Some(node.key.clone());
            }
            if response.clicked() {
                invoke(cx, node, action_ref, input);
                return true;
            }
            false
        }
        // G100 FX-060: 未実装の Action（割り当て無し・定義に無い・この版で未対応）は描かない（押せない形と理由の 1 行は FX-030）。
        // 理由は inspector のために gates に残す
        Err(gate) if unbuilt(cx, node) => {
            cx.state.gates.insert(node.key.clone(), gate);
            false
        }
        Err(gate) => {
            let height = if compact { theme::BUTTON_COMPACT_H } else { theme::BUTTON_H };
            match cx.defer.as_mut() {
                Some(defer) => {
                    theme::button_disabled(ui, label, height, &gate.text());
                    defer.push((label.to_owned(), gate.clone()));
                }
                None => {
                    ui.horizontal_wrapped(|ui| {
                        theme::button_disabled(ui, label, height, &gate.text());
                        gate_line(ui, &gate, cx.t);
                    });
                }
            }
            cx.state.gates.insert(node.key.clone(), gate);
            false
        }
    }
}

/// 外に出る手（`ExternalSend` / `ExternalWrite`）の押す前の 1 行: `Sends to {相手}` / `Writes to {相手}` · 戻せるか（`Undo::conservative` の語）。
/// 相手は action の capability の provider（無ければ `another app`）。読むだけ・この Mac の中の手には出さない。
fn external_effect_line(app: &crate::domain::app::AppDefinitionV3, a: &crate::domain::action::ActionDefinition) -> Option<String> {
    let verb = match a.effect {
        Effect::ExternalSend => "Sends to",
        Effect::ExternalWrite => "Writes to",
        _ => return None,
    };
    let to = a.capability_ref.as_deref().and_then(|c| app.capability(c)).and_then(|c| c.provider.clone()).unwrap_or_else(|| "another app".into());
    Some(format!("{verb} {to} · {}", crate::ui::runs::Undo::conservative(a.effect).wording()))
}

/// 押しても何も起きない（作られていない）Action か: 割り当てが無い・定義に無い・action か capability がこの版で未対応・capability が定義に無い。
/// 未接続・preview・実行中・入力待ち は作られている（押せない形と理由で出す）
fn unbuilt(cx: &Cx<'_>, node: &UiNode) -> bool {
    use crate::domain::action::ActionReadiness;
    use crate::domain::capability::CapabilityReadiness;
    let Some(action) = node.action_ref.as_deref().and_then(|id| cx.app.action(id)) else { return true };
    cx.host.readiness(&action.id).unwrap_or(action.readiness) == ActionReadiness::Unsupported
        || action.capability_ref.as_deref().is_some_and(|c| cx.app.capability(c).is_none_or(|c| c.readiness == CapabilityReadiness::Unsupported))
}

/// 押せない理由の 1 行: §11 の状態が当てはまれば状態語つき、そうでなければ補足の文だけ。
fn gate_line(ui: &mut egui::Ui, gate: &Gate, t: Tokens) {
    match gate.state {
        Some(state) => states::inline(ui, state, &gate.reason),
        None => {
            theme::label(ui, Type::Helper, &gate.reason, t.text_secondary);
        }
    }
}

/// 入力欄の Id（app id + node key + field 名 + bind 先の record）。app を切り替えて戻っても同じ欄は同じ Id。
/// bind 先が別の record に替われば別の欄になる（R2-01: 焦点・cursor・undo・打ちかけの数が前の record の物のまま残らず、
/// 前の record に向けて打っていた続きが次の record の欄に入らない）。record が無い欄の Id は record を含まない。
fn field_id(cx: &Cx<'_>, node_key: &str, field: &str, record: Option<&Value>) -> egui::Id {
    let id = egui::Id::new(("kangae-field", &cx.app.id, node_key, field));
    record.map_or(id, |r| id.with(draft_key(r).to_string()))
}

// ---------------------------------------------------------------------------
// 容器: column / row / split / tabs
// ---------------------------------------------------------------------------

fn column(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let Some(slot) = node.props.get("slot").and_then(|v| v.as_str()) else {
        return column_in(ui, node, cx);
    };
    // W13c: 配置の型の区画（`composer::layout::place` の容れ物）。AX の `slot:<区画>` の group の下に描く。
    // 中の部品の id は draw_node が容れ物の key（slot.<区画>）の下で積むので、型を替えても同じ id の node が別の親へ移らない
    // （移ると macOS の accesskit_consumer が落ちる・W14b の BG。試験 w13c_main_demo_* が frame の前後の親を見る）
    let id = ui.id().with(("slot", slot));
    let ctx = ui.ctx().clone();
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_author_id(crate::ui::ax::AxScope::Slot.id(slot));
    });
    let rect = ctx.with_accessibility_parent(id, || {
        ui.scope(|ui| {
            if slot == "drawer" {
                egui::CollapsingHeader::new("Drawer").id_salt(("slot", slot)).show(ui, |ui| column_in(ui, node, cx));
            } else {
                column_in(ui, node, cx);
            }
        })
        .response
        .rect
    });
    ctx.accesskit_node_builder(id, |n| {
        n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
}

fn column_in(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = theme::SP_4;
        for child in &node.children {
            draw_node(ui, child, cx);
        }
    });
}

/// 文字と button だけなら折り返して並べる（縮めない・UI-006）。塊が混ざれば幅を解く。
/// 文字と button だけの row: button は幅を先に測って行に詰め、入らなければ次の行へ（縮めない・窓の外へ押し出さない・UI-006）。
/// button 以外（文字・run_progress 等）は 1 行を占める。押せない button の理由は row の下に「button 名: 理由」で並べる。
/// 塊（list 等）が混ざる row は幅を解く（`blocks`）。
// ponytail: 自分で折り返すのは button だけ（幅を描く前に測れるのが button だけなので）。文字を横に詰めたくなったら測り方を足す
fn row(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    if !node.children.iter().all(layout::is_inline) {
        blocks(ui, node, cx, None);
        return;
    }
    let gap = theme::SP_2;
    let available = ui.available_width();
    let mut lines: Vec<Vec<&UiNode>> = Vec::new();
    let mut line: Vec<&UiNode> = Vec::new();
    let mut used = 0.0;
    for child in node.children.iter().filter(|c| visible(c, cx.state)) {
        let width = (child.component == "button").then(|| {
            ui.painter().layout_no_wrap(button_label(child).to_owned(), Type::Body.font(), Color32::PLACEHOLDER).size().x + 2.0 * theme::BUTTON_PAD_X
        });
        match width {
            Some(w) if line.is_empty() || used + gap + w <= available => {
                used += if line.is_empty() { w } else { gap + w };
                line.push(child);
            }
            Some(w) => {
                lines.push(std::mem::replace(&mut line, vec![child]));
                used = w;
            }
            None => {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                lines.push(vec![child]);
                used = 0.0;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    let outer = cx.defer.replace(Vec::new());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        for line in lines {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for child in line {
                    draw_node(ui, child, cx);
                }
            });
        }
        for (label, gate) in cx.defer.take().unwrap_or_default() {
            let reason = if gate.reason.is_empty() { label } else { format!("{label}: {}", gate.reason) };
            gate_line(ui, &Gate { reason, ..gate }, cx.t);
        }
    });
    cx.defer = outer;
}

/// 幅を解いて横に並べる。解けなければ縦に畳む（優先度の高い順）。gutter 12（DESIGN §7）。隠れている子は数えない。
fn blocks(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>, ratio: Option<Vec<f32>>) {
    let kids: Vec<(usize, &UiNode)> = node.children.iter().enumerate().filter(|(_, c)| visible(c, cx.state) && !zoom_hides(ui.ctx(), c, cx)).collect();
    if kids.is_empty() {
        return;
    }
    let gutter = theme::SP_3;
    let available = ui.available_width();
    let free = available - gutter * (kids.len() - 1) as f32;
    let ratio_sum: f32 = ratio.as_ref().map(|r| kids.iter().map(|(i, _)| r[*i]).sum()).unwrap_or(1.0);
    let wants: Vec<layout::Want> = kids
        .iter()
        .map(|(i, c)| {
            let share = ratio.as_ref().map(|r| r[*i] / ratio_sum).unwrap_or(1.0 / kids.len() as f32);
            let mut w = layout::want(c, free, share, ratio.is_some());
            // LAYOUT: 列の中の面の service の最小の幅（Web は縮こまらない・desktop app は細くしない）。収まらなければ `solve` が None = 縦に畳む
            w.min = w.min.max(service_min_w(c, cx));
            w.preferred = w.preferred.max(w.min);
            w
        })
        .collect();
    match layout::solve(available, &wants, gutter) {
        Some(widths) => {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gutter;
                for ((_, child), w) in kids.iter().zip(widths) {
                    ui.vertical(|ui| {
                        ui.set_width(w);
                        draw_node(ui, child, cx);
                    });
                }
            });
        }
        None => {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = theme::SP_6;
                for i in layout::collapse_order(&wants) {
                    draw_node(ui, kids[i].1, cx);
                }
            });
        }
    }
}

/// 子の label が tab 名。36 高・下に 2px の indicator（§2.3）。選択は AppState.tabs（app を切り替えても残る）。
fn tabs(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let n = node.children.len();
    if n == 0 {
        states::inline(ui, RegionState::Empty, "");
        return;
    }
    let t = cx.t;
    let current = cx.state.tabs.get(&node.key).copied().unwrap_or(0).min(n - 1);
    let mut next = current;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, child) in node.children.iter().enumerate() {
            let label = child.label.as_deref().unwrap_or(&child.key);
            let galley = ui.painter().layout_no_wrap(label.to_owned(), Type::Nav.font(), Color32::PLACEHOLDER);
            let size = Vec2::new(galley.size().x + 2.0 * theme::SP_4, theme::TAB_H);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, i == current, label));
            if response.clicked() {
                next = i;
            }
            if ui.is_rect_visible(rect) {
                let color = if i == current { t.text_primary } else { t.text_secondary };
                ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
                if i == current {
                    let bar = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - theme::TAB_INDICATOR), Vec2::new(rect.width(), theme::TAB_INDICATOR));
                    ui.painter().rect_filled(bar, 0.0, t.text_primary);
                }
                theme::focus_ring(ui, &response, rect, CornerRadius::same(theme::RADIUS_SM));
            }
        }
    });
    theme::rule(ui);
    ui.add_space(theme::SP_4);
    cx.state.tabs.insert(node.key.clone(), next);
    draw_node(ui, &node.children[next], cx);
}

// ---------------------------------------------------------------------------
// 文字と操作: text / button / form / input / editor / composer（command_input・chat の入力）
// ---------------------------------------------------------------------------

fn text(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let t = cx.t;
    let (kind, color) = match prop_str(node, "style") {
        Some("timer" | "display") => (Type::Display, t.text_primary),
        Some("heading") => (Type::Group, t.text_primary), // UI-SWEEP-1101: Project の Page の群の題も同じ段
        Some("helper" | "caption") => (Type::Helper, t.text_secondary),
        Some("mono") => (Type::Mono, t.text_primary),
        _ => (Type::Body, t.text_primary),
    };
    if node.binding.is_some() {
        // 値は data からだけ（timer の残り時間などを作らない）。label は値の名前。
        if let Some(label) = &node.label {
            theme::label(ui, Type::Nav, label, t.text_secondary);
        }
        // 値は `data:<key>` の中（W12a: 試験の道具が値を引く所・状態の 1 行は content が外に描く）。値が無くても入れ物は出す（空を測れる）
        let v = content(ui, node, cx, false);
        super::data_scope(ui, &node.key, |ui| {
            if let Some(v) = v {
                theme::label(ui, kind, cell_text(&v), color);
            }
        });
        return;
    }
    theme::label(ui, kind, prop_str(node, "text").or(node.label.as_deref()).unwrap_or(""), color);
}

fn button_label(node: &UiNode) -> &str {
    node.label.as_deref().or(prop_str(node, "label")).unwrap_or(&node.key)
}

fn button(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    gated_button(ui, node, cx, button_label(node), variant(node), false, json!({}));
}

/// form: props.fields[] = { name, label, input: text | multiline | integer | number | select(options) | toggle }。
/// 初期値は bind 先の data（object の同名 key）、編集した値は今の bind 先の record に付いた draft（R2-01: bind 先が別の record に
/// 替われば、その record の draft か data の値を出して送る。前の record で打った文は前の record に残り、戻れば戻る）。
/// 送るのは宣言した欄に今見えている値だけ（空の欄は送らない）。bind 先 data の、欄の無い key は送らない（R-SEC-1）。
/// 送ったら draft に送った時の data を記し、data がそこから変われば（反映された）draft を消して data を出す。
fn form(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    ui.spacing_mut().item_spacing.y = theme::SP_3; // 関連する欄の間は 12（§2.2）
    let bound = bound(ui, node, cx);
    let mut input = Map::new();
    for f in node.props.get("fields").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object) {
        let name = f.get("name").and_then(Value::as_str).unwrap_or("value");
        if let Some(v) = field(ui, &node.key, f, bound.record.as_ref().and_then(|r| r.get(name)), &bound, cx) {
            input.insert(name.to_owned(), v);
        }
    }
    let effect = node.action_ref.as_deref().and_then(|id| cx.app.action(id)).map(|a| a.effect);
    let submit = prop_str(node, "submit").unwrap_or(match effect {
        Some(Effect::ExternalSend) => "Send",
        Some(Effect::LocalWrite | Effect::ExternalWrite) => "Save",
        Some(Effect::Destructive) => "Delete",
        _ => "Run",
    });
    if gated_button(ui, node, cx, submit, Variant::Ghost, true, Value::Object(input)) {
        cx.state.mark_sent(&node.key, bound.record.as_ref());
        // W2b: 相手の record の無い「足す」form は、runtime が通した（region が無い）時だけ欄を空に戻す
        if bound.record.is_none() && cx.host.region(&node.key).is_none() {
            cx.state.clear_entry_after_save(&node.key);
        }
    }
}

/// input: props 自体が 1 つの field（name / label / input）。label が無ければ node.label。初期値は bind 先の data そのもの。
fn input(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let mut f = node.props.clone();
    if let (false, Some(label)) = (f.contains_key("label"), &node.label) {
        f.insert("label".into(), Value::String(label.clone()));
    }
    let bound = bound(ui, node, cx);
    field(ui, &node.key, &f, bound.record.as_ref(), &bound, cx);
}

/// 1 つの入力欄。見出しの label を accessible name にする（空欄でも AX で名前が付く）。値は今の bind 先の draft → 無ければ初期値。
/// 打てない状態（`bindings::editable`）の間は打たせない。戻り値は欄に今見えている値（空なら None）。
/// form はこれだけを送る（見せる値と送る値を同じにする・R1-05）。
fn field(ui: &mut egui::Ui, node_key: &str, f: &Map<String, Value>, initial: Option<&Value>, bound: &Bound, cx: &mut Cx<'_>) -> Option<Value> {
    let name = f.get("name").and_then(Value::as_str).unwrap_or("value");
    let label_text = f.get("label").and_then(Value::as_str).unwrap_or(name);
    let kind = f.get("input").and_then(Value::as_str).unwrap_or("text");
    let record = bound.record.as_ref();
    let id = field_id(cx, node_key, name, record);
    let width = ui.available_width().min(theme::COMMAND_MAX_W);
    let margin = theme::input_margin(ui, Type::Body, theme::INPUT_H);
    let current = cx.state.draft(node_key, record).and_then(|d| d.get(name)).or(initial).and_then(|v| shown(kind, v));
    let edited = ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = theme::SP_2; // label と入力欄の間は 8（§2.2）
        let label = theme::label(ui, Type::Nav, label_text, cx.t.text_secondary).id;
        ui.add_enabled_ui(bound.editable, |ui| field_input(ui, kind, f, id, label, width, margin, current.clone())).inner
    });
    match edited.inner {
        Some(v) => {
            let now = shown(kind, &v);
            cx.state.draft_mut(node_key, record).insert(name.to_owned(), v);
            now
        }
        None => current,
    }
}

/// その欄が見せられる値か（見せられない形・空は None = 欄は空で、送らない）。data の値を欄の形に合わせて作り替えない。
fn shown(kind: &str, v: &Value) -> Option<Value> {
    let fits = match kind {
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "toggle" | "boolean" | "checkbox" => v.is_boolean(),
        "select" => v.as_str().is_some_and(|s| !s.is_empty()),
        _ => match v {
            Value::String(s) => !s.is_empty(),
            other => other.is_number() || other.is_boolean(),
        },
    };
    fits.then(|| v.clone())
}

/// 入力欄の本体。変わった時だけ draft に入れる新しい値（null = 値なし: 空にした・数として読めない）。
/// 値が無い間は 0 / off を作らない（数は空欄に「未入力」、on/off はどちらでもない形・R1-05）。
#[allow(clippy::too_many_arguments)]
fn field_input(ui: &mut egui::Ui, kind: &str, f: &Map<String, Value>, id: egui::Id, label: egui::Id, width: f32, margin: egui::Margin, current: Option<Value>) -> Option<Value> {
    match kind {
        "multiline" => {
            let mut s = current.as_ref().map(cell_text).unwrap_or_default();
            let edit = egui::TextEdit::multiline(&mut s).id(id).desired_width(width).desired_rows(3).font(Type::Body.font()).margin(margin);
            ui.add(edit).labelled_by(label).ax_text_input(ui, &mut s).changed().then_some(Value::String(s))
        }
        "integer" | "number" => {
            // 打っている途中の文字（"-" や "1."）は焦点のある間だけ egui の memory に置く。draft には数として読めた時だけ数、他は null
            let focused = ui.memory(|m| m.has_focus(id));
            let typed = ui.data(|d| d.get_temp::<String>(id)).filter(|_| focused);
            let mut s = typed.unwrap_or_else(|| current.as_ref().map(Value::to_string).unwrap_or_default());
            let edit = egui::TextEdit::singleline(&mut s).id(id).desired_width(width).font(Type::Body.font()).margin(margin);
            let response = ui.add_sized([width, theme::INPUT_H], edit).labelled_by(label).ax_text_input(ui, &mut s);
            if response.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, s.clone()));
            } else {
                ui.data_mut(|d| d.remove::<String>(id));
            }
            let t = s.trim();
            let parsed = if kind == "integer" { t.parse::<i64>().ok().map(Value::from) } else { t.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number) };
            response.changed().then(|| parsed.unwrap_or(Value::Null))
        }
        "toggle" | "boolean" | "checkbox" => {
            let mut b = current.as_ref().and_then(Value::as_bool).unwrap_or(false);
            ui.add(egui::Checkbox::new(&mut b, "").indeterminate(current.is_none())).labelled_by(label).changed().then_some(Value::Bool(b))
        }
        "select" => {
            let options: Vec<String> = f.get("options").and_then(Value::as_array).into_iter().flatten().map(cell_text).collect();
            let before = english_option(current.as_ref().and_then(Value::as_str).unwrap_or(""), &options);
            let mut chosen = before.clone();
            egui::ComboBox::from_id_salt(id)
                .width(width)
                .selected_text(if chosen.is_empty() { "Choose" } else { chosen.as_str() })
                .show_ui(ui, |ui| {
                    for o in &options {
                        ui.selectable_value(&mut chosen, o.clone(), o);
                    }
                })
                .response
                .labelled_by(label);
            (chosen != before).then_some(Value::String(chosen))
        }
        _ => {
            let mut s = current.as_ref().map(cell_text).unwrap_or_default();
            let edit = egui::TextEdit::singleline(&mut s).id(id).desired_width(width).font(Type::Body.font()).margin(margin);
            ui.add_sized([width, theme::INPUT_H], edit).labelled_by(label).ax_text_input(ui, &mut s).changed().then_some(Value::String(s))
        }
    }
}

/// 保存済みの日本語の選択肢の値（FX-032 の前に作った定義・data）を、今の英語の選択肢に読み替えて見せる（data は書き換えない）。今の選択肢に無い値はそのまま。
fn english_option(value: &str, options: &[String]) -> String {
    const LEGACY: &[(&str, &str)] = &[
        ("食費", "Food"), ("日用品", "Household"), ("交通", "Transport"), ("住まい", "Housing"), ("娯楽", "Leisure"), ("収入", "Income"), ("その他", "Other"),
        ("読みたい", "Want to read"), ("読んでいる", "Reading"), ("読んだ", "Read"), ("見たい", "Want to watch"), ("見ている", "Watching"), ("見た", "Watched"),
        ("高", "High"), ("中", "Medium"), ("低", "Low"), ("未対応", "Open"), ("対応中", "In progress"), ("解決済み", "Resolved"),
        ("未着手", "Not started"), ("進行中", "In progress"), ("待ち", "Waiting"), ("完了", "Done"),
    ];
    if options.iter().any(|o| o == value) {
        return value.to_owned();
    }
    LEGACY.iter().find(|(ja, en)| *ja == value && options.iter().any(|o| o == en)).map_or_else(|| value.to_owned(), |(_, en)| (*en).to_owned())
}

/// editor: 複数行。初期値は bind 先の data（文字列）、以後は今の bind 先に付いた draft（R2-01: bind 先が替われば、その中身を出す）。
/// bind 先が使えない時は状態語を出して編集させない。
// ponytail: 文字列に bind した editor（selected.body 等）の相手は文字列そのもの。本文が同じ 2 つの record（空の新しいノート 2 つ）は
// 見分けられない（draft がもう片方にも出る）。editor の文を送る action は今の v3 に無いので、送り先を持つ時に親の record の id で付ける
fn editor(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let label = heading(ui, node, cx.t);
    let bound = bound(ui, node, cx);
    let record = bound.record.as_ref();
    let id = field_id(cx, &node.key, "text", record);
    let margin = theme::input_margin(ui, Type::Body, theme::INPUT_H);
    let typed = cx.state.draft(&node.key, record).and_then(|d| d.get("text")).and_then(Value::as_str);
    let mut s = typed.or(record.and_then(Value::as_str)).unwrap_or("").to_owned();
    let edit = egui::TextEdit::multiline(&mut s).id(id).desired_width(f32::INFINITY).desired_rows(6).font(Type::Body.font()).margin(margin);
    let response = ui.add_enabled(bound.editable, edit);
    let response = match label {
        Some(l) => response.labelled_by(l),
        None => theme::ax_name(response, "Description"),
    }
    .ax_text_input(ui, &mut s);
    if response.changed() {
        cx.state.draft_mut(&node.key, record).insert("text".into(), Value::String(s));
    }
}

/// 1 行の入力 + 送る button（command_input / chat）。Enter か button で `{"text": ...}` を invoke し、送ったら draft を消す。
/// 入力は bind 先の record に付かない（chat の bind 先は会話の行で、行が増えても打ちかけを動かさない）: draft の record は None。
/// gate には text を空でも渡す（空の欄に「必須の text が無い」を常に出さない。空のまま押しても送らないのは下の条件）。
fn composer(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>, send_label: &str, name: Option<egui::Id>) {
    let t = cx.t;
    let id = field_id(cx, &node.key, "text", None);
    let margin = theme::input_margin(ui, Type::Body, theme::INPUT_H);
    let hint = prop_str(node, "placeholder").unwrap_or("").to_owned();
    let mut s = cx.state.draft(&node.key, None).and_then(|d| d.get("text")).and_then(Value::as_str).unwrap_or("").to_owned();
    let gate = bindings::gate(cx.app, node, &*cx.host, &json!({ "text": s.trim() }));
    let (mut send, mut changed) = (false, false);
    // 高さを 1 行（38）に縛る: `with_layout(right_to_left(Center))` は残りの高さを全部取り、入力欄を縦の真ん中へ落として後ろの部品を押し出す
    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), theme::INPUT_H), Layout::right_to_left(Align::Center), |ui| {
        match &gate {
            Ok(_) => send = theme::button_sized(ui, send_label, Variant::Primary, None, theme::INPUT_H).clicked(),
            Err(g) => {
                theme::button_disabled(ui, send_label, theme::INPUT_H, &g.text());
            }
        }
        let edit = egui::TextEdit::singleline(&mut s)
            .id(id)
            .desired_width(ui.available_width())
            .font(Type::Body.font())
            .hint_text(Type::Body.text(hint.as_str()).color(t.text_tertiary))
            .margin(margin);
        let r = ui.add_sized([ui.available_width(), theme::INPUT_H], edit);
        // 見出しの無い欄も名前を持つ（placeholder、無ければ送る button の語）
        let r = match name {
            Some(name) => r.labelled_by(name),
            None => theme::ax_name(r, if hint.is_empty() { send_label } else { &hint }),
        }
        .ax_text_input(ui, &mut s);
        send |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        changed = r.changed();
    });
    if changed {
        cx.state.draft_mut(&node.key, None).insert("text".into(), Value::String(s.clone()));
    }
    match gate {
        Ok(action_ref) if send && !s.trim().is_empty() => {
            invoke(cx, node, action_ref, json!({ "text": s.trim() }));
            cx.state.drafts.remove(&node.key);
        }
        Err(g) => {
            gate_line(ui, &g, t);
            cx.state.gates.insert(node.key.clone(), g);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// data を映す: chat / list / table / chart / detail / calendar / timeline / media / run_progress / waveform
// ---------------------------------------------------------------------------

/// chat: 本文 ≤760・author 行は在る時だけ・source を見せる（別 service が混ざる時の badge・DESIGN §4.3）。
fn chat(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let name = heading(ui, node, cx.t);
    let t = cx.t;
    if let Some(data) = content(ui, node, cx, true) {
        for m in data.as_array().into_iter().flatten() {
            ui.vertical(|ui| {
                ui.set_max_width(760.0);
                ui.spacing_mut().item_spacing.y = theme::SP_1;
                let meta: Vec<String> = [item_text(m, &["author", "from", "sender"]), item_text(m, &["source"]), item_text(m, &["at", "time", "date"])]
                    .into_iter()
                    .flatten()
                    .collect();
                if !meta.is_empty() && m.is_object() {
                    theme::label(ui, Type::Helper, meta.join(" · "), t.text_secondary);
                }
                theme::label(ui, Type::Body, item_text(m, &["text", "body", "message"]).unwrap_or_else(|| cell_text(m)), t.text_primary);
            });
            ui.add_space(theme::SP_3);
        }
    }
    if node.action_ref.is_some() {
        composer(ui, node, cx, "Send", name);
    }
}

/// list: 行 40・hover は surface.2・選択は左 2px の bar（§4.3）。行を押すと AppState.rows（has_selection の元）。
// ponytail: 全行を毎 frame 描く。100 行を超える list は §4.3 の virtualize（ScrollArea::show_rows）に上げる
/// W2b（読書記録の一覧に著者が出ない・docs/evidence/W2b/log.md 2026-10-20）。原因: 読書記録（記録の job）の一覧は list でなく timeline で、
/// composer は欄を `props.fields` に入れ、renderer の `timeline` は題名しか出さなかった。今は list・timeline とも `props.columns` をこの関数で出す。
///
/// 契約: 行の右の補足 = `source`（在れば）+ `columns` の 2 つ目以降の欄の値を、columns の順に " · " で繋いだ物。
/// - 値が無い・null・空文字の欄は飛ばす（「—」を並べない）。題名に使った欄は繰り返さない
/// - `columns` が無い list は今の決め打ちの key のまま（corpus の v3 例を変えない）
/// - 返す文は見せる文だけ（id・`*_id` の欄は出さない）
/// 例: columns ["title", "author"]・行 {"title": "吾輩は猫である", "author": "夏目漱石"} → "夏目漱石"
fn list_meta(item: &Value, columns: &[String]) -> String {
    let source = item_text(item, &["source"]);
    let rest = columns.iter().skip(1).filter(|c| *c != "id" && !c.ends_with("_id")).filter_map(|c| item.get(c)).filter(|v| !v.is_null()).map(cell_text);
    source.into_iter().chain(rest).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

fn list(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let data = content(ui, node, cx, true);
    // 行は `data:<key>` の中（W12a: 試験の道具が行の名前を引く所）。行が無くても入れ物は出す（0 行を測れる）
    super::data_scope(ui, &node.key, |ui| {
        if let Some(data) = data {
            list_rows(ui, node, cx, data);
        }
    });
}

/// list の行。`props.columns` が在れば先頭の欄が行の名前（W12a の変更 file は `path`）、無ければ決め打ちの key（corpus の v3 例のまま）。
fn list_rows(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>, data: Value) {
    let t = cx.t;
    let items: Vec<Value> = match data {
        Value::Array(a) => a,
        other => vec![other],
    };
    let selected = selected_row(cx, &node.key, &items);
    let columns: Vec<String> = node.props.get("columns").and_then(Value::as_array).into_iter().flatten().map(cell_text).collect();
    let mut clicked = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    theme::rule(ui);
    for (i, item) in items.iter().enumerate() {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::LIST_ROW_H), Sense::click());
        let title = columns
            .first()
            .and_then(|c| item.get(c))
            .filter(|v| !v.is_null())
            .map(cell_text)
            .or_else(|| item_text(item, &["title", "name", "label", "text", "subject"]))
            .unwrap_or_else(|| cell_text(item));
        let active = selected == Some(i);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &title));
        if response.clicked() {
            clicked = Some(i);
        }
        if ui.is_rect_visible(rect) {
            theme::row_background(ui, rect, response.hovered(), active);
            theme::focus_ring(ui, &response, rect, CornerRadius::same(theme::RADIUS_SM));
            let meta = if !item.is_object() {
                String::new()
            } else if columns.is_empty() {
                [item_text(item, &["source"]), item_text(item, &["subtitle", "meta", "status", "from", "at", "date"])].into_iter().flatten().collect::<Vec<_>>().join(" · ")
            } else {
                list_meta(item, &columns)
            };
            let meta_w = if meta.is_empty() { 0.0 } else { 220.0_f32.min(rect.width() * 0.35) };
            let x0 = rect.left() + theme::SP_3;
            let g = theme::truncated(ui, &title, Type::Body.font(), t.text_primary, (rect.right() - meta_w - theme::SP_3 - x0).max(40.0));
            ui.painter().galley(egui::pos2(x0, rect.center().y - g.size().y / 2.0), g, t.text_primary);
            if !meta.is_empty() {
                let g = theme::truncated(ui, &meta, Type::Helper.font(), t.text_tertiary, meta_w);
                ui.painter().galley(egui::pos2(rect.right() - theme::SP_3 - g.size().x, rect.center().y - g.size().y / 2.0), g, t.text_tertiary);
            }
            let line = egui::Rect::from_min_size(rect.left_bottom() - Vec2::new(0.0, theme::HAIRLINE), Vec2::new(rect.width(), theme::HAIRLINE));
            ui.painter().rect_filled(line, 0.0, t.border);
        }
    }
    toggle_row(cx, &node.key, &items, selected, clicked);
}

/// 選んだ record が今の行の何番目か（R1-03: 行番号では持たない。先頭に行が増えても別の record に移らない）。
/// 今の行に無ければ選択を外す。在れば中身を今の行に差し替える。
/// 鍵で探す選択（`RowPick::by_key`）は、鍵が 1 行だけに当たればその行（中身が変わっても追う）。2 行以上に当たれば鍵は外部 key
/// だったので、その中から中身全体が同じ行を探し、以後は中身全体で探す（R2-02: 同じ親を持つ行を 1 行に潰さず、消えた行の選択を兄弟へ移さない）。
// ponytail: 鍵を持たない・鍵が行を決めない行は、中身が 1 つ変わると別の record 扱い（選択が外れる）。runtime が行に id を付ければ保たれる
fn selected_row(cx: &mut Cx<'_>, key: &str, items: &[Value]) -> Option<usize> {
    let pick = cx.state.rows.get_mut(key)?;
    let found = if pick.by_key {
        let keys = record_keys(&pick.record);
        let hits: Vec<usize> = (0..items.len()).filter(|&i| record_keys(&items[i]) == keys).collect();
        match hits[..] {
            [i] => Some(i),
            [] => None,
            _ => {
                pick.by_key = false;
                twins(items, &pick.record).nth(pick.nth)
            }
        }
    } else {
        twins(items, &pick.record).nth(pick.nth)
    };
    match found {
        Some(i) => {
            pick.record = items[i].clone();
            pick.nth = twins(items, &items[i]).position(|j| j == i).unwrap_or(0);
        }
        None => {
            cx.state.rows.remove(key);
        }
    }
    found
}

/// 中身全体が `record` と同じ行の番号（前から）。中身まで同じ双子は、押した行が双子の何番目かで区別する（R3-02）。
fn twins<'a>(items: &'a [Value], record: &'a Value) -> impl Iterator<Item = usize> + 'a {
    items.iter().enumerate().filter(move |(_, item)| *item == record).map(|(i, _)| i)
}

/// 行を指す鍵: `id`（null でない）が在ればそれだけ（`*_id` は外部 key のことがある）、無ければ null でない `*_id` の組。どちらも無ければ空。
fn record_keys(v: &Value) -> Vec<(&str, &Value)> {
    let Some(o) = v.as_object() else { return Vec::new() };
    match o.get("id").filter(|id| !id.is_null()) {
        Some(id) => vec![("id", id)],
        None => o.iter().filter(|(k, v)| k.ends_with("_id") && !v.is_null()).map(|(k, v)| (k.as_str(), v)).collect(),
    }
}

/// 行を押した: 選んでいる行なら外し、別の行ならその行を選ぶ。鍵で追うのは、鍵が今の行の 1 行だけを指す時だけ（R2-02）。
fn toggle_row(cx: &mut Cx<'_>, key: &str, items: &[Value], selected: Option<usize>, clicked: Option<usize>) {
    match clicked {
        Some(i) if selected == Some(i) => {
            cx.state.rows.remove(key);
        }
        Some(i) => {
            let keys = record_keys(&items[i]);
            let by_key = !keys.is_empty() && items.iter().filter(|item| record_keys(item) == keys).count() == 1;
            let nth = twins(items, &items[i]).position(|j| j == i).unwrap_or(0);
            cx.state.rows.insert(key.to_owned(), RowPick { record: items[i].clone(), by_key, nth });
        }
        None => {}
    }
}

/// table: 見出し行 40・数値列は右寄せ・zebra 無し（§4.3）。列の min の和が幅を超えたら横 scroll と 1 行の説明（UI-006: overflow を明示）。
fn table(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>, data: &Value) {
    let t = cx.t;
    let rows: Vec<Value> = data.as_array().into_iter().flatten().filter(|r| r.is_object()).cloned().collect();
    let mut columns: Vec<String> = node.props.get("columns").and_then(Value::as_array).into_iter().flatten().map(cell_text).collect();
    if columns.is_empty() {
        columns = rows.first().and_then(Value::as_object).map(|r| r.keys().cloned().collect()).unwrap_or_default();
    }
    if columns.is_empty() {
        states::inline(ui, RegionState::Partial, "The table has no columns yet");
        return;
    }
    let numeric: HashSet<String> = node.props.get("numeric_columns").and_then(Value::as_array).into_iter().flatten().map(cell_text).collect();
    let mut widths: Vec<f32> = columns
        .iter()
        .map(|c| (ui.painter().layout_no_wrap(c.clone(), Type::Helper.font(), Color32::PLACEHOLDER).size().x + 2.0 * theme::SP_3).max(layout::COLUMN_MIN_W))
        .collect();
    let available = ui.available_width();
    let overflow = layout::table_overflows(available, &widths);
    if overflow {
        states::inline(ui, RegionState::Partial, &format!("Scroll sideways to see all {}", crate::domain::plural(columns.len(), "column")));
    } else {
        let total: f32 = widths.iter().sum();
        for w in widths.iter_mut() {
            *w *= available / total;
        }
    }
    let selected = selected_row(cx, &node.key, &rows);
    let mut clicked = None;
    let mut body = |ui: &mut egui::Ui| {
        let width = widths.iter().sum::<f32>();
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, theme::LIST_ROW_H), Sense::hover());
        let mut x = rect.left();
        for (c, w) in columns.iter().zip(&widths) {
            let g = theme::truncated(ui, c, Type::Helper.font(), t.text_secondary, w - 2.0 * theme::SP_3);
            let gx = if numeric.contains(c) { x + w - theme::SP_3 - g.size().x } else { x + theme::SP_3 };
            ui.painter().galley(egui::pos2(gx, rect.center().y - g.size().y / 2.0), g, t.text_secondary);
            x += w;
        }
        theme::rule(ui);
        for (i, row) in rows.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, theme::LIST_ROW_H), Sense::click());
            let active = selected == Some(i);
            let first = columns.first().and_then(|c| row.get(c)).map(cell_text).unwrap_or_default();
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &first));
            if response.clicked() {
                clicked = Some(i);
            }
            if ui.is_rect_visible(rect) {
                theme::row_background(ui, rect, response.hovered(), active);
                let mut x = rect.left();
                for (c, w) in columns.iter().zip(&widths) {
                    let text = row.get(c).map(cell_text).unwrap_or_else(|| "—".into());
                    let g = theme::truncated(ui, &text, Type::Body.font(), t.text_primary, w - 2.0 * theme::SP_3);
                    let gx = if numeric.contains(c) { x + w - theme::SP_3 - g.size().x } else { x + theme::SP_3 };
                    ui.painter().galley(egui::pos2(gx, rect.center().y - g.size().y / 2.0), g, t.text_primary);
                    x += w;
                }
                let line = egui::Rect::from_min_size(rect.left_bottom() - Vec2::new(0.0, theme::HAIRLINE), Vec2::new(rect.width(), theme::HAIRLINE));
                ui.painter().rect_filled(line, 0.0, t.border);
            }
        }
    };
    if overflow {
        egui::ScrollArea::horizontal()
            .id_salt("kangae-table-x")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| body(ui));
    } else {
        body(ui);
    }
    toggle_row(cx, &node.key, &rows, selected, clicked);
}

/// chart: 絵だけで伝えない（§9「表か文字の代替を必ず」）。この版は代替の表（object の行）か値の並びで出す。
fn chart(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let Some(data) = content(ui, node, cx, true) else { return };
    theme::label(ui, Type::Helper, "Showing the chart's values as a table", cx.t.text_secondary);
    if data.as_array().is_some_and(|a| a.iter().all(Value::is_object)) {
        table(ui, node, cx, &data);
    } else {
        for v in data.as_array().into_iter().flatten() {
            theme::label(ui, Type::Body, cell_text(v), cx.t.text_primary);
        }
    }
}

/// detail: title + metadata 1 行 + 項目（key と値の行）。項目ごとの panel を作らない（§4.3）。
fn detail(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let t = cx.t;
    let Some(data) = content(ui, node, cx, true) else { return };
    let data = match data {
        Value::Array(a) => a.into_iter().next().unwrap_or(Value::Null),
        other => other,
    };
    let Some(obj) = data.as_object() else {
        theme::label(ui, Type::Body, cell_text(&data), t.text_primary);
        return;
    };
    const TITLE: [&str; 4] = ["title", "name", "subject", "id"];
    const META: [&str; 5] = ["status", "updated_at", "at", "source", "from"];
    if let Some(title) = item_text(&data, &TITLE) {
        theme::label(ui, Type::HeadingMd, title, t.text_primary);
    }
    let meta: Vec<String> = META.iter().filter_map(|k| obj.get(*k)).map(cell_text).collect();
    if !meta.is_empty() {
        theme::label(ui, Type::Helper, meta.join(" · "), t.text_secondary);
    }
    ui.add_space(theme::SP_4);
    let key_w = 160.0_f32.min(ui.available_width() * 0.3);
    for (k, v) in obj.iter().filter(|(k, _)| !TITLE.contains(&k.as_str()) && !META.contains(&k.as_str())) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(key_w);
                theme::label(ui, Type::Helper, k, t.text_secondary);
            });
            theme::label(ui, Type::Body, cell_text(v), t.text_primary);
        });
        ui.add_space(theme::SP_2);
    }
}

/// calendar: 日付（start の先頭 10 文字）ごとの一覧。月の格子はこの版では一覧で代わりにする（その旨を 1 行）。
fn calendar(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let t = cx.t;
    let Some(data) = content(ui, node, cx, true) else { return };
    if prop_str(node, "mode") == Some("month") {
        theme::label(ui, Type::Helper, "Showing the month as a list by date", t.text_secondary);
    }
    let mut days: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for e in data.as_array().into_iter().flatten() {
        let start = item_text(e, &["start", "date", "at"]).unwrap_or_default();
        let day = start.get(..10).unwrap_or("No date").to_owned();
        days.entry(day).or_default().push(e);
    }
    for (day, events) in days {
        theme::label(ui, Type::Nav, day, t.text_secondary);
        for e in events {
            ui.horizontal(|ui| {
                let start = item_text(e, &["start", "at"]).unwrap_or_default();
                ui.vertical(|ui| {
                    ui.set_width(56.0);
                    theme::label(ui, Type::Mono, start.get(11..16).unwrap_or("—"), t.text_tertiary);
                });
                theme::label(ui, Type::Body, item_text(e, &["title", "summary", "name"]).unwrap_or_else(|| cell_text(e)), t.text_primary);
            });
        }
        ui.add_space(theme::SP_3);
    }
}

/// timeline: 段階の縦並び。status が run の語なら状態語（glyph つき）、無ければ •。
///
/// 契約（W2b）: `props.columns`（composer `layout.rs` の `columns`・先頭が題名）が在れば、各行の題名の後ろに `list_meta` の文
/// （2 つ目以降の欄の値を " · " で繋いだ物）を出す。例: 読書記録の行 → "• 吾輩は猫である · 夏目漱石 · 読んだ"。columns が無い timeline は今のまま。
fn timeline(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let t = cx.t;
    let Some(data) = content(ui, node, cx, true) else { return };
    let columns: Vec<String> = node.props.get("columns").and_then(Value::as_array).into_iter().flatten().map(cell_text).collect();
    for e in data.as_array().into_iter().flatten() {
        let title = item_text(e, &["title", "label", "name", "text"]).unwrap_or_else(|| cell_text(e));
        let meta = Some(list_meta(e, &columns)).filter(|m| e.is_object() && !m.is_empty()).map(|m| format!(" · {m}")).unwrap_or_default();
        let at = item_text(e, &["at", "time", "date"]).filter(|_| e.is_object()).map(|a| format!("{meta} · {a}")).unwrap_or(meta);
        match bindings::parse_run_status(e) {
            Some(status) => states::inline(ui, bindings::run_state(status).0, &format!("{title}{at}")),
            None => {
                theme::label(ui, Type::Body, format!("• {title}{at}"), t.text_primary);
            }
        }
    }
}

/// 再生面の枠（data が在る時だけ。空の黒い面は出さない・§4.4）。
fn placeholder(ui: &mut egui::Ui, t: Tokens, height: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(theme::RADIUS_MD), t.surface_2, theme::hairline(&t), StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, Type::Body.font(), t.text_secondary);
}

/// media: data が在れば再生面（16:9・96〜240 高）、無ければ状態の 1 行。操作は action の button（再生 / 一時停止 等）。
fn media(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    if let Some(data) = content(ui, node, cx, false) {
        let height = (ui.available_width() * 9.0 / 16.0).clamp(96.0, 240.0);
        placeholder(ui, cx.t, height, &item_text(&data, &["title", "name", "url"]).unwrap_or_else(|| cell_text(&data)));
    }
    if node.action_ref.is_some() {
        let label = prop_str(node, "label").unwrap_or(if prop_str(node, "mode") == Some("playback") { "Play / Pause" } else { "Open" });
        gated_button(ui, node, cx, label, Variant::Ghost, true, json!({}));
    }
}

/// run_progress: data `{status, detail|step|message, steps[]}`。status は domain の RunStatus（snake_case）。読めない語は「一部のみ表示」で原文を出す。
fn run_progress(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let Some(data) = content(ui, node, cx, false) else { return };
    let line = |ui: &mut egui::Ui, v: &Value, name: String| {
        let (state, extra) = match bindings::parse_run_status(v) {
            Some(status) => bindings::run_state(status),
            None => (RegionState::Partial, ""),
        };
        let detail = [name, extra.to_owned()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        states::inline(ui, state, &detail);
    };
    line(ui, &data, item_text(&data, &["detail", "step", "message", "title"]).filter(|_| data.is_object()).unwrap_or_default());
    for s in data.get("steps").and_then(Value::as_array).into_iter().flatten() {
        line(ui, s, item_text(s, &["title", "action_ref", "id"]).unwrap_or_default());
    }
}

/// recorder: 録音の開始は action の button（mic の許可・接続が無ければ理由つき disabled）。最長の秒数を添える。
/// G88b: host が `take(recorder_ref)` を返す node は録音の帯（`take_band`）。
fn recorder(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let max = node.props.get("max_seconds").and_then(Value::as_u64);
    if let Some((r, pane)) = prop_str(node, "recorder_ref").and_then(|r| cx.host.take(r).map(|p| (r, p))) {
        take_band(ui, node, r, &pane, max, cx);
        return;
    }
    let input = match max {
        Some(m) => json!({ "max_seconds": m }),
        None => json!({}),
    };
    gated_button(ui, node, cx, prop_str(node, "label").unwrap_or("Start recording"), Variant::Primary, false, input);
    if let Some(m) = max {
        theme::label(ui, Type::Helper, format!("Up to {m} s"), cx.t.text_tertiary);
    }
}

/// AI-COMPOSE-1101: AI が書いた部品の枠の既定の高さ（props の `height` で 120〜720 に替えられる）。
const CANVAS_PART_H: f32 = 320.0;

/// AI-COMPOSE-1101（owner 206）: AI が HTML / SVG で書いた部品（`CanvasProps`）= 閉じた WebView の枠。WebView の入った build（`markdown::canvas_host`）で
/// 枠が全部見えている frame だけ slot を置き（WebView は egui で切れない）、app.rs の `canvas_sync` が開いて合わせる。chat の card と違い `Run` を押さずに動く
/// （Page に置いた = 使う部品）。置けない間は枠の真ん中に理由の 1 行。中の頼み（read / set / act）は host が受ける（`surface::CanvasBridge::Page`）。
fn canvas(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let t = cx.t;
    let Some(p) = crate::domain::ui::CanvasProps::of(node) else {
        states::inline(ui, RegionState::Partial, "This part can't be shown");
        return;
    };
    let h = node.props.get("height").and_then(Value::as_f64).map_or(CANVAS_PART_H, |h| (h as f32).clamp(120.0, 720.0));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), t.surface_1);
    let ctx = ui.ctx().clone();
    let line = if !crate::ui::markdown::canvas_host(&ctx) {
        "This build can't show parts the AI made"
    } else if !ui.clip_rect().contains_rect(rect) {
        "Scroll until all of it shows to use it"
    } else {
        let key = egui::Id::new(("kangae.canvas.part", node.key.as_str(), p.html.as_str())).value();
        crate::ui::markdown::push_canvas(&ctx, crate::ui::markdown::CanvasSlot { key, html: p.html, rect, part: Some((node.key.clone(), p.values)) });
        return;
    };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, line, Type::Small.font(), t.text_secondary);
}

/// G88b: 録音の帯（図 §10）。録る前 = `Record humming`・mic が off = 理由と `Open System Settings`・録っている = 経った秒と `Stop`・
/// 録った = 波形・`Play`・`Add take to {app}`（押す前に何が起きるかを下に 1 行・入れる先が Ready でなければ押せず理由の 1 行）・`Discard`。
/// 入れる先が無い（`feeds` が null・表に無い app）なら `Reveal in Finder`。押した手は host（`take_act`）へ。Primary は次の一手の数え方（`next_step_variant`）。
fn take_band(ui: &mut egui::Ui, node: &UiNode, r: &str, pane: &TakePane, max: Option<u64>, cx: &mut Cx<'_>) {
    let t = cx.t;
    let mut act = None;
    let mut pick = |hit: bool, a: TakeAct| {
        if hit {
            act = Some(a);
        }
    };
    match &pane.stage {
        TakeStage::Idle => {
            let v = states::next_step_variant(ui);
            pick(theme::button(ui, prop_str(node, "label").unwrap_or("Record humming"), v).clicked(), TakeAct::Record);
            if let Some(m) = max {
                theme::label(ui, Type::Helper, format!("Up to {m} s"), t.text_tertiary);
            }
        }
        TakeStage::MicOff => {
            theme::label(ui, Type::Helper, "Microphone access is off", t.text_secondary);
            let v = states::next_step_variant(ui);
            pick(theme::button(ui, "Open System Settings", v).clicked(), TakeAct::MicSettings);
        }
        TakeStage::Recording { seconds } => {
            theme::label(ui, Type::Helper, format!("Recording · {seconds:.0} s"), t.text_secondary);
            pick(theme::button(ui, "Stop", Variant::Ghost).clicked(), TakeAct::Stop);
        }
        TakeStage::Recorded { seconds, peaks } => {
            bars(ui, peaks, &t);
            theme::label(ui, Type::Helper, format!("Take · {seconds:.0} s"), t.text_tertiary);
            ui.horizontal(|ui| {
                pick(theme::button(ui, "Play", Variant::Ghost).clicked(), TakeAct::Play);
                match &pane.add {
                    Some(add) => {
                        let label = format!("Add take to {}", add.app);
                        match &add.blocked {
                            None => {
                                let v = states::next_step_variant(ui);
                                pick(theme::button(ui, &label, v).clicked(), TakeAct::Add)
                            }
                            Some(why) => {
                                theme::button_disabled(ui, &label, theme::BUTTON_H, why);
                            }
                        }
                    }
                    None => pick(theme::button(ui, "Reveal in Finder", Variant::Ghost).clicked(), TakeAct::Reveal),
                }
                pick(theme::button(ui, "Discard", Variant::Ghost).clicked(), TakeAct::Discard);
            });
            if let Some(add) = &pane.add {
                let line = add.blocked.clone().unwrap_or_else(|| format!("Goes to {}'s cursor on the selected track", add.app));
                theme::label(ui, Type::Helper, line, t.text_secondary);
            }
        }
    }
    if !pane.line.is_empty() {
        theme::label(ui, Type::Helper, &pane.line, t.text_secondary);
    }
    // 録っている間と入れている間（裏の thread）は秒と終わりを拾うために起こす。止まっている帯は frame を頼まない
    if matches!(pane.stage, TakeStage::Recording { .. }) || pane.line.ends_with('…') {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    if let Some(a) = act {
        // 押せなかった理由は host が次の frame の `line` に出す（帯の 1 行は 1 つ）
        let _ = cx.host.take_act(r, a);
    }
}

/// UI-5m: Mirror の面が拡大の形の間（`MirrorPages::zoomed`）、その面の `source_surface` を含まない node は描かない（side・bottom・drawer を隠す）。
/// 祖先の容れ物は描く（AX の node の親を替えない・W14b）。拡大した面が Page の木に無ければ何も隠さない。
pub(super) fn zoom_hides(ctx: &egui::Context, node: &UiNode, cx: &Cx<'_>) -> bool {
    #[cfg(feature = "mirror")]
    if let Some(z) = cx.surfaces.mirror.as_deref().and_then(|m| m.zoomed(ctx)) {
        fn holds(n: &UiNode, z: &str) -> bool {
            (n.component == "source_surface" && prop_str(n, "surface_ref") == Some(z)) || n.children.iter().any(|c| holds(c, z))
        }
        return holds(&cx.app.composition.root, z) && !holds(node, z);
    }
    let _ = (ctx, node, cx);
    false
}

/// source_surface（DESIGN §4.4）: 元の app の画面は埋め込まない。表示の種類と編集できる場所を 1 行で言い、
/// 外の window を開く button（handoff）を置く。`embedded` も handoff で開く（埋め込みに失敗した黒い面を出さない）。
/// 例外（W12W-1）: host が `embeds(surface_ref)` と言う面だけ、残りの高さ（最小 `EMBED_MIN_HEIGHT`）の矩形を予約して `place_surface` に渡す。
/// I33b: Mirror の面の枠（`MirrorPages::show`）と、確かめていない面の帯の右端の状態。Page の木の後（`floating_faces`）で下から上の順に呼ぶ。
#[cfg(feature = "mirror")]
fn mirror_face(ui: &mut egui::Ui, rect: egui::Rect, spec: &crate::domain::project::SurfaceSpec, cx: &mut Cx<'_>) {
    let receipt = cx.host.surface_receipt(&spec.id);
    let Some(pages) = cx.surfaces.mirror.as_deref_mut() else { return };
    let in_band = super::surface_unchecked(receipt.as_ref()) && matches!(pages.window(&spec.id), crate::ui::mirror_view::MirrorWindow::Open | crate::ui::mirror_view::MirrorWindow::Zoomed);
    pages.show(ui, rect, spec);
    if in_band {
        let slot = crate::ui::mirror_view::band_status_rect(rect);
        super::surface_status(&mut ui.new_child(egui::UiBuilder::new().id_salt(("band-status", &spec.id)).max_rect(slot).layout(egui::Layout::right_to_left(Align::Center))), &spec.id, receipt.as_ref());
    }
}

/// I33b: Page の木を描いた後に、`MirrorPages::later` が覚えた枠を下から上の順（最後に動かした枠が一番上）で描く。
/// 同じ layer の後に描いた物が描きも hit も上 = 前の部品・他の列・他の枠に重ねた枠が上。`render_with` が `draw_node` の直後に 1 回呼ぶ
#[cfg(feature = "mirror")]
pub(super) fn floating_faces(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let Some(stack) = cx.surfaces.mirror.as_deref_mut().map(|p| p.floating()) else { return };
    let specs = cx.surfaces.specs;
    for (id, rect) in stack {
        if let Some(spec) = specs.iter().find(|s| s.id == id) {
            mirror_face(ui, rect, spec, cx);
        }
    }
}

fn source_surface(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let t = cx.t;
    // W16M-1: Project の surface_specs の Mirror の面は、後ろの元の窓の映像を矩形に描く
    let spec = prop_str(node, "surface_ref").and_then(|r| cx.surfaces.specs.iter().find(|s| s.id == r));
    let sid = prop_str(node, "surface_ref");
    let embeds = sid.is_some_and(|s| cx.host.embeds(s));
    // 面の頭は 1 行（W15-c2）: Mirror の面は枠の題（`{App} · {書類}`）が名前を言うので見出しを重ねず、Web の面は下で名前と状態を 1 行にする
    if !embeds && !spec.is_some_and(|s| s.backend == crate::domain::project::SurfaceBackendKind::Mirror) {
        heading(ui, node, cx.t);
    }
    if let Some(spec) = spec.filter(|s| s.backend == crate::domain::project::SurfaceBackendKind::Mirror) {
        // INTEG-2: Mirror の面も他の面と同じく `surface-status:<id>` の 1 行を先に出す（L1 の「面ごとに 1 つ」・CLI の surfaces と画面の突き合わせ）
        // G93: 確かめていない面は 1 行を取らず、開いた枠の title bar の右端へ（AX の `surface-status:<id>` は同じ・畳んだ面は今まで通りの行）。
        // NOTIFY-1101 から `○` は描かない = 帯の右端に置くのは AX の node だけ（面の上に空の行の間を作らない）
        let receipt = cx.host.surface_receipt(&spec.id);
        #[cfg(feature = "mirror")]
        let in_band = super::surface_unchecked(receipt.as_ref()) && cx.surfaces.mirror.as_deref().is_some_and(|p| matches!(p.window(&spec.id), crate::ui::mirror_view::MirrorWindow::Open | crate::ui::mirror_view::MirrorWindow::Zoomed));
        #[cfg(not(feature = "mirror"))]
        let in_band = false;
        if !in_band {
            super::surface_status(ui, &spec.id, receipt.as_ref());
        }
        #[cfg(feature = "mirror")]
        let (below, kind, band) = (reserved_below(ui, node, cx), service(node, cx), recorder_below(node, cx));
        #[cfg(feature = "mirror")]
        if let Some(pages) = cx.surfaces.mirror.as_deref_mut() {
            let w = ui.available_width();
            // UI-5m: 高さは窓の形で決まる（畳んだ = 題の帯だけ・下の region が空いた所を使う／拡大 = Page の中身の見える残り）
            let rest = below_cursor(ui) - theme::SP_4;
            // G92 → LAYOUT: 開いた形は service の種類（`service`）で決める。伸びる面（主の列の app）= 窓の残りから後ろに縦に並ぶ物（Terminal・面・録音の帯）の分を
            // 引いた全部・伸びない面（side の app = 小さい side）= 床のまま。前は幅 × 0.625 で縛り、主の REAPER が低かった（owner 10-31「reaper が小さすぎる」）
            let floor = face_floor(&kind);
            let open = if kind.is_some_and(|(_, grows)| grows) { grow_height(rest, below, floor, band) } else { floor };
            // 床も入らない間（低い窓・下の panel）は伸びない面を 1 回だけ畳む（黄 = 帯だけ・本物も最小化）。保存せず、足りるようになったら戻す（`fold_for_room`）。
            // 利用者が開き直したら畳み直さない（その後は scroll）・利用者が畳んだ面は戻さない
            // MIRROR-FRAME-1101: 中身の無い枠は畳まない（畳むと `Choose a window` が隠れる・本物の窓も無い）
            pages.fold_for_room(&spec.id, !spec.blank_frame() && !kind.is_some_and(|(_, grows)| grows) && rest - below < floor, ui.ctx().input(|i| i.focused), ui.ctx().input(|i| i.time));
            // MIRROR-FIT-1101（owner 198 の 8）: 枠は本物の窓の大きさ（入らなければ比のまま縮める）= 枠の外に灰色の余白を作らない
            let (fit_open, fit_zoom) = (pages.frame_size(&spec.id, Vec2::new(w, open)), pages.frame_size(&spec.id, Vec2::new(w, rest.max(floor))));
            let h = pages.height(ui.ctx(), &spec.id, fit_open.y, fit_zoom.y);
            let fw = if pages.window(&spec.id) == crate::ui::mirror_view::MirrorWindow::Zoomed { fit_zoom.x } else { fit_open.x };
            // MIRROR-FIT-1101: 右下以外の隅で大きさを替えた枠は、引いた隅の反対を Page の上に残す（並びの所から右・下へずらす・開いた形の間だけ）
            // I33b: `room` = Page の列（`render_with` が木の前に覚えた矩形）を並びの所から見た物 = 左・上へも（前の部品に重ねてよい）・横は列の幅まで
            let at = ui.cursor().min;
            let page = cx.state.rects.get(super::MIRROR_PAGE).copied().unwrap_or(egui::Rect::from_min_max(at, egui::pos2(at.x + w, f32::INFINITY)));
            let shift = pages.shift(&spec.id, fit_open, egui::Rect::from_min_max(page.min - at.to_vec2(), egui::pos2(page.max.x - at.x, f32::INFINITY)));
            note_height(ui.ctx(), &node.key, "face", (h + shift.y).max(0.0));
            // MIRROR-HYBRID-1101 追記 246: 人が四隅で大きさを決めた Mirror は Browser と同じく縮まない = 前の面が残す分はこの高さ（`need`・部品を引いて大きくする道は 1 つ）
            note_height(ui.ctx(), &node.key, "fixed", if spec.frame.is_some_and(|f| f.valid()) { (h + shift.y).max(0.0) } else { 0.0 });
            // I33b: 取るのは並びの所から枠の右下まで（左・上へ重ねた分は取らない = 負は 0・横は列の幅まで = 横の row の列を広げない）。後ろの部品は枠の下から
            let (slot, _) = ui.allocate_exact_size((Vec2::new(fw, h) + shift).max(Vec2::ZERO).min(Vec2::new(w, f32::INFINITY)), egui::Sense::hover());
            let rect = egui::Rect::from_min_size(slot.min + shift, Vec2::new(fw, h));
            // 試験と負の対照が測る枠の矩形（node の矩形は頭の行を含む）
            cx.state.rects.insert(format!("face:{}", node.key), rect);
            // MIRROR-FRAME-1101: 足した枠を次の frame で見える所へ（G92 の Browser・Terminal と同じ `reveal`）
            if cx.host.take_reveal(&spec.id) {
                ui.scroll_to_rect(rect, None);
            }
            // I33b: 並びの所では描かず、Page の木の後で下から上の順に（`floating_faces`）
            pages.later(&spec.id, rect);
            return;
        }
        // NOTIFY-1101: 映せない build（feature `mirror` 無し・Mirror の部品が無い）は面の場所に何も描かず、その Project を開いた時に toast 1 回（失敗 = × まで残る）
        crate::ui::toast::post_once(ui.ctx(), &format!("no-mirror:{}", spec.id), "This version can't mirror apps", true, None);
        return;
    }
    // W12W-1: Page の中に置く面（今は WebView）。説明・状態の 1 行の下に矩形を予約し、毎 frame host に渡す（中身を描くのは host）
    if let Some(sid) = sid.filter(|s| cx.host.embeds(s)) {
        let (browser, frame) = cx
            .surfaces
            .specs
            .iter()
            .find(|s| s.id == sid)
            .map(|s| (s.browser, s.frame))
            .unwrap_or((false, None));
        // NOTIFY-1101（owner 185「Excalidraw ○ いらない」）: 面の頭に名前は描かない（面そのものが見えている）。状態は確かめた・届かない時だけ 1 行
        // （確かめていない `○` は描かない・AX の `surface-status:<id>` は残す）
        super::surface_status(ui, sid, cx.host.surface_receipt(sid).as_ref());
        // LAYOUT: 後ろに縦に並ぶ物（面・Terminal・録音の帯）の分を窓に残す。床は `face_floor`（`need` が後ろの面に残す分と同じ値）
        let h = web_height(ui, reserved_below(ui, node, cx), face_floor(&service(node, cx)), recorder_below(node, cx));
        if browser {
            let top = ui.cursor().top();
            let at = ui.cursor().min;
            let w = ui.available_width();
            // G92: 足した Browser を次の frame で見える所まで送る（題の帯から矩形の下端まで）・FX3（FX-014）: その上の欄に keyboard focus
            let reveal = cx.host.take_reveal(sid);
            let face = browser_surface(ui, sid, frame, reveal, h, cx);
            note_height(ui.ctx(), &node.key, "face", face);
            cx.state.rects.insert(format!("face:{}", node.key), egui::Rect::from_min_size(at, Vec2::new(w, face)));
            // 利用者が大きさを決めた Browser（frame）は縮まない = 前の面が残す分はこの高さ（`need`）
            note_height(ui.ctx(), &node.key, "fixed", if frame.is_some_and(|f| f.valid()) { face } else { 0.0 });
            if reveal {
                ui.scroll_to_rect(egui::Rect::from_min_max(egui::pos2(ui.min_rect().left(), top), egui::pos2(ui.min_rect().right(), ui.cursor().top())), Some(Align::Max));
            }
            return;
        }
        note_height(ui.ctx(), &node.key, "face", h);
        let (slot, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
        cx.state.rects.insert(format!("face:{}", node.key), slot);
        ui.painter().rect(slot, CornerRadius::same(theme::RADIUS_SM), t.surface_2, theme::hairline(&t), StrokeKind::Inside);
        // scroll で見えない所・下の panel の下まで WebView（子の NSView は egui の clip を受けない）を出さない（MIRROR-FIT-1101: 縮めずに切り抜く）
        cx.host.place_surface(sid, slot, visible_part(ui, slot));
        return;
    }
    // W16-1: Native Direct で重ねる面（feature native_direct）。None は W3 の 1 行のまま
    let portal = prop_str(node, "surface_ref").and_then(|sid| cx.host.surface_portal(sid));
    let overlay = portal.as_ref().map(|v| format!("{}'s window sits over this page · typing goes to {}", v.app_name, v.app_name));
    let about = match (prop_str(node, "display"), node.action_ref.is_some()) {
        _ if overlay.is_some() => overlay.as_deref().unwrap_or_default(),
        (Some("mirror"), _) => "View only",
        (Some("native" | "composite"), _) => "Shows it in Shibubu",
        // 押す button（action）が無い面は「開きます」と約束しない（W3 の Docs・Diagram の面。Page に置くのは W12W・W16）
        (_, false) => "This version can't show it on this page",
        (Some("embedded"), true) => "Opens in the original app's window instead of embedding (edit there too)",
        (_, true) => "Opens in the original app's window (edit there too)",
    };
    theme::label(ui, Type::Helper, about, t.text_secondary);
    if let Some(sid) = sid {
        super::surface_status(ui, sid, cx.host.surface_receipt(sid).as_ref());
        #[cfg(feature = "native_direct")]
        if let Some(v) = &portal {
            native_portal(ui, sid, v, cx);
        }
    }
    if node.binding.is_some() {
        if let Some(data) = content(ui, node, cx, false) {
            states::inline(ui, RegionState::Ready, &item_text(&data, &["title", "name", "url"]).unwrap_or_default());
        }
    }
    if node.action_ref.is_some() {
        gated_button(ui, node, cx, "Open window", Variant::Ghost, true, json!({}));
    }
}

/// terminal（W5・W10）: 汎用の木の Terminal の部品（上に状態の 1 行・中に画面 = `AxScope::Terminal.id(terminal_ref)` の要素・終わった session は `Restart`）。
/// TERM-ONE-1101: Project の Page は描く木からこの node を外す（`generated::shown_root`・Terminal は下の panel の tab の 1 種類）= ここに来るのは Project の外の木と試験だけ。
/// 画面は `pane.screen` が在れば VT の grid（`terminal_grid`・W10）、無ければ W5 の plain text。
/// host が None（Project の外の app の木・terminal_ref が無い）は 1 行だけで、偽の Terminal を描かない（D-e）。描画の範囲は `TerminalPane` の doc。
fn terminal(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    let top = ui.cursor().top();
    heading(ui, node, cx.t);
    if prop_str(node, "terminal_ref").and_then(|r| terminal_body(ui, &node.key, r, top, &mut *cx.host, cx.t, None)).is_none() {
        states::inline(ui, RegionState::Empty, "No terminal on this page");
    }
}
/// G95: Terminal 1 つの本体（状態の 1 行・grid・終わっていれば `Restart`）。Page の `terminal` node と Project の下の panel（`ui::terminal_panel`）が同じ本体を描く。
/// TERM-ONE-1101（owner 181・185）: terminal ごとの入力欄（`Type in the terminal`・`Send`）は無い = 打つのは grid に直に（IME の変換中の字も grid の cursor の所）。
/// `key` = egui の id の元（node の key か `panel.{tab}`）・`r` = `terminal_ref`・`top` = 見せる時に送る矩形の上端・`bottom` = grid の行数を決める下端（None = Page の主領域の残り・`below_cursor`）。
/// 返り値: None = host にこの Terminal が無い（呼ぶ側が 1 行を出す）。保証しない: 画面を写せない Terminal（`screen` が None）に打つ事（打つ所が無い・状態の 1 行と出力だけ）。
pub fn terminal_body(ui: &mut egui::Ui, key: &str, r: &str, top: f32, host: &mut dyn super::ActionHost, t: Tokens, bottom: Option<f32>) -> Option<()> {
    let pane = host.terminal(r)?;
    // owner 210: 下の panel の中は grid だけが端から端まで。状態の 1 行・入力欄・Restart は panel の会話と同じ内側（`terminal_panel::INSET_X`）
    let inset = || egui::Frame::new().inner_margin(egui::Margin::symmetric(if bottom.is_some() { crate::ui::terminal_panel::INSET_X as i8 } else { 0 }, 0));
    // G92: Add terminal / Show terminal の後の 1 frame（入力欄を描いた後に、見出しから入力欄までを窓の下端に合わせて送る）
    let reveal = host.take_reveal(r);
    // 状態の 1 行は印（● 準備完了 等）を付けない素の行（「完了」は L1 の成功の語・Terminal の状態は surface-status ではない）。
    // NOTIFY-1101（owner 185「zsh · pid … いらない」）: 動いている間の `zsh · pid N · ~` は描かない（tab の名前で足りる）。AX の StaticText には残す
    // （試験の側 rehearse.py・test.sh が pid をここから読む）。止まった・誤りの 1 行（Restart・送れなかった理由）は今まで通り描く
    if pane.pid.is_some_and(|p| pane.status_line.contains(&format!(" · pid {p}"))) {
        let (id, at) = (ui.id().with((key, "terminal-status")), ui.cursor().min);
        ui.ctx().accesskit_node_builder(id, |n| {
            n.set_role(egui::accesskit::Role::Label);
            n.set_value(pane.status_line.as_str());
            // cua-driver の elements は操作できる要素だけ並べる（`theme::heading` の doc）= 前の選べる label と同じく Click を持たせて rehearse.py が読めるように
            n.add_action(egui::accesskit::Action::Click);
            n.set_bounds(egui::accesskit::Rect { x0: at.x.into(), y0: at.y.into(), x1: (at.x + 1.0).into(), y1: (at.y + 1.0).into() });
        });
    } else {
        inset().show(ui, |ui| ui.add(egui::Label::new(Type::Helper.text(pane.status_line.as_str()).color(t.text_secondary)).wrap().selectable(true)));
    }
    if let Some(screen) = &pane.screen {
        // TERM-XTERM-1101: 中身は host の描き手（xterm.js の頁）。描き手が無い時だけ egui の grid
        if !terminal_native(ui, key, r, &pane, host, t, bottom, reveal) {
            terminal_grid(ui, key, r, &pane, screen, host, t, bottom);
        }
    } else {
        // 出力: plain text（W5 の描画の範囲は TerminalPane の doc）。AX の identifier は terminal:<id>（試験の側がこの value を読む）
        egui::Frame::new().fill(t.surface_2).inner_margin(theme::SP_2).corner_radius(CornerRadius::same(theme::RADIUS_SM)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt((key, "terminal-text")).max_height(320.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                let shown = if pane.text.is_empty() { " " } else { pane.text.as_str() };
                let out = ui.add(egui::Label::new(Type::Mono.text(shown).color(t.text_primary)).wrap().selectable(true));
                let ax = crate::ui::ax::AxScope::Terminal.id(r);
                ui.ctx().accesskit_node_builder(out.id, |n| n.set_author_id(ax));
            });
        });
    }
    // 見せる手（Show terminal・New terminal）の後に grid へ focus（そのまま打てる）
    if pane.accepts_input {
        // xterm の頁の focus は `terminal_place` の focus が持つ（egui の grid の id に focus を置かない）
        if reveal && pane.screen.is_some() && ui.data(|d| d.get_temp::<bool>(egui::Id::new((key, "terminal-native")))) != Some(true) {
            let grid = ui.make_persistent_id((key, "terminal-grid"));
            ui.memory_mut(|m| m.request_focus(grid));
        }
    } else {
        // G94: 起こせなかった理由（`No terminal on this page`・消えた Terminal）を Terminal の 1 行に出す（捨てない）。起こせれば消す。
        // spawn の失敗は host が状態の 1 行に出すので、同じ文を重ねない
        let eid = ui.make_persistent_id((key, "restart-err"));
        if inset().show(ui, |ui| theme::button_compact(ui, "Restart", Variant::Ghost)).inner.clicked() {
            match host.terminal_restart(r) {
                Ok(()) => ui.data_mut(|d| {
                    d.remove::<String>(eid);
                }),
                Err(e) => ui.data_mut(|d| d.insert_temp(eid, e)),
            }
        }
        if let Some(line) = ui.data(|d| d.get_temp::<String>(eid)).filter(|l| !pane.status_line.contains(l.as_str())) {
            inset().show(ui, |ui| states::inline_wrap(ui, RegionState::Error, &line));
        }
    }
    if reveal {
        ui.scroll_to_rect(egui::Rect::from_min_max(egui::pos2(ui.min_rect().left(), top), egui::pos2(ui.min_rect().right(), ui.cursor().top())), Some(Align::Max));
    }
    Some(())
}
/// TERM-XTERM-1101（owner 204 の 4「普通のterminalをshibubuで使えないの？orcaみたいに」）: 中身を host の描き手（xterm.js の頁・`ActionHost::terminal_place`）に任せる。
/// host が描かない（None）なら false = 呼ぶ側が `terminal_grid`。枠は grid と同じ所（panel の中は panel の下端まで・端から端まで・Page の中は主領域の残り）・
/// 地は頁と同じ色（頁が出る前の frame に別の色を見せない）・AX の `terminal:<id>` は alacritty の字のまま（試験の側が読む）・頁の右 click は grid と同じ menu。
/// `focus` = Show terminal / New terminal の後の 1 frame（頁へ focus）。
#[allow(clippy::too_many_arguments)]
fn terminal_native(ui: &mut egui::Ui, key: &str, r: &str, pane: &TerminalPane, host: &mut dyn super::ActionHost, t: Tokens, bottom: Option<f32>, focus: bool) -> bool {
    let top = ui.cursor().top();
    let room = bottom.map_or_else(|| below_cursor(ui).max(f32::from(TERM_MIN_ROWS) * Type::Mono.line_height()), |b| b - top);
    let rect = egui::Rect::from_min_size(egui::pos2(ui.max_rect().left(), top), Vec2::new(ui.max_rect().width(), room.max(0.0)));
    let Some(placed) = host.terminal_place(r, rect, rect.intersect(ui.clip_rect()), focus && pane.accepts_input) else {
        ui.data_mut(|d| d.insert_temp(egui::Id::new((key, "terminal-native")), false));
        return false;
    };
    ui.data_mut(|d| d.insert_temp(egui::Id::new((key, "terminal-native")), true));
    let _ = ui.allocate_rect(rect, Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t.surface_1);
    let id = ui.make_persistent_id((key, "terminal-native"));
    let resp = ui.interact(rect, id, Sense::hover());
    let ax = crate::ui::ax::AxScope::Terminal.id(r);
    let text = pane.text.clone();
    ui.ctx().accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::Label);
        n.set_author_id(ax);
        n.set_value(text);
        n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
    // 頁の右 click: その所に grid と同じ menu（選びの字は menu を開いている間持つ）
    let sel_id = id.with("selection");
    if let Some((at, sel)) = placed.menu {
        ui.data_mut(|d| d.insert_temp(sel_id, sel));
        open_grid_menu(ui, resp.id, rect.min + at);
    }
    let sel: String = ui.data(|d| d.get_temp(sel_id)).unwrap_or_default();
    let copy = || sel.clone();
    terminal_grid_menu(ui, r, pane, &resp, host, (!sel.is_empty()).then_some(&copy as &dyn Fn() -> String), true);
    true
}

/// MIRROR-FIT-1101（owner 198 の 8）: Page の面の矩形のうち見えている所 = scroll の clip の内で、下の panel（`terminal_panel::covering`・幅いっぱいの下の帯）より上。
/// 子の NSView（WebView）は egui の clip も panel の Area も受けないので、ここで切った外を `AppKitOp::WebClip` が切り抜く（WebView は `slot` の大きさのまま）。
/// 全部被さった面は高さ 1pt 未満（`PageWebViews::place` が隠す）。
fn visible_part(ui: &egui::Ui, slot: egui::Rect) -> egui::Rect {
    let shown = slot.intersect(ui.clip_rect());
    match crate::ui::terminal_panel::covering(ui.ctx()) {
        // ponytail: panel は幅いっぱいの下の帯なので上端で切るだけ（横や真ん中に被さる物が来たら矩形の引き算に）
        Some(c) if c.intersects(shown) => egui::Rect::from_min_max(shown.min, egui::pos2(shown.max.x, shown.max.y.min(c.min.y))),
        _ => shown,
    }
}

/// LAYOUT: Page の中の Web の面の高さ = 窓の残り − 後ろに縦に並ぶ物の分（`below` = `reserved_below`）。床 = `face_floor`・上限 4 × `EMBED_MIN_HEIGHT`。
fn web_height(ui: &egui::Ui, below: f32, floor: f32, band: bool) -> f32 {
    let h = grow_height(below_cursor(ui) - theme::SP_4, below, floor, band);
    if h.is_finite() {
        h
    } else {
        super::EMBED_MIN_HEIGHT
    }
}

/// W12W-B: Browser の題の帯・戻る / 進む / 読み直し・URL 欄・WebView の矩形。frame が在ればその大きさ（位置は Area）。
/// LAYOUT: `default_h` は frame の無い時の高さ（`web_height`）。返り値は面の高さ（`note_height` の `"face"`）。
fn browser_surface(ui: &mut egui::Ui, sid: &str, frame: Option<crate::domain::project::SurfaceFrame>, reveal: bool, default_h: f32, cx: &mut Cx<'_>) -> f32 {
    let avail_w = ui.available_width();
    let size = Vec2::new(
        frame.filter(|f| f.valid()).map(|f| f.w as f32).unwrap_or(avail_w).clamp(280.0, avail_w.max(280.0)),
        frame.filter(|f| f.valid()).map(|f| f.h as f32).unwrap_or(default_h).clamp(default_h.min(super::EMBED_MIN_HEIGHT), 4.0 * super::EMBED_MIN_HEIGHT),
    );
    if let Some(f) = frame.filter(|f| f.valid()) {
        let ctx = ui.ctx().clone();
        egui::Area::new(ui.id().with(("web-area", sid)))
            .current_pos(egui::pos2(f.x as f32, f.y as f32))
            .order(egui::Order::Middle)
            .movable(false)
            .show(&ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                browser_chrome(ui, sid, size, reveal, cx);
            });
        ui.allocate_exact_size(size, Sense::hover());
    } else {
        browser_chrome(ui, sid, size, reveal, cx);
    }
    size.y
}

fn browser_chrome(ui: &mut egui::Ui, sid: &str, size: Vec2, reveal: bool, cx: &mut Cx<'_>) {
    const TITLE_H: f32 = 18.0;
    const EDGE: f32 = 6.0;
    let t = cx.t;
    let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
    let title = egui::Rect::from_min_size(outer.min, Vec2::new(outer.width(), TITLE_H));
    let title_r = ui.interact(title, ui.id().with(("web-title", sid)), Sense::drag());
    ui.painter().rect_filled(title, CornerRadius::ZERO, t.surface_3);
    ui.painter().text(
        title.left_center() + Vec2::new(theme::SP_2, 0.0),
        egui::Align2::LEFT_CENTER,
        "Browser",
        Type::Helper.font(),
        t.text_secondary,
    );
    let chrome = egui::Rect::from_min_size(egui::pos2(outer.min.x, title.max.y), Vec2::new(outer.width(), theme::INPUT_H));
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(chrome), |ui| {
        ui.horizontal_centered(|ui| {
            let back = theme::ax_name(theme::button_compact(ui, theme::icon_glyph(Icon::ArrowLeft), Variant::Ghost), "Back");
            ui.ctx().accesskit_node_builder(back.id, |n| n.set_author_id(format!("web-back:{sid}")));
            if back.clicked() {
                cx.host.browser_back(sid);
            }
            let forward = theme::ax_name(theme::button_compact(ui, theme::icon_glyph(Icon::ArrowRight), Variant::Ghost), "Forward");
            ui.ctx().accesskit_node_builder(forward.id, |n| n.set_author_id(format!("web-forward:{sid}")));
            if forward.clicked() {
                cx.host.browser_forward(sid);
            }
            let reload = theme::ax_name(theme::button_compact(ui, theme::icon_glyph(Icon::Refresh), Variant::Ghost), "Reload");
            ui.ctx().accesskit_node_builder(reload.id, |n| n.set_author_id(format!("web-reload:{sid}")));
            if reload.clicked() {
                cx.host.browser_reload(sid);
            }
            let bar_id = ui.make_persistent_id(("web-bar", sid));
            let focused = ui.memory(|m| m.has_focus(bar_id));
            let mut s = ui.ctx().data(|d| d.get_temp::<String>(bar_id.with("text"))).unwrap_or_else(|| cx.host.browser_url(sid).unwrap_or_default());
            if !focused {
                if let Some(u) = cx.host.browser_url(sid).filter(|u| *u != s) {
                    s = u;
                }
            }
            let edit = egui::TextEdit::singleline(&mut s)
                .id(bar_id)
                .desired_width(ui.available_width())
                .hint_text("Search or enter website");
            let field = theme::ax_name(ui.add(edit), "Search or enter website").ax_text_input(ui, &mut s);
            ui.ctx().accesskit_node_builder(field.id, |n| n.set_author_id(format!("web-bar:{sid}")));
            if reveal {
                field.request_focus();
            }
            ui.ctx().data_mut(|d| d.insert_temp(bar_id.with("text"), s.clone()));
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                cx.host.browser_go(sid, &s);
            }
        });
    });
    let slot = egui::Rect::from_min_max(egui::pos2(outer.min.x, chrome.max.y), outer.max);
    ui.painter().rect(slot, CornerRadius::same(theme::RADIUS_SM), t.surface_2, theme::hairline(&t), StrokeKind::Inside);
    let shown = visible_part(ui, slot);
    if shown.width() >= 1.0 && shown.height() >= 1.0 {
        cx.host.place_surface(sid, slot, shown);
    }
    let right = egui::Rect::from_min_max(egui::pos2(outer.max.x - EDGE, outer.min.y), outer.max);
    let bottom = egui::Rect::from_min_max(egui::pos2(outer.min.x, outer.max.y - EDGE), outer.max);
    let right_r = ui.interact(right, ui.id().with(("web-e", sid)), Sense::drag());
    let bottom_r = ui.interact(bottom, ui.id().with(("web-s", sid)), Sense::drag());
    let origin = ui.clip_rect().min;
    let mem = ui.id().with(("web-frame", sid));
    if title_r.dragged() || right_r.dragged() || bottom_r.dragged() {
        ui.ctx().data_mut(|d| {
            let f = d.get_temp_mut_or_insert_with(mem, || {
                (
                    (outer.min.x - origin.x) as f64,
                    (outer.min.y - origin.y) as f64,
                    outer.width() as f64,
                    outer.height() as f64,
                )
            });
            if title_r.dragged() {
                let dlt = title_r.drag_delta();
                f.0 += dlt.x as f64;
                f.1 += dlt.y as f64;
            }
            if right_r.dragged() {
                f.2 = (f.2 + right_r.drag_delta().x as f64).max(280.0);
            }
            if bottom_r.dragged() {
                f.3 = (f.3 + bottom_r.drag_delta().y as f64).max(super::EMBED_MIN_HEIGHT as f64);
            }
        });
    }
    if title_r.drag_stopped() || right_r.drag_stopped() || bottom_r.drag_stopped() {
        if let Some((x, y, w, h)) = ui.ctx().data_mut(|d| d.remove_temp::<(f64, f64, f64, f64)>(mem)) {
            cx.host.browser_frame(sid, x, y, w, h);
        }
    }
}

/// W16-1: 枠の高さ（元の窓をこの大きさに合わせる。Page は縦に流れるので残り全部ではなく固定）。
#[cfg(feature = "native_direct")]
const PORTAL_H: f32 = 480.0;

/// W16-1: 枠を描き、AX の identifier `surface-frame:<id>`（bounds = 帯の下の slot）を付け、枠の画面座標と押された一手を Host へ返す。
#[cfg(feature = "native_direct")]
fn native_portal(ui: &mut egui::Ui, sid: &str, v: &crate::ui::portal_frame::PortalView, cx: &mut Cx<'_>) {
    let (id, r) = ui
        .allocate_ui(Vec2::new(ui.available_width(), PORTAL_H), |ui| {
            // show が AX の node に使う id（その node に identifier を足す）
            (crate::ui::portal_frame::frame_id(ui, sid), crate::ui::portal_frame::show(ui, sid, &v.frame(), |_| {}))
        })
        .inner;
    // 枠は Page の縦の流れで Shibubu の窓の下端を越える（実機 3 回目: 枠の下端 966・窓の下端 960）。元の窓は見えている所だけに合わせる
    // ponytail: 枠が全部隠れる所まで scroll したら slot を送らない（元の窓は前の位置のまま）。要れば HostHidden と同じ手で Shibubu を上に置く
    let slot = r.slot.intersect(ui.clip_rect());
    let slot = (slot.width() >= 1.0 && slot.height() >= 1.0).then_some(slot);
    let ctx = ui.ctx().clone();
    ctx.accesskit_node_builder(id, |n| {
        n.set_author_id(crate::ui::ax::AxScope::SurfaceFrame.id(sid));
        let b = slot.unwrap_or(r.slot);
        n.set_bounds(egui::accesskit::Rect { x0: b.min.x.into(), y0: b.min.y.into(), x1: b.max.x.into(), y1: b.max.y.into() });
    });
    let (minimized, fullscreen) = ctx.input(|i| (i.viewport().minimized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    let at = crate::surface::Host { slot: slot.and_then(|s| crate::ui::portal_frame::slot_on_screen(&ctx, s)), minimized, fullscreen };
    cx.host.surface_placed(sid, at, r.action);
}

/// W10: Terminal の画面の行の数（PTY は `TermSize::default` の 24 行で起き、描いた欄の行数を `ActionHost::terminal_resize` で受ける）。
/// 列は描ける幅から決め、`screen` と違えば `terminal_resize` で PTY と VT に渡す（欄の行数 = PTY の行数なので prompt と cursor の行は欄の中）。
pub const TERM_ROWS: u16 = 24;
/// G92: 欄の行数の床（Mirror の枠の下に置いた Terminal が窓に入る高さ）。前は `TERM_ROWS` の床で、Mirror の下の Terminal の入力欄が窓の外だった。
pub const TERM_MIN_ROWS: u16 = 6;
/// 欄の行数の上限（窓が極端に高い時に PTY の画面を無制限に広げない）。
const TERM_ROWS_MAX: u16 = 120;

/// G92: 窓の中で cursor より下に残る高さ。主領域を送る前の位置で測る（`shell::main_scrolled`・送るたびに部品が伸びて足した Terminal が窓の外へ逃げない）。
/// 伸びる面（主の列の app・Web）の高さ: 見える残り `rest` から後ろに縦に並ぶ物の分 `below` を引いた全部（上限 4 × `EMBED_MIN_HEIGHT`）。
/// 床（`floor`）が入らない時、後ろに録音の帯が在れば（`band` = `recorder_below`）、帯が見えて押せるように小さい side の床（`APP_SIDE.1`）まで縮む
/// （下の panel を開いた 1440×900・DOGFOOD-X 周 2 の赤「録音の帯が panel の下に隠れて押せない」）。帯が無ければ床のまま（後ろの Terminal は G92 の通り送って見せる）。
fn grow_height(rest: f32, below: f32, floor: f32, band: bool) -> f32 {
    let low = if band { floor.min(layout::APP_SIDE.1) } else { floor };
    (rest - below).min(4.0 * super::EMBED_MIN_HEIGHT).max(low)
}

fn below_cursor(ui: &egui::Ui) -> f32 {
    ui.clip_rect().bottom() - ui.cursor().top() - crate::ui::shell::main_scrolled(ui.ctx())
}

/// 欄の行数: Page の残りの高さから、欄の枠と下の入力欄（Send / Restart の 1 行）を引いた分。窓が低ければ `TERM_MIN_ROWS` まで縮める（G92）。
/// 逆向き（欄の行数 → 要る高さ）は `term_min_height` が同じ式で持つ。
/// Wave: W15（patch c3 = 439408f7・§0 の行は W15）
pub fn term_rows(below_cursor: f32, rh: f32, pad: f32, item_gap: f32) -> u16 {
    ((term_room(below_cursor, pad, item_gap) / rh).floor().max(0.0) as u16).clamp(TERM_MIN_ROWS, TERM_ROWS_MAX)
}

/// `term_rows` の欄に使える高さ（欄の枠の内側の余白と、下の入力欄の 1 行を引いた残り）。
fn term_room(below_cursor: f32, pad: f32, item_gap: f32) -> f32 {
    below_cursor - 2.0 * pad - theme::INPUT_H - item_gap - theme::SP_4
}

/// G92: `TERM_MIN_ROWS` の Terminal 1 つが Page の縦の流れで取る高さ（前の部品との間・見出し・状態の 1 行・欄・入力欄）。
/// Mirror の枠はこの分を窓に残して高さを抑える（`source_surface`）。見出しと状態の行は 1 行ずつで見積もる（長くて折り返すと少し窓の外へ出る）。
fn term_min_height(ui: &egui::Ui) -> f32 {
    let gap = ui.spacing().item_spacing.y;
    let (rh, head, helper) = ui.fonts(|f| (f.row_height(&Type::Mono.font()), f.row_height(&Type::HeadingSm.font()), f.row_height(&Type::Helper.font())));
    let below_cursor = f32::from(TERM_MIN_ROWS) * rh - term_room(0.0, theme::SP_2, gap);
    gap + head + theme::SP_2 + gap + helper + gap + below_cursor
}

/// LAYOUT: node の前の frame の高さを覚える（`draw_node` が node 全体を `"node"`、面が矩形を `"face"` で）。`reserved_below` が後ろの物の見積もりに読む。
/// 高さが替わった（初めて描いた）frame は次の frame を 1 回頼む = 入力が無くても前の部品が残す分を決め直す（同じ高さに落ち着けば頼まない）。
pub(super) fn note_height(ctx: &egui::Context, key: &str, part: &'static str, h: f32) {
    let before = last_height(ctx, key, part);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("kangae-last-h", part, key)), h));
    if !matches!(before, Some(b) if (b - h).abs() <= 0.5) {
        ctx.request_repaint();
    }
}

/// LAYOUT: `note_height` で覚えた前の frame の高さ（描いた事が無ければ None）。
fn last_height(ctx: &egui::Context, key: &str, part: &'static str) -> Option<f32> {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::new(("kangae-last-h", part, key))))
}

/// LAYOUT: 窓の残りに合わせて伸び縮みする面（Mirror の面・Page の中の Web）か。`reserved_below` が床で数え、`service_min_w` が幅を決める。
fn is_face(node: &UiNode, cx: &Cx<'_>) -> bool {
    node.component == "source_surface"
        && prop_str(node, "surface_ref").is_some_and(|sid| cx.host.embeds(sid) || cx.surfaces.specs.iter().any(|s| s.id == sid && s.backend == crate::domain::project::SurfaceBackendKind::Mirror))
}

/// LAYOUT（operator 10-31 の門 3「service ごとの大きさ」）: 部品の service の種類 → `layout` の表の (列の min の幅, 床の高さ) と、窓の残りを取る（伸びる）か。
/// 種類 = 面の backend × 置き場（side の区画の中か）。app の名前・bundle id で分けない（engine の P0）。面でも録音の帯でもない → None。
fn service(node: &UiNode, cx: &Cx<'_>) -> Option<((f32, f32), bool)> {
    use crate::domain::project::SurfaceBackendKind as B;
    if node.component == "recorder" {
        return Some((layout::RECORDER, false));
    }
    let sid = prop_str(node, "surface_ref").filter(|_| node.component == "source_surface")?;
    if cx.host.embeds(sid) {
        return Some((layout::WEB, true));
    }
    let in_side = path_to(&cx.app.composition.root, &node.key).iter().any(|n| prop_str(n, "slot") == Some("side"));
    Some(match cx.surfaces.specs.iter().find(|s| s.id == sid)?.backend {
        B::WebView => (layout::WEB, true),
        B::Mirror | B::NativeDirect if in_side => (layout::APP_SIDE, false),
        B::Mirror | B::NativeDirect => (layout::APP_MAIN, true),
    })
}

/// LAYOUT: 面の床（`service` の表の高さ）。面が自分の高さに使う床と、前の面が `need` で残す分は同じこの値（違うと最後の面が残された分より高く取って窓の外に出る）。
fn face_floor(kind: &Option<((f32, f32), bool)>) -> f32 {
    kind.map_or(super::EMBED_MIN_HEIGHT, |((_, floor), _)| floor)
}

/// LAYOUT: 根から `key` の node までの道（根と node を含む・無ければ空）。
fn path_to<'a>(root: &'a UiNode, key: &str) -> Vec<&'a UiNode> {
    fn walk<'a>(n: &'a UiNode, key: &str, out: &mut Vec<&'a UiNode>) -> bool {
        out.push(n);
        if n.key == key || n.children.iter().any(|c| walk(c, key, out)) {
            return true;
        }
        out.pop();
        false
    }
    let mut out = Vec::new();
    walk(root, key, &mut out);
    out
}

/// LAYOUT: 横に並べる容れ物（split と、塊の混ざる row。`blocks` が描く）。中の子は縦の流れの兄弟でない。
fn across(node: &UiNode) -> bool {
    node.component == "split" || (node.component == "row" && !node.children.iter().all(layout::is_inline))
}

/// LAYOUT: `node` を縦の流れの後ろに置いた時に要る高さ（前との間を含む・`reserved_below` の兄弟 1 つ分）。
fn need(ui: &egui::Ui, node: &UiNode, cx: &Cx<'_>) -> f32 {
    if !visible(node, cx.state) || zoom_hides(ui.ctx(), node, cx) {
        return 0.0;
    }
    let gap = theme::SP_4;
    let last = |part| last_height(ui.ctx(), &node.key, part);
    let drawer = prop_str(node, "slot") == Some("drawer");
    if node.component == "terminal" {
        term_min_height(ui)
    } else if is_face(node, cx) {
        // 頭の行（名前・状態の 1 行）は前の frame の node と面の差。面は床まで縮む = 床の分・畳んだ / 閉じた Mirror は帯だけ（窓の形を見る）・
        // 利用者が大きさを決めた Browser と Mirror（MIRROR-HYBRID-1101 追記 246）は縮まない = その高さ
        let face = match last("fixed") {
            Some(h) if h > 0.0 => h,
            _ => band_only(node, cx).unwrap_or_else(|| face_floor(&service(node, cx))),
        };
        gap + (last("node").unwrap_or(0.0) - last("face").unwrap_or(0.0)).max(0.0) + face
    } else if across(node) {
        node.children.iter().map(|c| need(ui, c, cx)).fold(0.0, f32::max)
    } else if node.component == "recorder" && last("node").is_none() {
        // 初めての frame（まだ描いていない）は録音の帯の 1 段の見積もり（前の面が全部を取って次の frame で縮む揺れを小さくする）
        gap + 2.0 * theme::INPUT_H
    } else if node.component == "column" && !drawer {
        node.children.iter().map(|c| need(ui, c, cx)).sum()
    } else {
        last("node").map_or(0.0, |h| gap + h)
    }
}

/// LAYOUT（owner 10-31 の 19 点の 3・5）: 縦の流れで `node` の後ろに描く物が要る高さ（G92 の `terminals_after` を、面・録音の帯・文にも広げた物）。
/// 根（`cx.app.composition.root` = 置いた後の Page の木）から `node` までの道を下から辿り、親が縦の流れなら後ろの兄弟の要る高さを足す・
/// 親が split / 塊の row / tabs なら足さない（隣の列・隠れた tab）。兄弟 1 つの要る高さ（`need`）: 面（Mirror・Page の中の Web）= 間 + 前の frame の頭の行 +
/// 床（`service` の表）か帯（畳んだ・閉じた Mirror）か利用者が決めた Browser の高さ・Terminal = `term_min_height`・split / 塊の row = 列ごとの最大・
/// 他の容れ物 = 子の和・葉（録音の帯・文）= 間 + 前の frame に描いた高さ（初めての frame の録音の帯は 1 段の見積もり・文は 0）。
/// 例: `let open = (rest - reserved_below(ui, node, cx)).max(face_floor(&service(node, cx)));`
/// 保証しない: `blocks` が縦に畳んだ split の隣の列（畳んだ時は main の列の後ろに side の列が来るが残さない = side は scroll の先）。
fn reserved_below(ui: &egui::Ui, node: &UiNode, cx: &Cx<'_>) -> f32 {
    path_to(&cx.app.composition.root, &node.key)
        .windows(2)
        .filter(|w| !across(w[0]) && w[0].component != "tabs")
        .map(|w| w[0].children.iter().skip_while(|c| c.key != w[1].key).skip(1).map(|c| need(ui, c, cx)).sum::<f32>())
        .sum()
}

/// LAYOUT（DOGFOOD-X 周 2）: `reserved_below` と同じ道の後ろの兄弟（の子孫）に録音の帯が在るか。在れば伸びる面は帯を窓に残すために床の下まで縮む（`grow_height`）。
fn recorder_below(node: &UiNode, cx: &Cx<'_>) -> bool {
    let path = path_to(&cx.app.composition.root, &node.key);
    path.windows(2).filter(|w| !across(w[0]) && w[0].component != "tabs").any(|w| w[0].children.iter().skip_while(|c| c.key != w[1].key).skip(1).any(|c| c.nodes().into_iter().any(|n| n.component == "recorder")))
}

/// LAYOUT（門 3「service ごとの大きさ」）: row / split の子 1 つ（列）が持つ部品の最小の幅 = 子孫の `service` の表の幅の最大。
/// 面も録音の帯も無い → 0（今まで通り `layout::want` の min）。
/// 例（`blocks`）: `w.min = w.min.max(service_min_w(child, cx)); w.preferred = w.preferred.max(w.min);`
fn service_min_w(node: &UiNode, cx: &Cx<'_>) -> f32 {
    node.nodes().into_iter().filter_map(|n| service(n, cx)).map(|((w, _), _)| w).fold(0.0, f32::max)
}

/// LAYOUT（門 5）: 畳んだ（黄）・閉じた（赤）Mirror = 帯だけを描く面なら帯の高さ（`BAND_H`）。`need` が床でなく帯の分を残す（窓の形を読む・前の frame を待たない）。
fn band_only(node: &UiNode, cx: &Cx<'_>) -> Option<f32> {
    #[cfg(feature = "mirror")]
    if let (Some(sid), Some(pages)) = (prop_str(node, "surface_ref"), cx.surfaces.mirror.as_deref()) {
        use crate::ui::mirror_view::{MirrorWindow as W, BAND_H};
        return matches!(pages.window(sid), W::Minimized | W::Closed).then_some(BAND_H);
    }
    let _ = (node, cx);
    None
}

/// TERM-FEEL-1101: grid の 1 列の幅 = galley が実際に字を送る幅（2 字目の x）。epaint は字ごとに x を画素へ丸める（epaint 0.31.1 の
/// text_layout.rs:185 `cursor_x = round_to_pixel(cursor_x)`）ので、丸める前の `glyph_width('M')`（Hack 12pt で 7.22pt・2x で 14.45px）で列を切ると、
/// 1 つの galley で流す字（7.0pt 送り）より cursor・背景・全角の後ろが 1 字ごとに右へずれる（owner 182 通目「terminalのpointerと最後の文字との間に一文字ぐらいのスペース」
/// = 47 列で約 1.6 字）。字・背景・cursor・全角・PTY に送る列の数・IME の窓が同じ格子に乗る（CHAT-ONE の変換中の字も `term_cell` で同じ格子に置く）。Wave: TERM-FEEL-1101
pub(crate) fn term_cell_width(f: &egui::text::Fonts, font: &egui::FontId) -> f32 {
    let g = f.layout_no_wrap("MM".to_owned(), font.clone(), Color32::PLACEHOLDER);
    g.rows.first().and_then(|r| Some(r.glyphs.get(1)?.pos.x - r.glyphs.first()?.pos.x)).unwrap_or_else(|| f.glyph_width(font, 'M'))
}

/// TERM-FEEL-1101: 行の左上 `at` から `col` 列目の cell（幅 `cw` = `term_cell_width`・高さ `rh`）。字の run・背景・cursor・IME の窓が同じ式を通る。Wave: TERM-FEEL-1101
pub(crate) fn term_cell(at: egui::Pos2, col: u16, cw: f32, rh: f32) -> egui::Rect {
    egui::Rect::from_min_size(at + Vec2::new(f32::from(col) * cw, 0.0), Vec2::new(cw, rh))
}

/// W10: VT の画面を等幅の grid で描く。cell の左端は `col × 文字幅`（`term_cell`・全角は 2 cell）・色のある背景は cell の矩形・cursor は cell の枠
/// （focus の間は塗り）。scrollback は上へ scroll（新しい出力が来たら下に付く）。AX は 1 つの要素で value = `pane.text`（`terminal:<ref>`）。
/// grid を click すると focus し、その間の打鍵・貼り付け・IME の確定を `terminal_input_bytes` → `terminal_keys`（Tab・矢印・Esc も Terminal へ）。
fn terminal_grid(ui: &mut egui::Ui, key: &str, r: &str, pane: &TerminalPane, screen: &TermScreen, host: &mut dyn super::ActionHost, t: Tokens, bottom: Option<f32>) {
    let font = Type::Mono.font();
    let (cw, rh) = ui.fonts(|f| (term_cell_width(f, &font), f.row_height(&font)));
    let id = ui.make_persistent_id((key, "terminal-grid"));
    // UI-SWEEP-1101 追記（owner 210「terminalとかの枠マジでいらない」）: 下の panel の中（`bottom` = Some）は枠・角の丸み・面の色なし（panel と同じ地）で panel の端から端まで・
    // 内側は 4（Orca の xterm の pane の余白）。Page の Terminal（`bottom` = None）は今の箱のまま
    let in_panel = bottom.is_some();
    let pad = if in_panel { theme::SP_1 } else { theme::SP_2 };
    // scroll bar の幅を先に引いて、bar が出ても消えても列が揺れない（毎 frame resize しない）
    let cols = ((ui.available_width() - 2.0 * pad - ui.spacing().scroll.allocated_width().max(ui.spacing().scroll.bar_width)) / cw).floor().max(0.0) as u16;
    // G95: panel の中は panel の下端までの高さ（主領域の scroll を引かない）・Page の中は主領域の残り
    let room = bottom.map_or_else(|| below_cursor(ui), |b| b - ui.cursor().top());
    let rows = term_rows(room, rh, pad, ui.spacing().item_spacing.y);
    if pane.accepts_input && (rows, cols) != (screen.rows, screen.cols) {
        let _ = host.terminal_resize(r, rows, cols); // 誤り（0 列 = 窓が狭い）は host が状態の 1 行に出す
    }
    let focused = pane.accepts_input && ui.memory(|m| m.has_focus(id));
    if focused {
        crate::ui::ax::note_terminal_focus(ui.ctx(), id); // TITLEBAR-ICONS-1101: ⌘1〜9 は Terminal の tab へ（app.rs の keyboard）
    }
    // TEXT-SELECT-1101: drag で選んだ cell の範囲（行 = `screen.lines` の位置・列）。始めの cell と今の cell・両端を含む
    let sel_id = id.with("selection");
    let mut sel: Option<TermSelection> = ui.data(|d| d.get_temp::<Option<TermSelection>>(sel_id)).flatten();
    let sel_fill = ui.visuals().selection.bg_fill;
    // 描いた最初の行（位置と左上）= pointer から cell を引く元
    let mut first_row: Option<(usize, egui::Pos2)> = None;
    // TERM-ONE-1101（CHAT-ONE-1101 の 3・owner 181「terminalごとにchat barがあるのがおかしい」）: IME の変換中の字（preedit）は cursor の cell から grid の格子に
    // 下線つきで描く（`term_cell`・`paint_term_row`）。PTY に送るのは確定（`Ime(Commit)`・`terminal_input_bytes`）だけ。変換の間の Enter・Backspace・矢印は IME の物 = 送らない。
    let pre_id = id.with("preedit");
    let mut preedit: String = if focused { ui.data(|d| d.get_temp(pre_id)).unwrap_or_default() } else { String::new() };
    let composing = !preedit.is_empty();
    if focused {
        ui.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Ime(egui::ImeEvent::Preedit(s)) => preedit.clone_from(s),
                    egui::Event::Ime(egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled) => preedit.clear(),
                    _ => {}
                }
            }
        });
    }
    ui.data_mut(|d| d.insert_temp(pre_id, preedit.clone()));
    let mut cursor_rect = None;
    let ring = if focused && !in_panel { egui::Stroke::new(theme::HAIRLINE, t.focus_ring) } else { egui::Stroke::NONE };
    // COPY-SCROLL-1101: 引いて選んでいる間に grid の上下の外へ出たら scrollback を流して選びを伸ばす（選びの行は scrollback の通しの位置なので流しても切れない）
    let dragging = ui.ctx().is_being_dragged(id);
    let frame = if in_panel { egui::Frame::new().inner_margin(pad) } else { egui::Frame::new().fill(t.surface_2).stroke(ring).inner_margin(pad).corner_radius(CornerRadius::same(theme::RADIUS_SM)) };
    let framed = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0; // 行の間を空けない（tmux の状態の行の背景が続く・show_rows の行の高さ = rh）
        let height = rh * f32::from(screen.rows.max(1));
        let cursor_at = screen.cursor.map(|c| (screen.history + usize::from(c.row), c.col));
        egui::ScrollArea::vertical().id_salt((key, "terminal-scroll")).max_height(height).min_scrolled_height(height).stick_to_bottom(true).auto_shrink([false, false]).show_rows(
            ui,
            rh,
            screen.lines.len(),
            |ui, range| {
                theme::edge_scroll(ui, dragging);
                for i in range {
                    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rh), Sense::hover());
                    first_row.get_or_insert((i, row.min));
                    if let Some((c0, c1)) = sel.and_then(|s| selection_cols(s, i, screen.cols)) {
                        let span = egui::Rect::from_min_max(row.min + Vec2::new(f32::from(c0) * cw, 0.0), row.min + Vec2::new(f32::from(c1) * cw, rh));
                        ui.painter().rect_filled(span, 0.0, sel_fill);
                    }
                    paint_term_row(ui.painter(), row.min, cw, rh, &screen.lines[i], &font, &t);
                    // TERM-RESTORE-1101（owner 189・185「いらない文字」）: 前の起動の画面と新しい shell の間は行の真ん中に細い線 1 本だけ（字は出さない）
                    if screen.rule == Some(i) {
                        ui.painter().hline(row.x_range(), row.center().y, egui::Stroke::new(theme::HAIRLINE, t.border));
                    }
                    if let Some((_, col)) = cursor_at.filter(|(at, _)| *at == i) {
                        let cell = term_cell(row.min, col, cw, rh);
                        if !preedit.is_empty() {
                            // 変換中の字: 下の字を隠して同じ格子に描き、cursor（と IME の候補の窓）は変換中の字の後ろの cell
                            use unicode_width::UnicodeWidthStr;
                            let end = term_cell(row.min, col.saturating_add(preedit.width().max(1) as u16), cw, rh);
                            ui.painter().rect_filled(egui::Rect::from_min_max(cell.min, end.left_bottom()), 0.0, t.surface_2);
                            let run = TermRun { col: 0, text: preedit.clone(), style: TermStyle { underline: true, ..TermStyle::default() } };
                            paint_term_row(ui.painter(), cell.min, cw, rh, &[run], &font, &t);
                            ui.painter().rect_filled(end, 0.0, t.text_primary.gamma_multiply(0.45));
                            cursor_rect = Some(end);
                        } else if focused {
                            ui.painter().rect_filled(cell, 0.0, t.text_primary.gamma_multiply(0.45));
                            cursor_rect = Some(cell);
                        } else {
                            ui.painter().rect_stroke(cell, 0.0, egui::Stroke::new(theme::HAIRLINE, t.text_secondary), StrokeKind::Inside);
                            cursor_rect = Some(cell);
                        }
                    }
                }
            },
        );
    });
    let rect = framed.response.rect;
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    // TEXT-SELECT-1101: drag で cell を選び（Terminal.app と同じ・押した cell から今の cell まで）、⌘C（egui の `Event::Copy`）で選んだ字を写す。
    // click だけなら選択を消す。drag を始めたら grid に focus（⌘C を受ける・打鍵の席も grid）
    let cell_at = |p: egui::Pos2| {
        first_row.map(|(i0, o)| {
            let line = (i0 as f32 + ((p.y - o.y) / rh).floor()).clamp(0.0, screen.lines.len().saturating_sub(1) as f32) as usize;
            let col = ((p.x - o.x) / cw).floor().clamp(0.0, f32::from(screen.cols.saturating_sub(1))) as u16;
            (line, col)
        })
    };
    if resp.drag_started() {
        sel = ui.input(|i| i.pointer.press_origin()).and_then(cell_at).map(|c| (c, c));
        resp.request_focus();
    } else if resp.dragged() {
        if let (Some(s), Some(c)) = (sel.as_mut(), resp.interact_pointer_pos().and_then(cell_at)) {
            s.1 = c;
        }
    } else if resp.clicked() {
        // COPY-SCROLL-1101: ⇧ ＋ click は選びの端をそこまで伸ばす（Terminal.app と同じ）・ただの click は消す
        sel = sel.filter(|_| ui.input(|i| i.modifiers.shift)).and_then(|(from, _)| resp.interact_pointer_pos().and_then(cell_at).map(|to| (from, to)));
    }
    if let Some(s) = sel.filter(|_| resp.has_focus() && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))) {
        ui.ctx().copy_text(selection_text(&screen.lines, s));
    }
    ui.data_mut(|d| d.insert_temp(sel_id, sel));
    let ax = crate::ui::ax::AxScope::Terminal.id(r);
    let text = pane.text.clone();
    ui.ctx().accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::Label);
        n.set_author_id(ax);
        n.set_value(text);
        n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
    let copy = sel.map(|s| move || selection_text(&screen.lines, s));
    terminal_grid_menu(ui, r, pane, &resp, host, copy.as_ref().map(|f| f as &dyn Fn() -> String), false);
    if !pane.accepts_input {
        return;
    }
    if resp.clicked() {
        resp.request_focus();
    }
    let view = pane.view.clone().unwrap_or(InputView::Drawer);
    // W11: 離すのは時間（InputLease::HUMAN_HOLD）で、focus が外れた時ではない（入力欄と同じ）
    if resp.gained_focus() {
        let _ = host.terminal_touch_human(r, view.clone());
    }
    // 錠は窓が後ろでも効かせる（`focused` = Memory の focus）: 後ろで Tab が focus を次の部品（Mirror の面）へ動かさない。送るのは窓が前の時だけ（今まで通り）
    if focused {
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
    }
    if resp.has_focus() {
        let bytes: Vec<u8> = ui.input(|i| i.events.iter().filter(|ev| !(composing && matches!(ev, egui::Event::Key { .. }))).filter_map(|ev| terminal_input_bytes(ev, screen)).flatten().collect());
        if !bytes.is_empty() {
            let _ = host.terminal_keys(r, &bytes); // 送れない理由は host が状態の 1 行に出す
        }
        // IME（日本語の入力）を許し、変換の窓を cursor の cell（変換中は変換中の字の後ろ）に出す
        let cursor_rect = cursor_rect.unwrap_or(egui::Rect::from_min_size(rect.min, Vec2::new(cw, rh)));
        ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect, cursor_rect }));
    }
}

/// L7-ORCA #9: grid の右 click（two-finger click）の menu。押した所に開く（下半分なら上へ）・Esc と外の click と grid の click で閉じる。
/// `Copy`（drag で選んだ所が在る時だけ・TEXT-SELECT-1101）・`Copy all`（画面と scrollback の全部）・動いている間だけ `Paste`（clipboard を grid へ）と `Clear`（⌃L）・
/// Project の下の panel の Terminal（席 = Drawer）だけ `Split Right` / `Split Down` / `Terminate`（Page の Terminal は plan が置いた物なので割らない・閉じない）。
/// `Memory::popup` で開く = WebView を隠す規則に入る（`theme::menu_popup` と同じ）。
/// TERM-XTERM-1101: `selected` = 選んだ字を作る手（選びが無ければ None・grid は押した時だけ作る）・`native` = 中身が xterm の頁（Paste は clipboard を読んで PTY へ・
/// 頁の右 click は `open_grid_menu` が開く）。
fn terminal_grid_menu(ui: &mut egui::Ui, r: &str, pane: &TerminalPane, resp: &egui::Response, host: &mut dyn super::ActionHost, selected: Option<&dyn Fn() -> String>, native: bool) {
    use crate::ui::terminal_panel::PanelAct;
    const MENU_W: f32 = 200.0;
    let menu = resp.id.with("terminal-grid-menu");
    let at = resp.id.with("terminal-grid-menu-at");
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos().or_else(|| ui.ctx().pointer_interact_pos()) {
            open_grid_menu(ui, resp.id, p);
        }
    } else if resp.clicked() && ui.memory(|m| m.is_popup_open(menu)) {
        ui.memory_mut(|m| m.close_popup());
    }
    let Some(p) = ui.data(|d| d.get_temp::<egui::Pos2>(at)) else { return };
    let mut anchor = resp.clone();
    anchor.rect = egui::Rect::from_min_size(p, Vec2::new(MENU_W, 0.0));
    let up = if p.y > ui.ctx().screen_rect().center().y { egui::AboveOrBelow::Above } else { egui::AboveOrBelow::Below };
    let panel = pane.view == Some(InputView::Drawer);
    let picked = egui::popup::popup_above_or_below_widget(ui, menu, &anchor, up, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let item = |ui: &mut egui::Ui, name: &str, v: Variant| theme::button_menu_item(ui, name, "", v).clicked();
        let mut picked = None;
        if selected.is_some() && item(ui, "Copy", Variant::Ghost) {
            picked = Some("copy-selection");
        }
        if item(ui, "Copy all", Variant::Ghost) {
            picked = Some("copy");
        }
        if pane.accepts_input && item(ui, "Paste", Variant::Ghost) {
            picked = Some("paste");
        }
        if pane.accepts_input && item(ui, "Clear", Variant::Ghost) {
            picked = Some("clear");
        }
        if panel {
            ui.separator();
            for (name, key) in [("Split Right", "right"), ("Split Down", "down")] {
                if item(ui, name, Variant::Ghost) {
                    picked = Some(key);
                }
            }
            if item(ui, "Terminate", Variant::Danger) {
                picked = Some("terminate");
            }
        }
        picked
    })
    .flatten();
    let Some(picked) = picked else { return };
    ui.memory_mut(|m| m.close_popup());
    let tab = r.to_owned();
    // 誤りは host が Terminal の状態の 1 行に出す（keys）か、panel の host が Page の上の 1 行に出す（panel の手）
    let _ = match picked {
        "copy-selection" => {
            if let Some(text) = selected {
                ui.ctx().copy_text(text());
            }
            Ok(())
        }
        "copy" => {
            ui.ctx().copy_text(pane.text.clone());
            Ok(())
        }
        // TERM-XTERM-1101: xterm の頁は egui の `Event::Paste` を受けない = clipboard を読んで grid と同じ形（bracketed paste）で PTY へ
        "paste" if native => arboard::Clipboard::new()
            .and_then(|mut c| c.get_text())
            .map_err(|e| format!("Couldn't read the clipboard: {e}"))
            .and_then(|text| match pane.screen.as_ref().and_then(|s| super::terminal_input_bytes(&egui::Event::Paste(text), s)) {
                Some(bytes) => host.terminal_keys(r, &bytes),
                None => Ok(()),
            }),
        "paste" => {
            // egui-winit が clipboard を読んで次の frame に `Event::Paste` を入れる → focus の在る grid が `terminal_input_bytes` で送る
            resp.request_focus();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            Ok(())
        }
        "clear" => host.terminal_keys(r, b"\x0c"),
        "right" => host.terminal_panel_act(r, PanelAct::SplitRight { tab }),
        "down" => host.terminal_panel_act(r, PanelAct::SplitDown { tab }),
        _ => host.terminal_panel_act(r, PanelAct::Close { tab }),
    };
}

/// grid の右 click の menu を `p` に開く（grid の secondary click と xterm の頁の右 click の両方・TERM-XTERM-1101）。
fn open_grid_menu(ui: &mut egui::Ui, resp_id: egui::Id, p: egui::Pos2) {
    ui.data_mut(|d| d.insert_temp(resp_id.with("terminal-grid-menu-at"), p));
    ui.memory_mut(|m| m.open_popup(resp_id.with("terminal-grid-menu")));
}

/// TEXT-SELECT-1101: grid の選択 = (drag を始めた cell, 今の cell)。cell = (`TermScreen::lines` の位置, 列)。どちらが前でもよい（両端を含む）。
/// ponytail: 行の位置は scrollback の先頭からの数なので、scrollback が上限で古い行を捨てると選んだ所が 1 行ずつずれる。困ったら履歴の通し番号で持つ
type TermSelection = ((usize, u16), (usize, u16));

/// 選択のうち `line` 行目に掛かる列の [始め, 終わり)（掛からなければ None）。間の行は端から端（`cols`）まで。
fn selection_cols(s: TermSelection, line: usize, cols: u16) -> Option<(u16, u16)> {
    let (a, b) = if s.0 <= s.1 { s } else { (s.1, s.0) };
    (a.0..=b.0).contains(&line).then(|| (if line == a.0 { a.1 } else { 0 }, if line == b.0 { b.1 + 1 } else { cols }))
}

/// 選んだ cell の字（行ごとに末尾の空白を落とし `\n` で繋ぐ・全角は始めの cell が範囲に在れば入る・幅 0 の結合文字は前の字に付く）。
fn selection_text(lines: &[Vec<TermRun>], s: TermSelection) -> String {
    use unicode_width::UnicodeWidthChar;
    let (a, b) = if s.0 <= s.1 { s } else { (s.1, s.0) };
    let mut out: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate().take(b.0 + 1).skip(a.0) {
        let (c0, c1) = selection_cols(s, i, u16::MAX).unwrap_or((0, 0));
        let (mut text, mut took, mut filled) = (String::new(), false, c0);
        for run in line {
            let mut col = run.col;
            for c in run.text.chars() {
                let w = c.width().unwrap_or(0) as u16;
                if w == 0 {
                    if took {
                        text.push(c);
                    }
                    continue;
                }
                took = (c0..c1).contains(&col);
                if took {
                    // run の間の空きの列（VT が空白を run にしない所）は空白で埋める
                    while filled < col {
                        text.push(' ');
                        filled += 1;
                    }
                    text.push(c);
                    filled = col.saturating_add(w);
                }
                col = col.saturating_add(w);
            }
        }
        out.push(text.trim_end().to_owned());
    }
    out.join("\n")
}

/// grid の 1 行: run ごとに背景の矩形を塗り、文字を cell の位置に置く。ASCII の続きは 1 つの galley（Hack の等幅・字の送り = `cw` = `term_cell_width`）、
/// それ以外の文字（全角・罫線の fallback・絵文字）は 1 文字ずつ自分の cell に置く（fallback の font の幅で後ろの列がずれない）。
fn paint_term_row(p: &egui::Painter, at: egui::Pos2, cw: f32, rh: f32, line: &[TermRun], font: &egui::FontId, t: &Tokens) {
    use unicode_width::UnicodeWidthChar;
    let rgb = |c: TermColor, default: Color32| match c {
        TermColor::Default => default,
        TermColor::Indexed(i) => theme::ansi256(t, i),
        TermColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    };
    for run in line {
        let st = run.style;
        let (mut fg, mut bg) = (rgb(st.fg, t.text_primary), rgb(st.bg, t.surface_2));
        if st.inverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        if st.dim {
            fg = fg.gamma_multiply(0.6);
        }
        let cells: usize = run.text.chars().map(|c| c.width().unwrap_or(0)).sum();
        let x0 = term_cell(at, run.col, cw, rh).min.x;
        if bg != t.surface_2 {
            p.rect_filled(egui::Rect::from_min_size(egui::pos2(x0, at.y), Vec2::new(cells as f32 * cw, rh)), 0.0, bg);
        }
        let line_of = |on: bool| if on { egui::Stroke::new(1.0, fg) } else { egui::Stroke::NONE };
        let format = egui::TextFormat { font_id: font.clone(), color: fg, italics: st.italic, underline: line_of(st.underline), strikethrough: line_of(st.strike), ..Default::default() };
        let put = |x: f32, s: &str| {
            if s.is_empty() {
                return;
            }
            let galley = p.layout_job(egui::text::LayoutJob::single_section(s.to_owned(), format.clone()));
            let pos = egui::pos2(x, at.y);
            if st.bold {
                p.galley(pos + Vec2::new(0.6, 0.0), galley.clone(), fg); // ponytail: 太字の face が Mono に無いので 0.6px ずらして重ねる
            }
            p.galley(pos, galley, fg);
        };
        // 文字 + 後ろの結合文字（幅 0）を 1 つの塊にする。ASCII だけの塊は続けて 1 つの galley、それ以外は塊ごとに自分の cell へ
        let mut clusters: Vec<(String, usize)> = Vec::new();
        for c in run.text.chars() {
            match (c.width().unwrap_or(0), clusters.last_mut()) {
                (0, Some((s, _))) => s.push(c),
                (w, _) => clusters.push((c.to_string(), w)),
            }
        }
        let (mut x, mut ascii, mut ascii_x) = (x0, String::new(), x0);
        for (s, w) in &clusters {
            if s.len() == 1 && s.is_ascii() {
                ascii.push_str(s);
            } else {
                put(ascii_x, &ascii);
                ascii.clear();
                put(x, s);
                ascii_x = x + *w as f32 * cw;
            }
            x += *w as f32 * cw;
        }
        put(ascii_x, &ascii);
    }
}

#[cfg(test)]
mod visible_part_tests {
    use super::*;

    /// MIRROR-FIT-1101（owner 198 の 8「panel は Page を縮めずに被せる」）: 下の panel が被さった Web の面は、見えている所だけ panel の上端で切る
    /// （WebView の大きさの `slot` は替えない）・被さっていない面と panel の無い frame はそのまま・全部被さった面は高さ 1pt 未満（`PageWebViews::place` が隠す）。
    /// 負の対照（未実行）: `visible_part` を `slot.intersect(ui.clip_rect())` に戻す → 2 つ目が 400 の高さのままで赤（interface の commit 0a4806a6 の形）。
    #[test]
    fn mirror_fit_the_panel_cuts_only_what_it_covers() {
        let ctx = egui::Context::default();
        let slot = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), Vec2::new(600.0, 400.0));
        let cover = |top: f32| Some(egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(800.0, 600.0)));
        let mut got = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(800.0, 600.0));
        let _ = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                got.push(visible_part(ui, slot));
                for top in [300.0, 500.0, 20.0] {
                    crate::ui::terminal_panel::set_covering(ctx, cover(top));
                    got.push(visible_part(ui, slot));
                }
            });
        });
        assert_eq!(got[0], slot, "panel の無い frame で切った");
        assert_eq!(got[1], egui::Rect::from_min_max(slot.min, egui::pos2(620.0, 300.0)), "被さった所を切らない（WebView が panel の上に残る）");
        assert_eq!(got[2], slot, "被さっていない面を切った");
        assert!(got[3].height() < 1.0, "全部被さった面を見せる: {:?}", got[3]);
    }
}

#[cfg(test)]
mod term_feel_tests {
    use super::*;

    const AT: egui::Pos2 = egui::pos2(10.0, 10.0);

    /// 2x の画面（Retina）で grid の 1 行を `paint_term_row` で描き、(字の galley の (左端の x, galley) の並び, 列の幅 `cw`) を返す。
    /// font は起動時と同じ（`theme::install`）・fonts は最初の `run` の後でないと読めない（theme.rs の `shell_glyphs_exist_in_loaded_fonts` と同じ形）。
    fn painted(text: &str) -> (Vec<(f32, std::sync::Arc<egui::Galley>)>, f32) {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.set_pixels_per_point(2.0);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        assert_eq!(ctx.pixels_per_point(), 2.0, "前提: 2x");
        let mut cw = 0.0;
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            let font = Type::Mono.font();
            let (w, rh) = ctx.fonts(|f| (term_cell_width(f, &font), f.row_height(&font)));
            cw = w;
            let line = [TermRun { col: 0, text: text.to_owned(), ..TermRun::default() }];
            paint_term_row(&ctx.layer_painter(egui::LayerId::background()), AT, cw, rh, &line, &font, &theme::LIGHT);
        });
        let texts = out.shapes.into_iter().filter_map(|c| if let egui::Shape::Text(t) = c.shape { Some((t.pos.x, t.galley)) } else { None }).collect();
        (texts, cw)
    }

    /// 0.5px（2x で 0.25pt）
    const HALF_PX: f32 = 0.25;

    /// TERM-FEEL-1101（owner 182 通目「pointerと最後の文字との間に一文字ぐらいのスペース」・見本 ref-1716-terminal-line-crop.png）:
    /// 40 字の prompt の後の cursor（40 列目の cell）の左端 = 字の galley の右端（41 字目が来る所）と 0.5px 以内・幅は字の送り 1 つ。
    /// 負の対照: `term_cell_width` の中身を `f.glyph_width(font, 'M')` に戻す → cursor が 40 × 0.22pt ≈ 9pt（18px）右・赤。
    #[test]
    fn term_feel_cursor_sits_right_after_a_40_char_prompt() {
        let prompt = "you@mac ~/projects/shibubu-demo/app % ls";
        assert_eq!(prompt.chars().count(), 40);
        // 41 字目（cursor の所に次に打つ字）を足して描く = galley の 40 字目の x が「右端」
        let (texts, cw) = painted(&format!("{prompt}x"));
        let [(x, g)] = texts.as_slice() else { panic!("ASCII の続きは 1 つの galley のはず: {}", texts.len()) };
        let glyphs = &g.rows[0].glyphs;
        let right = x + glyphs[40].pos.x;
        let cell = term_cell(AT, 40, cw, 0.0);
        assert!((cell.min.x - right).abs() <= HALF_PX, "cursor の左端 {} ≠ 字の右端 {right}（cw {cw}）", cell.min.x);
        assert!((cell.width() - (glyphs[40].pos.x - glyphs[39].pos.x)).abs() <= HALF_PX, "cursor の幅 {} ≠ 字の送り 1 つ", cell.width());
    }

    /// TERM-FEEL-1101: 全角 1 字の後の cursor は 2 cell 後 = 後ろの ASCII の 1 字目の所・その後の字も格子のまま（`あabcd` の `d` = 5 列目）。
    /// 負の対照: `term_cell_width` を `glyph_width('M')` に戻す → 5 列目の cursor が `d` より 0.67pt（1.3px）右・赤。
    #[test]
    fn term_feel_cursor_after_a_wide_char_is_two_cells_on() {
        let (texts, cw) = painted("あabcd");
        let [(wide_x, _), (ascii_x, g)] = texts.as_slice() else { panic!("全角 1 つと ASCII の続き 1 つ: {}", texts.len()) };
        assert!((wide_x - AT.x).abs() <= HALF_PX, "全角は 0 列目");
        let after_wide = term_cell(AT, 2, cw, 0.0).min.x;
        assert!((after_wide - AT.x - 2.0 * cw).abs() < 1e-3, "全角の後の cursor は 2 cell 後");
        assert!((after_wide - ascii_x).abs() <= HALF_PX, "全角の後の cursor {after_wide} ≠ 次の字 {ascii_x}");
        let d = ascii_x + g.rows[0].glyphs[3].pos.x;
        let at_d = term_cell(AT, 5, cw, 0.0).min.x;
        assert!((at_d - d).abs() <= HALF_PX, "5 列目の cursor {at_d} ≠ `d` {d}（cw {cw}）");
    }
}

/// waveform: 数値の配列を縦棒で（working 色・64 高の描画面）。数値が無ければ状態の 1 行。
fn waveform(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    heading(ui, node, cx.t);
    let t = cx.t;
    let Some(data) = content(ui, node, cx, false) else { return };
    let samples: Vec<f32> = data.as_array().into_iter().flatten().filter_map(Value::as_f64).map(|x| x as f32).collect();
    if samples.is_empty() {
        states::inline(ui, RegionState::Partial, "No waveform values");
        return;
    }
    bars(ui, &samples, &t);
}

/// 数値の列を縦棒で（waveform の node と録音の帯の波形・64 高の描画面・一番大きい振れ幅で正規化）。
fn bars(ui: &mut egui::Ui, samples: &[f32], t: &Tokens) {
    if samples.is_empty() {
        return;
    }
    let (w, h) = (ui.available_width(), theme::SP_9);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(theme::RADIUS_SM), t.surface_2, theme::hairline(t), StrokeKind::Inside);
    let peak = samples.iter().fold(0.0_f32, |a, b| a.max(b.abs())).max(1e-6);
    let bar = w / samples.len() as f32;
    for (i, s) in samples.iter().enumerate() {
        let hh = (s.abs() / peak * (h - 2.0 * theme::SP_1)).max(1.0);
        let center = egui::pos2(rect.left() + (i as f32 + 0.5) * bar, rect.center().y);
        ui.painter().rect_filled(egui::Rect::from_center_size(center, Vec2::new((bar * 0.6).max(1.0), hh)), 0.0, t.text_secondary);
    }
}
