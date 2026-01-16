//! Interpreter-based codegen backend
//!
//! This is the fallback backend that wraps the existing interpreter.
//! Always available on all platforms.

use super::CodegenExit;
use crate::cpu::rv32::Cpu;
use crate::memory::Bus;

/// Interpreter codegen backend (fallback)
pub struct InterpCodegen;

impl InterpCodegen {
    pub fn new() -> Self {
        InterpCodegen
    }
}

impl Default for InterpCodegen {
    fn default() -> Self {
        Self::new()
    }
}
