//! Memory subsystem with device-mapped I/O
//!
//! Memory layout (based on jor1k RISC-V):
//! 0x00000000 - 0x00001FFF: Boot ROM
//! 0x02000000 - 0x02001FFF: CLINT (timer)
//! 0x03000000 - 0x03001FFF: UART
//! 0x04000000 - 0x04FFFFFF: PLIC (interrupt controller)
//! 0x20000000 - 0x20001FFF: VirtIO device 1 (9p)
//! 0x80000000 - ...:        RAM (DRAM_BASE)

use std::sync::Arc;
use serde::{Serialize, Deserialize};

/// Trait for memory-mapped devices
pub trait Device: Send + Sync {
    fn read8(&self, offset: u32) -> u8;
    fn write8(&mut self, offset: u32, value: u8);
    
    fn read16(&self, offset: u32) -> u16 {
        let lo = self.read8(offset) as u16;
        let hi = self.read8(offset + 1) as u16;
        lo | (hi << 8)
    }
    
    fn write16(&mut self, offset: u32, value: u16) {
        self.write8(offset, value as u8);
        self.write8(offset + 1, (value >> 8) as u8);
    }
    
    fn read32(&self, offset: u32) -> u32 {
        let lo = self.read16(offset) as u32;
        let hi = self.read16(offset + 2) as u32;
        lo | (hi << 16)
    }
    
    fn write32(&mut self, offset: u32, value: u32) {
        self.write16(offset, value as u16);
        self.write16(offset + 2, (value >> 16) as u16);
    }
    
    fn reset(&mut self);
}

/// A memory-mapped device region
struct DeviceMapping {
    base: u32,
    size: u32,
    device_idx: usize,
}

/// Memory subsystem with RAM and device mappings
#[derive(Serialize, Deserialize)]
pub struct Memory {
    /// Main RAM (starts at DRAM_BASE)
    ram: Vec<u8>,
    ram_base: u32,
    
    /// Boot ROM
    rom: Vec<u8>,
    
    /// Device mappings
    #[serde(skip)]
    mappings: Vec<DeviceMapping>,
    
    /// Actual devices (stored separately for mutability)
    #[serde(skip)]
    devices: Vec<Box<dyn Device>>,
}

/// Memory addresses
pub const DRAM_BASE: u32 = 0x8000_0000;
pub const ROM_BASE: u32 = 0x0000_1000;
pub const ROM_SIZE: u32 = 0x0000_2000;

/// Bus interface for CPU memory access
pub trait Bus {
    fn read8(&self, addr: u32) -> u8;
    fn write8(&mut self, addr: u32, value: u8);
    fn read16(&self, addr: u32) -> u16;
    fn write16(&mut self, addr: u32, value: u16);
    fn read32(&self, addr: u32) -> u32;
    fn write32(&mut self, addr: u32, value: u32);
}

impl Bus for Memory {
    fn read8(&self, addr: u32) -> u8 {
        self.read8(addr)
    }
    
    fn write8(&mut self, addr: u32, value: u8) {
        self.write8(addr, value)
    }
    
    fn read16(&self, addr: u32) -> u16 {
        self.read16(addr)
    }
    
    fn write16(&mut self, addr: u32, value: u16) {
        self.write16(addr, value)
    }
    
    fn read32(&self, addr: u32) -> u32 {
        self.read32(addr)
    }
    
    fn write32(&mut self, addr: u32, value: u32) {
        self.write32(addr, value)
    }
}

impl Memory {
    pub fn new(ram_size_mb: u32) -> Self {
        let ram_size = (ram_size_mb as usize) * 1024 * 1024;
        
        Memory {
            ram: vec![0u8; ram_size],
            ram_base: DRAM_BASE,
            rom: vec![0u8; ROM_SIZE as usize],
            mappings: Vec::new(),
            devices: Vec::new(),
        }
    }
    
    /// Get RAM size in bytes
    pub fn ram_size(&self) -> usize {
        self.ram.len()
    }
    
    /// Add a device at the specified address range
    pub fn add_device(&mut self, device: Box<dyn Device>, base: u32, size: u32) {
        let device_idx = self.devices.len();
        self.devices.push(device);
        self.mappings.push(DeviceMapping { base, size, device_idx });
    }
    
    /// Get device by index (for interrupt handling etc)
    pub fn get_device_mut(&mut self, idx: usize) -> Option<&mut Box<dyn Device>> {
        self.devices.get_mut(idx)
    }
    
    /// Initialize boot ROM with jump to kernel
    pub fn init_boot_rom(&mut self) {
        // Boot at 0x1000, jump to DRAM_BASE (0x80000000)
        // auipc t0, 0x80000000 - 0x1000 (load upper 20 bits)
        // jalr zero, t0, 0 (jump to t0)
        
        let instructions: [u32; 8] = [
            0x7ffff297,           // auipc t0, 0x7ffff (t0 = 0x1000 + 0x7ffff000 = 0x80000000)
            0x00028067,           // jalr zero, t0, 0
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
        ];
        
        for (i, &inst) in instructions.iter().enumerate() {
            let offset = i * 4;
            self.rom[offset] = inst as u8;
            self.rom[offset + 1] = (inst >> 8) as u8;
            self.rom[offset + 2] = (inst >> 16) as u8;
            self.rom[offset + 3] = (inst >> 24) as u8;
        }
    }
    
    /// Load binary data into RAM
    pub fn load_binary(&mut self, data: &[u8], addr: u32) -> Result<(), String> {
        if addr < self.ram_base {
            return Err(format!("Load address 0x{:08x} below RAM base", addr));
        }
        
        let offset = (addr - self.ram_base) as usize;
        if offset + data.len() > self.ram.len() {
            return Err(format!("Binary too large for RAM"));
        }
        
        self.ram[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }
    
    /// Find device for address
    fn find_device(&self, addr: u32) -> Option<(usize, u32)> {
        for mapping in &self.mappings {
            if addr >= mapping.base && addr < mapping.base + mapping.size {
                return Some((mapping.device_idx, addr - mapping.base));
            }
        }
        None
    }
    
    /// Read 8 bits
    pub fn read8(&self, addr: u32) -> u8 {
        // Check ROM
        if addr >= ROM_BASE && addr < ROM_BASE + ROM_SIZE {
            return self.rom[(addr - ROM_BASE) as usize];
        }
        
        // Check RAM
        if addr >= self.ram_base && addr < self.ram_base + self.ram.len() as u32 {
            return self.ram[(addr - self.ram_base) as usize];
        }
        
        // Check devices
        if let Some((idx, offset)) = self.find_device(addr) {
            return self.devices[idx].read8(offset);
        }
        
        // Unmapped - return 0
        0
    }
    
    /// Write 8 bits
    pub fn write8(&mut self, addr: u32, value: u8) {
        // Check RAM
        if addr >= self.ram_base && addr < self.ram_base + self.ram.len() as u32 {
            self.ram[(addr - self.ram_base) as usize] = value;
            return;
        }
        
        // Check devices
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].write8(offset, value);
            return;
        }
        
        // Unmapped - ignore
    }
    
    /// Read 16 bits (little endian)
    pub fn read16(&self, addr: u32) -> u16 {
        // Check RAM (fast path)
        if addr >= self.ram_base && addr + 1 < self.ram_base + self.ram.len() as u32 {
            let offset = (addr - self.ram_base) as usize;
            return u16::from_le_bytes([self.ram[offset], self.ram[offset + 1]]);
        }
        
        // Check devices
        if let Some((idx, offset)) = self.find_device(addr) {
            return self.devices[idx].read16(offset);
        }
        
        // Fallback to byte reads
        let lo = self.read8(addr) as u16;
        let hi = self.read8(addr + 1) as u16;
        lo | (hi << 8)
    }
    
    /// Write 16 bits (little endian)
    pub fn write16(&mut self, addr: u32, value: u16) {
        // Check RAM (fast path)
        if addr >= self.ram_base && addr + 1 < self.ram_base + self.ram.len() as u32 {
            let offset = (addr - self.ram_base) as usize;
            let bytes = value.to_le_bytes();
            self.ram[offset] = bytes[0];
            self.ram[offset + 1] = bytes[1];
            return;
        }
        
        // Check devices
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].write16(offset, value);
            return;
        }
        
        // Fallback to byte writes
        self.write8(addr, value as u8);
        self.write8(addr + 1, (value >> 8) as u8);
    }
    
    /// Read 32 bits (little endian)
    pub fn read32(&self, addr: u32) -> u32 {
        // Check ROM
        if addr >= ROM_BASE && addr + 3 < ROM_BASE + ROM_SIZE {
            let offset = (addr - ROM_BASE) as usize;
            return u32::from_le_bytes([
                self.rom[offset],
                self.rom[offset + 1],
                self.rom[offset + 2],
                self.rom[offset + 3],
            ]);
        }
        
        // Check RAM (fast path)
        if addr >= self.ram_base && addr + 3 < self.ram_base + self.ram.len() as u32 {
            let offset = (addr - self.ram_base) as usize;
            return u32::from_le_bytes([
                self.ram[offset],
                self.ram[offset + 1],
                self.ram[offset + 2],
                self.ram[offset + 3],
            ]);
        }
        
        // Check devices
        if let Some((idx, offset)) = self.find_device(addr) {
            return self.devices[idx].read32(offset);
        }
        
        // Fallback to byte reads
        let b0 = self.read8(addr) as u32;
        let b1 = self.read8(addr + 1) as u32;
        let b2 = self.read8(addr + 2) as u32;
        let b3 = self.read8(addr + 3) as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }
    
    /// Write 32 bits (little endian)
    pub fn write32(&mut self, addr: u32, value: u32) {
        // Check RAM (fast path)
        if addr >= self.ram_base && addr + 3 < self.ram_base + self.ram.len() as u32 {
            let offset = (addr - self.ram_base) as usize;
            let bytes = value.to_le_bytes();
            self.ram[offset] = bytes[0];
            self.ram[offset + 1] = bytes[1];
            self.ram[offset + 2] = bytes[2];
            self.ram[offset + 3] = bytes[3];
            return;
        }
        
        // Check devices
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].write32(offset, value);
            return;
        }
        
        // Fallback to byte writes
        self.write8(addr, value as u8);
        self.write8(addr + 1, (value >> 8) as u8);
        self.write8(addr + 2, (value >> 16) as u8);
        self.write8(addr + 3, (value >> 24) as u8);
    }
    
    pub fn reset(&mut self) {
        self.ram.fill(0);
        for device in &mut self.devices {
            device.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ram_read_write() {
        let mut mem = Memory::new(1);
        
        // Write to RAM
        mem.write32(DRAM_BASE, 0xDEADBEEF);
        assert_eq!(mem.read32(DRAM_BASE), 0xDEADBEEF);
        
        // Write bytes
        mem.write8(DRAM_BASE + 4, 0x42);
        assert_eq!(mem.read8(DRAM_BASE + 4), 0x42);
    }
    
    #[test]
    fn test_load_binary() {
        let mut mem = Memory::new(1);
        let data = [0x13, 0x00, 0x00, 0x00]; // NOP instruction
        
        mem.load_binary(&data, DRAM_BASE).unwrap();
        assert_eq!(mem.read32(DRAM_BASE), 0x00000013);
    }
}
