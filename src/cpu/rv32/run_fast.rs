//! Giant inline switch execution loop
//!
//! jor1k-style monolithic execution for maximum performance.
//! All hot opcodes inlined, state destructured into locals.

use crate::cpu::Cpu;
use crate::cpu::rv32::mmu::AccessType;
use crate::cpu::csr::*;
use crate::cpu::trap::Trap;
use crate::cpu::PrivilegeLevel;
use crate::memory::{Memory, Bus, DRAM_BASE};
use crate::devices::{Uart, Clint, Plic, Virtio9p};

/// Opcode constants (matching decode.rs)
const OP_LUI: u32 = 0b0110111;
const OP_AUIPC: u32 = 0b0010111;
const OP_JAL: u32 = 0b1101111;
const OP_JALR: u32 = 0b1100111;
const OP_BRANCH: u32 = 0b1100011;
const OP_LOAD: u32 = 0b0000011;
const OP_STORE: u32 = 0b0100011;
const OP_OP_IMM: u32 = 0b0010011;
const OP_OP: u32 = 0b0110011;
const OP_MISC_MEM: u32 = 0b0001111;
const OP_SYSTEM: u32 = 0b1110011;
const OP_AMO: u32 = 0b0101111;
const OP_LOAD_FP: u32 = 0b0000111;
const OP_STORE_FP: u32 = 0b0100111;
const OP_MADD: u32 = 0b1000011;
const OP_MSUB: u32 = 0b1000111;
const OP_NMSUB: u32 = 0b1001011;
const OP_NMADD: u32 = 0b1001111;
const OP_OP_FP: u32 = 0b1010011;

/// Inline immediate extractors
#[inline(always)]
fn imm_i(inst: u32) -> i32 { (inst as i32) >> 20 }

#[inline(always)]
fn imm_s(inst: u32) -> i32 {
    ((inst & 0xFE000000) as i32 >> 20) | ((inst >> 7) & 0x1F) as i32
}

#[inline(always)]
fn imm_b(inst: u32) -> i32 {
    ((inst & 0x80000000) as i32 >> 19) |
        (((inst >> 7) & 1) << 11) as i32 |
        (((inst >> 25) & 0x3F) << 5) as i32 |
        (((inst >> 8) & 0xF) << 1) as i32
}

#[inline(always)]
fn imm_u(inst: u32) -> u32 { inst & 0xFFFF_F000 }

#[inline(always)]
fn imm_j(inst: u32) -> i32 {
    ((inst & 0x80000000) as i32 >> 11) |
        (inst & 0xFF000) as i32 |
        (((inst >> 20) & 1) << 11) as i32 |
        (((inst >> 21) & 0x3FF) << 1) as i32
}

/// System bus for inline memory access
pub struct FastBus<'a> {
    pub memory: &'a mut Memory,
    pub uart: &'a mut Uart,
    pub clint: &'a mut Clint,
    pub plic: &'a mut Plic,
    pub virtio9p: &'a mut Virtio9p,
}

impl<'a> FastBus<'a> {
    /// Inline RAM/ROM read32 - uses Memory::read32 which handles ROM and RAM
    #[inline(always)]
    pub fn read32_fast(&mut self, paddr: u32) -> u32 {
        // Memory::read32 handles both ROM (0x1000) and RAM (0x80000000+)
        if paddr >= DRAM_BASE || (paddr >= 0x1000 && paddr < 0x3000) {
            self.memory.read32(paddr)
        } else {
            self.read32_device(paddr)
        }
    }

    #[cold]
    fn read32_device(&mut self, paddr: u32) -> u32 {
        // Device access - slow path
        const CLINT_BASE: u32 = 0x0200_0000;
        const UART_BASE: u32 = 0x0300_0000;
        const PLIC_BASE: u32 = 0x0400_0000;
        const VIRTIO_BASE: u32 = 0x2000_0000;
        
        if paddr >= CLINT_BASE && paddr < CLINT_BASE + 0x10000 {
            self.clint.read32(paddr - CLINT_BASE)
        } else if paddr >= UART_BASE && paddr < UART_BASE + 0x1000 {
            self.uart.read8(paddr - UART_BASE) as u32
        } else if paddr >= PLIC_BASE && paddr < PLIC_BASE + 0x400000 {
            self.plic.read32(paddr - PLIC_BASE)
        } else if paddr >= VIRTIO_BASE && paddr < VIRTIO_BASE + 0x1000 {
            self.virtio9p.read32(paddr - VIRTIO_BASE)
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn write32_fast(&mut self, paddr: u32, value: u32) {
        if paddr >= DRAM_BASE {
            self.memory.write32(paddr, value);
        } else {
            self.write32_device(paddr, value);
        }
    }

    #[cold]
    fn write32_device(&mut self, paddr: u32, value: u32) {
        const CLINT_BASE: u32 = 0x0200_0000;
        const UART_BASE: u32 = 0x0300_0000;
        const PLIC_BASE: u32 = 0x0400_0000;
        const VIRTIO_BASE: u32 = 0x2000_0000;
        
        if paddr >= CLINT_BASE && paddr < CLINT_BASE + 0x10000 {
            self.clint.write32(paddr - CLINT_BASE, value);
        } else if paddr >= UART_BASE && paddr < UART_BASE + 0x1000 {
            self.uart.write8(paddr - UART_BASE, value as u8);
        } else if paddr >= PLIC_BASE && paddr < PLIC_BASE + 0x400000 {
            self.plic.write32(paddr - PLIC_BASE, value);
        } else if paddr >= VIRTIO_BASE && paddr < VIRTIO_BASE + 0x1000 {
            self.virtio9p.write32(paddr - VIRTIO_BASE, value);
        }
    }

    #[inline(always)]
    pub fn read8_fast(&mut self, paddr: u32) -> u8 {
        if paddr >= DRAM_BASE {
            self.memory.read8(paddr)
        } else {
            0 // Device access for bytes is rare
        }
    }

    #[inline(always)]
    pub fn read16_fast(&mut self, paddr: u32) -> u16 {
        if paddr >= DRAM_BASE {
            self.memory.read16(paddr)
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn write8_fast(&mut self, paddr: u32, value: u8) {
        if paddr >= DRAM_BASE {
            self.memory.write8(paddr, value);
        } else {
            self.write8_device(paddr, value);
        }
    }

    #[cold]
    fn write8_device(&mut self, paddr: u32, value: u8) {
        const UART_BASE: u32 = 0x0300_0000;
        if paddr >= UART_BASE && paddr < UART_BASE + 0x1000 {
            self.uart.write8(paddr - UART_BASE, value);
        }
    }

    #[inline(always)]
    pub fn write16_fast(&mut self, paddr: u32, value: u16) {
        if paddr >= DRAM_BASE {
            self.memory.write16(paddr, value);
        }
    }
}

/// Run the CPU with giant inline switch
/// Returns number of cycles executed
pub fn run_fast(
    cpu: &mut Cpu,
    memory: &mut Memory,
    uart: &mut Uart,
    clint: &mut Clint,
    plic: &mut Plic,
    virtio9p: &mut Virtio9p,
    max_cycles: u32,
) -> u32 {
    // Destructure hot state into locals
    let mut pc = cpu.pc;
    let mut regs = cpu.regs;
    
    let mut cycles = 0u32;
    const TIMER_BATCH: u32 = 64;

    // Create fast bus
    let mut bus = FastBus { memory, uart, clint, plic, virtio9p };

    'main: while cycles < max_cycles {
        // Batched timer update
        if cycles & (TIMER_BATCH - 1) == 0 {
            bus.clint.tick(TIMER_BATCH as u64);
            cpu.csr.time = bus.clint.get_mtime();
            
            // Check UART interrupt
            if bus.uart.has_interrupt() {
                bus.plic.raise_interrupt(10); // UART_IRQ
            }
            
            // Check timer interrupt
            if bus.clint.timer_interrupt {
                cpu.csr.mip |= MIP_STIP;
            } else {
                cpu.csr.mip &= !MIP_STIP;
            }
            
            // Check for pending interrupts
            if let Some(trap) = cpu.check_interrupts() {
                // Write back and handle
                cpu.pc = pc;
                cpu.regs = regs;
                cpu.handle_trap(trap);
                pc = cpu.pc;
                regs = cpu.regs;
            }
        }

        // WFI handling
        if cpu.wfi {
            let pending = cpu.csr.mip & cpu.csr.mie;
            if pending != 0 {
                cpu.wfi = false;
            } else {
                cycles += 1;
                continue;
            }
        }

        // Inline TLB lookup for instruction fetch
        let satp = cpu.csr.satp;
        let mstatus = cpu.csr.mstatus;
        let priv_level = cpu.priv_level;

        let paddr = if priv_level == PrivilegeLevel::Machine || (satp >> 31) == 0 {
            pc // No translation in M-mode or when paging disabled
        } else {
            // Need full TLB lookup - write back and use existing method
            cpu.pc = pc;
            cpu.regs = regs;
            match cpu.mmu.translate(pc, AccessType::Instruction, priv_level, &mut SystemBusAdapter(&mut bus), satp, mstatus) {
                Ok(pa) => pa,
                Err(cause) => {
                    cpu.handle_trap(Trap::from_cause(cause, pc));
                    pc = cpu.pc;
                    regs = cpu.regs;
                    cycles += 1;
                    continue;
                }
            }
        };

        // Inline RAM read for instruction fetch
        let inst = bus.read32_fast(paddr);
        
        // Extract common fields inline
        let opcode = inst & 0x7F;
        let rd = ((inst >> 7) & 0x1F) as usize;
        let rs1_idx = ((inst >> 15) & 0x1F) as usize;
        let rs2_idx = ((inst >> 20) & 0x1F) as usize;
        let funct3 = (inst >> 12) & 0x7;
        let funct7 = (inst >> 25) & 0x7F;

        // Giant inline switch
        match opcode {
            OP_LUI => {
                let imm = imm_u(inst);
                if rd != 0 { regs[rd] = imm; }
                pc = pc.wrapping_add(4);
            }

            OP_AUIPC => {
                let imm = imm_u(inst);
                if rd != 0 { regs[rd] = pc.wrapping_add(imm); }
                pc = pc.wrapping_add(4);
            }

            OP_JAL => {
                let imm = imm_j(inst) as u32;
                if rd != 0 { regs[rd] = pc.wrapping_add(4); }
                pc = pc.wrapping_add(imm);
            }

            OP_JALR => {
                let imm = imm_i(inst) as u32;
                let target = (regs[rs1_idx].wrapping_add(imm)) & !1;
                if rd != 0 { regs[rd] = pc.wrapping_add(4); }
                pc = target;
            }

            OP_BRANCH => {
                let rs1 = regs[rs1_idx];
                let rs2 = regs[rs2_idx];
                let imm = imm_b(inst) as u32;
                
                let taken = match funct3 {
                    0b000 => rs1 == rs2, // BEQ
                    0b001 => rs1 != rs2, // BNE
                    0b100 => (rs1 as i32) < (rs2 as i32), // BLT
                    0b101 => (rs1 as i32) >= (rs2 as i32), // BGE
                    0b110 => rs1 < rs2, // BLTU
                    0b111 => rs1 >= rs2, // BGEU
                    _ => {
                        cpu.pc = pc;
                        cpu.regs = regs;
                        cpu.handle_trap(Trap::IllegalInstruction(inst));
                        pc = cpu.pc;
                        regs = cpu.regs;
                        cycles += 1;
                        continue;
                    }
                };
                
                if taken {
                    pc = pc.wrapping_add(imm);
                } else {
                    pc = pc.wrapping_add(4);
                }
            }

            OP_LOAD => {
                let imm = imm_i(inst) as u32;
                let vaddr = regs[rs1_idx].wrapping_add(imm);
                
                // Translate
                let load_paddr = if priv_level == PrivilegeLevel::Machine || (satp >> 31) == 0 {
                    vaddr
                } else {
                    cpu.pc = pc;
                    cpu.regs = regs;
                    match cpu.mmu.translate(vaddr, AccessType::Load, priv_level, &mut SystemBusAdapter(&mut bus), satp, mstatus) {
                        Ok(pa) => pa,
                        Err(cause) => {
                            cpu.handle_trap(Trap::from_cause(cause, vaddr));
                            pc = cpu.pc;
                            regs = cpu.regs;
                            cycles += 1;
                            continue;
                        }
                    }
                };
                
                let value = match funct3 {
                    0b000 => bus.read8_fast(load_paddr) as i8 as i32 as u32, // LB
                    0b001 => bus.read16_fast(load_paddr) as i16 as i32 as u32, // LH
                    0b010 => bus.read32_fast(load_paddr), // LW
                    0b100 => bus.read8_fast(load_paddr) as u32, // LBU
                    0b101 => bus.read16_fast(load_paddr) as u32, // LHU
                    _ => {
                        cpu.pc = pc;
                        cpu.regs = regs;
                        cpu.handle_trap(Trap::IllegalInstruction(inst));
                        pc = cpu.pc;
                        regs = cpu.regs;
                        cycles += 1;
                        continue;
                    }
                };
                
                if rd != 0 { regs[rd] = value; }
                pc = pc.wrapping_add(4);
            }

            OP_STORE => {
                let imm = imm_s(inst) as u32;
                let vaddr = regs[rs1_idx].wrapping_add(imm);
                let value = regs[rs2_idx];
                
                // Translate
                let store_paddr = if priv_level == PrivilegeLevel::Machine || (satp >> 31) == 0 {
                    vaddr
                } else {
                    cpu.pc = pc;
                    cpu.regs = regs;
                    match cpu.mmu.translate(vaddr, AccessType::Store, priv_level, &mut SystemBusAdapter(&mut bus), satp, mstatus) {
                        Ok(pa) => pa,
                        Err(cause) => {
                            cpu.handle_trap(Trap::from_cause(cause, vaddr));
                            pc = cpu.pc;
                            regs = cpu.regs;
                            cycles += 1;
                            continue;
                        }
                    }
                };
                
                match funct3 {
                    0b000 => bus.write8_fast(store_paddr, value as u8), // SB
                    0b001 => bus.write16_fast(store_paddr, value as u16), // SH
                    0b010 => bus.write32_fast(store_paddr, value), // SW
                    _ => {
                        cpu.pc = pc;
                        cpu.regs = regs;
                        cpu.handle_trap(Trap::IllegalInstruction(inst));
                        pc = cpu.pc;
                        regs = cpu.regs;
                        cycles += 1;
                        continue;
                    }
                }
                
                pc = pc.wrapping_add(4);
            }

            OP_OP_IMM => {
                let rs1 = regs[rs1_idx];
                let imm = imm_i(inst) as u32;
                let shamt = imm & 0x1F;
                
                let result = match funct3 {
                    0b000 => rs1.wrapping_add(imm), // ADDI
                    0b010 => if (rs1 as i32) < (imm as i32) { 1 } else { 0 }, // SLTI
                    0b011 => if rs1 < imm { 1 } else { 0 }, // SLTIU
                    0b100 => rs1 ^ imm, // XORI
                    0b110 => rs1 | imm, // ORI
                    0b111 => rs1 & imm, // ANDI
                    0b001 => rs1 << shamt, // SLLI
                    0b101 => {
                        if (imm >> 10) & 1 != 0 {
                            ((rs1 as i32) >> shamt) as u32 // SRAI
                        } else {
                            rs1 >> shamt // SRLI
                        }
                    }
                    _ => {
                        cpu.pc = pc;
                        cpu.regs = regs;
                        cpu.handle_trap(Trap::IllegalInstruction(inst));
                        pc = cpu.pc;
                        regs = cpu.regs;
                        cycles += 1;
                        continue;
                    }
                };
                
                if rd != 0 { regs[rd] = result; }
                pc = pc.wrapping_add(4);
            }

            OP_OP => {
                let rs1 = regs[rs1_idx];
                let rs2 = regs[rs2_idx];
                
                let result = if funct7 == 0b0000001 {
                    // M extension
                    match funct3 {
                        0b000 => rs1.wrapping_mul(rs2), // MUL
                        0b001 => ((rs1 as i32 as i64).wrapping_mul(rs2 as i32 as i64) >> 32) as u32, // MULH
                        0b010 => ((rs1 as i32 as i64).wrapping_mul(rs2 as u64 as i64) >> 32) as u32, // MULHSU
                        0b011 => ((rs1 as u64).wrapping_mul(rs2 as u64) >> 32) as u32, // MULHU
                        0b100 => { // DIV
                            if rs2 == 0 { 0xFFFFFFFF }
                            else if rs1 as i32 == i32::MIN && rs2 as i32 == -1 { rs1 }
                            else { ((rs1 as i32).wrapping_div(rs2 as i32)) as u32 }
                        }
                        0b101 => { // DIVU
                            if rs2 == 0 { 0xFFFFFFFF } else { rs1 / rs2 }
                        }
                        0b110 => { // REM
                            if rs2 == 0 { rs1 }
                            else if rs1 as i32 == i32::MIN && rs2 as i32 == -1 { 0 }
                            else { ((rs1 as i32).wrapping_rem(rs2 as i32)) as u32 }
                        }
                        0b111 => { // REMU
                            if rs2 == 0 { rs1 } else { rs1 % rs2 }
                        }
                        _ => {
                            cpu.pc = pc;
                            cpu.regs = regs;
                            cpu.handle_trap(Trap::IllegalInstruction(inst));
                            pc = cpu.pc;
                            regs = cpu.regs;
                            cycles += 1;
                            continue;
                        }
                    }
                } else {
                    // Base integer
                    match (funct3, funct7) {
                        (0b000, 0b0000000) => rs1.wrapping_add(rs2), // ADD
                        (0b000, 0b0100000) => rs1.wrapping_sub(rs2), // SUB
                        (0b001, 0b0000000) => rs1 << (rs2 & 0x1F), // SLL
                        (0b010, 0b0000000) => if (rs1 as i32) < (rs2 as i32) { 1 } else { 0 }, // SLT
                        (0b011, 0b0000000) => if rs1 < rs2 { 1 } else { 0 }, // SLTU
                        (0b100, 0b0000000) => rs1 ^ rs2, // XOR
                        (0b101, 0b0000000) => rs1 >> (rs2 & 0x1F), // SRL
                        (0b101, 0b0100000) => ((rs1 as i32) >> (rs2 & 0x1F)) as u32, // SRA
                        (0b110, 0b0000000) => rs1 | rs2, // OR
                        (0b111, 0b0000000) => rs1 & rs2, // AND
                        _ => {
                            cpu.pc = pc;
                            cpu.regs = regs;
                            cpu.handle_trap(Trap::IllegalInstruction(inst));
                            pc = cpu.pc;
                            regs = cpu.regs;
                            cycles += 1;
                            continue;
                        }
                    }
                };
                
                if rd != 0 { regs[rd] = result; }
                pc = pc.wrapping_add(4);
            }

            OP_MISC_MEM => {
                // FENCE instructions - mostly no-op
                if funct3 == 1 {
                    // FENCE.I - invalidate caches
                    cpu.icache.invalidate_all();
                    cpu.cache_invalidation_pending = true;
                }
                pc = pc.wrapping_add(4);
            }

            // Complex instructions - fallback to step
            OP_SYSTEM | OP_AMO | OP_LOAD_FP | OP_STORE_FP | OP_MADD | OP_MSUB | OP_NMSUB | OP_NMADD | OP_OP_FP => {
                // Write back state and use slow path
                cpu.pc = pc;
                cpu.regs = regs;
                
                let result = cpu.step(&mut SystemBusAdapter(&mut bus));
                
                pc = cpu.pc;
                regs = cpu.regs;
                
                if let Err(trap) = result {
                    // Check for SBI call - handle common ones inline
                    if matches!(trap, Trap::EnvironmentCallFromS) {
                        // Handle SBI calls inline for speed
                        let eid = regs[17]; // a7 = Extension ID
                        let a0 = regs[10];
                        let a1 = regs[11];
                        
                        let (error, value) = match eid {
                            0 => { // SBI_EXT_LEGACY_SET_TIMER
                                bus.clint.write32(0x4000, a0);      // mtimecmp low
                                bus.clint.write32(0x4004, a1);      // mtimecmp high
                                cpu.csr.mip &= !MIP_STIP;            // Clear pending timer
                                (0u32, 0u32)
                            }
                            1 => { // SBI_EXT_LEGACY_CONSOLE_PUTCHAR
                                bus.uart.write8(0, a0 as u8);
                                (0u32, 0u32)
                            }
                            2 => { // SBI_EXT_LEGACY_CONSOLE_GETCHAR
                                ((-1i32) as u32, 0u32)
                            }
                            _ => {
                                // Unknown SBI - return to System to handle
                                cpu.pc = pc;
                                cpu.regs = regs;
                                cycles += 1;
                                cpu.instruction_count += 1;
                                cpu.csr.cycle = cpu.csr.cycle.wrapping_add(1);
                                cpu.pending_sbi_call = true;
                                break;
                            }
                        };
                        
                        // Set return values (a0=error, a1=value)
                        if 10 != 0 { regs[10] = error; }
                        if 11 != 0 { regs[11] = value; }
                        // Advance PC past ecall
                        pc = pc.wrapping_add(4);
                    } else {
                        cpu.handle_trap(trap);
                        pc = cpu.pc;
                        regs = cpu.regs;
                    }
                }
            }

            _ => {
                // Unknown opcode
                cpu.pc = pc;
                cpu.regs = regs;
                cpu.handle_trap(Trap::IllegalInstruction(inst));
                pc = cpu.pc;
                regs = cpu.regs;
            }
        }

        cycles += 1;
        cpu.instruction_count += 1;
        cpu.csr.cycle = cpu.csr.cycle.wrapping_add(1);
    }

    // Write back final state
    cpu.pc = pc;
    cpu.regs = regs;

    // Process VirtIO queues
    bus.virtio9p.process_queues(bus.memory);

    cycles
}

/// Adapter to make FastBus work with Bus trait for MMU
struct SystemBusAdapter<'a, 'b>(&'a mut FastBus<'b>);

impl<'a, 'b> Bus for SystemBusAdapter<'a, 'b> {
    fn read8(&mut self, addr: u32) -> u8 { self.0.read8_fast(addr) }
    fn read16(&mut self, addr: u32) -> u16 { self.0.read16_fast(addr) }
    fn read32(&mut self, addr: u32) -> u32 { self.0.read32_fast(addr) }
    fn read64(&mut self, addr: u32) -> u64 {
        let lo = self.0.read32_fast(addr) as u64;
        let hi = self.0.read32_fast(addr.wrapping_add(4)) as u64;
        (hi << 32) | lo
    }
    fn write8(&mut self, addr: u32, val: u8) { self.0.write8_fast(addr, val) }
    fn write16(&mut self, addr: u32, val: u16) { self.0.write16_fast(addr, val) }
    fn write32(&mut self, addr: u32, val: u32) { self.0.write32_fast(addr, val) }
    fn write64(&mut self, addr: u32, val: u64) {
        self.0.write32_fast(addr, val as u32);
        self.0.write32_fast(addr.wrapping_add(4), (val >> 32) as u32);
    }
}
