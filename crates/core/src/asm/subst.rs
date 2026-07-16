//! Substitution-expression evaluator for `.rept` macro expansion
//! (docs/formats.md (assembly text)). Pure and arch-agnostic: a body
//! line's `{expr}` markers are evaluated over the loop variable and
//! replaced with their decimal value before the line is re-parsed.
//!
//! Grammar (left-associative within each level):
//!
//! ```text
//! expr := mul (('+' | '-') mul)*
//! mul  := atom (('*' | '%') atom)*
//! atom := var | integer | '(' expr ')'
//! ```
//!
//! Arithmetic is `i64`. `%` is Rust's remainder; operands are
//! non-negative in practice, and a negative remainder — reachable only
//! when the left operand went negative through subtraction — is rejected.
//! Overflow and a zero modulus are errors, never panics.

/// Evaluate a substitution expression over the loop variable. `var` is
/// the only identifier that resolves (to `value`); any other identifier
/// is an "unknown variable" error. Returns the message on any failure.
pub(crate) fn eval_expr(text: &str, var: &str, value: i64) -> Result<i64, String> {
    let mut eval = Eval {
        chars: text.chars().collect(),
        pos: 0,
        var,
        value,
    };
    let result = eval.expr()?;
    eval.skip_ws();
    if let Some(c) = eval.peek() {
        return Err(format!("unexpected `{c}` in expression `{text}`"));
    }
    Ok(result)
}

/// Replace every `{expr}` occurrence in `text` with its evaluated
/// decimal. An unmatched `{` (no closing `}`) or an eval failure surface
/// as `Err`; a `}` with no opener passes through as literal text.
pub(crate) fn substitute(text: &str, var: &str, value: i64) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("unbalanced `{{` in `{text}`"))?;
        let value = eval_expr(&after[..close], var, value)?;
        out.push_str(&value.to_string());
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A recursive-descent evaluator over the expression's characters.
struct Eval<'a> {
    chars: Vec<char>,
    pos: usize,
    var: &'a str,
    value: i64,
}

impl Eval<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c == ' ' || c == '\t') {
            self.pos += 1;
        }
    }

    fn expr(&mut self) -> Result<i64, String> {
        let mut acc = self.mul()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') => {
                    self.pos += 1;
                    let rhs = self.mul()?;
                    acc = acc
                        .checked_add(rhs)
                        .ok_or_else(|| "arithmetic overflow".to_string())?;
                }
                Some('-') => {
                    self.pos += 1;
                    let rhs = self.mul()?;
                    acc = acc
                        .checked_sub(rhs)
                        .ok_or_else(|| "arithmetic overflow".to_string())?;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn mul(&mut self) -> Result<i64, String> {
        let mut acc = self.atom()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    let rhs = self.atom()?;
                    acc = acc
                        .checked_mul(rhs)
                        .ok_or_else(|| "arithmetic overflow".to_string())?;
                }
                Some('%') => {
                    self.pos += 1;
                    let rhs = self.atom()?;
                    let rem = acc.checked_rem(rhs).ok_or_else(|| {
                        if rhs == 0 {
                            "modulo by zero".to_string()
                        } else {
                            "arithmetic overflow".to_string()
                        }
                    })?;
                    if rem < 0 {
                        return Err("negative modulo result".to_string());
                    }
                    acc = rem;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    fn atom(&mut self) -> Result<i64, String> {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                let inner = self.expr()?;
                self.skip_ws();
                match self.peek() {
                    Some(')') => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => Err("expected `)`".to_string()),
                }
            }
            Some(c) if c.is_ascii_digit() => self.number(),
            Some(c) if c.is_alphabetic() || c == '_' => self.ident(),
            Some(c) => Err(format!("expected a value, found `{c}`")),
            None => Err("expected a value".to_string()),
        }
    }

    fn number(&mut self) -> Result<i64, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<i64>()
            .map_err(|_| format!("integer `{text}` out of range"))
    }

    fn ident(&mut self) -> Result<i64, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            self.pos += 1;
        }
        let name: String = self.chars[start..self.pos].iter().collect();
        if name == self.var {
            Ok(self.value)
        } else {
            Err(format!("unknown variable `{name}`"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_the_utm_expression() {
        assert_eq!(eval_expr("(v+1)%127", "v", 126).unwrap(), 0);
        assert_eq!(eval_expr("(v+1)%127", "v", 5).unwrap(), 6);
        assert_eq!(eval_expr("v", "v", 42).unwrap(), 42);
        assert_eq!(eval_expr("v*2+1", "v", 3).unwrap(), 7);
    }

    #[test]
    fn precedence_binds_star_and_percent_before_plus_and_minus() {
        // `*`/`%` bind tighter than `+`/`-`: 1 + 2*3 = 7, not 9.
        assert_eq!(eval_expr("1+2*3", "v", 0).unwrap(), 7);
        // Parens override: (1+2)*3 = 9.
        assert_eq!(eval_expr("(1+2)*3", "v", 0).unwrap(), 9);
        // Left-associative subtraction: 10 - 3 - 2 = 5, not 9.
        assert_eq!(eval_expr("10-3-2", "v", 0).unwrap(), 5);
    }

    #[test]
    fn whitespace_between_tokens_is_ignored() {
        assert_eq!(eval_expr("  ( v + 1 ) % 4 ", "v", 6).unwrap(), 3);
    }

    #[test]
    fn substitutes_all_occurrences() {
        assert_eq!(
            substitute("Linc{v}: wr {v+1}", "v", 9).unwrap(),
            "Linc9: wr 10"
        );
    }

    #[test]
    fn substitute_without_markers_is_verbatim() {
        assert_eq!(substitute("        nop", "v", 3).unwrap(), "        nop");
    }

    #[test]
    fn errors_are_reported() {
        assert!(eval_expr("v+", "v", 0).is_err()); // dangling operator
        assert!(eval_expr("w", "v", 0).is_err()); // unknown var
        assert!(eval_expr("", "v", 0).is_err()); // empty
        assert!(eval_expr("v v", "v", 0).is_err()); // trailing garbage
        assert!(substitute("{v", "v", 0).is_err()); // unbalanced
        assert!(substitute("{w}", "v", 0).is_err()); // eval failure inside
    }

    #[test]
    fn modulo_zero_and_negative_results_error() {
        assert!(eval_expr("v%0", "v", 5).is_err());
        // 5 - 10 = -5; -5 % 3 is negative in Rust → rejected.
        assert!(eval_expr("(v-10)%3", "v", 5).is_err());
    }

    #[test]
    fn modulo_overflow_is_an_error_not_a_panic() {
        assert!(eval_expr("(0-9223372036854775807-1)%(0-1)", "v", 0).is_err());
        assert!(eval_expr("v%0", "v", 5).is_err());
    }
}
