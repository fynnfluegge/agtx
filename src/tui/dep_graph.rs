//! Pure, TUI-independent dependency-graph model.
//!
//! Builds a topologically-leveled view of task dependencies from the
//! `referenced_tasks` field. Each task becomes a [`DepNode`] assigned to a
//! `level` (column): level 0 holds tasks with no in-graph dependencies, and a
//! task's level is one past the maximum level of its dependencies.
//!
//! The module is deliberately free of any ratatui/database types so it can be
//! unit-tested in isolation. The caller supplies a `deps_satisfied` closure so
//! the "unblocked" rule stays consistent with the board's move-gating
//! (`Database::deps_satisfied`).

use crate::db::{Task, TaskStatus};
use std::collections::{HashMap, HashSet, VecDeque};

/// A single task rendered in the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepNode {
    pub task_id: String,
    pub title: String,
    pub status: TaskStatus,
    /// True when the task is in Backlog and all its dependencies are satisfied
    /// (in Review/Done) — i.e. it is actionable and can be batch-moved.
    pub unblocked: bool,
    /// Topological column index (0 = no in-graph dependencies).
    pub level: usize,
    /// Titles of the task's (in-graph) dependencies, for the per-node hint line.
    pub dep_titles: Vec<String>,
}

/// The full dependency graph: nodes plus the level groupings used for layout.
#[derive(Debug, Clone, Default)]
pub struct DepGraph {
    /// All nodes, ordered by `(level, title)`.
    pub nodes: Vec<DepNode>,
    /// Directed edges `(dependency_id, dependent_id)`.
    pub edges: Vec<(String, String)>,
    /// Indices into `nodes`, grouped by level. `levels[l]` lists every node at
    /// column `l`, already sorted by title.
    pub levels: Vec<Vec<usize>>,
}

impl DepGraph {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Number of topological columns.
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Every node currently flagged as unblocked (actionable Backlog tasks).
    pub fn unblocked_ids(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| n.unblocked)
            .map(|n| n.task_id.clone())
            .collect()
    }
}

/// Parse the comma-separated `referenced_tasks` field into dependency IDs.
fn parse_refs(refs: &Option<String>) -> Vec<String> {
    match refs.as_deref() {
        Some(s) => s
            .split(',')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect(),
        None => Vec::new(),
    }
}

/// Build the dependency graph from a project's tasks.
///
/// `deps_satisfied` should return true when every dependency of the given task
/// is in Review/Done (the same rule `Database::deps_satisfied` applies). It is
/// passed as a closure so this function stays database-free and testable.
///
/// References to tasks that are not present in `tasks` (deleted/unknown) are
/// dropped, matching `deps_satisfied`'s treatment of missing deps as satisfied.
/// Cycles are tolerated: any nodes that cannot be topologically resolved are
/// placed in a column after the resolvable maximum, so the view never hangs.
pub fn build_dep_graph(tasks: &[Task], deps_satisfied: impl Fn(&Task) -> bool) -> DepGraph {
    // Known task ids, so we can drop dangling references.
    let known: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let title_of: HashMap<&str, &str> = tasks
        .iter()
        .map(|t| (t.id.as_str(), t.title.as_str()))
        .collect();

    // dependency_id -> dependent_id edges, restricted to known tasks.
    let mut edges: Vec<(String, String)> = Vec::new();
    // For each task: its in-graph dependency ids.
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks {
        let refs: Vec<String> = parse_refs(&task.referenced_tasks)
            .into_iter()
            .filter(|r| known.contains(r.as_str()))
            .collect();
        for dep in &refs {
            edges.push((dep.clone(), task.id.clone()));
        }
        deps.insert(task.id.clone(), refs);
    }

    // Kahn-style level assignment. in_degree counts unresolved dependencies.
    let mut in_degree: HashMap<String, usize> =
        deps.iter().map(|(id, d)| (id.clone(), d.len())).collect();
    // dependency_id -> dependents, to decrement when a dependency is resolved.
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for (dep, dependent) in &edges {
        dependents
            .entry(dep.clone())
            .or_default()
            .push(dependent.clone());
    }

    let mut level_of: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &queue {
        level_of.insert(id.clone(), 0);
    }

    while let Some(id) = queue.pop_front() {
        let lvl = *level_of.get(&id).unwrap_or(&0);
        if let Some(children) = dependents.get(&id) {
            for child in children {
                // Child sits at least one column past this dependency.
                let entry = level_of.entry(child.clone()).or_insert(0);
                if lvl + 1 > *entry {
                    *entry = lvl + 1;
                }
                if let Some(d) = in_degree.get_mut(child) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
    }

    // Anything left with in_degree > 0 is part of a cycle. Place it after the
    // resolved maximum so rendering is total and never loops.
    let resolved_max = level_of.values().copied().max().unwrap_or(0);
    let cycle_level = if level_of.len() == tasks.len() {
        resolved_max
    } else {
        resolved_max + 1
    };
    for task in tasks {
        level_of.entry(task.id.clone()).or_insert(cycle_level);
    }

    // Build nodes.
    let mut nodes: Vec<DepNode> = tasks
        .iter()
        .map(|task| {
            let level = *level_of.get(&task.id).unwrap_or(&0);
            let unblocked = task.status == TaskStatus::Backlog && deps_satisfied(task);
            let dep_titles = deps
                .get(&task.id)
                .map(|d| {
                    d.iter()
                        .filter_map(|id| title_of.get(id.as_str()).map(|t| t.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            DepNode {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status,
                unblocked,
                level,
                dep_titles,
            }
        })
        .collect();

    // Stable ordering: by level, then title, then id.
    nodes.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.task_id.cmp(&b.task_id))
    });

    // Group indices by level.
    let max_level = nodes.iter().map(|n| n.level).max().unwrap_or(0);
    let mut levels: Vec<Vec<usize>> =
        vec![Vec::new(); if nodes.is_empty() { 0 } else { max_level + 1 }];
    for (idx, node) in nodes.iter().enumerate() {
        levels[node.level].push(idx);
    }

    DepGraph {
        nodes,
        edges,
        levels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn task(id: &str, status: TaskStatus, refs: Option<&str>) -> Task {
        Task {
            id: id.to_string(),
            title: format!("Task {id}"),
            description: None,
            status,
            agent: "claude".to_string(),
            project_id: "proj".to_string(),
            session_name: None,
            worktree_path: None,
            branch_name: None,
            pr_number: None,
            pr_url: None,
            plugin: None,
            cycle: 0,
            referenced_tasks: refs.map(|s| s.to_string()),
            escalation_note: None,
            base_branch: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Mirror of Database::deps_satisfied for tests: deps satisfied when every
    /// referenced task in `tasks` is Review/Done (missing => satisfied).
    fn satisfied_fn(tasks: Vec<Task>) -> impl Fn(&Task) -> bool {
        move |t: &Task| {
            let refs = parse_refs(&t.referenced_tasks);
            refs.iter().all(|rid| {
                tasks.iter().find(|x| &x.id == rid).map_or(true, |dep| {
                    matches!(dep.status, TaskStatus::Review | TaskStatus::Done)
                })
            })
        }
    }

    fn node<'a>(g: &'a DepGraph, id: &str) -> &'a DepNode {
        g.nodes
            .iter()
            .find(|n| n.task_id == id)
            .expect("node exists")
    }

    #[test]
    fn linear_chain_levels_increase() {
        // a -> b -> c (c depends on b depends on a)
        let tasks = vec![
            task("a", TaskStatus::Done, None),
            task("b", TaskStatus::Backlog, Some("a")),
            task("c", TaskStatus::Backlog, Some("b")),
        ];
        let g = build_dep_graph(&tasks, satisfied_fn(tasks.clone()));
        assert_eq!(node(&g, "a").level, 0);
        assert_eq!(node(&g, "b").level, 1);
        assert_eq!(node(&g, "c").level, 2);
        assert_eq!(g.level_count(), 3);
    }

    #[test]
    fn diamond_shared_dependency() {
        // a -> b, a -> c, b -> d, c -> d. d must be 2 columns past a.
        let tasks = vec![
            task("a", TaskStatus::Done, None),
            task("b", TaskStatus::Backlog, Some("a")),
            task("c", TaskStatus::Backlog, Some("a")),
            task("d", TaskStatus::Backlog, Some("b,c")),
        ];
        let g = build_dep_graph(&tasks, satisfied_fn(tasks.clone()));
        assert_eq!(node(&g, "a").level, 0);
        assert_eq!(node(&g, "b").level, 1);
        assert_eq!(node(&g, "c").level, 1);
        assert_eq!(node(&g, "d").level, 2);
    }

    #[test]
    fn multiple_roots_at_level_zero() {
        let tasks = vec![
            task("a", TaskStatus::Done, None),
            task("b", TaskStatus::Backlog, None),
            task("c", TaskStatus::Backlog, Some("a,b")),
        ];
        let g = build_dep_graph(&tasks, satisfied_fn(tasks.clone()));
        assert_eq!(node(&g, "a").level, 0);
        assert_eq!(node(&g, "b").level, 0);
        assert_eq!(node(&g, "c").level, 1);
        assert_eq!(g.levels[0].len(), 2);
    }

    #[test]
    fn cycle_does_not_panic_or_hang() {
        // a -> b -> a (mutual). Both should still appear with finite levels.
        let tasks = vec![
            task("a", TaskStatus::Backlog, Some("b")),
            task("b", TaskStatus::Backlog, Some("a")),
        ];
        let g = build_dep_graph(&tasks, satisfied_fn(tasks.clone()));
        assert_eq!(g.nodes.len(), 2);
        // Neither node is dropped.
        assert!(g.nodes.iter().any(|n| n.task_id == "a"));
        assert!(g.nodes.iter().any(|n| n.task_id == "b"));
    }

    #[test]
    fn missing_reference_is_dropped() {
        // b references a deleted task "ghost" plus real "a".
        let tasks = vec![
            task("a", TaskStatus::Done, None),
            task("b", TaskStatus::Backlog, Some("a,ghost")),
        ];
        let g = build_dep_graph(&tasks, satisfied_fn(tasks.clone()));
        // Only the edge a->b survives; b stays at level 1.
        assert_eq!(node(&g, "b").level, 1);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0], ("a".to_string(), "b".to_string()));
        // dep_titles only lists the known dependency.
        assert_eq!(node(&g, "b").dep_titles, vec!["Task a".to_string()]);
    }

    #[test]
    fn unblocked_only_for_backlog_with_satisfied_deps() {
        let tasks = vec![
            // satisfied dependency (Done)
            task("done_dep", TaskStatus::Done, None),
            // unsatisfied dependency (Backlog)
            task("open_dep", TaskStatus::Backlog, None),
            // Backlog + dep satisfied => unblocked
            task("ready", TaskStatus::Backlog, Some("done_dep")),
            // Backlog + dep NOT satisfied => blocked
            task("blocked", TaskStatus::Backlog, Some("open_dep")),
            // Backlog, no deps => unblocked
            task("free", TaskStatus::Backlog, None),
            // Already in progress => not "unblocked" even with satisfied deps
            task("running", TaskStatus::Running, Some("done_dep")),
        ];
        let g = build_dep_graph(&tasks, satisfied_fn(tasks.clone()));
        assert!(node(&g, "ready").unblocked);
        assert!(node(&g, "free").unblocked);
        assert!(!node(&g, "blocked").unblocked);
        assert!(!node(&g, "running").unblocked);
        assert!(!node(&g, "done_dep").unblocked);
        // open_dep is itself a Backlog task with no deps, so it too is unblocked.
        assert!(node(&g, "open_dep").unblocked);

        let mut unblocked = g.unblocked_ids();
        unblocked.sort();
        assert_eq!(
            unblocked,
            vec![
                "free".to_string(),
                "open_dep".to_string(),
                "ready".to_string()
            ]
        );
    }

    #[test]
    fn empty_input_yields_empty_graph() {
        let g = build_dep_graph(&[], |_| true);
        assert!(g.is_empty());
        assert_eq!(g.level_count(), 0);
    }
}
