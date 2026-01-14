//! Basic Block JIT Compilation (v1)
//!
//! Compiles sequences of instructions into blocks that execute together,
//! reducing the per-instruction overhead of interpretation.

use std::collections::HashMap;
use crate::cpu::Cpu;
use super::super::icache::CachedInst;
use super::super::decode::*;
use crate::memory::Bus;
use crate::cpu::trap::Trap;

/// Result of executing a compiled block
pub enum BlockResult {
    /// Continue execution at the given PC
    Continue(u32),
    /// A trap occurred during execution
    Trap(Trap),
    /// Need to fall back to interpreter (e.g., for complex instructions)
    Interpret,
}

/// A compiled basic block
pub struct CompiledBlock {
    /// Start physical address of the block
    pub start_paddr: u32,
    /// Number of instructions in the block
    pub inst_count: u32,
    /// The cached decoded instructions
    pub instructions: Vec<CachedInst>,
    /// Generation counter for invalidation
    pub generation: u32,
}

/// Block cache - stores compiled basic blocks
pub struct BlockCache {
    /// Map from physical address to compiled block
    blocks: HashMap<u32, CompiledBlock>,
    /// Generation counter for bulk invalidation
    generation: u32,
    /// Statistics
    pub hits: u64,
    pub misses: u64,
    pub compiles: u64,
}

impl Default for BlockCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockCache {
    pub fn new() -> Self {
        BlockCache {
            blocks: HashMap::with_capacity(4096),
            generation: 1,
            hits: 0,
            misses: 0,
            compiles: 0,
        }
    }

    /// Look up a block by physical address, returns None if not found or stale
    #[inline(always)]
    pub fn get(&mut self, paddr: u32) -> Option<&CompiledBlock> {
        if let Some(block) = self.blocks.get(&paddr) {
            if block.generation == self.generation {
                self.hits += 1;
                return Some(block);
            }
        }
        self.misses += 1;
        None
    }

    /// Get a block without updating stats (for re-borrowing after compile)
    #[inline(always)]
    pub fn get_block(&self, paddr: u32) -> Option<&CompiledBlock> {
        self.blocks.get(&paddr).filter(|b| b.generation == self.generation)
    }

    /// Compile a basic block starting at the given physical address
    pub fn compile_block(&mut self, bus: &mut impl Bus, start_paddr: u32) -> &CompiledBlock {
        let mut instructions = Vec::with_capacity(32);
        let mut paddr = start_paddr;

        // Scan instructions until we hit a block terminator
        loop {
            let inst = bus.read32(paddr);
            let cached = CachedInst::decode(inst);
            instructions.push(cached);

            // Check if this is a block-ending instruction
            if is_block_terminator(cached.opcode) {
                break;
            }

            paddr += 4;

            // Limit block size to avoid huge blocks
            if instructions.len() >= 64 {
                break;
            }
        }

        let block = CompiledBlock {
            start_paddr,
            inst_count: instructions.len() as u32,
            instructions,
            generation: self.generation,
        };

        self.blocks.insert(start_paddr, block);
        self.compiles += 1;

        self.blocks.get(&start_paddr).unwrap()
    }

    /// Invalidate all blocks (e.g., on FENCE.I or SFENCE.VMA)
    pub fn invalidate_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Invalidate blocks in a specific page
    #[allow(dead_code)]
    pub fn invalidate_page(&mut self, page_addr: u32) {
        let page_base = page_addr & !0xFFF;
        self.blocks.retain(|addr, block| {
            // Keep block if it's from a different generation OR different page
            block.generation != self.generation || (*addr & !0xFFF) != page_base
        });
    }

    /// Reset the cache
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.generation = 1;
        self.hits = 0;
        self.misses = 0;
        self.compiles = 0;
    }

    /// Get hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Check if an opcode terminates a basic block
#[inline(always)]
fn is_block_terminator(opcode: u8) -> bool {
    matches!(
        opcode as u32,
        OP_BRANCH |  // BEQ, BNE, BLT, BGE, BLTU, BGEU
        OP_JAL |     // JAL
        OP_JALR |    // JALR  
        OP_SYSTEM    // ECALL, EBREAK, WFI, MRET, SRET, CSR ops
    )
}

/// Execute a compiled block
/// 
/// Returns the result indicating next PC, trap, or need for interpreter fallback
#[inline(always)]
pub fn execute_block(cpu: &mut Cpu, block: &CompiledBlock, bus: &mut impl Bus) -> BlockResult {
    let inst_count = block.instructions.len();
    
    for (i, cached) in block.instructions.iter().enumerate() {
        let is_last = i == inst_count - 1;
        let inst = cached.raw;

        // Execute the instruction
        match cpu.execute_cached(inst, cached, bus) {
            Ok(()) => {
                // For non-terminal instructions, PC was already advanced in execute_cached
                // We just continue to the next instruction
                if !is_last {
                    continue;
                }
            }
            Err(trap) => {
                return BlockResult::Trap(trap);
            }
        }
    }

    BlockResult::Continue(cpu.pc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_block_terminator() {
        // Branch
        assert!(is_block_terminator(OP_BRANCH as u8));
        // JAL
        assert!(is_block_terminator(OP_JAL as u8));
        // JALR
        assert!(is_block_terminator(OP_JALR as u8));
        // SYSTEM
        assert!(is_block_terminator(OP_SYSTEM as u8));
        // Non-terminators
        assert!(!is_block_terminator(OP_LUI as u8));
        assert!(!is_block_terminator(OP_AUIPC as u8));
        assert!(!is_block_terminator(OP_LOAD as u8));
        assert!(!is_block_terminator(OP_STORE as u8));
        assert!(!is_block_terminator(OP_OP_IMM as u8));
        assert!(!is_block_terminator(OP_OP as u8));
    }
}
