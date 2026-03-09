---
name: agtx-merge-conflicts
description: Resolve merge conflicts by merging the default branch into the current feature branch.
---

# Merge Conflict Resolution

Your feature branch has **merge conflicts** with the default branch (main/master).

## Instructions

1. First, commit all current work so nothing is lost:
   - Review the staged/unstaged changes with `git diff` and `git diff --cached`
   - Write a descriptive commit message that summarizes the actual changes
   - `git add -A && git commit -m "<your message>"`
   - If there is nothing to commit, skip this step.

2. Merge the default branch into your current branch:
   ```bash
   git fetch origin
   git merge origin/main
   ```
   If the project uses `master` instead of `main`, use `origin/master`.

3. Resolve ALL merge conflicts:
   - Open each conflicted file
   - Remove conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
   - Choose the correct resolution for each conflict, preserving both sides' intent when possible

4. After resolving all conflicts:
   - `git add -A && git commit --no-edit`

5. Run tests to verify nothing is broken. Fix any issues introduced by the merge.

## Rules

- **Always merge, never rebase.** The branch may have been shared or pushed.
- Do NOT squash commits.
- Do NOT force push.
- After committing the merge, say: "Merge conflicts resolved and committed."
- Then **stop and wait** for further instructions.
