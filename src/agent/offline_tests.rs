//! Offline multi_tool tests via inference_callback (no live LLM).

#[cfg(test)]
mod tests {
    use crate::agent::order::{obtain_thread_order, validate_permutation};
    use crate::agent::thread::run_thread_agent;
    use crate::db::{insert_test_email, open_in_memory};
    use crate::email_index::EmailIndex;
    use crate::summarize::select_active_threads;
    use crate::tools::ToolCtx;
    use crate::week::week_window;
    use async_openai::types::{
        ChatCompletionMessageToolCall, ChatCompletionToolType, FunctionCall,
    };
    use chrono::NaiveDate;
    use da_harness::multi_tool::{assistant_tool_calls, InferenceCallback};
    use futures::FutureExt;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn tool_call(id: &str, name: &str, arguments: &str) -> ChatCompletionMessageToolCall {
        ChatCompletionMessageToolCall {
            id: id.into(),
            r#type: ChatCompletionToolType::Function,
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }

    fn temp_out() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "nfs-agent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn offline_order_agent_submits_permutation() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <a@t>",
            "Alpha",
            "a@b",
            "2026-07-16T00:00:00+00:00",
            "body a\n",
            None,
            "[]",
        )
        .await
        .unwrap();
        insert_test_email(
            &pool,
            " <b@t>",
            "Beta",
            "a@b",
            "2026-07-17T00:00:00+00:00",
            "body b\n",
            None,
            "[]",
        )
        .await
        .unwrap();

        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let week = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let out = temp_out();
        let active = select_active_threads(&index, week);
        assert_eq!(active.len(), 2);

        let ctx = ToolCtx::new(
            pool.clone(),
            index,
            out.clone(),
            week,
            week_window(week),
        );

        let call = Arc::new(AtomicUsize::new(0));
        let roots_json = serde_json::json!({
            "ordered_root_ids": ["<b@t>", "<a@t>"],
            "notes": "offline"
        })
        .to_string();

        let cb: InferenceCallback = Arc::new(move |_msgs| {
            let n = call.fetch_add(1, Ordering::SeqCst);
            let roots_json = roots_json.clone();
            async move {
                if n == 0 {
                    Ok(assistant_tool_calls(vec![tool_call(
                        "1",
                        "SubmitThreadOrder",
                        &roots_json,
                    )]))
                } else {
                    // After tool result, end with text (session ends via drop tx)
                    Ok(da_harness::multi_tool::assistant_text("done"))
                }
            }
            .boxed()
        });

        let order = obtain_thread_order(ctx, week, &active, None, Some(cb))
            .await
            .unwrap();
        assert_eq!(order, vec!["<b@t>", "<a@t>"]);
        assert!(out.join("2026-07-20/.thread-order.json").is_file());

        let _ = std::fs::remove_dir_all(&out);
    }

    #[tokio::test]
    async fn offline_thread_agent_writes_summary() {
        let pool = open_in_memory().await.unwrap();
        insert_test_email(
            &pool,
            " <solo@t>",
            "Solo thread",
            "alice@ex.com",
            "2026-07-18T12:00:00+00:00",
            "important body\n",
            None,
            "[]",
        )
        .await
        .unwrap();

        let index = Arc::new(EmailIndex::load(&pool).await.unwrap());
        let week = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let out = temp_out();
        std::fs::create_dir_all(out.join("2026-07-20/thread")).unwrap();

        let active = select_active_threads(&index, week);
        assert_eq!(active.len(), 1);
        let thread = &active[0];
        let order = vec![thread.root_id.clone()];

        let ctx = ToolCtx::new(
            pool.clone(),
            index.clone(),
            out.clone(),
            week,
            week_window(week),
        );

        let body = serde_json::json!({
            "title": "Solo summary",
            "markdown_body": "This week **solo** moved forward.",
            "key_message_ids": ["<solo@t>"]
        })
        .to_string();

        let call = Arc::new(AtomicUsize::new(0));
        let cb: InferenceCallback = Arc::new(move |_msgs| {
            let n = call.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            async move {
                if n == 0 {
                    Ok(assistant_tool_calls(vec![tool_call(
                        "1",
                        "SubmitThreadSummary",
                        &body,
                    )]))
                } else {
                    Ok(da_harness::multi_tool::assistant_text("ok"))
                }
            }
            .boxed()
        });

        let path = run_thread_agent(
            ctx,
            week,
            thread,
            index.as_ref(),
            &order,
            1,
            1,
            None,
            Some(cb),
        )
        .await
        .unwrap();

        assert!(path.is_file());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Solo summary"));
        assert!(text.contains("moved forward"));
        assert!(text.contains("lore.kernel.org/linux-nfs/solo@t"));

        // Resume skip
        let path2 = run_thread_agent(
            ToolCtx::new(
                pool,
                index.clone(),
                out.clone(),
                week,
                week_window(week),
            ),
            week,
            thread,
            index.as_ref(),
            &order,
            1,
            1,
            None,
            None, // would fail if re-run agent
        )
        .await
        .unwrap();
        assert_eq!(path, path2);

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn validate_permutation_unit() {
        let exp: HashSet<_> = ["<a>", "<b>"].into_iter().map(String::from).collect();
        assert!(validate_permutation(&["<b>".into(), "<a>".into()], &exp).is_ok());
    }
}
