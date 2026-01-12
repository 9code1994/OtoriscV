//! UART 16550 compatible device
//!
//! Based on jor1k's uart.js implementation

use std::collections::VecDeque;

// UART registers (offset from base)
const UART_RBR: u32 = 0; // Receive Buffer Register (read)
const UART_THR: u32 = 0; // Transmitter Holding Register (write)
const UART_IER: u32 = 1; // Interrupt Enable Register
const UART_IIR: u32 = 2; // Interrupt Identification Register (read)
const UART_FCR: u32 = 2; // FIFO Control Register (write)
const UART_LCR: u32 = 3; // Line Control Register
const UART_MCR: u32 = 4; // Modem Control Register
const UART_LSR: u32 = 5; // Line Status Register
const UART_MSR: u32 = 6; // Modem Status Register
const UART_SCR: u32 = 7; // Scratch Register

// Line Status Register bits
const LSR_DATA_READY: u8 = 0x01;        // Data available
const LSR_TX_EMPTY: u8 = 0x20;          // TX buffer empty
const LSR_TRANSMITTER_EMPTY: u8 = 0x40; // TX empty and line idle

// Interrupt Enable Register bits
const IER_RX_AVAILABLE: u8 = 0x01;
const IER_TX_EMPTY: u8 = 0x02;

// Interrupt Identification Register values
const IIR_NO_INTERRUPT: u8 = 0x01;
const IIR_TX_EMPTY: u8 = 0x02;
const IIR_RX_AVAILABLE: u8 = 0x04;
const IIR_FIFO_ENABLED: u8 = 0xC0;

/// UART 16550 device
pub struct Uart {
    /// Receive FIFO
    rx_fifo: VecDeque<u8>,
    /// Transmit buffer (output to host)
    tx_buffer: Vec<u8>,
    
    /// Interrupt Enable Register
    ier: u8,
    /// Line Control Register
    lcr: u8,
    /// Modem Control Register
    mcr: u8,
    /// Scratch Register
    scr: u8,
    /// FIFO Control (write-only, but we track if FIFOs enabled)
    fifo_enabled: bool,
    /// Divisor latch (when DLAB set)
    divisor: u16,
    
    /// Pending interrupt
    pending_interrupt: bool,
    /// Interrupt line number
    pub interrupt_line: u32,
}

impl Uart {
    pub fn new(interrupt_line: u32) -> Self {
        Uart {
            rx_fifo: VecDeque::new(),
            tx_buffer: Vec::new(),
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            fifo_enabled: false,
            divisor: 0,
            pending_interrupt: false,
            interrupt_line,
        }
    }
    
    /// Receive a character from host (keyboard input)
    pub fn receive_char(&mut self, c: u8) {
        self.rx_fifo.push_back(c);
        self.check_interrupt();
    }
    
    /// Get pending TX output
    pub fn get_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx_buffer)
    }
    
    /// Check if interrupt should be raised
    fn check_interrupt(&mut self) {
        let mut interrupt = false;
        
        // RX data available and RX interrupt enabled
        if !self.rx_fifo.is_empty() && (self.ier & IER_RX_AVAILABLE) != 0 {
            interrupt = true;
        }
        
        // TX empty and TX interrupt enabled
        if (self.ier & IER_TX_EMPTY) != 0 {
            interrupt = true;
        }
        
        self.pending_interrupt = interrupt;
    }
    
    /// Check if interrupt is pending
    pub fn has_interrupt(&self) -> bool {
        self.pending_interrupt
    }
    
    /// Get Line Status Register value
    fn get_lsr(&self) -> u8 {
        let mut lsr = LSR_TX_EMPTY | LSR_TRANSMITTER_EMPTY;
        if !self.rx_fifo.is_empty() {
            lsr |= LSR_DATA_READY;
        }
        lsr
    }
    
    /// Get Interrupt Identification Register value
    fn get_iir(&self) -> u8 {
        let fifo_bits = if self.fifo_enabled { IIR_FIFO_ENABLED } else { 0 };
        
        // Priority: RX available > TX empty
        if !self.rx_fifo.is_empty() && (self.ier & IER_RX_AVAILABLE) != 0 {
            return fifo_bits | IIR_RX_AVAILABLE;
        }
        
        if (self.ier & IER_TX_EMPTY) != 0 {
            return fifo_bits | IIR_TX_EMPTY;
        }
        
        fifo_bits | IIR_NO_INTERRUPT
    }
    
    /// Check if DLAB (Divisor Latch Access Bit) is set
    fn is_dlab_set(&self) -> bool {
        (self.lcr & 0x80) != 0
    }

    /// Read register
    pub fn read8(&self, offset: u32) -> u8 {
        match offset {
            UART_RBR => {
                if self.is_dlab_set() {
                    self.divisor as u8
                } else {
                    self.rx_fifo.front().copied().unwrap_or(0)
                }
            }
            UART_IER => {
                if self.is_dlab_set() {
                    (self.divisor >> 8) as u8
                } else {
                    self.ier
                }
            }
            UART_IIR => self.get_iir(),
            UART_LCR => self.lcr,
            UART_MCR => self.mcr,
            UART_LSR => self.get_lsr(),
            UART_MSR => 0,
            UART_SCR => self.scr,
            _ => 0,
        }
    }
    
    /// Write register
    pub fn write8(&mut self, offset: u32, value: u8) {
        match offset {
            UART_THR => {
                if self.is_dlab_set() {
                    self.divisor = (self.divisor & 0xFF00) | (value as u16);
                } else {
                    self.tx_buffer.push(value);
                }
            }
            UART_IER => {
                if self.is_dlab_set() {
                    self.divisor = (self.divisor & 0x00FF) | ((value as u16) << 8);
                } else {
                    self.ier = value & 0x0F;
                    self.check_interrupt();
                }
            }
            UART_FCR => {
                self.fifo_enabled = (value & 0x01) != 0;
                if (value & 0x02) != 0 {
                    self.rx_fifo.clear();
                }
            }
            UART_LCR => self.lcr = value,
            UART_MCR => self.mcr = value,
            UART_SCR => self.scr = value,
            _ => {}
        }
        self.check_interrupt();
    }
    
    pub fn read32(&self, offset: u32) -> u32 {
        self.read8(offset) as u32
    }
    
    pub fn write32(&mut self, offset: u32, value: u32) {
        self.write8(offset, value as u8);
    }
    
    /// Reset device
    pub fn reset(&mut self) {
        self.rx_fifo.clear();
        self.tx_buffer.clear();
        self.ier = 0;
        self.lcr = 0;
        self.mcr = 0;
        self.scr = 0;
        self.fifo_enabled = false;
        self.divisor = 0;
        self.pending_interrupt = false;
    }

    /// Read and consume from RX FIFO (called when actually reading RBR)
    pub fn read_rbr(&mut self) -> u8 {
        if self.is_dlab_set() {
            self.divisor as u8
        } else {
            let c = self.rx_fifo.pop_front().unwrap_or(0);
            self.check_interrupt();
            c
        }
    }
}
