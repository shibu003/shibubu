//! 後で行う受け入れ test（owner 2026-10-16「linterに後で行うtestとして入れといて」）。
//! 原文と場面の分解は docs/ACCEPTANCE.md。MVP（docs/INCOMPLETE_AUDIT.md の P0）が済んだら
//! `cargo test --test acceptance -- --ignored` で開始する。それまでは #[ignore] のまま、
//! scripts/lint.sh が 3 本の存在を検査して「後で行う test N 件」を表示する。
//! 通す時は配った binary と test 用アカウントの上で（本番アカウントを test に使わない）。

#[test]
#[ignore = "MVP 完成後に開始: LinkedIn の follower / following と YC の一覧から営業メールを一括送信し、返信の数と本文まで取る統合 app（docs/ACCEPTANCE.md test1）"]
fn test1_linkedin_gmail_outreach() {
    todo!("docs/ACCEPTANCE.md §3 test1 の場面（一覧 → 相手別の下書き → 一括送信 → 返信取得）を通す");
}

#[test]
#[ignore = "MVP 完成後に開始: Calendar と Gmail と claude.ai を 1 つの app にし、全部そこから操作でき chat bar でも指示できる（docs/ACCEPTANCE.md test2）"]
fn test2_calendar_gmail_claude_app() {
    todo!("docs/ACCEPTANCE.md §3 test2 の場面（3 card + chat bar → 承認 → 実行）を通す");
}

#[test]
#[ignore = "MVP 完成後に開始: Reaper と YouTube と Apple Music を繋ぎ、library / download の再生・YouTube の検索再生 window・鼻歌の録音 → Reaper へ insert（docs/ACCEPTANCE.md test3）"]
fn test3_reaper_youtube_applemusic_app() {
    todo!("docs/ACCEPTANCE.md §3 test3 の場面（再生 → 検索 window → 録音 → Reaper に track）を通す");
}
