//! System orchestrator
//!
//! Brings together CPU, memory, and devices

use crate::cpu::Cpu;
use crate::cpu::csr::*;
use crate::memory::{Memory, DRAM_BASE};
use crate::devices::{Uart, Clint, Plic, Virtio9p};
use crate::devices::virtio_9p::{Backend, in_memory::InMemoryFileSystem};
#[cfg(not(target_arch = "wasm32"))]
use crate::devices::virtio_9p::host::HostFileSystem;
use serde::{Serialize, Deserialize};
#[allow(unused_imports)]
use std::sync::atomic::{AtomicU64, Ordering};

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
#[derive(Serialize, Deserialize)]
pub struct System {
    pub cpu: Cpu,
    memory: Memory,
    
    // Direct device references (since we can't easily downcast)
    uart: Uart,
    pub clint: Clint,
    plic: Plic,
    virtio9p: Virtio9p,
}

impl System {
    /// Create a new system with the specified RAM size and optional host FS path
    pub fn new(ram_size_mb: u32, fs_path: Option<&str>) -> Result<Self, String> {
        if ram_size_mb == 0 || ram_size_mb > 2048 {
            return Err(format!("Invalid RAM size: {}MB", ram_size_mb));
        }
        
        let mut memory = Memory::new(ram_size_mb);
        memory.init_boot_rom();
        
        let fs_backend = if let Some(_path) = fs_path {
            #[cfg(not(target_arch = "wasm32"))]
            {
                Backend::Host(HostFileSystem::new(_path))
            }
            #[cfg(target_arch = "wasm32")]
            {
                // Host filesystem not available on WASM, use in-memory
                Backend::InMemory(InMemoryFileSystem::new())
            }
        } else {
            Backend::InMemory(InMemoryFileSystem::new())
        };

        Ok(System {
            cpu: Cpu::new(),
            memory,
            uart: Uart::new(UART_IRQ),
            clint: Clint::new(),
            plic: Plic::new(),
            virtio9p: Virtio9p::new("rootfs", fs_backend),
        })
    }
    
    /// Load a binary at the specified address
    pub fn load_binary(&mut self, data: &[u8], addr: u32) -> Result<(), String> {
        self.memory.load_binary(data, addr)
    }

    /// Setup system for Linux booting
    /// Loads kernel image and generates/loads DTB
    pub fn setup_linux_boot(&mut self, kernel: &[u8], cmdline: &str) -> Result<(), String> {
        self.setup_linux_boot_with_initrd(kernel, None, cmdline)
    }
    
    /// Setup system for Linux booting with optional initrd
    /// Loads kernel, initrd (if provided), and generates DTB
    pub fn setup_linux_boot_with_initrd(&mut self, kernel: &[u8], initrd: Option<&[u8]>, cmdline: &str) -> Result<(), String> {
        // Load kernel at DRAM_BASE (0x80000000)
        self.load_binary(kernel, DRAM_BASE)?;
        
        let ram_size = self.memory.ram_size();
        let ram_size_mb = (ram_size / 1024 / 1024) as u32;
        
        // Calculate addresses for initrd and DTB
        // Layout: [kernel] ... [initrd aligned to 4KB] [DTB aligned to 4KB] [end of RAM]
        let ram_end = DRAM_BASE + ram_size as u32;
        
        // Load initrd if provided
        let initrd_info = if let Some(initrd_data) = initrd {
            // Place initrd before DTB, aligned to page boundary
            // Reserve space for DTB (typically ~4KB, reserve 64KB to be safe)
            let dtb_reserve = 64 * 1024;
            let initrd_end = (ram_end - dtb_reserve) & !0xFFF; // Align down to 4KB
            let initrd_start = (initrd_end - initrd_data.len() as u32) & !0xFFF; // Align down
            
            // Make sure initrd doesn't overlap kernel
            let kernel_end = DRAM_BASE + kernel.len() as u32;
            if initrd_start < kernel_end + 0x100000 { // Leave at least 1MB gap
                return Err(format!(
                    "Not enough RAM for kernel ({} bytes) and initrd ({} bytes)", 
                    kernel.len(), initrd_data.len()
                ));
            }
            
            self.load_binary(initrd_data, initrd_start)?;
            println!("  Initrd loaded at 0x{:08x}-0x{:08x} ({} bytes)", 
                     initrd_start, initrd_start + initrd_data.len() as u32, initrd_data.len());
            
            Some((initrd_start, initrd_start + initrd_data.len() as u32))
        } else {
            None
        };
        
        // Generate DTB with initrd info
        let dtb = crate::devices::dtb::generate_fdt(ram_size_mb, cmdline, initrd_info);
        
        // Load DTB at end of RAM (aligned to 4KB)
        let dtb_addr = if initrd_info.is_some() {
            // Place after initrd
            let (_, initrd_end) = initrd_info.unwrap();
            (initrd_end + 0x1000) & !0xFFF // Align up with some padding
        } else {
            // No initrd, place at end of RAM
            (ram_end - dtb.len() as u32) & !0xFFF
        };
        
        // Actually, let's put DTB at end of RAM to be safe
        let dtb_addr = (ram_end - dtb.len() as u32) & !0xFFF;
        
        self.load_binary(&dtb, dtb_addr)?;
        println!("  DTB loaded at 0x{:08x} ({} bytes)", dtb_addr, dtb.len());
        
        // Setup CPU State for Linux boot via boot ROM
        // Boot ROM at 0x1000 will:
        // 1. Set up medeleg/mideleg for exception delegation
        // 2. Set mtvec to SBI handler
        // 3. Set MPP=Supervisor in mstatus
        // 4. Set mepc=0x80000000 (kernel entry)
        // 5. Execute mret to drop to S-mode and start kernel
        //
        // We just need to set up the registers that Linux expects:
        // a0 (x10) = hartid (0)
        // a1 (x11) = dtb address
        self.cpu.reset();  // PC = 0x1000 (boot ROM)
        self.cpu.write_reg(10, 0);       // a0 = hartid
        self.cpu.write_reg(11, dtb_addr); // a1 = dtb address
        
        Ok(())
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
            
            // If waiting for interrupt, check if any interrupt is pending
            // WFI wakes when (mip & mie) != 0, regardless of global enables
            if self.cpu.wfi {
                let pending = self.cpu.csr.mip & self.cpu.csr.mie;
                if pending != 0 {
                    self.cpu.wfi = false;
                    // The interrupt will be handled on next iteration
                    // (or the kernel will poll and see data available)
                } else {
                    cycles += 1;
                    continue;
                }
            }
            
            // Execute one instruction with device access
            match self.step_with_devices() {
                Ok(()) => {}
                Err(trap) => {
                    // Handle SBI calls from S-mode directly in Rust
                    if let crate::cpu::trap::Trap::EnvironmentCallFromS = trap {
                        self.handle_sbi_call();
                    } else {
                        self.cpu.handle_trap(trap);
                    }
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
        
        let result = self.cpu.step(&mut bus);
        
        // Handle VirtIO queues
        // We drop 'bus' here so we can borrow 'virtio9p' and 'memory' separately
        // (Borrow checker: bus holds mutable refs to fields, so bus must die before we use them again)
        drop(bus);
        
        self.virtio9p.process_queues(&mut self.memory);
        
        result
    }
    
    /// Handle SBI (Supervisor Binary Interface) calls from S-mode
    /// 
    /// SBI provides M-mode services to S-mode OS like Linux.
    /// The kernel uses ecall to invoke SBI services.
    /// 
    /// Calling convention:
    /// - a7 = Extension ID (EID)
    /// - a6 = Function ID (FID)  
    /// - a0-a5 = Arguments
    /// - Returns: a0 = error code, a1 = value
    fn handle_sbi_call(&mut self) {
        let eid = self.cpu.read_reg(17);  // a7 = Extension ID
        let fid = self.cpu.read_reg(16);  // a6 = Function ID
        let a0 = self.cpu.read_reg(10);
        let a1 = self.cpu.read_reg(11);

        // SBI error codes
        const SBI_SUCCESS: u32 = 0;
        const SBI_ERR_NOT_SUPPORTED: u32 = (-2i32) as u32;
        
        // Extension IDs
        const SBI_EXT_LEGACY_SET_TIMER: u32 = 0;
        const SBI_EXT_LEGACY_CONSOLE_PUTCHAR: u32 = 1;
        const SBI_EXT_LEGACY_CONSOLE_GETCHAR: u32 = 2;
        const SBI_EXT_BASE: u32 = 0x10;
        const SBI_EXT_TIME: u32 = 0x54494D45;  // "TIME"
        const SBI_EXT_IPI: u32 = 0x735049;     // "sPI"
        const SBI_EXT_RFENCE: u32 = 0x52464E43; // "RFNC"
        const SBI_EXT_HSM: u32 = 0x48534D;     // "HSM"
        const SBI_EXT_SRST: u32 = 0x53525354;  // "SRST"
        
        let (error, value) = match eid {
            SBI_EXT_LEGACY_SET_TIMER => {
                // Legacy set_timer: a0,a1 = 64-bit timer value
                self.clint.write32(0x4000, a0);      // mtimecmp low
                self.clint.write32(0x4004, a1);      // mtimecmp high
                // Clear pending timer interrupt when new timer is set
                self.cpu.csr.clear_interrupt_pending(MIP_STIP);
                (SBI_SUCCESS, 0)
            }
            
            SBI_EXT_LEGACY_CONSOLE_PUTCHAR => {
                // Legacy console_putchar: a0 = character
                self.uart.write8(0, a0 as u8);
                (SBI_SUCCESS, 0)
            }
            
            SBI_EXT_LEGACY_CONSOLE_GETCHAR => {
                // Legacy console_getchar: returns character in a0, or -1 if none
                // For now, return -1 (no input available)
                ((-1i32) as u32, 0)
            }
            
            SBI_EXT_BASE => {
                // Base extension - provides SBI version info
                match fid {
                    0 => (SBI_SUCCESS, 0x00000002),  // sbi_get_spec_version: return SBI 0.2
                    1 => (SBI_SUCCESS, 0),            // sbi_get_impl_id: 0 = BBL
                    2 => (SBI_SUCCESS, 0),            // sbi_get_impl_version  
                    3 => {
                        // sbi_probe_extension: check if extension is available
                        // a0 = extension ID to probe
                        let probe_eid = a0;
                        let available = match probe_eid {
                            0 => 1,                           // Legacy set_timer
                            1 => 1,                           // Legacy console_putchar
                            2 => 1,                           // Legacy console_getchar
                            0x10 => 1,                        // SBI_EXT_BASE
                            0x54494D45 => 1,                  // SBI_EXT_TIME
                            0x735049 => 0,                    // SBI_EXT_IPI - not available
                            0x52464E43 => 0,                  // SBI_EXT_RFENCE - not available
                            0x48534D => 0,                    // SBI_EXT_HSM - not available
                            0x53525354 => 0,                  // SBI_EXT_SRST - not available
                            _ => 0,
                        };
                        (SBI_SUCCESS, available)
                    }
                    4 => (SBI_SUCCESS, 0),            // sbi_get_mvendorid
                    5 => (SBI_SUCCESS, 0),            // sbi_get_marchid
                    6 => (SBI_SUCCESS, 0),            // sbi_get_mimpid
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                }
            }
            
            SBI_EXT_TIME => {
                // Timer extension
                match fid {
                    0 => {
                        // sbi_set_timer: a0,a1 = 64-bit timer value
                        self.clint.write32(0x4000, a0);
                        self.clint.write32(0x4004, a1);
                        self.cpu.csr.clear_interrupt_pending(MIP_STIP);
                        (SBI_SUCCESS, 0)
                    }
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                }
            }
            
            SBI_EXT_IPI | SBI_EXT_RFENCE | SBI_EXT_HSM => {
                // IPI, remote fence, HSM - minimal support
                (SBI_SUCCESS, 0)
            }
            
            SBI_EXT_SRST => {
                // System reset
                match fid {
                    0 => {
                        // sbi_system_reset
                        eprintln!("SBI system reset requested");
                        self.cpu.wfi = true;  // Halt
                        (SBI_SUCCESS, 0)
                    }
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                }
            }
            
            _ => {
                // Unknown extension - return not supported
                (SBI_ERR_NOT_SUPPORTED, 0)
            }
        };
        
        // Set return values
        self.cpu.write_reg(10, error);  // a0 = error
        self.cpu.write_reg(11, value);  // a1 = value
        
        // Advance PC past ecall
        self.cpu.pc = self.cpu.pc.wrapping_add(4);
    }

    /// Update interrupt pending bits from devices
    fn update_interrupts(&mut self) {
        // CLINT timer interrupt
        // When CLINT timer fires, we set both MTIP and STIP
        // The kernel in S-mode sees STIP (which is delegated via mideleg)
        if self.clint.timer_interrupt {
            self.cpu.csr.set_interrupt_pending(MIP_MTIP);
            self.cpu.csr.set_interrupt_pending(MIP_STIP);
        } else {
            self.cpu.csr.clear_interrupt_pending(MIP_MTIP);
            self.cpu.csr.clear_interrupt_pending(MIP_STIP);
        }
        
        if self.clint.software_interrupt {
            self.cpu.csr.set_interrupt_pending(MIP_MSIP);
        } else {
            self.cpu.csr.clear_interrupt_pending(MIP_MSIP);
        }
        
        // UART -> PLIC
        // Note: PLIC pending bits are cleared via claim/complete mechanism
        // We only raise interrupts here, the UART interrupt is level-triggered
        if self.uart.has_interrupt() {
            self.plic.raise_interrupt(UART_IRQ);
        } else {
            self.plic.clear_interrupt(UART_IRQ);
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
    
    /// Get missing blobs for lazy loading
    pub fn get_missing_blobs(&self) -> Vec<String> {
        self.virtio9p.get_missing_blobs()
    }
    
    /// Provide a blob for lazy loading
    pub fn provide_blob(&mut self, hash: String, data: Vec<u8>) {
        self.virtio9p.provide_blob(hash, data, &mut self.memory);
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
    fn read8(&mut self, addr: u32) -> u8 {
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
    
    fn read16(&mut self, addr: u32) -> u16 {
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
    
    fn read32(&mut self, addr: u32) -> u32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::DRAM_BASE;

    #[test]
    fn test_setup_linux_boot() {
        let mut sys = System::new(16, None).unwrap(); // 16MB RAM
        let dummy_kernel = vec![0x13, 0x00, 0x00, 0x00]; // NOP
        
        // Should succeed
        sys.setup_linux_boot(&dummy_kernel, "console=ttyS0").unwrap();
        
        // registers should be set
        assert_eq!(sys.cpu.pc, DRAM_BASE);
        assert_eq!(sys.cpu.read_reg(10), 0); // a0
        
        // DTB should be at end of RAM (aligned)
        let dtb_addr = sys.cpu.read_reg(11); // a1
        assert!(dtb_addr > DRAM_BASE);
        assert!(dtb_addr < DRAM_BASE + 16 * 1024 * 1024);
        assert_eq!(dtb_addr & 0xFFF, 0); // Aligned
        
        // Check DTB magic (FDT is big-endian, so we read bytes or swap)
        // 0xd00dfeed stored as [d0, 0d, fe, ed]
        // read32 (LE) reads as 0xedfe0dd0
        let magic_val = sys.memory.read32(dtb_addr);
        assert_eq!(magic_val.to_be(), 0xd00dfeed);
    }
}
