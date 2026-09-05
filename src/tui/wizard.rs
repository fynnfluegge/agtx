//! The task creation / edit wizard: one state value for the whole flow.
//!
//! `WizardState` holds every field at once — both text inputs, both list picks,
//! the step. That is what makes back-navigation work: stepping back has to find
//! the earlier answers still intact.
//!
//! It is also why the "seeded" flags exist. A step re-entered from the right
//! must not re-initialise itself over what the user already chose there.

use std::collections::HashSet;

use super::text_input::TextInput;

/// One option offered by a picker, with the blurb shown beside it.
///
/// Shared by the agent step, the plugin step and the board's `P` popup: the
/// same kind of choice in three places.
#[derive(Debug, Clone)]
pub struct PickOption {
    /// What is stored. `""` means "none", which for a plugin is the agtx
    /// default.
    pub name: String,
    pub label: String,
    pub description: String,
    /// Currently active for this project, drawn with a check.
    pub active: bool,
}

impl PickOption {
    pub fn new(name: &str, label: &str, description: &str, active: bool) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            active,
        }
    }

    fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let needle = needle.to_lowercase();
        self.label.to_lowercase().contains(&needle)
            || self.description.to_lowercase().contains(&needle)
    }
}

/// A pick-one-from-a-list step, with an optional filter.
///
/// `selected` indexes `options`, not the filtered view, so a filter that
/// narrows and widens again leaves the cursor on the same option rather than on
/// whatever now sits at that position.
#[derive(Debug, Clone, Default)]
pub struct ListPick {
    pub options: Vec<PickOption>,
    pub selected: usize,
    /// `Some` while `/` filtering is open. Empty-but-present means "filtering,
    /// nothing typed yet", which still shows the filter in the border.
    pub filter: Option<TextInput>,
    seeded: bool,
}

impl ListPick {
    /// Indices of the options the filter admits, in order.
    pub fn matching(&self) -> Vec<usize> {
        let needle = self.filter.as_ref().map(|f| f.as_str()).unwrap_or("");
        self.options
            .iter()
            .enumerate()
            .filter(|(_, o)| o.matches(needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// Where the cursor sits within the filtered view, for the scrollbar.
    pub fn position(&self) -> usize {
        self.matching()
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0)
    }

    pub fn selected_option(&self) -> Option<&PickOption> {
        self.options.get(self.selected)
    }

    pub fn select_next(&mut self) {
        let visible = self.matching();
        if let Some(pos) = visible.iter().position(|i| *i == self.selected) {
            if let Some(next) = visible.get(pos + 1) {
                self.selected = *next;
            }
        } else if let Some(first) = visible.first() {
            self.selected = *first;
        }
    }

    pub fn select_prev(&mut self) {
        let visible = self.matching();
        if let Some(pos) = visible.iter().position(|i| *i == self.selected) {
            if pos > 0 {
                self.selected = visible[pos - 1];
            }
        } else if let Some(first) = visible.first() {
            self.selected = *first;
        }
    }

    pub fn cycle(&mut self) {
        let visible = self.matching();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        self.selected = visible[(pos + 1) % visible.len()];
    }

    pub fn start_filter(&mut self) {
        self.filter.get_or_insert_with(TextInput::new);
    }

    pub fn stop_filter(&mut self) {
        self.filter = None;
    }

    pub fn is_filtering(&self) -> bool {
        self.filter.is_some()
    }

    /// Move the cursor onto a visible option, if the filter has excluded it.
    /// Call after every change to the filter text.
    pub fn settle(&mut self) {
        let visible = self.matching();
        if visible.is_empty() {
            return;
        }
        if !visible.contains(&self.selected) {
            self.selected = visible[0];
        }
    }

    /// True the first time only, so a step seeds itself once per run.
    pub fn take_seed(&mut self) -> bool {
        !std::mem::replace(&mut self.seeded, true)
    }

    /// Whether the step is worth showing: one option is not a choice.
    pub fn is_a_choice(&self) -> bool {
        self.options.len() > 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Title,
    Agent,
    Plugin,
    Prompt,
}

impl WizardStep {
    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Agent => "Agent",
            Self::Plugin => "Plugin",
            Self::Prompt => "Prompt",
        }
    }

    /// Whether this step is a list rather than a text field.
    pub fn is_list(self) -> bool {
        matches!(self, Self::Agent | Self::Plugin)
    }
}

#[derive(Debug, Clone)]
pub struct WizardState {
    step: WizardStep,
    /// Which optional steps this run includes. Decided after the title step,
    /// once the options are known: with one agent or one plugin there is
    /// nothing to pick.
    agent_step: bool,
    plugin_step: bool,
    pub title: TextInput,
    pub prompt: TextInput,
    pub agent: ListPick,
    pub plugin: ListPick,
    /// `Some(id)` when editing an existing task, `None` when creating.
    pub editing_task_id: Option<String>,
    pub referenced_task_ids: HashSet<String>,
    /// File paths, skill commands and task references inserted through the
    /// `#` / `/` / `!` dropdowns, highlighted in the rendered prompt.
    pub highlighted_references: HashSet<String>,
    /// Why the last save or advance was refused. Cleared on the next keystroke,
    /// so it never outlives the mistake it describes.
    pub validation: Option<String>,
    /// The prompt has been loaded for this run. Re-entering the step must not
    /// reload it from the database, or an edit made before stepping back is
    /// silently discarded.
    prompt_seeded: bool,
}

impl WizardState {
    /// A fresh task.
    pub fn creating() -> Self {
        Self {
            step: WizardStep::Title,
            agent_step: false,
            plugin_step: false,
            title: TextInput::new(),
            prompt: TextInput::new(),
            agent: ListPick::default(),
            plugin: ListPick::default(),
            editing_task_id: None,
            referenced_task_ids: HashSet::new(),
            highlighted_references: HashSet::new(),
            validation: None,
            prompt_seeded: false,
        }
    }

    /// Editing an existing task, opened on its title.
    pub fn editing(task_id: impl Into<String>, title: impl Into<String>) -> Self {
        let mut wizard = Self::creating();
        wizard.editing_task_id = Some(task_id.into());
        wizard.title.set_text(title.into());
        wizard
    }

    pub fn step(&self) -> WizardStep {
        self.step
    }

    pub fn is_editing(&self) -> bool {
        self.editing_task_id.is_some()
    }

    /// The steps this run walks through, in order.
    pub fn steps(&self) -> Vec<WizardStep> {
        let mut steps = vec![WizardStep::Title];
        if self.agent_step {
            steps.push(WizardStep::Agent);
        }
        if self.plugin_step {
            steps.push(WizardStep::Plugin);
        }
        steps.push(WizardStep::Prompt);
        steps
    }

    /// Position of the current step within `steps`, for the breadcrumb.
    pub fn step_index(&self) -> usize {
        self.steps()
            .iter()
            .position(|s| *s == self.step)
            .unwrap_or(0)
    }

    pub fn on_first_step(&self) -> bool {
        self.step_index() == 0
    }

    pub fn on_last_step(&self) -> bool {
        self.step_index() + 1 == self.steps().len()
    }

    /// Include or drop an optional step, once its options are known.
    ///
    /// Dropping the step the cursor stands on would leave `step` outside
    /// `steps` and every index derived from it wrong, so it retreats to Title.
    pub fn set_optional_step(&mut self, step: WizardStep, enabled: bool) {
        match step {
            WizardStep::Agent => self.agent_step = enabled,
            WizardStep::Plugin => self.plugin_step = enabled,
            _ => return,
        }
        if !enabled && self.step == step {
            self.step = WizardStep::Title;
        }
    }

    /// The list the current step is picking from, if it is a list step.
    pub fn current_list(&self) -> Option<&ListPick> {
        match self.step {
            WizardStep::Agent => Some(&self.agent),
            WizardStep::Plugin => Some(&self.plugin),
            _ => None,
        }
    }

    pub fn current_list_mut(&mut self) -> Option<&mut ListPick> {
        match self.step {
            WizardStep::Agent => Some(&mut self.agent),
            WizardStep::Plugin => Some(&mut self.plugin),
            _ => None,
        }
    }

    /// Move to the next step. `false` when already on the last one — the
    /// caller saves instead.
    pub fn advance(&mut self) -> bool {
        let steps = self.steps();
        let index = self.step_index();
        match steps.get(index + 1) {
            Some(next) => {
                self.enter(*next);
                true
            }
            None => false,
        }
    }

    /// Move to the previous step. `false` when already on the first one — the
    /// caller cancels instead.
    pub fn back(&mut self) -> bool {
        let index = self.step_index();
        if index == 0 {
            return false;
        }
        let previous = self.steps()[index - 1];
        self.enter(previous);
        true
    }

    fn enter(&mut self, step: WizardStep) {
        // A filter belongs to the visit, not to the step: coming back to a list
        // shows the whole list again, not the last search.
        if let Some(list) = self.current_list_mut() {
            list.stop_filter();
        }
        self.step = step;
        self.validation = None;
    }

    /// The field the caret is in, or `None` on a list step.
    pub fn active_input(&self) -> Option<&TextInput> {
        match self.step {
            WizardStep::Title => Some(&self.title),
            WizardStep::Prompt => Some(&self.prompt),
            _ => None,
        }
    }

    pub fn active_input_mut(&mut self) -> Option<&mut TextInput> {
        match self.step {
            WizardStep::Title => Some(&mut self.title),
            WizardStep::Prompt => Some(&mut self.prompt),
            _ => None,
        }
    }

    /// A task needs a title and nothing else; the prompt, agent and plugin all
    /// have defaults. This is what `Ctrl+S` checks from any step.
    pub fn can_save(&self) -> bool {
        !self.title.as_str().trim().is_empty()
    }

    /// The plugin to stamp on the task. `None` is the agtx default.
    pub fn plugin_name(&self) -> Option<&str> {
        self.plugin
            .selected_option()
            .map(|o| o.name.as_str())
            .filter(|name| !name.is_empty())
    }

    pub fn plugin_label(&self) -> &str {
        self.plugin
            .selected_option()
            .map(|o| o.label.as_str())
            .unwrap_or("agtx")
    }

    /// The agent to run the task with, when the step offered a choice.
    pub fn agent_name(&self) -> Option<&str> {
        self.agent
            .selected_option()
            .map(|o| o.name.as_str())
            .filter(|name| !name.is_empty())
    }

    pub fn take_prompt_seed(&mut self) -> bool {
        !std::mem::replace(&mut self.prompt_seeded, true)
    }
}

#[cfg(test)]
#[path = "wizard_tests.rs"]
mod tests;
