//! W18-2 撤去の回帰（YC_WAVES §5.5 W18-2・8 通目「代わりが足りなければ W18 を完了にしない」）の契約。
//! 製品から CuaDriver が消えた事を、source・依存・release の binary・.app の中身の 4 か所で機械的に見る。
//! **製品の code（`src/`）には置かない**: 判定は `WORDS` の語を literal で持つので、`src/` に書くと release の binary の `strings` と
//! `rg -i cua src` が自分に当たる（W18-2 の門が自分で赤くなる）。`tests/common/old_new.rs` と同じく試験の側に置く。
//! 判定の純関数は text / bytes を受けるだけで process を起こさない（`otool -L` の出力・`Cargo.lock` の中身は呼ぶ側が渡す）。
//! 許してよい行は `ALLOWLIST_PATH`（`.app` の外・commit する）だけ。知らない行は赤。W17-3 の表の比べは `tests/common/old_new.rs` の `Table::against`。
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::path::Path;

/// release の binary の `strings` で引く語（YC_WAVES W18-2）。大小文字を区別しない。素の "cua" では引かない
/// （`AdapterKind` の serde alias `"cua"`・FromStr の `"cua_browser_tab"` は W17-2 #12 で意図して残した古い保存の読み）。
pub const WORDS: [&str; 5] = [
    "cua-driver",
    "CuaDriver",
    "cuadriver",
    "CUA_DRIVER",
    "trycua",
];

/// 許す行の正本（repo の root から）と形の版。形を変えたら版を上げ、版の違う file は読まない。
pub const ALLOWLIST_PATH: &str = "scripts/qa/cua-allowlist.json";
pub const ALLOWLIST_VERSION: u32 = 1;

/// 許す行の 1 つ。行番号はずれるので file と部分文字列で当てる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrcLine {
    /// repo の root からの path（例 `"src/app.rs"`）
    pub file: String,
    /// その行が含む文字列（大小文字を区別する・trim した行の部分文字列）
    pub contains: String,
    pub kind: Kind,
    /// 残す理由 1 行（空は Allowlist::load が拒む）
    pub why: String,
}

/// 許す理由の種類。YC_WAVES W18-2 の「説明の comment の allowlist だけ」に、W17-2 #12 で残すと決めた 2 種を足した
/// （`Comment` 以外の行は `why` に決めた Wave を書く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// 説明の comment（試験の道具の cua-driver・旧い道の由来を説明する）
    Comment,
    /// `#[cfg(test)]` の中の code（例 `app.rs` の `connections_row_reports_bridge_trust_without_cua`）
    Test,
    /// 古い保存を読むための data（`capability.rs` の `#[serde(alias = "cua")]`・FromStr の `"cua" | "Cua" | "cua_browser_tab"`）
    CompatData,
}

/// release の binary の `strings` に出てよい 1 つ（`WORDS` に当たる文字列のうち）。今は 0 件が既定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryString {
    pub contains: String,
    pub why: String,
}

/// `ALLOWLIST_PATH` の形（serde_json・pretty で 1 つ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowlist {
    pub version: u32,
    /// `rg -i cua src` に当たってよい行
    pub src_lines: Vec<SrcLine>,
    pub binary_strings: Vec<BinaryString>,
    /// `.app` の中の file の一覧（`.app` の root からの path・昇順。例 `"Contents/MacOS/shibubu"`）。これと完全に一致する事を見る
    pub app_contents: Vec<String>,
}

/// 見つけた漏れ 1 つ。どれも赤（緑にしてよいのは空の Vec だけ）。測れなかった時は判定の側が `Err(理由)`（未測・緑にしない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leak {
    /// `src/` の行が大小文字を問わず "cua" を含み、allowlist の `src_lines` に無い
    Src {
        file: String,
        line: usize,
        text: String,
    },
    /// release の binary の印字できる文字列（4 byte 以上）が `WORDS` を含み、`binary_strings` に無い
    Binary { binary: String, text: String },
    /// `otool -L` の行が "cua" を含む
    Linked { binary: String, lib: String },
    /// `.app` の中の path の名前が "cua" を含む（`find -iname '*cua*'`）
    AppNamed { path: String },
    /// `.app` に在るが `app_contents` に無い（`scripts/qa/`・cua-driver の binary の混入など）
    AppExtra { path: String },
    /// `app_contents` に在るが `.app` に無い（一覧が古い。赤にして一覧を直させる）
    AppMissing { path: String },
    /// `Cargo.lock` の package の name が "cua" を含む（`cargo tree` の代わり・process を起こさず決定的）
    Crate { name: String },
}

impl Allowlist {
    /// 使い方: `let allow = Allowlist::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(ALLOWLIST_PATH))?;`
    /// Err（理由 1 行）: 読めない・形が違う（知らない欄）・版が違う・`why` が空の行がある。
    /// 保証しない: 許した行が今も src に在る事（それは `src_hits` が見ない・古い行は残っても赤にならない）。
    pub fn load(path: &Path) -> Result<Allowlist, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("allowlist を読めません（{}）: {e}", path.display()))?;
        let a: Allowlist = serde_json::from_str(&text)
            .map_err(|e| format!("allowlist の形が違います（{}）: {e}", path.display()))?;
        if a.version != ALLOWLIST_VERSION {
            return Err(format!(
                "allowlist の版が {}（読めるのは {ALLOWLIST_VERSION}）: {}",
                a.version,
                path.display()
            ));
        }
        let blank = a
            .src_lines
            .iter()
            .map(|l| &l.why)
            .chain(a.binary_strings.iter().map(|b| &b.why))
            .any(|w| w.trim().is_empty());
        if blank {
            return Err(format!(
                "allowlist に理由（why）が空の行があります: {}",
                path.display()
            ));
        }
        Ok(a)
    }
}

/// `root`（repo の root）の下の `src/` の file を全部読む（`rg -i cua src` と同じく拡張子で絞らない）。返り値は（repo の root からの path, 中身）を path の昇順で。
/// 使い方: `let files = read_src(Path::new(env!("CARGO_MANIFEST_DIR")))?; assert!(src_hits(&files, &allow).is_empty())`
/// Err = 読めない file・UTF-8 でない file がある・`src/` が空（未測）。
pub fn read_src(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for rel in walk(&root.join("src"))? {
        let path = format!("src/{rel}");
        let text = std::fs::read_to_string(root.join(&path))
            .map_err(|e| format!("{path} を読めません: {e}"))?;
        out.push((path, text));
    }
    if out.is_empty() {
        return Err(format!("{} の下に file が無い", root.join("src").display()));
    }
    Ok(out)
}

/// `dir` の下の file（dir は入れない）を `dir` からの `/` 区切りの path で昇順に（`find -type f | sort` と同じ並び）。
/// symlink は辿らず file として数える。
fn walk(dir: &Path) -> Result<Vec<String>, String> {
    fn go(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
        let rd =
            std::fs::read_dir(dir).map_err(|e| format!("{} を読めません: {e}", dir.display()))?;
        for e in rd {
            let e = e.map_err(|e| format!("{} を読めません: {e}", dir.display()))?;
            let p = e.path();
            let ty = e
                .file_type()
                .map_err(|e| format!("{} を読めません: {e}", p.display()))?;
            if ty.is_dir() {
                go(base, &p, out)?;
            } else {
                let rel = p.strip_prefix(base).unwrap();
                out.push(
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// `rg -i cua src` の判定。"cua" を大小文字を問わず含む行のうち、同じ file で `contains` を含む `src_lines` の行が無い物を返す。
/// 保証する: 入力だけで決まる（時刻・env を読まない）。保証しない: comment の中かどうかの判定（kind は人が決めて allowlist に書く）。
pub fn src_hits(files: &[(String, String)], allow: &Allowlist) -> Vec<Leak> {
    let mut hits = Vec::new();
    for (file, text) in files {
        let allowed: Vec<&str> = allow
            .src_lines
            .iter()
            .filter(|a| &a.file == file)
            .map(|a| a.contains.as_str())
            .collect();
        for (i, line) in text.lines().enumerate() {
            let t = line.trim();
            if t.to_lowercase().contains("cua") && !allowed.iter().any(|c| t.contains(c)) {
                hits.push(Leak::Src {
                    file: file.clone(),
                    line: i + 1,
                    text: t.to_string(),
                });
            }
        }
    }
    hits
}

/// release の binary 1 本の `strings -a` 相当（印字できる ASCII の 4 byte 以上の並び）を `WORDS` で引き、`binary_strings` に無い物を返す。
/// 使い方: `binary_hits("shibubu", &std::fs::read(bin)?, &allow)`。`strings` の command は使わない（入力の bytes だけで決まる）。
pub fn binary_hits(binary: &str, bytes: &[u8], allow: &Allowlist) -> Vec<Leak> {
    let words: Vec<String> = WORDS.iter().map(|w| w.to_lowercase()).collect();
    bytes
        .split(|b| !(0x20..=0x7e).contains(b))
        .filter(|run| run.len() >= 4)
        .map(|run| String::from_utf8_lossy(run).into_owned())
        .filter(|s| {
            let l = s.to_lowercase();
            words.iter().any(|w| l.contains(w.as_str()))
                && !allow.binary_strings.iter().any(|b| s.contains(&b.contains))
        })
        .map(|text| Leak::Binary {
            binary: binary.to_string(),
            text,
        })
        .collect()
}

/// `otool -L <binary>` の出力（呼ぶ側が渡す）で "cua" を含む行。1 行目（binary 自身の path）は見ない。
pub fn linked_hits(binary: &str, otool_l: &str) -> Vec<Leak> {
    otool_l
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| l.to_lowercase().contains("cua"))
        .map(|l| Leak::Linked {
            binary: binary.to_string(),
            lib: l.to_string(),
        })
        .collect()
}

/// `.app` の中の file の一覧（`.app` の root からの path・昇順・dir は入れない）。Err = 読めない（未測）。
pub fn app_listing(app: &Path) -> Result<Vec<String>, String> {
    let v = walk(app)?;
    if v.is_empty() {
        return Err(format!("{} の中に file が無い", app.display()));
    }
    Ok(v)
}

/// `.app` の一覧の判定: 名前に "cua" を含む path（`AppNamed`）・`app_contents` との差（`AppExtra` / `AppMissing`）。
pub fn app_hits(listing: &[String], allow: &Allowlist) -> Vec<Leak> {
    let named = listing
        .iter()
        .filter(|p| p.to_lowercase().contains("cua"))
        .map(|p| Leak::AppNamed { path: p.clone() });
    let extra = listing
        .iter()
        .filter(|p| !allow.app_contents.contains(p))
        .map(|p| Leak::AppExtra { path: p.clone() });
    let missing = allow
        .app_contents
        .iter()
        .filter(|p| !listing.contains(p))
        .map(|p| Leak::AppMissing { path: p.clone() });
    named.chain(extra).chain(missing).collect()
}

/// `Cargo.lock` の中身で package の `name = "…"` が "cua" を含む物（`cargo tree` に無い事の代わり）。
pub fn lock_hits(cargo_lock: &str) -> Vec<Leak> {
    cargo_lock
        .lines()
        .filter_map(|l| l.trim().strip_prefix("name = \"")?.strip_suffix('"'))
        .filter(|n| n.to_lowercase().contains("cua"))
        .map(|n| Leak::Crate {
            name: n.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// 本体あり（interface の commit で緑）: 正本の allowlist を読める・版の違う file と理由の空の行と知らない欄を拒む
    #[test]
    fn cua_allowlist_loads_and_refuses_other_versions() {
        let saved = Allowlist::load(&repo().join(ALLOWLIST_PATH)).unwrap();
        assert!(
            !saved.app_contents.is_empty(),
            "app_contents が空（一覧と一致を見られない）"
        );
        let dir = std::env::temp_dir().join(format!("w18-2-allow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, v: serde_json::Value| {
            let p = dir.join(name);
            std::fs::write(&p, v.to_string()).unwrap();
            p
        };
        let mut v = serde_json::to_value(&saved).unwrap();
        v["version"] = (ALLOWLIST_VERSION + 1).into();
        assert!(Allowlist::load(&write("ver.json", v))
            .unwrap_err()
            .contains("版"));
        let mut v = serde_json::to_value(&saved).unwrap();
        v["src_lines"] = serde_json::json!([{ "file": "src/x.rs", "contains": "cua", "kind": "comment", "why": " " }]);
        assert!(Allowlist::load(&write("why.json", v))
            .unwrap_err()
            .contains("why"));
        let mut v = serde_json::to_value(&saved).unwrap();
        v["extra"] = 1.into();
        assert!(Allowlist::load(&write("extra.json", v))
            .unwrap_err()
            .contains("形"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `rg -i cua src` に当たる行は allowlist の行だけ
    #[test]
    fn cua_src_mentions_only_allowlisted_lines() {
        let allow = Allowlist::load(&repo().join(ALLOWLIST_PATH)).unwrap();
        let hits = src_hits(&read_src(&repo()).unwrap(), &allow);
        assert!(hits.is_empty(), "allowlist に無い cua の行:\n{hits:#?}");
    }

    /// `cargo tree` に cua の crate が無い（Cargo.lock で見る）
    #[test]
    fn cua_crate_is_not_in_cargo_lock() {
        let lock = std::fs::read_to_string(repo().join("Cargo.lock")).unwrap();
        assert_eq!(lock_hits(&lock), vec![]);
    }

    /// 判定ごとに漏れを 1 つ植えると拾い、許した物は拾わない（負の対照を判定の側に持つ・holdout と共有）
    #[test]
    fn cua_judges_catch_planted_leaks() {
        let allow = Allowlist {
            version: ALLOWLIST_VERSION,
            src_lines: vec![SrcLine {
                file: "src/a.rs".into(),
                contains: "cua-driver が読む".into(),
                kind: Kind::Comment,
                why: "説明".into(),
            }],
            binary_strings: vec![],
            app_contents: vec![
                "Contents/Info.plist".into(),
                "Contents/MacOS/shibubu".into(),
            ],
        };
        let files = vec![(
            "src/a.rs".to_string(),
            "// cua-driver が読む\nlet x = \"CUA\";\n".to_string(),
        )];
        assert_eq!(
            src_hits(&files, &allow),
            vec![Leak::Src {
                file: "src/a.rs".into(),
                line: 2,
                text: "let x = \"CUA\";".into()
            }]
        );
        assert_eq!(
            binary_hits("b", b"\x00\x01run trycua now\x00cua\x00", &allow).len(),
            1,
            "素の cua は引かず trycua は引く"
        );
        assert_eq!(
            linked_hits(
                "b",
                "b:\n\t/usr/lib/libSystem.B.dylib\n\t@rpath/libcua.dylib\n"
            )
            .len(),
            1
        );
        let listing = vec![
            "Contents/Info.plist".to_string(),
            "Contents/MacOS/cua-driver".to_string(),
        ];
        let app = app_hits(&listing, &allow);
        assert!(app.contains(&Leak::AppNamed {
            path: "Contents/MacOS/cua-driver".into()
        }));
        assert!(app.contains(&Leak::AppExtra {
            path: "Contents/MacOS/cua-driver".into()
        }));
        assert!(app.contains(&Leak::AppMissing {
            path: "Contents/MacOS/shibubu".into()
        }));
        assert_eq!(
            lock_hits("[[package]]\nname = \"cua-sdk\"\nversion = \"1\"\n"),
            vec![Leak::Crate {
                name: "cua-sdk".into()
            }]
        );
    }

    /// 実機（release の .app）: `SHIBUBU_APP` に build した `.app` の絶対 path（例 `target/release/bundle/Shibubu.app`）。
    /// `Contents/MacOS/` の各 binary を `binary_hits`・`otool -L` を `linked_hits`・一覧を `app_hits` で見る。どれも空で緑。
    /// 罠の E2E の .app の側: `SHIBUBU_RUN` に `scripts/qa/bg/run.sh` の run の dir を渡すと、`run.json` の `trap_marks` と
    /// `cua_children_of_shibubu` が 0 である事も見る（null = stop していない = 未測で赤）。
    #[test]
    #[ignore = "実機: SHIBUBU_APP に release の .app を渡して回す（任意で SHIBUBU_RUN に run.sh の run の dir）"]
    fn cua_release_app_has_no_cua() {
        let app = PathBuf::from(std::env::var("SHIBUBU_APP").expect("SHIBUBU_APP が要ります"));
        let allow = Allowlist::load(&repo().join(ALLOWLIST_PATH)).unwrap();
        let listing = app_listing(&app).unwrap();
        let mut leaks = app_hits(&listing, &allow);
        let bins: Vec<&String> = listing
            .iter()
            .filter(|p| p.starts_with("Contents/MacOS/"))
            .collect();
        assert!(!bins.is_empty(), "Contents/MacOS/ に binary が無い（未測）");
        for rel in bins {
            let bin = app.join(rel);
            leaks.extend(binary_hits(rel, &std::fs::read(&bin).unwrap(), &allow));
            let o = std::process::Command::new("otool")
                .arg("-L")
                .arg(&bin)
                .output()
                .expect("otool を起こせません（未測）");
            // shell script（launcher）は Mach-O でないので otool は依存を出さない（1 行目だけ・依存 0）
            leaks.extend(linked_hits(rel, &String::from_utf8_lossy(&o.stdout)));
        }
        println!("見た file {} 本: {listing:#?}", listing.len());
        assert!(leaks.is_empty(), "撤去の漏れ:\n{leaks:#?}");
        if let Ok(run) = std::env::var("SHIBUBU_RUN") {
            let j: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(Path::new(&run).join("run.json")).unwrap(),
            )
            .unwrap();
            for k in ["trap_marks", "cua_children_of_shibubu"] {
                assert_eq!(
                    j[k].as_u64(),
                    Some(0),
                    "{run}/run.json の {k} = {}（null は stop していない＝未測）",
                    j[k]
                );
            }
        }
    }
}
