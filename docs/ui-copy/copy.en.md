## 11 English copy

Style follows the DIGEST style sheet: plain, calm and specific; sentence case; buttons start with a verb and name the result; no period on buttons, tabs, labels, states or placeholders, and periods on sentences; ` · ` separates at most four segments, most important first; `…` only on commands that need more input; curly quotes around names the user chose; **project** is what Shibubu makes and **app** is a macOS app it shows; no "please", "sorry", "successfully", "not implemented", and none of the internal words (lease, revision, surface, spec, receipt, lane, portal, rail, drawer, skeleton, LLM, CLI, pid, SHA, JSON). JP is today's string. `(new)` marks a string with no JP today, `(code)` one that exists only in source, `(removed)` a string that is no longer shown. `{x}` is a value. A `{reason}` is filled only with an English string from these tables, never with an error's raw text or `Display`; the "Errors that fill {reason}" table maps each error enum. An empty state's line is drawn in `text-secondary` with no mark (§8); only Failed (red circle), Ready (green circle), Verified (green check), Running and Loading (a spinning ring) carry a painted icon before the word, never a text glyph (ICONS-1101 追記 238). The tables cover the strings on the paths this spec designs; the rest are found by the §13 check, which scans every `.rs` file compiled into the release binary (all of `src/` except `src/bin/`, `#[cfg(test)]` code, comments and named model-prompt constants) and fails the build on any CJK literal.

### Title line and switcher

| JP | EN |
|---|---|
| ○ 開発環境（現在地） | {Project name} (a red circle only when it failed) |
| Shibubu（Home へ） | Home |
| (new) location tooltip | Switch project · ⌘O |
| (new) location accessible name | Current place: {name}. Switch project |
| (new) cross-project segment | {n} needs you · “{name}” failed · “{name}” is ready · {n} running (each with its Region states mark) |
| (new) segment accessible name | Another project needs you: “{name}” |
| ← 戻る | Back to {Page} · ⌘[ |
| 提案 (⌘⇧P) | Suggestions · ⇧⌘P |
| (new) ✦ with new suggestions | Suggestions · {n} new |
| その他（•••） | (removed; items moved to menus) |
| (new) window title (Window menu, Mission Control) | {Project name} — Shibubu / Shibubu |
| (new) project rename | Rename project… · placeholder `Project name` |

### The one input and its list

| JP | EN |
|---|---|
| 作りたいことを入力 | What do you want to create? (placeholder on Home and while building) |
| 作りたいこと（accessible name） | What to create |
| この Project への変更（例: Tests Page を足して・Excalidraw を右へ） | Change this project, e.g. add a Tests page (project) |
| この Project への変更（accessible name） | Change this project |
| (new) placeholder, Runs, Connections, Legacy canvas | Search commands and projects |
| (new) scope token | Go to |
| (new) right-edge labels | ↵ Change · ↵ Open · ↵ Run · ↵ Run in terminal · on Home the primary button `Create ⌘↵` or `Set up {folder} ⌘↵`, and `↵ Open “{name}”` on an exact project name |
| (new) shell text row (K9) | Run in terminal: “{text}” |
| (new) move focus out of a terminal, website or mirror (Y4) | Move focus to Shibubu · ⌘⎋ |
| (new) idle hint | ⌘K |
| 新しいアプリを作る · ⌘K でコマンド（tooltip） | Type what to create, or press ⌘K for commands |
| 「{}」でアプリを作る | Create project: “{text}” (Home) · Create new project from “{text}” (elsewhere) |
| (new) | Change this project: “{text}” |
| 新しいアプリを作る（palette） | New project |
| 空のアプリを追加 · ⌘N | New blank canvas (opens the legacy canvas) |
| {} へ移動 | Go to Home · Go to All projects · Go to Runs · Go to Connections · Go to {Page} |
| 開く: {} | Open {name} |
| 提案を開く / 提案を閉じる | Show suggestions / Hide suggestions |
| コマンドやアプリ名を入力 | Type a command or project name |
| コマンド（見出し） | Commands |
| (new) section headings | Recent · Pages · Projects |
| 一致する項目はありません | No projects match |
| (new) | Clear terminal |
| AI が変更を読んでいます（読み終わると当てます・最長 2 分） | Reading your change · {0:12} · Stop ⌘. |
| (new) free text on Runs, Connections, Legacy canvas | No command matches · ⌘↵ to create a project from this |
| (new) second change while one is read | One change at a time. The last one is still being read. |
| (new) | Still reading. This can take up to 2 minutes. |
| (new) | Stopped. Nothing was changed. |
| 「{said}」は読めません。「<名前> Page を足して」か「<面の名前> を右へ」と打ってください | Not sure what to change. Try “{form 1}” or “{form 2}”. The two forms come from the active reader's table, for example “add a Tests page” and “move Excalidraw right” |
| (new) rescue row | Create new project from “{text}” ⌘↵ |
| AI を使えなかったので、手元の読み方で読みました（{why}） | AI wasn't available, so the change was read with simple rules · {reason} |

### Home

| JP | EN |
|---|---|
| 何を作りたいですか？ | What do you want to create? (the input's placeholder on Home; Home has no heading) |
| 例: 仕事の進捗を一か所で管理したい | (removed; the placeholder is the question) |
| 作る（⌘Enter） | Create · ⌘↵ |
| (new) footer | Choose folder… or drop one here |
| (new) first Home only | ⌘K for commands |
| (new) while dragging | Drop to use {~/path} |
| (new) folder token remove, accessible | Remove folder |
| (new) dropped folder without Git | No Git repo in this folder. Choose the repo's top folder. |
| (new) folder panel title | Choose the repo for this project |
| (new) | macOS may ask to let Shibubu use your Documents folder. |
| (new) examples line label | Try |
| タイマー / 在庫管理 / 研究ノート（chips） | Dev environment for a repo · Inventory tracker · Research notes |
| タイマーを作って / 在庫を管理したい / 研究ノートを作りたい | Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. · Track my inventory · Keep research notes |
| ◐ 一部のみ表示 · LLM の CLI が使えません（使わない設定です）。AI 無しの手元の推定で作ります。 | (moved to Connections › AI: `AI · Off · Projects are built with simple rules`) |
| (new) AI notice, until AI is first used | AI reads your request with your Claude Code account · Use simple rules (on the AI-check line after Enter: · Send · Use simple rules) |
| (new) folder panel cancelled | No folder chosen · Create without one ⌘↵ |
| (new) token-only Enter | Set up {folder} |
| アプリ（見出し） | Continue |
| {n} 件 | All projects · {n} |
| まだアプリはありません | (removed; the field is the action) |
| 目的は未記入 | No purpose set |
| 名前や目的で絞り込む / 絞り込み | (removed; the one input filters) |
| 絞り込みに一致するアプリはありません | No projects match |
| {n} 個の操作はまだ使えません（Apps の行） | ◐ Partly shown · {n} actions not set up |
| (new) | Building stopped when Shibubu quit · ⌘↵ to try again |
| この Project は見つかりません。Apps から開き直してください | This project couldn't be found · Go to Home (a moved folder is not this case: the project opens and its regions say `× Folder not found · Choose folder…`) |
| (new) | One project at a time. “{name}” is still building. |
| (new) Continue row, folder moved | × Folder missing |
| 作りたい物を 1 行で書いてください | Describe what to create in one line |

### Building

| JP | EN |
|---|---|
| (new) title line | ▶ Building · {elapsed} (the step words appear only in the step list) |
| 読んでいます | Reading your request |
| 調べています | Checking what's on this Mac |
| 組んでいます | Putting the pages together |
| 確かめています | Running the checks |
| 直しています | Fixing {n} failed checks |
| キャンセル | Stop · ⌘. |
| 停止は未実装です（実行中のレーンは最後まで進みます） | (removed; Cancel works) |
| AI が一言を読んでいます（作り終わると開きます・最長 2 分・待つ間も画面は使えます） | (removed; the segment and Continue say when it is ready) |
| AI の CLI を確かめています（終わったら一言を読みます・最長 30 秒）(`src/app.rs:1097`) | Checking for AI on this Mac · {0:04} · Use simple rules now |
| AI の CLI を確かめ終わる前だったので、手元の推定で作りました…(`src/generated.rs:439`) | Built with simple rules · the AI check didn't finish |
| (new) while the AI process ends | Stopping the AI… |
| (new) cancelled | Stopped. Nothing was added to Projects. |
| (new) receipt | Built {n} pages · with simple rules · Set up AI (AI off; `Set up AI` opens Connections › AI, O9) |
| (new) first AI project only | Built {n} pages · AI can make mistakes |
| AI を使えなかったので、手元の推定で作りました（{why}） | Built with simple rules because AI wasn't available · {reason} |
| 作れませんでした | Couldn't build it |
| 通らない所が残りました | Some checks still fail |
| 時間内に通りませんでした | Didn't pass in time |
| 直しても良くなりませんでした | Fixes didn't help |
| 確かめる前に時間が切れました | Ran out of time before checking |
| 何を作るか読めませんでした | Couldn't tell what to build |
| 目的が読めませんでした。何を記録・管理・計測したいかを 1 行で書いてください | Couldn't tell what to build. Say in one line what you want to track, manage or measure. |
| 今ある app への変更に読めました。作りたい物を「〜を作って」の形で書いてください | That reads like a change to an existing project. To create one, start with “Make…”. |
| 作った画面が検査に落ちました（{}）。言い方を変えてもう一度 | The built screen failed a check · {reason} · Say it another way |
| AI を使えませんでした（{why}）。言い方を変えてもう一度 | AI wasn't available · {reason} · Say it another way |
| 作った app を保存できませんでした（{e:?}） | Couldn't save the project · {reason} |
| 言い方を変える | Say it another way |
| もう一度作る | Try again |
| この接続を許可する | Allow this connection |
| 元の app で開く | Open in {App} |
| 説明文が残っています | Leftover description text |
| 読み込み中の表示が無い部品があります | A part has no loading state |
| 試しに押した操作が失敗しました | A test press failed |
| 頼まれていない外への操作 | An outward action you didn't ask for |
| 頼まれていない接続先 | A connection you didn't ask for |
| 頼まれていない部品 | A part you didn't ask for |
| 注意: | Note: |
| 口が無い / この Mac の中 / web で開く / 元の app の横に並べる / 元の app の画面を映す / 他の人のコードを隔離して使う / ほかの口 | Not reachable / On this Mac / On the web / Beside {App} / Mirrored / Code from a repo, isolated / Other |
| · 代わり: | · Instead: |

### Tab row and menus

| JP | EN |
|---|---|
| Dev / Docs / Diagram / Tests（tab） | Dev / Docs / Diagram / Tests (named by the user) |
| (new) overflow tab | More |
| (new) tab with state, accessible | {Page}, needs approval · {Page}, failed · {Page}, running |
| (new) ＋ tooltip | Add · ⌘T for a page |
| ＋ Page | Add page |
| ＋ Terminal | Add terminal |
| ＋ Browser | Add browser |
| ↶ 元に戻す（⌘Z） | Undo {action} · ⌘Z |
| ↷ やり直す（⇧⌘Z） | Redo {action} · ⇧⌘Z |
| (new) ••• tooltip | Page options |
| (new) | Rename… |
| (new) rename placeholder | Page name |
| (new) blank page name | Enter a page name |
| → 右へ | Move right |
| (new) | Move left |
| 配置: {shown} | Layout |
| 開発環境 / 音楽制作 / タスク管理 / なし / (new G88a) AI が決めた置き場 | Development / Music / Tasks / None / Suggested |
| (new G88b) 録音の帯: 録る / mic が off / 設定を開く / 止める / 試聴 / take を入れる / 入れる先 / 先に開く / Finder で見る / 捨てる | Record humming / Microphone access is off / Open System Settings / Stop / Play / Add take to REAPER / Goes to REAPER's cursor on the selected track / Open REAPER first / Reveal in Finder / Discard |
| ↻ 読み直す | Refresh · ⌘R |
| Terminal の引き出し（⌃`） | Show terminal · ⌃` (G92: shows this page's terminal, adds one if none) |
| この Page を消す | Delete page… |
| (new) inert menu item | A project needs at least one page |
| 「{}」を消します。この Page で打ちかけた文字も消えます | Delete “{Page}”? ⌘Z brings it back. |
| 消す / やめる | Delete / Keep |
| (new) | Couldn't delete “{Page}”. Nothing was changed. |
| この Page は消えました。/ 先頭の Page へ | This page was deleted. / Go to first page |
| この Page はまだ空です。上のタブで別の Page へ移るか、「＋ Page」で足せます | Nothing here yet · Add terminal · Add browser |
| この Page に描ける app がありません。上のタブで別の Page へ移れます | Nothing to show on this page |

### Receipts and change results

| JP | EN |
|---|---|
| (new) | Added page “{Page}” · ⌘Z to undo |
| (new) | Moved “{Page}” right · Moved “{Page}” left |
| (new) | Moved {Excalidraw} to the right of {Terminal} |
| (new) | Added a terminal to “{Page}” · Added a browser to “{Page}” |
| (new) | Renamed “{old}” to “{new}” |
| (new) | Layout: {Music} |
| (new) | Deleted page “{Page}” · ⌘Z to undo |
| (new) | Undid: {action} · Files unchanged · ⇧⌘Z to redo · Undid: {action} · Changes inside {App} stay · ⇧⌘Z to redo ({action} is “the last change” when the page list didn't change) |
| (new) | Undid: {action} · {docs/arch.excalidraw} stays as saved |
| (new) | Redid: {action} |
| 変更の履歴を読めません（後の版か壊れた保存です）。元に戻す・やり直すは使えません。編集はこのまま続けられます | Undo is off · history unreadable |
| Project を保存できませんでした（{e}）。もう一度押してください | Couldn't undo. Nothing was changed. · Try again |
| この Project は別の所で更新されました（今は revision {}）。開き直してから押してください | This project changed somewhere else. Reopen it, then try again. |
| {what} は既に在ります | There's already {what}. |
| (new) | “{Page}” is already the last page. |
| (new) | Duplicated “{name}” |
| (new) | GitHub is now verified |
| (new) | “{name}” is ready · Open |
| (new) return summary | While you were away: {n} runs finished · Open Runs (who needs you is in the segment) |
| (new) project renamed | Renamed project to “{name}” |
| (new) folder set (E13) | Using {~/code/my-app} |
| (new) lane stopped | Stopped after {n} of {m} steps. The finished steps stay. |
| (new) project deleted | Deleted project “{name}” |
| (new) address copied | Copied the address |
| (new) relaunch after quitting mid-run | Shibubu quit during {n} run · “{action}” may not have been sent · Open Runs |
| (new) ⌘Q while a run sends (replaces the input) | {n} run is still sending · Quit anyway ⌘Q · Cancel |
| (new) a page edit while a change is read | One change at a time. The last one is still being read. |
| 配置の型を保存できませんでした（{e}） | Couldn't save the layout · {reason} |
| 配置の型を読めません（{e}）。今の並びで描きます | Couldn't read the layout, so pages are shown in their current order |
| 入力途中の状態を保存できませんでした（{e}） | Couldn't keep what you were typing on this page · {reason} |
| この Page の入力途中の状態を読めません（後の版か壊れた保存です）。空の状態で開きました | Couldn't restore what you were typing on this page. It opened empty. |
| 保存エラー: {e}（app.rs） | Couldn't save your changes · {reason} (danger, in the receipt slot until the next save works) |

### Terminal and Git

| JP | EN |
|---|---|
| bash · pid 26918 · ~/shibubu-yc-demo | {shell} · {~/code/my-app} (tooltip: Process {26918}) |
| (new) first focus | Keys go to Terminal · ⌘⎋ to leave |
| (new) while focused | ⌘⎋ to leave |
| (new) | Shell restarted · earlier output not kept |
| 前の起動の Terminal を同じ画面のまま起こし直せなかった（右上の toast と tab の本体・{e} = 理由・TERM-RESTORE-1101） | Couldn't reopen the terminal ({e}) |
| Terminal を起こしています | Starting the shell |
| (new) | No folder, started in your home folder |
| (new) Tests output region title | Output · {command} · Shibubu is running this (while it runs) |
| 終わりました（終了 code {code}）。新しく起こせます / 終わりました（{sig}）。… / 終わりました。… | Shell exited (code {n}) · Restart / Shell stopped ({signal}) · Restart / Shell exited · Restart |
| (new) | Couldn't start the shell · {reason} · Try again |
| この Terminal は Project にありません。Page の「＋ Terminal」で置き直してください | This terminal is no longer in the project · Add terminal |
| この画面には Terminal がありません | No terminal on this page |
| この Project には Terminal の場所（repo）がありません。一言に repo の場所を足して作り直してください | This project has no folder for the terminal · Choose folder… |
| この Project の Terminal はこの版ではまだ開けません。今は Mac の Terminal で {} を開いてください | This version can't open a terminal here · Open in Terminal |
| Terminal の記録を保存できませんでした（{e}）。閉じると記録が残りません | Couldn't save the terminal's history · It will be lost when you close it |
| Terminal の引き出しの開閉を保存できませんでした（{e}） | Couldn't remember whether the terminal panel was open · {reason} |
| Terminal の引き出しの開閉を読めません（後の版か壊れた保存です）。閉じた状態で開きました | Couldn't restore the terminal panel, so it opened closed |
| Git の欄: branch・SHA・変更した file | {branch} · {commit} · {n} changed / clean |
| (new) | Loading Git status |
| (new) | … Checking for changes |
| (new) | {n} new commits since {yesterday} |
| (new) | No changes |
| (new) | This folder isn't a Git repo |
| (new) | No folder · Choose folder… |
| (new) | Folder not found · Choose folder… |
| 読めていません | Not read yet |
| この版では {} の状態を読めません | This version can't read {name} |
| {} の状態を保存できませんでした（{e}）。「↻ 読み直す」で読み直せます | Couldn't keep {name}'s status · Refresh |
| (new) | Couldn't read Git · {reason} · Try again |

### Mirror (TextEdit) strip

| JP | EN |
|---|---|
| 記号 + app 名 · 状態語 · Manual / Mirror / External · 同期の確度 | [app icon] {file} · {App} · {who acts} (state only when not Ready; mode and sync in the tooltip) |
| (new) who acts | You are typing in {App} · Shibubu is saving · Paused |
| (new) tooltip of who acts | Typing, ⌘S and single clicks reach {App}. For anything else, open {App}. (+ `{App} saves as you type.` once the §13 autosave probe confirms it) |
| (new) verified save | ✓ Saved · checked in {App} · {14:01} |
| {app} の窓を映して、ここでの入力を {app} へ送ります。よければ「映す」を押してください（押すまで何も映さず何も送りません） | Mirror shows {App}'s window here and sends your typing to it. macOS will ask for screen recording. |
| 映す | Mirror here |
| 外で開く | Open in {App} |
| (new) mode menu | Mirror here · Open in its own window · Stop mirroring |
| 見つかりません | Not found |
| 許可待ち | Needs permission |
| 接続中 | Attaching |
| 直接操作中 | (removed with Native Direct) |
| 映像で操作中 / {} の窓を映しています | Mirroring |
| 閲覧専用・編集不可 | View only |
| 終了しています | Not available |
| 同期が遅れています | Behind · checked {time} |
| 一体化に失敗しました | Couldn't attach |
| ライブ画面 / 状態連携中 / 定期確認 / 推測・未検証 / 情報が古い可能性 / 状態取得非対応 | Live view / Live state / Checked periodically / Guessed / May be out of date / Can't read its state |
| 入力: クリックとキー | Input: clicks and keys |
| 再探索 | Find again |
| 許可を開く | Open settings |
| キャンセル（接続中） | Cancel |
| (new) after cancelling an attach | Cancelled. Nothing was mirrored. |
| 切り離す | Stop mirroring |
| 元の窓を開く | Open {App} |
| 再試行 | Try again |
| 更新確認 | Check now |
| 元の app で続ける | Open in {App} |
| Kangae に戻す（重ね直す） | (removed with Native Direct) |
| (new) external body, only after the open succeeded | {file} is open in {App}. |
| (new) first visit, not yet opened or checked | {file} · {App} · Open in {App} |
| (new) | Show {App} |
| (new) | Open {App} |
| (new) | Keys go to {App} · ⌘⎋ to leave |
| 画面収録の許可が要ります。システム設定 → プライバシーとセキュリティ → 画面収録 で Shibubu を許可してください（それまで何も映さず何も送りません） | ⊘ Screen recording is off · Open settings (then `Check again`) |
| アクセシビリティの許可が要ります。…（それまで文字・key・click は送りません） | ⊘ Accessibility is off · nothing is sent · Open settings |
| {app} に保存させる許可が要ります。…オートメーション…（それまで ⌘S は送りません） | ⊘ Can't ask {App} to save · ⌘S isn't sent · Open settings |
| {what}は後ろの {app} に届きません。{app} を前に出して操作してください（ここからは送りませんでした） | {What} doesn't reach {App} from here. Nothing was sent. · Open {App}, where {What} is Scrolling, Dragging, This action or a ⌘ key other than ⌘S (`⌘Z doesn't reach TextEdit from here. Nothing was sent.`) |
| (new) the app answered the save with an error | × {App} didn't save · {its reason} · Open {App} |
| {app} が click に答えませんでした（効いたかは分かりません）。{app} を前に出して確かめてください | {App} didn't answer the click. Check it in {App}. · Open {App} |
| 元の窓が閉じました。元の app で窓を開き直すと、ここにまた映ります | {App}'s window closed · Open {file} |
| {app} で {t} の窓が開いていません。{app} で {t} を開くと、ここに映ります | {file} isn't open in {App} · Open {file} |
| {app} の窓が開いていません。{app} で窓を開くと、ここに映ります | {App} has no open window · Open {App} |
| {name} の窓が見つかりません。{name} で開いてから Page を開き直してください | Couldn't find {App}'s window · Find again |
| (new G87) Shibubu が {app} を背景で起こし、窓が出るのを待っている | … Starting {App}… |
| (new G87) {app} がこの Mac に入っていない（Shibubu は何も入れない） | ○ {App} isn't installed on this Mac · Find again |
| 映すための部品が入っていません（この版では映せません） / この版の Shibubu には {name} の窓を映す部品が入っていません。今は {name} で直接開いて使えます | This version can't mirror apps · Open in {App} |
| この版の Shibubu は {name} の窓を Page に重ねません。今は {name} で直接開いて使えます | (removed with Native Direct; `Open in {App}` is the route) |
| Mirror の helper が動きません: {e} | Mirroring stopped working · {reason} · Try again |
| 入力を送りませんでした（口が違います: {reason}） | Nothing was sent · {reason} |
| 入力の記録を書けないので送りませんでした: {e} | Nothing was sent, because the input log couldn't be written · {reason} |
| 映す同意を保存できませんでした（{e}）。再起動するともう一度聞きます | Couldn't remember your Mirror choice. Shibubu asks again after a restart. |
| この面は Mirror で映せません（Mirror は app の窓だけ） | Only app windows can be mirrored |
| (new) screen access lapsed | Screen recording stopped · Check again |
| (new) macOS re-asking | macOS is asking about screen recording again · Open settings |
| (new) the folder has no README.md | This folder has no README.md · Create README.md |
| (new) create approval | ▣ Create {~/path}/README.md · New file · Create · Cancel |
| (new) | Behind · checked {14:02} |

### Website (Excalidraw) and browser

| JP | EN |
|---|---|
| (new) strip | Excalidraw · excalidraw.com |
| (new) save hint, until the first save | To save into the repo: menu › Save to… › Save to file · Choose file… |
| 読み込み中 … {host} | Loading {host} |
| excalidraw.com はまだ確かめていません。今は browser で直接開いて使えます | (Ready; strip tooltip `Not checked by Shibubu`) · when the site can't be shown: ⊘ Can't show {host} here · Open in browser |
| {name} が在るかをまだ確かめていません。一言を送り直すと確かめます | {name} hasn't been checked · Check again (re-reads this project; never a new project) |
| この版の Shibubu には Web の面が入っていません。今は browser で {name} を直接開いて使えます | This version can't show websites here · Open in browser |
| browser で開く | Open in browser |
| この Page に {host} を置けませんでした（{why}） | Couldn't open {host} here · {reason} |
| もう一度 | Try again |
| (new) | Couldn't load {site} · Try again |
| 保存の欄（web-save-name） | ▣ Save to {~/path/docs/} [{name}] · New file in docs/ / Replaces the file that is there |
| 保存（web-save） | Save |
| (new) | Saving to {docs/name} |
| repo の {shown} に置きました（git status で見られます） | ✓ Saved · checked on disk · {14:02} · Git on {Dev} updated → Saved {14:02} |
| (new) no read-back | Saved, not checked |
| (new) | × Couldn't save · {reason} · Try again (`Nothing was saved` added only after checking docs/) |
| (new) | Cancelled. Nothing was saved. |
| (new) | Didn't receive the file · Choose file… |
| URL 欄 | Search or enter website |
| 前 / 次 / 読み直し | Back / Forward / Reload |
| (new) strip context menu | Open in browser · Reload · Copy address (dragging is shown by the open-hand cursor, no tooltip) |

### Tests

| JP | EN |
|---|---|
| (new) strip | Tests · {command} ⌄ |
| 走らせる | Run tests |
| 走らせています（{command}）。結果はこの下に出ます | Running {command} · {elapsed} |
| 完了 / 成功 | Passed · {time} · {duration} |
| 終了 code {} | Failed · {n} failed · exit {code} |
| まだ走らせた test はありません。「走らせる」を押すと、結果がここに出ます | No runs yet |
| Human の lease の間は拒否 | (removed; tests run in Shibubu's own terminal, so your typing never holds them) |
| (new) output region | Output · {command} |
| (new) timeout | {command} didn't finish in {n} s · Run again |
| (new) exit 127 | × {tool} isn't installed · Go to terminal |
| (new) | No test command found · Go to terminal |
| この Project には Terminal がありません。＋ Terminal で置いてから走らせてください | (removed; tests need only the folder: This project has no folder · Choose folder…) |
| 走らせる command が空です。command を書いてから走らせてください | No test command set |

### Approvals and outward actions

| JP | EN |
|---|---|
| 承認して送る | Approve and send |
| 実行前の確認（window title） | (removed; the approval is a line on the region) |
| 対象と操作を確認してください。ワークフロー途中で失敗するとそこで停止します。 | If a step fails, the rest stops. |
| 外部変更あり / 読み取り | Changes things outside Shibubu / Reads only |
| PID {pid} / Window {window_id} / AX: {label} | {App} window · {control} (process number in the tooltip) |
| 入力: {text} | Types “{text}” |
| キャンセル / 確認して実行 | Cancel / Approve and run |
| 承認待ちです（この画面ではまだ承認できません） | ▣ {action} · {consequence} · Approve and {verb} · Cancel |
| (new) consequence segments | New file in docs/ · Replaces the file that is there · Sends to GitHub · Changes things outside Shibubu |
| 内容を承認して適用 | Approve and apply |
| 書いたが確かめられませんでした: {reason} | Saved, not checked · {reason} |
| 断られました: {reason} | Declined · {reason} |
| 失敗しました（{}） | Failed · {reason} |
| 届いたか分かりません。Runs で確かめてください: {reason} | Can't tell if it arrived · {reason} · Open Runs |
| 実行できませんでした（{}） | Couldn't run it · {reason} |
| (new) | Stopped before sending. Nothing left this Mac. |
| (new) partial | Stopped after saving {README.md}. {The diagram} wasn't saved. |
| (new) | Stopped. Couldn't check what changed. Open Runs to see. |

### Suggestions panel

| JP | EN |
|---|---|
| 次にできること | Suggestions |
| このアプリについて | About |
| 実行履歴 | Runs |
| (new) About row | What you asked for |
| (new) | All runs |
| 今のところ新しい提案はありません | No new suggestions |
| 提案を検討中 | Looking for suggestions |
| プレビュー / 適用 / 後で / 非表示 | Preview / Apply / Later / Hide |
| プレビュー: {} | Preview: {title} |
| 変更前 / 変更後 / 変更前を見る / 変更後を見る | Before / After / Show before / Show after |
| 根拠 / 変更点 / 必要権限 / 取り消し | Why / What changes / Needs access to / How to undo |
| 根拠はありません（表示しない提案） | (removed; such suggestions are not shown) |
| 構成は変えません / なし | Pages stay as they are / None |
| アクション / 影響 / リスク | Action / Effect / Risk |
| （空） | (empty) |
| 取り消す | Undo |
| 閉じる | Close |
| 最近の適用 / 詳細 / 信頼度 | Recently applied / Details / Confidence |
| {n} 件に戻す / すべて見る（ほか {n} 件） | Show fewer / Show {n} more |
| 提案を閉じる (Esc) | Close suggestions · Esc |
| 実行履歴はまだありません。 | No runs yet |
| Agent に頼む / このアプリをどう変えますか？ | (removed; the rail input is gone, ask in the one input) |
| 提案 engine は未実装です。入力は記録していません。 | (removed) |
| (new) | Applied “{title}” · ⌘Z to undo |
| (new) | Couldn't load suggestions · Try again |

### Runs

| JP | EN |
|---|---|
| (new) | While you were away: {n} runs finished |
| 何をしたか · app · 何分前 · 結果の 1 語 | {Did what} · {project} · {5 min ago} · {result} |
| 何をしたか / 相手 / 結果 / 根拠 / 取り消し | What / With / Result / What was checked / Undo |
| 未記録 | Not recorded |
| {} の詳細 | Details of {what} |
| 待機中 / 準備中 / 接続待ち / 承認待ち | Queued / Preparing / Not connected / Needs approval |
| 実行中 / 確かめ中 | Running / Checking |
| 確かめた / 確かめられていない | Verified / Not verified |
| 断られた / 失敗した / 入ったか分からない / 取り消した | Declined / Failed / Sent, not confirmed / Undone |
| 読み戻して一致 / 読み戻せていない | Read back and matched / Couldn't read it back |
| 相手が受け付けなかった / 届いたか分からない | {App} didn't accept it / Can't tell if it arrived |
| 準備ができていない / 許可が無い / 承認されなかった | Not set up yet / Needs permission / Not approved |
| 入力が正しくない / 接続が切れた / 時間内に終わらなかった / 回数の上限に達した | Input not accepted / Connection lost / Took too long / Limit reached |
| 相手が見つからない / 相手が決まらない / 操作を断られた / 確かめに失敗した / 内部の不具合 | Couldn't find {App} / Couldn't tell which {App} / {App} refused the action / The check failed / Something went wrong in Shibubu |
| 変更なし / この Mac で戻せる / 相手の側で戻す / 取り消せません / 戻せるか未確認 | Nothing changed / Can be undone on this Mac / Undo it in {App} / Can't be undone / Not sure it can be undone |
| 止める | Stop |
| 止めました（副作用の前） | Stopped before anything changed. |
| 止めるよう伝えました（間に合わなければ結果のまま残ります） | Asked it to stop. If it was too late, the result stays. |
| もう止められません（確かめる段まで進んでいます） | Too late to stop. It's already checking the result. |
| 止められませんでした（{}） | Couldn't stop it · {reason} |
| 接続する | Connect |
| {n} レーン · {current}（execution strip） | {n} running (location segment) · {current} (Runs row) |
| 停止（execution strip） | Stop |
| 詳細（execution strip） | (tooltip) Open Runs |
| (new) | Couldn't read runs · Try again |
| たった今 / {} 分前 / {} 時間前 / きのう / {} 日前 | Just now / {n} min ago / {n} h ago / Yesterday / {Oct 23} |

### Connections

| JP | EN |
|---|---|
| 接続 | Connections |
| GitHub・画面の操作（Shibubu Bridge）・Web JSON・信頼済 CLI | GitHub · Screen control · Web data · Trusted command-line tools |
| 使える → 認証済み → 権限と疎通 OK → 操作の証拠あり | Available → Connected → Ready → Verified |
| (meaning lines, per row; §5.9 table) | Token set · not checked · No token · public repos only · Not running, or Accessibility is off · Address set · {host} · not checked · No address set · No commands registered · {n} commands registered · Worked once, with proof · {when} |
| GITHUB_TOKEN あり（有効かは未確認・値は出さない）。書き込みはこの token で (`src/app.rs:415`) | Token set · not checked |
| GITHUB_TOKEN なし。公開 repo の読み取りだけ可能 | No token · public repos only |
| 未確認（開いた時に 1 回だけ確かめます）(`app.rs:417`) | Checked when you open Connections |
| 読み取り先: {host} / KANGAE_WEB_SERVICE_URL 未設定（https のみ）(`web_detail`, `app.rs:1012-1013`) | Address set · {host} · not checked / No address set |
| 取得済み {} 件 · CLI 登録 {enabled} 件（人が絶対パスを明示した物だけ実行）(`app.rs:427`) | {n} commands registered / No commands registered |
| MCP: {name} (`src/mcp.rs`) | MCP: {name} |
| コマンド: {cmd}（引数は出さない・RQ1 反証の直し）(`mcp.rs:120`) | Command: {cmd} |
| 未検出: {cmd}（実行ファイルが見つかりません）(`mcp.rs:120`) | ⊘ Command not found: {cmd} |
| (new) AI row | AI · Off · Projects are built with simple rules · Ready · Reads your requests with your Claude Code account · Use simple rules · Use AI |
| claude が見つかりません。Claude Code を入れて、Terminal で claude auth login を打ってください (`src/llm_cli.rs:318`) | Install Claude Code, then run claude auth login in Terminal |
| (new) | Checked {2 min ago} |
| 再確認 | Check again |
| 確認中… / 確認中 | Checking |
| Shibubu Bridge が動いていて、アクセシビリティの許可があります（押した結果は未検証） | Running, with Accessibility · clicks not verified yet |
| Shibubu Bridge を起こせないかアクセシビリティの許可が無いので…「再確認」 | Screen control didn't start, or Accessibility is off · Open settings |
| (new) | Needs Accessibility · System Settings › Privacy & Security › Accessibility · Open settings |
| (new) | Check failed · {reason} |
| bridge が起きていないので、app が在るかを確かめていません | Screen control isn't running, so apps weren't checked |
| 「{what}」はまだ使えません。取得元を繋ぐと使えます | “{what}” needs a connection · Connect |

### Permissions (introductions)

| JP | EN |
|---|---|
| (new) Screen Recording | Mirror shows {App}'s window here and sends your typing to it. macOS will ask for screen recording. |
| (new) Accessibility | Typing into {App} from here needs Accessibility, and macOS will ask for it. |
| (new) Automation | macOS will ask before Shibubu saves in {App}. |
| (new) Screen Recording, while a terminal runs a command | macOS may restart Shibubu. Running terminal commands will stop. |
| (new) Files | macOS may ask to let Shibubu use your Documents folder. |
| (new) buttons | Ask macOS · Mirror here · Open in {App} · Open {App} · Cancel · Open settings · Check again |
| (new) screen access lapsed | Screen recording stopped · Check again |
| Info.plist Automation purpose, `NSAppleEventsUsageDescription` (Japanese only today, `build-mac.sh:129`) | Shibubu asks the original app to save the document you are mirroring when you press ⌘S. (Screen Recording and Accessibility have no purpose string of their own; macOS shows its fixed text, cutoff knowledge.) |

### Region states

| JP | EN (mark · word; the word is shown only when not Ready) |
|---|---|
| 読み込み中 | (spinning ring) Loading {thing} |
| まだデータがありません | Nothing here yet |
| 一部のみ表示 | Partly shown |
| 古い情報の可能性があります | May be out of date |
| 接続が必要 | Not connected |
| 確認待ち | Needs approval |
| 失敗しました | (red circle) Failed |
| 準備完了 | (green circle, word not shown) Ready |
| 実行中 | (spinning ring) Running |
| 検証済み | (green check) Verified |
| (new) Page の頭 | (removed: the page name heading and the project's one-line goal are not drawn inside a Page; the tab names the page. A web surface's head is one row: its name in people's words, e.g. `Excalidraw`, then its state line. A Mirror surface has no heading: the frame's title `{App} · {file}` names it) |
| (new) 言う事の無い Page の状態 | (nothing: a project's shell app has no state line; `Partly shown` is drawn only with what is missing and the next step after ` · `, `renderer/mod.rs` `render_with`) |
| この Page を開くと確かめます | Checked when you open this page |
| Terminal を確かめる準備ができませんでした（{e}）。開き直すと確かめ直します | Couldn't check the terminal · Reopen the page to try again |
| 面の確かめを保存できませんでした / 面の確かめを読めません（{e}） | Couldn't keep this page's check · {reason} / Couldn't read this page's check · {reason} |
| 保存した Project を読めません（{e}）。もう一度開いてください / この版の Shibubu では開けない形です | Couldn't open the saved project · {reason} · Try again / This version can't open it |

### Project actions, appearance, menu bar

| JP | EN |
|---|---|
| テーマ | Appearance |
| システムに合わせる / ライト / ダーク | Match system / Light / Dark |
| このアプリを複製 | Duplicate canvas |
| 削除（確認つき） | Delete canvas… → Delete “{name}”? Its parts and history are deleted too. · Delete / Cancel |
| このアプリは削除されました。/ Apps へ | This project was deleted. / Go to Home |
| このアプリは見つかりません。 | This project couldn't be found. |
| ＋ 新しいアプリ / 新しいアプリ (⌘N) | New project · ⌘N |
| アプリはまだありません（sidebar） | (removed with the sidebar) |
| 操作ボタンには実際の実行先が必要…（sidebar 最下行） | (removed; the fact is in Connections › Screen control) |
| (new, USAGE-1) 使った記録をコピー（⌘K）/ コピーした | Copy usage report / Usage report copied |
| (new) menu bar: Shibubu | About Shibubu · Appearance ▸ · Quit Shibubu |
| (new) menu bar: File | New project ⌘N · New blank canvas · Open project… ⌘O · Rename project… · Choose folder… · Delete project… |
| (new) menu bar: Edit | Undo {action} ⌘Z · Redo {action} ⇧⌘Z · Cut · Copy · Paste · Select All (while a website has focus, Undo and Redo are the site's own) |
| (new) menu bar: View | Command menu ⌘K · Show suggestions ⇧⌘P · Show terminal ⌃` · Move focus to Shibubu ⌘⎋ |
| (new) delete project confirm | Delete “{name}”? The folder {~/path} stays on disk. · Delete / Cancel |
| (new) menu bar: Go | Back ⌘[ · Home ⇧⌘H · Next page ⇧⌘] · Previous page ⇧⌘[ · Runs · Connections |
| (new) menu bar: Page | Add page ⌘T · Add terminal · Add browser · Rename… · Move left · Move right · Layout ▸ · Refresh ⌘R · Delete page… |
| (new) menu bar: Window, Help | (system items) |

### Generated pages, status lines and errors that fill {reason}

| JP | EN |
|---|---|
| 送信 / 保存 / 削除する / 実行（default submit labels, `src/ui/renderer/components.rs:456-459`） | Send / Save / Delete / Run |
| 未入力（field hint, `components.rs:537`） | (no placeholder; the field's label names it) |
| 選ぶ（choice, `components.rs:558`） | Choose |
| 拡張「{name}」はこの版では表示できません（`components.rs:53`） | ◐ This version can't show “{name}” |
| 未対応の部品: {other}（`components.rs:55`） | ◐ This version can't show one part ({other}) |
| 接続済み・確認中（`src/ui/renderer/bindings.rs:86`） | … Checking the connection |
| 取得元の一部が使えません | ◔ Some sources aren't available |
| このデータの取得元には対応していません | × This data source isn't supported |
| データの取得元「{id}」が定義にありません | × The data source “{id}” is missing from the project |
| このアプリは実行が止められています（`bindings.rs:122`） | Actions are turned off for this project |
| 操作がまだ割り当てられていません（`bindings.rs:125`） | No action is set for this button |
| {operation}（{verified_at}）（`src/ui/renderer/mod.rs:505`） | ✓ {operation} · {time} |
| {name} はまだ確かめていません。今は browser で直接開いて使えます（`mod.rs:506`） | (Ready; tooltip `Not checked by Shibubu` · menu `Check again`) |
| {name} はこの Mac に在りますが、Shibubu にアクセシビリティの許可が無いので…（`mod.rs:508`） | ⊘ Accessibility is off for Shibubu · Open settings |
| {name} はこの Mac に在ります。今は {name} で直接開いて使えます…（`mod.rs:509`） | (Ready; tooltip `Not checked by Shibubu` · menu `Check again`) |
| {name} がこの Mac に入っていません。入れてから一言を送り直すと…（`mod.rs:510`） | ⊘ {name} isn't installed on this Mac · Check again |
| {name} はこの Page ではまだ使えません。今は browser で直接開いて使えます（`mod.rs:511`） | ⊘ Can't show {name} here · Open in browser |
| {name} が在るかを確かめられませんでした。一言を送り直すと…（`mod.rs:512`） | ⊘ Couldn't find {name} · Check again |
| {name} は終了しています。開き直すと戻ります（`mod.rs:513`） | ⊘ {name} isn't running · Open {name} |
| 名前が空です。保存する名前を入れてください（`src/project/web.rs:150`, `SaveError::EmptyName`） | Enter a file name |
| 「{n}」は使えない名前です。/ や .. を含まない名前にしてください（`BadName`） | “{n}” can't be a file name · no / or .. |
| repo の場所が渡されていません。一言に repo の場所を足して作り直してください（`NoWorkspace`） | This project has no folder · Choose folder… |
| 書き出した file がまだ届いていません。Excalidraw でもう一度書き出してください（`NotStaged`） | The file from Excalidraw hasn't arrived · Choose file… |
| repo に書けませんでした（{e}）。空きと権限を確かめてください（`Io`） | Couldn't write to the repo · check free space and permissions |
| {n} 秒たっても「{command}」の結果が返りませんでした…（`src/generated.rs:205`） | {command} didn't finish in {n} s · Run again |
| 「{command}」の途中で Terminal が終わったので、結果を取れませんでした…（`generated.rs:206`） | The run ended before {command} finished · Run again |
| 「{command}」の結果を取れませんでした。Terminal を確かめてから…（`generated.rs:208`） | Couldn't get the result of {command} · Run again |
| 開発環境（project name, `src/composer/project.rs:563, 627`） | Dev environment |
| repo を Dev・Docs・Diagram の Page で開く（purpose） | Open the repo in Dev, Docs and Diagram pages |
| repo: {path}（`dev-repo`, `project.rs:590`） | (removed; the terminal title shows the folder) |
| repo の場所を Project に入れました。上の「＋ Terminal」で…（`dev-next`） | (removed; the page has its terminal) |
| Git（`dev-git-title`） | (removed; the Git region's title line names it) |
| branch に居ません（detached HEAD） | Not on a branch |
| まだ commit がありません | No commits yet |
| 変更した file はありません | No changes |
| repo の場所が渡されていません。一言に repo の場所（/ から始まる path）を…（no-folder `dev-next`, `project.rs:612`） | (removed; the terminal starts in the home folder and Git says `No folder · Choose folder…`) |
| 名前 / 数量 / 更新日時（inventory fields, `src/composer/goal.rs:657`） | Name / Quantity / Updated |
| 「走らせる」で走らせた結果がここに出ます（`project.rs:766`） | (removed; `No runs yet`) |

### Errors that fill {reason}

Each error enum that reaches a `{reason}` gets one English row per variant, keyed by meaning; the inner technical text (`{e}`, `{why}`, an io error) goes to the log, never to the screen. The §13 check is expected to fail when its scope is widened: every remaining CJK literal is either mapped here or shown to be off screen (`// not on screen: {why}`, allowlisted by line).

| JP (source) | EN `{reason}` |
|---|---|
| `StoreError::Sqlite` `sqlite: {e}` (`src/storage/sqlite.rs:18-40`) | the project database couldn't be read or written |
| `StoreError::Json` `json: {e}` | a saved project couldn't be read |
| `StoreError::Io` `io: {e}` | the disk couldn't be read or written |
| `StoreError::Usage` `usage: {s}` | something went wrong in Shibubu |
| `StoreError::NewerSchema` 新しい版で保存されています（…）。Shibubu を新しくしてから開いてください | saved by a newer Shibubu · update Shibubu to open it |
| `LlmError::NoCli` LLM の CLI が使えません（{detail}）。AI 無しの手元の推定で作ります。(`src/llm/mod.rs:94`) | {detail}, from the next rows |
| no_cli 使わない設定です (`src/llm_cli.rs:285`) | AI is turned off in Connections |
| no_cli 指定された場所に CLI がありません (`:289`) | Claude Code isn't at the set path |
| no_cli CLI を起こせません（{e}）(`:222, :296`) | Claude Code couldn't start |
| no_cli CLI が --version に答えません (`:301`) | Claude Code didn't answer |
| no_cli CLI の終わりを読めません（{e}）(`:239`) | Claude Code's answer couldn't be read |
| no_cli login の状態を読めません（{why}）(`:305, :529`) | the sign-in state couldn't be read |
| no_cli login していません。Terminal で claude auth login を打ってください (`:531`) | not signed in · run claude auth login in Terminal |
| no_cli plan の login ではありません。…plan の account に替えてください (`:536`) | signed in with a key, not a plan · run claude auth login with your plan account |
| no_cli claude が見つかりません。… (`:318`) | Install Claude Code, then run claude auth login in Terminal |
| no_cli 利用者の shell で claude を探せませんでした（{e}）(`:342`) | your shell couldn't be searched for Claude Code |
| no_cli 呼び出しの記録を書けません / 呼び出しの準備の file を書けません / 作業 folder を作れません（{kind}）(`:147, :206, :380`) | the AI call couldn't be prepared on disk |
| no_cli G84: 選んだ model を CLI が断った（`llm_cli::model_error`・拒否と読めた時だけ） | {Model} isn't available on this account · choose Default in + › Model |
| no_cli G84: 選んだ model で走り切らなかった（拒否と読めない失敗・生の文は出さない） | Couldn't run with {Model} · choose Default in + › Model |
| `LlmError::NoKey` (`src/llm/mod.rs:93`) | no AI key is set |
| `LlmError::Auth` | the AI key was refused · Paste API key again in Settings › AI & Models |
| `LlmError::Quota` | the AI usage limit or balance is reached · add credit in the Anthropic Console or choose another account in Settings › AI & Models |
| `LlmError::PlanLimit` | your Claude plan's limit is reached · try again after it resets |
| `LlmError::RateLimited` | AI is busy · try again in {s} s / try again shortly |
| `LlmError::Server` | the AI service is having trouble · try again shortly |
| `LlmError::Rejected` | AI didn't accept the request |
| `LlmError::Timeout` | AI didn't answer within {n} s |
| `LlmError::Network` | the AI can't be reached · check the network |
| `LlmError::Refused` | AI declined to answer · say it another way |

### Legacy canvas

| JP | EN |
|---|---|
| 新しい統合アプリ（default name, `src/app.rs:299`） | Untitled canvas |
| {n} レーン / 待機中 | {n} running / Idle |
| owner/repo · 接続先として設定 | owner/repo · Set as source |
| このセッションのGitHub接続先を変更しました / owner/repo 形式で入力してください | GitHub source: {owner/repo} / Use the owner/repo form |
| 配置を編集 / 操作へ切替 / 配置編集中（書込不可） / 操作モード | Edit layout / Done / Editing layout · actions are off / (nothing) |
| ＋ UIパーツ · GitHub Issue · GitHub PR · Web JSON · Desktop / 画面の操作 · Repoの機能 · Workflow · メモ | Add part · GitHub issues · GitHub pull requests · Web data · Screen control · Repo feature · Workflow · Notes |
| ＋ GitHub Repoの機能を導入 / ＋ GitHubの一部分を取り込む | Import from GitHub… |
| ソース: {repo} / GitHub接続先を設定してください | Source: {owner/repo} / No GitHub source · Set as source |
| Issueを取得 / PRを取得 / 実データは取得後に表示されます。 | Get issues / Get pull requests / (removed) |
| Issueを作成（要確認） / タイトル / 本文 / Issue作成を確認 | New issue · Title · Description · Create issue… |
| KANGAE_WEB_SERVICE_URL に設定したHTTPS JSONを読み取ります。 / 読み取り先は未設定です / Web JSONを取得 | (removed) / Web data isn't set up · Connections / Get data |
| 画面の読み（Shibubu Bridge・AX）· 読むだけ · AX要素を明示登録 / 画面を読めません: {e} | (removed) / Can't read the screen · {reason} |
| ① ウィンドウを検出 / ② AX操作要素を読む / ③ クリック登録 / 文字入力登録 / 入力内容 / 実行 | Find windows / Read controls / Add click step / Add typing step / Text to type / Run |
| 画面の操作をWorkflowへ登録しました（実行の前に窓を読み直します） | Added a step to “{workflow}” · the window is read again before it runs |
| 公開Repoのサブフォルダを選び、… / CLI接続: 有効 / コードのみ取得・実行不可 / 選択機能を実行する | (removed) / Command registered / Code only · can't run / Run this feature |
| Workflowは画面と独立。… / ▶ {}を実行 / ⇉ 先頭2つのWorkflowを並列実行 / 実行前確認 → 逐次/並列処理 → … | (removed) / Run “{workflow}” / Run the first two at once / (removed) |
| アプリやサイトから、今回必要な操作だけを集めます。 / GitHub + IDE + Webを統合 | (removed) / New canvas for GitHub, editor and web |
| 詳細を閉じる / パーツ名 / 配置を固定する / このパーツを削除 | (removed; `Settings ⌄` closes) / Part name / Lock position / Delete part… |
| 統合アプリを削除 / UIパーツを削除（windows）· キャンセル / 削除する | Delete “{name}”? / Delete “{part}”? (inline) · Cancel / Delete |
| GitHubの機能だけを導入（window）/ Repo / 機能のフォルダ | Import from GitHub (in place) / owner/repository / Folder in the repo |
| ① ライセンス・パス・コミットを検査 / Commit / License / Description / License API | Check licence, path and commit / Commit · Licence · Description · Licence source |
| サブフォルダ取得は『単独で動く』ことを保証しません。依存関係・商用利用条件を確認してください。 | A folder on its own may not run. Check dependencies and licence terms. |
| ライセンス・依存関係・取り込む範囲をレビューした / ② 指定部分だけを取得（コードは実行しない） | I reviewed the licence, dependencies and scope / Fetch this folder only (code is not run) |
| 取得済み機能をCLIに接続 / 既存の信頼済みCLIを人間が明示指定。… | Connect a trusted command / (removed) |
| /absolute/path/to/trusted-tool / 固定引数（空白区切り） / CLI接続を登録する / 閉じる | /absolute/path/to/tool / Fixed arguments / Register command / Close |
| 同じ操作対象を別の実行レーンが使用中です（`src/app.rs:980`） | Another run is using this target |
