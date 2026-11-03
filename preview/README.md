# UI preview (local only)

A static browser mock of the Shibubu interface. Open `index.html` directly in a modern browser, or run this from the repository root and visit `http://localhost:8765`:

```sh
python3 -m http.server 8765 -d preview
```

- Type a short prompt. A small **heuristic mock generator** then picks one of several sample app shapes. This is not an LLM and does not generate apps in general.
- You can switch between app sessions, edit a local checklist, use a local timer and review agent proposals.
- You can apply, snooze or dismiss proposed **local UI changes**. Each change asks for explicit confirmation first. To undo the last change, open the `履歴` ("History") tab and click `直前の変更を取り消す` ("Undo the last change").
- The agent panel on the right is always shown. Nothing polls in the background, and nothing is sent to a server for inference.
- Data is kept only in the browser's `localStorage`. The preview uses no APIs, cua-driver, devices, real accounts, outbound messages or GitHub integration.

The preview shows UX and interaction states so the design can be judged before it exists in the native app. For the native app, see the [project README](../README.md).
