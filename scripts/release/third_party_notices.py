#!/usr/bin/env python3
"""release の門（RULE-OSS・YC_DECISIONS 86 通目）: 依存の license と vendor/ の LICENSE を検査し、THIRD-PARTY-NOTICES を書く。
  python3 -I scripts/release/third_party_notices.py --check                       # lint.sh: license と vendor/ の検査だけ
  python3 -I scripts/release/third_party_notices.py --out <Contents/Resources/THIRD-PARTY-NOTICES.txt>   # build-mac.sh: 検査 + 書き出し
  python3 -I scripts/release/third_party_notices.py --self-test                   # 負の対照（偽の GPL・LICENSE 無しの vendor）
--metadata FILE で cargo metadata の出力を渡せる（無ければ cargo metadata --locked --offline）。allow は deny.toml の [licenses].allow。
赤 = exit 1 で「crate 名 版: 理由」を 1 行ずつ。crate に license の file が無い時は名前・版・式だけ載せる（本文を作らない）。"""
import json, os, re, subprocess, sys, tempfile, tomllib

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
LIC_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense)", re.I)


def load_allow(path):
    with open(path, "rb") as f:
        return set(tomllib.load(f)["licenses"]["allow"])


def allowed(expr, allow):
    """SPDX 式の評価。AND は両側・OR は片側が許されればよい。WITH は例外を無視して本体で見る。"""
    toks = re.findall(r"\(|\)|[^\s()/]+|/", expr)
    toks = ["OR" if t == "/" else t for t in toks]
    pos = 0

    def atom():
        nonlocal pos
        t = toks[pos]; pos += 1
        if t == "(":
            v = or_(); pos += 1  # ")"
            return v
        ok = t in allow or t.rstrip("+") in allow
        if pos < len(toks) and toks[pos] == "WITH":
            pos += 2
        return ok

    def and_():
        nonlocal pos
        v = atom()
        while pos < len(toks) and toks[pos] == "AND":
            pos += 1; v = atom() and v
        return v

    def or_():
        nonlocal pos
        v = and_()
        while pos < len(toks) and toks[pos] == "OR":
            pos += 1; v = and_() or v
        return v

    return or_() if toks else False


def third_party(meta):
    own = set(meta["workspace_members"])
    return sorted((p for p in meta["packages"] if p["id"] not in own), key=lambda p: (p["name"], p["version"]))


def check(meta, allow, vendor_dir):
    bad = []
    for p in third_party(meta):
        lic = p.get("license")
        if not lic:
            bad.append(f"{p['name']} {p['version']}: license が無い")
        elif not allowed(lic, allow):
            bad.append(f"{p['name']} {p['version']}: license {lic} は許す物に無い（deny.toml）")
    if os.path.isdir(vendor_dir):
        for d in sorted(os.listdir(vendor_dir)):
            full = os.path.join(vendor_dir, d)
            if os.path.isdir(full) and not any(LIC_FILE.match(n) and not n.lower().startswith("notice") for n in os.listdir(full)):
                bad.append(f"vendor/{d}: LICENSE* / COPYING* が無い")
    return bad


def license_texts(d):
    out = []
    for n in sorted(os.listdir(d)) if os.path.isdir(d) else []:
        fp = os.path.join(d, n)
        if LIC_FILE.match(n) and os.path.isfile(fp):
            out.append(f"\n--- {n} ---\n" + open(fp, encoding="utf-8", errors="replace").read().rstrip())
    return out


def notices(meta, vendor_dir=None, assets_dir=None):
    out = ["Shibubu includes the third-party software below. Names, versions and licenses come from each crate's Cargo.toml;",
           "the license texts are copied from the files shipped in each crate.\n"]
    for p in third_party(meta):
        out.append("=" * 72)
        out.append(f"{p['name']} {p['version']}  —  {p.get('license')}")
        out.extend(license_texts(os.path.dirname(p["manifest_path"])))
        out.append("")
    # TERM-XTERM-1101: crate でない vendor（vendor/xterm = xterm.js・binary に入れる JS）。名前と版は dir の PACKAGES.md の表・本文は LICENSE*
    # PROVIDER-ICONS-1101: binary に入れる他人の絵（assets/<dir> で LICENSE* を持つ物 = assets/provider-logos の LobeHub の logo）も同じ形（表は README.md）
    ls = lambda top: sorted(os.listdir(top)) if top and os.path.isdir(top) else []
    extra = [(f"vendor/{d}", os.path.join(vendor_dir, d)) for d in ls(vendor_dir)
             if os.path.isdir(os.path.join(vendor_dir, d)) and not os.path.isfile(os.path.join(vendor_dir, d, "Cargo.toml"))]
    extra += [(f"assets/{d}", os.path.join(assets_dir, d)) for d in ls(assets_dir) if license_texts(os.path.join(assets_dir, d))]
    for name, full in extra:
        out.append("=" * 72)
        pk = next((p for p in (os.path.join(full, n) for n in ("PACKAGES.md", "README.md")) if os.path.isfile(p)), None)
        rows = [l for l in open(pk, encoding="utf-8").read().splitlines() if l.startswith("| @")] if pk else []
        out.append(name + "".join(f"\n  {r.split('|')[1].strip()} {r.split('|')[2].strip()}  —  {r.split('|')[4].strip()}" for r in rows))
        out.extend(license_texts(full))
        out.append("")
    return "\n".join(out)


def self_test():
    allow = {"MIT", "Apache-2.0"}
    for e, ok in [("MIT", 1), ("MIT OR Apache-2.0", 1), ("MIT/Apache-2.0", 1), ("GPL-3.0", 0), ("MIT OR GPL-3.0", 1),
                  ("MIT AND GPL-3.0", 0), ("(MIT OR GPL-3.0) AND Apache-2.0", 1), ("Apache-2.0 WITH LLVM-exception", 1), ("AGPL-3.0-only", 0)]:
        assert allowed(e, allow) == bool(ok), e
    with tempfile.TemporaryDirectory() as t:
        cd = os.path.join(t, "crate"); os.makedirs(cd)
        open(os.path.join(cd, "LICENSE-MIT"), "w").write("MIT TEXT")
        pk = lambda n, lic, i: {"id": i, "name": n, "version": "1.0.0", "license": lic, "manifest_path": os.path.join(cd, "Cargo.toml")}
        meta = {"workspace_members": ["me"], "packages": [pk("me", "GPL-3.0", "me"), pk("good", "MIT", "g")]}
        assert check(meta, allow, os.path.join(t, "no-vendor")) == [], "自分の crate は license を見ない"
        assert "MIT TEXT" in notices(meta) and "me 1.0.0" not in notices(meta)
        js = os.path.join(t, "vjs"); os.makedirs(os.path.join(js, "xterm"))
        open(os.path.join(js, "xterm", "LICENSE"), "w").write("XTERM MIT")
        open(os.path.join(js, "xterm", "PACKAGES.md"), "w").write("| @xterm/xterm | 6.0.0 | 2025-12-22 | MIT | x |\n")
        n = notices(meta, js)
        assert "XTERM MIT" in n and "@xterm/xterm 6.0.0" in n, "crate でない vendor の license が notices に無い"
        a = os.path.join(t, "assets"); os.makedirs(os.path.join(a, "logos")); os.makedirs(os.path.join(a, "own"))
        open(os.path.join(a, "logos", "LICENSE"), "w").write("LOBE MIT")
        open(os.path.join(a, "logos", "README.md"), "w").write("| @lobehub/icons-static-svg | 1.95.1 | 14 | MIT | x |\n")
        open(os.path.join(a, "own", "x.png"), "w").write("x")
        n = notices(meta, None, a)
        assert "LOBE MIT" in n and "@lobehub/icons-static-svg 1.95.1" in n and "assets/own" not in n, "binary に入れる絵の license が notices に無い"
        meta["packages"].append(pk("evil", "GPL-3.0", "e")); meta["packages"].append(pk("nolic", None, "n"))
        r = check(meta, allow, os.path.join(t, "no-vendor"))
        assert len(r) == 2 and any("evil" in x for x in r) and any("nolic" in x for x in r), r
        v = os.path.join(t, "vendor"); os.makedirs(os.path.join(v, "a")); os.makedirs(os.path.join(v, "b"))
        open(os.path.join(v, "a", "LICENSE-MIT"), "w").write("x")
        open(os.path.join(v, "b", "NOTICE"), "w").write("x")  # NOTICE だけでは足りない
        assert check({"workspace_members": [], "packages": []}, allow, v) == ["vendor/b: LICENSE* / COPYING* が無い"]
    print("third_party_notices: self-test ok")


def main(argv):
    if "--self-test" in argv:
        return self_test()
    opt = lambda k, d=None: argv[argv.index(k) + 1] if k in argv else d
    mf = opt("--metadata")
    if mf:
        meta = json.load(open(mf))
    else:
        meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
                                         cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    bad = check(meta, load_allow(os.path.join(ROOT, "deny.toml")), os.path.join(ROOT, "vendor"))
    if bad:
        print("third_party_notices: 赤\n" + "\n".join("  " + b for b in bad)); sys.exit(1)
    out = opt("--out")
    if out:
        os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
        open(out, "w", encoding="utf-8").write(notices(meta, os.path.join(ROOT, "vendor"), os.path.join(ROOT, "assets")))
    print(f"third_party_notices: ok（{len(third_party(meta))} 本）" + (f" → {os.path.basename(out)}" if out else ""))


if __name__ == "__main__":
    main(sys.argv[1:])
