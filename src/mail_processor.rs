use crate::models::{EmailMessage, Thread};
use std::collections::HashMap;

pub fn process_threads(messages: Vec<EmailMessage>, active_window_days: i64) -> Vec<Thread> {
    let now = messages.iter().map(|m| m.date).max();

    if now.is_none() {
        return Vec::new();
    }
    let now = now.unwrap();
    let active_cutoff = now - chrono::Duration::days(active_window_days);

    let mut id_to_msg = HashMap::new();
    for msg in &messages {
        id_to_msg.insert(msg.message_id.clone(), msg.clone());
    }

    let mut thread_map: HashMap<String, Vec<EmailMessage>> = HashMap::new();

    for msg in messages {
        let root_id = if !msg.references.is_empty() {
            msg.references[0].clone()
        } else if let Some(ref parent) = msg.in_reply_to {
            parent.clone()
        } else {
            msg.message_id.clone()
        };

        thread_map.entry(root_id).or_default().push(msg);
    }

    let mut threads: Vec<Thread> = thread_map
        .into_iter()
        .map(|(root_id, mut msgs)| {
            msgs.sort_by_key(|m| m.date);
            let subject = msgs[0].subject.clone();
            Thread {
                root_id,
                subject,
                messages: msgs,
            }
        })
        .filter(|t| t.messages.iter().any(|m| m.date >= active_cutoff))
        .collect();

    threads.sort_by_key(|t| t.last_activity());
    threads.reverse();
    threads
}
