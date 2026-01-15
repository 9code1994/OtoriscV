use otoriscv::System;
use std::fs;
use std::path::PathBuf;

#[test]
fn test_virtio_mmio_device_discovery() {
    // Load the test binary
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    d.push("virtio_test.bin");
    
    let code = fs::read(d).expect("Failed to read virtio_test.bin");
    
    // Initialize system
    let mut sys = System::new(16, None).unwrap();
    sys.load_binary(&code, 0x80000000).unwrap();
    sys.cpu.pc = 0x80000000;
    
    // Run for a fixed number of cycles or until we see output
    let max_cycles = 10000;
    let mut output = String::new();
    
    for _ in 0..max_cycles {
        let cycles = sys.run(1);
        if cycles == 0 {
            break; 
        }
        
        // Capture UART
        let out_bytes = sys.uart_get_output();
        if !out_bytes.is_empty() {
            output.push_str(&String::from_utf8_lossy(&out_bytes));
        }
        
        if output.contains("PASS: VirtIO Found") || output.contains("FAIL") {
            break;
        }
    }
    
    assert!(output.contains("PASS: VirtIO Found"), "Did not find expected success message: '{}'", output);
}
