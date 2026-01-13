use std::env;
use std::fs::File;
use std::io::{self, Read, Write, stdout};
use std::time::{Instant, Duration};
use std::thread;

// Use the library crate's modules
use otoriscv::System;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut kernel_path = String::new();
    let mut ram_size_mb = 64;
    let mut signature_file = String::new();
    let mut sig_begin = 0u32;
    let mut sig_end = 0u32;
    let mut raw_mode = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--ram" => {
                i += 1;
                ram_size_mb = args[i].parse().expect("Invalid RAM size");
            }
            "--signature" => {
                i += 1;
                signature_file = args[i].clone();
            }
            "--begin" => {
                i += 1;
                sig_begin = u32::from_str_radix(args[i].trim_start_matches("0x"), 16).expect("Invalid begin addr");
            }
            "--end" => {
                i += 1;
                sig_end = u32::from_str_radix(args[i].trim_start_matches("0x"), 16).expect("Invalid end addr");
            }
            "--raw" => {
                raw_mode = true;
            }
            arg if !arg.starts_with("-") => {
                kernel_path = arg.to_string();
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
            }
        }
        i += 1;
    }

    if kernel_path.is_empty() {
        eprintln!("Usage: {} <kernel-image> [--ram <mb>] [--signature <file> --begin <addr> --end <addr>] [--raw]", args[0]);
        std::process::exit(1);
    }
    
    println!("OtoRISCV CLI");
    println!("Loading kernel: {}", kernel_path);
    println!("RAM Size: {} MB", ram_size_mb);
    
    let mut system = System::new(ram_size_mb).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    
    // Read kernel file
    let mut f = File::open(&kernel_path)?;
    let mut kernel_data = Vec::new();
    f.read_to_end(&mut kernel_data)?;
    
    if raw_mode {
        system.load_binary(&kernel_data, 0x80000000).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        system.cpu.pc = 0x80000000;
    } else {
        // Setup Linux boot
        let cmdline = "console=ttyS0 root=/dev/vda ro";
        system.setup_linux_boot(&kernel_data, cmdline).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    }
    
    println!("System ready. Starting emulation...");
    println!("-------------------------------------");
    
    let mut instructions: u64 = 0;
    let max_cycles = 500_000_000; // Safety limit
    
    loop {
        // Run a batch of cycles
        let cycles_to_run = 1000;
        let cycles_run = system.run(cycles_to_run);
        instructions += cycles_run as u64;
        
        // Handle UART Output
        let output = system.uart_get_output();
        if !output.is_empty() {
             stdout().write_all(&output)?;
             stdout().flush()?;
        }
        
        // Check for halt conditions (EBREAK for tests, or WFI loop)
        // Compliance tests often treat EBREAK as halt
        let mcause = system.cpu.csr.mcause;
        if mcause == 3 { // Breakpoint
             println!("\nHit breakpoint (EBREAK) at {:x}, halting.", system.cpu.pc);
             break;
        }

        if system.cpu.pc == 0 {
             println!("\nPC jumped to 0, halting.");
             break;
        }
        
        if instructions > max_cycles {
             println!("\nTimeout reached, halting.");
             break;
        }

        if cycles_run == 0 {
             // WFI or halted
             break;
        }
    }

    // Dump signature if requested
    if !signature_file.is_empty() && sig_begin != 0 && sig_end != 0 {
        let mut sig_data = String::new();
        let mut addr = sig_begin;
        while addr < sig_end {
             // Read memory (4 bytes at a time)
             let bytes = system.read_memory(addr, 4);
             if bytes.len() == 4 {
                 let val = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                 // Format as lower-case hex, 8 digits, one per line
                 sig_data.push_str(&format!("{:08x}\n", val));
             }
             addr += 4;
        }
        
        let mut f = File::create(signature_file)?;
        f.write_all(sig_data.as_bytes())?;
        println!("Signature dumped.");
    }
    
    Ok(())
}
