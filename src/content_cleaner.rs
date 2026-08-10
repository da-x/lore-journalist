#![allow(unused)]

pub fn clean_email_body(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut cleaned_lines = Vec::new();
    let mut in_diff = false;
    let mut quote_block = Vec::new();

    for line in lines {
        // Handle diff detection
        if line.starts_with("diff --git") || line.starts_with("--- a/") {
            if !in_diff {
                cleaned_lines.push("[... Patch diff omitted for brevity ...]");
                in_diff = true;
            }
            continue;
        }

        // If we were in a diff, we stay in a diff until we see something that looks like a new section
        // (though in emails, the diff is usually at the end or followed by a signature)
        if in_diff {
            // Very simple heuristic: stop diff if we see a signature or something common
            if line == "-- " || line.starts_with("_______________________________________________")
            {
                in_diff = false;
            } else {
                continue;
            }
        }

        // Handle quote trimming
        if line.trim_start().starts_with('>') {
            quote_block.push(line);
            continue;
        } else {
            if !quote_block.is_empty() {
                process_quote_block(&mut cleaned_lines, &quote_block);
                quote_block.clear();
            }
            cleaned_lines.push(line);
        }
    }

    // Flush last quote block if any
    if !quote_block.is_empty() {
        process_quote_block(&mut cleaned_lines, &quote_block);
    }

    cleaned_lines.join("\n")
}

fn process_quote_block<'a>(cleaned_lines: &mut Vec<&'a str>, quote_block: &[&'a str]) {
    let is_mostly_diff = quote_block
        .iter()
        .filter(|l| {
            let t = l.trim_start_matches(|c| c == '>' || c == ' ');
            t.starts_with('+') || t.starts_with('-') || t.starts_with("@@")
        })
        .count()
        > quote_block.len() / 2;

    if quote_block.len() > 10 || (is_mostly_diff && quote_block.len() > 5) {
        cleaned_lines.push(quote_block[0]);
        if quote_block.len() > 2 {
            cleaned_lines.push(quote_block[1]);
            cleaned_lines.push("[... Trimmed large quote block ...]");
        } else if quote_block.len() > 1 {
            cleaned_lines.push("[... Trimmed large quote block ...]");
        }
        cleaned_lines.push(quote_block[quote_block.len() - 1]);
    } else {
        cleaned_lines.extend_from_slice(quote_block);
    }
}
