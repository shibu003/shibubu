//! Project の画面の側（YCM-012〜014）: Page の移り（`router`）と、Page ごとの入力途中の状態（`PageViewState`）。
//! 形（ProjectDefinitionV1・ProjectPatchOp）は `domain::project`、保存と差分の適用は `generated.rs`。
pub mod context; // W9: Terminal から Project を読む（`shibubu project pages|context|surfaces --json`）・L7: 承認を待つ一言を置く（`shibubu project ask`）
pub mod acquire; // PV0: 無い app を承認の後に 1 つ入れて読み戻す（前もって確かめた表と Homebrew の cask だけ・図 §49）
pub mod discover; // PV1: 迷いの一言の成果物の候補と道具の行（ui::discover の中身を表から作る・何も入れない・図 §50）
pub mod router;
/// W16-1: Native Direct の面を Page の枠に重ねる（feature `native_direct` の時だけ）。
#[cfg(feature = "native_direct")]
pub mod native;
pub mod web;

use crate::domain::project::{ReceiptKind, ReceiptState, SurfaceReceipt, SurfaceSpec, SurfaceTarget};
use crate::surface::Bridge;
use crate::ui::renderer::{AppState, Draft, RowPick};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// W2c: 製品の側の読み戻し。画面の状態の表示（AX の `surface-status:<surface_id>`）のための receipt を 1 件作る。
/// 計画の名前は `surface::readback` だが、surface は domain を引けない crate（audio 経由の bin・tests/portal.rs）にも入るので、両方を持つここに置く。
/// 例: `let r = project::readback(&mut bridge, &spec, "page.docs", "本文に nonce を入力"); r.validate()?; store.put_surface_receipt(..)?;`
/// 保証する: 読めなかった時は Err にせず state = unavailable / unverified を返す（成功の語を出す根拠を作らない）・WebView は自己申告しか無いので verified を返さない。
/// W2c では verified を一度も返さない: 操作の効果を操作とは別の道で読む口（W16 の file の sha・W12W の service）がまだ無い。
/// 入っている app は unverified（app_id と版と OS だけ埋める・bridge の `permission` が trusted でなければ oracle = `NOT_TRUSTED`）・入っていない / bridge が答えない は unavailable（理由は oracle の欄）。
/// 保証しない: 受け入れの `*_verified`（出してよいのは試験の側の `tests/common/readback.rs`・`scripts/qa/bg/oracle/*` だけ。製品が verified で試験の側が違えば L1）。
/// W12W-0: Web（`SurfaceTarget::Url`）は `surface::WEBVIEW_BUILT` が false の build では
/// state = unavailable・oracle = `surface::WEBVIEW_MISSING`（Page に置けない事を言う）。true の build では今のまま unverified（自己申告だけ）。
/// Wave: W2c
pub fn readback(bridge: &mut Bridge, spec: &SurfaceSpec, page_id: &str, operation: &str) -> SurfaceReceipt {
    let mut r = receipt(spec, page_id, ReceiptState::Unverified, "");
    r.operation = operation.to_string();
    match &spec.target {
        SurfaceTarget::Url { .. } if crate::surface::WEBVIEW_BUILT => r.oracle = "none: what's inside the web view is only self-reported, with no separate way to check".into(),
        SurfaceTarget::Url { .. } => {
            r.state = ReceiptState::Unavailable;
            r.oracle = crate::surface::WEBVIEW_MISSING.into();
        }
        SurfaceTarget::App { bundle_id, .. } => {
            let found = bridge.call("find_app", serde_json::json!({ "bundle_id": bundle_id }), std::time::Duration::from_secs(5));
            match found {
                Err(e) => {
                    r.state = ReceiptState::Unavailable;
                    r.oracle = format!("find_app: {e}");
                }
                Ok(v) if v["installed"] != serde_json::Value::Bool(true) => {
                    r.state = ReceiptState::Unavailable;
                    r.oracle = crate::domain::project::NOT_INSTALLED.into();
                }
                Ok(v) => {
                    r.app_version = v["version"].as_str().unwrap_or_default().to_string();
                    // W14（062 の不許可）: 入っていても AX の許可が無ければ、この Page からは操作できない事と次の一手を残す（許可の有る時と同じ receipt にしない）
                    r.oracle = match bridge.call("permission", serde_json::json!({}), std::time::Duration::from_secs(5)) {
                        Ok(p) if p["trusted"] == serde_json::Value::Bool(true) => "find_app: installed (the effect of actions hasn't been read back yet)".into(),
                        Ok(_) => crate::domain::project::NOT_TRUSTED.into(),
                        Err(e) => {
                            r.state = ReceiptState::Unavailable;
                            format!("permission: {e}")
                        }
                    };
                }
            }
        }
    }
    r
}

/// `sw_vers` の「版 (build)」。読めなければ空（verified の照合が空の欄で止める）。W19-2 の配線: Engine の `EnvFacts::os_version` も同じ値（app.rs）。
pub(crate) fn os_version() -> String {
    let read = |flag: &str| {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg(flag)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    match (read("-productVersion"), read("-buildVersion")) {
        (v, _) if v.is_empty() => String::new(),
        (v, b) if b.is_empty() => v,
        (v, b) => format!("{v} ({b})"),
    }
}

/// `https://excalidraw.com/#room=..` → `https://excalidraw.com`（scheme と host[:port]・userinfo は落とす）。scheme が無ければそのまま。
/// W13b の反証の直し: 無い app を入れる頁の link の文字に行き先の host を出すのにも使う（`Generated::draw_usable`）。
pub(crate) fn origin(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            // `\` も区切り（browser は https の `\` を `/` と読む・その後ろの `@` を userinfo にしない）
            let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
            // userinfo（user:password@）を落とす。receipt に保存されるので token を残さない
            let host = rest[..end].rsplit('@').next().unwrap_or_default();
            format!("{}{}", &url[..i + 3], host)
        }
        None => url.to_string(),
    }
}

/// W3: 主デモの「この repo」を Shibubu に渡す口（YC_WAVES §3・YC_ACCEPTANCE §1 の「W3 の契約で決める」）。
/// 試験の道具は `open -g --env SHIBUBU_WORKSPACE_DIR=$HOME/shibubu-yc-runs/<run>/shibubu-yc-demo` で渡す（`SHIBUBU_DATA_DIR` と同じ渡し方）。
/// 読むのは起動の時に `app.rs` の 1 か所だけ。保存先は Project の `resources`（`repo`）と `terminal_specs`（`working_directory`）で、新しい欄は作らない。
pub const WORKSPACE_ENV: &str = "SHIBUBU_WORKSPACE_DIR";

/// `WORKSPACE_ENV` の値を確かめる。
/// 例: `let ws = project::workspace_from_env().unwrap_or_else(|e| { note = Some(e.to_string()); None });`
/// 保証する: Ok(Some) は絶対 path で、今その場所に dir が在る（symlink は辿った先）。未設定・空は Ok(None)（Project は repo 無しで作り、Dev の Page が 1 行で言う）。
/// 保証しない: git の repo である事・後で消されない事・書ける事。
/// Wave: W3
pub fn workspace_from_env() -> Result<Option<std::path::PathBuf>, WorkspaceError> {
    workspace_from(std::env::var_os(WORKSPACE_ENV))
}

/// 反証の直し（W3）: 一言の中の repo の場所。Finder から開いた .app は env を渡せないので、`/` から始まる path を一言に書けば使う。
/// 例: `project::workspace_in("この repo の開発環境を作って /Users/you/code/x")` → `Some("/Users/you/code/x")`。
/// 保証する: 返すのは空白・括弧・読点で区切った語のうち、`/` から始まり（`/` 1 字は区切りなので取らない）今 dir が在って `.git` を持つ最初の 1 つ
/// （`/Applications` のような通りすがりの dir を repo と読まない）。保証しない: `.git` が壊れていない事。env の口（`workspace_from_env`）は `.git` を求めない。
/// G99: `~/` から始まる語も（HOME の下に読み替える・Finder から開いた人は path を `~/code/x` と書く）。
pub fn workspace_in(goal: &str) -> Option<std::path::PathBuf> {
    workspace_in_home(goal, &std::env::var("HOME").unwrap_or_default())
}

fn workspace_in_home(goal: &str, home: &str) -> Option<std::path::PathBuf> {
    goal.split(|c: char| c.is_whitespace() || "「」『』（）()\"'、。".contains(c))
        .filter_map(|t| match t.strip_prefix("~/") {
            Some(rest) if !home.is_empty() && !rest.is_empty() => Some(format!("{}/{rest}", home.trim_end_matches('/'))),
            _ => (t.len() > 1 && t.starts_with('/')).then(|| t.to_string()),
        })
        .find_map(|t| workspace_from(Some(t.into())).ok().flatten())
        .filter(|p| p.join(".git").exists())
}

/// `workspace_from_env` の中身（env は process 全体の物なので、試験はこちらを値で呼ぶ）。path は渡されたまま返す（symlink を解かない）。
fn workspace_from(value: Option<std::ffi::OsString>) -> Result<Option<std::path::PathBuf>, WorkspaceError> {
    let Some(p) = value.filter(|v| !v.is_empty()).map(std::path::PathBuf::from) else { return Ok(None) };
    if !p.is_absolute() {
        return Err(WorkspaceError::NotAbsolute(p));
    }
    if !p.is_dir() {
        return Err(WorkspaceError::NotADirectory(p));
    }
    Ok(Some(p))
}

/// W13: この Mac に入っている app の一覧（`composer::project::plan_project` が model に見せ、答えの app の名前を bundle id に引き直す）。
/// G98: `audio::surface::app_lines` の一覧（Spotlight〈`mdfind 'kMDItemContentType == "com.apple.application-bundle"'`〉・置き場の直の走査・
/// 動いている app の和集合・1 回の起動で 1 回読んで持つ）。app を起動しない・Apple Events を送らない・cua を使わない。
/// Spotlight の index が止まっている Mac でも在る app を「無い」にしない（verifier が Page に嘘の 1 問を置かない）。
/// `intel::investigate` は使わない（`audio/surface.rs` を取り込み、E2E の source の走査〈cua の語〉に入れられない）。
///
/// 例: `let apps = project::installed_apps(Duration::from_secs(5)).unwrap_or_else(|why| { line = Some(why); vec![] });`
/// 保証する: bundle id の在る物だけ・利用者が開く app の置き場（`/Applications`・`/System/Applications`・`~/Applications` とその下の folder）の
/// bundle だけ（他の app の中の helper の .app・system の部品は入れない）・bundle id で重ねない（同じ id の copy は名前の順で先の 1 つ）・名前の順。
/// 読めない時（3 つの源の全部）は Err の 1 行（呼ぶ側は空の一覧で続ける）。
/// W13b の反証の直し: 返すのは (model に見せる一覧, この Mac に在る app の全部)。全部 = 同じ 1 回の mdfind の、置き場を問わない bundle（Finder などの
/// `/System/Library/CoreServices` の下の app も入る・他の app の中の helper の .app と bundle id の無い物は入れない・同じ並びと重ねない決まり）。
/// `plan_project` が「一言が名指した app がこの Mac に無い」を決めるのに使う（一覧に無いだけで在る app を 1 問にしない）。
/// 署名を変えた理由: 一覧（prompt）を広げずに、在る / 無いを同じ 1 回の読みの全部で決めるため。
/// 保証しない: 起動の後に入れた app が出る事（持っている一覧を返す・`find_app` は無い時に読み直す）・その app が文書を開ける事。
pub fn installed_apps(timeout: std::time::Duration) -> Result<(Vec<crate::composer::project::KnownApp>, Vec<crate::composer::project::KnownApp>), String> {
    let text = crate::audio::surface::app_lines(timeout, false)?;
    let home = std::env::var("HOME").unwrap_or_default();
    Ok(parse_apps(&text, &home))
}

/// `audio::surface::app_lines` の行（`<path>   kMDItemDisplayName = <名前>   kMDItemCFBundleIdentifier = <id>`）を読む。
/// (置き場の中の一覧, 置き場を問わない全部)（`installed_apps` の doc）。
fn parse_apps(text: &str, home: &str) -> (Vec<crate::composer::project::KnownApp>, Vec<crate::composer::project::KnownApp>) {
    let user_apps = format!("{home}/Applications/");
    let roots = ["/Applications/", "/System/Applications/", user_apps.as_str()];
    // (置き場の中か, app)。置き場を問わず、他の app の中の helper の .app と bundle id・名前の無い物は落とす
    let read: Vec<(bool, crate::composer::project::KnownApp)> = crate::audio::surface::app_line_fields(text)
        .filter_map(|(path, name, id)| {
            let inner = path.strip_suffix(".app")?;
            let placed = roots.iter().any(|r| path.starts_with(r));
            (!inner.contains(".app/") && id != "(null)" && !id.is_empty() && !name.is_empty() && name != "(null)").then(|| {
                (placed, crate::composer::project::KnownApp { name: name.strip_suffix(".app").unwrap_or(name).to_string(), bundle_id: id.to_string() })
            })
        })
        .collect();
    // 名前の順・bundle id で重ねない（同じ id の copy は名前の順で先の 1 つ）
    let tidy = |mut apps: Vec<crate::composer::project::KnownApp>| {
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.name.cmp(&b.name)));
        let mut seen = std::collections::HashSet::new();
        apps.retain(|a| seen.insert(a.bundle_id.clone()));
        apps
    };
    let shown = tidy(read.iter().filter(|(placed, _)| *placed).map(|(_, a)| a.clone()).collect());
    (shown, tidy(read.into_iter().map(|(_, a)| a).collect()))
}

/// W3: bridge が無い・起こせない時の receipt（`readback` を呼ばずに作る）。Err にせず unavailable と理由。
/// 保証する: verified を返さない・app_id は spec の bundle id か origin。
pub fn unavailable(spec: &SurfaceSpec, page_id: &str, reason: &str) -> SurfaceReceipt {
    receipt(spec, page_id, ReceiptState::Unavailable, reason)
}

/// `readback` と `unavailable` の共通の形: kind と app_id は spec の target から（App = bundle id・Url = origin）、OS は今の版、他の欄は空。
fn receipt(spec: &SurfaceSpec, page_id: &str, state: ReceiptState, oracle: &str) -> SurfaceReceipt {
    let (kind, app_id) = match &spec.target {
        SurfaceTarget::App { bundle_id, .. } => (ReceiptKind::NativeApp, bundle_id.clone()),
        SurfaceTarget::Url { url } => (ReceiptKind::Web, origin(url)),
    };
    SurfaceReceipt {
        surface_id: spec.id.clone(),
        page_id: page_id.to_string(),
        kind,
        state,
        oracle: oracle.to_string(),
        app_id,
        app_version: String::new(),
        os_version: os_version(),
        operation: String::new(),
        verified_at: String::new(),
        evidence_ref: String::new(),
    }
}

/// `workspace_from_env` の拒否。Display は Home に出す 1 行（理由と次の一手）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// 相対 path（起動の場所で意味が変わるので受けない）。
    NotAbsolute(std::path::PathBuf),
    /// 在らない・dir でない。
    NotADirectory(std::path::PathBuf),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute(p) => write!(f, "Pass {WORKSPACE_ENV} as a path starting with / (it is {} now)", p.display()),
            Self::NotADirectory(p) => write!(f, "There's no folder at {} in {WORKSPACE_ENV}. Pass the place where you cloned the repo", p.display()),
        }
    }
}
impl std::error::Error for WorkspaceError {}

/// Page を離れる時に残し、戻った時に戻す入力途中の状態（SPEC v15 §2「復帰可能な PageViewState」）。
/// 保存形式: `ui_state(session_id = "main.{page.app_ref}", key = view_state_key(page_id), json = この型の JSON)`。新しい表は作らない。
/// `AppState` のうち frame ごとに作り直す物（rects / gates / regions）は持たない。
///
/// 使い方: 離れる時 `PageViewState::capture(&state)` を保存 → 戻る時 `load(...)?.unwrap_or_default().restore(&mut state)`。
/// 保証する: capture → JSON → restore で drafts / tabs / rows / selected が元どおり。保証しない: scroll と focus（W2 では持たない）、
/// app_ref を持たない Page（W3 の Terminal の Page 等。session が無いので保存先が無い = 呼ぶ側が保存しない）。
/// Wave: W2
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageViewState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drafts: BTreeMap<String, Vec<Draft>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tabs: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rows: BTreeMap<String, RowPick>,
}

impl PageViewState {
    /// 今の画面の状態から残す物だけを写す。
    pub fn capture(state: &AppState) -> Self {
        PageViewState {
            selected: state.selected.clone(),
            drafts: state.drafts.clone(),
            tabs: state.tabs.clone(),
            rows: state.rows.clone(),
        }
    }

    /// `state` の残す物をこれで置き換える（frame ごとの物は空にする）。前の Page の draft を混ぜない。
    pub fn restore(self, state: &mut AppState) {
        *state = AppState {
            selected: self.selected,
            drafts: self.drafts,
            tabs: self.tabs,
            rows: self.rows,
            ..AppState::default()
        };
    }
}

/// `ui_state` の key（`page:{page_id}:view`）。local action の source id（`local.*`）と重ならない。
///
/// 使い方: `store.put_ui_state(&session, &view_state_key(&page.id), &json)`
/// Wave: W2
pub fn view_state_key(page_id: &str) -> String {
    format!("page:{page_id}:view")
}

/// W13c: 利用者が選んだ配置の型（29 通目「利用者が替えられる」）。Project ごとに 1 つ。
/// 保存形式: `ui_state(session_id = "main.{Project の app}", key = PLACEMENT_KEY, json = {"template": "<型の id>" | null})`。新しい表・migration は無し。
/// 行が無い = 一言に合う既定（`composer::layout::default_template`）・`template: null` = 利用者が「型なし」（今の並び）を選んだ・知らない id = 型なし。
/// 使い方: Page タブの「配置: <名前>」→ `PageIntent::Placement` が次の型（同梱の順 → 型なし → 先頭）を書く → `Generated::placement(&project)` で描く。
/// 保証する: Project の revision と hash・Page の木を変えない（置き方は描く時だけ）・Undo / Redo の対象にしない・再起動の後も同じ型。
/// Wave: W13c
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementChoice {
    pub template: Option<String>,
}

/// `PlacementChoice` の `ui_state` の key。`page:{id}:view`・local action の source id（`local.*`）と重ならない（G92 で消した引き出しの `project:drawer` の行は前の版の DB に残っても読まない）。
pub const PLACEMENT_KEY: &str = "project:placement";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// 偽の bridge（tests/fake-surface-bridge.py・find_app は FAKE_INSTALLED で答える）。installed = "1" / "0" / "die"、または `FAKE_…=…` をそのまま export
    pub(crate) fn fake_bridge(name: &str, installed: &str) -> Bridge {
        let dir = std::env::temp_dir().join(format!("shibubu-w2c-readback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let py = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-surface-bridge.py");
        let sh = dir.join(name);
        let env = match installed {
            "die" => "export FAKE_MODE=die".to_string(),
            e if e.starts_with("FAKE_") => format!("export {e}"),
            _ => format!("export FAKE_INSTALLED='{installed}'"),
        };
        std::fs::write(&sh, format!("#!/bin/sh\n{env}\nexec python3 '{}'\n", py.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        // 高負荷で python の起動が 10 秒を越え D-a の門（yc_demo_e2e）ごと赤くなったので 30 秒
        Bridge::spawn(&sh, std::time::Duration::from_secs(30)).unwrap()
    }

    fn spec(target: SurfaceTarget) -> SurfaceSpec {
        SurfaceSpec { id: "docs".into(), backend: crate::domain::project::SurfaceBackendKind::NativeDirect, target, keep_alive: true, frame: None, browser: false }
    }

    #[test]
    fn w2c_readback_never_claims_verified_and_says_why() {
        let app = spec(SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("README.md".into()) });
        // 入っている app: unverified・版と OS が埋まる・validate を通る
        let mut b = fake_bridge("installed.sh", "1");
        let r = readback(&mut b, &app, "page.docs", "1 行足す");
        assert_eq!((r.kind, r.state), (ReceiptKind::NativeApp, ReceiptState::Unverified), "{r:?}");
        assert_eq!((r.surface_id.as_str(), r.page_id.as_str(), r.app_id.as_str(), r.app_version.as_str()), ("docs", "page.docs", "com.apple.TextEdit", "1.19"));
        assert!(!r.os_version.is_empty(), "sw_vers を読めていない");
        assert!(r.validate().is_ok());
        // 入っていない app: unavailable と理由
        let mut b = fake_bridge("missing.sh", "0");
        let r = readback(&mut b, &app, "page.docs", "1 行足す");
        assert_eq!(r.state, ReceiptState::Unavailable, "{r:?}");
        assert!(r.oracle.contains("入っていません"), "{}", r.oracle);
        // bridge が死んだ: Err にせず unavailable と bridge の理由
        let mut b = fake_bridge("die.sh", "die");
        let r = readback(&mut b, &app, "page.docs", "1 行足す");
        assert_eq!(r.state, ReceiptState::Unavailable, "{r:?}");
        assert!(r.oracle.starts_with("find_app: ") && r.oracle.len() > "find_app: ".len(), "{}", r.oracle);
        // Web は下の w12w0_web_surface_says_build_has_no_webview
    }

    /// W14（YCM-062・L1）: 不許可（AX の信頼が無い bridge）・未接続（bridge が落ちた）・timeout（bridge が 5 秒の内に答えない）の 3 つで、
    /// 面の receipt は verified にならず（不許可 = unverified・未接続と timeout = unavailable と bridge の理由）、画面の 1 行に成功の語（YC_ACCEPTANCE §4）が 0。
    /// 負の対照: `readback` の `Err(e)` の枝で state を Unavailable にしない（unverified のまま）と、未接続と timeout の行が赤。
    /// 不許可の行は oracle が `NOT_TRUSTED`・1 行が許可の次の一手を言う事で見る（反証の直し: 前は入っている app の既定値を見ていて、許可のある bridge に替えても緑だった）。
    /// 負の対照: 不許可の行の bridge を `FAKE_MODE=ok`（許可あり）に替える・`readback` の `permission` を聞く所を外す → どちらも不許可の行が赤。
    #[test]
    fn w14_denied_unreachable_and_timeout_never_read_as_success() {
        const SUCCESS_WORDS: [&str; 10] = ["接続済み", "保存しました", "保存済み", "済み", "完了", "✓", "Connected", "Saved", "Done", "Verified"];
        let app = spec(SurfaceTarget::App { bundle_id: "com.apple.TextEdit".into(), file: Some("README.md".into()) });
        for (name, mode, want) in [
            ("w14-denied.sh", "FAKE_MODE=untrusted", ReceiptState::Unverified),
            ("w14-gone.sh", "die", ReceiptState::Unavailable),
            ("w14-timeout.sh", "FAKE_MODE=slow FAKE_SLOW=8", ReceiptState::Unavailable),
        ] {
            let mut b = fake_bridge(name, mode);
            let r = readback(&mut b, &app, "page.docs", "1 行足す");
            assert_eq!(r.state, want, "{name}: {r:?}");
            if want == ReceiptState::Unavailable {
                assert!(r.oracle.starts_with("find_app: ") && r.oracle.len() > "find_app: ".len(), "{name}: 理由が無い {}", r.oracle);
            }
            let (_, line) = crate::ui::renderer::surface_status_line(Some(&r));
            if name == "w14-denied.sh" {
                assert_eq!(r.oracle, crate::domain::project::NOT_TRUSTED, "{name}: 許可が無い事を receipt に残していない {r:?}");
                assert!(line.contains("Accessibility") && line.contains("allow Shibubu"), "{name}: 許可の次の一手が無い {line}");
            }
            assert!(!SUCCESS_WORDS.iter().any(|w| line.contains(w)), "{name}: 成功の語 {line}");
            assert!(line.contains("TextEdit"), "{name}: app の名前で言う {line}");
        }
    }

    /// W12W-0 の契約（呼ぶ側の形）: Web の面は bridge に聞かず・app_id は origin。WebView の無い build では unavailable と WEBVIEW_MISSING・有る build では unverified のまま。
    #[test]
    fn w12w0_web_surface_says_build_has_no_webview() {
        let mut b = fake_bridge("w12w0-web.sh", "1");
        let web = spec(SurfaceTarget::Url { url: "https://excalidraw.com/#json=abc,key".into() });
        let r = readback(&mut b, &web, "page.diagram", "文字を置く");
        assert_eq!((r.kind, r.app_id.as_str()), (ReceiptKind::Web, "https://excalidraw.com"), "{r:?}");
        if crate::surface::WEBVIEW_BUILT {
            assert_eq!(r.state, ReceiptState::Unverified, "{r:?}");
        } else {
            assert_eq!((r.state, r.oracle.as_str()), (ReceiptState::Unavailable, crate::surface::WEBVIEW_MISSING), "{r:?}");
        }
        assert!(r.validate().is_ok());
    }

    /// W12W-0 の契約: release の .app（build-mac.sh）の cargo build は全部 `--features webview`（owner 2026-10-20「Okです」）。
    #[test]
    fn w12w0_release_build_has_webview() {
        let sh = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/release/build-mac.sh")).unwrap();
        let builds: Vec<&str> = sh.lines().map(str::trim).filter(|l| l.starts_with("cargo build")).collect();
        assert!(builds.len() >= 2, "native と --universal の 2 本が在るはず: {builds:?}");
        for l in builds {
            assert!(l.contains("--features webview"), "webview の無い release の build: {l}");
        }
    }

    #[test]
    fn w3_workspace_is_absolute_existing_dir_or_a_reason() {
        use std::ffi::OsString;
        assert_eq!(workspace_from(None), Ok(None));
        assert_eq!(workspace_from(Some(OsString::new())), Ok(None), "空は未設定と同じ");
        let rel = std::path::PathBuf::from("shibubu-yc-demo");
        assert_eq!(workspace_from(Some(rel.clone().into())), Err(WorkspaceError::NotAbsolute(rel)));
        let gone = std::env::temp_dir().join(format!("shibubu-w3-gone-{}", std::process::id()));
        assert_eq!(workspace_from(Some(gone.clone().into())), Err(WorkspaceError::NotADirectory(gone.clone())));
        let file = gone.with_extension("txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(workspace_from(Some(file.clone().into())), Err(WorkspaceError::NotADirectory(file.clone())), "file は dir でない");
        let dir = std::env::temp_dir();
        assert_eq!(workspace_from(Some(dir.clone().into())), Ok(Some(dir)));
        let _ = std::fs::remove_file(file);
        assert!(WorkspaceError::NotAbsolute("a".into()).to_string().contains(WORKSPACE_ENV));
    }

    /// 反証の直し（W3）: env を渡せない起動でも、一言に書いた在る dir を repo の場所として取る。在らない・相対は取らない。
    #[test]
    fn w3_workspace_in_the_sentence_is_an_existing_absolute_dir() {
        let dir = std::env::temp_dir().join(format!("shibubu-w3-ws-in-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let plain = std::env::temp_dir().join(format!("shibubu-w3-ws-in-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        let d = dir.display();
        assert_eq!(workspace_in(&format!("この repo の開発環境を作って {d}")), Some(dir.clone()));
        assert_eq!(workspace_in(&format!("この repo（{d}）の開発環境を作って")), Some(dir.clone()), "括弧の中");
        assert_eq!(workspace_in("この repo の開発環境を作って"), None);
        assert_eq!(workspace_in("/no/such/shibubu/dir の開発環境"), None, "在らない dir は取らない");
        assert_eq!(workspace_in("docs の開発環境"), None, "相対は取らない");
        assert_eq!(workspace_in("Dev / Docs / Diagram の開発環境を作って"), None, "区切りの / を repo と読まない");
        assert_eq!(workspace_in(&format!("{} の開発環境", plain.display())), None, ".git の無い dir は repo と読まない");
        // G99: `~/` は HOME の下（変異: 読み替えを外す → 赤）・`~` 1 字と HOME が無い時は取らない
        let home = dir.parent().unwrap().display().to_string();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(workspace_in_home(&format!("Set up a dev environment for ~/{name}"), &home), Some(dir.clone()), "~/ の path");
        assert_eq!(workspace_in_home("~ の開発環境", &home), None);
        assert_eq!(workspace_in_home(&format!("~/{name} の開発環境"), ""), None, "HOME が無い");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&plain).unwrap();
    }

    #[test]
    fn w3_unavailable_names_the_target_and_the_reason() {
        let web = spec(SurfaceTarget::Url { url: "https://u:p@excalidraw.com/#x".into() });
        let r = unavailable(&web, "page.d", "bridge が起きていません");
        assert_eq!((r.kind, r.state, r.app_id.as_str(), r.oracle.as_str()), (ReceiptKind::Web, ReceiptState::Unavailable, "https://excalidraw.com", "bridge が起きていません"));
        assert!(r.validate().is_ok());
    }

    #[test]
    fn w2c_origin_keeps_scheme_and_host_only() {
        assert_eq!(origin("https://excalidraw.com/#json=1"), "https://excalidraw.com");
        assert_eq!(origin("http://localhost:3000/a?b=1"), "http://localhost:3000");
        assert_eq!(origin("https://github.com"), "https://github.com");
        assert_eq!(origin("about:blank"), "about:blank");
        // userinfo（user:password@）は origin に入れない（receipt に保存されるので）
        assert_eq!(origin("https://alice:tok3n@git.example.com/repo"), "https://git.example.com");
        assert_eq!(origin("https://a@b:c@h.example:8443?x#y"), "https://h.example:8443");
        // W13b の反証の直し: browser は https の `\` を `/` と読む（行き先は a.example）。`@` の後ろの b.example を host と言わない（link の文字に出す）
        assert_eq!(origin("https://a.example\\@b.example/x"), "https://a.example");
    }

    #[test]
    fn capture_json_restore_round_trips_and_drops_frame_state() {
        let mut s = AppState::default();
        s.draft_mut("books-form", None)
            .insert("title".into(), json!("坊っちゃ"));
        s.tabs.insert("tabs".into(), 2);
        s.selected = Some("books-list".into());
        s.rows.insert(
            "books-list".into(),
            RowPick {
                record: json!({"id": "b1"}),
                by_key: true,
                nth: 0,
            },
        );
        s.gates.insert(
            "x".into(),
            crate::ui::renderer::bindings::Gate {
                state: None,
                reason: "r".into(),
            },
        );
        let json = serde_json::to_value(PageViewState::capture(&s)).unwrap();
        let back: PageViewState = serde_json::from_value(json).unwrap();
        let mut t = AppState::default();
        t.draft_mut("other", None)
            .insert("text".into(), json!("前の Page"));
        back.restore(&mut t);
        assert_eq!(
            t.draft("books-form", None).unwrap()["title"],
            json!("坊っちゃ")
        );
        assert!(
            t.draft("other", None).is_none(),
            "前の Page の draft を混ぜない"
        );
        assert_eq!(
            (t.tabs.get("tabs"), t.selected.as_deref()),
            (Some(&2), Some("books-list"))
        );
        assert_eq!(t.rows, s.rows);
        assert!(t.gates.is_empty());
        assert!(
            serde_json::from_value::<PageViewState>(json!({"scroll": 3})).is_err(),
            "知らない欄は拒否"
        );
    }

    /// W13: mdfind の行から、利用者が開く app の置き場の bundle だけ・bundle id の在る物だけ・id で重ねず名前の順。
    #[test]
    fn w13_installed_apps_reads_the_spotlight_lines() {
        let text = [
            "/Applications/Notes Maker.app   kMDItemDisplayName = Notes Maker   kMDItemCFBundleIdentifier = org.example.notes",
            "/System/Applications/Clock.app   kMDItemDisplayName = Clock.app   kMDItemCFBundleIdentifier = com.example.clock",
            "/Users/u/Applications/Draw.app   kMDItemDisplayName = Draw   kMDItemCFBundleIdentifier = org.example.draw",
            "/Applications/Utilities/Probe.app   kMDItemDisplayName = Probe   kMDItemCFBundleIdentifier = org.example.probe",
            "/Applications/Big.app/Contents/Helpers/Helper.app   kMDItemDisplayName = Helper   kMDItemCFBundleIdentifier = org.example.helper",
            "/System/Library/CoreServices/Finder.app   kMDItemDisplayName = Finder   kMDItemCFBundleIdentifier = com.example.finder",
            "/Applications/Broken.app   kMDItemDisplayName = Broken   kMDItemCFBundleIdentifier = (null)",
            "/Applications/Old/Notes Maker.app   kMDItemDisplayName = Notes Maker   kMDItemCFBundleIdentifier = org.example.notes",
            "garbage",
        ]
        .join("\n");
        let pairs = |apps: Vec<crate::composer::project::KnownApp>| -> Vec<(String, String)> { apps.into_iter().map(|a| (a.name, a.bundle_id)).collect() };
        let owned = |want: &[(&str, &str)]| -> Vec<(String, String)> { want.iter().map(|(n, b)| (n.to_string(), b.to_string())).collect() };
        let (shown, on_mac) = parse_apps(&text, "/Users/u");
        assert_eq!(pairs(shown), owned(&[("Clock", "com.example.clock"), ("Draw", "org.example.draw"), ("Notes Maker", "org.example.notes"), ("Probe", "org.example.probe")]));
        // W13b の反証の直し: 置き場の外（CoreServices）の app も「この Mac に在る」には入る（model には見せない）。helper の .app と bundle id の無い物は入れない
        assert_eq!(
            pairs(on_mac),
            owned(&[("Clock", "com.example.clock"), ("Draw", "org.example.draw"), ("Finder", "com.example.finder"), ("Notes Maker", "org.example.notes"), ("Probe", "org.example.probe")])
        );
    }

    /// G98: Spotlight が空（index が止まっている Mac）でも、置き場を直に読んで在る app を返す・Spotlight と直の走査の両方に在る app は 1 つ・
    /// `.app` の中の helper は拾わない・動いている app（lsappinfo の Foreground）も入る。負の対照: `read_app_lines` の直の走査を外す → 1 つ目の assert が赤。
    #[test]
    fn g98_apps_are_found_when_spotlight_is_empty() {
        let root = std::env::temp_dir().join(format!("shibubu-g98-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plist = |dir: &std::path::Path, id: &str, name: Option<&str>| {
            std::fs::create_dir_all(dir.join("Contents")).unwrap();
            let name = name.map(|n| format!("<key>CFBundleName</key>\n\t<string>{n}</string>\n")).unwrap_or_default();
            std::fs::write(dir.join("Contents/Info.plist"), format!("<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleIdentifier</key>\n\t<string>{id}</string>\n\t{name}</dict>\n</plist>\n")).unwrap();
        };
        let apps = root.join("Applications");
        plist(&apps.join("TextEdit.app"), "com.apple.TextEdit", Some("TextEdit"));
        plist(&apps.join("Tools/Draw.app"), "org.example.draw", None);
        plist(&apps.join("Big.app"), "org.example.big", Some("Big"));
        plist(&apps.join("Big.app/Contents/Helpers/Helper.app"), "org.example.helper", Some("Helper"));
        let pairs = |apps: Vec<crate::composer::project::KnownApp>| -> Vec<(String, String)> { apps.into_iter().map(|a| (a.name, a.bundle_id)).collect() };
        let ids = |apps: &[(String, String)]| -> Vec<String> { apps.iter().map(|(_, b)| b.clone()).collect() };
        // 1. Spotlight が空・動いている app も無い → 直の走査だけで TextEdit が在る（helper は入らない）
        let lines = crate::audio::surface::read_app_lines(Ok(String::new()), &[apps.clone()], Ok(String::new())).unwrap();
        let (_, on_mac) = parse_apps(&lines, "/Users/nobody");
        let on_mac = pairs(on_mac);
        assert!(ids(&on_mac).contains(&"com.apple.TextEdit".to_string()), "Spotlight が空だと TextEdit が無いことになった: {on_mac:?}");
        assert!(ids(&on_mac).contains(&"org.example.draw".to_string()), "置き場の下の folder の app が無い: {on_mac:?}");
        assert!(!ids(&on_mac).contains(&"org.example.helper".to_string()), "app の中の helper を拾った: {on_mac:?}");
        // 2. Spotlight と直の走査の両方に在る app は 1 つ・Spotlight が失敗しても Err にしない・動いている app も入る
        let spotlight = format!("{}   kMDItemDisplayName = TextEdit   kMDItemCFBundleIdentifier = com.apple.TextEdit", apps.join("TextEdit.app").display());
        let running = " 7) \"Music\" ASN:0x0-1: \n    bundleID=\"com.apple.Music\"\n    bundle path=\"/System/Applications/Music.app\"\n    pid = 9 type=\"Foreground\" flavor=3\n 8) \"Agent\" ASN:0x0-2: \n    bundleID=\"org.example.agent\"\n    bundle path=\"/Applications/Agent.app\"\n    pid = 10 type=\"UIElement\" flavor=3\n";
        let (_, on_mac) = parse_apps(&crate::audio::surface::read_app_lines(Ok(spotlight), &[apps.clone()], Ok(running.into())).unwrap(), "/Users/nobody");
        let on_mac = ids(&pairs(on_mac));
        assert_eq!(on_mac.iter().filter(|b| *b == "com.apple.TextEdit").count(), 1, "同じ bundle id が 2 つ: {on_mac:?}");
        assert!(on_mac.contains(&"com.apple.Music".to_string()) && !on_mac.contains(&"org.example.agent".to_string()), "動いている app の読み（Foreground だけ）: {on_mac:?}");
        assert!(crate::audio::surface::read_app_lines(Err("down".into()), &[apps.clone()], Err("down".into())).is_ok(), "Spotlight が落ちただけで Err になった");
        assert!(crate::audio::surface::read_app_lines(Err("down".into()), &[root.join("none")], Err("down".into())).is_err(), "3 つ全部が読めないのに Ok");
        let _ = std::fs::remove_dir_all(&root);
    }
}
