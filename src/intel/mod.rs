//! Software Intelligence の読むだけの調査（v0.13 W4-5・docs/SOFTWARE_INTELLIGENCE_AND_LEARNING.md §1〜§3・DESIGN §14 / §16.3-5）。
//! 1 つの app について「入っているか・試せる事・未確認の事・要る権限」を、どこを読んだか・いつ読んだかつきの data（`Intel`）で返す。
//! 読む物: Spotlight（mdfind）・Info.plist（plutil）・同梱の辞書（.sdef / 旧式の .scriptSuite）・起動中なら窓の一覧（macOS bridge だけ・
//! audio/surface.rs の `WindowReader`）・
//! service ごとの能力の一覧は製品に入れない（owner 10-23「serviceごとに能力をこの製品に入れるのではない」・要る時に Engine が調べる）。app を起動しない・command を打たない・Apple Events を送らない。
//! `/usr/bin/sdef` は Xcode が要る shim（Command Line Tools だけの Mac では exit 1）なので呼ばず、sdef が読む file を直接読む。
//! self-contained（src/bin/kangae-intel.rs と tests/intel.rs が `#[path]` で取り込む）。分岐は docs/diagrams.md §15。

pub mod analysis; // W19-3: 交換できる Analysis Backend（REA は 1 つ目・必須の依存にしない・serde と std だけ）
#[allow(dead_code)]
#[path = "../audio/surface.rs"]
mod surface;
/// ENGINE-AUDIT W4 → W6: `adapters::mac_gui` の口の棚（`shortcuts list`）が同じ上限つきの command の走らせ方を使う（adapters は `crate::` を書けないので intel 経由・
/// tests/adapters.rs は intel を `#[path]` で取り込む）
pub(crate) use surface::run_deadline;
#[allow(dead_code)]
#[path = "../storage/redact.rs"]
mod redact; // W19-3: 解析の道具の答えの秘密を伏せる（storage::log の redact と同じ file）

use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 評価の中で動かしてよいか（DESIGN §16.3-4「評価で動かすのは local と read の action だけ」）。
/// `domain::action` の `is_external`（外に出るか）とは範囲が違い、こちらは外を変えるかで分ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateClass {
    /// local と read（local_read / local_write / external_read）
    Read,
    /// 外に効く（external_write / external_send / destructive）と、作用が分からない物
    Outward,
}

/// effect（`domain::action` の 6 語）→ GateClass。分類はここ 1 か所だけ。
pub fn gate(effect: Option<&str>) -> GateClass {
    match effect {
        Some("local_read" | "local_write" | "external_read") => GateClass::Read,
        _ => GateClass::Outward,
    }
}

/// どこを読んだか・いつ読んだか。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub read: String,
    pub checked_at: String,
}

/// 1 つの観測。absent = 確かめて無かった・unchecked = 確かめられなかった（未確認）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Probe<T> {
    Found { value: T, source: Source },
    Absent { why: String, source: Source },
    Unchecked { why: String },
}

impl<T> Probe<T> {
    pub fn found(&self) -> Option<&T> {
        match self {
            Probe::Found { value, .. } => Some(value),
            _ => None,
        }
    }
    fn unchecked(&self) -> Option<&str> {
        match self {
            Probe::Unchecked { why } => Some(why),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct App {
    pub path: String,
    pub bundle_id: Option<String>,
    pub name: String,
    pub version: Option<String>,
    /// 起動中なら pid（lsappinfo で読む。起動はしない）
    pub pid: Option<u32>,
    /// Info.plist の NS*UsageDescription（app 自身が利用者に聞く権限）
    pub asks: Vec<String>,
    /// 選ばなかった同じ名前・同じ bundle id の copy
    pub other_copies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Dictionary {
    pub suites: Vec<Suite>,
    /// class の property の数（1 以上なら AppleScript の get で読める物が在る）
    pub properties: usize,
    /// hidden="yes" で外した suite と command の数
    pub hidden: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Suite {
    pub name: String,
    pub description: String,
    pub commands: Vec<ScriptCommand>,
    pub classes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptCommand {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AxWindow {
    pub id: i64,
    pub title: String,
    /// 今の画面（on screen かつ今の Space）に在る
    pub on_screen: bool,
}

/// 試せる事 1 つ（AX の窓・辞書の command）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Capability {
    pub id: String,
    pub summary: String,
    /// effect の 6 語（`domain::action`）。辞書は作用を書かないので、辞書の command は count / exists / get 以外 None
    pub effect: Option<String>,
    pub gate: GateClass,
    /// route の語（画面では build::route_word が利用者の語にする）。窓の読みは native_docking
    pub route: String,
    /// 同じ route に口が 2 つある時の区別
    pub endpoint: Option<String>,
    /// available / needs_check / excluded
    pub status: String,
    pub requires: Vec<String>,
    pub reason: Option<String>,
    /// runtime_tested / documented / inferred / stale
    pub evidence: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unknown {
    pub what: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Permission {
    /// tcc:automation:<bundle id> / tcc:accessibility / tcc:microphone / oauth_scope:<scope> …
    pub id: String,
    pub label: String,
    /// 誰に許すか（呼ぶ側・Kangae・その app 自身・接続する account）
    pub who: String,
    pub settings: String,
    /// 根拠（どの capability / Info.plist の key から）
    pub because: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Intel {
    pub query: String,
    pub checked_at: String,
    pub app: Probe<App>,
    pub dictionary: Probe<Dictionary>,
    pub windows: Probe<Vec<AxWindow>>,
    pub capabilities: Vec<Capability>,
    pub unknowns: Vec<Unknown>,
    pub permissions: Vec<Permission>,
}

pub const NOT_RUNNING: &str = "Not confirmed (it isn't running). Shibubu doesn't start it";
const NO_APP: &str = "Not looked at because the app wasn't found on this Mac";

/// 入力を整える: 前後の空白と末尾の .app を落とす。空・120 字超・制御文字・/ は断る（mdfind の式と path に渡すので）。
pub fn normalize(query: &str) -> Result<String, String> {
    let q = query.trim();
    let q = match q.len().checked_sub(4).and_then(|i| q.get(i..).map(|tail| (i, tail))) {
        Some((i, tail)) if tail.eq_ignore_ascii_case(".app") => q[..i].trim(),
        _ => q,
    };
    if q.is_empty() {
        return Err("Give one app name (for example REAPER)".into());
    }
    if q.chars().count() > 120 {
        return Err("The name is too long (up to 120 characters)".into());
    }
    if q.chars().any(|c| c.is_control() || c == '/') {
        return Err("A name can't contain / or control characters (give only an app name)".into());
    }
    Ok(q.to_string())
}

/// bundle id の形（英数・- ・_ の区切りが 3 つ以上。例 com.apple.Music）。
pub fn is_bundle_id(q: &str) -> bool {
    q.split('.').count() >= 3 && q.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

/// mdfind の式。文字列は "…" に入れ、\ " * を escape する（* は wildcard）。名前は大文字小文字・濁点を区別しない（cd）。
pub fn spotlight_query(bundle: Option<&str>, names: &[String]) -> String {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('*', "\\*");
    let mut terms: Vec<String> = bundle.map(|b| format!("kMDItemCFBundleIdentifier == \"{}\"", quote(b))).into_iter().collect();
    for n in names {
        let n = quote(n);
        terms.push(format!(
            "(kMDItemContentType == \"com.apple.application-bundle\" && (kMDItemFSName == \"{n}.app\"cd || kMDItemDisplayName == \"{n}\"cd || kMDItemDisplayName == \"{n}.app\"cd))"
        ));
    }
    terms.join(" || ")
}

/// 固定の場所（この順に優先）。~/Applications はこの後ろ。
const APP_DIRS: [&str; 4] = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities"];

fn rank(p: &Path, home_apps: Option<&Path>) -> usize {
    let parent = p.parent();
    APP_DIRS
        .iter()
        .position(|d| parent == Some(Path::new(d)))
        .unwrap_or(if parent.is_some() && parent == home_apps { APP_DIRS.len() } else { APP_DIRS.len() + 1 })
}

/// 外の command。KANGAE_INTEL_TOOLS（test 用の dir）が在ればその下の同じ名前を使う。
fn tool(name: &str, default: &str) -> PathBuf {
    std::env::var_os("KANGAE_INTEL_TOOLS").map(|d| PathBuf::from(d).join(name)).unwrap_or_else(|| PathBuf::from(default))
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(300).collect()
}

/// ENGINE-AUDIT W4: CFBundleURLTypes（URL scheme）と CFBundleDocumentTypes（開ける書類の型）を足した（口の棚 `adapters::mac_gui` の `desktop.app.surfaces` が読む）
const PLIST_KEYS: [&str; 12] = [
    "CFBundleIdentifier", "CFBundleShortVersionString", "CFBundleDisplayName", "CFBundleName", "OSAScriptingDefinition",
    "NSAppleScriptEnabled", "NSMicrophoneUsageDescription", "NSCameraUsageDescription", "NSSpeechRecognitionUsageDescription",
    "NSAppleEventsUsageDescription", "CFBundleURLTypes", "CFBundleDocumentTypes",
];

/// Info.plist の key → app 自身が聞く権限。
const USAGE: [(&str, &str); 4] = [
    ("NSMicrophoneUsageDescription", "tcc:microphone"),
    ("NSCameraUsageDescription", "tcc:camera"),
    ("NSSpeechRecognitionUsageDescription", "tcc:speech_recognition"),
    ("NSAppleEventsUsageDescription", "tcc:automation"),
];

/// plist を JSON で読む（plutil は binary の plist も読む）。<date> / <data> を含む plist は JSON に変換できないので、使う key だけ 1 つずつ抜く。
pub fn read_plist(path: &Path, timeout: Duration) -> Option<Value> {
    let plutil = |args: &[&str]| surface::run_deadline(Command::new("/usr/bin/plutil").args(args).arg(path), timeout).ok();
    if let Some((status, out, _)) = plutil(&["-convert", "json", "-o", "-"]) {
        if let Some(v) = status.success().then(|| serde_json::from_str::<Value>(&out).ok()).flatten() {
            return Some(v);
        }
    }
    let mut v = serde_json::Map::new();
    for key in PLIST_KEYS {
        // W4: 配列の key（URL scheme・書類の型）は raw だと要素の数しか出ないので json で抜く
        let array = matches!(key, "CFBundleURLTypes" | "CFBundleDocumentTypes");
        if let Some((status, out, _)) = plutil(&["-extract", key, if array { "json" } else { "raw" }, "-o", "-"]) {
            if status.success() && !out.trim().is_empty() {
                v.insert(key.to_string(), if array { serde_json::from_str(&out).unwrap_or(Value::Null) } else { Value::String(out.trim().to_string()) });
            }
        }
    }
    (!v.is_empty()).then_some(Value::Object(v))
}

fn truthy(v: &Value) -> bool {
    v.as_bool() == Some(true) || matches!(v.as_str(), Some("YES" | "yes" | "true" | "1"))
}

/// `lsappinfo info -only pid <bundle id>` の出力（`"pid"=812`）。起動していなければ空。
pub fn parse_lsappinfo_pid(out: &str) -> Option<u32> {
    let i = out.to_ascii_lowercase().find("\"pid\"=")?;
    out[i + 6..].trim_start().chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok().filter(|p| *p > 0)
}

fn running(bundle: &str, timeout: Duration) -> Result<Option<u32>, String> {
    let (status, out, err) = surface::run_deadline(Command::new(tool("lsappinfo", "/usr/bin/lsappinfo")).args(["info", "-only", "pid", bundle]), timeout)?;
    if !status.success() {
        return Err(format!("lsappinfo が失敗しました（{status}）: {}", first_line(&err)));
    }
    Ok(parse_lsappinfo_pid(&out))
}

/// 在りかを探して 1 本選ぶ。戻り値は (app, その Info.plist, 起動中か)。
fn find(
    bundle: Option<&str>,
    strict: bool,
    names: &[String],
    timeout: Duration,
    now: &str,
    unknowns: &mut Vec<Unknown>,
) -> (Probe<App>, Option<Value>, Result<Option<u32>, String>) {
    let query = spotlight_query(bundle, names);
    let spotlight = match surface::run_deadline(Command::new(tool("mdfind", "/usr/bin/mdfind")).arg(&query), timeout) {
        Ok((status, out, _)) if status.success() => Some(out),
        Ok((status, _, err)) => {
            unknowns.push(Unknown { what: "Spotlight".into(), why: format!("mdfind が失敗しました（{status}）: {}。固定の場所だけを見ました", first_line(&err)) });
            None
        }
        Err(e) => {
            unknowns.push(Unknown { what: "Spotlight".into(), why: format!("mdfind: {e}。固定の場所だけを見ました") });
            None
        }
    };
    let from_spotlight: Vec<PathBuf> =
        spotlight.as_deref().unwrap_or("").lines().map(str::trim).filter(|l| l.ends_with(".app")).map(PathBuf::from).collect();
    let home_apps = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications"));
    let dirs: Vec<PathBuf> = APP_DIRS.iter().map(PathBuf::from).chain(home_apps.clone()).collect();
    let mut paths = from_spotlight.clone();
    for n in names {
        paths.extend(dirs.iter().map(|d| d.join(format!("{n}.app"))));
    }
    let mut seen = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()) && p.join("Contents/Info.plist").is_file());
    let mut cands: Vec<(PathBuf, Value)> = paths.into_iter().filter_map(|p| read_plist(&p.join("Contents/Info.plist"), timeout).map(|v| (p, v))).collect();
    let bid = |v: &Value| v["CFBundleIdentifier"].as_str().map(str::to_string);
    let same_bundle = |v: &Value| bundle.is_some_and(|b| bid(v).is_some_and(|x| x.eq_ignore_ascii_case(b)));
    if strict {
        cands.retain(|(_, v)| same_bundle(v));
    }
    cands.sort_by_key(|(p, v)| (!same_bundle(v), rank(p, home_apps.as_deref())));
    let searched = format!(
        "mdfind {query}{}・/Applications・/Applications/Utilities・/System/Applications・/System/Applications/Utilities・~/Applications の 名前.app",
        if spotlight.is_some() { "" } else { "(no answer)" }
    );
    let Some((path, plist)) = cands.first().cloned() else {
        let app = match (&spotlight, names.is_empty()) {
            (Some(_), _) => Probe::Absent { why: "Not found on this Mac".into(), source: Source { read: searched, checked_at: now.into() } },
            (None, true) => Probe::Unchecked {
                why: "Spotlight didn't answer, so the app's location couldn't be looked up (only the fixed places can be searched, and only by name). Ask again with the app's name".into(),
            },
            (None, false) => Probe::Unchecked { why: "Not in the fixed places. Spotlight isn't answering, so other places weren't looked at".into() },
        };
        return (app, None, Ok(None));
    };
    let bundle_id = bid(&plist);
    let alive = bundle_id.as_deref().map_or_else(|| Err("Shibubu couldn't identify the app, so it didn't check whether it is running".to_string()), |b| running(b, timeout));
    let app = App {
        path: path.display().to_string(),
        name: plist["CFBundleDisplayName"]
            .as_str()
            .or(plist["CFBundleName"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
        version: plist["CFBundleShortVersionString"].as_str().map(str::to_string),
        pid: alive.as_ref().ok().copied().flatten(),
        asks: USAGE.iter().filter(|(key, _)| plist.get(*key).is_some()).map(|(key, _)| key.to_string()).collect(),
        other_copies: cands[1..].iter().map(|(p, v)| format!("{} ({})", p.display(), bid(v).unwrap_or_else(|| "unknown".into()))).collect(),
        bundle_id,
    };
    let how = if from_spotlight.contains(&path) { "With Spotlight" } else { "In the fixed places" };
    let source = Source { read: format!("{}/Contents/Info.plist (found: {how})", app.path), checked_at: now.into() };
    (Probe::Found { value: app, source }, Some(plist), alive)
}

fn read_text(p: &Path) -> Result<String, String> {
    let len = std::fs::metadata(p).map_err(|e| e.to_string())?.len();
    if len > 4 << 20 {
        return Err("It is over 4 MB".into());
    }
    std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string())
}

const SCRIPTING_DEFINITIONS: &str = "/System/Library/ScriptingDefinitions/";
const COCOA_STANDARD: &str = "/System/Library/ScriptingDefinitions/CocoaStandard.sdef";

/// xi:include の href を読む。OS の /System/Library/ScriptingDefinitions の下だけ（app が任意の file を読ませないように）。
pub fn system_include(href: &str) -> Option<String> {
    let path = href.strip_prefix("file://localhost").or_else(|| href.strip_prefix("file://"))?;
    (path.starts_with(SCRIPTING_DEFINITIONS) && !path.contains("..")).then(|| read_text(Path::new(path)).ok()).flatten()
}

/// 辞書を読む: Info.plist の OSAScriptingDefinition が指す .sdef → 旧式の .scriptSuite → 無し。
fn dictionary_probe(app_path: &Path, plist: &Value, timeout: Duration, now: &str) -> Probe<Dictionary> {
    let resources = app_path.join("Contents/Resources");
    let source = |read: String| Source { read, checked_at: now.into() };
    let scripting = truthy(&plist["NSAppleScriptEnabled"]);
    if let Some(name) = plist["OSAScriptingDefinition"].as_str() {
        if name.contains('/') {
            return Probe::Unchecked { why: format!("Info.plist の OSAScriptingDefinition が Resources の外を指しています（{name}）") };
        }
        return match read_text(&resources.join(name)) {
            Ok(xml) => Probe::Found {
                value: parse_sdef(&xml, &system_include),
                source: source(format!("Contents/Resources/{name}（Info.plist の OSAScriptingDefinition）")),
            },
            Err(e) => Probe::Unchecked { why: format!("Info.plist が指す辞書 {name} を読めません（{e}）") },
        };
    }
    let mut legacy: Vec<PathBuf> = std::fs::read_dir(&resources)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "scriptSuite"))
        .collect();
    legacy.sort();
    if !legacy.is_empty() {
        let mut dict = Dictionary::default();
        // NSAppleScriptEnabled の Cocoa app には Standard Suite が暗に付く（その正本が CocoaStandard.sdef）
        if scripting {
            if let Ok(xml) = read_text(Path::new(COCOA_STANDARD)) {
                let standard = parse_sdef(&xml, &|_| None);
                dict.properties += standard.properties;
                dict.suites.extend(standard.suites);
            }
        }
        let mut read = Vec::new();
        for p in &legacy {
            let Some(suite) = read_plist(p, timeout) else { continue };
            let term = read_plist(&p.with_extension("scriptTerminology"), timeout).unwrap_or(Value::Null);
            let (s, properties) = parse_script_suite(&suite, &term);
            dict.suites.push(s);
            dict.properties += properties;
            read.push(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
        let standard = if scripting {
            "＋ CocoaStandard.sdef の Standard Suite（NSAppleScriptEnabled の Cocoa app に暗に付く。Text Suite など他の暗の suite は読んでいない）"
        } else {
            ""
        };
        return Probe::Found {
            value: dict,
            source: source(format!("Contents/Resources/{}・.scriptTerminology（旧式の辞書）{standard}", read.join("・"))),
        };
    }
    if scripting {
        return Probe::Unchecked {
            why: "NSAppleScriptEnabled は true ですが、辞書の file（.sdef / .scriptSuite）が同梱されていません（実行中に作る辞書は読めない）".into(),
        };
    }
    Probe::Absent {
        why: "AppleScript の辞書がありません（Info.plist に OSAScriptingDefinition も NSAppleScriptEnabled も無く、.scriptSuite も無い）".into(),
        source: source("Contents/Info.plist・Contents/Resources".into()),
    }
}

struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    close: bool,
    self_close: bool,
}

/// XML の tag を順に（comment・CDATA・<? ?>・<!DOCTYPE> は飛ばす。属性値の中の > では閉じない）。crate を足さない最小の scan。
fn tags(xml: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        let skip_to = |end: &str| rest.find(end).map(|j| j + end.len());
        let n = if rest.starts_with("<!--") {
            skip_to("-->")
        } else if rest.starts_with("<![CDATA[") {
            skip_to("]]>")
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            skip_to(">")
        } else {
            let mut quote = None;
            let end = rest.char_indices().skip(1).find_map(|(j, c)| {
                match (quote, c) {
                    (None, '"' | '\'') => quote = Some(c),
                    (Some(q), c) if c == q => quote = None,
                    (None, '>') => return Some(j),
                    _ => {}
                }
                None
            });
            end.map(|j| {
                out.push(parse_tag(&rest[1..j]));
                j + 1
            })
        };
        let Some(n) = n else { break };
        rest = &rest[n..];
    }
    out
}

fn parse_tag(inner: &str) -> Tag {
    let close = inner.starts_with('/');
    let self_close = inner.ends_with('/');
    let inner = inner.trim_start_matches('/').trim_end_matches('/');
    let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
    let mut attrs = Vec::new();
    let mut s = &inner[name_end..];
    while let Some(eq) = s.find('=') {
        let key = s[..eq].trim().to_string();
        let after = s[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let Some(end) = after[1..].find(q) else { break };
        attrs.push((key, unescape(&after[1..1 + end])));
        s = &after[end + 2..];
    }
    Tag { name: inner[..name_end].to_string(), attrs, close, self_close }
}

/// XML の実体参照を戻す（&amp; &lt; &gt; &quot; &apos; &#N; &#xH;）。知らない物はそのまま。
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|j| *j <= 10).and_then(|j| {
            let ent = &rest[1..j];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => ent
                    .strip_prefix("#x")
                    .or(ent.strip_prefix("#X"))
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| ent.strip_prefix('#').map(|d| d.parse::<u32>()))
                    .and_then(|r| r.ok())
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, j + 1))
        });
        match decoded {
            Some((c, n)) => {
                out.push(c);
                rest = &rest[n..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// sdef（XML）から suite・command・class を読む。hidden="yes" は外して数える。xi:include は include が返した文字列を 1 段だけ読む。
pub fn parse_sdef(xml: &str, include: &dyn Fn(&str) -> Option<String>) -> Dictionary {
    let mut dict = Dictionary::default();
    let mut current: Option<(Suite, bool)> = None;
    let mut in_class = false;
    for t in tags(xml) {
        let attr = |k: &str| t.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        let hidden = attr("hidden").as_deref() == Some("yes");
        let suite_hidden = current.as_ref().is_some_and(|(_, h)| *h);
        match (t.name.as_str(), t.close) {
            ("suite", false) => {
                if let Some((s, h)) = current.take() {
                    if !h {
                        dict.suites.push(s);
                    }
                }
                dict.hidden += usize::from(hidden);
                let suite = Suite { name: attr("name").unwrap_or_default(), description: attr("description").unwrap_or_default(), ..Default::default() };
                current = Some((suite, hidden));
            }
            ("suite", true) => {
                if let Some((s, h)) = current.take() {
                    if !h {
                        dict.suites.push(s);
                    }
                }
            }
            ("command", false) if !suite_hidden => {
                if hidden {
                    dict.hidden += 1;
                } else if let Some((s, _)) = current.as_mut() {
                    s.commands.push(ScriptCommand { name: attr("name").unwrap_or_default(), description: attr("description").unwrap_or_default() });
                }
            }
            ("class" | "class-extension", false) if !suite_hidden => {
                in_class = !t.self_close;
                let name = if t.name == "class" { attr("name") } else { attr("extends") };
                if let (Some(n), Some((s, _))) = (name, current.as_mut()) {
                    if !s.classes.contains(&n) {
                        s.classes.push(n);
                    }
                }
            }
            ("class" | "class-extension", true) => in_class = false,
            ("property", false) if in_class && !suite_hidden => dict.properties += 1,
            ("xi:include", false) => {
                let Some(sub) = attr("href").and_then(|h| include(&h)).map(|x| parse_sdef(&x, &|_| None)) else { continue };
                dict.properties += sub.properties;
                dict.hidden += sub.hidden;
                match current.as_mut() {
                    // suite の中の include は中身をその suite に足す
                    Some((s, _)) => sub.suites.into_iter().for_each(|x| {
                        s.commands.extend(x.commands);
                        s.classes.extend(x.classes);
                    }),
                    None => dict.suites.extend(sub.suites),
                }
            }
            _ => {}
        }
    }
    if let Some((s, false)) = current {
        dict.suites.push(s);
    }
    dict
}

/// 旧式の辞書（Cocoa の .scriptSuite と .scriptTerminology を JSON にした物）→ (suite, property の数)。
pub fn parse_script_suite(suite: &Value, term: &Value) -> (Suite, usize) {
    let keys = |kind: &str| -> Vec<String> {
        let mut k: Vec<String> = suite[kind].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        k.sort();
        k
    };
    let named = |kind: &str, key: &str| term[kind][key]["Name"].as_str().unwrap_or(key).to_string();
    let commands = keys("Commands")
        .iter()
        .map(|k| ScriptCommand { name: named("Commands", k), description: term["Commands"][k]["Description"].as_str().unwrap_or("").to_string() })
        .collect();
    let classes = keys("Classes").iter().map(|k| named("Classes", k)).collect();
    let properties = suite["Classes"].as_object().map_or(0, |o| o.values().map(|c| c["Attributes"].as_object().map_or(0, |a| a.len())).sum());
    let name = term["Name"].as_str().or(suite["Name"].as_str()).unwrap_or("").to_string();
    (Suite { name, description: term["Description"].as_str().unwrap_or("").to_string(), commands, classes }, properties)
}

fn windows_probe(running: &Result<Option<u32>, String>, now: &str) -> Probe<Vec<AxWindow>> {
    let pid = match running {
        Err(e) => return Probe::Unchecked { why: format!("Couldn't check whether it is running ({e})") },
        Ok(None) => return Probe::Unchecked { why: NOT_RUNNING.into() },
        Ok(Some(pid)) => *pid,
    };
    match surface::windows(pid) {
        surface::WindowsRead::Unchecked { why, .. } => Probe::Unchecked { why: format!("The windows weren't looked at ({why}). You don't need the window check to work by hand") },
        surface::WindowsRead::Read { list, by: surface::Reader::Bridge, note } => {
            // source.read の頭で読んだ口が分かる（読む口は bridge だけ・YC W18-1）
            let read = format!("macOS bridge window_list {{\"pid\":{pid}}}");
            let source = Source { read: note.map_or(read.clone(), |n| format!("{read} ({n})")), checked_at: now.into() };
            if list.is_empty() {
                return Probe::Absent { why: "Running, but it has no window".into(), source };
            }
            Probe::Found {
                value: list.into_iter().map(|w| AxWindow { on_screen: w.here(), id: w.id, title: w.title }).collect(),
                source,
            }
        }
    }
}

/// 調べた物を 1 つにまとめる（純関数）。capabilities = AX → 辞書の command の順。
pub fn assemble(
    query: &str,
    now: &str,
    app: Probe<App>,
    dictionary: Probe<Dictionary>,
    windows: Probe<Vec<AxWindow>>,
    mut unknowns: Vec<Unknown>,
) -> Intel {
    let found = app.found();
    let mut caps = Vec::new();
    // service ごとの能力の一覧は製品に持たない（owner 10-23）: 外の口は、この Mac の app から読めないので未調査と言う
    unknowns.push(Unknown {
        what: "Outside this Mac".into(),
        why: if found.is_some() {
            "Pricing, APIs, web access and terms can't be read from an app on this Mac, so they haven't been looked into (Shibubu looks when it needs to). What can be tried is only what could be read on this Mac"
        } else {
            "It wasn't found on this Mac, so what can be tried is unknown (Shibubu looks when it needs to)"
        }
        .into(),
    });
    if let Probe::Found { value, source } = &windows {
        let titles: Vec<&str> = value.iter().map(|w| if w.title.trim().is_empty() { "(untitled)" } else { w.title.as_str() }).take(3).collect();
        caps.push(Capability {
            id: "ax.windows.read".into(),
            summary: format!(
                "macOS bridge（CGWindowList + AX）で窓 {} 個（{}）。窓の中の AX の木は読んでいない",
                value.len(),
                titles.join("・")
            ),
            effect: Some("external_read".into()),
            gate: GateClass::Read,
            route: "native_docking".into(),
            endpoint: None,
            status: "available".into(),
            requires: vec!["tcc:accessibility".into()],
            reason: None,
            evidence: "runtime_tested".into(),
            source: source.clone(),
        });
    }
    if let Probe::Found { value, source } = &dictionary {
        let automation = found.and_then(|a| a.bundle_id.as_deref()).map_or("tcc:automation".to_string(), |b| format!("tcc:automation:{b}"));
        let mut seen = HashSet::new();
        let mut push = |name: &str, summary: String, suite: &str, caps: &mut Vec<Capability>| {
            let lower = name.to_lowercase();
            if !seen.insert(lower.clone()) {
                return;
            }
            // 辞書は作用を書かない。読むだけと名前で分かる物だけ read、他は外に効く扱い
            let effect = matches!(lower.as_str(), "count" | "exists" | "get").then(|| "external_read".to_string());
            caps.push(Capability {
                id: format!("applescript.{}", lower.replace(' ', "_")),
                summary,
                gate: gate(effect.as_deref()),
                effect,
                route: "local".into(),
                endpoint: None,
                status: "available".into(),
                requires: vec![automation.clone()],
                reason: None,
                // inferred = 製品同梱の文書にしか無い
                evidence: "inferred".into(),
                source: Source { read: format!("{} {suite}", source.read), checked_at: source.checked_at.clone() },
            });
        };
        for s in &value.suites {
            for c in &s.commands {
                let summary = if c.description.is_empty() { c.name.clone() } else { format!("{} — {}", c.name, c.description) };
                push(&c.name, summary, &s.name, &mut caps);
            }
        }
        if value.properties > 0 {
            push("get", "get — read a property of a class (AppleScript's get; the dictionary doesn't list it as a command)".into(), "A property of a class", &mut caps);
        }
    }
    if let Some(why) = app.unchecked() {
        unknowns.push(Unknown { what: "Whether it is installed".into(), why: why.into() });
    }
    if found.is_some() {
        if let Some(why) = dictionary.unchecked() {
            unknowns.push(Unknown { what: "AppleScript dictionary".into(), why: why.into() });
        }
        if let Some(why) = windows.unchecked() {
            unknowns.push(Unknown { what: "Windows that accessibility can reach".into(), why: why.into() });
        }
    }
    let permissions = permissions(&caps, found);
    Intel {
        query: query.into(),
        checked_at: now.into(),
        app,
        dictionary,
        windows,
        capabilities: caps,
        unknowns,
        permissions,
    }
}

/// 要る権限: capability の requires の tcc: / oauth_scope: と、Info.plist の NS*UsageDescription（app 自身が聞く物）。
fn permissions(caps: &[Capability], app: Option<&App>) -> Vec<Permission> {
    let mut out: Vec<Permission> = Vec::new();
    let mut add = |id: &str, who: String, because: String| {
        if let Some(p) = out.iter_mut().find(|p| p.id == id && p.who == who) {
            if !p.because.contains(&because) {
                p.because.push(because);
            }
            return;
        }
        let (label, settings) = permission_label(id);
        out.push(Permission { id: id.into(), label, who, settings, because: vec![because] });
    };
    for c in caps {
        for r in c.requires.iter().filter(|r| r.starts_with("tcc:") || r.starts_with("oauth_scope:")) {
            // AI が窓を読む・押すのも Shibubu Bridge（Kangae の process）なので、AX の許可は経路によらず Kangae に（YC W18-1）
            let who = if r == "tcc:accessibility" {
                "Shibubu (or the app of the terminal it was started from)"
            } else if r.starts_with("oauth_scope:") {
                "The account to connect"
            } else {
                "The caller (the app or terminal running Shibubu)"
            };
            add(r.as_str(), who.into(), format!("{} ({})", c.id, c.route));
        }
    }
    for a in app.into_iter() {
        for (key, id) in USAGE.iter().filter(|(key, _)| a.asks.iter().any(|k| k.as_str() == *key)) {
            add(id, format!("{} itself (macOS asks when it is used)", a.name), format!("{key} in Info.plist"));
        }
    }
    out
}

fn permission_label(id: &str) -> (String, String) {
    let pane = |name: &str| format!("System Settings › Privacy & Security › {name}");
    let Some(t) = id.strip_prefix("tcc:") else {
        let scope = id.strip_prefix("oauth_scope:").unwrap_or(id);
        return (format!("OAuth scope {scope}"), "The consent screen when you connect".into());
    };
    match t.split_once(':') {
        Some(("automation", b)) => (format!("Automation (sends Apple events to {b})"), pane("Automation")),
        _ => match t {
            "automation" => ("Automation (sends Apple events)".into(), pane("Automation")),
            "accessibility" => ("Accessibility".into(), pane("Accessibility")),
            "microphone" => ("Microphone".into(), pane("Microphone")),
            "camera" => ("Camera".into(), pane("Camera")),
            "screen_recording" => ("Screen Recording".into(), pane("Screen Recording")),
            "full_disk_access" => ("Full Disk Access".into(), pane("Full Disk Access")),
            "speech_recognition" => ("Speech Recognition".into(), pane("Speech Recognition")),
            other => (other.into(), "System Settings › Privacy & Security".into()),
        },
    }
}

/// UTC の秒 → `YYYY-MM-DDTHH:MM:SSZ`（civil_from_days。crate を足さない）。
pub fn utc(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let s = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3_600, s % 3_600 / 60, s % 60)
}

/// 1 つの app を読むだけで調べる。timeout は外の command 1 回ごとの上限。起動しない・command を打たない。
pub fn investigate(query: &str, timeout: Duration) -> Result<Intel, String> {
    let q = normalize(query)?;
    let bundle_like = is_bundle_id(&q);
    let bundle = bundle_like.then(|| q.clone());
    let names: Vec<String> = if bundle_like { Vec::new() } else { vec![q.clone()] };
    let now = utc(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    let mut unknowns = Vec::new();
    let (app, plist, running) = find(bundle.as_deref(), bundle_like, &names, timeout, &now, &mut unknowns);
    let (dictionary, windows) = match (app.found(), &plist) {
        (Some(a), Some(p)) => (dictionary_probe(Path::new(&a.path), p, timeout, &now), windows_probe(&running, &now)),
        _ => (Probe::Unchecked { why: NO_APP.into() }, Probe::Unchecked { why: NO_APP.into() }),
    };
    Ok(assemble(&q, &now, app, dictionary, windows, unknowns))
}
