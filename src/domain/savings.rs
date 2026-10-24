//! Token-economy ledger: what the providers billed (exact) next to what each lever kept off the wire.
//! Lever sizes are measured in bytes on the real request, then converted with the tokens/byte ratio
//! calibrated on this conversation's own provider-reported prompt sizes.

use serde::{Deserialize, Serialize};

use super::engine::Engine;

/// Tokens/byte used until a first image-free request has been measured (~3.7 bytes per token).
const DEFAULT_RATIO: f64 = 0.27;

/// Input price multipliers relative to an uncached input token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    pub read: f64,
    pub write: f64,
}

impl Rates {
    /// Claude: published cache prices (5-minute TTL writes). DeepSeek: automatic disk cache, hits at ~1/10, no write premium.
    pub fn of(engine: Engine, model: &str) -> Self {
        match engine {
            Engine::Claude => {
                let read = if model.contains("fable-5-1") || model.contains("mythos-5-1") {
                    0.025
                } else if model.contains("opus-5-5") {
                    0.05
                } else {
                    0.1
                };
                Self { read, write: 1.25 }
            }
            Engine::DeepSeek => Self { read: 0.1, write: 1.0 },
            // Gemini implicit caching: hits at 1/4 of the input price, no write premium.
            Engine::Gemini => Self { read: 0.25, write: 1.0 },
        }
    }
}

/// Bytes kept off one request by each lever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Levers {
    /// Tool schemas Jev did not load (lazy tools).
    pub lazy: u64,
    /// Shell noise removed by the RTK compressor, counted again on every request that carries the output.
    pub rtk: u64,
    /// Playbook bodies not re-sent.
    pub playbook: u64,
    /// History replaced by the compaction summary.
    pub summary: u64,
    /// Old tool results replaced by stubs mid-turn.
    pub cleared: u64,
}

/// One request as measured: provider usage plus the levers' bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub prompt: u64,
    pub cached: u64,
    pub written: u64,
    pub completion: u64,
    /// Text bytes of the request (images and signatures excluded).
    pub bytes: u64,
    /// The request carried images: their tokens do not follow the text ratio.
    pub images: bool,
    pub levers: Levers,
    pub rates: Rates,
}

impl Default for Rates {
    fn default() -> Self {
        Self { read: 1.0, write: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    pub requests: u64,
    /// Provider-reported (exact).
    pub prompt: u64,
    pub cached: u64,
    pub written: u64,
    pub completion: u64,
    /// Calibration: provider tokens and text bytes of image-free requests.
    pub calib_tokens: u64,
    pub calib_bytes: u64,
    /// Estimated tokens kept off the wire, per lever.
    pub lazy: u64,
    pub rtk: u64,
    pub playbook: u64,
    pub summary: u64,
    pub cleared: u64,
    /// Input actually billed, in uncached-token equivalents (cache reads and writes weighted).
    pub billed: f64,
    /// Lever tokens priced at the effective rate of the request that avoided them
    /// (most would have been cache reads): what the levers really saved on the bill.
    pub levers_billed: f64,
    /// Same, for the levers Jev drives (lazy tools, playbook choice).
    pub jev_billed: f64,
}

impl Ledger {
    /// Tokens per text byte measured on this ledger's own requests.
    pub fn ratio(&self) -> f64 {
        if self.calib_bytes == 0 { DEFAULT_RATIO } else { self.calib_tokens as f64 / self.calib_bytes as f64 }
    }

    pub fn calibrated(&self) -> bool {
        self.calib_bytes > 0
    }

    pub fn record(&mut self, s: &Sample) {
        if s.prompt == 0 {
            return;
        }
        if !s.images && s.bytes > 0 {
            self.calib_tokens += s.prompt;
            self.calib_bytes += s.bytes;
        }
        let ratio = self.ratio();
        let tokens = |bytes: u64| (bytes as f64 * ratio).round() as u64;
        let fresh = s.prompt.saturating_sub(s.cached + s.written) as f64;
        let billed = fresh + s.cached as f64 * s.rates.read + s.written as f64 * s.rates.write;
        let rate = billed / s.prompt as f64;
        let l = &s.levers;
        self.levers_billed += tokens(l.lazy + l.rtk + l.playbook + l.summary + l.cleared) as f64 * rate;
        self.jev_billed += tokens(l.lazy + l.playbook) as f64 * rate;
        self.requests += 1;
        self.prompt += s.prompt;
        self.cached += s.cached;
        self.written += s.written;
        self.completion += s.completion;
        self.lazy += tokens(s.levers.lazy);
        self.rtk += tokens(s.levers.rtk);
        self.playbook += tokens(s.levers.playbook);
        self.summary += tokens(s.levers.summary);
        self.cleared += tokens(s.levers.cleared);
        self.billed += billed;
    }

    /// Tokens the levers kept off the wire.
    pub fn levers(&self) -> u64 {
        self.lazy + self.rtk + self.playbook + self.summary + self.cleared
    }

    /// Input without any Omega lever and without cache, every token at full price.
    pub fn baseline(&self) -> f64 {
        (self.prompt + self.levers()) as f64
    }

    /// Input bill with the prompt cache but without Omega's levers.
    pub fn without_levers(&self) -> f64 {
        self.billed + self.levers_billed
    }

    /// Billed input saved by the prompt cache (reads discount minus the write premium).
    pub fn cache_saved(&self) -> f64 {
        self.prompt as f64 - self.billed
    }

    /// Share of the baseline input bill avoided, in [0, 1].
    pub fn saved_share(&self) -> f64 {
        let base = self.baseline();
        if base == 0.0 { 0.0 } else { (1.0 - self.billed / base).clamp(0.0, 1.0) }
    }
}

impl std::ops::AddAssign for Ledger {
    fn add_assign(&mut self, o: Self) {
        self.requests += o.requests;
        self.prompt += o.prompt;
        self.cached += o.cached;
        self.written += o.written;
        self.completion += o.completion;
        self.calib_tokens += o.calib_tokens;
        self.calib_bytes += o.calib_bytes;
        self.lazy += o.lazy;
        self.rtk += o.rtk;
        self.playbook += o.playbook;
        self.summary += o.summary;
        self.cleared += o.cleared;
        self.billed += o.billed;
        self.levers_billed += o.levers_billed;
        self.jev_billed += o.jev_billed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_request_is_billed_at_the_read_rate_and_levers_use_the_measured_ratio() {
        let mut l = Ledger::default();
        let rates = Rates::of(Engine::Claude, "claude-opus-5-5");
        let levers = Levers { lazy: 4000, ..Levers::default() };
        l.record(&Sample { prompt: 10_000, cached: 8_000, written: 1_000, bytes: 40_000, levers, rates, ..Sample::default() });
        assert!((l.ratio() - 0.25).abs() < 1e-9);
        assert_eq!(l.lazy, 1000, "4000 octets × 0,25 token/octet");
        // 1000 frais + 8000 × 0,05 + 1000 × 1,25
        assert!((l.billed - 2650.0).abs() < 1e-6, "{}", l.billed);
        assert!((l.cache_saved() - 7350.0).abs() < 1e-6);
        assert!((l.levers_billed - 265.0).abs() < 1e-6, "1000 tokens évités au tarif effectif 0,265");
        assert!(l.saved_share() > 0.7);
    }

    #[test]
    fn image_requests_do_not_skew_the_calibration() {
        let mut l = Ledger::default();
        l.record(&Sample { prompt: 1000, bytes: 4000, rates: Rates::default(), ..Sample::default() });
        l.record(&Sample { prompt: 5000, bytes: 4000, images: true, rates: Rates::default(), ..Sample::default() });
        assert!((l.ratio() - 0.25).abs() < 1e-9);
        assert_eq!(l.prompt, 6000);
    }
}
