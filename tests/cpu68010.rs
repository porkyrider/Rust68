//! Targeted unit tests for the 68010 subset (`CpuType::M68010`).
//!
//! No SingleStepTests vectors exist for the 68010 (verified against
//! github.com/SingleStepTests/m68000 and 680x0: only the 68000 is published
//! there) — these tests are therefore written by hand, following the same
//! pattern as `tests/instructions.rs`. They cover: VBR (register +
//! relocation of the vector table), the format word of short exception
//! frames, MOVEC (VBR/SFC/DFC/USP), RTD, MOVES, MOVE from CCR, and —
//! importantly — that the default 68000 behavior (`CpuType::M68000`, never
//! touched by these changes) stays unchanged: these same opcodes must
//! still raise the "illegal instruction" exception (vector 4) on 68000,
//! exactly as before this extension.

use rust68::{Bus, Cpu, CpuType, FlatBus};

/// Builds a CPU + bus, sets up a reset vector (SSP=0x2000, PC=0x0400)
/// then loads `words` (16-bit big-endian words) starting at 0x0400 — same
/// convention as `tests/instructions.rs`.
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

// --- 68000 regression: these opcodes remain "illegal instruction" ---------

#[test]
fn movec_raises_illegal_instruction_on_68000() {
    let (mut cpu, mut bus) = setup(&[0x4E7A, 0x0801]); // MOVEC VBR,D0
    bus.write32(0x10, 0x0000_0600); // vector 4
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn rtd_raises_illegal_instruction_on_68000() {
    let (mut cpu, mut bus) = setup(&[0x4E74, 0x0010]); // RTD #16
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn moves_raises_illegal_instruction_on_68000() {
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x8000]); // MOVES.W D0,(A0)
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn move_from_ccr_raises_illegal_instruction_on_68000() {
    let (mut cpu, mut bus) = setup(&[0x42C0]); // MOVE from CCR,D0
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn short_frame_68000_does_not_change_size() {
    // TRAP #0: SR (word) + PC (long word) = 6 bytes, no format word.
    let (mut cpu, mut bus) = setup(&[0x4E40]);
    bus.write32(0x80, 0x0000_2000); // vector 32 (TRAP #0)
    let sp_before = cpu.sp();
    cpu.step(&mut bus).unwrap();
    assert_eq!(sp_before - cpu.sp(), 6, "68000: 6-byte frame, never a format word");
}

// --- MOVEC (68010) ---------------------------------------------------------

#[test]
fn movec_vbr_round_trip() {
    let (mut cpu, mut bus) = setup(&[
        0x4E7B, 0x0801, // MOVEC D0,VBR
        0x4E7A, 0x1801, // MOVEC VBR,D1
    ]);
    cpu.cpu_type = CpuType::M68010;
    cpu.d[0] = 0x0000_1234;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.vbr, 0x0000_1234, "VBR must receive D0's value");
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[1], 0x0000_1234, "D1 must receive VBR's value");
}

#[test]
fn movec_sfc_dfc_masked_to_3_bits() {
    let (mut cpu, mut bus) = setup(&[
        0x4E7B, 0x0000, // MOVEC D0,SFC
        0x4E7B, 0x0001, // MOVEC D0,DFC
    ]);
    cpu.cpu_type = CpuType::M68010;
    cpu.d[0] = 0xFFFF_FFFF;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.sfc, 0b111, "SFC only keeps the 3 usable bits");
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.dfc, 0b111, "DFC only keeps the 3 usable bits");
}

#[test]
fn movec_usp_round_trip() {
    let (mut cpu, mut bus) = setup(&[
        0x4E7B, 0x0800, // MOVEC D0,USP
        0x4E7A, 0x1800, // MOVEC USP,D1
    ]);
    cpu.cpu_type = CpuType::M68010;
    cpu.d[0] = 0x0000_7000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.usp, 0x0000_7000);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[1], 0x0000_7000);
}

#[test]
fn movec_unknown_control_register_raises_illegal_instruction() {
    // Reserved control register selector (neither SFC/DFC/USP/VBR).
    let (mut cpu, mut bus) = setup(&[0x4E7A, 0x0002]);
    cpu.cpu_type = CpuType::M68010;
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

// --- RTD (68010) ------------------------------------------------------------

#[test]
fn rtd_pops_pc_and_adjusts_sp() {
    let (mut cpu, mut bus) = setup(&[0x4E74, 0x0010]); // RTD #16
    cpu.cpu_type = CpuType::M68010;
    // cpu.a[7] == cpu.ssp == 0x2000 after reset (see `setup`).
    bus.write32(cpu.sp(), 0x0000_1234); // pushed return address
    let sp_before = cpu.sp();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0000_1234);
    assert_eq!(cpu.sp(), sp_before + 4 + 16, "pops the PC (4) then adds the displacement (16)");
}

// --- MOVES (68010) -----------------------------------------------------------

#[test]
fn moves_register_to_memory() {
    // MOVES.W D0,(A0)
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x8000]);
    cpu.cpu_type = CpuType::M68010;
    cpu.a[0] = 0x0000_3000;
    cpu.d[0] = 0x0000_ABCD;
    cpu.step(&mut bus).unwrap();
    assert_eq!(bus.read16(0x3000), 0xABCD);
}

#[test]
fn moves_memory_to_register_preserves_the_high_bytes() {
    // MOVES.W (A0),D1 — Dn destination: only the low 16 bits change.
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x1000]);
    cpu.cpu_type = CpuType::M68010;
    cpu.a[0] = 0x0000_3000;
    cpu.d[1] = 0x1111_0000;
    bus.write16(0x3000, 0xBEEF);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[1], 0x1111_BEEF);
}

#[test]
fn moves_raises_privilege_violation_in_user_mode() {
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x8000]);
    cpu.cpu_type = CpuType::M68010;
    cpu.set_supervisor(false);
    bus.write32(0x20, 0x0000_0700); // vector 8 (privilege violation)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0000_0700);
}

// --- MOVE from CCR (68010) ---------------------------------------------------

#[test]
fn move_from_ccr_is_not_privileged() {
    // MOVE from CCR,D0: 0100 0010 11 000 000 = 0x42C0
    let (mut cpu, mut bus) = setup(&[0x42C0]);
    cpu.cpu_type = CpuType::M68010;
    cpu.set_supervisor(false); // not privileged: must succeed in user mode
    cpu.sr = (cpu.sr & 0xFF00) | 0x0015; // X=1,Z=1,C=1 (0b10101)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0] & 0xFFFF, 0x0015);
}

// --- VBR: vector table relocation ---------------------------

#[test]
fn vbr_relocates_the_vector_table() {
    let (mut cpu, mut bus) = setup(&[0x4E40]); // TRAP #0 (vector 32)
    cpu.cpu_type = CpuType::M68010;
    cpu.vbr = 0x0000_1000;
    // Relocated vector 32: vbr + 32*4 = 0x1000 + 0x80 = 0x1080 (NOT 0x80).
    bus.write32(0x0080, 0x0000_9999); // default handler (address 0), must NOT be taken
    bus.write32(0x1080, 0x0000_2000); // real handler, relative to VBR
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0000_2000);
}

#[test]
fn short_frame_68010_adds_a_format_word() {
    let (mut cpu, mut bus) = setup(&[0x4E40]); // TRAP #0
    cpu.cpu_type = CpuType::M68010;
    bus.write32(0x80, 0x0000_2000);
    let sp_before = cpu.sp();
    cpu.step(&mut bus).unwrap();
    let sp_after = cpu.sp();
    assert_eq!(sp_before - sp_after, 8, "68010: 8-byte frame (SR+PC+format)");
    let format_word = bus.read16(sp_after + 6);
    assert_eq!(format_word, (32u16) << 2, "format nibble 0, vector offset = 32");
}
