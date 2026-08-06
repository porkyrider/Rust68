//! Unit tests for trace mode (the T bit of the SR, `Cpu::trace_pending` /
//! `Cpu::take_trace_exception`).
//!
//! TomHarte deliberately captures the effect of a single instruction without
//! chaining into the trace (see `tests/tomharte.rs`: a NOP with T=1 on
//! entry has an identical `final.sr`, no frame pushed) — this file is
//! therefore the only safety net for this mechanism, on the same principle
//! as `tests/interrupts.rs`.

use rust68::{Bus, Cpu, FlatBus, sr};

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
fn no_trace_if_t_is_zero() {
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP
    cpu.sr &= !sr::T;
    cpu.step(&mut bus).unwrap();
    assert!(!cpu.trace_pending);
}

#[test]
fn step_does_not_take_the_trace_itself_but_sets_trace_pending() {
    // Reproduces exactly the TomHarte case "NOP with T=1 on entry":
    // the instruction's direct effect (SR, PC, RAM) must be identical to
    // T=0, only trace_pending must signal that the trace is due.
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP
    cpu.sr |= sr::T;
    let sr_before = cpu.sr;
    let sp_before = cpu.sp();

    let cycles = cpu.step(&mut bus).unwrap();

    assert_eq!(cycles, 4, "unchanged cost: the trace is not taken within step()");
    assert_eq!(cpu.pc, 0x0402);
    assert_eq!(cpu.sr, sr_before, "SR unchanged by step() itself");
    assert_eq!(cpu.sp(), sp_before, "no frame pushed by step() itself");
    assert!(cpu.trace_pending);
}

#[test]
fn take_trace_exception_pushes_the_frame_and_clears_t() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr |= sr::T;
    bus.write32(0x0024, 0x0000_0900); // vector 9 (trace) * 4 = 0x24
    let sr_before = cpu.sr;
    cpu.step(&mut bus).unwrap();
    let pc_after_nop = cpu.pc;

    let cycles = cpu.take_trace_exception(&mut bus);

    assert_eq!(cycles, Some(34));
    assert_eq!(cpu.pc, 0x0900, "jump to the vector 9 handler");
    assert_eq!(cpu.sr & sr::T, 0, "T must be cleared when entering the handler");
    // Standard 6-byte frame: SP-6 = saved SR, SP-2 (longword) = return PC.
    assert_eq!(bus.read16(cpu.sp()), sr_before);
    assert_eq!(bus.read32(cpu.sp().wrapping_add(2)), pc_after_nop);
    assert!(!cpu.trace_pending, "consumed by take_trace_exception");
}

#[test]
fn take_trace_exception_does_nothing_if_nothing_is_pending() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr &= !sr::T;
    cpu.step(&mut bus).unwrap();
    let pc_before = cpu.pc;
    assert_eq!(cpu.take_trace_exception(&mut bus), None);
    assert_eq!(cpu.pc, pc_before, "no effect if no trace is due");
}

#[test]
fn an_instruction_that_sets_t_itself_only_traces_after_the_next_one() {
    // ORI #$8000,SR (0x007C + immediate) then a NOP. Empirically verified
    // against Hatari (2026-08-04, `Rick_Dangerous.stx`: a TOS routine that
    // sets T via ORI to SR to drive an instruction-by-instruction
    // decryption loop): real silicon STILL executes the following
    // instruction before the trace becomes effective — the same hardware
    // mechanism as the IPL mask delay (see
    // `Cpu::sr_write_pending_delay`). The old assumption ("right at the end
    // of THIS instruction") had never been verified against a reference
    // and caused a double bus fault that Hatari never has on this game.
    let (mut cpu, mut bus) = setup(&[0x007C, 0x8000, 0x4E71]);
    cpu.sr &= !sr::T;

    cpu.step(&mut bus).unwrap(); // ORI #$8000,SR
    assert!(cpu.sr & sr::T != 0, "T must be set by ORI to SR");
    assert!(
        !cpu.trace_pending,
        "not yet: it is THIS instruction that just set T"
    );

    cpu.step(&mut bus).unwrap(); // NOP
    assert!(
        cpu.trace_pending,
        "the trace triggers after the instruction FOLLOWING the one that set T"
    );
}

#[test]
fn trap_does_not_set_trace_pending_even_with_t_initially_set() {
    // take_exception (called by TRAP) clears T when entering its own
    // frame: trace_pending must therefore NOT be set in addition.
    let (mut cpu, mut bus) = setup(&[0x4E40]); // TRAP #0
    bus.write32(0x0080, 0x0000_0900); // vector 32 (TRAP #0) * 4 = 0x80
    cpu.sr |= sr::T;

    cpu.step(&mut bus).unwrap();

    assert_eq!(cpu.pc, 0x0900, "TRAP did jump to its own handler");
    assert_eq!(cpu.sr & sr::T, 0, "T cleared by TRAP's exception entry");
    assert!(
        !cpu.trace_pending,
        "no additional trace: TRAP's exception already handled T"
    );
}

#[test]
fn rte_popping_an_sr_with_t_set_only_triggers_the_trace_after_the_next_instruction() {
    let (mut cpu, mut bus) = setup(&[0x4E73]); // RTE
    bus.write16(0x0800, 0x4E71); // NOP at the return address
    // Push a return frame: SR (with T=1, supervisor) then PC=0x0800.
    let sp = cpu.sp() - 6;
    cpu.set_sp(sp);
    bus.write16(sp, sr::T | rust68::sr::S);
    bus.write32(sp + 2, 0x0800);

    cpu.step(&mut bus).unwrap(); // RTE

    assert_eq!(cpu.pc, 0x0800);
    assert!(cpu.sr & sr::T != 0);
    assert!(
        !cpu.trace_pending,
        "not yet: it is THIS instruction (RTE) that just restored T=1"
    );

    cpu.step(&mut bus).unwrap(); // NOP
    assert!(
        cpu.trace_pending,
        "the trace triggers after the instruction FOLLOWING the RTE that restored T"
    );
}
