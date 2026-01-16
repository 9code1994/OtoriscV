//! RV64IMAFDCB CPU module
//!
//! Implements the RISC-V 64-bit base integer instruction set
//! with M (multiply/divide), A (atomic), F (single-precision float),
//! D (double-precision float), C (compressed), and B (bitmanip) extensions.
//! Includes Sv39 virtual memory support.

pub mod csr;
mod decode;
mod execute;
mod execute_fp;
mod execute_c;
pub mod mmu;
pub mod trap;

pub use csr::Csr64;
pub use mmu::Mmu64;

use super::PrivilegeLevel;
use super::fpu::Fpu;
use crate::memory::Bus;
use serde::{Serialize, Deserialize};
use trap::{Trap64, check_pending_interrupts, handle_trap};

/// 64-bit CPU state
#[derive(Serialize, Deserialize)]
pub struct Cpu64 {
    /// Program counter
    pub pc: u64,
    /// General purpose registers (x0-x31)
    pub regs: [u64; 32],
    /// Floating-point unit (f0-f31 + FCSR)
    pub fpu: Fpu,
    /// Control and Status Registers
    pub csr: Csr64,
    /// Current privilege level
    pub priv_level: PrivilegeLevel,
    /// Wait for interrupt (WFI executed)
    pub wfi: bool,
    /// Reservation set for LR/SC (address, valid)
    pub reservation: Option<u64>,
    /// Instruction counter for performance
    pub instruction_count: u64,
    /// MMU for address translation
    #[serde(skip)]
    pub mmu: Mmu64,
    // Debugging helpers
    pub last_write_addr: u64,
    pub last_write_val: u64,
}

impl Cpu64 {
    pub fn new() -> Self {
        let mut cpu = Cpu64 {
            pc: 0x0000_0000_0000_1000,
            regs: [0u64; 32],
            fpu: Fpu::new(),
            csr: Csr64::new(),
            priv_level: PrivilegeLevel::Machine,
            wfi: false,
            reservation: None,
            instruction_count: 0,
            mmu: Mmu64::new(),
            last_write_addr: 0,
            last_write_val: 0,
        };
        cpu.regs[0] = 0;
        cpu
    }

    #[inline(always)]
    pub fn read_reg(&self, reg: u32) -> u64 {
        if reg == 0 { 0 } else { self.regs[reg as usize & 0x1F] }
    }

    #[inline(always)]
    pub fn write_reg(&mut self, reg: u32, value: u64) {
        if reg != 0 {
            self.regs[reg as usize & 0x1F] = value;
        }
    }

    pub fn step(&mut self, bus: &mut impl Bus) -> Result<(), Trap64> {
        let satp = self.csr.satp;
        let mstatus = self.csr.mstatus;
        let priv_level = self.priv_level;

        let paddr = match self.mmu.translate(self.pc, mmu::AccessType::Instruction, priv_level, bus, satp, mstatus) {
            Ok(pa) => pa,
            Err(cause) => return Err(Trap64::from_cause(cause, self.pc)),
        };

        let inst = self.read_inst(bus, paddr)?;
        
        // Debug instruction execution in potential infinite loops
        if std::env::var("RISCV_DEBUG").is_ok() && self.pc >= 0x800010fe && self.pc <= 0x80001110 {
            eprintln!("[CPU] PC={:#018x} inst={:#010x}", self.pc, inst);
        }
        
        if (inst & 0b11) != 0b11 {
            self.execute_compressed(inst as u16, bus)?;
        } else {
            self.execute(inst, bus)?;
        }
        self.instruction_count = self.instruction_count.wrapping_add(1);
        Ok(())
    }

    fn read_inst(&self, bus: &mut impl Bus, paddr: u64) -> Result<u32, Trap64> {
        let addr = Self::map_paddr(paddr)?;
        let inst16 = bus.read16(addr) as u32;
        if (inst16 & 0b11) != 0b11 {
            Ok(inst16)
        } else {
            Ok(bus.read32(addr))
        }
    }

    pub fn reset(&mut self) {
        self.pc = 0x0000_0000_0000_1000;
        self.regs = [0u64; 32];
        self.fpu.reset();
        self.csr.reset();
        self.priv_level = PrivilegeLevel::Machine;
        self.wfi = false;
        self.reservation = None;
        self.mmu.reset();
    }

    pub fn check_interrupts(&mut self) -> Option<Trap64> {
        check_pending_interrupts(self)
    }

    pub fn handle_trap(&mut self, trap: Trap64) {
        handle_trap(self, trap);
    }

    fn map_paddr(paddr: u64) -> Result<u32, Trap64> {
        if paddr > u32::MAX as u64 {
            return Err(Trap64::LoadAccessFault(paddr));
        }
        Ok(paddr as u32)
    }
}

impl Default for Cpu64 {
    fn default() -> Self {
        Self::new()
    }
}
