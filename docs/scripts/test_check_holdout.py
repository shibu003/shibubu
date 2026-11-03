#!/usr/bin/env python3
"""check_holdout.py の採点器そのものの test（holdout の prompt は使わない・持たない）。

  cd docs && python3 -I scripts/test_check_holdout.py

- `--prompts <path>`: 版を選べる・無い file は path を言って止まる・既定（v1 の場所）は変わらない
- 性質 `surface`（v0.14・元の app の画面を残す）: corpus の 05（source_surface が handoff）を 1 か所ずつ壊して、狙った行だけが赤くなる
- prompt の md は合成した 20 本（本物の holdout の文ではない）。jsonschema が要る（check_holdout と同じ）
"""
import contextlib
import copy
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_holdout as ch  # noqa: E402

CORPUS_05 = ch.DOCS / 'examples/v3/05_creative_editor.json'


def synthetic(i, kind):
    """kind の性質を満たす最小の 1 本。文は corpus と重ならない合成の文字列"""
    return {'id': f'H{i:02d}', 'kind': kind, 'prompt': f'合成の依頼 zq{i:02d}', 'components': [], 'named': [],
            'needs_connection': kind == 'connect', 'max_questions': 1 if kind == 'ambiguous' else 0,
            'gates': [{'verb': '送信', 'effects': ['external_send']}] if kind == 'risky' else [],
            'forbid_effects': [], 'local_only': kind == 'local', 'local_fallback': False}


def md(prompts):
    return '\n'.join(f'## {p["id"]}\n\n```json\n{json.dumps(p, ensure_ascii=False)}\n```\n' for p in prompts)


def twenty():
    kinds = ['local'] * 6 + ['connect'] * 6 + ['risky'] * 4 + ['ambiguous'] * 4
    return [synthetic(i, k) for i, k in enumerate(kinds, 21)]


def run_main(argv):
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = ch.main(argv)
    return code, out.getvalue()


class PromptsOption(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = Path(self.dir.name) / 'prompts-vX.md'

    def tearDown(self):
        self.dir.cleanup()

    def test_lists_the_chosen_file_in_either_position(self):
        self.path.write_text(md(twenty()), encoding='utf-8')
        for argv in (['--prompts', str(self.path), '--list'], ['--list', '--prompts', str(self.path)]):
            code, out = run_main(argv)
            lines = out.splitlines()
            self.assertEqual(code, 0)
            self.assertEqual(len(lines), 20)
            self.assertEqual(lines[0], 'H21\t合成の依頼 zq21')

    def test_missing_file_names_the_path(self):
        with self.assertRaises(SystemExit) as e:
            ch.main(['--prompts', str(self.path), '--list'])
        self.assertIn(str(self.path), str(e.exception.code))

    def test_default_is_still_the_v1_place(self):
        self.assertEqual(ch.PROMPTS, ch.DOCS / 'internal/holdout/prompts-v1.md')

    def test_surface_must_also_be_named(self):
        ps = twenty()
        ps[0]['surface'] = [['blender']]  # named に無い
        self.path.write_text(md(ps), encoding='utf-8')
        with self.assertRaises(SystemExit) as e:
            ch.load_prompts(self.path)
        self.assertIn('surface の組は named にも置く', str(e.exception.code))
        ps[0]['named'] = [['blender']]
        self.path.write_text(md(ps), encoding='utf-8')
        self.assertEqual(len(ch.load_prompts(self.path)), 20)

    def test_v1_shape_without_surface_still_loads(self):
        ps = twenty()
        self.assertTrue(all('surface' not in p for p in ps))
        self.path.write_text(md(ps), encoding='utf-8')
        self.assertEqual(len(ch.load_prompts(self.path)), 20)


class SurfaceRule(unittest.TestCase):
    """corpus の 05 は source_surface（key edit-surface・display handoff・action editor.open → capability editor.open）を持つ"""

    def setUp(self):
        self.base = json.loads(CORPUS_05.read_text(encoding='utf-8'))
        self.p = {**synthetic(1, 'local'), 'prompt': self.base['goal']['literal_goal'], 'local_only': False,
                  'named': [['editor']], 'surface': [['editor']]}

    def node(self, d, key):
        stack = [d['composition']['root']]
        while stack:
            n = stack.pop()
            if n['key'] == key:
                return n
            stack += n.get('children', [])
        raise AssertionError(f'{key} が無い')

    def rows(self, d, p=None):
        raw = json.dumps(d, ensure_ascii=False)
        return {label: (status, detail) for status, label, detail in ch.check(p or self.p, d, raw)}

    def find(self, rows, head):
        hit = [v for k, v in rows.items() if k.startswith(head)]
        return hit[0] if hit else None

    def test_handoff_surface_of_the_named_app_passes(self):
        rows = self.rows(self.base)
        self.assertEqual(self.find(rows, 'editor の元の画面を残す')[0], 'PASS')
        self.assertEqual(self.find(rows, 'editor の画面が開けない時に handoff へ戻れる')[0], 'PASS')

    def test_native_display_is_not_the_original_screen(self):
        d = copy.deepcopy(self.base)
        self.node(d, 'edit-surface')['props']['display'] = 'native'
        rows = self.rows(d)
        self.assertEqual(self.find(rows, 'editor の元の画面を残す'), ('FAIL', 'edit-surface=native'))
        self.assertIsNone(self.find(rows, 'editor の画面が開けない時'))

    def test_embedded_needs_a_handoff_fallback(self):
        d = copy.deepcopy(self.base)
        self.node(d, 'edit-surface')['props']['display'] = 'embedded'
        rows = self.rows(d)
        self.assertEqual(self.find(rows, 'editor の元の画面を残す')[0], 'PASS')
        self.assertEqual(self.find(rows, 'editor の画面が開けない時に handoff へ戻れる'), ('FAIL', 'edit-surface'))
        self.node(d, 'edit-surface')['props']['fallback'] = 'handoff'
        self.assertEqual(self.find(self.rows(d), 'editor の画面が開けない時に handoff へ戻れる')[0], 'PASS')

    def test_no_source_surface_fails(self):
        d = copy.deepcopy(self.base)
        n = self.node(d, 'edit-surface')
        n['component'] = 'editor'
        n.pop('props', None)
        self.assertEqual(self.find(self.rows(d), 'editor の元の画面を残す'), ('FAIL', 'editor の source_surface が無い'))

    def test_surface_of_another_app_does_not_count(self):
        p = {**self.p, 'named': [['editor'], ['blender']], 'surface': [['blender']]}
        rows = self.rows(self.base, p)
        self.assertEqual(self.find(rows, 'blender の元の画面を残す'), ('FAIL', 'blender の source_surface が無い'))

    def test_prompts_without_surface_get_no_surface_rows(self):
        p = {k: v for k, v in self.p.items() if k != 'surface'}
        rows = self.rows(self.base, p)
        self.assertFalse([k for k in rows if '元の画面を残す' in k or '画面が開けない時' in k])


if __name__ == '__main__':
    unittest.main(verbosity=1)
