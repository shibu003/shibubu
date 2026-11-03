#!/usr/bin/env python3
"""ui-strings.py を「英語の門の外の A 層」に向ける薄い口（W15-b7）。引数は ui-strings.py と同じ（--set の既定だけ outside）。

  python3 -I scripts/qa/ui-strings-outside.py --root <木> --map docs/ui-copy/outside.en.md

A 層は ui-strings.py の --set の口（SETS）に "outside" として足す。`uis.TARGETS =` の差し替えは使わない:
yc/ui の ui-strings.py（74c9ea2〜）では SETS["all"] と scan() の既定引数が import の時の TARGETS を掴むので届かず、
黙って UI-2 の集合を測る（3e82c5d の木で 1788/1788 が 675/1165）。SETS の無い ui-strings.py では AttributeError で止まる。
"""
import importlib.util
import os
import sys

spec = importlib.util.spec_from_file_location("uis", os.path.join(os.path.dirname(os.path.abspath(__file__)), "ui-strings.py"))
uis = importlib.util.module_from_spec(spec)
spec.loader.exec_module(uis)
# copy-scope.md の A 層（goal.rs は C 層・bin/domain/patch などは B 層で入れない）
uis.SETS["outside"] = (["src/generated.rs", "src/composer/project.rs", "src/composer/layout.rs", "src/composer/verifier.rs", "src/surface/**/*.rs", "src/project/**/*.rs",
                        "src/mcp.rs", "src/llm_cli.rs", "src/capability_engine/*.rs", "src/intel/**/*.rs", "src/google/**/*.rs", "src/media/**/*.rs", "src/audio/**/*.rs",
                        "src/agent/**/*.rs", "src/github.rs", "src/repos.rs", "src/gui/**/*.rs", "src/terminal/**/*.rs", "src/adapters/mac_gui.rs", "src/ui/apps.rs"], ())
sys.argv[1:1] = ["--set", "outside"]  # 後ろに --set を書けばそちらが勝つ（argparse は最後の値）
uis.main()
