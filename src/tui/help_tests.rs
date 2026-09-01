use super::*;

#[test]
fn every_entry_says_both_what_and_which() {
    for section in HELP {
        assert!(!section.title.is_empty());
        assert!(
            !section.entries.is_empty(),
            "section `{}` lists nothing",
            section.title
        );
        for entry in section.entries {
            assert!(!entry.keys.is_empty(), "in `{}`", section.title);
            assert!(!entry.action.is_empty(), "in `{}`", section.title);
        }
    }
}

#[test]
fn section_titles_are_unique() {
    let mut seen: Vec<&str> = Vec::new();
    for section in HELP {
        assert!(
            !seen.contains(&section.title),
            "duplicate section `{}`",
            section.title
        );
        seen.push(section.title);
    }
}

/// The overlay exists because the footer could not show everything. These are
/// the keys that were bound but advertised nowhere; if one is dropped from the
/// table it goes back to being undiscoverable.
#[test]
fn the_previously_unadvertised_keys_are_listed() {
    let listed: Vec<&str> = HELP
        .iter()
        .flat_map(|s| s.entries.iter().map(|e| e.keys))
        .collect();
    for key in ["C-n / C-p", "M", "D", "C-g", "C-q"] {
        assert!(
            listed.iter().any(|k| *k == key),
            "`{key}` is bound but not in the help table: {listed:?}"
        );
    }
}

#[test]
fn rows_render_titles_flush_and_entries_indented() {
    let rows = rows();
    let first = &rows[0];
    assert!(!first.0, "a section title is not indented");
    assert_eq!(first.1, HELP[0].title);

    let second = &rows[1];
    assert!(second.0, "an entry is indented");
    assert!(second.1.starts_with(HELP[0].entries[0].keys));
    assert!(second.1.contains(HELP[0].entries[0].action));
}

/// Sections are separated by a blank row, so the scroll offset can stay a plain
/// row index rather than needing to know about section structure.
#[test]
fn sections_are_separated_by_a_blank_row() {
    let rows = rows();
    let blanks = rows.iter().filter(|(_, text)| text.is_empty()).count();
    assert_eq!(blanks, HELP.len() - 1);
}

#[test]
fn the_content_width_covers_the_longest_row() {
    let width = content_width();
    for (indent, text) in rows() {
        let needed = text.chars().count() + if indent { 2 } else { 0 };
        assert!(needed <= width, "{needed} > {width} for {text:?}");
    }
}

/// The overlay is itself scrollable, and documents its own keys — a reference
/// that does not say how to read past its first screen is not much of one.
#[test]
fn the_overlay_documents_its_own_scrolling() {
    let section = HELP
        .iter()
        .find(|s| s.title == "This overlay")
        .expect("the overlay should list its own keys");
    let listed: Vec<&str> = section.entries.iter().map(|e| e.keys).collect();
    assert!(listed.iter().any(|k| k.contains("C-d")), "{listed:?}");
    assert!(listed.iter().any(|k| k.contains("Esc")), "{listed:?}");
}

/// Every scrollable surface uses the same chords, so both sections say so.
#[test]
fn the_scroll_chords_match_between_the_pane_and_the_overlay() {
    let chords = |title: &str| -> Vec<&str> {
        HELP.iter()
            .find(|s| s.title == title)
            .map(|s| s.entries.iter().map(|e| e.keys).collect())
            .unwrap_or_default()
    };
    for chord in ["C-d / C-u", "C-n / C-p"] {
        assert!(chords("Task pane").contains(&chord), "task pane: {chord}");
        assert!(chords("This overlay").contains(&chord), "overlay: {chord}");
    }
}

// --- columns ---

/// One column is the whole table, in order.
#[test]
fn a_single_column_holds_everything() {
    let columns = columns(1);
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0], rows());
}

/// Two columns roughly halve the height, which is the whole point: in one
/// column the reference is a tall thin strip that has to be scrolled even in a
/// large window.
#[test]
fn two_columns_are_roughly_balanced() {
    let columns = columns(2);
    assert_eq!(columns.len(), 2);
    let (a, b) = (columns[0].len(), columns[1].len());
    assert!(a > 0 && b > 0, "both columns carry something: {a}, {b}");

    let single = rows().len();
    assert!(
        a.max(b) < single * 3 / 4,
        "the taller column ({}) should be well under the single-column height ({single})",
        a.max(b)
    );
    assert!(
        a.abs_diff(b) <= single / 4,
        "columns are lopsided: {a} vs {b}"
    );
}

/// Nothing is lost or duplicated in the split.
#[test]
fn splitting_preserves_every_entry() {
    let flat: Vec<Row> = rows().into_iter().filter(|(_, t)| !t.is_empty()).collect();
    let mut split: Vec<Row> = columns(2)
        .into_iter()
        .flatten()
        .filter(|(_, t)| !t.is_empty())
        .collect();
    split.sort();
    let mut flat = flat;
    flat.sort();
    assert_eq!(flat, split);
}

/// A heading in one column with its keys in the next is worse than an uneven
/// pair of columns, so sections stay whole.
#[test]
fn a_section_is_never_split_across_columns() {
    for column in columns(2) {
        // Walk the column: every title must be followed by its own entries, and
        // a column must not open with an orphaned entry.
        assert!(
            column.first().map(|(indent, _)| !*indent).unwrap_or(true),
            "a column starts with an entry whose title is elsewhere"
        );
    }
    // And every title in HELP appears exactly once across the two columns.
    let titles: Vec<String> = columns(2)
        .into_iter()
        .flatten()
        .filter(|(indent, text)| !*indent && !text.is_empty())
        .map(|(_, text)| text)
        .collect();
    assert_eq!(titles.len(), HELP.len());
    for section in HELP {
        assert!(
            titles.iter().any(|t| t == section.title),
            "{}",
            section.title
        );
    }
}

#[test]
fn asking_for_no_columns_still_gives_one() {
    assert_eq!(columns(0).len(), 1);
}
