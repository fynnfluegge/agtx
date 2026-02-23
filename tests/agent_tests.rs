use agtx::agent::parse_agent_selection;

#[test]
fn test_parse_agent_selection_empty_defaults_to_first() {
    assert_eq!(parse_agent_selection("", 3), Some(0));
    assert_eq!(parse_agent_selection("  ", 3), Some(0));
    assert_eq!(parse_agent_selection("\n", 3), Some(0));
}

#[test]
fn test_parse_agent_selection_valid_numbers() {
    assert_eq!(parse_agent_selection("1", 3), Some(0));
    assert_eq!(parse_agent_selection("2", 3), Some(1));
    assert_eq!(parse_agent_selection("3", 3), Some(2));
}

#[test]
fn test_parse_agent_selection_trims_whitespace() {
    assert_eq!(parse_agent_selection(" 2 ", 3), Some(1));
    assert_eq!(parse_agent_selection("1\n", 3), Some(0));
}

#[test]
fn test_parse_agent_selection_out_of_range() {
    assert_eq!(parse_agent_selection("0", 3), None);
    assert_eq!(parse_agent_selection("4", 3), None);
    assert_eq!(parse_agent_selection("100", 3), None);
}

#[test]
fn test_parse_agent_selection_invalid_input() {
    assert_eq!(parse_agent_selection("abc", 3), None);
    assert_eq!(parse_agent_selection("-1", 3), None);
    assert_eq!(parse_agent_selection("1.5", 3), None);
}

#[test]
fn test_parse_agent_selection_single_agent() {
    assert_eq!(parse_agent_selection("1", 1), Some(0));
    assert_eq!(parse_agent_selection("2", 1), None);
    assert_eq!(parse_agent_selection("", 1), Some(0));
}
