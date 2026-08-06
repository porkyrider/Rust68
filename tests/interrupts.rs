//! Unit tests for the IPL interrupt mechanism (Cpu::take_interrupt).
//!
//! No TomHarte suite covers interrupts: they are events external to the
//! opcode flow, not instructions. This file therefore serves as a dedicated
//! safety net, on the same principle as `tests/instructions.rs`.

use rust68::{Bus, Cpu, FlatBus, sr};

/// Test bus: flat RAM + controllable IPL level and IACK vector.
struct IrqBus {
    inner: FlatBus,
    level: u8,
    vector_override: Option<u8>,
}

impl IrqBus {
    fn new() -> Self {
        IrqBus {
            inner: FlatBus::new(),
            level: 0,
            vector_override: None,
        }
    }
}

impl Bus for IrqBus {
    fn read8(&mut self, addr: u32) -> u8 {
        self.inner.read8(addr)
    }
    fn write8(&mut self, addr: u32, value: u8) {
        self.inner.write8(addr, value)
    }
    fn irq_level(&self) -> u8 {
        self.level
    }
    fn irq_ack(&mut self, level: u8) -> u8 {
        self.vector_override.unwrap_or(24 + level)
    }
}

/// Builds a CPU + bus, sets up a reset vector (SSP=0x2000, PC=0x0400)
/// and a NOP at 0x0400, like `tests/instructions.rs::setup`.
fn setup(words: &[u16]) -> (Cpu, IrqBus) {
    let mut bus = IrqBus::new();
    bus.write32(0x0000, 0x0000_2000);
    bus.write32(0x0004, 0x0000_0400);
    let mut addr = 0x0400;
    for &w in words {
        bus.write16(addr, w);
        addr += 2;
    }
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    (cpu, bus)
}

#[test]
fn interrupt_masked_if_level_insufficient() {
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP
    cpu.sr &= !sr::IPL_MASK; // mask = 0, but we request level... 0 too
    bus.level = 0; // no request
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cycles, 4, "normal NOP, no interrupt at level 0");
    assert_eq!(cpu.pc, 0x0402);

    // Mask at 3, request at level 2 (lower or equal): not taken.
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr = (cpu.sr & !sr::IPL_MASK) | (3 << 8);
    bus.level = 2;
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cycles, 4, "level <= current mask: no interrupt");
    assert_eq!(cpu.pc, 0x0402);
}

#[test]
fn autovector_interrupt_taken_and_mask_raised() {
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP never executed
    cpu.sr &= !sr::IPL_MASK; // mask = 0
    bus.write32(0x0068, 0x0000_0800); // level-2 autovector = 24+2=26, address 26*4=0x68
    bus.level = 2;
    let pc_before = cpu.pc;

    let cycles = cpu.step(&mut bus).unwrap();

    assert_eq!(cycles, 44);
    assert_eq!(cpu.pc, 0x0800, "jump to the vector 26 handler");
    assert_eq!(
        (cpu.sr & sr::IPL_MASK) >> 8,
        2,
        "the IPL mask must be raised to the accepted level"
    );
    // Standard 6-byte frame: SP-6 = saved SR, SP-2 (longword) = return PC.
    assert_eq!(bus.read32(cpu.sp().wrapping_add(2)), pc_before);
}

#[test]
fn interrupt_at_same_level_as_mask_not_retaken() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr = (cpu.sr & !sr::IPL_MASK) | (2 << 8); // mask already at 2
    bus.level = 2; // same level: must not retrigger
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cycles, 4, "level == current mask: no interrupt");
    assert_eq!(cpu.pc, 0x0402);
}

#[test]
fn level7_interrupt_always_taken() {
    // reset() sets the IPL mask to 7 by default: only level 7 (NMI)
    // must still be able to trigger.
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    assert_eq!((cpu.sr & sr::IPL_MASK) >> 8, 7);
    bus.write32(0x007C, 0x0000_0900); // vector 31 (24+7), address 31*4=0x7C
    bus.level = 7;

    let cycles = cpu.step(&mut bus).unwrap();

    assert_eq!(cycles, 44);
    assert_eq!(cpu.pc, 0x0900);
    assert_eq!((cpu.sr & sr::IPL_MASK) >> 8, 7);
}

#[test]
fn vectored_interrupt_uses_the_vector_supplied_by_the_peripheral() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr &= !sr::IPL_MASK;
    bus.vector_override = Some(0x40); // e.g. the MFP supplies its own vector
    bus.write32(0x0100, 0x0000_0A00); // 0x40 * 4 = 0x100
    bus.level = 5;

    cpu.step(&mut bus).unwrap();

    assert_eq!(cpu.pc, 0x0A00);
}

#[test]
fn interrupt_switches_user_to_supervisor() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.usp = 0x3000;
    cpu.set_supervisor(false);
    assert!(!cpu.supervisor());
    assert_eq!(cpu.sp(), 0x3000);

    cpu.sr &= !sr::IPL_MASK;
    bus.write32(0x0068, 0x0000_0800); // vector 26
    bus.level = 2;

    cpu.step(&mut bus).unwrap();

    assert!(cpu.supervisor(), "the interrupt must switch to supervisor");
    assert_eq!(cpu.usp, 0x3000, "the user USP must be preserved");
    assert_eq!(cpu.sp(), 0x2000 - 6, "the frame is stacked on the supervisor stack");
}
