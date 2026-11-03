#!/usr/bin/env python3
"""Validate Kangae v2 demonstrator app definitions. Requires jsonschema.

Static checks are not proof of external connectivity, native rendering, CUA safety,
user authorization, or that a component exists in the real registry.
"""
import json
from pathlib import Path
from jsonschema import Draft202012Validator

DOCS = Path(__file__).resolve().parents[1]
SCHEMA = json.loads((DOCS / 'schemas/app_definition_v2.schema.json').read_text(encoding='utf-8'))
Draft202012Validator.check_schema(SCHEMA)


def must(condition, message):
    if not condition:
        raise ValueError(message)


def unique_ids(items, label):
    ids = [item['id'] for item in items]
    must(len(ids) == len(set(ids)), f'duplicate {label}: {ids}')
    return set(ids)


def check_dag(steps):
    ids = unique_ids(steps, 'step ids')
    graph = {s['id']: s['after'] for s in steps}
    for step in steps:
        for dep in step['after']:
            must(dep in ids, f'unknown dependency {dep}')
            must(dep != step['id'], 'self dependency')
    visiting = set()
    visited = set()
    def visit(node):
        if node in visited:
            return
        must(node not in visiting, 'cycle detected')
        visiting.add(node)
        for dep in graph[node]:
            visit(dep)
        visiting.remove(node)
        visited.add(node)
    for node in ids:
        visit(node)


def check_refs(doc):
    source_ids = unique_ids(doc['sources'], 'sources')
    cap_ids = unique_ids(doc['capabilities'], 'capabilities')
    entity_ids = unique_ids(doc['entities'], 'entities')
    action_ids = unique_ids(doc['actions'], 'actions')
    unique_ids(doc['views'], 'views')
    unique_ids(doc['workflows'], 'workflows')
    for cap in doc['capabilities']:
        must(cap['source_id'] in source_ids, f"unknown source {cap['source_id']}")
    for action in doc['actions']:
        must(action['capability_id'] in cap_ids, f"unknown capability {action['capability_id']}")
        if 'target_entity_id' in action:
            must(action['target_entity_id'] in entity_ids, f"unknown entity {action['target_entity_id']}")
    seen_keys = set()
    count = 0
    def walk(node, depth):
        nonlocal count
        count += 1
        must(depth <= 24 and count <= 500, 'tree resource budget exceeded')
        must(node['key'] not in seen_keys, f"duplicate node key {node['key']}")
        seen_keys.add(node['key'])
        for field, refs in [('data_ref', entity_ids), ('action_ref', action_ids)]:
            if field in node:
                must(node[field] in refs, f"unknown {field} {node[field]}")
        typ = node['type_id']
        must(not (typ.startswith('layout.') and ('data_ref' in node or 'action_ref' in node)),
             f'layout operator cannot bind directly: {typ}')
        must(not (typ.startswith('ui.button') and 'action_ref' not in node),
             'button must bind to action')
        for child in node.get('children', []):
            walk(child, depth+1)
    for view in doc['views']:
        walk(view['root'], 1)
    for wf in doc['workflows']:
        check_dag(wf['steps'])
        for step in wf['steps']:
            must(step['action_id'] in action_ids, f"unknown action {step['action_id']}")
    return count


def main():
    validator = Draft202012Validator(SCHEMA)
    examples = sorted((DOCS/'examples').glob('*_v2.json'))
    for path in examples:
        data = json.loads(path.read_text(encoding='utf-8'))
        errors = sorted(validator.iter_errors(data), key=lambda e: list(map(str, e.path)))
        must(not errors, f'{path.name}: ' + '; '.join(e.message for e in errors))
        n = check_refs(data)
        print(f'PASS {path.name}: {len(data["views"])} views, {n} UI nodes, {len(data["actions"])} action refs')
    must(len(examples) >= 3, 'expected >=3 diverse examples')
    print('PASS: schema, references, stable keys, DAG and size budgets')
    print('NOTE: adapter availability, component registry and visual rendering are NOT tested.')

if __name__ == '__main__':
    main()
