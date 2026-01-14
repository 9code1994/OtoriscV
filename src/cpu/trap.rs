//! Trap handling (exceptions and interrupts)
//!
//! Based on jor1k's safecpu.js trap implementation

use super::{Cpu, PrivilegeLevel};
use super::csr::*;

/// Exception/interrupt cause
#[derive(Debug, Clone, Copy)]
pub enum Trap {
    // Exceptions (synchronous)
    InstructionAddressMisaligned(u32),
    InstructionAccessFault(u32),
    IllegalInstruction(u32),
    Breakpoint(u32),
    LoadAddressMisaligned(u32),
    LoadAccessFault(u32),
    StoreAddressMisaligned(u32),
    StoreAccessFault(u32),
    EnvironmentCallFromU,
    EnvironmentCallFromS,
    EnvironmentCallFromM,
    InstructionPageFault(u32),
    LoadPageFault(u32),
    StorePageFault(u32),
    
    // Interrupts (asynchronous)
    UserSoftwareInterrupt,
    SupervisorSoftwareInterrupt,
    MachineSoftwareInterrupt,
    UserTimerInterrupt,
    SupervisorTimerInterrupt,
    MachineTimerInterrupt,
    UserExternalInterrupt,
    SupervisorExternalInterrupt,
    MachineExternalInterrupt,
}

impl Trap {
    /// Get the cause code for mcause/scause
    pub fn code(&self) -> u32 {
        match self {
            // Exceptions
            Trap::InstructionAddressMisaligned(_) => 0,
            Trap::InstructionAccessFault(_) => 1,
            Trap::IllegalInstruction(_) => 2,
            Trap::Breakpoint(_) => 3,
            Trap::LoadAddressMisaligned(_) => 4,
            Trap::LoadAccessFault(_) => 5,
            Trap::StoreAddressMisaligned(_) => 6,
            Trap::StoreAccessFault(_) => 7,
            Trap::EnvironmentCallFromU => 8,
            Trap::EnvironmentCallFromS => 9,
            Trap::EnvironmentCallFromM => 11,
            Trap::InstructionPageFault(_) => 12,
            Trap::LoadPageFault(_) => 13,
            Trap::StorePageFault(_) => 15,
            
            // Interrupts (bit 31 set)
            Trap::UserSoftwareInterrupt => 0x80000000 | 0,
            Trap::SupervisorSoftwareInterrupt => 0x80000000 | 1,
            Trap::MachineSoftwareInterrupt => 0x80000000 | 3,
            Trap::UserTimerInterrupt => 0x80000000 | 4,
            Trap::SupervisorTimerInterrupt => 0x80000000 | 5,
            Trap::MachineTimerInterrupt => 0x80000000 | 7,
            Trap::UserExternalInterrupt => 0x80000000 | 8,
            Trap::SupervisorExternalInterrupt => 0x80000000 | 9,
            Trap::MachineExternalInterrupt => 0x80000000 | 11,
        }
    }
    
    /// Get the trap value (bad address, instruction, etc)
    pub fn value(&self) -> u32 {
        match self {
            Trap::InstructionAddressMisaligned(v) |
            Trap::InstructionAccessFault(v) |
            Trap::IllegalInstruction(v) |
            Trap::Breakpoint(v) |
            Trap::LoadAddressMisaligned(v) |
            Trap::LoadAccessFault(v) |
            Trap::StoreAddressMisaligned(v) |
            Trap::StoreAccessFault(v) |
            Trap::InstructionPageFault(v) |
            Trap::LoadPageFault(v) |
            Trap::StorePageFault(v) => *v,
            _ => 0,
        }
    }
    
    /// Is this an interrupt (vs exception)?
    pub fn is_interrupt(&self) -> bool {
        (self.code() & 0x80000000) != 0
    }
    
    /// Create trap from cause code and value
    pub fn from_cause(cause: u32, tval: u32) -> Self {
        match cause {
            0 => Trap::InstructionAddressMisaligned(tval),
            1 => Trap::InstructionAccessFault(tval),
            2 => Trap::IllegalInstruction(tval),
            3 => Trap::Breakpoint(tval),
            4 => Trap::LoadAddressMisaligned(tval),
            5 => Trap::LoadAccessFault(tval),
            6 => Trap::StoreAddressMisaligned(tval),
            7 => Trap::StoreAccessFault(tval),
            12 => Trap::InstructionPageFault(tval),
            13 => Trap::LoadPageFault(tval),
            15 => Trap::StorePageFault(tval),
            _ => Trap::IllegalInstruction(tval), // Fallback
        }
    }
}

/// Check for pending interrupts
pub fn check_pending_interrupts(cpu: &Cpu) -> Option<Trap> {
    let pending = cpu.csr.mip & cpu.csr.mie;
    if pending == 0 {
        return None;
    }
    
    // Check if interrupts can be taken at current privilege level
    let mie_enabled = (cpu.csr.mstatus & MSTATUS_MIE) != 0;
    let sie_enabled = (cpu.csr.mstatus & MSTATUS_SIE) != 0;
    
    let m_enabled = cpu.priv_level < PrivilegeLevel::Machine || 
                    (cpu.priv_level == PrivilegeLevel::Machine && mie_enabled);
    let s_enabled = cpu.priv_level < PrivilegeLevel::Supervisor || 
                    (cpu.priv_level == PrivilegeLevel::Supervisor && sie_enabled);
    
    // Check M-mode interrupts first (higher priority)
    let m_interrupts = pending & !cpu.csr.mideleg;
    if m_enabled && m_interrupts != 0 {
        // Priority: MEI > MSI > MTI > SEI > SSI > STI
        if m_interrupts & MIP_MEIP != 0 {
            return Some(Trap::MachineExternalInterrupt);
        }
        if m_interrupts & MIP_MSIP != 0 {
            return Some(Trap::MachineSoftwareInterrupt);
        }
        if m_interrupts & MIP_MTIP != 0 {
            return Some(Trap::MachineTimerInterrupt);
        }
    }
    
    // Check S-mode interrupts
    let s_interrupts = pending & cpu.csr.mideleg;
    if s_enabled && s_interrupts != 0 {
        if s_interrupts & MIP_SEIP != 0 {
            return Some(Trap::SupervisorExternalInterrupt);
        }
        if s_interrupts & MIP_SSIP != 0 {
            return Some(Trap::SupervisorSoftwareInterrupt);
        }
        if s_interrupts & MIP_STIP != 0 {
            return Some(Trap::SupervisorTimerInterrupt);
        }
    }
    
    None
}

/// Handle a trap (exception or interrupt)
pub fn handle_trap(cpu: &mut Cpu, trap: Trap) {
    let cause = trap.code();
    let tval = trap.value();
    let is_interrupt = trap.is_interrupt();
    
    // Determine if trap should be delegated to S-mode
    let deleg = if is_interrupt {
        cpu.csr.mideleg
    } else {
        cpu.csr.medeleg
    };
    
    let bit = cause & 0x7FFFFFFF;
    let delegate_to_s = cpu.priv_level <= PrivilegeLevel::Supervisor && 
                        bit < 32 && 
                        (deleg & (1 << bit)) != 0;
    
    if delegate_to_s {
        // Trap to S-mode
        cpu.csr.sepc = cpu.pc;
        cpu.csr.scause = cause;
        cpu.csr.stval = tval;
        
        // Update sstatus
        let mut status = cpu.csr.mstatus;
        
        // Set SPIE = SIE
        if (status & MSTATUS_SIE) != 0 {
            status |= MSTATUS_SPIE;
        } else {
            status &= !MSTATUS_SPIE;
        }
        
        // Set SPP = current privilege
        if cpu.priv_level == PrivilegeLevel::Supervisor {
            status |= MSTATUS_SPP;
        } else {
            status &= !MSTATUS_SPP;
        }
        
        // Clear SIE
        status &= !MSTATUS_SIE;
        
        cpu.csr.mstatus = status;
        cpu.priv_level = PrivilegeLevel::Supervisor;
        
        // Jump to stvec
        let vector = if is_interrupt && (cpu.csr.stvec & 1) != 0 {
            (cpu.csr.stvec & !1) + (bit * 4)
        } else {
            cpu.csr.stvec & !1
        };
        cpu.pc = vector;
    } else {
        // Trap to M-mode
        cpu.csr.mepc = cpu.pc;
        cpu.csr.mcause = cause;
        cpu.csr.mtval = tval;
        
        // Update mstatus
        let mut status = cpu.csr.mstatus;
        
        // Set MPIE = MIE
        if (status & MSTATUS_MIE) != 0 {
            status |= MSTATUS_MPIE;
        } else {
            status &= !MSTATUS_MPIE;
        }
        
        // Set MPP = current privilege
        status = (status & !MSTATUS_MPP) | ((cpu.priv_level as u32) << 11);
        
        // Clear MIE
        status &= !MSTATUS_MIE;
        
        cpu.csr.mstatus = status;
        cpu.priv_level = PrivilegeLevel::Machine;
        
        // Jump to mtvec
        let vector = if is_interrupt && (cpu.csr.mtvec & 1) != 0 {
            (cpu.csr.mtvec & !1) + (bit * 4)
        } else {
            cpu.csr.mtvec & !1
        };
        cpu.pc = vector;
    }
    
    // Clear WFI on interrupt
    cpu.wfi = false;
}

/// Handle MRET instruction
pub fn mret(cpu: &mut Cpu) {
    // Restore privilege from MPP
    let mpp = (cpu.csr.mstatus >> 11) & 3;
    cpu.priv_level = PrivilegeLevel::from(mpp as u8);
    
    // Restore MIE from MPIE
    let mut status = cpu.csr.mstatus;
    if (status & MSTATUS_MPIE) != 0 {
        status |= MSTATUS_MIE;
    } else {
        status &= !MSTATUS_MIE;
    }
    
    // Set MPIE = 1
    status |= MSTATUS_MPIE;
    
    // Set MPP = U (or M if U not supported)
    status &= !MSTATUS_MPP;
    
    cpu.csr.mstatus = status;
    
    // Jump to mepc
    cpu.pc = cpu.csr.mepc;
}

/// Handle SRET instruction
pub fn sret(cpu: &mut Cpu) {
    // Restore privilege from SPP
    let spp = (cpu.csr.mstatus >> 8) & 1;
    let old_priv = cpu.priv_level;
    cpu.priv_level = if spp == 1 { 
        PrivilegeLevel::Supervisor 
    } else { 
        PrivilegeLevel::User 
    };
    
    // Restore SIE from SPIE
    let mut status = cpu.csr.mstatus;
    if (status & MSTATUS_SPIE) != 0 {
        status |= MSTATUS_SIE;
    } else {
        status &= !MSTATUS_SIE;
    }
    
    // Set SPIE = 1
    status |= MSTATUS_SPIE;
    
    // Set SPP = U
    status &= !MSTATUS_SPP;
    
    cpu.csr.mstatus = status;
    
    // Jump to sepc
    cpu.pc = cpu.csr.sepc;
}
