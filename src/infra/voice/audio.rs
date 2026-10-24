//! Audio maths for dictation, pure and testable: mono mix, resampling to Whisper's 16 kHz, phrase detection.

/// Whisper's input rate.
pub const RATE: u32 = 16_000;
/// RMS analysis frame.
const FRAME_MS: usize = 20;
/// Speech needed before a pause can close a phrase.
const MIN_SPEECH_MS: usize = 300;
/// Silence that closes a phrase.
const PAUSE_MS: usize = 600;
/// Silence kept after the last word (soft consonants fade below the threshold).
const TAIL_MS: usize = 200;
/// A phrase longer than this is cut at its quietest moment (Whisper reads 30 s windows).
const MAX_PHRASE_MS: usize = 20_000;
/// Quietest speech threshold whatever the noise floor (normalised samples).
const MIN_THRESHOLD: f32 = 0.004;

/// Interleaved frames → mono average.
pub fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
}

/// `rate` → 16 kHz: windowed-sinc low-pass (Blackman, cut at 0.45 × 16 kHz) evaluated only where output samples fall,
/// then linear interpolation between the two filtered neighbours.
pub fn resample(pcm: &[f32], rate: u32) -> Vec<f32> {
    if rate == RATE || pcm.is_empty() {
        return pcm.to_vec();
    }
    let step = f64::from(rate) / f64::from(RATE);
    let taps = if step > 1.0 { lowpass(0.45 / step, ((20.0 * step) as usize) | 1) } else { vec![1.0] };
    let filtered = |k: usize| -> f32 {
        let half = taps.len() / 2;
        taps.iter()
            .enumerate()
            .filter_map(|(j, &t)| (k + j).checked_sub(half).and_then(|i| pcm.get(i)).map(|&s| s * t))
            .sum()
    };
    let len = (pcm.len() as f64 / step).floor() as usize;
    (0..len)
        .map(|i| {
            let x = i as f64 * step;
            let k = x.floor() as usize;
            let frac = (x - k as f64) as f32;
            let a = filtered(k);
            if frac == 0.0 { a } else { a + (filtered((k + 1).min(pcm.len() - 1)) - a) * frac }
        })
        .collect()
}

/// FIR low-pass taps (`cutoff` in cycles per input sample), unit DC gain.
fn lowpass(cutoff: f64, n: usize) -> Vec<f32> {
    use std::f64::consts::PI;
    let m = (n - 1) as f64;
    let raw: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m / 2.0;
            let sinc = if x == 0.0 { 2.0 * cutoff } else { (2.0 * PI * cutoff * x).sin() / (PI * x) };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / m).cos() + 0.08 * (4.0 * PI * i as f64 / m).cos();
            sinc * w
        })
        .collect();
    let gain: f64 = raw.iter().sum();
    raw.iter().map(|t| (t / gain) as f32).collect()
}

/// Speech/silence decisions with a noise floor learnt over the whole dictation.
pub struct Vad {
    rate: usize,
    floor: f32,
}

impl Vad {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as usize, floor: f32::MAX }
    }

    fn frame(&self) -> usize {
        (self.rate * FRAME_MS / 1000).max(1)
    }

    fn ms(&self, ms: usize) -> usize {
        ms / FRAME_MS
    }

    fn levels(&self, pcm: &[f32]) -> Vec<f32> {
        pcm.chunks(self.frame()).map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt()).collect()
    }

    /// Floor = quietest fifth of the frames heard, rising slowly so a noisier room is followed.
    fn learn(&mut self, levels: &[f32]) {
        if levels.len() < 10 {
            return;
        }
        let mut sorted = levels.to_vec();
        sorted.sort_by(f32::total_cmp);
        let quiet = sorted[sorted.len() / 5];
        self.floor = if quiet < self.floor { quiet } else { (self.floor * 1.05).min(quiet) };
    }

    fn threshold(&self) -> f32 {
        if self.floor == f32::MAX { MIN_THRESHOLD * 2.0 } else { (self.floor * 3.0).max(MIN_THRESHOLD) }
    }

    /// Sample index closing the first finished phrase of `pcm` (speech, then a pause), or a forced cut when it is
    /// too long. `None`: keep listening.
    pub fn phrase_end(&mut self, pcm: &[f32]) -> Option<usize> {
        let levels = self.levels(pcm);
        self.learn(&levels);
        let threshold = self.threshold();
        let frame = self.frame();
        let mut speech = 0;
        let mut last_voiced = None;
        for (i, &level) in levels.iter().enumerate() {
            if level > threshold {
                speech += 1;
                last_voiced = Some(i);
            } else if let Some(last) = last_voiced
                && speech >= self.ms(MIN_SPEECH_MS)
                && i - last >= self.ms(PAUSE_MS)
            {
                return Some(((last + 1 + self.ms(TAIL_MS)) * frame).min(pcm.len()));
            }
        }
        if levels.len() >= self.ms(MAX_PHRASE_MS) {
            // Quietest frame of the second half: late enough to keep long phrases whole, unlikely to split a word.
            let from = levels.len() / 2;
            let quiet = (from..levels.len()).min_by(|&a, &b| levels[a].total_cmp(&levels[b])).unwrap_or(levels.len() - 1);
            return Some(((quiet + 1) * frame).min(pcm.len()));
        }
        None
    }

    /// Enough speech in `pcm` to be worth transcribing (silence makes Whisper invent sentences).
    pub fn has_speech(&mut self, pcm: &[f32]) -> bool {
        let levels = self.levels(pcm);
        self.learn(&levels);
        let threshold = self.threshold();
        levels.iter().filter(|&&l| l > threshold).count() >= self.ms(MIN_SPEECH_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: u32 = 48_000;

    fn tone(ms: usize, amp: f32) -> Vec<f32> {
        (0..DEVICE as usize * ms / 1000).map(|i| amp * (i as f32 * 0.07).sin()).collect()
    }

    fn noise(ms: usize) -> Vec<f32> {
        (0..DEVICE as usize * ms / 1000).map(|i| 0.001 * ((i * 7919 % 101) as f32 / 50.0 - 1.0)).collect()
    }

    #[test]
    fn stereo_is_averaged_and_48k_becomes_16k() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        let pcm = tone(1000, 0.5);
        let out = resample(&pcm, DEVICE);
        assert_eq!(out.len(), 16_000);
        assert_eq!(resample(&out, RATE), out, "déjà en 16 kHz");
    }

    #[test]
    fn a_tone_above_8_khz_is_filtered_out() {
        // 12 kHz at 48 kHz folds onto 4 kHz without a low-pass; a 1 kHz voice band passes untouched.
        let wave = |hz: f32| -> Vec<f32> { (0..48_000).map(|i| (i as f32 * std::f32::consts::TAU * hz / 48_000.0).sin()).collect() };
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        let (high, voice) = (rms(&resample(&wave(12_000.0), DEVICE)), rms(&resample(&wave(1_000.0), DEVICE)));
        assert!(high < 0.01, "12 kHz : {high}");
        assert!((voice - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "1 kHz : {voice}");
        assert_eq!(resample(&wave(1_000.0), 44_100).len(), 17_414, "44,1 kHz aussi");
    }

    #[test]
    fn a_pause_after_speech_closes_the_phrase() {
        let mut vad = Vad::new(DEVICE);
        let speech = [noise(500), tone(1200, 0.2)].concat();
        let mut pcm = [speech.clone(), noise(300)].concat();
        assert_eq!(vad.phrase_end(&pcm), None, "pause trop courte");
        pcm.extend(noise(500));
        let cut = vad.phrase_end(&pcm).expect("phrase terminée");
        let tail = DEVICE as usize * TAIL_MS / 1000;
        assert!(cut >= speech.len() && cut <= speech.len() + tail + 2 * 960, "{cut} vs {}", speech.len());
    }

    #[test]
    fn noise_or_a_click_is_not_a_phrase() {
        let mut vad = Vad::new(DEVICE);
        let pcm = [noise(1000), tone(100, 0.3), noise(1500)].concat();
        assert_eq!(vad.phrase_end(&pcm), None);
        assert!(!vad.has_speech(&pcm));
        assert!(vad.has_speech(&[noise(500), tone(800, 0.1), noise(200)].concat()));
    }

    #[test]
    fn a_long_monologue_is_cut_at_its_quietest_point() {
        let mut vad = Vad::new(DEVICE);
        let mut pcm = [noise(500), tone(15_000, 0.2), tone(200, 0.01), tone(6_000, 0.2)].concat();
        pcm.truncate(DEVICE as usize * 21);
        let cut = vad.phrase_end(&pcm).expect("coupure forcée");
        let dip = DEVICE as usize * 15_500 / 1000;
        assert!(cut.abs_diff(dip) < DEVICE as usize / 4, "{cut} vs {dip}");
    }
}
