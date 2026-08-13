use crate::email_index::EmailIndex;
use anyhow::{Context, Result};
use regex::Regex;
use sqlx::SqlitePool;
use std::io::{self, IsTerminal, Write};
use tracing::info;

/// Search all threads for `pattern`. Matching lines are printed; match ranges are
/// highlighted in green when stdout is a terminal.
pub async fn run_grep(pool: &SqlitePool, pattern: &str) -> Result<()> {
    let re = Regex::new(pattern).with_context(|| format!("Invalid regex: {pattern}"))?;
    let color = io::stdout().is_terminal();

    info!("Loading email metadata...");
    let index = EmailIndex::load(pool).await?;
    let threads = index.threads();
    info!(
        "Loaded {} emails in {} threads; loading bodies...",
        index.len(),
        threads.len()
    );

    let bodies = EmailIndex::load_all_bodies(pool).await?;
    info!("Searching...");

    let mut stdout = io::stdout().lock();
    let mut match_lines = 0usize;
    let mut match_threads = 0usize;

    for thread in &threads {
        let text = index.compose_thread_text(thread, &bodies);
        let mut thread_header_printed = false;

        for line in text.lines() {
            if !re.is_match(line) {
                continue;
            }
            if !thread_header_printed {
                writeln!(stdout, "\n=== {} ({}) ===", thread.subject, thread.root_id)?;
                thread_header_printed = true;
                match_threads += 1;
            }
            let highlighted = highlight_matches(line, &re, color);
            writeln!(stdout, "{highlighted}")?;
            match_lines += 1;
        }
    }

    info!("Matched {match_lines} lines in {match_threads} threads");
    Ok(())
}

/// Highlight each non-overlapping regex match in `line` with green ANSI when `color`.
fn highlight_matches(line: &str, re: &Regex, color: bool) -> String {
    if !color {
        return line.to_string();
    }

    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    for m in re.find_iter(line) {
        out.push_str(&line[last..m.start()]);
        out.push_str("\x1b[32m");
        out.push_str(m.as_str());
        out.push_str("\x1b[0m");
        last = m.end();
    }
    out.push_str(&line[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_all_ranges() {
        let re = Regex::new("foo|bar").unwrap();
        let s = highlight_matches("x foo y bar z", &re, true);
        assert_eq!(s, "x \x1b[32mfoo\x1b[0m y \x1b[32mbar\x1b[0m z");
    }

    #[test]
    fn no_color_is_identity() {
        let re = Regex::new("foo").unwrap();
        assert_eq!(highlight_matches("a foo b", &re, false), "a foo b");
    }
}
