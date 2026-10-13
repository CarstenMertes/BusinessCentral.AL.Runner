//! Every token and node kind. Keywords are not separate kinds: AL keywords are
//! case-insensitive and mostly contextual, so the parser recognises them from
//! `Ident` text.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum SyntaxKind {
    // Trivia
    Whitespace,
    Newline,
    LineComment,
    BlockComment,
    /// A whole `#if`/`#else`/`#endif`/`#pragma`/... line, without its newline.
    Directive,
    /// UTF-8 byte order mark at the start of a file.
    Bom,

    // Atoms
    Ident,
    QuotedIdent,
    String,
    Integer,
    Decimal,
    /// `20240101D`, `0D`
    DateLit,
    /// `120000T`
    TimeLit,
    /// `0DT`
    DateTimeLit,
    /// `1L`
    BigIntLit,

    // Punctuation
    LParen,
    RParen,
    LBrack,
    RBrack,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    Plus,
    Minus,
    Star,
    Slash,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    /// Ternary `cond ? a : b`.
    Question,
    /// Alternative separator in `filter(A | B)` expressions.
    Pipe,

    /// Anything the lexer cannot classify, including unterminated strings and
    /// comments. The lexer never fails; errors are tokens.
    Error,
}

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::Whitespace
                | SyntaxKind::Newline
                | SyntaxKind::LineComment
                | SyntaxKind::BlockComment
                | SyntaxKind::Directive
                | SyntaxKind::Bom
        )
    }
}
