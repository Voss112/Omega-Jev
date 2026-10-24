//! Test helpers: real speech synthesised by Windows, as a microphone would deliver it.

use std::path::Path;

/// 16-bit PCM WAV → (rate, mono samples).
pub fn read_wav(path: &Path) -> (u32, Vec<f32>) {
    let bytes = std::fs::read(path).unwrap();
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (mut i, mut rate, mut channels, mut data) = (12, 0, 1, Vec::new());
    while i + 8 <= bytes.len() {
        let (id, len) = (&bytes[i..i + 4], u32_at(i + 4) as usize);
        match id {
            b"fmt " => (channels, rate) = (usize::from(u16_at(i + 10)), u32_at(i + 12)),
            b"data" => {
                let end = (i + 8 + len).min(bytes.len());
                data = bytes[i + 8..end].as_chunks::<2>().0.iter().map(|&s| f32::from(i16::from_le_bytes(s)) / 32768.0).collect();
            }
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    (rate, super::audio::to_mono(&data, channels))
}

/// `text` spoken by Windows (Zira, en-US) at 48 kHz mono.
pub fn speak(text: &str) -> (u32, Vec<f32>) {
    let wav = std::env::temp_dir().join(format!("omega-voice-{}-{}.wav", std::process::id(), text.len()));
    let script = format!(
        "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $s.SelectVoice('Microsoft Zira Desktop'); \
         $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(48000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono); \
         $s.SetOutputToWaveFile('{}', $f); $s.Speak('{text}'); $s.Dispose()",
        wav.display()
    );
    assert!(std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status().unwrap().success());
    let out = read_wav(&wav);
    let _ = std::fs::remove_file(&wav);
    out
}

/// Word error rate of `got` against `want` (case and punctuation ignored).
pub fn wer(want: &str, got: &str) -> f64 {
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(str::to_owned).collect()
    };
    let (a, b) = (words(want), words(got));
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, wa) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, wb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(wa != wb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()] as f64 / a.len().max(1) as f64
}
