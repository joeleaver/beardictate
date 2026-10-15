//! Stage 1: the microphone gain, then the VAD cutting audio into utterances.

use crossbeam_channel::{Receiver, Sender};

use bd_core::AudioSegment;

use super::Cut;
use crate::gain::InputGain;
use crate::{Input, Segmenting};

/// Stage 1: cut audio into utterances. Numbers segments over the pipeline's
/// life. When the input closes mid-session, cuts what is in progress.
pub(super) fn segment(mut segmenter: Box<dyn Segmenting>, input: Receiver<Input>, out: Sender<Cut>, gain: InputGain) {
    let mut next_id = 0u64;
    let mut used = 1.0;
    let mut send_all = |segs: Vec<AudioSegment>, used: f32, out: &Sender<Cut>| {
        for seg in segs {
            let _ = out.send(Cut::Segment(next_id, seg, used));
            next_id += 1;
        }
    };
    let mut dirty = false;
    for msg in &input {
        match msg {
            Input::Audio(mut samples) => {
                dirty = true;
                used = gain.apply(&mut samples);
                send_all(segmenter.accept(&samples), used, &out);
            }
            Input::EndSession => {
                dirty = false;
                send_all(segmenter.flush(), used, &out);
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
        }
    }
    if dirty {
        send_all(segmenter.flush(), used, &out);
        let _ = out.send(Cut::EndSession);
    }
}
