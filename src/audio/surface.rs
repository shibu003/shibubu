//! 元のアプリを人が直接操作するための native handoff（v0.13 の render_mode = native_handoff・DESIGN.md §14.2 の状態）。
//! 検出（Spotlight の bundle id → 在りかの fallback）→ 起動 / 前に出す（`open -b`。頼まれた時だけ）→ 窓が在るか → 状態。
//! 窓の読み（`WindowReader`）は macOS bridge（v0.14 §5.5・`bridge/macos-surface`・CGWindowList + AX）だけ（YC W18-1 で他の口を消した）。
//! bridge が無い・読めない時は「未確認（理由）」で、ready を名乗らず available_manual に留める
//! （黙って「窓が無い」にしない）。読むだけで、押す・打つは無い。判定は `classify`（純関数）。
//! この file は self-contained（src/bin/kangae-surface.rs・kangae-media・src/intel が `#[path]` で直接取り込む）。分岐は docs/diagrams.md §10・§18。

#[allow(dead_code, unused_imports)]
#[path = "../surface/mod.rs"]
mod portal; // macOS bridge の client（src/surface）

use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// handoff する先のアプリ。
#[derive(Debug, Clone, Copy)]
pub struct App {
    /// 人に見せる名前（`/Applications/<name>.app` の fallback にも使う）
    pub name: &'static str,
    pub bundle_id: &'static str,
    /// `pgrep -x` で探す process 名
    pub process: &'static str,
    /// 入っていない時に案内する入手先
    pub download: &'static str,
}

pub const REAPER: App = App {
    name: "REAPER",
    bundle_id: "com.cockos.reaper",
    process: "REAPER",
    download: "https://www.reaper.fm/download.php",
};

/// Source Surface の状態（DESIGN.md §14.2 のうち native handoff が取る 5 つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// アプリを探している途中（UI は probe の間これを出す）
    Detecting,
    /// 手で開ける・操作できる（未起動・窓が今の画面に無い・窓を確かめる手段が無い）
    AvailableManual,
    /// 起動中で、窓が今の画面に在るのを確かめた
    Ready,
    /// 起動したのに窓が出ない・開けなかった
    Degraded,
    /// この Mac に無い
    Unsupported,
}

impl State {
    pub fn as_str(&self) -> &'static str {
        match self {
            State::Detecting => "detecting",
            State::AvailableManual => "available_manual",
            State::Ready => "ready",
            State::Degraded => "degraded",
            State::Unsupported => "unsupported",
        }
    }
}

/// 窓の 1 件（bridge の window_list から・使う欄だけ）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Window {
    /// CGWindowID（bridge で CG の窓と対応が付かない時は 0）
    pub id: i64,
    pub title: String,
    /// CG の kCGWindowIsOnscreen（bridge で CG と対応が付かない AX の窓は、最小化でない事からの推定）
    pub on_screen: bool,
    /// 今の Space に在るか（W12W-1c・owner 13 通目）。Some = 読む口が知っている（CG の on screen・`from_window_list`）。
    /// None = 分からない: 公開 API では CG の off screen が「別の Space」か「最小化・隠れた」かを分けられない・AX の MainWindow /
    /// FocusedWindow は別の Space の窓も返し得る。保証: None を「今の画面」と数えない（`here`）。
    pub on_current_space: Option<bool>,
    /// 手前ほど大きい。null（重なり順が取れない）は None
    pub z: Option<i64>,
    /// bridge の AX の handle（同じ `WindowReader` の中だけ有効。値の読みに使う）
    pub handle: Option<String>,
}

impl Window {
    /// 今の画面に在ると言えるか（ready の根拠）。使い方: `list.iter().filter(|w| w.here())`。
    /// 保証: on screen かつ今の Space だと読む口が言った時だけ true（`on_current_space` が None なら false = 分からない物を ready にしない）。
    /// 保証しない: 窓が操作できる事（AX で要素に届くかは別。CG に窓が在るだけで「操作できた」と数えない）。
    pub fn here(&self) -> bool {
        self.on_screen && self.on_current_space == Some(true)
    }
}

/// bridge の window_list が AX で窓を取った属性（W12W-1c・`kAXWindowsAttribute` は別の Space の窓を落とすので 3 つを別々に読む）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxAttr {
    /// `kAXWindowsAttribute`（JSON `"windows"`）
    Windows,
    /// `kAXMainWindowAttribute`（JSON `"main_window"`）
    MainWindow,
    /// `kAXFocusedWindowAttribute`（JSON `"focused_window"`）
    FocusedWindow,
}

/// AX の 1 属性の答え。空（答えて 0）と失敗（AXError）を混ぜない（owner 13 通目「0件・タイムアウト・権限エラーを混同しない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxAnswer {
    /// 答えて窓が n 個（n ≥ 1）
    Answered(u32),
    /// 答えて 0 個（`success` で空の配列・`ax_no_value` = MainWindow / FocusedWindow が無い）
    Empty,
    /// 許可が無い（`not_trusted`・`ax_api_disabled`）
    NotTrusted,
    /// 答えなかった（bridge の code そのまま: `ax_cannot_complete` = 忙しい・timeout、`ax_failure` など）
    Failed(String),
}

/// bridge の window_list の `ax_reads[]` の 1 行（W12W-1c で bridge に足す保存形式）:
/// `{"attr": "windows" | "main_window" | "focused_window", "code": "success" | "ax_<AXError の名前>" | "not_trusted", "count": <窓の数>, "elapsed_ms": <その属性の読みにかかった時間>}`。
/// bridge は 3 つの属性の窓を CFEqual で 1 つにまとめて `windows[]` に入れ、3 つとも失敗した時だけ従来の `ax_error` を書く（1 つでも答えれば ax_error 無し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxRead {
    pub attr: AxAttr,
    pub answer: AxAnswer,
    pub elapsed_ms: u64,
}

/// window_list の result から `ax_reads[]` を読む（純関数）。使い方: `from_window_list` が note に「どの属性が・空か失敗か・何 ms」を書く。
/// 保証: 知らない attr の行は捨てる・`ax_reads` が無い（前の bridge）なら空の Vec（ax_error だけで読む従来の道）。
/// 保証しない: 窓が操作できる事・別の Space の窓を AX が返す事（app と macOS の版による）。
pub fn ax_reads(v: &Value) -> Vec<AxRead> {
    let rows = v["ax_reads"].as_array().into_iter().flatten();
    rows.filter_map(|r| {
        let attr = match r["attr"].as_str()? {
            "windows" => AxAttr::Windows,
            "main_window" => AxAttr::MainWindow,
            "focused_window" => AxAttr::FocusedWindow,
            _ => return None,
        };
        let n = r["count"].as_u64().unwrap_or(0) as u32;
        let answer = match r["code"].as_str().unwrap_or("") {
            "success" if n > 0 => AxAnswer::Answered(n),
            "success" | "ax_no_value" => AxAnswer::Empty,
            "not_trusted" | "ax_api_disabled" => AxAnswer::NotTrusted,
            code => AxAnswer::Failed(code.to_string()),
        };
        // Swift の JSONSerialization は小数で書く事がある
        Some(AxRead { attr, answer, elapsed_ms: r["elapsed_ms"].as_f64().unwrap_or(0.0).max(0.0).round() as u64 })
    })
    .collect()
}

/// `ax_reads` の答えなかった・空だった属性を人が読む 1 行に（from_window_list の note）。答えた属性は書かない。
fn ax_reads_line(reads: &[AxRead]) -> String {
    let name = |a: AxAttr| match a {
        AxAttr::Windows => "windows",
        AxAttr::MainWindow => "main_window",
        AxAttr::FocusedWindow => "focused_window",
    };
    let parts: Vec<String> = reads
        .iter()
        .filter_map(|r| match &r.answer {
            AxAnswer::Answered(_) => None,
            AxAnswer::Empty => Some(format!("{} is empty ({}ms)", name(r.attr), r.elapsed_ms)),
            AxAnswer::NotTrusted => Some(format!("{} isn't allowed", name(r.attr))),
            AxAnswer::Failed(c) => Some(format!("{} isn't answering ({c} · {}ms)", name(r.attr), r.elapsed_ms)),
        })
        .collect();
    parts.join(" · ")
}

/// 窓を読んだ口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// macOS bridge（CGWindowList + AX・v0.14 §5.5）。YC W18-1 の後は唯一の口
    Bridge,
}

impl Reader {
    pub fn label(self) -> &'static str {
        match self {
            Reader::Bridge => "macOS bridge",
        }
    }
}

/// 窓の一覧を読んだ結果。
#[derive(Debug, Clone, PartialEq)]
pub enum WindowsRead {
    /// 読めた（空なら窓が無い）。note は bridge を使わなかった理由・AX が読めず CG だけで答えた理由
    Read { list: Vec<Window>, by: Reader, note: Option<String> },
    /// 読めなかった（理由）。no_reader = 読む口がそもそも無い（待っても変わらない）
    Unchecked { why: String, no_reader: bool },
}

/// probe の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub state: State,
    /// 人の言葉の 1 文（次の一手つき）
    pub reason: String,
    /// .app の在りか
    pub app_path: Option<PathBuf>,
    pub version: Option<String>,
    pub pid: Option<u32>,
    /// ready の根拠の窓（今の画面の一番手前）。窓が今の画面に無い時は一番手前の窓
    pub window: Option<Window>,
    /// 窓を読んだ口と、その但し書き（読めなかった時は None）
    pub reader: Option<(Reader, Option<String>)>,
    pub checked_at: SystemTime,
}

/// process 名の完全一致で pid（`pgrep -x`。`pgrep -f` は他人の command line に当たるので使わない）。
pub fn pid_of(process: &str) -> Option<u32> {
    let o = Command::new("/usr/bin/pgrep").args(["-x", process]).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.trim().parse().ok())
}

/// 起動中の process の実行 file（`ps -o comm=` は macOS では full path）。
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    let o = Command::new("/bin/ps").args(["-o", "comm=", "-p", &pid.to_string()]).stderr(Stdio::null()).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// 実行 file を含む `.app` の dir。
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// cmd を走らせ、timeout を過ぎたら kill する。stdout / stderr は thread で drain する。
pub fn run_deadline(cmd: &mut Command, timeout: Duration) -> Result<(ExitStatus, String, String), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Can't start {:?}: {e}", cmd.get_program()))?;
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.by_ref().take(1 << 20).read_to_end(&mut b);
            b
        })
    };
    let out = drain(Box::new(child.stdout.take().ok_or("Can't read the output")?));
    let err = drain(Box::new(child.stderr.take().ok_or("Can't read the output")?));
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("No answer within {} s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let text = |h: std::thread::JoinHandle<Vec<u8>>| String::from_utf8_lossy(&h.join().unwrap_or_default()).into_owned();
    Ok((status, text(out), text(err)))
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(300).collect()
}

/// アプリの .app を探す: この Mac の app の一覧（`app_lines`・Spotlight と置き場の直の走査と動いている app の和集合）から bundle id で引く
/// （/Applications の下を優先）。持っている一覧に無ければ 1 回だけ読み直す（入れた後の「Check again」で見つける）。
/// G98: `project::installed_apps` と同じ一覧（同じ考えを 2 か所に置かない）。
pub fn find_app(app: &App) -> Option<PathBuf> {
    let pick = |lines: &str| {
        let mut found: Vec<PathBuf> = app_line_fields(lines).filter(|(_, _, id)| *id == app.bundle_id).map(|(p, ..)| PathBuf::from(p)).collect();
        found.sort_by_key(|p| !p.starts_with("/Applications/"));
        found.into_iter().find(|p| p.join("Contents/Info.plist").is_file())
    };
    let t = Duration::from_secs(10);
    app_lines(t, false).ok().as_deref().and_then(pick).or_else(|| app_lines(t, true).ok().as_deref().and_then(pick))
}

/// 利用者が app を入れる置き場（`app_lines` が深さ 2 まで直に読む）。
pub fn app_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities"].iter().map(PathBuf::from).collect();
    roots.extend(std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications")));
    roots
}

/// G98: この Mac の app の一覧を、`mdfind -attr kMDItemDisplayName -attr kMDItemCFBundleIdentifier` と同じ形の行
/// （`<path>   kMDItemDisplayName = <名前>   kMDItemCFBundleIdentifier = <id>`）で返す。源は 3 つの和集合:
/// (1) Spotlight（`kMDItemContentType == "com.apple.application-bundle"`）(2) `app_roots` を深さ 2 まで直に（`Contents/Info.plist`）
/// (3) 今動いている app（`lsappinfo list` の Foreground）。Spotlight の index が止まっている・切れている Mac でも (2)(3) で在る app を返す。
/// 1 回の起動で 1 回読んで持つ（`fresh` で読み直す）。Err は 3 つ全部が読めない時だけ。
/// 保証しない: 重ねない・helper の .app を落とす（読む側の `project::parse_apps` と `find_app` の仕事）。
pub fn app_lines(timeout: Duration, fresh: bool) -> Result<String, String> {
    static HELD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    if !fresh {
        if let Some(held) = HELD.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(held);
        }
    }
    // Spotlight は裏で待つ（数秒かかる Mac がある）・その間に置き場と動いている app を読む
    let spotlight = std::thread::spawn(move || {
        let query = "kMDItemContentType == \"com.apple.application-bundle\"";
        match run_deadline(Command::new("/usr/bin/mdfind").args(["-attr", "kMDItemDisplayName", "-attr", "kMDItemCFBundleIdentifier", query]), timeout) {
            Ok((s, out, _)) if s.success() => Ok(out),
            Ok((s, _, err)) => Err(format!("Spotlight didn't answer ({s}): {}", first_line(&err))),
            Err(e) => Err(format!("Spotlight didn't answer ({e})")),
        }
    });
    let running = run_deadline(Command::new("/usr/bin/lsappinfo").arg("list"), Duration::from_secs(5)).map(|(_, out, _)| out);
    let spotlight = spotlight.join().unwrap_or_else(|_| Err("Spotlight didn't answer".into()));
    let lines = read_app_lines(spotlight, &app_roots(), running)?;
    *HELD.lock().unwrap_or_else(|e| e.into_inner()) = Some(lines.clone());
    Ok(lines)
}

/// `app_lines` の中身（源を渡す版・test 用）: Spotlight の行 → 置き場の直の走査 → 動いている app の順に 1 つの文字列へ。
pub fn read_app_lines(spotlight: Result<String, String>, roots: &[PathBuf], running: Result<String, String>) -> Result<String, String> {
    let folders = folder_app_lines(roots);
    let running_lines = running.as_deref().map(running_app_lines).unwrap_or_default();
    if spotlight.is_err() && folders.is_empty() && running_lines.is_empty() {
        return Err(format!("Couldn't read the list of apps on this Mac ({})", spotlight.err().unwrap_or_default()));
    }
    Ok([spotlight.unwrap_or_default(), folders, running_lines].iter().filter(|s| !s.is_empty()).map(|s| s.trim_end()).collect::<Vec<_>>().join("\n"))
}

/// `app_lines` の行を (path, 名前, bundle id) に読む（形の合わない行は飛ばす）。
pub fn app_line_fields(lines: &str) -> impl Iterator<Item = (&str, &str, &str)> {
    lines.lines().filter_map(|line| {
        let (path, rest) = line.split_once("   kMDItemDisplayName = ")?;
        let (name, id) = rest.split_once("   kMDItemCFBundleIdentifier = ")?;
        Some((path, name.trim(), id.trim()))
    })
}

fn app_line(path: &Path, name: &str, id: &str) -> String {
    format!("{}   kMDItemDisplayName = {name}   kMDItemCFBundleIdentifier = {id}", path.display())
}

/// 置き場の `*.app` と、その下の folder（`.app` でない）の `*.app` を読む（`.app` の中には入らない = helper を拾わない）。
fn folder_app_lines(roots: &[PathBuf]) -> String {
    let mut bundles = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "app") {
                bundles.push(p);
            } else if p.is_dir() {
                let Ok(inner) = std::fs::read_dir(&p) else { continue };
                bundles.extend(inner.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "app")));
            }
        }
    }
    bundles
        .iter()
        .filter_map(|app| {
            let plist = info_plist_xml(&app.join("Contents/Info.plist"))?;
            let id = plist_string(&plist, "CFBundleIdentifier")?;
            let stem = app.file_stem()?.to_string_lossy().into_owned();
            let name = plist_string(&plist, "CFBundleDisplayName").or_else(|| plist_string(&plist, "CFBundleName")).filter(|n| !n.is_empty()).unwrap_or(stem);
            Some(app_line(app, &name, &id))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Info.plist を XML の文字列で（binary の plist は `plutil -convert xml1` で直す・多くは XML のまま読める）。
fn info_plist_xml(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if !bytes.starts_with(b"bplist") {
        return String::from_utf8(bytes).ok();
    }
    let o = Command::new("/usr/bin/plutil").args(["-convert", "xml1", "-o", "-"]).arg(path).stderr(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// TITLEBAR-ICONS-1101 の 2・SESSIONS-LEFT-1101 追記 224: Mac の app の icon の PNG（bundle id → `app_lines` の場所 → Info.plist の `CFBundleIconFile` の
/// .icns の中の PNG の 1 つ）。無い app・icon の無い app・PNG を持たない古い .icns は None（呼ぶ側は Phosphor の logo か頭文字の丸）。
/// UI の thread で呼ばない（初めは Spotlight を待つ・`theme::app_icon` が裏の thread で呼ぶ）。qlmanage は使わない（この Mac で 120 秒返らなかった）。
/// 例: `let png = app_icon_png("com.apple.calculator");`
/// PROVIDER-ICONS-1101 追記 252: 読んだ icon は data folder に bundle id と版で溜め（`icon_cache_file`）、次からは .icns を読まない（版が替われば読み直す）。
pub fn app_icon_png(bundle_id: &str) -> Option<Vec<u8>> {
    let lines = app_lines(Duration::from_secs(5), false).ok()?;
    let app = PathBuf::from(app_line_fields(&lines).find(|(_, _, id)| *id == bundle_id)?.0);
    let plist = info_plist_xml(&app.join("Contents/Info.plist"))?;
    let version = plist_string(&plist, "CFBundleShortVersionString").or_else(|| plist_string(&plist, "CFBundleVersion")).unwrap_or_default();
    let cache = icon_cache_file(&portal::data_dir(), bundle_id, &version);
    if let Some(png) = cache.as_deref().and_then(|f| std::fs::read(f).ok()).filter(|b| !b.is_empty()) {
        return Some(png);
    }
    let file = plist_string(&plist, "CFBundleIconFile")?;
    let file = if file.ends_with(".icns") { file } else { format!("{file}.icns") };
    let png = icns_png(&std::fs::read(app.join("Contents/Resources").join(file)).ok()?)?;
    if let Some(f) = &cache {
        keep_icon(f, &png);
    }
    Some(png)
}

/// 溜める icon を書く（tmp に書いて rename = 別の process〈kangae-surface 等〉が書きかけを読まない）。書けなくても icon は出す（溜まらないだけ）。
fn keep_icon(file: &Path, png: &[u8]) {
    let tmp = file.with_extension(format!("{}.part", std::process::id()));
    let ok = file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&tmp, png).is_ok() && std::fs::rename(&tmp, file).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// PROVIDER-ICONS-1101 追記 252（owner「未知のsoftwareのiconは自動でそのiconをappのiconとして使うように。そしてそれは…保存して使いまわせるように」）:
/// 取った icon を溜める file = `<dir>/icons/<key>@<version>.png`（app = bundle id と版〈`CFBundleShortVersionString`・無ければ `CFBundleVersion`〉・web = host と `site`）。
/// 版が替われば別の file = 取り直す。key と version は英数字と `.` `-` `_` だけ残し他は `_`（path の区切りを作れない）・空か `.` だけの key は None。純関数。
/// 溜めるのは各 Mac の data folder の中だけ（共有の server に送らない・配る .app に入れない = operator 11-02 05:0x）。
/// 例: `icon_cache_file(&crate::surface::data_dir(), "com.example.app", "2.1 (45)")` → `Some(<dir>/icons/com.example.app@2.1__45_.png)`
/// Wave: PROVIDER-ICONS-1101
pub fn icon_cache_file(dir: &Path, key: &str, version: &str) -> Option<PathBuf> {
    let safe = |s: &str| s.trim().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect::<String>();
    let key = safe(key);
    (!key.trim_matches('.').is_empty()).then(|| dir.join("icons").join(format!("{key}@{}.png", safe(version))))
}

/// PROVIDER-ICONS-1101 追記 252: web の service の icon の PNG（Page の web の部品・左の行）。溜めた file（`icon_cache_file(data_dir, host, "site")`）が在ればそれだけ・
/// 無ければ 1 回だけ取る（page の `<link rel="apple-touch-icon">` → `<link rel="icon">` → `/apple-touch-icon.png` → `/favicon.ico`・PNG / ICO / JPEG）→ 溜める。
/// 取れなかった site も空の file を溜める（毎回網に出ない）。http(s) でない URL・host の無い URL は None。UI の thread で呼ばない（`theme::site_icon` が裏の thread で呼ぶ）。
/// 例: `let png = site_icon_png("https://excalidraw.com/");`
/// Wave: PROVIDER-ICONS-1101
pub fn site_icon_png(url: &str) -> Option<Vec<u8>> {
    const PAGE_MAX: u64 = 512 * 1024;
    const ICON_MAX: u64 = 1024 * 1024;
    // 試験は網に出ない（左の行を描く試験が Page の web の部品の icon を取りに行かない）
    if cfg!(test) {
        return None;
    }
    let (scheme, rest) = url.split_once("://").filter(|(s, _)| matches!(*s, "http" | "https"))?;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    if host.is_empty() {
        return None;
    }
    let origin = format!("{scheme}://{host}");
    let cache = icon_cache_file(&portal::data_dir(), host, "site")?;
    if let Ok(b) = std::fs::read(&cache) {
        return (!b.is_empty()).then_some(b);
    }
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(8)).build();
    let get = |u: &str, max: u64| -> Result<Vec<u8>, bool> {
        // Err(true) = 網に出られない（溜めない = 次に開いた時に取り直す）・Err(false) = site が答えた（無い・大きすぎ）
        let r = agent.get(u).call().map_err(|e| matches!(e, ureq::Error::Transport(_)))?;
        let mut b = Vec::new();
        r.into_reader().take(max + 1).read_to_end(&mut b).map_err(|_| true)?;
        if b.len() as u64 > max {
            return Err(false);
        }
        Ok(b)
    };
    let page = match get(format!("{origin}/").as_str(), PAGE_MAX) {
        Err(true) => return None,
        p => p.ok().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default(),
    };
    let mut tries = icon_links(&page, &origin);
    tries.extend([format!("{origin}/apple-touch-icon.png"), format!("{origin}/favicon.ico")]);
    let png = tries.iter().find_map(|u| get(u.as_str(), ICON_MAX).ok().and_then(|b| icon_to_png(&b)));
    keep_icon(&cache, png.as_deref().unwrap_or_default());
    png
}

/// web の頁の `<link rel="apple-touch-icon">`（大きい・PNG が多い）→ `<link rel="icon">` の href を絶対の URL に（`//` = 同じ scheme・`/` = origin から・相対 = origin の直下）。
/// `data:` と SVG（`type="image/svg+xml"` か `.svg`・描けない）は落とす。純関数。
fn icon_links(html: &str, origin: &str) -> Vec<String> {
    // 改行と tab は空白に（属性の前の区切り・byte の位置は変わらない）
    let html = html.replace(['\n', '\r', '\t'], " ");
    let low = html.to_ascii_lowercase();
    let attr = |tag: &str, low_tag: &str, name: &str| -> Option<String> {
        let at = low_tag.find(&format!(" {name}="))? + name.len() + 2;
        let q = tag[at..].chars().next()?;
        let v = if q == '"' || q == '\'' { tag[at + 1..].split(q).next()? } else { tag[at..].split([' ', '>']).next()? };
        Some(v.trim().to_owned())
    };
    let mut touch = Vec::new();
    let mut icon = Vec::new();
    let mut from = 0;
    while let Some(i) = low[from..].find("<link") {
        let start = from + i;
        let end = low[start..].find('>').map_or(low.len(), |e| start + e);
        let (tag, low_tag) = (&html[start..end], &low[start..end]);
        from = end;
        let (Some(rel), Some(href)) = (attr(tag, low_tag, "rel"), attr(tag, low_tag, "href")) else { continue };
        let rel = rel.to_ascii_lowercase();
        let svg = low_tag.contains("image/svg") || href.to_ascii_lowercase().split(['?', '#']).next().is_some_and(|p| p.ends_with(".svg"));
        if href.is_empty() || href.starts_with("data:") || svg {
            continue;
        }
        let abs = if href.starts_with("http://") || href.starts_with("https://") {
            href
        } else if let Some(rest) = href.strip_prefix("//") {
            format!("{}://{rest}", origin.split("://").next().unwrap_or("https"))
        } else {
            format!("{origin}/{}", href.trim_start_matches('/'))
        };
        if rel.split_whitespace().any(|r| r.starts_with("apple-touch-icon")) {
            touch.push(abs);
        } else if rel.split_whitespace().any(|r| r == "icon") {
            icon.push(abs);
        }
    }
    touch.extend(icon);
    touch
}

/// 取った icon の bytes → 64 px までに縮めた PNG（PNG・JPEG・PNG を入れた ICO〈今の favicon の多く〉だけ・BMP の ICO と SVG は None）。
fn icon_to_png(b: &[u8]) -> Option<Vec<u8>> {
    let raw: &[u8] = if b.starts_with(&[0, 0, 1, 0]) {
        // ICO: 項目の表（16 byte ずつ・大きさ / 位置は le）から PNG の項目で一番大きい物
        let n = usize::from(u16::from_le_bytes([*b.get(4)?, *b.get(5)?]));
        (0..n)
            .filter_map(|i| {
                let e = b.get(6 + 16 * i..22 + 16 * i)?;
                let (len, at) = (u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize, u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize);
                let data = b.get(at..at.checked_add(len)?)?;
                data.starts_with(b"\x89PNG").then_some((if e[0] == 0 { 256 } else { usize::from(e[0]) }, data))
            })
            .max_by_key(|(w, _)| *w)?
            .1
    } else {
        b
    };
    let img = image::load_from_memory(raw).ok()?;
    let img = if img.width() > 64 || img.height() > 64 { img.resize(64, 64, image::imageops::FilterType::Triangle) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// .icns の中の PNG の項目を 1 つ（64 px の物を先に・無ければ近い大きさ）。.icns でない・PNG の項目が無い・切れた file は None。
fn icns_png(icns: &[u8]) -> Option<Vec<u8>> {
    const PREFER: [&[u8]; 10] = [b"ic12", b"icp6", b"ic07", b"ic13", b"ic08", b"ic14", b"ic09", b"ic10", b"ic11", b"icp5"];
    if !icns.starts_with(b"icns") {
        return None;
    }
    let (mut at, mut best): (usize, Option<(usize, &[u8])>) = (8, None);
    while let Some(head) = icns.get(at..at + 8) {
        let len = (u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize).max(8);
        let Some(data) = icns.get(at + 8..at + len) else { break };
        if let Some(rank) = PREFER.iter().position(|t| *t == &head[..4]).filter(|_| data.starts_with(b"\x89PNG")) {
            if best.map_or(true, |(r, _)| rank < r) {
                best = Some((rank, data));
            }
        }
        at += len;
    }
    best.map(|(_, d)| d.to_vec())
}

/// XML の plist の 1 段目の `<key>key</key><string>…</string>`（&amp; などの 5 つを戻す）。
fn plist_string(xml: &str, key: &str) -> Option<String> {
    let after = &xml[xml.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let v = after.trim_start().strip_prefix("<string>")?;
    let v = &v[..v.find("</string>")?];
    Some(v.trim().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}

/// `lsappinfo list` の Foreground の app（Dock に出る app）だけを行に。bundle id か `.app` の path が無い物は落とす。
fn running_app_lines(text: &str) -> String {
    let mut out = Vec::new();
    let (mut name, mut id, mut path) = (None, None, None);
    let quoted = |l: &str, key: &str| l.trim().strip_prefix(key).and_then(|v| v.strip_prefix('"')).and_then(|v| v.split('"').next()).map(str::to_string);
    for line in text.lines() {
        if let Some(n) = line.trim_start().split_once(") \"").filter(|(i, _)| i.trim().parse::<u32>().is_ok()).and_then(|(_, r)| r.split('"').next()) {
            (name, id, path) = (Some(n.to_string()), None, None);
        } else if let Some(v) = quoted(line, "bundleID=") {
            id = Some(v);
        } else if let Some(v) = quoted(line, "bundle path=") {
            path = Some(v);
        } else if line.contains("type=\"Foreground\"") {
            if let (Some(n), Some(i), Some(p)) = (&name, &id, &path) {
                if p.ends_with(".app") {
                    out.push(app_line(Path::new(p), n, i));
                }
            }
        }
    }
    out.join("\n")
}

/// .app の版（Info.plist の CFBundleShortVersionString。plutil は binary の plist も読む）。
pub fn version(app_path: &Path) -> Option<String> {
    let o = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app_path.join("Contents/Info.plist"))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !v.is_empty()).then_some(v)
}

/// pid の窓の一覧を 1 回読む（bridge だけ・無ければ未確認）。続けて読むなら `WindowReader` を持ち回る。
pub fn windows(pid: u32) -> WindowsRead {
    WindowReader::new().windows(pid)
}

/// 窓を読む口（v0.14 §0-8・図 §18 の「窓の読み」）。bridge は最初に要る時に 1 回だけ起こして持ち回る（handle はこの中だけ有効）。
pub struct WindowReader {
    bridge_path: Option<Result<PathBuf, String>>,
    bridge: Option<Result<portal::Bridge, String>>,
}

impl Default for WindowReader {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowReader {
    /// 環境から: bridge は `KANGAE_SURFACE_BRIDGE`（空なら使わない）→ 実行 file の隣…（`portal::find_bridge`）
    pub fn new() -> WindowReader {
        let bridge_path = match std::env::var_os("KANGAE_SURFACE_BRIDGE") {
            Some(p) if p.is_empty() => Err("KANGAE_SURFACE_BRIDGE が空なので bridge を使いません".to_string()),
            _ => portal::find_bridge().map_err(|e| e.to_string()),
        };
        WindowReader { bridge_path: Some(bridge_path), bridge: None }
    }

    /// test 用の作り方（YC W18-1・env を触らない）: 読む口は bridge だけ。None は「bridge を使わない」。
    /// 使い方: `WindowReader::with_bridge(Some(fake_bridge)).windows(4242)` / `WindowReader::with_bridge(None).windows(4242)`。
    /// 保証: bridge の他に読む口を探さない・起こさない。bridge が無い・読めない時は
    /// `WindowsRead::Unchecked { why, no_reader }`（why は "bridge: " で始まる・no_reader は常に true = 他に読む口が無い）。
    /// 保証しない: bridge が在る時の読みが今の画面と同じ事（呼んだ時点の読み）。
    /// Wave: W18-1
    pub fn with_bridge(bridge: Option<PathBuf>) -> WindowReader {
        WindowReader { bridge_path: Some(bridge.ok_or_else(|| "bridge を使いません".to_string())), bridge: None }
    }

    fn bridge(&mut self) -> Result<&mut portal::Bridge, String> {
        if self.bridge.is_none() {
            let started = match self.bridge_path.take().unwrap_or_else(|| Err("bridge を使いません".into())) {
                Ok(p) => portal::Bridge::spawn(&p, portal::BRIDGE_READY_LIMIT).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            self.bridge = Some(started);
        }
        match self.bridge.as_mut() {
            Some(Ok(b)) if b.alive() => Ok(b),
            Some(Ok(_)) => Err("bridge が止まりました".into()),
            Some(Err(e)) => Err(e.clone()),
            None => Err("bridge を使いません".into()),
        }
    }

    /// pid の窓の一覧。bridge の window_list → 無い・読めなければ未確認（理由・他に読む口は無い = no_reader）
    pub fn windows(&mut self, pid: u32) -> WindowsRead {
        let why = match self.bridge() {
            Ok(b) => {
                let limit = b.window_list_limit();
                match b.call("window_list", json!({ "pid": pid }), limit) {
                    Ok(v) => {
                        let (list, note) = from_window_list(&v);
                        return WindowsRead::Read { list, by: Reader::Bridge, note };
                    }
                    Err(e) => e.to_string(),
                }
            }
            Err(e) => e,
        };
        WindowsRead::Unchecked { why: format!("bridge: {why}"), no_reader: true }
    }

    /// 窓の AX の値（value / title / description）に needle を含む要素が在るか（browser の address bar）。読むだけ。
    /// bridge の ax_find（窓の handle が要る・owner 256 から数で切らず窓の全部を辿り、最初の当たりで止める）→ 無い・読めなければ Err（未確認の理由・"bridge: " で始まる）
    pub fn value_contains(&mut self, _pid: u32, w: &Window, needle: &str) -> Result<bool, String> {
        match (w.handle.clone(), self.bridge()) {
            (Some(h), Ok(b)) => {
                let limit = b.limit();
                b.call("ax_find", json!({ "handle": h, "query": needle }), limit)
                    .map(|v| v["hits"].as_array().is_some_and(|a| !a.is_empty()))
                    .map_err(|e| format!("bridge: {e}"))
            }
            (None, Ok(_)) => Err("bridge: その窓の AX の handle がありません（AX で読めていない窓）".to_string()),
            (_, Err(e)) => Err(format!("bridge: {e}")),
        }
    }
}

/// bridge の window_list の result → 窓の一覧と但し書き（純関数）。AX の窓を先に、AX に無い CG の窓（50pt 以上・layer 0）を後ろに足す。
/// CG の order は前から後ろの順なので z = -order（手前ほど大きい）。AX が読めない時は CG だけで答え、note に理由。
/// 今の Space（on_current_space）は CG の on screen の時だけ Some(true)・CG と対応の付かない AX の窓と off screen は None（分からない）。
/// note は ax_error か、ax_reads の答えない属性・CG に窓が在るのに 3 属性とも空の時だけ（全部答えた・前の bridge は None）。
pub fn from_window_list(v: &Value) -> (Vec<Window>, Option<String>) {
    let cg: Vec<&Value> = v["cg_windows"].as_array().map(|a| a.iter().filter(|w| w["layer"].as_i64() == Some(0)).collect()).unwrap_or_default();
    let cg_of = |id: i64| cg.iter().find(|w| w["id"].as_i64() == Some(id));
    let z_of = |w: &Value| w["order"].as_i64().map(|o| -o);
    let mut list: Vec<Window> = Vec::new();
    for a in v["windows"].as_array().into_iter().flatten() {
        let id = a["cg_id"].as_i64().unwrap_or(0);
        let matched = cg_of(id).filter(|_| id != 0);
        let minimized = a["minimized"].as_bool().unwrap_or(false);
        // CG と対応が付けば CG の on_screen。付かなければ on_screen は最小化でない事からの推定で、Space は分からない（None）:
        // AX の MainWindow / FocusedWindow は別の Space の窓も返し得る（W12W-1c）
        let cg_on = matched.and_then(|c| c["on_screen"].as_bool());
        list.push(Window {
            id,
            title: a["title"].as_str().unwrap_or("").to_string(),
            on_screen: cg_on.unwrap_or(!minimized),
            // CG の on screen = 今の Space。off screen は別の Space か最小化かを公開 API で分けられないので None
            on_current_space: (cg_on == Some(true)).then_some(true),
            z: matched.and_then(|c| z_of(c)),
            handle: a["handle"].as_str().map(String::from),
        });
    }
    let big = |w: &Value| w["frame"]["w"].as_f64().unwrap_or(0.0) >= 50.0 && w["frame"]["h"].as_f64().unwrap_or(0.0) >= 50.0;
    for c in cg.iter().filter(|c| big(c)) {
        let id = c["id"].as_i64().unwrap_or(0);
        if list.iter().any(|w| w.id == id && id != 0) {
            continue;
        }
        let here = c["on_screen"].as_bool().unwrap_or(false);
        list.push(Window { id, title: c["title"].as_str().unwrap_or("").to_string(), on_screen: here, on_current_space: here.then_some(true), z: z_of(c), handle: None });
    }
    let reads = ax_reads(v);
    let detail = ax_reads_line(&reads);
    let note = match v.get("ax_error").filter(|e| !e.is_null()) {
        Some(e) => Some(format!(
            "Couldn't read the windows through accessibility ({}{}), so the answer comes only from the window list (titles are empty without screen recording permission)",
            e["message"].as_str().unwrap_or("Reason unknown"),
            if detail.is_empty() { String::new() } else { format!(" · {detail}") }
        )),
        // 1 つでも答えない・許可が無い属性があれば言う。3 つとも空は、CG に窓が在る時だけ言う（窓が無い app の空は正常）
        None if reads.iter().any(|r| matches!(r.answer, AxAnswer::Failed(_) | AxAnswer::NotTrusted))
            || (!reads.is_empty() && reads.iter().all(|r| r.answer == AxAnswer::Empty) && list.iter().any(|w| w.handle.is_none())) =>
        {
            Some(format!("Part of the window reading was missing ({detail}). Windows that accessibility doesn't have were answered from the window list"))
        }
        None => None,
    };
    (list, note)
}

/// 起動していなければ起動し、起動中なら前に出す（`open -b <bundle id>`。-g を付けない = 前に出す）。終わりは待たない
/// （load 700 の機械では `open` 自体が 30 秒以上返らなかった。窓が出たかは窓の一覧で見る）。
pub fn open(app: &App) -> Result<std::process::Child, String> {
    Command::new("/usr/bin/open")
        .args(["-b", app.bundle_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Can't start open: {e}"))
}

/// `open` が失敗で終わっていれば、その理由（stderr の 1 行）。まだ動いている・成功なら None
fn open_failed(opener: &mut std::process::Child) -> Option<String> {
    match opener.try_wait() {
        Ok(Some(status)) if !status.success() => {
            let mut err = String::new();
            if let Some(mut e) = opener.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            Some(format!("open failed ({status}): {}", first_line(&err)))
        }
        _ => None,
    }
}

/// 状態を決める（純関数）: 入っているか / 起動中か / 窓の一覧（Unchecked = 窓を確かめられなかった・理由つき）。
/// 戻り値は (状態, 人の言葉の理由, 根拠の窓)。
pub fn classify(app: &App, installed: bool, pid: Option<u32>, windows: &WindowsRead) -> (State, String, Option<Window>) {
    let n = app.name;
    if !installed {
        return (
            State::Unsupported,
            format!("{n} isn't on this Mac (looked in Spotlight, /Applications and ~/Applications). Get it from: {}", app.download),
            None,
        );
    }
    if pid.is_none() {
        return (State::AvailableManual, format!("{n} is installed but not running. Open it with Open, or from the Dock or Finder"), None);
    }
    let (list, by) = match windows {
        WindowsRead::Read { list, by, .. } => (list, *by),
        WindowsRead::Unchecked { why, .. } => {
            return (State::AvailableManual, format!("{n} is running. Its window hasn't been checked ({why}). You can still work with it by hand without the window check"), None)
        }
    };
    // 一番手前（z が大きい）。on_screen_only なら今の画面（on screen かつ今の Space）の窓だけから
    let front = |on_screen_only: bool| {
        list.iter().filter(|w| !on_screen_only || w.here()).max_by_key(|w| w.z.unwrap_or(i64::MIN)).cloned()
    };
    if let Some(w) = front(true) {
        let title = if w.title.trim().is_empty() { "(untitled)".to_string() } else { w.title.clone() };
        return (State::Ready, format!("{n}'s window “{title}” is on the current screen (checked with {})", by.label()), Some(w));
    }
    if list.is_empty() {
        return (State::Degraded, format!("{n} is running but has no window (it may still be starting, or not responding). Wait a moment and check again, or reopen {n}"), None);
    }
    (State::AvailableManual, format!("{n} is running, but its window isn't on the current screen (another Space, or minimized). Bring it to the front with Open"), front(false))
}

/// 探して状態を返す。open_it なら起動 / 前に出してから、窓が今の画面に出るまで 1 秒ごとに最大 timeout 待つ。窓の読みは `WindowReader`（bridge が先）。
pub fn probe(app: &App, open_it: bool, timeout: Duration) -> Surface {
    probe_with(app, open_it, timeout, &mut WindowReader::new())
}

/// `probe` の窓を読む口を決めて渡す版（test 用）。
pub fn probe_with(app: &App, open_it: bool, timeout: Duration, reader: &mut WindowReader) -> Surface {
    let found = find_app(app);
    let mut pid = pid_of(app.process);
    let app_path = found.or_else(|| pid.and_then(exe_of).and_then(|e| bundle_of(&e)));
    let installed = app_path.is_some() || pid.is_some();
    let version = app_path.as_deref().and_then(version);
    let not_read = || WindowsRead::Unchecked { why: "Not read because it isn't running".into(), no_reader: false };
    let done = |state: State, reason: String, pid: Option<u32>, window: Option<Window>, w: &WindowsRead| Surface {
        state,
        reason,
        app_path: app_path.clone(),
        version: version.clone(),
        pid,
        window,
        reader: match w {
            WindowsRead::Read { by, note, .. } => Some((*by, note.clone())),
            WindowsRead::Unchecked { .. } => None,
        },
        checked_at: SystemTime::now(),
    };
    if open_it && installed {
        let mut opener = match open(app) {
            Ok(c) => c,
            Err(e) => return done(State::Degraded, format!("Couldn't open {} ({e}). Open it from the Dock or Finder", app.name), pid, None, &not_read()),
        };
        let started = Instant::now();
        let result = loop {
            if let Some(why) = open_failed(&mut opener) {
                break done(State::Degraded, format!("Couldn't open {} ({why}). Open it from the Dock or Finder", app.name), pid, None, &not_read());
            }
            pid = pid_of(app.process);
            let w = match pid {
                Some(p) => reader.windows(p),
                None => not_read(),
            };
            let (state, reason, window) = classify(app, installed, pid, &w);
            let out_of_time = started.elapsed() > timeout;
            // 窓を確かめる口が無ければ待っても変わらない
            if state == State::Ready || (matches!(w, WindowsRead::Unchecked { no_reader: true, .. }) && pid.is_some()) || out_of_time {
                break match (out_of_time && state != State::Ready, pid) {
                    (true, None) => done(
                        State::Degraded,
                        format!("Opened {}, but it hadn't started after {} s. Check the screen of {}", app.name, timeout.as_secs(), app.name),
                        None,
                        None,
                        &w,
                    ),
                    (true, Some(_)) => done(state, format!("{reason} (waited {} s)", timeout.as_secs()), pid, window, &w),
                    _ => done(state, reason, pid, window, &w),
                };
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        // まだ返っていない open は畳む（LaunchServices への依頼は届いている）
        if matches!(opener.try_wait(), Ok(None)) {
            let _ = opener.kill();
        }
        let _ = opener.wait();
        return result;
    }
    let w = match pid {
        Some(p) if installed => reader.windows(p),
        _ => not_read(),
    };
    let (state, reason, window) = classify(app, installed, pid, &w);
    done(state, reason, pid, window, &w)
}

#[cfg(test)]
mod icon_cache_tests {
    use super::*;

    /// PROVIDER-ICONS-1101 追記 252: 溜める file は data folder の icons の下だけ（区切りを作れない・空と `..` は None）・版が替われば別の file。
    /// 負の対照（未実行・owner 186 / 187）: `safe` を外す → `../../etc/passwd` が icons の外を指して 3 つ目が赤。
    #[test]
    fn provider_icons_252_icon_cache_file_stays_in_the_icons_dir() {
        let d = Path::new("/d");
        assert_eq!(icon_cache_file(d, "com.example.app", "2.1 (45)"), Some(PathBuf::from("/d/icons/com.example.app@2.1__45_.png")));
        assert_ne!(icon_cache_file(d, "com.example.app", "2.2"), icon_cache_file(d, "com.example.app", "2.1"), "版が替わっても同じ file");
        assert_eq!(icon_cache_file(d, "../../etc/passwd", "1"), Some(PathBuf::from("/d/icons/.._.._etc_passwd@1.png")), "区切りを作った");
        assert_eq!(icon_cache_file(d, "excalidraw.com", "site"), Some(PathBuf::from("/d/icons/excalidraw.com@site.png")));
        assert_eq!((icon_cache_file(d, "..", "1"), icon_cache_file(d, " ", "1")), (None, None));
    }

    /// PROVIDER-ICONS-1101 追記 252: 頁の link から icon の URL（apple-touch-icon が先・相対 / `//` を絶対に・SVG と data: は落とす）。ICO は中の PNG の一番大きい物。
    /// 負の対照（未実行・owner 186 / 187）: `touch.extend(icon)` を `icon.extend(touch)` の順に → 1 つ目が favicon で赤・ICO の `max_by_key` を `min_by_key` に → 16 px で赤。
    #[test]
    fn provider_icons_252_site_icon_links_and_ico() {
        let html = r#"<head><LINK rel="icon" href="/favicon-32.png"><link rel='icon' type='image/svg+xml' href='/i.svg'>
            <link href="//cdn.example.com/t.png" rel="apple-touch-icon" sizes="180x180"><link rel="icon" href="data:,"><link rel=stylesheet href=/a.css></head>"#;
        assert_eq!(icon_links(html, "https://example.com"), ["https://cdn.example.com/t.png", "https://example.com/favicon-32.png"]);
        let png = |w: u32| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(w, w).write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        let (small, big) = (png(16), png(48));
        let mut ico = vec![0, 0, 1, 0, 2, 0];
        let at = 6 + 32;
        for (w, data, off) in [(16u8, &small, at), (48, &big, at + small.len())] {
            ico.extend([w, w, 0, 0, 1, 0, 32, 0]);
            ico.extend((data.len() as u32).to_le_bytes());
            ico.extend((off as u32).to_le_bytes());
        }
        ico.extend(&small);
        ico.extend(&big);
        let got = image::load_from_memory(&icon_to_png(&ico).expect("PNG の ICO を読めない")).unwrap();
        assert_eq!(got.width(), 48, "ICO の一番大きい PNG でない");
        assert_eq!(image::load_from_memory(&icon_to_png(&png(128)).unwrap()).unwrap().width(), 64, "64 px に縮めない");
        assert!(icon_to_png(b"<svg/>").is_none());
    }
}
