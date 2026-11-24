//! A JSON reader with the rules of ArduinoJson 7.3 (`deserializeJson()` with
//! the firmware's configuration: double precision, 64-bit integers, nesting
//! limit 10, no comments, no NaN/Infinity, Unicode escapes decoded), and its
//! `is<T>()`/`as<T>()` conversions. The HTTP API bodies are read with it, so
//! clients see the same behaviour as before, including what ArduinoJson
//! accepts beyond JSON: single quotes, unquoted keys, `+1`, `.5`, `01`.
//! Port of `Json/JsonDeserializer.hpp` and `Numbers/parseNumber.hpp`;
//! verified against ArduinoJson (`tests/http_settings.rs`).

/// A number as ArduinoJson stores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    U(u64),
    I(i64),
    F(f32),
    D(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(Num),
    /// Bytes as received (no UTF-8 check); may contain NUL.
    Str(Vec<u8>),
    Arr(Vec<Json>),
    /// Members in order; a repeated key replaces the value in place.
    Obj(Vec<(Vec<u8>, Json)>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    EmptyInput,
    IncompleteInput,
    InvalidInput,
    TooDeep,
}

const NESTING_LIMIT: u8 = 10;
const NULL: Json = Json::Null;

/// `deserializeJson(doc, input)`: the input ends at the first NUL.
pub fn deserialize(input: &[u8]) -> Result<Json, Error> {
    let input = crate::util::cutil::until_nul(input);
    let mut p = Parser {
        s: input,
        pos: 0,
        found_something: false,
    };
    let v = p.variant(NESTING_LIMIT)?;
    // a number at the top must end the input
    if matches!(v, Json::Num(_)) && p.current() != 0 {
        return Err(Error::InvalidInput);
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    found_something: bool,
}

impl Parser<'_> {
    fn current(&self) -> u8 {
        self.s.get(self.pos).copied().unwrap_or(0)
    }
    fn advance(&mut self) {
        if self.pos < self.s.len() {
            self.pos += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.current() != c {
            return false;
        }
        self.advance();
        true
    }

    fn skip_spaces(&mut self) -> Result<(), Error> {
        loop {
            match self.current() {
                0 => {
                    return Err(if self.found_something {
                        Error::IncompleteInput
                    } else {
                        Error::EmptyInput
                    })
                }
                b' ' | b'\t' | b'\r' | b'\n' => self.advance(),
                _ => {
                    self.found_something = true;
                    return Ok(());
                }
            }
        }
    }

    fn variant(&mut self, limit: u8) -> Result<Json, Error> {
        self.skip_spaces()?;
        match self.current() {
            b'[' => self.array(limit),
            b'{' => self.object(limit),
            b'"' | b'\'' => Ok(Json::Str(self.quoted_string()?)),
            b't' => self.keyword(b"true").map(|_| Json::Bool(true)),
            b'f' => self.keyword(b"false").map(|_| Json::Bool(false)),
            b'n' => self.keyword(b"null").map(|_| Json::Null),
            _ => self.number(),
        }
    }

    fn array(&mut self, limit: u8) -> Result<Json, Error> {
        if limit == 0 {
            return Err(Error::TooDeep);
        }
        self.advance(); // '['
        self.skip_spaces()?;
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.variant(limit - 1)?);
            self.skip_spaces()?;
            if self.eat(b']') {
                return Ok(Json::Arr(items));
            }
            if !self.eat(b',') {
                return Err(Error::InvalidInput);
            }
        }
    }

    fn object(&mut self, limit: u8) -> Result<Json, Error> {
        if limit == 0 {
            return Err(Error::TooDeep);
        }
        self.advance(); // '{'
        self.skip_spaces()?;
        let mut members: Vec<(Vec<u8>, Json)> = Vec::new();
        if self.eat(b'}') {
            return Ok(Json::Obj(members));
        }
        loop {
            let key = if matches!(self.current(), b'"' | b'\'') {
                self.quoted_string()?
            } else {
                self.unquoted_string()?
            };
            self.skip_spaces()?;
            if !self.eat(b':') {
                return Err(Error::InvalidInput);
            }
            let value = self.variant(limit - 1)?;
            match members.iter_mut().find(|(k, _)| *k == key) {
                Some(member) => member.1 = value,
                None => members.push((key, value)),
            }
            self.skip_spaces()?;
            if self.eat(b'}') {
                return Ok(Json::Obj(members));
            }
            if !self.eat(b',') {
                return Err(Error::InvalidInput);
            }
            self.skip_spaces()?;
        }
    }

    fn quoted_string(&mut self) -> Result<Vec<u8>, Error> {
        let stop = self.current();
        self.advance();
        let mut out = Vec::new();
        // Utf16::Codepoint: the high surrogate stays until a low one comes
        let mut high: u32 = 0;
        loop {
            let c = self.current();
            self.advance();
            if c == stop {
                return Ok(out);
            }
            if c == 0 {
                return Err(Error::IncompleteInput);
            }
            if c != b'\\' {
                out.push(c);
                continue;
            }
            let e = self.current();
            if e == 0 {
                return Err(Error::IncompleteInput);
            }
            if e == b'u' {
                self.advance();
                let unit = self.hex4()?;
                if (0xd800..0xdc00).contains(&unit) {
                    high = unit & 0x3ff;
                    continue;
                }
                let cp = if (0xdc00..0xe000).contains(&unit) {
                    0x10000 + ((high << 10) | (unit & 0x3ff))
                } else {
                    unit
                };
                encode_utf8(cp, &mut out);
                continue;
            }
            let unescaped = match e {
                b'/' => b'/',
                b'\'' => b'\'',
                b'"' => b'"',
                b'\\' => b'\\',
                b'b' => 0x08,
                b'f' => 0x0c,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                _ => return Err(Error::InvalidInput),
            };
            self.advance();
            out.push(unescaped);
        }
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let mut v = 0u32;
        for _ in 0..4 {
            let d = self.current();
            if d == 0 {
                return Err(Error::IncompleteInput);
            }
            // decodeHex(): no range check below 'A' (e.g. '/' wraps)
            let value = if d < b'A' {
                d.wrapping_sub(b'0')
            } else {
                (d & !0x20).wrapping_sub(b'A').wrapping_add(10)
            };
            if value > 0x0f {
                return Err(Error::InvalidInput);
            }
            v = ((v << 4) | value as u32) & 0xffff;
            self.advance();
        }
        Ok(v)
    }

    fn unquoted_string(&mut self) -> Result<Vec<u8>, Error> {
        let ok = |c: u8| c.is_ascii_digit() || (b'_'..=b'z').contains(&c) || c.is_ascii_uppercase();
        if !ok(self.current()) {
            return Err(Error::InvalidInput);
        }
        let start = self.pos;
        while ok(self.current()) {
            self.advance();
        }
        Ok(self.s[start..self.pos].to_vec())
    }

    fn keyword(&mut self, word: &[u8]) -> Result<(), Error> {
        for &w in word {
            let c = self.current();
            if c == 0 {
                return Err(Error::IncompleteInput);
            }
            if c != w {
                return Err(Error::InvalidInput);
            }
            self.advance();
        }
        Ok(())
    }

    /// Up to 63 characters that can be in a number, parsed by
    /// `parseNumber()`; the rest stays in the input.
    fn number(&mut self) -> Result<Json, Error> {
        let start = self.pos;
        let can_be = |c: u8| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'e' | b'E');
        while can_be(self.current()) && self.pos - start < 63 {
            self.advance();
        }
        parse_number(&self.s[start..self.pos])
            .map(Json::Num)
            .ok_or(Error::InvalidInput)
    }
}

/// `Utf8::encodeCodepoint()`.
fn encode_utf8(cp: u32, out: &mut Vec<u8>) {
    if cp < 0x80 {
        out.push(cp as u8);
        return;
    }
    let mut rev = vec![((cp | 0x80) & 0xbf) as u8];
    let mut c16 = (cp >> 6) as u16;
    if c16 < 0x20 {
        rev.push((c16 | 0xc0) as u8);
    } else {
        rev.push(((c16 | 0x80) & 0xbf) as u8);
        c16 >>= 6;
        if c16 < 0x10 {
            rev.push((c16 | 0xe0) as u8);
        } else {
            rev.push(((c16 | 0x80) & 0xbf) as u8);
            c16 >>= 6;
            rev.push((c16 | 0xf0) as u8);
        }
    }
    out.extend(rev.iter().rev());
}

const POW10_POS_F64: [u64; 9] = [
    0x4024000000000000,
    0x4059000000000000,
    0x40C3880000000000,
    0x4197D78400000000,
    0x4341C37937E08000,
    0x4693B8B5B5056E17,
    0x4D384F03E93FF9F5,
    0x5A827748F9301D32,
    0x75154FDD7F73BF3C,
];
const POW10_NEG_F64: [u64; 9] = [
    0x3FB999999999999A,
    0x3F847AE147AE147B,
    0x3F1A36E2EB1C432D,
    0x3E45798EE2308C3A,
    0x3C9CD2B297D889BC,
    0x3949F623D5A8A733,
    0x32A50FFD44F4A73D,
    0x255BBA08CF8C979D,
    0x0AC8062864AC6F43,
];
const POW10_POS_F32: [u32; 6] = [
    0x41200000, 0x42c80000, 0x461c4000, 0x4cbebc20, 0x5a0e1bca, 0x749dc5ae,
];
const POW10_NEG_F32: [u32; 6] = [
    0x3dcccccd, 0x3c23d70a, 0x38d1b717, 0x322bcc77, 0x24e69595, 0x0a4fb11f,
];

/// `make_float()`: the mantissa times 10^e by binary powers of ten.
fn make_f64(mut m: f64, e: i32) -> f64 {
    let table = if e > 0 {
        &POW10_POS_F64
    } else {
        &POW10_NEG_F64
    };
    let mut e = e.unsigned_abs();
    let mut i = 0;
    while e != 0 {
        if e & 1 != 0 {
            m *= f64::from_bits(table[i]);
        }
        e >>= 1;
        i += 1;
    }
    m
}

fn make_f32(mut m: f32, e: i32) -> f32 {
    let table = if e > 0 {
        &POW10_POS_F32
    } else {
        &POW10_NEG_F32
    };
    let mut e = e.unsigned_abs();
    let mut i = 0;
    while e != 0 {
        if e & 1 != 0 {
            m *= f32::from_bits(table[i]);
        }
        e >>= 1;
        i += 1;
    }
    m
}

/// `parseNumber()`: integers when the text is only digits (and fits),
/// floats otherwise, in single precision when exponent and mantissa allow.
pub fn parse_number(text: &[u8]) -> Option<Num> {
    const MANTISSA_MAX: u64 = (1 << 52) - 1;
    const F32_MANTISSA_MAX: u64 = (1 << 23) - 1;
    let at = |i: usize| text.get(i).copied().unwrap_or(0);
    let mut i = 0;
    let negative = match at(0) {
        b'-' => {
            i += 1;
            true
        }
        b'+' => {
            i += 1;
            false
        }
        _ => false,
    };
    if !at(i).is_ascii_digit() && at(i) != b'.' {
        return None;
    }

    let mut mantissa: u64 = 0;
    let mut exponent_offset: i16 = 0;
    while at(i).is_ascii_digit() {
        let digit = (at(i) - b'0') as u64;
        if mantissa > u64::MAX / 10 {
            break;
        }
        mantissa *= 10;
        if mantissa > u64::MAX - digit {
            break;
        }
        mantissa += digit;
        i += 1;
    }

    if at(i) == 0 {
        if negative {
            if mantissa <= 1 << 63 {
                return Some(Num::I((!mantissa).wrapping_add(1) as i64));
            }
        } else {
            return Some(Num::U(mantissa));
        }
    }

    while mantissa > MANTISSA_MAX {
        mantissa /= 10;
        exponent_offset += 1;
    }
    while at(i).is_ascii_digit() {
        exponent_offset += 1;
        i += 1;
    }
    if at(i) == b'.' {
        i += 1;
        while at(i).is_ascii_digit() {
            if mantissa < MANTISSA_MAX / 10 {
                mantissa = mantissa * 10 + (at(i) - b'0') as u64;
                exponent_offset -= 1;
            }
            i += 1;
        }
    }

    let mut exponent: i32 = 0;
    if matches!(at(i), b'e' | b'E') {
        i += 1;
        let mut negative_exponent = false;
        if at(i) == b'-' {
            negative_exponent = true;
            i += 1;
        } else if at(i) == b'+' {
            i += 1;
        }
        while at(i).is_ascii_digit() {
            exponent = exponent * 10 + (at(i) - b'0') as i32;
            if exponent + exponent_offset as i32 > 308 {
                return Some(if negative_exponent {
                    Num::F(if negative { -0.0 } else { 0.0 })
                } else {
                    Num::D(if negative {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    })
                });
            }
            i += 1;
        }
        if negative_exponent {
            exponent = -exponent;
        }
    }
    exponent += exponent_offset as i32;

    if at(i) != 0 {
        return None;
    }

    let is_double = !(-38..=38).contains(&exponent) || mantissa > F32_MANTISSA_MAX;
    Some(if is_double {
        let v = make_f64(mantissa as f64, exponent);
        Num::D(if negative { -v } else { v })
    } else {
        let v = make_f32(mantissa as f32, exponent);
        Num::F(if negative { -v } else { v })
    })
}

// ---- JsonVariant conversions ----

impl Json {
    /// `doc["key"]` (null when absent or not an object).
    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(members) => members
                .iter()
                .find(|(k, _)| k == key.as_bytes())
                .map_or(&NULL, |(_, v)| v),
            _ => &NULL,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Json::Obj(_))
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// `is<uintN_t>()` with that maximum, then `as<>()`.
    pub fn as_uint(&self, max: u64) -> Option<u64> {
        match self {
            Json::Num(Num::U(v)) if *v <= max => Some(*v),
            Json::Num(Num::I(v)) if *v >= 0 && (*v as u64) <= max => Some(*v as u64),
            _ => None,
        }
    }

    /// `is<int>()`, then `as<int>()`.
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Json::Num(Num::U(v)) => i32::try_from(*v).ok(),
            Json::Num(Num::I(v)) => i32::try_from(*v).ok(),
            _ => None,
        }
    }

    /// `is<bool>()`, then `as<bool>()`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// `is<float>()` (any number), then `as<float>()`.
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Json::Num(Num::U(v)) => Some(*v as f32),
            Json::Num(Num::I(v)) => Some(*v as f32),
            Json::Num(Num::F(f)) => Some(*f),
            Json::Num(Num::D(d)) => Some(*d as f32),
            _ => None,
        }
    }

    /// `is<const char *>()`, then `as<const char *>()`: the string as C sees
    /// it (up to the first NUL).
    pub fn as_cstr(&self) -> Option<&[u8]> {
        match self {
            Json::Str(s) => Some(crate::util::cutil::until_nul(s)),
            _ => None,
        }
    }
}

// ---- as<T>() of any value (ArduinoJson's convertNumber/asIntegral) ----

/// How ArduinoJson stores a parsed double: as float when that is lossless.
fn stored(n: Num) -> Num {
    match n {
        Num::D(d) if (d as f32) as f64 == d => Num::F(d as f32),
        n => n,
    }
}

/// The number `as<intN_t>()`/`as<uintN_t>()` converts: strings are parsed
/// (`parseNumber<T>()`), bools are 0/1, anything else has none.
fn number_of(v: &Json) -> Option<Num> {
    match v {
        Json::Num(n) => Some(stored(*n)),
        Json::Bool(b) => Some(Num::U(*b as u64)),
        Json::Str(s) => parse_number(crate::util::cutil::until_nul(s)).map(stored),
        _ => None,
    }
}

impl Json {
    /// `as<int>()`: 0 when the value does not fit.
    pub fn to_i32(&self) -> i32 {
        match number_of(self) {
            Some(Num::U(v)) if v <= i32::MAX as u64 => v as i32,
            Some(Num::I(v)) if i32::try_from(v).is_ok() => v as i32,
            // float32 -> int32: up to 2147483520.0f (highest_for<int32>)
            Some(Num::F(f)) if f >= i32::MIN as f32 && f <= f32::from_bits(0x4EFF_FFFF) => f as i32,
            Some(Num::D(d)) if d >= i32::MIN as f64 && d <= i32::MAX as f64 => d as i32,
            _ => 0,
        }
    }

    /// `as<uint32_t>()`: 0 when the value does not fit.
    pub fn to_u32(&self) -> u32 {
        match number_of(self) {
            Some(Num::U(v)) if v <= u32::MAX as u64 => v as u32,
            Some(Num::I(v)) if (0..=u32::MAX as i64).contains(&v) => v as u32,
            // float32 -> uint32: up to 4294967040.0f (highest_for<uint32>)
            Some(Num::F(f)) if f >= 0.0 && f <= f32::from_bits(0x4F7F_FFFF) => f as u32,
            Some(Num::D(d)) if d >= 0.0 && d <= u32::MAX as f64 => d as u32,
            _ => 0,
        }
    }

    /// `as<double>()`: numbers, bools (0/1) and number strings; 0 otherwise.
    pub fn to_f64(&self) -> f64 {
        match number_of(self) {
            Some(Num::U(v)) => v as f64,
            Some(Num::I(v)) => v as f64,
            Some(Num::F(f)) => f as f64,
            Some(Num::D(d)) => d,
            None => 0.0,
        }
    }

    /// `as<uint8_t>()`: 0 when the value does not fit.
    pub fn to_u8(&self) -> u8 {
        match number_of(self) {
            Some(Num::U(v)) if v <= 255 => v as u8,
            Some(Num::I(v)) if (0..=255).contains(&v) => v as u8,
            Some(Num::F(f)) if (0.0..=255.0).contains(&f) => f as u8,
            Some(Num::D(d)) if (0.0..=255.0).contains(&d) => d as u8,
            _ => 0,
        }
    }

    /// `as<uint16_t>()`: 0 when the value does not fit.
    pub fn to_u16(&self) -> u16 {
        match number_of(self) {
            Some(Num::U(v)) if v <= 65535 => v as u16,
            Some(Num::I(v)) if (0..=65535).contains(&v) => v as u16,
            Some(Num::F(f)) if (0.0..=65535.0).contains(&f) => f as u16,
            Some(Num::D(d)) if (0.0..=65535.0).contains(&d) => d as u16,
            _ => 0,
        }
    }

    /// `as<uint64_t>()`: 0 when the value does not fit.
    pub fn to_u64(&self) -> u64 {
        match number_of(self) {
            Some(Num::U(v)) => v,
            Some(Num::I(v)) if v >= 0 => v as u64,
            // highest_for<uint64>: 0x5F7FFFFF (float), 0x43EFFFFFFFFFFFFF (double)
            Some(Num::F(f)) if f >= 0.0 && f <= f32::from_bits(0x5F7F_FFFF) => f as u64,
            Some(Num::D(d)) if d >= 0.0 && d <= f64::from_bits(0x43EF_FFFF_FFFF_FFFF) => d as u64,
            _ => 0,
        }
    }

    /// `as<bool>()`: false for null, false and zero; true for strings,
    /// arrays and objects.
    pub fn truthy(&self) -> bool {
        match self {
            Json::Null => false,
            Json::Bool(b) => *b,
            Json::Num(Num::U(v)) => *v != 0,
            Json::Num(Num::I(v)) => *v != 0,
            Json::Num(Num::F(f)) => *f != 0.0,
            Json::Num(Num::D(d)) => *d != 0.0,
            _ => true,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// `v[i]` of an array: null when out of range or not an array.
    pub fn at(&self, i: usize) -> &Json {
        self.as_array().and_then(|a| a.get(i)).unwrap_or(&NULL)
    }

    /// `as<JsonArray>().size()`: 0 for non-arrays.
    pub fn len(&self) -> usize {
        self.as_array().map_or(0, <[Json]>::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Serializer (port of `Json/JsonSerializer.hpp`, `Json/TextFormatter.hpp`
// and `Numbers/FloatParts.hpp`): the output of `serializeJson()` byte for
// byte (verified against ArduinoJson when the port was made).

/// 1e1, 1e2, 1e4, ... 1e256 and their reciprocals, bit-exact as in
/// `Numbers/FloatTraits.hpp`.
const POSITIVE_POWERS: [u64; 9] = [
    0x4024000000000000,
    0x4059000000000000,
    0x40C3880000000000,
    0x4197D78400000000,
    0x4341C37937E08000,
    0x4693B8B5B5056E17,
    0x4D384F03E93FF9F5,
    0x5A827748F9301D32,
    0x75154FDD7F73BF3C,
];
const NEGATIVE_POWERS: [u64; 9] = [
    0x3FB999999999999A,
    0x3F847AE147AE147B,
    0x3F1A36E2EB1C432D,
    0x3E45798EE2308C3A,
    0x3C9CD2B297D889BC,
    0x3949F623D5A8A733,
    0x32A50FFD44F4A73D,
    0x255BBA08CF8C979D,
    0x0AC8062864AC6F43,
];

/// `normalize()`: scales `value` into [1, 10) for large and tiny values;
/// the power of ten taken out.
fn normalize(value: &mut f64) -> i16 {
    let pos = |i: usize| f64::from_bits(POSITIVE_POWERS[i]);
    let neg = |i: usize| f64::from_bits(NEGATIVE_POWERS[i]);
    let mut powers = 0i16;
    let mut index: i32 = 8;
    let mut bit: i16 = 1 << 8;
    if *value >= 1e7 {
        while index >= 0 {
            if *value >= pos(index as usize) {
                *value *= neg(index as usize);
                powers += bit;
            }
            bit >>= 1;
            index -= 1;
        }
    }
    if *value > 0.0 && *value <= 1e-5 {
        while index >= 0 {
            if *value < neg(index as usize) * 10.0 {
                *value *= pos(index as usize);
                powers -= bit;
            }
            bit >>= 1;
            index -= 1;
        }
    }
    powers
}

/// `decomposeFloat()`: integral part, decimals, exponent, decimal places.
fn decompose(mut value: f64, mut places: i8) -> (u32, u32, i16, i8) {
    let mut max_decimal = 10u32.pow(places as u32);
    let mut exponent = normalize(&mut value);
    let mut integral = value as u32;
    let mut tmp = integral;
    while tmp >= 10 {
        max_decimal /= 10;
        places -= 1;
        tmp /= 10;
    }
    let mut remainder = (value - integral as f64) * max_decimal as f64;
    let mut decimal = remainder as u32;
    remainder -= decimal as f64;
    // round up if the remainder is >= 0.5
    decimal += (remainder * 2.0) as u32;
    if decimal >= max_decimal {
        decimal = 0;
        integral += 1;
        if exponent != 0 && integral >= 10 {
            exponent += 1;
            integral = 1;
        }
    }
    while decimal % 10 == 0 && places > 0 {
        decimal /= 10;
        places -= 1;
    }
    (integral, decimal, exponent, places)
}

/// `writeFloat()`: NaN and infinities as `null` (the firmware's
/// configuration), else integral part, up to `places` decimals, exponent.
pub fn write_float(out: &mut Vec<u8>, mut value: f64, places: i8) {
    if value.is_nan() || value.is_infinite() {
        out.extend_from_slice(b"null");
        return;
    }
    if value < 0.0 {
        out.push(b'-');
        value = -value;
    }
    let (integral, decimal, exponent, places) = decompose(value, places);
    out.extend_from_slice(integral.to_string().as_bytes());
    if places > 0 {
        out.push(b'.');
        out.extend_from_slice(format!("{decimal:0w$}", w = places as usize).as_bytes());
    }
    if exponent != 0 {
        out.push(b'e');
        out.extend_from_slice(exponent.to_string().as_bytes());
    }
}

/// `writeString()`: `"` `\` and \b \f \n \r \t escaped, the other control
/// characters as `\u00XX` (ArduinoJson wrote them raw, which is not valid
/// JSON), all other bytes as they are.
pub fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'"');
    for &c in s {
        let esc = match c {
            b'"' => b'"',
            b'\\' => b'\\',
            0x08 => b'b',
            0x0c => b'f',
            b'\n' => b'n',
            b'\r' => b'r',
            b'\t' => b't',
            c if c < 0x20 => {
                out.extend_from_slice(format!("\\u{c:04x}").as_bytes());
                continue;
            }
            c => {
                out.push(c);
                continue;
            }
        };
        out.push(b'\\');
        out.push(esc);
    }
    out.push(b'"');
}

impl Num {
    /// A double as ArduinoJson stores it: as a float if that is lossless
    /// (and then written with 6 decimal places instead of 9).
    pub fn double(v: f64) -> Num {
        if (v as f32) as f64 == v || v.is_nan() {
            Num::F(v as f32)
        } else {
            Num::D(v)
        }
    }
}

impl Json {
    /// `serializeJson()`
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        match self {
            Json::Null => out.extend_from_slice(b"null"),
            Json::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Json::Num(Num::U(u)) => out.extend_from_slice(u.to_string().as_bytes()),
            Json::Num(Num::I(i)) => out.extend_from_slice(i.to_string().as_bytes()),
            Json::Num(Num::F(f)) => write_float(out, *f as f64, 6),
            // a double that fits a float losslessly is stored as a float
            // (`VariantData::setFloat()`), parsed or set
            Json::Num(Num::D(d)) => {
                write_float(out, *d, if (*d as f32) as f64 == *d { 6 } else { 9 })
            }
            Json::Str(s) => write_string(out, s),
            Json::Arr(a) => {
                out.push(b'[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    v.write(out);
                }
                out.push(b']');
            }
            Json::Obj(o) => {
                out.push(b'{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write_string(out, k);
                    out.push(b':');
                    v.write(out);
                }
                out.push(b'}');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_escaped() {
        let mut out = Vec::new();
        write_string(&mut out, b"a\x00\x01\x08\t\n\x0b\x0c\r\x1f\x7f\"\\z");
        assert_eq!(
            out,
            b"\"a\\u0000\\u0001\\b\\t\\n\\u000b\\f\\r\\u001f\x7f\\\"\\\\z\""
        );
        // valid JSON that reads back as the same bytes
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v.as_str().unwrap().as_bytes(),
            b"a\x00\x01\x08\t\n\x0b\x0c\r\x1f\x7f\"\\z"
        );
    }

    #[test]
    fn numbers_like_arduinojson() {
        assert_eq!(parse_number(b"-0"), Some(Num::I(0)));
        assert_eq!(parse_number(b"+7"), Some(Num::U(7)));
        assert_eq!(
            parse_number(b"18446744073709551615"),
            Some(Num::U(u64::MAX))
        );
        // single precision: 0.05 is 5 * 0.01f
        assert_eq!(
            parse_number(b"0.05"),
            Some(Num::F(5.0f32 * f32::from_bits(0x3c23d70a)))
        );
        assert!(matches!(parse_number(b"1e39"), Some(Num::D(_))));
        assert_eq!(parse_number(b"1x"), None);
        assert_eq!(parse_number(b"."), Some(Num::F(0.0)));
    }

    #[test]
    fn documents() {
        let doc = deserialize(b"{'a':1, b_2 : \"x\\u00e9\", \"a\":[true,null]} rest").unwrap();
        assert_eq!(doc.get("a"), &Json::Arr(vec![Json::Bool(true), Json::Null]));
        assert_eq!(doc.get("b_2").as_cstr(), Some(&b"x\xc3\xa9"[..]));
        assert_eq!(deserialize(b""), Err(Error::EmptyInput));
        assert_eq!(deserialize(b"{\"a\":"), Err(Error::IncompleteInput));
        assert_eq!(deserialize(b"12 "), Err(Error::InvalidInput));
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(deserialize(deep(10).as_bytes()).is_ok());
        assert_eq!(deserialize(deep(11).as_bytes()), Err(Error::TooDeep));
    }
}
