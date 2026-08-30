---
name: agtx:merge-conflicts
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
   - **Before resolving**, note which files have conflicts: `git diff --name-only --diff-filter=U`
   - Open each conflicted file
   - Remove conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`)
   - Choose the correct resolution for each conflict, preserving both sides' intent when possible

4. After resolving all conflicts:
   - `git add -A && git commit --no-edit`

5. **Review only the conflicted files** to catch resolution mistakes:
   - For each file that had conflicts (from step 3), compare your resolution against both parents:
     - `git diff HEAD^2..HEAD -- <file>` (what changed vs the default branch — shows only your feature's additions). **Start here** — this is usually the smaller, more focused diff.
     - `git diff HEAD^1..HEAD -- <file>` (what changed vs your branch before merge — shows what main brought in). This can be very large; if so, focus on the areas around the conflict markers you resolved rather than reading the entire diff.
   - Verify:
     - No code was accidentally dropped from either side
     - The combined logic is coherent (e.g., imports, function signatures, variable names all consistent)
     - No duplicate code blocks were introduced
   - If you find a problem, fix it immediately and commit the fix separately

6. Run tests to verify nothing is broken. Fix any issues introduced by the merge.

## Rules

- **Always merge, never rebase.** The branch may have been shared or pushed.
- Do NOT squash commits.
- Do NOT force push.
- After committing the merge, say: "Merge conflicts resolved and committed."
- Then **stop and wait** for further instructions.
