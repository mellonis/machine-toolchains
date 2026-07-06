//! `.pmc` recursive-descent parser (docs/language.md): tokens → AST.

use std::collections::HashSet;

use crate::compiler::{CompileError, CompileErrorKind};
use crate::lexer::{Token, TokenKind};

/// docs/language.md: words that cannot name a function.
pub const RESERVED: [&str; 8] = [
    "goto", "check", "left", "right", "mark", "unmark", "halt", "debugger",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub imports: Vec<Import>,
}

/// One `use` list item: `use a, std::b as c;` yields two of these.
/// Every import declares an external symbol by its FULL `::`-joined
/// path and binds ONE bare name in its declaring scope (alias if
/// present, else the path tail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// `IDENT (:: IDENT)*` — `use std::goToEnd;` → `["std", "goToEnd"]`.
    pub path: Vec<String>,
    /// `as NAME` rebinds the bare name (the declared symbol is unchanged).
    pub alias: Option<String>,
    pub line: u32,
    /// The declaring namespace block's path; empty = file level. The
    /// binding is visible in that block and nested scopes only.
    pub ns: Vec<String>,
}

impl Import {
    /// The bare name this import binds in its scope.
    pub fn binding(&self) -> &str {
        self.alias.as_deref().unwrap_or_else(|| {
            self.path
                .last()
                .expect("parser: import paths are non-empty")
        })
    }

    /// The full `::`-joined external symbol this import declares.
    pub fn full_path(&self) -> String {
        self.path.join("::")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub line: u32,
    pub col: u32,
    pub body: Vec<Statement>,
    /// `export` (contextual keyword) or `main` (always exported).
    pub exported: bool,
    /// Nesting is always local; flatten computes this for top-level
    /// functions as `!exported`.
    pub local: bool,
    /// Nested function definitions (docs/language.md (visibility)), hoisted and visible to
    /// their own siblings and enclosing scope's body; emptied by flatten.
    pub nested: Vec<Function>,
    /// Enclosing namespace path (parser-set on top-level definitions;
    /// nested functions inherit through their top-level ancestor). The
    /// full symbol joins namespaces with `::` and nesting with `.` —
    /// `std::api.helper`.
    pub ns: Vec<String>,
}

/// One `;`-terminated statement: an optional run of labels, then one or
/// more comma-separated items. `items.len() > 1` only for comma groups,
/// whose position rules the parser has enforced: `check`/`halt` only
/// last, a successor only on the last item, `goto` never grouped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub labels: Vec<u32>,
    pub items: Vec<Item>,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Left,
    Right,
    Mark,
    Unmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Successor {
    FallThrough,
    Label(u32),
    Return,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckArm {
    Label(u32),
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Builtin {
        which: Builtin,
        succ: Successor,
        line: u32,
    },
    Debugger {
        line: u32,
    },
    Call {
        name: String,
        succ: Successor,
        line: u32,
    },
    Check {
        marked: CheckArm,
        blank: CheckArm,
        line: u32,
    },
    Halt {
        line: u32,
    },
    Goto {
        label: u32,
        line: u32,
    },
}

fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(n) => format!("`{n}`"),
        TokenKind::Number(v) => format!("`{v}`"),
        TokenKind::At => "`@`".into(),
        TokenKind::Bang => "`!`".into(),
        TokenKind::Comma => "`,`".into(),
        TokenKind::Semi => "`;`".into(),
        TokenKind::Colon => "`:`".into(),
        TokenKind::ColonColon => "`::`".into(),
        TokenKind::LParen => "`(`".into(),
        TokenKind::RParen => "`)`".into(),
        TokenKind::LBrace => "`{`".into(),
        TokenKind::RBrace => "`}`".into(),
        TokenKind::Eof => "end of file".into(),
    }
}

pub fn parse(tokens: &[Token]) -> Result<Program, CompileError> {
    Parser {
        tokens,
        pos: 0,
        namespaces: HashSet::new(),
    }
    .program()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Every namespace path declared so far (reopened blocks insert the
    /// same path again, harmlessly). Namespace names share the name pool
    /// with function names per scope — a human-clarity rule: since `::`
    /// (namespaces) and `.` (nesting) are distinct separators, `a::x`
    /// and `a.x` cannot collide; the pool rule just stops both spellings
    /// coexisting confusingly in one file.
    namespaces: HashSet<Vec<String>>,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        // Safe: the lexer always appends Eof and bump() never passes it.
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            self.pos += 1;
        }
    }

    fn err_at(t: &Token, kind: CompileErrorKind) -> CompileError {
        CompileError {
            line: t.line,
            col: t.col,
            kind,
        }
    }

    fn expected(t: &Token, what: &'static str) -> CompileError {
        Self::err_at(
            t,
            CompileErrorKind::Expected {
                what,
                found: describe(&t.kind),
            },
        )
    }

    fn expect(&mut self, kind: &TokenKind, what: &'static str) -> Result<(), CompileError> {
        if &self.peek().kind == kind {
            self.bump();
            Ok(())
        } else {
            Err(Self::expected(self.peek(), what))
        }
    }

    fn program(mut self) -> Result<Program, CompileError> {
        let mut functions: Vec<Function> = Vec::new();
        let mut imports = Vec::new();
        self.top_items(&[], &mut functions, &mut imports, None)?;
        Ok(Program { functions, imports })
    }

    /// One namespace level's item loop; the whole file is the `ns == []`
    /// level. Handles `use` (legal at any namespace depth, never in
    /// function bodies), `namespace NAME { … }` (contextual; recurse
    /// with the extended path), `export`, and function definitions.
    /// `terminator` is `Some(RBrace)` inside a block, `None` at file
    /// level (ends at Eof).
    fn top_items(
        &mut self,
        ns: &[String],
        functions: &mut Vec<Function>,
        imports: &mut Vec<Import>,
        terminator: Option<&TokenKind>,
    ) -> Result<(), CompileError> {
        loop {
            let t = self.peek().clone();
            match (&t.kind, terminator) {
                (TokenKind::Eof, None) => return Ok(()),
                (TokenKind::Eof, Some(_)) => {
                    return Err(Self::expected(&t, "`}` to close the namespace block"));
                }
                (k, Some(term)) if k == term => {
                    self.bump();
                    return Ok(());
                }
                _ => {}
            }
            // Contextual keyword: `use` + identifier = import declaration;
            // `use` + `(` is a function NAMED use.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "use")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                self.bump();
                loop {
                    // path := IDENT (`::` IDENT)*  [ `as` IDENT ]
                    let t = self.peek().clone();
                    let TokenKind::Ident(name) = &t.kind else {
                        return Err(Self::expected(&t, "an imported function name"));
                    };
                    if RESERVED.contains(&name.as_str()) {
                        return Err(Self::expected(&t, "an imported function name"));
                    }
                    let mut path = vec![name.clone()];
                    self.bump();
                    while matches!(self.peek().kind, TokenKind::ColonColon) {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(seg) = &t.kind else {
                            return Err(Self::expected(&t, "a name after `::`"));
                        };
                        path.push(seg.clone());
                        self.bump();
                    }
                    let alias = if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "as") {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(a) = &t.kind else {
                            return Err(Self::expected(&t, "an alias after `as`"));
                        };
                        self.bump();
                        Some(a.clone())
                    } else {
                        None
                    };
                    imports.push(Import {
                        path,
                        alias,
                        line: t.line,
                        ns: ns.to_vec(),
                    });
                    let sep = self.peek().clone();
                    match sep.kind {
                        TokenKind::Comma => {
                            self.bump();
                        }
                        TokenKind::Semi => {
                            self.bump();
                            break;
                        }
                        _ => return Err(Self::expected(&sep, "`,` or `;`")),
                    }
                }
                continue;
            }
            // Contextual keyword: `namespace NAME {` opens a (reopenable)
            // block; `namespace` + `(` stays a function NAMED namespace.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "namespace")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
                && matches!(
                    self.tokens.get(self.pos + 2).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                )
            {
                self.bump(); // `namespace`
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    unreachable!("checked above");
                };
                let name = name.clone();
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::ReservedFunctionName(name),
                    ));
                }
                // Shared name pool: a namespace may not reuse a sibling
                // function's name (reopening the same namespace is fine).
                if functions.iter().any(|g| g.ns == ns && g.name == name) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::DuplicateFunction(name),
                    ));
                }
                self.bump(); // the name
                self.bump(); // `{`
                let mut child = ns.to_vec();
                child.push(name);
                self.namespaces.insert(child.clone());
                self.top_items(&child, functions, imports, Some(&TokenKind::RBrace))?;
                continue;
            }
            // Contextual keyword: `export` + identifier = exported def;
            // `export` + `(` is a function NAMED export.
            let exported = if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                ) {
                self.bump();
                true
            } else {
                false
            };
            let mut f = self.function()?;
            f.ns = ns.to_vec();
            // Only the un-namespaced top-level `main` auto-exports (and is
            // the entry); a namespaced `main` is an ordinary function.
            f.exported = exported || (ns.is_empty() && f.name == "main");
            if functions.iter().any(|g| g.ns == f.ns && g.name == f.name) {
                return Err(CompileError {
                    line: f.line,
                    col: f.col,
                    kind: CompileErrorKind::DuplicateFunction(f.name),
                });
            }
            // Shared name pool: a function may not reuse a sibling
            // namespace's name.
            let mut as_ns = ns.to_vec();
            as_ns.push(f.name.clone());
            if self.namespaces.contains(&as_ns) {
                return Err(CompileError {
                    line: f.line,
                    col: f.col,
                    kind: CompileErrorKind::DuplicateFunction(f.name),
                });
            }
            functions.push(f);
        }
    }

    fn function(&mut self) -> Result<Function, CompileError> {
        let name_tok = self.peek().clone();
        let TokenKind::Ident(name) = &name_tok.kind else {
            return Err(Self::expected(&name_tok, "a function name"));
        };
        let name = name.clone();
        if RESERVED.contains(&name.as_str()) {
            return Err(Self::err_at(
                &name_tok,
                CompileErrorKind::ReservedFunctionName(name),
            ));
        }
        self.bump();
        self.expect(&TokenKind::LParen, "`(` after the function name")?;
        self.expect(&TokenKind::RParen, "`)` (functions take no parameters)")?;
        self.expect(&TokenKind::LBrace, "`{`")?;

        let mut body = Vec::new();
        let mut nested = Vec::new();
        let mut seen_labels: HashSet<u32> = HashSet::new();
        loop {
            // Nested definition: IDENT ( ) {  — visibility-only nesting.
            let is_nested_def = matches!(&self.peek().kind, TokenKind::Ident(w)
                    if !RESERVED.contains(&w.as_str()))
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::LParen)
                )
                && matches!(
                    self.tokens.get(self.pos + 2).map(|t| &t.kind),
                    Some(TokenKind::RParen)
                )
                && matches!(
                    self.tokens.get(self.pos + 3).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                );
            if is_nested_def {
                let child = self.function()?;
                if nested.iter().any(|g: &Function| g.name == child.name) {
                    return Err(CompileError {
                        line: child.line,
                        col: child.col,
                        kind: CompileErrorKind::DuplicateFunction(child.name),
                    });
                }
                nested.push(child);
                continue;
            }
            // `export` before a nested definition is an error.
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                let t = self.peek();
                return Err(CompileError {
                    line: t.line,
                    col: t.col,
                    kind: CompileErrorKind::NestedExport,
                });
            }
            // Labels announced before the next statement (possibly stacked).
            let mut labels = Vec::new();
            loop {
                let tok = self.peek().clone();
                let TokenKind::Number(n) = tok.kind else {
                    break;
                };
                self.bump();
                self.expect(&TokenKind::Colon, "`:` after a label number")?;
                if !seen_labels.insert(n) {
                    return Err(Self::err_at(&tok, CompileErrorKind::DuplicateLabel(n)));
                }
                labels.push(n);
            }
            if matches!(self.peek().kind, TokenKind::RBrace) {
                if let Some(&label) = labels.first() {
                    let t = self.peek();
                    return Err(CompileError {
                        line: t.line,
                        col: t.col,
                        kind: CompileErrorKind::DanglingLabel(label),
                    });
                }
                self.bump();
                break;
            }
            body.push(self.statement(labels)?);
        }
        Ok(Function {
            name,
            line: name_tok.line,
            col: name_tok.col,
            body,
            exported: false,
            local: false,
            nested,
            ns: Vec::new(),
        })
    }

    fn statement(&mut self, labels: Vec<u32>) -> Result<Statement, CompileError> {
        let line = self.peek().line;
        let mut items = vec![self.item(false)?];
        while matches!(self.peek().kind, TokenKind::Comma) {
            let comma = self.peek().clone();
            // Whatever precedes a `,` must be bare (docs/language.md).
            match items.last().expect("items is never empty") {
                Item::Check { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "check must be the last command in a comma group",
                        ),
                    ));
                }
                Item::Halt { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "halt must be the last command in a comma group",
                        ),
                    ));
                }
                Item::Goto { .. } => {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition("goto cannot appear in a comma group"),
                    ));
                }
                Item::Builtin { succ, .. } | Item::Call { succ, .. }
                    if *succ != Successor::FallThrough =>
                {
                    return Err(Self::err_at(
                        &comma,
                        CompileErrorKind::GroupPosition(
                            "only the last command in a comma group may take a successor",
                        ),
                    ));
                }
                _ => {}
            }
            self.bump();
            items.push(self.item(true)?);
        }
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(Statement {
            labels,
            items,
            line,
        })
    }

    fn item(&mut self, in_group: bool) -> Result<Item, CompileError> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::At => {
                self.bump();
                let name_tok = self.peek().clone();
                let TokenKind::Ident(name) = &name_tok.kind else {
                    return Err(Self::expected(&name_tok, "a function name after `@`"));
                };
                let mut name = name.clone();
                if RESERVED.contains(&name.as_str()) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::BuiltinCalled(name),
                    ));
                }
                self.bump();
                // Qualified call: `@ns::path::f()` — ABSOLUTE (flatten
                // skips the scope chain), `::` segments only (nested
                // functions stay unnameable — the grammar has no `.`).
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(seg) = &t.kind else {
                        return Err(Self::expected(&t, "a name after `::`"));
                    };
                    name.push_str("::");
                    name.push_str(seg);
                    self.bump();
                }
                self.expect(&TokenKind::LParen, "`(` (user calls are written `@name()`)")?;
                let succ = self.successor()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Item::Call {
                    name,
                    succ,
                    line: tok.line,
                })
            }
            TokenKind::Ident(word) => match word.as_str() {
                "goto" => {
                    if in_group {
                        return Err(Self::err_at(
                            &tok,
                            CompileErrorKind::GroupPosition("goto cannot appear in a comma group"),
                        ));
                    }
                    self.bump();
                    let target = self.peek().clone();
                    match target.kind {
                        TokenKind::Number(n) => {
                            self.bump();
                            Ok(Item::Goto {
                                label: n,
                                line: tok.line,
                            })
                        }
                        TokenKind::Bang => Err(Self::err_at(&target, CompileErrorKind::GotoReturn)),
                        _ => Err(Self::expected(&target, "a numeric label after `goto`")),
                    }
                }
                "check" => {
                    self.bump();
                    self.expect(&TokenKind::LParen, "`(` after `check`")?;
                    let marked = self.check_arm()?;
                    self.expect(&TokenKind::Comma, "`,` between check arms")?;
                    let blank = self.check_arm()?;
                    self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(Item::Check {
                        marked,
                        blank,
                        line: tok.line,
                    })
                }
                "halt" => {
                    self.bump();
                    Ok(Item::Halt { line: tok.line })
                }
                "debugger" => {
                    self.bump();
                    Ok(Item::Debugger { line: tok.line })
                }
                "left" | "right" | "mark" | "unmark" => {
                    let which = match word.as_str() {
                        "left" => Builtin::Left,
                        "right" => Builtin::Right,
                        "mark" => Builtin::Mark,
                        _ => Builtin::Unmark,
                    };
                    self.bump();
                    let succ = if matches!(self.peek().kind, TokenKind::LParen) {
                        self.bump();
                        let succ = self.successor()?;
                        self.expect(&TokenKind::RParen, "`)`")?;
                        succ
                    } else {
                        Successor::FallThrough
                    };
                    Ok(Item::Builtin {
                        which,
                        succ,
                        line: tok.line,
                    })
                }
                other => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::UnknownCommand(other.to_string()),
                )),
            },
            _ => Err(Self::expected(&tok, "a command")),
        }
    }

    /// Inside `( … )`: empty → fall through, `N` → label, `!` → return.
    fn successor(&mut self) -> Result<Successor, CompileError> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Number(n) => {
                self.bump();
                Ok(Successor::Label(n))
            }
            TokenKind::Bang => {
                self.bump();
                Ok(Successor::Return)
            }
            _ => Ok(Successor::FallThrough), // the caller checks the `)`
        }
    }

    fn check_arm(&mut self) -> Result<CheckArm, CompileError> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Number(n) => {
                self.bump();
                Ok(CheckArm::Label(n))
            }
            TokenKind::Bang => {
                self.bump();
                Ok(CheckArm::Return)
            }
            _ => Err(Self::expected(&t, "a label number or `!`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompileErrorKind;
    use crate::lexer::lex;

    fn parse_src(src: &str) -> Result<Program, CompileError> {
        parse(&lex(src).unwrap())
    }

    #[test]
    fn parses_the_spec_sample() {
        let src = r#"
// Move right until the first blank cell.
goToEnd() {
1:  right;
    check(1, 2);      // cell marked -> goto 1, blank -> goto 2
2:  left;             // last command - implicit return
}

goToBegin() {
1:  left(2);
2:  check(1, 3);
3:  right(!);
}

main() {
    @goToEnd();
    right;
    check(3, 4);
3:  unmark(!);
4:  mark;
}
"#;
        let p = parse_src(src).unwrap();
        assert_eq!(
            p.functions
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["goToEnd", "goToBegin", "main"]
        );
        let main = &p.functions[2];
        assert_eq!(main.body.len(), 5);
        assert_eq!(
            main.body[0].items,
            vec![Item::Call {
                name: "goToEnd".into(),
                succ: Successor::FallThrough,
                line: main.body[0].line
            }]
        );
        assert_eq!(main.body[3].labels, vec![3]);
        assert_eq!(
            main.body[3].items,
            vec![Item::Builtin {
                which: Builtin::Unmark,
                succ: Successor::Return,
                line: main.body[3].line
            }]
        );
        match &main.body[2].items[0] {
            Item::Check {
                marked: CheckArm::Label(3),
                blank: CheckArm::Label(4),
                ..
            } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn comma_groups_parse_and_enforce_positions() {
        let p = parse_src("f() { 1: right, right, mark(5); 5: left, check(1, !); }").unwrap();
        assert_eq!(p.functions[0].body[0].items.len(), 3);
        assert_eq!(p.functions[0].body[1].items.len(), 2);

        let e = parse_src("f() { left(1), left(2); 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("successor")));

        let e = parse_src("f() { check(1, 2), left; 1: mark; 2: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("check")));

        let e = parse_src("f() { halt, left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("halt")));

        let e = parse_src("f() { goto 1, left; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
        let e = parse_src("f() { left, goto 1; 1: mark; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GroupPosition(m) if m.contains("goto")));
    }

    #[test]
    fn reserved_and_at_rules() {
        let e = parse_src("check() { }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::ReservedFunctionName(n) if n == "check"));

        let e = parse_src("f() { @left(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::BuiltinCalled(n) if n == "left"));

        let e = parse_src("f() { flip; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "flip"));

        // A user function called without `@` is the same error (docs/language.md).
        let e = parse_src("f() { goToEnd(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "goToEnd"));
    }

    #[test]
    fn goto_bang_is_a_dedicated_error() {
        let e = parse_src("f() { goto !; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::GotoReturn));
    }

    #[test]
    fn duplicate_and_dangling_diagnostics() {
        let e = parse_src("f() { } f() { }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "f"));

        let e = parse_src("f() { 1: left; 1: right; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateLabel(1)));

        let e = parse_src("f() { left; 2: }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DanglingLabel(2)));
    }

    #[test]
    fn empty_function_and_stacked_labels() {
        let p = parse_src("f() { }").unwrap();
        assert!(p.functions[0].body.is_empty());

        let p = parse_src("f() { 1: 2: left; }").unwrap();
        assert_eq!(p.functions[0].body[0].labels, vec![1, 2]);
    }

    #[test]
    fn unicode_function_names_and_calls() {
        let p = parse_src("идиВКонец() { right(!); } main() { @идиВКонец(); }").unwrap();
        assert_eq!(p.functions[0].name, "идиВКонец");
        match &p.functions[1].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "идиВКонец"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn export_is_contextual_and_main_auto_exports() {
        let p = parse_src("export api() { left; } helper() { right; } main() { mark; }").unwrap();
        assert!(p.functions[0].exported);
        assert!(!p.functions[1].exported);
        assert!(p.functions[2].exported); // main
        let p = parse_src("export() { left; } main() { @export(); }").unwrap();
        assert_eq!(p.functions[0].name, "export"); // a function NAMED export
    }

    #[test]
    fn nested_definitions_parse_recursively() {
        let p = parse_src("main() { walk() { step() { right; } @step(); } @walk(); }").unwrap();
        let main = &p.functions[0];
        assert_eq!(main.nested.len(), 1);
        assert_eq!(main.nested[0].name, "walk");
        assert_eq!(main.nested[0].nested[0].name, "step");
    }

    #[test]
    fn namespace_blocks_stamp_paths_and_nest() {
        let p =
            parse_src("namespace a { f() { left; } namespace b { g() { right; } } } h() { mark; }")
                .unwrap();
        let tagged: Vec<(&str, Vec<&str>)> = p
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.ns.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(
            tagged,
            vec![("f", vec!["a"]), ("g", vec!["a", "b"]), ("h", vec![])]
        );
        // `namespace` + `(` stays a function NAMED namespace.
        let p = parse_src("namespace() { left; } main() { @namespace(); }").unwrap();
        assert_eq!(p.functions[0].name, "namespace");
    }

    #[test]
    fn import_paths_aliases_and_scopes_parse() {
        let p = parse_src("use a, std::b as c; namespace ns { use d::e; }").unwrap();
        assert_eq!(p.imports.len(), 3);
        assert_eq!(p.imports[0].path, vec!["a"]);
        assert_eq!(p.imports[0].alias, None);
        assert_eq!(p.imports[0].binding(), "a");
        assert!(p.imports[0].ns.is_empty());
        assert_eq!(p.imports[1].path, vec!["std", "b"]);
        assert_eq!(p.imports[1].alias.as_deref(), Some("c"));
        assert_eq!(p.imports[1].binding(), "c");
        assert_eq!(p.imports[1].full_path(), "std::b");
        assert_eq!(p.imports[2].path, vec!["d", "e"]);
        assert_eq!(p.imports[2].ns, vec!["ns"]);
    }

    #[test]
    fn qualified_calls_parse_to_joined_names() {
        let p = parse_src("main() { @std::api::run(); }").unwrap();
        match &p.functions[0].body[0].items[0] {
            Item::Call { name, .. } => assert_eq!(name, "std::api::run"),
            other => panic!("unexpected {other:?}"),
        }
        let e = parse_src("main() { @std::(); }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Expected { what, .. } if what.contains("::")));
    }

    #[test]
    fn namespace_name_pool_and_reopening_rules() {
        // Reopening the same namespace is legal (scopes merge by path).
        assert!(parse_src("namespace a { f() { left; } } namespace a { g() { right; } }").is_ok());
        // Same (path, name) across reopened blocks is a duplicate.
        let e =
            parse_src("namespace a { f() { left; } } namespace a { f() { right; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "f"));
        // The same bare name in different namespaces is legal.
        assert!(parse_src("namespace a { f() { left; } } namespace b { f() { right; } }").is_ok());
        // Namespace and function names share one pool per scope.
        let e = parse_src("namespace a { } a() { left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "a"));
        let e = parse_src("a() { left; } namespace a { }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "a"));
        // An unclosed block is an error, not silent Eof acceptance.
        let e = parse_src("namespace a { f() { left; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::Expected { .. }));
    }

    #[test]
    fn use_stays_illegal_inside_function_bodies() {
        let e = parse_src("main() { use go; }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::UnknownCommand(n) if n == "use"));
    }

    #[test]
    fn nested_export_and_same_scope_duplicates_error() {
        let e = parse_src("main() { export inner() { left; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::NestedExport));
        let e = parse_src("main() { f() { left; } f() { right; } }").unwrap_err();
        assert!(matches!(e.kind, CompileErrorKind::DuplicateFunction(n) if n == "f"));
    }
}
