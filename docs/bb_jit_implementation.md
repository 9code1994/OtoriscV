# Basic Block JIT Implementation Plan

## Overview

This document describes the implementation of **Basic Block Compilation** for the RV32 emulator. Instead of interpreting one instruction at a time, we compile sequences of instructions into closures that execute directly.

## Current Performance

| Optimization | Boot Time | IPS |
|--------------|-----------|-----|
| Baseline | 38.4s | 1.88M |
| + Instruction Cache | 24.8s | 2.9M |
| **Target (with BB-JIT)** | **~15-18s** | **~4-5M** |

## Design

### What is a Basic Block?

A **basic block** is a sequence of instructions with:
- Single entry point (first instruction)
- Single exit point (last instruction)
- No branches/jumps in the middle

```
Basic Block Example:
  lui   a0, 0x80000        ; Entry
  addi  a0, a0, 0x100      ; Sequential
  lw    a1, 0(a0)          ; Sequential  
  beq   a1, zero, label    ; Exit (branch)
```

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         CPU Step                             │
├─────────────────────────────────────────────────────────────┤
│  1. Translate PC → Physical Address                         │
│  2. Lookup block cache by physical address                  │
│     ├─ HIT:  Execute compiled closure                       │
│     └─ MISS: Compile block, cache, then execute             │
│  3. Handle result (next PC, trap, etc.)                     │
└─────────────────────────────────────────────────────────────┘
```

## Implementation

### File: `src/cpu/rv32/bb_jit.rs`

#### 1. Block Result Enum

```rust
pub enum BlockResult {
    /// Continue to next PC
    Continue(u32),
    /// A trap occurred
    Trap(crate::cpu::trap::Trap),
    /// Need interpreter fallback (e.g., unsupported instruction)
    Interpret,
}
```

#### 2. Compiled Block Structure

```rust
pub struct CompiledBlock {
    /// Start physical address
    pub start_paddr: u32,
    /// Number of instructions in block
    pub inst_count: u32,
    /// The compiled function
    pub execute: fn(&mut Cpu, &[u32], &mut dyn Bus) -> BlockResult,
    /// Raw instructions (for immediate extraction)
    pub instructions: Vec<u32>,
    /// Generation for invalidation
    pub generation: u32,
}
```

#### 3. Block Cache

```rust
use std::collections::HashMap;

pub struct BlockCache {
    /// Map from physical address to compiled block
    blocks: HashMap<u32, CompiledBlock>,
    /// Generation for bulk invalidation
    generation: u32,
    /// Stats
    pub hits: u64,
    pub misses: u64,
    pub compiles: u64,
}

impl BlockCache {
    pub fn new() -> Self { ... }
    
    pub fn get(&mut self, paddr: u32) -> Option<&CompiledBlock> { ... }
    
    pub fn insert(&mut self, block: CompiledBlock) { ... }
    
    pub fn invalidate_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}
```

#### 4. Block Compiler

The key insight: instead of generating machine code, we generate a **static dispatch function** that executes a sequence of pre-decoded instructions:

```rust
impl BlockCache {
    pub fn compile_block(&mut self, cpu: &Cpu, bus: &mut impl Bus, start_paddr: u32) -> &CompiledBlock {
        // 1. Scan instructions until we hit a block terminator
        let mut instructions = Vec::new();
        let mut paddr = start_paddr;
        
        loop {
            let inst = bus.read32(paddr);
            instructions.push(inst);
            
            let opcode = inst & 0x7F;
            
            // Check if this is a block-ending instruction
            if is_block_terminator(opcode) {
                break;
            }
            
            paddr += 4;
            
            // Limit block size
            if instructions.len() >= 64 {
                break;
            }
        }
        
        // 2. Create compiled block
        let block = CompiledBlock {
            start_paddr,
            inst_count: instructions.len() as u32,
            execute: execute_block,  // Static function
            instructions,
            generation: self.generation,
        };
        
        self.blocks.insert(start_paddr, block);
        self.compiles += 1;
        
        self.blocks.get(&start_paddr).unwrap()
    }
}

fn is_block_terminator(opcode: u32) -> bool {
    matches!(opcode, 
        0x63 |  // BRANCH
        0x6F |  // JAL
        0x67 |  // JALR
        0x73    // SYSTEM (includes ECALL, WFI, etc.)
    )
}
```

#### 5. Block Executor

The executor runs all instructions in the block sequentially:

```rust
fn execute_block(cpu: &mut Cpu, instructions: &[u32], bus: &mut dyn Bus) -> BlockResult {
    for (i, &inst) in instructions.iter().enumerate() {
        let is_last = i == instructions.len() - 1;
        
        // Use the existing execute_cached for each instruction
        let cached = CachedInst::decode(inst);
        
        match cpu.execute_cached(inst, &cached, bus) {
            Ok(()) => {
                if !is_last {
                    // For non-terminal instructions, PC was already advanced
                    // We just continue
                }
            }
            Err(trap) => return BlockResult::Trap(trap),
        }
    }
    
    BlockResult::Continue(cpu.pc)
}
```

### Integration with System

Modify `System::run()` to use block cache:

```rust
// In System struct
pub struct System {
    // ... existing fields ...
    block_cache: BlockCache,
}

// In System::run()
pub fn run(&mut self, max_cycles: u32) -> u32 {
    while cycles < max_cycles {
        // ... timer and interrupt handling ...
        
        if self.cpu.wfi {
            // ... WFI handling ...
            continue;
        }
        
        // Translate PC
        let paddr = match self.cpu.mmu.translate(...) {
            Ok(pa) => pa,
            Err(cause) => { /* handle */ }
        };
        
        // Try block cache
        if let Some(block) = self.block_cache.get(paddr) {
            self.block_cache.hits += 1;
            
            match (block.execute)(&mut self.cpu, &block.instructions, &mut self.bus) {
                BlockResult::Continue(next_pc) => {
                    cycles += block.inst_count;
                }
                BlockResult::Trap(trap) => {
                    self.cpu.handle_trap(trap);
                    cycles += 1;
                }
                BlockResult::Interpret => {
                    // Fallback to single-step
                    self.step_with_devices();
                    cycles += 1;
                }
            }
        } else {
            // Compile new block
            self.block_cache.compile_block(...);
        }
    }
}
```

### Invalidation

Blocks must be invalidated when:
1. **FENCE.I** instruction is executed
2. **SFENCE.VMA** changes address translation
3. **Write to executable memory** (self-modifying code)

```rust
// On FENCE.I or SFENCE.VMA
self.block_cache.invalidate_all();

// On write to potential code region (optional optimization)
if addr >= DRAM_BASE && addr < DRAM_BASE + kernel_size {
    self.block_cache.invalidate_page(addr >> 12);
}
```

## Implementation Order

1. [ ] Create `bb_jit.rs` with `BlockResult`, `CompiledBlock`, `BlockCache`
2. [ ] Implement `compile_block` to scan and collect instructions
3. [ ] Implement `execute_block` to run instruction sequence
4. [ ] Add `BlockCache` to `System`
5. [ ] Modify `System::run` to use block cache
6. [ ] Add invalidation on FENCE.I / SFENCE.VMA
7. [ ] Benchmark and tune block size limit

## Expected Results

- **30-50% speedup** over instruction cache alone
- **4-5M IPS** target
- **~15-18s** boot time

## Risks

1. **Incorrect invalidation** - May execute stale code
2. **Memory overhead** - Storing compiled blocks
3. **Compilation overhead** - May slow down cold paths

Mitigation: Start with conservative invalidation (invalidate all on any FENCE), optimize later.
