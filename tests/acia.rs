#![cfg(feature = "atari-st")]
//! Unit tests for the MC6850 ACIA (`rust68::peripherals::atari_st::acia`).

use rust68::peripherals::atari_st::acia::{Acia, reg};

#[test]
fn initial_state_transmitter_ready_receiver_empty() {
    let mut acia = Acia::new();
    let status = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status & 0x01, 0, "RDRF must be 0 at reset");
    assert_eq!(status & 0x02, 0x02, "TDRE must be 1 at reset (transmitter ready)");
}

#[test]
fn reception_arms_rdrf_and_read_clears_it() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x41);
    assert_eq!(acia.read(reg::CONTROL_STATUS) & 0x01, 0x01, "RDRF armed");
    assert_eq!(acia.read(reg::DATA), 0x41);
    assert_eq!(acia.read(reg::CONTROL_STATUS) & 0x01, 0, "RDRF cleared by the read");
}

#[test]
fn overrun_if_byte_not_read_before_the_next_one() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x41);
    acia.push_rx_byte(0x42); // not read: must be lost, not queued
    let status = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status & 0x20, 0x20, "OVRN must arm");
    assert_eq!(acia.read(reg::DATA), 0x41, "the first byte remains available");

    // The read clears OVRN along with RDRF.
    let status_after = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status_after & 0x20, 0, "OVRN cleared by the read");
}

#[test]
fn byte_level_transmission() {
    let mut acia = Acia::new();
    acia.write(reg::DATA, 0x55);
    assert_eq!(acia.read(reg::CONTROL_STATUS) & 0x02, 0x02, "TDRE remains active");
    assert_eq!(acia.take_tx_byte(), Some(0x55));
    assert_eq!(acia.take_tx_byte(), None);
}

#[test]
fn reception_interrupt_gated_by_rie() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x10);
    assert!(!acia.irq_requested(), "RIE not enabled by default: no IRQ");

    acia.write(reg::CONTROL_STATUS, 0x80); // RIE alone (bit7), remains at 0
    acia.push_rx_byte(0x10);
    assert!(acia.irq_requested(), "RIE active + RDRF: IRQ requested");
}

#[test]
fn transmission_interrupt_gated_by_tie() {
    let mut acia = Acia::new();
    // TDRE is active right from reset, but TIE (bits 6-5 = 01) is not.
    assert!(!acia.irq_requested());

    acia.write(reg::CONTROL_STATUS, 0b0010_0000); // bits6-5 = 01: TIE active
    assert!(acia.irq_requested(), "TIE active + TDRE: IRQ requested");

    acia.write(reg::CONTROL_STATUS, 0b0100_0000); // bits6-5 = 10: TIE inactive
    assert!(!acia.irq_requested());
}

#[test]
fn master_reset_clears_the_status_flags() {
    let mut acia = Acia::new();
    acia.push_rx_byte(0x10);
    acia.push_rx_byte(0x20); // overrun
    assert_ne!(acia.read(reg::CONTROL_STATUS) & 0x21, 0);

    acia.write(reg::CONTROL_STATUS, 0x03); // bits0-1 = 11: master reset
    let status = acia.read(reg::CONTROL_STATUS);
    assert_eq!(status & 0x01, 0, "RDRF cleared by master reset");
    assert_eq!(status & 0x20, 0, "OVRN cleared by master reset");
    assert_eq!(status & 0x02, 0x02, "TDRE re-armed by master reset");
}
