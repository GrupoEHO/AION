use chumsky::prelude::*;
use std::fmt;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    NodeTag(String),
    HexColor(String),
    AtIdent(String),
    DollarIdent(String),
    Ident(String),
    Str(String),
    Int(i64),
    Float(String),
    Bool(bool),
    Null,
    Let,
    Arrow,
    FatArrow,
    EqEq,
    NotEq,
    Le,
    Ge,
    AndAnd,
    OrOr,
    Colon,
    Comma,
    Dot,
    Eq,
    Question,
    Semi,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Lt,
    Gt,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::NodeTag(s) => write!(f, "{s}"),
            Token::HexColor(s) => write!(f, "{s}"),
            Token::AtIdent(s) => write!(f, "@{s}"),
            Token::DollarIdent(s) => write!(f, "{s}"),
            Token::Ident(s) => write!(f, "{s}"),
            Token::Str(s) => write!(f, "\"{s}\""),
            Token::Int(n) => write!(f, "{n}"),
            Token::Float(s) => write!(f, "{s}"),
            Token::Bool(b) => write!(f, "{b}"),
            Token::Null => write!(f, "null"),
            Token::Let => write!(f, "let"),
            Token::Arrow => write!(f, "->"),
            Token::FatArrow => write!(f, "=>"),
            Token::EqEq => write!(f, "=="),
            Token::NotEq => write!(f, "!="),
            Token::Le => write!(f, "<="),
            Token::Ge => write!(f, ">="),
            Token::AndAnd => write!(f, "&&"),
            Token::OrOr => write!(f, "||"),
            Token::Colon => write!(f, ":"),
            Token::Comma => write!(f, ","),
            Token::Dot => write!(f, "."),
            Token::Eq => write!(f, "="),
            Token::Question => write!(f, "?"),
            Token::Semi => write!(f, ";"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::Lt => write!(f, "<"),
            Token::Gt => write!(f, ">"),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Percent => write!(f, "%"),
            Token::Bang => write!(f, "!"),
        }
    }
}

pub type SpannedToken = (Token, Range<usize>);

pub fn lexer() -> impl Parser<char, Vec<SpannedToken>, Error = Simple<char>> {
    let line_comment = just("//")
        .then(none_of("\n\r").repeated())
        .ignored();

    let block_comment = just("/*")
        .then(take_until(just("*/")))
        .ignored();

    let ws = filter(|c: &char| c.is_whitespace())
        .repeated()
        .at_least(1)
        .ignored();

    let padding = line_comment.or(block_comment).or(ws).repeated();

    let hash_token = just('#')
        .ignore_then(
            filter(|c: &char| c.is_ascii_alphanumeric() || *c == '_')
                .repeated()
                .at_least(1)
                .collect::<String>(),
        )
        .map(|s| {
            if (s.len() == 6 || s.len() == 8 || s.len() == 3)
                && s.chars().all(|c| c.is_ascii_hexdigit())
            {
                Token::HexColor(format!("#{s}"))
            } else {
                Token::NodeTag(format!("#{s}"))
            }
        });

    let ident_chars = || {
        filter(|c: &char| c.is_ascii_alphabetic() || *c == '_')
            .chain::<char, _, _>(
                filter(|c: &char| c.is_ascii_alphanumeric() || *c == '_').repeated(),
            )
            .collect::<String>()
    };

    let at_ident = just('@').ignore_then(ident_chars()).map(Token::AtIdent);

    let dollar_ident = just('$')
        .ignore_then(ident_chars())
        .map(|s| Token::DollarIdent(format!("${s}")));

    let escape = just('\\').ignore_then(choice((
        just('\\').to('\\'),
        just('"').to('"'),
        just('n').to('\n'),
        just('t').to('\t'),
        just('r').to('\r'),
    )));

    let string_lit = just('"')
        .ignore_then(escape.or(none_of("\\\"")).repeated().collect::<String>())
        .then_ignore(just('"'))
        .map(Token::Str);

    let float_lit = text::int(10)
        .then_ignore(just('.'))
        .then(text::digits(10))
        .map(|(a, b)| Token::Float(format!("{a}.{b}")));

    let int_lit = text::int(10)
        .try_map(|s: String, span| {
            s.parse::<i64>()
                .map(Token::Int)
                .map_err(|e| Simple::custom(span, format!("invalid integer `{s}`: {e}")))
        });

    let ident_or_kw = ident_chars().map(|s| match s.as_str() {
        "true" => Token::Bool(true),
        "false" => Token::Bool(false),
        "null" => Token::Null,
        "let" => Token::Let,
        _ => Token::Ident(s),
    });

    let multi_op = choice((
        just("->").to(Token::Arrow),
        just("=>").to(Token::FatArrow),
        just("==").to(Token::EqEq),
        just("!=").to(Token::NotEq),
        just("<=").to(Token::Le),
        just(">=").to(Token::Ge),
        just("&&").to(Token::AndAnd),
        just("||").to(Token::OrOr),
    ));

    let single_op = choice((
        just(':').to(Token::Colon),
        just(',').to(Token::Comma),
        just('.').to(Token::Dot),
        just('=').to(Token::Eq),
        just('?').to(Token::Question),
        just(';').to(Token::Semi),
        just('{').to(Token::LBrace),
        just('}').to(Token::RBrace),
        just('[').to(Token::LBracket),
        just(']').to(Token::RBracket),
        just('(').to(Token::LParen),
        just(')').to(Token::RParen),
        just('<').to(Token::Lt),
        just('>').to(Token::Gt),
        just('+').to(Token::Plus),
        just('-').to(Token::Minus),
        just('*').to(Token::Star),
        just('/').to(Token::Slash),
        just('%').to(Token::Percent),
        just('!').to(Token::Bang),
    ));

    let token = choice((
        hash_token,
        at_ident,
        dollar_ident,
        string_lit,
        float_lit,
        int_lit,
        ident_or_kw,
        multi_op,
        single_op,
    ));

    padding
        .clone()
        .ignore_then(token.map_with_span(|tok, span| (tok, span)).then_ignore(padding).repeated())
        .then_ignore(end())
}
