# 完成の筋書き（1 回の録画の台本）

G-final の録画 5 本は、この 1 枚を毎回そのまま通す（`README.md`）。録画 n の前に試験の repo を clone し直し、data dir を初期化する。

手の数: 27（表の行数と一致しなければ `rehearse.sh --dry-run` が赤）

書き方の約束（`scripts/qa/bg/rehearse.py` が表を読む。`--dry-run` が検査する）:
- 「誰」= `BG`（試験の道具が背景で通す）か `FG`（録画の中で人が前面で触る。リハーサルでは通さず「FG」と数える）。
- 「手」= rehearse.py の手の名前。表に無い名前は赤。
- 「画面に見える筈の物」の `…` は画面の文言で、`docs/internal/ui-pro/strings.csv` の原文（`{}` を含む型は穴を埋めた形）と一致しなければ赤。まだ CSV に無い文言は `⟦未収録: 理由⟧` と書く（書き写して嘘を作らない）。
- 『…』は人が打つ文・押す物の名前（画面の文言の主張ではない）。`<nonce>` は録画 n ごとに変わる。
- Page の tab の名前は Project ごとに違う（偽の CLI = Docs / Dev / Diagram・本物の AI = README / Dev terminal / Architecture 等）。表の『Docs』『Dev』『Diagram』は**面の種類**（TextEdit の README の面・Terminal・Excalidraw）で、rehearse が押す直前の DB から tab の名前を引く。

| # | 誰 | 手 | 人がする事 | 画面に見える筈の物 | 読み戻しで確かめる物 | 秒 |
|---|---|---|---|---|---|---|
| 1 | BG | generate | Home の欄に主デモの一言『Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw.』を打ち、欄の右端の `↵ Send`（AX の名前は `Send`）を押す → 会話に出る案の card の先頭の `Build now` を押す（AX の名前も同じ語・最初から出る・TALK-1101: `Preview plan` は中身を card に開くだけで押さない・L2: Home の一言は作らずに会話になり、作るのは `Build now` だけ） | `What do you want to create?`・`↵ Send`・⟦未収録: 案の card の Proposal・Build now・Preview plan は src/domain/mod.rs の chat の const で、ui-strings.py の対象の外⟧・⟦未収録: TALK-1101 から Build now が作る時の AI の読みを 1 回する（答えていない問いは AI が選び、作った Project の Chat に `I picked: …`）。Preview plan を押した時だけ card に `The AI is reading this plan…` → 何が出来るかの 1〜2 文と `Nothing is made until you press Build now.`・節（On screen・How you use it・On your Mac・Not decided yet）。どれも src/domain/mod.rs の chat の const か AI の文で、ui-strings.py の対象の外⟧・`Checking for AI on this Mac`・`Dev environment`・`Change this project` | L2: `Accept` を押す前は送信より後の Project が 0・I1: DB の `project_revisions` revision 1 の origin が generate・作られた時刻が送信より後・Page が 2 枚以上 | 50 |
| 2 | BG | add_terminal | Dev の Page で右下の ＋（`Add to page`・MIRROR-FRAME-1101 から。帯の tab の ＋ は `Add page · ⌘T`）を開き `Add terminal` を押す | `Add to page`・`Add terminal`・`… Starting the shell` | AX の `slot:main` の下に `terminal:*` が出る | 20 |
| 3 | BG | term_type | 下の欄が隠れていれば帯の `•••`（`Terminal options`）→ `Input field (for IME)` で出し（L7: 既定は隠れる・押すたびに出る / 隠れるので出ていれば押さない）、Terminal に『echo <nonce>』『echo $((<a>*<b>))』『false; echo "<nonce>:$?"』『git status』『echo <nonce> > undo-probe.txt』を打つ | `Type in the terminal` | C1: 出力に nonce と積と `<nonce>:1`・`ps -o ppid=,tty=,comm=` の親が Shibubu・tty あり・comm が zsh か bash | 40 |
| 4 | BG | term_type | Terminal に『sleep 15; echo done』を打つ | `Type in the terminal` | C2 の始め: PID を記録 | 5 |
| 5 | BG | goto_tab | 『Docs』のタブを押し、`Mirror here` を押して TextEdit の Mirror に同意する（同意は bundle id ごとなので手 22 の notes.md にも効く） | `README.md in TextEdit` | AX の Page タブの選択が Docs | 5 |
| 6 | FG | fg_textedit | 枠の中の TextEdit の README の窓に、人の手で 1 行（nonce を含む）を打つ（まだ保存しない） | ⟦未収録: AX の label は `Mirroring · TextEdit`（画面に出る文ではない）⟧ | A2: TextEdit の AX value に nonce・disk の README には無い | 20 |
| 7 | BG | goto_tab | 『Dev』のタブへ移り、『Docs』へ戻る | `README.md in TextEdit` | A2: 戻った後も同じ文字と同じ窓 id・C2: Dev の Terminal に `done`（同じ PID） | 15 |
| 8 | FG | fg_textedit | もう 1 文字打ち、menu の File › Save（⌘S）で保存する | ⟦未収録: AX の label は `Mirroring · TextEdit`（画面に出る文ではない）⟧ | A1→E1: 打つ前に relay の記録 1 件・AX value が変わる・README.md の sha が変わり中身と `git diff` に nonce | 20 |
| 9 | BG | goto_tab | 『Diagram』のタブを押す | `Not checked by Shibubu` | B2: Shibubu の窓の `AXWebArea` の element を記録（Excalidraw が Live） | 20 |
| 10 | BG | b1 | Excalidraw に nonce の文字を置き、書き出して `Save to repo` を押す | `Save to repo` | B1: `docs/arch.excalidraw` の sha と要素数 ≥ 1・nonce の文字・`git status`・B2: 戻った後も同じ element | 60 |
| 11 | BG | b4 | 帯の ＋ の `Add browser` で試験の repo の Issue を開き、nonce の comment を書く | ⟦未収録: Add browser の URL 欄と comment の欄は web の面の中の文言で、CSV の対象の外⟧ | B4: `oracle/github.sh` の before に無く after に nonce の comment（WebView とも製品とも別の道） | 40 |
| 12 | BG | nl_edit | 欄に『add a Tests page』と打って送る | `Change this project, e.g. add a Tests page`・`No runs yet` | I2: revision の origin が nl_edit・ops は AddPage 1 つ・Page タブが 1 枚増える・他の Page の hash が同じ | 20 |
| 13 | BG | nl_edit | 欄に『move Excalidraw right』と打って送る | `Change this project` | I2: ops は MoveSurface 1 つ・Excalidraw の面が右の Page へ | 15 |
| 14 | BG | nl_edit | 欄に『move TextEdit right』と打って送る | `README.md in TextEdit` | D1: ops は MoveSurface 1 つ・枠の x が変わる・TextEdit の窓の pid・窓 id・bounds が前後で同じ（Mirror は映すだけで本物の窓を動かさない・`oracle/windows.swift`） | 20 |
| 15 | BG | undo | 帯の ↶（`Undo · ⌘Z`）を押す | `Undo · ⌘Z` | A3: Project の hash が編集の前に戻る・`undo-probe.txt` と `docs/arch.excalidraw` の sha・README の sha・TextEdit の AX value は Undo の前後で同じ | 10 |
| 16 | BG | redo | 帯の ↷（`Redo · ⇧⌘Z`）を押す | `Redo · ⇧⌘Z` | A3: Project の hash が編集の後に戻る・外の効果は同じ | 10 |
| 17 | FG | fg_lease | Docs の TextEdit を前に出して 1 文字打つ（人の lease） | ⟦未収録: TextEdit の窓の中は元の app の画面⟧ | F2 の前提: `lsappinfo front` が TextEdit・TextEdit の AX value が act の記録に無い変化 | 10 |
| 18 | BG | ask_ai | 17 の 5 秒以内に、欄へ『Change the README title to “Shibubu Demo” and save it』を送る | ⟦未収録: mac_gui.rs:513⟧ | F2: README の sha が変わらない・act の記録が 0 件・拒否の 1 行が出る | 10 |
| 19 | FG | fg_front | Shibubu の窓の Page タブ（枠の外）を click して前面を Shibubu に戻す | `README.md in TextEdit` | `lsappinfo front` が Shibubu・TextEdit が後ろ（試験の側が記録） | 5 |
| 20 | BG | ask_ai | 最後の人の変化から 5 秒より後に、同じ一言を送る | `Verified`・`Approve and send`・⟦未収録: 承認の 1 行は engine_view.rs:232 の型（`will send 2 steps` を含む）で、{goal} と {} の穴が長い⟧ | F1: act(SetValue) → AX value が新しい題（disk はまだ古い）→ act(保存) → README の sha が変わる。AI が fs で直接書いたら赤 | 40 |
| 21 | BG | nl_edit | 欄に『Add a page that mirrors notes.md』と打って送る | `Change this project`・`Mirror shows TextEdit's window here and sends your typing to it. macOS will ask for screen recording.` | I2 と同じ検査: ops は AddPage 1 つと Mirror の面 1 つ・Mirror の Page は Docs の README とは別の TextEdit の窓（notes.md） | 25 |
| 22 | BG | mirror_consent | 同意済み（手 5）で 2 つ目の Mirror（notes.md）が映る。`依存: G32 ＋ sendback 69`（入るまで赤のまま測る） | ⟦未収録: AX の label は `Mirroring · TextEdit`（画面に出る文ではない）⟧ | 同意の記録は 1 件のまま・notes.md の Mirror の矩形が写り、手 5 の README の Mirror も止まらない | 10 |
| 23 | BG | e0 | 試験の道具が後ろの TextEdit の notes.md に nonce1 → nonce2 を AX で入れる | ⟦未収録: AX の label は `Mirroring · TextEdit`（画面に出る文ではない）⟧ | E0: Shibubu の窓の `screencapture -x -l <wid>` の Mirror の矩形を OCR・nonce1 が写り、替えて 2 秒以内に nonce2 が写る | 25 |
| 24 | FG | fg_mirror_type | Mirror の矩形の上で打ち、⌘S で保存する | ⟦未収録: AX の label は `Mirroring · TextEdit`（画面に出る文ではない）⟧ | E1: 変化の前に relay の記録 1 件・notes.md の AX value と sha が変わる・試験の道具が直接 postToPid した変化は赤 | 25 |
| 25 | BG | restart | Shibubu を終了し、同じ data dir で起動し直す | `Dev environment` | B3: 図が残り、もう 1 文字置いて書き出すと sha が変わり 2 つの nonce を含む・D1: 窓 id が同じ・Page の順が戻る | 40 |
| 26 | BG | d2 | 外から TextEdit を終了する | `Not connected`・`The app's window closed` | D2: `oracle/windows.swift` で窓が無い・AX の木に `Not available`（AXImage の label・B23 の前の .app は `Not running`）が在り `Connected` が無い | 15 |
| 27 | BG | recount | oracle を取り直して数え直す | — | 数え直し: Page 移動・編集・再起動の後に `native_verified`・`web_verified`・`service_verified`・`terminal_real` が減らない・`lies` 0・`dead_clicks` 0 | 20 |
