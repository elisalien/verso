//! Interpreter for the EEL expression language of MilkDrop presets.
//!
//! Source is compiled to a small tree against a [`Symbols`] table, then
//! evaluated over a slice of `f64` slots. Semantics follow ns-eel2 as used by
//! MilkDrop 2 and Butterchurn: every value is a float, comparisons give 0 or 1,
//! division and modulo by zero give 0, identifiers are case-insensitive.

use std::collections::HashMap;

/// Values within this distance of zero count as false.
const EPSILON: f64 = 0.00001;
/// Size of `megabuf` and `gmegabuf`, in values.
pub const MEGABUF_SIZE: usize = 1 << 20;
/// Upper bound on `loop` and `while` iterations, as in ns-eel2.
const MAX_LOOP: usize = 1 << 20;

/// Variable name to slot index, shared by every program run on the same values.
#[derive(Debug, Default, Clone)]
pub struct Symbols {
    slots: HashMap<String, usize>,
    names: Vec<String>,
}

impl Symbols {
    /// Slot of `name`, created if missing.
    pub fn slot(&mut self, name: &str) -> usize {
        let name = name.to_ascii_lowercase();
        if let Some(&slot) = self.slots.get(&name) {
            return slot;
        }
        let slot = self.names.len();
        self.slots.insert(name.clone(), slot);
        self.names.push(name);
        slot
    }

    pub fn get(&self, name: &str) -> Option<usize> {
        self.slots.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Func1 {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sqr,
    Sqrt,
    Exp,
    Log,
    Log10,
    Abs,
    Sign,
    Floor,
    Ceil,
    Int,
    Bnot,
    Rand,
    Invsqrt,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Func2 {
    Atan2,
    Pow,
    Min,
    Max,
    Above,
    Below,
    Equal,
    Band,
    Bor,
    Sigmoid,
}

#[derive(Debug, Clone)]
enum Target {
    Var(usize),
    Mem { global: bool, index: Box<Expr> },
}

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    Var(usize),
    Mem {
        global: bool,
        index: Box<Expr>,
    },
    /// `op` is `None` for plain `=`.
    Assign(Target, Option<BinOp>, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Seq(Vec<Expr>),
    Call1(Func1, Box<Expr>),
    Call2(Func2, Box<Expr>, Box<Expr>),
    Loop(Box<Expr>, Box<Expr>),
    While(Box<Expr>),
    MemCopy(bool, Box<Expr>, Box<Expr>, Box<Expr>),
    MemSet(bool, Box<Expr>, Box<Expr>, Box<Expr>),
}

/// Compiled EEL code.
#[derive(Debug, Clone, Default)]
pub struct Program {
    body: Vec<Expr>,
}

impl Program {
    /// Compiles `source`, adding its variables to `symbols`.
    pub fn compile(source: &str, symbols: &mut Symbols) -> Result<Program, String> {
        let tokens = lex(source)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            symbols,
        };
        let body = parser.statements(None)?;
        if parser.pos < parser.tokens.len() {
            return Err(format!("unexpected {:?}", parser.tokens[parser.pos]));
        }
        Ok(Program { body })
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    /// Runs the program over `vars`, which must hold every slot of the
    /// symbols it was compiled against.
    pub fn run(&self, env: &mut Env) {
        for expr in &self.body {
            eval(expr, env);
        }
    }
}

/// Mutable state a program runs against.
pub struct Env<'a> {
    pub vars: &'a mut [f64],
    pub megabuf: &'a mut Vec<f64>,
    pub gmegabuf: &'a mut Vec<f64>,
    pub rng: &'a mut Rng,
}

/// Small xorshift generator, so presets do not need a global RNG.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    /// Uniform value in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn truthy(v: f64) -> bool {
    v.abs() > EPSILON
}

fn bool_num(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

fn mem_index(i: f64) -> Option<usize> {
    let i = (i + 0.0001).floor();
    (i >= 0.0 && i < MEGABUF_SIZE as f64).then_some(i as usize)
}

fn mem_read(buf: &[f64], i: f64) -> f64 {
    mem_index(i)
        .and_then(|i| buf.get(i).copied())
        .unwrap_or(0.0)
}

fn mem_slot(buf: &mut Vec<f64>, i: f64) -> Option<&mut f64> {
    let i = mem_index(i)?;
    if i >= buf.len() {
        buf.resize(i + 1, 0.0);
    }
    Some(&mut buf[i])
}

fn binop(op: BinOp, a: f64, b: f64) -> f64 {
    match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => {
            if b == 0.0 {
                0.0
            } else {
                a / b
            }
        }
        BinOp::Mod => {
            let (a, b) = (a.floor(), b.floor());
            if b == 0.0 { 0.0 } else { a % b }
        }
        BinOp::Pow => finite_or_zero(a.powf(b)),
        BinOp::BitAnd => ((a.floor() as i64) & (b.floor() as i64)) as f64,
        BinOp::BitOr => ((a.floor() as i64) | (b.floor() as i64)) as f64,
        BinOp::Eq => bool_num((a - b).abs() < EPSILON),
        BinOp::Ne => bool_num((a - b).abs() >= EPSILON),
        BinOp::Lt => bool_num(a < b),
        BinOp::Gt => bool_num(a > b),
        BinOp::Le => bool_num(a <= b),
        BinOp::Ge => bool_num(a >= b),
    }
}

fn call1(f: Func1, x: f64, rng: &mut Rng) -> f64 {
    match f {
        Func1::Sin => x.sin(),
        Func1::Cos => x.cos(),
        Func1::Tan => x.tan(),
        Func1::Asin => finite_or_zero(x.asin()),
        Func1::Acos => finite_or_zero(x.acos()),
        Func1::Atan => x.atan(),
        Func1::Sqr => x * x,
        Func1::Sqrt => x.abs().sqrt(),
        Func1::Exp => x.exp(),
        Func1::Log => finite_or_zero(x.ln()),
        Func1::Log10 => finite_or_zero(x.log10()),
        Func1::Abs => x.abs(),
        Func1::Sign => {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        Func1::Floor => x.floor(),
        Func1::Ceil => x.ceil(),
        Func1::Int => x.trunc(),
        Func1::Bnot => bool_num(!truthy(x)),
        Func1::Rand => {
            let xf = x.floor();
            if xf < 1.0 {
                rng.next_f64()
            } else {
                rng.next_f64() * xf
            }
        }
        Func1::Invsqrt => {
            let r = x.abs().sqrt();
            if r == 0.0 { 0.0 } else { 1.0 / r }
        }
    }
}

fn call2(f: Func2, a: f64, b: f64) -> f64 {
    match f {
        Func2::Atan2 => a.atan2(b),
        Func2::Pow => finite_or_zero(a.powf(b)),
        Func2::Min => a.min(b),
        Func2::Max => a.max(b),
        Func2::Above => bool_num(a > b),
        Func2::Below => bool_num(a < b),
        Func2::Equal => bool_num((a - b).abs() < EPSILON),
        Func2::Band => bool_num(truthy(a) && truthy(b)),
        Func2::Bor => bool_num(truthy(a) || truthy(b)),
        Func2::Sigmoid => {
            let t = 1.0 + (-a * b).exp();
            if t.abs() > EPSILON { 1.0 / t } else { 0.0 }
        }
    }
}

fn eval(e: &Expr, env: &mut Env) -> f64 {
    match e {
        Expr::Num(v) => *v,
        Expr::Var(slot) => env.vars[*slot],
        Expr::Mem { global, index } => {
            let i = eval(index, env);
            mem_read(if *global { env.gmegabuf } else { env.megabuf }, i)
        }
        Expr::Assign(target, op, value) => {
            let v = eval(value, env);
            match target {
                Target::Var(slot) => {
                    let new = match op {
                        Some(op) => binop(*op, env.vars[*slot], v),
                        None => v,
                    };
                    env.vars[*slot] = new;
                    new
                }
                Target::Mem { global, index } => {
                    let i = eval(index, env);
                    let buf = if *global {
                        &mut *env.gmegabuf
                    } else {
                        &mut *env.megabuf
                    };
                    match mem_slot(buf, i) {
                        Some(cell) => {
                            let new = match op {
                                Some(op) => binop(*op, *cell, v),
                                None => v,
                            };
                            *cell = new;
                            new
                        }
                        None => 0.0,
                    }
                }
            }
        }
        Expr::Bin(op, a, b) => {
            let a = eval(a, env);
            let b = eval(b, env);
            binop(*op, a, b)
        }
        Expr::Neg(a) => -eval(a, env),
        Expr::Not(a) => bool_num(!truthy(eval(a, env))),
        Expr::And(a, b) => bool_num(truthy(eval(a, env)) && truthy(eval(b, env))),
        Expr::Or(a, b) => bool_num(truthy(eval(a, env)) || truthy(eval(b, env))),
        Expr::If(c, a, b) => {
            if truthy(eval(c, env)) {
                eval(a, env)
            } else {
                eval(b, env)
            }
        }
        Expr::Seq(items) => {
            let mut last = 0.0;
            for item in items {
                last = eval(item, env);
            }
            last
        }
        Expr::Call1(f, a) => {
            let x = eval(a, env);
            call1(*f, x, env.rng)
        }
        Expr::Call2(f, a, b) => {
            let a = eval(a, env);
            let b = eval(b, env);
            call2(*f, a, b)
        }
        Expr::Loop(count, body) => {
            let n = eval(count, env).floor().clamp(0.0, MAX_LOOP as f64) as usize;
            let mut last = 0.0;
            for _ in 0..n {
                last = eval(body, env);
            }
            last
        }
        Expr::While(body) => {
            let mut last = 0.0;
            for _ in 0..MAX_LOOP {
                last = eval(body, env);
                if !truthy(last) {
                    break;
                }
            }
            last
        }
        Expr::MemCopy(global, dst, src, len) => {
            let dst = eval(dst, env);
            let src = eval(src, env);
            let len = eval(len, env);
            let buf = if *global {
                &mut *env.gmegabuf
            } else {
                &mut *env.megabuf
            };
            let (mut d, mut s, mut n) = (dst.floor(), src.floor(), len.floor());
            if s < 0.0 {
                n += s;
                d -= s;
                s = 0.0;
            }
            if d < 0.0 {
                n += d;
                s -= d;
                d = 0.0;
            }
            let end = (d.max(s) + n).min(MEGABUF_SIZE as f64);
            if n > 0.0 && end > d.max(s) {
                let (d, s) = (d as usize, s as usize);
                let n = end as usize - d.max(s);
                if buf.len() < d.max(s) + n {
                    buf.resize(d.max(s) + n, 0.0);
                }
                buf.copy_within(s..s + n, d);
            }
            dst
        }
        Expr::MemSet(global, dst, value, len) => {
            let dst = eval(dst, env);
            let value = eval(value, env);
            let len = eval(len, env);
            let buf = if *global {
                &mut *env.gmegabuf
            } else {
                &mut *env.megabuf
            };
            if let Some(start) = mem_index(dst) {
                let end = (start + len.max(0.0) as usize).min(MEGABUF_SIZE);
                if end > start {
                    if buf.len() < end {
                        buf.resize(end, 0.0);
                    }
                    buf[start..end].fill(value);
                }
            }
            dst
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    /// Operator or punctuation.
    Op(&'static str),
}

const OPERATORS: &[&str] = &[
    "+=", "-=", "*=", "/=", "%=", "^=", "|=", "&=", "==", "!=", "<=", ">=", "&&", "||", "+", "-",
    "*", "/", "%", "^", "|", "&", "!", "<", ">", "=", "?", ":", "(", ")", ",", ";",
];

fn lex(src: &str) -> Result<Vec<Token>, String> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if src[i..].starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if src[i..].starts_with("/*") {
            i = src[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |e| i + 2 + e + 2);
        } else if c.is_ascii_digit()
            || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                let mut j = i + 1;
                if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j].is_ascii_digit() {
                    i = j;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text = &src[start..i];
            // "1.2.3" style typos: keep the leading valid number.
            let value = text
                .parse::<f64>()
                .or_else(|_| {
                    let mut dots = 0;
                    let valid: String = text
                        .chars()
                        .take_while(|&ch| {
                            if ch == '.' {
                                dots += 1;
                            }
                            dots < 2
                        })
                        .collect();
                    valid.parse::<f64>()
                })
                .map_err(|_| format!("bad number `{text}`"))?;
            tokens.push(Token::Num(value));
        } else if c == b'$' {
            let start = i + 1;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'\'')
            {
                i += 1;
            }
            let name = src[start..i].to_ascii_lowercase();
            let value = match name.as_str() {
                "pi" => std::f64::consts::PI,
                "e" => std::f64::consts::E,
                "phi" => std::f64::consts::GOLDEN_RATIO,
                n if n.starts_with('x') => i64::from_str_radix(&n[1..], 16)
                    .map_err(|_| format!("bad hex constant `${name}`"))?
                    as f64,
                n if n.starts_with('\'') && n.len() >= 3 => n.as_bytes()[1] as f64,
                _ => return Err(format!("unknown constant `${name}`")),
            };
            tokens.push(Token::Num(value));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            tokens.push(Token::Ident(src[start..i].to_ascii_lowercase()));
        } else if let Some(op) = OPERATORS.iter().find(|op| src[i..].starts_with(**op)) {
            tokens.push(Token::Op(op));
            i += op.len();
        } else {
            // Stray characters (non-ASCII in comments that lost their
            // markers, backslashes...) are skipped like ns-eel2 does.
            i += src[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    symbols: &'a mut Symbols,
}

fn assign_op(op: &str) -> Option<Option<BinOp>> {
    Some(match op {
        "=" => None,
        "+=" => Some(BinOp::Add),
        "-=" => Some(BinOp::Sub),
        "*=" => Some(BinOp::Mul),
        "/=" => Some(BinOp::Div),
        "%=" => Some(BinOp::Mod),
        "^=" => Some(BinOp::Pow),
        "|=" => Some(BinOp::BitOr),
        "&=" => Some(BinOp::BitAnd),
        _ => return None,
    })
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_op(&self) -> Option<&'static str> {
        match self.peek() {
            Some(Token::Op(op)) => Some(op),
            _ => None,
        }
    }

    fn eat(&mut self, op: &str) -> bool {
        if self.peek_op() == Some(op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, op: &str) -> Result<(), String> {
        if self.eat(op) {
            Ok(())
        } else {
            Err(format!("expected `{op}`, found {:?}", self.peek()))
        }
    }

    /// `;`-separated expressions until `end` (or the end of input).
    fn statements(&mut self, end: Option<&str>) -> Result<Vec<Expr>, String> {
        let mut items = Vec::new();
        loop {
            while self.eat(";") {}
            match self.peek_op() {
                None if self.peek().is_none() => break,
                Some(op) if Some(op) == end || op == "," => break,
                _ => {}
            }
            // A missing `;` between statements is tolerated, as many presets
            // rely on line breaks alone.
            items.push(self.assignment()?);
        }
        Ok(items)
    }

    /// A sequence used as one value: function arguments and parentheses.
    fn sequence(&mut self, end: &str) -> Result<Expr, String> {
        let mut items = self.statements(Some(end))?;
        Ok(match items.len() {
            0 => Expr::Num(0.0),
            1 => items.pop().unwrap(),
            _ => Expr::Seq(items),
        })
    }

    fn assignment(&mut self) -> Result<Expr, String> {
        let lhs = self.ternary()?;
        if let Some(op) = self.peek_op().and_then(assign_op) {
            self.pos += 1;
            let target = match lhs {
                Expr::Var(slot) => Target::Var(slot),
                Expr::Mem { global, index } => Target::Mem { global, index },
                _ => return Err("cannot assign to an expression".into()),
            };
            let rhs = self.assignment()?;
            return Ok(Expr::Assign(target, op, Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn ternary(&mut self) -> Result<Expr, String> {
        let cond = self.logic()?;
        if self.eat("?") {
            let a = self.assignment()?;
            let b = if self.eat(":") {
                self.assignment()?
            } else {
                Expr::Num(0.0)
            };
            return Ok(Expr::If(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn logic(&mut self) -> Result<Expr, String> {
        let mut lhs = self.comparison()?;
        loop {
            if self.eat("&&") {
                lhs = Expr::And(Box::new(lhs), Box::new(self.comparison()?));
            } else if self.eat("||") {
                lhs = Expr::Or(Box::new(lhs), Box::new(self.comparison()?));
            } else {
                return Ok(lhs);
            }
        }
    }

    fn binary_level(
        &mut self,
        ops: &[(&str, BinOp)],
        next: fn(&mut Self) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let mut lhs = next(self)?;
        'outer: loop {
            for (text, op) in ops {
                if self.eat(text) {
                    let rhs = next(self)?;
                    lhs = Expr::Bin(*op, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        self.binary_level(
            &[
                ("==", BinOp::Eq),
                ("!=", BinOp::Ne),
                ("<=", BinOp::Le),
                (">=", BinOp::Ge),
                ("<", BinOp::Lt),
                (">", BinOp::Gt),
            ],
            Self::bitwise,
        )
    }

    fn bitwise(&mut self) -> Result<Expr, String> {
        self.binary_level(&[("|", BinOp::BitOr), ("&", BinOp::BitAnd)], Self::additive)
    }

    fn additive(&mut self) -> Result<Expr, String> {
        self.binary_level(
            &[("+", BinOp::Add), ("-", BinOp::Sub)],
            Self::multiplicative,
        )
    }

    fn multiplicative(&mut self) -> Result<Expr, String> {
        self.binary_level(
            &[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)],
            Self::unary,
        )
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.primary()?;
        if self.eat("^") {
            // Right associative, binds tighter than unary minus on its left.
            let exp = self.unary()?;
            return Ok(Expr::Bin(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn args(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if self.eat(")") {
            return Ok(args);
        }
        loop {
            args.push(self.sequence(")")?);
            if self.eat(",") {
                continue;
            }
            self.expect(")")?;
            return Ok(args);
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Token::Num(v)) => {
                self.pos += 1;
                Ok(Expr::Num(v))
            }
            Some(Token::Op("(")) => {
                self.pos += 1;
                let e = self.sequence(")")?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Token::Ident(name)) => {
                self.pos += 1;
                if self.eat("(") {
                    let args = self.args()?;
                    return call(&name, args);
                }
                Ok(Expr::Var(self.symbols.slot(&name)))
            }
            other => Err(format!("unexpected {other:?}")),
        }
    }
}

fn call(name: &str, mut args: Vec<Expr>) -> Result<Expr, String> {
    let arity = |n: usize, args: &Vec<Expr>| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("{name}() takes {n} arguments, got {}", args.len()))
        }
    };
    let f1 = match name {
        "sin" => Some(Func1::Sin),
        "cos" => Some(Func1::Cos),
        "tan" => Some(Func1::Tan),
        "asin" => Some(Func1::Asin),
        "acos" => Some(Func1::Acos),
        "atan" => Some(Func1::Atan),
        "sqr" => Some(Func1::Sqr),
        "sqrt" => Some(Func1::Sqrt),
        "exp" => Some(Func1::Exp),
        "log" => Some(Func1::Log),
        "log10" => Some(Func1::Log10),
        "abs" => Some(Func1::Abs),
        "sign" => Some(Func1::Sign),
        "floor" => Some(Func1::Floor),
        "ceil" => Some(Func1::Ceil),
        "int" => Some(Func1::Int),
        "bnot" => Some(Func1::Bnot),
        "rand" => Some(Func1::Rand),
        "invsqrt" => Some(Func1::Invsqrt),
        _ => None,
    };
    if let Some(f) = f1 {
        arity(1, &args)?;
        return Ok(Expr::Call1(f, Box::new(args.pop().unwrap())));
    }
    let f2 = match name {
        "atan2" => Some(Func2::Atan2),
        "pow" => Some(Func2::Pow),
        "min" => Some(Func2::Min),
        "max" => Some(Func2::Max),
        "above" => Some(Func2::Above),
        "below" => Some(Func2::Below),
        "equal" => Some(Func2::Equal),
        "band" => Some(Func2::Band),
        "bor" => Some(Func2::Bor),
        "sigmoid" => Some(Func2::Sigmoid),
        _ => None,
    };
    if let Some(f) = f2 {
        arity(2, &args)?;
        let b = args.pop().unwrap();
        let a = args.pop().unwrap();
        return Ok(Expr::Call2(f, Box::new(a), Box::new(b)));
    }
    let mut take = |n| -> Result<Vec<Box<Expr>>, String> {
        arity(n, &args)?;
        Ok(args.drain(..).map(Box::new).collect())
    };
    Ok(match name {
        "if" => {
            let [c, a, b] = take(3)?.try_into().unwrap();
            Expr::If(c, a, b)
        }
        "loop" => {
            let [n, body] = take(2)?.try_into().unwrap();
            Expr::Loop(n, body)
        }
        "while" => {
            let [body] = take(1)?.try_into().unwrap();
            Expr::While(body)
        }
        "exec2" => Expr::Seq(take(2)?.into_iter().map(|b| *b).collect()),
        "exec3" => Expr::Seq(take(3)?.into_iter().map(|b| *b).collect()),
        "megabuf" | "gmegabuf" => {
            let [index] = take(1)?.try_into().unwrap();
            Expr::Mem {
                global: name == "gmegabuf",
                index,
            }
        }
        "memcpy" => {
            let [d, s, n] = take(3)?.try_into().unwrap();
            Expr::MemCopy(false, d, s, n)
        }
        "memset" => {
            let [d, v, n] = take(3)?.try_into().unwrap();
            Expr::MemSet(false, d, v, n)
        }
        // Memory is never freed early; returning the argument keeps code
        // that chains on it working.
        "freembuf" => {
            let [a] = take(1)?.try_into().unwrap();
            *a
        }
        _ => return Err(format!("unknown function `{name}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (Symbols, Vec<f64>) {
        let mut symbols = Symbols::default();
        let program = Program::compile(src, &mut symbols).unwrap();
        let mut vars = vec![0.0; symbols.len()];
        let mut megabuf = Vec::new();
        let mut gmegabuf = Vec::new();
        let mut rng = Rng::new(7);
        program.run(&mut Env {
            vars: &mut vars,
            megabuf: &mut megabuf,
            gmegabuf: &mut gmegabuf,
            rng: &mut rng,
        });
        (symbols, vars)
    }

    fn value(src: &str, name: &str) -> f64 {
        let (symbols, vars) = run(src);
        vars[symbols.get(name).unwrap()]
    }

    #[test]
    fn precedence_and_assignment() {
        assert_eq!(value("x = 1 + 2 * 3;", "x"), 7.0);
        assert_eq!(value("x = -2 ^ 2;", "x"), -4.0);
        assert_eq!(value("x = 2 ^ 3 ^ 2;", "x"), 512.0);
        assert_eq!(value("a = b = 3; x = a + b;", "x"), 6.0);
        assert_eq!(value("x = 5; x += 2; x *= 3;", "x"), 21.0);
        assert_eq!(value("X = 4; x = x + 1;", "x"), 5.0);
    }

    #[test]
    fn milkdrop_semantics() {
        assert_eq!(value("x = 1 / 0;", "x"), 0.0);
        assert_eq!(value("x = 7 % 3;", "x"), 1.0);
        assert_eq!(value("x = 7 % 0;", "x"), 0.0);
        assert_eq!(value("x = 5 & 3;", "x"), 1.0);
        assert_eq!(value("x = 5 | 3;", "x"), 7.0);
        assert_eq!(value("x = equal(0.1, 0.1000001);", "x"), 1.0);
        assert_eq!(value("x = above(2, 1) + below(2, 1);", "x"), 1.0);
        assert_eq!(value("x = sqrt(-4);", "x"), 2.0);
        assert_eq!(value("x = pow(-8, 0.5);", "x"), 0.0);
        assert_eq!(value("x = int(-2.7);", "x"), -2.0);
    }

    #[test]
    fn control_flow() {
        assert_eq!(value("y = 0; x = if(1, 2, y = 5);", "y"), 0.0);
        assert_eq!(value("x = 0; loop(10, x += 1);", "x"), 10.0);
        assert_eq!(value("x = 0; while(x += 1; x < 5);", "x"), 5.0);
        assert_eq!(value("x = 3 > 2 ? 10 : 20;", "x"), 10.0);
        assert_eq!(value("x = exec2(a = 2, a * 3);", "x"), 6.0);
        assert_eq!(value("x = (a = 2; a + 1);", "x"), 3.0);
        assert_eq!(value("x = 1 && 0 || 1;", "x"), 1.0);
        assert_eq!(value("x = !0;", "x"), 1.0);
    }

    #[test]
    fn megabuf() {
        assert_eq!(
            value("megabuf(10) = 4; x = megabuf(10) + megabuf(99999);", "x"),
            4.0
        );
        assert_eq!(
            value("gmegabuf(3) = 2; gmegabuf(3) += 1; x = gmegabuf(3);", "x"),
            3.0
        );
        assert_eq!(value("megabuf(-1) = 4; x = megabuf(-1);", "x"), 0.0);
    }

    #[test]
    fn comments_constants_and_typos() {
        assert_eq!(
            value("// c^(-1)\nx = $pi; /* y = 3; */", "x"),
            std::f64::consts::PI
        );
        assert_eq!(value("x = $x10 + .5 + 1e1;", "x"), 26.5);
        assert_eq!(value("x = 1.5.2;", "x"), 1.5);
        assert_eq!(value(";;x = 1;;", "x"), 1.0);
    }

    #[test]
    fn errors() {
        let mut symbols = Symbols::default();
        assert!(Program::compile("x = nope(1);", &mut symbols).is_err());
        assert!(Program::compile("x = (1;", &mut symbols).is_err());
        assert!(Program::compile("1 = 2;", &mut symbols).is_err());
    }
}
