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
