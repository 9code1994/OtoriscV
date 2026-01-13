//! CLINT - Core Local Interruptor
//!
//! Provides timer and software interrupts for RISC-V
//! Based on jor1k's clint.js

// CLINT memory map (relative to base 0x02000000)
const MSIP_BASE: u32 = 0x0000;       // Machine Software Interrupt Pending
const MTIMECMP_BASE: u32 = 0x4000;   // Machine Timer Compare
const MTIME_BASE: u32 = 0xBFF8;      // Machine Timer

use serde::{Serialize, Deserialize};

/// CLINT device
#[derive(Serialize, Deserialize)]
pub struct Clint {
    /// Machine Software Interrupt Pending (per hart, we support 1)
    msip: u32,
    /// Timer compare value
    mtimecmp: u64,
    /// Current timer value
    mtime: u64,
    
    /// Timer interrupt pending
    pub timer_interrupt: bool,
    /// Software interrupt pending  
    pub software_interrupt: bool,
}

impl Clint {
    pub fn new() -> Self {
        Clint {
            msip: 0,
            mtimecmp: u64::MAX,
            mtime: 0,
            timer_interrupt: false,
            software_interrupt: false,
        }
    }
    
    /// Advance timer by given ticks
    pub fn tick(&mut self, ticks: u64) {
        self.mtime = self.mtime.wrapping_add(ticks);
        self.check_timer();
    }
    
    /// Set mtime directly (for time sync)
    pub fn set_mtime(&mut self, value: u64) {
        self.mtime = value;
        self.check_timer();
    }
    
    /// Get current mtime
    pub fn get_mtime(&self) -> u64 {
        self.mtime
    }
    
    /// Check if timer interrupt should fire
    fn check_timer(&mut self) {
        self.timer_interrupt = self.mtime >= self.mtimecmp;
    }
    
    pub fn read8(&self, offset: u32) -> u8 {
        let word_offset = offset & !3;
        let byte_offset = offset & 3;
        let word = self.read32(word_offset);
        ((word >> (byte_offset * 8)) & 0xFF) as u8
    }
    
    pub fn write8(&mut self, offset: u32, value: u8) {
        let word_offset = offset & !3;
        let byte_offset = offset & 3;
        let mut word = self.read32(word_offset);
        let mask = 0xFF << (byte_offset * 8);
        word = (word & !mask) | ((value as u32) << (byte_offset * 8));
        self.write32(word_offset, word);
    }
    
    pub fn read32(&self, offset: u32) -> u32 {
        match offset {
            o if o >= MSIP_BASE && o < MSIP_BASE + 4 => self.msip,
            o if o >= MTIMECMP_BASE && o < MTIMECMP_BASE + 4 => self.mtimecmp as u32,
            o if o >= MTIMECMP_BASE + 4 && o < MTIMECMP_BASE + 8 => (self.mtimecmp >> 32) as u32,
            o if o >= MTIME_BASE && o < MTIME_BASE + 4 => self.mtime as u32,
            o if o >= MTIME_BASE + 4 && o < MTIME_BASE + 8 => (self.mtime >> 32) as u32,
            _ => 0,
        }
    }
    
    pub fn write32(&mut self, offset: u32, value: u32) {
        match offset {
            o if o >= MSIP_BASE && o < MSIP_BASE + 4 => {
                self.msip = value & 1;
                self.software_interrupt = self.msip != 0;
            }
            o if o >= MTIMECMP_BASE && o < MTIMECMP_BASE + 4 => {
                self.mtimecmp = (self.mtimecmp & 0xFFFFFFFF00000000) | (value as u64);
                self.check_timer();
            }
            o if o >= MTIMECMP_BASE + 4 && o < MTIMECMP_BASE + 8 => {
                self.mtimecmp = (self.mtimecmp & 0x00000000FFFFFFFF) | ((value as u64) << 32);
                self.check_timer();
            }
            o if o >= MTIME_BASE && o < MTIME_BASE + 4 => {
                self.mtime = (self.mtime & 0xFFFFFFFF00000000) | (value as u64);
                self.check_timer();
            }
            o if o >= MTIME_BASE + 4 && o < MTIME_BASE + 8 => {
                self.mtime = (self.mtime & 0x00000000FFFFFFFF) | ((value as u64) << 32);
                self.check_timer();
            }
            _ => {}
        }
    }
    
    pub fn reset(&mut self) {
        self.msip = 0;
        self.mtimecmp = u64::MAX;
        self.mtime = 0;
        self.timer_interrupt = false;
        self.software_interrupt = false;
    }
}
