//! Stage 1: the microphone gain, then the VAD cutting audio into utterances.

use crossbeam_channel::{Receiver, Sender};

use bd_core::{AudioSegment, LiveAudio};

use super::Cut;
use crate::gain::InputGain;
use crate::{Input, Segmenting};

/// Stage 1: cut audio into utterances. Numbers segments over the pipeline's
/// life. When the input closes mid-session, cuts what is in progress. With
/// `live`, hands out each utterance's audio as it is spoken
/// ([`Cut::Live`]) for a streaming recognizer.
pub(super) fn segment(mut segmenter: Box<dyn Segmenting>, input: Receiver<Input>, out: Sender<Cut>, gain: InputGain, live: bool) {
    let mut next_id = 0u64;
    let mut used = 1.0;
    let mut send_segments = |segs: Vec<AudioSegment>, used: f32, out: &Sender<Cut>| {
        for seg in segs {
            let _ = out.send(Cut::Segment(next_id, seg, used));
            next_id += 1;
        }
    };
    // The live utterance's id: taken at its start, kept until its end.
    let mut live_id = None;
    let mut send_live = |audio: Vec<LiveAudio>, used: f32, out: &Sender<Cut>, next: &mut u64| {
        for a in audio {
            let id = *live_id.get_or_insert_with(|| {
                *next += 1;
                *next - 1
            });
            if matches!(a, LiveAudio::End(_) | LiveAudio::Drop) {
                live_id = None;
            }
            let _ = out.send(Cut::Live(id, a, used));
        }
    };
    let mut live_next = 0u64;
    let mut send_all = |segmenter: &mut Box<dyn Segmenting>, samples: Option<&[f32]>, used: f32, out: &Sender<Cut>| match (live, samples) {
        (false, Some(s)) => send_segments(segmenter.accept(s), used, out),
        (false, None) => send_segments(segmenter.flush(), used, out),
        (true, Some(s)) => send_live(segmenter.accept_live(s), used, out, &mut live_next),
        (true, None) => send_live(segmenter.flush_live(), used, out, &mut live_next),
    };
    let mut dirty = false;
    for msg in &input {
        match msg {
            Input::Audio(mut samples) => {
                dirty = true;
                used = gain.apply(&mut samples);
                send_all(&mut segmenter, Some(&samples), used, &out);
            }
            Input::EndSession => {
                dirty = false;
                send_all(&mut segmenter, None, used, &out);
                let _ = out.send(Cut::EndSession);
            }
            Input::NewPassage => {
                let _ = out.send(Cut::NewPassage);
            }
            Input::Reconfigure(config) => {
                gain.set(config.input_gain, config.auto_gain);
                let _ = out.send(Cut::Reconfigure(config));
            }
            Input::Awake(awake) => {
                let _ = out.send(Cut::Awake(awake));
            }
            Input::Pad(action) => {
                let _ = out.send(Cut::Pad(action));
            }
        }
    }
    if dirty {
        send_all(&mut segmenter, None, used, &out);
        let _ = out.send(Cut::EndSession);
    }
}
