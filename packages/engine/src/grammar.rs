//! Grammar-constrained decoding (phase M3): a JSON Schema becomes a byte
//! matcher, and at every step only the tokens whose bytes keep the output
//! valid can be sampled. A tool call can not come out malformed.
//!
//! The output is compact JSON (no whitespace) with object properties in
//! alphabetical order (the order `serde_json` keeps them in); optional
//! properties may be left out. Supported
//! schema subset: `object` (`properties`, `required`), `array` (`items`,
//! `minItems`, `maxItems`), `string` (`enum`, `maxLength`), `integer`,
//! `number`, `boolean`, `null`.
//!
//! Constrained decoding with a grammar: Willard and Louf, "Efficient
//! Guided Generation for Large Language Models" (2023); Geng et al.,
//! "Grammar-Constrained Decoding for Structured NLP Tasks without
//! Finetuning" (2023). JSON itself: RFC 8259.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::tokenizer::Tokenizer;

/// Characters a string may hold when the schema sets no `maxLength`, so a
/// model that never closes a string still finishes.
pub const DEFAULT_MAX_STRING: usize = 512;
/// Digits before the point, after it, and in the exponent: every number
/// the grammar allows fits an i64 (integers) or an f64 (numbers) and parses
/// everywhere.
const MAX_INT_DIGITS: usize = 15;
const MAX_FRAC_DIGITS: usize = 15;
const MAX_EXP_DIGITS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum Schema {
    Object(Vec<Property>),
    Array {
        items: Box<Schema>,
        min: usize,
        max: Option<usize>,
    },
    String {
        /// JSON-encoded contents (between the quotes) of the allowed values.
        choices: Option<Vec<Vec<u8>>>,
        max_len: usize,
    },
    Integer,
    Number,
    Boolean,
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    /// `"name":` as bytes, without the opening quote.
    key: Vec<u8>,
    pub schema: Schema,
    pub required: bool,
}

impl Schema {
    pub fn from_json(v: &Value) -> Result<Self> {
        let bad = |m: String| Error::BadHeader(format!("schema: {m}"));
        if let Some(choices) = v.get("enum") {
            let list = choices
                .as_array()
                .ok_or_else(|| bad("enum must be a list".into()))?;
            let encoded = list
                .iter()
                .map(|c| match c {
                    Value::String(_) => {
                        let s = serde_json::to_string(c).unwrap();
                        Ok(s.as_bytes()[1..s.len() - 1].to_vec())
                    }
                    _ => Err(bad("only string enums are supported".into())),
                })
                .collect::<Result<Vec<_>>>()?;
            if encoded.is_empty() {
                return Err(bad("empty enum".into()));
            }
            return Ok(Schema::String {
                choices: Some(encoded),
                max_len: usize::MAX,
            });
        }
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("every schema needs a \"type\"".into()))?;
        Ok(match ty {
            "object" => {
                let required: Vec<&str> = v
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|r| r.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let props = match v.get("properties") {
                    Some(Value::Object(map)) => map
                        .iter()
                        .map(|(name, s)| {
                            let quoted = serde_json::to_string(name).unwrap();
                            let mut key = quoted.as_bytes()[1..].to_vec();
                            key.push(b':');
                            Ok(Property {
                                name: name.clone(),
                                key,
                                schema: Schema::from_json(s)?,
                                required: required.contains(&name.as_str()),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                    _ => Vec::new(),
                };
                if let Some(missing) = required
                    .iter()
                    .find(|r| !props.iter().any(|p| p.name == **r))
                {
                    return Err(bad(format!(
                        "required property {missing} is not in properties"
                    )));
                }
                Schema::Object(props)
            }
            "array" => {
                let items = v
                    .get("items")
                    .ok_or_else(|| bad("array needs \"items\"".into()))?;
                let min = v.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
                let max = v
                    .get("maxItems")
                    .and_then(Value::as_u64)
                    .map(|m| m as usize);
                if max.is_some_and(|m| m < min) {
                    return Err(bad("maxItems is below minItems".into()));
                }
                Schema::Array {
                    items: Box::new(Schema::from_json(items)?),
                    min,
                    max,
                }
            }
            "string" => Schema::String {
                choices: None,
                max_len: v
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .map_or(DEFAULT_MAX_STRING, |m| m as usize),
            },
            "integer" => Schema::Integer,
            "number" => Schema::Number,
            "boolean" => Schema::Boolean,
            "null" => Schema::Null,
            other => return Err(bad(format!("type {other} is not supported"))),
        })
    }

    /// The shortest valid value.
    pub fn minimal(&self) -> Vec<u8> {
        match self {
            Schema::Object(props) => {
                let mut out = vec![b'{'];
                let mut first = true;
                for p in props.iter().filter(|p| p.required) {
                    if !first {
                        out.push(b',');
                    }
                    first = false;
                    out.push(b'"');
                    out.extend(&p.key);
                    out.extend(p.schema.minimal());
                }
                out.push(b'}');
                out
            }
            Schema::Array { items, min, .. } => {
                let item = items.minimal();
                let mut out = vec![b'['];
                for i in 0..*min {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend(&item);
                }
                out.push(b']');
                out
            }
            Schema::String { choices, .. } => {
                let mut out = vec![b'"'];
                if let Some(c) = choices {
                    out.extend(c.iter().min_by_key(|c| c.len()).unwrap());
                }
                out.push(b'"');
                out
            }
            Schema::Integer | Schema::Number => b"0".to_vec(),
            Schema::Boolean => b"true".to_vec(),
            Schema::Null => b"null".to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Num {
    Start,
    Minus,
    Zero,
    Int,
    Dot,
    Frac,
    E,
    ESign,
    Exp,
}

impl Num {
    fn accepting(self) -> bool {
        matches!(self, Num::Zero | Num::Int | Num::Frac | Num::Exp)
    }

    /// Whether moving from `from` to `to` stays within the digit limits
    /// (`digits` counts the current part).
    fn fits(from: Num, to: Num, digits: usize) -> bool {
        let limit = match to {
            Num::Int => MAX_INT_DIGITS,
            Num::Frac => MAX_FRAC_DIGITS,
            Num::Exp => MAX_EXP_DIGITS,
            _ => return true,
        };
        from != to || digits < limit
    }

    fn next(self, b: u8, integer: bool) -> Option<Num> {
        use Num::*;
        let digit = b.is_ascii_digit();
        Some(match (self, b) {
            (Start, b'-') => Minus,
            (Start | Minus, b'0') => Zero,
            (Start | Minus, _) if digit => Int,
            (Int, _) if digit => Int,
            (Zero | Int, b'.') if !integer => Dot,
            (Dot | Frac, _) if digit => Frac,
            (Zero | Int | Frac, b'e' | b'E') if !integer => E,
            (E, b'+' | b'-') => ESign,
            (E | ESign | Exp, _) if digit => Exp,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Escape {
    None,
    Backslash,
    /// Hex digits of `\uXXXX` still to come.
    Hex(u8),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ObjState {
    Open,
    /// After `{`: a key or `}`.
    First,
    /// After `,`: a key.
    NextKey,
    Key,
    InValue,
    /// After a value: `,` or `}`.
    After,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ArrState {
    Open,
    First,
    InItem,
    After,
}

#[derive(Debug, Clone)]
enum Frame<'s> {
    Value(&'s Schema),
    Str {
        schema: &'s Schema,
        /// Indices into the enum still matching.
        cands: Vec<usize>,
        pos: usize,
        chars: usize,
        esc: Escape,
        /// UTF-8 continuation bytes still expected, and the range the next
        /// one must be in.
        cont: u8,
        lo: u8,
        hi: u8,
    },
    Lit {
        bytes: &'static [u8],
        pos: usize,
    },
    Num {
        integer: bool,
        state: Num,
        /// Digits so far in the current part (integer, fraction, exponent).
        digits: usize,
    },
    Obj {
        props: &'s [Property],
        next: usize,
        state: ObjState,
        cands: Vec<usize>,
        pos: usize,
    },
    Arr {
        items: &'s Schema,
        min: usize,
        max: Option<usize>,
        count: usize,
        state: ArrState,
    },
}

/// Where the output stands against the schema.
#[derive(Debug, Clone)]
pub struct Matcher<'s> {
    stack: Vec<Frame<'s>>,
    done: bool,
}

impl<'s> Matcher<'s> {
    pub fn new(schema: &'s Schema) -> Self {
        Self {
            stack: vec![Frame::Value(schema)],
            done: false,
        }
    }

    /// The output is a complete value (nothing more may follow).
    pub fn is_complete(&self) -> bool {
        self.done
            || (self.stack.len() == 1
                && matches!(self.stack[0], Frame::Num { state, .. } if state.accepting()))
    }

    pub fn accepts(&self, bytes: &[u8]) -> bool {
        let mut m = self.clone();
        bytes.iter().all(|&b| m.advance(b))
    }

    /// Feeds one byte. On `false` the byte does not fit and this matcher
    /// must be dropped (callers try bytes on a clone).
    pub fn advance(&mut self, b: u8) -> bool {
        loop {
            let Some(top) = self.stack.last_mut() else {
                return false;
            };
            match top {
                Frame::Value(schema) => {
                    let schema = *schema;
                    let frame = match schema {
                        Schema::Object(props) => Frame::Obj {
                            props,
                            next: 0,
                            state: ObjState::Open,
                            cands: Vec::new(),
                            pos: 0,
                        },
                        Schema::Array { items, min, max } => Frame::Arr {
                            items,
                            min: *min,
                            max: *max,
                            count: 0,
                            state: ArrState::Open,
                        },
                        Schema::String { choices, .. } => {
                            if b != b'"' {
                                return false;
                            }
                            let cands = choices
                                .as_ref()
                                .map_or(Vec::new(), |c| (0..c.len()).collect());
                            *top = Frame::Str {
                                schema,
                                cands,
                                pos: 0,
                                chars: 0,
                                esc: Escape::None,
                                cont: 0,
                                lo: 0,
                                hi: 0,
                            };
                            return true;
                        }
                        Schema::Integer | Schema::Number => Frame::Num {
                            integer: matches!(schema, Schema::Integer),
                            state: Num::Start,
                            digits: 0,
                        },
                        Schema::Boolean => match b {
                            b't' => Frame::Lit {
                                bytes: b"true",
                                pos: 0,
                            },
                            b'f' => Frame::Lit {
                                bytes: b"false",
                                pos: 0,
                            },
                            _ => return false,
                        },
                        Schema::Null => Frame::Lit {
                            bytes: b"null",
                            pos: 0,
                        },
                    };
                    *top = frame;
                    continue;
                }
                Frame::Lit { bytes, pos } => {
                    if bytes.get(*pos) != Some(&b) {
                        return false;
                    }
                    *pos += 1;
                    if *pos == bytes.len() {
                        self.pop();
                    }
                    return true;
                }
                Frame::Num {
                    integer,
                    state,
                    digits,
                } => match state.next(b, *integer) {
                    Some(s) if Num::fits(*state, s, *digits) => {
                        // Only digits loop on a state.
                        *digits = if s == *state {
                            *digits + 1
                        } else if b.is_ascii_digit() {
                            1
                        } else {
                            0
                        };
                        *state = s;
                        return true;
                    }
                    _ if state.accepting() => {
                        // The number ends here; the byte belongs to the parent.
                        if self.stack.len() == 1 {
                            return false;
                        }
                        self.pop();
                        continue;
                    }
                    _ => return false,
                },
                Frame::Str {
                    schema,
                    cands,
                    pos,
                    chars,
                    esc,
                    cont,
                    lo,
                    hi,
                } => {
                    let Schema::String { choices, max_len } = schema else {
                        unreachable!()
                    };
                    if let Some(choices) = choices {
                        if b == b'"' {
                            if !cands.iter().any(|&c| choices[c].len() == *pos) {
                                return false;
                            }
                            self.pop();
                            return true;
                        }
                        let p = *pos;
                        cands.retain(|&c| choices[c].get(p) == Some(&b));
                        if cands.is_empty() {
                            // Leave the frame as it was.
                            return false;
                        }
                        *pos += 1;
                        return true;
                    }
                    if *cont > 0 {
                        if b < *lo || b > *hi {
                            return false;
                        }
                        *cont -= 1;
                        (*lo, *hi) = (0x80, 0xbf);
                        return true;
                    }
                    match *esc {
                        Escape::Backslash => {
                            *esc = match b {
                                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                                    Escape::None
                                }
                                b'u' => Escape::Hex(4),
                                _ => return false,
                            };
                            return true;
                        }
                        Escape::Hex(n) => {
                            if !b.is_ascii_hexdigit() {
                                return false;
                            }
                            *esc = if n == 1 {
                                Escape::None
                            } else {
                                Escape::Hex(n - 1)
                            };
                            return true;
                        }
                        Escape::None => {}
                    }
                    if b == b'"' {
                        self.pop();
                        return true;
                    }
                    if b < 0x20 || *chars >= *max_len {
                        return false;
                    }
                    match b {
                        b'\\' => *esc = Escape::Backslash,
                        0x00..=0x7f => {}
                        0xc2..=0xdf => (*cont, *lo, *hi) = (1, 0x80, 0xbf),
                        0xe0 => (*cont, *lo, *hi) = (2, 0xa0, 0xbf),
                        0xed => (*cont, *lo, *hi) = (2, 0x80, 0x9f),
                        0xe1..=0xef => (*cont, *lo, *hi) = (2, 0x80, 0xbf),
                        0xf0 => (*cont, *lo, *hi) = (3, 0x90, 0xbf),
                        0xf1..=0xf3 => (*cont, *lo, *hi) = (3, 0x80, 0xbf),
                        0xf4 => (*cont, *lo, *hi) = (3, 0x80, 0x8f),
                        _ => return false,
                    }
                    *chars += 1;
                    return true;
                }
                Frame::Obj {
                    props,
                    next,
                    state,
                    cands,
                    pos,
                } => {
                    let props: &'s [Property] = props;
                    match *state {
                        ObjState::Open => {
                            if b != b'{' {
                                return false;
                            }
                            *state = ObjState::First;
                            return true;
                        }
                        ObjState::First | ObjState::NextKey | ObjState::After => {
                            let rest = &props[*next..];
                            let can_close = rest.iter().all(|p| !p.required);
                            if b == b'}' && can_close && *state != ObjState::NextKey {
                                self.pop();
                                return true;
                            }
                            if *state == ObjState::After {
                                if b != b',' || rest.is_empty() {
                                    return false;
                                }
                                *state = ObjState::NextKey;
                                return true;
                            }
                            if b != b'"' {
                                return false;
                            }
                            // Keys up to and including the next required one.
                            let upto = rest
                                .iter()
                                .position(|p| p.required)
                                .map_or(rest.len(), |i| i + 1);
                            if upto == 0 {
                                return false;
                            }
                            *cands = (*next..*next + upto).collect();
                            *pos = 0;
                            *state = ObjState::Key;
                            return true;
                        }
                        ObjState::Key => {
                            let p = *pos;
                            let kept: Vec<usize> = cands
                                .iter()
                                .copied()
                                .filter(|&c| props[c].key.get(p) == Some(&b))
                                .collect();
                            if kept.is_empty() {
                                return false;
                            }
                            *cands = kept;
                            *pos += 1;
                            if let Some(&c) = cands.iter().find(|&&c| props[c].key.len() == *pos) {
                                *next = c + 1;
                                *state = ObjState::InValue;
                                let schema = &props[c].schema;
                                self.stack.push(Frame::Value(schema));
                            }
                            return true;
                        }
                        ObjState::InValue => unreachable!("a child frame is above"),
                    }
                }
                Frame::Arr {
                    items,
                    min,
                    max,
                    count,
                    state,
                } => {
                    let items: &'s Schema = items;
                    match *state {
                        ArrState::Open => {
                            if b != b'[' {
                                return false;
                            }
                            *state = ArrState::First;
                            return true;
                        }
                        ArrState::First => {
                            if b == b']' && *min == 0 {
                                self.pop();
                                return true;
                            }
                            if *max == Some(0) {
                                return false;
                            }
                            *state = ArrState::InItem;
                            self.stack.push(Frame::Value(items));
                            continue;
                        }
                        ArrState::After => {
                            if b == b']' && *count >= *min {
                                self.pop();
                                return true;
                            }
                            if b == b',' && max.is_none_or(|m| *count < m) {
                                *state = ArrState::InItem;
                                self.stack.push(Frame::Value(items));
                                return true;
                            }
                            return false;
                        }
                        ArrState::InItem => unreachable!("a child frame is above"),
                    }
                }
            }
        }
    }

    /// The child on top finished: the parent moves on.
    fn pop(&mut self) {
        self.stack.pop();
        match self.stack.last_mut() {
            None => self.done = true,
            Some(Frame::Obj { state, .. }) => *state = ObjState::After,
            Some(Frame::Arr { state, count, .. }) => {
                *state = ArrState::After;
                *count += 1;
            }
            Some(_) => unreachable!("only containers have children"),
        }
    }

    /// The shortest bytes that complete the output.
    pub fn finish(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (depth, frame) in self.stack.iter().enumerate().rev() {
            let has_child = depth + 1 < self.stack.len();
            match frame {
                Frame::Value(s) => out.extend(s.minimal()),
                Frame::Lit { bytes, pos } => out.extend(&bytes[*pos..]),
                Frame::Num { state, .. } => {
                    if !state.accepting() {
                        out.push(b'0');
                    }
                }
                Frame::Str {
                    schema,
                    cands,
                    pos,
                    esc,
                    cont,
                    lo,
                    ..
                } => {
                    let Schema::String { choices, .. } = schema else {
                        unreachable!()
                    };
                    if let Some(choices) = choices {
                        let best = cands
                            .iter()
                            .map(|&c| &choices[c])
                            .min_by_key(|c| c.len())
                            .unwrap();
                        out.extend(&best[*pos..]);
                    } else {
                        for i in 0..*cont {
                            out.push(if i == 0 { *lo } else { 0x80 });
                        }
                        match esc {
                            Escape::Backslash => out.push(b'n'),
                            Escape::Hex(n) => out.extend(std::iter::repeat_n(b'0', *n as usize)),
                            Escape::None => {}
                        }
                    }
                    out.push(b'"');
                }
                Frame::Obj {
                    props,
                    next,
                    state,
                    cands,
                    pos,
                } => {
                    let mut next = *next;
                    match state {
                        ObjState::Open => {
                            out.extend(Schema::Object(props.to_vec()).minimal());
                            continue;
                        }
                        ObjState::Key => {
                            let c = cands
                                .iter()
                                .copied()
                                .min_by_key(|&c| (!props[c].required, props[c].key.len()))
                                .unwrap();
                            out.extend(&props[c].key[*pos..]);
                            out.extend(props[c].schema.minimal());
                            next = c + 1;
                        }
                        ObjState::NextKey => {
                            // A key must come: the next required one, or the
                            // next optional one when none is required.
                            let rest = &props[next..];
                            let c = next + rest.iter().position(|p| p.required).unwrap_or(0);
                            out.push(b'"');
                            out.extend(&props[c].key);
                            out.extend(props[c].schema.minimal());
                            next = c + 1;
                        }
                        ObjState::InValue => debug_assert!(has_child),
                        ObjState::First | ObjState::After => {}
                    }
                    let mut first = matches!(state, ObjState::First);
                    for p in props[next..].iter().filter(|p| p.required) {
                        if !first {
                            out.push(b',');
                        }
                        first = false;
                        out.push(b'"');
                        out.extend(&p.key);
                        out.extend(p.schema.minimal());
                    }
                    out.push(b'}');
                }
                Frame::Arr {
                    items,
                    min,
                    count,
                    state,
                    ..
                } => {
                    let item = items.minimal();
                    let (mut have, mut first) = (*count, false);
                    match state {
                        ArrState::Open => {
                            out.extend(
                                Schema::Array {
                                    items: Box::new((*items).clone()),
                                    min: *min,
                                    max: None,
                                }
                                .minimal(),
                            );
                            continue;
                        }
                        ArrState::First => first = true,
                        // The item above counts once it finishes.
                        ArrState::InItem => have += 1,
                        ArrState::After => {}
                    }
                    while have < *min {
                        if !first {
                            out.push(b',');
                        }
                        first = false;
                        out.extend(&item);
                        have += 1;
                    }
                    out.push(b']');
                }
            }
        }
        out
    }
}

/// The vocabulary as a byte trie, so the tokens allowed after any state are
/// found by walking shared prefixes once.
pub struct TokenTrie {
    nodes: Vec<Node>,
}

struct Node {
    children: Vec<(u8, usize)>,
    token: Option<u32>,
}

impl TokenTrie {
    /// Every non-special token (special tokens never appear inside JSON).
    pub fn new(tokenizer: &Tokenizer) -> Self {
        let mut nodes = vec![Node {
            children: Vec::new(),
            token: None,
        }];
        let base = tokenizer.vocab_size() - tokenizer.spec().special.len();
        for id in 0..base as u32 {
            let mut at = 0;
            for b in tokenizer.decode_bytes(&[id]) {
                at = match nodes[at].children.iter().find(|(c, _)| *c == b) {
                    Some(&(_, n)) => n,
                    None => {
                        nodes.push(Node {
                            children: Vec::new(),
                            token: None,
                        });
                        let n = nodes.len() - 1;
                        nodes[at].children.push((b, n));
                        n
                    }
                };
            }
            nodes[at].token = Some(id);
        }
        Self { nodes }
    }

    /// Calls `f(token, state_after)` for every token the matcher accepts.
    pub fn allowed<'s>(&self, m: &Matcher<'s>, mut f: impl FnMut(u32, &Matcher<'s>)) {
        self.walk(0, m, &mut f);
    }

    fn walk<'s>(&self, node: usize, m: &Matcher<'s>, f: &mut impl FnMut(u32, &Matcher<'s>)) {
        for &(b, child) in &self.nodes[node].children {
            let mut next = m.clone();
            if next.advance(b) {
                if let Some(t) = self.nodes[child].token {
                    f(t, &next);
                }
                self.walk(child, &next, f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Schema {
        Schema::from_json(&json!({
            "type": "object",
            "properties": {
                "tool": {"enum": ["system.info", "system.kill", "files.read"]},
                "pid": {"type": "integer"},
                "force": {"type": "boolean"},
                "paths": {"type": "array", "items": {"type": "string", "maxLength": 8}, "maxItems": 3},
                "note": {"type": "string"}
            },
            "required": ["tool", "paths"]
        }))
        .unwrap()
    }

    fn accepts(s: &Schema, text: &str) -> bool {
        let mut m = Matcher::new(s);
        text.bytes().all(|b| m.advance(b)) && m.is_complete()
    }

    #[test]
    fn accepts_valid_and_rejects_invalid() {
        let s = schema();
        // A JSON escape built at run time: backslash, "u00e9".
        let esc = format!("{}u00e9", '\\');
        assert!(accepts(&s, r#"{"paths":[],"tool":"system.info"}"#));
        assert!(accepts(
            &s,
            &format!(
                r#"{{"force":true,"note":"x{esc}","paths":["a","é"],"pid":-42,"tool":"system.kill"}}"#
            )
        ));
        assert!(
            !accepts(&s, r#"{"paths":[],"tool":"system.reboot"}"#),
            "not in enum"
        );
        assert!(
            !accepts(&s, r#"{"tool":"files.read","paths":[]}"#),
            "alphabetical order"
        );
        assert!(
            !accepts(&s, r#"{"tool":"files.read"}"#),
            "paths is required"
        );
        assert!(
            !accepts(&s, r#"{"paths":[],"pid":1.5,"tool":"files.read"}"#),
            "integer"
        );
        assert!(
            !accepts(&s, r#"{"paths":["a","b","c","d"],"tool":"files.read"}"#),
            "maxItems"
        );
        assert!(
            !accepts(&s, r#"{"paths":["123456789"],"tool":"files.read"}"#),
            "maxLength"
        );
        assert!(
            !accepts(&s, r#"{"paths":[] ,"tool":"files.read"}"#),
            "no whitespace"
        );
        assert!(
            !accepts(&s, "{\"paths\":[\"\x01\"],\"tool\":\"files.read\"}"),
            "control char"
        );
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let s = Schema::from_json(&json!({"type": "string"})).unwrap();
        let mut m = Matcher::new(&s);
        assert!(m.advance(b'"'));
        assert!(!m.advance(0xc0));
        assert!(m.advance(0xe0));
        assert!(!m.advance(0x80), "overlong");
        assert!(m.advance(0xa0));
        assert!(!m.advance(b'"'), "inside a character");
    }

    #[test]
    fn finish_always_completes() {
        let s = schema();
        let esc = format!("{}u00e9{}n", '\\', '\\');
        let full = format!(
            r#"{{"force":true,"note":"x{esc}","paths":["ab","c"],"pid":-12.0,"tool":"system.kill"}}"#
        );
        let full = full.as_bytes();
        let mut tried = 0;
        for cut in 0..=full.len() {
            let mut m = Matcher::new(&s);
            if !full[..cut].iter().all(|&b| m.advance(b)) {
                continue;
            }
            tried += 1;
            let rest = m.finish();
            let mut done = m.clone();
            assert!(
                rest.iter().all(|&b| done.advance(b)),
                "cut {cut}: {:?}",
                String::from_utf8_lossy(&rest)
            );
            assert!(done.is_complete(), "cut {cut}");
            let text = [&full[..cut], &rest[..]].concat();
            serde_json::from_slice::<Value>(&text).unwrap();
        }
        // "-12.0" is not an integer: the walk stops there.
        assert!(tried > 60, "{tried}");
    }

    #[test]
    fn top_level_numbers_and_minimal_values() {
        let n = Schema::from_json(&json!({"type": "number"})).unwrap();
        assert!(accepts(&n, "-0.5e+3"));
        assert!(!accepts(&n, "01"));
        assert!(!accepts(&n, "1e100"), "exponent digits");
        assert!(accepts(&n, "1e99"));
        assert!(accepts(&n, "123456789012345"));
        assert!(!accepts(&n, "1234567890123456"), "integer digits");
        assert_eq!(
            schema().minimal(),
            br#"{"paths":[],"tool":"files.read"}"#.to_vec()
        );
        let s = schema();
        assert!(accepts(&s, std::str::from_utf8(&s.minimal()).unwrap()));
    }
}
