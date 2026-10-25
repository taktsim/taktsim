//! Splits the source into tokens (ADR-0015, "Lexical rules").

use taktsim_units::{Count, Rate, Ratio, SimDuration};

use super::ast::{CmpOp, KEYWORDS, Quantity, is_ident_char, is_ident_start};
use super::error::{ErrorKind, ExprError, Hint, UNITS, suggest};
use super::span::Span;

/// A reserved word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keyword {
    And,
    Or,
    Not,
    In,
    True,
    False,
}

impl Keyword {
    pub(crate) fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "and" => Self::And,
            "or" => Self::Or,
            "not" => Self::Not,
            "in" => Self::In,
            "true" => Self::True,
            "false" => Self::False,
            _ => return None,
        })
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
            Self::Not => "not",
            Self::In => "in",
            Self::True => "true",
            Self::False => "false",
        }
    }
}

/// A keyword written in another case, e.g. `AND`.
pub(crate) fn miscased_keyword(word: &str) -> Option<&'static str> {
    KEYWORDS
        .iter()
        .copied()
        .find(|k| k.eq_ignore_ascii_case(word) && *k != word)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Ident(String),
    /// A call argument; only lexed right after `name(` or a `,` inside it.
    ComponentId(String),
    Keyword(Keyword),
    Int(u64),
    Number(f64),
    Quantity(Quantity),
    Str(String),
    Cmp(CmpOp),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) tok: Tok,
    pub(crate) span: Span,
}

/// The tokens of `src`, ending with [`Tok::Eof`].
pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, ExprError> {
    let mut lexer = Lexer { src, pos: 0 };
    let mut tokens: Vec<Token> = Vec::new();
    let mut in_args = false;
    loop {
        lexer.skip_whitespace();
        let start = lexer.pos;
        let Some(b) = lexer.peek() else {
            tokens.push(Token {
                tok: Tok::Eof,
                span: Span::new(start, start),
            });
            return Ok(tokens);
        };
        let tok = if in_args && is_ident_start(b) {
            lexer.component_id()
        } else {
            lexer.token(b)?
        };
        let after_name = tokens
            .last()
            .is_some_and(|t| matches!(t.tok, Tok::Ident(_)));
        in_args = match &tok {
            Tok::LParen => after_name,
            Tok::ComponentId(_) | Tok::Comma => in_args,
            Tok::Ident(_)
            | Tok::Keyword(_)
            | Tok::Int(_)
            | Tok::Number(_)
            | Tok::Quantity(_)
            | Tok::Str(_)
            | Tok::Cmp(_)
            | Tok::Plus
            | Tok::Minus
            | Tok::Star
            | Tok::Slash
            | Tok::RParen
            | Tok::LBracket
            | Tok::RBracket
            | Tok::Dot
            | Tok::Eof => false,
        };
        tokens.push(Token {
            tok,
            span: Span::new(start, lexer.pos),
        });
    }
}

struct Lexer<'s> {
    src: &'s str,
    pos: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.peek_at(0)
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.as_bytes().get(self.pos + offset).copied()
    }

    fn eat_while(&mut self, f: impl Fn(u8) -> bool) {
        while self.peek().is_some_and(&f) {
            self.pos += 1;
        }
    }

    fn skip_whitespace(&mut self) {
        self.eat_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'));
    }

    fn text(&self, start: usize) -> &str {
        self.src.get(start..self.pos).unwrap_or_default()
    }

    fn error(&self, kind: ErrorKind, start: usize) -> ExprError {
        ExprError::new(kind, Span::new(start, self.pos))
    }

    /// Consumes `len` bytes as `tok`.
    fn take(&mut self, len: usize, tok: Tok) -> Tok {
        self.pos += len;
        tok
    }

    /// Consumes `len` bytes and reports them as unexpected.
    fn reject(&mut self, len: usize, hint: Option<Hint>) -> Result<Tok, ExprError> {
        let start = self.pos;
        self.pos += len;
        Err(self.error(
            ErrorKind::UnexpectedText {
                found: self.text(start).to_owned(),
                hint,
            },
            start,
        ))
    }

    fn token(&mut self, b: u8) -> Result<Tok, ExprError> {
        let next = self.peek_at(1);
        match b {
            b'0'..=b'9' => self.number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => Ok(self.word()),
            b'\'' => self.string(),
            b'(' => Ok(self.take(1, Tok::LParen)),
            b')' => Ok(self.take(1, Tok::RParen)),
            b'[' => Ok(self.take(1, Tok::LBracket)),
            b']' => Ok(self.take(1, Tok::RBracket)),
            b',' => Ok(self.take(1, Tok::Comma)),
            b'+' => Ok(self.take(1, Tok::Plus)),
            b'-' => Ok(self.take(1, Tok::Minus)),
            b'*' => Ok(self.take(1, Tok::Star)),
            b'/' => Ok(self.take(1, Tok::Slash)),
            b'.' if next.is_some_and(|c| c.is_ascii_digit()) => {
                self.reject(1, Some(Hint::LeadingDot))
            }
            b'.' => Ok(self.take(1, Tok::Dot)),
            b'=' => match next {
                Some(b'=') => Ok(self.take(2, Tok::Cmp(CmpOp::Eq))),
                Some(b'>') => self.reject(2, Some(Hint::UseGe)),
                Some(b'<') => self.reject(2, Some(Hint::UseLe)),
                Some(_) | None => self.reject(1, Some(Hint::UseEq)),
            },
            b'!' if next == Some(b'=') => Ok(self.take(2, Tok::Cmp(CmpOp::Ne))),
            b'!' => self.reject(1, Some(Hint::UseNot)),
            b'<' => match next {
                Some(b'=') => Ok(self.take(2, Tok::Cmp(CmpOp::Le))),
                Some(b'>') => self.reject(2, Some(Hint::UseNe)),
                Some(_) | None => Ok(self.take(1, Tok::Cmp(CmpOp::Lt))),
            },
            b'>' if next == Some(b'=') => Ok(self.take(2, Tok::Cmp(CmpOp::Ge))),
            b'>' => Ok(self.take(1, Tok::Cmp(CmpOp::Gt))),
            b'&' => self.reject(1 + usize::from(next == Some(b'&')), Some(Hint::UseAnd)),
            b'|' => self.reject(1 + usize::from(next == Some(b'|')), Some(Hint::UseOr)),
            b'"' => self.reject(1, Some(Hint::SingleQuotes)),
            b'%' => self.reject(1, Some(Hint::NoModulo)),
            _ => {
                // One whole character, which may be several bytes.
                let len = self
                    .src
                    .get(self.pos..)
                    .and_then(|s| s.chars().next())
                    .map_or(1, char::len_utf8);
                self.reject(len, None)
            }
        }
    }

    fn word(&mut self) -> Tok {
        let start = self.pos;
        self.eat_while(is_ident_char);
        let word = self.text(start);
        Keyword::from_word(word).map_or_else(|| Tok::Ident(word.to_owned()), Tok::Keyword)
    }

    fn component_id(&mut self) -> Tok {
        let start = self.pos;
        self.eat_while(is_ident_char);
        while self.peek() == Some(b'-') && self.peek_at(1).is_some_and(is_ident_char) {
            self.pos += 1;
            self.eat_while(is_ident_char);
        }
        Tok::ComponentId(self.text(start).to_owned())
    }

    fn string(&mut self) -> Result<Tok, ExprError> {
        let start = self.pos;
        let rest = self.src.get(start + 1..).unwrap_or_default();
        if let Some(len) = rest.find('\'') {
            self.pos = start + 1 + len + 1;
            Ok(Tok::Str(rest.get(..len).unwrap_or_default().to_owned()))
        } else {
            self.pos = self.src.len();
            Err(self.error(ErrorKind::UnterminatedString, start))
        }
    }

    /// A number, with its unit if one follows.
    fn number(&mut self) -> Result<Tok, ExprError> {
        let start = self.pos;
        self.eat_while(|b| b.is_ascii_digit());
        let mut fraction = false;
        if self.peek() == Some(b'.') {
            if !self.peek_at(1).is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
                return Err(self.malformed(start));
            }
            self.pos += 1;
            self.eat_while(|b| b.is_ascii_digit());
            fraction = true;
            if self.peek() == Some(b'.') {
                self.pos += 1;
                self.eat_while(|b| b.is_ascii_digit());
                return Err(self.malformed(start));
            }
        }
        if let Some(unit_start) = self.unit_start() {
            return self.quantity(start, unit_start);
        }
        let text = self.text(start);
        if fraction {
            match text.parse::<f64>() {
                Ok(x) if x.is_finite() => Ok(Tok::Number(x)),
                Ok(_) | Err(_) => Err(self.error(
                    ErrorKind::NumberTooLarge {
                        text: text.to_owned(),
                    },
                    start,
                )),
            }
        } else {
            text.parse::<u64>().map(Tok::Int).map_err(|_| {
                self.error(
                    ErrorKind::IntTooLarge {
                        text: text.to_owned(),
                    },
                    start,
                )
            })
        }
    }

    fn malformed(&self, start: usize) -> ExprError {
        self.error(
            ErrorKind::MalformedNumber {
                text: self.text(start).to_owned(),
            },
            start,
        )
    }

    /// Where the unit after a number starts: right after it, or after one
    /// space unless the next word is a keyword (`3 and x`).
    fn unit_start(&self) -> Option<usize> {
        let is_unit_start = |b: u8| b == b'%' || is_ident_start(b);
        match self.peek() {
            Some(b) if is_unit_start(b) => Some(self.pos),
            Some(b' ') if self.peek_at(1).is_some_and(is_unit_start) => {
                let word_start = self.pos + 1;
                let word_len = self
                    .src
                    .as_bytes()
                    .get(word_start..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|&&b| is_ident_char(b))
                    .count();
                let word = self
                    .src
                    .get(word_start..word_start + word_len)
                    .unwrap_or_default();
                Keyword::from_word(word).is_none().then_some(word_start)
            }
            Some(_) | None => None,
        }
    }

    /// The quantity `src[start..]` whose unit starts at `unit_start`.
    fn quantity(&mut self, start: usize, unit_start: usize) -> Result<Tok, ExprError> {
        self.pos = unit_start;
        if self.peek() == Some(b'%') {
            self.pos += 1;
        } else {
            self.eat_while(is_ident_char);
            let base = self.text(unit_start);
            if (base == "parts" || base == "part")
                && self.peek() == Some(b'/')
                && self.peek_at(1).is_some_and(|b| b.is_ascii_alphabetic())
            {
                self.pos += 1;
                self.eat_while(is_ident_char);
            }
        }
        let text = self.text(start);
        let unit = self.text(unit_start);
        let parsed = match unit {
            "%" => text.parse::<Ratio>().map(Quantity::Ratio),
            "d" | "h" | "min" | "s" | "ms" => text.parse::<SimDuration>().map(Quantity::Time),
            "parts" | "part" => text.parse::<Count>().map(Quantity::Count),
            u if u.starts_with("parts/") || u.starts_with("part/") => {
                text.parse::<Rate>().map(Quantity::Rate)
            }
            _ => {
                return Err(self.error(
                    ErrorKind::UnknownUnit {
                        unit: unit.to_owned(),
                        suggestion: suggest(unit, UNITS),
                    },
                    unit_start,
                ));
            }
        };
        parsed
            .map(Tok::Quantity)
            .map_err(|source| self.error(ErrorKind::InvalidQuantity { source }, start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn call_arguments_are_component_ids() {
        assert_eq!(
            toks("f(weld-1, b) - c"),
            [
                Tok::Ident("f".into()),
                Tok::LParen,
                Tok::ComponentId("weld-1".into()),
                Tok::Comma,
                Tok::ComponentId("b".into()),
                Tok::RParen,
                Tok::Minus,
                Tok::Ident("c".into()),
                Tok::Eof,
            ]
        );
        // Outside a call, `-` is an operator.
        assert_eq!(
            toks("weld-1"),
            [Tok::Ident("weld".into()), Tok::Minus, Tok::Int(1), Tok::Eof]
        );
        // `(` after a keyword or literal does not start arguments.
        assert_eq!(toks("not (a)")[2], Tok::Ident("a".into()));
        assert_eq!(toks("f(a - b)")[3], Tok::Minus);
        assert_eq!(toks("f(a b)")[3], Tok::ComponentId("b".into()));
    }

    #[test]
    fn keywords_end_a_number() {
        assert_eq!(
            toks("3 and x"),
            [
                Tok::Int(3),
                Tok::Keyword(Keyword::And),
                Tok::Ident("x".into()),
                Tok::Eof
            ]
        );
        assert_eq!(toks("3 in")[1], Tok::Keyword(Keyword::In));
    }

    #[test]
    fn spans_cover_tokens() {
        let spans: Vec<Span> = tokenize("ab <= 'x'")
            .unwrap()
            .into_iter()
            .map(|t| t.span)
            .collect();
        assert_eq!(
            spans,
            [
                Span::new(0, 2),
                Span::new(3, 5),
                Span::new(6, 9),
                Span::new(9, 9)
            ]
        );
    }

    #[test]
    fn miscased_keywords() {
        assert_eq!(miscased_keyword("AND"), Some("and"));
        assert_eq!(miscased_keyword("and"), None);
        assert_eq!(miscased_keyword("andx"), None);
    }

    #[test]
    fn keyword_names() {
        for k in KEYWORDS {
            assert_eq!(Keyword::from_word(k).map(Keyword::as_str), Some(k));
        }
    }
}
