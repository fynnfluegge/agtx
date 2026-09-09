---
name: agtx-review
description: Self-review completed work. Check for correctness, edge cases, and code quality. Write review to .agtx/review.md and stop.
---

# Review Phase

You are in the **review phase** of an agtx-managed task.

## Instructions

1. Pick the scope. If `.agtx/reviewed-at` exists, a prior review covered everything up to that commit: review `git diff $(cat .agtx/reviewed-at)..HEAD` plus `git status --short` for uncommitted work, and read the prior `.agtx/review.md` — check its points were addressed, don't re-review the branch. Otherwise review all changes made during execution: `git diff HEAD` (staged+unstaged) and `git log --oneline $(git merge-base HEAD origin/HEAD)..HEAD` for your commits. Do NOT diff against `main` or `origin/main` — those may include unrelated upstream history. Either way `??` lines in `git status --short` are untracked files that `git diff` never shows — read them in full.
2. Check for:
   - Correctness and edge cases
   - Error handling
   - Code style consistency with the existing codebase
   - Test coverage
   - Security issues (injection, XSS, etc.)
3. Fix any issues you find
4. Commit them. `git add -A`, commit, verify `git status --short` clean. Uncommitted = destroyed at task completion; merge is blocked until clean.

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

## Output Style

Terse. No pleasantries. Fragments OK. Short synonyms. Code exact.
Status updates: one line. Pattern: [what] [why]. Done.
