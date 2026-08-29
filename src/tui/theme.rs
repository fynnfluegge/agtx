use ratatui::prelude::*;

use crate::config::ThemeConfig;

fn color(hex: &str) -> Color {
    ThemeConfig::parse_hex(hex)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .unwrap_or(Color::White)
}

/// Semantic styles shared by the board chrome and cards. Keeping these names
/// independent of individual widgets makes the existing palette usable by a
/// less border-heavy visual language.
#[derive(Clone, Copy)]
pub struct TuiStyles {
    pub selected: Color,
    pub dimmed: Color,
    pub text: Color,
    pub description: Color,
    pub column_header: Color,
}

impl TuiStyles {
    pub fn from_theme(theme: &ThemeConfig) -> Self {
        Self {
            selected: color(&theme.color_selected),
            dimmed: color(&theme.color_dimmed),
            text: color(&theme.color_text),
            description: color(&theme.color_description),
            column_header: color(&theme.color_column_header),
        }
    }

    pub fn keycap(self) -> Style {
        Style::default().fg(self.selected).bold()
    }

    pub fn muted(self) -> Style {
        Style::default().fg(self.dimmed)
    }
}
