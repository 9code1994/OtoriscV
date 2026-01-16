// Debug utilities for tracking execution and detecting hangs

use std::collections::{HashMap, VecDeque};

const PC_HISTORY_SIZE: usize = 100;
const LOOP_DETECTION_THRESHOLD: usize = 10;

pub struct ExecutionTracker {
    pc_history: VecDeque<u64>,
    pc_frequency: HashMap<u64, usize>,
    instruction_count: u64,
    last_progress_count: u64,
}

impl ExecutionTracker {
    pub fn new() -> Self {
        ExecutionTracker {
            pc_history: VecDeque::with_capacity(PC_HISTORY_SIZE),
            pc_frequency: HashMap::new(),
            instruction_count: 0,
            last_progress_count: 0,
        }
    }

    pub fn track_pc(&mut self, pc: u64) {
        self.instruction_count += 1;

        // Update frequency map
        *self.pc_frequency.entry(pc).or_insert(0) += 1;

        // Add to history
        if self.pc_history.len() >= PC_HISTORY_SIZE {
            if let Some(old_pc) = self.pc_history.pop_front() {
                if let Some(count) = self.pc_frequency.get_mut(&old_pc) {
                    *count -= 1;
                    if *count == 0 {
                        self.pc_frequency.remove(&old_pc);
                    }
                }
            }
        }
        self.pc_history.push_back(pc);
        
        // Check for potential infinite loop every 1000 instructions
        if self.instruction_count % 1000 == 0 {
            self.check_for_loop();
        }
    }

    fn check_for_loop(&mut self) {
        // If we're executing the same small set of PCs repeatedly
        if self.pc_frequency.len() <= 5 {
            let mut sorted: Vec<_> = self.pc_frequency.iter().collect();
            sorted.sort_by_key(|(_pc, &count)| std::cmp::Reverse(count));
            
            eprintln!("\n[DEBUG] Potential infinite loop detected at instruction {}:", self.instruction_count);
            eprintln!("[DEBUG] Top PCs being executed:");
            for (pc, count) in sorted.iter().take(5) {
                eprintln!("[DEBUG]   PC={:#018x} count={} ({:.1}%)", 
                    pc, count, (**count as f64 / self.pc_history.len() as f64) * 100.0);
            }
            eprintln!("[DEBUG] Unique PCs in last {} instructions: {}", 
                PC_HISTORY_SIZE, self.pc_frequency.len());
        }
    }

    pub fn should_print_status(&mut self) -> bool {
        if self.instruction_count - self.last_progress_count >= 10000 {
            self.last_progress_count = self.instruction_count;
            true
        } else {
            false
        }
    }

    pub fn get_instruction_count(&self) -> u64 {
        self.instruction_count
    }

    pub fn get_unique_pc_count(&self) -> usize {
        self.pc_frequency.len()
    }
}

pub struct InterruptTracker {
    last_timer_interrupt: u64,
    timer_interrupt_count: u64,
    external_interrupt_count: u64,
    instruction_count: u64,
}

impl InterruptTracker {
    pub fn new() -> Self {
        InterruptTracker {
            last_timer_interrupt: 0,
            timer_interrupt_count: 0,
            external_interrupt_count: 0,
            instruction_count: 0,
        }
    }

    pub fn on_instruction(&mut self) {
        self.instruction_count += 1;
        
        // Warn if no timer interrupts for a long time
        if self.instruction_count - self.last_timer_interrupt > 100000 {
            if self.instruction_count % 50000 == 0 {
                eprintln!("[DEBUG] WARNING: No timer interrupts for {} instructions!", 
                    self.instruction_count - self.last_timer_interrupt);
                eprintln!("[DEBUG] Timer interrupts received: {}", self.timer_interrupt_count);
            }
        }
    }

    pub fn on_timer_interrupt(&mut self) {
        self.timer_interrupt_count += 1;
        self.last_timer_interrupt = self.instruction_count;
        
        if std::env::var("RISCV_DEBUG").is_ok() {
            eprintln!("[DEBUG] Timer interrupt #{} at instruction {}", 
                self.timer_interrupt_count, self.instruction_count);
        }
    }

    pub fn on_external_interrupt(&mut self) {
        self.external_interrupt_count += 1;
        
        if std::env::var("RISCV_DEBUG").is_ok() {
            eprintln!("[DEBUG] External interrupt #{} at instruction {}", 
                self.external_interrupt_count, self.instruction_count);
        }
    }

    pub fn get_stats(&self) -> (u64, u64, u64) {
        (self.timer_interrupt_count, self.external_interrupt_count, 
         self.instruction_count - self.last_timer_interrupt)
    }
}
