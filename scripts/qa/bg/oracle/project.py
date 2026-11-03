#!/usr/bin/env python3
"""oracle/project.py — 生成された Project を DB から読み戻す（I1 / I3 の「revision 1 が generate・Page が 2 枚以上・surface_specs が在る」・画面とは別の道）。

呼ぶ側の使い方:
    python3 -I scripts/qa/bg/oracle/project.py read "$R/data" [--since <epoch 秒>] > "$R/oracle/project.json"
    exit: 0 読めた（合否は ok で見る）/ 2 使い方 / 4 DB が無い・表が無い

出力（shibubu.qa.project/1）: {schema, t, db, ok, projects:[{id, rev1_origin, rev1_created_at, pages, surface_specs, goal_sha8, ok}]}
  ok = since より後に作られた revision 1 の origin が generate で、pages >= 2 かつ surface_specs >= 1 の Project が 1 つ以上。
保証する: <data-dir>/kangae.sqlite3 を read-only で開き書かない。goal は sha256 の頭 8 桁だけ残す（held-out の文を出さない）。
保証しない: surface_specs の target が一言の名指した app / origin と一致するか（YC_ACCEPTANCE I3 の本判定は target 集合の突き合わせ・ここは存在だけ）。
"""
import datetime, hashlib, json, os, sqlite3, sys, time

SCHEMA = "shibubu.qa.project/1"


def read_projects(data_dir, since=0.0):
    db = os.path.join(data_dir, "kangae.sqlite3")
    if not os.path.isfile(db):
        raise FileNotFoundError(f"DB が無い: {db}")
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        for t in ("projects", "project_revisions"):
            if not con.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (t,)).fetchone():
                raise FileNotFoundError(f"{t} の表が無い: {db}")
        out = []
        for pid, js in con.execute("SELECT id, json FROM projects ORDER BY created_at"):
            rev = con.execute("SELECT origin, created_at FROM project_revisions WHERE project_id=? AND revision=1", (pid,)).fetchone()
            d = json.loads(js)
            origin, created = rev if rev else (None, None)
            fresh = created is not None and datetime.datetime.fromisoformat(created.replace("Z", "+00:00")).timestamp() > since
            pages, specs = len(d.get("pages") or []), len(d.get("surface_specs") or [])
            out.append({"id": pid, "rev1_origin": origin, "rev1_created_at": created, "pages": pages, "surface_specs": specs,
                        "goal_sha8": hashlib.sha256((d.get("goal") or "").encode()).hexdigest()[:8],
                        "ok": bool(fresh and origin == "generate" and pages >= 2 and specs >= 1)})
    finally:
        con.close()
    return {"schema": SCHEMA, "t": time.time(), "db": db, "ok": any(p["ok"] for p in out), "projects": out}


def main(a):
    if len(a) < 2 or a[0] != "read":
        print("usage: project.py read <data-dir> [--since <epoch>]", file=sys.stderr)
        return 2
    since = float(a[a.index("--since") + 1]) if "--since" in a else 0.0
    try:
        print(json.dumps(read_projects(a[1], since), ensure_ascii=False, sort_keys=True))
    except FileNotFoundError as e:
        print(f"project.py: {e}", file=sys.stderr)
        return 4
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
