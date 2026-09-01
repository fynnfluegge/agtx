use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Global configuration (stored in ~/.config/agtx/)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Default agent for new tasks
    #[serde(default = "default_agent")]
    pub default_agent: String,

    /// Per-phase agent overrides
    #[serde(default)]
    pub agents: PhaseAgentsConfig,

    /// Worktree settings
    #[serde(default)]
    pub worktree: WorktreeConfig,

    /// UI theme/colors
    #[serde(default)]
    pub theme: ThemeConfig,

    /// Whether to automatically fullscreen-attach to the tmux session when opening a task popup
    #[serde(default)]
    pub fullscreen_on_enter: bool,

    /// Write agent lifecycle-hook configs into worktrees so agents report their
    /// own phase status. When false, agtx falls back to inferring liveness from
    /// tmux pane output.
    #[serde(default = "default_agent_hooks")]
    pub agent_hooks: bool,
    /// Answer agents' trust and permission-bypass prompts on the user's behalf.
    ///
    /// Off by default: those prompts ask the user to vouch for a directory or to
    /// accept unattended tool execution, and that is their decision. With this
    /// off a task whose agent is waiting on one shows as `Blocked` with the reason
    /// on the card, and the user answers in the agent's own pane.
    ///
    /// Turning it on restores the historical behaviour — agtx reads the pane and
    /// sends the answer — which is what unattended runs want. It is already
    /// effectively on inside the Docker sandbox and the benchmark, where the
    /// container is disposable and pre-accepting is the documented policy.
    ///
    /// Most users never need it: trust is inherited from the project root for
    /// claude, codex and gemini, `--trust` covers cursor and grok, opencode never
    /// asks, and antigravity's per-worktree entry is seeded from the consent the
    /// user already gave the project (see `agent::trust`).
    #[serde(default)]
    pub auto_trust: bool,

    /// Check GitHub once a day for a newer agtx release and show a notice in
    /// the header. Set false to never ask; `AGTX_NO_UPDATE_CHECK=1` is the
    /// per-invocation equivalent, for CI and containers.
    #[serde(default = "default_update_check")]
    pub update_check: bool,
}

fn default_update_check() -> bool {
    true
}

fn default_agent_hooks() -> bool {
    true
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_agent: default_agent(),
            agents: PhaseAgentsConfig::default(),
            worktree: WorktreeConfig::default(),
            theme: ThemeConfig::default(),
            fullscreen_on_enter: false,
            agent_hooks: default_agent_hooks(),
            auto_trust: false,
            update_check: default_update_check(),
        }
    }
}

/// Theme configuration with hex colors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Border color for selected elements (hex, e.g. "#FFFF00")
    #[serde(default = "default_color_selected")]
    pub color_selected: String,

    /// Border color for normal/unselected elements (hex, e.g. "#00FFFF")
    #[serde(default = "default_color_normal")]
    pub color_normal: String,

    /// Border color for dimmed/inactive elements (hex, e.g. "#666666")
    #[serde(default = "default_color_dimmed")]
    pub color_dimmed: String,

    /// Text color for titles (hex, e.g. "#FFFFFF")
    #[serde(default = "default_color_text")]
    pub color_text: String,

    /// Accent color for highlights (hex, e.g. "#00FFFF")
    #[serde(default = "default_color_accent")]
    pub color_accent: String,

    /// Color for task descriptions (hex, e.g. "#FFB6C1")
    #[serde(default = "default_color_description")]
    pub color_description: String,

    /// Color for column headers when not selected (hex, e.g. "#AAAAAA")
    #[serde(default = "default_color_column_header")]
    pub color_column_header: String,

    /// Color for popup borders (hex, e.g. "#00FF00")
    #[serde(default = "default_color_popup_border")]
    pub color_popup_border: String,

    /// Background color for popup headers (hex, e.g. "#00FFFF")
    #[serde(default = "default_color_popup_header")]
    pub color_popup_header: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            color_selected: default_color_selected(),
            color_normal: default_color_normal(),
            color_dimmed: default_color_dimmed(),
            color_text: default_color_text(),
            color_accent: default_color_accent(),
            color_description: default_color_description(),
            color_column_header: default_color_column_header(),
            color_popup_border: default_color_popup_border(),
            color_popup_header: default_color_popup_header(),
        }
    }
}

fn default_color_selected() -> String {
    "#ead49a".to_string() // Yellow
}

fn default_color_normal() -> String {
    "#5cfff7".to_string() // Cyan
}

fn default_color_dimmed() -> String {
    "#9C9991".to_string() // Dark Gray
}

fn default_color_text() -> String {
    "#f2ece6".to_string() // Light Rose
}

fn default_color_accent() -> String {
    "#5cfff7".to_string() // Cyan
}

fn default_color_description() -> String {
    "#C4B0AC".to_string() // Rose (dimmed 80%)
}

fn default_color_column_header() -> String {
    "#a0d2fa".to_string() // Light Blue Gray
}

fn default_color_popup_border() -> String {
    "#9ffcf8".to_string() // Light Cyan
}

fn default_color_popup_header() -> String {
    "#69fae7".to_string() // Light Cyan
}

impl ThemeConfig {
    /// Parse a hex color string to RGB tuple
    pub fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    }
}

fn default_agent() -> String {
    "claude".to_string()
}

/// Per-phase agent overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PhaseAgentsConfig {
    pub research: Option<String>,
    pub planning: Option<String>,
    pub running: Option<String>,
    pub review: Option<String>,
}

/// Worktree configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Whether to use git worktrees for task isolation
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Automatically clean up worktrees after merge/reject
    #[serde(default = "default_true")]
    pub auto_cleanup: bool,

    /// Base branch to create worktrees from (empty = auto-detect main/master)
    #[serde(default)]
    pub base_branch: String,

    /// Directory (relative to project root) where worktrees are created
    #[serde(default = "default_worktree_dir")]
    pub worktree_dir: String,

    /// Prefix for branch names (e.g. "user/name" → "user/name/slug")
    #[serde(default = "default_branch_prefix")]
    pub branch_prefix: String,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_cleanup: true,
            base_branch: String::new(),
            worktree_dir: default_worktree_dir(),
            branch_prefix: default_branch_prefix(),
        }
    }
}

fn default_worktree_dir() -> String {
    crate::git::DEFAULT_WORKTREE_DIR.to_string()
}

fn default_branch_prefix() -> String {
    "task".to_string()
}

fn default_true() -> bool {
    true
}

/// Project-specific configuration (stored in .agtx/config.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Override default agent for this project
    pub default_agent: Option<String>,

    /// Per-phase agent overrides for this project
    pub agents: Option<PhaseAgentsConfig>,

    /// Override base branch for this project
    pub base_branch: Option<String>,

    /// GitHub URL for this project
    pub github_url: Option<String>,

    /// Directory (relative to project root) where worktrees are created
    pub worktree_dir: Option<String>,

    /// Comma-separated list of files to copy from project root into worktrees
    pub copy_files: Option<String>,

    /// Shell command to run inside the worktree after creation and file copying
    pub init_script: Option<String>,

    /// Shell command to run inside the worktree before removal
    pub cleanup_script: Option<String>,

    /// Workflow plugin name (e.g. "gsd", "spec-kit")
    pub workflow_plugin: Option<String>,

    /// Override branch prefix for this project (e.g. "user/name")
    pub branch_prefix: Option<String>,

    /// Skip git worktree creation — agent works directly in the project root.
    /// Useful when the repo is already an isolated environment (e.g. Docker container).
    pub skip_worktree: Option<bool>,
}

/// Serialise `value` over the TOML already at `path`, keeping everything agtx
/// did not put there.
///
/// The rule: *agtx rewrites the values of keys it knows, never deletes a key it
/// does not recognise, and removes a key it manages when that field becomes
/// unset.* `managed` names the keys that may be removed — the optional ones,
/// since a required key is always re-emitted. Everything else in the file
/// survives untouched, formatting and comments included.
///
/// Serialising the struct alone would not do: it emits a pristine document and
/// so drops every comment and unrecognised key the user wrote.
fn write_toml_preserving(path: &Path, value: &impl Serialize, managed: &ManagedKeys) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut rendered = toml_edit::ser::to_document(value).context("Failed to serialize config")?;
    // serde renders a nested struct as an *inline* table (`worktree = { .. }`).
    // Normalising to real tables keeps the user's `[worktree]` section on its
    // own lines with its comments, and keeps a fresh file in the documented
    // format.
    expand_inline_tables(rendered.as_table_mut());

    // A file that does not parse cannot be merged into, so start clean. The
    // alternative — refusing to save — leaves the user unable to fix it from
    // inside agtx.
    let mut doc = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .unwrap_or_default();

    merge_table(doc.as_table_mut(), rendered.as_table(), managed);
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

/// Remove `key`, moving any comments above it onto the key that follows.
///
/// A comment belongs to the *next* key's decor, so removing the first key in a
/// file would take the file's header comment with it. Erring toward keeping the
/// user's text leaves a comment that really was about the removed key slightly
/// orphaned, which is visible and fixable where deletion is not.
///
/// A comment above the *last* key in a table has nothing to move onto and is
/// dropped.
fn remove_key_keeping_comments(table: &mut toml_edit::Table, key: &str) {
    if !table.contains_key(key) {
        return;
    }
    let prefix = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string();

    let successor = table
        .iter()
        .map(|(k, _)| k.to_string())
        .skip_while(|k| k != key)
        .nth(1);

    table.remove(key);

    if !prefix.contains('#') {
        return;
    }
    if let Some(next) = successor {
        if let Some(mut next_key) = table.key_mut(&next) {
            let decor = next_key.leaf_decor_mut();
            let existing = decor.prefix().and_then(|p| p.as_str()).unwrap_or("");
            decor.set_prefix(format!("{prefix}{existing}"));
        }
    }
}

/// Turn every inline table into a `[section]`, recursively.
fn expand_inline_tables(table: &mut toml_edit::Table) {
    let inline_keys: Vec<String> = table
        .iter()
        .filter(|(_, item)| item.is_inline_table())
        .map(|(key, _)| key.to_string())
        .collect();
    for key in inline_keys {
        if let Some(item) = table.get_mut(&key) {
            let expanded = std::mem::replace(item, toml_edit::Item::None)
                .into_table()
                .unwrap_or_else(|original| {
                    // Cannot happen for an inline table, but losing the value
                    // would be worse than an odd-looking one.
                    let mut fallback = toml_edit::Table::new();
                    fallback.insert(&key, original);
                    fallback
                });
            *item = toml_edit::Item::Table(expanded);
        }
    }
    for (_, item) in table.iter_mut() {
        if let Some(sub) = item.as_table_mut() {
            expand_inline_tables(sub);
        }
    }
}

/// The keys a writer is allowed to delete, per table. See
/// `write_toml_preserving`.
struct ManagedKeys {
    /// Deletable keys at the top level.
    root: &'static [&'static str],
    /// Deletable keys inside a named sub-table.
    nested: &'static [(&'static str, &'static [&'static str])],
}

impl ManagedKeys {
    fn for_table(&self, table: Option<&str>) -> &'static [&'static str] {
        match table {
            None => self.root,
            Some(name) => self
                .nested
                .iter()
                .find(|(t, _)| *t == name)
                .map(|(_, keys)| *keys)
                .unwrap_or(&[]),
        }
    }
}

fn merge_table(target: &mut toml_edit::Table, source: &toml_edit::Table, managed: &ManagedKeys) {
    merge_table_named(target, source, managed, None);
}

fn merge_table_named(
    target: &mut toml_edit::Table,
    source: &toml_edit::Table,
    managed: &ManagedKeys,
    name: Option<&str>,
) {
    for (key, item) in source.iter() {
        match (
            item.as_table(),
            target.get_mut(key).and_then(|i| i.as_table_mut()),
        ) {
            // Both sides are tables: recurse, so the sub-table's own comments
            // and layout survive too.
            (Some(sub_source), Some(sub_target)) => {
                merge_table_named(sub_target, sub_source, managed, Some(key));
            }
            _ => match target.get_mut(key) {
                // Replacing an existing value keeps the decor around it, which
                // is where a trailing `# comment` on that line lives.
                Some(existing) => {
                    let decor = existing.as_value().map(|v| v.decor().clone());
                    *existing = item.clone();
                    if let (Some(decor), Some(value)) = (decor, existing.as_value_mut()) {
                        *value.decor_mut() = decor;
                    }
                }
                // An empty table is nothing but a header. Writing one into a
                // file that had no `[agents]` only appends noise — and appends
                // it *after* whatever sections the user wrote.
                None if item.as_table().is_some_and(|t| t.is_empty()) => {}
                None => {
                    target.insert(key, item.clone());
                }
            },
        }
    }

    // An optional field the user cleared has to actually leave the file.
    for key in managed.for_table(name) {
        if !source.contains_key(key) {
            remove_key_keeping_comments(target, key);
        }
    }

    // Recurse into managed sub-tables the serialization omitted entirely, so a
    // cleared `[agents]` still drops its keys.
    for (table_name, _) in managed.nested {
        if source.contains_key(table_name) {
            continue;
        }
        if let Some(sub) = target.get_mut(table_name).and_then(|i| i.as_table_mut()) {
            for key in managed.for_table(Some(table_name)) {
                remove_key_keeping_comments(sub, key);
            }
        }
    }
}

/// Optional keys on `GlobalConfig`. Everything else it writes is required, so
/// it is always re-emitted and can never need deleting.
const GLOBAL_MANAGED: ManagedKeys = ManagedKeys {
    root: &[],
    nested: &[("agents", &["research", "planning", "running", "review"])],
};

/// `ProjectConfig` is optional all the way down: every field can be cleared.
const PROJECT_MANAGED: ManagedKeys = ManagedKeys {
    root: &[
        // `agents` is itself optional here, so clearing it must drop the whole
        // `[agents]` section rather than leaving an empty header behind.
        "agents",
        "default_agent",
        "base_branch",
        "github_url",
        "worktree_dir",
        "copy_files",
        "init_script",
        "cleanup_script",
        "workflow_plugin",
        "branch_prefix",
        "skip_worktree",
    ],
    nested: &[("agents", &["research", "planning", "running", "review"])],
};

impl GlobalConfig {
    /// Load global config from default location
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            toml::from_str(&content).context("Failed to parse global config")
        } else {
            Ok(Self::default())
        }
    }

    /// Save global config to its default location, keeping the user's comments
    /// and any keys agtx does not recognise. See `write_toml_preserving`.
    pub fn save(&self) -> Result<()> {
        write_toml_preserving(&Self::config_path()?, self, &GLOBAL_MANAGED)
    }

    /// Get the path to the global config file.
    ///
    /// Always `~/.config/agtx/` on every platform — see the config-path split
    /// note in CLAUDE.md. Honours `AGTX_CONFIG_DIR` like `TrustStore::path`, so
    /// a test exercising the writer cannot overwrite the real user's
    /// `config.toml`.
    pub fn config_path() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("AGTX_CONFIG_DIR") {
            if !dir.is_empty() {
                return Ok(PathBuf::from(dir).join("config.toml"));
            }
        }
        let home = std::env::var("HOME").context("Could not determine home directory")?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("agtx")
            .join("config.toml"))
    }

    /// Get the path to the global data directory
    ///
    /// Honours `AGTX_DATA_DIR` like `Database::data_root`, so a test or smoke run
    /// that redirects the databases also redirects the first-run probe that looks
    /// for `index.db` beside them.
    pub fn data_dir() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("AGTX_DATA_DIR") {
            if !dir.is_empty() {
                return Ok(PathBuf::from(dir));
            }
        }
        let dirs = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine data directory")?;
        Ok(dirs.data_dir().to_path_buf())
    }
}

impl ProjectConfig {
    /// Load project config from a project directory
    pub fn load(project_path: &Path) -> Result<Self> {
        let config_path = project_path.join(".agtx").join("config.toml");

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            toml::from_str(&content).context("Failed to parse project config")
        } else {
            Ok(Self::default())
        }
    }

    /// Save project config, keeping the user's comments and any keys agtx does
    /// not recognise. See `write_toml_preserving`.
    ///
    /// Note this invalidates the project's trust hash — see
    /// `TrustStore::retrust_after_agtx_write`, which every in-agtx caller of
    /// this must go through.
    pub fn save(&self, project_path: &Path) -> Result<()> {
        let config_path = project_path.join(".agtx").join("config.toml");
        write_toml_preserving(&config_path, self, &PROJECT_MANAGED)
    }
}

/// Action to take on first run based on config/data state.
#[derive(Debug, PartialEq)]
pub enum FirstRunAction {
    /// Config file already exists — nothing to do
    ConfigExists,
    /// Old config was found and migrated to new location
    Migrated,
    /// Existing user (has database) but no config file — save defaults silently
    ExistingUserSaveDefaults,
    /// New user — prompt for agent selection
    NewUserPrompt,
}

/// Determine what first-run action to take.
/// Pure logic — no side effects — so it's easily testable.
pub fn determine_first_run_action(
    config_exists: bool,
    migrated: bool,
    db_exists: bool,
) -> FirstRunAction {
    if config_exists {
        return FirstRunAction::ConfigExists;
    }
    if migrated {
        return FirstRunAction::Migrated;
    }
    if db_exists {
        return FirstRunAction::ExistingUserSaveDefaults;
    }
    FirstRunAction::NewUserPrompt
}

/// Merged configuration (global + project)
#[derive(Debug, Clone)]
pub struct MergedConfig {
    pub default_agent: String,
    pub phase_agents: PhaseAgentsConfig,
    pub worktree_enabled: bool,
    pub skip_worktree: bool,
    pub auto_cleanup: bool,
    pub base_branch: String,
    pub worktree_dir: String,
    pub github_url: Option<String>,
    pub theme: ThemeConfig,
    pub copy_files: Option<String>,
    pub init_script: Option<String>,
    pub cleanup_script: Option<String>,
    pub workflow_plugin: Option<String>,
    pub fullscreen_on_enter: bool,
    pub branch_prefix: String,
    pub agent_hooks: bool,
    pub auto_trust: bool,
    /// Global-only: a project does not get to decide whether the user is told
    /// about agtx releases.
    pub update_check: bool,
}

impl MergedConfig {
    /// Create merged config from global and project configs
    pub fn merge(global: &GlobalConfig, project: &ProjectConfig) -> Self {
        let project_agents = project.agents.clone().unwrap_or_default();
        Self {
            default_agent: project
                .default_agent
                .clone()
                .unwrap_or_else(|| global.default_agent.clone()),
            phase_agents: PhaseAgentsConfig {
                research: project_agents.research.or(global.agents.research.clone()),
                planning: project_agents.planning.or(global.agents.planning.clone()),
                running: project_agents.running.or(global.agents.running.clone()),
                review: project_agents.review.or(global.agents.review.clone()),
            },
            worktree_enabled: global.worktree.enabled,
            skip_worktree: project.skip_worktree.unwrap_or(!global.worktree.enabled),
            auto_cleanup: global.worktree.auto_cleanup,
            base_branch: project
                .base_branch
                .clone()
                .unwrap_or_else(|| global.worktree.base_branch.clone()),
            worktree_dir: project
                .worktree_dir
                .clone()
                .unwrap_or_else(|| global.worktree.worktree_dir.clone()),
            github_url: project.github_url.clone(),
            theme: global.theme.clone(),
            copy_files: project.copy_files.clone(),
            init_script: project.init_script.clone(),
            cleanup_script: project.cleanup_script.clone(),
            workflow_plugin: project.workflow_plugin.clone(),
            fullscreen_on_enter: global.fullscreen_on_enter,
            agent_hooks: global.agent_hooks,
            auto_trust: global.auto_trust,
            update_check: global.update_check,
            branch_prefix: project
                .branch_prefix
                .clone()
                .unwrap_or_else(|| global.worktree.branch_prefix.clone()),
        }
    }

    /// Get the agent name for a given phase.
    /// Falls back to default_agent if no phase-specific override is set.
    pub fn agent_for_phase(&self, phase: &str) -> &str {
        self.explicit_agent_for_phase(phase)
            .unwrap_or(&self.default_agent)
    }

    /// Get the explicitly configured agent for a phase, if any.
    /// Returns None when no phase-specific override is set (no fallback).
    pub fn explicit_agent_for_phase(&self, phase: &str) -> Option<&str> {
        match phase {
            "research" => self.phase_agents.research.as_deref(),
            "planning" | "planning_with_research" => self.phase_agents.planning.as_deref(),
            "running" | "running_with_research_or_planning" => self.phase_agents.running.as_deref(),
            "review" => self.phase_agents.review.as_deref(),
            _ => None,
        }
    }
}

/// Workflow plugin configuration loaded from plugin.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPlugin {
    pub name: String,
    pub description: Option<String>,
    pub init_script: Option<String>,
    /// List of supported agent names (e.g. ["claude", "codex", "gemini", "opencode"]).
    /// If empty or omitted, all agents are assumed supported.
    #[serde(default)]
    pub supported_agents: Vec<String>,
    #[serde(default)]
    pub artifacts: PluginArtifacts,
    #[serde(default)]
    pub commands: PluginCommands,
    #[serde(default)]
    pub prompts: PluginPrompts,
    #[serde(default)]
    pub prompt_triggers: PluginPromptTriggers,
    /// Extra directories to copy from project root to worktrees (e.g. [".specify"]).
    #[serde(default)]
    pub copy_dirs: Vec<String>,
    /// Individual files to copy from project root to worktrees (e.g. ["PROJECT.md"]).
    /// Merged with project-level copy_files during worktree setup.
    #[serde(default)]
    pub copy_files: Vec<String>,
    /// When true, enables Review → Planning transition for multi-phase workflows.
    #[serde(default)]
    pub cyclic: bool,
    /// When true, send a "clear context" command (agent-specific) before the
    /// phase skill and prompt on phase transitions. Currently honored only for
    /// Claude Code (`/clear`); other agents fall through to normal send.
    #[serde(default)]
    pub clear_context_on_advance: bool,
    /// Files/dirs to copy from worktree back to project root after a phase completes.
    /// Keyed by phase name (e.g. { research = ["PROJECT.md", ".planning"] }).
    #[serde(default)]
    pub copy_back: std::collections::HashMap<String, Vec<String>>,
    /// Auto-dismiss rules for interactive prompts that appear before the prompt trigger.
    /// Each rule specifies patterns to detect and keystrokes to send in response.
    #[serde(default)]
    pub auto_dismiss: Vec<AutoDismiss>,
}

/// Rule for auto-dismissing interactive prompts in the tmux pane.
/// When all `detect` patterns are present in the pane content (AND logic),
/// the `response` keystrokes are sent automatically.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoDismiss {
    /// All patterns must be present in pane content for the rule to trigger.
    pub detect: Vec<String>,
    /// Newline-separated keystrokes to send (e.g. "2\nEnter").
    pub response: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginArtifacts {
    #[serde(default)]
    pub preresearch: Vec<String>,
    pub research: Option<String>,
    pub planning: Option<String>,
    pub running: Option<String>,
    pub review: Option<String>,
}

/// Slash commands to invoke per phase (sent via tmux send_keys as real interactive commands).
/// e.g. "/gsd:plan-phase 1" or "/speckit.plan"
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginCommands {
    /// Command to run before research artifacts exist (e.g. "/gsd:new-project").
    /// Used only when no research artifacts are found in the project root.
    /// Falls back to `research` if not set.
    pub preresearch: Option<String>,
    pub research: Option<String>,
    pub planning: Option<String>,
    pub running: Option<String>,
    pub review: Option<String>,
}

/// Task content prompts per phase (sent after the command).
/// Should contain just the task description, not slash commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginPrompts {
    pub research: Option<String>,
    pub planning: Option<String>,
    pub planning_with_research: Option<String>,
    pub running: Option<String>,
    /// Prompt for Running after research or planning. Usually empty — prior phase provides context.
    pub running_with_research_or_planning: Option<String>,
    pub review: Option<String>,
}

/// Text patterns to wait for before sending the prompt for each phase.
/// When set, the system polls the tmux pane for this text before sending the prompt.
/// Useful for interactive commands like /gsd:new-project that ask questions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginPromptTriggers {
    pub research: Option<String>,
    pub planning: Option<String>,
    pub running: Option<String>,
    pub review: Option<String>,
}

impl WorkflowPlugin {
    /// Check if a phase's command or prompt contains `{task}`, meaning the phase
    /// can receive task context directly and can be entered from Backlog.
    /// If neither command nor prompt has `{task}`, the phase depends on a prior phase.
    /// If no command AND no prompt exist at all (e.g. void plugin), the phase is ungated.
    pub fn phase_accepts_task(&self, phase: &str) -> bool {
        let cmd = match phase {
            "planning" => self.commands.planning.as_deref(),
            "running" => self.commands.running.as_deref(),
            _ => None,
        };

        let prompt = match phase {
            "planning" => self.prompts.planning.as_deref(),
            "running" => self.prompts.running.as_deref(),
            _ => None,
        };

        // No command and no prompt → ungated (e.g. void plugin)
        if cmd.is_none() && prompt.is_none() {
            return true;
        }

        cmd.map_or(false, |c| c.contains("{task}") || c.contains("{task_id}"))
            || prompt.map_or(false, |p| p.contains("{task}") || p.contains("{task_id}"))
    }

    /// Check if the given agent is supported by this plugin.
    /// Returns true if supported_agents is empty (all agents allowed) or contains the agent.
    pub fn supports_agent(&self, agent_name: &str) -> bool {
        self.supported_agents.is_empty() || self.supported_agents.iter().any(|a| a == agent_name)
    }

    /// Validate that a plugin name is safe for use in filesystem paths.
    /// Rejects names containing path separators, traversal sequences, or
    /// characters outside [a-zA-Z0-9_-].
    pub fn validate_plugin_name(name: &str) -> Result<()> {
        if name.is_empty() {
            anyhow::bail!("Plugin name must not be empty");
        }
        if name.contains('.') || name.contains('/') || name.contains('\\') {
            anyhow::bail!(
                "Plugin name '{}' contains invalid characters (., /, \\)",
                name
            );
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!(
                "Plugin name '{}' contains invalid characters (only a-z, A-Z, 0-9, -, _ allowed)",
                name
            );
        }
        Ok(())
    }

    /// Directory holding global (user-level) plugins: `~/.config/agtx/plugins`.
    pub fn global_plugins_dir() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("agtx")
                .join("plugins"),
        )
    }

    /// Directory holding a project's local plugins: `{project}/.agtx/plugins`.
    pub fn project_plugins_dir(project_path: &Path) -> PathBuf {
        project_path.join(".agtx").join("plugins")
    }

    /// Load a plugin by name, checking project-local then global directories
    pub fn load(name: &str, project_path: Option<&Path>) -> Result<Self> {
        Self::validate_plugin_name(name)?;
        // 1. Check project-local
        if let Some(pp) = project_path {
            let local_path = Self::project_plugins_dir(pp).join(name).join("plugin.toml");
            if local_path.exists() {
                let content = std::fs::read_to_string(&local_path)?;
                return toml::from_str(&content).context("Failed to parse plugin.toml");
            }
        }
        // 2. Check global
        let global_path = Self::global_plugins_dir()
            .context("Could not determine home directory")?
            .join(name)
            .join("plugin.toml");
        if global_path.exists() {
            let content = std::fs::read_to_string(&global_path)?;
            return toml::from_str(&content).context("Failed to parse plugin.toml");
        }
        anyhow::bail!("Plugin '{}' not found", name)
    }

    /// Get the plugin directory path (for reading skill files)
    pub fn plugin_dir(name: &str, project_path: Option<&Path>) -> Option<PathBuf> {
        if Self::validate_plugin_name(name).is_err() {
            return None;
        }
        // Same lookup order: project-local first, then global
        if let Some(pp) = project_path {
            let local = Self::project_plugins_dir(pp).join(name);
            if local.join("plugin.toml").exists() {
                return Some(local);
            }
        }
        let global = Self::global_plugins_dir()?.join(name);
        if global.join("plugin.toml").exists() {
            return Some(global);
        }
        None
    }
}

/// Trust-on-first-use store for project configs.
///
/// Tracks SHA-256 hashes of `.agtx/config.toml` contents keyed by canonical project path.
/// When a project's config hash doesn't match the stored value, dangerous fields
/// (`init_script`, `copy_files`) are suppressed until the user explicitly trusts the project.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrustStore {
    #[serde(default)]
    pub projects: std::collections::HashMap<String, String>,
}

impl TrustStore {
    /// Load the trust store from the platform config directory (e.g. `~/.config/agtx/trusted_projects.toml` on Linux, `~/Library/Application Support/agtx/` on macOS).
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    /// Persist the trust store to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Where `trusted_projects.toml` lives.
    ///
    /// Honours `AGTX_CONFIG_DIR` like `Database::data_root` honours
    /// `AGTX_DATA_DIR`: `App::new` reads this store and several paths write it,
    /// so without a redirect the test suite would append temp-dir entries to
    /// the real user's file.
    fn path() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("AGTX_CONFIG_DIR") {
            if !dir.is_empty() {
                return Ok(PathBuf::from(dir).join("trusted_projects.toml"));
            }
        }
        let dirs = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine config directory")?;
        Ok(dirs.config_dir().join("trusted_projects.toml"))
    }

    /// Compute SHA-256 of a project's `.agtx/config.toml` content.
    pub fn hash_config(project_path: &Path) -> Option<String> {
        let config_path = project_path.join(".agtx").join("config.toml");
        let content = std::fs::read(&config_path).ok()?;
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(&content);
        Some(format!("{:x}", hash))
    }

    /// Check if a project's config is trusted (hash matches stored value).
    pub fn is_trusted(&self, project_path: &Path) -> bool {
        let canonical = project_path
            .canonicalize()
            .unwrap_or_else(|_| project_path.to_path_buf());
        let key = canonical.to_string_lossy().to_string();
        match (self.projects.get(&key), Self::hash_config(project_path)) {
            (Some(stored), Some(current)) => stored == &current,
            (None, None) => true, // No .agtx/config.toml — nothing to distrust
            _ => false,
        }
    }

    /// Mark a project's current config as trusted.
    pub fn trust_project(&mut self, project_path: &Path) -> Result<()> {
        let canonical = project_path
            .canonicalize()
            .unwrap_or_else(|_| project_path.to_path_buf());
        let key = canonical.to_string_lossy().to_string();
        if let Some(hash) = Self::hash_config(project_path) {
            self.projects.insert(key, hash);
            self.save()?;
        }
        Ok(())
    }

    /// Re-record trust after **agtx itself** rewrote `.agtx/config.toml`.
    ///
    /// Trust is a hash of that file, so every write agtx makes on the user's
    /// behalf invalidates it: the project would show the untrusted banner on
    /// the next launch and silently lose its `init_script`, `cleanup_script`
    /// and `copy_files` — over a change the user made in agtx's own UI, to a
    /// field (`workflow_plugin`) that is not one of the dangerous three.
    ///
    /// `was_trusted` is the state read *before* the write, and it is the whole
    /// safety of this: a project the user has never approved must not become
    /// trusted just because agtx touched its config, or an `init_script` they
    /// never vouched for starts running on the next worktree. This restores a
    /// prior decision; it never makes one.
    pub fn retrust_after_agtx_write(project_path: &Path, was_trusted: bool) -> Result<()> {
        if !was_trusted {
            return Ok(());
        }
        let mut store = Self::load()?;
        store.trust_project(project_path)
    }
}

#[cfg(test)]
mod managed_keys_tests {
    use super::*;

    /// The managed-key lists say what a save may *delete*. A field missing from
    /// them survives being cleared: the value goes on living in the file after
    /// the user removed it.
    ///
    /// Both literals below are exhaustive on purpose, so adding a field to the
    /// struct fails to compile here until someone has looked at this list.
    #[test]
    fn project_managed_keys_covers_every_optional_field() {
        let config = ProjectConfig {
            default_agent: Some("claude".into()),
            agents: Some(PhaseAgentsConfig {
                research: Some("claude".into()),
                planning: Some("claude".into()),
                running: Some("codex".into()),
                review: Some("claude".into()),
            }),
            base_branch: Some("main".into()),
            github_url: Some("https://example.invalid".into()),
            worktree_dir: Some(".agtx/worktrees".into()),
            copy_files: Some(".env".into()),
            init_script: Some("true".into()),
            cleanup_script: Some("true".into()),
            workflow_plugin: Some("gsd".into()),
            branch_prefix: Some("task".into()),
            skip_worktree: Some(false),
        };
        let mut doc = toml_edit::ser::to_document(&config).unwrap();
        // The writer expands inline tables before merging, so the guard has to
        // look at the same shape it will.
        expand_inline_tables(doc.as_table_mut());
        for (key, item) in doc.as_table().iter() {
            if item.is_table() {
                // Sub-tables need both: `nested` so their own keys can be
                // cleared, and `root` when the whole section is optional.
                assert!(
                    PROJECT_MANAGED.nested.iter().any(|(t, _)| *t == key),
                    "sub-table `{key}` is not in PROJECT_MANAGED.nested"
                );
            }
            assert!(
                PROJECT_MANAGED.root.contains(&key),
                "`{key}` can be cleared but PROJECT_MANAGED would not remove it"
            );
        }

        let agents = doc["agents"].as_table().unwrap();
        for (key, _) in agents.iter() {
            assert!(
                PROJECT_MANAGED.for_table(Some("agents")).contains(&key),
                "`agents.{key}` can be cleared but would not be removed"
            );
        }
    }

    /// `GlobalConfig`'s only optional fields are the four phase agents; every
    /// other key is required and therefore always re-emitted.
    #[test]
    fn global_managed_keys_covers_the_phase_agents() {
        let agents = PhaseAgentsConfig {
            research: Some("claude".into()),
            planning: Some("claude".into()),
            running: Some("codex".into()),
            review: Some("claude".into()),
        };
        let mut doc = toml_edit::ser::to_document(&agents).unwrap();
        expand_inline_tables(doc.as_table_mut());
        for (key, _) in doc.as_table().iter() {
            assert!(
                GLOBAL_MANAGED.for_table(Some("agents")).contains(&key),
                "`agents.{key}` can be cleared but would not be removed"
            );
        }
    }
}
