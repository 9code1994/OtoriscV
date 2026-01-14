//! RISC-V CPU modules
//!
//! Supports both RV32GC and RV64GC architectures.
//! Currently implements RV32IMAFD with planned RV32C extension.

// Shared modules
pub mod fpu;
pub mod trap;

// Architecture-specific modules
pub mod rv32;
pub mod rv64;

// Re-export commonly used types from shared modules
pub use fpu::Fpu;
pub use trap::Trap;

// Re-export RV32 as the default (for backwards compatibility)
pub use rv32::Cpu;
pub use rv32::csr::{self, Csr};
pub use rv32::mmu::{self, Mmu};

/// Privilege levels (shared across architectures)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum PrivilegeLevel {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

impl From<u8> for PrivilegeLevel {
    fn from(val: u8) -> Self {
        match val & 3 {
            0 => PrivilegeLevel::User,
            1 => PrivilegeLevel::Supervisor,
            3 => PrivilegeLevel::Machine,
            _ => PrivilegeLevel::Machine,
        }
    }
}
