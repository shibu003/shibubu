// ui-strings.py の scan が 1 行の全部の literal を数えるかの fixture（test.sh の ui-strings scan）。行番号は test.sh が期待に持つ。
fn f(x: u8, parts: &[String]) {
    let a = (String::from("名前を入力"), String::from("保存に失敗"));
    let b = format!("読み込めません: {} · {}", x, "閉じる");
    let c = r#"設定 "を" 開く"#;
    let d = parts.join("・");
}
