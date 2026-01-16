//! WASM JIT runtime - JavaScript interop for module instantiation
//!
//! This module provides the bridge between Rust and JavaScript for
//! runtime WASM module compilation and execution.

use wasm_bindgen::prelude::*;

// Import JavaScript function to compile and instantiate WASM modules
#[wasm_bindgen]
extern "C" {
    /// Called from Rust to compile a WASM module in JavaScript
    /// Returns a module ID that can be used to call the function
    #[wasm_bindgen(js_namespace = window, js_name = "otoriscvCompileWasm")]
    fn js_compile_wasm(bytecode: &[u8]) -> u32;
    
    /// Called from Rust to execute a compiled WASM function
    /// Takes module ID and register state array
    /// Returns next PC
    #[wasm_bindgen(js_namespace = window, js_name = "otoriscvRunWasm")]
    fn js_run_wasm(module_id: u32, registers: &mut [u32]) -> u32;
    
    /// Free a compiled WASM module
    #[wasm_bindgen(js_namespace = window, js_name = "otoriscvFreeWasm")]
    fn js_free_wasm(module_id: u32);
}

/// Compiled WASM block handle
#[derive(Debug)]
pub struct CompiledWasmBlock {
    module_id: u32,
}

impl CompiledWasmBlock {
    /// Compile bytecode to WASM module
    pub fn compile(bytecode: &[u8]) -> Option<Self> {
        let module_id = js_compile_wasm(bytecode);
        if module_id == 0 {
            None
        } else {
            Some(CompiledWasmBlock { module_id })
        }
    }
    
    /// Execute the compiled block
    /// Updates registers in place, returns next PC
    pub fn execute(&self, registers: &mut [u32; 32]) -> u32 {
        js_run_wasm(self.module_id, registers)
    }
}

impl Drop for CompiledWasmBlock {
    fn drop(&mut self) {
        js_free_wasm(self.module_id);
    }
}
