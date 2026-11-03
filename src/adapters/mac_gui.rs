//! W17-0 interface: Shibubu 独自の GUI 操作（MacGuiAdapter）と、macOS surface bridge に足す 5 つの口の JSON の契約。
//! 読む口（`windows`・`elements`・`gui_error`・bridge の `ax_tree`）は W17-1 で書いた。操作の口（`ax_press`・
//! `ax_set_value`・`ax_menu`・`key_to_pid` と `act` / `verify`・registry の `Adapter`・旧 lane の `run_desktop`）は W17-2 で書いた。
//! - 置き換えた物: 旧い GUI 操作の module（YC_GATE #1〜#3・#11）。YC W18-1 で消した。
//! - 経路: Rust → `surface::Bridge`（子 process・1 行 1 JSON）→ `bridge/macos-surface/KangaeSurfaceBridge.swift`（公開 API の
//!   AXUIElementPerformAction・AXUIElementSetAttributeValue・CGEvent.postToPid だけ）。非公開 API・座標の click・mouse は使わない。
//! - W17-1 interface: W2c の `gui::GuiAdapter`（observe / act / verify・yc-mvp `28e5b3d`）を merge して合わせた。外への口は `MacGui::{observe, act, verify}`
//!   （W17-0 の trait `MacGuiAdapter` も R-gui の `GuiAdapter` も実装 1 つ・使う側 0 なので消し、同じ署名の inherent method にした）。契約の型は `gui`・bridge の口の JSON は ここ。`ElementRef` は `gui::ElementRef`。
//!   旧 canvas の Desktop パネル（YC_GATE #5）は ① `MacGui::windows`（bridge の `apps` + `window_list`）・② `MacGui::elements`（`ax_tree`）で読む。
//! - 保存形式: 保存する物は無い（ref・handle・snapshot は bridge が生きている間だけ）。`AdapterKind::MacGui`（serde `"mac_gui"`・
//!   alias `"cua"`・FromStr `"mac_gui" | "cua" | "Cua" | "cua_browser_tab"`）。旧 lane の `ActionKind::Desktop` は `app_name`（空 = 旧 binding）を持つ。
//! - W17-2 interface: 旧 lane の実行 `MacGui::run_desktop`（`cua::run_desktop` の代わり・#3・#6）・`Chord` の FromStr（`GuiAction::Key` → `key_to_pid`）。
//!   Connections の行（#4・#10）は新しい口を作らず registry の `Adapter::check` → `readiness` → `sdk::tier_with` で出す。
use super::super::domain::{
    action::Effect,
    plural,
    binding::Source,
    capability::{CapabilityReadiness, CapabilityRecord, Reuse},
    run::{ErrorCode, Observation, Outcome},
};
use super::super::gui::{
    act_consent_prompt, human_holds, retarget, ActLog, ActReceipt, ActRecord, Control, GuiAction, GuiError, GuiTarget, HumanSignals,
    NotSent, WindowState, ACT_LOG_ENV, ACT_LOG_FILE, HUMAN_LEASE_LINE,
};
use super::super::model::ActionKind;
use super::super::surface::{
    act_consent_path, act_consents, find_bridge, now_ms, parse_windows, AppIdentity, Bridge, BridgeError, WindowInfo, BRIDGE_READY_LIMIT,
};
use super::sdk::{now, official_records, read_result, source, Adapter, AdapterKind, Progress, Route, SourceSync, Spec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

// ---- Rust の側の型（trait の入出力）----

/// 宛先の窓。R-G2 で Core の `gui::WindowRef` に移した（registry の `desktop.windows.list` の payload の形）。ここは同じ型を使う。
/// 使い方: `let w = WindowRef { pid: 501, window_id: 8812, app_name: "TextEdit".into(), title: String::new(), bundle_id: String::new(), document: String::new() };`
pub use super::super::gui::WindowRef;

/// AX の要素の参照（bridge の `"e<snapshot>.<n>"`・JSON では文字列 1 つ）。W2c の `gui::ElementRef` をそのまま使う（同じ型を 2 つ持たない）。
/// 使い方: `elements()` で得た ref をそのまま `press` / `set_text` / `GuiAction` に渡す。
/// 保証: 古い ref（同じ窓に新しい `elements()` があった）は bridge が `stale_element` で拒み、何も送らない。
pub use super::super::gui::ElementRef;

/// AX の要素 1 つ。bridge の `ax_tree` の応答の `elements[]` と同じ形（JSON の契約を兼ねる）。
/// label は AXTitle → AXDescription → 見出しの順で最初の空でない物。
/// 保証しない: value が画面の表示と一致する事（app が AXValue を持たない時は None）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    #[serde(rename = "ref")]
    pub r: ElementRef,
    pub role: String,
    #[serde(default)]
    pub subrole: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: Option<String>,
    /// AX の action 名（`"AXPress"` 等）。ここに無い action は bridge が送らない
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub value_settable: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// ENGINE-AUDIT W3: AXSelected（bridge の `ax_tree` の `selected`・属性の無い要素と前の bridge は None）
    #[serde(default)]
    pub selected: Option<bool>,
    /// W3: AXExpanded（同上 `expanded`）
    #[serde(default)]
    pub expanded: Option<bool>,
    #[serde(default)]
    pub depth: u32,
    #[serde(default)]
    pub parent: Option<ElementRef>,
}
fn yes() -> bool {
    true
}

/// 1 つの窓の要素の一覧（`elements()` の Ok）。
/// 使い方: `els.find(&label, &role)` で押す相手を 1 つに絞る（0 件 / 複数は押さない）。
/// 保証: 窓の全部（owner 256: `MacGui::read` は数でも時間でも切らず、読み終えた時だけ返す = 一覧に無い物は窓に無い）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Elements {
    pub app_name: String,
    pub snapshot: u64,
    pub items: Vec<Element>,
}

impl Elements {
    /// 名前（`name_of`・OPEN-INPLACE-1101 の 3 から label の無い一覧の行は子の字）と role が完全一致する要素がちょうど 1 つならそれ。
    /// 0 件 = `Err("見つかりません…")`・複数 = `Err("同じ名前が N 個…")`（押さない）。
    /// 使い方: `let el = els.find("保存", "AXButton")?;` 保証しない: 見つけた要素が押せる事（`actions` を見る）。
    pub fn find(&self, label: &str, role: &str) -> Result<&Element, String> {
        // owner 256: 窓の全部を読むので、親の表は 1 回だけ作る（要素ごとに `name_of` で作り直すと 5000 要素で 2500 万回）
        let by = self.by_ref();
        let hits: Vec<&Element> = self
            .items
            .iter()
            .filter(|e| e.role == role && self.name_in(&by, e) == label)
            .collect();
        match hits.as_slice() {
            [one] => Ok(one),
            [] => Err(format!("“{label}” ({role}) wasn't found in {}'s window. Add the step again", self.app_name)),
            many => Err(format!("There are {} controls named “{label}” ({role}). Not pressing any of them", many.len())),
        }
    }
    /// M2 G-c: label に `words`（利用者の言葉）を大文字小文字を問わず含む要素（窓そのもの = depth 0 を除く）がちょうど 1 つならそれ。`role` が空なら
    /// `action`（押す AX の action・既定 AXPress）を持つ要素の中から・空でなければその role の中から。0 件 = 見つからない・複数 = N 個ある の `Err`（押さない）。
    /// `words` が空白だけなら `Err`。G74b: 別の当たりの中に在る当たりは数えない（message の行と、その中の本文の字は 1 つ = 一番外の物。
    /// Chromium の行は外の group と中の字の両方が同じ label と AXShowMenu を持つ）。
    /// 使い方: `let el = els.find_containing("general", "", "AXPress")?;` 保証しない: 見つけた要素が利用者の意図した物である事（言葉を含む物が 1 つだけ、までしか見ない）。
    /// OPEN-INPLACE-1101 の 3（owner 241）: 名前は `name_of`（label か、label の無い一覧の行・升の子の字）で比べ、`role` が空の時は一覧の行（AXRow・AXCell・
    /// AXOutline の子）も押せる物に数える（AXPress を持たない行は選ぶ = bridge の `ax_press` に `AXSelect`〈AXSelected = true〉）。app を問わない一般の扱い。
    pub fn find_containing(&self, words: &str, role: &str, action: &str) -> Result<&Element, String> {
        let words = words.trim();
        if words.is_empty() {
            return Err("Name what to press, then ask again".into());
        }
        match self.find_containing_n(words, role, action, false, None) {
            Ok(one) => Ok(one),
            Err(0) => Err(format!("No control whose name contains “{words}” was found in {}'s window. Nothing was pressed", self.app_name)),
            Err(n) => Err(format!("There are {n} controls whose name contains “{words}” in {}'s window. Nothing was pressed. Use more of the name", self.app_name)),
        }
    }
    /// ENGINE-AUDIT W5: `find_containing` の数を返す形（Err = 当たりの数: 0 = 無い・2 以上 = 絞れない → `run_desktop_matching` が `Failed { TargetNotFound / TargetAmbiguous }` に）。
    /// `any` = 押せる形でない要素（見出し・子の字に名前がある button・自前の tile）も当たりに数える（bridge の梯子で押す）。`within` = (role, 名前) の道の中だけ
    /// （role が空なら名前だけ・名前は祖先の `name_in` に含まれる・大文字小文字は見ない）= 同じ名前が 2 つ以上の時に AI が道で 1 つに絞る（W3 の行の「in 道 › 道」）
    pub fn find_containing_n(&self, words: &str, role: &str, action: &str, any: bool, within: Option<(&str, &str)>) -> Result<&Element, usize> {
        let want = words.trim().to_lowercase();
        if want.is_empty() {
            return Err(0);
        }
        let by = self.by_ref();
        // W6: 押せる形の判定は gui::pressable 1 つ（Engine の探す段と同じ）＋ AXOutline の子（Element の親で分かる）
        let pressable = |e: &Element| if role.is_empty() { any || super::super::gui::pressable(&e.role, &e.actions, action) || row_like(&by, e) } else { e.role == role };
        let inside = |e: &Element| within.map_or(true, |(r, l)| ancestors(&by, e).any(|a| (r.is_empty() || a.role == r) && self.name_in(&by, a).to_lowercase().contains(&l.trim().to_lowercase())));
        let hits: Vec<&Element> = self.items.iter().filter(|e| e.depth > 0 && pressable(e) && inside(e) && self.name_in(&by, e).to_lowercase().contains(&want)).collect();
        let hits: Vec<&Element> = hits.iter().copied().filter(|h| !ancestors(&by, h).any(|a| hits.iter().any(|o| o.r == a.r))).collect();
        match hits.as_slice() {
            [one] => Ok(one),
            other => Err(other.len()),
        }
    }
    /// G74b（Identity Verification・owner 12:2x「削除が shibubu からできるか」）: 押す要素 `el` を囲む一番近い dialog（role AXSheet か subrole が
    /// AXDialog・AXSystemDialog・AXApplicationDialog の要素・窓そのものも含む）の中に、`texts` の 1 つ 1 つを label か値に含む要素が在るか。
    /// Ok = 何を見て一致と言ったかの 1 行・Err = dialog が無い / 無い字が在る理由（字数だけ・中身を出さない = message の本文を log に書かない）。
    /// 窓全体では探さない（後ろの行に同じ本文が残っていても、dialog が別の message を見せていれば押さない）。
    /// 使い方: `els.shown_in_dialog(&delete_button, &["本文".into()])?;` 保証しない: dialog の role が本物の app で上の表に入る事（Discord は未測）。
    /// Wave: M2
    pub fn shown_in_dialog(&self, el: &Element, texts: &[String]) -> Result<String, String> {
        let by = self.by_ref();
        let Some(d) = ancestors(&by, el).find(|e| is_dialog(e)) else {
            return Err(format!("The {} to press isn't inside a dialog, so what it would act on can't be checked. Nothing was pressed", el.role));
        };
        if texts.is_empty() || texts.iter().any(|t| t.trim().is_empty()) {
            return Err("The text the dialog should show is empty. Nothing was pressed".into());
        }
        let inside: Vec<&Element> = self.items.iter().filter(|e| ancestors(&by, e).any(|a| a.r == d.r)).collect();
        let shows = |t: &String| inside.iter().any(|e| e.label.contains(t.as_str()) || e.value.as_deref().is_some_and(|v| v.contains(t.as_str())));
        let chars: usize = texts.iter().map(|t| t.chars().count()).sum();
        match texts.iter().filter(|t| !shows(t)).count() {
            0 => Ok(format!("the dialog around it shows all {} expected texts ({chars} characters)", texts.len())),
            n => Err(format!("The dialog doesn't show {n} of the {} texts it should ({chars} characters in all), so it may be about something else. Nothing was pressed", texts.len())),
        }
    }
    /// ENGINE-AUDIT W7: 要素 `el` が確認の窓（`is_dialog` = sheet・dialog の subrole・窓そのものが dialog でも）の中か（`gui::asks` の `in_dialog`）
    pub fn in_dialog(&self, el: &Element) -> bool {
        let by = self.by_ref();
        let inside = ancestors(&by, el).any(is_dialog);
        inside
    }
    /// G74c（owner 14:4x「文字そのものを長押しすると出てくる」・Discord 15:0x の実測）: 開いている menu = AXMenuItem を子に持つ要素か、role AXMenu の要素。
    /// 無ければ None。Chromium の menu は AXRole が AXGroup（role description だけが menu・bridge は role description を出さない・言語で変わる）なので形で見る。
    /// 使い方: `if let Some(m) = els.open_menu() { /* 項目は AXMenuItem・m.actions に AXCancel があれば閉じられるかも */ }`
    /// 保証しない: その menu を Shibubu の手が開いた事（人が開いた menu も同じに見える）
    /// Wave: M2
    pub fn open_menu(&self) -> Option<&Element> {
        let by = self.by_ref();
        let parent = self.items.iter().find(|e| e.role == "AXMenuItem").and_then(|i| i.parent.as_ref()).and_then(|p| by.get(p).copied());
        parent.or_else(|| self.items.iter().find(|e| e.role == "AXMenu"))
    }
    /// ref → 要素（`ancestors` が親を辿る表）
    fn by_ref(&self) -> HashMap<&ElementRef, &Element> {
        self.items.iter().map(|e| (&e.r, e)).collect()
    }
    /// OPEN-INPLACE-1101 の 3（owner 241「No control whose name contains “Sounds”」・GarageBand の library の一覧の行は label が空で、名前は子の AXStaticText に在る）:
    /// 人が見る要素の名前 = label。label が空の一覧の行・升（AXRow・AXCell・AXOutline の子）は、その下の AXStaticText の値か label を読みの順に「 · 」で繋いだ物
    /// （同じ字は 1 回）。どちらも無ければ空。`Facts::controls` の行と `find_containing` の比べる字は同じこれ（読んだ木で選んだ名前で押せる）。
    /// 例: label の無い AXRow の子に AXStaticText「Acoustic Guitar」→ `name_of(row)` == "Acoustic Guitar"
    /// Wave: OPEN-INPLACE-1101
    pub fn name_of(&self, e: &Element) -> String {
        if !e.label.trim().is_empty() {
            return e.label.clone();
        }
        self.name_in(&self.by_ref(), e)
    }
    /// `name_of` の本体（親の表 `by` を呼ぶ側が 1 回作って使い回す）
    fn name_in(&self, by: &HashMap<&ElementRef, &Element>, e: &Element) -> String {
        if !e.label.trim().is_empty() {
            return e.label.clone();
        }
        if !row_like(by, e) {
            return String::new();
        }
        // ponytail: 行ごとに全要素を舐める（O(行 × 要素 × 深さ)・窓の全部を読むので 5000 要素の窓で数千の行なら重い）。重ければ親 → 子の表を 1 回作る
        let mut seen: Vec<&str> = Vec::new();
        for x in self.items.iter().filter(|x| x.role == "AXStaticText" && ancestors(by, x).any(|a| a.r == e.r)) {
            let t = x.value.as_deref().filter(|v| !v.trim().is_empty()).unwrap_or(&x.label).trim();
            if !t.is_empty() && !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen.join(" · ")
    }
}

/// OPEN-INPLACE-1101 の 3: 一覧の行・升（AXRow・AXCell・AXOutline の子）。AXPress を持たなくても選べる物（`find_containing` が押せる物に数え、
/// `run_desktop_matching` は AXPress の無い行に `AXSelect` = AXSelected を置く）。app の名前で分けない。
fn row_like(by: &HashMap<&ElementRef, &Element>, e: &Element) -> bool {
    e.role == "AXRow" || e.role == "AXCell" || e.parent.as_ref().and_then(|p| by.get(p)).is_some_and(|p| p.role == "AXOutline")
}

/// G74b・W7: 確認の窓 = role AXSheet か subrole が AXDialog・AXSystemDialog・AXApplicationDialog（`shown_in_dialog` と `in_dialog` が同じこれ）
fn is_dialog(e: &Element) -> bool {
    e.role == "AXSheet" || ["AXDialog", "AXSystemDialog", "AXApplicationDialog"].contains(&e.subrole.as_str())
}

/// G74b: `e` の親から窓までの要素（`Element::parent` を辿る・読みに無い親で止まる・壊れた応答の輪でも要素の数で止まる）。
fn ancestors<'a>(by: &'a HashMap<&'a ElementRef, &'a Element>, e: &'a Element) -> impl Iterator<Item = &'a Element> + 'a {
    let up = move |x: &&'a Element| x.parent.as_ref().and_then(|p| by.get(p).copied());
    std::iter::successors(up(&e), up).take(by.len())
}

/// 鍵の 1 つ。Named は bridge の key 名（`"return"`・`"tab"`・`"escape"`・`"delete"`・`"left"` 等）・Char は 1 文字（`'s'`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Named(String),
    Char(char),
}

/// 修飾 key つきの 1 打。使い方: `Chord { key: Key::Char('s'), cmd: true, shift: false, option: false, control: false }` = ⌘S。
/// Wave: W17-0
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub cmd: bool,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

/// `GuiAction::Key { keys }` の文字列 → `Chord`。形は `[mod+]*key`（mod = `cmd` / `shift` / `option` / `control`・大文字小文字を問わない）、
/// key は 1 文字（`Key::Char`）か bridge の key 名（`return`・`tab`・`escape`・`delete`・`left` 等 → `Key::Named`）。
/// 使い方: `let c: Chord = "cmd+s".parse()?; gui.key(pid, &c)`。
/// 保証: 空・key が無い（`"cmd+"`）・同じ mod が 2 回・知らない mod は `GuiError::Unsupported`（何も送らない）。保証しない: key 名を bridge が知っている事（知らなければ bridge が `bad_args`）。
impl std::str::FromStr for Chord {
    type Err = GuiError;
    fn from_str(keys: &str) -> Result<Chord, GuiError> {
        let bad =
            |why: String| GuiError::Unsupported(format!("Not sending the key “{keys}” ({why})"));
        let lower = keys.trim().to_lowercase();
        let mut parts: Vec<&str> = lower.split('+').map(str::trim).collect();
        let key = parts.pop().unwrap_or_default();
        if key.is_empty() {
            return Err(bad("No key given. The format is cmd+s".into()));
        }
        let mut c = Chord {
            key: match key.chars().collect::<Vec<_>>().as_slice() {
                [ch] => Key::Char(*ch),
                _ => Key::Named(key.to_string()),
            },
            cmd: false,
            shift: false,
            option: false,
            control: false,
        };
        for m in parts {
            let slot = match m {
                "cmd" => &mut c.cmd,
                "shift" => &mut c.shift,
                "option" => &mut c.option,
                "control" => &mut c.control,
                other => {
                    return Err(bad(format!(
                        "Unknown modifier key “{other}”. Use cmd, shift, option or control"
                    )))
                }
            };
            if std::mem::replace(slot, true) {
                return Err(bad(format!("{m} appears twice")));
            }
        }
        Ok(c)
    }
}

/// 1 回の操作の結果。どれも受け入れの「verified」とは別物（受け入れの verified は試験の側の oracle だけ: YC_ACCEPTANCE §2）。
/// 使い方: `let o: Outcome = gui.press(&w, &el).into();`（run の記録はそのまま domain の Outcome で残す）。
/// 保証: `Unsupported` は何も送っていない。`Delivered{sure:false}` は送ったかも分からない（自動の再試行をしない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiResult {
    /// 送った後、同じ adapter が別の AX の読みで期待した値を見た（`set_text` の読み戻しが一致した時だけ）
    Verified { evidence: String },
    /// 送った / 受理された。効いたかは読んでいない。sure = false は送った後に bridge が切れた・`cannot_complete` 等で届いたかも分からない
    Delivered { detail: String, sure: bool },
    /// 送っていない。理由 1 行（許可が無い・AX を持たない app・要素が古い・同名が複数・窓が曖昧・action が無い）
    Unsupported(String),
}

/// domain の Outcome へ。Verified → Verified・Delivered(sure) → Unverified・Delivered(!sure) → EffectUnknown・Unsupported → Rejected。
impl From<GuiResult> for Outcome {
    fn from(r: GuiResult) -> Outcome {
        match r {
            GuiResult::Verified { evidence } => Outcome::Verified {
                evidence: vec![evidence],
            },
            // 「Whether the work itself succeeded isn't verified」= app.rs の履歴が「完了」でなく「受理（結果未検証）」と出す語（github.rs の「業務上の成功は未検証」と同じ役）
            GuiResult::Delivered { detail, sure: true } => Outcome::Unverified {
                reason: format!("{detail}. Whether the work itself succeeded isn't verified"),
            },
            GuiResult::Delivered {
                detail,
                sure: false,
            } => Outcome::EffectUnknown { reason: detail },
            GuiResult::Unsupported(reason) => Outcome::Rejected { reason },
        }
    }
}

// ---- bridge の JSON の契約（KangaeSurfaceBridge.swift に W17-1 / W17-2 で足す 5 つの cmd）----
// 共通: 今の形 `{"id":N,"cmd":"…",…}` → `{"id":N,"ok":true,"result":{…}}` / `{"id":N,"ok":false,"error":{"code","message"}}`。
// 操作の口は result に `front_before` / `front_after`（frontmostPid・既存の `raise` と同じ名前）を入れ、D-g（前面を奪わない）を試験が機械で見る。
// 許可が無ければ `requireTrust()` が `not_trusted` を返し何も送らない。どの cmd も app を前に出さない。

/// bridge の cmd 名（Swift の `case "…"` と一字一句同じ）。
pub const CMD_AX_TREE: &str = "ax_tree";
pub const CMD_AX_PRESS: &str = "ax_press";
pub const CMD_AX_SET_VALUE: &str = "ax_set_value";
/// G79: 欄の中のちょうど 1 か所だけを置き換える（要求は `AxSetValueReq` の replace 付き・result は `AxSetValueReply`）。ax_set_value の key でなく別の cmd:
/// 前の bridge は知らない key を捨てて全文を置き換えるが、知らない cmd なら何も置かずに断る
pub const CMD_AX_REPLACE_TEXT: &str = "ax_replace_text";
pub const CMD_AX_MENU: &str = "ax_menu";
/// ENGINE-AUDIT W3: app の menu bar を読むだけ（題名と 1 階目の項目・押さない・前面を変えない）。要求は `AxMenuTreeReq`・result は `AxMenuTreeReply`。
/// 前の bridge は知らない cmd なので断る（読めない = `desktop.menu.read` は Rejected の 1 行）
pub const CMD_AX_MENU_TREE: &str = "ax_menu_tree";
pub const CMD_KEY_TO_PID: &str = "key_to_pid";
/// ENGINE-AUDIT W8a（spec 262「隠れていたら解いてから読み直す」・r8b = Mirror の Page の GarageBand が隠れていて承認の後に 0 手で止まった）: 隠れた app の窓を見せる
/// （`NSRunningApplication.unhide()` = 窓を前の重なりの順に戻す・activate しない = 前面は取らない・owner 254）。要求は `AppUnhideReq`・result は `AppUnhideReply`。
/// 頼んだ後、隠れが解けるまで 100 ms おきに見て最長 1.5 秒（解けなくても error にしない = `hidden: true`）。起こさない・窓を動かさない・押さない。
/// error: `no_such_process`・`bad_args`。前の bridge は知らない cmd なので断る（= 解けないまま `op` の門へ・bridge は .app に同梱なので混ざらない）。
/// `open -g -b` は使わない: 動いている隠れた app を見せる約束が文書に無い（隠れを解くのは activate の側で、-g はそれを止める向き）
pub const CMD_APP_UNHIDE: &str = "app_unhide";

/// W8a: `app_unhide` の要求。例: `{"id":9,"cmd":"app_unhide","pid":222}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppUnhideReq {
    pub pid: u32,
}
/// W8a: `app_unhide` の result（返す時の隠れ・前面の pid の前と後 = D-g を数える）。例: `{"hidden":false,"front_before":1,"front_after":1}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppUnhideReply {
    pub hidden: bool,
    pub front_before: i64,
    pub front_after: i64,
}

/// ENGINE-AUDIT W3: `ax_menu_tree` の要求（pid の app の menu bar）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxMenuTreeReq {
    pub pid: u32,
}
/// W3: menu bar の 1 つ（題名と、その下の 1 階目の項目の題名・区切りは入れない・submenu は題名だけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxMenuEntry {
    pub title: String,
    #[serde(default)]
    pub items: Vec<String>,
}
/// W3: `ax_menu_tree` の result。`desktop.menu.read` の payload もこの形（`menus`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxMenuTreeReply {
    pub menus: Vec<AxMenuEntry>,
}

/// `ax_tree` の要求（W17-1 で bridge に足した口）。handle = `window_list` / `windows` の `"w<N>"`。
/// actionable_only = 押せる / 値を持つ要素だけ（窓そのものは常に入る）。
/// 例: `{"id":7,"cmd":"ax_tree","handle":"w2","actionable_only":false}`・続き: `{"id":8,"cmd":"ax_tree","handle":"w2","more":true}`
/// 保証: 呼ぶたびにその窓の snapshot を 1 上げ、前の snapshot の ref は `stale_element` になる（窓ごとに最新 1 つだけ持つ）。読むだけで何も押さない・前面を変えない。
/// owner 256: 数では切らない（前の max_elements は外した・送っても bridge は見ない）。1 回の要求は時間で区切り、`complete = false` と `paused` を返す。
/// more = true は同じ snapshot の続き（snapshot を上げない・前の要求の ref は有効なまま・他の欄は見ない）。読みかけが無ければ `bad_args`。
/// G31: 1 回の要求の時間は今の AX の timeout（起動時か `config`）の 6 割（`config` で伸ばせば区切りも伸びる）。pid ごとの初回に app 要素へ AXManualAccessibility を
/// 立てる（Chromium の中身を出させる・Cocoa の app は attributeUnsupported で何も起きない・画面は変えない）。
/// error: `not_trusted`（許可なし）・`unknown_handle`（窓を知らない）・`bad_args`。それ以外の AX の失敗は error にせず complete = false。
/// FX-112: `root` = 窓の中の部分木だけを読む（Discord の 2000 要素を毎回読まない）。bridge は窓から幅優先で role と label（大文字小文字を問わず含む）の
/// 合う最初の要素を探し、`elements` は窓そのもの（[0]）とその要素から下だけ。見つからなければ窓だけ・`root_found = false`。前の bridge は root を知らず窓全体を返す
/// （`root_found` が無い = None）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxTreeReq {
    pub handle: String,
    pub actionable_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<AxRoot>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}
/// FX-112: 読む部分木の根（窓の中の 1 つ・役割と名前で決める = server の一覧・channel の本文・入力欄の囲み）。registry の input の `within`。
/// 新しい型にした理由: `ax_tree` の要求の中の物で、label と role だけを持つ既存の型が無い（`Control`・`Element` は読んだ結果の形）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxRoot {
    pub role: String,
    pub label: String,
}
/// `ax_tree` の result。区切り（時間）・app が答えない は error にせず complete = false と `paused`（owner 256: 続きは `AxTreeReq::more`）。
/// 形: `elements` は幅優先の順。**`elements[0]` は窓そのもの**（ref `"e<snapshot>.0"`・role `AXWindow`・depth 0・parent null）で、
/// それ以外はちょうど 1 つの parent（同じ snapshot の ref）を持つ。visited = 辿った AX の要素の数（返さなかった物を含む）。
/// waking（G31）= Chromium / Electron の app（Discord 等）に中身を AX に出すよう頼んだ直後で、まだ出ていない（窓の枠だけの木で complete = true に
/// 見える）。呼ぶ側は間を置いて読み直し、出ないうちは「無い」と読まない。前の bridge は返さない = false
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxTreeReply {
    pub handle: String,
    pub snapshot: u64,
    pub elements: Vec<Element>,
    pub visited: u32,
    pub complete: bool,
    #[serde(default)]
    pub waking: bool,
    /// FX-112: `root` を頼んだ時に見つかったか（None = 頼んでいない・前の bridge・まだ探している）
    #[serde(default)]
    pub root_found: Option<bool>,
    /// owner 256: complete = false の訳。`PAUSED_TIME`（1 回の要求の時間の区切り・続きを頼めば読み進む）か `"app"`（app が答えない・同じ所から読み直す）。
    /// elements はその要求で新しく読めた物だけ。None = 読み終えた・前の bridge（途中で切った読みに続きは無い）
    #[serde(default)]
    pub paused: Option<String>,
}
/// owner 256: `AxTreeReply::paused` の「時間の区切り」（app は答えている・続きを頼めば読み進む）
pub const PAUSED_TIME: &str = "time";

/// `ax_press` の要求。action 省略 = `"AXPress"`。要素の actions に無い action は送らず `action_unsupported`。
/// G74f: focus = true なら bridge は AXFocused=true を置いてから action を送る（置けなくても送る）。`MacGui` は menu の項目（AXMenuItem）の AXPress にだけ付ける
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxPressReq {
    pub element: ElementRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focus: bool,
    /// ENGINE-AUDIT W5（節 11「押し方が AXPress か行の 2 つしか無い」）: 梯子 = 要素自身が action を持たなければ、近い祖先の AXPress → 入れ物の AXSelectedRows → 行の AXSelected →
    /// AXShowMenu / AXExpand の順に試す（どれで押せたかは `AxPressReply::via`・1 つ効いたら止める・前面は変えない）。false = 今まで通り要素自身だけ（無ければ `action_unsupported`）。
    /// Mirror の `ax_at_point`（LAYOUT）と同じ梯子を bridge の 1 関数に置く（Rust は段を知らない）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ladder: bool,
}
/// `ax_press` の result。focus_rc = focus を頼んだ時の AXFocused の set の AXError（0 = 置けた・頼まなければ無い）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxPressReply {
    pub performed: String,
    pub front_before: i64,
    pub front_after: i64,
    #[serde(default)]
    pub focus_rc: Option<i64>,
    /// W5: 梯子のどの段で押せたか（`ladder` を頼んだ時だけ）: `self`（要素自身）・`ancestor:<role>`（近い祖先の AXPress）・`rows`（入れ物の AXSelectedRows）・
    /// `selected`（行の AXSelected）・`menu`（AXShowMenu）・`expand`（AXExpand）。頼まなければ・古い bridge なら None（= 要素自身）
    #[serde(default)]
    pub via: Option<String>,
}

/// `ax_set_value` の要求。`AXUIElementIsAttributeSettable` が false なら送らず `not_settable`。
/// 置き方（W17-2 戻し）: AXSelectedText と AXSelectedTextRange の両方を置ける要素（NSTextView 等）は全選択 → AXSelectedText の set（app の編集として数わる =
/// TextEdit の書類が編集済みになり File › Save が disk に書く）。range か selected text の set が error（何も変えていない）なら AXValue に落ちる。
/// AXValue の set は TextEdit の書類を編集済みにせず、保存を押しても disk が変わらない（W17-2 戻しの偵察・2026-10-21）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxSetValueReq {
    pub element: ElementRef,
    pub value: String,
    /// G79（`ax_replace_text` だけ）: 欄の全文の中でこの字と完全一致する所がちょうど 1 つの時だけ、そこを value に置き換える（全選択しない）。
    /// 0 か所・2 か所以上は何も置かずに `ax_replace_count`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
}
/// `ax_set_value` の result。read_back = set の後に同じ要素の AXValue を読んだ値（読めなければ null）・matches = read_back == value。
/// via = 置き方（`"selected_text"` | `"value"`・W17-2 戻しより前の bridge は返さない = None）。全選択が効かず後ろに足された時も matches = false で拾う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxSetValueReply {
    pub set: bool,
    pub read_back: Option<String>,
    pub matches: bool,
    #[serde(default)]
    pub via: Option<String>,
    /// G79（`ax_replace_text` だけ）: 置き換える前と後の欄の全文の長さ（UTF-16・切らない）。読み戻しの合否は「欄に value が在り、差 = len(value) − len(replace)」
    #[serde(default)]
    pub length_before: Option<i64>,
    #[serde(default)]
    pub length_after: Option<i64>,
    pub front_before: i64,
    pub front_after: i64,
}

/// `ax_menu` の要求。app の kAXMenuBarAttribute から題名の完全一致で辿る。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxMenuReq {
    pub pid: u32,
    pub path: Vec<String>,
}
/// `ax_menu` の result。`menu_not_found` の message は「どこで止まったか・その階の題名の一覧」を含む。
/// enabled_before = 押す前に読んだ最後の項目の AXEnabled（W17-2 戻し・前の bridge は返さない = None）。背景の app の menu は検証され直さず古い
/// （TextEdit の Save は保存できる時も false と読めた）ので、bridge は false でも押す（`menu_disabled` で止めない）。押した後に効いたかは呼ぶ側が読み戻す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxMenuReply {
    pub pressed: Vec<String>,
    #[serde(default)]
    pub enabled_before: Option<bool>,
    pub front_before: i64,
    pub front_after: i64,
}

/// `key_to_pid` の要求。key + mods（`"cmd"`・`"shift"`・`"option"`・`"control"`）か text のどちらか 1 つ（両方 / どちらも無い = `bad_args`）。
/// 実装は `bridge/macos-mirror/KangaeMirror.swift` の `CGEvent(keyboardEventSource:…)` → `postToPid` を持ってくる。mouse は入れない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyToPidReq {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
impl KeyToPidReq {
    /// Chord → 要求（Char は 1 文字の key 名・Named はそのまま・mods は cmd / shift / option / control の順）。
    pub fn chord(pid: u32, c: &Chord) -> KeyToPidReq {
        let key = match &c.key {
            Key::Named(n) => n.clone(),
            Key::Char(ch) => ch.to_string(),
        };
        let mods = [
            (c.cmd, "cmd"),
            (c.shift, "shift"),
            (c.option, "option"),
            (c.control, "control"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, m)| m.to_string())
        .collect();
        KeyToPidReq {
            pid,
            key: Some(key),
            mods,
            text: None,
        }
    }
}
/// `key_to_pid` の result。sent = keyDown / keyUp の数・via = `"pid"`。届いたかは返さない（Rust は常に Delivered）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyToPidReply {
    pub sent: u32,
    pub via: String,
    pub front_before: i64,
    pub front_after: i64,
}

/// 5 つの cmd が返す error の code（`BridgeError::Failed{code}` の値）。どれも cannot_complete 以外は「送っていない」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxErrorCode {
    /// Accessibility の許可が無い（何も送らない）
    NotTrusted,
    /// 窓の handle を知らない（bridge が起こし直された・窓が閉じた）
    UnknownHandle,
    /// その窓に新しい snapshot がある（読み直して ref を取り直す）
    StaleElement,
    /// kAXErrorInvalidUIElement（要素が消えた）
    InvalidElement,
    /// 要素の actions に無い・kAXErrorActionUnsupported
    ActionUnsupported,
    /// kAXErrorCannotComplete。**送った後かもしれない** → `Delivered{sure:false}`。`ax_menu` は最後の AXPress の時だけ（G31: 押す前の menu の読みが
    /// 答えない時は `ax_cannot_complete` = 送っていない → `gui_result` が「app が答えない・何も送っていない」の 1 行）
    CannotComplete,
    /// AXValue を set できない要素
    NotSettable,
    /// menu の題名が見つからない
    MenuNotFound,
    /// pid の process が無い
    NoSuchProcess,
    /// 要求の形が違う
    BadArgs,
}
impl AxErrorCode {
    /// `"stale_element"` → `Some(StaleElement)`。知らない code は None（呼ぶ側は Unsupported にする）。
    pub fn from_code(code: &str) -> Option<AxErrorCode> {
        serde_json::from_value(Value::String(code.to_string())).ok()
    }
}

/// bridge の失敗を 1 回の操作の結果へ。`Failed{cannot_complete}`・送った後の `Timeout` / `Gone` は `Delivered{sure:false}`、
/// それ以外（送る前の失敗・明示の拒否）は `Unsupported(1 行)`。sent = 要求を stdin へ書き終えたか。
/// 使い方: `Err(e) => gui_result(&e, true)`。保証しない: sent = true の Timeout で app 側に効いていない事。
pub fn gui_result(e: &BridgeError, sent: bool) -> GuiResult {
    let unknown = |why: String| {
        GuiResult::Delivered { detail: format!("Sent, but the result was lost ({why}). Don't run it twice, and read the screen again"), sure: false }
    };
    match e {
        BridgeError::Failed { code, message }
            if AxErrorCode::from_code(code) == Some(AxErrorCode::CannotComplete) =>
        {
            unknown(message.clone())
        }
        // G31: 押す前の読みで app が答えなかった（Zoom の menu bar・混んだ機械）。何も送っていないので、待ってもう一度の 1 行
        BridgeError::Failed { code, .. } if code == "ax_cannot_complete" => GuiResult::Unsupported(NOT_ANSWERING_LINE.into()),
        BridgeError::Timeout { .. } | BridgeError::Gone(_) if sent => unknown(e.to_string()),
        _ => GuiResult::Unsupported(format!("Not sent: {e}")),
    }
}

/// input の名指しの書類（bundle_id・document・document_file）。bundle か書類が無ければ None。
fn pinned_of(input: &Value) -> Option<(String, String, Option<String>)> {
    let s = |k: &str| input[k].as_str().filter(|v| !v.trim().is_empty()).map(String::from);
    Some((s("bundle_id")?, s("document")?, s("document_file")))
}

/// bridge の `window_list` の応答の、CG の窓番号が付いた窓（`MacGui::windows_where` と同じ WindowRef の作り方）。
fn window_refs(list: &Value, app_name: &str, bundle: &str) -> Vec<WindowRef> {
    list["windows"].as_array().into_iter().flatten().filter_map(|raw| {
        let x = WindowInfo::from_json(raw)?;
        Some(WindowRef { pid: x.pid, window_id: x.cg_id?, app_name: app_name.to_string(), title: x.title.clone(), bundle_id: bundle.to_string(), document: raw["document"].as_str().unwrap_or("").to_owned() })
    }).collect()
}

/// WindowRef（CG の窓番号）→ bridge の窓 handle。`window_list` の窓のうち pid と cg_id が一致する物がちょうど 1 つの時だけ Ok。
/// 0 件 = 「窓が見つかりません」・cg_id が None（cg_match = none / ambiguous）= 「窓を 1 つに決められません」の Unsupported（推測で選ばない）。
/// 使い方: `let h = handle_for(&parse_windows(&r), &w)?;`
pub fn handle_for(windows: &[WindowInfo], w: &WindowRef) -> Result<String, GuiResult> {
    let same_pid: Vec<&WindowInfo> = windows.iter().filter(|x| x.pid == w.pid).collect();
    let hits: Vec<&WindowInfo> = same_pid
        .iter()
        .copied()
        .filter(|x| x.cg_id == Some(w.window_id))
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.handle.clone()),
        [] if same_pid.iter().any(|x| x.cg_id.is_none()) => Err(GuiResult::Unsupported(format!(
            "Can't settle on one window of {} (its window numbers don't line up). Not pressing",
            w.app_name
        ))),
        [] => Err(GuiResult::Unsupported(format!("{}'s window wasn't found. Reopen the window and add the step again", w.app_name))),
        many => Err(GuiResult::Unsupported(format!("{} windows of {} match and Shibubu can't tell which. Not pressing", many.len(), w.app_name))),
    }
}

/// AXDocument の無い窓（REAPER・Zoom）の宛先を、題名の中身を使わずに窓の役割で 1 つに決める（G31b・`document` の無い保存した手順の決め直し）。
/// `windows` は 1 つの app（bundle）の `window_list` の窓（bundle で絞るのは呼ぶ側）。CG の窓番号の付いた・最小化していない窓のうち:
/// 1 つならそれ（Zoom の唯一の窓は AXDialog）→ 2 つ以上なら普通の窓（AXStandardWindow）で外から動かせる物に絞って 1 つ（REAPER の起動直後の
/// 評価版の「About」は main だが動かせない・本体は動かせて main でない）→ まだ複数（絞って 0 個なら絞る前）なら main の 1 つ。
/// Err = 理由の 1 行（窓が 0 個・同じ役割の窓が 2 つ以上 = 決められない。先頭の窓に繋がない）。新しい型なし（`WindowRef` に役割の欄を足すと
/// UI の列の struct の literal が割れるので、役割は `window_list` の `WindowInfo` から読む）。
/// 使い方: `let w = pick_by_role(&infos, "REAPER")?; let (pid, window_id) = (w.pid, w.cg_id);`
/// 保証しない: 選んだ窓が人の意図した窓である事（保存した窓が今も在る間は呼ぶ側がそちらを使う・`MacGui::retargeted`）。
/// CG の窓番号の付かない窓（`cg_match` が none / ambiguous）が 1 つでも在れば決めない（その窓が本当の宛先かもしれない・1 つしか読めない窓を選ばない）。
pub fn pick_by_role(windows: &[WindowInfo], app_name: &str) -> Result<WindowInfo, String> {
    let open: Vec<&WindowInfo> = windows.iter().filter(|w| !w.minimized).collect();
    let unmatched = open.iter().filter(|w| w.cg_id.is_none()).count();
    if unmatched > 0 {
        let what = if unmatched == 1 { "window" } else { "windows" };
        return Err(format!("{unmatched} {app_name} {what} can't be matched to the screen yet, so Shibubu can't tell which to use. Wait a moment, then ask again"));
    }
    let work: Vec<&WindowInfo> = open.iter().copied().filter(|w| w.standard() && w.movable).collect();
    let pool = if work.is_empty() { &open } else { &work };
    let main: Vec<&WindowInfo> = pool.iter().copied().filter(|w| w.main).collect();
    match (open.as_slice(), work.as_slice(), main.as_slice()) {
        ([one], _, _) | (_, [one], _) | (_, _, [one]) => Ok((**one).clone()),
        ([], ..) if windows.iter().any(|w| w.minimized) => Err(format!("{app_name}'s window is minimised. Restore it from the Dock, then ask again")),
        ([], ..) => Err(format!("No window of {app_name} is open. Open it, then ask again")),
        (many, ..) => Err(format!("{} windows of {app_name} are open and Shibubu can't tell which. Bring the one you want to the front, then ask again", many.len())),
    }
}

// ---- 本物の adapter（bridge を包む）----

/// registry に載せる action（W17-2 より前の desktop の adapter と同じ id・入力。旧 Desktop の binding がこちらを呼ぶ）。
/// desktop.ax.menu / desktop.key は W17-2 で足す口。どれも approval つきの external_write（読む 3 つを除く）。
/// ENGINE-AUDIT W7: 承認の要らない押す手の action の id（`capability_engine::REVEAL_ACTION` と同じ字）
pub const REVEAL: &str = "desktop.ax.reveal";

const SPECS: &[Spec] = &[
    // 入力の main = true（bundle_id と）: その app の窓を `pick_by_role` で主窓 1 つに絞る（M2 G-i・Engine の generate が model に渡す窓 = REAPER の評価版の
    // About を渡さない）。窓が在って決められない（同じ役割の窓が 2 つ以上・画面にまだ出ない・最小化）時は Rejected の 1 行・窓が 0 個なら空の一覧。
    // G73: 入力の document（書類の file 名）の題名の窓が在れば役割より先に書類で 1 つ（`gui::retarget` と同じ題名の一致・2 個以上は Rejected の 1 行）。
    // G75（名指しの対象を代用しない）: document が在り、その bundle の窓が 1 つ以上在って名指しの書類が無い時は Rejected（別の書類で代用しない）。
    // その bundle の窓が 0 個（読めた・答えなかったのでも、まだ置いている途中でもない）は空の一覧。bundle id を app 名にした Rejected にはしない。
    // 役割で選ぶのは document が無い時（REAPER・Zoom・Discord の主窓）だけ
    Spec {
        id: "desktop.windows.list",
        effect: Effect::LocalRead,
        approval: false,
        scopes: &["desktop.read"],
        input: r#"{"type":"object","properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"main":{"type":"boolean"},"document":{"type":"string"},"document_file":{"type":"string"}}}"#,
        verifier: "read_back",
    },
    // G70: 起動中の app（bridge の `apps`・AX を読まないので答えない app を待たない）を入力の pid / bundle_id で絞る。payload は `[surface::AppIdentity]`
    // （pid・name・bundle_id = `MacGui::op` の門の `app_of` と同じ読み）。読むのは Page の承認の同意の宛先（`engine_page::CoreEngine::grant`）
    Spec {
        id: "desktop.apps.list",
        effect: Effect::LocalRead,
        approval: false,
        scopes: &["desktop.read"],
        input: r#"{"type":"object","properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"}}}"#,
        verifier: "read_back",
    },
    // 出力は `gui::WindowState`（R-G2）。入力の app_name は任意: 窓を決められない時の 1 行に app 名を出すためだけ（照合には使わない）。
    // G74b: role・subrole・contains（label か値に含む）の在る物だけで controls を絞る（消した行が無い・行が 1 つ減ったを読み戻す）。
    // owner 256: 読みは窓の全部（数でも時間でも切らない・読み終えられなければ Rejected = 数えられない・「無い」と言わない）
    Spec {
        id: "desktop.window.state",
        effect: Effect::LocalRead,
        approval: false,
        scopes: &["desktop.read"],
        input: r#"{"type":"object","required":["pid","window_id"],"properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"},"window_id":{"type":"integer"},"app_name":{"type":"string"},"role":{"type":"string"},"subrole":{"type":"string"},"contains":{"type":"string"},"within":{"type":"object","required":["role","label"],"properties":{"role":{"type":"string"},"label":{"type":"string"}}}}}"#,
        verifier: "read_back",
    },
    // 入力の match = "contains"（M2 G-c）: label を利用者の言葉として、窓の中の要素（窓そのものを除く）の label に大文字小文字を問わず含む物を探す（role が空なら
    // AXPress を持つ要素の中から）。ちょうど 1 つの時だけ押し、0 個・2 個以上は Rejected の 1 行（何も押さない）。既定 "exact" = label と role の完全一致（今まで通り）。
    // click は窓の要素を全部読む（owner 256・前は 2000 まで）。窓の中身は model に渡さない（channel 名を CLI に出さない）。
    // G74b: press = 送る AX の action（既定 AXPress・AXShowMenu = 要素の context menu を開く）。only_if = 押す要素を囲む dialog がこの字を全部見せている時だけ
    // 押す（同じ読みの中で・`Elements::shown_in_dialog`。消す・送るの確認の dialog が別の物を見せていれば何も押さずに Rejected）
    // G74c: role = "AXMenuItem" = 前の手の身振り（中身への AXShowMenu）で出る menu の項目。menu（`Elements::open_menu`）が描かれるまで MENU_GAP おきに
    // 待ち始めから MENU_WAIT（3 秒）まで読み直し、AXMenuItem の中だけを探す。menu が出ない → 何も押さずに Rejected・項目が無い → 何も押さず、menu に AXCancel を 1 回送って
    // 読み直し、閉じなければ「開いたまま・Esc で閉じる」を 1 行に足す（Discord の menu は AXCancel で閉じない・実測）
    // ENGINE-AUDIT W5 の契約: 押す名前が今の木に無い = `Failed { TargetNotFound }`・1 つに絞れない = `Failed { TargetAmbiguous }`（どちらも何も押していない・Core の `execute` が探す段に入る・
    // 文は Core が `Searched` から作る）。人の lease・同意・app の入れ替わり・dialog や menu が出ない・欄に人の字 = 今まで通り `Rejected { reason }`（人の言葉の 1 行）
    Spec {
        id: "desktop.ax.click",
        effect: Effect::ExternalWrite,
        approval: true,
        scopes: &["desktop.input"],
        input: r#"{"type":"object","required":["pid","window_id","app_name","label","role"],"properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"},"window_id":{"type":"integer"},"app_name":{"type":"string"},"label":{"type":"string"},"role":{"type":"string"},"match":{"type":"string","enum":["exact","contains"]},"press":{"type":"string","enum":["AXPress","AXShowMenu"]},"only_if":{"type":"array","items":{"type":"string"}},"ladder":{"type":"boolean"},"within":{"type":"object","required":["role","label"],"properties":{"role":{"type":"string"},"label":{"type":"string"}}}}}"#,
        verifier: "state_compare",
    },
    // ENGINE-AUDIT W7（spec 261「戻せる手は聞かない」・operator 11-03 の決め 1〜3）: 承認の要らない押す手（開く・選ぶ・切り替える）。input と押し方は click と同じ
    // （同じ `run_desktop_matching`）・Effect は ExternalWrite のまま（read-only の automation では走らない）。どちらの手にするかは Core が `gui::asks` で決める
    // （`recipe::reveal_or_click`）。送る直前に解決した要素の名前と、確認の窓（sheet・dialog）の中かを `gui::asks` にもう 1 回照らし、当たれば何も押さずに
    // `Failed { ApprovalDenied }`（Core の `run_steps` が `WorkflowError::Asks` = その手を click に書き換えて承認の card）
    Spec {
        id: REVEAL,
        effect: Effect::ExternalWrite,
        approval: false,
        scopes: &["desktop.input"],
        input: r#"{"type":"object","required":["pid","window_id","app_name","label","role"],"properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"},"window_id":{"type":"integer"},"app_name":{"type":"string"},"label":{"type":"string"},"role":{"type":"string"},"match":{"type":"string","enum":["exact","contains"]},"press":{"type":"string","enum":["AXPress","AXShowMenu"]},"only_if":{"type":"array","items":{"type":"string"}},"ladder":{"type":"boolean"},"within":{"type":"object","required":["role","label"],"properties":{"role":{"type":"string"},"label":{"type":"string"}}}}}"#,
        verifier: "state_compare",
    },
    // G79: replace（任意）= 欄の今の字の中で完全一致する所がちょうど 1 つの時だけ、そこだけを text に置き換える（0 か所・2 か所以上は何も置かずに Rejected
    // の 1 行に数）。replace が無ければ欄の全文を text に置き換える（書類の本文なら他の行は全部消える）。読み戻しは「欄に text が在り、長さの差 = len(text) − len(replace)」
    Spec {
        id: "desktop.ax.type_text",
        effect: Effect::ExternalWrite,
        approval: true,
        scopes: &["desktop.input"],
        input: r#"{"type":"object","required":["pid","window_id","app_name","label","role","text"],"properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"},"window_id":{"type":"integer"},"app_name":{"type":"string"},"label":{"type":"string"},"role":{"type":"string"},"text":{"type":"string"},"replace":{"type":"string"},"within":{"type":"object","required":["role","label"],"properties":{"role":{"type":"string"},"label":{"type":"string"}}}}}"#,
        verifier: "state_compare",
    },
    // G75 Identity: menu と key は app の主窓に効くので、input に bundle_id と document が在れば、送る直前にその書類の窓が app の主窓（AXMain）の時だけ送る
    // （書類の窓が後ろなら何も送らずに Rejected の 1 行・前面は動かさない）。document が無ければ今まで通り（G31 の pid の決め直し）
    Spec {
        id: "desktop.ax.menu",
        effect: Effect::ExternalWrite,
        approval: true,
        scopes: &["desktop.input"],
        input: r#"{"type":"object","required":["pid","app_name","path"],"properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"},"app_name":{"type":"string"},"path":{"type":"array","items":{"type":"string"}}}}"#,
        verifier: "state_compare",
    },
    // ENGINE-AUDIT W3（節 3「menu bar が見えない」・254「menu bar の AX から隠れた枠を開く手を探す」）: app の menu bar を読むだけ（題名と 1 階目の項目）。
    // 押さない・前面を変えない・承認も AI の同意も要らない。payload = `AxMenuTreeReply`（menus: [{title, items}]）。pid か bundle_id で app を決める
    Spec {
        id: "desktop.menu.read",
        effect: Effect::LocalRead,
        approval: false,
        scopes: &["desktop.read"],
        input: r#"{"type":"object","properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"app_name":{"type":"string"}}}"#,
        verifier: "read_back",
    },
    // ENGINE-AUDIT W4（節 4・211 の決まり 2・252「app の file を先に見る」）: 宛先の app の bundle を読むだけで分かる口の棚（`gui::AppSurfaces` = 同梱の data・.strings・辞書・
    // URL scheme・書類の型・shortcuts）。app を起こさない・押さない・承認も同意も要らない。bundle@版で `app-surfaces/` に溜め、2 回目は読み直さない。bundle_id か app_name で app を決める
    Spec {
        id: "desktop.app.surfaces",
        effect: Effect::LocalRead,
        approval: false,
        scopes: &["desktop.read"],
        input: r#"{"type":"object","properties":{"bundle_id":{"type":"string"},"app_name":{"type":"string"}}}"#,
        verifier: "read_back",
    },
    // CHAT-013: 名指しの書類が閉じている時に Engine が出す「開く」手（承認つき）。document_file（`file://` + 絶対 path・Project の repo の書類）を bundle_id の
    // app で開き（`open -b`・前面は取らない頼み = -g）、その書類の窓が出るまで読み直す（`OPEN_WAIT`）。在る書類しか開かない・file を作らない・書き換えない
    Spec {
        id: OPEN_DOCUMENT,
        effect: Effect::ExternalWrite,
        approval: true,
        scopes: &["desktop.input"],
        input: r#"{"type":"object","required":["bundle_id","app_name","document","document_file"],"properties":{"bundle_id":{"type":"string"},"app_name":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"}}}"#,
        verifier: "read_back",
    },
    Spec {
        id: "desktop.key",
        effect: Effect::ExternalWrite,
        approval: true,
        scopes: &["desktop.input"],
        input: r#"{"type":"object","required":["pid","app_name","key"],"properties":{"pid":{"type":"integer"},"bundle_id":{"type":"string"},"document":{"type":"string"},"document_file":{"type":"string"},"app_name":{"type":"string"},"key":{"type":"string"},"mods":{"type":"array","items":{"type":"string"}}}}"#,
        verifier: "state_compare",
    },
];

/// FX-115: 2 つの窓の題名が同じ場所か。書類の窓が後ろに足す状態（「README.md — 編集済み」・「— Edited」）は場所でないので、最後の ` — ` から後ろが
/// 状態の語（3 語まで・`-` `|` `#` `/` `·` を持たない）の時だけ比べない。server の名前の「 — 」（`#general | Shibubu — Prod - Discord`）は場所のまま比べる（反証 G-A）。
fn same_place(a: &str, b: &str) -> bool {
    let base = |t: &str| match t.rsplit_once(" — ") {
        Some((head, tail)) if tail.split_whitespace().count() <= 3 && !tail.contains(['-', '|', '#', '/', '·']) => head.trim().to_string(),
        _ => t.trim().to_string(),
    };
    base(a) == base(b)
}

/// FX-115: 送る直前の宛先の確かめ。`put` = 字を置いた時に覚えた題名（None = この窓に字を置いていない・Some(None) = 置く前に題名を読めなかった）、
/// `now` = 今の題名を読む。読めない・違う時は Err（送らない）。置く前に読めなかった時も送らない（反証 G-A の fail-open）
fn same_place_now(put: Option<Option<String>>, now: impl FnOnce() -> Option<String>) -> Result<(), String> {
    match put {
        None => Ok(()),
        Some(None) => Err("the window's title couldn't be read when the text was put, so where it would go couldn't be checked".to_string()),
        Some(Some(before)) => match now() {
            Some(now) if same_place(&now, &before) => Ok(()),
            Some(_) => Err("the window now shows another place than where the text was put (its title changed)".to_string()),
            None => Err("the window's title couldn't be read again, so where it would go couldn't be checked".to_string()),
        },
    }
}

/// CHAT-013: 名指しの書類を開く registry の action の id（Engine の `open_named` が承認の手に出す）。
pub const OPEN_DOCUMENT: &str = "desktop.document.open";

/// ENGINE-AUDIT W4: 口の棚の溜め（`<data dir>/app-surfaces/<bundle>@<版>.json`・版が替われば新しい file・2 回目は読み直さない）
const SURFACES_DIR: &str = "app-surfaces";
/// W4: app を探す・plist と辞書を読む上限（Spotlight が答えない時は固定の場所だけ）
const SURFACES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// W4: `shortcuts list` の上限（無い・遅い時は空）
const SHORTCUTS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// W4: 溜めの file の名前に使える字だけ（bundle id・版）
fn file_safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect()
}

/// W4: 溜めた棚（`app-surfaces/*.json`）から query（bundle id か app の名前）の物を引く。溜めた path の Info.plist の版が同じ時だけ（版が替わった・app が消えた = 読み直す）。
/// bundle id なら file の名前（`<bundle>@`）で絞る = 他の app の大きな file を開かない（名前の時は全部開いて名前で照らす・溜まるのは使った app の数だけ）
fn kept_surfaces(shelf: &std::path::Path, query: &str) -> Option<super::super::gui::AppSurfaces> {
    let prefix = format!("{}@", file_safe(query));
    let by_bundle = super::super::intel::is_bundle_id(query);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(shelf).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    files
        .into_iter()
        .filter(|p| !by_bundle || p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix)))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|t| serde_json::from_str::<super::super::gui::AppSurfaces>(&t).ok())
        .filter(|s| s.bundle_id == query || s.name.eq_ignore_ascii_case(query))
        .find(|s| {
            let v = super::super::intel::read_plist(&std::path::Path::new(&s.path).join("Contents/Info.plist"), SURFACES_TIMEOUT);
            v.as_ref().and_then(|v| v["CFBundleShortVersionString"].as_str()).unwrap_or_default() == s.version
        })
}

/// ENGINE-AUDIT W4（節 4・owner 211 の決まり 2・252「app の file を先に見る」）: 宛先の app の bundle を**読むだけ**で分かる口の棚（`gui::AppSurfaces`）。
/// `intel::investigate` で app を決め（Spotlight と固定の場所・起こさない）、Info.plist の URL scheme と書類の型・辞書の suite・Contents/Resources の同梱の data
/// （拡張子ごとに全部の名前 = Resources からの相対の path の拡張子無し・`.lproj` の下は *.strings の表の名前だけ）・`shortcuts list`。app の名前の枝は無い（どの app も同じ読み）。
/// 保証する: 何も起こさない・押さない・書かない（溜めの file だけ書く）。保証しない: 追加 download の data（bundle の外）・速さ（初回は Resources を全部辿る・2 回目は溜めから）。
fn app_surfaces(bundle: Option<&str>, name: Option<&str>) -> Result<super::super::gui::AppSurfaces, String> {
    use super::super::gui::{AppSurfaces, ResourceGroup};
    let query = bundle.filter(|b| !b.trim().is_empty()).or(name.filter(|n| !n.trim().is_empty())).ok_or("Name the app (its bundle id or name) to read what it has")?;
    // 溜めが先（Spotlight・lsappinfo・辞書・bridge の window_list を run ごとに起こさない）。版が替わっていれば下で読み直す
    let shelf = super::super::surface::data_dir().join(SURFACES_DIR);
    if let Some(kept) = kept_surfaces(&shelf, query) {
        return Ok(kept);
    }
    let intel = super::super::intel::investigate(query, SURFACES_TIMEOUT)?;
    let Some(app) = intel.app.found().cloned() else { return Err(format!("{query} wasn't found on this Mac")) };
    let bundle_id = app.bundle_id.clone().unwrap_or_else(|| query.to_owned());
    let version = app.version.clone().unwrap_or_default();
    let file = shelf.join(format!("{}@{}.json", file_safe(&bundle_id), file_safe(if version.is_empty() { "unversioned" } else { &version })));
    let root = std::path::Path::new(&app.path);
    let plist = super::super::intel::read_plist(&root.join("Contents/Info.plist"), SURFACES_TIMEOUT).unwrap_or(Value::Null);
    let listed = |v: &Value, keys: &[&str]| -> Vec<String> {
        let mut out: Vec<String> = v
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|t| keys.iter().flat_map(|k| t[*k].as_array().into_iter().flatten().filter_map(Value::as_str).map(|s| s.to_lowercase()).collect::<Vec<_>>()).collect::<Vec<_>>())
            .collect();
        out.sort();
        out.dedup();
        out
    };
    let url_schemes = listed(&plist["CFBundleURLTypes"], &["CFBundleURLSchemes"]);
    // 書類の型 = 拡張子（CFBundleTypeExtensions）か UTI（LSItemContentTypes・今の app はこちらが多い）
    let document_types = listed(&plist["CFBundleDocumentTypes"], &["CFBundleTypeExtensions", "LSItemContentTypes"]);
    let dictionary: Vec<String> = intel.dictionary.found().map(|d| d.suites.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
    let resources_dir = root.join("Contents/Resources");
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    let mut strings: Vec<String> = Vec::new();
    let mut stack = vec![resources_dir.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let file_name = e.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                if file_name.ends_with(".lproj") {
                    for f in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                        let n = f.file_name().to_string_lossy().into_owned();
                        if let Some(stem) = n.strip_suffix(".strings") {
                            if !strings.iter().any(|s| s == stem) {
                                strings.push(stem.to_owned());
                            }
                        }
                    }
                } else {
                    stack.push(p);
                }
                continue;
            }
            let Some(ext) = p.extension().and_then(|x| x.to_str()).map(str::to_lowercase) else { continue };
            if ext == "strings" {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if !strings.iter().any(|s| s == stem) {
                        strings.push(stem.to_owned());
                    }
                }
                continue;
            }
            let rel = p.strip_prefix(&resources_dir).unwrap_or(&p).with_extension("").to_string_lossy().into_owned();
            groups.entry(ext).or_default().push(rel);
        }
    }
    strings.sort();
    let resources: Vec<ResourceGroup> = groups
        .into_iter()
        .map(|(ext, mut names)| {
            names.sort();
            ResourceGroup { count: names.len(), ext, names }
        })
        .collect();
    let shortcuts: Vec<String> = match super::super::intel::run_deadline(std::process::Command::new("/usr/bin/shortcuts").arg("list"), SHORTCUTS_TIMEOUT) {
        Ok((status, out, _)) if status.success() => out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    let s = AppSurfaces { bundle_id, name: app.name.clone(), version, path: app.path.clone(), url_schemes, document_types, dictionary, resources, strings, shortcuts, read_at: super::super::runtime::now_rfc3339() };
    if std::fs::create_dir_all(&shelf).is_ok() {
        if let Ok(t) = serde_json::to_string(&s) {
            let _ = std::fs::write(&file, t);
        }
    }
    Ok(s)
}
/// CHAT-013: 開いた書類の窓が出るまで読み直す上限（TextEdit の起動から窓まで・混んだ機械で数秒）
const OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// 本物の GUI 操作（`surface::Bridge` を包む）。W2c の口（`observe` / `act` / `verify`）と registry の `Adapter` を持つ。
/// 使い方: `let mut g = MacGui::new(); g.check();`（`KANGAE_SURFACE_BRIDGE` で偽の bridge `tests/fake-surface-bridge.py` に向けられる）。
/// 保証: bridge が無い / 版が違う / 許可が無い時は何も送らず `Rejected`・`Unsupported` の 1 行（D-e）。cua-driver を起こさない（D-a）。
/// Wave: W17-0, W17-1, W17-2
pub struct MacGui {
    /// None = `surface::find_bridge`（`KANGAE_SURFACE_BRIDGE` → 実行 file の隣…）。Some = `at` で決めた場所（test が env を触らない）
    bridge_path: Option<PathBuf>,
    bridge: Option<Bridge>,
    sync: SourceSync,
    /// 読んだ要素の ref → (role, label)。`verify` が act の前の ref を新しい読みの要素に引き直す（ref は読むたびに変わる）
    seen: HashMap<String, (String, String)>,
    /// FX-115: 字を置いた窓 (pid, window_id) → 置いた時の窓の題名（Discord は題名が channel と server）。送る直前の読み直し（`recheck`）で題名が
    /// 変わっていれば送らない（宛先が替わった）。None = 置く前に題名を読めなかった = 宛先を確かめられないので送らない（反証 G-A の fail-open）。置くたびに上書き・`cleanup` で空に
    put_titles: HashMap<(u32, i64), Option<String>>,
    /// FX-112: この invoke の読みの部分木（input の `within`・invoke の終わりで None）。`read` が bridge の ax_tree の root に渡す
    within: Option<AxRoot>,
    /// 名指しの書類（bundle・書類の題名・repo の file）。この invoke の読みで保存した窓番号の窓が一覧に無い時、同じ一覧の中で `gui::retarget` と同じ一致で決め直す（invoke の終わりで None）
    pinned: Option<(String, String, Option<String>)>,
    /// FX-114: この起動で書く手を送った（かもしれない）pid。送った後に app が隠れた（⌘H）ら、その窓の読みで済みを言わない（隠れた app への入力は後で反映されうる）
    sent_to: std::collections::HashSet<u32>,
    /// 最後の `check` で Ready にならなかった理由の 1 行（registry が gate の定型文の代わりに出す・operator 2026-10-23 (B)）。None = 確かめていない / Ready
    not_ready: Option<&'static str>,
    /// pid → bundle id（`app_of` で読んだ値。act の記録の宛先に書く）
    bundles: HashMap<u32, String>,
    /// act の記録の file（None = `ACT_LOG_ENV` → data dir の `ACT_LOG_FILE`・`act_log_path`）
    act_log: Option<PathBuf>,
    /// AI に任せる app の同意の file（None = `surface::act_consent_path`）。送るたびに読み直す（起動の後に押した同意も効く）
    consent_file: Option<PathBuf>,
    /// 人が前面の間、書く手を待たせる上限（`new` = `HUMAN_WAIT_DEFAULT`・`at` = 0 = 偽の bridge の試験は待たない・試験が待ちを見る時は `with_human_wait`）
    human_wait: std::time::Duration,
    /// 答えない app を読み直して待つ間の 1 行の受け口（`Adapter::set_progress`・Engine の run ごと）。None = 誰も見ていない
    progress: Option<Progress>,
    /// ENGINE-ADJUST-1102 ①（owner 256・人の Stop）: 人の Stop の印（`Adapter::set_stop`・Engine の run ごと）。立っていれば、窓の木の続き（bridge の more）を頼まず・
    /// 答えない app を待たず・人の lease を待たず・書く手を送らない（読みかけは使わない）。None = 印を持たない呼び手（旧 Desktop の panel・試験）
    stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// 許可が無い時の 1 行（`require_trust` と `not_ready_line` が同じ文を出す）。
const UNTRUSTED_LINE: &str = "Accessibility permission is missing. Allow Shibubu in System Settings › Privacy & Security › Accessibility";
/// app 名の無い旧 binding を押さない時の 1 行（次の一手つき・D-e）。
const OLD_BINDING_LINE: &str = "This action was saved in an old format. Press Find windows and Read controls again, then add the step again";
/// bridge を起こせない時の 1 行（`sdk::legacy` の NotReady と同じ文）。
const NO_BRIDGE_LINE: &str = "Screen control didn't start. Reopen Shibubu";

/// 操作の口の失敗（`op`）。
enum OpError {
    /// 送る前に断った（人が lease を持つ・app ごとの AI の同意が無い・宛先の app が分からない・許可が無い）。何も送っていない
    Refused(GuiError),
    /// bridge の失敗。sent = 要求を bridge に渡した後か（bridge を起こせなかった・前面を読めなかった時は false）
    Bridge { e: BridgeError, sent: bool },
}
impl OpError {
    /// 操作の結果へ: Refused = 送っていない Unsupported（→ Rejected・Denied は理由の 1 行のまま）・Bridge = `gui_result`
    fn result(&self) -> GuiResult {
        match self {
            Self::Refused(GuiError::Denied(why)) => GuiResult::Unsupported(why.clone()),
            Self::Refused(e) => GuiResult::Unsupported(e.to_string()),
            Self::Bridge { e, sent } => gui_result(e, *sent),
        }
    }
}
/// 答えなかった app を読み直す時の AX の上限（秒）。実測の 14.3 秒（W19-3b・load 166 の裏の TextEdit）より上・bridge の上限 120 より下
const PATIENT_AX_TIMEOUT: f64 = 30.0;
/// 人が lease を持つ間の 1 行（registry の道は Rejected の理由・`MacGui::act` は `GuiError::Denied`）。
/// RQ6（RQ-024・P49「人間が操作中の入力欄や既存の下書きは、Agentによる変更から保護する必要があります」）: 書類の窓に Shibubu が字を置いた後の本文（窓ごと）。
/// 窓の題が「編集済み」で、今の本文がここに残した本文と違う窓 = 人の未保存の変更が在る（人が続けて打てば本文が違うので断る）。
// ponytail: Vec の線形探索（窓は数個）。HashSet は const の new が無く static にできない
static AI_WROTE: std::sync::Mutex<Vec<((u32, i64), String)>> = std::sync::Mutex::new(Vec::new());

/// macOS の書類の app が未保存の変更を題の終わりに足す語（言語ごと）。
// ponytail: 表にある言語だけ（Mac の表示言語は他にもある）。表に無い言語の Mac では未保存を見逃す = 窓の AX の edited の印を bridge が出せるようになったら、題の語でなくそれを読む
const EDITED_MARKS: [&str; 20] = [
    "Edited", "編集済み", "Bearbeitet", "Modifié", "Modifie", "Editado", "Modificato", "Bewerkt", "已编辑", "已編輯", "편집됨", "Изменено", "Изменён", "Redigerad", "Redigeret", "Muokattu", "Edytowano", "Düzenlendi", "Upravené", "Szerkesztve",
];

/// 書類の窓の題（窓そのもの = depth 0 の label）が `… — Edited` など（`EDITED_MARKS`）で終わるか（書類の名前の中の語は数えない）。
fn title_says_edited(els: &Elements) -> bool {
    let Some(win) = els.items.iter().find(|e| e.depth == 0) else { return false };
    let t = win.label.trim_end();
    EDITED_MARKS.iter().any(|m| t.strip_suffix(m).is_some_and(|rest| rest.trim_end().ends_with(['—', '–', '-'])))
}

/// 書類の窓に人の未保存の変更が在るか。`edited` = 題が編集済み・`mine` = Shibubu が置いた後に残した本文・`now` = 今の本文。
/// 題が編集済みでも、今の本文が Shibubu の置いた直後のままなら人は触っていない。人が続けて打てば本文が違うので true。
fn human_unsaved(edited: bool, mine: Option<&str>, now: &str) -> bool {
    edited && mine != Some(now)
}

/// RQ6（RQ-025）: 窓を読まない app（password manager）。AI に渡す物にも記録にも入れない。bundle id の完全一致（小文字で比べる）。
const NEVER_READ: [&str; 8] = [
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
    "com.apple.keychainaccess",
    "com.apple.passwords",
    "org.keepassxc.keepassxc",
    "com.lastpass.lastpass",
    "com.dashlane.dashlanephonefinal",
];

/// RQ6（RQ-025）: 読んだ要素の secure text（password の欄 = subrole AXSecureTextField）の値を落とす。名前と役割は残す（どの欄かは言える・中身は言わない）。
/// REVIEW-RQN（operator 5）: `own` = Shibubu 自身の窓（読む窓の pid がこの process）は、名前と値の中の email も伏せる（Settings › AI & Models の account の行は
/// 画面の読み上げのため AX の名前に email を持つ・AI に渡す読みと記録には入れない）。他の app の窓の email は残す（その app を操作する依頼で要る）。
fn hide_secrets(items: &mut [Element], own: bool) {
    for e in items.iter_mut() {
        if e.subrole == "AXSecureTextField" {
            e.value = None;
        }
        if own {
            e.label = hide_emails(&e.label);
            e.value = e.value.as_deref().map(hide_emails);
        }
    }
}

/// 文の中の email の形の語（`@` の前が空でなく、後ろに `.` を含む）を `(email hidden)` に替える。空白はそのまま・`@team` のような語は残す。
fn hide_emails(text: &str) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|w| {
            let word = w.trim_end();
            match word.split_once('@') {
                Some((user, host)) if !user.is_empty() && host.contains('.') => format!("(email hidden){}", &w[word.len()..]),
                _ => w.to_owned(),
            }
        })
        .collect()
}

/// RQ6（RQ-024）: 帯の「今の活動」の 1 行（`Typing in {app}` 相当の人の待ち / `Shibubu is …`）。最後に言った行と時刻。`activity_line` が TTL の間だけ返す。
static ACTIVITY: std::sync::Mutex<Option<(String, std::time::Instant)>> = std::sync::Mutex::new(None);
const ACTIVITY_TTL: std::time::Duration = std::time::Duration::from_secs(4);
/// RQ6: 人が前面で作業している間、書く手を待たせる上限と見に行く間隔。上限を過ぎたら止めて 1 行（無期限には止めない）。試験の crate では待たない。
const HUMAN_WAIT_DEFAULT: std::time::Duration = std::time::Duration::from_secs(10);
const HUMAN_POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// RQ6（UIUX §6.4）: 送った手の後で前面の app が替わった回数（Shibubu が focus を奪った疑い）。process の間だけ数える。
static FOCUS_TAKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn say_activity(line: String) {
    *ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()) = Some((line, std::time::Instant::now()));
}

/// 帯に出す今の活動の 1 行（なければ None）。例: `Waiting for you to finish typing in TextEdit`・`Shibubu is typing in TextEdit`。app.rs が毎 frame 読んで帯へ渡す。
/// Wave: RQ6
pub fn activity_line() -> Option<String> {
    ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(_, at)| at.elapsed() < ACTIVITY_TTL).map(|(l, _)| l.clone())
}

/// 前面の app を奪った回数（`Copy diagnostics` の行）。
/// Wave: RQ6
pub fn focus_taken_count() -> u64 {
    FOCUS_TAKEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// 帯の活動の行を消す（待つのをやめた時・手を送る時に、もう言わない事を言い続けない）。
fn clear_activity() {
    *ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 人が前面の間、帯に `Waiting for you to finish typing in {app}` を言いながら `max` まで待つ。返すのは (待った時間, 上限で諦めたか)。
/// 続けても諦めても帯の「待っている」は消す（RQ6 反証 rqa1: 止めた後も 4 秒間 Waiting を言い続けた）。
fn wait_for_human(app: &str, mut still_holds: impl FnMut() -> bool, max: std::time::Duration, step: std::time::Duration) -> (std::time::Duration, bool) {
    let r = wait_while_human_holds(
        || {
            say_activity(format!("Waiting for you to finish typing in {app}"));
            still_holds()
        },
        max,
        step,
        std::thread::sleep,
    );
    clear_activity();
    r
}

/// 人が持っている間、`max` まで `step` おきに `still_holds` を見て待つ。待った時間と、まだ持っているか（= 上限で諦める）を返す。`wait` は待つ道具（試験は偽の時計）。
fn wait_while_human_holds(mut still_holds: impl FnMut() -> bool, max: std::time::Duration, step: std::time::Duration, mut wait: impl FnMut(std::time::Duration)) -> (std::time::Duration, bool) {
    let mut waited = std::time::Duration::ZERO;
    while still_holds() {
        if waited >= max {
            return (waited, true);
        }
        wait(step);
        waited += step;
    }
    (waited, false)
}

/// 押す前の読みで app が答えず、何も送らなかった時の 1 行（G31・`gui_result`）
const NOT_ANSWERING_LINE: &str = "The app isn't answering, so nothing was sent. Wait a moment, then ask again";
/// ENGINE-ADJUST-1102 ①: 人の Stop で窓の読みを止めた 1 行（続きを頼まない・読みかけは使わない・`Rejected`）
const STOPPED_READ_LINE: &str = "Stopped by you · the window wasn't read";
/// ENGINE-ADJUST-1102 ①: 人の Stop で書く手を送らなかった 1 行（`op` の門・`Rejected`）
const STOPPED_SEND_LINE: &str = "Stopped by you · nothing was sent";
/// G74c: menu の項目を押す手が、前の手の身振りで出る menu を待つ長さと読み直しの間（Discord の menu は数百 ms 遅れて描かれる）。回数でなく時間で切る
/// （Discord の 2000 までの読みは 1 回 5 秒かかる事がある・G31 log :71 = 回数で切ると最長が読みの数倍に延びる）。待ち始めから MENU_WAIT を過ぎたら読み直さない
const MENU_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const MENU_GAP: std::time::Duration = std::time::Duration::from_millis(300);
/// Chromium の中身を頼んだ直後（ax_tree の waking）に読み直す回数と間（G31 実測: 中身が出るまで 1〜3 秒以上・混み具合で変わる）
const WAKE_TRIES: usize = 4;
const WAKE_GAP: std::time::Duration = std::time::Duration::from_secs(1);
/// AX の窓の枠が CG にまだ無い窓（`cg_match: none`）の読み直し（G31b・REAPER の起動直後に AeroSpace が窓を並べ直す間の読み・G31 log.md:12〜14）。
/// CG_SETTLE_GAP おきに最長 CG_SETTLE_TRIES 回。ambiguous（同じ枠の CG の窓が 2 つ以上）は待っても直らないので読み直さない
const CG_SETTLE_TRIES: usize = 3;
const CG_SETTLE_GAP: std::time::Duration = std::time::Duration::from_millis(500);

/// `MacGui::windows_where` の読み（G31b で数と WindowInfo が増え、tuple に並べると呼ぶ側で取り違えるので名前を付けた・mac_gui の中だけ）
struct Listed {
    /// CG の窓番号が付いた窓（registry の `desktop.windows.list` の payload）
    windows: Vec<WindowRef>,
    /// 読めた窓の全部（窓番号の付かない窓も・`pick_by_role` が役割を読む元）
    infos: Vec<WindowInfo>,
    /// 読めなかった物の 1 行ずつ（答えなかった app・AX が読めなかった app・CG の窓番号が付かなかった窓）。宛先を決め直す側が理由に足す
    skipped: Vec<String>,
    /// 答えなかった app の数
    unanswered: usize,
    /// 読み直しても CG に枠が出なかった窓（`cg_match: none`）の数
    unsettled: usize,
}

/// 宛先の窓を決められない時の 1 行（`MacGui::retargeted` と `desktop.windows.list` の main〈G75〉で同じ文）: 決め方の 1 行（`e`）に、読めた窓の題名と
/// 読めなかった物を足す（「無い」と断定しない・W19-3b の実機では窓が開いていたのに「見つかりません」）。`which` = 探した物（「the window with “README.md”」）。
fn unresolved(l: &Listed, app: &str, which: &str, e: GuiError) -> String {
    let line = match e {
        // 1 つも読めず、答えなかった app がある = 窓が無いとは言えない（W19-3b 実機: 開いている窓に「開いてから」と言っていた）。
        // 読み直しても CG に枠が出ない窓だけ（G31b・REAPER の起動直後）も「無い」と言わない。窓番号が ambiguous の窓だけの時は待っても直らないので、決め方の 1 行のまま
        GuiError::NotFound(_) if l.windows.is_empty() && l.unanswered > 0 => format!("{app} isn't answering, so {which} couldn't be found. Wait a moment, then ask again"),
        GuiError::NotFound(_) if l.windows.is_empty() && l.unsettled > 0 => format!("{app} is still placing its window, so {which} couldn't be found. Wait a moment, then ask again"),
        e => e.to_string(),
    };
    // 題名は “” で包む（Page の 1 行は “” の外の `a.b` を識別子と読んで定型文に替える・engine_page::plain）
    // 題名は AX（kAXTitle）の読み・窓番号は CG の枠の一致。題名が空の窓も数に入れて「（題名なし）」と出す（AX が題名を返さない場面と窓が無い場面を分ける）
    let seen: String = l.windows.iter().take(8).map(|w| format!("“{}”", if w.title.trim().is_empty() { "(untitled)" } else { &w.title })).collect();
    let read = if l.windows.len() == 1 { "window was" } else { "windows were" };
    let seen = format!("{} {app} {read} read{}{seen}", l.windows.len(), if seen.is_empty() { "" } else { ": " });
    if l.skipped.is_empty() { format!("{line} ({seen})") } else { format!("{line} ({seen} · not readable: {})", l.skipped.join("・")) }
}

impl MacGui {
    /// bridge はまだ起こさない（`check` か最初の呼び出しで `surface::find_bridge` → `Bridge::spawn`）。
    pub fn new() -> Self {
        Self {
            bridge_path: None,
            bridge: None,
            sync: SourceSync::default(),
            seen: HashMap::new(),
            put_titles: HashMap::new(),
            within: None,
            pinned: None,
            sent_to: std::collections::HashSet::new(),
            not_ready: None,
            bundles: HashMap::new(),
            act_log: None,
            consent_file: None,
            human_wait: HUMAN_WAIT_DEFAULT,
            progress: None,
            stop: None,
        }
    }
    /// 人を待つ上限を替える（試験が短い上限で待ちの本体を走らせる）。
    /// Wave: RQ6
    #[allow(dead_code)]
    pub fn with_human_wait(self, human_wait: std::time::Duration) -> Self {
        Self { human_wait, ..self }
    }
    /// 試験だけ: act の記録の file を決める（env を触らない・並ぶ試験が同じ file に書かない）。
    #[cfg(test)]
    pub fn with_act_log(self, path: PathBuf) -> Self {
        Self {
            act_log: Some(path),
            ..self
        }
    }
    /// 試験だけ: AI に任せる app の同意の file を決める（env を触らない・`surface::grant_act_consent` で書く）。
    #[cfg(test)]
    pub fn with_act_consents(self, path: PathBuf) -> Self {
        Self {
            consent_file: Some(path),
            ..self
        }
    }
    /// act の記録の置き場: `with_act_log` → `gui::ACT_LOG_ENV` → `surface::data_dir()`（FX1）の `gui::ACT_LOG_FILE`。
    /// 試験の build は前の 2 つが無ければ書かない（同じ temp の data dir に試験どうしの記録を混ぜない）。
    /// ENGINE-ADJUST-1102 ①: 人の Stop の印が立っているか（印を持たなければ false）
    fn halted(&self) -> bool {
        self.stop.as_ref().is_some_and(|s| s.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn act_log_path(&self) -> Option<PathBuf> {
        if let Some(p) = self.act_log.clone().or_else(|| std::env::var_os(ACT_LOG_ENV).map(PathBuf::from)) {
            return Some(p);
        }
        if cfg!(test) {
            return None;
        }
        Some(super::super::surface::data_dir().join(ACT_LOG_FILE))
    }
    /// 送った操作 1 つを act の記録に 1 行（W19-1・F1 の経路と W19-3b の「許可が無い間は 0 件」の照合の相手）。送らなかった操作では呼ばない
    /// （例外: app ごとの同意が無くて断った操作は `not_sent: Some(NeedsConsent)` で 1 行）。
    /// 置き場の dir が無ければ作る（書けずに 0 件 =「送っていない」と区別できない、にしない）。
    /// ponytail: それでも書けなければ stderr に 1 行（送った後なので操作の結果は変えない）。画面の 1 行（GuiResult の detail）に出すのは `op` の返り値の形を変える時
    fn record_act(&mut self, target: &GuiTarget, action: &GuiAction, sent_ms: f64, not_sent: Option<NotSent>) {
        let Some(path) = self.act_log_path() else { return };
        let rec = ActRecord {
            receipt: ActReceipt {
                target: target.clone(),
                action: action.clone(),
                sent_ms,
            },
            adapter: "mac_gui".into(),
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            not_sent,
        };
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        let wrote = dir
            .map_or(Ok(()), |d| std::fs::create_dir_all(d).map_err(|e| format!("Can't create the place for the action log, {} ({e})", d.display())))
            .and_then(|_| ActLog::open(&path))
            .and_then(|mut log| log.append(&rec));
        if let Err(e) = wrote {
            eprintln!("shibubu: 送った操作を記録できませんでした: {e}");
        }
    }
    /// pid と CG の窓番号 → act の記録の宛先（bundle id は `app_of` で読んだ値・読んでいなければ空）
    fn target_of(&self, pid: u32, window_id: Option<i64>) -> GuiTarget {
        GuiTarget {
            bundle_id: self.bundles.get(&pid).cloned().unwrap_or_default(),
            pid,
            window_id: window_id.and_then(|w| u32::try_from(w).ok()),
        }
    }
    /// bridge の場所を決めて作る（test 用: env を触らない）。
    pub fn at(path: PathBuf) -> Self {
        Self {
            bridge_path: Some(path),
            human_wait: std::time::Duration::ZERO,
            ..Self::new()
        }
    }
    /// 生きている bridge（無い・止まっていれば起こし直す）。
    fn bridge(&mut self) -> Result<&mut Bridge, BridgeError> {
        if !self.bridge.as_ref().is_some_and(Bridge::alive) {
            let path = match &self.bridge_path {
                Some(p) => p.clone(),
                None => find_bridge()?,
            };
            self.bridge = Some(Bridge::spawn(&path, BRIDGE_READY_LIMIT)?);
        }
        Ok(self.bridge.as_mut().expect("直前に入れた"))
    }
    /// 要求 1 つ（待つ上限は `Bridge::limit`・window_list だけ `Bridge::window_list_limit` = bridge の打ち切りの 3 × 今の AX の上限 + 2 秒・G31b）。
    fn call(&mut self, cmd: &str, args: Value) -> Result<Value, BridgeError> {
        let b = self.bridge()?;
        let limit = if cmd == "window_list" { b.window_list_limit() } else { b.limit() };
        b.call(cmd, args, limit)
    }
    /// `window_list` を 1 回。AX の窓の枠が CG にまだ無い窓（`cg_match: none`）が在れば CG_SETTLE_GAP おきに最長 CG_SETTLE_TRIES 回読み直す
    /// （待つ間は進行の 1 行・AX が読めなくなった読み直しは使わず前の読みを返す・G31b）。返す物 = (最後の result, 待った秒・待たなければ 0)
    fn window_list(&mut self, pid: u64, app: &str) -> Result<(Value, f64), BridgeError> {
        let unsettled = |v: &Value| v["windows"].as_array().is_some_and(|ws| ws.iter().any(|w| w["cg_match"] == "none"));
        let mut list = self.call("window_list", json!({ "pid": pid }))?;
        if !unsettled(&list) {
            return Ok((list, 0.0));
        }
        let progress = self.progress.clone();
        let tell = |line: Option<String>| progress.as_ref().map(|p| p(line));
        tell(Some(format!("Waiting for {app} to finish placing its window")));
        let t = std::time::Instant::now();
        for _ in 0..CG_SETTLE_TRIES {
            std::thread::sleep(CG_SETTLE_GAP);
            match self.call("window_list", json!({ "pid": pid })) {
                Ok(v) if v.get("ax_error").is_none() => list = v,
                Ok(_) => break,
                Err(e) => {
                    tell(None);
                    return Err(e);
                }
            }
            if !unsettled(&list) {
                break;
            }
        }
        tell(None);
        Ok((list, t.elapsed().as_secs_f64()))
    }
    /// 操作の cmd を 1 つ送る（4 つの操作の唯一の口）。`Bridge::call` の Gone は書く前か後かを区別できないので、渡した後の Gone / 時間切れは
    /// 「送ったかもしれない」に倒す（EffectUnknown を「送っていない」と言わない向き）。送った・送ったかもしれない時だけ act の記録に 1 行（W19-1）。
    fn op<T: Serialize>(&mut self, cmd: &str, req: &T, target: &GuiTarget, action: &GuiAction) -> Result<Value, OpError> {
        let not_sent = |e| OpError::Bridge { e, sent: false };
        let human_wait = self.human_wait;
        self.bridge().map_err(not_sent)?;
        // ENGINE-ADJUST-1102 ①: 人の Stop の印が立っていれば書く手を送らない（同意・lease の前 = 何も読まず・何も記録しない）。待っている間に立った時も送らない
        let halt = self.stop.clone();
        let halted = || halt.as_ref().is_some_and(|s| s.load(std::sync::atomic::Ordering::SeqCst));
        if halted() {
            return Err(OpError::Refused(GuiError::Denied(STOPPED_SEND_LINE.into())));
        }
        // app ごとの AI の同意（W19-1・YC_DECISIONS 11 通目「既定は無し」・operator 2026-10-23）: 宛先の bundle id は呼ぶ側の値でなく pid から bridge の
        // `apps` で読み直す。同意が無ければ送らず act の記録に `needs_consent` の 1 行。許可が無い時は許可の 1 行だけ（記録 0 件・W19-3b）
        let (app_name, bundle_id, hidden) = self.app_of(target.pid).map_err(OpError::Refused)?;
        let target = &GuiTarget { bundle_id: bundle_id.clone(), ..target.clone() };
        let consents = act_consents(&self.consent_file.clone().unwrap_or_else(act_consent_path));
        if !consents.contains(&bundle_id) {
            self.require_trust().map_err(OpError::Refused)?;
            self.record_act(target, action, now_ms(), Some(NotSent::NeedsConsent));
            return Err(OpError::Refused(GuiError::Denied(act_consent_prompt(&app_name))));
        }
        // G74f（本物 18:37: ⌘H で隠れた Discord への AXShowMenu は rc 0 で答えたが menu は出なかった）: 隠れたままの app に AXShowMenu は送らない（どの app でも・
        // 届いても効かない手を送った事にしない）。ENGINE-AUDIT W8a: 隠れは `invoke` の `shown_for_act` が読む前に解く（前面は取らない）= ここに来るのは解けなかった時。
        // AXShowMenu 以外の手は送る（効いたかは読み戻し・FX-114 = 送った後の隠れた app の窓の読みは Unverified）。読む口（`read`）は止めない
        let body = serde_json::to_value(req).expect("要求は JSON にできる");
        if hidden && body["action"] == "AXShowMenu" {
            return Err(OpError::Refused(GuiError::Denied(format!("{app_name} is hidden, so nothing was sent. Show its window (it can stay behind other windows), then ask again"))));
        }
        let b = self.bridge().map_err(not_sent)?;
        let limit = b.limit();
        // 人の lease（W19-1・YC_ACCEPTANCE F2）: Native Direct の面では人の入力が Shibubu を通らないので、送る前に元の app を観る。
        // 宛先の app が前面 = 人が触っている。読めなければ送らない（閉じる側）。
        // ponytail: 値の変化（kAXValueChangedNotification）から 5 秒の項は bridge の observe が値の変化を受けていないので None。bridge に足した時に埋める
        // RQ6（RQ-024）: 人が前面の間は黙って止めず、帯に待っている事を言って上限まで待つ（上限を過ぎたら今まで通りの 1 行で止める）。前面は毎回読み直す
        let mut read_err = None;
        let mut holds = |read_err: &mut Option<OpError>| match b.call("frontmost", json!({}), limit) {
            Ok(front) => human_holds(&HumanSignals { app_in_front: front["pid"].as_u64() == Some(target.pid as u64), last_unattributed_change_ms: None }, now_ms()),
            Err(e) => {
                *read_err = Some(not_sent(e));
                false
            }
        };
        if holds(&mut read_err) {
            let (_, gave_up) = wait_for_human(&app_name, || !halted() && holds(&mut read_err), human_wait, HUMAN_POLL);
            if gave_up {
                return Err(OpError::Refused(GuiError::Denied(HUMAN_LEASE_LINE.into())));
            }
        }
        if let Some(e) = read_err {
            return Err(e);
        }
        if halted() {
            return Err(OpError::Refused(GuiError::Denied(STOPPED_SEND_LINE.into())));
        }
        say_activity(format!("Shibubu is {} {app_name}", match cmd {
            CMD_AX_PRESS => "pressing a button in",
            CMD_AX_SET_VALUE | CMD_AX_REPLACE_TEXT => "typing in",
            CMD_AX_MENU => "opening a menu in",
            _ => "sending keys to",
        }));
        let sent_ms = now_ms();
        let r = b.call(cmd, body, limit).map_err(|e| OpError::Bridge { e, sent: true });
        // bridge が送る前に断った（not_trusted・stale_element 等 = Unsupported）は送っていないので書かない
        let maybe_sent = match &r {
            Ok(_) => true,
            Err(o) => matches!(o.result(), GuiResult::Delivered { .. }),
        };
        if maybe_sent {
            self.record_act(target, action, sent_ms, None);
            self.sent_to.insert(target.pid);
        }
        r
    }
    /// 操作の応答 → 型。front_before ≠ front_after は D-g（前面を奪った）の 1 行を返す
    fn reply<R: for<'de> Deserialize<'de>>(v: Value, cmd: &str) -> Result<R, GuiResult> {
        serde_json::from_value(v).map_err(|e| GuiResult::Delivered {
            detail: format!("The answer to {cmd} is in the wrong shape ({e}). It may have been sent already. Read the screen again"),
            sure: false,
        })
    }
    /// 送った後の結果（Delivered）の 1 行。前面が変わったらそれも書く（成功にしない・未検証のまま）
    fn delivered(what: String, front_before: i64, front_after: i64) -> GuiResult {
        let front = if front_before == front_after {
            String::new()
        } else {
            FOCUS_TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // RQ6（UIUX §6.4）: 前面を奪った回数
            format!(". The app in front changed")
        };
        GuiResult::Delivered {
            detail: format!("{what} (whether it took effect isn't verified){front}"),
            sure: true,
        }
    }
    /// AX の許可（今の値を `permission` で読む。起動の後に許可されても効く）。`window_list` は許可が無くても error にせず窓を空で返すので、
    /// 空の一覧を「窓が無い」と読まないために先に見る。
    fn require_trust(&mut self) -> Result<(), GuiError> {
        let v = self
            .call("permission", json!({}))
            .map_err(|e| gui_error(&e))?;
        if v["trusted"].as_bool() != Some(true) {
            return Err(GuiError::Denied(UNTRUSTED_LINE.into()));
        }
        Ok(())
    }
    /// registry の input に `bundle_id` と `document` が在れば、`gui::retarget` で今の pid と窓番号に置き換えた input（無ければ None = そのまま）。
    /// 窓の一覧はその bundle の app だけを読む。Err = 理由の 1 行（許可が無い・窓が 0 個・2 個以上）。
    /// G31: 窓を持たない menu と key は、`document` が無くても `bundle_id` が在れば `repid` で pid だけ決め直す（Discord は自分を更新して pid が変わる）。
    /// G31b: 窓の action（`desktop.window.state`・`desktop.ax.click`・`desktop.ax.type_text`）で `document` が無く `bundle_id` が在れば、保存した
    /// (pid, window_id) がその bundle の今の窓に在る間はそのまま（None）・無ければ `pick_by_role` で窓の役割から決め直す（REAPER・Zoom は AXDocument が無い）。
    fn retargeted(&mut self, action: &str, input: &Value) -> Result<Option<Value>, String> {
        let some = |k: &str| input[k].as_str().filter(|v| !v.trim().is_empty());
        let document = some("document");
        let bundle = match (action, some("bundle_id"), document) {
            // CHAT-013: 開く手は名指しの書類がまだ開いていない時の手 = 書類の窓で決め直さない（決め直すと「isn't open」で開く前に止まる・g101-closed）
            ("desktop.windows.list" | "desktop.apps.list" | OPEN_DOCUMENT, ..) | (_, None, _) => return Ok(None),
            (_, Some(bundle), Some(_)) => bundle,
            ("desktop.window.state" | "desktop.ax.click" | REVEAL | "desktop.ax.type_text", Some(bundle), None) => bundle,
            ("desktop.ax.menu" | "desktop.key", Some(bundle), None) => return self.repid(bundle, some("app_name").unwrap_or(bundle), input),
            _ => return Ok(None),
        };
        let l = self.windows_where(&|_, b| b == bundle, true)?;
        let app = some("app_name").unwrap_or(bundle);
        let picked = match document {
            Some(document) => retarget(bundle, app, document, some("document_file"), &l.windows).map(|t| (t.pid, t.window_id.map(i64::from))),
            // AXDocument の無い窓（G31b）: 保存した窓が今もその bundle の窓ならそのまま（AI が主窓でない窓を選んでいたかもしれない）・無ければ役割で
            None if l.windows.iter().any(|w| input["pid"].as_u64() == Some(u64::from(w.pid)) && input["window_id"].as_i64() == Some(w.window_id)) => return Ok(None),
            None => pick_by_role(&l.infos, app).map(|w| (w.pid, w.cg_id)).map_err(GuiError::NotFound),
        };
        let which = document.map_or("its window".to_string(), |d| format!("the window with “{d}”"));
        let (pid, window_id) = picked.map_err(|e| unresolved(&l, app, &which, e))?;
        // G75 Identity: menu と key は app の主窓に効く = 名指しの書類の窓が主窓（AXMain）の時だけ送る（後ろなら送らない・前面は動かさない）
        if let (Some(d), "desktop.ax.menu" | "desktop.key") = (document, action) {
            if !l.infos.iter().any(|w| w.main && w.pid == pid && w.cg_id.is_some() && w.cg_id == window_id) {
                return Err(format!("“{d}” isn't the front window of {app}, so this step wasn't sent (it would go to another window). Bring “{d}” to the front in {app}, then ask again"));
            }
        }
        let mut v = input.clone();
        v["pid"] = json!(pid);
        if v.get("window_id").is_some() {
            v["window_id"] = json!(window_id);
        }
        Ok(Some(v))
    }
    /// 窓を持たない操作（menu・key）の宛先の pid を bundle で決め直す（G31）。保存した pid がまだその bundle の起動中の process なら None（そのまま）・
    /// 違えば起動中の process がちょうど 1 つの時だけその pid に替えた input。0 個 = 起動していない・2 個以上 = 決められない（推測で選ばない）の 1 行。
    fn repid(&mut self, bundle: &str, app: &str, input: &Value) -> Result<Option<Value>, String> {
        let v = self.call("apps", json!({ "regular_only": false })).map_err(|e| gui_error(&e).to_string())?;
        let pids: Vec<u64> = v["apps"].as_array().into_iter().flatten().filter(|a| a["bundle_id"] == bundle).filter_map(|a| a["pid"].as_u64()).collect();
        if input["pid"].as_u64().is_some_and(|p| pids.contains(&p)) {
            return Ok(None);
        }
        match pids.as_slice() {
            [one] => {
                let mut moved = input.clone();
                moved["pid"] = json!(one);
                Ok(Some(moved))
            }
            [] => Err(format!("{app} isn't running. Open it, then ask again")),
            many => Err(format!("{} copies of {app} are running and Shibubu can't tell which. Quit the others, then ask again", many.len())),
        }
    }
    /// pid の app の (名前, bundle_id, hidden)（bridge の `apps` で読んだ値。保存した値をそのまま返さない）。hidden = ⌘H で隠れている（G74f）
    fn app_of(&mut self, pid: u32) -> Result<(String, String, bool), GuiError> {
        let v = self
            .call("apps", json!({ "regular_only": false }))
            .map_err(|e| gui_error(&e))?;
        v["apps"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["pid"].as_u64() == Some(pid as u64))
            .map(|a| {
                (
                    a["name"].as_str().unwrap_or("").to_string(),
                    a["bundle_id"].as_str().unwrap_or("").to_string(),
                    a["hidden"].as_bool().unwrap_or(false),
                )
            })
            .ok_or_else(|| GuiError::NotFound(format!("That app isn't running. Open it, then choose the window again")))
            .inspect(|(_, bundle, _)| {
                self.bundles.insert(pid, bundle.clone());
            })
    }
    /// ENGINE-AUDIT W8a: 書く手（click・type_text・menu・key）の前、窓を読む前に呼ぶ（`invoke` の書く手の腕・`retargeted` の後）。宛先の app が隠れていて、
    /// AI に任せる同意が在り、人の Stop が立っていない時だけ `app_unhide`（前面は取らない）。→ Ok(今も隠れているか)・読めない → Err（`app_of` の 1 行）。
    /// 見せた後の読み（`run_desktop_matching` の `read`）がそのまま「読み直し」になる（`op` の中で解いて読み直すと snapshot が上がり、押す ref が `stale_element` になる）。
    /// 同意の無い app は見せない（見える変化を同意の前に起こさない・`op` が今まで通り同意の 1 行と `NeedsConsent` の記録で断る）。
    /// 解けない時の門は `op`: 隠れたままの app への `AXShowMenu` だけ断る（本物 18:37 = rc 0 で答えて menu が出なかった）・他の手は送る（効いたかは
    /// 読み戻しが決める = FX-114 で送った後の隠れた app の `window.state` は Unverified → 済みと言わない）。解いた app を run の後に隠し直さない。
    /// 例（`invoke`）: `let _ = self.shown_for_act(int("pid") as u32);`
    fn shown_for_act(&mut self, pid: u32) -> Result<bool, GuiError> {
        let (app_name, bundle_id, hidden) = self.app_of(pid)?;
        if !hidden || self.halted() || !act_consents(&self.consent_file.clone().unwrap_or_else(act_consent_path)).contains(&bundle_id) {
            return Ok(hidden);
        }
        say_activity(format!("Shibubu is showing {app_name}'s hidden window"));
        let v = self.call(CMD_APP_UNHIDE, json!(AppUnhideReq { pid })).map_err(|e| gui_error(&e))?;
        let r: AppUnhideReply = serde_json::from_value(v).map_err(|e| GuiError::Bridge(format!("The answer to {CMD_APP_UNHIDE} is in the wrong shape ({e})")))?;
        if r.front_before != r.front_after {
            FOCUS_TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // RQ6（UIUX §6.4）: 前面を奪った回数（unhide は activate しない = 0 のはず）
        }
        Ok(r.hidden)
    }
    /// `apps` → `window_list` → `handle_for` → `ax_tree`（`elements` と `observe` の共通の本体）。bundle = Some なら pid の app がその bundle の時だけ読む。
    /// G31（REAPER・Zoom・Discord の実測）: ax_tree の読み直しが 2 つ。① waking（Chromium の中身を頼んだ直後・枠だけの木）は WAKE_GAP おきに最長
    /// WAKE_TRIES 回・② app が答えない（`AxTreeReply::paused` が時間の区切りでない・空を含む）は AX の上限を伸ばして続きを 1 回（`patiently`）。
    /// それでも 1 つも進まない・waking のままなら「答えない / まだ見せていない」の 1 行で Err（空の木を「何も無い」と読まない）。
    /// owner 256（「300要素で打ち切るとか打ち切ることはしないで」）: 数でも時間でも切らない。時間で区切った応答は続き（`AxTreeReq::more`）を読み終えるまで頼み、
    /// 区切りごとに進行の 1 行「Reading X… N items so far」（部品と chat）。途中まで読めた物を窓の全部として返さない（Ok は読み終えた時だけ）
    fn read(&mut self, w: &WindowRef, bundle: Option<&str>) -> Result<Elements, GuiError> {
        self.require_trust()?;
        let (app_name, bundle_id, _) = self.app_of(w.pid)?;
        if NEVER_READ.contains(&bundle_id.to_ascii_lowercase().as_str()) {
            return Err(GuiError::Denied(format!("{app_name} is a password manager, so Shibubu doesn't read its window")));
        }
        if bundle.is_some_and(|b| b != bundle_id) {
            return Err(GuiError::NotFound(format!("This window now belongs to {app_name}. Choose the window again")));
        }
        let (list, waited) = self.window_list(u64::from(w.pid), &app_name).map_err(|e| gui_error(&e))?;
        let unsettled = waited > 0.0 && list["windows"].as_array().is_some_and(|ws| ws.iter().any(|x| x["cg_match"] == "none"));
        // 108 通目の #12: 決め直した窓番号（retarget の時の一覧）が、読む時の一覧とずれる（窓が増減・並び直しの最中）と「窓が無い」か別の窓を読む。
        // 名指しの書類が在れば、読む一覧の中で `gui::retarget` と同じ一致（書類の file も見る = 別の窓で代用しない）で決め直す。決まらない時は渡された窓番号のまま
        let by_document = self.pinned.as_ref().and_then(|(b, d, f)| retarget(b, &app_name, d, f.as_deref(), &window_refs(&list, &app_name, &bundle_id)).ok());
        let w = &by_document.map_or_else(|| w.clone(), |t| WindowRef { pid: t.pid, window_id: t.window_id.map_or(w.window_id, i64::from), ..w.clone() });
        let found = handle_for(&parse_windows(&list), w);
        let handle = found.map_err(|r| match r {
            // 読み直しても CG に枠が出なかった時は、待った事と bridge の原文を足す（G31b）
            GuiResult::Unsupported(why) if unsettled => GuiError::NotFound(format!("{why} (waited {waited:.1} s for {app_name} to place its window · cg_match: none)")),
            GuiResult::Unsupported(why) => GuiError::NotFound(why),
            other => GuiError::Bridge(format!("{other:?}")),
        })?;
        let root = self.within.clone();
        let started = std::time::Instant::now();
        let req = serde_json::to_value(AxTreeReq {
            handle,
            actionable_only: false,
            root: root.clone(),
            more: false,
        })
        .expect("AxTreeReq は JSON にできる");
        let parse = |v: Value| serde_json::from_value::<AxTreeReply>(v).map_err(|e| GuiError::Bridge(format!("ax_tree の応答の形が違います: {e}")));
        // ENGINE-ADJUST-1102 ①: 人の Stop の印（読み始める前と、続きを頼む前に見る・立っていれば読みかけは使わない）
        let halt = self.stop.clone();
        let halted = || halt.as_ref().is_some_and(|s| s.load(std::sync::atomic::Ordering::SeqCst));
        if halted() {
            return Err(GuiError::Denied(STOPPED_READ_LINE.into()));
        }
        let mut reply = parse(self.call(CMD_AX_TREE, req.clone()).map_err(|e| gui_error(&e))?)?;
        let progress = self.progress.clone();
        let tell = |line: Option<String>| progress.as_ref().map(|p| p(line));
        if reply.waking {
            tell(Some(format!("Waiting for {app_name} to show what's in its window")));
            for _ in 0..WAKE_TRIES {
                std::thread::sleep(WAKE_GAP);
                match self.call(CMD_AX_TREE, req.clone()).map_err(|e| gui_error(&e)).and_then(parse) {
                    Ok(r) => reply = r,
                    Err(e) => {
                        tell(None);
                        return Err(e);
                    }
                }
                if !reply.waking {
                    break;
                }
            }
            tell(None);
        }
        if reply.waking {
            return Err(GuiError::NotFound(format!("{app_name} hasn't shown what's in its window yet. Wait a moment, then ask again")));
        }
        // owner 256: 読み終えるまで続きを頼む（doc）。reading = 進行の 1 行を出した（最後に消す）
        let mut more = req.clone();
        more["more"] = json!(true);
        let mut items = std::mem::take(&mut reply.elements);
        let (mut slices, mut reading) = (1, false);
        while !reply.complete {
            let busy = reply.paused.as_deref() != Some(PAUSED_TIME);
            if !busy {
                reading = true;
                tell(Some(format!("Reading {app_name}… {} so far", plural(items.len(), "item"))));
            }
            let stop = |why: GuiError| -> Result<Elements, GuiError> {
                if reading {
                    tell(None);
                }
                Err(why)
            };
            // ENGINE-ADJUST-1102 ①（owner 256「止めるのは人の Stop だけ」）: 続きを頼む前に印を見る。立っていれば more を頼まず、ここまでの読みかけは使わない
            if halted() {
                return stop(GuiError::Denied(STOPPED_READ_LINE.into()));
            }
            let next = if busy {
                match self.patiently(&[app_name.as_str()], 1, |b, limit| b.call(CMD_AX_TREE, more.clone(), limit)) {
                    Some(r) => r,
                    None => self.call(CMD_AX_TREE, more.clone()),
                }
            } else {
                self.call(CMD_AX_TREE, more.clone())
            };
            let mut next = match next.map_err(|e| gui_error(&e)).and_then(parse) {
                Ok(r) => r,
                Err(e) => return stop(e),
            };
            if next.snapshot != reply.snapshot {
                return stop(GuiError::NotFound(format!("{app_name}'s window was read again by something else while Shibubu was reading it. Ask again")));
            }
            if busy && next.elements.is_empty() && !next.complete && next.paused.as_deref() != Some(PAUSED_TIME) {
                return stop(GuiError::NotFound(if items.is_empty() {
                    format!("{app_name} isn't answering, so its window couldn't be read. Wait a moment, then ask again")
                } else {
                    format!("{app_name} stopped answering while its window was being read ({} so far), so nothing in it was used. Wait a moment, then ask again", plural(items.len(), "item"))
                }));
            }
            items.append(&mut next.elements);
            reply = next;
            slices += 1;
        }
        if reading {
            tell(None);
        }
        // FX-112・owner 256: 読んだ要素の数・区切りの数・ms（手ごとの読みの記録・部分木で読む前後を比べる）
        eprintln!(
            "shibubu: ax_tree {app_name}{}・要素 {}・辿った {}・区切り {slices}・{} ms",
            root.as_ref().map_or(String::new(), |r| format!("・部分木 {}", r.role)),
            items.len(),
            reply.visited,
            started.elapsed().as_millis()
        );
        // FX-112: 部分木の根が見つからない時は窓全体の部分一致で代わりにしない
        if root.is_some() && reply.root_found == Some(false) {
            return Err(GuiError::NotFound(format!("The part of {app_name}'s window this step works in wasn't found (it may have moved or closed), so nothing there was read or changed. Ask again")));
        }
        if items.is_empty() {
            return Err(GuiError::NotFound(format!("{app_name} isn't answering, so its window couldn't be read. Wait a moment, then ask again")));
        }
        // RQ6（RQ-025）: password の欄の値は AI に渡さず記録にも残さない・REVIEW-RQN 5: 自分の窓の email も
        hide_secrets(&mut items, w.pid == std::process::id());
        // ponytail: 読んだ ref を貯めるだけ。1 万を超えたら捨てる（古い ref の verify は Unverified になる）
        if self.seen.len() > 10_000 {
            self.seen.clear();
        }
        self.seen.extend(items.iter().map(|e| (e.r.0.clone(), (e.role.clone(), e.label.clone()))));
        Ok(Elements {
            app_name,
            snapshot: reply.snapshot,
            items,
        })
    }
    /// 旧 Desktop パネルの「① ウィンドウを検出」（YC_GATE #5・旧 `cua::list_windows` の代わり）。bridge の `apps`（regular の app）→ pid ごとに
    /// `window_list` → `cg_id` が決まった窓だけを、app の名前つきで返す（`cg_match` が none / ambiguous の窓は推測で入れない）。
    /// 使い方: `let ws = g.windows()?; let els = g.elements(&ws[0])?;`
    /// 保証: 読むだけ・cua-driver を起こさない・前面を変えない。Err = 理由 1 行（bridge が無い・版が違う・許可が無い）。保証しない: 一覧が今の画面と同じ事（呼んだ時点の読み）。
    /// Wave: W17-1
    pub fn windows(&mut self) -> Result<Vec<WindowRef>, String> {
        self.windows_where(&|_, _| true, false).map(|l| l.windows)
    }
    /// `windows` の本体。`keep(pid, bundle_id)` が true の app の窓だけ（他の app に `window_list` を送らない・答えない app を待たない）。
    /// patient = 答えなかった app を AX の上限を伸ばして 1 回読み直す（絞った読みだけ。全部の app の一覧では 1 つの忙しい app で長く止めない）。
    /// CG に枠がまだ無い窓（`cg_match: none`）の app は短く読み直す（`window_list`・絞らない一覧でも・G31b）。返す物 = `Listed`
    fn windows_where(&mut self, keep: &dyn Fn(u64, &str) -> bool, patient: bool) -> Result<Listed, String> {
        self.require_trust().map_err(|e| e.to_string())?;
        let apps = self
            .call("apps", json!({}))
            .map_err(|e| gui_error(&e).to_string())?;
        // app ごとの読み（Err = 読めなかった物の 1 行・秒 = CG の枠を待った秒）。答えなかった app の読み直しは全部を読んでから（待つ app の数で最長の秒を言う）
        let mut read: Vec<(u64, String, String, Result<(Value, f64), String>)> = Vec::new();
        for a in apps["apps"].as_array().into_iter().flatten() {
            let (Some(pid), name, bundle) = (a["pid"].as_u64(), a["name"].as_str().unwrap_or(""), a["bundle_id"].as_str().unwrap_or("")) else {
                continue;
            };
            if !keep(pid, bundle) {
                continue;
            }
            // 1 つの app が答えない（AX の時間切れ）だけなら、その app を飛ばして他を出す。bridge が止まった時だけ Err
            let list = match self.window_list(pid, name) {
                Ok(v) => Ok(v),
                Err(
                    e @ (BridgeError::Gone(_)
                    | BridgeError::Broken(_)
                    | BridgeError::Spawn(_)
                    | BridgeError::NotFound(_)),
                ) => return Err(gui_error(&e).to_string()),
                Err(e) => Err(format!("{name} ({})", if matches!(e, BridgeError::Timeout { .. }) { "Timed out".to_string() } else { e.to_string() })),
            };
            read.push((pid, name.to_string(), bundle.to_string(), list));
        }
        // 裏に押し出された app は最初の AX の要求に 3 秒では答えない（W19-3b 実機: load 166 で TextEdit の 1 回目 14.3 秒・2 回目 0.1 秒）
        let slow: Vec<usize> = (0..read.len()).filter(|&i| patient && matches!(&read[i].3, Ok((l, _)) if l["ax_error"]["code"] == "ax_cannot_complete")).collect();
        if !slow.is_empty() {
            let who: Vec<(u64, String)> = slow.iter().map(|&i| (read[i].0, read[i].1.clone())).collect();
            let names: Vec<&str> = who.iter().map(|(_, n)| n.as_str()).collect();
            let again = self
                .patiently(&names, who.len(), |b, limit| who.iter().map(|(pid, _)| b.call("window_list", json!({ "pid": pid, "budget_s": PATIENT_AX_TIMEOUT }), limit).ok()).collect::<Vec<_>>())
                .unwrap_or_else(|| vec![None; who.len()]);
            for (&i, again) in slow.iter().zip(again) {
                if let Some(v) = again {
                    read[i].3 = Ok((v, 0.0));
                }
            }
        }
        let mut l = Listed { windows: Vec::new(), infos: Vec::new(), skipped: Vec::new(), unanswered: 0, unsettled: 0 };
        for (_, name, bundle, list) in read {
            let (list, waited) = match list {
                Ok(v) => v,
                Err(line) => {
                    l.skipped.push(line);
                    l.unanswered += 1;
                    continue;
                }
            };
            if let Some(m) = list["ax_error"]["message"].as_str() {
                l.skipped.push(format!("{name} ({m})"));
                l.unanswered += 1;
            }
            for raw in list["windows"].as_array().into_iter().flatten() {
                let Some(x) = WindowInfo::from_json(raw) else { continue };
                match x.cg_id {
                    Some(window_id) => l.windows.push(WindowRef { pid: x.pid, window_id, app_name: name.clone(), title: x.title.clone(), bundle_id: bundle.clone(), document: raw["document"].as_str().unwrap_or("").to_owned() }),
                    // 読み直しても CG に枠が出なかった窓は、待った秒と bridge の原文を足す（G31b・「無い」と読まない）
                    None if waited > 0.0 && raw["cg_match"] == "none" => {
                        l.unsettled += 1;
                        l.skipped.push(format!("“{}” in {name} (the window number can't be read · waited {waited:.1} s · cg_match: none)", x.title_text()));
                    }
                    None => l.skipped.push(format!("“{}” in {name} (the window number can't be read)", x.title_text())),
                }
                l.infos.push(x);
            }
        }
        Ok(l)
    }
    /// 答えなかった app（名前）の読みを、bridge の AX の上限を PATIENT_AX_TIMEOUT 秒に伸ばして `again(bridge, 1 回の待つ上限)` で読み直し、元に戻す
    /// （`window_list` の読み直しと、G31 から `read` の途中で切れた木の読み直し）。`calls` = again が bridge に送る数（最長の秒の見積もりだけに使う）。
    /// Rust 側の待ち（`Bridge::limit` = hello の ax_timeout + 2）も合わせて動かす（bridge だけ伸ばすと Rust が先に時間切れ）。
    /// 待つ間は進行の 1 行（`Adapter::set_progress`）に「X が答えないので待っています（最長 N 秒）」（N = calls × 伸ばした後の `Bridge::limit`）・
    /// 読み直しが終わったら None（答えても答えなくても・W19-3b G2）。伸ばせない（config を知らない bridge）・bridge を起こせない時は読み直さず 1 行も出さない（None）。
    /// window_list の読み直しは要求に `budget_s: PATIENT_AX_TIMEOUT` を付ける（G31b: bridge は今の上限の 3 倍 = 90 秒まで読むので、1 回の待つ上限 30 + 2 秒の中で打ち切らせる）
    fn patiently<T>(&mut self, names: &[&str], calls: usize, again: impl FnOnce(&mut Bridge, std::time::Duration) -> T) -> Option<T> {
        // ENGINE-ADJUST-1102 ①: 人の Stop の後は答えない app を待たない（None = 呼ぶ側は「答えない」の道のまま・何も送らない）
        if self.halted() {
            return None;
        }
        let set = |b: &mut Bridge, t: Value| {
            let limit = b.limit();
            b.call("config", json!({ "ax_timeout": t }), limit).is_ok().then(|| b.hello["ax_timeout"] = t)
        };
        let tell = |p: &Option<Progress>, line: Option<String>| {
            if let Some(p) = p {
                p(line)
            }
        };
        let progress = self.progress.clone();
        let b = self.bridge().ok()?;
        let base = b.hello["ax_timeout"].as_f64().filter(|t| t.is_finite() && *t > 0.0).unwrap_or(3.0);
        set(b, json!(PATIENT_AX_TIMEOUT))?;
        let limit = b.limit();
        let mut uniq: Vec<&str> = Vec::new();
        for &n in names {
            let n = if n.trim().is_empty() { "Target app" } else { n };
            if !uniq.contains(&n) {
                uniq.push(n);
            }
        }
        let most = (limit.as_secs_f64() * calls as f64).ceil();
        tell(&progress, Some(format!("Waiting because {} isn't answering (up to {most} s)", uniq.join("・"))));
        let out = again(b, limit);
        set(b, json!(base));
        tell(&progress, None);
        Some(out)
    }
    /// 窓の要素を読むだけ（YC_GATE #2 の get_window_state・#5 の「② 要素」）。`window_list` → `handle_for` → `ax_tree`。
    /// Err = 未確認の理由 1 行（許可なし・窓が無い・CG の窓番号が曖昧）。古い snapshot の ref は無効になる。
    /// 使い方: `let els = g.elements(&w)?; let el = els.find("保存", "AXButton")?;`
    /// Wave: W17-1
    pub fn elements(&mut self, w: &WindowRef) -> Result<Elements, String> {
        self.read(w, None).map_err(|e| e.to_string())
    }
    /// `action`（AXPress・G74b の AXShowMenu = context menu を開く）を送る。Verified は返さない（効いたかの読み戻しは呼ぶ側）
    pub fn press(&mut self, w: &WindowRef, el: &ElementRef, action: &str) -> GuiResult {
        self.press_with(w, el, action, false)
    }
    /// ENGINE-AUDIT W5: `ladder` = 要素自身が action を持たなければ bridge の梯子で押す（どの段で押せたかは detail の「via …」）
    pub fn press_with(&mut self, w: &WindowRef, el: &ElementRef, action: &str, ladder: bool) -> GuiResult {
        let req = AxPressReq { ladder, ..self.press_req(el, action) };
        let t = self.target_of(w.pid, Some(w.window_id));
        match self.op(CMD_AX_PRESS, &req, &t, &GuiAction::Press { element: el.clone() }) {
            Ok(v) => match Self::reply::<AxPressReply>(v, CMD_AX_PRESS) {
                Ok(r) => Self::delivered(
                    // focus を頼んだのに置けなかった・答えに無い（focus を知らない古い bridge）時はそれも書く。梯子で押した段も
                    format!("Sent {} to {}'s window{}{}", r.performed, w.app_name, r.via.as_deref().filter(|v| *v != "self").map(|v| format!(" via {v}")).unwrap_or_default(), match (req.focus, r.focus_rc) {
                        (false, _) | (true, Some(0)) => String::new(),
                        (true, Some(rc)) => format!(" (focus couldn't be set first, AXError {rc})"),
                        (true, None) => " (the bridge didn't say whether focus was set first)".into(),
                    }),
                    r.front_before,
                    r.front_after,
                ),
                Err(g) => g,
            },
            Err(o) => o.result(),
        }
    }
    /// `ax_press` の要求。G74f: menu の項目（今までの読みで role AXMenuItem）の AXPress だけ focus を付ける（本物 18:45: Discord の menu の項目は
    /// AXFocused=true を置いた後の AXPress で確認の dialog が出た・17:10 は置かずに押して何も起きなかった）
    /// ponytail: 18:45 は Discord を見せた後でもあった = focus と見えている事のどちらが効いたかは未分離。focus 無しで効くと分かれば外す
    fn press_req(&self, el: &ElementRef, action: &str) -> AxPressReq {
        AxPressReq {
            element: el.clone(),
            action: (action != "AXPress").then(|| action.to_string()),
            focus: action == "AXPress" && self.seen.get(&el.0).is_some_and(|(role, _)| role == "AXMenuItem"),
            ladder: false,
        }
    }
    /// AXValue の set → 同じ要素の AXValue を読み戻し、一致で Verified・違えば Delivered{sure:true}・settable でなければ Unsupported。
    /// G79: replace = 欄の中のちょうど 1 か所だけを text に置き換える（`ax_replace_text`・0 か所 / 2 か所以上は bridge が何も置かずに断る = Unsupported）。
    /// Verified = bridge が全文で合うと読み、読み戻しに text が在り、全文の長さの差 = len(text) − len(replace)（UTF-16・bridge が切らずに数えた長さ）
    pub fn set_text(&mut self, w: &WindowRef, el: &ElementRef, text: &str, replace: Option<&str>) -> GuiResult {
        let req = AxSetValueReq {
            element: el.clone(),
            value: text.to_string(),
            replace: replace.map(String::from),
        };
        let t = self.target_of(w.pid, Some(w.window_id));
        let act = GuiAction::SetValue {
            element: el.clone(),
            value: text.to_string(),
        };
        let cmd = if replace.is_some() { CMD_AX_REPLACE_TEXT } else { CMD_AX_SET_VALUE };
        let units = |s: &str| s.encode_utf16().count() as i64;
        let held = |r: &AxSetValueReply| {
            r.matches
                && replace.is_none_or(|old| {
                    r.read_back.as_deref().is_some_and(|b| b.contains(text)) && r.length_before.zip(r.length_after).is_some_and(|(b, a)| a - b == units(text) - units(old))
                })
        };
        match self.op(cmd, &req, &t, &act) {
            Ok(v) => match Self::reply::<AxSetValueReply>(v, cmd) {
                Ok(r) if held(&r) && r.front_before == r.front_after => GuiResult::Verified {
                    evidence: match replace {
                        None => format!("Put {} characters in the field of {} and read the field back: they match", text.chars().count(), w.app_name),
                        Some(old) => format!(
                            "Replaced the one place in the field of {} ({} characters became {}) and read the field back: it holds them and its length changed by exactly that much",
                            w.app_name,
                            old.chars().count(),
                            text.chars().count()
                        ),
                    },
                },
                Ok(r) => {
                    let back = r
                        .read_back
                        .as_deref()
                        .map_or("Can't be read".to_string(), |b| {
                            format!("{} characters", b.chars().count())
                        });
                    Self::delivered(
                        format!(
                            "Put a value in the field of {} (read back: {back} · differs from what was put)",
                            w.app_name
                        ),
                        r.front_before,
                        r.front_after,
                    )
                }
                Err(g) => g,
            },
            Err(o) => o.result(),
        }
    }
    /// G74 ②③: 窓をもう 1 回読み（置いた道 = bridge の ax_set_value の読み戻しとは別の読み）、label と role がちょうど 1 つの欄の値が `text` と同じか。
    /// Ok = 何を読んで一致と言ったかの 1 行・Err = 違う / 読めない理由（字数だけ・欄の中身と label は出さない = channel の名前を書かない）。
    /// G79: replace が在れば欄の一部だけを置き換えた手 = 欄が text を含めば合う（欄の残りは置き換えていない字・長さは置いた時に bridge が全文で見た）。
    fn field_holds(&mut self, w: &WindowRef, label: &str, role: &str, text: &str, replace: Option<&str>) -> Result<String, String> {
        let els = self.read(w, None).map_err(|e| format!("the window couldn't be read ({e})"))?;
        if !w.app_name.is_empty() && els.app_name != w.app_name {
            return Err("the window now belongs to another app".into());
        }
        let el = els.find(label, role).map_err(|_| "the field couldn't be found as exactly one".to_string())?;
        let n = text.chars().count();
        match el.value.as_deref().map(field_text) {
            Some(v) if replace.is_some() && v.contains(&field_text(text)) => Ok(format!("a new read of the window ({} elements) shows the field holds those {n} characters", els.items.len())),
            Some(v) if v == field_text(text) => Ok(format!("a new read of the window ({} elements) shows the field holds exactly those {n} characters", els.items.len())),
            Some(v) => Err(format!("it holds {} characters that differ from the {n} that were put", v.chars().count())),
            None => Err("its text couldn't be read".into()),
        }
    }
    /// pid 宛ての key（CGEvent.postToPid・前面を変えない）。Delivered だけ（届いたかは bridge も知らない）
    pub fn key(&mut self, pid: u32, chord: &Chord) -> GuiResult {
        let req = KeyToPidReq::chord(pid, chord);
        let t = self.target_of(pid, None);
        let mods = [(chord.cmd, "cmd"), (chord.shift, "shift"), (chord.option, "option"), (chord.control, "control")];
        let key = match &chord.key {
            Key::Char(c) => c.to_string(),
            Key::Named(n) => n.clone(),
        };
        let keys = mods.iter().filter(|(on, _)| *on).map(|(_, m)| m.to_string()).chain([key]).collect::<Vec<_>>().join("+");
        match self.op(CMD_KEY_TO_PID, &req, &t, &GuiAction::Key { keys }) {
            Ok(v) => match Self::reply::<KeyToPidReply>(v, CMD_KEY_TO_PID) {
                Ok(r) => Self::delivered(
                    format!("Sent keys to the app ({} key presses)", r.sent),
                    r.front_before,
                    r.front_after,
                ),
                Err(g) => g,
            },
            Err(o) => o.result(),
        }
    }
    /// menu bar を題名の完全一致で辿り、最後を AXPress（例 `&["ファイル", "保存"]`）。Verified は返さない。
    /// 押す前の読みが灰色（enabled_before = false）でも押し、Delivered の 1 行に「押す前の読みは灰色」を足す（W17-2 戻し・背景の app の menu の有効は古い事がある）
    pub fn menu(&mut self, pid: u32, path: &[&str]) -> GuiResult {
        let req = AxMenuReq {
            pid,
            path: path.iter().map(|p| p.to_string()).collect(),
        };
        let t = self.target_of(pid, None);
        match self.op(CMD_AX_MENU, &req, &t, &GuiAction::Menu { path: req.path.clone() }) {
            Ok(v) => match Self::reply::<AxMenuReply>(v, CMD_AX_MENU) {
                Ok(r) => Self::delivered(
                    format!(
                        "Pressed the menu item “{}”{}",
                        r.pressed.join(" › "),
                        if r.enabled_before == Some(false) { " (It read as grayed out before the press. A menu item of an app in the background can be out of date)" } else { "" }
                    ),
                    r.front_before,
                    r.front_after,
                ),
                Err(g) => g,
            },
            Err(o) => o.result(),
        }
    }
    /// 旧 canvas の lane の Desktop 操作 1 回（YC_GATE #3・#6・`cua::run_desktop` の代わり。`app.rs` の `execute` が呼ぶ）。
    /// 規則（旧 `cua::Driver::run_desktop_outcome` と同じ）: TypeText の text は 1〜1000 byte・押す前に窓を全部読み直す（owner 256）・
    /// `app_name` が空でなく今の app 名と違えば Rejected・label + role が 0 件 = `Failed{TargetNotFound}`・複数 = Rejected・
    /// Click → `press`・TypeText → `set_text`（settable でなければ Rejected・何も打たない）・結果は `From<GuiResult> for Outcome`。
    /// 使い方: registry の `desktop.ax.click` / `desktop.ax.type_text` の invoke が呼ぶ（app.rs の `execute` は registry を通す・W19-1）。
    /// 保証: 送る前の失敗は Rejected / Failed（何も送っていない）・送った後に分からなくなれば EffectUnknown・cua-driver を起こさない。
    /// 保証しない: `app_name` が空の旧 binding（W17-2 より前に登録した物）の app の照合（旧 `cua::Target{app_name: None}` と同じ）。
    /// Wave: W17-2
    pub fn run_desktop(&mut self, kind: &ActionKind) -> Outcome {
        self.run_desktop_matching(kind, &Value::Null, false)
    }
    /// `run_desktop` の本体。`input` = registry の input（無ければ Null）: match = "contains"（M2 G-c・click だけ・`Elements::find_containing` で探す・0 件も
    /// Rejected の 1 行）・bundle_id と document（G74 ①: 窓を書類で決め直した手 = 頼みの中身が書類そのもの = 欄に字が在っても置いてよい）・
    /// press（G74b: 送る AX の action・click だけ）・only_if（G74b: 押す前の同じ読みで、押す要素を囲む dialog がこの字を全部見せている時だけ押す）
    /// W7: `reveal` = 承認の要らない押す手（`REVEAL`）= 解決した要素が `gui::asks` に当たれば押さずに `Failed { ApprovalDenied }`
    fn run_desktop_matching(&mut self, kind: &ActionKind, input: &Value, reveal: bool) -> Outcome {
        let ActionKind::Desktop {
            pid,
            window_id,
            label,
            role,
            tool,
            text,
            app_name,
        } = kind
        else {
            return Outcome::Rejected {
                reason: "Not a desktop action".into(),
            };
        };
        let typing = *tool == super::super::model::DesktopTool::TypeText;
        let contains = !typing && input["match"] == "contains";
        let document = ["bundle_id", "document"].iter().all(|k| input[*k].as_str().is_some_and(|v| !v.trim().is_empty()));
        let press = if typing { "AXPress" } else { input["press"].as_str().unwrap_or("AXPress") };
        let only_if: Vec<String> = input["only_if"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(String::from)).collect();
        let replace = input["replace"].as_str().filter(|_| typing);
        // 入力の検査は読む前（何も起こさない）
        if !["AXPress", "AXShowMenu"].contains(&press) {
            return Outcome::Rejected { reason: format!("Shibubu doesn't send {press}. Nothing was pressed") };
        }
        if replace == Some("") {
            return Outcome::Rejected { reason: "The text to replace is empty, so nothing was put".into() };
        }
        if typing && (text.is_empty() || text.len() > 1000) {
            return Outcome::Rejected {
                reason: "The text is empty or too long".into(),
            };
        }
        let Ok(pid32) = u32::try_from(*pid) else {
            return Outcome::Rejected {
                reason: format!("The target window isn't valid any more. Choose it again, then add the step again"),
            };
        };
        let w = WindowRef {
            pid: pid32,
            window_id: *window_id,
            app_name: app_name.clone(),
            title: String::new(),
            bundle_id: String::new(),
            document: String::new(),
        };
        // 押す前に読み直す（ref は読むたびに変わる・別の読みの ref では押さない）。click も type_text も窓の全部を読む（owner 256・M2 G-c・③-8: Discord の
        // 入力欄は窓の奥に在り、bridge の ax_tree は幅優先なので上限で切ると届かない）
        let els = match self.read(&w, None) {
            Ok(e) => e,
            Err(GuiError::Timeout { .. }) => {
                return Outcome::Failed {
                    error: ErrorCode::Timeout,
                }
            }
            Err(GuiError::Denied(why) | GuiError::NotFound(why)) => {
                return Outcome::Rejected { reason: why }
            }
            Err(e) => {
                return Outcome::Rejected {
                    reason: e.to_string(),
                }
            }
        };
        // pid / window_id は再起動で別の app に使い回される。保存した app 名と違えば押さない（空 = W17-2 より前の binding は照合しない）
        if !app_name.is_empty() && els.app_name != *app_name {
            let now = if els.app_name.is_empty() {
                "Unknown"
            } else {
                &els.app_name
            };
            return Outcome::Rejected { reason: format!("The target app may have changed (saved: {app_name} · now: {now}). Add the step again") };
        }
        // G74c（owner 14:4x「文字そのものを長押しすると出てくる」）: menu の項目（role = AXMenuItem）を押す手は、前の手の身振りで出る menu が描かれるまで
        // 読み直し、無ければ何も押さない。項目が無ければ何も押さず、menu に AXCancel を 1 回送って読み直す（閉じなければ「開いたまま・Esc」）
        // G74e（本物 17:10 = Delete Message を押した後に確認の dialog が出なかった）: 確認の dialog の button（only_if の手）も、前の手で出る dialog が
        // 描かれるまで同じ時間だけ読み直す（混雑で遅れた dialog を「出なかった」と言わない）。各読みの数を stderr に 1 行（本物の切り分け・数だけ）
        let mut els = els;
        let item = role == "AXMenuItem";
        let want = label.trim().to_lowercase();
        let named = |e: &Element| e.role == *role && if contains { e.label.to_lowercase().contains(&want) } else { e.label == *label };
        if item || !only_if.is_empty() {
            let start = std::time::Instant::now();
            let shown = |els: &Elements| if item { els.open_menu().is_some() } else { els.items.iter().any(|e| named(e)) };
            for n in 0.. {
                let menu_items = els.items.iter().filter(|e| e.role == "AXMenuItem").count();
                eprintln!("shibubu: {role} を待つ読み {n}・{} ms・要素 {}・menu の項目 {menu_items}・当たり {}", start.elapsed().as_millis(), els.items.len(), shown(&els));
                if shown(&els) || start.elapsed() >= MENU_WAIT {
                    break;
                }
                std::thread::sleep(MENU_GAP);
                els = match self.read(&w, None) {
                    Ok(e) => e,
                    Err(e) => return Outcome::Rejected { reason: e.to_string() },
                };
            }
        }
        if item {
            let app = els.app_name.clone();
            let Some(menu) = els.open_menu().cloned() else {
                return Outcome::Rejected { reason: format!("No menu is open in {app}, so “{label}” wasn't pressed (nothing in the window was pressed). If it closed, for example while you were approving, ask again") };
            };
            let items: Vec<&Element> = els.items.iter().filter(|e| e.role == "AXMenuItem").collect();
            if !items.iter().any(|e| named(e)) {
                let n = if items.len() == 1 { "1 item".to_string() } else { format!("{} items", items.len()) };
                let closed = menu.actions.iter().any(|a| a == "AXCancel")
                    && matches!(self.press(&w, &menu.r, "AXCancel"), GuiResult::Delivered { sure: true, .. })
                    && self.read(&w, None).is_ok_and(|e| e.open_menu().is_none());
                let after = if closed { "The menu was closed".to_string() } else { format!("The menu is still open there. Press Esc in {app} to close it") };
                return Outcome::Rejected { reason: format!("The menu open in {app} ({n}) has no “{label}”, so nothing in it was pressed. {after}") };
            }
        }
        let by = els.by_ref();
        // ENGINE-AUDIT W5: within = 道（role・名前）の中だけで数える（同じ名前が 2 つ以上の時に AI が絞る）・ladder = 押せる形でない要素も当たりにして bridge の梯子で押す
        let within = input["within"].as_object().map(|w| (w.get("role").and_then(Value::as_str).unwrap_or_default(), w.get("label").and_then(Value::as_str).unwrap_or_default())).filter(|(_, l)| !l.trim().is_empty());
        let ladder = input["ladder"] == true;
        let inside = |e: &Element| within.map_or(true, |(r, l)| ancestors(&by, e).any(|a| (r.is_empty() || a.role == r) && els.name_in(&by, a).to_lowercase().contains(&l.trim().to_lowercase())));
        let hits = els
            .items
            .iter()
            .filter(|e| e.role == *role && els.name_in(&by, e) == *label && inside(e))
            .count();
        // W5 の契約: 無い = Failed { TargetNotFound }・絞れない = Failed { TargetAmbiguous }（どちらも何も押していない・Core の execute が探す段に入る）
        let el = match hits {
            _ if contains => match els.find_containing_n(label, role, press, ladder, within) {
                Ok(el) => el.clone(),
                Err(0) => return Outcome::Failed { error: ErrorCode::TargetNotFound },
                Err(_) => return Outcome::Failed { error: ErrorCode::TargetAmbiguous },
            },
            // G74e: 確認の dialog の button が待っても出ない = 前の手は届いたが app が dialog を出さなかった。何も押さず、走った手の後の止め（engine の Stopped）に
            // 何が見えたかの 1 行を渡す（TargetNotFound だと「確かめられない」の文になる）。only_if の無い手は今までどおり
            0 if !only_if.is_empty() => {
                let app = &els.app_name;
                let open = if els.open_menu().is_some() { format!(". A menu is still open there. Press Esc in {app} to close it") } else { String::new() };
                return Outcome::Rejected { reason: format!("No confirmation with “{label}” appeared in {app}, so “{label}” wasn't pressed{open}") };
            }
            0 => {
                return Outcome::Failed {
                    error: ErrorCode::TargetNotFound,
                }
            }
            1 => match els.items.iter().find(|e| e.role == *role && els.name_in(&by, e) == *label && inside(e)) {
                Some(e) => e.clone(),
                None => return Outcome::Failed { error: ErrorCode::TargetNotFound },
            },
            _ => return Outcome::Failed { error: ErrorCode::TargetAmbiguous },
        };
        // ENGINE-AUDIT W7: 承認の要らない押す手は、押す直前の同じ読みで解決した要素の名前と、確認の窓の中かをもう 1 回 `gui::asks` に照らす（計画の段は名前だけ）。
        // 当たれば何も押さない（Core がその手を承認の要る click に書き換えて card）
        if reveal && super::super::gui::asks(&els.name_in(&by, &el), &el.role, els.in_dialog(&el)).is_some() {
            return Outcome::Failed { error: ErrorCode::ApprovalDenied };
        }
        let w = WindowRef {
            app_name: els.app_name.clone(),
            ..w
        };
        // OPEN-INPLACE-1101 の 3: AXPress を持たない一覧の行（子の字が名前）は選ぶ（bridge の ax_press の AXSelect = AXSelected を置く）
        let press = if press == "AXPress" && !el.actions.iter().any(|a| a == press) && row_like(&els.by_ref(), &el) { "AXSelect" } else { press };
        // G74b（Identity Verification）: 押す前の同じ読みで、押す要素を囲む dialog が頼まれた字を全部見せている時だけ押す（窓全体では探さない =
        // 後ろの行に同じ本文が残っていても、dialog が別の message を見せていれば押さない）
        let confirmed = match only_if.is_empty() {
            true => None,
            false => match els.shown_in_dialog(&el, &only_if) {
                Ok(line) => Some(line),
                Err(reason) => return Outcome::Rejected { reason },
            },
        };
        // W5: 要素自身が action を持たない時だけ bridge の梯子（近い祖先の AXPress → 入れ物の AXSelectedRows → 行の AXSelected → AXShowMenu / AXExpand）
        let climb = ladder && press != "AXSelect" && !el.actions.iter().any(|a| a == press);
        let r = match (typing, el.value_settable) {
            (false, _) => match (self.press_with(&w, &el.r, press, climb), confirmed) {
                (GuiResult::Delivered { detail, sure }, Some(c)) => GuiResult::Delivered { detail: format!("{detail}; {c}"), sure },
                (r, _) => r,
            },
            (true, true) => {
                // G74 ①（owner 12:2x・③-8 で Discord の入力欄の書きかけが投稿された）: 打つ前の欄に人の字が在れば、書類の窓でない限り置かない
                // （放置された下書きを消さない・その後の送る手にも進ませない）。placeholder を値として返す欄（値 = label）と、もう打つ文と同じ欄は空とみなす
                let before = el.value.as_deref().map(field_text).unwrap_or_default();
                if !document && !before.trim().is_empty() && before.trim() != el.label.trim() && before != field_text(text) {
                    let app = &w.app_name;
                    return Outcome::Rejected {
                        reason: format!("{app}'s field already has text in it ({} characters) that Shibubu didn't put there, so nothing was sent. Send or clear it in {app} yourself, then ask again", before.chars().count()),
                    };
                }
                // RQ6（RQ-024）: 書類の窓でも、人の未保存の変更（題が「編集済み」で Shibubu が置いた窓でない）が在れば置かない。保存済みの本文だけが対象
                if document {
                    let key = (w.pid, w.window_id);
                    let now = el.value.as_deref().map(field_text).unwrap_or_default();
                    let mut wrote = AI_WROTE.lock().unwrap_or_else(|e| e.into_inner());
                    let edited = title_says_edited(&els);
                    if !edited {
                        wrote.retain(|(k, _)| *k != key); // 人が保存した: 覚えた本文は要らない
                    } else if human_unsaved(true, wrote.iter().find(|(k, _)| *k == key).map(|(_, t)| t.as_str()), &now) {
                        let app = &w.app_name;
                        return Outcome::Rejected {
                            reason: format!("This {app} document has changes you haven't saved, so nothing was put in it. Save or undo them in {app} yourself, then ask again"),
                        };
                    }
                }
                // G79: replace = 欄の今の字のちょうど 1 か所だけを置き換える。読んだ字（bridge は VALUE_READ_MAX 字で切る）で先に数え、2 か所以上と、切れていない字で
                // 0 か所なら何も送らない（送った後も bridge が切らない全文で数え直し、ちょうど 1 か所でなければ何も置かない）
                if let Some(old) = replace {
                    let v = el.value.as_deref().unwrap_or_default();
                    let n = v.char_indices().filter(|(i, _)| v[*i..].starts_with(old)).count();
                    if n >= 2 || (n == 0 && v.chars().count() < VALUE_READ_MAX) {
                        let app = &w.app_name;
                        return Outcome::Rejected {
                            reason: format!("The text to replace is in {n} places in {app}'s field (Shibubu replaces it only where it is in exactly one place), so nothing was put. Ask again"),
                        };
                    }
                }
                let put = self.set_text(&w, &el.r, text, replace);
                if document && matches!(put, GuiResult::Verified { .. } | GuiResult::Delivered { .. }) {
                    // 置いた直後の本文 = replace なら読んだ本文の 1 か所を替えた物・そうでなければ打った文（切れて読んだ本文は次の照合で合わず断る側）
                    let body = el.value.as_deref().unwrap_or_default();
                    let expected = field_text(&match replace {
                        Some(old) => body.replacen(old, text, 1),
                        None => text.to_owned(),
                    });
                    let mut wrote = AI_WROTE.lock().unwrap_or_else(|e| e.into_inner());
                    wrote.retain(|(k, _)| *k != (w.pid, w.window_id));
                    wrote.push(((w.pid, w.window_id), expected));
                }
                match put {
                    // G74 ②: 置いた道（bridge の ax_set_value の読み戻し）が合っても、窓を読み直した欄の値でも合う時だけ Verified
                    // （rich text の欄は置いた道だけが合うと言う・③-8）。証拠の行に両方の読み
                    GuiResult::Verified { evidence } => match self.field_holds(&w, label, role, text, replace) {
                        Ok(again) => GuiResult::Verified { evidence: format!("{evidence}; read again separately: {again}") },
                        Err(why) => GuiResult::Delivered { detail: format!("Put the text in the field of {}, but when the field was read again separately {why}", w.app_name), sure: true },
                    },
                    other => other,
                }
            }
            // 値を置けない欄は打たない: pid 宛てに打つと app の今の focus（承認していない欄・1 字の shortcut）に入る。
            // ponytail: bridge に欄へ focus を移す口が無い。W19 で AXFocused を置いて読み戻してから key_to_pid で打つ
            (true, false) => {
                return Outcome::Rejected {
                    reason: format!("Can't put text in “{label}” (nothing was typed). Press Read controls, choose a field that takes text, then add the typing step again"),
                }
            }
        };
        r.into()
    }
}

/// bridge の ax_tree が要素の値を切る長さ（Swift の `axValueText` の `prefix(2000)`）。これ以上の字は続きが切れているかもしれない
const VALUE_READ_MAX: usize = 2000;

/// G74: 欄の値を比べる形（Chromium の rich text の欄は空の行に U+FEFF・U+200B を置き、末尾に改行を足す事がある）。
fn field_text(v: &str) -> String {
    let t: String = v.chars().filter(|c| !matches!(c, '\u{feff}' | '\u{200b}')).collect();
    t.trim_end_matches(['\n', '\r']).to_string()
}

/// bridge の失敗 → `GuiError`（読む口 `read` と menu の読み用。操作の結果は `gui_result`）。
/// `not_trusted` → Denied・`unknown_handle` / `stale_element` / `invalid_element` / `no_such_process` / `menu_not_found` / `NotFound` → NotFound・
/// `Timeout` → Timeout{ms}・`Gone` / `Spawn` / 壊れた応答 → Bridge・それ以外（`bad_args`・知らない code・`Version` 等）→ Unsupported。
/// 保証: 文言は bridge の message を 1 行で含む（理由を捨てない）。
/// Wave: W17-1
pub fn gui_error(e: &BridgeError) -> GuiError {
    use AxErrorCode as C;
    match e {
        BridgeError::Failed { code, message } => match AxErrorCode::from_code(code) {
            Some(C::NotTrusted) => GuiError::Denied(message.clone()),
            Some(
                C::UnknownHandle
                | C::StaleElement
                | C::InvalidElement
                | C::NoSuchProcess
                | C::MenuNotFound,
            ) => GuiError::NotFound(message.clone()),
            _ => GuiError::Unsupported(format!("{message} ({code})")),
        },
        BridgeError::NotFound(s) => GuiError::NotFound(s.clone()),
        BridgeError::Timeout { ms, .. } => GuiError::Timeout { ms: *ms },
        BridgeError::Gone(_) | BridgeError::Spawn(_) | BridgeError::Broken(_) => {
            GuiError::Bridge(e.to_string())
        }
    }
}

/// invoke の規則（旧 `cua::Driver::run_desktop_outcome` と同じ）:
/// 押す前に `elements` で読み直す・保存した app_name と違えば拒否・label + role が 0 件 = Failed{TargetNotFound}（途中までの読みなら Rejected）・複数 = Rejected・
/// app_name の無い binding は押さない・送った後の失敗は EffectUnknown。
impl Adapter for MacGui {
    fn id(&self) -> &'static str {
        "mac_gui"
    }
    /// #12: `AdapterKind::MacGui`（保存は `"mac_gui"`・古い `"cua"` も読める）
    fn kind(&self) -> AdapterKind {
        AdapterKind::MacGui
    }
    fn route(&self) -> Route {
        Route::OsAutomation
    }
    fn provider(&self) -> &'static str {
        "desktop"
    }
    /// AX の能力（窓の一覧・読み・click・type_text・menu・key）はどの app の窓にも使う = Shared（"desktop" は bundle id でも origin でもない）
    fn records(&self) -> Vec<CapabilityRecord> {
        official_records(self, Reuse::Shared)
    }
    fn set_progress(&mut self, progress: Progress) {
        self.progress = Some(progress);
    }
    /// ENGINE-ADJUST-1102 ①: 人の Stop の印（`read` の more の前・`patiently`・`op` の送る前と lease の待ちで見る）
    fn set_stop(&mut self, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.stop = Some(stop);
    }
    fn specs(&self) -> &'static [Spec] {
        SPECS
    }
    fn readiness(&self, _action: &str) -> CapabilityReadiness {
        self.sync.capability(true)
    }
    fn sources(&self) -> Vec<Source> {
        ["desktop.windows", "desktop.window"]
            .iter()
            .map(|id| source(id, AdapterKind::MacGui, "desktop", &self.sync, true))
            .collect()
    }
    /// bridge の hello（version・trusted）。bridge が無い・許可が無い時は missing（ADR-003 C1 / C11）と、その理由の 1 行（`not_ready_line`）
    fn check(&mut self) {
        let t = now();
        let hello = self.call("hello", json!({}));
        self.not_ready = match &hello {
            Ok(v) if v["trusted"].as_bool() == Some(true) => None,
            Ok(_) => Some(UNTRUSTED_LINE),
            Err(_) => Some(NO_BRIDGE_LINE),
        };
        match hello {
            Ok(v) if v["trusted"].as_bool() == Some(true) => {
                let version = v["version"].as_str().unwrap_or("?");
                self.sync.record(
                    t,
                    &Outcome::Verified {
                        evidence: vec![format!(
                            "Screen control {version} · Accessibility is on"
                        )],
                    },
                    None,
                )
            }
            _ => self.sync.missing(t),
        }
    }
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        // W19-3b: 保存した手順の宛先（pid・窓番号は起動ごとに変わる）を、input の bundle id と文書の題名で今の窓に決め直す（読む前・送る前）。
        // 決められない（窓が 0 個・2 個以上・許可が無い）時は何も送らず 1 行（act の記録 0 件）
        let moved;
        let input = match self.retargeted(action, input) {
            Ok(Some(v)) => {
                moved = v;
                &moved
            }
            Ok(None) => input,
            Err(reason) => return (Outcome::Rejected { reason }, None),
        };
        self.within = serde_json::from_value(input["within"].clone()).ok();
        self.pinned = pinned_of(input);
        let t = now();
        let int = |k: &str| input[k].as_i64().unwrap_or(0);
        let text = |k: &str| input[k].as_str().unwrap_or("").to_string();
        let (outcome, observation) = match action {
            // 入力の pid と bundle_id は任意: その app の窓だけ（`engine_page` の同意の宛先を引く・W19-1 の残り・W19-3b）。main = true（bundle_id と）は主窓 1 つ（M2 G-i）
            "desktop.windows.list" => {
                let bundle = input["bundle_id"].as_str();
                let document = input["document"].as_str().filter(|d| !d.trim().is_empty());
                let main = |l: Listed| match (input["main"].as_bool(), bundle) {
                    // 窓 0 個を bundle id の app 名で retarget すると、理由の “” の外に com.apple.TextEdit が出て Page は定型の Ready になる（w19 の承認の前）。
                    (Some(true), Some(_)) if l.windows.is_empty() && l.unanswered == 0 && l.unsettled == 0 => Ok(Vec::new()),
                    (Some(true), Some(b)) => {
                        let app = l.windows.first().map_or(b, |w| w.app_name.as_str());
                        // G73: 書類の題名の窓（`gui::retarget` と同じ一致）で決める（1 つ = それ・2 つ以上 = retarget の 1 行で止める）。
                        // G75: 窓が在って名指しの書類が無い時だけ止める（別の書類で代用しない）。窓が 0 個は上の arm。
                        // 役割（G31b）は名指しが無い時（scene 2 の REAPER・Discord・Zoom の席の Page = 書類が無い）だけ
                        match document {
                            Some(d) => crate::gui::retarget(b, app, d, input["document_file"].as_str().filter(|f| !f.trim().is_empty()), &l.windows)
                                .map(|t| l.windows.iter().filter(|r| r.pid == t.pid && t.window_id.map(i64::from) == Some(r.window_id)).cloned().collect())
                                .map_err(|e| unresolved(&l, app, &format!("the window with “{d}”"), e)),
                            None if !l.infos.is_empty() => pick_by_role(&l.infos, app).map(|w| l.windows.iter().filter(|r| r.pid == w.pid && Some(r.window_id) == w.cg_id).cloned().collect()),
                            None => Ok(l.windows),
                        }
                    }
                    _ => Ok(l.windows),
                };
                match self.windows_where(&|p, b| input["pid"].as_u64().is_none_or(|want| want == p) && bundle.is_none_or(|want| want == b), input["pid"].is_u64() || bundle.is_some()).and_then(main) {
                    Ok(ws) => read_result(Ok(json!(ws)), "desktop.windows", "window_list", t),
                    Err(reason) => (Outcome::Rejected { reason }, None),
                }
            }
            // G70: `app_of` と同じ `apps`（regular_only = false）を入力の pid / bundle_id で絞る（AX を読まない）
            "desktop.apps.list" => match self.call("apps", json!({ "regular_only": false })) {
                Ok(v) => {
                    let apps: Vec<AppIdentity> = v["apps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|a| input["pid"].as_u64().is_none_or(|p| a["pid"].as_u64() == Some(p)) && input["bundle_id"].as_str().is_none_or(|b| a["bundle_id"] == b))
                        .map(|a| AppIdentity {
                            bundle_id: a["bundle_id"].as_str().unwrap_or("").to_string(),
                            name: a["name"].as_str().unwrap_or("").to_string(),
                            pid: a["pid"].as_u64().and_then(|p| u32::try_from(p).ok()),
                            ..Default::default()
                        })
                        .collect();
                    read_result(Ok(json!(apps)), "desktop.windows", "apps", t)
                }
                Err(e) => (Outcome::Rejected { reason: gui_error(&e).to_string() }, None),
            },
            // 出力は `gui::WindowState`（R-G2 の契約）。② の ref は `Control::element` で返す（operator 2026-10-23・その読みの間だけ有効・押す側は label + role で読み直す）。
            // 入力の app_name は窓を決められない時の 1 行（handle_for）にだけ使う（照合しない）
            "desktop.window.state" => {
                let w = WindowRef {
                    pid: int("pid") as u32,
                    window_id: int("window_id"),
                    app_name: text("app_name"),
                    title: String::new(),
                    bundle_id: String::new(),
                    document: String::new(),
                };
                // G74b: role・subrole・contains で絞る読みも窓の全部（owner 256: `read` は途中までの読みを返さない = 消えた・減ったを途中の読みで言わない）
                // G74c: menu が開いている間は窓の残りが木から消える（Discord 15:0x: 582 → 75 要素・行 0）ので読まない（Rejected = 消えた・減ったと言わない）
                // FX-114: この起動で書く手を送った app が今は隠れている（⌘H）= 隠れた app への入力は後で反映されうるので、窓の読みで済み・消えたを言わない（再実行もしない）
                if self.sent_to.contains(&w.pid) {
                    if let Ok((app, _, true)) = self.app_of(w.pid) {
                        return (Outcome::Unverified { reason: format!("{app} is hidden now, so whether the step took effect can't be read from its window. It won't be sent again — show {app}'s window and look") }, None);
                    }
                }
                let (role, subrole, has) = (text("role"), text("subrole"), text("contains"));
                let read = self.elements(&w);
                // ENGINE-AUDIT W7: role で絞る読みの contains は名前（`name_in` = label の無い一覧の行は子の字・AI と Engine の `click_checks` が見る名前）でも当てる。
                // role の無い読み（G75 の置いた字の読み戻し）は今まで通り label と値だけ
                let keep = |e: &Element, name: &str| {
                    let named = e.label.contains(&has) || (!role.is_empty() && name.contains(&has));
                    (role.is_empty() || e.role == role) && (subrole.is_empty() || e.subrole == subrole) && (has.is_empty() || named || e.value.as_deref().is_some_and(|v| v.contains(&has)))
                };
                match read {
                    Ok(els) if els.open_menu().is_some() => (Outcome::Rejected { reason: format!("A menu is open in {} and hides the rest of its window, so it isn't read now. Close the menu, then ask again", els.app_name) }, None),
                    Ok(els) => {
                        // OPEN-INPLACE-1101 の 3: label の無い一覧の行は子の字を名前に（`Elements::name_of`・AI が選ぶ名前と押す時に比べる名前が同じ）
                        let (names, paths): (Vec<String>, Vec<Vec<String>>) = {
                            let by = els.by_ref();
                            let names: Vec<String> = els.items.iter().map(|e| els.name_in(&by, e)).collect();
                            // ENGINE-AUDIT W3: 道 = 名前の在る祖先（近い順・3 つまで・窓そのものは入れない）。同じ名前の 2 つを道で見分ける
                            let paths: Vec<Vec<String>> = els
                                .items
                                .iter()
                                .map(|e| ancestors(&by, e).filter(|a| a.depth > 0).map(|a| els.name_in(&by, a)).filter(|n| !n.trim().is_empty()).take(3).collect())
                                .collect();
                            (names, paths)
                        };
                        let state = WindowState {
                            pid: w.pid,
                            window_id: w.window_id,
                            app_name: els.app_name,
                            complete: true,
                            controls: els
                                .items
                                .into_iter()
                                .zip(names)
                                .zip(paths)
                                .skip(1)
                                .filter(|((e, name), _)| keep(e, name.as_str()))
                                .map(|((e, label), path)| Control {
                                    element: Some(e.r),
                                    label,
                                    role: e.role,
                                    value: e.value,
                                    actions: e.actions,
                                    value_settable: e.value_settable,
                                    path,
                                    selected: e.selected,
                                    expanded: e.expanded,
                                })
                                .collect(),
                        };
                        read_result(Ok(json!(state)), "desktop.window", "ax_tree", t)
                    }
                    Err(reason) => (Outcome::Rejected { reason }, None),
                }
            }
            // ENGINE-AUDIT W4: app の口の棚（bundle を読むだけ・起こさない・承認も同意も要らない・bundle@版で溜める）。`path` は AI に渡さない（呼ぶ側の `AppSurfaces::lines`）
            "desktop.app.surfaces" => match app_surfaces(input["bundle_id"].as_str(), input["app_name"].as_str()) {
                Ok(s) => read_result(Ok(json!(s)), "desktop.app", "surfaces", t),
                Err(reason) => (Outcome::Rejected { reason }, None),
            },
            // ENGINE-AUDIT W3: menu bar を読むだけ（pid か bundle_id の起動中の app・承認も AI の同意も要らない・押さない・前面を変えない）
            "desktop.menu.read" => {
                let pid = match (input["pid"].as_u64(), input["bundle_id"].as_str().filter(|b| !b.trim().is_empty())) {
                    (Some(p), _) => u32::try_from(p).ok(),
                    (None, Some(b)) => self
                        .call("apps", json!({ "regular_only": false }))
                        .ok()
                        .and_then(|v| v["apps"].as_array().and_then(|a| a.iter().find(|x| x["bundle_id"] == b).and_then(|x| x["pid"].as_u64())))
                        .and_then(|p| u32::try_from(p).ok()),
                    _ => None,
                };
                let Some(pid) = pid else { return (Outcome::Rejected { reason: "Name the app (its pid or bundle id) to read its menus".into() }, None) };
                match self.call(CMD_AX_MENU_TREE, json!(AxMenuTreeReq { pid })) {
                    Ok(v) => match serde_json::from_value::<AxMenuTreeReply>(v) {
                        Ok(r) => read_result(Ok(json!(r)), "desktop.menu", "ax_menu_tree", t),
                        Err(e) => (Outcome::Rejected { reason: format!("ax_menu_tree の応答の形が違います: {e}") }, None),
                    },
                    Err(e) => (Outcome::Rejected { reason: gui_error(&e).to_string() }, None),
                }
            }
            _ => {
                let app = text("app_name");
                if app.trim().is_empty() {
                    // W17-2 より前に登録した旧 binding（app 名を持たない）は、pid / 窓番号が別の app に使い回されても気付けないので押さない（W19-1・operator 2026-10-23）
                    return (Outcome::Rejected { reason: OLD_BINDING_LINE.into() }, None);
                }
                // ENGINE-AUDIT W8a: 書く手は窓を読む前に隠れを解く（同意が在る app だけ・前面は取らない）→ 下の読みがそのまま読み直し。読めない時は `op` が同じ 1 行で断る
                if self.prechecks(action) {
                    let _ = self.shown_for_act(int("pid") as u32);
                }
                let o = match action {
                    "desktop.ax.click" | REVEAL | "desktop.ax.type_text" => {
                        let tool = if action == "desktop.ax.type_text" { super::super::model::DesktopTool::TypeText } else { super::super::model::DesktopTool::Click };
                        // FX-115: 字を置く窓の題名（Discord = channel と server）を置く前に覚える。送る直前の `recheck` が同じ題名かを見る
                        let key = (int("pid") as u32, int("window_id"));
                        let before = (action == "desktop.ax.type_text").then(|| self.title_of(key.0, key.1, &app));
                        let o = self.run_desktop_matching(&ActionKind::Desktop { pid: int("pid"), window_id: int("window_id"), label: text("label"), role: text("role"), tool, text: text("text"), app_name: app }, input, action == REVEAL);
                        if let (Some(t), Outcome::Verified { .. } | Outcome::Unverified { .. }) = (before, &o) {
                            self.put_titles.insert(key, t);
                        }
                        o
                    }
                    // menu と key は窓を持たないので、pid の app 名を読んで保存値と照合してから送る
                    "desktop.ax.menu" | "desktop.key" => match self.app_of(int("pid") as u32) {
                        Err(e) => Outcome::Rejected { reason: e.to_string() },
                        Ok((now_app, ..)) if now_app != app => Outcome::Rejected { reason: format!("The target app may have changed (saved: {app} · now: {now_app}). Add the step again") },
                        Ok(_) if action == "desktop.ax.menu" => {
                            let path: Vec<String> = input["path"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)).collect();
                            self.menu(int("pid") as u32, &path.iter().map(String::as_str).collect::<Vec<_>>()).into()
                        }
                        Ok(_) => {
                            let mods: Vec<&str> = input["mods"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                            let keys = mods.iter().copied().chain([input["key"].as_str().unwrap_or("")]).collect::<Vec<_>>().join("+");
                            match keys.parse::<Chord>() {
                                Ok(c) => self.key(int("pid") as u32, &c).into(),
                                Err(e) => Outcome::Rejected { reason: e.to_string() },
                            }
                        }
                    },
                    OPEN_DOCUMENT => self.open_document(input),
                    other => Outcome::Rejected { reason: format!("Unknown action: {other}") },
                };
                (o, None)
            }
        };
        (self.within, self.pinned) = (None, None);
        self.sync.record(t, &outcome, observation.as_ref());
        (outcome, observation)
    }
    /// G74 ③: 送る直前の再確認。`desktop.ax.type_text` だけ: 宛先を invoke と同じに決め直し（bundle_id と document）、窓を読み直して欄が今も打った文か（送らない）
    fn recheck(&mut self, action: &str, input: &Value) -> Option<Outcome> {
        if action != "desktop.ax.type_text" {
            return None;
        }
        let moved;
        let input = match self.retargeted(action, input) {
            Ok(Some(v)) => {
                moved = v;
                &moved
            }
            Ok(None) => input,
            Err(reason) => return Some(Outcome::Unverified { reason }),
        };
        let s = |k: &str| input[k].as_str().unwrap_or("");
        let Some(pid) = input["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
            return Some(Outcome::Unverified { reason: "The target window isn't valid any more".into() });
        };
        let w = WindowRef { pid, window_id: input["window_id"].as_i64().unwrap_or(0), app_name: s("app_name").into(), title: String::new(), bundle_id: String::new(), document: String::new() };
        self.within = serde_json::from_value(input["within"].clone()).ok();
        self.pinned = pinned_of(input);
        let held = self.field_holds(&w, s("label"), s("role"), s("text"), input["replace"].as_str());
        (self.within, self.pinned) = (None, None);
        // FX-115: 宛先も読み直す = 字を置いた時の窓の題名（channel・server・書類）と今の題名が違えば送らない（題名は 1 行に書かない = channel の名前を出さない）
        let put = self.put_titles.get(&(pid, w.window_id)).cloned();
        let place = same_place_now(put, || self.title_of(pid, w.window_id, &w.app_name));
        Some(match held.and_then(|again| place.map(|()| again)) {
            Ok(again) => Outcome::Verified { evidence: vec![format!("Read the field of {} again right before the next step: {again}, and the window still shows the same place", w.app_name)] },
            Err(why) => Outcome::Unverified { reason: format!("Read the field of {} again right before the next step: {why}", w.app_name) },
        })
    }
    /// WF-006: 4 つの書く手は送る直前に宛先を読み直す = `op` が pid から bridge の `apps` で app を読み直し（同意の無い app・隠れたままの app への AXShowMenu は送らない。隠れは読む前に `shown_for_act` が解く・W8a）、
    /// bundle_id と document が在れば `retargeted` が今の窓に決め直す（0 個・2 個以上・主窓でない書類は送らない）
    fn prechecks(&self, action: &str) -> bool {
        matches!(action, "desktop.ax.click" | REVEAL | "desktop.ax.type_text" | "desktop.ax.menu" | "desktop.key")
    }
    /// 最後の `check` で分かった、Ready でない理由の 1 行（bridge が無い / 許可が無い）。registry の invoke と Connections が同じ文を出す
    fn not_ready_line(&self) -> Option<&'static str> {
        self.not_ready
    }
    fn cleanup(&mut self) {
        self.bridge = None;
        self.sync = SourceSync::default();
        self.seen.clear();
        self.put_titles.clear();
        self.not_ready = None;
        self.bundles.clear();
        self.stop = None;
    }
}

// FX-115・CHAT-013 の口（Adapter の trait の外 = MacGui だけの手）
impl MacGui {
    /// FX-115: 窓 (pid, 窓番号) の今の題名（bridge の window_list・読めない・窓が無い = None）。
    fn title_of(&mut self, pid: u32, window_id: i64, app: &str) -> Option<String> {
        let (list, _) = self.window_list(u64::from(pid), app).ok()?;
        parse_windows(&list).into_iter().find(|x| x.pid == pid && x.cg_id == Some(window_id)).map(|x| x.title)
    }
    /// CHAT-013: Project の repo の書類（`document_file`）を bundle の app で開き（bridge の `open_document`・前面を取らない頼み）、その書類の窓が
    /// `retarget` で 1 つに決まるまで読み直す（`OPEN_WAIT`）。在る file しか開かない・作らない・書き換えない。
    fn open_document(&mut self, input: &Value) -> Outcome {
        let s = |k: &str| input[k].as_str().unwrap_or("").trim().to_string();
        let (bundle, app, doc, file) = (s("bundle_id"), s("app_name"), s("document"), s("document_file"));
        let path = crate::gui::file_path(&file);
        if bundle.is_empty() || doc.is_empty() || !path.is_file() {
            return Outcome::Rejected { reason: format!("“{doc}” isn't in this project's folder any more, so nothing was opened") };
        }
        if let Err(e) = self.call("open_document", json!({ "bundle_id": bundle, "path": path, "activate": false })) {
            return Outcome::Rejected { reason: format!("{app} couldn't open “{doc}” ({}). Nothing was changed", gui_error(&e)) };
        }
        let t = std::time::Instant::now();
        loop {
            if let Ok(l) = self.windows_where(&|_, b| b == bundle, false) {
                if retarget(&bundle, &app, &doc, Some(&file), &l.windows).is_ok() {
                    return Outcome::Verified { evidence: vec![format!("Opened this project's “{doc}” in {app} (its window is there now)")] };
                }
            }
            if t.elapsed() >= OPEN_WAIT {
                return Outcome::Unverified { reason: format!("Asked {app} to open “{doc}”, but its window didn't appear within {} s", OPEN_WAIT.as_secs()) };
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}

/// 反証 G-A の直し（FX-115）: 送る直前の宛先の確かめは、置く前に題名を読めなかった時も・今読めない時も・違う時も送らない。
/// 負の対照: `same_place_now` の Some(None) の腕を Ok(()) にする → 1 つ目が赤。
#[cfg(test)]
mod g_a_fix {
    use super::{same_place, same_place_now};

    #[test]
    fn g_a_fx115_an_unreadable_title_before_the_text_was_put_does_not_send() {
        let t = |s: &str| Some(s.to_string());
        assert!(same_place_now(Some(None), || t("#general | Test Server - Discord")).is_err(), "置く前に読めなかったのに送った");
        assert!(same_place_now(Some(t("#general | Test Server - Discord")), || None).is_err(), "今読めないのに送った");
        assert!(same_place_now(Some(t("#general | Test Server - Discord")), || t("#random | Test Server - Discord")).is_err());
        assert!(same_place_now(Some(t("README.md")), || t("README.md — Edited")).is_ok(), "書類の状態の後ろ書きは同じ場所");
        assert!(same_place_now(None, || None).is_ok(), "字を置いていない窓は確かめない");
        assert!(!same_place("#general | Shibubu — Prod - Discord", "#general | Shibubu — Dev - Discord"), "server 名の em dash");
    }
}

#[cfg(test)]
mod rq6_tests {
    use super::*;

    fn window(label: &str) -> Elements {
        let el = |depth: u32, label: &str| Element { r: ElementRef("e1.0".into()), role: "AXWindow".into(), subrole: String::new(), label: label.into(), value: None, actions: vec![], value_settable: false, enabled: true, selected: None, expanded: None, depth, parent: None };
        Elements { app_name: "TextEdit".into(), snapshot: 1, items: vec![el(0, label), el(1, "Edited notes")] }
    }

    /// RQ6（RQ-024）: 窓の題の終わりの「編集済み」だけが人の未保存の変更。書類の名前の中の Edited・題が無い窓は数えない。
    /// 負の対照: `title_says_edited` を `label.contains("Edited")` に替える → 「Edited notes.md」で赤。
    /// RQ6（RQ-025）: secure text の値だけが落ち（名前・役割は残る）、普通の欄の値は残る。password manager の bundle id は読まない一覧に在る。
    /// 負の対照: `hide_secrets` の filter を外す（全部の欄の値を落とす）→ 普通の欄の値が消えて赤・中身を空にする → secure の値が残って赤。
    #[test]
    fn rq6_secure_text_values_are_dropped_and_password_managers_are_never_read() {
        let el = |sub: &str, value: &str| Element { r: ElementRef("e1.1".into()), role: "AXTextField".into(), subrole: sub.into(), label: "Password".into(), value: Some(value.into()), actions: vec![], value_settable: true, enabled: true, selected: None, expanded: None, depth: 1, parent: None };
        let mut items = vec![el("AXSecureTextField", "hunter2"), el("", "alice")];
        hide_secrets(&mut items, false);
        assert_eq!((items[0].value.clone(), items[0].label.as_str()), (None, "Password"), "secure の値が残った");
        assert_eq!(items[1].value.as_deref(), Some("alice"), "普通の欄の値まで落とした");
        assert!(NEVER_READ.contains(&"com.1password.1password") && !NEVER_READ.contains(&"com.apple.textedit"));
    }

    /// REVIEW-RQN（operator 5）: Shibubu 自身の窓の読みは、Settings › AI & Models の account の行（AX の名前に email）の email を伏せ、名前の残りと `@team` は残す。
    /// 他の app の窓は伏せない。負の対照: `if own` の腕を外す → email が残って赤・`own` を見ずに伏せる → 他の app の email が消えて赤。
    #[test]
    fn rqn_own_window_hides_account_emails() {
        let el = |label: &str, value: Option<&str>| Element { r: ElementRef("e1.2".into()), role: "AXButton".into(), subrole: String::new(), label: label.into(), value: value.map(str::to_owned), actions: vec![], value_settable: false, enabled: true, selected: None, expanded: None, depth: 1, parent: None };
        let rows = || vec![el("Re-authenticate Claude plan · someone@example.com", None), el("Remove Claude plan · someone@example.com", Some("someone@example.com")), el("Mention @team", None)];
        let mut own = rows();
        hide_secrets(&mut own, true);
        let all: String = own.iter().map(|e| format!("{} {:?}\n", e.label, e.value)).collect();
        assert!(!all.contains("someone@") && !all.contains("example.com"), "自分の窓の読みに email が残った: {all}");
        assert_eq!(own[0].label, "Re-authenticate Claude plan · (email hidden)");
        assert_eq!(own[2].label, "Mention @team", "email でない @ まで伏せた");
        let mut other = rows();
        hide_secrets(&mut other, false);
        assert_eq!(other[1].value.as_deref(), Some("someone@example.com"), "他の app の窓の email まで伏せた");
    }

    /// RQ6（RQ-024）: 人が持っている間は上限まで見に行き、手放せば続ける・手放さなければ上限で諦める（無期限に止めない）。
    /// 負の対照: 上限の比較を外す（`waited >= max` を消す）→ 持ち続ける人で止まらず赤（試験が終わらない前に `calls` の上限で panic）・先に 1 回も見ない → 手放した人でも待ちが 1 回増えて赤。
    #[test]
    fn rq6_waits_for_the_human_up_to_a_limit_and_then_gives_up() {
        use std::time::Duration;
        let step = Duration::from_millis(500);
        let mut calls = 0;
        // 3 回目に手放す人: 2 回待って続ける
        let (waited, gave_up) = wait_while_human_holds(|| { calls += 1; calls < 3 }, Duration::from_secs(10), step, |_| {});
        assert_eq!((waited, gave_up), (step * 2, false));
        // 手放さない人: 上限（2 秒 = 4 回待つ）で諦める
        let mut calls = 0;
        let (waited, gave_up) = wait_while_human_holds(|| { calls += 1; assert!(calls < 100, "上限で止まらない"); true }, Duration::from_secs(2), step, |_| {});
        assert_eq!((waited, gave_up), (Duration::from_secs(2), true));
        // 最初から手放している人は待たない
        let (waited, gave_up) = wait_while_human_holds(|| false, Duration::from_secs(10), step, |_| panic!("待った"));
        assert_eq!((waited, gave_up), (Duration::ZERO, false));
    }

    /// RQ6 反証の直し（rq2）: 題が編集済みでも、今の本文が Shibubu の置いた直後のままなら人は触っていない。人が続けて打った（本文が違う）・置いた覚えが無いなら人の未保存。
    /// 負の対照: `mine != Some(now)` を `mine.is_none()` にする → 人が続けて打った本文で false になって赤。
    #[test]
    fn rq6_a_window_the_ai_wrote_is_protected_again_once_the_human_types_more() {
        assert!(!human_unsaved(false, None, "x"), "題が編集済みでない");
        assert!(human_unsaved(true, None, "x"), "置いた覚えの無い編集済み");
        assert!(!human_unsaved(true, Some("x"), "x"), "置いた直後のまま");
        assert!(human_unsaved(true, Some("x"), "x and more"), "人が続けて打った");
    }

    /// RQ6 反証 rqa1 + 待ちの本体: 実際に寝て待つ loop を短い上限で走らせる。人が途中で離れれば上限の前に進み（待った時間 > 0）、居続ければ上限で諦める。
    /// どちらも終わった後は帯の「待っている」が消えている。負の対照: `wait_while_human_holds` の `while` を `if false` にする → 1 本目の待った時間が 0 で赤・
    /// `clear_activity()` を外す → 2 本とも Waiting が残って赤。
    #[test]
    fn rq6_the_wait_really_waits_then_goes_on_or_gives_up_and_the_band_stops_saying_it() {
        use std::time::{Duration, Instant};
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(()); // ACTIVITY は process で 1 つ
        let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let step = Duration::from_millis(20);
        // 人が 100ms 後に離れる
        let t0 = Instant::now();
        let (waited, gave_up) = wait_for_human("TextEdit", || t0.elapsed() < Duration::from_millis(100), Duration::from_secs(5), step);
        assert!(!gave_up && waited >= Duration::from_millis(60) && t0.elapsed() < Duration::from_secs(2), "{waited:?} {gave_up}");
        assert_eq!(activity_line(), None, "続ける時に待っていると言い続けた");
        // 人が居続ける: 200ms で諦める
        let t0 = Instant::now();
        let (waited, gave_up) = wait_for_human("TextEdit", || true, Duration::from_millis(200), step);
        assert!(gave_up && waited >= Duration::from_millis(200) && t0.elapsed() < Duration::from_secs(3), "{waited:?} {gave_up}");
        assert_eq!(activity_line(), None, "諦めた後も待っていると言い続けた");
    }

    #[test]
    fn rq6_only_a_trailing_edited_mark_in_the_window_title_means_unsaved() {
        assert!(title_says_edited(&window("README.md — Edited")));
        assert!(title_says_edited(&window("名称未設定 — 編集済み")));
        assert!(!title_says_edited(&window("README.md")));
        assert!(!title_says_edited(&window("Edited notes.md")));
        for t in ["Notes — Bearbeitet", "Notes — Modifié", "Notes — Editado", "Notes - Modificato"] {
            assert!(title_says_edited(&window(t)), "{t}");
        }
        assert!(!title_says_edited(&Elements { items: vec![], ..window("") }));
    }
}
