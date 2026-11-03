#!/usr/bin/env python3
"""W13c: 同梱の配置の型（assets/templates/*.json）を docs/schemas/placement_template_v1.schema.json で検査する。Requires jsonschema.

意味の検査（Page ごとに main が 1 つ・size の和）は Rust の composer::layout::validate_template（cargo test の w13c_*）。ここは形と、
壊した型を schema が本当に拒むか（負の対照: 未知の欄・知らない区画・知らない種類・size の範囲の外）。
"""
import copy
import json
from pathlib import Path
from jsonschema import Draft202012Validator

DOCS = Path(__file__).resolve().parents[1]
ROOT = DOCS.parent
SCHEMA = json.loads((DOCS / 'schemas/placement_template_v1.schema.json').read_text(encoding='utf-8'))
Draft202012Validator.check_schema(SCHEMA)
V = Draft202012Validator(SCHEMA)

files = sorted((ROOT / 'assets/templates').glob('*.json'))
assert [f.name for f in files] == ['dev.json', 'music.json', 'tasks.json'], [f.name for f in files]
for f in files:
    t = json.loads(f.read_text(encoding='utf-8'))
    errs = [e.message for e in V.iter_errors(t)]
    assert not errs, f'{f.name}: {errs}'

base = json.loads(files[0].read_text(encoding='utf-8'))
broken = []
for why, change in [
    ('未知の欄', lambda t: t['pages'][0]['regions'][0].update(x=1)),
    ('知らない区画', lambda t: t['pages'][0]['regions'][0].update(slot='left')),
    ('知らない種類', lambda t: t['pages'][0]['regions'][0]['accepts'].append('reaper')),
    ('size の範囲の外', lambda t: t['pages'][0]['regions'][0].update(size=1.5)),
    ('id の無い型', lambda t: t.pop('id')),
]:
    t = copy.deepcopy(base)
    change(t)
    if V.is_valid(t):
        broken.append(why)
assert not broken, f'schema が拒まない: {broken}'
print(f'validate_placement_templates: ok（{len(files)} 個・負の対照 5 つを拒んだ）')
