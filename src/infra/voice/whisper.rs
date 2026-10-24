//! Speech → text with whisper.cpp: one loaded model and one decoding state reused for every phrase.

use std::path::Path;

use anyhow::{Result, anyhow};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use super::audio::{RATE, Vad};

/// whisper.cpp ignores input shorter than one second.
const MIN_SAMPLES: usize = RATE as usize * 11 / 10;
/// A segment Whisper itself rates as probably silent is dropped.
const NO_SPEECH: f32 = 0.6;
/// What Whisper writes over silence or music (subtitle credits learnt from its training data).
const ARTIFACTS: [&str; 8] = [
    "sous-titres réalisés par",
    "sous-titrage",
    "amara.org",
    "merci d'avoir regardé",
    "thanks for watching",
    "thank you for watching",
    "[musique]",
    "(musique)",
];

pub struct Transcriber {
    /// Keeps the model alive; compute buffers allocated once instead of per phrase.
    state: WhisperState,
    threads: i32,
}

impl Transcriber {
    /// Loads the model on the GPU (Vulkan; CPU if no device) and warms it up: the first pass compiles the GPU
    /// pipelines (1.5 s on an RX 6950 XT), the next ones take 0.2 s.
    pub fn load(model: &Path) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
            .map_err(|e| anyhow!("modèle Whisper illisible : {e}"))?;
        let state = ctx.create_state().map_err(|e| anyhow!("Whisper : {e}"))?;
        // Physical cores for the CPU fallback: hyper-threads slow ggml's matrix kernels down.
        let threads = std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(1, 8));
        let mut me = Self { state, threads: i32::try_from(threads).unwrap_or(4) };
        me.run(&[0.0; MIN_SAMPLES], "en", "", SamplingStrategy::Greedy { best_of: 1 })?;
        Ok(me)
    }

    /// 16 kHz mono → text. `language`: ISO 639-1 code or `auto`. `prompt`: what precedes (spelling, punctuation
    /// style, continuity across phrases). Audio without speech gives "" (Whisper would invent "you", credits…).
    pub fn transcribe(&mut self, pcm: &[f32], language: &str, prompt: &str) -> Result<String> {
        if !Vad::new(RATE).has_speech(pcm) {
            return Ok(String::new());
        }
        // Beam search: more robust on hard words, a few ms on the GPU.
        self.run(pcm, language, prompt, SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 })
    }

    /// Always the full 30 s encoder window: a window cut to the phrase (`audio_ctx`) is faster on CPU but makes the
    /// decoder repeat short phrases ("Now commit the changes." ×3 at 172 frames, ×2 at 384).
    fn run(&mut self, pcm: &[f32], language: &str, prompt: &str, strategy: SamplingStrategy) -> Result<String> {
        let mut padded;
        let audio = if pcm.len() < MIN_SAMPLES {
            padded = pcm.to_vec();
            padded.resize(MIN_SAMPLES, 0.0);
            &padded
        } else {
            pcm
        };
        let mut params = FullParams::new(strategy);
        params.set_n_threads(self.threads);
        params.set_language(Some(language));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if !prompt.is_empty() {
            params.set_initial_prompt(prompt);
        }
        self.state.full(params, audio).map_err(|e| anyhow!("Whisper : {e}"))?;
        let text: Vec<String> = self
            .state
            .as_iter()
            .filter(|s| s.no_speech_probability() < NO_SPEECH)
            .filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_owned()))
            .filter(|t| !t.is_empty() && !artifact(t))
            .collect();
        Ok(text.join(" "))
    }
}

fn artifact(text: &str) -> bool {
    let lower = text.to_lowercase();
    ARTIFACTS.iter().any(|a| lower.contains(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_credits_are_recognised_as_artifacts() {
        assert!(artifact("Sous-titres réalisés par la communauté d'Amara.org"));
        assert!(artifact("Thank you for watching!"));
        assert!(!artifact("Corrige le bug dans le parseur."));
    }

    use super::super::testkit::{speak, wer};

    /// Real model (downloaded and SHA-256 checked by `model::ensure`), production settings.
    #[test]
    #[ignore = "télécharge 574 Mo puis transcrit : cargo test --release -- --ignored voice_live --nocapture"]
    fn voice_live() {
        use std::time::Instant;

        use super::super::{audio, model};
        const LONG: &str = "Omega, please open the parser module and fix the bug on line forty two. \
            Then run the tests, check the warnings, and build a new installer for Windows.";
        const SHORT: &str = "Now commit the changes.";
        let rt = tokio::runtime::Runtime::new().unwrap();
        let http = crate::infra::http::client("").unwrap();
        let t = Instant::now();
        let fresh = !model::installed(&model::TURBO);
        let path = rt.block_on(model::ensure(&http, &model::TURBO, |_, _| {})).unwrap();
        println!("MESURE modèle : {} ({})", path.display(), if fresh { format!("téléchargé en {:.1} s", t.elapsed().as_secs_f64()) } else { "déjà là".into() });

        let t = Instant::now();
        let mut w = Transcriber::load(&path).unwrap();
        println!("MESURE chargement + chauffe GPU : {:.2} s", t.elapsed().as_secs_f64());
        for said in [LONG, SHORT] {
            let (rate, pcm) = speak(said);
            let pcm16 = audio::resample(&pcm, rate);
            let t = Instant::now();
            let text = w.transcribe(&pcm16, "en", "").unwrap();
            let err = wer(said, &text);
            println!(
                "MESURE {:.1} s de parole → {:.2} s · WER {:.1} % · « {text} »",
                pcm16.len() as f64 / f64::from(RATE),
                t.elapsed().as_secs_f64(),
                err * 100.0
            );
            assert!(err < 0.1, "WER {err}");
        }
        let silence = vec![0.0f32; RATE as usize * 2];
        let t = Instant::now();
        assert_eq!(w.transcribe(&silence, "en", "").unwrap(), "", "silence → rien (Whisper seul écrit « you »)");
        println!("MESURE silence → « » en {:.3} s", t.elapsed().as_secs_f64());
    }
}
