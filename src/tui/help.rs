//! The `?` overlay: every binding, grouped by where it applies.
//!
//! This table is the complete list. `build_footer_text` shows a contextual
//! handful, because the footer is one line and gets truncated on a narrow
//! terminal — so a binding that is not here is undiscoverable.

pub struct HelpEntry {
    pub keys: &'static str,
    pub action: &'static str,
}

pub struct HelpSection {
    pub title: &'static str,
    pub entries: &'static [HelpEntry],
}

const fn e(keys: &'static str, action: &'static str) -> HelpEntry {
    HelpEntry { keys, action }
}

pub const HELP: &[HelpSection] = &[
    HelpSection {
        title: "Board",
        entries: &[
            e("h j k l  ←↓↑→", "Move between columns and tasks"),
            e("o", "New task"),
            e("Enter", "Open the task's pane — or edit it, in Backlog"),
            e("C-f", "Open the task's pane fullscreen"),
            e("d", "Show the task's git diff"),
            e("D", "Dependency graph overlay"),
            e("W", "Serve the board to a phone, and manage paired devices"),
            e("/", "Search tasks"),
            e("x", "Delete task"),
            e("q", "Quit"),
        ],
    },
    HelpSection {
        title: "Moving tasks",
        entries: &[
            e("m", "Move forward one column"),
            e("M", "Backlog straight to Running"),
            e("R", "Start research on a Backlog task, in place"),
            e("r", "Review → Running, or Running → Planning"),
            e("p", "Review → Planning, next phase (cyclic plugins)"),
        ],
    },
    HelpSection {
        title: "Project",
        entries: &[
            e(",", "Configuration"),
            e("P", "Select the workflow plugin"),
            e("e", "Toggle the project sidebar"),
            e("u", "Install the update, when one was found"),
            e("O", "Toggle the orchestrator (--experimental)"),
        ],
    },
    HelpSection {
        title: "Task pane",
        entries: &[
            e("C-d / C-u", "Page down / up"),
            e("PageDown / PageUp", "Page down / up"),
            e("C-n / C-p", "Scroll five lines down / up"),
            e("C-g", "Jump to the bottom"),
            e("C-f", "Toggle fullscreen"),
            e("C-q", "Close the pane"),
            e("anything else", "Forwarded to the agent, Esc included"),
        ],
    },
    HelpSection {
        title: "New task",
        entries: &[
            e("Enter", "Next step — or save, on the prompt"),
            e("S-Tab / C-b", "Back one step"),
            e("C-s", "Save from any step"),
            e("Esc", "Cancel"),
            e("\\ + Enter", "Newline in the prompt (also C-j, Alt+Enter)"),
            e("#  @", "Insert a file path"),
            e("/", "Insert a skill command"),
            e("!", "Reference another task, creating a dependency"),
        ],
    },
    HelpSection {
        title: "Configuration",
        entries: &[
            e("h / l   Tab", "Move between sections"),
            e("j / k", "Move between fields"),
            e("Enter / Space", "Toggle, or open the field"),
            e("C-s", "Save"),
            e("Esc", "Close, asking first if anything changed"),
        ],
    },
    HelpSection {
        title: "This overlay",
        entries: &[
            e("C-d / C-u", "Page down / up"),
            e("C-n / C-p", "Scroll five lines down / up"),
            e("j / k", "Scroll one line"),
            e("C-g / Home", "Jump to the bottom / top"),
            e("? / Esc / q", "Close"),
        ],
    },
    HelpSection {
        title: "Mobile (W)",
        entries: &[
            e("s", "Start or stop serving"),
            e("j k  ↓↑", "Select a paired device"),
            e("x", "Revoke the selected device"),
            e("r", "Reload the device list"),
            e("Esc q W", "Close — serving continues"),
        ],
    },
    HelpSection {
        title: "Dependency graph",
        entries: &[
            e("h j k l", "Move between nodes and levels"),
            e("Space", "Mark an unblocked node"),
            e("a / c", "Mark all unblocked / clear marks"),
            e("Enter", "Move the marked tasks forward"),
            e("Esc / q", "Close"),
        ],
    },
];

/// One rendered row: whether it is indented (an entry rather than a section
/// title) and its text.
pub type Row = (bool, String);

fn section_rows(section: &HelpSection) -> Vec<Row> {
    let mut out = vec![(false, section.title.to_string())];
    for entry in section.entries {
        out.push((true, format!("{:<18}{}", entry.keys, entry.action)));
    }
    out
}

/// Rows the overlay draws, as one flat list.
pub fn rows() -> Vec<Row> {
    columns(1).into_iter().next().unwrap_or_default()
}

/// Split the table across `count` columns, balanced by height.
///
/// Sections stay whole: a heading in one column with its keys in the next is
/// worse than an uneven pair. Two columns roughly halve the height, which is
/// what lets the whole reference fit a normal window without scrolling.
pub fn columns(count: usize) -> Vec<Vec<Row>> {
    let count = count.max(1);
    let blocks: Vec<Vec<Row>> = HELP.iter().map(section_rows).collect();
    if count == 1 {
        return vec![join(&blocks)];
    }

    if blocks.is_empty() {
        return vec![Vec::new(); count];
    }
    let total: usize = blocks.iter().map(|b| b.len()).sum::<usize>() + blocks.len() - 1;
    let target = total.div_ceil(count);

    let mut out: Vec<Vec<Vec<Row>>> = vec![Vec::new(); count];
    let mut column = 0usize;
    let mut height = 0usize;
    for (i, block) in blocks.iter().enumerate() {
        let remaining_columns = count - column - 1;
        let remaining_blocks = blocks.len() - i;
        let would_be = height + block.len() + usize::from(height > 0);
        // Break when taking the block lands further from the target height than
        // leaving it. `abs_diff` on both sides: `height` can already be past
        // target, where a plain subtraction underflows.
        let overshoots = height > 0 && target.abs_diff(would_be) > target.abs_diff(height);
        if overshoots && remaining_columns > 0 && remaining_blocks > remaining_columns {
            column += 1;
            height = 0;
        }
        height += block.len() + usize::from(height > 0);
        out[column].push(block.clone());
    }

    out.iter().map(|blocks| join(blocks)).collect()
}

/// Concatenate section blocks with a blank row between them.
fn join(blocks: &[Vec<Row>]) -> Vec<Row> {
    let mut out = Vec::new();
    for block in blocks {
        if !out.is_empty() {
            out.push((false, String::new()));
        }
        out.extend(block.iter().cloned());
    }
    out
}

/// The width one column needs, in cells, before borders and padding.
pub fn content_width() -> usize {
    HELP.iter()
        .flat_map(section_rows)
        .map(|(indent, text)| text.chars().count() + if indent { 2 } else { 0 })
        .max()
        .unwrap_or(40)
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
