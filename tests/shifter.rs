#![cfg(feature = "atari-st")]
//! Unit tests for the Shifter (`rust68::peripherals::atari_st::shifter`).

use rust68::peripherals::atari_st::shifter::{Resolution, Shifter, addr, border};

#[test]
fn video_base_address_high_mid_loaded_into_counter() {
    let mut sh = Shifter::new();
    sh.write(addr::VIDEO_BASE_HIGH, 0x12);
    sh.write(addr::VIDEO_BASE_MID, 0x34);
    assert_eq!(sh.read(addr::VIDEO_BASE_HIGH), 0x12);
    assert_eq!(sh.read(addr::VIDEO_BASE_MID), 0x34);

    sh.start_frame();
    assert_eq!(sh.read(addr::VIDEO_COUNTER_HIGH), 0x12);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_MID), 0x34);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 0x00);
}

#[test]
fn resolution_read_and_written() {
    let mut sh = Shifter::new();
    assert_eq!(sh.resolution(), Resolution::Low);
    sh.write(addr::RESOLUTION, 0b01);
    assert_eq!(sh.resolution(), Resolution::Medium);
    assert_eq!(sh.read(addr::RESOLUTION), 0b01);
    sh.write(addr::RESOLUTION, 0b10);
    assert_eq!(sh.resolution(), Resolution::High);
}

#[test]
fn palette_round_trip() {
    // Word write (`.W`/`.L`, normal board path — see
    // `write_palette_word`): both bytes are taken as-is, no
    // duplication (unlike `write`, reserved for isolated `.B`
    // accesses, see `isolated_byte_duplicated_into_both_halves`).
    let mut sh = Shifter::new();
    let addr_color3 = addr::PALETTE_BASE + 3 * 2;
    sh.write_palette_word(addr_color3, 0x0777); // R=7, G=7, B=7
    assert_eq!(sh.read(addr_color3), 0x07);
    assert_eq!(sh.read(addr_color3 + 1), 0x77);
}

#[test]
fn isolated_byte_duplicated_into_both_halves() {
    // Documented real hardware behavior (Hatari, `Video_ColorReg_WriteWord`):
    // an isolated `.B` access on a palette register duplicates the written
    // byte into BOTH halves of the word before masking — the other half is
    // not preserved. Reproduces the example given in the Hatari-side
    // comment:
    //   move.w #0,$ff8240      -> color 0 = $000
    //   move.b #7,$ff8240      -> color 0 = $707
    //   move.b #$55,$ff8241    -> color 0 = $555
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    assert_eq!(sh.palette_raw()[0], 0x000);

    sh.write(addr::PALETTE_BASE, 0x07); // .B on the high byte
    assert_eq!(sh.palette_raw()[0], 0x707, ".B high duplicates 0x07 into both bytes");

    sh.write(addr::PALETTE_BASE + 1, 0x55); // .B on the low byte
    assert_eq!(sh.palette_raw()[0], 0x555, ".B low duplicates 0x55 into both bytes");
}

#[test]
fn low_resolution_render_one_16_pixel_group() {
    let mut sh = Shifter::new();
    // Palette: color 0 = black, color 1 = white.
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    let c1 = addr::PALETTE_BASE + 1 * 2;
    sh.write_palette_word(c1, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // low resolution, 4 planes

    // Plane 0 = 0x8000 (bit15 set), planes 1-3 = 0: pixel 0 must have
    // color index 1 (bit0 of plane0 set), the other 15 must have index 0.
    // A full line is required (160 bytes in low resolution); only the
    // first group of 16 pixels is non-zero.
    let mut ram = vec![0u8; 160];
    ram[0] = 0x80;
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 320, "low resolution: 320 pixels per line");
    assert_eq!(pixels[0], (255, 255, 255), "pixel 0 -> color 1 (white)");
    for p in &pixels[1..16] {
        assert_eq!(*p, (0, 0, 0), "pixels 1-15 -> color 0 (black)");
    }
}

#[test]
fn ste_palette_reorders_fine_precision_bit_before_rgb_conversion() {
    // Real bug: bit 3 of an STE palette nibble is NOT the most significant
    // bit of a normal 4-bit value — it's a fine precision bit added at the
    // BOTTOM by the hardware, with bits 2-0 remaining the original ST
    // nibble (same bus positions). Real intensity = (bits2-0<<1)|bit3,
    // confirmed against Hatari (`conv_st.c`, `ConvST_SetupRGBTable`).
    let mut sh = Shifter::new();
    sh.set_ste_palette(true);
    sh.write(addr::RESOLUTION, 0b00);

    // 0x0777: low 3-bit nibble set to 7 everywhere, fine precision bit at
    // 0 — exactly what an STE game would write wanting a component "at
    // maximum in ST 3-bit fashion". Before the fix, read as the raw 4-bit
    // value 7 -> RGB (119,119,119) instead of the real (238,238,238) (one
    // notch below true white 15/15) — a darkening by half.
    let c1 = addr::PALETTE_BASE + 1 * 2;
    sh.write_palette_word(c1, 0x0777);
    let mut ram = vec![0u8; 160];
    ram[0] = 0x80; // pixel 0 -> color index 1 (plane 0 only)
    let pixels = sh.render_scanline(&ram);
    assert_eq!(
        pixels[0],
        (238, 238, 238),
        "0x777 in STE palette: (bits2-0=7,bit3=0) -> real intensity 14/15, not 7/15"
    );

    // 0x0FFF: full nibble at 1111 (bits2-0=7 AND fine precision bit=1)
    // -> real intensity 15/15, true white. `start_frame()` reloads the
    // video counter (otherwise the next line would read past the 160
    // bytes of `ram`, out of bounds — see `render_scanline`).
    sh.write_palette_word(c1, 0x0FFF);
    sh.start_frame();
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels[0], (255, 255, 255), "0xFFF: real intensity 15/15");

    // 0x0888: ONLY the fine precision bit is set (bits2-0=0) -> real
    // intensity 1/15, nearly black — not 8/15 (mid-gray) as a naive
    // interpretation of the nibble would read.
    sh.write_palette_word(c1, 0x0888);
    sh.start_frame();
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels[0], (17, 17, 17), "0x888: real intensity 1/15, nearly black");
}

#[test]
fn medium_resolution_render_two_planes() {
    let mut sh = Shifter::new();
    for (i, val) in [(0u32, 0x000), (1, 0x700), (2, 0x070), (3, 0x777)] {
        let a = addr::PALETTE_BASE + i * 2;
        sh.write_palette_word(a, val as u16);
    }
    sh.write(addr::RESOLUTION, 0b01); // medium resolution, 2 planes

    // Plane0 = 0x8000 (pixel0 set), Plane1 = 0x4000 (pixel1 set). A full
    // line is required (160 bytes in medium resolution).
    let mut ram = vec![0u8; 160];
    ram[0..4].copy_from_slice(&[0x80, 0x00, 0x40, 0x00]);
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 640, "medium resolution: 640 pixels per line");
    assert_eq!(pixels[0], (255, 0, 0), "pixel0: plane0 only -> color 1 (red)");
    assert_eq!(pixels[1], (0, 255, 0), "pixel1: plane1 only -> color 2 (green)");
    assert_eq!(pixels[2], (0, 0, 0), "pixel2: no plane set -> color 0 (black)");
}

#[test]
fn high_resolution_render_monochrome() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b10);
    let mut ram = vec![0u8; 80]; // 640/8 = 80 bytes/line in high resolution
    ram[0] = 0b1000_0000;
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels.len(), 640, "high resolution: 640 pixels per line");
    assert_eq!(pixels[0], (0, 0, 0), "set bit -> black");
    assert_eq!(pixels[1], (255, 255, 255), "clear bit -> white");
}

#[test]
fn video_counter_advances_by_bytes_consumed() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00); // low resolution: 160 bytes/line
    let ram = vec![0u8; 1000];
    sh.render_scanline(&ram);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_HIGH), 0);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_MID), 0);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160);
}

#[test]
fn insufficient_ram_returns_black_line_but_still_advances() {
    // The Shifter's address counter is a simple generator, independent of
    // whether RAM is physically present at that address (see the doc of
    // `render_scanline`): only the DISPLAYED content depends on the
    // available RAM, not the counter's advance.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    let ram = vec![0u8; 10]; // far less than the 160 bytes required
    let pixels = sh.render_scanline(&ram);
    assert!(pixels.iter().all(|&p| p == (0, 0, 0)));
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160, "the counter advances anyway");
}

// --- STE fine scrolling (`write_hscroll`/`write_line_width`) -------------
//
// Low resolution: 4 planes, 8 bytes (4 words) per 16-pixel group, 160
// bytes/line (20 groups). All tests below only use plane 0 (the other 3
// remain zero): plane 0's source bit stream IS then directly the color
// index (0 or 1), which simplifies pixel-by-pixel verification.

#[test]
fn scroll_with_preload_reads_one_extra_group_and_shifts() {
    // $FF8265 (with preload): one EXTRA 16-pixel group is read (168 bytes
    // instead of 160) to fill the right edge without loss — see the doc of
    // `Shifter::write_hscroll`. Plane 0: group 0 = all 1s, "extra" group
    // (index 20) = all 0s. With a 1-bit shift, output pixel x reads source
    // bit x+1: pixels 0..14 stay within group 0 (all 1s, color 1), pixel 15
    // reaches the first bit of the extra group (all 0s, color 0).
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000); // color 0 = black
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777); // color 1 = white
    sh.write_hscroll(1, true, true); // apply_now=true: handled directly here, no gating to test

    let mut ram = vec![0u8; 160 + 8]; // 160 (normal line) + 8 (extra group)
    ram[0] = 0xFF;
    ram[1] = 0xFF; // plane 0, group 0 = 0xFFFF
    // extra group (bytes 160-167): plane 0 already 0 by default.
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 320);
    for (x, &p) in pixels.iter().take(15).enumerate() {
        assert_eq!(p, (255, 255, 255), "pixel {x}: within group 0 (all 1s) shifted -> color 1");
    }
    assert_eq!(pixels[15], (0, 0, 0), "pixel 15: reaches the extra group (all 0s) -> color 0");

    // The extra group is really CONSUMED on the address counter (168
    // bytes, not 160).
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 168);
}

#[test]
fn scroll_without_preload_blackens_first_16_pixels() {
    // $FF8264 (without preload): no extra byte is read — the first 16
    // output pixels come out black (hardware shift register not yet
    // loaded), the rest comes from the NORMAL buffer (160 bytes), shifted.
    // With a 1-bit shift: pixel 16 reads source bit 1 of group 0 (all 1s
    // here) -> color 1.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777);
    sh.write_hscroll(1, false, true); // false = $FF8264, without preload

    let mut ram = vec![0u8; 160]; // NO extra bytes needed
    ram[0] = 0xFF;
    ram[1] = 0xFF; // plane 0, group 0 = 0xFFFF
    let pixels = sh.render_scanline(&ram);

    for (x, &p) in pixels.iter().take(16).enumerate() {
        assert_eq!(p, (0, 0, 0), "pixel {x}: blackened edge (register not yet loaded)");
    }
    assert_eq!(pixels[16], (255, 255, 255), "pixel 16: first real pixel, shifted from group 0");

    // No extra byte consumed (160, not 168).
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160);
}

#[test]
fn zero_scroll_reproduces_exact_legacy_rendering() {
    // Non-regression guard: `h_scroll_count == 0` (the default state,
    // never modified by a game that doesn't use scrolling) MUST produce a
    // result identical to before this work, regardless of which register
    // is used to set it to zero.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777);
    sh.write_hscroll(0, true, true); // zero scroll, via $FF8265

    let mut ram = vec![0u8; 160];
    ram[0] = 0x80; // pixel 0 -> color 1 (see `low_resolution_render_one_16_pixel_group`)
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels[0], (255, 255, 255));
    for p in &pixels[1..16] {
        assert_eq!(*p, (0, 0, 0));
    }
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160, "no extra bytes when the scroll is zero");
}

#[test]
fn line_width_adds_bytes_to_address_advance() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_line_width(3, true); // +3 words = +6 bytes per line

    let ram = vec![0u8; 200];
    sh.render_scanline(&ram);
    assert_eq!(
        sh.read(addr::VIDEO_COUNTER_LOW),
        160 + 6,
        "advance = normal line bytes + LineWidth*2"
    );
}

#[test]
fn scroll_and_line_width_accumulate_on_address_advance() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_hscroll(1, true, true); // +8 bytes (preloaded extra group)
    sh.write_line_width(3, true); // +6 bytes

    let ram = vec![0u8; 200];
    sh.render_scanline(&ram);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160 + 8 + 6);
}

#[test]
fn pending_write_applies_only_to_next_line() {
    // `apply_now=false`: the value must NOT affect the line currently
    // being rendered, only the next one — reproduces Hatari's `New*`
    // mechanism (the cycle-exact gating itself, computed by the caller, is
    // tested board-side in tests/atari_st.rs).
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 1 * 2, 0x0777);

    let mut ram = vec![0u8; 160 + 8];
    ram[0] = 0xFF;
    ram[1] = 0xFF;

    sh.write_hscroll(1, true, false); // pending, NOT applied to this line
    assert_eq!(sh.h_scroll_count(), 0, "effective value unchanged before the commit");
    let pixels_line_1 = sh.render_scanline(&ram);
    assert_eq!(pixels_line_1[0], (255, 255, 255), "line 1: still zero scroll (pixel 0 = group 0 all 1s)");
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 160, "line 1: no extra bytes, scroll not yet effective");

    // The commit happened at the end of rendering line 1: line 2 now sees
    // the scroll.
    assert_eq!(sh.h_scroll_count(), 1, "effective value updated after the commit");
    sh.write(addr::VIDEO_COUNTER_LOW, 0); // resume at the same RAM location for comparison
    let pixels_line_2 = sh.render_scanline(&ram);
    assert_eq!(pixels_line_2[15], (0, 0, 0), "line 2: scroll now effective (see the preload test)");
}

// --- Horizontal border / STE overscan (`write_resolution`/`write_sync`,
// see the `border` module) ------------------------------------------------

#[test]
fn write_resolution_arms_then_confirms_left_off_2_ste() {
    // Very early hi-res (cycle <= 4) arms the attempt; a return to
    // low/medium resolution EXACTLY at cycle 4 confirms it as
    // `LEFT_OFF_2_STE` (see the doc of `write_resolution`).
    let mut sh = Shifter::new();
    sh.write_resolution(0b10, 4);
    assert_eq!(
        sh.border_mask() & border::LEFT_OFF_2_STE,
        0,
        "attempt armed but not yet confirmed: no rendering effect"
    );
    sh.write_resolution(0b00, 4);
    assert_eq!(sh.border_mask() & border::LEFT_OFF_2_STE, border::LEFT_OFF_2_STE);
    assert_eq!(sh.resolution(), Resolution::Low, "the final resolution remains the one requested");
}

#[test]
fn write_resolution_outside_window_arms_nothing() {
    let mut sh = Shifter::new();
    sh.write_resolution(0b10, 10); // cycle 10 > HDE_On_Hi (4): too late
    sh.write_resolution(0b00, 4);
    assert_eq!(sh.border_mask(), 0);
}

#[test]
fn write_resolution_return_outside_cycle_4_cancels_attempt() {
    let mut sh = Shifter::new();
    sh.write_resolution(0b10, 2); // arms
    sh.write_resolution(0b00, 6); // return, but not exactly at cycle 4: cancelled
    assert_eq!(sh.border_mask(), 0);
}

#[test]
fn left_off_2_ste_reveals_20_bytes_of_left_border() {
    // Medium resolution (2 planes, 4 bytes/16-pixel group) rather than low
    // resolution: BYTES_LEFT_OFF_2_STE (20 bytes) doesn't fall on a group
    // boundary in low resolution (8 bytes/group), which would complicate
    // pixel-by-pixel verification without adding anything to what this
    // test aims to cover (the detection mechanism + segment placement, not
    // group rounding).
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000); // black
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777); // white (index 1)
    sh.write(addr::RESOLUTION, 0b01); // medium resolution
    sh.write(addr::VIDEO_COUNTER_LOW, 100);

    sh.write_resolution(0b10, 4);
    sh.write_resolution(0b01, 4);
    assert_eq!(sh.border_mask() & border::LEFT_OFF_2_STE, border::LEFT_OFF_2_STE);

    let mut ram = vec![0u8; 260];
    ram[80] = 0x80; // left border, pixel 0 -> color 1 (counter - 20 = 80)
    ram[100] = 0x80; // central segment (nominal counter = 100), pixel 0 -> color 1
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 720, "80 px of left border (20 bytes, 2 planes) + 640 nominal px");
    assert_eq!(pixels[0], (255, 255, 255), "first pixel of the revealed left border");
    for p in &pixels[1..80] {
        assert_eq!(*p, (0, 0, 0), "rest of the left border: black (RAM is zero)");
    }
    assert_eq!(pixels[80], (255, 255, 255), "first pixel of the central segment (nominal counter)");

    // The counter advances as if only a RIGHT extension had occurred (none
    // here): +160 (nominal medium-resolution width), NOT +180 (which would
    // also count the 20 "borrowed" left-border bytes before the nominal
    // position) — see the doc of `render_scanline`.
    assert_eq!(sh.read(addr::VIDEO_COUNTER_MID), 1);
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 4); // 100+160 = 260 = 0x104
    assert_eq!(sh.border_mask(), 0, "mask reset to zero after rendering the line");
}

#[test]
fn right_off_reveals_44_bytes_of_right_border() {
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // low resolution, 160 bytes/line nominal

    // RIGHT_OFF: $FF820A switch to 60Hz within the window ]372,376].
    sh.write_sync(0x00, 374);
    assert_eq!(sh.border_mask() & border::RIGHT_OFF, border::RIGHT_OFF);

    let mut ram = vec![0u8; 160 + 44];
    ram[160] = 0x80; // first byte of the right segment -> segment pixel 0 -> color 1
    let pixels = sh.render_scanline(&ram);

    assert_eq!(pixels.len(), 320 + 88, "320 nominal px + 88 px of right border (44 bytes, 4 planes)");
    assert_eq!(pixels[320], (255, 255, 255), "first pixel of the revealed right border");
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 204, "160 (nominal) + 44 (RIGHT_OFF)");
}

#[test]
fn right_off_outside_window_has_no_effect() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_sync(0x00, 100); // well before the window ]372,376]
    assert_eq!(sh.border_mask(), 0);
    sh.write_sync(0x00, 400); // well after
    assert_eq!(sh.border_mask(), 0);
}

#[test]
fn early_60hz_nudge_adds_2_bytes_on_left() {
    // The `RIGHT_MINUS_2` nudge (-2, saturated to 0 without `RIGHT_OFF`
    // active at the same time) has no visible effect here — a documented
    // limitation in the module doc (no shortening of the central segment).
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_sync(0x00, 10); // cycle 10 <= 36 (Preload_Start_Low_60)
    assert_eq!(sh.border_mask() & border::LEFT_PLUS_2, border::LEFT_PLUS_2);
    assert_eq!(sh.border_mask() & border::RIGHT_MINUS_2, border::RIGHT_MINUS_2);

    sh.write(addr::VIDEO_COUNTER_LOW, 50);
    let ram = vec![0u8; 250];
    let pixels = sh.render_scanline(&ram);
    assert_eq!(pixels.len(), 320 + 4, "320 nominal px + 4 px of left nudge (2 bytes, 4 planes)");
    assert_eq!(sh.read(addr::VIDEO_COUNTER_LOW), 210, "50 + 160: the left nudge doesn't advance the counter");
}

#[test]
fn return_to_50hz_cancels_ongoing_horizontal_effects() {
    let mut sh = Shifter::new();
    sh.write_sync(0x00, 10); // arms the nudges
    assert_ne!(sh.border_mask(), 0);
    sh.write_sync(0x02, 20); // bit1=1: return to 50Hz
    assert_eq!(sh.border_mask(), 0);
}

// --- Per-mechanism cancellation windows (Hatari style,
// `Video_Update_Glue_State`), see the doc of `write_sync` -----------------

#[test]
fn return_to_50hz_after_52_cycles_no_longer_cancels_left_nudge_but_cancels_right() {
    // `LEFT_PLUS_2` is only cancellable up to cycle 52 (`HDE_On_Low_60`),
    // `RIGHT_MINUS_2` up to cycle 376 (`HDE_Off_Low_50`) — two DISTINCT
    // windows, not one global cancellation.
    let mut sh = Shifter::new();
    sh.write_sync(0x00, 10); // arms both nudges
    sh.write_sync(0x02, 60); // return to 50Hz at cycle 60: > 52, <= 376
    assert_eq!(
        sh.border_mask() & border::LEFT_PLUS_2,
        border::LEFT_PLUS_2,
        "left nudge already locked in (cycle 60 > 52): not cancelled"
    );
    assert_eq!(
        sh.border_mask() & border::RIGHT_MINUS_2,
        0,
        "right nudge still cancellable (cycle 60 <= 376)"
    );
}

// --- STOP_MIDDLE and RIGHT_OFF_FULL (Phase 3) ------------------------------

#[test]
fn stop_middle_shortens_line_by_106_bytes() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00); // low resolution: 160 bytes/line nominal

    // Hi-res in the middle of the line (cycle 100, within `]4, 164]`):
    // STOP_MIDDLE, not LEFT_OFF_PENDING (cycle > 4).
    sh.write_resolution(0b10, 100);
    assert_eq!(sh.border_mask() & border::STOP_MIDDLE, border::STOP_MIDDLE);
    // The resolution stays HIGH (no return to low/medium resolution
    // before the end of the line): this is the realistic case — the line
    // actually renders in high resolution, shortened.
    assert_eq!(sh.resolution(), Resolution::High);

    let ram = vec![0xFFu8; 200];
    let pixels = sh.render_scanline(&ram);

    // Nominal high resolution: 80 bytes/line (640/8). -106 saturated to 0
    // (`saturating_sub`): the shortened line ends up empty in practice at
    // this resolution (80 < 106) — see the documented limitation about
    // `BORDERBYTES_*` not being scaled per plane.
    assert_eq!(pixels.len(), 0);
}

#[test]
fn stop_middle_in_low_resolution_does_not_go_below_zero() {
    // Combined with active fine scrolling: preloading adds READ bytes (for
    // source addressing), NOT DISPLAYED pixels (same principle as without
    // `STOP_MIDDLE`, see `scroll_with_preload_reads_one_extra_group_and_
    // shifts`) — the displayed width of the central segment depends solely
    // on the `STOP_MIDDLE` reduction, not on preloading.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_hscroll(1, true, true); // active preload: +8 bytes READ, 0 displayed
    sh.write_resolution(0b10, 100); // STOP_MIDDLE
    sh.write_resolution(0b00, 200); // return to low resolution, AFTER the point of no return (164)
    assert_eq!(sh.border_mask() & border::STOP_MIDDLE, border::STOP_MIDDLE, "cycle 200 > 164: not cancelled");

    let ram = vec![0u8; 200];
    let pixels = sh.render_scanline(&ram);
    // 160 (low res.) - 106 (StopMiddle) = 54 bytes displayed = 108 pixels
    // (low resolution, 4 planes); preloading adds nothing to this.
    assert_eq!(pixels.len(), 108);
}

#[test]
fn stop_middle_cancelled_by_return_to_low_resolution_at_cycle_4() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_resolution(0b10, 100); // arms STOP_MIDDLE
    sh.write_resolution(0b00, 4); // return exactly at cycle 4: cancels (see write_resolution)
    assert_eq!(sh.border_mask() & border::STOP_MIDDLE, 0);
}

#[test]
fn right_off_full_adds_66_bytes_and_forces_left_off_on_next_line() {
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // low resolution

    // Hi-res after the STOP_MIDDLE window (>164), within `]164, 376]`:
    // RIGHT_OFF + RIGHT_OFF_FULL directly (no need for $FF820A).
    sh.write_resolution(0b10, 200);
    assert_eq!(sh.border_mask() & border::RIGHT_OFF, border::RIGHT_OFF);
    assert_eq!(sh.border_mask() & border::RIGHT_OFF_FULL, border::RIGHT_OFF_FULL);
    sh.write_resolution(0b00, 300); // back to low resolution for rendering (final resolution)

    let mut ram = vec![0u8; 300];
    ram[160] = 0x80; // first byte of the right segment -> color 1
    let pixels = sh.render_scanline(&ram);
    // 320 nominal px + 44+22=66 bytes = 132 px of right border.
    assert_eq!(pixels.len(), 320 + 132);
    assert_eq!(pixels[320], (255, 255, 255));

    // Cascade: the NEXT line starts directly with LEFT_OFF_2_STE armed
    // (20 bytes of left border), without the confirmation dance.
    assert_eq!(
        sh.border_mask() & border::LEFT_OFF_2_STE,
        border::LEFT_OFF_2_STE,
        "cascade: left border forced right from the start of the next line"
    );

    sh.write(addr::VIDEO_COUNTER_LOW, 100);
    let mut ram2 = vec![0u8; 260]; // start(80) + left border(20) + center(160) = 260
    ram2[80] = 0x80; // left border of line 2 (counter-20=80)
    let pixels2 = sh.render_scanline(&ram2);
    assert_eq!(pixels2.len(), 320 + 40, "40 px of left border (20 bytes, 4 planes)");
    assert_eq!(pixels2[0], (255, 255, 255), "left border of line 2, forced by the cascade");
}

#[test]
fn right_off_full_outside_window_arms_nothing() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_resolution(0b10, 400); // well after the `]164, 376]` window
    assert_eq!(sh.border_mask() & (border::RIGHT_OFF | border::RIGHT_OFF_FULL), 0);
}

#[test]
fn right_off_remains_cancellable_only_within_its_trigger_window() {
    // `RIGHT_OFF`'s cancellation window (`cycles_in_line <= 376`) is
    // nearly identical to its trigger window (`]372, 376]`) — a second
    // write shortly after (cycle 375) still cancels it, but a much later
    // write (cycle 400, by which the next line would already be past the
    // point of no return) can no longer do so.
    let mut sh = Shifter::new();
    sh.write_sync(0x00, 374); // arms RIGHT_OFF
    sh.write_sync(0x02, 375); // 50Hz, still within the cancellation window
    assert_eq!(sh.border_mask() & border::RIGHT_OFF, 0, "cancelled: cycle 375 <= 376");

    let mut sh2 = Shifter::new();
    sh2.write_sync(0x00, 374); // arms RIGHT_OFF
    sh2.write_sync(0x02, 400); // 50Hz, well after the cancellation window
    assert_eq!(
        sh2.border_mask() & border::RIGHT_OFF,
        border::RIGHT_OFF,
        "locked in for the rest of the line: cycle 400 > 376"
    );
}

// --- OVERSCAN_MED_RES and FOUR_BIT_SCROLL (Phase 4, Steem SSE style) -------

#[test]
fn med_res_tricks_ignored_if_left_off_2_ste_not_active() {
    // Precondition confirmed in the Steem source (`!left_border`): these
    // two tricks refine an already-revealed left border, they don't
    // trigger it themselves.
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write_resolution(0b01, 30); // within the window, but no prior LEFT_OFF_2_STE
    let ram = vec![0u8; 200];
    let _ = sh.render_scanline(&ram);
    assert_eq!(sh.border_mask() & (border::OVERSCAN_MED_RES | border::FOUR_BIT_SCROLL), 0);
}

#[test]
fn four_bit_scroll_shifts_read_without_changing_width() {
    // Isolated from OVERSCAN_MED_RES: r1=16 is NOT within its window
    // (`]24,48]`, 16 is not > 24) but IS within that of FOUR_BIT_SCROLL
    // (`[16,48]`).
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00); // low resolution
    sh.write(addr::VIDEO_COUNTER_LOW, 100);

    sh.write_resolution(0b10, 4); // arms LEFT_OFF_PENDING
    sh.write_resolution(0b00, 4); // confirms LEFT_OFF_2_STE
    sh.write_resolution(0b01, 16); // medium resolution at cycle 16 (r1)
    sh.write_resolution(0b00, 30); // return to low resolution at cycle 30 (r0_next)

    assert_eq!(sh.resolution(), Resolution::Low);

    let mut ram = vec![0u8; 270]; // start(84) + left border(20) + center(160) = 264
    // cycles_in_med_res=30-16=14, cycles_in_low_res=16-4=12,
    // shift_in_bytes=8-14/2+12/4=8-7+3=4 ; start=100-20(LEFT_OFF_2_STE)+4=84
    // (`SHIFT_SDP` Steem style ADDS the shift to the read position).
    ram[84] = 0x80;
    ram[80] = 0x80; // position WITHOUT the shift (counter-20): must NOT be what is read
    let pixels = sh.render_scanline(&ram);

    // `border_mask()` is only checkable BEFORE `render_scanline` for
    // mechanisms armed write-by-write (see the other tests) — here
    // OVERSCAN_MED_RES/FOUR_BIT_SCROLL are only computed INSIDE
    // `render_scanline` (analyzing the whole write history of the line,
    // Steem style) and then reset to zero before it returns: only the
    // EFFECT (pixel content) is observable from the outside, checked
    // below.
    assert_eq!(pixels.len(), 320 + 40, "width unchanged: only LEFT_OFF_2_STE adds pixels");
    assert_eq!(pixels[0], (255, 255, 255), "read from the position shifted by FOUR_BIT_SCROLL (84), not 80");
}

#[test]
fn overscan_med_res_shifts_read_without_changing_width() {
    // Isolated from FOUR_BIT_SCROLL: no write after switching to medium
    // resolution (no "next change" to measure), the line stays rendered
    // in medium resolution.
    let mut sh = Shifter::new();
    sh.write_palette_word(addr::PALETTE_BASE, 0x0000);
    sh.write_palette_word(addr::PALETTE_BASE + 2, 0x0777);
    sh.write(addr::RESOLUTION, 0b00);
    sh.write(addr::VIDEO_COUNTER_LOW, 100);

    sh.write_resolution(0b10, 4); // arms LEFT_OFF_PENDING
    sh.write_resolution(0b00, 4); // confirms LEFT_OFF_2_STE
    sh.write_resolution(0b01, 30); // medium resolution at cycle 30 (r1), within ]24,48]

    assert_eq!(sh.resolution(), Resolution::Medium);

    let mut ram = vec![0u8; 260];
    // cycles_in_low_res=30-4=26, shift=-((26/2)%8)/2=-((13%8)/2)=-2 ;
    // start=100-20(LEFT_OFF_2_STE)+(-2)=78.
    ram[78] = 0x80;
    let pixels = sh.render_scanline(&ram);

    // See the equivalent comment in
    // `four_bit_scroll_shifts_read_without_changing_width` on why
    // `border_mask()` isn't checkable here — only the effect (pixel
    // content) is.
    // Medium resolution (2 planes): 20 bytes of left border = 80 px.
    assert_eq!(pixels.len(), 80 + 640, "width unchanged by the shift itself");
    assert_eq!(pixels[0], (255, 255, 255), "read from the position shifted by OVERSCAN_MED_RES");
}

#[test]
fn resolution_write_history_and_med_res_byte_shift_reset_every_line() {
    let mut sh = Shifter::new();
    sh.write(addr::RESOLUTION, 0b00);
    sh.write(addr::VIDEO_COUNTER_LOW, 100);
    sh.write_resolution(0b10, 4);
    sh.write_resolution(0b00, 4);
    sh.write_resolution(0b01, 30); // arms OVERSCAN_MED_RES for this line (medium resolution)

    let ram = vec![0u8; 260];
    let pixels1 = sh.render_scanline(&ram);
    assert_eq!(pixels1.len(), 80 + 640, "line 1: LEFT_OFF_2_STE left border still active");

    // Next line, no new $FF8260 write: LEFT_OFF_2_STE is NOT carried over
    // (no `RIGHT_OFF_FULL` cascade here) — neither it nor
    // OVERSCAN_MED_RES/FOUR_BIT_SCROLL (which depend on it, see
    // `detect_med_res_tricks`) survive into the next line.
    let ram2 = vec![0u8; 300];
    let pixels2 = sh.render_scanline(&ram2);
    assert_eq!(pixels2.len(), 640, "line 2: nominal width, no more left border");
}
