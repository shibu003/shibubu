//! W2c: 試験の側の oracle と L1〜L4 の判定（tests/common/readback.rs）が、偽の成功を赤にし本物を通す事。
//! common/readback.rs は std と serde_json だけに依るので、ui を引かずにここで単独に compile する。
#[path = "common/readback.rs"]
mod readback;

use readback::*;
use serde_json::json;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn node(id: Option<&str>, text: Option<&str>, children: Vec<AxText>) -> AxText {
    AxText {
        identifier: id.map(str::to_string),
        text: text.map(str::to_string),
        children,
    }
}

#[test]
fn oracle_diff_lists_each_changed_pointer() {
    let o = Oracle::take(
        "file",
        json!({"sha": "a", "n": [1, 2], "keep": true}),
        json!({"sha": "b", "n": [1], "new/x": 1, "keep": true}),
    );
    assert!(o.changed());
    assert_eq!(
        o.diff,
        vec![
            "/n/1: 2 -> (無い)",
            "/new~1x: (無い) -> 1",
            "/sha: \"a\" -> \"b\""
        ]
    );
    let same = Oracle::take("file", json!({"sha": "a"}), json!({"sha": "a"}));
    assert!(!same.changed(), "{:?}", same.diff);
    assert_eq!(
        Oracle::take("v", json!(1), json!(2)).diff,
        vec!["/: 1 -> 2"]
    );
}

#[test]
fn l1_status_rejects_a_verified_display_the_oracle_did_not_confirm() {
    assert!(l1_status("docs", "verified", false).is_err());
    assert!(l1_status("docs", "接続済み", false).is_err());
    // UI-2: 表の読み戻していない保存の語は主張ではない・同じ行に別の成功の語があれば赤のまま
    assert!(l1_status("docs", "Saved, not checked · docs/arch.excalidraw", false).is_ok());
    assert!(l1_status("docs", "Saved · docs/arch.excalidraw", false).is_err());
    assert!(l1_status("docs", "Saved, not checked · ✓ Done", false).is_err());
    assert!(l1_status("docs", "verified", true).is_ok());
    assert!(l1_status("docs", "unverified", false).is_ok());
    assert!(
        l1_status("docs", "unavailable", true).is_ok(),
        "控えめな表示は嘘ではない"
    );
}

#[test]
fn l1_words_allows_success_words_only_in_a_verified_surface_status() {
    let tree = |status_id: &str, extra: AxText| {
        node(
            None,
            None,
            vec![
                node(
                    Some(&format!("surface-status:{status_id}")),
                    None,
                    vec![node(None, Some("保存しました"), vec![])],
                ),
                node(
                    Some("terminal:dev"),
                    None,
                    vec![node(None, Some("Done"), vec![])],
                ),
                node(Some("data:books"), Some("完了した本"), vec![]),
                node(Some("webview:diagram"), Some("Saved"), vec![]),
                extra,
            ],
        )
    };
    let quiet = node(None, Some("Excalidraw"), vec![]);
    assert!(l1_words(&tree("docs", quiet.clone()), &["docs"]).is_ok());
    let e = l1_words(&tree("docs", quiet.clone()), &[]).unwrap_err();
    assert!(
        e.contains("surface-status:docs") && e.contains("保存しました"),
        "{e}"
    );
    let e = l1_words(
        &tree("docs", node(Some("header"), Some("✓ 接続済み"), vec![])),
        &["docs"],
    )
    .unwrap_err();
    assert!(e.contains("header"), "{e}");
    // toast も同じ扱い・別の surface の verified では通さない
    let t = node(
        None,
        None,
        vec![node(Some("toast:docs"), Some("Saved"), vec![])],
    );
    assert!(l1_words(&t, &["docs"]).is_ok());
    assert!(l1_words(&t, &["diagram"]).is_err());
}

#[test]
fn l2_needs_an_effect_or_a_refusal_and_ignores_volatile_nodes() {
    let page = |text: &str, clock: &str| {
        node(
            Some("page:dev"),
            None,
            vec![
                node(Some("data:readme"), Some(text), vec![]),
                node(Some("time:clock"), Some(clock), vec![]),
            ],
        )
    };
    let (before, after_same, after_new) =
        (page("a", "10:00"), page("a", "10:01"), page("a b", "10:01"));
    assert!(l2_effect("保存", None, "page:dev", &before, &after_new, None).is_ok());
    let e = l2_effect("保存", None, "page:dev", &before, &after_same, None).unwrap_err();
    assert!(e.contains("保存") && e.contains("何も起きない"), "{e}");
    assert!(l2_effect(
        "保存",
        None,
        "page:dev",
        &before,
        &after_same,
        Some("許可がありません")
    )
    .is_ok());
    assert!(
        l2_effect("保存", None, "page:dev", &before, &after_same, Some("  ")).is_err(),
        "空の拒否は拒否でない"
    );
    let changed = Oracle::take("sha", json!("x"), json!("y"));
    let still = Oracle::take("sha", json!("x"), json!("x"));
    assert!(l2_effect(
        "保存",
        Some(&changed),
        "page:dev",
        &before,
        &after_same,
        None
    )
    .is_ok());
    assert!(l2_effect("保存", Some(&still), "page:dev", &before, &after_same, None).is_err());
    // target が現れた・消えたのも効果
    let empty = node(None, None, vec![]);
    assert!(l2_effect("開く", None, "page:dev", &empty, &before, None).is_ok());
}

/// 本物の tty を持つ zsh を `script` の下に起こし、(script の pid, zsh の pid) を返す。
fn zsh_on_a_pty() -> (std::process::Child, u32) {
    let child = Command::new("/usr/bin/script")
        .args(["-q", "/dev/null", "/bin/zsh", "-f", "-c", "sleep 20; :"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = Command::new("/usr/bin/pgrep")
            .args(["-P", &child.id().to_string()])
            .output()
            .unwrap();
        if let Some(pid) = String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
        {
            return (child, pid);
        }
        assert!(Instant::now() < deadline, "script の下に zsh が起きない");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn l3_needs_nonce_product_parent_tty_and_a_shell() {
    let (mut script, zsh) = zsh_on_a_pty();
    let parent = script.id();
    let out = "n-7f3a\n$ echo $((6*7))\n42\n";
    let ok = l3_terminal(out, "n-7f3a", 42, zsh, parent);
    let no_nonce = l3_terminal("42\n", "n-7f3a", 42, zsh, parent);
    let no_product = l3_terminal("n-7f3a 420\n", "n-7f3a", 42, zsh, parent);
    let wrong_parent = l3_terminal(out, "n-7f3a", 42, zsh, std::process::id());
    // 積の数字が nonce や打った式の中にだけ在る出力は赤（積は自分の行に出ていない）
    let product_only_in_nonce = l3_terminal("n-42ab\n$ echo $((6*7))\n", "n-42ab", 42, zsh, parent);
    let product_only_in_command = l3_terminal("n-7f3a\n$ echo 42 $((6*7))\n", "n-7f3a", 42, zsh, parent);
    let _ = script.kill();
    let _ = script.wait();
    assert_eq!(ok, Ok(()));
    assert!(no_nonce.unwrap_err().contains("nonce"));
    assert!(no_product.unwrap_err().contains("積 42"));
    assert!(wrong_parent.unwrap_err().contains("親は"));
    assert!(product_only_in_nonce.unwrap_err().contains("積 42"));
    assert!(product_only_in_command.unwrap_err().contains("積 42"));
    // tty を持たない子（この test が直接起こした sleep）は赤
    let mut bare = Command::new("/bin/sleep")
        .arg("20")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let e = l3_terminal(out, "n-7f3a", 42, bare.id(), std::process::id()).unwrap_err();
    let _ = bare.kill();
    let _ = bare.wait();
    assert!(
        e.contains("tty を持たない") && e.contains("zsh / bash でない"),
        "{e}"
    );
}

#[test]
fn l4_needs_measured_windows_that_stay_the_same() {
    let w = |pid, window_id| WindowFact { pid, window_id };
    assert!(
        l4_same_windows(&[], &[]).unwrap_err().contains("0 件"),
        "測っていない緑を出さない"
    );
    assert!(l4_same_windows(&[w(1, 10), w(2, 20)], &[w(2, 20), w(1, 10)]).is_ok());
    assert!(l4_same_windows(&[w(1, 10)], &[w(1, 11)]).is_err());
    assert!(l4_same_windows(&[w(1, 10)], &[]).is_err());
}
