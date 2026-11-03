//! W17-3（新旧の比べ）の契約: 旧 canvas の Desktop の筋書きを、旧い道（cua-driver・`cua::Driver`）と新しい道（bridge・`MacGui`）の両方で回し、
//! 表の各欄を**試験の側の oracle だけ**で埋めて比べる。製品が返した値（`Outcome`・`GuiResult`・`Elements`）は手を進める事にだけ使い、表に書かない。
//! 表（`TABLE_PATH`・`Table`）が W18 の回帰の正本: W18-1 で旧い道（製品の `src/cua.rs`）を消したので `run(_, _, Way::Old)` は手を打たず未測（`OLD_WAY_REMOVED`）。
//! W18-2 は `Way::New` だけで回して保存した表の New の行と比べる（`Table::against`）。
//! 製品の code（`src/`）には置かない（oracle は試験の側の cua-driver・製品に cua-driver を足さない）。tests/adapters.rs が `#[path]` で取り込み、`super::{adapters, model, crypto, readback}` を引く。
//! 図は docs/diagrams.md §28 の末尾。
#![allow(dead_code)]

use super::adapters::mac_gui::MacGui;
use super::crypto::sha256_hex;
use super::model::{ActionKind, DesktopTool};
use super::readback::WindowFact;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// 保存する表の場所（repo の root から）と形の版。形を変えたら版を上げ、W18-2 は版の違う表を読まない。
pub const TABLE_PATH: &str = "docs/evidence/W17-3/table.json";
pub const TABLE_VERSION: u32 = 1;

/// W18-1 の後に `run(_, _, Way::Old)` が返す未測の理由（YC W18-1 interface）。旧い道は製品から消したので、Old の読みは保存した表の行だけ。
pub const OLD_WAY_REMOVED: &str = "旧い道（src/cua.rs の cua::Driver）は W18-1 で製品から消しました。Old の読みは保存した表（docs/evidence/W17-3/table.json）の行だけです";

/// 筋書き（YC_WAVES W17-3）。
/// 回す順は `ORDER`（窓 → 計算機 → TextEdit）。TextEdit は列 N と共有で前面を取るので、owner が先に開いていなければ `Status::Unmeasured`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    /// 旧パネルの ① 窓の検出 → ② 要素を読んで登録（`ActionKind::Desktop` を作る）→ ③ 実行（計算機の key を 1 つ押す）。
    /// oracle = CGWindowList の pid と窓 id・表示の AXValue
    Windows,
    /// 計算機で `Fixture::calculator_keys` を順に押す。oracle = 表示の AXValue
    Calculator,
    /// TextEdit の本文に `Fixture::text` を入れて保存。oracle = disk の file の sha256 と本文の AXValue
    TextEdit,
}
pub const ORDER: [Scenario; 3] = [Scenario::Windows, Scenario::Calculator, Scenario::TextEdit];

/// 道。`AdapterKind` は使わない（W17-2 で `"cua"` が `MacGui` の alias になり、2 つの道を分けられない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Way {
    /// 旧い道（W17-3 の時の製品の `src/cua.rs`）。W18-1 で走らせる道は消した。variant は保存した表（`TABLE_PATH`）を読むために残す。
    /// `run(rig, s, Way::Old)` は手を打たず `Status::Unmeasured { reason: OLD_WAY_REMOVED }`。
    Old,
    /// `adapters::mac_gui::MacGui` の `windows` / `elements` / `run_desktop`
    New,
}

/// oracle の読み 1 回分。値はすべて試験の側が相手の app（計算機・TextEdit）と disk から読んだ物で、Shibubu の応答は入らない。
/// 例: `Reading::Calculator { display: "3".into() }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scenario", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reading {
    /// windows = CGWindowList の [pid, 窓 id]（`cg_windows` の結果・並びは窓 id の昇順）
    Windows {
        windows: Vec<[u32; 2]>,
        display: String,
    },
    Calculator {
        display: String,
    },
    TextEdit {
        sha256: String,
        ax_value: String,
    },
}

/// 1 つの筋書きを 1 つの道で回した結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Status {
    /// before = 手の前・after = 手の後（どちらも oracle）
    Measured { before: Reading, after: Reading },
    /// 測っていない（TextEdit を owner が開いていない・AX の許可が無い・oracle が読めない）。理由 1 行。緑に数えない
    Unmeasured { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub scenario: Scenario,
    pub way: Way,
    pub status: Status,
}

/// 両方の道で同じ手にする入力。道ごとに変えない（同じ入力なら oracle の after が同じになる筈・それを比べる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    /// 計算機で押す AXButton の label（例 `["1", "+", "2", "="]`）。手の前に試験の側が表示を消す（"C" を押す）
    pub calculator_keys: Vec<String>,
    /// TextEdit に入れる文字。手の前に試験の側が書類を空にして保存する（道ごとに同じ前の状態から）
    pub text: String,
    /// 先に TextEdit で開いた空の file（temp の下の `w17-3-*.txt` だけ・`textedit_file_ok`）。None = TextEdit の行は Unmeasured（試験は TextEdit を起こさない）
    #[serde(default)]
    pub textedit_file: Option<PathBuf>,
}

/// 新しい道の TextEdit の保存（`MacGui::menu`・bridge の `ax_menu`）。題名は完全一致: 書類に file が在る時の TextEdit（en）の File の下は「Save」（ellipsis 無し・W17-2 戻しの偵察 03:30 の `menu_not_found` の一覧）。
pub const SAVE_MENU: [&str; 2] = ["File", "Save"];

/// 保存する表（`TABLE_PATH` に serde_json の pretty で 1 つ）。rows は `ORDER` × [Old, New] の 6 行（W18 の後は New の 3 行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    pub version: u32,
    /// 測った時刻（ISO 8601・表示だけ）
    pub measured_at: String,
    /// 測った HEAD の commit（表示だけ）
    pub head: String,
    pub fixture: Fixture,
    pub rows: Vec<Row>,
}

/// 2 つの行の比べ（Old と New・または保存した New と今の New）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agreement {
    /// どちらも測れて、after が同じで、どちらも before ≠ after（手が効いた）
    Same,
    /// どちらも測れたが after が違う
    Differs { a: Reading, b: Reading },
    /// 測れたが before = after（押しても何も起きない = L2）。どちらの道か
    NoEffect { way: Way },
    /// どちらかが測れていない（理由はその行の物）
    Unmeasured { reason: String },
}

impl Agreement {
    /// 緑に数えてよいのは `Same` だけ（未測・効かなかった手・食い違いは緑にしない）。
    /// 使い方: `assert!(compare(&old, &new).is_green())`。保証しない: 製品の表示が正しい事（それは L1 の検査）。
    pub fn is_green(&self) -> bool {
        matches!(self, Agreement::Same)
    }
}

/// 2 つの行を比べる。scenario の違う行を渡すのは呼ぶ側の誤り（panic）。
/// 判定の順: どちらかが Unmeasured → `Unmeasured` / どちらかの before = after → `NoEffect` / after が違う → `Differs` / 他は `Same`。
/// 保証する: 同じ 2 行なら同じ答え（時刻・環境を読まない）。保証しない: 行が本当に oracle で埋まっている事（`run` の約束）。
pub fn compare(a: &Row, b: &Row) -> Agreement {
    assert_eq!(a.scenario, b.scenario, "筋書きの違う行は比べない");
    let (a_before, a_after, b_before, b_after) = match (&a.status, &b.status) {
        (Status::Unmeasured { reason }, _) | (_, Status::Unmeasured { reason }) => {
            return Agreement::Unmeasured {
                reason: reason.clone(),
            }
        }
        (
            Status::Measured {
                before: ab,
                after: aa,
            },
            Status::Measured {
                before: bb,
                after: ba,
            },
        ) => (ab, aa, bb, ba),
    };
    if a_before == a_after {
        return Agreement::NoEffect { way: a.way };
    }
    if b_before == b_after {
        return Agreement::NoEffect { way: b.way };
    }
    if a_after != b_after {
        return Agreement::Differs {
            a: a_after.clone(),
            b: b_after.clone(),
        };
    }
    Agreement::Same
}

impl Table {
    /// 保存した表を読む（YC W18-1 interface・W18-2 の回帰の入口）。版が `TABLE_VERSION` と違う・知らない欄がある・読めない時は Err（理由 1 行）。
    /// 使い方: `let saved = Table::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE_PATH))?;`
    /// 保証しない: 表の行が oracle で埋まっている事（書いた時の `run` の約束）。
    pub fn load(path: &Path) -> Result<Table, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("表を読めません（{}）: {e}", path.display()))?;
        let t: Table = serde_json::from_str(&text).map_err(|e| format!("表の形が違います（{}）: {e}", path.display()))?;
        if t.version != TABLE_VERSION {
            return Err(format!("表の版が {}（読めるのは {TABLE_VERSION}）: {}", t.version, path.display()));
        }
        Ok(t)
    }

    /// W18-2 の回帰（YC W18-1 interface・本体は W18-2）: 保存した表の New の行と、今 `Way::New` だけで回した行（`now`）を `ORDER` の筋書きごとに `compare` する。
    /// 保存した表に Old と New の両方が在り `verdicts` が Same の筋書きだけを比べ、そうでない筋書き（TextEdit の未測など）は `Unmeasured`（緑にしない）。
    /// W18-2 interface で足した約束: `Windows` の pid と窓 id は run ごとに変わる（計算機を開き直すと別の値）ので、保存した行との比べは
    /// before / after の窓の数と display で行う（同じ run の Old と New を比べる `compare` は今のまま pid と窓 id まで見る）。
    /// 返り値は `ORDER` の順に 3 つ。`now` に無い筋書きは `Unmeasured`。TextEdit が未測のままなら全緑にならない＝W18 を done にしない（8 通目）。
    /// 使い方: `assert!(saved.against(&now).iter().all(|(_, a)| a.is_green()), "{:?}", saved.against(&now))`。
    /// 保証しない: `now` の行が New の道で回した物である事（呼ぶ側が `Way::New` で `run` する）。
    pub fn against(&self, now: &[Row]) -> Vec<(Scenario, Agreement)> {
        // Windows の [pid, 窓 id] は run ごとに変わるので、保存した行との比べでは数だけ残す（display はそのまま比べる）
        let count_only = |r: &Row| -> Row {
            let n = |x: &Reading| match x {
                Reading::Windows { windows, display } => Reading::Windows { windows: vec![[0, 0]; windows.len()], display: display.clone() },
                o => o.clone(),
            };
            match &r.status {
                Status::Measured { before, after } => Row { status: Status::Measured { before: n(before), after: n(after) }, ..r.clone() },
                _ => r.clone(),
            }
        };
        self.verdicts()
            .into_iter()
            .map(|(s, was)| {
                let saved = self.rows.iter().find(|r| r.scenario == s && r.way == Way::New);
                let a = match (was, saved, now.iter().find(|r| r.scenario == s)) {
                    (Agreement::Same, Some(saved), Some(now)) => compare(&count_only(saved), &count_only(now)),
                    (Agreement::Same, _, None) => Agreement::Unmeasured { reason: format!("{s:?} を今回の New で回していない") },
                    (was, _, _) => Agreement::Unmeasured { reason: format!("保存した表の {s:?} が Same でない（{was:?}）ので比べない") },
                };
                (s, a)
            })
            .collect()
    }

    /// `ORDER` の筋書きごとに Old と New の行を `compare` する。片方の行が無い筋書きは `Unmeasured`。
    /// 使い方: `assert!(t.verdicts().iter().all(|(_, a)| a.is_green()), "{:?}", t.verdicts())`
    pub fn verdicts(&self) -> Vec<(Scenario, Agreement)> {
        let row = |s: Scenario, w: Way| self.rows.iter().find(|r| r.scenario == s && r.way == w);
        ORDER
            .iter()
            .map(|&s| {
                let a = match (row(s, Way::Old), row(s, Way::New)) {
                    (Some(old), Some(new)) => compare(old, new),
                    (old, _) => Agreement::Unmeasured {
                        reason: format!(
                            "{:?} の行が表に無い",
                            if old.is_none() { Way::Old } else { Way::New }
                        ),
                    },
                };
                (s, a)
            })
            .collect()
    }
}

// ---- 試験の側の oracle（どれも相手の app か disk を読む。Shibubu には聞かない） ----

/// CGWindowList で pid の画面上の窓（`scripts/qa/bg/oracle/ax.swift` を swiftc で build した binary の `windows` の出力の 2・3 列目）。
/// 保証しない: 窓が AX で読める事（画面収録の許可が無いと題名が空なだけで pid と窓 id は出る）。
pub fn cg_windows(windows_bin: &Path, pid: u32) -> Result<Vec<WindowFact>, String> {
    let out = Command::new(windows_bin).output().map_err(|e| {
        format!(
            "CG の窓の一覧を起こせません（{}）: {e}",
            windows_bin.display()
        )
    })?;
    if !out.status.success() {
        return Err(format!("CG の窓の一覧が失敗しました（{}）", out.status));
    }
    let mut v: Vec<WindowFact> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut c = l.split('\t').skip(1);
            let p: u32 = c.next()?.parse().ok()?;
            let w: u32 = c.next()?.parse().ok()?;
            (p == pid).then_some(WindowFact {
                pid: p,
                window_id: w,
            })
        })
        .collect();
    v.sort_by_key(|w| w.window_id);
    Ok(v)
}

/// 試験の側の cua-driver の session（旧い道の `cua::Driver` とは別の label・読みが旧い道の token を古くしない）
const ORACLE_SESSION: &str = "w173-oracle";

/// 試験の側の cua-driver で窓を 1 回読む（`get_window_state`・timeout_ms 10000・途中までの読みは Err）。
fn cua_state(cua_bin: &Path, pid: u32, window_id: u32) -> Result<Value, String> {
    let params = json!({"pid": pid, "window_id": window_id, "include_screenshot": false, "timeout_ms": 10000, "session": ORACLE_SESSION});
    let v = cua_call(cua_bin, "get_window_state", params)?;
    if v["truncated"] == true || v["degraded"] == true {
        return Err(format!("AX の読みが途中まで（pid {pid}・窓 {window_id}）"));
    }
    Ok(v)
}

fn cua_call(cua_bin: &Path, method: &str, params: Value) -> Result<Value, String> {
    let out = Command::new(cua_bin)
        .args(["call", method, &params.to_string(), "--raw"])
        .output()
        .map_err(|e| format!("試験の側の cua-driver を起こせません: {e}"))?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let v: Value = raw
        .find('{')
        .and_then(|i| serde_json::from_str(&raw[i..]).ok())
        .ok_or_else(|| {
            format!(
                "cua-driver {method} の JSON が読めません（{}）",
                raw.trim().chars().take(200).collect::<String>()
            )
        })?;
    if !out.status.success() || v.get("error").is_some_and(|e| !e.is_null()) {
        return Err(format!(
            "cua-driver {method} が拒みました: {}",
            raw.trim().chars().take(300).collect::<String>()
        ));
    }
    Ok(v)
}

/// 読んだ窓の中で role・label が一致する要素（ちょうど 1 つ）。
fn one<'a>(v: &'a Value, role: &str, label: Option<&str>) -> Result<&'a Value, String> {
    let hits: Vec<&Value> = v["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["role"].as_str() == Some(role))
        .filter(|e| label.is_none_or(|l| e["label"].as_str() == Some(l)))
        .collect();
    match hits.as_slice() {
        [e] => Ok(e),
        h => Err(format!(
            "{role} {label:?} が {} 個（ちょうど 1 つでない）",
            h.len()
        )),
    }
}

/// 試験の側の cua-driver（絶対 path・背景・`get_window_state` の timeout_ms 10000）で、窓の中の `role` の要素の AXValue（ちょうど 1 つ・0 件 / 複数は Err）。
/// 例: `ax_value(&cua, calc_pid, wid, "AXStaticText")`。保証しない: 前面と cursor を見る事（D-g は run.sh の側）。
pub fn ax_value(cua_bin: &Path, pid: u32, window_id: u32, role: &str) -> Result<String, String> {
    let v = cua_state(cua_bin, pid, window_id)?;
    let e = one(&v, role, None)?;
    Ok(match &e["value"] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

/// disk の file の sha256（`crypto::sha256_hex`・小文字の hex）。
pub fn file_sha256(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|b| sha256_hex(&b))
        .map_err(|e| format!("{} を読めません: {e}", path.display()))
}

/// TextEdit の行で空にしてよい file か: temp（`std::env::temp_dir()`・`/private/tmp`）の下に在り、名前が `w17-3-*.txt`。
/// 他の file（owner の本物の書類）は空にしない。
pub fn textedit_file_ok(file: &Path) -> Result<(), String> {
    let real = file
        .canonicalize()
        .map_err(|e| format!("{} が無い: {e}", file.display()))?;
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !(name.starts_with("w17-3-") && name.ends_with(".txt")) {
        return Err(format!(
            "{} は w17-3-*.txt でない・空にしない",
            real.display()
        ));
    }
    let roots = [std::env::temp_dir(), PathBuf::from("/private/tmp")];
    if !roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| real.starts_with(r))
    {
        return Err(format!("{} は temp の下でない・空にしない", real.display()));
    }
    Ok(())
}

/// TextEdit の窓の題名が `file` の物か（file 名か拡張子無しの名前で始まる・空は false）。
pub fn title_matches(title: &str, file: &Path) -> bool {
    let names = [file.file_name(), file.file_stem()].map(|n| n.and_then(|n| n.to_str()));
    names.into_iter().flatten().any(|n| {
        title == n
            || title
                .strip_prefix(n)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

/// 手の前の TextEdit が空か（disk の sha が空の sha・本文の AXValue が ""）。違えば Err（前の状態が揃っていない）。
pub fn require_empty(before: &Reading) -> Result<(), String> {
    match before {
        Reading::TextEdit { sha256, ax_value }
            if *sha256 == sha256_hex(b"") && ax_value.is_empty() =>
        {
            Ok(())
        }
        r => Err(format!("手の前の TextEdit が空に揃っていない: {r:?}")),
    }
}

/// 数える判定（TextEdit の未測を除く）が全部 Same の時だけ `out` に表を書く。赤い run は保存した正本を上書きしない。
pub fn save_if_green(t: &Table, out: &Path) -> Result<(), String> {
    let red: Vec<_> = t
        .verdicts()
        .into_iter()
        .filter(|(s, a)| !(*s == Scenario::TextEdit && matches!(a, Agreement::Unmeasured { .. })))
        .filter(|(_, a)| !a.is_green())
        .collect();
    if !red.is_empty() {
        return Err(format!(
            "赤い判定があるので {} を書かない: {red:?}",
            out.display()
        ));
    }
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(out, serde_json::to_string_pretty(t).unwrap()).map_err(|e| e.to_string())
}

/// TextEdit の窓を題名で 1 つ選ぶ（W17-2 戻し）。ws = (pid, 窓 id, 題名)。`title_matches` が全体でちょうど 1 つの時だけ Ok・0 / 2 以上は Err（推測で選ばない）。
/// TextEdit は列 M・N と同じ process を共有するので「pid 1 つ・窓 1 つ」（`only_window`）では選べない。
/// 例: `pick_window(&[(36303, 5428, "notes.md".into()), (94903, 5565, "w17-3-a.txt".into())], file)` → `Ok((94903, 5565))`。
pub fn pick_window(ws: &[(u32, u32, String)], file: &Path) -> Result<(u32, u32), String> {
    let hits: Vec<(u32, u32)> = ws.iter().filter(|(_, _, t)| title_matches(t, file)).map(|(p, w, _)| (*p, *w)).collect();
    match hits[..] {
        [one] => Ok(one),
        _ => Err(format!("TextEdit の窓で題名が {} の物が {} 個（ちょうど 1 つでない）", file.display(), hits.len())),
    }
}

/// 試験の側: 起動中の TextEdit（`pgrep -x`・いくつでも）の窓を試験の側の cua-driver の `list_windows`（hidden の app の窓も出る）で読み、`pick_window`。app は起こさない。
fn textedit_window(rig: &Rig, file: &Path) -> Result<(u32, u32), String> {
    let out = Command::new("pgrep").args(["-x", "TextEdit"]).output().map_err(|e| format!("pgrep: {e}"))?;
    let mut ws = Vec::new();
    for pid in String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
        let v = cua_call(&rig.cua_bin, "list_windows", json!({ "pid": pid }))?;
        ws.extend(v["windows"].as_array().into_iter().flatten().filter_map(|w| {
            Some((pid, w["window_id"].as_u64()? as u32, w["title"].as_str().unwrap_or("").to_string()))
        }));
    }
    pick_window(&ws, file)
}

/// 試験の側の cua-driver の session を始める（cua-driver は始めていない session の `get_window_state` を拒む・同じ名前で何度呼んでもよい）。
fn oracle_session(cua_bin: &Path) -> Result<(), String> {
    cua_call(cua_bin, "start_session", json!({ "session": ORACLE_SESSION })).map(drop)
}

/// 新しい道の保存の手（`MacGui::menu(pid, &SAVE_MENU)`）。返すのは log の 1 行だけ（効いたかは oracle の disk の sha で見る）。Old は手を打たない。
fn way_save(rig: &Rig, way: Way, pid: u32) -> String {
    match way {
        Way::Old => OLD_WAY_REMOVED.to_string(),
        Way::New => format!("{:?}", new_way(rig, pid).menu(pid, &SAVE_MENU)),
    }
}

/// 新しい道の MacGui。W19-1 の app ごとの AI の同意（既定は無し）は、試験の人が宛先の app（pid の bundle id を bridge の窓の一覧で読む）に
/// 「AI に任せる」を押した状態にしてから作る（同意の file は run ごとの temp・bridge の隣の build の dir には書かない）。読めなければ同意なしのまま（MacGui が断る）。
fn new_way(rig: &Rig, pid: u32) -> MacGui {
    let file = std::env::temp_dir().join(format!("old-new-act-consents-{}.jsonl", std::process::id()));
    let mut g = MacGui::at(rig.bridge.clone()).with_act_consents(file.clone());
    if let Some(w) = g.windows().ok().and_then(|ws| ws.into_iter().find(|w| w.pid == pid)) {
        let _ = super::surface::grant_act_consent(&file, &w.bundle_id, super::surface::now_ms());
    }
    g
}

/// TextEdit の行が fixture の文字を disk に書けたか（after の sha = `sha256(text)` かつ AXValue = text・before ≠ after）。表の判定（`compare` / `against`）とは別の事実で、
/// 旧い道の無い今の TextEdit の行を W18 の規則でどう数えるかは決めていない（報告で operator に渡す）。
pub fn textedit_wrote(row: &Row, text: &str) -> bool {
    let want = Reading::TextEdit { sha256: sha256_hex(text.as_bytes()), ax_value: text.to_string() };
    matches!(&row.status, Status::Measured { before, after } if *after == want && before != after)
}

// ---- 筋書きを回す ----

/// 回すのに要る物（どれも絶対 path）。cua_bin は旧い道と oracle の両方が使う試験の側の cua-driver。
#[derive(Debug, Clone)]
pub struct Rig {
    pub cua_bin: PathBuf,
    pub bridge: PathBuf,
    pub windows_bin: PathBuf,
    pub fixture: Fixture,
}

/// 1 つの筋書きを 1 つの道で回し、oracle で埋めた行を返す。panic しない・行を飛ばさない（読めない時は `Status::Unmeasured` と理由）。
/// 手の前に相手の app を同じ前の状態に戻す（計算機は "C"・TextEdit は書類を空にして保存し、空を読み戻す）。前面を取る手（activate・open -g 無し）は打たない。
/// 例: `let rows: Vec<Row> = ORDER.iter().flat_map(|s| [Way::Old, Way::New].map(|w| run(&rig, *s, w))).collect();`
pub fn run(rig: &Rig, scenario: Scenario, way: Way) -> Row {
    if way == Way::Old {
        return Row { scenario, way, status: Status::Unmeasured { reason: OLD_WAY_REMOVED.into() } };
    }
    let status = match measure(rig, scenario, way) {
        Ok((before, after)) => Status::Measured { before, after },
        Err(reason) => Status::Unmeasured { reason },
    };
    Row {
        scenario,
        way,
        status,
    }
}

/// 筋書き 1 本。Err = oracle が読めない・前提が無い（Unmeasured）。道が拒んだ・効かなかった手は Ok（after を読む・`compare` が NoEffect にする）。
fn measure(rig: &Rig, scenario: Scenario, way: Way) -> Result<(Reading, Reading), String> {
    oracle_session(&rig.cua_bin)?;
    match scenario {
        Scenario::Windows | Scenario::Calculator => {
            let (pid, wid) = only_window(rig, "Calculator")?;
            let read = || -> Result<Reading, String> {
                let display = ax_value(&rig.cua_bin, pid, wid, "AXStaticText")?;
                Ok(match scenario {
                    Scenario::Windows => Reading::Windows {
                        windows: cg_windows(&rig.windows_bin, pid)?
                            .iter()
                            .map(|w| [w.pid, w.window_id])
                            .collect(),
                        display,
                    },
                    _ => Reading::Calculator { display },
                })
            };
            clear_calculator(rig, pid, wid)?;
            let before = read()?;
            let keys: Vec<&str> = match scenario {
                // ① 検出 → ② 要素を読んで登録 → ③ key を 1 つ実行（旧パネルの筋書き）
                Scenario::Windows => rig
                    .fixture
                    .calculator_keys
                    .iter()
                    .take(1)
                    .map(String::as_str)
                    .collect(),
                _ => rig
                    .fixture
                    .calculator_keys
                    .iter()
                    .map(String::as_str)
                    .collect(),
            };
            for k in keys {
                eprintln!(
                    "{scenario:?} {way:?} 「{k}」 -> {}",
                    way_press(rig, way, pid, wid, k, Tool::Click)
                );
            }
            Ok((before.clone(), settle(&before, read)?))
        }
        Scenario::TextEdit => {
            // 先に開いた file が無ければ TextEdit に触らない（試験は TextEdit を起こさない・前面を取る）
            let Some(file) = &rig.fixture.textedit_file else {
                return Err("TextEdit で開いた file が無い（KANGAE_REAL_TEXTEDIT_FILE が無い）・試験は TextEdit を起こさない".into());
            };
            // 書いてよいのは temp の下の w17-3-*.txt だけ・題名がその file の窓だけ（共有の TextEdit の他の書類に触らない）
            textedit_file_ok(file)?;
            let (pid, wid) = textedit_window(rig, file)?;
            let read = || -> Result<Reading, String> {
                Ok(Reading::TextEdit {
                    sha256: file_sha256(file)?,
                    ax_value: ax_value(&rig.cua_bin, pid, wid, "AXTextArea")?,
                })
            };
            // 前の状態は「空の file を開いたばかり」だけ（空に戻す手は打たない: AXValue の set は書類を編集済みにせず保存が効かない・W17-2 戻しの偵察）
            let before = read()?;
            require_empty(&before)?;
            eprintln!(
                "TextEdit {way:?} 入力 -> {}",
                way_press(rig, way, pid, wid, "", Tool::Type(&rig.fixture.text))
            );
            // 保存も道の手（W17-2 戻し・旧い道は消したので道ごとに揃える相手が無い）
            eprintln!("TextEdit {way:?} 保存 -> {}", way_save(rig, way, pid));
            // 背景の TextEdit の保存は 2〜33 秒遅れて disk に出た（W17-2 戻しの偵察・押さなければ 50 秒書かれない）。disk の sha が変わるまで 1 秒おきに 60 回まで待ってから読む
            let Reading::TextEdit { sha256: empty, .. } = &before else { unreachable!("TextEdit の読み") };
            for _ in 0..60 {
                if file_sha256(file)? != *empty {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            Ok((before.clone(), read()?))
        }
    }
}

/// 背景の AXPress は遅れて効く。before と違う読みが出るまで 200ms おきに 5 回まで読む（変わらなければ最後の読み）。
fn settle(before: &Reading, read: impl Fn() -> Result<Reading, String>) -> Result<Reading, String> {
    let mut now = read()?;
    for _ in 0..5 {
        if now != *before {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
        now = read()?;
    }
    Ok(now)
}

/// 起動中の app（`pgrep -x`・ちょうど 1 つ）と、その画面上の窓（CGWindowList・ちょうど 1 つ）。app は起こさない。
fn only_window(rig: &Rig, app: &str) -> Result<(u32, u32), String> {
    let out = Command::new("pgrep")
        .args(["-x", app])
        .output()
        .map_err(|e| format!("pgrep: {e}"))?;
    let pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let [pid] = pids[..] else {
        return Err(format!(
            "{app} の process が {} 個（1 つ起動してから回す・試験は起こさない）",
            pids.len()
        ));
    };
    match cg_windows(&rig.windows_bin, pid)?[..] {
        [w] => Ok((pid, w.window_id)),
        ref ws => Err(format!("{app}（pid {pid}）の画面上の窓が {} 個", ws.len())),
    }
}

/// 試験の側の手: 計算機の表示を "0" に戻す（"clear" を押してから "all clear"・道を通さない）。
fn clear_calculator(rig: &Rig, pid: u32, wid: u32) -> Result<(), String> {
    for _ in 0..4 {
        let v = cua_state(&rig.cua_bin, pid, wid)?;
        let label = if one(&v, "AXButton", Some("clear")).is_ok() {
            "clear"
        } else {
            "all clear"
        };
        let e = one(&v, "AXButton", Some(label))?;
        let token = e["element_token"]
            .as_str()
            .ok_or("clear の element_token が無い")?;
        let target = json!({"kind": "window", "pid": pid, "window_id": wid});
        cua_call(
            &rig.cua_bin,
            "click",
            json!({"target": target, "element_token": token, "delivery_mode": "background", "session": ORACLE_SESSION}),
        )?;
        std::thread::sleep(Duration::from_millis(200));
        let v = cua_state(&rig.cua_bin, pid, wid)?;
        if label == "all clear" && ax_value_of(&v, "AXStaticText")? == "0" {
            return Ok(());
        }
    }
    Err("計算機の表示を 0 に戻せません".into())
}

fn ax_value_of(v: &Value, role: &str) -> Result<String, String> {
    Ok(one(v, role, None)?["value"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

enum Tool<'a> {
    Click,
    Type(&'a str),
}

/// 道ごとの ① 検出 → ② 要素 → ③ 登録して実行。製品の値（窓の一覧・要素・Outcome）は手を進めるのにだけ使い、返すのは log の 1 行だけ。
/// label が空 = 入力の欄（TextEdit の AXTextArea）。① の窓は pid と窓 id（oracle が選んだ窓）で 1 つに絞る（共有の TextEdit は窓が複数・W17-2 戻し）。
fn way_press(rig: &Rig, way: Way, pid: u32, wid: u32, label: &str, tool: Tool) -> String {
    let (dtool, text) = match tool {
        Tool::Click => (DesktopTool::Click, String::new()),
        Tool::Type(t) => (DesktopTool::TypeText, t.to_string()),
    };
    match way {
        // `run` が先に返す（旧い道は W18-1 で消した）
        Way::Old => OLD_WAY_REMOVED.to_string(),
        Way::New => {
            let mut g = new_way(rig, pid);
            let ws = match g.windows() {
                Ok(ws) => ws.into_iter().filter(|w| w.pid == pid && w.window_id == i64::from(wid)).collect::<Vec<_>>(),
                Err(e) => return format!("① 検出が失敗: {e}"),
            };
            let [w] = &ws[..] else {
                return format!("① 検出で pid {pid} の窓 {wid} が {} 個", ws.len());
            };
            let els = match g.elements(w) {
                Ok(e) => e,
                Err(e) => return format!("② 要素が失敗: {e}"),
            };
            let pick: Vec<_> = els
                .items
                .iter()
                .filter(|e| {
                    if label.is_empty() {
                        e.role == "AXTextArea"
                    } else {
                        e.label == label && e.role == "AXButton"
                    }
                })
                .collect();
            let [el] = &pick[..] else {
                return format!("② 要素で「{label}」が {} 個", pick.len());
            };
            let kind = ActionKind::Desktop {
                pid: pid as i64,
                window_id: w.window_id,
                label: el.label.clone(),
                role: el.role.clone(),
                tool: dtool,
                text,
                app_name: w.app_name.clone(),
            };
            format!("③ {:?}", g.run_desktop(&kind))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        let m = |d: &str| Reading::Calculator { display: d.into() };
        Table {
            version: TABLE_VERSION,
            measured_at: "2026-10-20T00:00:00Z".into(),
            head: "37a23dc".into(),
            fixture: Fixture {
                calculator_keys: vec!["1".into(), "+".into(), "2".into(), "=".into()],
                text: "w17-3".into(),
                textedit_file: None,
            },
            rows: vec![
                Row {
                    scenario: Scenario::Calculator,
                    way: Way::Old,
                    status: Status::Measured {
                        before: m("0"),
                        after: m("3"),
                    },
                },
                Row {
                    scenario: Scenario::TextEdit,
                    way: Way::New,
                    status: Status::Unmeasured {
                        reason: "owner が TextEdit を開いていない".into(),
                    },
                },
            ],
        }
    }

    #[test]
    fn old_new_table_round_trips_and_refuses_unknown_fields() {
        let t = table();
        let s = serde_json::to_string_pretty(&t).unwrap();
        assert_eq!(serde_json::from_str::<Table>(&s).unwrap(), t);
        let mut v: serde_json::Value = serde_json::from_str(&s).unwrap();
        v["rows"][0]["product_said"] = "verified".into(); // 製品の値の欄は表に入れない
        assert!(
            serde_json::from_value::<Table>(v).is_err(),
            "知らない欄を読んだ"
        );
    }

    /// YC W18-1: 旧い道は製品から消したので、`run(_, _, Way::Old)` はどの筋書きも手を打たず未測（理由 = OLD_WAY_REMOVED）。
    /// Rig の path はどれも在らない物: 手を打てば別の理由（起こせない）の未測になるので、理由の一致で「打っていない」を見る
    #[test]
    fn old_new_old_way_is_unmeasured_after_removal() {
        let rig = Rig {
            cua_bin: "/nonexistent/w18-1/cua-driver".into(),
            bridge: "/nonexistent/w18-1/bridge".into(),
            windows_bin: "/nonexistent/w18-1/windows".into(),
            fixture: Fixture { calculator_keys: vec!["1".into()], text: "w18-1".into(), textedit_file: None },
        };
        for s in ORDER {
            let r = run(&rig, s, Way::Old);
            assert_eq!((r.scenario, r.way), (s, Way::Old));
            assert_eq!(r.status, Status::Unmeasured { reason: OLD_WAY_REMOVED.into() }, "{s:?}");
        }
    }

    /// W18-1 interface: W17-3 で保存した表は W18-1 の後も読める（`Way::Old` の variant を残す理由）・版の違う表は読まない
    #[test]
    fn old_new_saved_table_loads_and_refuses_other_versions() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE_PATH);
        let saved = Table::load(&path).unwrap();
        assert!(saved.rows.iter().any(|r| r.way == Way::Old), "保存した表に Old の行が無い");
        assert!(saved.rows.iter().any(|r| r.way == Way::New), "保存した表に New の行が無い");
        let d = std::env::temp_dir().join(format!("old-new-load-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let other = d.join("table.json");
        let mut t = table();
        t.version = TABLE_VERSION + 1;
        std::fs::write(&other, serde_json::to_string(&t).unwrap()).unwrap();
        assert!(Table::load(&other).unwrap_err().contains("版"), "版の違う表を読んだ");
        assert!(Table::load(&d.join("無い.json")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_new_only_same_is_green() {
        let r = Reading::Calculator {
            display: "3".into(),
        };
        assert!(Agreement::Same.is_green());
        for a in [
            Agreement::Differs { a: r.clone(), b: r },
            Agreement::NoEffect { way: Way::New },
            Agreement::Unmeasured {
                reason: "未測".into(),
            },
        ] {
            assert!(!a.is_green(), "{a:?}");
        }
    }

    #[test]
    fn old_new_compare_order_and_missing_rows() {
        let m = |d: &str| Reading::Calculator { display: d.into() };
        let row = |way, b: &str, a: &str| Row {
            scenario: Scenario::Calculator,
            way,
            status: Status::Measured {
                before: m(b),
                after: m(a),
            },
        };
        let un = |way| Row {
            scenario: Scenario::Calculator,
            way,
            status: Status::Unmeasured {
                reason: "許可が無い".into(),
            },
        };
        let (old, new) = (row(Way::Old, "0", "3"), row(Way::New, "0", "3"));
        assert_eq!(compare(&old, &new), Agreement::Same);
        assert_eq!(
            compare(&old, &row(Way::New, "0", "4")),
            Agreement::Differs {
                a: m("3"),
                b: m("4")
            }
        );
        // 効かなかった手（before = after）は after が同じでも緑にしない（L2）・Differs より先
        assert_eq!(
            compare(&row(Way::Old, "0", "0"), &row(Way::New, "0", "0")),
            Agreement::NoEffect { way: Way::Old }
        );
        assert_eq!(
            compare(&old, &row(Way::New, "0", "0")),
            Agreement::NoEffect { way: Way::New }
        );
        // 未測が一番先（NoEffect の行と並んでも Unmeasured）
        assert_eq!(
            compare(&row(Way::Old, "0", "0"), &un(Way::New)),
            Agreement::Unmeasured {
                reason: "許可が無い".into()
            }
        );
        let mut t = table();
        t.rows = vec![old.clone(), new];
        let v = t.verdicts();
        assert_eq!(v.iter().map(|(s, _)| *s).collect::<Vec<_>>(), ORDER);
        assert_eq!(v[1].1, Agreement::Same);
        assert!(
            matches!(v[0].1, Agreement::Unmeasured { .. }),
            "窓の行が無いのに {:?}",
            v[0]
        );
        t.rows = vec![old];
        assert!(
            matches!(t.verdicts()[1].1, Agreement::Unmeasured { .. }),
            "New の行が無いのに緑"
        );
    }

    #[test]
    fn old_new_textedit_file_must_be_a_throwaway_under_temp() {
        let tmp = std::env::temp_dir().join(format!("w173-guard-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let ok = tmp.join("w17-3-a.txt");
        let bad_name = tmp.join("notes.txt");
        std::fs::write(&ok, "").unwrap();
        std::fs::write(&bad_name, "").unwrap();
        assert!(textedit_file_ok(&ok).is_ok(), "{:?}", textedit_file_ok(&ok));
        assert!(
            textedit_file_ok(&bad_name).is_err(),
            "名前が w17-3-*.txt でないのに通した"
        );
        let home = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(textedit_file_ok(&home).is_err(), "temp の外を通した");
        assert!(
            textedit_file_ok(&tmp.join("w17-3-none.txt")).is_err(),
            "無い file を通した"
        );
        // 題名: file 名か拡張子無しの名前で始まる・空は通さない
        assert!(title_matches("w17-3-a.txt", &ok));
        assert!(title_matches("w17-3-a — Edited", &ok));
        assert!(!title_matches("", &ok));
        assert!(!title_matches("Untitled", &ok));
        assert!(
            !title_matches("w17-3-ab.txt", &ok),
            "別の file の題名を通した"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn old_new_textedit_before_must_be_empty() {
        let empty = sha256_hex(b"");
        let r = |s: &str, a: &str| Reading::TextEdit {
            sha256: s.into(),
            ax_value: a.into(),
        };
        assert!(require_empty(&r(&empty, "")).is_ok());
        assert!(
            require_empty(&r(&empty, "w17-3-1")).is_err(),
            "本文が残っているのに通した"
        );
        assert!(
            require_empty(&r(&sha256_hex(b"x"), "")).is_err(),
            "disk が空でないのに通した"
        );
    }

    /// W17-2 戻し: 共有の TextEdit（列 M の窓が 4 つ・別の instance）から題名で 1 つだけ選ぶ。0 件・2 件以上・題名の前方一致の別の file は選ばない
    #[test]
    fn old_new_textedit_window_is_picked_by_title_across_instances() {
        let file = Path::new("/private/tmp/w17-3-a.txt");
        let w = |p: u32, id: u32, t: &str| (p, id, t.to_string());
        let shared = vec![w(36303, 5428, "w16m2-e1.md"), w(36303, 2181, "notes.md"), w(36303, 2768, ""), w(94903, 5565, "w17-3-a.txt")];
        assert_eq!(pick_window(&shared, file), Ok((94903, 5565)), "先頭や pid の数で選んだ");
        assert!(pick_window(&shared[..3], file).is_err(), "題名の合わない窓を選んだ");
        let twice = [shared.clone(), vec![w(36303, 5542, "w17-3-a.txt")]].concat();
        assert!(pick_window(&twice, file).unwrap_err().contains("2"), "同じ題名が 2 つなのに選んだ");
        assert!(pick_window(&[w(1, 2, "w17-3-ab.txt")], file).is_err(), "別の file を選んだ");
    }

    /// W17-2 戻し: 新しい道の TextEdit の手は、bridge の ax_set_value（本文に fixture の文字）→ ax_menu（SAVE_MENU）の順で、key_to_pid は打たない。
    /// 背景の app の menu の AXEnabled は古い（保存できるのに false）ので、bridge が灰色と読んでも押して、その事を log の 1 行に残す
    #[test]
    fn old_new_textedit_new_way_sets_then_saves_through_bridge() {
        let d = std::env::temp_dir().join(format!("w173-te-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("bridge.log");
        let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-surface-bridge.py");
        let env = format!("export FAKE_LOG='{}'\nexport FAKE_APP_NAME=TextEdit\nexport FAKE_MENU='File>Save'\nexport FAKE_MENU_STALE=1\nexec python3 '{py}'", log.display());
        let rig = Rig {
            cua_bin: "/nonexistent/w17-2/cua-driver".into(),
            bridge: super::super::script(&d, "bridge.sh", &env),
            windows_bin: "/nonexistent/w17-2/windows".into(),
            fixture: Fixture { calculator_keys: vec![], text: "w17-3-te".into(), textedit_file: None },
        };
        // 偽の bridge の窓 = pid 222・CG の窓番号 300（本文 = AXTextArea・置ける）
        let typed = way_press(&rig, Way::New, 222, 300, "", Tool::Type("w17-3-te"));
        let saved = way_save(&rig, Way::New, 222);
        assert!(typed.contains("Verified"), "{typed}");
        assert!(saved.contains("File › Save") && saved.contains("grayed out"), "{saved}");
        let lines: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(String::from).collect();
        let at = |cmd: &str, has: &str| lines.iter().position(|l| l.contains(&format!(r#""cmd":"{cmd}""#)) && l.contains(has));
        let set = at("ax_set_value", r#""value":"w17-3-te""#).expect("本文に置いていない");
        let menu = at("ax_menu", r#""path":["File","Save"]"#).expect("新しい道で保存していない");
        assert!(set < menu, "置く前に保存した");
        assert!(at("key_to_pid", "").is_none(), "pid 宛てに打った");
        // Old は手を打たない
        assert_eq!(way_save(&rig, Way::Old, 222), OLD_WAY_REMOVED);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// W17-2 戻し: TextEdit の行が fixture の文字を disk に書けたか（sha と AXValue の両方・before ≠ after）。表の判定とは別の事実
    #[test]
    fn old_new_textedit_wrote_needs_disk_and_ax() {
        let te = |s: &[u8], a: &str| Reading::TextEdit { sha256: sha256_hex(s), ax_value: a.into() };
        let row = |after| Row { scenario: Scenario::TextEdit, way: Way::New, status: Status::Measured { before: te(b"", ""), after } };
        assert!(textedit_wrote(&row(te(b"w17-3-x", "w17-3-x")), "w17-3-x"));
        assert!(!textedit_wrote(&row(te(b"", "w17-3-x")), "w17-3-x"), "置いただけ（保存されていない）を書けたと数えた");
        assert!(!textedit_wrote(&row(te(b"w17-3-x", "")), "w17-3-x"), "AXValue が違うのに書けたと数えた");
        assert!(!textedit_wrote(&row(te(b"w17-3-y", "w17-3-y")), "w17-3-x"), "別の文字を書けたと数えた");
        let un = Row { scenario: Scenario::TextEdit, way: Way::New, status: Status::Unmeasured { reason: "未測".into() } };
        assert!(!textedit_wrote(&un, "w17-3-x"));
    }

    #[test]
    fn old_new_red_table_is_not_saved() {
        let dir = std::env::temp_dir().join(format!("w173-save-{}", std::process::id()));
        let out = dir.join("table.json");
        let m = |d: &str| Reading::Calculator { display: d.into() };
        let row = |s, way, a: &str| Row {
            scenario: s,
            way,
            status: Status::Measured {
                before: m("0"),
                after: m(a),
            },
        };
        let mut t = table();
        t.rows = [Scenario::Windows, Scenario::Calculator]
            .into_iter()
            .flat_map(|s| [row(s, Way::Old, "3"), row(s, Way::New, "4")])
            .collect();
        assert!(save_if_green(&t, &out).is_err());
        assert!(!out.exists(), "赤い表を書いた");
        // 窓と計算機が Same・TextEdit の行は無い（未測は数えない）→ 書く
        t.rows = [Scenario::Windows, Scenario::Calculator]
            .into_iter()
            .flat_map(|s| [row(s, Way::Old, "3"), row(s, Way::New, "3")])
            .collect();
        save_if_green(&t, &out).unwrap();
        assert_eq!(
            serde_json::from_str::<Table>(&std::fs::read_to_string(&out).unwrap()).unwrap(),
            t
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_new_cg_windows_reads_pid_and_window_columns() {
        let dir = std::env::temp_dir().join(format!("w173-cg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("windows");
        std::fs::write(
            &bin,
            "#!/bin/sh\nprintf '0\\t3789\\t408\\t0,25,1680,955\\tOrca\\n1\\t65750\\t3320\\t1,1,1,1\\tCalculator\\n2\\t65750\\t3317\\t2,2,2,2\\tCalculator\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let w = cg_windows(&bin, 65750).unwrap();
        assert_eq!(
            w,
            vec![
                WindowFact {
                    pid: 65750,
                    window_id: 3317
                },
                WindowFact {
                    pid: 65750,
                    window_id: 3320
                }
            ]
        );
        assert!(cg_windows(&dir.join("none"), 1).is_err());
        let f = dir.join("f.txt");
        std::fs::write(&f, "abc").unwrap();
        assert_eq!(
            file_sha256(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 実機（BG）: KANGAE_REAL_CUA（試験の側の cua-driver）・KANGAE_REAL_BRIDGE・KANGAE_REAL_WINDOWS（windows.swift の binary）・
    /// KANGAE_REAL_TEXTEDIT_FILE（owner が開いた temp の下の w17-3-*.txt・無ければ TextEdit の行は未測）。表は判定が緑の時だけ TABLE_PATH に書く。
    #[test]
    #[ignore]
    fn old_new_real_desktop_table() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} が要ります"));
        let fixture = Fixture {
            calculator_keys: vec!["1".into(), "+".into(), "2".into(), "=".into()],
            text: format!("w17-3-{}", std::process::id()),
            textedit_file: std::env::var("KANGAE_REAL_TEXTEDIT_FILE")
                .ok()
                .map(PathBuf::from),
        };
        let rig = Rig {
            cua_bin: var("KANGAE_REAL_CUA").into(),
            bridge: var("KANGAE_REAL_BRIDGE").into(),
            windows_bin: var("KANGAE_REAL_WINDOWS").into(),
            fixture: fixture.clone(),
        };
        let rows: Vec<Row> = ORDER
            .iter()
            .flat_map(|s| [Way::Old, Way::New].map(|w| run(&rig, *s, w)))
            .collect();
        let t = Table {
            version: TABLE_VERSION,
            measured_at: var("KANGAE_REAL_AT"),
            head: var("KANGAE_REAL_HEAD"),
            fixture,
            rows,
        };
        println!("{:#?}", t.verdicts());
        // TextEdit の未測は緑に数えない（行は残す）。窓と計算機は Same でなければ赤・赤い run は正本を上書きしない
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE_PATH);
        save_if_green(&t, &out).unwrap();
    }

    /// W18-2: 保存した表の Same の筋書きだけを保存した New と比べ、それ以外は Unmeasured（holdout と共有・合成した行で回す）
    #[test]
    fn old_new_against_compares_only_saved_same() {
        let saved = Table::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE_PATH)).unwrap();
        let new_rows: Vec<Row> = saved.rows.iter().filter(|r| r.way == Way::New).cloned().collect();
        let v = saved.against(&new_rows);
        assert_eq!(v.iter().map(|(s, _)| *s).collect::<Vec<_>>(), ORDER.to_vec());
        for ((s, a), (_, was)) in v.iter().zip(saved.verdicts()) {
            assert_eq!(a.is_green(), was.is_green(), "{s:?}: 保存した表で Same の筋書きだけ緑");
        }
        let calc_off: Vec<Row> = new_rows
            .iter()
            .map(|r| match (r.scenario, &r.status) {
                (Scenario::Calculator, Status::Measured { before, .. }) => Row {
                    status: Status::Measured { before: before.clone(), after: Reading::Calculator { display: "999".into() } },
                    ..r.clone()
                },
                _ => r.clone(),
            })
            .collect();
        let off = saved.against(&calc_off);
        assert!(matches!(off[1], (Scenario::Calculator, Agreement::Differs { .. })), "{off:?}");
        let bump = |x: &Reading| match x {
            Reading::Windows { windows, display } => Reading::Windows { windows: windows.iter().map(|[p, w]| [p + 1, w + 1]).collect(), display: display.clone() },
            o => o.clone(),
        };
        let renumbered: Vec<Row> = new_rows
            .iter()
            .map(|r| match &r.status {
                Status::Measured { before, after } => Row { status: Status::Measured { before: bump(before), after: bump(after) }, ..r.clone() },
                _ => r.clone(),
            })
            .collect();
        assert!(saved.against(&renumbered)[0].1.is_green(), "窓の pid と窓 id が変わっただけで赤にしない");
    }

    /// W18-2 実機: 撤去した後の新しい道だけで W17-3 の筋書きを回し、保存した表と比べる（KANGAE_REAL_CUA・BRIDGE・WINDOWS・TEXTEDIT_FILE は
    /// `old_new_real_desktop_table` と同じ）。表は書き換えない。TextEdit が未測なら赤（W18 を done にしない・行を W17-2 に戻す）。
    /// **門は TextEdit が背景の時に通す**（operator 2026-10-23）: 新しい道は AI の道で、宛先の app が前面の間は人の lease（W19-1 `gui::human_holds`）で
    /// 打つ・保存するを送らない（製品の規則・`way_save` を bridge の `ax_menu` 直呼びに替えない）。前面での保存は人の手（A1 / E1）で、この門の外。
    /// 宛先の app には AI に任せる同意を置いてから回す（`new_way`・W19-1 の app ごとの同意）。
    #[test]
    #[ignore = "実機: W18-2 実装の後に BG で回す（TextEdit は背景）"]
    fn old_new_new_way_matches_saved_table() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} が要ります"));
        let saved = Table::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE_PATH)).unwrap();
        let rig = Rig {
            cua_bin: var("KANGAE_REAL_CUA").into(),
            bridge: var("KANGAE_REAL_BRIDGE").into(),
            windows_bin: var("KANGAE_REAL_WINDOWS").into(),
            fixture: Fixture { textedit_file: std::env::var("KANGAE_REAL_TEXTEDIT_FILE").ok().map(PathBuf::from), ..saved.fixture.clone() },
        };
        let now: Vec<Row> = ORDER.iter().map(|s| run(&rig, *s, Way::New)).collect();
        // KANGAE_REAL_DUMP=<path> なら今回の New の行を evidence に書く（表 TABLE_PATH は書き換えない）
        if let Ok(p) = std::env::var("KANGAE_REAL_DUMP") {
            std::fs::write(&p, serde_json::to_string_pretty(&now).unwrap()).unwrap();
        }
        if let Some(te) = now.iter().find(|r| r.scenario == Scenario::TextEdit) {
            println!("TextEdit の New が fixture の文字を disk に書けたか（textedit_wrote）: {}", textedit_wrote(te, &rig.fixture.text));
        }
        let v = saved.against(&now);
        println!("{v:#?}");
        assert!(v.iter().all(|(_, a)| a.is_green()), "{v:?}");
    }
}
