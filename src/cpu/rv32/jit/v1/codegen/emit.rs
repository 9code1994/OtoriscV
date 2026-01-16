//! WASM instruction emitter for RV32I ALU operations
//! 
//! Generates WASM bytecode from RISC-V basic blocks.
//! For now, only handles pure ALU operations (register-register).
//! Memory and system calls fall back to interpreter.

#[cfg(target_arch = "wasm32")]
use super::wasm::WasmBuilder;

/// RISC-V opcodes
#[cfg(target_arch = "wasm32")]
mod rv_opcode {
    pub const OP_LUI: u8 = 0b0110111;
    pub const OP_AUIPC: u8 = 0b0010111;
    pub const OP_OP: u8 = 0b0110011;      // R-type ALU
    pub const OP_OP_IMM: u8 = 0b0010011;  // I-type ALU
    pub const OP_LOAD: u8 = 0b0000011;
    pub const OP_STORE: u8 = 0b0100011;
    pub const OP_BRANCH: u8 = 0b1100011;
    pub const OP_JAL: u8 = 0b1101111;
    pub const OP_JALR: u8 = 0b1100111;
    pub const OP_SYSTEM: u8 = 0b1110011;
}

/// Result of emitting a block
#[cfg(target_arch = "wasm32")]
pub enum EmitResult {
    /// Successfully emitted all instructions
    Success,
    /// Contains unsupported instruction, need fallback
    NeedsFallback,
}

/// Emit WASM code for a single RISC-V instruction
/// 
/// Registers are represented as WASM locals 0-31.
/// Returns false if instruction requires fallback to interpreter.
#[cfg(target_arch = "wasm32")]
pub fn emit_instruction(
    builder: &mut WasmBuilder,
    inst: &crate::cpu::rv32::icache::CachedInst,
    _pc: u32,
) -> bool {
    use rv_opcode::*;
    
    match inst.opcode {
        // LUI rd, imm
        OP_LUI if inst.rd != 0 => {
            let imm = (inst.raw & 0xFFFFF000) as i32;
            builder.i32_const(imm);
            builder.local_set(inst.rd as u32);
            true
        }
        
        // AUIPC rd, imm - needs PC, skip for now
        OP_AUIPC => false,
        
        // R-type ALU operations
        OP_OP if inst.rd != 0 => {
            emit_r_type_alu(builder, inst)
        }
        
        // I-type ALU operations
        OP_OP_IMM if inst.rd != 0 => {
            emit_i_type_alu(builder, inst)
        }
        
        // x0 writes are NOPs
        OP_LUI | OP_OP | OP_OP_IMM if inst.rd == 0 => true,
        
        // Memory and control flow need fallback
        OP_LOAD | OP_STORE | OP_BRANCH | OP_JAL | OP_JALR | OP_SYSTEM => false,
        
        _ => false,
    }
}

/// Emit R-type ALU instruction (ADD, SUB, AND, OR, XOR, etc.)
#[cfg(target_arch = "wasm32")]
fn emit_r_type_alu(
    builder: &mut WasmBuilder,
    inst: &crate::cpu::rv32::icache::CachedInst,
) -> bool {
    // Load rs1 and rs2
    builder.local_get(inst.rs1 as u32);
    builder.local_get(inst.rs2 as u32);
    
    match (inst.funct3, inst.funct7) {
        // ADD
        (0b000, 0b0000000) => builder.i32_add(),
        // SUB
        (0b000, 0b0100000) => builder.i32_sub(),
        // AND
        (0b111, 0b0000000) => builder.i32_and(),
        // OR
        (0b110, 0b0000000) => builder.i32_or(),
        // XOR
        (0b100, 0b0000000) => builder.i32_xor(),
        // SLL
        (0b001, 0b0000000) => builder.i32_shl(),
        // SRL
        (0b101, 0b0000000) => builder.i32_shr_u(),
        // SRA
        (0b101, 0b0100000) => builder.i32_shr_s(),
        // SLT (signed less than)
        (0b010, 0b0000000) => builder.i32_lt_s(),
        // SLTU (unsigned less than)
        (0b011, 0b0000000) => builder.i32_lt_u(),
        // Unknown - fallback
        _ => return false,
    }
    
    // Store to rd
    builder.local_set(inst.rd as u32);
    true
}

/// Emit I-type ALU instruction (ADDI, ANDI, ORI, etc.)
#[cfg(target_arch = "wasm32")]
fn emit_i_type_alu(
    builder: &mut WasmBuilder,
    inst: &crate::cpu::rv32::icache::CachedInst,
) -> bool {
    // Extract I-type immediate (sign-extended)
    let imm = ((inst.raw as i32) >> 20) as i32;
    
    // Load rs1
    builder.local_get(inst.rs1 as u32);
    
    match inst.funct3 {
        // ADDI
        0b000 => {
            builder.i32_const(imm);
            builder.i32_add();
        }
        // ANDI
        0b111 => {
            builder.i32_const(imm);
            builder.i32_and();
        }
        // ORI
        0b110 => {
            builder.i32_const(imm);
            builder.i32_or();
        }
        // XORI
        0b100 => {
            builder.i32_const(imm);
            builder.i32_xor();
        }
        // SLTI (signed less than immediate)
        0b010 => {
            builder.i32_const(imm);
            builder.i32_lt_s();
        }
        // SLTIU (unsigned less than immediate)
        0b011 => {
            builder.i32_const(imm);
            builder.i32_lt_u();
        }
        // SLLI
        0b001 => {
            let shamt = (imm & 0x1F) as i32;
            builder.i32_const(shamt);
            builder.i32_shl();
        }
        // SRLI / SRAI
        0b101 => {
            let shamt = (imm & 0x1F) as i32;
            builder.i32_const(shamt);
            if (imm >> 10) & 1 == 1 {
                builder.i32_shr_s(); // SRAI
            } else {
                builder.i32_shr_u(); // SRLI
            }
        }
        _ => return false,
    }
    
    // Store to rd
    builder.local_set(inst.rd as u32);
    true
}

/// Check if a block can be compiled to WASM (ALU-only)
#[cfg(target_arch = "wasm32")]
pub fn can_compile_block(block: &super::super::CompiledBlock) -> bool {
    use rv_opcode::*;
    
    for inst in &block.instructions {
        match inst.opcode {
            OP_LUI | OP_OP | OP_OP_IMM => continue,
            _ => return false,
        }
    }
    
    true
}
