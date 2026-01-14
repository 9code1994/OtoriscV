use std::env;
use std::fs::File;
use std::io::{self, Read, Write, stdout};
use std::time::{Duration, Instant};

// Use the library crate's modules
use otoriscv::System;

// Set stdin to non-blocking mode
fn set_nonblocking(fd: i32, nonblocking: bool) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if nonblocking {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        } else {
            libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
    }
}

// Set terminal to raw mode (no echo, no line buffering)
fn set_raw_terminal(enable: bool) {
    use std::mem::MaybeUninit;
    
    static mut ORIG_TERMIOS: MaybeUninit<libc::termios> = MaybeUninit::uninit();
    static mut SAVED: bool = false;
    
    unsafe {
        let fd = libc::STDIN_FILENO;
        
        if enable {
            // Save original settings
            if !SAVED {
                libc::tcgetattr(fd, ORIG_TERMIOS.as_mut_ptr());
                SAVED = true;
            }
            
            // Get current settings and modify
            let mut raw: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut raw);
            
            // Disable canonical mode and echo, but KEEP ISIG for Ctrl+C
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            // Disable special input processing
            raw.c_iflag &= !(libc::IXON | libc::ICRNL);
            // Set minimum chars and timeout for read
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            
            libc::tcsetattr(fd, libc::TCSANOW, &raw);
        } else {
            // Restore original settings
            if SAVED {
                libc::tcsetattr(fd, libc::TCSANOW, ORIG_TERMIOS.as_ptr());
            }
        }
    }
}

struct BenchmarkConfig {
    enabled: bool,
    exit_on_prompt: bool,
    fast_mode: bool,
}

struct BenchmarkResult {
    wall_time: Duration,
    boot_time: Option<Duration>,
    instructions: u64,
}

fn output_has_prompt(buffer: &[u8]) -> bool {
    const PROMPTS: [&[u8]; 4] = [b"\n# ", b"\n$ ", b"\n~ $", b"\n~# "];
    PROMPTS.iter().any(|pat| buffer.windows(pat.len()).any(|w| w == *pat))
}

fn run_emulator(system: &mut System, config: &BenchmarkConfig) -> io::Result<BenchmarkResult> {
    let start = Instant::now();
    let mut boot_time: Option<Duration> = None;
    let mut instructions: u64 = 0;
    let max_cycles = 10_000_000_000u64; // 10 billion cycles = 1000 seconds at 10MHz
    let mut stdin_buf = [0u8; 16];
    let mut prompt_buffer: Vec<u8> = Vec::new();
    const PROMPT_BUFFER_MAX: usize = 128;
    
    loop {
        // Check for stdin input (non-blocking)
        let n = unsafe {
            libc::read(0, stdin_buf.as_mut_ptr() as *mut libc::c_void, stdin_buf.len())
        };
        if n > 0 {
            for i in 0..n as usize {
                // Convert CR to LF for consistency
                let c = if stdin_buf[i] == b'\r' { b'\n' } else { stdin_buf[i] };
                system.uart_receive(c);
            }
        }
        
        // Run a batch of cycles
        let cycles_to_run = 10000;
        let cycles_run = if config.fast_mode {
            system.run_fast(cycles_to_run)
        } else {
            system.run(cycles_to_run)
        };
        instructions += cycles_run as u64;
        
        // Handle UART Output
        let output = system.uart_get_output();
        if !output.is_empty() {
            stdout().write_all(&output)?;
            stdout().flush()?;
            if config.enabled && boot_time.is_none() {
                prompt_buffer.extend_from_slice(&output);
                if prompt_buffer.len() > PROMPT_BUFFER_MAX {
                    let excess = prompt_buffer.len() - PROMPT_BUFFER_MAX;
                    prompt_buffer.drain(0..excess);
                }
                if output_has_prompt(&prompt_buffer) {
                    boot_time = Some(start.elapsed());
                    if config.exit_on_prompt {
                        break;
                    }
                }
            }
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
    
    Ok(BenchmarkResult {
        wall_time: start.elapsed(),
        boot_time,
        instructions: system.get_instruction_count(),
    })
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut kernel_path = String::new();
    let mut initrd_path = String::new();
    let mut ram_size_mb = 64;
    let mut signature_file = String::new();
    let mut sig_begin = 0u32;
    let mut sig_end = 0u32;
    let mut raw_mode = false;
    let mut fs_path = String::new();
    let mut config = BenchmarkConfig {
        enabled: false,
        exit_on_prompt: false,
        fast_mode: false,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--ram" => {
                i += 1;
                ram_size_mb = args[i].parse().expect("Invalid RAM size");
            }
            "--initrd" => {
                i += 1;
                initrd_path = args[i].clone();
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
            "--benchmark" => {
                config.enabled = true;
                config.exit_on_prompt = true;
            }
            "--still-broken-fast-mode" => {
                config.fast_mode = true;
            }
            "--fs" => {
                i += 1;
                fs_path = args[i].clone();
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
        eprintln!("Usage: {} <kernel-image> [--initrd <initrd>] [--ram <mb>] [--fs <host-path>] [--signature <file> --begin <addr> --end <addr>] [--raw] [--benchmark] [--still-broken-fast-mode]", args[0]);
        std::process::exit(1);
    }
    
    println!("OtoRISCV CLI");
    println!("Loading kernel: {}", kernel_path);
    if !initrd_path.is_empty() {
        println!("Loading initrd: {}", initrd_path);
    }
    if !fs_path.is_empty() {
        println!("Mounting host path: {}", fs_path);
    }
    println!("RAM Size: {} MB", ram_size_mb);
    
    let fs_option = if !fs_path.is_empty() { Some(fs_path.as_str()) } else { None };
    let mut system = System::new(ram_size_mb, fs_option).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    
    // Read kernel file
    let mut f = File::open(&kernel_path)?;
    let mut kernel_data = Vec::new();
    f.read_to_end(&mut kernel_data)?;
    
    // Read initrd if provided
    let initrd_data = if !initrd_path.is_empty() {
        let mut f = File::open(&initrd_path)?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        Some(data)
    } else {
        None
    };
    
    if raw_mode {
        system.load_binary(&kernel_data, 0x80000000).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        system.cpu.pc = 0x80000000;
    } else {
        // Setup Linux boot with optional initrd
        // lpj=10000 skips delay calibration loop for fast boot in emulator
        // Use console=ttyS0 for NS16550 UART
        let cmdline = if initrd_data.is_some() {
            "lpj=10000 console=ttyS0 earlycon rdinit=/sbin/init"
        } else {
            "lpj=10000 console=ttyS0 earlycon root=/dev/vda ro"
        };
        
        system.setup_linux_boot_with_initrd(
            &kernel_data, 
            initrd_data.as_deref(), 
            cmdline
        ).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    }
    
    println!("System ready. Starting emulation...");
    println!("-------------------------------------");
    
    // Enable raw terminal mode for interactive input
    set_raw_terminal(true);
    set_nonblocking(0, true);
    
    // Ensure we restore terminal on exit
    let result = run_emulator(&mut system, &config);
    
    // Restore terminal
    set_raw_terminal(false);
    set_nonblocking(0, false);
    
    let bench_result = result?;

    if config.enabled {
        let wall_secs = bench_result.wall_time.as_secs_f64();
        let ips = if wall_secs > 0.0 {
            (bench_result.instructions as f64) / wall_secs
        } else {
            0.0
        };
        let (tlb_hits, tlb_misses) = system.get_tlb_stats();
        let tlb_total = tlb_hits + tlb_misses;
        let tlb_hit_rate = if tlb_total > 0 {
            (tlb_hits as f64) / (tlb_total as f64)
        } else {
            0.0
        };
        println!("\nBenchmark results:");
        if let Some(bt) = bench_result.boot_time {
            println!("  Boot time: {:.3}s", bt.as_secs_f64());
        } else {
            println!("  Boot time: N/A (prompt not detected)");
        }
        println!("  Instructions: {}", bench_result.instructions);
        println!("  IPS: {:.3}", ips);
        if tlb_total > 0 {
            println!("  TLB hit rate: {:.3}", tlb_hit_rate);
        } else {
            println!("  TLB hit rate: N/A (paging disabled)");
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
