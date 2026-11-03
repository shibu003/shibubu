//! G104 `Link…`「token 入りは止める」の webhook（INTEG・2026-10-30・operator yc-mvp-18「ra2b」）: password の無い URL で、URL そのものが鍵の
//! webhook（Slack・Discord）を秘密と読む（HOME-ATTACH-1101 で `Link…` は消えた。同じ門 `has_secret` が添える file・folder・貼った文を守る）。G-A の反証 ra2 は URL の password の守りで先に赤くなり、webhook の守りを単独では測っていなかった。
//! 負の対照: `has_secret` の webhook の行（`hooks.slack.com/services/`…）を `false` にする → 赤（integ-negctl-webhook の log）。
#![allow(dead_code)]
#[path = "../src/domain/mod.rs"]
mod domain;

use domain::chat::has_secret;

#[test]
fn ra2b_webhook_links_without_a_password_are_not_added() {
    // 値は試験の中で組む（repo の secret の検査に本物の鍵の形の字を置かない）
    let fake = "Zq7xWm2Kp9Lr4Tn6Vb8Yc1Hd";
    for url in [
        format!("https://hooks.{}.com/services/T0AAAAAAA/B0BBBBBBB/{fake}", "slack"),
        format!("https://{}.com/api/webhooks/123456789012345678/{fake}", "discord"),
        format!("https://{}.com/api/webhooks/123456789012345678/{fake}", "discordapp"),
    ] {
        assert!(has_secret(&url), "鍵そのものの webhook を秘密と読まない: {url}");
    }
    assert!(!has_secret("https://example.slack.com/archives/C0AAAAAAA"), "鍵の無い Slack の link まで止めた");
}
