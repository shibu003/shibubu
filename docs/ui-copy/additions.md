## 11 English copy — additions (UI-pro-copy-2)

Appended to FINAL/copy.en.md: the strings that exist in source, are shown to the user, and have no row there (kinds error, status, button, label, tooltip, placeholder, menu). One row per JP string, same shape as FINAL: `| JP | EN |`, `{x}` keeps the name used in source. Style follows the DIGEST style sheet. Run the check with both tables:

`python3 -I scripts/qa/ui-strings.py --map FINAL/copy.en.md --map copy.en.additions.md --kinds error,status,button,label,tooltip,placeholder,menu --min 1`

A later `--map` replaces an earlier row with the same JP, so "Corrections" below fixes FINAL rows without editing FINAL. Glyph-only controls get the glyph plus the accessible name in a note. A `{reason}` is filled only from FINAL's "Errors that fill {reason}" table.

### Legacy canvas (src/app.rs)

| JP | EN |
|---|---|
| 操作ボタンには実際の実行先が必要です。変更系操作は確認後に実行します。 | Action buttons need a real target. Actions that change things run after you confirm. |
| 失敗 {title}: {e} | {title} failed: {e} |
| {}件のウィンドウ | {} windows |
| ライセンス・サブフォルダ・コミットを確認してください | Check the licence, subfolder and commit |
| 検査できません: {e} | Couldn't check: {e} |
| {}の{}を部分取得しました。コードは実行していません | Imported {} · {}. No code was run. |
| 部分取得できません: {e} | Couldn't import: {e} |
| 操作UIとWorkflow草案を生成しました。接続を確認してください。 | Built controls and a workflow · Check the connection |
| 実行できる操作がありません | No actions to run |
| 書き込み系操作は『操作モード』へ切り替えてから実行してください | Switch to control mode before running actions that write |
| 2つのWorkflowが必要です | Two workflows are needed |
| 書き込みを含むWorkflowは操作モードで実行してください | Run workflows that write in control mode |
| 同じウィンドウを2レーンで使えません | Two runs can't use the same window |
| 並列準備不可: {e} | Can't run in parallel: {e} |
| 先にウィンドウを検出してください | Find the windows first |
| 先に要素を取得してください | Read the controls first |
| 互換 view（旧 canvas）· 操作UI と Workflow を 1 つの作業面に | Classic canvas · controls and workflows in one place |
| GitHub | GitHub |
| 接続先として設定 | Set as source |
| ⌘E | ⌘E |
| ＋ UIパーツ | Add part |
| {} · {} | {} · {} |
| 未接続。実行できるステップを追加してください。 | Not connected. Add a step that can run. |
| 選択したUIの構造と操作先 | Structure and target of the selected control |
| × | × (accessible name: Close) |
| 配置/サイズ (px) | Position and size (px) |
| X | X |
| Y | Y |
| W | W |
| H | H |
| 操作UIツリー (DOM相当) | Control tree |
| eguiにはDOMがないため、役割・ソース・操作Bindingを型付きノードで管理します。 | Each control keeps its role, source and action as a typed node. |
| source: {} | Source: {} |
| action: {action} | Action: {action} |
| 保存したUI・Workflow・履歴を削除します。外部サービスのデータは削除しません。 | Deletes the saved controls, workflows and history. Data in other services is kept. |
| UIパーツのみ削除します。元アプリのデータやWorkflowは変更しません。 | Deletes this part only. The app's data and workflows stay as they are. |
| {repo}: {title} | {repo}: {title} |
| {}/args: {:?} | Command: {} {} |
| 公開リポジトリ全体ではなく、指定サブフォルダだけをコミット固定で取得します。 | Imports only the subfolder you name, pinned to one commit, not the whole public repo. |
| Repo | Repo |
| owner/repository | owner/repository |
| packages/specific-feature | packages/feature |
| 既存の信頼済みCLIを人間が明示指定。GitHubから取得したコードは自動実行・ビルドしません。 | You choose a command you already trust. Code imported from GitHub is never run or built automatically. |
| {}/{} · {} | {} · {} · {} |
| 実在する絶対パスの実行ファイルが必要です | Enter the full path of an existing program |
| CLI接続を登録。実行時は毎回確認します | Command saved. Shibubu asks before every run |
| 応答形式が一致しません | The reply isn't in the expected format |
| 表示する項目はありません | Nothing to show |
| #{number} | #{number} |
| このアプリを削除 | Delete canvas… |
| Apps へ | Go to Home |
| 画面の読みの形が違うので、Shibubu を開き直してください（{e}） | Shibubu can't read this screen's format. Reopen Shibubu ({e}) |
| 画面を読めなかったので、もう一度押してください | Couldn't read the screen. Press again |
| 読める窓がありません（CG の窓番号と AX の窓が 1 対 1 に決まる窓だけを出します）。窓を開いてからもう一度 ① を押してください | No window Shibubu can identify · Open a window, then press Find windows again |
| {} の窓に押せる要素がありません{} | No controls to press in the window of {}{} |
| （途中まで読みました。もう一度②を押してください） | · Only part of it was read · Press Read controls again |
| タイトル未取得 | Untitled |
| {}（題名なし） | {} · Untitled |
| 操作可能な要素: {}件 | {} controls found |
| 画面の操作ではありません | Not a screen action |

### Project builder (src/composer/project.rs)

| JP | EN |
|---|---|
| AI の答えの Page の数が {} です（1〜8） | The AI returned {} pages. A project has 1 to 8 |
| AI の答えの {} 番目の Page の欄が足りないか合いません | Page {} in the AI's answer is missing a field or has the wrong shape |
| {name} の Page | {name} page |
| 「{name}」の面はどの Page にも置かれていません | “{name}” isn't on any page |
| 「{name}」は右端の「{}」の Page に在り、右に Page がありません。先に「Tests Page を足して」のように Page を足してください | “{name}” is on “{}”, the last page, and there's no page to its right. Add a page first, for example “add a Tests page” |
| 「{name}」は左端の「{}」の Page に在り、左に Page がありません | “{name}” is on “{}”, the first page, so it can't move left |
| 「{page}」の Page はこの Project にありません。在る Page: {} | There's no “{page}” page in this project. Pages: {} |
| test は Tests の Page の「走らせる」で走らせてください | Run tests from the Tests page |
| (code) 既に在る Page の名前（copy.en.md の `There's already {what}.` の {what}） | a page named “{name}” |
| README.md（TextEdit） | README.md in TextEdit |
| 構成図（Excalidraw） | Diagram in Excalidraw |
| {shown} を Mirror で見る Page | Page that mirrors {shown} |
| Mirror で見る file がありません。notes.md のように書いてください | Name a file to mirror, for example notes.md |
| Mirror の file は Project の場所からの相対にしてください | Write the mirrored file's path relative to the project folder |
| Mirror の file に .. は使えません | A mirrored file's path can't contain .. |

### Generated project (src/generated.rs)

| JP | EN |
|---|---|
| Terminal の記録を読めません（{e}）。新しく起こしてください | Couldn't read the terminal history ({e}). Start a new terminal |
| 生成した app を保存できません（{e}）。閉じると消えます | Couldn't save the project ({e}). It will be lost when you close it |
| 保存した app のうち {broken} 個を開けませんでした（定義が今の検査を通りません） | {broken} saved projects couldn't be opened because their definitions no longer pass the check |
| 「{}」を消します。この Page で打ちかけた文字も消えます | Delete “{Page}”? ⌘Z brings it back. |
| 「{command}」を走らせています。結果が出てから、もう一度押してください | “{command}” is running. Press again when it finishes |
| 走らせる準備ができませんでした（{e}）。もう一度押してください | Couldn't get ready to run ({e}). Press again |
| 送りましたが、元の app で確かめる仕方がありませんでした（済みとは出しません）・元の app で中身を見てください | Sent, but there is no way to check it in the original app, so it isn't marked done. Look inside the app |
| 済みました（元の app の実状態で {verified} 件を確かめました） | Done. Checked {verified} items in the original app（{verified} = 1 は “1 item”・operator 10-29） |
| 頼んだ事: {goal} | You asked: {goal} |
| 止まりました: {l} | Stopped: {l} |
| 今は送れません（前の依頼をまだ進めています）・結果の 1 行が出てから、もう一度承認してください | Can't send now: the last request is still running. Approve again when its result appears |
| この Project は別の所で更新されました（今は revision {}）。開き直してから当て直してください | This project was changed somewhere else. Reopen it, then apply your change again |
| 面の確かめを読めません（{e}）。この版の Shibubu では読めない形です | Couldn't read the check ({e}). This version of Shibubu can't read that format |
| 先頭の Page へ | Go to first page |
| ⌘Z | ⌘Z |
| ⇧⌘Z | ⇧⌘Z |
| ▤ Terminal | Show terminal |
| ⌃` | ⌃` |
| 「{said}」は読めません。「<名前> Page を足して」か「<file> を Mirror で見る Page を足して」か「<面の名前> を右へ」と打ってください | Couldn't understand “{said}”. Try “Add a <name> page”, “Add a page that mirrors <file>” or “Move <name> right” |
| {why}。app の名前は確かめずに作りました | {why}. The project was built without checking the app's name |
| {name} の主役の部品が描かれません。作り直してください | The main part of {name} isn't drawn. Build it again |
| {name} の入力欄を押しても focus を取りません。作り直してください | The input in {name} doesn't take focus when pressed. Build it again |
| {} を確かめる画面を開けません（{}）。開き直してください | Couldn't open the screen that checks {} ({}). Reopen it |
| 「{what}」の操作が {} にありません。作り直してください | {} has no action for “{what}”. Build it again |
| 「{what}」が通りませんでした（{}）。作り直してください | “{what}” didn't go through ({}). Build it again |
| 「{what}」が断られました（{reason}）。作り直してください | “{what}” was refused ({reason}). Build it again |
| 「{what}」は承認を待つ手でした（評価では承認しません） | “{what}” needs approval, and the check never approves |
| 「{what}」を押せません（{}）。作り直してください | Can't press “{what}” ({}). Build it again |
| DB を読めません（{e}） | Couldn't read the database ({e}) |
| この Terminal は Project にありません。作り直してください | This terminal isn't in the project. Build it again |
| Terminal を起こせません（{e}）。作業 folder を確かめてから開き直してください | Couldn't start the terminal ({e}). Check the working folder, then reopen |
| Terminal の prompt が {} 秒で返りません。開き直すと確かめ直します | The terminal didn't show a prompt in {} s. Reopen to check again |
| Terminal が起きていません。開き直してください | The terminal isn't running. Reopen it |
| 1 行打つ先の folder を作れません（{}） | Couldn't create the folder for the command ({}) |
| 打った 1 行が終了 code {} で終わりました。開き直すと確かめ直します | The command ended with exit code {}. Reopen to check again |
| 打った 1 行の結果が {} 秒で返りません。開き直すと確かめ直します | The command gave no result in {} s. Reopen to check again |
| file を読めません（{}） | Couldn't read the file ({}) |
| {name} を Page から使うには、システム設定 › プライバシーとセキュリティ › アクセシビリティで Shibubu を許可してください | To use {name} from a page, allow Shibubu in System Settings › Privacy & Security › Accessibility |
| {name} がこの Mac に入っていません。入れてから一言を送り直してください | {name} isn't installed on this Mac. Install it, then send your request again |
| {name} はこの面の確かめ方では測れません | This check can't measure {name} |
| {} の中身を変える手は、評価では呼びません | The check never calls actions that change {} |
| {name} の窓の映像を待っています | Waiting for the {name} window |
| {name} の窓を映すには、この Page の同意か、システム設定 › プライバシーとセキュリティ › 画面収録の許可が要ります | To mirror the {name} window, allow it on this page or in System Settings › Privacy & Security › Screen Recording |
| {name} の窓を映す部品が答えません。開き直してください | The part that mirrors the {name} window isn't responding. Reopen it |
| {kind} を読めませんでした。「↻ 読み直す」で読み直せます | Couldn't read {kind}. Use “Refresh” to try again |
| 1 個の操作はまだ使えません（Apps の行・1 つの時） | 1 action not set up |

### Shibubu command line (src/project/context.rs)

| JP | EN |
|---|---|
| 知らない command: {other} | Unknown command: {other} |
| command がありません | No command given |
| 知らない引数: {other} | Unknown argument: {other} |
| Project の Terminal の中では Shibubu の画面を開きません（画面は今の Shibubu の窓を使ってください） | Shibubu doesn't open a second window from a project's terminal. Use the Shibubu window that is open |
| 知らない引数 {a} では Shibubu の画面を開きません（画面は引数なしの shibubu か Shibubu.app で開きます） | Shibubu doesn't open with the argument {a}. Open it with shibubu alone or with Shibubu.app |
| 保存先が絶対 path ではありません: {} | The save location isn't a full path: {} |

### Apps on this Mac (src/project/mod.rs, native.rs, router.rs, web.rs)

| JP | EN |
|---|---|
| この Mac の app の一覧を読めません（{}） | Couldn't read the list of apps on this Mac ({}) |
| この Mac の app の一覧が {} 秒で読めませんでした | The list of apps on this Mac didn't load in {} s |
| {app} の文書を読めません（{}） | Couldn't read the {app} document ({}) |
| 理由が返りません | No reason given |
| フォルダです | It's a folder |
| 読む許可がありません | No permission to read it |
| 開けません | Can't open it |
| {file_name} を開いた窓がありません | No window has {file_name} open |
| {file_name} の窓が {} 個あり、どれを重ねるか決められません | {file_name} is open in {} windows, so Shibubu can't tell which to cover |
| 「{}」は全画面です | “{}” is full screen |
| 「{}」の位置と大きさを外から変えられません | Can't move or resize “{}” from outside |
| 重ねた記録を書けません（{}: {e}） | Couldn't save the record of the covered window ({}: {e}) |
| 窓番号が無いので Shibubu の窓を上に置けない（元の窓は見えたまま） | The window has no number, so Shibubu can't be placed above it. The original stays visible |
| {:.0} Shibubu の窓を元の窓の上に置けない（{e}） | {:.0} Couldn't place Shibubu above the original window ({e}) |
| 窓が在るかを確かめられない: {e} | Couldn't check whether the window exists: {e} |
| Page の名前が空 | The page name is empty |
| {} の WebView はまだ使える状態ではありません | The web view for {} isn't ready yet |
| 書き出した file を repo の docs/ に置く名前 | Name for the exported file in the repo's docs folder |
| repo に保存 | Save to repo |

### Mirror and windows (src/surface/)

| JP | EN |
|---|---|
| {e:?} | {reason} |
| RGBA が {} byte・{w}×{hh}×4 と違う | The image has {} bytes, not {w}×{hh}×4 |
| 「{}」は位置と大きさを外から変えられない窓です。外で開くで使ってください | “{}” can't be moved or resized from outside. Use Open in {App} instead |
| 窓が最小化されています。Dock から戻してから、もう一度選んでください | The window is minimised. Restore it from the Dock, then choose again |
| 重ねられる窓がありません。app の窓を 1 つ開いてから、もう一度選んでください | No window to cover. Open a window of the app, then choose again |
| 窓が {} 個あり、どれを重ねるか決められません。使う窓を前に出してから、もう一度選んでください | {} windows are open and Shibubu can't tell which to cover. Bring the one you want to the front, then choose again |
| 「{}」は全画面です。全画面を解いてから、もう一度選んでください | “{}” is full screen. Leave full screen, then choose again |
| 「{}」の位置と大きさを読めません | Can't read the position and size of “{}” |
| 人が操作中なので、Agent の{what}はしません（任せる を押すと渡します） | You are working, so the agent won't {what}. Press Hand over to let it |
| drag の途中なので切り替えません。mouse を離してから、もう一度押してください | Not switching while you drag. Release the mouse, then press again |
| 理由が書かれていない失敗 | Failed with no reason given |
| KANGAE_SURFACE_BRIDGE の {} が在りません | The file {} named by KANGAE_SURFACE_BRIDGE doesn't exist |
| （{s}） | ({s}) |
| {:.0} degraded: {reason} | {:.0} Limited: {reason} |
| {} の時の attach は何もしない | Attaching does nothing while {} |
| {name} の窓を読めません: {e} | Couldn't read the {name} window: {e} |
| {} の時の attach_to は何もしない | Attaching to a window does nothing while {} |
| 観測できない通知: {} | Notification that can't be observed: {} |
| 観測を始められません（動きの検出なし）: {e} | Couldn't start watching (no change detection): {e} |
| bridge の壊れた行: {s} | Unreadable line from the bridge: {s} |
| {:.0} focus を奪った: 前 pid {before} → 後 pid {after} | {:.0} Focus was taken: process {before} → {after} |
| {:.0} frontmost が別の app（pid {after}）に変わった（Kangae の操作ではない） | {:.0} Another app (process {after}) came to the front, not caused by Shibubu |
| 前に戻せません: {e} | Couldn't move it back: {e} |
| 前後を読めません: {e} | Couldn't read the window order: {e} |
| 元の位置に戻せません: {e} | Couldn't restore the original position: {e} |
| {name} を前に出せません: {e} | Couldn't bring {name} to the front: {e} |
| {:.0} 再接続: 「{}」（{}） | {:.0} Reconnected to “{}” ({}) |
| 再接続の確認に失敗: {e} | Couldn't confirm the reconnection: {e} |
| 前後を確かめられない（AX の窓と CG の窓番号の対応が付かない） | Can't check the window order because the windows can't be matched up |
| {} の時の前後の確認は数えない | The window order check is skipped while {} |
| {:.0} docked の窓が今の画面に無い（別の Space・最小化） | {:.0} The docked window isn't on this screen (another Space or minimised) |
| Kangae の窓が前後の一覧に無い | The Shibubu window is missing from the window order |
| {:.0} 前後が崩れた: docked の順 {d} > Kangae の順 {h} | {:.0} Window order is wrong: docked window at {d}, Shibubu at {h} |
| {:.0} Kangae の窓を真下に置けない（{e}）→ AXRaise の道 | {:.0} Couldn't place Shibubu directly below ({e}). Raising the window instead |
| docked の窓の CG 番号が無いので真下に置けない → AXRaise の道 | The docked window has no number, so Shibubu can't go directly below. Raising the window instead |
| {} の時に元の窓が消えた | The original window disappeared while {} |
| 閉じる時に元の位置に戻せません: {e} | Couldn't restore the original position on close: {e} |
| REAPER の中で失敗しました: {} | Failed inside REAPER: {} |
| REAPER の応答を読めません: {} | Couldn't read REAPER's reply: {} |
| Kangae の窓が見つかりません | The Shibubu window wasn't found |
| main menu がありません | The main menu wasn't found |
| WebView がありません | The web view wasn't found |
| WebView の親の view がありません | The web view has no parent view |
| {name} に accessibilityChildren がありません | {name} has no accessible children |
| AX の木の WebView は別の物なので触らない | The accessibility tree points to a different web view, so it is left alone |
| WebView の窓か親の view がありません | The web view has no window or parent view |
| Kangae の窓の前後を変えられるのは macOS だけです | Only macOS can reorder the Shibubu window |
| まだ頁が読み込まれていないので script を走らせません | The page hasn't loaded yet, so the script wasn't run |
| この面は host を決めていない Browser なので script を走らせません | This browser has no fixed host, so the script wasn't run |
| YouTube の動画の id は英数字と _ - の 11 文字です（{video_id:?}） | A YouTube video ID is 11 letters, digits, _ or - ({video_id}) |
| 新しい窓は本人の browser で開きます | New windows open in your own browser |
| download は止めました（{}） | Download blocked ({}) |
| download に失敗しました（{}）。もう一度書き出してください | Download failed ({}). Export again |
| 開けない URL です: {url} | Can't open this URL: {url} |
| {} を起こせません: {e} | Couldn't start {}: {e} |
| WebView を作れません: {e} | Couldn't create the web view: {e} |
| 頁を開けませんでした: {e} | Couldn't open the page: {e} |
| 読み直せませんでした: {e} | Couldn't reload: {e} |
| 今の頁の URL を読めません: {e} | Couldn't read the URL of this page: {e} |
| script の答えを読めませんでした（undefined か例外） | Couldn't read the script's result (undefined or an error) |
| script を頼めませんでした: {e} | Couldn't run the script: {e} |
| 窓を探す途中で止まりました | Stopped while looking for windows |

### Interface (src/ui/)

| JP | EN |
|---|---|
| ↑ | ↑ (accessible name: Send) |
| v{} → v{} | v{} → v{} |
| （空） | (empty) |
| Home へ | Back to Home |
| 「{goal}」を進めています（終わると結果の 1 行が出ます） | Working on “{goal}” |
| ••• | ••• (accessible name: More) |
| ✦ | ✦ (accessible name: Suggestions) |
| 操作「{id}」が定義にありません | The action “{id}” isn't defined |
| プレビューのため実行しません | Not run in preview |
| この操作には対応していません | This action isn't supported |
| プレビューのため外部へは実行しません | Not sent outside in preview |
| 必要な機能「{cap_ref}」が定義にありません | The required capability “{cap_ref}” isn't defined |
| 表の列が決まっていません | The table has no columns yet |
| {} 列のうち幅に入らない列は、横にスクロールすると表示します | Scroll sideways to see all {} columns（数は 1 だけ単数・`domain::plural`・operator 10-29） |
| (code) 検査が通った時の状態の 1 行（src/ui/build_progress.rs の Stop::Passed） | Checks passed |
| グラフの値を表で表示しています | Showing the chart's values as a table |
| 月の表示は日付ごとの一覧で出しています | Showing the month as a list by date |
| {title}{at} | {title}{at} |
| • {title}{at} | • {title}{at} |
| 最長 {m} 秒 | Up to {m} s |
| この build には Mirror が入っていません（feature mirror で build した Shibubu で映ります） | This build can't mirror windows. Use a Shibubu built with the mirror feature |
| この Page の中で直接操作できます（読み込みに失敗した時は理由を下に出します） | {url} · Use it right here on the page |
| window を開く | Open window |
| この Terminal は Project の Page にだけ置けます。Page の「＋ Terminal」で置き直してください | A terminal can only sit on a project page. Add it again with “+ Terminal” on the page |
| 送る | Send |
| 新しく起こす | Restart |
| ← | arrow-left icon, ICONS-1101 (accessible name: Back) |
| → | arrow-right icon, ICONS-1101 (accessible name: Forward) |
| ↻ | refresh icon, ICONS-1101 (accessible name: Reload) |
| URL または検索 | Search or enter website |
| 波形の数値がありません | No waveform values |
| 押せない: {} | Can't press: {} |
| 用意された文: {} | Preset text: {} |
| 表示の条件: {v} | Shown when: {v} |
| 選んだ部品 | Selected part |
| ＋ | ＋ (accessible name: Add project) |
| WORKSPACE | Workspace |
| YOUR APPS | Your projects |

### Review fixes (G14 review-UI-1 and sendback, applied at INTEG-UI-a)

Strings the code does not show yet. The wave that changes the code takes the EN from here.

- `Stopped` (sendback 35.3, G14 A5): a Cancelled run stopped before anything changed, so `Undone` reads as "put back". `Undone` stays for a run that was undone (SPEC §6 U3, the Runs row in FINAL).
- `Hide sidebar` / `Show sidebar` (sendback 10): the title line button, its tooltip and the ⌘K rows. ⌃⌘S is the macOS Show / Hide Sidebar key.
- `⊘ Not set up` (G14 B7): a Connections row with no address or no command registered is not `Available`.
- `▣ Look inside {App} {version}?` (sendback 35.5, G14 A7): the title of the analysis approval. The body is the frame in rules.md; the buttons are `Look inside` and `Cancel`.

| JP | EN |
|---|---|
| (new) Runs result, Cancelled before anything changed | Stopped |
| (new) title line button and ⌘K row, sidebar open | Hide sidebar · ⌃⌘S |
| (new) title line button and ⌘K row, sidebar folded | Show sidebar · ⌃⌘S |
| (new) Connections row with nothing set up | ⊘ Not set up |
| (new) analysis approval title | ▣ Look inside {App} {version}? |

### Brought in by INTEG-UI-a (src/app.rs, src/ui/engine_view.rs)

W19-2 and W19-3 strings that reached yc/ui with the trunk merge (missing-en.md A and B).

- `Hand {} to the AI`: gui/mod.rs:247 already tells people to press “Hand {app_name} to the AI”, so the button reads the same. Several apps are joined with ", ".
- `Let the AI control {}? …`: the Desktop panel question beside that button. "Nothing is sent" matches gui/mod.rs:247.
- `Needs your consent before sending`: the state line above the consent button (5 words).
- `, and hands {} to the AI`: the end of the approval line, with the same verb as the button.
- The analysis approval line: the frame is in rules.md (b) Frames. `the analysis tool` matches capability_engine.rs:2332, `won't break anything` and `that claim is …'s` match capability_engine.rs:2342.

| JP | EN |
|---|---|
| {} を AI に任せる | Hand {} to the AI |
| {} を AI が操作してよいですか | Let the AI control {}? Nothing is sent to it until you press the button |
| 送る前に同意が要ります | Needs your consent before sending |
| この app | this app |
| ・{} を AI に任せる同意も記録 | , and hands {} to the AI |
| 解析の道具 | the analysis tool |
| 版は不明 | version unknown |
| 版 {v} | version {v} |

### UI-6a (src/ui/palette.rs, src/app.rs)

The header `•••` is `(removed; items moved to menus)` in FINAL (copy.en.md:18). Until the menu bar exists, its items are ⌘K rows (sendback 73, 76, 78; the header menu itself goes after yc/ui-5a). `Duplicate canvas` and `Delete canvas…` are FINAL rows already.

- `Appearance · {mode}`: the three theme rows. The mode words are the header menu's (`Match system`, `Light`, `Dark`), after the FINAL label `Appearance`.
- `You need at least one canvas`: the delete row when only one canvas is left, drawn as a row that can't be pressed (same shape as `A project needs at least one page`).
- `Verified` replaces `Has results` on a legacy workflow step (G14 C4): `Has results` is not a state word; a step result that is neither failed nor unverified is the Verified case (app.rs `verified`), next to `Not verified` and `Failed`.

| JP | EN |
|---|---|
| (new) ⌘K row, theme follows the system | Appearance · Match system |
| (new) ⌘K row, light theme | Appearance · Light |
| (new) ⌘K row, dark theme | Appearance · Dark |
| (new) ⌘K row, delete with one canvas left | You need at least one canvas |

### Pressing a control in another app (M2 G-c・G-e: src/ui/engine_view.rs, src/adapters/mac_gui.rs)

The approval line says what is pressed, in the words the person used. The way the control is found (`match`), window numbers and element roles never show.
G79: every character of what a step sends is shown (nothing is shortened with “…”), because approving sends exactly this, once. Internal values (key names, modifier names, the document field) are never listed as “Send “…””; keys are shown as macOS shows them (⌃⌥⇧⌘, arrows as ↑ ↓ ← →).

| JP | EN |
|---|---|
| (new) approval step, a control in an app | Press “{label}” in {App} |
| (new) approval step, a menu item in an app | Press {Menu › Item} in {App} |
| (new) approval step, where a step acts when it names a document (G79) | {App}'s “{document}” — used in place of {App} in the steps below and above |
| (new) approval step, replace one place in a document (G79) | In {App}'s “{document}”, replace “{old}” with “{new}” |
| (new) approval step, replace all the text of a document (G79) | In {App}'s “{document}”, replace all the text with “{text}” |
| (new) approval step, type into a field that isn't a document (G79) | Type “{text}” in {App} |
| (new) approval step, a key (G79) | Press {⌘S} in {App} |
| (new) while a change shows in the window but the saved file doesn't have it yet (G81 · the waiting line under “Working on “{goal}””; `✓ Verified` only after the file on disk has it, else G74's “Couldn't confirm whether it was sent…”) | Changed in {App} · checking the saved file… |
| (new) `✓ Verified` when the saved file on disk was read back too (G81 · {n} = the other checks in the window; with no saved file to read — no repo — it stays “Done. Checked {verified} items in the original app” and never mentions saving) | Done. Checked {n} items in the original app and the saved file（{n} = 1 は “1 item”） |
| (new) `✓ Verified` when only the saved file was read back (G81) | Done. Checked the saved file |
| (new) approval line with one step | To do “{goal}”, Shibubu will send 1 step: {step} (approving sends exactly this, once, and hands {App} to the AI) |
| (new) nothing in the window has that name | No control whose name contains “{words}” was found in {App}'s window. Nothing was pressed |
| (new) nothing with that name in the part of the window that could be read | No control whose name contains “{words}” was found in the part that was read (only part of the window was read). Nothing was pressed |
| (new) several things in the window have that name | There are {n} controls whose name contains “{words}” in {App}'s window. Nothing was pressed. Use more of the name |
| (new) the request names nothing to press | Name what to press, then ask again |

### Corrections to FINAL rows

A row here replaces the FINAL row with the same JP. These are the places where one EN stood for several different JP strings; each fix says what differs.

| JP | EN |
|---|---|
| タイマー | Dev environment for a repo |
| 在庫管理 | Inventory tracker |
| 研究ノート（chips） | Research notes |
| タイマーを作って | Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. |
| 在庫を管理したい | Track my inventory |
| 研究ノートを作りたい | Keep research notes |
| ＋ 新しいアプリ | Create project |
| 新しいアプリ (⌘N) | Create project · ⌘N |
| GitHub + IDE + Webを統合 | Create canvas for GitHub, editor and web |
| 対象と操作を確認してください。ワークフロー途中で失敗するとそこで停止します。 | Check the target and action. If a step fails, the rest stops. |

### Same EN on purpose

Several JP strings that mean the same thing in the same place map to one EN (the code has more than one spelling; the screen has one). The check skips these EN strings.

| EN | JP strings |
|---|---|
| No projects match | 一致する項目はありません; 絞り込みに一致するアプリはありません |
| Try again | もう一度; もう一度作る; 再試行 |
| Open in {App} | 元の app で続ける; 元の app で開く; 外で開く |
| Delete | 削除する; 消す |
| Cancel | 送らない; 調べない; キャンセル; キャンセル（接続中） |
| [app icon] {file} · {App} · {who acts} | External · 同期の確度; Mirror; 記号 + app 名 · 状態語 · Manua |
| Needs permission | 許可が無い; 許可待ち |
| Mirroring | {} の窓を映しています; 映像で操作中 |
| This version can't mirror apps · Open in {App} | この版の Shibubu には {name} の; 映すための部品が入っていません（この版では映せま |
| Save | 保存; 保存（web-save） |
| Passed · {time} · {duration} (green check painted before it, ICONS-1101 追記 238) | 完了; 成功 |
| No runs yet | まだ走らせた test はありません。「走らせる; 実行履歴はまだありません。 |
| {App} window · {control} | AX: {label}; PID {pid}; Window {window_id} |
| Undo | 取り消し; 取り消す |
| Checking | 確かめ中; 確認中; 確認中… |
| Stop | 停止（execution strip）; 止める |
| Install Claude Code, then run claude auth login in Terminal | claude が見つかりません。Claude C; no_cli claude が見つかりません。… |
| Send | 送る; 送信 |
| Run | 実行; 実行（default submit labels |
| the AI call couldn't be prepared on disk | no_cli 呼び出しの記録を書けません; 作業 folder を作れません（{kind}）; 呼び出しの準備の file を書けません |
| Add part · GitHub issues · GitHub pull requests · Web data · Screen control · Repo feature · Workflow · Notes | 画面の操作 · Repoの機能 · Workfl; ＋ UIパーツ · GitHub Issue · |
| Import from GitHub… | ＋ GitHub Repoの機能を導入; ＋ GitHubの一部分を取り込む |
| New issue · Title · Description · Create issue… | Issueを作成（要確認）; Issue作成を確認; タイトル … |
| Import from GitHub  / owner/repository / Folder in the repo | GitHubの機能だけを導入（window）/ ; 機能のフォルダ |
| Check licence, path and commit / Commit · Licence · Description · Licence source | Commit; Description; License … |

### text (fixed)

UI-pro-copy-3. The `text` rows with no interpolation, shown to the user as they are (class (a) of `ui-strings.py --classify`). Same shape as above; `=` in front of the JP keeps ` / ` inside one string.

| JP | EN |
|---|---|
| 確かめる thread を起こせませんでした | Couldn't start the verification thread |
| 確かめる途中で止まりました | Verification stopped partway |
| =Repo Feature / 部分導入 | Repo feature / partial import |
| 2レーン並列Workflow | Two-lane parallel workflow |
| Web JSONを読む | Get web data |
| はじめての統合アプリ | My first combined app |
| 保存待ち | Waiting to save |
| 保存済み | Saved |
| 信頼済 CLI | Trusted CLI |
| =Desktop / 画面の操作 | Desktop / screen control |
| メモ | Notes |
| Webサービスを読み取る | Read a web service |
| 画面の操作 | Screen control |
| 取り込んだ機能のCLI | Imported command |
| 明示登録CLIの実行 | Run registered command |
| 失敗 | Failed |
| 未検証 | Not verified |
| 結果あり | Verified |
| GitHubのIssue・PR、IDEの編集、Webサービスをまとめる | Bring together GitHub issues and pull requests, IDE edits and web services |
| 上の自然言語欄から統合UIを生成、または『＋ UIパーツ』で追加 | Describe what to build in the box above, or use Add part |
| 未接続 | Not connected |
| 固定 | Fixed |
| 選択Repoの信頼済CLIを実行 | Run trusted command |
| Feature未登録 | No feature registered |
| 目的 | Purpose |
| 未記入 | Not filled in |
| 構成 | Contents |
| 接続方式 | Connection type |
| =GitHub API / HTTPS / Shibubu Bridge（AX）/ 信頼済CLI | GitHub API / HTTPS / Shibubu Bridge (AX) / Trusted CLI |
| 自律操作 | Autonomous actions |
| なし。実行は毎回確認します | None. Shibubu asks before every run |
| 画面の操作（Shibubu Bridge） | Screen control (Shibubu Bridge) |
| Shibubu Bridge を起こせないかアクセシビリティの許可が無いので、システム設定 › プライバシーとセキュリティ › アクセシビリティで Shibubu を許可して「再確認」 | Screen control didn't start, or Accessibility is off · Open settings |
| 続けての編集を 1 回で組めなかったので、1 つずつ頼んでください | Couldn't build those edits in one go. Ask for them one at a time |
| この Project には Page を描く app がありません。Apps から作り直してください | This project can't draw its pages. Create it again from Home |
| この app は Page の入れ物です。上の Page タブで移れます | This app holds the pages. Switch between them with the page tabs above |
| AI の呼び出しが途中で止まりました | The AI call stopped partway |
| 頼んだ事の処理が途中で止まりました・もう一度頼んでください | Your request stopped partway. Ask again |
| この Project にその面がありません。Project を作り直してください | This project doesn't have that screen. Rebuild the project |
| まだ読み戻していません | Not read back yet |
| 文書 | Document |
| この app は開いていません。Apps から開き直してください | This project isn't open. Reopen it from Home |
| この Page の app は開いていません。Apps から開き直してください | The project for this page isn't open. Reopen it from Home |
| この Page に app がありません | This page has no app |
| 後の版か壊れた保存です | Saved by a newer version, or damaged |
| 主役の部品が描かれ、入力欄が focus を取りました | The main part was drawn and the input field has focus |
| 主役の部品が描かれました（入力欄の無い画面） | The main part was drawn (a screen with no input field) |
| 1 手 | 1 step |
| 1 手の操作がありません | This step has no actions |
| まだ何も入っていません | Nothing here yet |
| PTY が起きて prompt が返りました | The terminal started and showed a prompt |
| 1 行打ちました | Typed one line |
| この Page を開くと読み込みます | Opening this page loads it |
| この Page を開くと映します | Opening this page mirrors it |
| 権限がありません | Not permitted |
| 承認されませんでした | Not approved |
| 入力の形が違います | The input has the wrong format |
| 接続が切れました | The connection dropped |
| 時間切れ | Timed out |
| 同じ操作が実行中です | The same action is already running |
| 対象が 1 つに決まりません | Couldn't tell which one you mean |
| 相手に断られました | The other side refused |
| 内部の誤り | Internal error |
| 使い方: shibubu project pages\|context\|surfaces [--json] | Usage: shibubu project pages\|context\|surfaces [--json] |
| Terminal の記録 | Terminal history |
| 面の確かめ | Screen checks |
| まだ確かめていません | Not checked yet |
| 確かめ済み | Checked |
| 使えません | Unavailable |
| 面はありません（Page に app か Web を置くと出ます）\n | No screens (place an app or website on a page and they appear)\n |
| この shell は Project を知りません。Shibubu の Project の Terminal の中で打ってください | This shell doesn't know the project. Type this in a Shibubu project's terminal |
| none: WebView の中身は自己申告だけ（操作とは別の道が無い） | none: what's inside the web view is only self-reported, with no separate way to check |
| find_app: 入っている（操作の効果はまだ読んでいない） | find_app: installed (the effect of actions hasn't been read back yet) |
| Shibubu が全画面です。元の窓は前の Space に残ります | Shibubu is full screen. The original window stays in the previous Space |
| 元の窓が全画面です | The original window is full screen |
| 元の窓は今の Space にありません | The original window isn't in this Space |
| この面は Page に重ねる種類ではありません | This screen can't be placed over a page |
| アクセシビリティの許可が要ります。システム設定で Shibubu を on にしてから、この Page に戻ってください | Accessibility access is needed. Turn Shibubu on in System Settings, then come back to this page |
| 入力 | type |
| focus の移動 | move focus |
| 窓の移動 | move the window |
| この面に file の名前がありません | This screen has no file name |
| この Project に repo の場所がありません | This project has no repo location |
| 窓を重ねる部品が動いていません | The part that overlays windows isn't running |
| Page が 0 枚 | No pages |
| header の長さが無い | The header length is missing |
| header の JSON を読めない | Can't read the header JSON |
| header に seq が無い | The header has no seq |
| helper が止まりました | The helper stopped |
| stdin がありません | No stdin |
| stdout がありません | No stdout |
| 使わない方の窓を閉じると、ここに映ります | Close the window you don't need and this one shows here |
| 元の窓を映せませんでした。映し直し続けています（続く時は Shibubu を起こし直してください） | Couldn't mirror the original window. Shibubu keeps retrying (if this keeps happening, restart Shibubu) |
| この操作 | this action |
| アクセシビリティの許可が要ります。システム設定 → プライバシーとセキュリティ → アクセシビリティ で Shibubu を許可してください（それまで文字・key・click は送りません） | Accessibility access is needed. In System Settings → Privacy & Security → Accessibility, allow Shibubu (until then no text, keys or clicks are sent) |
| Native Direct（重ねて表示） | Native Direct (overlaid) |
| Native Portal（研究） | Native Portal (research) |
| Kangae の画面 | Shibubu screen |
| 表示 | Display |
| 操作 | Control |
| 状態 | State |
| 確認済み | Confirmed |
| 非対応 | Not supported |
| 前の窓に題名が無かったので、同じ窓か決められません。重ねる窓を選び直してください | The previous window had no title, so Shibubu can't tell if it's the same one. Choose the window to cover again |
| まだ重ねていない | Not overlaid yet |
| 元の app が直接受ける。まだ人の操作を見ていない | The original app receives input directly. No human input observed yet |
| この PoC に Agent の GUI 操作は無い | This proof of concept has no agent GUI control |
| 状態の読み戻しは繋いでいない | State read-back isn't connected |
| 窓の位置と大きさを枠に合わせ、動きに付いていくため（画面の中身は読みません） | To fit the window's position and size to the frame and follow its movement (screen contents are not read) |
| 未検出 | Not detected |
| 検出 | Detected |
| 重ねて表示中 | Overlaid |
| Kangae が隠れている | Shibubu is hidden |
| 追従中 | Following |
| 元の app が無い | Original app not found |
| 重なっていない | Not overlapping |
| 切り替え中 | Switching |
| 外で開いている | Open outside |
| 閉じた | Closed |
| stdin を開けません | Can't open stdin |
| stdout を開けません | Can't open stdout |
| stderr を開けません | Can't open stderr |
| Kangae が全画面です。元の窓は前の Space に残るので重ねられません（全画面を解くと合わせ直せます） | Shibubu is full screen. The original window stays in the previous Space, so it can't be overlaid (exit full screen to realign) |
| アクセシビリティの許可が外れました（システム設定で on にしてから 合わせ直す） | Accessibility access was turned off (turn it on in System Settings, then Realign) |
| Kangae が最小化されています（元の窓はそのまま） | Shibubu is minimized (the original window is unchanged) |
| 元の窓で開いている | Open in the original window |
| 読み戻しの frame が枠と許容内・前後で Kangae の窓より手前 | The read-back frame is within tolerance of the frame and in front of Shibubu's window |
| 読み戻しの frame が枠と許容内・Kangae の透明な枠の真下（間に枠と重なる窓が無い） | The read-back frame is within tolerance and directly below Shibubu's transparent frame (no overlapping window in between) |
| 元の窓が今の画面に無い | The original window isn't on this screen |
| Kangae の窓の後ろに隠れている | Hidden behind the Shibubu window |
| 元の窓が無い | No original window |
| 窓が枠から外れた | The window left the frame |
| 真上に置いた | Placed directly above |
| 真下に置いた | Placed directly below |
| AppKit が読み込まれていません（窓の無い process） | AppKit isn't loaded (a process with no windows) |
| AppKit が読み込まれていません | AppKit isn't loaded |
| Edit menu は既に在る | The Edit menu already exists |
| Edit menu を足した | Added the Edit menu |
| AX の木から WebView を外した | Removed the web view from the accessibility tree |
| この build に WebView は入っていません（cargo の feature webview） | This build doesn't include the web view (cargo feature webview) |
| sign-in は本人の browser でしてください（Kangae の中では login しません） | Sign in with your own browser. Shibubu doesn't sign in for you |
| 中身の process が 2 回終了しました | The content process quit twice |
| 読み込みが途中で止まりました | Loading stopped partway |
| YouTube がこの埋め込みを再生しません（Error 153） | YouTube doesn't play this embed (Error 153) |
| WebContent process が終了したので 1 回読み直した | The web content process quit, so it was reloaded once |
| 検査に落ちる:  | Fails the checks:  |
| 宣伝文の規則  | Marketing copy rules  |
| loading の skeleton が無い:  | No loading skeleton:  |
| 空打ちが落ちる:  | Crashes on empty input:  |
| 頼まれていない effect:  | Unrequested effect:  |
| 名指しに無い接続先:  | Connection not named:  |
| 指示に無い部品:  | Part not requested:  |
| 操作しませんでした（何も送っていません）・元の app の状態を確かめてから、もう一度頼んでください | Nothing was sent · Check {App}, then ask again |
| 頼んだ事の手順が返ってきませんでした・{looked}・元の app で続けてください（G100） | No steps came back for what you asked · {looked}. Continue in the original app |
| AI の答えを使えませんでした・言い換えて、もう一度頼んでください | Couldn't use the AI's answer. Rephrase and ask again |
| 足りない機能を作れませんでした（何も送っていません）・元の app で続けてください | Couldn't build the missing feature (nothing was sent). Continue in the original app |
| 結果を元の app で確かめられませんでした（済みとは出しません）・元の app で中身を見てください | Couldn't check the result in the original app, so it isn't marked done. Look inside the original app |
| 元の app に置いた物を確かめられなかったので、送る手は打っていません・元の app で確かめてから、もう一度頼んでください（G74・G100） | Couldn't confirm what was put in the original app, so Shibubu didn't do the send step. Check it in the original app, then ask again |
| 送れたかを確かめられませんでした（済みとは出しません）。もう一度は送りません・元の app で中身を見てください（G74） | Couldn't confirm whether it was sent, so it isn't marked done. It won't be sent again. Look inside the original app |
| {app} の欄に Shibubu が置いていない字（{n} 字）が既に在るので、何も送っていません・{app} で送るか消してから、もう一度頼んでください（G74） | {app}'s field already has text in it ({n} characters) that Shibubu didn't put there, so nothing was sent. Send or clear it in {app} yourself, then ask again |
| 押す {role} が dialog の中に無いので、何に効くかを確かめられません。何も押していません（G74b） | The {role} to press isn't inside a dialog, so what it would act on can't be checked. Nothing was pressed |
| dialog が見せるはずの字のうち {n} 個（全部で {chars} 字）が見当たらないので、別の物の dialog かもしれません。何も押していません（G74b） | The dialog doesn't show {n} of the {count} texts it should ({chars} characters in all), so it may be about something else. Nothing was pressed |
| dialog が見せるはずの字が空です。何も押していません（G74b） | The text the dialog should show is empty. Nothing was pressed |
| {app_name} で「{document}」が開いていません。何も変えていません・開いてから、もう一度頼んでください（G75・別の窓で代用しない） | “{document}” isn't open in {app_name}. Nothing was changed. Open it, then ask again |
| 「{d}」が {app} の前の窓ではないので、この手は送っていません（別の窓に効くため）。{app} で「{d}」を前に出してから、もう一度頼んでください（G75） | “{d}” isn't the front window of {app}, so this step wasn't sent (it would go to another window). Bring “{d}” to the front in {app}, then ask again |
| 手順が「{d}」でない {name} の窓に効くので、何も送っていません。もう一度頼んでください（G75） | The steps would act on another {name} window than “{d}”, so nothing was sent. Ask again |
| {app} の窓の一部しか読めなかったので、合う行を数えられません（G74b） | Only part of {app}'s window was read, so the matching rows can't be counted |
| Shibubu は {press} を送りません。何も押していません（G74b） | Shibubu doesn't send {press}. Nothing was pressed |
| {app} に menu が開いていないので「{label}」を押していません（窓の中の何も押していません）（G74c） | No menu is open in {app}, so “{label}” wasn't pressed (nothing in the window was pressed) |
| {app} に開いている menu（{n} 項目）に「{label}」が無いので、menu の中の何も押していません。menu を閉じました / menu はまだ開いています。{app} で Esc を押して閉じてください（G74c） | The menu open in {app} ({n} items) has no “{label}”, so nothing in it was pressed. The menu was closed / The menu is still open there. Press Esc in {app} to close it |
| {app} に menu が開いていて窓の残りを隠しているので、今は読みません。menu を閉じてから、もう一度頼んでください（G74c） | A menu is open in {app} and hides the rest of its window, so it isn't read now. Close the menu, then ask again |
| 先に元の物を読めなかったので、何も送っていません: {why}（G74d・手を走らせる前の読み） | Nothing was sent, because the original couldn't be read first: {why} |
| 手順の中の確かめが Shibubu の読めない形で書かれていたので、何も送っていません。もう一度頼めば手順を書き直します（B75・手を走らせる前の読みが形の違う入力） | Nothing was sent, because a check in the steps was written in a form Shibubu can't read. Ask again to have the steps written again |
| 手順がこの Project の repo の外の file を読もうとしたので、何も読まず送っていません。もう一度頼めば手順を書き直します（B76・model の読みは repo の下だけ） | Nothing was read or sent, because the steps would read a file outside this Project's repo. Ask again to have the steps written again |
| （止まった手の括弧の中）この Project の repo の外の file を読む手でした。もう一度頼めば手順を書き直します（B76・前の手の出力で埋めた後の場所が外・前の手は走った） | it would read a file outside this Project's repo; ask again to have the steps written again |
| 手「…」は、この Project の repo の外の file を読む手なので走らせていません。後ろの手も走らせていません（B76・run_workflow の口） | The step “…” wasn't run, because it would read a file outside this Project's repo. The later steps were not run |
| {why}。Shibubu はその手で止まったので、その手と後ろの手は走っていません（消す・送る手は打っていません）。走ったのは最初の {n} 手だけです。元の app の中を見てください（G74c・3 つ目の文） | {why}. Shibubu stopped at that step, so it and the steps after it didn't run (nothing they would delete or send was done). Only the first step ran / Only the first {n} steps ran. Look inside the original app |
| 保存した機能を読み書きできません・保存先の空きと権限を確かめてから、もう一度頼んでください | Can't read or write the saved features. Check free space and permissions where they're stored, then ask again |
| 前に作った手順を使った（作り直していない） | Reused the earlier procedure (not rebuilt) |
| 足りない機能を作り、写しで試してから使った | Built the missing feature, tried it on a copy, then used it |
| 確かめた機能を組み合わせる手順を作った | Built a procedure that combines checked features |
| 作った物: なし（前に作った物を使った） | Built: nothing (reused earlier work) |
| 送らない | Cancel |
| 承認して調べる | Look inside |
| 調べない | Cancel |
| 調べる前に承認が要ります | Approval needed before looking |
| 送る前に承認が要ります | Approval needed before sending |
| 対象の app | Target app |
| 元の窓を探しています | Looking for the original window |
| Mirror で元の app の窓を映している | Mirroring the original app's window |
| 使えるか未確認です | Not confirmed as usable |
| 一部利用できません | Partly unavailable |
| 準備しています | Getting ready |
| 結果を確認しています | Checking the result |
| 送信済みか未確認 | Not confirmed as sent |
| 引き出し | Show terminal（G92 で Hide terminal は無くした） |
| 日付なし | No date |
| =再生 / 一時停止 | Play / Pause |
| 開く | Open |
| 録音を始める | Start recording |
| 端末に映っている物を表示するだけです（ここからは操作しません） | View only · shows {App}'s window |
| Shibubu の画面で表示します | Shows it in Shibubu |
| 元の画面をこの Page に置くのは後の版です | This version can't show it on this page |
| 埋め込まずに、元の app の window で開きます（編集もそちらで） | Opens in the original app's window instead of embedding (edit there too) |
| 元の app の window で開きます（編集もそちらで） | Opens in the original app's window (edit there too) |
| scope なし | No scope |
|  · 実行前に承認 |  · Approval before running |
| 権限 | Permissions |
| まだ確かめていません（在るかを確かめるのは Project を作る時です） | Not checked yet (whether it exists is checked when the project is built) |

### Same EN on purpose (text)

Two JP strings that read the same in English, because the second only adds a note or comes from the same state in another place.

| EN | Why |
|---|---|
| Not connected | Waiting for a connection and no connection are one state to the user |
| Not verified | Same state, shown in two panels |
| Failed | Same state, shown in two panels |
| Not approved | Same state, shown in two panels |
| Screen control | The second adds the Bridge name in a note |
| Not checked yet | The second adds when it will be checked, in a note |
| AppKit isn't loaded | The second adds the reason, in a note |
| {name}'s window was moved from outside, so following stopped | The second differs only in the hint about a tiling manager |

### Mirror window buttons (UI-5m, src/ui/mirror_view.rs)

New strings with no JP source (the title band of a Mirror surface, same three buttons as a macOS window). The buttons are glyph-only circles; the EN is their accessible name.

| JP | EN |
|---|---|
| (new) 赤の button | Close mirror |
| (new) 黄の button | Minimize mirror |
| (new) 緑の button | Zoom mirror |
| (new) 拡大中の緑の button | Restore mirror |
| (new) 畳んだ帯の button | Open mirror · {App} |
| (new) 帯の題 | {App} · {document} |
| (new) 本物が枠に合わない時の帯の button（G91 改 2） | Fit to frame |
| (MIRROR-FIT-1101・旧 `{App} keeps its own size · showing it scaled` を替えた) その button の hover・本物が Page より大きく縮めて見せている | {App}'s window is larger than this page · showing it scaled |
| (MIRROR-FIT-1101) その button の hover・本物が外から動かされ自動の合わせ直しを使い切った | {App}'s window was moved off the frame · showing it here |
| (new) 窓の形を保存できなかった 1 行（src/generated.rs `save_mirror_choices`） | Couldn't remember how the mirror was left ({e}). It opens again after a restart. |

### AI errors (src/llm/mod.rs `LlmError` Display · r5 #21)

`LlmError` fills `{reason}` (copy.en.md "Errors that fill {reason}"). The variants with a FINAL row use it as is; these are the ones FINAL doesn't have, and the one that keeps its `{detail}`.
The inner technical text (a parse message, a schema path, an io error) never shows (D-e): r5 #21 showed 「AI の応答を読めませんでした（…）。もう一度試してください。」 with the parse text inside.

| JP | EN |
|---|---|
| AI の応答を読めませんでした（{detail}）。もう一度試してください。 | AI's answer couldn't be read · try again |
| AI の答えが決まりに合わないので使いませんでした（{path}: {reason}）。 | AI's answer didn't fit, so it wasn't used |
| 依頼の schema が送れない形です（{path}: {reason}）。 | the request to the AI couldn't be made |
| AI を呼ぶ上限（{detail}）に達したので、これ以上呼ばずに止めました。AI 無しの手元の推定で続けます（Settings › AI & Models › Execution limit で上限を上げると続きます）。 | AI was stopped at its limit ({detail}) · raise it in Settings › AI & Models › Execution limit |
| {} 回 / {} 秒（上限の detail） | {n} call / {n} calls / {n} seconds |
| AI の plan の使用量の上限に達しました（{detail}）。上限が戻ってから送り直すと AI が読みます。（FINAL の行に CLI の戻る時刻を足す・W13） | your Claude plan's limit is reached ({detail}) · try again after it resets |
| 確認の画面（“{label}”）が {app} に出なかったので、“{label}” は押していません（menu が開いたままなら {app} で Esc を押して閉じてください）。（G74e・確認の dialog の button の手・走った手の後なので Stopped の文の先頭） | No confirmation with “{label}” appeared in {app}, so “{label}” wasn't pressed. A menu is still open there. Press Esc in {app} to close it |
| {app} が隠れている（非表示）ので、何も送っていません。{app} の窓を出してから（他の窓の後ろのままで構いません）もう一度頼んでください。（G74f・隠れた app に送る手の前・どの app でも・勝手に見せない / 前に出さない） | {app} is hidden, so nothing was sent. Show its window (it can stay behind other windows), then ask again |
| この目的は {n} 個の操作に分けて保存してあり、AI の道具が要ります（1 手も送っていない）· Settings › AI & Models で足してください。（WF-E x6b・model が初めから無い run・operator 10-29） | This goal has {n} actions and needs an AI tool · Add one in Settings › AI & Models |

### Brought in by FX4 (src/ui/palette.rs, src/ui/shell.rs, src/model.rs)

The one field and the ⌘K list under it. Right-end words now say what ⌘↵ does when it differs from Enter; the AX name is the part before ` · `.

| JP | EN |
|---|---|
| (new) ⌘K row, recent commands off (default) | Remember recent commands |
| (new) ⌘K row, recent commands on | Forget recent commands |
| (new) ⌘K list heading above the last chosen rows | Recent |
| (new) field hint while the ⌘K list is open | Type a command or project name |
| (new) right end in a project | ↵ Change · ⌘↵ New project |
| (new) right end on Home, exact project name | ↵ Open “{name}” · ⌘↵ Create |
| (new) right end in Runs and Connections, a project or app matches | ↵ Open · ⌘↵ New project |
| (new) right end in Runs and Connections, another command matches | ↵ Run · ⌘↵ New project |
| Desktopで処理する (old canvas workflow) | Desktop steps |
| ようこそ (old canvas first part) | Notes |

### Brought in by FX5 (src/app.rs, src/github.rs)

The GitHub row reads the sign-in back (from `gh auth login`) and says who and when. It replaces `Token set · not checked` and `No token · public repos only` (copy.en.md:441-442). No environment variable name is shown.

| JP | EN |
|---|---|
| (new) GitHub row, signed in (read back from GitHub) | Signed in as {login} · checked {HH:MM} |
| (new) GitHub row, the reading is older than 10 minutes | Signed in as {login} · checked {HH:MM} · may be out of date |
| (new) GitHub row, no gh and no sign-in | Not signed in · Install the GitHub CLI (gh), then run gh auth login |
| (new) GitHub row, gh is there but not signed in | Not signed in · Run gh auth login in a terminal |
| (new) GitHub row, GitHub refused the saved sign-in | GitHub didn't accept the saved sign-in · Run gh auth login in a terminal |
| (new) refusals that used to name the token variable | …run gh auth login, then Check again in Connections |
| KANGAE_WEB_SERVICE_URL 未設定 / No address set / 日本語の拒否 | Web data can't be set up in this version |

### Brought in by FX7 (src/ui/runs.rs, src/generated.rs, src/runtime/receipt.rs)

A run that was sent but not confirmed comes first in Runs and offers `Check again`. It re-reads Shibubu's record; it never sends again.

| JP | EN |
|---|---|
| (new) Runs detail button for `Sent, not confirmed` | Check again |
| (new) after Check again, still unknown | Still not confirmed. Check {place} to see whether it arrived. Shibubu won't send it again. |
| (new) after Check again, the record says more | Checked again: {result}. |
| (new) after Check again, the run is gone | That run isn't in the history any more. |
| 再起動: 実行の途中で止まった。届いたか分からないので自動で送り直さない | Shibubu closed while this was being sent. It may have arrived, so it was not sent again |

### Installing an app (PV0・PV1: src/project/acquire.rs, src/generated.rs)

New strings with no JP source. Homebrew (the package manager) and Applications (the macOS folder) are names, so they keep their capital mid-sentence.

| JP | EN |
|---|---|
| (new) Homebrew の listing を読めない 1 行（acquire `listed`） | Couldn't read Homebrew's listing for {name} · Get it from {page} |
| (new) brew を起こせない 1 行（acquire `listed`・`install`） | Couldn't run Homebrew ({e}) · Get {name} from {page} |
| (new) 入れる直前の listing の読み直しが落ちた 1 行（generated `install_act`） | Couldn't check Homebrew's listing · Nothing was installed |
| (new) review の 1 行の変更（acquire `candidate`） | Adds {file} to Applications |

### Brought in by the CU findings of the morning gate (src/generated.rs, src/ui/header.rs)

| JP | EN |
|---|---|
| (new) Web save, the disk was read back and matched | Written to {path} · read back from disk |
| (new) right end of the field when `↵ Change · ⌘↵ New project` doesn't fit | ↵ Change (the full text is the tooltip) |

### Brought in by the CHAT-011 review (src/app.rs, src/ui/home.rs, src/domain/mod.rs `chat`)

| JP | EN |
|---|---|
| (new) the line left in the Home chat after the plan's `Later` (Y2), followed by ` · {the plan}` | Saved for later · nothing was built |
| (new) why `Accept` is grey while the AI reads the plan or the request (Y6; was "Wait for the current build to finish" for every wait) | Wait until the AI finishes reading |
| (new) why `Accept` is grey while the AI is being stopped (Y6) | Wait until the AI stops |

### Brought in by TERM-ONE-1101 (src/generated.rs `PageIntent::SetWorkspace`)

| JP | EN |
|---|---|
| (new) the Page line after the project's folder changes; terminals already running stay where they are (`{name}` = the folder's last name) | New terminals open in {name} |

## Brought in by SESSIONS-LEFT-1101

| JP | EN |
|---|---|
| 押して作ったまだ一言の無い session の左の行の題 | New session |
| 下の帯の Chat の tab（{title} = 今の session の題・24 字で切る・AX 名は Chat のまま） | Chat · {title} |
| Project の行の右端の + の AX 名と上に置いた時（見た目は +・{name} = Project の名前・追記 244 から hover と Tab の focus の時だけ出る） | New session in {name} |
| 左の sidebar の見出しと Project の行の hover で出る ⌄ の AX 名と上に置いた時（{name} = 見出しか Project の名前・開いている時） | Collapse {name} |
| 同じ ⌄ の畳んだ時（見た目は › に回る） | Expand {name} |
| 同じ行の ••• の AX 名と上に置いた時 | More for {name} |
| Your projects の見出しの + の AX 名と上に置いた時（⌘N と同じ = Home の新しい Project） | New project |
| Sessions の見出しの + の AX 名と上に置いた時（左上の New chat と同じ） | New chat |
| Your projects の見出しの ••• の menu（Archive した Project を後ろに出す・{n} = その数） | Show archived · {n} |
| 同じ menu の出している時 | Hide archived · {n} |
| 同じ menu の Archive した Project が無い時（押せない） | No archived projects |
| Project を Archive した時の窓の一番下の帯の 1 行 | Project archived · find it with Show archived in the ••• next to Your projects, or with ⌘K |

## Brought in by CHAT-SIDE-1101

| JP | EN |
|---|---|
| header の右端の ◨ の AX 名と上に置いた時（chat が下か畳んだ右の時・押すと右の sidebar に出す） | Show chat on the right |
| header の右端の ◨ の AX 名と上に置いた時（chat を右に出している時・押すと下へ戻す） | Hide right sidebar |

## Brought in by SIGNIN-1101

| JP | EN |
|---|---|
| sign in の tab を開いた時の知らせ（{name} = AI の名前） | Signing in to {name}… |
| Project が 1 つも無い時の下の 1 枚の sign in の terminal の見出し | Sign in to {name} |
| その 1 枚を閉じる button の AX 名（見た目は ×） | Close sign-in terminal |
| その 1 枚の shell を起こせなかった（{e} = 理由） | Couldn't open a terminal to sign in to {name} · {e} |
| sign in の tab を閉じた | Sign-in to {name} stopped · its terminal was closed |
| sign in の tab の shell が終わった | Sign-in to {name} stopped · its terminal ended |
| sign in の command を知らない AI（FOUND_CLIS の外・普段は出ない） | Sign in to {name} from its own app · Shibubu doesn't know its sign-in command |
| sign in の後の確かめが通らない（tab は残る） | {name} isn't signed in yet · finish in its terminal below, then pick {name} again |

## Brought in by CONNECTIONS-1101

| JP | EN |
|---|---|
| 確かめが通った・使える行の状態の語（緑・Ready と Verified もこれ） | Connected |
| 使えない行の状態の語（赤・Available と Checked はもう出さない） | Not connected |
| 確かめている間の語 | Checking |
| Connected の語の上に置いた時 | Shibubu can use it now |
| Not connected の語の上に置いた時 | Shibubu can't use it yet · the note on the left says what it needs |
| Checking の語の上に置いた時 | Shibubu is checking it now |
| sign in が要る AI の行の文 | Sign in needed |
| sign in が要る AI の行の button | Sign in |
| OS の許可が切れた行の文（前は Off） | Permission needed |
| OS の許可の button（前は Open settings） | Allow |
| 行の下の鍵の欄の hint（{n} = token など） | Paste the {n} here |
| 鍵の欄と引数の欄の button | Save |
| 行の下の Web data の URL の欄の hint | Web data URL, e.g. https://api.example.com/items |
| URL の欄の button | Test |
| Web の行（欄を開いている間） | Type the address below · Enter tests it |
| 行の下の Trusted command の引数の欄の hint | Fixed arguments for the program · Enter for none |
| 行の下の MCP の欄の hint | Command or JSON that starts the MCP server |
| MCP の名前の欄の hint | Name (optional) |
| MCP の名前の欄の AX 名 | MCP server name |
| MCP の欄の button | Add |
| 欄を閉じる button の AX 名（見た目は ×） | Close |
| 鍵の欄が空・空白入り | Paste the whole {name} into the field · it has no spaces |
| 同じ名前の MCP が在る | There's already an MCP server named {name} · pick another name |
| GitHub が token を受け付けない | GitHub didn't accept the token · Paste token to add a new one |
| Web data が鍵を求めた | Needs a token · Paste token to add it |

### Brought in by PROJECT-DIR-1101 (src/ui/header.rs `project_actions`・src/app.rs `Intent::ShowProjectFolder`)

| JP | EN |
|---|---|
| (new) project menu item (`{name} ⌄` and the Home row's `•••`): opens the project's own folder (`~/Shibubu/{project}`, or the folder chosen for it) in Finder | Show folder in Finder |
| (new) project menu item: choose another folder for the project; new terminals open there | Change folder… |
| (new) status line when the project's folder couldn't be made (no room, no permission) | Couldn't make this project's folder · Choose one with Change folder… |
| (new) Project settings › Folder access when the project has no repo but has its own folder | Read and write · the Terminal works in this project's own folder |
| (changed) Save to repo with no repo (`SaveError::NoWorkspace`; every project has its own folder now, so it doesn't say "no folder") | This project has no repo to save into · choose one with Change folder… |

### Brought in by ACCOUNTS-1101 (src/ui/settings.rs `accounts`・src/llm_cli.rs `add_cli_account` / `cli_account_name` / `one_account_line`; Connections and Settings share the one AI accounts block)

| JP | EN |
|---|---|
| (new) button on each AI's heading in the accounts block (Claude and the CLIs that can hold more than one sign-in: Grok, Kimi, OpenCode); AX name `Add {name} account` | + Add account |
| (changed in ACCOUNTS-1101 2 段目) name of an account added for a CLI — see below | ~~{Name} · account {n}~~ |
| (new) second line of an added CLI account that is signed in | Signed in · kept by Shibubu |
| (new) second line of a CLI found on this Mac (its own sign-in, not one Shibubu keeps) | Its own sign-in on this Mac |
| (new) red mark next to the Sign in button on an account that isn't signed in | Needs sign-in |
| (new) line under the heading of a CLI that keeps a single sign-in on the Mac (Cursor Agent), instead of `+ Add account` | {Name} keeps one sign-in on this Mac · Shibubu uses that one |
| (new) status line when an AI without its own row tries to add an account | {Name} can't add another account |
| (new) inline confirm on Remove for an added CLI account | Remove {name}? Shibubu deletes its sign-in · nothing changes at {Provider} |
| (new) status line when Re-authenticate is asked of a CLI account | Sign in from its row in Connections |
| (changed) name of the first row in each AI's block (the AI's own sign-in: Claude Code's, or the CLI / local LLM found on this Mac; was the AI's name, so `Grok` showed twice under the `Grok` heading) | System default |

### Brought in by ACCOUNTS-1101 2 段目 (src/llm_cli.rs `account_name` / `numbered_name`・src/app.rs `account_label`・src/ui/settings.rs `rename_cell`・src/ui/shell.rs `own_usage_breakdown`・src/own_usage.rs `Counter::panel`)

| JP | EN |
|---|---|
| (changed) name of an added account (Claude plan or CLI) in its row: the name the person gave it, else the part of the sign-in email before @ (the email itself is never shown), else the AI and a number (System default is the first) | {name} · e.g. work · ryo · Claude 2 · Grok 2 (was `Claude plan · {email}` / `{Name} · account {n}`) |
| (changed) the same account in the chat bar Provider menu, the bottom bar and the usage breakdown heading (named) / (not named) | {AI} · {name} (e.g. Claude · work) / {AI} {n} (e.g. Claude 2) |
| (new) hover on an added account's name in Connections and Settings | Click to rename · Enter keeps it |
| (new) AX name of that name (Button) / of the inline field | Rename {name} / Name for {name} |
| (new) Settings › AI & Models: the same usage block as the bottom bar's panel (table, notes, breakdown, Recount) | Used in Shibubu |
| (changed) heading of the weekly breakdown (bar panel and Settings) | This week by account and project (was This week by AI and project) |
| (changed) group heading in the breakdown = the account's name only (was `{AI} · {account}`, so `Grok · Grok`) | Claude · work · Grok · Claude Code |
| (new) first line of an account group with two or more lines | Total |

### Brought in by HOME-TERM-1101 (src/app.rs `start_cli_sign_in`・src/generated.rs `HOME_PLACE`; Home has the same Terminal panel as a project)

| JP | EN |
|---|---|
| (removed) heading of the single sign-in terminal shown outside a project, and its close button's AX name; sign-in outside a project now opens a tab in Home's Terminal | ~~Sign in to {name}~~ · ~~Close sign-in terminal~~ |
| (changed) name of the sign-in tab outside a project (same as in a project's panel) | {Name} {n} (e.g. Kimi Code 1) |
| (changed) Home's Terminal panel `+ New` menu has no browser item (there is no page to put it on) | New terminal · {agent} (no New browser tab) |

## Brought in by MIRROR-QUIET-1101

| JP | EN |
|---|---|
| 窓を探す helper の常駐が続けて落ちた時の右上の知らせ（{why} = exit status） | Mirror's helper keeps stopping ({why}). Shibubu keeps restarting it |

## Brought in by STATUSBAR-1101 追記 2

| JP | EN |
|---|---|
| 下の帯の Grok・Kimi Code の hover: login の file が無い（{Name} = Grok / Kimi Code・{cmd} = grok login / kimi login） | {Name} isn't signed in · run {cmd} in Terminal |
| 同: login の期限が切れた（Shibubu は直さない・{bin} = grok / kimi） | {Name}'s sign-in expired · open {bin} in Terminal once |
| 同: 使用量の口が 401 / 403 | {Name} refused the sign-in · run {cmd} in Terminal |
| 同: 答えに % が無い account | {Name} didn't report a usage percentage for this account |
| Grok の月の窓の名前（hover の窓の名前） | Monthly |

## Brought in by STATUSBAR-1101 追記 229 (src/ui/shell.rs `status_bar`)

| JP | EN |
|---|---|
| (new) 窓の一番下の帯の右端の歯車の AX の名前と hover（押すと Settings） | Settings · ⌘, |

## Brought in by MIRROR-SPEED-1101

Settings › General（feature mirror の build だけ）。数を打つ欄は作らない（owner 205）。

| JP | EN |
|---|---|
| Mirror の映像の滑らかさ（欄の名前） | Mirror smoothness |
| 欄の下の 1 行 | Auto lowers the frame rate when your Mac is busy · Smooth shows 60 frames a second · Saver shows 15 to save battery and heat |
| 選ぶ 3 つ（既定は Auto） | Auto · Smooth · Saver |

## Brought in by TERM-XTERM-1101

| JP | EN |
|---|---|
| Terminal の中の検索の欄（⌘F で開く・xterm の頁の中）の placeholder | Find |
| 同: 見つからない時の欄の右の 1 語 | No results |
| 同: 前へ・次へ・閉じるの button の名前（AX） | Previous match · Next match · Close find |
| Terminal の中身を新しい描き手で開けなかった時の右上の知らせ（{why} = WebView の理由・今まで通りの描き方で続ける） | Couldn't open the terminal view ({why}), so it's drawn the old way |
| Terminal の右 click の Paste で clipboard を読めなかった時の Terminal の状態の 1 行（{e} = 理由） | Couldn't read the clipboard: {e} |

## Brought in by MIRROR-HYBRID-1101

触った Mirror の本物を前に出さない時の右上の知らせ（追記 214 から押すたびに・× まで残る・{App} = app の名前）。帯の `Using {App} directly · click again` は無くなった（触った押しも本物へ届く）。

| JP | EN |
|---|---|
| ~~本物が枠より大きい（縮めて見せている）~~（追記 214 で出さない: 押すと本物を枠の大きさにしてから前に出す） | ~~{App}'s window is bigger than this frame · Fit to frame to use it here~~ |
| 本物が枠に合っていない（AX に答えない・別の Space） | {App}'s window isn't lined up with this frame · it stays a mirror |
| 本物を置けない（窓の対応が付かない・許可が無い） | Can't bring {App} here · it stays a mirror |

### Brought in by MIRROR-HYBRID-1101 追記 214

Said on the notice at the top right when a touched Mirror can't bring the real window forward (each press · stays until ×). `{App}` = the app's name.

| JP | EN |
|---|---|
| (new) 押した枠の全部が Page に見えていない（はみ出した・被さった） | Show all of {App}'s frame on the page to use it here · it stays a mirror |
| (new) 押した時に大きかった本物が枠の大きさまで縮まなかった（app の最小の大きさ） | {App}'s window won't shrink to this frame · it stays a mirror |
| (new) 本物の窓をまだ枠に置いていない（Dock を作っている途中）所を押した | {App}'s window isn't ready here yet · try again in a moment |
| (new) 前に出す手を打って 1 秒たっても本物が前に来ない（macOS が断った） | {App} didn't come to the front · it stays a mirror |

### Brought in by MIRROR-FIT-1101 追記 218 の 3

Said on the Mirror frame's title band for 6 seconds after the person drags a corner (`{App}` = the app's name).

| JP | EN |
|---|---|
| (new) 本物の窓をしまう口（Dock）の無い面の隅を引いた = Page の上の枠だけ替えた | {App}'s window keeps its size · only the frame changed |
| (new) 隅を引いた大きさを本物が受けなかった（大きくできなかった）= 枠は本物の大きさになる | {App}'s window didn't take that size · the frame follows the window |
| (new) MIRROR-HYBRID-1101 追記 240 の 2: 隅を引いて小さくした大きさを本物が受けなかった（app の最小の大きさ）= 枠は引いた所のまま、本物を比のまま縮めて見せる・次からはその最小で止まる | {App}'s window can't be that small · showing it scaled |
| (new) MIRROR-HYBRID-1101 追記 245 の 2: 窓の無い書類の面の真ん中の button（{doc} = Project の repo に在る書類の名前・書類の無い面は今まで通り `Open {App}`） | Open {App} · {doc} |

### Brought in by ENGINE-GENERIC-1101 (src/generated.rs `part_event`)

| JP | EN |
|---|---|
| (new) the Shibubu line in the Project chat when a part's trigger (what the person asked earlier, kept in the Project by the AI) runs the Engine — not a `You` line, the person didn't type it now | Doing what you asked earlier: {words} |
| (new) why a part's trigger didn't run (the value it needs isn't chosen yet), on the part's line | Choose in “{part}” first |

### Brought in by TALK-HONEST-1101 追記 232 (src/ui/engine_page.rs `CoreEngine::read`・src/generated.rs `changed_line`)

部品の read が読めなかった 1 行（部品の中に赤で出る・{what} = 部品が探した物・{shows} と {next} は窓の行から AI が書く）と、編集の後の chat の 1 行（Project の名前を 2 回でなく何を変えたか）。

| JP | EN |
|---|---|
| (replaces `{app} isn't open · open it, then try again`) | Looked for {what} in {app}, but {app} isn't open · Open {app}, or let Shibubu open it |
| (replaces `{app} has no window open · open one, then try again`) | Looked for {what} in {app}, but it has no window open · Open a window in {app}, or let Shibubu open it |
| (replaces `{app}'s window doesn't show that now · show it there, then try again`) | Looked for {what} in {app}, but its window shows {shows} now · {next}, or let Shibubu open it |
| (new) 上の 3 つの後に部品の頁の最後に出る button（押すと Engine が app を開く・初めての手は承認の card・✓ なら部品をもう一度読む） | Let Shibubu open it |
| (new) 押したが開ける一言が無い（読めた・読み直している・読む道が無い）時の status の 1 行 | Nothing to open for this part now · read it again first |
| (Engine への一言・画面には chat の You の行で出る) | Open {what} in {app} |
| 同: AI が next を書かなかった時の次の一手 | Show it in {app} |
| (replaces `Changed {project}: {page titles}`) 足した Page | Changed {project}: added the “{page}” page |
| 同: 部品を置いた（名前が無ければ a part） | Changed {project}: added “{part}” to “{page}” |
| 同: Page の題を替えた / Terminal を置いた / 面を動かした（名前が無ければ a window）/ 他の Page の変更 | renamed a page to “{title}” · added a terminal to “{page}” · moved “{name}” to “{page}” · changed “{page}” |

## Brought in by UI-SWEEP-1101 追記 215 (src/ui/terminal_panel.rs `draw`)

下の panel の帯の左端の開閉は文字の無い icon 1 つ（AX の名前は今まで通り `Show terminal` / `Hide terminal` / `Show chat` / `Hide chat`）。hover の文だけ替えた。

| JP | EN |
|---|---|
| 開閉の icon の hover（{name} = Show terminal / Hide terminal / Show chat / Hide chat） | {name} · ⌃` |
| 同: Terminal の tab が在り開いている時（隠しても shell は動いたまま・前は「Shells keep running while hidden · ⌃`」） | Hide terminal · shells keep running · ⌃` |
| Terminal の tab の自動の名前（題が無い時・{process} = 前面の process か shell・{folder} = 始めた folder の名前） | {process} · {folder} |
| Terminal の tab の hover の 2 行目（agent が居る時・{agent} = Claude Code 等・{state} = Working / Needs you / Done） | {agent} · {state} · {folder の path} |
| 左の sidebar の行の hover（{kind} = Terminal / Chat / Page / agent の名前） | {kind} · {title} |
| 窓の一番下の帯の畳んだ Terminal の tab の hover（{name} = tab の名前・押すと下の panel がその tab で開く） | {name} · Open below |
| 窓の一番下の帯の畳んだ Chat の tab の hover | Open the chat below |

| MIRROR-FRAME-1101: Page の右下の ＋（部品を足す・pointer が Page の上の時だけ見える）の AX と tooltip | Add to page |
| 同: 帯の tab の右の ＋（menu を開かず Page を 1 枚足す）の AX と tooltip | Add page · ⌘T |
| 同: 部品の menu の項目（空の Mirror の枠） | Add mirror frame |
| 同: 枠を足した後の Page の上の 1 行 | Added a mirror frame to “{Page}” · ⌘Z to undo |
| 同: 空の枠の題の帯と AX の名前 | Mirror frame |
| 同: 空の枠の真ん中の button | Choose a window |
| 同: その下の 1 行（引いて入れる道） | Or drag a window's title bar here |
| 同: 窓の一覧の字の欄（AX と placeholder） | Filter windows |
| 同: 窓の一覧の行（{App} = app の名前・題が空なら {App} だけ） | {App} · {窓の題} |
| 同: 一覧が空（絞った後 / 窓が無い） | No windows match / No open windows |
| 同: 一覧の最後の行 | Open an app… |
| 同: 開く panel の上の 1 行と決める button | Choose an app to mirror / Mirror |
| 同: 選んだ物が .app でない・bundle id が読めない | Shibubu can't tell which app this is · choose one from Applications |
| 同: 引いた窓が枠の上に来た時の帯の 1 行 | Release to mirror this window |
| 同: 窓を入れる op の bundle id が空白だけ（画面に出る道は無い・domain の拒否） | Choose an app |
| PASSWORD-1101（owner 252）: chat の password manager の card の見出し（AX の名前も） | Password manager |
| 同: 名指しした manager を繋ぐ card の題・手 | Connect {Manager} / Connect / Not now |
| 同: CLI が見つからない時の題・手 | {Manager} isn't on this Mac / Check again |
| 同: 1Password の app の連携が無い時の 1 行 | Install the 1Password app and turn on Settings › Developer › Integrate with 1Password CLI, then try again |
| 同: Bitwarden の CLI に鍵が掛かっている時の 1 行 | Bitwarden is locked · its CLI can't use Touch ID · run bw unlock in Terminal and start Shibubu from there |
| 同: 繋いでいる間の 1 行（Connect は灰） | Approve with Touch ID in 1Password |
| 同: 繋いだ後の会話の 1 行 | {Manager} is connected. Shibubu keeps only item names and reads a value only at the moment it uses it. |
| 同: chat に打った password を止めた card の題・次の 1 行 | That looks like a password. Shibubu didn't send it to the AI. / Save it in a password manager, or delete it. |
| 同: その手 | Save to {Manager} / Delete |
| 同: 保存した・消した後の 1 行 | Saved in {Manager} as “{name}” / Deleted · Shibubu didn't keep it |
| 同: chat の値を保存する項目の名前 | Password from chat · {YYYY-MM-DD HH:MM} |
| 同: Connections の Paste の欄の下の見出しと項目の行（AX の名前も） | Or choose an item from your password manager / {Manager} · {item name} |
| 同: 項目を選んだ後の 1 行 | Using {Manager} · {item name} · Shibubu reads it only when it uses it |
| 同: Connections の password manager の行の detail | Not connected · Connect signs in with Touch ID / Not connected · Connect lets Shibubu read items you choose / Connected · {n} items |
| 同: 読めなかった・sign in できなかった時 | Couldn't use {Manager} · {理由} · Connect again |
| 同: CLI が見つからない時の入れ方（card の 1 行・行の detail は `{Manager} isn't on this Mac · ` の後） | Install the 1Password app and the 1Password CLI, then turn on Settings › Developer › Integrate with 1Password CLI / Install the Bitwarden CLI (bw), then check again |
| 同: 繋いだ行の detail（確かめる前） | Connected · Shibubu reads only items you choose |
| 同: 行の Disconnect の後 | {Manager} disconnected · Shibubu won't read its items · Connect to use them again |
| 同: Paste の欄の下の項目の絞り込みの欄（AX の名前も） | Filter items |
| 同: 項目を選べなかった時 | Couldn't keep the choice in Keychain · {理由} |
| 同: 保存先の CLI が無くなっていた時 | {Manager} isn't on this Mac · Delete it, or install it and try again |
| 同: Bitwarden に保存しようとした時 | Shibubu can't save to Bitwarden · save it in the Bitwarden app |
| 同: CLI を起こせない・答えない時（`{cli}` = op・security・bw） | Couldn't start {cli} · {理由} / {cli} didn't answer · try again |

### Brought in by MIRROR-PIN-1102 (src/generated.rs `pin_window`・ENGINE の `facts`)

| JP | EN |
|---|---|
| MIRROR-PIN-1102: 同じ app の窓が Page の Mirror に 2 つ以上映っていて、どれを使うか決まらない時（Engine は何も送らない・`{App}` = 窓の一覧の app 名） | {n} {App} windows are on this page · Click the one Shibubu should use, then ask again |
| 同: 映していた窓が閉じた時（ENGINE が `facts` で出す・別の窓で代用しない） | The {App} window shown on this page has closed · Nothing was changed · Choose a window for the frame, then ask again |

### Brought in by ENGINE-AUDIT W7 (src/generated.rs `poll_engine`・src/ui/engine_view.rs `asks_every_time`・src/surface/mod.rs page bridge)

| JP | EN |
|---|---|
| ENGINE-AUDIT W7: 頼んだ形がもう在って手を 1 つも打たなかった時の 1 行（✓ でも Partial でもない・`{why}` = Engine の `EngineReply::Done.already`） | Already like that · {why} |
| 同: 押す手だけの承認の card（消す・送る・買う系・確認の窓の中 = Always allow でも毎回聞く）で Always allow の代わりに出す 1 行（`{names}` = 押す名前を “ ” で囲み「 · 」でつなぐ・Page の欄・chat の列・部品の所で同じ字） | Asks every time before pressing {names} |
| 同: 押す名前の無い押す手だけの時 | Asks every time before this press |
