//! Targeted unit tests for the 68000 core.
//!
//! These tests validate the observable instruction-by-instruction behavior.
//! They serve as a safety net during development, complementing the
//! TomHarte conformance suite (see `tests/tomharte.rs`).

use rust68::{Bus, Cpu, FlatBus, StepError, ccr};

/// Builds a CPU + bus, sets up a reset vector (SSP=0x2000, PC=0x0400)
/// then loads `words` (16-bit big-endian words) starting at 0x0400.
fn setup(words: &[u16]) -> (Cpu, FlatBus) {
    let mut bus = FlatBus::new();
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
fn reset_loads_ssp_and_pc() {
    let (cpu, _bus) = setup(&[]);
    assert_eq!(cpu.ssp, 0x2000);
    assert_eq!(cpu.sp(), 0x2000);
    assert_eq!(cpu.pc, 0x0400);
    assert!(cpu.supervisor());
}

#[test]
fn nop_advances_the_pc() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0402);
    assert_eq!(cycles, 4);
}

#[test]
fn moveq_loads_signed_constant() {
    // MOVEQ #-1, D3  => 0111 011 0 11111111 = 0x76FF
    let (mut cpu, mut bus) = setup(&[0x76FF]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[3], 0xFFFF_FFFF);
    assert!(cpu.flag(ccr::N));
    assert!(!cpu.flag(ccr::Z));

    // MOVEQ #0, D0 => 0x7000
    let (mut cpu, mut bus) = setup(&[0x7000]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0);
    assert!(cpu.flag(ccr::Z));
    assert!(!cpu.flag(ccr::N));
}

#[test]
fn move_immediate_long_to_data_reg() {
    // MOVE.L #$12345678, D0
    // 00 10 000 000 111 100  (size L, dst Dn reg0, src immediate)
    // = 0x203C, followed by the long value
    let (mut cpu, mut bus) = setup(&[0x203C, 0x1234, 0x5678]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0x1234_5678);
    assert!(!cpu.flag(ccr::Z));
    assert!(!cpu.flag(ccr::N));
}

#[test]
fn movea_does_not_touch_the_flags() {
    // First sets Z via MOVEQ #0,D0 then MOVEA.L #$1000,A1.
    // MOVEA.L: 00 10 001 001 111 100 = 0x227C
    let (mut cpu, mut bus) = setup(&[0x7000, 0x227C, 0x0000, 0x1000]);
    cpu.step(&mut bus).unwrap(); // MOVEQ #0,D0 -> Z=1
    assert!(cpu.flag(ccr::Z));
    cpu.step(&mut bus).unwrap(); // MOVEA.L
    assert_eq!(cpu.a[1], 0x0000_1000);
    assert!(cpu.flag(ccr::Z), "MOVEA must not modify the CCR");
}

#[test]
fn add_data_reg_with_carry_and_overflow() {
    // D0 = 0x7FFFFFFF, ADD.L D0,D0 -> signed overflow, no carry.
    // MOVE.L #$7FFFFFFF,D0 ; ADD.L D0,D0
    // ADD.L D0,D0 (Dn+ea->ea, dst=ea=D0): 1101 000 1 10 000 000 = 0xD180
    let (mut cpu, mut bus) = setup(&[0x203C, 0x7FFF, 0xFFFF, 0xD180]);
    cpu.step(&mut bus).unwrap(); // load D0
    cpu.step(&mut bus).unwrap(); // ADD.L D0,D0
    assert_eq!(cpu.d[0], 0xFFFF_FFFE);
    assert!(cpu.flag(ccr::N));
    assert!(cpu.flag(ccr::V), "0x7FFFFFFF + itself overflows in signed arithmetic");
    assert!(!cpu.flag(ccr::C));
    assert!(!cpu.flag(ccr::X));
}

#[test]
fn add_produces_carry() {
    // D0 = 0xFFFFFFFF ; ADD.L D0,D0 -> carry, X
    let (mut cpu, mut bus) = setup(&[0x203C, 0xFFFF, 0xFFFF, 0xD180]);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0xFFFF_FFFE);
    assert!(cpu.flag(ccr::C));
    assert!(cpu.flag(ccr::X));
    assert!(!cpu.flag(ccr::V), "no signed overflow here");
}

#[test]
fn clr_sets_to_zero_and_sets_z() {
    // MOVEQ #5,D2 ; CLR.L D2
    // CLR.L D2: 0100 0010 10 000 010 = 0x4282
    let (mut cpu, mut bus) = setup(&[0x7405, 0x4282]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[2], 5);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[2], 0);
    assert!(cpu.flag(ccr::Z));
    assert!(!cpu.flag(ccr::N));
}

#[test]
fn lea_loads_address_without_dereferencing() {
    // LEA (d16,PC),A0: 0100 000 111 111 010 = 0x41FA, disp.
    // PC base points at the displacement word (0x0402), disp=0x10 -> 0x0412.
    let (mut cpu, mut bus) = setup(&[0x41FA, 0x0010]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[0], 0x0412);
}

#[test]
fn bra_branches_unconditionally() {
    // BRA.B +4: 0110 0000 00000100 = 0x6004
    // base = PC after opcode = 0x0402; target = 0x0406.
    let (mut cpu, mut bus) = setup(&[0x6004]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0406);
}

#[test]
fn beq_branches_if_zero() {
    // MOVEQ #0,D0 (Z=1) ; BEQ +4
    // BEQ.B: 0110 0111 00000100 = 0x6704 ; base after opcode = 0x0404
    let (mut cpu, mut bus) = setup(&[0x7000, 0x6704]);
    cpu.step(&mut bus).unwrap(); // Z=1
    cpu.step(&mut bus).unwrap(); // BEQ taken
    assert_eq!(cpu.pc, 0x0408);
}

#[test]
fn beq_not_taken_if_not_zero() {
    // MOVEQ #1,D0 (Z=0) ; BEQ +4 -> not taken, PC continues after the opcode.
    let (mut cpu, mut bus) = setup(&[0x7001, 0x6704]);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0404, "BEQ not taken: PC right after the opcode");
}

#[test]
fn bsr_pushes_the_return_address() {
    // BSR.B +4: 0110 0001 00000100 = 0x6104
    let (mut cpu, mut bus) = setup(&[0x6104]);
    let sp_before = cpu.sp();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0406);
    assert_eq!(cpu.sp(), sp_before - 4);
    // The pushed return address points right after the BSR opcode.
    assert_eq!(bus.read32(cpu.sp()), 0x0402);
}

#[test]
fn line_a_triggers_exception() {
    // 0xA000 (Line-A) triggers exception vector 10.
    // Vector 10 is at address 40 (0x28). We install a target address.
    let mut bus = FlatBus::new();
    bus.write32(0x0000, 0x0000_2000); // initial SSP
    bus.write32(0x0004, 0x0000_0400); // initial PC
    bus.write32(0x0028, 0x0000_0800); // vector 10 -> 0x0800
    bus.write16(0x0400, 0xA000); // Line-A
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    let result = cpu.step(&mut bus);
    assert!(result.is_ok(), "Line-A must succeed (exception)");
    // Our convention: cpu.pc = vector_address, cpu.pc+4 = vector_address+4 (TomHarte)
    assert_eq!(cpu.pc, 0x0800, "PC must equal the vector address");
}
