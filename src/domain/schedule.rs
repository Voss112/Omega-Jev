//! Prompts programmés : une date/heure, un moteur, un effort. Toujours exécutés en bypass.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::engine::{Effort, Engine};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub prompt: String,
    /// First run, in UTC.
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub daily: bool,
    pub engine: Engine,
    pub effort: Effort,
}

impl Job {
    pub fn new(prompt: String, at: DateTime<Utc>, daily: bool, engine: Engine, effort: Effort) -> Self {
        Self { id: Uuid::new_v4(), prompt, at, daily, engine, effort }
    }

    pub fn due(&self, now: DateTime<Utc>) -> bool {
        self.at <= now
    }

    /// Daily jobs move to their next occurrence; one-shot jobs are removed by the caller.
    pub fn advance(&mut self, now: DateTime<Utc>) {
        while self.at <= now {
            self.at += Duration::days(1);
        }
    }

    /// `24/10 08:30 · chaque jour · Claude · effort Max`
    pub fn label(&self) -> String {
        let when = self.at.with_timezone(&Local).format("%d/%m %H:%M");
        let repeat = if self.daily { "chaque jour" } else { "une fois" };
        format!("{when} · {repeat} · {} · effort {}", self.engine.label(), self.effort.label())
    }
}

/// `"2026-10-24"` + `"08:30"` in local time → UTC instant.
pub fn parse_when(date: &str, time: &str) -> Result<DateTime<Utc>, String> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").map_err(|_| "date attendue : AAAA-MM-JJ".to_owned())?;
    let time = NaiveTime::parse_from_str(time.trim(), "%H:%M").map_err(|_| "heure attendue : HH:MM".to_owned())?;
    let naive = date.and_time(time);
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
        .ok_or_else(|| "heure locale inexistante (changement d'heure)".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    #[test]
    fn a_job_fires_once_its_instant_is_reached() {
        let job = Job::new("audit".into(), at(24, 8), false, Engine::Claude, Effort::Max);
        assert!(!job.due(at(24, 7)));
        assert!(job.due(at(24, 8)));
        assert!(job.due(at(25, 1)));
    }

    #[test]
    fn a_daily_job_moves_to_the_next_day() {
        let mut job = Job::new("veille".into(), at(24, 8), true, Engine::Gemini, Effort::Medium);
        job.advance(at(24, 9));
        assert_eq!(job.at, at(25, 8));
        job.advance(at(27, 12));
        assert_eq!(job.at, at(28, 8));
    }

    #[test]
    fn when_parsing_rejects_garbage() {
        assert!(parse_when("24/10/2026", "08:30").is_err());
        assert!(parse_when("2026-10-24", "8h30").is_err());
        assert!(parse_when("2026-10-24", "08:30").is_ok());
    }

    #[test]
    fn label_reads_like_a_sentence() {
        let job = Job::new("x".into(), at(24, 8), true, Engine::Claude, Effort::Max);
        let label = job.label();
        assert!(label.contains("chaque jour") && label.contains("Claude") && label.contains("effort Max"), "{label}");
    }
}
