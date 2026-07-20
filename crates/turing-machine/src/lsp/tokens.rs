//! Semantic tokens.
//!
//! Emitted from the significant token stream rather than the CST, for the
//! same reason classification is: highlighting must not switch off the
//! moment a brace is unbalanced. Each identifier takes its type from the
//! keyword or punctuation immediately around it, which is enough to
//! separate the six legend types the service advertises without needing a
//! parse to have succeeded.
//!
//! Literals need no context at all: a glyph is a string, a number is a
//! number. Reserved words are deliberately NOT emitted — keyword colouring
//! is the editor grammar's job, and emitting it here would fight it.

use mtc_core::lsp::SemToken;

use super::{
    DocState, MODIFIER_DECLARATION, TOKEN_TYPE_FUNCTION, TOKEN_TYPE_NAMESPACE, TOKEN_TYPE_NUMBER,
    TOKEN_TYPE_STRING, TOKEN_TYPE_TYPE, TOKEN_TYPE_VARIABLE, significant,
};
use crate::lexer::{RESERVED, Token, TokenKind};

pub(super) fn semantic_tokens(state: &DocState) -> Option<Vec<SemToken>> {
    let sig = significant(state.tokens.as_ref()?);
    let mut out = Vec::new();
    for (i, token) in sig.iter().enumerate() {
        let Some((token_type, modifiers)) = classify(&sig, i, token) else {
            continue;
        };
        out.push(SemToken {
            span: token.span(),
            token_type,
            modifiers,
        });
    }
    Some(out)
}

fn ident_at(sig: &[Token], i: usize) -> Option<&str> {
    match &sig.get(i)?.kind {
        TokenKind::Ident(s) => Some(s.as_str()),
        _ => None,
    }
}

fn classify(sig: &[Token], i: usize, token: &Token) -> Option<(u32, u32)> {
    match &token.kind {
        TokenKind::Glyph(_) => Some((TOKEN_TYPE_STRING, 0)),
        TokenKind::Number(..) => Some((TOKEN_TYPE_NUMBER, 0)),
        TokenKind::Ident(word) => {
            if RESERVED.contains(&word.as_str()) {
                return None;
            }
            // A `::` chain: every segment but the last names a namespace.
            if matches!(sig.get(i + 1).map(|t| &t.kind), Some(TokenKind::ColonColon)) {
                return Some((TOKEN_TYPE_NAMESPACE, 0));
            }
            let prev = i.checked_sub(1);
            if matches!(prev.map(|j| &sig[j].kind), Some(TokenKind::ColonColon)) {
                return Some((TOKEN_TYPE_FUNCTION, 0));
            }
            // `tape NAME : ALPHABET` — the slot after the colon is a type.
            if matches!(prev.map(|j| &sig[j].kind), Some(TokenKind::Colon)) {
                return Some((TOKEN_TYPE_TYPE, 0));
            }
            match prev.and_then(|j| ident_at(sig, j)) {
                Some("namespace") => Some((TOKEN_TYPE_NAMESPACE, MODIFIER_DECLARATION)),
                Some("alphabet") => Some((TOKEN_TYPE_TYPE, MODIFIER_DECLARATION)),
                Some("routine") | Some("graph") => {
                    Some((TOKEN_TYPE_FUNCTION, MODIFIER_DECLARATION))
                }
                Some("state") | Some("tape") | Some("as") => {
                    Some((TOKEN_TYPE_VARIABLE, MODIFIER_DECLARATION))
                }
                Some("goto") | Some("then") | Some("call") | Some("graft") | Some("bind") => {
                    Some((TOKEN_TYPE_FUNCTION, 0))
                }
                _ => Some((TOKEN_TYPE_VARIABLE, 0)),
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use mtc_core::lsp::LanguageService;

    use super::super::TmcLanguageService;
    use super::*;

    /// The legend arrays and the index constants are two spellings of one
    /// fact; this is the guard that keeps them one fact.
    #[test]
    fn legend_indices_match_the_advertised_legend() {
        let service = TmcLanguageService::new();
        let (types, modifiers) = service.token_legend();
        assert_eq!(types[TOKEN_TYPE_NAMESPACE as usize], "namespace");
        assert_eq!(types[TOKEN_TYPE_TYPE as usize], "type");
        assert_eq!(types[TOKEN_TYPE_FUNCTION as usize], "function");
        assert_eq!(types[TOKEN_TYPE_VARIABLE as usize], "variable");
        assert_eq!(types[TOKEN_TYPE_STRING as usize], "string");
        assert_eq!(types[TOKEN_TYPE_NUMBER as usize], "number");
        assert_eq!(types.len(), 6);
        assert_eq!(modifiers, ["declaration"]);
        assert_eq!(MODIFIER_DECLARATION, 1);
    }
}
