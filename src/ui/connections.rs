//! Connections（§10）: AI providers / Apps & services（GitHub・Web data）/ Tools & MCP（画面の操作・信頼済 CLI・MCP）/ System access（OS の許可）。
//! 中では段（`DisplayTier`: Available → Connected → Ready → Verified）を持ち、画面は 2 つだけ: 緑の `Connected`（使える）と赤の `Not connected`（まだ使えない・
//! 何が要るかは行の短い説明に 1 つ）。CONNECTIONS-1101（owner 196「connectedやabailableの違いもわかりずらいから１つに決めて」）。
//! 実状態は app.rs が `ConnectionsModel` に詰める（画面の操作は `MacGui` の check を thread で 1 回 + `Check again`・YC W17-2）。

use crate::domain::icon::Icon;
use super::motion;
use super::shell::{self, Intent};
use super::theme::{self, Type, Variant};
use crate::domain::capability::DisplayTier;
use eframe::egui::{self, Vec2};

/// GitHub の行の名前（`Check again` が `Intent::RecheckGithub` になる行）。
pub const GITHUB_ROW: &str = "GitHub";

/// CONNECTIONS-1101: 画面の 2 つの状態の分け目。`Connected`・`Ready`（読み戻して確かめた）・`Verified`（操作の証拠）= 使える = `Connected`、
/// `Available`（見つかったがまだ繋いでいない・sign in / 許可 / 設定が要る）・`Unavailable` = `Not connected`。例: `connected(DisplayTier::Ready)` → true
pub const fn connected(t: DisplayTier) -> bool {
    matches!(t, DisplayTier::Connected | DisplayTier::Ready | DisplayTier::Verified)
}

/// 題の大きさ（owner 196 の 7・8: `Connections` は大きく太く・群の題は `Type::Group`）
const TITLE_SIZE: f32 = 24.0;

/// Connections の 1 行。UI-4a で field `settings`（`Open settings` の button の行き先・sendback 68）を足した。例: `Connection { settings: None, ..row }`。
/// 保証する: `settings` が `None` の行は `Open settings` の button を描かない。保証しない: 押した後に許可が付いた事（`Check again` で測り直す）。
/// 契約（RQ4 から）: field `key`・`acts`・`checked_at` を足した（行の宛先・行で押せる手・確かめた時刻）。例:
/// `Connection { key: "web".into(), acts: vec![ConnAct::CheckAgain, ConnAct::Disconnect], checked_at: Some(now), ..row }`。
/// Wave: UI-4a, RQ4
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub name: std::borrow::Cow<'static, str>,
    pub state: DisplayTier,
    /// 1 行の実状態（env の有無・daemon の返事など。値や secret は書かない）
    pub detail: String,
    /// 再確認できる（画面の操作）。
    pub recheck: bool,
    pub checking: bool,
    /// 行の右に `Open settings`（copy.en.md:263）の button を出す時の System Settings の pane の URL（画面の操作の行だけ
    /// `Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")`・他は `None`）。例: `Connection { settings: None, ..row }`。
    /// 契約（UI-4a）: button は `detail` が `· Open settings` で終わる時だけ出し、押すと `row` の中で `/usr/bin/open <url>` を 1 回（Intent を足さない =
    /// check_wired・前例 `src/audio/surface.rs:435`）。`app.rs` の試験 `ui4a_gui_row_next_step_is_a_button` は literal を写さず「detail の最後の ` · ` の後の語 = 行に在る button の名」を見る（sendback 68）。
    /// 保証しない: 許可が実際に付いた事（押した後は今の `Check again` で測り直す）。
    pub settings: Option<&'static str>,
    /// RQ4: 行の宛先（`Intent::Connection { row, .. }` の `row`）。`"github"`・`"gui"`・`"web"`・`"cli"`（Trusted commands の見出しの行 = `Add`）・
    /// `"cli:<RepoFeature の id>"`・`"mcp"`（MCP servers の見出しの行 = `Add`）・`"mcp-found"`（他の app の設定を読む手の行・`Check again` =「Look in Claude's settings」）・`"mcp:<server>"`（`~/.shibubu/mcp.json` の server）・`"mcp-found:<server>"`（他の app の
    /// 設定に在る server・`Connect` = Shibubu の file へ写す）・`"ai"`（Settings › AI & Models の行と、Connections の AI providers の「使っている AI」の行・`Check again` = 探し直す）。
    /// ACCOUNTS-1101: 見つかった AI と足した account は行でなく `settings::accounts` の部品（前の `"ai:found:<id>"` の行は無くした）。群は `group_of` が key から決める。保存しない（毎 frame の写し）。
    pub key: std::borrow::Cow<'static, str>,
    /// RQ4（RQ-015〜RQ-017）: 行の右に左から並べる手。`draw` は各手を `ConnAct::label` の button にし（`AllowTool` は行の下の checkbox）、押すと
    /// `Intent::Connection { row: key, act }` を返す。空なら今の `recheck`・`settings` だけ（RQ4 の本体で `recheck` の行は `CheckAgain` を持ち、`recheck` を消す）。
    /// 契約: 押して何も起きない手を入れない（作り手 = app.rs の `connections_model` と `mcp::connection_rows` が、その行で効く手だけを入れる）。
    pub acts: Vec<ConnAct>,
    /// RQ4（RQ-015「確認の時刻で読み戻す」）: 最後に確かめた時刻（unix 秒）。Some なら `draw` が detail の末尾に `checked HH:MM`（local 時刻）を足す。
    /// None = まだ確かめていない（`Checked when you open Connections` などの detail のまま）。保存しない（起動ごとに確かめ直す）。
    pub checked_at: Option<i64>,
}

/// RQ4（RQ-015〜RQ-017）: Connections の 1 行で押せる手（MH-S3「Add/Connect/Reconnect/Test/Disconnect」）。`Intent::Connection { row, act }` の `act`。
/// 例: `Connection { key: "mcp:penpot".into(), acts: vec![ConnAct::CheckAgain, ConnAct::Disconnect, ConnAct::Remove], ..row }` → `Check again`・`Disconnect`・`Remove…` の 3 つの button。
/// 行ごとの意味（処理は app.rs の `Intent::Connection` の腕）:
/// - `Connect`: その行を使い始める（`web` = 1 つの欄を URL の欄にする〈`Shell::connecting`〉・`mcp-found:<s>` = Shibubu の `mcp.json` へ写して確かめる・
///   `cli:<id>` / `mcp:<s>` = 止めていた物を戻す）
/// - `CheckAgain`: 今の設定のまま確かめ直す（`gui` = bridge の `hello`・`github` = gh の login か Keychain の token で `GET /user`・`web` = 1 回 GET・
///   `mcp:<s>` = `tools/list`〈`mcp::forget_tools` で前の答えを捨てる〉・`mcp-found` = 他の app の設定を 1 回読む〈`mcp::found_config`・開いただけでは読まない〉）。結果は `checked_at` と detail
/// - `Test`: `web` の欄に打った URL を保存の前に 1 回 GET する（`Shell::connecting` の間の 2 つ目の手）
/// - `Disconnect`: Shibubu が使うのをやめる。設定は残す（`github` / `web` = `UiSettings::disconnected` に足す・`mcp:<s>` = `McpServerConfig::disabled`・
///   `cli:<id>` = `RepoFeature::enabled = false`）。Keychain の秘密は消さない（`Remove` が消す）
/// - `Reconnect`: 切った接続を同じ設定で戻して確かめる（`disconnected` から外して `CheckAgain` と同じ）
/// - `Add`: 2 つ目を足す（`mcp` = 1 つの欄を「起こす command か README の JSON」の欄に・`cli` = `osascript` の choose file で program を選ぶ〈絶対 path を打たせない〉）
/// - `Remove`: 設定ごと消す（行内の確認 `Shell::connection_confirm` の `Remove` の後だけ・Keychain の秘密も）
/// - `PasteSecret(name)`: clipboard を 1 回読み Keychain の `secret_store::connection_key(row, name)` へ（`github` = `token`・`web` = `token`・`mcp:<s>` = env の名前）。
///   値は Intent にも画面にも載せない（RQ2 の `PasteApiKey` と同じ道）
/// - `AllowTool { tool, allowed }`: `mcp:<s>` の tool を Engine に使わせる / 使わせない（`McpServerConfig::allowed_tools`）
///   OPEN-INPLACE-1101 の 2: `gui`（Screen control）の行では、承認の card の Always allow で許した手（`surface::AlwaysAllowed::label` =「GarageBand · Click」）。
///   入った checkbox で並び、外す（`allowed: false`）= `surface::revoke_always`（以後その手はまた承認を聞く）。新しい手の型を足さず同じ checkbox の道を使う
/// - `Submit`: 1 つの欄の Enter（`Shell::connecting` の間・欄の文は Intent に載せず app.rs が `shell.command` から読む = URL の query の鍵を Debug に出さない）
/// - `UseItem { name, item }`: Paste の欄の下の項目を押した = 行の秘密 `name` に項目の参照を置く（値は置かない・使う瞬間に `vault::resolve`）。label = `item.label()`
/// PASSWORD-1101: password manager の行 `pm:<scheme>`（`vault::Manager::row`・chat の card からも同じ手）= `Connect`（sign in = 1Password は Touch ID・
/// `UiSettings::vaults` に足す）・`CheckAgain`（CLI を探し直して sign in）・`Disconnect`（vaults から外す・card の `Not now` も = 繋いでいなければ card を閉じるだけ）・
/// `Add`（chat で止めた `Shell::held` をその manager に保存）・`Remove`（`Shell::held` を捨てる・manager の項目は消さない）。
/// 保証しない: 行の外の物（OS の許可・gh の login そのもの）を変える事（`Disconnect` は Shibubu が使わないだけ・gh の login は消さない）。
/// 新しくした理由: 接続の手の動詞を持つ既存の型が無い（`portal_frame::PortalAction` は面の帯の次の一手 1 つ・`ui::runs::RunAction` は Run の行の手・`Intent` に行ごとの
/// variant を並べると 10 個 × 行の key になる）。FX5（yc/fixcurrent）の `Intent::RecheckGithub` と今の `Intent::RecheckGui` は merge の時に
/// `Intent::Connection { row: "github" | "gui", act: ConnAct::CheckAgain }` へ寄せる（同じ行に 2 つの道を作らない）。
/// Wave: RQ4
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnAct {
    Connect,
    CheckAgain,
    Test,
    Disconnect,
    Reconnect,
    Add,
    Remove,
    PasteSecret(String),
    AllowTool { tool: String, allowed: bool },
    Submit,
    /// PASSWORD-1101 の 2: 行の秘密（`name`）に password manager の項目を使う（Keychain の `connection_key(row, name)` に値でなく `item.stored()` を置く）
    UseItem { name: String, item: crate::domain::vault::SecretRef },
}

impl ConnAct {
    /// button の文字（英語・sentence case・AX の名前も同じ）。`Remove` は確認を開くので `…`・`Add` は行の key で文字が替わる（`cli` = `Choose program…`・他 = `Add`）
    /// ので呼ぶ側が `row` を渡す。`AllowTool` は tool の名前（checkbox の label）・`Submit` は描かない（空）。
    /// Wave: RQ4
    pub fn label(&self, row: &str) -> String {
        match self {
            ConnAct::Connect => "Connect".into(),
            // L4: `mcp-found` の行の Check again = 他の app（Claude Desktop・Claude Code）の設定を 1 回読む（押した時だけ）
            ConnAct::CheckAgain if row == "mcp-found" => "Look in Claude's settings".into(),
            ConnAct::CheckAgain => "Check again".into(),
            ConnAct::Test => "Test".into(),
            ConnAct::Disconnect => "Disconnect".into(),
            ConnAct::Reconnect => "Reconnect".into(),
            ConnAct::Add if row == "cli" => "Choose program…".into(),
            ConnAct::Add => "Add".into(),
            ConnAct::Remove => "Remove…".into(),
            ConnAct::PasteSecret(name) => format!("Paste {name}"),
            ConnAct::AllowTool { tool, .. } => tool.clone(),
            ConnAct::Submit => String::new(),
            ConnAct::UseItem { item, .. } => item.label(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionsModel {
    pub rows: Vec<Connection>,
}

/// 111 通目（docs 01 SS05・02 IA）: Connections の 4 つの群（上からこの順・行の無い群は見出しも出さない）。Account・Capability・OS の許可・Tool を 1 列に混ぜない。
pub const GROUPS: [&str; 4] = ["AI providers", "Apps & services", "Tools & MCP", "System access"];

/// 行の `key` → `GROUPS` の番号（`:` の前で決める）。`ai` = AI providers・`gui`（Screen control）/ `cli` / `mcp` / `mcp-found` = Tools & MCP・
/// OS の許可（`screen-recording`・`accessibility`・`microphone`）= System access・他（`github`・`web`・この先の service）= Apps & services。
pub fn group_of(key: &str) -> usize {
    match key.split(':').next().unwrap_or(key) {
        "ai" => 0,
        "gui" | "cli" | "mcp" | "mcp-found" => 2,
        "screen-recording" | "accessibility" | "microphone" => 3,
        _ => 1,
    }
}

/// TITLEBAR-ICONS-1101 の 3（owner 216「connectionも同じ。文字だけだとどこに何があるの代わりずらい。orcaを見習って」）: 行の左の相手の icon の出所 =
/// `theme::kind_icon` に渡す (同梱の logo の key, Mac の app の bundle id, Phosphor)。全部 None なら kind_icon が行の名前の頭文字の丸を描く（MCP の server = 名前で分かる）。
/// PROVIDER-ICONS-1101: `mcp-found`（Claude の設定を読む行）は Claude の logo が先。`ai`（使っている AI の確かめ・Check again）は Sparkles のまま（行の key は provider を持たない・
/// AI ごとの logo はすぐ下の account の塊 = `settings::accounts`）。
/// `ai` = Sparkles（使っている AI の名前は行の文・AI ごとの icon は下の account の塊 = `settings::accounts`）・`github` = GitHub の logo・`web` = Globe・
/// `gui` = 押す指・`cli` / `cli:*` = Terminal・`mcp` = Plug・`mcp-found` = Claude の app（Claude の設定を読む行・無ければ Plug）・OS の許可 = Monitor / Accessibility / Mic。
/// 例: `let (logo, app, icon) = row_icon(&c.key); theme::kind_icon(ui, at.center(), logo, app, icon, &c.name, t.text_primary)`
/// Wave: TITLEBAR-ICONS-1101, PROVIDER-ICONS-1101
pub fn row_icon(key: &str) -> (Option<&'static str>, Option<&'static str>, Option<Icon>) {
    match key.split(':').next().unwrap_or(key) {
        "ai" => (None, None, Some(Icon::Sparkles)),
        "github" => (None, None, Some(Icon::GithubLogo)),
        "web" => (None, None, Some(Icon::Globe)),
        "gui" => (None, None, Some(Icon::MousePointerClick)),
        "cli" => (None, None, Some(Icon::SquareTerminal)),
        "mcp" if key == "mcp" => (None, None, Some(Icon::Plug)),
        "mcp-found" if key == "mcp-found" => {
            let claude = crate::domain::terminal::agent_kind("claude");
            (claude.and_then(|k| k.logo), claude.and_then(|k| k.app), Some(Icon::Plug))
        }
        "screen-recording" => (None, None, Some(Icon::Monitor)),
        "accessibility" => (None, None, Some(Icon::Accessibility)),
        "microphone" => (None, None, Some(Icon::Mic)),
        // PASSWORD-1101: password manager の行 = その app の icon（Keychain は頭文字の丸）
        "pm" => (None, crate::domain::vault::Manager::of_row(key).and_then(crate::domain::vault::Manager::app), None),
        _ => (None, None, None),
    }
}

/// Connections の画面（凡例と行）。
/// 契約（RQ4 から・本体で足す）: 行ごとに `Connection::acts` の手を button（`AllowTool` は行の下の checkbox）で描き、押した手を
/// `Intent::Connection { row: key, act }` で返す。`Shell::connection_confirm` がその行の key の間は行の下に `theme::confirm_inline`（`Remove` と `Keep`）
/// 見出しの行 `Trusted commands`（key `cli`・`Add` = `Choose program…`）と `MCP servers`（key `mcp`・`Add`）は
/// 0 件でも描く（`No commands yet` / `No MCP servers yet` の 1 行と `Add`）。`confirm` = `Shell::connection_confirm`: 行の `Remove…` は確認を開くだけ（`*confirm = key`・
/// Intent を返さない）。その key の行の下に確認を描き、行に `Remove` が在れば `Remove “{name}”?` の `Remove` = `ConnAct::Remove`、無ければ足す前の Trusted command の
/// `Add “{name}”?` の `Allow` = `ConnAct::Connect`。`Keep` は `*confirm = None` だけ。
/// 保証する: 押せるのに効かない button を描かない（`acts` に無い手は描かない）。保証しない: 確かめの結果（app.rs が裏の thread で測り `checked_at` と detail に書く）。
/// Wave: RQ4
/// CONNECTIONS-1101（owner 196）: 3 つ目の引数は `Shell`（`connection_confirm` = 行内の確認・`connecting` / `field` / `field_name` = 行の下にその場で開く欄）。
/// ACCOUNTS-1101（owner 203）: 4 つ目の引数 `ai` = AI providers の群の中身（app.rs が `settings::accounts` を渡す = Settings › AI & Models の Accounts と同じ部品）。
/// 群の見出しと model の `ai` の行（今使っている AI の確かめと `Check again`）の下に描く。`ai:found:*` の行は無くした（account の行が持つ）。
/// 例（app.rs）: `ui::connections::draw(ui, &model, &mut self.shell, &mut |ui| settings::accounts(ui, &self.store.ui, busy, &emails, &unsigned))`
pub fn draw(ui: &mut egui::Ui, model: &ConnectionsModel, shell: &mut shell::Shell, ai: &mut dyn FnMut(&mut egui::Ui) -> Option<Intent>) -> Option<Intent> {
    // UI-SWEEP-1101 追記（owner 209）: button の枠は Settings・Connections・Runs の中だけ
    let _framed = theme::framed(ui.ctx());
    let t = theme::tokens(ui.ctx());
    let mut intent = None;
    // CONNECTIONS-1101（owner 196 の 7）: 題は 24 の太字（`content_opening` の段は UI-SWEEP が全 page で揃える）・凡例の行は消した（196 の 1）
    let mut title = Type::HeadingLg.font();
    title.size = TITLE_SIZE;
    ui.add(egui::Label::new(egui::RichText::new("Connections").font(title).color(t.text_primary)).selectable(true));
    ui.add_space(theme::SP_1_5);
    // 行の下の欄の最初の frame の focus の印（閉じたら消す = 開き直すとまた focus）
    if shell.connecting.is_none() {
        ui.data_mut(|d| d.remove::<String>(egui::Id::new(FIELD_OPENED)));
    }
    let field_at = shell.connecting.as_deref().map(|k| field_row(k, &model.rows));
    // 111 通目: 行は `GROUPS` の群ごと（見出し + 行）。群の中は model の順のまま
    let mut rows: Vec<&Connection> = model.rows.iter().collect();
    rows.sort_by_key(|c| group_of(&c.key)); // 安定
    // 行は上から 30ms ずつずらして fade + slide（§15.2-1 list の stagger・起点は View の替わり目）
    let t0 = shell::view_since(ui.ctx());
    let mut group = None;
    for (i, c) in rows.into_iter().enumerate() {
        let g = group_of(&c.key);
        if group != Some(g) {
            // ACCOUNTS-1101: AI providers の群の終わり（次の群の見出しの前）に AI ごとの account の部品
            if group == Some(0) {
                intent = ai(ui).or(intent);
            }
            group = Some(g);
            ui.add_space(theme::SP_3);
            theme::heading(ui, Type::Group, GROUPS[g], t.text_primary); // UI-SWEEP-1101: 群の題（page の題より小さく行より大きい太字）
            theme::rule(ui);
        }
        let start = t0 + motion::stagger(ui.ctx(), i) as f64;
        motion::fade_slide_in(ui, ("kangae.conn_row", i), start, |ui| {
            let mut act = row(ui, c);
            // RQ4: 行の `Remove…` は確認を開くだけ（Remove はこの確認の `Remove` からだけ届く）
            if act == Some(ConnAct::Remove) {
                shell.connection_confirm = Some(c.key.to_string());
                act = None;
            }
            // 行内の確認（`Remove…` と足す前の Trusted command）。答えは同じ行の手として返す・`Keep` は閉じるだけ
            if shell.connection_confirm.as_deref() == Some(c.key.as_ref()) {
                let removing = c.acts.contains(&ConnAct::Remove);
                let question = if removing { format!("Remove “{}”? Its settings and saved keys are removed.", c.name) } else { format!("Add “{}”? Shibubu asks before every run.", c.name) };
                match theme::confirm_inline_with(ui, &question, if removing { "Remove" } else { "Allow" }) {
                    Some(true) => act = Some(if removing { ConnAct::Remove } else { ConnAct::Connect }),
                    Some(false) => shell.connection_confirm = None,
                    None => {}
                }
            }
            // CONNECTIONS-1101（owner 196 の 9・10）: その場で開く欄（Paste token の鍵・Web の URL・MCP の Add・Trusted command の引数）
            if field_at.as_deref() == Some(c.key.as_ref()) {
                if let Some(i) = row_field(ui, shell) {
                    intent = Some(i);
                }
            }
            match act {
                // 前からの `Check again` の口（recheck の行・画面の操作）
                Some(ConnAct::CheckAgain) if c.recheck => intent = Some(Intent::RecheckGui),
                Some(act) => intent = Some(Intent::Connection { row: c.key.to_string(), act }),
                None => {}
            }
        });
    }
    if group == Some(0) {
        intent = ai(ui).or(intent);
    }
    intent
}

/// 1 行（名前・実状態・状態語・行の手）。押された手（`recheck` の行の `Check again` は `ConnAct::CheckAgain`）。状態語の色は替わる時に cross-fade。
/// RQ4: `acts` の手を右に左から button で（`AllowTool` は行の下の checkbox・押すと許す / 外すを反転した手）・`checked_at` は detail の後ろに `checked {何分前}`。
pub(crate) fn row(ui: &mut egui::Ui, c: &Connection) -> Option<ConnAct> {
    let t = theme::tokens(ui.ctx());
    let mut pressed = None;
    let detail = match c.checked_at {
        Some(at) => {
            // 何分前（tests/ui_portal.rs が #[path] で取り込むので crate の時刻の口を引かない）。言い方が替わる 1 分後に描き直す（idle の frame は 0）
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(60));
            let ago = match (now - at).max(0) {
                0..=59 => "just now".to_owned(),
                d @ 60..=3599 => format!("{} min ago", d / 60),
                d @ 3600..=86399 => format!("{} h ago", d / 3600),
                d => format!("{} days ago", d / 86400),
            };
            format!("{} · checked {ago}", c.detail)
        }
        None => c.detail.to_string(),
    };
    let detail = shown_detail(c, &detail);
    // CONNECTIONS-1101（owner 196「スペース無駄にしないで」）: 広い段は 1 行（`ROW_H`）= 左に名前と短い説明（1 行で切る・全文は hover）・右に状態の点と語・主な手 1 つ・`•••`。
    // 狭い（`ROW_NARROW` 未満）時は今まで通り手の列を文の下へ（200 字の名前と 4 行の誤りでも重ならない・111 通目）。
    let acts = row_actions(c);
    ui.scope(|ui| {
        let w = ui.available_width();
        if w < ROW_NARROW {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space(theme::SP_3);
                    mark(ui, c);
                    ui.vertical(|ui| text_block(ui, c, &detail));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(theme::SP_3);
                    action_cluster(ui, c, &acts, &mut pressed, false);
                });
            });
        } else {
            ui.allocate_ui_with_layout(Vec2::new(w, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_height(ROW_H);
                ui.add_space(theme::SP_3);
                let right = ROW_RIGHT_W.min(w * 0.5);
                ui.allocate_ui_with_layout(Vec2::new((w - right - theme::SP_3 * 2.0).max(0.0), ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| text_line(ui, c, &detail));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(theme::SP_3);
                    action_cluster(ui, c, &acts, &mut pressed, true);
                });
            });
        }
    });
    // RQ4（RQ-016）: server の tool を Engine に使わせるか（checkbox の名前 = tool の名前）
    let tools: Vec<(&String, bool)> = c.acts.iter().filter_map(|a| if let ConnAct::AllowTool { tool, allowed } = a { Some((tool, *allowed)) } else { None }).collect();
    if !tools.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(theme::SP_3);
            theme::label(ui, Type::Helper, "Tools the AI may use", t.text_tertiary);
            for (tool, allowed) in tools {
                let mut on = allowed;
                if ui.checkbox(&mut on, Type::Helper.text(tool.as_str())).changed() {
                    pressed = Some(ConnAct::AllowTool { tool: tool.clone(), allowed: on });
                }
            }
        });
    }
    theme::rule(ui);
    pressed
}

/// CONNECTIONS-1101: Paste token の行の下の鍵の欄の key（`Shell::connecting` に置く・`row` に `:` が入ってよい = `mcp:penpot`）。
/// 例: `secret_key("github", "token")` → `"secret:github:token"`・`secret_of("secret:mcp:penpot:API_KEY")` → `Some(("mcp:penpot", "API_KEY"))`
pub fn secret_key(row: &str, name: &str) -> String {
    format!("secret:{row}:{name}")
}
pub fn secret_of(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("secret:")?.rsplit_once(':')
}

/// PASSWORD-1101 の 2: Paste の欄の下で password manager の項目を選べる行 = 使う瞬間に `vault::resolve` が参照を値へ読む口が在る行（GitHub の token・Web data の token）。
/// MCP の server の env は選べない（server を起こす時に `McpConfig::resolved` が読む = その時に Touch ID を待てない・Paste だけ）。
/// Wave: PASSWORD-1101
pub fn takes_items(row: &str) -> bool {
    matches!(row, "github" | "web")
}

/// 広い段の 1 行の高さ（CONNECTIONS-1101: 36〜40・前は docs 03 の 64 以上で約 96 に伸びていた → SIZES-1101 / 226 で一覧の行 `LIST_ROW_H` 32 = Orca `h-8`・
/// 中の button は 28 / 24）と、手の列を下へ回す幅・右の列の幅の目安。
const ROW_H: f32 = theme::LIST_ROW_H;
const ROW_NARROW: f32 = 600.0;
const ROW_RIGHT_W: f32 = 300.0;

/// 行で押せる手 1 つ。`Open settings` は `Intent` を返さず行の中で開く（`Connection::settings`）。
#[derive(Clone, Debug, PartialEq)]
enum RowAct {
    Act(ConnAct),
    OpenSettings(&'static str),
}

/// 行の手を 1 本の列に（先頭 = 主な手 1 つ・残りは `•••`）。主な手の優先: `Open settings`（次の一手がそれの行）→ 始める手（Connect / Reconnect / Add）→ Check again → 先頭。
/// `recheck` の行の `Check again` は `acts` に在れば重ねない。`AllowTool` と `Submit` は行の手でない。
fn row_actions(c: &Connection) -> Vec<RowAct> {
    let mut v: Vec<RowAct> = c.acts.iter().filter(|a| !matches!(a, ConnAct::AllowTool { .. } | ConnAct::Submit)).cloned().map(RowAct::Act).collect();
    if c.recheck && !v.contains(&RowAct::Act(ConnAct::CheckAgain)) {
        v.push(RowAct::Act(ConnAct::CheckAgain));
    }
    let open = c.settings.filter(|_| c.detail.ends_with("· Open settings")).map(RowAct::OpenSettings);
    let rank = |a: &RowAct| match a {
        RowAct::OpenSettings(_) => 0,
        RowAct::Act(ConnAct::Connect | ConnAct::Reconnect | ConnAct::Add | ConnAct::PasteSecret(_)) => 1,
        RowAct::Act(ConnAct::CheckAgain) => 2,
        _ => 3,
    };
    v.extend(open);
    v.sort_by_key(rank); // 安定: 同じ順位は acts の順のまま
    v
}

fn act_label(c: &Connection, a: &RowAct) -> String {
    match a {
        RowAct::Act(a) => a.label(&c.key),
        // CONNECTIONS-1101（owner 196 の 4）: 許可が要る行の手は `Allow`（System Settings の該当の所を開く）
        RowAct::OpenSettings(_) => "Allow".into(),
    }
}

/// TITLEBAR-ICONS-1101 の 3: 行の左の相手の icon（`row_icon` の出所を `theme::kind_icon` で・`ICON_UI` 四方の席・どの群も同じ大きさ）。
fn mark(ui: &mut egui::Ui, c: &Connection) {
    let (logo, app, icon) = row_icon(&c.key);
    let (_, at) = ui.allocate_space(Vec2::splat(theme::ICON_UI));
    theme::kind_icon(ui, at.center(), logo, app, icon, &c.name, theme::tokens(ui.ctx()).text_primary);
}

/// 名前と detail（どちらも折り返す）。detail の全文は hover にも。
fn text_block(ui: &mut egui::Ui, c: &Connection, detail: &str) {
    let t = theme::tokens(ui.ctx());
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add(egui::Label::new(Type::BodyStrong.text(c.name.to_string()).color(t.text_primary)).wrap());
    ui.add(egui::Label::new(Type::Helper.text(detail).color(t.text_secondary)).wrap());
}

/// 広い段の 1 行の文（相手の icon・名前・短い説明）。名前と説明は 1 行で切る（切った時は全文を hover に・egui の Label の既定）。
fn text_line(ui: &mut egui::Ui, c: &Connection, detail: &str) {
    let t = theme::tokens(ui.ctx());
    ui.spacing_mut().item_spacing.x = theme::SP_3;
    mark(ui, c);
    ui.add(egui::Label::new(Type::BodyStrong.text(c.name.to_string()).color(t.text_primary)).truncate());
    ui.add(egui::Label::new(Type::Helper.text(detail).color(t.text_secondary)).truncate());
}

/// CONNECTIONS-1101: 行の短い説明。末尾の ` · Open settings` は button（`Allow`）が言うので文から落とす（残りが `Off` だけなら `Permission needed`）。
fn shown_detail(c: &Connection, detail: &str) -> String {
    match detail.split_once(" · Open settings") {
        Some((rest, tail)) if c.settings.is_some() => {
            let rest = if rest.trim() == "Off" { "Permission needed" } else { rest.trim() };
            format!("{rest}{tail}")
        }
        _ => detail.to_owned(),
    }
}

/// 行の下の欄を描く行の key: 鍵の欄 = その row・Trusted command の引数（`cli:<id>`・足す前は行が無い）= 在ればその行・無ければ見出しの行 `cli`・他 = 同じ key。
fn field_row(connecting: &str, rows: &[Connection]) -> String {
    if let Some((row, _)) = secret_of(connecting) {
        return row.to_owned();
    }
    if rows.iter().any(|c| c.key == connecting) {
        return connecting.to_owned();
    }
    connecting.split(':').next().unwrap_or(connecting).to_owned()
}

/// 行の下の欄の最初の frame に focus を置いた印（egui の temp・中身は開いた欄の key）
const FIELD_OPENED: &str = "kangae.conn_field_opened";

/// CONNECTIONS-1101（owner 196 の 9・10）: `Shell::connecting` の行の下にその場で開く 1 行の欄（上の欄に focus を移さない）。Enter か主の button で送り、`×` で閉じる
/// （Esc も app.rs の連鎖で閉じる）。鍵（`secret:<row>:<name>`）は伏せ字の欄で、送る手は同じ `PasteSecret(name)`（app.rs が `Shell::field` から取って Keychain へ・
/// Intent に値を載せない）。他は `Submit`（文は app.rs が `Shell::field` から読む）。MCP の Add は名前の欄（任意）も（`Shell::field_name`・`McpConfig::add_named`）。
fn row_field(ui: &mut egui::Ui, shell: &mut shell::Shell) -> Option<Intent> {
    let key = shell.connecting.clone()?;
    let secret = secret_of(&key).map(|(r, n)| (r.to_owned(), n.to_owned()));
    let (hint, send) = match (&secret, key.as_str()) {
        (Some((_, n)), _) => (format!("Paste the {n} here"), "Save"),
        (None, "web") => ("Web data URL, e.g. https://api.example.com/items".to_owned(), "Test"),
        (None, k) if k.starts_with("cli:") => ("Fixed arguments for the program · Enter for none".to_owned(), "Save"),
        _ => ("Command or JSON that starts the MCP server".to_owned(), "Add"),
    };
    let mut out = None;
    let mut close = false;
    ui.horizontal(|ui| {
        ui.add_space(theme::SP_3);
        if key == "mcp" {
            let name = egui::TextEdit::singleline(&mut shell.field_name).hint_text("Name (optional)").font(Type::Body.font());
            let _ = theme::ax_name(ui.add_sized([160.0, theme::INPUT_H], name), "MCP server name");
        }
        let w = (ui.available_width() - 120.0).max(160.0);
        let edit = egui::TextEdit::singleline(&mut shell.field.0).hint_text(hint.as_str()).password(secret.is_some()).font(Type::Body.font());
        let r = ui.add_sized([w, theme::INPUT_H], edit);
        let r = theme::ax_name(r, secret.as_ref().map_or(hint.as_str(), |(_, n)| n.as_str()));
        let opened = egui::Id::new(FIELD_OPENED);
        if ui.data(|d| d.get_temp::<String>(opened)).as_deref() != Some(key.as_str()) {
            r.request_focus();
            ui.data_mut(|d| d.insert_temp(opened, key.clone()));
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let pressed = theme::button_compact(ui, send, Variant::Primary).clicked();
        close = theme::ax_name(theme::button_compact(ui, theme::icon_glyph(Icon::X), Variant::Ghost), "Close").clicked();
        if enter || pressed {
            out = Some(match &secret {
                Some((row, n)) => Intent::Connection { row: row.clone(), act: ConnAct::PasteSecret(n.clone()) },
                None => Intent::Connection { row: key.clone(), act: ConnAct::Submit },
            });
        }
    });
    // PASSWORD-1101 の 2: GitHub と Web data の token の欄の下に、繋いだ password manager の項目（名前だけ・押す = 値でなく参照を置く `UseItem`）
    if let Some((row, n)) = secret.as_ref().filter(|(r, _)| takes_items(r) && !shell.vault_items.is_empty()) {
        let t = theme::tokens(ui.ctx());
        ui.add_space(theme::SP_2);
        ui.horizontal(|ui| {
            ui.add_space(theme::SP_3);
            ui.vertical(|ui| {
                theme::label(ui, Type::Helper, crate::domain::vault::PICK, t.text_secondary);
                let filter = egui::TextEdit::singleline(&mut shell.field_name).hint_text("Filter items").font(Type::Body.font());
                let _ = theme::ax_name(ui.add_sized([240.0, theme::INPUT_H], filter), "Filter items");
                let want = shell.field_name.trim().to_lowercase();
                egui::ScrollArea::vertical().id_salt("kangae.vault_items").max_height(180.0).show(ui, |ui| {
                    for item in shell.vault_items.iter().filter(|i| want.is_empty() || i.label().to_lowercase().contains(&want)) {
                        if theme::button_compact(ui, &item.label(), Variant::Ghost).clicked() {
                            out = Some(Intent::Connection { row: row.clone(), act: ConnAct::UseItem { name: n.clone(), item: item.clone() } });
                        }
                    }
                });
            });
        });
    }
    if close {
        shell.end_connecting();
        return None;
    }
    out
}

/// 状態語・主な手 1 つ・`•••`。`right_to_left` の時は右から置く（画面では 状態語 → 主な手 → `•••`）。
fn action_cluster(ui: &mut egui::Ui, c: &Connection, acts: &[RowAct], pressed: &mut Option<ConnAct>, right_to_left: bool) {
    let t = theme::tokens(ui.ctx());
    // CONNECTIONS-1101（owner 196 の 2・3・5）: 状態は小さい点と語の 2 つだけ（緑の Connected・赤の Not connected・確かめている間は灰の Checking）
    let on = connected(c.state);
    let (word, gloss, color) = match (c.checking, on) {
        (true, _) => ("Checking", "Shibubu is checking it now", t.text_tertiary),
        (false, true) => ("Connected", "Shibubu can use it now", t.success),
        (false, false) => ("Not connected", "Shibubu can't use it yet · the note on the left says what it needs", t.danger),
    };
    let color = motion::cross_fade(ui.ctx(), ui.next_auto_id().with("kangae.state"), color);
    let state_label = |ui: &mut egui::Ui| {
        let dot = |ui: &mut egui::Ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, color);
        };
        ui.spacing_mut().item_spacing.x = theme::SP_1;
        // right_to_left は右から置く = 語を先に（画面では 点 → 語）
        if right_to_left {
            ui.add(egui::Label::new(Type::Small.text(word).color(color))).on_hover_text(gloss);
            dot(ui);
        } else {
            dot(ui);
            ui.add(egui::Label::new(Type::Small.text(word).color(color))).on_hover_text(gloss);
        }
    };
    let primary = |ui: &mut egui::Ui, a: &RowAct, pressed: &mut Option<ConnAct>| {
        let label = act_label(c, a);
        if c.checking && *a == RowAct::Act(ConnAct::CheckAgain) {
            ui.add_enabled_ui(false, |ui| {
                theme::button_compact(ui, "Checking", Variant::Ghost);
            });
        } else if theme::button_compact(ui, &label, if on { Variant::Ghost } else { Variant::Primary }).clicked() {
            match a {
                RowAct::Act(a) => *pressed = Some(a.clone()),
                RowAct::OpenSettings(url) => open_settings(url),
            }
        }
    };
    let more = |ui: &mut egui::Ui, rest: &[RowAct], pressed: &mut Option<ConnAct>| {
        // `•••` も helper と同じ形（Connections は `framed` の範囲の中 = border_strong の枠・owner 209）。入力欄の枠（border_input）を読ませない
        let r = ui.scope(|ui| {
            theme::native_buttons(ui);
            ui.menu_button(theme::icon_glyph(Icon::Ellipsis), |ui| {
                for a in rest {
                    let label = act_label(c, a);
                    // 危険な手は赤・確認は行の下の `Remove` / `Keep`（docs 00「Delete / Remove は赤」）
                    let text = if matches!(a, RowAct::Act(ConnAct::Remove)) { Type::Nav.text(label.as_str()).color(t.danger) } else { Type::Nav.text(label.as_str()) };
                    if ui.add(egui::Button::new(text).min_size(Vec2::new(196.0, 32.0))).clicked() {
                        match a {
                            RowAct::Act(a) => *pressed = Some(a.clone()),
                            RowAct::OpenSettings(url) => open_settings(url),
                        }
                        ui.close_menu();
                    }
                }
            })
        })
        .inner;
        theme::ax_name(r.response, &format!("More actions for {}", c.name));
    };
    // Not connected の行は主の手 1 つ（primary）・Connected の行は始める手（Use・Add）だけを主に（Ghost）、Check again などは `•••` の中（owner 196 の 3）
    let (first, rest) = match acts.split_first() {
        Some((f, r)) if !on || matches!(f, RowAct::Act(ConnAct::Connect | ConnAct::Add)) => (Some(f), r),
        _ => (None, acts),
    };
    if right_to_left {
        if !rest.is_empty() {
            more(ui, rest, pressed);
            ui.add_space(theme::SP_2);
        }
        if let Some(first) = first {
            primary(ui, first, pressed);
            ui.add_space(theme::SP_2);
        }
        state_label(ui);
    } else {
        state_label(ui);
        if let Some(first) = first {
            ui.add_space(theme::SP_2);
            primary(ui, first, pressed);
        }
        if !rest.is_empty() {
            ui.add_space(theme::SP_2);
            more(ui, rest, pressed);
        }
    }
}

/// System Settings の pane を 1 回開く（終わりは待たない = 画面を止めない）。Connections の `Open settings` と Mirror の面の `Open settings` が同じ道を通る。
pub fn open_settings(url: &str) {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/open").arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    });
}

/// 画面収録の許可の pane（Mirror の面の `Open settings` の行き先）。
pub const SCREEN_CAPTURE_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// MIRROR-AUTO-1101 B: 画面収録の許可が無い時の右上の知らせ（`toast::post_once`）の手の語。app.rs が `toast::draw` の答えをこの語で受けて `SCREEN_CAPTURE_SETTINGS` を開く。
/// Wave: MIRROR-AUTO-1101
pub const SCREEN_CAPTURE_ACTION: &str = "Open Settings";

#[cfg(test)]
mod tests {
    use super::*;
    /// CONNECTIONS-1101（owner 196 の 2）: 画面の状態は 2 つ = Connected（Connected・Ready・Verified）と Not connected（Available・Unavailable）。
    /// 負の対照: `connected` に Available を足す → sign in が要る行が緑になって赤。
    #[test]
    fn connections1101_only_usable_tiers_are_connected() {
        for (s, on) in [(DisplayTier::Unavailable, false), (DisplayTier::Available, false), (DisplayTier::Connected, true), (DisplayTier::Ready, true), (DisplayTier::Verified, true)] {
            assert_eq!(connected(s), on, "{s:?}");
        }
    }

    /// CONNECTIONS-1101（owner 196 の 1・3・4）: 凡例の行は無い・2 つの状態の語だけ・Connected の行の Check again は `•••` の中・Not connected の行は主の手が button で名前と 1 行に並ぶ。
    /// 負の対照: `action_cluster` の `first` の条件を外す → Connected の行に Check again の button が出て赤。
    #[test]
    fn connections1101_rows_are_one_line_without_a_legend() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        motion::set_reduce_motion(&ctx, true);
        ctx.enable_accesskit();
        let row = |key: &str, name: &str, state: DisplayTier, acts: Vec<ConnAct>| Connection { name: name.to_owned().into(), state, detail: "d".into(), recheck: false, checking: false, settings: None, key: key.to_owned().into(), acts, checked_at: None };
        let model = ConnectionsModel { rows: vec![row("web", "Web data", DisplayTier::Ready, vec![ConnAct::CheckAgain, ConnAct::Disconnect]), row("github", "GitHub", DisplayTier::Available, vec![ConnAct::PasteSecret("token".into())])] };
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 900.0))), ..Default::default() };
        let out = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui, &model, &mut shell::Shell::new(shell::View::Connections), &mut |_| None);
            });
        });
        let t = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
        let texts: Vec<String> = t.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect();
        assert!(!texts.iter().any(|x| x.contains(" = ")), "凡例の行が残った: {texts:?}");
        assert!(texts.iter().any(|x| x == "Connected") && texts.iter().any(|x| x == "Not connected"), "2 つの状態の語が無い: {texts:?}");
        assert!(!texts.iter().any(|x| x == "Checked" || x == "Available" || x == "Verified"), "段の語が画面に出た: {texts:?}");
        let buttons: Vec<String> = t.nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::Button).filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
        assert!(!buttons.iter().any(|b| b == "Check again"), "Connected の行の Check again が行に出た: {buttons:?}");
        assert!(buttons.iter().any(|b| b == "Paste token") && buttons.iter().any(|b| b == "More actions for Web data"), "{buttons:?}");
        let h = t.nodes.iter().find(|(_, n)| n.label() == Some("Paste token")).and_then(|(_, n)| n.bounds()).expect("Paste token の枠");
        let name = t.nodes.iter().find(|(_, n)| n.value() == Some("GitHub")).and_then(|(_, n)| n.bounds()).expect("GitHub の枠");
        assert!(((h.y0 + h.y1) / 2.0 - (name.y0 + name.y1) / 2.0).abs() < 4.0, "button と名前が 1 行に並ばない: {h:?} {name:?}");
    }

    /// UI-4a（sendback 68）→ CONNECTIONS-1101: 次の一手が `Open settings` で終わり行き先を持つ行だけ `Allow` の button がある（押さない = System Settings を開かない）
    #[test]
    fn ui4a_open_settings_is_a_button_only_when_the_row_says_so() {
        let buttons = |settings: Option<&'static str>, detail: &str| {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let c = Connection { name: "Screen control".into(), state: DisplayTier::Available, detail: detail.into(), recheck: true, checking: false, settings, key: "gui".into(), acts: vec![], checked_at: None };
            let out = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    row(ui, &c);
                });
            });
            let t = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
            t.nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::Button).filter_map(|(_, n)| n.label().map(str::to_owned)).collect::<Vec<_>>()
        };
        const URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
        let off = "Screen control didn't start, or Accessibility is off · Open settings";
        assert!(buttons(Some(URL), off).iter().any(|b| b == "Allow"), "{:?}", buttons(Some(URL), off));
        assert!(!buttons(None, off).iter().any(|b| b == "Allow"), "行き先の無い行に button");
        assert!(!buttons(Some(URL), "Ready · Accessibility on").iter().any(|b| b == "Allow"), "次の一手が別の行に button");
    }

    /// FX5（FX-028）→ CONNECTIONS-1101: 段 `Ready` は画面に `Ready` の語でも `Checked` でも出ず、`Connected`。
    #[test]
    fn fx5_ready_is_shown_as_connected_never_as_ready() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let c = Connection { name: "Screen Recording".into(), state: DisplayTier::Ready, detail: "Allowed".into(), recheck: false, checking: false, settings: None, key: "screen-recording".into(), acts: vec![], checked_at: None };
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                row(ui, &c);
            });
        });
        let t = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
        let texts: Vec<String> = t.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect();
        assert!(texts.iter().any(|x| x == "Connected"), "{texts:?}");
        assert!(!texts.iter().any(|x| x.contains("Ready") || x.contains("Checked")), "段の語が画面に出た: {texts:?}");
    }

    /// RQ4（RQ-015・caller の形・本体の前は赤）: 行の `acts` は左から button になり、押した手が `Intent::Connection { row: key, act }` で返る。
    /// `acts` に無い手（ここでは `Remove…`）は描かない。`checked_at` が在れば detail の後ろに `checked HH:MM`。
    #[test]
    fn rq4_row_acts_are_buttons_and_check_time_is_shown() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let c = Connection {
            name: "Web data".into(),
            state: DisplayTier::Connected,
            detail: "api.example.com".into(),
            recheck: false,
            checking: false,
            settings: None,
            key: "web".into(),
            acts: vec![ConnAct::CheckAgain, ConnAct::Disconnect],
            checked_at: Some(0),
        };
        let model = ConnectionsModel { rows: vec![c] };
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui, &model, &mut shell::Shell::new(shell::View::Connections), &mut |_| None);
            });
        });
        let t = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
        let buttons: Vec<String> = t.nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::Button).filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
        // 111 通目 → CONNECTIONS-1101: Connected の行は Check again も `•••` の menu（開くまで描かない）
        assert!(!buttons.iter().any(|b| b == "Check again"), "Connected の行の Check again が行に出た: {buttons:?}");
        assert!(buttons.iter().any(|b| b == "More actions for Web data"), "`•••` が無い: {buttons:?}");
        assert!(!buttons.iter().any(|b| b == "Disconnect"), "副の手が行に並んだ（主は 1 つ）: {buttons:?}");
        assert!(!buttons.iter().any(|b| b == "Remove…"), "acts に無い手を描いた: {buttons:?}");
        let texts: Vec<String> = t.nodes.iter().filter_map(|(_, n)| n.label().or(n.value()).map(str::to_owned)).collect();
        assert!(texts.iter().any(|x| x.contains("checked ")), "確かめた時刻が出ない: {texts:?}");
        // label の表（本体あり）: Add は行で文字が替わる・Submit は描かない
        assert_eq!(ConnAct::Add.label("cli"), "Choose program…");
        assert_eq!(ConnAct::Add.label("mcp"), "Add");
        assert_eq!(ConnAct::CheckAgain.label("mcp-found"), "Look in Claude's settings");
        assert_eq!(ConnAct::PasteSecret("token".into()).label("github"), "Paste token");
        assert!(ConnAct::Submit.label("web").is_empty());
    }

    /// L1（109 通目・20）: detail が長くても、文は button に重ならない（button の上に文字が乗らない）。
    /// L4 の行（111 通目）と合わせた後も同じ性質を、文と button の外枠の重なりで見る（INTEG 473f4037: x だけ比べると、`ROW_NARROW` 未満で
    /// button が文の下の段に回った形〈重なっていない〉も赤になった）。幅は 2 つ = 狭い段（520・button は下）と広い段（720・button は右）。
    /// 負の対照: `row` の広い段の右の列の幅（`right`）を 0 にする → 文が手の下まで伸びて 720 で赤。
    #[test]
    fn l1_row_text_stops_before_the_buttons() {
        let ctx = egui::Context::default();
        theme::install(&ctx, egui::ThemePreference::Light);
        ctx.enable_accesskit();
        let c = Connection {
            name: "A connection with a rather long name".into(),
            state: DisplayTier::Connected,
            // 広い段の全幅でも折り返す長さ（文が右の列の幅で止まっている事を見る）
            detail: "api.example.com · a very long note that keeps going and going well past the buttons, and on past the edge of a wide window so that it has to wrap".into(),
            recheck: true,
            checking: false,
            settings: None,
            key: "web".into(),
            acts: vec![ConnAct::CheckAgain, ConnAct::Disconnect],
            checked_at: None,
        };
        for width in [520.0_f32, 720.0] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 400.0));
            let mut nodes = Vec::new();
            for _ in 0..2 {
                let out = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        row(ui, &c);
                    });
                });
                nodes = out.platform_output.accesskit_update.map(|u| u.nodes).unwrap_or_default();
            }
            let buttons: Vec<_> = nodes.iter().filter(|(_, n)| n.role() == egui::accesskit::Role::Button).filter_map(|(_, n)| n.bounds()).collect();
            let texts: Vec<_> = nodes.iter().filter(|(_, n)| n.role() != egui::accesskit::Role::Button && n.value().is_some_and(|v| v.starts_with("api.example.com") || v.starts_with("A connection"))).filter_map(|(_, n)| n.bounds()).collect();
            assert!(!buttons.is_empty() && !texts.is_empty(), "{width}: button か文が AX に無い");
            for text in &texts {
                for b in &buttons {
                    let apart = text.x1 <= b.x0 || b.x1 <= text.x0 || text.y1 <= b.y0 || b.y1 <= text.y0;
                    assert!(apart, "{width}: 文 {text:?} が button {b:?} に重なった");
                }
            }
        }
    }

    /// 111 通目（QA K-01・K-02）: 200 字の名前・4 行の誤り・10 個の tool でも、文と手の外枠が重ならない（960 と 1440 と 420 の幅）。
    /// 失敗の行は主な手が 1 つ（Reconnect）で、他は `•••` の中。負の対照: `row` の左の文を `allocate_ui_with_layout` の幅指定なしの横並びに戻す → 文が手に重なって赤。
    #[test]
    fn rq_n_long_rows_never_overlap_their_actions() {
        let long = "n".repeat(200);
        let detail = "The server didn't answer\nSecond error line\nThird error line\nFourth error line · Reconnect";
        let tools: Vec<ConnAct> = (0..10).map(|i| ConnAct::AllowTool { tool: format!("tool_{i}"), allowed: true }).collect();
        for width in [420.0_f32, 960.0, 1440.0] {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            ctx.enable_accesskit();
            let mut acts = vec![ConnAct::Reconnect, ConnAct::CheckAgain, ConnAct::Disconnect, ConnAct::Remove];
            acts.extend(tools.clone());
            let c = Connection { name: long.clone().into(), state: DisplayTier::Unavailable, detail: detail.into(), recheck: false, checking: false, settings: None, key: "mcp:x".into(), acts, checked_at: None };
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 900.0)));
            let out = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    row(ui, &c);
                });
            });
            let t = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
            let rects = |role: egui::accesskit::Role| -> Vec<egui::Rect> {
                t.nodes.iter().filter(|(_, n)| n.role() == role).filter_map(|(_, n)| n.bounds().map(|b| egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))).collect()
            };
            let (labels, buttons) = (rects(egui::accesskit::Role::Label), rects(egui::accesskit::Role::Button));
            for l in &labels {
                for b in &buttons {
                    assert!(!l.shrink(1.0).intersects(*b), "幅 {width}: 文 {l:?} が button {b:?} に重なった");
                }
            }
            assert!(!buttons.is_empty(), "幅 {width}: 手が無い");
        }
    }

    /// 111 通目（docs 01 SS05）: 行は 4 つの群（AI providers → Apps & services → Tools & MCP → System access）の見出しの下に群ごとに並ぶ（model の順が混ざっていても・
    /// 群の中は model の順）。行の無い群は見出しも出さない。ACCOUNTS-1101: `ai`（account の部品・ここでは `Grok` の 1 行）は AI providers の群の行の後・次の群の見出しの前。
    /// 負の対照: `draw` の `rows.sort_by_key(…group_of…)` を外す → Screen Recording が AI providers の見出しの上に出て赤。`ai` を群の終わりで呼ばない → `Grok` が無く赤。
    #[test]
    fn l4_rows_sit_under_their_group_heading_in_order() {
        let row = |key: &str, name: &str, acts: Vec<ConnAct>| Connection { name: name.to_owned().into(), state: DisplayTier::Available, detail: "d".into(), recheck: false, checking: false, settings: None, key: key.to_owned().into(), acts, checked_at: None };
        let texts = |rows: Vec<Connection>| {
            let ctx = egui::Context::default();
            theme::install(&ctx, egui::ThemePreference::Light);
            motion::set_reduce_motion(&ctx, true);
            ctx.enable_accesskit();
            let model = ConnectionsModel { rows };
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 2400.0))), ..Default::default() };
            let out = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw(ui, &model, &mut shell::Shell::new(shell::View::Connections), &mut |ui| {
                        ui.label("Grok");
                        None
                    });
                });
            });
            let t = out.platform_output.accesskit_update.expect("AccessKit の木が無い");
            // 見出しは Label と、その中の TextRun の 2 つの node になる（TextRun は数えない）
            let mut v: Vec<(f64, String, egui::accesskit::Role)> = t.nodes.iter().filter(|(_, n)| n.role() != egui::accesskit::Role::TextRun).filter_map(|(_, n)| Some((n.bounds()?.y0, n.label().or(n.value())?.to_owned(), n.role()))).collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            v
        };
        let names = ["AI", "Grok", "GitHub", "x server", "Screen control", "Screen Recording"];
        let got = texts(vec![
            row("screen-recording", "Screen Recording", vec![]),
            row("github", "GitHub", vec![]),
            row("mcp:x", "x server", vec![]),
            row("ai", "AI", vec![ConnAct::CheckAgain]),
            row("gui", "Screen control", vec![ConnAct::CheckAgain]),
        ]);
        let order: Vec<&str> = got.iter().map(|(_, s, _)| s.as_str()).filter(|s| GROUPS.contains(s) || names.contains(s)).collect();
        assert_eq!(order, ["AI providers", "AI", "Grok", "Apps & services", "GitHub", "Tools & MCP", "x server", "Screen control", "System access", "Screen Recording"], "{got:?}");
        let only = texts(vec![row("github", "GitHub", vec![])]);
        let heads: Vec<&str> = only.iter().map(|(_, s, _)| s.as_str()).filter(|s| GROUPS.contains(s)).collect();
        assert_eq!(heads, ["Apps & services"], "行の無い群の見出しを出した");
        for (key, g) in [("ai", 0), ("ai:other", 0), ("github", 1), ("web", 1), ("gui", 2), ("cli:abc", 2), ("mcp-found:x", 2), ("accessibility", 3), ("microphone", 3)] {
            assert_eq!(group_of(key), g, "{key}");
        }
    }

    /// TITLEBAR-ICONS-1101 の 3: 行の左の icon の出所。決まった行は全部が違う icon（文字を読まずに分かる）・MCP の server は名前の頭文字（None, None）・
    /// 他の app の設定を読む行は Claude の app の icon（無ければ Plug）。負の対照（未実行）: `web` の腕を消す → 頭文字になって赤・`mcp:*` を Plug にする → server が見分けられず赤
    #[test]
    fn titlebar_icons_3_each_fixed_row_has_its_own_icon() {
        let fixed = ["ai", "github", "web", "gui", "cli", "mcp", "screen-recording", "accessibility", "microphone"];
        let icons: Vec<Icon> = fixed.iter().map(|k| row_icon(k).2.unwrap_or_else(|| panic!("{k} に icon が無い"))).collect();
        let mut seen = icons.clone();
        seen.sort_by_key(|i| format!("{i:?}"));
        seen.dedup();
        assert_eq!(seen.len(), fixed.len(), "同じ icon の行が在る: {icons:?}");
        assert_eq!(row_icon("cli:abc"), row_icon("cli"), "Trusted command の行は Terminal");
        assert_eq!(row_icon("mcp:penpot"), (None, None, None), "MCP の server は頭文字");
        assert_eq!(row_icon("mcp-found:penpot"), (None, None, None), "他の app に在る server も頭文字");
        assert_eq!(row_icon("mcp-found"), (Some("claude"), Some("com.anthropic.claudefordesktop"), Some(Icon::Plug)), "Claude の設定の行は Claude の logo が先");
    }
}
