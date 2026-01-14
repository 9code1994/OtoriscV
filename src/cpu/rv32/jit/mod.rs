//! Just-In-Time compilation for RISC-V execution
//!
//! This module contains both versions of the JIT compiler:
//! - v1: Simple basic block caching
//! - v2: Advanced JIT with CFG optimization

pub mod v1;
pub mod v2;

// Re-export v1 types for backward compatibility
pub use v1::{BlockCache, BlockResult, execute_block};

// Re-export v2 types
pub use v2::{
    Page, RegionResult, JitState, execute_region,
    HEAT_PER_BLOCK, JIT_THRESHOLD,
};
