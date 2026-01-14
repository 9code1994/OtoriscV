//! Core types for JIT v2

use super::super::super::icache::CachedInst;
use crate::cpu::trap::Trap;

// =============================================================================
// Core Types
// =============================================================================

/// Virtual page identifier (upper 20 bits of virtual address)
/// 
/// JIT v2 works with virtual addresses. Blocks are keyed by VA, and
/// the cache is invalidated on SFENCE.VMA or privilege level changes.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Page(u32);

impl Page {
    #[inline(always)]
    pub fn of(vaddr: u32) -> Self {
        Page(vaddr >> 12)
    }

    #[inline(always)]
    pub fn base_addr(self) -> u32 {
        self.0 << 12
    }

    #[inline(always)]
    pub fn contains(self, vaddr: u32) -> bool {
        Page::of(vaddr) == self
    }
}

/// Result of executing a compiled region
#[derive(Debug)]
pub enum RegionResult {
    /// Continue execution at the given PC
    Continue(u32),
    /// A trap occurred
    Trap(Trap),
    /// Exit region (indirect jump, system call, etc.)
    Exit(u32),
}

// =============================================================================
// Basic Block Types
// =============================================================================

/// Branch condition extracted from instruction
#[derive(Clone, Debug)]
pub enum BranchCondition {
    /// BEQ rs1, rs2
    Eq { rs1: u8, rs2: u8 },
    /// BNE rs1, rs2
    Ne { rs1: u8, rs2: u8 },
    /// BLT rs1, rs2 (signed)
    Lt { rs1: u8, rs2: u8 },
    /// BGE rs1, rs2 (signed)
    Ge { rs1: u8, rs2: u8 },
    /// BLTU rs1, rs2 (unsigned)
    Ltu { rs1: u8, rs2: u8 },
    /// BGEU rs1, rs2 (unsigned)
    Geu { rs1: u8, rs2: u8 },
}

impl BranchCondition {
    /// Evaluate the condition against CPU register values
    #[inline(always)]
    pub fn evaluate(&self, regs: &[u32; 32]) -> bool {
        match self {
            Self::Eq { rs1, rs2 } => regs[*rs1 as usize] == regs[*rs2 as usize],
            Self::Ne { rs1, rs2 } => regs[*rs1 as usize] != regs[*rs2 as usize],
            Self::Lt { rs1, rs2 } => {
                (regs[*rs1 as usize] as i32) < (regs[*rs2 as usize] as i32)
            }
            Self::Ge { rs1, rs2 } => {
                (regs[*rs1 as usize] as i32) >= (regs[*rs2 as usize] as i32)
            }
            Self::Ltu { rs1, rs2 } => regs[*rs1 as usize] < regs[*rs2 as usize],
            Self::Geu { rs1, rs2 } => regs[*rs1 as usize] >= regs[*rs2 as usize],
        }
    }

    /// Extract branch condition from BRANCH instruction
    pub fn from_instruction(inst: u32) -> Option<Self> {
        let funct3 = ((inst >> 12) & 0x7) as u8;
        let rs1 = ((inst >> 15) & 0x1F) as u8;
        let rs2 = ((inst >> 20) & 0x1F) as u8;

        match funct3 {
            0b000 => Some(Self::Eq { rs1, rs2 }),  // BEQ
            0b001 => Some(Self::Ne { rs1, rs2 }),  // BNE
            0b100 => Some(Self::Lt { rs1, rs2 }),  // BLT
            0b101 => Some(Self::Ge { rs1, rs2 }),  // BGE
            0b110 => Some(Self::Ltu { rs1, rs2 }), // BLTU
            0b111 => Some(Self::Geu { rs1, rs2 }), // BGEU
            _ => None,
        }
    }
}

/// Basic block terminator type
#[derive(Clone, Debug)]
pub enum BasicBlockType {
    /// Fallthrough to next instruction (non-control-flow terminator or block size limit)
    Fallthrough {
        /// Physical address of next block (if in same page)
        next: Option<u32>,
    },
    /// Unconditional jump (JAL with rd=x0)
    Jump {
        /// Target physical address (if static and in same page)
        target: Option<u32>,
        /// Jump offset for recalculation
        offset: i32,
    },
    /// Conditional branch
    Branch {
        /// Target if branch taken
        taken: Option<u32>,
        /// Target if branch not taken (next instruction)
        not_taken: Option<u32>,
        /// Branch condition
        condition: BranchCondition,
        /// Branch offset
        offset: i32,
    },
    /// Indirect jump (JALR, computed gotos, returns)
    IndirectJump,
    /// System instruction (ECALL, EBREAK, CSR, etc.) - always exits
    System,
}

/// A basic block within a compiled region
#[derive(Clone)]
pub struct BasicBlock {
    /// Physical start address
    pub addr: u32,
    /// Physical end address (exclusive - address after last instruction)
    pub end_addr: u32,
    /// Decoded instructions
    pub instructions: Vec<CachedInst>,
    /// Terminator type with control flow edges
    pub ty: BasicBlockType,
    /// Is this an entry point from outside the region?
    pub is_entry_point: bool,
}

impl BasicBlock {
    /// Get successor addresses
    pub fn successors(&self) -> Vec<u32> {
        match &self.ty {
            BasicBlockType::Fallthrough { next: Some(addr) } => vec![*addr],
            BasicBlockType::Jump { target: Some(addr), .. } => vec![*addr],
            BasicBlockType::Branch {
                taken,
                not_taken,
                ..
            } => {
                let mut succs = Vec::with_capacity(2);
                if let Some(addr) = taken {
                    succs.push(*addr);
                }
                if let Some(addr) = not_taken {
                    succs.push(*addr);
                }
                succs
            }
            _ => vec![],
        }
    }
}

/// Structured control flow for code generation
#[derive(Clone, Debug)]
pub enum ControlFlowStructure {
    /// Single basic block
    Block(u32),
    /// Entry point dispatcher (br_table equivalent)
    Dispatcher(Vec<u32>),
    /// Loop construct (for back-edges)
    Loop(Vec<ControlFlowStructure>),
    /// Block construct (for forward jumps)
    Forward(Vec<ControlFlowStructure>),
}

impl ControlFlowStructure {
    /// Get the "head" addresses of this structure
    pub fn head(&self) -> Vec<u32> {
        match self {
            Self::Block(addr) => vec![*addr],
            Self::Dispatcher(entries) => entries.clone(),
            Self::Loop(children) | Self::Forward(children) => {
                children.first().map(|c| c.head()).unwrap_or_default()
            }
        }
    }

    /// Get all block addresses in this structure
    pub fn all_blocks(&self) -> Vec<u32> {
        match self {
            Self::Block(addr) => vec![*addr],
            Self::Dispatcher(_) => vec![],
            Self::Loop(children) | Self::Forward(children) => {
                children.iter().flat_map(|c| c.all_blocks()).collect()
            }
        }
    }
}

/// Compiled region for a page
pub struct CompiledRegion {
    /// Basic blocks in this region
    pub blocks: std::collections::HashMap<u32, BasicBlock>,
    /// Structured control flow
    pub structure: Vec<ControlFlowStructure>,
    /// Entry points
    pub entry_points: Vec<u32>,
    /// Generation counter for invalidation
    pub generation: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_operations() {
        let page = Page::of(0x8000_1234);
        assert_eq!(page.base_addr(), 0x8000_1000);
        assert!(page.contains(0x8000_1000));
        assert!(page.contains(0x8000_1FFF));
        assert!(!page.contains(0x8000_2000));
    }

    #[test]
    fn test_branch_condition() {
        let regs: [u32; 32] = {
            let mut r = [0u32; 32];
            r[1] = 10;
            r[2] = 20;
            r[3] = 10;
            r
        };

        let eq = BranchCondition::Eq { rs1: 1, rs2: 3 };
        assert!(eq.evaluate(&regs)); // 10 == 10

        let ne = BranchCondition::Ne { rs1: 1, rs2: 2 };
        assert!(ne.evaluate(&regs)); // 10 != 20

        let lt = BranchCondition::Lt { rs1: 1, rs2: 2 };
        assert!(lt.evaluate(&regs)); // 10 < 20
    }
}
