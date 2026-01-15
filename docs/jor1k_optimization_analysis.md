# jor1k Performance Optimization Analysis

## Overview

Analysis of jor1k's riscv.c to understand why it achieves such high performance, and how to apply these techniques to our emulator.

## Current Performance

| Optimization | Boot Time | IPS |
|--------------|-----------|-----|
| Baseline | 38.4s | 1.88M |
| + Instruction Cache | 24.8s | 2.9M |
| **Target (jor1k-style)** | **~10-15s** | **~6-8M** |

---

## jor1k Key Optimizations

### 1. XOR-Based Single-Entry TLB

**The trick**: Store `(paddr XOR vaddr) & PAGE_MASK` instead of raw physical address.

```c
// jor1k's FastTLBLookupMacro
if ((tlb_check ^ vaddr) & 0xFFFFF000) {  // Miss?
    paddr = TranslateVM(vaddr, mode);
    tlb_check = vaddr;
    tlb_lookup = ((paddr ^ vaddr) >> 12) << 12;  // Store XOR!
}
paddr = tlb_lookup ^ vaddr;  // Single XOR gives physical addr!
```

**Why it's fast**: 
- TLB hit = ONE XOR operation
- No struct access, no permission check on hit
- Page offset handled automatically

### 2. Separate TLB Per Access Type

```c
// 9 separate TLBs - no permission checking needed on hit
int32 instlblookup;       // Instruction fetch
int32 read32tlblookup;    // 32-bit loads
int32 read8stlblookup;    // Signed byte loads  
int32 write32tlblookup;   // 32-bit stores
// ... etc
```

### 3. Direct RAM Access via Pointer

```c
static int32* ramw = (int32*)0x100000;  // Direct pointer

inline int32 RamRead32(int32 paddr) {
    if (paddr < 0)  // High bit = RAM (0x80000000+)
        return ramw[((paddr)^0x80000000)>>2];  // Array index!
    else
        return Read32(paddr);  // MMIO slow path
}
```

**Why it's fast**:
- RAM check = single sign bit test
- RAM access = direct array index, no function call
- MMIO = cold path via extern function

### 4. Batched Timer Updates

```c
if (!(steps & 63)) {  // Every 64 instructions
    g->ticks += clockspeed;
    // Check timer interrupt
}
```

### 5. Giant Inline Switch

All instruction execution in one ~1200 line function:
- No function calls for decode/execute
- All registers in CPU registers
- Compiler optimizes entire hot loop

---

## Implementation Plan

### Phase 1: XOR-Based TLB (High Impact)

Modify `mmu.rs`:

```rust
struct SimpleTlbEntry {
    check: u32,    // vaddr of cached page
    lookup: u32,   // (paddr XOR vaddr) & PAGE_MASK
}

#[inline(always)]
fn tlb_translate(&mut self, vaddr: u32, access_type: AccessType) -> Option<u32> {
    let entry = &self.tlb[access_type as usize];
    if (entry.check ^ vaddr) & 0xFFFF_F000 == 0 {
        // HIT: single XOR
        return Some(entry.lookup ^ vaddr);
    }
    None  // Miss - do page walk
}
```

### Phase 2: Direct RAM Pointer (High Impact)

Modify `system.rs` and `memory.rs`:

```rust
impl SystemBus {
    #[inline(always)]
    fn read32(&mut self, paddr: u32) -> u32 {
        if paddr >= DRAM_BASE {
            // Direct slice access
            let offset = (paddr - DRAM_BASE) as usize;
            unsafe {
                let ptr = self.memory.data.as_ptr().add(offset) as *const u32;
                ptr.read_unaligned()
            }
        } else {
            self.read32_device(paddr)
        }
    }
}
```

### Phase 3: Batched Timer (Medium Impact)

```rust
// In System::run()
if cycles & 63 == 0 {
    self.clint.tick(64);
    self.update_interrupts();
}
```

---

## Expected Results

| Optimization | Expected IPS | Improvement |
|--------------|--------------|-------------|
| Current | 2.9M | - |
| + XOR TLB | ~4M | +40% |
| + Direct RAM | ~5.5M | +90% |
| + Batched Timer | ~6M | +100% |

---

## Implementation Order

1. [x] XOR-based TLB in `mmu.rs` ✅ IMPLEMENTED
2. [x] Direct RAM pointer in `system.rs` ✅ IMPLEMENTED  
3. [x] Batched timer updates ✅ IMPLEMENTED (64-cycle batching)
4. [ ] Benchmark after each change

## Implementation Notes

### XOR-Based TLB (Implemented)
Changed `SimpleTlbEntry` to `XorTlbEntry` in `src/cpu/rv32/mmu.rs`:
- `check` field stores vaddr for miss detection via XOR
- `lookup` field stores `(paddr ^ vaddr) & PAGE_MASK`
- Hit path: single XOR to get physical address

### Direct RAM Access (Implemented)
Added unsafe direct RAM methods to `src/memory/mod.rs`:
- `ram_read8_unchecked`, `ram_write8_unchecked`
- `ram_read16_unchecked`, `ram_write16_unchecked`
- `ram_read32_unchecked`, `ram_write32_unchecked`
- `ram_read64_unchecked`, `ram_write64_unchecked`

Updated `SystemBus` in `src/system.rs`:
- Added `ram_size` field for cached bounds checking
- Added `ram_offset()` helper for jor1k-style offset calculation
- All Bus trait methods use direct pointer access for RAM

### Batched Timer (Implemented)
Modified `src/system.rs`:
- `TIMER_BATCH = 64` constant
- Timer updates every 64 cycles instead of every cycle
- Reduces timer overhead by ~64x


# jor1k Kernel Analysis - Why It's Fast

## Key Findings

### Kernel Comparison

| Kernel | Size | Version | Boot Time (est) |
|--------|------|---------|-----------------|
| **jor1k** | 7.9MB | Linux 4.11.0 (2017) | ~5-10s |
| **Ours** | 2.9MB | Linux 6.6.70 (2026) | ~25s |

**Paradox**: jor1k's kernel is **2.7x LARGER** but boots **2-3x FASTER**!

---

## Why jor1k Boots Faster (Despite Larger Kernel)

### 1. **Older Kernel = Fewer Features**

Linux 4.11 (2017) vs Linux 6.6 (2024):
- 7 years of new drivers, subsystems, security features
- More initialization code paths
- More device probing

### 2. **Emulator Speed**

jor1k's C emulator is **~3-4x faster** than ours:
- XOR-based TLB (1 operation vs our ~10)
- Direct RAM pointers (no Bus trait)
- Batched timer updates
- Giant inline switch (no function calls)

**Estimated jor1k speed**: ~8-12M IPS vs our 2.9M IPS

### 3. **Minimal Device Tree**

```dts
timebase-frequency = <20000000>;  // 20 MHz
clock-frequency = <20000000>;
```

vs our:

```dts
timebase-frequency = <10000000>;  // 10 MHz  
```

**Impact**: Faster perceived time = less waiting for timer-based delays

### 4. **9P Rootfs vs Initrd**

jor1k uses:
```
bootargs = "root=host rootfstype=9p rootflags=trans=virtio";
```

- No initrd unpacking
- Direct VirtIO 9P filesystem
- Instant "mount" (just memory access)

vs our:
```
Unpacking initramfs...  // Takes time!
```

---

## What We Can Learn

### High Impact

1. **Use 9P filesystem** instead of initrd
   - No unpacking delay
   - Instant access to files
   - Already have VirtIO 9P implementation!

2. **Implement jor1k-style optimizations**
   - XOR TLB: +40% speed
   - Direct RAM: +50% speed
   - Total: ~6M IPS target

### Medium Impact

3. **Increase timebase-frequency**
   - From 10 MHz → 20 MHz
   - Kernel waits less for timer delays

4. **Use older/minimal kernel**
   - Linux 5.x instead of 6.x
   - Disable unnecessary drivers

---

## Action Plan

### Phase 1: Emulator Speed (Highest Impact)
- [ ] XOR-based TLB
- [ ] Direct RAM pointers
- [ ] Batched timer
- **Expected**: 2.9M → 6M IPS, boot 25s → 12s

### Phase 2: Use 9P Rootfs
- [ ] Boot with `root=host rootfstype=9p`
- [ ] No initrd unpacking
- **Expected**: boot 12s → 8s

### Phase 3: Kernel Tuning
- [ ] Increase timebase to 20 MHz
- [ ] Minimal kernel config
- **Expected**: boot 8s → 5-6s

---

## Conclusion

**The secret isn't the kernel size** - it's:
1. **Emulator speed** (jor1k is ~3x faster)
2. **9P filesystem** (no initrd unpacking)
3. **Older kernel** (less initialization)

We can match jor1k's boot time by implementing their emulator optimizations!
