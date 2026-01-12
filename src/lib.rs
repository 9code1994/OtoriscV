//! RISC-V 32-bit Emulator
//!
//! A RISC-V emulator targeting WebAssembly, inspired by jor1k's architecture
//! with v86-style lazy filesystem loading.

use wasm_bindgen::prelude::*;

mod cpu;
mod memory;
mod devices;
mod system;

pub use system::System;

/// Initialize panic hook for better error messages in browser console
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Log to browser console
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
    
    #[wasm_bindgen(js_namespace = console)]
    fn error(s: &str);
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
        
        let system = System::new(memory_size_mb)
            .map_err(|e| JsValue::from_str(&e))?;
        
        Ok(Emulator { system })
    }
    
    /// Load kernel binary into RAM at specified address
    pub fn load_kernel(&mut self, data: &[u8], load_addr: u32) -> Result<(), JsValue> {
        self.system.load_binary(data, load_addr)
            .map_err(|e| JsValue::from_str(&e))
    }
    
    /// Run the emulator for a specified number of cycles
    /// Returns the number of cycles actually executed
    pub fn run(&mut self, cycles: u32) -> u32 {
        self.system.run(cycles)
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
    
    /// Reset the emulator
    pub fn reset(&mut self) {
        self.system.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_emulator_creation() {
        // Basic test - actual testing requires wasm environment
    }
}
