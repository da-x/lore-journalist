//! Calendar week windows and week-resolution matrix (design KD1, KD9, KD15).

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, NaiveDate, Utc};
use std::fs;
use std::path::Path;
use tracing::warn;

/// Half-open UTC window for week ending on `w`:
/// `[W−6 00:00:00 UTC, W+1 00:00:00 UTC)`.
pub fn week_window(w: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let start_date = w
        .checked_sub_days(Days::new(6))
        .expect("week start date underflow");
    let end_date = w
        .checked_add_days(Days::new(1))
        .expect("week end date overflow");
    let start = start_date
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_utc();
    let end_exclusive = end_date
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_utc();
    (start, end_exclusive)
}

/// True if `t` falls in the half-open week window for `w`.
#[allow(dead_code)]
pub fn in_week_window(w: NaiveDate, t: DateTime<Utc>) -> bool {
    let (start, end_excl) = week_window(w);
    t >= start && t < end_excl
}

/// Fail if week ending `w` has not ended yet relative to `today` (UTC date).
pub fn assert_week_ended_at(w: NaiveDate, today: NaiveDate) -> Result<()> {
    if today <= w {
        bail!("week ending {w} has not ended yet (UTC today is {today}); cannot summarize");
    }
    Ok(())
}

/// Fail if week ending `w` has not ended yet (uses `Utc::now()` date).
pub fn assert_week_ended(w: NaiveDate) -> Result<()> {
    assert_week_ended_at(w, Utc::now().date_naive())
}

/// Outcome of the week resolution matrix (before wall-clock end checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveWeekOutcome {
    /// Run summarization for this week end date.
    Process(NaiveDate),
    /// Week dir already has `.complete`; exit success with no work.
    AlreadyComplete(NaiveDate),
}

/// Pure resolution of which week to process (design KD15).
///
/// `complete` / `incomplete` are week-ending dates discovered under `outputs_path`.
/// Warnings (e.g. ignored `--start-week`) are returned for the caller to log.
pub fn resolve_week(
    week: Option<NaiveDate>,
    start_week: Option<NaiveDate>,
    mut complete: Vec<NaiveDate>,
    mut incomplete: Vec<NaiveDate>,
) -> Result<(ResolveWeekOutcome, Vec<String>)> {
    complete.sort_unstable();
    complete.dedup();
    incomplete.sort_unstable();
    incomplete.dedup();

    let mut warnings = Vec::new();

    if let Some(w) = week {
        if start_week.is_some() {
            warnings.push("--week takes precedence; ignoring --start-week".to_string());
        }
        if complete.binary_search(&w).is_ok() {
            return Ok((ResolveWeekOutcome::AlreadyComplete(w), warnings));
        }
        // Missing or incomplete → process W.
        return Ok((ResolveWeekOutcome::Process(w), warnings));
    }

    // No --week: incomplete-first rules.
    if incomplete.len() > 1 {
        let list = incomplete
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "multiple incomplete week directories (no .complete): [{list}]; \
             finish or remove extras so at most one incomplete week remains"
        );
    }

    if incomplete.len() == 1 {
        let i = incomplete[0];
        if start_week.is_some() {
            warnings.push(format!(
                "resuming incomplete week {i}; ignoring --start-week"
            ));
        }
        return Ok((ResolveWeekOutcome::Process(i), warnings));
    }

    // No incomplete.
    if let Some(&last) = complete.last() {
        let next = last
            .checked_add_days(Days::new(7))
            .context("week +7 overflow")?;
        if let Some(s) = start_week {
            if s != next {
                bail!(
                    "--start-week is bootstrap-only; complete weeks already exist \
                     (next auto week is {next}, got --start-week {s})"
                );
            }
            // s == next: allow as explicit confirmation of the chain head.
        }
        return Ok((ResolveWeekOutcome::Process(next), warnings));
    }

    // Empty: no complete, no incomplete.
    let Some(s) = start_week else {
        bail!(
            "outputs_path has no week directories; pass --start-week YYYY-MM-DD \
             for the first edition"
        );
    };
    Ok((ResolveWeekOutcome::Process(s), warnings))
}

/// Parse `YYYY-MM-DD` into a calendar date.
pub fn parse_week_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("invalid week date {s:?}, expected YYYY-MM-DD"))
}

/// Scan `outputs_path` for `YYYY-MM-DD` directories; classify complete vs incomplete.
pub fn scan_week_dirs(outputs_path: &Path) -> Result<(Vec<NaiveDate>, Vec<NaiveDate>)> {
    let mut complete = Vec::new();
    let mut incomplete = Vec::new();

    if !outputs_path.exists() {
        return Ok((complete, incomplete));
    }

    for entry in fs::read_dir(outputs_path)
        .with_context(|| format!("read_dir {}", outputs_path.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Skip lock / hidden dirs that are not week folders.
        if name.starts_with('.') {
            continue;
        }
        let Ok(date) = NaiveDate::parse_from_str(name, "%Y-%m-%d") else {
            continue;
        };
        // Sanity: folder name must round-trip (reject non-calendar junk that parses).
        if date.format("%Y-%m-%d").to_string() != name {
            continue;
        }
        let marker = entry.path().join(".complete");
        if marker.is_file() {
            complete.push(date);
        } else {
            incomplete.push(date);
        }
    }

    complete.sort_unstable();
    incomplete.sort_unstable();
    Ok((complete, incomplete))
}

/// Resolve week from CLI flags + filesystem; log warnings via `tracing`.
pub fn resolve_week_from_outputs(
    outputs_path: &Path,
    week: Option<&str>,
    start_week: Option<&str>,
) -> Result<ResolveWeekOutcome> {
    let week = week.map(parse_week_date).transpose()?;
    let start_week = start_week.map(parse_week_date).transpose()?;
    let (complete, incomplete) = scan_week_dirs(outputs_path)?;
    let (outcome, warnings) = resolve_week(week, start_week, complete, incomplete)?;
    for w in warnings {
        warn!("{w}");
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    #[test]
    fn week_window_half_open_for_2026_07_20() {
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let (start, end_excl) = week_window(w);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap());
        assert_eq!(
            end_excl,
            Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn week_window_boundaries() {
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let at_start = Utc.with_ymd_and_hms(2026, 7, 14, 0, 0, 0).unwrap();
        let late_on_w = Utc.with_ymd_and_hms(2026, 7, 20, 23, 59, 59).unwrap();
        let at_end_excl = Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0).unwrap();
        let before = Utc.with_ymd_and_hms(2026, 7, 13, 23, 59, 59).unwrap();

        assert!(in_week_window(w, at_start));
        assert!(in_week_window(w, late_on_w));
        assert!(!in_week_window(w, at_end_excl));
        assert!(!in_week_window(w, before));
    }

    #[test]
    fn assert_week_ended_at_rules() {
        let w = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        assert!(assert_week_ended_at(w, NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()).is_err());
        assert!(assert_week_ended_at(w, NaiveDate::from_ymd_opt(2026, 7, 19).unwrap()).is_err());
        assert!(assert_week_ended_at(w, NaiveDate::from_ymd_opt(2026, 7, 21).unwrap()).is_ok());
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn resolve_empty_requires_start_week() {
        let err = resolve_week(None, None, vec![], vec![]).unwrap_err();
        assert!(err.to_string().contains("--start-week"));
    }

    #[test]
    fn resolve_bootstrap_start_week() {
        let (out, _) = resolve_week(None, Some(d(2026, 7, 20)), vec![], vec![]).unwrap();
        assert_eq!(out, ResolveWeekOutcome::Process(d(2026, 7, 20)));
    }

    #[test]
    fn resolve_plus_seven_after_complete() {
        let (out, _) = resolve_week(None, None, vec![d(2026, 7, 13)], vec![]).unwrap();
        assert_eq!(out, ResolveWeekOutcome::Process(d(2026, 7, 20)));
    }

    #[test]
    fn resolve_start_week_conflicts_with_chain() {
        let err =
            resolve_week(None, Some(d(2026, 1, 1)), vec![d(2026, 7, 13)], vec![]).unwrap_err();
        assert!(err.to_string().contains("bootstrap-only"));
    }

    #[test]
    fn resolve_start_week_matching_next_ok() {
        let (out, _) =
            resolve_week(None, Some(d(2026, 7, 20)), vec![d(2026, 7, 13)], vec![]).unwrap();
        assert_eq!(out, ResolveWeekOutcome::Process(d(2026, 7, 20)));
    }

    #[test]
    fn resolve_single_incomplete_resumes_ignores_start() {
        let (out, warnings) = resolve_week(
            None,
            Some(d(2026, 1, 1)),
            vec![d(2026, 7, 6)],
            vec![d(2026, 7, 13)],
        )
        .unwrap();
        assert_eq!(out, ResolveWeekOutcome::Process(d(2026, 7, 13)));
        assert!(warnings.iter().any(|w| w.contains("ignoring --start-week")));
    }

    #[test]
    fn resolve_multiple_incomplete_errors() {
        let err =
            resolve_week(None, None, vec![], vec![d(2026, 7, 6), d(2026, 7, 13)]).unwrap_err();
        assert!(err.to_string().contains("multiple incomplete"));
    }

    #[test]
    fn resolve_week_flag_already_complete() {
        let (out, _) =
            resolve_week(Some(d(2026, 7, 13)), None, vec![d(2026, 7, 13)], vec![]).unwrap();
        assert_eq!(out, ResolveWeekOutcome::AlreadyComplete(d(2026, 7, 13)));
    }

    #[test]
    fn resolve_week_flag_incomplete() {
        let (out, warnings) = resolve_week(
            Some(d(2026, 7, 20)),
            Some(d(2026, 1, 1)),
            vec![d(2026, 7, 13)],
            vec![d(2026, 7, 20)],
        )
        .unwrap();
        assert_eq!(out, ResolveWeekOutcome::Process(d(2026, 7, 20)));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("--week takes precedence"))
        );
    }

    #[test]
    fn resolve_week_flag_missing_dir() {
        let (out, _) =
            resolve_week(Some(d(2026, 8, 3)), None, vec![d(2026, 7, 27)], vec![]).unwrap();
        assert_eq!(out, ResolveWeekOutcome::Process(d(2026, 8, 3)));
    }

    #[test]
    fn scan_week_dirs_classifies() {
        let dir = tempfile_dir();
        fs::create_dir_all(dir.join("2026-07-13")).unwrap();
        fs::write(dir.join("2026-07-13").join(".complete"), b"").unwrap();
        fs::create_dir_all(dir.join("2026-07-20")).unwrap();
        fs::create_dir_all(dir.join("not-a-date")).unwrap();
        fs::write(dir.join(".summarize-week.lock"), b"").unwrap();

        let (complete, incomplete) = scan_week_dirs(&dir).unwrap();
        assert_eq!(complete, vec![d(2026, 7, 13)]);
        assert_eq!(incomplete, vec![d(2026, 7, 20)]);
    }

    fn tempfile_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "lore-week-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
