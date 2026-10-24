//! Markdown and reStructuredText parsed into one document model (see [`model`]), for the preview of the Markdown and
//! reStructuredText modules. Pure text in, a tree out: no UI, no I/O.

pub mod html;
pub mod markdown;
pub mod md_inline;
pub mod model;
pub mod rst;
pub mod rst_inline;

pub use model::{Align, Block, BlockKind, Definition, Document, Inline, ListItem};

/// The markup a preview can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Markup {
    Markdown,
    Rst,
}

impl Markup {
    /// From the `preview` field of a language in a module's manifest.
    pub fn from_name(name: &str) -> Option<Markup> {
        match name.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Some(Markup::Markdown),
            "rst" | "restructuredtext" => Some(Markup::Rst),
            _ => None,
        }
    }

    pub fn parse(self, text: &str) -> Document {
        match self {
            Markup::Markdown => markdown::parse(text),
            Markup::Rst => rst::parse(text),
        }
    }
}
