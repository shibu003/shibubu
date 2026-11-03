# Example app definitions

| Location | Contents | Checked by |
|---|---|---|
| `v3/*.json` | **Regression corpus of generated apps (10 files).** Each file is the expected AppDefinition v3 output for one prompt. | `scripts/validate_agent_native_contracts.py`, strictly (rules below) |
| `app_v3_*.json` | Three earlier v3 design samples (`local_demo` / `preview_only`). Keep these unchanged. | The same script, with looser rules |
| `*_v2.json`, `goal_*.json`, others | Older schema v1 / v2 app examples and goal plans | `validate_app_examples.py`, `validate_v2_apps.py`, `validate_goal_plans.py` |

## The v3 regression corpus

**How to check it.** Run `python3 -I scripts/validate_agent_native_contracts.py` from `docs/`. It should end with `PASS corpus 10 files`.
The script needs `jsonschema` (`python3 -m venv .venv && .venv/bin/pip install jsonschema`; do not commit `.venv`). `scripts/lint.sh` runs the same script.
The checks are **static only**. They do not test rendering, whether components exist in a registry, connections or sending.

**Starting state assumed by every file.** No account, device or key is connected yet. That is why every external source and action is `requires_connection`, and only local actions and handoffs are `ready`.

### The 10 files

The prompts are in Japanese, because the app's UI language is Japanese. An English gloss follows each one.

| # | File | Prompt (`goal.literal_goal`) | What it covers | Overall state |
|---|---|---|---|---|
| 1 | `01_timer_local.json` | `タイマーを作って` ("Make a timer") | Smallest case: no connections, suggestions or approvals. Text, row and form. | **ready** |
| 2 | `02_inventory.json` | `在庫を整理したい` ("I want to organize my inventory") | Records shown in a table inside a split view. Deleting needs approval even though it is local. CSV import reads only the file you pick. | **ready** |
| 3 | `03_coding_workspace.json` | `今やっているコードの作業を 1 つの画面にまとめたい` ("Put my current coding work on one screen") | Local tasks, notes and run history next to a GitHub suggestion. Running tests is an `external_write` that needs approval. | **partial** |
| 4 | `04_research_notes.json` | `研究ノートを作りたい` ("I want research notes") | The editor is the main element. Citations refer only to imported material and keep their source. | **ready** |
| 5 | `05_creative_editor.json` | `動画を編集したい` ("I want to edit video") | Editor-first. The main element is a `source_surface` with `display: handoff`, so the existing editor app is used rather than rebuilt. | **partial** |
| 6 | `06_communication_multi.json` | `複数サービスのチャットをまとめたい` ("Combine chats from several services") | The provider is still unchosen (`null`). Sending is an `external_send` that needs approval, and its result can be `outcome_unknown`. | **partial** |
| 7 | `07_device_mirror.json` | `スマホの画面を Mac で見て操作したい` ("See and control my phone screen on the Mac") | A `source_surface` with `display: mirror`. Each input sent to the device needs approval. The phone's OS is not assumed. | **partial** |
| 8 | `08_owner_test1_outreach.json` | `LinkedIn の follower と following から AI 開発・founder・engineer の list を作って、営業メールを一括で送れる画面にして。YC の企業と投資家も list に。` ("Build a list of AI developers, founders and engineers from my LinkedIn followers and following, and a screen to send outreach email to them in bulk. Include YC companies and investors.") | Tabs for list, drafts and results. Drafts are written per recipient and stored locally. Sending is an `external_send` behind an approval gate, and replies are an `external_read`. Open questions are kept in `assumptions`. No recipients, replies or successes are invented. | **partial** |
| 9 | `09_owner_test2_calendar_mail_chat.json` | `Calendar と Gmail と claude.ai をまとめた画面にして。全部そこから操作できて、chat bar でも指示できるように。` ("One screen for Calendar, Gmail and claude.ai, all usable from there, with a chat bar for instructions.") | A `command_input` chat bar is the agent's entry point. A three-way split holds a calendar, a list and an embedded `source_surface` that falls back to handoff. Creating, updating, drafting and sending all need approval. | **partial** |
| 10 | `10_owner_test3_music_workspace.json` | `Reaper と YouTube と Apple Music を繋いで。Apple Music の library と download の曲を再生、YouTube の検索と再生の window も出して。鼻歌を録音して Reaper に音楽として入れて。` ("Connect REAPER, YouTube and Apple Music. Play songs from my Apple Music library and downloads, show a YouTube search and player window, and record my humming into REAPER.") | Media components: recorder, waveform, and a `source_surface` that is embedded with handoff as the fallback. The microphone is a capability that waits for OS permission. Inserting into REAPER is an `external_write` that needs approval. Reference tracks are never imported as audio. | **partial** |

### Readiness values

| Meaning | Schema value | How the corpus uses it |
|---|---|---|
| Available (in the registry, not chosen yet) | `capabilities[].readiness: discoverable` | Suggestions the prompt did not name (GitHub, chat providers, devices, editors, web embedding) |
| Chosen but not authenticated | `requires_connection` (source / capability / action) | Services the prompt named (Gmail, Calendar, Apple Music, REAPER, LinkedIn) and OS permissions not yet granted (microphone) |
| Connected, not yet verified | `connected_unverified` | Not used in the corpus, since it describes the state after connecting |
| Ready (permission and health check passed) | `ready` | Local actions, and handoffs that only pass a URL to the OS (`web.handoff`) |
| Verified (evidence exists) | Not part of the definition. The action's `verifier` (`local_state`, `read_back`, `state_compare`, `source_receipt`) produces a receipt at run time. | A send result is `sent_confirmed`, `failed` or `outcome_unknown`, and stays unconfirmed until a receipt arrives. |

### Rules the validator enforces

A file that is missing from the table above also fails.

1. **Schema and references.** The file matches `schemas/agent_native_app_v3.schema.json`. IDs (actions, sources, capabilities, workflows) and node keys are unique. Every `action_ref`, `binding.source_ref`, `capability_ref` and `workflows[].action_refs` entry resolves.
2. **Honest readiness.**
   - A `ready` action needs a `ready` capability, or no capability if the action is local.
   - A `requires_connection` action has a capability that is not ready.
   - Every external effect (`external_*`) has a `capability_ref`.
   - Unconnected sources have no `last_observed_at`.
   - `preview_only`, `local_demo` and `preview` are not allowed.
   - `state.readiness` is `ready` when everything is ready, `partial` when ready and not-ready items are mixed, and `blocked` when nothing is ready.
3. **Approval gates.** `external_send` and `destructive` actions have `requires_approval: true`. A workflow that contains one has an `approval` value other than `none`.
4. **Disabled reasons.** A node whose `action_ref` points to an action that is not ready has a sentence in `states.disabled`.
5. **No invented data.**
   - Bound lists, tables, calendars, timelines, chats and detail views have a sentence in `states.empty`.
   - Nodes bound to an unconnected source have a sentence in `states.requires_connection`.
   - `props` never holds inline row data (`items`, `rows`, `data`, and so on).
   - No absolute paths appear anywhere.
6. **No KPI widgets.** The components `kpi`, `metric`, `stat` and `chart` are not used. A `card` is allowed only when it has `props.selectable: true`.
7. **Required fields.** `button`, `form`, `command_input` and `recorder` need an `action_ref`. A `source_surface` needs `props.display` set to `native`, `embedded`, `handoff`, `mirror` or `composite`.

### Vocabulary used by the corpus

- Components:
  - layout: `column`, `row`, `split` (`layout.ratio`), `tabs` (each child's `label` is the tab name)
  - content: `text`, `list`, `table`, `detail`, `form` (`props.fields`), `editor`, `chat`, `calendar`, `timeline`
  - controls: `button` (`props.variant`: `primary` / `secondary` / `ghost` / `danger`), `command_input`
  - media: `media`, `waveform`, `recorder`
  - other: `source_surface`, `run_progress`
- Keys under `states`: `empty`, `requires_connection`, `disabled`. Each value is the sentence to display. Visibility is set with `visibility: {"when": "has_selection"}`.
- Source `kind` values: `local_state`, `local_collection`, `external_optional` (no provider chosen yet), `external_api`, `os_automation`, `public_web`.
- External action IDs are the same as their capability IDs. Local actions are named `local.<source>.<verb>`.
- `metadata.corpus` holds `prompt` (the same as `goal.literal_goal`), `todo` (a tracking ID), `differs_by` and `scene`. The table above is written from these fields.
