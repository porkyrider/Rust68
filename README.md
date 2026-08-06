# Rust68

**Motorola 68000** (68k) series emulator in Rust.

The long-term goal is to cover the entire 68000 family and its variants
(68010, 68020, 68030…). The original **MC68000** core is complete and 100%
compliant with the [TomHarte](https://github.com/SingleStepTests/m68000)
test suite (state AND cycles). A complete **Atari ST/STE** system is built
on top of it (CPU, MFP 68901, GLUE, ACIA×2, IKBD, YM2149, Shifter, WD1772,
Blitter, DMA Sound/Microwire, `.st`/`.stx`/`.msa` floppy disk formats) — a
real, unmodified TOS boots up to the interactive GEM desktop. See
[ETAT.md](ETAT.md) for the detailed, up-to-date status of the project.

## Architecture

- **`Cpu`** — the processor state: registers `D0–D7` / `A0–A7`, `PC`, `SR`
  (system byte + CCR), stack pointers `USP`/`SSP`.
- **`Bus`** — a *trait* that the caller implements for its system. The CPU
  holds no memory: all accesses go through the bus. This allows different
  memory maps (Atari vs Amiga) to be modeled without touching the core.
  Only `read8`/`write8` are mandatory; 16/32-bit accesses are derived in
  **big-endian** (the 68000's native order).
- **`FlatBus`** — a simple `Bus` implementation (16 MB of flat RAM),
  provided for testing and prototyping.
- **Timing** — **per-instruction** cycle counting (Motorola tables).
  Cycle-accurate bus timing will come later.

## Example

```rust
use rust68::{Cpu, Bus, FlatBus};

let mut bus = FlatBus::new();
bus.write32(0x0000, 0x0000_1000); // initial SSP
bus.write32(0x0004, 0x0000_0400); // initial PC
bus.write16(0x0400, 0x4E71);      // NOP

let mut cpu = Cpu::new();
cpu.reset(&mut bus);
cpu.step(&mut bus).unwrap();
assert_eq!(cpu.pc, 0x0402);
```

## Tests

```sh
cargo test                      # 68000 core only
cargo test --features atari-st  # + all Atari ST peripherals
```

Two levels of testing:

1. **Targeted unit tests** (`tests/instructions.rs`) — one observable
   behavior per instruction. A safety net during development.
2. **TomHarte compliance** (`tests/tomharte.rs`) — the
   [SingleStepTests/m68000](https://github.com/SingleStepTests/m68000) suite,
   which provides state-before / state-after vectors for each opcode.

   The files are several hundred MB and are not version-controlled.
   Download them, then:

   ```sh
   TOMHARTE_DIR=/path/to/the/json cargo test --test tomharte -- --nocapture
   ```

   Without `TOMHARTE_DIR`, this test is skipped cleanly. Opcodes not yet
   implemented are counted as "skipped" rather than failures, which makes
   it possible to track coverage progress as implementation proceeds.

## Progress status

The full MC68000 instruction set is implemented (all addressing modes,
all exceptions — bus/address error, IPL interrupts, trace), 100% compliant
on both state and cycle-exactness across the 317,500 TomHarte tests.

The Atari ST system (`atari-st` feature) and its SDL2 frontend
(`sdl2-frontend` feature) are built on this core — see [ETAT.md](ETAT.md)
for the per-component breakdown, code architecture, and known limitations.

## License

[GNU General Public License v3.0 or later](LICENSE) (GPL-3.0-or-later).
Any redistribution, including in a closed-source or commercial project,
must republish the source code of its modifications under the same terms.
