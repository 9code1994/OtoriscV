//! Instruction execution (RV64)

use super::Cpu64;
use super::decode::*;
use super::csr::*;
use super::mmu::AccessType;
use super::trap::{self, Trap64};
use crate::cpu::PrivilegeLevel;
use crate::memory::Bus;

impl Cpu64 {
    pub fn execute(&mut self, inst: u32, bus: &mut impl Bus) -> Result<(), Trap64> {
        let d = DecodedInst::decode(inst);

        match d.opcode {
            OP_LUI => {
                let imm = DecodedInst::imm_u(inst) as i32 as i64 as u64;
                self.write_reg(d.rd, imm);
                self.pc = self.pc.wrapping_add(4);
            }
            OP_AUIPC => {
                let imm = DecodedInst::imm_u(inst) as i32 as i64 as u64;
                self.write_reg(d.rd, self.pc.wrapping_add(imm));
                self.pc = self.pc.wrapping_add(4);
            }
            OP_JAL => {
                let imm = DecodedInst::imm_j(inst) as i64 as u64;
                self.write_reg(d.rd, self.pc.wrapping_add(4));
                self.pc = self.pc.wrapping_add(imm);
            }
            OP_JALR => {
                let imm = DecodedInst::imm_i(inst) as i64 as u64;
                let target = (self.read_reg(d.rs1).wrapping_add(imm)) & !1;
                self.write_reg(d.rd, self.pc.wrapping_add(4));
                self.pc = target;
            }
            OP_BRANCH => {
                let rs1 = self.read_reg(d.rs1);
                let rs2 = self.read_reg(d.rs2);
                let imm = DecodedInst::imm_b(inst) as i64 as u64;

                let taken = match d.funct3 {
                    FUNCT3_BEQ => rs1 == rs2,
                    FUNCT3_BNE => rs1 != rs2,
                    FUNCT3_BLT => (rs1 as i64) < (rs2 as i64),
                    FUNCT3_BGE => (rs1 as i64) >= (rs2 as i64),
                    FUNCT3_BLTU => rs1 < rs2,
                    FUNCT3_BGEU => rs1 >= rs2,
                    _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                };

                if taken {
                    self.pc = self.pc.wrapping_add(imm);
                } else {
                    self.pc = self.pc.wrapping_add(4);
                }
            }
            OP_LOAD => {
                let imm = DecodedInst::imm_i(inst) as i64 as u64;
                let vaddr = self.read_reg(d.rs1).wrapping_add(imm);
                let satp = self.csr.satp;
                let mstatus = self.csr.mstatus;
                let mut priv_level = self.priv_level;

                if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
                    let mpp = (mstatus >> 11) & 3;
                    priv_level = PrivilegeLevel::from(mpp as u8);
                }

                let paddr = match self.mmu.translate(vaddr, AccessType::Load, priv_level, bus, satp, mstatus) {
                    Ok(pa) => pa,
                    Err(cause) => return Err(Trap64::from_cause(cause, vaddr)),
                };
                let addr = Self::map_paddr(paddr)?;

                let value = match d.funct3 {
                    FUNCT3_LB => bus.read8(addr) as i8 as i64 as u64,
                    FUNCT3_LH => {
                        if vaddr & 1 != 0 {
                            let b0 = bus.read8(addr) as u16;
                            let b1 = bus.read8(addr.wrapping_add(1)) as u16;
                            let v = (b1 << 8) | b0;
                            (v as i16) as i64 as u64
                        } else {
                            (bus.read16(addr) as i16) as i64 as u64
                        }
                    }
                    FUNCT3_LW => {
                        if vaddr & 3 != 0 {
                            let b0 = bus.read8(addr) as u32;
                            let b1 = bus.read8(addr.wrapping_add(1)) as u32;
                            let b2 = bus.read8(addr.wrapping_add(2)) as u32;
                            let b3 = bus.read8(addr.wrapping_add(3)) as u32;
                            let v = (b3 << 24) | (b2 << 16) | (b1 << 8) | b0;
                            (v as i32) as i64 as u64
                        } else {
                            (bus.read32(addr) as i32) as i64 as u64
                        }
                    }
                    FUNCT3_LBU => bus.read8(addr) as u64,
                    FUNCT3_LHU => {
                        if vaddr & 1 != 0 {
                            let b0 = bus.read8(addr) as u16;
                            let b1 = bus.read8(addr.wrapping_add(1)) as u16;
                            ((b1 << 8) | b0) as u64
                        } else {
                            bus.read16(addr) as u64
                        }
                    }
                    FUNCT3_LWU => {
                        if vaddr & 3 != 0 {
                            let b0 = bus.read8(addr) as u32;
                            let b1 = bus.read8(addr.wrapping_add(1)) as u32;
                            let b2 = bus.read8(addr.wrapping_add(2)) as u32;
                            let b3 = bus.read8(addr.wrapping_add(3)) as u32;
                            let v = (b3 << 24) | (b2 << 16) | (b1 << 8) | b0;
                            v as u64
                        } else {
                            bus.read32(addr) as u64
                        }
                    }
                    FUNCT3_LD => {
                        if vaddr & 7 != 0 {
                            let mut v = 0u64;
                            for i in 0..8 {
                                v |= (bus.read8(addr.wrapping_add(i)) as u64) << (i * 8);
                            }
                            v
                        } else {
                            bus.read64(addr)
                        }
                    }
                    _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                };

                self.write_reg(d.rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            OP_STORE => {
                let imm = DecodedInst::imm_s(inst) as i64 as u64;
                let vaddr = self.read_reg(d.rs1).wrapping_add(imm);
                let value = self.read_reg(d.rs2);
                let satp = self.csr.satp;
                let mstatus = self.csr.mstatus;
                let mut priv_level = self.priv_level;

                if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
                    let mpp = (mstatus >> 11) & 3;
                    priv_level = PrivilegeLevel::from(mpp as u8);
                }

                let paddr = match self.mmu.translate(vaddr, AccessType::Store, priv_level, bus, satp, mstatus) {
                    Ok(pa) => pa,
                    Err(cause) => return Err(Trap64::from_cause(cause, vaddr)),
                };
                let addr = Self::map_paddr(paddr)?;

                match d.funct3 {
                    0b000 => {
                        bus.write8(addr, value as u8);
                        self.last_write_addr = paddr;
                        self.last_write_val = value & 0xFF;
                    }
                    0b001 => {
                        if vaddr & 1 != 0 {
                            bus.write8(addr, value as u8);
                            bus.write8(addr.wrapping_add(1), (value >> 8) as u8);
                        } else {
                            bus.write16(addr, value as u16);
                        }
                        self.last_write_addr = paddr;
                        self.last_write_val = value & 0xFFFF;
                    }
                    0b010 => {
                        if vaddr & 3 != 0 {
                            bus.write8(addr, value as u8);
                            bus.write8(addr.wrapping_add(1), (value >> 8) as u8);
                            bus.write8(addr.wrapping_add(2), (value >> 16) as u8);
                            bus.write8(addr.wrapping_add(3), (value >> 24) as u8);
                        } else {
                            bus.write32(addr, value as u32);
                        }
                        self.last_write_addr = paddr;
                        self.last_write_val = value & 0xFFFF_FFFF;
                    }
                    0b011 => {
                        if vaddr & 7 != 0 {
                            for i in 0..8 {
                                bus.write8(addr.wrapping_add(i), (value >> (i * 8)) as u8);
                            }
                        } else {
                            bus.write64(addr, value);
                        }
                        self.last_write_addr = paddr;
                        self.last_write_val = value;
                    }
                    _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                }

                self.pc = self.pc.wrapping_add(4);
            }
            OP_OP_IMM => {
                let rs1 = self.read_reg(d.rs1);
                let imm = DecodedInst::imm_i(inst) as i64 as u64;
                let shamt = (imm & 0x3F) as u32;
                let funct6 = (inst >> 26) & 0x3F;

                let result = match d.funct3 {
                    FUNCT3_ADD_SUB => rs1.wrapping_add(imm),
                    FUNCT3_SLT => if (rs1 as i64) < (imm as i64) { 1 } else { 0 },
                    FUNCT3_SLTU => if rs1 < imm { 1 } else { 0 },
                    FUNCT3_XOR => rs1 ^ imm,
                    FUNCT3_OR => rs1 | imm,
                    FUNCT3_AND => rs1 & imm,
                    FUNCT3_SLL => rs1 << shamt,
                    FUNCT3_SRL_SRA => {
                        if funct6 == 0b010000 {
                            ((rs1 as i64) >> shamt) as u64
                        } else {
                            rs1 >> shamt
                        }
                    }
                    _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                };

                self.write_reg(d.rd, result);
                self.pc = self.pc.wrapping_add(4);
            }
            OP_OP => {
                let rs1 = self.read_reg(d.rs1);
                let rs2 = self.read_reg(d.rs2);

                let result = if d.funct7 == 0b0000001 {
                    self.execute_m_extension(d.funct3, rs1, rs2)?
                } else {
                    match (d.funct3, d.funct7) {
                        (FUNCT3_ADD_SUB, 0b0000000) => rs1.wrapping_add(rs2),
                        (FUNCT3_ADD_SUB, 0b0100000) => rs1.wrapping_sub(rs2),
                        (FUNCT3_SLL, 0b0000000) => rs1 << (rs2 & 0x3F),
                        (FUNCT3_SLT, 0b0000000) => if (rs1 as i64) < (rs2 as i64) { 1 } else { 0 },
                        (FUNCT3_SLTU, 0b0000000) => if rs1 < rs2 { 1 } else { 0 },
                        (FUNCT3_XOR, 0b0000000) => rs1 ^ rs2,
                        (FUNCT3_SRL_SRA, 0b0000000) => rs1 >> (rs2 & 0x3F),
                        (FUNCT3_SRL_SRA, 0b0100000) => ((rs1 as i64) >> (rs2 & 0x3F)) as u64,
                        (FUNCT3_OR, 0b0000000) => rs1 | rs2,
                        (FUNCT3_AND, 0b0000000) => rs1 & rs2,
                        _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                    }
                };

                self.write_reg(d.rd, result);
                self.pc = self.pc.wrapping_add(4);
            }
            OP_OP_IMM_32 => {
                let rs1 = self.read_reg(d.rs1) as u32;
                let imm = DecodedInst::imm_i(inst) as u32;
                let shamt = (imm & 0x1F) as u32;
                let funct7 = (inst >> 25) & 0x7F;

                let result = match d.funct3 {
                    FUNCT3_ADD_SUB => (rs1 as i32).wrapping_add(imm as i32) as u32,
                    FUNCT3_SLL => rs1 << shamt,
                    FUNCT3_SRL_SRA => {
                        if funct7 == 0b0100000 {
                            ((rs1 as i32) >> shamt) as u32
                        } else {
                            rs1 >> shamt
                        }
                    }
                    _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                };

                self.write_reg(d.rd, (result as i32 as i64) as u64);
                self.pc = self.pc.wrapping_add(4);
            }
            OP_OP_32 => {
                let rs1 = self.read_reg(d.rs1) as u32;
                let rs2 = self.read_reg(d.rs2) as u32;

                let result = if d.funct7 == 0b0000001 {
                    self.execute_m_extension_w(d.funct3, rs1, rs2)?
                } else {
                    match (d.funct3, d.funct7) {
                        (FUNCT3_ADD_SUB, 0b0000000) => rs1.wrapping_add(rs2),
                        (FUNCT3_ADD_SUB, 0b0100000) => rs1.wrapping_sub(rs2),
                        (FUNCT3_SLL, 0b0000000) => rs1 << (rs2 & 0x1F),
                        (FUNCT3_SRL_SRA, 0b0000000) => rs1 >> (rs2 & 0x1F),
                        (FUNCT3_SRL_SRA, 0b0100000) => ((rs1 as i32) >> (rs2 & 0x1F)) as u32,
                        _ => return Err(Trap64::IllegalInstruction(inst as u64)),
                    }
                };

                self.write_reg(d.rd, (result as i32 as i64) as u64);
                self.pc = self.pc.wrapping_add(4);
            }
            OP_AMO => {
                self.execute_amo(inst, &d, bus)?;
            }
            OP_MISC_MEM => {
                if d.funct3 == 0b001 {
                    self.mmu.invalidate();
                }
                self.pc = self.pc.wrapping_add(4);
            }
            OP_SYSTEM => {
                self.execute_system(inst, &d, bus)?;
            }
            OP_LOAD_FP => {
                self.execute_load_fp(inst, &d, bus)?;
            }
            OP_STORE_FP => {
                self.execute_store_fp(inst, &d, bus)?;
            }
            OP_OP_FP => {
                self.execute_op_fp(inst, &d)?;
            }
            OP_MADD | OP_MSUB | OP_NMSUB | OP_NMADD => {
                self.execute_fma(inst, &d, d.opcode)?;
            }
            _ => return Err(Trap64::IllegalInstruction(inst as u64)),
        }

        Ok(())
    }

    fn execute_system(&mut self, inst: u32, d: &DecodedInst, _bus: &mut impl Bus) -> Result<(), Trap64> {
        match d.funct3 {
            FUNCT3_PRIV => match inst {
                0x00000073 => {
                    let trap = match self.priv_level {
                        PrivilegeLevel::User => Trap64::EnvironmentCallFromU,
                        PrivilegeLevel::Supervisor => Trap64::EnvironmentCallFromS,
                        PrivilegeLevel::Machine => Trap64::EnvironmentCallFromM,
                    };
                    return Err(trap);
                }
                0x00100073 => return Err(Trap64::Breakpoint(self.pc)),
                0x10200073 => {
                    if self.priv_level < PrivilegeLevel::Supervisor {
                        return Err(Trap64::IllegalInstruction(inst as u64));
                    }
                    trap::sret(self);
                    return Ok(());
                }
                0x30200073 => {
                    if self.priv_level < PrivilegeLevel::Machine {
                        return Err(Trap64::IllegalInstruction(inst as u64));
                    }
                    trap::mret(self);
                    return Ok(());
                }
                0x10500073 => {
                    self.wfi = true;
                    self.pc = self.pc.wrapping_add(4);
                    return Ok(());
                }
                _ => {
                    if (inst >> 25) == 0b0001001 {
                        self.mmu.invalidate();
                        self.pc = self.pc.wrapping_add(4);
                        return Ok(());
                    }
                    return Err(Trap64::IllegalInstruction(inst as u64));
                }
            },
            FUNCT3_CSRRW | FUNCT3_CSRRS | FUNCT3_CSRRC |
            FUNCT3_CSRRWI | FUNCT3_CSRRSI | FUNCT3_CSRRCI => {
                let csr_addr = (inst >> 20) & 0xFFF;
                let is_imm = d.funct3 >= FUNCT3_CSRRWI;
                let rs1_val = if is_imm { d.rs1 as u64 } else { self.read_reg(d.rs1) };

                let old_val = match csr_addr {
                    CSR_FFLAGS => self.fpu.fflags.to_bits() as u64,
                    CSR_FRM => self.fpu.frm as u64,
                    CSR_FCSR => self.fpu.read_fcsr() as u64,
                    _ => self.csr.read(csr_addr, self.priv_level)
                        .ok_or(Trap64::IllegalInstruction(inst as u64))?,
                };

                let new_val = match d.funct3 & 0x3 {
                    0b01 => rs1_val,
                    0b10 => old_val | rs1_val,
                    0b11 => old_val & !rs1_val,
                    _ => old_val,
                };

                if d.funct3 & 0x3 == 0b01 || rs1_val != 0 {
                    match csr_addr {
                        CSR_FFLAGS => {
                            self.fpu.fflags = crate::cpu::fpu::FFlags::from_bits((new_val & 0x1F) as u32);
                            self.csr.mstatus |= MSTATUS_FS;
                        }
                        CSR_FRM => {
                            self.fpu.frm = crate::cpu::fpu::RoundingMode::from(new_val as u32);
                            self.csr.mstatus |= MSTATUS_FS;
                        }
                        CSR_FCSR => {
                            self.fpu.write_fcsr(new_val as u32);
                            self.csr.mstatus |= MSTATUS_FS;
                        }
                        _ => {
                            if csr_addr == CSR_SATP && new_val != old_val {
                                self.mmu.invalidate();
                            }
                            if !self.csr.write(csr_addr, new_val, self.priv_level) {
                                return Err(Trap64::IllegalInstruction(inst as u64));
                            }
                        }
                    }
                }

                self.write_reg(d.rd, old_val);
                self.pc = self.pc.wrapping_add(4);
            }
            _ => return Err(Trap64::IllegalInstruction(inst as u64)),
        }

        Ok(())
    }

    fn execute_m_extension(&self, funct3: u32, rs1: u64, rs2: u64) -> Result<u64, Trap64> {
        match funct3 {
            FUNCT3_MUL => Ok(rs1.wrapping_mul(rs2)),
            FUNCT3_MULH => {
                let res = (rs1 as i128) * (rs2 as i128);
                Ok((res >> 64) as u64)
            }
            FUNCT3_MULHSU => {
                let res = (rs1 as i128) * (rs2 as u128 as i128);
                Ok((res >> 64) as u64)
            }
            FUNCT3_MULHU => {
                let res = (rs1 as u128) * (rs2 as u128);
                Ok((res >> 64) as u64)
            }
            FUNCT3_DIV => {
                let a = rs1 as i64;
                let b = rs2 as i64;
                if b == 0 {
                    Ok(u64::MAX)
                } else if a == i64::MIN && b == -1 {
                    Ok(a as u64)
                } else {
                    Ok((a / b) as u64)
                }
            }
            FUNCT3_DIVU => {
                if rs2 == 0 { Ok(u64::MAX) } else { Ok(rs1 / rs2) }
            }
            FUNCT3_REM => {
                let a = rs1 as i64;
                let b = rs2 as i64;
                if b == 0 { Ok(rs1) }
                else if a == i64::MIN && b == -1 { Ok(0) }
                else { Ok((a % b) as u64) }
            }
            FUNCT3_REMU => {
                if rs2 == 0 { Ok(rs1) } else { Ok(rs1 % rs2) }
            }
            _ => Err(Trap64::IllegalInstruction(0)),
        }
    }

    fn execute_m_extension_w(&self, funct3: u32, rs1: u32, rs2: u32) -> Result<u32, Trap64> {
        match funct3 {
            FUNCT3_MUL => Ok(rs1.wrapping_mul(rs2)),
            FUNCT3_DIV => {
                let a = rs1 as i32;
                let b = rs2 as i32;
                if b == 0 { Ok(0xFFFF_FFFF) }
                else if a == i32::MIN && b == -1 { Ok(a as u32) }
                else { Ok((a / b) as u32) }
            }
            FUNCT3_DIVU => if rs2 == 0 { Ok(0xFFFF_FFFF) } else { Ok(rs1 / rs2) },
            FUNCT3_REM => {
                let a = rs1 as i32;
                let b = rs2 as i32;
                if b == 0 { Ok(rs1) }
                else if a == i32::MIN && b == -1 { Ok(0) }
                else { Ok((a % b) as u32) }
            }
            FUNCT3_REMU => if rs2 == 0 { Ok(rs1) } else { Ok(rs1 % rs2) },
            _ => Err(Trap64::IllegalInstruction(0)),
        }
    }

    fn execute_amo(&mut self, inst: u32, d: &DecodedInst, bus: &mut impl Bus) -> Result<(), Trap64> {
        let funct5 = (inst >> 27) & 0x1F;
        let width = d.funct3;
        let vaddr = self.read_reg(d.rs1);
        let satp = self.csr.satp;
        let mstatus = self.csr.mstatus;
        let priv_level = self.priv_level;

        let access = match funct5 {
            FUNCT5_LR => AccessType::Load,
            FUNCT5_SC => AccessType::Store,
            _ => AccessType::Store,
        };

        let paddr = match self.mmu.translate(vaddr, access, priv_level, bus, satp, mstatus) {
            Ok(pa) => pa,
            Err(cause) => return Err(Trap64::from_cause(cause, vaddr)),
        };
        let addr = Self::map_paddr(paddr)?;

        match width {
            0b010 => self.execute_amo_word(funct5, d, addr, bus, vaddr)?,
            0b011 => self.execute_amo_double(funct5, d, addr, bus, vaddr)?,
            _ => return Err(Trap64::IllegalInstruction(inst as u64)),
        }

        self.pc = self.pc.wrapping_add(4);
        Ok(())
    }

    fn execute_amo_word(&mut self, funct5: u32, d: &DecodedInst, addr: u32, bus: &mut impl Bus, vaddr: u64) -> Result<(), Trap64> {
        if vaddr & 3 != 0 {
            return Err(Trap64::StoreAddressMisaligned(vaddr));
        }

        match funct5 {
            FUNCT5_LR => {
                let val = bus.read32(addr);
                self.reservation = Some(vaddr);
                self.write_reg(d.rd, (val as i32 as i64) as u64);
            }
            FUNCT5_SC => {
                if self.reservation == Some(vaddr) {
                    bus.write32(addr, self.read_reg(d.rs2) as u32);
                    self.write_reg(d.rd, 0);
                } else {
                    self.write_reg(d.rd, 1);
                }
                self.reservation = None;
            }
            _ => {
                let rs2 = self.read_reg(d.rs2) as u32;
                let old = bus.read32(addr);
                let new = match funct5 {
                    FUNCT5_AMOSWAP => rs2,
                    FUNCT5_AMOADD => old.wrapping_add(rs2),
                    FUNCT5_AMOXOR => old ^ rs2,
                    FUNCT5_AMOAND => old & rs2,
                    FUNCT5_AMOOR => old | rs2,
                    FUNCT5_AMOMIN => if (old as i32) < (rs2 as i32) { old } else { rs2 },
                    FUNCT5_AMOMAX => if (old as i32) > (rs2 as i32) { old } else { rs2 },
                    FUNCT5_AMOMINU => if old < rs2 { old } else { rs2 },
                    FUNCT5_AMOMAXU => if old > rs2 { old } else { rs2 },
                    _ => return Err(Trap64::IllegalInstruction(0)),
                };
                bus.write32(addr, new);
                self.write_reg(d.rd, (old as i32 as i64) as u64);
            }
        }
        Ok(())
    }

    fn execute_amo_double(&mut self, funct5: u32, d: &DecodedInst, addr: u32, bus: &mut impl Bus, vaddr: u64) -> Result<(), Trap64> {
        if vaddr & 7 != 0 {
            return Err(Trap64::StoreAddressMisaligned(vaddr));
        }

        match funct5 {
            FUNCT5_LR => {
                let val = bus.read64(addr);
                self.reservation = Some(vaddr);
                self.write_reg(d.rd, val);
            }
            FUNCT5_SC => {
                if self.reservation == Some(vaddr) {
                    bus.write64(addr, self.read_reg(d.rs2));
                    self.write_reg(d.rd, 0);
                } else {
                    self.write_reg(d.rd, 1);
                }
                self.reservation = None;
            }
            _ => {
                let rs2 = self.read_reg(d.rs2);
                let old = bus.read64(addr);
                let new = match funct5 {
                    FUNCT5_AMOSWAP => rs2,
                    FUNCT5_AMOADD => old.wrapping_add(rs2),
                    FUNCT5_AMOXOR => old ^ rs2,
                    FUNCT5_AMOAND => old & rs2,
                    FUNCT5_AMOOR => old | rs2,
                    FUNCT5_AMOMIN => if (old as i64) < (rs2 as i64) { old } else { rs2 },
                    FUNCT5_AMOMAX => if (old as i64) > (rs2 as i64) { old } else { rs2 },
                    FUNCT5_AMOMINU => if old < rs2 { old } else { rs2 },
                    FUNCT5_AMOMAXU => if old > rs2 { old } else { rs2 },
                    _ => return Err(Trap64::IllegalInstruction(0)),
                };
                bus.write64(addr, new);
                self.write_reg(d.rd, old);
            }
        }
        Ok(())
    }
}
