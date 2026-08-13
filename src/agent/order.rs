//! Ordering agent: rank week's threads for serial summarization.

use super::session::{ORDER_AGENT_TIMEOUT, UsageStage, UsageTotals, run_until_submit};
use super::tool_build::build_order_tools;
use crate::ids::normalize_message_id;
use crate::outputs::{thread_order_path, write_atomic};
use crate::summarize::ActiveThread;
use crate::tools::ToolCtx;
use crate::tools::submit::{SubmitSlot, ThreadOrderPayload};
use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use da_harness::OpenAIClient;
use da_harness::multi_tool::InferenceCallback;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

const ORDER_SYSTEM: &str = r#"You are planning work for a serial weekly mailing-list summarizer.
Given the catalog of discussions active this week, decide the order in which they should be summarized.
Prefer: foundational patches / parent series before follow-ups; discussions that other threads cite before dependents; independent topics last or by last activity.
Use tools if helpful to check subjects and related roots. Do NOT write summaries.

CRITICAL — SubmitThreadOrder:
- Call SubmitThreadOrder exactly once.
- ordered_root_ids MUST be a permutation of the REQUIRED_ROOT_IDS list in the user message.
- Copy each root_id EXACTLY (including angle brackets). No trailing commas, quotes, or ellipses.
- Include EVERY required id exactly once — no omissions, no extras, no duplicates.
- Count must equal N_REQUIRED.
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadOrderFile {
    pub week_ending: String,
    pub ordered_root_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Load `.thread-order.json` if it is a valid permutation of `expected` roots.
pub fn load_valid_thread_order(
    outputs_path: &Path,
    week: NaiveDate,
    expected: &HashSet<String>,
) -> Result<Option<Vec<String>>> {
    let path = thread_order_path(outputs_path, week);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let file: ThreadOrderFile =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    // Prefer strict reuse; fall back to repair if file is slightly dirty.
    match coerce_to_permutation(&file.ordered_root_ids, expected, &[]) {
        Ok((order, report)) => {
            if report.changed() {
                warn!(
                    %week,
                    dropped_unknown = report.dropped_unknown.len(),
                    dropped_dupes = report.dropped_duplicates,
                    appended = report.appended.len(),
                    "repaired .thread-order.json on load; rewriting"
                );
                // Persist the cleaned permutation so resume does not re-repair forever
                // and so operators inspecting the file see the real order.
                let notes = file.notes.map(|n| {
                    if n.contains("[host-repaired order]") {
                        n
                    } else {
                        format!("{n} [host-repaired order]")
                    }
                });
                if let Err(e) = write_thread_order_file(
                    outputs_path,
                    week,
                    &ThreadOrderPayload {
                        ordered_root_ids: order.clone(),
                        notes: notes.or_else(|| Some("host-repaired order".into())),
                    },
                ) {
                    warn!(%week, error = %e, "failed to rewrite repaired .thread-order.json");
                }
            } else {
                info!(
                    %week,
                    ordered_root_ids = ?order,
                    notes = ?file.notes,
                    "reusing valid .thread-order.json"
                );
            }
            Ok(Some(order))
        }
        Err(e) => {
            warn!(%week, error = %e, "ignoring unusable .thread-order.json");
            Ok(None)
        }
    }
}

pub fn write_thread_order_file(
    outputs_path: &Path,
    week: NaiveDate,
    payload: &ThreadOrderPayload,
) -> Result<()> {
    let file = ThreadOrderFile {
        week_ending: week.format("%Y-%m-%d").to_string(),
        ordered_root_ids: payload.ordered_root_ids.clone(),
        notes: payload.notes.clone(),
    };
    let json = serde_json::to_string_pretty(&file).context("serialize thread order")?;
    let path = thread_order_path(outputs_path, week);
    write_atomic(&path, &format!("{json}\n"))?;
    Ok(())
}

/// Strip common LLM / header junk around Message-IDs (delegates to `normalize_message_id`).
pub fn sanitize_root_id(id: &str) -> String {
    normalize_message_id(id)
}

#[derive(Debug, Default, Clone)]
pub struct OrderRepairReport {
    pub dropped_unknown: Vec<String>,
    pub dropped_duplicates: usize,
    pub appended: Vec<String>,
    pub sanitized_from: Vec<(String, String)>, // (raw, cleaned) when different
}

impl OrderRepairReport {
    /// Any host-side change (including id sanitization).
    pub fn changed(&self) -> bool {
        self.structural_change() || !self.sanitized_from.is_empty()
    }

    /// Drops / appends (not mere whitespace/comma cleanup).
    pub fn structural_change(&self) -> bool {
        !self.dropped_unknown.is_empty() || self.dropped_duplicates > 0 || !self.appended.is_empty()
    }
}

/// Coerce a model-provided order into a full permutation of `expected`.
///
/// - Sanitizes each id (trim, strip trailing commas)
/// - Drops unknowns and duplicates (first wins)
/// - Appends any missing ids in `catalog_order` sequence (fallback: sorted)
///
/// `catalog_order` should be the host catalog order (e.g. active threads list).
pub fn coerce_to_permutation(
    ordered: &[String],
    expected: &HashSet<String>,
    catalog_order: &[String],
) -> Result<(Vec<String>, OrderRepairReport)> {
    if expected.is_empty() {
        bail!("expected root set is empty");
    }

    let mut report = OrderRepairReport::default();
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(expected.len());

    for id in ordered {
        let raw = id.to_string();
        let n = sanitize_root_id(id);
        if n != raw {
            report.sanitized_from.push((raw, n.clone()));
        }

        if n.is_empty() {
            continue;
        }
        if !expected.contains(&n) {
            report.dropped_unknown.push(n);
            continue;
        }
        if !seen.insert(n.clone()) {
            report.dropped_duplicates += 1;
            continue;
        }
        out.push(n);
    }

    // Append missing in catalog order, then any remaining sorted.
    let missing: HashSet<String> = expected.difference(&seen).cloned().collect();
    if !missing.is_empty() {
        let mut ordered_missing = Vec::new();
        for c in catalog_order {
            let c = sanitize_root_id(c);
            if missing.contains(&c) && !ordered_missing.contains(&c) {
                ordered_missing.push(c);
            }
        }
        let mut rest: Vec<_> = missing
            .into_iter()
            .filter(|m| !ordered_missing.contains(m))
            .collect();
        rest.sort();
        ordered_missing.extend(rest);

        for m in ordered_missing {
            report.appended.push(m.clone());
            out.push(m);
        }
    }

    if out.len() != expected.len() {
        bail!(
            "failed to build full permutation: got {} ids, expected {}",
            out.len(),
            expected.len()
        );
    }
    let got: HashSet<_> = out.iter().cloned().collect();
    if got != *expected {
        bail!("repaired order still does not match expected set");
    }
    Ok((out, report))
}

/// Strict validate: must already be a clean permutation (only whitespace normalize ok).
#[allow(dead_code)] // used in tests / optional strict callers
pub fn validate_permutation(ordered: &[String], expected: &HashSet<String>) -> Result<Vec<String>> {
    let (order, report) = coerce_to_permutation(ordered, expected, &[])?;
    if report.structural_change() {
        bail!(
            "order is not a clean permutation (dropped_unknown={}, dupes={}, appended={})",
            report.dropped_unknown.len(),
            report.dropped_duplicates,
            report.appended.len()
        );
    }
    Ok(order)
}

pub fn build_order_user_message(week: NaiveDate, active: &[ActiveThread]) -> String {
    let n = active.len();
    let mut s = format!(
        "Week ending: {}\nN_REQUIRED: {n}\nOrder these discussions for serial summarization.\n\nCatalog:\n",
        week.format("%Y-%m-%d")
    );
    for (i, t) in active.iter().enumerate() {
        s.push_str(&format!(
            "{}. root_id={} messages_this_week={} subject={}\n",
            i + 1,
            t.root_id,
            t.message_indices.len(),
            t.subject
        ));
    }
    s.push_str(&format!(
        "\nREQUIRED_ROOT_IDS (exactly {n}; copy each line exactly into ordered_root_ids):\n"
    ));
    for t in active {
        s.push_str(&t.root_id);
        s.push('\n');
    }
    s.push_str(
        "\nCall SubmitThreadOrder once with ordered_root_ids = a permutation of REQUIRED_ROOT_IDS (same count, no trailing commas).\n",
    );
    s
}

/// Run ordering agent (or return cached valid order).
#[allow(clippy::too_many_arguments)]
pub async fn obtain_thread_order(
    ctx: ToolCtx,
    week: NaiveDate,
    active: &[ActiveThread],
    client: Option<OpenAIClient>,
    inference: Option<InferenceCallback>,
    usage: UsageTotals,
) -> Result<Vec<String>> {
    let expected: HashSet<String> = active.iter().map(|t| t.root_id.clone()).collect();
    if let Some(order) = load_valid_thread_order(&ctx.outputs_path, week, &expected)? {
        return Ok(order);
    }

    if active.len() == 1 {
        let order = vec![active[0].root_id.clone()];
        let notes = Some("single thread; skipped LLM order".into());
        write_thread_order_file(
            &ctx.outputs_path,
            week,
            &ThreadOrderPayload {
                ordered_root_ids: order.clone(),
                notes: notes.clone(),
            },
        )?;
        info!(
            %week,
            n = 1,
            ordered_root_ids = ?order,
            notes = ?notes,
            "wrote .thread-order.json"
        );
        return Ok(order);
    }

    // Scope related search to this week's roots.
    let ctx = ctx.with_allowed_roots(Some(expected.clone()));
    let slot = SubmitSlot::new();
    let tools = build_order_tools(ctx.clone(), slot.clone())?;
    let user = build_order_user_message(week, active);

    let payload = run_until_submit(
        ORDER_SYSTEM,
        user,
        tools,
        slot,
        ORDER_AGENT_TIMEOUT,
        client,
        inference,
        usage,
        UsageStage::Order,
    )
    .await
    .context("ordering agent")?;

    let catalog: Vec<String> = active.iter().map(|t| t.root_id.clone()).collect();
    let (order, report) = coerce_to_permutation(&payload.ordered_root_ids, &expected, &catalog)
        .context("SubmitThreadOrder validation/repair")?;
    if report.changed() {
        warn!(
            %week,
            dropped_unknown = ?report.dropped_unknown,
            dropped_dupes = report.dropped_duplicates,
            appended = ?report.appended,
            sanitized = report.sanitized_from.len(),
            "repaired LLM thread order into a full permutation"
        );
    }
    let notes = match (payload.notes, report.changed()) {
        (Some(n), true) => Some(format!("{n} [host-repaired order]")),
        (None, true) => Some("host-repaired order".into()),
        (n, false) => n,
    };
    let payload = ThreadOrderPayload {
        ordered_root_ids: order.clone(),
        notes,
    };
    write_thread_order_file(&ctx.outputs_path, week, &payload)?;
    info!(
        %week,
        n = order.len(),
        ordered_root_ids = ?order,
        notes = ?payload.notes,
        "wrote .thread-order.json"
    );
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_trailing_comma() {
        assert_eq!(
            sanitize_root_id("<20251230141838.2547848-1-cel@kernel.org>,"),
            "<20251230141838.2547848-1-cel@kernel.org>"
        );
        assert_eq!(sanitize_root_id("  <a@b>, "), "<a@b>");
    }

    #[test]
    fn coerce_repairs_trailing_commas_and_missing() {
        let exp: HashSet<_> = ["<a>", "<b>", "<c>"]
            .into_iter()
            .map(String::from)
            .collect();
        let catalog = vec!["<a>".into(), "<b>".into(), "<c>".into()];
        let (order, report) =
            coerce_to_permutation(&["<b>,".into(), "<a>".into()], &exp, &catalog).unwrap();
        assert_eq!(order, vec!["<b>", "<a>", "<c>"]);
        assert!(report.appended.contains(&"<c>".to_string()));
        assert!(!report.sanitized_from.is_empty() || order[0] == "<b>");
    }

    #[test]
    fn coerce_drops_unknown_and_dupes() {
        let exp: HashSet<_> = ["<a>", "<b>"].into_iter().map(String::from).collect();
        let catalog = vec!["<a>".into(), "<b>".into()];
        let (order, report) = coerce_to_permutation(
            &["<a>".into(), "<x>".into(), "<a>".into(), "<b>".into()],
            &exp,
            &catalog,
        )
        .unwrap();
        assert_eq!(order, vec!["<a>", "<b>"]);
        assert_eq!(report.dropped_duplicates, 1);
        assert!(report.dropped_unknown.iter().any(|u| u == "<x>"));
    }

    #[test]
    fn strict_validate_rejects_incomplete() {
        let exp: HashSet<_> = ["<a>", "<b>", "<c>"]
            .into_iter()
            .map(String::from)
            .collect();
        // validate_permutation fails if repair would be needed
        assert!(validate_permutation(&["<a>".into(), "<b>".into()], &exp).is_err());
        let ok = validate_permutation(&[" <b>".into(), "<a>".into(), "<c>".into()], &exp).unwrap();
        assert_eq!(ok, vec!["<b>", "<a>", "<c>"]);
    }
}
