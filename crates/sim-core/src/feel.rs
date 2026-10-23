//! The small FEEL subset the simulator understands (spec §5.9):
//! literals (numbers, "strings", true, false, null), `-`, comparisons (`<`, `<=`, `>`, `>=`,
//! `=`, `!=`), ranges `[a..b]` / `(a..b)` / `]a..b[`, lists `"a","b"` and `not(...)` in decision
//! table input entries; and for gateway conditions `name op literal` combined with `and`, `or`,
//! `not(...)` and parentheses. Names may be dotted (`risk.score`).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Value>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => write!(f, "{n}"),
            Value::Str(s) => write!(f, "\"{s}\""),
            Value::List(l) => {
                let parts: Vec<String> = l.iter().map(Value::to_string).collect();
                write!(f, "[{}]", parts.join(", "))
            }
        }
    }
}

impl Value {
    /// FEEL comparison: only values of the same type compare; anything else is `None` (null).
    fn compare(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Num(a), Value::Num(b)) => a.partial_cmp(b),
            (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            (Value::Null, Value::Null) => Some(Ordering::Equal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Op {
    fn holds(self, ord: Option<Ordering>) -> bool {
        match (self, ord) {
            (Op::Ne, None) => true,
            (_, None) => false,
            (Op::Lt, Some(o)) => o == Ordering::Less,
            (Op::Le, Some(o)) => o != Ordering::Greater,
            (Op::Gt, Some(o)) => o == Ordering::Greater,
            (Op::Ge, Some(o)) => o != Ordering::Less,
            (Op::Eq, Some(o)) => o == Ordering::Equal,
            (Op::Ne, Some(o)) => o != Ordering::Equal,
        }
    }
}

// ------------------------------------------------------------------ tokens

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(Value),
    Name(String),
    Op(Op),
    Range,
    Comma,
    Open(char),
    Close(char),
    Dash,
    And,
    Or,
    Not,
}

fn tokenize(text: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < cs.len() && cs[i] != '"' {
                    if cs[i] == '\\' && i + 1 < cs.len() {
                        i += 1;
                    }
                    s.push(cs[i]);
                    i += 1;
                }
                if i >= cs.len() {
                    return Err("unterminated string".into());
                }
                i += 1;
                out.push(Tok::Lit(Value::Str(s)));
            }
            '.' if cs.get(i + 1) == Some(&'.') => {
                out.push(Tok::Range);
                i += 2;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '(' | '[' => {
                out.push(Tok::Open(c));
                i += 1;
            }
            ')' => {
                out.push(Tok::Close(c));
                i += 1;
            }
            ']' => {
                // `]` both closes `[a..b]` and opens `]a..b[` (exclusive start).
                out.push(
                    if out.is_empty() || matches!(out.last(), Some(Tok::Comma) | Some(Tok::Not)) {
                        Tok::Open(']')
                    } else {
                        Tok::Close(']')
                    },
                );
                i += 1;
            }
            '<' | '>' | '=' | '!' => {
                let two = cs.get(i + 1) == Some(&'=');
                let op = match (c, two) {
                    ('<', true) => Op::Le,
                    ('<', false) => Op::Lt,
                    ('>', true) => Op::Ge,
                    ('>', false) => Op::Gt,
                    ('!', true) => Op::Ne,
                    ('=', _) => Op::Eq,
                    _ => return Err(format!("unexpected '{c}'")),
                };
                out.push(Tok::Op(op));
                i += if two && c != '=' { 2 } else { 1 };
            }
            '-' if !cs
                .get(i + 1)
                .is_some_and(|d| d.is_ascii_digit() || *d == '.') =>
            {
                out.push(Tok::Dash);
                i += 1;
            }
            c if c.is_ascii_digit()
                || c == '-'
                || (c == '.' && cs.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                let start = i;
                i += 1;
                while i < cs.len()
                    && (cs[i].is_ascii_digit() || (cs[i] == '.' && cs.get(i + 1) != Some(&'.')))
                {
                    i += 1;
                }
                let s: String = cs[start..i].iter().collect();
                out.push(Tok::Lit(Value::Num(
                    s.parse().map_err(|_| format!("bad number '{s}'"))?,
                )));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < cs.len()
                    && (cs[i].is_alphanumeric()
                        || cs[i] == '_'
                        || (cs[i] == '.' && cs.get(i + 1).is_some_and(|x| x.is_alphabetic())))
                {
                    i += 1;
                }
                let w: String = cs[start..i].iter().collect();
                out.push(match w.as_str() {
                    "true" => Tok::Lit(Value::Bool(true)),
                    "false" => Tok::Lit(Value::Bool(false)),
                    "null" => Tok::Lit(Value::Null),
                    "and" => Tok::And,
                    "or" => Tok::Or,
                    "not" => Tok::Not,
                    _ => Tok::Name(w),
                });
            }
            _ => return Err(format!("unsupported character '{c}'")),
        }
    }
    Ok(out)
}

struct P {
    toks: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn expect(&mut self, t: Tok) -> Result<(), String> {
        match self.next() {
            Some(x) if x == t => Ok(()),
            other => Err(format!("expected {t:?}, found {other:?}")),
        }
    }
    fn literal(&mut self) -> Result<Value, String> {
        match self.next() {
            Some(Tok::Lit(v)) => Ok(v),
            other => Err(format!("expected a literal, found {other:?}")),
        }
    }
    fn done(&self) -> Result<(), String> {
        match self.peek() {
            None => Ok(()),
            Some(t) => Err(format!("unexpected {t:?}")),
        }
    }
}

// ------------------------------------------------------------------ unary tests

#[derive(Debug, Clone, PartialEq)]
pub enum Test {
    Cmp(Op, Value),
    Range {
        lo: Value,
        lo_incl: bool,
        hi: Value,
        hi_incl: bool,
    },
}

/// A decision table input entry.
#[derive(Debug, Clone, PartialEq)]
pub enum UnaryTests {
    Any,
    Tests { negated: bool, tests: Vec<Test> },
}

impl UnaryTests {
    pub fn parse(text: &str) -> Result<Self, String> {
        let toks = tokenize(text).map_err(|e| format!("'{text}': {e}"))?;
        if toks.is_empty() || toks == [Tok::Dash] {
            return Ok(UnaryTests::Any);
        }
        let mut p = P { toks, i: 0 };
        let negated = p.peek() == Some(&Tok::Not);
        if negated {
            p.next();
            p.expect(Tok::Open('('))
                .map_err(|e| format!("'{text}': {e}"))?;
        }
        let mut tests = Vec::new();
        loop {
            tests.push(parse_test(&mut p).map_err(|e| format!("'{text}': {e}"))?);
            if p.peek() == Some(&Tok::Comma) {
                p.next();
            } else {
                break;
            }
        }
        if negated {
            p.expect(Tok::Close(')'))
                .map_err(|e| format!("'{text}': {e}"))?;
        }
        p.done().map_err(|e| format!("'{text}': {e}"))?;
        Ok(UnaryTests::Tests { negated, tests })
    }

    pub fn matches(&self, v: &Value) -> bool {
        match self {
            UnaryTests::Any => true,
            UnaryTests::Tests { negated, tests } => tests.iter().any(|t| test(t, v)) != *negated,
        }
    }
}

fn parse_test(p: &mut P) -> Result<Test, String> {
    match p.peek() {
        Some(Tok::Op(_)) => {
            let Some(Tok::Op(op)) = p.next() else {
                unreachable!()
            };
            Ok(Test::Cmp(op, p.literal()?))
        }
        Some(Tok::Open(c)) if *c != '(' || matches!(p.toks.get(p.i + 2), Some(Tok::Range)) => {
            let c = *c;
            p.next();
            let lo = p.literal()?;
            p.expect(Tok::Range)?;
            let hi = p.literal()?;
            let hi_incl = match p.next() {
                Some(Tok::Close(']')) => true,
                Some(Tok::Close(')')) | Some(Tok::Open('[')) => false,
                other => return Err(format!("expected end of range, found {other:?}")),
            };
            Ok(Test::Range {
                lo,
                lo_incl: c == '[',
                hi,
                hi_incl,
            })
        }
        _ => Ok(Test::Cmp(Op::Eq, p.literal()?)),
    }
}

fn test(t: &Test, v: &Value) -> bool {
    match t {
        Test::Cmp(op, x) => op.holds(v.compare(x)),
        Test::Range {
            lo,
            lo_incl,
            hi,
            hi_incl,
        } => {
            let a = if *lo_incl { Op::Ge } else { Op::Gt };
            let b = if *hi_incl { Op::Le } else { Op::Lt };
            a.holds(v.compare(lo)) && b.holds(v.compare(hi))
        }
    }
}

/// A decision table output entry: a literal, or empty/`-`/`null` for null.
pub fn parse_output(text: &str) -> Result<Value, String> {
    let toks = tokenize(text).map_err(|e| format!("'{text}': {e}"))?;
    match toks.as_slice() {
        [] | [Tok::Dash] => Ok(Value::Null),
        [Tok::Lit(v)] => Ok(v.clone()),
        _ => Err(format!("'{text}': only literal outputs are supported")),
    }
}

// ------------------------------------------------------------------ conditions

/// A gateway condition (`conditionExpression`).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Const(bool),
    Cmp(String, Op, Value),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

impl Expr {
    /// Parse a condition; a leading `=` (Camunda 8 FEEL expression marker) is allowed.
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim();
        if t.starts_with("${") || t.starts_with("#{") {
            return Err(format!(
                "'{t}': JUEL expressions are not supported, use FEEL"
            ));
        }
        let t = t.strip_prefix('=').unwrap_or(t);
        let mut p = P {
            toks: tokenize(t).map_err(|e| format!("'{t}': {e}"))?,
            i: 0,
        };
        let e = or_expr(&mut p).map_err(|e| format!("'{t}': {e}"))?;
        p.done().map_err(|e| format!("'{t}': {e}"))?;
        Ok(e)
    }

    /// Unknown variables are null; a comparison with a list is true if any element satisfies it.
    pub fn eval(&self, vars: &BTreeMap<String, Value>) -> bool {
        match self {
            Expr::Const(b) => *b,
            Expr::Cmp(name, op, lit) => match vars.get(name).unwrap_or(&Value::Null) {
                Value::List(items) => items.iter().any(|v| op.holds(v.compare(lit))),
                v => op.holds(v.compare(lit)),
            },
            Expr::Not(e) => !e.eval(vars),
            Expr::And(es) => es.iter().all(|e| e.eval(vars)),
            Expr::Or(es) => es.iter().any(|e| e.eval(vars)),
        }
    }
}

fn or_expr(p: &mut P) -> Result<Expr, String> {
    let mut parts = vec![and_expr(p)?];
    while p.peek() == Some(&Tok::Or) {
        p.next();
        parts.push(and_expr(p)?);
    }
    Ok(if parts.len() == 1 {
        parts.remove(0)
    } else {
        Expr::Or(parts)
    })
}

fn and_expr(p: &mut P) -> Result<Expr, String> {
    let mut parts = vec![unary(p)?];
    while p.peek() == Some(&Tok::And) {
        p.next();
        parts.push(unary(p)?);
    }
    Ok(if parts.len() == 1 {
        parts.remove(0)
    } else {
        Expr::And(parts)
    })
}

fn unary(p: &mut P) -> Result<Expr, String> {
    match p.next() {
        Some(Tok::Not) => {
            p.expect(Tok::Open('('))?;
            let e = or_expr(p)?;
            p.expect(Tok::Close(')'))?;
            Ok(Expr::Not(Box::new(e)))
        }
        Some(Tok::Open('(')) => {
            let e = or_expr(p)?;
            p.expect(Tok::Close(')'))?;
            Ok(e)
        }
        Some(Tok::Lit(Value::Bool(b))) => Ok(Expr::Const(b)),
        Some(Tok::Name(n)) => match p.next() {
            Some(Tok::Op(op)) => Ok(Expr::Cmp(n, op, p.literal()?)),
            other => Err(format!(
                "expected a comparison after '{n}', found {other:?}"
            )),
        },
        other => Err(format!("expected a condition, found {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> Value {
        Value::Num(x)
    }
    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn unary_tests() {
        let m = |e: &str, v: Value| UnaryTests::parse(e).unwrap().matches(&v);
        assert!(m("-", n(5.0)) && m("", s("x")));
        assert!(m("< 1000", n(999.5)) && !m("< 1000", n(1000.0)));
        assert!(m("<= 1000", n(1000.0)) && m(">= -5", n(-5.0)) && !m("> 2", n(2.0)));
        assert!(
            m("[1000..5000]", n(1000.0))
                && m("[1000..5000]", n(5000.0))
                && !m("[1000..5000]", n(5000.1))
        );
        assert!(
            !m("(1..2)", n(1.0))
                && m("(1..2)", n(1.5))
                && !m("]1..2[", n(2.0))
                && m("]1..2]", n(2.0))
        );
        assert!(m("[1..2[", n(1.0)) && !m("[1..2[", n(2.0)));
        assert!(m("\"gold\"", s("gold")) && !m("\"gold\"", s("silver")));
        assert!(m("\"gold\",\"silver\"", s("silver")) && m("1, 2, 3", n(2.0)));
        assert!(m("not(\"gold\")", s("silver")) && !m("not(\"gold\")", s("gold")));
        assert!(m("not([1..2], 5)", n(3.0)) && !m("not([1..2], 5)", n(5.0)));
        assert!(m("true", Value::Bool(true)) && !m("true", Value::Bool(false)));
        assert!(m("12.5", n(12.5)) && m("-3", n(-3.0)));
        assert!(!m("< 5", s("a")), "different types do not compare");
        assert!(m("null", Value::Null));
        for bad in ["date(\"2020-01-01\")", "[1..", "< ", "\"open"] {
            assert!(UnaryTests::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn outputs() {
        assert_eq!(parse_output("\"auto\"").unwrap(), s("auto"));
        assert_eq!(parse_output("42").unwrap(), n(42.0));
        assert_eq!(parse_output("").unwrap(), Value::Null);
        assert!(parse_output("a + b").is_err());
    }

    #[test]
    fn conditions() {
        let vars: BTreeMap<String, Value> = [
            ("route".into(), s("auto")),
            ("score".into(), n(70.0)),
            ("risk.level".into(), s("high")),
            ("tags".into(), Value::List(vec![s("a"), s("b")])),
        ]
        .into();
        let c = |e: &str| Expr::parse(e).unwrap().eval(&vars);
        assert!(c("=route = \"auto\"") && !c("route = \"manual\""));
        assert!(c("score > 50 and route != \"manual\"") && !c("score > 80 or route = \"x\""));
        assert!(c("not(score < 10)") && c("(score >= 70) and (risk.level = \"high\")"));
        assert!(c("tags = \"b\"") && !c("tags = \"c\""));
        assert!(!c("missing = 1") && c("missing = null") && c("true"));
        assert!(Expr::parse("${amount > 5}").is_err());
        assert!(Expr::parse("score >").is_err());
        assert!(Expr::parse("score > 5 extra").is_err());
    }
}
