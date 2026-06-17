use tui_lipan::prelude::*;

use crate::state::{Direction, InputConfig, RATIO_STEP};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Spawn,
    Close,
    Focus(Direction),
    SwitchWorkspace(usize),
    MoveToWorkspace(usize),
    ToggleFloat,
    ToggleFullscreen,
    FlipSplit,
    AdjustRatio(f32),
}

pub fn is_prefix_key(key: KeyEvent, config: InputConfig) -> bool {
    key == config.prefix
}

pub fn action_for_held(key: KeyEvent, config: InputConfig) -> Option<Action> {
    if !config.modifier.matches(key) || key.mods.ctrl {
        return None;
    }
    action_for_command_key(key)
}

pub fn action_for_prefix(key: KeyEvent) -> Option<Action> {
    if key.mods.ctrl || key.mods.alt || key.mods.super_key {
        return None;
    }
    action_for_command_key(key)
}

fn action_for_command_key(key: KeyEvent) -> Option<Action> {
    if let Some((index, symbol_implies_shift)) = workspace_key(key) {
        return Some(if key.mods.shift || symbol_implies_shift {
            Action::MoveToWorkspace(index)
        } else {
            Action::SwitchWorkspace(index)
        });
    }

    match key.code {
        KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::Spawn),
        KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char('x') | KeyCode::Char('X') => {
            Some(Action::Close)
        }
        KeyCode::Char('t') | KeyCode::Char('T') => Some(Action::ToggleFloat),
        KeyCode::Char('f') | KeyCode::Char('F') => Some(Action::ToggleFullscreen),
        KeyCode::Char(' ') => Some(Action::FlipSplit),
        KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => {
            Some(Action::Focus(Direction::Left))
        }
        KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => {
            Some(Action::Focus(Direction::Down))
        }
        KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => Some(Action::Focus(Direction::Up)),
        KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => {
            Some(Action::Focus(Direction::Right))
        }
        KeyCode::Char('[') | KeyCode::Char('-') => Some(Action::AdjustRatio(-RATIO_STEP)),
        KeyCode::Char(']') | KeyCode::Char('=') | KeyCode::Char('+') => {
            Some(Action::AdjustRatio(RATIO_STEP))
        }
        _ => None,
    }
}

fn workspace_key(key: KeyEvent) -> Option<(usize, bool)> {
    let (digit, symbol_implies_shift) = match key.code {
        KeyCode::Char('1') => (1, false),
        KeyCode::Char('2') => (2, false),
        KeyCode::Char('3') => (3, false),
        KeyCode::Char('4') => (4, false),
        KeyCode::Char('5') => (5, false),
        KeyCode::Char('6') => (6, false),
        KeyCode::Char('7') => (7, false),
        KeyCode::Char('8') => (8, false),
        KeyCode::Char('9') => (9, false),
        KeyCode::Char('!') => (1, true),
        KeyCode::Char('@') => (2, true),
        KeyCode::Char('#') => (3, true),
        KeyCode::Char('$') => (4, true),
        KeyCode::Char('%') => (5, true),
        KeyCode::Char('^') => (6, true),
        KeyCode::Char('&') => (7, true),
        KeyCode::Char('*') => (8, true),
        KeyCode::Char('(') => (9, true),
        _ => return None,
    };

    Some((digit - 1, symbol_implies_shift))
}
