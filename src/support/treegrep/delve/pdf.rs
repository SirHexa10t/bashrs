//! PDF: the text of each page, as `gg` searches it — read out of the page's content streams and
//! mapped back to Unicode through each font, one line of output per line on the page.
//!
//! Hand-written, like the other decoders here (developer's call, 2026-11-05): the crates that read
//! a PDF's text (`pdf-extract`, `lopdf`) would add 31–35 packages to bashrs, most of them
//! encryption and parser machinery a text search never uses. What reading a PDF's text takes:
//!
//! - **Objects** — found by scanning the file for `N G obj` rather than trusting its
//!   cross-reference table (stale after edits, sometimes broken outright); a later definition
//!   replaces an earlier one, as an incremental update's does. Then the objects packed into object
//!   streams (PDF 1.5 and later).
//! - **Streams** — inflated (Flate, with PNG predictors) or otherwise decoded (ASCIIHex, ASCII85,
//!   RunLength). Image codecs are never needed. LZW (files from before about 2001) and encrypted
//!   files are not read: they yield no text, never garbage.
//! - **Pages** — in the page tree's order, each with the resources it inherits, and the text of
//!   the form XObjects it draws.
//! - **Text** — each show operator's bytes, split into codes and mapped through the font: its
//!   `ToUnicode` CMap where it has one (as Chrome, LibreOffice and Word write fonts), else its
//!   encoding and glyph names (as LaTeX does). Where each run lands — glyph widths advance the
//!   pen — tells lines and words apart: a jump of half an em up or down starts a line, a gap of
//!   more than [`SPACE_GAP`] em is a space, and kerning is neither.
//!
//! Bounded throughout — how much one stream may inflate to, how deep objects nest and forms draw
//! one another — and it never panics: what it can't read comes out empty.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::ops::Range;
use std::rc::Rc;

use flate2::read::{DeflateDecoder, ZlibDecoder};
use memchr::memmem;

/// Every page's text, in the page tree's order, its lines `\n`-separated — nothing for a file this
/// can't read as a PDF at all.
pub(super) fn pages(data: &[u8]) -> Result<Vec<String>, Encrypted> {
    let document = Document::scan(data);
    if document.encrypted {
        return Err(Encrypted);
    }
    let mut fonts = Fonts::new();
    Ok(document.pages().iter().map(|page| document.page_text(page, &mut fonts)).collect())
}

/// The file is encrypted: its text can't be read without decrypting it, which is left to the user —
/// even a file encrypted only to restrict printing, whose password is empty.
#[derive(Debug, PartialEq)]
pub(super) struct Encrypted;

// ---- limits and thresholds ------------------------------------------------------------------

/// How deep arrays and dictionaries may nest — and the page tree, and saved graphics states: past
/// any real file, short of a crafted one's stack overflow.
const MAX_NESTING: u32 = 64;

/// How deep form XObjects may draw one another.
const MAX_FORMS: usize = 8;

/// The most one stream may inflate to: page text is a small part of this, and no crafted stream
/// inflates without end.
const MAX_INFLATED: u64 = 64 << 20;

/// The most codes one CMap range or width run may cover — a two-byte font has 65,536 — so that a
/// crafted range over four-byte codes can't fill memory.
const MAX_RANGE: u32 = 1 << 16;

/// How many operands an operator may be given: the most any takes is six (`cm`, `Tm`); junk
/// beyond that is dropped rather than piled up.
const MAX_OPERANDS: usize = 32;

/// A run starting farther above or below the last than this fraction of an em begins a new line;
/// superscripts and subscripts stay on theirs.
const LINE_GAP: f64 = 0.5;

/// A run starting farther right of where the last ended than this fraction of an em begins a new
/// word: kerning stays well under it, and a word space (a quarter to a third of an em) well over.
const SPACE_GAP: f64 = 0.15;

/// The same, where a font gives no glyph widths and where its runs end is a guess
/// ([`GUESSED_WIDTH`] a glyph): only a gap this wide is a space, so that a bad guess never splits
/// a word.
const SPACE_GAP_GUESSED: f64 = 0.6;

/// The width guessed for a glyph whose font gives none, in thousandths of an em.
const GUESSED_WIDTH: f64 = 500.0;

// ---- syntax ---------------------------------------------------------------------------------

/// A token of PDF's syntax — the same in the file's objects, in a page's content, and in a CMap.
#[derive(Debug, PartialEq)]
enum Token<'a> {
    Number(f64),
    String(Vec<u8>),
    Name(Vec<u8>),
    ArrayStart,
    ArrayEnd,
    DictStart,
    DictEnd,
    /// Anything else: `obj`, `R`, `true`, an operator such as `Tj` or `'`.
    Word(&'a [u8]),
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).and_then(|digit| u8::try_from(digit).ok())
}

/// A word as a number, when it is one: `12`, `-3.5`, `.5`, `4.`.
fn number(word: &[u8]) -> Option<f64> {
    let text = std::str::from_utf8(word).ok()?;
    let body = text.strip_prefix(|c| c == '+' || c == '-').unwrap_or(text);
    let digits = body.bytes().filter(u8::is_ascii_digit).count();
    let dots = body.bytes().filter(|&byte| byte == b'.').count();
    if digits == 0 || dots > 1 || digits + dots != body.len() {
        return None;
    }
    text.parse().ok()
}

/// Reads tokens and objects from bytes, never past their end.
struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Lexer { data, pos }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Steps over white space and comments.
    fn skip_space(&mut self) {
        while let Some(byte) = self.peek() {
            if is_space(byte) {
                self.pos += 1;
            } else if byte == b'%' {
                while self.peek().is_some_and(|byte| byte != b'\n' && byte != b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token — `None` at the end.
    fn token(&mut self) -> Option<Token<'a>> {
        self.skip_space();
        let byte = self.peek()?;
        let start = self.pos;
        self.pos += 1;
        Some(match byte {
            b'(' => Token::String(self.literal_string()),
            b'<' if self.peek() == Some(b'<') => {
                self.pos += 1;
                Token::DictStart
            }
            b'<' => Token::String(self.hex_string()),
            b'>' if self.peek() == Some(b'>') => {
                self.pos += 1;
                Token::DictEnd
            }
            b'[' => Token::ArrayStart,
            b']' => Token::ArrayEnd,
            b'/' => Token::Name(self.name()),
            // A stray delimiter — a lone `)` or `>`, a PostScript brace — stands alone.
            b')' | b'>' | b'{' | b'}' => Token::Word(&self.data[start..self.pos]),
            _ => {
                while self.peek().is_some_and(|byte| !is_space(byte) && !is_delimiter(byte)) {
                    self.pos += 1;
                }
                let word = &self.data[start..self.pos];
                number(word).map_or(Token::Word(word), Token::Number)
            }
        })
    }

    /// A `( … )` string's bytes, its opening parenthesis already read: nested parentheses kept,
    /// escapes resolved, an escaped end of line dropped.
    fn literal_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'(' => {
                    depth += 1;
                    out.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(byte);
                }
                b'\\' => {
                    let Some(escaped) = self.peek() else { break };
                    self.pos += 1;
                    match escaped {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut value = u32::from(escaped - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(digit @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(digit - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(u8::try_from(value & 0xff).unwrap_or(0));
                        }
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(byte),
            }
        }
        out
    }

    /// A `< … >` string's bytes, its `<` already read; an odd last digit is padded with 0.
    fn hex_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut high = None;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            if byte == b'>' {
                break;
            }
            let Some(nibble) = hex_digit(byte) else { continue };
            match high.take() {
                Some(high) => out.push(high << 4 | nibble),
                None => high = Some(nibble),
            }
        }
        if let Some(high) = high {
            out.push(high << 4);
        }
        out
    }

    /// A name's bytes, its `/` already read, `#xx` escapes resolved.
    fn name(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(byte) = self.peek() {
            if is_space(byte) || is_delimiter(byte) {
                break;
            }
            self.pos += 1;
            if byte == b'#' {
                let high = self.data.get(self.pos).copied().and_then(hex_digit);
                let low = self.data.get(self.pos + 1).copied().and_then(hex_digit);
                if let (Some(high), Some(low)) = (high, low) {
                    out.push(high << 4 | low);
                    self.pos += 2;
                    continue;
                }
            }
            out.push(byte);
        }
        out
    }

    /// The next object — `None` at the end, or where no object can start.
    fn object(&mut self, depth: u32) -> Option<Object> {
        let token = self.token()?;
        self.object_from(token, depth)
    }

    /// The object `token` begins.
    fn object_from(&mut self, token: Token<'a>, depth: u32) -> Option<Object> {
        if depth > MAX_NESTING {
            return None;
        }
        Some(match token {
            Token::Number(value) => match self.reference_after(value) {
                Some(number) => Object::Reference(number),
                None => Object::Number(value),
            },
            Token::String(bytes) => Object::String(bytes),
            Token::Name(name) => Object::Name(name),
            // Junk inside an array or a dictionary — a stray word, a key with no value — is stepped
            // over; only the end of the data cuts one short.
            Token::ArrayStart => {
                let mut items = Vec::new();
                loop {
                    match self.token()? {
                        Token::ArrayEnd => break,
                        token => items.extend(self.object_from(token, depth + 1)),
                    }
                }
                Object::Array(items)
            }
            Token::DictStart => {
                let mut dict = Dict::new();
                loop {
                    match self.token()? {
                        Token::DictEnd => break,
                        Token::Name(key) => match self.token()? {
                            Token::DictEnd => break,
                            token => {
                                if let Some(value) = self.object_from(token, depth + 1) {
                                    dict.insert(key, value);
                                }
                            }
                        },
                        _ => {}
                    }
                }
                Object::Dict(dict)
            }
            Token::Word(b"true") => Object::Bool(true),
            Token::Word(b"false") => Object::Bool(false),
            Token::Word(b"null") => Object::Null,
            Token::ArrayEnd | Token::DictEnd | Token::Word(_) => return None,
        })
    }

    /// The object number of a reference — `N G R` — whose `N` was just read as `first`; the
    /// lexer is left where it was when what follows isn't one.
    fn reference_after(&mut self, first: f64) -> Option<u32> {
        let save = self.pos;
        let number = as_code(first).filter(|_| {
            matches!(self.token(), Some(Token::Number(generation)) if as_code(generation).is_some())
                && matches!(self.token(), Some(Token::Word(b"R")))
        });
        if number.is_none() {
            self.pos = save;
        }
        number
    }

    /// Steps over an inline image, `BI` already read: its parameters up to `ID`, then its data
    /// up to an `EI` standing alone.
    fn skip_inline_image(&mut self) {
        while let Some(token) = self.token() {
            if matches!(token, Token::Word(b"ID")) {
                break;
            }
        }
        let mut at = self.pos + 1;
        while let Some(found) = self.data.get(at..).and_then(|rest| memmem::find(rest, b"EI")) {
            let found = at + found;
            let before = found > 0 && self.data.get(found - 1).is_some_and(|&byte| is_space(byte));
            let after = self.data.get(found + 2).is_none_or(|&byte| is_space(byte));
            if before && after {
                self.pos = found + 2;
                return;
            }
            at = found + 2;
        }
        self.pos = self.data.len();
    }
}

/// A whole number that fits a code, an object number or a count.
fn as_code(value: f64) -> Option<u32> {
    (value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

/// A whole number that fits an offset or a length.
fn as_index(value: f64) -> Option<usize> {
    as_code(value).and_then(|value| usize::try_from(value).ok())
}

/// Up to four bytes, big-endian, as one number — a code, or a CMap's bound.
fn be(bytes: &[u8]) -> u32 {
    bytes.iter().take(4).fold(0, |value, &byte| value << 8 | u32::from(byte))
}

// ---- objects --------------------------------------------------------------------------------

type Dict = HashMap<Vec<u8>, Object>;

/// A PDF object. A reference keeps only its object number: the scan keeps one definition per
/// number, the newest, whatever its generation.
#[derive(Clone, Debug, PartialEq)]
enum Object {
    Null,
    Bool(bool),
    Number(f64),
    String(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Object>),
    Dict(Dict),
    Reference(u32),
    /// A stream: its dictionary, and where its (still encoded) data lies in the file.
    Stream(Dict, Range<usize>),
}

/// What an unknown key or a dangling reference resolves to.
static NULL: Object = Object::Null;

impl Object {
    fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(dict) | Object::Stream(dict, _) => Some(dict),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(items) => Some(items),
            _ => None,
        }
    }

    fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(name) => Some(name),
            _ => None,
        }
    }

    fn as_number(&self) -> Option<f64> {
        match self {
            Object::Number(value) => Some(*value),
            _ => None,
        }
    }
}

/// Whether a dictionary's `/Type` — written directly, as it always is — is `kind`.
fn is_type(dict: &Dict, kind: &[u8]) -> bool {
    dict.get(b"Type".as_slice()).and_then(Object::as_name) == Some(kind)
}

/// The object number of the `N G obj` whose `obj` starts at `at`: two whole numbers before it,
/// the first standing on its own.
fn object_number_before(data: &[u8], at: usize) -> Option<u32> {
    let mut pos = at;
    let skip_space = |pos: &mut usize| {
        let start = *pos;
        while *pos > 0 && data.get(*pos - 1).is_some_and(|&byte| is_space(byte)) {
            *pos -= 1;
        }
        *pos < start
    };
    let digits = |pos: &mut usize| {
        let end = *pos;
        while *pos > 0 && data.get(*pos - 1).is_some_and(u8::is_ascii_digit) {
            *pos -= 1;
        }
        (*pos < end).then(|| data.get(*pos..end)).flatten()
    };
    if !skip_space(&mut pos) {
        return None;
    }
    digits(&mut pos)?;
    if !skip_space(&mut pos) {
        return None;
    }
    let number = digits(&mut pos)?;
    if pos > 0 && data.get(pos - 1).is_some_and(|&byte| !is_space(byte) && !is_delimiter(byte)) {
        return None;
    }
    std::str::from_utf8(number).ok()?.parse().ok()
}

/// The data of the stream that follows `dict`, when one does — leaving `lexer` past its
/// `endstream`.
fn stream_after(data: &[u8], lexer: &mut Lexer<'_>, dict: &Dict) -> Option<Range<usize>> {
    let mut at = lexer.pos;
    while data.get(at).is_some_and(|&byte| is_space(byte)) {
        at += 1;
    }
    if !data.get(at..)?.starts_with(b"stream") {
        return None;
    }
    let mut start = at + 6;
    if data.get(start) == Some(&b'\r') {
        start += 1;
    }
    if data.get(start) == Some(&b'\n') {
        start += 1;
    }
    let (end, resume) = stream_end(data, dict, start)?;
    lexer.pos = resume;
    Some(start..end)
}

/// Where a stream's data ends, and where reading resumes past its `endstream`: at its `/Length`
/// when that is a plain number with `endstream` after it — else at the next `endstream`, since a
/// length given by reference may only be defined further on.
fn stream_end(data: &[u8], dict: &Dict, start: usize) -> Option<(usize, usize)> {
    let length = dict.get(b"Length".as_slice()).and_then(Object::as_number).and_then(as_index);
    if let Some(end) = length.and_then(|length| start.checked_add(length)) {
        let mut after = end;
        while data.get(after).is_some_and(|&byte| is_space(byte)) {
            after += 1;
        }
        if data.get(after..).is_some_and(|rest| rest.starts_with(b"endstream")) {
            return Some((end, after + 9));
        }
    }
    let found = start + memmem::find(data.get(start..)?, b"endstream")?;
    let mut end = found;
    if end > start && data.get(end - 1) == Some(&b'\n') {
        end -= 1;
    }
    if end > start && data.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    Some((end, found + 9))
}

/// A file's objects, found by scanning, and what its trailers say.
struct Document<'a> {
    data: &'a [u8],
    objects: HashMap<u32, Object>,
    /// The catalog's object number, from the newest trailer.
    root: Option<u32>,
    /// Whether a trailer names an encryption dictionary.
    encrypted: bool,
}

/// A page to read: its dictionary, and the resources it draws with — its own, or inherited from
/// the page tree above it.
struct Page<'d> {
    dict: &'d Dict,
    resources: Option<&'d Dict>,
}

/// The fonts loaded so far, by object number: most pages share theirs.
type Fonts = HashMap<u32, Rc<Font>>;

impl<'a> Document<'a> {
    /// Every object in `data`, the newest definition of each number kept, then those packed in
    /// object streams; and the trailers' root and encryption.
    fn scan(data: &'a [u8]) -> Self {
        let mut document = Document { data, objects: HashMap::new(), root: None, encrypted: false };
        let mut roots = Vec::new();
        let finder = memmem::Finder::new(b"obj");
        let mut pos = 0;
        while let Some(found) = data.get(pos..).and_then(|rest| finder.find(rest)).map(|at| pos + at) {
            pos = found + 3;
            if data.get(pos).is_some_and(|&byte| !is_space(byte) && !is_delimiter(byte)) {
                continue;
            }
            let Some(number) = object_number_before(data, found) else { continue };
            let mut lexer = Lexer::new(data, pos);
            let Some(object) = lexer.object(0) else { continue };
            let object = match object {
                Object::Dict(dict) => match stream_after(data, &mut lexer, &dict) {
                    Some(range) => Object::Stream(dict, range),
                    None => Object::Dict(dict),
                },
                other => other,
            };
            pos = pos.max(lexer.pos);
            // An XRef stream's dictionary carries what a classic trailer would.
            if let Some(dict) = object.as_dict().filter(|dict| is_type(dict, b"XRef")) {
                note_trailer(found, dict, &mut roots, &mut document.encrypted);
            }
            document.objects.insert(number, object);
        }
        for at in memmem::find_iter(data, b"trailer") {
            if let Some(Object::Dict(dict)) = Lexer::new(data, at + 7).object(0) {
                note_trailer(at, &dict, &mut roots, &mut document.encrypted);
            }
        }
        document.root = roots.iter().max_by_key(|(at, _)| *at).map(|(_, root)| *root);
        document.unpack_object_streams();
        document
    }

    /// Adds the objects packed in object streams — each stream's header lists object numbers with
    /// their offsets past `/First` — where no plain definition already gave the number.
    fn unpack_object_streams(&mut self) {
        let packed: Vec<(Dict, Range<usize>)> = self
            .objects
            .values()
            .filter_map(|object| match object {
                Object::Stream(dict, range) if is_type(dict, b"ObjStm") => {
                    Some((dict.clone(), range.clone()))
                }
                _ => None,
            })
            .collect();
        for (dict, range) in packed {
            let Some(data) = self.data.get(range).and_then(|raw| self.decode(&dict, raw)) else { continue };
            let count = dict.get(b"N".as_slice()).and_then(Object::as_number).and_then(as_index).unwrap_or(0);
            let Some(first) = dict.get(b"First".as_slice()).and_then(Object::as_number).and_then(as_index) else {
                continue;
            };
            let mut header = Lexer::new(&data, 0);
            let mut entries = Vec::new();
            while entries.len() < count.min(MAX_RANGE as usize) {
                let (Some(Token::Number(number)), Some(Token::Number(offset))) = (header.token(), header.token()) else {
                    break;
                };
                entries.push((number, offset));
            }
            for (number, offset) in entries {
                let Some(number) = as_code(number) else { continue };
                let Some(at) = as_index(offset).and_then(|offset| first.checked_add(offset)) else { continue };
                if self.objects.contains_key(&number) {
                    continue;
                }
                if let Some(object) = Lexer::new(&data, at).object(0) {
                    self.objects.insert(number, object);
                }
            }
        }
    }

    /// What `object` is, references followed.
    fn resolve<'s>(&'s self, object: &'s Object) -> &'s Object {
        let mut object = object;
        for _ in 0..MAX_NESTING {
            let Object::Reference(number) = object else { return object };
            object = self.objects.get(number).unwrap_or(&NULL);
        }
        &NULL
    }

    /// `dict`'s `key`, references followed.
    fn get<'s>(&'s self, dict: &'s Dict, key: &[u8]) -> &'s Object {
        dict.get(key).map_or(&NULL, |object| self.resolve(object))
    }

    /// A stream's data, decoded through its filters; `None` for a filter not read here.
    fn decode(&self, dict: &Dict, raw: &[u8]) -> Option<Vec<u8>> {
        let filters: Vec<&[u8]> = match self.get(dict, b"Filter") {
            Object::Null => Vec::new(),
            Object::Name(name) => vec![name],
            Object::Array(items) => items.iter().filter_map(|item| self.resolve(item).as_name()).collect(),
            _ => return None,
        };
        let params: Vec<Option<&Dict>> = match self.get(dict, b"DecodeParms") {
            Object::Dict(params) => vec![Some(params)],
            Object::Array(items) => items.iter().map(|item| self.resolve(item).as_dict()).collect(),
            _ => Vec::new(),
        };
        let mut data = raw.to_vec();
        for (index, filter) in filters.into_iter().enumerate() {
            data = match filter {
                b"FlateDecode" | b"Fl" => inflate(&data),
                b"ASCIIHexDecode" | b"AHx" => ascii_hex(&data),
                b"ASCII85Decode" | b"A85" => ascii85(&data),
                b"RunLengthDecode" | b"RL" => run_length(&data),
                _ => return None,
            };
            if let Some(params) = params.get(index).copied().flatten() {
                data = self.unpredict(data, params)?;
            }
        }
        Some(data)
    }

    /// `data` with a PNG predictor undone, as `params` describe it — rows of samples, each led by
    /// its filter type. Other predictors are left be: no text stream uses them.
    fn unpredict(&self, data: Vec<u8>, params: &Dict) -> Option<Vec<u8>> {
        let number = |key: &[u8], default: usize| {
            self.get(params, key).as_number().and_then(as_index).unwrap_or(default)
        };
        if number(b"Predictor", 1) < 10 {
            return Some(data);
        }
        let bits = number(b"Colors", 1).checked_mul(number(b"BitsPerComponent", 8))?;
        let row = number(b"Columns", 1).checked_mul(bits)?.checked_add(7)? / 8;
        let pixel = (bits / 8).max(1);
        if row == 0 || row > 1 << 24 {
            return None;
        }
        let mut out = Vec::with_capacity(data.len());
        let mut previous = vec![0u8; row];
        for chunk in data.chunks(row + 1) {
            let Some((&kind, samples)) = chunk.split_first() else { continue };
            let mut current = samples.to_vec();
            current.resize(row, 0);
            for index in 0..row {
                let left = if index >= pixel { current[index - pixel] } else { 0 };
                let up = previous[index];
                let up_left = if index >= pixel { previous[index - pixel] } else { 0 };
                let predicted = match kind {
                    1 => left,
                    2 => up,
                    3 => u8::try_from((u16::from(left) + u16::from(up)) / 2).unwrap_or(0),
                    4 => paeth(left, up, up_left),
                    _ => 0,
                };
                current[index] = current[index].wrapping_add(predicted);
            }
            out.extend_from_slice(&current[..samples.len().min(row)]);
            previous = current;
        }
        Some(out)
    }

    /// The pages, in the page tree's order — or, where the tree can't be found, every page object
    /// in the order of its number.
    fn pages(&self) -> Vec<Page<'_>> {
        let mut pages = Vec::new();
        let catalog = self.root.and_then(|root| self.objects.get(&root)).and_then(Object::as_dict).or_else(|| {
            self.objects
                .iter()
                .filter(|(_, object)| object.as_dict().is_some_and(|dict| is_type(dict, b"Catalog")))
                .max_by_key(|(number, _)| **number)
                .and_then(|(_, object)| object.as_dict())
        });
        if let Some(tree) = catalog.and_then(|catalog| catalog.get(b"Pages".as_slice())) {
            self.walk(tree, None, 0, &mut HashSet::new(), &mut pages);
        }
        if pages.is_empty() {
            let mut numbered: Vec<(u32, &Dict)> = self
                .objects
                .iter()
                .filter_map(|(number, object)| object.as_dict().map(|dict| (*number, dict)))
                .filter(|(_, dict)| is_type(dict, b"Page"))
                .collect();
            numbered.sort_by_key(|(number, _)| *number);
            pages = numbered
                .into_iter()
                .map(|(_, dict)| Page { dict, resources: self.get(dict, b"Resources").as_dict() })
                .collect();
        }
        pages
    }

    /// Collects the pages below `node`, each node's own resources replacing those it inherits;
    /// a node seen before (a cycle) is passed over.
    fn walk<'d>(
        &'d self,
        node: &'d Object,
        inherited: Option<&'d Dict>,
        depth: u32,
        visited: &mut HashSet<u32>,
        pages: &mut Vec<Page<'d>>,
    ) {
        if depth > MAX_NESTING {
            return;
        }
        if let Object::Reference(number) = node {
            if !visited.insert(*number) {
                return;
            }
        }
        let Some(dict) = self.resolve(node).as_dict() else { return };
        let resources = self.get(dict, b"Resources").as_dict().or(inherited);
        match self.get(dict, b"Kids").as_array() {
            Some(kids) => {
                for kid in kids {
                    self.walk(kid, resources, depth + 1, visited, pages);
                }
            }
            None => pages.push(Page { dict, resources }),
        }
    }

    /// One page's text.
    fn page_text(&self, page: &Page<'_>, fonts: &mut Fonts) -> String {
        let mut text = Text::default();
        let content = self.content_of(self.get(page.dict, b"Contents"));
        self.run(&content, page.resources, State::new(Matrix::IDENTITY), &mut text, fonts, &mut Vec::new());
        text.finish()
    }

    /// A page's content: one stream, or several drawn in turn.
    fn content_of(&self, contents: &Object) -> Vec<u8> {
        let parts: Vec<&Object> = match contents {
            Object::Array(parts) => parts.iter().collect(),
            single => vec![single],
        };
        let mut content = Vec::new();
        for part in parts {
            if let Object::Stream(dict, range) = self.resolve(part) {
                if let Some(data) = self.data.get(range.clone()).and_then(|raw| self.decode(dict, raw)) {
                    content.extend_from_slice(&data);
                    content.push(b'\n');
                }
            }
        }
        content
    }

    /// Draws `content`, sending what it shows to `text`: the operators that set the font, place
    /// the text and show it, and the form XObjects it draws. Everything else is passed over.
    fn run<'d>(
        &'d self,
        content: &[u8],
        resources: Option<&'d Dict>,
        initial: State,
        text: &mut Text,
        fonts: &mut Fonts,
        forms: &mut Vec<u32>,
    ) {
        let mut lexer = Lexer::new(content, 0);
        let mut operands: Vec<Object> = Vec::new();
        let mut state = initial;
        let mut saved: Vec<State> = Vec::new();
        let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
        while let Some(token) = lexer.token() {
            let Token::Word(operator) = token else {
                match lexer.object_from(token, 0) {
                    Some(operand) if operands.len() < MAX_OPERANDS => operands.push(operand),
                    _ => operands.clear(),
                }
                continue;
            };
            match operator {
                b"q" => {
                    if saved.len() < MAX_NESTING as usize {
                        saved.push(state.clone());
                    }
                }
                b"Q" => {
                    if let Some(previous) = saved.pop() {
                        state = previous;
                    }
                }
                b"cm" => {
                    if let Some(matrix) = last_numbers(&operands).map(Matrix) {
                        state.ctm = matrix.then(state.ctm);
                    }
                }
                b"BT" => (tm, tlm) = (Matrix::IDENTITY, Matrix::IDENTITY),
                b"Tf" => {
                    if let [.., Object::Name(name), size] = operands.as_slice() {
                        state.font = self.font(resources, name, fonts);
                        state.size = size.as_number().unwrap_or(0.0);
                    }
                }
                b"Tc" => set_last(&operands, &mut state.char_spacing),
                b"Tw" => set_last(&operands, &mut state.word_spacing),
                b"TL" => set_last(&operands, &mut state.leading),
                b"Ts" => set_last(&operands, &mut state.rise),
                b"Tz" => {
                    if let Some([percent]) = last_numbers(&operands) {
                        state.scale = percent / 100.0;
                    }
                }
                b"Td" | b"TD" => {
                    if let Some([x, y]) = last_numbers(&operands) {
                        if operator == b"TD" {
                            state.leading = -y;
                        }
                        tlm = Matrix::translate(x, y).then(tlm);
                        tm = tlm;
                    }
                }
                b"Tm" => {
                    if let Some(matrix) = last_numbers(&operands).map(Matrix) {
                        (tm, tlm) = (matrix, matrix);
                    }
                }
                b"T*" => next_line(&mut tm, &mut tlm, &state),
                b"Tj" => {
                    if let Some(Object::String(bytes)) = operands.last() {
                        show(bytes, &state, &mut tm, text);
                    }
                }
                b"'" => {
                    next_line(&mut tm, &mut tlm, &state);
                    if let Some(Object::String(bytes)) = operands.last() {
                        show(bytes, &state, &mut tm, text);
                    }
                }
                b"\"" => {
                    if let [.., word_spacing, char_spacing, Object::String(bytes)] = operands.as_slice() {
                        state.word_spacing = word_spacing.as_number().unwrap_or(state.word_spacing);
                        state.char_spacing = char_spacing.as_number().unwrap_or(state.char_spacing);
                        next_line(&mut tm, &mut tlm, &state);
                        show(bytes, &state, &mut tm, text);
                    }
                }
                b"TJ" => {
                    if let Some(Object::Array(items)) = operands.last() {
                        for item in items {
                            match item {
                                Object::String(bytes) => show(bytes, &state, &mut tm, text),
                                Object::Number(adjust) => {
                                    let step = -adjust / 1000.0 * state.size * state.scale;
                                    tm = Matrix::translate(step, 0.0).then(tm);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                b"Do" => {
                    if let Some(Object::Name(name)) = operands.last() {
                        self.draw_form(name, resources, &state, text, fonts, forms);
                    }
                }
                b"BI" => lexer.skip_inline_image(),
                _ => {}
            }
            operands.clear();
        }
    }

    /// Draws the form XObject `name` — its own content, with its own resources (or the drawer's),
    /// placed by its matrix — unless it is already being drawn, or forms nest too deep.
    fn draw_form<'d>(
        &'d self,
        name: &[u8],
        resources: Option<&'d Dict>,
        state: &State,
        text: &mut Text,
        fonts: &mut Fonts,
        forms: &mut Vec<u32>,
    ) {
        let xobjects = resources.map(|resources| self.get(resources, b"XObject")).and_then(Object::as_dict);
        let Some(entry) = xobjects.and_then(|xobjects| xobjects.get(name)) else { return };
        let Object::Reference(number) = entry else { return };
        if forms.len() >= MAX_FORMS || forms.contains(number) {
            return;
        }
        let Object::Stream(dict, range) = self.resolve(entry) else { return };
        if self.get(dict, b"Subtype").as_name() != Some(b"Form".as_slice()) {
            return;
        }
        let Some(content) = self.data.get(range.clone()).and_then(|raw| self.decode(dict, raw)) else { return };
        let matrix = self.get(dict, b"Matrix").as_array().and_then(last_numbers).map_or(Matrix::IDENTITY, Matrix);
        let own = self.get(dict, b"Resources").as_dict().or(resources);
        let mut inside = state.clone();
        inside.ctm = matrix.then(state.ctm);
        forms.push(*number);
        self.run(&content, own, inside, text, fonts, forms);
        forms.pop();
    }

    /// The font `name` in `resources`, loaded once per document.
    fn font(&self, resources: Option<&Dict>, name: &[u8], fonts: &mut Fonts) -> Option<Rc<Font>> {
        let entry = resources.map(|resources| self.get(resources, b"Font"))?.as_dict()?.get(name)?;
        let Object::Reference(number) = entry else {
            return Some(Rc::new(self.load_font(entry.as_dict()?)));
        };
        if let Some(font) = fonts.get(number) {
            return Some(Rc::clone(font));
        }
        let font = Rc::new(self.load_font(self.resolve(entry).as_dict()?));
        fonts.insert(*number, Rc::clone(&font));
        Some(font)
    }

    /// What a font's codes are: how its bytes split, each code's text, each glyph's width.
    fn load_font(&self, font: &Dict) -> Font {
        let subtype = self.get(font, b"Subtype").as_name().unwrap_or_default();
        let cmap = match self.get(font, b"ToUnicode") {
            Object::Stream(dict, range) => {
                self.data.get(range.clone()).and_then(|raw| self.decode(dict, raw)).map(|data| CMap::parse(&data))
            }
            _ => None,
        };
        let mut loaded = if subtype == b"Type0" {
            self.composite_font(font, cmap.as_ref())
        } else {
            self.simple_font(font, subtype)
        };
        if let Some(cmap) = cmap {
            // The font's own map to Unicode outranks anything its encoding implies — but not an
            // entry to control characters alone, a broken map's (one maps `c` to a carriage
            // return, which would take the letter out of every word).
            let printable = cmap.text.into_iter().filter(|(_, text)| !text.chars().all(char::is_control));
            loaded.text.extend(printable);
        }
        loaded
    }

    /// A Type0 font: two-byte codes under an `Identity` or Unicode CMap — else as its `ToUnicode`
    /// CMap's code space splits them — and widths from its CID font.
    fn composite_font(&self, font: &Dict, cmap: Option<&CMap>) -> Font {
        let encoding = self.get(font, b"Encoding").as_name().unwrap_or_default();
        let codes_are_unicode = [b"UCS2-H".as_slice(), b"UCS2-V", b"UTF16-H", b"UTF16-V"]
            .iter()
            .any(|suffix| encoding.ends_with(suffix));
        let identity = encoding == b"Identity-H" || encoding == b"Identity-V";
        let codes = match cmap {
            Some(cmap) if !identity && !codes_are_unicode && !cmap.ranges.is_empty() => {
                Codes::Ranges(cmap.ranges.clone())
            }
            _ => Codes::TwoBytes,
        };
        let descendant = self
            .get(font, b"DescendantFonts")
            .as_array()
            .and_then(<[Object]>::first)
            .and_then(|first| self.resolve(first).as_dict());
        let (widths, default_width) = match descendant {
            Some(cid_font) => (
                self.cid_widths(cid_font),
                self.get(cid_font, b"DW").as_number().unwrap_or(1000.0),
            ),
            None => (HashMap::new(), 1000.0),
        };
        Font { codes, text: HashMap::new(), codes_are_unicode, widths, default_width: Some(default_width), em: 1.0 }
    }

    /// A CID font's `/W` widths: `c [w …]` for a run of codes from `c`, `c₁ c₂ w` for a range.
    fn cid_widths(&self, cid_font: &Dict) -> HashMap<u32, f64> {
        let mut widths = HashMap::new();
        let Some(items) = self.get(cid_font, b"W").as_array() else { return widths };
        let mut items = items.iter().map(|item| self.resolve(item));
        while let Some(first) = items.next().and_then(Object::as_number).and_then(as_code) {
            match items.next() {
                Some(Object::Array(run)) => {
                    for (offset, width) in (0..MAX_RANGE).zip(run) {
                        let code = first.checked_add(offset);
                        if let (Some(code), Some(width)) = (code, self.resolve(width).as_number()) {
                            widths.insert(code, width);
                        }
                    }
                }
                Some(last) => {
                    let Some(last) = last.as_number().and_then(as_code) else { break };
                    let Some(width) = items.next().and_then(Object::as_number) else { break };
                    if last >= first && last - first < MAX_RANGE {
                        widths.extend((first..=last).map(|code| (code, width)));
                    }
                }
                None => break,
            }
        }
        widths
    }

    /// A simple font — Type1, TrueType, Type3: one byte a code, each code's text from its
    /// encoding and `/Differences`, its widths from `/FirstChar` and `/Widths`.
    fn simple_font(&self, font: &Dict, subtype: &[u8]) -> Font {
        let base_font = self.get(font, b"BaseFont").as_name().unwrap_or_default();
        let symbolic = [b"Symbol".as_slice(), b"Dingbats"]
            .iter()
            .any(|symbolic| memmem::find(base_font, symbolic).is_some());
        let (base, differences) = match self.get(font, b"Encoding") {
            Object::Name(name) => (Some(name.as_slice()), None),
            Object::Dict(dict) => (self.get(dict, b"BaseEncoding").as_name(), self.get(dict, b"Differences").as_array()),
            _ => (None, None),
        };
        // An embedded Type1 font naming no base encoding starts from the one its own program
        // declares — where old LaTeX fonts keep their ligatures (`fi` at 12).
        let builtin = (base.is_none() && (subtype == b"Type1" || subtype == b"MMType1"))
            .then(|| self.builtin_encoding(font))
            .flatten();
        let mut text = builtin.unwrap_or_default();
        if text.is_empty() {
            if let Some(encoding) = base_encoding(base, subtype, symbolic) {
                for code in 0..=255u8 {
                    if let Some(c) = encoding(code) {
                        text.insert(u32::from(code), c.to_string());
                    }
                }
            }
        }
        let mut code = None;
        for item in differences.unwrap_or_default() {
            match self.resolve(item) {
                Object::Number(value) => code = as_code(*value),
                Object::Name(name) => {
                    if let Some(current) = code {
                        // A name not known here — dvips's `/B9`, a subsetter's `/g37` — leaves the
                        // base encoding's character: such fonts keep their codes in the usual
                        // places, and a guess beats a hole in every word.
                        if let Some(glyph) = glyph_text(name) {
                            text.insert(current, glyph);
                        }
                        code = current.checked_add(1);
                    }
                }
                _ => {}
            }
        }
        // Type3 glyph widths are in the font's own units, which its matrix scales to text space.
        let scale = if subtype == b"Type3" {
            let matrix = self.get(font, b"FontMatrix").as_array();
            matrix.and_then(<[Object]>::first).and_then(Object::as_number).map_or(1.0, |a| a * 1000.0)
        } else {
            1.0
        };
        let first = self.get(font, b"FirstChar").as_number().and_then(as_code).unwrap_or(0);
        let mut widths = HashMap::new();
        for (offset, width) in (0..=255).zip(self.get(font, b"Widths").as_array().unwrap_or_default()) {
            let code = first.checked_add(offset);
            if let (Some(code), Some(width)) = (code, self.resolve(width).as_number()) {
                widths.insert(code, width * scale);
            }
        }
        let descriptor = self.get(font, b"FontDescriptor").as_dict();
        let missing = descriptor.and_then(|descriptor| self.get(descriptor, b"MissingWidth").as_number());
        // A font with no widths at all (one of the standard 14) leaves its runs' ends a guess.
        let default_width = (!widths.is_empty()).then(|| missing.unwrap_or(0.0) * scale);
        // A Type3 font's em: twice its glyphs' average width (a glyph is about half an em), since
        // its own units say nothing of its size — or, with no widths, what its matrix makes of a
        // thousand units.
        let em = if subtype == b"Type3" {
            let drawn: Vec<f64> = widths.values().copied().filter(|width| *width > 0.0).collect();
            match u32::try_from(drawn.len()) {
                Ok(count) if count > 0 => 2.0 * drawn.iter().sum::<f64>() / f64::from(count) / 1000.0,
                _ => scale.abs(),
            }
        } else {
            1.0
        };
        Font { codes: Codes::OneByte, text, codes_are_unicode: false, widths, default_width, em }
    }
}

impl Document<'_> {
    /// The encoding an embedded Type1 font's own program declares — its `dup 12 /fi put` lines, in
    /// the clear-text opening of the program — or `None` where it declares Standard, or there is
    /// no program to read.
    fn builtin_encoding(&self, font: &Dict) -> Option<HashMap<u32, String>> {
        let descriptor = self.get(font, b"FontDescriptor").as_dict()?;
        let Object::Stream(dict, range) = self.get(descriptor, b"FontFile") else { return None };
        let program = self.data.get(range.clone()).and_then(|raw| self.decode(dict, raw))?;
        let clear_length = self.get(dict, b"Length1").as_number().and_then(as_index);
        let clear = clear_length.and_then(|length| program.get(..length)).unwrap_or(program.as_slice());
        let at = memmem::find(clear, b"/Encoding")?;
        let mut lexer = Lexer::new(clear, at + 9);
        let mut text = HashMap::new();
        let (mut code, mut name) = (None, None);
        while let Some(token) = lexer.token() {
            match token {
                Token::Word(b"StandardEncoding") if text.is_empty() => return None,
                Token::Number(value) => code = as_code(value),
                Token::Name(glyph) => name = Some(glyph),
                Token::Word(b"put") => {
                    if let (Some(code), Some(glyph)) = (code.take(), name.take()) {
                        if let Some(glyph) = glyph_text(&glyph) {
                            text.insert(code, glyph);
                        }
                    }
                }
                Token::Word(b"def" | b"readonly") => break,
                _ => {}
            }
        }
        (!text.is_empty()).then_some(text)
    }
}

/// Notes a trailer's root, and whether it names an encryption dictionary.
fn note_trailer(at: usize, trailer: &Dict, roots: &mut Vec<(usize, u32)>, encrypted: &mut bool) {
    if let Some(Object::Reference(root)) = trailer.get(b"Root".as_slice()) {
        roots.push((at, *root));
    }
    if trailer.contains_key(b"Encrypt".as_slice()) {
        *encrypted = true;
    }
}

/// The last `N` operands as numbers — operators take their operands from the end, so junk before
/// them is ignored.
fn last_numbers<const N: usize>(operands: &[Object]) -> Option<[f64; N]> {
    let start = operands.len().checked_sub(N)?;
    let mut numbers = [0.0; N];
    for (slot, operand) in numbers.iter_mut().zip(operands.get(start..)?) {
        *slot = operand.as_number()?;
    }
    Some(numbers)
}

/// Sets `field` to the last operand, when it is a number.
fn set_last(operands: &[Object], field: &mut f64) {
    if let Some([value]) = last_numbers(operands) {
        *field = value;
    }
}

// ---- drawing text ---------------------------------------------------------------------------

/// A transform, as PDF writes one: `[a b c d e f]`, applied to a row vector `[x y 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Matrix([f64; 6]);

impl Matrix {
    const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn translate(x: f64, y: f64) -> Matrix {
        Matrix([1.0, 0.0, 0.0, 1.0, x, y])
    }

    /// This transform, then `next`.
    fn then(self, next: Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [na, nb, nc, nd, ne, nf] = next.0;
        Matrix([
            a * na + b * nc,
            a * nb + b * nd,
            c * na + d * nc,
            c * nb + d * nd,
            e * na + f * nc + ne,
            e * nb + f * nd + nf,
        ])
    }

    /// Where the origin lands.
    fn origin(self) -> (f64, f64) {
        (self.0[4], self.0[5])
    }

    /// How much it scales lengths, on average over the two axes.
    fn scale(self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }
}

/// The graphics state text is drawn in: where it lands, and the text state (saved and restored by
/// `q` and `Q` along with the rest).
#[derive(Clone)]
struct State {
    ctm: Matrix,
    font: Option<Rc<Font>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    /// Horizontal scaling, as a fraction (`Tz` gives a percentage).
    scale: f64,
    leading: f64,
    rise: f64,
}

impl State {
    fn new(ctm: Matrix) -> Self {
        State { ctm, font: None, size: 0.0, char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0, rise: 0.0 }
    }

    /// The text rendering matrix, for the text matrix `tm`.
    fn render(&self, tm: Matrix) -> Matrix {
        Matrix([self.size * self.scale, 0.0, 0.0, self.size, 0.0, self.rise]).then(tm).then(self.ctm)
    }
}

/// `T*`: the start of the next line, a leading below this one's.
fn next_line(tm: &mut Matrix, tlm: &mut Matrix, state: &State) {
    *tlm = Matrix::translate(0.0, -state.leading).then(*tlm);
    *tm = *tlm;
}

/// Draws `bytes` in the current font: their text goes to `text`, placed where it lands, and the
/// pen moves past them.
fn show(bytes: &[u8], state: &State, tm: &mut Matrix, text: &mut Text) {
    let font = state.font.as_deref();
    let start = state.render(*tm).origin();
    let em = state.size * font.map_or(1.0, |font| font.em) * tm.then(state.ctm).scale();
    let mut run = String::new();
    let mut advance = 0.0;
    let mut guessed = false;
    for (code, length) in split_codes(font, bytes) {
        match font {
            Some(font) => font.push_text(code, &mut run),
            // No font selected: the bytes as Latin-1, the most a reader can assume.
            None => run.extend(u8::try_from(code).ok().map(char::from)),
        }
        let width = font.and_then(|font| font.width(code)).unwrap_or_else(|| {
            guessed = true;
            GUESSED_WIDTH
        });
        let mut step = width / 1000.0 * state.size + state.char_spacing;
        if length == 1 && code == 32 {
            step += state.word_spacing;
        }
        advance += step * state.scale;
    }
    *tm = Matrix::translate(advance, 0.0).then(*tm);
    let end = state.render(*tm).origin();
    text.run(start, end, em, guessed, &run);
}

/// A show operator's bytes, as the font splits them into codes: each code with its byte length.
fn split_codes(font: Option<&Font>, bytes: &[u8]) -> Vec<(u32, usize)> {
    match font.map(|font| &font.codes) {
        None | Some(Codes::OneByte) => bytes.iter().map(|&byte| (u32::from(byte), 1)).collect(),
        Some(Codes::TwoBytes) => bytes.chunks(2).map(|pair| (be(pair), pair.len())).collect(),
        Some(Codes::Ranges(ranges)) => {
            let mut codes = Vec::new();
            let mut at = 0;
            while at < bytes.len() {
                let fits = |length: usize| {
                    bytes.get(at..at + length).is_some_and(|code| {
                        ranges.iter().any(|&(width, low, high)| width == length && (low..=high).contains(&be(code)))
                    })
                };
                let length = (1..=4).find(|&length| fits(length)).unwrap_or(1);
                codes.push((bytes.get(at..at + length).map_or(0, be), length));
                at += length;
            }
            codes
        }
    }
}

/// The text of a page, built run by run: each run placed after the last, on the same line or a
/// new one, after a space or none.
#[derive(Default)]
struct Text {
    out: String,
    pen: Option<Pen>,
}

/// Where the last run ended, in how large a font, and whether its end is a guess.
#[derive(Clone, Copy)]
struct Pen {
    x: f64,
    y: f64,
    em: f64,
    guessed: bool,
}

impl Text {
    /// Adds a run drawn from `start` to `end` in a font `em` high.
    fn run(&mut self, start: (f64, f64), end: (f64, f64), em: f64, guessed: bool, run: &str) {
        if let Some(pen) = self.pen {
            let em_now = em.abs().max(pen.em.abs());
            if em_now.is_finite() && em_now > 0.0 {
                if (start.1 - pen.y).abs() > LINE_GAP * em_now {
                    self.line_break();
                } else {
                    let gap = if guessed || pen.guessed { SPACE_GAP_GUESSED } else { SPACE_GAP };
                    if start.0 - pen.x > gap * em_now {
                        self.space();
                    }
                }
            }
        }
        for c in run.chars() {
            self.push(c);
        }
        self.pen = Some(Pen { x: end.0, y: end.1, em, guessed });
    }

    /// One character — ligatures spelled out, so `file` is found in a word set with `ﬁ`; a
    /// no-break space as a space; control characters (a font's map to nothing) dropped.
    fn push(&mut self, c: char) {
        let spelled = match c {
            '\u{fb00}' => "ff",
            '\u{fb01}' => "fi",
            '\u{fb02}' => "fl",
            '\u{fb03}' => "ffi",
            '\u{fb04}' => "ffl",
            '\u{fb05}' | '\u{fb06}' => "st",
            '\u{a0}' => " ",
            c if c.is_control() => "",
            c => {
                self.out.push(c);
                return;
            }
        };
        self.out.push_str(spelled);
    }

    fn space(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with([' ', '\n']) {
            self.out.push(' ');
        }
    }

    fn line_break(&mut self) {
        let kept = self.out.trim_end_matches(' ').len();
        self.out.truncate(kept);
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn finish(self) -> String {
        self.out.trim_end().to_owned()
    }
}

// ---- fonts ----------------------------------------------------------------------------------

/// How a font's bytes split into codes.
#[derive(Clone, Debug, PartialEq)]
enum Codes {
    OneByte,
    TwoBytes,
    /// As a CMap's code space says: `(bytes, low, high)` ranges.
    Ranges(Vec<(usize, u32, u32)>),
}

/// What a font's codes are.
struct Font {
    codes: Codes,
    /// Each code's text.
    text: HashMap<u32, String>,
    /// Whether a code is its own text — fonts encoded by a UCS-2 or UTF-16 CMap.
    codes_are_unicode: bool,
    /// Glyph widths by code, in thousandths of the font size…
    widths: HashMap<u32, f64>,
    /// …and the width of any other code — `None` where the font gives no widths at all.
    default_width: Option<f64>,
    /// How large the font's em is, in units of its size: 1, except for a Type3 font, whose glyphs
    /// are in units of its own — pixels, for the bitmap fonts of old LaTeX papers.
    em: f64,
}

impl Font {
    fn push_text(&self, code: u32, out: &mut String) {
        if let Some(text) = self.text.get(&code) {
            out.push_str(text);
        } else if self.codes_are_unicode {
            out.extend(char::from_u32(code));
        }
    }

    fn width(&self, code: u32) -> Option<f64> {
        self.widths.get(&code).copied().or(self.default_width)
    }
}

/// A `ToUnicode` CMap: its code space, and each code's text.
#[derive(Debug, Default)]
struct CMap {
    ranges: Vec<(usize, u32, u32)>,
    text: HashMap<u32, String>,
}

impl CMap {
    /// Reads the code space ranges and the `bfchar` and `bfrange` mappings; the PostScript around
    /// them is passed over.
    fn parse(data: &[u8]) -> CMap {
        let mut cmap = CMap::default();
        let mut lexer = Lexer::new(data, 0);
        let mut operands: Vec<Object> = Vec::new();
        while let Some(token) = lexer.token() {
            let Token::Word(word) = token else {
                if let Some(operand) = lexer.object_from(token, 0) {
                    operands.push(operand);
                }
                continue;
            };
            match word {
                b"endcodespacerange" => {
                    for pair in operands.as_chunks::<2>().0 {
                        if let [Object::String(low), Object::String(high)] = pair {
                            if low.len() == high.len() && (1..=4).contains(&low.len()) {
                                cmap.ranges.push((low.len(), be(low), be(high)));
                            }
                        }
                    }
                }
                b"endbfchar" => {
                    for pair in operands.as_chunks::<2>().0 {
                        if let [Object::String(code), target] = pair {
                            if let Some(text) = cmap_target(target, 0) {
                                cmap.text.insert(be(code), text);
                            }
                        }
                    }
                }
                b"endbfrange" => {
                    for triple in operands.as_chunks::<3>().0 {
                        let [Object::String(low), Object::String(high), target] = triple else { continue };
                        let (low, high) = (be(low), be(high));
                        if high < low || high - low >= MAX_RANGE {
                            continue;
                        }
                        for (offset, code) in (low..=high).enumerate() {
                            let text = match target {
                                Object::Array(targets) => targets.get(offset).and_then(|target| cmap_target(target, 0)),
                                single => u32::try_from(offset).ok().and_then(|offset| cmap_target(single, offset)),
                            };
                            if let Some(text) = text {
                                cmap.text.insert(code, text);
                            }
                        }
                    }
                }
                _ => {}
            }
            operands.clear();
        }
        cmap
    }
}

/// A CMap destination's text — UTF-16BE, its last unit raised by `offset` (a range's later
/// codes) — or a glyph name's.
fn cmap_target(target: &Object, offset: u32) -> Option<String> {
    match target {
        Object::String(bytes) => {
            let mut units: Vec<u16> =
                bytes.chunks(2).map(|pair| u16::try_from(be(pair)).unwrap_or(0)).collect();
            if let Some(last) = units.last_mut() {
                *last = last.wrapping_add(u16::try_from(offset).unwrap_or(0));
            }
            Some(String::from_utf16_lossy(&units))
        }
        Object::Name(name) => glyph_text(name),
        _ => None,
    }
}

/// A code's character in a simple font's base encoding.
type Encoding = fn(u8) -> Option<char>;

/// The base encoding a simple font's codes start from: the one it names — or, naming none,
/// Standard for a Type1 font, WinAnsi for a TrueType one, and nothing for a symbol font or a
/// Type3 font, whose `/Differences` alone say what their codes are.
fn base_encoding(name: Option<&[u8]>, subtype: &[u8], symbolic: bool) -> Option<Encoding> {
    match name {
        Some(b"WinAnsiEncoding") => Some(win_ansi),
        Some(b"MacRomanEncoding") => Some(mac_roman),
        Some(b"StandardEncoding") => Some(standard),
        Some(_) => None,
        None if symbolic || subtype == b"Type3" => None,
        None if subtype == b"TrueType" => Some(win_ansi),
        None => Some(standard),
    }
}

/// WinAnsiEncoding's `0x80`–`0x9F`, the rest being Latin-1; `\0` where a code means nothing.
const WIN_ANSI_HIGH: &str = "€\0‚ƒ„…†‡ˆ‰Š‹Œ\0Ž\0\0‘’“”•–—˜™š›œ\0žŸ";

/// MacRomanEncoding's `0x80`–`0xFF`: the classic Mac OS character set, as PDF has it — the
/// currency sign at `0xDB`, and nothing for the Apple logo at `0xF0`.
const MAC_ROMAN_HIGH: &str = concat!(
    "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü",
    "†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø",
    "¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄¤‹›ﬁﬂ",
    "‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\0ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ",
);

/// WinAnsiEncoding: Windows' code page 1252.
fn win_ansi(code: u8) -> Option<char> {
    match code {
        0x20..=0x7e | 0xa0..=0xff => Some(char::from(code)),
        0x80..=0x9f => WIN_ANSI_HIGH.chars().nth(usize::from(code - 0x80)).filter(|&c| c != '\0'),
        _ => None,
    }
}

/// MacRomanEncoding.
fn mac_roman(code: u8) -> Option<char> {
    match code {
        0x20..=0x7e => Some(char::from(code)),
        0x80..=0xff => MAC_ROMAN_HIGH.chars().nth(usize::from(code - 0x80)).filter(|&c| c != '\0'),
        _ => None,
    }
}

/// StandardEncoding, Adobe's own: ASCII with curly quotes, and its own upper half.
fn standard(code: u8) -> Option<char> {
    Some(match code {
        b'\'' => '’',
        b'`' => '‘',
        0x20..=0x7e => char::from(code),
        0xa1 => '¡',
        0xa2 => '¢',
        0xa3 => '£',
        0xa4 => '⁄',
        0xa5 => '¥',
        0xa6 => 'ƒ',
        0xa7 => '§',
        0xa8 => '¤',
        0xa9 => '\'',
        0xaa => '“',
        0xab => '«',
        0xac => '‹',
        0xad => '›',
        0xae => 'ﬁ',
        0xaf => 'ﬂ',
        0xb1 => '–',
        0xb2 => '†',
        0xb3 => '‡',
        0xb4 => '·',
        0xb6 => '¶',
        0xb7 => '•',
        0xb8 => '‚',
        0xb9 => '„',
        0xba => '”',
        0xbb => '»',
        0xbc => '…',
        0xbd => '‰',
        0xbf => '¿',
        0xc1 => '`',
        0xc2 => '´',
        0xc3 => 'ˆ',
        0xc4 => '˜',
        0xc5 => '¯',
        0xc6 => '˘',
        0xc7 => '˙',
        0xc8 => '¨',
        0xca => '˚',
        0xcb => '¸',
        0xcd => '˝',
        0xce => '˛',
        0xcf => 'ˇ',
        0xd0 => '—',
        0xe1 => 'Æ',
        0xe3 => 'ª',
        0xe8 => 'Ł',
        0xe9 => 'Ø',
        0xea => 'Œ',
        0xeb => 'º',
        0xf1 => 'æ',
        0xf5 => 'ı',
        0xf8 => 'ł',
        0xf9 => 'ø',
        0xfa => 'œ',
        0xfb => 'ß',
        _ => return None,
    })
}

/// A glyph name's text: a single letter is itself; `uniXXXX…` and `uXXXX` name their code
/// points; `f_f_i` is its parts; a suffix after a dot (`a.sc`) is a variant of the same
/// character; the rest come from [`GLYPH_NAMES`]. Unknown names — `g123`, a subsetter's
/// numbering — have no text.
fn glyph_text(name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(name).ok()?;
    let base = name.split('.').next().filter(|base| !base.is_empty())?;
    if base.contains('_') {
        return base.split('_').map(glyph_part).collect();
    }
    glyph_part(base)
}

fn glyph_part(name: &str) -> Option<String> {
    if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
        return Some(name.to_owned());
    }
    if let Some(hex) = name.strip_prefix("uni").filter(|hex| !hex.is_empty() && hex.len() % 4 == 0) {
        let units: Option<Vec<u16>> = (0..hex.len())
            .step_by(4)
            .map(|at| hex.get(at..at + 4).and_then(|unit| u16::from_str_radix(unit, 16).ok()))
            .collect();
        return String::from_utf16(&units?).ok();
    }
    if let Some(hex) = name.strip_prefix('u').filter(|hex| (4..=6).contains(&hex.len())) {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(String::from);
    }
    GLYPH_NAMES
        .binary_search_by(|(known, _)| known.cmp(&name))
        .ok()
        .map(|index| GLYPH_NAMES[index].1.to_owned())
}

/// The glyph names Latin text is set in — those of the Standard, WinAnsi and MacRoman encodings,
/// the ligatures, and the Greek LaTeX's fonts carry — with their text, sorted by name for the
/// binary search (a test holds the order). Single ASCII letters are left to [`glyph_part`].
const GLYPH_NAMES: &[(&str, &str)] = &[
    ("AE", "Æ"), ("Aacute", "Á"), ("Acircumflex", "Â"), ("Adieresis", "Ä"), ("Agrave", "À"),
    ("Aring", "Å"), ("Atilde", "Ã"), ("Ccedilla", "Ç"), ("Delta", "Δ"), ("Eacute", "É"),
    ("Ecircumflex", "Ê"), ("Edieresis", "Ë"), ("Egrave", "È"), ("Eth", "Ð"), ("Euro", "€"),
    ("Gamma", "Γ"), ("Iacute", "Í"), ("Icircumflex", "Î"), ("Idieresis", "Ï"), ("Igrave", "Ì"),
    ("Lambda", "Λ"), ("Lslash", "Ł"), ("Ntilde", "Ñ"), ("OE", "Œ"), ("Oacute", "Ó"),
    ("Ocircumflex", "Ô"), ("Odieresis", "Ö"), ("Ograve", "Ò"), ("Omega", "Ω"), ("Oslash", "Ø"),
    ("Otilde", "Õ"), ("Phi", "Φ"), ("Pi", "Π"), ("Psi", "Ψ"), ("Scaron", "Š"), ("Sigma", "Σ"),
    ("Theta", "Θ"), ("Thorn", "Þ"), ("Uacute", "Ú"), ("Ucircumflex", "Û"), ("Udieresis", "Ü"),
    ("Ugrave", "Ù"), ("Upsilon", "Υ"), ("Xi", "Ξ"), ("Yacute", "Ý"), ("Ydieresis", "Ÿ"),
    ("Zcaron", "Ž"), ("aacute", "á"), ("acircumflex", "â"), ("acute", "´"), ("adieresis", "ä"),
    ("ae", "æ"), ("agrave", "à"), ("alpha", "α"), ("ampersand", "&"), ("approxequal", "≈"),
    ("aring", "å"), ("asciicircum", "^"), ("asciitilde", "~"), ("asterisk", "*"), ("at", "@"),
    ("atilde", "ã"), ("backslash", "\\"), ("bar", "|"), ("beta", "β"), ("braceleft", "{"),
    ("braceright", "}"), ("bracketleft", "["), ("bracketright", "]"), ("breve", "˘"),
    ("brokenbar", "¦"), ("bullet", "•"), ("caron", "ˇ"), ("ccedilla", "ç"), ("cedilla", "¸"),
    ("cent", "¢"), ("chi", "χ"), ("circumflex", "ˆ"), ("colon", ":"), ("comma", ","),
    ("copyright", "©"), ("currency", "¤"), ("dagger", "†"), ("daggerdbl", "‡"), ("degree", "°"),
    ("delta", "δ"), ("dieresis", "¨"), ("divide", "÷"), ("dollar", "$"), ("dotaccent", "˙"),
    ("dotlessi", "ı"), ("dotlessj", "ȷ"), ("eacute", "é"), ("ecircumflex", "ê"),
    ("edieresis", "ë"), ("egrave", "è"), ("eight", "8"), ("ellipsis", "…"), ("emdash", "—"),
    ("endash", "–"), ("epsilon", "ε"), ("equal", "="), ("eta", "η"), ("eth", "ð"), ("exclam", "!"),
    ("exclamdown", "¡"), ("ff", "ff"), ("ffi", "ffi"), ("ffl", "ffl"), ("fi", "fi"), ("five", "5"),
    ("fl", "fl"), ("florin", "ƒ"), ("four", "4"), ("fraction", "⁄"), ("gamma", "γ"),
    ("germandbls", "ß"), ("grave", "`"), ("greater", ">"), ("greaterequal", "≥"),
    ("guillemotleft", "«"), ("guillemotright", "»"), ("guilsinglleft", "‹"),
    ("guilsinglright", "›"), ("hungarumlaut", "˝"), ("hyphen", "-"), ("iacute", "í"),
    ("icircumflex", "î"), ("idieresis", "ï"), ("igrave", "ì"), ("infinity", "∞"),
    ("integral", "∫"), ("iota", "ι"), ("kappa", "κ"), ("lambda", "λ"), ("less", "<"),
    ("lessequal", "≤"), ("logicalnot", "¬"), ("lozenge", "◊"), ("lslash", "ł"), ("macron", "¯"),
    ("minus", "−"), ("mu", "µ"), ("multiply", "×"), ("nbspace", " "), ("nine", "9"),
    ("notequal", "≠"), ("ntilde", "ñ"), ("nu", "ν"), ("numbersign", "#"), ("oacute", "ó"),
    ("ocircumflex", "ô"), ("odieresis", "ö"), ("oe", "œ"), ("ogonek", "˛"), ("ograve", "ò"),
    ("omega", "ω"), ("omicron", "ο"), ("one", "1"), ("onehalf", "½"), ("onequarter", "¼"),
    ("onesuperior", "¹"), ("ordfeminine", "ª"), ("ordmasculine", "º"), ("oslash", "ø"),
    ("otilde", "õ"), ("paragraph", "¶"), ("parenleft", "("), ("parenright", ")"),
    ("partialdiff", "∂"), ("percent", "%"), ("period", "."), ("periodcentered", "·"),
    ("perthousand", "‰"), ("phi", "φ"), ("pi", "π"), ("plus", "+"), ("plusminus", "±"),
    ("product", "∏"), ("psi", "ψ"), ("question", "?"), ("questiondown", "¿"), ("quotedbl", "\""),
    ("quotedblbase", "„"), ("quotedblleft", "“"), ("quotedblright", "”"), ("quoteleft", "‘"),
    ("quoteright", "’"), ("quotesinglbase", "‚"), ("quotesingle", "'"), ("radical", "√"),
    ("registered", "®"), ("rho", "ρ"), ("ring", "˚"), ("scaron", "š"), ("section", "§"),
    ("semicolon", ";"), ("seven", "7"), ("sfthyphen", "-"), ("sigma", "σ"), ("six", "6"),
    ("slash", "/"), ("space", " "), ("sterling", "£"), ("summation", "∑"), ("tau", "τ"),
    ("theta", "θ"), ("thorn", "þ"), ("three", "3"), ("threequarters", "¾"), ("threesuperior", "³"),
    ("tilde", "˜"), ("trademark", "™"), ("two", "2"), ("twosuperior", "²"), ("uacute", "ú"),
    ("ucircumflex", "û"), ("udieresis", "ü"), ("ugrave", "ù"), ("underscore", "_"),
    ("upsilon", "υ"), ("xi", "ξ"), ("yacute", "ý"), ("ydieresis", "ÿ"), ("yen", "¥"),
    ("zcaron", "ž"), ("zero", "0"), ("zeta", "ζ"),
];

// ---- filters --------------------------------------------------------------------------------

/// Flate: zlib, or — as some writers leave it — bare deflate. A broken or truncated stream gives
/// what inflated before the break.
fn inflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = ZlibDecoder::new(data).take(MAX_INFLATED).read_to_end(&mut out);
    if out.is_empty() {
        let _ = DeflateDecoder::new(data).take(MAX_INFLATED).read_to_end(&mut out);
    }
    out
}

/// ASCIIHex: pairs of hex digits up to `>`, white space between them ignored.
fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut lexer = Lexer::new(data, 0);
    lexer.hex_string()
}

/// ASCII85: groups of five base-85 digits for four bytes, `z` for four zeros, `~>` at the end.
fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u8; 5];
    let mut filled = 0;
    let value = |group: &[u8; 5]| group.iter().fold(0u64, |value, &digit| value * 85 + u64::from(digit));
    let body = data.strip_prefix(b"<~").unwrap_or(data);
    for &byte in body {
        match byte {
            b'~' => break,
            b'z' if filled == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[filled] = byte - b'!';
                filled += 1;
                if filled == 5 {
                    out.extend_from_slice(&u32::try_from(value(&group) & 0xffff_ffff).unwrap_or(0).to_be_bytes());
                    filled = 0;
                }
            }
            _ => {}
        }
    }
    if filled > 1 {
        group[filled..].fill(84);
        let bytes = u32::try_from(value(&group) & 0xffff_ffff).unwrap_or(0).to_be_bytes();
        out.extend_from_slice(&bytes[..filled - 1]);
    }
    out
}

/// RunLength: a length byte, then that many bytes plus one copied — or, past 128, one byte
/// repeated 257 less that many times; 128 ends the data.
fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&length) = data.get(at) {
        at += 1;
        match length {
            128 => break,
            0..=127 => {
                let end = (at + usize::from(length) + 1).min(data.len());
                out.extend_from_slice(data.get(at..end).unwrap_or_default());
                at = end;
            }
            _ => {
                let Some(&byte) = data.get(at) else { break };
                out.extend(std::iter::repeat_n(byte, 257 - usize::from(length)));
                at += 1;
            }
        }
    }
    out
}

/// The PNG Paeth predictor: whichever of left, up and up-left is nearest their sum's estimate.
fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(up_left);
    let distance = |value: u8| (estimate - i16::from(value)).abs();
    if distance(left) <= distance(up) && distance(left) <= distance(up_left) {
        left
    } else if distance(up) <= distance(up_left) {
        up
    } else {
        up_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// The real [`super::pages`], for the files here that aren't encrypted.
    fn pages(data: &[u8]) -> Vec<String> {
        super::pages(data).expect("only the encryption test encrypts a file")
    }

    /// A PDF of `objects`, numbered from 1, rooted at object 1 — with no cross-reference table,
    /// which this reader never reads.
    fn pdf(objects: &[Vec<u8>]) -> Vec<u8> {
        let numbered: Vec<(u32, Vec<u8>)> = (1..).zip(objects.iter().cloned()).collect();
        pdf_numbered(&numbered)
    }

    /// A PDF of `objects` under the numbers given, rooted at object 1.
    fn pdf_numbered(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        for (number, body) in objects {
            out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        out.extend_from_slice(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
        out
    }

    /// A stream object: `dict`'s entries, a `/Length` for `data`, and the data.
    fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    }

    fn deflated(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// One page drawing `content` (object 5) with `font` as `/F1` (object 4); `extra` objects
    /// follow from number 6.
    fn one_page(font: &str, content: Vec<u8>, extra: Vec<Vec<u8>>) -> Vec<u8> {
        let mut objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_vec(),
            font.as_bytes().to_vec(),
            content,
        ];
        objects.extend(extra);
        pdf(&objects)
    }

    /// A simple font every glyph of which is half an em wide.
    const EVEN: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
                        /FirstChar 0 /LastChar 255 /Widths [ 256*500 ] >>";

    fn even_font() -> String {
        EVEN.replace("256*500", &["500"; 256].join(" "))
    }

    #[test]
    fn a_simple_font_types_its_text() {
        let content = stream("", b"BT /F1 12 Tf 72 700 Td (Hello World) Tj ET");
        assert_eq!(pages(&one_page(&even_font(), content, vec![])), ["Hello World"]);
    }

    #[test]
    fn a_compressed_page_is_read() {
        let content = stream("/Filter /FlateDecode", &deflated(b"BT /F1 12 Tf 72 700 Td (Packed) Tj ET"));
        assert_eq!(pages(&one_page(&even_font(), content, vec![])), ["Packed"]);
    }

    /// The way Chrome, LibreOffice and Word write text: glyph numbers, two bytes each, read back
    /// through the font's own map to Unicode — single codes and ranges, a range's targets counted
    /// up or listed.
    #[test]
    fn glyph_numbers_read_through_the_fonts_unicode_map() {
        let font = "<< /Type /Font /Subtype /Type0 /BaseFont /AAAAAA+Test /Encoding /Identity-H \
                    /DescendantFonts [6 0 R] /ToUnicode 7 0 R >>";
        let cid_font = b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /AAAAAA+Test /DW 500 >>".to_vec();
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
            1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
            2 beginbfchar <0001> <0043> <0002> <0068> endbfchar\n\
            2 beginbfrange <0003> <0005> [<0075> <0074> <0065>] <0010> <0012> <0061> endbfrange\n\
            endcmap CMapName currentdict /CMap defineresource pop end end";
        let content = stream("", b"BT /F1 10 Tf 1 0 0 -1 50 50 Tm [<0001000200030004>-10<0005>] TJ \
            0 -20 Td <001000110012> Tj ET");
        let pdf = one_page(font, content, vec![cid_font, stream("", cmap)]);
        assert_eq!(pages(&pdf), ["Chute\nabc"]);
    }

    /// Kerning stays inside a word; a gap of a word space starts the next one.
    #[test]
    fn kerning_stays_inside_a_word_and_a_wide_gap_is_a_space() {
        let content = stream("", b"BT /F1 10 Tf 0 0 Td [(Chu) -20 (te) -400 (Order)] TJ ET");
        assert_eq!(pages(&one_page(&even_font(), content, vec![])), ["Chute Order"]);
    }

    /// Runs placed by their own positions: a move down starts a line, a jump right a word, and
    /// a run picking up where the last ended continues it.
    #[test]
    fn where_runs_land_makes_lines_and_words() {
        let content = stream(
            "",
            b"BT /F1 10 Tf 1 0 0 1 10 100 Tm (Line) Tj 1 0 0 1 40 100 Tm (one) Tj \
              1 0 0 1 10 80 Tm (Li) Tj 1 0 0 1 20 80 Tm (ne) Tj ET",
        );
        assert_eq!(pages(&one_page(&even_font(), content, vec![])), ["Line one\nLine"]);
    }

    /// A font that gives no widths leaves where runs end a guess: only a gap of most of an em is
    /// taken for a space, so a guess never splits a word.
    #[test]
    fn without_widths_only_a_wide_gap_is_a_space() {
        let helvetica = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
        let content = stream(
            "",
            b"BT /F1 10 Tf 1 0 0 1 0 0 Tm (Ch) Tj 1 0 0 1 12 0 Tm (ute) Tj \
              1 0 0 1 60 0 Tm (Order) Tj ET",
        );
        assert_eq!(pages(&one_page(helvetica, content, vec![])), ["Chute Order"]);
    }

    /// Glyph names spell out — letters, `uniXXXX`, `f_i`, ligatures, a variant suffix — and a
    /// ligature from the font's Unicode map does too.
    #[test]
    fn glyph_names_and_ligatures_spell_out() {
        let font = "<< /Type /Font /Subtype /Type1 /BaseFont /CMR10 /Encoding << /Type /Encoding \
                    /Differences [65 /f_i /uni0042 /fi /quoteright /a.sc /g37] >> >>";
        let content = stream("", b"BT /F1 10 Tf (ABCDEF) Tj ET");
        // `/g37` names nothing known, so code `F` keeps Standard's `F`.
        assert_eq!(pages(&one_page(font, content, vec![])), ["fiBfi’aF"]);
    }

    /// A font that renames its glyphs to names that mean nothing — dvips's bitmap fonts — keeps
    /// its base encoding's letters; and a broken Unicode map's entry to a control character
    /// doesn't take one away.
    #[test]
    fn meaningless_glyph_names_and_control_mappings_keep_the_letters() {
        let font = "<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] \
                    /Encoding << /BaseEncoding /WinAnsiEncoding /Differences [97 /CP /CR /CS] >> \
                    /ToUnicode 6 0 R >>";
        let cmap = b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfrange <63> <63> <000d> endbfrange";
        let content = stream("", b"BT /F1 10 Tf (abc) Tj ET");
        assert_eq!(pages(&one_page(font, content, vec![stream("", cmap)])), ["abc"]);
    }

    /// A Type3 font in units of its own — a bitmap font's pixels, at a thousand times the usual
    /// scale — has an em to match its glyphs: kerning stays inside a word, a word gap is a space.
    #[test]
    fn a_type3_fonts_matrix_scales_its_em() {
        let font = "<< /Type /Font /Subtype /Type3 /FontMatrix [1 0 0 -1 0 0] /FirstChar 97 \
                    /Widths [100 100 100] /Encoding << /BaseEncoding /WinAnsiEncoding >> >>";
        let content = stream("", b"BT /F1 0.12 Tf 1 0 0 -1 10 35 Tm [(ab) 4000 (c)] TJ 40 0 Td (abc) Tj ET");
        assert_eq!(pages(&one_page(font, content, vec![])), ["abc abc"]);
    }

    /// An embedded Type1 font that names no encoding uses the one its own program declares —
    /// where an old LaTeX font keeps `fi`, at code 12.
    #[test]
    fn a_type1_programs_own_encoding_is_read() {
        let font = "<< /Type /Font /Subtype /Type1 /BaseFont /CMR10 /FontDescriptor 6 0 R >>";
        let descriptor = b"<< /Type /FontDescriptor /FontName /CMR10 /FontFile 7 0 R >>".to_vec();
        let program = b"%!PS-AdobeFont-1.0: CMR10\n/Encoding 256 array\n\
            0 1 255 {1 index exch /.notdef put} for\ndup 12 /fi put\ndup 108 /l put\n\
            dup 101 /e put\nreadonly def\ncurrentfile eexec\n";
        // `\\014`: the PDF string's own octal escape for code 12.
        let content = stream("", b"BT /F1 10 Tf (\\014le) Tj ET");
        let program = stream(&format!("/Length1 {}", program.len()), program);
        assert_eq!(pages(&one_page(font, content, vec![descriptor, program])), ["file"]);
    }

    #[test]
    fn standard_encoding_has_curly_quotes_and_its_own_upper_half() {
        assert_eq!(standard(b'\''), Some('’'));
        assert_eq!(standard(0xae), Some('ﬁ'));
        assert_eq!(win_ansi(0x80), Some('€'));
        assert_eq!(win_ansi(0x81), None);
        assert_eq!(mac_roman(0x8e), Some('é'));
        assert_eq!(mac_roman(0xf0), None);
        assert_eq!(mac_roman(0xff), Some('ˇ'));
    }

    /// Pages come in the page tree's order, whatever their object numbers, and draw with the
    /// resources a node above them gives.
    #[test]
    fn pages_follow_the_tree_and_inherit_resources() {
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [4 0 R 3 0 R] /Count 2 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_vec(),
            even_font().into_bytes(),
            stream("", b"BT /F1 10 Tf (first) Tj ET"),
            stream("", b"BT /F1 10 Tf (second) Tj ET"),
        ];
        assert_eq!(pages(&pdf(&objects)), ["first", "second"]);
    }

    /// Objects packed into a compressed object stream are found like any other.
    #[test]
    fn objects_packed_in_an_object_stream_are_found() {
        let packed_font = even_font();
        let packed_page = "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>";
        let header = format!("3 0 4 {} ", packed_page.len() + 1);
        let body = format!("{header}{packed_page}\n{packed_font}");
        let object_stream = stream(
            &format!("/Type /ObjStm /N 2 /First {} /Filter /FlateDecode", header.len()),
            &deflated(body.as_bytes()),
        );
        // The page (3) and its font (4) live only inside the object stream (6).
        let file = pdf_numbered(&[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (5, stream("", b"BT /F1 10 Tf (inside) Tj ET")),
            (6, object_stream),
        ]);
        assert_eq!(pages(&file), ["inside"]);
    }

    /// A form XObject's text is read where the page draws it.
    #[test]
    fn a_form_xobjects_text_is_read() {
        let page = b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /X1 4 0 R >> >> /Contents 5 0 R >>";
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            page.to_vec(),
            stream("/Type /XObject /Subtype /Form /Resources << /Font << /F1 6 0 R >> >>", b"BT /F1 10 Tf (in a form) Tj ET"),
            stream("", b"q 1 0 0 1 0 0 cm /X1 Do Q"),
            even_font().into_bytes(),
        ];
        assert_eq!(pages(&pdf(&objects)), ["in a form"]);
    }

    /// An inline image's data, whatever bytes it holds, is stepped over.
    #[test]
    fn an_inline_image_is_stepped_over() {
        let content = stream(
            "",
            b"BT /F1 10 Tf 0 0 Td (before) Tj ET BI /W 4 /H 1 /BPC 8 /CS /G ID \x00(Tj\xff] EI\n\
              BT /F1 10 Tf 0 -20 Td (after) Tj ET",
        );
        assert_eq!(pages(&one_page(&even_font(), content, vec![])), ["before\nafter"]);
    }

    #[test]
    fn an_encrypted_file_says_so_and_yields_no_text() {
        let mut file = one_page(&even_font(), stream("", b"BT /F1 10 Tf (secret) Tj ET"), vec![]);
        let trailer = memmem::find(&file, b"<< /Root 1 0 R >>").unwrap();
        file.splice(trailer..trailer + 17, b"<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>".iter().copied());
        assert_eq!(super::pages(&file), Err(Encrypted));
    }

    /// No file — cut short anywhere, or with its bytes scrambled — makes the reader panic.
    #[test]
    fn broken_files_never_panic() {
        let font = "<< /Type /Font /Subtype /Type0 /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 7 0 R >>";
        let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfrange <00> <FF> <0041> endbfrange";
        let content = stream("/Filter /FlateDecode", &deflated(b"BT /F1 10 Tf [<0001>-300<0002>] TJ ET"));
        let cid = b"<< /Subtype /CIDFontType2 /W [1 [500 600] 3 9 400] >>".to_vec();
        let file = one_page(font, content, vec![cid, stream("", cmap)]);
        for end in 0..file.len() {
            let _ = super::pages(&file[..end]);
        }
        let mut scrambled = file.clone();
        for (index, byte) in scrambled.iter_mut().enumerate() {
            if index % 7 == 3 {
                *byte = byte.wrapping_mul(31).wrapping_add(17);
            }
        }
        let _ = super::pages(&scrambled);
        let _ = super::pages(&[0xff; 4096]);
        // A page tree that is its own child is walked once.
        let cycle = b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj \
            2 0 obj << /Type /Pages /Kids [2 0 R] >> endobj trailer << /Root 1 0 R >>";
        assert!(pages(cycle).is_empty());
    }

    #[test]
    fn the_encoding_tables_cover_their_ranges() {
        assert_eq!(WIN_ANSI_HIGH.chars().count(), 32);
        assert_eq!(MAC_ROMAN_HIGH.chars().count(), 128);
    }

    #[test]
    fn the_glyph_name_table_is_sorted_for_its_binary_search() {
        assert!(GLYPH_NAMES.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn filters_decode() {
        assert_eq!(ascii_hex(b"48 65 6C6C 6F>"), b"Hello");
        assert_eq!(ascii85(b"<~87cURD]i,\"Ebo80~>"), b"Hello World!");
        // A short last group: "Hello World" ends three digits into a group of five.
        assert_eq!(ascii85(b"<~87cURD]i,\"Ebo7~>"), b"Hello World");
        assert_eq!(run_length(&[2, b'a', b'b', b'c', 254, b'z', 128]), b"abczzz");
        assert_eq!(inflate(&deflated(b"inflated")), b"inflated");
    }
}
