#![cfg(feature = "atari-st")]
//! Checks the double bus fault (`Cpu::halted`): a bus/address error
//! occurring WHILE stacking the frame of a previous bus/address error
//! (or reading its vector) must HALT the CPU immediately, without
//! bouncing — the behavior of a real 68000, and verified identical on Hatari
//! (`src/cpu/newcpu.c`, `Exception()`:
//! `if ((m68k_areg(regs,7) & 1) || exception_in_exception < 0)
//! cpu_halt(CPU_HALT_DOUBLE_FAULT);`, immediate halt, never a new
//! attempt). A CLK-style bounce (github.com/TomHarte/
//! CLK) was considered at one point on the strength of its source code, but CLK
//! never actually implemented this case (a TODO acknowledged by its author) — it is not an
//! authoritative reference here, unlike Hatari.

use rust68::{Bus, Cpu, CpuType, sr};

/// Flat RAM bus with a configurable "hole": any read/write within
/// `hole` fails (bus error) instead of succeeding silently.
struct HoleBus {
    mem: Vec<u8>,
    hole: std::ops::Range<u32>,
    fault: Option<(u32, bool)>,
}

impl HoleBus {
    fn new(size: usize, hole: std::ops::Range<u32>) -> Self {
        HoleBus {
            mem: vec![0; size],
            hole,
            fault: None,
        }
    }
}

impl Bus for HoleBus {
    fn read8(&mut self, addr: u32) -> u8 {
        if self.hole.contains(&addr) {
            self.fault = Some((addr, false));
            return 0;
        }
        self.mem[(addr as usize) % self.mem.len()]
    }
    fn write8(&mut self, addr: u32, value: u8) {
        if self.hole.contains(&addr) {
            self.fault = Some((addr, true));
            return;
        }
        let len = self.mem.len();
        self.mem[(addr as usize) % len] = value;
    }
    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.fault.take()
    }
}

#[test]
fn address_error_while_stacking_halts_immediately() {
    // Initial stack at 0x0100, in a hole that covers everything below
    // it: the first stacking (14 bytes) is bound to fail.
    let hole = 0x0000..0x0100;
    let mut bus = HoleBus::new(0x1000, hole);

    let mut cpu = Cpu::new();
    cpu.cpu_type = CpuType::M68000;
    cpu.sr = sr::S; // supervisor
    cpu.a[7] = 0x0100;
    cpu.pc = 0x0400;

    // Odd address => address error (vector 3), triggered "by hand"
    // as the decoder would on a misaligned access.
    cpu.take_address_error(&mut bus, 0x1235, true);

    assert!(
        cpu.halted,
        "a bus/address error while stacking the frame must halt immediately (no bounce)"
    );
}

#[test]
fn address_error_with_valid_stack_does_not_halt() {
    // Valid initial stack (no hole): both the stacking and the vector
    // read succeed, no halt.
    let mut bus = HoleBus::new(0x1000, 0x0000..0x0000);
    bus.write32(0x0000_000C, 0x0000_0800); // vector 3 (address error)

    let mut cpu = Cpu::new();
    cpu.cpu_type = CpuType::M68000;
    cpu.sr = sr::S;
    cpu.a[7] = 0x0100;
    cpu.pc = 0x0400;

    cpu.take_address_error(&mut bus, 0x1235, true);

    assert!(!cpu.halted, "an isolated, non-nested exception must never halt");
    assert_eq!(cpu.pc, 0x0800);
    assert_eq!(cpu.a[7], 0x0100 - 14);
}

#[test]
fn cpu68010_same_scenario_also_halts() {
    // 68010+ "long format" group 0 frame not implemented (see the assert in
    // `take_group0_exception`): `take_exception` keeps the immediate hardware
    // halt in this case rather than escalating to it and panicking.
    let hole = 0x00F0..0x0100;
    let mut bus = HoleBus::new(0x1000, hole);

    let mut cpu = Cpu::new();
    cpu.cpu_type = CpuType::M68010;
    cpu.sr = sr::S;
    cpu.a[7] = 0x0100;
    cpu.pc = 0x0400;

    // take_exception (short frame, vector 5 = division by zero e.g.)
    // with a stack that fails on the very first stacking.
    cpu.take_exception(&mut bus, 5, 0x0400);

    assert!(cpu.halted, "68010+: immediate hardware halt, same as on 68000");
}
