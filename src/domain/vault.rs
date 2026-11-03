//! PASSWORD-1101（owner 252「passwordmanagerをchatで入れられるようにしたいのとそれの安全性を担保すること」・254「sign in とカード番号住所など」）:
//! 利用者の password manager（1Password の `op`・macOS の Keychain の `security`・Bitwarden の `bw`）の項目を参照だけで持ち、値は使う瞬間だけ memory に置く。
//! 型（`Manager`・`SecretRef`・`Secret`）と CLI の手（`Manager::find`・`sign_in`・`list_items`・`with_secret`・`save`・`resolve`）を 1 つの module に置く。
//! 置き場の理由: 型は `ConnAct::UseItem`・`Shell`・`UiSettings` に載る = ui だけを `#[path]` で引く試験の crate（tests/ui_*.rs）にも在る domain。
//! CLI の手は型を受け渡すので同じ所（`storage::secret_store` は domain を引かない試験の crate〈tests/storage.rs・codegen.rs〉に在るので、そこに置けない。
//! 同じ理由で CLI を起こす私的な `run` は `secret_store::security` と別に持つ）。
//! 守る事: Shibubu は master password を受け取らない・値は argv に出さない（渡す時は stdin）・値を返す fn は持たない（`with_secret` が閉包に貸すだけ・
//! 例外は `resolve` = 今の String の token の口〈github〉へ渡す 1 つ）・Err は英語の次の一手の 1 行で、値と CLI の stdout を入れない。
//! 承認（4: 項目×送り先の初回・払う手は毎回）は呼ぶ側 = ENGINE の執行が `with_secret` を呼ぶ前に済ませる。
//! Wave: PASSWORD-1101
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 参照を Keychain の値として置く時の頭（`SecretRef::stored`）。Connections の行の秘密（`storage::secret_store::connection_key`）に値の代わりに入る。
pub const REF_HEAD: &str = "shibubu-ref:";
/// chat の card の見出し（AX の名前も）
pub const HEADING: &str = "Password manager";
/// chat に打った password らしき物を止めた card の題と次の 1 行
pub const HELD: &str = "That looks like a password. Shibubu didn't send it to the AI.";
pub const HELD_NEXT: &str = "Save it in a password manager, or delete it.";
/// 1Password の app の連携を待つ間の 1 行（card と Connections の行）
pub const WAIT_TOUCH_ID: &str = "Approve with Touch ID in 1Password";
/// Connections の Paste の欄の下の項目の見出し
pub const PICK: &str = "Or choose an item from your password manager";
/// 1Password の app の連携が無い（`op account list` が空）時の次の一手
pub const OP_SETUP: &str = "Install the 1Password app and turn on Settings › Developer › Integrate with 1Password CLI, then try again";
/// Bitwarden の CLI に鍵が掛かっている時の次の一手（CLI に Touch ID が無い = Shibubu は master password を受け取らない）
pub const BW_LOCKED: &str = "Bitwarden is locked · its CLI can't use Touch ID · run bw unlock in Terminal and start Shibubu from there";
/// CLI が見つからない時の入れ方（card の 1 行・Connections の行の detail）。install はしない
pub const OP_INSTALL: &str = "Install the 1Password app and the 1Password CLI, then turn on Settings › Developer › Integrate with 1Password CLI";
pub const BW_INSTALL: &str = "Install the Bitwarden CLI (bw), then check again";
/// Shibubu 自身の鍵の Keychain の service（`storage::secret_store::KEYCHAIN_SERVICE`・Keychain の一覧に出さない）。storage を引けないので字で持つ（app.rs の試験が同じかを見る）
pub const OWN_SERVICE: &str = "Kangae";
/// chat の値を Keychain に保存する service（Keychain Access で見える名前）
const SAVE_SERVICE: &str = "Shibubu";
/// CLI を待つ長さ（Touch ID・Keychain の許可の窓を人が押すまで）
const WAIT: Duration = Duration::from_secs(120);

/// 利用者の password manager。繋いだ物は `UiSettings::vaults`（保存は小文字の名前）。
/// Wave: PASSWORD-1101
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    /// 1Password の CLI `op`（app の連携 = Touch ID で sign in・master password は Shibubu が受け取らない）
    OnePassword,
    /// macOS の Keychain（`/usr/bin/security`・login の keychain は Mac の login で開いている・項目の値は macOS の許可の窓の後）
    Keychain,
    /// Bitwarden の CLI `bw`（CLI に Touch ID が無い = `bw unlock` した terminal から起こした Shibubu だけが使える・保存はしない）
    Bitwarden,
}

impl Manager {
    pub const ALL: [Manager; 3] = [Manager::OnePassword, Manager::Keychain, Manager::Bitwarden];

    /// 画面の名前
    pub const fn label(self) -> &'static str {
        match self {
            Manager::OnePassword => "1Password",
            Manager::Keychain => "Keychain",
            Manager::Bitwarden => "Bitwarden",
        }
    }

    /// 参照の scheme（`op://…`・`keychain://…`・`bw://…`）と Connections の行の key の後ろ
    pub const fn scheme(self) -> &'static str {
        match self {
            Manager::OnePassword => "op",
            Manager::Keychain => "keychain",
            Manager::Bitwarden => "bw",
        }
    }

    /// Connections の行の key（`pm:op`）。`of_row` が逆。
    pub fn row(self) -> String {
        format!("pm:{}", self.scheme())
    }

    pub fn of_row(row: &str) -> Option<Manager> {
        let s = row.strip_prefix("pm:")?;
        Manager::ALL.into_iter().find(|m| m.scheme() == s)
    }

    /// Mac の app の bundle id（Connections の行の icon・`theme::kind_icon`）。Keychain は None（行は頭文字の丸）
    pub const fn app(self) -> Option<&'static str> {
        match self {
            Manager::OnePassword => Some("com.1password.1password"),
            Manager::Keychain => None,
            Manager::Bitwarden => Some("com.bitwarden.desktop"),
        }
    }

    /// この Mac の CLI（実行できる file）。`/opt/homebrew/bin`・`/usr/local/bin`・`/usr/bin`・PATH の順（Finder から起こした .app の PATH は短い）。
    /// Keychain = `/usr/bin/security`。install はしない（None = card が入れ方を言う）。
    pub fn find(self) -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let name = match self {
            Manager::OnePassword => "op",
            Manager::Keychain => return Some(PathBuf::from("/usr/bin/security")).filter(|p| p.is_file()),
            Manager::Bitwarden => "bw",
        };
        let path = std::env::var_os("PATH").unwrap_or_default();
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .into_iter()
            .map(PathBuf::from)
            .chain(std::env::split_paths(&path))
            .map(|d| d.join(name))
            .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
    }

    /// chat の一言が名指しした password manager（`1Password を使って`・`use my keychain`・`キーチェーン`・`bitwarden`）。無ければ None。
    /// 保証しない: 使いたいのか尋ねているだけかの区別（名指しなら繋ぐ card = 繋いでいれば呼ぶ側が出さない）。
    pub fn asked(text: &str) -> Option<Manager> {
        let low = text.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| low.contains(w));
        if has(&["1password", "1 password", "ワンパスワード"]) {
            Some(Manager::OnePassword)
        } else if has(&["keychain", "キーチェーン"]) {
            Some(Manager::Keychain)
        } else if has(&["bitwarden", "ビットワーデン"]) {
            Some(Manager::Bitwarden)
        } else {
            None
        }
    }
}

/// password manager の 1 項目の参照（値は持たない）。`path` = manager の中の場所（1Password = `vault/item/field` の id・Keychain = `service/account`
/// 〈それぞれの `%` と `/` は `%25`・`%2F`〉・Bitwarden = item の id）・`name` = 画面の名前（1Password の題・Keychain の `service · account`）。
/// Wave: PASSWORD-1101
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRef {
    pub manager: Manager,
    pub path: String,
    pub name: String,
}

impl SecretRef {
    /// CLI に渡す参照（`op://v/i/password`・`keychain://svc/acct`・`bw://id`）
    pub fn uri(&self) -> String {
        format!("{}://{}", self.manager.scheme(), self.path)
    }

    /// 画面の名前 `1Password · GitHub token`（値は出さない）
    pub fn label(&self) -> String {
        format!("{} · {}", self.manager.label(), self.name)
    }

    /// Keychain の値として置く 1 行（`shibubu-ref:` ＋ uri ＋ TAB ＋ 名前）。`parse` が逆。
    pub fn stored(&self) -> String {
        format!("{REF_HEAD}{}\t{}", self.uri(), self.name.replace(['\t', '\n'], " "))
    }

    /// `stored` の 1 行 → 参照。頭が `REF_HEAD` でない（= 貼った値そのもの）・scheme を知らない・場所が空・名前が空は None。
    pub fn parse(stored: &str) -> Option<SecretRef> {
        let (uri, name) = stored.strip_prefix(REF_HEAD)?.split_once('\t')?;
        let (scheme, path) = uri.split_once("://")?;
        let manager = Manager::ALL.into_iter().find(|m| m.scheme() == scheme)?;
        (!path.is_empty() && !name.trim().is_empty()).then(|| SecretRef { manager, path: path.into(), name: name.into() })
    }
}

/// 使う瞬間だけ memory に置く値。Clone・Display・Serialize を持たず、Debug は `Secret(…)`、Drop で中の byte を 0 にする。
/// 例: `with_secret(&r, &cli, |s| req.set("Authorization", &format!("Bearer {}", s.expose())))`（閉包の外へ値を持ち出さない）
/// 保証しない: OS の pipe・CLI の process・途中で作った写し（`format!` の結果など）を消す事・最適化が 0 にする書き込みを消さない事（`black_box` で止める）。
/// Wave: PASSWORD-1101
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Secret(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }
}

/// PASSWORD-1101 の 1: sign in（裏の thread で呼ぶ・Touch ID を待つ）。1Password = `op account list` が空なら app の連携が無い（Err `OP_SETUP`・`signin` を起こさない =
/// master password を聞く道に入らない）→ `op signin`（app の連携が Touch ID を出す）。Keychain = 何もしない（login の keychain は開いている）。
/// Bitwarden = `bw status` が `unlocked` の時だけ Ok（他は Err `BW_LOCKED`）。stdin は閉じたまま。
/// 保証しない: sign in が続く長さ（1Password は 10 分使わないと次の読みで Touch ID をまた出す）。
/// Wave: PASSWORD-1101
pub fn sign_in(m: Manager, cli: &Path) -> Result<(), String> {
    match m {
        Manager::Keychain => Ok(()),
        Manager::OnePassword => {
            let (ok, out, _) = run(cli, &["account", "list", "--format", "json"], None)?;
            let accounts = serde_json::from_str::<Vec<serde_json::Value>>(out.expose()).map_or(0, |a| a.len());
            if !ok || accounts == 0 {
                return Err(OP_SETUP.to_owned());
            }
            match run(cli, &["signin"], None)? {
                (true, _, _) => Ok(()),
                (false, _, why) => Err(failed(m, &why)),
            }
        }
        Manager::Bitwarden => {
            let (_, out, _) = run(cli, &["status"], None)?;
            let status = serde_json::from_str::<serde_json::Value>(out.expose()).ok().and_then(|v| v["status"].as_str().map(str::to_owned));
            if status.as_deref() == Some("unlocked") {
                Ok(())
            } else {
                Err(BW_LOCKED.to_owned())
            }
        }
    }
}

/// PASSWORD-1101 の 2: 項目の参照と名前だけ（値は読まない・答えに値が入る Bitwarden も、名前と id を取った後に答えを 0 にする）。並び = 名前の順。
/// 1Password = `op item list --format json` の Login・Password・API Credential・Database・Server・Email Account（field = `password`・API Credential は `credential`・
/// カードと住所は ENGINE の 254 の道 = 欄ごとの参照を ENGINE が組む）。Keychain = `security dump-keychain`（属性だけ）の generic password（`OWN_SERVICE` は除く）。
/// Bitwarden = `bw list items` の login。
/// Wave: PASSWORD-1101
pub fn list_items(m: Manager, cli: &Path) -> Result<Vec<SecretRef>, String> {
    let json = |args: &[&str]| -> Result<Vec<serde_json::Value>, String> {
        let (ok, out, why) = run(cli, args, None)?;
        if !ok {
            return Err(if m == Manager::Bitwarden && why.to_lowercase().contains("locked") { BW_LOCKED.to_owned() } else { failed(m, &why) });
        }
        // ponytail: serde の Value の中の写し（Bitwarden の答えは値を含む）は 0 にしない・答えの文（`out`）は Drop で 0
        serde_json::from_str(out.expose()).map_err(|_| failed(m, "its answer wasn't a list"))
    };
    let mut items: Vec<SecretRef> = match m {
        Manager::OnePassword => json(&["item", "list", "--format", "json"])?
            .iter()
            .filter_map(|i| {
                let field = match i["category"].as_str()? {
                    "LOGIN" | "PASSWORD" | "DATABASE" | "SERVER" | "EMAIL_ACCOUNT" => "password",
                    "API_CREDENTIAL" => "credential",
                    _ => return None,
                };
                let (vault, id, title) = (i["vault"]["id"].as_str()?, i["id"].as_str()?, i["title"].as_str()?);
                Some(SecretRef { manager: m, path: format!("{vault}/{id}/{field}"), name: title.to_owned() })
            })
            .collect(),
        Manager::Bitwarden => json(&["list", "items"])?
            .iter()
            .filter(|i| i["type"].as_u64() == Some(1))
            .filter_map(|i| Some(SecretRef { manager: m, path: i["id"].as_str()?.to_owned(), name: i["name"].as_str()?.to_owned() }))
            .collect(),
        Manager::Keychain => {
            // 属性だけ（`-d` を付けない = 値は読まない・許可の窓も出ない）。1 項目 = `keychain: "…"` から次まで・generic password = `class: "genp"`・
            // 名前は `    "svce"<blob>="…"` の形の物だけ（16 進の `0x…` は読まない）
            let (ok, out, why) = run(cli, &["dump-keychain"], None)?;
            if !ok {
                return Err(failed(m, &why));
            }
            let quoted = |rec: &str, key: &str| {
                let head = format!("\"{key}\"<blob>=\"");
                rec.lines().find_map(|l| l.trim().strip_prefix(head.as_str())?.strip_suffix('"').map(str::to_owned))
            };
            let enc = |s: &str| s.replace('%', "%25").replace('/', "%2F");
            out.expose()
                .split("keychain: ")
                .filter(|rec| rec.contains("class: \"genp\""))
                .filter_map(|rec| {
                    let svce = quoted(rec, "svce").filter(|s| !s.is_empty() && s != OWN_SERVICE)?;
                    let acct = quoted(rec, "acct").unwrap_or_default();
                    let name = if acct.is_empty() { svce.clone() } else { format!("{svce} · {acct}") };
                    Some(SecretRef { manager: m, path: format!("{}/{}", enc(&svce), enc(&acct)), name })
                })
                .collect()
        }
    };
    items.sort_by(|a, b| (a.name.to_lowercase(), &a.path).cmp(&(b.name.to_lowercase(), &b.path)));
    items.dedup_by(|a, b| a.path == b.path);
    Ok(items)
}

/// PASSWORD-1101 の 2・4（ENGINE が承認の後に呼ぶ口）: 参照の値を使う瞬間だけ取り、閉包に貸して、返った後に消す（`Secret` の Drop）。返すのは閉包の答えで値ではない。
/// 1Password = `op read --no-newline <uri>`・Keychain = `security find-generic-password -s <service> -a <account> -w`（macOS の許可の窓）・Bitwarden = `bw get password <id>`。
/// 例（ENGINE）: `let cli = r.manager.find().ok_or(OP_SETUP)?; vault::with_secret(&r, &cli, |s| send(s.expose()))?`
/// 保証する: 値を Err・Debug に出さない。保証しない: 閉包が値を写して持ち出さない事（呼ぶ側の約束）。
/// Wave: PASSWORD-1101
pub fn with_secret<R>(r: &SecretRef, cli: &Path, use_it: impl FnOnce(&Secret) -> R) -> Result<R, String> {
    let dec = |s: &str| s.replace("%2F", "/").replace("%25", "%");
    let args: Vec<String> = match r.manager {
        Manager::OnePassword => vec!["read".into(), "--no-newline".into(), r.uri()],
        Manager::Keychain => {
            let (svce, acct) = r.path.split_once('/').ok_or_else(|| failed(r.manager, "the item's place is broken"))?;
            let mut a = vec!["find-generic-password".into(), "-s".into(), dec(svce)];
            if !acct.is_empty() {
                a.extend(["-a".into(), dec(acct)]);
            }
            a.push("-w".into());
            a
        }
        Manager::Bitwarden => vec!["get".into(), "password".into(), r.path.clone()],
    };
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let (ok, out, why) = run(cli, &argv, None)?;
    if !ok {
        return Err(failed(r.manager, &why));
    }
    // `security -w` と `bw get` は末尾に改行（1Password は `--no-newline`）
    let value = Secret::new(out.expose().strip_suffix('\n').unwrap_or(out.expose()).to_owned());
    Ok(use_it(&value))
}

/// PASSWORD-1101 の 3: chat で止めた値を manager に新しい項目として保存し、参照を返す。1Password = `op item create --format json -`（値は stdin の JSON）・
/// Keychain = service `Shibubu`・account = 名前（`security -i` の stdin に hex）。Bitwarden は保存しない（Err・CLI が base64 の JSON を要る）。
/// Wave: PASSWORD-1101
pub fn save(m: Manager, cli: &Path, name: &str, value: &Secret) -> Result<SecretRef, String> {
    match m {
        Manager::OnePassword => {
            // ponytail: json! の中の写しは 0 にしない・stdin に流す文は Secret（Drop で 0）
            let field = serde_json::json!({"id": "password", "type": "CONCEALED", "purpose": "PASSWORD", "label": "password", "value": value.expose()});
            let template = Secret::new(serde_json::json!({"title": name, "category": "PASSWORD", "fields": [field]}).to_string());
            let (ok, out, why) = run(cli, &["item", "create", "--format", "json", "-"], Some(&template))?;
            if !ok {
                return Err(failed(m, &why));
            }
            let item: serde_json::Value = serde_json::from_str(out.expose()).map_err(|_| failed(m, "its answer wasn't an item"))?;
            match (item["vault"]["id"].as_str(), item["id"].as_str()) {
                (Some(vault), Some(id)) => Ok(SecretRef { manager: m, path: format!("{vault}/{id}/password"), name: name.to_owned() }),
                _ => Err(failed(m, "its answer had no item id")),
            }
        }
        Manager::Keychain => {
            // `security -i` の 1 行は空白で区切る = account は英数字と `. _ - @` だけ（他は `-`・続く `-` は 1 つに）
            let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@') { c } else { '-' }).take(200).collect();
            let account = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
            let hex: String = value.expose().bytes().map(|b| format!("{b:02x}")).collect();
            let line = Secret::new(format!("add-generic-password -U -a {account} -s {SAVE_SERVICE} -X {hex}\n"));
            drop(Secret::new(hex));
            let (ok, _, why) = run(cli, &["-i"], Some(&line))?;
            // `security -i` は 1 行の誤りでも 0 で終わる事がある = stderr が在れば失敗と読む
            if !ok || !why.is_empty() {
                return Err(failed(m, &why));
            }
            Ok(SecretRef { manager: m, path: format!("{SAVE_SERVICE}/{account}"), name: format!("{SAVE_SERVICE} · {account}") })
        }
        Manager::Bitwarden => Err("Shibubu can't save to Bitwarden · save it in the Bitwarden app".to_owned()),
    }
}

/// PASSWORD-1101 の 2: Connections の行の秘密（`connection_key` の値）を使う瞬間に値へ。参照（`SecretRef::stored`）なら manager から読み（`with_secret`）、
/// 貼った値ならそのまま。読めない（manager が無い・sign in が切れた・断られた）は None（呼ぶ側は token 無しと同じ = 401 の 1 行が次の一手を言う）。
/// 呼ぶ所: `github::Api::from_env`・Web data の読み（どちらも裏の thread・UI の thread で呼ばない = Touch ID を待つ）。
/// Wave: PASSWORD-1101
pub fn resolve(stored: String) -> Option<String> {
    let Some(r) = SecretRef::parse(&stored) else { return Some(stored) };
    let cli = r.manager.find()?;
    with_secret(&r, &cli, |s| s.expose().to_owned()).ok().filter(|v| !v.is_empty())
}

/// 読めなかった・sign in できなかった時の 1 行（docs/ui-copy/additions.md）。`why` = CLI の stderr の 1 行目（値は入らない）
fn failed(m: Manager, why: &str) -> String {
    format!("Couldn't use {} · {} · Connect again", m.label(), if why.is_empty() { "no answer" } else { why })
}

/// CLI を 1 回起こし、stdin（在れば `Secret` の文）を流して閉じ、`WAIT` で止める。返り = (0 で終わったか, stdout〈値が入りうる = Secret〉, stderr の 1 行目〈1Password の
/// `[ERROR] 日 時` の頭は落とす〉)。stdout と stderr は別 thread で吸う（大きな答え = Keychain の一覧で pipe が詰まらない）。`secret_store::security` と別に持つ理由は module の doc。
fn run(cli: &Path, args: &[&str], stdin: Option<&Secret>) -> Result<(bool, Secret, String), String> {
    let name = cli.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut child = Command::new(cli)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start {name} · {}", e.kind()))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.expose().as_bytes()); // drop で閉じる = CLI は EOF で読み終わる
    }
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::with_capacity(4096);
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            b
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} didn't answer · try again"));
            }
        }
    };
    let stdout = match String::from_utf8(out.join().unwrap_or_default()) {
        Ok(s) => Secret::new(s),
        Err(e) => {
            let mut b = e.into_bytes();
            b.fill(0);
            Secret::new(String::new())
        }
    };
    let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    let line = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = match line.strip_prefix("[ERROR] ") {
        Some(rest) => rest.splitn(3, ' ').nth(2).unwrap_or(rest),
        None => line,
    };
    Ok((status.success(), stdout, line.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = "SHIBUBU-TEST-SECRET-7f3a9c2e";

    /// 偽の `op`（本物の 1Password に触らない）。`account list` = `accounts`（`[]` = app の連携が無い形）・`signin` = 0・`item list` = 名前の一覧（値は無い）・
    /// `item create` = stdin を dir の `created.json` へ・`read` = 偽の値。呼ばれた引数を dir の `argv.log` に 1 行ずつ。
    fn fake_op(tag: &str, accounts: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("shibubu-pw1101-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let op = dir.join("op");
        let items = r#"[{"id":"i1","title":"GitHub token","vault":{"id":"v1","name":"Private"},"category":"API_CREDENTIAL"},{"id":"i2","title":"Bank card","vault":{"id":"v1","name":"Private"},"category":"CREDIT_CARD"},{"id":"i3","title":"Mail","vault":{"id":"v2","name":"Work"},"category":"LOGIN"}]"#;
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$1 $2\" in\n  'account list') printf '%s' '{accounts}' ;;\n  signin*) exit 0 ;;\n  'item list') printf '%s' '{items}' ;;\n  'item create') cat > '{created}'; printf '%s' '{{\"id\":\"i9\",\"title\":\"x\",\"vault\":{{\"id\":\"v1\"}}}}' ;;\n  'read --no-newline') printf '%s' '{FAKE}' ;;\n  *) echo \"unknown: $*\" >&2; exit 2 ;;\nesac\n",
            log = dir.join("argv.log").display(),
            created = dir.join("created.json").display(),
        );
        std::fs::write(&op, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&op, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, op)
    }

    /// PASSWORD-1101 の 2・3: 参照は値を持たず Keychain に置く 1 行と行き来できる・貼った値そのものは参照と読まない・値の Debug は伏せる。
    /// 負の対照: `Secret` の Debug を中身にする → 1 つ目の assert が赤 / `parse` が `REF_HEAD` を見ない → 貼った token を参照と読んで赤。
    #[test]
    fn password_1101_a_ref_round_trips_and_a_value_never_prints() {
        let s = Secret::new(FAKE.into());
        assert_eq!(format!("{s:?}"), "Secret(…)");
        assert_eq!(s.expose(), FAKE);
        let r = SecretRef { manager: Manager::OnePassword, path: "v1/i1/credential".into(), name: "GitHub token".into() };
        assert_eq!(r.uri(), "op://v1/i1/credential");
        assert_eq!(r.label(), "1Password · GitHub token");
        assert_eq!(SecretRef::parse(&r.stored()), Some(r.clone()));
        let k = SecretRef { manager: Manager::Keychain, path: "My%2FService/me@example.com".into(), name: "My/Service · me@example.com".into() };
        assert_eq!(SecretRef::parse(&k.stored()), Some(k));
        assert_eq!(SecretRef::parse("ghp_0123456789abcdef0123456789"), None, "貼った token を参照と読んだ");
        assert_eq!(SecretRef::parse("shibubu-ref:ftp://x\tX"), None);
        assert_eq!(SecretRef::parse("shibubu-ref:op://\tX"), None);
        assert_eq!(Manager::of_row("pm:keychain"), Some(Manager::Keychain));
        assert_eq!(Manager::of_row("github"), None);
    }

    /// PASSWORD-1101 の 1: chat の一言が名指しした manager（日本語も）。名指しでない一言・password の語だけは None。
    /// 負の対照: `asked` が小文字にしない → `Use my Keychain` が None で赤。
    #[test]
    fn password_1101_chat_names_a_manager() {
        assert_eq!(Manager::asked("1Password を使って"), Some(Manager::OnePassword));
        assert_eq!(Manager::asked("use 1password for github"), Some(Manager::OnePassword));
        assert_eq!(Manager::asked("キーチェーンに入れて"), Some(Manager::Keychain));
        assert_eq!(Manager::asked("Use my Keychain"), Some(Manager::Keychain));
        assert_eq!(Manager::asked("connect Bitwarden"), Some(Manager::Bitwarden));
        assert_eq!(Manager::asked("Track my inventory"), None);
        assert_eq!(Manager::asked("password を変えたい"), None);
    }

    /// PASSWORD-1101 の 4（ENGINE が呼ぶ形）: 承認の後に ENGINE が参照から値を使う瞬間だけ受け取る。値は閉包の中だけ・返すのは閉包の答え・argv に値が無い。
    /// 負の対照: `with_secret` が `--no-newline` を外す → 偽の op が知らない引数で止まり Err（argv の assert も）で赤 / 閉包に値でなく参照の uri を渡す → 長さが違って赤。
    #[test]
    fn password_1101_engine_uses_a_value_only_inside_with_secret() {
        let (dir, op) = fake_op("engine", "[]");
        let r = SecretRef::parse(&format!("{REF_HEAD}op://v1/i1/credential\tGitHub token")).expect("参照");
        // ENGINE の形: 送り先へ渡す手を閉包の中で組む（ここでは長さと頭だけを外へ）
        let sent = with_secret(&r, &op, |s| (s.expose().len(), s.expose().starts_with("SHIBUBU-TEST-SECRET-"))).expect("読めた");
        assert_eq!(sent, (FAKE.len(), true));
        let argv = std::fs::read_to_string(dir.join("argv.log")).unwrap();
        assert_eq!(argv.trim(), "read --no-newline op://v1/i1/credential");
        assert!(!format!("{r:?}").contains(FAKE));
    }

    /// PASSWORD-1101 の 2: 項目の一覧は名前と参照だけ（カードは出さない・名前の順）。偽の値はどこにも無い。
    /// 負の対照: category の表に CREDIT_CARD を足す → `Bank card` が出て赤。
    #[test]
    fn password_1101_items_are_names_and_refs_only() {
        let (_dir, op) = fake_op("items", "[]");
        let got = list_items(Manager::OnePassword, &op).expect("一覧");
        let want = vec![
            SecretRef { manager: Manager::OnePassword, path: "v1/i1/credential".into(), name: "GitHub token".into() },
            SecretRef { manager: Manager::OnePassword, path: "v2/i3/password".into(), name: "Mail".into() },
        ];
        assert_eq!(got, want);
        assert!(!format!("{got:?}").contains(FAKE));
    }

    /// PASSWORD-1101 の 1: app の連携が無い（account が 0）時は `signin` を起こさず次の一手を返す = master password を聞く道に入らない。在れば `signin` 1 回。
    /// 負の対照: `account list` を見ずに `signin` を起こす → argv.log に signin が在って赤。
    #[test]
    fn password_1101_sign_in_never_asks_for_the_master_password() {
        let (dir, op) = fake_op("signin-none", "[]");
        assert_eq!(sign_in(Manager::OnePassword, &op), Err(OP_SETUP.to_owned()));
        assert!(!std::fs::read_to_string(dir.join("argv.log")).unwrap().contains("signin"), "連携が無いのに signin を起こした");
        let (dir, op) = fake_op("signin-one", r#"[{"url":"my.1password.com","email":"a@example.com","user_uuid":"U","account_uuid":"A"}]"#);
        assert_eq!(sign_in(Manager::OnePassword, &op), Ok(()));
        assert_eq!(std::fs::read_to_string(dir.join("argv.log")).unwrap().lines().filter(|l| l.starts_with("signin")).count(), 1);
        assert_eq!(sign_in(Manager::Keychain, Path::new("/nonexistent/security")), Ok(()), "Keychain は何も起こさない");
    }

    /// PASSWORD-1101 の 3: chat で止めた値の保存は stdin だけで渡し（argv に無い）、参照を返す。Bitwarden は保存しない。
    /// 負の対照: 値を `password=…` の代入の引数で渡す → argv.log に値が在って赤。
    #[test]
    fn password_1101_save_sends_the_value_on_stdin_only() {
        let (dir, op) = fake_op("save", "[]");
        let r = save(Manager::OnePassword, &op, "Password from chat", &Secret::new(FAKE.into())).expect("保存");
        assert_eq!(r, SecretRef { manager: Manager::OnePassword, path: "v1/i9/password".into(), name: "Password from chat".into() });
        assert!(!std::fs::read_to_string(dir.join("argv.log")).unwrap().contains(FAKE), "値が argv に出た");
        assert!(std::fs::read_to_string(dir.join("created.json")).unwrap().contains(FAKE), "偽の 1Password に値が届いていない");
        assert!(save(Manager::Bitwarden, &op, "x", &Secret::new(FAKE.into())).is_err());
    }
}
