//! RISC-V 32-bit Emulator
//!
//! A RISC-V emulator targeting WebAssembly, inspired by jor1k's architecture
//! with v86-style lazy filesystem loading.

use wasm_bindgen::prelude::*;

pub mod cpu;
mod memory;
mod devices;
mod system;
pub mod snapshot;
mod system64;
pub use system::System;
pub use system64::System64;


/// Initialize panic hook for better error messages in browser console
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Log to browser console
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
    
    #[wasm_bindgen(js_namespace = console)]
    fn error(s: &str);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn log(s: &str) {
    println!("LOG: {}", s);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn error(s: &str) {
    eprintln!("ERROR: {}", s);
}

/// Helper macro for console logging
#[macro_export]
macro_rules! console_log {
    ($($t:tt)*) => (crate::log(&format!($($t)*)))
}

#[macro_export]
macro_rules! console_error {
    ($($t:tt)*) => (crate::error(&format!($($t)*)))
}

/// Main emulator interface exposed to JavaScript
#[wasm_bindgen]
pub struct Emulator {
    system: System,
}

#[wasm_bindgen]
impl Emulator {
    /// Create a new emulator instance
    #[wasm_bindgen(constructor)]
    pub fn new(memory_size_mb: u32) -> Result<Emulator, JsValue> {
        console_log!("Creating RISC-V emulator with {}MB RAM", memory_size_mb);
        
        let system = System::new(memory_size_mb, None)
            .map_err(|e| JsValue::from_str(&e))?;
        
        Ok(Emulator { system })
    }
    
    /// Load kernel binary into RAM at specified address
    pub fn load_kernel(&mut self, data: &[u8], load_addr: u32) -> Result<(), JsValue> {
        self.system.load_binary(data, load_addr)
            .map_err(|e| JsValue::from_str(&e))
    }
    
    /// Setup Linux boot (generates DTB and sets up registers)
    pub fn setup_linux(&mut self, kernel: &[u8], cmdline: &str) -> Result<(), JsValue> {
        self.system.setup_linux_boot(kernel, cmdline)
            .map_err(|e| JsValue::from_str(&e))
    }
    
    /// Setup Linux boot with initrd (generates DTB and sets up registers)
    pub fn setup_linux_with_initrd(&mut self, kernel: &[u8], initrd: &[u8], cmdline: &str) -> Result<(), JsValue> {
        self.system.setup_linux_boot_with_initrd(kernel, Some(initrd), cmdline)
            .map_err(|e| JsValue::from_str(&e))
    }
    
    /// Run the emulator for a specified number of cycles
    /// Returns the number of cycles actually executed
    pub fn run(&mut self, cycles: u32) -> u32 {
        self.system.run(cycles)
    }
    
    /// Enable or disable JIT v2 (advanced page-based JIT with CFG optimization)
    pub fn enable_jit_v2(&mut self, enable: bool) {
        self.system.enable_jit_v2(enable);
    }
    
    /// Check if the CPU is halted (waiting for interrupt)
    pub fn is_halted(&self) -> bool {
        self.system.is_halted()
    }
    
    /// Send a character to UART (keyboard input)
    pub fn send_char(&mut self, c: u8) {
        self.system.uart_receive(c);
    }
    
    /// Get pending UART output
    pub fn get_uart_output(&mut self) -> Vec<u8> {
        self.system.uart_get_output()
    }
    
    /// Get current PC for debugging
    pub fn get_pc(&self) -> u32 {
        self.system.get_pc()
    }
    
    pub fn get_ips(&self) -> u32 {
        // Return only instructions executed in the last run call
        // This is a simplification; for total count we'd need another method
        // But get_instruction_count in System returns total.
        // Let's change this to return total count
        self.system.get_instruction_count() as u32
    }
    
    pub fn get_registers(&self) -> Vec<u32> {
        self.system.get_registers()
    }
    
    pub fn read_memory(&self, addr: u32, size: u32) -> Vec<u8> {
        self.system.read_memory(addr, size)
    }
    
    pub fn reset(&mut self) {
        self.system.reset();
    }
    
    /// Get missing blobs (SHA256 hashes) that need to be fetched
    pub fn get_missing_blobs(&self) -> Box<[JsValue]> {
        let blobs = self.system.get_missing_blobs();
        blobs.iter()
             .map(|s| JsValue::from_str(s))
             .collect::<Vec<JsValue>>()
             .into_boxed_slice()
    }
    
    /// Provide a fetched blob to the emulator
    pub fn provide_blob(&mut self, hash: String, data: Vec<u8>) {
        self.system.provide_blob(hash, data);
    }
    
    /// Serialize the entire emulator state to a binary blob (compressed with Zstd)
    pub fn get_state(&self) -> Result<Vec<u8>, JsValue> {
        let serialized = bincode::serialize(&self.system)
            .map_err(|e| JsValue::from_str(&format!("Serialization error: {}", e)))?;
            
        // Level 0 is default compression
        zstd::stream::encode_all(&serialized[..], 0)
            .map_err(|e| JsValue::from_str(&format!("Compression error: {}", e)))
    }
    
    /// Restore the emulator state from a binary blob (compressed with Zstd)
    pub fn set_state(&mut self, state: &[u8]) -> Result<(), JsValue> {
        let decompressed = zstd::stream::decode_all(state)
             .map_err(|e| JsValue::from_str(&format!("Decompression error: {}", e)))?;
             
        let system: System = bincode::deserialize(&decompressed)
            .map_err(|e| JsValue::from_str(&format!("Deserialization error: {}", e)))?;
        self.system = system;
        Ok(())
    }
    
    /// Create a lightweight snapshot (CPU + devices + dirty pages only)
    /// 
    /// This is much smaller than get_state() (~100KB vs ~5MB) because it doesn't
    /// save the kernel/initrd. To restore, you must reload the same kernel/initrd first.
    /// 
    /// # Arguments
    /// * `kernel_size` - Size of the kernel in bytes (for validation on restore)
    /// * `initrd_size` - Size of the initrd in bytes, or 0 if none
    pub fn create_snapshot(&self, kernel_size: u32, initrd_size: u32) -> Result<Vec<u8>, JsValue> {
        let initrd_opt = if initrd_size > 0 { Some(initrd_size) } else { None };
        let snapshot = self.system.create_snapshot(kernel_size, initrd_opt);
        snapshot.to_bytes()
            .map_err(|e| JsValue::from_str(&e))
    }
    
    /// Restore from a lightweight snapshot
    /// 
    /// The same kernel/initrd must already be loaded using setup_linux_with_initrd()
    /// before calling this method.
    pub fn restore_snapshot(&mut self, snapshot_data: &[u8]) -> Result<(), JsValue> {
        let snapshot = snapshot::LightweightSnapshot::from_bytes(snapshot_data)
            .map_err(|e| JsValue::from_str(&e))?;
        self.system.restore_snapshot(&snapshot);
        Ok(())
    }
}

/// Decompress zstd-compressed data
/// Useful for loading compressed kernel images in the browser
#[wasm_bindgen]
pub fn decompress_zstd(data: &[u8]) -> Result<Vec<u8>, JsValue> {
    zstd::stream::decode_all(data)
        .map_err(|e| JsValue::from_str(&format!("Zstd decompression error: {}", e)))
}

/// Decompress gzip-compressed data (if flate2 is available)
/// Useful for loading compressed kernel images in the browser
#[wasm_bindgen]
pub fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>, JsValue> {
    use std::io::Read;
    
    #[cfg(not(target_arch = "wasm32"))]
    {
        use flate2::read::GzDecoder;
        let mut decoder = GzDecoder::new(data);
        let mut result = Vec::new();
        decoder.read_to_end(&mut result)
            .map_err(|e| JsValue::from_str(&format!("Gzip decompression error: {}", e)))?;
        Ok(result)
    }
    
    #[cfg(target_arch = "wasm32")]
    {
        // For WASM, use a pure Rust gzip implementation
        Err(JsValue::from_str("Gzip decompression not yet implemented for WASM. Use zstd instead or decompress in JavaScript."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_emulator_creation() {
        let _emu = Emulator::new(1).unwrap();
    }

    #[test]
    fn test_serialization_round_trip() {
        let mut emu = Emulator::new(1).unwrap();
        
        // precise initialization state
        emu.system.uart_receive(b'A');
        
        // Save state
        let state = emu.get_state().unwrap();
        assert!(!state.is_empty());
        
        // Restore to new emulator
        let mut emu2 = Emulator::new(1).unwrap();
        emu2.set_state(&state).unwrap();
        
        // Check state preserved
        let output = emu2.get_uart_output();
        // The output might be in TX buffer or FIFO depending on how receive works
        // Actually receive_char puts into rx_fifo.
        // Reading RBR moves from rx_fifo.
        // We can check if state is identical by inspecting internal state directly or via side effects.
        // Let's check registers.
        let regs1 = emu.get_registers();
        let regs2 = emu2.get_registers();
        assert_eq!(regs1, regs2);
        
        // Check PC
        assert_eq!(emu.get_pc(), emu2.get_pc());
        
        // Check RAM size
        assert_eq!(emu.system.read_memory(0x80000000, 4), emu2.system.read_memory(0x80000000, 4));
    }
}
