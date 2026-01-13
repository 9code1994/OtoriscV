//! RV32IMA CPU module
//!
//! Implements the RISC-V 32-bit base integer instruction set
//! with M (multiply/divide) and A (atomic) extensions

pub mod csr;
mod decode;
mod execute;
pub mod trap;

pub mod mmu;
pub use csr::Csr;
pub use mmu::Mmu;

use crate::memory::Bus;
use serde::{Serialize, Deserialize};

/// Privilege levels
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum PrivilegeLevel {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

impl From<u8> for PrivilegeLevel {
    fn from(val: u8) -> Self {
        match val & 3 {
            0 => PrivilegeLevel::User,
            1 => PrivilegeLevel::Supervisor,
            3 => PrivilegeLevel::Machine,
            _ => PrivilegeLevel::Machine,
        }
    }
}

/// CPU state
#[derive(Serialize, Deserialize)]
pub struct Cpu {
    /// Program counter
    pub pc: u32,
    /// General purpose registers (x0-x31)
    pub regs: [u32; 32],
    /// Control and Status Registers
    pub csr: Csr,
    /// Current privilege level
    pub priv_level: PrivilegeLevel,
    
    /// Wait for interrupt (WFI executed)
    pub wfi: bool,
    
    /// Reservation set for LR/SC (address, valid)
    pub reservation: Option<u32>,
    
    /// Instruction counter for performance
    pub instruction_count: u64,
    
    /// MMU for address translation
    #[serde(skip)] // TODO: serialize MMU state
    pub mmu: Mmu,

    // Debugging helpers
    pub last_write_addr: u32,
    pub last_write_val: u32,
}

impl Cpu {
    pub fn new() -> Self {
        let mut cpu = Cpu {
            pc: 0x0000_1000, // Boot ROM address
            regs: [0u32; 32],
            csr: Csr::new(),
            priv_level: PrivilegeLevel::Machine,
            wfi: false,
            reservation: None,
            instruction_count: 0,
            mmu: Mmu::new(),
            last_write_addr: 0,
            last_write_val: 0,
        };
        
        // x0 is always 0
        cpu.regs[0] = 0;
        
        cpu
    }
    
    /// Read register (x0 always returns 0)
    #[inline(always)]
    pub fn read_reg(&self, reg: u32) -> u32 {
        if reg == 0 {
            0
        } else {
            self.regs[reg as usize & 0x1F]
        }
    }
    
    /// Write register (x0 writes are ignored)
    #[inline(always)]
    pub fn write_reg(&mut self, reg: u32, value: u32) {
        if reg != 0 {
            self.regs[reg as usize & 0x1F] = value;
        }
    }
    
    /// Execute one instruction
    pub fn step(&mut self, bus: &mut impl Bus) -> Result<(), trap::Trap> {
        // Fetch instruction with translation
        let satp = self.csr.satp;
        let mstatus = self.csr.mstatus;
        let priv_level = self.priv_level;
        
        // TODO: AccessType::Instruction should use executable permission check
        let paddr = match self.mmu.translate(self.pc, mmu::AccessType::Instruction, priv_level, bus, satp, mstatus) {
            Ok(pa) => pa,
            Err(cause) => {
                return Err(trap::Trap::from_cause(cause, self.pc));
            }
        };
        
        let inst = bus.read32(paddr);
        
        // Decode and execute
        self.execute(inst, bus)?;
        
        self.instruction_count += 1;
        
        Ok(())
    }
    
    /// Reset CPU state
    pub fn reset(&mut self) {
        self.pc = 0x0000_1000;
        self.regs = [0u32; 32];
        self.csr.reset();
        self.priv_level = PrivilegeLevel::Machine;
        self.wfi = false;
        self.reservation = None;
    }
    
    /// Check for pending interrupts and handle if any
    pub fn check_interrupts(&mut self) -> Option<trap::Trap> {
        trap::check_pending_interrupts(self)
    }
    
    /// Handle a trap (exception or interrupt)
    pub fn handle_trap(&mut self, trap: trap::Trap) {
        trap::handle_trap(self, trap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cpu_creation() {
        let cpu = Cpu::new();
        assert_eq!(cpu.pc, 0x1000);
        assert_eq!(cpu.read_reg(0), 0);
        assert_eq!(cpu.priv_level, PrivilegeLevel::Machine);
    }
    
    #[test]
    fn test_x0_always_zero() {
        let mut cpu = Cpu::new();
        cpu.write_reg(0, 0xDEADBEEF);
        assert_eq!(cpu.read_reg(0), 0);
        
        cpu.write_reg(1, 0x12345678);
        assert_eq!(cpu.read_reg(1), 0x12345678);
    }
}
