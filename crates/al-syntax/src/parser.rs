//! Parser infrastructure. The grammar lives in `grammar/`.
//!
//! Every input token, trivia included, is added to the tree exactly once, so
//! the tree's text is the source by construction. Trivia is attached to the
//! innermost node open when the next significant token arrives; nodes start
//! *after* pending trivia, so leading comments belong to the parent.

use crate::preprocessor::preprocess;
use crate::{SyntaxKind, SyntaxNode, Token, lex};
use rowan::{Checkpoint, GreenNode, GreenNodeBuilder};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

pub struct Parse {
    pub green: GreenNode,
    pub errors: Vec<ParseError>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

/// Lex, preprocess with `symbols`, and parse a whole `.al` file.
pub fn parse(src: &str, symbols: &BTreeSet<String>) -> Parse {
    parse_entry(src, symbols, crate::grammar::source_file)
}

/// Parses a standalone type such as a table field's `Code[20]`; `None` if it is
/// not exactly one well-formed type.
pub fn parse_type(src: &str) -> Option<crate::ast::TypeRef> {
    let p = parse_entry(src, &BTreeSet::new(), crate::grammar::type_ref);
    if !p.errors.is_empty() {
        return None;
    }
    p.syntax().children().find_map(crate::ast::TypeRef::cast)
}

pub(crate) fn parse_entry(src: &str, symbols: &BTreeSet<String>, entry: fn(&mut Parser)) -> Parse {
    let pre = preprocess(src, &lex(src), symbols);
    let mut p = Parser::new(src, &pre.tokens);
    p.errors.extend(pre.diagnostics.into_iter().map(|d| ParseError {
        offset: d.offset,
        message: d.message,
    }));
    p.builder.start_node(SyntaxKind::SourceFile.into());
    entry(&mut p);
    // Anything the entry point did not consume is an error, but still part of the tree.
    if !p.at_eof() {
        p.error("unexpected input");
        p.start(SyntaxKind::ErrorNode);
        while !p.at_eof() {
            p.bump_any();
        }
        p.finish();
    }
    p.flush_trivia();
    p.builder.finish_node();
    Parse {
        green: p.builder.finish(),
        errors: p.errors,
    }
}

pub(crate) struct Parser<'a> {
    src: &'a str,
    tokens: Vec<(SyntaxKind, usize, usize)>,
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    pub(crate) errors: Vec<ParseError>,
    /// Guards against grammar loops that do not consume input.
    fuel: std::cell::Cell<u32>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, toks: &[Token]) -> Self {
        let mut tokens = Vec::with_capacity(toks.len());
        let mut off = 0;
        for t in toks {
            tokens.push((t.kind, off, off + t.len as usize));
            off += t.len as usize;
        }
        Parser {
            src,
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
            fuel: 256.into(),
        }
    }

    fn sig(&self, n: usize) -> Option<usize> {
        let mut seen = 0;
        for i in self.pos..self.tokens.len() {
            if !self.tokens[i].0.is_trivia() {
                if seen == n {
                    return Some(i);
                }
                seen += 1;
            }
        }
        None
    }

    /// Kind of the n-th significant token ahead (0 = current).
    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        if self.fuel.get() == 0 {
            panic!("parser made no progress at offset {}", self.offset());
        }
        self.fuel.set(self.fuel.get() - 1);
        self.sig(n).map_or(SyntaxKind::__Last, |i| self.tokens[i].0)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn nth_text(&self, n: usize) -> &'a str {
        self.sig(n)
            .map_or("", |i| &self.src[self.tokens[i].1..self.tokens[i].2])
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    pub(crate) fn at_eof(&self) -> bool {
        self.sig(0).is_none()
    }

    /// Current token is the identifier `kw`, case-insensitively.
    pub(crate) fn at_kw(&self, kw: &str) -> bool {
        self.nth_kw(0, kw)
    }

    pub(crate) fn nth_kw(&self, n: usize, kw: &str) -> bool {
        self.nth(n) == SyntaxKind::Ident && self.nth_text(n).eq_ignore_ascii_case(kw)
    }

    pub(crate) fn offset(&self) -> usize {
        self.sig(0).map_or(self.src.len(), |i| self.tokens[i].1)
    }

    fn flush_trivia(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].0.is_trivia() {
            self.push_raw();
        }
    }

    fn push_raw(&mut self) {
        let (k, s, e) = self.tokens[self.pos];
        self.builder.token(k.into(), &self.src[s..e]);
        self.pos += 1;
    }

    pub(crate) fn start(&mut self, kind: SyntaxKind) {
        self.flush_trivia();
        self.builder.start_node(kind.into());
    }

    pub(crate) fn finish(&mut self) {
        self.builder.finish_node();
    }

    pub(crate) fn checkpoint(&mut self) -> Checkpoint {
        self.flush_trivia();
        self.builder.checkpoint()
    }

    pub(crate) fn start_at(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.builder.start_node_at(cp, kind.into());
    }

    pub(crate) fn bump_any(&mut self) {
        self.flush_trivia();
        if self.pos < self.tokens.len() {
            self.push_raw();
            self.fuel.set(256);
        }
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump_any();
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump_any();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(&format!("expected {kind:?}"));
        false
    }

    pub(crate) fn expect_kw(&mut self, kw: &str) -> bool {
        if self.eat_kw(kw) {
            return true;
        }
        self.error(&format!("expected `{kw}`"));
        false
    }

    pub(crate) fn error(&mut self, msg: &str) {
        let found = self.nth_text(0);
        self.errors.push(ParseError {
            offset: self.offset(),
            message: format!("{msg}, found `{found}`"),
        });
    }

    /// Records an error and wraps the current token in an `ErrorNode`, so the
    /// parser always makes progress.
    pub(crate) fn err_and_bump(&mut self, msg: &str) {
        self.error(msg);
        if !self.at_eof() {
            self.start(SyntaxKind::ErrorNode);
            self.bump_any();
            self.finish();
        }
    }
}

/// Compact S-expression view of a tree for tests: nodes in parentheses,
/// significant tokens by their text, trivia omitted.
pub fn debug_tree(node: &SyntaxNode) -> String {
    let mut s = String::new();
    write_node(node, &mut s);
    s
}

fn write_node(node: &SyntaxNode, s: &mut String) {
    s.push('(');
    s.push_str(&format!("{:?}", node.kind()));
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => {
                s.push(' ');
                write_node(&n, s);
            }
            rowan::NodeOrToken::Token(t) if !t.kind().is_trivia() => {
                s.push(' ');
                s.push_str(t.text());
            }
            rowan::NodeOrToken::Token(_) => {}
        }
    }
    s.push(')');
}
