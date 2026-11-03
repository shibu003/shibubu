# Architecture

Shibubu is a single Rust crate. The desktop app is the `shibubu` binary (`src/main.rs`). The `kangae-*` command-line tools in `src/bin/` are development tools and are not part of the app bundle. This page describes the code as it is today and its known limits.

## Request flow

```
one sentence on Home
        |
        v
reply in the chat with a Proposal card  --  nothing is made until "Build now"
        |
        v
project composer  --  goal -> a Project with Pages, each Page holding surfaces
        |              (a real app window, a web page, a terminal)
        v
stored as revision 1 in SQLite; every later change ("add a Tests page") is a new revision, and ⌘Z / ⇧⌘Z move between them
        |
        v
a request to act ("change the README title and save it")
        |
        v
capability engine  --  plan steps -> show them -> wait for "Approve and send"
        |               -> act through the app (Accessibility bridge) -> read the app back
        v
run record: "Verified" only when the read-back shows the change; otherwise the step says what is unknown
```

- The AI steps run the AI command-line tool that is already signed in on the Mac (Claude Code's `claude -p`, or Grok) in a pseudo-terminal, with its own tools turned off (read-only file access only when you attach files) and without web search or MCP servers. Shibubu stores no model API key.
- A model never sends raw input events or shell commands to other apps. It proposes typed steps, and the engine runs them only after approval.
- If you typed in the target app a moment ago, the step is refused, so the AI does not type over you.
- When an app accepts an action, that does not count as success. The result stays unverified until it has been read back.

## Source layout (`src/`)

| Path | Role |
|---|---|
| `main.rs`, `app.rs` | Entry point, app state and the event loop |
| `ui/` | The shell: Home, the Project and its Pages, the build chat, the Terminal panel, Runs, Connections, Settings and the theme. `ui/renderer/` draws generated screens natively. |
| `generated.rs` | Connects Home's sentence to the composer, the stored revisions, the engine and the screen |
| `composer/` | Goal → Project. The AI reads the sentence when one is signed in; `composer/project.rs` is the rule-based path (**Use simple rules**). Also goal → app definition for generated screens. |
| `project/` | Page routing, per-Page input state, finding installed apps and checking that a surface is really there |
| `capability_engine/` | The engine: split the goal into steps, reuse what already works, generate only what is missing, verify against the real app, file or service, then record it |
| `adapters/`, `gui/` | Connection backends: git, GitHub, web data, trusted commands, and the macOS GUI adapter (observe / act / verify through the bridge) |
| `surface/` | Window and app surfaces, and the client for the Swift bridge |
| `terminal/` | A real PTY per Project, with a VT parser (`alacritty_terminal`) |
| `llm_cli.rs`, `llm/` | Running the signed-in AI CLI, and the model provider interface |
| `runtime/`, `domain/` | The action runtime (permission gate, executor, leases, run receipts) and the typed contracts it uses. The contracts mirror the JSON schemas in `docs/schemas/`. |
| `storage/` | SQLite store and migrations, Keychain secret store, event bus and a redacted log |
| `mcp.rs` | MCP servers from Shibubu's own config (`~/.shibubu/mcp.json`) |
| `audio/` | Recording and playback for Record humming |
| `media/`, `intel/`, `google/`, `codegen/` | Music and YouTube, app introspection, Gmail and Calendar, and sandboxed JXA generation. Mostly used by the `kangae-*` tools for now. |

Modules that are compiled into the app but only partly connected to the GUI are marked `allow(dead_code)` in `src/main.rs`.

The native helpers are written in Swift and use only public APIs:

- `bridge/macos-surface`: lists apps and windows, reads the Accessibility tree, presses and types into other apps. It is bundled into the app.
- `bridge/macos-mirror`: shows another app's window inside a Page with ScreenCaptureKit (macOS asks for the Screen Recording permission). It is built only with `KANGAE_FEATURES=mirror`, and the released `Shibubu.dmg` is built with it. Without it, a Page offers to open the app instead.

## Known limitations

1. macOS only. The Apple Silicon build is cross-compiled and has not been run yet.
2. What Shibubu can read and operate depends on what each app exposes to Accessibility. Some apps expose only their menus.
3. The AI steps need a signed-in AI CLI. Without one, Shibubu opens and says what is missing, but it cannot plan steps.
4. Trusted commands that you add in Connections run without a sandbox, after you approve each run. The JXA code generator runs inside `sandbox-exec`.
