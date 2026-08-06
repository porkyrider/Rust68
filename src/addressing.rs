//! Operand sizes and effective addressing (EA) modes of the 68000, extended
//! to the 68020 subset of the "full" extension word (see
//! [`Cpu::resolve_indexed_full`]).

use crate::bus::Bus;
use crate::cpu::{ADDR_MASK, CpuType, Cpu};

/// Size of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// Byte (8 bits).
    Byte,
    /// Word (16 bits).
    Word,
    /// Longword (32 bits).
    Long,
}

impl Size {
    /// Number of bytes for the size.
    pub fn bytes(self) -> u32 {
        match self {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        }
    }

    /// Decodes the standard 2-bit size field (00=byte, 01=word, 10=long).
    pub fn from_bits(bits: u16) -> Option<Size> {
        match bits & 0b11 {
            0b00 => Some(Size::Byte),
            0b01 => Some(Size::Word),
            0b10 => Some(Size::Long),
            _ => None,
        }
    }

    /// Sign-extends a value of this size into a `u32`.
    pub fn sign_extend(self, value: u32) -> u32 {
        match self {
            Size::Byte => value as u8 as i8 as i32 as u32,
            Size::Word => value as u16 as i16 as i32 as u32,
            Size::Long => value,
        }
    }

    /// Mask keeping only the significant bits of the size.
    pub fn mask(self) -> u32 {
        match self {
            Size::Byte => 0x0000_00FF,
            Size::Word => 0x0000_FFFF,
            Size::Long => 0xFFFF_FFFF,
        }
    }

    /// Sign bit (the most significant bit of the size).
    pub fn msb(self) -> u32 {
        match self {
            Size::Byte => 0x0000_0080,
            Size::Word => 0x0000_8000,
            Size::Long => 0x8000_0000,
        }
    }
}

/// A resolved effective address, ready to be read or written.
///
/// The EA is resolved in one shot (consuming any necessary extension words
/// and applying predecrement / postincrement side effects), then
/// read/written via [`Operand::read`] / [`Operand::write`].
#[derive(Debug, Clone, Copy)]
pub enum Operand {
    /// Data register Dn.
    DataReg(usize),
    /// Address register An.
    AddrReg(usize),
    /// Memory location at the resolved address.
    Memory(u32),
    /// Immediate data already extracted from the instruction stream.
    Immediate(u32),
}

impl Operand {
    /// Reads the operand's value at the given size.
    /// Returns `Err((fault_addr, pc_at_fault))` if a word/long memory access hits an odd address.
    pub fn read(self, cpu: &Cpu, bus: &mut impl Bus, size: Size) -> Result<u32, (u32, u32)> {
        match self {
            Operand::DataReg(r) => Ok(cpu.d[r] & size.mask()),
            Operand::AddrReg(r) => Ok(match size {
                Size::Word => cpu.a[r] as u16 as i16 as i32 as u32 & size.mask(),
                _ => cpu.a[r] & size.mask(),
            }),
            Operand::Memory(addr) => {
                if size != Size::Byte && addr & 1 != 0 {
                    return Err((addr, cpu.ea_frame_pc));
                }
                Ok(read_sized(bus, addr, size))
            }
            Operand::Immediate(v) => Ok(v & size.mask()),
        }
    }

    /// Writes `value` into the operand at the given size.
    /// Returns `Err((fault_addr, pc_at_fault))` if a word/long memory access hits an odd address.
    pub fn write(
        self,
        cpu: &mut Cpu,
        bus: &mut impl Bus,
        size: Size,
        value: u32,
    ) -> Result<(), (u32, u32)> {
        match self {
            Operand::DataReg(r) => {
                let keep = !size.mask();
                cpu.d[r] = (cpu.d[r] & keep) | (value & size.mask());
                Ok(())
            }
            Operand::AddrReg(r) => {
                cpu.a[r] = size.sign_extend(value);
                Ok(())
            }
            Operand::Memory(addr) => {
                if size != Size::Byte && addr & 1 != 0 {
                    // A write performs an extra prefetch before the bus cycle:
                    // the frame PC is advanced by 2 relative to a read.
                    return Err((addr, cpu.ea_frame_pc.wrapping_add(2)));
                }
                write_sized(bus, addr, size, value);
                Ok(())
            }
            Operand::Immediate(_) => panic!("cannot write to an immediate operand"),
        }
    }
}

/// Reads a value of the given size from memory.
fn read_sized(bus: &mut impl Bus, addr: u32, size: Size) -> u32 {
    let addr = addr & ADDR_MASK;
    match size {
        Size::Byte => bus.read8(addr) as u32,
        Size::Word => bus.read16(addr) as u32,
        Size::Long => bus.read32(addr),
    }
}

/// Writes a value of the given size to memory.
fn write_sized(bus: &mut impl Bus, addr: u32, size: Size, value: u32) {
    let addr = addr & ADDR_MASK;
    match size {
        Size::Byte => bus.write8(addr, value as u8),
        Size::Word => bus.write16(addr, value as u16),
        Size::Long => bus.write32(addr, value),
    }
}

/// Extra effective address (EA) calculation cycles, based on addressing
/// mode and size — "OPERAND EFFECTIVE ADDRESS CALCULATION TIMES" table from
/// Yacht.txt (3rdparty/doc/Yacht.txt, lines 127-153 of the STAY repo).
/// This cost is added on top of the instruction's base cost; it does not
/// replace anything.
pub fn ea_extra_cycles(mode: u16, reg: u16, size: Size) -> u32 {
    let long = size == Size::Long;
    match mode {
        0b000 | 0b001 => 0, // Dn, An
        0b010 | 0b011 => {
            if long {
                8
            } else {
                4
            }
        } // (An), (An)+
        0b100 => {
            if long {
                10
            } else {
                6
            }
        } // -(An)
        0b101 => {
            if long {
                12
            } else {
                8
            }
        } // (d16,An)
        0b110 => {
            if long {
                14
            } else {
                10
            }
        } // (d8,An,Xn)
        0b111 => match reg {
            0b000 => {
                if long {
                    12
                } else {
                    8
                }
            } // (xxx).W
            0b001 => {
                if long {
                    16
                } else {
                    12
                }
            } // (xxx).L
            0b010 => {
                if long {
                    12
                } else {
                    8
                }
            } // (d16,PC)
            0b011 => {
                if long {
                    14
                } else {
                    10
                }
            } // (d8,PC,Xn)
            0b100 => {
                if long {
                    8
                } else {
                    4
                }
            } // #imm
            _ => 0,
        },
        _ => 0,
    }
}

impl Cpu {
    /// Resolves an effective address described by the `mode` (3 bits) and
    /// `reg` (3 bits) fields extracted from the instruction, for an
    /// operation of size `size`. Consumes the necessary extension words and
    /// applies predecrement / postincrement.
    ///
    /// Returns `None` for an invalid mode encoding.
    pub fn resolve_ea(
        &mut self,
        bus: &mut impl Bus,
        mode: u16,
        reg: u16,
        size: Size,
    ) -> Option<Operand> {
        self.ea_extra_cycles = ea_extra_cycles(mode, reg, size);
        // Default address-error prefix for an ae_read/ae_write that
        // immediately follows: calibrated against ProcessorTests (see
        // Cpu::fault_prefix). Byte/Word = 4 (opcode fetch); Long = 0 — a
        // long access, split into two word transactions by TimedBus,
        // doesn't "pay" this prefix a second time when the fault occurs on
        // the first word. Observed identical whether it's a plain read
        // (TST/CMP/DIVU...) or the RMW re-read of a `Dn,<ea>`
        // (OR/AND/EOR/ADD/SUB) — only the immediate-to-memory family
        // (ORI/ANDI/SUBI/ADDI/EORI, op_line_0) differs and self-corrects
        // after this call.
        self.fault_prefix = if size == Size::Long { 0 } else { 4 };
        let reg = reg as usize;
        // PC after the opcode fetch (before any extension word for this
        // EA). This is the address-error frame PC for An-based modes:
        // displacements/index relative to An do not advance the frame PC.
        let pc_before_ext = self.pc;
        self.ea_frame_pc = pc_before_ext;
        self.ea_is_pc_relative = false;
        match mode {
            // Dn
            0b000 => Some(Operand::DataReg(reg)),
            // An
            0b001 => Some(Operand::AddrReg(reg)),
            // (An)
            0b010 => Some(Operand::Memory(self.a[reg])),
            // (An)+ : postincrement
            0b011 => {
                let addr = self.a[reg];
                // Address error on a LONG access to an odd address: the
                // postincrement (+4) is NOT committed (both word cycles are
                // aborted). For a WORD access, the postincrement (+2)
                // remains committed. Verified on
                // CMP/ADD/AND/OR/SUB/CLR/NOT/NEG/TST/MOVEtoCCR/MOVEfromSR.
                if size == Size::Long && addr & 1 != 0 {
                    return Some(Operand::Memory(addr));
                }
                // A7 stays word-aligned even for a byte access.
                let step = if reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[reg] = self.a[reg].wrapping_add(step);
                Some(Operand::Memory(addr))
            }
            // -(An) : predecrement
            0b100 => {
                let step = if reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[reg] = self.a[reg].wrapping_sub(step);
                // The predecrement consumes an extra prefetch cycle for
                // word/byte accesses only (not long).
                if size != Size::Long {
                    self.ea_frame_pc = self.ea_frame_pc.wrapping_add(2);
                }
                Some(Operand::Memory(self.a[reg]))
            }
            // (d16,An) : 16-bit signed displacement from An
            0b101 => {
                let disp = self.fetch_word(bus) as i16 as i32;
                let addr = (self.a[reg] as i32).wrapping_add(disp) as u32;
                Some(Operand::Memory(addr))
            }
            // (d8,An,Xn) : indexed addressing with 8-bit displacement (or
            // the 68020 "full" extension word, see `resolve_indexed`).
            0b110 => {
                let addr = self.resolve_indexed(bus, self.a[reg])?;
                Some(Operand::Memory(addr))
            }
            // Modes 0b111 : selection is made on `reg`.
            0b111 => match reg {
                // (xxx).W : short absolute address, sign-extended to 32 bits
                0b000 => {
                    let addr = self.fetch_word(bus) as i16 as i32 as u32;
                    // Absolute modes: the frame PC advances with the read of the address.
                    self.ea_frame_pc = self.pc;
                    Some(Operand::Memory(addr))
                }
                // (xxx).L : long absolute address
                0b001 => {
                    let addr = self.fetch_long(bus);
                    self.ea_frame_pc = self.pc;
                    Some(Operand::Memory(addr))
                }
                // (d16,PC) : PC-relative displacement — program space (FC=2/6)
                0b010 => {
                    let base = self.pc;
                    let disp = self.fetch_word(bus) as i16 as i32;
                    let addr = (base as i32).wrapping_add(disp) as u32;
                    self.ea_is_pc_relative = true;
                    Some(Operand::Memory(addr))
                }
                // (d8,PC,Xn) : PC-relative indexed — program space (FC=2/6)
                0b011 => {
                    let base = self.pc;
                    let addr = self.resolve_indexed(bus, base)?;
                    self.ea_is_pc_relative = true;
                    Some(Operand::Memory(addr))
                }
                // #imm : immediate data
                0b100 => {
                    let value = match size {
                        Size::Byte => self.fetch_word(bus) as u8 as u32,
                        Size::Word => self.fetch_word(bus) as u32,
                        Size::Long => self.fetch_long(bus),
                    };
                    Some(Operand::Immediate(value))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Resolves an indexed `(d8,base,Xn)` mode: an extension word supplies
    /// the index register, its size (W/L), and an 8-bit signed
    /// displacement — the "brief" format, the only one existing on
    /// 68000/68010. On the 68020, if bit 8 of the extension word is set,
    /// delegates to the "full" format ([`Self::resolve_indexed_full`]):
    /// real 68000/68010 silicon never inspects this bit, so the gate is on
    /// `cpu_type`, not just the bit itself — a 68000 program that happened
    /// to produce a word with this bit set would still be decoded in brief
    /// form, as on real hardware.
    ///
    /// Returns `None` if the full extension word designates a memory
    /// indirection (`I/IS != 000`, pre/post-indexed) — not implemented (see
    /// [`Self::resolve_indexed_full`]); the caller (`resolve_ea`) propagates
    /// this `None` like any invalid EA encoding.
    fn resolve_indexed(&mut self, bus: &mut impl Bus, base: u32) -> Option<u32> {
        let ext = self.fetch_word(bus);
        if ext & 0x0100 != 0 && self.cpu_type == CpuType::M68020 {
            return self.resolve_indexed_full(bus, base, ext);
        }
        let is_addr = ext & 0x8000 != 0; // bit 15 : A/D
        let xreg = ((ext >> 12) & 0b111) as usize;
        let long = ext & 0x0800 != 0; // bit 11 : W/L
        let disp = ext as i8 as i32; // 8-bit signed displacement

        let index_full = if is_addr { self.a[xreg] } else { self.d[xreg] };
        let index = if long {
            index_full as i32
        } else {
            index_full as u16 as i16 as i32
        };

        Some((base as i32).wrapping_add(disp).wrapping_add(index) as u32)
    }

    /// Resolves the 68020 "full" extension word (bit 8 set), the subset
    /// without memory indirection (`I/IS == 000` only — see
    /// [`Self::resolve_indexed`]'s doc):
    /// - bits 15/14-12/11 : D/A, Xn, W/L — same as the brief format.
    /// - bits 10-9 : SCALE (x1/x2/x4/x8), applied to the index once
    ///   sign-extended per W/L (`index << scale`, NOT the other way round —
    ///   the 68020 sign-extends first, then scales).
    /// - bit 7 : BS (Base register Suppress) — the base register (`base`,
    ///   passed by the caller: An or PC) is ignored if set.
    /// - bit 6 : IS (Index Suppress) — the index term (both register AND
    ///   scale) is ignored if set.
    /// - bits 5-4 : base displacement size (00 reserved, defensively
    ///   treated as null; 01 null; 10 signed word; 11 long),
    ///   additional extension words consumed via `fetch_word`/`fetch_long`
    ///   (same mechanism as `(d16,An)`/`(xxx).L` elsewhere in this file).
    /// - bits 2-0 : I/IS — memory indirection selection. Only `000` (no
    ///   indirection, direct "full" indexed addressing) is handled here;
    ///   any other value (pre/post-indexed) returns `None` — out of scope
    ///   for now (rare in practice, would need an intermediate memory
    ///   read plus an external displacement, to be added in a later pass).
    fn resolve_indexed_full(&mut self, bus: &mut impl Bus, base: u32, ext: u16) -> Option<u32> {
        if ext & 0b111 != 0 {
            return None; // memory indirection: not implemented
        }
        let is_addr = ext & 0x8000 != 0;
        let xreg = ((ext >> 12) & 0b111) as usize;
        let long = ext & 0x0800 != 0;
        let scale = (ext >> 9) & 0b11;
        let base_suppress = ext & 0x0080 != 0;
        let index_suppress = ext & 0x0040 != 0;
        let bd_size = (ext >> 4) & 0b11;

        let base_disp: i32 = match bd_size {
            0b10 => self.fetch_word(bus) as i16 as i32,
            0b11 => self.fetch_long(bus) as i32,
            // 00 (reserved) and 01 (null): no extra word, null term.
            _ => 0,
        };

        let base_term: i32 = if base_suppress { 0 } else { base as i32 };
        let index_term: i32 = if index_suppress {
            0
        } else {
            let index_full = if is_addr { self.a[xreg] } else { self.d[xreg] };
            let index = if long {
                index_full as i32
            } else {
                index_full as u16 as i16 as i32
            };
            index << scale
        };

        Some(base_term.wrapping_add(base_disp).wrapping_add(index_term) as u32)
    }
}
