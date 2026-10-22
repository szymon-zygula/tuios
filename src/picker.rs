use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Fuzzy-filtered selection over a list of strings owned by the caller.
pub struct Picker {
    pub query: String,
    pub matches: Vec<Match>,
    pub selected: usize,
    /// Keeps the scroll offset between frames.
    pub state: ratatui::widgets::TableState,
    matcher: Matcher,
}

pub struct Match {
    pub index: usize,
    /// Character positions in the item that matched the query, for highlighting.
    pub positions: Vec<u32>,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            state: Default::default(),
            matcher: Matcher::new(Config::DEFAULT),
        }
    }
}

impl Picker {
    pub fn reset(&mut self, items: &[String]) {
        self.query.clear();
        self.selected = 0;
        self.refilter(items, false);
    }

    /// Recomputes matches. With `keep`, the same item stays selected if still visible,
    /// so live updates to the list do not move the cursor under the user.
    pub fn refilter(&mut self, items: &[String], keep: bool) {
        let previous = keep.then(|| self.selected_index()).flatten();
        self.matches.clear();
        if self.query.is_empty() {
            self.matches.extend((0..items.len()).map(|index| Match {
                index,
                positions: Vec::new(),
            }));
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
            let mut scored = Vec::new();
            let mut buf = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let mut positions = Vec::new();
                if let Some(score) = pattern.indices(
                    Utf32Str::new(item, &mut buf),
                    &mut self.matcher,
                    &mut positions,
                ) {
                    positions.sort_unstable();
                    positions.dedup();
                    scored.push((score, Match { index, positions }));
                }
            }
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index.cmp(&b.1.index)));
            self.matches.extend(scored.into_iter().map(|(_, m)| m));
        }
        self.selected = previous
            .and_then(|p| self.matches.iter().position(|m| m.index == p))
            .unwrap_or(if keep { self.selected } else { 0 })
            .min(self.matches.len().saturating_sub(1));
    }

    /// Empties the filter while keeping the cursor on the same item.
    pub fn clear_query(&mut self, items: &[String]) {
        self.query.clear();
        self.refilter(items, true);
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.matches.get(self.selected).map(|m| m.index)
    }

    pub fn select_item(&mut self, index: usize) {
        if let Some(pos) = self.matches.iter().position(|m| m.index == index) {
            self.selected = pos;
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Handles query editing and cursor movement; returns false for keys it ignores.
    pub fn handle_key(&mut self, key: KeyEvent, items: &[String]) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Char('p' | 'k') if ctrl => self.move_by(-1),
            KeyCode::Char('n' | 'j') if ctrl => self.move_by(1),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::Home if ctrl => self.selected = 0,
            KeyCode::End if ctrl => self.move_by(isize::MAX / 2),
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.refilter(items, false);
            }
            KeyCode::Char('w') if ctrl => {
                let trimmed = self.query.trim_end();
                let cut = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
                self.query.truncate(cut);
                self.refilter(items, false);
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter(items, false);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.query.push(c);
                self.refilter(items, false);
            }
            _ => return false,
        }
        true
    }
}
