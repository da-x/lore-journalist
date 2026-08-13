//! Static HTML export from published markdown. Not a static site generator:
//! we convert files ourselves, write one shared stylesheet, and keep every
//! intra-site href relative to the current page.

mod links;
mod markdown;
mod page;
mod render;

pub use render::{html_dir_from_config, maybe_render_html, render_html_tree};
