//! Dictation session: microphone → phrases (pause detection) → Whisper on the GPU → text, phrase by phrase while the
//! user keeps speaking. Runs on its own thread (model and audio calls block); the UI polls `updates`.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use reqwest::Client;
use tokio::runtime::Handle;

use crate::infra::voice::{
    audio::{self, Vad},
    mic::Mic,
    model,
    whisper::Transcriber,
};

/// Microphone polling period: a phrase is sent to Whisper at most this long after its closing pause.
const TICK: Duration = Duration::from_millis(40);
/// Characters of preceding text given to Whisper as context (spelling, punctuation, continuity).
const PROMPT_CHARS: usize = 300;

#[derive(Debug, PartialEq)]
pub enum Update {
    /// First use: the model is being downloaded (bytes).
    Download { done: u64, total: u64 },
    /// The microphone is open: speech from now on is captured.
    Listening,
    /// A transcribed phrase, to append to the draft.
    Text(String),
    Failed(String),
    /// Stopped and everything said was transcribed.
    Finished,
}

pub struct Dictation {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    pub updates: mpsc::Receiver<Update>,
}

impl Dictation {
    /// Starts listening. `context`: text already in the draft (continuity for the first phrase).
    pub fn start(rt: Handle, http: Client, language: String, context: String, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, updates) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let session = Session { stop: Arc::clone(&stop), level: Arc::clone(&level), language, prompt: tail(&context) };
        thread::spawn(move || {
            let emit = |u: Update| {
                let _ = tx.send(u);
                wake();
            };
            if let Err(e) = session.run(&rt, &http, &emit) {
                emit(Update::Failed(format!("{e:#}")));
            }
            emit(Update::Finished);
        });
        Self { stop, level, updates }
    }

    /// Stops recording; what was said so far is still transcribed, then `Finished` arrives.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Current microphone loudness, 0..=1 (for the meter).
    pub fn level(&self) -> f32 {
        (f32::from_bits(self.level.load(Ordering::Relaxed)) * 8.0).sqrt().min(1.0)
    }
}

/// Recorded audio, growing while the session runs (the microphone, or a recording in tests).
trait Source {
    fn rate(&self) -> u32;
    /// Mono samples not consumed yet.
    fn pending(&self) -> Vec<f32>;
    /// Drops the first `n` pending samples once they are transcribed.
    fn consume(&self, n: usize);
    fn error(&self) -> Option<String>;
}

impl Source for Mic {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn pending(&self) -> Vec<f32> {
        Mic::pending(self)
    }

    fn consume(&self, n: usize) {
        Mic::consume(self, n);
    }

    fn error(&self) -> Option<String> {
        Mic::error(self)
    }
}

struct Session {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    language: String,
    prompt: String,
}

impl Session {
    fn run(mut self, rt: &Handle, http: &Client, emit: &dyn Fn(Update)) -> Result<()> {
        let path = self.model(rt, http, emit)?;
        if self.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Microphone first: speech during the model load (0.5 s) is buffered, not lost.
        let mic = Mic::open(Arc::clone(&self.level))?;
        emit(Update::Listening);
        let mut whisper = Transcriber::load(&path)?;
        self.listen(&mic, &mut whisper, emit)
    }

    /// Transcribes each phrase as soon as its closing pause is heard; on stop, whatever remains.
    fn listen(&mut self, source: &impl Source, whisper: &mut Transcriber, emit: &dyn Fn(Update)) -> Result<()> {
        let rate = source.rate();
        let mut vad = Vad::new(rate);
        while !self.stop.load(Ordering::Relaxed) {
            thread::sleep(TICK);
            if let Some(e) = source.error() {
                anyhow::bail!("micro perdu : {e}");
            }
            let pending = source.pending();
            if let Some(end) = vad.phrase_end(&pending) {
                self.transcribe(whisper, &pending[..end], rate, emit)?;
                source.consume(end);
            }
        }
        self.transcribe(whisper, &source.pending(), rate, emit)
    }

    /// The model path, downloaded (and hash-checked) on first use.
    fn model(&self, rt: &Handle, http: &Client, emit: &dyn Fn(Update)) -> Result<PathBuf> {
        let mut last = 0;
        rt.block_on(model::ensure(http, &model::TURBO, |done, total| {
            // One update per percent: the UI does not need 50 000 messages.
            let pct = done * 100 / total.max(1);
            if pct != last {
                last = pct;
                emit(Update::Download { done, total });
            }
        }))
    }

    fn transcribe(&mut self, whisper: &mut Transcriber, pcm: &[f32], rate: u32, emit: &dyn Fn(Update)) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let text = whisper.transcribe(&audio::resample(pcm, rate), &self.language, &self.prompt)?;
        if !text.is_empty() {
            self.prompt = tail(&format!("{} {text}", self.prompt));
            emit(Update::Text(text));
        }
        Ok(())
    }
}

/// Last `PROMPT_CHARS` characters, cut on a char boundary.
fn tail(text: &str) -> String {
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(PROMPT_CHARS);
    text.chars().skip(skip).collect()
}

/// `text` appended to `draft` with exactly one separating space (none at the start or after a new line).
pub fn append(draft: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
        draft.push(' ');
    }
    draft.push_str(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_are_appended_with_one_space() {
        let mut draft = String::new();
        append(&mut draft, " Bonjour. ");
        append(&mut draft, "Corrige le bug.");
        assert_eq!(draft, "Bonjour. Corrige le bug.");
        draft.push('\n');
        append(&mut draft, "Merci");
        append(&mut draft, "  ");
        assert_eq!(draft, "Bonjour. Corrige le bug.\nMerci");
    }

    #[test]
    fn the_prompt_keeps_the_end_of_the_text() {
        let long = "é".repeat(PROMPT_CHARS + 10);
        assert_eq!(tail(&long).chars().count(), PROMPT_CHARS);
        assert_eq!(tail("  court "), "court");
    }

    /// A finished recording played into the session as if it came from the microphone.
    struct Recording {
        rate: u32,
        pcm: std::sync::Mutex<Vec<f32>>,
    }

    impl Source for Recording {
        fn rate(&self) -> u32 {
            self.rate
        }

        fn pending(&self) -> Vec<f32> {
            self.pcm.lock().unwrap().clone()
        }

        fn consume(&self, n: usize) {
            let mut pcm = self.pcm.lock().unwrap();
            let n = n.min(pcm.len());
            pcm.drain(..n);
        }

        fn error(&self) -> Option<String> {
            None
        }
    }

    /// Real pipeline (pause detection → Whisper on the GPU → updates) on two sentences separated by a 1 s pause:
    /// the first is written while "the user" is still there, the second when dictation stops.
    #[test]
    #[ignore = "modèle réel : cargo test --release -- --ignored dictation_live --nocapture"]
    fn dictation_live() {
        use std::time::Instant;

        use crate::infra::voice::testkit::{speak, wer};
        const FIRST: &str = "Open the settings file and add a new option.";
        const SECOND: &str = "Then write a test for it.";
        let rt = tokio::runtime::Runtime::new().unwrap();
        let path = rt.block_on(model::ensure(&crate::infra::http::client("").unwrap(), &model::TURBO, |_, _| {})).unwrap();
        let (rate, first) = speak(FIRST);
        let (_, second) = speak(SECOND);
        let pcm = [first, vec![0.0; rate as usize], second].concat();
        let mut whisper = Transcriber::load(&path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(2));
                stop.store(true, Ordering::Relaxed);
            })
        };
        let mut session =
            Session { stop, level: Arc::new(AtomicU32::new(0)), language: "en".into(), prompt: String::new() };
        let (tx, rx) = mpsc::channel();
        let t = Instant::now();
        let recording = Recording { rate, pcm: std::sync::Mutex::new(pcm) };
        session.listen(&recording, &mut whisper, &|u| tx.send((t.elapsed(), u)).unwrap()).unwrap();
        stopper.join().unwrap();
        let texts: Vec<(Duration, String)> = rx
            .try_iter()
            .filter_map(|(at, u)| if let Update::Text(text) = u { Some((at, text)) } else { None })
            .collect();
        for (at, text) in &texts {
            println!("MESURE {:.2} s → « {text} »", at.as_secs_f64());
        }
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].0 < Duration::from_secs(2), "1re phrase écrite pendant la dictée");
        assert!(wer(FIRST, &texts[0].1) < 0.1 && wer(SECOND, &texts[1].1) < 0.1, "{texts:?}");
    }
}
