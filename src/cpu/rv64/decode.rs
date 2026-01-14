//! Instruction decoder
//!
//! Decodes RV64IMAFD instructions

/// Decoded instruction fields
#[derive(Debug)]
pub struct DecodedInst {
    pub opcode: u32,
    pub rd: u32,
    pub rs1: u32,
    pub rs2: u32,
    pub rs3: u32,     // For R4-type (fused multiply-add)
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
        let rs3 = (inst >> 27) & 0x1F;  // For R4-type (fused multiply-add)
        let funct3 = (inst >> 12) & 0x7;
        let funct7 = (inst >> 25) & 0x7F;
        let imm_i = 0;
        let imm_s = 0;
        let imm_b = 0;
        let imm_u = 0;
        let imm_j = 0;

        DecodedInst {
            opcode,
            rd,
            rs1,
            rs2,
            rs3,
            funct3,
            funct7,
            imm_i,
            imm_s,
            imm_b,
            imm_u,
            imm_j,
        }
    }

    #[inline(always)]
    pub fn imm_i(inst: u32) -> i32 {
        (inst as i32) >> 20
    }

    #[inline(always)]
    pub fn imm_s(inst: u32) -> i32 {
        ((inst & 0xFE000000) as i32 >> 20) | ((inst >> 7) & 0x1F) as i32
    }

    #[inline(always)]
    pub fn imm_b(inst: u32) -> i32 {
        ((inst & 0x80000000) as i32 >> 19) |
            (((inst >> 7) & 1) << 11) as i32 |
            (((inst >> 25) & 0x3F) << 5) as i32 |
            (((inst >> 8) & 0xF) << 1) as i32
    }

    #[inline(always)]
    pub fn imm_u(inst: u32) -> i32 {
        (inst & 0xFFFFF000) as i32
    }

    #[inline(always)]
    pub fn imm_j(inst: u32) -> i32 {
        ((inst & 0x80000000) as i32 >> 11) |
            (inst & 0xFF000) as i32 |
            (((inst >> 20) & 1) << 11) as i32 |
            (((inst >> 21) & 0x3FF) << 1) as i32
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
pub const OP_OP_IMM_32: u32 = 0b0011011;
pub const OP_OP_32: u32 = 0b0111011;
pub const OP_MISC_MEM: u32 = 0b0001111;
pub const OP_SYSTEM: u32 = 0b1110011;
pub const OP_AMO: u32 = 0b0101111;

// Floating-point opcodes (F and D extensions)
pub const OP_LOAD_FP: u32 = 0b0000111;   // FLW, FLD
pub const OP_STORE_FP: u32 = 0b0100111;  // FSW, FSD
pub const OP_MADD: u32 = 0b1000011;      // FMADD.S, FMADD.D
pub const OP_MSUB: u32 = 0b1000111;      // FMSUB.S, FMSUB.D
pub const OP_NMSUB: u32 = 0b1001011;     // FNMSUB.S, FNMSUB.D
pub const OP_NMADD: u32 = 0b1001111;     // FNMADD.S, FNMADD.D
pub const OP_OP_FP: u32 = 0b1010011;     // All other FP operations

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
pub const FUNCT3_LWU: u32 = 0b110;
pub const FUNCT3_LD: u32 = 0b011;

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

// FP funct3 (width specifier for LOAD_FP/STORE_FP)
pub const FUNCT3_FLW: u32 = 0b010;   // Single-precision (32-bit)
pub const FUNCT3_FLD: u32 = 0b011;   // Double-precision (64-bit)

// FP funct7 (operation encoding for OP_FP)
pub const FUNCT7_FADD_S: u32 = 0b0000000;
pub const FUNCT7_FSUB_S: u32 = 0b0000100;
pub const FUNCT7_FMUL_S: u32 = 0b0001000;
pub const FUNCT7_FDIV_S: u32 = 0b0001100;
pub const FUNCT7_FSQRT_S: u32 = 0b0101100;
pub const FUNCT7_FSGNJ_S: u32 = 0b0010000;  // FSGNJ/FSGNJN/FSGNJX via funct3
pub const FUNCT7_FMIN_S: u32 = 0b0010100;   // FMIN/FMAX via funct3
pub const FUNCT7_FCVT_W_S: u32 = 0b1100000; // FCVT.W.S / FCVT.WU.S
pub const FUNCT7_FMV_X_W: u32 = 0b1110000;  // FMV.X.W / FCLASS.S
pub const FUNCT7_FCMP_S: u32 = 0b1010000;   // FEQ/FLT/FLE via funct3
pub const FUNCT7_FCVT_S_W: u32 = 0b1101000; // FCVT.S.W / FCVT.S.WU
pub const FUNCT7_FMV_W_X: u32 = 0b1111000;  // FMV.W.X
pub const FUNCT7_FMV_D_X: u32 = 0b1111001;  // FMV.D.X

// D extension funct7
pub const FUNCT7_FADD_D: u32 = 0b0000001;
pub const FUNCT7_FSUB_D: u32 = 0b0000101;
pub const FUNCT7_FMUL_D: u32 = 0b0001001;
pub const FUNCT7_FDIV_D: u32 = 0b0001101;
pub const FUNCT7_FSQRT_D: u32 = 0b0101101;
pub const FUNCT7_FSGNJ_D: u32 = 0b0010001;
pub const FUNCT7_FMIN_D: u32 = 0b0010101;
pub const FUNCT7_FCVT_S_D: u32 = 0b0100000; // FCVT.S.D
pub const FUNCT7_FCVT_D_S: u32 = 0b0100001; // FCVT.D.S
pub const FUNCT7_FCVT_W_D: u32 = 0b1100001; // FCVT.W.D / FCVT.WU.D
pub const FUNCT7_FCMP_D: u32 = 0b1010001;   // FEQ/FLT/FLE via funct3
pub const FUNCT7_FCLASS_D: u32 = 0b1110001; // FCLASS.D
pub const FUNCT7_FCVT_D_W: u32 = 0b1101001; // FCVT.D.W / FCVT.D.WU

// FP funct3 for sign injection
pub const FUNCT3_FSGNJ: u32 = 0b000;
pub const FUNCT3_FSGNJN: u32 = 0b001;
pub const FUNCT3_FSGNJX: u32 = 0b010;

// FP funct3 for min/max
pub const FUNCT3_FMIN: u32 = 0b000;
pub const FUNCT3_FMAX: u32 = 0b001;

// FP funct3 for comparison
pub const FUNCT3_FEQ: u32 = 0b010;
pub const FUNCT3_FLT: u32 = 0b001;
pub const FUNCT3_FLE: u32 = 0b000;

// FP fmt field (for fused multiply-add, bits [26:25])
pub const FMT_S: u32 = 0b00;  // Single-precision
pub const FMT_D: u32 = 0b01;  // Double-precision
