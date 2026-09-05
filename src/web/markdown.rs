use pulldown_cmark::{html, Options, Parser};

pub fn render_markdown(content: &str) -> String {
    let stripped = crate::skills::strip_frontmatter(content);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(stripped, opts);
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

#[cfg(test)]
mod tests {
    use super::render_markdown;

    #[test]
    fn render_markdown_strips_frontmatter() {
        let html = render_markdown("---\ntitle: Test\n---\n# Body\n");

        assert!(html.contains("<h1>Body</h1>"));
        assert!(!html.contains("title: Test"));
    }

    #[test]
    fn render_markdown_enables_tables_tasklists_and_strikethrough() {
        let html = render_markdown("| A | B |\n| - | - |\n| 1 | 2 |\n\n- [x] done\n\n~~old~~\n");

        assert!(html.contains("<table>"));
        assert!(html.contains("checkbox"));
        assert!(html.contains("<del>old</del>"));
    }
}
