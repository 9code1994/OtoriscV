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
    fn read8(&mut self, addr: u32) -> u8;
    fn write8(&mut self, addr: u32, value: u8);
    fn read16(&mut self, addr: u32) -> u16;
    fn write16(&mut self, addr: u32, value: u16);
    fn read32(&mut self, addr: u32) -> u32;
    fn write32(&mut self, addr: u32, value: u32);
    fn read64(&mut self, addr: u32) -> u64;
    fn write64(&mut self, addr: u32, value: u64);
}

impl Bus for Memory {
    fn read8(&mut self, addr: u32) -> u8 {
        Memory::read8(self, addr)
    }
    
    fn write8(&mut self, addr: u32, value: u8) {
        Memory::write8(self, addr, value)
    }
    
    fn read16(&mut self, addr: u32) -> u16 {
        Memory::read16(self, addr)
    }
    
    fn write16(&mut self, addr: u32, value: u16) {
        Memory::write16(self, addr, value)
    }
    
    fn read32(&mut self, addr: u32) -> u32 {
        Memory::read32(self, addr)
    }
    
    fn write32(&mut self, addr: u32, value: u32) {
        Memory::write32(self, addr, value)
    }
    
    fn read64(&mut self, addr: u32) -> u64 {
        Memory::read64(self, addr)
    }
    
    fn write64(&mut self, addr: u32, value: u64) {
        Memory::write64(self, addr, value)
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
    
    /// Get direct access to RAM slice (for jor1k-style direct access optimization)
    /// 
    /// # Safety
    /// Callers must ensure:
    /// - Addresses are properly bounds-checked before access
    /// - No aliasing violations when combined with other mutable access
    #[inline(always)]
    pub fn ram_slice(&self) -> &[u8] {
        &self.ram
    }
    
    /// Get mutable direct access to RAM slice
    #[inline(always)]
    pub fn ram_slice_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }
    
    /// Direct 32-bit RAM read (no bounds check - caller must ensure validity)
    /// 
    /// jor1k-style optimization: single array access, no function call overhead
    #[inline(always)]
    pub unsafe fn ram_read32_unchecked(&self, offset: usize) -> u32 {
        debug_assert!(offset + 3 < self.ram.len());
        let ptr = self.ram.as_ptr().add(offset) as *const u32;
        ptr.read_unaligned()
    }
    
    /// Direct 32-bit RAM write (no bounds check - caller must ensure validity)
    #[inline(always)]
    pub unsafe fn ram_write32_unchecked(&mut self, offset: usize, value: u32) {
        debug_assert!(offset + 3 < self.ram.len());
        let ptr = self.ram.as_mut_ptr().add(offset) as *mut u32;
        ptr.write_unaligned(value);
    }
    
    /// Direct 8-bit RAM read (no bounds check)
    #[inline(always)]
    pub unsafe fn ram_read8_unchecked(&self, offset: usize) -> u8 {
        debug_assert!(offset < self.ram.len());
        *self.ram.get_unchecked(offset)
    }
    
    /// Direct 8-bit RAM write (no bounds check)
    #[inline(always)]
    pub unsafe fn ram_write8_unchecked(&mut self, offset: usize, value: u8) {
        debug_assert!(offset < self.ram.len());
        *self.ram.get_unchecked_mut(offset) = value;
    }
    
    /// Direct 16-bit RAM read (no bounds check)
    #[inline(always)]
    pub unsafe fn ram_read16_unchecked(&self, offset: usize) -> u16 {
        debug_assert!(offset + 1 < self.ram.len());
        let ptr = self.ram.as_ptr().add(offset) as *const u16;
        ptr.read_unaligned()
    }
    
    /// Direct 16-bit RAM write (no bounds check)
    #[inline(always)]
    pub unsafe fn ram_write16_unchecked(&mut self, offset: usize, value: u16) {
        debug_assert!(offset + 1 < self.ram.len());
        let ptr = self.ram.as_mut_ptr().add(offset) as *mut u16;
        ptr.write_unaligned(value);
    }
    
    /// Direct 64-bit RAM read (no bounds check)
    #[inline(always)]
    pub unsafe fn ram_read64_unchecked(&self, offset: usize) -> u64 {
        debug_assert!(offset + 7 < self.ram.len());
        let ptr = self.ram.as_ptr().add(offset) as *const u64;
        ptr.read_unaligned()
    }
    
    /// Direct 64-bit RAM write (no bounds check)
    #[inline(always)]
    pub unsafe fn ram_write64_unchecked(&mut self, offset: usize, value: u64) {
        debug_assert!(offset + 7 < self.ram.len());
        let ptr = self.ram.as_mut_ptr().add(offset) as *mut u64;
        ptr.write_unaligned(value);
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
    
    /// Initialize boot ROM with minimal SBI-like firmware
    /// 
    /// This sets up the system for Linux boot:
    /// 1. Delegate exceptions/interrupts to S-mode
    /// 2. Set MPP to Supervisor mode  
    /// 3. Set MEPC to kernel entry (0x80000000)
    /// 4. Use MRET to drop to S-mode and start kernel
    pub fn init_boot_rom(&mut self) {
        // Boot ROM at 0x1000
        // Acts as minimal M-mode firmware like OpenSBI
        //
        // Linux expects:
        // - a0 = hartid (already set by setup_linux_boot)
        // - a1 = dtb address (already set by setup_linux_boot)
        // - Running in S-mode with SBI available for ecalls
        
        let instructions: [u32; 29] = [
            // === Setup exception delegation ===
            // Delegate most exceptions to S-mode, but NOT ecall from S-mode
            // medeleg = 0xB1FF (delegate exceptions 0-8, 12-15 to S-mode)
            // Bit 8 (ecall from U) is delegated, bit 9 (ecall from S) is NOT
            0x0000b2b7,           // lui t0, 0xB         ; t0 = 0xB000
            0x1ff28293,           // addi t0, t0, 0x1FF  ; t0 = 0xB1FF
            0x30229073,           // csrw medeleg, t0
            
            // Delegate S-mode interrupts (bits 1,5,9 = SSI, STI, SEI)
            0x00000293,           // li t0, 0
            0x22228293,           // addi t0, t0, 0x222  ; t0 = 0x222 (SSI+STI+SEI)
            0x30329073,           // csrw mideleg, t0
            
            // === Setup mstatus for transition to S-mode ===
            // Set MPP = Supervisor (01), MPIE = 1
            // mstatus bits: MPP[12:11]=01 (S-mode), MPIE[7]=1
            0x00000297,           // auipc t0, 0         ; t0 = PC (for computing addresses)
            0x00001337,           // lui t1, 1           ; t1 = 0x1000
            0x88030313,           // addi t1, t1, -0x780 ; t1 = 0x880 (MPP=01, MPIE=1)
            0x30031073,           // csrw mstatus, t1
            
            // === Set mepc to kernel entry point ===
            0x800002b7,           // lui t0, 0x80000     ; t0 = 0x80000000
            0x34129073,           // csrw mepc, t0
            
            // === Set up mtvec for SBI trap handler ===
            // Point to simple SBI handler at ROM address 0x1080
            0x000012b7,           // lui t0, 0x1         ; t0 = 0x1000
            0x08028293,           // addi t0, t0, 0x80   ; t0 = 0x1080
            0x30529073,           // csrw mtvec, t0
            
            // === Enable counter access from S-mode ===
            0x00700293,           // li t0, 7            ; enable cycle, time, instret
            0x30629073,           // csrw mcounteren, t0
            
            // === Jump to S-mode kernel using MRET ===
            0x30200073,           // mret
            
            // === Padding ===
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
            0x00000013,           // nop
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
        
        // Add SBI trap handler at offset 0x80 (address 0x1080)
        // This handles ecalls from S-mode (SBI calls)
        self.init_sbi_handler();
    }
    
    /// Initialize SBI ecall handler stub in boot ROM
    /// 
    /// Note: SBI calls are now handled directly in Rust (system.rs handle_sbi_call).
    /// This stub is kept for compatibility but should never be executed.
    fn init_sbi_handler(&mut self) {
        // Minimal SBI handler stub at ROM offset 0x80 (address 0x1080)
        // Just in case we somehow reach here, loop forever
        let sbi_handler: [u32; 4] = [
            0x0000006f,           // 0x00: j 0 (infinite loop)
            0x00000013,           // 0x04: nop
            0x00000013,           // 0x08: nop
            0x00000013,           // 0x0C: nop
        ];
        
        // Write SBI handler at offset 0x80
        for (i, &inst) in sbi_handler.iter().enumerate() {
            let offset = 0x80 + i * 4;
            if offset + 3 < self.rom.len() {
                self.rom[offset] = inst as u8;
                self.rom[offset + 1] = (inst >> 8) as u8;
                self.rom[offset + 2] = (inst >> 16) as u8;
                self.rom[offset + 3] = (inst >> 24) as u8;
            }
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
    
    /// Read 64 bits (little endian) - needed for RV64 and FLD
    pub fn read64(&self, addr: u32) -> u64 {
        // Check RAM (fast path)
        if addr >= self.ram_base && addr + 7 < self.ram_base + self.ram.len() as u32 {
            let offset = (addr - self.ram_base) as usize;
            return u64::from_le_bytes([
                self.ram[offset],
                self.ram[offset + 1],
                self.ram[offset + 2],
                self.ram[offset + 3],
                self.ram[offset + 4],
                self.ram[offset + 5],
                self.ram[offset + 6],
                self.ram[offset + 7],
            ]);
        }
        
        // Fallback to two 32-bit reads
        let lo = self.read32(addr) as u64;
        let hi = self.read32(addr + 4) as u64;
        lo | (hi << 32)
    }
    
    /// Write 64 bits (little endian) - needed for RV64 and FSD
    pub fn write64(&mut self, addr: u32, value: u64) {
        // Check RAM (fast path)
        if addr >= self.ram_base && addr + 7 < self.ram_base + self.ram.len() as u32 {
            let offset = (addr - self.ram_base) as usize;
            let bytes = value.to_le_bytes();
            self.ram[offset] = bytes[0];
            self.ram[offset + 1] = bytes[1];
            self.ram[offset + 2] = bytes[2];
            self.ram[offset + 3] = bytes[3];
            self.ram[offset + 4] = bytes[4];
            self.ram[offset + 5] = bytes[5];
            self.ram[offset + 6] = bytes[6];
            self.ram[offset + 7] = bytes[7];
            return;
        }
        
        // Fallback to two 32-bit writes
        self.write32(addr, value as u32);
        self.write32(addr + 4, (value >> 32) as u32);
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
