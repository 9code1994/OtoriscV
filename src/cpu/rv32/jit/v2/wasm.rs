//! WebAssembly code generation for JIT compiled regions

#[cfg(target_arch = "wasm32")]
pub mod builder {
    //! WebAssembly bytecode builder
    
    use std::collections::HashMap;
    
    /// WebAssembly opcodes
    pub mod op {
        pub const OP_UNREACHABLE: u8 = 0x00;
        pub const OP_NOP: u8 = 0x01;
        pub const OP_BLOCK: u8 = 0x02;
        pub const OP_LOOP: u8 = 0x03;
        pub const OP_IF: u8 = 0x04;
        pub const OP_ELSE: u8 = 0x05;
        pub const OP_END: u8 = 0x0B;
        pub const OP_BR: u8 = 0x0C;
        pub const OP_BR_IF: u8 = 0x0D;
        pub const OP_BR_TABLE: u8 = 0x0E;
        pub const OP_RETURN: u8 = 0x0F;
        pub const OP_CALL: u8 = 0x10;
        pub const OP_CALL_INDIRECT: u8 = 0x11;
        
        pub const OP_DROP: u8 = 0x1A;
        pub const OP_SELECT: u8 = 0x1B;
        
        pub const OP_LOCAL_GET: u8 = 0x20;
        pub const OP_LOCAL_SET: u8 = 0x21;
        pub const OP_LOCAL_TEE: u8 = 0x22;
        pub const OP_GLOBAL_GET: u8 = 0x23;
        pub const OP_GLOBAL_SET: u8 = 0x24;
        
        pub const OP_I32_LOAD: u8 = 0x28;
        pub const OP_I64_LOAD: u8 = 0x29;
        pub const OP_I32_LOAD8_S: u8 = 0x2C;
        pub const OP_I32_LOAD8_U: u8 = 0x2D;
        pub const OP_I32_LOAD16_S: u8 = 0x2E;
        pub const OP_I32_LOAD16_U: u8 = 0x2F;
        pub const OP_I32_STORE: u8 = 0x36;
        pub const OP_I32_STORE8: u8 = 0x3A;
        pub const OP_I32_STORE16: u8 = 0x3B;
        
        pub const OP_I32_CONST: u8 = 0x41;
        pub const OP_I64_CONST: u8 = 0x42;
        
        pub const OP_I32_EQZ: u8 = 0x45;
        pub const OP_I32_EQ: u8 = 0x46;
        pub const OP_I32_NE: u8 = 0x47;
        pub const OP_I32_LT_S: u8 = 0x48;
        pub const OP_I32_LT_U: u8 = 0x49;
        pub const OP_I32_GT_S: u8 = 0x4A;
        pub const OP_I32_GT_U: u8 = 0x4B;
        pub const OP_I32_LE_S: u8 = 0x4C;
        pub const OP_I32_LE_U: u8 = 0x4D;
        pub const OP_I32_GE_S: u8 = 0x4E;
        pub const OP_I32_GE_U: u8 = 0x4F;
        
        pub const OP_I32_CLZ: u8 = 0x67;
        pub const OP_I32_CTZ: u8 = 0x68;
        pub const OP_I32_POPCNT: u8 = 0x69;
        pub const OP_I32_ADD: u8 = 0x6A;
        pub const OP_I32_SUB: u8 = 0x6B;
        pub const OP_I32_MUL: u8 = 0x6C;
        pub const OP_I32_DIV_S: u8 = 0x6D;
        pub const OP_I32_DIV_U: u8 = 0x6E;
        pub const OP_I32_REM_S: u8 = 0x6F;
        pub const OP_I32_REM_U: u8 = 0x70;
        pub const OP_I32_AND: u8 = 0x71;
        pub const OP_I32_OR: u8 = 0x72;
        pub const OP_I32_XOR: u8 = 0x73;
        pub const OP_I32_SHL: u8 = 0x74;
        pub const OP_I32_SHR_S: u8 = 0x75;
        pub const OP_I32_SHR_U: u8 = 0x76;
        pub const OP_I32_ROTL: u8 = 0x77;
        pub const OP_I32_ROTR: u8 = 0x78;
        
        pub const TYPE_I32: u8 = 0x7F;
        pub const TYPE_I64: u8 = 0x7E;
        pub const TYPE_F32: u8 = 0x7D;
        pub const TYPE_F64: u8 = 0x7C;
        pub const TYPE_VOID: u8 = 0x40;
    }
    
    /// Label for structured control flow
    #[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
    pub struct Label(u32);
    
    /// WebAssembly module builder
    pub struct WasmBuilder {
        /// Output bytecode for function body
        code: Vec<u8>,
        /// Label stack for control flow
        label_stack: Vec<Label>,
        /// Next label ID
        next_label: u32,
        /// Label to stack depth mapping
        label_depths: HashMap<Label, usize>,
        /// Number of local variables
        local_count: u32,
    }
    
    impl WasmBuilder {
        pub fn new() -> Self {
            WasmBuilder {
                code: Vec::with_capacity(4096),
                label_stack: Vec::new(),
                next_label: 0,
                label_depths: HashMap::new(),
                local_count: 0,
            }
        }
        
        /// Reset builder for new function
        pub fn reset(&mut self) {
            self.code.clear();
            self.label_stack.clear();
            self.next_label = 0;
            self.label_depths.clear();
            self.local_count = 0;
        }
        
        /// Get generated code
        pub fn get_code(&self) -> &[u8] {
            &self.code
        }
        
        /// Allocate a new local variable
        pub fn alloc_local(&mut self) -> u32 {
            let idx = self.local_count;
            self.local_count += 1;
            idx
        }
        
        // === Control Flow ===
        
        /// Begin a block (forward jump target)
        pub fn block_void(&mut self) -> Label {
            let label = Label(self.next_label);
            self.next_label += 1;
            self.label_depths.insert(label, self.label_stack.len());
            self.label_stack.push(label);
            self.code.push(op::OP_BLOCK);
            self.code.push(op::TYPE_VOID);
            label
        }
        
        /// Begin a loop (backward jump target)
        pub fn loop_void(&mut self) -> Label {
            let label = Label(self.next_label);
            self.next_label += 1;
            self.label_depths.insert(label, self.label_stack.len());
            self.label_stack.push(label);
            self.code.push(op::OP_LOOP);
            self.code.push(op::TYPE_VOID);
            label
        }
        
        /// End a block or loop
        pub fn end(&mut self) {
            self.label_stack.pop();
            self.code.push(op::OP_END);
        }
        
        /// Unconditional branch
        pub fn br(&mut self, label: Label) {
            let depth = self.label_depth(label);
            self.code.push(op::OP_BR);
            self.write_leb128_u32(depth);
        }
        
        /// Conditional branch (if top of stack is non-zero)
        pub fn br_if(&mut self, label: Label) {
            let depth = self.label_depth(label);
            self.code.push(op::OP_BR_IF);
            self.write_leb128_u32(depth);
        }
        
        /// Branch table (switch)
        pub fn br_table(&mut self, labels: &[Label], default: Label) {
            self.code.push(op::OP_BR_TABLE);
            self.write_leb128_u32(labels.len() as u32);
            for &label in labels {
                let depth = self.label_depth(label);
                self.write_leb128_u32(depth);
            }
            let default_depth = self.label_depth(default);
            self.write_leb128_u32(default_depth);
        }
        
        /// If-then (condition on stack)
        pub fn if_void(&mut self) -> Label {
            let label = Label(self.next_label);
            self.next_label += 1;
            self.label_depths.insert(label, self.label_stack.len());
            self.label_stack.push(label);
            self.code.push(op::OP_IF);
            self.code.push(op::TYPE_VOID);
            label
        }
        
        /// Else branch
        pub fn else_(&mut self) {
            self.code.push(op::OP_ELSE);
        }
        
        /// Return from function
        pub fn return_(&mut self) {
            self.code.push(op::OP_RETURN);
        }
        
        // === Local Variables ===
        
        /// Get local variable
        pub fn local_get(&mut self, idx: u32) {
            self.code.push(op::OP_LOCAL_GET);
            self.write_leb128_u32(idx);
        }
        
        /// Set local variable
        pub fn local_set(&mut self, idx: u32) {
            self.code.push(op::OP_LOCAL_SET);
            self.write_leb128_u32(idx);
        }
        
        /// Tee local variable (set and keep on stack)
        pub fn local_tee(&mut self, idx: u32) {
            self.code.push(op::OP_LOCAL_TEE);
            self.write_leb128_u32(idx);
        }
        
        // === Constants ===
        
        /// Push i32 constant
        pub fn i32_const(&mut self, value: i32) {
            self.code.push(op::OP_I32_CONST);
            self.write_leb128_i32(value);
        }
        
        // === Arithmetic ===
        
        pub fn i32_add(&mut self) { self.code.push(op::OP_I32_ADD); }
        pub fn i32_sub(&mut self) { self.code.push(op::OP_I32_SUB); }
        pub fn i32_mul(&mut self) { self.code.push(op::OP_I32_MUL); }
        pub fn i32_div_s(&mut self) { self.code.push(op::OP_I32_DIV_S); }
        pub fn i32_div_u(&mut self) { self.code.push(op::OP_I32_DIV_U); }
        pub fn i32_rem_s(&mut self) { self.code.push(op::OP_I32_REM_S); }
        pub fn i32_rem_u(&mut self) { self.code.push(op::OP_I32_REM_U); }
        pub fn i32_and(&mut self) { self.code.push(op::OP_I32_AND); }
        pub fn i32_or(&mut self) { self.code.push(op::OP_I32_OR); }
        pub fn i32_xor(&mut self) { self.code.push(op::OP_I32_XOR); }
        pub fn i32_shl(&mut self) { self.code.push(op::OP_I32_SHL); }
        pub fn i32_shr_s(&mut self) { self.code.push(op::OP_I32_SHR_S); }
        pub fn i32_shr_u(&mut self) { self.code.push(op::OP_I32_SHR_U); }
        
        // === Comparison ===
        
        pub fn i32_eqz(&mut self) { self.code.push(op::OP_I32_EQZ); }
        pub fn i32_eq(&mut self) { self.code.push(op::OP_I32_EQ); }
        pub fn i32_ne(&mut self) { self.code.push(op::OP_I32_NE); }
        pub fn i32_lt_s(&mut self) { self.code.push(op::OP_I32_LT_S); }
        pub fn i32_lt_u(&mut self) { self.code.push(op::OP_I32_LT_U); }
        pub fn i32_gt_s(&mut self) { self.code.push(op::OP_I32_GT_S); }
        pub fn i32_gt_u(&mut self) { self.code.push(op::OP_I32_GT_U); }
        pub fn i32_le_s(&mut self) { self.code.push(op::OP_I32_LE_S); }
        pub fn i32_le_u(&mut self) { self.code.push(op::OP_I32_LE_U); }
        pub fn i32_ge_s(&mut self) { self.code.push(op::OP_I32_GE_S); }
        pub fn i32_ge_u(&mut self) { self.code.push(op::OP_I32_GE_U); }
        
        // === Memory ===
        
        /// Load i32 from memory (address on stack)
        pub fn i32_load(&mut self, align: u32, offset: u32) {
            self.code.push(op::OP_I32_LOAD);
            self.write_leb128_u32(align);
            self.write_leb128_u32(offset);
        }
        
        /// Store i32 to memory (address and value on stack)
        pub fn i32_store(&mut self, align: u32, offset: u32) {
            self.code.push(op::OP_I32_STORE);
            self.write_leb128_u32(align);
            self.write_leb128_u32(offset);
        }
        
        // === Helpers ===
        
        fn label_depth(&self, label: Label) -> u32 {
            let target_depth = *self.label_depths.get(&label).unwrap();
            (self.label_stack.len() - 1 - target_depth) as u32
        }
        
        fn write_leb128_u32(&mut self, mut value: u32) {
            loop {
                let byte = (value & 0x7F) as u8;
                value >>= 7;
                if value == 0 {
                    self.code.push(byte);
                    break;
                } else {
                    self.code.push(byte | 0x80);
                }
            }
        }
        
        fn write_leb128_i32(&mut self, mut value: i32) {
            loop {
                let byte = (value & 0x7F) as u8;
                value >>= 7;
                let done = (value == 0 && byte & 0x40 == 0) 
                        || (value == -1 && byte & 0x40 != 0);
                if done {
                    self.code.push(byte);
                    break;
                } else {
                    self.code.push(byte | 0x80);
                }
            }
        }
    }
    
    impl Default for WasmBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}
