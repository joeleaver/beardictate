//! Nemotron fed live from the segmenter, against the real models. Skips
//! unless `BD_MODELS_DIR` is set and holds the Nemotron model.

mod common;

use bd_asr::{NemotronAsr, NemotronConfig, Segmenter, SegmenterConfig};
use bd_core::{LiveAudio, StreamingAsr};
use common::*;

const NEMOTRON: &str = "asr-nemotron-speech-streaming-en-0.6b-560ms-int8";

#[test]
fn words_arrive_while_speaking_and_each_utterance_ends_whole() {
    let Some(models) = models_dir() else { return };
    let dir = models.join(NEMOTRON);
    if !dir.is_dir() {
        return;
    }
    let en = read_wav_16k(&parakeet_dir(&models).join("test_wavs/en.wav"));
    let mut audio = silence(0.5);
    audio.extend(&en);
    audio.extend(silence(1.5));
    audio.extend(&en);
    audio.extend(silence(1.0));

    let mut seg = Segmenter::new(&vad_model(&models), SegmenterConfig::default()).unwrap();
    let mut asr = NemotronAsr::new(&dir, &NemotronConfig::default()).unwrap();
    let (mut partials, mut finals) = (Vec::new(), Vec::new());
    let mut handle = |ev: LiveAudio, asr: &mut NemotronAsr| match ev {
        LiveAudio::Start(a) => {
            asr.start().unwrap();
            partials.extend(asr.feed(&a).unwrap());
        }
        LiveAudio::More(a) => partials.extend(asr.feed(&a).unwrap()),
        LiveAudio::End(_) => finals.push(asr.finish().unwrap()),
        LiveAudio::Drop => {
            asr.finish().unwrap();
        }
    };
    for chunk in audio.chunks(1600) {
        for ev in seg.accept_live(chunk) {
            handle(ev, &mut asr);
        }
    }
    for ev in seg.flush_live() {
        handle(ev, &mut asr);
    }
    assert_eq!(finals.len(), 2, "{finals:?}");
    for f in &finals {
        let words = f.to_lowercase();
        assert!(words.contains("ask not what your country can do for you"), "{f:?}");
    }
    assert!(partials.len() >= 4, "text should grow while speaking: {partials:?}");
    assert!(partials.iter().any(|p| p.split_whitespace().count() < 8), "{partials:?}");
}
