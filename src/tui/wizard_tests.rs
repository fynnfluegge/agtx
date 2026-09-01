use super::*;

fn opt(name: &str) -> PickOption {
    PickOption::new(name, name, "", false)
}

#[test]
fn a_new_wizard_starts_on_the_title() {
    let wizard = WizardState::creating();
    assert_eq!(wizard.step(), WizardStep::Title);
    assert!(wizard.on_first_step());
    assert!(!wizard.is_editing());
}

#[test]
fn editing_opens_on_the_title_already_filled_in() {
    let wizard = WizardState::editing("t-1", "Fix login");
    assert!(wizard.is_editing());
    assert_eq!(wizard.title.as_str(), "Fix login");
    assert_eq!(wizard.title.cursor, 9, "caret at the end, ready to append");
    assert_eq!(wizard.step(), WizardStep::Title);
}

#[test]
fn the_plugin_step_is_skipped_when_it_is_not_enabled() {
    let mut wizard = WizardState::creating();
    assert_eq!(wizard.steps(), &[WizardStep::Title, WizardStep::Prompt]);
    assert!(wizard.advance());
    assert_eq!(wizard.step(), WizardStep::Prompt);
    assert!(wizard.on_last_step());
}

#[test]
fn the_plugin_step_sits_between_the_other_two_when_enabled() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    assert_eq!(
        wizard.steps(),
        &[WizardStep::Title, WizardStep::Plugin, WizardStep::Prompt]
    );
    assert!(wizard.advance());
    assert_eq!(wizard.step(), WizardStep::Plugin);
    assert!(wizard.advance());
    assert_eq!(wizard.step(), WizardStep::Prompt);
}

/// The point of the whole restructure: a typo caught on the last step is one
/// keypress from being fixed, not a reason to start over.
#[test]
fn stepping_back_keeps_both_fields() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    wizard.title.set_text("Fix login");
    wizard.advance();
    wizard.advance();
    wizard.prompt.set_text("the session cookie expires early");

    assert!(wizard.back());
    assert_eq!(wizard.step(), WizardStep::Plugin);
    assert!(wizard.back());
    assert_eq!(wizard.step(), WizardStep::Title);
    assert_eq!(wizard.title.as_str(), "Fix login");
    assert_eq!(wizard.prompt.as_str(), "the session cookie expires early");
}

#[test]
fn advancing_past_the_last_step_is_declined() {
    let mut wizard = WizardState::creating();
    wizard.advance();
    assert!(wizard.on_last_step());
    assert!(
        !wizard.advance(),
        "the caller saves rather than moving further"
    );
    assert_eq!(wizard.step(), WizardStep::Prompt);
}

#[test]
fn stepping_back_from_the_first_step_is_declined() {
    let mut wizard = WizardState::creating();
    assert!(
        !wizard.back(),
        "the caller cancels rather than moving further"
    );
    assert_eq!(wizard.step(), WizardStep::Title);
}

/// Disabling the step while standing on it would leave `step` outside `steps`,
/// and every index derived from it wrong.
#[test]
fn disabling_the_plugin_step_moves_off_it() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    wizard.advance();
    assert_eq!(wizard.step(), WizardStep::Plugin);

    wizard.set_optional_step(WizardStep::Plugin, false);
    assert_eq!(wizard.step(), WizardStep::Title);
    assert_eq!(wizard.step_index(), 0);
}

#[test]
fn the_breadcrumb_index_follows_the_actual_flow() {
    let mut wizard = WizardState::creating();
    assert_eq!(wizard.step_index(), 0);
    wizard.advance();
    assert_eq!(
        wizard.step_index(),
        1,
        "the prompt is step 2 of 2 without a plugin step"
    );

    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    wizard.advance();
    wizard.advance();
    assert_eq!(wizard.step_index(), 2);
}

// --- the active field ---

#[test]
fn the_active_field_follows_the_step() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    wizard.title.set_text("t");
    wizard.prompt.set_text("p");

    assert_eq!(wizard.active_input().map(|i| i.as_str()), Some("t"));
    wizard.advance();
    assert!(
        wizard.active_input().is_none(),
        "the plugin step is a list, not a text field"
    );
    wizard.advance();
    assert_eq!(wizard.active_input().map(|i| i.as_str()), Some("p"));
}

#[test]
fn the_active_field_is_the_one_edited() {
    let mut wizard = WizardState::creating();
    wizard.active_input_mut().unwrap().insert_char('a');
    wizard.advance();
    wizard.active_input_mut().unwrap().insert_char('b');
    assert_eq!(wizard.title.as_str(), "a");
    assert_eq!(wizard.prompt.as_str(), "b");
}

// --- saving ---

#[test]
fn saving_needs_a_title_and_nothing_else() {
    let mut wizard = WizardState::creating();
    assert!(!wizard.can_save());
    wizard.title.set_text("   ");
    assert!(!wizard.can_save(), "whitespace is not a title");
    wizard.title.set_text("Fix login");
    assert!(
        wizard.can_save(),
        "the prompt and plugin both have defaults"
    );
}

// --- plugin selection ---

#[test]
fn plugin_selection_clamps_at_both_ends() {
    let mut wizard = WizardState::creating();
    wizard.plugin.options = vec![opt("agtx"), opt("gsd")];

    wizard.plugin.select_prev();
    assert_eq!(wizard.plugin.selected, 0);
    wizard.plugin.select_next();
    assert_eq!(wizard.plugin.selected, 1);
    wizard.plugin.select_next();
    assert_eq!(wizard.plugin.selected, 1, "clamped at the last option");
}

#[test]
fn cycling_wraps_where_selection_clamps() {
    let mut wizard = WizardState::creating();
    wizard.plugin.options = vec![opt("agtx"), opt("gsd")];
    wizard.plugin.cycle();
    assert_eq!(wizard.plugin.selected, 1);
    wizard.plugin.cycle();
    assert_eq!(wizard.plugin.selected, 0);
}

#[test]
fn cycling_an_empty_list_does_not_divide_by_zero() {
    let mut wizard = WizardState::creating();
    wizard.plugin.cycle();
    assert_eq!(wizard.plugin.selected, 0);
}

/// "agtx" is the default, stored as no plugin at all — the name is only
/// recorded on the task when it is something else.
#[test]
fn the_plugin_name_is_none_when_the_option_carries_no_name() {
    let mut wizard = WizardState::creating();
    wizard.plugin.options = vec![PickOption::new("", "agtx", "", true)];
    assert_eq!(wizard.plugin_name(), None);
    assert_eq!(wizard.plugin_label(), "agtx");

    wizard.plugin.options.push(opt("gsd"));
    wizard.plugin.selected = 1;
    assert_eq!(wizard.plugin_name(), Some("gsd"));
}

// --- seeding ---

/// Both guards exist because of back-navigation: re-entering a step from the
/// right must not overwrite what the user did the first time through.
#[test]
fn each_step_seeds_itself_exactly_once() {
    let mut wizard = WizardState::creating();
    assert!(wizard.plugin.take_seed());
    assert!(!wizard.plugin.take_seed());
    assert!(!wizard.plugin.take_seed());

    assert!(wizard.take_prompt_seed());
    assert!(!wizard.take_prompt_seed());
}

// --- validation ---

#[test]
fn a_validation_message_does_not_survive_a_step_change() {
    let mut wizard = WizardState::creating();
    wizard.title.set_text("t");
    wizard.validation = Some("nope".to_string());
    wizard.advance();
    assert!(wizard.validation.is_none());

    wizard.validation = Some("nope".to_string());
    wizard.back();
    assert!(wizard.validation.is_none());
}

// --- filtering ---

fn picker() -> ListPick {
    let mut list = ListPick::default();
    list.options = vec![
        PickOption::new("agtx", "agtx", "Built-in workflow", true),
        PickOption::new("gsd", "gsd", "Get Shit Done", false),
        PickOption::new("spec-kit", "spec-kit", "GitHub spec-kit workflow", false),
        PickOption::new("bmad", "bmad", "AI-driven agile development", false),
    ];
    list
}

#[test]
fn an_unfiltered_list_shows_everything() {
    let list = picker();
    assert!(!list.is_filtering());
    assert_eq!(list.matching().len(), 4);
}

#[test]
fn a_filter_narrows_by_label_and_by_description() {
    let mut list = picker();
    list.start_filter();
    list.filter.as_mut().unwrap().set_text("spec");
    // "spec-kit" by label, and by its description too — one match either way.
    assert_eq!(list.matching(), vec![2]);

    list.filter.as_mut().unwrap().set_text("workflow");
    assert_eq!(
        list.matching(),
        vec![0, 2],
        "matching on the description as well as the name"
    );
}

#[test]
fn filtering_is_case_insensitive() {
    let mut list = picker();
    list.start_filter();
    list.filter.as_mut().unwrap().set_text("GSD");
    assert_eq!(list.matching(), vec![1]);
}

/// `selected` indexes the options, not the filtered view, so narrowing and
/// widening again leaves the cursor on the same option rather than on whatever
/// happens to sit at that position now.
#[test]
fn the_cursor_stays_on_the_same_option_across_a_filter() {
    let mut list = picker();
    list.selected = 2;
    list.start_filter();
    list.filter.as_mut().unwrap().set_text("spec");
    list.settle();
    assert_eq!(list.selected, 2);

    list.filter.as_mut().unwrap().set_text("");
    list.settle();
    assert_eq!(list.selected, 2, "still spec-kit, not the third row");
}

/// When the filter excludes the current pick, the cursor has to move somewhere
/// real — an Enter on an invisible selection would pick something the user
/// cannot see.
#[test]
fn a_filter_that_excludes_the_pick_moves_it_to_the_first_match() {
    let mut list = picker();
    list.selected = 0;
    list.start_filter();
    list.filter.as_mut().unwrap().set_text("gsd");
    list.settle();
    assert_eq!(list.selected, 1);
}

#[test]
fn navigation_moves_within_the_filtered_view() {
    let mut list = picker();
    list.start_filter();
    list.filter.as_mut().unwrap().set_text("d"); // gsd, bmad ("...Done", "...development")
    list.settle();
    let visible = list.matching();
    assert!(visible.len() >= 2, "{visible:?}");

    list.selected = visible[0];
    list.select_next();
    assert_eq!(list.selected, visible[1], "skips the filtered-out rows");
    list.select_next();
    if visible.len() == 2 {
        assert_eq!(list.selected, visible[1], "clamped at the last match");
    }
    list.select_prev();
    assert_eq!(list.selected, visible[0]);
}

#[test]
fn a_filter_matching_nothing_leaves_the_pick_alone() {
    let mut list = picker();
    list.selected = 1;
    list.start_filter();
    list.filter.as_mut().unwrap().set_text("zzzz");
    list.settle();
    assert!(list.matching().is_empty());
    assert_eq!(list.selected, 1, "nothing to move to, so nothing moves");
}

#[test]
fn the_scrollbar_position_is_an_index_into_the_visible_rows() {
    let mut list = picker();
    list.selected = 3;
    assert_eq!(list.position(), 3);

    list.start_filter();
    list.filter.as_mut().unwrap().set_text("d");
    list.settle();
    let visible = list.matching();
    list.selected = *visible.last().unwrap();
    assert_eq!(list.position(), visible.len() - 1);
}

/// A filter belongs to the visit, not to the step: coming back to a list should
/// show the whole list again, not the last search.
#[test]
fn stepping_away_clears_the_filter() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    wizard.title.set_text("t");
    wizard.advance();
    wizard.plugin.start_filter();
    wizard.plugin.filter.as_mut().unwrap().set_text("gsd");

    wizard.advance();
    wizard.back();
    assert!(!wizard.plugin.is_filtering(), "the filter did not survive");
}

// --- the agent step ---

#[test]
fn the_agent_step_sits_between_the_title_and_the_plugin() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Agent, true);
    wizard.set_optional_step(WizardStep::Plugin, true);
    assert_eq!(
        wizard.steps(),
        vec![
            WizardStep::Title,
            WizardStep::Agent,
            WizardStep::Plugin,
            WizardStep::Prompt
        ]
    );
}

#[test]
fn either_optional_step_can_be_absent_on_its_own() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Agent, true);
    assert_eq!(
        wizard.steps(),
        vec![WizardStep::Title, WizardStep::Agent, WizardStep::Prompt]
    );

    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Plugin, true);
    assert_eq!(
        wizard.steps(),
        vec![WizardStep::Title, WizardStep::Plugin, WizardStep::Prompt]
    );
}

#[test]
fn the_agent_name_is_none_until_the_step_is_seeded() {
    let wizard = WizardState::creating();
    assert_eq!(
        wizard.agent_name(),
        None,
        "so the caller falls back to config"
    );
}

#[test]
fn only_a_list_step_has_a_list() {
    let mut wizard = WizardState::creating();
    wizard.set_optional_step(WizardStep::Agent, true);
    assert!(wizard.current_list().is_none(), "the title is a text field");
    wizard.advance();
    assert!(wizard.current_list().is_some());
    wizard.advance();
    assert!(wizard.current_list().is_none(), "so is the prompt");
}

#[test]
fn a_single_option_is_not_a_choice() {
    let mut list = ListPick::default();
    assert!(!list.is_a_choice());
    list.options = vec![PickOption::new("a", "a", "", true)];
    assert!(!list.is_a_choice());
    list.options.push(PickOption::new("b", "b", "", false));
    assert!(list.is_a_choice());
}
