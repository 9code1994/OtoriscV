//! JIT v2: Advanced Basic Block JIT with CFG Optimization
//!
//! Inspired by v86's JIT architecture:
//! - Hotness-based compilation
//! - Control flow graph analysis with SCC detection
//! - Structured control flow for efficient code generation
//! - Multi-entry point dispatching

pub mod types;
pub mod cfg;
pub mod discovery;
pub mod state;
pub mod execute;
pub mod codegen;

// Re-export commonly used types
pub use types::{
    Page, RegionResult, BranchCondition, BasicBlock, BasicBlockType,
    ControlFlowStructure, CompiledRegion,
};
pub use state::{JitState, HEAT_PER_BLOCK, JIT_THRESHOLD};
pub use execute::{execute_region, execute_region_linear};
pub use codegen::{CodegenExit, CodegenError, DefaultCodegen};

#[cfg(target_arch = "wasm32")]
pub use codegen::wasm::WasmCodegen;

