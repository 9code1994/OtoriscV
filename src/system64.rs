//! System orchestrator for RV64
//!
//! Brings together CPU64, memory, and devices for 64-bit RISC-V execution

use crate::cpu::rv64::Cpu64;
use crate::cpu::rv64::csr::*;
use crate::memory::{Memory, Bus, DRAM_BASE};
use crate::devices::{Uart, Clint, Plic, Virtio9p};
use crate::devices::virtio_9p::{Backend, in_memory::InMemoryFileSystem};
#[cfg(not(target_arch = "wasm32"))]
use crate::devices::virtio_9p::host::HostFileSystem;
use serde::{Serialize, Deserialize};

// Device base addresses (matching QEMU virt machine)
const CLINT_BASE: u32 = 0x0200_0000;
const CLINT_SIZE: u32 = 0x0001_0000;
const UART_BASE: u32 = 0x1000_0000; // QEMU virt uses 0x10000000 for UART
const UART_SIZE: u32 = 0x0000_1000;
const PLIC_BASE: u32 = 0x0C00_0000; // QEMU virt PLIC at 0x0C000000
const PLIC_SIZE: u32 = 0x0400_0000;

// VirtIO devices
const VIRTIO_BASE: u32 = 0x1000_1000;
const VIRTIO_SIZE: u32 = 0x0000_1000;

// UART interrupt line on PLIC
const UART_IRQ: u32 = 10;

/// System64 state
#[derive(Serialize, Deserialize)]
pub struct System64 {
    pub cpu: Cpu64,
    memory: Memory,
    uart: Uart,
    pub clint: Clint,
    plic: Plic,
    virtio9p: Virtio9p,
}

impl System64 {
    /// Create a new 64-bit system with the specified RAM size
    pub fn new(ram_size_mb: u32, fs_path: Option<&str>) -> Result<Self, String> {
        if ram_size_mb == 0 || ram_size_mb > 2048 {
            return Err(format!("Invalid RAM size: {}MB", ram_size_mb));
        }

        let mut memory = Memory::new(ram_size_mb);
        memory.init_boot_rom_rv64(); // RV64-specific boot ROM

        let fs_backend = if let Some(_path) = fs_path {
            #[cfg(not(target_arch = "wasm32"))]
            {
                Backend::Host(HostFileSystem::new(_path))
            }
            #[cfg(target_arch = "wasm32")]
            {
                Backend::InMemory(InMemoryFileSystem::new())
            }
        } else {
            Backend::InMemory(InMemoryFileSystem::new())
        };

        Ok(System64 {
            cpu: Cpu64::new(),
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

    /// Setup system for Linux booting with optional initrd
    pub fn setup_linux_boot_with_initrd(&mut self, kernel: &[u8], initrd: Option<&[u8]>, cmdline: &str) -> Result<(), String> {
        // Load kernel at DRAM_BASE (0x80000000)
        self.load_binary(kernel, DRAM_BASE)?;

        let ram_size = self.memory.ram_size();
        let ram_size_mb = (ram_size / 1024 / 1024) as u32;
        let ram_end = DRAM_BASE + ram_size as u32;

        // Load initrd if provided
        let initrd_info = if let Some(initrd_data) = initrd {
            let dtb_reserve = 64 * 1024;
            let initrd_end = (ram_end - dtb_reserve) & !0xFFF;
            let initrd_start = (initrd_end - initrd_data.len() as u32) & !0xFFF;

            let kernel_end = DRAM_BASE + kernel.len() as u32;
            if initrd_start < kernel_end + 0x100000 {
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

        // Generate DTB
        let dtb = crate::devices::dtb::generate_fdt(ram_size_mb, cmdline, initrd_info);
        let dtb_addr = (ram_end - dtb.len() as u32) & !0xFFF;

        self.load_binary(&dtb, dtb_addr)?;
        println!("  DTB loaded at 0x{:08x} ({} bytes)", dtb_addr, dtb.len());

        // Setup CPU State for Linux boot via boot ROM
        // Boot ROM  at 0x1000 will:
        // 1. Set up medeleg/mideleg for exception delegation
        // 2. Set mtvec to SBI handler
        // 3. Set MPP=Supervisor in mstatus
        // 4. Set mepc=0x80000000 (kernel entry)
        // 5. Execute mret to drop to S-mode and start kernel
        //
        // Set up registers that Linux expects:
        // a0 (x10) = hartid (0)
        // a1 (x11) = dtb address
        self.cpu.reset();  // PC = 0x1000 (boot ROM)
        self.cpu.regs[10] = 0;               // a0 = hartid
        self.cpu.regs[11] = dtb_addr as u64; // a1 = dtb address

        Ok(())
    }

    /// Run the emulator for a specified number of cycles
    pub fn run(&mut self, max_cycles: u32) -> u32 {
        let debug = std::env::var("RISCV_DEBUG").is_ok();
        
        let mut cycles = 0u32;
        const TIMER_BATCH: u32 = 64;

        while cycles < max_cycles {
            if cycles & (TIMER_BATCH - 1) == 0 {
                self.clint.tick(TIMER_BATCH as u64);
                self.cpu.csr.time = self.clint.get_mtime();
                self.update_interrupts();

                if let Some(trap) = self.cpu.check_interrupts() {
                    self.cpu.handle_trap(trap);
                }
            }

            if self.cpu.wfi {
                let pending = self.cpu.csr.mip & self.cpu.csr.mie;
                if pending != 0 {
                    self.cpu.wfi = false;
                } else {
                    let ticks_to_timer = self.clint.ticks_until_interrupt();
                    if ticks_to_timer > 0 {
                        let skip = ticks_to_timer.min((max_cycles - cycles) as u64) as u32;
                        if skip > 1 {
                            self.clint.tick(skip as u64);
                            self.cpu.csr.time = self.clint.get_mtime();
                            cycles += skip;
                            continue;
                        }
                    }
                    cycles += 1;
                    continue;
                }
            }

            match self.step() {
                Ok(inst_count) => {
                    cycles += inst_count;
                    self.cpu.csr.cycle = self.cpu.csr.cycle.wrapping_add(inst_count as u64);
                }
                Err(trap) => {
                    // Handle SBI calls from S-mode directly in Rust
                    if matches!(trap, crate::cpu::rv64::trap::Trap64::EnvironmentCallFromS) {
                        if debug {
                            let eid = self.cpu.regs[17];
                            let a0 = self.cpu.regs[10];
                            eprintln!("[SBI] eid={:#x} a0={:#x} PC={:#018x}", eid, a0, self.cpu.pc);
                        }
                        self.handle_sbi_call();
                    } else {
                        if debug {
                            eprintln!("[TRAP] {:?} at PC={:#018x}", trap, self.cpu.pc);
                        }
                        self.cpu.handle_trap(trap);
                    }
                    cycles += 1;
                    self.cpu.csr.cycle = self.cpu.csr.cycle.wrapping_add(1);
                }
            }
        }

        cycles
    }

    fn step(&mut self) -> Result<u32, crate::cpu::rv64::trap::Trap64> {
        let mut bus = SystemBus64::new(
            &mut self.memory,
            &mut self.uart,
            &mut self.clint,
            &mut self.plic,
            &mut self.virtio9p,
        );

        self.cpu.step(&mut bus)?;
        drop(bus);
        self.virtio9p.process_queues(&mut self.memory);
        Ok(1)
    }

    fn update_interrupts(&mut self) {
        if self.clint.timer_interrupt {
            self.cpu.csr.mip |= MIP_MTIP;
            self.cpu.csr.mip |= MIP_STIP;
        } else {
            self.cpu.csr.mip &= !MIP_MTIP;
            self.cpu.csr.mip &= !MIP_STIP;
        }

        if self.clint.software_interrupt {
            self.cpu.csr.mip |= MIP_MSIP;
        } else {
            self.cpu.csr.mip &= !MIP_MSIP;
        }

        if self.uart.has_interrupt() {
            self.plic.raise_interrupt(UART_IRQ);
        } else {
            self.plic.clear_interrupt(UART_IRQ);
        }

        if self.plic.m_external_interrupt {
            self.cpu.csr.mip |= MIP_MEIP;
        } else {
            self.cpu.csr.mip &= !MIP_MEIP;
        }

        if self.plic.s_external_interrupt {
            self.cpu.csr.mip |= MIP_SEIP;
        } else {
            self.cpu.csr.mip &= !MIP_SEIP;
        }
    }

    fn handle_sbi_call(&mut self) {
        let eid = self.cpu.regs[17];  // a7 = Extension ID  
        let fid = self.cpu.regs[16];  // a6 = Function ID
        let a0 = self.cpu.regs[10];

        // SBI error codes
        const SBI_SUCCESS: u64 = 0;
        const SBI_ERR_NOT_SUPPORTED: u64 = (-2i64) as u64;
        
        // Extension IDs
        const SBI_EXT_LEGACY_SET_TIMER: u64 = 0;
        const SBI_EXT_LEGACY_CONSOLE_PUTCHAR: u64 = 1;
        const SBI_EXT_LEGACY_CONSOLE_GETCHAR: u64 = 2;
        const SBI_EXT_BASE: u64 = 0x10;
        const SBI_EXT_TIME: u64 = 0x54494D45;  // "TIME"
        const SBI_EXT_IPI: u64 = 0x735049;     // "sPI"
        const SBI_EXT_RFENCE: u64 = 0x52464E43; // "RFNC"
        const SBI_EXT_HSM: u64 = 0x48534D;     // "HSM"
        const SBI_EXT_SRST: u64 = 0x53525354;  // "SRST"
        
        let (error, value) = match eid {
            SBI_EXT_LEGACY_SET_TIMER => {
                // RV64: Timer is single 64-bit value in a0
                let timer_val = a0;
                self.clint.write32(0x4000, timer_val as u32);      // mtimecmp low
                self.clint.write32(0x4004, (timer_val >> 32) as u32);  // mtimecmp high
                (SBI_SUCCESS, 0)
            }
            
            SBI_EXT_LEGACY_CONSOLE_PUTCHAR => {
                self.uart.write8(0, a0 as u8);
                (SBI_SUCCESS, 0)
            }
            
            SBI_EXT_LEGACY_CONSOLE_GETCHAR => {
                ((-1i64) as u64, 0)
            }
            
            SBI_EXT_BASE => {
                match fid {
                    0 => (SBI_SUCCESS, 0x00000002),  // sbi_get_spec_version: SBI 0.2
                    1 => (SBI_SUCCESS, 0),            // sbi_get_impl_id: 0 = BBL
                    2 => (SBI_SUCCESS, 0),            // sbi_get_impl_version  
                    3 => {
                        // sbi_probe_extension
                        let probe_eid = a0;
                        let available = match probe_eid {
                            0 => 1,  // Legacy set_timer
                            1 => 1,  // Legacy console_putchar
                            2 => 1,  // Legacy console_getchar
                            0x10 => 1,  // SBI_EXT_BASE
                            0x54494D45 => 1,  // SBI_EXT_TIME
                            0x735049 => 0,    // SBI_EXT_IPI
                            0x52464E43 => 0,  // SBI_EXT_RFENCE
                            0x48534D => 0,    // SBI_EXT_HSM
                            0x53525354 => 0,  // SBI_EXT_SRST
                            _ => 0,
                        };
                        (SBI_SUCCESS, available)
                    }
                    4 => (SBI_SUCCESS, 0),  // sbi_get_mvendorid
                    5 => (SBI_SUCCESS, 0),  // sbi_get_marchid
                    6 => (SBI_SUCCESS, 0),  // sbi_get_mimpid
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                }
            }
            
            SBI_EXT_TIME => {
                match fid {
                    0 => {
                        // sbi_set_timer: RV64 has single 64-bit value in a0
                        let timer_val = a0;
                        self.clint.write32(0x4000, timer_val as u32);
                        self.clint.write32(0x4004, (timer_val >> 32) as u32);
                        (SBI_SUCCESS, 0)
                    }
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                }
            }
            
            SBI_EXT_IPI | SBI_EXT_RFENCE | SBI_EXT_HSM => {
                (SBI_SUCCESS, 0)
            }
            
            SBI_EXT_SRST => {
                match fid {
                    0 => {
                        eprintln!("SBI system reset requested");
                        self.cpu.wfi = true;
                        (SBI_SUCCESS, 0)
                    }
                    _ => (SBI_ERR_NOT_SUPPORTED, 0),
                }
            }
            
            _ => (SBI_ERR_NOT_SUPPORTED, 0),
        };
        
        // Set return values
        self.cpu.regs[10] = error;  // a0 = error
        self.cpu.regs[11] = value;  // a1 = value
        
        // Advance PC past ecall
        self.cpu.pc = self.cpu.pc.wrapping_add(4);
    }

    pub fn is_halted(&self) -> bool {
        self.cpu.wfi
    }

    pub fn uart_receive(&mut self, c: u8) {
        self.uart.receive_char(c);
    }

    pub fn uart_get_output(&mut self) -> Vec<u8> {
        self.uart.get_output()
    }

    pub fn get_pc(&self) -> u64 {
        self.cpu.pc
    }

    pub fn get_instruction_count(&self) -> u64 {
        self.cpu.instruction_count
    }

    pub fn get_tlb_stats(&self) -> (u64, u64) {
        self.cpu.mmu.tlb_stats()
    }

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

    pub fn reset(&mut self) {
        self.cpu.reset();
        self.memory.reset();
        self.uart.reset();
        self.clint.reset();
        self.plic.reset();
        self.virtio9p.reset();
    }
}

/// Bus implementation for RV64 system
struct SystemBus64<'a> {
    memory: &'a mut Memory,
    uart: &'a mut Uart,
    clint: &'a mut Clint,
    plic: &'a mut Plic,
    virtio9p: &'a mut Virtio9p,
    ram_size: usize,
}

impl<'a> SystemBus64<'a> {
    fn new(
        memory: &'a mut Memory,
        uart: &'a mut Uart,
        clint: &'a mut Clint,
        plic: &'a mut Plic,
        virtio9p: &'a mut Virtio9p,
    ) -> Self {
        let ram_size = memory.ram_size();
        SystemBus64 { memory, uart, clint, plic, virtio9p, ram_size }
    }

    #[inline(always)]
    fn ram_offset(&self, addr: u32) -> Option<usize> {
        if addr >= DRAM_BASE {
            let offset = (addr - DRAM_BASE) as usize;
            if offset < self.ram_size {
                return Some(offset);
            }
        }
        None
    }
}

impl<'a> Bus for SystemBus64<'a> {
    fn read8(&mut self, addr: u32) -> u8 {
        if let Some(offset) = self.ram_offset(addr) {
            return unsafe { self.memory.ram_read8_unchecked(offset) };
        }
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            return self.clint.read8(addr - CLINT_BASE);
        }
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            return self.uart.read8(addr - UART_BASE);
        }
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            return self.plic.read8(addr - PLIC_BASE);
        }
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            return self.virtio9p.read8(addr - VIRTIO_BASE);
        }
        self.memory.read8(addr)
    }

    fn write8(&mut self, addr: u32, value: u8) {
        if let Some(offset) = self.ram_offset(addr) {
            unsafe { self.memory.ram_write8_unchecked(offset, value) };
            return;
        }
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            self.clint.write8(addr - CLINT_BASE, value);
            return;
        }
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            self.uart.write8(addr - UART_BASE, value);
            return;
        }
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            self.plic.write8(addr - PLIC_BASE, value);
            return;
        }
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            self.virtio9p.write8(addr - VIRTIO_BASE, value);
            return;
        }
        self.memory.write8(addr, value);
    }

    fn read16(&mut self, addr: u32) -> u16 {
        if let Some(offset) = self.ram_offset(addr) {
            if offset + 1 < self.ram_size {
                return unsafe { self.memory.ram_read16_unchecked(offset) };
            }
        }
        let lo = self.read8(addr) as u16;
        let hi = self.read8(addr + 1) as u16;
        lo | (hi << 8)
    }

    fn write16(&mut self, addr: u32, value: u16) {
        if let Some(offset) = self.ram_offset(addr) {
            if offset + 1 < self.ram_size {
                unsafe { self.memory.ram_write16_unchecked(offset, value) };
                return;
            }
        }
        self.write8(addr, value as u8);
        self.write8(addr + 1, (value >> 8) as u8);
    }

    fn read32(&mut self, addr: u32) -> u32 {
        if let Some(offset) = self.ram_offset(addr) {
            if offset + 3 < self.ram_size {
                return unsafe { self.memory.ram_read32_unchecked(offset) };
            }
        }
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            return self.clint.read32(addr - CLINT_BASE);
        }
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            return self.uart.read32(addr - UART_BASE);
        }
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            return self.plic.read32(addr - PLIC_BASE);
        }
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            return self.virtio9p.read32(addr - VIRTIO_BASE);
        }
        self.memory.read32(addr)
    }

    fn write32(&mut self, addr: u32, value: u32) {
        if let Some(offset) = self.ram_offset(addr) {
            if offset + 3 < self.ram_size {
                unsafe { self.memory.ram_write32_unchecked(offset, value) };
                return;
            }
        }
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            self.clint.write32(addr - CLINT_BASE, value);
            return;
        }
        if addr >= UART_BASE && addr < UART_BASE + UART_SIZE {
            self.uart.write32(addr - UART_BASE, value);
            return;
        }
        if addr >= PLIC_BASE && addr < PLIC_BASE + PLIC_SIZE {
            self.plic.write32(addr - PLIC_BASE, value);
            return;
        }
        if addr >= VIRTIO_BASE && addr < VIRTIO_BASE + VIRTIO_SIZE {
            self.virtio9p.write32(addr - VIRTIO_BASE, value);
            return;
        }
        self.memory.write32(addr, value);
    }

    fn read64(&mut self, addr: u32) -> u64 {
        if let Some(offset) = self.ram_offset(addr) {
            if offset + 7 < self.ram_size {
                return unsafe { self.memory.ram_read64_unchecked(offset) };
            }
        }
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            let lo = self.clint.read32(addr - CLINT_BASE) as u64;
            let hi = self.clint.read32(addr - CLINT_BASE + 4) as u64;
            return lo | (hi << 32);
        }
        let lo = self.read32(addr) as u64;
        let hi = self.read32(addr + 4) as u64;
        lo | (hi << 32)
    }

    fn write64(&mut self, addr: u32, value: u64) {
        if let Some(offset) = self.ram_offset(addr) {
            if offset + 7 < self.ram_size {
                unsafe { self.memory.ram_write64_unchecked(offset, value) };
                return;
            }
        }
        if addr >= CLINT_BASE && addr < CLINT_BASE + CLINT_SIZE {
            self.clint.write32(addr - CLINT_BASE, value as u32);
            self.clint.write32(addr - CLINT_BASE + 4, (value >> 32) as u32);
            return;
        }
        self.write32(addr, value as u32);
        self.write32(addr + 4, (value >> 32) as u32);
    }
}
