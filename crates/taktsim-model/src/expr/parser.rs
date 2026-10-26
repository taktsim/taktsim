//! Recursive-descent parser for the ADR-0009 grammar.
//!
//! ```ebnf
//! expr = or ;
//! or   = and { "or" and } ;
//! and  = not { "and" not } ;
//! not  = [ "not" ] cmp ;
//! cmp  = sum [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) sum | "in" list ] ;
//! sum  = term { ( "+" | "-" ) term } ;
//! term = unary { ( "*" | "/" ) unary } ;
//! unary = [ "-" ] atom ;
//! atom = quantity | number | string | bool | call | path | "(" expr ")" ;
//! call = ident "(" [ component-id { "," component-id } ] ")" ;
//! path = ident { "." ident } ;
//! list = "[" [ atom { "," atom } ] "]" ;
//! ```
//!
//! Nodes are built with [`Expr::new`], which enforces the depth limit.
//! Only parentheses make the parser recurse, so they are counted against
//! the same limit; a list cannot nest without them.

use super::MAX_DEPTH;
use super::ast::{BinOp, Call, ComponentId, Expr, ExprKind, Ident, List, Number, Path, Text};
use super::error::{ErrorKind, Expectation, ExprError, Hint};
use super::lexer::{Keyword, Tok, Token, tokenize, units};
use super::span::{Span, Spanned};

/// Parses `source` into a syntax tree.
///
/// # Errors
///
/// An [`ExprError`] with the span of the first text that is not part of
/// the grammar, what was expected there and, for common mistakes, the fix.
///
/// ```
/// use taktsim_model::expr::parse;
///
/// let expr = parse("variant == 'premium'")?;
/// assert_eq!(expr.to_string(), "variant == 'premium'");
///
/// let err = parse("variant = 'premium'").unwrap_err();
/// assert_eq!(err.to_string(), "unexpected `=`; write `==` to compare");
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
pub fn parse(source: &str) -> Result<Expr, ExprError> {
    let tokens = tokenize(source)?;
    let end = tokens.last().cloned().unwrap_or(Token {
        tok: Tok::Eof,
        span: Span::default(),
    });
    let mut parser = Parser {
        source,
        tokens,
        pos: 0,
        parens: 0,
        end,
    };
    parser.all()
}

struct Parser<'s> {
    source: &'s str,
    tokens: Vec<Token>,
    pos: usize,
    /// Open parentheses.
    parens: usize,
    /// The end of the input, returned when reading past it.
    end: Token,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&self.end)
    }

    fn at(&self, tok: &Tok) -> bool {
        self.peek().tok == *tok
    }

    fn previous(&self) -> Option<&Tok> {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.tokens.get(i))
            .map(|t| &t.tok)
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if token.tok != Tok::Eof {
            self.pos += 1;
        }
        token
    }

    /// An error for the current token, which is not `expected`.
    fn unexpected(&self, expected: Expectation, hint: Option<Hint>) -> ExprError {
        let token = self.peek();
        let found = (token.tok != Tok::Eof).then(|| {
            self.source
                .get(token.span.range())
                .unwrap_or_default()
                .to_owned()
        });
        ExprError::new(
            ErrorKind::Expected {
                expected,
                found,
                hint,
            },
            token.span,
        )
    }

    fn all(&mut self) -> Result<Expr, ExprError> {
        if self.at(&Tok::Eof) {
            return Err(ExprError::new(ErrorKind::Empty, self.peek().span));
        }
        let expr = self.or()?;
        if self.at(&Tok::Eof) {
            return Ok(expr);
        }
        let hint = match &self.peek().tok {
            Tok::RParen => Some(Hint::UnmatchedParen),
            Tok::Ident(word) => {
                let after_number = matches!(self.previous(), Some(Tok::Int(_) | Tok::Number(_)));
                (after_number && units().contains(&word.as_str())).then_some(Hint::UnitSpacing)
            }
            Tok::LBracket => Some(Hint::ListOnlyAfterIn),
            Tok::ComponentId(_)
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
            | Tok::LParen
            | Tok::RBracket
            | Tok::Comma
            | Tok::Dot
            | Tok::Eof => None,
        };
        Err(self.unexpected(Expectation::OperatorOrEnd, hint))
    }

    fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Result<Expr, ExprError> {
        let span = lhs.span().to(rhs.span());
        let kind = ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        };
        Expr::new(kind, span)
    }

    fn or(&mut self) -> Result<Expr, ExprError> {
        let mut lhs = self.and()?;
        while self.at(&Tok::Keyword(Keyword::Or)) {
            self.bump();
            let rhs = self.and()?;
            lhs = Self::binary(BinOp::Or, lhs, rhs)?;
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<Expr, ExprError> {
        let mut lhs = self.not()?;
        while self.at(&Tok::Keyword(Keyword::And)) {
            self.bump();
            let rhs = self.not()?;
            lhs = Self::binary(BinOp::And, lhs, rhs)?;
        }
        Ok(lhs)
    }

    fn not(&mut self) -> Result<Expr, ExprError> {
        if !self.at(&Tok::Keyword(Keyword::Not)) {
            return self.cmp();
        }
        let keyword = self.bump();
        if self.at(&Tok::Keyword(Keyword::Not)) {
            return Err(ExprError::new(ErrorKind::DoubleNot, self.peek().span));
        }
        let operand = self.cmp()?;
        let span = keyword.span.to(operand.span());
        Expr::new(ExprKind::Not(Box::new(operand)), span)
    }

    fn cmp(&mut self) -> Result<Expr, ExprError> {
        let lhs = self.sum()?;
        let expr = if let Tok::Cmp(op) = self.peek().tok {
            self.bump();
            let rhs = self.sum()?;
            let span = lhs.span().to(rhs.span());
            let kind = ExprKind::Compare {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
            Expr::new(kind, span)?
        } else if self.at(&Tok::Keyword(Keyword::In)) {
            self.bump();
            let list = self.list()?;
            let span = lhs.span().to(list.span);
            let kind = ExprKind::In {
                lhs: Box::new(lhs),
                list,
            };
            Expr::new(kind, span)?
        } else {
            return Ok(lhs);
        };
        if matches!(self.peek().tok, Tok::Cmp(_) | Tok::Keyword(Keyword::In)) {
            return Err(ExprError::new(ErrorKind::ComparisonChain, self.peek().span));
        }
        Ok(expr)
    }

    fn sum(&mut self) -> Result<Expr, ExprError> {
        let mut lhs = self.term()?;
        loop {
            let op = if self.at(&Tok::Plus) {
                BinOp::Add
            } else if self.at(&Tok::Minus) {
                BinOp::Sub
            } else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = self.term()?;
            lhs = Self::binary(op, lhs, rhs)?;
        }
    }

    fn term(&mut self) -> Result<Expr, ExprError> {
        let mut lhs = self.unary()?;
        loop {
            let op = if self.at(&Tok::Star) {
                BinOp::Mul
            } else if self.at(&Tok::Slash) {
                BinOp::Div
            } else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = self.unary()?;
            lhs = Self::binary(op, lhs, rhs)?;
        }
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        if !self.at(&Tok::Minus) {
            return self.atom(Expectation::Expression);
        }
        let minus = self.bump();
        if self.at(&Tok::Minus) {
            return Err(ExprError::new(ErrorKind::DoubleMinus, self.peek().span));
        }
        let operand = self.atom(Expectation::Expression)?;
        let span = minus.span.to(operand.span());
        Expr::new(ExprKind::Neg(Box::new(operand)), span)
    }

    fn atom(&mut self, expected: Expectation) -> Result<Expr, ExprError> {
        let token = self.peek().clone();
        let kind = match token.tok {
            Tok::Int(n) => ExprKind::Int(n),
            Tok::Number(x) => ExprKind::Number(Number(x)),
            Tok::Quantity(q) => ExprKind::Quantity(q),
            Tok::Str(s) => ExprKind::Str(Text(s)),
            Tok::Keyword(Keyword::True) => ExprKind::Bool(true),
            Tok::Keyword(Keyword::False) => ExprKind::Bool(false),
            Tok::Ident(name) => return self.name(name, token.span),
            Tok::LParen => return self.paren(),
            Tok::Keyword(Keyword::Not) => {
                return Err(self.unexpected(expected, Some(Hint::ParenthesizeNot)));
            }
            Tok::Minus if expected == Expectation::ListElement => {
                return Err(self.unexpected(expected, Some(Hint::ParenthesizeElement)));
            }
            Tok::RParen | Tok::RBracket if self.previous() == Some(&Tok::Comma) => {
                return Err(self.unexpected(expected, Some(Hint::TrailingComma)));
            }
            Tok::LBracket => return Err(self.unexpected(expected, Some(Hint::ListOnlyAfterIn))),
            Tok::Keyword(Keyword::And | Keyword::Or | Keyword::In)
            | Tok::ComponentId(_)
            | Tok::Cmp(_)
            | Tok::Plus
            | Tok::Minus
            | Tok::Star
            | Tok::Slash
            | Tok::RParen
            | Tok::RBracket
            | Tok::Comma
            | Tok::Dot
            | Tok::Eof => return Err(self.unexpected(expected, None)),
        };
        self.bump();
        Expr::new(kind, token.span)
    }

    /// A path or a call, starting with the identifier `first`.
    fn name(&mut self, first: String, first_span: Span) -> Result<Expr, ExprError> {
        self.bump();
        if self.at(&Tok::LParen) {
            return self.call(Ident(first), first_span);
        }
        let mut segments = vec![Ident(first)];
        let mut span = first_span;
        while self.at(&Tok::Dot) {
            self.bump();
            let token = self.peek().clone();
            if let Tok::Ident(segment) = token.tok {
                self.bump();
                segments.push(Ident(segment));
                span = span.to(token.span);
            } else if let Tok::Keyword(keyword) = token.tok {
                return Err(ExprError::new(
                    ErrorKind::KeywordAsName {
                        keyword: keyword.as_str(),
                    },
                    token.span,
                ));
            } else {
                return Err(self.unexpected(Expectation::Name, None));
            }
        }
        let path = Path(segments);
        if self.at(&Tok::LParen) {
            return Err(ExprError::new(
                ErrorKind::CallOnPath {
                    path: path.to_string(),
                },
                span,
            ));
        }
        Expr::new(ExprKind::Path(path), span)
    }

    fn call(&mut self, name: Ident, name_span: Span) -> Result<Expr, ExprError> {
        self.bump(); // `(`
        let mut args = Vec::new();
        if !self.at(&Tok::RParen) {
            loop {
                let token = self.peek().clone();
                let Tok::ComponentId(id) = token.tok else {
                    let hint = if self.at(&Tok::RParen) {
                        Hint::TrailingComma
                    } else {
                        Hint::ArgsAreIds
                    };
                    return Err(self.unexpected(Expectation::ComponentId, Some(hint)));
                };
                self.bump();
                args.push(Spanned {
                    value: ComponentId(id),
                    span: token.span,
                });
                if self.at(&Tok::Comma) {
                    self.bump();
                } else if self.at(&Tok::RParen) {
                    break;
                } else {
                    return Err(self.unexpected(Expectation::CommaOrParen, Some(Hint::ArgsAreIds)));
                }
            }
        }
        let close = self.bump();
        let call = Call {
            name: Spanned {
                value: name,
                span: name_span,
            },
            args,
        };
        Expr::new(ExprKind::Call(call), name_span.to(close.span))
    }

    fn paren(&mut self) -> Result<Expr, ExprError> {
        let open = self.bump();
        self.parens += 1;
        if self.parens > MAX_DEPTH {
            return Err(ExprError::new(
                ErrorKind::TooDeep { limit: MAX_DEPTH },
                open.span,
            ));
        }
        let inner = self.or()?;
        if !self.at(&Tok::RParen) {
            return Err(self.unexpected(Expectation::CloseParen, None));
        }
        let close = self.bump();
        self.parens = self.parens.saturating_sub(1);
        Ok(inner.with_span(open.span.to(close.span)))
    }

    /// The list after `in`. Its elements are atoms, so it recurses only
    /// through a parenthesized element.
    fn list(&mut self) -> Result<List, ExprError> {
        if !self.at(&Tok::LBracket) {
            return Err(self.unexpected(Expectation::List, None));
        }
        let open = self.bump();
        let mut items = Vec::new();
        if !self.at(&Tok::RBracket) {
            loop {
                items.push(self.atom(Expectation::ListElement)?);
                if self.at(&Tok::Comma) {
                    self.bump();
                } else if self.at(&Tok::RBracket) {
                    break;
                } else {
                    return Err(self
                        .unexpected(Expectation::CommaOrBracket, Some(Hint::ParenthesizeElement)));
                }
            }
        }
        let close = self.bump();
        Ok(List {
            items,
            span: open.span.to(close.span),
        })
    }
}
