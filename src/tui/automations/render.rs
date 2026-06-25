//! Rendering for the Automations view.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use super::{humanize_until, outcome_glyph, AutomationsView, Mode};
use crate::automation::model::{RunOutcome, SessionMode, Trigger};
use crate::tui::dialogs::centered_rect;
use crate::tui::styles::Theme;

impl AutomationsView {
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border))
            .title(Span::styled(
                " Automations ",
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = outer.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(outer, area);

        // body + footer
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        let body = rows[0];
        let footer = rows[1];

        if self.automations.is_empty() {
            let hint = Paragraph::new(vec![
                Line::from(Span::styled(
                    "No automations yet.",
                    Style::default().fg(theme.text),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Press 'a' to create one, or use `aoe automation add` from the CLI.",
                    Style::default().fg(theme.dimmed),
                )),
            ])
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
            frame.render_widget(hint, body);
            self.render_footer(frame, footer, theme);
            return;
        }

        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(body);
        self.render_list(frame, panes[0], theme);
        self.render_detail(frame, panes[1], theme);
        self.render_footer(frame, footer, theme);

        if self.mode == Mode::ConfirmDelete {
            self.render_confirm_delete(frame, area, theme);
        }
    }

    fn render_list(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let now = chrono::Utc::now();
        let items: Vec<ListItem> = self
            .automations
            .iter()
            .map(|a| {
                let Trigger::Cron { expr } = &a.trigger;
                let glyph = if a.enabled { "●" } else { "○" };
                let glyph_style = if a.enabled {
                    Style::default().fg(theme.running)
                } else {
                    Style::default().fg(theme.dimmed)
                };
                let next = match (a.enabled, a.state.next_fire) {
                    (true, Some(t)) => humanize_until(now, t),
                    _ => "—".to_string(),
                };
                let last = a.state.last_run.as_ref().map(|r| &r.outcome);
                let outcome_style = outcome_color(last, theme);
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{glyph} "), glyph_style),
                    Span::styled(
                        format!("{:<20} ", truncate(&a.name, 20)),
                        Style::default().fg(theme.text),
                    ),
                    Span::styled(
                        format!("{:<12} ", truncate(expr, 12)),
                        Style::default().fg(theme.dimmed),
                    ),
                    Span::styled(format!("{next:<7} "), Style::default().fg(theme.hint)),
                    Span::styled(outcome_glyph(last), outcome_style),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(theme.selection)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("");
        let mut state = ListState::default();
        state.select(Some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(a) = self.selected_automation() else {
            return;
        };
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(theme.border));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let label = |s: &str| Span::styled(format!("{s:<10}"), Style::default().fg(theme.dimmed));
        let value = |s: String| Span::styled(s, Style::default().fg(theme.text));
        let Trigger::Cron { expr } = &a.trigger;
        let spec = &a.spec;

        let mut lines = vec![
            Line::from(Span::styled(
                a.name.clone(),
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                label("id"),
                Span::styled(a.short_id().to_string(), Style::default().fg(theme.dimmed)),
            ]),
            Line::from(""),
            Line::from(vec![
                label("status"),
                if a.enabled {
                    Span::styled("enabled", Style::default().fg(theme.running))
                } else {
                    Span::styled("disabled", Style::default().fg(theme.dimmed))
                },
            ]),
            Line::from(vec![label("cron"), value(expr.clone())]),
            Line::from(vec![label("next"), value(fmt_local(a.state.next_fire))]),
            Line::from(vec![
                label("mode"),
                value(match a.session_mode {
                    SessionMode::Fresh => format!("fresh (keep {})", a.retention.keep_last),
                    SessionMode::Persistent => "persistent".to_string(),
                }),
            ]),
            Line::from(""),
            Line::from(vec![label("project"), value(spec.project_path.clone())]),
            Line::from(vec![
                label("tool"),
                value(
                    spec.command
                        .clone()
                        .or_else(|| spec.tool.clone())
                        .unwrap_or_else(|| "(default)".into()),
                ),
            ]),
            Line::from(vec![
                label("prompt"),
                value(truncate(&spec.initial_prompt, 60).to_string()),
            ]),
        ];

        match &a.state.last_run {
            Some(run) => {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    label("last run"),
                    value(
                        run.at
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string(),
                    ),
                ]));
                lines.push(Line::from(vec![
                    label("outcome"),
                    Span::styled(
                        describe_outcome(&run.outcome),
                        outcome_color(Some(&run.outcome), theme),
                    ),
                ]));
            }
            None => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "never run",
                    Style::default().fg(theme.dimmed),
                )));
            }
        }

        if a.state.consecutive_failures > 0 {
            lines.push(Line::from(vec![
                label("failures"),
                Span::styled(
                    a.state.consecutive_failures.to_string(),
                    Style::default().fg(theme.error),
                ),
            ]));
        }

        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            Rect {
                x: inner.x + 1,
                ..inner
            },
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if let Some(err) = &self.error {
            frame.render_widget(
                Paragraph::new(Span::styled(err.clone(), Style::default().fg(theme.error))),
                area,
            );
            return;
        }
        if let Some(status) = &self.status {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    status.clone(),
                    Style::default().fg(theme.running),
                )),
                area,
            );
            return;
        }
        let hint = "↑↓ move   space toggle   a add   e edit   d delete   r run now   esc close";
        frame.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(theme.dimmed))),
            area,
        );
    }

    fn render_confirm_delete(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let name = self
            .selected_automation()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let rect = centered_rect(area, 48, 5);
        frame.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.error))
            .title(" Delete automation ");
        let text = Paragraph::new(vec![
            Line::from(Span::styled(
                format!("Delete \"{}\"?", truncate(&name, 40)),
                Style::default().fg(theme.text),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "y confirm    n cancel",
                Style::default().fg(theme.dimmed),
            )),
        ])
        .alignment(Alignment::Center)
        .block(block);
        frame.render_widget(text, rect);
    }
}

fn outcome_color(outcome: Option<&RunOutcome>, theme: &Theme) -> Style {
    match outcome {
        Some(RunOutcome::Completed) => Style::default().fg(theme.running),
        Some(RunOutcome::Failed { .. }) | Some(RunOutcome::TimedOut) => {
            Style::default().fg(theme.error)
        }
        Some(RunOutcome::Running) => Style::default().fg(theme.waiting),
        None => Style::default().fg(theme.dimmed),
    }
}

fn describe_outcome(outcome: &RunOutcome) -> String {
    match outcome {
        RunOutcome::Running => "running".into(),
        RunOutcome::Completed => "completed".into(),
        RunOutcome::TimedOut => "timed out".into(),
        RunOutcome::Failed { reason } => format!("failed: {}", truncate(reason, 40)),
    }
}

fn fmt_local(t: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match t {
        Some(t) => t
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => "pending".into(),
    }
}

/// Truncate to `max` chars with an ellipsis, counting by `char` so multibyte
/// names don't panic on a byte-index slice.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_ellipsizes() {
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn truncate_multibyte_does_not_panic() {
        let s = "日本語のテキスト";
        let t = truncate(s, 3);
        assert_eq!(t.chars().count(), 3);
    }
}
