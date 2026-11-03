"""BG の driver が AX で要素を探す label の 1 か所の表（W15-b32）。

driver は JP の literal を直に書かず `names("create")` / `eq("create")` / `has("save_repo")` で引く。
画面が英語になっても（UI-1〜6・UI-2b・UI-2c）同じ driver が通るように、**identifier が在れば先に・無ければ EN と JP の両方**で探す。
literal（表に無い語）はそのまま 1 語として通る（ユーザーが名前を付けた tab の `Docs` / `Tests` など）。

LABELS[key] = (role, jp, en, ident, status)
  role   AX の role（AXButton / AXTextArea / AXTextField / …・None = 不問）
  jp     今の表記
  en     英語の表の訳（docs/ui-copy/copy.en.md・additions.md・outside.en.md の `| JP | EN |`）。空 = 表に無い
  ident  AX identifier（完全一致。末尾 `*` は前方一致）。None = 今の src/ui に付いていない
  status "en" = 英語の表に在る / "pending:UI-5a" = tab と帯の label は UI-5a で決まる（UI-5a の本体で back と memo だけ残る）/ "pending:UI-n" = 表に無い（JP で探す）
一覧と file:line は docs/evidence/W15/driver-labels.md。
"""
LABELS = {
    # UI-4a（trunk 9f60c76）: 一言は header の 1 つの欄（singleline の TextEdit = AXTextField・AX の名前 `What to create`〈src/ui/shell.rs:70 View::field_name〉・placeholder は `What do you want to create?`〈shell.rs:58〉）。
    # 古い .app の AXTextArea の欄は fixture（scripts/qa/fixtures/labels）にだけ残る。driver は role をこの表から引く（rehearse.h_generate）
    "goal":      ("AXTextField", "何を作りたいですか？",   ("What to create", "What do you want to create?"), None, "en"),
    "create":    ("AXButton",    "作る",                   ("Create", "Create · ⌘↵"), None, "en"),
    # L2（YC_DECISIONS 109 通目 15・111 通目）: Home の欄の右端は `↵ Send`（AX の名前 `Send` = 下の send・src/ui/shell.rs の enter_label）で、Home の一言は作らずに会話になる。
    # TALK-1101（owner 191 通目）: 作るのは会話の案の card の先頭の `Build now`（最初から出る・AX の名前も同じ語・src/domain/mod.rs の chat::BUILD_NOW・前の `Accept`）。
    # `Build now` は答えの要る問い（needs）と busy の間は灰。`Preview plan` は中身を card に開くだけ（押さなくてよい）。上の create は古い .app の木（fixture）
    "preview_plan": ("AXButton", "（L2）案を見る",        ("Preview plan",), None, "en"),
    "accept":    ("AXButton",    "（L2）案で作る",         ("Build now",), None, "en"),
    # CHAT-ONE-1101: Project の欄は下の panel の Chat の tab の chat bar（Home と同じ部品・AX の名前も同じ `What to create`・畳んだ panel と Terminal の tab には無い）。
    # 送ると会話（TALK-1101: 案の card の先頭の `Build now` で変わる）。`Change this project` は古い .app の木（fixture）
    "change":    ("AXTextField", "この Project への変更",  ("What to create", "Ask, or say what to change, e.g. add a Tests page", "Change this project"), None, "en"),
    "term_in":   ("AXTextField", "Terminal に打つ",        ("Type in the terminal",), None, "en"),
    # L7（yc/snap-b70 696ef28c・src/ui/terminal_panel.rs）: 上の欄は既定で隠れる。帯の ••• → menu の項目で出す / 隠す（rehearse.term_field）
    "term_opts": ("AXButton",    "Terminal の •••",        ("Terminal options",), None, "en"),
    "term_ime":  ("AXButton",    "Terminal の欄を出す",    ("Input field (for IME)",), None, "en"),
    "add_term":  ("AXButton",    "＋ Terminal",            ("Add terminal",), None, "en"),
    # UI-5a: 帯の ＋ と ••• の icon button（AX の名前 = tooltip・copy.en.md:144 / :150）。Delete page… などは ••• の menu の項目（先に開く）
    # MIRROR-FRAME-1101（owner 253 = 案 3）: Add mirror frame / Add browser / Add terminal は Page の右下の ＋（`Add to page`・pointer が Page の上の時だけ見えるが
    # AX には常に在る）の menu。帯の tab の ＋ は `Add page · ⌘T`（menu を開かない）。`Add · ⌘T for a page` は古い .app の木（fixture）
    "add_menu":  ("AXButton",    "＋",                     ("Add to page", "Add · ⌘T for a page"), None, "en"),
    "options":   ("AXButton",    "•••",                    ("Page options",), None, "en"),
    "add_browser": ("AXButton",  "＋ Browser",             ("Add browser",), None, "en"),
    "save_repo": ("AXButton",    "repo に保存",            ("Save to repo",), None, "en"),
    "mirror":    ("AXButton",    "映す",                   ("Mirror here",), "mirror-consent:*", "en"),
    # Mirror の矩形の AX の名前の頭（src/ui/mirror_view.rs の `{state.wording()} · {label}`・PortalState::Mirror = copy.en.md:255）。identifier の surface-frame:* は映っていない面にも付くので名前で見る
    "mirror_live": ("AXImage",   "映しています · ",        ("Mirroring · ",), None, "en"),
    # 元の app が居ない時の矩形の名前の頭（PortalState::SourceUnavailable = copy.en.md:257・#26 の D2）。B23 から `Not available · `・`Not running · ` は 45b0c6dd までの .app の木
    "mirror_gone": ("AXImage",   "終了しています · ",      ("Not available · ", "Not running · "), None, "en"),
    "add":       ("AXButton",    "足す",                   ("Add",), None, "en"),
    # Send: Engine の送る・L2 から Home の欄の右端 `↵ Send` も同じ AX の名前（rehearse.h_generate が押す）
    "send":      ("AXButton",    "送る",                   ("Send",), None, "en"),
    # W15-b49: Engine の依頼の行（src/generated.rs:160 SEND_EDIT・src/ui/engine_view.rs:53,57,60）。JP は drive-w19-3b.py が探していた語
    # UI-4a: Page の中の `Send changes` は消えた（generated.rs:1614・d1_one_field）。Project の送るは header の欄の右端の button `↵ Change`（AX の名前は鍵の記号を外した `Change`・header.rs:119・shell.rs:412）。
    # `Send changes` は古い .app の木（r10-restart）の fixture
    "send_edit": ("AXButton",    "変更を送る",             ("Change", "Send changes"), None, "en"),
    "approve":   ("AXButton",    "承認して送る",           ("Approve and send",), None, "en"),
    "cancel":    ("AXButton",    "送らない",               ("Cancel",), None, "en"),
    "close":     ("AXButton",    "閉じる",                 ("Close",), None, "en"),
    # 結果の語（src/generated.rs:728 の Done 行・src/adapters/mac_gui.rs:513 の lease の拒否・src/ui/renderer/components.rs:1241 の Browser の URL 欄）
    "done_checked": ("AXStaticText", "元の app の実状態で",  ("Done. Checked",), None, "en"),
    "nothing_sent": ("AXStaticText", "何も送りませんでした", ("so nothing was sent",), None, "en"),
    "browser_url": ("AXTextField", "URL",                  ("Search or enter website",), None, "en"),
    "run_tests": ("AXButton",    "走らせる",               ("Run tests",), None, "en"),
    "reload":    ("AXButton",    "↻ 読み直す",             ("Refresh", "↻ Reload"), None, "en"),
    # 候補 EN（tuple には入れない・決まったら入れて status を "en" に）: 出典無し（yc/ui の docs/ui-copy に「題名」の表示の行は無い）
    "title":     (None,          "題名",                   (), None, "pending:UI-n"),
    # Save: yc/ui の driver（drive-w2b-ax.py:80）が英語の .app で探す語。copy.en.md:315 は web-save の button で、別の画面の Save と同じ語になる（W15-b45）
    "save":      ("AXButton",    "保存",                   ("Save",), None, "en"),
    # UI-5a: 帯の ↶ の AX の名前 `Undo · ⌘Z`（copy.en.md:148 の {action} は UI-5b・sendback 5a-3）。前の 2 つは古い .app の木（fixture）のため残す
    "undo":      ("AXButton",    "↶ 元に戻す",             ("Undo · ⌘Z", "↶ Undo", "Undo"), None, "en"),
    # UI-5a: 帯の ↷ の AX の名前 `Redo · ⇧⌘Z`（copy.en.md:149・在る時だけ描く）。`Redo` は ••• の menu の項目と古い木
    "redo":      ("AXButton",    "↷ やり直す",             ("Redo · ⇧⌘Z", "↷ Redo", "Redo"), None, "en"),
    # 候補 EN（tuple には入れない・決まったら入れて status を "en" に）: UI-5a で title line の ‹ の席の AX の名前は `Back to {Page}`（copy.en.md:15・⌘[ は UI-5b）。
    # {Page} が実行時に埋まり、完全一致では引けず「Back to」の部分一致は `Back to projects` にも当たるので、前方一致の引き方を足すまで pending のまま
    "back":      ("AXButton",    "← 戻る",                 ("← Back", "Back"), None, "pending:UI-5a"),
    # UI-5a: ••• の menu の項目（copy.en.md:158・⌃`）。先に options を押して開く
    "drawer":    ("AXButton",    "Terminal の引き出し",    ("Show terminal", "Hide terminal"), None, "en"),
    # 候補 EN（tuple には入れない・決まったら入れて status を "en" に）: 出典無し（seed の Page 名。sidebar の tab の label は UI-5a で決まる）
    "memo":      ("AXCheckBox",  "メモ",                   (), None, "pending:UI-5a"),
    # Screen recording is off: yc/ui src/surface/mirror.rs:986（MirrorError::ScreenCaptureDenied の 1 行・" · Open settings" が続くので部分一致）（W15-b45）
    "mirror_denied": ("AXStaticText", "許可が要ります",    ("Screen recording is off",), None, "en"),
    # 状態の行の語（部分一致で引く・outside.en.md の `| JP | EN |` の EN の頭）
    "checking":  ("AXStaticText", "確かめています",        ("Checking",), None, "en"),
    "running_cmd": ("AXStaticText", "走らせています",      ("Running (",), None, "en"),
    "exit_code": ("AXStaticText", "終了 code",             ("Exit code",), None, "en"),
    "usage_refuse": ("AXStaticText", "画面を開きません",   ("isn't opened",), None, "en"),
    # 判定の側が読む文（W15-b36・w15_judge / tally）。EN の出典: saved_line = outside.en.md:352・copy.en.md:317／connected = copy.en.md:435／saved_msg・finished = tally が元から持つ EN の語（Saved・Done）
    "saved_line": ("AXStaticText", "に置きました",          ("Placed it in", "Saved · checked on disk"), None, "en"),
    "connected": ("AXStaticText", "接続済み",              ("Connected",), None, "en"),
    "saved_msg": ("AXStaticText", "保存しました",          ("Saved",), None, "en"),
    "finished":  ("AXStaticText", "完了",                  ("Done",), None, "en"),
    # 候補 EN（tuple には入れない・決まったら入れて status を "en" に）: Failed（yc/ui additions.md:350）／ × Failed（copy.en.md:480）／ Failed · {reason}（copy.en.md:360）
    "failed":    ("AXStaticText", "失敗",                  (), None, "pending:UI-n"),
    # 候補 EN（tuple には入れない・決まったら入れて status を "en" に）: 出典無し（src/llm/mod.rs:108 の文は yc/ui の表に無い）
    "ai_cap":    ("AXStaticText", "AI を呼ぶ上限",         ("AI was stopped at its limit",), None, "en"),
    # 候補 EN（tuple には入れない・決まったら入れて status を "en" に）: 出典無し（src/domain/terminal.rs:170 の文は yc/ui の表に無い）
    # 再起動した app の入口（#25）: Project 名（src/composer/project.rs:563,627・copy.en.md:542）。sidebar の Project の入口（AXCheckBox・w15_judge.restart_entry・drive-integ.py）
    "dev_env":   ("AXCheckBox", "開発環境",               ("Dev environment",), None, "en"),
    "human_typing": ("AXStaticText", "人が Terminal に打っている間は", (), None, "pending:UI-n"),
    # W15-M3（scene 2）: 承認の 1 行の「押す先」。G の M2（G-e）が入れる語: `Press Track › Insert new track in REAPER` / `Press “general” in Discord`。
    # jp は画面に出ない placeholder（空にすると jp_literals の部分一致が全 literal に当たる）。M2 の commit の後に en を埋めて status を "en" にする（pending の間 ask2 は照合しない）
    "press_track":   ("AXStaticText", "（M2 待ち）押す先: menu",  (), None, "pending:M2"),
    "press_channel": ("AXStaticText", "（M2 待ち）押す先: click", (), None, "pending:M2"),
}


def names(x):
    """x = 表の key なら EN → JP の順の全表記・それ以外は literal 1 語。"""
    if x in LABELS:
        _, jp, en, _, _ = LABELS[x]
        return tuple(en) + (jp,)
    return (x,)


def ident(x):
    return LABELS[x][3] if x in LABELS else None


def eq(x):
    """値の完全一致の述語（drive-w15 / drive-integ1 の `eq`）。"""
    n = names(x)
    return lambda v: v in n


def has(x):
    n = names(x)
    return lambda v: any(s in v for s in n)


def hit(e, x, contains=False):
    """AX の要素 e（label / value / title / identifier）が x に当たるか。identifier が在れば先に見る。"""
    i = ident(x)
    eid = e.get("identifier") or ""
    if i and (eid.startswith(i[:-1]) if i.endswith("*") else eid == i):
        return True
    ok = has(x) if contains else eq(x)
    return any(ok(e.get(k) or "") for k in ("label", "value", "title"))


if __name__ == "__main__":
    # shell 用（W15-b44）: --print <key> = 探す語（EN → JP）を 1 行ずつ / --token <tree.json> <key> [role] = 当たった最初の要素の element_token（部分一致・identifier も見る）
    import json, sys
    a = sys.argv[1:]
    if a[:1] == ["--print"] and len(a) == 2:
        print("\n".join(names(a[1])))
    elif a[:1] == ["--token"] and len(a) in (3, 4):
        try:
            els = json.load(open(a[1])).get("elements", [])
        except Exception:
            sys.exit(0)
        for e in els:
            if (len(a) < 4 or not a[3] or e.get("role") == a[3]) and hit(e, a[2], contains=True):
                print(e["element_token"]); break
    else:
        sys.exit("usage: labels.py --print <key> | --token <tree.json> <key> [role]")
