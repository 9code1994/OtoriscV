//! RV64IMAFD CPU module (placeholder)
//!
//! Will implement the RISC-V 64-bit base integer instruction set
//! with M (multiply/divide), A (atomic), F (single-precision float),
//! and D (double-precision float) extensions.

// TODO: Implement RV64GC in Phase 2

/// Placeholder for Cpu64 - not yet implemented
pub struct Cpu64 {
    _placeholder: (),
}

impl Cpu64 {
    pub fn new() -> Self {
        unimplemented!("RV64 CPU not yet implemented")
    }
}

impl Default for Cpu64 {
    fn default() -> Self {
        Self::new()
    }
}
