# RV64GC Upgrade Plan

## Overview

Upgrade OtoRISC emulator from RV32IMAFD to support **both RV32GC and RV64GC**.

**RV32GC = RV32IMAFDC** (current + C extension)
**RV64GC = RV64IMAFDC** (new)

---

## Architecture: Dual RV32/RV64 Support

### Option A: Separate Modules (Recommended)

```
src/
├── lib.rs
├── main.rs
├── system.rs
├── arch/
│   ├── mod.rs          # Re-exports, XLen trait
│   ├── rv32/
│   │   ├── mod.rs
│   │   ├── cpu.rs      # Cpu32 with u32 regs
│   │   ├── decode.rs
│   │   ├── execute.rs
│   │   ├── execute_c.rs  # C extension
│   │   ├── mmu.rs      # Sv32
│   │   └── csr.rs
│   └── rv64/
│       ├── mod.rs
│       ├── cpu.rs      # Cpu64 with u64 regs
│       ├── decode.rs
│       ├── execute.rs
│       ├── execute_c.rs
│       ├── mmu.rs      # Sv39
│       └── csr.rs
├── shared/
│   ├── mod.rs
│   ├── fpu.rs          # Shared FPU operations
│   ├── trap.rs         # Trap definitions
│   └── csr_defs.rs     # CSR address constants
├── devices/            # Shared (parameterized by address width)
│   ├── mod.rs
│   ├── uart.rs
│   ├── plic.rs
│   ├── clint.rs
│   └── virtio/
└── memory/             # Shared
    └── mod.rs
```

### Option B: Generic with XLen Trait

```rust
// src/arch/mod.rs
pub trait XLen: Copy + Clone + Default + ... {
    type Reg: Copy + Into<u64> + From<u64>;
    const XLEN: usize;
    const MISA_MXL: u64;
}

pub struct Rv32;
impl XLen for Rv32 {
    type Reg = u32;
    const XLEN: usize = 32;
    const MISA_MXL: u64 = 1 << 30;
}

pub struct Rv64;
impl XLen for Rv64 {
    type Reg = u64;
    const XLEN: usize = 64;
    const MISA_MXL: u64 = 2 << 62;
}

pub struct Cpu<X: XLen> {
    pub pc: X::Reg,
    pub regs: [X::Reg; 32],
    // ...
}
```

### Option C: Feature Flags (Simplest)

```toml
# Cargo.toml
[features]
default = ["rv32"]
rv32 = []
rv64 = []
```

```rust
#[cfg(feature = "rv32")]
pub type XReg = u32;
#[cfg(feature = "rv64")]
pub type XReg = u64;
```

### Recommended: Hybrid Approach

Use **separate modules** for CPU cores but **shared code** for:
- FPU operations (already 64-bit internally)
- Device interfaces (parameterize address type)
- Trap definitions
- CSR constants

---

## Proposed Directory Structure

```
src/
├── lib.rs
├── main.rs
├── system.rs           # System<A: Arch> or runtime selection
│
├── cpu/
│   ├── mod.rs          # Common types, re-exports
│   ├── trap.rs         # Shared trap definitions
│   ├── fpu.rs          # Shared FPU (already done)
│   │
│   ├── rv32/
│   │   ├── mod.rs      # pub struct Cpu32
│   │   ├── decode.rs   # RV32 decoder
│   │   ├── execute.rs  # RV32I + M + A execution
│   │   ├── execute_fp.rs
│   │   ├── execute_c.rs  # NEW: RV32C
│   │   ├── csr.rs      # 32-bit CSRs
│   │   └── mmu.rs      # Sv32
│   │
│   └── rv64/
│       ├── mod.rs      # pub struct Cpu64
│       ├── decode.rs   # RV64 decoder (adds *W ops)
│       ├── execute.rs  # RV64I + M + A execution
│       ├── execute_fp.rs
│       ├── execute_c.rs  # RV64C
│       ├── csr.rs      # 64-bit CSRs
│       └── mmu.rs      # Sv39
│
├── devices/            # Unchanged, use generic addresses
└── memory/             # Add read64/write64
```

---

## Implementation Phases (Revised for Dual Architecture)

### Phase 0: Restructure for Dual Architecture (2 days)

**0.1 Create directory structure:**
```bash
mkdir -p src/cpu/rv32 src/cpu/rv64
```

**0.2 Move existing RV32 code:**
```bash
# Move current CPU code to rv32/
mv src/cpu/decode.rs src/cpu/rv32/
mv src/cpu/execute.rs src/cpu/rv32/
mv src/cpu/execute_fp.rs src/cpu/rv32/
mv src/cpu/csr.rs src/cpu/rv32/
mv src/cpu/mmu.rs src/cpu/rv32/

# Keep shared code in cpu/
# src/cpu/fpu.rs - already generic
# src/cpu/trap.rs - shared definitions
```

**0.3 Create shared Bus trait:**
```rust
// src/memory/mod.rs
pub trait Bus {
    fn read8(&self, addr: u64) -> u8;
    fn read16(&self, addr: u64) -> u16;
    fn read32(&self, addr: u64) -> u32;
    fn read64(&self, addr: u64) -> u64;  // NEW
    fn write8(&mut self, addr: u64, val: u8);
    fn write16(&mut self, addr: u64, val: u16);
    fn write32(&mut self, addr: u64, val: u32);
    fn write64(&mut self, addr: u64, val: u64);  // NEW
}
```

**0.4 Create Cpu trait for polymorphism:**
```rust
// src/cpu/mod.rs
pub trait Cpu {
    fn step(&mut self, bus: &mut impl Bus) -> Result<(), Trap>;
    fn reset(&mut self);
    fn pc(&self) -> u64;
    fn set_pc(&mut self, pc: u64);
    // ...
}
```

---

## Phase 1: Complete RV32GC (Add C Extension)

Before RV64, complete RV32GC by adding compressed instructions.

### 1.1 Add C Extension to RV32 (src/cpu/rv32/execute_c.rs)

**Instruction fetch change:**
```rust
// src/cpu/rv32/mod.rs
pub fn step(&mut self, bus: &mut impl Bus) -> Result<(), Trap> {
    let inst_lo = bus.read16(self.pc);
    
    if (inst_lo & 0b11) != 0b11 {
        // Compressed 16-bit instruction
        let inst = inst_lo as u32;
        self.execute_compressed(inst, bus)?;
    } else {
        // Standard 32-bit instruction
        let inst_hi = bus.read16(self.pc.wrapping_add(2));
        let inst = ((inst_hi as u32) << 16) | (inst_lo as u32);
        self.execute(inst, bus)?;
    }
    Ok(())
}
```

**RV32C instructions (Quadrant 0, 1, 2):**
| Quadrant | Instructions |
|----------|-------------|
| C0 (00) | C.ADDI4SPN, C.FLD, C.LW, C.FLW, C.FSD, C.SW, C.FSW |
| C1 (01) | C.NOP, C.ADDI, C.JAL, C.LI, C.ADDI16SP, C.LUI, C.SRLI, C.SRAI, C.ANDI, C.SUB, C.XOR, C.OR, C.AND, C.J, C.BEQZ, C.BNEZ |
| C2 (10) | C.SLLI, C.FLDSP, C.LWSP, C.FLWSP, C.JR, C.MV, C.EBREAK, C.JALR, C.ADD, C.FSDSP, C.SWSP, C.FSWSP |

---

## Phase 2: Implement RV64GC Core

### 2.1 Register Width (src/cpu/rv64/mod.rs)

```rust
// Before
pub regs: [u32; 32],
pub pc: u32,

// After
pub regs: [u64; 32],
pub pc: u64,
```

### 2.2 CSR Changes (src/cpu/rv64/csr.rs)

| CSR | RV32 | RV64 |
|-----|------|------|
| mstatus | 32-bit | 64-bit (MSTATUSH merged) |
| mtvec | 32-bit | 64-bit |
| mepc | 32-bit | 64-bit |
| satp | Sv32 mode | Sv39/Sv48 mode |
| mcause | 32-bit | 64-bit |
| All xEPC | 32-bit | 64-bit |

**MISA changes:**
```rust
// RV32: bit 30 = 1 (MXL=1 for 32-bit)
// RV64: bit 63:62 = 0b10 (MXL=2 for 64-bit)
misa: (2u64 << 62) | (1 << 8) | (1 << 12) | (1 << 0) | (1 << 18) | (1 << 5) | (1 << 3) | (1 << 2)
//     MXL=64        I          M           A          S           F          D          C
```

**Implementation checklist:**
- [ ] Promote all RV32 CSR fields used by traps to `u64` (mstatus, sstatus, mtvec, stvec, mepc, sepc, mcause, scause, mtval, stval).
- [ ] Update CSR read/write masks for RV64 (split MSTATUSH if needed, or treat as unified 64-bit).
- [ ] Ensure `csr.read`/`csr.write` return `u64` for RV64 (or widen internally and mask on RV32).
- [ ] Update `Trap` fields and helpers to carry 64-bit `pc` and `tval`.
- [ ] Update `pc` advance logic to use `u64` and preserve low-bit alignment rules.

### 2.3 Instruction Decoding (src/cpu/rv64/decode.rs)

**New RV64I instructions to add:**

| Instruction | Opcode | Description |
|-------------|--------|-------------|
| LWU | LOAD, funct3=110 | Load word unsigned |
| LD | LOAD, funct3=011 | Load doubleword |
| SD | STORE, funct3=011 | Store doubleword |
| ADDIW | OP-IMM-32 (0b0011011) | Add immediate word |
| SLLIW | OP-IMM-32 | Shift left logical immediate word |
| SRLIW | OP-IMM-32 | Shift right logical immediate word |
| SRAIW | OP-IMM-32 | Shift right arithmetic immediate word |
| ADDW | OP-32 (0b0111011) | Add word |
| SUBW | OP-32 | Subtract word |
| SLLW | OP-32 | Shift left logical word |
| SRLW | OP-32 | Shift right logical word |
| SRAW | OP-32 | Shift right arithmetic word |

**New opcodes:**
```rust
pub const OP_OP_IMM_32: u64 = 0b0011011;  // RV64I word operations
pub const OP_OP_32: u64 = 0b0111011;       // RV64I word operations
```

**M extension additions for RV64:**
- MULW, DIVW, DIVUW, REMW, REMUW (word-sized operations)

**A extension additions for RV64:**
- LR.D, SC.D, AMO*.D (doubleword atomics)

**Implementation checklist:**
- [ ] Add RV64 opcodes to decoder (OP-IMM-32, OP-32).
- [ ] Implement LWU/LD/SD paths in execute (sign/zero extension rules).
- [ ] Implement ADDIW/SLLIW/SRLIW/SRAIW and ensure result is sign-extended to XLEN.
- [ ] Implement ADDW/SUBW/SLLW/SRLW/SRAW with sign-extended 32-bit result.
- [ ] Extend M extension with word ops (MULW/DIVW/DIVUW/REMW/REMUW).
- [ ] Extend A extension with 64-bit LR/SC/AMO*.D (alignment to 8 bytes).

### 2.4 MMU Changes (src/cpu/rv64/mmu.rs)

**Sv32 → Sv39 transition:**

| Feature | Sv32 | Sv39 |
|---------|------|------|
| Virtual address | 32-bit | 39-bit (sign-extended to 64) |
| Physical address | 34-bit | 56-bit |
| Page table levels | 2 | 3 |
| PTE size | 4 bytes | 8 bytes |
| VPN bits | 10+10 | 9+9+9 |

**SATP format change:**
```
Sv32 SATP (32-bit):
  [31]    MODE (0=bare, 1=Sv32)
  [30:22] ASID (9 bits)
  [21:0]  PPN (22 bits)

Sv39 SATP (64-bit):
  [63:60] MODE (0=bare, 8=Sv39, 9=Sv48)
  [59:44] ASID (16 bits)
  [43:0]  PPN (44 bits)
```

**Implementation checklist:**
- [ ] Add Sv39 page walk (3 levels, 9-bit VPN segments).
- [ ] Use 8-byte PTE reads and update A/D bits with 64-bit writes.
- [ ] Enforce canonical address check (sign-extend VA[38]).
- [ ] Support 4KB/2MB/1GB pages (leaf PTE at level 0/1/2).
- [ ] Implement RV64 TLB entries (tag size, page size tracking).
- [ ] Invalidate TLB on `satp` write and `SFENCE.VMA`.

### 2.5 FPU Changes (src/cpu/fpu.rs - Shared)

Minimal changes needed:
- FCVT.L.S, FCVT.LU.S (float to 64-bit int)
- FCVT.S.L, FCVT.S.LU (64-bit int to float)
- FCVT.L.D, FCVT.LU.D (double to 64-bit int)
- FCVT.D.L, FCVT.D.LU (64-bit int to double)
- FMV.X.D, FMV.D.X (move bits between x-reg and f-reg)

**Implementation checklist:**
- [ ] Add 64-bit integer conversion ops for single and double precision.
- [ ] Add FMV.X.D / FMV.D.X (bitwise moves).
- [ ] Ensure FCVT results are sign/zero extended correctly for RV64.

### 2.6 RV64C Compressed Instructions (src/cpu/rv64/execute_c.rs)

C extension adds 16-bit instruction formats:

**Quadrant 0 (C0):** opcode[1:0] = 00
- C.ADDI4SPN, C.FLD, C.LW, C.FLW, C.LD (RV64), C.FSD, C.SW, C.FSW, C.SD (RV64)

**Quadrant 1 (C1):** opcode[1:0] = 01
- C.NOP, C.ADDI, C.JAL (RV32), C.ADDIW (RV64), C.LI, C.ADDI16SP, C.LUI
- C.SRLI, C.SRAI, C.ANDI, C.SUB, C.XOR, C.OR, C.AND
- C.SUBW (RV64), C.ADDW (RV64), C.J, C.BEQZ, C.BNEZ

**Quadrant 2 (C2):** opcode[1:0] = 10
- C.SLLI, C.FLDSP, C.LWSP, C.FLWSP, C.LDSP (RV64)
- C.JR, C.MV, C.EBREAK, C.JALR, C.ADD
- C.FSDSP, C.SWSP, C.FSWSP, C.SDSP (RV64)

**Instruction fetch changes:**
```rust
pub fn step(&mut self, bus: &mut impl Bus) -> Result<(), Trap> {
    let inst_lo = bus.read16(paddr);
    
    if (inst_lo & 0b11) != 0b11 {
        // Compressed instruction (16-bit)
        self.execute_compressed(inst_lo as u32, bus)?;
        self.pc = self.pc.wrapping_add(2);
    } else {
        // Normal instruction (32-bit)
        let inst_hi = bus.read16(paddr + 2);
        let inst = (inst_hi as u32) << 16 | inst_lo as u32;
        self.execute(inst, bus)?;
        // PC already updated by execute
    }
}
```

**Implementation checklist:**
- [ ] Implement RV64C-specific ops: C.LD/C.SD, C.LDSP/C.SDSP, C.ADDIW, C.ADDW, C.SUBW.
- [ ] Ensure compressed immediate decoding uses RV64 sign-extension rules.
- [ ] Validate 16-bit instruction fetch on odd/aligned addresses.

---

## Phase 2: Memory and Bus Changes

### 2.1 Memory Module (src/memory/mod.rs)

```rust
// Add 64-bit read/write
fn read64(&self, addr: u64) -> u64;
fn write64(&mut self, addr: u64, value: u64);
```

### 2.2 Device MMIO Updates

All devices need address parameters changed from u32 to u64:
- UART
- PLIC  
- CLINT
- VirtIO

---

## Phase 3: Buildroot RV64 Toolchain

### 3.1 Create New Buildroot Config

```bash
cd build-linux/buildroot-2025.02.9

# Start fresh config for RV64
make clean
make qemu_riscv64_virt_defconfig

# Or create custom config
cat > configs/riscv64_minimal_defconfig << 'EOF'
BR2_riscv=y
BR2_RISCV_64=y
BR2_RISCV_ABI_LP64D=y
BR2_RISCV_ISA_RVC=y
BR2_TOOLCHAIN_BUILDROOT_MUSL=y
BR2_PACKAGE_BUSYBOX=y
BR2_TARGET_ROOTFS_CPIO=y
BR2_TARGET_ROOTFS_CPIO_GZIP=y
BR2_LINUX_KERNEL=y
BR2_LINUX_KERNEL_CUSTOM_VERSION=y
BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE="6.6.70"
BR2_LINUX_KERNEL_USE_CUSTOM_CONFIG=y
BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE="$(BR2_EXTERNAL)/kernel_rv64_minimal.config"
EOF

make riscv64_minimal_defconfig
make -j$(nproc)
```

### 3.2 Toolchain Output

After build, toolchain at:
```
build-linux/buildroot-2025.02.9/output/host/bin/riscv64-buildroot-linux-musl-gcc
```

---

## Phase 4: Kernel Build for RV64

### 4.1 Kernel Configuration

```bash
cd build-linux/linux-6.6.70

# Clean previous RV32 build
make ARCH=riscv mrproper

# Configure for RV64
make ARCH=riscv CROSS_COMPILE=riscv64-buildroot-linux-musl- defconfig

# Minimize config
make ARCH=riscv CROSS_COMPILE=riscv64-buildroot-linux-musl- menuconfig
# Disable: modules, networking (if not needed), most drivers
# Enable: virtio, 9p, serial console

make ARCH=riscv CROSS_COMPILE=riscv64-buildroot-linux-musl- -j$(nproc)
```

### 4.2 Key Kernel Config Differences

```
# RV64-specific
CONFIG_ARCH_RV64I=y
CONFIG_64BIT=y
CONFIG_RISCV_ISA_C=y
CONFIG_RISCV_ISA_ZICBOM=n  # Optional extensions
CONFIG_FPU=y
```

---

## Phase 5: TCC for RV64

### 5.1 Option A: Use Existing TCC RV64 Support

TCC has some RV64 support upstream. Check:
```bash
cd tinycc
git log --oneline --grep="riscv64"
```

### 5.2 Option B: Port tcc-riscv32 to RV64

Key changes in TCC:
1. `riscv64-gen.c` - Code generation for RV64
2. `riscv64-link.c` - ELF linking for RV64  
3. `riscv64-asm.c` - Assembler for RV64

**Register differences:**
- Pointer size: 4 → 8 bytes
- Long size: 4 → 8 bytes
- ABI: ILP32D → LP64D

**New instructions needed:**
- LD, SD for 64-bit loads/stores
- *W variants for 32-bit operations
- Address calculations use 64-bit

### 5.3 Build TCC for RV64

```bash
cd tinycc-forks/tcc-riscv64  # new directory

./configure \
    --cross-prefix=riscv64-buildroot-linux-musl- \
    --cpu=riscv64 \
    --enable-static

make
```

---

## Phase 6: Testing Plan

### 6.1 Unit Tests

```rust
#[cfg(test)]
mod rv64_tests {
    #[test]
    fn test_ld_sd() { /* ... */ }
    
    #[test]
    fn test_addiw() { /* ... */ }
    
    #[test]
    fn test_compressed_c_addi() { /* ... */ }
    
    #[test]
    fn test_sv39_translation() { /* ... */ }
}
```

### 6.2 RISC-V Compliance Tests

```bash
# Run official RISC-V architecture tests
cd tools/riscv-arch-test
./run_tests.sh rv64i rv64m rv64a rv64f rv64d rv64c
```

### 6.3 Linux Boot Test

```bash
cargo run --release -- \
    -k images/Image-rv64-minimal \
    -i images/rootfs-rv64.cpio
```

---

## Implementation Order (Revised)

### Phase 0: Restructure (2 days)
1. [ ] Create src/cpu/rv32/ and src/cpu/rv64/ directories
2. [ ] Move existing code to rv32/
3. [ ] Update Bus trait with u64 addresses and read64/write64
4. [ ] Create shared Cpu trait
5. [ ] Verify RV32 still works after restructure

### Phase 1: Complete RV32GC (2-3 days)
1. [ ] Implement compressed instruction fetch (16/32-bit detection)
2. [ ] Add RV32C decoder
3. [ ] Implement C0 quadrant (C.ADDI4SPN, C.LW, C.SW, C.FLW, C.FSW, etc.)
4. [ ] Implement C1 quadrant (C.ADDI, C.JAL, C.J, C.BEQZ, C.BNEZ, etc.)
5. [ ] Implement C2 quadrant (C.SLLI, C.LWSP, C.SWSP, C.JR, C.JALR, etc.)
6. [ ] Test with compressed-enabled kernel

### Phase 2: RV64GC Core (3-4 days)
1. [ ] Copy rv32/ to rv64/ as starting point
2. [ ] Change register width to 64-bit
3. [ ] Update PC to 64-bit
4. [ ] Add LD, SD, LWU instructions
5. [ ] Add *W instruction variants (ADDIW, ADDW, SUBW, etc.)
6. [ ] Update CSRs to 64-bit
7. [ ] Update MISA for RV64 (MXL=2)

### Phase 3: RV64 MMU and Extensions (2 days)
1. [ ] Implement Sv39 page table walk
2. [ ] Update M extension with *W variants (MULW, DIVW, etc.)
3. [ ] Update A extension with *.D atomics (LR.D, SC.D, AMO*.D)
4. [ ] Add FPU L/LU conversions and FMV.X.D/FMV.D.X
5. [ ] Port C extension to RV64 (add C.LD, C.SD, C.ADDIW, etc.)

### Phase 4: Toolchain (2 days)
1. [ ] Build Buildroot RV64 toolchain
2. [ ] Build Linux kernel for RV64
3. [ ] Build or port TCC for RV64

### Phase 5: Testing (2 days)
1. [ ] Run RISC-V compliance tests for RV64
2. [ ] Boot Linux RV64
3. [ ] Test TCC on RV64

---

## File Change Summary (Revised)

| File | Status | Description |
|------|--------|-------------|
| `src/cpu/mod.rs` | Modified | Add Cpu trait, re-export rv32/rv64 |
| `src/cpu/fpu.rs` | Shared | Already 64-bit, add L/LU conversions |
| `src/cpu/trap.rs` | Shared | Move from rv32, use u64 for addresses |
| `src/cpu/rv32/mod.rs` | Moved | Cpu32 struct |
| `src/cpu/rv32/decode.rs` | Moved | RV32 decoder |
| `src/cpu/rv32/execute.rs` | Moved | RV32 execution |
| `src/cpu/rv32/execute_fp.rs` | Moved | RV32 FP execution |
| `src/cpu/rv32/execute_c.rs` | **NEW** | RV32C compressed |
| `src/cpu/rv32/csr.rs` | Moved | 32-bit CSRs |
| `src/cpu/rv32/mmu.rs` | Moved | Sv32 MMU |
| `src/cpu/rv64/mod.rs` | **NEW** | Cpu64 struct |
| `src/cpu/rv64/decode.rs` | **NEW** | RV64 decoder |
| `src/cpu/rv64/execute.rs` | **NEW** | RV64 execution |
| `src/cpu/rv64/execute_fp.rs` | **NEW** | RV64 FP execution |
| `src/cpu/rv64/execute_c.rs` | **NEW** | RV64C compressed |
| `src/cpu/rv64/csr.rs` | **NEW** | 64-bit CSRs |
| `src/cpu/rv64/mmu.rs` | **NEW** | Sv39 MMU |
| `src/memory/mod.rs` | Modified | Add read64/write64, use u64 addresses |
| `src/devices/*.rs` | Modified | Use u64 addresses |
| `src/system.rs` | Modified | Support both Cpu32 and Cpu64 |
| `src/main.rs` | Modified | CLI flag for --arch rv32/rv64 |

---

## Command Line Interface

```bash
# Run RV32 (default, backward compatible)
cargo run --release -- -k images/Image-rv32 -i images/rootfs-rv32.cpio

# Run RV64
cargo run --release -- --arch rv64 -k images/Image-rv64 -i images/rootfs-rv64.cpio

# Or with feature flags (compile-time selection)
cargo run --release --features rv64 -- -k images/Image-rv64 -i images/rootfs-rv64.cpio
```

---

## Estimated Effort (Revised)

| Phase | Effort |
|-------|--------|
| Phase 0: Restructure | 2 days |
| Phase 1: RV32GC (C extension) | 2-3 days |
| Phase 2: RV64GC Core | 3-4 days |
| Phase 3: RV64 MMU/Extensions | 2 days |
| Phase 4: Toolchain | 2 days (mostly build time) |
| Phase 5: Testing | 2 days |
| **Total** | **~13-15 days** |

---

## Code Sharing Strategy

### Shared Between RV32 and RV64:

1. **FPU operations** (`src/cpu/fpu.rs`)
   - Already uses f32/f64 internally
   - Add shared L/LU conversion functions

2. **Trap definitions** (`src/cpu/trap.rs`)
   - Exception/interrupt codes same
   - Use u64 for addresses (RV32 just uses lower 32 bits)

3. **CSR address constants** (separate file)
   - CSR addresses are the same
   - Only values/widths differ

4. **Devices** (`src/devices/*.rs`)
   - MMIO addresses fit in 32-bit
   - Use u64 parameter, works for both

5. **Instruction format constants**
   - Opcodes identical
   - funct3/funct7 identical

### Different Between RV32 and RV64:

| Component | RV32 | RV64 |
|-----------|------|------|
| Register type | `u32` | `u64` |
| PC type | `u32` | `u64` |
| CSR value type | `u32` | `u64` |
| MISA.MXL | `1` (bits 31:30) | `2` (bits 63:62) |
| SATP format | Sv32 | Sv39/Sv48 |
| Page table levels | 2 | 3-4 |
| PTE size | 4 bytes | 8 bytes |
| Has *W instructions | No | Yes |
| Has *.D atomics | No | Yes |
| C.JAL | Yes | No (→ C.ADDIW) |
| C.LD/C.SD | No | Yes |

---

## References

- [RISC-V Unprivileged Spec](https://riscv.org/specifications/)
- [RISC-V Privileged Spec](https://riscv.org/specifications/privileged-isa/)
- [RISC-V C Extension](https://riscv.org/specifications/)
- [Sv39 Virtual Memory](https://five-embeddev.com/riscv-isa-manual/latest/supervisor.html#sv39)
