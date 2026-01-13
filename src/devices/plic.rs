//! PLIC - Platform-Level Interrupt Controller

//!
//! Based on jor1k's plic.js
//! Handles external interrupts for RISC-V

// PLIC memory map (relative to base 0x04000000)
const PRIORITY_BASE: u32 = 0x000000;      // Interrupt priorities (4 bytes each)
const PENDING_BASE: u32 = 0x001000;       // Interrupt pending bits
const ENABLE_BASE: u32 = 0x002000;        // Interrupt enable bits per context
const THRESHOLD_BASE: u32 = 0x200000;     // Priority threshold per context
const CLAIM_BASE: u32 = 0x200004;         // Claim/complete per context

const MAX_INTERRUPTS: usize = 128;
const MAX_CONTEXTS: usize = 2; // M-mode and S-mode for hart 0

use serde::{Serialize, Deserialize};

/// PLIC device
#[derive(Serialize, Deserialize)]
pub struct Plic {
    /// Interrupt priorities (0 = disabled, 1-7 = priority)
    #[serde(with = "big_array")]
    priorities: [u8; MAX_INTERRUPTS],
    /// Pending interrupt bits
    pending: [u32; MAX_INTERRUPTS / 32],
    /// Enabled interrupts per context
    enable: [[u32; MAX_INTERRUPTS / 32]; MAX_CONTEXTS],
    /// Priority threshold per context
    threshold: [u8; MAX_CONTEXTS],
    /// Current claimed interrupt per context
    claimed: [u32; MAX_CONTEXTS],
    
    /// External interrupt pending for S-mode
    pub s_external_interrupt: bool,
    /// External interrupt pending for M-mode
    pub m_external_interrupt: bool,
}

impl Plic {
    pub fn new() -> Self {
        Plic {
            priorities: [0; MAX_INTERRUPTS],
            pending: [0; MAX_INTERRUPTS / 32],
            enable: [[0; MAX_INTERRUPTS / 32]; MAX_CONTEXTS],
            threshold: [0; MAX_CONTEXTS],
            claimed: [0; MAX_CONTEXTS],
            s_external_interrupt: false,
            m_external_interrupt: false,
        }
    }
    
    /// Raise an external interrupt
    pub fn raise_interrupt(&mut self, irq: u32) {
        if irq == 0 || irq >= MAX_INTERRUPTS as u32 {
            return;
        }
        let idx = (irq / 32) as usize;
        let bit = 1 << (irq % 32);
        self.pending[idx] |= bit;
        self.update_external_interrupts();
    }
    
    /// Clear an interrupt (called when complete)
    pub fn clear_interrupt(&mut self, irq: u32) {
        if irq == 0 || irq >= MAX_INTERRUPTS as u32 {
            return;
        }
        let idx = (irq / 32) as usize;
        let bit = 1 << (irq % 32);
        self.pending[idx] &= !bit;
        self.update_external_interrupts();
    }
    
    /// Find highest priority pending interrupt for a context
    fn find_pending(&self, context: usize) -> Option<u32> {
        let mut best_irq = None;
        let mut best_priority = 0u8;
        
        for irq in 1..MAX_INTERRUPTS as u32 {
            let idx = (irq / 32) as usize;
            let bit = 1 << (irq % 32);
            
            if (self.pending[idx] & bit) != 0 && (self.enable[context][idx] & bit) != 0 {
                let priority = self.priorities[irq as usize];
                if priority > self.threshold[context] && priority > best_priority {
                    best_priority = priority;
                    best_irq = Some(irq);
                }
            }
        }
        
        best_irq
    }
    
    fn update_external_interrupts(&mut self) {
        self.m_external_interrupt = self.find_pending(0).is_some();
        self.s_external_interrupt = self.find_pending(1).is_some();
    }
    
    fn claim(&mut self, context: usize) -> u32 {
        if let Some(irq) = self.find_pending(context) {
            let idx = (irq / 32) as usize;
            let bit = 1 << (irq % 32);
            self.pending[idx] &= !bit;
            self.claimed[context] = irq;
            self.update_external_interrupts();
            irq
        } else {
            0
        }
    }
    
    fn complete(&mut self, context: usize, irq: u32) {
        if self.claimed[context] == irq {
            self.claimed[context] = 0;
        }
        self.update_external_interrupts();
    }

    pub fn read8(&self, offset: u32) -> u8 {
        let word_offset = offset & !3;
        let byte_offset = offset & 3;
        let word = self.read32(word_offset);
        ((word >> (byte_offset * 8)) & 0xFF) as u8
    }
    
    pub fn write8(&mut self, offset: u32, value: u8) {
        let word_offset = offset & !3;
        let byte_offset = offset & 3;
        let mut word = self.read32(word_offset);
        let mask = 0xFF << (byte_offset * 8);
        word = (word & !mask) | ((value as u32) << (byte_offset * 8));
        self.write32(word_offset, word);
    }
    
    pub fn read32(&self, offset: u32) -> u32 {
        match offset {
            o if o < 0x1000 => {
                let irq = o / 4;
                if irq < MAX_INTERRUPTS as u32 {
                    self.priorities[irq as usize] as u32
                } else {
                    0
                }
            }
            o if o >= PENDING_BASE && o < PENDING_BASE + 0x80 => {
                let idx = ((o - PENDING_BASE) / 4) as usize;
                if idx < self.pending.len() { self.pending[idx] } else { 0 }
            }
            o if o >= ENABLE_BASE && o < ENABLE_BASE + 0x100 => {
                let rel = o - ENABLE_BASE;
                let context = (rel / 0x80) as usize;
                let idx = ((rel % 0x80) / 4) as usize;
                if context < MAX_CONTEXTS && idx < self.enable[0].len() {
                    self.enable[context][idx]
                } else {
                    0
                }
            }
            o if o >= THRESHOLD_BASE => {
                let rel = o - THRESHOLD_BASE;
                let context = (rel / 0x1000) as usize;
                let reg = rel % 0x1000;
                if context >= MAX_CONTEXTS { return 0; }
                match reg {
                    0 => self.threshold[context] as u32,
                    4 => self.claimed[context],
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
    
    pub fn write32(&mut self, offset: u32, value: u32) {
        match offset {
            o if o < 0x1000 => {
                let irq = o / 4;
                if irq > 0 && irq < MAX_INTERRUPTS as u32 {
                    self.priorities[irq as usize] = (value & 0x7) as u8;
                    self.update_external_interrupts();
                }
            }
            o if o >= PENDING_BASE && o < PENDING_BASE + 0x80 => {}
            o if o >= ENABLE_BASE && o < ENABLE_BASE + 0x100 => {
                let rel = o - ENABLE_BASE;
                let context = (rel / 0x80) as usize;
                let idx = ((rel % 0x80) / 4) as usize;
                if context < MAX_CONTEXTS && idx < self.enable[0].len() {
                    self.enable[context][idx] = value;
                    self.update_external_interrupts();
                }
            }
            o if o >= THRESHOLD_BASE => {
                let rel = o - THRESHOLD_BASE;
                let context = (rel / 0x1000) as usize;
                let reg = rel % 0x1000;
                if context >= MAX_CONTEXTS { return; }
                match reg {
                    0 => {
                        self.threshold[context] = (value & 0x7) as u8;
                        self.update_external_interrupts();
                    }
                    4 => self.complete(context, value),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    
    pub fn reset(&mut self) {
        self.priorities.fill(0);
        self.pending.fill(0);
        for ctx in &mut self.enable {
            ctx.fill(0);
        }
        self.threshold.fill(0);
        self.claimed.fill(0);
        self.s_external_interrupt = false;
        self.m_external_interrupt = false;
    }
}

// Helper for serializing large arrays (>32 elements)
mod big_array {
    use super::*;
    use serde::{Serializer, Deserializer};
    use serde::ser::SerializeTuple;
    use serde::de::{Visitor, SeqAccess, Error};
    use std::fmt;

    pub fn serialize<S>(data: &[u8; MAX_INTERRUPTS], serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        let mut seq = serializer.serialize_tuple(MAX_INTERRUPTS)?;
        for e in data {
            seq.serialize_element(e)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; MAX_INTERRUPTS], D::Error>
    where D: Deserializer<'de> {
        struct ArrayVisitor;

        impl<'de> Visitor<'de> for ArrayVisitor {
            type Value = [u8; MAX_INTERRUPTS];

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an array of length 128")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where A: SeqAccess<'de> {
                let mut arr = [0u8; MAX_INTERRUPTS];
                for i in 0..MAX_INTERRUPTS {
                    arr[i] = seq.next_element()?
                        .ok_or_else(|| Error::invalid_length(i, &self))?;
                }
                Ok(arr)
            }
        }

        deserializer.deserialize_tuple(MAX_INTERRUPTS, ArrayVisitor)
    }
}
