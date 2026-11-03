//! YC MVP Wave 0 の通し（YCM-005・owner 2026-10-19 docs/YC_DECISIONS.md）: **CuaDriver なし**で 一言 → 生成 → 操作 → 保存 → 開き直して復元。
//! 画面は使わない（`Generated` の口を直接押す）。試験であって実機確認ではない。
//!
//! CuaDriver に依らない事を 4 重に固定する:
//! 1. 製品に cua の module は無い（YC W18-1 で `src/cua.rs` を消した）。この crate はデモ経路（generated・composer・runtime・storage・ui）だけを取り込み、cua を参照すると compile が落ちる。
//! 2. 不在の通し（owner 3 通目の 4）: PATH は保ち（PTY の shell が使う `sh` などは起こせる）、`cua-driver` の在る dir だけを PATH から外し、
//!    `CUA_DRIVER_BIN=""` にして（W18-1 の後の製品はこの変数を読まない・罠として置く）、`cua-driver` が起こせない事を assert してから通す。
//! 3. 罠の通し: `CUA_DRIVER_BIN` と PATH の先頭を「起こされたら印の file を作る script」にして通し、印が無い事を見る。
//! 4. 絶対 path・設定の経路: `/Applications/CuaDriver.app` は試験から罠に替えられない（外の場所を書き換えない）ので、デモ経路の source に
//!    cua を起こす手がかり（`CuaDriver.app`・`cua-driver` の実行・`CUA_DRIVER_BIN`・`cua::`）が comment の外に 1 つも無い事を見る（`demo_sources_name_no_cua_route`）。
//!
//! 注: この binary は取り込んだ module の unit test（generated・composer・ui…）も並行で回す。不在の通しの間は PATH から cua-driver の dir が
//! 抜けるだけなので、他の test が PATH で起こす command（sh・git 等）は揺れない。
//!
//! W2 から `demo()` の後半で Project の Page の段（＋Page で 3 枚・移って戻ると入力途中の文字が Page ごとに戻る・開き直して 3 枚）も通す。
//! W3 から一言から複数 Page の Project（`project_stage`・I1）も `demo()` の終わりで通す。
//! W4 から `demo()` の最後で Project の Terminal の段（本物の PTY・nonce の印・閉じると shell 0 本・開き直すと記録が Exited）も通す。
//! W9 から `demo()` の最後で Terminal の中の `shibubu project pages|surfaces|context --json`（`CARGO_BIN_EXE_shibubu`）を、試験の側の数（画面の AX の木・`$$`・`ps`）と照らす。
//! まだ無い段は偽物で埋めず、段ごとに `#[ignore]` の test にする（yc/pty と yc/qa の統合の時点で `#[ignore]` の段は 0）。
//! `cargo test --test yc_demo_e2e -- --ignored` で回すと、その段が出来るまで赤い。
#![allow(dead_code, unused_imports)]
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
#[allow(dead_code)] // ui::portal_frame が PortalState / SyncMode を再輸出する（W2c）
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
#[path = "../src/terminal/mod.rs"]
mod terminal;
// W13: generated.rs が `with_llm` で `llm::Llm` を受ける（E2E は渡さない＝model 無し・quota を使わない。`llm_cli` は取り込まない）
#[path = "../src/llm/mod.rs"]
mod llm;
#[path = "../src/generated.rs"]
mod generated;
// G88b: generated の take の帯が録音（`audio::record`）と REAPER への insert（`audio::reaper`）を呼ぶ
#[path = "../src/audio/mod.rs"]
mod audio;
// `mod cua;` は意図して置かない（上の 1）

use domain::action::{ActionInvocation, ActionReadiness, InvocationOrigin};
use domain::run::Outcome;
use generated::Generated;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use ui::renderer::ActionHost;
use ui::shell::View;

// env（PATH・CUA_DRIVER_BIN・SHELL）は process 全体の物なので、この file の test は 1 つずつ。generated.rs の試験（g90c）と同じ lock（`TEST_ENV` の doc）
use generated::tests::TEST_ENV as ENV;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("shibubu-yc-e2e-{tag}-{}", storage::new_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 一言 → 生成 → 操作（足す・状態を変える）→ 保存 → 開き直して復元。CuaDriver を一度も要らない事は呼ぶ側が env で強制する。
fn demo(dir: &Path) {
    let data = dir.join("data");
    // 一言 → 生成（composer → 検査 → runtime.open → SQLite の apps / app_revisions）
    let mut g = Generated::open(&data);
    assert!(g.note.is_none(), "保存できる DB で開く: {:?}", g.note);
    let id = g.create("読書記録をつけたい").expect("一言から app を作る");
    let row = g.rows().into_iter().find(|r| r.view == View::App(id.clone())).expect("Apps に載る");
    assert!(!row.status.contains("not set up"), "全部の操作が使える: {}", row.status);
    let (add, update, source) = {
        let (def, _, mut h) = g.open_app(&id).expect("開いた app");
        let add = def.actions.iter().find(|a| a.id.ends_with(".add")).expect("足す操作").id.clone();
        let update = def.actions.iter().find(|a| a.id.ends_with(".update_status")).expect("状態を変える操作").id.clone();
        let source = add.rsplit_once('.').unwrap().0.to_owned();
        assert_eq!(h.readiness(&add), None, "押せる（Unsupported / RequiresConnection でない）");
        // 操作: 1 冊足す → Verified（画面に何も足さない）・中身は ui_state
        h.invoke(ActionInvocation { action_ref: add.clone(), origin: InvocationOrigin::Button { node_key: "books-add".into() }, input: json!({ "title": "坊っちゃん", "author": "夏目漱石" }) });
        assert!(h.line.is_none(), "足すのが Verified: {:?}", h.line);
        let item = h.rt.ui_state(&source).unwrap()["items"][0]["id"].as_str().expect("足した 1 件").to_owned();
        // 操作: 状態を変える（読んでいる → 読んだ）→ Verified
        h.invoke(ActionInvocation { action_ref: update.clone(), origin: InvocationOrigin::Button { node_key: "books-status".into() }, input: json!({ "item_id": item, "status": "Read" }) });
        assert!(h.line.is_none(), "状態の変更が Verified: {:?}", h.line);
        (add, update, source)
    };
    let runs = g.run_entries();
    for a in [&add, &update] {
        let e = runs.iter().find(|e| &e.what == a).unwrap_or_else(|| panic!("Runs に {a} が無い"));
        assert!(matches!(e.outcome(), Some(Outcome::Verified { .. })), "{a}: {:?}", e.outcome());
    }
    // 保存 → 閉じる → 開き直す（同じ dir の SQLite）
    drop(g);
    let mut g = Generated::open(&data);
    assert!(g.note.is_none(), "開き直しで壊れた行が無い: {:?}", g.note);
    assert_eq!(g.rows().len(), 1, "再起動で app が残る");
    let (_, _, h) = g.open_app(&id).expect("開き直した app");
    let doc = h.rt.ui_state(&source).unwrap();
    assert_eq!(doc["items"][0]["title"], json!("坊っちゃん"));
    assert_eq!(doc["items"][0]["status"], json!("Read"), "変えた状態も残る");
    assert_eq!(h.readiness(&add), None, "開き直しても操作は使える（定義から導き直す）");
    assert_eq!(g.run_entries().len(), 2, "run の記録も残る");
    pages(&data, &id, g);
    // W3: 一言から複数 Page の Project（同じ env の下 = 不在と罠の両方で通す）
    project_stage(&dir.join("w3"));
    // W8: Undo・Redo・Terminal の引き出し・再起動（同じ env の下）
    undo_stage(&dir.join("w8"));
    terminal(dir, &data, &id);
    page_terminal(&dir.join("w5"), &std::env::var("PATH").unwrap());
    // W9: Terminal の中の `shibubu project …`（本物の bin・同じ env の下 = 不在と罠の両方で通す）
    project_cli_stage(&dir.join("w9"), &std::env::var("PATH").unwrap());
}

/// W8 の段（A3 の DB の側）: 主デモの一言 → 「Tests Page を足して」→ Undo → Redo → Undo → 再起動 → Redo（G92 で引き出しの開閉を消した）。
/// 判定は操作の口と別の道（Store を直に開いて projects の行・project_revisions を読む）。
/// 未測: Terminal の外の効果（`undo-probe.txt` の sha）。
fn undo_stage(dir: &Path) {
    use ui::shell::PageIntent;
    let data = dir.join("data");
    let said = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw.";
    let mut g = Generated::open(&data);
    let View::Project { project, page } = g.create_project(said, None, None).unwrap().unwrap() else { panic!("Project の View ではない") };
    g.open_project(&project, Some(&page)).unwrap();
    for (what, intent) in [
        ("Tests の Page を足す", PageIntent::Edit { text: "Tests Page を足して".into() }),
        ("元に戻す", PageIntent::Undo),
        ("やり直す", PageIntent::Redo),
        ("もう一度元に戻す", PageIntent::Undo),
    ] {
        g.page_intent(&project, intent).unwrap_or_else(|e| panic!("{what}: {e}"));
    }
    drop(g);
    let read = || {
        let store = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
        let revs: Vec<_> = store.list_project_revisions(&project).unwrap().into_iter().map(|r| (r.revision, r.origin, r.json)).collect();
        let head = store.get(storage::Table::Projects, &project).unwrap().expect("projects の行").json;
        (revs, head)
    };
    let (revs, head) = read();
    assert_eq!(revs.iter().map(|r| (r.0, r.1.as_str())).collect::<Vec<_>>(), [(1, "generate"), (2, "nl_edit")], "Undo / Redo は行を足さない・消さない");
    assert_eq!(head, revs[0].2, "Undo の後の projects の行は編集の前の snapshot と同じ");
    let mut g = Generated::open(&data);
    g.open_project(&project, None).unwrap();
    g.page_intent(&project, PageIntent::Redo).expect("再起動の後も Redo の行き先が在る");
    drop(g);
    let (revs, head) = read();
    assert_eq!((revs.len(), &head), (2, &revs[1].2), "Redo で編集の後の snapshot に戻り、行は増えない");
}

/// W4 の段（画面なし）: Project に本物の PTY の shell を起こし、nonce の印つきで command を回す → Shibubu を閉じると shell は 0 本・zombie 0 →
/// 開き直すと記録は Exited（動いていた記録は Unavailable）。
/// 子の PATH は**この試験が渡した PATH**（不在の通し = cua-driver の dir を外した物・罠の通し = 罠の dir が先頭の物）。製品の既定（`DEFAULT_CHILD_PATH` + 利用者の rc）
/// の性質ではない: 利用者の rc が足す dir は利用者の環境で、製品が cua-driver を起こす事とは別（起こさない事は罠の印で見る）。
fn terminal(dir: &Path, data: &Path, app: &str) {
    // 利用者の rc（~/.bashrc 等）は PATH に dir を足しうる（この Mac の rc は cua-driver の dir を足す）。試験は rc を読まない zsh（`-f`）に
    // 試験の PATH を渡し、その PATH が子に届く事を見る。SHELL は呼ぶ側の ENV の lock の中で替えて戻す。
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], child_path: std::env::var("PATH").unwrap(), ..Default::default() };
    let g = Generated::open(data).with_terminal_config(rc_free.clone());
    let View::Project { project, .. } = g.project_view_of(app).expect("Project") else { panic!() };
    let cwd = dir.canonicalize().unwrap();
    let info = g.open_terminal(&project, cwd.to_str().unwrap()).expect("Project の Terminal を起こす");
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    assert_eq!(info.shell, "/bin/zsh");
    let key = info.key.clone();
    assert_eq!(key.project_id, project, "session は Project の物");
    let ps = |pid: u32, col: &str| String::from_utf8_lossy(&Command::new("/bin/ps").args(["-o", col, "-p", &pid.to_string()]).output().unwrap().stdout).trim().to_owned();
    assert_eq!(ps(info.pid, "ppid="), std::process::id().to_string(), "shell の親が Shibubu");
    assert!(ps(info.pid, "tty=").starts_with("ttys"), "shell が tty を持たない: {}", ps(info.pid, "tty="));
    let t = std::time::Duration::from_secs(20);
    let out = g.terminals().run_marked(&key, "pwd", t).expect("pwd");
    assert_eq!(out.exit_code, 0);
    assert!(out.output.contains(&format!("\r\n{}\r\n", cwd.display())), "{:?}", out.output);
    let out = g.terminals().run_marked(&key, "echo $((6*7))", t).unwrap();
    assert!(out.output.contains("\r\n42\r\n") && out.nonce.len() == 32, "{out:?}");
    let found = g.terminals().run_marked(&key, "command -v cua-driver", t).unwrap();
    match std::env::var("CUA_DRIVER_BIN").as_deref() {
        // 罠の通し: 子は試験の PATH の先頭の罠を見つける（試験の PATH が子に届いている = 罠が PTY にも効く）。起こしていない事は通しの後の印で見る
        Ok(trap) if !trap.is_empty() => assert!(found.exit_code == 0 && found.output.contains(&format!("\r\n{trap}\r\n")), "子の PATH に罠が届かない: {found:?}"),
        // 不在の通し: 試験が cua-driver の dir を外した PATH で、子も cua-driver を見つけない
        _ => assert_ne!(found.exit_code, 0, "子の PATH に cua-driver が居る: {found:?}"),
    }
    assert_eq!(g.terminals().run_marked(&key, "command -v git >/dev/null", t).unwrap().exit_code, 0, "子の PATH に git が無い");
    // Shibubu を閉じる → shell は回収済み（ps に出ない = zombie でもない）
    drop(g);
    assert_eq!(ps(info.pid, "stat="), "", "閉じた後に shell が残る");
    // 前の起動が落ちて Running のまま残った記録を 1 行足しておき、開き直しで Unavailable に読む
    let crashed = terminal::SessionInfo { key: terminal::SessionKey { project_id: project.clone(), session_id: "crashed".into() }, status: terminal::SessionStatus::Running, ..info.clone() };
    let st = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    st.put(storage::Table::TerminalSessions, "crashed", Some(&project), &serde_json::to_value(&crashed).unwrap()).unwrap();
    drop(st);
    let g = Generated::open(data).with_terminal_config(rc_free);
    drop(g);
    let st = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    let rows: Vec<terminal::SessionInfo> = st.list(storage::Table::TerminalSessions, Some(&project)).unwrap().into_iter().map(|r| serde_json::from_value(r.json).unwrap()).collect();
    let status = |id: &str| rows.iter().find(|r| r.key.session_id == id).map(|r| r.status.clone());
    assert!(matches!(status(&key.session_id), Some(terminal::SessionStatus::Exited { .. })), "閉じた Terminal の記録: {rows:?}");
    assert_eq!(status("crashed"), Some(terminal::SessionStatus::Unavailable), "前の起動の Running は Unavailable");
}

/// W2 の段（画面なし）: 古い入口 View::App → 包んだ Project の先頭 Page → ＋Page で 3 枚 → A → B → C → 戻る で入力途中の文字が Page ごと
/// に戻る → 開き直しても 3 枚が同じ順。入力は Opened の AppState に直に書く（egui は回さない・実機は別）。
fn pages(data: &Path, app: &str, mut g: Generated) {
    let typed = |g: &mut Generated, key: &str| {
        let (_, state, _) = g.open_app(app).unwrap();
        state.draft(key, None).and_then(|d| d.get("text").cloned())
    };
    let page = |v: View| match v {
        View::Project { page, .. } => page,
        other => panic!("Project の View ではない: {other:?}"),
    };
    let View::Project { project, page: a } = g.project_view_of(app).expect("app は Project として開ける") else { panic!() };
    assert_eq!(page(g.open_project(&project, None).unwrap()), a);
    g.open_app(app).unwrap().1.draft_mut("a-note", None).insert("text".into(), json!("坊っちゃ"));
    let b = page(g.page_intent(&project, ui::shell::PageIntent::Add).unwrap());
    g.open_app(app).unwrap().1.draft_mut("b-note", None).insert("text".into(), json!("B"));
    let c = page(g.page_intent(&project, ui::shell::PageIntent::Add).unwrap());
    assert_eq!((typed(&mut g, "a-note"), typed(&mut g, "b-note")), (None, None), "C に A・B の文字を出さない");
    assert_eq!(page(g.navigate_page(&project, &a).unwrap()), a);
    assert_eq!((typed(&mut g, "a-note"), typed(&mut g, "b-note")), (Some(json!("坊っちゃ")), None));
    assert_eq!(page(g.navigate_page(&project, &b).unwrap()), b);
    assert_eq!((typed(&mut g, "a-note"), typed(&mut g, "b-note")), (None, Some(json!("B"))));
    assert_eq!(page(g.page_intent(&project, ui::shell::PageIntent::Back).unwrap()), a, "戻る");
    assert_eq!(typed(&mut g, "a-note"), Some(json!("坊っちゃ")));
    drop(g);
    let g = Generated::open(data);
    let (p, _) = g.project_of(app).unwrap().expect("開き直しても Project");
    assert_eq!(p.pages.iter().map(|p| p.id.clone()).collect::<Vec<_>>(), [a, b, c], "再起動しても 3 枚が同じ順");
}

/// CuaDriver が無い Mac: 使わない設定 + PATH に居ない事を確かめてから `f(dir)` を通す。PATH の他の dir（sh・git）は残す。
fn with_cuadriver_absent(tag: &str, f: impl FnOnce(&Path) + std::panic::UnwindSafe) {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch(tag);
    let (path, bin) = (std::env::var_os("PATH"), std::env::var_os("CUA_DRIVER_BIN"));
    let kept: Vec<PathBuf> = std::env::split_paths(&path.clone().unwrap_or_default()).filter(|d| !d.join("cua-driver").exists()).collect();
    std::env::set_var("PATH", std::env::join_paths(&kept).unwrap());
    std::env::set_var("CUA_DRIVER_BIN", "");
    // 不在の確認: PATH から起こせない・CUA_DRIVER_BIN は「使わない」・shell は起こせる（PATH を空にしていない）
    let spawned = Command::new("cua-driver").arg("status").output();
    assert!(matches!(&spawned, Err(e) if e.kind() == std::io::ErrorKind::NotFound), "cua-driver が起こせてしまう: {spawned:?}");
    assert_eq!(std::env::var("CUA_DRIVER_BIN").as_deref(), Ok(""));
    assert!(Command::new("sh").args(["-c", "exit 0"]).status().is_ok_and(|s| s.success()), "PATH から sh が起こせない（PATH を削りすぎ）");
    let r = std::panic::catch_unwind(|| f(&dir));
    match path {
        Some(p) => std::env::set_var("PATH", p),
        None => std::env::remove_var("PATH"),
    }
    match bin {
        Some(b) => std::env::set_var("CUA_DRIVER_BIN", b),
        None => std::env::remove_var("CUA_DRIVER_BIN"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
}

#[test]
fn demo_runs_with_cuadriver_absent() {
    with_cuadriver_absent("absent", demo);
}

/// FX6（FX-030・UIUX G06）: AI 無しで英語の一言 `a reading log` から作ると、押せる button（足す・状態を変える）は押すと DB に 1 行入って読み戻せ、
/// 欄の名前と選択肢の値は英語。試験であって実機確認ではない。
/// 負の対照（commit message）: `pick("status", …)` の選択肢を `"読んだ"` に戻す → 状態を変える assert が赤。
#[test]
fn fx6_a_reading_log_button_writes_one_row_and_reads_it_back() {
    with_cuadriver_absent("fx6", |dir| {
        let mut g = Generated::open(&dir.join("data"));
        let id = g.create("a reading log").expect("英語の一言から app を作る");
        let (def, _, mut h) = g.open_app(&id).expect("開いた app");
        let add = def.actions.iter().find(|a| a.id.ends_with(".add")).expect("足す操作").id.clone();
        let update = def.actions.iter().find(|a| a.id.ends_with(".update_status")).expect("状態を変える操作").id.clone();
        let source = add.rsplit_once('.').unwrap().0.to_owned();
        assert_eq!(h.readiness(&add), None, "足すが押せる");
        h.invoke(ActionInvocation { action_ref: add.clone(), origin: InvocationOrigin::Button { node_key: "books-add".into() }, input: json!({ "title": "Kokoro", "author": "Natsume Soseki" }) });
        assert!(h.line.is_none(), "足すが Verified: {:?}", h.line);
        let state = h.rt.ui_state(&source).unwrap();
        assert_eq!(state["items"][0]["title"], json!("Kokoro"), "足した 1 行が読み戻せない: {state}");
        let item = state["items"][0]["id"].as_str().expect("足した 1 件").to_owned();
        let status = json!({ "item_id": item, "status": "Read" });
        h.invoke(ActionInvocation { action_ref: update.clone(), origin: InvocationOrigin::Button { node_key: "books-status".into() }, input: status });
        assert!(h.line.is_none(), "英語の選択肢の値で状態を変えられない: {:?}", h.line);
        assert_eq!(h.rt.ui_state(&source).unwrap()["items"][0]["status"], json!("Read"), "変えた状態が読み戻せない");
    });
}

/// W15: 主デモを一言から再起動まで 1 本で通し、90 秒の中に収まる事と、feature を切った build では TextEdit の席が「外で開く」の 1 行になる事を見る。
/// 範囲は W16 より前（`native_direct`・`mirror` を切った .app と同じ）。試験であって実機確認ではない。
#[test]
fn demo_a_full() {
    with_cuadriver_absent("a_full", |dir| {
        let t = std::time::Instant::now();
        demo(dir);
        let took = t.elapsed();
        assert!(took < std::time::Duration::from_secs(90), "主デモが 90 秒を超えた: {took:?}");
        use surface::SurfaceBackend::{InteractiveMirror, NativeDirect};
        for b in [NativeDirect, InteractiveMirror] {
            // 切った build では「置けません」の 1 行・入れた build では None（両方向を同じ assert で縛る）
            assert_eq!(surface::missing_line(b, "TextEdit").is_some(), !surface::built(b));
        }
    });
}

/// 罠: CuaDriver を起こした瞬間に印が残る。通しの後に印が無い = デモ経路は一度も CuaDriver を起こしていない。
#[test]
fn demo_never_starts_cuadriver() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("trap");
    let marker = dir.join("cua-driver-was-started");
    let trap = dir.join("bin");
    std::fs::create_dir_all(&trap).unwrap();
    let script = trap.join("cua-driver");
    std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // 罠そのものが効く事を先に見る（効かない罠で「起こしていない」と言わない）
    let _ = Command::new(&script).status();
    assert!(marker.exists(), "罠の script が印を作れない");
    std::fs::remove_file(&marker).unwrap();
    let (path, bin) = (std::env::var_os("PATH"), std::env::var_os("CUA_DRIVER_BIN"));
    let mut p = vec![trap.clone()];
    p.extend(std::env::split_paths(&path.clone().unwrap_or_default()));
    std::env::set_var("PATH", std::env::join_paths(p).unwrap());
    std::env::set_var("CUA_DRIVER_BIN", &script);
    let r = std::panic::catch_unwind(|| demo(&dir));
    match path {
        Some(p) => std::env::set_var("PATH", p),
        None => std::env::remove_var("PATH"),
    }
    match bin {
        Some(b) => std::env::set_var("CUA_DRIVER_BIN", b),
        None => std::env::remove_var("CUA_DRIVER_BIN"),
    }
    let started = marker.exists();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
    assert!(!started, "デモ経路が CuaDriver を起こした");
}

/// 絶対 path（`/Applications/CuaDriver.app`）と設定の経路: デモ経路に取り込む source のどこにも、cua を起こす手がかりが comment の外に無い。
/// 罠の script で絶対 path を替えられない分を、ここで見る（この crate に取り込む dir と file は上の `#[path]` と同じ）。
#[test]
fn demo_sources_name_no_cua_route() {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(&e.unwrap().path(), out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p.to_path_buf());
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for d in ["domain", "storage", "runtime", "composer", "agent", "ui", "surface", "project", "terminal", "llm", "generated.rs"] {
        walk(&root.join(d), &mut files);
    }
    assert!(files.len() > 20, "取り込む source が見つからない: {}", files.len());
    let mut hits = Vec::new();
    for f in &files {
        for (i, line) in std::fs::read_to_string(f).unwrap().lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if ["CuaDriver.app", "cua-driver", "CUA_DRIVER_BIN", "cua::"].iter().any(|w| code.contains(w)) {
                hits.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "デモ経路に cua を起こす手がかりがある:\n{}", hits.join("\n"));
}

/// PV0: `#[path]` で取り込んだ `project::acquire` はこの結合試験の crate の一部として cfg(test) で compile される。製品の口 `Homebrew::find` は
/// ここでも本物の brew（/opt/homebrew/bin/brew・/usr/local/bin/brew）を拾わない = 試験が Install の道に届いても本物を叩かない（偽の brew は `pv0_` の試験が差す）。
/// 負の対照: `find` の `cfg!(test)` を外す → /usr/local/bin/brew の在るこの Mac で赤（brew の無い Mac では緑のまま = 測れない）。
#[test]
fn pv0_this_crate_never_finds_the_real_homebrew() {
    let h = project::acquire::Homebrew::find();
    assert_eq!(h.brew, None, "結合試験が本物の brew を拾った");
    assert_eq!(h.apps_dir, Path::new("/Applications"), "製品の口でない");
}

// ---- W3: 一言から複数 Page（I1）。この下の ignore はまだ無い段（偽物で埋めない・出来た Wave で外す） ----

/// W3 の I1（YC_ACCEPTANCE）: 空の data dir に主デモの一言（nonce 入り）を送ると、revision 1 の origin = generate・作った時刻が送信より後・
/// goal に nonce・Page が 2 枚以上で木が違う・surface_specs があり、bridge が無い時は全部の面が unavailable と理由（偽の verified 0）。
/// 読書記録は 1 Page の app のまま（Ok(None) → 今の `create`）。不在と罠の通し（`demo`）の終わりにも回す。
fn project_stage(dir: &Path) {
    let data = dir.join("data");
    let ws = dir.join("shibubu-yc-demo");
    std::fs::create_dir_all(&ws).unwrap();
    let nonce = format!("n-{}", storage::new_id());
    let said = format!("Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. {nonce}");
    let sent_at = runtime::now_rfc3339();
    let mut g = Generated::open(&data);
    let view = g.create_project(&said, Some(&ws), None).expect("一言から Project を作る").expect("主デモの一言は複数 Page の Project");
    let View::Project { project, page } = view else { panic!("Project の View ではない") };
    let app = g.app_of_project(&project).expect("殻の app が開いている");
    let (p, _) = g.project_of(&app).unwrap().expect("Project が読める");
    assert_eq!(p.pages[0].id, page, "先頭の Page（Dev）を開く");
    let receipts = g.surface_receipts(&project).unwrap();
    assert_eq!(receipts.len(), p.surface_specs.len(), "面ごとに読み戻しが 1 件");
    assert!(
        receipts.iter().all(|r| r.state == domain::project::ReceiptState::Unavailable && !r.oracle.is_empty()),
        "bridge 無しは unavailable と理由: {receipts:?}"
    );
    assert_eq!(g.create_project("読書記録をつけたい", Some(&ws), None).unwrap(), None, "読書記録は 1 Page の app のまま");
    drop(g);
    // 再起動しても Apps に載り、同じ Page で開ける
    let mut g = Generated::open(&data);
    assert!(g.rows().iter().any(|r| r.view == View::App(app.clone())), "再起動で Project の app が残る");
    assert_eq!(g.open_project(&project, None).unwrap(), View::Project { project: project.clone(), page: page.clone() });
    drop(g);
    let store = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    let bad = i1_violations(&store, &project, &nonce, &sent_at);
    assert!(bad.is_empty(), "I1: {bad:?}");
}

/// I1 の判定（DB だけを読む）: revision 1 が origin = generate・作った時刻が送信より後・goal に nonce・Page 2 枚以上で木が違う・surface_specs がある。
fn i1_violations(store: &storage::Store, project: &str, nonce: &str, sent_at: &str) -> Vec<String> {
    let revs = store.list_project_revisions(project).unwrap();
    let Some(first) = revs.first() else { return vec!["revision が無い".into()] };
    let p: domain::project::ProjectDefinitionV1 = serde_json::from_value(first.json.clone()).unwrap();
    let mut bad = Vec::new();
    if (first.revision, first.origin.as_str()) != (1, "generate") {
        bad.push(format!("revision 1 が generate でない: {} {}", first.revision, first.origin));
    }
    if first.created_at.as_str() <= sent_at {
        bad.push(format!("作った時刻 {} が送信 {sent_at} より後でない", first.created_at));
    }
    if !p.goal.contains(nonce) {
        bad.push(format!("goal に nonce が無い: {}", p.goal));
    }
    // key は Page の slug から作るので key ごと比べると必ず違う: key を除いた木（部品・label・props・子）を全部の組で比べる
    fn unkeyed(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(mut m) => {
                if m.contains_key("component") {
                    m.remove("key");
                }
                serde_json::Value::Object(m.into_iter().map(|(k, x)| (k, unkeyed(x))).collect())
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.into_iter().map(unkeyed).collect()),
            x => x,
        }
    }
    let trees: Vec<_> = p.pages.iter().map(|pg| unkeyed(serde_json::to_value(&pg.root).unwrap())).collect();
    if trees.len() < 2 || (1..trees.len()).any(|i| trees[..i].contains(&trees[i])) {
        bad.push(format!("Page が 2 枚以上で木が違う、でない（{} 枚）", p.pages.len()));
    }
    if p.surface_specs.is_empty() {
        bad.push("surface_specs が無い".into());
    }
    bad
}

#[test]
fn stage_project_with_pages() {
    let dir = scratch("w3");
    project_stage(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// I1 の負の対照: 一言を送る前に fixture（同じ形の Project の JSON）を読み込んだ DB は、形が同じでも I1 が赤。
#[test]
fn stage_project_fixture_is_not_i1() {
    let dir = scratch("w3-fixture");
    let store = storage::Store::open(&dir.join("kangae.sqlite3")).unwrap();
    let ctx = composer::ComposeContext { app_id: Some("app.fixture".into()), ..Default::default() };
    let fixture = composer::project::compose_project("この repo の開発環境を作って", &ctx, None).unwrap().unwrap().project;
    let json = serde_json::to_value(&fixture).unwrap();
    store.put(storage::Table::Projects, &fixture.id, None, &json).unwrap();
    store.put_project_revision(&fixture.id, 1, &json, None, &json!([]), "generate").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let sent_at = runtime::now_rfc3339();
    let bad = i1_violations(&store, &fixture.id, "n-this-run", &sent_at);
    assert!(bad.iter().any(|b| b.contains("作った時刻")) && bad.iter().any(|b| b.contains("nonce")), "fixture を I1 と読んだ: {bad:?}");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 反証の直し（W3）: I1 の「木が違う」は key を除いて比べる。全部の Page が key だけ違う同じ木なら赤。
#[test]
fn stage_project_same_tree_under_new_keys_is_not_i1() {
    fn rekey(v: &mut serde_json::Value, i: usize) {
        if let Some(k) = v.get("key").and_then(|k| k.as_str()).map(|k| format!("{k}-{i}")) {
            v["key"] = json!(k);
        }
        for c in v.get_mut("children").and_then(|c| c.as_array_mut()).into_iter().flatten() {
            rekey(c, i);
        }
    }
    let dir = scratch("w3-same-tree");
    let store = storage::Store::open(&dir.join("kangae.sqlite3")).unwrap();
    let ctx = composer::ComposeContext { app_id: Some("app.same".into()), ..Default::default() };
    let mut p = composer::project::compose_project("この repo の開発環境を作って", &ctx, None).unwrap().unwrap().project;
    let same = serde_json::to_value(&p.pages[1].root).unwrap();
    for (i, pg) in p.pages.iter_mut().enumerate() {
        let mut v = same.clone();
        rekey(&mut v, i);
        pg.root = serde_json::from_value(v).unwrap();
    }
    let json = serde_json::to_value(&p).unwrap();
    store.put(storage::Table::Projects, &p.id, None, &json).unwrap();
    store.put_project_revision(&p.id, 1, &json, None, &json!([]), "generate").unwrap();
    let bad = i1_violations(&store, &p.id, "開発環境", "0000");
    assert!(bad.iter().any(|b| b.contains("木が違う")), "key だけ違う同じ木を I1 と読んだ: {bad:?}");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// W4 で有効化（前は Cargo.toml の grep だった）: 本物の PTY の shell が `exit 3` で終わり、終わった後に書くと偽の成功でなく「終わっています」の 1 行。
#[test]
fn stage_real_terminal() {
    let m = terminal::TerminalManager::new(terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() });
    let spec = domain::project::TerminalSessionSpec { id: storage::new_id(), project_id: "p.stage".into(), shell: "/bin/zsh".into(), working_directory: "/".into(), keep_alive_on_page_switch: true, command: None };
    let key = terminal::SessionKey::of(&spec);
    m.spawn(&spec, terminal::TermSize::default()).expect("PTY の shell");
    let out = m.run_marked(&key, "pwd", std::time::Duration::from_secs(20)).unwrap();
    assert!(out.output.contains("\r\n/\r\n"), "{out:?}");
    m.write(&key, b"exit 3\n").unwrap();
    assert_eq!(m.wait_exit(&key, std::time::Duration::from_secs(20)).unwrap().code, Some(3));
    assert_eq!(m.write(&key, b"x").unwrap_err().to_string(), "The terminal has ended (exit code 3). Start a new one");
}

/// 混雑でも緑にする期限: 1 分 load を 4 で割った倍率（1〜8 倍）を掛ける。待つ対象が来ない（壊れている）なら伸ばしても赤のまま
/// （期限の上限を伸ばすだけで性質は変えない）。load は `sysctl -n vm.loadavg`（`{ 1.23 4.56 7.89 }`）。読めなければ 1 倍。
fn patient(base: std::time::Duration) -> std::time::Duration {
    let load = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().find_map(|w| w.parse::<f64>().ok()))
        .unwrap_or(0.0);
    base.mul_f64((load / 4.0).clamp(1.0, 8.0))
}

/// W5 の段（呼ぶ側の形・holdout の雛形）: Page に Terminal を置く → 長い command を打つ → 別の Page へ移る → 戻る → 同じ pid の出力に印。
/// 実装の commit で ignore を外し `demo()` の後半に足す。実機（BG）の同じ手は `ActionHost::terminal` の doc（`terminal:<id>` の AX の text）。
/// W5 の段: Page に Terminal を置く → 長い command を打つ → 別の Page へ移る → 戻る → 同じ pid の出力に印（manager の byte と、
/// Page を描いた AX の `terminal:<id>` の text の両方）・親が Shibubu で tty を持つ → Shibubu を閉じて開き直すと起こさず「新しく起こせます」→
/// 新しく起こすと同じ session id で別の pid。呼ぶ側の ENV の lock の中で呼ぶ（SHELL を替えて戻す）。
fn page_terminal(dir: &Path, child_path: &str) {
    let data = dir.join("data");
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], child_path: child_path.into(), ..Default::default() };
    let mut g = Generated::open(&data).with_terminal_config(rc_free.clone());
    let app = g.create("読書記録をつけたい").unwrap();
    let View::Project { project, page: a } = g.project_view_of(&app).unwrap() else { panic!() };
    g.open_project(&project, None).unwrap();
    let View::Project { page: b, .. } = g.page_intent(&project, ui::shell::PageIntent::Add).unwrap() else { panic!() };
    g.navigate_page(&project, &a).unwrap();
    let placed = g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: a.clone() });
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    assert_eq!(placed.unwrap(), View::Project { project: project.clone(), page: a.clone() }, "置いても Page は移らない");
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let spec = p.terminal_specs.first().expect("terminal_specs に 1 つ").clone();
    assert!(p.page(&a).unwrap().root.nodes().into_iter().any(|n| n.component == "terminal" && n.props["terminal_ref"] == json!(spec.id)), "Page A に terminal の席");
    assert!(!p.page(&b).unwrap().root.nodes().into_iter().any(|n| n.component == "terminal"), "B には置かない");
    let before = g.ensure_terminal(&project, &spec.id).unwrap();
    assert_eq!(before.status, terminal::SessionStatus::Running);
    assert_eq!(g.terminals().list(Some(&project)).len(), 1, "置いた時に 1 本だけ起こす");
    let key = terminal::SessionKey::of(&spec);
    let mark = format!("done-{}", storage::new_id());
    // 「sleep 2 の直後はまだ印が無い」は時間の仮定（混雑で frame の読みが 2 秒を超えると印が先に出て赤）→ 入力を待つ command にして、
    // 印が無い事を「入力を送る前」の順序で言う。送った後は印が出るまで待つ（期限だけ patient）
    g.terminals().write(&key, format!("read -r x; echo {mark}\n").as_bytes()).unwrap();
    assert!(!page_terminal_text(&mut g, &project, &a, &spec.id).contains(&format!("\n{mark}\n")), "正の対照: 入力を送る前は印が無い");
    g.terminals().write(&key, b"\n").unwrap();
    g.navigate_page(&project, &b).unwrap();
    let end = std::time::Instant::now() + patient(std::time::Duration::from_secs(20));
    while !String::from_utf8_lossy(&g.terminals().read_since(&key, 0).unwrap().bytes).contains(&format!("\r\n{mark}\r\n")) {
        assert!(std::time::Instant::now() < end, "入力を送っても印が出ない");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    g.navigate_page(&project, &a).unwrap();
    let after = g.ensure_terminal(&project, &spec.id).unwrap();
    assert_eq!(after.pid, before.pid, "Page を移って戻っても同じ shell");
    assert_eq!(g.terminals().list(Some(&project)).len(), 1, "移って戻っても起こし直さない");
    let out = g.terminals().read_since(&key, 0).unwrap();
    assert!(String::from_utf8_lossy(&out.bytes).contains(&format!("\r\n{mark}\r\n")), "移っている間に出た印が残る");
    let shown = page_terminal_text(&mut g, &project, &a, &spec.id);
    assert!(shown.contains(&format!("\n{mark}\n")), "Page の terminal:<id> の AX の text に印: {shown:?}");
    assert!(!shown.contains('\x1b'), "escape を落とす: {shown:?}");
    let ps = |pid: u32, col: &str| String::from_utf8_lossy(&Command::new("/bin/ps").args(["-o", col, "-p", &pid.to_string()]).output().unwrap().stdout).trim().to_owned();
    assert_eq!(ps(after.pid, "ppid="), std::process::id().to_string(), "親が Shibubu");
    assert!(ps(after.pid, "tty=").starts_with("ttys"));
    // Shibubu を閉じる → 開き直す（TERM-RESTORE-1101）: 前の起動の Terminal は同じ session id の新しい shell で起きる（Restart を押させない）
    drop(g);
    assert_eq!(ps(after.pid, "stat="), "", "閉じた後に shell が残る");
    let mut g = Generated::open(&data).with_terminal_config(rc_free);
    g.open_project(&project, Some(&a)).unwrap();
    let again = g.ensure_terminal(&project, &spec.id).unwrap();
    assert_eq!((again.key.clone(), again.status.clone()), (key.clone(), terminal::SessionStatus::Running), "開き直した Terminal に shell が無い");
    assert_ne!(again.pid, before.pid, "新しい shell は別の pid");
    assert!(!page_terminal_status(&mut g, &project, &a).iter().any(|t| t.contains("Restart")), "Restart を押させる 1 行が残った");
    assert!(g.restart_terminal(&project, &spec.id).is_err(), "動いている Terminal は起こし直さない");
    drop(g);
}

/// Page を egui で 2 frame 描き（AccessKit 有り）、`terminal:<id>` の要素の text を返す（試験の側が実機で読む物と同じ口）。
fn page_ax(g: &mut Generated, project: &str, page: &str) -> Vec<(Option<String>, String)> {
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let mut out = None;
    for _ in 0..2 {
        let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 900.0))), ..Default::default() };
        out = Some(ctx.run(input, |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                g.draw_page(ui, project, page);
            });
        }));
    }
    let update = out.unwrap().platform_output.accesskit_update.expect("AccessKit の木");
    update.nodes.iter().map(|(_, n)| (n.author_id().map(str::to_owned), n.label().or(n.value()).unwrap_or("").to_owned())).collect()
}

fn page_terminal_text(g: &mut Generated, project: &str, page: &str, terminal_ref: &str) -> String {
    let id = format!("terminal:{terminal_ref}");
    let hits: Vec<String> = page_ax(g, project, page).into_iter().filter(|(a, _)| a.as_deref() == Some(id.as_str())).map(|(_, t)| t).collect();
    assert_eq!(hits.len(), 1, "{id} の要素がちょうど 1 つ: {hits:?}");
    hits.into_iter().next().unwrap()
}

fn page_terminal_status(g: &mut Generated, project: &str, page: &str) -> Vec<String> {
    page_ax(g, project, page).into_iter().map(|(_, t)| t).collect()
}

#[test]
fn stage_terminal_on_page() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    page_terminal(&scratch("w5"), terminal::DEFAULT_CHILD_PATH);
}

/// W10 の段: Page の Terminal に `seq 1 200000` を流している間も Page の frame（`draw_page` = VT の写し・grid・AX）が 1 秒以上止まらず、
/// 終わった時の `terminal:<id>` の AX の text に `200000` の行が出る（manager の byte ではなく Page を描いた物で読む）。
/// W10 反証: grid を click して、stdin を読まない job（`sleep`）の前に 400 行を貼り付けた frame も止まらず、job の後に 400 行が動く。
#[test]
fn stage_terminal_vt_keeps_frames_during_flood() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let data = scratch("w10").join("data");
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&data).with_terminal_config(rc_free);
    let app = g.create("読書記録をつけたい").unwrap();
    let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
    g.open_project(&project, None).unwrap();
    let placed = g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: page.clone() });
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    placed.unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let spec = p.terminal_specs.first().unwrap().clone();
    g.ensure_terminal(&project, &spec.id).unwrap();
    let key = terminal::SessionKey::of(&spec);
    let id = format!("terminal:{}", spec.id);
    // 1 つの Context で frame を回す（本番の窓と同じ: font・AccessKit を毎回作り直さない）
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    // 窓は grid が画面に収まる高さ（貼り付けの段で grid を click する）
    let screen = eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 1600.0));
    let frame = |g: &mut Generated, events: Vec<eframe::egui::Event>| {
        let input = eframe::egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let t0 = std::time::Instant::now();
        let out = ctx.run(input, |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                g.draw_page(ui, &project, &page);
            });
        });
        let took = t0.elapsed();
        let node = out.platform_output.accesskit_update.and_then(|u| u.nodes.iter().find(|(_, n)| n.author_id() == Some(id.as_str())).map(|(_, n)| (n.value().unwrap_or("").to_owned(), n.bounds())));
        let (text, bounds) = node.unwrap_or_default();
        (took, text, bounds)
    };
    frame(&mut g, vec![]);
    frame(&mut g, vec![]); // 1 frame 目で行と列を決めて resize（Page の残りの高さの行 × 幅）
    let s = g.terminals().screen(&key, 0).unwrap();
    assert!(s.rows > 24 && s.rows <= 120, "1600 の窓は Page の残りの高さから 24 行より多くを PTY と VT に渡す（W15-c3 の term_rows）: {}", s.rows);
    assert!(s.cols > 80, "1440 の窓の幅から列: {}", s.cols);
    // 人の打鍵は Page の席から（grid・入力欄と同じ view。view の無い `write` は Page{session_id} で、後の grid の click が OtherView になる）
    let seat = domain::terminal::InputView::Page { page_id: page.clone() };
    g.terminals().write_as(&key, domain::terminal::InputOwner::Human, &seat, b"seq 1 200000\n").unwrap();
    let (start, mut worst, mut frames) = (std::time::Instant::now(), std::time::Duration::ZERO, 0);
    let text = loop {
        let (took, text, _) = frame(&mut g, vec![]);
        worst = worst.max(took);
        frames += 1;
        if text.contains("\n200000\n") {
            break text;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(300), "200000 が Page に出ない（{frames} frame）: {:?}", text.lines().rev().take(3).collect::<Vec<_>>());
        std::thread::sleep(std::time::Duration::from_millis(16));
    };
    // 流し終わった後の静かな frame（同じ 1 000 行の scrollback を写す・parse は無い）。debug の build の混雑の下では、静かな frame でも
    // 1 秒に近づく（draw_page の毎 frame の DB 読み・hash を含む）。だから「1 秒未満」か「静かな frame の 2 倍以内」の どちらかを要る:
    // UI の frame が出力を parse する（W5 の OutputView のように 1.3 MB を frame で読む）と、どちらも破れる。1 秒の線そのものは release の実機（drive-w10.py）
    let quiet = (0..8).map(|_| frame(&mut g, vec![]).0).max().unwrap();
    eprintln!("W10 flood: {frames} frame・一番長い frame {worst:?}・静かな frame の一番長い {quiet:?}・{:?}", start.elapsed());
    assert!(worst < std::time::Duration::from_secs(1) || worst <= quiet * 2, "Page の frame が {worst:?} 止まった（{frames} frame・静かな frame {quiet:?}）");
    assert!(text.contains("\n199999\n200000\n"), "最後の行の前は 199999");
    assert!(!text.contains('\x1b'), "escape は AX の text に出ない");

    // W10 反証: 貼り付けは UI の thread から PTY へ。stdin を読まない job の前で tty の入力が詰まっても、その frame は止まらない
    let b = frame(&mut g, vec![]).2.expect("terminal の AX の bounds");
    let at = eframe::egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    assert!(screen.contains(at), "grid の中心 {at:?} が窓の外");
    let button = |pressed| eframe::egui::Event::PointerButton { pos: at, button: eframe::egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&mut g, vec![eframe::egui::Event::PointerMoved(at), button(true)]);
    frame(&mut g, vec![button(false)]);
    frame(&mut g, vec![]); // click の次の frame から grid が focus を持つ
    // grid の click で人の lease は Page の席の物（view の無い `write` だと OtherView。a20a44d から赤で、targeted に入っていなかった）
    g.terminals().write_as(&key, domain::terminal::InputOwner::Human, &seat, b"sleep 6\n").unwrap();
    // zsh が sleep を前に出す（bracketed paste を外す）まで待つ: 外す前に貼ると 1 つの貼り付けとして入力欄に入り、動かない
    let t0 = std::time::Instant::now();
    while g.terminals().screen(&key, 0).unwrap().bracketed_paste {
        assert!(t0.elapsed() < std::time::Duration::from_secs(30), "sleep が前に出ない");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // 出力の行は `P1000Q`〜`P1399Q`（打った行の echo は `% echo P…` で、行の頭に `P` は来ない）。job が読まない間に tty の入力から
    // 溢れた分は macOS の tty が捨てる（どの端末でも同じ・2026-10-22 に python の pty で 400 行中 382〜397 行）ので、全行は数えない
    let paste: String = (0..400).map(|i| format!("echo P{}Q\n", i + 1000)).collect();
    let (pasted, ..) = frame(&mut g, vec![eframe::egui::Event::Paste(paste)]);
    eprintln!("W10 paste: 貼り付けの frame {pasted:?}・静かな frame の一番長い {quiet:?}");
    assert!(pasted < std::time::Duration::from_secs(1) || pasted <= quiet * 2, "貼り付けの frame が {pasted:?} 止まった（静かな frame {quiet:?}）");
    let t0 = std::time::Instant::now();
    let text = loop {
        let (_, text, _) = frame(&mut g, vec![]);
        if text.contains("\nP1399Q") {
            break text;
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(120), "貼った 400 行が動かない: {:?}", text.lines().rev().take(3).collect::<Vec<_>>());
        std::thread::sleep(std::time::Duration::from_millis(16));
    };
    assert!(text.find("\nP1000Q").is_none_or(|first| Some(first) < text.find("\nP1399Q")), "貼った順に動く");
    drop(g);
}

/// W2b-AX 直し（反証の指摘 1）: Project の Page に動いている Terminal が在っても、出力が無い間は frame を 1 つも頼まない
/// （直す前は components.rs が 100ms ごとに頼んだ = 背景・隠れた窓でも 10 frame/秒・main thread が GL の flush に居続け AX が後ろに並ぶ形）。
/// 出力が来た・shell が終わった時は PTY の側が egui を起こし（`Generated::wake_on_terminal`・app.rs と同じ口）、起きた frame で
/// AX の `terminal:<id>` に出力、状態の 1 行に「終わりました」が出る。frame は頼まれた時だけ描く（eframe の event loop の写し・入力は渡さない）。
#[test]
fn stage_idle_terminal_page_requests_no_frames_and_pty_wakes_it() {
    use eframe::egui;
    use std::time::{Duration, Instant};
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let data = scratch("w2b-ax-term").join("data");
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&data).with_terminal_config(rc_free);
    let ctx = egui::Context::default();
    ui::theme::install(&ctx, egui::ThemePreference::Light);
    ctx.enable_accesskit();
    g.wake_on_terminal(&ctx);
    let app = g.create("読書記録をつけたい").unwrap();
    let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
    g.open_project(&project, None).unwrap();
    let placed = g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: page.clone() });
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    placed.unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let spec = p.terminal_specs.first().expect("terminal_specs に 1 つ").clone();
    let key = terminal::SessionKey::of(&spec);
    let ax_id = format!("terminal:{}", spec.id);
    // 1 frame を描き、(次の frame を頼んだ遅れ, terminal:<id> の text, 全部の要素の text) を返す
    let mut t = 0.0;
    let mut frame = |g: &mut Generated| {
        t += 0.1;
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))), time: Some(t), focused: false, ..Default::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                g.draw_page(ui, &project, &page);
            });
        });
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        let nodes: Vec<(Option<String>, String)> = out.platform_output.accesskit_update.map(|u| u.nodes.iter().map(|(_, n)| (n.author_id().map(str::to_owned), n.label().or(n.value()).unwrap_or("").to_owned())).collect()).unwrap_or_default();
        let shown = nodes.iter().find(|(a, _)| a.as_deref() == Some(ax_id.as_str())).map(|(_, s)| s.clone()).unwrap_or_default();
        (delay, shown, nodes.into_iter().map(|(_, s)| s).collect::<Vec<_>>())
    };
    // 頼まれるまで待つ（混雑で背景の thread が遅れても落ちないよう長め）。頼まれたら true
    let asked = |within: Duration| {
        let end = Instant::now() + within;
        while !ctx.has_requested_repaint() {
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    };
    let ctx2 = ctx.clone();
    let causes = move || format!("{:?}", ctx2.repaint_causes());
    // ① 起動の出力が静まった後、動いている Terminal の Page は frame を頼まない（1 秒の間に PTY も egui を起こさない）
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut delay, _, _) = frame(&mut g);
    loop {
        if delay == Duration::MAX && !asked(Duration::from_secs(1)) {
            break;
        }
        assert!(Instant::now() < deadline, "動いている Terminal の Page が 30 秒たっても idle にならない（最後の frame は {delay:?} 後の frame を頼んだ・{}）", causes());
        delay = frame(&mut g).0;
    }
    assert_eq!(g.ensure_terminal(&project, &spec.id).unwrap().status, terminal::SessionStatus::Running, "試験が成り立たない: Terminal が動いていない");
    // ② 外から打った command の出力（AI の入力・長い command の続き）は、入力の無い idle の窓を起こして AX の text に出る
    let mark = storage::new_id();
    g.terminals().write(&key, format!("echo woke-$((6*7))-{mark}\n").as_bytes()).unwrap();
    let want = format!("woke-42-{mark}");
    let mut shown = String::new();
    while !shown.contains(&want) {
        assert!(asked(Duration::from_secs(20)), "PTY の出力が egui を起こさない（idle の窓は次の入力まで出力を出さない）: {shown:?}");
        shown = frame(&mut g).1;
    }
    // ③ shell が終わった事も起こす: 状態の 1 行が「終わりました」になり、入力欄が消える
    g.terminals().write(&key, b"exit 3\n").unwrap();
    let mut texts = Vec::new();
    while !texts.iter().any(|s: &String| s.contains("Shell exited (code 3)")) {
        assert!(asked(Duration::from_secs(20)), "shell が終わっても egui が起きない（Page は「動いている」のまま）: {texts:?}");
        texts = frame(&mut g).2;
    }
    assert!(!texts.iter().any(|s| s == "Type in the terminal"), "終わった Terminal に入力欄が残る: {texts:?}");
    drop(g);
}

/// 試験の側の目: Page を egui で 2 frame 描いた AccessKit の木（実機では cua-driver の AX の木）の (identifier, role, 文字, 左, 上)。
fn page_tree(g: &mut Generated, project: &str, page: &str) -> Vec<(Option<String>, eframe::egui::accesskit::Role, String, f64, f64)> {
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let mut out = None;
    for _ in 0..2 {
        let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 900.0))), ..Default::default() };
        out = Some(ctx.run(input, |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                g.draw_page(ui, project, page);
            });
        }));
    }
    let update = out.unwrap().platform_output.accesskit_update.expect("AccessKit の木");
    update
        .nodes
        .iter()
        .map(|(_, n)| {
            let b = n.bounds().unwrap_or_default();
            (n.author_id().map(str::to_owned), n.role(), n.label().or(n.value()).unwrap_or("").to_owned(), b.x0, b.y0)
        })
        .collect()
}

/// 画面の Page タブ（左から）: 帯の `＋`（AX `Add page · ⌘T`）と同じ行でその左に在る button（`↶ ↷` を除く）。製品の定義は見ない。
/// UI-5a: tab は 36 の行の高さ・`＋` は 28 の高さで行の中央なので、上端の差は 4（同じ行 = 差が 10 より小さい）。
fn screen_tabs(g: &mut Generated, project: &str, page: &str) -> Vec<String> {
    let tree = page_tree(g, project, page);
    let (_, _, _, px, py) = tree.iter().find(|n| n.2 == "Add page · ⌘T").cloned().expect("帯の ＋ の button");
    let mut tabs: Vec<(f64, String)> = tree
        .into_iter()
        .filter(|n| n.1 == eframe::egui::accesskit::Role::Button && (n.4 - py).abs() < 10.0 && n.3 < px && !n.2.starts_with("Undo") && !n.2.starts_with("Redo"))
        .map(|n| (n.3, n.2))
        .collect();
    tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
    tabs.into_iter().map(|t| t.1).collect()
}

/// その Page の AX の identifier のうち `prefix` で始まる物の残り（`terminal:` → Terminal の id・`surface-status:` → 面の id）。
fn screen_ids(g: &mut Generated, project: &str, page: &str, prefix: &str) -> Vec<String> {
    let mut ids: Vec<String> = page_tree(g, project, page).into_iter().filter_map(|n| n.0.and_then(|a| a.strip_prefix(prefix).map(str::to_owned))).collect();
    ids.sort();
    ids
}

/// pid が今、この試験の process（= Shibubu）の子として tty を持って動いているか（`ps`・zombie は数えない）。
fn ps_child_alive(pid: u32) -> bool {
    let o = Command::new("/bin/ps").args(["-o", "ppid=,tty=,stat=", "-p", &pid.to_string()]).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    let f: Vec<&str> = s.split_whitespace().collect();
    f.len() == 3 && f[0] == std::process::id().to_string() && f[1].starts_with("ttys") && !f[2].starts_with('Z')
}

/// `shibubu project pages --json` の Page ごとの数を、試験の側で数えた値と照らす（製品の registry とは照らさない）。
/// Page の数と順と題 = 画面の Page タブ・Page の Terminal と面 = その Page の AX の `terminal:<id>` / `surface-status:<id>`・
/// 動いている数と pid = shell 自身が `$$` で言った pid（`pids`）と `ps`。
fn cli_matches_screen(g: &mut Generated, project: &str, v: &serde_json::Value, pids: &std::collections::BTreeMap<String, u32>) {
    let pages = v["pages"].as_array().expect("pages");
    let first = pages[0]["id"].as_str().unwrap();
    let titles: Vec<String> = pages.iter().map(|p| p["title"].as_str().unwrap().to_owned()).collect();
    assert_eq!(titles, screen_tabs(g, project, first), "Page の数・順・題が画面の Page タブと同じ");
    for (i, p) in pages.iter().enumerate() {
        let id = p["id"].as_str().unwrap();
        assert_eq!(p["index"], json!(i));
        let mut terms: Vec<String> = p["terminals"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_owned()).collect();
        terms.sort();
        assert_eq!(terms, screen_ids(g, project, id, "terminal:"), "Page {id} の Terminal = 画面の terminal:<id>");
        let mut surfs: Vec<String> = p["surfaces"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_owned()).collect();
        surfs.sort();
        assert_eq!(surfs, screen_ids(g, project, id, "surface-status:"), "Page {id} の面 = 画面の surface-status:<id>");
        let alive_cli = p["terminals"].as_array().unwrap().iter().filter(|t| t["alive"] == json!(true)).count();
        let alive_ps = terms.iter().filter(|t| pids.get(*t).is_some_and(|&pid| ps_child_alive(pid))).count();
        assert_eq!(alive_cli, alive_ps, "Page {id} の動いている Terminal の数 = ps で数えた数");
        for t in p["terminals"].as_array().unwrap() {
            assert_eq!(t["pid"], json!(pids[t["id"].as_str().unwrap()]), "pid = shell 自身の $$");
        }
    }
}

/// W9 の段: Project の Terminal の中で本物の `shibubu`（cargo が作った bin）を `shibubu project pages|surfaces|context --json` と打つ。
/// Project と保存先は spawn が子に渡す環境変数だけで決まる（`--project` の口は無い）。返った Page ごとの数を試験の側の数（`cli_matches_screen`）と照らし、
/// Terminal を 1 本終わらせて Page を 1 枚消した後も同じく照らす（消した Page を出さない・終わった shell を動いていると言わない）。
/// Shibubu を閉じた後・前の起動が落ちて Running の記録が残った後（開き直しで Unavailable）も、読んだ時の pid で alive を決める。呼ぶ側の ENV の lock の中で呼ぶ。
fn project_cli_stage(dir: &Path, child_path: &str) {
    use ui::shell::PageIntent;
    let data = dir.join("data");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_shibubu"));
    let g = Generated::open(&data);
    let filled = g.terminals().config().project_cli.clone().expect("DB を開けた Generated は Terminal に Project の口を渡す");
    assert_eq!(filled.data_dir, data.canonicalize().unwrap(), "保存先は絶対 path");
    assert_eq!(filled.bin_dir.as_path(), std::env::current_exe().unwrap().parent().unwrap(), "shibubu の dir = 実行 file の dir");
    // 試験の実行 file の dir には shibubu が無いので、cargo が作った本物の shibubu の dir を渡す（rc を読まない zsh・PATH は呼ぶ側の物）
    let cli_dirs = terminal::ProjectCli { data_dir: filled.data_dir.clone(), bin_dir: exe.parent().unwrap().into() };
    let config = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], child_path: child_path.into(), project_cli: Some(cli_dirs.clone()), ..Default::default() };
    let mut g = g.with_terminal_config(config);
    let said = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw.";
    let View::Project { project, .. } = g.create_project(said, None, None).unwrap().unwrap() else { panic!("Project の View ではない") };
    g.open_project(&project, None).unwrap();
    let app = g.app_of_project(&project).unwrap();
    let page_ids: Vec<String> = g.project_of(&app).unwrap().unwrap().0.pages.iter().map(|p| p.id.clone()).collect();
    assert!(page_ids.len() >= 3, "主デモの一言は 3 枚以上: {page_ids:?}");
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let placed: Vec<_> = [&page_ids[0], &page_ids[0], &page_ids[1]].into_iter().map(|pg| g.page_intent(&project, PageIntent::AddTerminal { page: pg.clone() })).collect();
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    assert!(placed.iter().all(Result::is_ok), "Terminal を置く: {placed:?}");
    let specs = g.project_of(&app).unwrap().unwrap().0.terminal_specs;
    assert_eq!(specs.len(), 3);
    let keys: Vec<terminal::SessionKey> = specs.iter().map(terminal::SessionKey::of).collect();
    let t = std::time::Duration::from_secs(30);
    let sh = |g: &Generated, key: &terminal::SessionKey, cmd: &str| g.terminals().run_marked(key, cmd, t).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    // pid は shell 自身に言わせる（manager の SessionInfo を数えない）
    let mut pids = std::collections::BTreeMap::new();
    for (s, k) in specs.iter().zip(&keys) {
        let out = sh(&g, k, "echo $$");
        let pid: u32 = out.output.split(['\r', '\n']).find_map(|l| l.trim().parse().ok()).unwrap_or_else(|| panic!("$$ が読めない: {out:?}"));
        assert!(ps_child_alive(pid), "shell {pid} が Shibubu の子として tty で動いている");
        pids.insert(s.id.clone(), pid);
    }
    // 子の環境: 自分の Project の id と、PATH の先頭の本物の shibubu
    let env = sh(&g, &keys[0], "printf '%s|%s\\n' \"$SHIBUBU_PROJECT_ID\" \"$(command -v shibubu)\"");
    assert!(env.output.contains(&format!("\r\n{project}|{}\r\n", exe.display())), "子の環境: {:?}", env.output);
    let cli = |g: &Generated, what: &str| -> serde_json::Value {
        let out = sh(g, &keys[0], &format!("shibubu project {what} --json"));
        assert_eq!(out.exit_code, 0, "shibubu project {what}: {:?}", out.output);
        let line = out.output.split(['\r', '\n']).find(|l| l.starts_with('{')).unwrap_or_else(|| panic!("JSON の行が無い: {:?}", out.output));
        serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"))
    };
    let v = cli(&g, "pages");
    assert_eq!((v["schema"].as_str(), v["project"]["id"].as_str()), (Some(crate::project::context::SCHEMA), Some(project.as_str())));
    assert_eq!(v.as_object().unwrap().len(), 4, "pages は schema・observed_at・project・pages だけ");
    let counts: Vec<usize> = v["pages"].as_array().unwrap().iter().map(|p| p["terminals"].as_array().unwrap().len()).collect();
    assert_eq!(&counts[..3], [2, 1, 0], "置いた数（正の対照: 0 ばかりの一覧で緑にしない）");
    cli_matches_screen(&mut g, &project, &v, &pids);
    let s = cli(&g, "surfaces");
    let surfaces = s["surfaces"].as_array().unwrap();
    assert!(!surfaces.is_empty(), "主デモの Project は面を持つ（0 件の一覧で緑にしない）");
    assert!(surfaces.iter().all(|x| x["window"].is_null()), "S の列は面に窓を繋がない（M1 の後）");
    let ctx: crate::project::context::ProjectContext = serde_json::from_value(cli(&g, "context")).expect("context は ProjectContext の形（知らない欄なし）");
    assert_eq!(ctx.terminals.len(), 3);
    assert_eq!(serde_json::to_value(&ctx.surfaces).unwrap(), s["surfaces"], "surfaces と context の面は同じ");

    // 変える: Page B の shell を終わらせ（回収まで待つ）、3 枚目の Page を消す → 読み直すと消えた Page を出さず、終わった shell を動いていると言わない
    g.terminals().write(&keys[2], b"exit\n").unwrap();
    g.terminals().wait_exit(&keys[2], t).unwrap();
    assert!(!ps_child_alive(pids[&specs[2].id]), "前提: 終わった shell は ps に居ない");
    g.page_intent(&project, PageIntent::Delete { page: page_ids[2].clone() }).expect("Page を消す");
    let v = cli(&g, "pages");
    assert_eq!(v["pages"].as_array().unwrap().len(), page_ids.len() - 1, "消した Page を出さない");
    assert!(v["pages"].as_array().unwrap().iter().all(|p| p["id"] != json!(page_ids[2])));
    let b = &v["pages"][1]["terminals"][0];
    assert_eq!((b["alive"].clone(), b["recorded"]["state"].clone()), (json!(false), json!("running")), "記録は Running のままでも、読んだ時に居ない pid は動いていない");
    cli_matches_screen(&mut g, &project, &v, &pids);

    // Shibubu の外から（Terminal と同じ環境変数だけを渡して）読む: 閉じた後は全部止まっている・記録は Exited
    let run_cli = |what: &str, extra: &[(&str, &str)]| {
        let mut c = Command::new(&exe);
        c.args(["project", what, "--json"]).env_remove(terminal::PROJECT_ENV).env_remove(terminal::DATA_DIR_ENV).env_remove("KANGAE_DATA_DIR");
        for (k, v) in extra {
            c.env(k, v);
        }
        c.output().unwrap()
    };
    let env_of = [(terminal::PROJECT_ENV, project.as_str()), (terminal::DATA_DIR_ENV, cli_dirs.data_dir.to_str().unwrap())];
    drop(g);
    let o = run_cli("pages", &env_of);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    for p in v["pages"].as_array().unwrap() {
        for t in p["terminals"].as_array().unwrap() {
            assert_eq!((t["alive"].clone(), t["recorded"]["state"].clone()), (json!(false), json!("exited")), "閉じた後: {t}");
        }
    }
    assert!(pids.values().all(|&pid| !ps_child_alive(pid)), "閉じた後に shell が残る");
    // Project の Terminal の外（環境変数が無い）: stdout は空・stderr に 1 行・code 2
    let o = run_cli("pages", &[]);
    assert_eq!((o.status.code(), o.stdout.is_empty()), (Some(2), true));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.trim_end().lines().count() == 1 && err.contains("doesn't know the project"), "{err}");
    // 反証（W9）: Project の Terminal の中（PATH の先頭が shibubu・環境に Project と保存先）の打ち間違いは、2 つ目の Shibubu（GUI）を起こさず保存も開かない。
    // 直す前の binary は同じ環境と argv で GUI のまま 6 秒以上残り、渡した保存先に kangae.sqlite3 を作った（BACKGROUND は赤の時に前面を取らないため）
    let fresh = dir.join("typo-data");
    std::fs::create_dir_all(&fresh).unwrap();
    for typed in [&["--help"][..], &["pages", "--json"], &[]] {
        let mut child = Command::new(&exe)
            .args(typed)
            .env_remove("KANGAE_DATA_DIR")
            .env(terminal::PROJECT_ENV, &project)
            .env(terminal::DATA_DIR_ENV, &fresh)
            .env("SHIBUBU_BACKGROUND", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Project の Terminal の中の shibubu {typed:?} が 30 秒で終わらない（GUI を起こした）");
        }
        let o = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!((o.status.code(), o.stdout.is_empty()), (Some(2), true), "shibubu {typed:?}: {err}");
        assert!(err.trim_end().lines().count() == 1 && err.contains("Shibubu doesn't open"), "shibubu {typed:?}: {err}");
    }
    assert_eq!(std::fs::read_dir(&fresh).unwrap().count(), 0, "打ち間違いが保存を開いた（kangae.sqlite3 を作った）");
    // 前の起動が落ちて Running の記録が残った（pid はたまたま今動いている process = この試験）→ 開き直すまでは読んだ時の pid で動いて見え、
    // 開き直した後（Generated::start が Unavailable に書き戻す）は動いていないと読む
    let st = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    let crashed = terminal::SessionInfo { key: terminal::SessionKey { project_id: project.clone(), session_id: "crashed".into() }, pid: std::process::id(), shell: "/bin/zsh".into(), working_directory: "/".into(), status: terminal::SessionStatus::Running };
    st.put(storage::Table::TerminalSessions, "crashed", Some(&project), &serde_json::to_value(&crashed).unwrap()).unwrap();
    drop(st);
    let crashed_alive = |o: std::process::Output| -> serde_json::Value {
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v["terminals"].as_array().unwrap().iter().find(|t| t["id"] == json!("crashed")).expect("spec の無い記録も context に出る")["alive"].clone()
    };
    assert_eq!(crashed_alive(run_cli("context", &env_of)), json!(true), "開き直す前の Running の記録は、読んだ時に pid が在れば動いている（但し書き: pid の再利用は見ない）");
    drop(Generated::open(&data));
    assert_eq!(crashed_alive(run_cli("context", &env_of)), json!(false), "開き直した後の前の起動の pid は動いていない");
}

#[test]
fn stage_project_cli() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    project_cli_stage(&scratch("w9"), terminal::DEFAULT_CHILD_PATH);
}

/// 前面の不具合（10-22 04:24 yc-s10 の `deps/shibubu-660d77d7… --list`・10-21 yc-s13 の `deps/shibubu-5aa2d147… --list`）: どちらも libtest の
/// binary ではなく製品の bin（cargo がこの `CARGO_BIN_EXE_shibubu` のために作る物）で、知らない引数のまま画面を開き前面を取った。
/// 本物の bin を試験の側の argv で起こし、窓を開かず stderr の 1 行と code 2 で終わる事を見る。赤の時に窓が出ても前面を取らないよう
/// 背景の印（`SHIBUBU_BACKGROUND`）と空の保存先で起こし、30 秒で終わらなければ kill して赤（混んだ機械の起動の遅れより長く取る）。
#[test]
fn shibubu_bin_given_a_test_runner_argv_exits_without_a_window() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_shibubu"));
    let data = scratch("argv").join("data");
    for args in [&["--list"][..], &["--list", "--format", "terse"], &["w10_"], &["--exact", "w10_x"], &["--help"]] {
        let mut child = Command::new(&exe)
            .args(args)
            .env("SHIBUBU_BACKGROUND", "1")
            .env(terminal::DATA_DIR_ENV, &data)
            .env("KANGAE_DATA_DIR", &data)
            .env_remove(terminal::PROJECT_ENV)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let exited = loop {
            if child.try_wait().unwrap().is_some() {
                break true;
            }
            if started.elapsed() > std::time::Duration::from_secs(30) {
                let _ = child.kill();
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(exited, "shibubu {args:?} が 30 秒で終わらない（窓を開いた）: {err}");
        assert_eq!(out.status.code(), Some(2), "shibubu {args:?}: {err}");
        assert!(out.stdout.is_empty(), "stdout は空: {:?}", String::from_utf8_lossy(&out.stdout));
        assert!(err.contains("Shibubu doesn't open") && err.trim_end().lines().count() == 1, "1 行で何が起きなかったか: {err}");
    }
}

#[test]
fn stage_diff_edit_of_ui() {
    let dir = scratch("w7");
    let data = dir.join("data");
    let mut g = Generated::open(&data);
    let said = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw.";
    let View::Project { project, page } = g.create_project(said, None, None).unwrap().unwrap() else { panic!("Project の View ではない") };
    g.open_project(&project, Some(&page)).unwrap();
    let app = g.app_of_project(&project).unwrap();
    let (before, _) = g.project_of(&app).unwrap().unwrap();
    assert!(before.pages.iter().all(|pg| pg.title != "Tests"), "I2 の前提: 生成は Tests を作らない");
    let edit = |g: &mut Generated, text: &str| g.page_intent(&project, ui::shell::PageIntent::Edit { text: text.into() });
    edit(&mut g, "Tests Page を足して").expect("Tests の Page を足す");
    assert!(edit(&mut g, "Tests Page を足して").is_err(), "過剰側: 2 回目は ops 0 の 1 行");
    edit(&mut g, "Excalidraw を右へ").expect("Excalidraw を右の Page へ");
    drop(g);
    // 判定は DB だけを読む（操作の口とは別の道）
    let store = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    let revs = store.list_project_revisions(&project).unwrap();
    let edits: Vec<_> = revs.iter().filter(|r| r.origin == "nl_edit").map(|r| r.ops.as_array().map(|a| a.iter().map(|o| o["op"].clone()).collect::<Vec<_>>())).collect();
    assert_eq!(edits, [Some(vec![json!("add_page")]), Some(vec![json!("move_surface")])], "I2: nl_edit の revision 2 つ・ops は頼んだ物だけ");
    let after: domain::project::ProjectDefinitionV1 = serde_json::from_value(revs.last().unwrap().json.clone()).unwrap();
    assert_eq!(after.pages.len(), before.pages.len() + 1, "Page が 1 枚増える");
    for pg in &before.pages[..2] {
        assert_eq!(after.page(&pg.id), Some(pg), "触っていない Page（Dev・Docs）は同じ");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Project の `resources` に repo の folder `ws` を足して保存する（revision は足さない = 試験の前提を置くだけ）。
fn add_repo(dir: &Path, ws: &Path, app: &str, g: &mut Generated) {
    std::fs::create_dir_all(ws).unwrap();
    let (mut p, _) = g.project_of(app).unwrap().unwrap();
    p.resources.push(domain::project::ProjectResource { id: "repo".into(), kind: "repo".into(), uri: Some(format!("file://{}", ws.display())) });
    let store = storage::Store::open(&dir.join("data").join("kangae.sqlite3")).unwrap();
    store.put(storage::Table::Projects, &p.id, None, &serde_json::to_value(&p).unwrap()).unwrap();
}

/// W11 の通しの下ごしらえ: 一言 → Project・Page の Terminal・「Tests Page を足して」・rc を読まない zsh の session を起こす。
/// 返り値は（試験の dir〈data は `dir/data`〉・Generated・Project の id・最初の Page の id・session の鍵・Tests Page の id）。
fn w11_project(tag: &str) -> (PathBuf, Generated, String, String, terminal::SessionKey, String) {
    let dir = scratch(tag);
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&dir.join("data")).with_terminal_config(rc_free);
    let app = g.create("読書記録をつけたい").unwrap();
    let View::Project { project, page } = g.project_view_of(&app).unwrap() else { panic!() };
    g.open_project(&project, None).unwrap();
    g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: page.clone() }).unwrap();
    g.page_intent(&project, ui::shell::PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap();
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    // FX6（FX-033）: Run tests は repo の folder の無い Project を断る（$HOME で走らせない）。試験の Project に repo を持たせる
    add_repo(&dir, &dir.join("repo"), &app, &mut g);
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let spec = p.terminal_specs.first().expect("terminal_specs").clone();
    let tests_id = p.pages.iter().find(|pg| pg.title == "Tests").expect("Tests Page").id.clone();
    g.ensure_terminal(&project, &spec.id).unwrap();
    (dir, g, project, page, terminal::SessionKey::of(&spec), tests_id)
}

/// W11 holdout: 人が Terminal に打っている間の AI 入力は拒否（PTY に byte 0）・わざと失敗した test の RunReceipt は失敗。
#[test]
fn stage_lease_and_failed_run_receipt() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, mut g, project, page, key, tests_id) = w11_project("w11");
    let probe_dir = dir.join("oracle");
    std::fs::create_dir_all(&probe_dir).unwrap();
    let probe = probe_dir.join("lease-probe.bin");
    let data = dir.join("data");
    let view = domain::terminal::InputView::Page { page_id: page.clone() };
    g.terminals().touch_human(&key, view.clone()).unwrap();
    let cat = format!("cat -u > {}\n", probe.display());
    g.terminals().write_as(&key, domain::terminal::InputOwner::Human, &view, cat.as_bytes()).unwrap();
    let wait = std::time::Instant::now();
    while !probe.exists() {
        assert!(wait.elapsed() < std::time::Duration::from_secs(10), "cat が lease-probe.bin を作らない");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let nonce = format!("W11N{}", storage::new_id());
    // 人はまだ打っている（cat の起動を待つ間に混雑で HUMAN_HOLD を過ぎても、ここからの 5 秒は人の物）
    g.terminals().touch_human(&key, view.clone()).unwrap();
    let err = g.terminals().write_as(&key, domain::terminal::InputOwner::Agent, &view, nonce.as_bytes()).unwrap_err();
    assert!(err.to_string().contains("人が Terminal に打っている間は"), "{err}");
    let log = g.terminals().input_log(&key).unwrap();
    assert!(!log.windows(nonce.len()).any(|w| w == nonce.as_bytes()), "denied の nonce が input_log に入った");
    let human = b"human-marker\n";
    g.terminals().write_as(&key, domain::terminal::InputOwner::Human, &view, human).unwrap();
    let wait = std::time::Instant::now();
    loop {
        let probe_bytes = std::fs::read(&probe).unwrap_or_default();
        if probe_bytes.windows(human.len()).any(|w| w == human) {
            break;
        }
        assert!(wait.elapsed() < std::time::Duration::from_secs(10), "人の byte が lease-probe.bin に届かない");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // cooked の空行で Ctrl-D = cat の EOF。shell が次の command を受けてから receipt を取る
    g.terminals().write_as(&key, domain::terminal::InputOwner::Human, &view, b"\x04").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let ready = "echo W11READY\n";
    g.terminals().write_as(&key, domain::terminal::InputOwner::Human, &view, ready.as_bytes()).unwrap();
    let wait = std::time::Instant::now();
    loop {
        let out = g.terminals().read_since(&key, 0).unwrap();
        if String::from_utf8_lossy(&out.bytes).contains("W11READY") {
            break;
        }
        assert!(wait.elapsed() < std::time::Duration::from_secs(10), "cat の後に shell が動かない");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let probe_bytes = std::fs::read(&probe).unwrap_or_default();
    assert!(!probe_bytes.windows(nonce.len()).any(|w| w == nonce.as_bytes()), "AI の nonce が PTY に届いた: {probe_bytes:?}");
    assert!(g.terminals().input_log(&key).unwrap().windows(human.len()).any(|w| w == human));
    g.terminals().release_human(&key, &view).unwrap();
    g.page_intent(&project, ui::shell::PageIntent::RunTests { command: "false".into() }).unwrap();
    // W11 本体: 「走らせる」は待たずに返る（start_run）。印が返って receipt になるまで待つ
    let wait = std::time::Instant::now();
    while let Some(s) = g.run_state(&project) {
        assert!(matches!(s, domain::terminal::RunState::Running { .. }) && wait.elapsed() < std::time::Duration::from_secs(30), "{s:?}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    g.navigate_page(&project, &tests_id).unwrap();
    let texts = page_terminal_status(&mut g, &project, &tests_id);
    // UI-2 反証の直し: 結果の語も表（copy.en.md:333・334）の EN（`Passed` / `Failed`・ICONS-1101 追記 238 から印は語の頭に描く形で字に入らない）
    assert!(texts.iter().any(|t| t == "Failed"), "Tests Page に Failed が出ない: {texts:?}");
    assert!(!texts.iter().any(|t| t == "失敗" || t == "成功"), "結果の語が日本語のまま: {texts:?}");
    // UI-2: 表（copy.en.md:334）の EN は `exit {code}`。日本語の画面の時の「内部語 exit を出さない」は、その語が表の語になったので外した
    assert!(texts.iter().any(|t| t == "exit 1"), "終了 code が別欄に出ない: {texts:?}");
    drop(g);
    let store = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    let rows = store.list_run_receipts(&project, None).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r: domain::terminal::RunReceipt = serde_json::from_value(rows[0].json.clone()).unwrap();
    assert!(!r.succeeded() && r.exit_code != 0, "{r:?}");
    assert_eq!(r.status_line(), "失敗");
    r.validate().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// W11 本体の holdout: 「走らせる」は UI の thread を待たせない（8405a44 の実機で 13 秒止まった）。走っている間は Running・2 本目は Err、
/// 印が返れば receipt（失敗は「失敗」）で run_state は None、印が返らない終わり方は receipt を作らず Unfinished（「成功」「失敗」を出さない）。
#[test]
fn stage_run_tests_returns_at_once_and_keeps_unfinished_apart() {
    use domain::terminal::RunState;
    use std::time::{Duration, Instant};
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, g, project, _page, key, _tests_id) = w11_project("w11run");
    let until = |what: &str, done: &dyn Fn() -> bool| {
        let t = Instant::now();
        while !done() {
            assert!(t.elapsed() < Duration::from_secs(30), "{what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let t0 = Instant::now();
    g.start_run(&key, "repo", "sleep 3; false", Duration::from_secs(60)).unwrap();
    assert!(t0.elapsed() < Duration::from_secs(2), "start_run が command の終わりを待った: {:?}", t0.elapsed());
    assert!(matches!(g.run_state(&project), Some(RunState::Running { .. })), "{:?}", g.run_state(&project));
    assert!(g.start_run(&key, "repo", "true", Duration::from_secs(60)).is_err(), "走っている間の 2 本目が通った");
    until("印が返っても run_state が None にならない", &|| g.run_state(&project).is_none());
    g.start_run(&key, "repo", "sleep 3", Duration::from_secs(1)).unwrap();
    until("時間切れが Unfinished にならない", &|| matches!(g.run_state(&project), Some(RunState::Unfinished { .. })));
    let Some(RunState::Unfinished { line, .. }) = g.run_state(&project) else { unreachable!() };
    assert!(!line.is_empty() && !["成功", "失敗", "Passed", "Failed"].iter().any(|w| line.contains(w)), "{line}");
    drop(g);
    let store = storage::Store::open(&dir.join("data").join("kangae.sqlite3")).unwrap();
    let rows = store.list_run_receipts(&project, None).unwrap();
    assert_eq!(rows.len(), 1, "時間切れの run に receipt ができた: {rows:?}");
    let r: domain::terminal::RunReceipt = serde_json::from_value(rows[0].json.clone()).unwrap();
    assert_eq!((r.exit_code, r.status_line().as_str()), (1, "失敗"), "{r:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// W11 の通し（E2E・本物の cargo・製品の口）: 一言 → 「Tests Page を足して」→ Tests Page に ＋ Terminal → 人が `cd <失敗する crate>` を打つ →
/// すぐ Tests Page の「走らせる」を egui の click で押す（人が打っている間の AI = 拒否の 1 行・cargo は送らない）→ 5 秒待って押し直す → frame を回し続ける。
/// 8405a44 の実機ではこの「走らせる」の後に .app が消えた（DiagnosticReports 無し = main の thread の panic で exit 101 と同じ形・stderr は捨てていた）。
/// 押した frame が 2 秒未満で返り（UI の thread で cargo を待たない）・走っている間は「走らせています」・終わると Tests Page に「失敗」と
/// 「終了 code 101」・DB の RunReceipt の 101 は cargo test の `$?`（stdout に `w11-fail` と FAILED）で、Shibubu の process の終わりではない。
#[test]
fn stage_run_tests_real_cargo_on_tests_page() {
    use eframe::egui;
    use std::time::{Duration, Instant};
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("w11cargo");
    let ws = dir.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("Cargo.toml"), "[package]\nname = \"w11fail\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    // test は 3 秒かけて落ちる（UI の thread で待つ作りなら「走らせる」の frame が 3 秒以上止まって赤）
    std::fs::write(ws.join("src/lib.rs"), "#[test]\nfn w11_fail() {\n    std::thread::sleep(std::time::Duration::from_secs(3));\n    panic!(\"w11-fail\");\n}\n").unwrap();
    let shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/bin/zsh");
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&dir.join("data")).with_terminal_config(rc_free);
    let app = g.create("読書記録をつけたい").unwrap();
    let View::Project { project, .. } = g.project_view_of(&app).unwrap() else { panic!() };
    g.open_project(&project, None).unwrap();
    g.page_intent(&project, ui::shell::PageIntent::Edit { text: "Tests Page を足して".into() }).unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let tests = p.pages.iter().find(|pg| pg.title == "Tests").expect("Tests Page").id.clone();
    // FX6（FX-033）: repo の folder の無い Project は Run tests を断る。ws を Project の repo にする
    add_repo(&dir, &ws, &app, &mut g);
    g.navigate_page(&project, &tests).unwrap();
    let placed = g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: tests.clone() });
    match shell {
        Some(v) => std::env::set_var("SHELL", v),
        None => std::env::remove_var("SHELL"),
    }
    placed.unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    assert_eq!(p.terminal_specs.len(), 1, "Terminal は 1 本");
    let spec = p.terminal_specs[0].clone();
    g.ensure_terminal(&project, &spec.id).unwrap();
    let key = terminal::SessionKey::of(&spec);
    let ctx = egui::Context::default();
    ui::theme::install(&ctx, egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 1600.0));
    // 1 frame: Page を描き、返った intent と AX の (名前か値, bounds) を返す（試験の側が実機で読む口と同じ）
    let frame = |g: &mut Generated, events: Vec<egui::Event>| {
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        let mut intent = None;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                intent = g.draw_page(ui, &project, &tests);
            });
        });
        let nodes: Vec<(String, Option<egui::accesskit::Rect>)> = out
            .platform_output
            .accesskit_update
            .map(|u| u.nodes.iter().map(|(_, n)| (n.label().or(n.value()).unwrap_or("").to_owned(), n.bounds())).collect())
            .unwrap_or_default();
        (intent, nodes)
    };
    // 「走らせる」を click（押した frame・離した frame）→ 返った intent を app.rs と同じく page_intent に渡す。渡すのにかかった時間を返す
    let press_run = |g: &mut Generated| {
        let (_, nodes) = frame(g, vec![]);
        let b = nodes.iter().find(|(t, _)| t == "Run tests").and_then(|(_, b)| *b).expect("「走らせる」の AX の bounds");
        let at = egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(g, vec![egui::Event::PointerMoved(at), button(true)]);
        let (intent, _) = frame(g, vec![button(false)]);
        let intent = intent.expect("「走らせる」が intent を返さない");
        let ui::shell::Intent::Page(op) = intent else { panic!("Page の intent ではない") };
        assert!(matches!(op, ui::shell::PageIntent::RunTests { .. }), "{op:?}");
        let t0 = Instant::now();
        let r = g.page_intent(&project, op);
        (r, t0.elapsed())
    };
    frame(&mut g, vec![]);
    frame(&mut g, vec![]); // 1 frame 目で grid の列を決めて resize
    // 人が打つ（rc を読まない zsh なので cargo の dir を PATH に足す・cargo は試験を走らせている物）
    let cargo_dir = Path::new(env!("CARGO")).parent().unwrap().to_owned();
    g.terminals().write(&key, format!("export PATH={}:$PATH; cd {}\n", cargo_dir.display(), ws.display()).as_bytes()).unwrap();
    // FX6（FX-033）: Run tests は専用の session（`tests.{project}`）で走る。人が自分の Terminal に打っている間も拒否されず、cargo は人の PTY に送らない。
    // 専用の session も rc を読まない zsh なので、cargo の dir を PATH に足す（この打鍵は人の物 = 5 秒は AI が打てない）。作業 folder は Project の repo（= ws を cd 済みの cwd）
    let tests_spec = domain::project::TerminalSessionSpec { id: format!("tests.{project}"), project_id: project.clone(), shell: "/bin/zsh".into(), working_directory: ws.display().to_string(), keep_alive_on_page_switch: true, command: None };
    g.terminals().spawn(&tests_spec, terminal::TermSize::default()).unwrap();
    let tests_key = terminal::SessionKey::of(&tests_spec);
    g.terminals().write(&tests_key, format!("export PATH={}:$PATH\n", cargo_dir.display()).as_bytes()).unwrap();
    let typed = Instant::now();
    let failed = |nodes: &[(String, Option<egui::accesskit::Rect>)]| nodes.iter().filter(|(t, _)| t.contains("Failed")).map(|(t, _)| t.clone()).collect::<Vec<_>>();
    let (_, nodes) = frame(&mut g, vec![]);
    assert!(failed(&nodes).is_empty(), "走らせる前に「失敗しました」が出た: {:?}", failed(&nodes));
    // 打つのを止めて 5 秒たったら押し直す
    std::thread::sleep(domain::terminal::InputLease::HUMAN_HOLD.saturating_sub(typed.elapsed()) + Duration::from_millis(300));
    let echoed = String::from_utf8_lossy(&g.terminals().read_since(&key, 0).unwrap().bytes).into_owned();
    assert!(!echoed.contains("cargo test"), "拒否した「走らせる」の cargo test が PTY に届いた: {echoed:?}");
    let (ran, took) = press_run(&mut g);
    ran.expect("5 秒後の「走らせる」");
    // 走っている間の 2 本目も拒否の 1 行で、「失敗しました」ではない（test は 3 秒かけて落ちるので、まだ走っている）
    let (busy, _) = press_run(&mut g);
    let busy = busy.expect_err("走っている間の 2 本目が通った");
    let (_, nodes) = frame(&mut g, vec![]);
    assert!(busy.contains("is running") && nodes.iter().any(|(t, _)| t.contains(&busy)), "2 本目の拒否の 1 行が Page の AX に出ない: {busy}");
    assert!(failed(&nodes).is_empty(), "走っている間の 2 本目を「失敗しました」と出した: {:?}", failed(&nodes));
    eprintln!("W11 cargo: 「走らせる」の page_intent {took:?}");
    let (mut running_seen, mut worst, start) = (false, Duration::ZERO, Instant::now());
    let texts = loop {
        let t0 = Instant::now();
        let (_, nodes) = frame(&mut g, vec![]);
        worst = worst.max(t0.elapsed());
        let texts: Vec<String> = nodes.into_iter().map(|(t, _)| t).collect();
        running_seen |= texts.iter().any(|t| t.contains("Running"));
        if texts.iter().any(|t| t == "Failed") {
            break texts;
        }
        assert!(start.elapsed() < Duration::from_secs(600), "cargo test の結果が Tests Page に出ない: {:?}", texts.iter().filter(|t| t.contains("Running") || t.contains("result") || t.contains("finish")).collect::<Vec<_>>());
        std::thread::sleep(Duration::from_millis(50));
    };
    eprintln!("W11 cargo: 結果まで {:?}・一番長い frame {worst:?}", start.elapsed());
    assert!(took < Duration::from_secs(2), "「走らせる」が UI の thread で cargo を待った: {took:?}");
    assert!(running_seen, "走っている間に「走らせています」が出ない");
    assert!(texts.iter().any(|t| t == "exit 101"), "終了 code が別欄に出ない: {texts:?}");
    assert!(!texts.iter().any(|t| t == "Passed" || t == "成功"), "失敗の run に Passed: {texts:?}");
    for _ in 0..3 {
        frame(&mut g, vec![]); // 結果を描いた後の frame も panic しない（8405a44 はここで落ちた形）
    }
    drop(g);
    let store = storage::Store::open(&dir.join("data").join("kangae.sqlite3")).unwrap();
    let rows = store.list_run_receipts(&project, None).unwrap();
    assert_eq!(rows.len(), 1, "拒否した run に receipt ができた・または走らせた run に無い: {rows:?}");
    let r: domain::terminal::RunReceipt = serde_json::from_value(rows[0].json.clone()).unwrap();
    assert_eq!((r.exit_code, r.input.as_str(), r.lease_holder), (101, "cargo test", domain::terminal::InputOwner::Agent), "{r:?}");
    assert!(r.stdout.contains("w11-fail") && r.stdout.contains("FAILED"), "101 は cargo test の失敗の `$?`: {}", r.stdout);
    let _ = std::fs::remove_dir_all(&dir);
}

// W14: 試験の側の oracle と L1 の判定（std と serde_json だけ）を製品の画面に当てる
#[path = "common/readback.rs"]
mod readback;

/// 試験の側の目（L1 用）: Page を egui で 2 frame 描いた AccessKit の木を、親子のまま `readback::AxText` にする（実機では cua-driver の AX の木）。
/// 文字は label と value をつなぐ（どちらに成功の語が出ても見る）。
/// UI-5a: 帯の menu（`opener` = `Page options` か Page の右下の `Add to page`）を AX の Click で開いた後の文字（label と value）。帯の操作は menu の中なので、
/// BG の driver と同じ道（AX の Click）で開いて読む。
fn menu_texts(g: &mut Generated, project: &str, page: &str, opener: &str) -> Vec<String> {
    use eframe::egui::accesskit::{Action, ActionRequest};
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let (mut events, mut opened, mut texts) = (Vec::new(), false, Vec::new());
    for _ in 0..3 {
        let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 900.0))), events: std::mem::take(&mut events), ..Default::default() };
        let out = ctx.run(input, |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                g.draw_page(ui, project, page);
            });
        });
        let update = out.platform_output.accesskit_update.expect("AccessKit の木");
        texts = update.nodes.iter().flat_map(|(_, n)| [n.label(), n.value()]).flatten().map(str::to_owned).collect();
        if let Some((id, _)) = update.nodes.iter().find(|(_, n)| n.label() == Some(opener)).filter(|_| !opened) {
            events = vec![eframe::egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Click, target: *id, data: None })];
            opened = true;
        }
    }
    texts
}

fn page_ax_text(g: &mut Generated, project: &str, page: &str) -> readback::AxText {
    use eframe::egui::accesskit::{Node, NodeId};
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let mut out = None;
    for _ in 0..2 {
        let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 900.0))), ..Default::default() };
        out = Some(ctx.run(input, |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                g.draw_page(ui, project, page);
            });
        }));
    }
    let update = out.unwrap().platform_output.accesskit_update.expect("AccessKit の木");
    let nodes: std::collections::HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    fn build(id: NodeId, nodes: &std::collections::HashMap<NodeId, &Node>) -> readback::AxText {
        let n = nodes[&id];
        let text = [n.label(), n.value()].into_iter().flatten().collect::<Vec<_>>().join(" ");
        readback::AxText {
            identifier: n.author_id().map(str::to_owned),
            text: (!text.is_empty()).then_some(text),
            children: n.children().iter().filter(|c| nodes.contains_key(c)).map(|c| build(*c, nodes)).collect(),
        }
    }
    build(update.tree.expect("木の根").root, &nodes)
}

/// `surface-status:<id>` の (id, value)（L1 ①）。
fn surface_status_values(t: &readback::AxText, out: &mut Vec<(String, String)>) {
    if let Some(id) = t.identifier.as_deref().and_then(|a| a.strip_prefix("surface-status:")) {
        let value = t.text.as_deref().and_then(|s| s.rsplit(' ').next()).unwrap_or("").to_owned();
        out.push((id.to_owned(), value));
    }
    for c in &t.children {
        surface_status_values(c, out);
    }
}

/// 試験の repo の今（操作とは別の道 = git と disk）: HEAD・`git status`・`.git` の外の全 file の中身。
fn repo_state(ws: &Path) -> serde_json::Value {
    let git = |args: &[&str]| String::from_utf8_lossy(&Command::new("git").arg("-C").arg(ws).args(args).output().unwrap().stdout).into_owned();
    fn walk(root: &Path, p: &Path, out: &mut serde_json::Map<String, serde_json::Value>) {
        for e in std::fs::read_dir(p).unwrap().flatten() {
            let path = e.path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.insert(rel, json!(String::from_utf8_lossy(&std::fs::read(&path).unwrap())));
            }
        }
    }
    let mut files = serde_json::Map::new();
    walk(ws, ws, &mut files);
    json!({ "head": git(&["rev-parse", "HEAD"]), "status": git(&["status", "--porcelain", "--untracked-files=all"]), "files": files })
}

/// W14（YCM-060・L1 を製品の画面に）: 生成だけでは外へ送らず消さない —— 試験の repo（git）の HEAD・`git status`・全 file の中身と、
/// DB の RunReceipt（Terminal で走らせた記録）が一言の前後で同じ。生成した Project の全 Page を描いた AX の木で、成功の語が
/// 面に紐づかない所に出ず（L1 ②・この試験で oracle が verified と言った面は 0）、`surface-status:*` の値が verified でない（L1 ①）。
/// 未測: 外の service の数（B4 の GitHub の comment）は試験から読まない・act の記録の口は製品に未だ無い（W19-1）。
/// 負の対照: ① `surface_status_line` の Unavailable の行に「接続済み」を混ぜると L1 ② が赤 ② `create_project` の中で workspace に
/// file を 1 つ書くと 060 が赤（同じ run の正の対照: 試験が file を 1 つ書くと Oracle が changed）。
#[test]
fn w14_generation_alone_changes_nothing_outside_and_pages_claim_no_success() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("w14");
    let (data, ws) = (dir.join("data"), dir.join("repo"));
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("README.md"), "# w14\n").unwrap();
    let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&ws).args(["-c", "user.name=w14", "-c", "user.email=w14@example.invalid"]).args(args).status().unwrap().success(), "git {args:?}");
    git(&["init", "-q"]);
    git(&["add", "README.md"]);
    git(&["commit", "-qm", "w14"]);
    let before = repo_state(&ws);
    let mut g = Generated::open(&data);
    let said = format!("Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. n-{}", storage::new_id());
    let View::Project { project, .. } = g.create_project(&said, Some(&ws), None).unwrap().expect("主デモの一言は Project") else { panic!() };
    let o = readback::Oracle::take("w14_repo", before.clone(), repo_state(&ws));
    assert!(!o.changed(), "生成だけで試験の repo が変わった: {:?}", o.diff);
    std::fs::write(ws.join("w14-probe.txt"), "x").unwrap();
    assert!(readback::Oracle::take("w14_repo", before, repo_state(&ws)).changed(), "正の対照: repo に 1 file 足すと oracle が changed");
    std::fs::remove_file(ws.join("w14-probe.txt")).unwrap();

    let app = g.app_of_project(&project).unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let mut statuses = Vec::new();
    for page in &p.pages {
        let tree = page_ax_text(&mut g, &project, &page.id);
        readback::l1_words(&tree, &[]).unwrap_or_else(|e| panic!("L1 ②（Page {}）: {e}", page.title));
        surface_status_values(&tree, &mut statuses);
    }
    let mut ids: Vec<&str> = statuses.iter().map(|(id, _)| id.as_str()).collect();
    ids.sort();
    let mut specs: Vec<&str> = p.surface_specs.iter().map(|s| s.id.as_str()).collect();
    specs.sort();
    assert_eq!(ids, specs, "面ごとに surface-status が 1 つ（測った数 0 の緑にしない）");
    for (id, value) in &statuses {
        readback::l1_status(id, value, false).unwrap_or_else(|e| panic!("L1 ①: {e}"));
    }
    drop(g);
    let store = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    assert!(store.list_run_receipts(&project, None).unwrap().is_empty(), "生成だけで Terminal の run が走った");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `usable:<seat>` の (id, value)（W14b）。
fn usable_values(t: &readback::AxText, out: &mut Vec<(String, String)>) {
    if let Some(id) = t.identifier.as_deref().and_then(|a| a.strip_prefix("usable:")) {
        let value = t.text.as_deref().and_then(|s| s.rsplit(' ').next()).unwrap_or("").to_owned();
        out.push((id.to_owned(), value));
    }
    for c in &t.children {
        usable_values(c, out);
    }
}

/// W14b（DESIGN §16.3 規則 8・27 通目）: 主デモの一言の Project の 3 面と、読書記録の app の「使える / 理由の 1 行」を、画面の AX の木（`usable:<seat>`）で読む。
/// この試験の環境（bridge 無し・WebView の無い build・rc を読まない zsh）での正の答え: Terminal = ready（本物の PTY・印は temp の file・disk で読み戻す）・
/// Docs（TextEdit）= 要接続（bridge が答えないので在るかを確かめていない）・Diagram（Excalidraw）= 入れる（この版に Web の面が無い）・読書記録 = ready（別の session で 1 件入れて DB で読み戻す）。
/// 評価は利用者の物に書かない: 試験の repo（HEAD・status・全 file）・RunReceipt・Terminal の記録・読書記録の利用者の session の中身が、評価の前後で同じ（同じ run の正の対照: repo に 1 file 足すと changed）。
/// 面を測った数 0 の緑にしない（AX の席の数 = surface_specs + terminal_specs の数）。L1 ②（成功の語）も同じ木で見る。
#[test]
fn w14b_main_demo_three_faces_and_reading_log_say_usable_or_why() {
    use std::time::{Duration, Instant};
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("w14b");
    let (data, ws) = (dir.join("data"), dir.join("repo"));
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("README.md"), "# w14b\n").unwrap();
    let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&ws).args(["-c", "user.name=w14b", "-c", "user.email=w14b@example.invalid"]).args(args).status().unwrap().success(), "git {args:?}");
    git(&["init", "-q"]);
    git(&["add", "README.md"]);
    git(&["commit", "-qm", "w14b"]);
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&data).with_terminal_config(rc_free);
    let said = format!("Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. n-{}", storage::new_id());
    let View::Project { project, .. } = g.create_project(&said, Some(&ws), None).unwrap().expect("主デモの一言は Project") else { panic!() };
    let before = repo_state(&ws);
    let app = g.app_of_project(&project).unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    // 全 Page を描き、Terminal の席（裏の thread）が checking でなくなるまで読む
    let read_all = |g: &mut Generated| {
        let mut vals = Vec::new();
        for page in &p.pages {
            let tree = page_ax_text(g, &project, &page.id);
            readback::l1_words(&tree, &[]).unwrap_or_else(|e| panic!("L1 ②（Page {}）: {e}", page.title));
            usable_values(&tree, &mut vals);
        }
        vals.sort();
        vals
    };
    let start = Instant::now();
    let vals = loop {
        let vals = read_all(&mut g);
        if !vals.iter().any(|(_, v)| v == "checking") {
            break vals;
        }
        assert!(start.elapsed() < Duration::from_secs(90), "Terminal の席が 90 秒たっても checking のまま: {vals:?}");
        std::thread::sleep(Duration::from_millis(100));
    };
    let term = p.terminal_specs.first().expect("repo を渡したので Terminal の spec").id.clone();
    let mut want = vec![(term.clone(), "ready".to_owned()), ("diagram".to_owned(), "not_ready".to_owned()), ("docs".to_owned(), "not_ready".to_owned())];
    want.sort();
    assert_eq!(vals, want, "面ごとに usable が 1 つ・Terminal だけ使える");
    let lines: std::collections::BTreeMap<String, String> = g.usable_of(&project).into_iter().map(|u| (u.seat.id.clone(), u.line())).collect();
    assert_eq!(lines[&term], "Terminal works (Read back “Type one line” from a file on disk)", "{lines:?}");
    assert!(lines["docs"].contains("Couldn't find TextEdit"), "{lines:?}");
    assert!(lines["diagram"].contains("This version can't show websites here"), "{lines:?}");
    // 評価は利用者の物に書かない
    let o = readback::Oracle::take("w14b_repo", before.clone(), repo_state(&ws));
    assert!(!o.changed(), "評価で試験の repo が変わった: {:?}", o.diff);
    std::fs::write(ws.join("w14b-probe.txt"), "x").unwrap();
    assert!(readback::Oracle::take("w14b_repo", before, repo_state(&ws)).changed(), "正の対照: repo に 1 file 足すと oracle が changed");
    std::fs::remove_file(ws.join("w14b-probe.txt")).unwrap();
    assert!(g.terminals().list(None).is_empty(), "評価の PTY が利用者の Terminal の manager に入った");
    // 読書記録: 1 Page の app（包んだ Project）の席
    let book = g.create("読書記録をつけたい").unwrap();
    let View::Project { project: bp, page: bpage } = g.project_view_of(&book).unwrap() else { panic!() };
    g.open_project(&bp, None).unwrap();
    let mut bvals = Vec::new();
    usable_values(&page_ax_text(&mut g, &bp, &bpage), &mut bvals);
    assert_eq!(bvals, [(book.clone(), "ready".to_owned())], "読書記録の席");
    let bline = g.usable_of(&bp).into_iter().map(|u| u.line()).collect::<Vec<_>>();
    assert!(bline[0].ends_with(" works (Read back “Add one item” from Shibubu's database)"), "{bline:?}");
    let (def, _, h) = g.open_app(&book).unwrap();
    let add = def.actions.iter().find(|a| a.id.ends_with(".add")).unwrap().id.clone();
    let source = add.rsplit_once('.').unwrap().0;
    assert_eq!(h.data(&domain::binding::DataBinding { source_ref: source.into(), path: "items".into(), extra: Default::default() }), None, "評価の 1 件が利用者の timeline に出た");
    drop(g);
    let store = storage::Store::open(&data.join("kangae.sqlite3")).unwrap();
    assert!(store.list_run_receipts(&project, None).unwrap().is_empty(), "評価で Terminal の run の記録ができた");
    let sessions: Vec<String> = store.list(storage::Table::TerminalSessions, None).unwrap().into_iter().map(|r| r.id).collect();
    assert!(!sessions.iter().any(|s| s.starts_with("usable.")), "評価の Terminal が記録に残った: {sessions:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// W14b の BG で見つけた（release の .app が Dev → Docs で accesskit_consumer の tree.rs:306 の unwrap で落ちた）: 同じ egui の窓で Page を移った時、
/// AccessKit の node の id が前の frame と違う親の下に出る（= 移った node）と、consumer は「更新した node」と「親から外れた node」を同じ update で受け、
/// 消した node を引いて panic する。Page の Dev → Docs → Diagram → Dev を 1 つの Context で描き、続く 2 frame の両方に在る node の親が同じかを見る。
#[test]
fn w14b_page_switch_never_moves_an_accesskit_node_to_another_parent() {
    use eframe::egui::accesskit::NodeId;
    use std::collections::HashMap;
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("w14b-ax");
    let ws = dir.join("repo");
    std::fs::create_dir_all(&ws).unwrap();
    assert!(Command::new("git").arg("-C").arg(&ws).args(["init", "-q"]).status().unwrap().success());
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&dir.join("data")).with_terminal_config(rc_free);
    let View::Project { project, .. } = g.create_project("この repo の開発環境を作って", Some(&ws), None).unwrap().expect("Project") else { panic!() };
    let app = g.app_of_project(&project).unwrap();
    let (p, _) = g.project_of(&app).unwrap().unwrap();
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let mut prev: Option<(String, HashMap<NodeId, NodeId>)> = None;
    let mut moved = Vec::new();
    let order: Vec<&str> = p.pages.iter().map(|pg| pg.id.as_str()).chain([p.pages[0].id.as_str()]).collect();
    for page in order {
        for _ in 0..3 {
            let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 900.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                eframe::egui::CentralPanel::default().show(ctx, |ui| {
                    g.draw_page(ui, &project, page);
                });
            });
            let update = out.platform_output.accesskit_update.expect("AccessKit の木");
            let parents: HashMap<NodeId, NodeId> = update.nodes.iter().flat_map(|(id, n)| n.children().iter().map(move |c| (*c, *id))).collect();
            if let Some((from, before)) = &prev {
                for (child, parent) in &parents {
                    if let Some(old) = before.get(child).filter(|old| *old != parent) {
                        moved.push(format!("{from} → {page}: node {child:?} の親が {old:?} から {parent:?} へ"));
                    }
                }
            }
            prev = Some((page.to_owned(), parents));
        }
    }
    assert!(moved.is_empty(), "Page を移ると AccessKit の node が別の親へ移る（consumer が落ちる形）: {}", moved.iter().take(5).cloned().collect::<Vec<_>>().join(" / "));
    drop(g);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 部分木の identifier（`<scope>:<id>`）を全部。
fn ax_ids(t: &readback::AxText, out: &mut Vec<String>) {
    if let Some(i) = &t.identifier {
        out.push(i.clone());
    }
    for c in &t.children {
        ax_ids(c, out);
    }
}

fn ax_find<'a>(t: &'a readback::AxText, id: &str) -> Option<&'a readback::AxText> {
    if t.identifier.as_deref() == Some(id) {
        return Some(t);
    }
    t.children.iter().find_map(|c| ax_find(c, id))
}

/// W13c（29 通目）: 主デモの一言の Project は既定で「開発環境」の型 = Dev の main に Terminal・side に Git の欄（画面の AX の木の `slot:<区画>` の下で読む）。
/// 「配置」で型を替えると置き方だけが替わり（音楽制作 = Terminal は bottom）、型なしで今の並び。Project の hash と revision は替えても同じ・再起動の後も同じ型。
/// 型を替えた frame の前後で、同じ AX の node が別の親へ移らない（W14b の BG の panic と同じ形を作らない）。
#[test]
fn w13c_main_demo_dev_page_follows_the_dev_template_and_the_choice_persists() {
    use eframe::egui::accesskit::NodeId;
    use std::collections::HashMap;
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("w13c");
    let (data, ws) = (dir.join("data"), dir.join("repo"));
    std::fs::create_dir_all(&ws).unwrap();
    assert!(Command::new("git").arg("-C").arg(&ws).args(["init", "-q"]).status().unwrap().success());
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&data).with_terminal_config(rc_free.clone());
    let said = "Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw.";
    let View::Project { project, page: dev } = g.create_project(said, Some(&ws), None).unwrap().expect("Project") else { panic!() };
    g.open_project(&project, None).unwrap();
    g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: dev.clone() }).unwrap();
    let app = g.app_of_project(&project).unwrap();
    let (p0, hash0) = g.project_of(&app).unwrap().unwrap();
    let term = p0.terminal_specs.iter().find(|t| p0.pages[0].root.nodes().iter().any(|n| n.props.get("terminal_ref").and_then(|v| v.as_str()) == Some(t.id.as_str()))).expect("Dev に置いた Terminal").id.clone();
    let under = |t: &readback::AxText, slot: &str| {
        let mut ids = Vec::new();
        if let Some(s) = ax_find(t, &format!("slot:{slot}")) {
            ax_ids(s, &mut ids);
        }
        ids
    };
    let tree = page_ax_text(&mut g, &project, &dev);
    let main = under(&tree, "main");
    let side = under(&tree, "side");
    assert!(main.contains(&format!("terminal:{term}")), "Dev の main に Terminal が無い: {main:?}");
    assert!(["dev-git-branch", "dev-git-sha", "dev-git-changes"].iter().all(|k| side.contains(&format!("data:{k}"))), "Dev の side に Git の欄が無い: {side:?}");
    assert!(!side.iter().any(|i| i.starts_with("terminal:")), "side に Terminal: {side:?}");
    // UI-5a: 型の名前は ••• の menu の `Layout: {name}`（帯の文字から menu へ）
    let menu = menu_texts(&mut g, &project, &dev, "Page options");
    assert!(menu.iter().any(|s| s.contains("Layout: Development")), "既定の型の名前が出ない: {menu:?}");
    // 型を替える: 開発環境 → 音楽制作（Terminal は bottom）→ タスク管理 → なし（今の並び）。1 つの Context で描き、AX の node の親が移らない事も見る
    let ctx = eframe::egui::Context::default();
    ui::theme::install(&ctx, eframe::egui::ThemePreference::Light);
    ctx.enable_accesskit();
    let mut prev: Option<HashMap<NodeId, NodeId>> = None;
    let mut moved = Vec::new();
    let mut frame = |g: &mut Generated| {
        for _ in 0..2 {
            let input = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1440.0, 900.0))), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                eframe::egui::CentralPanel::default().show(ctx, |ui| {
                    g.draw_page(ui, &project, &dev);
                });
            });
            let update = out.platform_output.accesskit_update.unwrap();
            let parents: HashMap<NodeId, NodeId> = update.nodes.iter().flat_map(|(id, n)| n.children().iter().map(move |c| (*c, *id))).collect();
            if let Some(before) = &prev {
                moved.extend(parents.iter().filter(|(c, pa)| before.get(c).is_some_and(|old| old != *pa)).map(|(c, _)| format!("{c:?}")));
            }
            prev = Some(parents);
        }
    };
    frame(&mut g);
    let mut seen = Vec::new();
    for want in ["Music", "Tasks", "None", "Development"] {
        g.page_intent(&project, ui::shell::PageIntent::Placement).unwrap();
        frame(&mut g);
        let tree = page_ax_text(&mut g, &project, &dev);
        let menu = menu_texts(&mut g, &project, &dev, "Page options");
        assert!(menu.iter().any(|s| s.contains(&format!("Layout: {want}"))), "「配置」を押した後が {want} でない: {menu:?}");
        let mut ids = Vec::new();
        ax_ids(&tree, &mut ids);
        let slots: Vec<&String> = ids.iter().filter(|i| i.starts_with("slot:")).collect();
        match want {
            "Music" => assert!(under(&tree, "bottom").contains(&format!("terminal:{term}")), "音楽制作の型で Terminal が bottom でない: {slots:?}"),
            "なし" => assert!(slots.is_empty(), "型なしなのに区画: {slots:?}"),
            _ => {}
        }
        seen.push(want);
        let (p, hash) = g.project_of(&app).unwrap().unwrap();
        assert_eq!((p.revision, &hash), (p0.revision, &hash0), "型を替えて Project が変わった（{want}）");
    }
    assert!(moved.is_empty(), "型を替えると AX の node が別の親へ移る（consumer が落ちる形）: {moved:?}");
    // 再起動の後も選んだ型（いまは開発環境に戻した）。音楽制作に替えてから開き直す
    g.page_intent(&project, ui::shell::PageIntent::Placement).unwrap();
    drop(g);
    let g = Generated::open(&data).with_terminal_config(rc_free);
    assert_eq!(g.placement(&project).unwrap().map(|t| t.id), Some("music".into()), "再起動で型の選択が消えた");
    drop(g);
    let _ = std::fs::remove_dir_all(&dir);
}

/// W14b の直し（W13c の BG で見つけた）: 「＋ Terminal」で後から足した Terminal の席も、次に描いた時に測る（前は Project ごとに 1 回だけ測り、
/// 足した席が「確かめています: Terminal を起こしています」のまま残った = 測っていないのに測っていると言った）。前に測った席の答えは残す。
#[test]
fn w14b_terminal_added_later_is_measured_too() {
    use std::time::{Duration, Instant};
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("w14b-later");
    let ws = dir.join("repo");
    std::fs::create_dir_all(&ws).unwrap();
    assert!(Command::new("git").arg("-C").arg(&ws).args(["init", "-q"]).status().unwrap().success());
    let rc_free = terminal::TerminalConfig { shell_args: vec!["-f".into(), "-i".into()], ..Default::default() };
    let mut g = Generated::open(&dir.join("data")).with_terminal_config(rc_free);
    let View::Project { project, page: dev } = g.create_project("この repo の開発環境を作って", Some(&ws), None).unwrap().expect("Project") else { panic!() };
    let wait_ready = |g: &mut Generated, n: usize| {
        let start = Instant::now();
        loop {
            let mut vals = Vec::new();
            usable_values(&page_ax_text(g, &project, &dev), &mut vals);
            // Dev の Page の席は Terminal だけ（殻の app は席にならない・面は Docs と Diagram の Page）
            if vals.len() >= n && vals.iter().all(|(_, v)| v == "ready") {
                return vals;
            }
            assert!(start.elapsed() < Duration::from_secs(90), "Terminal の席が 90 秒たっても ready にならない: {vals:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let first = wait_ready(&mut g, 1);
    g.page_intent(&project, ui::shell::PageIntent::AddTerminal { page: dev.clone() }).unwrap();
    let both = wait_ready(&mut g, 2);
    assert_eq!(both.len(), first.len() + 1, "足した Terminal の席が増えていない: {first:?} → {both:?}");
    assert!(g.usable_of(&project).iter().filter(|u| u.seat.kind == composer::verifier::SeatKind::Terminal).all(|u| u.ready()), "{:?}", g.usable_of(&project));
    drop(g);
    let _ = std::fs::remove_dir_all(&dir);
}
