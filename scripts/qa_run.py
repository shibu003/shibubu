#!/usr/bin/env python3
"""Kangae の QA 台本（docs/qa/scenarios/*.json）を cua-driver の AX 経路で 1 手ずつ実行し、PASS / FAIL / 未測 の表と写真を残す。

  bash scripts/qa-run.sh docs/qa/scenarios/qa-01-timer.json …   # 走らせる。台本ごとに一時の KANGAE_DATA_DIR で Kangae を起こす
  python3 -I scripts/qa_run.py --check docs/qa/scenarios/*.json  # 形（schema・jsonschema が要る）と意味と自己検査だけ。lint.sh が呼ぶ

判定（docs/qa/README.md）:
- PASS = AX の木が完全で、期待が全部成り立った
- FAIL = 木が完全なのに label が無い / 当たりが 2 つ以上（推測で選ばない）/ 期待が外れた / driver が拒否した / Kangae が落ちた
- 未測 = 測れなかった（木が truncated・degraded / daemon が無い / 人の手 / 写真で人が見る項目 / 前の手が FAIL で先へ進まない）
pixel の座標は送らない（Kangae の窓への pixel 経路は daemon を落とした: docs/cua-probe-2026-10-16.md 追記）。
承認・許可の label は押さない。送信・削除・共有・取り込み等の label は high_impact: review_only の手だけが押せ、押した後に
「確認待ち」（DESIGN §11）が出なければ Kangae をその場で止めて FAIL。「確認待ち」が出ている間は Return / Space も送らない。
"""
import argparse
import copy
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import unicodedata
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SCHEMA = REPO / 'docs/schemas/qa_scenario_v1.schema.json'
SPEC = ['DESIGN.md', 'KANGAE_MASTER_SPEC.md', 'IMPLEMENTATION_TODO.md', 'docs/ACCEPTANCE.md', 'docs/MUSIC_WORKSPACE_CASE_STUDY.md']
# 公開の snapshot（scripts/release/public/publish.sh）には仕様の文書が無い → label の正本を UI の source と、台本が先に使う label に替える
PLANNED_LABELS = []
if not all((REPO / f).exists() for f in SPEC):
    SPEC = sorted(str(p.relative_to(REPO)) for p in (REPO / 'src').rglob('*.rs'))
    PLANNED_LABELS = ['後で接続', 'Add to REAPER', 'ローカルで続ける', '操作を教える']
PASS, FAIL, NA = 'PASS', 'FAIL', '未測'
HIDDEN = {'AXWindow', 'AXMenuBar', 'AXMenuBarItem', 'AXMenu', 'AXMenuItem'}   # role で名指ししない限り当てない（menu の「7」: cua-probe §9）
HIGH_IMPACT = re.compile(r'送信|送る|公開|削除|購入|招待|共有|取り込|インポート|import|add to reaper|reaper ?へ ?追加', re.I)
NEVER_PRESS = re.compile(r'承認|許可|approve|allow', re.I)
REVIEW = '確認待ち'                      # DESIGN §11「User approval required: 確認待ち」
CONFIRM_KEYS = {'return', 'enter', 'space'}
NEEDS = {'observe': (), 'click': ('target',), 'type': ('target', 'text'), 'key': ('keys',), 'type_focused': ('text',),
         'window': ('size',), 'restart': (), 'operator': ('instruction',)}
EDGE = {'top': '上端', 'left': '左端', 'right': '右端'}
SCALE = float(os.environ.get('QA_TIME_SCALE') or 1)   # 混んだ機械で待ち時間を全部 N 倍にする（判定の基準は変えない）


def norm(s):
    return re.sub(r'\s+', ' ', unicodedata.normalize('NFKC', '' if s is None else str(s))).strip()


def hit(want, have, how):
    have = norm(have)
    if how == 'regex':
        return re.search(want, have) is not None
    want = norm(want)
    return have == want if how == 'exact' else want in have


def matches(e, m):
    role = m.get('role')
    if role and e.get('role') not in ([role] if isinstance(role, str) else role):
        return False
    if not role and e.get('role') in HIDDEN:
        return False
    how = m.get('match', 'exact')
    if 'label' in m and not hit(m['label'], e.get('label'), how):
        return False
    if 'value' in m and not hit(m['value'], e.get('value'), how):
        return False
    return 'text' not in m or hit(m['text'], e.get('label'), how) or hit(m['text'], e.get('value'), how)


def desc(m):
    role = m.get('role')
    out = ['|'.join([role] if isinstance(role, str) else role)] if role else []
    out += [f'{k}="{m[k]}"' for k in ('label', 'value', 'text') if k in m]
    return ' '.join(out + ([f'({m["match"]})'] if m.get('match', 'exact') != 'exact' else []))


def frame(e):
    f = e.get('frame') or {}
    return (f.get('x', 0), f.get('y', 0), f.get('w', 0), f.get('h', 0))


def brief(e):
    x, y, w, h = frame(e)
    return f'{e.get("role")} "{norm(e.get("label") or e.get("value"))[:40]}" @{x:g},{y:g} {w:g}×{h:g}'


def shown(snap, word):
    return any(word in norm(e.get('label')) or word in norm(e.get('value')) for e in snap['els'])


def attr(e, k):
    if k == 'frame':
        return frame(e)
    return f'{norm(e.get("label"))} / {norm(e.get("value"))}' if k == 'text' else norm(e.get(k))


def region(els, win, edge):
    """窓の端に貼り付いた帯（Header / Sidebar / Agent rail）。AX に無ければ None。win = AXWindow の frame"""
    wx, wy, ww, wh = win
    near = lambda a, b: abs(a - b) <= 4
    found = []
    for e in els:
        x, y, w, h = frame(e)
        if e.get('role') in HIDDEN or w <= 0 or h <= 0:
            continue
        if (edge == 'top' and near(x, wx) and near(x + w, wx + ww) and h < wh / 2
                or edge == 'left' and near(x, wx) and w < ww / 2 and h >= wh / 2
                or edge == 'right' and near(x + w, wx + ww) and w < ww / 2 and h >= wh * 0.4):
            found.append(e)
    if not found:
        return None
    if edge == 'top':   # 一番上の全幅の帯。同じ高さに入れ子があれば背の高い方（panel そのもの）
        return min(found, key=lambda e: (frame(e)[1], -frame(e)[3]))
    return max(found, key=lambda e: (frame(e)[2], frame(e)[3]))


def chrome(e, win):
    """macOS の title bar の釦（閉じる・最小化・zoom）。Kangae の物ではないので名前の検査から外す"""
    return bool(win) and frame(e)[1] + frame(e)[3] <= win[1] + 28


def cell(s):
    return str(s).replace('|', '｜').replace('\n', ' ')


def home(s):
    return str(s).replace(str(Path.home()), '~')


class Driver:
    def __init__(self, exe):
        self.exe, self.session = exe, 'qa'

    def call(self, tool, args, timeout=120):
        """(data, err)。拒否は stdout の JSON（status: refused）と exit 1 で来るので stdout を先に読む（cua-probe §2）。
        session は JSON の引数（CLI の flag ではない）。無いと次の呼び出しで token が全部 stale になる（cua-probe §1）"""
        try:
            p = subprocess.run([self.exe, 'call', tool, json.dumps(dict(args, session=self.session), ensure_ascii=False)],
                               capture_output=True, text=True, timeout=timeout * SCALE)
        except subprocess.TimeoutExpired:
            return None, f'{tool}: cua-driver が {timeout * SCALE:.0f} 秒応答しない'
        out, data = p.stdout.strip(), None
        if '{' in out:
            try:
                data = json.loads(out[out.index('{'):out.rindex('}') + 1])
            except ValueError:
                pass
        if isinstance(data, dict) and data.get('status') == 'refused':
            r = data.get('refusal') or {}
            return None, f'{tool}: cua-driver が拒否（{r.get("code")}）: {r.get("message")}'
        if p.returncode != 0:
            return None, f'{tool}: exit {p.returncode}: {(out or p.stderr.strip())[:300]}'
        return (data if isinstance(data, dict) else {}), None


class Run:
    """台本 1 本。Kangae を一時の KANGAE_DATA_DIR で 1 つ起こし、手を順に打ち、手ごとに判定・写真・AX の木を残す"""

    def __init__(self, sc, args, out):
        self.sc, self.args, self.dir = sc, args, out / sc['id']
        self.dir.mkdir(parents=True, exist_ok=True)
        self.tmp = Path(tempfile.mkdtemp(prefix='kangae-qa-'))
        self.data, self.fx = self.tmp / 'data', self.tmp / 'fixtures'
        self.data.mkdir()
        self.fx.mkdir()
        for name, body in sc.get('fixtures', {}).items():
            (self.fx / name).write_text(body, encoding='utf-8')
        self.drv = Driver(args.driver)
        self.log = open(self.dir / 'kangae.log', 'ab')
        self.proc = self.wid = self.bounds = self.size = self.last = None
        self.size_bad, self.snaps, self.notes = '', {}, []

    # ---- 起動・窓 ----
    def alive(self):
        return self.proc is not None and self.proc.poll() is None

    def died(self):
        return f'Kangae が終了した（exit {self.proc.returncode}・kangae.log）'

    def launch(self):
        """'' か、起こせなかった理由"""
        self.proc = subprocess.Popen([self.args.bin], env=dict(os.environ, KANGAE_DATA_DIR=str(self.data)), cwd=self.tmp,
                                     stdin=subprocess.DEVNULL, stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True)
        self.drv.session = f'qa-{self.proc.pid}'
        deadline = time.time() + 60 * SCALE
        while time.time() < deadline:
            if not self.alive():
                return self.died()
            mine = [w for w in self.windows() if min(w['bounds']['width'], w['bounds']['height']) >= 200]
            if mine:   # 起動直後は title "" の 0×0 の一時窓も出る（accesskit-2026-10-16.md）
                best = max(mine, key=lambda w: w['bounds']['width'] * w['bounds']['height'])
                self.wid, self.bounds = best['window_id'], best['bounds']
                if self.size:
                    self.resize()
                return ''
            time.sleep(1)
        return f'{60 * SCALE:.0f} 秒待っても Kangae の窓が list_windows に出ない'

    def windows(self):
        d, _ = self.drv.call('list_windows', {'pid': self.proc.pid})
        return [w for w in (d or {}).get('windows', []) if w.get('pid') == self.proc.pid and w.get('bounds') and w.get('window_id')]

    def window_for(self, st):
        if st.get('in') != 'dialog':
            return self.wid
        others = [w for w in self.windows() if w['window_id'] != self.wid and min(w['bounds']['width'], w['bounds']['height']) >= 100]
        return max(others, key=lambda w: w.get('z_index') or -1)['window_id'] if others else self.wid

    def resize(self):
        W, H = self.size['width'], self.size['height']
        _, err = self.drv.call('set_window_frame', {'pid': self.proc.pid, 'window_id': self.wid, 'x': 0,
                                                    'y': max(self.bounds['y'], 25), 'width': W, 'height': H})
        got = next((w['bounds'] for w in self.windows() if w['window_id'] == self.wid), None)
        self.bounds = got or self.bounds
        if got and abs(got['width'] - W) <= 2 and abs(got['height'] - H) <= 2:
            self.size_bad = ''
        else:
            self.size_bad = f'窓が {W}×{H} にならない（実 {got["width"]:g}×{got["height"]:g}）' if got else f'窓の大きさを読み戻せない（{err}）'

    def stop(self):
        if self.alive():
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(5)

    def close(self):
        if self.proc is not None:
            self.stop()
            self.drv.call('end_session', {})
        self.log.close()
        if not self.args.keep_data:
            shutil.rmtree(self.tmp, ignore_errors=True)

    # ---- 観る ----
    def snapshot(self, wid):
        """{els, complete, why, win}。AX の走査は 4 秒、truncated / degraded なら 8 秒で 1 回だけ撮り直す（cua-probe §3・§4。どちらも QA_TIME_SCALE 倍）"""
        why, els, win = '', [], None
        for ms in (4000, 8000):
            d, err = self.drv.call('get_window_state', {'pid': self.proc.pid, 'window_id': wid, 'include_screenshot': False,
                                                        'timeout_ms': min(int(ms * SCALE), 120000)})
            if err:
                why = err
                continue
            els = d.get('elements') or []
            win = next((frame(e) for e in els if e.get('role') == 'AXWindow'), None)
            if not (d.get('truncated') or d.get('degraded')):
                self.last = {'els': els, 'complete': True, 'why': f'木は完全・要素 {len(els)}', 'win': win}
                return self.last
            why = f'木が不完全（{str(d.get("truncation_reason") or d.get("degraded_reason"))[:120]}・要素 {len(els)}）'
        self.last = {'els': els, 'complete': False, 'why': why, 'win': win}
        return self.last

    def resolve(self, st):
        """target の要素 1 つ（主窓で探す）。(snap, 要素, None) か (snap, None, (判定, 理由))。揃うまで timeout_s 見続ける"""
        loc, deadline = st['target'], time.time() + st.get('timeout_s', 10) * SCALE
        while True:
            snap = self.snapshot(self.wid)
            found = [e for e in snap['els'] if matches(e, loc)]
            if snap['complete'] and len(found) == 1:
                return snap, found[0], None
            if time.time() >= deadline or not self.alive():
                break
            time.sleep(0.5)
        if not self.alive():
            return snap, None, (FAIL, self.died())
        if not snap['complete']:
            return snap, None, (NA, f'{desc(loc)} を探せない: {snap["why"]}')
        if not found:
            return snap, None, (FAIL, f'label が無い: {desc(loc)}（{snap["why"]}・何も押していない）')
        return snap, None, (FAIL, f'{desc(loc)} が {len(found)} 件（推測で選ばない・何も押していない）: ' + ' / '.join(map(brief, found[:3])))

    def check(self, exp, snap):
        """期待 1 つずつ (判定, 理由)。木が不完全でも「在る」は証明できる（present の PASS・absent の FAIL）"""
        els, done, why = snap['els'], snap['complete'], snap['why']
        look = lambda m: [e for e in els if matches(e, m)]
        res = []
        for m in exp.get('present', []):
            n, lo, hi = len(look(m)), m.get('min', 1), m.get('max')
            if hi is not None and n > hi:
                res.append((FAIL, f'多すぎる: {desc(m)} が {n} 件（上限 {hi}）'))
            elif n >= lo:
                res.append((PASS, f'在る: {desc(m)}（{n} 件）'))
            else:
                res.append((FAIL if done else NA, f'無い: {desc(m)}（{n} 件・下限 {lo}・{why}）'))
        anchored = any(exp.get(k) for k in ('present', 'frames', 'compare')) or any(not r.get('absent') for r in exp.get('regions', []))
        for m in exp.get('absent', []):
            got = look(m)
            if got:
                res.append((FAIL, f'出てはいけない物が出た: {desc(m)}（{len(got)} 件・例 {brief(got[0])}）'))
            elif not done:
                res.append((NA, f'出ていない: {desc(m)}（{why}）'))
            elif not anchored:   # 在る物を 1 つも見ていない「無い」は、文字を AX に出さない画面（2026-10-17 の main: 見出しも説明も AX に無い）でも成り立つ
                res.append((NA, f'出ていない: {desc(m)}（同じ手に present が無く、文字が AX に出ていない画面と区別できない）'))
            else:
                res.append((PASS, f'出ていない: {desc(m)}'))
        for f in exp.get('frames', []):
            got = look(f['of'])
            if len(got) != 1:
                res.append((FAIL if done else NA, f'{desc(f["of"])} が {len(got)} 件で大きさを測れない（{why}）'))
                continue
            dims, tol = dict(zip('xywh', frame(got[0]))), f.get('tol', 2)
            bad = [f'{k}={dims[k]:g}（期待 {f[k]:g}±{tol:g}）' for k in 'wh' if k in f and abs(dims[k] - f[k]) > tol]
            res.append((FAIL if bad else PASS, f'{desc(f["of"])}: ' + ('・'.join(bad) or '・'.join(f'{k}={dims[k]:g}' for k in 'wh' if k in f))))
        b = self.bounds
        win = snap['win'] or (b and (b['x'], b['y'], b['width'], b['height']))
        for r in exp.get('regions', []):
            if self.size_bad or not done or not win:
                res.append((NA, f'{r["name"]}: {self.size_bad or why or "窓の frame が無い"}'))
                continue
            e = region(els, win, r['edge'])
            if r.get('absent'):
                res.append((FAIL, f'{r["name"]}: 帯が在る {brief(e)}') if e else
                           (PASS if anchored else NA, f'{r["name"]}: 窓の{EDGE[r["edge"]]}に帯が無い' + ('' if anchored else '（同じ手に present が無く、AX に出ていないだけと区別できない）')))
            elif e is None:
                res.append((NA, f'{r["name"]}: 窓の{EDGE[r["edge"]]}に貼り付いた AX の要素が無い（frame で測れない）'))
            else:
                k = 'w' if 'w' in r else 'h'
                v, tol = frame(e)[2 if k == 'w' else 3], r.get('tol', 2)
                res.append((PASS if abs(v - r[k]) <= tol else FAIL, f'{r["name"]}: {k}={v:g}（期待 {r[k]:g}±{tol:g}・{brief(e)}）'))
        if 'text_min_h' in exp:
            n = exp['text_min_h']   # h:1 は画面外に仮想化された行（get_window_state の説明）なので数えない
            texts = [e for e in els if e.get('role') == 'AXStaticText' and norm(e.get('label') or e.get('value')) and frame(e)[3] > 1]
            small = [e for e in texts if frame(e)[3] < n]
            if small:
                res.append((FAIL, f'高さ {n:g} 未満の文字 {len(small)} 件: ' + ' / '.join(map(brief, small[:3]))))
            else:
                res.append((PASS if done and texts else NA, f'文字 {len(texts)} 件は全部 高さ {n:g} 以上' + ('' if done else f'（{why}）')))
        if 'named' in exp:
            roles = [exp['named']] if isinstance(exp['named'], str) else exp['named']
            ctl = [e for e in els if e.get('role') in roles and not chrome(e, win)]
            nameless = [e for e in ctl if not norm(e.get('label'))]
            if nameless:
                res.append((FAIL, f'名前の無い {"・".join(sorted({e["role"] for e in nameless}))} {len(nameless)} 件: ' + ' / '.join(map(brief, nameless[:3]))))
            else:
                res.append((PASS if done and ctl else NA, f'{"・".join(roles)} の {len(ctl)} 件は全部名前を持つ' + ('' if done else f'（{why}）')))
        for c in exp.get('compare', []):
            old = self.snaps.get(c['since']) or {'els': [], 'complete': False}
            a, now = [e for e in old['els'] if matches(e, c['of'])], look(c['of'])
            if len(a) != 1 or len(now) != 1:
                res.append((FAIL if done and old['complete'] else NA, f'{desc(c["of"])} が {c["since"]} で {len(a)} 件・今 {len(now)} 件（比べられない）'))
                continue
            va, vb = attr(a[0], c['attr']), attr(now[0], c['attr'])
            same = all(abs(p - q) <= c.get('tol', 1) for p, q in zip(va, vb)) if c['attr'] == 'frame' else va == vb
            res.append((PASS if same == c['same'] else FAIL, f'{desc(c["of"])} の {c["attr"]} が {c["since"]} から{"同じ" if same else "変わった"}（{va} → {vb}）'))
        for f in exp.get('files', []):
            p = Path(f['path'])
            body = p.read_text(encoding='utf-8', errors='replace') if p.is_file() else None
            if body is None:
                res.append((FAIL, f'file が無い: {p.name}'))
            else:
                ok = re.search(f['regex'], body) is not None
                res.append((PASS if ok else FAIL, f'{p.name} に /{f["regex"]}/ が' + ('在る' if ok else f'無い（先頭 {body[:80]!r}）')))
        return res

    def watch(self, st):
        exp = st.get('expect') or {'review': ['人の手の効き目（expect の無い operator の手）']}
        time.sleep(st.get('settle_s', 0) * SCALE)
        deadline = time.time() + st.get('timeout_s', 10) * SCALE
        while True:
            snap = self.snapshot(self.window_for(st))
            res = self.check(exp, snap)
            if all(s == PASS for s, _ in res) or time.time() >= deadline or not self.alive():
                break
            time.sleep(0.5)
        self.snaps[st['id']] = snap
        if not self.alive():
            res.append((FAIL, self.died()))
        return res + [(NA, f'人が写真で見る: {r}') for r in exp.get('review', [])]

    # ---- 動かす ----
    def sent(self, err):
        """混雑時の AX の失敗と応答なしは、押せていても返る事がある（cua-probe §8）ので期待で判定する。
        それ以外（拒否・引数の誤り: 2026-10-17 に click が pid 無しで `Missing required integer field: pid`）は届いていないので FAIL"""
        if not err:
            return None
        if 'AX action failed' in err or '応答しない' in err:
            self.notes.append(f'{err}（届いた可能性がある・期待で判定）')
            return None
        return FAIL, err

    def act(self, st):
        """操作する。(判定, 理由) を返したら期待は見ない。None なら期待を見に行く"""
        do, pid = st['do'], self.proc.pid
        if do == 'observe':
            return None
        if do == 'operator':
            if not self.args.interactive:
                return NA, f'人の手: {st["instruction"]}（--interactive なら人が済ませて先へ進む）'
            print(f'\n[{self.sc["id"]} {st["id"]}] 人の手: {st["instruction"]}', flush=True)
            if input('済んだら Enter・やめるなら q: ').strip().lower() == 'q':
                return NA, '人の手の所で止めた'
            self.notes.append('人が実施（機械では測っていない）')
            return None   # 人が済ませた = 操作した。expect が無ければ watch が 未測 を 1 つ返す（先へは進む）
        if do == 'window':
            self.size = st['size']
            self.resize()
            return None
        if do == 'restart':
            self.drv.call('hotkey', {'pid': pid, 'window_id': self.wid, 'keys': ['cmd', 'q'], 'delivery_mode': 'foreground'})
            try:
                self.proc.wait(20 * SCALE)
            except subprocess.TimeoutExpired:
                self.notes.append('Cmd+Q で 20 秒以内に終わらない → SIGTERM')
                self.stop()
            bad = self.launch()
            return (FAIL if self.proc.poll() is not None else NA, f'起動し直せない: {bad}') if bad else None
        if do == 'key':
            keys, wid = st['keys'], self.window_for(st)
            if CONFIRM_KEYS & set(keys) and shown(self.snapshot(self.wid), REVIEW):
                return FAIL, f'「{REVIEW}」が出ている所で {"+".join(keys)} は送らない（承認の確定になりうる）'
            tool, a = ('press_key', {'key': keys[0]}) if len(keys) == 1 else ('hotkey', {'keys': keys})
            _, err = self.drv.call(tool, dict(a, pid=pid, window_id=wid, delivery_mode='foreground'))
            return self.sent(err)
        if do == 'type_focused':   # element_token 無し = pid の今の focus に打つ
            _, err = self.drv.call('type_text', {'pid': pid, 'text': st['text']})
            return self.sent(err)
        for attempt in (1, 2):   # click / type。古い token は撮り直して 1 回だけ
            snap, el, bad = self.resolve(st)
            if bad:
                return bad
            said = f'{norm(el.get("label"))} {norm(el.get("value"))}'.strip()
            if do == 'click':
                if NEVER_PRESS.search(said):
                    return FAIL, f'runner は承認・許可の label を押さない: "{said}"（人の手 operator にする）'
                if HIGH_IMPACT.search(said):
                    if st.get('high_impact') != 'review_only':
                        return FAIL, f'高影響の "{said}" を high_impact の宣言なしに押そうとした（台本の誤り・押していない）'
                    if shown(snap, REVIEW):
                        return FAIL, f'「{REVIEW}」が出ている所で "{said}" は押さない（確定になりうる）'
                _, err = self.drv.call('click', {'pid': pid, 'element_token': el['element_token']})   # pid は token と一緒でも要る（0.33.4）
            else:
                _, err = self.drv.call('type_text', {'pid': pid, 'element_token': el['element_token'], 'text': st['text']})
            if not (err and 'stale' in err and attempt == 1):
                return self.sent(err)

    def photo(self, st, name):
        if not self.alive():
            return '（写真なし: Kangae が終了）'
        path = self.dir / name
        _, err = self.drv.call('get_window_state', {'pid': self.proc.pid, 'window_id': self.window_for(st),
                                                    'include_accessibility_tree': False, 'screenshot_out_file': str(path)})
        return name if path.is_file() else f'（写真なし: {err or "file が出来ない"}）'

    def burst(self, st, t0):
        """操作の直後・期待を見る前に写真を N 枚（skeleton・animation の途中を人が見る）。撮れた時刻を notes に残す:
        間隔が animation（180〜240ms・DESIGN §15.2-1）より長ければ、途中が写らなくても動いていない証拠にはならない"""
        shots = []
        for i in range(1, st.get('burst', 0) + 1):
            shots.append(self.photo(st, f'{st["evidence"][:-4]}-b{i}.png'))
            shots[-1] += f'（+{time.time() - t0:.2f}s）'
        return shots

    def sub(self, v):
        if isinstance(v, str):
            return v.replace('${FIRST_WORDS}', self.sc['first_words']).replace('${FIXTURES}', str(self.fx)).replace('${DATA_DIR}', str(self.data))
        if isinstance(v, list):
            return [self.sub(x) for x in v]
        return {k: self.sub(x) for k, x in v.items()} if isinstance(v, dict) else v

    def step(self, st):
        st, self.notes, self.last = self.sub(st), [], None
        bad = self.act(st)
        self.acted = bad is None
        frames = self.burst(st, time.time()) if self.acted else []
        res = [bad] if bad else self.watch(st)
        status = FAIL if any(s == FAIL for s, _ in res) else NA if any(s == NA for s, _ in res) else PASS
        shot = ' '.join([self.photo(st, st['evidence'])] + frames)
        if st.get('high_impact') and status == FAIL and self.alive():
            self.stop()
            res.append((FAIL, f'「{REVIEW}」を確かめられないので Kangae を止めた'))
        if self.last:   # menu bar（AXMenu*）は落とす: Kangae の物ではなく、Apple menu に login 名と最近の file 名が出る
            keep = ('role', 'label', 'value', 'frame', 'depth')
            (self.dir / f'{st["id"]}.ax.json').write_text(json.dumps({'complete': self.last['complete'], 'why': self.last['why'],
                'elements': [{k: e.get(k) for k in keep if k in e} for e in self.last['els'] if not str(e.get('role')).startswith('AXMenu')]},
                ensure_ascii=False, indent=0), encoding='utf-8')
        return {'id': st['id'], 'intent': st['intent'], 'do': st['do'], 'status': status,
                'checks': [{'status': s, 'text': home(t)} for s, t in res], 'notes': [home(n) for n in self.notes], 'photo': shot}

    def run(self):
        steps, out = self.sc['steps'], []
        missing = [a for a in self.sc.get('requires_apps', []) if not Path(a).exists()]
        stop = f'要る app が無い: {", ".join(missing)}' if missing else ''
        if not stop:
            bad = self.launch()
            first = (FAIL if not self.alive() else NA, bad) if bad else (PASS, f'pid {self.proc.pid}・窓 {self.wid}（{self.bounds["width"]:g}×{self.bounds["height"]:g}）')
            out.append({'id': 's0', 'intent': 'Kangae を一時の KANGAE_DATA_DIR で起動して窓を待つ', 'do': 'launch', 'status': first[0],
                        'checks': [{'status': first[0], 'text': home(first[1])}], 'notes': [], 'photo': ''})
            stop = '起動できない' if bad else ''
        for st in steps:
            if stop:
                out.append({'id': st['id'], 'intent': st['intent'], 'do': st['do'], 'status': NA,
                            'checks': [{'status': NA, 'text': f'先へ進まない: {stop}'}], 'notes': [], 'photo': ''})
                continue
            r = self.step(st)
            out.append(r)
            if r['status'] == FAIL:
                stop = f'{st["id"]} が FAIL'
            elif not self.acted:   # 人の手の所・target を探せなかった所: 操作していない状態の上で先へ進まない
                stop = f'{st["id"]} の操作をしていない'
            elif not self.alive():
                stop = 'Kangae が終了した'
        return verdict(self.sc, out)


def verdict(sc, steps):
    st = [s['status'] for s in steps]
    return {'id': sc['id'], 'title': sc['title'], 'first_words': sc['first_words'], 'steps': steps,
            'status': FAIL if FAIL in st else NA if NA in st else PASS}


# ---- 台本の検査（--check と、走らせる前） ----
def problems_of(sc, stem, corpus, v):
    """形（v = jsonschema の validator・無ければ飛ばす）と意味の破れ"""
    out = [f'schema: /{"/".join(map(str, e.absolute_path))}: {e.message}' for e in v.iter_errors(sc)] if v else []
    if not isinstance(sc, dict) or not isinstance(sc.get('steps'), list) or not all(isinstance(s, dict) for s in sc['steps']):
        return out or ['steps が object の配列でない']
    if sc.get('id') != stem:
        out.append(f'id "{sc.get("id")}" が file 名と違う')
    own = norm(' '.join([str(sc.get('first_words', ''))] + [str(x) for x in (sc.get('fixtures') or {}).values()] + [str(s.get('text', '')) for s in sc['steps']]))
    seen, photos = [], set()
    for s in sc['steps']:
        sid, do, exp, tgt = s.get('id'), s.get('do'), s.get('expect') or {}, s.get('target') or {}
        if sid in seen:
            out.append(f'{sid}: 手の id が重なっている')
        if do not in NEEDS:
            out.append(f'{sid}: do "{do}" を知らない')
        out += [f'{sid}: {do} には {k} が要る' for k in NEEDS.get(do, ()) if k not in s]
        if do != 'operator' and not exp:
            out.append(f'{sid}: expect が無い（測らない手は置かない）')
        if s.get('evidence') in photos:
            out.append(f'{sid}: 写真の名前 {s.get("evidence")} が重なっている')
        photos.add(s.get('evidence'))
        out += [f'{sid}: compare の since "{c.get("since")}" が前の手に無い' for c in exp.get('compare', []) if c.get('since') not in seen]
        words = [tgt[k] for k in ('label', 'value', 'text') if k in tgt]
        if tgt.get('match', 'exact') != 'regex':
            out += [f'{sid}: target の "{w}" が仕様（{"・".join(SPEC)}）にも台本の fixture・打つ文字にも無い（label は仕様の文言から書く）'
                    for w in words if norm(w) not in corpus and norm(w) not in own]
        said = ' '.join(words)
        if do == 'click' and NEVER_PRESS.search(said):
            out.append(f'{sid}: 承認・許可は runner に押させない（operator の手にする）')
        if do == 'click' and HIGH_IMPACT.search(said) and s.get('high_impact') != 'review_only':
            out.append(f'{sid}: "{said}" は高影響。high_impact: review_only と「{REVIEW}」の期待が要る')
        if s.get('high_impact') and not any(REVIEW in str(m.get('label', '')) + str(m.get('text', '')) for m in exp.get('present', [])):
            out.append(f'{sid}: high_impact の手は expect.present に「{REVIEW}」を書く（DESIGN §11）')
        if s.get('burst') and not exp.get('review'):
            out.append(f'{sid}: burst の写真は人が見る物。expect.review に何を見るかを書く')
        seen.append(sid)
    return out


def self_test(sc, stem, corpus, v):
    """負の対照: 通った 1 本を 1 か所ずつ壊し、狙った規則で赤くなる事を毎回確かめる（赤くならない規則は守っていない）"""
    extra = lambda **kw: dict({'id': 's99', 'intent': '壊す', 'do': 'click', 'expect': {'present': [{'text': 'x'}]}, 'evidence': '99-x.png'}, **kw)

    def no_expect(s):
        s['steps'][0].pop('expect', None)
        s['steps'][0].update(do='observe')

    mutants = {
        'id と file 名の食い違い': lambda s: s.update(id='qa-99-other'),
        '写真の名前の無い手': lambda s: s['steps'][0].pop('evidence'),
        '測らない手（expect 無し）': no_expect,
        '仕様に無い label を押す': lambda s: s['steps'].append(extra(target={'label': '仕様に無い押し釦'})),
        '高影響を宣言なしに押す': lambda s: s['steps'].append(extra(target={'label': '送る'})),
        '承認を runner に押させる': lambda s: s['steps'].append(extra(target={'label': '承認'}, high_impact='review_only', expect={'present': [{'text': REVIEW}]})),
        '後ろの手を since に指す': lambda s: s['steps'].insert(0, extra(id='s98', do='observe', evidence='98-x.png',
                                                                     expect={'compare': [{'of': {'text': 'x'}, 'since': s['steps'][0]['id'], 'attr': 'value', 'same': True}]})),
        '見る物を書かない burst': lambda s: s['steps'].append(extra(do='observe', burst=3)),
    }
    out = []
    for name, f in mutants.items():
        m = copy.deepcopy(sc)
        f(m)
        if not problems_of(m, stem, corpus, v):
            out.append(f'自己検査: 「{name}」に壊しても赤くならない（その規則は守られていない）')
    return out, len(mutants)


def judge_test():
    """判定の自己検査: 作った木で check() が狙った判定（PASS / FAIL / 未測）を返すか。Kangae も driver も要らない"""
    el = lambda role, label, f: {'role': role, 'label': label, 'frame': dict(zip('xywh', f))}
    win = (0, 0, 1440, 900)
    tree = [el('AXWindow', 'Kangae', win), el('AXButton', '', (6, 6, 14, 16)),   # 2 つ目は title bar の macOS の釦
            el('AXGroup', '', (0, 28, 1440, 56)), el('AXGroup', '', (0, 84, 232, 816)), el('AXButton', '作る', (900, 300, 56, 36)),
            el('AXStaticText', '小さい字', (10, 100, 50, 9)), el('AXTextArea', '何を作りたいですか？', (300, 200, 600, 88))]
    snap = {'els': tree, 'complete': True, 'why': '作った木', 'win': win}
    part = dict(snap, complete=False, why='木が不完全')
    r = Run.__new__(Run)
    r.bounds, r.size_bad, r.snaps = None, '', {}
    cases = [
        ({'present': [{'role': 'AXButton', 'label': '作る'}]}, snap, PASS),
        ({'present': [{'label': '無い釦'}]}, snap, FAIL),
        ({'present': [{'label': '無い釦'}]}, part, NA),
        ({'present': [{'role': 'AXTextArea', 'label': '何を作りたいですか?'}]}, snap, PASS),   # NFKC: 全角の？と半角の?
        ({'present': [{'role': 'AXButton', 'min': 0, 'max': 1}]}, snap, FAIL),
        ({'absent': [{'text': '作る', 'match': 'contains'}]}, part, FAIL),
        ({'absent': [{'text': '送信済み', 'match': 'contains'}]}, part, NA),
        ({'absent': [{'text': '送信済み', 'match': 'contains'}]}, snap, NA),   # 在る物を見ていない「無い」
        ({'present': [{'label': '作る'}], 'absent': [{'text': '送信済み', 'match': 'contains'}]}, snap, PASS),
        ({'frames': [{'of': {'label': '作る'}, 'h': 36}]}, snap, PASS),
        ({'frames': [{'of': {'label': '作る'}, 'h': 40}]}, snap, FAIL),
        ({'regions': [{'name': 'Header', 'edge': 'top', 'h': 56}]}, snap, PASS),
        ({'regions': [{'name': 'Sidebar', 'edge': 'left', 'w': 216}]}, snap, FAIL),
        ({'regions': [{'name': 'Agent rail', 'edge': 'right', 'w': 296}]}, snap, NA),
        ({'regions': [{'name': 'Agent rail', 'edge': 'right', 'absent': True}]}, snap, NA),
        ({'present': [{'label': '作る'}], 'regions': [{'name': 'Agent rail', 'edge': 'right', 'absent': True}]}, snap, PASS),
        ({'text_min_h': 12}, snap, FAIL),
        ({'named': ['AXButton']}, snap, PASS),
        ({'named': ['AXGroup']}, snap, FAIL),
    ]
    out = []
    for exp, s, want in cases:
        got = [v for v, _ in r.check(exp, s)]
        status = FAIL if FAIL in got else NA if NA in got else PASS
        if status != want:
            out.append(f'判定の自己検査: {json.dumps(exp, ensure_ascii=False)} が {status}（期待 {want}）')
    return out, len(cases)


def load(paths, need_schema):
    try:
        from jsonschema import Draft202012Validator
        schema = json.loads(SCHEMA.read_text(encoding='utf-8'))
        Draft202012Validator.check_schema(schema)
        v = Draft202012Validator(schema)
    except ImportError:
        if need_schema:
            return [], None, None, ['jsonschema が無い（schema 検査に要る。pip3 install jsonschema）']
        print('qa_run: WARN jsonschema が無いので schema 検査は飛ばす（意味の検査だけ）', file=sys.stderr)
        v = None
    corpus = norm('\n'.join([(REPO / f).read_text(encoding='utf-8') for f in SPEC] + PLANNED_LABELS))
    scs, problems = [], []
    for p in paths:
        try:
            sc = json.loads(p.read_text(encoding='utf-8'))
        except (OSError, ValueError) as e:
            problems.append(f'{p}: 読めない: {e}')
            continue
        bad = problems_of(sc, p.stem, corpus, v)
        problems += [f'{p.name}: {b}' for b in bad]
        scs.append((sc, p, not bad))
    return scs, corpus, v, problems


def stdout(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=60 * SCALE).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return ''


def write(out, meta, results):
    md = [f'# QA run {meta["utc"]}', '',
          f'- Kangae: `{meta["bin"]}`（更新 {meta["bin_mtime"]}）・台本と runner の HEAD `{meta["head"]}`・{meta["driver"]}・1 分 load {meta["load"]}・QA_TIME_SCALE {meta["scale"]}',
          '- PASS = 木が完全で期待が全部成立 / FAIL = 木が完全なのに label が無い・2 件以上・期待外れ / 未測 = 測れなかった（理由つき）。写真と AX の木（<手>.ax.json）は台本の dir',
          '', '| 台本 | 判定 | PASS | FAIL | 未測 |', '|---|---|---:|---:|---:|']
    for r in results:
        n = {k: sum(s['status'] == k for s in r['steps']) for k in (PASS, FAIL, NA)}
        md.append(f'| {r["id"]} | {r["status"]} | {n[PASS]} | {n[FAIL]} | {n[NA]} |')
    for r in results:
        md += ['', f'## {r["id"]} — {r["title"]}', '', f'一言目: 「{r["first_words"]}」', '',
               '| 手 | 何をする | 判定 | 理由 | 写真 |', '|---|---|---|---|---|']
        for s in r['steps']:
            why = [c['text'] for c in s['checks'] if c['status'] != PASS] or [c['text'] for c in s['checks']]
            md.append(f'| {s["id"]} | {cell(s["intent"])} | {s["status"]} | {cell("・".join(why + s["notes"]))} | {s["photo"]} |')
    (out / 'RESULTS.md').write_text('\n'.join(md) + '\n', encoding='utf-8')
    (out / 'results.json').write_text(json.dumps({'meta': meta, 'results': results}, ensure_ascii=False, indent=1), encoding='utf-8')


def main():
    ap = argparse.ArgumentParser(description='Kangae の QA 台本を cua-driver の AX 経路で走らせる（docs/qa/README.md）')
    ap.add_argument('scenarios', nargs='+', type=Path)
    ap.add_argument('--check', action='store_true', help='形と意味の検査と自己検査だけ（Kangae を起こさない）')
    ap.add_argument('--bin', default=os.environ.get('KANGAE_BIN'), help='Kangae の実行 file（既定 target/release → target/debug）')
    ap.add_argument('--driver', default=os.environ.get('CUA_DRIVER') or str(Path.home() / '.local/bin/cua-driver'))
    ap.add_argument('--out', type=Path, help='既定 docs/internal/qa-runs/<UTC>/')
    ap.add_argument('--interactive', action='store_true', help='operator の手で止まらず、人が済ませるのを待つ')
    ap.add_argument('--keep-data', action='store_true', help='一時の KANGAE_DATA_DIR を消さない')
    a = ap.parse_args()
    scs, corpus, v, problems = load(a.scenarios, need_schema=a.check)
    if a.check:
        good = next(((sc, p) for sc, p, ok in scs if ok), None)
        armed = 0
        if good and v:
            bad, armed = self_test(good[0], good[1].stem, corpus, v)
            problems += bad
        bad, judged = judge_test()
        problems += bad
        for p in problems:
            print(f'qa_run: {p}', file=sys.stderr)
        if problems:
            return 1
        print(f'qa_run: 台本 {len(scs)} 本の形と意味 ok（台本を {armed} 通りに壊して全部赤・判定の自己検査 {judged} 件も期待どおり）')
        return 0
    if problems:
        for p in problems:
            print(f'qa_run: {p}', file=sys.stderr)
        print('qa_run: 台本が壊れているので走らせない', file=sys.stderr)
        return 2
    a.bin = a.bin or next((str(p) for p in (REPO / 'target/release/kangae-rust-mvp', REPO / 'target/debug/kangae-rust-mvp') if p.is_file()), None)
    if not a.bin or not Path(a.bin).is_file():
        print('qa_run: Kangae の実行 file が無い（--bin か KANGAE_BIN。cargo build --release で target/release に出来る）', file=sys.stderr)
        return 2
    utc = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    out = a.out or REPO / 'docs/internal/qa-runs' / utc
    out.mkdir(parents=True, exist_ok=True)
    have_driver = Path(a.driver).is_file()
    meta = {'utc': utc, 'bin': home(a.bin), 'bin_mtime': datetime.fromtimestamp(Path(a.bin).stat().st_mtime).isoformat(timespec='minutes'),
            'head': stdout(['git', '-C', str(REPO), 'rev-parse', '--short', 'HEAD']) or '?', 'load': round(os.getloadavg()[0], 1),
            'scale': SCALE, 'interactive': a.interactive,
            'driver': stdout([a.driver, '--version']) if have_driver else 'cua-driver 無し'}
    status = stdout([a.driver, 'status']) if have_driver else ''
    results = []
    for sc, _, _ in scs:
        if 'daemon is running' not in status:
            why = f'cua-driver の daemon が動いていない（open -a CuaDriver）: {home(a.driver)}'
            results.append(verdict(sc, [{'id': s['id'], 'intent': s['intent'], 'do': s['do'], 'status': NA,
                                         'checks': [{'status': NA, 'text': why}], 'notes': [], 'photo': ''} for s in sc['steps']]))
        else:
            r = Run(sc, a, out)
            try:
                results.append(r.run())
            finally:
                r.close()
        write(out, meta, results)   # 1 本ごとに書く（途中で止まっても残る）
        print(f'qa_run: {sc["id"]}: {results[-1]["status"]}', flush=True)
    print(f'qa_run: 結果 {home(out / "RESULTS.md")}')
    st = [r['status'] for r in results]
    return 1 if FAIL in st else 3 if NA in st else 0


if __name__ == '__main__':
    sys.exit(main())
