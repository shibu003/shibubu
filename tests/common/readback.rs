//! W2c: 試験の側の読み戻し（正の oracle・YC_ACCEPTANCE §2）と、偽の成功 L1〜L4 の判定（§4）。
//! 受け入れの `*_verified` を出してよいのはここと `scripts/qa/bg/oracle/*` だけ。製品の `surface::readback` の receipt は照合の相手で、正にしない。
//! std と serde_json だけに依る（`crate::ui` を引かない）—— どの test crate の `mod common;` からでも compile できる。
#![allow(dead_code)]

use serde_json::Value;

/// 操作とは別の道で読んだ before / after と、その差（1 件 1 行・JSON pointer と値）。
/// 例: `let o = Oracle::take("textedit_file_sha", before, after); assert!(o.changed(), "{:?}", o.diff);`
/// 保証する: diff は before と after だけから決まる（同じ入力なら同じ diff）。保証しない: before を操作の前に取った事（呼ぶ側の順）。
#[derive(Debug, Clone, PartialEq)]
pub struct Oracle {
    pub name: String,
    pub before: Value,
    pub after: Value,
    pub diff: Vec<String>,
}

impl Oracle {
    pub fn take(name: &str, before: Value, after: Value) -> Oracle {
        let mut diff = Vec::new();
        json_diff("", &before, &after, &mut diff);
        Oracle {
            name: name.to_string(),
            before,
            after,
            diff,
        }
    }

    /// 差が 1 件以上。「前後で同じ」を見る検査は、同じ run で changed() が true になる正の対照を 1 つ足す（§2）。
    pub fn changed(&self) -> bool {
        !self.diff.is_empty()
    }
}

/// 画面の AX の木（試験の道具が cua-driver の背景の読みで取った物を縮めた形）。identifier は `ui::ax::AxScope` の `<prefix>:<id>`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AxText {
    pub identifier: Option<String>,
    pub text: Option<String>,
    pub children: Vec<AxText>,
}

/// L1 の成功の語の表（YC_ACCEPTANCE §4）。
pub const SUCCESS_WORDS: &[&str] = &[
    "接続済み",
    "保存しました",
    "保存済み",
    "済み",
    "完了",
    "✓",
    "Connected",
    "Saved",
    "Done",
];

/// L1 ①: `surface-status:<surface_id>` の値（製品の表示）と試験の側の oracle の判定が合う。製品が verified で oracle が違えば Err。
/// 「確認済み」と読める表示 = 値が `verified` か、成功の語を含む。製品が控えめに出す（oracle が verified で表示が unverified）のは嘘ではないので通す。
pub fn l1_status(surface_id: &str, shown: &str, oracle_verified: bool) -> Result<(), String> {
    let claims = shown.trim() == "verified" || success_word(shown).is_some();
    if claims && !oracle_verified {
        return Err(format!("L1: surface-status:{surface_id} は「{shown}」と出したが、試験の側の oracle は verified でない"));
    }
    Ok(())
}

/// L1 ②: 成功の語が `terminal:*`・`webview:*`・`surface-frame:*`・`data:*` の外に出たら、その element が
/// `surface-status:<id>` か `toast:<id>` の中で、かつ `verified_surfaces` に id が在る時だけ通す。他は Err（どこに何が出たか）。
pub fn l1_words(tree: &AxText, verified_surfaces: &[&str]) -> Result<(), String> {
    fn walk(n: &AxText, owner: Option<&str>, path: &str, ok: &[&str], bad: &mut Vec<String>) {
        let (scope, id) = split(n.identifier.as_deref());
        if matches!(
            scope,
            Some("terminal" | "webview" | "surface-frame" | "data")
        ) {
            return; // 利用者・元の app のデータは製品の主張ではない
        }
        let owner = match scope {
            Some("surface-status" | "toast") => Some(id),
            _ => owner,
        };
        let here = format!("{path}/{}", n.identifier.as_deref().unwrap_or("_"));
        if let Some(w) = n.text.as_deref().and_then(success_word) {
            if !owner.is_some_and(|o| ok.contains(&o)) {
                bad.push(format!(
                    "{here}: 「{w}」（{}）",
                    n.text.as_deref().unwrap_or_default()
                ));
            }
        }
        for c in &n.children {
            walk(c, owner, &here, ok, bad);
        }
    }
    let mut bad = Vec::new();
    walk(tree, None, "", verified_surfaces, &mut bad);
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "L1: verified でない所に成功の語: {}",
            bad.join(" / ")
        ))
    }
}

/// L2: 手の前後で「期待する効果」（oracle の差か、`target` の identifier の部分木の変化）も、拒否の 1 行も無ければ Err。
/// 時刻・spinner・hover・進捗の node の変化は数えない。
pub fn l2_effect(
    step: &str,
    oracle: Option<&Oracle>,
    target: &str,
    before: &AxText,
    after: &AxText,
    refusal: Option<&str>,
) -> Result<(), String> {
    if oracle.is_some_and(Oracle::changed) {
        return Ok(());
    }
    if refusal.is_some_and(|r| !r.trim().is_empty()) {
        return Ok(());
    }
    let (b, a) = (
        find(before, target).map(stable),
        find(after, target).map(stable),
    );
    if b != a {
        return Ok(());
    }
    Err(format!("L2: {step}: 押しても何も起きない（oracle の差 0・{target} の部分木が同じ・拒否の 1 行も無い）"))
}

/// 時刻・spinner・hover・進捗の node（identifier の頭がこの表）は L2 の差に数えない。
pub const VOLATILE: &[&str] = &["time", "spinner", "hover", "progress"];

fn find<'a>(n: &'a AxText, id: &str) -> Option<&'a AxText> {
    if n.identifier.as_deref() == Some(id) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, id))
}

/// 部分木から VOLATILE の node を抜いた形（比べる用）。
fn stable(n: &AxText) -> AxText {
    AxText {
        identifier: n.identifier.clone(),
        text: n.text.clone(),
        children: n
            .children
            .iter()
            .filter(
                |c| !matches!(split(c.identifier.as_deref()).0, Some(p) if VOLATILE.contains(&p)),
            )
            .map(stable)
            .collect(),
    }
}

/// L3: Terminal の出力が本物か。出力に回ごとの `nonce` と `product`（積）が在り、`pid` が `ps` に在って親が `shibubu_pid`・tty を持つ。
/// `ps` は試験の側で呼ぶ（製品の registry を見ない）。
pub fn l3_terminal(
    output: &str,
    nonce: &str,
    product: u64,
    pid: u32,
    shibubu_pid: u32,
) -> Result<(), String> {
    let mut bad = Vec::new();
    if nonce.is_empty() || !output.contains(nonce) {
        bad.push(format!("出力に nonce「{nonce}」が無い"));
    }
    // 積は自分の行に出ている事（nonce や打った式の中の数字では通さない）
    if !output.lines().any(|l| l.trim() == product.to_string()) {
        bad.push(format!("出力に積 {product} が無い"));
    }
    let ps = std::process::Command::new("/bin/ps")
        .args(["-o", "ppid=,tty=,comm=", "-p", &pid.to_string()])
        .output();
    match ps {
        Err(e) => bad.push(format!("ps を起こせない: {e}")),
        Ok(o) => {
            let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let mut f = line.split_whitespace();
            match (
                f.next().and_then(|p| p.parse::<u32>().ok()),
                f.next(),
                f.next(),
            ) {
                (Some(ppid), Some(tty), Some(comm)) => {
                    if ppid != shibubu_pid {
                        bad.push(format!(
                            "pid {pid} の親は {ppid}（Shibubu {shibubu_pid} でない）"
                        ));
                    }
                    if tty == "??" || tty == "-" {
                        bad.push(format!("pid {pid} は tty を持たない"));
                    }
                    let name = comm
                        .rsplit('/')
                        .next()
                        .unwrap_or(comm)
                        .trim_start_matches('-');
                    if !matches!(name, "zsh" | "bash") {
                        bad.push(format!("pid {pid} は {comm}（zsh / bash でない）"));
                    }
                }
                _ => bad.push(format!("pid {pid} が ps に無い")),
            }
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("L3: {}", bad.join(" / ")))
    }
}

/// 試験の側が CGWindowList から読んだ Native の窓 1 つ（`scripts/qa/bg/oracle/windows.swift` の 1 行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowFact {
    pub pid: u32,
    pub window_id: u32,
}

/// L4: 移動・編集・再起動の前後で Native の窓（Native Direct か Mirror の surface）の pid と窓 id が同じ。
/// before が 0 件なら Ok でなく Err（測っていない緑を出さない）。
pub fn l4_same_windows(before: &[WindowFact], after: &[WindowFact]) -> Result<(), String> {
    if before.is_empty() {
        return Err("L4: 前の窓が 0 件（測っていない。surface が 1 つ以上ある場面で回す）".into());
    }
    let sorted = |w: &[WindowFact]| {
        let mut v: Vec<(u32, u32)> = w.iter().map(|w| (w.pid, w.window_id)).collect();
        v.sort_unstable();
        v
    };
    let (b, a) = (sorted(before), sorted(after));
    if b == a {
        Ok(())
    } else {
        Err(format!("L4: 窓がすり替わった（前 {b:?} → 後 {a:?}）"))
    }
}

/// `<prefix>:<id>` → (prefix, id)。`:` の無い identifier と None は (None, "")。
fn split(identifier: Option<&str>) -> (Option<&str>, &str) {
    match identifier.and_then(|i| i.split_once(':')) {
        Some((p, id)) => (Some(p), id),
        None => (None, ""),
    }
}

/// 表の「書いたが読み戻していない」の語（docs/ui-copy/copy.en.md の `Saved, not checked`）。成功の主張ではないので、成功の語を探す前に外す（UI-2）
pub const NOT_CHECKED: &str = "Saved, not checked";

fn success_word(text: &str) -> Option<&'static str> {
    let text = text.replace(NOT_CHECKED, "");
    SUCCESS_WORDS.iter().copied().find(|w| text.contains(w))
}

/// JSON の差を 1 件 1 行で（`<pointer>: <前> -> <後>`・片方に無い時は `(無い)`）。
fn json_diff(at: &str, b: &Value, a: &Value, out: &mut Vec<String>) {
    match (b, a) {
        (Value::Object(bo), Value::Object(ao)) => {
            let mut keys: Vec<&String> = bo.keys().chain(ao.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = format!("{at}/{}", k.replace('~', "~0").replace('/', "~1"));
                match (bo.get(k), ao.get(k)) {
                    (Some(x), Some(y)) => json_diff(&p, x, y, out),
                    (x, y) => out.push(format!("{p}: {} -> {}", show(x), show(y))),
                }
            }
        }
        (Value::Array(ba), Value::Array(aa)) => {
            for i in 0..ba.len().max(aa.len()) {
                let p = format!("{at}/{i}");
                match (ba.get(i), aa.get(i)) {
                    (Some(x), Some(y)) => json_diff(&p, x, y, out),
                    (x, y) => out.push(format!("{p}: {} -> {}", show(x), show(y))),
                }
            }
        }
        _ if b != a => out.push(format!(
            "{}: {b} -> {a}",
            if at.is_empty() { "/" } else { at }
        )),
        _ => {}
    }
}

fn show(v: Option<&Value>) -> String {
    v.map_or("(無い)".into(), Value::to_string)
}

// ---- E0: Mirror の映像が今の元の app（W16M-1・YC_ACCEPTANCE §3 の E0）----

/// Shibubu の窓の写真 1 枚の Mirror の矩形を OCR した結果。`at_ms` は写真を撮り始めた時刻（unix ms・`ocr_window` を呼ぶ前）。
/// W16M-1b: E0 の 2 秒はこの撮影開始の時刻で判定する（1 枚に 5〜6 秒かかる試験の側の遅さを製品の遅れに数えない）。
#[derive(Debug, Clone, PartialEq)]
pub struct OcrShot {
    pub at_ms: f64,
    pub lines: Vec<String>,
}

/// 試験の側の OCR: `screencapture -x -l <window_id>` で Shibubu の窓を撮り、`crop`（窓の中の point・x, y, w, h。
/// AX の `surface-frame:<surface_id>` の frame から窓の原点を引いた物）を切り出して、`scripts/qa/bg/oracle/ax.swift ocr`（Vision の
/// `VNRecognizeTextRequest`・試験の側だけ。src に Vision を入れない）で読んだ行。
/// 例: `let shot = OcrShot { at_ms: now_ms(), lines: ocr_window(wid, rect)? };`
/// 保証する: 撮れない（画面収録の許可が無い・窓が無い）時は Err に「未測」の語と理由（空の行で Ok を返さない＝黒い画面と区別する）。
/// 保証しない: 読んだ文字が正しい事（Vision の誤読は e0_current の nonce の照合で落ちる）。
pub fn ocr_window(window_id: u32, crop: [f64; 4]) -> Result<Vec<String>, String> {
    let png = std::env::temp_dir().join(format!("w16m1-shot-{}-{window_id}.png", std::process::id()));
    capture_window(window_id, &png)?;
    let r = ocr_png(&png, window_id, crop);
    let _ = std::fs::remove_file(&png);
    r
}

/// W16M-1b: 写真だけ撮る（`screencapture -x -o -l`）。E0 は撮るのを先に 250ms 刻みで済ませ、OCR（1 枚 5〜6 秒）は後で回す。
pub fn capture_window(window_id: u32, png: &std::path::Path) -> Result<(), String> {
    let _ = std::fs::remove_file(png);
    let o = std::process::Command::new("/usr/sbin/screencapture").args(["-x", "-o", "-l", &window_id.to_string()]).arg(png).output().map_err(|e| format!("未測: screencapture を起こせない: {e}"))?;
    if !o.status.success() || !png.exists() {
        return Err(format!("未測: 窓 {window_id} を撮れない（画面収録の許可が無いか窓が無い）: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(())
}

/// W16M-1b: 撮った写真の `crop` を Vision で読む（`ocr_window` の後半）。
pub fn ocr_png(png: &std::path::Path, window_id: u32, crop: [f64; 4]) -> Result<Vec<String>, String> {
    let tool = oracle_tool("ax")?;
    let [x, y, w, h] = crop.map(|v| v.to_string());
    let o = std::process::Command::new(&tool).arg("ocr").arg(png).arg(window_id.to_string()).args([&x, &y, &w, &h]).output().map_err(|e| format!("未測: ocr を起こせない: {e}"))?;
    if !o.status.success() {
        return Err(format!("未測: OCR できない: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    serde_json::from_slice(&o.stdout).map_err(|e| format!("未測: OCR の答えを読めない: {e}"))
}

/// `scripts/qa/bg/oracle/<name>.swift` を swiftc で `target/w16m1-<name>` に作る（source より新しければ作り直さない）。失敗は「未測」。
pub fn oracle_tool(name: &str) -> Result<std::path::PathBuf, String> {
    use std::process::Command;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join(format!("scripts/qa/bg/oracle/{name}.swift"));
    let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| root.join("target"));
    let tool = target.join(format!("w16m1-{name}"));
    let mtime = |p: &std::path::Path| p.metadata().and_then(|m| m.modified()).ok();
    if mtime(&tool) <= mtime(&src) {
        // 並んで走る試験と取り合わないよう別名に作ってから置き換える
        let tmp = target.join(format!("w16m1-{name}.{}", std::process::id()));
        let o = Command::new("/usr/bin/xcrun").args(["swiftc", "-O"]).arg(&src).arg("-o").arg(&tmp).output().map_err(|e| format!("未測: swiftc を起こせない: {e}"))?;
        if !o.status.success() {
            return Err(format!("未測: {name}.swift を build できない: {}", String::from_utf8_lossy(&o.stderr).lines().take(3).collect::<Vec<_>>().join(" / ")));
        }
        std::fs::rename(&tmp, &tool).map_err(|e| format!("未測: {e}"))?;
    }
    Ok(tool)
}

/// E0 の判定（純関数）。`before` = nonce1 を入れた後・入れ替える前の写真、`after` = `changed_at_ms` に nonce2 へ入れ替えた後の写真。
/// 合格: before のどれかに nonce1 が写り、`changed_at_ms` から 2 秒以内の after のどれかに nonce2 が写って nonce1 が写っていない。
/// 例: `e0_current(&before, &n1, &after, &n2, changed_at)?`
/// 保証する: 黒い画面（行 0）・最初の frame で止まった映像（nonce1 のまま）・2 秒より遅い映像・写真 0 枚は Err（測っていない緑を出さない）。
/// OCR が行を分けても拾うよう、行を空白を除いて繋げた文字で探す。保証しない: 写真が Mirror の矩形を撮った事（crop は呼ぶ側）。
pub fn e0_current(
    before: &[OcrShot],
    nonce1: &str,
    after: &[OcrShot],
    nonce2: &str,
    changed_at_ms: f64,
) -> Result<(), String> {
    let has = |s: &OcrShot, n: &str| {
        s.lines
            .concat()
            .split_whitespace()
            .collect::<String>()
            .contains(n)
    };
    if before.is_empty() || after.is_empty() {
        return Err(format!(
            "未測: 写真が無い（before {}・after {}）",
            before.len(),
            after.len()
        ));
    }
    if !before.iter().any(|s| has(s, nonce1)) {
        let lines: usize = before.iter().map(|s| s.lines.len()).sum();
        return Err(format!(
            "Mirror に nonce1 が写っていない（OCR の行 {lines}・0 なら黒い画面か何も映っていない）"
        ));
    }
    let deadline = changed_at_ms + 2000.0;
    match after
        .iter()
        .filter(|s| s.at_ms >= changed_at_ms && s.at_ms <= deadline)
        .find(|s| has(s, nonce2) && !has(s, nonce1))
    {
        Some(_) => Ok(()),
        None if after.iter().any(|s| has(s, nonce2)) => {
            Err("nonce2 が写ったのは 2 秒より後（映像が遅い）".into())
        }
        None => {
            Err("入れ替えた後も nonce2 が写らない（映像が止まっている・前の frame のまま）".into())
        }
    }
}

/// W16M-1b: E0 の合否（純関数）。OCR の判定（`e0_current`・撮影開始の時刻で 2 秒）と、製品の log の遅れ（`frame_latency` の `total_ms`
/// ＝ TextEdit に書いた時刻 → texture の更新）の両方を記録し、**両方が通った時だけ** Ok（製品の遅れ ms を返す）。
/// 例: `let ms = e0_verdict(e0_current(&b, &n1, &a, &n2, t), frame_latency(&recs, "notes", t).map(|l| l.total_ms))?;`
/// 保証する: 製品の log だけ・OCR だけでは緑にしない・log の遅れが 2 秒を超えれば赤・どちらかの「未測」はそのまま Err に残す。
/// 保証しない: 2 つが同じ frame を見た事（OCR は写真・log は texture の更新）。
pub fn e0_verdict(ocr: Result<(), String>, product_total_ms: Result<f64, String>) -> Result<f64, String> {
    match (ocr, product_total_ms) {
        (Ok(()), Ok(ms)) if ms <= 2000.0 => Ok(ms),
        (Ok(()), Ok(ms)) => Err(format!("OCR は 2 秒以内だが製品の log の遅れが {ms:.0} ms（2 秒を超えた）")),
        (o, l) => Err(format!(
            "OCR: {}・製品の log: {}",
            o.err().unwrap_or_else(|| "通った".into()),
            l.map(|ms| format!("{ms:.0} ms")).unwrap_or_else(|e| e)
        )),
    }
}

// ---- E1: Mirror の入力が Shibubu を通って元の app に届いた（W16M-2・YC_ACCEPTANCE §3 の E1）----

/// relay の記録 1 行のうち E1 が使う物。製品の `surface::mirror::RelayRecord` は feature `mirror` の後ろなので取り込まず、JSONL を読む。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelayFact {
    pub seq: u64,
    pub at_ms: f64,
}

/// relay の記録の file（`KANGAE_MIRROR_RELAY_LOG` の指す JSONL）の中身 → `RelayFact` の並び。
/// 例: `let relays = relay_facts(&std::fs::read_to_string(&log).map_err(|e| format!("未測: {e}"))?)?;`
/// 保証する: 空行は飛ばす・答えの行（v 3 の `outcome` 付き = 送った後に helper の答えが着いた時刻の行・MIRROR-HYBRID-1101 追記 235）は飛ばす（送った時刻だけで E1 を見る）・
/// `seq`（整数）か `at_ms`（数）の無い行・JSON でない行は Err（何行目か）。保証しない: 他の欄の形（製品の serde の試験）。
pub fn relay_facts(jsonl: &str) -> Result<Vec<RelayFact>, String> {
    jsonl
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .filter_map(|(i, l)| {
            let v: Value = match serde_json::from_str(l) {
                Ok(v) => v,
                Err(e) => return Some(Err(format!("relay の記録の {} 行目を読めない: {e}", i + 1))),
            };
            if v.get("outcome").is_some() {
                return None;
            }
            Some(match (v["seq"].as_u64(), v["at_ms"].as_f64()) {
                (Some(seq), Some(at_ms)) => Ok(RelayFact { seq, at_ms }),
                _ => Err(format!("relay の記録の {} 行目に seq か at_ms が無い: {l}", i + 1)),
            })
        })
        .collect()
}

/// 試験の側の oracle が読んだ元の app の値の変化 1 つ（TextEdit の AX value・保存の後の disk の sha・REAPER の ReaScript の値と `.rpp`）。
/// `at_ms` = 変化を読んだ時刻（手の後に読んだ after の時刻）・`what` = 何が変わったかの 1 行（証拠と Err に出す）。
#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange {
    pub at_ms: f64,
    pub what: String,
}

/// E1 の経路の判定（純関数）。変化を時刻の順に並べ、変化 1 つごとに、その前（1 つ前の変化より後・その変化より前）に relay の記録が 1 件以上在る事。
/// 合格なら変化の数を返す。
/// 例: `let n = e1_path(&relay_facts(&log)?, &[ValueChange { at_ms: t_ax, what: "TextEdit の AX value".into() }, ValueChange { at_ms: t_disk, what: "notes.md の sha".into() }])?;`
/// 保証する: 変化 0 件は「未測」の Err（測っていない緑を出さない）・relay の無い変化が 1 件でも在れば Err（試験の側から直接 `postToPid` した変化＝負の対照）・
/// relay より前の変化は通さない（時刻の順が relay → 変化）。保証しない: その relay がその変化を起こした事（値を変えなかった relay の直後の直接の変化は
/// 見分けない＝負の対照は relay 0 件の run で取る）・値の中身（oracle の側）。
pub fn e1_path(relays: &[RelayFact], changes: &[ValueChange]) -> Result<usize, String> {
    if changes.is_empty() {
        return Err(format!("未測: 元の app の値の変化が無い（relay の記録 {} 件）", relays.len()));
    }
    let mut sorted: Vec<&ValueChange> = changes.iter().collect();
    sorted.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
    let mut prev = f64::NEG_INFINITY;
    for c in &sorted {
        if !relays.iter().any(|r| r.at_ms > prev && r.at_ms < c.at_ms) {
            return Err(format!("relay の記録の無い変化: {}（{:.0} ms）。Shibubu の Mirror を通らずに変わった", c.what, c.at_ms));
        }
        prev = c.at_ms;
    }
    Ok(sorted.len())
}
