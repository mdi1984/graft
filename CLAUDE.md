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
Always visible: title bar, toolbar (Commit, Pull, Push, Fetch | Branch, Merge,
Stash, New worktree | Preview in main, Slash | search), tabs: Workspace, Review, Stashes.

Sidebar: repo picker; WORKSPACE (File status w/ count, History); WORKTREES
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

## MVP order
1. Cargo workspace + egui shell + theme
2. Open repo, commit graph (virtualized list)
3. Status + diff
4. Stage/unstage files, hunks, lines; commit
5. Worktree sidebar + file watcher
6. Push/pull/fetch
7. Slash, Stashes, Review, Conflicts, Preview in main