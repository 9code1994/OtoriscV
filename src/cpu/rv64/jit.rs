//! RV64 basic block JIT (no codegen)
//!
//! Caches decoded instruction blocks and executes them sequentially.

use std::collections::HashMap;

use crate::memory::Bus;

use super::Cpu64;
use super::decode::*;
use super::execute_c::expand_compressed;
use super::trap::Trap64;

/// Result of executing a compiled block
pub enum BlockResult {
    /// Continue execution at the given PC
    Continue(u64),
    /// A trap occurred during execution
    Trap(Trap64),
    /// Need to fall back to interpreter (e.g., unsupported instruction)
    Interpret,
}

pub enum CachedInst64 {
    Compressed(u16),
    Full { raw: u32, decoded: DecodedInst },
}

/// A compiled basic block
pub struct CompiledBlock {
    /// Start physical address of the block
    pub start_paddr: u64,
    /// Number of instructions in the block
    pub inst_count: u32,
    /// Cached instructions
    pub instructions: Vec<CachedInst64>,
    /// Generation counter for invalidation
    pub generation: u32,
}

/// Block cache - stores compiled basic blocks
pub struct BlockCache {
    /// Map from physical address to compiled block
    blocks: HashMap<u64, CompiledBlock>,
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
    pub fn get(&mut self, paddr: u64) -> Option<&CompiledBlock> {
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
    pub fn get_block(&self, paddr: u64) -> Option<&CompiledBlock> {
        self.blocks.get(&paddr).filter(|b| b.generation == self.generation)
    }

    /// Compile a basic block starting at the given physical address
    pub fn compile_block(&mut self, bus: &mut impl Bus, start_paddr: u64) -> &CompiledBlock {
        const MAX_BLOCK_INSTS: usize = 64;

        let mut instructions = Vec::with_capacity(32);
        let mut paddr = start_paddr;

        loop {
            let addr = match map_paddr(paddr) {
                Some(addr) => addr,
                None => break,
            };

            let inst16 = bus.read16(addr) as u16;
            if (inst16 & 0b11) != 0b11 {
                let terminator = if let Some(expanded) = expand_compressed(inst16) {
                    let d = DecodedInst::decode(expanded);
                    is_block_terminator_decoded(&d)
                } else {
                    true
                };

                instructions.push(CachedInst64::Compressed(inst16));
                paddr = paddr.wrapping_add(2);

                if terminator || instructions.len() >= MAX_BLOCK_INSTS {
                    break;
                }
                continue;
            }

            let inst = bus.read32(addr);
            let decoded = DecodedInst::decode(inst);
            let terminator = is_block_terminator_decoded(&decoded);

            instructions.push(CachedInst64::Full { raw: inst, decoded });
            paddr = paddr.wrapping_add(4);

            if terminator || instructions.len() >= MAX_BLOCK_INSTS {
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

    /// Reset the cache
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.generation = 1;
        self.hits = 0;
        self.misses = 0;
        self.compiles = 0;
    }

    /// Get hit rate
    #[allow(dead_code)]
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Execute a compiled block
#[inline(always)]
pub fn execute_block(cpu: &mut Cpu64, block: &CompiledBlock, bus: &mut impl Bus) -> BlockResult {
    for inst in &block.instructions {
        let result = match inst {
            CachedInst64::Compressed(raw) => cpu.execute_compressed(*raw, bus),
            CachedInst64::Full { raw, decoded } => cpu.execute_decoded(*raw, decoded, bus),
        };

        if let Err(trap) = result {
            return BlockResult::Trap(trap);
        }
    }

    BlockResult::Continue(cpu.pc)
}

#[inline(always)]
fn map_paddr(paddr: u64) -> Option<u32> {
    if paddr > u32::MAX as u64 {
        None
    } else {
        Some(paddr as u32)
    }
}

#[inline(always)]
fn is_block_terminator_decoded(d: &DecodedInst) -> bool {
    if matches!(d.opcode, OP_BRANCH | OP_JAL | OP_JALR | OP_SYSTEM) {
        return true;
    }

    d.opcode == OP_MISC_MEM && d.funct3 == 0b001
}
