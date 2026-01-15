# JIT V2 Debugging Journey

## Initial Problem: VA/PA Mismatch

### Symptoms
JIT v2 initially failed to boot Linux, causing illegal instruction traps and infinite loops. The `--jit-v2` flag would result in immediate crashes or hangs.

### Root Cause
JIT v2 was designed to use **virtual addresses (VA)** throughout, with the expectation that `SystemBus` would handle VA→PA translation. However, `SystemBus` operates directly on **physical addresses (PA)** without any translation.

**The mismatch:**
- JIT v2 passed VA to `bus.read32(vaddr)`
- `SystemBus.read32()` treated addresses as PA and accessed physical memory directly
- When MMU paging was enabled, this caused incorrect instruction fetches

### Working Implementation (JIT v1)
JIT v1 worked correctly by:
1. Calling `mmu.translate(pc, ...)` to convert VA → PA
2. Using PA as cache keys in `BlockCache`
3. Passing PA to `bus.read32(paddr)` for instruction fetch

---

## Fix Attempt 1: PA-Based Caching

### Approach
Modified JIT v2 to use physical addresses like v1:
- Added `mmu.translate()` call in `step_block_v2()` 
- Changed cache keys from VA to PA
- Updated `execute_region()` to accept `start_paddr` parameter

### Changes Made
- [system.rs](file:///home/mhz/Projects/riscv-emu-rs/src/system.rs): `step_block_v2()` now translates VA→PA
- [execute.rs](file:///home/mhz/Projects/riscv-emu-rs/src/cpu/rv32/jit/v2/execute.rs): `execute_region()` signature updated
- [discovery.rs](file:///home/mhz/Projects/riscv-emu-rs/src/cpu/rv32/jit/v2/discovery.rs): Documentation updated to reflect PA usage

### Result
✅ Basic PA translation fix worked - JIT v2 could execute blocks correctly when falling back to single-block execution (like v1).

---

## Fix Attempt 2: CFG Multi-Block Execution

### Goal
Implement full CFG-based multi-block execution within a page for better performance.

### Theory
Within a 4KB page, VA and PA share the same offset (lower 12 bits):
- `VA = 0xC000_1234` maps to `PA = 0x0000_1234`
- Page offset `0x234` is identical
- Therefore, intra-page branch targets can use PA-based computation

### Implementation Attempts

#### Attempt A: Store branch targets during discovery
```rust
let next_paddr = match &block.ty {
    BasicBlockType::Branch { taken, not_taken, condition } => {
        let take_branch = condition.evaluate(&cpu.regs);
        if take_branch { *taken } else { *not_taken }
    }
    // ...
};
```

**Problem:** Branch conditions were evaluated **after** block execution, using post-execution register values. This was fundamentally wrong because:
1. `execute_cached()` already handles branches correctly
2. Re-evaluating conditions with modified registers caused incorrect control flow

#### Attempt B: Use cpu.pc to compute next PA
```rust
let next_va = cpu.pc;  // Set by execute_cached
let next_page_offset = next_va & 0xFFF;
let next_paddr = page_base_pa | next_page_offset;
```

**Problem:** Still caused hangs at "Zone ranges:" during Linux boot. Unclear why - possibly:
- Incorrect PA computation edge cases
- Block ordering issues
- Interaction with MMU page table changes
- Edge case in page boundary handling

### Debug Attempts
1. ✅ Simplified to single-block execution - still hung
2. ✅ Added extensive logging - showed execution reaching specific point then hanging
3. ❌ Could not isolate exact instruction or block causing hang
4. ❌ Multi-block loop logic appeared correct but still failed

### Current Status
**Reverted to v1 fallback** in `step_block_v2()`:
```rust
fn step_block_v2(&mut self) -> Result<u32, Trap> {
    // CFG execution causes hangs - use v1 for correctness
    self.step_block_v1()
}
```

---

## Current State

### What Works ✅
- JIT v2 flag (`--jit-v2`) boots Linux successfully
- Uses v1's proven single-block execution internally
- Hotness tracking and compilation still happen (but unused)
- All PA-based caching infrastructure in place

### What Doesn't Work ❌
- CFG-based multi-block execution causes hangs
- No performance benefit over v1 (same backend)
- `execute_region()` function exists but is disabled

---

## Future Work

### To Fix CFG Multi-Block Execution

#### Option 1: Deeper Debugging
- Add per-instruction logging in execute_region
- Compare block execution order with v1
- Identify exact hang point and instruction

#### Option 2: Hybrid Approach
- Use VA internally for block discovery
- Translate to PA only for bus reads
- Store both VA and PA in BasicBlock
- Use VA for control flow, PA for cache lookup

#### Option 3: Virtual Address Execution
- Make `SystemBus` handle VA→PA translation
- Add MMU context to Bus trait
- Let JIT v2 work with VA throughout (original design intent)

### Performance Optimizations (Once CFG Works)
1. **Page-level compilation** - compile all hot blocks in a page together
2. **Multi-block regions** - eliminate overhead of returning to caller between blocks
3. **Better CFG analysis** - identify loops for optimization
4. **Inline cache** - for frequently taken paths

---

## Lessons Learned

1. **VA/PA must be handled carefully** - never assume bus handles translation
2. **Don't re-evaluate branch conditions** - execution already handled them
3. **Simple solutions work** - v1's approach is proven and correct
4. **Debug incrementally** - isolate each layer before adding complexity
5. **Document assumptions** - original v2 docs assumed bus did translation

## References

- [JIT v1 Implementation](file:///home/mhz/Projects/riscv-emu-rs/docs/bb_jit_implementation.md)
- [JIT v2 Design](file:///home/mhz/Projects/riscv-emu-rs/docs/bb_jit_v2_implementation.md)
- [Implementation Plan](file:///home/mhz/.gemini/antigravity/brain/582045f2-947f-453f-8d77-22ed5531f472/implementation_plan.md)
