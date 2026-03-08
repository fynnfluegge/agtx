---
name: agtx-merge-conflicts
description: Resolve merge conflicts by merging the default branch into the current feature branch.
---

# Merge Conflict Resolution

Your feature branch has **merge conflicts** with the default branch (main/master).

## Instructions

1. Merge the default branch into your current branch:
   ```bash
   git fetch origin
   git merge origin/main
   ```
   If the project uses `master` instead of `main`, use `origin/master`.

2. Resolve ALL merge conflicts:
   - Open each conflicted file
   - Remove conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
   - Choose the correct resolution for each conflict, preserving both sides' intent when possible

3. After resolving all conflicts:
   ```bash
   git add -A
   git commit -m "Merge main into feature branch and resolve conflicts"
   ```

4. Run tests to verify nothing is broken. Fix any issues introduced by the merge.

## Rules

- **Always merge, never rebase.** The branch may have been shared or pushed.
- Do NOT squash commits.
- Do NOT force push.
- After committing the merge, say: "Merge conflicts resolved and committed."
- Then **stop and wait** for further instructions.
