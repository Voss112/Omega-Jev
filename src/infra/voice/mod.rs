//! Local speech-to-text: microphone capture, audio maths, Whisper model and transcription. Nothing leaves the machine
//! except the one-time model download.

pub mod audio;
pub mod mic;
pub mod model;
#[cfg(test)]
pub mod testkit;
pub mod whisper;
