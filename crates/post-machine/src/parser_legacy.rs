//! Frozen parity oracle for the C1 CST migration
//! (`docs/superpowers/specs/2026-07-07-pmc-fmt-design.md`, "Architecture:
//! one unified lossless CST").
//!
//! This is a near-verbatim snapshot of the pre-C1 recursive-descent
//! parser: `tokens → Program` directly, with no CST intermediary. The
//! live [`crate::parser::parse`] now goes through
//! `lower_cst(parse_cst(tokens)?)`; [`parse_legacy`] preserves the old
//! path so the parity test (`tests/parser_parity.rs`) can assert the two
//! produce byte-identical `Program`s (and accept/reject identically)
//! across the whole corpus and every parser grammar case.
//!
//! It reuses [`crate::parser`]'s AST types verbatim (so `==` type-checks
//! across the two paths) and duplicates only the parsing machinery.
//!
//! **Keep-vs-delete is a review decision — do NOT delete.** Retaining it
//! is a cheap test-only oracle; the final review decides whether the
//! byte-identical compile/lint guarantee makes it redundant.

use std::collections::HashSet;

use mtc_core::diagnostics::Span;

use crate::compiler::{CompileError, CompileErrorKind};
use crate::lexer::{Token, TokenKind};
use crate::parser::{
    Builtin, CheckArm, Function, Import, Item, Label, Program, RESERVED, Statement, Successor,
};

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
        TokenKind::Comment(_) => "a comment".into(),
    }
}

pub fn parse_legacy(tokens: &[Token]) -> Result<Program, CompileError> {
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
    namespaces: HashSet<Vec<String>>,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn bump(&mut self) {
        if !matches!(self.tokens[self.pos].kind, TokenKind::Eof) {
            self.pos += 1;
        }
    }

    fn err_at(t: &Token, kind: CompileErrorKind) -> CompileError {
        CompileError {
            span: t.span(),
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
            if let TokenKind::Ident(w) = &t.kind
                && matches!(w.as_str(), "namespace" | "use" | "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                )
            {
                let kw: &'static str = match w.as_str() {
                    "use" => "use",
                    "export" => "export",
                    _ => "namespace",
                };
                return Err(Self::err_at(&t, CompileErrorKind::KeywordNeedsName(kw)));
            }
            let top_level_stmt = match &t.kind {
                TokenKind::At => true,
                TokenKind::Ident(w) => RESERVED.contains(&w.as_str()),
                _ => false,
            };
            if top_level_stmt {
                return Err(Self::err_at(
                    &t,
                    CompileErrorKind::TopLevelStatement(describe(&t.kind)),
                ));
            }
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "use")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                self.bump();
                loop {
                    let t = self.peek().clone();
                    let TokenKind::Ident(name) = &t.kind else {
                        return Err(Self::expected(&t, "an imported function name"));
                    };
                    if RESERVED.contains(&name.as_str()) {
                        return Err(Self::expected(&t, "an imported function name"));
                    }
                    let mut path = vec![name.clone()];
                    let path_start = t.span().start;
                    let mut path_end = t.span().end;
                    self.bump();
                    while matches!(self.peek().kind, TokenKind::ColonColon) {
                        self.bump();
                        let t = self.peek().clone();
                        let TokenKind::Ident(seg) = &t.kind else {
                            return Err(Self::expected(&t, "a name after `::`"));
                        };
                        if RESERVED.contains(&seg.as_str()) {
                            return Err(Self::err_at(
                                &t,
                                CompileErrorKind::ReservedName {
                                    name: seg.clone(),
                                    what: "path segment",
                                },
                            ));
                        }
                        path.push(seg.clone());
                        path_end = t.span().end;
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
                        span: Span {
                            start: path_start,
                            end: path_end,
                        },
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
                        TokenKind::Colon => {
                            return Err(Self::err_at(&sep, CompileErrorKind::SingleColonInPath));
                        }
                        _ => return Err(Self::expected(&sep, "`,` or `;`")),
                    }
                }
                continue;
            }
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
                        CompileErrorKind::ReservedName {
                            name,
                            what: "namespace",
                        },
                    ));
                }
                if functions.iter().any(|g| g.ns == ns && g.name == name) {
                    return Err(Self::err_at(
                        &name_tok,
                        CompileErrorKind::DuplicateName {
                            name,
                            what: "function",
                        },
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
            f.exported = exported || (ns.is_empty() && f.name == "main");
            if functions.iter().any(|g| g.ns == f.ns && g.name == f.name) {
                return Err(CompileError {
                    span: mtc_core::diagnostics::Span::point(f.line, f.col),
                    kind: CompileErrorKind::DuplicateName {
                        name: f.name,
                        what: "function",
                    },
                });
            }
            let mut as_ns = ns.to_vec();
            as_ns.push(f.name.clone());
            if self.namespaces.contains(&as_ns) {
                return Err(CompileError {
                    span: mtc_core::diagnostics::Span::point(f.line, f.col),
                    kind: CompileErrorKind::DuplicateName {
                        name: f.name,
                        what: "namespace",
                    },
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
                CompileErrorKind::ReservedName {
                    name,
                    what: "function",
                },
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
            if matches!(self.peek().kind, TokenKind::Eof) {
                return Err(Self::expected(
                    self.peek(),
                    "`}` to close the function body",
                ));
            }
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
                        span: mtc_core::diagnostics::Span::point(child.line, child.col),
                        kind: CompileErrorKind::DuplicateName {
                            name: child.name,
                            what: "function",
                        },
                    });
                }
                nested.push(child);
                continue;
            }
            if matches!(&self.peek().kind, TokenKind::Ident(w) if w == "export")
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(_))
                )
            {
                let t = self.peek().clone();
                return Err(Self::err_at(&t, CompileErrorKind::NestedExport));
            }
            let mut labels = Vec::new();
            loop {
                let tok = self.peek().clone();
                let TokenKind::Number(n) = tok.kind else {
                    break;
                };
                self.bump();
                let colon = self.peek().clone();
                self.expect(&TokenKind::Colon, "`:` after a label number")?;
                if !seen_labels.insert(n) {
                    return Err(Self::err_at(&tok, CompileErrorKind::DuplicateLabel(n)));
                }
                labels.push(Label {
                    value: n,
                    span: Span {
                        start: tok.span().start,
                        end: colon.span().end,
                    },
                });
            }
            if matches!(self.peek().kind, TokenKind::RBrace) {
                if let Some(label) = labels.first() {
                    let t = self.peek().clone();
                    return Err(Self::err_at(
                        &t,
                        CompileErrorKind::DanglingLabel(label.value),
                    ));
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
            name_span: name_tok.span(),
            body,
            exported: false,
            local: false,
            nested,
            ns: Vec::new(),
        })
    }

    fn statement(&mut self, labels: Vec<Label>) -> Result<Statement, CompileError> {
        let start = labels
            .first()
            .map(|l| l.span.start)
            .unwrap_or_else(|| self.peek().span().start);
        let line = self.peek().line;
        let mut items = vec![self.item(false)?];
        while matches!(self.peek().kind, TokenKind::Comma) {
            let comma = self.peek().clone();
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
        let semi = self.peek().clone();
        self.expect(&TokenKind::Semi, "`;`")?;
        Ok(Statement {
            labels,
            items,
            line,
            span: Span {
                start,
                end: semi.span().end,
            },
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
                let mut name_end = name_tok.span().end;
                self.bump();
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.bump();
                    let t = self.peek().clone();
                    let TokenKind::Ident(seg) = &t.kind else {
                        return Err(Self::expected(&t, "a name after `::`"));
                    };
                    if RESERVED.contains(&seg.as_str()) {
                        return Err(Self::err_at(
                            &t,
                            CompileErrorKind::ReservedName {
                                name: seg.clone(),
                                what: "path segment",
                            },
                        ));
                    }
                    name.push_str("::");
                    name.push_str(seg);
                    name_end = t.span().end;
                    self.bump();
                }
                if matches!(self.peek().kind, TokenKind::Colon) {
                    let t = self.peek().clone();
                    return Err(Self::err_at(&t, CompileErrorKind::SingleColonInPath));
                }
                let lparen = self.peek().clone();
                self.expect(&TokenKind::LParen, "`(` (user calls are written `@name()`)")?;
                let succ = self.successor()?;
                let rparen = self.peek().clone();
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Item::Call {
                    name,
                    name_span: Span {
                        start: name_tok.span().start,
                        end: name_end,
                    },
                    succ,
                    succ_span: Some(Span {
                        start: lparen.span().start,
                        end: rparen.span().end,
                    }),
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
                    let rparen = self.peek().clone();
                    self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(Item::Check {
                        marked,
                        blank,
                        span: Span {
                            start: tok.span().start,
                            end: rparen.span().end,
                        },
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
                    let (succ, succ_span) = if matches!(self.peek().kind, TokenKind::LParen) {
                        let lparen = self.peek().clone();
                        self.bump();
                        if matches!(self.peek().kind, TokenKind::RParen) {
                            let rparen = self.peek().clone();
                            return Err(CompileError {
                                span: Span {
                                    start: lparen.span().start,
                                    end: rparen.span().end,
                                },
                                kind: CompileErrorKind::EmptyBuiltinParens { name: word.clone() },
                            });
                        }
                        let succ = self.successor()?;
                        let rparen = self.peek().clone();
                        self.expect(&TokenKind::RParen, "`)`")?;
                        (
                            succ,
                            Some(Span {
                                start: lparen.span().start,
                                end: rparen.span().end,
                            }),
                        )
                    } else {
                        (Successor::FallThrough, None)
                    };
                    Ok(Item::Builtin {
                        which,
                        succ,
                        succ_span,
                        line: tok.line,
                    })
                }
                "use" => Err(Self::err_at(&tok, CompileErrorKind::KeywordInBody("use"))),
                "namespace" => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::KeywordInBody("namespace"),
                )),
                other => Err(Self::err_at(
                    &tok,
                    CompileErrorKind::UnknownCommand(other.to_string()),
                )),
            },
            _ => Err(Self::expected(&tok, "a command")),
        }
    }

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
