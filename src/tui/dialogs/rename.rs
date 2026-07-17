//! Rename session / group dialog

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::*;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use super::DialogResult;
use crate::session::FolderColor;
use crate::tui::components::{
    render_text_field, render_text_field_with_ghost, GroupGhostCompletion, ListPicker,
    ListPickerResult,
};
use crate::tui::styles::Theme;

/// Data returned when the rename dialog is submitted
#[derive(Debug, Clone)]
pub struct RenameData {
    /// New title (empty string means keep current)
    pub title: String,
    /// New group path (None means keep current, Some("") means remove from group)
    pub group: Option<String>,
    /// New profile (None means keep current, Some(name) means move to that profile)
    pub profile: Option<String>,
    /// Whether to also rename the git branch to match the title. Only ever
    /// true for a tied aoe-managed worktree session that opted into the
    /// branch toggle; always false otherwise.
    pub rename_branch: bool,
    /// Per-session manual color change (Session mode only). Outer `None` means
    /// unchanged; `Some(inner)` means set the manual color to `inner`
    /// (`Some(None)` clears it back to no manual color).
    pub manual_color: Option<Option<FolderColor>>,
    /// Per-session heat override change (Session mode only). Outer `None` means
    /// unchanged; `Some(inner)` sets `heat_enabled` to `inner` (`Some(None)`
    /// resets it to inherit the global default).
    pub heat_enabled: Option<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameMode {
    Session,
    Group,
}

pub struct RenameDialog {
    mode: RenameMode,
    current_title: String,
    current_group: String,
    current_profile: String,
    available_profiles: Vec<String>,
    new_title: Input,
    new_group: Input,
    profile_index: usize,
    focused_field: usize, // Session: 0=title, 1=group, 2=profile; Group: 0=group, 1=profile
    existing_groups: Vec<String>,
    group_picker: ListPicker,
    group_ghost: Option<GroupGhostCompletion>,
    /// Inline validation error shown in Group mode when a duplicate name is entered.
    validation_error: Option<String>,
    /// Hit rect per focusable field (title / group / profile), set by
    /// `render`. Drives click + hover routing.
    focusable_rects: Vec<(usize, Rect)>,
    /// Set for a tied aoe-managed worktree session via
    /// [`Self::with_worktree_branch`]. When present, the dialog grows a
    /// fourth focusable field: an "Also rename git branch" toggle. The
    /// payload is `(current_branch, upstream)`; `upstream` drives the
    /// remote-orphan warning when the toggle is on.
    worktree_branch: Option<WorktreeBranch>,
    /// State of the branch toggle. Meaningless unless `worktree_branch` is set.
    rename_branch: bool,
    /// Current manual session color (Session mode) and its original value, for
    /// change detection. `None` = no manual color.
    manual_color: Option<FolderColor>,
    orig_manual_color: Option<FolderColor>,
    /// Current per-session heat override (Session mode) and its original.
    /// `None` = inherit global; `Some(true/false)` = forced on/off.
    heat_enabled: Option<bool>,
    orig_heat_enabled: Option<bool>,
    /// Current group spine color (Group mode) and its original.
    group_color: Option<FolderColor>,
    orig_group_color: Option<FolderColor>,
    /// Grid color picker: whether the 4x4 swatch popup is open, and the flat
    /// index (`0..FolderColor::ALL.len()`) of the highlighted swatch.
    color_picker_open: bool,
    color_picker_index: usize,
    /// Per-swatch hit rects, refreshed each time the grid renders, so a mouse
    /// click can map back to a palette index.
    color_swatch_rects: Vec<(usize, Rect)>,
}

/// Columns in the swatch grid popup. The palette is exactly 16 entries, so a
/// 4x4 grid wraps cleanly as a torus in both axes (see the arrow handling).
const COLOR_GRID_COLS: usize = 4;

/// Branch context for a tied worktree session's rename toggle.
struct WorktreeBranch {
    /// The session's current git branch (shown in the toggle row).
    current: String,
    /// Short upstream ref (e.g. `origin/hi`) when the branch tracks a remote,
    /// else `None`. Drives the "remote branch won't follow" warning.
    upstream: Option<String>,
}

impl RenameDialog {
    pub fn mode(&self) -> RenameMode {
        self.mode
    }

    pub fn new(
        current_title: &str,
        current_group: &str,
        current_profile: &str,
        available_profiles: Vec<String>,
        existing_groups: Vec<String>,
    ) -> Self {
        let profile_index = available_profiles
            .iter()
            .position(|p| p == current_profile)
            .unwrap_or(0);

        Self {
            mode: RenameMode::Session,
            current_title: current_title.to_string(),
            current_group: current_group.to_string(),
            current_profile: current_profile.to_string(),
            available_profiles,
            new_title: Input::default(),
            new_group: Input::new(current_group.to_string()),
            profile_index,
            focused_field: 0,
            existing_groups,
            group_picker: ListPicker::new("Select Group"),
            group_ghost: None,
            validation_error: None,
            focusable_rects: Vec::new(),
            worktree_branch: None,
            rename_branch: false,
            manual_color: None,
            orig_manual_color: None,
            heat_enabled: None,
            orig_heat_enabled: None,
            group_color: None,
            orig_group_color: None,
            color_picker_open: false,
            color_picker_index: 0,
            color_swatch_rects: Vec::new(),
        }
    }

    /// Seed the per-session manual color and heat override (Session mode). The
    /// values become both the editable state and the change-detection baseline,
    /// so an untouched dialog reports no change for these fields.
    pub fn with_session_settings(
        mut self,
        manual_color: Option<FolderColor>,
        heat_enabled: Option<bool>,
    ) -> Self {
        self.manual_color = manual_color;
        self.orig_manual_color = manual_color;
        self.heat_enabled = heat_enabled;
        self.orig_heat_enabled = heat_enabled;
        self
    }

    /// Seed the group spine color (Group mode). Both the editable state and the
    /// change-detection baseline.
    pub fn with_group_color(mut self, color: Option<FolderColor>) -> Self {
        self.group_color = color;
        self.orig_group_color = color;
        self
    }

    /// The group color change to apply on submit (Group mode): `Some(value)`
    /// when the user changed it (`Some(None)` clears the color), `None` when
    /// unchanged.
    pub fn group_color_change(&self) -> Option<Option<FolderColor>> {
        if self.group_color != self.orig_group_color {
            Some(self.group_color)
        } else {
            None
        }
    }

    /// Attach tied-worktree branch context, enabling the "Also rename git
    /// branch" toggle. Call only for a Session-mode dialog whose session is a
    /// tied aoe-managed worktree. `upstream` is the short tracking ref
    /// (`origin/hi`) when the branch tracks a remote, used to warn that a
    /// rename leaves that remote branch behind.
    pub fn with_worktree_branch(mut self, current_branch: &str, upstream: Option<String>) -> Self {
        self.worktree_branch = Some(WorktreeBranch {
            current: current_branch.to_string(),
            upstream,
        });
        self
    }

    pub fn new_for_group(
        current_group: &str,
        current_profile: &str,
        available_profiles: Vec<String>,
        existing_groups: Vec<String>,
    ) -> Self {
        let profile_index = available_profiles
            .iter()
            .position(|p| p == current_profile)
            .unwrap_or(0);

        Self {
            mode: RenameMode::Group,
            current_title: String::new(),
            current_group: current_group.to_string(),
            current_profile: current_profile.to_string(),
            available_profiles,
            new_title: Input::default(),
            new_group: Input::new(current_group.to_string()),
            profile_index,
            focused_field: 0,
            existing_groups,
            group_picker: ListPicker::new("Select Group"),
            group_ghost: None,
            validation_error: None,
            focusable_rects: Vec::new(),
            worktree_branch: None,
            rename_branch: false,
            manual_color: None,
            orig_manual_color: None,
            heat_enabled: None,
            orig_heat_enabled: None,
            group_color: None,
            orig_group_color: None,
            color_picker_open: false,
            color_picker_index: 0,
            color_swatch_rects: Vec::new(),
        }
    }

    /// Whether the "Also rename git branch" toggle is present (tied worktree
    /// session only). When true it occupies focusable field index 3.
    fn shows_branch_toggle(&self) -> bool {
        self.mode == RenameMode::Session && self.worktree_branch.is_some()
    }

    fn is_branch_toggle_field(&self) -> bool {
        self.shows_branch_toggle() && self.focused_field == 3
    }

    /// Focusable index of the per-session Heat field (Session mode), placed
    /// after title/group/profile and the optional branch toggle so the
    /// existing indices stay stable.
    fn heat_field_index(&self) -> usize {
        if self.shows_branch_toggle() {
            4
        } else {
            3
        }
    }

    /// Focusable index of the Color field. Session mode: right after Heat.
    /// Group mode: after group/profile (index 2).
    fn color_field_index(&self) -> usize {
        match self.mode {
            RenameMode::Session => self.heat_field_index() + 1,
            RenameMode::Group => 2,
        }
    }

    fn is_heat_field(&self) -> bool {
        self.mode == RenameMode::Session && self.focused_field == self.heat_field_index()
    }

    fn is_color_field(&self) -> bool {
        self.focused_field == self.color_field_index()
    }

    fn field_count(&self) -> usize {
        match self.mode {
            // title, group, profile, optional branch toggle, then Heat + Color.
            RenameMode::Session => self.color_field_index() + 1,
            // group, profile, Color.
            RenameMode::Group => 3,
        }
    }

    /// Cycle the color field in place through the palette plus a trailing
    /// "none" slot, like the profile selector. Shared between Session (manual
    /// session color) and Group (spine color) modes. `forward` advances toward
    /// the next palette entry; wrapping passes through the "none" slot.
    fn cycle_color(&mut self, forward: bool) {
        let current = match self.mode {
            RenameMode::Session => self.manual_color,
            RenameMode::Group => self.group_color,
        };
        let n = FolderColor::ALL.len();
        // Slot space: 0..n select a palette color, n is "none".
        let cur = current
            .and_then(|c| FolderColor::ALL.iter().position(|x| *x == c))
            .unwrap_or(n);
        let slots = n + 1;
        let next = if forward {
            (cur + 1) % slots
        } else {
            (cur + slots - 1) % slots
        };
        let chosen = (next < n).then(|| FolderColor::ALL[next]);
        match self.mode {
            RenameMode::Session => self.manual_color = chosen,
            RenameMode::Group => self.group_color = chosen,
        }
    }

    fn current_color(&self) -> Option<FolderColor> {
        match self.mode {
            RenameMode::Session => self.manual_color,
            RenameMode::Group => self.group_color,
        }
    }

    fn set_color(&mut self, chosen: Option<FolderColor>) {
        match self.mode {
            RenameMode::Session => self.manual_color = chosen,
            RenameMode::Group => self.group_color = chosen,
        }
    }

    /// Open the 4x4 swatch grid, preselecting the swatch matching the current
    /// color (or the first swatch when unset).
    fn open_color_picker(&mut self) {
        self.color_picker_index = self
            .current_color()
            .and_then(|c| FolderColor::ALL.iter().position(|x| *x == c))
            .unwrap_or(0);
        self.color_picker_open = true;
    }

    /// Apply the highlighted swatch and close the grid.
    fn commit_color_picker(&mut self) {
        let chosen = FolderColor::ALL.get(self.color_picker_index).copied();
        if chosen.is_some() {
            self.set_color(chosen);
        }
        self.color_picker_open = false;
    }

    /// Move the grid highlight. The palette is a 4x4 torus: left/right step by
    /// one with wrap, up/down step by a row (`COLOR_GRID_COLS`) with wrap.
    fn move_color_picker(&mut self, dx: i32, dy: i32) {
        let n = FolderColor::ALL.len() as i32;
        let cols = COLOR_GRID_COLS as i32;
        let mut i = self.color_picker_index as i32;
        if dx != 0 {
            i = (i + dx).rem_euclid(n);
        }
        if dy != 0 {
            i = (i + dy * cols).rem_euclid(n);
        }
        self.color_picker_index = i as usize;
    }

    pub fn handle_click(&mut self, col: u16, row: u16) -> Option<DialogResult<RenameData>> {
        // Color grid popup is modal: a click on a swatch picks it; a click
        // anywhere else is swallowed so it neither dismisses the dialog nor
        // leaks to the fields beneath.
        if self.color_picker_open {
            let pos = ratatui::layout::Position::from((col, row));
            if let Some((idx, _)) = self
                .color_swatch_rects
                .iter()
                .find(|(_, rect)| rect.contains(pos))
            {
                self.color_picker_index = *idx;
                self.commit_color_picker();
            }
            return Some(DialogResult::Continue);
        }
        // Group picker overlay wins when active so a click can pick a
        // group row without dropping the dialog underneath.
        if self.group_picker.is_active() {
            match self.group_picker.handle_click(col, row) {
                ListPickerResult::Continue => return Some(DialogResult::Continue),
                ListPickerResult::Cancelled => return Some(DialogResult::Continue),
                ListPickerResult::Selected(value) => {
                    self.new_group = Input::new(value);
                    // Mirror the keyboard picker path: the ghost
                    // autocomplete state goes stale once the user
                    // commits to a value via the picker, so drop it.
                    self.group_ghost = None;
                    return Some(DialogResult::Continue);
                }
            }
        }
        let pos = ratatui::layout::Position::from((col, row));
        let hit = self
            .focusable_rects
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(f, _)| *f)?;
        self.focused_field = hit;
        // Cycle the profile chip on click; flip the branch toggle on click;
        // text fields just take focus.
        if self.is_profile_field() && !self.available_profiles.is_empty() {
            self.profile_index = (self.profile_index + 1) % self.available_profiles.len();
        } else if self.is_branch_toggle_field() {
            self.rename_branch = !self.rename_branch;
        } else if self.is_heat_field() {
            self.heat_enabled = match self.heat_enabled {
                None => Some(true),
                Some(true) => Some(false),
                Some(false) => None,
            };
        } else if self.is_color_field() {
            self.open_color_picker();
        }
        Some(DialogResult::Continue)
    }

    /// Hover only updates the group-picker overlay highlight (menu-style
    /// behavior the user expects). It deliberately does NOT move focus
    /// between the title / group / profile rows: stealing focus from the
    /// field the user is typing into just because the mouse cursor
    /// drifts across the dialog is jarring. Click still sets focus.
    pub fn handle_hover(&mut self, col: u16, row: u16) -> bool {
        if self.group_picker.is_active() {
            return self.group_picker.handle_hover(col, row);
        }
        false
    }

    fn is_profile_field(&self) -> bool {
        match self.mode {
            RenameMode::Session => self.focused_field == 2,
            RenameMode::Group => self.focused_field == 1,
        }
    }

    fn focused_input(&mut self) -> Option<&mut Input> {
        match self.mode {
            RenameMode::Session => match self.focused_field {
                0 => Some(&mut self.new_title),
                1 => Some(&mut self.new_group),
                _ => None,
            },
            RenameMode::Group => match self.focused_field {
                0 => Some(&mut self.new_group),
                _ => None,
            },
        }
    }

    fn is_group_field(&self) -> bool {
        match self.mode {
            RenameMode::Session => self.focused_field == 1,
            RenameMode::Group => self.focused_field == 0,
        }
    }

    fn next_field(&mut self) {
        self.focused_field = (self.focused_field + 1) % self.field_count();
    }

    fn prev_field(&mut self) {
        let count = self.field_count();
        self.focused_field = if self.focused_field == 0 {
            count - 1
        } else {
            self.focused_field - 1
        };
    }

    fn recompute_group_ghost(&mut self) {
        // Suffix matching (resolve a leaf to an existing nested path) is wanted
        // when MOVING a session into a folder (Session mode's group field) but
        // not when renaming a group's own name (Group mode), where the typed
        // text is a brand-new name and an unrelated suffix hit would surprise.
        let suffix_match = self.mode == RenameMode::Session;
        self.group_ghost =
            GroupGhostCompletion::compute(&self.new_group, &self.existing_groups, suffix_match);
    }

    fn accept_group_ghost(&mut self) {
        if let Some(ghost) = self.group_ghost.take() {
            if let Some(new_value) = ghost.accept(&self.new_group) {
                self.new_group = Input::new(new_value);
                self.recompute_group_ghost();
            }
        }
    }

    fn group_ghost_text(&self) -> Option<&str> {
        self.group_ghost.as_ref().map(|g| g.ghost_text())
    }

    fn selected_profile(&self) -> &str {
        &self.available_profiles[self.profile_index]
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> DialogResult<RenameData> {
        // Color grid popup is modal: while open it owns all keys so its arrows
        // and Enter don't leak into field navigation underneath.
        if self.color_picker_open {
            match key.code {
                KeyCode::Esc => self.color_picker_open = false,
                KeyCode::Left | KeyCode::Char('h') => self.move_color_picker(-1, 0),
                KeyCode::Right | KeyCode::Char('l') => self.move_color_picker(1, 0),
                KeyCode::Up | KeyCode::Char('k') => self.move_color_picker(0, -1),
                KeyCode::Down | KeyCode::Char('j') => self.move_color_picker(0, 1),
                KeyCode::Enter | KeyCode::Char(' ') => self.commit_color_picker(),
                // Clear the color outright without leaving the grid hunting for
                // a non-existent "none" cell.
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('n') => {
                    self.set_color(None);
                    self.color_picker_open = false;
                }
                _ => {}
            }
            return DialogResult::Continue;
        }

        // Handle group picker if active
        if self.group_picker.is_active() {
            if let ListPickerResult::Selected(value) = self.group_picker.handle_key(key) {
                self.new_group = Input::new(value);
                self.group_ghost = None;
            }
            return DialogResult::Continue;
        }

        // Heat field: Space cycles the tri-state override
        // None -> Some(true) -> Some(false) -> None. Enter is deliberately NOT
        // bound so it keeps its dialog-wide confirm/submit meaning, matching the
        // color and branch-toggle fields.
        if self.is_heat_field() && key.code == KeyCode::Char(' ') {
            self.heat_enabled = match self.heat_enabled {
                None => Some(true),
                Some(true) => Some(false),
                Some(false) => None,
            };
            return DialogResult::Continue;
        }

        // Color field: Left/Right nudge the palette inline (quick one-step
        // tweak, shown live in the row's own color); Space opens the 4x4 grid
        // to browse the whole palette at once. Enter is deliberately NOT bound
        // here so it keeps its dialog-wide "confirm/submit" meaning (opening the
        // grid on Enter overloaded the same key the grid then uses to confirm).
        if self.is_color_field() {
            match key.code {
                KeyCode::Char(' ') => {
                    self.open_color_picker();
                    return DialogResult::Continue;
                }
                KeyCode::Right => {
                    self.cycle_color(true);
                    return DialogResult::Continue;
                }
                KeyCode::Left => {
                    self.cycle_color(false);
                    return DialogResult::Continue;
                }
                _ => {}
            }
        }

        // Ctrl+P opens group picker on group field
        if key.code == KeyCode::Char('p')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.is_group_field()
            && !self.existing_groups.is_empty()
        {
            self.group_picker.activate(self.existing_groups.clone());
            return DialogResult::Continue;
        }

        // Right/End arrow at end of group input with ghost: accept ghost text
        if self.is_group_field()
            && matches!(key.code, KeyCode::Right | KeyCode::End)
            && key.modifiers == KeyModifiers::NONE
            && self.group_ghost.is_some()
        {
            let cursor = self.new_group.cursor();
            let char_len = self.new_group.value().chars().count();
            if cursor >= char_len {
                self.accept_group_ghost();
                return DialogResult::Continue;
            }
        }

        match key.code {
            KeyCode::Esc => DialogResult::Cancel,
            KeyCode::Enter => {
                let title_value = self.new_title.value().trim().to_string();
                let group_value = self.new_group.value().trim();
                let selected_profile = self.selected_profile();
                let profile_changed = selected_profile != self.current_profile;

                // If nothing has changed, cancel. Arming the branch toggle
                // counts as a change even when title/group/profile are
                // untouched (rename a drifted branch in place). A color or heat
                // edit likewise counts as a change.
                let branch_rename = self.shows_branch_toggle() && self.rename_branch;
                let manual_color_changed = self.manual_color != self.orig_manual_color;
                let heat_changed = self.heat_enabled != self.orig_heat_enabled;
                let group_color_changed = self.group_color != self.orig_group_color;
                if title_value.is_empty()
                    && group_value == self.current_group
                    && !profile_changed
                    && !branch_rename
                    && !manual_color_changed
                    && !heat_changed
                    && !group_color_changed
                {
                    return DialogResult::Cancel;
                }

                // Validate that the new group name does not already exist
                if self.mode == RenameMode::Group
                    && !group_value.is_empty()
                    && group_value != self.current_group
                    && self.existing_groups.iter().any(|g| g == group_value)
                {
                    self.validation_error = Some(
                        "A group with this name already exists.\nEnter a different name."
                            .to_string(),
                    );
                    return DialogResult::Continue;
                }

                // Determine the group value:
                // - Same as current means keep current group (None)
                // - Empty (and was non-empty) means remove from group (Some(""))
                // - Any other changed value means set new group
                let group = if group_value == self.current_group {
                    None
                } else if group_value.is_empty() {
                    Some(String::new())
                } else {
                    Some(group_value.to_string())
                };

                // Determine profile value
                let profile = if profile_changed {
                    Some(selected_profile.to_string())
                } else {
                    None
                };

                DialogResult::Submit(RenameData {
                    title: title_value,
                    group,
                    profile,
                    rename_branch: self.shows_branch_toggle() && self.rename_branch,
                    manual_color: manual_color_changed.then_some(self.manual_color),
                    heat_enabled: heat_changed.then_some(self.heat_enabled),
                })
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.prev_field();
                } else {
                    self.next_field();
                }
                if self.is_group_field() {
                    self.recompute_group_ghost();
                } else {
                    self.group_ghost = None;
                }
                DialogResult::Continue
            }
            KeyCode::Down => {
                self.next_field();
                if self.is_group_field() {
                    self.recompute_group_ghost();
                } else {
                    self.group_ghost = None;
                }
                DialogResult::Continue
            }
            KeyCode::Up => {
                self.prev_field();
                if self.is_group_field() {
                    self.recompute_group_ghost();
                } else {
                    self.group_ghost = None;
                }
                DialogResult::Continue
            }
            KeyCode::Char(' ') if self.is_branch_toggle_field() => {
                self.rename_branch = !self.rename_branch;
                DialogResult::Continue
            }
            KeyCode::Left if self.is_profile_field() => {
                // Cycle profile backwards
                if self.profile_index == 0 {
                    self.profile_index = self.available_profiles.len().saturating_sub(1);
                } else {
                    self.profile_index -= 1;
                }
                DialogResult::Continue
            }
            KeyCode::Right | KeyCode::Char(' ') if self.is_profile_field() => {
                // Cycle profile forwards
                self.profile_index = (self.profile_index + 1) % self.available_profiles.len();
                DialogResult::Continue
            }
            _ => {
                if let Some(input) = self.focused_input() {
                    input.handle_event(&crossterm::event::Event::Key(key));
                }
                if self.is_group_field() {
                    self.recompute_group_ghost();
                    self.validation_error = None;
                }
                DialogResult::Continue
            }
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Some(input) = self.focused_input() {
            super::paste_into_input(input, text);
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match self.mode {
            RenameMode::Session => self.render_session(frame, area, theme),
            RenameMode::Group => self.render_group(frame, area, theme),
        }
    }

    fn render_session(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.focusable_rects.clear();
        let show_toggle = self.shows_branch_toggle();
        // The remote-orphan warning only matters once the toggle is armed and
        // the branch actually tracks a remote.
        let show_warning = show_toggle
            && self.rename_branch
            && self
                .worktree_branch
                .as_ref()
                .is_some_and(|w| w.upstream.is_some());

        let dialog_width = 50;
        // +2 for the Heat and Color rows.
        let height = 17 + if show_toggle { 1 } else { 0 } + if show_warning { 2 } else { 0 };
        let dialog_area = super::centered_rect(area, dialog_width, height);

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.accent))
            .title(" Edit Session ")
            .title_style(Style::default().fg(theme.title).bold());

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Fixed rows first (current values, spacer, the three input fields),
        // then the optional branch toggle / warning, then spacer + hint. The
        // dynamic indices are tracked so the wiring below stays in sync.
        let mut constraints = vec![
            Constraint::Length(1), // 0 Current title
            Constraint::Length(1), // 1 Current group
            Constraint::Length(1), // 2 Current profile
            Constraint::Length(1), // 3 Spacer
            Constraint::Length(1), // 4 New title field
            Constraint::Length(1), // 5 New group field
            Constraint::Length(1), // 6 Profile selector
        ];
        let toggle_idx = show_toggle.then(|| {
            constraints.push(Constraint::Length(1));
            constraints.len() - 1
        });
        let warning_idx = show_warning.then(|| {
            constraints.push(Constraint::Length(2));
            constraints.len() - 1
        });
        // Heat + Color rows, placed after the optional branch toggle so their
        // focusable indices follow the toggle (see `heat_field_index`).
        constraints.push(Constraint::Length(1));
        let heat_chunk_idx = constraints.len() - 1;
        constraints.push(Constraint::Length(1));
        let color_chunk_idx = constraints.len() - 1;
        constraints.push(Constraint::Length(1)); // Spacer
        constraints.push(Constraint::Min(1)); // Hint
        let hint_idx = constraints.len() - 1;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints(constraints)
            .split(inner);

        // Current title
        let current_title_line = Line::from(vec![
            Span::styled("Current title: ", Style::default().fg(theme.dimmed)),
            Span::styled(&self.current_title, Style::default().fg(theme.text)),
        ]);
        frame.render_widget(Paragraph::new(current_title_line), chunks[0]);

        // Current group
        self.render_current_group(frame, chunks[1], theme);

        // Current profile
        self.render_current_profile(frame, chunks[2], theme);

        // New title field
        render_text_field(
            frame,
            chunks[4],
            "New title:",
            &self.new_title,
            self.focused_field == 0,
            None,
            theme,
        );
        self.focusable_rects.push((0, chunks[4]));

        // New group field
        self.render_group_field(frame, chunks[5], theme);
        self.focusable_rects.push((1, chunks[5]));

        // Profile selector
        self.render_profile_selector(frame, chunks[6], theme);
        self.focusable_rects.push((2, chunks[6]));

        // Branch toggle + remote-orphan warning (tied worktree only)
        if let Some(idx) = toggle_idx {
            self.render_branch_toggle(frame, chunks[idx], theme);
            self.focusable_rects.push((3, chunks[idx]));
        }
        if let Some(idx) = warning_idx {
            self.render_branch_warning(frame, chunks[idx], theme);
        }

        // Heat override row + Color row.
        self.render_heat_row(frame, chunks[heat_chunk_idx], theme);
        self.focusable_rects
            .push((self.heat_field_index(), chunks[heat_chunk_idx]));
        self.render_color_row(frame, chunks[color_chunk_idx], theme);
        self.focusable_rects
            .push((self.color_field_index(), chunks[color_chunk_idx]));

        // Hint
        self.render_hints(frame, chunks[hint_idx], theme);

        // Overlays drawn last so they sit on top.
        if self.group_picker.is_active() {
            self.group_picker.render(frame, area, theme);
        }
        if self.color_picker_open {
            self.render_color_picker(frame, area, theme);
        }
    }

    /// "Heat: default (on) | on | off" row. The tri-state override surfaces the
    /// inherited global value in the "default" label so the user knows what
    /// inheriting means.
    fn render_heat_row(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let focused = self.is_heat_field();
        let label_style = if focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.dimmed)
        };
        let value = match self.heat_enabled {
            None => "default".to_string(),
            Some(true) => "on".to_string(),
            Some(false) => "off".to_string(),
        };
        let value_style = if focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.text)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Heat:       ", label_style),
                Span::styled(value, value_style),
            ])),
            area,
        );
    }

    /// Color row: a swatch plus the color name (or "none"). Shared between
    /// Session (manual session color) and Group (spine color) modes.
    fn render_color_row(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let focused = self.is_color_field();
        let label_style = if focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.dimmed)
        };
        let current = match self.mode {
            RenameMode::Session => self.manual_color,
            RenameMode::Group => self.group_color,
        };
        let mut spans = vec![Span::styled("Color:      ", label_style)];
        // A left cycle chevron when focused signals the field steps through the
        // palette with the arrow keys (mirrors the profile selector affordance).
        if focused {
            spans.push(Span::styled("\u{2039} ", Style::default().fg(theme.accent)));
        }
        match current {
            Some(c) => {
                let (r, g, b) = c.rgb();
                let col = Color::Rgb(r, g, b);
                // The swatch and the name both render in the color itself, so
                // the row reads as the thing it names.
                spans.push(Span::styled("\u{2588} ", Style::default().fg(col)));
                spans.push(Span::styled(c.as_str(), Style::default().fg(col)));
            }
            None => {
                spans.push(Span::styled("none", Style::default().fg(theme.dimmed)));
            }
        }
        if focused {
            spans.push(Span::styled(" \u{203a}", Style::default().fg(theme.accent)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// 4x4 swatch grid popup. Each cell is a block in its own color; the
    /// highlighted cell is bracketed, and a caption names it in that color.
    /// Drawn last (over a `Clear`) so it sits on top of the dialog. Also
    /// records each swatch's hit rect for mouse picking.
    fn render_color_picker(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.color_swatch_rects.clear();
        const CELL_W: u16 = 5;
        let cols = COLOR_GRID_COLS;
        let rows = FolderColor::ALL.len().div_ceil(cols);
        let grid_w = CELL_W * cols as u16;
        // borders (2) + grid rows + blank + caption + hint.
        let popup_w = grid_w + 14;
        let popup_h = rows as u16 + 5;
        let popup_area = super::centered_rect(area, popup_w, popup_h);
        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.accent))
            .title(" Pick Color ")
            .title_style(Style::default().fg(theme.title).bold());
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let grid_x = inner.x + inner.width.saturating_sub(grid_w) / 2;
        let grid_y = inner.y;
        for (i, color) in FolderColor::ALL.iter().enumerate() {
            let r = (i / cols) as u16;
            let c = (i % cols) as u16;
            let cell = Rect {
                x: grid_x + c * CELL_W,
                y: grid_y + r,
                width: CELL_W,
                height: 1,
            };
            self.color_swatch_rects.push((i, cell));
            let (rr, gg, bb) = color.rgb();
            let sel = i == self.color_picker_index;
            let glyph = if sel {
                "[\u{2588}\u{2588}]"
            } else {
                " \u{2588}\u{2588} "
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    glyph,
                    Style::default().fg(Color::Rgb(rr, gg, bb)),
                ))),
                cell,
            );
        }

        // Caption: the highlighted swatch's name, in its own color.
        let cap_y = grid_y + rows as u16 + 1;
        if let Some(color) = FolderColor::ALL.get(self.color_picker_index) {
            let (rr, gg, bb) = color.rgb();
            let col = Color::Rgb(rr, gg, bb);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{2588} ", Style::default().fg(col)),
                    Span::styled(color.as_str(), Style::default().fg(col)),
                ])),
                Rect {
                    x: inner.x,
                    y: cap_y,
                    width: inner.width,
                    height: 1,
                },
            );
        }

        // Hint line.
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "\u{2190}\u{2191}\u{2193}\u{2192}",
                    Style::default().fg(theme.hint),
                ),
                Span::raw(" move  "),
                Span::styled("\u{21b5}", Style::default().fg(theme.hint)),
                Span::raw(" pick  "),
                Span::styled("\u{232b}", Style::default().fg(theme.hint)),
                Span::raw(" none  "),
                Span::styled("Esc", Style::default().fg(theme.hint)),
            ])),
            Rect {
                x: inner.x,
                y: cap_y + 1,
                width: inner.width,
                height: 1,
            },
        );
    }

    fn render_branch_toggle(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let focused = self.is_branch_toggle_field();
        let checkbox = if self.rename_branch { "[x]" } else { "[ ]" };
        let style = if focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.text)
        };
        let mut spans = vec![
            Span::styled(format!("{checkbox} "), style),
            Span::styled("Also rename git branch", style),
        ];
        // Show the current branch dimmed so the user knows what is being
        // renamed (and from what), since the title may already match the dir.
        if let Some(wt) = &self.worktree_branch {
            spans.push(Span::styled(
                format!("  ({})", wt.current),
                Style::default().fg(theme.dimmed),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_branch_warning(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(wt) = &self.worktree_branch else {
            return;
        };
        let Some(upstream) = &wt.upstream else {
            return;
        };
        let lines = vec![
            Line::from(Span::styled(
                format!("! branch '{}' tracks {};", wt.current, upstream),
                Style::default().fg(theme.error),
            )),
            Line::from(Span::styled(
                "  the remote branch won't follow",
                Style::default().fg(theme.error),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_group(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.focusable_rects.clear();
        let dialog_width = 50;
        let has_error = self.validation_error.is_some();
        // +1 for the Color row.
        let dialog_height = if has_error { 17 } else { 14 };
        let dialog_area = super::centered_rect(area, dialog_width, dialog_height);

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .title(" Rename Group ")
            .title_style(Style::default().fg(theme.title).bold());

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        let mut constraints = vec![
            Constraint::Length(1), // 0 Current group
            Constraint::Length(1), // 1 Current profile
            Constraint::Length(1), // 2 Spacer
            Constraint::Length(1), // 3 New group field
            Constraint::Length(1), // 4 Profile selector
            Constraint::Length(1), // 5 Color row
            Constraint::Length(1), // 6 Spacer
            Constraint::Min(1),    // 7 Hint
        ];
        if has_error {
            constraints.insert(6, Constraint::Length(2)); // Validation error (2 lines)
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints(constraints)
            .split(inner);

        // Current group
        self.render_current_group(frame, chunks[0], theme);

        // Current profile
        self.render_current_profile(frame, chunks[1], theme);

        // New group field
        self.render_group_field(frame, chunks[3], theme);
        self.focusable_rects.push((0, chunks[3]));

        // Profile selector
        self.render_profile_selector(frame, chunks[4], theme);
        self.focusable_rects.push((1, chunks[4]));

        // Color row (group spine color), focusable index 2.
        self.render_color_row(frame, chunks[5], theme);
        self.focusable_rects.push((2, chunks[5]));

        if has_error {
            // Validation error (two lines, one sentence each)
            let error_text: Vec<Line> = self
                .validation_error
                .as_deref()
                .unwrap_or("")
                .lines()
                .map(|l| {
                    Line::from(Span::styled(
                        l.to_string(),
                        Style::default().fg(theme.error),
                    ))
                })
                .collect();
            frame.render_widget(Paragraph::new(error_text), chunks[6]);
            // Hint is shifted one index further
            self.render_hints(frame, chunks[8], theme);
        } else {
            // Hint
            self.render_hints(frame, chunks[7], theme);
        }

        // Overlays drawn last.
        if self.group_picker.is_active() {
            self.group_picker.render(frame, area, theme);
        }
        if self.color_picker_open {
            self.render_color_picker(frame, area, theme);
        }
    }

    fn render_current_group(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let group_display = if self.current_group.is_empty() {
            "(none)".to_string()
        } else {
            self.current_group.clone()
        };
        let line = Line::from(vec![
            Span::styled("Current group: ", Style::default().fg(theme.dimmed)),
            Span::styled(group_display, Style::default().fg(theme.text)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_current_profile(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let line = Line::from(vec![
            Span::styled("Current profile: ", Style::default().fg(theme.dimmed)),
            Span::styled(&self.current_profile, Style::default().fg(theme.text)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_group_field(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let group_hint = if self.is_group_field() && !self.existing_groups.is_empty() {
            Some("Ctrl+P to browse")
        } else {
            None
        };
        render_text_field_with_ghost(
            frame,
            area,
            "New group:",
            &self.new_group,
            self.is_group_field(),
            group_hint,
            self.group_ghost_text(),
            theme,
        );
    }

    fn render_profile_selector(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let profile_focused = self.is_profile_field();
        let selected_profile = self.selected_profile();
        let profile_style = if profile_focused {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.text)
        };

        let profile_line = Line::from(vec![
            Span::styled(
                "Profile:    ",
                if profile_focused {
                    Style::default().fg(theme.accent)
                } else {
                    Style::default().fg(theme.dimmed)
                },
            ),
            Span::styled("< ", Style::default().fg(theme.dimmed)),
            Span::styled(selected_profile, profile_style),
            Span::styled(" >", Style::default().fg(theme.dimmed)),
        ]);
        frame.render_widget(Paragraph::new(profile_line), area);
    }

    fn render_hints(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut hint_spans = vec![
            Span::styled("Tab", Style::default().fg(theme.hint)),
            Span::raw(" switch  "),
        ];
        if self.is_branch_toggle_field() || self.is_heat_field() {
            hint_spans.push(Span::styled("Space", Style::default().fg(theme.hint)));
            hint_spans.push(Span::raw(" toggle  "));
        }
        if self.is_color_field() {
            hint_spans.push(Span::styled("Space", Style::default().fg(theme.hint)));
            hint_spans.push(Span::raw(" pick color  "));
        }
        if self.is_group_field() && !self.existing_groups.is_empty() {
            if self.group_ghost_text().is_some() {
                hint_spans.push(Span::styled("→", Style::default().fg(theme.hint)));
                hint_spans.push(Span::raw(" accept  "));
            }
            hint_spans.push(Span::styled("C-p", Style::default().fg(theme.hint)));
            hint_spans.push(Span::raw(" groups  "));
        }
        hint_spans.push(Span::styled("Enter", Style::default().fg(theme.hint)));
        hint_spans.push(Span::raw(" save  "));
        hint_spans.push(Span::styled("Esc", Style::default().fg(theme.hint)));
        hint_spans.push(Span::raw(" cancel"));
        let hint = Line::from(hint_spans);
        frame.render_widget(Paragraph::new(hint), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn default_profiles() -> Vec<String> {
        vec!["default".to_string()]
    }

    fn multi_profiles() -> Vec<String> {
        vec![
            "default".to_string(),
            "work".to_string(),
            "personal".to_string(),
        ]
    }

    #[test]
    fn test_new_dialog() {
        let dialog = RenameDialog::new(
            "Original Title",
            "work/frontend",
            "default",
            default_profiles(),
            Vec::new(),
        );
        assert_eq!(dialog.current_title, "Original Title");
        assert_eq!(dialog.current_group, "work/frontend");
        assert_eq!(dialog.current_profile, "default");
        assert_eq!(dialog.new_title.value(), "");
        assert_eq!(dialog.new_group.value(), "work/frontend"); // Pre-populated with current group
        assert_eq!(dialog.profile_index, 0);
        assert_eq!(dialog.focused_field, 0);
    }

    #[test]
    fn test_new_dialog_empty_group() {
        let dialog = RenameDialog::new("Title", "", "default", default_profiles(), Vec::new());
        assert_eq!(dialog.current_group, "");
    }

    #[test]
    fn test_new_dialog_with_non_default_profile() {
        let dialog = RenameDialog::new("Title", "group", "work", multi_profiles(), Vec::new());
        assert_eq!(dialog.current_profile, "work");
        assert_eq!(dialog.profile_index, 1); // "work" is at index 1
    }

    #[test]
    fn test_esc_cancels() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        let result = dialog.handle_key(key(KeyCode::Esc));
        assert!(matches!(result, DialogResult::Cancel));
    }

    #[test]
    fn test_enter_with_unchanged_fields_cancels() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        // Title is empty, group is pre-populated but unchanged, profile unchanged - should cancel
        let result = dialog.handle_key(key(KeyCode::Enter));
        assert!(matches!(result, DialogResult::Cancel));
    }

    #[test]
    fn test_enter_with_title_only_submits() {
        let mut dialog = RenameDialog::new(
            "Old Title",
            "group",
            "default",
            default_profiles(),
            Vec::new(),
        );
        dialog.handle_key(key(KeyCode::Char('N')));
        dialog.handle_key(key(KeyCode::Char('e')));
        dialog.handle_key(key(KeyCode::Char('w')));

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "New");
                assert_eq!(data.group, None); // Group unchanged
                assert_eq!(data.profile, None); // Profile unchanged
            }
            _ => panic!("Expected Submit result"),
        }
    }

    #[test]
    fn test_enter_with_group_only_submits() {
        let mut dialog = RenameDialog::new(
            "Title",
            "old-group",
            "default",
            default_profiles(),
            Vec::new(),
        );
        // Switch to group field and clear it
        dialog.handle_key(key(KeyCode::Tab));
        for _ in 0.."old-group".len() {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        // Type new group
        for c in "new-group".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, ""); // Title unchanged
                assert_eq!(data.group, Some("new-group".to_string()));
                assert_eq!(data.profile, None); // Profile unchanged
            }
            _ => panic!("Expected Submit result"),
        }
    }

    #[test]
    fn test_enter_with_both_fields_submits() {
        let mut dialog = RenameDialog::new(
            "Old Title",
            "old-group",
            "default",
            default_profiles(),
            Vec::new(),
        );
        // Type title
        for c in "New Title".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }
        // Switch to group field and clear it
        dialog.handle_key(key(KeyCode::Tab));
        for _ in 0.."old-group".len() {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        // Type new group
        for c in "new-group".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "New Title");
                assert_eq!(data.group, Some("new-group".to_string()));
                assert_eq!(data.profile, None); // Profile unchanged
            }
            _ => panic!("Expected Submit result"),
        }
    }

    #[test]
    fn test_clearing_group_removes_from_group() {
        let mut dialog = RenameDialog::new(
            "Title",
            "some-group",
            "default",
            default_profiles(),
            Vec::new(),
        );
        // Switch to group field and clear it
        dialog.handle_key(key(KeyCode::Tab));
        // Clear the pre-populated value
        for _ in 0.."some-group".len() {
            dialog.handle_key(key(KeyCode::Backspace));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "");
                assert_eq!(data.group, Some(String::new())); // Empty string means ungroup
                assert_eq!(data.profile, None);
            }
            _ => panic!("Expected Submit result"),
        }
    }

    #[test]
    fn test_tab_switches_fields() {
        // Session mode now has 5 fields: title, group, profile, heat, color.
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        assert_eq!(dialog.focused_field, 0);

        dialog.handle_key(key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 1);

        dialog.handle_key(key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 2);

        dialog.handle_key(key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 3); // heat

        dialog.handle_key(key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 4); // color

        dialog.handle_key(key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 0); // wrap
    }

    #[test]
    fn test_shift_tab_switches_fields_backwards() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        assert_eq!(dialog.focused_field, 0);

        dialog.handle_key(shift_key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 4); // wrap to color

        dialog.handle_key(shift_key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 3); // heat

        dialog.handle_key(shift_key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 2);
    }

    #[test]
    fn test_down_switches_to_next_field() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        assert_eq!(dialog.focused_field, 0);

        dialog.handle_key(key(KeyCode::Down));
        assert_eq!(dialog.focused_field, 1);

        dialog.handle_key(key(KeyCode::Down));
        assert_eq!(dialog.focused_field, 2);
    }

    #[test]
    fn test_up_switches_to_previous_field() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        dialog.focused_field = 2;

        dialog.handle_key(key(KeyCode::Up));
        assert_eq!(dialog.focused_field, 1);

        dialog.handle_key(key(KeyCode::Up));
        assert_eq!(dialog.focused_field, 0);
    }

    #[test]
    fn test_char_input_goes_to_focused_field() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());

        // Type in title field
        dialog.handle_key(key(KeyCode::Char('a')));
        assert_eq!(dialog.new_title.value(), "a");
        assert_eq!(dialog.new_group.value(), "group"); // Pre-populated

        // Switch to group and type (appends to pre-populated value)
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Char('b')));
        assert_eq!(dialog.new_title.value(), "a");
        assert_eq!(dialog.new_group.value(), "groupb");
    }

    #[test]
    fn test_char_input_ignored_on_profile_field() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", multi_profiles(), Vec::new());
        dialog.focused_field = 2; // Profile field

        // Typing should not affect anything
        dialog.handle_key(key(KeyCode::Char('a')));
        assert_eq!(dialog.profile_index, 0);
        assert_eq!(dialog.new_title.value(), "");
        assert_eq!(dialog.new_group.value(), "group");
    }

    #[test]
    fn test_backspace_removes_char_from_focused_field() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        dialog.handle_key(key(KeyCode::Char('a')));
        dialog.handle_key(key(KeyCode::Char('b')));
        dialog.handle_key(key(KeyCode::Char('c')));

        dialog.handle_key(key(KeyCode::Backspace));
        assert_eq!(dialog.new_title.value(), "ab");
    }

    #[test]
    fn test_current_values_preserved() {
        let mut dialog = RenameDialog::new(
            "Original",
            "original-group",
            "default",
            default_profiles(),
            Vec::new(),
        );
        dialog.handle_key(key(KeyCode::Char('N')));
        dialog.handle_key(key(KeyCode::Char('e')));
        dialog.handle_key(key(KeyCode::Char('w')));

        assert_eq!(dialog.current_title, "Original");
        assert_eq!(dialog.current_group, "original-group");
        assert_eq!(dialog.current_profile, "default");
        assert_eq!(dialog.new_title.value(), "New");
    }

    #[test]
    fn test_full_workflow_type_both_and_submit() {
        let mut dialog = RenameDialog::new(
            "Old Name",
            "old/group",
            "default",
            default_profiles(),
            Vec::new(),
        );

        // Type new title
        for c in "Renamed Project".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        // Switch to group and clear it, then type new group
        dialog.handle_key(key(KeyCode::Tab));
        for _ in 0.."old/group".len() {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        for c in "new/group".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "Renamed Project");
                assert_eq!(data.group, Some("new/group".to_string()));
                assert_eq!(data.profile, None);
            }
            _ => panic!("Expected Submit"),
        }
    }

    #[test]
    fn test_full_workflow_type_and_cancel() {
        let mut dialog = RenameDialog::new(
            "Old Name",
            "group",
            "default",
            default_profiles(),
            Vec::new(),
        );

        dialog.handle_key(key(KeyCode::Char('N')));
        dialog.handle_key(key(KeyCode::Char('e')));
        dialog.handle_key(key(KeyCode::Char('w')));

        let result = dialog.handle_key(key(KeyCode::Esc));
        assert!(matches!(result, DialogResult::Cancel));
    }

    #[test]
    fn test_whitespace_is_trimmed() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        for c in "  New Title  ".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }
        dialog.handle_key(key(KeyCode::Tab));
        // Clear pre-populated value first
        for _ in 0.."group".len() {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        for c in "  new-group  ".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "New Title");
                assert_eq!(data.group, Some("new-group".to_string()));
            }
            _ => panic!("Expected Submit"),
        }
    }

    #[test]
    fn test_left_right_arrow_moves_cursor_in_input() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        dialog.handle_key(key(KeyCode::Char('a')));
        dialog.handle_key(key(KeyCode::Char('b')));
        dialog.handle_key(key(KeyCode::Char('c')));

        // Move cursor left and insert
        dialog.handle_key(key(KeyCode::Left));
        dialog.handle_key(key(KeyCode::Char('X')));

        assert_eq!(dialog.new_title.value(), "abXc");
    }

    #[test]
    fn test_profile_selection_with_right_arrow() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", multi_profiles(), Vec::new());
        assert_eq!(dialog.profile_index, 0);
        assert_eq!(dialog.selected_profile(), "default");

        // Move to profile field
        dialog.focused_field = 2;

        // Cycle forward
        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.profile_index, 1);
        assert_eq!(dialog.selected_profile(), "work");

        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.profile_index, 2);
        assert_eq!(dialog.selected_profile(), "personal");

        // Wrap around
        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.profile_index, 0);
        assert_eq!(dialog.selected_profile(), "default");
    }

    #[test]
    fn test_profile_selection_with_space_key() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", multi_profiles(), Vec::new());
        dialog.focused_field = 2;

        // Space cycles forward like Right arrow
        dialog.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(dialog.profile_index, 1);
        assert_eq!(dialog.selected_profile(), "work");

        dialog.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(dialog.profile_index, 2);
        assert_eq!(dialog.selected_profile(), "personal");

        // Wrap around
        dialog.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(dialog.profile_index, 0);
        assert_eq!(dialog.selected_profile(), "default");
    }

    #[test]
    fn test_profile_selection_with_left_arrow() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", multi_profiles(), Vec::new());
        dialog.focused_field = 2;

        // Cycle backward (should wrap to end)
        dialog.handle_key(key(KeyCode::Left));
        assert_eq!(dialog.profile_index, 2);
        assert_eq!(dialog.selected_profile(), "personal");

        dialog.handle_key(key(KeyCode::Left));
        assert_eq!(dialog.profile_index, 1);
        assert_eq!(dialog.selected_profile(), "work");

        dialog.handle_key(key(KeyCode::Left));
        assert_eq!(dialog.profile_index, 0);
        assert_eq!(dialog.selected_profile(), "default");
    }

    #[test]
    fn test_profile_arrows_only_work_on_profile_field() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", multi_profiles(), Vec::new());
        assert_eq!(dialog.focused_field, 0); // Title field

        // Right arrow on title field should move cursor, not change profile
        dialog.handle_key(key(KeyCode::Char('a')));
        dialog.handle_key(key(KeyCode::Char('b')));
        let initial_profile = dialog.profile_index;
        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.profile_index, initial_profile);
    }

    #[test]
    fn test_submit_with_profile_change() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", multi_profiles(), Vec::new());

        // Change profile
        dialog.focused_field = 2;
        dialog.handle_key(key(KeyCode::Right)); // Select "work"

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "");
                assert_eq!(data.group, None);
                assert_eq!(data.profile, Some("work".to_string()));
            }
            _ => panic!("Expected Submit"),
        }
    }

    #[test]
    fn test_submit_with_all_changes() {
        let mut dialog = RenameDialog::new(
            "Old Title",
            "old-group",
            "default",
            multi_profiles(),
            Vec::new(),
        );

        // Change title
        for c in "New Title".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        // Change group
        dialog.handle_key(key(KeyCode::Tab));
        for _ in 0.."old-group".len() {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        for c in "new-group".chars() {
            dialog.handle_key(key(KeyCode::Char(c)));
        }

        // Change profile
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Right)); // Select "work"

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "New Title");
                assert_eq!(data.group, Some("new-group".to_string()));
                assert_eq!(data.profile, Some("work".to_string()));
            }
            _ => panic!("Expected Submit"),
        }
    }

    #[test]
    fn test_same_profile_returns_none() {
        let mut dialog = RenameDialog::new("Test", "group", "work", multi_profiles(), Vec::new());

        // Change title to trigger submit
        dialog.handle_key(key(KeyCode::Char('X')));

        // Profile stays at "work" (don't change it)
        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.profile, None); // Same profile, returns None
            }
            _ => panic!("Expected Submit"),
        }
    }

    fn ctrl_p() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
    }

    fn sample_groups() -> Vec<String> {
        vec![
            "work".to_string(),
            "work/frontend".to_string(),
            "personal".to_string(),
        ]
    }

    #[test]
    fn test_ctrl_p_opens_group_picker_on_group_field() {
        let mut dialog = RenameDialog::new(
            "Test",
            "group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        // Focus group field
        dialog.handle_key(key(KeyCode::Tab));
        assert_eq!(dialog.focused_field, 1);

        dialog.handle_key(ctrl_p());
        assert!(dialog.group_picker.is_active());
    }

    #[test]
    fn test_ctrl_p_ignored_on_title_field() {
        let mut dialog = RenameDialog::new(
            "Test",
            "group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        assert_eq!(dialog.focused_field, 0);

        dialog.handle_key(ctrl_p());
        assert!(!dialog.group_picker.is_active());
    }

    #[test]
    fn test_ctrl_p_ignored_on_profile_field() {
        let mut dialog = RenameDialog::new(
            "Test",
            "group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        dialog.focused_field = 2;

        dialog.handle_key(ctrl_p());
        assert!(!dialog.group_picker.is_active());
    }

    #[test]
    fn test_ctrl_p_ignored_when_no_groups() {
        let mut dialog =
            RenameDialog::new("Test", "group", "default", default_profiles(), Vec::new());
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(ctrl_p());
        assert!(!dialog.group_picker.is_active());
    }

    #[test]
    fn test_group_picker_select_sets_group_field() {
        let mut dialog = RenameDialog::new(
            "Test",
            "old-group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(ctrl_p()); // Open picker
        assert!(dialog.group_picker.is_active());

        // Select first item ("work")
        dialog.handle_key(key(KeyCode::Enter));
        assert!(!dialog.group_picker.is_active());
        assert_eq!(dialog.new_group.value(), "work");
    }

    #[test]
    fn test_group_picker_cancel_keeps_original_value() {
        let mut dialog = RenameDialog::new(
            "Test",
            "old-group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(ctrl_p()); // Open picker
        assert!(dialog.group_picker.is_active());

        // Cancel picker
        dialog.handle_key(key(KeyCode::Esc));
        assert!(!dialog.group_picker.is_active());
        assert_eq!(dialog.new_group.value(), "old-group");
    }

    #[test]
    fn test_group_picker_navigate_and_select() {
        let mut dialog = RenameDialog::new(
            "Test",
            "old-group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(ctrl_p()); // Open picker

        // Navigate down to second item ("work/frontend")
        dialog.handle_key(key(KeyCode::Down));
        dialog.handle_key(key(KeyCode::Enter));
        assert_eq!(dialog.new_group.value(), "work/frontend");
    }

    #[test]
    fn test_group_picker_selected_value_submits_correctly() {
        let mut dialog = RenameDialog::new(
            "Test",
            "old-group",
            "default",
            default_profiles(),
            sample_groups(),
        );
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(ctrl_p()); // Open picker
        dialog.handle_key(key(KeyCode::Enter)); // Select "work"

        let result = dialog.handle_key(key(KeyCode::Enter));
        match result {
            DialogResult::Submit(data) => {
                assert_eq!(data.group, Some("work".to_string()));
            }
            _ => panic!("Expected Submit"),
        }
    }

    // --- Group ghost autocomplete tests ---

    #[test]
    fn test_group_ghost_appears_on_typing() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(key(KeyCode::Char('p')));
        assert_eq!(dialog.group_ghost_text(), Some("ersonal"));
    }

    #[test]
    fn test_group_ghost_none_when_no_match() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Char('z')));
        assert!(dialog.group_ghost_text().is_none());
    }

    #[test]
    fn test_group_ghost_accept_with_right_arrow() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Char('p')));
        assert!(dialog.group_ghost_text().is_some());

        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.new_group.value(), "personal");
    }

    #[test]
    fn test_group_ghost_accept_with_end_key() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Char('p')));
        assert!(dialog.group_ghost_text().is_some());

        dialog.handle_key(key(KeyCode::End));
        assert_eq!(dialog.new_group.value(), "personal");
    }

    #[test]
    fn test_group_ghost_cleared_on_field_switch() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab)); // Focus group field
        dialog.handle_key(key(KeyCode::Char('p')));
        assert!(dialog.group_ghost_text().is_some());

        dialog.handle_key(key(KeyCode::Tab)); // Move to profile field
        assert!(dialog.group_ghost_text().is_none());
    }

    #[test]
    fn test_group_ghost_common_prefix_for_multiple_matches() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Char('w')));
        // "work" and "work/frontend" share common prefix "work"
        // Ghost should show "ork" (common prefix minus typed "w")
        assert_eq!(dialog.group_ghost_text(), Some("ork"));
    }

    #[test]
    fn test_group_ghost_cleared_on_picker_select() {
        let mut dialog =
            RenameDialog::new("Test", "", "default", default_profiles(), sample_groups());
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Char('w')));
        assert!(dialog.group_ghost_text().is_some());

        dialog.handle_key(ctrl_p()); // Open picker
        dialog.handle_key(key(KeyCode::Enter)); // Select "work"
        assert!(dialog.group_ghost_text().is_none());
        assert_eq!(dialog.new_group.value(), "work");
    }

    // --- Group rename duplicate validation tests ---

    fn existing_groups_with_personal() -> Vec<String> {
        vec![
            "work".to_string(),
            "personal".to_string(),
            "work/frontend".to_string(),
        ]
    }

    #[test]
    fn test_group_rename_duplicate_shows_error() {
        let mut dialog = RenameDialog::new_for_group(
            "work",
            "default",
            default_profiles(),
            existing_groups_with_personal(),
        );

        // Clear the pre-filled group name and type an existing group name
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        for ch in "personal".chars() {
            dialog.handle_key(key(KeyCode::Char(ch)));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(result, DialogResult::Continue),
            "should not submit when duplicate name"
        );
        assert!(
            dialog.validation_error.is_some(),
            "validation_error should be set"
        );
    }

    #[test]
    fn test_group_rename_error_clears_on_edit() {
        let mut dialog = RenameDialog::new_for_group(
            "work",
            "default",
            default_profiles(),
            existing_groups_with_personal(),
        );

        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        for ch in "personal".chars() {
            dialog.handle_key(key(KeyCode::Char(ch)));
        }
        dialog.handle_key(key(KeyCode::Enter));
        assert!(dialog.validation_error.is_some());

        // Any keystroke on the group field should clear the error
        dialog.handle_key(key(KeyCode::Backspace));
        assert!(
            dialog.validation_error.is_none(),
            "validation_error should clear on edit"
        );
    }

    #[test]
    fn test_group_rename_allows_own_name() {
        let mut dialog = RenameDialog::new_for_group(
            "work",
            "default",
            default_profiles(),
            existing_groups_with_personal(),
        );

        // Submitting the unchanged name should cancel (nothing changed), not error
        let result = dialog.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(result, DialogResult::Cancel),
            "unchanged name should cancel, not show duplicate error"
        );
        assert!(
            dialog.validation_error.is_none(),
            "no validation error for own name"
        );
    }

    // --- Branch-rename toggle (tied worktree) tests ---

    fn tied_dialog(upstream: Option<&str>) -> RenameDialog {
        RenameDialog::new("hi", "", "default", default_profiles(), Vec::new())
            .with_worktree_branch("thing", upstream.map(|s| s.to_string()))
    }

    #[test]
    fn test_branch_toggle_absent_without_worktree_context() {
        // A plain session (no with_worktree_branch) has no 4th field and
        // never emits rename_branch=true.
        let mut dialog = RenameDialog::new("hi", "", "default", default_profiles(), Vec::new());
        // title, group, profile, heat, color (no branch toggle).
        assert_eq!(dialog.field_count(), 5);
        assert!(!dialog.shows_branch_toggle());
        dialog.handle_key(key(KeyCode::Char('x')));
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => assert!(!data.rename_branch),
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn test_branch_toggle_present_for_tied_worktree() {
        let dialog = tied_dialog(Some("origin/thing"));
        assert!(dialog.shows_branch_toggle());
        // title, group, profile, branch toggle, heat, color.
        assert_eq!(dialog.field_count(), 6);
    }

    #[test]
    fn test_branch_toggle_defaults_off_and_flips_with_space() {
        let mut dialog = tied_dialog(None);
        // Tab title -> group -> profile -> toggle (index 3).
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Tab));
        assert!(dialog.is_branch_toggle_field());
        assert!(!dialog.rename_branch);
        dialog.handle_key(key(KeyCode::Char(' ')));
        assert!(dialog.rename_branch);
        // Space again toggles back off.
        dialog.handle_key(key(KeyCode::Char(' ')));
        assert!(!dialog.rename_branch);
    }

    #[test]
    fn test_branch_toggle_emitted_in_submit() {
        let mut dialog = tied_dialog(Some("origin/thing"));
        // Change the title so submit is not a no-op, then arm the toggle.
        dialog.handle_key(key(KeyCode::Char('x')));
        dialog.handle_key(key(KeyCode::Tab)); // group
        dialog.handle_key(key(KeyCode::Tab)); // profile
        dialog.handle_key(key(KeyCode::Tab)); // toggle
        dialog.handle_key(key(KeyCode::Char(' ')));
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, "x");
                assert!(data.rename_branch);
            }
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn test_branch_toggle_can_rename_branch_without_title_change() {
        // The toggle must be usable to bring a drifted branch in line with
        // the title even when the title itself is unchanged: arming it makes
        // the dialog submit (not cancel) so the rename flow runs.
        let mut dialog = tied_dialog(None);
        dialog.handle_key(key(KeyCode::Tab)); // group
        dialog.handle_key(key(KeyCode::Tab)); // profile
        dialog.handle_key(key(KeyCode::Tab)); // toggle
        dialog.handle_key(key(KeyCode::Char(' ')));
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => {
                assert_eq!(data.title, ""); // title unchanged
                assert!(data.rename_branch);
            }
            _ => panic!("expected submit even with no title change"),
        }
    }

    #[test]
    fn test_space_still_cycles_profile_not_branch_toggle() {
        // The branch-toggle space handler must not steal space from the
        // profile chip.
        let mut dialog = RenameDialog::new("hi", "", "default", multi_profiles(), Vec::new())
            .with_worktree_branch("thing", None);
        dialog.handle_key(key(KeyCode::Tab)); // group
        dialog.handle_key(key(KeyCode::Tab)); // profile
        assert!(dialog.is_profile_field());
        dialog.handle_key(key(KeyCode::Char(' '))); // cycle profile
        assert_eq!(dialog.profile_index, 1);
        assert!(!dialog.rename_branch);
    }

    // --- Heat / Color r-menu extensions ---

    #[test]
    fn session_has_heat_and_color_fields_group_has_only_color() {
        let session = RenameDialog::new("t", "", "default", default_profiles(), Vec::new());
        // title, group, profile, heat, color
        assert_eq!(session.field_count(), 5);
        assert!(!session.is_heat_field()); // focus starts on title
        let group = RenameDialog::new_for_group("g", "default", default_profiles(), Vec::new());
        // group, profile, color (no heat field in Group mode)
        assert_eq!(group.field_count(), 3);
    }

    #[test]
    fn heat_field_space_cycles_enter_submits() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(None, None);
        // Tab to the heat field (title->group->profile->heat).
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Tab));
        assert!(dialog.is_heat_field());
        dialog.handle_key(key(KeyCode::Char(' '))); // None -> Some(true)
        dialog.handle_key(key(KeyCode::Char(' '))); // Some(true) -> Some(false)
                                                    // Enter on the heat field now SUBMITS (Space is the only cycle key),
                                                    // matching the app-wide Enter=confirm convention and the color field.
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => {
                assert_eq!(data.heat_enabled, Some(Some(false)));
                assert_eq!(data.manual_color, None);
            }
            _ => panic!("Enter on the heat field should submit"),
        }
    }

    #[test]
    fn color_field_cycles_inline_through_palette_and_none() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(None, None);
        // Tab to color field (title->group->profile->heat->color).
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        assert!(dialog.is_color_field());
        assert_eq!(dialog.manual_color, None);
        // Right steps off the "none" slot onto the first palette color in place
        // — no popup, the field itself holds the live choice.
        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.manual_color, Some(FolderColor::ALL[0]));
        // Left steps back to "none" (wrapping through the trailing slot).
        dialog.handle_key(key(KeyCode::Left));
        assert_eq!(dialog.manual_color, None);
        // Cycling Right through every palette entry then once more lands back on
        // "none", covering the whole ring.
        for expected in FolderColor::ALL.iter() {
            dialog.handle_key(key(KeyCode::Right));
            assert_eq!(dialog.manual_color, Some(*expected));
        }
        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.manual_color, None);
        // Pick the first color and submit; the change is carried out.
        dialog.handle_key(key(KeyCode::Right));
        dialog.handle_key(key(KeyCode::Tab)); // off color, back to title
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => {
                assert_eq!(data.manual_color, Some(Some(FolderColor::ALL[0])));
            }
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn color_field_cycles_to_none_clears_existing_color() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(Some(FolderColor::Teal), None);
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        assert!(dialog.is_color_field());
        assert_eq!(dialog.manual_color, Some(FolderColor::Teal));
        // Cycle left until the field clears to "none".
        while dialog.manual_color.is_some() {
            dialog.handle_key(key(KeyCode::Left));
        }
        assert_eq!(dialog.manual_color, None);
        dialog.handle_key(key(KeyCode::Tab));
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => assert_eq!(data.manual_color, Some(None)),
            _ => panic!("expected submit clearing color"),
        }
    }

    #[test]
    fn group_color_change_only_when_edited() {
        let mut dialog =
            RenameDialog::new_for_group("g", "default", default_profiles(), Vec::new())
                .with_group_color(Some(FolderColor::Sky));
        // Unchanged: no group color change reported, and an otherwise-unchanged
        // dialog cancels.
        assert_eq!(dialog.group_color_change(), None);
        // Tab to color field (group->profile->color) and cycle to a different
        // color inline.
        dialog.handle_key(key(KeyCode::Tab));
        dialog.handle_key(key(KeyCode::Tab));
        assert!(dialog.is_color_field());
        while dialog.group_color != Some(FolderColor::ALL[0]) {
            dialog.handle_key(key(KeyCode::Right));
        }
        assert_eq!(dialog.group_color_change(), Some(Some(FolderColor::ALL[0])));
    }

    #[test]
    fn only_color_change_submits_not_cancels() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(None, None);
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        dialog.handle_key(key(KeyCode::Right)); // cycle onto the first color
        assert_eq!(dialog.manual_color, Some(FolderColor::ALL[0]));
        dialog.handle_key(key(KeyCode::Tab)); // off color
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(_) => {}
            _ => panic!("a sole color change must submit"),
        }
    }

    #[test]
    fn color_grid_opens_navigates_and_commits() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(None, None);
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        assert!(dialog.is_color_field());
        // Space opens the grid; with no current color it preselects swatch 0.
        dialog.handle_key(key(KeyCode::Char(' ')));
        assert!(dialog.color_picker_open);
        assert_eq!(dialog.color_picker_index, 0);
        // Right steps one swatch; Down steps a full row.
        dialog.handle_key(key(KeyCode::Right));
        dialog.handle_key(key(KeyCode::Down));
        assert_eq!(dialog.color_picker_index, 1 + COLOR_GRID_COLS);
        // Enter commits the highlighted swatch and closes the grid.
        dialog.handle_key(key(KeyCode::Enter));
        assert!(!dialog.color_picker_open);
        assert_eq!(
            dialog.manual_color,
            Some(FolderColor::ALL[1 + COLOR_GRID_COLS])
        );
    }

    #[test]
    fn color_grid_wraps_as_torus() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(None, None);
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        dialog.handle_key(key(KeyCode::Char(' '))); // open at index 0
                                                    // Left from the first cell wraps to the last.
        dialog.handle_key(key(KeyCode::Left));
        assert_eq!(dialog.color_picker_index, FolderColor::ALL.len() - 1);
        // Back to 0, then Up wraps to the bottom row of the same column.
        dialog.handle_key(key(KeyCode::Right));
        assert_eq!(dialog.color_picker_index, 0);
        dialog.handle_key(key(KeyCode::Up));
        assert_eq!(
            dialog.color_picker_index,
            FolderColor::ALL.len() - COLOR_GRID_COLS
        );
    }

    #[test]
    fn color_grid_backspace_clears_and_esc_keeps() {
        // Backspace inside the grid clears the color outright.
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(Some(FolderColor::Teal), None);
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        dialog.handle_key(key(KeyCode::Char(' '))); // open, preselects Teal
        let teal_idx = FolderColor::ALL
            .iter()
            .position(|c| *c == FolderColor::Teal)
            .unwrap();
        assert_eq!(dialog.color_picker_index, teal_idx);
        dialog.handle_key(key(KeyCode::Backspace));
        assert!(!dialog.color_picker_open);
        assert_eq!(dialog.manual_color, None);

        // Esc leaves the existing color untouched even after moving the cursor.
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new())
            .with_session_settings(Some(FolderColor::Teal), None);
        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Tab));
        }
        dialog.handle_key(key(KeyCode::Char(' ')));
        dialog.handle_key(key(KeyCode::Right));
        dialog.handle_key(key(KeyCode::Esc));
        assert!(!dialog.color_picker_open);
        assert_eq!(dialog.manual_color, Some(FolderColor::Teal));
    }

    #[test]
    fn legacy_new_without_settings_reports_no_color_heat_change() {
        // A dialog built without `with_session_settings` behaves like before:
        // an unchanged session submit reports no color/heat change.
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new());
        dialog.handle_key(key(KeyCode::Char('x'))); // change title so it submits
        match dialog.handle_key(key(KeyCode::Enter)) {
            DialogResult::Submit(data) => {
                assert_eq!(data.manual_color, None);
                assert_eq!(data.heat_enabled, None);
            }
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn tab_reaches_heat_and_color_fields() {
        let mut dialog = RenameDialog::new("t", "", "default", default_profiles(), Vec::new());
        let order: Vec<usize> = (0..dialog.field_count())
            .map(|_| {
                let f = dialog.focused_field;
                dialog.handle_key(key(KeyCode::Tab));
                f
            })
            .collect();
        // 0 title, 1 group, 2 profile, 3 heat, 4 color.
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_group_rename_submit_new_unique_name() {
        let mut dialog = RenameDialog::new_for_group(
            "work",
            "default",
            default_profiles(),
            existing_groups_with_personal(),
        );

        for _ in 0..4 {
            dialog.handle_key(key(KeyCode::Backspace));
        }
        for ch in "projects".chars() {
            dialog.handle_key(key(KeyCode::Char(ch)));
        }

        let result = dialog.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(result, DialogResult::Submit(_)),
            "unique name should submit"
        );
        assert!(dialog.validation_error.is_none());
    }
}
