# CORA

A native macOS desktop app that watches your GitHub pull requests and helps you review them well — an AI review copilot built with Tauri 2, React, and Rust.

CORA tracks the PRs that need your attention, analyzes each one against the whole repository (not just the diff), and turns the review into a guided reading: what matters, what's mechanical, what's risky, and what nobody noticed.

## What it does

**Stays on top of your queue**

- Background polling of GitHub for PRs where you're requested, mentioned, or involved
- Menu-bar tray plus a compact always-available "callout" window with action tiles (review requested, changes requested, mergeable, …)
- Native notifications when something needs you

**Analyzes the change like a principal engineer**

- Agentic analysis on AWS Bedrock (Claude): the model explores the repo — docs, tree, targeted reads — until it can place the change in the architecture
- Three-second TLDR assessment with risk calibration (a patch-level action bump is not an architectural event)
- A review plan classifying **every** file as critical / important / mechanical, grounded in computed diff metrics, driving your reading order
- Code findings — consequence-bearing defects and hand-rolled duplicates of existing code — with severity, kind, and a one-click path to a draft review comment
- AWS Well-Architected findings across all six pillars, material-only
- A C4 architecture graph of the system with the change highlighted

**Makes the review itself faster**

- Diff viewer with reading-order sort (interfaces first, tests last, churn-weighted) and per-file viewed tracking
- Noise files (lockfiles, generated code, snapshots) auto-skipped into a collapsed section — globs configurable
- "Since last look" diffs when a PR gets new commits
- Inline comments with GitHub-style code suggestions; resolve/unresolve threads
- Review-action gating: locked after you review until new commits, a re-request, or resolved threads
- File insights that follow your scroll — why this file was ranked, and its findings
- A per-PR assistant chat for questions the analysis didn't answer

## Requirements

- macOS (Apple Silicon or Intel)
- [Rust](https://rustup.rs) and [Node.js](https://nodejs.org) 20+
- A GitHub personal access token (stored in the macOS Keychain, never leaves the machine)
- An AWS account with Bedrock model access (auth via `aws sso login` profile; model id or inference-profile ARN configurable in Settings)

## Development

```bash
npm install
npm run tauri dev
```

Useful scripts:

| Command | What it does |
| --- | --- |
| `npm run tauri dev` | Run the app with hot reload |
| `npm run tauri build` | Build a release bundle |
| `npm run typecheck` | TypeScript check without emitting |
| `npm run bindings` | Regenerate `src/bindings/` TS types from the Rust types (ts-rs) |

## Project layout

```
src/                 React frontend
  components/        UI components (diff, analysis panels, assistant, files rail)
  state/             zustand stores (PRs, diffs, analysis, chat)
  windows/           App windows: main, settings, callout
  bindings/          Generated TS types — do not edit; run `npm run bindings`
src-tauri/src/       Rust backend
  github/            GitHub API client + background poller
  analysis/          Bedrock agentic analysis engine, tools, chat sessions
  store.rs           SQLite persistence (app data dir)
  secrets.rs         Keychain-backed token storage
```

## Configuration

Everything lives in Settings (gear icon): GitHub token, AWS profile/region/model, analysis passes, noise-file globs, notification and callout behavior. State is stored in `~/Library/Application Support/com.mp.cora/`.
