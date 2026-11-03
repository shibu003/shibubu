#!/usr/bin/env python3
"""composer の出力を holdout（実装役に見せない採点集合）の性質で採点する。行ごとに PASS / FAIL / MANUAL を出す。

  cd docs
  python3 -I scripts/check_holdout.py <prompt_id> <生成された AppDefinitionV3.json>
  python3 -I scripts/check_holdout.py --all <dir>   # <dir>/<prompt_id>.json を全部採点し、kind ごとに集計
  python3 -I scripts/check_holdout.py --list        # id<TAB>prompt（compose に流す用）
  python3 -I scripts/check_holdout.py --prompts ~/.kangae-qa/holdout/prompts-v2.md --list   # 版を選ぶ（どの形の前にも置ける）

- prompt と性質の本体は既定で internal/holdout/prompts-v1.md、`--prompts <path>` で別の版（gitignore・repo の外・commit しない・
  実装役に見せない）。この file は性質の「種類」と、全 prompt に掛かる禁止だけを持つ
- 性質 `surface`（任意・v0.14）: 名指しの app の元の画面を残す依頼。source_surface の display が handoff / mirror / composite /
  embedded（native は Kangae が作る画面なので数えない）で、開けない時に handoff へ戻れる事（DESIGN §4.4・v0.14 §3・§11）
- schema と corpus の規則は validate_agent_native_contracts.py の物をそのまま使う（metadata.corpus の 2 行は corpus の帳簿なので除く）
- 質問は metadata.questions[]（v3 の goal に questions が無いため。key が無い = 0 問）。各要素は blocks_step_id で
  止めている action / workflow / source の id か "goal" を名指す（GOAL_FIRST_DATA_CONTRACT §3）
- 前提: compose は鍵・アカウント・端末・OS の許可を 1 つも繋いでいない状態で走る（corpus と同じ）
- 静的検査だけ。画面に出る物（質問・承認画面の中身・空の状態の文）は MANUAL 行で、QA wave で画面の上で確かめる
"""
import ast
import json
import re
import sys
from collections import Counter
from pathlib import Path

from jsonschema import Draft202012Validator

DOCS = Path(__file__).resolve().parents[1]
PROMPTS = DOCS / 'internal/holdout/prompts-v1.md'
MIX = {'local': 6, 'connect': 6, 'risky': 4, 'ambiguous': 4}  # prompts-v1 の配分（brief 2026-10-17）
EFFECTS = {'local_read', 'local_write', 'external_read', 'external_write', 'external_send', 'destructive'}
KEYS = {'id', 'kind', 'prompt', 'components', 'named', 'needs_connection', 'max_questions', 'min_questions',
        'gates', 'forbid_effects', 'local_only', 'local_fallback', 'surface'}
OPTIONAL = {'min_questions', 'surface'}  # v1 は surface を持たない
KEEP_DISPLAY = {'handoff', 'mirror', 'composite', 'embedded'}  # 元の app の画面を残す display（DESIGN §4.4。native は Kangae が作る画面）
SCHEMA = Draft202012Validator(json.loads((DOCS / 'schemas/agent_native_app_v3.schema.json').read_text(encoding='utf-8')))

# corpus の規則は 1 か所のまま使う。validate_agent_native_contracts.py は import すると corpus 全体の検査が
# top-level で走るので、corpus_errors と大文字の定数だけを取り出して実行する
_SRC = DOCS / 'scripts/validate_agent_native_contracts.py'
_ns = {}
exec(compile(ast.Module([n for n in ast.parse(_SRC.read_text(encoding='utf-8')).body
                         if isinstance(n, ast.FunctionDef) and n.name == 'corpus_errors'
                         or isinstance(n, ast.Assign) and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets)],
                        type_ignores=[]), str(_SRC), 'exec'), _ns)
corpus_errors, BANNED = _ns['corpus_errors'], _ns['BANNED']

# 名指しが無いのに id・node key・scope に出たら決め打ちと見るサービス名。一般語と重なる名前（line box note teams threads
# signal zoom drive stripe x meta）は id では拾わない。provider 欄は名指しに無ければ名前を問わず落ちる
KNOWN = set('''
gmail google googleapis outlook office365 hotmail microsoft yahoo icloud apple amazon
slack discord chatwork whatsapp telegram wechat kakaotalk
twitter instagram facebook tiktok linkedin youtube pinterest reddit mastodon bluesky
wordpress hatena ameblo soundcloud spotify apple_music netflix
notion evernote onenote trello asana jira confluence todoist airtable kintone
dropbox onedrive sharepoint google_drive gdrive github gitlab bitbucket figma canva miro
moneyforward zaim freee paypal paypay rakuten mufg smbc mizuho
salesforce hubspot zoho pipedrive
android iphone ios ipados ipad fitbit garmin healthkit switchbot alexa kindle
timetree google_photos flickr anki quizlet duolingo shopify mercari chatgpt openai claude anthropic
'''.split())
NEUTRAL = {'os', 'local', 'system'}          # 端末そのもの。サービスの決め打ちではない
UI_LISTS = {'fields', 'columns', 'options'}  # props で object の配列を持ってよい key（form の欄・表の列・選択肢）
USEFUL = {'form', 'editor', 'list', 'table', 'calendar', 'timeline', 'graph', 'map', 'detail', 'media', 'waveform'}


def tokens(s):
    s = str(s).lower()
    return (set(re.split(r'[^a-z0-9_]+', s)) | set(re.split(r'[^a-z0-9]+', s))) - {''}


def inline_rows(v, path):
    """props の中で object の配列を持つ場所。UI_LISTS の下（form の欄・表の列）は見ない"""
    if isinstance(v, dict):
        return [r for k, x in v.items() if k not in UI_LISTS for r in inline_rows(x, f'{path}.{k}')]
    if isinstance(v, list) and any(isinstance(x, dict) for x in v):
        return [path]
    return []


def load_prompts(path=PROMPTS):
    if not path.exists():
        sys.exit(f'{path} が無い。holdout の本体は repo の外（~/.kangae-qa/holdout/）なので、--prompts でその file を指す')
    ps = [json.loads(b) for b in re.findall(r'^```json\n(.*?)^```', path.read_text(encoding='utf-8'), re.S | re.M)]
    corpus = [json.loads(f.read_text(encoding='utf-8'))['goal']['literal_goal'] for f in sorted((DOCS / 'examples/v3').glob('*.json'))]
    bad = []

    def need(ok, msg):
        if not ok:
            bad.append(msg)
    for p in ps:
        i = p.get('id', '?')
        extra, missing = set(p) - KEYS, KEYS - OPTIONAL - set(p)
        if extra or missing:
            bad.append(f'{i}: key 余分 {sorted(extra)}・不足 {sorted(missing)}')
            continue
        need(re.fullmatch(r'H\d\d', i), f'{i}: id は H01 の形')
        need(p['kind'] in MIX, f"{i}: kind {p['kind']} は {sorted(MIX)} のどれか")
        need(isinstance(p['prompt'], str) and p['prompt'] and p['prompt'] == p['prompt'].strip(), f'{i}: prompt が空か前後に空白')
        need(not any(c in p['prompt'] or p['prompt'] in c for c in corpus), f'{i}: corpus の prompt と重なる')
        need(all(set(g) == {'shape', 'any'} and g['any'] and all(isinstance(c, str) for c in g['any']) for g in p['components']),
             f'{i}: components は [{{"shape", "any": [...]}}]')
        need(all(g and all(isinstance(t, str) and t == t.lower() for t in g) for g in p['named']), f'{i}: named は小文字 token の組の配列')
        # surface の app は名指しでもある（名指しに無いと provider の決め打ちの行が、その app の capability を落とす）
        need(isinstance(p.get('surface', []), list) and all(g in p['named'] for g in p.get('surface', [])), f'{i}: surface の組は named にも置く')
        need(p['max_questions'] in (0, 1) and p.get('min_questions', 0) in (0, 1) and p.get('min_questions', 0) <= p['max_questions'],
             f'{i}: 質問数は 0 か 1（min ≤ max）')
        need(all(set(g) == {'verb', 'effects'} and g['effects'] and set(g['effects']) <= EFFECTS for g in p['gates']),
             f'{i}: gates は [{{"verb", "effects": [effect...]}}]')
        need(set(p['forbid_effects']) <= EFFECTS, f'{i}: forbid_effects に知らない effect')
        need(not {e for g in p['gates'] for e in g['effects']} & set(p['forbid_effects']), f'{i}: gate と forbid_effects が矛盾')
        need(all(isinstance(p[k], bool) for k in ('needs_connection', 'local_only', 'local_fallback')),
             f'{i}: needs_connection / local_only / local_fallback は bool')
        need(not (p['local_only'] and p['needs_connection']), f'{i}: local_only と needs_connection が矛盾')
        need({'local': p['local_only'] and p['max_questions'] == 0, 'connect': p['needs_connection'],
              'risky': bool(p['gates']), 'ambiguous': p['max_questions'] == 1}.get(p['kind']), f"{i}: kind {p['kind']} と性質が合わない")
    need(len({p.get('id') for p in ps}) == len(ps) and len({p.get('prompt') for p in ps}) == len(ps), 'id か prompt が重複')
    need(Counter(p.get('kind') for p in ps) == Counter(MIX), f"配分が {MIX} でない: {dict(Counter(p.get('kind') for p in ps))}")
    if bad:
        sys.exit(f'{path.name} の書式:\n  ' + '\n  '.join(bad))
    return ps


def check(p, d, raw):
    rows = []

    def row(ok, label, detail=''):
        rows.append(('PASS' if ok else 'FAIL', label, detail))
    try:
        SCHEMA.validate(d)
    except Exception as ex:
        row(False, 'schema（agent_native_app_v3）', str(ex).splitlines()[0])
        return rows  # 形が違うと以降は読めない
    row(True, 'schema（agent_native_app_v3）')
    errs = [m for m in corpus_errors(d, raw)[0] if not m.startswith('metadata.corpus')]
    for m in errs:
        row(False, 'corpus の規則', m)
    if not errs:
        row(True, 'corpus の規則（readiness・承認 gate・disabled 理由・鮮度・inline・KPI component・card）')
    goal = d['goal']['literal_goal']
    row(goal.strip() == p['prompt'], 'goal.literal_goal = prompt（そのまま）', '' if goal.strip() == p['prompt'] else goal)

    acts, caps, wfs = d['actions'], d.get('capabilities', []), d.get('workflows', [])
    srcs = {s['id']: s for s in d['sources']}
    nodes = []

    def walk(n, anc):
        nodes.append((n, anc))
        for ch in n.get('children', []):
            walk(ch, anc + [n['component']])
    walk(d['composition']['root'], [])

    def is_local(s):
        return s is not None and s['kind'].startswith('local')

    # ---- prompt ごとの性質 ----
    for g in p['components']:
        hit = [n['key'] for n, _ in nodes if n['component'] in g['any'] and ('binding' in n or 'action_ref' in n)]
        row(hit, f"component {' / '.join(g['any'])}（{g['shape']}・DESIGN §7）", ', '.join(hit) or '無い（binding か action_ref つきで要る）')

    allowed = {t for g in p['named'] for t in g}
    decided = [f"{x['id']}.provider={x['provider']}" for x in caps + d['sources']
               if x.get('provider') and not tokens(x['provider']) & (allowed | NEUTRAL)]
    names = ([x['id'] for x in acts + caps + d['sources'] + wfs] + [n['key'] for n, _ in nodes]
             + [n['binding']['path'] for n, _ in nodes if 'binding' in n] + [c.get('adapter') or '' for c in caps]
             + [s for x in acts + caps for s in x.get('required_scopes', [])]
             + d['agent'].get('policy_scope', []) + [s.get('scope', '') for s in d['sources']])
    decided += [f"{name}（{'・'.join(sorted(hit))}）" for name in names if (hit := tokens(name) & (KNOWN - allowed))]
    row(not decided, f"provider の決め打ちが無い（名指し: {'・'.join(g[0] for g in p['named']) or '無し'}）", ', '.join(dict.fromkeys(decided)))
    for g in p['named']:
        hit = [x['id'] for x in caps + d['sources'] if (tokens(x.get('provider') or '') | tokens(x['id'])) & set(g)]
        row(hit, f'名指しの {g[0]} に繋ぐ（別の先に替えない）', ', '.join(hit) or '名指しした先の capability / source が無い')
    for g in p.get('surface', []):
        acts_by, caps_by = {a['id']: a for a in acts}, {c['id']: c for c in caps}
        mine = []
        for n, _ in nodes:
            if n['component'] != 'source_surface':
                continue
            a = acts_by.get(n.get('action_ref'))
            linked = [a, caps_by.get(a.get('capability_ref')) if a else None, srcs.get((n.get('binding') or {}).get('source_ref'))]
            words = tokens(n['key']) | tokens(n.get('label', '')) | {t for v in n.get('props', {}).values() if isinstance(v, str) for t in tokens(v)}
            words |= {t for x in linked if x for t in tokens(x['id']) | tokens(x.get('provider') or '')}
            if words & set(g):
                mine.append(n)
        keep = [n for n in mine if n.get('props', {}).get('display') in KEEP_DISPLAY]
        row(keep, f'{g[0]} の元の画面を残す（source_surface・display が {" / ".join(sorted(KEEP_DISPLAY))}・v0.14 §3）',
            ', '.join(f"{n['key']}={n.get('props', {}).get('display')}" for n in mine) or f'{g[0]} の source_surface が無い')
        if keep:
            stuck = [n['key'] for n in keep if n['props']['display'] != 'handoff' and n['props'].get('fallback') != 'handoff']
            row(not stuck, f'{g[0]} の画面が開けない時に handoff へ戻れる（display=handoff か props.fallback=handoff・DESIGN §4.4）', ', '.join(stuck))

    if p['needs_connection']:
        wires = [s for s in srcs if not is_local(srcs[s])] + [c['id'] for c in caps if c.get('adapter') != 'handoff']
        row(wires, '頼まれた接続先を候補として持つ（外部の source か handoff 以外の capability）',
            ', '.join(wires) or '無い（local だけで済ませて、頼まれた接続を落としている）')

    qs = (d.get('metadata') or {}).get('questions', [])
    if not isinstance(qs, list):
        row(False, 'metadata.questions は配列', type(qs).__name__)
        qs = []
    lo, hi = p.get('min_questions', 0), p['max_questions']
    row(lo <= len(qs) <= hi, '質問しない' if hi == 0 else f'質問はちょうど {hi} 問' if lo == hi else f'質問は {hi} 問まで',
        f'{len(qs)} 問（metadata.questions）')
    steps = {a['id'] for a in acts} | {w['id'] for w in wfs} | set(srcs) | {'goal'}
    for k, q in enumerate(qs, 1):
        b = q.get('blocks_step_id') if isinstance(q, dict) else None
        row(b in steps, f'質問 {k} が止めている step を名指す（blocks_step_id）', repr(b))

    for g in p['gates']:
        hit = [a for a in acts if a['effect'] in g['effects']]
        row(hit, f"{g['verb']} の action が在る（{' / '.join(g['effects'])}）", ', '.join(a['id'] for a in hit) or '無い')
        if hit:
            loose = [a['id'] for a in hit if a.get('requires_approval') is not True]
            row(not loose, f"{g['verb']} は全部 requires_approval", ', '.join(loose))
            ids = {a['id'] for a in hit}
            unguarded = [w['id'] for w in wfs if ids & set(w['action_refs']) and w.get('approval') in (None, '', 'none')]
            row(not unguarded, f"{g['verb']} を含む workflow に approval", ', '.join(unguarded))

    if p['forbid_effects']:
        extra = [f"{a['id']}（{a['effect']}）" for a in acts if a['effect'] in p['forbid_effects']]
        row(not extra, f"頼まれていない effect が無い（{' / '.join(p['forbid_effects'])}）", ', '.join(extra))

    if p['local_only']:
        outside = ([s for s in srcs if not is_local(srcs[s])] + [a['id'] for a in acts if a['effect'].startswith('external')]
                   + [c['id'] for c in caps if c.get('adapter') != 'handoff'])
        row(not outside, 'local だけで動く（外部の source / action・handoff 以外の capability が無い）', ', '.join(dict.fromkeys(outside)))
        row(d['state']['readiness'] == 'ready', 'state.readiness = ready', d['state']['readiness'])

    if p['local_fallback']:
        # ponytail: 「接続を開くだけ」の action を id の token（connect…）で除いている。別の名前だと数えてしまう。
        # action registry に用途の区分が載ったらそちらで見る
        writes = [a['id'] for a in acts if a['effect'] == 'local_write' and a['readiness'] == 'ready'
                  and not any(t.startswith('connect') for t in tokens(a['id']))]
        parts = [n['key'] for n, _ in nodes if n['component'] in USEFUL and 'binding' in n and is_local(srcs.get(n['binding']['source_ref']))]
        row(writes and parts, '繋がなくても使える部分が在る（local-first）', f'ready な local_write {len(writes)}・local に bind した部品 {len(parts)}')

    # ---- 全 prompt に掛かる禁止（corpus の規則に無い分）----
    lies = [f"source {s['id']}={s['readiness']}" for s in d['sources']
            if not is_local(s) and s['readiness'] not in ('requires_connection', 'unsupported')]
    lies += [f"capability {c['id']}={c['readiness']}" for c in caps
             if c['readiness'] in ('connected_unverified', 'degraded', 'revoked', 'simulated_in_preview')
             or c['readiness'] == 'ready' and c.get('adapter') != 'handoff']
    row(not lies, '繋いでいない物を繋いだ扱いにしない（何も繋いでいない前提）', ', '.join(lies))
    nested = [n['key'] for n, anc in nodes if n['component'] == 'card' and 'card' in anc]
    row(not nested, 'card の中に card が無い（DESIGN §3）', ', '.join(nested))
    kpi = [n['key'] for n, _ in nodes
           if {v for v in (n.get('role'), n.get('props', {}).get('style'), n.get('props', {}).get('variant')) if isinstance(v, str)} & BANNED]
    row(not kpi, 'KPI の見た目が無い（role / props.style / props.variant）', ', '.join(kpi))
    inline = [f"{n['key']}.props{path}" for n, _ in nodes for path in inline_rows(n.get('props', {}), '')]
    row(not inline, 'props に行データが無い（偽 data）', ', '.join(inline))
    no_empty = [n['key'] for n, _ in nodes if n['component'] in ('graph', 'map') and 'binding' in n and not n.get('states', {}).get('empty')]
    row(not no_empty, 'bind した graph / map に states.empty', ', '.join(no_empty))

    if hi:
        rows.append(('MANUAL', '質問は preview と一緒に 1 つだけ出て、既定の答えと「後で」がある（GOAL_FIRST_APP_GENERATION §2 F）', ''))
    for g in p['gates']:
        rows.append(('MANUAL', f"{g['verb']} の確認画面に相手・中身・取り消せるかが出る（DESIGN §10）", ''))
    for g in p.get('surface', []):
        rows.append(('MANUAL', f'{g[0]} の画面は元の見た目のまま出て、手で操作している間に Agent が focus を奪わない（DESIGN §14.1・§14.3・PORTAL-09）', ''))
    rows.append(('MANUAL', '画面: 空の状態の文・disabled の理由・接続の導線が出る（描画は UI-005 / CON-002）', ''))
    return rows


def grade(p, path):
    if not path.exists():
        return [('FAIL', '出力が在る', str(path))]
    raw = path.read_text(encoding='utf-8')
    try:
        return check(p, json.loads(raw), raw)
    except json.JSONDecodeError as ex:
        return [('FAIL', 'JSON として読める', str(ex))]
    except Exception as ex:  # 想定外の形でも --all を止めない
        return [('FAIL', '検査が最後まで走る', f'{type(ex).__name__}: {ex}')]


def fmt(r):
    return f'{r[0]:<6} {r[1]}' + (f' — {r[2]}' if r[2] else '')


def main(argv):
    path = PROMPTS
    if '--prompts' in argv:
        i = argv.index('--prompts')
        if i + 1 >= len(argv):
            sys.exit('--prompts の後に prompts の md の path が要る')
        path, argv = Path(argv[i + 1]).expanduser(), argv[:i] + argv[i + 2:]
    prompts = load_prompts(path)
    by_id = {p['id']: p for p in prompts}
    if argv == ['--list']:
        for p in prompts:
            print(f"{p['id']}\t{p['prompt']}")
        return 0
    if len(argv) == 2 and argv[0] == '--all':
        tally = {}
        for p in prompts:
            fails = [r for r in grade(p, Path(argv[1]) / f"{p['id']}.json") if r[0] == 'FAIL']
            tally.setdefault(p['kind'], []).append(not fails)
            print(f"{'FAIL' if fails else 'PASS'} {p['id']} {p['kind']}")
            for r in fails:
                print('       ' + fmt(r))
        ok = sum(sum(v) for v in tally.values())
        print(' / '.join(f'{k} {sum(v)}/{len(v)}' for k, v in tally.items()) + f' → {ok}/{len(prompts)} PASS（静的検査のみ。MANUAL 行は画面で）')
        return 0 if ok == len(prompts) else 1
    if len(argv) == 2 and argv[0] in by_id:
        p = by_id[argv[0]]
        rows = grade(p, Path(argv[1]))
        for r in rows:
            print(fmt(r))
        n_fail, n = sum(r[0] == 'FAIL' for r in rows), sum(r[0] != 'MANUAL' for r in rows)
        print(f"{'FAIL' if n_fail else 'PASS'} {p['id']}（{p['kind']}）: {n - n_fail}/{n} 行")
        return 1 if n_fail else 0
    sys.exit(__doc__.split('\n\n')[1] + (f'\n知らない prompt id: {argv[0]}（{min(by_id)}〜{max(by_id)}）' if len(argv) == 2 else ''))


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
