pub mod body;
pub mod escape;
pub mod frontmatter;
pub mod link;
pub mod yaml;

use crate::bead::Bead;
use frontmatter::Context;

/// A generated note's full text.
///
/// Obsidian opens a note with the cursor on the first body line and
/// shows that line as source. The blank line keeps the cursor off the
/// callout.
pub fn note(bead: &Bead, cx: &Context) -> String {
    frontmatter::frontmatter(bead, cx) + "\n" + &body::body(bead, cx)
}
