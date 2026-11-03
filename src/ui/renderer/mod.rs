//! native renderer（UI-005 / 006 / 007 / 009 の最小 / 014）: AppDefinition v3 の UiNode 木を egui に描く。
//! - 中身を作らない: binding が使えて host が data を返した時だけ行を描く。それ以外は §11 の状態語（`states`）
//! - 押せない button は理由つき disabled（DESIGN §4.1・CD-02 の matrix）。押したら `ActionHost::invoke` に渡すだけ（検証と実行は runtime）
//! - node の余白を click すると 1px の outline（+4px gap・§6.3）と inspector（この file の `inspector_rows` / `draw_inspector`・末尾は幅の解決の `layout`）
//! - 色・文字・余白は `crate::ui::theme`、状態語は `crate::ui::states` から引く（ここで hex も状態語も作らない）
//!
//! 他 unit が使う公開 API: `ActionHost`, `AppState`（`Draft` / `RowPick`）, `render`。本番の ActionHost は src/generated.rs の `Host`（runtime に繋ぐ）。
//! tests/renderer.rs は `#[path]` で `mod domain;` と `mod ui;` を test crate の root に置いて取り込む（`crate::domain` / `crate::ui` がそのまま解決する）。

pub mod bindings;
pub mod components;
// `pub mod layout`（幅の解決）はこの file の末尾の inline module（FX3 refactor で renderer/layout.rs を寄せた）

use crate::domain::action::{ActionInvocation, ActionReadiness};
use crate::domain::app::{AppDefinitionV3, AppReadiness};
use crate::domain::binding::DataBinding;
use crate::domain::project::{PageDefinition, SurfaceReceipt, SurfaceSpec};
use crate::domain::terminal::{InputLease, InputOwner, InputView, RunReceipt, TermScreen};
use crate::domain::ui::UiNode;
use crate::ui::shell;
use crate::ui::states::{self, RegionState};
use crate::ui::theme::{self, Tokens};
use eframe::egui::{self, CornerRadius, Pos2, Rect, Stroke, StrokeKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

/// W12W-1: Page の中に置く面（`ActionHost::embeds`）に予約する矩形の最小の高さ（points）。Excalidraw の toolbar と canvas が両方見える高さ。
pub const EMBED_MIN_HEIGHT: f32 = 360.0;

/// renderer が app を動かす相手。runtime が実装し、統合役が繋ぐ（test は fake）。
/// 既定の実装は「何も知らない」: 定義の readiness・data 無し・runtime の状態無し。
pub trait ActionHost {
    /// runtime が今知っている action の readiness（接続した・切れた 等）。None なら定義の値を使う。
    fn readiness(&self, _action_ref: &str) -> Option<ActionReadiness> {
        None
    }
    /// binding の中身。None か空なら、その region は「まだデータがありません」を出す（中身を作らない）。
    fn data(&self, _binding: &DataBinding) -> Option<Value> {
        None
    }
    /// runtime だけが知っている region の状態（読み込み中・実行中・確認待ち・失敗・検証済み 等）と補足の文。
    /// None なら定義（binding の readiness）と data から決める。実行中・読み込み中・確認待ちの node の button は押せない。
    fn region(&self, _node_key: &str) -> Option<(RegionState, String)> {
        None
    }
    /// UI-6a（sendback 60.1）: この node の button を押し直させない印（状態の語とは別）。true = 送ったか分からない手（`Outcome::EffectUnknown`）か
    /// 承認待ち（`Invoked::NeedsApproval`）で、押し直すと同じ手が 2 回届く（重複の送信）。`bindings::gate` がこれも見て押せなくし、
    /// generated の `Host` が上書きする（EffectUnknown の語を `Stale` に替えても押し直せない・▣ は承認の button が同じ行に在る時だけ = sendback 31）。
    /// 既定（試験の fake・本体の前の Host）は false = 今の振る舞い（押せないのは region の Running・Loading・Blocked だけ）。
    /// 呼ぶ側の例: `if host.locked(&node.key) { return Err(Gate { state: Some(state), reason: detail }) }`。保証しない: 手が本当に届いたか（正は Runs の記録）。
    /// Wave: UI-6a
    fn locked(&self, _node_key: &str) -> bool {
        false
    }
    /// W3: `source_surface` の `props.surface_ref` が指す面の最新の読み戻し（`surface_receipts` の surface ごとの最新）。
    /// renderer はこれを `surface_status_line` で 1 行にし、AX の identifier `surface-status:<surface_id>`（`AxScope::SurfaceStatus`）を付けて出す。
    /// 既定（と、まだ読んでいない面）は None =「未確認」と出す。成功の語を出す根拠にしない。
    fn surface_receipt(&self, _surface_id: &str) -> Option<SurfaceReceipt> {
        None
    }
    /// W16-1: Native Direct で重ねる面の枠の今。Some なら `source_surface` は `portal_frame::show` で枠を描き、帯の下の slot を
    /// `surface_placed` で返す（枠の AX の identifier は `surface-frame:<surface_id>`・bounds は slot）。None（既定・feature `native_direct` 無し・
    /// 重ねない面）は W3 の `surface-status` の 1 行のまま。保証しない: 枠に窓が本当に重なっている事（正は試験の側の CGWindowList）。
    fn surface_portal(&self, _surface_id: &str) -> Option<crate::ui::portal_frame::PortalView> {
        None
    }
    /// W16-1: 描いた枠の画面座標（`at.slot` は global の point・`ui::portal_frame::slot_on_screen`）と、帯で押された次の一手。
    /// 呼ぶ側（`generated::Host`）は貯めるだけで、`draw_page` が描いた後に `NativeSurfaces::on_frame` へ流す。既定は捨てる。
    #[cfg_attr(not(feature = "native_direct"), allow(dead_code))]
    fn surface_placed(&mut self, _surface_id: &str, _at: crate::surface::Host, _action: Option<crate::ui::portal_frame::PortalAction>) {}
    /// W12W-1: この面（`surface_ref`）を Page の中に置く（今は WebView の面だけ）なら true。既定は false = 今まで通り 1 行と状態だけ。
    /// 例: renderer の `source_surface` が `if cx.host.embeds(sid) { let (slot, _) = ui.allocate_exact_size(..); cx.host.place_surface(sid, slot, visible_part(ui, slot)) }`。
    /// 保証する: true の時 renderer は残りの幅 × 残りの高さ（最小 `EMBED_MIN_HEIGHT`）の矩形を予約し、毎 frame `place_surface` を 1 回呼ぶ。
    /// 保証しない: その矩形に頁が出ている事（`surface_receipt` の 1 行で言う）。
    fn embeds(&self, _surface_id: &str) -> bool {
        false
    }
    /// W12W-1: この frame に予約した矩形（egui の points・窓の内側の左上が原点）。呼ばれなかった面は、その frame の終わりに呼ぶ側が隠す（Page を移った）。
    /// MIRROR-FIT-1101: `slot` = 面の矩形の全体（WebView はこの大きさ・縮めない）・`shown` = その内の見えている所（scroll の clip の内・下の panel の上 =
    /// `components::visible_part`）。子の NSView は egui の clip も panel の Area も受けないので、呼ぶ側は `shown` の外を切り抜く（`AppKitOp::WebClip`）。
    fn place_surface(&mut self, _surface_id: &str, _slot: Rect, _shown: Rect) {}
    /// W12W-B: Browser の上の欄で Enter。既定は何もしない。
    fn browser_go(&mut self, _surface_id: &str, _input: &str) {}
    /// W12W-B: 前の頁 / 次の頁 / 読み直し。既定は何もしない。
    fn browser_back(&mut self, _surface_id: &str) {}
    fn browser_forward(&mut self, _surface_id: &str) {}
    fn browser_reload(&mut self, _surface_id: &str) {}
    /// W12W-B: 題の帯 / 端の drag の終わり。既定は何もしない。
    fn browser_frame(&mut self, _surface_id: &str, _x: f64, _y: f64, _w: f64, _h: f64) {}
    /// W12W-B: 開いている頁の今の URL（欄の初期値）。既定は None。
    fn browser_url(&self, _surface_id: &str) -> Option<String> {
        None
    }
    /// 中身が時刻だけで次に替わるまで（本番は動いている timer の秒の替わり目・`AppRuntime::next_change`）。`render` はこの後の frame を頼む。
    /// 既定は None = 入力か invoke でしか替わらない（idle の frame を頼まない）。
    fn next_change(&self) -> Option<std::time::Duration> {
        None
    }
    /// button / form / command_input / chat / recorder / source_surface / media から。
    /// renderer は送る前に input を runtime と同じ `check_input` で見て、通らない button は理由つき disabled にする（gate）。
    /// それでも `validate_invocation`（domain）と実行は runtime。renderer は成功を演出しない。
    fn invoke(&mut self, inv: ActionInvocation);

    /// W5: Page の `terminal` node（`props.terminal_ref`）に描く物。None なら「この Terminal は Project の Page にだけ置けます」の 1 行。
    /// 本番（generated.rs の `Host`）は `Generated::ensure_terminal` で session を引き（無ければ起こす・動いていれば起こさない）、
    /// その session の `TerminalManager::screen` の写しを返す（W10）。毎 frame 呼ばれる（待たない）。
    /// 保証しない（ここは表示の口）: 出力が本物である事（L3 の判定は試験の側の nonce と `ps`）。
    fn terminal(&mut self, _terminal_ref: &str) -> Option<TerminalPane> {
        None
    }
    /// G92: 足したばかりの部品（`terminal_ref` か面の id）を、この frame で見える所まで送るか（1 回だけ true・読んだら消える）。
    /// 本番は `Add terminal` / `Add browser` / `Show terminal` の後の 1 frame。renderer は true の部品を `scroll_to_rect` で主領域の中に入れ、Terminal なら入力欄に focus。
    /// 既定は false（何も送らない）。
    fn take_reveal(&mut self, _part_ref: &str) -> bool {
        false
    }
    /// G88b: Page の `recorder` node（`props.recorder_ref`）の帯の今。None = その node は今の action の button（`recorder` の元の描き方）。毎 frame 呼ばれる（待たない）。
    fn take(&mut self, _recorder_ref: &str) -> Option<TakePane> {
        None
    }
    /// G88b: 帯で押された手。Err は帯の下の 1 行。既定は何もせず Err（偽の「録った」を出さない）。
    fn take_act(&mut self, _recorder_ref: &str, _act: TakeAct) -> Result<(), String> {
        Err("No recorder on this page".into())
    }
    /// W5: 入力欄の 1 行を送る（本番は `TerminalManager::write(key, line + "\n")`）。Err は Terminal の下に出す 1 行（D-e）。
    /// 既定は送らずに Err（偽の「送った」を出さない）。
    fn terminal_send(&mut self, _terminal_ref: &str, _line: &str) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W5: 終わった・前の起動の Terminal を同じ spec（同じ session id）で起こし直す（「新しく起こす」の button）。Err は 1 行。
    fn terminal_restart(&mut self, _terminal_ref: &str) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W10: 画面（grid）に focus がある間の打鍵・貼り付け・IME の確定を、`terminal_input_bytes` で byte にした物をそのまま送る
    /// （本番は `TerminalManager::write(key, bytes)`。`terminal_send` と同じく人の入力 = W11 の lease の Human の口）。
    /// Err は Terminal の下に出す 1 行。既定は送らずに Err。
    fn terminal_keys(&mut self, _terminal_ref: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W10: 部品が描ける大きさ（文字の行と列）。renderer は `TerminalPane::screen` の rows / cols と違う時だけ呼ぶ。
    /// 本番は `TerminalManager::resize`（PTY と VT を同じ呼び出しで・0 は拒否）。同じ session を 2 か所に出す時は最後に呼んだ大きさが勝つ。
    /// Err は 1 行（既定は Err・renderer は Err でも描き続ける）。
    fn terminal_resize(&mut self, _terminal_ref: &str, _rows: u16, _cols: u16) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W11: owner / view を指定して PTY に書く（本番は `TerminalManager::write_as`）。
    /// 人の口は今までどおり `terminal_send` / `terminal_keys`（本体の commit で lease を通す）。
    /// 既定は送らずに Err（偽の「送った」を出さない）。LeaseDenied の 1 行も Err で返す。
    fn terminal_write(
        &mut self,
        _terminal_ref: &str,
        _owner: InputOwner,
        _view: &InputView,
        _bytes: &[u8],
    ) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W11: 人が打ち始めた。本番は `TerminalManager::touch_human`。既定は Err。
    fn terminal_touch_human(&mut self, _terminal_ref: &str, _view: InputView) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W11: 人が離した。本番は `TerminalManager::release_human`。既定は Err。
    fn terminal_release_human(&mut self, _terminal_ref: &str, _view: &InputView) -> Result<(), String> {
        Err("No terminal on this page".into())
    }
    /// W11: 今の入力の lease。無い・未配線は None（Agent が書いてよいかは呼ぶ側が `may_write` で見る）。
    fn terminal_lease(&self, _terminal_ref: &str) -> Option<InputLease> {
        None
    }
    /// TERM-XTERM-1101（owner 204 の 4「普通のterminalをshibubuで使えないの？orcaみたいに」）: この Terminal の中身を host の描き手（xterm.js の WebView）に任せる。
    /// renderer は grid の代わりに枠を取り（`rect` = 全体・`shown` = その内の見えている所〈scroll の clip〉）、毎 frame ここに渡す。
    /// `focus` = Show terminal / New terminal の後の 1 frame（そのまま打てるように頁へ focus）。
    /// Some = host が描く（renderer は grid を描かず、`TermPlaced::menu` が在れば grid と同じ右 click の menu を開く）・None = 描き手が無い（試験・webview の無い build・
    /// 頁を開けなかった Terminal）→ 今まで通り egui の grid（`terminal_grid`）。
    /// 本番（`TermHost`・`Host`）は `TermViews::place` に積み、frame の終わりの `Generated::term_sync` が WebView を開く / 合わせる / 出力を流す。
    /// 例: `if let Some(p) = host.terminal_place(r, rect, rect.intersect(ui.clip_rect()), reveal) { /* menu だけ */ } else { terminal_grid(..) }`
    fn terminal_place(&mut self, _terminal_ref: &str, _rect: egui::Rect, _shown: egui::Rect, _focus: bool) -> Option<TermPlaced> {
        None
    }
    /// L7-ORCA #9: grid の右 click の `Split Right` / `Split Down` / `Terminate`（Project の下の panel の Terminal だけ）。
    /// 本番は panel の host が受けて、その frame の後に `Generated::panel_act` へ渡す。既定は Err（Page の Terminal は割らない）。
    fn terminal_panel_act(&mut self, _terminal_ref: &str, _act: crate::ui::terminal_panel::PanelAct) -> Result<(), String> {
        Err("No terminal panel here".into())
    }
    /// W11: Project の RunReceipt（Tests Page が描く。新しい順）。まだ無い・未配線は空。
    /// 保証しない（ここは表示の口）: 失敗を成功と出さない事（それは `RunReceipt::status_line`）。
    fn run_receipts(&self, _project_id: &str) -> Vec<RunReceipt> {
        Vec::new()
    }
}

/// この frame の Esc が app の物か（overlay・palette を閉じる・確かめを取り消す・node の選択を外す）。呼ぶのは app の `keyboard` と `render`。
/// Esc を手放さない widget（`set_focus_lock_filter` の escape・今は Terminal の grid だけ）が focus を持つ間は、その widget の物で false
/// （grid は 0x1b を PTY へ送る・vim の Esc で選択や overlay を閉じない）。egui は Esc を手放す widget の focus を frame の頭で外すので、
/// Esc の frame に focus が残っているのは手放さない widget だけ（app の Esc は文字の欄に focus があっても効く）。
pub fn escape_for_app(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape)) && ctx.memory(|m| m.focused().is_none())
}

/// W10: grid に focus がある間の egui の入力 1 つを、PTY に送る byte にする（純関数・結果を `ActionHost::terminal_keys` に渡す）。
/// 使い方: `for ev in ui.input(|i| i.events.clone()) { if let Some(b) = terminal_input_bytes(&ev, screen) { host.terminal_keys(r, &b)?; } }`
/// 保証する: `Text` と IME の確定（`Ime(Commit)`）は UTF-8 のまま・Enter = `\r`・Backspace = 0x7f・Esc = 0x1b・Tab = `\t`・Ctrl + 英字 = C0（Ctrl-C = 0x03）・
/// 矢印は `screen.app_cursor` の時 `ESC O A`〜、それ以外 `ESC [ A`〜・Home / End / PageUp / PageDown / Delete は xterm の列・
/// `Paste` は `screen.bracketed_paste` の時 `ESC [200~ … ESC [201~` で包む。⌘ の付いた key・離した key・mouse は None（Shibubu の short cut に譲る）。
/// 修飾の無い（Shift だけの）英字・数字・空白の `Key` は None（文字は egui が別に出す `Text` で送る・同じ文字を 2 度送らない）。
/// 貼り付けは bracketed の時は中の ESC を落とし（貼った文で包みを閉じさせない）、そうでない時は改行を CR にする（xterm・alacritty と同じ）。
/// Shift / Alt / Ctrl の付いた矢印・Home・End は `ESC [1;{1+Shift+2Alt+4Ctrl}A` の形・Shift+Tab は `ESC [Z`・F1〜F12 も xterm の列。
/// 保証しない: IME の変換中（preedit）の送信（TERM-ONE-1101: 表示は `terminal_grid` が cursor の cell に描く・送るのは確定だけ）・mouse の報告・Alt を Meta にする設定（Option の文字は egui が `Text` で出す）。
pub fn terminal_input_bytes(ev: &egui::Event, screen: &TermScreen) -> Option<Vec<u8>> {
    use egui::{Event, ImeEvent, Key};
    let bytes = match ev {
        Event::Text(t) | Event::Ime(ImeEvent::Commit(t)) => t.as_bytes().to_vec(),
        Event::Paste(t) if screen.bracketed_paste => format!("\x1b[200~{}\x1b[201~", t.replace('\x1b', "")).into_bytes(),
        Event::Paste(t) => t.replace("\r\n", "\r").replace('\n', "\r").into_bytes(),
        Event::Key { key, pressed: true, modifiers: m, .. } if !m.mac_cmd => {
            let mods = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
            // 矢印・Home・End: 修飾が無ければ DECCKM で ESC O / ESC [ を選ぶ・修飾があれば ESC [1;m x
            let cursor = |c: char| match (mods, screen.app_cursor) {
                (1, true) => format!("\x1bO{c}"),
                (1, false) => format!("\x1b[{c}"),
                _ => format!("\x1b[1;{mods}{c}"),
            };
            // PageUp 等: ESC [ n ~ ・修飾があれば ESC [ n ; m ~
            let tilde = |n: u8| if mods == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{mods}~") };
            let s = match key {
                Key::ArrowUp => cursor('A'),
                Key::ArrowDown => cursor('B'),
                Key::ArrowRight => cursor('C'),
                Key::ArrowLeft => cursor('D'),
                Key::Home => cursor('H'),
                Key::End => cursor('F'),
                Key::Insert => tilde(2),
                Key::Delete => tilde(3),
                Key::PageUp => tilde(5),
                Key::PageDown => tilde(6),
                Key::F1 | Key::F2 | Key::F3 | Key::F4 => {
                    let c = ['P', 'Q', 'R', 'S'][[Key::F1, Key::F2, Key::F3, Key::F4].iter().position(|k| k == key).unwrap_or(0)];
                    if mods == 1 { format!("\x1bO{c}") } else { format!("\x1b[1;{mods}{c}") }
                }
                Key::F5 => tilde(15),
                Key::F6 => tilde(17),
                Key::F7 => tilde(18),
                Key::F8 => tilde(19),
                Key::F9 => tilde(20),
                Key::F10 => tilde(21),
                Key::F11 => tilde(23),
                Key::F12 => tilde(24),
                Key::Tab if m.shift => "\x1b[Z".into(),
                Key::Tab => "\t".into(),
                Key::Enter => "\r".into(),
                Key::Backspace => "\x7f".into(),
                Key::Escape => "\x1b".into(),
                // Ctrl + 英字 = C0（Ctrl-C = 0x03）・Ctrl + [ \\ ] Space。Alt も付けば ESC を前に（Meta）
                k if m.ctrl => {
                    let c = match k {
                        Key::Space => 0,
                        Key::OpenBracket => 0x1b,
                        Key::Backslash => 0x1c,
                        Key::CloseBracket => 0x1d,
                        k => match k.name().as_bytes() {
                            [l @ b'A'..=b'Z'] => l - b'A' + 1,
                            _ => return None,
                        },
                    };
                    return Some(if m.alt { vec![0x1b, c] } else { vec![c] });
                }
                // 修飾の無い（Shift だけの）文字・数字・空白は egui が別に出す `Text` で送る
                _ => return None,
            };
            s.into_bytes()
        }
        _ => return None,
    };
    (!bytes.is_empty()).then_some(bytes)
}

/// G88b: Page の `recorder` node（`props.recorder_ref`）の帯の今（`ActionHost::take`）。録る・試聴・take を入れる道は host（generated.rs）が持ち、
/// renderer は描いて押された手（`TakeAct`）を返すだけ。新しい型の理由: `TerminalPane` は Terminal の写しで録音の段を持てず、audio の型（`Take`・`Receipt`）は
/// renderer の試験 crate（tests/renderer.rs は `#[path]` で ui だけ）に無いので、秒・peaks・文で持つ。
/// Wave: M2
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TakePane {
    pub stage: TakeStage,
    /// 入れる先（`feeds` の面の app が take を入れられる app の表に在る時だけ）。None = 入れる先が無い（`Reveal in Finder` だけ）
    pub add: Option<AddTake>,
    /// 帯の最後の 1 行（`✓ Added to REAPER · Undo in REAPER (⌘Z)`・拒否の理由）。空 = 出さない
    pub line: String,
}

/// G88b: 録音の帯の段。Wave: M2
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TakeStage {
    /// 録る前（`Record humming`）
    #[default]
    Idle,
    /// mic の許可が無い・拒否（`Microphone access is off · Open System Settings`）
    MicOff,
    /// 録っている（`Stop` と経った秒）
    Recording { seconds: f32 },
    /// 録った（波形・`Play`・`Add take to {app}`・`Discard`）。peaks は 0..1 の振れ幅
    Recorded { seconds: f32, peaks: Vec<f32> },
}

/// G88b: take を入れる先の app。`blocked` = 押せない理由の 1 行（`Open REAPER first` 等・面が Ready でない）。Wave: M2
/// ENGINE-GENERIC-1101（owner 211）: 取り下げた道（app ごとの入れる口を製品に持つ）。録画（11/02）が使うので残し、録画の後にこの型と `TakeAct::Add` を消す
/// （帯は録った事を `Generated::part_event` に渡すだけ・何をするかは利用者の一言と Project のきっかけ）。AI-COMPOSE-1101 の `by_ai` は消した。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddTake {
    pub app: String,
    pub blocked: Option<String>,
}

/// G88b: 帯で押された手（`ActionHost::take_act`）。Wave: M2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeAct {
    Record,
    Stop,
    Play,
    Add,
    Discard,
    Reveal,
    MicSettings,
}

/// W5: Page の Terminal 1 つ分の表示（`ActionHost::terminal` が毎 frame 返す・保存しない）。`crate::terminal` を引かない
/// （ui は terminal を取り込まない test crate にも入る）ので、表示に要る物だけを写した型。
///
/// 描画の範囲: W10 から本番の host は `screen`（VT の写し）を渡し、`text` は `screen.text()`。`screen` が None の時（VT の無い host・
/// 前の起動の Terminal）は `text` を plain text のまま出す（W5 の描き方）。偽の描画はしない。
/// AX: 出力の部分木の identifier は `AxScope::Terminal.id(terminal_ref)`（`terminal:<id>`・L1 の範囲の外）で、その text を試験の側が読む。
/// 例（試験の側の手・BG）: 入力欄を AXPress → pid 宛ての keyboard で `sleep 15; echo done` → 送る → 別の Page のタブ → 戻る → `terminal:<id>` の text に `done`。
/// W10: `screen` が Some の時は、その run を色・太字・反転つきの等幅の grid で描き（cursor の cell に印・`col` の位置に置く）、`text` は `screen.text()` と同じ
/// （AX の値。alt screen〈vim・tmux〉の間はちょうど今の画面の rows 行）。描いた大きさが `screen` の rows / cols と違えば `terminal_resize`。
/// grid を click すると focus し、その間の入力は `terminal_input_bytes` → `terminal_keys`（入力欄「Terminal に打つ」と「送る」は W5 のまま残す）。
/// None は W5 の plain text の描き方のまま。
/// Wave: W5
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalPane {
    /// 表示する出力（plain text・末尾の上限まで。上限で捨てた事は `status_line` に書く）。W10: `screen` が Some なら `screen.text()`。
    pub text: String,
    /// W10: VT の画面の写し（`TerminalManager::screen`）。None = W5 の plain text（VT の無い host・前の起動の Terminal）。
    pub screen: Option<TermScreen>,
    /// 子の shell の pid（Running の間だけ Some）。`ps -o ppid=,tty= -p {pid}` で親が Shibubu の事を試験の側が見る。
    pub pid: Option<u32>,
    /// 入力を受けるか（Running の時だけ true。false の時は入力欄を出さず「新しく起こす」を出す）。
    pub accepts_input: bool,
    /// 状態の 1 行（例「zsh · pid 1234」「終わりました（終了 code 3）。新しく起こせます」「前の起動の Terminal は残っていません。新しく起こせます」）。
    /// L1 の成功の語（接続済み・完了・Done・✓ 等）を使わない。
    pub status_line: String,
    /// W11: この席の入力の view（Page の席 / Drawer）。None は触らない（Project の外・前の起動）。
    pub view: Option<InputView>,
}

/// TERM-XTERM-1101: host の描き手（xterm.js）に任せた Terminal の、この frame の renderer への知らせ（`ActionHost::terminal_place` の Some）。
/// 新しくした理由: `TerminalPane` は Terminal の今（画面・状態）で、描き手の頁で起きた事（右 click）を返す口が無い。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TermPlaced {
    /// 頁で右 click された所（枠の左上からの point）とその時の選びの字（無ければ空）。renderer は grid と同じ menu をそこに開く（1 回だけ・読んだら消える）
    pub menu: Option<(egui::Vec2, String)>,
}

/// app 1 つ分の画面の状態（domain の `AppState` = 定義の `state` とは別物。こちらは入力の draft・選択・tab）。
/// 呼ぶ側が app ごとに持ち続ける（app を切り替えても捨てない: DESIGN §1.2「form edits を保つ」）。
/// 入力欄の egui Id は app id + node key なので、egui 側の状態（cursor 等）も app ごとに分かれる。
#[derive(Debug, Default)]
pub struct AppState {
    /// inspector で選んだ node key（list / table の行の選択 `rows` とは別）。開発用の選択（`inspecting` が false の間は持たない）。
    pub selected: Option<String>,
    /// G87: この app の画面で node の選択と inspector を出す（試験が立てる・製品は `INSPECT_ENV` の process だけ = `inspecting`）。既定 false。
    pub inspect: bool,
    /// 入力の draft: node key → 打った相手の record ごとの draft（R2-01）。見せる・送るのは今の bind 先と同じ record の物だけ
    /// （`draft` / `draft_mut`）。editor / command_input / chat の field 名は "text"。bind 先の無い入力は record = None の 1 つ。
    pub drafts: BTreeMap<String, Vec<Draft>>,
    /// tabs の選択: node key → 何番目の子か。
    pub tabs: BTreeMap<String, usize>,
    /// list / table の行の選択: node key → 選んだ行（data の 1 要素。行番号では持たない: 先頭に行が増えても別の record に移らない）。
    /// 描くたびに今の data から同じ record を探し、無い・行を描かない時は外す。`visibility: {"when": "has_selection"}` はこれが 1 つでも在れば真。
    pub rows: BTreeMap<String, RowPick>,
    /// この frame で描いた node の rect（選択・outline・test が「描いた」を見る）。
    pub rects: HashMap<String, Rect>,
    /// この frame で押せなかった action の node → 理由（inspector と test が読む）。
    pub gates: BTreeMap<String, bindings::Gate>,
    /// この frame の region の状態（node key → 状態語と補足。inspector と test が読む）。
    pub regions: BTreeMap<String, (RegionState, String)>,
}

/// 入力欄の draft 1 つ: 打った値と、打った相手の record（R2-01: 別の record の欄に前の record で打った文を出さない・送らない）。
/// serde は Page を離れる時の保存（`project::PageViewState`）に使う。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// field 名 → 打った値。
    pub values: Map<String, Value>,
    /// 打った相手: 打ち始めた時（送った後は送った時）の bind 先の data。None = bind 先が無い・空。
    pub record: Option<Value>,
    /// form で送った。bind 先が送った時の data から変われば（runtime が反映した・別の record に替わった）消す。打ち直せば外れる。
    pub sent: bool,
}

/// list / table の行の選択 1 つ（R1-03 / R2-02）。serde は `project::PageViewState` の保存に使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowPick {
    /// 選んだ行。描くたびに今の data の同じ行に差し替える。
    pub record: Value,
    /// 鍵（`id`、無ければ `*_id` の組）で探すか。押した時に鍵が今の行の 1 行だけを指していた時だけ true。
    /// 鍵が 2 行以上に当たったら外部 key だったと見て、以後は中身全体で探す（同じ親を持つ兄弟へ選択を移さない）。
    pub by_key: bool,
    /// 中身全体で探す時、中身が同じ行（双子）のうち何番目か。押した行を双子の 1 行目に移さない（R3-02）。
    pub nth: usize,
}

impl AppState {
    /// 今の bind 先 `record`（bind 先が無い・空なら None）に付いた draft の値。見せる・送るのはこれだけ。
    pub fn draft(&self, key: &str, record: Option<&Value>) -> Option<&Map<String, Value>> {
        self.drafts.get(key)?.iter().find(|d| same_draft_record(d.record.as_ref(), record)).map(|d| &d.values)
    }

    /// 今の bind 先に付いた draft を書く（無ければ作る）。書いた物はまだ送っていない（`sent` を外す: 保存が通らず打ち直した文を、
    /// 後から来た反映で消さない）。他の record の draft は残す（その record に戻れば戻る）。
    pub fn draft_mut(&mut self, key: &str, record: Option<&Value>) -> &mut Map<String, Value> {
        let list = self.drafts.entry(key.to_owned()).or_default();
        let i = match list.iter().position(|d| same_draft_record(d.record.as_ref(), record)) {
            Some(i) => i,
            None => {
                list.push(Draft { record: record.cloned(), ..Draft::default() });
                list.len() - 1
            }
        };
        let draft = &mut list[i];
        draft.sent = false;
        &mut draft.values
    }

    /// form で送った: 今の bind 先の draft に、送った時の data を記す（data が変われば `forget_sent` が消す）。
    pub fn mark_sent(&mut self, key: &str, record: Option<&Value>) {
        if let Some(draft) = self.drafts.get_mut(key).and_then(|l| l.iter_mut().find(|d| same_draft_record(d.record.as_ref(), record))) {
            draft.record = record.cloned();
            draft.sent = true;
        }
    }

    /// 送った draft のうち、bind 先が送った時の data から変わった物を消す（runtime が反映した・別の record に替わった:
    /// 反映の後に古い文で record を覆わない）。変わっていなければ残す（保存が通らなくても打った文を失わない）。
    pub fn forget_sent(&mut self, key: &str, record: Option<&Value>) {
        if let Some(list) = self.drafts.get_mut(key) {
            list.retain(|d| !d.sent || d.record.as_ref() == record);
            if list.is_empty() {
                self.drafts.remove(key);
            }
        }
    }

    /// W2b（読書記録の「保存の後に欄が空に戻らない」・integ log 36）。
    /// 新しい 1 件を足す form（bind 先の record が無い = composer の `entry` は path `draft` に bind し、そこに data は無い）は、
    /// `mark_sent` の record も今の record も None なので `forget_sent` が永遠に消さない。これが原因。
    ///
    /// 契約: `form` が送った直後（`gated_button` が true）に、`bound.record.is_none()` かつ `host.region(&node.key).is_none()`
    /// （runtime が Verified で通した。`generated::Host::invoke` は同期で、Verified の時だけ region を置かない）なら呼ぶ。
    /// - `drafts[key]` の record None の draft を消す → 次の frame で欄は全部空（hint だけ）
    /// - Partial / Error / Blocked（region が在る）の時は呼ばない: 通らなかった文を消さない（R2-01 と同じ約束）
    /// - bind 先の record が在る form（選んだ 1 件を直す form）は今の `forget_sent` のまま（呼ばない）
    /// Wave: W2b-4
    pub fn clear_entry_after_save(&mut self, key: &str) {
        if let Some(list) = self.drafts.get_mut(key) {
            list.retain(|d| d.record.is_some());
            if list.is_empty() {
                self.drafts.remove(key);
            }
        }
    }
}

/// draft の相手を決める鍵: `id`（null でない）が在ればその値、無ければ中身全体。
/// `*_id` は使わない: 外部 key のことがあり、同じ親を持つ別の record（同じ thread の別の下書き）を同じと読むと、打った文がその record に乗る。
pub fn draft_key(record: &Value) -> &Value {
    record.get("id").filter(|id| !id.is_null()).unwrap_or(record)
}

/// draft の相手と今の bind 先が同じ record か（両方に `id` が在ればその値、無ければ中身全体。None 同士は同じ）。
fn same_draft_record(a: Option<&Value>, b: Option<&Value>) -> bool {
    fn id(v: Option<&Value>) -> Option<&Value> {
        v?.get("id").filter(|id| !id.is_null())
    }
    match (id(a), id(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// 1 frame 分の描画の文脈（components が読む）。
pub struct Cx<'a> {
    pub app: &'a AppDefinitionV3,
    pub state: &'a mut AppState,
    pub host: &'a mut dyn ActionHost,
    pub t: Tokens,
    /// row の中: 押せない button の理由を button の横でなく row の下にまとめる（button の並びを横に溢れさせない）。
    pub defer: Option<Vec<(String, bindings::Gate)>>,
    /// Page に組み込んだ本物の app の面（`source_surface` の `props.surface_ref` で引く）。app 1 つを描く `render` では空。
    pub surfaces: PageSurfaces<'a>,
}

/// Project の `surface_specs` と、Mirror の面を描く `MirrorPages`（feature `mirror`・W16M-1）。
/// 例: `render_page(ui, &def, &page, state, &mut host, PageSurfaces { specs: &p.surface_specs, mirror: Some(&mut pages) })`
#[derive(Default)]
pub struct PageSurfaces<'a> {
    pub specs: &'a [SurfaceSpec],
    #[cfg(feature = "mirror")]
    pub mirror: Option<&'a mut crate::ui::mirror_view::MirrorPages>,
}

/// 主領域（`shell::draw_app_region` の中）に app 1 つを描く: 見出し + app の状態 1 行 + UiNode 木 + 選択中なら inspector。
pub fn render(ui: &mut egui::Ui, app: &AppDefinitionV3, state: &mut AppState, host: &mut dyn ActionHost) {
    render_with(ui, app, state, host, PageSurfaces::default(), true);
}

fn render_with(ui: &mut egui::Ui, app: &AppDefinitionV3, state: &mut AppState, host: &mut dyn ActionHost, surfaces: PageSurfaces<'_>, opening: bool) {
    let t = theme::tokens(ui.ctx());
    // Page の見出し（tab と同じ名前）と Project の一言の全文（全 Page で同じ文）は Page の中に出さない: tab の直下から本物の app が始まる（W15-c2・r20 の写真）
    if opening {
        shell::content_opening(ui, app.title.as_deref().unwrap_or(&app.id), &app.goal.literal_goal, None);
    }
    // 言う事が無い状態（Project の殻の app: Preview・動かす action も取得元も無く、文も無い）は 1 行も描かない。
    // 記号と「Partly shown」だけの行は何が欠けているかも次の一手も言わず、全 Page に出続ける（W15-c1・r20 の AX）
    let said = app.state.status_message.as_deref().unwrap_or("");
    if !(said.is_empty() && app.state.readiness == AppReadiness::Preview) {
        states::inline(ui, bindings::app_state(app.state.readiness), said);
    }
    ui.add_space(if opening { theme::SP_6 } else { theme::SP_2 });
    // G87: 選択と inspector は開発用。利用者の画面（`inspecting` が false）は click 面を作らず、前に保存した選択も持たない
    let inspect = inspecting(state);
    if !inspect || (state.selected.is_some() && escape_for_app(ui.ctx())) {
        state.selected = None;
    }
    // node の選択は 1 枚の click 面で受ける（木より先に置くので button や入力欄が上に来る）。範囲は木を描き始める所から下だけ
    // （G87: 主領域の全部〈clip_rect〉だと、先に置いた Page の帯・Page の上の行の button の mouse の click を取っていた）。
    // focusable にしない（Sense::CLICK だけ）: AX に余計な要素を出さず、Tab の順を乱さない（§9）。
    let pick = inspect.then(|| {
        let clip = ui.clip_rect();
        let below = Rect::from_min_max(Pos2::new(clip.left(), ui.cursor().top().max(clip.top())), clip.max);
        ui.interact(below, egui::Id::new(("kangae-pick", &app.id)), egui::Sense::CLICK)
    });
    state.rects.clear();
    state.gates.clear();
    state.regions.clear();
    // I33b: Mirror の枠を置ける所 = Page の列（左右 = 列の端・上 = 木の始まり・下 = 無限）。`source_surface` が並びの所から見た `room` にする
    state.rects.insert(MIRROR_PAGE.to_string(), Rect::from_min_max(Pos2::new(ui.max_rect().left(), ui.cursor().top()), Pos2::new(ui.max_rect().right(), f32::INFINITY)));
    let mut cx = Cx { app, state, host, t, defer: None, surfaces };
    // TEXT-SELECT-1101: 開発用の node の選択の間だけ label を選べなくする（選べる label は click を取り、木より先に置いた pick に届かない）。利用者の画面は選べるまま
    if inspect {
        ui.style_mut().interaction.selectable_labels = false;
    }
    draw_node(ui, &app.composition.root, &mut cx);
    // I33b: Mirror の枠は木の後に描く（重なった時に前の部品・他の列・他の枠より上）
    #[cfg(feature = "mirror")]
    components::floating_faces(ui, &mut cx);
    if inspect {
        ui.style_mut().interaction.selectable_labels = true;
    }
    // 時刻だけで替わる中身（動いている timer）は、替わる時の frame を自分で頼む（窓は idle の間 frame を頼まない・src/app.rs `next_repaint`）
    if let Some(d) = host.next_change() {
        ui.ctx().request_repaint_after(d);
    }
    if let Some(pos) = pick.filter(|p| p.clicked() || p.secondary_clicked()).and_then(|p| p.interact_pointer_pos()) {
        let hit = deepest_at(&app.composition.root, &state.rects, pos);
        state.selected = if hit == state.selected { None } else { hit };
    }
    draw_inspector(ui, app, state);
}

/// G87: node の選択と inspector（`Selected part`）を出すかを決める env（在れば出す・開発用）。利用者の build の既定は無し = 出さない。
pub const INSPECT_ENV: &str = "SHIBUBU_INSPECT";

/// G87: この画面で node の選択と inspector を出すか（`AppState::inspect` か、`INSPECT_ENV` の在る process）。
fn inspecting(state: &AppState) -> bool {
    static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    state.inspect || *ENV.get_or_init(|| std::env::var_os(INSPECT_ENV).is_some())
}

/// Project の Page 1 枚を描く（YCM-012）: `render` と同じ描き方で、木は `page.root`、見出しは `page.title`、click 面の Id は Page ごと。
/// `app` は Page が `app_ref` で指す app（node の action_ref / binding の行き先）。`render` の署名は変えない（tests/renderer.rs が呼ぶ）。
///
/// 使い方: `renderer::render_page(ui, &app_def, &page, state, &mut host, surfaces)`（`Generated::draw_page` が呼ぶ。surfaces = Project の `surface_specs` と Mirror）。
/// 保証する: 部品の表に無い部品は既存の `components` の「未対応の部品」の 1 行になり、偽の描画をしない。保証しない: app を指さない Page（W3）。
/// 中身は app の写しの木を Page の木に替えて `render` に渡すだけ（inspector・選択・入力欄も同じ道を通る）。
// ponytail: 毎 frame app を 1 回 clone する（draw も定義を clone している）。重くなったら render を root 引数に割る
/// I33b: `AppState::rects` の Page の列の矩形の key（`render_with` が木の前に入れ、`source_surface` が読む）
const MIRROR_PAGE: &str = "mirror-page";

pub fn render_page(ui: &mut egui::Ui, app: &AppDefinitionV3, page: &PageDefinition, state: &mut AppState, host: &mut dyn ActionHost, surfaces: PageSurfaces<'_>) {
    let mut shown = app.clone();
    shown.title = Some(page.title.clone());
    shown.composition.root = page.root.clone();
    render_with(ui, &shown, state, host, surfaces, false);
}

/// W12a: binding を持つ node が data から描いた部分を、AX の identifier `data:<node_key>`（`AxScope::Data`・W2c で名前だけ決めた物）の
/// Group に入れる（`surface_status` と同じ `accesskit_node_builder` + `with_accessibility_parent` の形）。
/// 例: `text` の値・`list` の行を `data_scope(ui, &node.key, |ui| ...)` の中で描く（W12a で text と list が呼ぶ・他の部品は使う Wave で）。
/// 保証する: 試験の道具は identifier `data:<node.key>` の部分木の文字で値を引ける（Dev の Page の `data:dev-git-sha` 等）・その部分木は
/// L1 の成功の語の検査から外れる（利用者の data・`AxScope::excluded_from_l1`）。状態の 1 行（空・失敗・接続が必要）はこの中に入れない（L1 の検査に残す）。
/// 保証しない: data の中身が正しい事（正は試験の側の oracle）。
pub fn data_scope<R>(ui: &mut egui::Ui, node_key: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ax_scope(ui, crate::ui::ax::AxScope::Data, node_key, add)
}

/// `add` が描く部分を、AX の identifier `<scope>:<id>` の Group に入れる（`data_scope` の本体・W19-1 で Connections の凡例〈`legend:connections`〉にも使う）。
/// 保証する: 部分木は identifier で引ける・`scope.excluded_from_l1()` なら L1 の成功の語の検査（tally.py）から外れる。
pub fn ax_scope<R>(ui: &mut egui::Ui, scope: crate::ui::ax::AxScope, id_in_scope: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let id = ui.id().with((scope.prefix(), id_in_scope));
    let ctx = ui.ctx().clone();
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_author_id(scope.id(id_in_scope));
    });
    // 中の Ui は group の id で名を分ける（`ui.scope` の既定の名 "child" だと、隣で同じ番目に来た別の子 Ui〈空の文の region 等〉と
    // 中の widget の id が揃い、frame をまたいで親だけ替わる → accesskit_consumer 0.26 の tree.rs:306 で落ちる・W12a 実機 4 回目）
    let (r, rect) = ctx.with_accessibility_parent(id, || {
        let shown = ui.push_id(id, add);
        (shown.inner, shown.response.rect)
    });
    ctx.accesskit_node_builder(id, |n| {
        n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
    r
}

/// W3: 面の状態の 1 行（D-e: 偽の出力を出さず次の一手を 1 行）。`surface-status:<surface_id>` の値と文はここだけで決める。
/// 例: `let (state, line) = surface_status_line(host.surface_receipt("docs").as_ref()); states::inline(ui, state, &line);`
/// 保証する: verified 以外（None を含む）は成功の語（YC_ACCEPTANCE §4 の表）を含まない・Blocked（▣ Needs approval）を返さない（承認の button はこの帯に無い・UI-1 反証）・「在ります」は find_app が確かめた app だけ（Web は確かめる道が無いので言わない）・
/// 内部の語（bundle id・origin の scheme・oracle の原文）を出さず app の名前で言う・入っていない app を開けとは言わない（次の一手は入れて一言を送り直す）。
/// 保証しない: verified が本当か（正は試験の側の oracle。W3 の製品は verified を作らない）。
/// Wave: W3
pub fn surface_status_line(receipt: Option<&SurfaceReceipt>) -> (RegionState, String) {
    use crate::domain::project::{ReceiptKind as K, ReceiptState as S, CHANGED_OUTSIDE, FILE_UNREADABLE_NEXT, IN_SYNC, NOT_INSTALLED, NOT_ON_DISK, NOT_TRUSTED};
    let Some(r) = receipt else {
        return (RegionState::Ready, "Not checked yet".into());
    };
    // 画面の名前: app は bundle id の最後の段（com.apple.TextEdit → TextEdit）・Web は host（https:// を落とす）
    let name = match r.kind {
        K::Web => r.app_id.split_once("://").map_or(r.app_id.as_str(), |(_, host)| host),
        _ => r.app_id.rsplit('.').next().unwrap_or(&r.app_id),
    };
    match (r.state, r.kind) {
        // Verified（✓）は verified の時だけ。UI-1 反証の直し: ▣（Blocked = Needs approval）は承認の button が同じ行に在る時だけの印なので、
        // 確かめていない面は表（copy.en.md 307・527・529）どおり Ready + 事実の 1 行（`Not checked by Shibubu`）・disk と違う窓は Stale・許可が無いのは ⊘。
        // Ready の語は面の帯に描かない（`surface_status`・SPEC §8「Ready は何も出さない」）ので、確かめていない面に成功の語も「Ready」も出ない
        (S::Verified, _) => (RegionState::Verified, format!("{} · {}", r.operation, r.verified_at)),
        // W16-2: 重ねた窓の文書が disk と違う（保存していない編集・保存の後の Undo）。次の一手は元の app で保存する
        (S::Unverified, _) if r.oracle == NOT_ON_DISK => (RegionState::Stale, format!("{name} has unsaved changes. Save in {name} and Shibubu reads them back")),
        // W16-3 / W16-2-b: disk が変わり窓は古い（外で書き換わった）。保存で上書きさせる一手は出さない
        (S::Unverified, _) if r.oracle == CHANGED_OUTSIDE => (RegionState::Stale, format!("The {name} document was changed outside. Reload it in {name}, or check with Open in {name}")),
        // W16-2: 保存せずに Undo して窓が disk と同じに戻った（IN_SYNC）。読み戻した後なので W3 の「まだ読み戻していません」に落とさない
        (S::Unverified, _) if r.oracle == IN_SYNC => (RegionState::Ready, format!("The document in {name}'s window matches what's on disk. Edit and save in {name} and Shibubu reads it back")),
        (S::Unverified, K::Web) => (RegionState::Ready, NOT_CHECKED.into()),
        // W14（062 の不許可）: 許可の有る app と同じ 1 行にしない（この Page から操作できない理由と次の一手）
        (S::Unverified, _) if r.oracle == NOT_TRUSTED => (RegionState::Disconnected, format!("{name} is on this Mac, but Shibubu doesn't have Accessibility access. In System Settings › Privacy & Security › Accessibility, allow Shibubu (you can open and use {name} directly for now)")),
        (S::Unverified, _) => (RegionState::Ready, NOT_CHECKED.into()),
        // W16-2: 面の file を disk で読めない。1 行（file の名前・理由・次の一手）は NativeError::FileUnreadable が作った物
        (S::Unavailable, _) if r.oracle.ends_with(FILE_UNREADABLE_NEXT) => (RegionState::Disconnected, r.oracle.clone()),
        (S::Unavailable, _) if r.oracle == NOT_INSTALLED =>(RegionState::Disconnected, format!("{name} isn't installed on this Mac. Install it, then send your request again")),
        (S::Unavailable, K::Web) => (RegionState::Disconnected, format!("Can't show {name} here · Open in browser")),
        // 表の EN の後ろの `Check again` はこの帯に押す物が無いので付けない（反証の直し・押せない button の名を言わない）
        (S::Unavailable, _) => (RegionState::Disconnected, format!("Couldn't find {name}")),
        (S::Ended, _) => (RegionState::Disconnected, format!("{name} isn't running · Open {name}")),
    }
}

/// 確かめていない面の事実（copy.en.md の tooltip の語）。
const NOT_CHECKED: &str = "Not checked by Shibubu";

/// G93: 確かめていない面の印（`○`）で出す状態か（Mirror の面は枠の title bar の右端に置く）。
/// 例: `renderer::surface_unchecked(host.surface_receipt("docs").as_ref())`。
pub fn surface_unchecked(receipt: Option<&SurfaceReceipt>) -> bool {
    let (state, line) = surface_status_line(receipt);
    state == RegionState::Ready && (receipt.is_none() || line == NOT_CHECKED)
}

/// `surface-status:<surface_id>` の 1 行を描く（AX の identifier = `AxScope::SurfaceStatus`・値 = receipt の state か `unchecked`）。
/// 状態の文字はこの node の子に置く（L1 の ②: 成功の語が出てよいのは surface-status の中だけ）。
/// G87 × NOTIFY-1101: 確かめていない面（読み戻しが無い・`Not checked by Shibubu`）は画面に何も描かない（前は小さな印 `○` と hover・AX の label は 1 行のまま）。
pub fn surface_status(ui: &mut egui::Ui, surface_id: &str, receipt: Option<&SurfaceReceipt>) {
    let (state, line) = surface_status_line(receipt);
    let id = ui.id().with(("surface-status", surface_id));
    let value = receipt.map_or("unchecked", |r| r.state.as_str());
    let ctx = ui.ctx().clone();
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_author_id(crate::ui::ax::AxScope::SurfaceStatus.id(surface_id));
        n.set_value(value);
        // Ready は語を出さない（画面と同じ・事実の 1 行だけ）。他は状態の語 + 1 行
        n.set_label(if state == RegionState::Ready { line.clone() } else { format!("{} {}", state.wording(), line) });
    });
    // 中の Ui の名は group の id から（`data_scope` と同じ理由）
    let rect = ctx.with_accessibility_parent(id, || {
        ui.push_id(id, |ui| {
            let t = theme::tokens(ui.ctx());
            // NOTIFY-1101（owner 185「○ いらない」）: 確かめていない面は何も描かない（いつも同じ事しか言わない印）。AX の label は 1 行のまま
            if state == RegionState::Ready {
                if !surface_unchecked(receipt) {
                    ui.add(egui::Label::new(theme::Type::Helper.text(&line).color(t.text_tertiary)).truncate());
                }
            } else {
                states::inline(ui, state, &line);
            }
        })
        .response
        .rect
    });
    ctx.accesskit_node_builder(id, |n| {
        n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
}

/// W14b: 使える検査の面ごとの 1 行（AX の identifier = `AxScope::Usable`・値 = `value`〈ready / checking / not_ready〉・label = `line`）。
/// `state` は通らない面の状態（None = 使える面: 文言の語〈準備完了〉を出さず glyph と `line` だけ ＝ 成功の語を出さない）。
/// 判定と文は `composer::verifier::Usable`（ui は composer を引かない crate にも入るので、値で受ける）。
/// 例: `renderer::usable_line(ui, &u.seat.id, u.value(), state_of(&u), &u.line(), &[])`（generated.rs）
/// W13b: `choices` = その 1 行への答え（`verifier::SeatKind::MissingApp` の席の 1 問・他の席は空）。`(button の文字, Some(https の URL))` は link
/// （押すと egui が既定の browser で開くだけ・`ui.hyperlink_to` と同じ・戻り値に出さない）、`(文字, None)` は button。並びのまま 1 行の下に
/// 横に並べ（入らなければ折り返す）、AX では `usable:<seat_id>` の group の子にする。戻り値 = この frame に押された button の `choices` の index。
/// 署名を変えた理由（W13b interface）: 問いの答えを使える検査の 1 行の下に置き、同じ group で AX から押せるようにするため。
/// 保証しない: link の頁が開けた事（browser の側）。
/// 契約（UI-6a = owner の問題 3「`● Terminal は使えます` が同じ Page に 2 回」）: `state` が None（通った席）は画面に何も描かない
/// （Ready の印も語も描かない・門 `a6_ready_silent`）。AX の Group（`usable:<seat_id>`・value・label = `line`）は残す（BG の oracle と試験が読む）。
/// `Some(Blocked)`（Needs approval・ICONS-1101 追記 238 から印は無い）は `choices` に押せる button が在る時だけ（W13b の 1 問）= sendback 31 / 56。button の無い Install / Permission は `Disconnected`・
/// Approve（外に効く 1 手を走らせていない）は `Ready`（事実の 1 行だけ = `states::inline_wrap` の Ready）を generated の `usable_state` が渡す。
/// 通らない席の同じ文は `states::inline_wrap` の 1 pass に 1 回で 1 行になる。
/// Wave: W14b, W13b, UI-6a
pub fn usable_line(ui: &mut egui::Ui, seat_id: &str, value: &str, state: Option<RegionState>, line: &str, choices: &[(String, Option<String>)]) -> Option<usize> {
    // 中の Label の id を席ごとに分ける（同じ位置の auto id のままだと、Page を移った時に同じ AX の node が別の席の group の下へ移り、
    // macOS の accesskit_consumer が tree.rs:306 の unwrap で落ちる・W14b の BG で見つけた）
    ui.push_id(("usable", seat_id), |ui| usable_line_in(ui, seat_id, value, state, line, choices)).inner
}

fn usable_line_in(ui: &mut egui::Ui, seat_id: &str, value: &str, state: Option<RegionState>, line: &str, choices: &[(String, Option<String>)]) -> Option<usize> {
    let id = ui.id().with(("usable", seat_id));
    let ctx = ui.ctx().clone();
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_author_id(crate::ui::ax::AxScope::Usable.id(seat_id));
        n.set_value(value);
        n.set_label(line);
    });
    // 理由と次の一手は文の後ろに在るので切らずに折り返す（D-e）
    let drawn = ctx.with_accessibility_parent(id, || {
        ui.scope(|ui| {
            // 通った席（None）は画面に何も描かない（Ready は何も出さない・同じ `Terminal …` が席ごとに並ばない）。AX の Group は上で作った
            if let Some(s) = state {
                states::inline_wrap(ui, s, line);
            }
            // W13b: 答え（link は egui が既定の browser で開くだけ・button は押された index を返す）
            let mut pressed = None;
            if !choices.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for (i, (text, url)) in choices.iter().enumerate() {
                        match url {
                            Some(url) => {
                                ui.hyperlink_to(text, url);
                            }
                            None => {
                                if theme::button_compact(ui, text, theme::Variant::Ghost).clicked() {
                                    pressed = Some(i);
                                }
                            }
                        }
                    }
                });
            }
            pressed
        })
    });
    let rect = drawn.response.rect;
    ctx.accesskit_node_builder(id, |n| {
        n.set_bounds(egui::accesskit::Rect { x0: rect.min.x.into(), y0: rect.min.y.into(), x1: rect.max.x.into(), y1: rect.max.y.into() });
    });
    drawn.inner
}

/// `visibility.when`: corpus の語彙は `has_selection` だけ（CD-11 (4)）。知らない条件では隠さない
/// （説明の無い空白を作らない。語彙は validator が縛る）。
pub fn visible(node: &UiNode, state: &AppState) -> bool {
    match node.visibility.as_ref().and_then(|v| v.get("when")).and_then(Value::as_str) {
        Some("has_selection") => !state.rows.is_empty(),
        _ => true,
    }
}

/// node 1 つ: 描いて rect を残し、選択中なら 1px の outline（4px 外側）。Id は node key で積む（入力欄の状態が木の形に依らない）。
pub fn draw_node(ui: &mut egui::Ui, node: &UiNode, cx: &mut Cx<'_>) {
    if !visible(node, cx.state) || components::zoom_hides(ui.ctx(), node, cx) {
        return;
    }
    // 中身の状態の 1 行は欄ごとの事実（同じ文でも別の node の行を「1 pass に 1 回」で消さない）
    let rect = states::each_its_own(ui, |ui| ui.push_id(&node.key, |ui| components::draw(ui, node, cx))).response.rect;
    cx.state.rects.insert(node.key.clone(), rect);
    // LAYOUT: 次の frame の前の部品が、この node の分を窓に残す（`components::reserved_below`）
    components::note_height(ui.ctx(), &node.key, "node", rect.height());
    if cx.state.selected.as_deref() == Some(node.key.as_str()) {
        let stroke = Stroke::new(theme::HAIRLINE, cx.t.text_primary);
        ui.painter().rect_stroke(rect.expand(theme::SP_1), CornerRadius::same(theme::RADIUS_SM), stroke, StrokeKind::Outside);
    }
}

/// pos を含む node のうち一番深い物（入れ子なら内側）。描いていない node（隠れている・選ばれていない tab）は rect が無いので選ばれない。
fn deepest_at(root: &UiNode, rects: &HashMap<String, Rect>, pos: Pos2) -> Option<String> {
    fn walk(node: &UiNode, rects: &HashMap<String, Rect>, pos: Pos2, depth: usize, best: &mut Option<(usize, String)>) {
        if rects.get(&node.key).is_some_and(|r| r.contains(pos)) && best.as_ref().is_none_or(|(d, _)| depth >= *d) {
            *best = Some((depth, node.key.clone()));
        }
        for child in &node.children {
            walk(child, rects, pos, depth + 1, best);
        }
    }
    let mut best = None;
    walk(root, rects, pos, 0, &mut best);
    best.map(|(_, key)| key)
}

// ---------------------------------------------------------------------------
// inspector（UI-009 の最小・旧 `renderer/inspector.rs` を UI-6a の後の Refactor 段で寄せた）: 選んだ node の
// name / role / binding / capability / 状態 / 権限 / source（DESIGN §6.3 の一覧の部分）。中身は `inspector_rows`（純関数・test が読む）。
// 置き場は主領域の右下に重ねる 296 幅の面（modal にしない・木の配置を動かさない）。`Close` / Esc / 余白の click で外れる。
// readiness は schema の語（snake_case）で出す: 定義の JSON と突き合わせられるように。
// ---------------------------------------------------------------------------

/// enum を schema の語（serde の snake_case）にする。
fn schema_word<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => "?".to_owned(),
    }
}

/// 選んだ node（`key`）の inspector の行（項目名, 値）。node が木に無ければ空。
pub fn inspector_rows(app: &AppDefinitionV3, state: &AppState, key: &str) -> Vec<(&'static str, String)> {
    let Some(node) = app.composition.root.find(key) else { return Vec::new() };
    let dash = || "—".to_owned();
    let action = node.action_ref.as_deref().and_then(|id| app.action(id));
    let name = match &node.label {
        Some(label) => format!("{label} ({} · {})", node.component, node.key),
        None => format!("{} ({})", node.key, node.component),
    };
    let binding = node.binding.as_ref().map(|b| format!("{} · {} · {:?}", b.source_ref, b.path, b.readiness(&app.sources, None)));
    let capability = match (node.action_ref.as_deref(), action) {
        (Some(id), None) => Some(format!("{id} · not defined")),
        (_, Some(a)) => Some(match a.capability_ref.as_deref() {
            None => format!("{} · {} · no capability (on this Mac)", a.id, schema_word(&a.readiness)),
            Some(c) => match app.capability(c) {
                Some(cap) => format!("{} · {} → {} · {}", a.id, schema_word(&a.readiness), cap.id, schema_word(&cap.readiness)),
                None => format!("{} → {c} · not defined", a.id),
            },
        }),
        (None, None) => None,
    };
    let mut status = Vec::new();
    if let Some(gate) = state.gates.get(key) {
        status.push(format!("Can't press: {}", gate.text()));
    }
    if let Some((s, detail)) = state.regions.get(key) {
        status.push(if detail.is_empty() { s.wording().to_owned() } else { format!("{} · {detail}", s.wording()) });
    }
    if let Some(s) = &node.states {
        status.push(format!("Preset text: {}", s.keys().cloned().collect::<Vec<_>>().join(" / ")));
    }
    if let Some(v) = node.visibility.as_ref().and_then(|v| v.get("when")).and_then(Value::as_str) {
        status.push(format!("Shown when: {v}"));
    }
    let permission = action.map(|a| {
        let scopes = if a.required_scopes.is_empty() { "No scope".to_owned() } else { a.required_scopes.join(", ") };
        let approval = if a.requires_approval { " · Approval before running" } else { "" };
        format!("{} · {scopes}{approval}", schema_word(&a.effect))
    });
    let source = node.binding.as_ref().and_then(|b| app.source(&b.source_ref)).map(|s| {
        let provider = s.provider.as_deref().map(|p| format!(" · {p}")).unwrap_or_default();
        let seen = s.last_observed_at.as_deref().map(|at| format!(" · Last fetched {at}")).unwrap_or_default();
        format!("{} · {} · {}{provider}{seen}", s.id, s.kind, schema_word(&s.readiness))
    });
    vec![
        ("name", name),
        ("role", node.role.clone().unwrap_or_else(dash)),
        ("binding", binding.unwrap_or_else(dash)),
        ("capability", capability.unwrap_or_else(dash)),
        ("State", if status.is_empty() { dash() } else { status.join(" / ") }),
        ("Permissions", permission.unwrap_or_else(dash)),
        ("source", source.unwrap_or_else(dash)),
    ]
}

/// 選択が在る時だけ、主領域の見えている範囲の右下に 296 幅で重ねる。選んだ node が木から消えていたら選択を外す。
fn draw_inspector(ui: &egui::Ui, app: &AppDefinitionV3, state: &mut AppState) {
    let Some(key) = state.selected.clone() else { return };
    let rows = inspector_rows(app, state, &key);
    if rows.is_empty() {
        state.selected = None;
        return;
    }
    let t = theme::tokens(ui.ctx());
    let corner = ui.clip_rect().right_bottom() - egui::Vec2::splat(theme::SP_4);
    egui::Area::new(egui::Id::new(("kangae-inspector", &app.id)))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(corner)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(t.surface_1)
                .stroke(theme::hairline(&t))
                .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                .inner_margin(egui::Margin::same(theme::SP_4 as i8))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(theme::DRAWER_W - 2.0 * theme::SP_4);
                    ui.horizontal(|ui| {
                        theme::label(ui, theme::Type::HeadingSm, "Selected part", t.text_primary);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::button_compact(ui, "Close", theme::Variant::Ghost).clicked() {
                                state.selected = None;
                            }
                        });
                    });
                    ui.add_space(theme::SP_2);
                    for (k, v) in rows {
                        theme::label(ui, theme::Type::Helper, k, t.text_tertiary);
                        theme::label(ui, theme::Type::Body, v, t.text_primary);
                        ui.add_space(theme::SP_1);
                    }
                });
        });
}

// 幅の解決（`components` の row / split / table だけが使う）。前は renderer/layout.rs（path `renderer::layout` は同じ）
pub mod layout {
    //! UI-006: 幅の解決。row / split の子に min / preferred / max と優先度を与え、収まる幅を配る。
    //! min の和が収まらなければ None（縦に畳む: 優先度の高い順）。表は列の min の和で横 overflow を決め、描く側が scroll と 1 行の説明を出す。
    //! 文字と button だけの row は `horizontal_wrapped`（縮めずに折り返す: label と button を読める大きさのまま残す）。
    //! 入力は木と `role` と `layout.priority`（CD-11 (2)）。`layout` の他の key は `ratio`（corpus が使う）だけ読む。
    use crate::domain::ui::UiNode;
    use serde_json::Value;

    /// 1 列の最小幅（見出しが読める幅）。これ未満に縮めず横 scroll にする。
    pub const COLUMN_MIN_W: f32 = 120.0;
    /// list / form などの塊が使い物になる最小幅。
    pub const BLOCK_MIN_W: f32 = 200.0;
    /// split の主役の最小の取り分（DESIGN §7「>= 60%」。`layout.ratio` が書かれていればそれを尊重）。
    pub const PRIMARY_SHARE: f32 = 0.6;
    /// LAYOUT（owner 10-31 の 19 点の 3・5「youtube の画面も縮こまっている」「reaper が小さすぎる」「service ごとの大きさ」・operator 10-31 の門 3）:
    /// service の種類ごとの (列の min の幅, 面の床の高さ)。種類 = 面の backend × 置き場（`components::service`）・app の名前で分けない。
    /// row / split の列はこの幅より細くしない（`components::blocks`・入らない幅は縦に畳む）。床 = 低い窓でここまで縮める高さ（面の高さと前の面が残す分で同じ値）。
    /// 窓の残りを取る（伸びる）のは APP_MAIN と WEB・APP_SIDE は床のまま（小さい side）・録音の帯は自分の中身の高さ（床は使わない）。
    /// 1440×900（rail 無し・Page 1152）の既定: REAPER ≈ 741（Page の 64%）・side ≈ 399 = Music 160 の上に YouTube ≥ 400。
    /// desktop app が主の列（REAPER）: 作業場が使える幅と高さ。
    pub const APP_MAIN: (f32, f32) = (480.0, 360.0);
    /// desktop app が side の区画（Music）: 小さい side。本物の最小の窓が枠より大きい app（Music は 980×580・operator 10-31 実測）は枠の中で縮めて映す（L5 の fit）。
    pub const APP_SIDE: (f32, f32) = (320.0, 160.0);
    /// Web の面（YouTube）: 動画の題と 1 枚が読める幅（owner の 374 は「縮こまっている」）と高さ。
    pub const WEB: (f32, f32) = (390.0, 400.0);
    /// 録音の帯: Record humming・波形・Add take が折り返さない幅（高さは自分の中身）。
    pub const RECORDER: (f32, f32) = (480.0, 0.0);

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Want {
        pub min: f32,
        pub preferred: f32,
        pub max: f32,
        /// 0 = primary（畳む時に先頭・縮める時に最後）/ 1 = secondary / 2 = supporting
        pub priority: u8,
    }

    /// 文字だけの部品（row では折り返して並べ、幅を解かない）。
    pub fn is_inline(node: &UiNode) -> bool {
        matches!(node.component.as_str(), "button" | "text" | "input" | "command_input" | "run_progress" | "recorder" | "extension")
    }

    /// `layout.priority`（primary / secondary / supporting）→ 無ければ role（primary が先、context 等の補助は後）。
    pub fn priority(node: &UiNode) -> u8 {
        match node.layout.as_ref().and_then(|l| l.get("priority")).and_then(Value::as_str) {
            Some("primary") => 0,
            Some("secondary") => 1,
            Some("supporting") => 2,
            _ => match node.role.as_deref() {
                Some("primary") => 0,
                Some("context" | "detail" | "activity" | "navigation") => 2,
                _ => 1,
            },
        }
    }

    /// row / split の子 1 つの要求。`share` は split の `layout.ratio` の取り分（無ければ 1/n）。
    pub fn want(node: &UiNode, free: f32, share: f32, has_ratio: bool) -> Want {
        let priority = priority(node);
        let columns = node.props.get("columns").and_then(Value::as_array).map(Vec::len).unwrap_or(0) as f32;
        let mut min = match node.component.as_str() {
            "table" | "chart" => (columns * COLUMN_MIN_W).max(BLOCK_MIN_W),
            _ => BLOCK_MIN_W,
        };
        if priority == 0 && !has_ratio {
            min = min.max(PRIMARY_SHARE * free);
        }
        Want { min, preferred: (share * free).max(min), max: f32::INFINITY, priority }
    }

    /// `layout.ratio` を読む（長さが children と違う・0 以下が在れば無視）。
    pub fn ratio(node: &UiNode) -> Option<Vec<f32>> {
        let r: Vec<f32> = node.layout.as_ref()?.get("ratio")?.as_array()?.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
        (r.len() == node.children.len() && r.iter().all(|x| *x > 0.0)).then_some(r)
    }

    /// 子の幅。None = 横に並べられない（min の和が available を超える）。
    /// preferred の和が free を超えれば preferred − min の余地に比例して縮め、足りなければ max までの余地に比例して広げる（free を使い切る）。
    pub fn solve(available: f32, wants: &[Want], gutter: f32) -> Option<Vec<f32>> {
        if wants.is_empty() {
            return Some(Vec::new());
        }
        let free = available - gutter * (wants.len() - 1) as f32;
        let min_sum: f32 = wants.iter().map(|w| w.min).sum();
        if min_sum > free {
            return None;
        }
        let mut widths: Vec<f32> = wants.iter().map(|w| w.preferred.clamp(w.min, w.max.max(w.min))).collect();
        let sum: f32 = widths.iter().sum();
        if sum > free {
            let room: f32 = wants.iter().zip(&widths).map(|(w, x)| x - w.min).sum();
            if room > 0.0 {
                let need = sum - free;
                for (w, x) in wants.iter().zip(widths.iter_mut()) {
                    *x -= (*x - w.min) / room * need;
                }
            }
        } else if sum < free {
            let rooms: Vec<f32> = wants.iter().zip(&widths).map(|(w, x)| (w.max - x).clamp(0.0, free)).collect();
            let room: f32 = rooms.iter().sum();
            if room > 0.0 {
                let extra = (free - sum).min(room);
                for (r, x) in rooms.iter().zip(widths.iter_mut()) {
                    *x += r / room * extra;
                }
            }
        }
        Some(widths)
    }

    /// 畳む時の順（priority 昇順・同じなら元の順）。
    pub fn collapse_order(wants: &[Want]) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..wants.len()).collect();
        idx.sort_by_key(|&i| wants[i].priority);
        idx
    }

    /// 表の列幅（見出しの文字幅 + 左右の余白、下限 COLUMN_MIN_W）の和が available を超えるか。超えれば横 scroll を明示する。
    pub fn table_overflows(available: f32, columns: &[f32]) -> bool {
        columns.iter().sum::<f32>() > available
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn w(min: f32, preferred: f32, max: f32, priority: u8) -> Want {
            Want { min, preferred, max, priority }
        }
        #[test]
        fn solve_fills_free_width_respects_max_and_collapses_when_mins_do_not_fit() {
            let inf = f32::INFINITY;
            let widths = solve(1000.0, &[w(200.0, 600.0, inf, 0), w(200.0, 400.0, inf, 1)], 12.0).unwrap();
            assert!((widths.iter().sum::<f32>() - 988.0).abs() < 0.01, "{widths:?}");
            assert!(widths[0] > widths[1]);
            let capped = solve(1000.0, &[w(200.0, 300.0, 320.0, 0), w(200.0, 300.0, inf, 1)], 12.0).unwrap();
            assert!(capped[0] <= 320.01 && (capped.iter().sum::<f32>() - 988.0).abs() < 0.01, "max を超えて配らない: {capped:?}");
            let squeezed = solve(500.0, &[w(200.0, 400.0, inf, 0), w(200.0, 400.0, inf, 1)], 12.0).unwrap();
            assert!(squeezed.iter().all(|x| *x >= 200.0) && (squeezed.iter().sum::<f32>() - 488.0).abs() < 0.01, "{squeezed:?}");
            assert_eq!(solve(300.0, &[w(200.0, 600.0, inf, 0), w(200.0, 400.0, inf, 1)], 12.0), None, "min の和 400 > 288 → 畳む");
            assert_eq!(collapse_order(&[w(1.0, 1.0, inf, 2), w(1.0, 1.0, inf, 0), w(1.0, 1.0, inf, 1)]), vec![1, 2, 0]);
        }
        #[test]
        fn table_overflow_is_a_function_of_column_mins() {
            assert!(table_overflows(400.0, &[COLUMN_MIN_W; 4]));
            assert!(!table_overflows(480.0, &[COLUMN_MIN_W; 4]));
        }
        #[test]
        fn layout_priority_wins_over_role() {
            let mut n = UiNode { key: "k".into(), component: "list".into(), role: Some("context".into()), ..UiNode::default() };
            assert_eq!(priority(&n), 2);
            n.layout = Some(serde_json::json!({"priority": "primary"}).as_object().unwrap().clone());
            assert_eq!(priority(&n), 0);
        }
    }
}
