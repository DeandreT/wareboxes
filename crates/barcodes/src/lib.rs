//! Dependency-free barcode encoders used by the Wareboxes client.
//!
//! Currently implements Code 128-B, GS1-128, UPC-A, and QR Code Model 2 byte mode.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarcodeError {
    Empty,
    UnsupportedKind(String),
    UnsupportedCharacter(char),
    InvalidLength {
        kind: &'static str,
        expected: &'static str,
    },
    InvalidChecksum {
        kind: &'static str,
    },
    InvalidGs1Ai(String),
    DataTooLong {
        max_bytes: usize,
    },
}

impl fmt::Display for BarcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BarcodeError::Empty => f.write_str("barcode value cannot be empty"),
            BarcodeError::UnsupportedKind(kind) => write!(f, "unsupported barcode kind: {kind}"),
            BarcodeError::UnsupportedCharacter(ch) => {
                write!(f, "barcode cannot encode character: {ch:?}")
            }
            BarcodeError::InvalidLength { kind, expected } => {
                write!(f, "{kind} must be {expected}")
            }
            BarcodeError::InvalidChecksum { kind } => {
                write!(f, "{kind} check digit is invalid")
            }
            BarcodeError::InvalidGs1Ai(ai) => {
                write!(f, "unsupported or invalid GS1 application identifier: {ai}")
            }
            BarcodeError::DataTooLong { max_bytes } => {
                write!(
                    f,
                    "QR payload is too long; max supported is {max_bytes} bytes"
                )
            }
        }
    }
}

impl std::error::Error for BarcodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarcodeKind {
    Code128,
    Gs1_128,
    UpcA,
    Qr,
}

impl BarcodeKind {
    pub fn parse(value: &str) -> Result<Self, BarcodeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "code128" | "code-128" | "128" => Ok(Self::Code128),
            "gs1-128" | "gs1128" | "gs1_128" | "ucc128" | "ucc-128" => Ok(Self::Gs1_128),
            "upc-a" | "upca" | "upc" => Ok(Self::UpcA),
            "qr" | "qrcode" | "qr-code" => Ok(Self::Qr),
            other => Err(BarcodeError::UnsupportedKind(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Module {
    pub black: bool,
    pub width: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedBarcode {
    pub modules: Vec<Module>,
}

impl EncodedBarcode {
    pub fn total_modules(&self) -> u32 {
        self.modules.iter().map(|module| module.width as u32).sum()
    }

    pub fn to_svg(&self, human_text: &str) -> String {
        let module_px = 2_u32;
        let quiet_zone = 20_u32;
        let bar_height = 74_u32;
        let text_height = 24_u32;
        let width = self.total_modules() * module_px + quiet_zone * 2;
        let height = bar_height + text_height + 20;
        let mut x = quiet_zone;
        let mut body = String::new();
        for module in &self.modules {
            let module_width = module.width as u32 * module_px;
            if module.black {
                body.push_str(&format!(
                    r#"<rect x="{x}" y="10" width="{module_width}" height="{bar_height}" fill="black"/>"#
                ));
            }
            x += module_width;
        }
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="white"/>{body}<text x="{}" y="{}" font-family="monospace" font-size="14" text-anchor="middle" fill="black">{}</text></svg>"#,
            width / 2,
            bar_height + 28,
            escape_xml(human_text),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedQr {
    pub version: u8,
    pub size: usize,
    modules: Vec<bool>,
}

impl EncodedQr {
    pub fn module(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    pub fn to_svg(&self, human_text: &str) -> String {
        let module_px = 8_usize;
        let quiet_zone = 4_usize;
        let image_modules = self.size + quiet_zone * 2;
        let side = image_modules * module_px;
        let text_height = 24_usize;
        let mut body = String::new();
        for y in 0..self.size {
            for x in 0..self.size {
                if self.module(x, y) {
                    body.push_str(&format!(
                        r#"<rect x="{}" y="{}" width="{module_px}" height="{module_px}" fill="black"/>"#,
                        (x + quiet_zone) * module_px,
                        (y + quiet_zone) * module_px
                    ));
                }
            }
        }
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{}" viewBox="0 0 {side} {}"><rect width="100%" height="100%" fill="white"/>{body}<text x="{}" y="{}" font-family="monospace" font-size="12" text-anchor="middle" fill="black">{}</text></svg>"#,
            side + text_height,
            side + text_height,
            side / 2,
            side + 16,
            escape_xml(human_text)
        )
    }
}

pub fn encode(kind: &str, value: &str) -> Result<EncodedBarcode, BarcodeError> {
    match BarcodeKind::parse(kind)? {
        BarcodeKind::Code128 => encode_code128_b(value),
        BarcodeKind::Gs1_128 => encode_gs1_128(value),
        BarcodeKind::UpcA => encode_upc_a(value),
        BarcodeKind::Qr => Err(BarcodeError::UnsupportedKind(
            "qr is a 2D barcode; use encode_qr".to_owned(),
        )),
    }
}

pub fn svg(kind: &str, value: &str) -> Result<String, BarcodeError> {
    match BarcodeKind::parse(kind)? {
        BarcodeKind::Code128 => Ok(encode_code128_b(value)?.to_svg(value.trim())),
        BarcodeKind::Gs1_128 => Ok(encode_gs1_128(value)?.to_svg(value.trim())),
        BarcodeKind::UpcA => Ok(encode_upc_a(value)?.to_svg(&upc_a_digits(value)?)),
        BarcodeKind::Qr => Ok(encode_qr(value)?.to_svg(value.trim())),
    }
}

pub fn normalized_value(kind: &str, value: &str) -> Result<String, BarcodeError> {
    match BarcodeKind::parse(kind)? {
        BarcodeKind::Code128 => {
            encode_code128_b(value)?;
            Ok(value.trim().to_owned())
        }
        BarcodeKind::Gs1_128 => Ok(gs1_symbols_to_string(&normalize_gs1_payload(value)?)),
        BarcodeKind::UpcA => upc_a_digits(value),
        BarcodeKind::Qr => {
            encode_qr(value)?;
            Ok(value.trim().to_owned())
        }
    }
}

pub fn encode_code128_b(value: &str) -> Result<EncodedBarcode, BarcodeError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(BarcodeError::Empty);
    }

    let mut codes = Vec::with_capacity(value.len() + 3);
    codes.push(104_u8); // Start Code B
    for ch in value.chars() {
        let codepoint = ch as u32;
        if !(32..=127).contains(&codepoint) {
            return Err(BarcodeError::UnsupportedCharacter(ch));
        }
        codes.push((codepoint - 32) as u8);
    }

    let checksum = codes
        .iter()
        .enumerate()
        .map(|(idx, code)| {
            if idx == 0 {
                *code as u32
            } else {
                *code as u32 * idx as u32
            }
        })
        .sum::<u32>()
        % 103;
    codes.push(checksum as u8);
    codes.push(106_u8); // Stop

    let mut modules = Vec::new();
    for code in codes {
        append_pattern(&mut modules, CODE128_PATTERNS[code as usize]);
    }
    Ok(EncodedBarcode { modules })
}

pub fn encode_gs1_128(value: &str) -> Result<EncodedBarcode, BarcodeError> {
    let payload = normalize_gs1_payload(value)?;
    let mut codes = Vec::with_capacity(payload.len() + 4);
    codes.push(104_u8); // Start Code B
    codes.push(102_u8); // FNC1 marks this as GS1-128.
    for symbol in payload {
        match symbol {
            Gs1Symbol::Char(ch) => {
                let codepoint = ch as u32;
                if !(32..=127).contains(&codepoint) {
                    return Err(BarcodeError::UnsupportedCharacter(ch));
                }
                codes.push((codepoint - 32) as u8);
            }
            Gs1Symbol::Fnc1 => codes.push(102_u8),
        }
    }
    append_code128_checksum_and_stop(&mut codes);
    Ok(EncodedBarcode {
        modules: code128_modules(&codes),
    })
}

pub fn encode_upc_a(value: &str) -> Result<EncodedBarcode, BarcodeError> {
    let digits = upc_a_digits(value)?;
    let mut bits = Vec::with_capacity(95);
    push_barcode_bits(&mut bits, "101");
    for digit in digits[..6].bytes() {
        push_barcode_bits(&mut bits, UPC_LEFT_PATTERNS[(digit - b'0') as usize]);
    }
    push_barcode_bits(&mut bits, "01010");
    for digit in digits[6..].bytes() {
        push_barcode_bits(&mut bits, UPC_RIGHT_PATTERNS[(digit - b'0') as usize]);
    }
    push_barcode_bits(&mut bits, "101");
    Ok(EncodedBarcode {
        modules: modules_from_bits(&bits),
    })
}

fn append_code128_checksum_and_stop(codes: &mut Vec<u8>) {
    let checksum = codes
        .iter()
        .enumerate()
        .map(|(idx, code)| {
            if idx == 0 {
                *code as u32
            } else {
                *code as u32 * idx as u32
            }
        })
        .sum::<u32>()
        % 103;
    codes.push(checksum as u8);
    codes.push(106_u8); // Stop
}

fn code128_modules(codes: &[u8]) -> Vec<Module> {
    let mut modules = Vec::new();
    for code in codes {
        append_pattern(&mut modules, CODE128_PATTERNS[*code as usize]);
    }
    modules
}

fn append_pattern(modules: &mut Vec<Module>, pattern: &str) {
    let mut black = true;
    for width in pattern.bytes() {
        modules.push(Module {
            black,
            width: width - b'0',
        });
        black = !black;
    }
}

fn modules_from_bits(bits: &[bool]) -> Vec<Module> {
    let mut modules = Vec::new();
    let mut black = true;
    let mut width = 0_u8;
    for bit in bits {
        if *bit == black {
            width += 1;
        } else {
            modules.push(Module { black, width });
            black = *bit;
            width = 1;
        }
    }
    if width > 0 {
        modules.push(Module { black, width });
    }
    modules
}

fn push_barcode_bits(bits: &mut Vec<bool>, pattern: &str) {
    bits.extend(pattern.bytes().map(|byte| byte == b'1'));
}

fn upc_a_digits(value: &str) -> Result<String, BarcodeError> {
    let digits = value
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .collect::<String>();
    if digits.is_empty() {
        return Err(BarcodeError::Empty);
    }
    if let Some(ch) = digits.chars().find(|ch| !ch.is_ascii_digit()) {
        return Err(BarcodeError::UnsupportedCharacter(ch));
    }
    match digits.len() {
        11 => {
            let check = upc_a_check_digit(&digits);
            Ok(format!("{digits}{check}"))
        }
        12 => {
            let expected = upc_a_check_digit(&digits[..11]);
            if digits.as_bytes()[11] - b'0' == expected {
                Ok(digits)
            } else {
                Err(BarcodeError::InvalidChecksum { kind: "UPC-A" })
            }
        }
        _ => Err(BarcodeError::InvalidLength {
            kind: "UPC-A",
            expected: "11 digits without a check digit or 12 digits with a valid check digit",
        }),
    }
}

fn upc_a_check_digit(first_11_digits: &str) -> u8 {
    let sum = first_11_digits
        .bytes()
        .enumerate()
        .map(|(idx, byte)| {
            let digit = (byte - b'0') as u32;
            if idx % 2 == 0 {
                digit * 3
            } else {
                digit
            }
        })
        .sum::<u32>();
    ((10 - sum % 10) % 10) as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gs1Symbol {
    Char(char),
    Fnc1,
}

fn normalize_gs1_payload(value: &str) -> Result<Vec<Gs1Symbol>, BarcodeError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(BarcodeError::Empty);
    }
    if value.contains('(') || value.contains(')') {
        normalize_parenthesized_gs1(value)
    } else {
        value.chars().map(validate_gs1_char).collect()
    }
}

fn gs1_symbols_to_string(symbols: &[Gs1Symbol]) -> String {
    symbols
        .iter()
        .map(|symbol| match symbol {
            Gs1Symbol::Char(ch) => *ch,
            Gs1Symbol::Fnc1 => '\u{1d}',
        })
        .collect()
}

fn normalize_parenthesized_gs1(value: &str) -> Result<Vec<Gs1Symbol>, BarcodeError> {
    let mut symbols = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index] != b'(' {
            return Err(BarcodeError::InvalidGs1Ai(value[index..].to_owned()));
        }
        let ai_start = index + 1;
        let Some(ai_end_offset) = value[ai_start..].find(')') else {
            return Err(BarcodeError::InvalidGs1Ai(value[ai_start..].to_owned()));
        };
        let ai_end = ai_start + ai_end_offset;
        let ai = &value[ai_start..ai_end];
        let spec = gs1_ai_spec(ai).ok_or_else(|| BarcodeError::InvalidGs1Ai(ai.to_owned()))?;
        for ch in ai.chars() {
            symbols.push(validate_gs1_char(ch)?);
        }
        index = ai_end + 1;

        let data_start = index;
        while index < bytes.len() && bytes[index] != b'(' {
            index += 1;
        }
        let data = value[data_start..index].trim();
        validate_gs1_data_length(ai, data, spec)?;
        for ch in data.chars() {
            symbols.push(validate_gs1_char(ch)?);
        }
        if !spec.fixed && index < bytes.len() {
            symbols.push(Gs1Symbol::Fnc1);
        }
    }
    if symbols.is_empty() {
        Err(BarcodeError::Empty)
    } else {
        Ok(symbols)
    }
}

fn validate_gs1_char(ch: char) -> Result<Gs1Symbol, BarcodeError> {
    if ch == '\u{1d}' {
        Ok(Gs1Symbol::Fnc1)
    } else if ch.is_ascii() && !ch.is_ascii_control() {
        Ok(Gs1Symbol::Char(ch))
    } else {
        Err(BarcodeError::UnsupportedCharacter(ch))
    }
}

#[derive(Debug, Clone, Copy)]
struct Gs1AiSpec {
    fixed: bool,
    len: usize,
}

fn gs1_ai_spec(ai: &str) -> Option<Gs1AiSpec> {
    match ai {
        "00" => Some(Gs1AiSpec {
            fixed: true,
            len: 18,
        }),
        "01" | "02" => Some(Gs1AiSpec {
            fixed: true,
            len: 14,
        }),
        "10" | "21" | "22" | "30" | "37" => Some(Gs1AiSpec {
            fixed: false,
            len: if ai == "30" || ai == "37" { 8 } else { 20 },
        }),
        "11" | "12" | "13" | "15" | "16" | "17" => Some(Gs1AiSpec {
            fixed: true,
            len: 6,
        }),
        "20" => Some(Gs1AiSpec {
            fixed: true,
            len: 2,
        }),
        "240" | "241" | "242" | "243" | "250" | "251" | "253" | "254" | "400" | "401" | "403"
        | "420" | "421" | "422" | "423" | "424" | "425" | "426" => Some(Gs1AiSpec {
            fixed: false,
            len: match ai {
                "242" | "422" | "424" | "425" | "426" => 6,
                "420" => 20,
                "421" => 12,
                "423" => 15,
                _ => 30,
            },
        }),
        _ if ai.len() == 4
            && ai.bytes().all(|byte| byte.is_ascii_digit())
            && matches!(&ai[..2], "31" | "32" | "33" | "34" | "35" | "36") =>
        {
            Some(Gs1AiSpec {
                fixed: true,
                len: 6,
            })
        }
        _ => None,
    }
}

fn validate_gs1_data_length(ai: &str, data: &str, spec: Gs1AiSpec) -> Result<(), BarcodeError> {
    let len = data.chars().count();
    let valid = if spec.fixed {
        len == spec.len
    } else {
        len > 0 && len <= spec.len
    };
    valid
        .then_some(())
        .ok_or_else(|| BarcodeError::InvalidGs1Ai(ai.to_owned()))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn encode_qr(value: &str) -> Result<EncodedQr, BarcodeError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(BarcodeError::Empty);
    }
    let bytes = value.as_bytes();
    let version = QR_VERSIONS
        .iter()
        .find(|info| bytes.len() <= info.byte_capacity())
        .copied()
        .ok_or(BarcodeError::DataTooLong {
            max_bytes: QR_VERSIONS
                .last()
                .copied()
                .map(QrVersionInfo::byte_capacity)
                .unwrap_or_default(),
        })?;

    let data = qr_data_codewords(bytes, version.data_codewords);
    let ecc = reed_solomon_remainder(&data, version.ecc_codewords);
    let mut codewords = data;
    codewords.extend(ecc);
    Ok(build_qr_matrix(version.version, &codewords))
}

#[derive(Debug, Clone, Copy)]
struct QrVersionInfo {
    version: u8,
    data_codewords: usize,
    ecc_codewords: usize,
}

impl QrVersionInfo {
    fn byte_capacity(self) -> usize {
        // Byte mode indicator: 4 bits. Version 1-9 count indicator: 8 bits.
        ((self.data_codewords * 8).saturating_sub(12)) / 8
    }
}

const QR_VERSIONS: [QrVersionInfo; 5] = [
    QrVersionInfo {
        version: 1,
        data_codewords: 19,
        ecc_codewords: 7,
    },
    QrVersionInfo {
        version: 2,
        data_codewords: 34,
        ecc_codewords: 10,
    },
    QrVersionInfo {
        version: 3,
        data_codewords: 55,
        ecc_codewords: 15,
    },
    QrVersionInfo {
        version: 4,
        data_codewords: 80,
        ecc_codewords: 20,
    },
    QrVersionInfo {
        version: 5,
        data_codewords: 108,
        ecc_codewords: 26,
    },
];

fn qr_data_codewords(data: &[u8], data_codewords: usize) -> Vec<u8> {
    let capacity_bits = data_codewords * 8;
    let mut bits = Vec::with_capacity(capacity_bits);
    push_bits(&mut bits, 0b0100, 4); // Byte mode
    push_bits(&mut bits, data.len() as u32, 8);
    for byte in data {
        push_bits(&mut bits, *byte as u32, 8);
    }

    let terminator = 4.min(capacity_bits.saturating_sub(bits.len()));
    bits.extend(std::iter::repeat(false).take(terminator));
    while bits.len() % 8 != 0 {
        bits.push(false);
    }

    let mut result = bits_to_bytes(&bits);
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if result.len() >= data_codewords {
            break;
        }
        result.push(pad);
    }
    result
}

fn push_bits(bits: &mut Vec<bool>, value: u32, len: usize) {
    for i in (0..len).rev() {
        bits.push(((value >> i) & 1) != 0);
    }
}

fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .fold(0_u8, |acc, bit| (acc << 1) | if *bit { 1 } else { 0 })
        })
        .collect()
}

fn build_qr_matrix(version: u8, codewords: &[u8]) -> EncodedQr {
    let size = 21 + (version as usize - 1) * 4;
    let mut modules = vec![false; size * size];
    let mut reserved = vec![false; size * size];

    draw_finder(&mut modules, &mut reserved, size, 0, 0);
    draw_finder(&mut modules, &mut reserved, size, size - 7, 0);
    draw_finder(&mut modules, &mut reserved, size, 0, size - 7);
    draw_timing(&mut modules, &mut reserved, size);
    draw_alignment(&mut modules, &mut reserved, size, version);
    reserve_format_areas(&mut reserved, size);
    set_function(
        &mut modules,
        &mut reserved,
        size,
        8,
        4 * version as usize + 9,
        true,
    );

    draw_data(&mut modules, &reserved, size, codewords);
    draw_format_bits(&mut modules, &mut reserved, size, 0);

    EncodedQr {
        version,
        size,
        modules,
    }
}

fn idx(size: usize, x: usize, y: usize) -> usize {
    y * size + x
}

fn set_function(
    modules: &mut [bool],
    reserved: &mut [bool],
    size: usize,
    x: usize,
    y: usize,
    black: bool,
) {
    modules[idx(size, x, y)] = black;
    reserved[idx(size, x, y)] = true;
}

fn draw_finder(modules: &mut [bool], reserved: &mut [bool], size: usize, x: usize, y: usize) {
    for dy in -1_i32..=7 {
        for dx in -1_i32..=7 {
            let xx = x as i32 + dx;
            let yy = y as i32 + dy;
            if xx < 0 || yy < 0 || xx >= size as i32 || yy >= size as i32 {
                continue;
            }
            let in_finder = (0..=6).contains(&dx) && (0..=6).contains(&dy);
            let black = in_finder
                && (dx == 0
                    || dx == 6
                    || dy == 0
                    || dy == 6
                    || ((2..=4).contains(&dx) && (2..=4).contains(&dy)));
            set_function(modules, reserved, size, xx as usize, yy as usize, black);
        }
    }
}

fn draw_timing(modules: &mut [bool], reserved: &mut [bool], size: usize) {
    for i in 8..(size - 8) {
        let black = i % 2 == 0;
        set_function(modules, reserved, size, i, 6, black);
        set_function(modules, reserved, size, 6, i, black);
    }
}

fn draw_alignment(modules: &mut [bool], reserved: &mut [bool], size: usize, version: u8) {
    if version == 1 {
        return;
    }
    let center = 4 * version as usize + 10;
    for dy in -2_i32..=2 {
        for dx in -2_i32..=2 {
            let x = (center as i32 + dx) as usize;
            let y = (center as i32 + dy) as usize;
            let black = dx.abs() == 2 || dy.abs() == 2 || (dx == 0 && dy == 0);
            set_function(modules, reserved, size, x, y, black);
        }
    }
}

fn reserve_format_areas(reserved: &mut [bool], size: usize) {
    for i in 0..=8 {
        if i != 6 {
            reserved[idx(size, 8, i)] = true;
            reserved[idx(size, i, 8)] = true;
        }
    }
    for i in 0..8 {
        reserved[idx(size, size - 1 - i, 8)] = true;
        reserved[idx(size, 8, size - 1 - i)] = true;
    }
}

fn draw_data(modules: &mut [bool], reserved: &[bool], size: usize, codewords: &[u8]) {
    let mut bits = Vec::with_capacity(codewords.len() * 8);
    for byte in codewords {
        push_bits(&mut bits, *byte as u32, 8);
    }

    let mut bit_idx = 0;
    let mut upward = true;
    let mut right = size - 1;
    while right > 0 {
        if right == 6 {
            right -= 1;
        }
        for vert in 0..size {
            let y = if upward { size - 1 - vert } else { vert };
            for col in 0..2 {
                let x = right - col;
                let index = idx(size, x, y);
                if reserved[index] {
                    continue;
                }
                let bit = bits.get(bit_idx).copied().unwrap_or(false);
                bit_idx += 1;
                modules[index] = bit ^ qr_mask_0(x, y);
            }
        }
        upward = !upward;
        right = right.saturating_sub(2);
    }
}

fn qr_mask_0(x: usize, y: usize) -> bool {
    (x + y) % 2 == 0
}

fn draw_format_bits(modules: &mut [bool], reserved: &mut [bool], size: usize, mask: u8) {
    let bits = format_bits_l(mask);
    for i in 0..=5 {
        set_function(modules, reserved, size, 8, i, ((bits >> i) & 1) != 0);
    }
    set_function(modules, reserved, size, 8, 7, ((bits >> 6) & 1) != 0);
    set_function(modules, reserved, size, 8, 8, ((bits >> 7) & 1) != 0);
    set_function(modules, reserved, size, 7, 8, ((bits >> 8) & 1) != 0);
    for i in 9..15 {
        set_function(modules, reserved, size, 14 - i, 8, ((bits >> i) & 1) != 0);
    }
    for i in 0..8 {
        set_function(
            modules,
            reserved,
            size,
            size - 1 - i,
            8,
            ((bits >> i) & 1) != 0,
        );
    }
    for i in 8..15 {
        set_function(
            modules,
            reserved,
            size,
            8,
            size - 15 + i,
            ((bits >> i) & 1) != 0,
        );
    }
    set_function(modules, reserved, size, 8, size - 8, true);
}

fn format_bits_l(mask: u8) -> u16 {
    let data = (0b01_u16 << 3) | mask as u16;
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ if (rem & 0x200) != 0 { 0x537 } else { 0 };
    }
    ((data << 10) | (rem & 0x3FF)) ^ 0x5412
}

fn reed_solomon_remainder(data: &[u8], degree: usize) -> Vec<u8> {
    let divisor = reed_solomon_divisor(degree);
    let mut result = vec![0_u8; degree];
    for byte in data {
        let factor = byte ^ result[0];
        result.copy_within(1.., 0);
        result[degree - 1] = 0;
        for (slot, coefficient) in result.iter_mut().zip(divisor.iter()) {
            *slot ^= gf_multiply(*coefficient, factor);
        }
    }
    result
}

fn reed_solomon_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0_u8; degree];
    result[degree - 1] = 1;
    let mut root = 1_u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_multiply(root, 0x02);
    }
    result
}

fn gf_multiply(mut x: u8, mut y: u8) -> u8 {
    let mut z = 0_u8;
    while y != 0 {
        if (y & 1) != 0 {
            z ^= x;
        }
        let carry = (x & 0x80) != 0;
        x <<= 1;
        if carry {
            x ^= 0x1D;
        }
        y >>= 1;
    }
    z
}

const CODE128_PATTERNS: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212",
    "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221",
    "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221",
    "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321",
    "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131",
    "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111",
    "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114",
    "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242",
    "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311",
    "113141", "114131", "311141", "411131", "211412", "211214", "211232", "2331112",
];

const UPC_LEFT_PATTERNS: [&str; 10] = [
    "0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011",
    "0110111", "0001011",
];

const UPC_RIGHT_PATTERNS: [&str; 10] = [
    "1110010", "1100110", "1101100", "1000010", "1011100", "1001110", "1010000", "1000100",
    "1001000", "1110100",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_code128_b_checksum_and_stop() {
        let encoded = encode_code128_b("AB").unwrap();
        let modules = encoded
            .modules
            .iter()
            .map(|module| char::from(b'0' + module.width))
            .collect::<String>();
        assert_eq!(
            modules,
            // Start B, A=33, B=34, checksum=(104 + 33*1 + 34*2)%103 = 102, Stop.
            "2112141113231311234111312331112"
        );
    }

    #[test]
    fn rejects_non_code128_b_characters() {
        assert!(matches!(
            encode_code128_b("abcé"),
            Err(BarcodeError::UnsupportedCharacter('é'))
        ));
    }

    #[test]
    fn encodes_gs1_128_with_initial_and_separator_fnc1() {
        let encoded = encode_gs1_128("(01)09506000134352(10)ABC123(17)260101").unwrap();
        let modules = encoded
            .modules
            .iter()
            .map(|module| char::from(b'0' + module.width))
            .collect::<String>();
        assert!(modules.starts_with("211214411131"));
        assert!(modules.ends_with("2331112"));
        assert_eq!(
            normalized_value("gs1-128", "(01)09506000134352(10)ABC123(17)260101").unwrap(),
            "010950600013435210ABC123\u{1d}17260101"
        );
    }

    #[test]
    fn validates_gs1_ai_lengths() {
        assert!(matches!(
            encode_gs1_128("(01)123"),
            Err(BarcodeError::InvalidGs1Ai(ai)) if ai == "01"
        ));
    }

    #[test]
    fn encodes_upc_a_and_appends_check_digit() {
        let encoded = encode_upc_a("03600029145").unwrap();
        assert_eq!(upc_a_digits("03600029145").unwrap(), "036000291452");
        assert_eq!(
            normalized_value("upc-a", "03600029145").unwrap(),
            "036000291452"
        );
        assert_eq!(encoded.total_modules(), 95);
        assert!(svg("upc-a", "03600029145")
            .unwrap()
            .contains("036000291452"));
    }

    #[test]
    fn rejects_invalid_upc_a_check_digit() {
        assert!(matches!(
            encode_upc_a("036000291453"),
            Err(BarcodeError::InvalidChecksum { kind: "UPC-A" })
        ));
    }

    #[test]
    fn encodes_qr_version_1_for_short_byte_payload() {
        let encoded = encode_qr("HELLO").unwrap();
        assert_eq!(encoded.version, 1);
        assert_eq!(encoded.size, 21);
        assert!(encoded.module(0, 0));
        assert!(encoded.module(10, 6));
        assert!(svg("qr", "HELLO").unwrap().contains("<svg"));
    }

    #[test]
    fn rejects_qr_payloads_beyond_supported_versions() {
        let long = "x".repeat(107);
        assert!(matches!(
            encode_qr(&long),
            Err(BarcodeError::DataTooLong { max_bytes: 106 })
        ));
    }
}
