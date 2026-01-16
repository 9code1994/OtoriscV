//! Lightweight snapshot system
//!
//! Saves only essential state (CPU + devices + dirty RAM pages) instead of full RAM,
//! reducing snapshot size from ~5MB to <100KB.

use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use crate::cpu::{PrivilegeLevel, Fpu};
use crate::cpu::rv32::Csr;

/// Lightweight snapshot that only saves changed state
/// 
/// This snapshot doesn't include the full RAM - it requires the same
/// kernel/initrd to be loaded before restoration.
#[derive(Serialize, Deserialize)]
pub struct LightweightSnapshot {
    /// Version for compatibility checking
    pub version: u32,
    
    /// Kernel size (for validation on restore)
    pub kernel_size: u32,
    
    /// Initrd size (for validation on restore)  
    pub initrd_size: Option<u32>,
    
    /// CPU state
    pub cpu: CpuSnapshot,
    
    /// UART state
    pub uart: UartSnapshot,
    
    /// CLINT state
    pub clint: ClintSnapshot,
    
    /// PLIC state
    pub plic: PlicSnapshot,
    
    /// Dirty RAM pages (page_addr -> page_data)
    /// Only pages that were modified after boot are stored
    pub dirty_pages: HashMap<u32, Vec<u8>>,
}

/// CPU state snapshot
#[derive(Serialize, Deserialize)]
pub struct CpuSnapshot {
    /// Program counter
    pub pc: u32,
    /// General purpose registers (x0-x31)
    pub regs: [u32; 32],
    /// Floating-point unit
    pub fpu: Fpu,
    /// Control and Status Registers
    pub csr: Csr,
    /// Current privilege level
    pub priv_level: PrivilegeLevel,
    /// Wait for interrupt flag
    pub wfi: bool,
    /// LR/SC reservation
    pub reservation: Option<u32>,
    /// Instruction counter
    pub instruction_count: u64,
}

/// UART state snapshot
#[derive(Serialize, Deserialize)]
pub struct UartSnapshot {
    /// Interrupt enable register
    pub ier: u8,
    /// FIFO control register
    pub fcr: u8,
    /// Line control register
    pub lcr: u8,
    /// Modem control register
    pub mcr: u8,
    /// Line status register
    pub lsr: u8,
    /// Modem status register
    pub msr: u8,
    /// Scratch register
    pub scr: u8,
    /// Divisor latch (low)
    pub dll: u8,
    /// Divisor latch (high)
    pub dlm: u8,
    /// RX FIFO contents
    pub rx_fifo: Vec<u8>,
    /// TX output buffer
    pub tx_output: Vec<u8>,
}

/// CLINT (Core Local Interruptor) state snapshot
#[derive(Serialize, Deserialize)]
pub struct ClintSnapshot {
    /// Machine time
    pub mtime: u64,
    /// Machine time compare
    pub mtimecmp: u64,
    /// Machine software interrupt pending
    pub msip: bool,
}

/// PLIC state snapshot
#[derive(Serialize, Deserialize)]
pub struct PlicSnapshot {
    /// Priority registers (interrupt 1-31)
    pub priority: [u8; 32],
    /// Pending bits
    pub pending: u32,
    /// Enable bits for context 0 (M-mode)
    pub enable_m: u32,
    /// Enable bits for context 1 (S-mode)
    pub enable_s: u32,
    /// Priority threshold for context 0
    pub threshold_m: u8,
    /// Priority threshold for context 1
    pub threshold_s: u8,
    /// Claimed interrupts for context 0
    pub claim_m: u32,
    /// Claimed interrupts for context 1
    pub claim_s: u32,
}

/// Page size for dirty tracking (4KB)
pub const PAGE_SIZE: u32 = 4096;

impl LightweightSnapshot {
    /// Current snapshot version
    pub const VERSION: u32 = 1;
    
    /// Create a new empty snapshot
    pub fn new(kernel_size: u32, initrd_size: Option<u32>) -> Self {
        LightweightSnapshot {
            version: Self::VERSION,
            kernel_size,
            initrd_size,
            cpu: CpuSnapshot {
                pc: 0,
                regs: [0; 32],
                fpu: Fpu::new(),
                csr: Csr::new(),
                priv_level: PrivilegeLevel::Machine,
                wfi: false,
                reservation: None,
                instruction_count: 0,
            },
            uart: UartSnapshot {
                ier: 0,
                fcr: 0,
                lcr: 0,
                mcr: 0,
                lsr: 0x60, // TX empty
                msr: 0,
                scr: 0,
                dll: 0,
                dlm: 0,
                rx_fifo: Vec::new(),
                tx_output: Vec::new(),
            },
            clint: ClintSnapshot {
                mtime: 0,
                mtimecmp: 0,
                msip: false,
            },
            plic: PlicSnapshot {
                priority: [0; 32],
                pending: 0,
                enable_m: 0,
                enable_s: 0,
                threshold_m: 0,
                threshold_s: 0,
                claim_m: 0,
                claim_s: 0,
            },
            dirty_pages: HashMap::new(),
        }
    }
    
    /// Serialize to bytes (compressed with zstd)
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let serialized = bincode::serialize(self)
            .map_err(|e| format!("Serialization error: {}", e))?;
        
        zstd::stream::encode_all(&serialized[..], 3)
            .map_err(|e| format!("Compression error: {}", e))
    }
    
    /// Deserialize from bytes (compressed with zstd)
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        let decompressed = zstd::stream::decode_all(data)
            .map_err(|e| format!("Decompression error: {}", e))?;
        
        bincode::deserialize(&decompressed)
            .map_err(|e| format!("Deserialization error: {}", e))
    }
}
