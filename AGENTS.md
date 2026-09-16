# Project Instructions

## Devenv

- `devenv test` is the authoritative test suite.
- After code changes, run `devenv test` rather than individual Cargo test commands.
- Before starting development processes, run `devenv processes list` to check whether `devenv up` is already running.
- Reuse an existing process manager instead of launching another `devenv up` instance:
  - `devenv processes status dev` checks the development process.
  - `devenv processes logs dev` reads its recent logs.
  - `devenv processes restart dev` restarts it after changes when needed.
  - `devenv processes attach` attaches to its live status and logs without taking ownership.
- Do not stop a running process manager unless the user explicitly asks.
