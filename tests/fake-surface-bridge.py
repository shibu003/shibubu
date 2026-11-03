#!/usr/bin/env python3
"""tests/portal.rs が使う偽の bridge（bridge/macos-surface/KangaeSurfaceBridge.swift と同じ 1 行 1 JSON の口）。
macOS の AX にも元の app にも触らない。振る舞いは環境変数で:
  FAKE_MODE      ok（既定）/ untrusted / slow / garbage / die / noready
  FAKE_SLOW      slow の時に各応答の前に待つ秒（既定 3）
  FAKE_AX_TIMEOUT ready で名乗る AX の timeout（秒・既定 1.0）。読み手の待つ上限はこれから決まる
  FAKE_TITLES    windows が返す題名を | で区切る（n 回目の windows は n 番目・最後を使い回す）。既定 "test.rpp - REAPER"
  FAKE_WINDOWS   0 なら windows は空
  FAKE_BUSY_WINDOWS 最初の n 回の windows は ax_cannot_complete（app が応答しません・映した直後の Numbers の実測・B14）
  FAKE_MIN_W / FAKE_MIN_H  set_frame の読み戻しをこの大きさ未満にしない（app の最小の大きさ）
  FAKE_MAX_W / FAKE_MAX_H  set_frame の読み戻しをこの大きさより大きくしない（大きくできない app・I31）
  FAKE_W / FAKE_H  窓 w1 の始めの大きさ（既定 800×600・MIRROR-FIT-1101 の Mirror の試験は枠と同じ 400×272）
  FAKE_PARTIAL_SETS 最初の n 回の set_frame は途中の大きさ（幅 85%・高さ -4）で止まる（背景の TextEdit の実測・W16-1 3 回目）
  FAKE_LAGGED_SETS 最初の n 回の set_frame の応答の actual は途中の大きさ（幅 85%・高さ -4）だが、窓は枠に落ち着く（後の frame は枠を返す・前に居る TextEdit の実測・W16-3 pane N）
  FAKE_OTHER_PID  apps が同じ bundle の process をもう 1 つ（この pid・窓なし）先に返す（`open -n` で 2 つ目の TextEdit を起こした場面）
  FAKE_STEAL     1 なら activate を頼まない raise の後も frontmost を元の app（222）にする（focus を奪った場面）
  FAKE_HOST_ABOVE 1 なら cg_windows で Kangae の窓を docked の窓より手前にする（AXRaise では上に出ない場面）
  FAKE_BETWEEN   1 なら Kangae と docked の間に、枠と重なる別の app の窓を 1 つ置く
  FAKE_BUTTONS   mouse の buttons
  FAKE_HOST_PID  cg_windows で Kangae の窓として出す pid
  FAKE_LOG       受け取った要求を 1 行ずつ追記する file
  FAKE_NO_CG     1 なら windows の窓に cg_id を付けない（attach の時に CG の窓番号と対応が付かない場面）
  FAKE_NO_CG_ROUNDS 最初の n 回の windows の窓に cg_id を付けない（起こした app の窓ができた直後・B89）
  FAKE_DIALOG_CG windows が w1 の前に、動かせない main の dialog（handle w9・CG の番号はこの値）を返す・w1 は main でない（REAPER の起動の評価版の dialog・B89）
  FAKE_WL        window_list が返す窓を JSON で: 回ごとの配列の配列（n 回目は n 番目・最後を使い回す）。
                 1 つの窓 = {"title": "...", "on_screen": true, "no_cg": true}（no_cg = AX の窓に CG の窓番号が付かない・cg_match ambiguous）。無ければ FAKE_TITLES の窓 1 つ。
                 G31b: "cg": "none"（AX の枠が CG にまだ無い・cg_match none）・"main"（既定は 1 つ目だけ）・"subrole"（既定 AXStandardWindow）・
                 "movable"（既定 true = position / size settable）・"minimized"・
                 G77: "pid"（その pid の window_list にだけ出す・FAKE_APP_MORE_PID の 2 つ目の起動の窓。無ければどの pid にも出す・窓番号は全体の並びの 300 + i）
  FAKE_WL_SLOW   window_list の応答の前に待つ秒（混んだ機械で bridge が 3 × ax_timeout まで読む場面・G31 Zoom の 9.18 秒）
  FAKE_AX_VALUE  ax_find がこの文字列を「窓の中の要素の value」として持つ（query を含めば hit）
  FAKE_INSTALLED 0 なら find_app は installed: false（既定 1・版 1.19）
  FAKE_GONE_FILE  この file が在る間は FAKE_HIDE_DOCKED=1 と同じ（試験の途中で窓を閉じた場面）
  FAKE_OFF_SCREEN_FILE  この file が在る間、cg_windows の docked の窓（500）は残し on_screen=false（別 Space。最小化ではない）
  FAKE_FULLSCREEN_FILE  この file が在る間、frame / windows の fullscreen=true（元の窓の全画面）
  FAKE_MINIMIZED_FILE  この file が在る間、frame の minimized=true かつ cg の on_screen=false（最小化。別 Space ではない）
  FAKE_FRAME_SLOW_FILE この file が在る間、frame の応答の前に 3 秒待つ（nap の TextEdit の AX・W16-2 実機 13〜31 秒。その間は他の要求も待つ）
  FAKE_APP_BUNDLE / FAKE_APP_NAME  apps が返す起動中の app（既定 com.cockos.reaper / REAPER）
  FAKE_APP_PID   その app の pid（既定 222。元の app を起こし直した場面 = 別の pid・W19-3b）
  FAKE_APP_MORE_PID 在れば apps が同じ app の 2 つ目の起動をこの pid で返す（待つ app の数 × 上限の 1 行・W19-3b G2）
  FAKE_HIDE_DOCKED 1 なら cg_windows（今の画面）に docked の窓（500）を入れない（窓が閉じた場面）
  FAKE_AX_TEXT_FILE ax_text が窓の文書としてこの file の中身を毎回読み直して返す（試験の途中で編集・Undo を書き換える）。無い・未設定なら ax_no_text_area
  FAKE_AX_SLOW   ax_text の応答の前に待つ秒（混んだ機械の TextEdit の実測は 16 秒・その間は他の要求も待つ = 本物の bridge と同じ 1 本の列）
  ax_tree        w1〜wN（window_list の窓）に、窓 + 「保存」AXButton + 本文 AXTextArea（settable・FAKE_AX_VALUE・本物と同じく 2000 字で切る）+ 枠 AXGroup の 4 要素を返す。FAKE_AX_UNLABELED=1 なら label が空白だけの AXButton（AXPress）を 1 つ足す。
                 FAKE_AX_SCROLLBARS=1 なら本文の隣に AXScrollBar 2 つ（AXValue = 値を置ける数・本物の TextEdit の書類の窓の形・G80）。
                 snapshot は呼ぶたびに 1 上がる（bridge 全体で 1 本）。FAKE_AX_PARTIAL=1 なら窓だけ読んだ所で app が答えなくなる（complete = false・
                 paused "app"・続き〈more〉も 0 要素のまま）。
                 owner 256: 本物と同じく数で切らず、1 回の応答を区切って complete = false・paused "time" を返し、more: true で同じ snapshot の続きを返す。
                 FAKE_AX_SLICE = 1 回の応答の要素の数（本物は時間で区切る・既定 = 全部を 1 回で）。FAKE_AX_MANY = 押せる AXButton「Item N」を N 個足す。
                 max_elements が来たら前の bridge と同じくそこで切る（complete = false・paused 無し・続き無し = 上限を戻すと全部は読めない負の対照）
                 FAKE_AX_DUP=1 なら「保存」AXButton を 2 つ・FAKE_AX_NOT_SETTABLE=1 なら本文を value_settable = false にする
  操作（YC W17-2）: ax_press / ax_set_value は最新の ax_tree の ref だけを受ける（古い snapshot は stale_element・actions に無い action は
                 action_unsupported・置けない要素は not_settable）。ax_set_value は本文の値を書き換え（次の ax_tree に出る）、key_to_pid の text は
                 本文の後ろに足す（前面の欄に打った場面）。ax_menu は FAKE_MENU（"ファイル>保存|…"・既定 "ファイル>保存"）の path だけ押せる。
                 ax_replace_text（G79）は本文の中で replace と完全一致する所がちょうど 1 か所の時だけそこを value に（他は ax_replace_count で何も置かない）
  FAKE_AX_FAIL   この code で 4 つの操作を失敗させる（cannot_complete = 送った後かもしれない・それ以外 = 送っていない）
  FAKE_FRONT_PID frontmost が返す pid（既定 1 = Shibubu。222 = 操作の宛先の app が前面 = 人が触っている場面・W19-1 の人の lease）
  FAKE_FRONT_CHANGE 1 なら操作の front_after を front_before と違えて返す（前面を奪った場面・D-g）
  FAKE_SET_MISMATCH 1 なら ax_set_value の read_back を置く前の値にする（app が値を受け付けなかった場面）
                 ax_set_value の result の via は本文（AXTextArea）なら "selected_text"・他は "value"（本物の bridge と同じ選び方・W17-2 戻し）
  FAKE_SET_LASTS rich text の欄（G74・③-8 の Discord の入力欄）: ax_set_value は「置けた・合う」と答える（read_back = 頼んだ値・matches = true）が、
                 欄の中身は次からの ax_tree n 回だけ頼んだ値で、その後は置く前の値に戻る（0 = 初めから変わらない・置いた道だけが合うと言う）
  FAKE_SET_BACK  在れば FAKE_SET_LASTS の n 回の後に戻す先を置く前の値でなくこの値にする（G75: 置いた字が消えたが「変わった」とは読める場面）
  FAKE_MENU_ADDS 1 なら ax_menu を押すたびに、次からの ax_tree の末尾に AXButton「Track N」を 1 つ足す（REAPER の Track > Insert new track・M2 G-b）。
                 無ければ menu を押しても木は変わらない（読み戻しが ref の違いだけで「変わった」と言わないかを見る）
  FAKE_AX_ROWS   | 区切りの名前ごとに、ax_tree の末尾に押せる AXLink「<名前> (text channel)」を足す（Discord の channel の行・M2 G-c。本物の role と label は未測）
  FAKE_ROW_TITLE 在れば、FAKE_AX_ROWS の行を ax_press で押した後の window_list の窓の題名をこの文字の {} を押した行の名前にした物にする
                 （channel を開くと窓の題名が変わる・M2 G-d）。元の題名が空の窓（画面収録の許可が無い場面）は空のまま
  FAKE_MENU_STALE 1 なら ax_menu の result の enabled_before を false にする（背景の app の menu の AXEnabled が古く灰色に読める場面・押すのは同じ・W17-2 戻し）
  FAKE_SAVE_TO   path なら ax_menu を押した FAKE_SAVE_MS（既定 0）ms 後に、押した時の本文をその file に書く（背景の TextEdit の File › Save が disk に
                 遅れて届く場面・r25 #20 で ≈ 17 秒・G81）。無ければ disk には何も書かない（保存が届かない場面）
  FAKE_DIE_ON    この cmd を受け取ったら応答せずに落ちる（送った後に bridge が切れた場面）
  FAKE_BODY_FILE 本文の値をこの file に持つ（要求ごとに読み直し・置いたら書く）。bridge を起こし直しても・Shibubu を開き直しても値が残る元の app（WF2 の BG の通し）
  FAKE_WL_WAKE   window_list の app が起きるのに要る秒: 起きるまでは config の ax_timeout（既定 1）がこれ未満なら窓 0 個と ax_error の cannot_complete・
                 1 回答えたら起きている（裏に押し出された app・W19-3b 実機で 1 回目 14.3 秒・2 回目 0.1 秒）
  FAKE_AX_WAKING 最初の n 回の ax_tree は窓だけ（complete = true）で waking = true（Chromium の中身を頼んだ直後・G31 Discord の実測）
  FAKE_AX_TREE_WAKE ax_tree の app が答えるのに要る秒: config の ax_timeout がこれ未満の間は elements 空・complete = false（窓の最初の読みが
                 cannot_complete・G31 Zoom / REAPER の混んだ機械）。伸ばした読みで答えたら起きている
  FAKE_MESSAGES  | 区切りの本文ごとに message の行（G74b・Discord の chat の行を真似る。本物の role と label は未測）: AXGroup（subrole
                 AXDocumentArticle・label = 本文）とその子の AXStaticText（label = 本文）。どちらも AXShowMenu を持つ。行に AXShowMenu → menu（G74c:
                 Discord 15:0x の実測の形 = AXRole は AXGroup（AXMenu ではない）・label「Message Actions」・AXCancel を持つ・子は AXMenuItem（AXPress））。
                 menu が見えている間は message の行が木から消える（実測: 582 → 75 要素・行 0）。項目「Delete Message」→ 押すと AXGroup（subrole
                 AXApplicationDialog・label「Delete Message」）が開き、中に本文の preview の AXStaticText と AXButton「Delete」「Cancel」。
                 Delete でその行が消え dialog が閉じる・Cancel は閉じるだけ。他の項目は menu を閉じるだけ
  FAKE_MENU_ITEMS  | 区切りの menu の項目（既定 "Add Reaction|Reply|Copy Text|Delete Message|Copy Message ID"）
  FAKE_MENU_LATE   n なら AXShowMenu の後の n 回の ax_tree には menu がまだ無い（描かれる前・行は見えたまま・G74c）
  FAKE_MENU_NEVER  1 なら AXShowMenu を受けても menu を開かない（身振りが届かなかった場面）
  FAKE_MENU_STICKY 1 なら menu への AXCancel は rc 0 で答えるが閉じない（Discord 15:0x の実測）
  FAKE_MENU_AT     n なら初めから n 行目の message の menu が開いている（前の run が開いたまま残した場面・G74d）
  FAKE_ITEM_DEAD   1 なら menu の項目への AXPress は rc 0 で答えるが何も起きない（menu は開いたまま・dialog は出ない = 本物 17:10 の 3 回目・G74e）
  FAKE_DIALOG_LATE n なら Delete Message の後の n 回の ax_tree には確認の dialog がまだ無い（描かれる前・menu は閉じている・G74e）
  FAKE_MESSAGES_OFF n なら menu は押した行から n 行後ろの message の物になる（app が別の行の menu を開いた場面 = dialog の preview が別の本文）
  FAKE_ITEM_FOCUS  1 なら menu の項目への AXPress は focus: true（AXFocused を置いてから押す）の時だけ効き、無ければ FAKE_ITEM_DEAD と同じ（本物 18:45・G74f）
  FAKE_HIDDEN      1 なら apps が宛先の app を hidden: true で返す（⌘H で隠した app・本物 18:37 = AXShowMenu は rc 0 で答えたが menu は出なかった・G74f）
  FAKE_HIDE_AFTER_DELETE 1 なら dialog の Delete を押した後から apps が宛先の app を hidden: true で返す（送った後に ⌘H・FX-114）
  FAKE_UNHIDE_STUCK 1 なら app_unhide を受けても隠れたまま（hidden: true を返す）。無ければ app_unhide の後の apps は FAKE_HIDDEN でも hidden: false（W8a）
  FAKE_OPEN_STATE  file の path。open_document で開いた書類をそこに書き、次の起動の window_list も読む（Engine は run ごとに bridge を起こし直す = 本物の app の窓は残る・CHAT-013）
  ax_tree の root  {role, label} なら窓とその要素（label は大文字小文字を問わず含む）から下だけ・root_found を返す（FX-112）。FAKE_ROOT_LOG = 頼まれた root を 1 行ずつ足す file
"""
import json
import os
import sys
import time

E = os.environ
MODE = E.get("FAKE_MODE", "ok")
APP_PID = int(E.get("FAKE_APP_PID", "222"))


def out(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def reply(rid, result):
    out({"id": rid, "ok": True, "result": result})


def fail(rid, code, message):
    out({"id": rid, "ok": False, "error": {"code": code, "message": message}})


if MODE == "noready":
    time.sleep(60)
    sys.exit(0)

trusted = MODE != "untrusted"
out({"event": "ready", "version": "fake", "pid": os.getpid(), "trusted": trusted, "ax_timeout": float(E.get("FAKE_AX_TIMEOUT", "1.0")), "t": 0})


def flag_file(key):
    p = E.get(key)
    return bool(p) and os.path.exists(p)


def docked_off_screen():
    return E.get("FAKE_OFF_SCREEN") == "1" or flag_file("FAKE_OFF_SCREEN_FILE") or source_minimized()


def source_fullscreen():
    return E.get("FAKE_FULLSCREEN") == "1" or flag_file("FAKE_FULLSCREEN_FILE")


def source_minimized():
    return E.get("FAKE_MINIMIZED") == "1" or flag_file("FAKE_MINIMIZED_FILE")

titles = E.get("FAKE_TITLES", "test.rpp - REAPER").split("|")
windows_calls = 0
wl_calls = 0
set_calls = 0
busy_windows = 0
snapshot = 0
ax_timeout = 1.0
awake = False
tree_awake = False
tree_calls = 0
body = E.get("FAKE_AX_VALUE", "")
tracks = 0  # FAKE_MENU_ADDS の ax_menu で増える
rows = [r for r in E.get("FAKE_AX_ROWS", "").split("|") if r]
row_refs = {}  # 最新の ax_tree の FAKE_AX_ROWS の行の ref → 名前
opened = None  # 押した FAKE_AX_ROWS の行の名前
hidden_now = False  # FAKE_HIDE_AFTER_DELETE の Delete の後に apps が hidden: true（FX-114）
unhidden = False  # W8a: app_unhide を受けた（FAKE_UNHIDE_STUCK でなければ、その後の apps は FAKE_HIDDEN でも hidden: false）
last_refs = {}  # 最新の ax_tree の ref → 要素
walk = None  # owner 256: 読みかけの木（snapshot・els〈None = app がまだ答えない〉・pos・extra・stuck）。more の ax_tree が続きを読む
set_revert = None  # FAKE_SET_LASTS: (あと何回の ax_tree で頼んだ値を見せるか, 戻す値)
messages = [m for m in E.get("FAKE_MESSAGES", "").split("|") if m]
msg_refs = {}  # 最新の ax_tree の FAKE_MESSAGES の要素の ref → ("row", i) / ("menu", None) / ("item", 項目) / ("delete", None) / ("cancel", None)
menu_for = int(E["FAKE_MENU_AT"]) if "FAKE_MENU_AT" in E else None  # context menu が開いている message の行
menu_late = 0  # FAKE_MENU_LATE: menu を見せるまでの残りの ax_tree の回数
items = E.get("FAKE_MENU_ITEMS", "Add Reaction|Reply|Copy Text|Delete Message|Copy Message ID").split("|")
dialog_for = None  # 消す確認の dialog が開いている message の行
dialog_late = 0  # FAKE_DIALOG_LATE: dialog を見せるまでの残りの ax_tree の回数


opened_doc = None  # CHAT-013: open_document で開いた file（次の window_list からその書類の窓が在る）
if E.get("FAKE_OPEN_STATE") and os.path.isfile(E["FAKE_OPEN_STATE"]):
    opened_doc = open(E["FAKE_OPEN_STATE"], encoding="utf-8").read() or None
typed = False  # FX-115: ax_set_value / ax_replace_text を 1 回でも受けた（FAKE_TITLE_AFTER_SET の題名に替える）


def tree_slice(rid, h):
    """owner 256: 読みかけ（walk）の続きを 1 区切り返す（本物の bridge の axTree と同じ形: elements は新しく読めた物だけ・ref は通し番号のまま）"""
    global walk
    w = walk
    k = int(E.get("FAKE_AX_SLICE", "0")) or len(w["els"])
    part = w["els"][w["pos"]:w["pos"] + k]
    w["pos"] += len(part)
    head = {"handle": h, "snapshot": w["snapshot"], "elements": part, "visited": w["pos"]}
    if w["stuck"]:
        reply(rid, dict(head, complete=False, paused="app", **w["extra"]))
        return
    done = w["pos"] >= len(w["els"])
    if done:
        walk = None
    reply(rid, dict(head, complete=done, **({} if done else {"paused": "time"}), **w["extra"]))


def window_list(pid):
    """CG の窓（前から後ろ・layer 0 と 1680×24 の帯 1 本）と AX の窓（許可が無ければ ax_error）"""
    global wl_calls, awake
    rounds = json.loads(E["FAKE_WL"]) if E.get("FAKE_WL") else [[{"title": titles[0], "on_screen": True}]]
    ws = rounds[min(wl_calls, len(rounds) - 1)]
    wl_calls += 1
    if opened_doc is not None:
        ws = [dict(w, main=False) for w in ws] + [{"title": os.path.basename(opened_doc), "document": "file://" + opened_doc, "main": True}]
    cg = [{"id": 900, "pid": pid, "layer": 0, "order": 0, "on_screen": False, "frame": {"x": 0, "y": 0, "w": 1680, "h": 24}}]
    ax = []
    for i, w in enumerate(ws):
        if w.get("pid", pid) != pid:
            continue
        frame = {"x": 100.0 + i, "y": 100.0, "w": 800.0, "h": 600.0}
        title = E["FAKE_ROW_TITLE"].replace("{}", opened) if opened is not None and E.get("FAKE_ROW_TITLE") and w["title"] else w["title"]
        if typed and E.get("FAKE_TITLE_AFTER_SET") and i == 0:
            title = E["FAKE_TITLE_AFTER_SET"]
        cg.append({"id": 300 + i, "pid": pid, "layer": 0, "order": 10 - i, "on_screen": w.get("on_screen", True), "frame": frame, "title": title})
        movable = w.get("movable", True)
        ax.append({"handle": "w%d" % (i + 1), "pid": pid, "title": title, "role": "AXWindow", "subrole": w.get("subrole", "AXStandardWindow"),
                   "minimized": w.get("minimized", False), "main": w.get("main", i == 0), "focused": i == 0, "fullscreen": False, "position_settable": movable,
                   "size_settable": movable, "frame": frame, "cg_id": 300 + i, "cg_match": "frame", "on_screen": w.get("on_screen", True),
                   "document": w.get("document", "")})
        if w.get("no_cg"):
            del ax[-1]["cg_id"]
            ax[-1]["cg_match"] = "ambiguous"
        if w.get("cg") == "none":
            # AX の枠が CG にまだ無い（起動直後に窓が並べ直される間・G31b）
            del ax[-1]["cg_id"]
            ax[-1]["cg_match"] = "none"
            cg.pop()
    time.sleep(float(E.get("FAKE_WL_SLOW", "0")))
    out = {"pid": pid, "cg_windows": cg, "windows": ax if trusted else []}
    awake = awake or ax_timeout >= float(E.get("FAKE_WL_WAKE", "0"))
    if trusted and not awake:
        out["windows"] = []
        out["ax_error"] = {"code": "ax_cannot_complete", "message": "窓の一覧を読めません: app が応答しません（忙しい・dialog が開いている）"}
    if not trusted:
        out["ax_error"] = {"code": "not_trusted", "message": "アクセシビリティの許可がありません"}
    return out
frame = {"x": 100.0, "y": 100.0, "w": float(E.get("FAKE_W", "800")), "h": float(E.get("FAKE_H", "600"))}

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    if E.get("FAKE_LOG"):
        with open(E["FAKE_LOG"], "a", encoding="utf-8") as f:
            f.write(line + "\n")
    if E.get("FAKE_DIE_ON") and ('"cmd": "%s"' % E["FAKE_DIE_ON"] in line or '"cmd":"%s"' % E["FAKE_DIE_ON"] in line):
        sys.exit(3)
    if MODE == "die":
        sys.stderr.write("fake bridge: 落ちます\n")
        sys.exit(3)
    if MODE == "slow":
        time.sleep(float(E.get("FAKE_SLOW", "3")))
    if MODE == "garbage":
        sys.stdout.write("this is not json\n")
        sys.stdout.flush()
    if E.get("FAKE_BODY_FILE") and os.path.exists(E["FAKE_BODY_FILE"]):
        with open(E["FAKE_BODY_FILE"], encoding="utf-8") as f:
            body = f.read()
    try:
        req = json.loads(line)
    except ValueError:
        out({"id": None, "ok": False, "error": {"code": "bad_json", "message": "1 行 1 つの JSON object で"}})
        continue
    rid, cmd = req.get("id"), req.get("cmd")
    if cmd == "hello":
        reply(rid, {"version": "fake", "trusted": trusted})
    elif cmd == "permission":
        reply(rid, {"trusted": trusted, "prompted": bool(req.get("prompt"))})
    elif cmd == "config":
        ax_timeout = float(req["ax_timeout"])
        reply(rid, {"ax_timeout": ax_timeout})
    elif cmd in ("windows", "observe", "set_frame", "raise", "frame", "minimize") and not trusted:
        fail(rid, "not_trusted", "アクセシビリティの許可がありません")
    elif cmd == "window_list":
        reply(rid, window_list(req.get("pid")))
    elif cmd == "ax_find":
        if not trusted:
            fail(rid, "not_trusted", "アクセシビリティの許可がありません")
            continue
        value = E.get("FAKE_AX_VALUE", "")
        q = req.get("query", "")
        hits = [{"role": "AXTextField", "attribute": "AXValue", "value": value}] if q and q in value else []
        reply(rid, {"handle": req.get("handle"), "hits": hits, "visited": 42, "complete": True})
    elif cmd == "ax_text":
        if not trusted:
            fail(rid, "not_trusted", "アクセシビリティの許可がありません")
            continue
        if req.get("handle") != "w1":
            fail(rid, "unknown_handle", "その窓の handle は知りません")
            continue
        time.sleep(float(E.get("FAKE_AX_SLOW", "0")))
        path = E.get("FAKE_AX_TEXT_FILE", "")
        if path and os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                reply(rid, {"handle": "w1", "text": f.read(), "reason": "ok"})
        else:
            reply(rid, {"handle": "w1", "text": None, "reason": "ax_no_text_area"})
    elif cmd == "ax_tree":
        if not trusted:
            fail(rid, "not_trusted", "アクセシビリティの許可がありません")
            continue
        h = req.get("handle") or ""
        if not (h.startswith("w") and h[1:].isdigit() and int(h[1:]) >= 1):
            fail(rid, "unknown_handle", "その窓の handle は知りません（windows で取り直してください）")
            continue
        more = req.get("more") is True
        if more and walk is None:
            fail(rid, "bad_args", "続きを読む木がありません（more の無い ax_tree から読み直してください）")
            continue
        if more and walk["els"] is not None:
            tree_slice(rid, h)
            continue
        if not more:
            snapshot += 1
            tree_calls += 1
            walk = None
        r = lambda n: "e%d.%d" % (snapshot, n)
        tree_awake = tree_awake or ax_timeout >= float(E.get("FAKE_AX_TREE_WAKE", "0"))
        if not tree_awake:
            # app がまだ答えない: 本物と同じく paused "app"・続き（more）で同じ snapshot を読み直す
            last_refs = {}
            walk = {"snapshot": snapshot, "els": None, "pos": 0, "extra": {}, "stuck": False}
            reply(rid, {"handle": h, "snapshot": snapshot, "elements": [], "visited": 1, "complete": False, "paused": "app"})
            continue
        if not more and tree_calls <= int(E.get("FAKE_AX_WAKING", "0")):
            last_refs = {r(0): {"actions": ["AXRaise"]}}
            reply(rid, {"handle": h, "snapshot": snapshot, "visited": 1, "complete": True, "waking": True,
                        "elements": [{"ref": r(0), "role": "AXWindow", "subrole": "AXStandardWindow", "label": titles[0], "value": None,
                                      "actions": ["AXRaise"], "value_settable": False, "enabled": True, "depth": 0, "parent": None}]})
            continue
        if set_revert is not None:
            n, prev = set_revert
            if n == 0:
                body, set_revert = prev, None
            else:
                set_revert = (n - 1, prev)
        els = [{"ref": r(0), "role": "AXWindow", "subrole": "AXStandardWindow", "label": titles[0], "value": None, "actions": ["AXRaise"],
                "value_settable": False, "enabled": True, "depth": 0, "parent": None}]
        if E.get("FAKE_AX_PARTIAL") != "1":
            els += [
                {"ref": r(1), "role": "AXGroup", "subrole": "", "label": "", "value": None, "actions": [], "value_settable": False, "enabled": True, "depth": 1, "parent": r(0)},
                {"ref": r(2), "role": "AXButton", "subrole": "", "label": "保存", "value": None, "actions": ["AXPress"], "value_settable": False, "enabled": True, "depth": 1, "parent": r(0)},
                {"ref": r(3), "role": "AXTextArea", "subrole": "", "label": "本文", "value": body[:2000], "actions": [], "value_settable": E.get("FAKE_AX_NOT_SETTABLE") != "1", "enabled": True, "depth": 2, "parent": r(1)},
            ]
            if E.get("FAKE_AX_SCROLLBARS") == "1":
                # G80: 本物の TextEdit の書類の窓（r23・bridge の ax_tree の実測）は本文の隣に AXScrollBar が 2 つ在り、AXValue は値を置ける数（bridge は数も字にする）
                for v in ("0.4285714285714286", "0"):
                    els.append({"ref": r(len(els)), "role": "AXScrollBar", "subrole": "", "label": "", "value": v, "actions": [], "value_settable": True, "enabled": True, "depth": 2, "parent": r(1)})
            if E.get("FAKE_AX_DUP") == "1":
                els.append({"ref": r(len(els)), "role": "AXButton", "subrole": "", "label": "保存", "value": None, "actions": ["AXPress"], "value_settable": False, "enabled": True, "depth": 1, "parent": r(0)})
            for n in range(tracks):
                els.append({"ref": r(len(els)), "role": "AXButton", "subrole": "", "label": "Track %d" % (n + 1), "value": None, "actions": ["AXPress"], "value_settable": False, "enabled": True, "depth": 1, "parent": r(0)})
            row_refs = {}
            for name in rows:
                row_refs[r(len(els))] = name
                els.append({"ref": r(len(els)), "role": "AXLink", "subrole": "", "label": "%s (text channel)" % name, "value": None, "actions": ["AXPress"], "value_settable": False, "enabled": True, "depth": 2, "parent": r(1)})
            if E.get("FAKE_AX_UNLABELED") == "1":
                els.append({"ref": r(len(els)), "role": "AXButton", "subrole": "", "label": " ", "value": None, "actions": ["AXPress"], "value_settable": False, "enabled": True, "depth": 1, "parent": r(0)})
            for n in range(int(E.get("FAKE_AX_MANY", "0"))):
                els.append({"ref": r(len(els)), "role": "AXButton", "subrole": "", "label": "Item %d" % (n + 1), "value": None, "actions": ["AXPress"], "value_settable": False, "enabled": True, "depth": 1, "parent": r(0)})
            msg_refs = {}
            def add(kind, i, role, subrole, label, actions, depth, parent):
                ref = r(len(els))
                if kind:
                    msg_refs[ref] = (kind, i)
                els.append({"ref": ref, "role": role, "subrole": subrole, "label": label, "value": None, "actions": actions, "value_settable": False, "enabled": True, "depth": depth, "parent": parent})
                return ref
            shown = menu_for is not None and menu_late == 0
            if menu_for is not None and menu_late > 0:
                menu_late -= 1
            for i, m in enumerate(messages if not shown else []):
                row = add("row", i, "AXGroup", "AXDocumentArticle", m, ["AXShowMenu"], 2, r(1))
                add("row", i, "AXStaticText", "", m, ["AXShowMenu"], 3, row)
            if shown:
                menu = add("menu", None, "AXGroup", "", "Message Actions", ["AXCancel"], 1, r(0))
                for it in items:
                    add("item", it, "AXMenuItem", "", it, ["AXPress"], 2, menu)
            if dialog_for is not None and dialog_late > 0:
                dialog_late -= 1
            elif dialog_for is not None:
                d = add(None, None, "AXGroup", "AXApplicationDialog", "Delete Message", [], 1, r(0))
                add(None, None, "AXStaticText", "", messages[dialog_for], [], 2, d)
                add("delete", None, "AXButton", "", "Delete", ["AXPress"], 2, d)
                add("cancel", None, "AXButton", "", "Cancel", ["AXPress"], 2, d)
        root_found = None
        if req.get("root"):
            # FX-112: 本物と同じ: 窓から幅優先で role と label（大文字小文字を問わず含む）の合う最初の要素を探し、窓とその要素から下だけを返す
            want = req["root"]
            hit = next((e for e in els[1:] if e["role"] == want.get("role") and want.get("label", "").lower() in e["label"].lower()), None)
            root_found = hit is not None
            keep = {hit["ref"]} if hit else set()
            for e in els[1:]:
                if e["parent"] in keep:
                    keep.add(e["ref"])
            els = [els[0]] + ([dict(hit, parent=els[0]["ref"])] if hit else []) + [e for e in els[1:] if e["ref"] in keep and e is not hit]
            if E.get("FAKE_ROOT_LOG"):
                open(E["FAKE_ROOT_LOG"], "a").write(json.dumps(want, ensure_ascii=False) + "\n")
        if req.get("actionable_only"):
            els = [e for e in els if e["depth"] == 0 or e["actions"] or e["value_settable"]]
            for e in els[1:]:
                e["parent"] = r(0)
        last_refs = {e["ref"]: e for e in els}
        extra = {} if root_found is None else {"root_found": root_found}
        cap = req.get("max_elements")
        if isinstance(cap, int) and len(els) > cap:
            walk = None
            reply(rid, dict({"handle": h, "snapshot": snapshot, "elements": els[:cap], "visited": cap, "complete": False}, **extra))
            continue
        walk = {"snapshot": snapshot, "els": els, "pos": 0, "extra": extra, "stuck": E.get("FAKE_AX_PARTIAL") == "1"}
        tree_slice(rid, h)
    elif cmd in ("ax_press", "ax_set_value", "ax_replace_text", "ax_menu", "key_to_pid"):
        if not trusted:
            fail(rid, "not_trusted", "アクセシビリティの許可がありません")
            continue
        if E.get("FAKE_AX_FAIL"):
            fail(rid, E["FAKE_AX_FAIL"], "偽の bridge が %s で止めました" % E["FAKE_AX_FAIL"])
            continue
        front = {"front_before": 1, "front_after": APP_PID if E.get("FAKE_FRONT_CHANGE") == "1" else 1}
        if cmd in ("ax_press", "ax_set_value", "ax_replace_text"):
            ref = req.get("element") or ""
            el = last_refs.get(ref)
            if el is None:
                fail(rid, "stale_element", "要素 %s は古い読みの物です（ax_tree で読み直してください）" % ref)
                continue
            if cmd == "ax_press":
                action = req.get("action") or "AXPress"
                if action not in el["actions"]:
                    fail(rid, "action_unsupported", "この要素は %s を持っていません" % action)
                    continue
                opened = row_refs.get(ref, opened)
                kind, i = msg_refs.get(ref, (None, None))
                if kind == "row" and action == "AXShowMenu" and E.get("FAKE_MENU_NEVER") != "1":
                    menu_for, dialog_for = (i + int(E.get("FAKE_MESSAGES_OFF", "0"))) % len(messages), None
                    menu_late = int(E.get("FAKE_MENU_LATE", "0"))
                elif kind == "item" and E.get("FAKE_ITEM_DEAD") != "1" and (E.get("FAKE_ITEM_FOCUS") != "1" or req.get("focus") is True):
                    dialog_for, menu_for = (menu_for if i == "Delete Message" else None), None
                    dialog_late = int(E.get("FAKE_DIALOG_LATE", "0"))
                elif kind == "menu" and action == "AXCancel" and E.get("FAKE_MENU_STICKY") != "1":
                    menu_for = None
                elif kind == "delete":
                    del messages[dialog_for]
                    dialog_for = None
                    hidden_now = hidden_now or E.get("FAKE_HIDE_AFTER_DELETE") == "1"
                elif kind == "cancel":
                    dialog_for = None
                reply(rid, dict(front, performed=action, **({"focus_rc": 0} if req.get("focus") is True else {})))
            else:
                if not el["value_settable"]:
                    fail(rid, "not_settable", "この要素の値は置けません")
                    continue
                old = body
                typed = True
                via = "selected_text" if el["role"] == "AXTextArea" else "value"
                if cmd == "ax_replace_text":
                    # G79: 本物の bridge と同じ: replace の完全一致（重なりも数える）がちょうど 1 か所の時だけそこを value に。他は何も置かない
                    rep = req.get("replace") or ""
                    hits = [i for i in range(len(body)) if rep and body.startswith(rep, i)]
                    if len(hits) != 1:
                        fail(rid, "ax_replace_count", "置き換える字が欄に %d か所あります（ちょうど 1 か所の時だけ置き換えます）。何も置いていません" % len(hits))
                        continue
                    body = body[:hits[0]] + req.get("value", "") + body[hits[0] + len(rep):]
                    u16 = lambda t: len(t.encode("utf-16-le")) // 2
                    reply(rid, dict(front, set=True, read_back=body, matches=True, via=via, length_before=u16(old), length_after=u16(body)))
                    continue
                if E.get("FAKE_SET_LASTS") is not None:
                    n = int(E["FAKE_SET_LASTS"])
                    back = E["FAKE_SET_BACK"] if "FAKE_SET_BACK" in E else old
                    body, set_revert = (req.get("value", ""), (n, back)) if n > 0 else (old, None)
                    reply(rid, dict(front, set=True, read_back=req.get("value", ""), matches=True, via=via))
                    continue
                if E.get("FAKE_SET_MISMATCH") != "1":
                    body = req.get("value", "")
                    if E.get("FAKE_BODY_FILE"):
                        with open(E["FAKE_BODY_FILE"], "w", encoding="utf-8") as f:
                            f.write(body)
                reply(rid, dict(front, set=True, read_back=body, matches=body == req.get("value"), via=via))
                if E.get("FAKE_SET_MISMATCH") == "1":
                    body = old
            continue
        if req.get("pid") != APP_PID:
            fail(rid, "no_such_process", "pid %s の app は動いていません" % req.get("pid"))
            continue
        if cmd == "ax_menu":
            path = req.get("path") or []
            known = [m.split(">") for m in E.get("FAKE_MENU", "ファイル>保存").split("|")]
            if path not in known:
                fail(rid, "menu_not_found", "「%s」が見つかりません（%s）" % (" › ".join(path), "・".join(">".join(k) for k in known)))
                continue
            tracks += E.get("FAKE_MENU_ADDS") == "1"
            if E.get("FAKE_SAVE_TO"):
                import threading
                threading.Timer(int(E.get("FAKE_SAVE_MS", "0")) / 1000, lambda text=body: open(E["FAKE_SAVE_TO"], "w").write(text)).start()
            reply(rid, dict(front, pressed=path, enabled_before=E.get("FAKE_MENU_STALE") != "1"))
        elif cmd == "ax_menu_tree":
            # ENGINE-AUDIT W3: menu bar を読むだけ（FAKE_MENU "ファイル>保存|編集>取り消す" → 題名ごとに 1 階目の項目・押さない）
            menus = {}
            for m in E.get("FAKE_MENU", "ファイル>保存").split("|"):
                parts = m.split(">")
                menus.setdefault(parts[0], [])
                if len(parts) > 1 and parts[1] not in menus[parts[0]]:
                    menus[parts[0]].append(parts[1])
            reply(rid, {"menus": [{"title": t, "items": its} for t, its in menus.items()]})
        else:
            key, text = req.get("key"), req.get("text")
            if (key is None) == (text is None) or key == "" or text == "" or any(m not in ("cmd", "shift", "option", "control") for m in req.get("mods", [])):
                fail(rid, "bad_args", "key か text のどちらか 1 つ（空でない）が要ります")
                continue
            if text is not None:
                body += text
            reply(rid, dict(front, sent=2 if key else 2 * ((len(text) + 15) // 16), via="pid"))
    elif cmd == "apps":
        bundle, name = E.get("FAKE_APP_BUNDLE", "com.cockos.reaper"), E.get("FAKE_APP_NAME", "REAPER")
        more = [int(E["FAKE_APP_MORE_PID"])] if E.get("FAKE_APP_MORE_PID") else []
        procs = [{"pid": p, "bundle_id": bundle, "name": name, "hidden": (E.get("FAKE_HIDDEN") == "1" and not unhidden) or hidden_now} for p in [APP_PID] + more]
        if E.get("FAKE_OTHER_PID"):
            procs.insert(0, {"pid": int(E["FAKE_OTHER_PID"]), "bundle_id": bundle, "name": name})
        reply(rid, {"apps": procs, "frontmost": 1})
    elif cmd == "windows" and busy_windows < int(E.get("FAKE_BUSY_WINDOWS", "0")):
        busy_windows += 1
        fail(rid, "ax_cannot_complete", "窓の一覧を読めません: app が応答しません（忙しい・dialog が開いている）")
    elif cmd == "windows":
        title = titles[min(windows_calls, len(titles) - 1)]
        windows_calls += 1
        ws = []
        if E.get("FAKE_WINDOWS", "1") != "0" and str(req.get("pid")) != E.get("FAKE_OTHER_PID", ""):
            dialog = E.get("FAKE_DIALOG_CG")
            if dialog:
                ws.append({"handle": "w9", "pid": APP_PID, "title": "About " + title, "role": "AXWindow", "subrole": "AXStandardWindow",
                           "minimized": False, "main": True, "focused": False, "fullscreen": False, "position_settable": False, "size_settable": False,
                           "frame": {"x": 300.0, "y": 200.0, "w": 513.0, "h": 405.0}, "cg_match": "frame", "cg_id": int(dialog), "on_screen": True})
            ws.append({"handle": "w1", "pid": APP_PID, "title": title, "role": "AXWindow", "subrole": "AXStandardWindow",
                       "minimized": source_minimized(), "main": not dialog, "focused": True, "fullscreen": source_fullscreen(),
                       "position_settable": True, "size_settable": True, "frame": dict(frame), "cg_match": "frame",
                       "on_screen": not docked_off_screen()})
            if E.get("FAKE_NO_CG") == "1" or windows_calls <= int(E.get("FAKE_NO_CG_ROUNDS", "0")):
                ws[-1]["cg_match"] = "none"
            else:
                ws[-1]["cg_id"] = 500
        reply(rid, {"pid": req.get("pid"), "windows": ws, "cg_windows": []})
    elif cmd == "observe":
        reply(rid, {"observing": req.get("pid"), "windows": 1, "failed": []})
    elif cmd == "unobserve":
        reply(rid, {"observing": False})
    elif cmd == "set_frame":
        if req.get("handle") != "w1":
            fail(rid, "unknown_handle", "その窓の handle は知りません")
            continue
        frame = {"x": float(req["x"]), "y": float(req["y"]),
                 "w": min(max(float(req["w"]), float(E.get("FAKE_MIN_W", "0"))), float(E.get("FAKE_MAX_W", "inf"))),
                 "h": min(max(float(req["h"]), float(E.get("FAKE_MIN_H", "0"))), float(E.get("FAKE_MAX_H", "inf")))}
        set_calls += 1
        if set_calls <= int(E.get("FAKE_PARTIAL_SETS", "0")):
            frame["w"], frame["h"] = frame["w"] * 0.85, frame["h"] - 4
        actual = dict(frame)
        if set_calls <= int(E.get("FAKE_LAGGED_SETS", "0")):
            actual["w"], actual["h"] = frame["w"] * 0.85, frame["h"] - 4
        reply(rid, {"handle": "w1", "requested": {k: req[k] for k in "xywh"}, "actual": actual, "t": 0})
    elif cmd == "frame":
        if req.get("handle") != "w1":
            fail(rid, "unknown_handle", "その窓の handle は知りません")
            continue
        if flag_file("FAKE_FRAME_SLOW_FILE"):
            time.sleep(3)
        reply(rid, {"handle": "w1", "pid": APP_PID, "title": titles[0], "role": "AXWindow", "subrole": "AXStandardWindow",
                    "minimized": source_minimized(), "main": True, "focused": True, "fullscreen": source_fullscreen(),
                    "position_settable": True, "size_settable": True, "frame": dict(frame), "cg_id": 500, "cg_match": "frame",
                    "on_screen": not docked_off_screen()})
    elif cmd == "minimize":
        reply(rid, {"handle": req.get("handle"), "minimized": bool(req.get("minimized")), "t": 0})
    elif cmd == "raise":
        after = APP_PID if (req.get("activate") or E.get("FAKE_STEAL") == "1") else 1
        reply(rid, {"handle": req.get("handle"), "front_before": 1, "front_after": after, "activate_requested": bool(req.get("activate")), "t": 0})
    elif cmd == "cg_windows":
        host = int(E.get("FAKE_HOST_PID", "1"))
        above = E.get("FAKE_HOST_ABOVE") == "1"
        ws = [
            {"id": 500, "pid": APP_PID, "layer": 0, "order": 2 if above else 0, "on_screen": not docked_off_screen(), "frame": dict(frame)},
            {"id": 600, "pid": host, "layer": 0, "order": 0 if above else 1, "on_screen": True, "frame": {"x": 0, "y": 0, "w": 1200, "h": 800}},
        ]
        if E.get("FAKE_HIDE_DOCKED") == "1" or (E.get("FAKE_GONE_FILE") and os.path.exists(E["FAKE_GONE_FILE"])):
            ws = [w for w in ws if w["id"] != 500]
        if E.get("FAKE_BETWEEN") == "1":
            ws.append({"id": 700, "pid": 333, "layer": 0, "order": 1 if above else 3, "on_screen": True, "frame": dict(frame)})
        reply(rid, {"windows": ws, "frontmost": 1})
    elif cmd == "mouse":
        reply(rid, {"buttons": int(E.get("FAKE_BUTTONS", "0"))})
    elif cmd == "frontmost":
        front = int(E.get("FAKE_FRONT_PID", "1"))
        reply(rid, {"pid": front, "bundle_id": "app.kangae.desktop" if front == 1 else E.get("FAKE_APP_BUNDLE", "com.cockos.reaper")})
    elif cmd == "find_app":
        bid = req.get("bundle_id", "")
        if E.get("FAKE_INSTALLED", "1") == "0":
            reply(rid, {"installed": False, "bundle_id": bid})
        else:
            reply(rid, {"installed": True, "bundle_id": bid, "path": "/Applications/Fake.app", "version": "1.19", "build": "419"})
    elif cmd == "open_document":
        if not os.path.isfile(req.get("path") or ""):
            fail(rid, "bad_args", "bundle_id と在る file の path が要ります")
            continue
        opened_doc = req["path"]
        if E.get("FAKE_OPEN_STATE"):
            open(E["FAKE_OPEN_STATE"], "w", encoding="utf-8").write(opened_doc)
        with open(E.get("FAKE_LOG", os.devnull), "a") as log:
            log.write("open_document %s %s\n" % (req.get("bundle_id"), opened_doc))
        reply(rid, {"pid": APP_PID, "bundle_id": req.get("bundle_id")})
    elif cmd == "app_unhide":
        if not isinstance(req.get("pid"), int):
            fail(rid, "bad_args", "pid が要ります")
            continue
        unhidden = unhidden or E.get("FAKE_UNHIDE_STUCK") != "1"
        reply(rid, {"hidden": (E.get("FAKE_HIDDEN") == "1" and not unhidden) or hidden_now, "front_before": 1, "front_after": 1})
    elif cmd == "open_app":
        reply(rid, {"pid": APP_PID, "bundle_id": req.get("bundle_id")})
    elif cmd == "quit":
        reply(rid, {"bye": True})
        sys.exit(0)
    else:
        fail(rid, "unknown_cmd", "知らない cmd: %s" % cmd)
