#!/usr/bin/env python3
"""汎用 Core と Adapter の境界の検査（owner 2026-10-21 18・19 通目。15 通目の「app 名の数が増えたら赤」を置き換えた）。

app 固有の能力そのものは禁止しない。Adapter（src/adapters/・extensions/）に置く app 専用の code（ReaScript・Blender の Python・
IDE の拡張・AX の手順など）は自由に足してよい。Core に app 名が出る事も一律には禁止しない: app の Identity の識別・
Capability Registry の照合・権限の判定・利用者が選んだ対象の検索は正当な共通処理（ZONES と baseline の legit）。
赤にするのは、汎用 Core（src/ のうち adapters/・bin/ を除く所と bridge/）に特定の app 専用の操作・業務処理が直に入る事:
  module  Core の中の app / service 専用の module（path に app の名前）
  branch  固定の app を前提にした分岐（app 名・bundle id の文字列との比較・match の腕・contains など）
  direct  Core が registry を通さず Adapter の中身を直に呼ぶ（`adapters::<module>`。`adapters::sdk` と型は可）
  swap    src/adapters/ の module が `Adapter` trait を実装していない（registry で交換・追加できない）
  table   app 名・bundle id の文字列と Rust の関数（`crate::…::f`）を組にした表の行（app ごとの決まった手を Core に持つ・
          ENGINE-GENERIC-1101 owner 211「appに合わせて機能を追加するのではなく」・例 `("com.x.app", "App", crate::audio::x::insert)`）
丸ごと試験の file（名前が `*_attack.rs`・頭に `#![cfg(test)]`）は Core の code でないので見ない（10-31 operator: WF-E の反証 wfe_attack.rs）。
`#[cfg(test)]` は次の項目（`mod x { … }`・`fn f() { … }`・`use …;`）だけを飛ばす（ENGINE-GENERIC-1101: 前は file の最初の `#[cfg(test)]` から
後ろを全部飛ばし、generated.rs は 2692 行目の試験の fn から後ろの 4000 行の製品の code が見えなかった）。
既存の候補は scripts/app-agnostic-baseline.tsv に場所ごとの数と分類（violation = Adapter へ分離・移行する / legit = 正当な共通処理と理由）で載せる。
violation は件数で消さず、要る機能を Adapter へ移した時に --update で基準を下げる。新しい違反・増えた違反は赤。

  python3 -I scripts/check_app_agnostic.py             # 検査（lint.sh から）
  python3 -I scripts/check_app_agnostic.py --update    # 基準を下げ直す（増えた時は書かない・分類と理由は残す）
  python3 -I scripts/check_app_agnostic.py --self-test # 負の対照
"""
import os
import re
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
BASE = os.path.join(ROOT, "scripts", "app-agnostic-baseline.tsv")
APPS = (r"textedit|reaper|blender|excalidraw|youtube|calculator|notion|slack|safari|chrome|vscode|figma|github|google|gmail|"
        r"apple_?music|spotify|discord|obsidian|ableton|photoshop|xcode")
APP = re.compile(rf"(?i)(?<![a-z])({APPS})(?![a-z])")
LIT = rf'"[^"\n]*?(?:(?i:(?<![a-z])(?:{APPS})(?![a-z]))|\b(?:com|org|io)\.[a-z0-9-]+\.[A-Za-z0-9]+)[^"\n]*"'
TABLE = re.compile(rf"\(\s*{LIT}\s*,[^()\n]*?(?:crate|super|self)::[A-Za-z0-9_:]*[a-z_][A-Za-z0-9_]*\s*\)")
BRANCH = re.compile(
    rf"{LIT}\s*(?:=>|\|(?!\|))|(?:==|!=)\s*&?{LIT}|{LIT}\s*(?:==|!=)|"
    rf"\.(?:contains|starts_with|ends_with|eq_ignore_ascii_case|eq)\(\s*&?{LIT}|matches!\([^)]*{LIT}")
DIRECT = re.compile(r"adapters::(?:\{([^}]*)\}|([a-z_][a-z0-9_]*))")
TEST_FILE = re.compile(r"(?m)^\s*#!\[cfg\(test\)\]")   # file の頭の inner attribute = 丸ごと試験
COMMENT = re.compile(r"^\s*(//|/\*|\*|#(?!\[))")
# 正当な共通処理の置き場（app 名の照合が仕事の module）。ここでの比較・照合は branch に数えない
ZONES = {
    "src/intel/": "Software Intelligence の Identity の識別と Registry の照合",
    "src/runtime/registry.rs": "Capability / Action Registry の照合",
    "src/runtime/permission.rs": "権限の判定",
}


def is_core(rel):
    p = rel.replace(os.sep, "/")
    if "/tests/" in p or p.startswith(("src/adapters/", "src/bin/", "extensions/")):
        return False
    return p.startswith(("src/", "bridge/"))


def in_zone(rel):
    p = rel.replace(os.sep, "/")
    return any(p.startswith(z) for z in ZONES)


def code_lines(text):
    lines, n = text.split("\n"), 0
    while n < len(lines):
        line = lines[n]
        if line.lstrip().startswith("#[cfg(test)]"):
            # 試験の項目だけ飛ばす: 次の属性でも空でもない行から、`{` が在れば括弧が閉じるまで・無ければその 1 行（`use …;`）
            # ponytail: 文字列や注の中の括弧も数える。ずれたら飛ばす所が長くなる（= 見落とす向き・負の対照で確かめる）
            k = next((k for k in range(n + 1, len(lines)) if lines[k].strip() and not lines[k].lstrip().startswith("#[")), len(lines))
            depth, e = 0, k
            for e in range(k, len(lines)):
                depth += lines[e].count("{") - lines[e].count("}")
                if depth <= 0 and ("{" in "".join(lines[k:e + 1]) or lines[e].rstrip().endswith(";")):
                    break
            n = e + 1
            continue
        if not COMMENT.match(line):
            yield line.split("//")[0] if '"' not in line else line
        n += 1


def violations(rel, text):
    out = {}
    def add(k):
        out[k] = out.get(k, 0) + 1
    if not is_core(rel) or rel.endswith("_attack.rs") or TEST_FILE.search(text):
        return out                                  # Adapter・bin・丸ごと試験の file（*_attack.rs・#![cfg(test)]）は見ない
    m = APP.search(os.path.basename(rel)) or APP.search(os.path.dirname(rel.replace(os.sep, "/")).split("src/", 1)[-1])
    if m:
        add(("module", rel, m.group(1).lower()))
    for line in code_lines(text):
        if not in_zone(rel):
            for b in BRANCH.finditer(line):
                a = APP.search(b.group(0))
                add(("branch", rel, a.group(1).lower() if a else "bundle-id"))
        for t in TABLE.finditer(line):
            a = APP.search(t.group(0))
            add(("table", rel, a.group(1).lower() if a else "bundle-id"))
        for d in DIRECT.finditer(line):
            for name in re.split(r"[,\s]+", d.group(1) or d.group(2) or ""):
                name = name.split("::")[0].strip()
                if name and name[0].islower() and name not in ("sdk", "self"):
                    add(("direct", rel, name))
    return out


def swap_violations(root):
    """src/adapters/mod.rs の `pub mod x;` ごとに、x が `Adapter` を実装しているか（registry で交換・追加できるか）。"""
    out = {}
    mod = os.path.join(root, "src", "adapters", "mod.rs")
    if not os.path.exists(mod):
        return out
    for name in re.findall(r"^\s*pub mod ([a-z_][a-z0-9_]*);", open(mod, encoding="utf-8").read(), re.M):
        if name == "sdk":
            continue
        for p in (os.path.join(root, "src", "adapters", name + ".rs"), os.path.join(root, "src", "adapters", name, "mod.rs")):
            if os.path.exists(p):
                if not re.search(r"impl(?:<[^>]*>)?\s+(?:sdk::)?Adapter\s+for\b", open(p, encoding="utf-8").read()):
                    out[("swap", os.path.relpath(p, root), name)] = 1
                break
    return out


def scan(root):
    out = swap_violations(root)
    for top in ("src", "bridge", "extensions"):
        for d, _, fs in os.walk(os.path.join(root, top)):
            for f in fs:
                if f.endswith((".rs", ".swift")):
                    p = os.path.join(d, f)
                    rel = os.path.relpath(p, root)
                    for k, n in violations(rel, open(p, encoding="utf-8", errors="replace").read()).items():
                        out[k] = out.get(k, 0) + n
    return out


def load(path):
    """baseline: check・file・token・数・分類（violation / legit）・理由。"""
    base = {}
    try:
        for l in open(path, encoding="utf-8"):
            r = l.rstrip("\n").split("\t")
            if len(r) >= 4 and r[3].isdigit():
                base[(r[0], r[1], r[2])] = (int(r[3]), r[4] if len(r) > 4 else "violation", r[5] if len(r) > 5 else "")
    except FileNotFoundError:
        pass
    return base


def check(now, base):
    bad = []
    for k, n in sorted(now.items()):
        cnt, cls, _ = base.get(k, (0, "violation", ""))
        if cls == "legit":
            continue                                 # 正当な共通処理と分類し理由を書いた物は数で縛らない
        if n > cnt:
            bad.append(f"{k[0]} {k[1]} {k[2]}: {n} 件（基準 {cnt}）")
    return bad


def self_test():
    v = lambda rel, s: violations(rel, s)
    assert v("src/ui/x.rs", 'match app { "reaper" => run(), _ => {} }'), "Core の match の腕で赤くならない"
    assert v("src/ui/x.rs", 'if name == "TextEdit" { f() }'), "Core の比較で赤くならない"
    assert v("src/ui/x.rs", 'if id.contains("com.cockos.reaper") {}'), "Core の bundle id の contains で赤くならない"
    assert v("src/ui/x.rs", 'use crate::adapters::github::Client;'), "Core が adapter を直に呼んで赤くならない"
    assert v("src/ui/blender_panel.rs", "fn f() {}"), "Core の app 専用の module で赤くならない"
    assert not v("src/adapters/reaper.rs", 'match app { "reaper" => run(), _ => {} }'), "Adapter の中の app 専用の code を赤にした"
    assert not v("src/adapters/blender/mod.rs", 'if name == "Blender" {}'), "Adapter の中の app 専用の code を赤にした"
    assert not v("src/ui/x.rs", 'use crate::adapters::sdk::Adapter; use super::adapters::{AdapterRegistry, sdk};'), "registry と sdk を赤にした"
    assert not v("src/ui/x.rs", 'let label = "TextEdit で開く";'), "表示の文言を分岐と読んだ"
    assert not v("src/ui/x.rs", '// if name == "TextEdit"'), "comment を読んだ"
    assert not v("src/ui/x.rs", 'fn f() {}\n#[cfg(test)]\nmod t { fn g() { assert!(a == "REAPER"); } }'), "試験の module を読んだ"
    assert not v("src/ui/x.rs", '#[cfg(test)]\nfn g() {\n    assert!(a == "REAPER");\n}\n'), "試験の fn を読んだ"
    # ENGINE-GENERIC-1101: 試験の項目の後ろの製品の code は見る（前は最初の #[cfg(test)] から後ろを全部飛ばした）
    after = '#[cfg(test)]\nfn g() {\n    let x = 1;\n}\n#[cfg(test)]\nuse a::b;\nfn h() { if name == "TextEdit" { f() } }'
    assert v("src/ui/x.rs", after), "試験の fn の後ろの製品の分岐を見落とした"
    # ENGINE-GENERIC-1101: app と関数を組にした表（app ごとの決まった手）
    table = 'pub const TAKE_APPS: [(&str, &str, Insert); 1] = [("com.cockos.reaper", "REAPER", crate::audio::reaper::insert)];'
    assert v("src/generated.rs", table) == {("table", "src/generated.rs", "reaper"): 1}, f"app と関数の表を拾わない: {v('src/generated.rs', table)}"
    assert v("src/ui/x.rs", 'const T: &[(&str, F)] = &[("Blender", self::run)];'), "app 名と関数の表を拾わない"
    assert not v("src/adapters/reaper.rs", table), "Adapter の中の表を赤にした"
    assert not v("src/ui/x.rs", 'let t = [("recorder", "recorded")];'), "部品の手の表（app 名なし）を赤にした"
    assert not v("src/ui/x.rs", 'let r = ("com.cockos.reaper", "REAPER");'), "関数を持たない組（表示の名前）を表と読んだ"
    assert not v("src/capability_engine/wfe_attack.rs", 'if name == "TextEdit" { f() }'), "丸ごと試験の *_attack.rs を読んだ"
    assert not v("src/ui/reaper_t.rs", '//! 試験\n#![cfg(test)]\nif name == "TextEdit" { f() }'), "頭に #![cfg(test)] の file を読んだ"
    assert v("src/ui/attack_plan.rs", 'if name == "TextEdit" { f() }'), "名前に attack を含むだけの Core の file を外した"
    assert not v("src/intel/identity.rs", 'if bundle == "com.cockos.reaper" { kind = Daw }'), "Identity の識別（ZONES）を赤にした"
    assert not v("src/runtime/permission.rs", 'if app == "TextEdit" { scope() }'), "権限の判定（ZONES）を赤にした"
    k = ("branch", "src/x.rs", "reaper")
    assert check({k: 2}, {k: (2, "violation", "")}) == []
    assert check({k: 3}, {k: (2, "violation", "")}), "増えたのに赤くならない"
    assert check({k: 9}, {k: (2, "legit", "sign-in の判定")}) == [], "legit に分類した物を数で縛った"
    import tempfile
    d = tempfile.mkdtemp()
    os.makedirs(os.path.join(d, "src", "adapters"))
    open(os.path.join(d, "src", "adapters", "mod.rs"), "w").write("pub mod sdk;\npub mod good;\npub mod bad;\n")
    open(os.path.join(d, "src", "adapters", "good.rs"), "w").write("impl Adapter for Good {}\n")
    open(os.path.join(d, "src", "adapters", "bad.rs"), "w").write("pub fn run() {}\n")
    sw = swap_violations(d)
    assert ("swap", os.path.join("src", "adapters", "bad.rs"), "bad") in sw and len(sw) == 1, f"Adapter を実装しない module を拾わない: {sw}"
    print("check_app_agnostic: self-test ok")


def main():
    if "--self-test" in sys.argv:
        return self_test()
    now = scan(ROOT)
    base = load(BASE)
    if "--update" in sys.argv:
        grew = check(now, base) if base else []
        if grew:
            sys.exit("check_app_agnostic: 違反が増えたので基準は書かない（" + "; ".join(grew) + "）。Adapter に置くか、正当な共通処理なら baseline に legit と理由を手で書く")
        rows = []
        for k, n in sorted(now.items()):
            _, cls, why = base.get(k, (0, "violation", ""))
            rows.append(f"{k[0]}\t{k[1]}\t{k[2]}\t{n}\t{cls}\t{why}\n")
        with open(BASE, "w", encoding="utf-8") as f:
            f.write("".join(rows))
        return print(f"check_app_agnostic: 基準を書いた（{len(rows)} 行）")
    bad = check(now, base)
    if bad:
        print("check_app_agnostic: 汎用 Core に app 専用の処理が増えた（owner 18・19 通目・YC_WAVES §1「汎用 Core と Adapter」）。"
              "app 固有の能力は src/adapters/ か extensions/ に置き、Core は registry を通して呼ぶ。正当な共通処理（Identity・Registry の照合・権限・検索）なら baseline に legit と理由を書く:\n  "
              + "\n  ".join(bad))
        sys.exit(1)
    v = sum(n for k, n in now.items() if base.get(k, (0, "violation", ""))[1] != "legit")
    l = sum(n for k, n in now.items() if base.get(k, (0, "violation", ""))[1] == "legit")
    print(f"check_app_agnostic: ok（Adapter へ移す違反 {v} 件・正当な共通処理 {l} 件・増えていない）")


if __name__ == "__main__":
    main()
