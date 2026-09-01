use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn editor() -> ConfigEditor {
    ConfigEditor::open(
        GlobalConfig::default(),
        None,
        &["claude".to_string(), "codex".to_string()],
        &["agtx".to_string(), "gsd".to_string()],
    )
}

fn editor_with_project() -> ConfigEditor {
    ConfigEditor::open(
        GlobalConfig::default(),
        Some(ProjectConfig::default()),
        &["claude".to_string(), "codex".to_string()],
        &["agtx".to_string(), "gsd".to_string()],
    )
}

/// Jump straight to a field, so a test about that field is not also a test of
/// how many times `j` has to be pressed to reach it.
fn focus(ed: &mut ConfigEditor, id: FieldId) {
    for section in 0..ed.sections.len() {
        if let Some(field) = ed.sections[section].fields.iter().position(|f| f.id == id) {
            ed.section = section;
            ed.field = field;
            return;
        }
    }
    panic!("no field {id:?} in the form");
}

// --- the field table ---

/// `read` and `write` are the two halves of one contract. A field wired into
/// one and not the other is a setting that silently will not stick.
#[test]
fn every_field_round_trips_through_read_and_write() {
    let mut ed = editor_with_project();
    let ids: Vec<FieldId> = ed
        .sections
        .iter()
        .flat_map(|s| s.fields.iter().map(|f| (f.id, f.kind.clone())))
        .map(|(id, _)| id)
        .collect();

    for id in ids {
        let kind = ed
            .sections
            .iter()
            .flat_map(|s| s.fields.iter())
            .find(|f| f.id == id)
            .unwrap()
            .kind
            .clone();

        let written = match &kind {
            FieldKind::Toggle => FieldValue::Bool(!ed.value(id).as_bool()),
            // The last choice is the one least likely to already be selected.
            FieldKind::Choice(choices) => FieldValue::Text(choices.last().unwrap().value.clone()),
            FieldKind::Color => FieldValue::Text("#123456".to_string()),
            FieldKind::Text => FieldValue::Text("probe-value".to_string()),
        };
        ed.set(id, written.clone());
        assert_eq!(ed.value(id), written, "{id:?} did not survive a write");
    }
}

#[test]
fn the_project_section_only_exists_when_a_project_is_open() {
    assert!(!editor().sections.iter().any(|s| s.title == "Project"));
    assert!(editor_with_project()
        .sections
        .iter()
        .any(|s| s.title == "Project"));
}

/// Writing a project field with no project open must not panic, and must not
/// leave the form claiming unsaved changes it does not have.
#[test]
fn project_fields_are_inert_without_a_project() {
    let mut ed = editor();
    ed.set(FieldId::ProjectInitScript, FieldValue::Text("x".into()));
    assert!(!ed.dirty);
}

/// `dirty` tracks what is stored, not what was asked for: clearing a field to
/// blank stores `None`, and that is a real change even though the *typed* value
/// and the read-back value are both empty.
#[test]
fn clearing_a_set_field_counts_as_a_change() {
    let mut ed = editor_with_project();
    ed.set(
        FieldId::ProjectInitScript,
        FieldValue::Text("npm ci".into()),
    );
    ed.mark_saved("saved");
    assert!(!ed.dirty);

    ed.set(FieldId::ProjectInitScript, FieldValue::Text(String::new()));
    assert!(ed.dirty, "unsetting a field is an edit");
    assert_eq!(ed.project.as_ref().unwrap().init_script, None);
}

// --- navigation ---

#[test]
fn sections_wrap_and_reset_the_field_cursor() {
    let mut ed = editor();
    ed.handle_key(key(KeyCode::Char('j')));
    assert_eq!(ed.field, 1);

    ed.handle_key(key(KeyCode::Char('l')));
    assert_eq!(ed.current_section().title, "Agents");
    assert_eq!(ed.field, 0, "a new section starts at its first field");

    ed.handle_key(key(KeyCode::Char('h')));
    assert_eq!(ed.current_section().title, "General");
    ed.handle_key(key(KeyCode::Char('h')));
    assert_eq!(ed.current_section().title, "Theme", "sections wrap");
}

#[test]
fn the_field_cursor_clamps_rather_than_wrapping() {
    let mut ed = editor();
    ed.handle_key(key(KeyCode::Char('k')));
    assert_eq!(ed.field, 0);
    for _ in 0..50 {
        ed.handle_key(key(KeyCode::Char('j')));
    }
    assert_eq!(ed.field, ed.current_section().fields.len() - 1);
}

// --- editing ---

#[test]
fn space_flips_a_toggle_and_marks_the_form_dirty() {
    let mut ed = editor();
    focus(&mut ed, FieldId::AutoTrust);
    assert!(!ed.value(FieldId::AutoTrust).as_bool());

    ed.handle_key(key(KeyCode::Char(' ')));
    assert!(ed.value(FieldId::AutoTrust).as_bool());
    assert!(ed.dirty);
}

#[test]
fn a_choice_field_opens_a_list_and_commits_the_pick() {
    let mut ed = editor();
    focus(&mut ed, FieldId::DefaultAgent);
    assert_eq!(ed.value(FieldId::DefaultAgent).as_text(), "claude");

    ed.handle_key(key(KeyCode::Enter));
    assert!(matches!(ed.editing, Some(EditState::Choice { .. })));
    ed.handle_key(key(KeyCode::Char('j')));
    ed.handle_key(key(KeyCode::Enter));

    assert!(ed.editing.is_none());
    assert_eq!(ed.value(FieldId::DefaultAgent).as_text(), "codex");
}

/// The list opens on whatever is already stored, so the first thing the cursor
/// is on is the current answer.
#[test]
fn a_choice_list_opens_on_the_current_value() {
    let mut ed = editor();
    ed.set(FieldId::DefaultAgent, FieldValue::Text("codex".into()));
    focus(&mut ed, FieldId::DefaultAgent);
    ed.handle_key(key(KeyCode::Enter));
    match ed.editing {
        Some(EditState::Choice { selected }) => assert_eq!(selected, 1),
        other => panic!("expected a choice list, got {other:?}"),
    }
}

#[test]
fn escape_abandons_an_edit_without_changing_the_value() {
    let mut ed = editor();
    focus(&mut ed, FieldId::WorktreeBaseBranch);
    ed.handle_key(key(KeyCode::Enter));
    ed.handle_key(key(KeyCode::Char('x')));
    ed.handle_key(key(KeyCode::Esc));

    assert!(ed.editing.is_none());
    assert_eq!(ed.value(FieldId::WorktreeBaseBranch).as_text(), "");
    assert!(!ed.dirty, "an abandoned edit is not a change");
}

/// Esc belongs to the open field first — closing the inline editor must not
/// also close the whole form.
#[test]
fn escape_in_an_open_field_does_not_close_the_editor() {
    let mut ed = editor();
    focus(&mut ed, FieldId::WorktreeBaseBranch);
    ed.handle_key(key(KeyCode::Enter));
    assert_eq!(ed.handle_key(key(KeyCode::Esc)), EditorAction::None);
}

#[test]
fn a_text_field_commits_what_was_typed() {
    let mut ed = editor();
    focus(&mut ed, FieldId::WorktreeBranchPrefix);
    ed.handle_key(key(KeyCode::Enter));
    for _ in 0.."task".len() {
        ed.handle_key(key(KeyCode::Backspace));
    }
    for c in "wip".chars() {
        ed.handle_key(key(KeyCode::Char(c)));
    }
    ed.handle_key(key(KeyCode::Enter));

    assert_eq!(ed.value(FieldId::WorktreeBranchPrefix).as_text(), "wip");
    assert!(ed.dirty);
}

/// A blank optional field means unset, and that is decided in one place.
#[test]
fn clearing_an_optional_text_field_unsets_it() {
    let mut ed = editor_with_project();
    ed.set(
        FieldId::ProjectInitScript,
        FieldValue::Text("npm ci".into()),
    );
    assert_eq!(
        ed.project.as_ref().unwrap().init_script.as_deref(),
        Some("npm ci")
    );

    ed.set(FieldId::ProjectInitScript, FieldValue::Text("   ".into()));
    assert_eq!(ed.project.as_ref().unwrap().init_script, None);
}

// --- colours ---

/// A bad colour would make the whole board unreadable on the next redraw, and
/// the value is typed by hand.
#[test]
fn a_bad_colour_is_refused_and_the_field_stays_open() {
    let mut ed = editor();
    focus(&mut ed, FieldId::ColorSelected);
    ed.handle_key(key(KeyCode::Enter));
    for _ in 0..8 {
        ed.handle_key(key(KeyCode::Backspace));
    }
    for c in "nope".chars() {
        ed.handle_key(key(KeyCode::Char(c)));
    }
    ed.handle_key(key(KeyCode::Enter));

    assert!(ed.editing.is_some(), "the field stays open to be fixed");
    assert!(ed.status.is_some(), "and says why");
    assert_eq!(ed.value(FieldId::ColorSelected).as_text(), "#ead49a");
}

#[test]
fn a_good_colour_is_accepted() {
    let mut ed = editor();
    focus(&mut ed, FieldId::ColorSelected);
    ed.handle_key(key(KeyCode::Enter));
    for _ in 0..8 {
        ed.handle_key(key(KeyCode::Backspace));
    }
    for c in "#00ff00".chars() {
        ed.handle_key(key(KeyCode::Char(c)));
    }
    ed.handle_key(key(KeyCode::Enter));

    assert!(ed.editing.is_none());
    assert_eq!(ed.value(FieldId::ColorSelected).as_text(), "#00ff00");
}

// --- leaving ---

#[test]
fn escape_closes_a_clean_form_immediately() {
    let mut ed = editor();
    assert_eq!(ed.handle_key(key(KeyCode::Esc)), EditorAction::Close);
    assert!(!ed.confirming_discard);
}

#[test]
fn escape_on_a_dirty_form_asks_first() {
    let mut ed = editor();
    focus(&mut ed, FieldId::AutoTrust);
    ed.handle_key(key(KeyCode::Char(' ')));

    assert_eq!(ed.handle_key(key(KeyCode::Esc)), EditorAction::None);
    assert!(ed.confirming_discard);

    assert_eq!(ed.handle_key(key(KeyCode::Char('y'))), EditorAction::Close);
}

#[test]
fn the_discard_prompt_offers_saving_instead() {
    let mut ed = editor();
    focus(&mut ed, FieldId::AutoTrust);
    ed.handle_key(key(KeyCode::Char(' ')));
    ed.handle_key(key(KeyCode::Esc));

    assert_eq!(ed.handle_key(key(KeyCode::Char('s'))), EditorAction::Save);
}

#[test]
fn any_other_key_backs_out_of_the_discard_prompt() {
    let mut ed = editor();
    focus(&mut ed, FieldId::AutoTrust);
    ed.handle_key(key(KeyCode::Char(' ')));
    ed.handle_key(key(KeyCode::Esc));

    assert_eq!(ed.handle_key(key(KeyCode::Esc)), EditorAction::None);
    assert!(!ed.confirming_discard);
    assert!(ed.dirty, "backing out keeps the changes");
}

#[test]
fn ctrl_s_asks_the_caller_to_save() {
    let mut ed = editor();
    assert_eq!(ed.handle_key(ctrl(KeyCode::Char('s'))), EditorAction::Save);
}

#[test]
fn marking_saved_clears_the_dirty_flag() {
    let mut ed = editor();
    focus(&mut ed, FieldId::AutoTrust);
    ed.handle_key(key(KeyCode::Char(' ')));
    assert!(ed.dirty);

    ed.mark_saved("Saved.");
    assert!(!ed.dirty);
    assert_eq!(ed.status.as_deref(), Some("Saved."));
    assert_eq!(
        ed.handle_key(key(KeyCode::Esc)),
        EditorAction::Close,
        "a saved form closes without asking"
    );
}

/// Re-setting a field to what it already held is not an edit, so it must not
/// arm the discard prompt.
#[test]
fn setting_a_value_to_itself_is_not_a_change() {
    let mut ed = editor();
    let current = ed.value(FieldId::DefaultAgent);
    ed.set(FieldId::DefaultAgent, current);
    assert!(!ed.dirty);
}
