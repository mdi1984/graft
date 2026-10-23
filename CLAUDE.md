# Graft — a fast git GUI for Windows (Rust + egui)

Personal replacement for Sourcetree. Goals: fast on big repos, first-class
git worktree support (for parallel AI-agent work), pleasant code review.

## Stack
- UI: eframe/egui (custom dark theme, Inter for UI, JetBrains Mono for code)
- Git reads: gix (gitoxide): log, graph, status, diff, worktree list
- Git writes: shell out to git.exe (commit, stage via apply, push/pull/fetch,
  merge, rebase, stash, worktree add/remove). Keeps hooks, config and
  Git Credential Manager working.
- File watching: notify + notify-debouncer-full (one watcher per worktree)
- Background jobs: std threads + crossbeam-channel; cancellable
- Diff: imara-diff (Histogram), word-level highlights; syntect for syntax
- Misc: serde + toml config, directories, rfd, open, tracing

## Architecture
1. UI thread: render only, never touches git/disk. Sends commands, receives results.
2. App state: Repo -> Worktrees -> status/branch/dirty/ahead-behind.
3. Job runner: background pool for git ops, graph layout, diffs.
4. Git engine: reads via gix, writes via git CLI; watcher triggers incremental refresh.

## Windows rules
- Treat file content as bytes, not String. Respect core.autocrlf.
- Handle no-newline-at-EOF and binary files explicitly.

## Screens
Always visible: title bar, project tabs (one per open repo, top of the window),
toolbar (Commit, Pull, Push, Fetch | Branch, Merge, Stash, New worktree |
Preview in main, Slash | search), tabs: Workspace, Review, Stashes.

Project tabs: repo name + dirty dot; close, reorder, restored on next launch.
A "+" tab opens the start page: Open local folder, Clone from URL (progress,
credentials via git/GCM), Create new repo, Recent repositories.

Sidebar: WORKSPACE (File status w/ count, History); WORKTREES
(branch, path, dirty count, ahead badge); BRANCHES; REMOTES/STASHES/TAGS.

Workspace (main area switches):
- Clicking a dirty worktree -> File status: left column = staged files above
  unstaged files; right = diff with Stage hunk / Discard hunk and per-line
  checkboxes ("N lines selected -> Stage lines" bar). Bottom, full width:
  commit summary, description, Amend, "Push after commit", Commit button.
- Clicking a clean worktree or branch -> History: commit graph (colored lanes,
  ref chips), an "Uncommitted changes" row at the top if dirty, details pane right.

Review: worktree vs merge-base of base branch, INCLUDING uncommitted changes.
File list with "Viewed" checkboxes (keyed by content hash) + progress bar;
side-by-side diff with word highlights; local notes; Open in IDE, Preview in main, Merge.

Stashes: create (message, -u, --keep-index, --staged for selected lines);
list shows which worktree each stash came from; Apply / Pop / Apply to
another worktree / Branch from stash / Drop.

Conflicts: merge banner (Abort / Continue, disabled until resolved);
per-conflict cards Ours | (Base) | Theirs with Use ours/theirs/both/Edit;
live Result panel; whole-file take ours/theirs for lock files.
Read stages 1/2/3 from the index; resolve -> git add -> merge --continue.

## Feature mechanics
- Line staging (MVP): build patch from selected lines (unselected '-' -> context,
  unselected '+' -> dropped), `git apply --cached --recount`; unstage with --reverse.
  Later: content-based (apply in memory, write blob, update index).
- Slash (cleanup): `git fetch --prune`, then group: upstream [gone] (pre-checked),
  merged into main (pre-checked), never pushed (unchecked, warn), stale worktrees
  (git worktree prune). Remove worktrees before branches. Never touch current,
  default, protected patterns, locked worktrees. Dry run default.
  Undo log of name+SHA ("Recently slashed").
- Preview in main: require clean main, `git checkout --detach <wt HEAD>`, apply the
  worktree's uncommitted diff, banner with "Restore main".
- Copy configured ignored files (.env etc.) into new worktrees.

## Everyday git (beyond the original spec)
- Branches: checkout (refuse or offer stash when dirty), create, rename, delete,
  set upstream, checkout a remote branch as tracking branch, compare two branches.
- Commit context menu in History: checkout detached, create branch/tag here,
  cherry-pick, revert, reset soft/mixed/hard (hard asks first), copy SHA.
- Rebase onto a branch (same banner/continue/abort flow as merge).
- Tags: create (lightweight/annotated), delete, push. Remotes: add/edit/remove,
  fetch all, optional auto-fetch interval.
- Push: publish new branch with upstream, force only as --force-with-lease.
  Pull follows the repo's pull.rebase / ff settings.
- Files: discard changes, add to .gitignore, open in Explorer / IDE,
  file history and blame.
- Undo last commit (soft reset to HEAD~1, guarded by reflog).

## Trust and polish
- Git output console: every command Graft runs, with exit code and output.
  Errors surface as readable notifications, never silently.
- Progress + cancel for long operations (clone, fetch, push, pull).
- Settings: git identity (global / per repo), IDE command, auto-fetch interval,
  Slash protected patterns, ignored files copied into new worktrees.
- Detect and warn for submodules, Git LFS, very large repos.
- Keyboard shortcuts for main actions; Ctrl+K palette searches commits, branches, files.
- App icon "Splice": `crates/graft/assets/icon/` (SVG source; .ico + PNGs are
  generated by `cargo run -p icon-gen`).

## Working agreement
The owner does not read the code. Every change must be verified before it
lands: `cargo fmt`, `cargo clippy --workspace --all-targets` with no warnings,
`cargo test --workspace`, and launching Graft to check the screen. Git logic
gets tests against throwaway repos. One branch + PR per step, merged into
main once checks pass.

## MVP order
1. Cargo workspace + egui shell + theme
2. Open/clone/init repo, project tabs, commit graph (virtualized list)
3. Status + diff
4. Stage/unstage files, hunks, lines; commit
5. Worktree sidebar + file watcher
6. Push/pull/fetch
7. Slash, Stashes, Review, Conflicts, Preview in main