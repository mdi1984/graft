# Graft

A fast git client for Windows, made for working with several git worktrees at
once (for example one per AI agent) and for reviewing what they did.

## Install

You need [Rust](https://rustup.rs) and [Git for Windows](https://git-scm.com/download/win).

```powershell
powershell -ExecutionPolicy Bypass -File tools\install.ps1
```

This builds Graft and installs it to `%LOCALAPPDATA%\Programs\Graft`, then adds it
to the Start menu. Run it again to update. To try it without installing, run
`cargo run --release`.

## What it does

- **Projects as tabs.** Open local repositories, clone from a URL or create a new
  one. Drop a folder on the window to open it. Graft reopens your tabs on start.
- **History.** A commit graph with branch and tag chips, commit details, and each
  file's diff.
  - Right-click a commit to check it out, cherry-pick or revert it, reset a branch
    to it, or start a branch or tag there.
- **File status.** Stage or discard whole files, single hunks or selected lines.
  Then commit, amend, or commit and push in one go.
- **Worktrees.** Every worktree in the sidebar, with its dirty count and how far
  it is ahead. Create one for a new or existing branch; ignored files like `.env`
  are copied over. Changes show up live.
- **Branches, remotes, tags.** Check out, create, rename, delete, push, track
  remote branches. Add, edit and remove remotes (for example to publish a repo
  you created locally).
- **Pull, push, fetch.** With progress and Cancel. Force push asks first and uses
  `--force-with-lease`. Optional fetch in the background.
- **Merge and rebase, with a conflict resolver.** Ours, base and theirs for each
  conflict, a live result, editing by hand, and take-ours/take-theirs for whole
  files.
- **Stashes.** Create, including untracked files or only the staged changes.
  Apply, pop, drop, apply to another worktree, or make a branch from one. Each
  stash shows which worktree it came from.
- **Review.** What a worktree's branch would bring into main, uncommitted work
  included, side by side.
  - Mark files as viewed; a file shows up again if it changes after that.
  - Keep notes per file.
  - Open the worktree in your IDE, then merge.
- **Preview in main.** Check out an agent's worktree state (commits plus
  uncommitted changes) in your main folder to run it, then restore main with one
  click.
- **Slash.** Clean up finished work: branches whose upstream is gone, merged
  branches, missing worktrees. Nothing is deleted until you confirm, protected
  branches are never touched, and deleted branches can be restored.
- **File history and blame.** Right-click any file.
- **Search** (Ctrl+K) for worktrees, branches, tags, commits and changed files.
- **Git console.** Every git command Graft runs, with its output and timing.
- **Settings** (gear, Ctrl+,): your commit name and email, your IDE command,
  background fetch, protected branches.

## Keyboard

| Keys | Does |
| --- | --- |
| Ctrl+K | Search |
| Ctrl+T | Open a repository (new tab) |
| Ctrl+O | Open a folder |
| Ctrl+W | Close the tab |
| Ctrl+Tab, Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+, | Settings |
| Ctrl+Enter | Commit (in the commit message) |
| ↑ / ↓ | Move through commits and search results |
| V | Mark the file viewed (Review) |

## How it works

Graft reads git data with [gitoxide](https://github.com/GitoxideLabs/gitoxide)
and runs `git.exe` for everything that changes the repository. Your hooks,
config and Git Credential Manager keep working. The window never waits for git:
all git work runs in the background. Built with [egui](https://github.com/emilk/egui).

`CLAUDE.md` has the design notes.
