#!/usr/bin/env python3
"""Validate v0.11 goal-first example plans (structure + narrow semantic rules).

Static examples only; does not attest to connected accounts, usable adapters,
actual app generation, safe external actions or UI rendering.
"""
import json
from pathlib import Path
import jsonschema

ROOT = Path(__file__).resolve().parents[1]
schema = json.loads((ROOT / 'schemas' / 'goal_plan_v1.schema.json').read_text())
plans = sorted((ROOT / 'examples').glob('goal_*.json'))
assert len(plans) >= 5
for path in plans:
    plan = json.loads(path.read_text())
    jsonschema.Draft202012Validator(schema).validate(plan)
    need_ids = {n['id'] for n in plan['needs']}
    assert len(need_ids) == len(plan['needs']), path
    assert len({r['id'] for r in plan['requirements']}) == len(plan['requirements']), path
    assert len({v['id'] for v in plan['ui_intents']}) == len(plan['ui_intents']), path
    assert sum(1 for v in plan['ui_intents'] if v['primary']) == 1, path
    for question in plan['questions']:
        assert question['blocking_need_id'] in need_ids, (path, question)
    if plan['source_policy'] == 'local_only':
        assert plan['candidate_sources'] == [], path
        assert plan['questions'] == [], path
    # The plan must not pretend permission or a verified external write.
    for need in plan['needs']:
        if need['effect'] in ('external_write', 'execute'):
            assert need['status'] != 'planned_local', path
    # Only user-named source assertions are allowed to be listed as explicit.
    for src in plan['candidate_sources']:
        if src['evidence'] == 'user_named':
            assert src['label'].casefold() in plan['prompt'].casefold(), (path,src)
    print('PASS', path.name, '| sources:',len(plan['candidate_sources']),'| questions:',len(plan['questions']))
print('PASS: all goal-first example structures and semantics')
