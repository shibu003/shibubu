//! W12a: git の repo の HEAD・branch・変更 file を読む Adapter（YC_WAVES の W12a・§1「汎用 Core と Adapter」）。
//! Core（generated.rs）は git を知らない: Project の `resources` の `{kind: "repo", uri: "file://<abs>"}` を
//! `domain::project::ResourceReader`（`AdapterRegistry` が実装）に渡すだけで、registry が `reads` の kind でこれを引く。
//! 経路は許可された OS automation（local の子 process・利用者の機械の git）。読むだけ・承認は要らない（LocalRead）。
//! この module は `crate::` を書かない（tests/adapters.rs が `#[path]` で main.rs と同じ並びに取り込む）。
use super::super::domain::{
    action::Effect,
    binding::Source,
    capability::CapabilityReadiness,
    run::{ErrorCode, Observation, Outcome},
};
use super::super::repos::wait_bounded;
use super::sdk::{now, read_result, source, Adapter, AdapterKind, Route, SourceSync, Spec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 読む action の id（`Adapter::reads` の `("repo", STATUS_READ)`・registry の invoke の名前）。
pub const STATUS_READ: &str = "git.status.read";
/// Observation の source_ref（provenance）。画面の binding が引く key はこれではなく `domain::project::resource_source_id(resource.id)`。
pub const SOURCE: &str = "git.status";

/// G81: repo の work tree の中の file 1 つの中身を読む action（`{"uri": "file://<file の絶対 path>"}`・git の実行 file は使わない）。
/// Engine が Page の書類の保存を disk で確かめる読み戻し（`capability_engine` の `saved_checks`）。payload は `{"exists", "text", "complete"}`。
pub const FILE_READ: &str = "git.file.read";
/// `FILE_READ` の Observation の source_ref（repo の `SOURCE` の記録と混ぜない）。
pub const FILE_SOURCE: &str = "git.file";

const SPECS: &[Spec] = &[
    Spec {
        id: STATUS_READ,
        effect: Effect::LocalRead,
        approval: false,
        scopes: &[],
        input: r#"{"type":"object","required":["uri"],"properties":{"uri":{"type":"string"}}}"#,
        verifier: "read_back",
    },
    Spec {
        id: FILE_READ,
        effect: Effect::LocalRead,
        approval: false,
        scopes: &[],
        // B74: model は disk の場所を知らない（FX-111）= repo からの相対で書く（Engine が repo の下の絶対に直す・`capability_engine::in_repo`）。読みの形も書く（実例: /content を当て推量した）。
        // B76: 絶対 path は勧めない（Engine は model の読みを Project の repo の下だけに縛る = `capability_engine::outside_repo`）
        input: r#"{"type":"object","required":["uri"],"properties":{"uri":{"type":"string","description":"A file in the Project's repo, as its path from the repo (README.md, docs/notes.md). The read gives exists, text and complete: check /text"}}}"#,
        verifier: "read_back",
    },
];

/// `git.status.read` の Observation の payload（保存形式）。`Generated::read_resources` がこの JSON をそのまま
/// `ui_state(session "main.{app}", key resource_source_id("repo"))` に書き、Dev の Page の binding（`head.sha`・`head.branch`・`changes`）が引く。
/// 例: `{"head":{"sha":"<40 桁の 16 進>","branch":"main"},"changes":[{"status":" M","path":"README.md"}]}`
/// 保証する: 試験の側の `scripts/qa/bg/oracle/git.sh read <dir>`（shibubu.qa.git/1）と同じ 3 つの git の読みから作る
/// （`rev-parse HEAD`・`branch --show-current`・`status --porcelain=v1 -z`）ので、同じ瞬間に読めば値が一致する。
/// 保証しない: 読んだ後の変化（読み直すまで古い）・submodule の中身・ignore された file。
/// Wave: W12a
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoStatus {
    pub head: Head,
    /// `status --porcelain=v1 -z` の並びのまま（untracked の `??` を含む・ignore は含まない）。空 = 作業木が clean。
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    /// `rev-parse HEAD` の 40 桁（短くしない）。commit が 1 つも無い（unborn）時は null。
    pub sha: Option<String>,
    /// `branch --show-current`。detached HEAD の時は null（空文字にしない）。
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// porcelain v1 の XY の 2 字そのまま（" M"・"M "・"??"・"R " 等）。
    pub status: String,
    /// repo の root からの path。`-z` で読むので引用符も escape も無い生の path（空白・日本語・改行もそのまま）。
    pub path: String,
    /// rename / copy の元の path（その時だけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig_path: Option<String>,
}

/// git の repo を読む Adapter。
///
/// 例: `let mut r = AdapterRegistry::default(); r.register(Box::new(Git::new()))?;`（通常は `AdapterRegistry::with_local_adapters()`）
/// → `ResourceReader::read(&mut r, &ProjectResource{id: "repo", kind: "repo", uri: Some("file:///Users/you/x")})`
///
/// 保証する:
/// - 読むだけ: `git --no-optional-locks -C <dir>`（index.lock を取らない・利用者が同時に打つ git と競合しない）。環境の GIT_DIR・GIT_WORK_TREE・
///   GIT_INDEX_FILE を外して `<dir>` だけを読む。1 回の読みは 5 秒で打ち切る（`Failed{Timeout}`）。
/// - git の実行 file（`check` が決める）: PATH の上の git → /opt/homebrew/bin/git → /usr/local/bin/git → /usr/bin/git の順。/usr/bin/git は
///   `xcode-select -p` が 0 の時（Command Line Tools が入っている時）だけ使う（入っていない時に叩くと install の窓が出て前面を取る）。
///   見つからなければ readiness は RequiresConnection・invoke は `Failed{NotReady}`・registry の resource の読みは `not_ready_line`
///   （Command Line Tools か git を入れて「↻ 読み直す」）の `Rejected`。
/// - 誤りの種類（`Outcome::Failed{error}`・Observation は None）: uri が `file://` の絶対 path でない = InvalidInput・dir が無い / git の repo でない =
///   TargetNotFound・時間切れ = Timeout・git が 0 以外で終わった / 出力が読めない = Internal。読めた時だけ `Verified` と Observation（source_ref = `SOURCE`・
///   payload = `RepoStatus` の JSON）。
///
/// 保証しない: 速さ（大きな repo の status は秒単位）・bare repo・`<dir>` が worktree の中の sub dir の時の path の基準（root からの path で返す）。
/// 契約（UI-2 から・口は変えない）: Page の Git の欄に出る文（`not_ready_line`・読めない理由）は英語。表に EN の無い文は sendback に足し、
/// その間は日本語のまま allowlist（git.rs は TARGETS の外・台本の Git の欄だけが UI-2 の範囲）。payload（`RepoStatus` の JSON）は変えない。
/// Wave: W12a, UI-2
#[derive(Default)]
pub struct Git {
    /// 使う git の実行 file。`new` では未定（`check` が上の順で決める）・`at` は渡した物に固定。
    bin: Option<PathBuf>,
    pinned: bool,
    sync: SourceSync,
}

impl Git {
    /// git の場所は最初の `check` で決める。
    pub fn new() -> Self {
        Self::default()
    }

    /// git の実行 file を `bin` に固定する（試験: 在らない path で NotReady を見る）。`check` は探し直さない。
    pub fn at(bin: PathBuf) -> Self {
        Self {
            bin: Some(bin),
            pinned: true,
            sync: SourceSync::default(),
        }
    }
}

impl Adapter for Git {
    fn id(&self) -> &'static str {
        "git"
    }
    fn kind(&self) -> AdapterKind {
        AdapterKind::Repo
    }
    fn route(&self) -> Route {
        Route::OsAutomation
    }
    fn provider(&self) -> &'static str {
        "git"
    }
    fn specs(&self) -> &'static [Spec] {
        SPECS
    }
    fn reads(&self) -> &'static [(&'static str, &'static str)] {
        &[("repo", STATUS_READ)]
    }
    /// git が見つからない（`check` の後も RequiresConnection）時に Page の上に出す、入れる物と次の一手の 1 行。
    /// 押す button の名前は画面の今の名前（UI-2 で `Refresh`・generated.rs の Git の欄）。文の英語は表に無いので「8 本の後」（sendback 50）。
    fn not_ready_line(&self) -> Option<&'static str> {
        Some("Git isn't on this Mac. In Terminal, run xcode-select --install to get the Command Line Tools (or install git), then press Refresh")
    }
    /// git が見つかって `check` か読みが 1 回通れば Ready・見つからなければ RequiresConnection（`SourceSync` の規則）。
    /// G81: `FILE_READ` は git を使わないので、git の無い Mac でも Ready。
    fn readiness(&self, action: &str) -> CapabilityReadiness {
        if action == FILE_READ {
            return CapabilityReadiness::Ready;
        }
        self.sync.capability(true)
    }
    /// source は `SOURCE` 1 つ。
    fn sources(&self) -> Vec<Source> {
        vec![source(SOURCE, AdapterKind::Repo, "git", &self.sync, true)]
    }
    /// git の実行 file を決め（`at` なら固定）、`git --version` が 0 で終われば疎通の証拠。repo は読まない。
    fn check(&mut self) {
        let t = now();
        if !self.pinned {
            self.bin = find_git();
        }
        let ok = self.bin.as_deref().filter(|b| usable(b)).is_some_and(|b| {
            matches!(
                git(b, None, &["--version"], Instant::now() + LIMIT),
                Ok((true, _))
            )
        });
        match (ok, &self.bin) {
            (true, Some(b)) => self.sync.record(
                t,
                &Outcome::Verified {
                    evidence: vec![format!("{} --version", b.display())],
                },
                None,
            ),
            _ => self.sync.missing(t),
        }
    }
    /// `{"uri": "file://<abs>"}` の repo を読む（上の誤りの種類）。G81: `FILE_READ` はその file を読む（`read_file`・repo の source の記録に混ぜない）。
    fn invoke(&mut self, action: &str, input: &Value) -> (Outcome, Option<Observation>) {
        let t = now();
        if action == FILE_READ {
            return read_result(read_file(input), FILE_SOURCE, "file text", t);
        }
        let read = repo_dir(input).and_then(|dir| match &self.bin {
            Some(bin) if usable(bin) => read_repo(bin, &dir),
            _ => Err(ErrorCode::NotReady),
        });
        let payload = read.and_then(|s| serde_json::to_value(s).map_err(|_| ErrorCode::Internal));
        let (outcome, observation) = read_result(payload, SOURCE, "git status", t);
        self.sync.record(t, &outcome, observation.as_ref());
        (outcome, observation)
    }
    /// 決めた git の場所（`at` でない時）と記録を捨てる。
    fn cleanup(&mut self) {
        if !self.pinned {
            self.bin = None;
        }
        self.sync = SourceSync::default();
    }
}

/// 1 回の読み（check の `--version`・invoke の 4 つの git を合わせて）の時間の上限。
const LIMIT: Duration = Duration::from_secs(5);
/// Command Line Tools の shim。CLT が無い Mac で叩くと install の窓を出して前面を取る。
const SYSTEM_GIT: &str = "/usr/bin/git";
/// status の出力の上限（`repos::wait_bounded` が残すのは 1MB + 1 byte まで）。超えたら途中で切れた一覧を返さず Internal。
// ponytail: 1MB を超える変更の一覧（untracked が数万 file）は読めない。要るなら status を -uno にするか件数だけ数える
const MAX_STATUS: usize = 1 << 20;

/// PATH の上の git → /opt/homebrew/bin/git → /usr/local/bin/git → /usr/bin/git の順で、使える最初の 1 つ。
/// launchd から起こした .app の PATH は `/usr/bin:/bin:…` なので、PATH の上でも /usr/bin/git に着く（`usable` が CLT を見る）。
fn find_git() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join("git"))
        .chain(["/opt/homebrew/bin/git", "/usr/local/bin/git", SYSTEM_GIT].map(PathBuf::from))
        .find(|p| usable(p))
}

/// 実行できる file で、/usr/bin/git（の symlink を含む）なら `xcode-select -p` が 0 の時だけ。
fn usable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let exec = p
        .metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    let shim = p.canonicalize().is_ok_and(|c| c == Path::new(SYSTEM_GIT));
    exec && (!shim || clt_installed())
}

/// Command Line Tools（か Xcode）が入っているか。`xcode-select -p` は path を言うだけで窓を出さない。
fn clt_installed() -> bool {
    Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()
        .and_then(|c| wait_bounded(c, LIMIT).ok().flatten())
        .is_some_and(|(s, _, _)| s.success())
}

/// `git --no-optional-locks [-C dir] <args>` を `until` まで走らせる。Ok は (exit 0 か, stdout)。
/// 環境の GIT_DIR・GIT_WORK_TREE・GIT_INDEX_FILE を外し（`dir` だけを読む）、LC_ALL=C（誤りの文を英語に揃える）・fsmonitor を起こさない。
fn git(
    bin: &Path,
    dir: Option<&Path>,
    args: &[&str],
    until: Instant,
) -> Result<(bool, Vec<u8>), ErrorCode> {
    let mut cmd = Command::new(bin);
    cmd.args(["--no-optional-locks", "-c", "core.fsmonitor=false"]);
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|_| ErrorCode::NotReady)?;
    match wait_bounded(child, until.saturating_duration_since(Instant::now())) {
        Ok(Some((status, out, _))) => Ok((status.success(), out)),
        Ok(None) => Err(ErrorCode::Timeout),
        Err(_) => Err(ErrorCode::Internal),
    }
}

/// input の `uri`（`file://` + 絶対 path・percent の解き直しはしない = composer は生の path を書く）→ dir。形が違えば InvalidInput。
fn repo_dir(input: &Value) -> Result<PathBuf, ErrorCode> {
    let uri = input
        .get("uri")
        .and_then(Value::as_str)
        .ok_or(ErrorCode::InvalidInput)?;
    let path = uri
        .strip_prefix("file://")
        .filter(|p| p.starts_with('/'))
        .ok_or(ErrorCode::InvalidInput)?;
    Ok(PathBuf::from(path))
}

/// repo を読む: work tree か（違えば TargetNotFound）→ status → HEAD（unborn は None）→ branch（detached は None）。4 つで `LIMIT` を分け合う。
fn read_repo(bin: &Path, dir: &Path) -> Result<RepoStatus, ErrorCode> {
    if !dir.is_dir() {
        return Err(ErrorCode::TargetNotFound);
    }
    let until = Instant::now() + LIMIT;
    let run = |args: &[&str]| git(bin, Some(dir), args, until);
    match run(&["rev-parse", "--is-inside-work-tree"])? {
        (true, out) if out.trim_ascii() == b"true" => {}
        _ => return Err(ErrorCode::TargetNotFound),
    }
    let (ok, out) = run(&["status", "--porcelain=v1", "-z"])?;
    if !ok || out.len() > MAX_STATUS {
        return Err(ErrorCode::Internal);
    }
    let changes = parse_porcelain(&out)?;
    let (ok, out) = run(&["rev-parse", "-q", "--verify", "HEAD"])?;
    let sha = match (ok, String::from_utf8_lossy(&out).trim().to_owned()) {
        (false, _) => None,
        (true, s)
            if (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Some(s)
        }
        (true, _) => return Err(ErrorCode::Internal),
    };
    let (ok, out) = run(&["branch", "--show-current"])?;
    if !ok {
        return Err(ErrorCode::Internal);
    }
    let branch = Some(String::from_utf8_lossy(&out).trim().to_owned()).filter(|b| !b.is_empty());
    Ok(RepoStatus {
        head: Head { sha, branch },
        changes,
    })
}

/// G81: `FILE_READ` の読み。uri（`repo_dir` と同じ形 = `file://` + 絶対 path）の file を、`..` を持たず、symlink を解いた先の上のどこかに `.git` が在る
/// （git の repo の work tree の中の）時だけ読む = 鍵の置き場など repo の外は TargetNotFound。file が無い（保存の途中の置き換え・まだ保存していない・その folder も無い）は
/// exists = false・text = 空（読みは止めない・Engine の読み戻しは合うまで読み直す）。UTF-8 でない byte は置き換え、`MAX_STATUS` を超えたら頭だけで complete = false。
// ponytail: 1MB を超える書類は頭だけ。要るなら読み戻しの字の在る所だけを探す読みに
fn read_file(input: &Value) -> Result<Value, ErrorCode> {
    use std::io::Read;
    let path = repo_dir(input)?;
    if path.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(ErrorCode::InvalidInput);
    }
    let missing = || serde_json::json!({ "exists": false, "text": "", "complete": true });
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(ErrorCode::TargetNotFound);
    };
    let at = match dir.canonicalize() {
        Ok(dir) => dir.join(name),
        // 書類の folder もまだ無い（まだ保存していない）: 在る所まで上って repo の中かだけを見る（読みは止めない）
        Err(_) => {
            let base = dir.ancestors().find(|d| d.exists()).and_then(|d| d.canonicalize().ok()).ok_or(ErrorCode::TargetNotFound)?;
            return if base.ancestors().any(|d| d.join(".git").exists()) { Ok(missing()) } else { Err(ErrorCode::TargetNotFound) };
        }
    };
    let at = if at.exists() { at.canonicalize().map_err(|_| ErrorCode::TargetNotFound)? } else { at };
    if !at.ancestors().skip(1).any(|d| d.join(".git").exists()) {
        return Err(ErrorCode::TargetNotFound);
    }
    let file = match std::fs::File::open(&at) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(missing()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Err(ErrorCode::PermissionDenied),
        Err(_) => return Err(ErrorCode::Internal),
    };
    let mut buf = Vec::new();
    file.take(MAX_STATUS as u64 + 1).read_to_end(&mut buf).map_err(|_| ErrorCode::Internal)?;
    let complete = buf.len() <= MAX_STATUS;
    buf.truncate(MAX_STATUS);
    Ok(serde_json::json!({ "exists": true, "text": String::from_utf8_lossy(&buf), "complete": complete }))
}

/// `status --porcelain=v1 -z`: 1 件 = `XY<空白>path\0`。X か Y が R / C の時だけ次の `\0` までが元の path（`-z` は「新 元」の順）。
fn parse_porcelain(out: &[u8]) -> Result<Vec<Change>, ErrorCode> {
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut fields = out.split(|&b| b == 0).filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let Some(f) = fields.next() {
        if f.len() < 4 || f[2] != b' ' {
            return Err(ErrorCode::Internal);
        }
        let orig_path = if f[..2].iter().any(|&b| b == b'R' || b == b'C') {
            Some(text(fields.next().ok_or(ErrorCode::Internal)?))
        } else {
            None
        };
        changes.push(Change {
            status: text(&f[..2]),
            path: text(&f[3..]),
            orig_path,
        });
    }
    Ok(changes)
}
