//! Text for Mermaid flowcharts, shared by every `ir graph` renderer.

use alloc::string::String;

/// An edge label's text with the three characters Mermaid would not keep
/// written as Mermaid's own entity codes: `<` as `#lt;`, `>` as `#gt;` and
/// `&` as `#amp;`. Mermaid's parse-stage sanitizer treats a `<` as the
/// start of markup and empties the label around it (`m[<]` would draw as
/// nothing), while its entity codes are decoded when the label is drawn,
/// in every label mode (docs/formats.md (graph label notation)).
pub fn edge_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '&' => out.push_str("#amp;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutation: return the text unchanged.
    #[test]
    fn the_three_characters_become_entity_codes() {
        assert_eq!(edge_label("m[<,>] a&b"), "m[#lt;,#gt;] a#amp;b");
        assert_eq!(edge_label("[{1–3},*×2] exit #0"), "[{1–3},*×2] exit #0");
    }
}
