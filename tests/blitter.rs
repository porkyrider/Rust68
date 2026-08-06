#![cfg(feature = "atari-st")]
//! Unit tests for the Blitter (`rust68::peripherals::atari_st::blitter`).
//!
//! No TomHarte-equivalent test suite exists for this peripheral: these
//! tests validate the internal logic implemented (OP truth table, HOP,
//! endmask, X/Y traversal), and for `skew` specifically, the behavior *as
//! implemented* rather than a verified hardware reference (see the
//! documented limitations in the module).

use rust68::peripherals::atari_st::blitter::{Blitter, reg};
use rust68::{Bus, FlatBus};

// Full word/long write — reflects the real `.W`/`.L` access path used by
// the CPU (see `Blitter::write_word`/`Blitter::write_long`: an isolated
// `.B` access on these registers is ignored on real silicon, so composing
// the value via two/four byte-by-byte `bl.write()` calls would no longer
// have any effect since this change).
fn write_word(bl: &mut Blitter, offset: u32, value: u16) {
    bl.write_word(offset, value);
}

fn write_long(bl: &mut Blitter, offset: u32, value: u32) {
    bl.write_long(offset, value);
}

#[test]
fn registers_16_and_32_bit_round_trip() {
    let mut bl = Blitter::new();
    write_word(&mut bl, reg::SRC_X_INC, 0xFFFE); // -2 as i16
    write_word(&mut bl, reg::X_COUNT, 10);
    write_long(&mut bl, reg::SRC_ADDR, 0x001234);
    write_word(&mut bl, reg::HALFTONE_BASE + 4, 0xABCD); // halftone[2]

    assert_eq!(bl.read(reg::SRC_X_INC), 0xFF);
    assert_eq!(bl.read(reg::SRC_X_INC + 1), 0xFE);
    assert_eq!(
        (bl.read(reg::X_COUNT) as u16) << 8 | bl.read(reg::X_COUNT + 1) as u16,
        10
    );
    assert_eq!(
        (bl.read(reg::SRC_ADDR3) as u32)
            | ((bl.read(reg::SRC_ADDR2) as u32) << 8)
            | ((bl.read(reg::SRC_ADDR1) as u32) << 16)
            | ((bl.read(reg::SRC_ADDR) as u32) << 24),
        0x001234
    );
    assert_eq!(
        (bl.read(reg::HALFTONE_BASE + 4) as u16) << 8 | bl.read(reg::HALFTONE_BASE + 5) as u16,
        0xABCD
    );
}

#[test]
fn hop_zero_ignores_source_and_halftone_when_op_always_one() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0);
    bl.write(reg::OP, 0x0F); // OP = always 1, to isolate the effect of HOP
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF); // source: all 1s
    bus.write16(0x2000, 0x0000); // dest: all 0s
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    // OP=0xF -> output always 1 regardless of s/d: the result must
    // therefore be 0xFFFF independently of HOP.
    assert_eq!(bus.read16(0x2000), 0xFFFF);
}

#[test]
fn hop_zero_means_all_ones_not_zero() {
    // According to the BLITTER.TXT datasheet (info-coach.fr) and
    // BLIT_FAQ.TXT (ggnkua/Atari_ST_Sources): the HOP table is 0=all ones,
    // 1=halftone, 2=source, 3=source AND halftone — HOP=0 therefore does
    // NOT set the result to zero. We use OP=0xC (copy of hop_result) to
    // directly observe the effect of HOP.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0);
    bl.write(reg::OP, 0x3); // copy hop_result to the destination
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x0000); // source: all 0s (must have no effect)
    bus.write16(0x2000, 0x5555); // dest: doesn't matter, replaced by hop_result
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "HOP=0 -> all bits set to 1");
}

/// OP truth table verified by directly solving the system of equations
/// laid out by the official Blitter manual (User Manual for the Atari ST
/// Bit-Block Transfer Processor, archive.org — cross-checked with
/// BLITTER.TXT, same values): OP=1 "source AND destination", OP=2 "source
/// AND NOT destination", OP=4 "NOT source AND destination", OP=8 "NOT
/// source AND NOT destination" can only be simultaneously satisfied with
/// the inverted index `3 - ((s<<1)|d)` — not the direct index `(s<<1)|d`
/// used by the code before the fix (confirmed bug: this direct index gave
/// e.g. OP=3="NOT source" and OP=0xA="destination unchanged" instead of
/// "source" and "NOT destination").
#[test]
fn op_0x3_is_source_op_0xa_is_not_destination() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only (no halftone)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    // OP=0x3: source, independent of the destination.
    bl.write(reg::OP, 0x3);
    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0b1010_1010_1010_1010);
    bus.write16(0x2000, 0b1111_0000_1111_0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0b1010_1010_1010_1010, "OP=0x3 copies the source as-is");

    // OP=0xA: NOT(destination), independent of the source.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    bl.write(reg::OP, 0xA);
    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF);
    bus.write16(0x2000, 0x1234);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), !0x1234u16, "OP=0xA inverts the destination, independently of the source");
}

/// Checks all 16 OP values at once (one source/dest bit at a time, to read
/// the truth table directly) against the official Blitter manual — locks
/// in the fixed indexing bug (see `apply_op`) for good.
#[test]
fn op_truth_table_complete_sixteen_values() {
    // (rule, table [((0,0)),(0,1),(1,0),(1,1)] for (source,destination))
    let rules: [(u8, [bool; 4]); 16] = [
        (0x0, [false, false, false, false]), // all zeros
        (0x1, [false, false, false, true]),  // source AND destination
        (0x2, [false, false, true, false]),  // source AND NOT destination
        (0x3, [false, false, true, true]),   // source
        (0x4, [false, true, false, false]),  // NOT source AND destination
        (0x5, [false, true, false, true]),   // destination
        (0x6, [false, true, true, false]),   // source XOR destination
        (0x7, [false, true, true, true]),    // source OR destination
        (0x8, [true, false, false, false]),  // NOT source AND NOT destination
        (0x9, [true, false, false, true]),   // NOT(source XOR destination)
        (0xA, [true, false, true, false]),   // NOT destination
        (0xB, [true, false, true, true]),    // source OR NOT destination
        (0xC, [true, true, false, false]),   // NOT source
        (0xD, [true, true, false, true]),    // NOT source OR destination
        (0xE, [true, true, true, false]),    // NOT source OR NOT destination
        (0xF, [true, true, true, true]),     // all ones
    ];

    for (op, table) in rules {
        for (i, &(s, d)) in [(0u16, 0u16), (0, 1), (1, 0), (1, 1)].iter().enumerate() {
            let mut bl = Blitter::new();
            bl.write(reg::HOP, 2); // source only
            bl.write(reg::OP, op);
            write_word(&mut bl, reg::SRC_X_INC, 2);
            write_word(&mut bl, reg::DST_X_INC, 2);
            write_word(&mut bl, reg::X_COUNT, 1);
            write_word(&mut bl, reg::Y_COUNT, 1);
            write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
            write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
            write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
            let mut bus = FlatBus::new();
            bus.write16(0x1000, s * 0xFFFF);
            bus.write16(0x2000, d * 0xFFFF);
            write_long(&mut bl, reg::SRC_ADDR, 0x1000);
            write_long(&mut bl, reg::DST_ADDR, 0x2000);
            bl.execute(&mut bus);
            let expected = if table[i] { 0xFFFFu16 } else { 0x0000 };
            assert_eq!(
                bus.read16(0x2000),
                expected,
                "OP={op:#x} with (source={s},destination={d}) must give {expected:#06x}"
            );
        }
    }
}

#[test]
fn endmask_masks_first_and_last_word_of_each_line() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // replace with source (pure copy)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 3);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0x00FF); // first word: only the low byte passes
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF); // middle word: everything passes
    write_word(&mut bl, reg::ENDMASK_3, 0xFF00); // last word: only the high byte passes

    let mut bus = FlatBus::new();
    for i in 0..3 {
        bus.write16(0x1000 + i * 2, 0xFFFF);
        bus.write16(0x2000 + i * 2, 0x0000);
    }
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x00FF, "first word masked by ENDMASK1");
    assert_eq!(bus.read16(0x2002), 0xFFFF, "middle word masked by ENDMASK2");
    assert_eq!(bus.read16(0x2004), 0xFF00, "last word masked by ENDMASK3");
}

/// According to the official Blitter manual: "In the case of a one word
/// line ENDMASK 1 is used" — ENDMASK_3 is simply IGNORED for a one-word
/// line, not ANDed together with ENDMASK_1 (an earlier fix had assumed
/// the opposite — "the two masks merge" — which silently zeroed out
/// otherwise valid blits as soon as ENDMASK_3 was 0, a very common case in
/// practice observed on small mouse cursor blits).
#[test]
fn single_word_line_uses_endmask1_only_endmask3_ignored() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // replace with source (pure copy)
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFF00); // the only mask that should count
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF); // never used here (no middle word)
    write_word(&mut bl, reg::ENDMASK_3, 0x0000); // must be ignored, not combined

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF);
    bus.write16(0x2000, 0x0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xFF00,
        "ENDMASK_1 alone must apply; ENDMASK_3=0 must not cancel everything"
    );
}

#[test]
fn y_traversal_advances_via_y_increments() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3); // copy
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::SRC_Y_INC, 0); // no Y advance on the source side: rereads the same line
    write_word(&mut bl, reg::DST_Y_INC, 4); // skips a 2-word line on the dest side
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x4242);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x4242, "line 0");
    assert_eq!(bus.read16(0x2004), 0x4242, "line 1, after DST_Y_INC");
}

#[test]
fn halftone_cycles_per_line() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // halftone only
    bl.write(reg::OP, 0x3); // copy of the HOP result
    write_word(&mut bl, reg::HALFTONE_BASE, 0x1111);
    write_word(&mut bl, reg::HALFTONE_BASE + 2, 0x2222);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::SRC_Y_INC, 0);
    write_word(&mut bl, reg::DST_Y_INC, 4);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x1111, "line 0 uses halftone[0]");
    assert_eq!(bus.read16(0x2004), 0x2222, "line 1 uses halftone[1]");
}

/// The official manual documents that X_COUNT/Y_COUNT count down
/// internally during execution and then return to their INITIAL value
/// once the blit is finished — never to zero (0 meaning 65536; a caller
/// chaining blits without rewriting Y_COUNT each time, relying on this
/// documented persistence, would otherwise see a spurious 0 = 65536
/// lines). Confirmed buggy before the fix: the code explicitly reset
/// `y_count` to 0 at the end of execution.
#[test]
fn busy_cleared_and_y_count_returns_to_initial_value_after_execute() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 5);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    bl.write(reg::CONTROL, 1 << 7); // BUSY set "by hand" for the test

    let mut bus = FlatBus::new();
    bl.execute(&mut bus);

    assert!(!bl.busy(), "BUSY must be cleared after execute()");
    // Y_COUNT is a LIVE counter (it really counts down on each line
    // processed, not a frozen copy reread identically) — confirmed by
    // Hatari (`BlitterRegs.y_count--`) and Steem SSE (`Blitter.YCounter--`
    // then `Blitter.YCount=(WORD)Blitter.YCounter`, resynced on every
    // line). Once the blit is fully finished, it therefore reads 0, not
    // its starting value — an earlier version of this test assumed the
    // opposite (frozen register), consistent with the old "all at once"
    // model that never modified the visible register, only a local copy.
    assert_eq!(bl.read(reg::Y_COUNT), 0, "high byte of Y_COUNT after a finished blit");
    assert_eq!(bl.read(reg::Y_COUNT + 1), 0, "Y_COUNT must be 0 once the blit is fully finished");
}

#[test]
fn fxsr_primes_the_buffer_register_before_first_read() {
    // According to the datasheet: FXSR (bit 7 of SKEW) triggers an extra
    // source read right at the start of the line, to prime the "buffer
    // register" used by the skew shift. Without FXSR, this buffer starts
    // at zero.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0x84); // FXSR=1, skew=4
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // FXSR reads at the current SRC_ADDR (0x1000) then advances SRC_ADDR
    // by SRC_X_INC BEFORE the normal read of word 0, which therefore
    // takes place at 0x1000+2=0x1002 (confirmed by Hatari,
    // `Blitter_ProcessWord`: `src_addr += src_x_incr` happens between the
    // FXSR read and the normal read of the first word).
    bus.write16(0x1000, 0x000F); // "previous" word (read by FXSR): low bits = 1111
    bus.write16(0x1002, 0x0000); // current source word (word 0, actually read at SRC_ADDR+SRC_X_INC)
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xF000,
        "FXSR=1: the low 4 bits of the previous word rise to the top of the shifted result"
    );

    // Same test without FXSR: the priming buffer starts at zero, so the
    // previous word in memory no longer has any effect.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x04); // FXSR=0, skew=4
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    let mut bus = FlatBus::new();
    bus.write16(0x0FFE, 0xF000);
    bus.write16(0x1000, 0x0000);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0x0000, "FXSR=0: no priming, initial buffer at zero");
}

#[test]
fn nfsr_single_word_line_rereads_and_combines_word_with_itself() {
    // NFSR (bit 6 of SKEW) on a line of a SINGLE word is a special case
    // documented separately by Hatari (`Blitter_ProcessWord`, comment
    // "Special 'weird' case for x_count=1 and NFSR=1"): the normal source
    // read does take place (unlike a multi-word line, where NFSR skips
    // the last read), BUT the silicon additionally performs an extra
    // shift+reread (reusing the last word read off the bus) both before
    // AND after processing the word. With SKEW=0, this amounts to
    // combining the source word with itself in the 32-bit buffer
    // register, which simply yields that same word as output — verified
    // by an exhaustive differential test comparing our implementation to
    // a direct port of `Blitter_ProcessWord` (`tests/blitter_hatari_diff.rs`,
    // 0 mismatches across 8731 configurations including this one). An
    // earlier version treated NFSR as a simple "reuse the previous word"
    // even for X_COUNT=1, which would have given 0x0000 here (null
    // initial buffer) instead of 0xFFFF.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0x40); // NFSR=1, skew=0
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1); // a single word: it's also the last one
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "NFSR=1, single-word line: read then recombined with itself");
}

#[test]
fn nfsr_single_word_line_with_fxsr_also_rereads_the_normal_word() {
    // Same special case as the previous test, but with FXSR=1 as well:
    // the buffer is first primed by the FXSR read (at SRC_ADDR=0x1000),
    // THEN the "normal" read of word 0 also takes place (at
    // 0x1000+SRC_X_INC=0x1002 — the X_COUNT=1 case does NOT suppress
    // this read, only the extra end-of-word shift+reread reuses the last
    // word read off the bus, here the one at 0x1002, not the one at
    // 0x1000). With SKEW=0, it is therefore the value read at 0x1002
    // that dominates in the output, not the FXSR priming value —
    // verified by the same exhaustive differential test.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0xC0); // FXSR=1, NFSR=1, skew=0
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0xABCD); // read by FXSR (at the current SRC_ADDR)
    bus.write16(0x1002, 0x1111); // read by the normal read of word 0
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0x1111,
        "NFSR=1+FXSR=1, single-word line: the normal read (0x1002) dominates in the output"
    );
}

#[test]
fn smudge_selects_halftone_via_low_bits_of_source() {
    // SMUDGE (bit 5 of CONTROL): the halftone word used for each word
    // comes from the low 4 bits of the shifted source word, not from the
    // current line number — so it can potentially differ for each word
    // within the same line (unlike normal mode).
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // halftone only
    bl.write(reg::OP, 0x3); // copy of the HOP result
    bl.write(reg::CONTROL, 0x20); // SMUDGE=1, line number=0
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 3, 0x3333); // halftone[3]
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 7, 0x7777); // halftone[7]
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 2);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x0003); // low nibble = 3
    bus.write16(0x1002, 0x0007); // low nibble = 7
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x3333, "word 0: source nibble 3 -> halftone[3]");
    assert_eq!(bus.read16(0x2002), 0x7777, "word 1: source nibble 7 -> halftone[7], same line");
}

#[test]
fn halftone_line_number_readable_and_settable_via_control() {
    // The halftone line number is exposed directly by bits 0-3 of
    // CONTROL (readable/writable), not a hidden counter. Its direction of
    // travel follows the sign of DST_Y_INC.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 1); // halftone only
    bl.write(reg::OP, 0x3); // copy
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 5, 0x5555); // halftone[5]
    write_word(&mut bl, reg::HALFTONE_BASE + 2 * 4, 0x4444); // halftone[4]

    bl.write(reg::CONTROL, 5); // preset the line number to 5
    assert_eq!(bl.read(reg::CONTROL) & 0x0F, 5, "line number read back as written");

    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::SRC_Y_INC, 0);
    write_word(&mut bl, reg::DST_Y_INC, 0xFFFC); // -4 as i16
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x5555, "line 0: number preset to 5");
    assert_eq!(
        bus.read16(0x1FFC),
        0x4444,
        "line 1: number decremented to 4 (negative DST_Y_INC), dst = 0x2000-4"
    );
    assert_eq!(
        bl.read(reg::CONTROL) & 0x0F,
        3,
        "final line number = 5-2 after 2 decreasing lines"
    );
}

#[test]
fn skew_zero_does_not_modify_source_word() {
    // skew=0 must always return the current word as-is, regardless of the
    // previous word — this is the part of `skew` we're certain about.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    bus.write16(0x0FFE, 0xAAAA); // word just before the source (must have no effect)
    bus.write16(0x1000, 0x1234);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x1234, "skew=0: source word unchanged");
}

/// Checks the shifted combination against the concrete worked example from
/// BLIT_FAQ.TXT (SKEW=3: "reads out bits 18..3" of a buffer [previous
/// (high, bits 16-31)][current (low, bits 0-15)]) — locks in the bug
/// (word order AND shift direction both reversed) found and fixed this
/// session in `skewed_source`.
#[test]
fn nonzero_skew_combines_previous_and_current_in_correct_order() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0x83); // FXSR set (bit 7) + skew=3

    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // FXSR reads at the current SRC_ADDR (0x1000) then SRC_ADDR advances
    // by SRC_X_INC before the normal read of word 0, which therefore
    // takes place at 0x1000+2=0x1002.
    bus.write16(0x1000, 0x0005); // "previous" word (read by FXSR): low bits = 101
    bus.write16(0x1002, 0x0000); // "current" word (word 0)
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    // Expected: low bits (101) of the previous word raised to the top
    // (shifted by 16-3=13), combined with the high bits (all 0 here) of
    // the current word shifted right by 3 -> 0b101_0000000000000 = 0xA000.
    assert_eq!(bus.read16(0x2000), 0xA000, "shifted combination of previous/current in the correct order");
}

/// Same worked example as the previous test, but with a negative
/// SRC_X_INC (decreasing traversal, "mirror" blit): according to Hatari
/// (`Blitter_SourceShift`/`Blitter_SourceFetch`), the order of the two
/// halves of the 32-bit buffer is REVERSED in this case — the CURRENT
/// (newly read) word occupies the HIGH half and the PREVIOUS word the LOW
/// half (instead of the reverse for an increasing traversal). We
/// therefore place the "101" pattern in the CURRENT word (not the
/// previous one) to obtain the same 0xA000 result — a version that
/// ignored the direction (as before this fix) would give 0x0000 here,
/// since it would look for the pattern in the wrong word.
#[test]
fn nonzero_skew_reverses_combination_order_if_src_x_inc_negative() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0x83); // FXSR set (bit 7) + skew=3

    write_word(&mut bl, reg::SRC_X_INC, 0xFFFE); // -2: decreasing traversal
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // FXSR reads at the current SRC_ADDR (0x1000) then SRC_ADDR advances
    // by SRC_X_INC (-2) before the normal read of word 0, which therefore
    // takes place at 0x1000-2=0x0FFE.
    bus.write16(0x1000, 0x0000); // "previous" word (read by FXSR)
    bus.write16(0x0FFE, 0x0005); // "current" word (word 0): low bits = 101
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(
        bus.read16(0x2000),
        0xA000,
        "decreasing traversal: the pattern must be looked for in the CURRENT word, not the previous one"
    );
}

/// End-of-line address advance for a multi-word/multi-line blit with a
/// nonzero X_INC. On real silicon (confirmed by Hatari, `Blitter_Step`:
/// the pointer advances by X_INC between words, but the LAST word of the
/// line advances by Y_INC INSTEAD of X_INC), Y_INC must therefore already
/// account for the (X_COUNT-1) X_INC steps taken within the line. A
/// previous bug added Y_INC alone to the START-of-line address, losing
/// the contribution of (X_COUNT-1)*X_INC — invisible on single-word blits
/// (mouse cursor) but corrupting every multi-word blit (GEM text/icons),
/// which matches exactly the symptoms observed live by the user.
#[test]
fn end_of_line_advance_accounts_for_x_count_minus_one_times_x_inc() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    bl.write(reg::SKEW, 0x00); // no shift/FXSR/NFSR

    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    // Real (memory) line width = 20 bytes; X_COUNT=3 words traversed in
    // steps of 2 -> Y_INC must be 20 - (3-1)*2 = 16 for the next line to
    // start at the right place.
    write_word(&mut bl, reg::SRC_Y_INC, 16);
    write_word(&mut bl, reg::DST_Y_INC, 16);
    write_word(&mut bl, reg::X_COUNT, 3);
    write_word(&mut bl, reg::Y_COUNT, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);

    let mut bus = FlatBus::new();
    // Line 0 (source at 0x1000, real width 20 bytes)
    bus.write16(0x1000, 0x1111);
    bus.write16(0x1002, 0x2222);
    bus.write16(0x1004, 0x3333);
    // Line 1 (source at 0x1000+20=0x1014)
    bus.write16(0x1014, 0x4444);
    bus.write16(0x1016, 0x5555);
    bus.write16(0x1018, 0x6666);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0x1111, "line 0, word 0");
    assert_eq!(bus.read16(0x2002), 0x2222, "line 0, word 1");
    assert_eq!(bus.read16(0x2004), 0x3333, "line 0, word 2");
    assert_eq!(bus.read16(0x2014), 0x4444, "line 1 (0x2000+20), word 0");
    assert_eq!(bus.read16(0x2016), 0x5555, "line 1, word 1");
    assert_eq!(bus.read16(0x2018), 0x6666, "line 1, word 2");
}

/// X_COUNT=0 written as a full WORD: the official Blitter manual and
/// Hatari document this value as meaning 65536, but three independent
/// attempts at implementing this rule all noticeably worsened the
/// corruption observed in practice (see the comment on
/// `Blitter::write_word`) — reverted to the written value as-is (clamped
/// to a minimum of 1 word by `execute`, not 65536) pending localization of
/// the real root cause upstream.
#[test]
fn x_count_zero_written_as_word_stays_clamped_to_one_word() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0); // all ones, result independent of the source
    bl.write(reg::OP, 0x3); // copy of the HOP result (so 0xFFFF everywhere)
    write_word(&mut bl, reg::X_COUNT, 0);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    assert_eq!(bl.read(reg::X_COUNT), 0);
    assert_eq!(bl.read(reg::X_COUNT1), 0);

    let mut bus = FlatBus::new();
    bus.write16(0x2002, 0x4242); // second word: must not be modified
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "first (and only) word processed");
    assert_eq!(bus.read16(0x2002), 0x4242, "no second word (so not 65536)");
}

/// Writing one or two ISOLATED bytes to X_COUNT: on real silicon, an
/// isolated `.B` access to this register is ignored (see `Blitter::write`)
/// — x_count therefore stays at its previous value (0, never touched).
/// `execute` then clamps x_count to a minimum of 1 (see its comment: this
/// avoids an arithmetic overflow in the end-of-line advance calculation)
/// — so exactly ONE word must be processed, neither zero nor 65536.
#[test]
fn x_count_zero_written_as_isolated_byte_does_not_trigger_conversion() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 0);
    bl.write(reg::OP, 0x3);
    bl.write(reg::X_COUNT, 0x00);
    bl.write(reg::X_COUNT1, 0x00);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    let mut bus = FlatBus::new();
    bus.write16(0x2002, 0x4242); // second word: must not be modified
    bl.execute(&mut bus);

    assert_eq!(bus.read16(0x2000), 0xFFFF, "x_count=0 -> clamped to a single word processed");
    assert_eq!(
        bus.read16(0x2002),
        0x4242,
        "only one word processed: no second word (so not 65536)"
    );
}

/// Reproduces the non-HOG mode restart loop used by TOS (`TAS.B` on
/// CONTROL in a loop until BUSY clears): once the blit is finished,
/// resetting the BUSY bit WITHOUT having rewritten Y_COUNT must NOT
/// retrigger a full execution — the official Blitter manual explicitly
/// documents that "the flag will remain clear... the BLiTTER won't be
/// restarted" as long as Y_COUNT hasn't been explicitly rearmed. A real
/// bug (confirmed by direct comparison with Blitter enabled/disabled on a
/// concrete case) caused the entire blit to re-execute on every restart
/// attempt, writing corrupted content from addresses already advanced by
/// the previous pass.
#[test]
fn control_busy_reset_without_rewriting_y_count_does_not_retrigger() {
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy
    write_word(&mut bl, reg::SRC_X_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 1);
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    let mut bus = FlatBus::new();
    bus.write16(0x1000, 0x1234);
    bl.execute(&mut bus);
    assert_eq!(bus.read16(0x2000), 0x1234, "first blit: normal copy");

    // SRC_ADDR/DST_ADDR have advanced (default Y_INC = 0, so unchanged
    // here, but the point is to check that NO second copy happens): we
    // modify the source to detect a second pass. `bl.write(CONTROL,..)`
    // followed by `bl.execute(..)` reproduces what the board (`AtariSt`)
    // does on a real CONTROL write: it alone triggers `execute()`,
    // `Blitter::write` alone does not.
    bus.write16(0x1000, 0x9999);
    bl.write(reg::CONTROL, 0x80); // TAS.B resets BUSY without rewriting Y_COUNT
    bl.execute(&mut bus);
    assert_eq!(
        bus.read16(0x2000),
        0x1234,
        "resetting BUSY without rearming Y_COUNT must NOT retrigger the blit"
    );
    assert!(!bl.busy(), "BUSY must remain readable as 0, not stay \"stuck\" at 1");

    // Explicit rearm (rewriting Y_COUNT): a new trigger MUST then work
    // normally.
    write_word(&mut bl, reg::Y_COUNT, 1);
    bl.write(reg::CONTROL, 0x80);
    bl.execute(&mut bus);
    assert_eq!(
        bus.read16(0x2000),
        0x9999,
        "after explicitly rearming Y_COUNT, a new blit must execute"
    );
}

#[test]
fn non_hog_slice_counts_real_bus_accesses_not_words() {
    // Distinguishes the current model (`BUS_ACCESSES_PER_SLICE` = 64 real
    // bus accesses) from the old approximation based on words processed:
    // this blit READS the source (HOP=2 "source only", OP=0x3 "copy" —
    // need_src=true), so each word costs 3 bus accesses (source read +
    // destination read for the OP combination + destination write), not 2
    // like the HOP=0 blit used by the slice tests in `tests/atari_st.rs`.
    //
    // The budget is checked BEFORE processing a word, against the total
    // accumulated by the words already processed (not a mid-word stop):
    // after 21 words (63 accesses, < 64), word 22 still starts and brings
    // the total to 66 — the slice therefore stops after 22 words, not 21.
    let mut bl = Blitter::new();
    bl.write(reg::HOP, 2); // source only
    bl.write(reg::OP, 0x3); // copy (source AND destination read/written)
    bl.write(reg::SKEW, 0x00); // no FXSR (no extra bus access to count)

    write_word(&mut bl, reg::SRC_X_INC, 0);
    write_word(&mut bl, reg::SRC_Y_INC, 2);
    write_word(&mut bl, reg::DST_X_INC, 0);
    write_word(&mut bl, reg::DST_Y_INC, 2);
    write_word(&mut bl, reg::X_COUNT, 1);
    write_word(&mut bl, reg::Y_COUNT, 30); // 30 lines of 1 word = 30 words
    write_word(&mut bl, reg::ENDMASK_1, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_2, 0xFFFF);
    write_word(&mut bl, reg::ENDMASK_3, 0xFFFF);
    write_long(&mut bl, reg::SRC_ADDR, 0x1000);
    write_long(&mut bl, reg::DST_ADDR, 0x2000);

    let mut bus = FlatBus::new();
    bl.write(reg::CONTROL, 0x80); // BUSY=1, HOG=0: triggers the first slice

    bl.execute(&mut bus);
    assert_eq!(
        (bl.read(reg::Y_COUNT) as u16) << 8 | bl.read(reg::Y_COUNT + 1) as u16,
        30 - 22,
        "22 words (66 bus accesses) processed by the first slice, not 21 (63) nor a multiple of 64/3 rounded to a word"
    );
    assert!(bl.busy(), "blit not finished: BUSY must remain observable by polling");

    // Resume (typical `TAS.B`: resets BUSY without rewriting Y_COUNT):
    // the remaining 8 words (24 accesses, under the 64 budget) finish the blit.
    bl.write(reg::CONTROL, 0x80);
    bl.execute(&mut bus);
    assert_eq!(bl.read(reg::Y_COUNT), 0, "blit finished after the second slice");
    assert_eq!(bl.read(reg::Y_COUNT + 1), 0);
    assert!(!bl.busy());
}
