//! Device Tree Blob (DTB) Generator
//!
//! Implements a minimal FDT (Flattened Device Tree) writer.
//! Structure: Header -> Reserve Map -> Structure Block -> Strings Block

use std::collections::HashMap;

const FDT_MAGIC: u32 = 0xd00dfeed;
const FDT_VERSION: u32 = 17;
const FDT_LAST_COMP_VERSION: u32 = 16;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

pub struct DtbBuilder {
    struct_buf: Vec<u8>,
    strings_buf: Vec<u8>,
    string_offsets: HashMap<String, u32>,
}

impl DtbBuilder {
    pub fn new() -> Self {
        DtbBuilder {
            struct_buf: Vec::new(),
            strings_buf: Vec::new(),
            string_offsets: HashMap::new(),
        }
    }

    pub fn begin_node(&mut self, name: &str) {
        self.push_u32(FDT_BEGIN_NODE);
        self.struct_buf.extend_from_slice(name.as_bytes());
        self.struct_buf.push(0); // Null terminator
        self.align(4);
    }

    pub fn end_node(&mut self) {
        self.push_u32(FDT_END_NODE);
    }

    pub fn property_u32(&mut self, name: &str, value: u32) {
        self.property(name, &value.to_be_bytes());
    }
    
    pub fn property_u64(&mut self, name: &str, value: u64) {
        self.property(name, &value.to_be_bytes());
    }

    pub fn property_null(&mut self, name: &str) {
        self.property(name, &[]);
    }
    
    pub fn property_string(&mut self, name: &str, value: &str) {
        let mut data = value.as_bytes().to_vec();
        data.push(0); // Null terminator
        self.property(name, &data);
    }
    
    pub fn property_array_u32(&mut self, name: &str, values: &[u32]) {
        let mut data = Vec::with_capacity(values.len() * 4);
        for v in values {
            data.extend_from_slice(&v.to_be_bytes());
        }
        self.property(name, &data);
    }

    pub fn property(&mut self, name: &str, data: &[u8]) {
        self.push_u32(FDT_PROP);
        self.push_u32(data.len() as u32);
        
        let name_off = self.get_string_offset(name);
        self.push_u32(name_off);
        
        self.struct_buf.extend_from_slice(data);
        self.align(4);
    }

    fn push_u32(&mut self, v: u32) {
        self.struct_buf.extend_from_slice(&v.to_be_bytes());
    }

    fn align(&mut self, alignment: usize) {
        while self.struct_buf.len() % alignment != 0 {
            self.struct_buf.push(0);
        }
    }

    fn get_string_offset(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.string_offsets.get(s) {
            return off;
        }
        
        let off = self.strings_buf.len() as u32;
        self.strings_buf.extend_from_slice(s.as_bytes());
        self.strings_buf.push(0);
        self.string_offsets.insert(s.to_string(), off);
        off
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.push_u32(FDT_END);

        let off_mem_rsvmap = 40; // Header size
        let rsvmap_size = 16; // One empty entry (8 bytes address + 8 bytes size)
                               // Actually specs say list terminated by 0 address and 0 size.
                               // So just 1 entry of 0,0 is enough to terminate?
                               // Standard is (address, size) pairs. (0,0) terminates.
        
        let off_dt_struct = off_mem_rsvmap + rsvmap_size;
        let size_dt_struct = self.struct_buf.len() as u32;
        
        let off_dt_strings = off_dt_struct + size_dt_struct;
        let size_dt_strings = self.strings_buf.len() as u32;
        
        let totalsize = off_dt_strings + size_dt_strings;

        let mut final_buf = Vec::with_capacity(totalsize as usize);

        // Header
        final_buf.extend_from_slice(&FDT_MAGIC.to_be_bytes());
        final_buf.extend_from_slice(&totalsize.to_be_bytes());
        final_buf.extend_from_slice(&off_dt_struct.to_be_bytes());
        final_buf.extend_from_slice(&off_dt_strings.to_be_bytes());
        final_buf.extend_from_slice(&off_mem_rsvmap.to_be_bytes());
        final_buf.extend_from_slice(&FDT_VERSION.to_be_bytes());
        final_buf.extend_from_slice(&FDT_LAST_COMP_VERSION.to_be_bytes());
        final_buf.extend_from_slice(&0u32.to_be_bytes()); // boot_cpuid_phys
        final_buf.extend_from_slice(&size_dt_strings.to_be_bytes());
        final_buf.extend_from_slice(&size_dt_struct.to_be_bytes());

        // Reserve Map
        final_buf.extend_from_slice(&0u64.to_be_bytes());
        final_buf.extend_from_slice(&0u64.to_be_bytes());

        // Struct Block
        final_buf.extend_from_slice(&self.struct_buf);
        

        // Strings Block
        final_buf.extend_from_slice(&self.strings_buf);

        final_buf
    }
}

/// Generate the Device Tree Blob for our emulator
pub fn generate_fdt(ram_size_mb: u32, cmdline: &str) -> Vec<u8> {
    let mut dtb = DtbBuilder::new();
    
    // Root node
    dtb.begin_node("");
    dtb.property_u32("#address-cells", 2);
    dtb.property_u32("#size-cells", 2);
    dtb.property_string("compatible", "riscv-emu");
    dtb.property_string("model", "riscv-emu");

    // /chosen
    dtb.begin_node("chosen");
    dtb.property_string("bootargs", cmdline);
    dtb.end_node();
    
    // /cpus
    dtb.begin_node("cpus");
    dtb.property_u32("#address-cells", 1);
    dtb.property_u32("#size-cells", 0);
    dtb.property_u32("timebase-frequency", 10_000_000); // 10 MHz
    
        // /cpus/cpu@0
        dtb.begin_node("cpu@0");
        dtb.property_string("device_type", "cpu");
        dtb.property_u32("reg", 0);
        dtb.property_string("status", "okay");
        dtb.property_string("compatible", "riscv");
        dtb.property_string("riscv,isa", "rv32ima");
        dtb.property_string("mmu-type", "riscv,sv32");
        
            // /cpus/cpu@0/interrupt-controller
            dtb.begin_node("interrupt-controller");
            dtb.property_u32("#interrupt-cells", 1);
            dtb.property_null("interrupt-controller");
            dtb.property_string("compatible", "riscv,cpu-intc");
            dtb.property_u32("phandle", 1); // PHANDLE_CPU_INTC
            dtb.end_node();
            
        dtb.end_node(); // cpu@0
    dtb.end_node(); // cpus

    // /memory
    // Name should be memory@<base>
    dtb.begin_node("memory@80000000");
    dtb.property_string("device_type", "memory");
    // reg = <address_hi address_lo size_hi size_lo>
    // RAM Base: 0x80000000
    // RAM Size: ram_size_mb * 1024 * 1024
    let ram_size = (ram_size_mb as u64) * 1024 * 1024;
    dtb.property_array_u32("reg", &[0, 0x80000000, (ram_size >> 32) as u32, ram_size as u32]);
    dtb.end_node();

    // /soc
    dtb.begin_node("soc");
    dtb.property_u32("#address-cells", 2);
    dtb.property_u32("#size-cells", 2);
    dtb.property_string("compatible", "simple-bus");
    dtb.property_null("ranges");
    
        // CLINT
        dtb.begin_node("clint@2000000");
        dtb.property_string("compatible", "riscv,clint0");
        // Interrupts: M-mode SW (3) and Timer (7) for CPU 0
        // interrupts-extended = <&cpu0_intc 3 &cpu0_intc 7>
        dtb.property_array_u32("interrupts-extended", &[1, 3, 1, 7]); 
        dtb.property_array_u32("reg", &[0, 0x02000000, 0, 0x10000]);
        dtb.end_node();
        
        // PLIC
        dtb.begin_node("plic@4000000");
        dtb.property_string("compatible", "riscv,plic0");
        // Interrupts: M-mode Ext (11) and S-mode Ext (9) for CPU 0 (Context 1)
        // interrupts-extended = <&cpu0_intc 11 &cpu0_intc 9>
        dtb.property_array_u32("interrupts-extended", &[1, 11, 1, 9]); 
        dtb.property_array_u32("reg", &[0, 0x04000000, 0, 0x4000000]);
        dtb.property_u32("riscv,ndev", 32); 
        dtb.property_u32("#interrupt-cells", 1);
        dtb.property_null("interrupt-controller");
        dtb.property_u32("phandle", 2); // PHANDLE_PLIC
        dtb.end_node();
        
        // UART
        dtb.begin_node("uart@3000000");
        dtb.property_string("compatible", "ns16550a");
        dtb.property_array_u32("reg", &[0, 0x03000000, 0, 0x1000]);
        dtb.property_u32("interrupts", 10);
        dtb.property_u32("interrupt-parent", 2); // &plic
        dtb.property_u32("clock-frequency", 3686400); 
        dtb.end_node();
        
        // VirtIO
        dtb.begin_node("virtio@20000000");
        dtb.property_string("compatible", "virtio,mmio");
        dtb.property_array_u32("reg", &[0, 0x20000000, 0, 0x1000]);
        dtb.property_u32("interrupts", 1); // Assumed IRQ 1
        dtb.property_u32("interrupt-parent", 2); // &plic
        dtb.end_node();
        
    dtb.end_node(); // soc

    dtb.end_node(); // root

    dtb.finish()
}
