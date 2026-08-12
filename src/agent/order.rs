//! Ordering agent: rank week's threads for serial summarization.

use super::session::{run_until_submit, ORDER_AGENT_TIMEOUT};
use super::tool_build::build_order_tools;
use crate::ids::normalize_message_id;
use crate::outputs::{thread_order_path, write_atomic};
use crate::summarize::ActiveThread;
use crate::tools::submit::{SubmitSlot, ThreadOrderPayload};
use crate::tools::ToolCtx;
use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use da_harness::multi_tool::InferenceCallback;
use da_harness::OpenAIClient;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

const ORDER_SYSTEM: &str = r#"You are planning work for a serial weekly mailing-list summarizer.
Given the catalog of discussions active this week, decide the order in which they should be summarized.
Prefer: foundational patches / parent series before follow-ups; discussions that other threads cite before dependents; independent topics last or by last activity.
Use tools if helpful to check subjects and related roots. Do NOT write summaries.
Call SubmitThreadOrder exactly once with every catalog root_id exactly once (a permutation).
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
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let file: ThreadOrderFile = serde_json::from_str(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    match validate_permutation(&file.ordered_root_ids, expected) {
        Ok(order) => {
            info!(%week, "reusing valid .thread-order.json");
            Ok(Some(order))
        }
        Err(e) => {
            warn!(%week, error = %e, "ignoring invalid .thread-order.json");
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

/// Normalize and require a permutation of expected root ids.
pub fn validate_permutation(
    ordered: &[String],
    expected: &HashSet<String>,
) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(ordered.len());
    for id in ordered {
        let n = normalize_message_id(id);
        if !expected.contains(&n) {
            bail!("ordered root not in expected set: {n}");
        }
        if !seen.insert(n.clone()) {
            bail!("duplicate root in order: {n}");
        }
        out.push(n);
    }
    if seen.len() != expected.len() {
        let missing: Vec<_> = expected.difference(&seen).cloned().collect();
        bail!("order missing {} root(s), e.g. {:?}", missing.len(), missing.first());
    }
    Ok(out)
}

pub fn build_order_user_message(week: NaiveDate, active: &[ActiveThread]) -> String {
    let mut s = format!(
        "Week ending: {}\nOrder these discussions for serial summarization.\n\nCatalog:\n",
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
    s.push_str(
        "\nCall SubmitThreadOrder with every root_id exactly once.\n",
    );
    s
}

/// Run ordering agent (or return cached valid order).
pub async fn obtain_thread_order(
    ctx: ToolCtx,
    week: NaiveDate,
    active: &[ActiveThread],
    client: Option<OpenAIClient>,
    inference: Option<InferenceCallback>,
) -> Result<Vec<String>> {
    let expected: HashSet<String> = active.iter().map(|t| t.root_id.clone()).collect();
    if let Some(order) =
        load_valid_thread_order(&ctx.outputs_path, week, &expected)?
    {
        return Ok(order);
    }

    if active.len() == 1 {
        let order = vec![active[0].root_id.clone()];
        write_thread_order_file(
            &ctx.outputs_path,
            week,
            &ThreadOrderPayload {
                ordered_root_ids: order.clone(),
                notes: Some("single thread; skipped LLM order".into()),
            },
        )?;
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
    )
    .await
    .context("ordering agent")?;

    let order = validate_permutation(&payload.ordered_root_ids, &expected)
        .context("SubmitThreadOrder validation")?;
    let payload = ThreadOrderPayload {
        ordered_root_ids: order.clone(),
        notes: payload.notes,
    };
    write_thread_order_file(&ctx.outputs_path, week, &payload)?;
    info!(%week, n = order.len(), "wrote .thread-order.json");
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutation_ok_and_errors() {
        let exp: HashSet<_> = ["<a>", "<b>", "<c>"].into_iter().map(String::from).collect();
        let ok = validate_permutation(
            &[" <b>".into(), "<a>".into(), "<c>".into()],
            &exp,
        )
        .unwrap();
        assert_eq!(ok, vec!["<b>", "<a>", "<c>"]);

        assert!(validate_permutation(&["<a>".into(), "<b>".into()], &exp).is_err());
        assert!(validate_permutation(
            &["<a>".into(), "<b>".into(), "<c>".into(), "<a>".into()],
            &exp
        )
        .is_err());
        assert!(validate_permutation(
            &["<a>".into(), "<b>".into(), "<x>".into()],
            &exp
        )
        .is_err());
    }
}
