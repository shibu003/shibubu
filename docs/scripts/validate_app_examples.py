#!/usr/bin/env python3
"""Spec QA only: JSON Schema plus cross-reference checks for Kangae example apps.
Requires: pip install jsonschema. It does not execute actions or launch apps.
"""
import json
from pathlib import Path
from jsonschema import Draft202012Validator
ROOT = Path(__file__).resolve().parents[1]
SCHEMA = json.loads((ROOT / "schemas" / "app_definition_v1.schema.json").read_text())

def unique_ids(objects, kind):
    ids = [obj['id'] for obj in objects]
    assert len(ids) == len(set(ids)), f"duplicate {kind} id"
    return set(ids)

def check(app):
    Draft202012Validator(SCHEMA).validate(app)
    sources = unique_ids(app['sources'], 'source')
    cap_ids = unique_ids(app['capabilities'], 'capability')
    datasets = unique_ids(app['datasets'], 'dataset')
    workflows = unique_ids(app['workflows'], 'workflow')
    assert all(c['source_id'] in sources for c in app['capabilities'])
    assert all(d['produced_by'] in cap_ids for d in app['datasets'])
    assert sum(r['role'] == 'primary' for r in app['view']['regions']) == 1
    roles = [r['role'] for r in app['view']['regions']]
    assert len(set(roles)) == len(roles), 'duplicate region role'
    keys = []
    for region in app['view']['regions']:
        keys.append(region['key'])
        for comp in region['components']:
            keys.append(comp['key'])
            if 'action_id' in comp:
                assert comp['action_id'] in cap_ids
            if 'dataset_id' in comp:
                assert comp['dataset_id'] in datasets
            if 'workflow_id' in comp:
                assert comp['workflow_id'] in workflows
            if comp['kind'] == 'button':
                assert 'action_id' in comp, 'button without action binding'
            if comp['kind'] == 'workflow_launcher':
                assert 'workflow_id' in comp
    assert len(keys) == len(set(keys)), 'duplicate region/component key'
    for wf in app['workflows']:
        steps = wf['steps']
        step_ids = unique_ids(steps, 'step')
        graph = {s['id']: s['after'] for s in steps}
        assert all(s['capability_id'] in cap_ids for s in steps)
        assert all(dep in step_ids for deps in graph.values() for dep in deps)
        pending, complete = set(), set()
        def visit(node):
            assert node not in pending, 'workflow has cycle'
            if node in complete: return
            pending.add(node)
            for dep in graph[node]: visit(dep)
            pending.remove(node)
            complete.add(node)
        for node in graph: visit(node)

if __name__ == '__main__':
    Draft202012Validator.check_schema(SCHEMA)
    count = 0
    for path in sorted((ROOT / 'examples').glob('*.json')):
        app = json.loads(path.read_text())
        # Versions 2/3 and Agent proposals have separate validators.
        if app.get('schema_version') != '1.0' or 'view' not in app:
            continue
        check(app)
        count += 1
        print(f'PASS {path.name}: {app["view"]["pattern"]}')
    assert count >= 4, 'expected four v1 examples'
