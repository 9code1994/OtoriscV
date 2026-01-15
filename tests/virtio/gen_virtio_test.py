import struct

def instruction(val):
    return struct.pack('<I', val)

# Helper functions for instruction encoding
def lui(rd, imm):
    return instruction((imm << 12) | (rd << 7) | 0x37)

def addi(rd, rs1, imm):
    return instruction(((imm & 0xFFF) << 20) | (rs1 << 15) | (0 << 12) | (rd << 7) | 0x13)

def lw(rd, rs1, imm):
    return instruction(((imm & 0xFFF) << 20) | (rs1 << 15) | (2 << 12) | (rd << 7) | 0x03)

def bne_offset(rs1, rs2, offset):
    # Branch if rs1 != rs2
    imm12 = (offset >> 12) & 1
    imm11 = (offset >> 11) & 1
    imm10_5 = (offset >> 5) & 0x3F
    imm4_1 = (offset >> 1) & 0xF
    return instruction((imm12 << 31) | (imm10_5 << 25) | (rs2 << 20) | (rs1 << 15) | (1 << 12) | (imm4_1 << 8) | (imm11 << 7) | 0x63)

def jal_offset(rd, offset):
    # Jump and link
    # offset is signed
    offset_u = offset & 0xffffffff
    imm20 = (offset_u >> 20) & 1
    imm10_1 = (offset_u >> 1) & 0x3FF
    imm11 = (offset_u >> 11) & 1
    imm19_12 = (offset_u >> 12) & 0xFF
    return instruction((imm20 << 31) | (imm10_1 << 21) | (imm11 << 20) | (imm19_12 << 12) | (rd << 7) | 0x6F)

def li(rd, val):
    # Load immediate 32-bit
    upper = (val >> 12) & 0xFFFFF
    lower = val & 0xFFF
    # Sign extend lower if it's negative (12-bit)
    if lower >= 2048:
        upper += 1
        lower -= 4096
    
    code = b''
    if upper != 0:
        code += lui(rd, upper)
        if lower != 0:
            code += addi(rd, rd, lower)
    else:
        code += addi(rd, 0, lower)
    return code

# Simple UART putc (hardcoded address)
UART_BASE = 0x03000000
VIRTIO_BASE = 0x20000000

# Registers
x0 = 0
ra = 1
t0 = 5
t1 = 6
t2 = 7
t3 = 28
t4 = 29
t5 = 30
t6 = 31

def gen_puts_code():
    # Expects address of string in t0
    # Clobbers t1, t2
    code = b''
    
    # Load UART base
    code += li(t1, UART_BASE)
    
    # Loop start
    # 1. lb t2, 0(t0)
    def lb(rd, rs1, imm):
        return instruction(((imm & 0xFFF) << 20) | (rs1 << 15) | (0 << 12) | (rd << 7) | 0x03)
        
    def sb(rs2, rs1, imm):
        imm11_5 = (imm >> 5) & 0x7F
        imm4_0 = imm & 0x1F
        return instruction((imm11_5 << 25) | (rs2 << 20) | (rs1 << 15) | (0 << 12) | (imm4_0 << 7) | 0x23)

    # Loop body:
    # label_loop:
    #   lb t2, 0(t0)      (4 bytes)
    #   beq t2, x0, end   (4 bytes) -> offset 16 (jump over sb, addi, j)
    #   sb t2, 0(t1)      (4 bytes)
    #   addi t0, t0, 1    (4 bytes)
    #   j label_loop      (4 bytes) -> offset -16
    # label_end:
    
    code += lb(t2, t0, 0)
    
    # beq t2, x0, +16
    offset = 16
    imm12 = (offset >> 12) & 1
    imm11 = (offset >> 11) & 1
    imm10_5 = (offset >> 5) & 0x3F
    imm4_1 = (offset >> 1) & 0xF
    code += instruction((imm12 << 31) | (imm10_5 << 25) | (x0 << 20) | (t2 << 15) | (0 << 12) | (imm4_1 << 8) | (imm11 << 7) | 0x63)
    
    code += sb(t2, t1, 0)
    code += addi(t0, t0, 1)
    
    # j loop (-16)
    code += jal_offset(x0, -16)
    
    return code

def gen_virtio_test():
    code = b''
    data = b''
    string_offset = 0x80000000 + 0x1000 # Put strings at +4KB
    
    strings = {}
    def add_string(s):
        nonlocal data
        if s in strings:
            return strings[s]
        addr = string_offset + len(data)
        data += s.encode('utf-8') + b'\0'
        strings[s] = addr
        return addr

    # Load VirtIO Base
    code += li(t3, VIRTIO_BASE)
    
    # Load Magic (offset 0)
    code += lw(t4, t3, 0)
    
    # Expected: 0x74726976
    code += li(t5, 0x74726976)
    
    pass_addr = add_string("PASS: VirtIO Found\n")
    fail_addr = add_string("FAIL: Magic Mismatch\n")
    
    # bne t4, t5, load_fail
    # If equal, fall through to load_pass
    
    # Offset calculation:
    # We want to skip the load_pass block if equal fails.
    # load_pass block:
    #   li t0, pass_addr  (2 instrs, 8 bytes)
    #   j print_chk       (1 instr, 4 bytes)
    # Total 12 bytes.
    # So bne offset = 16 (jump over 12 bytes to next instr)
    
    code += bne_offset(t4, t5, 16)
    
    # load_pass:
    code += li(t0, pass_addr)
    # Jump to print (skip load_fail)
    # load_fail block is: li t0, fail_addr (2 instrs, 8 bytes)
    # so jump 12 bytes to land after it?
    # No, jump 8 bytes + 4 (instruction itself) = 12 bytes.
    code += jal_offset(x0, 12)
    
    # load_fail:
    code += li(t0, fail_addr)
    
    # print_chk:
    code += gen_puts_code()
    
    # Halt
    code += jal_offset(x0, 0)

    return code, data

code, data = gen_virtio_test()

if len(code) > 0x1000:
    print("Error: Code too large")
else:
    padding = b'\0' * (0x1000 - len(code))
    final = code + padding + data
    with open('virtio_test.bin', 'wb') as f:
        f.write(final)
    print(f"Generated virtio_test.bin, size {len(final)}")
