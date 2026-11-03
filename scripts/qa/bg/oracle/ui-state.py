#!/usr/bin/env python3
"""oracle/ui-state.py — Shibubu の data dir の ui_state を読み戻す（W2b-3 の受け入れ「入れた文字が ui_state に入る」・画面とは別の道）。

呼ぶ側の使い方:
    python3 -I scripts/qa/bg/oracle/ui-state.py read "$R/data" --needle qaneedle5821 > "$R/oracle/03-type.after.uistate.json"
    exit: 0 読めた（合否は has_needle で見る）/ 2 使い方 / 4 DB が無い・ui_state の表が無い / 70 未実装

出力（shibubu.qa.uistate/1）: {schema, t, db, needle, rows:[{session_id, key, updated_at, json_sha256, has_needle}], has_needle}
保証する: <data-dir>/kangae.sqlite3 を read-only（sqlite の mode=ro）で開き、書かない。値は sha256 と has_needle だけ残す（中身は evidence に書かない）。
          DB や表が無い時は exit 4（has_needle=false の緑にしない）。
保証しない: 製品がどの key に書くか（key の名前は製品の側・ここでは全行を引く）。書き込みの途中の WAL は sqlite が見せる所まで。
"""
from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import sys
import time

SCHEMA = "shibubu.qa.uistate/1"
DB_NAME = "kangae.sqlite3"


def read_ui_state(data_dir: str, needle: str | None) -> dict:
    """ui_state の全行を読み、shibubu.qa.uistate/1 の dict を返す。DB・表が無ければ FileNotFoundError。"""
    db = os.path.join(data_dir, DB_NAME)
    if not os.path.isfile(db):
        raise FileNotFoundError(f"DB が無い: {db}")
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        if not con.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='ui_state'").fetchone():
            raise FileNotFoundError(f"ui_state の表が無い: {db}")
        rows = [{"session_id": s, "key": k, "updated_at": u, "json_sha256": hashlib.sha256(j.encode()).hexdigest(),
                 "has_needle": bool(needle) and needle in j}
                for s, k, j, u in con.execute("SELECT session_id, key, json, updated_at FROM ui_state ORDER BY session_id, key")]
    finally:
        con.close()
    return {"schema": SCHEMA, "t": time.time(), "db": db, "needle": needle, "rows": rows, "has_needle": any(r["has_needle"] for r in rows)}


def main(argv: list[str]) -> int:
    if len(argv) not in (3, 5) or argv[1] != "read" or (len(argv) == 5 and argv[3] != "--needle"):
        print("usage: ui-state.py read <data-dir> [--needle s]", file=sys.stderr)
        return 2
    try:
        d = read_ui_state(argv[2], argv[4] if len(argv) == 5 else None)
    except NotImplementedError as e:
        print(f"ui-state.py: {e}", file=sys.stderr)
        return 70
    except FileNotFoundError as e:
        print(f"ui-state.py: {e}", file=sys.stderr)
        return 4
    print(json.dumps(d, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
