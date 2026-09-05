//! The in-TUI config editor: `~/.config/agtx/config.toml` and a project's
//! `.agtx/config.toml` as a form.
//!
//! The fields are **declared**, not open-coded: one `FieldId` per setting, and
//! exactly two matches over it — `read` and `write`. The alternative is a match
//! arm per field in the renderer, the key handler, the loader and the saver,
//! which is four places to forget when a setting is added. The enum also makes
//! the compiler check that both directions cover every field.

use crate::config::{GlobalConfig, ProjectConfig};

use super::text_input::TextInput;

/// Every setting the editor exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldId {
    // General (global)
    DefaultAgent,
    FullscreenOnEnter,
    AgentHooks,
    AutoTrust,
    UpdateCheck,
    // Per-phase agents (global)
    AgentResearch,
    AgentPlanning,
    AgentRunning,
    AgentReview,
    // Worktree (global)
    WorktreeEnabled,
    WorktreeAutoCleanup,
    WorktreeBaseBranch,
    WorktreeDir,
    WorktreeBranchPrefix,
    // Theme (global)
    ColorSelected,
    ColorNormal,
    ColorDimmed,
    ColorText,
    ColorAccent,
    ColorDescription,
    ColorColumnHeader,
    ColorPopupBorder,
    ColorPopupHeader,
    // Project overrides
    ProjectDefaultAgent,
    ProjectWorkflowPlugin,
    ProjectBaseBranch,
    ProjectWorktreeDir,
    ProjectBranchPrefix,
    ProjectGithubUrl,
    ProjectCopyFiles,
    ProjectInitScript,
    ProjectCleanupScript,
    ProjectSkipWorktree,
}

/// What a field holds. `Choice` and `Color` are both text underneath; the kind
/// only decides how it is edited and drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Bool(bool),
    /// An unset optional field is the empty string.
    Text(String),
}

impl FieldValue {
    pub fn as_bool(&self) -> bool {
        matches!(self, Self::Bool(true))
    }

    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(t) => t,
            Self::Bool(_) => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Toggle,
    /// A closed set of options, built at open time because the list depends on
    /// what is installed — see `ConfigEditor::open`.
    Choice(Vec<Choice>),
    Text,
    /// Text, drawn with a swatch of the colour it names.
    Color,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What is stored. Empty means unset.
    pub value: String,
    pub label: String,
}

impl Choice {
    fn new(value: &str, label: &str) -> Self {
        Self {
            value: value.to_string(),
            label: label.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub id: FieldId,
    pub label: &'static str,
    /// One line under the field. Where a setting only affects *new* worktrees,
    /// this is where it says so.
    pub help: &'static str,
    pub kind: FieldKind,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub title: &'static str,
    pub fields: Vec<Field>,
}

/// What the editor is currently doing to the selected field.
#[derive(Debug, Clone)]
pub enum EditState {
    /// A text or colour field, open for typing.
    Text(TextInput),
    /// A choice list, open with a cursor in it.
    Choice { selected: usize },
}

/// What the caller must do after handling a key.
///
/// The editor never touches the filesystem: that keeps it pure, and keeps the
/// trust re-record and the config re-merge in one place in `app.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    None,
    Save,
    Close,
}

#[derive(Debug, Clone)]
pub struct ConfigEditor {
    pub global: GlobalConfig,
    /// `None` in dashboard mode, where there is no project to override.
    pub project: Option<ProjectConfig>,
    pub sections: Vec<Section>,
    pub section: usize,
    pub field: usize,
    pub editing: Option<EditState>,
    /// Set by any change, cleared by a save. Gates the discard prompt.
    pub dirty: bool,
    /// Asking whether to throw away unsaved changes.
    pub confirming_discard: bool,
    /// A line under the form: what was saved, or why something was refused.
    pub status: Option<String>,
}

impl ConfigEditor {
    /// Build the form for the configs as they are on disk.
    ///
    /// `agents` and `plugins` are the installed ones: the choice lists depend
    /// on the machine, so they cannot be static.
    pub fn open(
        global: GlobalConfig,
        project: Option<ProjectConfig>,
        agents: &[String],
        plugins: &[String],
    ) -> Self {
        let agent_required: Vec<Choice> =
            agents.iter().map(|a| Choice::new(a, a)).collect::<Vec<_>>();
        let mut agent_optional = vec![Choice::new("", "(use default)")];
        agent_optional.extend(agent_required.iter().cloned());

        let mut project_agent = vec![Choice::new("", "(use global)")];
        project_agent.extend(agent_required.iter().cloned());

        let mut plugin_choices = vec![Choice::new("", "(use global default)")];
        plugin_choices.extend(plugins.iter().map(|p| Choice::new(p, p)));

        let mut sections = vec![
            Section {
                title: "General",
                fields: vec![
                    Field {
                        id: FieldId::DefaultAgent,
                        label: "Default agent",
                        help: "Agent used for new tasks when no phase override applies.",
                        kind: FieldKind::Choice(agent_required.clone()),
                    },
                    Field {
                        id: FieldId::FullscreenOnEnter,
                        label: "Fullscreen on Enter",
                        help: "Enter opens a task's pane fullscreen instead of windowed.",
                        kind: FieldKind::Toggle,
                    },
                    Field {
                        id: FieldId::AgentHooks,
                        label: "Agent hooks",
                        help: "Let agents report their own status. Applies to new worktrees.",
                        kind: FieldKind::Toggle,
                    },
                    Field {
                        id: FieldId::AutoTrust,
                        label: "Auto-answer trust prompts",
                        help: "Off by default: trust and permission prompts are yours to answer.",
                        kind: FieldKind::Toggle,
                    },
                    Field {
                        id: FieldId::UpdateCheck,
                        label: "Check for updates",
                        help: "Ask GitHub once a day for a newer agtx release.",
                        kind: FieldKind::Toggle,
                    },
                ],
            },
            Section {
                title: "Agents",
                fields: vec![
                    Field {
                        id: FieldId::AgentResearch,
                        label: "Research",
                        help: "Agent for the research phase.",
                        kind: FieldKind::Choice(agent_optional.clone()),
                    },
                    Field {
                        id: FieldId::AgentPlanning,
                        label: "Planning",
                        help: "Agent for the planning phase.",
                        kind: FieldKind::Choice(agent_optional.clone()),
                    },
                    Field {
                        id: FieldId::AgentRunning,
                        label: "Running",
                        help: "Agent for the running phase.",
                        kind: FieldKind::Choice(agent_optional.clone()),
                    },
                    Field {
                        id: FieldId::AgentReview,
                        label: "Review",
                        help: "Agent for the review phase.",
                        kind: FieldKind::Choice(agent_optional),
                    },
                ],
            },
            Section {
                title: "Worktree",
                fields: vec![
                    Field {
                        id: FieldId::WorktreeEnabled,
                        label: "Use worktrees",
                        help: "Off means tasks run in the project root.",
                        kind: FieldKind::Toggle,
                    },
                    Field {
                        id: FieldId::WorktreeAutoCleanup,
                        label: "Auto cleanup",
                        help: "Remove a task's worktree when it reaches Done.",
                        kind: FieldKind::Toggle,
                    },
                    Field {
                        id: FieldId::WorktreeBaseBranch,
                        label: "Base branch",
                        help: "Empty auto-detects main or master. Affects new worktrees.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::WorktreeDir,
                        label: "Worktree dir",
                        help: "Where worktrees are created. Affects new worktrees only.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::WorktreeBranchPrefix,
                        label: "Branch prefix",
                        help: "\"task\" gives task/{slug}. Affects new worktrees only.",
                        kind: FieldKind::Text,
                    },
                ],
            },
            Section {
                title: "Theme",
                fields: vec![
                    color_field(FieldId::ColorSelected, "Selected", "Selected elements."),
                    color_field(FieldId::ColorNormal, "Normal", "Normal borders."),
                    color_field(FieldId::ColorDimmed, "Dimmed", "Inactive elements."),
                    color_field(FieldId::ColorText, "Text", "Task titles and body text."),
                    color_field(FieldId::ColorAccent, "Accent", "Highlights."),
                    color_field(
                        FieldId::ColorDescription,
                        "Description",
                        "Task descriptions.",
                    ),
                    color_field(
                        FieldId::ColorColumnHeader,
                        "Column header",
                        "Unselected column headers.",
                    ),
                    color_field(FieldId::ColorPopupBorder, "Popup border", "Popup borders."),
                    color_field(FieldId::ColorPopupHeader, "Popup header", "Popup headers."),
                ],
            },
        ];

        if project.is_some() {
            sections.push(Section {
                title: "Project",
                fields: vec![
                    Field {
                        id: FieldId::ProjectDefaultAgent,
                        label: "Default agent",
                        help: "Overrides the global default for this project only.",
                        kind: FieldKind::Choice(project_agent),
                    },
                    Field {
                        id: FieldId::ProjectWorkflowPlugin,
                        label: "Workflow plugin",
                        help: "Plugin stamped on new tasks. Existing tasks keep theirs.",
                        kind: FieldKind::Choice(plugin_choices),
                    },
                    Field {
                        id: FieldId::ProjectBaseBranch,
                        label: "Base branch",
                        help: "Branch this project's worktrees are cut from.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectWorktreeDir,
                        label: "Worktree dir",
                        help: "Overrides the global location. Affects new worktrees only.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectBranchPrefix,
                        label: "Branch prefix",
                        help: "Overrides the global prefix. Affects new worktrees only.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectGithubUrl,
                        label: "GitHub URL",
                        help: "Repository URL used for pull request operations.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectCopyFiles,
                        label: "Copy files",
                        help: "Comma-separated files copied into each new worktree.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectInitScript,
                        label: "Init script",
                        help: "Shell command run in a new worktree. Requires project trust.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectCleanupScript,
                        label: "Cleanup script",
                        help: "Shell command run before a worktree is removed.",
                        kind: FieldKind::Text,
                    },
                    Field {
                        id: FieldId::ProjectSkipWorktree,
                        label: "Skip worktree",
                        help: "Work directly in the project root, e.g. inside a container.",
                        kind: FieldKind::Choice(vec![
                            Choice::new("", "(use global)"),
                            Choice::new("true", "yes"),
                            Choice::new("false", "no"),
                        ]),
                    },
                ],
            });
        }

        Self {
            global,
            project,
            sections,
            section: 0,
            field: 0,
            editing: None,
            dirty: false,
            confirming_discard: false,
            status: None,
        }
    }

    /// Put the cursor on a named field, wherever it lives.
    ///
    /// First run uses this to open on `Default agent`.
    pub fn focus(&mut self, id: FieldId) {
        for (section, fields) in self.sections.iter().enumerate() {
            if let Some(field) = fields.fields.iter().position(|f| f.id == id) {
                self.section = section;
                self.field = field;
                return;
            }
        }
    }

    pub fn current_section(&self) -> &Section {
        &self.sections[self.section.min(self.sections.len() - 1)]
    }

    pub fn current_field(&self) -> Option<&Field> {
        self.current_section().fields.get(self.field)
    }

    pub fn value(&self, id: FieldId) -> FieldValue {
        read(id, &self.global, self.project.as_ref())
    }

    fn set(&mut self, id: FieldId, value: FieldValue) {
        // Compare what is *stored* before and after, not the value handed in:
        // a write can normalise (a blank optional field becomes unset) or
        // decline (a project field with no project open). The discard prompt
        // trusts `dirty`, so it must be true only when something really moved.
        let before = self.value(id);
        write(id, value, &mut self.global, self.project.as_mut());
        if self.value(id) != before {
            self.dirty = true;
            self.status = None;
        }
    }

    fn move_section(&mut self, delta: isize) {
        let count = self.sections.len() as isize;
        let next = (self.section as isize + delta).rem_euclid(count);
        self.section = next as usize;
        self.field = 0;
    }

    fn move_field(&mut self, delta: isize) {
        let count = self.current_section().fields.len() as isize;
        if count == 0 {
            return;
        }
        let next = (self.field as isize + delta).clamp(0, count - 1);
        self.field = next as usize;
    }

    /// Open the selected field for editing, or flip it if it is a toggle.
    fn activate(&mut self) {
        let Some(field) = self.current_field().cloned() else {
            return;
        };
        match field.kind {
            FieldKind::Toggle => {
                let flipped = !self.value(field.id).as_bool();
                self.set(field.id, FieldValue::Bool(flipped));
            }
            FieldKind::Choice(ref choices) => {
                let current = self.value(field.id);
                let selected = choices
                    .iter()
                    .position(|c| c.value == current.as_text())
                    .unwrap_or(0);
                self.editing = Some(EditState::Choice { selected });
            }
            FieldKind::Text | FieldKind::Color => {
                let mut input = TextInput::new();
                input.set_text(self.value(field.id).as_text().to_string());
                self.editing = Some(EditState::Text(input));
            }
        }
    }

    /// Commit whatever the open editor holds.
    fn commit(&mut self) {
        let Some(field) = self.current_field().cloned() else {
            self.editing = None;
            return;
        };
        match self.editing.take() {
            Some(EditState::Text(input)) => {
                let text = input.as_str().trim().to_string();
                if matches!(field.kind, FieldKind::Color)
                    && !text.is_empty()
                    && !is_hex_color(&text)
                {
                    // Re-open on the bad value rather than discarding what was
                    // typed: the user still has to fix it.
                    self.editing = Some(EditState::Text(input));
                    self.status = Some("Colours are hex, like #5cfff7.".to_string());
                    return;
                }
                self.set(field.id, FieldValue::Text(text));
            }
            Some(EditState::Choice { selected }) => {
                if let FieldKind::Choice(ref choices) = field.kind {
                    if let Some(choice) = choices.get(selected) {
                        self.set(field.id, FieldValue::Text(choice.value.clone()));
                    }
                }
            }
            None => {}
        }
    }

    /// Handle one key, reporting what the caller should do next.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> EditorAction {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if self.confirming_discard {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => EditorAction::Close,
                KeyCode::Char('s') | KeyCode::Char('S') => EditorAction::Save,
                _ => {
                    self.confirming_discard = false;
                    EditorAction::None
                }
            };
        }

        // A field open for editing owns its keys, including Esc: closing the
        // field must not close the whole form.
        //
        // The choice count is read up front because the cursor lives in
        // `self.editing` and the list in `self.sections`, and the borrow checker
        // will not allow holding one while asking the other.
        let choices = choice_count(self.current_field());
        if let Some(state) = self.editing.as_mut() {
            match (state, key.code) {
                (_, KeyCode::Esc) => self.editing = None,
                (_, KeyCode::Enter) => self.commit(),
                (EditState::Choice { selected }, KeyCode::Char('j') | KeyCode::Down) => {
                    let last = choices.saturating_sub(1);
                    if *selected < last {
                        *selected += 1;
                    }
                }
                (EditState::Choice { selected }, KeyCode::Char('k') | KeyCode::Up) => {
                    *selected = selected.saturating_sub(1);
                }
                (EditState::Text(input), _) => {
                    input.handle_edit_key(key);
                }
                _ => {}
            }
            return EditorAction::None;
        }

        match key.code {
            KeyCode::Char('s') if ctrl => return EditorAction::Save,
            KeyCode::Esc => {
                if self.dirty {
                    self.confirming_discard = true;
                    return EditorAction::None;
                }
                return EditorAction::Close;
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_field(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_field(-1),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => self.move_section(-1),
            // The Kitty protocol reports Shift+Tab as Tab with SHIFT rather
            // than as BackTab, so both spellings have to go backwards.
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.move_section(-1),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => self.move_section(1),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(),
            _ => {}
        }
        EditorAction::None
    }

    /// Called by the saver once the write succeeded.
    pub fn mark_saved(&mut self, message: impl Into<String>) {
        self.dirty = false;
        self.confirming_discard = false;
        self.status = Some(message.into());
    }
}

fn choice_count(field: Option<&Field>) -> usize {
    match field.map(|f| &f.kind) {
        Some(FieldKind::Choice(choices)) => choices.len(),
        _ => 0,
    }
}

fn color_field(id: FieldId, label: &'static str, help: &'static str) -> Field {
    Field {
        id,
        label,
        help,
        kind: FieldKind::Color,
    }
}

pub fn is_hex_color(text: &str) -> bool {
    crate::config::ThemeConfig::parse_hex(text).is_some()
}

fn text(value: &str) -> FieldValue {
    FieldValue::Text(value.to_string())
}

fn opt(value: Option<&String>) -> FieldValue {
    FieldValue::Text(value.cloned().unwrap_or_default())
}

/// Blank means "unset" for every optional field; decided in one place.
fn some_unless_blank(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// One of the two matches over `FieldId`. See the module docs.
fn read(id: FieldId, global: &GlobalConfig, project: Option<&ProjectConfig>) -> FieldValue {
    use FieldId::*;
    let p = project;
    match id {
        DefaultAgent => text(&global.default_agent),
        FullscreenOnEnter => FieldValue::Bool(global.fullscreen_on_enter),
        AgentHooks => FieldValue::Bool(global.agent_hooks),
        AutoTrust => FieldValue::Bool(global.auto_trust),
        UpdateCheck => FieldValue::Bool(global.update_check),

        AgentResearch => opt(global.agents.research.as_ref()),
        AgentPlanning => opt(global.agents.planning.as_ref()),
        AgentRunning => opt(global.agents.running.as_ref()),
        AgentReview => opt(global.agents.review.as_ref()),

        WorktreeEnabled => FieldValue::Bool(global.worktree.enabled),
        WorktreeAutoCleanup => FieldValue::Bool(global.worktree.auto_cleanup),
        WorktreeBaseBranch => text(&global.worktree.base_branch),
        WorktreeDir => text(&global.worktree.worktree_dir),
        WorktreeBranchPrefix => text(&global.worktree.branch_prefix),

        ColorSelected => text(&global.theme.color_selected),
        ColorNormal => text(&global.theme.color_normal),
        ColorDimmed => text(&global.theme.color_dimmed),
        ColorText => text(&global.theme.color_text),
        ColorAccent => text(&global.theme.color_accent),
        ColorDescription => text(&global.theme.color_description),
        ColorColumnHeader => text(&global.theme.color_column_header),
        ColorPopupBorder => text(&global.theme.color_popup_border),
        ColorPopupHeader => text(&global.theme.color_popup_header),

        ProjectDefaultAgent => opt(p.and_then(|c| c.default_agent.as_ref())),
        ProjectWorkflowPlugin => opt(p.and_then(|c| c.workflow_plugin.as_ref())),
        ProjectBaseBranch => opt(p.and_then(|c| c.base_branch.as_ref())),
        ProjectWorktreeDir => opt(p.and_then(|c| c.worktree_dir.as_ref())),
        ProjectBranchPrefix => opt(p.and_then(|c| c.branch_prefix.as_ref())),
        ProjectGithubUrl => opt(p.and_then(|c| c.github_url.as_ref())),
        ProjectCopyFiles => opt(p.and_then(|c| c.copy_files.as_ref())),
        ProjectInitScript => opt(p.and_then(|c| c.init_script.as_ref())),
        ProjectCleanupScript => opt(p.and_then(|c| c.cleanup_script.as_ref())),
        ProjectSkipWorktree => FieldValue::Text(match p.and_then(|c| c.skip_worktree) {
            Some(true) => "true".to_string(),
            Some(false) => "false".to_string(),
            None => String::new(),
        }),
    }
}

/// The other match over `FieldId`. See the module docs.
fn write(
    id: FieldId,
    value: FieldValue,
    global: &mut GlobalConfig,
    project: Option<&mut ProjectConfig>,
) {
    use FieldId::*;
    let flag = value.as_bool();
    let string = value.as_text().to_string();

    match id {
        DefaultAgent => global.default_agent = string,
        FullscreenOnEnter => global.fullscreen_on_enter = flag,
        AgentHooks => global.agent_hooks = flag,
        AutoTrust => global.auto_trust = flag,
        UpdateCheck => global.update_check = flag,

        AgentResearch => global.agents.research = some_unless_blank(string),
        AgentPlanning => global.agents.planning = some_unless_blank(string),
        AgentRunning => global.agents.running = some_unless_blank(string),
        AgentReview => global.agents.review = some_unless_blank(string),

        WorktreeEnabled => global.worktree.enabled = flag,
        WorktreeAutoCleanup => global.worktree.auto_cleanup = flag,
        WorktreeBaseBranch => global.worktree.base_branch = string,
        WorktreeDir => global.worktree.worktree_dir = string,
        WorktreeBranchPrefix => global.worktree.branch_prefix = string,

        ColorSelected => global.theme.color_selected = string,
        ColorNormal => global.theme.color_normal = string,
        ColorDimmed => global.theme.color_dimmed = string,
        ColorText => global.theme.color_text = string,
        ColorAccent => global.theme.color_accent = string,
        ColorDescription => global.theme.color_description = string,
        ColorColumnHeader => global.theme.color_column_header = string,
        ColorPopupBorder => global.theme.color_popup_border = string,
        ColorPopupHeader => global.theme.color_popup_header = string,

        ProjectDefaultAgent
        | ProjectWorkflowPlugin
        | ProjectBaseBranch
        | ProjectWorktreeDir
        | ProjectBranchPrefix
        | ProjectGithubUrl
        | ProjectCopyFiles
        | ProjectInitScript
        | ProjectCleanupScript
        | ProjectSkipWorktree => {
            // Reachable only when a project is open, since those fields are
            // only added to the form then.
            let Some(config) = project else { return };
            match id {
                ProjectDefaultAgent => config.default_agent = some_unless_blank(string),
                ProjectWorkflowPlugin => config.workflow_plugin = some_unless_blank(string),
                ProjectBaseBranch => config.base_branch = some_unless_blank(string),
                ProjectWorktreeDir => config.worktree_dir = some_unless_blank(string),
                ProjectBranchPrefix => config.branch_prefix = some_unless_blank(string),
                ProjectGithubUrl => config.github_url = some_unless_blank(string),
                ProjectCopyFiles => config.copy_files = some_unless_blank(string),
                ProjectInitScript => config.init_script = some_unless_blank(string),
                ProjectCleanupScript => config.cleanup_script = some_unless_blank(string),
                ProjectSkipWorktree => {
                    config.skip_worktree = match string.as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    }
                }
                _ => unreachable!("outer match already narrowed to project fields"),
            }
        }
    }
}

#[cfg(test)]
#[path = "config_editor_tests.rs"]
mod tests;
