//! System orchestrator
//!
//! Brings together CPU, memory, and devices

use crate::cpu::Cpu;
use crate::cpu::csr::*;
use crate::memory::{Memory, DRAM_BASE};
use crate::devices::{Uart, Clint, Plic, Virtio9p};

// Device base addresses (matching jor1k)
const CLINT_BASE: u32 = 0x0200_0000;
const CLINT_SIZE: u32 = 0x0001_0000;
const UART_BASE: u32 = 0x0300_0000;
const UART_SIZE: u32 = 0x0000_1000;
const PLIC_BASE: u32 = 0x0400_0000;
const PLIC_SIZE: u32 = 0x0400_0000;

// VirtIO devices
const VIRTIO_BASE: u32 = 0x2000_0000;
const VIRTIO_SIZE: u32 = 0x0000_1000;

// UART interrupt line on PLIC
const UART_IRQ: u32 = 10;

/// System state  
pub struct System {
    cpu: Cpu,
    memory: Memory,
    
    // Direct device references (since we can't easily downcast)
    uart: Uart,
    clint: Clint,
    plic: Plic,
    virtio9p: Virtio9p,
}

impl System {
    /// Create a new system with the specified RAM size
    pub fn new(ram_size_mb: u32) -> Result<Self, String> {
        if ram_size_mb == 0 || ram_size_mb > 2048 {
            return Err(format!("Invalid RAM size: {}MB", ram_size_mb));
        }
        
        let mut memory = Memory::new(ram_size_mb);
        memory.init_boot_rom();
        
        Ok(System {
            cpu: Cpu::new(),
            memory,
            uart: Uart::new(UART_IRQ),
            clint: Clint::new(),
            plic: Plic::new(),
            virtio9p: Virtio9p::new("rootfs"),
        })
    }
    
    /// Load a binary at the specified address
    pub fn load_binary(&mut self, data: &[u8], addr: u32) -> Result<(), String> {
        self.memory.load_binary(data, addr)
    }
    
    /// Run the emulator for a specified number of cycles
    /// Returns the number of cycles actually executed
    pub fn run(&mut self, max_cycles: u32) -> u32 {
        let mut cycles = 0u32;
        
        while cycles < max_cycles {
            // Update timer
            self.clint.tick(1);
            self.cpu.csr.time = self.clint.get_mtime();
            
            // Check for interrupts
            self.update_interrupts();
            
            // Handle pending interrupts
            if let Some(trap) = self.cpu.check_interrupts() {
                self.cpu.handle_trap(trap);
            }
            
            // If waiting for interrupt, don't execute
            if self.cpu.wfi {
                cycles += 1;
                continue;
            }
            
            // Execute one instruction with device access
            match self.step_with_devices() {
                Ok(()) => {}
                Err(trap) => {
                    self.cpu.handle_trap(trap);
                }
            }
            
            cycles += 1;
            self.cpu.csr.cycle = self.cpu.csr.cycle.wrapping_add(1);
        }
        
        cycles
    }
    
    /// Execute one instruction, handling device I/O specially
    fn step_with_devices(&mut self) -> Result<(), crate::cpu::trap::Trap> {
        // Create a temporary bus that has access to everything
        let mut bus = SystemBus {
            memory: &mut self.memory,
            uart: &mut self.uart,
            clint: &mut self.clint,
            plic: &mut self.plic,
            virtio9p: &mut self.virtio9p,
        };
        
        self.cpu.step(&mut bus)
    }
    
    // ... rest of file ...
    /// Update interrupt pending bits from devices
    fn update_interrupts(&mut self) {
        // CLINT interrupts
        if self.clint.timer_interrupt {
            self.cpu.csr.set_interrupt_pending(MIP_MTIP);
        } else {
            self.cpu.csr.clear_interrupt_pending(MIP_MTIP);
        }
        
        if self.clint.software_interrupt {
            self.cpu.csr.set_interrupt_pending(MIP_MSIP);
        } else {
            self.cpu.csr.clear_interrupt_pending(MIP_MSIP);
        }
        
        // UART -> PLIC
        if self.uart.has_interrupt() {
            self.plic.raise_interrupt(UART_IRQ);
        }
        
        // PLIC -> CPU
        if self.plic.m_external_interrupt {
            self.cpu.csr.set_interrupt_pending(MIP_MEIP);
        } else {
            self.cpu.csr.clear_interrupt_pending(MIP_MEIP);
        }
        
        if self.plic.s_external_interrupt {
            self.cpu.csr.set_interrupt_pending(MIP_SEIP);
        } else {
            self.cpu.csr.clear_interrupt_pending(MIP_SEIP);
        }
    }
    
    /// Check if CPU is halted (WFI)
    pub fn is_halted(&self) -> bool {
        self.cpu.wfi
    }
    
    /// Send a character to UART
    pub fn uart_receive(&mut self, c: u8) {
        self.uart.receive_char(c);
    }
    
    /// Get pending UART output
    pub fn uart_get_output(&mut self) -> Vec<u8> {
        self.uart.get_output()
    }
    
    /// Get current PC
    pub fn get_pc(&self) -> u32 {
        self.cpu.pc
    }
    
    /// Get instruction count
    pub fn get_instruction_count(&self) -> u64 {
        self.cpu.instruction_count
    }

    /// Get all register values (x0-x31)
    pub fn get_registers(&self) -> Vec<u32> {
        self.cpu.regs.to_vec()
    }
    
    /// Read debugging memory (safe, no side effects)
    pub fn read_memory(&self, addr: u32, size: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity(size as usize);
        for i in 0..size {
            let read_addr = addr + i;
            if read_addr >= DRAM_BASE {
                data.push(self.memory.read8(read_addr));
            } else {
                data.push(0);
            }
        }
        data
    }
    
    /// Reset the system
    pub fn reset(&mut self) {
        self.cpu.reset();
        self.memory.reset();
        self.uart.reset();
        self.clint.reset();
        self.plic.reset();
        self.virtio9p.reset();
    }
}

/// Bus implementation that routes to devices
struct SystemBus<'a> {
    memory: &'a mut Memory,
    uart: &'a mut Uart,
    clint: &'a mut Clint,
    plic: &'a mut Plic,
    virtio9p: &'a mut Virtio9p,
}

use crate::memory::Bus;

impl<'a> Bus for SystemBus<'a> {
    fn read8(&self, addr: u32) -> u8 {
        // Check CLINT
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            return self.clint.read8(addr - CLINT_BASE);
        }
        // Check UART
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            return self.uart.read8(addr - UART_BASE);
        }
        // Check PLIC
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            return self.plic.read8(addr - PLIC_BASE);
        }
        // Check VirtIO
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            return self.virtio9p.read8(addr - VIRTIO_BASE);
        }
        // Default to memory
        self.memory.read8(addr)
    }
    
    fn write8(&mut self, addr: u32, value: u8) {
        // Check CLINT
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            self.clint.write8(addr - CLINT_BASE, value);
            return;
        }
        // Check UART
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            self.uart.write8(addr - UART_BASE, value);
            return;
        }
        // Check PLIC
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            self.plic.write8(addr - PLIC_BASE, value);
            return;
        }
        // Check VirtIO
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            self.virtio9p.write8(addr - VIRTIO_BASE, value);
            return;
        }
        // Default to memory
        self.memory.write8(addr, value);
    }
    
    fn read16(&self, addr: u32) -> u16 {
        // For devices, we can fall back to read8 logic (or implement specific if needed)
        // Since our devices don't explicitly implement read16, we'll compose it
        // BUT wait, Clint and Plic MIGHT support wide reads.
        // For now, let's just use byte reads for devices unless we're sure.
        // ACTUALLY, memory.read16 handles unaligned access well.
        // Let's implement 16-bit access by delegating to read8 for simple devices
        // and check memory first for RAM speed.
        
        let lo = self.read8(addr) as u16;
        let hi = self.read8(addr + 1) as u16;
        lo | (hi << 8)
    }
    
    fn write16(&mut self, addr: u32, value: u16) {
        self.write8(addr, value as u8);
        self.write8(addr + 1, (value >> 8) as u8);
    }
    
    fn read32(&self, addr: u32) -> u32 {
        // Check CLINT
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            return self.clint.read32(addr - CLINT_BASE);
        }
        // Check UART
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            return self.uart.read32(addr - UART_BASE);
        }
        // Check PLIC
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            return self.plic.read32(addr - PLIC_BASE);
        }
        // Check VirtIO
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            return self.virtio9p.read32(addr - VIRTIO_BASE);
        }
        // Default to memory
        self.memory.read32(addr)
    }
    
    fn write32(&mut self, addr: u32, value: u32) {
        // Check CLINT
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            self.clint.write32(addr - CLINT_BASE, value);
            return;
        }
        // Check UART
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            self.uart.write32(addr - UART_BASE, value);
            return;
        }
        // Check PLIC
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            self.plic.write32(addr - PLIC_BASE, value);
            return;
        }
        // Check VirtIO
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            self.virtio9p.write32(addr - VIRTIO_BASE, value);
            return;
        }
        // Default to memory
        self.memory.write32(addr, value);
    }
}
