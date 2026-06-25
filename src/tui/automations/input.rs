//! Key handling for the Automations view.

use crossterm::event::{KeyCode, KeyEvent};

use super::{AutomationsAction, AutomationsView, Mode};

impl AutomationsView {
    pub fn handle_key(&mut self, key: KeyEvent) -> AutomationsAction {
        // Clear transient banners on any interaction; a fresh failure re-sets them.
        self.error = None;
        self.status = None;
        match self.mode {
            Mode::List => self.handle_list_key(key),
            Mode::ConfirmDelete => self.handle_confirm_key(key),
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> AutomationsAction {
        match key.code {
            KeyCode::Esc => return AutomationsAction::Close,
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev(),
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle_enabled(),
            KeyCode::Char('a') => return AutomationsAction::StartAdd,
            KeyCode::Char('e') => {
                if let Some(a) = self.selected_automation() {
                    return AutomationsAction::StartEdit(Box::new(a.clone()));
                }
            }
            KeyCode::Char('d') => {
                if self.selected_automation().is_some() {
                    self.mode = Mode::ConfirmDelete;
                }
            }
            KeyCode::Char('r') => self.run_now(),
            _ => {}
        }
        AutomationsAction::Continue
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> AutomationsAction {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.delete_selected();
                self.mode = Mode::List;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.mode = Mode::List;
            }
            _ => {}
        }
        AutomationsAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::model::{Automation, LaunchSpec, Trigger};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_path: "/tmp/p".into(),
            group_path: String::new(),
            tool: Some("claude".into()),
            command: None,
            extra_args: String::new(),
            view: crate::session::View::Terminal,
            worktree_branch: None,
            sandbox: false,
            auto_approve: true,
            max_runtime_secs: 1800,
            initial_prompt: "hi".into(),
            agent_name: None,
            agent_model: None,
        }
    }

    fn view() -> AutomationsView {
        AutomationsView {
            profile: "default".into(),
            automations: vec![Automation::new(
                "a",
                spec(),
                Trigger::Cron {
                    expr: "* * * * *".into(),
                },
            )],
            selected: 0,
            mode: Mode::List,
            error: None,
            status: None,
        }
    }

    #[test]
    fn esc_closes_from_list() {
        let mut v = view();
        assert!(matches!(
            v.handle_key(key(KeyCode::Esc)),
            AutomationsAction::Close
        ));
    }

    #[test]
    fn a_starts_add() {
        let mut v = view();
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('a'))),
            AutomationsAction::StartAdd
        ));
    }

    #[test]
    fn d_enters_confirm_then_n_cancels() {
        let mut v = view();
        v.handle_key(key(KeyCode::Char('d')));
        assert_eq!(v.mode, Mode::ConfirmDelete);
        v.handle_key(key(KeyCode::Char('n')));
        assert_eq!(v.mode, Mode::List);
    }

    #[test]
    fn d_on_empty_list_stays_in_list_mode() {
        let mut v = view();
        v.automations.clear();
        v.handle_key(key(KeyCode::Char('d')));
        assert_eq!(v.mode, Mode::List);
    }

    #[test]
    fn confirm_keys_only_active_in_confirm_mode() {
        let mut v = view();
        // 'y' in list mode is a no-op (not a delete confirmation).
        v.handle_key(key(KeyCode::Char('y')));
        assert_eq!(v.mode, Mode::List);
        assert_eq!(v.automations.len(), 1);
    }
}
