//! The bear on screen, on the UI side: its menu, sent to the engine, which
//! runs the bear's window ([`crate::pad_window`]) and hands the choices
//! back ([`crate::engine::Ui::act`]).

use std::cell::RefCell;

use rinch::reactive::Effect;

use crate::engine::Command;
use crate::menu::{self, Entry};
use crate::ui::Shared;

/// Keeps the bear's menu current.
pub fn wire(shared: &Shared) {
    let s = shared.clone();
    let last = RefCell::new(None::<Vec<Entry>>);
    Effect::new(move || {
        let entries = menu::entries(&s);
        if last.borrow().as_ref() != Some(&entries) {
            *last.borrow_mut() = Some(entries.clone());
            crate::engine_send(Command::BearMenu(entries));
        }
    });
}
