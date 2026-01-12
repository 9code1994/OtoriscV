//! Instruction decoder
//!
//! Decodes RV32IMA instructions

/// Decoded instruction fields
#[derive(Debug)]
pub struct DecodedInst {
    pub opcode: u32,
    pub rd: u32,
    pub rs1: u32,
    pub rs2: u32,
    pub funct3: u32,
    pub funct7: u32,
    pub imm_i: i32,
    pub imm_s: i32,
    pub imm_b: i32,
    pub imm_u: i32,
    pub imm_j: i32,
}

impl DecodedInst {
    #[inline(always)]
    pub fn decode(inst: u32) -> Self {
        let opcode = inst & 0x7F;
        let rd = (inst >> 7) & 0x1F;
        let rs1 = (inst >> 15) & 0x1F;
        let rs2 = (inst >> 20) & 0x1F;
        let funct3 = (inst >> 12) & 0x7;
        let funct7 = (inst >> 25) & 0x7F;
        
        // I-type immediate
        let imm_i = (inst as i32) >> 20;
        
        // S-type immediate
        let imm_s = ((inst & 0xFE000000) as i32 >> 20) | ((inst >> 7) & 0x1F) as i32;
        
        // B-type immediate
        let imm_b = ((inst & 0x80000000) as i32 >> 19) |
                    (((inst >> 7) & 1) << 11) as i32 |
                    (((inst >> 25) & 0x3F) << 5) as i32 |
                    (((inst >> 8) & 0xF) << 1) as i32;
        
        // U-type immediate
        let imm_u = (inst & 0xFFFFF000) as i32;
        
        // J-type immediate
        let imm_j = ((inst & 0x80000000) as i32 >> 11) |
                    (inst & 0xFF000) as i32 |
                    (((inst >> 20) & 1) << 11) as i32 |
                    (((inst >> 21) & 0x3FF) << 1) as i32;
        
        DecodedInst {
            opcode,
            rd,
            rs1,
            rs2,
            funct3,
            funct7,
            imm_i,
            imm_s,
            imm_b,
            imm_u,
            imm_j,
        }
    }
}

// Opcodes
pub const OP_LUI: u32 = 0b0110111;
pub const OP_AUIPC: u32 = 0b0010111;
pub const OP_JAL: u32 = 0b1101111;
pub const OP_JALR: u32 = 0b1100111;
pub const OP_BRANCH: u32 = 0b1100011;
pub const OP_LOAD: u32 = 0b0000011;
pub const OP_STORE: u32 = 0b0100011;
pub const OP_OP_IMM: u32 = 0b0010011;
pub const OP_OP: u32 = 0b0110011;
pub const OP_MISC_MEM: u32 = 0b0001111;
pub const OP_SYSTEM: u32 = 0b1110011;
pub const OP_AMO: u32 = 0b0101111;

// Branch funct3
pub const FUNCT3_BEQ: u32 = 0b000;
pub const FUNCT3_BNE: u32 = 0b001;
pub const FUNCT3_BLT: u32 = 0b100;
pub const FUNCT3_BGE: u32 = 0b101;
pub const FUNCT3_BLTU: u32 = 0b110;
pub const FUNCT3_BGEU: u32 = 0b111;

// Load/Store funct3
pub const FUNCT3_LB: u32 = 0b000;
pub const FUNCT3_LH: u32 = 0b001;
pub const FUNCT3_LW: u32 = 0b010;
pub const FUNCT3_LBU: u32 = 0b100;
pub const FUNCT3_LHU: u32 = 0b101;

// ALU funct3
pub const FUNCT3_ADD_SUB: u32 = 0b000;
pub const FUNCT3_SLL: u32 = 0b001;
pub const FUNCT3_SLT: u32 = 0b010;
pub const FUNCT3_SLTU: u32 = 0b011;
pub const FUNCT3_XOR: u32 = 0b100;
pub const FUNCT3_SRL_SRA: u32 = 0b101;
pub const FUNCT3_OR: u32 = 0b110;
pub const FUNCT3_AND: u32 = 0b111;

// M extension funct3
pub const FUNCT3_MUL: u32 = 0b000;
pub const FUNCT3_MULH: u32 = 0b001;
pub const FUNCT3_MULHSU: u32 = 0b010;
pub const FUNCT3_MULHU: u32 = 0b011;
pub const FUNCT3_DIV: u32 = 0b100;
pub const FUNCT3_DIVU: u32 = 0b101;
pub const FUNCT3_REM: u32 = 0b110;
pub const FUNCT3_REMU: u32 = 0b111;

// System funct3
pub const FUNCT3_PRIV: u32 = 0b000;
pub const FUNCT3_CSRRW: u32 = 0b001;
pub const FUNCT3_CSRRS: u32 = 0b010;
pub const FUNCT3_CSRRC: u32 = 0b011;
pub const FUNCT3_CSRRWI: u32 = 0b101;
pub const FUNCT3_CSRRSI: u32 = 0b110;
pub const FUNCT3_CSRRCI: u32 = 0b111;

// AMO funct5
pub const FUNCT5_LR: u32 = 0b00010;
pub const FUNCT5_SC: u32 = 0b00011;
pub const FUNCT5_AMOSWAP: u32 = 0b00001;
pub const FUNCT5_AMOADD: u32 = 0b00000;
pub const FUNCT5_AMOXOR: u32 = 0b00100;
pub const FUNCT5_AMOAND: u32 = 0b01100;
pub const FUNCT5_AMOOR: u32 = 0b01000;
pub const FUNCT5_AMOMIN: u32 = 0b10000;
pub const FUNCT5_AMOMAX: u32 = 0b10100;
pub const FUNCT5_AMOMINU: u32 = 0b11000;
pub const FUNCT5_AMOMAXU: u32 = 0b11100;
