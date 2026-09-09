---
name: agtx-review
description: Self-review completed work. Check for correctness, edge cases, and code quality. Write review to .agtx/review.md and stop.
---

# Review Phase

You are in the **review phase** of an agtx-managed task.

## Instructions

1. Work out what to review. **If `.agtx/reviewed-at` exists**, a previous review already
   passed over everything up to the commit named in it, and you are looking only at what
   was done since:
   - `git diff $(cat .agtx/reviewed-at)..HEAD` — what has been committed since
   - `git status --short` — everything not yet committed. Lines starting `??` are
     **untracked files, which `git diff` does not show at all**; read those files in full,
     they are usually the newest work
   - Read the previous `.agtx/review.md` first — it says what was asked for. Your job is
     to check those points were actually addressed, not to re-litigate the whole branch.

   **Otherwise** review all changes made during execution: `git diff HEAD`
   (staged+unstaged), `git status --short` for untracked files, and
   `git log --oneline $(git merge-base HEAD origin/HEAD)..HEAD` to see only your commits.
   Do NOT diff against `main` or `origin/main` — those may include unrelated upstream
   history.
2. Check for:
   - Correctness and edge cases
   - Error handling
   - Code style consistency with the existing codebase
   - Test coverage
   - Security issues (injection, XSS, etc.)
3. Fix any issues you find
4. **Commit anything you changed.** `git add -A`, commit, and confirm `git status --short`
   is clean. Fixes left uncommitted are destroyed when the task completes, and the task
   cannot be merged until the tree is clean.

## Output

Write your review to `.agtx/review.md` in the **current working directory** with these sections:

## Review
Findings from your review — what looks good, what was fixed, any concerns.

## Status
Either `READY` (good to merge) or `NEEDS_WORK` (with explanation of remaining issues).

## CRITICAL: Stop After Writing

After writing `.agtx/review.md` (in the current working directory):
- Say: "Review written to `.agtx/review.md`."
- Wait for further instructions
