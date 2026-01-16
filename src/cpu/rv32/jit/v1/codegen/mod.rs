//! Unified code generation backends for JIT v1
//!
//! This module provides code generation backends for basic block JIT:
//! - Interpreter (default, always available)
//! - WASM (wasm32 target)
//! - dynasm-rs (native, feature-gated)

use crate::cpu::trap::Trap;

pub mod interp;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(target_arch = "wasm32")]
pub mod emit;

#[cfg(target_arch = "wasm32")]
pub mod runtime;

#[cfg(all(not(target_arch = "wasm32"), feature = "jit-dynasm"))]
pub mod dynasm;

/// Result of executing compiled code
#[derive(Debug)]
pub enum CodegenExit {
    /// Continue execution at the given VA
    Continue(u32),
    /// A trap occurred
    Trap(Trap),
    /// Fall back to interpreter
    Fallback,
}

/// Error during code generation
#[derive(Debug)]
pub enum CodegenError {
    /// Block too complex to compile
    TooComplex,
    /// Unsupported instruction
    UnsupportedInstruction(u32),
    /// Backend-specific error
    BackendError(String),
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodegenError::TooComplex => write!(f, "Block too complex to compile"),
            CodegenError::UnsupportedInstruction(inst) => {
                write!(f, "Unsupported instruction: 0x{:08x}", inst)
            }
            CodegenError::BackendError(msg) => write!(f, "Backend error: {}", msg),
        }
    }
}

impl std::error::Error for CodegenError {}

// Re-export the default backend based on target
#[cfg(target_arch = "wasm32")]
pub use wasm::WasmCodegen as DefaultCodegen;

#[cfg(not(target_arch = "wasm32"))]
pub use interp::InterpCodegen as DefaultCodegen;
