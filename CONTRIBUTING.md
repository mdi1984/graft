# Contributing to Graft

Thanks for wanting to help. First, a confession: no human has written a line of Graft's code. Claude
wrote all of it, and the owner hasn't read any of it. We'd like to keep that streak going.

## We don't accept hand-written code

Not because hand-written code is bad. It's because we can't review it the way we review everything
else here. So:

### The best contribution is an issue

Found a bug? Missing a feature? [Open an issue](../../issues/new) and describe it the way you'd
explain it to a colleague:

- **What you did**, step by step.
- **What you expected**, and **what happened instead**.
- **A screenshot** if it's something you can see. One screenshot beats three paragraphs.
- Your Windows and Git versions if it smells like an environment thing (`git --version`).

We'll turn it into a prompt ourselves. You did the hard part, which is noticing.

### If you really want to send code

Fine, we're flattered. A few rules:

1. **Let a strong frontier model write it.** If you typed the `fn` keyword yourself, you've gone too far.
2. **Keep it small: under 200 changed lines.** Bigger ideas start as an issue, so we can agree on the
   shape before anyone burns tokens.
3. **Write the PR description for a human.** Say what changes for someone using Graft, not which
   functions moved. If it reads like a commit log, start over.
4. **Show it.** If anything changes on screen, add a screenshot (before and after is even better).
5. **Leave it green.** These must pass, with no warnings:

   ```powershell
   cargo fmt --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

   Then launch Graft and check the change with your own eyes. Changes to git logic come with a test
   against a throwaway repository.

6. **One change per PR.** A fix plus a refactor plus a new feature is three PRs.

`CLAUDE.md` describes how Graft is built. Hand it to your model before anything else.

## Security

Found a way for a repository to make Graft do something it shouldn't? Please don't open a public
issue. Use [private vulnerability reporting](../../security/advisories/new) instead.

## License

By contributing, you agree that your contribution is licensed under the [MIT License](LICENSE).
