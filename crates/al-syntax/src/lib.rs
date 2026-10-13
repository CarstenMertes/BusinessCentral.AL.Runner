//! AL syntax: lexer, preprocessor and lossless parser.

pub mod lexer;
pub mod syntax_kind;

pub use lexer::{Token, lex};
pub use syntax_kind::SyntaxKind;
