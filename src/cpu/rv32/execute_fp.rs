//! Floating-point instruction execution (F and D extensions)
//!
//! Implements RV32F and RV32D instruction semantics

use super::Cpu;
use super::decode::*;
use super::csr::*;
use super::mmu::AccessType;
use crate::cpu::PrivilegeLevel;
use crate::cpu::trap::Trap;
use crate::cpu::fpu;
use crate::memory::Bus;

impl Cpu {
    /// Execute floating-point load instructions (FLW, FLD)
    pub fn execute_load_fp(&mut self, inst: u32, d: &DecodedInst, bus: &mut impl Bus) -> Result<(), Trap> {
        // Check if FP is enabled (FS != 0 in mstatus)
        if (self.csr.mstatus & MSTATUS_FS) == 0 {
            return Err(Trap::IllegalInstruction(inst));
        }
        
        let imm = DecodedInst::imm_i(inst) as u32;
        let vaddr = self.read_reg(d.rs1).wrapping_add(imm);
        let satp = self.csr.satp;
        let mstatus = self.csr.mstatus;
        let mut priv_level = self.priv_level;
        
        // Handle MPRV
        if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
            let mpp = (mstatus >> 11) & 3;
            priv_level = PrivilegeLevel::from(mpp as u8);
        }
        
        let paddr = match self.mmu.translate(vaddr, AccessType::Load, priv_level, bus, satp, mstatus) {
            Ok(pa) => pa,
            Err(cause) => {
                return Err(Trap::from_cause(cause, vaddr));
            }
        };
        
        match d.funct3 {
            FUNCT3_FLW => {
                // FLW - Load single-precision float
                if vaddr & 3 != 0 {
                    return Err(Trap::LoadAddressMisaligned(vaddr));
                }
                let value = bus.read32(paddr);
                self.fpu.write_f32(d.rd, value);
            }
            FUNCT3_FLD => {
                // FLD - Load double-precision float
                if vaddr & 7 != 0 {
                    return Err(Trap::LoadAddressMisaligned(vaddr));
                }
                let lo = bus.read32(paddr);
                let hi = bus.read32(paddr.wrapping_add(4));
                let value = (hi as u64) << 32 | lo as u64;
                self.fpu.write_f64(d.rd, value);
            }
            _ => return Err(Trap::IllegalInstruction(inst)),
        }
        
        // Mark FP state as dirty
        self.csr.mstatus |= MSTATUS_FS;
        self.pc = self.pc.wrapping_add(4);
        Ok(())
    }
    
    /// Execute floating-point store instructions (FSW, FSD)
    pub fn execute_store_fp(&mut self, inst: u32, d: &DecodedInst, bus: &mut impl Bus) -> Result<(), Trap> {
        // Check if FP is enabled
        if (self.csr.mstatus & MSTATUS_FS) == 0 {
            return Err(Trap::IllegalInstruction(inst));
        }
        
        let imm = DecodedInst::imm_s(inst) as u32;
        let vaddr = self.read_reg(d.rs1).wrapping_add(imm);
        let satp = self.csr.satp;
        let mstatus = self.csr.mstatus;
        let mut priv_level = self.priv_level;
        
        // Handle MPRV
        if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
            let mpp = (mstatus >> 11) & 3;
            priv_level = PrivilegeLevel::from(mpp as u8);
        }
        
        let paddr = match self.mmu.translate(vaddr, AccessType::Store, priv_level, bus, satp, mstatus) {
            Ok(pa) => pa,
            Err(cause) => {
                return Err(Trap::from_cause(cause, vaddr));
            }
        };
        
        match d.funct3 {
            FUNCT3_FLW => {
                // FSW - Store single-precision float
                if vaddr & 3 != 0 {
                    return Err(Trap::StoreAddressMisaligned(vaddr));
                }
                let value = self.fpu.read_f32(d.rs2);
                bus.write32(paddr, value);
            }
            FUNCT3_FLD => {
                // FSD - Store double-precision float
                if vaddr & 7 != 0 {
                    return Err(Trap::StoreAddressMisaligned(vaddr));
                }
                let value = self.fpu.read_f64(d.rs2);
                bus.write32(paddr, value as u32);
                bus.write32(paddr.wrapping_add(4), (value >> 32) as u32);
            }
            _ => return Err(Trap::IllegalInstruction(inst)),
        }
        
        self.pc = self.pc.wrapping_add(4);
        Ok(())
    }
    
    /// Execute fused multiply-add instructions (FMADD, FMSUB, FNMSUB, FNMADD)
    pub fn execute_fma(&mut self, inst: u32, d: &DecodedInst, opcode: u32) -> Result<(), Trap> {
        // Check if FP is enabled
        if (self.csr.mstatus & MSTATUS_FS) == 0 {
            return Err(Trap::IllegalInstruction(inst));
        }
        
        let rm = self.fpu.effective_rm(d.funct3);
        let fmt = (inst >> 25) & 0b11;
        
        match fmt {
            FMT_S => {
                // Single-precision
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let rs3 = self.fpu.read_f32(d.rs3);
                
                let (result, flags) = match opcode {
                    OP_MADD => {
                        // FMADD.S: rd = rs1 * rs2 + rs3
                        fpu::f32_fmadd(rs1, rs2, rs3, rm)
                    }
                    OP_MSUB => {
                        // FMSUB.S: rd = rs1 * rs2 - rs3
                        let neg_rs3 = rs3 ^ 0x8000_0000;
                        fpu::f32_fmadd(rs1, rs2, neg_rs3, rm)
                    }
                    OP_NMSUB => {
                        // FNMSUB.S: rd = -(rs1 * rs2) + rs3 = -rs1 * rs2 + rs3
                        let neg_rs1 = rs1 ^ 0x8000_0000;
                        fpu::f32_fmadd(neg_rs1, rs2, rs3, rm)
                    }
                    OP_NMADD => {
                        // FNMADD.S: rd = -(rs1 * rs2) - rs3 = -rs1 * rs2 - rs3
                        let neg_rs1 = rs1 ^ 0x8000_0000;
                        let neg_rs3 = rs3 ^ 0x8000_0000;
                        fpu::f32_fmadd(neg_rs1, rs2, neg_rs3, rm)
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FMT_D => {
                // Double-precision
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let rs3 = self.fpu.read_f64(d.rs3);
                
                let (result, flags) = match opcode {
                    OP_MADD => fpu::f64_fmadd(rs1, rs2, rs3, rm),
                    OP_MSUB => {
                        let neg_rs3 = rs3 ^ 0x8000_0000_0000_0000;
                        fpu::f64_fmadd(rs1, rs2, neg_rs3, rm)
                    }
                    OP_NMSUB => {
                        let neg_rs1 = rs1 ^ 0x8000_0000_0000_0000;
                        fpu::f64_fmadd(neg_rs1, rs2, rs3, rm)
                    }
                    OP_NMADD => {
                        let neg_rs1 = rs1 ^ 0x8000_0000_0000_0000;
                        let neg_rs3 = rs3 ^ 0x8000_0000_0000_0000;
                        fpu::f64_fmadd(neg_rs1, rs2, neg_rs3, rm)
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            _ => return Err(Trap::IllegalInstruction(inst)),
        }
        
        self.csr.mstatus |= MSTATUS_FS;
        self.pc = self.pc.wrapping_add(4);
        Ok(())
    }
    
    /// Execute floating-point computational instructions (OP-FP opcode)
    pub fn execute_op_fp(&mut self, inst: u32, d: &DecodedInst) -> Result<(), Trap> {
        // Check if FP is enabled
        if (self.csr.mstatus & MSTATUS_FS) == 0 {
            return Err(Trap::IllegalInstruction(inst));
        }
        
        let rm = self.fpu.effective_rm(d.funct3);
        
        match d.funct7 {
            // ============ Single-precision operations ============
            FUNCT7_FADD_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let (result, flags) = fpu::f32_add(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FSUB_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let (result, flags) = fpu::f32_sub(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FMUL_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let (result, flags) = fpu::f32_mul(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FDIV_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let (result, flags) = fpu::f32_div(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FSQRT_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let (result, flags) = fpu::f32_sqrt(rs1, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FSGNJ_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let result = match d.funct3 {
                    FUNCT3_FSGNJ => fpu::f32_sgnj(rs1, rs2),
                    FUNCT3_FSGNJN => fpu::f32_sgnjn(rs1, rs2),
                    FUNCT3_FSGNJX => fpu::f32_sgnjx(rs1, rs2),
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FMIN_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let (result, flags) = match d.funct3 {
                    FUNCT3_FMIN => fpu::f32_min(rs1, rs2),
                    FUNCT3_FMAX => fpu::f32_max(rs1, rs2),
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FCVT_W_S => {
                // FCVT.W.S / FCVT.WU.S: float to int
                let rs1 = self.fpu.read_f32(d.rs1);
                let (result, flags) = if d.rs2 == 0 {
                    // FCVT.W.S (signed)
                    let (v, f) = fpu::f32_to_i32(rs1, rm);
                    (v as u32, f)
                } else if d.rs2 == 1 {
                    // FCVT.WU.S (unsigned)
                    fpu::f32_to_u32(rs1, rm)
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                };
                self.fpu.fflags.merge(flags);
                self.write_reg(d.rd, result);
            }
            FUNCT7_FMV_X_W => {
                if d.funct3 == 0 && d.rs2 == 0 {
                    // FMV.X.W: move bits from f-reg to x-reg
                    let value = self.fpu.read_f32(d.rs1);
                    self.write_reg(d.rd, value);
                } else if d.funct3 == 1 && d.rs2 == 0 {
                    // FCLASS.S: classify float
                    let rs1 = self.fpu.read_f32(d.rs1);
                    let result = fpu::f32_classify(rs1);
                    self.write_reg(d.rd, result);
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                }
            }
            FUNCT7_FCMP_S => {
                let rs1 = self.fpu.read_f32(d.rs1);
                let rs2 = self.fpu.read_f32(d.rs2);
                let (result, flags) = match d.funct3 {
                    FUNCT3_FEQ => {
                        let (v, f) = fpu::f32_eq(rs1, rs2);
                        (v as u32, f)
                    }
                    FUNCT3_FLT => {
                        let (v, f) = fpu::f32_lt(rs1, rs2);
                        (v as u32, f)
                    }
                    FUNCT3_FLE => {
                        let (v, f) = fpu::f32_le(rs1, rs2);
                        (v as u32, f)
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                self.fpu.fflags.merge(flags);
                self.write_reg(d.rd, result);
            }
            FUNCT7_FCVT_S_W => {
                // FCVT.S.W / FCVT.S.WU: int to float
                let rs1 = self.read_reg(d.rs1);
                let (result, flags) = if d.rs2 == 0 {
                    // FCVT.S.W (signed)
                    fpu::i32_to_f32(rs1 as i32, rm)
                } else if d.rs2 == 1 {
                    // FCVT.S.WU (unsigned)
                    fpu::u32_to_f32(rs1, rm)
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                };
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FMV_W_X => {
                if d.funct3 == 0 && d.rs2 == 0 {
                    // FMV.W.X: move bits from x-reg to f-reg
                    let value = self.read_reg(d.rs1);
                    self.fpu.write_f32(d.rd, value);
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                }
            }
            
            // ============ Double-precision operations ============
            FUNCT7_FADD_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let (result, flags) = fpu::f64_add(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FSUB_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let (result, flags) = fpu::f64_sub(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FMUL_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let (result, flags) = fpu::f64_mul(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FDIV_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let (result, flags) = fpu::f64_div(rs1, rs2, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FSQRT_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let (result, flags) = fpu::f64_sqrt(rs1, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FSGNJ_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let result = match d.funct3 {
                    FUNCT3_FSGNJ => fpu::f64_sgnj(rs1, rs2),
                    FUNCT3_FSGNJN => fpu::f64_sgnjn(rs1, rs2),
                    FUNCT3_FSGNJX => fpu::f64_sgnjx(rs1, rs2),
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FMIN_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let (result, flags) = match d.funct3 {
                    FUNCT3_FMIN => fpu::f64_min(rs1, rs2),
                    FUNCT3_FMAX => fpu::f64_max(rs1, rs2),
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FCVT_S_D => {
                // FCVT.S.D: double to single
                let rs1 = self.fpu.read_f64(d.rs1);
                let (result, flags) = fpu::f64_to_f32(rs1, rm);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f32(d.rd, result);
            }
            FUNCT7_FCVT_D_S => {
                // FCVT.D.S: single to double
                let rs1 = self.fpu.read_f32(d.rs1);
                let (result, flags) = fpu::f32_to_f64(rs1);
                self.fpu.fflags.merge(flags);
                self.fpu.write_f64(d.rd, result);
            }
            FUNCT7_FCVT_W_D => {
                // FCVT.W.D / FCVT.WU.D: double to int
                let rs1 = self.fpu.read_f64(d.rs1);
                let (result, flags) = if d.rs2 == 0 {
                    let (v, f) = fpu::f64_to_i32(rs1, rm);
                    (v as u32, f)
                } else if d.rs2 == 1 {
                    fpu::f64_to_u32(rs1, rm)
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                };
                self.fpu.fflags.merge(flags);
                self.write_reg(d.rd, result);
            }
            FUNCT7_FCMP_D => {
                let rs1 = self.fpu.read_f64(d.rs1);
                let rs2 = self.fpu.read_f64(d.rs2);
                let (result, flags) = match d.funct3 {
                    FUNCT3_FEQ => {
                        let (v, f) = fpu::f64_eq(rs1, rs2);
                        (v as u32, f)
                    }
                    FUNCT3_FLT => {
                        let (v, f) = fpu::f64_lt(rs1, rs2);
                        (v as u32, f)
                    }
                    FUNCT3_FLE => {
                        let (v, f) = fpu::f64_le(rs1, rs2);
                        (v as u32, f)
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                self.fpu.fflags.merge(flags);
                self.write_reg(d.rd, result);
            }
            FUNCT7_FCLASS_D => {
                if d.funct3 == 1 && d.rs2 == 0 {
                    // FCLASS.D
                    let rs1 = self.fpu.read_f64(d.rs1);
                    let result = fpu::f64_classify(rs1);
                    self.write_reg(d.rd, result);
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                }
            }
            FUNCT7_FCVT_D_W => {
                // FCVT.D.W / FCVT.D.WU: int to double
                let rs1 = self.read_reg(d.rs1);
                let result = if d.rs2 == 0 {
                    // FCVT.D.W (signed) - always exact
                    fpu::i32_to_f64(rs1 as i32)
                } else if d.rs2 == 1 {
                    // FCVT.D.WU (unsigned) - always exact
                    fpu::u32_to_f64(rs1)
                } else {
                    return Err(Trap::IllegalInstruction(inst));
                };
                self.fpu.write_f64(d.rd, result);
            }
            
            _ => return Err(Trap::IllegalInstruction(inst)),
        }
        
        self.csr.mstatus |= MSTATUS_FS;
        self.pc = self.pc.wrapping_add(4);
        Ok(())
    }
}
