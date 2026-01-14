//! RV64 compressed (C) extension execution

use super::Cpu64;
use super::decode::*;
use super::trap::Trap64;
use crate::memory::Bus;

impl Cpu64 {
    pub fn execute_compressed(&mut self, inst16: u16, bus: &mut impl Bus) -> Result<(), Trap64> {
        let expanded = expand_compressed(inst16).ok_or(Trap64::IllegalInstruction(inst16 as u64))?;
        let pc_before = self.pc;
        self.execute(expanded, bus)?;
        if self.pc == pc_before.wrapping_add(4) {
            self.pc = pc_before.wrapping_add(2);
        }
        Ok(())
    }
}

fn expand_compressed(inst: u16) -> Option<u32> {
    let opcode = inst & 0b11;
    let funct3 = (inst >> 13) & 0b111;

    match (funct3, opcode) {
        (0b000, 0b00) => c_addi4spn(inst),
        (0b010, 0b00) => c_lw(inst),
        (0b011, 0b00) => c_ld(inst),
        (0b110, 0b00) => c_sw(inst),
        (0b111, 0b00) => c_sd(inst),

        (0b000, 0b01) => c_addi(inst),
        (0b001, 0b01) => c_addiw(inst),
        (0b010, 0b01) => c_li(inst),
        (0b011, 0b01) => c_addi16sp_lui(inst),
        (0b100, 0b01) => c_alu_imm(inst),
        (0b101, 0b01) => c_j(inst),
        (0b110, 0b01) => c_beqz(inst),
        (0b111, 0b01) => c_bnez(inst),

        (0b000, 0b10) => c_slli(inst),
        (0b010, 0b10) => c_lwsp(inst),
        (0b011, 0b10) => c_ldsp(inst),
        (0b100, 0b10) => c_misc_alu(inst),
        (0b110, 0b10) => c_swsp(inst),
        (0b111, 0b10) => c_sdsp(inst),
        _ => None,
    }
}

fn reg_prime(val: u16) -> u32 {
    8 + (val as u32 & 0x7)
}

fn sign_extend(val: u32, bits: u32) -> i32 {
    let shift = 32 - bits;
    ((val << shift) as i32) >> shift
}

fn encode_i(op: u32, rd: u32, rs1: u32, funct3: u32, imm: i32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | op
}

fn encode_u(op: u32, rd: u32, imm: i32) -> u32 {
    (imm as u32 & 0xFFFFF000) | (rd << 7) | op
}

fn encode_r(op: u32, rd: u32, rs1: u32, rs2: u32, funct3: u32, funct7: u32) -> u32 {
    (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | op
}

fn encode_s(op: u32, rs1: u32, rs2: u32, funct3: u32, imm: i32) -> u32 {
    let imm_u = imm as u32;
    let imm_11_5 = (imm_u >> 5) & 0x7F;
    let imm_4_0 = imm_u & 0x1F;
    (imm_11_5 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (imm_4_0 << 7) | op
}

fn encode_b(op: u32, rs1: u32, rs2: u32, funct3: u32, imm: i32) -> u32 {
    let imm_u = imm as u32;
    let imm_12 = (imm_u >> 12) & 1;
    let imm_10_5 = (imm_u >> 5) & 0x3F;
    let imm_4_1 = (imm_u >> 1) & 0xF;
    let imm_11 = (imm_u >> 11) & 1;
    (imm_12 << 31) | (imm_10_5 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) |
        (imm_4_1 << 8) | (imm_11 << 7) | op
}

fn encode_j(op: u32, rd: u32, imm: i32) -> u32 {
    let imm_u = imm as u32;
    let imm_20 = (imm_u >> 20) & 1;
    let imm_10_1 = (imm_u >> 1) & 0x3FF;
    let imm_11 = (imm_u >> 11) & 1;
    let imm_19_12 = (imm_u >> 12) & 0xFF;
    (imm_20 << 31) | (imm_19_12 << 12) | (imm_11 << 20) | (imm_10_1 << 21) | (rd << 7) | op
}

fn c_addi4spn(inst: u16) -> Option<u32> {
    let rd = reg_prime((inst >> 2) & 0x7);
    let imm = ((inst as u32 >> 12) & 1) << 5
        | ((inst as u32 >> 11) & 1) << 4
        | ((inst as u32 >> 7) & 0xF) << 6
        | ((inst as u32 >> 6) & 1) << 2
        | ((inst as u32 >> 5) & 1) << 3;
    if imm == 0 {
        return None;
    }
    Some(encode_i(OP_OP_IMM, rd, 2, FUNCT3_ADD_SUB, imm as i32))
}

fn c_lw(inst: u16) -> Option<u32> {
    let rd = reg_prime((inst >> 2) & 0x7);
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let imm = ((inst as u32 >> 10) & 0x7) << 3
        | ((inst as u32 >> 6) & 1) << 2
        | ((inst as u32 >> 5) & 1) << 6;
    Some(encode_i(OP_LOAD, rd, rs1, FUNCT3_LW, imm as i32))
}

fn c_ld(inst: u16) -> Option<u32> {
    let rd = reg_prime((inst >> 2) & 0x7);
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let imm = ((inst as u32 >> 10) & 0x7) << 3
        | ((inst as u32 >> 5) & 0x3) << 6;
    Some(encode_i(OP_LOAD, rd, rs1, FUNCT3_LD, imm as i32))
}

fn c_sw(inst: u16) -> Option<u32> {
    let rs2 = reg_prime((inst >> 2) & 0x7);
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let imm = ((inst as u32 >> 10) & 0x7) << 3
        | ((inst as u32 >> 6) & 1) << 2
        | ((inst as u32 >> 5) & 1) << 6;
    Some(encode_s(OP_STORE, rs1, rs2, FUNCT3_LW, imm as i32))
}

fn c_sd(inst: u16) -> Option<u32> {
    let rs2 = reg_prime((inst >> 2) & 0x7);
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let imm = ((inst as u32 >> 10) & 0x7) << 3
        | ((inst as u32 >> 5) & 0x3) << 6;
    Some(encode_s(OP_STORE, rs1, rs2, FUNCT3_LD, imm as i32))
}

fn c_addi(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    let imm = sign_extend(((inst as u32 >> 2) & 0x1F) | ((inst as u32 >> 12) & 1) << 5, 6);
    if rd == 0 && imm == 0 {
        return Some(encode_i(OP_OP_IMM, 0, 0, FUNCT3_ADD_SUB, 0));
    }
    Some(encode_i(OP_OP_IMM, rd, rd, FUNCT3_ADD_SUB, imm))
}

fn c_addiw(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    let imm = sign_extend(((inst as u32 >> 2) & 0x1F) | ((inst as u32 >> 12) & 1) << 5, 6);
    Some(encode_i(OP_OP_IMM_32, rd, rd, FUNCT3_ADD_SUB, imm))
}

fn c_li(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    let imm = sign_extend(((inst as u32 >> 2) & 0x1F) | ((inst as u32 >> 12) & 1) << 5, 6);
    Some(encode_i(OP_OP_IMM, rd, 0, FUNCT3_ADD_SUB, imm))
}

fn c_addi16sp_lui(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    if rd == 2 {
        let imm = ((inst as u32 >> 12) & 1) << 9
            | ((inst as u32 >> 3) & 0x3) << 7
            | ((inst as u32 >> 5) & 1) << 6
            | ((inst as u32 >> 2) & 1) << 5
            | ((inst as u32 >> 6) & 1) << 4;
        let imm = sign_extend(imm, 10);
        Some(encode_i(OP_OP_IMM, 2, 2, FUNCT3_ADD_SUB, imm))
    } else {
        let imm = sign_extend(((inst as u32 >> 12) & 1) << 5 | ((inst as u32 >> 2) & 0x1F), 6);
        if imm == 0 {
            return None;
        }
        Some(encode_u(OP_LUI, rd, imm << 12))
    }
}

fn c_alu_imm(inst: u16) -> Option<u32> {
    let subop = (inst >> 10) & 0x3;
    let rs1 = reg_prime((inst >> 7) & 0x7);

    match subop {
        0b00 => {
            let shamt = ((inst as u32 >> 2) & 0x1F) | (((inst as u32 >> 12) & 1) << 5);
            Some(encode_i(OP_OP_IMM, rs1, rs1, FUNCT3_SRL_SRA, shamt as i32))
        }
        0b01 => {
            let shamt = ((inst as u32 >> 2) & 0x1F) | (((inst as u32 >> 12) & 1) << 5);
            Some(encode_i(OP_OP_IMM, rs1, rs1, FUNCT3_SRL_SRA, (0b010000 << 6) as i32 | shamt as i32))
        }
        0b10 => {
            let imm = sign_extend(((inst as u32 >> 2) & 0x1F) | ((inst as u32 >> 12) & 1) << 5, 6);
            Some(encode_i(OP_OP_IMM, rs1, rs1, FUNCT3_AND, imm))
        }
        0b11 => c_alu_reg(inst),
        _ => None,
    }
}

fn c_alu_reg(inst: u16) -> Option<u32> {
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let rs2 = reg_prime((inst >> 2) & 0x7);
    let funct2 = (inst >> 5) & 0x3;
    let is_w = ((inst >> 12) & 1) != 0;

    if !is_w {
        let (funct3, funct7) = match funct2 {
            0b00 => (FUNCT3_ADD_SUB, 0b0100000),
            0b01 => (FUNCT3_XOR, 0b0000000),
            0b10 => (FUNCT3_OR, 0b0000000),
            0b11 => (FUNCT3_AND, 0b0000000),
            _ => return None,
        };
        Some(encode_r(OP_OP, rs1, rs1, rs2, funct3, funct7))
    } else {
        let (funct3, funct7) = match funct2 {
            0b00 => (FUNCT3_ADD_SUB, 0b0100000),
            0b01 => (FUNCT3_ADD_SUB, 0b0000000),
            _ => return None,
        };
        Some(encode_r(OP_OP_32, rs1, rs1, rs2, funct3, funct7))
    }
}

fn c_j(inst: u16) -> Option<u32> {
    let imm = decode_cj_imm(inst);
    Some(encode_j(OP_JAL, 0, imm))
}

fn c_beqz(inst: u16) -> Option<u32> {
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let imm = decode_cb_imm(inst);
    Some(encode_b(OP_BRANCH, rs1, 0, FUNCT3_BEQ, imm))
}

fn c_bnez(inst: u16) -> Option<u32> {
    let rs1 = reg_prime((inst >> 7) & 0x7);
    let imm = decode_cb_imm(inst);
    Some(encode_b(OP_BRANCH, rs1, 0, FUNCT3_BNE, imm))
}

fn c_slli(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    let shamt = ((inst as u32 >> 2) & 0x1F) | (((inst as u32 >> 12) & 1) << 5);
    Some(encode_i(OP_OP_IMM, rd, rd, FUNCT3_SLL, shamt as i32))
}

fn c_lwsp(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    if rd == 0 {
        return None;
    }
    let imm = ((inst as u32 >> 12) & 1) << 5
        | ((inst as u32 >> 4) & 0x7) << 2
        | ((inst as u32 >> 2) & 0x3) << 6;
    Some(encode_i(OP_LOAD, rd, 2, FUNCT3_LW, imm as i32))
}

fn c_ldsp(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    if rd == 0 {
        return None;
    }
    let imm = ((inst as u32 >> 12) & 1) << 5
        | ((inst as u32 >> 5) & 0x3) << 3
        | ((inst as u32 >> 2) & 0x7) << 6;
    Some(encode_i(OP_LOAD, rd, 2, FUNCT3_LD, imm as i32))
}

fn c_misc_alu(inst: u16) -> Option<u32> {
    let rd = ((inst >> 7) & 0x1F) as u32;
    let rs2 = ((inst >> 2) & 0x1F) as u32;
    let bit12 = (inst >> 12) & 1;

    if bit12 == 0 {
        if rs2 == 0 {
            if rd == 0 {
                None
            } else {
                Some(encode_i(OP_JALR, 0, rd, FUNCT3_ADD_SUB, 0))
            }
        } else {
            Some(encode_r(OP_OP, rd, 0, rs2, FUNCT3_ADD_SUB, 0))
        }
    } else {
        if rs2 == 0 {
            if rd == 0 {
                Some(0x0010_0073) // EBREAK
            } else {
                Some(encode_i(OP_JALR, 1, rd, FUNCT3_ADD_SUB, 0))
            }
        } else {
            Some(encode_r(OP_OP, rd, rd, rs2, FUNCT3_ADD_SUB, 0))
        }
    }
}

fn c_swsp(inst: u16) -> Option<u32> {
    let rs2 = ((inst >> 2) & 0x1F) as u32;
    let imm = ((inst as u32 >> 9) & 0xF) << 2
        | ((inst as u32 >> 7) & 0x3) << 6;
    Some(encode_s(OP_STORE, 2, rs2, FUNCT3_LW, imm as i32))
}

fn c_sdsp(inst: u16) -> Option<u32> {
    let rs2 = ((inst >> 2) & 0x1F) as u32;
    let imm = ((inst as u32 >> 10) & 0x7) << 3
        | ((inst as u32 >> 7) & 0x7) << 6;
    Some(encode_s(OP_STORE, 2, rs2, FUNCT3_LD, imm as i32))
}

fn decode_cj_imm(inst: u16) -> i32 {
    let imm = ((inst as u32 >> 12) & 1) << 11
        | ((inst as u32 >> 8) & 0x1) << 10
        | ((inst as u32 >> 9) & 0x3) << 8
        | ((inst as u32 >> 6) & 0x1) << 7
        | ((inst as u32 >> 7) & 0x1) << 6
        | ((inst as u32 >> 2) & 0x1) << 5
        | ((inst as u32 >> 11) & 0x1) << 4
        | ((inst as u32 >> 3) & 0x7) << 1;
    sign_extend(imm, 12)
}

fn decode_cb_imm(inst: u16) -> i32 {
    let imm = ((inst as u32 >> 12) & 1) << 8
        | ((inst as u32 >> 5) & 0x3) << 6
        | ((inst as u32 >> 2) & 0x1) << 5
        | ((inst as u32 >> 10) & 0x3) << 3
        | ((inst as u32 >> 3) & 0x3) << 1;
    sign_extend(imm, 9)
}
