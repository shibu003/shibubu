# Contributing to Shibubu

Thanks for your interest. Shibubu is alpha, so interfaces change often. If you plan a large change, open an issue first so we can agree on the approach.

## Setup

- macOS 14.0 or later (other platforms are not supported)
- Rust stable
- Xcode Command Line Tools: `xcode-select --install`
- Python 3, for the lint scripts. The schema checks also need `jsonschema`: `pip3 install jsonschema`, or install it into a virtual environment.

```sh
git clone https://github.com/shibu003/shibubu.git
cd shibubu
cargo run --release
```

Optional: run `git config core.hooksPath .githooks` to enable the repository's git hooks.

## Build, test, lint

```sh
cargo check
cargo test                                # unit and integration tests in tests/*.rs
bash scripts/lint.sh                      # UI copy check, license check, cargo check, clippy, cargo test and schema checks
bash scripts/release/build-mac.sh         # Shibubu.app and Shibubu.dmg (see docs/RELEASE.md)
```

`scripts/lint.sh` is what CI runs on macOS. Please run it before opening a pull request. Without `jsonschema` installed, it skips the schema checks and says so.

Individual checks:

```sh
python3 -I scripts/check_ui_copy.py --report             # UI text rules (docs/ui-copy-rules.md)
(cd docs && python3 -I scripts/validate_agent_native_contracts.py)   # example app corpus (docs/examples/README.md)
python3 -I scripts/qa_run.py --check docs/qa/scenarios/*.json        # QA scenario files
```

## Guidelines

- Keep each pull request focused on one change.
- Add or update tests with behavior changes. When you add a check, show that it fails when the behavior it guards is broken.
- Do not invent data or success in the UI or the code. Show unconnected and unverified states as they are.
- Actions that send, write or delete must stay behind user approval.
- Never commit secrets, tokens, `.env*` files, local databases or absolute paths from your machine.
- UI text follows [docs/ui-copy-rules.md](docs/ui-copy-rules.md).

## Pull requests

Fill in every section of the [pull request template](.github/PULL_REQUEST_TEMPLATE.md):

- **Summary**: what changed and why.
- **Tested**: the commands you ran and their results, with screenshots for UI changes.
- **Rollback**: how to undo the change.
- **Risk**: how many reviewers it needs. 0 for docs or changes behind a flag, 1 for normal code, 2 for CI, dependencies, release scripts, secrets or billing.
- **Security**: whether it touches secrets, permissions, network access or running external code.

Commit messages should include `tested:` and `rollback:` lines. `scripts/merge-gate.sh` looks for them.

## Reporting security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).
