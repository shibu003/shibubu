//! Interactive Mirror の口（YC W16M-0・YCM-063・v0.14 §3.3）。feature `mirror` の時だけ compile する（W15 の .app では切る）。
//! 口は 2 つで、型で分ける（混ぜない）:
//!   - 人の口 `MirrorSurface::relay_human` … Mirror の矩形の上で人がした入力を、helper の `stream` の stdin へ送る。送る前に必ず
//!     `RelayRecord` を relay の記録（JSONL）に足す（E1: 元の app の値の変化の前に relay の記録が 1 件ある）。
//!   - 自動の口 `AutomatedInput::send` … `kangae-mirror measure` と試験だけ。helper の `input` subcommand（別 process）で送り、
//!     relay の記録には書かない。AI の GUI 操作は Mirror を通らない（W2c の口 = `MacGui::{observe, act, verify}`）。
//! 同意: `ConsentGate` が app（bundle id）ごとに持ち、`ConsentGrant` を持たない限り `MirrorSurface` は作れない
//! ＝ 同意の前は stream も relay も 0 件（compile で守る）。同意の保存は `ui_state(session_id = CONSENT_UI_STATE_SESSION, key = bundle id)`。
//! 映像（W16M-1）: `MirrorSurface::stream` が helper の stdout（tag 1 = frame）から最新の `MirrorFrame`（RGBA）を返し、
//! `ui::mirror_view::MirrorPages` が Page の矩形（`SurfaceSpec{backend: Mirror}`）に egui の texture で描く。
//! 入力（W16M-2）: `route_for` が入力ごとに背景の app に届く道（postToPid の key・AX の点の要素・Apple Event の save）を決め、届く道の無い
//! 入力（drag・scroll・2 回以上の click・⌘Z などの menu の key）は送らず `NeedsFront` の 1 行。道を走らせるのは helper の stream の人の口
//! （bridge/macos-mirror/KangaeMirror.swift の `sendHuman`）。経路の証拠（E1）は relay の記録と試験の側の値の変化の時刻の順（`tests/common/readback.rs` の `e1_path`）。
//! 2 面以上（G32）: 同じ helper の `MirrorSurface` は helper の process 1 本（`HelperProc`）を分け合い、2 面目からの stream と窓探しは
//! その process の stdin（`{"cmd":"open"}`・`{"cmd":"list"}`）に頼む。別の process の同じ helper の SCShareableContent は生きている stream を
//! -3805 で落とすが、同じ process の中では落ちない（G32 の probe）。行の契約は KangaeMirror.swift の頭の `stream`。
//! 常駐（MIRROR-QUIET-1101・owner 208）: 開いている面が 0 の間も、窓探し・status・doc-path は 1 本の常駐（`helper serve`・`resident`）の stdin で
//! 頼む（探すたびに process を起こさない＝launchservicesd への check in も WindowServer への接続も Shibubu 1 回の起動で 1 回）。
//! 写さない映像（MIRROR-SPEED-1101・owner 202・205）: helper は SCK の IOSurface を mach port で渡し（`--surfaces`・`inbox` / `read_surfaces`）、
//! `MirrorFrame::surface` の `IoSurface` を UI が GPU の texture にそのまま載せる（`ui::mirror_view`）。受け口を作れない時は今まで通り pipe の RGBA。
//! 図は docs/diagrams.md §19 の「W16M-0 の口」「W16M-1 の映像」「W16M-2 の入力」「G32 の 1 本の helper」。
//! Wave: W16M-0, W16M-1, W16M-1b, W16M-2

use super::{ControlLease, MirrorFrameMeta};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, Weak};
use std::task::Waker;

/// Mirror の時計（unix ms・小数）。relay の記録の `at_ms`・frame の `received_ms` がこれで書かれる。
/// R-M: PoC（`src/bin/kangae-mirror.rs`）と試験（`tests/mirror_e0.rs`）の同じ本体の写しを消してこれを呼ぶ（private → pub・本体は同じ）。
/// 例: `let at_ms = now_ms(); lease.human_input(at_ms);`
/// 保証する: 小数の ms（`as_secs_f64() * 1000`・JSONL は `float_roundtrip` で同じ値に読み戻る）。
/// 保証しない: `surface::now_ms`（整数 ms・`as_millis`）と同じ値である事＝寄せない（記録の精度が変わり E1 の時刻の順に効く）。
pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// relay の記録 1 行の版（W16M-2 で 2: `route` を足した。記録は run ごとの file なので 1 の行は読まない）。
/// MIRROR-HYBRID-1101 追記 235 で 3: 答えの行（`outcome`・`ms`）を足した（2 の行もそのまま読める）
pub const RELAY_SCHEMA_V: u32 = 3;
/// 同意の記録の版（W19-1 で `surface` に移した）。
pub use super::CONSENT_SCHEMA_V;
/// 同意を置く `ui_state` の session_id（key は bundle id・json は `ConsentRecord`）。
pub const CONSENT_UI_STATE_SESSION: &str = "mirror-consent";
/// relay の記録（JSONL）の置き場を指す環境変数。無ければ data dir の `mirror-relay.jsonl`（試験は evidence の下を指す）。
pub const RELAY_LOG_ENV: &str = "KANGAE_MIRROR_RELAY_LOG";

// ---- 入力 ----

/// 修飾 key（helper の `mods` と同じ語）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyMod {
    Cmd,
    Shift,
    Option,
    Control,
}

/// 入力 1 つ。x / y は映す窓の中の割合（0〜1。Mirror の矩形の点を frame の letterbox を引いて直した物）。
/// helper の 1 行の JSON（`op` の tag）と同じ形。focus（AX）は人の入力ではないので無い。
/// 例: `MirrorInput::Text { text: "nonce".into() }`。保証: 形だけ（値の範囲は helper が `bad_point` で拒む）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum MirrorInput {
    Click { x: f64, y: f64, count: u32 },
    Scroll { x: f64, y: f64, dx: i32, dy: i32 },
    /// steps は helper の刻み数（無ければ helper の既定 8・KangaeMirror.swift の drag）。
    Drag {
        x: f64,
        y: f64,
        x2: f64,
        y2: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
    },
    Text { text: String },
    Key { key: String, mods: Vec<KeyMod> },
}

/// relay の記録に残す入力の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Click,
    Scroll,
    Drag,
    Text,
    Key,
}

impl MirrorInput {
    /// 例: `input.kind() == InputKind::Key`。保証: variant と 1 対 1。
    pub fn kind(&self) -> InputKind {
        match self {
            MirrorInput::Click { .. } => InputKind::Click,
            MirrorInput::Scroll { .. } => InputKind::Scroll,
            MirrorInput::Drag { .. } => InputKind::Drag,
            MirrorInput::Text { .. } => InputKind::Text,
            MirrorInput::Key { .. } => InputKind::Key,
        }
    }
    /// Mirror の矩形の中の位置（窓の中の割合）。key と text は None。drag は始点。
    pub fn pos(&self) -> Option<[f64; 2]> {
        match *self {
            MirrorInput::Click { x, y, .. }
            | MirrorInput::Scroll { x, y, .. }
            | MirrorInput::Drag { x, y, .. } => Some([x, y]),
            MirrorInput::Text { .. } | MirrorInput::Key { .. } => None,
        }
    }
}

/// helper の 1 行に付ける出所（`"origin"`）。`stream` の stdin は `human` だけを受け、`input` subcommand は `human` を拒む（KangaeMirror.swift の頭の契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputOrigin {
    Human,
    Automated,
}

// ---- 入力の道（W16M-2）----

/// 人の入力 1 つを背景の元の app に届ける道（W16M-2）。`route_for` が決め、relay の記録に残す。走らせるのは helper の stream の人の口
/// （KangaeMirror.swift の `sendHuman`。W17-2 の bridge の口〈yc/gui 140ed05〉は ax_tree の上に在り点の要素も無いので使わない）。
/// 実測（2026-10-21・TextEdit・背景・docs/evidence/W16M-2/log.md）: 文字・矢印・⌃E・⌘← は postToPid で届く・mouse の postToPid は 0/4・
/// menu の key equivalent（⌘S・⌘Z・⌘A）は postToPid で処理されない・menu の項目は AXEnabled が古いまま false で AXPress / AXPick しても何も起きない
/// （計画の「保存と Undo は AX の menu」は成り立たなかった）・Apple Event の save は 0.3 秒で disk に書けて前面は変わらない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRoute {
    /// `CGEvent.postToPid`（文字と key。cursor と前面を変えない）。key は app の key の窓に届くので、helper は送る前に映している窓を AX で main にし、
    /// なれなければ送らない（別の書類に打たない・W16M-2 で実測）
    PostToPid,
    /// `AXUIElementCopyElementAtPosition` で点の要素を引き、AXPress を持てば押す・文字の欄なら `AXSelectedTextRange` をその点の文字の位置に置く。
    /// どちらでもない要素には送らず、helper の答えは `needs_front`（`MirrorSurface::refusal` が `NeedsFront` にする）
    AxAtPoint,
    /// ⌘S: 映している窓の書類（題名 = 書類の名前）を Apple Event の `save` で保存させる（app を前に出さない）。同じ名前の書類が 1 つでなければ
    /// 送らない（別の書類を保存しない）。許可（オートメーション）が無ければ helper の答えは `automation_denied`
    SaveDocument,
    /// MIRROR-HYBRID-1101: 本物の窓を枠の位置のまま Shibubu の上に出した後（`MirrorPages::go_direct`）の、触った最初の 1 押し（click・drag・scroll）。
    /// helper は点の一番上の窓（layer 0・見える物）が映している窓になるのを 500ms まで待って HID で送る（本物の cursor の所 = 人が押した所）。
    /// 来なければ送らず `not_front`。Accessibility の許可が要る（無ければ `not_trusted`）。`route_for` は返さない（`MirrorSurface::relay_front` だけが使う）
    Front,
    /// 公開 API で背景の app に届く道が無い（drag・scroll・2 回以上の click・⌘S 以外の ⌘ と文字の key）。送らず、relay の記録も書かず `MirrorError::NeedsFront`
    NeedsFront,
}

/// ⌘ と組んでも postToPid で届く key（文字の欄の key binding で、menu の key equivalent ではない。⌘← は実測で届いた）。
const CMD_MOVE_KEYS: [&str; 10] = ["left", "right", "up", "down", "home", "end", "pageup", "pagedown", "delete", "forwarddelete"];

/// 入力 → 道（純関数・表）。
/// 例: `assert_eq!(route_for(&MirrorInput::Key { key: "s".into(), mods: vec![KeyMod::Cmd] }), InputRoute::SaveDocument);`
/// 保証する: ⌘S（⌘ だけ）→ SaveDocument・⌘ と移動の key（`CMD_MOVE_KEYS`）→ PostToPid・それ以外の ⌘ の key（⌘Z・⇧⌘Z・⌥⌘S・⌘A…）→ NeedsFront・
/// ⌘ の無い key（⌃E を含む）と文字 → PostToPid・1 回の click → AxAtPoint・drag / scroll / 2 回以上の click → NeedsFront。
/// 保証しない: その道で実際に届く事（読み戻しは試験の側の E1）。
pub fn route_for(input: &MirrorInput) -> InputRoute {
    match input {
        MirrorInput::Text { .. } => InputRoute::PostToPid,
        MirrorInput::Key { mods, .. } if !mods.contains(&KeyMod::Cmd) => InputRoute::PostToPid,
        MirrorInput::Key { key, mods } => {
            let key = key.to_lowercase();
            if key == "s" && mods.iter().all(|m| *m == KeyMod::Cmd) {
                InputRoute::SaveDocument
            } else if CMD_MOVE_KEYS.contains(&key.as_str()) {
                InputRoute::PostToPid
            } else {
                InputRoute::NeedsFront
            }
        }
        MirrorInput::Click { count, .. } if *count <= 1 => InputRoute::AxAtPoint,
        MirrorInput::Click { .. } | MirrorInput::Scroll { .. } | MirrorInput::Drag { .. } => InputRoute::NeedsFront,
    }
}

/// frame の pixel の点 → 元の窓の中の割合（0〜1）。contentRect（point）× scaleFactor で中身の pixel の矩形を出し、余白（letterbox）を引く。
/// PoC（`src/bin/kangae-mirror.rs`）と Page（`ui::mirror_view::mirror_inputs`）が同じ物を使う。
/// 例: `let [x, y] = frame_point_to_window(&meta, px, py)?; MirrorInput::Click { x, y, count: 1 }`
/// 保証する: 中身の外（余白）は None（その点は元の窓に当たらない）・contentRect が 0 なら frame 全体を中身とする。保証しない: 元の窓が今もその大きさである事（helper が送るたびに bounds を引き直す）。
pub fn frame_point_to_window(meta: &MirrorFrameMeta, px: f64, py: f64) -> Option<[f64; 2]> {
    let (x0, y0, w, h) = content_px(meta);
    let (nx, ny) = ((px - x0) / w, (py - y0) / h);
    ((0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny)).then_some([nx, ny])
}

/// `frame_point_to_window` の逆: 元の窓の中の割合 → frame の pixel の点（W16M-3: helper が答えた caret の矩形を映像の上に描く）。
/// 例: `let [px, py] = window_point_to_frame(&meta, 0.06, 0.08);`
/// 保証する: 余白を足し戻す（`frame_point_to_window` と往復する）。保証しない: 割合が 0〜1 の中である事（そのまま延ばす）。
/// Wave: W16M-3
pub fn window_point_to_frame(meta: &MirrorFrameMeta, nx: f64, ny: f64) -> [f64; 2] {
    let (x0, y0, w, h) = content_px(meta);
    [x0 + nx * w, y0 + ny * h]
}

/// frame の中で元の窓の中身が占める pixel の矩形（contentRect〈point〉× scaleFactor・0 なら frame 全体）。
fn content_px(meta: &MirrorFrameMeta) -> (f64, f64, f64, f64) {
    let [cx, cy, cw, ch] = meta.content;
    if cw > 0.0 && ch > 0.0 {
        (cx * meta.factor, cy * meta.factor, cw * meta.factor, ch * meta.factor)
    } else {
        (0.0, 0.0, meta.width as f64, meta.height as f64)
    }
}

/// 映す相手。窓 id と pid は対で持ち、どちらかが変われば止まる（別の窓に付け替えない・PORTAL-11）。同意は bundle id で引く（pid は起動ごとに変わる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorTarget {
    pub window_id: u64,
    pub pid: i64,
    pub bundle_id: String,
    pub app_name: String,
}

impl MirrorTarget {
    /// helper の `list`（capture できる窓）から、bundle id が同じで題名に `title_has` を含む窓を 1 つ選ぶ（W16M-1・Page の配線が
    /// `SurfaceTarget::App{bundle_id, file}` の file の名前を `title_has` に渡す）。
    /// W16M-3（窓の選び方）: `want` = 映したい書類の絶対 path（Project の repo の dir + `file`）。Some なら題名の合う窓ごとに helper の `doc-path`（その窓の書類の path）を問い、
    /// 別の書類と分かった窓は選ばない（M2 で実測: 前の run の復元した同じ題名の窓を選び、その書類に打った）。
    /// 例: `let t = MirrorTarget::find(&helper, "com.apple.TextEdit", Some("notes.md"), Some(Path::new("/repo/docs/notes.md")))?;`
    /// 保証する: 返す窓は今 helper が capture できる窓。`want` が Some なら、書類が `want` の窓 → 書類を引けない窓が 1 つだけならその窓。書類を引けない窓が
    /// 2 つ以上（どれが書類の窓か確かめられない）なら当て推量で選ばず `Ambiguous`（M4 ③: .app に TextEdit を操作する許可が無く、全部の窓が引けず、
    /// list の先頭＝owner の別の書類の窓を選んで映した）。書類が別と分かった窓しか無ければ `NoWindow`。`want` が None なら前の窓（list の先頭）。
    /// 無ければ `NoWindow`（app を起こさない・file を開かない）。`ask` が true の時だけ、許可がまだ決まっていない app に許可を尋ねてから書類を引く
    /// （macOS の確認の画面が前に出る＝利用者が button を押した時だけ・`MirrorPages::show`）。
    /// 保証しない: 同意（呼ぶ側が `ConsentGate::check`）と画面収録の許可（`MirrorSurface::open` が見る）・`want` が None の時に同じ題名の窓が 2 つ在れば
    /// 正しい方を選ぶ事（先頭を選ぶ・Project に repo の dir が無い時）。
    /// G32: 同じ helper の面が開いている間は `list` の process を起こさず、その process の stdin の `{"cmd":"list"}` で引く（開いている面を落とさない）。
    /// G78: 書類の問いが時間切れ（ae_-1712・背景の app の App Nap）なら同じ窓にもう 1 回だけ問う（起動ごとに 1 回・他の理由は問い直さない）。
    /// RQ4（RQ-019・`Choose window…`）: その app の今 capture できる窓の題名を helper の `list` の順で返す（選ばない・app を起こさない・file を開かない）。
    /// 空の題名の窓は入れない。返した題名を `PageIntent::ChooseWindow { title: Some(題名) }` に渡すと、次の `find` はその語を `title_has` に使う。
    /// 例（本体の `MirrorPages::show` の帯の menu・裏の thread）: `let titles = MirrorTarget::titles(&helper, "com.apple.TextEdit")?;`
    /// 保証する: 開いている面を落とさない（`find` と同じく live の helper の stdin で引く）。窓が 0 なら `Ok(vec![])`（`NoWindow` にしない = menu が `No windows · Open {app}`）。
    /// 失敗: helper が起きない・画面収録の許可が無い = `find` と同じ `MirrorError`。
    /// Wave: RQ4
    pub fn titles(helper: &Path, bundle_id: &str) -> Result<Vec<String>, MirrorError> {
        let me = std::process::id() as i64;
        let list = list_windows(helper)?;
        let mut out: Vec<String> = Vec::new();
        for w in list.as_array().into_iter().flatten().filter(|w| w["bundle_id"].as_str() == Some(bundle_id) && w["pid"].as_i64() != Some(me)) {
            let title = w["title"].as_str().unwrap_or("").trim();
            if !title.is_empty() && !out.iter().any(|t| t == title) {
                out.push(title.to_owned());
            }
        }
        Ok(out)
    }

    /// MIRROR-FRAME-1101（owner 247「自分が開いているappやbrowserを入れるとmirrorされるように。webserviceやchromeなども対象」）: 今 画面に出ている全部の app の窓
    /// （helper の `list` の on_screen の物・Shibubu 自身も入れる = 呼ぶ側が pid で外す）を、前から後ろの順（macOS の窓の重なりの順・読めない窓は後ろ）で返す。
    /// 1 つ = (窓・題〈空もある〉・frame = [x, y, w, h]〈画面の左上が原点の point = `pointer_now`・`slot_on_screen` と同じ〉)。
    /// Dock・menu bar・80×60 より小さい窓は helper の `windowList` が先に落とす（layer 0 だけ）。app 別の枝は無い（browser の窓も同じ 1 行）。
    /// 使う所: 空の枠の `Choose a window` の一覧（Shibubu の pid を外す）・引いて入れた窓の判定（`dropped_window` = 前の窓と Shibubu の重なり）。
    /// 例（裏の thread）: `let list = MirrorTarget::windows(&helper)?;`
    /// 保証する: 開いている面を落とさない（`titles` と同じ live の helper の stdin）・窓が 0 なら `Ok(vec![])`。
    /// 失敗: helper が起きない・画面収録の許可が無い = `find` と同じ `MirrorError`。
    /// Wave: MIRROR-FRAME-1101
    pub fn windows(helper: &Path) -> Result<Vec<(MirrorTarget, String, [f64; 4])>, MirrorError> {
        let list = list_windows(helper)?;
        let num = |w: &Value, k: &str| w["frame"][k].as_f64().unwrap_or(0.0);
        let mut out: Vec<(MirrorTarget, String, [f64; 4])> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w["on_screen"].as_bool() == Some(true))
            .filter_map(|w| {
                let (window_id, pid) = (w["window_id"].as_u64()?, w["pid"].as_i64()?);
                let bundle_id = w["bundle_id"].as_str().unwrap_or("").to_string();
                let app_name = w["app"].as_str().filter(|a| !a.is_empty()).unwrap_or(bundle_id.as_str()).to_string();
                let title = w["title"].as_str().unwrap_or("").trim().to_string();
                Some((MirrorTarget { window_id, pid, bundle_id, app_name }, title, [num(w, "x"), num(w, "y"), num(w, "w"), num(w, "h")]))
            })
            .collect();
        // 前から後ろ（macOS の窓の重なりの順）。順の読めない窓は後ろ（その間は helper の順のまま = 安定な sort）
        let order = window_order();
        out.sort_by_key(|(t, _, _)| order.iter().position(|&id| id == t.window_id).unwrap_or(usize::MAX));
        Ok(out)
    }

    /// MIRROR-FRAME-1101: 窓ごとの小さな絵（`Choose a window` の一覧の行の左）。常駐の helper（ready で `shot` を言った版）に 1 回の `{"cmd":"shot"}` で頼み、
    /// 幅 `width` px の PNG を (窓の id, PNG の byte) で返す（順は helper の撮れた順）。
    /// 保証する: 絵は Shibubu 専用の一時 dir に書かせ、読んだら dir ごと消す（画面の中身を disk に残さない）・helper の process を新しく起こさない（`windows` の後に呼ぶ）。
    /// 失敗は無い（空 / 足りない）: 撮れない窓・`shot` を知らない helper・常駐が居ない・許可が無い・答えない = その窓は入らない
    /// （行は灰の地のまま = 絵は飾りで、選ぶ事を止めない）。
    /// 例（裏の thread・`windows` の後）: `let shots = MirrorTarget::shots(&helper, &[70, 71], 112);`
    /// Wave: MIRROR-FRAME-1101
    pub fn shots(helper: &Path, windows: &[u64], width: u32) -> Vec<(u64, Vec<u8>)> {
        static N: AtomicU64 = AtomicU64::new(0);
        let Some(p) = capable(helper).filter(|p| p.routes.lock().unwrap().shot) else { return Vec::new() };
        let dir = std::env::temp_dir().join(format!("shibubu-shots-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        if windows.is_empty() || std::fs::create_dir(&dir).is_err() {
            return Vec::new();
        }
        let ans = p.ask(json!({"cmd": "shot", "windows": windows, "width": width, "dir": dir.display().to_string()}), LIST_WAIT);
        let ids = ans.ok().and_then(|a| a["shots"].as_array().cloned()).unwrap_or_default();
        let out = ids.iter().filter_map(|id| id.as_u64()).filter_map(|id| Some((id, std::fs::read(dir.join(format!("{id}.png"))).ok()?))).collect();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    /// Wave: W16M-1, W16M-3
    pub fn find(helper: &Path, bundle_id: &str, title_has: Option<&str>, want: Option<&Path>, ask: bool) -> Result<MirrorTarget, MirrorError> {
        let list = list_windows(helper)?;
        let me = std::process::id() as i64;
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let want = want.map(canon);
        let mut app_name = None;
        // 書類を引けなかった窓（別の書類とは分からない）と引けない理由
        let mut unknown: Vec<(MirrorTarget, String)> = Vec::new();
        // G78: 時間切れで問い直した起動（pid）
        let mut woken: Vec<i64> = Vec::new();
        for w in list.as_array().into_iter().flatten() {
            if w["bundle_id"].as_str() != Some(bundle_id) || w["pid"].as_i64() == Some(me) {
                continue;
            }
            let name = w["app"].as_str().unwrap_or(bundle_id).to_string();
            app_name.get_or_insert_with(|| name.clone());
            let title = w["title"].as_str().unwrap_or("");
            // 題名の無い窓（常駐 app の小窓・panel）は選ばない
            if title.trim().is_empty() || title_has.is_some_and(|t| !title.contains(t)) {
                continue;
            }
            let (Some(window_id), Some(pid)) = (w["window_id"].as_u64(), w["pid"].as_i64()) else { continue };
            let t = MirrorTarget { window_id, pid, bundle_id: bundle_id.into(), app_name: name };
            let Some(want) = &want else { return Ok(t) };
            // 古い helper（doc-path が無い）・許可が無い・名前の無い書類は None
            let answer = match doc_path(helper, pid, window_id, ask) {
                // G78（#5・W15 b74 の実測）: 背景に置かれた app（App Nap）は最初の AppleEvent に答えず helper の 5 秒で ae_-1712、その問いで起きて直後の問いには
                // 答える。時間切れの時だけ同じ窓にもう 1 回問う。起動（pid）ごとに 1 回だけ = 足す待ちは起動ごとに helper の時間切れ 1 回分（5 秒）まで。
                // 許可（not_permitted_*）・app が居ない（ae_-600）は問い直しても変わらないので問い直さない
                Ok(v) if v["reason"] == "ae_-1712" && !woken.contains(&pid) => {
                    woken.push(pid);
                    doc_path(helper, pid, window_id, ask)
                }
                a => a,
            };
            let doc = answer.as_ref().ok().and_then(|v| v["path"].as_str().map(|p| canon(Path::new(p))));
            match doc {
                Some(d) if &d == want => return Ok(t),
                Some(_) => {}
                // ponytail: AX の AXDocument で引く道は、背景の TextEdit が AX に答えない（M2）ので足していない
                None => {
                    // 引けない理由（.app に TextEdit を操作する許可が無い not_permitted_* など）は画面に出さないので stderr に（BG の run.sh は app.log に取る・M4 ③）
                    let why = match &answer { Ok(v) => v["reason"].as_str().unwrap_or("").to_string(), Err(e) => e.to_string() };
                    eprintln!("M4: Mirror: 窓 {window_id}（pid {pid}）の書類を引けません: {why}");
                    unknown.push((t, why));
                }
            }
        }
        match unknown.len() {
            0 => {}
            1 => return Ok(unknown.remove(0).0),
            count => {
                let t = &unknown[0].0;
                // -1743 = 断られた（もう尋ねられない）・-1744 = まだ尋ねていない（AEDeterminePermissionToAutomateTarget）
                let denied = unknown.iter().any(|(_, why)| why == "not_permitted_-1743");
                let ask_pid = (!denied && unknown.iter().all(|(_, why)| why == "not_permitted_-1744")).then_some(t.pid);
                return Err(MirrorError::Ambiguous { app_name: t.app_name.clone(), title_has: title_has.map(str::to_string), count, ask_pid, denied });
            }
        }
        Err(MirrorError::NoWindow {
            // 窓が 1 つも無い時の名前は bundle id の末尾（com.apple.TextEdit → TextEdit）
            app_name: app_name.unwrap_or_else(|| bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_string()),
            title_has: title_has.map(str::to_string),
        })
    }
}

/// MIRROR-FRAME-1101: 画面に出ている窓の CG の番号を前から後ろの順で（`CGWindowListCreate`・読むだけ・許可は要らない・何も動かさない）。読めなければ空。
#[cfg(target_os = "macos")]
fn window_order() -> Vec<u64> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreate(option: u32, relative_to: u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }
    // 理由: crate を足さずに窓の重なりの順を読む。前提: option 1 = kCGWindowListOptionOnScreenOnly・relative_to 0 = kCGNullWindowID・返る CFArray の値は
    // CGWindowID を pointer の幅に入れた整数（前から後ろ・callbacks なし）・返した物は CFRelease
    unsafe {
        let a = CGWindowListCreate(1, 0);
        if a.is_null() {
            return Vec::new();
        }
        let ids = (0..CFArrayGetCount(a)).map(|i| CFArrayGetValueAtIndex(a, i) as usize as u64).collect();
        CFRelease(a);
        ids
    }
}

#[cfg(not(target_os = "macos"))]
fn window_order() -> Vec<u64> {
    Vec::new()
}

/// helper の `list --all`（capture できる窓）。G32: 同じ helper の面が開いている間はその process の stdin で引く（開いている面を落とさない）。
/// MIRROR-QUIET-1101: 開いている面が 0 の間も常駐（`resident`）の stdin で引く（探すたびに process を起こさない）。`serve` を知らない helper だけ 1 回きりの `list --all`
fn list_windows(helper: &Path) -> Result<Value, MirrorError> {
    // ponytail: `serve` を知らない helper では、面が 0 の時の list と同時に起きた 1 面目の open は重なりうる（その面は 1 回落ちて次の探し直しで直る）
    match resident(helper)? {
        Some(p) => p.list(true),
        None => {
            let out = Command::new(helper)
                .args(["list", "--all"])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| MirrorError::Helper(format!("{}: {e}", helper.display())))?;
            // 許可が無い時の list は `[]` と exit 3（KangaeMirror.swift の list）。NoWindow にすると「窓を開いて」と嘘の次の一手になる
            if out.status.code() == Some(3) {
                return Err(MirrorError::ScreenCaptureDenied);
            }
            serde_json::from_slice(&out.stdout).map_err(|_| MirrorError::Helper(format!("Can't read the list answer ({})", out.status)))
        }
    }
}

/// 窓 `window`（pid `pid`）の書類の path を問う（`helper doc-path` の答えの形）。MIRROR-QUIET-1101: stdin の doc-path に答える process が在ればその stdin で、
/// 無ければ 1 回きりの `doc-path`。`ask` = macOS に許可を尋ねさせる（人が答えるまで `ASK_WAIT` 待つ）
fn doc_path(helper: &Path, pid: i64, window: u64, ask: bool) -> Result<Value, MirrorError> {
    if let Some(p) = capable(helper) {
        return p.ask(json!({"cmd": "doc-path", "pid": pid, "window": window, "ask": ask}), if ask { ASK_WAIT } else { LIST_WAIT });
    }
    let (p, w) = (pid.to_string(), window.to_string());
    let mut args = vec!["doc-path", "--pid", &p, "--window", &w];
    if ask {
        args.push("--ask");
    }
    helper_json(helper, &args)
}

/// MIRROR-QUIET-1101: その helper の生きている process のうち stdin の status / doc-path に答える物（起こさない・待たない＝UI の thread からも呼べる）
fn capable(helper: &Path) -> Option<Arc<HelperProc>> {
    live_proc(&mut PROCS.lock().unwrap(), helper).filter(|p| p.routes.lock().unwrap().caps)
}

// ---- 映像（W16M-1）----

/// helper が送った 1 frame。pixel は RGBA（helper が BGRA から並べ替え・行の余白を詰めた物）で、そのまま `egui::ColorImage::from_rgba_unmultiplied` に渡せる。
/// 保証する: `surface` が None なら `rgba.len() == meta.width * meta.height * 4`（`parse_frame` が照合する）・Some なら `rgba` は空で `surface` の大きさが
/// `meta.width × meta.height`（`read_surfaces` が照合する）。保証しない: 元の窓の今の中身（frame は `meta.captured_ms` の 1 枚）。
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorFrame {
    pub meta: MirrorFrameMeta,
    pub rgba: Vec<u8>,
    /// W16M-1b: 読み手の thread がこの frame を helper の stdout から受け取った時刻（unix ms・`read_helper` が置く）。
    /// `parse_frame` だけで作った frame は 0（header に無い値なので `MirrorFrameMeta` には入れない）。
    pub received_ms: f64,
    /// MIRROR-SPEED-1101: helper が IOSurface を mach port で渡した frame（`--surfaces`・写し 0 回）。画素はこの IOSurface（BGRA）で、描く側は GPU の
    /// texture にそのまま載せる（`ui::mirror_view`）・載せられない時だけ `IoSurface::to_rgba`。pipe の tag 1 の frame（前の版の helper・試験の偽物・
    /// 受け口を作れなかった起動）は None。
    pub surface: Option<Arc<IoSurface>>,
}

/// MIRROR-SPEED-1101: helper から mach port で受け取った IOSurface 1 枚（SCK が撮った BGRA の画素・GPU の memory を process の間で共有）。
/// 持っている間は use count を 1 つ上げる（SCK の pool がこの面に次の frame を書かない = 描いている途中の絵が替わらない）・Drop で下げて放す。
/// 新しい型の理由: `MirrorFrame` の画素は `Vec<u8>`（pipe の写し）しか無く、写さずに渡す物を入れる所が無い。
/// 例: `if let Some(s) = &f.surface { let (w, h) = s.size(); metal_texture(device, s.raw(), w, h) } else { ColorImage::from_rgba_unmultiplied(.., &f.rgba) }`
/// 保証する: Send + Sync（IOSurface は process と thread をまたいで読むための物・ここは読むだけ）・同じ面は `id` が同じ（SCK の pool の使い回し）。
/// 保証しない: 中身が今の元の窓である事（frame は `meta.captured_ms` の 1 枚）。macOS 以外は作られない。
/// Wave: MIRROR-SPEED-1101
pub struct IoSurface {
    raw: *mut std::ffi::c_void,
}

// SAFETY: IOSurfaceRef は CF の型で thread をまたいで使える（画素を読む時は `to_rgba` が IOSurfaceLock を取る）
unsafe impl Send for IoSurface {}
unsafe impl Sync for IoSurface {}

impl PartialEq for IoSurface {
    fn eq(&self, other: &IoSurface) -> bool {
        self.id() == other.id()
    }
}

impl std::fmt::Debug for IoSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.size();
        write!(f, "IoSurface({} {w}×{h})", self.id())
    }
}

/// MIRROR-SPEED-1101: IOSurface と mach の C の口（macOS の SDK の IOSurface.h・mach/message.h・servers/bootstrap.h）
#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::{c_char, c_void};
    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
        pub fn IOSurfaceLookupFromMachPort(port: u32) -> *mut c_void;
        pub fn IOSurfaceIncrementUseCount(s: *mut c_void);
        pub fn IOSurfaceDecrementUseCount(s: *mut c_void);
        pub fn IOSurfaceGetID(s: *mut c_void) -> u32;
        pub fn IOSurfaceGetWidth(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetHeight(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetBytesPerRow(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetBaseAddress(s: *mut c_void) -> *mut c_void;
        pub fn IOSurfaceLock(s: *mut c_void, options: u32, seed: *mut u32) -> i32;
        pub fn IOSurfaceUnlock(s: *mut c_void, options: u32, seed: *mut u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(cf: *const c_void);
    }
    // libSystem（mach と launchd の bootstrap）
    extern "C" {
        pub static bootstrap_port: u32;
        pub static mach_task_self_: u32;
        pub fn bootstrap_check_in(bp: u32, name: *const c_char, sp: *mut u32) -> i32;
        pub fn mach_msg(msg: *mut c_void, option: i32, send_size: u32, rcv_size: u32, rcv_name: u32, timeout: u32, notify: u32) -> i32;
        pub fn mach_msg_destroy(msg: *mut c_void);
        pub fn mach_port_deallocate(task: u32, name: u32) -> i32;
        pub fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: i32) -> i32;
    }
}

#[cfg(target_os = "macos")]
impl IoSurface {
    /// IOSurface の id（process をまたいで同じ・GPU の texture の登録の鍵）
    pub fn id(&self) -> u32 {
        unsafe { ffi::IOSurfaceGetID(self.raw) }
    }
    /// 幅と高さ（pixel）
    pub fn size(&self) -> (u32, u32) {
        unsafe { (ffi::IOSurfaceGetWidth(self.raw) as u32, ffi::IOSurfaceGetHeight(self.raw) as u32) }
    }
    /// IOSurfaceRef（`newTextureWithDescriptor:iosurface:plane:` に渡す・借りるだけ = release しない）
    pub fn raw(&self) -> *mut std::ffi::c_void {
        self.raw
    }
    /// CPU で 1 回写した RGBA（GPU の texture に載せられない時の控え・行の余白を詰めて BGRA → RGBA）。lock できない・大きさが合わない = None
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        const READ_ONLY: u32 = 1; // kIOSurfaceLockReadOnly
        unsafe {
            if ffi::IOSurfaceLock(self.raw, READ_ONLY, std::ptr::null_mut()) != 0 {
                return None;
            }
            let (w, h) = (ffi::IOSurfaceGetWidth(self.raw), ffi::IOSurfaceGetHeight(self.raw));
            let (base, row) = (ffi::IOSurfaceGetBaseAddress(self.raw) as *const u8, ffi::IOSurfaceGetBytesPerRow(self.raw));
            let mut out = Vec::with_capacity(w * h * 4);
            if !base.is_null() && row >= w * 4 {
                for y in 0..h {
                    for px in std::slice::from_raw_parts(base.add(y * row), w * 4).chunks_exact(4) {
                        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    }
                }
            }
            ffi::IOSurfaceUnlock(self.raw, READ_ONLY, std::ptr::null_mut());
            (out.len() == w * h * 4).then_some(out)
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for IoSurface {
    /// use count を下げて放す（SCK の pool がこの面を次の frame に使える）
    fn drop(&mut self) {
        unsafe {
            ffi::IOSurfaceDecrementUseCount(self.raw);
            ffi::CFRelease(self.raw);
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl IoSurface {
    pub fn id(&self) -> u32 {
        0
    }
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }
    pub fn raw(&self) -> *mut std::ffi::c_void {
        self.raw
    }
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        None
    }
}

/// MIRROR-SPEED-1101: helper の撮る枚数（`model::MirrorSmoothness::arg` の語・既定 auto）。起こす helper の `--smooth` と、動いている helper の stdin の
/// `{"cmd":"smoothness","mode":…}` に渡す（`set_smoothness`）
static SMOOTH: Mutex<&'static str> = Mutex::new("auto");

/// MIRROR-SPEED-1101（owner 205）: Settings › General の `Mirror smoothness` を helper に渡す（app.rs の当て直しが起動の 1 回と `SettingsChanged` で呼ぶ）。
/// 例: `mirror::set_smoothness(store.ui.mirror_smoothness.arg())`。`mode` = "auto" | "smooth" | "saver"（他の語は helper が `bad_smoothness` で断る）。
/// 保証する: 次に起こす helper は `--smooth <mode>`・動いている helper（面の process と常駐）には stdin の 1 行で（起こし直さない・待たない）・同じ値なら何も送らない。
/// 保証しない: `--fps` を渡して起こした stream（PoC の measure）の枚数・helper が受け取った事（答えは無い・stats の `fps` で分かる）。
/// Wave: MIRROR-SPEED-1101
pub fn set_smoothness(mode: &'static str) {
    {
        let mut now = SMOOTH.lock().unwrap();
        if *now == mode {
            return;
        }
        *now = mode;
    }
    // 動いている helper（面の process と常駐）へ。Arc は PROCS の lock の外で落とす（最後の 1 つなら HelperProc の Drop が process を止める）
    let procs: Vec<Arc<HelperProc>> = {
        let mut procs = PROCS.lock().unwrap();
        procs.retain(|(_, w)| w.strong_count() > 0);
        procs.iter().filter_map(|(_, w)| w.upgrade()).collect()
    };
    for p in procs.iter().filter(|p| !p.routes.lock().unwrap().dead) {
        let _ = p.send(&json!({"cmd": "smoothness", "mode": mode}));
    }
}

/// MIRROR-SPEED-1101: 「撮った → 描いた」の ms の中央値と 95% 点（最も近い順位・並べ替えるので `ms` の順は崩れる）。空なら None（0 ms と言わない）。
/// 例: `if let Some((p50, p95)) = percentiles(&mut ms) { eprintln!("… {p50:.0} ms · {p95:.0} ms") }`（`MirrorPages` が 1 分ごとに app.log へ）
/// Wave: MIRROR-SPEED-1101
pub fn percentiles(ms: &mut [f64]) -> Option<(f64, f64)> {
    if ms.is_empty() {
        return None;
    }
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((q * ms.len() as f64).ceil() as usize).clamp(1, ms.len()) - 1];
    Some((at(0.5), at(0.95)))
}

/// MIRROR-SPEED-1101: IOSurface の受け口を 1 つ作る（`bootstrap_check_in` の名前 `ai.shibubu.desktop.mirror.<pid>.<n>` と、その受信の権利の port）。
/// `HelperProc::spawn` が helper ごとに 1 つ作り、helper に `--surfaces <名前>` で渡す。作れない時は None（`--surfaces` を渡さない = pipe の RGBA のまま）。
#[cfg(target_os = "macos")]
fn inbox() -> Option<(String, u32)> {
    static N: AtomicU64 = AtomicU64::new(0);
    static SAID: std::sync::Once = std::sync::Once::new();
    let name = format!("ai.shibubu.desktop.mirror.{}.{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let c = std::ffi::CString::new(name.as_str()).ok()?;
    let mut port = 0u32;
    // SAFETY: bootstrap_port は libSystem が置くこの process の bootstrap の port・name は NUL で終わる 128 byte 未満
    let kr = unsafe { ffi::bootstrap_check_in(ffi::bootstrap_port, c.as_ptr(), &mut port) };
    if kr != 0 {
        SAID.call_once(|| eprintln!("MIRROR-SPEED-1101: IOSurface の受け口を作れません（bootstrap_check_in {kr}）。Mirror の画素は pipe で受けます"));
        return None;
    }
    Some((name, port))
}

#[cfg(not(target_os = "macos"))]
fn inbox() -> Option<(String, u32)> {
    None
}

/// MIRROR-SPEED-1101: 受け口を閉じる（受信の権利を捨てる = helper の次の送りは失敗して pipe に戻る）
#[cfg(target_os = "macos")]
fn close_inbox(port: u32) {
    // SAFETY: port はこの process が bootstrap_check_in で受け取った受信の権利（MACH_PORT_RIGHT_RECEIVE = 1）
    unsafe { ffi::mach_port_mod_refs(ffi::mach_task_self_, port, 1, -1) };
}

#[cfg(not(target_os = "macos"))]
fn close_inbox(_port: u32) {}

/// MIRROR-SPEED-1101: 受け口 `port` を読み続け、helper（pid `pid`）が送った 1 通（KangaeMirror.swift の頭の `--surfaces` の形）の IOSurface を
/// header の `"stream"` の `Sink` へ（`MirrorFrame::surface`・`rgba` は空）。形の違う 1 通・送り手の pid が helper でない 1 通は持っている権利ごと捨てる。
/// `routes` が dead（helper の stdout が閉じた）になったら受け口を閉じて終わる（500ms ごとに見る）。
#[cfg(target_os = "macos")]
fn read_surfaces(port: u32, pid: u32, routes: Arc<Mutex<Routes>>) {
    // MACH_RCV_MSG | MACH_RCV_TIMEOUT | MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT)（送り手の pid を audit token で読む）
    const RCV: i32 = 0x0300_0102;
    const RCV_TIMED_OUT: i32 = 0x1000_4003;
    const RCV_TOO_LARGE: i32 = 0x1000_4004;
    const RCV_INTERRUPTED: i32 = 0x1000_4005;
    let mut buf = vec![0u32; 1024];
    while !routes.lock().unwrap().dead {
        // SAFETY: buf は 4096 byte（受ける大きさに渡す）・port はこの thread が持つ受信の権利
        let r = unsafe { ffi::mach_msg(buf.as_mut_ptr().cast(), RCV, 0, (buf.len() * 4) as u32, port, 500, 0) };
        match r {
            0 => {}
            // 500ms 来ない = dead を見直す・大き過ぎる 1 通は kernel が捨てた（header の JSON は数百 byte）・signal で起きた
            RCV_TIMED_OUT | RCV_TOO_LARGE | RCV_INTERRUPTED => continue,
            _ => {
                eprintln!("MIRROR-SPEED-1101: IOSurface の受け口を読めません（mach_msg {r:#x}）。この helper の画素は以後 pipe だけ");
                break;
            }
        }
        // SAFETY: buf は mach_msg がたった今書いた 1 通
        if let Some((f, stream)) = unsafe { surface_frame(&mut buf, pid) } {
            let sink = routes.lock().unwrap().streams.get(&stream).cloned();
            if let Some(s) = sink {
                s.put(f);
            }
        }
    }
    close_inbox(port);
}

#[cfg(not(target_os = "macos"))]
fn read_surfaces(_port: u32, _pid: u32, _routes: Arc<Mutex<Routes>>) {}

/// MIRROR-SPEED-1101: 受け取った 1 通 → (frame, header の stream)。形が違う・送り手が helper（`pid`）でない 1 通は持っている権利ごと捨てる（mach_msg_destroy）。
/// 形（KangaeMirror.swift の頭の `--surfaces`）: [0] bits（COMPLEX）・[1] 大きさ・[5] id 0x5348・[6] 記述子の数 1・[7] IOSurface の port・[9] の上の 8 bit = port 記述子（0）・
/// [10] JSON の長さ・44 byte から JSON・4 byte に揃えた所から audit の trailer（[1] trailer の大きさ ≥ 52・[10] audit token の pid）
/// SAFETY（呼ぶ側）: `buf` は mach_msg が書いた 1 通（`buf[1]` の後ろに MACH_RCV_TRAILER_AUDIT の trailer）
#[cfg(target_os = "macos")]
unsafe fn surface_frame(buf: &mut [u32], pid: u32) -> Option<(MirrorFrame, u64)> {
    let size = buf[1] as usize;
    let t = size.div_ceil(4);
    let ours = buf[0] & 0x8000_0000 != 0 && buf[5] == 0x5348 && size >= 44 && t + 13 <= buf.len() && buf[6] == 1 && buf[9] >> 24 == 0;
    if !(ours && buf[t + 1] >= 52 && buf[t + 10] == pid) {
        ffi::mach_msg_destroy(buf.as_mut_ptr().cast());
        return None;
    }
    let raw = ffi::IOSurfaceLookupFromMachPort(buf[7]);
    ffi::mach_port_deallocate(ffi::mach_task_self_, buf[7]);
    if raw.is_null() {
        return None;
    }
    ffi::IOSurfaceIncrementUseCount(raw);
    // ここから先で返さない時は surface の Drop が use count を下げて放す
    let surface = IoSurface { raw };
    let len = buf[10] as usize;
    let bytes = std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size);
    let h: Value = bytes.get(44..44 + len).and_then(|b| serde_json::from_slice(b).ok())?;
    let meta = frame_meta(&h).ok()?;
    if surface.size() != (meta.width, meta.height) {
        return None;
    }
    let stream = h["stream"].as_u64().unwrap_or(1);
    Some((MirrorFrame { meta, rgba: Vec::new(), received_ms: 0.0, surface: Some(Arc::new(surface)) }, stream))
}

/// helper の tag 1 の payload（`[header 長 u32 LE][header JSON][RGBA]`・KangaeMirror.swift の `stream`）を 1 frame に。
/// 例: `let f = parse_frame(payload)?; texture.set(ColorImage::from_rgba_unmultiplied([f.meta.width as usize, f.meta.height as usize], &f.rgba), ..)`
/// 保証する: header の欄（seq・w・h・captured_ms・content）が欠けた物・RGBA の長さが w × h × 4 と違う物は Err（黒や崩れた texture を作らない）。factor が無ければ 1。
pub fn parse_frame(mut payload: Vec<u8>) -> Result<MirrorFrame, MirrorError> {
    let bad = |why: &str| MirrorError::Helper(format!("The frame has the wrong format ({why})"));
    let hl = payload
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or_else(|| bad("The header length is missing"))?;
    let h: Value = payload
        .get(4..4 + hl)
        .and_then(|b| serde_json::from_slice(b).ok())
        .ok_or_else(|| bad("Can't read the header JSON"))?;
    let meta = frame_meta(&h)?;
    payload.drain(..4 + hl);
    if payload.len() != meta.width as usize * meta.height as usize * 4 {
        return Err(bad(&format!("The image has {} bytes, not {}×{}×4", payload.len(), meta.width, meta.height)));
    }
    Ok(MirrorFrame { meta, rgba: payload, received_ms: 0.0, surface: None })
}

/// MIRROR-SPEED-1101: frame の header JSON（pipe の tag 1 と mach の 1 通が同じ形・KangaeMirror.swift の `stream`）→ meta。
/// 欄（seq・w・h・captured_ms・content）が欠けた物は Err（0 で埋めない）・factor が無ければ 1
fn frame_meta(h: &Value) -> Result<MirrorFrameMeta, MirrorError> {
    let bad = |why: &str| MirrorError::Helper(format!("The frame has the wrong format ({why})"));
    let num = |v: &Value, k: &str| v[k].as_f64().ok_or_else(|| bad(&format!("The header has no {k}")));
    let c = &h["content"];
    Ok(MirrorFrameMeta {
        seq: h["seq"].as_u64().ok_or_else(|| bad("The header has no seq"))?,
        width: num(h, "w")? as u32,
        height: num(h, "h")? as u32,
        captured_ms: num(h, "captured_ms")?,
        content: [num(c, "x")?, num(c, "y")?, num(c, "w")?, num(c, "h")?],
        factor: h["factor"].as_f64().unwrap_or(1.0),
    })
}

// ---- 映像の遅れの記録（W16M-1b・E0 の残りの赤の切り分け）----

/// texture の更新の記録（`FrameLog` の JSONL の 1 行）を書く先を指す環境変数。無ければ書かない（製品の既定は記録 0 件）。
/// 試験の側が `open -g --env KANGAE_MIRROR_FRAME_LOG=<evidence の下>` で渡す。
pub const FRAME_LOG_ENV: &str = "KANGAE_MIRROR_FRAME_LOG";
/// texture の更新の記録の版。
pub const FRAME_SCHEMA_V: u32 = 1;

/// Mirror の矩形の texture を 1 回更新した記録（`MirrorPages::show` が `texture.set` の直後に書く）。
/// 例: `let recs = FrameLog::read(&path)?; let l = frame_latency(&recs, "notes", changed_at)?;`
/// 保証する: 1 行 = texture の更新 1 回・時刻は全部 unix ms（captured = helper の SCK の displayTime の写し・received = 読み手の thread・
/// shown = UI の thread で texture を上げた時）。保証しない: 画面に実際に出た時刻（swap の後）・frame の中身が今の元の窓である事（E0 の OCR）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRecord {
    pub v: u32,
    /// `SurfaceSpec.id`（矩形の AX id `surface-frame:<surface_id>` と同じ）
    pub surface_id: String,
    pub seq: u64,
    pub captured_ms: f64,
    pub received_ms: f64,
    pub shown_ms: f64,
}

/// texture の更新の記録の置き場（`FRAME_LOG_ENV`）。無ければ None ＝ 書かない。
pub fn frame_log_path() -> Option<PathBuf> {
    std::env::var_os(FRAME_LOG_ENV).map(PathBuf::from)
}

/// TextEdit に書いた時刻から texture の更新までの遅れを 3 つに分けた物（どこで遅れたかを決める）。
/// helper の側 = `capture_ms + pipe_ms`・Shibubu の描き直しの側 = `redraw_ms`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameLatency {
    /// 書いた後に撮った最初の frame の seq
    pub seq: u64,
    /// 書いた → SCK が撮った（captured_ms − changed_at）
    pub capture_ms: f64,
    /// 撮った → 読み手の thread が受け取った（received_ms − captured_ms）
    pub pipe_ms: f64,
    /// 受け取った → texture に上げた（shown_ms − received_ms・App Nap・隠れた窓の描き直しの間引きはここに出る）
    pub redraw_ms: f64,
    /// 書いた → texture に上げた（shown_ms − changed_at）
    pub total_ms: f64,
}

/// E0 の製品の側の記録（純関数）: `surface_id` の記録のうち `changed_at_ms` 以後に撮った（captured_ms ≥ changed_at_ms）最初の texture の更新の遅れ。
/// 例: `let l = frame_latency(&FrameLog::read(&p)?, "notes", changed_at)?; println!("{l:?}")`
/// 保証する: その記録が無ければ Err（「未測」の語・映像が止まった / log が無いを 0 遅れの緑にしない）。
/// 保証しない: 合否（E0 の合格は試験の側の OCR の写真が要る・`tests/common/readback.rs` の `e0_verdict`）・captured の時計の写しの誤差。
pub fn frame_latency(records: &[FrameRecord], surface_id: &str, changed_at_ms: f64) -> Result<FrameLatency, String> {
    let r = records
        .iter()
        .filter(|r| r.surface_id == surface_id && r.captured_ms >= changed_at_ms)
        .min_by(|a, b| a.shown_ms.total_cmp(&b.shown_ms))
        .ok_or_else(|| format!("Not measured: no record that a frame taken after writing to {surface_id} was uploaded to the texture (the video is stalled and there is no log)"))?;
    Ok(FrameLatency {
        seq: r.seq,
        capture_ms: r.captured_ms - changed_at_ms,
        pipe_ms: r.received_ms - r.captured_ms,
        redraw_ms: r.shown_ms - r.received_ms,
        total_ms: r.shown_ms - changed_at_ms,
    })
}

// ---- 背景の間引きを止める（W16M-1b の直しの候補・公開 API の NSProcessInfo）----

/// Mirror を開いている間だけ App Nap を止める印（`NSProcessInfo beginActivityWithOptions:reason:`・Drop で `endActivity:`）。
/// 例: `MirrorPages::end_frame` が毎 frame の終わりに、Mirror の Page を描いて開いた面が在る間だけ持ち、それ以外は None にする（毎 frame begin / end しない）
/// 保証する: 持っている間 `NSActivityUserInitiatedAllowingIdleSystemSleep` が効く・Drop で必ず終える（Mirror の Page を離れた frame の終わりで放して間引きを戻す）。macOS 以外は None。
/// 実測（W16M-1b・docs/evidence/W16M-1b/log.md）: 背景の Shibubu は 30〜60 秒で描き直しが約 21 秒に 1 回まで間引かれた（frame は毎秒 10〜16 枚着いていた）。
/// これを持つと着いた frame がほぼ全部 texture に上がる（隠れた窓のままでも）。frame の着いた時の起こし（`on_frame`）は要らなかったので消した。
/// 保証しない: macOS が activity を断った時（None）の間引き（呼ぶ側は今まで通り 33ms ごとの描き直しを頼むだけ）。
#[derive(Debug)]
pub struct ActivityHold {
    /// `beginActivity…` の返した id<NSObject>（retain 済み・Drop で endActivity: と release）
    token: usize,
}

/// `NSActivityUserInitiatedAllowingIdleSystemSleep`（NSProcessInfo.h: `(0x00FFFFFF | 1<<20) & ~(1<<20)`）。
#[cfg(target_os = "macos")]
const NS_ACTIVITY_USER_INITIATED_ALLOWING_IDLE_SYSTEM_SLEEP: u64 = 0x00FF_FFFF & !(1 << 20);

#[cfg(target_os = "macos")]
mod objc {
    use std::ffi::{c_char, c_void, CStr};
    // NSProcessInfo は Foundation・objc_msgSend は libobjc（#[path] で取り込む試験の crate は eframe を link しないので自分で link する）
    #[link(name = "Foundation", kind = "framework")]
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    pub type Id = *mut c_void;
    pub unsafe fn class(name: &CStr) -> Id {
        objc_getClass(name.as_ptr())
    }
    pub unsafe fn sel(name: &CStr) -> Id {
        sel_registerName(name.as_ptr())
    }
    /// 引数なしで id を返す message（main.rs と同じ形: 引数が pointer / 整数だけなら素の C の呼び方）。
    pub unsafe fn send0(o: Id, s: &CStr) -> Id {
        let f: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s))
    }
    pub unsafe fn send_p(o: Id, s: &CStr, a: *const c_void) -> Id {
        let f: unsafe extern "C" fn(Id, Id, *const c_void) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s), a)
    }
    pub unsafe fn send_u_p(o: Id, s: &CStr, a: u64, b: Id) -> Id {
        let f: unsafe extern "C" fn(Id, Id, u64, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(o, sel(s), a, b)
    }
}

impl ActivityHold {
    #[cfg(target_os = "macos")]
    pub fn begin(reason: &str) -> Option<ActivityHold> {
        let reason = std::ffi::CString::new(reason.replace('\0', " ")).ok()?;
        unsafe {
            // autorelease pool の無い thread（試験）でも漏らさないよう alloc + init の NSString を使い、渡した後に release
            let s = objc::send0(objc::class(c"NSString"), c"alloc");
            let s = objc::send_p(s, c"initWithUTF8String:", reason.as_ptr().cast());
            if s.is_null() {
                return None;
            }
            let info = objc::send0(objc::class(c"NSProcessInfo"), c"processInfo");
            let token = objc::send_u_p(info, c"beginActivityWithOptions:reason:", NS_ACTIVITY_USER_INITIATED_ALLOWING_IDLE_SYSTEM_SLEEP, s);
            objc::send0(s, c"release");
            if token.is_null() {
                return None;
            }
            // 返り値は +0（autorelease）。Drop まで持つので retain する
            objc::send0(token, c"retain");
            Some(ActivityHold { token: token as usize })
        }
    }
    #[cfg(not(target_os = "macos"))]
    pub fn begin(_reason: &str) -> Option<ActivityHold> {
        None
    }
}

impl Drop for ActivityHold {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe {
            let token = self.token as objc::Id;
            let info = objc::send0(objc::class(c"NSProcessInfo"), c"processInfo");
            objc::send_p(info, c"endActivity:", token);
            objc::send0(token, c"release");
        }
    }
}


// ---- 同意（YCM-063・未決 7-5）----

/// 同意の記録（`ui_state` の json）。型は W19-1 で `surface` に移した（AI の操作の同意と同じ型・feature `mirror` の外でも使う）。
pub use super::ConsentRecord;

/// 同意の証。field は private で、`ConsentGate::check` / `grant` からしか得られない。
/// 例: `let g = gate.check(&target)?; MirrorSurface::open(helper, target, g, log)?`。保証: その bundle id に同意の記録がある。しない: 許可（画面収録）の有無。
#[derive(Debug, Clone, PartialEq)]
pub struct ConsentGrant {
    bundle_id: String,
}

impl ConsentGrant {
    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }
}

/// app ごとの同意の表。読み書きは呼ぶ側が `ui_state` へ（この型は DB を知らない）。
/// 例: `let gate = ConsentGate::from_records(rows)`。保証: 記録の無い app には `ConsentGrant` を出さない。
#[derive(Debug, Clone, Default)]
pub struct ConsentGate {
    granted: HashMap<String, ConsentRecord>,
}

impl ConsentGate {
    pub fn from_records(records: impl IntoIterator<Item = ConsentRecord>) -> ConsentGate {
        ConsentGate {
            granted: records
                .into_iter()
                .map(|r| (r.bundle_id.clone(), r))
                .collect(),
        }
    }
    /// 同意があれば証を返す。無ければ `NoConsent`（同意の 1 行を出す）。
    pub fn check(&self, target: &MirrorTarget) -> Result<ConsentGrant, MirrorError> {
        match self.granted.get(&target.bundle_id) {
            Some(_) => Ok(ConsentGrant {
                bundle_id: target.bundle_id.clone(),
            }),
            None => Err(MirrorError::NoConsent {
                app_name: target.app_name.clone(),
            }),
        }
    }
    /// 人が同意の button（AX identifier `consent_button_id`）を押した時だけ呼ぶ。返す記録を呼ぶ側が `ui_state` に保存する。
    pub fn grant(&mut self, target: &MirrorTarget, now_ms: f64) -> (ConsentGrant, ConsentRecord) {
        let r = ConsentRecord {
            v: CONSENT_SCHEMA_V,
            bundle_id: target.bundle_id.clone(),
            granted_at_ms: now_ms,
        };
        self.granted.insert(r.bundle_id.clone(), r.clone());
        (
            ConsentGrant {
                bundle_id: r.bundle_id.clone(),
            },
            r,
        )
    }
    /// 同意を取り消す。在ったら true。開いている `MirrorSurface` は呼ぶ側が閉じる。
    pub fn revoke(&mut self, bundle_id: &str) -> bool {
        self.granted.remove(bundle_id).is_some()
    }
}

/// 同意の 1 行（D-e: 次の一手を 1 行）。
pub fn consent_prompt(app_name: &str) -> String {
    format!("Mirror shows {app_name}'s window here and sends your typing to it. macOS will ask for screen recording.")
}

/// 同意の button の AX identifier（試験の道具が BG で AXPress する）。
pub fn consent_button_id(bundle_id: &str) -> String {
    format!("mirror-consent:{bundle_id}")
}

// ---- relay の記録（E1 の相手）----

/// 人の入力 1 つの記録（JSONL の 1 行）。helper に送る前に書く。
/// 保証: 送る前の行は `MirrorSurface::{relay_human, relay_front}` だけが書く（自動の口は書かない）・答えの行（`outcome` 付き）は `note_answer` だけが書く。
/// しない: 元の app に届いた事（それは試験の側の読み戻し）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayRecord {
    pub v: u32,
    /// surface の中の通し番号（1 から）。helper の行の `relay_seq` と同じ。
    pub seq: u64,
    /// 書いた時刻（unix ms）。helper に送るより前。
    pub at_ms: f64,
    pub kind: InputKind,
    /// W16M-2: `route_for` が決めた道。`NeedsFront` の入力は記録を書かない（送らない）ので、ここに `needs_front` は来ない。
    pub route: InputRoute,
    /// 宛先の pid と窓 id。
    pub pid: i64,
    pub window_id: u64,
    /// Mirror の矩形の中の位置（窓の中の割合）。key と text は None。
    pub pos: Option<[f64; 2]>,
    /// MIRROR-HYBRID-1101 追記 235 の 3（owner「反応しない時もあるし」）: helper の答え（`delivered` = 送った・`refused: <理由>` = 送っていない）。送る前の行は None・
    /// 答えが着いたら同じ `seq` でもう 1 行（`at_ms` = 着いた時刻・`outcome` と `ms` 付き・`MirrorSurface::note_answer`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// 追記 235 の 3: 送る前の行の `at_ms` から、helper が送った時刻（`sent_ms`）か断った答えが着くまでの ms（front の道は本物が前に来るのを待った間を含む）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<f64>,
}

/// 追記 235 の 3: 答えを待つ送った記録の上限（古い物から捨てる・答えの来ない入力で増え続けない）
const ANSWER_CAP: usize = 64;

/// 追記 235 の 3: 送った記録 `rec` と helper の答え `ev`（`sent` / `error`）から答えの行（`outcome`・`ms`・`at_ms` = `now`）。答えでなければ None。純関数
fn answered(rec: &RelayRecord, ev: &Value, now: f64) -> Option<RelayRecord> {
    let (outcome, at) = match ev["kind"].as_str()? {
        "sent" => ("delivered".to_owned(), ev["sent_ms"].as_f64().unwrap_or(now)),
        "error" => (format!("refused: {}", ev["reason"].as_str().unwrap_or("no_reason")), now),
        _ => return None,
    };
    Some(RelayRecord { at_ms: now, outcome: Some(outcome), ms: Some((at - rec.at_ms).max(0.0)), ..rec.clone() })
}

/// 追記だけの JSONL の file（relay の記録 `RelayLog` と texture の更新の記録 `FrameLog` が同じ物を使う）。
/// 例: `RelayLog::open(Path::new(&std::env::var(RELAY_LOG_ENV)?))?`。
/// 保証: `append` が Ok なら 1 行が flush 済み。しない: 他の process との同時の追記の順。
#[derive(Debug)]
pub struct JsonlLog<T> {
    path: PathBuf,
    _rec: std::marker::PhantomData<fn(T)>,
}

/// relay の記録の file（W16M-0）。
pub type RelayLog = JsonlLog<RelayRecord>;
/// texture の更新の記録の file（W16M-1b・`FRAME_LOG_ENV` が在る時だけ書く）。
pub type FrameLog = JsonlLog<FrameRecord>;

impl<T: Serialize + serde::de::DeserializeOwned> JsonlLog<T> {
    pub fn open(path: &Path) -> Result<JsonlLog<T>, MirrorError> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| MirrorError::Log(format!("{}: {e}", path.display())))?;
        Ok(JsonlLog {
            path: path.to_path_buf(),
            _rec: std::marker::PhantomData,
        })
    }
    pub fn append(&mut self, record: &T) -> Result<(), MirrorError> {
        let line = serde_json::to_string(record).map_err(|e| MirrorError::Log(e.to_string()))?;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|e| MirrorError::Log(format!("{}: {e}", self.path.display())))?;
        writeln!(f, "{line}")
            .and_then(|_| f.flush())
            .map_err(|e| MirrorError::Log(format!("{}: {e}", self.path.display())))
    }
    /// 試験の側が読む。知らない field の行は Err（`deny_unknown_fields`）。
    pub fn read(path: &Path) -> Result<Vec<T>, MirrorError> {
        let f = std::fs::File::open(path)
            .map_err(|e| MirrorError::Log(format!("{}: {e}", path.display())))?;
        BufReader::new(f)
            .lines()
            .filter(|l| !matches!(l, Ok(l) if l.trim().is_empty()))
            .map(|l| {
                let l = l.map_err(|e| MirrorError::Log(e.to_string()))?;
                serde_json::from_str(&l).map_err(|e| MirrorError::Log(format!("{e}: {l}")))
            })
            .collect()
    }
}

/// relay の記録の置き場: `RELAY_LOG_ENV` → `super::data_dir()`（FX1）の `mirror-relay.jsonl`。
pub fn relay_log_path() -> PathBuf {
    if let Some(p) = std::env::var_os(RELAY_LOG_ENV) {
        return PathBuf::from(p);
    }
    super::data_dir().join("mirror-relay.jsonl")
}

/// helper を 1 回走らせて stdout の JSON 1 行を返す（`status`・`list`・`input`・`ax-*`）。
/// R-M: PoC の `helper_json`（`Result<Value, String>`）と `tests/mirror_e0.rs` の `helper_json`（panic）を消してこれを呼ぶ（private → pub・本体は同じ）。
/// 例: `helper_json(&helper, &["status"])?["screen_capture"] == json!(true)`・PoC は `.map_err(|e| e.to_string())`・試験は `.unwrap_or_else(|e| panic!("未測: {e}"))`。
/// 保証する: 起こせない・stdout が JSON でない時は `MirrorError::Helper`（文に exit status と stderr の最後の 1 行）。stdin は閉じる。
/// 保証しない: exit code を返す事（`MirrorTarget::find` は exit 3 = 許可なしを読むので自前の `Command` のまま）。
pub fn helper_json(helper: &Path, args: &[&str]) -> Result<Value, MirrorError> {
    let out = Command::new(helper)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| MirrorError::Helper(format!("{}: {e}", helper.display())))?;
    serde_json::from_slice(&out.stdout).map_err(|_| {
        MirrorError::Helper(format!(
            "Can't read the answer ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("")
        ))
    })
}

/// helper の error の reason → 画面の 1 行の種類。
fn helper_error(reason: &str) -> MirrorError {
    match reason {
        "window_gone" | "pid_changed" => MirrorError::WindowGone,
        "not_human_relay" | "human_origin_on_automated_port" => MirrorError::WrongPort {
            reason: reason.into(),
        },
        "no_screen_capture_permission" => MirrorError::ScreenCaptureDenied,
        r => MirrorError::Helper(r.into()),
    }
}

// ---- 1 本の helper（G32）----

/// stream 1 本の受け口（読み手の thread が置く最新の frame と以後ずっと返す Err・出来事の口）。`MirrorSurface` が反対の端を持つ。
#[derive(Debug, Clone)]
struct Sink {
    video: Arc<Mutex<Video>>,
    events: mpsc::SyncSender<Value>,
    /// G90 (3): この stream の app の表示名（`WindowHidden` の 1 行が言う）
    app_name: String,
}

/// 読み手の thread が helper の 1 通を宛先へ分ける表。stream の id は helper の行の `"stream"`（無い行は 1 = 起こした時の窓）。
#[derive(Debug, Default)]
struct Routes {
    streams: HashMap<u64, Sink>,
    /// `{"cmd":"list"}`・`status`・`doc-path` の答え（`{"kind":"windows"|"answer","id":R}`）を待つ口（`HelperProc::ask`）
    asks: HashMap<u64, mpsc::Sender<Value>>,
    /// helper の stdout が閉じた（process が終わった）。以後この process には頼まない
    dead: bool,
    /// MIRROR-QUIET-1101: helper が `{"kind":"ready"}` を言った（stdin の `status`・`doc-path` に答える版）。言わない前の版と偽物には 1 回きりの process で問う
    caps: bool,
    /// MIRROR-FRAME-1101: ready の caps に `shot` が在る（窓の小さな絵に答える版）。無い版には `MirrorTarget::shots` が頼まない
    shot: bool,
}

/// helper の process 1 本（`stream --window <最初の窓>`・MIRROR-QUIET-1101 の常駐は `serve`＝窓 0 本）。同じ helper の path の `MirrorSurface` が `Arc` で
/// 分け合い、最後の 1 つが落ちたら止める（常駐は `KEEP` も `Arc` を持つので面が 0 でも残る）。
/// 新しい型の理由: 前は `MirrorSurface` が child と stdin を 1 つずつ持ち、面ごとに process が起き、2 本目の process の SCShareableContent が
/// 1 本目の stream を -3805 で落とした（G29 4/4）。
#[derive(Debug)]
struct HelperProc {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    /// stream と list の id（1 は起こした時の窓）
    next: AtomicU64,
    routes: Arc<Mutex<Routes>>,
}

/// helper の path ごとの生きている process（`MirrorSurface` が `Arc` を持ち、ここは `Weak`＝面が無くなれば process も無い）。
static PROCS: Mutex<Vec<(PathBuf, Weak<HelperProc>)>> = Mutex::new(Vec::new());

/// `{"cmd":"list"}` の答えを待つ長さ（helper の SCShareableContent の期限 30 秒 + 余り）
const LIST_WAIT: std::time::Duration = std::time::Duration::from_secs(45);

/// MIRROR-QUIET-1101: helper の path ごとの常駐（`serve`）。面が 0 になっても `Arc` を持つ（`PROCS` の `Weak` が消えない）
#[derive(Default)]
struct Keep {
    proc: Option<Arc<HelperProc>>,
    /// 起こした時刻（落ちた時に「すぐ落ちた = 続けて落ちた」かを数える）
    born: Option<std::time::Instant>,
    /// 続けて落ちた回数・次に起こしてよい時刻・最後の落ち方（exit status）
    fails: u32,
    retry_at: Option<std::time::Instant>,
    why: String,
    /// None = まだ試していない・Some(false) = ready を言わずに終わった（`serve` を知らない）・Some(true) = 一度 ready を言った
    serves: Option<bool>,
}

static KEEP: Mutex<Vec<(PathBuf, Keep)>> = Mutex::new(Vec::new());
/// 起こした常駐の ready を待つ上限（混んだ機械で swift の binary が立つまで）
const READY_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
/// これより長く生きてから落ちた常駐は「続けて落ちた」と数えない（1 回目から数え直す）
const STEADY: std::time::Duration = std::time::Duration::from_secs(60);
/// 人が macOS の確認に答える書類の問い（doc-path の `ask`）を待つ上限
const ASK_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// その helper の生きている process（面が 1 つも無い・helper が止まった物は除く）。`procs` は `PROCS` の中身（呼ぶ側が lock を持つ）
fn live_proc(procs: &mut Vec<(PathBuf, Weak<HelperProc>)>, helper: &Path) -> Option<Arc<HelperProc>> {
    procs.retain(|(_, w)| w.strong_count() > 0);
    procs.iter().filter(|(p, _)| p == helper).filter_map(|(_, w)| w.upgrade()).find(|p| !p.routes.lock().unwrap().dead)
}

/// MIRROR-QUIET-1101（owner 208「garagebandのappにいくと何回ももう１つのshibubuが起動しようとしてくる」）: 常駐の helper（`serve`）が
/// 続けて落ちた時の 1 行（Shibubu が右上の知らせに出す）。落ちていない・1 回だけ落ちた = None。
/// 例: `if let Some(line) = mirror::helper_down(&helper) { toast::post_once(ctx, "mirror-helper-down", line, true, None) }`
/// 保証する: 読むだけ（helper を起こさない・lock は短い）。
/// Wave: MIRROR-QUIET-1101
pub fn helper_down(helper: &Path) -> Option<String> {
    let keep = KEEP.lock().unwrap();
    let (_, k) = keep.iter().find(|(h, _)| h == helper)?;
    let up = k.proc.as_ref().is_some_and(|p| !p.routes.lock().unwrap().dead);
    (k.fails >= 2 && !up).then(|| format!("Mirror's helper keeps stopping ({}). Shibubu keeps restarting it", k.why))
}

/// MIRROR-QUIET-1101: その helper の process を 1 本だけ返す（生きている面の process → 常駐の `serve`・無ければ起こして ready を待つ）。
/// Ok(None) = `serve` を知らない helper（ready を言わずに終わった・前の版・試験の偽物）= 呼ぶ側は今まで通り 1 回きりの process で。
/// Err = 常駐が落ちて起こし直しを待っている間（`backoff`）。代わりに 1 回きりの process を起こさない（窓探しのたびに process を起こさない）。
/// 保証する: 常駐は面が全部閉じても残る（Shibubu が終わると stdin が閉じて終わる）。UI の thread から呼ばない（起こす時は ready まで `READY_WAIT` 待つ）。
fn resident(helper: &Path) -> Result<Option<Arc<HelperProc>>, MirrorError> {
    let now = std::time::Instant::now();
    // 探して起こすまでを PROCS の lock の中で（2 つの窓探しが同時でも常駐は 1 本）。lock の順は PROCS → KEEP
    let p = {
        let mut procs = PROCS.lock().unwrap();
        if let Some(p) = live_proc(&mut procs, helper) {
            return Ok(Some(p));
        }
        let mut keep = KEEP.lock().unwrap();
        let i = match keep.iter().position(|(h, _)| h == helper) {
            Some(i) => i,
            None => {
                keep.push((helper.to_path_buf(), Keep::default()));
                keep.len() - 1
            }
        };
        let k = &mut keep[i].1;
        if k.serves == Some(false) {
            return Ok(None);
        }
        // 生きている process が無い = 前の常駐も落ちた（すぐ落ちたなら続けて数える）
        if let Some(old) = k.proc.take() {
            let why = old.child.lock().unwrap().try_wait().ok().flatten().map_or_else(|| "it stopped answering".to_string(), |s| s.to_string());
            k.fails = if k.born.is_some_and(|b| now.duration_since(b) < STEADY) { k.fails + 1 } else { 1 };
            k.retry_at = Some(now + backoff(k.fails));
            eprintln!("MIRROR-QUIET-1101: Mirror の helper の常駐が落ちました（{why}・続けて {} 回目）。{} 秒後に起こし直します", k.fails, backoff(k.fails).as_secs());
            k.why = why;
        }
        if let Some(at) = k.retry_at.filter(|at| now < *at) {
            return Err(MirrorError::Helper(format!("Mirror's helper stopped ({}). It starts again in {} s", k.why, (at - now).as_secs() + 1)));
        }
        let p = Arc::new(HelperProc::spawn(helper, &["serve"], None)?);
        procs.push((helper.to_path_buf(), Arc::downgrade(&p)));
        (k.proc, k.born, k.retry_at) = (Some(Arc::clone(&p)), Some(now), None);
        p
    };
    // ready か終わるまで lock の外で待つ（待つ間に他の窓探しが頼んだ行は helper の stdin に溜まる）
    let (caps, dead) = loop {
        let (caps, dead) = {
            let r = p.routes.lock().unwrap();
            (r.caps, r.dead)
        };
        if caps || dead || now.elapsed() >= READY_WAIT {
            break (caps, dead);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let mut keep = KEEP.lock().unwrap();
    let k = keep.iter_mut().find(|(h, _)| h == helper).map(|(_, k)| k);
    match (caps, dead, k) {
        (true, _, Some(k)) => k.serves = Some(true),
        // 一度も ready を言わずに終わった = `serve` を知らない helper（前の版・試験の偽物）: 以後この path は 1 回きりの process で
        (false, true, Some(k)) if k.serves.is_none() => {
            (k.serves, k.proc) = (Some(false), None);
            return Ok(None);
        }
        (false, true, _) => return Err(MirrorError::Helper("Mirror's helper stopped as it started".into())),
        // 混んでいて ready がまだ: 生きているので頼む（行は stdin に溜まり、答えは各々の期限まで待つ）
        _ => {}
    }
    Ok(Some(p))
}

/// MIRROR-QUIET-1101: 常駐が `fails` 回続けて落ちた後、起こし直すまでの間（1→2→4…最大 30 秒）
fn backoff(fails: u32) -> std::time::Duration {
    std::time::Duration::from_secs((1u64 << fails.saturating_sub(1).min(5)).min(30))
}

impl HelperProc {
    /// `helper <args>` を起こす（`stream --window <窓>` = `first` がその窓の stream 1・MIRROR-QUIET-1101 の `serve` = 窓 0 本で `first` は None）。
    /// 読み手の thread が stdout を持つ。次の stream と問いの id は 2 から
    fn spawn(helper: &Path, args: &[&str], first: Option<(u64, Sink)>) -> Result<HelperProc, MirrorError> {
        // MIRROR-SPEED-1101: 撮る枚数（Settings の Mirror smoothness）と IOSurface の受け口（作れなければ渡さない = pipe の RGBA）。
        // 知らない名前は helper の `arg` が読み飛ばす（前の版の helper・試験の偽物は今まで通り pipe で送る）
        let smooth = *SMOOTH.lock().unwrap();
        let inbox = inbox();
        let mut cmd = Command::new(helper);
        cmd.args(args).args(["--smooth", smooth]);
        if let Some((name, _)) = &inbox {
            cmd.args(["--surfaces", name.as_str()]);
        }
        let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
            Ok(c) => c,
            Err(e) => {
                if let Some((_, port)) = inbox {
                    close_inbox(port);
                }
                return Err(MirrorError::Helper(format!("{}: {e}", helper.display())));
            }
        };
        let stdin = child.stdin.take().ok_or_else(|| MirrorError::Helper("No stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| MirrorError::Helper("No stdout".into()))?;
        let routes = Arc::new(Mutex::new(Routes { streams: first.into_iter().collect(), ..Routes::default() }));
        let r2 = Arc::clone(&routes);
        std::thread::spawn(move || read_helper(stdout, r2));
        // MIRROR-SPEED-1101: IOSurface の受け口の読み手（helper の pid の 1 通だけ受ける）
        if let Some((_, port)) = inbox {
            let (r3, pid) = (Arc::clone(&routes), child.id());
            std::thread::spawn(move || read_surfaces(port, pid, r3));
        }
        Ok(HelperProc { child: Mutex::new(child), stdin: Mutex::new(stdin), next: AtomicU64::new(2), routes })
    }
    /// stdin に 1 行（`"cmd"` の行と入力の行）
    fn send(&self, line: &Value) -> Result<(), MirrorError> {
        let mut w = self.stdin.lock().unwrap();
        writeln!(w, "{line}").and_then(|_| w.flush()).map_err(|e| MirrorError::Helper(format!("Can't send to the stream: {e}")))
    }
    /// 同じ process の中に窓 `window` の stream を足す（`{"cmd":"open"}`）。返すのはその id。始まったか・止まったかは `sink` に来る
    fn open(&self, window: u64, sink: Sink) -> Result<u64, MirrorError> {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut r = self.routes.lock().unwrap();
            if r.dead {
                return Err(MirrorError::Helper("The helper stopped".into()));
            }
            r.streams.insert(id, sink);
        }
        self.send(&json!({"cmd": "open", "stream": id, "window": window})).inspect_err(|_| {
            self.routes.lock().unwrap().streams.remove(&id);
        })?;
        Ok(id)
    }
    /// stream `id` を外して止める（`{"cmd":"close"}`・答えは待たない）
    fn close(&self, id: u64) {
        self.routes.lock().unwrap().streams.remove(&id);
        let _ = self.send(&json!({"cmd": "close", "stream": id}));
    }
    /// MIRROR-QUIET-1101: 問いの 1 行（`q` に `"id"` を足す）を送り、同じ id の答え（`{"kind":"windows"|"answer","id":R}`）を `wait` まで待つ。
    /// 答えない・helper が止まった（読み手が待ち口を捨てる）= `Helper`
    fn ask(&self, mut q: Value, wait: std::time::Duration) -> Result<Value, MirrorError> {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        q["id"] = json!(id);
        let (tx, rx) = mpsc::channel();
        {
            let mut r = self.routes.lock().unwrap();
            if r.dead {
                return Err(MirrorError::Helper("The helper stopped".into()));
            }
            r.asks.insert(id, tx);
        }
        let gone = || {
            self.routes.lock().unwrap().asks.remove(&id);
        };
        self.send(&q).inspect_err(|_| gone())?;
        rx.recv_timeout(wait).map_err(|_| {
            gone();
            MirrorError::Helper(format!("No answer to {}", q["cmd"].as_str().unwrap_or("the question")))
        })
    }
    /// 窓の一覧を同じ process の中で（`{"cmd":"list"}`）。答えの `windows`（`helper list` の stdout と同じ形）。
    /// 許可が無い = `ScreenCaptureDenied`・取れない・答えない（`LIST_WAIT`）・helper が止まった = `Helper`
    fn list(&self, all: bool) -> Result<Value, MirrorError> {
        let ans = self.ask(json!({"cmd": "list", "all": all}), LIST_WAIT)?;
        match ans["error"].as_str() {
            Some("no_screen_capture_permission") => Err(MirrorError::ScreenCaptureDenied),
            Some(r) => Err(MirrorError::Helper(r.into())),
            None => Ok(ans["windows"].clone()),
        }
    }
}

impl Drop for HelperProc {
    /// 最後の面が落ちた: process を止める（stdin が閉じても止まるが、答えない helper も残さない）
    fn drop(&mut self) {
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- 人の口 ----

/// 同意済みの 1 つの窓の Mirror（stream と人の口）。作るには `ConsentGrant` が要る。
/// 例: `let mut m = MirrorSurface::open(&helper, target, grant, log)?; m.relay_human(&mut lease, input)?;`
/// 保証: 自動の入力を送る method を持たない。しない: 入力が元の app に届いた事・意味の変化（読み戻しは別の道）。
#[derive(Debug)]
pub struct MirrorSurface {
    target: MirrorTarget,
    log: RelayLog,
    next_seq: u64,
    /// G32: helper の process（同じ helper の面で分け合う）と、その中のこの面の stream の id
    proc: Arc<HelperProc>,
    stream: u64,
    /// 読み手の thread が置く最新の frame と、以後ずっと返す Err（窓が消えた・helper が止まった）
    video: Arc<Mutex<Video>>,
    events: Receiver<Value>,
    /// 追記 235 の 3: 答えをまだ読んでいない送った記録（seq → 記録・`ANSWER_CAP` まで）
    unanswered: HashMap<u64, RelayRecord>,
}

#[derive(Debug, Default)]
struct Video {
    latest: Option<MirrorFrame>,
    fatal: Option<MirrorError>,
    /// G97: 着いた物（frame・以後の Err・出来事）で UI を起こす口（`MirrorSurface::wake_with`）。無い間は UI が見に来るのを待つ
    wake: Option<Waker>,
}

/// helper の出来事を溜めておく上限（読む側は毎 frame か毎 poll で空にする）。
const EVENT_CAP: usize = 64;

impl Sink {
    /// 以後ずっと返す Err（最初の 1 つを保つ）。前の frame を今の物として返さない
    fn fatal(&self, e: MirrorError) {
        {
            let mut v = self.video.lock().unwrap();
            v.fatal.get_or_insert(e);
            v.latest = None;
        }
        self.wake();
    }
    /// G97: UI を起こす（lock を離してから・起こされた UI がすぐ `stream` で lock を取る）
    fn wake(&self) {
        let w = self.video.lock().unwrap().wake.clone();
        if let Some(w) = w {
            w.wake();
        }
    }
    /// tag 1 → `latest` を置き換え（古い物は捨てる）。形の違う frame は捨てる（黒や崩れた texture を作らない）。次の frame を待つ
    fn frame(&self, payload: Vec<u8>) {
        match parse_frame(payload) {
            Ok(f) => self.put(f),
            Err(_) => self.wake(),
        }
    }
    /// MIRROR-SPEED-1101: 1 枚を `latest` に（pipe の tag 1 も mach の IOSurface も同じ所へ・受け取った時刻を置く）。
    /// 読まれなかった古い 1 枚は lock の外で放す（IOSurface なら use count が下がる = SCK の pool に返る）
    fn put(&self, mut f: MirrorFrame) {
        f.received_ms = now_ms();
        let old = {
            let mut v = self.video.lock().unwrap();
            if v.fatal.is_none() {
                v.latest.replace(f)
            } else {
                None
            }
        };
        drop(old);
        self.wake();
    }
    /// tag 2 → `events` へ（error の window_gone / pid_changed と stopped は `fatal` にも）。
    /// `events` は `EVENT_CAP` 件で満ちる口: 誰も読まない間（Page が見えていない）に着いた出来事は捨てる（2 秒ごとの stats で溜めない）。
    fn event(&self, ev: Value) {
        let reason = ev["reason"].as_str().unwrap_or("");
        match ev["kind"].as_str() {
            Some("error") if helper_error(reason) == MirrorError::WindowGone => self.fatal(MirrorError::WindowGone),
            // G90 (3): 窓は在るが画面に出ていない（隠れた・最小化）= 閉じたと言わない・名前はこの面の app
            Some("stopped") if reason == "window_hidden" => self.fatal(MirrorError::WindowHidden { app_name: self.app_name.clone() }),
            Some("stopped") => self.fatal(helper_error(reason)),
            _ => {}
        }
        let _ = self.events.try_send(ev);
        self.wake();
    }
}

/// helper の stdout の 1 通（`[tag u8][len u32 LE][payload]`）。EOF・途中で切れたら None。
/// R-gui: PoC（`src/bin/kangae-mirror.rs`）の measure の stream の読みも同じこれを呼ぶ（写しを消した・private → pub・本体は同じ）。
pub fn read_message(r: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head).ok()?;
    let mut payload = vec![0u8; u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize];
    r.read_exact(&mut payload).ok()?;
    Some((head[0], payload))
}

/// frame の header の `"stream"`（無い・読めない = 1 ＝ 起こした時の窓・前の版の helper と試験の偽の helper）
fn frame_stream(payload: &[u8]) -> u64 {
    let hl = payload.get(0..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).unwrap_or(0);
    payload.get(4..4 + hl).and_then(|b| serde_json::from_slice::<Value>(b).ok()).and_then(|h| h["stream"].as_u64()).unwrap_or(1)
}

/// helper の stdout（1 本・N 本の stream が混ざる）を読み続け、`"stream"` の id の `Sink` へ分ける（G32）。tag 1 = frame・tag 2 = 出来事
/// （`{"kind":"windows"|"answer","id":R}` は `asks` の R へ・`ready` は `caps`・stopped はその stream を表から外す）。EOF・壊れた tag で `dead` にし、残っている全部の
/// stream に `Helper` の Err を書く（読み手が死んだ時に誰も書かない stream を残さない）・待っている list の口は捨てる（待つ側は Err）。
fn read_helper(mut stdout: impl Read, routes: Arc<Mutex<Routes>>) {
    let mut why = MirrorError::Helper("The helper stopped".into());
    while let Some((tag, payload)) = read_message(&mut stdout) {
        match tag {
            1 => {
                let sink = routes.lock().unwrap().streams.get(&frame_stream(&payload)).cloned();
                if let Some(s) = sink {
                    s.frame(payload);
                }
            }
            2 => {
                let Ok(ev) = serde_json::from_slice::<Value>(&payload) else { continue };
                let mut r = routes.lock().unwrap();
                // MIRROR-QUIET-1101: stdin の status / doc-path に答える版（stream も serve も最初に 1 回言う）
                if ev["kind"] == "ready" {
                    r.caps = true;
                    // MIRROR-FRAME-1101: 小さな絵に答える版
                    r.shot = ev["caps"].as_array().is_some_and(|c| c.iter().any(|c| c == "shot"));
                    continue;
                }
                if ev["kind"] == "windows" || ev["kind"] == "answer" {
                    if let Some(tx) = ev["id"].as_u64().and_then(|id| r.asks.remove(&id)) {
                        let _ = tx.send(ev);
                    }
                    continue;
                }
                let id = ev["stream"].as_u64().unwrap_or(1);
                let sink = if ev["kind"] == "stopped" { r.streams.remove(&id) } else { r.streams.get(&id).cloned() };
                drop(r);
                if let Some(s) = sink {
                    s.event(ev);
                }
            }
            t => {
                why = MirrorError::Helper(format!("The helper's answer has the wrong format (tag {t})"));
                break;
            }
        }
    }
    let mut r = routes.lock().unwrap();
    r.dead = true;
    r.asks.clear();
    for (_, s) in r.streams.drain() {
        s.fatal(why.clone());
    }
}

impl MirrorSurface {
    /// helper の `stream --window <id>` を起こす。grant の bundle id が target と違えば `NoConsent`・画面収録の許可が無ければ `ScreenCaptureDenied`（何も起こさない）。
    /// G32: 同じ helper の面が既に開いていれば process を起こさず、その process の stdin に `{"cmd":"open","stream":N,"window":<id>}` を送る
    /// （1 面目を落とさない）。この面が落ちたら `{"cmd":"close","stream":N}`、最後の面が落ちたら process を止める。
    /// 例: `let a = MirrorSurface::open(&helper, docs, g1, log1)?; let b = MirrorSurface::open(&helper, notes, g2, log2)?;`（process は 1 本・a も b も映る）
    pub fn open(
        helper: &Path,
        target: MirrorTarget,
        grant: ConsentGrant,
        log: RelayLog,
    ) -> Result<MirrorSurface, MirrorError> {
        if grant.bundle_id != target.bundle_id {
            return Err(MirrorError::NoConsent {
                app_name: target.app_name.clone(),
            });
        }
        // MIRROR-QUIET-1101: 許可は答える process（常駐）が在ればその stdin で（process を起こさない）・無ければ 1 回きりの `status`
        let status = match capable(helper) {
            Some(p) => p.ask(json!({"cmd": "status"}), LIST_WAIT)?,
            None => helper_json(helper, &["status"])?,
        };
        if status["screen_capture"] != json!(true) {
            return Err(MirrorError::ScreenCaptureDenied);
        }
        let video = Arc::new(Mutex::new(Video::default()));
        let (tx, events) = mpsc::sync_channel(EVENT_CAP);
        let sink = Sink { video: Arc::clone(&video), events: tx, app_name: target.app_name.clone() };
        // 探して起こすまでを PROCS の lock の中で（2 つの面が同時に開いても process は 1 本）
        let mut procs = PROCS.lock().unwrap();
        let (proc, stream) = match live_proc(&mut procs, helper) {
            Some(p) => {
                let id = p.open(target.window_id, sink)?;
                (p, id)
            }
            None => {
                let w = target.window_id.to_string();
                let p = Arc::new(HelperProc::spawn(helper, &["stream", "--window", &w], Some((1, sink)))?);
                procs.push((helper.to_path_buf(), Arc::downgrade(&p)));
                (p, 1)
            }
        };
        drop(procs);
        Ok(MirrorSurface { target, log, next_seq: 1, proc, stream, video, events, unanswered: HashMap::new() })
    }
    /// 人の入力を 1 つ送る。lease を人に戻し（`ControlLease::human_input`）、`RelayRecord` を書いてから helper の stdin へ
    /// `origin: human`・`relay_seq` 付きで送る。返すのは書いた記録。記録が書けなければ送らない。
    /// helper の答え（sent / error・`id` = `relay_seq`）は `take_event` で読み、拒否は `refusal` で 1 行の Err にする。
    /// W16M-2: 先に `route_for` で道を決め、`NeedsFront` なら lease も記録も触らず送らず `Err(NeedsFront)`（E1 の「relay → 変化」を崩さない）。
    /// それ以外は記録の `route` に残し、helper の行に `"route"` を付けて送る（helper はその道だけで送る・`via` の mouse は人の口では使わない）。
    /// 例: `match m.relay_human(&mut lease, MirrorInput::Key { key: "s".into(), mods: vec![KeyMod::Cmd] }) { Ok(r) => assert_eq!(r.route, InputRoute::SaveDocument), Err(e) => line = e.to_string() }`
    /// 保証しない: 届いた事・値が変わった事（試験の側の E1）。
    pub fn relay_human(
        &mut self,
        lease: &mut ControlLease,
        input: MirrorInput,
    ) -> Result<RelayRecord, MirrorError> {
        let route = route_for(&input);
        if route == InputRoute::NeedsFront {
            return Err(MirrorError::NeedsFront { app_name: self.target.app_name.clone(), kind: input.kind() });
        }
        self.relay_on(lease, input, route)
    }
    /// `relay_human` と `relay_front` の本体: lease を人に戻し、記録（`route`）を書いてから helper の stdin へ（記録が書けなければ送らない）
    fn relay_on(&mut self, lease: &mut ControlLease, input: MirrorInput, route: InputRoute) -> Result<RelayRecord, MirrorError> {
        let at_ms = now_ms();
        lease.human_input(at_ms);
        let record = RelayRecord {
            v: RELAY_SCHEMA_V,
            seq: self.next_seq,
            at_ms,
            kind: input.kind(),
            route,
            pid: self.target.pid,
            window_id: self.target.window_id,
            pos: input.pos(),
            outcome: None,
            ms: None,
        };
        self.log.append(&record)?;
        self.next_seq += 1;
        self.unanswered.insert(record.seq, record.clone());
        if self.unanswered.len() > ANSWER_CAP {
            if let Some(old) = self.unanswered.keys().min().copied() {
                self.unanswered.remove(&old);
            }
        }
        let mut line =
            serde_json::to_value(&input).map_err(|e| MirrorError::Helper(e.to_string()))?;
        line["origin"] = json!(InputOrigin::Human);
        line["relay_seq"] = json!(record.seq);
        line["id"] = json!(record.seq);
        line["route"] = json!(route);
        line["stream"] = json!(self.stream);
        self.proc.send(&line)?;
        Ok(record)
    }
    /// MIRROR-HYBRID-1101: 触った最初の 1 押し（click・drag・scroll）を、前に出した本物へ HID で送り直す（`InputRoute::Front`・記録と helper の行は `relay_human` と同じ形）。
    /// 文字と key は送らず Err(NeedsFront)（本物が前に出れば人の key は本物へ直に行く・送り直すと 2 回届く）。
    /// 例: `pages.go_direct(id); m.relay_front(&mut lease, MirrorInput::Click { x, y, count: 1 })`（呼ぶ側 = `MirrorPages::show` が離した押しで）
    /// 保証しない: 届いた事（helper の答えが `not_front` = 本物が前に来なかった・`refusal` が NeedsFront にする）。
    /// Wave: MIRROR-HYBRID-1101
    pub fn relay_front(&mut self, lease: &mut ControlLease, input: MirrorInput) -> Result<RelayRecord, MirrorError> {
        if matches!(input, MirrorInput::Text { .. } | MirrorInput::Key { .. }) {
            return Err(MirrorError::NeedsFront { app_name: self.target.app_name.clone(), kind: input.kind() });
        }
        self.relay_on(lease, input, InputRoute::Front)
    }
    pub fn target(&self) -> &MirrorTarget {
        &self.target
    }
    /// MIRROR-SPEED-1101: Shibubu がこの面を描く大きさ（pixel = 映像の矩形の point × 画面の倍率）を helper に渡す（`{"cmd":"view","stream":N,"w":W,"h":H}`）。
    /// helper は窓の pixel とこれの小さい方で撮る（縦横の比は窓のまま・本物より細かくは撮らない）。
    /// 例: `m.view(1600, 1000)`（呼ぶ側 = `MirrorPages::show`・大きさが替わった時だけ・四隅を引く間は `VIEW_GAP_MS` ごとまで）。
    /// 保証しない: 撮る大きさが替わった時刻（答えは無い・次の frame の w / h で分かる）・送れた事（helper が止まっていれば次の `stream` が Err）。
    /// Wave: MIRROR-SPEED-1101
    pub fn view(&self, w: u32, h: u32) {
        let _ = self.proc.send(&json!({"cmd": "view", "stream": self.stream, "w": w, "h": h}));
    }
    /// 映像（W16M-1・W2c の契約の `stream`）。前に呼んだ後に着いた frame のうち一番新しい 1 枚を返し、間の古い frame は捨てる（来ていなければ None）。
    /// 待たない（egui の 1 frame の中で毎回呼ぶ）。helper の stdout は open の時に起こした読み手の thread が持つ。
    /// 例: `match m.stream() { Ok(Some(f)) => view.upload(f), Ok(None) => {}, Err(e) => shown = e.to_string() }`
    /// 保証する: helper が `window_gone`・`pid_changed` を出したら `WindowGone`・`window_hidden`（G90 (3)）なら `WindowHidden`・helper が止まったら `Helper`・`stopped` の理由が
    /// 画面収録の許可なら `ScreenCaptureDenied`（どれも以後ずっと同じ Err。前の frame を今の物として返さない）。
    /// 保証しない: frame が今の元の窓と同じ中身である事（それは試験の側の E0 の OCR・`tests/common/readback.rs` の `e0_current`）。
    pub fn stream(&mut self) -> Result<Option<MirrorFrame>, MirrorError> {
        let mut v = self.video.lock().unwrap();
        match &v.fatal {
            Some(e) => Err(e.clone()),
            None => Ok(v.latest.take()),
        }
    }
    /// G97: 着いた frame・以後の Err・出来事で、`make` が作る Waker を起こす（UI の描き直し）。作るのは最初の 1 回だけ（毎 frame 呼んでよい）。
    /// 例: `m.wake_with(|| Waker::from(Arc::new(Repaint(ctx.clone()))))`。保証しない: 起こす回数（続けて着けば着いた数だけ・まとめるのは Waker の側）。
    pub fn wake_with(&self, make: impl FnOnce() -> Waker) {
        self.video.lock().unwrap().wake.get_or_insert_with(make);
    }
    /// 試験: 描き直しを起こす口が在るか（`wake_with` を呼んだか）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn wakes(&self) -> bool {
        self.video.lock().unwrap().wake.is_some()
    }
    /// helper の出来事（tag 2 の JSON: `sent`・`stats`・`error`・`stopped`）を 1 つ。無ければ None（待たない）。
    /// 例: `while let Some(ev) = m.take_event() { if ev["kind"] == "sent" { .. } }`。保証: 着いた順。しない: frame との前後（frame は `stream`）・読まずに `EVENT_CAP` 件を超えた分（捨てる）。
    pub fn take_event(&mut self) -> Option<Value> {
        self.events.try_recv().ok()
    }
    /// MIRROR-HYBRID-1101 追記 235 の 3（owner「反応しない時もあるし」）: helper の出来事 1 つが送った入力の答え（`sent` / `error`・`id` = `relay_seq`）なら、
    /// その入力の結果（`outcome`）と押してからの ms（`ms`）を relay の記録にもう 1 行書いて返す。それ以外・知らない id・書けない時は None。
    /// 例: `while let Some(ev) = m.take_event() { m.note_answer(&ev); if let Some(e) = m.refusal(&ev) { … } }`（呼ぶ側 = `MirrorPages::show`）
    /// 保証しない: 本物の値が変わった事（E1・送っただけ）・Shibubu を通らない押し（本物が前に居る間の押しは本物へ直に行く = 記録に出ない）
    /// Wave: MIRROR-HYBRID-1101 追記 235
    pub fn note_answer(&mut self, ev: &Value) -> Option<RelayRecord> {
        let id = ev["id"].as_u64().filter(|_| matches!(ev["kind"].as_str(), Some("sent" | "error")))?;
        let rec = self.unanswered.remove(&id)?;
        let done = answered(&rec, ev, now_ms())?;
        self.log.append(&done).ok()?;
        Some(done)
    }
    /// W16M-2: helper の出来事 1 つが relay の答えの拒否なら、画面の 1 行の Err（D-e）。それ以外（sent・stats・stopped・`id` の無い行）は None。
    /// reason → Err: `needs_front` → `NeedsFront{kind = op}`・`automation_denied` → `AutomationDenied`・`not_trusted` → `AccessibilityDenied`・
    /// `not_front`（MIRROR-HYBRID-1101 の front の道で本物が前に来なかった・送っていない）→ `NeedsFront`・
    /// `ax_press_failed_*` / `ax_select_failed_*`（AX の点の道で元の app が答えなかった・helper は動いている）→ `NoAnswer`・
    /// `window_gone` / `pid_changed` → None（`stream` が `WindowGone` を返す）・他 → `Helper(reason)`（口の違いは `WrongPort`）。
    /// 例: `while let Some(ev) = m.take_event() { if let Some(e) = m.refusal(&ev) { pane_line = Some(e.to_string()) } }`
    /// 保証する: 拒否の答えを成功に読まない・`app_name` は今の target の物。保証しない: 送れた入力が値を変えた事（E1）。
    pub fn refusal(&self, ev: &Value) -> Option<MirrorError> {
        if ev["kind"] != "error" || ev.get("id").is_none() {
            return None;
        }
        let app_name = self.target.app_name.clone();
        let reason = ev["reason"].as_str().unwrap_or("no_reason");
        Some(match reason {
            "window_gone" | "pid_changed" => return None,
            // MIRROR-HYBRID-1101: not_front = 前に出した本物が 500ms で点の一番上に来なかった（送っていない）
            "needs_front" | "not_front" => match serde_json::from_value::<InputKind>(ev["op"].clone()) {
                Ok(kind) => MirrorError::NeedsFront { app_name, kind },
                Err(_) => MirrorError::Helper(reason.into()),
            },
            "automation_denied" => MirrorError::AutomationDenied { app_name },
            "not_trusted" => MirrorError::AccessibilityDenied,
            r if r.starts_with("ax_press_failed_") || r.starts_with("ax_select_failed_") => MirrorError::NoAnswer { app_name },
            r => helper_error(r),
        })
    }
}

impl Drop for MirrorSurface {
    /// この面の stream だけを止める（他の面の stream は止めない）。最後の面なら `HelperProc` の Drop が process を止める
    fn drop(&mut self) {
        self.proc.close(self.stream);
    }
}

// ---- 自動の口 ----

/// measure と試験の入力の口（helper の `input` subcommand・stream を通らない・relay の記録に書かない）。
/// 例: `AutomatedInput::new(helper).send(&target, &input)?`。保証: `origin: automated` で送り、helper は `stream` の口に入れない。
/// しない: 同意と lease の確認（製品の AI はこの口を使わない・W2c の口 = `MacGui::act`）。
#[derive(Debug, Clone)]
pub struct AutomatedInput {
    helper: PathBuf,
}

impl AutomatedInput {
    pub fn new(helper: PathBuf) -> AutomatedInput {
        AutomatedInput { helper }
    }
    /// 送った時刻（helper の `sent_ms`・unix ms）を返す。
    pub fn send(&self, target: &MirrorTarget, input: &MirrorInput) -> Result<f64, MirrorError> {
        let mut line =
            serde_json::to_value(input).map_err(|e| MirrorError::Helper(e.to_string()))?;
        line["origin"] = json!(InputOrigin::Automated);
        let (w, p, l) = (
            target.window_id.to_string(),
            target.pid.to_string(),
            line.to_string(),
        );
        let r = helper_json(&self.helper, &["input", "--window", &w, "--pid", &p, &l])?;
        match (r["kind"].as_str(), r["sent_ms"].as_f64()) {
            (Some("sent"), Some(ms)) => Ok(ms),
            _ => Err(helper_error(r["reason"].as_str().unwrap_or("no_reason"))),
        }
    }
}

// ---- エラー ----

/// 画面に出す時は `to_string()` の 1 行（D-e: 偽の成功を出さず次の一手を 1 行）。
#[derive(Debug, Clone, PartialEq)]
pub enum MirrorError {
    /// その app に同意が無い（同意の button を出す）
    NoConsent { app_name: String },
    /// 画面収録の許可が無い（W16M-3。何も送らない）
    ScreenCaptureDenied,
    /// 窓が消えた・pid が変わった（別の窓に付け替えない）
    WindowGone,
    /// G90 (3): 元の窓は在るが画面に出ていない（app が隠れた ⌘H・窓を最小化した）。SCK は隠れた窓の frame を出さない（10-28 実測: Music の隠れた窓も
    /// 最小化した窓も status suspended だけで complete 0）ので映さず、面は `Show {App}` の button（`open -g -b` = 前に出さずに出す・Music は最小化も戻る）
    WindowHidden { app_name: String },
    /// 映す窓が見つからない（W16M-1・`MirrorTarget::find`。app を起こさず file も開かない）
    NoWindow { app_name: String, title_has: Option<String> },
    /// M4 ③: 題名の合う窓が `count` 個あり、どれが書類の窓か確かめられない（書類を引けない）。当て推量で選ばない（別の書類・owner の書類に打たない）。
    /// `ask_pid` = 許可がまだ決まっていない時の、尋ねる先の pid（button で尋ねる）・`denied` = 許可を断られている（システム設定で出す）
    Ambiguous { app_name: String, title_has: Option<String>, count: usize, ask_pid: Option<i64>, denied: bool },
    /// helper が口の違いで行を拒んだ（`not_human_relay`・`human_origin_on_automated_port`）
    WrongPort { reason: String },
    /// helper の実行 file がこの版に入っていない（画面には path を出さない・path は Debug の側だけ）
    HelperMissing(PathBuf),
    /// helper を起こせない・答えない
    Helper(String),
    /// relay の記録を書けない・読めない
    Log(String),
    /// W16M-2: 公開 API で背景の app に届かない入力（drag・scroll・2 回以上の click・⌘Z などの menu の key・押せず文字の欄でもない所の click・
    /// 保存の宛先の書類が 1 つに決まらない）。送らない（偽の成功にしない）
    NeedsFront { app_name: String, kind: InputKind },
    /// W16M-2: ⌘S を Apple Event で送る許可（オートメーション）が無い。映像と他の道はそのまま
    AutomationDenied { app_name: String },
    /// W16M-2: アクセシビリティの許可が無く、文字・key・click を送れない（送る窓を AX で確かめられない）。映像と ⌘S の道はそのまま
    AccessibilityDenied,
    /// W16M-2（反証の直し）: AX の点の道（click）で元の app が押す・選択に答えなかった（混んだ機械の -25204 など）。helper は動いている。
    /// 効いたかは分からない（答えの前に押せている事がある）ので「送らなかった」とも「届いた」とも言わない
    NoAnswer { app_name: String },
    /// G87: Shibubu が app を起こした（`Launcher`）後、窓が出るのを待っている（`LAUNCH_WAIT_MS` まで。過ぎたら `NoWindow`）
    Starting { app_name: String },
    /// G87: その bundle id の app がこの Mac に無い（`open -b` が app を引けない）。何も入れない
    NotInstalled { app_name: String },
}

/// G87: app を Mac の上で起こす口。`(bundle_id, if_stopped)` → `Ok((画面の名前, 起こした))`。if_stopped = 動いていれば起こさない
/// （Page を開いた最初の探し）・false = 動いていても `open` する（`Open {App}` の button = 窓の無い app に窓を出させる・隠れた窓の隠しを解く）。
/// 入っていなければ `Err(NotInstalled)`。前面を取らない（背景で起こす）。製品は `launch_app`・試験は偽の fn（実の app を起こさない）。
/// MIRROR-HYBRID-1101 追記 245 の 3: 動いていない app は隠して起こす（窓ができる間も画面に出さない・隠しは `Pane::lost` が Shibubu の後ろで 1 回解く）。
/// 例: `let (name, started) = (pages_launcher)("com.cockos.reaper", true)?;`
/// Wave: M2
pub type Launcher = fn(&str, bool) -> Result<(String, bool), MirrorError>;

/// G87: `Launcher` の製品の形: 動いているかと名前は `lsappinfo`（LaunchServices の今の app）・起こすのは `open -g -b <bundle_id>`（背景で・前面を取らない）。
/// 追記 245 の 3（本体の commit）: 動いていなければ `open -g -j -b`（隠して起こす = NSWorkspace の activates false・hides true）。動いていれば `-j` を付けない
/// （隠れた窓を出す・窓の無い app に窓を出させる道）。
/// 名前 = LaunchServices の表示名（`REAPER`・`Music`）。入っていない app は名前を引けないので bundle id の最後の語の頭を大文字に。
/// 保証しない: 起こされた app が前面を取らない事（`open -g` でも新しい instance が前に出る app が在る・W17-1 の TextEdit）・起こした後に窓を出す事。
/// Wave: M2
pub fn launch_app(bundle_id: &str, if_stopped: bool) -> Result<(String, bool), MirrorError> {
    // 動いている app の表示名（`"LSDisplayName"="REAPER"`）。動いていなければ None
    let running = || {
        let out = Command::new("/usr/bin/lsappinfo").args(["info", "-only", "name", "-app", bundle_id]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let name = text.split_once('=')?.1.trim().trim_matches('"').to_string();
        (!name.is_empty()).then_some(name)
    };
    let was = running();
    if let Some(name) = was.clone().filter(|_| if_stopped) {
        return Ok((name, false));
    }
    // MIRROR-HYBRID-1101 追記 245 の 3（owner「gaagebandがshibubuの上に毎回出るのは避けたい…shibubuの後ろで起動されるように」）: 動いていない app は隠して起こす
    // （窓ができる間も画面に出さない・隠しは窓ができた後に `Pane::lost` が launcher で 1 回解く）。動いている app は隠さない（隠れた窓を出す・窓を出させる道）
    let flags: &[&str] = if was.is_some() { &["-g", "-b"] } else { &["-g", "-j", "-b"] };
    let out = Command::new("/usr/bin/open").args(flags).arg(bundle_id).stdin(Stdio::null()).output().map_err(|e| MirrorError::Helper(format!("open: {e}")))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).into_owned();
        // ponytail: 入っていない事は open の stderr の語（LSCopyApplicationURLsForBundleIdentifier … bundle identifier）で見分ける。macOS が文を替えたら Helper の 1 行になる
        if why.contains("bundle identifier") {
            let last = bundle_id.rsplit('.').next().unwrap_or(bundle_id);
            let mut c = last.chars();
            let app_name = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
            return Err(MirrorError::NotInstalled { app_name });
        }
        return Err(MirrorError::Helper(format!("open {} {bundle_id}: {}", flags.join(" "), why.trim())));
    }
    let name = running().unwrap_or_else(|| bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_string());
    Ok((name, true))
}

/// G90（DOGFOOD-X 周 2 回 1）: 動いている app（bundle id）を前へ。LaunchServices の pid（`lsappinfo info -only pid -app` → `"pid"=495`）→ `surface::activate_pid`
/// （Apple Events を使わない = 自動化の許可の問いが出ない）。例: `activate_app("com.stablyai.orca")`。
/// 保証しない: 前に出る事（macOS 14 の協調の activate・`activate_pid`）・同じ bundle id の process が 2 つの時にどちらか（LaunchServices が返す 1 つ）。
pub fn activate_app(bundle_id: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/lsappinfo").args(["info", "-only", "pid", "-app", bundle_id]).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("lsappinfo: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = text.split_once('=').and_then(|(_, p)| p.trim().parse::<u32>().ok()).ok_or_else(|| format!("{bundle_id} isn't running"))?;
    // `super`（この file は src/audio/surface.rs からも `#[path]` で取り込まれ、そこでは crate::surface が無い）
    super::activate_pid(pid)
}

/// G90: 今 前面の app の bundle id（LaunchServices の `lsappinfo front` → `info -only bundleid`）。読めなければ None。
/// `lsappinfo front` は `ASN:0x0-cb0cb:` と下の語の `0x` を落として返し、そのままでは `info` が何も返さない（10-27 実測）ので `0x` を補う。
/// 例: `front_app().as_deref() == Some("com.cockos.reaper")`。
/// Wave: M2
pub fn front_app() -> Option<String> {
    let run = |args: &[&str]| Command::new("/usr/bin/lsappinfo").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let asn = run(&["front"])?;
    let (hi, lo) = asn.trim_end_matches(':').strip_prefix("ASN:")?.split_once('-')?;
    let lo = if lo.starts_with("0x") { lo.to_string() } else { format!("0x{lo}") };
    let info = run(&["info", "-only", "bundleid", &format!("ASN:{hi}-{lo}")])?;
    let id = info.split_once('=')?.1.trim().trim_matches('"').to_string();
    (!id.is_empty()).then_some(id)
}

/// G91（改・owner 19:06「dragできないとかクリックすらできなかった」）: 画面の pointer の位置（global の point・左上が原点 = AX と bridge の set_frame と同じ座標）と、
/// mouse の button が 1 つでも押されているか。本物の窓が前に居る間は Shibubu に pointer の event が来ないので、`MirrorPages` が direct の間だけ ≈ 10 Hz で読む。
/// CoreGraphics の `CGEventCreate(NULL)` + `CGEventGetLocation` と `CGEventSourceButtonState`（公開 API・許可は要らない）。読めなければ位置は None。
/// 例: `let (at, down) = pointer_now(); let outside = at.is_some_and(|[x, y]| !slot.contains(x, y));`
#[cfg(target_os = "macos")]
pub fn pointer_now() -> (Option<[f64; 2]>, bool) {
    use std::ffi::c_void;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    // 理由: crate を足さずに pointer の今を読む。前提: source = NULL の CGEventCreate は今の pointer の位置を持つ event を返す（返した物は CFRelease）・
    // state 0 = kCGEventSourceStateCombinedSessionState・button 0〜2 = 左・右・中。どれも読むだけで他の process に何も送らない
    unsafe {
        let e = CGEventCreate(std::ptr::null());
        let at = (!e.is_null()).then(|| {
            let p = CGEventGetLocation(e);
            CFRelease(e as *const c_void);
            [p.x, p.y]
        });
        (at, (0..3).any(|b| CGEventSourceButtonState(0, b)))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn pointer_now() -> (Option<[f64; 2]>, bool) {
    (None, false)
}

/// MIRROR-HYBRID-1101: 人が最後に keyboard か mouse を触ってからの秒（どの app でも・`CGEventSourceSecondsSinceLastEventType` の HID の state・許可は要らない）。
/// 本物の窓が前に居る間は Shibubu に event が来ないので、`MirrorPages` が direct の間だけ読み、`IDLE_S` を超えたら本物を Shibubu の下へ戻す。読めなければ 0（戻さない）
/// 例: `if (self.idle)() >= IDLE_S { self.put_back(id) }`
/// Wave: MIRROR-HYBRID-1101
#[cfg(target_os = "macos")]
pub fn idle_secs() -> f64 {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    // 理由: crate を足さずに人の手の静けさを読む。前提: state 1 = kCGEventSourceStateHIDSystemState（人の keyboard と mouse）・type !0 = kCGAnyInputEventType。読むだけ
    let s = unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) };
    if s.is_finite() && s >= 0.0 {
        s
    } else {
        0.0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn idle_secs() -> f64 {
    0.0
}

/// M4 ③: `Ambiguous` の面の button の文字（押すと macOS が「Shibubu に <app> の操作を許可しますか」と尋ねる・`MirrorPages::show`）
pub fn ask_button(app_name: &str) -> String {
    format!("Check {app_name}'s document")
}

impl std::fmt::Display for MirrorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MirrorError::NoConsent { app_name } => write!(f, "{}", consent_prompt(app_name)),
            MirrorError::ScreenCaptureDenied => write!(f, "Screen recording is off · Open settings"),
            MirrorError::WindowGone => write!(f, "The app's window closed"),
            MirrorError::WindowHidden { app_name } => write!(f, "{app_name}'s window is hidden"),
            MirrorError::NoWindow { app_name, title_has: Some(t) } => write!(f, "No {app_name} window with {t} open was found. Open {t} in {app_name} and it shows here"),
            // G87: 次の一手は文でなく面の button（`Open {App}`・`MirrorPages::show`）
            MirrorError::NoWindow { app_name, title_has: None } => write!(f, "{app_name} has no open window"),
            MirrorError::Ambiguous { app_name, title_has, count, ask_pid, denied } => {
                let what = title_has.as_deref().map(|t| format!("with {t} open, ")).unwrap_or_default();
                write!(f, "There are {count} {app_name} windows {what}and Shibubu can't tell which one is this project's document. ")?;
                match (denied, ask_pid) {
                    (true, _) => write!(f, "In System Settings → Privacy & Security → Automation, allow Shibubu to control {app_name} and it can tell the windows apart"),
                    (false, Some(_)) => write!(f, "Press “{}” to allow it and it can tell the windows apart", ask_button(app_name)),
                    (false, None) => write!(f, "Close the window you don't need and this one shows here"),
                }
            }
            MirrorError::WrongPort { reason } => write!(f, "Nothing was sent · {reason}"),
            MirrorError::HelperMissing(_) => write!(f, "This version can't mirror apps"),
            // W16M-3（反証）: reason（stream_error・capture_timeout・list の exit status 等）と path は内部の語なので出さない（Debug と stderr の側だけ）。
            // 面の Err は 2 秒ごとに探し直す（`MirrorPages::show`）
            MirrorError::Helper(_) => write!(f, "Couldn't mirror the original window. Shibubu keeps retrying (if this keeps happening, restart Shibubu)"),
            MirrorError::Log(e) => write!(f, "Nothing was sent, because the input log couldn't be written · {e}"),
            MirrorError::NeedsFront { app_name, kind } => {
                let what = match kind {
                    InputKind::Scroll => "Scrolling",
                    InputKind::Drag => "Dragging",
                    InputKind::Click | InputKind::Text | InputKind::Key => "This action",
                };
                write!(f, "{what} doesn't reach {app_name} from here. Nothing was sent.")
            }
            MirrorError::AutomationDenied { app_name } => write!(f, "Permission to have {app_name} save is needed. In System Settings → Privacy & Security → Automation, allow Shibubu to control {app_name} (until then ⌘S isn't sent)"),
            MirrorError::NoAnswer { app_name } => write!(f, "{app_name} didn't answer the click. Check it in {app_name}."),
            MirrorError::AccessibilityDenied => write!(f, "Accessibility access is needed. In System Settings → Privacy & Security → Accessibility, allow Shibubu (until then no text, keys or clicks are sent)"),
            MirrorError::Starting { app_name } => write!(f, "Starting {app_name}…"),
            MirrorError::NotInstalled { app_name } => write!(f, "{app_name} isn't installed on this Mac"),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// helper の stdout の message 1 つ `[tag u8][len u32 LE][payload]`（R-M: w16m1 / w16m1b の `msg` の closure 2 つと `event_msg` を寄せた）。
    /// 例: `let mut bytes = msg(1, frame_payload(header(1, 1), 4)); bytes.extend(msg(2, serde_json::to_vec(&json!({"kind": "stopped", "reason": "x"})).unwrap()));`
    /// 保証する: `read_helper` がそのまま読める並び（tag → 長さ LE 4 byte → payload）。保証しない: payload の中身（frame は `frame_payload`・出来事は JSON を呼ぶ側が作る）。
    pub(crate) fn msg(tag: u8, payload: Vec<u8>) -> Vec<u8> {
        let mut m = vec![tag];
        m.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        m.extend(payload);
        m
    }

    /// status が許可ありで、`stream)` の中身 `stream_sh` を走らせる偽の helper（sh）と、その dir
    /// （R-M: `w16m1_stream_*`・`w16m1b_reader_stamps_*`・`ui::mirror_view::tests` の inline の sh 4 本を寄せた）。
    /// `{d}` は dir の path に置き換える（例: `"cat {d}/frames.bin; sleep 1; cat {d}/gone.bin; cat >/dev/null"`）。dir に置く file はこれを呼んだ後に書く（前の run の残りを先に消すため）。
    /// 例: `let (sh, dir) = fake_stream_helper("cat >> {d}/stdin"); let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl"))?)?;`
    /// 保証する: dir は test ごと・process ごとに別（`#[path]` で取り込む別の module の試験と同時に走っても混ざらない）・前の run の残りは消す・実行 bit あり。
    /// 保証しない: `input` の答え（それは `fake_helper(bool)`・w16m0 / w16m2 の 7 本〈8 か所〉が使うので署名を変えない）。
    pub(crate) fn fake_stream_helper(stream_sh: &str) -> (PathBuf, PathBuf) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "w16m1-stream-{}-{}-{}",
            module_path!().replace("::", "-"),
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir); // pid の使い回しで前の run の記録が残っていても混ぜない
        std::fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("helper");
        let stream_sh = stream_sh.replace("{d}", &dir.display().to_string());
        std::fs::write(&sh, format!("#!/bin/sh\ncase \"$1\" in\n status) echo '{{\"screen_capture\":true}}';;\n stream) {stream_sh};;\nesac\n")).unwrap();
        std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        (sh, dir)
    }

    fn target(bundle: &str) -> MirrorTarget {
        MirrorTarget {
            window_id: 7,
            pid: 42,
            bundle_id: bundle.into(),
            app_name: "TextEdit".into(),
        }
    }

    /// G87: 製品の launcher。動いている app（Finder）は if_stopped で起こさず表示名だけ・入っていない bundle id は NotInstalled（名前 = 最後の語の頭を大文字に）。
    /// 実の app は起こさない（動いている Finder と、無い id だけを渡す）。負の対照（commit message）: lsappinfo の名前を読まない → Finder の名前が赤
    #[test]
    fn g87_launch_app_reads_running_apps_and_unknown_bundles() {
        assert_eq!(launch_app("com.apple.finder", true), Ok(("Finder".into(), false)), "動いている Finder を起こした・名前を読めない");
        assert_eq!(launch_app("com.example.shibubu-g87-missing", true), Err(MirrorError::NotInstalled { app_name: "Shibubu-g87-missing".into() }));
    }

    /// G90: 前面の app の bundle id を読める（GUI の login の上では前面の app が必ず 1 つ在る）。
    /// 負の対照（commit message）: `0x` を補わない → lsappinfo の info が空で None → 赤
    #[test]
    fn g90_front_app_reads_the_frontmost_bundle_id() {
        let id = front_app().expect("前面の app の bundle id を読めない");
        assert!(id.contains('.') && !id.contains('"'), "{id}");
    }

    #[test]
    fn w16m0_consent_is_per_app_and_absent_by_default() {
        let mut gate = ConsentGate::default();
        let te = target("com.apple.TextEdit");
        assert_eq!(
            gate.check(&te),
            Err(MirrorError::NoConsent {
                app_name: "TextEdit".into()
            })
        );
        let (g, rec) = gate.grant(&te, 1.0);
        assert_eq!(g.bundle_id(), "com.apple.TextEdit");
        assert!(gate.check(&te).is_ok());
        assert!(
            gate.check(&target("com.cockos.reaper")).is_err(),
            "TextEdit の同意は REAPER に効かない"
        );
        // 保存した記録から作り直しても同じ・取り消すと無い
        let mut again = ConsentGate::from_records([rec]);
        assert!(again.check(&te).is_ok());
        assert!(again.revoke("com.apple.TextEdit"));
        assert!(again.check(&te).is_err());
    }

    #[test]
    fn w16m0_records_reject_unknown_fields() {
        let r = RelayRecord {
            v: RELAY_SCHEMA_V,
            seq: 1,
            at_ms: 1.5,
            kind: InputKind::Key,
            route: InputRoute::SaveDocument,
            pid: 42,
            window_id: 7,
            pos: None,
            outcome: None,
            ms: None,
        };
        let mut v = serde_json::to_value(&r).unwrap();
        assert_eq!(serde_json::from_value::<RelayRecord>(v.clone()).unwrap(), r);
        v["origin"] = json!("automated");
        assert!(serde_json::from_value::<RelayRecord>(v).is_err());
        let mut c =
            json!({"v": CONSENT_SCHEMA_V, "bundle_id": "com.apple.TextEdit", "granted_at_ms": 1.0});
        assert!(serde_json::from_value::<ConsentRecord>(c.clone()).is_ok());
        c["pid"] = json!(42);
        assert!(serde_json::from_value::<ConsentRecord>(c).is_err());
    }

    #[test]
    fn w16m0_input_matches_the_helper_line() {
        let i = MirrorInput::Click {
            x: 0.5,
            y: 0.25,
            count: 1,
        };
        assert_eq!(
            serde_json::to_value(&i).unwrap(),
            json!({"op": "click", "x": 0.5, "y": 0.25, "count": 1})
        );
        assert_eq!((i.kind(), i.pos()), (InputKind::Click, Some([0.5, 0.25])));
        let k = MirrorInput::Key {
            key: "s".into(),
            mods: vec![KeyMod::Cmd],
        };
        assert_eq!(
            serde_json::to_value(&k).unwrap(),
            json!({"op": "key", "key": "s", "mods": ["cmd"]})
        );
        assert_eq!(k.pos(), None);
        // drag の steps は helper まで落とさず渡す（measure の 10 が helper の既定 8 に化けない）
        let d = json!({"op": "drag", "x": 0.5, "y": 0.9, "x2": 0.7, "y2": 0.9, "steps": 10});
        let di: MirrorInput = serde_json::from_value(d.clone()).unwrap();
        assert_eq!(serde_json::to_value(&di).unwrap(), d);
        // 知らない field は黙って捨てず拒む
        assert!(serde_json::from_value::<MirrorInput>(
            json!({"op": "click", "x": 0.5, "y": 0.5, "count": 1, "stpes": 3})
        )
        .is_err());
    }

    /// W16M-2: 入力 → 道の表（文字・key・⌘ の移動は postToPid・⌘S は Apple Event の save・⌘Z などの menu の key と届く道の無い mouse は NeedsFront・
    /// 1 回の click は AX の点）。表の根拠は 2026-10-21 の実測（InputRoute の doc）
    #[test]
    fn w16m2_route_for_is_the_w16m2_table() {
        let key = |k: &str, mods: &[KeyMod]| MirrorInput::Key { key: k.into(), mods: mods.to_vec() };
        let (cmd, shift, opt, ctrl) = (KeyMod::Cmd, KeyMod::Shift, KeyMod::Option, KeyMod::Control);
        assert_eq!(route_for(&key("s", &[cmd])), InputRoute::SaveDocument);
        assert_eq!(route_for(&key("S", &[cmd])), InputRoute::SaveDocument);
        // menu の key equivalent は背景の app で処理されない（⌘Z・⇧⌘Z・⌘A は実測で何も起きない）。⌥⌘S・⇧⌘S は保存ではない
        for mods in [&[cmd][..], &[shift, cmd][..]] {
            assert_eq!(route_for(&key("z", mods)), InputRoute::NeedsFront, "{mods:?}");
        }
        assert_eq!(route_for(&key("a", &[cmd])), InputRoute::NeedsFront);
        assert_eq!(route_for(&key("s", &[cmd, opt])), InputRoute::NeedsFront);
        assert_eq!(route_for(&key("s", &[shift, cmd])), InputRoute::NeedsFront);
        assert_eq!(route_for(&key("return", &[cmd])), InputRoute::NeedsFront);
        // ⌘ と移動の key・⌃ の key は文字の欄の key binding で届く（⌘←・⌃E は実測で届いた）
        assert_eq!(route_for(&key("left", &[cmd])), InputRoute::PostToPid);
        assert_eq!(route_for(&key("Right", &[shift, cmd])), InputRoute::PostToPid);
        assert_eq!(route_for(&key("delete", &[cmd])), InputRoute::PostToPid);
        assert_eq!(route_for(&key("e", &[ctrl])), InputRoute::PostToPid);
        assert_eq!(route_for(&key("s", &[])), InputRoute::PostToPid);
        assert_eq!(route_for(&key("return", &[])), InputRoute::PostToPid);
        assert_eq!(route_for(&MirrorInput::Text { text: "メモ".into() }), InputRoute::PostToPid);
        assert_eq!(route_for(&MirrorInput::Click { x: 0.5, y: 0.5, count: 1 }), InputRoute::AxAtPoint);
        assert_eq!(route_for(&MirrorInput::Click { x: 0.5, y: 0.5, count: 2 }), InputRoute::NeedsFront);
        assert_eq!(route_for(&MirrorInput::Scroll { x: 0.5, y: 0.5, dx: 0, dy: -120 }), InputRoute::NeedsFront);
        assert_eq!(route_for(&MirrorInput::Drag { x: 0.1, y: 0.1, x2: 0.5, y2: 0.1, steps: None }), InputRoute::NeedsFront);
        // 記録の形（JSONL）と helper の行の "route": 道は snake_case（KangaeMirror.swift の Route の rawValue と同じ語）
        assert_eq!(serde_json::to_value(InputRoute::SaveDocument).unwrap(), json!("save_document"));
        assert_eq!(serde_json::to_value(InputRoute::AxAtPoint).unwrap(), json!("ax_at_point"));
        assert_eq!(serde_json::to_value(InputRoute::PostToPid).unwrap(), json!("post_to_pid"));
    }

    /// W16M-2: frame の pixel → 元の窓の割合（Retina の倍率と余白を引く・PoC の to_window から移した）
    #[test]
    fn w16m2_frame_point_to_window_subtracts_retina_and_letterbox() {
        // 960×600 の frame に、point で (10, 0, 460, 300) の中身・scaleFactor 2（= pixel で 20..940 × 0..600）
        let m = MirrorFrameMeta { seq: 1, width: 960, height: 600, captured_ms: 0.0, content: [10.0, 0.0, 460.0, 300.0], factor: 2.0 };
        assert_eq!(frame_point_to_window(&m, 20.0, 0.0), Some([0.0, 0.0]));
        assert_eq!(frame_point_to_window(&m, 480.0, 300.0), Some([0.5, 0.5]));
        assert_eq!(frame_point_to_window(&m, 940.0, 600.0), Some([1.0, 1.0]));
        // 余白（letterbox）は元の窓に当たらない
        assert_eq!(frame_point_to_window(&m, 10.0, 300.0), None);
        assert_eq!(frame_point_to_window(&m, 950.0, 300.0), None);
        // contentRect が 0 なら frame 全体
        let whole = MirrorFrameMeta { content: [0.0; 4], ..m };
        assert_eq!(frame_point_to_window(&whole, 480.0, 300.0), Some([0.5, 0.5]));
        // W16M-3: 逆（caret を描く）は同じ範囲で往復する
        assert_eq!(window_point_to_frame(&m, 0.5, 0.5), [480.0, 300.0]);
        assert_eq!(window_point_to_frame(&m, 0.0, 1.0), [20.0, 600.0]);
        assert_eq!(window_point_to_frame(&whole, 0.25, 0.5), [240.0, 300.0]);
    }

    /// helper の tag 1 の payload を作る（KangaeMirror.swift の stream と同じ並び）。
    fn frame_payload(header: Value, rgba_len: usize) -> Vec<u8> {
        let hd = serde_json::to_vec(&header).unwrap();
        let mut p = (hd.len() as u32).to_le_bytes().to_vec();
        p.extend_from_slice(&hd);
        p.extend(std::iter::repeat(7u8).take(rgba_len));
        p
    }

    fn header(w: u32, h: u32) -> Value {
        json!({"seq": 3, "w": w, "h": h, "captured_ms": 10.5, "sent_ms": 11.0,
               "content": {"x": 0.0, "y": 4.0, "w": 2.0, "h": 1.0}, "scale": 1.0, "factor": 2.0})
    }

    /// G97: 読み手の thread が置いた frame・止まった事は UI を起こす（後ろの Shibubu が 33ms で見に来なくても映る・止まった 1 行が出る）。
    /// 負の対照: `Sink::frame` の `self.wake()` を外す → 赤。
    #[test]
    fn g97_a_frame_from_the_helper_wakes_the_ui() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        struct Count(AtomicUsize);
        impl std::task::Wake for Count {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, SeqCst);
            }
        }
        let video = Arc::new(Mutex::new(Video::default()));
        let (tx, _rx) = mpsc::sync_channel(EVENT_CAP);
        let sink = Sink { video: Arc::clone(&video), events: tx, app_name: String::new() };
        let n = Arc::new(Count(AtomicUsize::new(0)));
        video.lock().unwrap().wake = Some(Waker::from(Arc::clone(&n)));
        sink.frame(frame_payload(header(4, 2), 4 * 2 * 4));
        assert!(video.lock().unwrap().latest.is_some(), "前提: frame が置かれていない");
        assert_eq!(n.0.load(SeqCst), 1, "着いた frame が UI を起こさない");
        sink.event(json!({"kind": "stopped", "reason": "window_gone"}));
        assert!(n.0.load(SeqCst) >= 2, "止まった事が UI を起こさない");
    }

    /// W16M-1: frame の形の照合（黒や崩れた texture を作らない）
    #[test]
    fn w16m1_parse_frame_checks_the_header_and_the_rgba_length() {
        let f = parse_frame(frame_payload(header(4, 2), 4 * 2 * 4)).unwrap();
        assert_eq!(
            f.meta,
            MirrorFrameMeta { seq: 3, width: 4, height: 2, captured_ms: 10.5, content: [0.0, 4.0, 2.0, 1.0], factor: 2.0 }
        );
        assert_eq!(f.rgba, vec![7u8; 32]);
        // RGBA が 1 byte 足りない・多い → Err
        assert!(matches!(parse_frame(frame_payload(header(4, 2), 31)), Err(MirrorError::Helper(_))));
        assert!(matches!(parse_frame(frame_payload(header(4, 2), 33)), Err(MirrorError::Helper(_))));
        // header の欄が欠けた → Err（0 で埋めない）
        for k in ["seq", "w", "h", "captured_ms", "content"] {
            let mut h = header(4, 2);
            h.as_object_mut().unwrap().remove(k);
            assert!(parse_frame(frame_payload(h, 32)).is_err(), "{k} が無いのに通った");
        }
        // factor が無ければ 1
        let mut h = header(1, 1);
        h.as_object_mut().unwrap().remove("factor");
        assert_eq!(parse_frame(frame_payload(h, 4)).unwrap().meta.factor, 1.0);
        // header の長さが payload より長い・空 → Err
        assert!(parse_frame(vec![0xff, 0xff, 0, 0, b'{']).is_err());
        assert!(parse_frame(vec![]).is_err());
    }

    /// holdout（W16M-1）: stream は一番新しい frame だけを返し、窓が消えたら以後ずっと WindowGone（前の frame を今の物として返さない）
    #[test]
    fn w16m1_stream_returns_the_newest_frame_and_then_window_gone() {
        // 偽の helper: status は許可あり・stream は frame を 2 枚（seq 1・2）流し、1 秒後に window_gone
        let (sh, dir) = fake_stream_helper("cat {d}/frames.bin; sleep 1; cat {d}/gone.bin; cat >/dev/null");
        let mut h1 = header(1, 1);
        h1["seq"] = json!(1);
        let mut h2 = header(1, 1);
        h2["seq"] = json!(2);
        let mut bytes = msg(1, frame_payload(h1, 4));
        bytes.extend(msg(1, frame_payload(h2, 4)));
        std::fs::write(dir.join("frames.bin"), &bytes).unwrap();
        let gone = msg(2, serde_json::to_vec(&json!({"kind": "error", "reason": "window_gone"})).unwrap());
        std::fs::write(dir.join("gone.bin"), &gone).unwrap();
        let te = target("com.apple.TextEdit");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        // 2 枚とも着いた後の 1 回目は seq 2 だけ（seq 1 は捨てる）・2 回目は新しい物が無いので None
        assert_eq!(m.stream().unwrap().map(|f| f.meta.seq), Some(2));
        assert_eq!(m.stream().unwrap(), None);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert_eq!(m.stream().unwrap_err(), MirrorError::WindowGone);
        assert_eq!(m.stream().unwrap_err(), MirrorError::WindowGone, "窓が消えた後に Ok を返した");
    }

    /// MIRROR-SPEED-1101: 1 分の 1 行の数（中央値と 95% 点・最も近い順位）。
    /// 負の対照: `percentiles` の並べ替えを外す → 逆順の 1..=100 の中央値が 51 で赤。
    #[test]
    fn mirror_speed_percentiles_are_median_and_p95() {
        let mut ms: Vec<f64> = (1..=100u32).rev().map(f64::from).collect();
        assert_eq!(percentiles(&mut ms), Some((50.0, 95.0)));
        assert_eq!(percentiles(&mut [7.0]), Some((7.0, 7.0)));
        assert_eq!(percentiles(&mut [30.0, 10.0, 20.0]), Some((20.0, 30.0)));
        assert_eq!(percentiles(&mut Vec::<f64>::new()), None, "frame の無い分を 0 ms と言った");
    }

    /// MIRROR-SPEED-1101: 描く大きさは helper の stdin の 1 行で（起こし直さない）・起こす helper には撮る枚数（`--smooth`）を渡す。
    /// `set_smoothness` は全部の helper の stdin に書くので、ここでは呼ばない（並んで走る他の試験の偽の helper の stdin を汚す）。
    /// 負の対照: `MirrorSurface::view` の send を外す → stdin に view の行が無くて赤。
    #[test]
    fn mirror_speed_view_reaches_the_helper_and_spawns_carry_the_smoothness() {
        let (sh, dir) = fake_stream_helper("echo \"$@\" > {d}/args; cat >> {d}/stdin");
        let te = target("com.apple.TextEdit");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        m.view(1600, 1000);
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while (read("stdin").is_empty() || read("args").is_empty()) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let lines: Vec<Value> = read("stdin").lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        assert_eq!(lines, [json!({"cmd": "view", "stream": 1, "w": 1600, "h": 1000})], "描く大きさが helper に届かない");
        assert!(read("args").contains("--smooth "), "起こした helper に撮る枚数を渡していない: {}", read("args"));
    }

    /// MIRROR-SPEED-1101: helper の mach message（KangaeMirror.swift の頭の `--surfaces` の形）で渡した IOSurface は、header の stream の面に写さずに届く
    /// （`rgba` は空・`surface` は送った面・持っている間は use count が上がる = SCK の pool が書かない・放すと下がる）。送り手の pid が helper でない 1 通は捨てる。
    /// 負の対照: `read_surfaces` の pid の照合を外す → 2 つ目の受け口（helper の pid を別の数と思っている）にも届いて赤・
    /// `IOSurfaceIncrementUseCount` を外す → 持っている間の in use が false で赤。
    #[cfg(target_os = "macos")]
    #[test]
    fn mirror_speed_iosurface_frames_arrive_without_copying() {
        use std::ffi::{c_char, c_void, CString};
        #[link(name = "IOSurface", kind = "framework")]
        extern "C" {
            static kIOSurfaceWidth: *const c_void;
            static kIOSurfaceHeight: *const c_void;
            static kIOSurfaceBytesPerElement: *const c_void;
            static kIOSurfacePixelFormat: *const c_void;
            fn IOSurfaceCreate(props: *const c_void) -> *mut c_void;
            fn IOSurfaceCreateMachPort(s: *mut c_void) -> u32;
            fn IOSurfaceGetID(s: *mut c_void) -> u32;
            fn IOSurfaceIsInUse(s: *mut c_void) -> u8;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            static kCFTypeDictionaryKeyCallBacks: [usize; 6];
            static kCFTypeDictionaryValueCallBacks: [usize; 5];
            fn CFDictionaryCreateMutable(alloc: *const c_void, cap: isize, keys: *const c_void, values: *const c_void) -> *mut c_void;
            fn CFDictionarySetValue(d: *mut c_void, k: *const c_void, v: *const c_void);
            fn CFNumberCreate(alloc: *const c_void, ty: isize, v: *const c_void) -> *mut c_void;
            fn CFRelease(cf: *const c_void);
        }
        extern "C" {
            static bootstrap_port: u32;
            fn bootstrap_look_up(bp: u32, name: *const c_char, sp: *mut u32) -> i32;
            fn mach_msg(msg: *mut c_void, option: i32, send_size: u32, rcv_size: u32, rcv_name: u32, timeout: u32, notify: u32) -> i32;
        }
        // 64×32 の BGRA の IOSurface（SCK の frame と同じ形）
        let surface = unsafe {
            let d = CFDictionaryCreateMutable(std::ptr::null(), 0, std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(), std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast());
            for (k, v) in [(kIOSurfaceWidth, 64i32), (kIOSurfaceHeight, 32), (kIOSurfaceBytesPerElement, 4), (kIOSurfacePixelFormat, 0x4247_5241)] {
                let n = CFNumberCreate(std::ptr::null(), 3, (&v as *const i32).cast()); // kCFNumberSInt32Type
                CFDictionarySetValue(d, k, n);
                CFRelease(n);
            }
            let s = IOSurfaceCreate(d);
            CFRelease(d);
            s
        };
        assert!(!surface.is_null(), "前提: IOSurface を作れない");
        // helper（KangaeMirror.swift の Surfaces.send）と同じ 1 通: header 24 byte・記述子 1 つ・[len u32][header JSON]
        let send = |name: &str, header: &Value| unsafe {
            let c = CString::new(name).unwrap();
            let mut server = 0u32;
            assert_eq!(bootstrap_look_up(bootstrap_port, c.as_ptr(), &mut server), 0, "前提: 受け口の名前を引けない");
            let body = serde_json::to_vec(header).unwrap();
            let size = 44 + body.len().div_ceil(4) * 4;
            let mut m = vec![0u32; size / 4];
            (m[0], m[1], m[2], m[5], m[6]) = (0x8000_0013, size as u32, server, 0x5348, 1);
            (m[7], m[9], m[10]) = (IOSurfaceCreateMachPort(surface), 17 << 16, body.len() as u32);
            std::slice::from_raw_parts_mut(m.as_mut_ptr().cast::<u8>(), size)[44..44 + body.len()].copy_from_slice(&body);
            assert_eq!(mach_msg(m.as_mut_ptr().cast(), 0x11, size as u32, 0, 0, 1000, 0), 0, "前提: mach message を送れない");
        };
        let mut h = header(64, 32);
        h["stream"] = json!(1);
        let me = std::process::id();
        let (tx, _rx) = mpsc::sync_channel(EVENT_CAP);
        // 1 つ目: helper の pid = この process → 届く
        let video = Arc::new(Mutex::new(Video::default()));
        let routes = one_route(Arc::clone(&video), tx.clone());
        let (name, port) = inbox().expect("前提: bootstrap_check_in で受け口を作れない");
        let r = Arc::clone(&routes);
        std::thread::spawn(move || read_surfaces(port, me, r));
        send(&name, &h);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while video.lock().unwrap().latest.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let f = video.lock().unwrap().latest.take().expect("渡した IOSurface の frame が面に届かない");
        let s = f.surface.clone().expect("frame が IOSurface を持たない（写した）");
        assert_eq!((s.id(), s.size(), f.rgba.len(), f.meta.seq), (unsafe { IOSurfaceGetID(surface) }, (64, 32), 0, 3));
        assert_ne!(unsafe { IOSurfaceIsInUse(surface) }, 0, "持っている間に use count を上げていない（SCK の pool が描いている面に書く）");
        drop((f, s));
        assert_eq!(unsafe { IOSurfaceIsInUse(surface) }, 0, "放した後も use count が残る（SCK の pool が尽きる）");
        // 2 つ目: helper の pid を別の数と思っている受け口 → 捨てる
        let video2 = Arc::new(Mutex::new(Video::default()));
        let routes2 = one_route(Arc::clone(&video2), tx);
        let (name2, port2) = inbox().expect("前提: 2 つ目の受け口を作れない");
        let r = Arc::clone(&routes2);
        std::thread::spawn(move || read_surfaces(port2, me + 1, r));
        send(&name2, &h);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(video2.lock().unwrap().latest.is_none(), "helper でない pid の IOSurface を映した");
        assert_eq!(unsafe { IOSurfaceIsInUse(surface) }, 0, "捨てた 1 通の use count が残る");
        routes.lock().unwrap().dead = true;
        routes2.lock().unwrap().dead = true;
        unsafe { CFRelease(surface) };
    }

    /// 読み手の thread の表に stream 1 だけ（起こした時の窓・`"stream"` の無い行の宛先）
    fn one_route(video: Arc<Mutex<Video>>, events: mpsc::SyncSender<Value>) -> Arc<Mutex<Routes>> {
        Arc::new(Mutex::new(Routes { streams: HashMap::from([(1, Sink { video, events, app_name: "Music".into() })]), ..Routes::default() }))
    }

    /// W16M-1: 本物の helper（KangaeMirror.swift の 2 秒の見張り・起こした時の確認）は窓の消えと許可の無さを
    /// `{"kind":"stopped","reason":..}` で出して exit する。それが WindowGone / ScreenCaptureDenied になる（EOF の Helper にならない）。
    #[test]
    fn w16m1_stopped_from_the_real_helper_maps_to_window_gone_and_denied() {
        for (reason, want) in [
            ("window_gone", MirrorError::WindowGone),
            ("no_screen_capture_permission", MirrorError::ScreenCaptureDenied),
        ] {
            let video = Arc::new(Mutex::new(Video::default()));
            let (tx, _rx) = mpsc::sync_channel(EVENT_CAP);
            read_helper(std::io::Cursor::new(msg(2, serde_json::to_vec(&json!({"kind": "stopped", "reason": reason})).unwrap())), one_route(Arc::clone(&video), tx));
            assert_eq!(video.lock().unwrap().fatal, Some(want), "stopped/{reason}");
        }
    }

    /// W16M-1: Page に出る 1 行は、使う人が出来る次の一手だけ（内部の語・path を出さない・Page に無い「選ぶ」を頼まない）
    #[test]
    fn w16m1_page_lines_have_no_internal_words_or_paths() {
        let missing = MirrorError::HelperMissing(PathBuf::from("/Users/x/Shibubu.app/Contents/MacOS/kangae-mirror-helper")).to_string();
        assert!(!missing.contains('/') && !missing.contains("helper"), "{missing}");
        let gone = MirrorError::WindowGone.to_string();
        assert!(!gone.contains("choose") && gone.contains("The app's window closed"), "{gone}");
        // W16M-3（反証）: helper の止まった理由（stopped の reason・list の時間切れ・起こせない）は内部の語を出さず、次の一手の 1 行
        let stopped = ["stream_error", "capture_timeout", "shareable_failed", "start_failed"].map(helper_error);
        let others = [MirrorError::Helper("list の答えを読めません（exit status: 2）".into()), MirrorError::Helper("/x/kangae-mirror-helper: No such file or directory (os error 2)".into())];
        for e in stopped.into_iter().chain(others) {
            let line = e.to_string();
            assert!(!line.contains("helper") && !line.contains('_') && !line.contains("exit status") && !line.contains('/'), "内部の語: {line}（{e:?}）");
            assert!(line.contains("Retrying") && line.contains("restart Shibubu"), "次の一手が無い: {line}");
        }
    }

    /// W16M-1: Page が見えていない間（誰も take_event を呼ばない）も、2 秒ごとの stats で出来事が際限なく溜まらない
    #[test]
    fn w16m1_events_do_not_pile_up_while_nobody_reads_them() {
        let video = Arc::new(Mutex::new(Video::default()));
        let (tx, rx) = mpsc::sync_channel(EVENT_CAP);
        let bytes: Vec<u8> = (0..1000).flat_map(|i| msg(2, serde_json::to_vec(&json!({"kind": "stats", "frames": i})).unwrap())).collect();
        read_helper(std::io::Cursor::new(bytes), one_route(video, tx));
        let n = rx.try_iter().count();
        // 送った 1000 件より少なく、上限まで（上限が大きすぎても赤）
        assert!(n >= 1 && n <= EVENT_CAP && n < 1000, "溜まった出来事 {n} 件");
    }

    fn frame_rec(seq: u64, captured: f64, received: f64, shown: f64) -> FrameRecord {
        FrameRecord { v: FRAME_SCHEMA_V, surface_id: "notes".into(), seq, captured_ms: captured, received_ms: received, shown_ms: shown }
    }

    /// W16M-1b: 遅れを helper の側（撮る・送る）と描き直しの側に分ける。書く前に撮った frame・別の面の記録は数えない・記録 0 件は未測
    #[test]
    fn w16m1b_frame_latency_splits_capture_pipe_and_redraw() {
        let t = 10_000.0;
        let mut other = frame_rec(9, 10_050.0, 10_060.0, 10_070.0);
        other.surface_id = "other".into();
        let recs = [
            frame_rec(1, 9_900.0, 9_910.0, 12_000.0), // 書く前に撮った frame（遅れて上がっても数えない）
            other,
            frame_rec(3, 10_400.0, 10_450.0, 15_000.0),
            frame_rec(2, 10_100.0, 10_120.0, 14_000.0),
        ];
        let l = frame_latency(&recs, "notes", t).unwrap();
        assert_eq!(l, FrameLatency { seq: 2, capture_ms: 100.0, pipe_ms: 20.0, redraw_ms: 3_880.0, total_ms: 4_000.0 });
        assert!(frame_latency(&recs[..1], "notes", t).unwrap_err().contains("Not measured"));
        assert!(frame_latency(&[], "notes", t).unwrap_err().contains("Not measured"));
        // FrameLog の往復（RelayLog と同じ JSONL の口）と、知らない欄の拒否
        let p = std::env::temp_dir().join(format!("w16m1b-frames-{}-{}.jsonl", module_path!().replace("::", "-"), std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut log = FrameLog::open(&p).unwrap();
        log.append(&recs[2]).unwrap();
        assert_eq!(FrameLog::read(&p).unwrap(), vec![recs[2].clone()]);
        std::fs::write(&p, r#"{"v":1,"surface_id":"notes","seq":1,"captured_ms":1,"received_ms":1,"shown_ms":1,"x":0}"#).unwrap();
        assert!(FrameLog::read(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// holdout（W16M-1b）: 読み手の thread は frame ごとに受け取った時刻を置く（形の違う frame は捨てる）。
    /// 起こし（`on_frame`）の半分は、実測で要らなかったので口ごと消した（docs/evidence/W16M-1b/log.md）。
    #[test]
    fn w16m1b_reader_stamps_received_ms() {
        // 1 秒待ってから（open の後に）frame を 2 枚・その間に形の違う frame を 1 枚
        let (sh, dir) = fake_stream_helper("sleep 1; cat {d}/frames.bin; cat >/dev/null");
        let mut bytes = msg(1, frame_payload(header(1, 1), 4));
        bytes.extend(msg(1, frame_payload(header(1, 1), 3)));
        bytes.extend(msg(1, frame_payload(header(1, 1), 4)));
        std::fs::write(dir.join("frames.bin"), &bytes).unwrap();
        let te = target("com.apple.TextEdit");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&sh, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        let t0 = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let f = m.stream().unwrap().expect("frame が着いている");
        assert!(f.received_ms >= t0 && f.received_ms <= now_ms(), "received_ms {} が受け取った時刻でない", f.received_ms);
    }

    /// 反証（W16M-1b）: 記録の時刻は小数の ms でも書いた値のまま読み戻る（serde_json の既定の読みは最後の 1 桁がずれ、w16m0 の試験が時々赤くなった）
    #[test]
    fn w16m1b_jsonl_log_reads_back_fractional_ms_exactly() {
        let dir = std::env::temp_dir().join(format!("w16m1b-f64-{}-{}", module_path!().replace("::", "-"), std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("relay.jsonl");
        let mut log = RelayLog::open(&p).unwrap();
        // 実測で読み戻しがずれた値（1792524649386.5981 が .598 で読み戻る等）
        let recs: Vec<RelayRecord> = [1792524649386.5981, 1792524759403.0781, 1792525053115.8281, 1792525104134.0999]
            .iter()
            .enumerate()
            .map(|(i, &at_ms)| RelayRecord { v: RELAY_SCHEMA_V, seq: i as u64 + 1, at_ms, kind: InputKind::Text, route: InputRoute::PostToPid, pid: 42, window_id: 7, pos: None, outcome: None, ms: None })
            .collect();
        for r in &recs {
            log.append(r).unwrap();
        }
        assert_eq!(RelayLog::read(&p).unwrap(), recs);
    }

    /// MIRROR-HYBRID-1101 追記 235 の 3: 送った押しの答えから答えの行（届いた = sent の sent_ms までの ms・断った = 理由と着いた時までの ms）。答えでない出来事は None。
    /// 記録の file に書いて読み戻せる（`deny_unknown_fields` の下で新しい 2 つの field を読む・前の形の行〈field 無し〉も読める）。
    /// 負の対照（未実行）: `outcome` を書かない → 読み戻しの outcome が None で赤・`ms` を at_ms からでなく 0 から数える → 120 でなく 1120 で赤
    #[test]
    fn hybrid235_each_relayed_press_gets_its_outcome_and_ms() {
        let rec = RelayRecord { v: RELAY_SCHEMA_V, seq: 3, at_ms: 1000.0, kind: InputKind::Click, route: InputRoute::Front, pid: 42, window_id: 7, pos: Some([0.5, 0.5]), outcome: None, ms: None };
        let sent = answered(&rec, &json!({"kind": "sent", "id": 3, "op": "click", "route": "front", "sent_ms": 1120.0}), 1200.0).unwrap();
        assert_eq!((sent.seq, sent.outcome.as_deref(), sent.ms, sent.at_ms), (3, Some("delivered"), Some(120.0), 1200.0));
        let refused = answered(&rec, &json!({"kind": "error", "id": 3, "op": "click", "reason": "not_front"}), 1500.0).unwrap();
        assert_eq!((refused.outcome.as_deref(), refused.ms), (Some("refused: not_front"), Some(500.0)));
        assert!(answered(&rec, &json!({"kind": "stats", "fps": 30}), 1500.0).is_none(), "答えでない出来事を答えの行にした");
        let dir = std::env::temp_dir().join(format!("shibubu-hy235-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("relay.jsonl");
        std::fs::write(&p, "{\"v\":2,\"seq\":3,\"at_ms\":1000.0,\"kind\":\"click\",\"route\":\"ax_at_point\",\"pid\":42,\"window_id\":7,\"pos\":[0.5,0.5]}\n").unwrap();
        let mut log = RelayLog::open(&p).unwrap();
        log.append(&sent).unwrap();
        let back = RelayLog::read(&p).unwrap();
        assert_eq!((back.len(), back[0].outcome.clone(), back[1].clone()), (2, None, sent), "前の形の行と答えの行を読み戻せない");
    }

    /// holdout（W16M-1b）: App Nap を止める印は macOS で取れて、Drop で終える（panic しない・2 回続けて取れる）
    #[test]
    fn w16m1b_activity_hold_begins_and_ends() {
        let a = ActivityHold::begin("試験");
        assert_eq!(a.is_some(), cfg!(target_os = "macos"));
        drop(a);
        assert_eq!(ActivityHold::begin("試験 2").is_some(), cfg!(target_os = "macos"));
    }

    /// W16M-1: find は bundle id と題名で選び、無ければ NoWindow（app を起こさない）・許可が無い list（exit 3）は ScreenCaptureDenied
    #[test]
    fn w16m1_find_picks_the_window_by_bundle_and_title() {
        let dir = std::env::temp_dir().join(format!("w16m1-find-{}-{}", module_path!().replace("::", "-"), std::process::id())); // 同じ file を #[path] で取り込む別の module の試験と同時に走る
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let list = json!([
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 50, "title": "README.md", "window_id": 11},
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 50, "title": "", "window_id": 12},
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 50, "title": "notes.md", "window_id": 13},
            {"app": "Finder", "bundle_id": "com.apple.finder", "pid": 9, "title": "notes.md", "window_id": 14}
        ]);
        let ok = dir.join("helper");
        std::fs::write(&ok, format!("#!/bin/sh\necho '{list}'\n")).unwrap();
        let denied = dir.join("denied");
        std::fs::write(&denied, "#!/bin/sh\necho '[]'\nexit 3\n").unwrap();
        for p in [&ok, &denied] {
            std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        let t = MirrorTarget::find(&ok, "com.apple.TextEdit", Some("notes.md"), None, false).unwrap();
        assert_eq!((t.window_id, t.pid, t.app_name.as_str()), (13, 50, "TextEdit"), "README の窓や Finder の notes.md を選んだ");
        assert_eq!(MirrorTarget::find(&ok, "com.apple.TextEdit", None, None, false).unwrap().window_id, 11);
        assert_eq!(
            MirrorTarget::find(&ok, "com.apple.TextEdit", Some("todo.md"), None, false),
            Err(MirrorError::NoWindow { app_name: "TextEdit".into(), title_has: Some("todo.md".into()) })
        );
        assert_eq!(
            MirrorTarget::find(&ok, "com.cockos.reaper", None, None, false),
            Err(MirrorError::NoWindow { app_name: "reaper".into(), title_has: None })
        );
        assert_eq!(MirrorTarget::find(&denied, "com.apple.TextEdit", None, None, false), Err(MirrorError::ScreenCaptureDenied));
    }

    /// M4 ③（.app の実機で発見: .app に TextEdit を操作する許可が無く全部の窓の書類を引けず、list の先頭＝owner の別の書類の窓を映した）:
    /// 題名の合う窓のうち書類を引けない窓が 2 つ以上なら選ばない（`Ambiguous`）。許可がまだ決まっていない（-1744）なら尋ねる先の pid・断られた（-1743）ならシステム設定の 1 行・
    /// 許可でない理由（窓が無い -1728）なら窓を閉じる 1 行。`ask` = true の時だけ helper に `--ask` を渡す。負の対照: 数の判定を外すと先頭（owner の窓）を選んで赤。
    #[test]
    fn m4_find_does_not_guess_between_same_title_windows_it_cannot_tell_apart() {
        let dir = std::env::temp_dir().join(format!("m4-ambiguous-{}-{}", module_path!().replace("::", "-"), std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/docs")).unwrap();
        let mine = dir.join("repo/docs/notes.md");
        std::fs::write(&mine, "mine").unwrap();
        let list = r#"[{"app":"TextEdit","bundle_id":"com.apple.TextEdit","pid":486,"title":"notes.md","window_id":48},{"app":"TextEdit","bundle_id":"com.apple.TextEdit","pid":70,"title":"notes.md","window_id":410}]"#;
        // doc-path の答え（2 窓とも同じ理由）と、渡された引数の記録
        let sh = |name: &str, reason: &str| {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\necho \"$@\" >> {d}/{name}.args\ncase \"$1\" in\n list) echo '{list}';;\n doc-path) echo '{{\"path\":null,\"reason\":\"{reason}\"}}';;\nesac\n", d = dir.display())).unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            p
        };
        let find = |h: &Path, ask: bool| MirrorTarget::find(h, "com.apple.TextEdit", Some("notes.md"), Some(&mine), ask);
        let undecided = find(&sh("undecided", "not_permitted_-1744"), false).unwrap_err();
        assert_eq!(undecided, MirrorError::Ambiguous { app_name: "TextEdit".into(), title_has: Some("notes.md".into()), count: 2, ask_pid: Some(486), denied: false }, "確かめられない 2 窓の先頭を選んだ");
        assert_eq!(undecided.to_string(), "There are 2 TextEdit windows with notes.md open, and Shibubu can't tell which one is this project's document. Press “Check TextEdit's document” to allow it and it can tell the windows apart");
        let denied = find(&sh("denied", "not_permitted_-1743"), false).unwrap_err();
        assert!(matches!(denied, MirrorError::Ambiguous { denied: true, ask_pid: None, .. }), "{denied:?}");
        assert!(denied.to_string().ends_with("Automation, allow Shibubu to control TextEdit and it can tell the windows apart"), "{denied}");
        let gone = find(&sh("gone", "ae_-1728"), false).unwrap_err();
        assert!(matches!(gone, MirrorError::Ambiguous { denied: false, ask_pid: None, count: 2, .. }), "{gone:?}");
        assert!(gone.to_string().ends_with("Close the window you don't need and this one shows here"), "{gone}");
        for e in [&undecided, &denied, &gone] {
            let s = e.to_string();
            assert!(!s.contains("not_permitted") && !s.contains("pid") && !s.contains("486") && !s.contains("Apple Event"), "内部の語: {s}");
        }
        // ask は button を押した時だけ: false では --ask を渡さず、true では渡す
        let args = |name: &str| std::fs::read_to_string(dir.join(format!("{name}.args"))).unwrap_or_default();
        assert!(!args("undecided").contains("--ask"), "{}", args("undecided"));
        let _ = find(&sh("asked", "not_permitted_-1744"), true);
        assert!(args("asked").lines().filter(|l| l.starts_with("doc-path")).all(|l| l.ends_with("--ask")), "{}", args("asked"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G78（27 手の #5・W15 b74 の実測 = App Nap）: 背景に置かれた TextEdit は最初の AppleEvent に helper の 5 秒で答えず ae_-1712、その問いで起きて
    /// 直後の問いには答える。偽の helper は起動（pid）ごとに最初の n 回の doc-path を ae_-1712 にする（list = 2 つの起動の README.md・先頭が
    /// この Project の書類）。woke（n = 1）: 時間切れの窓にもう 1 回問う → この Project の窓に決まる（同意の確かめ・映写へ）。
    /// sleepy（n = 2）: 2 回目も時間切れ → 今どおり決めない（起動ごとに問い直しは 1 回だけ）。許可（-1743）・app が居ない（-600）は問い直さない。
    /// 負の対照（G78_MUT）: none = 問い直さない → woke が r19 の #5 の形（2 窓とも引けず Ambiguous）/ any = 理由を見ずに問い直す → 許可の窓を 2 回問う
    #[test]
    fn g78_find_asks_a_napping_app_once_more_after_a_time_out() {
        let dir = std::env::temp_dir().join(format!("g78-nap-{}-{}", module_path!().replace("::", "-"), std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).unwrap();
        std::fs::create_dir_all(dir.join("old")).unwrap();
        let (mine, old) = (dir.join("repo/README.md"), dir.join("old/README.md"));
        std::fs::write(&mine, "mine").unwrap();
        std::fs::write(&old, "old").unwrap();
        let list = r#"[{"app":"TextEdit","bundle_id":"com.apple.TextEdit","pid":1987,"title":"README.md","window_id":3834},{"app":"TextEdit","bundle_id":"com.apple.TextEdit","pid":2035,"title":"README.md","window_id":3841}]"#;
        // doc-path --pid P --window N（$3 = P・$5 = N）。起動ごとに問われた回数を n<P> に数え、最初の nap 回は reason、その後は窓の書類
        let sh = |name: &str, nap: u32, reason: &str| {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join("helper");
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\ncase \"$1\" in\n list) echo '{list}';;\n doc-path) n=$(cat {d}/n$3 2>/dev/null || echo 0); echo $((n+1)) > {d}/n$3\n  if [ $n -lt {nap} ]; then echo '{{\"path\":null,\"reason\":\"{reason}\"}}'; elif [ \"$5\" = 3834 ]; then echo '{{\"path\":\"{mine}\"}}'; else echo '{{\"path\":\"{old}\"}}'; fi;;\nesac\n",
                    d = d.display(),
                    mine = mine.display(),
                    old = old.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            let r = MirrorTarget::find(&p, "com.apple.TextEdit", Some("README.md"), Some(&mine), false).map(|t| (t.pid, t.window_id));
            let asked = |pid: u32| std::fs::read_to_string(d.join(format!("n{pid}"))).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
            (r, asked(1987), asked(2035))
        };
        assert_eq!(sh("woke", 1, "ae_-1712"), (Ok((1987, 3834)), 2, 0), "時間切れの窓に 1 回だけ問い直して、この Project の書類の窓に決める");
        let (r, a, b) = sh("sleepy", 2, "ae_-1712");
        assert!(matches!(r, Err(MirrorError::Ambiguous { count: 2, ask_pid: None, denied: false, .. })), "{r:?}");
        assert_eq!((a, b), (2, 2), "問い直しは起動ごとに 1 回だけ（足す待ちは起動ごとに helper の時間切れ 1 回分）");
        for reason in ["not_permitted_-1743", "ae_-600"] {
            let (r, a, b) = sh(&reason.replace('_', "-"), 9, reason);
            assert!(matches!(r, Err(MirrorError::Ambiguous { count: 2, .. })), "{reason}: {r:?}");
            assert_eq!((a, b), (1, 1), "{reason}: 時間切れでない理由は問い直さない");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W16M-3（M2 の実機の不具合: 同じ題名の別の書類の窓を選んで、その書類に打った）: 書類の path を渡すと、題名が同じでも書類が違う窓は選ばない。
    /// 偽の helper の list は前の run の notes.md（窓 21・先頭）・この Project の notes.md（窓 22）・書類を答えない notes.md（窓 23）と README（窓 11）。
    /// 負の対照は同じ試験の中: path を渡さない（前の選び方）と先頭の別の書類（窓 21）を選ぶ。
    #[test]
    fn w16m3_find_picks_the_window_of_the_document_path_not_the_first_same_title() {
        let dir = std::env::temp_dir().join(format!("w16m3-find-{}-{}", module_path!().replace("::", "-"), std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("old/docs")).unwrap();
        std::fs::create_dir_all(dir.join("repo/docs")).unwrap();
        let (old, mine) = (dir.join("old/docs/notes.md"), dir.join("repo/docs/notes.md"));
        std::fs::write(&old, "old").unwrap();
        std::fs::write(&mine, "mine").unwrap();
        let list = json!([
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 60, "title": "notes.md", "window_id": 21},
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 61, "title": "notes.md", "window_id": 22},
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 61, "title": "notes.md", "window_id": 23},
            {"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 61, "title": "README.md", "window_id": 11}
        ]);
        // doc-path --pid P --window N（$5 = N）。窓 23 は書類を引けない（許可が無い時の答えと同じ形）・窓 11 は答えの形が壊れている
        let sh = |name: &str, w22: &str| {
            let p = dir.join(name);
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\ncase \"$1\" in\n list) echo '{list}';;\n doc-path) echo \"$5\" >> {d}/asked; case \"$5\" in\n  21) echo '{{\"path\":\"{old}\"}}';;\n  22) echo '{{\"path\":\"{w22}\"}}';;\n  23) echo '{{\"path\":null,\"reason\":\"not_permitted_-1744\"}}';;\n  *) echo 'not json';;\n esac;;\nesac\n",
                    d = dir.display(),
                    old = old.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            p
        };
        let helper = sh("helper", &mine.display().to_string());
        let pick = |h: &Path, want: Option<&Path>| MirrorTarget::find(h, "com.apple.TextEdit", Some("notes.md"), want, false).map(|t| (t.pid, t.window_id));
        assert_eq!(pick(&helper, Some(&mine)), Ok((61, 22)), "同じ題名の別の書類（前の run の notes.md）か、書類を引けない窓を選んだ");
        // 負の対照: 書類の path を渡さない（M2 までの選び方）と先頭の別の書類を選ぶ
        assert_eq!(pick(&helper, None), Ok((60, 21)));
        // /private/tmp と /tmp のような別の綴りでも同じ書類（canonicalize）
        let via_dotdot = dir.join("repo/docs/../docs/notes.md");
        assert_eq!(pick(&helper, Some(&via_dotdot)), Ok((61, 22)));
        // この Project の書類の窓が無く、書類を引けない窓が在る → その窓（前の選び方と同じ・許可が無い人を止めない）
        let gone = sh("gone", "/nowhere/docs/notes.md");
        assert_eq!(pick(&gone, Some(&mine)), Ok((61, 23)));
        // 書類が別と分かった窓しか無い → NoWindow（別の書類に打たない）。1 行は file を言い、内部の語を出さない
        let only_old = dir.join("only-old");
        std::fs::write(
            &only_old,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n list) echo '[{{\"app\":\"TextEdit\",\"bundle_id\":\"com.apple.TextEdit\",\"pid\":60,\"title\":\"notes.md\",\"window_id\":21}}]';;\n doc-path) echo '{{\"path\":\"{}\"}}';;\nesac\n",
                old.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&only_old, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let e = MirrorTarget::find(&only_old, "com.apple.TextEdit", Some("notes.md"), Some(&mine), false).unwrap_err();
        assert_eq!(e, MirrorError::NoWindow { app_name: "TextEdit".into(), title_has: Some("notes.md".into()) });
        assert_eq!(e.to_string(), "No TextEdit window with notes.md open was found. Open notes.md in TextEdit and it shows here");
        // 書類を問うのは題名の合う窓だけ（README の窓 11 は問わない）
        let asked = std::fs::read_to_string(dir.join("asked")).unwrap();
        assert!(!asked.lines().any(|l| l == "11"), "題名の合わない窓の書類まで問うた: {asked:?}");
    }

    /// 偽の helper（罠の印と同じ形）: 呼ばれた subcommand を calls に足し、stream の stdin を stdin に溜める。
    /// status は `screen_capture`、input は sent を返す。
    fn fake_helper(screen_capture: bool) -> (PathBuf, PathBuf) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "w16m0-{}-{}-{}",
            module_path!().replace("::", "-"),
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir); // pid の使い回しで前の run の記録が残っていても混ぜない
        std::fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("helper");
        let d = dir.display();
        std::fs::write(
            &sh,
            format!(
                "#!/bin/sh\necho \"$1\" >> {d}/calls\ncase \"$1\" in\n status) echo '{{\"screen_capture\":{screen_capture}}}';;\n stream) cat >> {d}/stdin;;\n input) echo '{{\"kind\":\"sent\",\"sent_ms\":5}}';;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        (sh, dir)
    }

    fn lines(p: &Path) -> Vec<String> {
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// stream の stdin に n 行が着くまで待つ（sh の cat は別 process）
    fn wait_lines(p: &Path, n: usize) -> Vec<String> {
        for _ in 0..200 {
            if lines(p).len() >= n {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        lines(p)
    }

    /// holdout: 同意の前は stream も relay も 0 件
    #[test]
    fn w16m0_nothing_streams_or_relays_before_consent() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let log_path = dir.join("relay.jsonl");
        let gate = ConsentGate::default();
        // 同意が無い: grant が出ないので open に辿り着かない（compile で守る口）・helper は 1 度も呼ばれない
        assert!(gate.check(&te).is_err());
        assert!(lines(&dir.join("calls")).is_empty());
        // 別の app の同意で open しても NoConsent・helper を起こさない
        let (other, _) = ConsentGate::default().grant(&target("com.cockos.reaper"), 1.0);
        let e = MirrorSurface::open(
            &helper,
            te.clone(),
            other,
            RelayLog::open(&log_path).unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            e,
            MirrorError::NoConsent {
                app_name: "TextEdit".into()
            }
        );
        assert!(
            lines(&dir.join("calls")).is_empty(),
            "同意の前に helper を起こした"
        );
        // 同意はあるが画面収録の許可が無い: status だけ・stream は起こさない
        let (no_cap, cdir) = fake_helper(false);
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let e = MirrorSurface::open(&no_cap, te.clone(), g, RelayLog::open(&log_path).unwrap())
            .unwrap_err();
        assert_eq!(e, MirrorError::ScreenCaptureDenied);
        assert_eq!(lines(&cdir.join("calls")), ["status"]);
        assert_eq!(RelayLog::read(&log_path).unwrap(), vec![]);
        // 正の対照: 同意の後は stream が起き、人の入力 1 つ = 記録 1 行 = stdin 1 行（origin: human・relay_seq）
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&helper, te.clone(), g, RelayLog::open(&log_path).unwrap())
            .unwrap();
        let mut lease = ControlLease::human(0.0);
        lease.hand_to_agent(1.0);
        let r = m
            .relay_human(
                &mut lease,
                MirrorInput::Text {
                    text: "nonce".into(),
                },
            )
            .unwrap();
        assert!(lease.human_has_control(), "人の入力で lease が人に戻る");
        assert_eq!(
            (r.seq, r.kind, r.pid, r.window_id, r.pos),
            (1, InputKind::Text, 42, 7, None)
        );
        assert_eq!(RelayLog::read(&log_path).unwrap(), vec![r]);
        let sent: Value = serde_json::from_str(&wait_lines(&dir.join("stdin"), 1)[0]).unwrap();
        assert_eq!(
            (
                sent["origin"].as_str(),
                sent["relay_seq"].as_u64(),
                sent["op"].as_str()
            ),
            (Some("human"), Some(1), Some("text"))
        );
        assert_eq!(lines(&dir.join("calls")), ["status", "stream"]);
    }

    /// 記録が書けなければ送らない（E1 の順: relay → 値の変化）
    #[test]
    fn w16m0_relay_is_written_before_send_or_not_sent() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let log_path = dir.join("relay.jsonl");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m =
            MirrorSurface::open(&helper, te, g, RelayLog::open(&log_path).unwrap()).unwrap();
        let mut lease = ControlLease::human(0.0);
        m.relay_human(
            &mut lease,
            MirrorInput::Key {
                key: "a".into(),
                mods: vec![],
            },
        )
        .unwrap();
        wait_lines(&dir.join("stdin"), 1);
        // 記録の file を dir にすり替える → 書けない
        std::fs::remove_file(&log_path).unwrap();
        std::fs::create_dir(&log_path).unwrap();
        let e = m
            .relay_human(
                &mut lease,
                MirrorInput::Click {
                    x: 0.5,
                    y: 0.5,
                    count: 1,
                },
            )
            .unwrap_err();
        assert!(matches!(e, MirrorError::Log(_)), "{e:?}");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(lines(&dir.join("stdin")).len(), 1, "記録の無い入力を送った");
    }

    /// holdout: 自動の口は relay の記録に書かない・helper の口は origin で拒む（本物の helper を swiftc で作る）
    #[test]
    fn w16m0_automated_input_never_writes_a_relay_record() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let log_path = dir.join("relay.jsonl");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&helper, te.clone(), g, RelayLog::open(&log_path).unwrap())
            .unwrap();
        let auto = AutomatedInput::new(helper.clone());
        assert_eq!(
            auto.send(&te, &MirrorInput::Text { text: "x".into() }),
            Ok(5.0)
        );
        assert_eq!(
            RelayLog::read(&log_path).unwrap(),
            vec![],
            "自動の入力が relay の記録に入った"
        );
        m.relay_human(
            &mut ControlLease::human(0.0),
            MirrorInput::Text { text: "y".into() },
        )
        .unwrap();
        assert_eq!(RelayLog::read(&log_path).unwrap().len(), 1);

        // 本物の helper: input の口は origin: human を、窓も許可も見る前に拒む。automated は口を通って窓の検査に着く
        let real = real_helper();
        let gone = MirrorTarget {
            window_id: 1,
            pid: 1,
            bundle_id: "x".into(),
            app_name: "x".into(),
        };
        let human =
            json!({"op": "text", "text": "x", "origin": "human", "relay_seq": 1}).to_string();
        let r: Value = serde_json::from_slice(
            &Command::new(&real)
                .args(["input", "--window", "1", "--pid", "1", &human])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert_eq!(r["reason"], "human_origin_on_automated_port");
        assert_eq!(
            AutomatedInput::new(real).send(&gone, &MirrorInput::Text { text: "x".into() }),
            Err(MirrorError::WindowGone)
        );
    }

    /// holdout（W16M-2）: 届く道の無い mouse（scroll・drag・2 回の click）と menu の key（⌘Z）は送らず、relay の記録も書かず NeedsFront の 1 行（偽の成功にしない）
    #[test]
    fn w16m2_mouse_without_a_public_api_path_is_refused_with_needs_front_and_no_relay_record() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let log_path = dir.join("relay.jsonl");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&helper, te, g, RelayLog::open(&log_path).unwrap()).unwrap();
        let mut lease = ControlLease::human(0.0);
        for (input, kind) in [
            (MirrorInput::Scroll { x: 0.5, y: 0.5, dx: 0, dy: -120 }, InputKind::Scroll),
            (MirrorInput::Drag { x: 0.1, y: 0.1, x2: 0.5, y2: 0.1, steps: None }, InputKind::Drag),
            (MirrorInput::Click { x: 0.5, y: 0.5, count: 2 }, InputKind::Click),
            (MirrorInput::Key { key: "z".into(), mods: vec![KeyMod::Cmd] }, InputKind::Key),
        ] {
            let e = m.relay_human(&mut lease, input).unwrap_err();
            assert_eq!(e, MirrorError::NeedsFront { app_name: "TextEdit".into(), kind });
            assert!(e.to_string().contains("doesn't reach TextEdit"), "{e}");
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(RelayLog::read(&log_path).unwrap(), vec![], "送らない入力の relay の記録を書いた");
        assert_eq!(lines(&dir.join("stdin")).len(), 0, "届く道の無い mouse を helper に送った");
    }

    /// holdout（W16M-2）: 送る入力は記録に道を残し、helper の行にも同じ道を付ける（⌘S → Apple Event の保存・1 回の click → AX の点・文字 → postToPid）
    #[test]
    fn w16m2_relay_line_carries_the_route_and_the_record_keeps_it() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let log_path = dir.join("relay.jsonl");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&helper, te, g, RelayLog::open(&log_path).unwrap()).unwrap();
        let mut lease = ControlLease::human(0.0);
        let sent = [
            (MirrorInput::Key { key: "s".into(), mods: vec![KeyMod::Cmd] }, InputRoute::SaveDocument),
            (MirrorInput::Click { x: 0.5, y: 0.5, count: 1 }, InputRoute::AxAtPoint),
            (MirrorInput::Text { text: "w16m2".into() }, InputRoute::PostToPid),
        ];
        for (input, route) in sent.clone() {
            assert_eq!(m.relay_human(&mut lease, input).unwrap().route, route);
        }
        let got = wait_lines(&dir.join("stdin"), 3);
        for (line, (_, route)) in got.iter().zip(&sent) {
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["route"], serde_json::to_value(route).unwrap(), "{line}");
            assert_eq!(v["origin"], json!("human"));
        }
        let recs = RelayLog::read(&log_path).unwrap();
        assert_eq!(recs.iter().map(|r| r.route).collect::<Vec<_>>(), sent.iter().map(|(_, r)| *r).collect::<Vec<_>>());
        assert!(recs.iter().all(|r| r.v == RELAY_SCHEMA_V));
    }

    /// MIRROR-HYBRID-1101: 触った最初の 1 押し（click・drag・scroll）は `route: "front"` で helper へ（記録も Front）・文字と key は送らない（本物が前なら人の key は直に行く）・
    /// helper の `not_front`（本物が前に来なかった）は NeedsFront の 1 行
    #[test]
    fn mirror_hybrid_front_route_resends_pointer_inputs_and_refuses_keys() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let log_path = dir.join("relay.jsonl");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let mut m = MirrorSurface::open(&helper, te, g, RelayLog::open(&log_path).unwrap()).unwrap();
        let mut lease = ControlLease::human(0.0);
        let pointer = [
            MirrorInput::Click { x: 0.5, y: 0.5, count: 1 },
            MirrorInput::Drag { x: 0.1, y: 0.1, x2: 0.5, y2: 0.1, steps: None },
            MirrorInput::Scroll { x: 0.5, y: 0.5, dx: 0, dy: -120 },
        ];
        for input in pointer {
            assert_eq!(m.relay_front(&mut lease, input).unwrap().route, InputRoute::Front);
        }
        for (input, kind) in [(MirrorInput::Text { text: "a".into() }, InputKind::Text), (MirrorInput::Key { key: "z".into(), mods: vec![KeyMod::Cmd] }, InputKind::Key)] {
            assert_eq!(m.relay_front(&mut lease, input).unwrap_err(), MirrorError::NeedsFront { app_name: "TextEdit".into(), kind });
        }
        let got = wait_lines(&dir.join("stdin"), 3);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(lines(&dir.join("stdin")).len(), 3, "文字と key を front で送った");
        for line in &got {
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!((v["route"].as_str(), v["origin"].as_str()), (Some("front"), Some("human")), "{line}");
        }
        let recs = RelayLog::read(&log_path).unwrap();
        assert!(recs.len() == 3 && recs.iter().all(|r| r.route == InputRoute::Front), "{recs:?}");
        let e = m.refusal(&json!({"kind": "error", "id": 1, "op": "click", "reason": "not_front"}));
        assert_eq!(e, Some(MirrorError::NeedsFront { app_name: "TextEdit".into(), kind: InputKind::Click }));
    }

    /// holdout（W16M-2）: helper の拒否の答えは成功に読まず 1 行の Err に（sent・stats・id の無い行は None）
    #[test]
    fn w16m2_helper_refusals_become_one_line_errors() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let log = RelayLog::open(&dir.join("relay.jsonl")).unwrap();
        let m = MirrorSurface::open(&helper, te, g, log).unwrap();
        let app = || "TextEdit".to_string();
        assert_eq!(
            m.refusal(&json!({"kind": "error", "id": 3, "op": "click", "route": "ax_at_point", "reason": "needs_front"})),
            Some(MirrorError::NeedsFront { app_name: app(), kind: InputKind::Click })
        );
        assert_eq!(
            m.refusal(&json!({"kind": "error", "id": 4, "op": "key", "route": "save_document", "reason": "automation_denied"})),
            Some(MirrorError::AutomationDenied { app_name: app() })
        );
        assert_eq!(m.refusal(&json!({"kind": "error", "id": 5, "op": "click", "reason": "not_trusted"})), Some(MirrorError::AccessibilityDenied));
        assert_eq!(m.refusal(&json!({"kind": "sent", "id": 6, "op": "key", "route": "save_document", "sent_ms": 1.0})), None);
        assert_eq!(m.refusal(&json!({"kind": "stats", "frames": 3})), None);
        // 窓が消えた は stream の WindowGone が言う（二重に出さない）
        assert_eq!(m.refusal(&json!({"kind": "error", "id": 7, "op": "text", "reason": "window_gone"})), None);
        // 読めない行（id の無い bad_json）は入力の拒否ではない・知らない理由は Helper の 1 行（成功にしない）
        assert_eq!(m.refusal(&json!({"kind": "error", "reason": "bad_json"})), None);
        assert_eq!(m.refusal(&json!({"kind": "error", "id": 8, "op": "key", "reason": "bad_route"})), Some(MirrorError::Helper("bad_route".into())));
    }

    /// 反証（W16M-2）: AX の点の道で元の app が押す・選択に答えなかった（混んだ機械の -25204 など）時、helper は動いている。
    /// 「helper が動きません」と内部の語（reason の code）を出さず、何が起きたかと次の一手（前に出して確かめる）の 1 行にする（D-e）。
    #[test]
    fn w16m2_ax_failures_at_the_point_say_the_app_did_not_answer() {
        let (helper, dir) = fake_helper(true);
        let te = target("com.apple.TextEdit");
        let (g, _) = ConsentGate::default().grant(&te, 1.0);
        let m = MirrorSurface::open(&helper, te, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        for reason in ["ax_press_failed_-25204", "ax_select_failed_-25200"] {
            let e = m
                .refusal(&json!({"kind": "error", "id": 9, "op": "click", "route": "ax_at_point", "reason": reason}))
                .unwrap_or_else(|| panic!("{reason} を拒否に読まない"));
            assert!(!matches!(e, MirrorError::Helper(_)), "{reason} → {e:?}（helper は答えている）");
            let line = e.to_string();
            assert!(line.contains("Check it in TextEdit") || line.contains("doesn't reach TextEdit"), "{reason} → {line}");
            assert!(!line.contains("helper") && !line.contains("ax_") && !line.contains("-252"), "{reason} → 内部の語: {line}");
        }
    }

    /// G32 の偽の helper: 呼ばれた subcommand を calls に足し、stream は pid を書いて起こした時の frame（{d}/f1.bin）を流し、
    /// stdin の行を {d}/stdin に溜めながら `stdin_sh`（`$l` = 行）を走らせる。dir に置く file はこれを呼んだ後に書く。
    fn g32_helper(stdin_sh: &str) -> (PathBuf, PathBuf) {
        let (sh, dir) = fake_stream_helper("");
        let d = dir.display();
        std::fs::write(
            &sh,
            format!("#!/bin/sh\necho \"$1\" >> {d}/calls\ncase \"$1\" in\n status) echo '{{\"screen_capture\":true}}';;\n list) echo '[]';;\n stream) echo $$ > {d}/pid; cat {d}/f1.bin; while read -r l; do echo \"$l\" >> {d}/stdin; {}; done;;\nesac\n", stdin_sh.replace("{d}", &d.to_string())),
        )
        .unwrap();
        (sh, dir)
    }

    fn g32_open(sh: &Path, dir: &Path, window_id: u64) -> MirrorSurface {
        let t = MirrorTarget { window_id, pid: 42, bundle_id: "com.apple.TextEdit".into(), app_name: "TextEdit".into() };
        let (g, _) = ConsentGate::default().grant(&t, 1.0);
        MirrorSurface::open(sh, t, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap()
    }

    /// 最初に着く frame の seq（2 秒まで待つ）
    fn g32_first_seq(m: &mut MirrorSurface) -> Option<u64> {
        for _ in 0..200 {
            if let Some(f) = m.stream().unwrap() {
                return Some(f.meta.seq);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    fn g32_frame(seq: u64, stream: Option<u64>) -> Vec<u8> {
        let mut h = header(1, 1);
        h["seq"] = json!(seq);
        if let Some(s) = stream {
            h["stream"] = json!(s);
        }
        msg(1, frame_payload(h, 4))
    }

    /// holdout（G32）: 同じ helper の 2 面は process 1 本を分け合う（2 面目は stdin の open・窓探しも stdin の list＝別の process の
    /// SCShareableContent で生きている stream を落とさない）。frame は header の stream の面へ（欄の無い frame は 1 面目）・入力は面の stream へ・
    /// 1 面目を閉じても 2 面目の process は止まらず（close は stream 1 だけ）、最後の面で process を止める。
    #[test]
    fn g32_two_surfaces_share_one_helper_process() {
        let (sh, dir) = g32_helper(r#"case "$l" in *'"cmd":"open"'*) cat {d}/f2.bin;; *'"cmd":"list"'*) cat {d}/win.bin;; esac"#);
        std::fs::write(dir.join("f1.bin"), g32_frame(1, None)).unwrap();
        let mut f2 = msg(2, serde_json::to_vec(&json!({"kind": "started", "stream": 2})).unwrap());
        f2.extend(g32_frame(5, Some(2)));
        std::fs::write(dir.join("f2.bin"), f2).unwrap();
        // list の id は open の後の次の番号（stream 1 は起こした時・2 は open・3 が list）
        let win = json!({"kind": "windows", "id": 3, "windows": [{"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 50, "title": "notes.md", "window_id": 8}]});
        std::fs::write(dir.join("win.bin"), msg(2, serde_json::to_vec(&win).unwrap())).unwrap();
        let calls = || lines(&dir.join("calls"));
        let mut a = g32_open(&sh, &dir, 7);
        // 1 面目の frame が着いた＝stream の process が calls を書いた後に 2 面目を開く（calls の順を決める）
        assert_eq!(g32_first_seq(&mut a), Some(1));
        let mut b = g32_open(&sh, &dir, 8);
        assert_eq!(calls(), ["status", "stream", "status"], "2 面目で helper の process をもう 1 本起こした");
        let open: Value = serde_json::from_str(&wait_lines(&dir.join("stdin"), 1)[0]).unwrap();
        assert_eq!((open["cmd"].as_str(), open["stream"].as_u64(), open["window"].as_u64()), (Some("open"), Some(2), Some(8)));
        assert_eq!(g32_first_seq(&mut b), Some(5), "2 面目の frame が 2 面目に来ない");
        assert_eq!(a.stream().unwrap(), None, "2 面目の frame が 1 面目に入った");
        let t = MirrorTarget::find(&sh, "com.apple.TextEdit", Some("notes.md"), None, false).unwrap();
        assert_eq!(t.window_id, 8);
        assert!(!calls().contains(&"list".to_string()), "面が開いているのに list の process を起こした: {:?}", calls());
        b.relay_human(&mut ControlLease::human(0.0), MirrorInput::Text { text: "b".into() }).unwrap();
        drop(a);
        b.relay_human(&mut ControlLease::human(0.0), MirrorInput::Text { text: "b2".into() }).unwrap();
        let got: Vec<Value> = wait_lines(&dir.join("stdin"), 5).iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!((got[1]["cmd"].as_str(), got[2]["stream"].as_u64(), got[2]["text"].as_str()), (Some("list"), Some(2), Some("b")));
        assert_eq!((got[3]["cmd"].as_str(), got[3]["stream"].as_u64()), (Some("close"), Some(1)));
        assert_eq!((got.len(), got[4]["text"].as_str()), (5, Some("b2")), "1 面目を閉じたら 2 面目の process も止めた");
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        drop(b);
        assert!(!Command::new("kill").args(["-0", pid.trim()]).status().unwrap().success(), "最後の面を閉じても helper の process が残った");
    }

    /// holdout（G32）: helper の process が死んだら、その中の全部の面に Err が届き（読み手の thread の EOF）、次の open は新しい process を起こす
    /// （死んだ process に頼まない）。偽の helper は 2 面目の open を受けたら終わる。
    #[test]
    fn g32_helper_death_reaches_every_surface_and_the_next_open_starts_anew() {
        let (sh, dir) = g32_helper("exit 0");
        std::fs::write(dir.join("f1.bin"), g32_frame(1, None)).unwrap();
        let (mut a, mut b) = (g32_open(&sh, &dir, 7), g32_open(&sh, &dir, 8));
        for (name, m) in [("1 面目", &mut a), ("2 面目", &mut b)] {
            let mut got = Ok(None);
            for _ in 0..300 {
                got = m.stream();
                if got.is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(matches!(got, Err(MirrorError::Helper(_))), "{name}: helper が死んだのに {got:?}");
        }
        // 新しい process の frame が着いた後に数える（calls は process が起きてから書く）
        let mut c = g32_open(&sh, &dir, 9);
        assert_eq!(g32_first_seq(&mut c), Some(1), "3 面目に frame が来ない");
        assert_eq!(lines(&dir.join("calls")).iter().filter(|c| *c == "stream").count(), 2, "死んだ process に open を送った");
    }

    /// MIRROR-QUIET-1101（owner 208）: 窓を探す・書類を問う・許可を見る・映すが、開いている面が無い間も 1 本の常駐（`serve`）の stdin で済む
    /// （process を起こすのは 1 回）。最後の面を閉じても常駐は残り、次の窓探しも同じ process
    #[test]
    fn mirror_quiet_one_resident_process_finds_checks_and_streams() {
        let (sh, dir) = fake_stream_helper("");
        let d = dir.display().to_string();
        std::fs::write(
            &sh,
            format!(
                concat!(
                    "#!/bin/sh\necho \"$1\" >> {d}/calls\ncase \"$1\" in\n",
                    " serve) echo $$ > {d}/pid; cat {d}/ready.bin; while read -r l; do echo \"$l\" >> {d}/stdin;\n",
                    "   id=$(echo \"$l\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p'); [ -n \"$id\" ] && [ -f {d}/ans-$id.bin ] && cat {d}/ans-$id.bin;\n",
                    "   case \"$l\" in *'\"cmd\":\"open\"'*) cat {d}/open.bin;; esac; done;;\nesac\n"
                ),
                d = d
            ),
        )
        .unwrap();
        let notes = dir.join("notes.md");
        std::fs::write(&notes, "x").unwrap();
        let ev = |v: Value| msg(2, serde_json::to_vec(&v).unwrap());
        std::fs::write(dir.join("ready.bin"), ev(json!({"kind": "ready", "caps": ["list", "status", "doc-path", "open"]}))).unwrap();
        // 常駐の id は 2 から（stream 1 は無い）: list 2 → doc-path 3 → status 4 → open の stream 5 → 閉じた後の list 6
        let win = json!([{"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "pid": 50, "title": "notes.md", "window_id": 8}]);
        std::fs::write(dir.join("ans-2.bin"), ev(json!({"kind": "windows", "id": 2, "windows": win}))).unwrap();
        std::fs::write(dir.join("ans-3.bin"), ev(json!({"kind": "answer", "id": 3, "path": notes.display().to_string(), "reason": "ok"}))).unwrap();
        std::fs::write(dir.join("ans-4.bin"), ev(json!({"kind": "answer", "id": 4, "screen_capture": true, "accessibility": true, "post_event": true}))).unwrap();
        std::fs::write(dir.join("ans-6.bin"), ev(json!({"kind": "windows", "id": 6, "windows": []}))).unwrap();
        let mut open = ev(json!({"kind": "started", "stream": 5}));
        open.extend(g32_frame(5, Some(5)));
        std::fs::write(dir.join("open.bin"), open).unwrap();
        let t = MirrorTarget::find(&sh, "com.apple.TextEdit", Some("notes.md"), Some(&notes), false).unwrap();
        assert_eq!(t.window_id, 8);
        let (g, _) = ConsentGate::default().grant(&t, 1.0);
        let mut m = MirrorSurface::open(&sh, t, g, RelayLog::open(&dir.join("relay.jsonl")).unwrap()).unwrap();
        assert_eq!(g32_first_seq(&mut m), Some(5), "常駐の中の stream の frame が来ない");
        let sent: Vec<Value> = wait_lines(&dir.join("stdin"), 4).iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(sent.iter().map(|v| v["cmd"].as_str().unwrap_or("")).collect::<Vec<_>>(), ["list", "doc-path", "status", "open"]);
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        drop(m);
        assert!(Command::new("kill").args(["-0", pid.trim()]).status().unwrap().success(), "最後の面を閉じたら常駐も止めた（次の窓探しで process を起こし直す）");
        let e = MirrorTarget::find(&sh, "com.apple.TextEdit", None, None, false).unwrap_err();
        assert!(matches!(e, MirrorError::NoWindow { .. }), "{e:?}");
        assert_eq!(lines(&dir.join("calls")), ["serve"], "窓探し・書類・許可・映すのどれかで helper の process をもう 1 本起こした");
    }

    /// MIRROR-FRAME-1101: 小さな絵は常駐への 1 回の `shot` で・撮れた窓だけ・一時 dir は読んだら消す。ready で `shot` を言わない helper には頼まない（空 = 灰の地）。
    /// 負の対照: caps の `shot` を見ずに頼む → 2 周目の stdin に shot の行が出て赤・dir を消さない → `exists` の assert が赤
    #[test]
    fn mirror_frame_1101_shots_come_in_one_ask_and_leave_nothing_behind() {
        for shot in [true, false] {
            let (sh, dir) = fake_stream_helper("");
            let d = dir.display().to_string();
            // 常駐の偽物: shot の行の "dir" に 70 の絵だけ書いてから答える（71 は撮れない窓）
            std::fs::write(
                &sh,
                format!(
                    concat!(
                        "#!/bin/sh\ncase \"$1\" in\n serve) cat {d}/ready.bin; while read -r l; do echo \"$l\" >> {d}/stdin;\n",
                        "   id=$(echo \"$l\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p'); out=$(echo \"$l\" | sed -n 's/.*\"dir\":\"\\([^\"]*\\)\".*/\\1/p');\n",
                        "   [ -n \"$out\" ] && printf PNG70 > \"$out/70.png\"; [ -n \"$id\" ] && [ -f {d}/ans-$id.bin ] && cat {d}/ans-$id.bin; done;;\nesac\n"
                    ),
                    d = d
                ),
            )
            .unwrap();
            let ev = |v: Value| msg(2, serde_json::to_vec(&v).unwrap());
            let caps = if shot { json!(["list", "status", "doc-path", "open", "shot"]) } else { json!(["list", "status", "doc-path", "open"]) };
            std::fs::write(dir.join("ready.bin"), ev(json!({"kind": "ready", "caps": caps}))).unwrap();
            let win = |id: u64| json!({"app": "Google Chrome", "bundle_id": "com.google.Chrome", "pid": 7, "title": "Docs", "window_id": id, "on_screen": true, "frame": {"x": 0, "y": 0, "w": 500, "h": 400}});
            // 常駐の id は 2 から: windows の list 2 → shot 3
            std::fs::write(dir.join("ans-2.bin"), ev(json!({"kind": "windows", "id": 2, "windows": [win(70), win(71)]}))).unwrap();
            std::fs::write(dir.join("ans-3.bin"), ev(json!({"kind": "answer", "id": 3, "shots": [70]}))).unwrap();
            assert_eq!(MirrorTarget::windows(&sh).expect("一覧").len(), 2);
            let got = MirrorTarget::shots(&sh, &[70, 71], 112);
            let sent: Vec<Value> = lines(&dir.join("stdin")).iter().map(|l| serde_json::from_str(l).unwrap()).collect();
            if !shot {
                assert!(got.is_empty() && sent.iter().all(|v| v["cmd"] != "shot"), "shot を言わない helper に頼んだ: {sent:?}");
                continue;
            }
            assert_eq!(got, vec![(70, b"PNG70".to_vec())], "撮れた窓の絵だけを返さない");
            let q = sent.iter().find(|v| v["cmd"] == "shot").expect("shot を頼まない");
            assert_eq!((q["windows"].clone(), q["width"].clone()), (json!([70, 71]), json!(112)));
            assert!(!Path::new(q["dir"].as_str().unwrap()).exists(), "画面の絵の一時 dir を残した");
        }
    }

    /// MIRROR-QUIET-1101: `serve` を知らない helper（前の版・ready を言わずに終わる）には `serve` を 1 回だけ試し、以後は今まで通り 1 回きりの `list`
    #[test]
    fn mirror_quiet_helper_without_serve_falls_back_to_one_shot_list() {
        let (sh, dir) = g32_helper(":");
        for _ in 0..2 {
            let e = MirrorTarget::find(&sh, "com.apple.TextEdit", None, None, false).unwrap_err();
            assert!(matches!(e, MirrorError::NoWindow { .. }), "{e:?}");
        }
        assert_eq!(lines(&dir.join("calls")), ["serve", "list", "list"], "serve を知らない helper に何度も serve を起こした");
        assert_eq!(helper_down(&sh), None, "serve を知らない helper を「落ち続けている」と言った");
    }

    /// MIRROR-QUIET-1101: 常駐が続けて落ちた時の起こし直しの間は 1→2→4…最大 30 秒
    #[test]
    fn mirror_quiet_resident_restarts_wait_1_2_4_up_to_30_s() {
        let s = |n| backoff(n).as_secs();
        assert_eq!([s(1), s(2), s(3), s(4), s(5), s(6), s(40)], [1, 2, 4, 8, 16, 30, 30]);
    }

    /// holdout（G32）: 開いている process の list の答えの error は、許可が無い = ScreenCaptureDenied（「窓を開いて」の NoWindow にしない）・他は Helper
    #[test]
    fn g32_list_errors_through_the_open_process() {
        let (sh, dir) = g32_helper(r#"case "$l" in *'"cmd":"list"'*) cat {d}/deny.bin;; esac"#);
        std::fs::write(dir.join("f1.bin"), g32_frame(1, None)).unwrap();
        let deny = |id: u64, e: &str| msg(2, serde_json::to_vec(&json!({"kind": "windows", "id": id, "error": e})).unwrap());
        let mut both = deny(2, "no_screen_capture_permission");
        both.extend(deny(3, "capture_timeout"));
        std::fs::write(dir.join("deny.bin"), both).unwrap();
        let _a = g32_open(&sh, &dir, 7);
        assert_eq!(MirrorTarget::find(&sh, "com.apple.TextEdit", None, None, false), Err(MirrorError::ScreenCaptureDenied));
        assert_eq!(MirrorTarget::find(&sh, "com.apple.TextEdit", None, None, false), Err(MirrorError::Helper("capture_timeout".into())));
    }

    /// `KANGAE_MIRROR_HELPER` か、KangaeMirror.swift を swiftc -Onone で target の下に作った物（source より新しければ作り直さない）。
    /// swiftc が無ければ赤（skip しない）。
    fn real_helper() -> PathBuf {
        if let Some(p) = std::env::var_os("KANGAE_MIRROR_HELPER") {
            return PathBuf::from(p);
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = root.join("bridge/macos-mirror/KangaeMirror.swift");
        let out = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join("w16m0-kangae-mirror-helper");
        let fresh = |o: &Path| {
            o.metadata().and_then(|m| m.modified()).ok()
                > src.metadata().and_then(|m| m.modified()).ok()
        };
        if !fresh(&out) {
            // 同じ file を #[path] で取り込む別の module の試験と同時に走るので、別名に作ってから置き換える
            let tmp = out.with_extension(module_path!().replace("::", "-"));
            let o = Command::new("swiftc")
                .args(["-Onone", "-parse-as-library"])
                .arg(&src)
                .arg("-o")
                .arg(&tmp)
                .output()
                .expect("swiftc が要ります");
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            std::fs::rename(&tmp, &out).unwrap();
        }
        out
    }

    /// G90 (3)（owner の qa 10: Music の `Mirror here` の後に `The app's window closed`・窓は在って隠れていた）: helper の `stopped` の `window_hidden` は
    /// `WindowHidden`（その面の app の名前）で、閉じたと言わない。負の対照: `Sink::event` の window_hidden の腕を外す → Helper の 1 行で赤
    #[test]
    fn g90c_a_hidden_window_is_not_a_closed_window() {
        let video = Arc::new(Mutex::new(Video::default()));
        let (tx, _rx) = mpsc::sync_channel(EVENT_CAP);
        read_helper(std::io::Cursor::new(msg(2, serde_json::to_vec(&json!({"kind": "stopped", "reason": "window_hidden"})).unwrap())), one_route(Arc::clone(&video), tx));
        let e = video.lock().unwrap().fatal.clone();
        assert_eq!(e, Some(MirrorError::WindowHidden { app_name: "Music".into() }), "隠れた窓を閉じた窓か helper の止まりに読んだ");
        assert_eq!(e.unwrap().to_string(), "Music's window is hidden");
    }
}
