//! Instruction execution
//!
//! Implements RV32IMA instruction semantics

use super::Cpu;
use super::decode::*;
use super::csr::*; // for MSTATUS_* constants
use super::mmu::AccessType;
use crate::cpu::PrivilegeLevel;
use crate::cpu::trap::{self, Trap};
use crate::memory::Bus;

impl Cpu {
    /// Execute a single instruction
    pub fn execute(&mut self, inst: u32, bus: &mut impl Bus) -> Result<(), Trap> {
        let d = DecodedInst::decode(inst);
        
        match d.opcode {
            OP_LUI => {
                self.write_reg(d.rd, d.imm_u as u32);
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_AUIPC => {
                self.write_reg(d.rd, self.pc.wrapping_add(d.imm_u as u32));
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_JAL => {
                self.write_reg(d.rd, self.pc.wrapping_add(4));
                self.pc = self.pc.wrapping_add(d.imm_j as u32);
            }
            
            OP_JALR => {
                let target = (self.read_reg(d.rs1).wrapping_add(d.imm_i as u32)) & !1;
                self.write_reg(d.rd, self.pc.wrapping_add(4));
                self.pc = target;
            }
            
            OP_BRANCH => {
                let rs1 = self.read_reg(d.rs1);
                let rs2 = self.read_reg(d.rs2);
                
                let taken = match d.funct3 {
                    FUNCT3_BEQ => rs1 == rs2,
                    FUNCT3_BNE => rs1 != rs2,
                    FUNCT3_BLT => (rs1 as i32) < (rs2 as i32),
                    FUNCT3_BGE => (rs1 as i32) >= (rs2 as i32),
                    FUNCT3_BLTU => rs1 < rs2,
                    FUNCT3_BGEU => rs1 >= rs2,
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                
                if taken {
                    self.pc = self.pc.wrapping_add(d.imm_b as u32);
                } else {
                    self.pc = self.pc.wrapping_add(4);
                }
            }
            
            OP_LOAD => {
                let vaddr = self.read_reg(d.rs1).wrapping_add(d.imm_i as u32);
                let satp = self.csr.satp;
                let mstatus = self.csr.mstatus;
                let mut priv_level = self.priv_level;
                
                // Handle MPRV (Modify Privilege)
                if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
                    let mpp = (mstatus >> 11) & 3;
                    priv_level = PrivilegeLevel::from(mpp as u8);
                }
                
                // Translate address
                let paddr = match self.mmu.translate(vaddr, AccessType::Load, priv_level, bus, satp, mstatus) {
                    Ok(pa) => pa,
                    Err(cause) => {
                        return Err(Trap::from_cause(cause, vaddr));
                    }
                };
                
                // Emulate misaligned loads (byte-by-byte) for full hardware support
                let value = match d.funct3 {
                    FUNCT3_LB => bus.read8(paddr) as i8 as i32 as u32,
                    FUNCT3_LH => {
                        if vaddr & 1 != 0 {
                            // Misaligned halfword - do byte-by-byte load
                            let b0 = bus.read8(paddr) as u32;
                            let b1 = bus.read8(paddr.wrapping_add(1)) as u32;
                            ((b1 << 8) | b0) as i16 as i32 as u32
                        } else {
                            bus.read16(paddr) as i16 as i32 as u32
                        }
                    }
                    FUNCT3_LW => {
                        if vaddr & 3 != 0 {
                            // Misaligned word - do byte-by-byte load
                            let b0 = bus.read8(paddr) as u32;
                            let b1 = bus.read8(paddr.wrapping_add(1)) as u32;
                            let b2 = bus.read8(paddr.wrapping_add(2)) as u32;
                            let b3 = bus.read8(paddr.wrapping_add(3)) as u32;
                            (b3 << 24) | (b2 << 16) | (b1 << 8) | b0
                        } else {
                            bus.read32(paddr)
                        }
                    }
                    FUNCT3_LBU => bus.read8(paddr) as u32,
                    FUNCT3_LHU => {
                        if vaddr & 1 != 0 {
                            // Misaligned halfword - do byte-by-byte load
                            let b0 = bus.read8(paddr) as u32;
                            let b1 = bus.read8(paddr.wrapping_add(1)) as u32;
                            (b1 << 8) | b0
                        } else {
                            bus.read16(paddr) as u32
                        }
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                
                self.write_reg(d.rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_STORE => {
                let vaddr = self.read_reg(d.rs1).wrapping_add(d.imm_s as u32);
                let value = self.read_reg(d.rs2);
                let satp = self.csr.satp;
                let mstatus = self.csr.mstatus;
                let mut priv_level = self.priv_level;
                
                // Handle MPRV
                if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
                    let mpp = (mstatus >> 11) & 3;
                    priv_level = PrivilegeLevel::from(mpp as u8);
                }
                
                // Translate address
                let paddr = match self.mmu.translate(vaddr, AccessType::Store, priv_level, bus, satp, mstatus) {
                    Ok(pa) => pa,
                    Err(cause) => {
                        return Err(Trap::from_cause(cause, vaddr));
                    }
                };
                
                // Emulate misaligned stores (byte-by-byte) for full hardware support
                match d.funct3 {
                    0b000 => { // SB
                        bus.write8(paddr, value as u8);
                        self.last_write_addr = paddr;
                        self.last_write_val = value as u8 as u32;
                    }
                    0b001 => { // SH
                        if vaddr & 1 != 0 {
                            // Misaligned halfword - do byte-by-byte store
                            bus.write8(paddr, value as u8);
                            bus.write8(paddr.wrapping_add(1), (value >> 8) as u8);
                        } else {
                            bus.write16(paddr, value as u16);
                        }
                        self.last_write_addr = paddr;
                        self.last_write_val = value as u16 as u32;
                    }
                    0b010 => { // SW
                        if vaddr & 3 != 0 {
                            // Misaligned word - do byte-by-byte store
                            bus.write8(paddr, value as u8);
                            bus.write8(paddr.wrapping_add(1), (value >> 8) as u8);
                            bus.write8(paddr.wrapping_add(2), (value >> 16) as u8);
                            bus.write8(paddr.wrapping_add(3), (value >> 24) as u8);
                        } else {
                            bus.write32(paddr, value);
                        }
                        self.last_write_addr = paddr;
                        self.last_write_val = value;
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                }
                
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_OP_IMM => {
                let rs1 = self.read_reg(d.rs1);
                let imm = d.imm_i as u32;
                let shamt = (imm & 0x1F) as u32;
                
                let result = match d.funct3 {
                    FUNCT3_ADD_SUB => rs1.wrapping_add(imm), // ADDI
                    FUNCT3_SLT => if (rs1 as i32) < (imm as i32) { 1 } else { 0 }, // SLTI
                    FUNCT3_SLTU => if rs1 < imm { 1 } else { 0 }, // SLTIU
                    FUNCT3_XOR => rs1 ^ imm, // XORI
                    FUNCT3_OR => rs1 | imm, // ORI
                    FUNCT3_AND => rs1 & imm, // ANDI
                    FUNCT3_SLL => rs1 << shamt, // SLLI
                    FUNCT3_SRL_SRA => {
                        if (imm >> 10) & 1 != 0 {
                            // SRAI
                            ((rs1 as i32) >> shamt) as u32
                        } else {
                            // SRLI
                            rs1 >> shamt
                        }
                    }
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                
                self.write_reg(d.rd, result);
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_OP => {
                let rs1 = self.read_reg(d.rs1);
                let rs2 = self.read_reg(d.rs2);
                
                let result = if d.funct7 == 0b0000001 {
                    // M extension
                    self.execute_m_extension(d.funct3, rs1, rs2)?
                } else {
                    // Base integer
                    match (d.funct3, d.funct7) {
                        (FUNCT3_ADD_SUB, 0b0000000) => rs1.wrapping_add(rs2), // ADD
                        (FUNCT3_ADD_SUB, 0b0100000) => rs1.wrapping_sub(rs2), // SUB
                        (FUNCT3_SLL, 0b0000000) => rs1 << (rs2 & 0x1F), // SLL
                        (FUNCT3_SLT, 0b0000000) => if (rs1 as i32) < (rs2 as i32) { 1 } else { 0 }, // SLT
                        (FUNCT3_SLTU, 0b0000000) => if rs1 < rs2 { 1 } else { 0 }, // SLTU
                        (FUNCT3_XOR, 0b0000000) => rs1 ^ rs2, // XOR
                        (FUNCT3_SRL_SRA, 0b0000000) => rs1 >> (rs2 & 0x1F), // SRL
                        (FUNCT3_SRL_SRA, 0b0100000) => ((rs1 as i32) >> (rs2 & 0x1F)) as u32, // SRA
                        (FUNCT3_OR, 0b0000000) => rs1 | rs2, // OR
                        (FUNCT3_AND, 0b0000000) => rs1 & rs2, // AND
                        _ => return Err(Trap::IllegalInstruction(inst)),
                    }
                };
                
                self.write_reg(d.rd, result);
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_MISC_MEM => {
                // FENCE instructions - no-op in simple implementation
                self.pc = self.pc.wrapping_add(4);
            }
            
            OP_SYSTEM => {
                self.execute_system(inst, &d, bus)?;
            }
            
            OP_AMO => {
                self.execute_amo(inst, &d, bus)?;
            }
            
            // Floating-point extensions (F and D)
            OP_LOAD_FP => {
                self.execute_load_fp(inst, &d, bus)?;
            }
            
            OP_STORE_FP => {
                self.execute_store_fp(inst, &d, bus)?;
            }
            
            OP_MADD | OP_MSUB | OP_NMSUB | OP_NMADD => {
                self.execute_fma(inst, &d, d.opcode)?;
            }
            
            OP_OP_FP => {
                self.execute_op_fp(inst, &d)?;
            }
            
            _ => {
                return Err(Trap::IllegalInstruction(inst));
            }
        }
        
        Ok(())
    }
    
    /// Execute M extension instructions
    fn execute_m_extension(&self, funct3: u32, rs1: u32, rs2: u32) -> Result<u32, Trap> {
        Ok(match funct3 {
            FUNCT3_MUL => {
                // MUL - lower 32 bits of rs1 * rs2
                rs1.wrapping_mul(rs2)
            }
            FUNCT3_MULH => {
                // MULH - upper 32 bits of signed * signed
                let result = (rs1 as i32 as i64).wrapping_mul(rs2 as i32 as i64);
                (result >> 32) as u32
            }
            FUNCT3_MULHSU => {
                // MULHSU - upper 32 bits of signed * unsigned
                let result = (rs1 as i32 as i64).wrapping_mul(rs2 as u64 as i64);
                (result >> 32) as u32
            }
            FUNCT3_MULHU => {
                // MULHU - upper 32 bits of unsigned * unsigned
                let result = (rs1 as u64).wrapping_mul(rs2 as u64);
                (result >> 32) as u32
            }
            FUNCT3_DIV => {
                // DIV - signed division
                if rs2 == 0 {
                    u32::MAX // Division by zero returns -1
                } else if rs1 as i32 == i32::MIN && rs2 as i32 == -1 {
                    rs1 // Overflow case
                } else {
                    ((rs1 as i32).wrapping_div(rs2 as i32)) as u32
                }
            }
            FUNCT3_DIVU => {
                // DIVU - unsigned division
                if rs2 == 0 {
                    u32::MAX
                } else {
                    rs1 / rs2
                }
            }
            FUNCT3_REM => {
                // REM - signed remainder
                if rs2 == 0 {
                    rs1 // Division by zero returns dividend
                } else if rs1 as i32 == i32::MIN && rs2 as i32 == -1 {
                    0 // Overflow case
                } else {
                    ((rs1 as i32).wrapping_rem(rs2 as i32)) as u32
                }
            }
            FUNCT3_REMU => {
                // REMU - unsigned remainder
                if rs2 == 0 {
                    rs1
                } else {
                    rs1 % rs2
                }
            }
            _ => return Err(Trap::IllegalInstruction(0)),
        })
    }
    
    /// Execute SYSTEM instructions
    fn execute_system(&mut self, inst: u32, d: &DecodedInst, _bus: &mut impl Bus) -> Result<(), Trap> {
        match d.funct3 {
            FUNCT3_PRIV => {
                match inst {
                    0x00000073 => {
                        // ECALL
                        let trap = match self.priv_level {
                            PrivilegeLevel::User => Trap::EnvironmentCallFromU,
                            PrivilegeLevel::Supervisor => Trap::EnvironmentCallFromS,
                            PrivilegeLevel::Machine => Trap::EnvironmentCallFromM,
                        };
                        return Err(trap);
                    }
                    0x00100073 => {
                        // EBREAK
                        return Err(Trap::Breakpoint(self.pc));
                    }
                    0x10200073 => {
                        // SRET
                        if self.priv_level < PrivilegeLevel::Supervisor {
                            return Err(Trap::IllegalInstruction(inst));
                        }
                        trap::sret(self);
                        return Ok(());
                    }
                    0x30200073 => {
                        // MRET
                        if self.priv_level < PrivilegeLevel::Machine {
                            return Err(Trap::IllegalInstruction(inst));
                        }
                        trap::mret(self);
                        return Ok(());
                    }
                    0x10500073 => {
                        // WFI
                        self.wfi = true;
                        self.pc = self.pc.wrapping_add(4);
                        return Ok(());
                    }
                    _ => {
                        // SFENCE.VMA - treat as no-op for now
                        if (inst >> 25) == 0b0001001 {
                            self.pc = self.pc.wrapping_add(4);
                            return Ok(());
                        }
                        return Err(Trap::IllegalInstruction(inst));
                    }
                }
            }
            
            FUNCT3_CSRRW | FUNCT3_CSRRS | FUNCT3_CSRRC |
            FUNCT3_CSRRWI | FUNCT3_CSRRSI | FUNCT3_CSRRCI => {
                let csr_addr = (inst >> 20) & 0xFFF;
                let is_imm = d.funct3 >= FUNCT3_CSRRWI;
                
                let rs1_val = if is_imm {
                    d.rs1 // Zero-extended immediate
                } else {
                    self.read_reg(d.rs1)
                };
                
                // Handle FP CSRs specially (they live in FPU, not CSR)
                let old_val = match csr_addr {
                    CSR_FFLAGS => self.fpu.fflags.to_bits(),
                    CSR_FRM => self.fpu.frm as u32,
                    CSR_FCSR => self.fpu.read_fcsr(),
                    _ => self.csr.read(csr_addr, self.priv_level)
                        .ok_or(Trap::IllegalInstruction(inst))?,
                };
                
                // Calculate new value based on operation
                let new_val = match d.funct3 & 0x3 {
                    0b01 => rs1_val, // CSRRW(I)
                    0b10 => old_val | rs1_val, // CSRRS(I)
                    0b11 => old_val & !rs1_val, // CSRRC(I)
                    _ => old_val,
                };
                
                // Write new value (unless rs1 == 0 for RS/RC)
                if d.funct3 & 0x3 == 0b01 || rs1_val != 0 {
                    match csr_addr {
                        CSR_FFLAGS => {
                            self.fpu.fflags = crate::cpu::fpu::FFlags::from_bits(new_val & 0x1F);
                            // Set FS to dirty
                            self.csr.mstatus |= MSTATUS_FS;
                        }
                        CSR_FRM => {
                            self.fpu.frm = crate::cpu::fpu::RoundingMode::from(new_val);
                            self.csr.mstatus |= MSTATUS_FS;
                        }
                        CSR_FCSR => {
                            self.fpu.write_fcsr(new_val);
                            self.csr.mstatus |= MSTATUS_FS;
                        }
                        _ => {
                            if !self.csr.write(csr_addr, new_val, self.priv_level) {
                                return Err(Trap::IllegalInstruction(inst));
                            }
                        }
                    }
                }
                
                self.write_reg(d.rd, old_val);
                self.pc = self.pc.wrapping_add(4);
            }
            
            _ => return Err(Trap::IllegalInstruction(inst)),
        }
        
        Ok(())
    }
    
    /// Execute atomic (A extension) instructions
    fn execute_amo(&mut self, inst: u32, d: &DecodedInst, bus: &mut impl Bus) -> Result<(), Trap> {
        let vaddr = self.read_reg(d.rs1);
        
        // Check alignment
        if vaddr & 3 != 0 {
            return Err(Trap::StoreAddressMisaligned(vaddr));
        }
        
        // Get translation parameters
        let satp = self.csr.satp;
        let mstatus = self.csr.mstatus;
        let mut priv_level = self.priv_level;
        
        // Handle MPRV (Machine PReVious privilege for loads/stores)
        if (mstatus & MSTATUS_MPRV) != 0 && priv_level == PrivilegeLevel::Machine {
            let mpp = (mstatus >> 11) & 3;
            priv_level = PrivilegeLevel::from(mpp as u8);
        }
        
        let funct5 = d.funct7 >> 2;
        
        match funct5 {
            FUNCT5_LR => {
                // LR.W - Load Reserved
                // Translate virtual address to physical
                let paddr = match self.mmu.translate(vaddr, AccessType::Load, priv_level, bus, satp, mstatus) {
                    Ok(pa) => pa,
                    Err(cause) => {
                        return Err(Trap::from_cause(cause, vaddr));
                    }
                };
                
                let value = bus.read32(paddr);
                self.write_reg(d.rd, value);
                // Store VIRTUAL address for reservation (LR/SC pair uses same vaddr)
                self.reservation = Some(vaddr);
            }
            FUNCT5_SC => {
                // SC.W - Store Conditional
                // Check reservation against VIRTUAL address
                let success = self.reservation == Some(vaddr);
                
                if success {
                    let paddr = match self.mmu.translate(vaddr, AccessType::Store, priv_level, bus, satp, mstatus) {
                        Ok(pa) => pa,
                        Err(cause) => {
                            return Err(Trap::from_cause(cause, vaddr));
                        }
                    };
                    
                    bus.write32(paddr, self.read_reg(d.rs2));
                    self.write_reg(d.rd, 0); // Success
                } else {
                    self.write_reg(d.rd, 1); // Failure
                }
                self.reservation = None;
            }
            _ => {
                // AMO operations - need to translate and then do atomic read-modify-write
                let paddr = match self.mmu.translate(vaddr, AccessType::Store, priv_level, bus, satp, mstatus) {
                    Ok(pa) => pa,
                    Err(cause) => {
                        return Err(Trap::from_cause(cause, vaddr));
                    }
                };
                
                let old_val = bus.read32(paddr);
                let rs2 = self.read_reg(d.rs2);
                
                let new_val = match funct5 {
                    FUNCT5_AMOSWAP => rs2,
                    FUNCT5_AMOADD => old_val.wrapping_add(rs2),
                    FUNCT5_AMOXOR => old_val ^ rs2,
                    FUNCT5_AMOAND => old_val & rs2,
                    FUNCT5_AMOOR => old_val | rs2,
                    FUNCT5_AMOMIN => std::cmp::min(old_val as i32, rs2 as i32) as u32,
                    FUNCT5_AMOMAX => std::cmp::max(old_val as i32, rs2 as i32) as u32,
                    FUNCT5_AMOMINU => std::cmp::min(old_val, rs2),
                    FUNCT5_AMOMAXU => std::cmp::max(old_val, rs2),
                    _ => return Err(Trap::IllegalInstruction(inst)),
                };
                
                bus.write32(paddr, new_val);
                self.write_reg(d.rd, old_val);
            }
        }
        
        self.pc = self.pc.wrapping_add(4);
        Ok(())
    }
}
