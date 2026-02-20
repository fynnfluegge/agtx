use crate::db::{Task, TaskStatus};

/// State for the kanban board view
#[derive(Debug)]
pub struct BoardState {
    pub tasks: Vec<Task>,
    pub selected_column: usize,
    pub selected_row: usize,
}

impl BoardState {
    pub fn new() -> Self {
        Self {
            tasks: vec![],
            selected_column: 0,
            selected_row: 0,
        }
    }

    /// Get tasks in a specific column
    pub fn tasks_in_column(&self, column: usize) -> Vec<&Task> {
        let status = TaskStatus::columns().get(column).copied();
        match status {
            Some(s) => self.tasks.iter().filter(|t| t.status == s).collect(),
            None => vec![],
        }
    }

    /// Get the currently selected task (immutable)
    pub fn selected_task(&self) -> Option<&Task> {
        let column_tasks = self.tasks_in_column(self.selected_column);
        column_tasks.get(self.selected_row).copied()
    }

    /// Get the currently selected task (mutable)
    pub fn selected_task_mut(&mut self) -> Option<&mut Task> {
        let status = TaskStatus::columns()
            .get(self.selected_column)
            .copied()?;

        let matching_indices: Vec<usize> = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.status == status)
            .map(|(i, _)| i)
            .collect();

        matching_indices.get(self.selected_row).and_then(|&idx| self.tasks.get_mut(idx))
    }

    /// Move selection left
    pub fn move_left(&mut self) {
        if self.selected_column > 0 {
            self.selected_column -= 1;
            self.clamp_row();
        }
    }

    /// Move selection right
    pub fn move_right(&mut self) {
        if self.selected_column < TaskStatus::columns().len() - 1 {
            self.selected_column += 1;
            self.clamp_row();
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        let column_count = self.tasks_in_column(self.selected_column).len();
        if self.selected_row < column_count.saturating_sub(1) {
            self.selected_row += 1;
        }
    }

    /// Ensure selected_row is valid for current column
    fn clamp_row(&mut self) {
        let column_count = self.tasks_in_column(self.selected_column).len();
        if column_count == 0 {
            self.selected_row = 0;
        } else if self.selected_row >= column_count {
            self.selected_row = column_count - 1;
        }
    }
}

impl Default for BoardState {
    fn default() -> Self {
        Self::new()
    }
}
