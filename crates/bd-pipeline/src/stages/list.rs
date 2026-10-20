//! List mode ("bear, start a list" … "bear, end the list"): each thing said
//! is an item on a line of its own, for the editor's bullets. Items start
//! without a capital and end without the full stop the recognizer adds,
//! and they aren't tidied (the model would put both back). A mark said out
//! loud stays.

use bd_commands::{Action, ActionKind};

use super::Finisher;
use crate::Event;

impl Finisher {
    /// List mode on or off, by voice ("bear, start a list") or reconfigured.
    pub(super) fn set_list(&mut self, on: bool) {
        if self.list_mode != on {
            tracing::info!(on, "list mode");
            self.list_mode = on;
            self.after_list = !on && self.passage_has_text;
            self.emit(Event::ListMode(on));
        }
    }

    /// A case mode, by voice ("bear, all caps") or reconfigured.
    pub(super) fn set_case(&mut self, mode: crate::CaseMode) {
        if self.case_mode != mode {
            tracing::info!(?mode, "case mode");
            self.case_mode = mode;
            self.emit(Event::CaseMode(mode));
        }
    }

    /// One segment's commanded text in the case mode.
    pub(super) fn in_case(&self, text: &str, actions: &[Action]) -> String {
        let said_last = said_last_mark(text, actions);
        crate::case::apply(text, self.case_mode, &self.vocabulary.spellings(), said_last)
    }
}

/// Whether the speaker said the mark at the end ("…, question mark").
fn said_last_mark(text: &str, actions: &[Action]) -> bool {
    actions.iter().any(|a| match &a.kind {
        ActionKind::Punctuation(mark) => a.offset + mark.len() >= text.trim_end_matches([' ', '\t']).len(),
        _ => false,
    })
}

/// `text`, one segment's commanded text, as a list item: on a new line
/// when `new_line` (there's text before it in this passage), its first word
/// in lower case unless it's "I", in capitals, or one of `names`, and
/// without the recognizer's mark at its end.
pub(super) fn item(text: &str, actions: &[Action], new_line: bool, names: &[String]) -> String {
    let body = text.trim_matches([' ', '\t']);
    if !body.chars().any(char::is_alphanumeric) {
        return text.to_string();
    }
    let mut body = body.to_string();
    // The recognizer's full stop (or ours, at a pause) comes off; a mark the
    // speaker said ("…, question mark") stays.
    if !said_last_mark(text, actions) {
        let trimmed = body.trim_end_matches(['.', '!', '?', '\u{2026}', ',', ';', ':']).len();
        body.truncate(trimmed);
    }
    lower_first_word(&mut body, names);
    if new_line && !body.starts_with('\n') {
        body.insert(0, '\n');
    }
    body
}

fn lower_first_word(body: &mut String, names: &[String]) {
    let Some(start) = body.find(char::is_alphabetic) else { return };
    let word: String = body[start..].chars().take_while(|c| c.is_alphanumeric() || *c == '\'' || *c == '\u{2019}').collect();
    let first = word.chars().next().unwrap_or(' ');
    let rest_upper = word.chars().skip(1).any(char::is_uppercase);
    let keep = word == "I"
        || word.starts_with("I'")
        || word.starts_with("I\u{2019}")
        || rest_upper
        || names.iter().any(|n| {
            let bare = word.trim_end_matches("'s").trim_end_matches("\u{2019}s");
            n.split_whitespace().next() == Some(bare)
        });
    if keep || !first.is_uppercase() {
        return;
    }
    let lower: String = first.to_lowercase().collect();
    body.replace_range(start..start + first.len_utf8(), &lower);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(text: &str, new_line: bool) -> String {
        item(text, &[], new_line, &["Ysolde".to_string()])
    }

    #[test]
    fn an_item_is_bare_and_on_its_own_line() {
        assert_eq!(i("Eggs.", false), "eggs");
        assert_eq!(i("Milk and bread.", true), "\nmilk and bread");
        assert_eq!(i("I need stamps.", true), "\nI need stamps");
        assert_eq!(i("NASA forms.", true), "\nNASA forms");
        assert_eq!(i("Ysolde's book.", true), "\nYsolde's book");
        assert_eq!(i("\nOpen the door.", true), "\nopen the door");
    }

    #[test]
    fn a_mark_said_out_loud_stays() {
        let actions = [Action { kind: ActionKind::Punctuation("?"), offset: 13 }];
        assert_eq!(item("Call the vet?", &actions, false, &[]), "call the vet?");
    }
}
