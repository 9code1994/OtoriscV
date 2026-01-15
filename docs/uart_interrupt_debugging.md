# Linux Boot & UART Interrupt Debugging Journey

This document chronicles the debugging journey to get Linux booting and interactive I/O working on the OtoRISCV emulator. It covers two major issues:

1. **Linux Boot Failure** — Kernel crashed/hung during boot
2. **UART Input Broken** — No keyboard input after boot succeeded

---

# Part 1: Linux Boot Failures

## The Problem

When first attempting to boot Linux, the kernel would crash or hang very early in the boot process. No kernel messages appeared, or the system would trap into an infinite loop.

## Root Cause Analysis

### Bug #1: Misaligned Memory Access

**Symptom**: Linux kernel crashed with `LoadAddressMisaligned` or `StoreAddressMisaligned` exceptions.

**Investigation**: The RISC-V spec allows implementations to trap on misaligned access, but Linux kernel **assumes hardware handles misaligned access**. The kernel performs misaligned loads/stores in various places (especially in optimized memcpy, string operations, and during early boot).

**Original code** (pre-215973c):
```rust
// Threw exceptions on misaligned access
FUNCT3_LH => {
    if vaddr & 1 != 0 {
        return Err(Trap::LoadAddressMisaligned(vaddr));
    }
    bus.read16(paddr) as i16 as i32 as u32
}

FUNCT3_LW => {
    if vaddr & 3 != 0 {
        return Err(Trap::LoadAddressMisaligned(vaddr));
    }
    bus.read32(paddr)
}
```

**Solution** (commit 215973c): Emulate misaligned access byte-by-byte:

```rust
// Emulate misaligned access in software
FUNCT3_LH => {
    if vaddr & 1 != 0 {
        // Byte-by-byte load for misaligned half-word
        let b0 = bus.read8(paddr) as u32;
        let b1 = bus.read8(paddr.wrapping_add(1)) as u32;
        ((b1 << 8) | b0) as i16 as i32 as u32
    } else {
        bus.read16(paddr) as i16 as i32 as u32
    }
}

FUNCT3_LW => {
    if vaddr & 3 != 0 {
        // Byte-by-byte load for misaligned word
        let b0 = bus.read8(paddr) as u32;
        let b1 = bus.read8(paddr.wrapping_add(1)) as u32;
        let b2 = bus.read8(paddr.wrapping_add(2)) as u32;
        let b3 = bus.read8(paddr.wrapping_add(3)) as u32;
        (b3 << 24) | (b2 << 16) | (b1 << 8) | b0
    } else {
        bus.read32(paddr)
    }
}
```

Same pattern applied to `SH`, `SW`, `LHU` instructions.

---

### Bug #2: Exception Delegation Missing

**Symptom**: Linux kernel hung in infinite loop — ecalls from U-mode trapped to M-mode handler instead of S-mode.

**Investigation**: Linux runs in S-mode and expects to handle exceptions from U-mode applications (syscalls). But the emulator's boot ROM wasn't setting up exception delegation properly.

**Original boot ROM**: Simple jump to kernel:
```rust
let instructions: [u32; 2] = [
    0x7ffff297,  // auipc t0, 0x7ffff (t0 = 0x80000000)
    0x00028067,  // jalr zero, t0, 0
];
```

**Problems**:
1. No exception delegation setup (`medeleg`)
2. No interrupt delegation setup (`mideleg`)
3. Kernel starts in M-mode instead of S-mode
4. No SBI handler for ecalls from S-mode

**Solution** (commit 215973c): Full boot ROM acting as minimal SBI firmware:

```rust
let instructions: [u32; 29] = [
    // === Setup exception delegation ===
    // medeleg = 0xB1FF: delegate exceptions 0-8, 12-15 to S-mode
    // Bit 8 (ecall from U-mode) IS delegated
    // Bit 9 (ecall from S-mode) is NOT delegated (handled by SBI)
    0x0000b2b7,  // lui t0, 0xB
    0x1ff28293,  // addi t0, t0, 0x1FF   ; t0 = 0xB1FF
    0x30229073,  // csrw medeleg, t0
    
    // Delegate S-mode interrupts (SSI, STI, SEI)
    0x00000293,  // li t0, 0
    0x22228293,  // addi t0, t0, 0x222   ; t0 = 0x222
    0x30329073,  // csrw mideleg, t0
    
    // === Setup mstatus: MPP=Supervisor, MPIE=1 ===
    0x00001337,  // lui t1, 1
    0x88030313,  // addi t1, t1, -0x780  ; t1 = 0x880
    0x30031073,  // csrw mstatus, t1
    
    // === Set mepc to kernel entry ===
    0x800002b7,  // lui t0, 0x80000
    0x34129073,  // csrw mepc, t0
    
    // === Setup mtvec for SBI handler ===
    0x000012b7,  // lui t0, 0x1
    0x08028293,  // addi t0, t0, 0x80    ; t0 = 0x1080
    0x30529073,  // csrw mtvec, t0
    
    // === Enable counters for S-mode ===
    0x00700293,  // li t0, 7
    0x30629073,  // csrw mcounteren, t0
    
    // === Drop to S-mode via MRET ===
    0x30200073,  // mret
    // ...
];
```

---

### Bug #3: SBI Call Handling

**Symptom**: After kernel boots, it immediately crashes when trying to print messages.

**Investigation**: Linux uses SBI ecalls to communicate with M-mode firmware. When the kernel executes `ecall` in S-mode, the trap goes to M-mode (since medeleg bit 9 is not set). But our M-mode handler was just an infinite loop.

**Solution**: Handle SBI calls directly in Rust (system.rs):

```rust
// In run loop
match self.step_with_devices() {
    Ok(()) => {}
    Err(trap) => {
        // Handle SBI calls from S-mode directly
        if let Trap::EnvironmentCallFromS = trap {
            self.handle_sbi_call();
        } else {
            self.cpu.handle_trap(trap);
        }
    }
}

fn handle_sbi_call(&mut self) {
    let eid = self.cpu.read_reg(17);  // a7 = Extension ID
    let fid = self.cpu.read_reg(16);  // a6 = Function ID
    let a0 = self.cpu.read_reg(10);
    
    match eid {
        0x01 => {  // Legacy console_putchar
            print!("{}", a0 as u8 as char);
        }
        0x4442434E => {  // DBCN (Debug Console)
            // Handle debug console extension
        }
        // ... other extensions
    }
    
    // Advance PC past ecall instruction
    self.cpu.pc = self.cpu.pc.wrapping_add(4);
}
```

---

### Bug #4: Initrd Loading

**Symptom**: Kernel boots but panics: "No working init found"

**Investigation**: Kernel couldn't find the initial ramdisk (initrd) containing `/init`. The DTB didn't specify initrd location.

**Solution**: Proper initrd loading with DTB integration:

```rust
pub fn setup_linux_boot_with_initrd(
    &mut self, 
    kernel: &[u8], 
    initrd: Option<&[u8]>, 
    cmdline: &str
) -> Result<(), String> {
    // Load kernel at DRAM_BASE
    self.load_binary(kernel, DRAM_BASE)?;
    
    // Load initrd if provided
    let initrd_info = if let Some(initrd_data) = initrd {
        let initrd_end = (ram_end - 64*1024) & !0xFFF;
        let initrd_start = (initrd_end - initrd_data.len() as u32) & !0xFFF;
        self.load_binary(initrd_data, initrd_start)?;
        Some((initrd_start, initrd_start + initrd_data.len() as u32))
    } else {
        None
    };
    
    // Generate DTB with initrd info (linux,initrd-start/end)
    let dtb = generate_fdt(ram_size_mb, cmdline, initrd_info);
    // ...
}
```

---

## Linux Boot: Summary of Fixes

| Bug | Issue | Fix |
|-----|-------|-----|
| 1 | Misaligned load/store trapped | Emulate byte-by-byte |
| 2 | No exception delegation | Setup medeleg/mideleg in boot ROM |
| 3 | No SBI call handling | Handle ecalls from S-mode in Rust |
| 4 | Initrd not found | Pass initrd location in DTB |

After these fixes, Linux successfully boots to userspace.

---

# Part 2: UART Interrupt Issues

## The Problem

After booting Linux on the RISC-V emulator, the system appeared to work — kernel messages printed correctly via SBI `console_putchar`. However:

1. **No keyboard input** — typing in the terminal did nothing
2. **No shell prompt interaction** — programs couldn't read from stdin
3. **One-way communication** — output worked, input was completely broken

## Initial State (commit 3f53c1a)

The emulator could boot Linux and print kernel messages via the SBI legacy console interface. But any attempt to read input failed silently.

---

## Debugging Journey

### Phase 1: Verify Host Input Reaches UART

**Hypothesis**: Maybe the host terminal input isn't even reaching the UART device.

**Investigation**: Added debug prints to `main.rs` and `uart.receive_char()`.

**Finding**: ✅ Characters were being received and pushed into `uart.rx_fifo`. The host-side path was working.

---

### Phase 2: Check UART Interrupt Generation

**Hypothesis**: Maybe the UART isn't raising an interrupt when data arrives.

**Investigation**: Instrumented `uart.has_interrupt()` and checked `interrupt_flags`.

**Finding**: 🐛 **Bug #1 Found** — The original UART implementation had a `pending_interrupt` boolean that was checked in `check_interrupt()`, but the logic was problematic:

```rust
// BROKEN: check_interrupt() had issues
fn check_interrupt(&mut self) {
    let mut interrupt = false;
    if !self.rx_fifo.is_empty() && (self.ier & IER_RX_AVAILABLE) != 0 {
        interrupt = true;
    }
    // TX always triggers if enabled - wrong!
    if (self.ier & IER_TX_EMPTY) != 0 {
        interrupt = true;  // This fires constantly
    }
    self.pending_interrupt = interrupt;
}
```

**Problems**:
1. TX interrupt fired constantly if enabled, not just when data was written
2. RX interrupt wasn't tracked as a persistent flag
3. Reading IIR didn't clear TX interrupt (per 16550 spec, reading IIR clears THRI)

---

### Phase 3: Fix UART Interrupt Model (commit 215973c)

**Solution**: Changed from boolean `pending_interrupt` to proper `interrupt_flags` bitmask:

```rust
// FIXED: Proper interrupt flag tracking
pub fn receive_char(&mut self, c: u8) {
    self.rx_fifo.push_back(c);
    self.interrupt_flags |= IIR_RX_AVAILABLE;  // Set RX flag
}

pub fn has_interrupt(&self) -> bool {
    // RX interrupt: flag AND enabled
    if (self.interrupt_flags & IIR_RX_AVAILABLE) != 0 && (self.ier & IER_RX_AVAILABLE) != 0 {
        return true;
    }
    // TX interrupt: flag AND enabled
    if (self.interrupt_flags & IIR_TX_EMPTY) != 0 && (self.ier & IER_TX_EMPTY) != 0 {
        return true;
    }
    false
}

fn get_iir(&mut self) -> u8 {
    // Reading IIR when THRI is pending clears it (per 16550 spec)
    if (self.interrupt_flags & IIR_TX_EMPTY) != 0 {
        self.interrupt_flags &= !IIR_TX_EMPTY;
        return fifo_bits | IIR_TX_EMPTY;
    }
    // ...
}
```

**Also fixed**: Reading RBR now clears RX interrupt when FIFO becomes empty.

---

### Phase 4: Discover Bus Trait Mutability Issue

**Hypothesis**: UART interrupt is raised, but maybe PLIC isn't seeing it?

**Investigation**: Added debug to PLIC and interrupt path.

**Finding**: 🐛 **Bug #2 Found** — The `Bus` trait had `fn read8(&self, ...)` (immutable), but UART's `read8` needed to be mutable to:
- Clear interrupt flags when IIR is read
- Pop from RX FIFO when RBR is read

**Solution**: Changed Bus trait to use `&mut self`:

```rust
// BEFORE (broken)
pub trait Bus {
    fn read8(&self, addr: u32) -> u8;  // Can't mutate!
    // ...
}

// AFTER (fixed)
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;  // Now UART can clear flags
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    // ...
}
```

---

### Phase 5: PLIC Priority Bug

**Hypothesis**: UART interrupt reaches PLIC, but claim returns nothing.

**Investigation**: Traced `plic.claim()` and `plic.find_pending()`.

**Finding**: 🐛 **Bug #3 Found** — PLIC priority comparison was wrong:

```rust
// BROKEN
if priority > self.threshold[context] && priority > best_priority {
    // When priority=1 and threshold=0, this works...
    // But when priority=0, interrupt is silently dropped!
}
```

Per PLIC spec, priority 0 means "never interrupt" — the check should be `priority > 0`.

**Solution** (commit 1b5101c):

```rust
// FIXED
if priority > 0 && priority > self.threshold[context] && priority >= best_priority {
    // Now handles priority correctly
}
```

---

### Phase 6: PLIC Claim Returns Wrong Value

**Investigation**: PLIC was setting `claimed[context]` but `read32` for the claim register was returning `claimed[context]` directly instead of calling `claim()`.

**Finding**: 🐛 **Bug #4 Found** — Reading PLIC claim register didn't actually perform the claim:

```rust
// BROKEN
4 => self.claimed[context],  // Just returns cached value!

// FIXED
4 => self.claim(context),  // Actually perform claim logic
```

---

### Phase 7: Host Input Still Not Working

After all PLIC/UART fixes, the interrupt path was correct. But typing still didn't work.

**Investigation**: Added more debug, found interrupts were pending but never taken.

**Finding**: Debug output showed:
```
[IRQ] SEIP pending! mip=000002a0 mie=00000220 mideleg=00000222 mstatus=000000a8
[IRQ]   sie_enabled=false  ← THIS IS THE PROBLEM
```

The **SIE bit in mstatus was 0**, meaning S-mode interrupts were globally disabled.

---

### Phase 8: Why Is SIE Disabled?

**Hypothesis**: Linux should enable SIE. Why isn't it?

**Investigation**: The test `init_minishell.c` had a tight polling loop:

```c
while (cmd_len < 126) {
    long n = syscall3(SYS_read, 0, (long)&c, 1);
    if (n <= 0) continue;  // ← BUSY WAIT!
    // ...
}
```

**Root Cause**: When `read()` returns immediately (EAGAIN), the loop spins at 100% CPU. The kernel:
1. Never goes idle
2. Never re-enables SIE
3. Never processes pending interrupts

**Solution**: Add yielding when no data available:

```c
static void yield_cpu(void) {
    struct timespec ts = { 0, 10000000 };  // 10ms
    syscall2(SYS_nanosleep, (long)&ts, 0);
}

if (n <= 0) {
    yield_cpu();  // Let kernel process interrupts
    continue;
}
```

---

### Phase 9: Non-Blocking Host Input (commit 1b5101c)

**Final Issue**: Host terminal was blocking, not in raw mode.

**Solution**: Set terminal to raw mode and non-blocking:

```rust
fn set_raw_terminal(enable: bool) {
    // Disable canonical mode and echo
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    // Set minimum chars and timeout for read
    raw.c_cc[libc::VMIN] = 0;
    raw.c_cc[libc::VTIME] = 0;
}

fn set_nonblocking(fd: i32, nonblocking: bool) {
    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
}
```

---

## Complete Interrupt Flow (After Fixes)

```
Host stdin (raw mode, non-blocking)
            ↓
main.rs: libc::read() → got char 'a'
            ↓
system.uart_receive('a')
            ↓
uart.receive_char('a'):
    rx_fifo.push('a')
    interrupt_flags |= IIR_RX_AVAILABLE  ← Flag set
            ↓
system.update_interrupts():
    uart.has_interrupt() == true
    plic.raise_interrupt(10)            ← SEIP raised
            ↓
cpu.check_interrupts():
    MIP.SEIP = 1, SIE = 1               ← Kernel is idle
    → Trap::SupervisorExternalInterrupt
            ↓
Linux IRQ handler:
    plic.claim() → returns 10
    uart.read8(IIR) → 0x04 (RX available)
    uart.read8(RBR) → 'a'               ← Data read!
    plic.complete(10)
            ↓
Linux tty layer → userspace read() → 'a'
```

---

## Summary of All Bugs Fixed

### Part 1: Linux Boot Fixes

| Bug | File | Issue | Fix |
|-----|------|-------|-----|
| B1 | cpu/execute.rs | Misaligned load/store trapped | Emulate byte-by-byte |
| B2 | memory/mod.rs | No exception delegation | Setup medeleg/mideleg in boot ROM |
| B3 | system.rs | No SBI call handling | Handle ecalls from S-mode in Rust |
| B4 | system.rs, dtb.rs | Initrd not found | Pass initrd location in DTB |

### Part 2: UART/Interrupt Fixes

| Bug | File | Issue | Fix |
|-----|------|-------|-----|
| 1 | uart.rs | TX interrupt fires constantly | Use `interrupt_flags` bitmask |
| 2 | uart.rs | RBR read doesn't pop FIFO | Pop from FIFO on read |
| 3 | uart.rs | IIR read doesn't clear THRI | Clear TX flag on IIR read |
| 4 | memory/mod.rs | Bus::read8 is immutable | Change to `&mut self` |
| 5 | plic.rs | Priority 0 should never interrupt | Add `priority > 0` check |
| 6 | plic.rs | Claim register read wrong | Call `claim()` not cached value |
| 7 | main.rs | Terminal not in raw mode | Set raw mode + non-blocking |
| 8 | init_*.c | Busy-wait prevents interrupts | Use `nanosleep()` to yield |

---

## Key Learnings

### Linux Boot
1. **Misaligned access is common** — Linux assumes hardware handles it; emulate byte-by-byte if not
2. **Exception delegation is critical** — medeleg/mideleg must be set for S-mode OS to handle traps
3. **SBI is the M-mode ↔ S-mode interface** — Handle ecalls from S-mode to provide console, timer, etc.
4. **DTB must describe hardware** — Initrd location, memory size, device addresses all in DTB

### UART Interrupts
5. **16550 UART semantics matter** — Reading IIR clears THRI, reading RBR pops FIFO
6. **Bus trait mutability** — Device reads can have side effects (clearing flags)
7. **PLIC priority 0 = disabled** — Per spec, priority 0 means "never interrupt"
8. **SIE must be enabled** — S-mode interrupts require mstatus.SIE = 1
9. **Busy-wait prevents interrupts** — Kernel only enables SIE when idle or returning from syscall
10. **Raw terminal mode** — Required for character-at-a-time input
11. **Non-blocking I/O** — Required for emulator main loop to poll input

---

## Commits

- **215973c**: Linux boot fixes (misaligned access, exception delegation, SBI handling, initrd), UART interrupt model fix, Bus mutability
- **1b5101c**: Fix PLIC claim, add non-blocking input, raw terminal mode

---

## Reference: mstatus Bits

| Bit | Name | Description |
|-----|------|-------------|
| 1 | SIE | Supervisor Interrupt Enable |
| 3 | MIE | Machine Interrupt Enable |
| 5 | SPIE | Previous SIE before trap |
| 7 | MPIE | Previous MIE before trap |

## Reference: MIP/MIE Bits

| Bit | Name | Description |
|-----|------|-------------|
| 1 | SSIP/SSIE | Supervisor Software Interrupt |
| 5 | STIP/STIE | Supervisor Timer Interrupt |
| 9 | SEIP/SEIE | Supervisor External Interrupt |

