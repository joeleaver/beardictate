//! What the finisher takes in from the recognizer: transcripts, through the
//! voice wake gate, the vocabulary and the command pass, into the queue;
//! control messages; and new settings.

use std::time::{Duration, Instant};

use bd_commands::CommandPass;
use crossbeam_channel::Receiver;

use super::gate::Step;
use super::rejoin::Ending;
use super::{Finisher, Heard, Item, Pending, list};
use crate::{PadAction, PipelineConfig};

impl Finisher {
    pub(super) fn take_in_ready(&mut self, input: &Receiver<Heard>) {
        while let Ok(heard) = input.try_recv() {
            self.take_in(heard);
        }
    }

    pub(super) fn take_in(&mut self, heard: Heard) {
        match &heard {
            Heard::Text { id, .. } => {
                if self.speaking == Some(*id) {
                    self.speaking = None;
                }
                self.rejoin.seen(*id);
            }
            Heard::Partial { id, text, .. } if !text.is_empty() => self.rejoin.seen(*id),
            _ => {}
        }
        let (id, text, pause, ending) = match heard {
            Heard::Text { id, text, pause, ending } => (id, text, pause, ending),
            Heard::EndSession => {
                self.rejoin.forget();
                return self.queue.push_back(Item::EndSession);
            }
            Heard::Reconfigure(config) => return self.reconfigure(*config),
            Heard::Partial { id, text, settled } => {
                self.speaking = (!text.is_empty()).then_some(id);
                // Typed a sentence at a time, it only says someone's speaking.
                if self.types_live() || self.pad.is_some() {
                    self.queue.push_back(Item::Partial(id, text, settled));
                }
                return;
            }
            Heard::Pad(action) => return self.pad_action(action),
            Heard::Awake(awake) => {
                self.rejoin.forget();
                let step = self.gate.set(awake);
                return self.follow(step, 0, None, Ending::Alone);
            }
            Heard::NewPassage => {
                self.quote_open = false;
                self.passage_has_text = false;
                self.rejoin.forget();
                return self.queue.push_back(Item::NewPassage);
            }
        };
        let words = text.split_whitespace().count();
        let step = self.gate.pass(text);
        let kept = step.pass.as_deref().map_or(0, |t| t.split_whitespace().count());
        if kept < words {
            tracing::info!(segment = id, words, kept, woke = step.woke, sleeps = step.sleep_after, "voice wake kept only part");
        }
        self.follow(step, id, pause, ending);
    }

    /// Carries out what the voice wake gate decided.
    fn follow(&mut self, step: Step, id: u64, pause: Option<Duration>, ending: Ending) {
        if step.woke {
            if step.fresh {
                self.take_in(Heard::NewPassage);
            }
            self.queue.push_back(Item::Wake);
        }
        if let Some(text) = step.pass {
            self.take_in_text(id, text, pause, ending);
        }
        if step.sleep_after {
            self.rejoin.forget();
            self.queue.push_back(Item::Sleep);
        }
    }

    fn take_in_text(&mut self, id: u64, heard: String, pause: Option<Duration>, ending: Ending) {
        if self.pad.is_none() {
            self.new_passage_if_moved_on();
        }
        let text = self.own_words(&heard);
        let (text, pad_action) = self.pad_command(text);
        let mut out = self.pass.apply(&text, self.quote_open);
        let scratches = out.scratch_previous();
        if scratches > 0 {
            let mut restored = None;
            for _ in 0..scratches {
                match self.drop_latest() {
                    Some(quote_before) => restored = Some(quote_before),
                    None => {
                        self.emit(crate::Event::ScratchTooLate);
                        break;
                    }
                }
            }
            if let Some(quote) = restored {
                self.quote_open = quote;
                out = self.pass.apply(&text, quote);
            }
        }
        let quote_before = self.quote_open;
        self.quote_open = out.quote_open;
        self.note_commanded(id, &heard, &text, &out, pad_action.as_ref());
        // Whether it carries on what was said before a pause, read before
        // it's made a list item; then it's what the next one reads on from.
        // Not when it scratched what it would carry on, nor in the pad.
        if scratches > 0 {
            self.rejoin.refuse(id);
        }
        let joins = scratches == 0 && out.has_words() && pad_action.is_none() && self.pad.is_none() && self.joins_on(id, &out.text);
        let sentence_case = self.case_mode == crate::CaseMode::Normal && self.pad.is_none() && pad_action.is_none();
        self.rejoin.taken(id, ending, &out, self.list_mode, sentence_case);
        let cased = self.case_mode != crate::CaseMode::Normal && out.has_words();
        let list = (self.list_mode || cased) && out.has_words();
        if cased {
            out.text = self.in_case(&out.text, &out.actions);
        }
        if self.list_mode && out.has_words() {
            let new_line = self.passage_has_text && !joins;
            out.text = list::item(&out.text, &out.actions, new_line, &self.vocabulary.spellings());
        } else if std::mem::take(&mut self.after_list) && out.has_words() && !out.text.starts_with('\n') {
            out.text.insert(0, '\n');
        }
        let long_pause = !list && !joins && matches!((pause, self.pause_paragraph), (Some(p), Some(min)) if p >= min);
        if long_pause && self.passage_has_text && !out.text.starts_with('\n') && out.has_words() {
            out.text.insert_str(0, "\n\n");
        }
        if out.has_words() {
            self.passage_has_text = true;
        }
        // Live text may be on the page already: an utterance that comes to
        // nothing still has to take it back.
        let typed_live = self.types_live();
        if !out.text.is_empty() || typed_live {
            if self.pad.is_some() {
                self.pad_push(id, out.text, quote_before);
            } else {
                self.queue.push_back(Item::Segment(Pending { id, quote_before, out, ready: Instant::now(), list }));
            }
        }
        match pad_action {
            Some(PadAction::List(on)) => self.set_list(on),
            Some(PadAction::Case(mode)) => self.set_case(mode),
            Some(action) if self.pad.is_some() => self.pad_action(action),
            // In order, after what was said before it.
            Some(action) => self.queue.push_back(Item::Edit(action)),
            None => {}
        }
    }

    /// Segments already taken in keep the settings they were read with.
    pub(super) fn reconfigure(&mut self, config: PipelineConfig) {
        self.vocabulary = config.vocabulary;
        self.pass = CommandPass::new(config.commands, config.style.clone());
        self.style = config.style;
        self.pause_paragraph = config.pause_paragraph;
        self.sentence_hold = config.sentence_hold;
        self.sentence_hold_speaking = config.sentence_hold_speaking;
        self.live_typing = config.live_typing;
        self.list_mode = config.list_mode;
        self.case_mode = config.case_mode;
        self.rejoin.configure(config.rejoin, config.list_join);
        if !(self.live && self.live_typing && !config.pad) {
            self.tidy_stop();
        }
        self.context_window = config.context_window;
        self.cleanup_context_chars = config.cleanup_context_chars;
        self.passage_gap = config.passage_gap;
        self.blank_line_paragraphs = config.blank_line_paragraphs;
        self.gate.configure(config.voice_wake);
        self.set_pad(config.pad);
    }
}
