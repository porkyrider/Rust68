# Rust68 — MC68000 Emulator Status

> Last updated: 2026-08-06

## License

GPL-3.0-or-later (see [LICENSE](LICENSE)). Any redistribution, including
in a closed-source or commercial project, must republish the source code
of its modifications under the same terms.

## Current TomHarte score

| Category | Count |
|-----------|--------|
| **Passed (register/RAM state AND cycles)** | **317,500** |
| Failures | **0** |
| **Total tests** | **317,500** |

**State compliance (registers/RAM): 100%. Cycle-exact compliance: 100%.**

```sh
# Targeted test (~1s)
TOMHARTE_DIR=… FOCUS=MOVE.l cargo test --release --test tomharte -- --nocapture

# Full run with regression detection (~40s)
TOMHARTE_DIR=… cargo test --release --test tomharte -- --nocapture

# Update the baseline after an improvement
TOMHARTE_DIR=… BASELINE=1 cargo test --release --test tomharte -- --nocapture
```

The baseline (`tomharte_baseline.txt`, in .gitignore) is used to detect
regressions and to visualize progress instruction by instruction.

---

## Code architecture

The 68000 core (`cpu.rs`/`addressing.rs`/`execute.rs`/`bus.rs`) does not
depend on any particular system. Peripherals and boards are organized
**one submodule per emulated system** under `peripherals/`/`systems/`,
each behind its own Cargo feature — none enabled by default: `cargo
build`/`cargo test` with no options only compile/pull in the core, you
need `--features atari-st` explicitly for the Atari ST:

```
src/
  cpu.rs          (~685 l.)  — registers, SR, USP/SSP, fetch_word, take_exception,
                                take_bus_error_full, take_interrupt, take_trace_exception,
                                exception_log
  addressing.rs   (~455 l.)  — Operand, Size, resolve_ea (all 68000 addressing modes)
  execute.rs     (~3290 l.)  — decoding and execution of all instructions
  bus.rs          (~320 l.)  — Bus trait + FlatBus (16 MB flat RAM) + TimedBus
                                (DRAM/video wait-states) + take_bus_fault() + irq_level/irq_ack
  peripherals/
    mod.rs                   — `#[cfg(feature = "atari-st")] pub mod atari_st;`
    atari_st/
      mfp.rs      (~670 l.)  — MC68901 MFP (chip alone, see dedicated section)
      glue.rs     (~370 l.)  — GLUE HBL/VBL timing + STE vertical border (see dedicated section)
      acia.rs     (~150 l.)  — MC6850 ACIA (chip alone, see dedicated section)
      ikbd.rs     (~520 l.)  — IKBD HD6301 keyboard/mouse controller (see dedicated section)
      ym2149.rs   (~555 l.)  — YM2149 PSG (chip alone, see dedicated section)
      microwire.rs(~465 l.)  — serial interface to the LMC1992 mixer (STE), bass/treble filter
      dma_sound.rs(~310 l.)  — STE DMA Sound (8-bit PCM playback from RAM)
      drive_sound.rs(~270 l.)— floppy drive mechanical sound effects
      shifter.rs  (~1150 l.) — Shifter video (chip alone, see dedicated section)
      wd1772.rs   (~945 l.)  — WD1772 floppy controller (chip alone, see dedicated section)
      stx.rs      (~610 l.)  — minimal `.stx` (Pasti) floppy disk reader
      msa.rs      (~265 l.)  — `.msa` (Magic Shadow Archiver) floppy disk reader
      blitter.rs  (~880 l.)  — Blitter BitBlt coprocessor (chip alone, see dedicated section)
  systems/
    mod.rs                   — `#[cfg(feature = "atari-st")] pub mod atari_st;`
    atari_st/
      mod.rs     (~1760 l.)  — minimal ST board (RAM/ROM/MFP/GLUE/ACIA/YM2149/Shifter/WD1772/
                                Blitter, see dedicated section)
      model.rs                — ST/STE/Mega ST/Mega STE model lexicon (RAM, Blitter, original
                                TOS — see dedicated section)
  bin/
    atari_st_sdl2.rs         — SDL2 frontend (video/keyboard/mouse/sound), `sdl2-frontend` feature
  lib.rs                     — public exports
tests/
  instructions.rs            — targeted unit tests (CPU)
  interrupts.rs               — tests of the IPL mechanism (Cpu::take_interrupt)
  trace.rs                     — tests of trace mode (Cpu::take_trace_exception)
  mfp.rs                      — MFP 68901 tests (`atari-st` feature)
  glue.rs                      — GLUE tests (HBL/VBL) (`atari-st` feature)
  acia.rs                      — MC6850 ACIA tests (`atari-st` feature)
  ym2149.rs                    — YM2149 PSG tests (`atari-st` feature)
  shifter.rs                   — Shifter video tests (`atari-st` feature)
  wd1772.rs                    — WD1772 floppy controller tests (`atari-st` feature)
  blitter.rs                   — Blitter tests (`atari-st` feature)
  blitter_hatari_diff.rs      — differential oracle (direct port of `Blitter_ProcessWord`)
  atari_st.rs                 — end-to-end Atari ST board tests (`atari-st` feature)
  tomharte.rs                — TomHarte compliance harness (with FOCUS and baseline)
examples/
  rd_menu_repro.rs / rd_menu_ca6a.rs — headless (no SDL2) reproduction of a GEM menu
                                click-drag, with targeted Blitter/CPU tracing — diagnostic
                                infrastructure kept in the tree, not throwaway scripts.
```

### Cargo features

| Feature | Default | Effect |
|---|---|---|
| `atari-st` | disabled | Compiles `peripherals::atari_st` and `systems::atari_st`. Without it (default), only the 68000 core is available. |
| `sdl2-frontend` | disabled | Compiles the `atari_st_sdl2` binary (depends on `atari-st`, enables the `sdl2` dependency). |

---

## Implemented instructions (complete MC68000 set)

### Data transfers
- `MOVE.b/w/l`, `MOVEA.w/l`, `MOVEQ`
- `MOVEM.w/l`, `MOVEP.w/l`
- `LEA`, `PEA`
- `MOVE from/to CCR`, `MOVE from/to SR` *(to SR: privileged)*
- `MOVE from/to USP` *(privileged)*
- `EXT.w/l`, `SWAP`, `EXG`, `CLR`

### Integer arithmetic
- `ADD/ADDA/ADDQ/ADDX`, `SUB/SUBA/SUBQ/SUBX`
- `NEG/NEGX`, `MULU/MULS`, `DIVU/DIVS`

### BCD arithmetic
- `ABCD`, `SBCD`, `NBCD` *(V/N flags undefined on hardware)*

### Logic & bits
- `AND/OR/EOR/NOT`, `ORI/ANDI/EORI to CCR/SR`
- `BTST/BSET/BCLR/BCHG`

### Comparison & test
- `CMP/CMPA/CMPI/CMPM`, `TST`, `CHK`

### Shifts & rotations
- `LSL/LSR`, `ASL/ASR`, `ROL/ROR`, `ROXL/ROXR` (b/w/l, register and memory)

### Branches
- `BRA`, `BSR`, `Bcc`, `DBcc`, `Scc`, `JMP`, `JSR`, `RTS`, `RTR`
- `LINK`, `UNLK`

### CPU control
- `NOP`, `RESET`, `STOP`, `RTE`, `TAS`

### Software exceptions
- `TRAP #0–#15`, `TRAPV`, `ILLEGAL`, Line-A, Line-F
- Privilege violation for all privileged instructions in user mode
- Divide-by-zero (vector 5): PC frame = address of the *next*
  instruction (post-instruction trap, RTE must not re-execute the DIVU/DIVS)

### Address Error (vector 3) & Bus Error (vector 2)
Full implementation of the 14-byte frame for all word/long accesses at an
odd address: read AE, write AE, instruction fetch AE. Correct handling of
all MOVE.l cases (LSW-first write order for `-(An)`, CCR depending on
src/dst mode, pipeline-correct frame_pc), with `fault_prefix` computed
automatically by `resolve_ea` so as not to re-charge the cost already
incurred by the faulting instruction.

Bus error (same frame, vector 2) is triggered via `Bus::take_bus_fault()`
— an optional hook (default: never faults) that the `Bus` implementation
can fill in to signal an access to an area with no chip select at all
(the physical "hole" between installed RAM and ROM on a real ST/STE).
This is the mechanism many programs/demos use to detect installed RAM
(handler at vector $8 + scanning increasing addresses). Checked after the
opcode fetch and after each execution in `Cpu::step`.

**Double bus fault (`Cpu::halted`)**: a bus/address error occurring while
pushing the frame of a previous bus/address error (or while reading its
vector) — typically a stack pointer outside any mapped area — is detected
and permanently halts the CPU (`StepError::DoubleFault`), as a real 68000
would (HALT until a hardware `/RESET`), rather than bouncing indefinitely
on a vector with a corrupted frame. Real bug found while digging into the
Atari ST cold boot (see below): previously, the fault flag set by the
frame push itself remained unconsumed and contaminated the next `step`,
causing a silent infinite cascade.

### IPL interrupts
`Cpu::take_interrupt` (called by `Cpu::step` before fetching each opcode):
recognizes an external interrupt request and takes it if its level exceeds
the current IPL mask of the SR (level 7 = NMI, always taken). Standard
6-byte frame (SR+PC), switch to supervisor mode, IPL mask raised to the
accepted level, acknowledge cycle via two methods of the `Bus` trait:

- `Bus::irq_level() -> u8`: level requested (0 = no request) by the host
  system.
- `Bus::irq_ack(level) -> u8`: vector to use (default: autovector
  24+level).

Approximate cost of 44 cycles (no TomHarte suite covers interrupts).
Tested in `tests/interrupts.rs`. Not handled: wake-up from STOP, proper
edge-detect for level 7.

### Trace (SR T bit)
The 68000 triggers the vector 9 exception after each instruction if the
SR's T bit is set at the end of it (re-read after the instruction, not
before: an instruction that sets T itself — MOVE/ANDI/ORI to SR, RTE
popping an SR with T=1 — therefore traces as soon as it finishes).
TomHarte deliberately captures the effect of a single instruction without
chaining into the trace even when T=1 on entry (the `final.sr`/`final.pc`
of a NOP with T=1 is identical to T=0, no frame pushed): `Cpu::step`
therefore does **not** take the exception itself, it sets
`Cpu::trace_pending` to true — it is up to the caller (a real emulation
loop, not the compliance harness) to call `Cpu::take_trace_exception`
after each `step` if it wants the real effect, before the next `step`
(preserves the silicon's trace-before-interrupt priority). Standard
6-byte frame, cost of 34 cycles. No exception internal to an instruction
(TRAP, CHK, division by zero, ILLEGAL, Line-A/F, privilege violation)
triggers an additional trace: their own `take_exception` already clears T
on entering their frame. Tested in `tests/trace.rs` (7 tests).

### Diagnostic log
`Cpu::exception_log`: circular buffer (`EXCEPTION_LOG_CAP` = 4096) of
exceptions taken (vector, pushed PC, faulting address, write?, handler PC,
cycles). Pure logging with no effect on execution, for external
diagnostic harnesses.

---

## MFP 68901 (`src/peripherals/atari_st/mfp.rs`)

`peripherals::atari_st::mfp::Mfp` models the chip **alone**, independent
of any system wiring: it is up to the board to map `Mfp::read`/`write`
into its `Bus`, wire `Mfp::iack` to `Bus::irq_ack`, and connect
`Mfp::interrupt_requested()` to `Bus::irq_level` (wired to IPL6 on a real
ST/STE).

Covered:
- 24 logical registers (`mfp::reg`): GPIP/AER/DDR, IERA/B, IPRA/B,
  ISRA/B, IMRA/B, VR, TACR/TBCR/TCDCR + 4 timer data registers,
  SCR/UCR/RSR/TSR/UDR (USART).
- 16-channel interrupt controller (`mfp::channel`, table fixed by the
  datasheet): IPR only arms if IER is active, `interrupt_requested()`
  only surfaces if IMR is unmasked, IPR/ISR only clear on a write of 0
  (never on a write of 1). `iack()` computes the vector
  (`VR[7:3] | channel`), clears IPR, then depending on the S bit (bit 3
  of VR, checked against Hatari `mfp.c`, `MFP_ProcessIACK` —
  counter-intuitive meaning: bit SET = SEI/"software end-of-interrupt",
  ISR stays armed until cleared by software; bit 0 = automatic EOI, ISR
  armed THEN cleared in the SAME cycle, never observably set). A VR write
  that transitions the S bit from 1 (SEI) to 0 (auto) clears ISRA/ISRB in
  bulk. **Real bug fixed**: an earlier version armed ISR
  UNCONDITIONALLY (without looking at the S bit), which, combined with
  the nested priority below, indefinitely blocked any lower-priority MFP
  channel as soon as the first auto-EOI happened — reproduced in practice
  as a sound that never stops and a mouse that responds poorly under GEM
  (Desktop > Info).
- **Nested priority between channels** (`highest_priority_pending`): a
  pending+enabled+unmasked channel only requests service if no channel
  of STRICTLY higher priority is already "in service" (ISR) — a
  lower-priority ISR remains preemptable by a higher one, but blocks any
  channel of equal or lower priority until its own acknowledgment.
  Checked against Hatari (`mfp.c`,
  `MFP_InterruptRequest`/`MFP_CheckPendingInterrupts`).
- 4 timers A/B/C/D: delay mode (prescale ÷4/÷10/÷16/÷50/÷64/÷100/÷200)
  and event-count mode (A/B only, driven by `pulse_ta`/`pulse_tb` rather
  than the clock). Reading the data register of a RUNNING timer returns
  the current countdown value (real chip behavior, used by TOS to read a
  clock without stopping it).
- GPIP with per-pin edge detection (AER), on the 8 dedicated channels.
- Simplified "byte-level" USART (`push_rx_byte`/`take_tx_byte`): no
  simulation of start/stop/parity bits nor a real baud rate.

Known limitations, documented in the module — deliberate scope choices,
not bugs:
- `tick()` assumes a fixed 8 MHz CPU clock (ST/STE); the period of a
  delay-mode timer is modeled as `(data+1) × prescale` MFP cycles — not
  verified against an external hardware reference (no test suite
  equivalent to TomHarte exists for the MFP), consistent with
  `model::MachineProfile`, which documents `cpu_hz` as informative only
  for the same reason.
- If several timer periods elapse within a single `tick()` call, the
  channel only arms once — consistent with real silicon (`IPR` is a bit,
  not an occurrence counter).

Tested in `tests/mfp.rs` (15 tests, including preemption/blocking via
nested priority).

---

## GLUE (`src/peripherals/atari_st/glue.rs`)

`peripherals::atari_st::glue::Glue` models the HBL/VBL timing generator
of the GLUE chip (GLUE's memory/bus role is not covered):

- `tick(cpu_cycles)` advances a line counter; at the end of each line, it
  arms HBL (IPL2); at the end of each frame (313 lines in PAL 50 Hz, 263
  in NTSC 60 Hz — `VideoMode`), it arms VBL (IPL4) and increments
  `frame_count()`.
- `ack_hbl`/`ack_vbl` acknowledge (to be called from `Bus::irq_ack`).
- `current_line()`/`frame_count()`/`lines_per_frame()` exposed for
  reading, to synchronize video rendering with the scan position.
- **Numbering aligned with Hatari**: `current_line()` is an ABSOLUTE
  position within the frame (0..`lines_per_frame()`), including a real
  top blanking period before the nominal visible window
  (`VideoMode::frame_start_line()`, 63 in PAL/34 in NTSC —
  Hatari's `VIDEO_START_HBL_50HZ`/`_60HZ`) — not just a bottom blanking
  as in an earlier version. `display_line()` returns the index into the
  framebuffer (`Some(0..200)`) or `None` if the current line is in
  blanking/border.
- **Vertical overscan** (`$FF820A`, `write_sync`/`read_sync`): a 50/60Hz
  switch (bit 1) occurring in the right cycle window
  (`LINE_REMOVE_BORDER_CYCLE`) near the top (close to
  `frame_start_line()`) or the bottom (close to the end of the nominal
  visible window) extends the displayed window of the current frame —
  +29 lines at the top (34 instead of 63), +47 at the bottom in PAL
  (Hatari-style, `Video_Update_Glue_State`). Simplification: once
  triggered for a frame, the extension is never canceled by a later
  write that would revert it (Hatari handles a few finer cancellation
  cases, not modeled here). Horizontal border (left/right, `$FF8260`):
  modeled on the `Shifter` side, not here — see its dedicated section.

Limitations: timing only (not GLUE's memory/bus role); the usual
Hatari/WinSTon constants of 512 cycles/line (PAL) and 508 (NTSC), not
verified against a formal hardware reference.

Tested in `tests/glue.rs` (8 tests) and `tests/atari_st.rs` (end-to-end
vertical overscan).

---

## ACIA 6850 (`src/peripherals/atari_st/acia.rs`)

`peripherals::atari_st::acia::Acia` models a single MC6850 chip; the
Atari ST has **two** of them onboard (keyboard, MIDI), each a separate
instance in `AtariSt` (`acia_keyboard`/`acia_midi`), whose IRQs are
OR-wired onto `GPIP4` of the MFP.

- CONTROL/STATUS and DATA registers, RDRF/TDRE/OVRN/FE flags faithful to
  the MC6850: reading DATA clears RDRF+OVRN together; writing CONTROL
  with bits0-1=`11` triggers a Master Reset (clears the flags, re-arms
  TDRE).
- No receive FIFO (as on real silicon): a byte received while the
  previous one hasn't been read triggers `OVRN` and the new byte is
  **lost**.
- `irq_requested()`: `(RDRF && RIE) || (TDRE && TIE)` — `DCD`/`CTS`
  always 0 (no external handshake simulated), `PE` always 0 (no parity
  simulation).
- `push_rx_byte`/`take_tx_byte`: "byte-level" model.

Tested in `tests/acia.rs` (7 tests).

---

## IKBD (`src/peripherals/atari_st/ikbd.rs`)

`peripherals::atari_st::ikbd::Ikbd` models the HD6301 controller
(keyboard/mouse/joystick), wired to `acia_keyboard` — previously absent:
the commands sent by TOS to configure the IKBD (reset, mouse modes…)
were written to the ACIA but never read/interpreted (`take_tx_byte` was
never called anywhere), and the demo binary handled an ad hoc output
queue by hand (`ikbd_tx_queue`) itself. Port very largely inspired by
the companion project Stay (same findings, same fixes, adapted to the
absence of a joystick here):

- Self-test response `0xF1` **deferred** (`IKBD_RESET_CYCLES`, 5,000,000
  cycles) rather than immediate on reset: delivering it too early makes
  the byte arrive before TOS has finished configuring the MFP's
  IERB/IMRB, with the corresponding ACIA interrupt (pending but still
  masked) then silently cleared by TOS's later normal write to IERB —
  the byte is then never read, RDRF stays permanently full, blocking
  every subsequent byte (keyboard AND mouse) forever.
- `receive_cmd`/`execute_cmd`: reset (`0x80 0x01`), relative mouse mode
  (`0x08`) AND absolute mode (`0x09`, see below), mouse action (`0x07`),
  Y axis (`0x0F`/`0x10`), absolute position query (`0x0D`), direct
  loading of the internal position (`0x0E`) actually implemented;
  joystick commands (not modeled, no gamepad frontend) still consume the
  correct number of parameter bytes so as not to desynchronize the
  following command stream.
- **Absolute mouse mode (`0x09`), real bug fixed**: the command was
  recognized (correct number of parameter bytes consumed) but completely
  ignored — `execute_cmd` had no case for `0x09`, so the mouse stayed in
  relative mode forever and kept emitting `0xF8` packets even after GEM
  switched to absolute mode (e.g. opening a modal dialog such as
  Desktop > Info, which uses `0x09` to bound/calibrate the mouse to its
  own area). Result: desynchronization of the serial stream on the GEM
  side, cursor movement appearing "rotated" (the `dx`/`dy` bytes
  misinterpreted as a packet of a different format) as long as the
  dialog stayed open, plus a repeated beep (GEM's error behavior when
  faced with unexpected data) — reproduced and diagnosed under real
  conditions, confirmed against Hatari (`ikbd.c`,
  `IKBD_Cmd_AbsMouseMode`/`IKBD_SendAutoKeyboardCommands`): in absolute
  mode, real silicon **never** sends an automatic packet on movement —
  only in response to `0x0D`, or on button press/release if `0x07`
  requested it (bits 0-1). `mouse_x`/`mouse_y` are tracked PERMANENTLY
  (regardless of the active mode) and bounded by the `MaxX`/`MaxY` of the
  last `0x09` command received (639/399 by default, before any command)
  — the bounding only applies to the NEXT movement, not retroactively to
  the position already tracked.
- `mouse_move(dx, dy, buttons)`: internal absolute position tracked and
  bounded (see above), standard relative packet `0xF8|buttons, dx, dy`
  emitted only if something changed (relative mode only).
- `AtariSt::tick` wires everything together: draining `take_tx_byte` into
  `Ikbd::receive_cmd`, delivering one byte from `Ikbd::pop_tx` per tick
  (gated by RDRF), and **explicitly forcing a GPIP4 release** before
  re-arming RDRF for the next byte within the same tick — on real
  silicon, the ACIA's `/IRQ` genuinely rises over the serial interval
  between two bytes; without this forced release, the 2nd/3rd byte of
  each frame (e.g. a mouse frame) never produces an edge for
  `Mfp::set_gpip_input`, which is rightly edge-triggered — the exact bug
  already isolated and fixed in Stay.
- The demo binary (`atari_st_sdl2`) samples mouse movement only once per
  video frame rather than on every raw SDL event (potentially much more
  frequent than the real ~50 Hz), and caps each packet at ±15 units
  (real IKBD firmware limit, not just the ±127 of a signed byte field),
  splitting a larger movement across several packets — the same approach
  as Stay's `Machine::flush_input_vbl`/`mouse_move`.
- Additional RAM pre-fill at "warm restart" boot (see next section):
  `$0EE4`/`$0EE5` = `0x11`/`0x11` (IKBD Timer-C gate, normally set by
  TOS itself during its setup — without it, the Timer C handler's
  `ASL.W #3,($0EE4).L; BPL` would take the wrong branch and never drain
  pending IKBD bytes).

Tested in `src/peripherals/atari_st/ikbd.rs` (15 unit tests, including
absolute mode: no automatic packet on movement, MaxX/MaxY bounding,
conditional button reporting via `0x07`, return to relative mode via
`0x08`, direct loading via `0x0E`).

---

## Audio (YM2149, DMA Sound, Microwire/LMC1992)

Complete pipeline, from silicon to SDL2 output, clocked once per output
sample (fixed 44100 Hz, `atari_st_sdl2.rs::mix_sample` + further
processing in `main`): YM2149 (3 channels) + DMA Sound (STE PCM) mixed
into a raw stereo sample, DC removed (a mixing artifact, not a physical
phenomenon), Microwire gain + bass/treble filter applied in one pass,
drive mechanical sound added on top.

### YM2149 (`src/peripherals/atari_st/ym2149.rs`)

`peripherals::atari_st::ym2149::Ym2149` models the chip alone
(register-for-register compatible with the General Instrument
AY-3-8910): 3 square-tone channels, a shared noise generator, an
envelope generator, 2 8-bit I/O ports. Mapped into `AtariSt` (public
field `ym2149`) at selector `0xFF8800` / data `0xFF8802`, clocked by
`AtariSt::tick` (CPU/4 = 2 MHz on ST/STE). Does not generate an IPL (no
interrupt on ST).

- 16 registers with exact masking of their real width (e.g. tone coarse
  4 bits, noise period 5 bits, envelope shape 4 bits).
- Tone generators (3×) and noise generator (17-bit LFSR, standard
  bit0 XOR bit3 polynomial) driven by `tick()`.
- Envelope generator: 5-bit ramp (double the resolution of the fixed
  4-bit amplitude), table of the 10 standard shapes
  (Continue/Attack/Alternate/Hold). Writing the shape register always
  restarts the envelope from the beginning (documented silicon
  behavior).
- `channel_level(channel)`: level 0-31 combining the tone/noise gating
  (`MIXER`) and the fixed amplitude (converted 4->5 bits via
  `VOLUME_4_TO_5`, a MEASURED table taken from Hatari — NOT a simple ×2,
  see below) or the envelope (already 0-31 natively).
- `take_averaged_levels()`: time average of levels since the last call
  (anti-aliasing for high tones — specific to Rust68, neither Hatari nor
  Steem SSE need this because they sample at an internal rate far finer
  than the output rate).
- **Non-linear 3-channel mixing** (`mix_channels_model`, Hatari-style
  `YM2149_BuildModelVolumeTable`/`YM_MODEL_MIXING`, `sound.c`): models
  the real DAC as three adjustable pull-up resistors in parallel on a
  fixed load resistor (voltage divider), NOT a simple sum — combining
  2-3 channels at full amplitude saturates well below 3× a single voice
  on real silicon. Constants (`WARP`, `FOURTH2`) taken as-is from Hatari
  (model attributed to David Savinkoff, based on real measurements by
  Paulo Simoes/Benjamin Gerard); Steem SSE independently confirms the
  phenomenon (its own measured "LJBK" table, comment: "there's some
  interaction between channels on the ST. The sound is very saturated").
  Conductance table built once (32 values), linearly interpolated for
  the FRACTIONAL levels that `take_averaged_levels` returns (Hatari's
  model is natively discrete 0-31, this interpolation is an adaptation,
  not an approximation of the formula itself).
- Ports A/B: raw 8-bit registers, direction set by bits 6-7 of `MIXER`;
  bit meaning (drive select, joystick, Centronics…) not interpreted, left
  to the board/caller.

Tested in `tests/ym2149.rs` (15 tests, including the non-linear
saturation of 3-channel mixing and its monotonicity/interpolation).

### STE DMA Sound (`src/peripherals/atari_st/dma_sound.rs`)

`peripherals::atari_st::dma_sound::DmaSound` reads signed 8-bit PCM
samples from RAM at one of 4 hardware frequencies (6258/12517/25033/50066
Hz), mono or stereo, with end-of-frame loop/stop and conversion to the
host output frequency via a fixed 32.32 accumulator (same technique as
Hatari, `dmaSnd.c`). Mapped into `AtariSt` (`$FF8900`-`$FF8921`), XSINT
wired to the MFP's Timer A on each end of frame.

Documented limitation (see its module doc): no 8-byte hardware FIFO
(read at the HBL rate on real silicon, here at the exact rate of each
sample consumed) — a real but narrow audible effect, only for software
that rewrites the sample buffer WHILE it plays (2 cases named on the
Hatari side: "Mental Hangover", "Power Up Plus" — no known case in this
project). Steem SSE itself does not model this FIFO for STE DMA sound
either. To be reconsidered if a concrete case reveals a problem.

Tested in `src/peripherals/atari_st/dma_sound.rs` (internal tests, 3
tests) and `tests/atari_st.rs` (end-to-end playback via the bus,
XSINT/Timer A).

### Microwire / LMC1992 (`src/peripherals/atari_st/microwire.rs`)

`peripherals::atari_st::microwire::Microwire` models the external
LMC1992 mixer (STE), driven serially via `$FF8922` (DATA)/`$FF8924`
(MASK): master volume, independent left/right volume, AND a bass/treble
filter — applied to the FINAL output signal (PSG *and* DMA Sound already
mixed), not to an individual source.

- Command decoding: reproduces the LMC1992 algorithm (address prefix,
  command selector, value), volume tables (master, left/right) taken
  as-is from Hatari.
- **Bass/treble filter** (`filter_left`/`filter_right`, Hatari-style
  `DmaSnd_Bass_Shelf`/`Treble_Shelf`/`Set_Tone_Level`/`IIRfilterL`/`R`,
  `dmaSnd.c`): two first-order "shelf" filters — bass (118.2763 Hz
  knee), treble (8438.756 Hz knee), values measured on the real
  LMC1992 circuit — algebraically combined into a single biquad, 13 gain
  steps (-12dB to +12dB in 2dB increments) precomputed for Rust68's
  fixed output frequency (44100 Hz). The volume gain applies at the
  filter's INPUT (same pass), not downstream — the cartridge/SDL2 binary
  smooths this gain over time BEFORE injecting it, to avoid a zipper
  noise "click" on an abrupt change. Filter state (2 intermediate
  samples) INDEPENDENT per left/right channel. Steem SSE also implements
  a bass/treble filter but with a different topology ("Audio EQ
  Cookbook" form) and its own code admits it is not faithful ("does give
  bass and treble but certainly not close to the STE") — which is why
  the Hatari version was ported, not Steem's.
- Not modeled: mixing mode (no audible effect with a single host output
  source), real serial timing (instantaneous decoding, documented as
  having no functional consequence).

Tested in `src/peripherals/atari_st/microwire.rs` (internal tests, 9
tests including filter behavior at steady state for bass at maximum/
minimum, exact transparency at the default setting, and channel
independence) and `tests/atari_st.rs` (end-to-end master volume wiring).

---

## Shifter (`src/peripherals/atari_st/shifter.rs`)

`peripherals::atari_st::shifter::Shifter` models the video chip alone: it
reads video RAM line by line and converts it to 24-bit RGB pixels
according to the programmed resolution (low 320×200/4 planes, medium
640×200/2 planes, high 640×400/1 monochrome plane). Mapped into
`AtariSt` (public field `shifter`, addresses `$FF8201`/`03` video base,
`$FF8205`/`07`/`09` video counter, `$FF8240`-`$FF825E` 16-color palette,
`$FF8260` resolution).

- Bitplane deinterleaving (standard Atari ST word-interleaved bitplane
  format: for a group of 16 pixels, one consecutive word per plane) —
  done word by word then bit by bit, MSB first.
- 16-color palette, two formats depending on `set_ste_palette`/
  `MachineProfile::ste_palette` (wired in `AtariSt::from_model`): ST
  (9 bits, 3 bits/component, `0x0777` mask) or STE (12 bits, 4
  bits/component, `0x0FFF` mask) — both masks checked against Hatari
  (`video.c`, `Video_ColorReg_WriteWord`).
- **Real bug fixed**: in the STE palette, bit 3 of each color nibble is
  NOT the high-order bit of a normal 4-bit value — it is a fine-precision
  bit added at the BOTTOM by the hardware to stay compatible with the
  ST's 3-bit format (bits 2-0, same bus positions). The real intensity
  is `(bits2-0 << 1) | bit3`, not the nibble read as-is (confirmed
  against Hatari, `conv_st.c`, `ConvST_SetupRGBTable`). Before the fix,
  `0x0777` (the "ST 3-bit style" maximum, what an STE game naturally
  writes for a component at full intensity) gave RGB 119/255 (47%)
  instead of 238/255 (93%, one notch below true white) — a systematic
  and severe darkening of any palette built this way, consistent with
  game colors reported as "very dark". See `ste_nibble_to_intensity` in
  the module.
- Wired into `AtariSt::tick` on the GLUE's HBL/VBL rhythm: detects
  changes in `Glue::current_line`/`frame_count` (via an **absolute**
  line counter, not the GLUE's wrapping counter) and feeds
  `AtariSt::framebuffer` (`Vec<Vec<(u8,u8,u8)>>`, one entry per line).
- **Cycle-exact STE fine scrolling** (`$FF8264`/`$FF8265` HorScroll
  WITHOUT/WITH preload, `$FF820F` LineWidth), Hatari-style. Hatari itself
  does not do real-time pixel-by-pixel rendering: it timestamps register
  writes during the line to decide where they apply, then converts the
  whole line at once at HBL — a model already in place here
  (`render_scanline` per line), no pipeline overhaul needed. `Glue`
  exposes 3 new accessors (`cycles_in_line`/`line_start_cycle`/
  `line_end_cycle`, 56/376 in PAL, 52/372 in NTSC) letting the board
  compute whether a write applies to the CURRENT line or is deferred to
  the next one (`pending_h_scroll`/`pending_line_width`, committed at the
  end of `render_scanline`) — thresholds identical to Hatari (`video.c`,
  `Video_HorScroll_Write`/`Video_LineWidth_WriteByte`). `$FF8265` (with
  preload) reads one extra group of 16 pixels per line to fill the right
  edge without loss; `$FF8264` (without) blackens the first 16 pixels
  (shift register not yet loaded). Deliberately out of scope: the
  "`$8265` then `$8264`=0" bug (336-pixel line, specific demos), timing
  variants across STF motherboard revisions — unrelated to the
  scrolling mechanism itself.
- **Border/overscan removal (Hatari-style unless otherwise noted)**: the
  VERTICAL overscan (top/bottom, `$FF820A`) is modeled — see the GLUE
  section above, `Glue::write_sync`/`display_line`. Part of the
  HORIZONTAL overscan is modeled (`Shifter::write_resolution`/
  `write_sync`, `border` module): `LEFT_OFF_2_STE` (left border, hi-res
  trick then a stack-precise return at cycle 4, +20 bytes, short STE
  variant only — not the original ST variant with medium-resolution
  stabilizer), `RIGHT_OFF` (right border via `$FF820A` in the
  `]372,376]` window, +44 bytes), and the `LEFT_PLUS_2`/`RIGHT_MINUS_2`
  nudges (±2 bytes, early 60Hz switch). Cycle constants and byte effects
  taken as-is from Hatari (`video.h`/`video.c`, `Video_Update_Glue_State`,
  STE machine values), including the cancellation windows SPECIFIC to
  each mechanism on a return to 50Hz (`LEFT_PLUS_2` up to cycle 52,
  `RIGHT_MINUS_2`/`RIGHT_OFF` up to cycle 376 — not a global
  cancellation, checked against `video.c`). `STOP_MIDDLE` (line
  shortened by 106 bytes, hi-res switch mid-line, `]4,164]`) and
  `RIGHT_OFF_FULL` (TOTAL removal of the right border, +22 bytes ON TOP
  OF `RIGHT_OFF`, hi-res switch in `]164,376]`, which FORCES
  `LEFT_OFF_2_STE` from the START of the NEXT line — cross-line cascade,
  `pending_left_off_next_line`) are also modeled, triggered by
  `Shifter::write_resolution` (not `$FF820A`). Documented limitations
  (see `Shifter`'s module doc): `BORDERBYTES_*` calibrated for low
  resolution and applied as-is to other resolutions (no per-plane
  scaling); left/right border rendered INDEPENDENTLY of STE fine
  scrolling (no fidelity if both tricks are combined on the same line);
  `RIGHT_MINUS_2` without `RIGHT_OFF` has no effect; `STOP_MIDDLE`
  cancellation modeled via a resolution change back (`$FF8260`, before
  cycle 164), not via `$FF820A` as Hatari actually does in this specific
  case (documented deviation, design choice); once triggered,
  `RIGHT_OFF_FULL` cannot be canceled within the same line (Hatari has a
  cancellation window via `$FF820A`, not modeled).
  **`OVERSCAN_MED_RES`/`FOUR_BIT_SCROLL` (refined medium-resolution left
  border): modeled, but based on Steem SSE, not Hatari.** Research was
  done on the Hatari side first (`video.c`, `Video_WriteToGlueRes`):
  detection there is a stack of case-by-case reverse-engineered
  heuristics tied to specific demos (branches literally commented "No
  Cooper", "PYM", "ST Connexion", each with its own cycle thresholds),
  including two STE-specific offset constants that Hatari itself labels
  unverified ("should be measured on real STE", `video.c`, comments near
  lines 3940-3981) — judged too fragile to port as-is. Steem SSE
  (another open-source ST/STE emulator, `glue.cpp`,
  `TGlue::CheckSideOverscan`) models the SAME trick with a generalizable
  approach: instead of fixed thresholds, it measures the number of
  cycles that ACTUALLY elapsed between successive resolution changes
  within the line (history of `$FF8260` writes) — the cycle windows
  converge well with Hatari's (real corroboration), and this GENERAL
  formula (not case-by-case fixed thresholds) was judged portable. This
  is the one implemented (`Shifter::detect_med_res_tricks`/
  `resolution_write_history`): precondition that `LEFT_OFF_2_STE` is
  already active on the line (these two tricks refine an already
  revealed left border, they do not trigger it themselves);
  `OVERSCAN_MED_RES` (last switch to medium resolution within `]24,48]`)
  and `FOUR_BIT_SCROLL` (same trigger, `[16,48]` window, also measures
  the following change) shift the READ position of the left border
  (Steem's `SHIFT_SDP` style) without changing its width or the video
  counter's advance. Documented limitations (see `Shifter`'s module
  doc): Steem's guard `!ShiftModeChangeAtCycle(...)` is not reproduced
  (function not fully understood); `HblPixelShift` (fine pixel-level
  offset, via the `hscroll` register, for two precise cycle values) is
  not modeled — Steem itself labels this value as "maybe coming from the
  demo author", so no more verified than the rest.

Limitations: video counter always accepted for writes (STE behavior, not
read-only as on the original ST); high-resolution mode polarity
convention (bit=1 → black) not verified against a real hardware capture;
no DRAM/video contention modeled for Shifter access.

Tested in `tests/shifter.rs` (35 tests, including reordering of the STE
fine-precision bit, fine scrolling with/without preload, and the
horizontal border mechanisms `LEFT_OFF_2_STE`/`RIGHT_OFF`/nudges/
`STOP_MIDDLE`/`RIGHT_OFF_FULL`/`OVERSCAN_MED_RES`/`FOUR_BIT_SCROLL`) and
`tests/atari_st.rs` (full 313-line PAL frame rendering, cycle-exact
"gating" of scroll/line-width writes depending on position within the
line, among other end-to-end tests).

---

## WD1772 (`src/peripherals/atari_st/wd1772.rs`)

`peripherals::atari_st::wd1772::Wd1772` models the floppy controller
alone: 4 registers (Command/Status, Track, Sector, Data) multiplexed by
the A1-A0 lines, Type I command set (Restore/Seek/Step/Step-In/
Step-Out), Type II (Read/Write Sector, single- and multi-sector) and
Type IV (Force Interrupt).

- The disk is abstracted via the `FloppyDisk` trait (track/side/sector
  addressing, not byte), an object (`Box<dyn FloppyDisk>` on the
  `AtariSt` side) to accept several formats without coupling the board
  to any one of them: `RawDiskImage` for raw `.st` (linear sectors),
  `stx::StxImage` for a minimal `.stx` (Pasti) reader — reverse-engineered
  by direct inspection of real files then cross-checked against the
  format's public documentation and Hatari, now exposes `bit_position`
  (the real position of the ID field on the physical track, used by
  `Wd1772::cycles_to_target_sector` for a faithful rotational latency
  calculation even on a non-standard-formatted track) — and `msa::parse`
  for `.msa` (Magic Shadow Archiver, a simple RLE-compressed
  track-by-track container, decompressed in memory into an equivalent
  `RawDiskImage`, no protection metadata in this format).
- Type II transfer via the `DmaChannel` trait (`pull`/`push` one byte at
  a time): the WD1772 knows nothing of RAM, only the disk and this
  channel — it is up to the board to implement it with access to its
  RAM.
- **Real timing, not instantaneous**: `execute_command` only STARTS the
  command (sets `BUSY`); it is `tick()`, called by the board on every
  clock advance, that progresses the real delay and finishes the command
  (final status, `/INTRQ`) once elapsed — `BUSY` is therefore genuinely
  observable by software polling it, as on real silicon. Rotational
  latency via a disk angular position tracked continuously
  (`rotation_phase`, not a simple fixed average delay: sequential
  sector-by-sector reading finds a neighboring sector much faster than a
  randomly picked one, as on real silicon); head load (15 ms) gated on
  the command's `E` bit, not automatic. Constants checked against Hatari
  (`fdc.c`). No real verification/CRC (V bit always succeeds), Type III
  (Read Address/Track, Write Track/Format) not implemented.

Wiring in `systems::atari_st::AtariSt`: multiplexed register at
`0xFF8604` (register select via `0xFF8606`, simplified model — no real
sector-count register nor real FDC/HDC selection), DMA address counter
at `0xFF8609`/`0B`/`0D`. `/INTRQ` wired to `GPIP5` of the MFP, relayed
by `AtariSt::tick`. Disk inserted via the public field
`AtariSt::floppy_a`.

Tested in `tests/wd1772.rs` (20 tests, including rotational latency via
`bit_position` and stopping at the real sector count per track),
`src/peripherals/atari_st/msa.rs` (8 tests), `src/peripherals/atari_st/stx.rs`
(9 tests) and `tests/atari_st.rs` (end-to-end sector read/write
round-trip via DMA, among other end-to-end tests).

---

## Blitter (`src/peripherals/atari_st/blitter.rs`)

`peripherals::atari_st::blitter::Blitter` models the Atari STE's block
transfer coprocessor (BitBlt) alone: it combines a source word
(optionally shifted bit by bit via `skew`, a 32-bit shift register
PERSISTENT across the chip's entire lifetime — not reset to zero between
two lines nor between two logically separate blits), a halftone pattern,
and the destination content, via a programmable boolean function (`OP`,
one of 16 two-input functions, standard truth-table convention shared by
many "raster op" chips), with line-edge masking (`ENDMASK1/2/3`) and
X/Y increment traversal. Mapped into `AtariSt` at `0xFF8A00` (public
field `blitter`). Processed **word by word** (a direct translation of
Hatari's state machine, `Blitter_ProcessWord`), not line by line with a
precomputed advance formula, in order to faithfully reproduce this
persistent shift register.

In HOG mode (bit 6 of `CONTROL`), `execute()` processes the whole blit in
a single call. In non-HOG mode (the most common in practice), a single
call only processes 64 real bus accesses (source read, destination
read/write, each counted separately — value and counting method taken
from Hatari, `BLITTER_NONHOG_BUS_BLITTER`) before yielding control, with
`BUSY` staying set between two slices — the CPU then has 256 cycles
(`AtariSt::BLITTER_SLICE_CYCLES`, calibrated on the same `64*4` as
Hatari) to run in parallel before `AtariSt::tick` calls `execute()` again
for the next slice. The initial trigger happens via a write to the
BUSY/START bit of the control register; an accidental CONTROL trigger
(`TAS.B` in the typical software restart loop) does NOT re-execute the
whole blit from the start — `execute()` resumes exactly where the
previous slice stopped (see `armed`).

`CONTROL` (`0xFF8A3C`: BUSY/HOG/SMUDGE/halftone line number) and `SKEW`
(`0xFF8A3D`: FXSR/NFSR/shift) registers match the chip's real offset —
cross-checked against the `BLITTER.TXT` datasheet (info-coach.fr), the
`BLIT_FAQ.TXT` (`ggnkua/Atari_ST_Sources` repo), and Hatari's source code
(`src/blitter.c`), all three of which agree. An earlier version had
these two offsets swapped (fixed bug) and treated `HOP=0` as "all zero"
instead of "all one" (per the datasheet's table, also fixed).

- `FXSR`/`NFSR` (bits of the `SKEW` register) honored explicitly rather
  than deduced from `skew != 0`: `FXSR` triggers the priming read at the
  start of a line, `NFSR` suppresses the last source read of the line.
- `SMUDGE` (bit of the `CONTROL` register) implemented: the halftone word
  used for each word comes from the low 4 bits of the shifted source
  word, potentially different for each word of the same line (instead of
  the current line number in normal mode).
- The halftone line number (bits 0-3 of `CONTROL`) is directly
  readable/writable by software (not a hidden internal counter), and
  advances or retreats at the end of a line depending on the sign of
  `DST_Y_INC`, per the datasheet.
- No "line draw" mode for polygon plotting (unlike the Amiga Blitter):
  this is not a limitation of this implementation, the Atari STE chip
  simply doesn't have this feature — the CONTROL register's "line
  number" field is used only for halftone selection as above.

`skew` correctly handles traversal direction (positive/negative
`SRC_X_INC`, "mirrored" blit): the source shift register is fed
differently depending on the sign (`shift_buffer`/`fetch_buffer`, a
direct translation of Hatari's `Blitter_SourceShift`/
`Blitter_SourceFetch`) — an earlier version always assumed forward
traversal.

**Remaining limitations, to take with caution** (no suite equivalent to
TomHarte exists for this peripheral):
- The 64-bus-access slice model remains an approximation of Hatari's
  "cycle exact" mode, which interleaves these accesses IN THE MIDDLE of
  CPU instruction execution (not just between complete instructions) and
  reproduces a documented real-silicon bug case where the Blitter
  sometimes stops at 63 accesses instead of 64 — not modeled here.

Tested in `tests/blitter.rs` (23 tests: OP truth table, HOP including
HOP=0, endmask, X/Y traversal, halftone line number cycle and register,
FXSR, NFSR, SMUDGE, skew=0/mirror, non-HOG slices counted in real bus
accesses), `tests/blitter_hatari_diff.rs` (direct port of
`Blitter_ProcessWord` as a differential oracle) and `tests/atari_st.rs`
(end-to-end blit triggered via the control register, among other
end-to-end tests).

---

## Atari ST Board (`src/systems/atari_st/mod.rs`)

`systems::atari_st::AtariSt` implements `Bus` for a minimal ST/STE,
wiring together all the chips above:

- Installed RAM at `0x000000` (chosen size), TOS ROM at `0xFC0000`
  (`DEFAULT_ROM_BASE`, read-only).
- MFP 68901 at odd addresses `0xFFFA01`-`0xFFFA2F` (public field `mfp`).
  Keyboard ACIA (`0xFFFC00`/`02`) and MIDI ACIA (`0xFFFC04`/`06`), public
  fields `acia_keyboard`/`acia_midi`, the first wired to an IKBD
  controller (field `ikbd`, see dedicated section). YM2149
  (`0xFF8800`/`02`, field `ym2149`). Shifter (`0xFF8201`+, field
  `shifter`). WD1772/DMA (`0xFF8604`+, fields `wd1772`/`floppy_a`).
  Blitter (`0xFF8A00`+, field `blitter`, triggered by a write to the
  BUSY/START bit of its control register).
- Beyond installed RAM but within the fixed 4 MB "ST RAM" address space
  (two 2 MB MMU banks on a real ST/STE), an access **never** triggers a
  bus error: the MMU always responds with /DTACK there on real silicon,
  even without physical RAM at the precise address (confirmed by the
  Atari community). Modeled with a fixed unstored value (a read never
  returns what was just written) rather than the real value (a bus
  capacitance residue, non-deterministic). It is this lack of
  persistence, not a bus error, that TOS observes for its RAM detection
  at the very start of the cold boot. The real "hole" (bus error via
  `take_bus_fault`, the mechanism many programs/demos use once TOS has
  started) only begins at 4 MB, up to `0xFF8000`.
- Rest of the unmapped I/O area: real chip select but peripheral not yet
  emulated → neutral read `0xFF`, write ignored (no bus error, so as not
  to break software status polling).
- `irq_level`/`irq_ack` wire the MFP (IPL6), VBL (IPL4) and HBL (IPL2) in
  decreasing priority order. The two ACIAs and the WD1772 don't generate
  an IPL directly: their IRQs are OR-wired onto `GPIP4`/`GPIP5` of the
  MFP.
- `reset_bus` resets MFP/ACIA×2/YM2149/Shifter/WD1772; the GLUE, however,
  is **not** reset (video timing keeps going independently of a CPU
  `/RESET` on real silicon); neither is the inserted disk (`floppy_a`)
  (physical medium, not chip state).
- `AtariSt::tick(cpu_cycles)` advances MFP/GLUE/YM2149, relays
  ACIA/WD1772 IRQs, and triggers video rendering (Shifter) at the GLUE's
  HBL/VBL rate — to be called explicitly by the caller after each
  `Cpu::step` (this crate does not advance peripherals on its own).

Known limitations: no ROM mirror at `0xE00000` (130ST); no DRAM/video
contention (`is_contended` stays `false`); UDS/LDS decoding of even
addresses adjacent to the MFP not precisely modeled; simplified
DMA/WD1772 registers (see WD1772 section).

Tested in `tests/atari_st.rs` (55 tests, including several end-to-end
ones: GPIP→MFP→IPL interrupt, MFP/VBL/HBL priority, ACIA→GPIP4→MFP,
WD1772→GPIP5→MFP, video rendering, floppy read/write via DMA, blit
triggered via the control register, color monitor persisting across
`/RESET`).

---

## Model lexicon (`src/systems/atari_st/model.rs`)

General principle, valid for any machine being emulated (not just this
one): emulating a specific real machine isn't about picking a random RAM
size and hoping it works — it's a set of characteristics that go
together (CPU speed, original RAM, expected ROM/BIOS, hardware options).
`model::AtariModel` gathers these characteristics for the ST/STE range in
a queryable form (`AtariModel::profile() -> MachineProfile`) rather than
leaving them scattered as magic constants in the demo binary.

- 6 models covered: `St520`, `St1040`, `MegaSt`, `Ste520`, `Ste1040`,
  `MegaSte` — characteristics (RAM, stock Blitter, original TOS, CPU
  frequency) cross-checked from several public references (Wikipedia,
  old-computers.com, atari-wiki.com, atari-forum.com).
- `AtariModel::parse(name)`: case- and separator-insensitive recognition
  (`"1040ste"`, `"1040STE"`, `"Mega-STE"`…).
- `AtariSt::from_model(profile, rom)`: builds the board with the RAM and
  Blitter presence of the chosen model (`AtariSt::blitter_present`,
  checked by `is_blitter_addr`); the ROM is supplied separately (the
  installed TOS version is **not** a property of the model — a real ST
  can perfectly well run with a newer TOS than the original one, a
  common EPROM upgrade). The ROM base (`0xFC0000` vs `0xE00000`) is
  still set independently via `set_rom_base`, already auto-detected from
  `os_version` in the TOS header.
- Demo binary: `--model <name>` (default `1040ste`) — running the binary
  WITHOUT arguments (no dedicated `--help` flag: `--help` would be
  treated as a ROM path and fail to be read) prints the usage message
  listing the models. Example:
  `cargo run --release --features sdl2-frontend --bin atari_st_sdl2 --
  --model 520ste tos162.img disk.stx`.

What this lexicon does **not** (yet) model:
- `cpu_hz` is informative only: both the MFP rate (fixed 192/625 clock
  ratio) and the `atari_st_sdl2` binary's audio pacing assume an 8 MHz
  CPU. Choosing a Mega STE model therefore does not make the emulation
  run at 16 MHz.
- The Mega ST is modeled with the Blitter present by default (PLCC
  socket present on all motherboards but not always populated at the
  factory in early runs) — adjust manually via `MachineProfile.has_blitter`
  if a precise Blitter-less Mega ST is needed.

Tested in `src/systems/atari_st/model.rs` (3 unit tests) and
`tests/atari_st.rs` (1 end-to-end test: RAM/Blitter set according to the
model).

---

## What remains for the Atari ST emulation

The MC68000 CPU is complete, 100% state-compliant AND 100% cycle-exact,
trace mode included. IPL interrupts, the MFP 68901, the GLUE, the two
MC6850 ACIAs, the YM2149, the Shifter, the WD1772, the Blitter, and a
minimal board wiring it all together are in place — every component of
the original roadmap is covered.

A real, unmodified TOS 1.62 boots up to the interactive GEM desktop
(low-resolution color video, keyboard, mouse, floppy disk icons,
pull-down menus) via the `atari_st_sdl2` demo binary (`cargo run
--release --features sdl2-frontend --bin atari_st_sdl2 -- <rom.img>
[disk.st|.stx|.msa]`) — see the Architecture section for details on
Cargo features. The "warm restart" shortcut (pre-filled
`memvalid`/`memval2`/`memval3`/`phystop` cookies, see the `main` code)
remains the recommended default path (fast, reliable);
`RUST68_COLD_BOOT=1` forces a real cold boot (real RAM detection by TOS,
slower), functional but less tested day-to-day.

**Resolved GEM bugs**:
- *Mouse clicks with no effect* (icons, menu items) — a multi-layered
  cause: the IKBD controller didn't exist at all (TOS commands never
  interpreted, GPIP4 edge starved between bytes of the same frame,
  `$0EE4` not pre-filled — see the IKBD section), then one final,
  simpler bug once the IKBD was in place: the left/right bits of the
  mouse packet were swapped (`queue_mouse_move`, `atari_st_sdl2.rs`).
  "Exclusive" mouse mode (`Cmd+Shift+F10`) was added on this occasion.
- *Corrupted text in GEM pull-down menus* — `AtariSt::write16` split
  every combined `.W` write to the Blitter's CONTROL+SKEW (`$FF8A3C`/
  `3D`) into two sequential `write8` calls, CONTROL then SKEW; but
  writing CONTROL synchronously triggers `Blitter::execute()` as soon as
  the BUSY bit is set, so the Blitter sometimes started with the OLD
  SKEW, an instant before the new one was set by that same `.W` access
  (a real TOS 1.62 case: `MOVE.W D7,(A5)` at `$E11746`, arming the 4
  planes of a menu-restoring blit with a shared SKEW). Fixed by writing
  SKEW before delegating to `write8` for CONTROL. Verified by direct
  comparison with a real Hatari (differential trace patch
  `RUST68_HATARI_TRACE` on `blitter.c`): both now run identically on
  this blit. Diagnostic tools kept in the tree (zero cost when not
  enabled): `RUST68_TRACE_BLIT_REGS`/`RUST68_TRACE_BLIT_START`/
  `RUST68_TRACE_BLITTER_WORDS`, `examples/rd_menu_repro.rs`/
  `rd_menu_ca6a.rs` (headless reproduction of a menu click-drag, without
  SDL2 or real interaction).

Avenues for further work (none of them are blockers, they are
deepenings):
- Verification of points documented as unconfirmed against a real
  hardware reference (MFP/GLUE timing, Shifter high-resolution polarity,
  the Blitter's "63 bus accesses instead of 64" edge case).
- DRAM/video contention for Shifter/Blitter accesses (the mechanism is
  already generic via `Bus::is_contended`, just not yet wired to these
  two chips).
- `.stx`: per-sector protection metadata (fuzzy bits, timing)
  deliberately ignored by the minimal reader — real game protections
  would remain blocked.
- Residual timing drift (~60-80 VBL on a very large protected floppy
  transfer, `Rick_Dangerous.stx`) after the STX rotational latency fix
  (`bit_position`) — cause not identified, final crash unchanged (A6=0
  at the same location).
- **Unresolved GEM bug**: in the standard GEM desktop (TOS 1.62, no
  third-party software), opening `Desktop > Info` (whose Atari logo
  animates through a color cycle, the only desktop dialog to do
  continuous work while it stays open) triggers a repeated beep (the
  internal "ping", several times per second) and produces a "rotated"
  mouse cursor movement (right→down, left→up, up→left, down→right — a
  dx↔dy swap). Both stop when the dialog is closed; no other modal
  dialog tested reproduces the problem. Avenues already ruled out after
  investigation (`RUST68_TRACE_IKBD`/`_DISPATCH`/`_READER` traces,
  `RUST68_TRACE_MFP_REQUEST`, screenshot taken during the bug): malformed
  mouse packets at the byte level (no — well-formed throughout),
  absolute mouse mode `$09` never sent for this specific dialog (no — no
  `$09` command in the trace), corrupted video rendering (no — clean
  screenshot, logo animating normally), MFP request flow chan=4/GPIP4
  (confirmed red herring — normal, expected behavior as soon as the
  mouse moves, not a sign of malfunction). Two real, unrelated bugs were
  found and fixed along the way (see the MFP and IKBD sections) but
  neither resolved this specific case. Most promising lead, not yet
  verified: the VBL (which drives the logo's color cycle) might be
  delayed/disrupted by ACIA traffic during this particular dialog,
  precisely because it's the only one that continuously exercises the
  VBL path — would need an `RUST68_TRACE_IRQ`-only trace (not combined
  with `_MFP_REQUEST`, too voluminous together) to confirm.
