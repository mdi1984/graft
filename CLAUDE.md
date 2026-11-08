# Graft — a fast git GUI for Windows (Rust + egui)

Personal replacement for Sourcetree. Goals: fast on big repos, first-class
git worktree support (for parallel AI-agent work), pleasant code review.

## Stack
- UI: eframe/egui (custom dark theme, Inter for UI, JetBrains Mono for code)
- Git reads: gix (gitoxide) for the history walk, which must be fast on big
  repos. Everything else (status via porcelain v2, refs via for-each-ref,
  worktree list, diffs, commit details) parses git.exe's machine-readable
  output, so results always match git (fsmonitor, untracked cache, config).
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

Where code goes:
- `crates/graft-core`: git and the disk, no UI. One module per area
  (`ops` stage/discard, `remote` push/pull/fetch, `slash`, `preview`,
  `conflicts`, `stash`, `worktree`, ...). `cli.rs` runs git.
- `crates/graft/src/project/`: one open repository's state and rules.
  Views send an `Action`; each feature starts its jobs and handles their
  results in its own file (`run_op`, `run_head_op` in `ops.rs`).
- `crates/graft/src/ui/`: drawing only. `ui/can.rs` answers "may this
  button act now, and if not, why".
- `crates/graft/src/app.rs`: the window, project tabs, settings, updates.

## Code rules
These kept a 129-commit cleanup busy; follow them so it isn't needed again.
1. One rule, one place. Whether an action may run, and why not, is decided
   in `ui/can.rs` or `project/`; views ask and show the reason, never
   re-check. Before adding an `if` that refuses something, search for the
   existing rule and extend it.
2. Every action carries its target. A confirm, dialog or job records the
   worktree / branch / file it is about when it is created (`Selection`),
   checks it again when it runs (`still_selected`), and never acts on
   "whatever is selected now". Follow-up questions queue through `ask`.
3. Destructive git operations re-check right before they run (delete
   branch, discard, force push, Restore): use commands that compare first
   (`update-ref -d <name> <old>`, `--force-with-lease`, applying the exact
   diff the user saw) so a change made meanwhile makes them fail, not
   destroy it.
4. git runs through one path. Every call goes through `cli.rs`; settings
   every call needs go in its `CONFIG` list, never at one call site. Pass
   refs in full (`refs/heads/x`), so a tag named like a branch can't be
   picked; pass many paths via stdin. Behavior that depends on git's
   config or version gets a test with that config.

## Windows rules
- Treat file content as bytes, not String. Respect core.autocrlf.
- Handle no-newline-at-EOF and binary files explicitly.
- Repos inside WSL (`\\wsl.localhost\<distro>\...`, see `wsl.rs`): git runs
  in the distro via `wsl.exe`; absolute paths go through `Git::path_arg` /
  `Git::native_path`; changes are polled (Windows sends no file events there).
  WSL's git can be older (Ubuntu 24.04: 2.43), e.g. `checkout`/`reset` reject
  `--end-of-options`.

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
- Every bug fix comes with a test that fails without it. Before finishing,
  search for code doing the same thing: if the fix belongs there too, share
  the logic instead of fixing twice.
- Tests must not depend on the machine: throwaway repos, a test identity
  (`testing::git`), no global config where it matters. Before pushing, run
  the tests once with `GIT_CONFIG_GLOBAL` emptied (`NUL` on Windows),
  as CI has no git identity.

## Releases
- Semantic versions, one for the whole workspace (`[workspace.package] version`
  in Cargo.toml). Tags are `vX.Y.Z`; 1.0.0 is the first public release.
- To release: bump the version (plus `cargo update -w`) in a PR and merge it,
  then tag the merge commit and push the tag by hand:
  `git tag v1.2.3 <sha>` and `git push origin v1.2.3`.
- `.github/workflows/release.yml` runs on the tag: it fails unless the tag
  matches Cargo.toml, runs the tests, builds, and publishes a GitHub release
  with generated notes. `tools/package.ps1` makes the files:
  `Graft-X.Y.Z-x64.zip` (exe, LICENSE, README), `Graft-X.Y.Z-x64-setup.exe`
  (per-user Inno Setup installer from `installer/graft.iss`, used by winget),
  `graft-x64.exe` (stable name for the in-app updater) and `SHA256SUMS.txt`.
- In-app updater (`update.rs`): a Graft installed by the setup (its uninstall
  record names Graft's folder, see `installed.rs`) downloads the new setup
  and runs it with `/SILENT /WAITPID=<pid> /LAUNCH="<exe>"`; a zip copy swaps
  its exe. On start Graft corrects a stale DisplayVersion in that record.
  Never test the installer on the owner's machine with the real AppId: it
  replaces (and an uninstall removes) the owner's install record. CI's
  package job runs the real setup-to-setup update.
- `.github/workflows/ci.yml` checks fmt, clippy (-D warnings) and tests on
  every PR and push to main, and builds the packages (artifact
  `graft-packages`).
