//! AI の GUI 操作の契約の型（`GuiTarget`・`GuiAction`・`ActReceipt`・`GuiError`・`WindowState`）。W6b で W2c の `AxTree` / `Expect` / `Verdict`（observe / act / verify）は消した。口（observe / act / verify）は
//! `adapters::mac_gui::MacGui` の inherent method（W17-2 で bridge の `ax_tree`・`ax_press`・`ax_set_value`・`ax_menu`・`key_to_pid` に。
//! 実装 1 つ・`dyn` の呼び元 0 なので R-gui で trait `GuiAdapter` を畳んだ）。bridge の JSON の型は mac_gui に置く（契約 = ここ・bridge の口 = mac_gui）。
//! 人の直接操作（Native Direct・`surface::MirrorSurface::relay_human`）とは型で分ける: ここの `GuiAction` から Mirror の入力は作れず、
//! Mirror の `MirrorInput`（feature `mirror`）からここの操作も作れない（2・3 通目）。座標の click・cursor の移動・前面化は口に無い（AX の element だけ）。
//! serde / std だけに依る（surface・cua を引かない）。図は docs/diagrams.md の「29. Surface・読み戻し・GUI 操作の契約」。tests/adapters.rs が `#[path]` で取り込む。
#![allow(dead_code)] // W17-2 で配線するまで main からの呼び出しが無い

use serde::{Deserialize, Serialize};
use std::fmt;

/// 操作する相手の窓。pid と窓 id は `observe` の前に呼ぶ側が bridge の `window_list` で決める（保存しない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiTarget {
    pub bundle_id: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,
}

/// `observe` が返す AX の要素の token（その観測の間だけ有効。古い token の操作は `GuiError::NotFound`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ElementRef(pub String);

/// 宛先の窓。window_id は CG の窓番号（旧 `ActionKind::Desktop` と同じ）。app_name は登録時に保存した値で、押す前に照合する。
/// title は表示だけ（旧 Desktop パネルの一覧の 1 行。照合に使わない・画面収録の許可が無いと空）。
/// R-G2: `adapters::mac_gui` から移した（registry の `desktop.windows.list` の payload `[WindowRef]` の形 = Core と Adapter の契約）。
/// mac_gui は `pub use` で同じ型を使う（型は 1 つ）。
/// 使い方: `let ws: Vec<WindowRef> = serde_json::from_value(obs.payload)?;`（`AdapterRegistry::invoke(.., "desktop.windows.list", ..)` の Observation）
/// 保証しない: 窓が今も在る事（次の読み・操作が Err / `Unsupported` で知らせる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRef {
    pub pid: u32,
    pub window_id: i64,
    pub app_name: String,
    #[serde(default)]
    pub title: String,
    /// bridge の `apps` で読んだ bundle id（AI の操作の app ごとの同意の鍵・W19-1）。空 = 読めなかった（同意の button を出さない）
    #[serde(default)]
    pub bundle_id: String,
    /// FX-111: 窓の書類の file（bridge の AXDocument = `file:///…` の URL）。空 = 読めない（書類の無い窓・前の bridge・AX が答えない）。
    /// 同じ題名の窓が 2 つ以上でも、これで Project の repo の書類の窓を 1 つに決める（`retarget` の `file`）
    /// 空（書類を読めない窓・前の bridge）は書き出さない（model に渡す窓の形を前と同じにする）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub document: String,
}

/// registry の `desktop.window.state` の payload（R-G2 で決めた保存しない JSON の契約）。作るのは `adapters::mac_gui` の invoke で、読むのは
/// 旧 Desktop パネルの ②（`app.rs` の `desktop_controls`・W19-1 で registry に寄せた）と `tests/adapters.rs`。
/// 使い方: `let s: WindowState = serde_json::from_value(obs.payload)?; s.controls.iter().filter(|c| !c.label.trim().is_empty() && (!c.actions.is_empty() || c.value_settable))`
/// 保証: controls は窓そのもの（ax_tree の先頭）を含まない。W19-1 の本体（operator 2026-10-23「② の ref は payload で返す」）から各 control は AX の ref を
/// `Control::element` に載せる（読んだ snapshot の間だけ有効・押す側は label + role で読み直す）。complete = false は途中までの読み（一覧に無い事を
/// 「無い」と読まない）。owner 256 から `MacGui` は窓を全部読み終えた時だけ返す（数でも時間でも切らない）ので true。false は前の記録・他の作り手の物。
/// 保証しない: 呼んだ後も窓と要素が同じ事・ref が次の読みでも同じ事。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub pid: u32,
    pub window_id: i64,
    /// 今の app 名（bridge の `apps` で読んだ値。保存した値を返さない）
    pub app_name: String,
    pub complete: bool,
    pub controls: Vec<Control>,
}

/// `WindowState` の要素 1 つ（label / role / value と、押せるか・値を置けるか）。ref を持たないので、押す側は label + role で読み直して探す。
/// R-G2: `actions` と `value_settable` を足した（前の payload は label / role / value だけ）。足した 2 つが無い古い payload も読める（serde の default）。
/// W19-1: `element`（AX の ref）を足した。無い payload（R-G2 の物）も読め、None は書かない（R-G2 の payload と同じ JSON）。
/// Wave: R-G2, W19-1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// その読みの AX の ref（`e<snapshot>.<n>`・旧 Desktop パネルの ② が見せる）。次の読みで古くなるので押す・保存の鍵にしない。
    /// `adapters::mac_gui` は W19-1 の本体から全部の要素に入れる（None は R-G2 の payload を読んだ時だけ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementRef>,
    pub label: String,
    pub role: String,
    #[serde(default)]
    pub value: Option<String>,
    /// AX の action 名（`"AXPress"` 等）
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub value_settable: bool,
    /// ENGINE-AUDIT W3（節 3「AI に見せる窓が狭く平たい」）: 道 = 名前の在る祖先の名前（近い順・3 つまで・`adapters::mac_gui` が `Element::parent` から作る）。
    /// 同じ名前の 2 つを道で見分ける（「Guitar · in Library › Sounds」）。前の payload（無い）は空
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// W3: AXSelected（行・tab・一覧の項目）。None = その要素に無い・前の bridge
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// W3: AXExpanded（disclosure・tree の行）。None = 無い・前の bridge
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
}

impl WindowState {
    /// ENGINE-AUDIT W3（owner 256 の 2「読みは全部・AI に見せる量は絞る」・節 3）: AI に渡す行。`words`（一言・操作の want・状況の語 = `words_of`）を名前か値に含む行を
    /// 全部先頭に（当たり）、残りを木の順に `budget_chars` まで（道〈祖先の名前〉だけが語に当たる行を先に）、入らなかった分は最後の 1 行「… and N more (AXRow 120 · AXButton 30)」
    /// （AI は一覧が途中と知る）。行の形 = `role: 名前 · in 道 › 道 · selected · press: AXPress · ×N`（同じ役割と名前の数）。同じ行は 1 回。数の上限は無い（字の予算だけ）。
    /// r8（operator 11-03・chars_controls 20,856 / shown 221 / 382 要素）: 道で当たる行まで予算の外にすると、語に当たる祖先（Library・Sounds）の下の行が全部当たりになり
    /// 予算が効かなかった。道だけの当たりは予算の中（祖先そのものは名前で当たるので先頭に残る）。
    /// 例: `state.lines(&words_of(&[goal, &op.want]), 12_000)`
    pub fn lines(&self, words: &[String], budget_chars: usize) -> Vec<String> {
        // 同じ役割と名前の数（道で見分ける）
        let mut same: std::collections::HashMap<(&str, &str), usize> = std::collections::HashMap::new();
        for c in &self.controls {
            *same.entry((c.role.as_str(), c.label.trim())).or_insert(0) += 1;
        }
        let line = |c: &Control| {
            let name: String = c.label.trim().chars().take(NAME_CHARS).collect();
            let mut s = format!("{}: {name}", c.role);
            if let Some(v) = c.value.as_deref().map(str::trim).filter(|v| !v.is_empty() && *v != name) {
                s.push_str(&format!(" = {}", v.chars().take(NAME_CHARS).collect::<String>()));
            }
            if !c.path.is_empty() {
                s.push_str(&format!(" · in {}", c.path.join(" › ")));
            }
            if c.selected == Some(true) {
                s.push_str(" · selected");
            }
            if c.expanded == Some(true) {
                s.push_str(" · expanded");
            }
            if !c.actions.is_empty() {
                s.push_str(&format!(" · press: {}", c.actions.join(" ")));
            } else if matches!(c.role.as_str(), "AXRow" | "AXCell") {
                s.push_str(" · select");
            } else if c.value_settable {
                s.push_str(" · type");
            }
            if same.get(&(c.role.as_str(), c.label.trim())).copied().unwrap_or(1) > 1 {
                s.push_str(&format!(" · ×{}", same[&(c.role.as_str(), c.label.trim())]));
            }
            s
        };
        let shown = |c: &Control| !c.label.trim().is_empty() || c.value.as_deref().is_some_and(|v| !v.trim().is_empty());
        let named = |c: &Control| {
            let hay = format!("{} {}", c.label, c.value.as_deref().unwrap_or_default()).to_lowercase();
            words.iter().any(|w| hay.contains(w.as_str()))
        };
        let mut out: Vec<String> = Vec::new();
        let mut used = 0usize;
        // ① 名前か値が語に当たる行（全部・予算を見ない）
        for c in self.controls.iter().filter(|c| shown(c) && named(c)) {
            let l = line(c);
            if !out.contains(&l) {
                used += l.chars().count();
                out.push(l);
            }
        }
        // ② 残りを予算まで: 道だけが語に当たる行（木の順）→ その他（押せる物・一覧の行・値を置ける欄・見出し・木の順）
        let mut left = 0usize;
        let mut roles: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        let rest = |by_path: bool| self.controls.iter().filter(move |c| shown(c) && !named(c) && matches(c, words) == by_path);
        for c in rest(true).chain(rest(false)) {
            let l = line(c);
            if out.contains(&l) {
                continue;
            }
            if used + l.chars().count() <= budget_chars {
                used += l.chars().count();
                out.push(l);
            } else {
                left += 1;
                *roles.entry(c.role.as_str()).or_insert(0) += 1;
            }
        }
        if left > 0 {
            let counts: Vec<String> = roles.iter().map(|(r, n)| format!("{r} {n}")).collect();
            out.push(format!("… and {left} more ({})", counts.join(" · ")));
        }
        out
    }
    /// W3: 窓の全部の名前（`role: 名前`・上限なし・同じ名前は同じ数だけ並ぶ = `unseen_label` が「ちょうど 1 つ」を数えられる）。
    /// `unseen_label`（押す名前が今の木に在るか）と再利用の前の照合はこれを見る（AI に見せる行と分ける）。
    pub fn names(&self) -> Vec<String> {
        self.controls.iter().filter(|c| !c.label.trim().is_empty()).map(|c| format!("{}: {}", c.role, c.label.trim())).collect()
    }
}

/// W3: 1 行の名前・値の字数の上限（AI に見せる行の長さ・読む数ではない）
const NAME_CHARS: usize = 80;

/// ENGINE-AUDIT W4（節 4「discover が宛先の app 自体を調べない」・owner 211 の決まり 2・252「app の file を先に見る」）: app の口の棚 = 宛先の bundle を**読むだけ**で分かる口。
/// `desktop.app.surfaces`（Adapter が bundle@版で `app-surfaces/` に溜める）の payload・generate の input `app_surfaces`（`lines`）・`CoreEngine::read` が AX の前に引く。
/// 新しい型の理由: app の disk の口（同梱の data・.strings・辞書・URL scheme・書類の型・shortcuts）を持つ型が無い。gui に置く = Core・Adapter・engine_page の全部が名指せる。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppSurfaces {
    pub bundle_id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// `.app` の場所（AI には渡さない = 利用者の home を含み得る）
    #[serde(default)]
    pub path: String,
    /// Info.plist の CFBundleURLTypes の scheme（`obsidian` 等・`<scheme>://` で開ける）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub url_schemes: Vec<String>,
    /// Info.plist の CFBundleDocumentTypes の拡張子（開ける書類の型）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_types: Vec<String>,
    /// AppleScript の辞書（sdef / scriptSuite）の suite の名前（無い = 標準の suite だけか辞書が無い）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dictionary: Vec<String>,
    /// Contents/Resources の同梱の data（拡張子ごと・名前は Resources からの相対の path の拡張子無し・全部）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceGroup>,
    /// *.strings の表の名前（UI に出る名前の元・中身は入れない）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strings: Vec<String>,
    /// `shortcuts list` の名前（この Mac の全部・app ごとではない）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcuts: Vec<String>,
    /// 読んだ時刻（RFC3339・bundle@版の溜めの印）
    #[serde(default)]
    pub read_at: String,
}

/// W4: 同梱の data の 1 群（拡張子ごと）。`count` は全部の数・`names` も全部（AI に見せる量は `AppSurfaces::lines` が絞る）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceGroup {
    pub ext: String,
    pub count: usize,
    #[serde(default)]
    pub names: Vec<String>,
}

impl AppSurfaces {
    /// W4: AI に見せる行（`WindowState::lines` と同じ決まり = 語の当たりは全部先頭・残りは字の予算まで・入らない分は「… and N more」）。
    /// 行の形: `url: obsidian://`・`opens: .md .txt`・`dictionary: Standard Suite · Text Suite`・`patch (1334): Guitar/Acoustic · Guitar/Clean …`・`strings: Localizable · LocalizablePatches`・
    /// `shortcut: Add to Notes`。当たりが無い群は数と先頭の数個だけ。
    pub fn lines(&self, words: &[String], budget_chars: usize) -> Vec<String> {
        let hit = |n: &str| {
            let l = n.to_lowercase();
            words.iter().any(|w| l.contains(w.as_str()))
        };
        let mut out: Vec<String> = Vec::new();
        for s in &self.url_schemes {
            out.push(format!("url: {s}://"));
        }
        if !self.document_types.is_empty() {
            // 拡張子は .ext・UTI（public.plain-text）はそのまま
            out.push(format!("opens: {}", self.document_types.iter().map(|e| if e.contains('.') { e.clone() } else { format!(".{e}") }).collect::<Vec<_>>().join(" ")));
        }
        if !self.dictionary.is_empty() {
            out.push(format!("dictionary: {}", self.dictionary.join(" · ")));
        }
        if !self.strings.is_empty() {
            out.push(format!("strings: {}", self.strings.join(" · ")));
        }
        // 同梱の data: 語の当たりの群を先頭に（名前は字の予算まで・contains で当たる = 短い語なら数千にもなる）、当たりの無い群は数と先頭の数個だけ（残りの予算まで）、
        // 入らない分は「… and N more」「… and N more kinds」の 1 行
        let mut left = budget_chars;
        let (with, without): (Vec<&ResourceGroup>, Vec<&ResourceGroup>) = self.resources.iter().partition(|g| g.names.iter().any(|n| hit(n)));
        for g in with {
            let mut shown: Vec<&str> = Vec::new();
            for h in g.names.iter().map(String::as_str).filter(|n| hit(n)) {
                let n = h.chars().count() + 3;
                if n > left && !shown.is_empty() {
                    break;
                }
                left = left.saturating_sub(n);
                shown.push(h);
            }
            let rest = g.count.saturating_sub(shown.len());
            out.push(format!("{} ({}): {}{}", g.ext, g.count, shown.join(" · "), if rest > 0 { format!(" … and {rest} more") } else { String::new() }));
        }
        let mut kinds_left = 0;
        for (i, g) in without.iter().enumerate() {
            if left == 0 {
                kinds_left = without.len() - i;
                break;
            }
            let head: Vec<&str> = g.names.iter().take(5).map(String::as_str).collect();
            let rest = g.count.saturating_sub(head.len());
            let line = format!("{} ({}): {}{}", g.ext, g.count, head.join(" · "), if rest > 0 { format!(" … and {rest} more") } else { String::new() });
            left = left.saturating_sub(line.chars().count());
            out.push(line);
        }
        if kinds_left > 0 {
            out.push(format!("… and {kinds_left} more kinds"));
        }
        let sh: Vec<&str> = self.shortcuts.iter().map(String::as_str).filter(|n| hit(n)).collect();
        if !sh.is_empty() {
            out.push(format!("shortcut: {}", sh.join(" · ")));
        } else if !self.shortcuts.is_empty() {
            out.push(format!("shortcuts: {} on this Mac", self.shortcuts.len()));
        }
        out
    }
    /// W4: 語に当たる同梱の data の名前（群ごと・`CoreEngine::read` が AX の前に引く = 読めれば app を開かず押す手 0）。空 = 当たり無し
    pub fn hits(&self, words: &[String]) -> Vec<(String, Vec<String>)> {
        if words.is_empty() {
            return Vec::new();
        }
        self.resources
            .iter()
            .filter_map(|g| {
                let names: Vec<String> = g
                    .names
                    .iter()
                    .filter(|n| {
                        let l = n.to_lowercase();
                        words.iter().any(|w| l.contains(w.as_str()))
                    })
                    .cloned()
                    .collect();
                (!names.is_empty()).then(|| (g.ext.clone(), names))
            })
            .collect()
    }
}

/// ENGINE-AUDIT W6（R1・spec「pressable の判定を 1 関数にする」）: 押せる形か = `action`（AXPress・AXShowMenu）を持つ要素か、一覧の行・升（AXRow・AXCell = 選ぶ）。
/// `adapters::mac_gui::Elements::find_containing_n`（押す相手を数える）と `capability_engine::search`（目当てが今の木に押せる形で在るか）が同じこれを使う。
/// 例: `pressable("AXRow", &[], "AXPress")` → true・`pressable("AXStaticText", &[], "AXPress")` → false
pub fn pressable(role: &str, actions: &[String], action: &str) -> bool {
    actions.iter().any(|a| a == action) || matches!(role, "AXRow" | "AXCell")
}

/// ENGINE-AUDIT W7（spec 261「戻せる手は聞かない」・operator 11-03 の決め 2 = 聞くのは消す・送る・買う手と確認の窓の中の手だけ）: 押す手が人の承認（card）を要るか。
/// Some(理由の 1 行・人の言葉・名前を写す) = 要る / None = 要らない（開く・選ぶ・切り替える = 戻せる手）。要る = ① 名前が消す・送る・買う系の語（英語は語ごとに比べる =
/// 「Sender」「Undelete」は当てない・日本語は含むで当てる）② `in_dialog`（押す要素が sheet か dialog の中 = 保存・消す・送るの確認）。app の名前・role で分けない（`role` は今は見ない・
/// 渡すのは呼ぶ側の形を揃えるため）。呼ぶ側: plan の段の `recipe::reveal_or_click`（`in_dialog` = false・名前だけ）と、送る直前の `mac_gui` の reveal（解決した要素で）と、
/// Always allow の `engine_page::CoreEngine::skips_approval`（打つ欄の名前・menu の道の項目・`in_dialog` = false = 当たれば毎回 card）。
/// 保証しない: 語に当たらない名前の危ない button（それは送った後の読み戻しと Stop が受ける）。
/// 例: `asks("Delete Message", "AXMenuItem", false)` → Some・`asks("Acoustic Guitar", "AXRow", false)` → None・`asks("OK", "AXButton", true)` → Some
pub fn asks(label: &str, role: &str, in_dialog: bool) -> Option<String> {
    // 英語は語ごと（小文字）・日本語は含む。「Empty」は入れない（新しい書類の雛形の名前に在る・「Empty Trash」は trash で当たる）
    const WORDS: [(&str, &[&str], &[&str]); 3] = [
        ("deletes something", &["delete", "remove", "trash", "erase", "clear", "discard", "destroy", "wipe", "uninstall"], &["削除", "消去", "消す", "捨てる", "ゴミ箱"]),
        ("sends something", &["send", "post", "submit", "publish", "reply", "forward", "upload", "tweet"], &["送信", "送る", "投稿", "返信", "転送"]),
        ("buys something", &["buy", "purchase", "pay", "order", "checkout", "subscribe", "donate"], &["購入", "買う", "支払", "注文", "決済", "申し込"]),
    ];
    let _ = role;
    let label = label.trim();
    let words: Vec<String> = label.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    if let Some((what, ..)) = WORDS.iter().find(|(_, en, ja)| words.iter().any(|w| en.contains(&w.as_str())) || ja.iter().any(|j| label.contains(j))) {
        return Some(format!("“{label}” {what}"));
    }
    in_dialog.then(|| format!("“{label}” is in a window that asks you to confirm"))
}

/// ENGINE-AUDIT W3: その要素が `words`（`words_of`）のどれかに当たるか（名前・道・値を小文字で含む）。`Facts` の当たりの数と、`WindowState::lines` の ② の並び
/// （道だけの当たりを先に・r8 から ① は名前か値の当たりだけ）。
pub fn matches(c: &Control, words: &[String]) -> bool {
    if words.is_empty() {
        return false;
    }
    let hay = format!("{} {} {}", c.label, c.path.join(" "), c.value.as_deref().unwrap_or_default()).to_lowercase();
    words.iter().any(|w| hay.contains(w.as_str()))
}

/// ENGINE-AUDIT W3: 一言・操作・状況の文から探す語（空白と記号で切った 2 字以上の語・小文字・重ねない・日本語の文の中の英語の語も拾う）。
/// 例: `words_of(&["ライブラリの Guitar をクリック", "Open the guitar list"])` → `["ライブラリの", "guitar", "をクリック", "open", "list"]`（the は機能語 = 探さない）
pub fn words_of(texts: &[&str]) -> Vec<String> {
    // W4: 英語の機能語は探す語にしない（contains で当たる = "the" が Theremin・Other に当たり、当たりの行が膨らむ）。UI の名前になり得る語（open・list・show・all）は残す
    const STOP: [&str; 27] = [
        "the", "and", "for", "with", "from", "into", "this", "that", "of", "to", "in", "on", "at", "by", "as", "is", "are", "be", "it", "its", "an", "my", "me", "you", "your", "please", "now",
    ];
    let mut out: Vec<String> = Vec::new();
    for t in texts {
        for w in t.split(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '_' && c != '-')) {
            let w = w.trim().to_lowercase();
            if w.chars().count() >= 2 && !STOP.contains(&w.as_str()) && !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
}

/// AI がしてよい操作は 4 つだけ。どれも窓の読み（`WindowState` / `adapters::mac_gui::Elements`）で得た element か pid 宛て（座標を持たない）。act の記録（`ActReceipt`）の形。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GuiAction {
    /// AXPress
    Press { element: ElementRef },
    /// AXValue を置く
    SetValue { element: ElementRef, value: String },
    /// menu bar の項目を上から順に（例 ["File", "Save"]）
    Menu { path: Vec<String> },
    /// pid 宛ての key（例 "cmd+s"）。前面の app には送らない
    Key { keys: String },
}

/// act の記録（`ActLog` の 1 行・製品の自己申告。成功の根拠にはしない・読み戻しか試験の側の oracle で見る）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActReceipt {
    pub target: GuiTarget,
    pub action: GuiAction,
    /// 送った時刻（unix ms）
    pub sent_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiError {
    /// この adapter・この OS では出来ない（未導入を含む）
    Unsupported(String),
    /// 窓・要素・menu の項目が無い（token が古い時も）
    NotFound(String),
    /// 人が操作中（lease が人）・同意が無い・AX の許可が無い。何も送っていない
    Denied(String),
    Timeout {
        ms: u64,
    },
    /// bridge が死んだ・壊れた応答
    Bridge(String),
}

impl fmt::Display for GuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(why) => write!(
                f,
                "This action isn't available ({why}). Do it in the app yourself"
            ),
            // 名前だけの時に包む。完全な 1 行（「。」か「. 」で区切った文）はそのまま（W19-3b: 包み直すと「'…窓が見つかりません。…' が見つかりません」）
            Self::NotFound(line) if line.contains('。') || line.contains(". ") => f.write_str(line),
            Self::NotFound(what) => {
                write!(f, "“{what}” wasn't found. Read the screen again, then try again")
            }
            Self::Denied(why) => write!(f, "Didn't act ({why})"),
            Self::Timeout { ms } => write!(
                f,
                "No answer after {ms} ms. Check that the app is responding"
            ),
            Self::Bridge(why) => write!(
                f,
                "The part that controls the screen stopped ({why}). Reopen Shibubu"
            ),
        }
    }
}
impl std::error::Error for GuiError {}

// ---- W19-1: act の記録と、Native Direct の面で人が lease を持つかの判定（YC_ACCEPTANCE の F1・F2）----

/// act の記録の file を指す環境変数（試験の側が run ごとに置き場を決める）。無ければ data dir の `ACT_LOG_FILE`。
/// Wave: W19-1
pub const ACT_LOG_ENV: &str = "SHIBUBU_GUI_ACT_LOG";
/// data dir の下の act の記録の file 名（追記だけの JSONL）。
/// Wave: W19-1
pub const ACT_LOG_FILE: &str = "gui-acts.jsonl";

/// act の記録の 1 行（時刻・手段・宛先は `receipt`・Adapter の id と版）。F1 の経路（act → AX value → act → disk）と、F2 の「act の記録に無い値の変化」
/// の照合に使う（製品の記録なので照合の相手・正は試験の側の読み戻し）。
/// Wave: W19-1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActRecord {
    pub receipt: ActReceipt,
    /// 操作した Adapter の id（registry の `Adapter::id`・`mac_gui` か recipe の id）
    pub adapter: String,
    /// その Adapter の版（Shibubu の版か recipe の版）
    pub adapter_version: String,
    /// 送らなかった理由（None = 送った・送ったかもしれない）。今は app ごとの同意が無い時の `NeedsConsent` だけ（operator 2026-10-23・その時の
    /// `receipt.sent_ms` は断った時刻）。送った数を数える側は `None` の行だけを数える
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_sent: Option<NotSent>,
}

/// act の記録の、送らなかった行の理由。
/// Wave: W19-1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotSent {
    /// 宛先の app を AI に任せる同意が無い（`surface::act_consents` に bundle id が無い）
    NeedsConsent,
}

/// AI に任せる同意が無い app への操作を断った時の 1 行（YC_DECISIONS 11 通目「同意の無い app への act は 0 件で 1 行」・modal にしない）。
/// Wave: W19-1
pub fn act_consent_prompt(app_name: &str) -> String {
    format!("{app_name} {ACT_CONSENT_MARK} (nothing was sent). To allow it, press “Hand {app_name} to the AI”")
}

/// `act_consent_prompt` の 1 行に必ず在る句（Engine の拒否から「同意が無くて送らなかった」を見分ける・`engine_page`）。
/// Wave: W19-1
pub const ACT_CONSENT_MARK: &str = "isn't allowed to be controlled by the AI yet";

/// 人の lease（宛先の app が前面のまま `HUMAN_WAIT` を過ぎた）で何も送らなかった時の 1 行（`adapters::mac_gui` の `op` が出す）。ここに置く理由: Engine が
/// 拒否の理由から「人の側で止めた」を見分ける（`capability_engine::counts_as_failure` = 保存した手順の失敗に数えない）・Adapter と Core が同じ字を持つ。
/// Wave: W19-1（11-03 DEMO-FREEZE で mac_gui から移した）
pub const HUMAN_LEASE_LINE: &str = "Someone is using the app in front, so nothing was sent. Go back to Shibubu, then ask again";

/// 「AI に任せる」の button の AX identifier（試験の道具が BG で AXPress する・Mirror の `consent_button_id` と別の口）。
/// Wave: W19-1
pub fn act_consent_button_id(bundle_id: &str) -> String {
    format!("gui-act-consent:{bundle_id}")
}

/// act の記録の file（追記だけの JSONL・1 行 1 `ActRecord`）。
/// Wave: W19-1
/// 例: `let mut log = ActLog::open(&path)?; let r = adapter.act(&t, &a)?; log.append(&ActRecord { receipt: r, adapter: "mac_gui".into(), adapter_version: v })?`
/// 保証する: `append` が Ok なら 1 行が flush 済み・送らなかった操作（`GuiError::Denied`・送る前の拒否）は書かない（W19-3b の「許可が無い間は act の記録が 0 件」）。
/// 例外は app ごとの同意が無くて断った操作で、`not_sent: needs_consent` の 1 行を書く（Accessibility の許可が無い時は書かない）。
/// 保証しない: 他の process との同時の追記の順。
#[derive(Debug)]
pub struct ActLog {
    path: std::path::PathBuf,
}

impl ActLog {
    /// Wave: W19-1
    pub fn open(path: &std::path::Path) -> Result<ActLog, String> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("Can't open the action log {} ({e})", path.display()))?;
        Ok(ActLog {
            path: path.to_path_buf(),
        })
    }
    /// Wave: W19-1
    pub fn append(&mut self, record: &ActRecord) -> Result<(), String> {
        use std::io::Write;
        let line = serde_json::to_string(record).map_err(|e| e.to_string())?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{line}").and_then(|_| f.flush()))
            .map_err(|e| format!("Can't write to the action log {} ({e})", self.path.display()))
    }
    /// 試験の側が読む（知らない欄の行は Err）。
    /// Wave: W19-1
    pub fn read(path: &std::path::Path) -> Result<Vec<ActRecord>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Can't read the action log {} ({e})", path.display()))?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| format!("{e}: {l}")))
            .collect()
    }
}

/// F2 の人の lease の判定（YC_ACCEPTANCE の F2・W19-1 で決める）が使う、最後の人の値の変化から lease を人に残す時間。
/// Wave: W19-1
pub const HUMAN_LEASE_GRACE_MS: f64 = 5000.0;

/// Native Direct の面で人が操作しているかの材料（人の入力は Shibubu を通らないので、Shibubu は元の app を観る）。
/// Wave: W19-1
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanSignals {
    /// 重ねた窓が key の窓で、その app が前面（`lsappinfo front` と同じ事実を bridge が返す）
    pub app_in_front: bool,
    /// act の記録に無い値の変化（bridge の `ax_observe` が受ける kAXValueChangedNotification）の最後の時刻（unix ms）
    #[serde(default)]
    pub last_unattributed_change_ms: Option<f64>,
}

/// 人が lease を持つか（持つ間、AI の `act` は `GuiError::Denied` で何も送らない）。純関数（時計は引数）。
/// Wave: W19-1
/// 例: `if gui::human_holds(&signals, now_ms) { return Err(GuiError::Denied("人が操作中です".into())) }`
/// 保証する: app が前面の間、または最後の人の変化から `HUMAN_LEASE_GRACE_MS` の間は true・どちらでもなければ false（いつも拒否しない）。
/// 保証しない: Mirror の面（`surface::ControlLease` が人の入力を直接見る）・Terminal（W11 の lease）。
pub fn human_holds(signals: &HumanSignals, now_ms: f64) -> bool {
    signals.app_in_front
        || signals
            .last_unattributed_change_ms
            .is_some_and(|t| now_ms - t < HUMAN_LEASE_GRACE_MS)
}

// ---- W19-3b: Page の移動・自然言語の編集・再起動の後に、AI の操作の宛先を決め直す ----

/// 保存していない宛先（pid・窓 id は起動ごとに変わる）を、今の窓の一覧（registry の `desktop.windows.list` の payload）から決め直す。
/// 判定は `surface::pick_reconnect`（PORTAL-11）と同じ: 同じ app で、題名が `document` と一致する窓が 1 つだけの時だけ Ok（gui は surface を引かないので写しを持つ）。
/// 一致 = 同じ題名か、題名の後ろに空白で続く印だけが違う（「README.md — 編集済み」は同じ文書・「README.md.bak」は別・`project::native::pick_document_window` と同じ。
/// 実機の TextEdit は AI が書いた後・保存の前に題名へ「— 編集済み」が付く）。
/// 同じ app = 窓の `bundle_id` が `bundle_id` と一致（W19-1 で `WindowRef` に入った bridge の読み）。窓の `bundle_id` が空（読めなかった）なら選ばない（app 名だけで繋がない）。
/// `app_name` は理由の 1 行にだけ使う。`document` が空なら決められないので NotFound。
/// Accessibility の許可が無い時は呼ぶ前の registry の gate（readiness）で止まり、何も送らず act の記録も書かない（`ActLog` の約束）。
/// 製品の口: `MacGui` の registry の action（`desktop.window.state`・`desktop.ax.*`・`desktop.key`）は input に `bundle_id` と `document` が在れば、
/// 送る前・読む前にこれで pid と窓番号を決め直す（保存した手順の宛先は起動ごとに変わる・W19a2 の実機で「窓が見つかりません」）。
/// Wave: W19-3b
/// 例: `let ws: Vec<WindowRef> = serde_json::from_value(obs.payload)?; let t = gui::retarget("com.apple.TextEdit", "TextEdit", "README.md", Some("file:///r/README.md"), &ws)?;`
/// 保証する: 0 個・2 個以上は `GuiError::NotFound`（理由の 1 行・先頭に繋がない）・別の app の窓は選ばない・window_id は Some（u32 に入らない窓番号の窓は選ばない）。
/// 保証しない: 窓が今も在る事（次の `observe` が `NotFound` で知らせる）。同意（送る時に `MacGui::op` が見る）。
/// FX-111: `file` = Project の repo の書類（`file://` + 絶対 path・`EngineContext::document_file`）。在れば題名の合う窓のうち書類（`WindowRef::document`）が
/// その file の窓だけを数える = 同じ題名が 2 つでも repo の方に決め、窓が 1 つでも別の folder の書類なら代用しない（0 個と同じ）。題名の合う窓の
/// どれも書類を読めない時（前の bridge・AX が答えない）だけ今までの題名の数で決める。None = 今まで通り（題名だけ）。
pub fn retarget(bundle_id: &str, app_name: &str, document: &str, file: Option<&str>, windows: &[WindowRef]) -> Result<GuiTarget, GuiError> {
    let same: Vec<&WindowRef> = titled(bundle_id, document, windows).filter(|w| u32::try_from(w.window_id).is_ok()).collect();
    // FX-111: 書類を読めた窓が在れば、Project の repo の書類の窓だけを数える（同じ題名の別の folder の書類で代用しない）
    let same = match file {
        Some(f) if same.iter().any(|w| !w.document.is_empty()) => {
            let ours: Vec<&WindowRef> = same.iter().copied().filter(|w| same_file(&w.document, f)).collect();
            if ours.is_empty() && same.iter().all(|w| !w.document.is_empty()) {
                return Err(GuiError::NotFound(format!(
                    "The “{document}” open in {app_name} is another file, not the one in this project. Nothing was changed. Open this project's {document}, then ask again"
                )));
            }
            if ours.is_empty() {
                return Err(GuiError::NotFound(format!("{} windows of {app_name} have “{document}” open and Shibubu can't tell which. Close the others, then ask again", same.len())));
            }
            ours
        }
        _ => same,
    };
    match same.as_slice() {
        [w] => Ok(GuiTarget { bundle_id: bundle_id.into(), pid: w.pid, window_id: u32::try_from(w.window_id).ok() }),
        // G75: 名指しの書類が開いていない = 別の窓で代用しない・何も変えていない・次の一手
        [] => Err(GuiError::NotFound(format!("“{document}” isn't open in {app_name}. Nothing was changed. Open it, then ask again"))),
        many => Err(GuiError::NotFound(format!("{} windows of {app_name} have “{document}” open and Shibubu can't tell which. Close the others, then ask again", many.len()))),
    }
}

/// CHAT-013: 名指しの書類（`document` の題名・`file` の書類）がその app で閉じているか = 題名の合う窓が無いか、在っても全部が別の file を開いている
/// （書類を読めない題名の合う窓が 1 つでも在れば false = 閉じているとは言わない）。true の時だけ Engine は「開く」承認の手を出す。
/// 例: `if gui::is_closed("com.apple.TextEdit", "README.md", "file:///r/README.md", &ws) { /* Open README.md in TextEdit */ }`
pub fn is_closed(bundle_id: &str, document: &str, file: &str, windows: &[WindowRef]) -> bool {
    titled(bundle_id, document, windows).all(|w| !w.document.is_empty() && !same_file(&w.document, file))
}

/// その app（bundle）の窓のうち題名が名指しの書類の物（`retarget` と `is_closed` の同じ一致: 題名 = 書類か、書類の後ろに空白〈「— 編集済み」〉）。
fn titled<'a>(bundle_id: &'a str, document: &'a str, windows: &'a [WindowRef]) -> impl Iterator<Item = &'a WindowRef> {
    let named = move |t: &str| !document.is_empty() && (t == document || t.strip_prefix(document).is_some_and(|rest| rest.starts_with(' ')));
    windows.iter().filter(move |w| !w.bundle_id.is_empty() && w.bundle_id == bundle_id && named(&w.title))
}

/// FX-111: 書類の場所（`file://…` の URL・`file://localhost/…`・絶対 path）の disk の path（% の符号を解く）。
/// 例: `gui::file_path("file:///Users/you/My%20Repo/README.md") == PathBuf::from("/Users/you/My Repo/README.md")`
pub fn file_path(url: &str) -> std::path::PathBuf {
    let s = url.trim();
    let s = s.strip_prefix("file://").map_or(s, |rest| rest.strip_prefix("localhost").unwrap_or(rest));
    // 反証 G-A: Shibubu が作る場所（`EngineRequest::document_file` = `file://` + 符号にしない path）は % を解くと別の場所になる（folder 名の `50%25 off`）。
    // 解かない path が disk に在ればそれ（AX の URL は % を符号にしているので、解かない字の path はふつう在らない）
    // ponytail: 「解かない path が在る」で見分ける。AX の URL と同じ字の folder が別に在る Mac では誤る。直すなら document_file を符号にした URL で作る
    if std::path::Path::new(s).exists() {
        return std::path::PathBuf::from(s);
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    std::path::PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

/// FX-111: 2 つの書類の場所（`file://…` の URL か絶対 path・% の符号は解く）が同じ file か（両方 disk に在れば canonicalize で比べる = /tmp と /private/tmp）。
fn same_file(a: &str, b: &str) -> bool {
    let real = |s: &str| {
        let p = file_path(s);
        std::fs::canonicalize(&p).unwrap_or(p)
    };
    !a.trim().is_empty() && !b.trim().is_empty() && real(a) == real(b)
}

/// W19-1 の署名を compile で縛り、保存形式（serde の往復と未知の欄の拒否）と act の記録の file を走らせる（W19-1）・W19-3b の宛先の決め直し（W19-3b）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn w19_1_act_log_and_lease_signatures() {
        let _: fn(&std::path::Path) -> Result<ActLog, String> = ActLog::open;
        let _: fn(&mut ActLog, &ActRecord) -> Result<(), String> = ActLog::append;
        let _: fn(&std::path::Path) -> Result<Vec<ActRecord>, String> = ActLog::read;
        let _: fn(&HumanSignals, f64) -> bool = human_holds;
    }

    #[test]
    fn w19_1_act_record_round_trips_and_rejects_unknown_fields() {
        let rec = ActRecord {
            receipt: ActReceipt {
                target: GuiTarget { bundle_id: "org.example.app".into(), pid: 42, window_id: Some(7) },
                action: GuiAction::Key { keys: "cmd+s".into() },
                sent_ms: 1.5,
            },
            adapter: "mac_gui".into(),
            adapter_version: "0.4.0".into(),
            not_sent: None,
        };
        let v = serde_json::to_value(&rec).unwrap();
        assert!(v.get("not_sent").is_none(), "送った行は not_sent を書かない（W19-1 の前の行と同じ形）");
        let refused = ActRecord { not_sent: Some(NotSent::NeedsConsent), ..rec.clone() };
        let rv = serde_json::to_value(&refused).unwrap();
        assert_eq!(rv["not_sent"], "needs_consent");
        assert_eq!(serde_json::from_value::<ActRecord>(rv).unwrap(), refused);
        assert_eq!(serde_json::from_value::<ActRecord>(v.clone()).unwrap(), rec);
        let mut extra = v;
        extra["value"] = serde_json::json!("打った文字");
        assert!(serde_json::from_value::<ActRecord>(extra).is_err(), "未知の欄は拒否");
        let s = HumanSignals { app_in_front: false, last_unattributed_change_ms: Some(10.0) };
        assert_eq!(serde_json::from_value::<HumanSignals>(serde_json::to_value(s).unwrap()).unwrap(), s);
    }

    /// F2 の人の lease: 前面の間・最後の人の変化から 5 秒の間は人が持つ。どちらでもなければ AI に渡す（いつも拒否する実装は赤）
    #[test]
    fn w19_1_human_holds_while_in_front_or_within_the_grace() {
        let s = |front: bool, last: Option<f64>| HumanSignals {
            app_in_front: front,
            last_unattributed_change_ms: last,
        };
        assert!(human_holds(&s(true, None), 0.0), "前面の間は人");
        assert!(human_holds(&s(false, Some(1000.0)), 1000.0 + HUMAN_LEASE_GRACE_MS - 1.0), "変化から 5 秒の間は人");
        assert!(!human_holds(&s(false, Some(1000.0)), 1000.0 + HUMAN_LEASE_GRACE_MS), "5 秒たったら外れる");
        assert!(!human_holds(&s(false, None), 0.0), "人の材料が無ければ拒まない");
    }

    /// act の記録: 追記だけ・1 行 1 件・開き直しても前の行が残る・知らない欄の行は読みで Err
    #[test]
    fn w19_1_act_log_appends_one_line_per_act_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("shibubu-actlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ACT_LOG_FILE);
        let rec = |keys: &str| ActRecord {
            receipt: ActReceipt {
                target: GuiTarget { bundle_id: "org.example.app".into(), pid: 42, window_id: None },
                action: GuiAction::Key { keys: keys.into() },
                sent_ms: 1.0,
            },
            adapter: "mac_gui".into(),
            adapter_version: "0.4.0".into(),
            not_sent: None,
        };
        let mut log = ActLog::open(&path).unwrap();
        assert_eq!(ActLog::read(&path).unwrap(), vec![], "開いただけでは 0 件");
        log.append(&rec("cmd+s")).unwrap();
        ActLog::open(&path).unwrap().append(&rec("cmd+z")).unwrap();
        assert_eq!(ActLog::read(&path).unwrap(), vec![rec("cmd+s"), rec("cmd+z")]);
        std::fs::write(&path, format!("{}\n{{\"x\":1}}\n", serde_json::to_string(&rec("a")).unwrap())).unwrap();
        assert!(ActLog::read(&path).is_err(), "知らない形の行は Err");
        assert!(ActLog::open(&dir.join("無い").join(ACT_LOG_FILE)).is_err(), "置き場が無ければ開けない（黙って捨てない）");
    }

    /// ② の ref（operator 2026-10-23）: `Control::element` は payload を往復し、R-G2 の payload（element 無し）も読め、None は JSON に出さない。
    #[test]
    fn w19_1_control_carries_ref_and_reads_r_g2_payload() {
        let c = Control {
            element: Some(ElementRef("e3.2".into())),
            label: "保存".into(),
            role: "AXButton".into(),
            value: None,
            actions: vec!["AXPress".into()],
            value_settable: false,
            path: Vec::new(),
            selected: None,
            expanded: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["element"], "e3.2");
        assert_eq!(serde_json::from_value::<Control>(v).unwrap(), c);
        let old = serde_json::json!({ "label": "保存", "role": "AXButton", "value": null, "actions": ["AXPress"], "value_settable": false });
        let read: Control = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(read, Control { element: None, ..c });
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            old,
            "None は書かない（R-G2 の payload と同じ JSON）"
        );
    }

    /// W19-3b の署名を compile で縛る。
    #[test]
    fn w19_3b_retarget_signature() {
        let _: fn(&str, &str, &str, Option<&str>, &[WindowRef]) -> Result<GuiTarget, GuiError> = retarget;
    }

    /// W19-3b: 再起動・Page の移動の後に宛先を決め直す規則（PORTAL-11 と同じ）。同じ bundle で題名が完全に一致する窓が 1 つの時だけ Ok・
    /// 0 個 / 2 個以上 / 別の app / bundle の読めない窓 / 題名が空は NotFound（先頭に繋がない）
    #[test]
    fn w19_3b_retarget_picks_only_one_window_of_the_same_app_by_exact_title() {
        let w = |pid: u32, id: i64, app: &str, bundle: &str, title: &str| WindowRef { pid, window_id: id, app_name: app.into(), title: title.into(), bundle_id: bundle.into(), document: String::new() };
        let te = "com.apple.TextEdit";
        let ws = vec![
            w(501, 10, "TextEdit", te, "README.md"),
            w(501, 11, "TextEdit", te, "notes.md"),
            w(777, 12, "Notes", "com.apple.Notes", "README.md"),
            w(501, 13, "TextEdit", te, "README.md.bak"),
            w(888, 14, "TextEdit", "", "notes.md"),
            w(501, 15, "TextEdit", te, "draft.md — 編集済み"),
        ];
        assert_eq!(retarget(te, "TextEdit", "draft.md", None, &ws).unwrap().window_id, Some(15), "書いた後・保存の前の「— 編集済み」は同じ文書");
        assert_eq!(retarget(te, "TextEdit", "README.md", None, &ws).unwrap(), GuiTarget { bundle_id: te.into(), pid: 501, window_id: Some(10) });
        assert_eq!(retarget(te, "TextEdit", "notes.md", None, &ws).unwrap().window_id, Some(11), "bundle の読めない同じ名前の app の窓は数えない");
        let not_found = |r: Result<GuiTarget, GuiError>| matches!(r, Err(GuiError::NotFound(_)));
        assert!(not_found(retarget(te, "TextEdit", "無い.md", None, &ws)), "0 個");
        let two = [ws.clone(), vec![w(502, 20, "TextEdit", te, "README.md")]].concat();
        assert!(not_found(retarget(te, "TextEdit", "README.md", None, &two)), "2 個（別の起動の同じ題名）は決めない");
        assert!(not_found(retarget("com.apple.Notes", "Notes", "notes.md", None, &ws)), "別の app の窓は選ばない");
        assert!(not_found(retarget(te, "TextEdit", "", None, &ws)), "題名が空は決められない");
        assert!(not_found(retarget(te, "TextEdit", "big", None, &[w(501, i64::from(u32::MAX) + 1, "TextEdit", te, "big")])), "u32 に入らない窓番号");
    }

    /// G101（FX-111・CHAT-013）: 同じ題名の窓は書類（AXDocument）が Project の repo の file の窓に決める・別の folder の書類だけなら代用しない・
    /// 書類を読めない窓だけなら今まで通り（題名の数）・閉じている = 題名の合う窓が無いか全部が別の file。file の URL の % と /tmp ↔ /private/tmp は同じ file。
    /// 負の対照: retarget の FX-111 の絞り込みを外す → 2 つの README で「can't tell which」になり赤・is_closed を `false` → closed が赤。
    #[test]
    fn g101_the_window_of_this_projects_file_is_picked_by_its_document() {
        let te = "com.apple.TextEdit";
        let not_found = |r: Result<GuiTarget, GuiError>| matches!(r, Err(GuiError::NotFound(_)));
        let d = std::env::temp_dir().join(format!("g101-gui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("my repo")).unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        let (ours, other) = (d.join("my repo/README.md"), d.join("other/README.md"));
        std::fs::write(&ours, "a").unwrap();
        std::fs::write(&other, "b").unwrap();
        let ours_url = format!("file://{}", ours.display()).replace(' ', "%20");
        let file = format!("file://{}", ours.display());
        let w = |id: i64, title: &str, doc: &str| WindowRef { pid: 501, window_id: id, app_name: "TextEdit".into(), title: title.into(), bundle_id: te.into(), document: doc.into() };
        let other_url = format!("file://{}", other.display());
        assert_eq!(file_path(&ours_url), ours, "% を解く");
        assert_eq!(file_path("file://localhost/tmp/x"), std::path::PathBuf::from("/tmp/x"));
        let twins = [w(10, "README.md", &other_url), w(11, "README.md — Edited", &ours_url)];
        assert_eq!(retarget(te, "TextEdit", "README.md", Some(&file), &twins).unwrap().window_id, Some(11), "同じ題名の 2 つから repo の書類");
        assert!(not_found(retarget(te, "TextEdit", "README.md", None, &twins)), "file を渡さなければ今まで通り決めない");
        let alone = [w(10, "README.md", &other_url)];
        let e = retarget(te, "TextEdit", "README.md", Some(&file), &alone).unwrap_err().to_string();
        assert!(e.contains("another file") && e.contains("Nothing was changed"), "1 つでも別の folder の書類は代用しない: {e}");
        let mixed = [w(10, "README.md", &other_url), w(11, "README.md", "")];
        assert!(not_found(retarget(te, "TextEdit", "README.md", Some(&file), &mixed)), "読めない窓が混じれば決めない");
        let old = [w(10, "README.md", "")];
        assert_eq!(retarget(te, "TextEdit", "README.md", Some(&file), &old).unwrap().window_id, Some(10), "書類を読めない（前の bridge）なら題名で");
        let tmp_alias = format!("file:///tmp/../tmp/{}", ours.strip_prefix(std::env::temp_dir()).map(|p| p.display().to_string()).unwrap_or_default());
        if std::env::temp_dir().starts_with("/tmp") {
            assert!(same_file(&tmp_alias, &file), "canonicalize で同じ file");
        }

        assert!(is_closed(te, "README.md", &file, &[]), "窓が無い");
        assert!(is_closed(te, "README.md", &file, &[w(10, "notes.md", "")]), "別の書類だけ");
        assert!(is_closed(te, "README.md", &file, &alone), "同じ名前の別の file だけ");
        assert!(!is_closed(te, "README.md", &file, &twins), "repo の書類が開いている");
        assert!(!is_closed(te, "README.md", &file, &old), "読めない同じ題名の窓 = 閉じているとは言わない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// r8（operator 11-03・chars_controls 20,856）: 語に当たる祖先（Library）の下の 500 行は道だけの当たり = 字の予算の中・名前の当たり（祖先と Acoustic Guitar）は
    /// 予算の外で先頭。負の対照（手で）: ① の `named` を `matches` に戻す → 501 行が全部 ① に入り、字の合計が予算の数倍で赤。
    #[test]
    fn r8_rows_under_a_matching_parent_stay_inside_the_budget() {
        let row = |label: &str, path: &[&str]| Control {
            element: None,
            label: label.into(),
            role: "AXRow".into(),
            value: None,
            actions: vec![],
            value_settable: false,
            path: path.iter().map(|p| p.to_string()).collect(),
            selected: None,
            expanded: None,
        };
        let mut controls = vec![row("Library", &[])];
        controls.extend((0..500).map(|i| row(&format!("Patch {i}"), &["Library"])));
        controls.push(row("Acoustic Guitar", &["Library"]));
        let s = WindowState { pid: 1, window_id: 2, app_name: "Mixer".into(), complete: true, controls };
        let lines = s.lines(&words_of(&["Open the guitar list in the Library"]), 2_000);
        let total: usize = lines.iter().map(|l| l.chars().count()).sum();
        assert!(lines[..2].iter().any(|l| l.starts_with("AXRow: Acoustic Guitar")) && lines[..2].iter().any(|l| l.starts_with("AXRow: Library")), "名前の当たりが先頭に無い: {:?}", &lines[..3]);
        assert!(total <= 2_000 + 200, "道だけの当たりが予算の外に出た: {total} 字・{} 行", lines.len());
        assert!(lines.last().is_some_and(|l| l.starts_with("… and ")), "入らない分の 1 行が無い: {:?}", lines.last());
    }

    /// ENGINE-AUDIT W7（operator 11-03 の決め 2）: 聞くのは消す・送る・買う系の名前と、確認の窓（sheet・dialog）の中の手だけ。開く・選ぶ・切り替える手は聞かない。
    /// 負の対照（本体の後・手で）: 英語の語ごとの判定を「含む」に替える → 「Sender」「Undelete」が赤 / in_dialog を見ない → 「OK」が赤
    #[test]
    fn w7_asks_only_for_deleting_sending_buying_or_inside_a_confirmation() {
        for (label, role) in [("Delete Message", "AXMenuItem"), ("Move to Trash", "AXButton"), ("Send", "AXButton"), ("Post", "AXButton"), ("Buy Now", "AXButton"), ("Place Order", "AXButton"), ("Pay", "AXButton"), ("Remove Track", "AXMenuItem"), ("削除", "AXButton"), ("送信", "AXButton"), ("購入する", "AXButton")] {
            assert!(asks(label, role, false).is_some_and(|why| why.contains(label)), "{label} を聞かなかった");
        }
        for (label, role) in [("Acoustic Guitar", "AXRow"), ("Library", "AXButton"), ("Sender", "AXStaticText"), ("Undelete", "AXButton"), ("Sounds", "AXRadioButton"), ("Show Library", "AXMenuItem"), ("ギター", "AXCell"), ("", "AXRow")] {
            assert_eq!(asks(label, role, false), None, "{label} を聞いた（開く・選ぶ手）");
        }
        assert!(asks("OK", "AXButton", true).is_some(), "確認の窓の中の手を聞かなかった");
        assert!(asks("Acoustic Guitar", "AXRow", true).is_some(), "sheet の中の行も聞く");
    }
}
