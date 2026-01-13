//! Control and Status Registers
//!
//! Based on jor1k's safecpu.js CSR implementation

use super::PrivilegeLevel;
use serde::{Serialize, Deserialize};

// CSR addresses
pub const CSR_FFLAGS: u32 = 0x001;
pub const CSR_FRM: u32 = 0x002;
pub const CSR_FCSR: u32 = 0x003;

// Supervisor CSRs
pub const CSR_SSTATUS: u32 = 0x100;
pub const CSR_SIE: u32 = 0x104;
pub const CSR_STVEC: u32 = 0x105;
pub const CSR_SCOUNTEREN: u32 = 0x106;
pub const CSR_SSCRATCH: u32 = 0x140;
pub const CSR_SEPC: u32 = 0x141;
pub const CSR_SCAUSE: u32 = 0x142;
pub const CSR_STVAL: u32 = 0x143;
pub const CSR_SIP: u32 = 0x144;
pub const CSR_SATP: u32 = 0x180;

// Machine CSRs
pub const CSR_MSTATUS: u32 = 0x300;
pub const CSR_MISA: u32 = 0x301;
pub const CSR_MEDELEG: u32 = 0x302;
pub const CSR_MIDELEG: u32 = 0x303;
pub const CSR_MIE: u32 = 0x304;
pub const CSR_MTVEC: u32 = 0x305;
pub const CSR_MCOUNTEREN: u32 = 0x306;
pub const CSR_MSCRATCH: u32 = 0x340;
pub const CSR_MEPC: u32 = 0x341;
pub const CSR_MCAUSE: u32 = 0x342;
pub const CSR_MTVAL: u32 = 0x343;
pub const CSR_MIP: u32 = 0x344;
pub const CSR_MHARTID: u32 = 0xF14;

// Time CSRs
pub const CSR_CYCLE: u32 = 0xC00;
pub const CSR_TIME: u32 = 0xC01;
pub const CSR_INSTRET: u32 = 0xC02;
pub const CSR_CYCLEH: u32 = 0xC80;
pub const CSR_TIMEH: u32 = 0xC81;
pub const CSR_INSTRETH: u32 = 0xC82;

// MSTATUS bits
pub const MSTATUS_UIE: u32 = 1 << 0;
pub const MSTATUS_SIE: u32 = 1 << 1;
pub const MSTATUS_MIE: u32 = 1 << 3;
pub const MSTATUS_UPIE: u32 = 1 << 4;
pub const MSTATUS_SPIE: u32 = 1 << 5;
pub const MSTATUS_MPIE: u32 = 1 << 7;
pub const MSTATUS_SPP: u32 = 1 << 8;
pub const MSTATUS_MPP: u32 = 3 << 11;
pub const MSTATUS_FS: u32 = 3 << 13;
pub const MSTATUS_XS: u32 = 3 << 15;
pub const MSTATUS_MPRV: u32 = 1 << 17;
pub const MSTATUS_SUM: u32 = 1 << 18;
pub const MSTATUS_MXR: u32 = 1 << 19;
pub const MSTATUS_TVM: u32 = 1 << 20;
pub const MSTATUS_TW: u32 = 1 << 21;
pub const MSTATUS_TSR: u32 = 1 << 22;
pub const MSTATUS_SD: u32 = 1 << 31;

// MIP/MIE bits (interrupt pending/enable)
pub const MIP_SSIP: u32 = 1 << 1;  // Supervisor software interrupt
pub const MIP_MSIP: u32 = 1 << 3;  // Machine software interrupt
pub const MIP_STIP: u32 = 1 << 5;  // Supervisor timer interrupt
pub const MIP_MTIP: u32 = 1 << 7;  // Machine timer interrupt
pub const MIP_SEIP: u32 = 1 << 9;  // Supervisor external interrupt
pub const MIP_MEIP: u32 = 1 << 11; // Machine external interrupt

/// CSR storage
#[derive(Serialize, Deserialize)]
pub struct Csr {
    /// MSTATUS register
    pub mstatus: u32,
    /// MISA - ISA and extensions
    pub misa: u32,
    /// Machine exception delegation
    pub medeleg: u32,
    /// Machine interrupt delegation
    pub mideleg: u32,
    /// Machine interrupt enable
    pub mie: u32,
    /// Machine trap vector
    pub mtvec: u32,
    /// Machine counter enable
    pub mcounteren: u32,
    /// Machine scratch
    pub mscratch: u32,
    /// Machine exception PC
    pub mepc: u32,
    /// Machine cause
    pub mcause: u32,
    /// Machine trap value
    pub mtval: u32,
    /// Machine interrupt pending
    pub mip: u32,
    
    /// Supervisor trap vector
    pub stvec: u32,
    /// Supervisor counter enable
    pub scounteren: u32,
    /// Supervisor scratch
    pub sscratch: u32,
    /// Supervisor exception PC
    pub sepc: u32,
    /// Supervisor cause
    pub scause: u32,
    /// Supervisor trap value
    pub stval: u32,
    /// Supervisor address translation and protection
    pub satp: u32,
    
    /// Cycle counter
    pub cycle: u64,
    /// Timer value (mtime)
    pub time: u64,
}

impl Csr {
    pub fn new() -> Self {
        Csr {
            // MISA: RV32IMA (I=8, M=12, A=0, 32-bit=1<<30)
            misa: (1 << 30) | (1 << 8) | (1 << 12) | (1 << 0) | (1 << 18), // I M A S
            mstatus: 0,
            medeleg: 0,
            mideleg: 0,
            mie: 0,
            mtvec: 0,
            mcounteren: 0,
            mscratch: 0,
            mepc: 0,
            mcause: 0,
            mtval: 0,
            mip: 0,
            
            stvec: 0,
            scounteren: 0,
            sscratch: 0,
            sepc: 0,
            scause: 0,
            stval: 0,
            satp: 0,
            
            cycle: 0,
            time: 0,
        }
    }
    
    /// Read CSR (returns None if invalid/inaccessible)
    pub fn read(&self, addr: u32, priv_level: PrivilegeLevel) -> Option<u32> {
        // Check privilege level (CSR[9:8] encodes minimum privilege)
        let min_priv = ((addr >> 8) & 3) as u8;
        if (priv_level as u8) < min_priv {
            return None;
        }
        
        Some(match addr {
            CSR_MSTATUS => self.mstatus,
            CSR_MISA => self.misa,
            CSR_MEDELEG => self.medeleg,
            CSR_MIDELEG => self.mideleg,
            CSR_MIE => self.mie,
            CSR_MTVEC => self.mtvec,
            CSR_MCOUNTEREN => self.mcounteren,
            CSR_MSCRATCH => self.mscratch,
            CSR_MEPC => self.mepc,
            CSR_MCAUSE => self.mcause,
            CSR_MTVAL => self.mtval,
            CSR_MIP => self.mip,
            CSR_MHARTID => 0, // Hart ID is always 0
            
            CSR_SSTATUS => self.mstatus & (MSTATUS_SIE | MSTATUS_SPIE | MSTATUS_SPP | 
                                           MSTATUS_FS | MSTATUS_XS | MSTATUS_SUM | 
                                           MSTATUS_MXR | MSTATUS_SD),
            CSR_SIE => self.mie & self.mideleg,
            CSR_STVEC => self.stvec,
            CSR_SCOUNTEREN => self.scounteren,
            CSR_SSCRATCH => self.sscratch,
            CSR_SEPC => self.sepc,
            CSR_SCAUSE => self.scause,
            CSR_STVAL => self.stval,
            CSR_SIP => self.mip & self.mideleg,
            CSR_SATP => self.satp,
            
            CSR_CYCLE | CSR_INSTRET => self.cycle as u32,
            CSR_CYCLEH | CSR_INSTRETH => (self.cycle >> 32) as u32,
            CSR_TIME => self.time as u32,
            CSR_TIMEH => (self.time >> 32) as u32,
            
            CSR_FCSR | CSR_FFLAGS | CSR_FRM => 0, // FPU not implemented
            
            _ => 0, // Unknown CSR returns 0
        })
    }
    
    /// Write CSR (returns false if invalid/inaccessible/read-only)
    pub fn write(&mut self, addr: u32, value: u32, priv_level: PrivilegeLevel) -> bool {
        // Check privilege level
        let min_priv = ((addr >> 8) & 3) as u8;
        if (priv_level as u8) < min_priv {
            return false;
        }
        
        // Check read-only (CSR[11:10] == 0b11 means read-only)
        if (addr >> 10) & 3 == 3 {
            return false;
        }
        
        match addr {
            CSR_MSTATUS => {
                let mask = MSTATUS_SIE | MSTATUS_MIE | MSTATUS_SPIE | MSTATUS_MPIE |
                           MSTATUS_SPP | MSTATUS_MPP | MSTATUS_FS | MSTATUS_MPRV |
                           MSTATUS_SUM | MSTATUS_MXR | MSTATUS_TVM | MSTATUS_TW | MSTATUS_TSR;
                self.mstatus = value & mask;
            }
            CSR_MEDELEG => self.medeleg = value,
            CSR_MIDELEG => self.mideleg = value,
            CSR_MIE => {
                let mask = MIP_SSIP | MIP_MSIP | MIP_STIP | MIP_MTIP | MIP_SEIP | MIP_MEIP;
                self.mie = value & mask;
            }
            CSR_MTVEC => self.mtvec = value & !3, // Align to 4 bytes
            CSR_MCOUNTEREN => self.mcounteren = value,
            CSR_MSCRATCH => self.mscratch = value,
            CSR_MEPC => self.mepc = value & !1, // Align to 2 bytes
            CSR_MCAUSE => self.mcause = value,
            CSR_MTVAL => self.mtval = value,
            CSR_MIP => {
                // Only SSIP and STIP are writable
                let mask = MIP_SSIP | MIP_STIP;
                self.mip = (self.mip & !mask) | (value & mask);
            }
            
            CSR_SSTATUS => {
                let mask = MSTATUS_SIE | MSTATUS_SPIE | MSTATUS_SPP | MSTATUS_FS |
                           MSTATUS_XS | MSTATUS_SUM | MSTATUS_MXR;
                self.mstatus = (self.mstatus & !mask) | (value & mask);
            }
            CSR_SIE => {
                self.mie = (self.mie & !self.mideleg) | (value & self.mideleg);
            }
            CSR_STVEC => self.stvec = value & !3,
            CSR_SCOUNTEREN => self.scounteren = value,
            CSR_SSCRATCH => self.sscratch = value,
            CSR_SEPC => self.sepc = value & !1,
            CSR_SCAUSE => self.scause = value,
            CSR_STVAL => self.stval = value,
            CSR_SIP => {
                let mask = MIP_SSIP & self.mideleg;
                self.mip = (self.mip & !mask) | (value & mask);
            }
            CSR_SATP => self.satp = value,
            
            _ => return false,
        }
        
        true
    }
    
    /// Set interrupt pending bit
    pub fn set_interrupt_pending(&mut self, bit: u32) {
        self.mip |= bit;
    }
    
    /// Clear interrupt pending bit
    pub fn clear_interrupt_pending(&mut self, bit: u32) {
        self.mip &= !bit;
    }
    
    /// Reset CSRs
    pub fn reset(&mut self) {
        self.mstatus = 0;
        self.medeleg = 0;
        self.mideleg = 0;
        self.mie = 0;
        self.mtvec = 0;
        self.mcounteren = 0;
        self.mscratch = 0;
        self.mepc = 0;
        self.mcause = 0;
        self.mtval = 0;
        self.mip = 0;
        
        self.stvec = 0;
        self.scounteren = 0;
        self.sscratch = 0;
        self.sepc = 0;
        self.scause = 0;
        self.stval = 0;
        self.satp = 0;
        
        self.cycle = 0;
        self.time = 0;
    }
}
