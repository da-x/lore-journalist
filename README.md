# Weekly mailing-list discussion summarizer

Turns a lore-style git mail archive (ingested into SQLite) into **one completed calendar week** of markdown under `outputs_path`, and optionally a mirrored static HTML tree under `html_outputs_path`. The list name, site titles, archive URL, and agent focus come from config so the same binary works for any mailing list. Agents explore mail and prior summaries via tools, then submit order / thread / week results. Per-message markdown is **not** written; citations are lore permalinks.

Design: [`doc/design.md`](doc/design.md).

## Build

```bash
cargo build --release
```

The binary name is `code` (see `Cargo.toml`). Examples below use `cargo run --`.

## Config

`config.toml` (pass `--config PATH`, default `config.toml`):

```toml
db_path = ".git/db.sqlite"
git_repo_path = "/path/to/mail-archive.git"
lore_base_url = "https://lore.kernel.org/your-list/"
outputs_path = "/path/to/weekly-outputs"   # required for summarize-week
html_outputs_path = "/path/to/weekly-html" # optional; omit to skip HTML
html_site_url = "https://example.com/weekly/" # optional; public prefix for og:url / canonical
html_og_image = "https://example.com/weekly/og.png" # optional; absolute image for Slack cards

[list]
title = "Example Mailing List Weekly Summaries"   # root catalog H1
short_title = "Example Weekly Summaries"          # HTML header; defaults to title
name = "the example mailing list"                 # agent role ("covering …")
focus = "Focus heavily on core development and important bug fixes."

[openai]
api_base = "https://api.x.ai/v1"
model_name = "..."
api_key = "..."
```

`outputs_path` is required for `summarize-week`, `regenerate-root-index`, and `render-html`. `html_outputs_path` is optional; when set, a successful week publish also writes static HTML there. `lore_base_url` is the public archive prefix for this list (default `https://lore.kernel.org/`).

`html_site_url` is the public prefix of that HTML tree (used only for `og:url` and `<link rel="canonical">`). If it is unset, an `http(s)` `base_url` is used; `base_url = "/"` is ignored. Without a public prefix, pages still get `og:title` / `og:description` / Twitter Card tags. Slack’s crawler must be able to GET the pasted URL — a host that is only reachable on Tailscale will not unfurl from Slack’s cloud.

`[list]` is how the same binary covers different mailing lists: titles go into the markdown catalog and HTML chrome; `name` and `focus` go into the thread and week agent prompts. Omit `[list]` for generic wording (`Mailing List Weekly Summaries` / `this mailing list`).

## Commands

| Command | Purpose |
|---|---|
| `build-db` | Walk the git mail repo and insert cleaned bodies into SQLite |
| `meta` | Load the email index and print count / date range |
| `grep PATTERN` | Regex search over composed subject + body |
| `summarize-week` | Produce **one** week edition (lock, order, serial threads, overview, `.complete`); also HTML if `html_outputs_path` is set |
| `regenerate-root-index` | Rewrite `{outputs}/index.md` from complete week dirs (no agents, no week rewrites) |
| `render-html` | Convert the existing markdown tree to static HTML (backfill / CSS refresh) |

```bash
cargo run -- --config config.toml build-db
cargo run -- --config config.toml meta
cargo run -- --config config.toml grep 'regression'
cargo run -- --config config.toml summarize-week --start-week 2026-07-20
cargo run -- --config config.toml summarize-week --week 2026-07-20
cargo run -- --config config.toml regenerate-root-index
cargo run -- --config config.toml render-html
cargo run -- --config config.toml render-html --html-dir /path/to/weekly-html
```

There is **no** `--concurrency` flag. Thread agents always run **serially** in the order from `.thread-order.json`.

### `regenerate-root-index`

Rewrites `{outputs}/index.md` from week directories that already have `.complete`. Headlines come from each week’s front matter. Incomplete weeks are skipped. Does not take the summarize lock, does not run agents, and does not rewrite week pages.

`--outputs PATH` overrides `config.outputs_path`. After a catalog format change, run `render-html` as well if you publish the HTML tree.

### `summarize-week` flags

| Flag | Meaning |
|---|---|
| `--start-week YYYY-MM-DD` | Bootstrap only when no complete weeks exist under `outputs_path` |
| `--week YYYY-MM-DD` | Explicit week end date; wins over auto-resolve |
| `--prepare-only` | Layout / empty stub only; skip LLM agents |

Week folder names are the **calendar end date** `W` (not forced to Sunday / ISO week). The message window is UTC half-open `[W−6 00:00:00, W+1 00:00:00)`. The week must have already ended in UTC.

### Week resolution

1. `--week` wins (already-complete week → exit 0 no-op).
2. Else if any incomplete week dir exists → resume it (exactly one incomplete allowed).
3. Else if ≥1 complete week → auto `W_last_complete + 7`.
4. Else require `--start-week`.

### Resume

- A week without `.complete` is incomplete. Re-run resumes: valid `.thread-order.json` is reused; existing `thread/<stem>.md` files are skipped; only missing threads run; overview runs only when every expected thread file exists.
- To **force re-ordering** while the week is still incomplete, delete `{W}/.thread-order.json` and re-run. Do not delete `.complete` weeks to rewrite published output (v1 never rewrites a completed week).

### Lock

`summarize-week` takes an exclusive non-blocking flock on `{outputs_path}/.summarize-week.lock`. A second process exits non-zero immediately (`summarize_lock_busy=true` in the log). Safe for cron re-entry.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (including empty-week stub, or `--week` already complete) |
| non-zero | Lock held; week not ended; bootstrap error; ordering / thread / overview failure; missing `outputs_path` |

On thread failure the process **continues** through the remaining order, then exits non-zero **without** overview or `.complete`. The error and logs include `failed_thread_ids` and a reason per id (`timeout`, `no submit`, `agent error`).

Empty weeks still write a stub `index.md`, update the root catalog, write `.complete`, and exit 0.

## Output layout

```
{outputs_path}/
  .summarize-week.lock
  index.md                          # root catalog (updated when a week completes, or via regenerate-root-index)
  2026-07-20/
    .complete                       # written last after fsync
    .thread-order.json              # resume state (not published)
    index.md                        # week overview
    thread/<stem>.md                # per-thread summary (lore links for messages)
```

No `messages/` directory. Cleaned bodies stay in SQLite for inference tools.

### HTML layout

When `html_outputs_path` is set (or `render-html` is run), markdown is converted in-process — no static site generator. One shared `style.css` is written at the HTML root. Intra-site links stay **relative** (the tree does not assume it is mounted at `/`) and always name `index.html` explicitly (directory URLs are not assumed to resolve).

```
{html_outputs_path}/
  style.css                         # the one stylesheet
  index.html                        # from index.md
  2026-07-20/
    index.html                      # from 2026-07-20/index.md
    thread/<stem>.html              # from thread/<stem>.md
```

Relative `.md` hrefs become `.html`; lore permalinks are left absolute. Already-complete weeks are not refreshed by `summarize-week`; use `render-html` to backfill.

Each page `<head>` includes a description, Open Graph tags (`og:title`, `og:description`, `og:type`, `og:site_name`), and Twitter Card tags so Slack and similar clients can unfurl the link. Week and thread pages are `og:type` `article` and, when front matter has `week_ending`, also `article:published_time`. `og:url` / canonical are emitted only when `html_site_url` (or an absolute `base_url`) is set; `og:image` only when `html_og_image` is an absolute `http(s)` URL.

## Observability

`RUST_LOG` defaults to `info` (stdout). Useful structured fields:

- `summarize_week`, `summarize_empty_week`, `summarize_lock_busy`
- `summarize_threads_total`, `summarize_threads_skipped`, `summarize_threads_failed`
- `summarize_tokens_prompt`, `summarize_tokens_completion`, `summarize_tokens_total`
- `summarize_tokens_order`, `summarize_tokens_thread`, `summarize_tokens_week`
- `summarize_duration_ms`
- `failed_thread_ids`, `failed_reasons`, `ordering_failed`
- `ordered_root_ids`, `notes` (when ordering completes or a valid order file is reused)

An **indicatif** progress bar on stderr shows threads completed / total during the serial loop.

Cron: treat a missing `{W}/.complete` after the scheduled window as a failed run; re-invoke `summarize-week` to resume.

## Tests

```bash
cargo test
```
