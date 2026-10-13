//! AL syntax: lexer, preprocessor and lossless parser.

mod grammar;
pub mod lexer;
mod parser;
pub mod preprocessor;
pub mod syntax_kind;

pub use lexer::{Token, lex};
pub use parser::{Parse, ParseError, debug_tree, parse};
pub use syntax_kind::{AlLanguage, SyntaxKind, SyntaxNode, SyntaxToken};
