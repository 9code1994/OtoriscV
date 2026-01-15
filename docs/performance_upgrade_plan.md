# Performance Upgrade Plan for RV32 Emulator

This document outlines performance optimization strategies based on analysis of the current codebase and reference implementations (v86, jor1k, rvemu).

## Current Performance Bottlenecks

### 1. No Software TLB Cache (CRITICAL)

**Location:** `src/cpu/rv32/mmu.rs`

**Problem:** Every memory access performs a full Sv32 page table walk:
- 2 memory reads for a 4KB page (level 1 + level 0 PTEs)
- 1 memory read for a megapage
- This means ~3 memory reads per actual memory access when paging is enabled

**Reference:** jor1k uses per-instruction-type TLB entries:
```javascript
// jor1k's fastcpu.js - separate TLB for each operation
var read32tlb_index  = -1;  // virtual page number
var read32tlb_entry  = -1;  // physical frame number
var store32tlb_index = -1;
var store32tlb_entry = -1;
```

---

### 2. No Instruction Fetch Caching

**Location:** `src/cpu/rv32/mod.rs` (Cpu::step)

**Problem:** Every instruction fetch:
1. Translates PC via full MMU (no icache TLB)
2. Reads 32 bits from bus
3. Decodes all immediate formats (even unused ones)

**Current code:**
```rust
let paddr = self.mmu.translate(self.pc, AccessType::Instruction, ...)?;
let inst = bus.read32(paddr);
let d = DecodedInst::decode(inst);  // Decodes ALL immediate types
```

---

### 3. Eager Immediate Decoding

**Location:** `src/cpu/rv32/decode.rs`

**Problem:** `DecodedInst::decode()` computes ALL immediate formats for every instruction:
- `imm_i`, `imm_s`, `imm_b`, `imm_u`, `imm_j` are all computed
- Most instructions only need 1 of these

**Impact:** 5 expensive bit manipulations per instruction, even for simple ALU ops.

---

### 4. Cascading Device Checks in Bus

**Location:** `src/system.rs` (SystemBus impl)

**Problem:** Every memory access checks all devices sequentially:
```rust
if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE { ... }
if addr >= UART_BASE && addr < UART_BASE + UART_SIZE { ... }
if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE { ... }
if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE { ... }
// finally, memory
```

**Impact:** 4+ comparisons for every RAM access (which is the common case).

---

### 5. No WFI Batching / Idle Skip

**Location:** `src/system.rs` (System::run)

**Problem:** WFI loop still increments cycles one at a time:
```rust
if self.cpu.wfi {
    // checks pending, but still cycles += 1 per loop
    cycles += 1;
    continue;
}
```

**Improvement:** Skip ahead to next timer interrupt instead of spinning.

---

## Proposed Optimizations

### Phase 1: Software TLB (High Impact, Medium Effort)

Add a simple software TLB to the MMU:

```rust
// src/cpu/rv32/mmu.rs
pub struct Mmu {
    // TLB: 16 entries, fully associative (or direct-mapped for simplicity)
    tlb: [TlbEntry; 16],
    tlb_generation: u32,  // Invalidate all on satp write
}

struct TlbEntry {
    vpn: u32,           // Virtual page number (vaddr >> 12)
    ppn: u32,           // Physical page number
    perm: u8,           // R/W/X/U permissions
    valid: bool,
    generation: u32,    // For lazy invalidation
}

impl Mmu {
    pub fn translate_fast(&mut self, vaddr: u32, access: AccessType, ...) -> Result<u32, u32> {
        let vpn = vaddr >> 12;
        let slot = (vpn as usize) & 0xF;  // Direct-mapped
        
        if self.tlb[slot].valid 
           && self.tlb[slot].vpn == vpn 
           && self.tlb[slot].generation == self.tlb_generation
           && self.check_perm_fast(self.tlb[slot].perm, access) 
        {
            // TLB hit!
            return Ok((self.tlb[slot].ppn << 12) | (vaddr & 0xFFF));
        }
        
        // TLB miss: full walk, then fill
        let paddr = self.translate_slow(vaddr, access, ...)?;
        self.fill_tlb(vpn, paddr >> 12, ...);
        Ok(paddr)
    }
    
    pub fn invalidate(&mut self) {
        self.tlb_generation = self.tlb_generation.wrapping_add(1);
    }
}
```

**Expected Speedup:** 2-5x for paged workloads (Linux kernel)

**Baseline (Before Phase 1):**
Benchmark results:
  Boot time: 29.802s
  Instructions: 72196385
  IPS: 2422543.062
  TLB hit rate: N/A (TLB cache not implemented)

---

### Phase 2: Lazy Immediate Decoding (Medium Impact, Low Effort)

Only decode the immediate format needed:

```rust
// src/cpu/rv32/decode.rs
impl DecodedInst {
    #[inline(always)]
    pub fn decode_minimal(inst: u32) -> Self {
        DecodedInst {
            opcode: inst & 0x7F,
            rd: (inst >> 7) & 0x1F,
            rs1: (inst >> 15) & 0x1F,
            rs2: (inst >> 20) & 0x1F,
            funct3: (inst >> 12) & 0x7,
            funct7: (inst >> 25) & 0x7F,
            // Leave immediates uninitialized or zero
            imm_i: 0, imm_s: 0, imm_b: 0, imm_u: 0, imm_j: 0,
            rs3: 0,
        }
    }
    
    #[inline(always)]
    pub fn imm_i(inst: u32) -> i32 {
        (inst as i32) >> 20
    }
    
    // ... other immediate decoders as standalone functions
}

// In execute.rs:
OP_LOAD => {
    let imm = DecodedInst::imm_i(inst);  // Only decode what we need
    ...
}
```

**Expected Speedup:** 10-20% for instruction decode

---

### Phase 3: Fast RAM Path (Medium Impact, Low Effort)

Optimize for the common case (RAM access):

```rust
// src/system.rs
impl<'a> Bus for SystemBus<'a> {
    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        // Fast path: RAM (most common)
        if addr >= DRAM_BASE {
            return self.memory.read32(addr);
        }
        self.read32_slow(addr)
    }
    
    #[cold]
    fn read32_slow(&mut self, addr: u32) -> u32 {
        // Device checks here...
    }
}
```

**Expected Speedup:** 5-15% for memory-bound workloads

---

### Phase 4: WFI Fast Forward (Medium Impact, Low Effort)

Skip cycles when waiting for timer:

```rust
// src/system.rs
if self.cpu.wfi {
    let pending = self.cpu.csr.mip & self.cpu.csr.mie;
    if pending != 0 {
        self.cpu.wfi = false;
    } else {
        // Fast-forward to next timer interrupt
        let cycles_to_timer = self.clint.cycles_until_interrupt();
        let skip = cycles_to_timer.min(max_cycles - cycles);
        self.clint.tick(skip);
        cycles += skip;
        continue;
    }
}
```

**Expected Speedup:** Massive for idle workloads, minimal for busy workloads

---

### Phase 5: Instruction Block Caching (High Impact, High Effort)

Cache decoded instructions for hot code paths:

```rust
struct ICache {
    // Map from physical page to decoded instructions
    pages: HashMap<u32, CachedPage>,
}

struct CachedPage {
    decoded: [Option<CachedInst>; 1024],  // 4KB / 4 bytes per inst
    valid: bool,
}

struct CachedInst {
    opcode: u8,
    rd: u8, rs1: u8, rs2: u8,
    funct3: u8, funct7: u8,
    imm: i32,  // Pre-decoded based on opcode
}
```

**Expected Speedup:** 30-50% for tight loops

---

### Phase 6: JIT Compilation to Host (Very High Impact, Very High Effort)

Like v86, translate RISC-V basic blocks to WASM/native:

1. **Interpreter with profiling**: Count executions per page
2. **Hot page detection**: When threshold reached, compile
3. **Basic block compilation**: Find block boundaries, generate WASM
4. **Execution**: Call compiled WASM for hot pages

This is a major undertaking (v86's JIT is ~90KB of Rust) but provides ~5-10x speedup.

---

## Implementation Priority

| Phase | Optimization | Impact | Effort | Priority |
|-------|--------------|--------|--------|----------|
| 1 | Software TLB | High | Medium | **P0** |
| 3 | Fast RAM Path | Medium | Low | **P0** |
| 4 | WFI Fast Forward | High (idle) | Low | **P1** |
| 2 | Lazy Immediate Decode | Medium | Low | **P1** |
| 5 | Instruction Cache | High | High | **P2** |
| 6 | JIT Compilation | Very High | Very High | **P3** |

---

## Benchmarking

Before and after each optimization, measure:

1. **Instructions per second (IPS)**: `instruction_count / wall_time`
2. **Boot time**: Time to Linux shell prompt
3. **TLB hit rate** (Phase 1): `tlb_hits / (tlb_hits + tlb_misses)`

Benchmark command:
```bash
# Native
time cargo run --release -- \
    images/Image-minimal \
    --initrd images/rootfs_tcc.cpio \
    --ram 64 \
    --benchmark
 
# WASM (in browser, measure via console.time)
```

---

## References

- **v86 how-it-works**: Details on TLB, JIT, and lazy flags
- **jor1k fastcpu.js**: asm.js optimizations and per-instruction TLB
- **rvemu cpu.rs**: Clean Rust interpreter structure

---

## Appendix: Current Code Structure

```
src/cpu/rv32/
├── mod.rs       # CPU state, step() function
├── decode.rs    # Instruction decoder (eager)
├── execute.rs   # Instruction execution (switch on opcode)
├── execute_fp.rs # F/D extension execution
├── mmu.rs       # Sv32 translation (no TLB!)
└── csr.rs       # CSR handling

src/system.rs    # Bus routing (cascading if-else)
src/memory/mod.rs # RAM access
```
