//! Prompts programmés : une date/heure, un moteur, un effort. Toujours exécutés en bypass.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    engine::{Effort, Engine},
    models,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub prompt: String,
    /// First run, in UTC.
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub daily: bool,
    pub engine: Engine,
    /// Exact model id chosen at scheduling time; empty means "whatever the settings say when it fires".
    #[serde(default)]
    pub model: String,
    pub effort: Effort,
    /// Kept but not fired until resumed.
    #[serde(default)]
    pub paused: bool,
    /// One-shot job that already ran (kept in the Tasks tab with its conversation).
    #[serde(default)]
    pub done: bool,
    /// Latest runs, newest last (at most `RUNS_KEPT`).
    #[serde(default)]
    pub runs: Vec<Run>,
}

/// One execution of a job: when, and the conversation it opened.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub at: DateTime<Utc>,
    pub conv: Uuid,
}

const RUNS_KEPT: usize = 10;

impl Job {
    pub fn new(prompt: String, at: DateTime<Utc>, daily: bool, engine: Engine, model: String, effort: Effort) -> Self {
        Self { id: Uuid::new_v4(), prompt, at, daily, engine, model, effort, paused: false, done: false, runs: Vec::new() }
    }

    pub fn due(&self, now: DateTime<Utc>) -> bool {
        !self.paused && !self.done && self.at <= now
    }

    /// Records a run; a one-shot job is then done, a daily one moves to its next occurrence.
    pub fn ran(&mut self, conv: Uuid, now: DateTime<Utc>) {
        self.runs.push(Run { at: now, conv });
        if self.runs.len() > RUNS_KEPT {
            self.runs.remove(0);
        }
        if self.daily {
            self.advance(now);
        } else {
            self.done = true;
        }
    }

    /// Still waiting for its next run.
    pub fn pending(&self) -> bool {
        !self.done
    }

    /// Daily jobs move to their next occurrence at the same local time, across DST changes;
    /// one-shot jobs are removed by the caller.
    pub fn advance(&mut self, now: DateTime<Utc>) {
        let local = self.at.with_timezone(&Local);
        let (mut day, wall) = (local.date_naive(), local.time());
        while self.at <= now {
            let Some(next) = day.succ_opt() else { break };
            day = next;
            // A wall time skipped by the spring-forward jump fires one hour later that day.
            self.at = instant(day, wall)
                .or_else(|| instant(day, wall + Duration::hours(1)))
                .unwrap_or_else(|| self.at + Duration::days(1));
        }
    }

    /// `24/10 08:30 · chaque jour · Claude Opus 5.5 · effort Max`
    pub fn label(&self) -> String {
        let when = self.at.with_timezone(&Local).format("%d/%m %H:%M");
        let repeat = if self.daily { "chaque jour" } else { "une fois" };
        let who =
            if self.model.trim().is_empty() { self.engine.label().to_owned() } else { models::name(self.engine, &self.model) };
        format!("{when} · {repeat} · {who} · effort {}", self.effort.label())
    }
}

/// `"2026-10-24"` + `"08:30"` in local time → UTC instant.
pub fn parse_when(date: &str, time: &str) -> Result<DateTime<Utc>, String> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").map_err(|_| "date attendue : AAAA-MM-JJ".to_owned())?;
    let time = NaiveTime::parse_from_str(time.trim(), "%H:%M").map_err(|_| "heure attendue : HH:MM".to_owned())?;
    instant(date, time).ok_or_else(|| "heure locale inexistante (changement d'heure)".to_owned())
}

/// Local wall-clock time → UTC instant; the earlier one when the time repeats (fall back).
fn instant(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    Local.from_local_datetime(&date.and_time(time)).earliest().map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    #[test]
    fn a_job_fires_once_its_instant_is_reached() {
        let job = Job::new("audit".into(), at(24, 8), false, Engine::Claude, String::new(), Effort::Max);
        assert!(!job.due(at(24, 7)));
        assert!(job.due(at(24, 8)));
        assert!(job.due(at(25, 1)));
    }

    #[test]
    fn a_daily_job_moves_to_the_next_day() {
        let first = at(24, 8);
        let mut job = Job::new("veille".into(), first, true, Engine::Claude, String::new(), Effort::Medium);
        for now in [at(24, 9), at(27, 12)] {
            job.advance(now);
            assert!(job.at > now && job.at - now <= Duration::hours(25), "{}", job.at);
            assert_eq!(job.at.with_timezone(&Local).time(), first.with_timezone(&Local).time());
        }
    }

    #[test]
    fn a_daily_job_keeps_its_local_time_across_a_dst_change() {
        // 25/10/2026 is a DST change in Europe: whatever the local zone, the wall-clock time must not drift.
        let first = parse_when("2026-10-24", "08:30").unwrap();
        let mut job = Job::new("veille".into(), first, true, Engine::Claude, String::new(), Effort::Medium);
        job.advance(first + Duration::days(3) - Duration::hours(2));
        let local = job.at.with_timezone(&Local);
        assert_eq!(local.format("%Y-%m-%d %H:%M").to_string(), "2026-10-27 08:30");
    }

    #[test]
    fn runs_are_recorded_and_one_shots_finish() {
        let mut once = Job::new("x".into(), at(24, 8), false, Engine::Claude, String::new(), Effort::Max);
        let conv = Uuid::new_v4();
        once.ran(conv, at(24, 9));
        assert!(once.done && !once.due(at(30, 0)), "une fois : terminée, jamais relancée");
        assert_eq!(once.runs.last().map(|r| r.conv), Some(conv));
        let mut daily = Job::new("y".into(), at(24, 8), true, Engine::Claude, String::new(), Effort::Max);
        for _ in 0..12 {
            daily.ran(Uuid::new_v4(), daily.at);
        }
        assert!(!daily.done && daily.runs.len() == 10, "historique borné");
        daily.paused = true;
        assert!(!daily.due(daily.at + Duration::days(2)), "en pause : jamais déclenchée");
        let old: Job = serde_json::from_str(&serde_json::to_string(&Job::new("z".into(), at(24, 8), false, Engine::Claude, String::new(), Effort::Low)).unwrap().replace(",\"paused\":false,\"done\":false,\"runs\":[]", "")).unwrap();
        assert!(!old.paused && !old.done && old.runs.is_empty(), "ancien fichier relu");
    }

    #[test]
    fn when_parsing_rejects_garbage() {
        assert!(parse_when("24/10/2026", "08:30").is_err());
        assert!(parse_when("2026-10-24", "8h30").is_err());
        assert!(parse_when("2026-10-24", "08:30").is_ok());
    }

    #[test]
    fn label_reads_like_a_sentence() {
        let job = Job::new("x".into(), at(24, 8), true, Engine::Claude, "claude-opus-5-5".into(), Effort::Max);
        let label = job.label();
        assert!(label.contains("chaque jour") && label.contains("Claude Opus 5.5") && label.contains("effort Max"), "{label}");
        let legacy = Job::new("x".into(), at(24, 8), false, Engine::Claude, String::new(), Effort::Max);
        assert!(legacy.label().contains("Claude"), "un job sans modèle nomme le moteur");
    }
}
