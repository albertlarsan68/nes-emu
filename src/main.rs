use std::io::Read;

use bitflags::bitflags;

#[derive(Debug, Clone, Copy)]
pub enum Mirroring {
    Horizontal,
    Vertical,
}

pub trait Mapper {
    fn read(&mut self, address: u16) -> Option<u8>;
    fn write(&mut self, address: u16, data: u8) -> bool;
}

pub trait CpuBusMember {
    fn read(&mut self, address: u16) -> Option<u8>;
    fn write(&mut self, address: u16, data: u8) -> bool;
}

pub struct Mmc1 {
    pages: Vec<[u8; Self::ROM_PAGE_SIZE]>,
}

impl Mmc1 {
    pub const ROM_PAGE_SIZE: usize = 16 * 1024;
}

impl Mapper for Mmc1 {
    fn read(&mut self, address: u16) -> Option<u8> {
        match address {
            0xC000..=0xFFFF => self.pages.last().map(|d| d[address as usize - 0xC000]),
            _ => None,
        }
    }

    fn write(&mut self, _address: u16, _data: u8) -> bool {
        false
    }
}

pub enum MapperEnum {
    Mmc1(Mmc1),
}

impl MapperEnum {
    pub fn read(&mut self, address: u16) -> Option<u8> {
        match self {
            Self::Mmc1(mmc1) => mmc1.read(address),
        }
    }

    pub fn write(&mut self, address: u16, data: u8) -> bool {
        match self {
            Self::Mmc1(mmc1) => mmc1.write(address, data),
        }
    }
}

pub struct Cart {
    mapper: MapperEnum,
}

impl CpuBusMember for Cart {
    fn read(&mut self, address: u16) -> Option<u8> {
        self.mapper.read(address)
    }

    fn write(&mut self, address: u16, data: u8) -> bool {
        self.mapper.write(address, data)
    }
}

pub struct Ram {
    storage: Box<[u8; Self::RAM_SIZE]>,
}

impl Ram {
    const RAM_SIZE: usize = 2 * 1024;
}

impl CpuBusMember for Ram {
    fn read(&mut self, address: u16) -> Option<u8> {
        if address > 0x1FFF {
            return None;
        }
        Some(self.storage[address as usize % Self::RAM_SIZE])
    }

    fn write(&mut self, address: u16, data: u8) -> bool {
        if address > 0x1FFF {
            return false;
        }
        self.storage[address as usize % Self::RAM_SIZE] = data;
        true
    }
}

pub struct CpuMemoryBus {
    last_exchanged_value: u8,
    cart: Cart,
    ram: Ram,
}

impl CpuMemoryBus {
    pub fn read(&mut self, address: u16) -> u8 {
        let data = self.cart.read(address).unwrap_or_else(|| {
            self.ram.read(address).unwrap_or_else(|| {
                eprintln!("[WARNING] Reading byte from open bus at 0x{address:04x}");
                self.last_exchanged_value
            })
        });
        self.last_exchanged_value = data;
        data
    }

    pub fn write(&mut self, address: u16, data: u8) {
        self.last_exchanged_value = data;
        let mut written = false;
        written = self.cart.write(address, data) || written;
        written = self.ram.write(address, data) || written;
        if !written {
            eprintln!("[WARNING] Writing byte to open bus at 0x{address:04x} = 0x{data:02x}",);
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct CpuStatusFlags: u8 {
        const CARRY = 0b0000_0001;
        const ZERO = 0b0000_0010;
        const INTERRUPT_DISABLE = 0b0000_0100;
        const DECIMAL = 0b0000_1000;
        const B_FLAG = 0b0001_0000;
        const IGNORED = 0b0010_0000;
        const OVERFLOW = 0b0100_0000;
        const NEGATIVE = 0b1000_0000;
    }
}

#[derive(Debug)]
pub struct Cpu {
    a_reg: u8,
    x_reg: u8,
    y_reg: u8,
    prog_counter: u16,
    stack_pointer: u8,
    status_flags: CpuStatusFlags,
}

impl Cpu {
    pub fn new(_bus: &mut CpuMemoryBus) -> Self {
        Self {
            a_reg: 0,
            x_reg: 0,
            y_reg: 0,
            prog_counter: 0,
            stack_pointer: 0xFF,
            status_flags: CpuStatusFlags::from_bits_retain(0x34),
        }
    }
    pub fn reset(&mut self, bus: &mut CpuMemoryBus) {
        self.stack_pointer = self.stack_pointer.wrapping_sub(3);
        self.status_flags |= CpuStatusFlags::INTERRUPT_DISABLE;
        let reset_vector = u16::from(bus.read(0xfffd)) << 8 | u16::from(bus.read(0xfffc));
        self.prog_counter = reset_vector;
    }

    #[allow(clippy::too_many_lines)]
    pub fn run_instr(&mut self, bus: &mut CpuMemoryBus) {
        let opcode = self.read_instr_byte(bus);
        match opcode {
            0x08 => {
                bus.read(self.prog_counter);
                self.push_stack(bus, self.status_flags.bits());
                eprintln!("PHP (Implied) => 0b{:08b}", self.status_flags.bits());
            }
            0x8E => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                bus.write(address, self.x_reg);
                eprintln!("STX (Absolute) => 0x{address:04x} = 0x{:02x}", self.x_reg);
            }
            0x8C => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                bus.write(address, self.y_reg);
                eprintln!("STY (Absolute) => 0x{address:04x} = 0x{:02x}", self.y_reg);
            }
            0x8D => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                bus.write(address, self.a_reg);
                eprintln!("STA (Absolute) => 0x{address:04x} = 0x{:02x}", self.a_reg);
            }
            0x68 => {
                bus.read(self.prog_counter);
                self.a_reg = self.pull_stack(bus);
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("PLA (Implied) => 0x{:02x}", self.a_reg);
            }
            0xBA => {
                bus.read(self.prog_counter);
                self.x_reg = self.stack_pointer;
                self.status_flags.set(CpuStatusFlags::ZERO, self.x_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.x_reg & 0b1000_0000 != 0);
                eprintln!("TSX (Implied) => 0x{:02x}", self.x_reg);
            }
            0xAD => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                self.a_reg = bus.read(address);
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("LDA (Absolute) => 0x{address:04x} = 0x{:02x}", self.a_reg);
            }
            0x4C => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                self.prog_counter = address;
                eprintln!("JMP (Absolute) => 0x{address:04x}");
            }
            0xA0 => {
                let value = self.read_instr_byte(bus);
                self.y_reg = value;
                self.status_flags.set(CpuStatusFlags::ZERO, self.y_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("LDY (Immediate) => 0x{:02x}", self.y_reg);
            }
            0xA2 => {
                let value = self.read_instr_byte(bus);
                self.x_reg = value;
                self.status_flags.set(CpuStatusFlags::ZERO, self.x_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.x_reg & 0b1000_0000 != 0);
                eprintln!("LDX (Immediate) => 0x{:02x}", self.x_reg);
            }
            0xA9 => {
                let value = self.read_instr_byte(bus);
                self.a_reg = value;
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("LDA (Immediate) => 0x{:02x}", self.a_reg);
            }
            0x78 => {
                bus.read(self.prog_counter);
                self.status_flags
                    .set(CpuStatusFlags::INTERRUPT_DISABLE, true);
                eprintln!("SEI (Implied)");
            }
            0xD8 => {
                bus.read(self.prog_counter);
                self.status_flags.set(CpuStatusFlags::DECIMAL, false);
                eprintln!("CLD (Implied)");
            }
            0x9A => {
                bus.read(self.prog_counter);
                self.stack_pointer = self.x_reg;
                eprintln!("TXS (Implied)");
            }
            0x20 => {
                let low_addr = self.read_instr_byte(bus);
                bus.read(u16::from(self.stack_pointer) | 0x0100);
                self.push_stack(bus, (self.prog_counter >> 8) as u8);
                self.push_stack(bus, (self.prog_counter & 0xFF) as u8);
                let address = u16::from(low_addr) | u16::from(self.read_instr_byte(bus)) << 8;
                self.prog_counter = address;
                eprintln!("JSR (Absolute) => 0x{address:04x}");
            }
            0x84 => {
                let address = u16::from(self.read_instr_byte(bus));
                bus.write(address, self.y_reg);
                eprintln!("STY (Zero Page) => 0x{address:02x} = 0x{:02x}", self.y_reg);
            }
            0x86 => {
                let address = u16::from(self.read_instr_byte(bus));
                bus.write(address, self.x_reg);
                eprintln!("STX (Zero Page) => 0x{address:02x} = 0x{:02x}", self.x_reg);
            }
            0x91 => {
                let indirect_address_pointer = self.read_instr_byte(bus);
                let address = u16::from(bus.read(indirect_address_pointer.into()))
                    | u16::from(bus.read(indirect_address_pointer.wrapping_add(1).into())) << 8;
                let address = address.wrapping_add(self.y_reg.into());
                bus.read(address);
                bus.write(address, self.a_reg);
                eprintln!("STA (Indirect,Y) => 0x{indirect_address_pointer:02x} -> 0x{address:04x} = 0x{:02x}", self.a_reg);
            }
            0xC8 => {
                bus.read(self.prog_counter);
                self.y_reg = self.y_reg.wrapping_add(1);
                self.status_flags.set(CpuStatusFlags::ZERO, self.y_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("INY (Implied) => 0x{:02x}", self.y_reg);
            }
            0xE8 => {
                bus.read(self.prog_counter);
                self.x_reg = self.x_reg.wrapping_add(1);
                self.status_flags.set(CpuStatusFlags::ZERO, self.x_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.x_reg & 0b1000_0000 != 0);
                eprintln!("INX (Implied) => 0x{:02x}", self.x_reg);
            }
            0xD0 => {
                let operand = self.read_instr_byte(bus);
                if !(self.status_flags & CpuStatusFlags::ZERO).is_empty() {
                    eprintln!("BNE (Relative) => 0x{operand:02x}, not taken");
                    return;
                }
                let (new_pc, wrapped) = self.prog_counter.overflowing_add(
                    u16::from(operand) & if operand & 0x80 != 0 { 0xFF00 } else { 0x0000 },
                );
                self.prog_counter = new_pc;
                if wrapped {
                    bus.read(new_pc);
                }
                eprintln!("BNE (Relative) => 0x{operand:02x} -> 0x{new_pc:04x}, taken");
            }
            0xE6 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                bus.write(u16::from(address), data);
                let new_data = data.wrapping_add(1);
                bus.write(u16::from(address), new_data);
                self.status_flags.set(CpuStatusFlags::ZERO, new_data == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, new_data & 0b1000_0000 != 0);
                eprintln!("INC (Zero Page) => 0x{address:02x} -> 0x{data:02x} -> 0x{new_data:02x}");
            }
            0xAA => {
                bus.read(self.prog_counter);
                self.x_reg = self.a_reg;
                self.status_flags.set(CpuStatusFlags::ZERO, self.x_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.x_reg & 0b1000_0000 != 0);
                eprintln!("TAX (Implied) => 0x{:02x}", self.x_reg);
            }
            0x95 => {
                let address = self.read_instr_byte(bus);
                bus.read(u16::from(address));
                bus.write(u16::from(address.wrapping_add(self.x_reg)), self.a_reg);
                eprintln!(
                    "STA (Zero Page,X) => 0x{address:02x} -> 0x{:02x} = 0x{:02x}",
                    address.wrapping_add(self.x_reg),
                    self.a_reg
                );
            }
            0xCA => {
                bus.read(self.prog_counter);
                self.x_reg = self.x_reg.wrapping_sub(1);
                self.status_flags.set(CpuStatusFlags::ZERO, self.x_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.x_reg & 0b1000_0000 != 0);
                eprintln!("DEX (Implied) => 0x{:02x}", self.x_reg);
            }
            0x9D => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                bus.read(address.wrapping_add(self.x_reg.into()));
                bus.write(address.wrapping_add(self.x_reg.into()), self.a_reg);
                eprintln!(
                    "STA (Absolute,X) => 0x{address:04x} -> 0x{:04x} = 0x{:02x}",
                    address.wrapping_add(self.x_reg.into()),
                    self.a_reg
                );
            }
            0x60 => {
                bus.read(self.prog_counter);
                let address = self.pull_stack_address(bus);
                self.prog_counter = address;
                self.read_instr_byte(bus);
                eprintln!("RTS (Implied) => 0x{address:04x}");
            }
            0x2c => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                let data = bus.read(address);
                self.status_flags
                    .set(CpuStatusFlags::ZERO, data & self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, data & 0b1000_0000 != 0);
                self.status_flags
                    .set(CpuStatusFlags::OVERFLOW, data & 0b0100_0000 != 0);
                eprintln!(
                    "BIT (Absolute) => 0x{address:04x} -> 0x{data:02x} & 0x{:02x}",
                    self.a_reg
                );
            }
            0x30 => {
                let operand = self.read_instr_byte(bus);
                if (self.status_flags & CpuStatusFlags::NEGATIVE).is_empty() {
                    eprintln!("BMI (Relative) => 0x{operand:02x}, not taken");
                    return;
                }
                let (new_pc, wrapped) = self.prog_counter.overflowing_add(
                    u16::from(operand) & if operand & 0x80 != 0 { 0xFF00 } else { 0x0000 },
                );
                self.prog_counter = new_pc;
                if wrapped {
                    bus.read(new_pc);
                }
                eprintln!("BMI (Relative) => 0x{operand:02x} -> 0x{new_pc:04x}, taken");
            }
            0x88 => {
                bus.read(self.prog_counter);
                self.y_reg = self.y_reg.wrapping_sub(1);
                self.status_flags.set(CpuStatusFlags::ZERO, self.y_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("DEY (Implied) => 0x{:02x}", self.y_reg);
            }
            0x10 => {
                let operand = self.read_instr_byte(bus);
                if !(self.status_flags & CpuStatusFlags::NEGATIVE).is_empty() {
                    eprintln!("BPL (Relative) => 0x{operand:02x}, not taken");
                    return;
                }
                let (new_pc, wrapped) = self.prog_counter.overflowing_add(
                    u16::from(operand) & if operand & 0x80 != 0 { 0xFF00 } else { 0x0000 },
                );
                self.prog_counter = new_pc;
                if wrapped {
                    bus.read(new_pc);
                }
                eprintln!("BPL (Relative) => 0x{operand:02x} -> 0x{new_pc:04x}, taken");
            }
            0x98 => {
                bus.read(self.prog_counter);
                self.a_reg = self.y_reg;
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("TYA (Implied) => 0x{:02x}", self.a_reg);
            }
            0x0D => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                let data = bus.read(address);
                self.a_reg |= data;
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("ORA (Absolute) => 0x{address:04x} = 0x{data:02x}");
            }
            0x85 => {
                let address = u16::from(self.read_instr_byte(bus));
                bus.write(address, self.a_reg);
                eprintln!("STA (Zero Page) => 0x{address:02x} = 0x{:02x}", self.a_reg);
            }
            0x48 => {
                bus.read(self.prog_counter);
                self.push_stack(bus, self.a_reg);
                eprintln!("PHA (Implied) => 0x{:02x}", self.a_reg);
            }
            0xA8 => {
                bus.read(self.prog_counter);
                self.y_reg = self.a_reg;
                self.status_flags.set(CpuStatusFlags::ZERO, self.y_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("TAY (Implied) => 0x{:02x}", self.y_reg);
            }
            0x28 => {
                bus.read(self.prog_counter);
                self.status_flags = CpuStatusFlags::from_bits_truncate(self.pull_stack(bus));
                eprintln!("PLP (Implied) => 0b{:08b}", self.status_flags.bits());
            }
            0xC9 => {
                let operand = self.read_instr_byte(bus);
                self.status_flags
                    .set(CpuStatusFlags::CARRY, self.a_reg >= operand);
                self.status_flags
                    .set(CpuStatusFlags::ZERO, self.a_reg == operand);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("CMP (Immediate) => 0x{operand:02x}");
            }
            0xF0 => {
                let operand = self.read_instr_byte(bus);
                if (self.status_flags & CpuStatusFlags::ZERO).is_empty() {
                    eprintln!("BEQ (Relative) => 0x{operand:02x}, not taken");
                    return;
                }
                let (new_pc, wrapped) = self.prog_counter.overflowing_add(
                    u16::from(operand) & if operand & 0x80 != 0 { 0xFF00 } else { 0x0000 },
                );
                self.prog_counter = new_pc;
                if wrapped {
                    bus.read(new_pc);
                }
                eprintln!("BEQ (Relative) => 0x{operand:02x} -> 0x{new_pc:04x}, taken");
            }
            0x24 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                self.status_flags
                    .set(CpuStatusFlags::ZERO, data & self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, data & 0b1000_0000 != 0);
                self.status_flags
                    .set(CpuStatusFlags::OVERFLOW, data & 0b0100_0000 != 0);
                eprintln!(
                    "BIT (Zero Page) => 0x{address:02x} -> 0x{data:02x} & 0x{:02x}",
                    self.a_reg
                );
            }
            0x45 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                self.a_reg ^= data;
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("EOR (Zero Page) => 0x{address:02x} = 0x{data:02x}");
            }
            0x46 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                bus.write(u16::from(address), data);
                self.status_flags
                    .set(CpuStatusFlags::CARRY, data & 0b0000_0001 != 0);
                let new_data = data >> 1;
                bus.write(u16::from(address), new_data);
                self.status_flags.set(CpuStatusFlags::ZERO, new_data == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, new_data & 0b1000_0000 != 0);
                eprintln!("LSR (Zero Page) => 0x{address:02x} -> 0x{data:02x} -> 0x{new_data:02x}");
            }
            0x66 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                bus.write(u16::from(address), data);
                let new_carry = data & 0b0000_0001 != 0;
                let new_data = data >> 1
                    | if (self.status_flags & CpuStatusFlags::CARRY).is_empty() {
                        0
                    } else {
                        0b1000_0000
                    };
                bus.write(u16::from(address), new_data);
                self.status_flags.set(CpuStatusFlags::CARRY, new_carry);
                self.status_flags.set(CpuStatusFlags::ZERO, new_data == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, new_data & 0b1000_0000 != 0);
                eprintln!("ROR (Zero Page) => 0x{address:02x} -> 0x{data:02x} -> 0x{new_data:02x}");
            }
            0x6A => {
                bus.read(self.prog_counter);
                let data = self.a_reg;
                let new_carry = data & 0b0000_0001 != 0;
                let new_data = data >> 1
                    | if (self.status_flags & CpuStatusFlags::CARRY).is_empty() {
                        0
                    } else {
                        0b1000_0000
                    };
                self.a_reg = new_data;
                self.status_flags.set(CpuStatusFlags::CARRY, new_carry);
                self.status_flags.set(CpuStatusFlags::ZERO, new_data == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, new_data & 0b1000_0000 != 0);
                eprintln!("ROR (Accumulator) => 0x{data:02x} -> 0x{new_data:02x}");
            }
            0x90 => {
                let operand = self.read_instr_byte(bus);
                if (self.status_flags & CpuStatusFlags::CARRY).is_empty() {
                    eprintln!("BCC (Relative) => 0x{operand:02x}, not taken");
                    return;
                }
                let (new_pc, wrapped) = self.prog_counter.overflowing_add(
                    u16::from(operand) & if operand & 0x80 != 0 { 0xFF00 } else { 0x0000 },
                );
                self.prog_counter = new_pc;
                if wrapped {
                    bus.read(new_pc);
                }
                eprintln!("BCC (Relative) => 0x{operand:02x} -> 0x{new_pc:04x}, taken");
            }
            0xA5 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                self.a_reg = data;
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("LDA (Zero Page) => 0x{address:02x} = 0x{data:02x}");
            }
            0x49 => {
                let operand = self.read_instr_byte(bus);
                self.a_reg ^= operand;
                self.status_flags.set(CpuStatusFlags::ZERO, self.a_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.a_reg & 0b1000_0000 != 0);
                eprintln!("EOR (Immediate) => 0x{operand:02x}");
            }
            0xA6 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                self.x_reg = data;
                self.status_flags.set(CpuStatusFlags::ZERO, self.x_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.x_reg & 0b1000_0000 != 0);
                eprintln!("LDX (Zero Page) => 0x{address:02x} = 0x{data:02x}");
            }
            0xAC => {
                let address = u16::from(self.read_instr_byte(bus))
                    | u16::from(self.read_instr_byte(bus)) << 8;
                let data = bus.read(address);
                self.y_reg = data;
                self.status_flags.set(CpuStatusFlags::ZERO, self.y_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("LDY (Absolute) => 0x{address:04x} = 0x{data:02x}");
            }
            0xA4 => {
                let address = self.read_instr_byte(bus);
                let data = bus.read(u16::from(address));
                self.y_reg = data;
                self.status_flags.set(CpuStatusFlags::ZERO, self.y_reg == 0);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("LDY (Zero Page) => 0x{address:02x} = 0x{data:02x}");
            }
            0xC0 => {
                let operand = self.read_instr_byte(bus);
                self.status_flags
                    .set(CpuStatusFlags::CARRY, self.y_reg >= operand);
                self.status_flags
                    .set(CpuStatusFlags::ZERO, self.y_reg == operand);
                self.status_flags
                    .set(CpuStatusFlags::NEGATIVE, self.y_reg & 0b1000_0000 != 0);
                eprintln!("CPY (Immediate) => 0x{operand:02x}");
            }
            _ => todo!("implement opcode 0x{:x}", opcode),
        };
    }

    fn push_stack(&mut self, bus: &mut CpuMemoryBus, data: u8) {
        bus.write(u16::from(self.stack_pointer) | 0x0100, data);
        self.stack_pointer = self.stack_pointer.wrapping_sub(1);
    }

    fn pull_stack(&mut self, bus: &mut CpuMemoryBus) -> u8 {
        bus.read(u16::from(self.stack_pointer) | 0x0100);
        self.stack_pointer = self.stack_pointer.wrapping_add(1);
        bus.read(u16::from(self.stack_pointer) | 0x0100)
    }

    fn pull_stack_address(&mut self, bus: &mut CpuMemoryBus) -> u16 {
        bus.read(u16::from(self.stack_pointer) | 0x0100);
        self.stack_pointer = self.stack_pointer.wrapping_add(1);
        let low = bus.read(u16::from(self.stack_pointer) | 0x0100);
        self.stack_pointer = self.stack_pointer.wrapping_add(1);
        u16::from(bus.read(u16::from(self.stack_pointer) | 0x0100)) << 8 | u16::from(low)
    }

    fn read_instr_byte(&mut self, bus: &mut CpuMemoryBus) -> u8 {
        let data = bus.read(self.prog_counter);
        self.prog_counter = self.prog_counter.wrapping_add(1);
        data
    }
}

fn main() {
    let mut file = std::fs::File::open(std::env::args().nth(1).expect("Not enough arguments"))
        .expect("Unable to open file");
    let mut header_bytes = [0; 16];
    file.read_exact(&mut header_bytes)
        .expect("Error reading header");
    if !(header_bytes[0] == b'N'
        && header_bytes[1] == b'E'
        && header_bytes[2] == b'S'
        && header_bytes[3] == 0x1A)
    {
        panic!("File is not a iNES ROM");
    }
    let prg_rom_size = header_bytes[4] as usize * (16 * 1024);
    #[allow(clippy::no_effect_underscore_binding)]
    let _chr_rom_size = header_bytes[5] as usize * (8 * 1024);
    let _mirroring_type = if header_bytes[6] & 0b0000_0001 != 0 {
        Mirroring::Vertical
    } else {
        Mirroring::Horizontal
    };
    #[allow(clippy::no_effect_underscore_binding)]
    let _has_persistant_memory = header_bytes[6] & 0b0000_0010 != 0;
    let has_trainer = header_bytes[6] & 0b0000_0100 != 0;
    #[allow(clippy::no_effect_underscore_binding)]
    let _provides_four_screen_vram = header_bytes[6] & 0b0000_1000 != 0;
    let mapper_number = header_bytes[6] & 0xf0 >> 4 | header_bytes[7] & 0xf0;
    assert!(
        mapper_number == 1,
        "Mapper number {mapper_number} is not yet supported"
    );
    // dbg!(
    //     prg_rom_size,
    //     chr_rom_size,
    //     mirroring_type,
    //     has_persistant_memory,
    //     has_trainer,
    //     provides_four_screen_vram,
    //     mapper_number,
    // );
    let _trainer = {
        let mut buf = vec![0; if has_trainer { 512 } else { 0 }];
        file.read_exact(&mut buf).expect("Error reading trainer");
        buf
    };
    let prg_rom_data = {
        let mut buf = vec![0; prg_rom_size];
        file.read_exact(&mut buf).expect("Error readung rom data");
        buf
    };
    let mmc = Mmc1 {
        pages: prg_rom_data
            .chunks_exact(16 * 1024)
            .map(|d| d.to_vec().try_into().expect("Shouldn't happen"))
            .collect::<Vec<_>>(),
    };
    let mapper = MapperEnum::Mmc1(mmc);
    let cart = Cart { mapper };
    let ram = Ram {
        storage: Box::new([0; Ram::RAM_SIZE]),
    };
    let mut cpu_mem_bus = CpuMemoryBus {
        last_exchanged_value: 0,
        cart,
        ram,
    };
    let mut cpu = Cpu::new(&mut cpu_mem_bus);
    cpu.reset(&mut cpu_mem_bus);
    loop {
        cpu.run_instr(&mut cpu_mem_bus);
    }
}
