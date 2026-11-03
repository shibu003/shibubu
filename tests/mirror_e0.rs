//! Mirror の実機の受け入れ（YC_ACCEPTANCE §3）: W16M-1 の E0（映像が今の元の app）と W16M-2 の E1（Mirror の上の入力が Shibubu を通って元の app を
//! 変える・`w16m2_e1_*`・道具は scripts/qa/bg/mirror-e1.sh）。
//! E0: 実機の BG の段（scripts/qa/bg の W16M-1 の手）が
//! Shibubu を `open -g --env SHIBUBU_BACKGROUND=1` で起こし、cua-driver の背景の AX の読みで Shibubu の窓 id と
//! `surface-frame:<surface_id>` の矩形を取って、環境変数で渡してから `cargo test --features mirror --test mirror_e0 -- --ignored` で回す。
//! 製品の code は何も呼ばない（操作は試験の側の AppleScript・読みは試験の側の screencapture と Vision）。
//! 負の対照（実装の commit で取る）: `MirrorPages::show` が黒い texture を上げる・最初の frame の後に `stream` を読まない → どちらも赤。
#![cfg(feature = "mirror")]
#![allow(dead_code, unused_imports)]

#[path = "common/readback.rs"]
mod readback;
// Mirror の Page を data dir に置く試験の側の手（`w16m1_seed_mirror_page`）が `Generated` を使う（yc_demo_e2e と同じ取り込み）
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/storage/mod.rs"]
mod storage;
#[path = "../src/runtime/mod.rs"]
mod runtime;
#[path = "../src/composer/mod.rs"]
mod composer;
#[path = "../src/agent/mod.rs"]
mod agent;
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
#[path = "../src/project/mod.rs"]
mod project;
// 統合（yc/trunk）: generated.rs が S の Terminal（W4〜W10）と W13 の `with_llm` を引くので、yc_demo_e2e.rs と同じく取り込む（`llm_cli` は取り込まない）
#[path = "../src/terminal/mod.rs"]
mod terminal;
#[path = "../src/llm/mod.rs"]
mod llm;
#[path = "../src/generated.rs"]
mod generated;
// G88b: generated の take の帯が録音（`audio::record`）と REAPER への insert（`audio::reaper`）を呼ぶ
#[path = "../src/audio/mod.rs"]
mod audio;

use readback::{capture_window, e0_current, e0_verdict, e1_path, ocr_png, relay_facts, OcrShot, ValueChange};
use std::process::Command;
use std::time::Duration;
use surface::mirror::{helper_json, now_ms};

/// 渡されていなければ「未測」で赤（skip も緑もしない）。
fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| {
        panic!("未測: {k} が無い（scripts/qa/bg の W16M-1 の手が Shibubu を BG で起こして渡す）")
    })
}

/// 試験の側が TextEdit の文書の窓（題名 = doc）の text area の AXValue を替え、書けた時刻（unix ms）を返す
/// （`scripts/qa/bg/oracle/ax.swift set`・activate しない＝前面を変えない）。AppleScript の `set text` は混んだ機械で 80 秒かかったので使わない。
fn textedit_set_text(doc: &str, text: &str) -> f64 {
    let pid = String::from_utf8_lossy(&Command::new("/usr/bin/pgrep").args(["-x", "TextEdit"]).output().expect("pgrep").stdout).lines().next().map(str::to_string);
    let pid = pid.unwrap_or_else(|| panic!("未測: TextEdit が起きていない（試験の側が open -g -a TextEdit で {doc} を開く）"));
    let tool = readback::oracle_tool("ax").unwrap_or_else(|e| panic!("{e}"));
    let o = Command::new(tool).args(["set", pid.as_str(), doc, text]).output().expect("ax set");
    assert!(o.status.success(), "未測: TextEdit に入れられない: {}", String::from_utf8_lossy(&o.stderr));
    now_ms()
}

/// `until_ms` まで `every` ごとに撮り（撮影開始の時刻を残す）、撮り終えてから全部を OCR する（W16M-1b: 1 枚 5〜6 秒の OCR で
/// 250ms 刻みを崩さない）。撮れなければ「未測」で赤（画面収録の許可が無い時は owner に許可を頼む 1 行）。
fn shots(wid: u32, rect: [f64; 4], until_ms: f64, every: Duration) -> Vec<OcrShot> {
    let dir = std::env::temp_dir().join(format!("w16m1b-e0-{}-{}", std::process::id(), now_ms() as u64));
    std::fs::create_dir_all(&dir).unwrap();
    let mut taken = vec![];
    while now_ms() < until_ms {
        let at_ms = now_ms();
        let png = dir.join(format!("{}.png", taken.len()));
        capture_window(wid, &png).unwrap_or_else(|e| {
            panic!("{e}（画面収録の許可が無い時: owner に「システム設定 → プライバシーとセキュリティ → 画面収録 で、試験を回す terminal を許可してください」と頼む）")
        });
        taken.push((at_ms, png));
        std::thread::sleep(every);
    }
    let v = taken.iter().map(|(at_ms, png)| OcrShot { at_ms: *at_ms, lines: ocr_png(png, wid, rect).unwrap_or_else(|e| panic!("{e}")) }).collect();
    let _ = std::fs::remove_dir_all(&dir);
    v
}

#[test]
#[ignore = "実機: 画面収録の許可・TextEdit の notes.md・BG の Shibubu の Mirror の Page（scripts/qa/bg の W16M-1 の手が起こして環境変数で渡す）"]
fn w16m1_e0_mirror_shows_the_current_textedit_text() {
    let wid: u32 = env("W16M1_SHIBUBU_WID")
        .parse()
        .expect("W16M1_SHIBUBU_WID は数");
    let r: Vec<f64> = env("W16M1_MIRROR_RECT")
        .split(',')
        .map(|s| s.trim().parse().expect("W16M1_MIRROR_RECT は x,y,w,h"))
        .collect();
    let rect = [r[0], r[1], r[2], r[3]];
    let doc = env("W16M1_TEXTEDIT_DOC"); // 例 notes.md（Docs の Page の README とは別の窓・W16M-0）
    // W16M-1b: Vision が 0 と O・1 と l を読み違えた（"e0n1…" → "eOn1…"）ので、紛れない小文字 16 字だけで nonce を作る
    let tag: String = format!("{:x}", now_ms() as u64).chars().map(|c| b"abcdefhkmnprtwxy"[c.to_digit(16).unwrap() as usize] as char).collect();
    let (n1, n2) = (format!("first{tag}"), format!("second{tag}"));

    let set1 = textedit_set_text(&doc, &n1);
    let before = shots(wid, rect, set1 + 2500.0, Duration::from_millis(500));
    // 2 秒の起点は TextEdit に書けた時刻（書く道具の遅さを映像の遅さに数えない）
    let changed_at = textedit_set_text(&doc, &n2);
    let after = shots(wid, rect, changed_at + 2500.0, Duration::from_millis(250));
    // W16M-1b: 製品の log（Shibubu を `KANGAE_MIRROR_FRAME_LOG` 付きで起こした時の texture の更新の記録）でも 2 秒を見る。両方が通った時だけ緑
    let log = std::path::PathBuf::from(env(surface::mirror::FRAME_LOG_ENV));
    let latency = surface::mirror::FrameLog::read(&log).map_err(|e| format!("未測: {e:?}")).and_then(|recs| surface::mirror::frame_latency(&recs, "notes", changed_at));
    println!("E0 changed_at={changed_at:.0} latency={latency:?}");
    println!("E0 after(撮影開始 − changed_at・nonce2 の有無)={:?}", after.iter().map(|s| ((s.at_ms - changed_at) as i64, s.lines.concat().contains(&n2))).collect::<Vec<_>>());
    match e0_verdict(e0_current(&before, &n1, &after, &n2, changed_at), latency.map(|l| l.total_ms)) {
        Ok(ms) => println!("E0 緑: 製品の log の遅れ {ms:.0} ms"),
        Err(e) => panic!("E0 が赤: {e}\nbefore: {before:?}\nafter: {after:?}"),
    }
}

/// 実機の BG の段の前段: 空の data dir に、一言から作った app の Project と、その先頭の Page に Mirror の面
/// （`SurfaceSpec{id: "notes", backend: Mirror, target: TextEdit + W16M1_TEXTEDIT_DOC}`・`source_surface` の node）を置く。
/// Mirror の Page を足す自然言語の手は W16M-3 なので、ここは直接の編集（YC_WAVES W16M-0 の「Mirror の Page の作り方」）。
/// 例: `W16M1_SEED_DIR=$D W16M1_TEXTEDIT_DOC=docs/notes.md cargo test --features mirror --test mirror_e0 w16m1_seed -- --ignored`
#[test]
#[ignore = "実機の BG の段の前段（W16M1_SEED_DIR に Mirror の Page を置く）"]
fn w16m1_seed_mirror_page() {
    use domain::project::{ProjectDefinitionV1, ProjectPatchOp, RevisionOrigin, SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
    use serde_json::json;
    let dir = std::path::PathBuf::from(env("W16M1_SEED_DIR"));
    let doc = env("W16M1_TEXTEDIT_DOC");
    let mut g = generated::Generated::open(&dir);
    let app = g.create("メモを書きたい").expect("一言から app を作る");
    let (p, hash) = g.project_of(&app).unwrap().expect("Project");
    // 包んだだけの Project を保存の行にする（名前を変えるだけの差分）
    let first = p.pages[0].id.clone();
    let (_, _) = g.apply_project(&app, &hash, &[ProjectPatchOp::RenamePage { page_id: first.clone(), title: "Mirror".into() }], RevisionOrigin::DirectEdit).expect("Page の名前");
    drop(g);
    let store = storage::Store::open(&dir.join("kangae.sqlite3")).unwrap();
    let row = store.get(storage::Table::Projects, &p.id).unwrap().expect("保存した Project");
    let mut p: ProjectDefinitionV1 = serde_json::from_value(row.json).unwrap();
    p.surface_specs.push(SurfaceSpec {
        id: "notes".into(),
        backend: SurfaceBackendKind::Mirror,
        target: SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some(doc) },
        keep_alive: true,
        frame: None,
        browser: false,
    });
    let node = serde_json::from_value(json!({"key": "notes-mirror", "component": "source_surface", "label": "notes.md", "props": {"surface_ref": "notes", "display": "mirror"}})).unwrap();
    p.pages[0].root.children.insert(0, node);
    store.put(storage::Table::Projects, &p.id, None, &serde_json::to_value(&p).unwrap()).unwrap();
    // 開き直して読める・Mirror の面が在る
    let g = generated::Generated::open(&dir);
    let (back, _) = g.project_of(&app).unwrap().unwrap();
    assert_eq!(back.surface_specs.len(), 1);
    println!("SEEDED project={} page={first}", back.id);
}

/// 今の前面の app（LaunchServices の ASN）。手の前後で同じ事を見る（D-g の 1 の一部・全部は mirror-e1.sh の front-guard）。
fn front() -> String {
    String::from_utf8_lossy(&Command::new("/usr/bin/lsappinfo").arg("front").output().expect("lsappinfo").stdout).trim().to_string()
}

/// `ok` が真になるまで 10 秒待ち、真になった時刻（unix ms・読み戻しの時刻 = ValueChange の at_ms）を返す。来なければ赤。
fn wait_until(what: &str, ok: impl Fn() -> bool) -> f64 {
    for _ in 0..100 {
        if ok() {
            return now_ms();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("E1 が赤: 10 秒待っても {what}");
}

/// 書類の窓を持つ TextEdit の (pid, 窓 id)。TextEdit は他の列の試験が別の process で起こしている事がある（2026-10-21 に 2 つ在った）。
/// **stream を起こす前に呼ぶ**: helper の `list`（ScreenCaptureKit）の process が終わると、同じ主体の動いている stream が -3805 で止まる（W16M-2 で実測）。
/// M4: 同じ題名の別の書類の窓（前の run の復元・試験の罠）を読み戻しに使わない。製品と同じ書類の path の選び方（`MirrorTarget::find` の want）。
fn textedit_window(doc: &std::path::Path) -> (String, String) {
    let title = doc.file_name().and_then(|n| n.to_str()).expect("file の path");
    let helper = std::path::PathBuf::from(env("KANGAE_MIRROR_HELPER"));
    let t = surface::mirror::MirrorTarget::find(&helper, "com.apple.TextEdit", Some(title), Some(doc), false)
        .unwrap_or_else(|e| panic!("未測: TextEdit に {title} の窓が無い（open -g -n -a TextEdit {}）: {e:?}", doc.display()));
    (t.pid.to_string(), t.window_id.to_string())
}

/// E1 の手と読み戻しと判定（下の 2 本が共通に使う）。`send` が人の手の代わりに入力 1 つを Shibubu の道へ渡す（文字 1 つと ⌘S の 2 回）。
/// 読み戻しは操作と別の道: TextEdit の AX value（helper の ax-text）と disk。合格: 値の変化（AX に文字・disk に文字）ごとに、その前に relay の記録
/// （`e1_path`）・relay の道が文字 = post_to_pid と ⌘S = save_document・前面が同じ。負の対照（同じ run の中）: 試験の側から TextEdit に直接 postToPid
/// → 値は変わるが relay の記録が無い → `e1_path` が赤になる事を見る。返すのは証拠の 1 行。
fn e1_run(doc: &std::path::Path, (te_pid, te_wid): (&str, &str), relay_log: &std::path::Path, nonce: &str, mut send: impl FnMut(surface::mirror::MirrorInput)) -> String {
    use surface::mirror::{KeyMod, MirrorInput};
    let title = doc.file_name().and_then(|n| n.to_str()).expect("W16M2_TEXTEDIT_DOC は file の path").to_string();
    let (typed, direct) = (format!("{nonce}m"), format!("{nonce}d"));
    let helper = std::path::PathBuf::from(env("KANGAE_MIRROR_HELPER"));
    let ax = || helper_json(&helper, &["ax-text", "--pid", te_pid, "--title", &title]).unwrap_or_else(|e| panic!("未測: {e:?}"))["text"].as_str().unwrap_or("").to_string();
    let disk = || std::fs::read_to_string(doc).unwrap_or_default();
    let front0 = front();
    assert!(!ax().contains(nonce) && !disk().contains(nonce), "前の run の文字が残っている（書類を作り直す）");

    // 1. Mirror の上で打つ → TextEdit の AX value が変わる（disk はまだ: 保存の変化を ⌘S に数えるため・自動保存は 4.5 秒は来ないと実測）
    send(MirrorInput::Text { text: typed.clone() });
    let t_ax = wait_until(&format!("TextEdit の AX value に {typed} が出ない（Mirror の入力が relay されていない）"), || ax().contains(&typed));
    assert!(!disk().contains(&typed), "⌘S の前に disk に {typed}（自動保存）。保存の変化を ⌘S に数えられない");
    // 2. Mirror の上で ⌘S → disk に書かれる（Apple Event の save・TextEdit は前に出ない）
    send(MirrorInput::Key { key: "s".into(), mods: vec![KeyMod::Cmd] });
    let t_disk = wait_until(&format!("disk に {typed} が書かれない（⌘S が保存に届いていない）"), || disk().contains(&typed));
    let log = std::fs::read_to_string(relay_log).unwrap_or_else(|e| panic!("未測: relay の記録 {}: {e}", relay_log.display()));
    let relays = relay_facts(&log).unwrap_or_else(|e| panic!("{e}"));
    let changes = vec![
        ValueChange { at_ms: t_ax, what: format!("TextEdit の AX value に {typed}") },
        ValueChange { at_ms: t_disk, what: format!("{title} の disk に {typed}") },
    ];
    let n = e1_path(&relays, &changes).unwrap_or_else(|e| panic!("E1 が赤: {e}\nrelay: {relays:?}"));
    let routes: Vec<String> = log.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["route"].to_string()).collect();
    assert!(routes.contains(&"\"post_to_pid\"".to_string()) && routes.contains(&"\"save_document\"".to_string()), "relay の道が違う: {routes:?}");
    assert_eq!(front(), front0, "前面の app が変わった");

    // 3. 負の対照: 試験の側から TextEdit に直接 postToPid（Shibubu を通らない）→ 値は変わるが relay の記録が無いので E1 が赤
    let r = helper_json(&helper, &["input", "--window", te_wid, "--pid", te_pid, &serde_json::json!({"op": "text", "text": direct, "origin": "automated"}).to_string()]).unwrap_or_else(|e| panic!("未測: {e:?}"));
    assert_eq!(r["kind"], "sent", "試験の側の postToPid が送れない: {r}");
    let t_direct = wait_until(&format!("負の対照: 直接 postToPid の {direct} が TextEdit に出ない"), || ax().contains(&direct));
    let relays_after = relay_facts(&std::fs::read_to_string(relay_log).unwrap()).unwrap();
    let mut all = changes.clone();
    all.push(ValueChange { at_ms: t_direct, what: format!("TextEdit の AX value に {direct}（直接 postToPid）") });
    let red = e1_path(&relays_after, &all).expect_err("負の対照: relay の無い変化を E1 に数えた");
    assert_eq!(front(), front0, "前面の app が変わった");
    format!("E1 緑: 変化 {n} 件（AX {t_ax:.0} ms・disk {t_disk:.0} ms・relay {} 件 {routes:?}）・負の対照は赤: {red}", relays.len())
}

/// W16M-2 の受け入れ E1（YC_WAVES W16M-2）: Mirror の上で打った文字と ⌘S が、背景の Shibubu（egui の Mirror の矩形 → `mirror_inputs` → relay）を通って
/// 背景の TextEdit の値と disk を変える。人の手の代わりに試験の側が、背景の Shibubu の pid へ postToPid で key を送る（helper の自動の口・前面と cursor を
/// 変えない）。Mirror の矩形の keyboard の focus は mirror-e1.sh が AX の AXPress（`oracle/ax.swift press`・hit test と identifier の一致）で先に取ってある。
/// 背景の Shibubu が pid 宛ての key を受けるのは、`SHIBUBU_BACKGROUND=1` の窓が app の中の key の窓だから（main.rs `order_back_and_make_key`）。
#[test]
#[ignore = "実機: scripts/qa/bg/mirror-e1.sh が背景の Shibubu の Mirror を focus にして環境変数で渡す"]
fn w16m2_e1_typing_and_cmd_s_on_the_mirror_reach_textedit() {
    let (swid, spid) = (env("W16M2_SHIBUBU_WID"), env("W16M2_SHIBUBU_PID"));
    let doc = std::path::PathBuf::from(env("W16M2_TEXTEDIT_DOC"));
    let relay_log = std::path::PathBuf::from(env(surface::mirror::RELAY_LOG_ENV));
    // needle（mirror-e1.sh の front-guard が owner の前面の欄に漏れていないかを見る語）
    let nonce = env("W16M2_NONCE");
    // TextEdit の窓は mirror-e1.sh が Shibubu を起こす前に引いて渡す（Shibubu の stream が動いた後に list を走らせない）
    let te = (env("W16M2_TEXTEDIT_PID"), env("W16M2_TEXTEDIT_WID"));
    let helper = std::path::PathBuf::from(env("KANGAE_MIRROR_HELPER"));
    let out = e1_run(&doc, (&te.0, &te.1), &relay_log, &nonce, |input| {
        let mut line = serde_json::to_value(&input).unwrap();
        line["origin"] = serde_json::json!("automated");
        let r = helper_json(&helper, &["input", "--window", &swid, "--pid", &spid, &line.to_string()]).unwrap_or_else(|e| panic!("未測: {e:?}"));
        assert_eq!(r["kind"], "sent", "試験の側の Shibubu への postToPid が送れない: {r}");
    });
    println!("{out}");
}

/// W16M-2 の E1 の下の段（GUI を通さない）: 製品の `MirrorSurface::relay_human`（同意・本物の helper の stream の人の口・relay の記録）から
/// 背景の TextEdit の値と disk まで。egui の focus と key（上の 1 本）が測れない時の切り分けと、人の口より下の回帰。
/// 例: `KANGAE_MIRROR_HELPER=<helper> W16M2_TEXTEDIT_DOC=<open -g で開いた書類> cargo test --features mirror --test mirror_e0 w16m2_e1_relay_human -- --ignored`
#[test]
#[ignore = "実機: TextEdit が W16M2_TEXTEDIT_DOC を開いている（open -g）・端末に画面収録・Accessibility・TextEdit のオートメーションの許可"]
fn w16m2_e1_relay_human_reaches_textedit() {
    use surface::mirror::{ConsentGate, MirrorSurface, MirrorTarget, RelayLog};
    let helper = std::path::PathBuf::from(env("KANGAE_MIRROR_HELPER"));
    let doc = std::path::PathBuf::from(env("W16M2_TEXTEDIT_DOC"));
    let title = doc.file_name().and_then(|n| n.to_str()).expect("file の path").to_string();
    let te = textedit_window(&doc);
    // M4 ② の負の対照: W16M3_FIND_TITLE_ONLY=1 で製品の側の窓の選び方だけを題名だけ（M2 までの形）に戻す（読み戻しは書類の窓のまま）→ 罠の窓を選べば段 ① で赤
    let want = std::env::var_os("W16M3_FIND_TITLE_ONLY").is_none().then_some(doc.as_path());
    let target = MirrorTarget::find(&helper, "com.apple.TextEdit", Some(&title), want, false).unwrap_or_else(|e| panic!("未測: {e:?}"));
    println!("E1 の段 ①: 選んだ窓 pid {} 窓 {}（書類の窓 = {te:?}）", target.pid, target.window_id);
    let (grant, _) = ConsentGate::default().grant(&target, now_ms());
    let relay_log = std::env::temp_dir().join(format!("w16m2-e1-relay-human-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&relay_log);
    let mut m = MirrorSurface::open(&helper, target, grant, RelayLog::open(&relay_log).unwrap()).unwrap_or_else(|e| panic!("未測: {e:?}"));
    // helper は stream が始まってから stdin を読む（混んだ機械では 30 秒まで・KangaeMirror.swift の runStream）。始まる前の時間を E1 の待ちに数えない
    let mut started = false;
    for _ in 0..400 {
        while let Some(ev) = m.take_event() {
            started |= ev["kind"] == "started";
        }
        m.stream().unwrap_or_else(|e| panic!("未測: {e:?}"));
        if started {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(started, "未測: helper の stream が 40 秒で始まらない");
    let mut lease = surface::ControlLease::human(now_ms());
    let nonce: String = format!("w16m2b{:x}", now_ms() as u64);
    // M4（E1 の切り分け）: 送った行の helper の答えを待ってから値を読む。断られたら、値の 10 秒の待ちではなく断った段の原文で赤にする
    // （M2 は relay の記録が在るのに値が変わらず、helper の答えが残っていなかった）。Ok = sent の答え・Err = 拒否の答え（どちらも helper の原文）
    let answer = |m: &mut MirrorSurface, seq: u64| -> Result<serde_json::Value, serde_json::Value> {
        for _ in 0..100 {
            while let Some(ev) = m.take_event() {
                if ev["id"] == seq {
                    return if m.refusal(&ev).is_some() { Err(ev) } else { Ok(ev) };
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("未測: helper が relay seq {seq} に 10 秒答えない");
    };
    let out = e1_run(&doc, (&te.0, &te.1), &relay_log, &nonce, |input| {
        let what = serde_json::to_string(&input).unwrap();
        let rec = m.relay_human(&mut lease, input).unwrap_or_else(|e| panic!("relay_human が断った: {e}"));
        assert_eq!((rec.pid.to_string(), rec.window_id.to_string()), te, "E1 の段 ①: relay の宛先が書類の窓ではない");
        match answer(&mut m, rec.seq) {
            Ok(ev) => println!("E1 の段 ②: relay seq {} {what} → helper {ev}", rec.seq),
            Err(ev) => panic!("E1 の段 ②: helper が relay seq {} の {what} を断った: {ev}", rec.seq),
        }
    });
    // helper の答えに拒否が無い（sent だけ）
    while let Some(ev) = m.take_event() {
        assert_eq!(m.refusal(&ev), None, "helper が拒否した: {ev}");
    }
    println!("{out}");

    // 押す（AX の点）: 3 文字目の点（helper の ax-bounds・窓の中の割合）を 1 回 click → TextEdit の選択の位置が 3 に（読みは試験の側の `ax sel`）。
    // その変化の前に、その変化だけの relay の記録が在る（e1_path・前の変化 = 上の負の対照より後）
    let sel_tool = readback::oracle_tool("ax").unwrap_or_else(|e| panic!("{e}"));
    let sel = || String::from_utf8_lossy(&Command::new(&sel_tool).args(["sel", te.0.as_str(), title.as_str()]).output().expect("ax sel").stdout).trim().to_string();
    assert_ne!(sel(), "3 0", "click の前から選択が 3（変化を測れない）");
    let r = helper_json(&helper, &["ax-bounds", "--pid", &te.0, "--window", &te.1, "--loc", "3", "--len", "1", "--title", &title]).unwrap_or_else(|e| panic!("未測: {e:?}"))["rect"].clone();
    let num = |k: &str| r[k].as_f64().unwrap_or_else(|| panic!("未測: 3 文字目の矩形が無い: {r}"));
    let (x, y) = (num("x") + num("w") / 2.0, num("y") + num("h") / 2.0);
    let before = now_ms();
    let rec = m.relay_human(&mut lease, surface::mirror::MirrorInput::Click { x, y, count: 1 }).unwrap_or_else(|e| panic!("relay_human が click を断った: {e}"));
    assert_eq!(rec.route, surface::mirror::InputRoute::AxAtPoint);
    let t_sel = wait_until("click の後も TextEdit の選択が 3 にならない（AX の点の道が届いていない）", || sel() == "3 0");
    let clicks: Vec<_> = relay_facts(&std::fs::read_to_string(&relay_log).unwrap()).unwrap().into_iter().filter(|f| f.at_ms > before).collect();
    let n = e1_path(&clicks, &[ValueChange { at_ms: t_sel, what: "TextEdit の選択が 3 文字目".into() }]).unwrap_or_else(|e| panic!("E1（click）が赤: {e}"));
    while let Some(ev) = m.take_event() {
        assert_eq!(m.refusal(&ev), None, "helper が click を拒否した: {ev}");
    }
    println!("E1（click）緑: 変化 {n} 件（選択 3・{t_sel:.0} ms・relay seq {}）", rec.seq);
}

/// 反証の直し（W16M-2）: 同じ app の別の窓が映している窓と同じ位置と大きさ（zoom した 2 枚・全画面・tab）でも、Mirror の上の入力はその別の書類に入らない。
/// 試験の側が双子の書類 B を書類 A の窓と同じ位置と大きさにして app の中の main にし（`oracle/ax.swift frame`・activate しない）、A を映す Mirror の人の口へ
/// ① B が前に在る点への click → B の選択の位置は変わらず `NeedsFront`（前は B の文字の欄に選択の位置を置いた）
/// ② 文字 → A の AX value に入り B には入らない（前は B に入った＝2026-10-21 に helper だけで再現）。
/// 例: `KANGAE_MIRROR_HELPER=<helper> W16M2_TEXTEDIT_DOC=<A> W16M2_TEXTEDIT_TWIN=<B> cargo test --features mirror --test mirror_e0 w16m2_same_frame -- --ignored`
/// （A と B は試験のために作った別の書類を `open -g -a TextEdit` で開く・run ごとに作り直す）
#[test]
#[ignore = "実機: TextEdit が W16M2_TEXTEDIT_DOC と W16M2_TEXTEDIT_TWIN を開いている（open -g）・端末に画面収録・Accessibility の許可"]
fn w16m2_same_frame_twin_window_never_gets_the_mirror_input() {
    use surface::mirror::{ConsentGate, MirrorError, MirrorInput, MirrorSurface, MirrorTarget, RelayLog};
    let helper = std::path::PathBuf::from(env("KANGAE_MIRROR_HELPER"));
    let (doc, twin) = (std::path::PathBuf::from(env("W16M2_TEXTEDIT_DOC")), std::path::PathBuf::from(env("W16M2_TEXTEDIT_TWIN")));
    let name = |p: &std::path::Path| p.file_name().and_then(|n| n.to_str()).expect("file の path").to_string();
    let (a, b) = (name(&doc), name(&twin));
    let win = helper_json(&helper, &["list", "--all"]).unwrap_or_else(|e| panic!("未測: {e:?}")).as_array().into_iter().flatten().find(|w| w["bundle_id"] == "com.apple.TextEdit" && w["title"] == a.as_str()).cloned()
        .unwrap_or_else(|| panic!("未測: TextEdit に {a} の窓が無い（open -g -a TextEdit {}）", doc.display()));
    let pid = win["pid"].to_string();
    let f = &win["frame"];
    let frame: Vec<String> = ["x", "y", "w", "h"].iter().map(|k| f[*k].to_string()).collect();
    let tool = readback::oracle_tool("ax").unwrap_or_else(|e| panic!("{e}"));
    let o = Command::new(tool).args(["frame", pid.as_str(), b.as_str()]).args(&frame).output().expect("ax frame");
    assert!(o.status.success(), "未測: {b} を {a} と同じ位置と大きさにできない: {}", String::from_utf8_lossy(&o.stderr));
    let ax = |t: &str| helper_json(&helper, &["ax-text", "--pid", &pid, "--title", t]).unwrap_or_else(|e| panic!("未測: {e:?}"))["text"].as_str().unwrap_or("").to_string();
    let sel_tool = readback::oracle_tool("ax").unwrap_or_else(|e| panic!("{e}"));
    let sel = |t: &str| String::from_utf8_lossy(&Command::new(&sel_tool).args(["sel", pid.as_str(), t]).output().expect("ax sel").stdout).trim().to_string();

    let target = MirrorTarget::find(&helper, "com.apple.TextEdit", Some(&a), Some(&doc), false).unwrap_or_else(|e| panic!("未測: {e:?}"));
    let (grant, _) = ConsentGate::default().grant(&target, now_ms());
    let relay_log = std::env::temp_dir().join(format!("w16m2-twin-relay-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&relay_log);
    let mut m = MirrorSurface::open(&helper, target, grant, RelayLog::open(&relay_log).unwrap()).unwrap_or_else(|e| panic!("未測: {e:?}"));
    let mut started = false;
    for _ in 0..400 {
        while let Some(ev) = m.take_event() {
            started |= ev["kind"] == "started";
        }
        m.stream().unwrap_or_else(|e| panic!("未測: {e:?}"));
        if started {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(started, "未測: helper の stream が 40 秒で始まらない");
    // relay の記録の seq（= helper の答えの id）の答えを 10 秒待つ: Ok(()) = sent・Err = 拒否の 1 行
    let answer = |m: &mut MirrorSurface, seq: u64| -> Result<(), MirrorError> {
        for _ in 0..100 {
            while let Some(ev) = m.take_event() {
                if ev["id"] == seq {
                    return m.refusal(&ev).map_or(Ok(()), Err);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("未測: helper が relay seq {seq} に 10 秒答えない");
    };
    let mut lease = surface::ControlLease::human(now_ms());
    let front0 = front();
    let mut red = Vec::new();

    // ① B が前に在る点への click（A の窓の中央）
    let sel_b = sel(&b);
    let rec = m.relay_human(&mut lease, MirrorInput::Click { x: 0.5, y: 0.5, count: 1 }).unwrap_or_else(|e| panic!("relay_human が click を断った: {e}"));
    match answer(&mut m, rec.seq) {
        Err(MirrorError::NeedsFront { .. }) => {}
        other => red.push(format!("click の答えが NeedsFront でない: {other:?}")),
    }
    std::thread::sleep(Duration::from_millis(500));
    if sel(&b) != sel_b {
        red.push(format!("click が双子の {b} の選択を {sel_b} → {} に動かした", sel(&b)));
    }

    // ② 文字
    let nonce = format!("w16m2tw{:x}", now_ms() as u64);
    let rec = m.relay_human(&mut lease, MirrorInput::Text { text: nonce.clone() }).unwrap_or_else(|e| panic!("relay_human が文字を断った: {e}"));
    if let Err(e) = answer(&mut m, rec.seq) {
        red.push(format!("文字が断られた（A は 1 つに決まるはず）: {e}"));
    }
    let mut landed = false;
    for _ in 0..50 {
        if ax(&a).contains(&nonce) {
            landed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !landed {
        red.push(format!("文字 {nonce} が映している {a} に入らない"));
    }
    if ax(&b).contains(&nonce) {
        red.push(format!("文字 {nonce} が双子の {b} に入った"));
    }
    if front() != front0 {
        red.push("前面の app が変わった".into());
    }
    assert!(red.is_empty(), "赤:\n{}", red.join("\n"));
    println!("緑: 双子の {b}（{a} と同じ位置と大きさ・前）への click は NeedsFront で選択は {sel_b} のまま・文字 {nonce} は {a} にだけ入った・前面は同じ");
}

/// holdout（W16M-3 の不具合 ②・実機・実装の commit で通す）: Mirror の映像の本文のある文字を 1 回 click すると、本物の TextEdit の選択の位置がその文字に来る
/// （owner の前面の手と Grok の手で「1 ng」）。道を 3 つに切り、どの段で赤かを panic の文に書く:
/// (1) 写し = 本物の helper の frame の `meta`（Retina・`--max-width` 960 の縮小・余白）で、映像のその文字の点 → `fit` → `mirror_inputs` → 窓の中の割合が、
///     AX の `ax-bounds` のその文字の矩形に入る。content × factor が frame の中に収まる事も見る（SCK の contentRect は surface の point。
///     2026-10-23 の probe: 661×433 pt の窓で frame 960×628・content × factor = 958.7×628）
/// (2) AX の点 = その割合の click を人の口（`relay_human`）で送ると、helper の答えが `sent` で `ax: select`・`loc` がその文字
///     （`needs_front` なら、その点の要素が映している窓の物でない＝他の窓に覆われた点の hit test の疑い）
/// (3) 読み戻し = 試験の側の `ax sel` の選択の位置がその文字。背景の NSTextView は caret を描かないので、映像の caret では判定しない。
/// (4) 見える = helper の答えの `caret`（Shibubu が映像の上に描く）がその文字の左の辺。2026-10-23 の切り分けで (1)〜(3) は緑・(4) が無かった
///     （`docs/evidence/W16M-3/click-diagnosis.txt`）＝ owner の「cursor が動かない」の正体。
/// 例: `KANGAE_MIRROR_HELPER=<helper> W16M3_TEXTEDIT_DOC=<自分の run の dir に作って open -g -n -a TextEdit で開いた書類〈2 行以上〉> cargo test --features mirror --test mirror_e0 w16m3_click -- --ignored`
#[test]
#[ignore = "実機: TextEdit が W16M3_TEXTEDIT_DOC を開いている（open -g -n）・端末に画面収録・Accessibility の許可"]
fn w16m3_click_on_the_mirror_image_moves_the_textedit_selection_to_that_character() {
    use eframe::egui::{pos2, Event, Modifiers, PointerButton, Rect, Vec2};
    use surface::mirror::{ConsentGate, InputRoute, MirrorInput, MirrorSurface, MirrorTarget, RelayLog};
    use ui::mirror_view::{fit, mirror_inputs};
    let helper = std::path::PathBuf::from(env("KANGAE_MIRROR_HELPER"));
    let doc = std::path::PathBuf::from(env("W16M3_TEXTEDIT_DOC"));
    let title = doc.file_name().and_then(|n| n.to_str()).expect("file の path").to_string();
    let te = textedit_window(&doc);
    let target = MirrorTarget::find(&helper, "com.apple.TextEdit", Some(&title), Some(&doc), false).unwrap_or_else(|e| panic!("未測: {e:?}"));
    let (grant, _) = ConsentGate::default().grant(&target, now_ms());
    let relay_log = std::env::temp_dir().join(format!("w16m3-click-relay-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&relay_log);
    let mut m = MirrorSurface::open(&helper, target, grant, RelayLog::open(&relay_log).unwrap()).unwrap_or_else(|e| panic!("未測: {e:?}"));
    let (mut started, mut meta) = (false, None);
    for _ in 0..400 {
        while let Some(ev) = m.take_event() {
            started |= ev["kind"] == "started";
        }
        if let Some(f) = m.stream().unwrap_or_else(|e| panic!("未測: {e:?}")) {
            meta = Some(f.meta);
        }
        if started && meta.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let meta = meta.unwrap_or_else(|| panic!("未測: helper の stream の frame が 40 秒で来ない"));

    // (1) 写し
    let [cx, cy, cw, ch] = meta.content;
    assert!(cw > 0.0 && ch > 0.0, "(1) 写しが赤: frame の contentRect が 0（{meta:?}）");
    let (cx, cy, cw, ch) = (cx * meta.factor, cy * meta.factor, cw * meta.factor, ch * meta.factor);
    assert!(cx + cw <= meta.width as f64 + 2.0 && cy + ch <= meta.height as f64 + 2.0, "(1) 写しが赤: content × factor が frame の外（contentRect が surface の point でない）: {meta:?}");
    let loc = 5;
    let r = helper_json(&helper, &["ax-bounds", "--pid", &te.0, "--window", &te.1, "--loc", &loc.to_string(), "--len", "1", "--title", &title]).unwrap_or_else(|e| panic!("未測: {e:?}"))["rect"].clone();
    let num = |k: &str| r[k].as_f64().unwrap_or_else(|| panic!("未測: {loc} 文字目の矩形が無い: {r}"));
    let (rx, ry, rw, rh) = (num("x"), num("y"), num("w"), num("h"));
    // 文字の左 1/4・縦の中（AXRangeForPosition がその文字の前を返す側）。描く側は frame を `fit` の範囲に uv 0〜1 で貼る
    let (fx, fy) = (rx + rw / 4.0, ry + rh / 2.0);
    let rect = Rect::from_min_size(pos2(40.0, 60.0), Vec2::new(640.0, 420.0));
    let shown = fit(rect, &meta);
    let (px, py) = (cx + fx * cw, cy + fy * ch);
    let p = pos2(shown.min.x + (px / meta.width as f64) as f32 * shown.width(), shown.min.y + (py / meta.height as f64) as f32 * shown.height());
    let press = |pressed: bool| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let inputs = mirror_inputs(&[press(true), press(false)], shown, &meta, false);
    let [MirrorInput::Click { x, y, count: 1 }] = inputs.as_slice() else { panic!("(1) 写しが赤: 映像の点の click が 1 つの Click にならない: {inputs:?}") };
    assert!((rx..=rx + rw).contains(x) && (ry..=ry + rh).contains(y), "(1) 写しが赤: 映像の {loc} 文字目の点が窓の ({x:.4}, {y:.4}) に写った（AX の矩形 {r}・{meta:?}）");

    // (2) AX の点
    let sel_tool = readback::oracle_tool("ax").unwrap_or_else(|e| panic!("{e}"));
    let sel = || String::from_utf8_lossy(&Command::new(&sel_tool).args(["sel", te.0.as_str(), title.as_str()]).output().expect("ax sel").stdout).trim().to_string();
    assert_ne!(sel(), format!("{loc} 0"), "click の前から選択が {loc}（変化を測れない・書類の先頭に cursor を置き直す）");
    let mut lease = surface::ControlLease::human(now_ms());
    let rec = m.relay_human(&mut lease, MirrorInput::Click { x: *x, y: *y, count: 1 }).unwrap_or_else(|e| panic!("(2) AX の点が赤: relay_human が click を断った: {e}"));
    assert_eq!(rec.route, InputRoute::AxAtPoint);
    let mut ans = serde_json::Value::Null;
    for _ in 0..100 {
        if let Some(ev) = std::iter::from_fn(|| m.take_event()).find(|ev| ev["id"] == serde_json::json!(rec.seq)) {
            ans = ev;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ans["kind"] == "sent" && ans["ax"] == "select" && ans["loc"] == serde_json::json!(loc), "(2) AX の点が赤: helper の答え {ans}（{x:.4}, {y:.4}）");

    // (3) 読み戻し
    let t_sel = wait_until(&format!("(3) 読み戻しが赤: click の後も TextEdit の選択が {loc} にならない（helper は {ans}）"), || sel() == format!("{loc} 0"));
    // (4) 見える: 背景の NSTextView は caret を描かない（docs/evidence/W16M-3/click-diagnosis.txt）。helper の答えの caret（Shibubu が映像に描く）が
    //     その文字の左の辺・同じ行にある
    let c = |k: &str| ans["caret"][k].as_f64().unwrap_or_else(|| panic!("(4) 見えるが赤: helper の答えに caret が無い（映像に何も描けない）: {ans}"));
    assert!((c("x") - rx).abs() <= rw / 2.0 && (c("y") - ry).abs() <= rh / 2.0, "(4) 見えるが赤: caret ({:.4}, {:.4}) が {loc} 文字目の左の辺 ({rx:.4}, {ry:.4}) に無い: {ans}", c("x"), c("y"));
    println!("緑: 映像の {loc} 文字目の点 → 窓の ({x:.4}, {y:.4}) → helper {ans} → 選択 {loc}（{t_sel:.0} ms）・frame {}×{} content×factor {cw:.1}×{ch:.1}", meta.width, meta.height);
}
