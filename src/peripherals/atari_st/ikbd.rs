//! Emulation of the IKBD (HD6301) keyboard/mouse/joystick controller.
//!
//! On real ST/STE, the HD6301 talks to the CPU via the keyboard ACIA
//! (`AtariSt::acia_keyboard`) at 7812.5 baud. Protocol (outgoing, IKBD →
//! CPU):
//! - Key press: `0xNN` (scancode 0x01-0x72)
//! - Key release: `0x80 | 0xNN`
//! - Relative mouse movement (default mode, `$08`): `0xF8|buttons,
//!   dx, dy` — sent automatically on every movement.
//! - Absolute mouse position (mode `$09`, see [`Ikbd::mouse_mode_absolute`]):
//!   `0xF7, buttons, xmsb, xlsb, ymsb, ylsb` — **NOT sent automatically
//!   on movement** (real silicon behavior, confirmed against
//!   Hatari): only in response to `$0D`, or on button press/release
//!   if `$07` requested it.
//!
//! Commands (incoming, CPU → IKBD via the ACIA transmitter): `0x80 0x01`
//! (reset), `0x07` (mouse action), `0x08`/`0x09` (relative/absolute mode),
//! `0x0D` (interrogate absolute position), `0x0E` (load internal
//! position), etc. — see [`Ikbd::receive_cmd`].
//!
//! This module does not model the joystick (Rust68 currently has no
//! gamepad frontend): the related commands are recognized and consume
//! the correct number of parameter bytes (so as not to desynchronize
//! the subsequent command stream), but have no effect.

use std::collections::VecDeque;

/// CPU cycles (68000 at 8 MHz) between a reset (power-on or the
/// `0x80 0x01` command) and the actual arrival of the IKBD's `0xF1`
/// self-test response.
///
/// Delivering `0xF1` synchronously (immediately on reset) makes the
/// byte arrive before the TOS has finished configuring the MFP's
/// IERB/IMRB: the corresponding ACIA interrupt, pending but still
/// masked at that point, is then silently cleared by the TOS's
/// subsequent (normal) write to IERB that enables the ACIA channel.
/// Since the byte is therefore never read, the ACIA's `RDRF` stays
/// permanently full, blocking every following byte (keyboard AND
/// mouse) behind it forever — no IKBD interrupt can ever arrive
/// again.
///
/// Value chosen empirically (see the companion project Stay, which
/// isolated and fixed exactly this regression): large enough to fall
/// due after the TOS has finished the IERB/IMRB setup of its
/// keyboard/mouse initialization.
const IKBD_RESET_CYCLES: u32 = 5_000_000;

/// Full state of an emulated HD6301 IKBD.
pub struct Ikbd {
    /// Output queue: bytes waiting to be delivered to the ACIA (RX, CPU side).
    tx_queue: VecDeque<u8>,
    /// Incoming command buffer (CPU → IKBD via the ACIA transmitter).
    cmd_buf: Vec<u8>,
    /// Number of parameter bytes still expected before executing the current command.
    cmd_remaining: usize,

    // Mouse state.
    mouse_x: i32,
    mouse_y: i32,
    mouse_buttons: u8,
    /// Direction of the Y axis: 1 = origin at the top (downward = positive), -1 = origin at the bottom.
    y_axis: i8,
    /// `true` if the mouse is in ABSOLUTE mode (`$09`), `false` = relative
    /// (`$08`, default) — see the doc of [`Self::mouse_move`].
    /// **Real bug fixed**: `$09` was recognized (correct number of
    /// parameter bytes consumed) but completely ignored — the mouse
    /// remained in relative mode forever, sending `0xF8` packets that
    /// GEM, once switched to absolute mode for a modal dialog box
    /// (e.g. Desktop > Info), no longer expects at all — desynchronizing
    /// its serial stream parser and producing apparently "rotated"
    /// cursor movement (the `dx`/`dy` bytes misinterpreted) for as long
    /// as the dialog box stays open. Confirmed against Hatari
    /// (`ikbd.c`, `IKBD_Cmd_AbsMouseMode`/`IKBD_SendAutoKeyboardCommands`:
    /// in absolute mode, real silicon NEVER sends an automatic packet
    /// on movement — only on `$0D` interrogation, or on button
    /// press/release if `$07` requested it).
    mouse_mode_absolute: bool,
    /// Current bounds (`$09`, MSB first) of absolute mode — also
    /// used as clamping bounds for [`Self::mouse_x`]/`mouse_y` at ALL
    /// times (real silicon: a single internal position tracked
    /// continuously, clamped by these limits, regardless of the
    /// currently active reporting mode — see Hatari,
    /// `IKBD_UpdateInternalMousePosition`). Default values unchanged
    /// from historical behavior (639/399) as long as no `$09` command
    /// has been received yet.
    abs_max_x: u16,
    abs_max_y: u16,
    /// Last parameter byte of the `$07` ("mouse action") command —
    /// bits 0-1: report absolute position on button press/release
    /// (the only AUTOMATIC reporting mechanism in absolute mode, see
    /// [`Self::mouse_mode_absolute`]).
    mouse_action: u8,

    /// Cycles remaining before delivering a pending reset `0xF1`
    /// response (see [`IKBD_RESET_CYCLES`]). `None` = no reset in progress.
    reset_pending_cycles: Option<u32>,
    /// Keyboard/mouse bytes that occurred during an ongoing reset (see
    /// [`Self::reset_pending_cycles`]), held back to be delivered
    /// right after `0xF1` rather than before. On real HD6301
    /// hardware, the controller runs its self-test and does not scan
    /// the keyboard during this delay; delivering a scancode before
    /// `0xF1` makes host software (e.g. the diagnostic cartridge,
    /// test K1) believe the keyboard is not responding correctly, and
    /// causes it to fall back to RS232 mode ("keyboard dead").
    pending_during_reset: VecDeque<u8>,
}

impl Ikbd {
    pub fn new() -> Self {
        Ikbd {
            tx_queue: VecDeque::new(),
            cmd_buf: Vec::new(),
            cmd_remaining: 0,
            mouse_x: 0,
            mouse_y: 0,
            mouse_buttons: 0,
            y_axis: 1,
            mouse_mode_absolute: false,
            abs_max_x: 639,
            abs_max_y: 399,
            mouse_action: 0,
            // Power-on self-test: deferred like a software reset (see
            // the doc of IKBD_RESET_CYCLES), not available from cycle 0.
            reset_pending_cycles: Some(IKBD_RESET_CYCLES),
            pending_during_reset: VecDeque::new(),
        }
    }

    /// Advances the reset response delay, if one is in progress. To be
    /// called once per bus tick with the number of elapsed cycles,
    /// before [`Self::pop_tx`].
    pub fn tick(&mut self, cycles: u32) {
        if let Some(remaining) = self.reset_pending_cycles {
            if cycles >= remaining {
                self.reset_pending_cycles = None;
                self.tx_queue.push_back(0xF1);
                self.tx_queue.extend(self.pending_during_reset.drain(..));
            } else {
                self.reset_pending_cycles = Some(remaining - cycles);
            }
        }
    }

    /// Removes the next byte to inject into the ACIA (RX), if there is one.
    pub fn pop_tx(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }

    /// Receives a command byte sent by the CPU (via the keyboard
    /// ACIA's transmitter).
    pub fn receive_cmd(&mut self, byte: u8) {
        if self.cmd_remaining > 0 {
            self.cmd_buf.push(byte);
            self.cmd_remaining -= 1;
            if self.cmd_remaining == 0 {
                self.execute_cmd();
            }
            return;
        }

        self.cmd_buf.clear();
        self.cmd_buf.push(byte);

        match byte {
            0x80 => self.cmd_remaining = 1, // reset: expects parameter 0x01
            0x07 => self.cmd_remaining = 1, // mouse button action
            0x08 => {}                       // relative mouse mode (no parameter)
            0x09 => self.cmd_remaining = 4, // absolute mouse mode
            0x0A => self.cmd_remaining = 2, // mouse keycodes
            0x0B => self.cmd_remaining = 2, // mouse threshold
            0x0C => self.cmd_remaining = 2, // mouse scale
            0x0D => {}                       // interrogate absolute position (responds directly)
            0x0E => self.cmd_remaining = 5, // set internal position
            0x0F => self.y_axis = -1,       // Y=0 at bottom
            0x10 => self.y_axis = 1,        // Y=0 at top
            0x11 => {}                       // start keyboard transmission
            0x12 => {}                       // mouse disabled
            0x13 => {}                       // stop keyboard transmission
            // 0x14-0x1A: joystick commands — not modeled (no gamepad
            // frontend), but the parameter byte count must remain
            // correct so as not to desynchronize the stream.
            0x14 | 0x15 | 0x16 | 0x18 | 0x1A => {}
            0x17 => self.cmd_remaining = 1,
            0x19 => self.cmd_remaining = 6,
            0x1B => self.cmd_remaining = 6, // set the clock
            0x1C => {}                       // read the clock
            0x20 => self.cmd_remaining = 3, // load into memory
            0x21 => self.cmd_remaining = 2, // read memory
            0x22 => self.cmd_remaining = 2, // execute
            _ => {}
        }

        if self.cmd_remaining == 0 {
            self.execute_cmd();
        }
    }

    fn execute_cmd(&mut self) {
        match self.cmd_buf[0] {
            0x80 => {
                // Software reset: command 0x80 + parameter 0x01.
                if self.cmd_buf.get(1) == Some(&0x01) {
                    self.mouse_buttons = 0;
                    self.y_axis = 1;
                    self.mouse_mode_absolute = false;
                    self.reset_pending_cycles = Some(IKBD_RESET_CYCLES);
                }
            }
            // Mouse action ($07): bits 0-1 = report absolute position
            // on button press/release (the ONLY automatic reporting
            // mechanism in absolute mode, see
            // `Self::mouse_mode_absolute`) — has no effect in relative
            // mode (already reported on every movement).
            0x07 => self.mouse_action = self.cmd_buf.get(1).copied().unwrap_or(0),
            // Relative mode ($08): no parameter.
            0x08 => self.mouse_mode_absolute = false,
            // Absolute mode ($09): MaxX/MaxY bounds, MSB first — see
            // the doc of `Self::mouse_mode_absolute`. Does NOT touch
            // `mouse_x`/`mouse_y` themselves (real silicon: the
            // clamping only applies to the NEXT movement, not
            // retroactively).
            0x09 => {
                self.mouse_mode_absolute = true;
                self.abs_max_x = ((self.cmd_buf[1] as u16) << 8) | self.cmd_buf[2] as u16;
                self.abs_max_y = ((self.cmd_buf[3] as u16) << 8) | self.cmd_buf[4] as u16;
            }
            // Interrogate absolute position → 0xF7 + buttons + x(2) + y(2).
            0x0D => {
                let bytes = self.abs_report_bytes();
                self.tx_queue.extend(bytes);
            }
            // Load internal position ($0E): filler byte + X(2)/Y(2),
            // MSB first — GEM typically uses this right after $09 to
            // recenter the cursor within the dialog box's bounds
            // before the user moves the mouse.
            0x0E => {
                let x = ((self.cmd_buf[2] as u16) << 8) | self.cmd_buf[3] as u16;
                let y = ((self.cmd_buf[4] as u16) << 8) | self.cmd_buf[5] as u16;
                self.mouse_x = x as i32;
                self.mouse_y = y as i32;
            }
            _ => {}
        }
        self.cmd_buf.clear();
    }

    /// The 6 bytes of the absolute position report (`0xF7` + buttons +
    /// x(2) + y(2), MSB first) — shared between the `$0D` response and
    /// the automatic on-button report in absolute mode (see
    /// [`Self::mouse_move`]).
    fn abs_report_bytes(&self) -> [u8; 6] {
        let x = self.mouse_x as u16;
        let y = self.mouse_y as u16;
        [0xF7, self.mouse_buttons, (x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8]
    }

    // ── Events coming from the host ──────────────────────────────────────

    /// Signals a key press (make). `scancode` is the Atari IKBD scancode.
    pub fn key_make(&mut self, scancode: u8) {
        self.push_output(scancode);
    }

    /// Signals a key release (break). Code = `0x80 | make`.
    pub fn key_break(&mut self, scancode: u8) {
        self.push_output(0x80 | scancode);
    }

    /// Routes a keyboard/mouse byte to `tx_queue`, or to
    /// `pending_during_reset` if a reset is in progress (see the doc of
    /// that field) so that it never arrives before the `0xF1` self-test.
    fn push_output(&mut self, byte: u8) {
        if self.reset_pending_cycles.is_some() {
            self.pending_during_reset.push_back(byte);
        } else {
            self.tx_queue.push_back(byte);
        }
    }

    /// Signals a relative mouse movement and the button state.
    pub fn mouse_move(&mut self, dx: i8, dy: i8, buttons: u8) {
        let buttons_changed = buttons != self.mouse_buttons;
        self.mouse_buttons = buttons;
        let eff_dy = if self.y_axis < 0 { dy.wrapping_neg() } else { dy };
        // Internal position tracked at ALL times, clamped by
        // `abs_max_x`/`_y` — real silicon, regardless of the currently
        // active reporting mode (see the doc of `Self::abs_max_x`).
        self.mouse_x = (self.mouse_x + dx as i32).clamp(0, self.abs_max_x as i32);
        self.mouse_y = (self.mouse_y + eff_dy as i32).clamp(0, self.abs_max_y as i32);
        if self.mouse_mode_absolute {
            // Absolute mode: NO automatic report on movement (real
            // silicon) — only on button press/release if `$07`
            // requested it (bits 0-1), everything else goes through an
            // explicit `$0D` interrogation. See the doc of
            // `Self::mouse_mode_absolute`.
            if buttons_changed && self.mouse_action & 0x03 != 0 {
                let bytes = self.abs_report_bytes();
                for b in bytes {
                    self.push_output(b);
                }
            }
            return;
        }
        if dx == 0 && dy == 0 && !buttons_changed {
            return;
        }
        self.push_output(0xF8 | (buttons & 0x03));
        self.push_output(dx as u8);
        self.push_output(eff_dy as u8);
    }
}

impl Default for Ikbd {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(ikbd: &mut Ikbd) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(b) = ikbd.pop_tx() {
            out.push(b);
        }
        out
    }

    #[test]
    fn reset_response_is_deferred_not_immediate() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES - 1);
        assert!(drain(&mut ikbd).is_empty(), "0xF1 must not arrive before the full delay");
        ikbd.tick(1);
        assert_eq!(drain(&mut ikbd), vec![0xF1]);
    }

    #[test]
    fn key_pressed_during_reset_arrives_after_0xf1_not_before() {
        // Reproduces the diagnostic cartridge scenario (test K1): a key
        // pressed during the reset window must never precede the
        // `0xF1` self-test byte, or it will make the test believe the
        // keyboard is not responding (RS232 "keyboard dead" fallback).
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES / 2);
        ikbd.key_make(0x1E); // 'A' key pressed right in the middle of the reset
        assert!(
            drain(&mut ikbd).is_empty(),
            "the scancode must not be delivered before the end of the reset"
        );
        ikbd.tick(IKBD_RESET_CYCLES / 2);
        assert_eq!(drain(&mut ikbd), vec![0xF1, 0x1E]);
    }

    #[test]
    fn reset_command_restarts_the_delay() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);

        ikbd.receive_cmd(0x80);
        ikbd.receive_cmd(0x01);
        assert!(drain(&mut ikbd).is_empty(), "0xF1 must again be deferred after a software reset");
        ikbd.tick(IKBD_RESET_CYCLES);
        assert_eq!(drain(&mut ikbd), vec![0xF1]);
    }

    #[test]
    fn relative_movement_packet_standard_format() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        ikbd.mouse_move(5, -3, 0b01);
        assert_eq!(drain(&mut ikbd), vec![0xF9, 5, (-3i8) as u8]);
    }

    #[test]
    fn no_packet_if_nothing_changes() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        ikbd.mouse_move(0, 0, 0);
        assert!(drain(&mut ikbd).is_empty());
    }

    #[test]
    fn y_axis_inverted_by_command_0x0f() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0F); // Y=0 at bottom
        ikbd.mouse_move(0, 10, 0);
        assert_eq!(drain(&mut ikbd), vec![0xF8, 0, (-10i8) as u8]);
    }

    #[test]
    fn absolute_position_interrogation_0x0d() {
        let mut ikbd = Ikbd::new();
        ikbd.mouse_move(100, 50, 0b11);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0b11, 0x00, 100, 0x00, 50]);
    }

    // --- Absolute mode ($09), real bug fixed -------------------------------

    fn send_cmd(ikbd: &mut Ikbd, bytes: &[u8]) {
        for &b in bytes {
            ikbd.receive_cmd(b);
        }
    }

    #[test]
    fn absolute_mode_sends_no_automatic_packet_on_movement() {
        // Core of the fixed bug: real silicon, in absolute mode, NEVER
        // sends an automatic packet on movement (neither relative
        // `0xF8` nor absolute `0xF7`) — only on `$0D` interrogation or
        // on button if `$07` requested it. A regression reintroducing
        // an automatic send here would recreate exactly the GEM bug
        // ("rotated" cursor while a modal dialog box is open).
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]); // max_x=0x031F, max_y=0x018F
        drain(&mut ikbd); // the command itself yields no response

        ikbd.mouse_move(5, -3, 0b01);
        assert!(drain(&mut ikbd).is_empty(), "no automatic packet in absolute mode");
    }

    #[test]
    fn absolute_mode_still_tracks_position_queryable_via_0x0d() {
        let mut ikbd = Ikbd::new();
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]);
        drain(&mut ikbd);

        ikbd.mouse_move(10, 20, 0);
        ikbd.mouse_move(5, 5, 0);
        drain(&mut ikbd);

        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 15, 0x00, 25]);
    }

    #[test]
    fn absolute_mode_clamps_position_to_max_x_max_y() {
        let mut ikbd = Ikbd::new();
        send_cmd(&mut ikbd, &[0x09, 0x00, 0x0A, 0x00, 0x05]); // max_x=10, max_y=5
        drain(&mut ikbd);

        ikbd.mouse_move(100, 100, 0);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 10, 0x00, 5], "clamped to max_x/max_y, not 639/399");
    }

    #[test]
    fn absolute_mode_reports_automatically_on_button_if_0x07_requested_it() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES); // the automatic report goes through push_output, gated during a reset
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]);
        send_cmd(&mut ikbd, &[0x07, 0x03]); // mouse action: bits 0-1 set
        drain(&mut ikbd);

        ikbd.mouse_move(0, 0, 0b01); // button change, no movement
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0b01, 0x00, 0, 0x00, 0]);
    }

    #[test]
    fn absolute_mode_without_0x07_action_reports_nothing_on_button() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]); // no $07
        drain(&mut ikbd);

        ikbd.mouse_move(0, 0, 0b01);
        assert!(drain(&mut ikbd).is_empty());
    }

    #[test]
    fn returning_to_relative_mode_via_0x08_restores_automatic_packets() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        send_cmd(&mut ikbd, &[0x09, 0x03, 0x1F, 0x01, 0x8F]);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x08); // back to relative mode
        drain(&mut ikbd);

        ikbd.mouse_move(5, -3, 0b01);
        assert_eq!(drain(&mut ikbd), vec![0xF9, 5, (-3i8) as u8], "relative mode restored");
    }

    #[test]
    fn command_0x0e_loads_internal_position_directly() {
        let mut ikbd = Ikbd::new();
        send_cmd(&mut ikbd, &[0x0E, 0x00, 0x00, 0x64, 0x00, 0x32]); // x=100, y=50
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 100, 0x00, 50]);
    }

    #[test]
    fn joystick_command_with_parameters_does_not_desync_the_rest() {
        let mut ikbd = Ikbd::new();
        ikbd.tick(IKBD_RESET_CYCLES);
        drain(&mut ikbd);
        // 0x19 expects 6 parameter bytes (joystick cursor) — not
        // modeled, but must be properly absorbed so that the next
        // command (absolute position interrogation) is read correctly.
        ikbd.receive_cmd(0x19);
        for b in 0..6 {
            ikbd.receive_cmd(b);
        }
        ikbd.mouse_move(1, 1, 0);
        drain(&mut ikbd);
        ikbd.receive_cmd(0x0D);
        assert_eq!(drain(&mut ikbd), vec![0xF7, 0, 0x00, 1, 0x00, 1]);
    }
}
