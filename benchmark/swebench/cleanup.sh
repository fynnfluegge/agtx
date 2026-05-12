#!/usr/bin/env bash
# Clean up agtx state from a SWE-bench run.
#
# Usage:
#   ./cleanup.sh                          # clean ALL instances
#   ./cleanup.sh astropy__astropy-12907   # clean one specific instance
#
# What it removes:
#   - .agtx/ dir inside the repo clone (worktrees, task DB, artifacts)
#   - tmux sessions on the swebench and agtx servers
#   - Central project DB in ~/Library/Application Support/agtx/projects/
#   - Entry in the global index.db
#
# Repo clones under /tmp/swebench_repos/ are kept so the next run skips re-cloning.

set -euo pipefail

WORKDIR="${SWEBENCH_WORKDIR:-/tmp/swebench_repos}"

# Locate the central agtx DB directory (macOS vs Linux)
if [[ -d "$HOME/Library/Application Support/agtx" ]]; then
    AGTX_DB_DIR="$HOME/Library/Application Support/agtx"
else
    AGTX_DB_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/agtx"
fi

# Project DBs are stored in a projects/ subdirectory
PROJECTS_DIR="$AGTX_DB_DIR/projects"

clean_instance() {
    local instance="$1"
    local slug
    slug=$(echo "$instance" | tr '[:upper:]' '[:lower:]' | tr '_' '-' | cut -c1-50)

    echo "Cleaning $instance..."

    # 1. Kill tmux sessions first (before removing DB, so TUI exits cleanly)
    tmux -L swebench kill-session -t "$slug" 2>/dev/null && echo "  killed swebench session $slug" || true
    tmux -L agtx kill-session -t "$instance" 2>/dev/null && echo "  killed agtx session $instance" || true

    # 2. Remove .agtx/ dir from the repo clone
    local agtx_dir="$WORKDIR/$instance/.agtx"
    if [[ -d "$agtx_dir" ]]; then
        rm -rf "$agtx_dir"
        echo "  removed $agtx_dir"
    fi

    # 3. Delete the central project DB.
    #    Project DBs are in $PROJECTS_DIR/{hash}.db — find by querying the tasks table
    #    for a task whose title matches the instance ID.
    if [[ -d "$PROJECTS_DIR" ]]; then
        for db in "$PROJECTS_DIR"/*.db; do
            [[ -f "$db" ]] || continue
            result=$(sqlite3 "$db" \
                "SELECT title FROM tasks WHERE title LIKE '%$instance%' LIMIT 1" \
                2>/dev/null || true)
            if [[ -n "$result" ]]; then
                rm -f "${db}" "${db}-shm" "${db}-wal"
                echo "  deleted project DB: $db"
            fi
        done
    fi

    # 4. Remove from global index
    sqlite3 "$AGTX_DB_DIR/index.db" \
        "DELETE FROM projects WHERE path LIKE '%$instance%';" 2>/dev/null || true
    echo "  removed from index.db"
}

clean_all() {
    echo "Cleaning all SWE-bench instances in $WORKDIR..."

    # 1. Kill the swebench tmux server (only used by the benchmark)
    tmux -L swebench kill-server 2>/dev/null && echo "  killed swebench tmux server" || true

    # 2. Kill per-instance sessions on the shared agtx tmux server
    #    (named after the instance_id, e.g. "astropy__astropy-12907")
    for instance_dir in "$WORKDIR"/*/; do
        [[ -d "$instance_dir" ]] || continue
        instance=$(basename "$instance_dir")
        tmux -L agtx kill-session -t "$instance" 2>/dev/null && echo "  killed agtx session $instance" || true
    done

    # 3. Remove all .agtx/ dirs
    find "$WORKDIR" -maxdepth 2 -name ".agtx" -type d -exec rm -rf {} + 2>/dev/null || true
    echo "  removed all .agtx/ dirs"

    # 4. Delete all project DBs for swebench repos.
    #    Project DBs are in $PROJECTS_DIR/{hash}.db — find by querying the tasks table
    #    for tasks whose title contains a swebench instance pattern (repo__repo-NNNNN).
    if [[ -d "$PROJECTS_DIR" ]]; then
        for db in "$PROJECTS_DIR"/*.db; do
            [[ -f "$db" ]] || continue
            result=$(sqlite3 "$db" \
                "SELECT title FROM tasks WHERE title LIKE '%\_\_%' ESCAPE '\\' LIMIT 1" \
                2>/dev/null || true)
            if [[ -n "$result" ]]; then
                rm -f "${db}" "${db}-shm" "${db}-wal"
                echo "  deleted project DB: $db"
            fi
        done
    fi

    # 5. Remove swebench repo entries from global index
    sqlite3 "$AGTX_DB_DIR/index.db" \
        "DELETE FROM projects WHERE path LIKE '%swebench_repos%';" 2>/dev/null || true
    echo "  cleaned index.db"
}

if [[ $# -eq 0 ]]; then
    clean_all
else
    for instance in "$@"; do
        clean_instance "$instance"
    done
fi

echo "Done."
