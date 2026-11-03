//! W2b: 入力欄（egui の `TextEdit`）が外の AX（VoiceOver・cua-driver・Shibubu の MacGui）から受ける操作の契約。
//! 図 docs/diagrams.md §27。証拠: docs/evidence/integ-2026-10-19/log.md（詰まり 1〜3）・docs/accesskit-2026-10-16.md。
//!
//! 利用者の操作フロー（VoiceOver の人）:
//!   1. VO+→ で Home の入力欄に来る → 見出し（`labelled_by`）が名前として読まれる（例「What do you want to create?」）
//!   2. VO+Space（= AXPress）で欄に入る → 打てる状態になる（caret が出る）
//!   3. 打つ（keyboard。VoiceOver の人は Shibubu を前面にしている）→ AXValue が打った文になる
//!   4. VO+→ で「作る」/「保存」→ VO+Space で送る
//!
//! AX の契約（macOS の AX 属性 / action → accesskit_macos 0.18.1 → egui 0.31.1 → Shibubu）:
//!
//! | 外から来る物 | accesskit の Action | 今（W2b 前） | W2b の後 |
//! |---|---|---|---|
//! | `AXFocused = true`（set） | `Focus` | 効く（egui `Memory::id_requested_by_accesskit`。integ log 27・31 で実測） | 同じ（触らない） |
//! | `AXPress`（kAXPressAction） | `Click` | **効かない**: egui の TextEdit は `FAKE_PRIMARY_CLICKED` を focus の判定の前に外し（text_edit/builder.rs:575）、focus は pointer の位置がある時だけ | **効く**: [`AxTextInput::ax_text_input`] が `Click` の要求を見て `request_focus` する（`src/ui` の全部の TextEdit に付けた。旧 CompatCanvas の `app.rs` の 9 欄は対象外） |
//! | `AXValue = "…"`（set） | なし | **受けない**: accesskit_macos 0.18.1 の `setAccessibilityValue:` は空の本体（node.rs:517）。egui の TextEdit も `SetValue` を扱わない | **macOS では Unsupported のまま**（偽の成功を返さない）。app 側は `SetValue` を受ける口を持つ（unit で確かめた）が、accesskit_macos が写さないので macOS からは届かない。値は keyboard で入れる |
//! | `kAXConfirmAction` | なし | 受けない（accesskit_macos が写さない） | **Unsupported のまま**。送るのは「保存」/「作る」button の `AXPress` |
//! | `AXValue`（get） | — | 欄の今の文 | 同じ |
//! | `AXTitle` / `AXDescription`（get） | — | 見出しの文（`labelled_by` を付けた欄だけ） | 全部の欄に付く（renderer の欄は `labelled_by`・名前の無い command_input は node の label か「入力」・shell の欄は `theme::ax_name`） |
//!
//! 背景からの文字入力: `AXValue` の set では入らない（上の表）。cua-driver の `type_text` は CGEvent で前面を取った
//! （docs/accesskit-2026-10-16.md）。前面を変えずに入れる道は 2 つで、どちらも未測（W2b-3 で測って決める）:
//!   (a) 製品は触らない: `AXFocused = true` の後、試験の道具が pid 宛ての keyboard 事象（CGEventPostToPid）を送る。
//!       非 active の app の窓に keyDown が届くかは分かっていない
//!   (b) `[patch.crates-io]` で accesskit_macos 0.18.1 の `setAccessibilityValue:` を `Action::SetValue`（`ActionData::Value`）に写し、
//!       （依存の差し替え = owner の承認が要る）。受ける側の [`AxTextInput::ax_text_input`] は既に `SetValue` で String を置き換える
//! どちらも通らない間、文字を入れる段の実機は owner の手（VoiceOver・前面）で行い、背景の試験は「未測」と書く。

use eframe::egui;

/// 入力欄の `Response` に、AX の `AXPress`（accesskit `Click`）で focus を移す口と、`SetValue` で文を置き換える口を足す。
///
/// 契約:
/// - `ui.add(TextEdit)` の直後に 1 回呼ぶ（`labelled_by` の前後どちらでもよい）。`text` は TextEdit に渡した String。戻り値は同じ `Response`。
/// - その frame の入力に `ActionRequest { action: Click, target: self.id }` があれば `request_focus`。
///   次の frame から keyboard の文字（`Event::Text`）がこの欄に入る。
/// - `ActionRequest { action: SetValue, target: self.id, data: Some(ActionData::Value(v)) }` があれば `text` を `v` に置き換え、`changed` を立てる
///   （呼び手は今と同じく `changed()` を見て draft に入れる）。**macOS では今は届かない**: accesskit_macos 0.18.1 が
///   `setAccessibilityValue:` を写さない（上の表）。届くのは accesskit の他の口と unit 試験だけ。上の (b) を採ればそのまま効く。
/// - 欄が打てない（`!self.enabled()`）時は何もしない（理由つき disabled の欄に focus も値も渡さない）。
/// - `Focus` の要求は egui が既に扱うので、ここでは見ない。pointer の click・keyboard の Tab の挙動は変えない。
/// - UI-5a: 呼ぶたびに、この欄の id と「文字が在るか」（`!text.is_empty()`）を egui の temp data に書く（`undo_for_project` が ⌘Z の持ち主を決める時に読む・
///   `src/ui` の 13 欄はどれもこれを通るので、欄ごとの登録は要らない）。
/// Wave: W2b-3, UI-5a
pub trait AxTextInput {
    fn ax_text_input(self, ui: &egui::Ui, text: &mut String) -> Self;
}

impl AxTextInput for egui::Response {
    fn ax_text_input(mut self, ui: &egui::Ui, text: &mut String) -> Self {
        use egui::accesskit::{Action, ActionData};
        ui.data_mut(|d| d.insert_temp(self.id.with(FILLED), !text.is_empty()));
        if !self.enabled() {
            return self;
        }
        let (press, set) = ui.input(|i| {
            let set = i.accesskit_action_requests(self.id, Action::SetValue).filter_map(|r| match &r.data {
                Some(ActionData::Value(v)) => Some(v.to_string()),
                _ => None,
            });
            (i.has_accesskit_action_request(self.id, Action::Click), set.last())
        });
        if press {
            self.request_focus();
        }
        if let Some(v) = set.filter(|v| v != text) {
            *text = v;
            self.mark_changed();
        }
        self
    }
}

/// UI-5a: ⌘Z / ⇧⌘Z を project の Undo / Redo に回すか（SPEC §6 Y2・Focus owner・Done 4）。focus の在る widget が、文字の在る入力欄
/// （この frame か前の frame に `AxTextInput::ax_text_input` を通った欄）の時だけ false（その欄の egui の undo に譲る）。focus が無い・tab や button・
/// Terminal の grid・空の欄に在る時は true。今の門 `!ctx.wants_keyboard_input()`（何かに focus が在れば偽 = Terminal から ⌘Z が効かない）を置き換える。
/// `ax_text_input` を通らない TextEdit（旧 CompatCanvas の app.rs の欄・どの View にも出る Import from GitHub の 4 欄）は文字が在るか分からないので
/// false（欄が持つ = 前の門と同じ）。TextEdit かどうかは egui が欄ごとに残す `TextEditState` で見る。
/// 呼ぶ例（app.rs の keyboard）: `let project = crate::ui::ax::undo_for_project(ctx); … project && i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)`。
/// 保証しない: WebView（WKWebView）が focus を持つ時（egui に key が届かない = その頁の ⌘Z）・Mirror の面の ⌘Z（F12 の 1 行は後）。
/// Wave: UI-5a
pub fn undo_for_project(ctx: &egui::Context) -> bool {
    let Some(id) = ctx.memory(|m| m.focused()) else { return true };
    match ctx.data(|d| d.get_temp::<bool>(id.with(FILLED))) {
        Some(filled) => !filled,
        None => egui::TextEdit::load_state(ctx, id).is_none(),
    }
}

/// `ax_text_input` が欄ごとに「文字が在るか」を書く egui の temp の鍵（欄の id と組む）。
const FILLED: &str = "kangae-ax-text-filled";

/// TITLEBAR-ICONS-1101（operator の決め: ⌘1〜9 は Terminal に focus が在る時は Terminal の tab・他は n 枚目の Page・2 種類の Terminal で分けない）:
/// keyboard の focus が egui の grid の Terminal に在るか（grid が focus を持つ frame ごとに `note_terminal_focus` で自分の id を書き、今の focus と比べる。
/// grid が描かれなくなれば egui が focus を外すので古い id は当たらない）。xterm の頁（WebView）の focus は egui に見えない = app.rs の `term_keys` が見る。
/// 呼ぶ例（app.rs の keyboard）: `let in_terminal = self.term_keys || crate::ui::ax::terminal_focused(ctx);`
pub fn terminal_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|f| ctx.data(|d| d.get_temp::<egui::Id>(egui::Id::new(TERMINAL_FOCUS))) == Some(f))
}

/// `terminal_focused` の書き手（`components::terminal_grid` が focus の frame に呼ぶ）。
pub fn note_terminal_focus(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(TERMINAL_FOCUS), id));
}

const TERMINAL_FOCUS: &str = "kangae-terminal-focus";

/// W2c: AX の identifier の名前空間（L1 の検査の範囲・YC_ACCEPTANCE §4）。identifier は `<prefix>:<id>`（例 `surface-status:docs`）。
/// 例: 描く側は面の状態の node の AX の identifier（accesskit の author_id）に `AxScope::SurfaceStatus.id("docs")` を付ける・試験の側は `AxScope::parse("terminal:dev")` で部分木を分ける。
/// 保証する: 名前はこの表の 9 つだけで、L1 の成功の語の検査から外すのは `excluded_from_l1` が true の 5 つだけ（W14b の `Usable`・W13c の `Slot` は L1 の検査に残す）。保証しない: 描く側が実際に付けている事（W2c の実装と tally.py の L1 ① で見る）。
/// Wave: W2c
#[allow(dead_code)] // W2c の実装で renderer と portal_frame が付ける
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxScope {
    /// 面の状態の 1 行（値 = verified / unverified / unavailable / ended）。成功の語を出してよいのはここと Toast だけ
    SurfaceStatus,
    /// 面に紐づく toast
    Toast,
    /// PTY の出力（利用者の data・L1 の範囲の外）
    Terminal,
    /// Web の中身（L1 の範囲の外）
    WebView,
    /// Native Direct と Mirror の枠の中（元の app の画面・L1 の範囲の外）
    SurfaceFrame,
    /// 生成した部品が出す利用者のデータ（node の key・L1 の範囲の外）
    Data,
    /// 状態の語の説明（凡例・例 `legend:connections` の「Connected = 認証済み」）。何かが済んだと言う行ではない（L1 の範囲の外・W19-1・operator 2026-10-23）
    Legend,
    /// W14b: 使える検査の面ごとの 1 行（値 = ready / checking / not_ready・id = 席の id）。成功の語は出さない（L1 の範囲の中）
    /// Wave: W14b
    Usable,
    /// W13c: 配置の型の区画の容れ物（id = main / side / top / bottom / drawer）。中の部品の scope はそのまま（L1 の範囲の中）
    /// Wave: W13c
    Slot,
}

#[allow(dead_code)]
impl AxScope {
    pub const ALL: [AxScope; 9] = [AxScope::SurfaceStatus, AxScope::Toast, AxScope::Terminal, AxScope::WebView, AxScope::SurfaceFrame, AxScope::Data, AxScope::Legend, AxScope::Usable, AxScope::Slot];

    pub const fn prefix(self) -> &'static str {
        match self {
            AxScope::SurfaceStatus => "surface-status",
            AxScope::Toast => "toast",
            AxScope::Terminal => "terminal",
            AxScope::WebView => "webview",
            AxScope::SurfaceFrame => "surface-frame",
            AxScope::Data => "data",
            AxScope::Legend => "legend",
            AxScope::Usable => "usable",
            AxScope::Slot => "slot",
        }
    }

    /// `<prefix>:<id>`。
    pub fn id(self, id: &str) -> String {
        format!("{}:{id}", self.prefix())
    }

    /// `<prefix>:<id>` を分ける（表に無い prefix・id が空は None）。
    pub fn parse(identifier: &str) -> Option<(AxScope, &str)> {
        let (p, id) = identifier.split_once(':')?;
        let scope = AxScope::ALL.into_iter().find(|s| s.prefix() == p)?;
        (!id.is_empty()).then_some((scope, id))
    }

    /// L1 の成功の語の検査から部分木ごと外す（利用者・元の app の文字なので）。
    pub const fn excluded_from_l1(self) -> bool {
        matches!(self, AxScope::Terminal | AxScope::WebView | AxScope::SurfaceFrame | AxScope::Data | AxScope::Legend)
    }
}

#[cfg(test)]
mod undo_owner_tests {
    use super::*;

    /// 1 frame 描いて、欄（`ax_text_input` を通す）に focus を置くかを選ぶ。返り値はその欄の id。
    fn frame(ctx: &egui::Context, text: &mut String, focus: bool) -> egui::Id {
        let mut id = egui::Id::NULL;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(text)).ax_text_input(ui, text);
                if focus {
                    r.request_focus();
                }
                id = r.id;
            });
        });
        id
    }

    /// UI-5a Done 4 の芯: focus が無い・空の欄なら project・文字の在る欄なら欄の undo。Terminal の grid（TextEdit でない focus の widget）でも project の Undo
    /// （今の門 `!wants_keyboard_input()` はそこで偽 = Terminal から ⌘Z が効かなかった・その差を同じ試験で見る）。
    #[test]
    fn ui5a_undo_goes_to_a_field_only_when_it_holds_text() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        frame(&ctx, &mut text, false);
        assert!(undo_for_project(&ctx), "focus が無い = project の Undo");
        frame(&ctx, &mut text, true);
        frame(&ctx, &mut text, true);
        assert!(undo_for_project(&ctx), "空の欄に focus = project の Undo（SPEC Y2）");
        text.push_str("add a Tests page");
        frame(&ctx, &mut text, true);
        assert!(!undo_for_project(&ctx), "文字の在る欄に focus = 欄の undo");
        // Terminal の grid の形: 文字の欄を通らない focus の widget（click を取る矩形）。文字の在る欄は描いたまま
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = ui.add(egui::TextEdit::singleline(&mut text)).ax_text_input(ui, &mut text);
                    let (_, grid) = ui.allocate_exact_size(egui::vec2(200.0, 80.0), egui::Sense::click());
                    grid.request_focus();
                });
            });
        }
        assert!(ctx.wants_keyboard_input(), "前提: grid が keyboard の focus を持つ（今の門ならここで ⌘Z を取らない）");
        assert!(undo_for_project(&ctx), "Terminal の grid に focus = project の Undo（Done 4）");
    }

    /// UI-5a 反証: `ax_text_input` を通さない欄（旧 CompatCanvas の app.rs の欄・どの View にも出る Import from GitHub の 4 欄）は文字が在るか分からないので、
    /// focus が在れば欄が ⌘Z を持つ（UI-5a の前の門 `wants_keyboard_input` と同じ）。Project の中で欄に打っている間に project の revision を戻さない。
    #[test]
    fn ui5a_undo_stays_with_a_field_that_does_not_report_its_text() {
        let ctx = egui::Context::default();
        let mut text = String::from("owner/repository");
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| ui.add(egui::TextEdit::singleline(&mut text)).request_focus());
            });
        }
        assert!(ctx.wants_keyboard_input(), "前提: 欄が keyboard の focus を持つ");
        assert!(!undo_for_project(&ctx), "ax_text_input を通さない欄に focus = 欄の undo（project の Undo を走らせない）");
    }
}
