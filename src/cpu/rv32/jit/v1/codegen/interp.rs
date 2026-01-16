//! Interpreter "codegen" - just a passthrough to the interpreter
//!
//! This is the default fallback when no native codegen is available.

/// Interpreter-based codegen (fallback)
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
