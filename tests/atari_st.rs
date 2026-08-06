#![cfg(feature = "atari-st")]
//! Tests for the Atari ST board (`rust68::systems::atari_st::AtariSt`).

use rust68::peripherals::atari_st::acia;
use rust68::peripherals::atari_st::blitter::reg as blitter_reg;
use rust68::peripherals::atari_st::mfp::{channel, reg};
use rust68::peripherals::atari_st::shifter::addr as shifter_addr;
use rust68::peripherals::atari_st::wd1772::{self, RawDiskImage, SECTOR_SIZE};
use rust68::peripherals::atari_st::ym2149;
use rust68::systems::atari_st::{
    ACIA_KEYBOARD_CONTROL, ACIA_KEYBOARD_DATA, ACIA_MIDI_CONTROL, ACIA_MIDI_DATA, AtariSt,
    BLITTER_BASE, DEFAULT_ROM_BASE, DMA_ADDR_HIGH, DMA_ADDR_LOW, DMA_ADDR_MID, DMA_MODE, FDC_DATA,
    GLUE_SYNC, IO_BASE, MEMORY_CONF, MFP_BASE, STE_DMA_SOUND_BASE, YM2149_DATA, YM2149_SELECT,
    model,
};
use rust68::{Bus, Cpu};

#[test]
fn gpip7_indicates_color_monitor_even_after_reset() {
    // GPIP7 = "MONO DETECT" signal from the monitor connector on a real
    // ST/STE: tied to ground (0) for a monochrome monitor, pulled up to 1
    // otherwise. Without this wiring, the TOS would read 0 by default and
    // wrongly force high-resolution monochrome mode (320x200 color is
    // expected by default here). The signal reflects an external physical
    // connection, not a resettable MFP state: it must remain at 1 even
    // after a software `/RESET`.
    let mut st = AtariSt::new(0x1000, vec![]);
    assert_eq!(st.read8(MFP_BASE) & 0x80, 0x80, "GPIP7 = 1 (color monitor)");
    st.reset_bus();
    assert_eq!(
        st.read8(MFP_BASE) & 0x80,
        0x80,
        "GPIP7 stays at 1 after /RESET"
    );
}

#[test]
fn ram_read_write() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x100, 0x42);
    assert_eq!(st.read8(0x100), 0x42);
}

#[test]
fn rom_read_only() {
    let mut rom = vec![0u8; 0x100];
    rom[0] = 0xAB;
    let mut st = AtariSt::new(0x1000, rom);
    assert_eq!(st.read8(DEFAULT_ROM_BASE), 0xAB);
    st.write8(DEFAULT_ROM_BASE, 0xFF); // must be ignored
    assert_eq!(st.read8(DEFAULT_ROM_BASE), 0xAB);
}

#[test]
fn from_model_sets_ram_and_blitter_presence() {
    use rust68::systems::atari_st::model::AtariModel;

    // 1040STE: Blitter present standard, 1 MB of RAM (the example given for
    // this glossary).
    // Full WORD write: an isolated `.B` access on this register is ignored
    // on real silicon (see `Blitter::write`).
    let mut ste = AtariSt::from_model(AtariModel::Ste1040.profile(), vec![]);
    ste.write16(BLITTER_BASE + blitter_reg::HALFTONE_BASE, 0x4200);
    assert_eq!(
        ste.read8(BLITTER_BASE + blitter_reg::HALFTONE_BASE),
        0x42,
        "Blitter present standard on 1040STE: register actually read/written"
    );
    assert_eq!(ste.take_bus_fault(), None);

    // 520ST: no Blitter present standard — the zone falls back to the
    // generic I/O stub (0xFF, writes ignored), like an absent peripheral.
    let mut st = AtariSt::from_model(AtariModel::St520.profile(), vec![]);
    assert_eq!(st.read8(BLITTER_BASE), 0xFF, "no Blitter: generic I/O stub");
    assert_eq!(st.take_bus_fault(), None, "real chip select: no bus error");
    st.write8(BLITTER_BASE + blitter_reg::CONTROL, 0x80); // must not trigger a blit
}

#[test]
fn mfp_mapped_at_odd_addresses() {
    let mut st = AtariSt::new(0x1000, vec![]);
    // reg::VR is logical register 11 -> address MFP_BASE + 11*2.
    let addr = MFP_BASE + (reg::VR as u32) * 2;
    st.write8(addr, 0x48);
    assert_eq!(st.mfp.read(reg::VR), 0x48);
    assert_eq!(st.read8(addr), 0x48);
}

#[test]
fn physical_hole_triggers_bus_fault() {
    let mut st = AtariSt::new(0x1000, vec![]); // tiny RAM
    // Beyond the fixed 4 MB "ST RAM" address space (see
    // `floating_access_beyond_ram_within_4mb` below): a real hole, with no
    // chip select at all.
    let addr = 0x0050_0000;
    let value = st.read8(addr);
    assert_eq!(value, 0xFF);
    assert_eq!(st.take_bus_fault(), Some((addr, false)));
    // The fault must be consumed (reset to None) by take_bus_fault.
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn floating_access_beyond_ram_within_4mb() {
    // On real silicon, the MMU always responds with /DTACK across the
    // entire "ST RAM" address space (4 MB) — never a bus error beyond the
    // installed RAM as long as we stay within that range, even without
    // physical RAM at the exact address. An access there "floats": unlike
    // real RAM, a read does NOT return what was just written — this is
    // exactly what the TOS observes (not an address mirroring) for its RAM
    // detection at the very start of a cold boot. Confirmed by Hatari's
    // source code (`stMemory.c`, `VoidMem_bank`) — see the doc of
    // `AtariSt::in_floating_st_ram` for the history of bus error/mirroring
    // attempts tried and abandoned here.
    let mut st = AtariSt::new(0x1000, vec![]); // 4 KB of real RAM

    st.write8(0x0010_0000, 0x42); // well beyond the real 4 KB, but < 4 MB
    assert_eq!(
        st.read8(0x0010_0000),
        0x00,
        "floating access: never returns what was just written"
    );
    assert_eq!(st.take_bus_fault(), None, "within 4 MB: never a bus error");

    // Just below the 4 MB limit: still floating.
    assert_eq!(st.read8(ST_RAM_ADDRESS_SPACE_TEST - 1), 0x00);
    assert_eq!(st.take_bus_fault(), None);
}

/// Local copy of the 4 MB limit (private constant of the module) to keep
/// the test readable without publicly exposing a value that isn't meant to
/// be a stable API.
const ST_RAM_ADDRESS_SPACE_TEST: u32 = 4 * 1024 * 1024;

#[test]
fn write_to_first_8_bytes_triggers_bus_error_with_real_rom() {
    // Documented in black and white by Atari (Mega Service Manual, RAM
    // memory map): "the first 8 bytes of ROM are mapped into addresses
    // 0-7. These are reset vectors which the 68000 uses on start-up" — a
    // permanent mirroring, distinct from the overlay that can be disabled
    // via MEMORY_CONF. Writing there must trigger a real bus error (Glue:
    // "asserts Bus Error if... writing to ROM"), not be silently ignored.
    // Confirmed necessary by the STe factory diagnostic cartridge (test
    // "I7 Bus error not detected").
    let rom = vec![0u8; 0x1000];
    let mut st = AtariSt::new(0x1000, rom);

    st.write8(0x0000, 0x42);
    assert_eq!(st.take_bus_fault(), Some((0x0000, true)));

    st.write8(0x0007, 0x42);
    assert_eq!(st.take_bus_fault(), Some((0x0007, true)));

    // From address 8 onward: normal RAM, no bus error (a read there still
    // sees the ROM overlay while it's active — see `AtariSt::overlay` —
    // but the write itself does land in RAM without a fault).
    st.write8(0x0008, 0x42);
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn write_to_first_8_bytes_without_real_rom_does_not_fault() {
    // Without real ROM (bare test rig), this hardware mirroring makes no
    // sense: several integration tests write directly to low RAM (test
    // reset vector, video content at address 0...).
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x0000, 0x42);
    assert_eq!(st.take_bus_fault(), None);
    assert_eq!(st.read8(0x0000), 0x42);
}

#[test]
fn unemulated_peripheral_responds_neutral_without_fault() {
    let mut st = AtariSt::new(0x1000, vec![]);
    let addr = IO_BASE + 0x950; // still-free zone, beyond the DMA sound/Microwire stub
    assert_eq!(st.read8(addr), 0xFF);
    assert_eq!(st.take_bus_fault(), None, "real chip select: no bus error");
    st.write8(addr, 0x12); // must not panic, simply ignored
}

#[test]
fn dma_sound_microwire_stub_responds_zero_not_0xff() {
    // Unlike the rest of the unemulated I/O zone (0xFF, see the previous
    // test): an STE TOS writes then re-reads the Microwire register in a
    // loop, waiting for it to drop back to zero (end of serial shift).
    // With 0xFF (bits always at 1), this wait never ends.
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(STE_DMA_SOUND_BASE + 0x22, 0x34);
    assert_eq!(st.read8(STE_DMA_SOUND_BASE + 0x22), 0x00);
    assert_eq!(st.take_bus_fault(), None);
}

#[test]
fn dma_sound_reads_samples_from_ram_via_full_bus() {
    // End-to-end wiring (not just the isolated `DmaSound` module): the
    // registers written via the `AtariSt` bus must drive an actual read
    // from ITS OWN RAM (see `AtariSt::next_dma_sample`).
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x100, 0x11);
    st.write8(0x101, 0x22);

    st.write8(STE_DMA_SOUND_BASE + 0x07, 0x00); // frame start low (base 0x100 already 0)
    st.write8(STE_DMA_SOUND_BASE + 0x05, 0x01); // frame start mid -> start = 0x100
    st.write8(STE_DMA_SOUND_BASE + 0x13, 0x02); // frame end low
    st.write8(STE_DMA_SOUND_BASE + 0x11, 0x01); // frame end mid -> end = 0x102
    st.write8(STE_DMA_SOUND_BASE + 0x21, 0x83); // mono, 50066 Hz
    st.write8(STE_DMA_SOUND_BASE + 0x01, 0x01); // PLAY (no loop)

    assert_eq!(st.next_dma_sample(50066), (0x11, 0x11));
    assert_eq!(st.next_dma_sample(50066), (0x22, 0x22));
    // End of frame reached, no loop: silence afterward.
    assert_eq!(st.next_dma_sample(50066), (0, 0));
}

#[test]
fn dma_sound_loop_pulses_timer_a_via_xsint() {
    // Real hardware wiring: XSINT (end of DMA frame, including when
    // looping) is connected to the MFP Timer A event-counting input —
    // without this relay, software that counts loops via this timer (the
    // STe factory diagnostic cartridge, Audio test) never gets its
    // interrupt. Reproduces this scenario precisely: Timer A in
    // event-counting mode (bit3 set), armed to interrupt after 3 loops.
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(0x100, 0x11);
    st.write8(0x101, 0x22);

    // A single-word frame (2 bytes — frame addresses are word-aligned on
    // real silicon, bit0 wired to ground, see `DmaSound::write`), loop
    // enabled: the 2 bytes consumed cause a loop every time.
    st.write8(STE_DMA_SOUND_BASE + 0x07, 0x00);
    st.write8(STE_DMA_SOUND_BASE + 0x05, 0x01); // start = 0x100
    st.write8(STE_DMA_SOUND_BASE + 0x13, 0x02);
    st.write8(STE_DMA_SOUND_BASE + 0x11, 0x01); // end = 0x102
    st.write8(STE_DMA_SOUND_BASE + 0x21, 0x83); // mono, 50066 Hz
    st.write8(STE_DMA_SOUND_BASE + 0x01, 0x03); // PLAY + LOOP

    // The data must be written BEFORE the control: `reload()` (triggered by
    // the TACR write) reloads the counter from the data.
    st.mfp.write(reg::TADR, 3); // 3 pulses before triggering
    st.mfp.write(reg::TACR, 0x08); // bit3 = event-counting mode
    st.mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    st.mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));

    // Mono, 2 bytes/frame: 2 calls consume an entire frame (1 XSINT edge).
    // 3 loops therefore require 6 calls.
    for _ in 0..5 {
        st.next_dma_sample(50066);
        assert!(!st.mfp.interrupt_requested(), "not yet 3 loops");
    }
    st.next_dma_sample(50066);
    assert!(st.mfp.interrupt_requested(), "3 loops: Timer A must have triggered");
}

#[test]
fn microwire_master_volume_wired_end_to_end_via_bus() {
    // End-to-end wiring (not just the isolated `Microwire` module): the
    // MASK ($FF8924) then DATA ($FF8922) registers written via the
    // `AtariSt` bus must drive `st.microwire.left_gain()`/`right_gain()` —
    // this is how the STe factory diagnostic cartridge changes volume
    // between its various tone tests (mask always `$7FF`,
    // data = command | `$400`).
    let mut st = AtariSt::new(0x1000, vec![]);
    assert!((st.microwire.left_gain() - 1.0).abs() < 0.001, "full volume by default");

    // Type=3 (master volume) << 6, value=0: the most attenuated index (-80dB).
    let cmd: u16 = 0x400 | (3 << 6);
    st.write8(0xFF8924, 0x07); // MASK high
    st.write8(0xFF8925, 0xFF); // MASK low -> 0x7FF
    st.write8(0xFF8922, (cmd >> 8) as u8); // DATA high
    st.write8(0xFF8923, (cmd & 0xFF) as u8); // DATA low -> decoded here

    assert!(st.microwire.left_gain() < 0.01, "command via the bus must strongly attenuate");
    assert!(st.microwire.right_gain() < 0.01, "both channels are attenuated by the master volume");

    // DATA always reads back as 0 immediately (silicon simulated as "shift
    // already finished", see the doc of `read8`).
    assert_eq!(st.read8(0xFF8922), 0x00);
}

#[test]
fn irq_level_wired_to_ipl6_via_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    assert_eq!(st.irq_level(), 0, "no MFP interrupt pending");

    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 0x01);
    st.mfp.write(reg::IERB, 1 << channel::GPIP0);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP0);
    st.mfp.set_gpip_input(0, true);

    assert_eq!(st.irq_level(), 6, "MFP wired to IPL6");
    let vector = st.irq_ack(6);
    assert_eq!(vector & 0x07, channel::GPIP0);
    assert_eq!(st.irq_level(), 0, "IACK cleared the pending MFP interrupt");
}

#[test]
fn reset_bus_preserves_ste_extended_palette() {
    // `ste_palette` is a characteristic of the silicon (which Shifter is
    // physically in the machine), not a register — RESET (which the TOS
    // itself executes early at startup, see `AtariSt::reset_bus`) must not
    // reset it. A previous bug replaced the entire Shifter with
    // `Shifter::new()` (always `ste_palette=false`), which silently
    // switched any STE machine back to ST palette (9 bits) as soon as the
    // TOS's first RESET instruction ran — reproduced here by writing a
    // 12-bit value ($0FFF, white) and checking that it is NOT truncated to
    // the ST mask ($0777) after reset.
    let profile = model::AtariModel::Ste1040.profile();
    let mut st = AtariSt::from_model(profile, vec![]);
    st.reset_bus();
    st.write16(shifter_addr::PALETTE_BASE, 0x0FFF);
    assert_eq!(
        st.shifter.palette_raw()[0],
        0x0FFF,
        "STE palette (12 bits) must survive reset_bus, not fall back to the ST mask (9 bits)"
    );
}

#[test]
fn reset_bus_resets_mfp() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.mfp.write(reg::IERA, 0xFF);
    st.reset_bus();
    assert_eq!(st.mfp.read(reg::IERA), 0, "reset_bus must reset the MFP");
}

#[test]
fn mfp_vbl_hbl_priority() {
    let mut st = AtariSt::new(0x1000, vec![]);
    assert_eq!(st.irq_level(), 0);

    // A complete line (512 PAL cycles): HBL alone pending.
    st.tick(512);
    assert_eq!(st.irq_level(), 2, "HBL alone pending: IPL2");

    // A complete frame (313 lines): VBL must dominate HBL.
    st.tick(512 * 312);
    assert_eq!(st.irq_level(), 4, "VBL present: IPL4 dominates HBL (IPL2)");

    // The MFP dominates everything else.
    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 0x01);
    st.mfp.write(reg::IERB, 1 << channel::GPIP0);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP0);
    st.mfp.set_gpip_input(0, true);
    assert_eq!(st.irq_level(), 6, "MFP present: IPL6 dominates VBL/HBL");

    // Acknowledge in priority order.
    st.irq_ack(6);
    assert_eq!(st.irq_level(), 4, "MFP acknowledged: VBL becomes visible again");
    let vbl_vector = st.irq_ack(4);
    assert_eq!(vbl_vector, 28, "level 4 autovector = 24+4");
    assert_eq!(st.irq_level(), 2, "VBL acknowledged: HBL becomes visible again");
    let hbl_vector = st.irq_ack(2);
    assert_eq!(hbl_vector, 26, "level 2 autovector = 24+2");
    assert_eq!(st.irq_level(), 0);
}

#[test]
fn tick_advances_mfp_and_glue_together() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    st.mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    st.mfp.write(reg::TADR, 200);
    st.mfp.write(reg::TACR, 7); // /200, the slowest period

    for _ in 0..600 {
        st.tick(512); // 600 lines: well over 1 frame and 1 timer period
    }

    assert!(st.glue.frame_count() >= 1, "the GLUE must have advanced");
    assert!(
        st.mfp.read(reg::IPRA) & (1 << (channel::TIMER_A - 8)) != 0,
        "the MFP must have advanced too"
    );
}

#[test]
fn reset_bus_does_not_touch_glue() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.tick(512 * 313); // 1 complete frame: VBL pending
    assert!(st.glue.vbl_pending());
    st.reset_bus();
    assert!(
        st.glue.vbl_pending(),
        "video timing continues independently of a CPU /RESET"
    );
}

/// End-to-end integration test: a real CPU takes an interrupt generated by
/// the MFP via the board, through the entire
/// Cpu::step -> Bus::irq_level -> Bus::irq_ack -> Mfp::iack mechanism.
#[test]
fn cpu_takes_an_mfp_interrupt_end_to_end() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    // Reset vector: SSP=0x2000, PC=0x0400.
    st.write32(0x0000, 0x0000_2000);
    st.write32(0x0004, 0x0000_0400);
    st.write16(0x0400, 0x4E71); // NOP, never executed if the IRQ is taken first

    let mut cpu = Cpu::new();
    cpu.reset(&mut st);
    cpu.sr &= !rust68::sr::IPL_MASK; // IPL mask = 0: nothing blocks the interrupt

    st.mfp.write(reg::DDR, 0x00);
    st.mfp.write(reg::AER, 0x01);
    st.mfp.write(reg::IERB, 1 << channel::GPIP0);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP0);
    st.mfp.write(reg::VR, 0x40); // base vector 0x40, channel 0 -> vector 0x40
    st.write32(0x0040 * 4, 0x0000_0800); // handler at 0x0800
    st.mfp.set_gpip_input(0, true);

    let pc_avant = cpu.pc;
    let cycles = cpu.step(&mut st).unwrap();

    assert_eq!(cycles, 44);
    assert_eq!(cpu.pc, 0x0800, "the CPU must have jumped to the MFP handler");
    assert_eq!((cpu.sr & rust68::sr::IPL_MASK) >> 8, 6, "IPL mask raised to 6");
    assert_eq!(
        st.read32(cpu.sp().wrapping_add(2)),
        pc_avant,
        "the pushed return PC must be the one from before the interrupt"
    );
}

#[test]
fn acias_mapped_at_correct_addresses() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(ACIA_KEYBOARD_DATA, 0x99); // should go to the keyboard ACIA, not MIDI
    assert_eq!(st.acia_keyboard.take_tx_byte(), Some(0x99));
    assert_eq!(st.acia_midi.take_tx_byte(), None);

    st.write8(ACIA_MIDI_DATA, 0x77);
    assert_eq!(st.acia_midi.take_tx_byte(), Some(0x77));
    assert_eq!(st.acia_keyboard.take_tx_byte(), None);

    assert_eq!(
        st.read8(ACIA_KEYBOARD_CONTROL),
        st.acia_keyboard.read(acia::reg::CONTROL_STATUS)
    );
    assert_eq!(
        st.read8(ACIA_MIDI_CONTROL),
        st.acia_midi.read(acia::reg::CONTROL_STATUS)
    );
}

#[test]
fn acia_irq_relayed_via_mfp_gpip4() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.mfp.write(reg::DDR, 0x00); // GPIP4 as input
    // The ACIA's `/IRQ` is a real active-low signal wired without an
    // inverter: GPIP4 goes from 1 (idle) to 0 when the IRQ activates, a
    // FALLING edge (AER=0, the default value — no need to write it
    // explicitly, kept here for test readability).
    st.mfp.write(reg::AER, 0);
    st.mfp.write(reg::IERB, 1 << channel::GPIP4);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP4);

    assert_eq!(st.irq_level(), 0);

    // Enable RIE on the keyboard ACIA then receive a byte: IRQ requested.
    st.acia_keyboard.write(acia::reg::CONTROL_STATUS, 0x80);
    st.acia_keyboard.push_rx_byte(0x41);
    assert!(st.acia_keyboard.irq_requested());

    st.tick(4); // advances the GPIP4 wiring (see AtariSt::tick)
    assert_eq!(st.irq_level(), 6, "the ACIA IRQ must propagate up to the MFP (IPL6)");
}

#[test]
fn reset_bus_resets_acias() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.acia_keyboard.write(acia::reg::CONTROL_STATUS, 0x80);
    st.acia_keyboard.push_rx_byte(0x41);
    assert!(st.acia_keyboard.irq_requested());

    st.reset_bus();

    assert!(!st.acia_keyboard.irq_requested(), "reset_bus must reset the ACIAs");
}

#[test]
fn ym2149_mapped_at_ff8800_ff8802() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    st.write8(YM2149_DATA, 0x0F);
    assert_eq!(st.read8(YM2149_SELECT), ym2149::reg::AMPLITUDE_A);
    assert_eq!(st.read8(YM2149_DATA), 0x0F);
    assert_eq!(
        st.ym2149.channel_level(0),
        0,
        "without enabling a tone/noise gate in MIXER, the channel stays muted"
    );
}

#[test]
fn tick_advances_ym2149() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(YM2149_SELECT, ym2149::reg::MIXER);
    st.write8(YM2149_DATA, 0b0000_1001); // tone+noise gates for A open
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    st.write8(YM2149_DATA, 0x0F);

    st.tick(4);
    // 31, not 30: 4-bit amplitude -> 5-bit scale Hatari-style
    // (`YmVolume4to5`), not a simple x2 — see `Ym2149::channel_level`.
    assert_eq!(st.ym2149.channel_level(0), 31, "the YM2149 must have been clocked by tick()");
}

#[test]
fn reset_bus_resets_ym2149() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    st.write8(YM2149_DATA, 0x0F);
    st.reset_bus();
    st.write8(YM2149_SELECT, ym2149::reg::AMPLITUDE_A);
    assert_eq!(st.read8(YM2149_DATA), 0, "reset_bus must reset the YM2149");
}

#[test]
fn shifter_registers_mapped_correctly() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::VIDEO_BASE_HIGH, 0x00);
    st.write8(shifter_addr::VIDEO_BASE_MID, 0x10); // video base = 0x001000
    st.write8(shifter_addr::RESOLUTION, 0b00);
    assert_eq!(st.read8(shifter_addr::VIDEO_BASE_MID), 0x10);
    assert_eq!(st.shifter.resolution(), rust68::peripherals::atari_st::shifter::Resolution::Low);
}

/// End-to-end integration test: writes a known pattern into video RAM,
/// advances the board up to the first DISPLAYED line (see
/// `Glue::display_line` — a real top blanking of 63 PAL lines precedes the
/// first displayed line, needed for STE top-border removal), and checks
/// that the framebuffer contains the expected pixels.
#[test]
fn tick_renders_a_video_line_into_the_framebuffer() {
    let mut st = AtariSt::new(0x10000, vec![]);
    // Video base = 0x000000 (default), low resolution.
    st.write8(shifter_addr::RESOLUTION, 0b00);
    // Palette color 1 = white. Write as a word (normal CPU `.W` access):
    // two isolated `write8` calls would each duplicate their byte into both
    // halves of the register (real hardware behavior, see
    // `Shifter::write`) and would NOT give the intended result in general.
    let c1 = shifter_addr::PALETTE_BASE + 2;
    st.write16(c1, 0x0777);
    // Plane 0, first word = 0x8000 (pixel 0 set -> color 1).
    st.write8(0x0000, 0x80);
    st.write8(0x0001, 0x00);

    st.tick(512 * 63); // crosses the top blanking (63 PAL lines) up to the 1st displayed line

    assert!(!st.framebuffer.is_empty(), "a line must have been rendered");
    assert_eq!(st.glue.display_line(), Some(0));
    let line0 = &st.framebuffer[st.glue.display_line().unwrap() as usize];
    assert_eq!(line0.len(), 320);
    assert_eq!(line0[0], (255, 255, 255), "pixel 0 -> color 1 (white)");
    assert_eq!(line0[1], (0, 0, 0), "pixel 1 -> color 0 (black)");
}

#[test]
fn tick_renders_a_complete_frame() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);
    st.tick(512 * 313); // a complete PAL frame (313 lines, 200 of which are visible)
    // Only the 200 visible lines are rendered (see `Glue::visible_lines`
    // and `AtariSt::tick`): the Shifter only fetches/advances its video
    // counter there, not during the ~113 vertical blanking lines, exactly
    // as on real silicon.
    assert_eq!(st.framebuffer.len(), 200);
    assert!(
        st.framebuffer.iter().all(|line| line.len() == 320),
        "each rendered line must have the width of the low resolution"
    );
}

/// STE top-border removal end-to-end: a `$FF820A` write placed right at the
/// very start of a frame must cause extra lines (coming from the top
/// blanking, normally invisible) to appear at the START of the frame's
/// framebuffer.
///
/// Ticks line by line (512 cycles at a time), not the whole frame in a
/// single call: `Glue::tick` resets `display_start`/`display_end` to their
/// nominal value on frame wraparound, WITHIN the same call that would have
/// extended them — a `tick()` covering the entire frame at once would thus
/// lose the extension before the render catch-up loop even gets a chance
/// to use it (see the doc of `Glue::display_index`). No consequence in real
/// usage: the caller (SDL2 frontend, `AtariSt::tick` is documented for
/// this) ticks after EVERY CPU instruction, a handful of cycles at a time,
/// never a whole frame at once — only a "coarse" test like a giant isolated
/// `tick(512*313)` would be exposed to this.
#[test]
fn early_glue_sync_write_removes_top_border_end_to_end() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);
    // cycle ~200 in the line: early enough to stay within the VERTICAL
    // border-removal window (`LINE_REMOVE_BORDER_CYCLE`=504), but outside
    // the HORIZONTAL nudge windows `LEFT_PLUS_2`/`RIGHT_MINUS_2` (`<=36`)
    // and `RIGHT_OFF` (`]372,376]`) — writing at cycle 0 exactly would ALSO
    // trigger the horizontal nudge (same register, real and intended
    // hardware effect, see `Shifter::write_sync`), which would muddy this
    // test dedicated to the vertical effect alone.
    st.tick(200);
    st.write8(GLUE_SYNC, 0x00); // bit1=0: 60Hz

    st.tick(312); // finishes line 0
    for _ in 0..312 {
        st.tick(512);
    }

    // 63-34 = 29 extra lines (see `Glue::write_sync`), on top of the 200
    // nominal lines.
    assert_eq!(
        st.framebuffer.len(),
        229,
        "200 nominal lines + 29 revealed top-border lines"
    );
    assert!(st.framebuffer.iter().all(|line| line.len() == 320));
}

/// Symmetric case: bottom-border removal end-to-end, writing just before
/// the nominal end of the displayed window. Same reason for ticking line
/// by line as the previous test.
#[test]
fn glue_sync_write_at_end_of_display_removes_bottom_border_end_to_end() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);

    for _ in 0..262 {
        st.tick(512); // up to the last nominal displayed line (262, PAL)
    }
    // Same reason as in the previous test (cycle ~200, outside the
    // horizontal nudge windows): see the comment there.
    st.tick(200);
    st.write8(GLUE_SYNC, 0x00); // bit1=0: 60Hz
    st.tick(312); // finishes this line
    for _ in 0..50 {
        st.tick(512); // the rest of the frame
    }

    assert_eq!(
        st.framebuffer.len(),
        247,
        "200 nominal lines + 47 revealed bottom-border lines"
    );
    assert!(st.framebuffer.iter().all(|line| line.len() == 320));
}

/// RIGHT border removal (`RIGHT_OFF`) end-to-end, via a `$FF820A` write in
/// the middle of the frame (far from the edges, to trigger ONLY the
/// horizontal effect on the `Shifter` side — see `Shifter::write_sync` —
/// and not the vertical overscan of `Glue::write_sync`, which only
/// triggers near the top/bottom of the displayed window).
#[test]
fn glue_sync_write_mid_line_removes_right_border_end_to_end() {
    let mut st = AtariSt::new(0x1_0000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);

    for _ in 0..100 {
        st.tick(512); // advances to a well-displayed line, far from the frame edges
    }
    st.tick(374); // cycle 374: within the RIGHT_OFF window ]372,376]
    st.write8(GLUE_SYNC, 0x00); // bit1=0: 60Hz
    st.tick(512 - 374); // finishes the line: triggers rendering (HBL)

    // Verified empirically (as with Phase 0/1): after 100 tick(512),
    // `display_line()`==Some(37) and framebuffer[37] is already rendered
    // (PREVIOUS line) — it is the render finished by THIS tick(512-374),
    // one line further, that receives the write above: framebuffer[38].
    assert_eq!(
        st.framebuffer[38].len(),
        320 + 88,
        "320 nominal px + 88 right-border px (44 bytes, 4 planes)"
    );
    // The other lines of the frame, unaffected by this one-off write,
    // remain at the nominal width.
    assert_eq!(st.framebuffer[37].len(), 320);
}

/// Cycle-exact "gating" of STE fine-scroll writes (`$FF8264`/`$FF8265`): a
/// write before the active display start of the current line
/// (`Glue::line_start_cycle`, 56 in PAL) applies immediately — see the doc
/// of `AtariSt::write8` for these addresses and `Shifter::write_hscroll`.
/// RAM pattern identical to the Shifter's unit tests (plane 0, word all
/// ones): pixel 15 stays white with no active scroll, turns black as soon
/// as the scroll+prefetch applies (reaches the extra group, left at zero).
#[test]
fn hscroll_write_before_display_start_applies_to_current_line() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);
    st.write16(shifter_addr::PALETTE_BASE + 2, 0x0777); // color 1 = white

    // First crosses the top blanking (63 PAL lines, see
    // `Glue::display_line`) to reach the first actually displayed line —
    // otherwise the write below would fall in the border (always
    // immediate, which is not what this test checks). This first pass
    // already renders displayed line 0 (framebuffer[0], pattern not
    // relevant here — scroll still zero at this point): it is the NEXT
    // displayed line (framebuffer[1]) that this test targets, pattern
    // placed at byte 160 (0xA0, where it starts).
    st.tick(512 * 63);
    assert_eq!(st.glue.display_line(), Some(0));
    st.write8(0x00A0, 0xFF);
    st.write8(0x00A1, 0xFF); // plane 0, group 0 of displayed line 1 = 0xFFFF

    st.tick(4); // well before line_start_cycle (56 in PAL)
    assert!(st.glue.cycles_in_line() <= st.glue.line_start_cycle());
    st.write8(shifter_addr::HSCROLL_PREFETCH, 1);
    assert_eq!(st.shifter.h_scroll_count(), 1, "early write: effective immediately");

    st.tick(512 - 4); // finishes displayed line 1
    let line1 = &st.framebuffer[1];
    assert_eq!(line1[15], (0, 0, 0), "displayed line 1: the scroll was indeed applied to THIS line");
}

/// Symmetric to the previous test: a write AFTER the active display start
/// must NOT affect the current line, only the next one.
#[test]
fn hscroll_write_after_display_start_applies_to_next_line() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);
    st.write16(shifter_addr::PALETTE_BASE + 2, 0x0777);

    // Crosses the top blanking (see the previous test): already renders
    // displayed line 0 (pattern not relevant). Pattern for displayed line
    // 1 (starts at byte 160, read WITHOUT scroll so WITHOUT the extra
    // group: 160 bytes consumed — write too late for it) and for displayed
    // line 2 (starts at byte 320, read WITH scroll once it takes effect).
    st.tick(512 * 63);
    assert_eq!(st.glue.display_line(), Some(0));
    st.write8(0x00A0, 0xFF); // 160 = 0xA0, displayed line 1
    st.write8(0x00A1, 0xFF);
    st.write8(0x0140, 0xFF); // 320 = 0x140, displayed line 2
    st.write8(0x0141, 0xFF);

    st.tick(100); // after line_start_cycle (56 in PAL)
    assert!(st.glue.cycles_in_line() > st.glue.line_start_cycle());
    st.write8(shifter_addr::HSCROLL_PREFETCH, 1);
    assert_eq!(st.shifter.h_scroll_count(), 0, "late write: not yet effective (pending)");

    st.tick(512 - 100); // finishes displayed line 1 WITHOUT the scroll
    assert_eq!(st.shifter.h_scroll_count(), 1, "committed at the end of displayed line 1: effective going forward");
    let line1 = &st.framebuffer[1];
    assert_eq!(line1[15], (255, 255, 255), "displayed line 1: not yet scrolled (write too late for it)");

    st.tick(512); // finishes displayed line 2, scroll now active
    let line2 = &st.framebuffer[2];
    assert_eq!(line2[15], (0, 0, 0), "displayed line 2: scroll effective as of this line");
}

/// In the vertical border (outside the displayed window, see
/// `Glue::display_line`), there is no line currently being rendered to
/// protect: the write ALWAYS applies immediately, regardless of the cycle
/// position. The very start of the frame (top blanking, 63 PAL lines) is
/// already a border zone — no need to tick up to the bottom border to
/// verify it.
#[test]
fn write_in_vertical_border_always_applies_immediately() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);
    assert_eq!(st.glue.display_line(), None, "top blanking right from construction");

    st.tick(100); // arbitrary position, past line_start_cycle
    st.write8(shifter_addr::HSCROLL_PREFETCH, 5);
    assert_eq!(
        st.shifter.h_scroll_count(),
        5,
        "in the border, no line to protect: effective immediately"
    );
}

/// `$FF820F` (LineWidth) is gated by the active display end
/// (`Glue::line_end_cycle`, 376 in PAL), NOT the start — a different
/// threshold from the scroll one, faithfully reproduced (see
/// `Video_LineWidth_WriteByte` in Hatari: LineWidth is added to the address
/// when the active display ends, not at its start).
#[test]
fn linewidth_write_gated_by_end_of_display_not_start() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b00);

    // Crosses the top blanking to reach the start of the first displayed
    // line (see the scroll gating tests above) — this pass already renders
    // displayed line 0 and advances the video counter by 160 bytes;
    // resetting it to zero keeps the following calculations readable
    // (advance of THIS line only), without changing what this test checks
    // (the write's "gating", not the absolute value of the counter).
    st.tick(512 * 63);
    assert_eq!(st.glue.display_line(), Some(0));
    st.write8(shifter_addr::VIDEO_COUNTER_HIGH, 0);
    st.write8(shifter_addr::VIDEO_COUNTER_MID, 0);
    st.write8(shifter_addr::VIDEO_COUNTER_LOW, 0);

    // Write before line_end_cycle (376 in PAL): effective on line 0.
    st.tick(370);
    assert!(st.glue.cycles_in_line() <= st.glue.line_end_cycle());
    st.write8(shifter_addr::LINE_WIDTH, 3);
    assert_eq!(st.shifter.line_width(), 3, "before the end of display: effective immediately");
    st.tick(512 - 370);
    assert_eq!(st.read8(shifter_addr::VIDEO_COUNTER_LOW), 160 + 3 * 2, "line 0: advances with LineWidth=3");

    // Write after line_end_cycle: deferred to the next line.
    st.tick(400);
    assert!(st.glue.cycles_in_line() > st.glue.line_end_cycle());
    st.write8(shifter_addr::LINE_WIDTH, 5);
    assert_eq!(st.shifter.line_width(), 3, "after the end of display: not yet effective (pending)");
    st.tick(512 - 400);
    assert_eq!(st.shifter.line_width(), 5, "committed at the end of line 1: effective going forward");
}

#[test]
fn reset_bus_resets_shifter_and_resynchronizes_tracking() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(shifter_addr::RESOLUTION, 0b01); // medium resolution
    st.tick(512 * 5);
    st.reset_bus();
    assert_eq!(
        st.shifter.resolution(),
        rust68::peripherals::atari_st::shifter::Resolution::Low,
        "reset_bus must reset the Shifter"
    );
    // No massive catch-up on the next tick: a single short tick must render
    // at most one line.
    let lignes_avant = st.framebuffer.len();
    st.tick(4);
    assert!(st.framebuffer.len() <= lignes_avant + 1);
}

fn test_disk() -> RawDiskImage {
    let mut data = vec![0u8; 9 * SECTOR_SIZE];
    data[0] = 0xAB; // sector 1, track 0, side 0: recognizable pattern
    RawDiskImage::new(data, 80, 1, 9)
}

/// Programs the YM2149 port A to select drive A, side 0 (see
/// `AtariSt::floppy_drive_select`) — without this, port A keeps its
/// default value after reset (0xFF, "no drive selected"), exactly as on
/// real silicon before the TOS programs it.
fn select_drive_a_side_0(st: &mut AtariSt) {
    st.write8(YM2149_SELECT, ym2149::reg::IO_PORT_A);
    st.write8(YM2149_DATA, 0b1111_1101); // bit1=0 (drive A), bit0=1 (side 0)
}

/// Advances the emulation until the current WD1772 command completes (see
/// the doc of `wd1772::Wd1772::tick` — commands are no longer synchronous):
/// in blocks of 50,000 cycles, up to 3 million cycles total (ample margin
/// even for a complete Type II command — head load + rotational latency +
/// transfer).
fn run_disk_to_completion(st: &mut AtariSt) {
    let mut total = 0u32;
    while st.wd1772.busy() && total < 6_000_000 {
        st.tick(50_000);
        total += 50_000;
    }
    assert!(!st.wd1772.busy(), "the command did not finish within the expected cycle margin");
}

#[test]
fn fdc_registers_multiplexed_via_dma_mode() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(DMA_MODE, wd1772::reg::TRACK << 1);
    st.write8(FDC_DATA, 42);
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 42);
    assert_eq!(st.read8(FDC_DATA), 42);
}

#[test]
fn dma_mode_as_16_bits_decodes_selector_in_bits_1_2() {
    // Real bug fixed: `DMA_MODE` ($FF8606) is a 16-bit register on real
    // silicon, with the FDC register selector in bits 1-2 of the low byte
    // (confirmed by Hatari, `fdc.c`:
    // `FDC_reg = (FDC_DMA.Mode & 0x6) >> 1`) — NOT bits 0-1 of an isolated
    // `write8`. The TOS accesses it almost always with a full word write
    // (`move.w`, as here via `write16`), never an isolated `write8`: an
    // earlier version that naively decomposed this access byte by byte
    // only ever saw the high byte (always zero), leaving the selector
    // permanently stuck at COMMAND_STATUS no matter what the TOS wrote —
    // making any real Track/Sector/Data selection impossible, and thus any
    // floppy disk read.
    let mut st = AtariSt::new(0x1000, vec![]);

    // bits1-2 = 01 (TRACK); full value as a real TOS writes it.
    st.write16(DMA_MODE, 0x0082);
    st.write8(FDC_DATA, 7);
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 7, "bits1-2=01 -> TRACK");

    // bits1-2 = 10 (SECTOR)
    st.write16(DMA_MODE, 0x0084);
    st.write8(FDC_DATA, 8);
    assert_eq!(st.wd1772.read(wd1772::reg::SECTOR), 8, "bits1-2=10 -> SECTOR");

    // bits1-2 = 11 (DATA)
    st.write16(DMA_MODE, 0x0086);
    st.write8(FDC_DATA, 9);
    assert_eq!(st.wd1772.read(wd1772::reg::DATA), 9, "bits1-2=11 -> DATA");
}

#[test]
fn fdc_data_as_16_bits_correctly_reaches_wd1772() {
    // Real bug fixed: `FDC_DATA` ($FF8604) is also a 16-bit register on
    // real silicon, with the actual WD1772 byte in the LOW byte of the
    // word (confirmed by Hatari, `fdc.c`:
    // `FDC_DiskController_WriteWord`/`...ReadWord` read/write
    // `0xff8605`) — NOT the high byte that a generic byte-by-byte
    // composition would see. The TOS accesses it almost always as a full
    // word: without this interception, any command or track/sector/data
    // value written this way would be silently lost (only the high byte,
    // always zero, would reach the WD1772).
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write16(DMA_MODE, 0x0082); // TRACK (bits1-2=01)
    st.write16(FDC_DATA, 0x0055); // WORD write, as the TOS really does
    assert_eq!(st.wd1772.read(wd1772::reg::TRACK), 0x55, "expected low byte of the word, not the high byte");

    // Symmetric read: the actual byte must appear in the low byte of the
    // word read, not the high byte.
    assert_eq!(st.read16(FDC_DATA), 0x0055, "expected WD1772 byte in the low byte of the word read");
}

#[test]
fn dma_address_counter_round_trip() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x02);
    st.write8(DMA_ADDR_LOW, 0x10);
    assert_eq!(st.read8(DMA_ADDR_HIGH), 0x00);
    assert_eq!(st.read8(DMA_ADDR_MID), 0x02);
    assert_eq!(st.read8(DMA_ADDR_LOW), 0x10);
}

/// End-to-end integration test: inserts a disk, sets the sector and DMA
/// address, triggers Read Sector via the command register write, and
/// checks that RAM received the sector.
#[test]
fn read_sector_end_to_end_via_dma() {
    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(Box::new(test_disk()));
    select_drive_a_side_0(&mut st);

    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00); // DMA address = 0x1000

    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 1); // sector 1

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0
    run_disk_to_completion(&mut st);

    assert_eq!(st.read8(0x1000), 0xAB, "the sector 1 pattern must be in RAM");
    assert!(st.wd1772.interrupt_requested());
}

#[test]
fn dma_sector_counter_limits_multi_sector_transfer() {
    // Real bug fixed: bit 4 of `DMA_MODE` switches `FDC_DATA` to a separate
    // DMA sector counter (not a WD1772 register, see the doc of
    // `AtariSt::dma_sector_count`) that REALLY limits the number of sectors
    // transferred in multiple-sector read (bit M) — independently of what
    // the WD1772 would keep doing on its own (it always finds the next
    // sectors as long as they exist on the track). Without this limit, a
    // GEMDOS that only requests a few sectors (the root directory, for
    // instance) sees its transfer overflow into RAM well beyond what it
    // expects, as soon as the physical track has more — the case of
    // non-standard protected tracks, confirmed on a real commercial image
    // (`Rick_Dangerous.stx`, track 1: 12 real physical sectors versus 10
    // expected by the disk's BPB).
    let mut data = vec![0u8; 9 * SECTOR_SIZE];
    for s in 0..9usize {
        data[s * SECTOR_SIZE] = 0x10 + s as u8; // recognizable per-sector pattern
    }
    let disque = RawDiskImage::new(data, 80, 1, 9);

    let mut st = AtariSt::new(0x4000, vec![]);
    st.floppy_a = Some(Box::new(disque));
    select_drive_a_side_0(&mut st);

    // Sentinel in RAM before the transfer, to detect an overflow.
    for i in 0..(9 * SECTOR_SIZE as u32) {
        st.write8(0x1000 + i, 0xEE);
    }

    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00);

    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 1); // starting sector = 1

    // Programs the DMA sector counter to 2 (bit4 of DMA_MODE set).
    st.write8(DMA_MODE, 0x10);
    st.write8(FDC_DATA, 2);

    // Read Sector MULTIPLE (m=1, bit4 of the command): bit4 of DMA_MODE
    // dropped back to 0, so FDC_DATA becomes the command/status register
    // again.
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1001_0000);
    run_disk_to_completion(&mut st);

    assert_eq!(st.read8(0x1000), 0x10, "sector 1 must be transferred");
    assert_eq!(st.read8(0x1000 + SECTOR_SIZE as u32), 0x11, "sector 2 must be transferred");
    assert_eq!(
        st.read8(0x1000 + 2 * SECTOR_SIZE as u32),
        0xEE,
        "the DMA sector counter must stop the transfer before the 3rd sector"
    );
}

#[test]
fn disk_side_wired_from_ym2149_port_a() {
    // Core of the fixed bug: `wd1772.side` previously always stayed at 0
    // regardless of what the YM2149 port A indicated, making unreadable
    // any content located on side 1 of a double-sided floppy (the case of
    // practically all real ST software in the 720 KB `.st` format).
    let mut data = vec![0u8; 2 * 9 * SECTOR_SIZE]; // 2 sides, 9 sectors, track 0
    data[0] = 0xAA; // track 0, side 0, sector 1
    data[9 * SECTOR_SIZE] = 0xBB; // track 0, side 1, sector 1
    let disque = RawDiskImage::new(data, 80, 2, 9);

    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(Box::new(disque));
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00); // DMA address = 0x1000
    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 1); // sector 1

    // Drive A, side 0 (bit0=1): must read the side 0 pattern.
    st.write8(YM2149_SELECT, ym2149::reg::IO_PORT_A);
    st.write8(YM2149_DATA, 0b1111_1101);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0
    run_disk_to_completion(&mut st);
    assert_eq!(st.read8(0x1000), 0xAA, "expected side 0");

    // Drive A, side 1 (bit0=0): must now read the side 1 pattern.
    st.write8(DMA_ADDR_MID, 0x10); // re-arm the DMA address (advanced by the previous read)
    st.write8(DMA_ADDR_LOW, 0x00);
    st.write8(YM2149_SELECT, ym2149::reg::IO_PORT_A);
    st.write8(YM2149_DATA, 0b1111_1100);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0
    run_disk_to_completion(&mut st);
    assert_eq!(st.read8(0x1000), 0xBB, "expected side 1 after changing port A");
}

#[test]
fn drive_not_selected_responds_not_ready() {
    // If port A deselects drive A (bit1=1) — for example because
    // port A hasn't been programmed by TOS yet (default value after
    // reset, see `Ym2149::new`) — the FDC must not silently
    // use `floppy_a` anyway: it must respond NOT_READY, like a
    // real controller that sees no active drive on the bus.
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(Box::new(test_disk()));
    // Port A never programmed: stays at its default value (0xFF).

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1000_0000); // Read Sector, m=0

    assert_eq!(
        st.wd1772.read(wd1772::reg::COMMAND_STATUS) & wd1772::status::NOT_READY,
        wd1772::status::NOT_READY,
        "no drive selected -> NOT_READY, not a silent read of floppy_a"
    );
}

#[test]
fn write_sector_end_to_end_via_dma() {
    let mut st = AtariSt::new(0x2000, vec![]);
    st.floppy_a = Some(Box::new(test_disk()));
    select_drive_a_side_0(&mut st);

    // Prepares 512 bytes of 0x55 in RAM starting at 0x1000.
    for i in 0..SECTOR_SIZE as u32 {
        st.write8(0x1000 + i, 0x55);
    }
    st.write8(DMA_ADDR_HIGH, 0x00);
    st.write8(DMA_ADDR_MID, 0x10);
    st.write8(DMA_ADDR_LOW, 0x00);

    st.write8(DMA_MODE, wd1772::reg::SECTOR << 1);
    st.write8(FDC_DATA, 2); // sector 2

    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b1010_0000); // Write Sector, m=0
    run_disk_to_completion(&mut st);

    let sector_2 = st.floppy_a.as_ref().unwrap().read_sector(0, 0, 2).unwrap();
    assert!(sector_2.iter().all(|&b| b == 0x55));
}

#[test]
fn wd1772_irq_relayed_via_mfp_gpip5() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(Box::new(test_disk()));
    st.mfp.write(reg::DDR, 0x00);
    // The WD1772's `/INTRQ` is a real active-low signal wired without an
    // inverter: GPIP5 goes from 1 (idle) to 0 when /INTRQ asserts, a FALLING
    // edge (AER=0, the default value — kept explicit here for readability).
    st.mfp.write(reg::AER, 0);
    st.mfp.write(reg::IERB, 1 << channel::GPIP5);
    st.mfp.write(reg::IMRB, 1 << channel::GPIP5);

    assert_eq!(st.irq_level(), 0);
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b0000_0000); // Restore

    st.tick(4); // relays /INTRQ to GPIP5
    assert_eq!(st.irq_level(), 6, "the WD1772 IRQ must propagate up to the MFP (IPL6)");
}

#[test]
fn reset_bus_resets_wd1772() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.floppy_a = Some(Box::new(test_disk()));
    st.write8(DMA_MODE, wd1772::reg::COMMAND_STATUS << 1);
    st.write8(FDC_DATA, 0b0000_0000); // Restore -> raises /INTRQ
    assert!(st.wd1772.interrupt_requested());

    st.reset_bus();

    assert!(!st.wd1772.interrupt_requested(), "reset_bus must reset the WD1772");
}

// Full word/long write (real CPU `.W`/`.L` access): on real
// silicon, most Blitter registers ignore an isolated `.B`
// access (see `Blitter::write`) — composing the value via separate
// `write8` calls would therefore no longer have any effect since this change.
fn write_blitter_word(st: &mut AtariSt, offset: u32, value: u16) {
    st.write16(BLITTER_BASE + offset, value);
}

fn write_blitter_long(st: &mut AtariSt, offset: u32, value: u32) {
    st.write32(BLITTER_BASE + offset, value);
}

#[test]
fn blitter_registers_mapped_at_ff8a00() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(BLITTER_BASE + blitter_reg::HOP, 0b10);
    assert_eq!(st.read8(BLITTER_BASE + blitter_reg::HOP), 0b10);
    assert_eq!(st.blitter.read(blitter_reg::HOP), 0b10);
}

/// End-to-end integration test: triggers a blit (pure copy) by
/// writing the control register's BUSY bit, and checks that the board's
/// RAM was indeed modified.
#[test]
fn writing_control_triggers_the_blit_end_to_end() {
    let mut st = AtariSt::new(0x4000, vec![]);
    st.write8(BLITTER_BASE + blitter_reg::HOP, 2); // source only
    st.write8(BLITTER_BASE + blitter_reg::OP, 0x3); // copy (source)
    write_blitter_word(&mut st, blitter_reg::SRC_X_INC, 2);
    write_blitter_word(&mut st, blitter_reg::DST_X_INC, 2);
    write_blitter_word(&mut st, blitter_reg::X_COUNT, 1);
    write_blitter_word(&mut st, blitter_reg::Y_COUNT, 1);
    write_blitter_word(&mut st, blitter_reg::ENDMASK_1, 0xFFFF);
    write_blitter_word(&mut st, blitter_reg::ENDMASK_2, 0xFFFF);
    write_blitter_word(&mut st, blitter_reg::ENDMASK_3, 0xFFFF);
    write_blitter_long(&mut st, blitter_reg::SRC_ADDR, 0x1000);
    write_blitter_long(&mut st, blitter_reg::DST_ADDR, 0x2000);
    st.write16(0x1000, 0x1234);

    st.write8(BLITTER_BASE + blitter_reg::CONTROL, 0x80); // BUSY/START

    assert_eq!(st.read16(0x2000), 0x1234, "the blit must have copied the source into RAM");
    assert!(!st.blitter.busy(), "BUSY must be cleared after execution");
}

#[test]
fn reset_bus_resets_blitter() {
    let mut st = AtariSt::new(0x1000, vec![]);
    st.write8(BLITTER_BASE + blitter_reg::OP, 0xC);
    st.reset_bus();
    assert_eq!(st.blitter.read(blitter_reg::OP), 0, "reset_bus must reset the Blitter");
}

#[test]
fn ste_mmu_intra_bank_mirroring_when_memconf_overstates_real_ram() {
    // Real 1040STE: two banks of 512 KB each (1 MB total). On cold
    // reset, MEMCONF is 0 (default "128 KB" banks, see
    // `AtariSt::translate_ram_addr`) — the RAM detection software then
    // writes a candidate estimate (e.g. "2 MB" per bank) BEFORE
    // correcting it; it's precisely DURING this window that the hardware
    // mirroring must show up, so the correction can take
    // place. Reproduces the real bug found via the STe factory
    // diagnostic cartridge (it reported "2M RAM" instead of 1 MB) and verified directly
    // against Hatari (which correctly displays "1M RAM").
    let mut st = AtariSt::from_model(model::AtariModel::Ste1040.profile(), vec![]);
    assert!(st.blitter_present(), "the 1040STE has a Blitter (STE)");

    // MEMCONF = 0x0A: bank 0 = 2 MB (bits 3-2 = 10), bank 1 = 2 MB
    // (bits 1-0 = 10) — the "too large" estimate the cartridge takes
    // before correcting it.
    st.write8(MEMORY_CONF, 0x0A);

    // Writes a pattern at the very start of the real bank 0.
    st.write32(0x10, 0xA5A51234);
    // Address beyond the REAL bank 0 (512 KB) but still within its
    // current LOGICAL size (2 MB): must wrap onto the same physical
    // RAM (mirroring), not read an independent value.
    assert_eq!(
        st.read32(0x10 + 0x80000),
        0xA5A51234,
        "address beyond the real bank (512 KB) but within its logical size (2 MB): must wrap"
    );

    // Once MEMCONF is corrected to the true configuration (512 KB + 512 KB,
    // code 0x05), the mirroring disappears: the two addresses become
    // independent again (identity, normal behavior once TOS has booted).
    st.write8(MEMORY_CONF, 0x05);
    st.write32(0x10 + 0x80000, 0xDEADBEEF);
    assert_eq!(
        st.read32(0x10),
        0xA5A51234,
        "correct MEMCONF: no more mirroring, bank 1 is independent"
    );
    assert_eq!(st.read32(0x10 + 0x80000), 0xDEADBEEF);
}

/// Sets up a trivial blit (20 words, 1 per line — `X_COUNT=1`,
/// `Y_COUNT=20`), with no dependency on the source (`OP=0x0F`: output always
/// 1, so `HOP=0` ignores the source) — only progress (slicing,
/// `BUSY`) matters here, not the content written. `DST_ADDR` must
/// point into the RAM allocated by the caller.
/// HOP=0/OP=0x0F blit (constant 1, ignores the source — see `need_src` in
/// `Blitter::execute`): each word costs only 2 real bus accesses (read
/// + destination write, no source read), `y_count` lines of 1 word
/// each.
fn setup_line_blit(st: &mut AtariSt, dst_addr: u32, control: u8, y_count: u16) {
    st.write8(BLITTER_BASE + blitter_reg::HOP, 0);
    st.write8(BLITTER_BASE + blitter_reg::OP, 0x0F);
    st.write16(BLITTER_BASE + blitter_reg::DST_X_INC, 0);
    st.write16(BLITTER_BASE + blitter_reg::DST_Y_INC, 2);
    st.write16(BLITTER_BASE + blitter_reg::X_COUNT, 1);
    st.write16(BLITTER_BASE + blitter_reg::Y_COUNT, y_count);
    st.write16(BLITTER_BASE + blitter_reg::ENDMASK_1, 0xFFFF);
    st.write16(BLITTER_BASE + blitter_reg::ENDMASK_2, 0xFFFF);
    st.write16(BLITTER_BASE + blitter_reg::ENDMASK_3, 0xFFFF);
    st.write32(BLITTER_BASE + blitter_reg::DST_ADDR, dst_addr);
    // Triggers: the very first slice executes synchronously,
    // right at this write (see `AtariSt::write8`, CONTROL branch).
    st.write8(BLITTER_BASE + blitter_reg::CONTROL, control);
}

fn blit_y_count(st: &mut AtariSt) -> u16 {
    ((st.read8(BLITTER_BASE + blitter_reg::Y_COUNT) as u16) << 8)
        | st.read8(BLITTER_BASE + blitter_reg::Y_COUNT1) as u16
}

#[test]
fn non_hog_blit_progresses_in_64_bus_access_slices_at_cpu_pace() {
    // Historical limitation fixed (see the blitter module's doc): the
    // Blitter in shared mode (non-HOG) must hand control back to the CPU every
    // 64 REAL bus accesses (`BUS_ACCESSES_PER_SLICE`, value and counting
    // method taken from Hatari, `BLITTER_NONHOG_BUS_BLITTER`), not
    // execute the entire blit in one go nor approximate it via a number of words
    // processed — and resumption must be driven by `AtariSt::tick` at the
    // real CPU pace (`BLITTER_SLICE_CYCLES` = 256 cycles between two
    // slices, calibrated against Hatari's `src/blitter.c`), not by a simple
    // software rewrite of CONTROL.
    //
    // HOP=0/OP=0x0F blit (see `setup_line_blit`): each word costs 2
    // bus accesses (no source read), so 32 words fit within one
    // 64-access slice — 40 lines of 1 word force a second slice
    // (32 then 8 remaining).
    let mut st = AtariSt::new(0x1_0000, vec![]);
    setup_line_blit(&mut st, 0x2000, 0x80, 40); // BUSY=1, HOG=0

    // First slice already processed by the CONTROL write itself:
    // 32 of the 40 words (40 lines, 1 word/line, 2 bus accesses/word) are done,
    // 8 remain.
    assert_eq!(blit_y_count(&mut st), 8, "32 lines (64 bus accesses) processed by the first slice");
    assert!(st.blitter.busy(), "blit not finished: BUSY must stay observable by polling");

    // Fewer than 256 cycles elapsed: no additional slice.
    st.tick(255);
    assert_eq!(blit_y_count(&mut st), 8, "not yet enough CPU cycles for a new slice");
    assert!(st.blitter.busy());

    // The 256th cycle triggers the next slice, which finishes the blit (only
    // 8 words remain, under the budget of 32 words/64 accesses).
    st.tick(1);
    assert_eq!(blit_y_count(&mut st), 0, "blit finished after the second slice");
    assert!(!st.blitter.busy(), "BUSY drops once the blit is actually finished");
}

#[test]
fn hog_blit_finishes_in_a_single_slice() {
    // Conversely: in HOG mode (bit 6 of CONTROL), real silicon keeps
    // the bus until fully done, never handing it back to the CPU — so
    // a single call (the one triggered by the CONTROL write) must suffice,
    // regardless of the blit's size.
    let mut st = AtariSt::new(0x1_0000, vec![]);
    setup_line_blit(&mut st, 0x2000, 0xC0, 40); // BUSY=1, HOG=1

    assert_eq!(blit_y_count(&mut st), 0, "HOG mode: everything processed in a single call");
    assert!(!st.blitter.busy());
}
