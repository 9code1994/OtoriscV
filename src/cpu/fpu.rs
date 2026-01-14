//! Floating-Point Unit (F and D extensions)
//!
//! Implements RV32F (single-precision) and RV32D (double-precision) extensions

use serde::{Serialize, Deserialize};

/// Rounding modes
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum RoundingMode {
    /// Round to Nearest, ties to Even
    RNE = 0b000,
    /// Round towards Zero
    RTZ = 0b001,
    /// Round Down (towards -∞)
    RDN = 0b010,
    /// Round Up (towards +∞)
    RUP = 0b011,
    /// Round to Nearest, ties to Max Magnitude
    RMM = 0b100,
    /// Dynamic (use frm)
    DYN = 0b111,
}

impl From<u32> for RoundingMode {
    fn from(val: u32) -> Self {
        match val & 0b111 {
            0b000 => RoundingMode::RNE,
            0b001 => RoundingMode::RTZ,
            0b010 => RoundingMode::RDN,
            0b011 => RoundingMode::RUP,
            0b100 => RoundingMode::RMM,
            _ => RoundingMode::DYN,
        }
    }
}

/// Exception flags (fflags)
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct FFlags {
    /// Invalid Operation
    pub nv: bool,
    /// Divide by Zero
    pub dz: bool,
    /// Overflow
    pub of: bool,
    /// Underflow
    pub uf: bool,
    /// Inexact
    pub nx: bool,
}

impl FFlags {
    pub fn to_bits(&self) -> u32 {
        (self.nx as u32) |
        ((self.uf as u32) << 1) |
        ((self.of as u32) << 2) |
        ((self.dz as u32) << 3) |
        ((self.nv as u32) << 4)
    }
    
    pub fn from_bits(bits: u32) -> Self {
        FFlags {
            nx: (bits & 0b00001) != 0,
            uf: (bits & 0b00010) != 0,
            of: (bits & 0b00100) != 0,
            dz: (bits & 0b01000) != 0,
            nv: (bits & 0b10000) != 0,
        }
    }
    
    pub fn merge(&mut self, other: FFlags) {
        self.nx |= other.nx;
        self.uf |= other.uf;
        self.of |= other.of;
        self.dz |= other.dz;
        self.nv |= other.nv;
    }
}

/// Floating-point register file and state
#[derive(Serialize, Deserialize)]
pub struct Fpu {
    /// Floating-point registers (64-bit each for D extension, NaN-boxed for F)
    pub fregs: [u64; 32],
    /// Floating-point rounding mode (frm)
    pub frm: RoundingMode,
    /// Floating-point exception flags (fflags)
    pub fflags: FFlags,
}

impl Fpu {
    pub fn new() -> Self {
        Fpu {
            fregs: [0u64; 32],
            frm: RoundingMode::RNE,
            fflags: FFlags::default(),
        }
    }
    
    /// Read single-precision register (returns f32 bits)
    /// F registers are NaN-boxed in D registers - upper 32 bits must be all 1s
    #[inline(always)]
    pub fn read_f32(&self, reg: u32) -> u32 {
        let val = self.fregs[reg as usize & 0x1F];
        // If not properly NaN-boxed, return canonical NaN
        if (val >> 32) != 0xFFFF_FFFF {
            0x7FC0_0000 // Canonical NaN
        } else {
            val as u32
        }
    }
    
    /// Write single-precision register (NaN-boxes the value)
    #[inline(always)]
    pub fn write_f32(&mut self, reg: u32, value: u32) {
        // NaN-box: set upper 32 bits to all 1s
        self.fregs[reg as usize & 0x1F] = 0xFFFF_FFFF_0000_0000 | (value as u64);
    }
    
    /// Read double-precision register
    #[inline(always)]
    pub fn read_f64(&self, reg: u32) -> u64 {
        self.fregs[reg as usize & 0x1F]
    }
    
    /// Write double-precision register
    #[inline(always)]
    pub fn write_f64(&mut self, reg: u32, value: u64) {
        self.fregs[reg as usize & 0x1F] = value;
    }
    
    /// Read FCSR (frm + fflags)
    pub fn read_fcsr(&self) -> u32 {
        ((self.frm as u32) << 5) | self.fflags.to_bits()
    }
    
    /// Write FCSR
    pub fn write_fcsr(&mut self, value: u32) {
        self.fflags = FFlags::from_bits(value & 0x1F);
        self.frm = RoundingMode::from((value >> 5) & 0b111);
    }
    
    /// Get effective rounding mode (resolve DYN)
    pub fn effective_rm(&self, inst_rm: u32) -> RoundingMode {
        let rm = RoundingMode::from(inst_rm);
        if matches!(rm, RoundingMode::DYN) {
            self.frm
        } else {
            rm
        }
    }
    
    pub fn reset(&mut self) {
        self.fregs = [0u64; 32];
        self.frm = RoundingMode::RNE;
        self.fflags = FFlags::default();
    }
}

// ============================================================================
// Single-precision (F32) operations
// ============================================================================

/// Check if f32 is a signaling NaN
pub fn f32_is_snan(bits: u32) -> bool {
    let exp = (bits >> 23) & 0xFF;
    let frac = bits & 0x7FFFFF;
    exp == 0xFF && frac != 0 && (frac & 0x400000) == 0
}

/// Check if f32 is any NaN
pub fn f32_is_nan(bits: u32) -> bool {
    let exp = (bits >> 23) & 0xFF;
    let frac = bits & 0x7FFFFF;
    exp == 0xFF && frac != 0
}

/// Canonical NaN for f32
pub const F32_CANONICAL_NAN: u32 = 0x7FC0_0000;

/// f32 add with flags
pub fn f32_add(a: u32, b: u32, rm: RoundingMode) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    // Check for signaling NaN
    if f32_is_snan(a) || f32_is_snan(b) {
        flags.nv = true;
    }
    
    let result = match rm {
        RoundingMode::RNE => af + bf, // Rust default is RNE
        RoundingMode::RTZ => {
            // TODO: proper RTZ implementation
            af + bf
        }
        _ => af + bf,
    };
    
    if result.is_nan() {
        if !af.is_nan() && !bf.is_nan() {
            flags.nv = true;
        }
        return (F32_CANONICAL_NAN, flags);
    }
    
    if result.is_infinite() && !af.is_infinite() && !bf.is_infinite() {
        flags.of = true;
        flags.nx = true;
    }
    
    (result.to_bits(), flags)
}

/// f32 sub with flags
pub fn f32_sub(a: u32, b: u32, rm: RoundingMode) -> (u32, FFlags) {
    // Negate b and add
    let b_neg = b ^ 0x8000_0000;
    f32_add(a, b_neg, rm)
}

/// f32 mul with flags
pub fn f32_mul(a: u32, b: u32, rm: RoundingMode) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) || f32_is_snan(b) {
        flags.nv = true;
    }
    
    // Check for 0 * inf
    if (af == 0.0 && bf.is_infinite()) || (af.is_infinite() && bf == 0.0) {
        flags.nv = true;
        return (F32_CANONICAL_NAN, flags);
    }
    
    let result = af * bf;
    
    if result.is_nan() {
        if !af.is_nan() && !bf.is_nan() {
            flags.nv = true;
        }
        return (F32_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f32 div with flags
pub fn f32_div(a: u32, b: u32, rm: RoundingMode) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) || f32_is_snan(b) {
        flags.nv = true;
    }
    
    // Check for 0/0 or inf/inf
    if (af == 0.0 && bf == 0.0) || (af.is_infinite() && bf.is_infinite()) {
        flags.nv = true;
        return (F32_CANONICAL_NAN, flags);
    }
    
    // Check for x/0 (divide by zero)
    if bf == 0.0 && !af.is_nan() {
        flags.dz = true;
    }
    
    let result = af / bf;
    
    if result.is_nan() && !af.is_nan() && !bf.is_nan() {
        flags.nv = true;
        return (F32_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f32 sqrt with flags
pub fn f32_sqrt(a: u32, _rm: RoundingMode) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) {
        flags.nv = true;
    }
    
    // sqrt of negative number (except -0)
    if af < 0.0 && a != 0x8000_0000 {
        flags.nv = true;
        return (F32_CANONICAL_NAN, flags);
    }
    
    let result = af.sqrt();
    
    if result.is_nan() && !af.is_nan() {
        flags.nv = true;
        return (F32_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f32 min with flags (returns smaller, or canonical NaN)
pub fn f32_min(a: u32, b: u32) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) || f32_is_snan(b) {
        flags.nv = true;
    }
    
    if af.is_nan() && bf.is_nan() {
        return (F32_CANONICAL_NAN, flags);
    }
    
    if af.is_nan() {
        return (b, flags);
    }
    if bf.is_nan() {
        return (a, flags);
    }
    
    // Handle -0 vs +0
    if a == 0x8000_0000 && b == 0 {
        return (a, flags); // -0 < +0
    }
    if a == 0 && b == 0x8000_0000 {
        return (b, flags);
    }
    
    if af < bf { (a, flags) } else { (b, flags) }
}

/// f32 max with flags
pub fn f32_max(a: u32, b: u32) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) || f32_is_snan(b) {
        flags.nv = true;
    }
    
    if af.is_nan() && bf.is_nan() {
        return (F32_CANONICAL_NAN, flags);
    }
    
    if af.is_nan() {
        return (b, flags);
    }
    if bf.is_nan() {
        return (a, flags);
    }
    
    // Handle -0 vs +0
    if a == 0x8000_0000 && b == 0 {
        return (b, flags); // +0 > -0
    }
    if a == 0 && b == 0x8000_0000 {
        return (a, flags);
    }
    
    if af > bf { (a, flags) } else { (b, flags) }
}

/// f32 compare equal
pub fn f32_eq(a: u32, b: u32) -> (bool, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) || f32_is_snan(b) {
        flags.nv = true;
    }
    
    (af == bf, flags)
}

/// f32 compare less than
pub fn f32_lt(a: u32, b: u32) -> (bool, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_nan(a) || f32_is_nan(b) {
        flags.nv = true;
        return (false, flags);
    }
    
    (af < bf, flags)
}

/// f32 compare less than or equal
pub fn f32_le(a: u32, b: u32) -> (bool, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let mut flags = FFlags::default();
    
    if f32_is_nan(a) || f32_is_nan(b) {
        flags.nv = true;
        return (false, flags);
    }
    
    (af <= bf, flags)
}

/// f32 to i32 conversion
pub fn f32_to_i32(a: u32, rm: RoundingMode) -> (i32, FFlags) {
    let af = f32::from_bits(a);
    let mut flags = FFlags::default();
    
    if af.is_nan() {
        flags.nv = true;
        return (i32::MAX, flags);
    }
    
    if af >= (i32::MAX as f32) {
        flags.nv = true;
        return (i32::MAX, flags);
    }
    
    if af <= (i32::MIN as f32) {
        flags.nv = true;
        return (i32::MIN, flags);
    }
    
    let result = match rm {
        RoundingMode::RTZ => af.trunc() as i32,
        RoundingMode::RDN => af.floor() as i32,
        RoundingMode::RUP => af.ceil() as i32,
        RoundingMode::RNE | RoundingMode::RMM => af.round() as i32,
        RoundingMode::DYN => af.round() as i32,
    };
    
    if (result as f32) != af {
        flags.nx = true;
    }
    
    (result, flags)
}

/// f32 to u32 conversion
pub fn f32_to_u32(a: u32, rm: RoundingMode) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let mut flags = FFlags::default();
    
    if af.is_nan() {
        flags.nv = true;
        return (u32::MAX, flags);
    }
    
    if af >= (u32::MAX as f32) {
        flags.nv = true;
        return (u32::MAX, flags);
    }
    
    if af < 0.0 {
        flags.nv = true;
        return (0, flags);
    }
    
    let result = match rm {
        RoundingMode::RTZ => af.trunc() as u32,
        RoundingMode::RDN => af.floor() as u32,
        RoundingMode::RUP => af.ceil() as u32,
        RoundingMode::RNE | RoundingMode::RMM => af.round() as u32,
        RoundingMode::DYN => af.round() as u32,
    };
    
    if (result as f32) != af {
        flags.nx = true;
    }
    
    (result, flags)
}

/// f32 to i64 conversion
pub fn f32_to_i64(a: u32, rm: RoundingMode) -> (i64, FFlags) {
    let af = f32::from_bits(a);
    let mut flags = FFlags::default();

    if af.is_nan() {
        flags.nv = true;
        return (i64::MAX, flags);
    }

    if af >= (i64::MAX as f32) {
        flags.nv = true;
        return (i64::MAX, flags);
    }

    if af <= (i64::MIN as f32) {
        flags.nv = true;
        return (i64::MIN, flags);
    }

    let result = match rm {
        RoundingMode::RTZ => af.trunc() as i64,
        RoundingMode::RDN => af.floor() as i64,
        RoundingMode::RUP => af.ceil() as i64,
        RoundingMode::RNE | RoundingMode::RMM => af.round() as i64,
        RoundingMode::DYN => af.round() as i64,
    };

    if (result as f32) != af {
        flags.nx = true;
    }

    (result, flags)
}

/// f32 to u64 conversion
pub fn f32_to_u64(a: u32, rm: RoundingMode) -> (u64, FFlags) {
    let af = f32::from_bits(a);
    let mut flags = FFlags::default();

    if af.is_nan() {
        flags.nv = true;
        return (u64::MAX, flags);
    }

    if af >= (u64::MAX as f32) {
        flags.nv = true;
        return (u64::MAX, flags);
    }

    if af < 0.0 {
        flags.nv = true;
        return (0, flags);
    }

    let result = match rm {
        RoundingMode::RTZ => af.trunc() as u64,
        RoundingMode::RDN => af.floor() as u64,
        RoundingMode::RUP => af.ceil() as u64,
        RoundingMode::RNE | RoundingMode::RMM => af.round() as u64,
        RoundingMode::DYN => af.round() as u64,
    };

    if (result as f32) != af {
        flags.nx = true;
    }

    (result, flags)
}

/// i32 to f32 conversion
pub fn i32_to_f32(a: i32, _rm: RoundingMode) -> (u32, FFlags) {
    let result = a as f32;
    let mut flags = FFlags::default();
    
    // Check if conversion was exact
    if (result as i32) != a {
        flags.nx = true;
    }
    
    (result.to_bits(), flags)
}

/// u32 to f32 conversion
pub fn u32_to_f32(a: u32, _rm: RoundingMode) -> (u32, FFlags) {
    let result = a as f32;
    let mut flags = FFlags::default();
    
    if (result as u32) != a {
        flags.nx = true;
    }
    
    (result.to_bits(), flags)
}

/// i64 to f32 conversion
pub fn i64_to_f32(a: i64, _rm: RoundingMode) -> (u32, FFlags) {
    let result = a as f32;
    let mut flags = FFlags::default();
    if (result as i64) != a {
        flags.nx = true;
    }
    (result.to_bits(), flags)
}

/// u64 to f32 conversion
pub fn u64_to_f32(a: u64, _rm: RoundingMode) -> (u32, FFlags) {
    let result = a as f32;
    let mut flags = FFlags::default();
    if (result as u64) != a {
        flags.nx = true;
    }
    (result.to_bits(), flags)
}

/// Sign injection: copy sign from b to a
pub fn f32_sgnj(a: u32, b: u32) -> u32 {
    (a & 0x7FFF_FFFF) | (b & 0x8000_0000)
}

/// Sign injection negative: copy negated sign from b to a
pub fn f32_sgnjn(a: u32, b: u32) -> u32 {
    (a & 0x7FFF_FFFF) | ((b ^ 0x8000_0000) & 0x8000_0000)
}

/// Sign injection xor: xor signs
pub fn f32_sgnjx(a: u32, b: u32) -> u32 {
    a ^ (b & 0x8000_0000)
}

/// Classify f32 (FCLASS.S)
pub fn f32_classify(a: u32) -> u32 {
    let sign = (a >> 31) & 1;
    let exp = (a >> 23) & 0xFF;
    let frac = a & 0x7F_FFFF;
    
    if exp == 0 {
        if frac == 0 {
            // Zero
            if sign != 0 { 1 << 3 } else { 1 << 4 } // -0 or +0
        } else {
            // Subnormal
            if sign != 0 { 1 << 2 } else { 1 << 5 }
        }
    } else if exp == 0xFF {
        if frac == 0 {
            // Infinity
            if sign != 0 { 1 << 0 } else { 1 << 7 }
        } else {
            // NaN
            if (frac & 0x40_0000) != 0 {
                1 << 9 // Quiet NaN
            } else {
                1 << 8 // Signaling NaN
            }
        }
    } else {
        // Normal
        if sign != 0 { 1 << 1 } else { 1 << 6 }
    }
}

// ============================================================================
// Double-precision (F64) operations
// ============================================================================

/// Check if f64 is a signaling NaN
pub fn f64_is_snan(bits: u64) -> bool {
    let exp = (bits >> 52) & 0x7FF;
    let frac = bits & 0xF_FFFF_FFFF_FFFF;
    exp == 0x7FF && frac != 0 && (frac & 0x8_0000_0000_0000) == 0
}

/// Check if f64 is any NaN
pub fn f64_is_nan(bits: u64) -> bool {
    let exp = (bits >> 52) & 0x7FF;
    let frac = bits & 0xF_FFFF_FFFF_FFFF;
    exp == 0x7FF && frac != 0
}

/// Canonical NaN for f64
pub const F64_CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

/// f64 add with flags
pub fn f64_add(a: u64, b: u64, _rm: RoundingMode) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) {
        flags.nv = true;
    }
    
    let result = af + bf;
    
    if result.is_nan() {
        if !af.is_nan() && !bf.is_nan() {
            flags.nv = true;
        }
        return (F64_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f64 sub with flags
pub fn f64_sub(a: u64, b: u64, rm: RoundingMode) -> (u64, FFlags) {
    let b_neg = b ^ 0x8000_0000_0000_0000;
    f64_add(a, b_neg, rm)
}

/// f64 mul with flags
pub fn f64_mul(a: u64, b: u64, _rm: RoundingMode) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) {
        flags.nv = true;
    }
    
    if (af == 0.0 && bf.is_infinite()) || (af.is_infinite() && bf == 0.0) {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    let result = af * bf;
    
    if result.is_nan() && !af.is_nan() && !bf.is_nan() {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f64 div with flags
pub fn f64_div(a: u64, b: u64, _rm: RoundingMode) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) {
        flags.nv = true;
    }
    
    if (af == 0.0 && bf == 0.0) || (af.is_infinite() && bf.is_infinite()) {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    if bf == 0.0 && !af.is_nan() {
        flags.dz = true;
    }
    
    let result = af / bf;
    
    if result.is_nan() && !af.is_nan() && !bf.is_nan() {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f64 sqrt with flags
pub fn f64_sqrt(a: u64, _rm: RoundingMode) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) {
        flags.nv = true;
    }
    
    if af < 0.0 && a != 0x8000_0000_0000_0000 {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    let result = af.sqrt();
    
    if result.is_nan() && !af.is_nan() {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// f64 min with flags
pub fn f64_min(a: u64, b: u64) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) {
        flags.nv = true;
    }
    
    if af.is_nan() && bf.is_nan() {
        return (F64_CANONICAL_NAN, flags);
    }
    if af.is_nan() {
        return (b, flags);
    }
    if bf.is_nan() {
        return (a, flags);
    }
    
    if a == 0x8000_0000_0000_0000 && b == 0 {
        return (a, flags);
    }
    if a == 0 && b == 0x8000_0000_0000_0000 {
        return (b, flags);
    }
    
    if af < bf { (a, flags) } else { (b, flags) }
}

/// f64 max with flags
pub fn f64_max(a: u64, b: u64) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) {
        flags.nv = true;
    }
    
    if af.is_nan() && bf.is_nan() {
        return (F64_CANONICAL_NAN, flags);
    }
    if af.is_nan() {
        return (b, flags);
    }
    if bf.is_nan() {
        return (a, flags);
    }
    
    if a == 0x8000_0000_0000_0000 && b == 0 {
        return (b, flags);
    }
    if a == 0 && b == 0x8000_0000_0000_0000 {
        return (a, flags);
    }
    
    if af > bf { (a, flags) } else { (b, flags) }
}

/// f64 compare equal
pub fn f64_eq(a: u64, b: u64) -> (bool, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) {
        flags.nv = true;
    }
    
    (af == bf, flags)
}

/// f64 compare less than
pub fn f64_lt(a: u64, b: u64) -> (bool, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_nan(a) || f64_is_nan(b) {
        flags.nv = true;
        return (false, flags);
    }
    
    (af < bf, flags)
}

/// f64 compare less than or equal
pub fn f64_le(a: u64, b: u64) -> (bool, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let mut flags = FFlags::default();
    
    if f64_is_nan(a) || f64_is_nan(b) {
        flags.nv = true;
        return (false, flags);
    }
    
    (af <= bf, flags)
}

/// f64 to i32 conversion
pub fn f64_to_i32(a: u64, rm: RoundingMode) -> (i32, FFlags) {
    let af = f64::from_bits(a);
    let mut flags = FFlags::default();
    
    if af.is_nan() {
        flags.nv = true;
        return (i32::MAX, flags);
    }
    
    if af >= (i32::MAX as f64) + 1.0 {
        flags.nv = true;
        return (i32::MAX, flags);
    }
    
    if af < (i32::MIN as f64) {
        flags.nv = true;
        return (i32::MIN, flags);
    }
    
    let result = match rm {
        RoundingMode::RTZ => af.trunc() as i32,
        RoundingMode::RDN => af.floor() as i32,
        RoundingMode::RUP => af.ceil() as i32,
        _ => af.round() as i32,
    };
    
    if (result as f64) != af {
        flags.nx = true;
    }
    
    (result, flags)
}

/// f64 to u32 conversion
pub fn f64_to_u32(a: u64, rm: RoundingMode) -> (u32, FFlags) {
    let af = f64::from_bits(a);
    let mut flags = FFlags::default();
    
    if af.is_nan() {
        flags.nv = true;
        return (u32::MAX, flags);
    }
    
    if af >= (u32::MAX as f64) + 1.0 {
        flags.nv = true;
        return (u32::MAX, flags);
    }
    
    if af < 0.0 {
        flags.nv = true;
        return (0, flags);
    }
    
    let result = match rm {
        RoundingMode::RTZ => af.trunc() as u32,
        RoundingMode::RDN => af.floor() as u32,
        RoundingMode::RUP => af.ceil() as u32,
        _ => af.round() as u32,
    };
    
    if (result as f64) != af {
        flags.nx = true;
    }
    
    (result, flags)
}

/// f64 to i64 conversion
pub fn f64_to_i64(a: u64, rm: RoundingMode) -> (i64, FFlags) {
    let af = f64::from_bits(a);
    let mut flags = FFlags::default();

    if af.is_nan() {
        flags.nv = true;
        return (i64::MAX, flags);
    }

    if af >= (i64::MAX as f64) + 1.0 {
        flags.nv = true;
        return (i64::MAX, flags);
    }

    if af < (i64::MIN as f64) {
        flags.nv = true;
        return (i64::MIN, flags);
    }

    let result = match rm {
        RoundingMode::RTZ => af.trunc() as i64,
        RoundingMode::RDN => af.floor() as i64,
        RoundingMode::RUP => af.ceil() as i64,
        _ => af.round() as i64,
    };

    if (result as f64) != af {
        flags.nx = true;
    }

    (result, flags)
}

/// f64 to u64 conversion
pub fn f64_to_u64(a: u64, rm: RoundingMode) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let mut flags = FFlags::default();

    if af.is_nan() {
        flags.nv = true;
        return (u64::MAX, flags);
    }

    if af >= (u64::MAX as f64) + 1.0 {
        flags.nv = true;
        return (u64::MAX, flags);
    }

    if af < 0.0 {
        flags.nv = true;
        return (0, flags);
    }

    let result = match rm {
        RoundingMode::RTZ => af.trunc() as u64,
        RoundingMode::RDN => af.floor() as u64,
        RoundingMode::RUP => af.ceil() as u64,
        _ => af.round() as u64,
    };

    if (result as f64) != af {
        flags.nx = true;
    }

    (result, flags)
}

/// i32 to f64 conversion (always exact)
pub fn i32_to_f64(a: i32) -> u64 {
    (a as f64).to_bits()
}

/// u32 to f64 conversion (always exact)
pub fn u32_to_f64(a: u32) -> u64 {
    (a as f64).to_bits()
}

/// i64 to f64 conversion
pub fn i64_to_f64(a: i64) -> (u64, FFlags) {
    let result = a as f64;
    let mut flags = FFlags::default();
    if (result as i64) != a {
        flags.nx = true;
    }
    (result.to_bits(), flags)
}

/// u64 to f64 conversion
pub fn u64_to_f64(a: u64) -> (u64, FFlags) {
    let result = a as f64;
    let mut flags = FFlags::default();
    if (result as u64) != a {
        flags.nx = true;
    }
    (result.to_bits(), flags)
}

/// f32 to f64 conversion
pub fn f32_to_f64(a: u32) -> (u64, FFlags) {
    let af = f32::from_bits(a);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) {
        flags.nv = true;
        return (F64_CANONICAL_NAN, flags);
    }
    
    if af.is_nan() {
        return (F64_CANONICAL_NAN, flags);
    }
    
    ((af as f64).to_bits(), flags)
}

/// f64 to f32 conversion
pub fn f64_to_f32(a: u64, _rm: RoundingMode) -> (u32, FFlags) {
    let af = f64::from_bits(a);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) {
        flags.nv = true;
        return (F32_CANONICAL_NAN, flags);
    }
    
    if af.is_nan() {
        return (F32_CANONICAL_NAN, flags);
    }
    
    let result = af as f32;
    
    if result.is_infinite() && !af.is_infinite() {
        flags.of = true;
        flags.nx = true;
    }
    
    if result == 0.0 && af != 0.0 {
        flags.uf = true;
        flags.nx = true;
    }
    
    (result.to_bits(), flags)
}

/// Sign injection for f64
pub fn f64_sgnj(a: u64, b: u64) -> u64 {
    (a & 0x7FFF_FFFF_FFFF_FFFF) | (b & 0x8000_0000_0000_0000)
}

pub fn f64_sgnjn(a: u64, b: u64) -> u64 {
    (a & 0x7FFF_FFFF_FFFF_FFFF) | ((b ^ 0x8000_0000_0000_0000) & 0x8000_0000_0000_0000)
}

pub fn f64_sgnjx(a: u64, b: u64) -> u64 {
    a ^ (b & 0x8000_0000_0000_0000)
}

/// Classify f64 (FCLASS.D)
pub fn f64_classify(a: u64) -> u32 {
    let sign = (a >> 63) & 1;
    let exp = (a >> 52) & 0x7FF;
    let frac = a & 0xF_FFFF_FFFF_FFFF;
    
    if exp == 0 {
        if frac == 0 {
            if sign != 0 { 1 << 3 } else { 1 << 4 }
        } else {
            if sign != 0 { 1 << 2 } else { 1 << 5 }
        }
    } else if exp == 0x7FF {
        if frac == 0 {
            if sign != 0 { 1 << 0 } else { 1 << 7 }
        } else {
            if (frac & 0x8_0000_0000_0000) != 0 {
                1 << 9
            } else {
                1 << 8
            }
        }
    } else {
        if sign != 0 { 1 << 1 } else { 1 << 6 }
    }
}

/// Fused multiply-add for f32: (a * b) + c
pub fn f32_fmadd(a: u32, b: u32, c: u32, _rm: RoundingMode) -> (u32, FFlags) {
    let af = f32::from_bits(a);
    let bf = f32::from_bits(b);
    let cf = f32::from_bits(c);
    let mut flags = FFlags::default();
    
    if f32_is_snan(a) || f32_is_snan(b) || f32_is_snan(c) {
        flags.nv = true;
    }
    
    // Use Rust's mul_add for fused operation
    let result = af.mul_add(bf, cf);
    
    if result.is_nan() {
        if !af.is_nan() && !bf.is_nan() && !cf.is_nan() {
            flags.nv = true;
        }
        return (F32_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}

/// Fused multiply-add for f64
pub fn f64_fmadd(a: u64, b: u64, c: u64, _rm: RoundingMode) -> (u64, FFlags) {
    let af = f64::from_bits(a);
    let bf = f64::from_bits(b);
    let cf = f64::from_bits(c);
    let mut flags = FFlags::default();
    
    if f64_is_snan(a) || f64_is_snan(b) || f64_is_snan(c) {
        flags.nv = true;
    }
    
    let result = af.mul_add(bf, cf);
    
    if result.is_nan() {
        if !af.is_nan() && !bf.is_nan() && !cf.is_nan() {
            flags.nv = true;
        }
        return (F64_CANONICAL_NAN, flags);
    }
    
    (result.to_bits(), flags)
}
