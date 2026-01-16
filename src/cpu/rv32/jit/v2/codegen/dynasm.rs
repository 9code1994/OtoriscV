//! Dynasm-rs native JIT backend for x86_64
//!
//! Generates x86_64 machine code directly for RISC-V basic blocks.
//! Only available on native CLI builds with `jit-dynasm` feature.

#[cfg(all(not(target_arch = "wasm32"), feature = "jit-dynasm", target_arch = "x86_64"))]
mod backend {
    use dynasm::dynasm;
    use dynasmrt::{DynasmApi, DynasmLabelApi, x64::Assembler, ExecutableBuffer};
    use std::mem;
    
    use crate::cpu::rv32::icache::CachedInst;
    
    /// RISC-V opcodes
    mod rv_opcode {
        pub const OP_LUI: u8 = 0b0110111;
        pub const OP_AUIPC: u8 = 0b0010111;
        pub const OP_OP: u8 = 0b0110011;      // R-type ALU
        pub const OP_OP_IMM: u8 = 0b0010011;  // I-type ALU
    }
    
    /// Compiled native code block
    pub struct NativeBlock {
        /// Executable buffer containing x86_64 code
        code: ExecutableBuffer,
        /// Function pointer to the compiled code
        /// Signature: fn(regs: *mut u32) -> u32 (returns next PC offset)
        func: unsafe extern "sysv64" fn(*mut u32) -> u32,
    }
    
    impl NativeBlock {
        /// Execute the compiled block
        /// Updates registers in place, returns instruction count executed
        pub fn execute(&self, regs: &mut [u32; 32]) -> u32 {
            unsafe { (self.func)(regs.as_mut_ptr()) }
        }
    }
    
    /// Compile a sequence of instructions to native x86_64 code
    pub fn compile_block(instructions: &[CachedInst]) -> Option<NativeBlock> {
        let mut ops = Assembler::new().ok()?;
        
        // x86_64 SysV calling convention:
        // RDI = first arg (pointer to regs array)
        // We'll use:
        //   RDI = regs pointer
        //   RAX, RCX, RDX = scratch
        //   R8-R15 = could cache RISC-V regs but we'll use memory for simplicity
        
        let mut inst_count = 0u32;
        
        for inst in instructions {
            if !emit_instruction(&mut ops, inst) {
                // Instruction not supported, abort compilation
                return None;
            }
            inst_count += 1;
        }
        
        // Return instruction count
        dynasm!(ops
            ; mov eax, inst_count as i32
            ; ret
        );
        
        let code = ops.finalize().ok()?;
        let func: unsafe extern "sysv64" fn(*mut u32) -> u32 = unsafe {
            mem::transmute(code.ptr(dynasmrt::AssemblyOffset(0)))
        };
        
        Some(NativeBlock { code, func })
    }
    
    /// Emit x86_64 code for a single RISC-V instruction
    fn emit_instruction(ops: &mut Assembler, inst: &CachedInst) -> bool {
        use rv_opcode::*;
        
        match inst.opcode {
            // LUI rd, imm
            OP_LUI if inst.rd != 0 => {
                let imm = (inst.raw & 0xFFFFF000) as i32;
                let rd_off = (inst.rd as i32) * 4;
                dynasm!(ops
                    ; mov DWORD [rdi + rd_off], imm
                );
                true
            }
            
            // R-type ALU
            OP_OP if inst.rd != 0 => {
                emit_r_type_alu(ops, inst)
            }
            
            // I-type ALU
            OP_OP_IMM if inst.rd != 0 => {
                emit_i_type_alu(ops, inst)
            }
            
            // x0 writes are NOPs
            OP_LUI | OP_OP | OP_OP_IMM if inst.rd == 0 => true,
            
            // Unsupported
            _ => false,
        }
    }
    
    /// Emit R-type ALU instruction
    fn emit_r_type_alu(ops: &mut Assembler, inst: &CachedInst) -> bool {
        let rs1_off = (inst.rs1 as i32) * 4;
        let rs2_off = (inst.rs2 as i32) * 4;
        let rd_off = (inst.rd as i32) * 4;
        
        // Load rs1 into eax
        dynasm!(ops
            ; mov eax, [rdi + rs1_off]
        );
        
        match (inst.funct3, inst.funct7) {
            // ADD
            (0b000, 0b0000000) => {
                dynasm!(ops
                    ; add eax, [rdi + rs2_off]
                );
            }
            // SUB
            (0b000, 0b0100000) => {
                dynasm!(ops
                    ; sub eax, [rdi + rs2_off]
                );
            }
            // AND
            (0b111, 0b0000000) => {
                dynasm!(ops
                    ; and eax, [rdi + rs2_off]
                );
            }
            // OR
            (0b110, 0b0000000) => {
                dynasm!(ops
                    ; or eax, [rdi + rs2_off]
                );
            }
            // XOR
            (0b100, 0b0000000) => {
                dynasm!(ops
                    ; xor eax, [rdi + rs2_off]
                );
            }
            // SLL
            (0b001, 0b0000000) => {
                dynasm!(ops
                    ; mov ecx, [rdi + rs2_off]
                    ; shl eax, cl
                );
            }
            // SRL
            (0b101, 0b0000000) => {
                dynasm!(ops
                    ; mov ecx, [rdi + rs2_off]
                    ; shr eax, cl
                );
            }
            // SRA
            (0b101, 0b0100000) => {
                dynasm!(ops
                    ; mov ecx, [rdi + rs2_off]
                    ; sar eax, cl
                );
            }
            // SLT (signed)
            (0b010, 0b0000000) => {
                dynasm!(ops
                    ; cmp eax, [rdi + rs2_off]
                    ; setl al
                    ; movzx eax, al
                );
            }
            // SLTU (unsigned)
            (0b011, 0b0000000) => {
                dynasm!(ops
                    ; cmp eax, [rdi + rs2_off]
                    ; setb al
                    ; movzx eax, al
                );
            }
            _ => return false,
        }
        
        // Store result to rd
        dynasm!(ops
            ; mov [rdi + rd_off], eax
        );
        
        true
    }
    
    /// Emit I-type ALU instruction
    fn emit_i_type_alu(ops: &mut Assembler, inst: &CachedInst) -> bool {
        let rs1_off = (inst.rs1 as i32) * 4;
        let rd_off = (inst.rd as i32) * 4;
        let imm = ((inst.raw as i32) >> 20) as i32;
        
        // Load rs1 into eax
        dynasm!(ops
            ; mov eax, [rdi + rs1_off]
        );
        
        match inst.funct3 {
            // ADDI
            0b000 => {
                dynasm!(ops
                    ; add eax, imm
                );
            }
            // ANDI
            0b111 => {
                dynasm!(ops
                    ; and eax, imm
                );
            }
            // ORI
            0b110 => {
                dynasm!(ops
                    ; or eax, imm
                );
            }
            // XORI
            0b100 => {
                dynasm!(ops
                    ; xor eax, imm
                );
            }
            // SLTI
            0b010 => {
                dynasm!(ops
                    ; cmp eax, imm
                    ; setl al
                    ; movzx eax, al
                );
            }
            // SLTIU
            0b011 => {
                dynasm!(ops
                    ; cmp eax, imm
                    ; setb al
                    ; movzx eax, al
                );
            }
            // SLLI
            0b001 => {
                let shamt = (imm & 0x1F) as i8;
                dynasm!(ops
                    ; shl eax, shamt
                );
            }
            // SRLI / SRAI
            0b101 => {
                let shamt = (imm & 0x1F) as i8;
                if (imm >> 10) & 1 == 1 {
                    dynasm!(ops
                        ; sar eax, shamt
                    );
                } else {
                    dynasm!(ops
                        ; shr eax, shamt
                    );
                }
            }
            _ => return false,
        }
        
        // Store result to rd
        dynasm!(ops
            ; mov [rdi + rd_off], eax
        );
        
        true
    }
    
    /// Check if a block can be compiled (ALU-only)
    pub fn can_compile(instructions: &[CachedInst]) -> bool {
        use rv_opcode::*;
        
        for inst in instructions {
            match inst.opcode {
                OP_LUI | OP_OP | OP_OP_IMM => continue,
                _ => return false,
            }
        }
        true
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "jit-dynasm", target_arch = "x86_64"))]
pub use backend::*;

// Stub for when dynasm is not enabled
#[cfg(not(all(not(target_arch = "wasm32"), feature = "jit-dynasm", target_arch = "x86_64")))]
pub fn can_compile(_instructions: &[crate::cpu::rv32::icache::CachedInst]) -> bool {
    false
}
