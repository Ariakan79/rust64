// The CPU
#![allow(non_snake_case)]
//extern crate sdl2;
use c64::opcodes::*;
use c64::memory;
use c64::vic;
use c64::cia;
use std::cell::RefCell;
use std::rc::Rc;

use utils;

pub type CPUShared = Rc<RefCell<CPU>>;


// status flags for P register
pub enum StatusFlag
{
    Carry            = 1 << 0,
    Zero             = 1 << 1,
    InterruptDisable = 1 << 2,
    DecimalMode      = 1 << 3,
    Break            = 1 << 4,
    Unused           = 1 << 5,
    Overflow         = 1 << 6,
    Negative         = 1 << 7,
}

// action to perform on specific CIA and VIC events
pub enum CallbackAction
{
    None,
    TriggerVICIrq,
    ClearVICIrq,
    TriggerCIAIrq,
    ClearCIAIrq,
    TriggerNMI,
    ClearNMI
}

pub static NMI_VECTOR:   u16 = 0xFFFA;
pub static RESET_VECTOR: u16 = 0xFFFC;
pub static IRQ_VECTOR:   u16 = 0xFFFE;

enum CPUState
{
    FetchOp,
    FetchOperand,
    ExecuteOp
}

pub struct CPU
{
    pub PC: u16, // program counter
    pub SP: u8,  // stack pointer
    pub P: u8,   // processor status
    pub A: u8,   // accumulator
    pub X: u8,   // index register
    pub Y: u8,   // index register
    pub mem_ref: Option<memory::MemShared>, // reference to shared system memory
    pub vic_ref: Option<vic::VICShared>,
    pub cia1_ref: Option<cia::CIAShared>,
    pub cia2_ref: Option<cia::CIAShared>,
    curr_instr: Instruction,
    pub ba_low: bool,  // is BA low?
    pub cia_irq: bool,
    pub vic_irq: bool,
    state: CPUState,
    irq_cycles: u8,
    op_cycles: u8,
    curr_op: u8,
    nmi: bool,
    pub prev_PC: u16, // previous program counter - for debugging
    dfff_byte: u8,
    pub op_debugger: utils::OpDebugger
}

impl CPU
{
    pub fn new_shared() -> CPUShared
    {
        Rc::new(RefCell::new(CPU
        {
            PC: 0,
            SP: 0xFF,
            P: 0,
            A: 0,
            X: 0,
            Y: 0,
            mem_ref: None,
            vic_ref: None,
            cia1_ref: None,
            cia2_ref: None,
            ba_low: false,
            cia_irq: false,
            vic_irq: false,
            state: CPUState::FetchOp,
            irq_cycles: 0,
            op_cycles: 0,
            curr_instr: Instruction::new(Op::BRK, AddrMode::Implied),
            curr_op: 0,
            nmi: false,
            prev_PC: 0,
            dfff_byte: 0x55,
            op_debugger: utils::OpDebugger::new()
        }))
    }

    pub fn set_references(&mut self, memref: memory::MemShared, vicref: vic::VICShared, cia1ref: cia::CIAShared, cia2ref: cia::CIAShared)
    {
        self.mem_ref = Some(memref);
        self.vic_ref = Some(vicref);
        self.cia1_ref = Some(cia1ref);
        self.cia2_ref = Some(cia2ref);
    }    
    
    pub fn set_status_flag(&mut self, flag: StatusFlag, value: bool)
    {
        if value { self.P |=   flag as u8;  }
        else     { self.P &= !(flag as u8); }
    }

    pub fn get_status_flag(&mut self, flag: StatusFlag) -> bool
    {
        self.P & flag as u8 != 0x00
    }

    // these flags will be set in tandem quite often
    pub fn set_zn_flags(&mut self, value: u8)
    {
        self.set_status_flag(StatusFlag::Zero, value == 0x00);
        self.set_status_flag(StatusFlag::Negative, (value as i8) < 0);
    }
    
    pub fn reset(&mut self)
    {
        // reset program counter
        let pc = self.read_word_le(RESET_VECTOR);
        self.PC = pc;
        self.SP = 0xFF;
        self.ba_low = false;
        self.cia_irq = false;
        self.vic_irq = false;
        self.nmi = false;
    }

    pub fn update(&mut self)
    {
        match self.state
        {
            CPUState::FetchOp => {
                if self.ba_low { return; }
                let next_op = self.next_byte();
                //self.curr_instr = opcodes::Instruction::new(opcodes::AddrMode::Implied);
                
                // implied addressed mode instructions don't fetch operands
                self.state = match self.curr_instr.addr_mode {
                    AddrMode::Implied => CPUState::ExecuteOp,
                    _ => CPUState::FetchOperand
                };
            },
            CPUState::FetchOperand => {
                self.fetch_operand();
                self.state = CPUState::ExecuteOp;
            }
            CPUState::ExecuteOp => {
                self.run_instruction();
                
                self.state = CPUState::FetchOp;
            }
        }
        /*if self.process_nmi() { self.irq_cycles = 7; }
        else if self.process_irq() { self.irq_cycles = 7; }
        
        if !self.ba_low {

            if self.irq_cycles > 0
            {
                self.irq_cycles -= 1;
                return
            }
            
            if self.op_cycles == 0
            {
                self.curr_op = self.next_byte();
                let co = self.curr_op;
                self.op_cycles = self.get_op_cycles(co);
            }

            if self.op_cycles > 0
            {
                self.op_cycles -= 1;
            }

            if self.op_cycles == 0
            {
                let co = self.curr_op;
                self.process_op(co);
            }
        }*/
    }

    pub fn next_byte(&mut self) -> u8
    {
        let pc = self.PC;
        let op = self.read_byte(pc);
        self.PC += 1;
        op
    }

    pub fn next_word(&mut self) -> u16
    {
        let word = self.read_word_le(self.PC);
        self.PC += 2;
        word
    }
    

    // stack memory: $0100 - $01FF (256 byes)
    // TODO: some extra message if stack over/underflow occurs? (right now handled by Rust)
    pub fn push_byte(&mut self, value: u8)
    {
        self.SP -= 0x01;
        let newSP = (self.SP + 0x01) as u16;
        self.write_byte(0x0100 + newSP, value);
    }

    pub fn pop_byte(&mut self) -> u8
    {
        let addr = 0x0100 + (self.SP + 0x01) as u16;
        let value = self.read_byte(addr);
        self.SP += 0x01;
        value
    }

    pub fn push_word(&mut self, value: u16)
    {
        self.SP -= 0x02;
        self.write_word_le(0x0100 + (self.SP + 0x01) as u16, value);
    }

    pub fn pop_word(&mut self) -> u16
    {
        let value = self.read_word_le(0x0100 + (self.SP + 0x01) as u16);
        self.SP += 0x02;
        value
    }

    pub fn write_byte(&mut self, addr: u16, value: u8) -> bool
    {
        let mut write_callback = CallbackAction::None;
        let mut mem_write_ok = true;
        let io_enabled = as_ref!(self.mem_ref).io_on;

        match addr
        {
            // VIC-II address space
            0xD000...0xD3FF => {
                if io_enabled
                {
                    as_mut!(self.vic_ref).write_register(addr, value, &mut write_callback);
                }
                else
                {
                    mem_write_ok = as_mut!(self.mem_ref).write_byte(addr, value);
                }
            },
            // color RAM address space
            0xD800...0xDBFF => {
                if io_enabled
                {
                    mem_write_ok = as_mut!(self.mem_ref).write_byte(addr, value & 0x0F);
                }
                else
                {
                    mem_write_ok = as_mut!(self.mem_ref).write_byte(addr, value);
                }
            },
            // CIA1 address space
            0xDC00...0xDCFF => {
                if io_enabled
                {
                    as_mut!(self.cia1_ref).write_register(addr, value, &mut write_callback);
                }
                else
                {
                    mem_write_ok = as_mut!(self.mem_ref).write_byte(addr, value);
                }
            },
            // CIA2 address space
            0xDD00...0xDDFF => {
                if io_enabled
                {
                    as_mut!(self.cia2_ref).write_register(addr, value, &mut write_callback);
                }
                else
                {
                    mem_write_ok = as_mut!(self.mem_ref).write_byte(addr, value);
                }
            },
            _ => mem_write_ok = as_mut!(self.mem_ref).write_byte(addr, value),
        }

        // on VIC/CIA register write perform necessary action on the CPU
        match write_callback
        {
            CallbackAction::TriggerVICIrq => self.trigger_vic_irq(),
            CallbackAction::ClearVICIrq   => self.clear_vic_irq(),
            CallbackAction::TriggerCIAIrq => self.trigger_cia_irq(),
            CallbackAction::ClearCIAIrq   => self.clear_cia_irq(),
            CallbackAction::TriggerNMI    => self.trigger_nmi(),
            CallbackAction::ClearNMI      => self.clear_nmi(),
            _ => (),
        }

        mem_write_ok
    }
    
    pub fn read_byte(&mut self, addr: u16) -> u8
    {
        let byte: u8;
        let mut read_callback = CallbackAction::None;
        let io_enabled = as_ref!(self.mem_ref).io_on;
        match addr
        {
            // VIC-II address space
            0xD000...0xD3FF => {
                if io_enabled
                {
                    byte = as_mut!(self.vic_ref).read_register(addr);
                }
                else
                {
                    byte = as_mut!(self.mem_ref).read_byte(addr);
                }
            },
            // color RAM address space
            0xD800...0xDBFF => {
                if io_enabled
                {
                    byte = (as_ref!(self.mem_ref).read_byte(addr) & 0x0F) | (as_ref!(self.vic_ref).last_byte & 0xF0);
                }
                else
                {
                    byte = as_mut!(self.mem_ref).read_byte(addr);
                }
            },
            // CIA1 address space
            0xDC00...0xDCFF => {
                if io_enabled
                {
                    byte = as_mut!(self.cia1_ref).read_register(addr, &mut read_callback);
                }
                else
                {
                    byte = as_mut!(self.mem_ref).read_byte(addr);
                }
            },
            // CIA2 address space
            0xDD00...0xDDFF => {
                if io_enabled
                {
                    byte = as_mut!(self.cia2_ref).read_register(addr, &mut read_callback);
                }
                else
                {
                    byte = as_mut!(self.mem_ref).read_byte(addr);
                }
            },
            0xDF00...0xDF9F => {
                if io_enabled
                {
                    byte = as_ref!(self.vic_ref).last_byte;
                }
                else
                {
                    byte = as_mut!(self.mem_ref).read_byte(addr);
                }
            },
            0xDFFF => {
                if io_enabled
                {
                    self.dfff_byte = !self.dfff_byte;
                    byte = self.dfff_byte;
                }
                else
                {
                    byte = as_mut!(self.mem_ref).read_byte(addr);
                }
            }, 
            _ => byte = as_mut!(self.mem_ref).read_byte(addr)
        }

        match read_callback
        {
            CallbackAction::TriggerCIAIrq => self.trigger_cia_irq(),
            CallbackAction::ClearCIAIrq   => self.clear_cia_irq(),
            CallbackAction::TriggerNMI    => self.trigger_nmi(),
            CallbackAction::ClearNMI      => self.clear_nmi(),
            _ => (),
        }

        byte
    }

    pub fn read_word_le(&self, addr: u16) -> u16
    {
        as_ref!(self.mem_ref).read_word_le(addr)
    }

    pub fn write_word_le(&self, addr: u16, value: u16) -> bool
    {
        as_ref!(self.mem_ref).write_word_le(addr, value)
    }
    
    fn process_nmi(&mut self) -> bool
    {
        // only process irq if it's the "fetch op" stage
        if self.op_cycles != 0 { return false }
        // 7 cycles
        if self.nmi
        {
            let curr_pc = self.PC;
            let curr_p = self.P;
            self.push_word(curr_pc);
            self.push_byte(curr_p);
            self.set_status_flag(StatusFlag::InterruptDisable, true);
            self.PC = as_ref!(self.mem_ref).read_word_le(NMI_VECTOR);
            self.nmi = false;
            true
        }
        else
        {
            false
        }
    }
    
    fn process_irq(&mut self) -> bool
    {
        // only process irq if it's the "fetch op" stage
        if self.op_cycles != 0 { return false }
        // 7 cycles
        if (self.cia_irq || self.vic_irq) && !self.get_status_flag(StatusFlag::InterruptDisable)
        {
            self.set_status_flag(StatusFlag::Break, false);
            let curr_pc = self.PC;
            let curr_p = self.P;
            //println!("PC {} P {}", curr_pc, curr_p);
            self.push_word(curr_pc);
            self.push_byte(curr_p);
            self.set_status_flag(StatusFlag::InterruptDisable, true);
            self.PC = as_ref!(self.mem_ref).read_word_le(IRQ_VECTOR);
            self.cia_irq = false;
            self.vic_irq = false;
            true
        }
        else
        {
            false
        }
    }

    pub fn trigger_vic_irq(&mut self)
    {
        // TODO:
        //println!("VIC irq triggered");
        self.vic_irq = true;
    }

    pub fn clear_vic_irq(&mut self)
    {
        // TODO
        self.vic_irq = false;
    }

    pub fn trigger_nmi(&mut self)
    {
        // TODO
        //println!("NMI irq");
        self.nmi = true;
    }

    pub fn clear_nmi(&mut self)
    {
        // TODO
        self.nmi = false;
    }

    pub fn trigger_cia_irq(&mut self)
    {
        // TODO
        //println!("CIA irq triggered");
        self.cia_irq = true;
    }

    pub fn clear_cia_irq(&mut self)
    {
        // TODO
        self.cia_irq = false;
    }
    
    fn process_op(&mut self, opcode: u8) -> u8
    {
        //utils::debug_instruction(opcode, self);
      /*  self.prev_PC = self.PC;
        match opcodes::get_instruction(opcode, self)
        {
            Some((instruction, num_cycles, addr_mode)) => {
                //utils::debug_instruction(opcode, Some((&instruction, num_cycles, &addr_mode)), self);
                instruction.run(&addr_mode, self);
                num_cycles
            },
            None => panic!("No instruction - this should never happen! (0x{:02X} at ${:04X})", opcode, self.PC)
    } */
        0
    }

    
    fn fetch_operand(&mut self) -> bool
    {
        match self.curr_instr.addr_mode
        {
            _ => {}
        }

        // fetch complete
        true
    }

    fn run_instruction(&mut self) -> bool
    {
        match self.curr_instr.op
        {
            Op::LDA => {
                if self.ba_low { return false; }
                let na = self.curr_instr.operand_lo;
                self.A = na;
                self.set_zn_flags(na);
            },
            _ => { }
        }

        // instruction finished execution
        true
    }
}
