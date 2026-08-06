#![cfg(feature = "atari-st")]
//! Unit tests for the GLUE (`rust68::peripherals::atari_st::glue`).

use rust68::peripherals::atari_st::glue::{Glue, VideoMode};

#[test]
fn no_hbl_before_end_of_line() {
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(511); // 1 cycle before end of line (512 cycles PAL)
    assert!(!glue.hbl_pending());
    assert_eq!(glue.current_line(), 0);
}

#[test]
fn hbl_arms_at_the_end_of_each_line() {
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(512);
    assert!(glue.hbl_pending());
    assert_eq!(glue.current_line(), 1);

    glue.ack_hbl();
    assert!(!glue.hbl_pending());

    glue.tick(512);
    assert!(glue.hbl_pending());
    assert_eq!(glue.current_line(), 2);
}

#[test]
fn vbl_arms_at_the_visible_to_blanking_transition_not_at_wraparound() {
    // On real silicon, VBL occurs at the START of vertical blanking (right
    // after the last visible line), not at the full frame wraparound
    // (`line` going back to 0) — the rest of the blanking period elapses
    // AFTERWARDS, before visible line 0 of the next frame is displayed.
    // Conflating the two would render this line 0 in the same breath as
    // VBL being armed, leaving software no chance at all to take the
    // interrupt before it is already consumed.
    //
    // Hatari's absolute numbering (`VIDEO_START_HBL_50HZ`=63,
    // `VIDEO_END_HBL_50HZ`=263): a real TOP blanking of 63 lines precedes
    // the first displayed line (needed for STE top-border removal, see
    // `peripherals::atari_st::shifter`), so the 200 displayed lines run
    // from absolute line 63 to 262 inclusive, not 0 to 199 — only the
    // DISTRIBUTION of blanking changes (63 before + 50 after instead of 0
    // before + 113 after), not its total (113).
    let mut glue = Glue::new(VideoMode::Pal50);
    assert_eq!(glue.display_line(), None, "still in the top blanking right at the start");

    // 262 full lines (last displayed line, absolute 261, PAL):
    // no VBL yet.
    glue.tick(512 * 262);
    assert_eq!(glue.current_line(), 262);
    assert_eq!(glue.display_line(), Some(199), "last of the 200 displayed lines");
    assert!(!glue.vbl_pending());
    assert_eq!(glue.frame_count(), 0);

    // Absolute line 263 (transition into the bottom vertical blanking)
    // triggers VBL.
    glue.tick(512);
    assert!(glue.vbl_pending());
    assert!(glue.hbl_pending(), "any end of line is also an end of line (HBL)");
    assert_eq!(glue.current_line(), 263);
    assert_eq!(glue.display_line(), None, "entering the bottom blanking");
    assert_eq!(glue.frame_count(), 0, "the frame only rolls over on full wraparound (313 lines)");

    glue.ack_vbl();
    assert!(!glue.vbl_pending());

    // The rest of the bottom blanking (50 lines) makes the line wrap and
    // frame_count() advance, without re-arming VBL (already consumed
    // above) — then the top blanking of the next frame (63 lines) must
    // elapse before a displayed line reappears.
    glue.tick(512 * 50);
    assert_eq!(glue.current_line(), 0, "the line wraps to 0 at the start of the next frame");
    assert_eq!(glue.frame_count(), 1);
    assert!(!glue.vbl_pending());
    assert_eq!(glue.display_line(), None, "top blanking of the new frame");

    glue.tick(512 * 63);
    assert_eq!(glue.display_line(), Some(0), "first displayed line of the new frame");
}

#[test]
fn ntsc_uses_different_constants() {
    let mut pal = Glue::new(VideoMode::Pal50);
    let mut ntsc = Glue::new(VideoMode::Ntsc60);
    pal.tick(508);
    ntsc.tick(508);
    assert!(!pal.hbl_pending(), "508 cycles < 512 (PAL): no HBL yet");
    assert!(ntsc.hbl_pending(), "508 cycles = exactly one NTSC line");
}

#[test]
fn several_lines_in_a_single_tick() {
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(512 * 5 + 100); // 5 full lines + remainder
    assert_eq!(glue.current_line(), 5);
    assert!(glue.hbl_pending());
}

#[test]
fn write_sync_to_60hz_early_in_top_blanking_removes_the_top_border() {
    // $FF820A, bit1=0 = 60Hz selection. Occurring while still within the
    // nominal top blanking (before PAL line 63) and early enough within
    // the line cycle-wise, this switch pulls the start of the displayed
    // window to the NOMINAL start position of the 60Hz mode (34) —
    // revealing lines 34..63, normally in blanking (see Hatari,
    // `video.c`, `Video_Update_Glue_State`).
    let mut glue = Glue::new(VideoMode::Pal50);
    assert_eq!(glue.display_line(), None, "still in the border before the write");

    glue.write_sync(0x00); // bit1=0: 60Hz
    assert_eq!(glue.read_sync(), 0x00);

    // Lines 34..63 are now displayed (previously: border).
    glue.tick(512 * 34);
    assert_eq!(
        glue.display_line(),
        Some(0),
        "line 34: first pixel of the removed top border"
    );
    glue.tick(512 * 29); // reaches line 63, normal nominal position
    assert_eq!(
        glue.display_line(),
        Some(29),
        "line 63: junction with the nominal window (63-34=29 revealed lines)"
    );
}

#[test]
fn write_sync_to_60hz_at_end_of_display_removes_the_bottom_border() {
    // Same register, but this time the switch occurs on the nominal
    // second-to-last/last displayed line (262 in PAL, right before the
    // VBL transition at 263): it extends the window by
    // `VIDEO_HEIGHT_BOTTOM_50HZ` (47) additional lines instead of
    // shifting its start.
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.tick(512 * 262); // last nominal displayed line (262, PAL)
    assert_eq!(glue.display_line(), Some(199));

    glue.write_sync(0x00); // bit1=0: 60Hz, early in this line (cycles_in_line=0)

    // Without the removal, VBL would trigger at line 263 (right after):
    // it stays pending here until line 310 (263+47).
    glue.tick(512); // line 263: bottom border removed, must remain displayed
    assert_eq!(glue.display_line(), Some(200), "bottom border removed: still displayed");
    assert!(!glue.vbl_pending(), "VBL deferred as long as the bottom border stays removed");

    glue.tick(512 * 46); // up to line 309 (last extended line)
    assert_eq!(glue.display_line(), Some(246));
    assert!(!glue.vbl_pending());

    glue.tick(512); // line 310: end of the extended window, VBL now armed
    assert!(glue.vbl_pending());
    assert_eq!(glue.display_line(), None);
}

#[test]
fn write_sync_to_50hz_or_outside_the_cycle_window_has_no_effect() {
    // A switch TO 50Hz (bit1=1) must never extend the window — nor should
    // a switch to 60Hz occurring too late within the line (past the
    // "gating" threshold, see the doc of `Glue::write_sync`).
    let mut glue = Glue::new(VideoMode::Pal50);
    glue.write_sync(0x02); // bit1=1: 50Hz, no possible effect
    glue.tick(512 * 34);
    assert_eq!(glue.display_line(), None, "50Hz: the top border must not have been removed");

    let mut glue2 = Glue::new(VideoMode::Pal50);
    glue2.tick(510); // past the cycle threshold (504), still line 0 (< 512)
    assert_eq!(glue2.current_line(), 0);
    glue2.write_sync(0x00);
    glue2.tick(512 * 34 - 510);
    assert_eq!(
        glue2.display_line(),
        None,
        "switch too late within the line: no effect"
    );
}
