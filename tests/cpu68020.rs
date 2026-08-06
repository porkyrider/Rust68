//! Targeted unit tests for the 68020 subset (`CpuType::M68020`).
//!
//! As with `tests/cpu68010.rs`, no SingleStepTests vectors exist for the
//! 68020 (verified against github.com/SingleStepTests/680x0: only
//! `68000/` is published there) — tests written by hand. The addressing
//! part (the "full" extension word, see `Cpu::resolve_indexed_full`) is the
//! riskiest part (no automated conformance safety net, many
//! SCALE/BS/IS/BD SIZE combinations): covered by a systematic matrix
//! rather than 2-3 spot checks.
//!
//! All effective addresses are observed via LEA (no actual memory access
//! needed to verify the address computation itself).

use rust68::{Bus, Cpu, CpuType, FlatBus};

/// Builds a CPU + bus, sets up a reset vector (SSP=0x2000, PC=0x0400)
/// then loads `words` starting at 0x0400 — same convention as
/// `tests/cpu68010.rs`/`tests/instructions.rs`.
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
    cpu.cpu_type = CpuType::M68020;
    (cpu, bus)
}

// --- Addressing: full extension word, LEA (d8,A0,Xn),A1 = 0x43F0 -------
// (aaa=001=A1, opmode=111, mode=110, base An=000=A0 — see this file's doc)

#[test]
fn full_format_scale_x1_x2_x4_x8() {
    // bit11=1(long) bit8=1(full) bits5-4=01(BD null) — only SCALE varies.
    let cases = [(0x0910u16, 1u32), (0x0B10, 2), (0x0D10, 4), (0x0F10, 8)];
    for (ext, expected_shift) in cases {
        let (mut cpu, mut bus) = setup(&[0x43F0, ext]);
        cpu.a[0] = 0x0000_1000;
        cpu.d[0] = 1;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.a[1], 0x0000_1000 + expected_shift, "ext={ext:#06x}");
    }
}

#[test]
fn full_format_base_suppress() {
    // IS=1 (index suppressed) fixed; BD=$0050 (word); BS varies.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0960, 0x0050]); // BS=0
    cpu.a[0] = 0x0000_2000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0000_2050, "BS=0: base included");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x09E0, 0x0050]); // BS=1
    cpu.a[0] = 0x0000_2000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0000_0050, "BS=1: base suppressed, ignored even when non-zero");
}

#[test]
fn full_format_index_suppress() {
    // BS=1 (base suppressed) fixed, BD null; index D0=3 (long, x1); IS varies.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0990]); // IS=0
    cpu.a[0] = 0x9999_0000;
    cpu.d[0] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 3, "IS=0: index included");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x09D0]); // IS=1
    cpu.a[0] = 0x9999_0000;
    cpu.d[0] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0, "IS=1: index suppressed, ignored even when non-zero");
}

#[test]
fn full_format_bd_size_null_word_long() {
    // BS=1 and IS=1 fixed (address = BD alone).
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x01D0]); // BD null
    cpu.a[0] = 0x1234_5678;
    cpu.d[0] = 0xFFFF_FFFF;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0, "BD null");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x01E0, 0xFFF0]); // BD word, negative
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0xFFFF_FFF0, "BD word: sign-extended (-16)");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x01F0, 0x0001, 0x2345]); // BD long
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0001_2345, "BD long: full 32 bits");
}

#[test]
fn full_format_all_terms_nonzero() {
    // BS=0, IS=0, SCALE=x4, BD=$0100 (word) — catches an accumulation/
    // sign error that wouldn't show up if a term were zero.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0D20, 0x0100]);
    cpu.a[0] = 0x0001_0000;
    cpu.d[0] = 5;
    cpu.step(&mut bus).unwrap();
    // 0x00010000 + 0x0100 + (5<<2=20=0x14)
    assert_eq!(cpu.a[1], 0x0001_0114);
}

#[test]
fn full_format_index_long_vs_word() {
    // D0 = 0x0001_FFF0: as a word (low bits $FFF0 = -16 signed) vs as a
    // long (0x0001FFF0, positive) — two very different results, catches a
    // mishandled W/L index bit.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0190]); // bit11=0 (word)
    cpu.a[0] = 0;
    cpu.d[0] = 0x0001_FFF0;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0xFFFF_FFF0, "word index: sign-extended from the low 16 bits");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0990]); // bit11=1 (long)
    cpu.a[0] = 0;
    cpu.d[0] = 0x0001_FFF0;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0001_FFF0, "long index: full 32-bit value");
}

#[test]
fn full_format_via_pc_relative() {
    // LEA (d8,PC,Xn),A1 = 0x43FB (mode=111,reg=011) — `resolve_indexed` is
    // shared by both callers; verifies that the full format also works
    // through this second path.
    let (mut cpu, mut bus) = setup(&[0x43FB, 0x0160, 0x0010]); // IS=1, BD=$0010 word
    cpu.step(&mut bus).unwrap();
    // base = PC right after the opcode = 0x0402 (before any extension word).
    assert_eq!(cpu.a[1], 0x0402 + 0x0010);
}

#[test]
fn full_format_memory_indirection_not_supported() {
    // I/IS=001 (bits2-0): memory indirection, out of scope — must not
    // silently compute a wrong address. Internally, `execute` fails
    // explicitly (`StepError::IllegalAddressing`), but `step` does not let
    // it propagate: like real silicon (and verified against Hatari), it
    // dispatches the illegal instruction exception (vector 4) instead of
    // panicking — see the comment at the interception site in
    // `Cpu::step`.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0911]);
    cpu.a[0] = 0x1000;
    bus.write32(0x10, 0x0000_0600); // vector 4
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

// --- EXTB.L ------------------------------------------------------------------

#[test]
fn extb_l_positive_and_negative_byte() {
    let (mut cpu, mut bus) = setup(&[0x49C0]); // EXTB.L D0
    cpu.d[0] = 0x1234_5642; // low byte = 0x42 (positive)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0x0000_0042);

    let (mut cpu, mut bus) = setup(&[0x49C0]);
    cpu.d[0] = 0x1234_56F0; // low byte = 0xF0 (negative, -16)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0xFFFF_FFF0);
}

// --- MULU.L / MULS.L -----------------------------------------------------

#[test]
fn mulu_l_32x32_to_32() {
    let (mut cpu, mut bus) = setup(&[0x4C01, 0x0000]); // MULU.L D1,D0
    cpu.d[0] = 1000;
    cpu.d[1] = 2000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 2_000_000);
}

#[test]
fn muls_l_negative_operand() {
    let (mut cpu, mut bus) = setup(&[0x4C01, 0x0800]); // MULS.L D1,D0 (bit11=1)
    cpu.d[0] = 0xFFFF_FFFB; // -5
    cpu.d[1] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0] as i32, -15);
}

#[test]
fn mulu_l_64_bit_result() {
    // MULU.L D1,D2:D0 (Dh=D2, Dl=D0, 64-bit result)
    let (mut cpu, mut bus) = setup(&[0x4C01, 0x2400]);
    cpu.d[0] = 0x0001_0000;
    cpu.d[1] = 0x0001_0000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[2], 1, "high word");
    assert_eq!(cpu.d[0], 0, "low word");
}

// --- DIVU.L / DIVS.L -----------------------------------------------------

#[test]
fn divu_l_32_over_32_with_remainder() {
    // DIVU.L D1,D2:D0 (Dr=D2, Dq=D0, 32-bit dividend)
    let (mut cpu, mut bus) = setup(&[0x4C41, 0x2000]);
    cpu.d[0] = 100;
    cpu.d[1] = 7;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 14, "quotient");
    assert_eq!(cpu.d[2], 2, "remainder");
}

#[test]
fn divu_l_64_over_32() {
    // DIVU.L D1,D2:D0 with bit10=1 (64-bit dividend, Dr:Dq = D2:D0 = 10)
    let (mut cpu, mut bus) = setup(&[0x4C41, 0x2400]);
    cpu.d[0] = 10;
    cpu.d[2] = 0;
    cpu.d[1] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 3, "quotient");
    assert_eq!(cpu.d[2], 1, "remainder");
}

#[test]
fn divu_l_by_zero_raises_vector_5() {
    let (mut cpu, mut bus) = setup(&[0x4C41, 0x2000]);
    cpu.d[1] = 0; // zero divisor
    bus.write32(0x14, 0x0000_0600); // vector 5
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

// --- 68000/68010 regression ------------------------------------------------

#[test]
fn new_instructions_illegal_on_68000_and_68010() {
    for cpu_type in [CpuType::M68000, CpuType::M68010] {
        for words in [
            [0x49C0u16, 0].as_slice(),           // EXTB.L
            [0x4C01, 0x0000].as_slice(),         // MULU.L
            [0x4C41, 0x2000].as_slice(),         // DIVU.L
        ] {
            let (mut cpu, mut bus) = setup(words);
            cpu.cpu_type = cpu_type;
            bus.write32(0x10, 0x0000_0600); // vector 4
            cpu.step(&mut bus).unwrap();
            assert_eq!(cpu.pc, 0x0600, "cpu_type={cpu_type:?} words={words:?}");
        }
    }
}

#[test]
fn full_extension_word_ignored_on_68010_decodes_as_brief() {
    // On the 68010, bit 8 of the extension word is never inspected: LEA
    // (d8,A0,Xn),A1 with a word that WOULD be a full format under 68020 must
    // be decoded as a brief format (8-bit signed displacement = low byte).
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0910]); // low byte = 0x10
    cpu.cpu_type = CpuType::M68010;
    cpu.a[0] = 0x0000_1000;
    cpu.d[0] = 1; // brief index: added as-is (word, not extended here since =1)
    cpu.step(&mut bus).unwrap();
    // Brief format: addr = A0 + index(D0, signed word) + 8-bit displacement ($10).
    assert_eq!(cpu.a[1], 0x0000_1000 + 1 + 0x10);
}
