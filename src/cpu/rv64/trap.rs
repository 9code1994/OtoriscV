//! Trap handling for RV64

use super::csr::*;
use crate::cpu::PrivilegeLevel;
use super::Cpu64;

/// Exception/interrupt cause
#[derive(Debug, Clone, Copy)]
pub enum Trap64 {
    // Exceptions
    InstructionAddressMisaligned(u64),
    InstructionAccessFault(u64),
    IllegalInstruction(u64),
    Breakpoint(u64),
    LoadAddressMisaligned(u64),
    LoadAccessFault(u64),
    StoreAddressMisaligned(u64),
    StoreAccessFault(u64),
    EnvironmentCallFromU,
    EnvironmentCallFromS,
    EnvironmentCallFromM,
    InstructionPageFault(u64),
    LoadPageFault(u64),
    StorePageFault(u64),

    // Interrupts
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

impl Trap64 {
    pub fn code(&self) -> u64 {
        match self {
            Trap64::InstructionAddressMisaligned(_) => 0,
            Trap64::InstructionAccessFault(_) => 1,
            Trap64::IllegalInstruction(_) => 2,
            Trap64::Breakpoint(_) => 3,
            Trap64::LoadAddressMisaligned(_) => 4,
            Trap64::LoadAccessFault(_) => 5,
            Trap64::StoreAddressMisaligned(_) => 6,
            Trap64::StoreAccessFault(_) => 7,
            Trap64::EnvironmentCallFromU => 8,
            Trap64::EnvironmentCallFromS => 9,
            Trap64::EnvironmentCallFromM => 11,
            Trap64::InstructionPageFault(_) => 12,
            Trap64::LoadPageFault(_) => 13,
            Trap64::StorePageFault(_) => 15,

            Trap64::UserSoftwareInterrupt => (1u64 << 63) | 0,
            Trap64::SupervisorSoftwareInterrupt => (1u64 << 63) | 1,
            Trap64::MachineSoftwareInterrupt => (1u64 << 63) | 3,
            Trap64::UserTimerInterrupt => (1u64 << 63) | 4,
            Trap64::SupervisorTimerInterrupt => (1u64 << 63) | 5,
            Trap64::MachineTimerInterrupt => (1u64 << 63) | 7,
            Trap64::UserExternalInterrupt => (1u64 << 63) | 8,
            Trap64::SupervisorExternalInterrupt => (1u64 << 63) | 9,
            Trap64::MachineExternalInterrupt => (1u64 << 63) | 11,
        }
    }

    pub fn value(&self) -> u64 {
        match self {
            Trap64::InstructionAddressMisaligned(v) |
            Trap64::InstructionAccessFault(v) |
            Trap64::IllegalInstruction(v) |
            Trap64::Breakpoint(v) |
            Trap64::LoadAddressMisaligned(v) |
            Trap64::LoadAccessFault(v) |
            Trap64::StoreAddressMisaligned(v) |
            Trap64::StoreAccessFault(v) |
            Trap64::InstructionPageFault(v) |
            Trap64::LoadPageFault(v) |
            Trap64::StorePageFault(v) => *v,
            _ => 0,
        }
    }

    pub fn is_interrupt(&self) -> bool {
        (self.code() & (1u64 << 63)) != 0
    }

    pub fn from_cause(cause: u64, tval: u64) -> Self {
        match cause {
            0 => Trap64::InstructionAddressMisaligned(tval),
            1 => Trap64::InstructionAccessFault(tval),
            2 => Trap64::IllegalInstruction(tval),
            3 => Trap64::Breakpoint(tval),
            4 => Trap64::LoadAddressMisaligned(tval),
            5 => Trap64::LoadAccessFault(tval),
            6 => Trap64::StoreAddressMisaligned(tval),
            7 => Trap64::StoreAccessFault(tval),
            12 => Trap64::InstructionPageFault(tval),
            13 => Trap64::LoadPageFault(tval),
            15 => Trap64::StorePageFault(tval),
            _ => Trap64::IllegalInstruction(tval),
        }
    }
}

pub fn check_pending_interrupts(cpu: &Cpu64) -> Option<Trap64> {
    let pending = cpu.csr.mip & cpu.csr.mie;
    if pending == 0 {
        return None;
    }

    let mie_enabled = (cpu.csr.mstatus & MSTATUS_MIE) != 0;
    let sie_enabled = (cpu.csr.mstatus & MSTATUS_SIE) != 0;

    let m_enabled = cpu.priv_level < PrivilegeLevel::Machine ||
        (cpu.priv_level == PrivilegeLevel::Machine && mie_enabled);
    let s_enabled = cpu.priv_level < PrivilegeLevel::Supervisor ||
        (cpu.priv_level == PrivilegeLevel::Supervisor && sie_enabled);

    let m_interrupts = pending & !cpu.csr.mideleg;
    if m_enabled && m_interrupts != 0 {
        if m_interrupts & MIP_MEIP != 0 {
            return Some(Trap64::MachineExternalInterrupt);
        }
        if m_interrupts & MIP_MSIP != 0 {
            return Some(Trap64::MachineSoftwareInterrupt);
        }
        if m_interrupts & MIP_MTIP != 0 {
            return Some(Trap64::MachineTimerInterrupt);
        }
    }

    let s_interrupts = pending & cpu.csr.mideleg;
    if s_enabled && s_interrupts != 0 {
        if s_interrupts & MIP_SEIP != 0 {
            return Some(Trap64::SupervisorExternalInterrupt);
        }
        if s_interrupts & MIP_SSIP != 0 {
            return Some(Trap64::SupervisorSoftwareInterrupt);
        }
        if s_interrupts & MIP_STIP != 0 {
            return Some(Trap64::SupervisorTimerInterrupt);
        }
    }

    None
}

pub fn handle_trap(cpu: &mut Cpu64, trap: Trap64) {
    let cause = trap.code();
    let tval = trap.value();
    let is_interrupt = trap.is_interrupt();

    let deleg = if is_interrupt { cpu.csr.mideleg } else { cpu.csr.medeleg };
    let bit = cause & 0x7FFF_FFFF_FFFF_FFFF;
    let delegate_to_s = cpu.priv_level <= PrivilegeLevel::Supervisor &&
        bit < 64 &&
        (deleg & (1u64 << bit)) != 0;

    if delegate_to_s {
        cpu.csr.sepc = cpu.pc;
        cpu.csr.scause = cause;
        cpu.csr.stval = tval;

        let mut status = cpu.csr.mstatus;
        if (status & MSTATUS_SIE) != 0 {
            status |= MSTATUS_SPIE;
        } else {
            status &= !MSTATUS_SPIE;
        }
        if cpu.priv_level == PrivilegeLevel::Supervisor {
            status |= MSTATUS_SPP;
        } else {
            status &= !MSTATUS_SPP;
        }
        status &= !MSTATUS_SIE;
        cpu.csr.mstatus = status;

        cpu.priv_level = PrivilegeLevel::Supervisor;
        cpu.pc = cpu.csr.stvec;
        return;
    }

    cpu.csr.mepc = cpu.pc;
    cpu.csr.mcause = cause;
    cpu.csr.mtval = tval;

    let mut status = cpu.csr.mstatus;
    if (status & MSTATUS_MIE) != 0 {
        status |= MSTATUS_MPIE;
    } else {
        status &= !MSTATUS_MPIE;
    }
    status = (status & !MSTATUS_MPP) | ((cpu.priv_level as u64) << 11);
    status &= !MSTATUS_MIE;
    cpu.csr.mstatus = status;

    cpu.priv_level = PrivilegeLevel::Machine;
    cpu.pc = cpu.csr.mtvec;
}

pub fn mret(cpu: &mut Cpu64) {
    let mpp = (cpu.csr.mstatus >> 11) & 3;
    cpu.priv_level = PrivilegeLevel::from(mpp as u8);

    let mut status = cpu.csr.mstatus;
    if (status & MSTATUS_MPIE) != 0 {
        status |= MSTATUS_MIE;
    } else {
        status &= !MSTATUS_MIE;
    }
    status |= MSTATUS_MPIE;
    status &= !MSTATUS_MPP;
    cpu.csr.mstatus = status;
    cpu.pc = cpu.csr.mepc;
}

pub fn sret(cpu: &mut Cpu64) {
    let spp = (cpu.csr.mstatus >> 8) & 1;
    cpu.priv_level = if spp == 1 { PrivilegeLevel::Supervisor } else { PrivilegeLevel::User };

    let mut status = cpu.csr.mstatus;
    if (status & MSTATUS_SPIE) != 0 {
        status |= MSTATUS_SIE;
    } else {
        status &= !MSTATUS_SIE;
    }
    status |= MSTATUS_SPIE;
    status &= !MSTATUS_SPP;
    cpu.csr.mstatus = status;
    cpu.pc = cpu.csr.sepc;
}
