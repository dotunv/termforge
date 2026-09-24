# Notes for coding agents

Read `README.md`, `CONTRIBUTING.md` and `docs/adr/` first. Then:

- Run `cargo xtask ci` and make it pass before proposing changes.
- Do not add dependencies to `crates/tf-proto`, `crates/tf-tap` or `crates/tf-ui` without an ADR.
- Never add `gpui` to anything except `bins/termforge`.
- Do not change `tf-store` migrations that already exist; append new ones.
- Shell scripts in `crates/tf-shell/scripts` must work in Windows PowerShell 5.1 and must not replace user key bindings or prompts.
