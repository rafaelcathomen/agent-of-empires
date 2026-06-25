//! Schedule dialog for creating or editing an automation.
//!
//! Collects the automation-only fields (name, cron expression, initial prompt,
//! session mode, retention) on top of a launch spec. For an add, the launch
//! spec comes from the new-session wizard's `NewSessionData`; for an edit, it
//! comes from the existing automation. Submitting yields a fully-built
//! `Automation` the host upserts into the store.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use super::NewSessionData;
use crate::automation::cron;
use crate::automation::model::{Automation, LaunchSpec, Retention, SessionMode, Trigger};
use crate::session::View;
use crate::tui::components::render_text_field;
use crate::tui::styles::Theme;

/// Outcome of a key event in the schedule dialog. Unlike a plain
/// `DialogResult`, this adds `EditSpec`, which asks the host to detour through
/// the new-session wizard (prefilled from the in-progress automation) to edit
/// the launch spec, then return to this dialog.
pub enum ScheduleOutcome {
    Continue,
    Cancel,
    Submit(Box<Automation>),
    EditSpec(Box<Automation>),
}

const FIELD_COUNT: usize = 5;
const F_NAME: usize = 0;
const F_CRON: usize = 1;
const F_PROMPT: usize = 2;
const F_MODE: usize = 3;
const F_KEEP: usize = 4;

/// Where the launch spec for this automation comes from.
enum Source {
    /// New automation: build a fresh `LaunchSpec` from wizard data.
    Add(Box<NewSessionData>),
    /// Edit an existing automation: preserve its id and state. `new_spec` is
    /// `Some` after a launch-spec detour through the wizard (rebuild the spec
    /// from it); `None` for a schedule/identity-only edit (keep the existing
    /// spec, just refresh `initial_prompt`).
    Edit {
        existing: Box<Automation>,
        new_spec: Option<Box<NewSessionData>>,
    },
}

pub struct ScheduleDialog {
    source: Source,
    name: Input,
    cron: Input,
    prompt: Input,
    session_mode: SessionMode,
    keep_last: Input,
    focused_field: usize,
    error: Option<String>,
}

impl ScheduleDialog {
    /// Create the dialog for a new automation from collected wizard data.
    pub fn new_add(data: NewSessionData) -> Self {
        let default_name = if data.title.trim().is_empty() {
            String::new()
        } else {
            data.title.clone()
        };
        Self {
            source: Source::Add(Box::new(data)),
            name: Input::from(default_name),
            cron: Input::default(),
            prompt: Input::default(),
            session_mode: SessionMode::Fresh,
            keep_last: Input::from("5".to_string()),
            focused_field: F_NAME,
            error: None,
        }
    }

    /// Create the dialog to edit an existing automation's schedule/identity,
    /// keeping its launch spec.
    pub fn new_edit(automation: Automation) -> Self {
        Self::new_edit_inner(automation, None)
    }

    /// Create the dialog to edit an existing automation after a launch-spec
    /// detour: schedule fields prefill from `existing`, but the spec is rebuilt
    /// from the freshly collected wizard `data` on submit.
    pub fn new_edit_with_spec(existing: Automation, data: NewSessionData) -> Self {
        Self::new_edit_inner(existing, Some(Box::new(data)))
    }

    fn new_edit_inner(automation: Automation, new_spec: Option<Box<NewSessionData>>) -> Self {
        let Trigger::Cron { expr } = &automation.trigger;
        let cron = Input::from(expr.clone());
        let name = Input::from(automation.name.clone());
        let prompt = Input::from(automation.spec.initial_prompt.clone());
        let keep_last = Input::from(automation.retention.keep_last.to_string());
        let session_mode = automation.session_mode.clone();
        Self {
            source: Source::Edit {
                existing: Box::new(automation),
                new_spec,
            },
            name,
            cron,
            prompt,
            session_mode,
            keep_last,
            focused_field: F_NAME,
            error: None,
        }
    }

    fn is_edit(&self) -> bool {
        matches!(self.source, Source::Edit { .. })
    }

    fn title(&self) -> &'static str {
        match self.source {
            Source::Add(_) => " New Automation ",
            Source::Edit { .. } => " Edit Automation ",
        }
    }

    /// Build the resulting `Automation` from the current field values, or an
    /// error message if a field is invalid.
    fn build(&self) -> Result<Automation, String> {
        let name = self.name.value().trim().to_string();
        if name.is_empty() {
            return Err("Name is required.".to_string());
        }
        let expr = self.cron.value().trim().to_string();
        cron::parse(&expr).map_err(|_| format!("Invalid cron expression: {expr}"))?;
        let prompt = self.prompt.value().trim().to_string();
        if prompt.is_empty() {
            return Err("Prompt is required (the work each run does).".to_string());
        }
        let keep_last = self.parse_keep_last()?;

        let mut automation = match &self.source {
            Source::Add(data) => {
                let spec = launch_spec_from(data, &prompt);
                Automation::new(&name, spec, Trigger::Cron { expr })
            }
            Source::Edit { existing, new_spec } => {
                let mut a = (**existing).clone();
                a.name = name;
                a.trigger = Trigger::Cron { expr };
                match new_spec {
                    // Launch spec was re-edited via the wizard: rebuild it.
                    Some(data) => a.spec = launch_spec_from(data, &prompt),
                    // Schedule/identity-only edit: keep the spec, refresh prompt.
                    None => a.spec.initial_prompt = prompt,
                }
                a
            }
        };
        automation.session_mode = self.session_mode.clone();
        automation.retention = Retention { keep_last };
        Ok(automation)
    }

    fn parse_keep_last(&self) -> Result<u32, String> {
        let raw = self.keep_last.value().trim();
        if raw.is_empty() {
            return Ok(Retention::default().keep_last);
        }
        raw.parse::<u32>()
            .map_err(|_| format!("Keep-last must be a number, got \"{raw}\"."))
            .and_then(|n| {
                if n == 0 {
                    Err("Keep-last must be at least 1.".to_string())
                } else {
                    Ok(n)
                }
            })
    }

    fn active_input(&mut self) -> Option<&mut Input> {
        match self.focused_field {
            F_NAME => Some(&mut self.name),
            F_CRON => Some(&mut self.cron),
            F_PROMPT => Some(&mut self.prompt),
            F_KEEP => Some(&mut self.keep_last),
            _ => None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ScheduleOutcome {
        use crossterm::event::KeyModifiers;
        // Ctrl+E (edit mode only): detour through the wizard to edit the launch
        // spec. Build first so the in-progress schedule fields are carried back.
        if key.code == KeyCode::Char('e')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.is_edit()
        {
            return match self.build() {
                Ok(automation) => ScheduleOutcome::EditSpec(Box::new(automation)),
                Err(msg) => {
                    self.error = Some(msg);
                    ScheduleOutcome::Continue
                }
            };
        }
        match key.code {
            KeyCode::Esc => ScheduleOutcome::Cancel,
            KeyCode::Enter => match self.build() {
                Ok(automation) => ScheduleOutcome::Submit(Box::new(automation)),
                Err(msg) => {
                    self.error = Some(msg);
                    ScheduleOutcome::Continue
                }
            },
            KeyCode::Tab | KeyCode::Down => {
                self.focused_field = (self.focused_field + 1) % FIELD_COUNT;
                ScheduleOutcome::Continue
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focused_field = (self.focused_field + FIELD_COUNT - 1) % FIELD_COUNT;
                ScheduleOutcome::Continue
            }
            KeyCode::Char(' ') if self.focused_field == F_MODE => {
                self.session_mode = match self.session_mode {
                    SessionMode::Fresh => SessionMode::Persistent,
                    SessionMode::Persistent => SessionMode::Fresh,
                };
                ScheduleOutcome::Continue
            }
            _ => {
                if let Some(input) = self.active_input() {
                    input.handle_event(&crossterm::event::Event::Key(key));
                }
                ScheduleOutcome::Continue
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let dialog_area = super::centered_rect(area, 64, 17);
        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1))
            .border_style(Style::default().fg(theme.accent))
            .title(self.title())
            .title_style(Style::default().fg(theme.title).bold());
        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(1), // name
                Constraint::Length(1), // cron
                Constraint::Length(1), // prompt
                Constraint::Length(1), // mode
                Constraint::Length(1), // keep_last
                Constraint::Length(1), // spacer
                Constraint::Length(1), // error
                Constraint::Min(1),    // hint
            ])
            .split(inner);

        render_text_field(
            frame,
            chunks[0],
            "Name:",
            &self.name,
            self.focused_field == F_NAME,
            None,
            theme,
        );
        render_text_field(
            frame,
            chunks[1],
            "Cron:",
            &self.cron,
            self.focused_field == F_CRON,
            Some("min hour dom mon dow  (e.g. 0 9 * * 1)"),
            theme,
        );
        render_text_field(
            frame,
            chunks[2],
            "Prompt:",
            &self.prompt,
            self.focused_field == F_PROMPT,
            Some("what each run should do"),
            theme,
        );

        let mode_style = if self.focused_field == F_MODE {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.text)
        };
        let mode_label = match self.session_mode {
            SessionMode::Fresh => "Fresh (new session each run)",
            SessionMode::Persistent => "Persistent (reuse one session)",
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Mode:  ", Style::default().fg(theme.dimmed)),
                Span::styled(mode_label, mode_style),
            ])),
            chunks[3],
        );

        // Keep-last only applies to Fresh mode; dim it for Persistent.
        let keep_label = if matches!(self.session_mode, SessionMode::Fresh) {
            "Keep last:"
        } else {
            "Keep last (fresh only):"
        };
        render_text_field(
            frame,
            chunks[4],
            keep_label,
            &self.keep_last,
            self.focused_field == F_KEEP,
            None,
            theme,
        );

        if let Some(err) = &self.error {
            frame.render_widget(
                Paragraph::new(Span::styled(err.clone(), Style::default().fg(theme.error))),
                chunks[6],
            );
        }

        let mut hint = vec![
            Span::styled("Tab", Style::default().fg(theme.hint)),
            Span::raw(" move  "),
            Span::styled("Space", Style::default().fg(theme.hint)),
            Span::raw(" mode  "),
            Span::styled("Enter", Style::default().fg(theme.hint)),
            Span::raw(" save  "),
        ];
        if self.is_edit() {
            hint.push(Span::styled("Ctrl+E", Style::default().fg(theme.hint)));
            hint.push(Span::raw(" launch spec  "));
        }
        hint.push(Span::styled("Esc", Style::default().fg(theme.hint)));
        hint.push(Span::raw(" cancel"));
        frame.render_widget(Paragraph::new(Line::from(hint)), chunks[7]);
    }
}

/// Map collected new-session wizard data plus a prompt onto an automation
/// `LaunchSpec`. Mirrors the `NewSessionData -> InstanceParams` mapping in
/// `home::operations::create_session`, minus the launch-now-only fields.
fn launch_spec_from(data: &NewSessionData, prompt: &str) -> LaunchSpec {
    LaunchSpec {
        project_path: data.path.clone(),
        group_path: data.group.clone(),
        tool: if data.tool.trim().is_empty() {
            None
        } else {
            Some(data.tool.clone())
        },
        command: if data.command_override.trim().is_empty() {
            None
        } else {
            Some(data.command_override.clone())
        },
        extra_args: data.extra_args.clone(),
        view: View::Terminal,
        worktree_branch: if data.worktree_enabled {
            data.worktree_branch.clone()
        } else {
            None
        },
        sandbox: data.sandbox,
        auto_approve: data.yolo_mode,
        max_runtime_secs: 1800,
        initial_prompt: prompt.to_string(),
        agent_name: None,
        agent_model: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn type_str(d: &mut ScheduleDialog, s: &str) {
        for c in s.chars() {
            d.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn sample_data() -> NewSessionData {
        NewSessionData {
            profile: "default".into(),
            title: "my session".into(),
            path: "/home/me/proj".into(),
            group: "team".into(),
            tool: "claude".into(),
            worktree_enabled: true,
            worktree_branch: Some("feat/x".into()),
            create_new_branch: false,
            base_branch: None,
            extra_repo_paths: vec![],
            sandbox: true,
            sandbox_image: String::new(),
            yolo_mode: true,
            extra_env: vec![],
            extra_args: "--foo".into(),
            command_override: String::new(),
            scratch: false,
        }
    }

    #[test]
    fn add_defaults_name_to_session_title() {
        let d = ScheduleDialog::new_add(sample_data());
        assert_eq!(d.name.value(), "my session");
        assert_eq!(d.keep_last.value(), "5");
    }

    #[test]
    fn empty_name_blocks_submit_with_error() {
        let mut d = ScheduleDialog::new_add(sample_data());
        // Clear the defaulted name.
        d.name = Input::default();
        type_str(&mut d, ""); // no-op
        let r = d.handle_key(key(KeyCode::Enter));
        assert!(matches!(r, ScheduleOutcome::Continue));
        assert!(d.error.as_deref().unwrap().contains("Name"));
    }

    #[test]
    fn invalid_cron_blocks_submit() {
        let mut d = ScheduleDialog::new_add(sample_data());
        d.cron = Input::from("not a cron".to_string());
        d.prompt = Input::from("do the thing".to_string());
        let r = d.handle_key(key(KeyCode::Enter));
        assert!(matches!(r, ScheduleOutcome::Continue));
        assert!(d.error.as_deref().unwrap().contains("cron"));
    }

    #[test]
    fn missing_prompt_blocks_submit() {
        let mut d = ScheduleDialog::new_add(sample_data());
        d.cron = Input::from("0 9 * * *".to_string());
        let r = d.handle_key(key(KeyCode::Enter));
        assert!(matches!(r, ScheduleOutcome::Continue));
        assert!(d.error.as_deref().unwrap().contains("Prompt"));
    }

    #[test]
    fn valid_add_submits_mapped_automation() {
        let mut d = ScheduleDialog::new_add(sample_data());
        d.cron = Input::from("0 9 * * 1".to_string());
        d.prompt = Input::from("summarize slack".to_string());
        // Toggle to Persistent then back to Fresh to exercise the toggle.
        d.focused_field = F_MODE;
        d.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(d.session_mode, SessionMode::Persistent);
        d.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(d.session_mode, SessionMode::Fresh);

        match d.handle_key(key(KeyCode::Enter)) {
            ScheduleOutcome::Submit(a) => {
                assert_eq!(a.name, "my session");
                assert_eq!(a.spec.project_path, "/home/me/proj");
                assert_eq!(a.spec.group_path, "team");
                assert_eq!(a.spec.tool.as_deref(), Some("claude"));
                assert_eq!(a.spec.extra_args, "--foo");
                assert!(a.spec.sandbox);
                assert!(a.spec.auto_approve);
                assert_eq!(a.spec.worktree_branch.as_deref(), Some("feat/x"));
                assert_eq!(a.spec.initial_prompt, "summarize slack");
                assert_eq!(a.session_mode, SessionMode::Fresh);
                assert_eq!(a.retention.keep_last, 5);
                let Trigger::Cron { expr } = &a.trigger;
                assert_eq!(expr, "0 9 * * 1");
            }
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn worktree_branch_dropped_when_worktree_disabled() {
        let mut data = sample_data();
        data.worktree_enabled = false;
        let spec = launch_spec_from(&data, "p");
        assert!(spec.worktree_branch.is_none());
    }

    #[test]
    fn edit_preserves_id_and_overwrites_fields() {
        let original = {
            let mut d = ScheduleDialog::new_add(sample_data());
            d.cron = Input::from("0 9 * * *".to_string());
            d.prompt = Input::from("orig".to_string());
            match d.handle_key(key(KeyCode::Enter)) {
                ScheduleOutcome::Submit(a) => *a,
                _ => panic!("expected submit"),
            }
        };
        let original_id = original.id.clone();

        let mut edit = ScheduleDialog::new_edit(original);
        // Field prefills come from the existing automation.
        assert_eq!(edit.cron.value(), "0 9 * * *");
        assert_eq!(edit.prompt.value(), "orig");
        // Change the cron.
        edit.cron = Input::from("30 8 * * *".to_string());
        match edit.handle_key(key(KeyCode::Enter)) {
            ScheduleOutcome::Submit(a) => {
                assert_eq!(a.id, original_id, "edit must preserve the id");
                let Trigger::Cron { expr } = &a.trigger;
                assert_eq!(expr, "30 8 * * *");
            }
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn keep_last_zero_is_rejected() {
        let mut d = ScheduleDialog::new_add(sample_data());
        d.cron = Input::from("0 9 * * *".to_string());
        d.prompt = Input::from("x".to_string());
        d.keep_last = Input::from("0".to_string());
        let r = d.handle_key(key(KeyCode::Enter));
        assert!(matches!(r, ScheduleOutcome::Continue));
        assert!(d.error.as_deref().unwrap().contains("at least 1"));
    }

    #[test]
    fn ctrl_e_in_edit_requests_spec_detour() {
        let original = {
            let mut d = ScheduleDialog::new_add(sample_data());
            d.cron = Input::from("0 9 * * *".to_string());
            d.prompt = Input::from("orig".to_string());
            match d.handle_key(key(KeyCode::Enter)) {
                ScheduleOutcome::Submit(a) => *a,
                _ => panic!("expected submit"),
            }
        };
        let mut edit = ScheduleDialog::new_edit(original);
        let ctrl_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert!(matches!(
            edit.handle_key(ctrl_e),
            ScheduleOutcome::EditSpec(_)
        ));
    }

    #[test]
    fn ctrl_e_ignored_in_add_mode() {
        let mut d = ScheduleDialog::new_add(sample_data());
        let ctrl_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        // In add mode Ctrl+E is not a spec detour; it is swallowed by the
        // focused text input (no EditSpec, no Submit).
        assert!(matches!(d.handle_key(ctrl_e), ScheduleOutcome::Continue));
    }

    #[test]
    fn new_edit_with_spec_rebuilds_launch_spec_but_keeps_id() {
        // Start from an existing automation with one project path...
        let existing = {
            let mut d = ScheduleDialog::new_add(sample_data());
            d.cron = Input::from("0 9 * * *".to_string());
            d.prompt = Input::from("orig".to_string());
            match d.handle_key(key(KeyCode::Enter)) {
                ScheduleOutcome::Submit(a) => *a,
                _ => panic!("expected submit"),
            }
        };
        let existing_id = existing.id.clone();

        // ...and re-edit the spec with new wizard data (different path).
        let mut new_data = sample_data();
        new_data.path = "/srv/other".into();
        let mut d = ScheduleDialog::new_edit_with_spec(existing, new_data);
        // Schedule fields prefill from the existing automation.
        assert_eq!(d.cron.value(), "0 9 * * *");
        assert_eq!(d.prompt.value(), "orig");
        match d.handle_key(key(KeyCode::Enter)) {
            ScheduleOutcome::Submit(a) => {
                assert_eq!(a.id, existing_id, "id is preserved across a spec edit");
                assert_eq!(
                    a.spec.project_path, "/srv/other",
                    "spec rebuilt from new data"
                );
                assert_eq!(a.spec.initial_prompt, "orig");
            }
            _ => panic!("expected submit"),
        }
    }
}
