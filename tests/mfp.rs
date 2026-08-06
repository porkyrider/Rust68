#![cfg(feature = "atari-st")]
//! Unit tests for the MC68901 MFP (`rust68::peripherals::atari_st::mfp`).
//!
//! No TomHarte suite exists for this peripheral (Atari ST specific,
//! not part of the 68k family itself): this file is the only safety
//! net, on the same principle as `tests/instructions.rs`.

use rust68::peripherals::atari_st::mfp::{Mfp, channel, reg};

#[test]
fn gpip_read_reflects_ddr() {
    let mut mfp = Mfp::new();
    // DDR=0x0F: P0-P3 as output, P4-P7 as input.
    mfp.write(reg::DDR, 0x0F);
    mfp.write(reg::GPIP, 0b0000_0101); // output latch for P0-P3
    mfp.set_gpip_input(4, true); // P4 (input) set to 1

    let gpip = mfp.read(reg::GPIP);
    assert_eq!(gpip & 0x0F, 0b0101, "output bits reflect the written latch");
    assert_eq!(gpip & 0x10, 0x10, "P4 (input) reflects the applied level");
}

#[test]
fn gpip_rising_or_falling_edge_depending_on_aer() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00); // all inputs
    mfp.write(reg::IERB, 1 << channel::GPIP0); // enable channel GPIP0
    mfp.write(reg::IMRB, 1 << channel::GPIP0);

    // AER bit0 = 0: triggers on FALLING edge.
    mfp.write(reg::AER, 0x00);
    mfp.set_gpip_input(0, true); // rising: should trigger nothing
    assert!(!mfp.interrupt_requested());
    mfp.set_gpip_input(0, false); // falling: should trigger
    assert!(mfp.interrupt_requested());

    // Acknowledge, then check the opposite sense with AER bit0 = 1.
    mfp.iack();
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::IERB, 1 << channel::GPIP0);
    mfp.write(reg::IMRB, 1 << channel::GPIP0);
    mfp.write(reg::AER, 0x01); // rising edge
    mfp.set_gpip_input(0, true);
    assert!(mfp.interrupt_requested());
}

#[test]
fn interrupt_masked_without_ier_or_imr() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    // IER not enabled: the edge must not even arm IPR.
    mfp.set_gpip_input(0, true);
    assert!(!mfp.interrupt_requested());
    assert_eq!(mfp.read(reg::IPRB) & 1, 0);

    // IER enabled but IMR not enabled: IPR arms, but no visible request.
    mfp.write(reg::IERB, 0x01);
    mfp.set_gpip_input(0, false);
    mfp.set_gpip_input(0, true);
    assert_eq!(mfp.read(reg::IPRB) & 1, 1, "IPR arms as soon as IER is active");
    assert!(
        !mfp.interrupt_requested(),
        "IMR not enabled: no request toward the CPU"
    );
}

#[test]
fn ipr_isr_clear_only_by_writing_zero() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    mfp.write(reg::IERB, 0x01);
    mfp.set_gpip_input(0, true);
    assert_eq!(mfp.read(reg::IPRB) & 1, 1);

    // Writing 1 must not re-arm / has no effect (bit already 1).
    mfp.write(reg::IPRB, 0xFF);
    assert_eq!(mfp.read(reg::IPRB) & 1, 1, "writing 1 does not affect the bit");

    // Writing 0 clears it.
    mfp.write(reg::IPRB, 0xFE);
    assert_eq!(mfp.read(reg::IPRB) & 1, 0, "writing 0 clears the bit");
}

#[test]
fn iack_computes_vector_and_arms_isr_in_both_eoi_modes() {
    // Bit S (VR bit 3) — counter-intuitive meaning, confirmed against Hatari
    // (`mfp.c`, `MFP_ProcessIACK`): bit SET = SEI ("software
    // end-of-interrupt", ISR stays armed until cleared by software), bit
    // CLEAR = AUTOMATIC EOI (the silicon arms THEN clears ISR in the SAME
    // IACK cycle — never observable as set). See the docs on `Mfp::iack`.
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    mfp.write(reg::IERB, 1 << channel::GPIP0);
    mfp.write(reg::IMRB, 1 << channel::GPIP0);
    mfp.write(reg::VR, 0x40); // vector base 0x40 (bits 7-3), bit3=0: automatic EOI
    mfp.set_gpip_input(0, true);

    let vector = mfp.iack();
    assert_eq!(vector, 0x40, "vector = VR[7:4] | channel (channel 0 here)");
    assert_eq!(mfp.read(reg::ISRB) & 1, 0, "automatic EOI: ISR never observable as set after the IACK");
    assert_eq!(mfp.read(reg::IPRB) & 1, 0, "IPR is cleared by the IACK");

    // SEI mode (bit 3 set): ISR arms and STAYS set after the IACK, until
    // explicit software clearing — this is the mode used by the STe
    // factory diagnostic cartridge (test "T0 MFP timer", VR=0x48):
    // its shared interrupt handler (a single routine for the 4
    // Timer A/B/C/D vectors) tells which timer triggered by reading
    // precisely these ISR bits before clearing them itself.
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    mfp.write(reg::IERB, 1 << channel::GPIP0);
    mfp.write(reg::IMRB, 1 << channel::GPIP0);
    mfp.write(reg::VR, 0x48); // bit3 set: SEI
    mfp.set_gpip_input(0, true);
    mfp.iack();
    assert_eq!(mfp.read(reg::ISRB) & 1, 1, "SEI: ISR stays armed after the IACK");
}

#[test]
fn iack_excludes_s_bit_auto_eoi_from_vector() {
    // VR bit 3 (S, SEI/software-EOI when set — see the docs on
    // `Mfp::iack`) is a separate control bit, NOT the high-order bit
    // of the channel number: only VR[7:4] form the vector base, the 4
    // channel bits (0-15) occupy the entire low part. A real TOS programs
    // VR=0x48 (base 0x40, SEI active) then installs its handlers at
    // `0x100 + channel*4` (vector `0x40 | channel`) — confusing
    // the S bit with the channel's high bit would compute `0x48 | channel`
    // instead, an offset of 8 vectors (32 bytes) which vectors to
    // a handler that was never installed (a real bug encountered while
    // booting a real TOS: Timer C, VR=0x48, vectored to `$134`
    // instead of `$114` where TOS had actually installed its
    // handler).
    let mut mfp = Mfp::new();
    mfp.write(reg::IERB, 1 << channel::TIMER_C);
    mfp.write(reg::IMRB, 1 << channel::TIMER_C);
    mfp.write(reg::VR, 0x48);
    // Timer C in delay mode, prescaler ÷4 (control=1), data=1: counts down
    // on the very first tick and triggers `request()` internally. The data
    // must be written BEFORE the control: `reload()` (triggered by
    // the write to TCDCR) reloads the counter from the data.
    mfp.write(reg::TCDR, 1);
    mfp.write(reg::TCDCR, 0x01 << 4);
    mfp.tick(100);

    let vector = mfp.iack();
    assert_eq!(
        vector,
        0x40 | channel::TIMER_C,
        "VR=0x48 with S=1: vector = VR[7:4](0x40) | channel, not VR[7:3](0x48) | channel"
    );
}

#[test]
fn priority_of_highest_channel() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0xFF); // all rising edges
    mfp.write(reg::IERB, (1 << channel::GPIP0) | (1 << channel::GPIP2));
    mfp.write(reg::IMRB, (1 << channel::GPIP0) | (1 << channel::GPIP2));
    mfp.set_gpip_input(0, true);
    mfp.set_gpip_input(2, true);

    // Channel 2 (GPIP2) > channel 0 (GPIP0): must be acknowledged first.
    let vector = mfp.iack();
    assert_eq!(vector & 0x07, channel::GPIP2);
    let vector2 = mfp.iack();
    assert_eq!(vector2 & 0x07, channel::GPIP0);
}

#[test]
fn a_lower_priority_isr_does_not_block_a_higher_channel() {
    // Channel 5 (GPIP5, low priority) already "in service": a new channel
    // of STRICTLY higher priority (TIMER_A=13) must still be able
    // to request service — this is preemption of a lower-priority
    // ISR by a higher channel, the real MC68901 behavior
    // (see the docs on `Mfp::highest_priority_pending`).
    let mut mfp = Mfp::new();
    // SEI mode (VR bit 3 set): ISR stays armed after the IACK until
    // cleared by software — needed here to observe the preemption (in
    // automatic EOI, ISR never stays set, see the docs on
    // `Mfp::iack`).
    mfp.write(reg::VR, 0x08);
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0xFF);
    mfp.write(reg::IERB, 1 << channel::GPIP5);
    mfp.write(reg::IMRB, 1 << channel::GPIP5);
    mfp.set_gpip_input(5, true);
    let v = mfp.iack(); // GPIP5 becomes "in service" (ISR armed, IPR cleared)
    assert_eq!(v & 0x0F, channel::GPIP5);
    assert_eq!(mfp.read(reg::ISRB) & (1 << channel::GPIP5), 1 << channel::GPIP5);

    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 1);
    mfp.write(reg::TACR, 1); // starts, ÷4
    mfp.tick(1000); // comfortably enough to trigger at least once

    assert!(
        mfp.interrupt_requested(),
        "TIMER_A (channel 13) must preempt GPIP5 (channel 7) already in-service"
    );
    let v2 = mfp.iack();
    assert_eq!(v2 & 0x0F, channel::TIMER_A, "TIMER_A must be the acknowledged channel");
}

#[test]
fn a_higher_priority_isr_blocks_a_lower_channel() {
    // Mirror of the previous test: channel 13 (TIMER_A) in service, a
    // new channel 5 (GPIP2, lower priority) pending+unmasked must NOT
    // generate a request until TIMER_A is acknowledged.
    let mut mfp = Mfp::new();
    // SEI mode (VR bit 3 set), same reason as the previous test: ISR
    // must stay set after the IACK to observe the blocking.
    mfp.write(reg::VR, 0x08);
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 1);
    mfp.write(reg::TACR, 1);
    mfp.tick(1000);
    assert!(mfp.interrupt_requested());
    let v = mfp.iack();
    assert_eq!(v & 0x0F, channel::TIMER_A);
    assert_eq!(
        mfp.read(reg::ISRA) & (1 << (channel::TIMER_A - 8)),
        1 << (channel::TIMER_A - 8)
    );

    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0xFF);
    mfp.write(reg::IERB, 1 << channel::GPIP2);
    mfp.write(reg::IMRB, 1 << channel::GPIP2);
    mfp.set_gpip_input(2, true);

    assert_eq!(mfp.read(reg::IPRB) & (1 << channel::GPIP2), 1 << channel::GPIP2, "GPIP2 stays pending");
    assert!(
        !mfp.interrupt_requested(),
        "GPIP2 (channel 2) must not request service while TIMER_A (channel 13) is in-service"
    );

    // Acknowledging TIMER_A (SEI, explicitly armed mode above) clears the
    // way for GPIP2.
    mfp.write(reg::ISRA, 0x00);
    assert!(
        mfp.interrupt_requested(),
        "TIMER_A acknowledged: GPIP2 becomes eligible again"
    );
    let v2 = mfp.iack();
    assert_eq!(v2 & 0x0F, channel::GPIP2);
}

#[test]
fn timer_a_delay_mode_triggers_and_reloads() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 5); // reload value
    mfp.write(reg::TACR, 1); // starts in delay mode, ÷4

    // Reading while the timer runs: returns the current count, not 5.
    assert_eq!(mfp.read(reg::TADR), 5);

    // Clearly insufficient budget: not triggered yet.
    mfp.tick(10);
    assert!(
        !mfp.interrupt_requested(),
        "10 CPU cycles are not enough to exhaust data=5 at ÷4"
    );

    // Generous budget (well above (5+1)*4 MFP cycles converted
    // to CPU cycles): must have triggered at least once.
    for _ in 0..30 {
        mfp.tick(10);
    }
    assert!(
        mfp.interrupt_requested(),
        "generous budget: timer A must have triggered"
    );
    assert_eq!(mfp.read(reg::IPRA) & (1 << (channel::TIMER_A - 8)), 1 << (channel::TIMER_A - 8));
}

#[test]
fn stopped_timer_never_triggers() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 1); // minimal period
    mfp.write(reg::TACR, 0); // stopped

    for _ in 0..1000 {
        mfp.tick(100);
    }
    assert!(!mfp.interrupt_requested(), "stopped timer (control=0): never triggers");
    assert_eq!(mfp.read(reg::TADR), 1, "reading while stopped = reload value as-is");
}

#[test]
fn timer_a_event_count_ignores_tick_and_reacts_to_pulse() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 3);
    mfp.write(reg::TACR, 0x08); // event-count mode

    // tick() must never advance a timer in event-count mode.
    for _ in 0..10_000 {
        mfp.tick(1000);
    }
    assert!(!mfp.interrupt_requested(), "event-count: tick() must not decrement anything");

    // pulse_ta() must decrement: period = data = 3 pulses (same counter
    // as delay mode, see the docs on `Timer::decrement` — the count
    // displayed right after reload is already `data`, so the
    // trigger happens on the `data`-th decrement, not the `data+1`-th).
    for _ in 0..2 {
        mfp.pulse_ta();
        assert!(!mfp.interrupt_requested());
    }
    mfp.pulse_ta();
    assert!(mfp.interrupt_requested(), "3rd pulse: trigger expected");
}

#[test]
fn timer_c_and_d_share_tcdcr() {
    let mut mfp = Mfp::new();
    mfp.write(reg::TCDCR, (0x03 << 4) | 0x02); // Timer C = ÷16, Timer D = ÷10
    assert_eq!(mfp.read(reg::TCDCR), (0x03 << 4) | 0x02);
}

#[test]
fn usart_byte_level_reception_and_transmission() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::RX_FULL - 8));
    mfp.write(reg::IMRA, 1 << (channel::RX_FULL - 8));

    mfp.push_rx_byte(0x41);
    assert!(mfp.interrupt_requested(), "byte received: RX_FULL must arm");
    assert_eq!(mfp.read(reg::RSR) & 0x80, 0x80, "buffer full");
    assert_eq!(mfp.read(reg::UDR), 0x41, "reading UDR returns the received byte");
    assert_eq!(mfp.read(reg::RSR) & 0x80, 0, "reading UDR clears buffer full");

    // Transmission: writing UDR must be retrievable via take_tx_byte.
    mfp.write(reg::UDR, 0x42);
    assert_eq!(mfp.take_tx_byte(), Some(0x42));
    assert_eq!(mfp.take_tx_byte(), None);
    assert_eq!(mfp.read(reg::TSR) & 0x80, 0x80, "buffer empty after transmission");
}

#[test]
fn tsr_buffer_empty_is_1_at_reset_and_survives_transmitter_enable() {
    // Reproduces the standard USART initialization sequence (e.g.
    // STe factory diagnostic cartridge): TSR starts at "buffer empty" by
    // default (no byte pending at reset), and enabling it via bit0
    // (Transmitter Enable) must not clear this hardware status bit —
    // otherwise no transmission could ever take place afterward.
    let mut mfp = Mfp::new();
    assert_eq!(mfp.read(reg::TSR) & 0x80, 0x80, "buffer empty right at reset");

    mfp.write(reg::TSR, 0x01); // enables the transmitter (bit0), as a real driver would
    assert_eq!(
        mfp.read(reg::TSR) & 0x80,
        0x80,
        "buffer empty must survive transmitter activation"
    );
    assert_eq!(mfp.read(reg::TSR) & 0x01, 0x01, "enable bit correctly taken into account");

    mfp.write(reg::UDR, 0x55);
    assert_eq!(mfp.take_tx_byte(), Some(0x55), "transmission must work after activation");
}
