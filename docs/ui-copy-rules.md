# UI copy rules: no marketing copy

The app's UI text is Japanese. The rule is that the UI shows names, states, reasons and next steps, and no promotional or explanatory filler. UI that the AI generates follows the same rule.

The check is `scripts/check_ui_copy.py`, which uses only the Python standard library. The word lists (promotional words, sentence endings, end punctuation and length limits) live in a single file, `docs/ui-copy-rules.json`. The Rust tests (`tests/common/mod.rs`) read the same file. The allowlist table in §3 of this page is also read by the check.

```sh
python3 -I scripts/check_ui_copy.py --report      # list findings, always exit 0 (what lint.sh runs today)
python3 -I scripts/check_ui_copy.py --strict      # exit 1 if any finding (same as no arguments)
python3 -I scripts/check_ui_copy.py --self-test   # run the rules and extraction against tests/fixtures/ui_copy/
python3 -I scripts/check_ui_copy.py --self-test --disable multi   # negative control: with a rule disabled, its bad lines pass and the run exits 1
```

Output is one finding per line: `file:line: [rule id,...] reason — current text`.

## 1. What is checked

| Location | What is extracted |
|---|---|
| `src/ui/**/*.rs` and `src/app.rs` | String literals that contain Japanese, including raw strings (`r"…"`, `r#"…"#`); byte strings are skipped. See the skip list below. |
| `docs/examples/**/*.json` (and `docs/examples/generated/` if it exists) | Values of `title`, `label`, `text`, `description`, `placeholder`, `help`, `empty` and `states.*`, including arrays like `"states": ["…"]`. Anything under a JSON Schema (`input_schema`, `properties`, and their direct `description`) is meant for models and is skipped. |

Skipped in Rust sources:

- Comments (`//`, `/* */`).
- Items under a test cfg: `cfg(test)`, `cfg(all(test, …))`, `cfg(any(test, …))`. The rest of a module or file after `#![cfg(test)]` is skipped too.
  - `not(test)`, `cfg_attr(test, …)` and `feature = "test"` are real code, so they are checked.
  - A skipped item ends at its `;` or `{…}` block for fn, mod, impl, struct, const, use and similar items (commas in `where` clauses or generics do not end it).
  - A skipped field or match arm ends at its `,`. An arm continues while `{…}` is followed by `,`, `.` or `else`.
  - Code after the skipped item is checked again.
- Arguments of `assert!`, `panic!`, `eprintln!`, `log::…`, `tracing::…` and `.expect(`. Other literals on the same line are still checked.
- Literals that start with `/` (font paths).

The kind of a literal depends on the innermost call around it and on which argument it is (the argument's position, not a count of literals):

- Wrappers such as `Some(`, `format!(`, `.text(`, `RichText::new(` and `{…}` pass through to the call outside them.
- Functions that take `ui` first, such as `theme::button(ui, …)`, do not count `ui` as an argument.
- Both branches of `if … {"a"} else {"b"}` count as the same argument.

Each literal is then classified:

| Kind | Where it comes from |
|---|---|
| button | 1st argument of `button`, `button_compact`, `button_sized`, `chip`, `icon_button`, `menu_button`, `small_button`; 2nd of `checkbox` and `selectable_label`; 3rd of `radio_value`; `egui::Button::new(…)`, also inside `ui.add` / `ui.add_enabled`; the cta of `content_opening` |
| placeholder | `hint_text` |
| label | the title of `content_opening`; `ui.heading("…")` |
| lead | the lead of `content_opening`; the first `label` / `small` within 3 statements after a heading (`Type::Heading*`, `Type::Display`, `.strong()`, `.heading()`, `ui.heading(…)`, `Window::new`, `content_opening`) |
| hero | `Type::Display` |

- A call that is split over several lines (as rustfmt does, one argument per line) counts as one statement: a line that ends with an open bracket joins with the following lines that start with `.` or `?`. Headings and `Type::Display` are looked for inside that statement as well.
- If brackets do not balance, or a test cfg item never closes before the end of the file, the check warns on stderr. `--strict` then exits 1 even with zero findings, so nothing after that point is silently missed.
- In JSON, `title` and `label` are labels, `placeholder` is a placeholder, and everything else is text.

## 2. Rules

| id | Flags | Example (flagged) |
|---|---|---|
| `dekimasu` | Descriptions ending in "you can …" forms (regex: `(でき\|られ\|[めせべえけれね])ます。?` at the end) | `下の入力から直接頼めます` ("You can ask directly from the input below") |
| `ad-word` | Promotional words: `まず`, `から組み立て`, `ひとことで`, `一言で`, `かんたん`, `簡単に`, `すぐに`, `あなたの`, `最適な`, `シームレス`, `を一か所で`, `ましょう` ("first", "build from", "in one word", "easy", "right away", "your", "optimal", "seamless", "all in one place", "let's") | `あなたのアプリ` ("Your app") |
| `exclaim` | Exclamation marks (`!`, `！`) | `保存！` ("Saved!") |
| `multi` | Descriptions of two or more sentences. **Two sentences are allowed** only when the first states a fact about state or a reason, and the second is a next step of 30 characters or fewer. Allowed first-sentence endings: `ません` / `ない` / `なし` / `未〜` / `です` / `でした` / `失敗しました` ("not", "none", "not yet", "is", "was", "failed"). | `作ったアプリの一覧。選ぶと開きます。` ("A list of apps you made. Select one to open it.") |
| `hero` | Display-size text that is not a question and is longer than 10 characters | `仕事も生活も一つの画面で変える` ("Change work and life on one screen") |
| `lead` | Explanatory text under a heading: kind lead, 8 or more characters. State facts are exempt, meaning text ending in `ません` / `ない` / `なし` / `中` / `済み` / `件` / `ください` ("not", "none", "in progress", "done", "items", "please"). | `選択したUIの構造と操作先` ("Structure and targets of the selected UI") |
| `long-name` | A button or label name longer than 16 characters (ASCII counts as half; `{…}` placeholders are not counted) | `ライセンス・依存関係・取り込む範囲をレビューした` ("Reviewed license, dependencies and import scope") |

**Allowed:**

- Action names, such as `作る` ("Create") and `＋ 新しいアプリ` ("+ New app").
- A one-line state fact, such as `まだデータがありません` ("No data yet").
- An error reason with a next step, such as `先にウィンドウを検出してください` ("Detect a window first").
- Placeholders that start with `例: ` ("e.g. "). These are the user's own words, so the word rules do not apply. A two-sentence placeholder is still flagged by `multi`.

**Fixtures** in `tests/fixtures/ui_copy/`:

- `ok.txt`: 10 lines that pass.
- `ng.txt`: 10 lines that fail, each on exactly one rule.
- `extract_rs*.txt`: extraction kinds. `extract_rs_inner_cfg.txt` yields 0 items because of `#![cfg(test)]` at the top of the file.
- `extract_json.json`: only the checked fields are extracted.

**Known gaps:**

- A single-sentence feature description ending in `〜します。` / `〜ます。` ("does …") is not covered by any rule, so it passes.
- `dekimasu` also matches ichidan verbs (`集めます`, "collects") and passives (`表示されます`, "is shown"). Both are descriptions, so the check errs on the side of removing them.
- Text passed through a variable (a `let` before `states::region`) is classified as text.
- Commas in closures (`|a, b|`) and turbofish count as argument separators, which can shift the kind of later arguments.
- `cfg(any(test, feature = …))` is skipped as test code even when the feature build uses it.
- For a field under a test cfg, the skip ends at a comma inside the type's generics. Types contain no literals, so nothing is missed.

## 3. Allowlist

Add a row here only for a false positive that cannot be fixed by rewording. `file` accepts globs, and the text is matched as a substring. Every row needs a reason.

<!-- allowlist:start -->
| file | text (substring) | reason |
|---|---|---|
<!-- allowlist:end -->

The list is empty for now. Row format: `` | `src/app.rs` | `外部サービスのデータは削除しません` | The delete confirmation must state its scope ("Data in external services is not deleted") | ``

## 4. Switching lint from `--report` to `--strict`

1. Remove or shorten the strings that `python3 -I scripts/check_ui_copy.py --report` lists. If you change anything under `src/` or `docs/examples/v3/`, also run `cd docs && python3 -I scripts/validate_agent_native_contracts.py`.
2. Keep fixing until `python3 -I scripts/check_ui_copy.py --strict` exits 0 with zero findings. Strings you keep go into the §3 allowlist, each with a reason.
3. In `scripts/lint.sh`, change `--report` to `--strict` (leave `--self-test` as is).
4. Negative control: put one fixed string back, confirm that `bash scripts/lint.sh` fails, then revert it.
