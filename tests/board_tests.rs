use agtx::db::{Task, TaskStatus};
use agtx::tui::board::BoardState;

fn create_test_task(title: &str, status: TaskStatus) -> Task {
    let mut task = Task::new(title, "claude", "test-project");
    task.status = status;
    task
}

// === BoardState Tests ===

#[test]
fn test_board_state_new() {
    let board = BoardState::new();

    assert!(board.tasks.is_empty());
    assert_eq!(board.selected_column, 0);
    assert_eq!(board.selected_row, 0);
}

#[test]
fn test_board_state_default() {
    let board = BoardState::default();

    assert!(board.tasks.is_empty());
    assert_eq!(board.selected_column, 0);
    assert_eq!(board.selected_row, 0);
}

#[test]
fn test_tasks_in_column_empty() {
    let board = BoardState::new();

    for i in 0..6 {
        assert!(board.tasks_in_column(i).is_empty());
    }
}

#[test]
fn test_tasks_in_column_with_tasks() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
        create_test_task("Task 3", TaskStatus::Running),
        create_test_task("Task 4", TaskStatus::Done),
    ];

    assert_eq!(board.tasks_in_column(0).len(), 2); // Backlog
    assert_eq!(board.tasks_in_column(1).len(), 0); // Explore
    assert_eq!(board.tasks_in_column(2).len(), 0); // Planning
    assert_eq!(board.tasks_in_column(3).len(), 1); // Running
    assert_eq!(board.tasks_in_column(4).len(), 0); // Review
    assert_eq!(board.tasks_in_column(5).len(), 1); // Done
}

#[test]
fn test_tasks_in_column_invalid_column() {
    let board = BoardState::new();

    assert!(board.tasks_in_column(99).is_empty());
}

#[test]
fn test_selected_task_empty_board() {
    let board = BoardState::new();

    assert!(board.selected_task().is_none());
}

#[test]
fn test_selected_task_with_tasks() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
    ];
    board.selected_column = 0;
    board.selected_row = 1;

    let task = board.selected_task().unwrap();
    assert_eq!(task.title, "Task 2");
}

#[test]
fn test_move_left() {
    let mut board = BoardState::new();
    board.selected_column = 2;

    board.move_left();
    assert_eq!(board.selected_column, 1);

    board.move_left();
    assert_eq!(board.selected_column, 0);

    // Should not go below 0
    board.move_left();
    assert_eq!(board.selected_column, 0);
}

#[test]
fn test_move_right() {
    let mut board = BoardState::new();
    board.selected_column = 0;

    board.move_right();
    assert_eq!(board.selected_column, 1);

    board.move_right();
    assert_eq!(board.selected_column, 2);

    board.move_right();
    assert_eq!(board.selected_column, 3);

    board.move_right();
    assert_eq!(board.selected_column, 4);

    board.move_right();
    assert_eq!(board.selected_column, 5);

    // Should not go beyond last column
    board.move_right();
    assert_eq!(board.selected_column, 5);
}

#[test]
fn test_move_up() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
        create_test_task("Task 3", TaskStatus::Backlog),
    ];
    board.selected_row = 2;

    board.move_up();
    assert_eq!(board.selected_row, 1);

    board.move_up();
    assert_eq!(board.selected_row, 0);

    // Should not go below 0
    board.move_up();
    assert_eq!(board.selected_row, 0);
}

#[test]
fn test_move_down() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
        create_test_task("Task 3", TaskStatus::Backlog),
    ];
    board.selected_row = 0;

    board.move_down();
    assert_eq!(board.selected_row, 1);

    board.move_down();
    assert_eq!(board.selected_row, 2);

    // Should not go beyond last task
    board.move_down();
    assert_eq!(board.selected_row, 2);
}

#[test]
fn test_move_down_empty_column() {
    let mut board = BoardState::new();
    board.selected_row = 0;

    // Moving down in empty column should stay at 0
    board.move_down();
    assert_eq!(board.selected_row, 0);
}

#[test]
fn test_move_left_clamps_row() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
        create_test_task("Task 3", TaskStatus::Backlog),
        create_test_task("Task 4", TaskStatus::Planning), // Only 1 task in Planning
    ];
    board.selected_column = 0; // Backlog with 3 tasks
    board.selected_row = 2; // Last task in Backlog

    board.move_right(); // Move to Explore (column 1, empty)

    // Row should be clamped to 0 (Explore is empty)
    assert_eq!(board.selected_column, 1);
    assert_eq!(board.selected_row, 0);
}

#[test]
fn test_move_to_empty_column_clamps_row() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
        // Explore and Planning columns are empty
    ];
    board.selected_column = 0;
    board.selected_row = 1;

    board.move_right(); // Move to empty Explore column

    assert_eq!(board.selected_column, 1);
    assert_eq!(board.selected_row, 0);
}

#[test]
fn test_selected_task_mut() {
    let mut board = BoardState::new();
    board.tasks = vec![
        create_test_task("Task 1", TaskStatus::Backlog),
        create_test_task("Task 2", TaskStatus::Backlog),
    ];
    board.selected_column = 0;
    board.selected_row = 0;

    if let Some(task) = board.selected_task_mut() {
        task.title = "Modified Task".to_string();
    }

    assert_eq!(board.tasks[0].title, "Modified Task");
}
