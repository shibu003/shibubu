#!/usr/bin/env -S python3 -I
"""試験の側の見本 app「Quillpad」（架空・tests/fake-mcp-server.py と同じ app）の .app の中に同梱される CLI。W19-3 の試験が
<試験の置き場>/Quillpad.app/Contents/MacOS/quillctl に写す。公式の MCP（読むだけ）には題を書く tool が無く、この CLI は解析（tests/fake-mcp-server.py の
REA の役）でしか見つからない、という見本。製品の code はこの名前を知らない。

  quillctl set-title <path> <title>

文書は JSON の file {"title", "body"}。外に効く操作は持たない（読み書きするのは引数の path の file だけ）。
"""
import json
import os
import sys

USAGE = "usage: quillctl set-title <path> <title>"


def main(argv):
    if len(argv) != 4 or argv[1] != "set-title":
        print(USAGE, file=sys.stderr)
        return 2
    path, title = argv[2], argv[3]
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    doc["title"] = title
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False)
    os.replace(tmp, path)
    print(json.dumps({"saved": True}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
