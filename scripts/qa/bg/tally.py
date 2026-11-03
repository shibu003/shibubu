#!/usr/bin/env python3
"""tally.py — run の記録から YC_ACCEPTANCE §5 の数を数える（試験の側・画面には出さない）。

呼ぶ側の使い方:
    python3 -I scripts/qa/bg/tally.py <run-dir> > docs/evidence/<W>/tally.json
    exit: 0 数えた（合否は中の値で見る）/ 2 使い方 / 4 知らない schema・壊れた記録

読む物: <run-dir>/run.json（shibubu.qa.run/1）・steps.jsonl（shibubu.qa.step/1）・steps/*.verdict.json（shibubu.qa.verdict/1）
        ・oracle/*.json（shibubu.qa.windows/1・shibubu.qa.github/1・shibubu.qa.uistate/1・shibubu.qa.a1/1・shibubu.qa.git/1）・ax/*.json（cua-driver get_window_state の木）
        ・steps/*.axprobe.json（shibubu.qa.axprobe/1・W12W-1c の cause_of が読む）・load.jsonl（shibubu.qa.load/1・W12W-1c の場の自分の負荷の窓・無ければ空）。
保証する: *_verified は oracle の JSON からだけ数える（製品の自己申告・画面の文字では数えない）。知らない schema は読まず exit 4。
保証しない: L1 の ①（surface-status:* の identifier）は W2c で identifier が決まるまで数えない（lies に入るのは ② だけ・その事を unmeasured に書く）。
"""
from __future__ import annotations

import glob
import json
import os
import re
import sqlite3
import sys
from datetime import datetime
from dataclasses import asdict, dataclass, field, fields

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import owner_idle as OI  # 在席の信号は 1 か所（W15-b42）
import labels as L  # app の文は表の key で引く（W15-b36）

SCHEMA = "shibubu.qa.tally/1"

# L1 の ② の成功の語の表（YC_ACCEPTANCE §4）。
SUCCESS_WORDS = L.names("connected") + L.names("saved_msg") + L.names("finished") + ("保存済み", "済み", "✓")  # 「済み」は 接続済み・保存済み を含む語尾 → Lie.word は当たった語のうち一番長い物
NOT_CHECKED = "Saved, not checked"  # docs/ui-copy/copy.en.md の読み戻していない保存の語（find_lies が SUCCESS_WORDS を探す前に外す）
# L1 の ② で引かない部分木の identifier の頭（Shibubu 自身の表示でない所・src/ui/ax.rs の `AxScope::excluded_from_l1` と同じ 5 つ）。
# legend: は状態の語の説明（Connections の凡例「Connected = 認証済み」）で、何かが済んだと言う行ではない（W19-1・operator 2026-10-23）。
EXCLUDED_PREFIXES = ("terminal:", "webview:", "surface-frame:", "data:", "legend:")


@dataclass
class Lie:
    """L1 の 1 件。step は手の名前、element は AX の identifier か role+label、word は引いた語。"""
    step: str
    element: str
    word: str


@dataclass
class DeadClick:
    """L2 の 1 件。expect は宣言した期待する効果。"""
    step: str
    expect: str


@dataclass
class Expect:
    """手の「期待する効果」（L2・run.sh step --expect "<kind>:<target>"）。
    kind: ax=target の element（identifier か role:label・"*" は窓の木ぜんぶ）の部分木の文字が変わる / ui_state=oracle/ui-state で target（needle）が before 無し → after 在り
          / oracle=R/oracle/<step>.<target>.{before,after}.json の t 以外が変わる / refused=拒否の 1 行が出る（target は空でよい・例: AXValue の set の Unsupported）。"""
    kind: str
    target: str


EXPECT_KINDS = ("ax", "ui_state", "oracle", "refused")
# L2 で変化に数えない node（時刻・spinner・hover・進捗）の role。
VOLATILE_ROLES = ("AXProgressIndicator", "AXBusyIndicator", "AXHelpTag")


@dataclass
class Claim:
    """画面が「接続済み」と出している 1 件（claimed_connected の 1）。surface_id と kind は W2c の surface-status:<surface_id> から。それまで None。"""
    surface_id: str | None
    kind: str | None  # NativeApp | Web | Terminal（SurfaceReceipt の kind）
    element: str


# ---- W12W-1c: 1 手の切り分け（製品の欠陥 / 環境 / 未測・owner 13 通目）----

@dataclass(frozen=True)
class Thresholds:
    """混雑の閾値。owner 13 通目「閾値は CPU 数に固定しない」→ load1 と ncpu は numbers に記録するだけで判定に使わない。
    使い方: cause_of(step, probe, cua, Thresholds(ax_slow_ms=1500))。値は W12W-1c の切り分けの実験で決める（今は暫定）。"""
    # AX の 1 属性の応答（axprobe の elapsed_ms）がこれ以上 = 混雑
    ax_slow_ms: int = 2000
    # axprobe の pressure.memory_pressure がこれのどれか = 混雑
    pressure_red: tuple[str, ...] = ("warn", "critical")


# product.kind ごとに取り得る reason（record.schema.json の cause が同じ値を持つ）。
PRODUCT_REASONS: dict[str, tuple[str, ...]] = {
    "ok": ("reached", "answered"),
    "defect": ("elements_missing",),
    "environment": ("not_trusted", "off_space", "ax_timeout", "congested", "ax_empty"),
    "unmeasured": ("no_probe", "no_cg_window", "walk_incomplete", "other"),
}
DRIVER_KINDS = ("resolved", "unresolved", "unmeasured")


@dataclass
class ProductCause:
    """製品の判定。kind は PRODUCT_REASONS の鍵・reason はその値の 1 つ・numbers は決めた根拠の数値（ax の elapsed_ms の最大・load1・ncpu・memory_pressure・swap_used_mb）。"""
    kind: str
    reason: str
    numbers: dict


@dataclass
class DriverCause:
    """cua-driver の検証の判定（製品の判定と独立: cua-driver で通らない事を製品の欠陥と書かない・逆も）。
    kind は DRIVER_KINDS の 1 つ。reason は unresolved の時 cua-driver の refusal の code（ax_window_unresolved など）・unmeasured の時は応答が無い / 読めない理由。"""
    kind: str
    reason: str | None
    element_count: int | None


@dataclass
class Cause:
    """1 手の切り分け（record.schema.json の cause）。operable は AX で want の要素に届いた時だけ True（CG に窓が在るだけでは False）。"""
    step: str
    product: ProductCause
    driver: DriverCause
    operable: bool


def cause_of(step: str, probe: dict | None, cua: dict | str | None, th: Thresholds = Thresholds()) -> Cause:
    """1 手を製品の欠陥 / 環境 / 未測に分ける（純関数・docs/diagrams.md §30 の順）。
    使い方: count() が steps.jsonl の手ごとに cause_of(st["step"], <steps/<name>.axprobe.json>, st["cua"]) を呼び、tally の causes に並べる。
    product の順: probe 無し → unmeasured/no_probe ・ want に AX で届いた → ok/reached ・ 許可無し → environment/not_trusted
      ・ CG に窓（layer 0・50pt 以上）が無い → unmeasured/no_cg_window ・ どれも off screen → environment/off_space
      ・ 3 属性のどれかが timeout → environment/ax_timeout ・ 応答が th.ax_slow_ms 以上か memory_pressure が th.pressure_red → environment/congested
      ・ 3 属性とも empty → environment/ax_empty ・ 答えたのに want が木を全部見ても無い → defect/elements_missing
      ・ want 無しで答えた → ok/answered ・ 木を全部見ていない → unmeasured/walk_incomplete ・ それ以外 → unmeasured/other。
    driver は cua（その手の cua-driver の stdout の JSON・st["cua"]。JSON でなければ原文の str・st["cua_raw"]）だけから（driver_of）: None → unmeasured ・ refusal / degraded（ax_window_unresolved）
      → unresolved(code) ・ elements → resolved(数)。
    保証する: AX が 0 だけで defect にしない（負の対照: 「AX 0 = 欠陥」に戻すと環境の run が赤）・driver から product を決めない・CG だけで operable にしない。
    probe の want・pressure.swap_used_mb・pressure.hid_idle_s は無い時は鍵ごと無い（probe.get で読む・null も同じ扱い）。
    保証しない: 閾値が正しい事（実験で決める）・Space の種類（probe の space は numbers に写すだけ）。"""
    driver = driver_of(cua)
    if not probe:
        return Cause(step, ProductCause("unmeasured", "no_probe", {}), driver, False)
    atts = {k: probe["ax"][k] for k in ("windows", "main_window", "focused_window")}
    pr = probe.get("pressure") or {}
    want = probe.get("want") or None
    cg = [w for w in probe.get("cg") or [] if w.get("layer") == 0 and w["bounds"]["w"] >= 50 and w["bounds"]["h"] >= 50]
    nums = {"ax": {k: f"{a['outcome']}/{a['ax_code']}" for k, a in atts.items()}, "ax_ms_max": max(a["elapsed_ms"] for a in atts.values()),
            "load1": pr.get("load1"), "ncpu": pr.get("ncpu"), "memory_pressure": pr.get("memory_pressure"), "swap_used_mb": pr.get("swap_used_mb"),
            "space": probe.get("space"), "pri": probe.get("pri"), "cg_windows": len(cg), "cg_onscreen": sum(1 for w in cg if w.get("onscreen"))}
    if want:
        nums["want"] = {k: want.get(k) for k in ("identifier", "found", "visited", "complete", "elapsed_ms")}

    def product(kind: str, reason: str, operable: bool = False) -> Cause:
        return Cause(step, ProductCause(kind, reason, nums), driver, operable)

    outcomes = [a["outcome"] for a in atts.values()]
    if want and want.get("found"):
        return product("ok", "reached", True)
    if not probe.get("trusted") or "not_trusted" in outcomes:
        return product("environment", "not_trusted")
    if not cg:
        return product("unmeasured", "no_cg_window")
    if not nums["cg_onscreen"]:
        return product("environment", "off_space")
    if "timeout" in outcomes:
        return product("environment", "ax_timeout")
    if nums["ax_ms_max"] >= th.ax_slow_ms or pr.get("memory_pressure") in th.pressure_red:
        return product("environment", "congested")
    if all(o == "empty" for o in outcomes):
        return product("environment", "ax_empty")
    # 欠陥と言えるのはここだけ: CG に窓が在り（on screen）・AX が答え・want の木を全部見ても無い
    if "answered" in outcomes and want and want.get("complete"):
        return product("defect", "elements_missing")
    if "answered" in outcomes and not want:
        return product("ok", "answered")
    if "answered" in outcomes and want:
        return product("unmeasured", "walk_incomplete")
    return product("unmeasured", "other")


def driver_of(cua: dict | str | None) -> DriverCause:
    """その手の cua-driver の応答（load_run の st["cua"]）だけから cua-driver の判定を出す。製品の判定は見ない。
    unresolved の印は 4 つ: refusal の code・element_token の手の `effect: refused` と `code`（off_space_or_ax_unresolved）・`degraded_reason` の頭（get_window_state が窓の AX を解けない時は refusal でなく
    `degraded: true`・`degraded_reason: "ax_window_unresolved: …"`・elements が空で返る）・`background_input.exact_window.status`（ax_unresolved）。
    resolved: elements が 1 つ以上（要素の数）・element_token の手が通った（effect か ✅ の summary・要素の数は無い）。それ以外は unmeasured。"""
    if isinstance(cua, str):  # JSON でない応答の原文（例: "AX action failed: AXUIElementPerformAction(AXPress) returned -25204"）
        m = re.search(r"returned (-\d+)", cua)
        return DriverCause("unresolved", f"ax_action_failed:{m.group(1)}", None) if m else DriverCause("unmeasured", "JSON でない: " + cua.strip()[:60], None)
    if not isinstance(cua, dict):
        return DriverCause("unmeasured", "cua-driver の応答が無い", None)
    ref = cua.get("refusal")
    if ref or "refused" in (cua.get("status"), cua.get("effect")):
        return DriverCause("unresolved", (ref.get("code") if isinstance(ref, dict) else None) or cua.get("code") or "refused", None)
    exact = ((cua.get("background_input") or {}).get("exact_window") or {}).get("status")
    if cua.get("degraded_reason") or exact == "ax_unresolved":
        return DriverCause("unresolved", str(cua.get("degraded_reason") or exact).split(":", 1)[0], len(cua.get("elements") or []))
    els = cua.get("elements")
    if isinstance(els, list) and els:
        # 窓は解けたが木を読み切っていない（walk の timeout）は reason に残す
        return DriverCause("resolved", None if cua.get("elements_complete", True) else "elements_incomplete", len(els))
    if cua.get("effect") in ("confirmed", "unverifiable") or str(cua.get("summary", "")).startswith("✅"):
        return DriverCause("resolved", None, None)
    return DriverCause("unmeasured", "elements も refusal も無い", None)


# W12W-1c の場（run.json の scene）→ その場で手の axprobe が読むはずの space。同じ手を場ごとに回して causes を比べる。
SCENE_SPACE: dict[str, str] = {"normal_low": "normal", "normal_load": "normal", "fullscreen": "fullscreen", "split_view": "split_view"}


def scene_held(scene: str | None, probes: list[dict | None], loads: list[dict]) -> bool | None:
    """宣言した場（run.json の scene）が、手の axprobe の space と run.sh load の窓（load.jsonl の行）に合うか。純関数。
    使い方: count() が scene_held(run.get("scene"), [st["axprobe_doc"] for st in steps], r["loads"]) を tally の scene_held に入れ、operator は True の run だけを
      その場の判定として log に並べる（False・None の run は場の比べに数えない）。
    True: probe の在る手が 1 つ以上・その全部の space が SCENE_SPACE[scene]・normal_low は probe の在る手のどれの t も窓（t0〜t1）の外
      ・normal_load は probe の在る手の全部の t がどれかの窓の中（1 手だけ重なった run を負荷の場と呼ばない）・fullscreen / split_view は窓を見ない。
    False: space が違う手が在る・load の条件が合わない（normal_low で窓の中の手・normal_load で窓の外の手）。None: scene が None か知らない値・probe の在る手が 0・space が unknown の手が在る（合うとも合わないとも言わない）。
    順: scene → probe の在る手 → unknown（None）→ space の違い → load の窓（False）→ True。窓は両端を含む（t0 ≤ t ≤ t1）。
    保証しない: Space の種類が正しい事（axprobe の推定）・負荷が実際に効いた事（load1 と AX の応答は cause の numbers で見る）。"""
    want = SCENE_SPACE.get(scene) if scene else None
    ps = [p for p in probes if p]
    if want is None or not ps:
        return None
    spaces = [p.get("space") or "unknown" for p in ps]
    if "unknown" in spaces:
        return None
    if any(sp != want for sp in spaces):
        return False
    inside = [in_load(p["t"], loads) for p in ps]
    if scene == "normal_low":
        return not any(inside)
    if scene == "normal_load":
        return all(inside)
    return True


def in_load(t: float, loads: list[dict]) -> bool:
    """時刻 t が run.sh load の窓（load.jsonl の t0〜t1・両端を含む）のどれかの中か。scene_held と count の numbers["self_load"] が使う。"""
    return any(ld["t0"] <= t <= ld["t1"] for ld in loads)


READ_TOOLS = ("get_window_state", "list_windows", "ax_tree")  # 画面を変えない手（run.sh の element_token 不要の 3 つ・ax_tree は front-guard ax）


def parse_expect(s: str) -> Expect:
    """`--expect` の文字を読む。kind が EXPECT_KINDS に無い・target が要るのに空なら ValueError（tally は exit 4）。
    使い方: parse_expect("ui_state:qaneedle5821")。保証しない: target の element が木に在るか（effect_seen が見る）。"""
    kind, sep, target = s.partition(":")
    if not sep or kind not in EXPECT_KINDS:
        raise ValueError(f"--expect の kind が知らない物: {s!r}（{'|'.join(EXPECT_KINDS)}:<target>）")
    if kind != "refused" and not target:
        raise ValueError(f"--expect {kind}: に target が無い")
    return Expect(kind, target)


def effect_seen(st: dict, expect: Expect, before_ax, after_ax, oracle: dict[str, dict]) -> bool:
    """1 手の期待する効果が前後で起きたか。VOLATILE_ROLES の node の変化は数えない。st は steps.jsonl の 1 行（cua を含む）。
    使い方: find_dead_clicks が expect を持つ手ごとに呼ぶ。保証する: oracle の before / after の片方でも無ければ False（測れない手を緑にしない）。"""
    if expect.kind == "ax":
        b, a = _subtree_texts(before_ax, expect.target), _subtree_texts(after_ax, expect.target)
        # after の木に target が無い（AX が答えない・空の木）は測れない手 → 効果無し。before に無く after に出たのは効果。
        return a is not None and b != a
    if expect.kind in ("ui_state", "oracle"):
        kind = "uistate" if expect.kind == "ui_state" else expect.target
        b, a = (oracle.get(f"{st['step']}.{w}.{kind}.json") for w in ("before", "after"))
        if b is None or a is None:
            return False
        if expect.kind == "ui_state":
            return not b["has_needle"] and a["has_needle"]
        return {k: v for k, v in b.items() if k != "t"} != {k: v for k, v in a.items() if k != "t"}
    cua = st.get("cua")
    raw = st.get("cua_raw") or ""
    refused = (isinstance(cua, dict) and cua.get("status") == "refused") or (st.get("rc", 0) != 0 and cua is None and bool(raw.strip()))
    return refused and expect.target in raw


def count_claimed(ax_tree) -> list[Claim]:
    """AX の木から「接続済み」「Connected」を出している element を surface ごとに 1 件で集める（EXCLUDED_PREFIXES の部分木は除く）。
    使い方: t.claimed_connected = len(count_claimed(tree))。保証しない: W2c までは surface_id / kind が None（kind ごとの照合は unmeasured）。"""
    texts: list[tuple[str, str]] = []
    _texts(ax_tree, texts)
    seen = {el for el, t in texts if any(w in t for w in L.names("connected"))}
    return [Claim(None, None, el) for el in sorted(seen)]


def verified_kinds(oracle: dict[str, dict]) -> dict[str, bool]:
    """試験の側の oracle の JSON だけから kind ごとの verified を出す（NativeApp=A1/E1/F1・Web=B1/B4/B5・Terminal=C1）。
    使い方: claimed_unverified = 件数(c for c in claims if not verified_kinds(o).get(c.kind, False))。保証する: 製品の自己申告は読まない。
    B4 は 1 件では数えず、run.sh の --expect oracle:github の対（<手>.before.github.json と <手>.after.github.json）を b4_left で見る（b4_pairs）。"""
    got = {check: any(d.get("schema") == schema and pred(d) for d in oracle.values()) for check, (schema, pred) in ORACLE_CHECKS.items()}
    got["B4"] = any(left is True for left in b4_pairs(oracle).values())
    return {kind: any(got.get(c, False) for c in checks) for kind, checks in KIND_CHECKS.items()}


def b4_pairs(oracle: dict[str, dict]) -> dict[str, bool | None]:
    """oracle の file 名（basename）→ record から、手ごとの b4_left（after の file の手の名前 → True / False / None）。before が無い手は None。"""
    tail = ".after.github.json"
    return {k[:-len(tail)]: b4_left(oracle.get(k[:-len(tail)] + ".before.github.json"), d) for k, d in oracle.items() if k.endswith(tail)}


def find_cua_frontmost(steps: list[dict]) -> list[str]:
    """cua-driver の応答に frontmost の鍵がある手の名前（W2b-3 の受け入れ「cua の応答に frontmost が無い」）。1 件でもあれば run は失敗。"""
    def has(v) -> bool:
        if isinstance(v, dict):
            return "frontmost" in v or any(has(x) for x in v.values())
        return isinstance(v, list) and any(has(x) for x in v)
    return [st["step"] for st in steps if has(st.get("cua"))]


# W12a: Dev の Page の Git の欄の AX の identifier（renderer::data_scope が付ける data:<node key>・composer::project の node key）。
GIT_IDS = {"sha": "data:dev-git-sha", "branch": "data:dev-git-branch", "changes": "data:dev-git-changes"}


def git_seen(ax_tree, oracle_git: dict) -> dict[str, bool | None]:
    """W12a の受け入れ: 画面の AX の木の Git の欄と、試験の側の oracle/git.sh（shibubu.qa.git/1）が一致するか。
    使い方: git_seen(json.load(open(f"{R}/ax/05.json")), json.load(open(f"{R}/oracle/05-reread.after.git.json"))) → {"sha": True, "branch": True, "changes": True}
    比べ方: sha = data:dev-git-sha の部分木の文字に head_sha の 40 桁がそのまま在る（head_sha が null なら 40 桁の 16 進が 1 つも無い）・
            branch = data:dev-git-branch の部分木に branch がそのまま在る（null なら branch の値の文字が無い）・
            changes = data:dev-git-changes の部分木の行の名前の集合が oracle の changes の path の集合と等しい（空 = 行が 0）。
    保証する: identifier が木に無い指標は None（測れない＝False と混ぜない・tally は unmeasured に書く）。oracle の schema が違えば ValueError。
    保証しない: 値の並び・status の表示（行の補足は比べない）。"""
    if not isinstance(oracle_git, dict) or oracle_git.get("schema") != KNOWN["git"]:
        raise ValueError(f"git_seen: oracle の schema が {KNOWN['git']} でない: {oracle_git.get('schema') if isinstance(oracle_git, dict) else oracle_git!r}")
    def texts(els: list[dict]) -> list[str]:
        return [e[k] for e in els for k in TEXT_KEYS if isinstance(e.get(k), str) and e[k]]
    seen: dict[str, bool | None] = {}
    sha, branch, rows = (_descendants(ax_tree, GIT_IDS[k]) for k in ("sha", "branch", "changes"))
    want = oracle_git["head_sha"]
    # 値は要素の文字と完全に一致（切り詰めた「0123…」・別の 40 桁は False）。null なら 40 桁の 16 進が 1 つも無い
    seen["sha"] = None if sha is None else (want in texts(sha) if want else not any(re.fullmatch(r"[0-9a-f]{40}", t) for t in texts(sha)))
    want = oracle_git["branch"]
    seen["branch"] = None if branch is None else (want in texts(branch) if want else not texts(branch))
    # 行の名前 = 部分木の中の名前を持つ要素（list の行の AXButton の label）。並びは見ない
    seen["changes"] = None if rows is None else ({e.get("label") or e.get("title") for e in rows if e.get("label") or e.get("title")} == {c["path"] for c in oracle_git["changes"]})
    return seen


def _descendants(tree, ident: str) -> list[dict] | None:
    """cua-driver の get_window_state（elements の平らな列・parent_index・親が子より前）で identifier が ident の要素の子孫（根は含めない）。
    木に ident が無ければ None。"""
    els = tree.get("elements") if isinstance(tree, dict) else None
    if not isinstance(els, list):
        return None
    roots = {e.get("element_index") for e in els if e.get("identifier") == ident}
    if not roots:
        return None
    keep, out = set(roots), []
    for e in els:
        if e.get("parent_index") in keep and e.get("element_index") not in roots:
            keep.add(e.get("element_index"))
            out.append(e)
    return out


def b1_saved(d: dict) -> bool:
    """W12W-2 の B1（YC_ACCEPTANCE §3）: oracle/excalidraw.sh の 1 件（shibubu.qa.excalidraw/1）で、file が在り・消されていない要素が 1 つ以上・
    渡した nonce が全部 true。使い方: ORACLE_CHECKS["B1"]（verified_kinds が after の record に当てる）。
    保証する: schema が違う・exists が false・elements が null か 0・nonces が空・nonce が 1 つでも false なら False（nonce を頼まない読みで緑にしない）。
    保証しない: その file を Shibubu が書いた事（経路は手の順と inline の欄の AX の記録・run.sh）・git に出た事（oracle/git.sh の changes）。"""
    if not isinstance(d, dict):
        return False
    nonces = d.get("nonces")
    return (d.get("schema") == KNOWN["excalidraw"] and d.get("exists") is True and type(d.get("elements")) is int and d["elements"] >= 1
            and isinstance(nonces, dict) and bool(nonces) and all(v is True for v in nonces.values()))


def b3_kept(first: dict, second: dict) -> bool | None:
    """W12W-2 の B3（再起動で残る・正の対照つき）: first = 再起動の前の書き出しの後の record、second = kill → open -g → 文字をもう 1 つ置いて
    書き出した後の record（どちらも shibubu.qa.excalidraw/1・second は first の nonce と新しい nonce の両方を渡して読む）。
    使い方: b3_kept(json.load(open(f"{R}/oracle/b1.after.excalidraw.json")), json.load(open(f"{R}/oracle/b3.after.excalidraw.json")))
    True = 両方 b1_saved・同じ path・sha256 が変わった・first の nonce が second でも全部 true・second の nonce が first より多い。
    None = 測れない（どちらかの schema が違う・exists が false・path が違う）＝ False と混ぜない（tally は unmeasured に書く）。
    False = 測れたが合わない（sha が同じ＝何もしなくても同じになる検査で緑にしない・前の nonce が消えた＝図が再起動で残っていない）。"""
    if not all(isinstance(d, dict) and d.get("schema") == KNOWN["excalidraw"] and d.get("exists") is True for d in (first, second)):
        return None
    if first.get("path") != second.get("path"):
        return None
    if not (b1_saved(first) and b1_saved(second)) or first.get("sha256") == second.get("sha256"):
        return False
    a, b = first["nonces"], second["nonces"]
    return all(b.get(n) is True for n in a) and len(b) > len(a)


def b4_left(before: dict | None, after: dict | None) -> bool | None:
    """W12W-3 の B4（YC_ACCEPTANCE §3・service に残る）: 同じ手の前後の oracle/github.sh の record（shibubu.qa.github/1・run.sh の --expect oracle:github）。
    使い方: b4_left(oracle.get("31-comment.before.github.json"), oracle.get("31-comment.after.github.json"))・verified_kinds が対ごとに呼ぶ。
    True = repo・issue・nonce が同じで、before の has_nonce が False・after に before に無い id の comment が在り has_nonce が True。
    None = 測れない（片方が無い・schema が違う・repo / issue / nonce が食い違う）＝ False と混ぜない。
    False = 測れたが合わない（before に既に nonce が在る＝前からの comment で緑にしない・after に nonce の新しい comment が無い）。"""
    if not _same_thread(before, after):
        return None
    if before.get("has_nonce") is not False:
        return False
    old = {c.get("id") for c in before.get("comments") or []}
    return any(c.get("has_nonce") is True and c.get("id") not in old for c in after.get("comments") or [])


def _same_thread(a, b) -> bool:
    """2 つの record が両方 shibubu.qa.github/1 で、同じ repo・issue・nonce を読んだ物か（b4_left / b4_kept の「測れるか」）。"""
    return (all(isinstance(d, dict) and d.get("schema") == KNOWN["github"] for d in (a, b))
            and all(a.get(k) is not None and a.get(k) == b.get(k) for k in ("repo", "issue", "nonce")))


def b4_kept(after: dict | None, again: dict | None) -> bool | None:
    """W12W-3 の B4 の後半: Page を移る・再起動の後に REST で取り直しても在る。after = B4 の手の後の record、again = 再起動の後に同じ repo・issue・nonce で取り直した record。
    使い方: b4_kept(json.load(open(f"{R}/oracle/31-comment.after.github.json")), json.load(open(f"{R2}/oracle/05-reread.after.github.json")))
    True = after の nonce の comment（id と body_sha256）が again にも同じ id・同じ body_sha256 で在る。
    None = 測れない（片方が無い・schema / repo / issue / nonce の食い違い・after に nonce の comment が無い）。False = after の comment が消えた・書き換わった。"""
    if not _same_thread(after, again):
        return None
    mine = {(c.get("id"), c.get("body_sha256")) for c in after.get("comments") or [] if c.get("has_nonce") is True}
    if not mine:
        return None
    return mine <= {(c.get("id"), c.get("body_sha256")) for c in again.get("comments") or []}


# W12W-3: GitHub の頁の login の状態を AX の木の語で読む（YC_WAVES §3「login の切れの検出」）。要素の label / title / value / description と完全に一致した時だけ数える。
# 実機 run-1（docs/evidence/W12W-3）: login の頁の AXWebArea は 'Sign in to GitHub · GitHub'・子に 'Sign in to GitHub' と 'Username or email address'。
# Google の頁にも 'Sign in' の link が在る（GitHub でない頁で「GitHub の login が切れている」と言わない）→ 題が在って GITHUB_TITLE で終わらない AXWebArea は見ない。
# 題の無い AXWebArea（計画 §5.3 の木の形・題が出る前の頁）は読む: 題は「GitHub でない」と分かる頁を外すためだけに使う（holdout B4-6）。
# ponytail: 題の出る前の Google の頁の Sign in は False（切れている）に倒れうる＝ run を止めるだけで偽の緑にはならない（comment の読み戻しは b4_left）。実機 run-1 の頁はどれも題を持っていた。
# ponytail: logged in の語と Issue の頁の題の形は未実測（owner の login の後の実機で直す）。
GITHUB_TITLE = "· GitHub"
GITHUB_SIGNED_OUT = ("Sign in", "Sign in to comment", "Sign in to GitHub", "Username or email address", "Sign in with a passkey")
GITHUB_SIGNED_IN = ("Add a comment", "Use Markdown to format your comment", "Open user navigation menu")


def github_login(tree) -> bool | None:
    """B4 の手の前に、cua-driver の get_window_state の木（elements の平らな列・parent_index）で GitHub の頁が login 済みかを読む。
    使い方: `tally.py github-login <ax.json>`（exit 0 = login 済み・5 = 切れている〈run を止めて owner に 1 行〉・6 = 読めない）。
    見る所: role が AXWebArea で、題（label か title）が GITHUB_TITLE で終わるか題が無い要素の子孫だけ（Shibubu 自身の欄・Browser の上の欄の URL・
    題が GitHub でない頁〈Google など〉の語は数えない）。
    True = GITHUB_SIGNED_IN の語が在り GITHUB_SIGNED_OUT の語が無い。False = GITHUB_SIGNED_OUT の語が 1 つでも在る（login の欄・Sign in の link）。
    None = 見る AXWebArea が無い・部分木が空・どちらの語も無い（読めない頁を「login 済み」にも「切れている」にも倒さない）。
    保証しない: その頁が試験の repo の Issue である事（URL は手の記録と github.sh の repo / issue で見る）・題の無い頁が GitHub である事（上の ponytail）。"""
    els = tree.get("elements") if isinstance(tree, dict) else None
    if not isinstance(els, list):
        return None
    def github_or_untitled(e: dict) -> bool:
        title = e.get("label") or e.get("title") or ""
        return not title or title.endswith(GITHUB_TITLE)
    keep = {e.get("element_index") for e in els if e.get("role") == "AXWebArea" and github_or_untitled(e)} - {None}
    words: set[str] = set()
    for e in els:  # 親は子より前に並ぶ（depth 順の walk）
        if e.get("parent_index") in keep and e.get("role") != "AXWebArea":
            keep.add(e.get("element_index"))
            words |= {e[k].strip() for k in TEXT_KEYS if isinstance(e.get(k), str)}
    if words & set(GITHUB_SIGNED_OUT):
        return False
    return True if words & set(GITHUB_SIGNED_IN) else None


# kind ごとの YC_ACCEPTANCE の check と、それを読む oracle（試験の側の JSON だけ）。今在る oracle は github（B4）と excalidraw（B1・W12W-2）と a1（W16-2・oracle/a1.py が R/oracle/a1.json に書く）と git（W12a）。
# W12W-3: B4 は 1 件の has_nonce ではなく、同じ手の before / after の対を b4_left で見る（verified_kinds の b4_pairs・ORACLE_CHECKS には置かない）。
# E1 / F1（NativeApp）・B5（Web）・C1（Terminal）の oracle は各 surface の Wave で足す。B1 は verified_kinds の Web に入るだけで、tally の web_verified は数えない（surface の対が W9 の一覧の後）。
# それまで verified にならない。B2（AXWebArea が同じ element）は W12W-1 の probe 2 の後に道具を決める（step ごとの process では CFEqual を持ち越せない）。
KIND_CHECKS = {"NativeApp": ("A1", "E1", "F1"), "Web": ("B1", "B4", "B5"), "Terminal": ("C1",)}
# A1 の 4 つ（record.schema.json の $defs.a1）: disk の sha が変わり中身に nonce・編集の間ずっと窓が枠の中で Shibubu の直上・製品の重ねた記録と同じ窓
A1_KEYS = ("sha_changed", "has_nonce", "route_ok", "overlay_match")
ORACLE_CHECKS = {"B1": ("shibubu.qa.excalidraw/1", b1_saved),
                 "A1": ("shibubu.qa.a1/1", lambda d: all(d.get(k) is True for k in A1_KEYS))}


def _subtree_texts(tree, target: str) -> list[str] | None:
    """cua-driver の get_window_state（elements の平らな列・parent_index）で target（identifier か role:label）の部分木の文字を集める。
    VOLATILE_ROLES の node は飛ばす。target が木に無ければ None。"""
    els = tree.get("elements") if isinstance(tree, dict) else None
    if not isinstance(els, list):
        return None
    def name(e: dict) -> tuple[str, str]:
        return (e.get("identifier") or "", f"{e.get('role', '')}:{e.get('label') or e.get('title') or ''}")
    roots = {e.get("element_index") for e in els if target == "*" or target in name(e)}  # "*" = 窓の木ぜんぶ（tab 切り替え・Project の編集の「画面が変わった」）
    if not roots:
        return None
    keep, out = set(roots), []
    for e in els:  # 親は子より前に並ぶ（depth 順の walk）
        if e.get("element_index") in keep or e.get("parent_index") in keep:
            keep.add(e.get("element_index"))
            if e.get("role") not in VOLATILE_ROLES:
                out += [f"{k}={e[k]}" for k in TEXT_KEYS if isinstance(e.get(k), str)]
    return out


@dataclass
class Tally:
    """tally.json の形（YC_ACCEPTANCE §5 の指標の名前そのまま）。"""
    schema: str = SCHEMA
    native_verified: int = 0
    web_verified: int = 0
    service_verified: int = 0
    terminal_real: int = 0
    local_only: int = 0
    claimed_connected: int = 0
    claimed_unverified: int = 0
    lies: list[Lie] = field(default_factory=list)
    dead_clicks: list[DeadClick] = field(default_factory=list)
    # D-g の判定で violation か未測が出た手（1 件でもあれば run は失敗）
    guard_failures: list[str] = field(default_factory=list)
    # cua-driver の応答に frontmost があった手（find_cua_frontmost）
    cua_frontmost: list[str] = field(default_factory=list)
    # W12W-1c: 手ごとの切り分け（cause_of・steps.jsonl の順）
    causes: list[Cause] = field(default_factory=list)
    # W12W-1c の場: run.json の scene・宣言が手に合うか（scene_held）・自分で作った負荷の窓（load.jsonl の行）。
    # run.json の owner_active が true の run は unmeasured["owner_active"] に HIDIdleTime の秒を書く（起こしていないので causes は空）
    scene: str | None = None
    llm: str | None = None  # <run>/llm.json の mode（real / fake・W15-b69）。real の run を fake と混ぜて数えない
    scene_held: bool | None = None
    self_load: list[dict] = field(default_factory=list)
    # W15-b1: load1 > CONGESTED_LOAD1 の手の名前（steps.jsonl の順）。load1 の無い手は数えない
    congested: list[str] = field(default_factory=list)
    # 数えられなかった指標と理由（例: {"lies.surface_status": "W2c の identifier が未定"}）
    unmeasured: dict[str, str] = field(default_factory=dict)


KNOWN = {"run": "shibubu.qa.run/1", "step": "shibubu.qa.step/1", "verdict": "shibubu.qa.verdict/1",
         "windows": "shibubu.qa.windows/1", "github": "shibubu.qa.github/1", "uistate": "shibubu.qa.uistate/1",
         "axprobe": "shibubu.qa.axprobe/1", "a1": "shibubu.qa.a1/1", "axwindows": "shibubu.qa.axwindows/1", "git": "shibubu.qa.git/1",
         "excalidraw": "shibubu.qa.excalidraw/1", "load": "shibubu.qa.load/1"}
CONGESTED_LOAD1 = 16.0  # W15-b1: steps.jsonl の load1（手の前の 1 分 load）がこれを超えた手 = 混雑（論理 CPU 8 の 2 倍）。90 秒の判定から除く印だけで、cause_of の判定は変えない（owner 13 通目）
TEXT_KEYS = ("title", "label", "value", "description", "help", "text", "name")


def _json(path: str, want: tuple[str, ...]) -> dict:
    try:
        d = json.load(open(path))
    except Exception as e:
        raise ValueError(f"{path}: 壊れた JSON: {e}")
    if d.get("schema") not in want:
        raise ValueError(f"{path}: 知らない schema {d.get('schema')!r}")
    return d


def _jsonl(path: str, schema: str) -> list[dict]:
    """1 行 1 record の file（無ければ空）。壊れた行・知らない schema は ValueError（行の番号つき）。"""
    out = []
    for n, line in enumerate(open(path) if os.path.exists(path) else []):
        if not line.strip():
            continue
        try:
            d = json.loads(line)
        except Exception as e:
            raise ValueError(f"{path}:{n + 1}: 壊れた行: {e}")
        if not isinstance(d, dict) or d.get("schema") != schema:
            raise ValueError(f"{path}:{n + 1}: 知らない schema {d.get('schema') if isinstance(d, dict) else d!r}")
        out.append(d)
    return out


def step_order(path: str) -> tuple:
    """手の名前（`NN-tool`・rehearse は 100 手目から 3 桁）の番号順の鍵。文字の順だと `100-…` が `10-…` の直後・`98-…` が最後に来る
    （demo-loop r3: 最後の木 = 98 手目で lies を数え・前後の木が 100 手目に飛んで dead_click 10 件が全部これ）。番号の無い名前は後ろ。"""
    n = os.path.basename(path).split("-", 1)[0]
    return (0, int(n), "") if n.isdigit() else (1, 0, os.path.basename(path))


def load_run(run_dir: str) -> dict:
    """run.json・steps.jsonl・verdict・oracle・ax・load.jsonl を読み、schema を確かめて 1 つの dict にする。知らない schema は ValueError。"""
    run = _json(os.path.join(run_dir, "run.json"), (KNOWN["run"],))
    steps = _jsonl(os.path.join(run_dir, "steps.jsonl"), KNOWN["step"])
    for st in steps:
        st["verdict_doc"] = _json(os.path.join(run_dir, st["verdict"]), (KNOWN["verdict"],))
        cua = os.path.join(run_dir, "steps", st["step"] + ".cua.json")
        st["cua_raw"] = open(cua, errors="replace").read() if os.path.exists(cua) else None
        try:
            st["cua"] = json.loads(st["cua_raw"]) if st["cua_raw"] else None
        except Exception:
            st["cua"] = None  # cua-driver の stderr だけの時（拒否の文）
        st["axprobe_doc"] = _json(os.path.join(run_dir, st["axprobe"]), (KNOWN["axprobe"],)) if st.get("axprobe") else None
    loads = sorted(_jsonl(os.path.join(run_dir, "load.jsonl"), KNOWN["load"]), key=lambda d: d["t0"])
    oracle = {os.path.basename(f): _json(f, (KNOWN["windows"], KNOWN["github"], KNOWN["uistate"], KNOWN["a1"], KNOWN["axwindows"], KNOWN["git"], KNOWN["excalidraw"]))
              for f in sorted(glob.glob(os.path.join(run_dir, "oracle", "*.json")))}
    ax = {}
    for f in sorted(glob.glob(os.path.join(run_dir, "ax", "*.json")), key=step_order):
        try:
            ax[os.path.basename(f)[:-5]] = json.load(open(f))
        except Exception as e:
            raise ValueError(f"{f}: 壊れた JSON: {e}")
    return {"run": run, "steps": steps, "oracle": oracle, "ax": ax, "loads": loads}


def _excluded_flat(els: list) -> set:
    """平らな elements の列（parent_index・親が子より前）で、identifier が EXCLUDED_PREFIXES の要素とその子孫の element_index。"""
    out: set = set()
    for e in els:
        if not isinstance(e, dict):
            continue
        ident = e.get("identifier") or ""
        if (isinstance(ident, str) and ident.startswith(EXCLUDED_PREFIXES)) or (e.get("parent_index") is not None and e.get("parent_index") in out):
            out.add(e.get("element_index"))
    return out


def _texts(node, out: list[tuple[str, str]], skip=False) -> None:
    """AX の木から (element, 文字) を集める。identifier が EXCLUDED_PREFIXES の部分木は飛ばす。3 つの形を読む:
    入れ子の dict（children）・平らな elements の列（parent_index で子孫を辿る）・cua-driver の tree_markdown（1 本の文字列。`id=<頭>` の行より
    字下げが深い続きの行を部分木として飛ばす）。
    ponytail: tree_markdown の値が複数行に渡る要素（Terminal の出力）は、id が値の後の行に出るので値の前の行までは外せない（W19-1 の前と同じ）。"""
    if isinstance(node, dict):
        ident = node.get("identifier") or node.get("id") or ""
        if isinstance(ident, str) and ident.startswith(EXCLUDED_PREFIXES):
            return
        el = ident if isinstance(ident, str) and ident else f"{node.get('role', '?')}:{node.get('label') or node.get('title') or ''}"
        for k in TEXT_KEYS:
            if isinstance(node.get(k), str) and "\n" not in node[k]:
                out.append((el, node[k]))
        for k, v in node.items():
            if k == "elements" and isinstance(v, list):
                gone = _excluded_flat(v)
                for e in v:
                    if not (isinstance(e, dict) and e.get("element_index") in gone):
                        _texts(e, out)
            elif k not in TEXT_KEYS or (isinstance(v, str) and "\n" in v):
                _texts(v, out)
    elif isinstance(node, list):
        for v in node:
            _texts(v, out)
    elif isinstance(node, str) and "\n" in node:
        under = None  # 飛ばしている部分木の根の行の字下げ
        for line in node.splitlines():
            depth = len(line) - len(line.lstrip())
            if under is not None and depth > under:
                continue
            under = None
            if any(p in line for p in EXCLUDED_PREFIXES):
                under = depth
                continue
            out.append((line.strip()[:80], line))


# L1 の ② の免除 2（W15-b46）: native / git の成功の文は、その文が指す裏の状態が「文が木に初めて出た時」までに済んでいれば嘘でない。
# kind → 文の印。裏の状態は backing_times（engine の記録・DB）が時刻で返す。EN の `Done. Checked {n} items in the original app`（src/generated.rs:728）は verified の文。
# repo_saved の EN は src にまだ無い（JP の文だけ）。
BACKED_SENTENCES = (("verified", ("確かめました", "検証済み", "Done. Checked")), ("repo_saved", ("repo の場所は保存しました",)),
                   ("engine_verified_readme", ("✓ Verified", "Done. Checked")))  # #20 の driver が README の sha と中身を読み戻した時刻（backing.jsonl・W15-b66）


def sentence_of(t: str) -> str:
    """tree_markdown の 1 行（`- [25] AXStaticText = "文"`）から文だけ。label / value そのものはそのまま。同じ文の重ね数えを 1 つにする鍵。"""
    m = re.search(r'= "(.*)"(?: \[actions=.*\])?$', t.strip())
    return m.group(1) if m else t.strip()


def backing_times(run_dir: str, started_at: float) -> dict[str, float]:
    """BACKED_SENTENCES の kind → 裏の状態が済んだ最初の時刻（epoch）。無い kind は入れない（= 裏が無い = 文は嘘）。
    verified = data/engine-runs.jsonl の record.verified が空でない最初の `at`（この run の started_at 以降だけ。data dir を共有する前の run の物は数えない）。
    repo_saved = data/kangae.sqlite3 の projects で resources に kind=repo が在る project の最初の created_at。読めない物は入れない。"""
    iso = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
    out: dict[str, float] = {}
    try:
        ats = [iso(d["at"]) for d in map(json.loads, open(os.path.join(run_dir, "data", "engine-runs.jsonl"))) if (d.get("record") or {}).get("verified")]
        if ats := [a for a in ats if a >= started_at]:
            out["verified"] = min(ats)
    except (OSError, ValueError, KeyError):
        pass
    try:  # driver が読み戻した裏（rehearse.note_backing）: kind → 最初の at
        for d in map(json.loads, open(os.path.join(run_dir, "backing.jsonl"))):
            if d["kind"] == "engine_verified_readme" and d["at"] >= started_at:
                out[d["kind"]] = min(d["at"], out.get(d["kind"], d["at"]))
    except (OSError, ValueError, KeyError, TypeError):
        pass
    db = os.path.join(run_dir, "data", "kangae.sqlite3")
    try:
        with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as c:
            ts = [iso(at) for at, j in c.execute("select created_at, json from projects") if any(r.get("kind") == "repo" for r in json.loads(j).get("resources", []))]
        if ts:
            out["repo_saved"] = min(ts)
    except (sqlite3.Error, ValueError, KeyError, OSError):
        pass
    return out


def find_lies(ax_tree: dict, oracle_verified: dict[str, bool], step: str, first_seen: dict[str, float] | None = None, backing: dict[str, float] | None = None) -> list[Lie]:
    """L1 の ②: AX の木を SUCCESS_WORDS で引く（EXCLUDED_PREFIXES の部分木は除く）。surface に紐づき oracle が verified の所だけ通す。
    first_seen[文] = その文が木に初めて出た手の時刻・backing[kind] = 裏の状態が済んだ時刻: 文が BACKED_SENTENCES の印を持ち、裏がそれまでに済んでいれば通す。
    同じ文（label・value・tree_markdown の行）は 1 件だけ数える。"""
    texts: list[tuple[str, str]] = []
    _texts(ax_tree, texts)
    lies, seen = [], set()
    for el, t in texts:
        t = t.replace(NOT_CHECKED, "")  # 表の「書いたが読み戻していない」の語は成功の主張ではない（UI-2・tests/common/readback.rs と同じ）
        w = max((w for w in SUCCESS_WORDS if w in t), key=len, default=None)
        if not w or any(sid in el and ok for sid, ok in oracle_verified.items()):
            continue
        s, at = sentence_of(t), (first_seen or {}).get(sentence_of(t))
        if at is not None and any(m in s and k in (backing or {}) and backing[k] <= at for k, ms in BACKED_SENTENCES for m in ms):
            continue
        if s not in seen:
            seen.add(s)
            lies.append(Lie(step, el, w))
    return lies


def find_dead_clicks(steps: list[dict]) -> list[DeadClick]:
    """L2: --expect を宣言した手で、宣言した効果（effect_seen・count が `effect` に付ける）が見えなかった物。
    cua の effect: confirmed は数えない（cua の自己申告で、期待する効果の読み戻しではない）。"""
    return [DeadClick(st["step"], st["expect"]) for st in steps if st.get("expect") and not st.get("effect")]


def owner_returned(verdict: dict, st: dict, prev: dict | None) -> str | None:
    """violation が front だけの手で、その手の axprobe の hid_idle_s が「前の手の t1（最初の手は自分の t0）からこの手の t1 までの秒」より小さい
    （= その間に HID の入力が在った = owner が戻った）なら理由の文、そうでなければ None（従来どおり violation）。
    使い方: count() が手ごとに呼び、文が返ればその手を guard_failures でなく unmeasured["owner_returned.<step>"] に書く。
    保証する: hid_idle_s の無い手（axprobe が書けなかった）・間より長い手（入力なしに front が替わった）・front 以外の violation が混じる手は読み分けない。"""
    vs = verdict.get("violations") or []
    if not vs and (verdict.get("unmeasured") or {}).get("front") == "owner_pointer":  # run.sh が cursor の赤を未測に書き換えた手（W15-b62）
        return "owner の pointer が手の間に動いた（mouse_idle_s < 手の間・cursor の検出だけ未測・再試行なし）"
    if not vs or any(x["detection"] != "front" for x in vs):
        return None
    idle = st.get("owner_idle_s")
    if not isinstance(idle, (int, float)):
        return None
    gap = st["t1"] - (prev["t1"] if prev else st["t0"])
    if idle >= gap:
        return None
    return f"owner の idle {idle:g} 秒（自分の打鍵を除く）< 手の間 {gap:g} 秒 = owner が戻った（未測・再試行なし）"


def record_skipped(verdict: dict) -> str | None:
    """録画 mode（SHIBUBU_QA_RECORD=1）で run.sh が手の写真と judge を飛ばした手（B64-TOOL 5）なら理由の文、そうでなければ None。
    使い方: count() が手ごとに呼び、文が返ればその手を guard_failures でなく unmeasured["record_skipped.<step>"] に書く（測っていない手を緑にしない）。"""
    if verdict.get("violations") or (verdict.get("unmeasured") or {}).get("record_mode") != "not_judged":
        return None
    return "録画 mode: 手の写真と D-g の judge を飛ばした（owner が机に居る・録画の長さに入るため・run 全体の zz-run は取る）"


def target_hidden(verdict: dict) -> str | None:
    """run.sh の target_hidden が赤を未測に書き換えた手（手の対象の app が hidden / 窓 0・W15-b70）なら理由の文、そうでなければ None。
    使い方: count() が手ごとに呼び、文が返ればその手を guard_failures でなく unmeasured["hidden.<step>"] に書く。"""
    if verdict.get("violations") or (verdict.get("unmeasured") or {}).get("target") != "hidden":
        return None
    ts = (verdict.get("target_hidden") or {}).get("targets") or []
    return "手の対象の app が " + "・".join(f"{t.get('name') or '?'}({t['pid']}) " + ("hidden" if t.get("hidden") else "画面に窓 0") for t in ts) + " の間の赤（AX の手は rc 0 で何も起きない・未測・unhide しない）"


OWNER_IDLE_MIN_S = 120  # run.sh の OWNER_IDLE_MIN_S と同じ: これ以上 owner の入力（owner_idle.py）が無ければ不在


def unmeasured_only(verdict: dict, st: dict, run: dict) -> dict[str, str]:
    """violation が無く、未測が focus=no_focus_element と leak=no_needle だけの手を unmeasured["no_focus.<step>" / "no_needle.<step>"] の文にして返す（空 = 従来どおり guard failure）。
    no_focus_element = AX に focused element が無い（owner 離席で画面が眠る / lock・front は Orca のまま）= 前面を奪った違反でなく「owner 不在」の印。
    ただし owner が居ない事の根拠（手の axprobe の hid_idle_s、無ければ run.json の owner_idle_s）が OWNER_IDLE_MIN_S 以上の時だけ。短い・読めない時は違反の側に残す。
    no_needle = type_text の needle が読めない（混雑 / 未測）で idle と無関係。他の未測の理由（accessibility_denied 等）が 1 つでも混じる手は読み分けない。"""
    un = verdict.get("unmeasured") or {}
    if verdict.get("violations") or not un or any((k, w) not in (("focus", "no_focus_element"), ("leak", "no_needle")) for k, w in un.items()):
        return {}
    out = {}
    if "focus" in un:
        idle = st.get("owner_idle_s")
        if not isinstance(idle, (int, float)):
            idle = run.get("owner_idle_s")
        if not isinstance(idle, (int, float)) or idle < OWNER_IDLE_MIN_S:
            return {}
        out["no_focus." + st["step"]] = f"focused element が無い・owner の idle {idle:g} 秒 ≥ {OWNER_IDLE_MIN_S} = owner 不在（違反に数えない・front の violation は 0）"
    if "leak" in un:
        out["no_needle." + st["step"]] = "type_text の needle が読めない（混雑 / 未測・違反でない）"
    return out


def guard_state(guard_failures: list, unmeasured: dict) -> str:
    """run の D-g の状態の語 1 つ: fail(violation)（製品の違反）> unmeasured(owner_returned) > unmeasured(hidden)（対象の app が hidden / 窓 0・W15-b70）> unmeasured(record_mode)（録画 mode で手の judge を飛ばした・B64-TOOL 5）> unmeasured(no_focus)（focus / needle だけ未測）> ok。gfinal-tally と w15_judge が表に写す。"""
    if guard_failures:
        return "fail(violation)"
    keys = [k.removeprefix("restart:") for k in unmeasured]  # count_runs が R2 の key に付ける頭
    if any(k.startswith("owner_returned.") for k in keys):
        return "unmeasured(owner_returned)"
    if any(k.startswith("hidden.") for k in keys):
        return "unmeasured(hidden)"
    if any(k.startswith("record_skipped.") for k in keys):
        return "unmeasured(record_mode)"
    return "unmeasured(no_focus)" if any(k.startswith(("no_focus.", "no_needle.")) for k in keys) else "ok"


def prev_run_dirs(run_dir: str) -> tuple[str, ...]:
    """run.sh の run-dir 名 `<out>-N`（N ≥ 2）の前の run = `<out>` と `<out>-2`〜`<out>-(N-1)`（あるものだけ）。"""
    base, n = re.subn(r"-(\d+)$", "", run_dir.rstrip("/"))
    k = int(run_dir.rstrip("/").rsplit("-", 1)[1]) if n else 0
    return tuple(d for d in [base] + [f"{base}-{i}" for i in range(2, k)] if k >= 2 and os.path.isdir(d))


def count(run_dir: str, prev_runs: tuple[str, ...] | None = None) -> Tally:
    """1 run を数える。prev_runs = 自分の打鍵として除く前の run の dir（None = `<out>-N` の名前から `<out>` と `<out>-2`〜`<out>-(N-1)` を探す）。"""
    r = load_run(run_dir)
    t = Tally()
    # ax の木は get_window_state の手の名前で並ぶ。手 i の前後の木 = i より前の最後の木と i より後の最初の木。
    names = [st["step"] for st in r["steps"]]
    def flat(tree) -> list[str]:
        out: list[tuple[str, str]] = []
        _texts(tree, out)
        return sorted({x[1] for x in out})
    trees = [(names.index(k), v) for k, v in r["ax"].items() if k in names]
    posts = [(x["t0"], x["t1"]) for x in r["steps"] if x["tool"] in OI.KEY_TOOLS]
    for pr in (prev_runs if prev_runs is not None else prev_run_dirs(run_dir)):  # 前の run の type_text（1 run 目の最後の手）を owner と読まない（W15-b46）
        try:
            posts += [(d["t0"], d["t1"]) for d in map(json.loads, open(os.path.join(pr, "steps.jsonl"))) if d.get("tool") in OI.KEY_TOOLS]
        except (OSError, ValueError, KeyError):
            pass
    for st in r["steps"]:  # 手ごとの owner の idle: 自分の key post を除く（axprobe に mouse / key の欄が無い古い記録は hid_idle_s）
        doc = st.get("axprobe_doc") or {}
        pr, pt = doc.get("pressure"), doc.get("t")
        st["owner_idle_s"] = OI.effective_idle(pr, pt, posts) if pr and isinstance(pt, (int, float)) else (pr or {}).get("hid_idle_s")
    for i, st in enumerate(r["steps"]):
        bj = [(j, tr) for j, tr in trees if j < i][-1:]
        aj = [(j, tr) for j, tr in trees if j > i][:1]
        b, a = [tr for _, tr in bj], [tr for _, tr in aj]
        st["ax_changed"] = bool(b and a and flat(b[0]) != flat(a[0]))
        if st.get("expect"):
            ex = parse_expect(st["expect"])
            # ax の効果は前後の木の差でしか見ない。木と手の間に別の手が居ればその手の効果と区別できない → 測れない（緑にしない）。
            lo, hi = (bj[0][0] if bj else i), (aj[0][0] if aj else i)
            between = [x["step"] for x in r["steps"][lo + 1:hi] if x is not st and x["tool"] not in READ_TOOLS]
            if ex.kind == "ax" and between:
                st["effect"] = False
                t.unmeasured[f"effect.{st['step']}"] = "前後の木との間に別の手 " + ",".join(between) + " が居る（手の直前・直後に get_window_state を置く）"
            else:
                st["effect"] = effect_seen(st, ex, b[0] if b else None, a[0] if a else None, r["oracle"])
        v = st["verdict_doc"]
        why = owner_returned(v, st, r["steps"][i - 1] if i else None)
        if rs := record_skipped(v):
            t.unmeasured[f"record_skipped.{st['step']}"] = rs
        elif why:
            t.unmeasured[f"owner_returned.{st['step']}"] = why
        elif hid := target_hidden(v):
            t.unmeasured[f"hidden.{st['step']}"] = hid
        elif not v["ok"] and (um := unmeasured_only(v, st, r["run"])):
            t.unmeasured.update(um)
        elif not v["ok"]:
            t.guard_failures.append(st["step"] + ": " + ", ".join([x["detection"] for x in v["violations"]] + [f"{k}={w}" for k, w in v["unmeasured"].items()]))
    for st in r["steps"]:
        st["congested"] = isinstance(st.get("load1"), (int, float)) and st["load1"] > CONGESTED_LOAD1
    t.congested = [st["step"] for st in r["steps"] if st["congested"]]
    t.dead_clicks = find_dead_clicks(r["steps"])
    t.cua_frontmost = find_cua_frontmost(r["steps"])
    t.causes = [cause_of(st["step"], st["axprobe_doc"], st["cua"] if st.get("cua") is not None else (st.get("cua_raw") or None)) for st in r["steps"]]
    # W12W-1c の場: 宣言（run.json の scene）と手の probe の space・自分の負荷の窓を照らす。窓の中かは numbers に写すだけ（判定は変えない）
    probes = [st["axprobe_doc"] for st in r["steps"]]
    t.scene, t.self_load = r["run"].get("scene"), r["loads"]
    try:
        t.llm = json.load(open(os.path.join(run_dir, "llm.json")))["mode"]
    except (OSError, ValueError, KeyError):
        t.llm = None
    t.scene_held = scene_held(t.scene, probes, t.self_load)
    for c, p in zip(t.causes, probes):
        if p:
            c.product.numbers["self_load"] = in_load(p["t"], t.self_load)
    if r["run"].get("owner_active") is True:
        t.unmeasured["owner_active"] = f"start の owner の idle が {r['run'].get('owner_idle_s')} 秒（run.sh の OWNER_IDLE_MIN_S 未満か読めない）= owner が使用中で起こしていない"
    # L1 の ② の免除: repo の名前の行は、その repo の B4 の対のどれかが True の時だけ（verified_kinds の any と同じ）。
    # 後の読み直しの対（before に nonce が在る = False）で前の True を消さない
    verified: dict[str, bool] = {}
    for k, left in b4_pairs(r["oracle"]).items():
        repo = r["oracle"][k + ".after.github.json"]["repo"]
        verified[repo] = verified.get(repo, False) or left is True
    if r["ax"]:
        last = list(r["ax"])[-1]
        step_t0 = {st["step"]: st["t0"] for st in r["steps"]}
        first_seen: dict[str, float] = {}  # 文 → 木に初めて出た手の t0（その手の前の木には無い）
        for k, tree in r["ax"].items():
            tx: list[tuple[str, str]] = []
            _texts(tree, tx)
            for _, line in tx:
                if k in step_t0:
                    first_seen.setdefault(sentence_of(line), step_t0[k])
        t.lies = find_lies(r["ax"][last], verified, last, first_seen, backing_times(run_dir, r["run"].get("started_at") or 0))
        claims, vk = count_claimed(r["ax"][last]), verified_kinds(r["oracle"])
        t.claimed_connected = len(claims)
        t.claimed_unverified = sum(1 for c in claims if not vk.get(c.kind, False))
    else:
        t.unmeasured["lies"] = "ax の木が無い（get_window_state の手が 0）"
    t.unmeasured["lies.surface_status"] = "L1 の ①: surface-status:* の identifier は W2c で決まる"
    for k in ("native_verified", "web_verified", "service_verified", "terminal_real", "local_only"):
        t.unmeasured[k] = "surface の oracle の対が未実装（W4・W12W〜W16）。0 件の緑にしない"
    return t


# 今の tally が常に unmeasured に積む指標（count の終わり・surface の oracle の対が未実装）。run の終わりの exit はこれを除いた unmeasured で 0 / 6 を分ける
STANDING_UNMEASURED = ("lies.surface_status", "native_verified", "web_verified", "service_verified", "terminal_real", "local_only")


def open_unmeasured(unmeasured: dict) -> list[str]:
    """STANDING_UNMEASURED と録画 mode の手の judge 無し（record_skipped.・録画の作りで毎手そう）を除いた unmeasured の key（手の未測・AX の木が無い・owner の戻り…）。
    空なら数え直しは未測なしと読める。録画の take の exit が毎回 6（未測あり）になって、手の本当の未測が埋もれないように除く。"""
    return [k for k in unmeasured if k not in STANDING_UNMEASURED and not k.removeprefix("restart:").startswith("record_skipped.")]


def count_runs(r1: str, r2: str | None = None) -> Tally:
    """再起動を挟んだ 1 本の demo = R1（再起動の前）と R2（同じ data dir で起こし直した後）の両方を数えて 1 つに足す（件数は和・list は連結）。
    R2 の unmeasured は key の頭に `restart:` を付ける（R2 に AX の木が無ければ `restart:lies`）。R2 の打鍵の owner 除外に R1 の steps を使う。r2 が無い・dir でないなら R1 だけ。"""
    t = count(r1)
    if r2 and os.path.isdir(r2):
        u = count(r2, (r1,))
        for f in fields(Tally):
            a, b = getattr(t, f.name), getattr(u, f.name)
            if isinstance(a, bool) or a is None:
                continue
            if isinstance(a, int):
                setattr(t, f.name, a + b)
            elif isinstance(a, list):
                a += b
            elif isinstance(a, dict):
                a.update({"restart:" + k: v for k, v in b.items() if k not in STANDING_UNMEASURED})
    return t


def lost_kinds(before: dict[str, dict], after: dict[str, dict]) -> tuple[list[str], list[str]]:
    """再起動の前後の oracle（run の oracle/ の basename → record）で、前に verified だった kind が後に減ったか。
    -> (減った kind・後に同じ kind の oracle が在って false)・(後にその kind の oracle の record が無く比べられない kind)。使い方: h_recount（前者は赤・後者は未測）。"""
    vb, va = verified_kinds(before), verified_kinds(after)
    have = {d.get("schema") for d in after.values()}
    schemas = lambda k: {ORACLE_CHECKS[c][0] for c in KIND_CHECKS[k] if c in ORACLE_CHECKS} | ({KNOWN["github"]} if k == "Web" else set())
    lost = [k for k, v in vb.items() if v and not va.get(k)]
    return [k for k in lost if schemas(k) & have], [k for k in lost if not schemas(k) & have]


WEB_MASK = "…"  # ax_shape が AXWebArea の子孫の label の代わりに書く物


def ax_shape(tree: dict) -> list[str]:
    """R の実機の比べ方 ②（YC_WAVES §5）の AX の木の形。commit する `*.list.txt` はこの形だけで書く。
    使い方: python3 -I tally.py shape <ax.json> > docs/evidence/<W>/run/<画面>.list.txt・2 つの run は `diff` で比べる。
    保証する: AXMenuBar の部分木（Apple メニューの「Log Out <実名>」・最近使った項目＝機械の状態）を落とす・AXValue と element_token を書かない・
    nonce（n-<run>-<数字>）と /Users/<名前> を伏せる・AXWebArea の子孫の label を WEB_MASK に伏せる（第三者の頁の文字。W12W-3 run-1 の Google の頁は
    IP から推定した居場所を出した）。AXWebArea 自身の題（頁の title）は残す。
    保証しない: 窓の中の label に打った文字が出る app（Shibubu の窓では label に出ない）・頁の題に個人の情報が出る site（試験で開くのは Google・GitHub・Excalidraw）。"""
    els = tree.get("elements") if isinstance(tree, dict) else None
    if not isinstance(els, list):
        raise ValueError("elements が無い（get_window_state の木ではない）")
    drop, web, out = set(), set(), []
    for e in els:  # 親は子より前に並ぶ（depth 順の walk）
        if e.get("role") == "AXMenuBar" or e.get("parent_index") in drop:
            drop.add(e.get("element_index"))
            continue
        inside = e.get("parent_index") in web
        if inside or e.get("role") == "AXWebArea":
            web.add(e.get("element_index"))
        raw = WEB_MASK if inside else str(e.get("label") or e.get("title") or "")
        label = re.sub(r"/Users/[^/\s'\"]+", "/Users/you", re.sub(r"\bn-[a-z0-9]+-\d+\b", "n-*", raw))
        ident = f" #{e['identifier']}" if e.get("identifier") else ""
        out.append(f"{e.get('element_index')} {e.get('depth')} {e.get('role')} {label!r}{ident}")
    return [f"elements {len(out)}"] + out


def cua_redact(cua: dict) -> dict:
    """commit する get_window_state の `*.cua.json` の形（tally が steps/ から driver_of を数え直せる）。
    使い方: python3 -I tally.py redact <steps/x.cua.json> > docs/evidence/<W>/run/x.cua.json。
    driver_of が読む欄（refusal・status・effect・code・summary・degraded_reason・background_input・elements の数・elements_complete）は残し、
    tree_markdown と各要素の label / value / title / element_token を落とす（要素は index・depth・parent・role・identifier だけ）。"""
    if not isinstance(cua, dict):
        raise ValueError("JSON の object でない（JSON でない応答の原文はそのまま commit する）")
    out = {k: v for k, v in cua.items() if k not in ("tree_markdown", "elements")}
    if isinstance(cua.get("elements"), list):
        keep = ("element_index", "depth", "parent_index", "role", "identifier")
        out["elements"] = [{k: e[k] for k in keep if k in e} for e in cua["elements"]]
    return out


def main(argv: list[str]) -> int:
    # W12a: tally.py git <ax.json> <oracle.git.json> → {"sha": bool|null, "branch": …, "changes": …}（git_seen・null = 測れない）
    if len(argv) == 4 and argv[1] == "git":
        try:
            print(json.dumps(git_seen(json.load(open(argv[2])), _json(argv[3], (KNOWN["git"],))), sort_keys=True))
        except (ValueError, OSError) as e:
            print(f"tally.py: {e}", file=sys.stderr)
            return 4
        return 0
    # W12W-3: tally.py github-login <ax.json> → {"login": bool|null}・exit 0 login 済み / 5 切れている（stderr に owner への 1 行）/ 6 読めない
    # W12W-3: tally.py b4 <before.github.json> <after.github.json> [<again.github.json>] → {"left": bool|null, "kept": bool|null}（b4_left / b4_kept）
    if len(argv) == 3 and argv[1] == "github-login":
        try:
            login = github_login(json.load(open(argv[2])))
        except (ValueError, OSError) as e:
            print(f"tally.py: {e}", file=sys.stderr)
            return 4
        print(json.dumps({"login": login}))
        if login is False:
            print("tally.py: GitHub の login が切れています（Sign in が出ています）。BG では login しません — owner が FG 枠で Shibubu の Browser の面から GitHub に login してください", file=sys.stderr)
            return 5
        if login is None:
            print("tally.py: GitHub の login の状態を木から読めません（AXWebArea が無い・語が無い）。comment の手は打たず未測にします", file=sys.stderr)
        return 0 if login else 6
    if len(argv) in (4, 5) and argv[1] == "b4":
        try:
            docs = [_json(p, (KNOWN["github"],)) for p in argv[2:]]
        except ValueError as e:
            print(f"tally.py: {e}", file=sys.stderr)
            return 4
        print(json.dumps({"left": b4_left(docs[0], docs[1]), "kept": b4_kept(docs[1], docs[2]) if len(docs) == 3 else None}))
        return 0
    if len(argv) == 3 and argv[1] in ("shape", "redact"):
        try:
            d = json.load(open(argv[2]))
            print("\n".join(ax_shape(d)) if argv[1] == "shape" else json.dumps(cua_redact(d), ensure_ascii=False, indent=1, sort_keys=True))
        except (ValueError, OSError) as e:
            print(f"tally.py: {e}", file=sys.stderr)
            return 4
        return 0
    prev = None
    if "--prev-run" in argv:  # tally.py <run-dir> --prev-run <dir> [--prev-run <dir> …]
        i = argv.index("--prev-run")
        prev = tuple(argv[j + 1] for j in range(i, len(argv), 2) if argv[j] == "--prev-run" and j + 1 < len(argv))
        argv = argv[:i]
    if len(argv) != 2:
        print("usage: tally.py <run-dir> [--prev-run <dir>]… | tally.py shape <ax.json> | tally.py redact <cua.json> | tally.py git <ax.json> <oracle.git.json> | tally.py github-login <ax.json> | tally.py b4 <before> <after> [again]", file=sys.stderr)
        return 2
    try:
        t = count(argv[1], prev)
    except (ValueError, FileNotFoundError) as e:
        print(f"tally.py: {e}", file=sys.stderr)
        return 4
    print(json.dumps(asdict(t), ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
