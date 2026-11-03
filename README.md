# Shibubu

**Type one sentence. Shibubu opens your Mac apps in one window, and AI changes nothing without your OK.**

> **Status: alpha (0.5.0)** — it works end to end, but expect rough edges. macOS 14 or later. Tested on macOS 14.5 (Intel). The Apple Silicon build is cross-compiled and has not been run yet — if you try it on an M-series Mac, please open an issue either way.

## What it does

1. **Say what you're working on.** On Home, type a goal — for example: *"Set up a dev environment for this repo. Edit the README in TextEdit and draw the architecture in Excalidraw. Put each on its own page."* Shibubu answers in the chat, in the language you typed, and shows a **Proposal** card. **Preview plan** opens the plan inside the card. Nothing is made until you press **Build now**.
2. **Get a project of real apps.** Press **Build now** and Shibubu creates a Project with one Page per app. If a question in the plan is still open, Shibubu picks an answer and says what it picked in the Project's chat. A Page shows the real app, not a mock-up.
3. **Change it in words.** *"Add a Tests page."* Every change is a revision, and ⌘Z undoes it.
4. **Let the AI act — after you approve.** *"Change the README title to 'Shibubu Demo' and save it."* Shibubu shows the steps first. Nothing is sent until you press **Approve and send**.
5. **Only "Verified" when it really happened.** Shibubu reads the app back after each step. If it can't confirm the change, it says so instead of reporting success.
6. **You keep the keyboard.** If you just typed in that app, the AI step is refused and tells you why.

## Why it's different

- **Your real apps.** Shibubu works with the apps you already have open (TextEdit, a browser tab, a design tool) instead of generating throwaway ones.
- **Approval before side effects.** Anything that writes, sends or deletes outside Shibubu runs only after you approve the exact steps.
- **No invented results.** "The app accepted the input" is not "done". A result stays unverified until it is read back.
- **Bring your own AI.** The AI steps use the AI command-line tool already signed in on your Mac — Claude Code (`claude`) or Grok. No API key is needed.

## Install

**Download:** `Shibubu.dmg` from [Releases](https://github.com/shibu003/shibubu/releases). Open it, drag Shibubu to Applications, and open it. The first time, macOS blocks it because it is not notarized yet: open **System Settings › Privacy & Security** and click **Open Anyway** ([details](docs/RELEASE.md#3-installing-and-first-launch)).

**Build from source:**

```sh
git clone https://github.com/shibu003/shibubu.git
cd shibubu
cargo run --release
```

To package `Shibubu.app` and `Shibubu.dmg` the way the release is built: `KANGAE_FEATURES=mirror bash scripts/release/build-mac.sh [--universal]` ([details](docs/RELEASE.md)). `cargo run` leaves out Mirror, so a Page offers to open the app instead of showing its window.

### Requirements

- macOS 14.0 or later
- Claude Code (`claude`) or Grok CLI, signed in — for the AI steps. Without one, Shibubu still opens and shows what is missing.
- Accessibility permission, so Shibubu can read and operate app windows.
- Screen Recording permission, only when a Page mirrors another app's window (for example TextEdit). macOS asks the first time; it is under **System Settings › Privacy & Security › Screen Recording**.
- To build: Rust stable and the Xcode Command Line Tools (`xcode-select --install`)

## How it works

Rust, with [egui](https://github.com/emilk/egui) on wgpu (Metal). Apps are read and operated through the macOS Accessibility API, and an app's window is shown inside a Page with ScreenCaptureKit, both via small Swift bridges. A single capability engine runs every step: it plans, asks for approval, acts, and reads back. Projects, revisions and runs are stored locally in SQLite. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Limits today

- macOS only.
- Some code comments are in Japanese. English issues and pull requests are welcome.
- Desktop control depends on what each app exposes to Accessibility. Some apps show only their menus.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).

## License

[Apache-2.0](LICENSE). See [NOTICE](NOTICE).
