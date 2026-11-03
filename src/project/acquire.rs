//! PV0（図 §49・spec §10-2 の acquisition の最小形）: 一言が名指した app がこの Mac に無い席（W13b の 1 問）を、
//! 承認の後に Homebrew の cask で 1 つだけ入れ、入れた .app を読み戻してから席を本物の面に替える（一言は打ち直さない）。
//! 新しい engine・goal planner・shell の解釈器を作らない。入れ方は前もって確かめた表（`CATALOG`）の 1 件だけで、
//! model が書いた URL・bundle id・command は使わない（LLM の URL と command を信じない・禁止の 7 つ = PV-§1-c）。
//! Homebrew 自体は入れない（無ければ公式の頁へ渡すだけ）。Gatekeeper・quarantine を弱める引数を渡さない。管理者の操作・account・支払いは
//! Shibubu の中で起こさない（表の物は app と command の symlink だけを置く cask・stdin は null で sudo の問いに答えない = ACQ-16 の半分）。
//! 再開の仕組みは持たない（PV0）: 落ちた後は 1 問が残り、押し直すと先に実際の Mac を読む（`Homebrew::resolve` の `Present`・ACQ-08 の前倒し）。
//! 置き場が capability_engine でなく project の理由: 席の 1 問を描く generated.rs は e2e の試験の crate（tests/yc_demo_e2e.rs・mirror_e0.rs）が
//! `#[path]` で引き、その crate は capability_engine と repos を持たない（generated.rs は Engine の型を持たない約束・図 §33）。
//! この file は std と serde_json だけを使う（ui・composer に依らない = acquire だけを `#[path]` で引く反証の crate〈tests/pv0_attack.rs〉が compile できる）。
//! PV1 の発見の手（ui の型を作る）は `project::discover`。

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 前もって確かめた入れ方 1 件（Homebrew の公式の cask の listing と同じ値を書き写した物・確かめた日と出所は `CATALOG` の doc）。
/// 契約（PV1 で欄を足した）: `web`・`license`・`price` = 発見の道具の行と 1 問の web 版の答えに出す公式の頁で読んだ値。
/// Wave: PV0, PV1
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// 席の `app_name` と比べる名前（大小を問わない・前後の空白を落とす）
    pub app_name: &'static str,
    /// 同じ app の別の呼び名（cask の `name`）
    pub aliases: &'static [&'static str],
    /// Homebrew の cask の token（`brew install --cask <token>` に渡すのはこれだけ）
    pub cask: &'static str,
    /// cask の app の artifact（`apps_dir` に入る .app の名前）
    pub app_file: &'static str,
    /// 読み戻しで比べる CFBundleIdentifier（違えば Verified にしない）
    pub bundle_id: &'static str,
    /// review に出す発行元
    pub publisher: &'static str,
    /// cask の `url` の host がこのどれかでなければ入れない
    pub source_hosts: &'static [&'static str],
    /// Homebrew が無い・入れられない時の次の一手（公式の頁・https）。model の `install_url` の代わりに出す
    pub official_page: &'static str,
    /// PV1: 入れずに使える公式の web 版（https・無ければ None）。発見の行の Web option と 1 問の `Use <名前> in the browser`
    pub web: Option<&'static str>,
    /// PV1: license の名前（公式の頁で読んだ物）
    pub license: &'static str,
    /// PV1: 値段（公式の頁で読んだ物）
    pub price: &'static str,
}

/// PV0 の表（1 件 = spec §12 の demo の候補「Godot」）。値は https://formulae.brew.sh/api/cask/godot.json（2026-10-29 に読んだ）の
/// token・name・artifacts（app = Godot.app・binary = godot・uninstall は quit だけ・pkg / installer / script 無し）・url の host・homepage。
/// 版は書かない（入れる直前に `brew info` で読む）。足す時は同じ読みを 1 件ずつしてから（listing が変われば `candidate` が拒む）。
/// PV1 の欄: web = https://editor.godotengine.org/releases/latest/（2026-10-29 に `Godot Engine Web Editor (4.7.2.stable.official)` と名乗るのを読んだ・
/// 版は web 版の物なので書かない）・license と price = https://godotengine.org/license/（同じ日に「released under the permissive MIT license」・
/// 「You are free to use Godot Engine, for any purpose」を読んだ）。
/// Wave: PV0, PV1
pub static CATALOG: [Entry; 1] = [Entry {
    app_name: "Godot",
    aliases: &["Godot Engine"],
    cask: "godot",
    app_file: "Godot.app",
    bundle_id: "org.godotengine.godot",
    publisher: "Godot Engine",
    source_hosts: &["github.com"],
    official_page: "https://godotengine.org/",
    web: Some("https://editor.godotengine.org/releases/latest/"),
    license: "MIT",
    price: "Free",
}];


/// 席の app の名前で表を引く（名前か別名が大小を問わず同じ時だけ）。model の bundle id・URL は見ない。
/// 例: `acquisition::entry_for("godot")` → `Some(&CATALOG[0])`・`entry_for("Zeta Edit")` → None
/// Wave: PV0
pub fn entry_for(app_name: &str) -> Option<&'static Entry> {
    let want = app_name.trim();
    CATALOG.iter().find(|e| std::iter::once(e.app_name).chain(e.aliases.iter().copied()).any(|n| n.eq_ignore_ascii_case(want)))
}

/// 導入の段（O21 の 6 語 = PV-013 の正）。同じ場所の 1 行の頭に「<段> (n of 6)」で出す。
/// Wave: PV0
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Goal,
    Discover,
    Resolve,
    ApproveInstall,
    Verify,
    Compose,
}

impl Stage {
    pub const ALL: [Stage; 6] = [Stage::Goal, Stage::Discover, Stage::Resolve, Stage::ApproveInstall, Stage::Verify, Stage::Compose];

    /// O21 の語のまま（翻訳しない）。
    pub const fn label(self) -> &'static str {
        match self {
            Stage::Goal => "Goal",
            Stage::Discover => "Discover",
            Stage::Resolve => "Resolve",
            Stage::ApproveInstall => "Approve & Install",
            Stage::Verify => "Verify",
            Stage::Compose => "Compose",
        }
    }

    /// 1 行の頭: `Resolve (3 of 6)`。
    pub fn row(self) -> String {
        let n = Self::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1;
        format!("{} ({n} of {})", self.label(), Self::ALL.len())
    }
}

/// Homebrew の listing で確かめた 1 件（承認の前の review に出す物）。
/// Wave: PV0
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entry: &'static Entry,
    /// listing の `version`（承認した版）
    pub version: String,
    /// listing の `url` の host（review の `Source:`）
    pub source: String,
    /// この Mac に起きる変更（artifact ごとに 1 句: `Adds Godot.app to Applications`・`Adds the godot command`）
    pub changes: Vec<String>,
}

/// 読み戻した .app（`apps_dir/<app_file>` の Info.plist）。
/// Wave: PV0
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub bundle_id: String,
    /// CFBundleShortVersionString（読めなければ `Unknown`）
    pub version: String,
    pub path: PathBuf,
}

/// `Homebrew::resolve` の答え。
/// Wave: PV0
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// 入れる前の review に出す
    Review(Candidate),
    /// 既にこの Mac に在る（読み戻しが表の bundle id と合った）→ 入れずに席を替える（ACQ-02・二重に入れない）
    Present(Installed),
    /// 入れない（Homebrew が無い・listing が読めない・表と違う〈token・host・pkg / installer / script〉）。理由と次の一手の 1 行
    Refused(String),
}

/// `Homebrew::install` / `Homebrew::check` の答え。
/// Wave: PV0
#[derive(Debug, Clone, PartialEq)]
pub enum Installation {
    /// 読み戻しが表の bundle id と合った（これだけが Verified・ACQ-10）
    Verified(Installed),
    /// 入らなかったと言える（Homebrew が 0 以外で終わり .app が無い・入れる前から別の物が在る）。Retry は出さない
    Failed(String),
    /// 入ったか分からない（時間切れ・0 で終わったのに .app が無い・Homebrew が走った後の .app が別の bundle id か読めない）。
    /// Retry は出さず `Check again`（読み戻しだけ）・「入った」とも「何も置いていない」とも言わない
    Unsure(String),
    /// 承認の後に listing が替わった（版・出所・この Mac に起きる変更のどれか）: 入れていない。替わった中身の review に戻す（もう一度の承認を待つ）
    Changed(Candidate),
}

/// Homebrew の口（決まった引数だけ・shell を通さない・`bounded` で時間を切る）。`Git::at` と同じ形で、試験は偽の brew と
/// 一時の `apps_dir` を渡す（この Mac に本物を入れない）。
/// Wave: PV0
#[derive(Debug, Clone, PartialEq)]
pub struct Homebrew {
    /// 使う brew（None = この Mac に無い → Install を出さない）
    pub brew: Option<PathBuf>,
    /// cask の app を置く dir（`--appdir`・製品は /Applications）
    pub apps_dir: PathBuf,
    /// `brew info` の時間の上限
    pub info_limit: Duration,
    /// `brew install` の時間の上限（超えたら止めて Unsure）
    pub install_limit: Duration,
}

impl Homebrew {
    /// 製品の口: brew は Homebrew の既定の 2 つの置き場（/opt/homebrew・/usr/local）だけを見る（PATH は見ない）・apps_dir = /Applications。
    /// 試験の build（cfg(test)）では brew = None（試験が偽の brew を渡し忘れても本物を叩かない）。
    pub fn find() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let runs = |p: &PathBuf| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
        // ponytail: Homebrew の既定の 2 つの置き場だけ（PATH に置かれた別の brew を拾わない）。別の prefix の Homebrew は Install が出ない。G99 の resolver が来たら寄せる
        let brew = if cfg!(test) { None } else { ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].into_iter().map(PathBuf::from).find(runs) };
        Self { brew, apps_dir: PathBuf::from("/Applications"), info_limit: Duration::from_secs(60), install_limit: Duration::from_secs(15 * 60) }
    }

    /// 決まった引数の brew を固定する（試験）。
    pub fn at(brew: PathBuf, apps_dir: PathBuf) -> Self {
        Self { brew: Some(brew), apps_dir, info_limit: Duration::from_secs(10), install_limit: Duration::from_secs(30) }
    }

    /// 入れる前の段: 先に実際の Mac を読み（在れば `Present`）、無ければ `brew info --cask --json=v2 <token>` の listing を表と照らす。何も入れない。
    pub fn resolve(&self, entry: &'static Entry) -> Resolution {
        match readback(entry, &self.apps_dir) {
            Some(Ok(i)) => return Resolution::Present(i),
            // 同じ名前の別の物が在る: 上書きしない
            Some(Err(why)) => return Resolution::Refused(format!("{why} · Nothing was placed")),
            None => {}
        }
        let Some(brew) = &self.brew else { return Resolution::Refused(no_brew(entry)) };
        self.listed(brew, entry).map_or_else(Resolution::Refused, Resolution::Review)
    }

    /// `brew info --cask --json=v2 <token>` の listing を表と照らす（何も入れない）。Err = 入れない理由と次の一手の 1 行。
    fn listed(&self, brew: &Path, entry: &'static Entry) -> Result<Candidate, String> {
        let (name, page) = (entry.app_name, host(entry.official_page));
        match bounded(&mut brew_cmd(brew, &["info", "--cask", "--json=v2", entry.cask]), self.info_limit) {
            Ok(Some((true, out, _))) => serde_json::from_str::<Value>(&out)
                .map_err(|_| format!("Couldn't read Homebrew's listing for {name} · Get it from {page}"))
                .and_then(|listing| candidate(entry, &listing)),
            Ok(Some((false, _, err))) => Err(format!("Homebrew couldn't find {name} · {} · Get it from {page}", last_line(&err))),
            Ok(None) => Err(format!("Homebrew didn't answer in {} s · Get {name} from {page}", self.info_limit.as_secs())),
            Err(e) => Err(format!("Couldn't run Homebrew ({e}) · Get {name} from {page}")),
        }
    }

    /// 承認の後だけ呼ぶ: 直前にもう一度 Mac を読み（在れば入れない）、listing を照らし直し（承認した 1 件と違えば入れない = `Changed`・照らせなければ `Failed`）、
    /// `brew install --cask --appdir=<apps_dir> <token>` を 1 回・読み戻す。
    pub fn install(&self, c: &Candidate) -> Installation {
        let e = c.entry;
        // 承認から今までの間に入った（利用者が入れた・前の Homebrew が後で終えた）なら入れない
        match readback(e, &self.apps_dir) {
            Some(Ok(i)) => return Installation::Verified(i),
            Some(Err(why)) => return Installation::Failed(format!("{why} · Nothing was placed")),
            None => {}
        }
        let Some(brew) = &self.brew else { return Installation::Failed(no_brew(e)) };
        // 承認から今までの間に listing が替わった（版・出所・script 入り）なら入れない（review で見せた物と入る物を同じにする）
        match self.listed(brew, e) {
            Ok(now) if now == *c => {}
            Ok(now) => return Installation::Changed(now),
            Err(why) => return Installation::Failed(why),
        }
        let args = self.install_args(e);
        let ran = bounded(&mut brew_cmd(brew, &args.iter().map(String::as_str).collect::<Vec<_>>()), self.install_limit);
        let look = "Look in Applications, then press Check again";
        match (ran, readback(e, &self.apps_dir)) {
            // 止めた: 途中まで写した .app かもしれない（読めても Verified にしない）
            (Ok(None), _) => Installation::Unsure(format!("Homebrew didn't finish installing {} in {} min · {look}", e.app_name, self.install_limit.as_secs().div_ceil(60))),
            (Err(why), _) => Installation::Failed(format!("Couldn't run Homebrew ({why}) · Get {} from {}", e.app_name, host(e.official_page))),
            (Ok(Some(_)), Some(Ok(i))) => Installation::Verified(i),
            // Homebrew が走った後に別の物（か読めない物）: Homebrew がこの Mac に置いたかもしれない = 入ったとも何も置いていないとも言わない
            (Ok(Some(_)), Some(Err(why))) => Installation::Unsure(format!("Homebrew ran · {why} · It wasn't put on this page · Check again")),
            (Ok(Some((true, ..))), None) => Installation::Unsure(format!("Homebrew finished, but {} isn't in Applications · {look}", e.app_file)),
            (Ok(Some((false, _, err))), None) => Installation::Failed(format!("Homebrew couldn't install {} · {} · Get it from {}", e.app_name, last_line(&err), host(e.official_page))),
        }
    }

    /// `Check again`: 読み戻しだけ（入れない・brew を叩かない）。別の物・読めない物は Unsure のまま（Homebrew が走った後なので「何も置いていない」と言わない）。
    pub fn check(&self, entry: &'static Entry) -> Installation {
        match readback(entry, &self.apps_dir) {
            Some(Ok(i)) => Installation::Verified(i),
            Some(Err(why)) => Installation::Unsure(format!("{why} · It wasn't put on this page · Check again")),
            None => Installation::Failed(format!("{} isn't in Applications · Nothing was placed · Get it from {}", entry.app_file, host(entry.official_page))),
        }
    }

    /// install の argv（brew の後ろ）。試験が引数を縛る。
    pub fn install_args(&self, entry: &Entry) -> Vec<String> {
        vec!["install".into(), "--cask".into(), format!("--appdir={}", self.apps_dir.display()), entry.cask.into()]
    }
}

/// brew の command（決まった引数・親の HOMEBREW_* と SUDO_ASKPASS を全部外してから BREW_ENV だけを足す）。
fn brew_cmd(brew: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(brew);
    c.args(args);
    // 親の HOMEBREW_*（HOMEBREW_CASK_OPTS の --no-quarantine・--appdir 等 = review に無い引数）を渡さない。
    // ponytail: env の口だけ。Homebrew の brew.env（/etc/homebrew・~/.homebrew）は env では塞げない（Homebrew 自身が読む）
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("HOMEBREW_") {
            c.env_remove(&k);
        }
    }
    c.env_remove("SUDO_ASKPASS").envs(BREW_ENV);
    c
}

/// Homebrew の無い Mac の 1 行（Homebrew を入れない・公式の頁へ）。`Generated::install_act` が Install を押された時にも同じ 1 行を返す。
/// Wave: PV0
pub fn no_brew(e: &Entry) -> String {
    format!("Homebrew isn't on this Mac, so Shibubu won't install {} itself · Get it from {}", e.app_name, host(e.official_page))
}

/// https の URL の host（userinfo と port を落とす）。https でなければ空。
pub(super) fn host(url: &str) -> &str {
    let Some(rest) = url.strip_prefix("https://") else { return "" };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    host.split(':').next().unwrap_or("")
}

/// stderr の最後の空でない行（HOME を `~` に縮め、長ければ 200 文字で切る・空なら決まった 1 句）。
fn last_line(err: &str) -> String {
    let l = err.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no reason given");
    tilde(l, &std::env::var("HOME").unwrap_or_default()).chars().take(200).collect()
}

/// 文の中の HOME（`/Users/<名前>`）を `~` に縮める（画面・log・usage report に利用者の名前を出さない）。縮めるのは HOME の後が path の続きでない時だけ
/// （`/Users/a` は `/Users/ab` を縮めない）・HOME が空か `/` なら何もしない。例: `tilde("rb_sysopen - /Users/a/Library/x", "/Users/a")` → `rb_sysopen - ~/Library/x`
/// ponytail: capability_engine の scrub と同じ役の写し（あちらは e2e の crate に無い）
/// Wave: PV0
pub fn tilde(line: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return line.to_owned();
    }
    let (mut out, mut rest) = (String::new(), line);
    while let Some(i) = rest.find(home) {
        let after = &rest[i + home.len()..];
        let ends = after.chars().next().map_or(true, |c| !(c.is_alphanumeric() || c == '-' || c == '_'));
        out.push_str(&rest[..i]);
        out.push_str(if ends { "~" } else { home });
        rest = after;
    }
    out + rest
}

/// 子 process を 1 回（stdin null・shell なし）。limit を超えたら止めて Ok(None)。Ok(Some((0 で終わったか, stdout, stderr)))。
// ponytail: repos::wait_bounded と同じ形の写し（repos は generated.rs を引く e2e の crate に無い）。brew の子（curl）は止めない。止めるなら process group で
fn bounded(cmd: &mut Command, limit: Duration) -> Result<Option<(bool, String, String)>, String> {
    fn drain<R: std::io::Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            String::from_utf8_lossy(&b).into_owned()
        })
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.kind().to_string())?;
    let (out, err) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let until = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    };
    Ok(Some((status.success(), out.join().unwrap_or_default(), err.join().unwrap_or_default())))
}

/// brew に渡す環境（足す物）。HOMEBREW_NO_AUTO_UPDATE = review に無い変更（Homebrew 自体と tap の更新）をしない・
/// HOMEBREW_NO_ANALYTICS = 利用者の代わりに外へ送らない・NONINTERACTIVE = 問いで止まらない。親の HOMEBREW_* は全部外してから足す（`brew_cmd`）・
/// SUDO_ASKPASS は外す（管理者の問いに答えない）。
/// Wave: PV0
pub const BREW_ENV: [(&str, &str); 3] = [("HOMEBREW_NO_AUTO_UPDATE", "1"), ("HOMEBREW_NO_ANALYTICS", "1"), ("NONINTERACTIVE", "1")];

/// listing（`brew info --cask --json=v2` の JSON）を表の 1 件と照らす（純粋な関数・試験はこれを直に呼ぶ）。
/// 通る = `casks[0]` の token が表と同じ・`url` が https で host が `source_hosts` のどれか・disabled でない・version が在る・
/// artifacts が app（表の `app_file` を同じ名前で・別の名前の target は拒む）・binary・uninstall・zap だけ（pkg・installer・preflight / postflight の script・他の物が 1 つでも在れば拒む）。
/// Wave: PV0
pub fn candidate(entry: &'static Entry, listing: &Value) -> Result<Candidate, String> {
    let name = entry.app_name;
    let changed = |why: &str| format!("Homebrew's listing for {name} {why}, so Shibubu won't install it from there · Get it from {}", host(entry.official_page));
    let cask = listing["casks"].get(0).ok_or_else(|| changed("is missing"))?;
    if cask["token"].as_str() != Some(entry.cask) {
        return Err(changed("is for a different package"));
    }
    if cask["disabled"].as_bool() == Some(true) {
        return Err(changed("is disabled"));
    }
    let version = cask["version"].as_str().map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| changed("has no version"))?;
    let source = host(cask["url"].as_str().unwrap_or_default());
    if !entry.source_hosts.contains(&source) {
        return Err(changed(&format!("points to {}", if source.is_empty() { "an address that isn't https" } else { source })));
    }
    let unreadable = || changed("has contents Shibubu can't read");
    let mut changes = Vec::new();
    for a in cask["artifacts"].as_array().ok_or_else(|| changed("has no contents"))? {
        let obj = a.as_object().ok_or_else(unreadable)?;
        let kinds: Vec<&str> = obj.keys().map(String::as_str).filter(|k| *k != "target").collect();
        let [kind] = kinds.as_slice() else { return Err(unreadable()) };
        match *kind {
            "app" => {
                let file = entry.app_file;
                let apps = obj["app"].as_array().filter(|a| (1..=2).contains(&a.len())).ok_or_else(unreadable)?;
                if apps[0].as_str() != Some(file) {
                    return Err(changed("installs a different app"));
                }
                // 別の名前で置く listing（`["Godot.app", {"target": "Evil.app"}]`）は拒む: review で言う名前と入る名前を同じにする・読み戻しは表の名前を見る
                let inner = apps.get(1).map(|t| t["target"].as_str().ok_or_else(unreadable)).transpose()?;
                let outer = obj.get("target").map(|t| t.as_str().ok_or_else(unreadable)).transpose()?;
                let renamed = inner.filter(|t| *t != file).or_else(|| outer.filter(|t| *t != file && !t.ends_with(&format!("/{file}"))));
                if let Some(other) = renamed {
                    return Err(changed(&format!("now puts {file} in Applications as {other}")));
                }
                changes.push(format!("Adds {file} to Applications"));
            }
            "binary" => {
                let named = obj["binary"].get(1).and_then(|t| t["target"].as_str());
                let cmd = named.or_else(|| obj.get("target").and_then(Value::as_str).and_then(|t| t.rsplit('/').next())).ok_or_else(unreadable)?;
                changes.push(format!("Adds the {cmd} command"));
            }
            // 消す時だけ効く（PV0 は消さない）
            "uninstall" | "zap" => {}
            other => return Err(changed(&format!("now runs {other}"))),
        }
    }
    let app = format!("Adds {} to Applications", entry.app_file);
    if changes.iter().filter(|c| **c == app).count() != 1 {
        return Err(changed("doesn't install exactly one app"));
    }
    Ok(Candidate { entry, version: version.to_owned(), source: source.to_owned(), changes })
}

/// review の 1 行（`Approve & Install (4 of 6) · Will install …`）。
/// Wave: PV0
pub fn review_line(c: &Candidate) -> String {
    review(c, "")
}

/// 承認の後に listing が替わって入れなかった時の review の 1 行（`Installation::Changed`・替わった中身を言い直し、もう一度の承認を待つ）。
/// Wave: PV0
pub fn changed_review_line(c: &Candidate) -> String {
    review(c, "Homebrew's listing changed after you approved, so nothing was installed · ")
}

fn review(c: &Candidate, note: &str) -> String {
    let e = c.entry;
    format!(
        "{} · {note}Will install {} {} · Publisher: {} · Source: {} · Via: Homebrew (cask {}) · {} · Will request: nothing at install",
        Stage::ApproveInstall.row(),
        e.app_name,
        c.version,
        e.publisher,
        c.source,
        e.cask,
        c.changes.join(" · ")
    )
}

/// `apps_dir/<app_file>` を読む: None = .app が無い・Some(Ok) = 表の bundle id と合う・Some(Err) = 在るが別の物か読めない（何が在るかの 1 句・
/// 「何も置いていない」等の段の言葉は呼ぶ側が足す）。
/// Wave: PV0
pub fn readback(entry: &Entry, apps_dir: &Path) -> Option<Result<Installed, String>> {
    let path = apps_dir.join(entry.app_file);
    if !path.exists() {
        return None;
    }
    let plist = path.join("Contents/Info.plist");
    let read = |key: &str| -> Option<String> {
        let mut c = Command::new("/usr/bin/plutil");
        c.args(["-extract", key, "raw", "-o", "-"]).arg(&plist);
        match bounded(&mut c, Duration::from_secs(5)) {
            Ok(Some((true, out, _))) => Some(out.trim().to_owned()).filter(|v| !v.is_empty()),
            _ => None,
        }
    };
    let file = entry.app_file;
    // 「Found」と言わない: Homebrew が走った後にも同じ 1 句を使う（前から在った物と読ませない・反証 pv0x_a_wrong_app_installed_by_brew_is_not_reported_as_found）
    let Some(id) = read("CFBundleIdentifier") else { return Some(Err(format!("Applications has a {file}, but Shibubu couldn't read what it is"))) };
    if id != entry.bundle_id {
        return Some(Err(format!("Applications has a {file}, but it is {id}, not {}", entry.app_name)));
    }
    Some(Ok(Installed { bundle_id: id, version: read("CFBundleShortVersionString").unwrap_or_else(|| "Unknown".into()), path }))
}

#[cfg(test)]
pub(crate) mod pv0_tests {
    use super::*;
    use serde_json::json;

    /// Homebrew の listing の形（2026-10-29 の godot の artifacts を写した物）。`version` と `url` だけ替えて使う。
    pub(crate) fn listing(version: &str, url: &str) -> Value {
        json!({ "formulae": [], "casks": [{
            "token": "godot", "name": ["Godot Engine"], "version": version, "homepage": "https://godotengine.org/", "url": url,
            "deprecated": false, "disabled": false,
            "artifacts": [
                { "uninstall": [{ "quit": "org.godotengine.godot" }] },
                { "app": ["Godot.app"], "target": "/Applications/Godot.app" },
                { "binary": ["$APPDIR/Godot.app/Contents/MacOS/Godot", { "target": "godot" }], "target": "$HOMEBREW_PREFIX/bin/godot" },
                { "zap": [{ "trash": ["~/Library/Application Support/Godot"] }] }
            ]
        }]})
    }

    pub(crate) const URL: &str = "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_macos.universal.zip";

    /// 表は名前か別名で引く（model の bundle id・URL を見ない）。表に無い app は None（Install を出さない）。
    /// 負の対照: 大小を見る比べにする → `godot` で赤。
    #[test]
    fn pv0_the_catalog_is_looked_up_by_name_only() {
        assert_eq!(entry_for("godot").map(|e| e.cask), Some("godot"));
        assert_eq!(entry_for(" Godot Engine ").map(|e| e.cask), Some("godot"));
        assert_eq!(entry_for("Zeta Edit"), None);
        assert_eq!(entry_for("org.godotengine.godot"), None, "bundle id で引いた");
    }

    /// listing が表と合う時だけ候補・review に版と発行元と出所と変更（app と command）が出る。
    /// 負の対照: host の照合を外す → evil.example で赤／artifacts の許す種類に pkg を足す → pkg で赤／app の target の照合を外す → Evil.app で赤。
    #[test]
    fn pv0_the_listing_must_match_the_catalog() {
        let e = &CATALOG[0];
        let c = candidate(e, &listing("4.7.2", URL)).expect("合う listing");
        assert_eq!((c.version.as_str(), c.source.as_str()), ("4.7.2", "github.com"));
        assert_eq!(c.changes, ["Adds Godot.app to Applications", "Adds the godot command"]);
        let line = review_line(&c);
        assert!(line.starts_with("Approve & Install (4 of 6) · Will install Godot 4.7.2"), "{line}");
        for want in ["Publisher: Godot Engine", "Source: github.com", "Homebrew", "Adds Godot.app to Applications", "Adds the godot command"] {
            assert!(line.contains(want), "{want} が無い: {line}");
        }
        // model の偽の domain・http・版の無い listing・token の違い・pkg・script・別の app・disabled は通らない（ACQ-05・ACQ-06）
        assert!(candidate(e, &listing("4.7.2", "https://evil.example/Godot.zip")).is_err());
        assert!(candidate(e, &listing("4.7.2", "https://github.com.evil.example/x.zip")).is_err());
        assert!(candidate(e, &listing("4.7.2", "http://github.com/godotengine/x.zip")).is_err());
        assert!(candidate(e, &listing("", URL)).is_err());
        let mut other = listing("4.7.2", URL);
        other["casks"][0]["token"] = json!("godot-mono");
        assert!(candidate(e, &other).is_err());
        for bad in [json!({ "pkg": ["Godot.pkg"] }), json!({ "installer": [{ "script": { "executable": "install.sh" } }] }), json!({ "postflight": null }), json!({ "app": ["Other.app"] })] {
            let mut l = listing("4.7.2", URL);
            l["casks"][0]["artifacts"].as_array_mut().unwrap().push(bad.clone());
            assert!(candidate(e, &l).is_err(), "{bad} を通した");
        }
        let mut off = listing("4.7.2", URL);
        off["casks"][0]["disabled"] = json!(true);
        assert!(candidate(e, &off).is_err());
        // 別の名前で置く app（反証 pv0x_an_app_artifact_renamed_by_target_is_refused）: 拒む・理由の 1 行に表の名前と置く名前の両方
        for (app, target, other) in [
            (json!(["Godot.app", { "target": "Evil.app" }]), json!("/Applications/Evil.app"), "Evil.app"),
            (json!(["Godot.app"]), json!("/Applications/Evil.app"), "Evil.app"),
            (json!(["Godot.app", { "target": "../x/Godot.app" }]), json!("/Applications/Godot.app"), "../x/Godot.app"),
        ] {
            let mut l = listing("4.7.2", URL);
            for a in l["casks"][0]["artifacts"].as_array_mut().unwrap() {
                if a.get("app").is_some() {
                    *a = json!({ "app": app.clone(), "target": target.clone() });
                }
            }
            let why = candidate(e, &l).expect_err("別の名前で置く listing を通した");
            assert!(why.contains("puts Godot.app in Applications as") && why.contains(other), "{why}");
        }
    }

    /// install の argv は決まった物だけ（--appdir・token）・quarantine を外さない・強制しない。段の 1 行の頭は O21 の語。
    /// 負の対照: argv に `--no-quarantine` を足す → 赤。
    #[test]
    fn pv0_install_args_are_fixed_and_keep_quarantine() {
        let h = Homebrew::at(PathBuf::from("/x/brew"), PathBuf::from("/tmp/apps"));
        assert_eq!(h.install_args(&CATALOG[0]), ["install", "--cask", "--appdir=/tmp/apps", "godot"]);
        assert!(!h.install_args(&CATALOG[0]).iter().any(|a| a.contains("quarantine") || a.contains("force") || a.contains("sudo")));
        assert_eq!(Stage::ALL.map(Stage::row), ["Goal (1 of 6)", "Discover (2 of 6)", "Resolve (3 of 6)", "Approve & Install (4 of 6)", "Verify (5 of 6)", "Compose (6 of 6)"]);
        // 試験の build は本物の brew を見つけない
        assert_eq!(Homebrew::find().brew, None);
    }

    /// 偽の brew（sh・この Mac に何も入れない）。`info` は `dir/info.json` を出す。`install` は mode で
    /// ok = `--appdir` に Godot.app（bundle id = org.godotengine.godot・版 4.7.2）を作る・wrong = 別の bundle id の Godot.app・
    /// fail = stderr に 1 行を出して 1 で終わる・slow = 30 秒眠る（時間切れを作る）。呼ばれた argv は `dir/calls`、
    /// 渡された HOMEBREW_* / NONINTERACTIVE / SUDO_ASKPASS は `dir/env` に 1 行ずつ。返すのは brew の path。
    pub(crate) fn fake_brew(dir: &Path, mode: &str, listing: &Value) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("info.json"), listing.to_string()).unwrap();
        let script = format!(
            r#"#!/bin/sh
d='{dir}'
printf '%s\n' "$*" >> "$d/calls"
env | grep -E '^(HOMEBREW_|NONINTERACTIVE|SUDO_ASKPASS)' | sort >> "$d/env"
case "$1" in
  info) cat "$d/info.json" ;;
  install)
    app=''
    for a in "$@"; do case "$a" in --appdir=*) app="${{a#--appdir=}}" ;; esac; done
    case '{mode}' in
      slow) exec sleep 30 ;;
      fail) echo 'Error: Download failed: Godot_v4.7.2-stable_macos.universal.zip' >&2; exit 1 ;;
      wrong) id=com.example.other ;;
      *) id=org.godotengine.godot ;;
    esac
    mkdir -p "$app/Godot.app/Contents"
    printf '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>%s</string><key>CFBundleShortVersionString</key><string>4.7.2</string></dict></plist>' "$id" > "$app/Godot.app/Contents/Info.plist"
    ;;
esac
"#,
            dir = dir.display()
        );
        let brew = dir.join("brew");
        std::fs::write(&brew, script).unwrap();
        std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
        brew
    }

    /// 承認の後に listing が替わったら入れない（反証 pv0x_a_listing_that_changed_after_the_review_is_not_installed）: 版が替わった → `Changed`（替わった中身・
    /// install を叩かない・review の 1 行に替わった事）・照らせない中身に替わった（postflight）→ `Failed`（理由の 1 行）。承認と同じ listing の時だけ入れる。
    /// 負の対照: install の直前の照らし直しを外す → 替わった listing で install が呼ばれて赤。
    #[test]
    fn pv0_a_listing_that_changed_after_approval_is_not_installed() {
        let d = temp("changed");
        let h = Homebrew::at(fake_brew(&d, "ok", &listing("4.7.2", URL)), d.join("apps"));
        let approved = candidate(&CATALOG[0], &listing("4.7.2", URL)).unwrap();
        let count = |what: &str| calls(&d).iter().filter(|c| c.starts_with(what)).count();
        std::fs::write(d.join("info.json"), listing("4.7.3", URL).to_string()).unwrap();
        match h.install(&approved) {
            Installation::Changed(now) => {
                assert_eq!(now.version, "4.7.3");
                let line = changed_review_line(&now);
                assert!(line.starts_with("Approve & Install (4 of 6) · Homebrew's listing changed after you approved, so nothing was installed · Will install Godot 4.7.3"), "{line}");
            }
            other => panic!("版の替わった listing: {other:?}"),
        }
        let mut script = listing("4.7.2", URL);
        script["casks"][0]["artifacts"].as_array_mut().unwrap().push(json!({ "postflight": null }));
        std::fs::write(d.join("info.json"), script.to_string()).unwrap();
        let r = h.install(&approved);
        assert!(matches!(&r, Installation::Failed(l) if l.contains("postflight")), "{r:?}");
        assert_eq!(count("install"), 0, "替わった listing で入れた: {:?}", calls(&d));
        std::fs::write(d.join("info.json"), listing("4.7.2", URL).to_string()).unwrap();
        assert!(matches!(h.install(&approved), Installation::Verified(_)));
        assert_eq!((count("info"), count("install")), (3, 1), "{:?}", calls(&d));
    }

    /// 画面の 1 行に HOME の path を出さない（反証 pv0x_brew_errors_on_screen_carry_no_home_path）: brew の stderr の最後の行の HOME を `~` に。
    /// 負の対照: last_line の tilde を外す → HOME の path が残って赤。
    #[test]
    fn pv0_home_paths_on_screen_are_shortened() {
        assert_eq!(tilde("Error: Permission denied - /Users/a/Library/Caches/x.zip", "/Users/a"), "Error: Permission denied - ~/Library/Caches/x.zip");
        assert_eq!(tilde("at /Users/a and /Users/ab/x", "/Users/a/"), "at ~ and /Users/ab/x");
        assert_eq!(tilde("in /Users/a.", "/Users/a"), "in ~.");
        assert_eq!(tilde("/x/y", ""), "/x/y");
        assert_eq!(tilde("/x/y", "/"), "/x/y");
        let home = std::env::var("HOME").unwrap();
        let line = last_line(&format!("==> Downloading\nError: Permission denied @ rb_sysopen - {home}/Library/Caches/Homebrew/downloads/x.zip\n"));
        assert!(line.contains("~/Library/Caches") && !line.contains(&home), "{line}");
    }

    /// 呼ばれた argv（`dir/calls` の行）。
    pub(crate) fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// 試験ごとの一時 dir（brew と apps）。
    pub(crate) fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shibubu-pv0-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("apps")).unwrap();
        d
    }

    /// resolve は先に実際の Mac を読む: 在れば brew を 1 回も叩かず Present（二重に入れない・ACQ-02）・無ければ info を 1 回だけ（install しない）。
    /// brew が無い Mac は Refused（Homebrew を入れない）。
    /// 負の対照: resolve の先頭の readback を外す → 在る Mac で info が呼ばれ Review になって赤。
    #[test]
    fn pv0_resolve_reads_the_mac_first_and_never_installs() {
        let d = temp("resolve");
        let h = Homebrew::at(fake_brew(&d, "ok", &listing("4.7.2", URL)), d.join("apps"));
        assert!(matches!(h.resolve(&CATALOG[0]), Resolution::Review(c) if c.version == "4.7.2"));
        assert_eq!(calls(&d), ["info --cask --json=v2 godot"], "resolve で info の他を叩いた");
        // 在る Mac（ok の install を 1 回させて Godot.app を置く）→ 次の resolve は brew を叩かない
        assert!(matches!(h.install(&candidate(&CATALOG[0], &listing("4.7.2", URL)).unwrap()), Installation::Verified(_)));
        let before = calls(&d).len();
        assert!(matches!(h.resolve(&CATALOG[0]), Resolution::Present(i) if i.bundle_id == "org.godotengine.godot"));
        assert_eq!(calls(&d).len(), before, "在る app で brew を叩いた");
        // brew の無い Mac（Godot.app も無い置き場 = 先の readback が Present にしない）
        let none = Homebrew { brew: None, apps_dir: d.join("elsewhere"), ..h.clone() };
        assert!(matches!(none.resolve(&CATALOG[0]), Resolution::Refused(l) if l.contains("Homebrew isn't on this Mac") && l.contains("godotengine.org")));
    }

    /// install: 承認した 1 件を決まった argv で 1 回・環境は BREW_ENV（親の HOMEBREW_* と SUDO_ASKPASS を渡さない）・読み戻しが表の bundle id と合う時だけ Verified。
    /// Homebrew が置いた別の bundle id = Unsure（反証 pv0x_a_wrong_app_installed_by_brew_is_not_reported_as_found・入ったとも何も置いていないとも言わない・check も Unsure）・
    /// 0 以外 = Failed（stderr の最後の 1 行）・時間切れ = Unsure・check は brew を叩かない。
    /// 負の対照: readback の bundle id の比べを外す → wrong が Verified で赤／BREW_ENV を渡さない → env で赤／`brew_cmd` の HOMEBREW_* を外す行を消す → env で赤。
    #[test]
    fn pv0_install_verifies_by_readback_and_never_retries() {
        let c = candidate(&CATALOG[0], &listing("4.7.2", URL)).unwrap();
        let run = |mode: &str, limit: Duration| {
            let d = temp(&format!("install-{mode}"));
            let h = Homebrew { install_limit: limit, ..Homebrew::at(fake_brew(&d, mode, &listing("4.7.2", URL)), d.join("apps")) };
            (h.install(&c), d, h)
        };
        // 親の HOMEBREW_*（review に無い cask の引数 = 反証 pv0x_cask_options_from_the_parent_env_do_not_reach_brew）は渡さない（名前はこの試験だけの物）
        std::env::set_var("HOMEBREW_PV0_PROBE_CASK_OPTS", "--no-quarantine");
        let (ok, d, h) = run("ok", Duration::from_secs(30));
        std::env::remove_var("HOMEBREW_PV0_PROBE_CASK_OPTS");
        match &ok {
            Installation::Verified(i) => assert_eq!((i.bundle_id.as_str(), i.version.as_str(), i.path.clone()), ("org.godotengine.godot", "4.7.2", d.join("apps/Godot.app"))),
            other => panic!("{other:?}"),
        }
        let apps = d.join("apps");
        assert_eq!(calls(&d), ["info --cask --json=v2 godot".to_owned(), format!("install --cask --appdir={} godot", apps.display())], "入れる直前に listing を照らし直していない");
        let env = std::fs::read_to_string(d.join("env")).unwrap();
        for want in ["HOMEBREW_NO_AUTO_UPDATE=1", "HOMEBREW_NO_ANALYTICS=1", "NONINTERACTIVE=1"] {
            assert!(env.lines().any(|l| l == want), "{want} が無い: {env}");
        }
        assert!(!env.contains("SUDO_ASKPASS") && !env.contains("HOMEBREW_PV0_PROBE"), "親の env を brew に渡した: {env}");
        // 入った後の 2 回目の install は brew を叩かない（直前の読み）・check も叩かない
        assert!(matches!(h.install(&c), Installation::Verified(_)));
        assert!(matches!(h.check(&CATALOG[0]), Installation::Verified(_)));
        assert_eq!(calls(&d).len(), 2, "入っている app を入れ直した");

        let (wrong, _, h) = run("wrong", Duration::from_secs(30));
        let says = |i: &Installation| matches!(i, Installation::Unsure(l) if l.contains("com.example.other") && l.contains("Check again") && !l.contains("Nothing was placed"));
        assert!(says(&wrong) && matches!(&wrong, Installation::Unsure(l) if l == "Homebrew ran · Applications has a Godot.app, but it is com.example.other, not Godot · It wasn't put on this page · Check again"), "{wrong:?}");
        assert!(says(&h.check(&CATALOG[0])), "Check again が別の app を入った・何も置いていないと言った");
        let (fail, ..) = run("fail", Duration::from_secs(30));
        assert!(matches!(&fail, Installation::Failed(l) if l.contains("Download failed")), "{fail:?}");
        let (slow, d, h) = run("slow", Duration::from_millis(500));
        assert!(matches!(&slow, Installation::Unsure(l) if l.contains("Check again")), "{slow:?}");
        assert!(matches!(h.check(&CATALOG[0]), Installation::Failed(_)), "無い app を check が Verified にした");
        assert_eq!(calls(&d).len(), 2, "check が brew を叩いた");
    }
}
