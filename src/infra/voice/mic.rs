//! Default microphone (WASAPI on Windows): mono samples at the device rate, appended to a shared buffer.

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU32, Ordering},
};

use anyhow::{Context as _, Result, bail};
use cpal::{
    FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use super::audio;

/// Shared between the audio callback and the reader.
#[derive(Default)]
struct Shared {
    samples: Mutex<Vec<f32>>,
    error: Mutex<Option<String>>,
}

pub struct Mic {
    /// Recording stops when the stream is dropped.
    _stream: Stream,
    pub rate: u32,
    shared: Arc<Shared>,
}

impl Mic {
    /// Opens the default input device. `level` receives the RMS of each callback (f32 bits) for the UI meter.
    pub fn open(level: Arc<AtomicU32>) -> Result<Self> {
        let device =
            cpal::default_host().default_input_device().context("aucun micro détecté (Paramètres Windows → Son → Entrée)")?;
        let supported = device.default_input_config().context("micro inutilisable (accès refusé ou périphérique occupé)")?;
        let channels = usize::from(supported.channels()).max(1);
        let rate = supported.sample_rate();
        let config = supported.config();
        let shared = Arc::new(Shared::default());
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, channels, &shared, level),
            SampleFormat::I16 => build::<i16>(&device, config, channels, &shared, level),
            SampleFormat::I32 => build::<i32>(&device, config, channels, &shared, level),
            SampleFormat::U16 => build::<u16>(&device, config, channels, &shared, level),
            other => bail!("format de micro non géré : {other:?}"),
        }?;
        stream.play().context("démarrage du micro")?;
        Ok(Self { _stream: stream, rate, shared })
    }

    /// Mono samples recorded and not consumed yet.
    pub fn pending(&self) -> Vec<f32> {
        self.shared.samples.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Drops the first `n` pending samples (transcribed): memory stays bounded by the phrase being spoken.
    pub fn consume(&self, n: usize) {
        let mut samples = self.shared.samples.lock().unwrap_or_else(PoisonError::into_inner);
        let n = n.min(samples.len());
        samples.drain(..n);
    }

    /// The device failed mid-recording (unplugged…).
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn build<T>(device: &cpal::Device, config: StreamConfig, channels: usize, shared: &Arc<Shared>, level: Arc<AtomicU32>) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let (data, failed) = (Arc::clone(shared), Arc::clone(shared));
    device
        .build_input_stream::<T, _, _>(
            config,
            move |input: &[T], _| {
                let samples: Vec<f32> = input.iter().map(|s| s.to_sample::<f32>()).collect();
                let mono = audio::to_mono(&samples, channels);
                if !mono.is_empty() {
                    let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
                    level.store(rms.to_bits(), Ordering::Relaxed);
                }
                data.samples.lock().unwrap_or_else(PoisonError::into_inner).extend(mono);
            },
            move |e: cpal::Error| {
                if fatal(e.kind()) {
                    *failed.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.to_string());
                }
            },
            None,
        )
        .context("ouverture du micro")
}

/// A glitch (WASAPI data discontinuity under load), a reroute or a refused real-time priority leaves the stream
/// running: only the loss of the device or of the stream ends the dictation.
fn fatal(kind: cpal::ErrorKind) -> bool {
    !matches!(kind, cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_buffer_glitch_does_not_stop_the_dictation() {
        assert!(!super::fatal(cpal::ErrorKind::Xrun));
        assert!(!super::fatal(cpal::ErrorKind::DeviceChanged));
        assert!(super::fatal(cpal::ErrorKind::DeviceNotAvailable));
        assert!(super::fatal(cpal::ErrorKind::StreamInvalidated));
    }
}
