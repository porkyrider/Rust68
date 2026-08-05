//! Compare notre Blitter reel contre le portage exact de
//! `hatari_reference_execute_chained` (tests/blitter_hatari_diff.rs, deja
//! valide sur 8731 cas + une sequence chainee) sur la sequence REELLE
//! complete a 188 appels capturee lors de la selection de l'icone.
use rust68::peripherals::atari_st::blitter::{Blitter, reg};
use rust68::Bus;

struct Mem { m: std::collections::HashMap<u32,u8> }
impl Bus for Mem {
    fn read8(&mut self, a: u32) -> u8 { *self.m.get(&a).unwrap_or(&0) }
    fn write8(&mut self, a: u32, v: u8) { self.m.insert(a,v); }
}

fn wl(bl: &mut Blitter, off: u32, v: u32) { bl.write_long(off, v); }
fn ww(bl: &mut Blitter, off: u32, v: u16) { bl.write_word(off, v); }

/// Rejoue jusqu'a completion totale, comme le fait le CPU reel via sa boucle
/// de scrutation TAS.B qui rappelle execute() a chaque reprise tant que
/// BUSY reste actif (execute() est volontairement incremental : une seule
/// invocation ne traite qu'une tranche de 16 mots en mode non-HOG).
fn run_to_completion(bl: &mut Blitter, bus: &mut impl Bus) {
    bl.execute(bus);
    while bl.busy() {
        bl.execute(bus);
    }
}

#[derive(Clone, Copy, Debug)]
struct Cfg { hop: u8, op: u8, skew_reg: u8, control_line: u8, smudge: bool, src_x_inc: i16, src_y_inc: i16, dst_x_inc: i16, dst_y_inc: i16, x_count: u16, y_count: u16, endmask: [u16;3], halftone: [u16;16] }

fn apply_op_reference(op: u8, s: u16, d: u16) -> u16 {
    match op & 0x0F {
        0x0 => 0, 0x1 => s & d, 0x2 => s & !d, 0x3 => s, 0x4 => !s & d, 0x5 => d, 0x6 => s ^ d, 0x7 => s | d,
        0x8 => !s & !d, 0x9 => !(s ^ d), 0xA => !d, 0xB => s | !d, 0xC => !s, 0xD => !s | d, 0xE => !s | !d, 0xF => 0xFFFF, _ => unreachable!() }
}

fn hatari_reference_execute_chained(cfg: &Cfg, src_addr0: u32, dst_addr0: u32, bus: &mut impl Bus, buffer: &mut u32) {
    let fxsr_reg = cfg.skew_reg & 0x80 != 0;
    let nfsr_reg = cfg.skew_reg & 0x40 != 0;
    let skew = (cfg.skew_reg & 0x0F) as u32;
    let lop_need_src = !matches!(cfg.op & 0x0F, 0x00 | 0x05 | 0x0A | 0x0F);
    let hop_uses_src = (cfg.hop & 0x02) != 0 || (cfg.hop == 1 && cfg.smudge);
    let need_src = lop_need_src && hop_uses_src;
    let x_count_reset = (cfg.x_count as u32).max(1);
    let mut x_count = x_count_reset;
    let mut y_count = cfg.y_count as u32;
    let mut src_addr = src_addr0;
    let mut dst_addr = dst_addr0;
    let mut halftone_line = (cfg.control_line & 0x0F) as usize;
    let mut bus_word: u16 = 0;
    let mut have_fxsr = false;
    let mut nfsr_dynamic = false;
    let shift = |buffer: &mut u32, sxi: i16| { if sxi < 0 { *buffer >>= 16; } else { *buffer <<= 16; } };
    let fetch = |buffer: &mut u32, sxi: i16, w: u16| { if sxi < 0 { *buffer |= (w as u32) << 16; } else { *buffer |= w as u32; } };
    while y_count > 0 {
        let first_word = x_count == x_count_reset;
        if first_word { nfsr_dynamic = false; }
        let mask = if first_word || x_count_reset == 1 { cfg.endmask[0] } else if x_count == 1 { cfg.endmask[2] } else { cfg.endmask[1] };
        if fxsr_reg && !have_fxsr && need_src {
            shift(buffer, cfg.src_x_inc);
            let w = bus.read16(src_addr & rust68::ADDR_MASK);
            bus_word = w; fetch(buffer, cfg.src_x_inc, w);
            src_addr = src_addr.wrapping_add(cfg.src_x_inc as i32 as u32);
            have_fxsr = true;
        }
        let mut fetch_src = false;
        if need_src && !nfsr_dynamic {
            shift(buffer, cfg.src_x_inc);
            let w = bus.read16(src_addr & rust68::ADDR_MASK);
            bus_word = w; fetch(buffer, cfg.src_x_inc, w);
            fetch_src = true;
        }
        let weird_case = nfsr_reg && x_count == 1;
        if weird_case { shift(buffer, cfg.src_x_inc); fetch(buffer, cfg.src_x_inc, bus_word); }
        let source_val = (*buffer >> skew) as u16;
        let halftone_word = if cfg.smudge { cfg.halftone[(source_val & 0x0F) as usize] } else { cfg.halftone[halftone_line] };
        let hop_result = match cfg.hop & 0x3 { 0 => 0xFFFF, 1 => halftone_word, 2 => source_val, 3 => source_val & halftone_word, _ => unreachable!() };
        let dst_word = bus.read16(dst_addr & rust68::ADDR_MASK);
        let lop = apply_op_reference(cfg.op, hop_result, dst_word);
        let dst_data = if mask != 0xFFFF { (lop & mask) | (dst_word & !mask) } else { lop };
        bus.write16(dst_addr & rust68::ADDR_MASK, dst_data);
        if weird_case { shift(buffer, cfg.src_x_inc); fetch(buffer, cfg.src_x_inc, bus_word); }
        if x_count == 2 && nfsr_reg { nfsr_dynamic = true; }
        if fetch_src {
            if x_count == 1 || nfsr_dynamic { src_addr = src_addr.wrapping_add(cfg.src_y_inc as i32 as u32); }
            else { src_addr = src_addr.wrapping_add(cfg.src_x_inc as i32 as u32); }
        }
        if x_count == 1 {
            have_fxsr = false; y_count -= 1; x_count = x_count_reset;
            dst_addr = dst_addr.wrapping_add(cfg.dst_y_inc as i32 as u32);
            halftone_line = if cfg.dst_y_inc >= 0 { (halftone_line + 1) & 15 } else { halftone_line.wrapping_sub(1) & 15 };
        } else {
            x_count -= 1; dst_addr = dst_addr.wrapping_add(cfg.dst_x_inc as i32 as u32);
        }
    }
}

fn main() {
    let mut mem_real = Mem { m: std::collections::HashMap::new() };
    let mut mem_ref = Mem { m: std::collections::HashMap::new() };
    let src_base: u32 = 0xd000;
    let src_bytes: &[u8] = &[
        0x74,0x69,0x6f,0x6e,0x7c,0x22,0x46,0x6f,0x72,0x6d,0x61,0x74,0x61,0x67,0x65,0x22,
        0x20,0x64,0x75,0x20,0x6d,0x65,0x6e,0x75,0x20,0x46,0x69,0x63,0x68,0x69,0x65,0x72,
        0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x43,
        0x6f,0x70,0x69,0x65,0x72,0x20,0x6c,0x65,0x20,0x64,0x69,0x73,0x71,0x75,0x65,0x20,
        0x25,0x53,0x3a,0x20,0x73,0x75,0x72,0x20,0x6c,0x65,0x7c,0x64,0x69,0x73,0x71,0x75,
        0x65,0x20,0x25,0x53,0x3a,0x20,0x65,0x66,0x66,0x61,0x63,0x65,0x72,0x61,0x20,0x74,
        0x6f,0x75,0x74,0x65,0x73,0x20,0x6c,0x65,0x73,0x7c,0x64,0x6f,0x6e,0x6e,0x82,0x65,
        0x73,0x20,0x63,0x6f,0x6e,0x74,0x65,0x6e,0x75,0x65,0x73,0x20,0x73,0x75,0x72,0x20,
        0x6c,0x65,0x7c,0x64,0x69,0x73,0x71,0x75,0x65,0x20,0x25,0x53,0x3a,0x2e,0x20,0x43,
        0x6c,0x69,0x71,0x75,0x65,0x72,0x20,0x73,0x75,0x72,0x20,0x4f,0x4b,0x7c,0x70,0x6f,
        0x75,0x72,0x20,0x63,0x6f,0x6e,0x66,0x69,0x72,0x6d,0x65,0x72,0x20,0x6c,0x61,0x20,
        0x63,0x6f,0x70,0x69,0x65,0x2e,0x5d,0x5b,0x4f,0x4b,0x7c,0x41,0x4e,0x4e,0x55,0x4c,
        0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x56,0x6f,0x75,0x73,0x20,0x6e,0x65,0x20,
        0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,0x61,0x73,0x20,0x64,0x82,0x70,0x6c,0x61,
        0x63,0x65,0x72,0x7c,0x6c,0x65,0x73,0x20,0x64,0x6f,0x73,0x73,0x69,0x65,0x72,0x73,
        0x20,0x65,0x74,0x20,0x6c,0x65,0x73,0x20,0x66,0x69,0x63,0x68,0x69,0x65,0x72,0x73,
        0x7c,0x73,0x75,0x72,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,0x75,0x2e,0x5d,
        0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x56,0x6f,0x75,
        0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,0x61,0x73,0x20,
        0x70,0x6c,0x61,0x63,0x65,0x72,0x7c,0x6c,0x27,0x69,0x63,0x93,0x6e,0x65,0x20,0x64,
        0x65,0x20,0x6c,0x61,0x20,0x70,0x6f,0x75,0x62,0x65,0x6c,0x6c,0x65,0x20,0x64,0x61,
        0x6e,0x73,0x7c,0x75,0x6e,0x65,0x20,0x66,0x65,0x6e,0x88,0x74,0x72,0x65,0x2e,0x5d,
        0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x32,0x5d,0x5b,0x56,0x6f,0x75,
        0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,0x61,0x73,0x20,
        0x63,0x72,0x82,0x65,0x72,0x20,0x64,0x65,0x7c,0x64,0x6f,0x73,0x73,0x69,0x65,0x72,
        0x20,0x61,0x76,0x65,0x63,0x20,0x63,0x65,0x20,0x6e,0x6f,0x6d,0x2e,0x7c,0x52,0x65,
        0x63,0x6f,0x6d,0x6d,0x65,0x6e,0x63,0x65,0x7a,0x20,0x65,0x6e,0x20,0x64,0x6f,0x6e,
        0x6e,0x61,0x6e,0x74,0x20,0x75,0x6e,0x7c,0x61,0x75,0x74,0x72,0x65,0x20,0x6e,0x6f,
        0x6d,0x2c,0x20,0x6f,0x75,0x20,0x61,0x6e,0x6e,0x75,0x6c,0x65,0x7a,0x20,0x6c,0x61,
        0x7c,0x64,0x65,0x6d,0x61,0x6e,0x64,0x65,0x20,0x64,0x65,0x20,0x63,0x72,0x82,0x61,
        0x74,0x69,0x6f,0x6e,0x2e,0x5d,0x5b,0x52,0x90,0x45,0x53,0x53,0x41,0x59,0x45,0x52,
        0x7c,0x41,0x4e,0x4e,0x55,0x4c,0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x43,0x65,
        0x20,0x64,0x69,0x73,0x71,0x75,0x65,0x20,0x6e,0x27,0x61,0x20,0x70,0x61,0x73,0x20,
        0x61,0x73,0x73,0x65,0x7a,0x20,0x64,0x65,0x7c,0x70,0x6c,0x61,0x63,0x65,0x20,0x70,
        0x6f,0x75,0x72,0x20,0x63,0x65,0x74,0x74,0x65,0x20,0x6f,0x70,0x82,0x72,0x61,0x74,
        0x69,0x6f,0x6e,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,
        0x5d,0x5b,0x56,0x6f,0x75,0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,
        0x20,0x70,0x61,0x73,0x20,0x63,0x72,0x82,0x65,0x72,0x20,0x6f,0x75,0x7c,0x61,0x63,
        0x63,0x82,0x64,0x65,0x72,0x20,0x85,0x20,0x75,0x6e,0x20,0x64,0x6f,0x73,0x73,0x69,
        0x65,0x72,0x20,0x61,0x75,0x73,0x73,0x69,0x7c,0x6c,0x6f,0x69,0x6e,0x20,0x64,0x61,
        0x6e,0x73,0x20,0x6c,0x27,0x61,0x72,0x62,0x6f,0x72,0x65,0x73,0x63,0x65,0x6e,0x63,
        0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,
        0x56,0x6f,0x75,0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,
        0x61,0x73,0x20,0x63,0x6f,0x70,0x69,0x65,0x72,0x7c,0x75,0x6e,0x20,0x64,0x6f,0x73,
        0x73,0x69,0x65,0x72,0x20,0x6f,0x75,0x20,0x75,0x6e,0x20,0x66,0x69,0x63,0x68,0x69,
        0x65,0x72,0x20,0x64,0x65,0x7c,0x6f,0x75,0x20,0x73,0x75,0x72,0x20,0x75,0x6e,0x65,
        0x20,0x63,0x61,0x72,0x74,0x6f,0x75,0x63,0x68,0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,
        0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x4f,0x70,0x82,0x72,0x61,0x74,0x69,
        0x6f,0x6e,0x20,0x64,0x65,0x20,0x63,0x6f,0x70,0x69,0x65,0x20,0x69,0x6e,0x76,0x61,
        0x6c,0x69,0x64,0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,
        0x31,0x5d,0x5b,0x4c,0x61,0x20,0x70,0x6f,0x75,0x62,0x65,0x6c,0x6c,0x65,0x20,0x73,
        0x65,0x72,0x74,0x20,0x85,0x20,0x65,0x66,0x66,0x61,0x63,0x65,0x72,0x7c,0x75,0x6e,
        0x20,0x64,0x6f,0x73,0x73,0x69,0x65,0x72,0x20,0x6f,0x75,0x20,0x75,0x6e,0x20,0x66,
        0x69,0x63,0x68,0x69,0x65,0x72,0x7c,0x64,0x65,0x20,0x66,0x61,0x87,0x6f,0x6e,0x20,
        0x70,0x65,0x72,0x6d,0x61,0x6e,0x65,0x6e,0x74,0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,
        0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x4c,0x65,0x20,0x73,0x79,0x73,0x74,
        0x8a,0x6d,0x65,0x20,0x6e,0x27,0x61,0x20,0x70,0x6c,0x75,0x73,0x20,0x61,0x73,0x73,
        0x65,0x7a,0x20,0x64,0x65,0x7c,0x6d,0x82,0x6d,0x6f,0x69,0x72,0x65,0x20,0x21,0x5d,
        0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x55,0x6e,0x65,
        0x20,0x65,0x72,0x72,0x65,0x75,0x72,0x20,0x61,0x20,0x82,0x74,0x82,0x20,0x64,0x82,
        0x74,0x65,0x63,0x74,0x82,0x65,0x7c,0x64,0x75,0x72,0x61,0x6e,0x74,0x20,0x75,0x6e,
        0x20,0x66,0x6f,0x72,0x6d,0x61,0x74,0x61,0x67,0x65,0x20,0x6f,0x75,0x20,0x75,0x6e,
        0x65,0x7c,0x63,0x6f,0x70,0x69,0x65,0x2e,0x20,0x4c,0x65,0x20,0x64,0x69,0x73,0x71,
        0x75,0x65,0x20,0x63,0x6f,0x6e,0x63,0x65,0x72,0x6e,0x82,0x20,0x65,0x73,0x74,0x7c,
        0x70,0x65,0x75,0x74,0x2d,0x88,0x74,0x72,0x65,0x20,0x70,0x72,0x6f,0x74,0x82,0x67,
        0x82,0x20,0x65,0x6e,0x20,0x82,0x63,0x72,0x69,0x74,0x75,0x72,0x65,0x7c,0x6f,0x75,
        0x20,0x69,0x6e,0x75,0x74,0x69,0x6c,0x69,0x73,0x61,0x62,0x6c,0x65,0x2e,0x5d,0x5b,
        0x52,0x90,0x45,0x53,0x53,0x41,0x59,0x45,0x52,0x7c,0x41,0x42,0x41,0x4e,0x44,0x4f,
        0x4e,0x4e,0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x43,0x65,0x20,0x64,0x69,0x73,
        0x71,0x75,0x65,0x20,0x63,0x6f,0x6e,0x74,0x69,0x65,0x6e,0x74,0x20,0x25,0x4c,0x7c,
        0x6f,0x63,0x74,0x65,0x74,0x73,0x20,0x64,0x69,0x73,0x70,0x6f,0x6e,0x69,0x62,0x6c,
        0x65,0x73,0x20,0x70,0x6f,0x75,0x72,0x7c,0x6c,0x27,0x75,0x74,0x69,0x6c,0x69,0x73,
        0x61,0x74,0x65,0x75,0x72,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,
        0x5b,0x33,0x5d,0x5b,0x4c,0x65,0x20,0x64,0x69,0x73,0x71,0x75,0x65,0x20,0x64,0x65,
        0x20,0x64,0x65,0x73,0x74,0x69,0x6e,0x61,0x74,0x69,0x6f,0x6e,0x20,0x6e,0x27,0x65,
        0x73,0x74,0x7c,0x70,0x61,0x73,0x20,0x64,0x75,0x20,0x6d,0x88,0x6d,0x65,0x20,0x74,
        0x79,0x70,0x65,0x20,0x71,0x75,0x65,0x20,0x6c,0x65,0x20,0x64,0x69,0x73,0x71,0x75,
        0x65,0x7c,0x64,0x65,0x20,0x64,0x82,0x70,0x61,0x72,0x74,0x2e,0x20,0x49,0x6e,0x73,
        0x82,0x72,0x65,0x72,0x20,0x75,0x6e,0x20,0x61,0x75,0x74,0x72,0x65,0x7c,0x64,0x69,
        0x73,0x71,0x75,0x65,0x2e,0x5d,0x5b,0x52,0x90,0x45,0x53,0x53,0x41,0x59,0x45,0x52,
        0x7c,0x41,0x42,0x41,0x4e,0x44,0x4f,0x4e,0x4e,0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,
        0x5b,0x53,0x61,0x75,0x76,0x65,0x72,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,
        0x75,0x20,0x3f,0x5d,0x5b,0x4f,0x4b,0x7c,0x41,0x4e,0x4e,0x55,0x4c,0x45,0x52,0x5d,
        0x00,0x5b,0x31,0x5d,0x5b,0x49,0x6d,0x70,0x72,0x65,0x73,0x73,0x69,0x6f,0x6e,0x20,
        0x64,0x65,0x20,0x6c,0x27,0x82,0x63,0x72,0x61,0x6e,0x20,0x3f,0x5d,0x5b,0x4f,0x4b,
        0x7c,0x41,0x4e,0x4e,0x55,0x4c,0x45,0x52,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x53,0x65,
        0x75,0x6c,0x73,0x20,0x6c,0x65,0x73,0x20,0x6c,0x65,0x63,0x74,0x65,0x75,0x72,0x73,
        0x20,0x64,0x65,0x7c,0x64,0x69,0x73,0x71,0x75,0x65,0x74,0x74,0x65,0x73,0x20,0x73,
        0x6f,0x6e,0x74,0x20,0x75,0x74,0x69,0x6c,0x69,0x73,0x61,0x62,0x6c,0x65,0x73,0x7c,
        0x70,0x6f,0x75,0x72,0x20,0x63,0x65,0x74,0x74,0x65,0x20,0x6f,0x70,0x82,0x72,0x61,
        0x74,0x69,0x6f,0x6e,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,
        0x31,0x5d,0x5b,0x41,0x62,0x61,0x6e,0x64,0x6f,0x6e,0x6e,0x65,0x72,0x20,0x63,0x65,
        0x74,0x74,0x65,0x20,0x6f,0x70,0x82,0x72,0x61,0x74,0x69,0x6f,0x6e,0x20,0x3f,0x5d,
        0x5b,0x20,0x4f,0x55,0x49,0x20,0x7c,0x4e,0x4f,0x4e,0x5d,0x00,0x5b,0x31,0x5d,0x5b,
        0x44,0x82,0x73,0x6f,0x6c,0x82,0x2c,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,
        0x75,0x20,0x6e,0x65,0x20,0x70,0x65,0x75,0x74,0x7c,0x70,0x61,0x73,0x20,0x69,0x6e,
        0x73,0x74,0x61,0x6c,0x6c,0x65,0x72,0x20,0x64,0x27,0x69,0x63,0x93,0x6e,0x65,0x7c,
        0x73,0x75,0x70,0x6c,0x82,0x6d,0x65,0x6e,0x74,0x61,0x69,0x72,0x65,0x2e,0x5d,0x5b,
        0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x44,0x82,0x73,0x6f,
        0x6c,0x82,0x2c,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,0x75,0x20,0x6e,0x65,
        0x20,0x70,0x65,0x75,0x74,0x20,0x70,0x61,0x73,0x7c,0x69,0x6e,0x73,0x74,0x61,0x6c,
        0x6c,0x65,0x72,0x20,0x64,0x27,0x61,0x70,0x70,0x6c,0x69,0x63,0x61,0x74,0x69,0x6f,
        0x6e,0x7c,0x73,0x75,0x70,0x6c,0x82,0x6d,0x65,0x6e,0x74,0x61,0x69,0x72,0x65,0x2e,
        0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x00,0x00,0x00,0x00,0x00,0x1b,
        0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x1b,
        0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x3b,0xb8,0x00,0x00,0x3b,0xb8,0x00,0x00,0x3b,
        0xb8,0x00,0x00,0x3b,0xb8,0x00,0x00,0x7b,0xbc,0x00,0x00,0x7b,0xbc,0x00,0x00,0xfb,
        0xbe,0x00,0x01,0xf3,0x9f,0x00,0x03,0xf3,0x9f,0x80,0x0f,0xe3,0x8f,0xe0,0x7f,0xc3,
        0x87,0xfc,0x7f,0x83,0x83,0xfc,0x7e,0x03,0x80,0xfc,0x78,0x03,0x80,0x3c,0x00,0x00,
        0x00,0x00,0x09,0xf9,0x0f,0x8c,0x1d,0xfb,0x8f,0xcc,0x1c,0x63,0x8c,0xec,0x36,0x66,
        0xcc,0xec,0x36,0x66,0xcd,0xcc,0x7f,0x6f,0xed,0x8c,0x7f,0x6f,0xed,0xcc,0x63,0x6c,
        0x6c,0xec,0x63,0x6c,0x6c,0x6c,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x0f,0xe0,0x00,0x00,0x1f,0xf0,0x00,0x7f,0x7f,0xfc,0x00,0xff,
        0xff,0xfc,0x03,0xff,0xff,0xff,0x03,0xff,0xff,0xff,0x0f,0xff,0xff,0xff,0x0f,0xff,
        0xff,0xff,0x3f,0xff,0xff,0xff,0x3f,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xff,0xff,0xff,0xff,0xff,0xfe,0xff,0xff,0xff,0xfc,0xff,0xff,0xff,0xf8,0xff,0xff,
        0xff,0xf0,0xff,0xff,0xff,0xe0,0xff,0xff,0xff,0xc0,0xff,0xff,0xff,0x80,0xff,0xff,
        0xff,0x00,0xff,0xff,0xfe,0x00,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x0f,0xe0,0x00,0x00,0x18,0x30,0x00,0x7f,0x70,0x1c,0x00,0xc1,
        0x80,0x04,0x03,0x80,0xff,0xf7,0x02,0x00,0x00,0x15,0x0f,0xfb,0xfb,0xd3,0x08,0x06,
        0x0c,0x57,0x3f,0xfc,0x07,0x4d,0x20,0x00,0x01,0x59,0xff,0xff,0xfe,0x31,0x80,0x00,
        0x02,0x63,0x80,0x00,0x02,0xc5,0x80,0x00,0x03,0x89,0x80,0x00,0x03,0x13,0x80,0x00,
        0x02,0x25,0x80,0x00,0x02,0x49,0x80,0x00,0x02,0x91,0x81,0xfe,0x03,0x23,0x81,0x02,
        0x02,0x46,0x81,0x02,0x02,0x8c,0x81,0xfe,0x03,0x18,0x80,0x00,0x02,0x30,0x80,0x00,
        0x02,0x60,0x83,0x06,0x02,0xc0,0x87,0xfc,0x03,0x80,0x80,0x00,0x03,0x00,0x80,0x00,
        0x02,0x00,0x80,0x00,0x02,0x00,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x0f,0xfc,0x00,0x00,0x1f,0xfe,0x1f,0xff,0xff,0xfe,0x3f,0xff,0xff,0xfe,0x3f,0xff,
        0xff,0xfe,0x3f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x3f,0xff,0xff,0xfc,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x0f,0xfc,0x00,0x00,0x18,0x06,0x1f,0xff,0xf0,0x02,0x20,0x00,0x00,0x02,0x3f,0xff,
        0xff,0xf2,0x20,0x00,0x00,0x0a,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x60,0x00,
        0x00,0x06,0x3f,0xff,0xff,0xfc,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x03,0xe0,0x00,0x00,0x7e,0x3f,0x00,0x01,0xff,0xff,0xc0,0x03,0xff,
        0xff,0xe0,0x03,0xff,0xff,0xe0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x00,0xff,
        0xff,0x80,0x00,0x3f,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x03,0xe0,0x00,0x00,0x7e,0x3f,0x00,0x01,0xc6,0x31,0xc0,0x02,0x00,
        0x00,0x20,0x03,0xc0,0x01,0xe0,0x01,0x7f,0xff,0x40,0x01,0x00,0x00,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x8a,0x40,0x01,0x44,0xda,0x40,0x01,0x44,0x72,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x64,0x26,0x40,0x01,0x86,0x60,0xc0,0x00,0xe0,
        0x03,0x80,0x00,0x3f,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x7f,0xff,0xff,0xfc,0x7f,0xff,
        0xff,0xfc,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x1f,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x7f,0xff,0xff,0xfc,0x40,0x00,
        0x00,0x04,0x55,0x55,0x55,0x56,0x40,0x00,0x00,0x06,0x7f,0xff,0xff,0xfe,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x7f,0xff,
        0xff,0xfe,0x1f,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x1f,0xff,
        0xff,0x80,0x1f,0xff,0xff,0x80,0x1f,0xff,0xff,0xe0,0x1f,0xff,0xff,0xe0,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x07,0xff,0xff,0xf8,0x07,0xff,
        0xff,0xf8,0x01,0xff,0xff,0xf8,0x01,0xff,0xff,0xf8,0x00,0x00,0x00,0x00,0x1f,0xff,
        0xff,0x80,0x10,0x00,0x00,0x80,0x10,0x00,0x00,0xe0,0x10,0x00,0x00,0xa0,0x10,0x00,
        0x00,0xb8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x3f,0xa8,0x10,0x00,0x21,0xa8,0x10,0x00,0x23,0x28,0x10,0x00,0x26,0x28,0x10,0x00,
        0x2c,0x28,0x10,0x00,0x38,0x28,0x1f,0xff,0xf0,0x28,0x04,0x00,0x00,0x28,0x07,0xff,
        0xff,0xe8,0x01,0x00,0x00,0x08,0x01,0xff,0xff,0xf8,0x00,0x00,0xcc,0xc3,0x00,0x00,
        0xcc,0xe6,0x00,0x00,0xcc,0xee,0x00,0x00,0xcc,0xff,0x00,0x00,0xcd,0x09,0x00,0x00,
        0xcd,0x1d,0x00,0x00,0xcd,0x36,0x00,0x00,0xcd,0x4b,0x00,0x00,0xcd,0x60,0x00,0x00,
        0xcd,0x7a,0x00,0x00,0xcd,0x8e,0x00,0x00,0xcd,0xa3,0x00,0x00,0xcd,0xb7,0x00,0x00,
        0xce,0x07,0x00,0x00,0xce,0x7e,0x00,0x00,0xce,0xbd,0x00,0x00,0xcf,0x41,0x00,0x00,
        0xcf,0x90,0x00,0x00,0xd0,0x2b,0x00,0x00,0xd0,0xc4,0x00,0x00,0xd1,0x19,0x00,0x00,
        0xd1,0x69,0x00,0x00,0xd1,0xfa,0x00,0x00,0xd2,0x3e,0x00,0x00,0xd2,0x9c,0x00,0x00,
        0xd2,0xf5,0x00,0x00,0xd3,0x1f,0x00,0x00,0xd3,0x75,0x00,0x00,0xd3,0xa9,0x00,0x00,
        0xd4,0x46,0x00,0x00,0xd4,0x90,0x00,0x00,0xd5,0x0d,0x00,0x00,0xd5,0x31,0x00,0x00,
        0xd5,0x5a,0x00,0x00,0xd5,0xaf,0x00,0x00,0xd5,0xdc,0x00,0x00,0xd6,0x28,0x00,0x00,
        0xd6,0x7a,0x00,0x04,0x00,0x20,0x00,0x00,0x00,0x00,0x00,0x02,0x00,0x00,0xd6,0xfa,
        0x00,0x00,0xd7,0x7a,0x00,0x00,0xcc,0x4e,0x10,0x00,0x00,0x0d,0x00,0x0d,0x00,0x14,
        0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,0x00,0x48,0x00,0x08,0x00,0x00,
        0xd7,0xfa,0x00,0x00,0xd8,0x7a,0x00,0x00,0xcc,0x5b,0x10,0x00,0x00,0x00,0x00,0x00,
        0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,0x00,0x48,0x00,0x08,
        0x00,0x00,0xd8,0xfa,0x00,0x00,0xd9,0x7a,0x00,0x00,0xcc,0x68,0x10,0x00,0x00,0x00,
        0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,0x00,0x48,
        0x00,0x08,0x00,0x00,0xd9,0xfa,0x00,0x00,0xda,0x7a,0x00,0x00,0xcc,0x75,0x10,0x00,
        0x00,0x00,0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,
        0x00,0x48,0x00,0x08,0x00,0x00,0xda,0xfa,0x00,0x00,0xdb,0x7a,0x00,0x00,0xcc,0x82,
        0x10,0x00,0x00,0x00,0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,
        0x00,0x20,0x00,0x48,0x00,0x08,0x00,0x00,0xc5,0x60,0x00,0x00,0xc5,0x62,0x00,0x00,
        0xc5,0x63,0x00,0x03,0x00,0x06,0x00,0x02,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x02,
        0x00,0x01,0x00,0x00,0xc5,0x64,0x00,0x00,0xc5,0x70,0x00,0x00,0xc5,0x83,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x13,0x00,0x00,
        0xc5,0x85,0x00,0x00,0xc5,0x90,0x00,0x00,0xc5,0xab,0x00,0x03,0x00,0x06,0x00,0x01,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0b,0x00,0x1b,0x00,0x00,0xc5,0xad,0x00,0x00,
        0xc5,0xb4,0x00,0x00,0xc5,0xc4,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x07,0x00,0x10,0x00,0x00,0xc5,0xc6,0x00,0x00,0xc5,0xcd,0x00,0x00,
        0xc5,0xde,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x07,
        0x00,0x11,0x00,0x00,0xc5,0xe0,0x00,0x00,0xc5,0xe6,0x00,0x00,0xc6,0x01,0x00,0x03,
        0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x1b,0x00,0x00,
        0xc6,0x03,0x00,0x00,0xc6,0x09,0x00,0x00,0xc6,0x24,0x00,0x03,0x00,0x06,0x00,0x01,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x1b,0x00,0x00,0xc6,0x4e,0x00,0x00,
        0xc6,0x50,0x00,0x00,0xc6,0x63,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x02,0x00,0x13,0x00,0x00,0xc6,0x65,0x00,0x00,0xc6,0x71,0x00,0x00,
        0xc6,0x8e,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,
        0x00,0x1d,0x00,0x00,0xc6,0x90,0x00,0x00,0xc6,0x96,0x00,0x00,0xc6,0xb6,0x00,0x03,
        0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x20,0x00,0x00,
        0xc6,0xb8,0x00,0x00,0xc6,0xbe,0x00,0x00,0xc6,0xde,0x00,0x03,0x00,0x06,0x00,0x01,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x20,0x00,0x00,0xc6,0xe0,0x00,0x00,
        0xc6,0xeb,0x00,0x00,0xc7,0x08,0x00,0x03,0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x0b,0x00,0x1d,0x00,0x00,0xc7,0x0a,0x00,0x00,0xc7,0x15,0x00,0x00,
        0xc7,0x35,0x00,0x03,0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0b,
        0x00,0x20,0x00,0x00,0xc7,0xed,0x00,0x00,0xc7,0xf9,0x00,0x00,0xc8,0x0c,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x13,0x00,0x00,
        0xc8,0x1b,0x00,0x00,0xc8,0x42,0x00,0x00,0xc8,0x69,0x00,0x03,0x00,0x06,0x00,0x00,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x27,0x00,0x27,0x00,0x00,0xc8,0x89,0x00,0x00,
        0xc8,0x8b,0x00,0x00,0xc8,0x9f,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x02,0x00,0x14,0x00,0x00,0xc8,0xa1,0x00,0x00,0xc8,0xae,0x00,0x00,
        0xc8,0xcb,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0d,
        0x00,0x1d,0x00,0x00,0xc9,0x1c,0x00,0x00,0xc9,0x28,0x00,0x00,0xc9,0x4c,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x24,0x00,0x00,
        0xc9,0x4e,0x00,0x00,0xc9,0x52,0x00,0x00,0xc9,0x69,0x00,0x03,0x00,0x06,0x00,0x00,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x04,0x00,0x17,0x00,0x00,0xc9,0xce,0x00,0x00,
        0xc9,0xd4,0x00,0x00,0xc9,0xef,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x06,0x00,0x1b,0x00,0x00,0xc9,0xf1,0x00,0x00,0xc9,0xf7,0x00,0x00,
        0xca,0x12,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,
        0x00,0x1b,0x00,0x00,0xca,0x14,0x00,0x00,0xca,0x20,0x00,0x00,0xca,0x37,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x17,0x00,0x00,
        0xca,0x39,0x00,0x00,0xca,0x45,0x00,0x00,0xca,0x5c,0x00,0x03,0x00,0x06,0x00,0x00,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x17,0x00,0x00,0xca,0x69,0x00,0x00,
        0xca,0x6b,0x00,0x00,0xca,0x6c,0x00,0x03,0x00,0x06,0x00,0x02,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x02,0x00,0x01,0x00,0x00,0xca,0x6d,0x00,0x00,0xca,0x79,0x00,0x00,
        0xca,0x93,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,
    ];
    for (i, &b) in src_bytes.iter().enumerate() { mem_real.write8(src_base + i as u32, b); mem_ref.write8(src_base + i as u32, b); }

    let rom_base: u32 = 0xe2aa00;
    let rom_bytes: &[u8] = &[
        0x02,0xfa,0x03,0x00,0x03,0x06,0x03,0x0c,0x03,0x12,0x03,0x18,0x03,0x1e,0x03,0x24,
        0x03,0x2a,0x03,0x30,0x03,0x36,0x03,0x3c,0x03,0x42,0x03,0x48,0x03,0x4e,0x03,0x54,
        0x03,0x5a,0x03,0x60,0x03,0x66,0x03,0x6c,0x03,0x72,0x03,0x78,0x03,0x7e,0x03,0x84,
        0x03,0x8a,0x03,0x90,0x03,0x96,0x03,0x9c,0x03,0xa2,0x03,0xa8,0x03,0xae,0x03,0xb4,
        0x03,0xba,0x03,0xc0,0x03,0xc6,0x03,0xcc,0x03,0xd2,0x03,0xd8,0x03,0xde,0x03,0xe4,
        0x03,0xea,0x03,0xf0,0x03,0xf6,0x03,0xfc,0x04,0x02,0x04,0x08,0x04,0x0e,0x04,0x14,
        0x04,0x1a,0x04,0x20,0x04,0x26,0x04,0x2c,0x04,0x32,0x04,0x38,0x04,0x3e,0x04,0x44,
        0x04,0x4a,0x04,0x50,0x04,0x56,0x04,0x5c,0x04,0x62,0x04,0x68,0x04,0x6e,0x04,0x74,
        0x04,0x7a,0x04,0x80,0x04,0x86,0x04,0x8c,0x04,0x92,0x04,0x98,0x04,0x9e,0x04,0xa4,
        0x04,0xaa,0x04,0xb0,0x04,0xb6,0x04,0xbc,0x04,0xc2,0x04,0xc8,0x04,0xce,0x04,0xd4,
        0x04,0xda,0x04,0xe0,0x04,0xe6,0x04,0xec,0x04,0xf2,0x04,0xf8,0x04,0xfe,0x05,0x04,
        0x05,0x0a,0x05,0x10,0x05,0x16,0x05,0x1c,0x05,0x22,0x05,0x28,0x05,0x2e,0x05,0x34,
        0x05,0x3a,0x05,0x40,0x05,0x46,0x05,0x4c,0x05,0x52,0x05,0x58,0x05,0x5e,0x05,0x64,
        0x05,0x6a,0x05,0x70,0x05,0x76,0x05,0x7c,0x05,0x82,0x05,0x88,0x05,0x8e,0x05,0x94,
        0x05,0x9a,0x05,0xa0,0x05,0xa6,0x05,0xac,0x05,0xb2,0x05,0xb8,0x05,0xbe,0x05,0xc4,
        0x05,0xca,0x05,0xd0,0x05,0xd6,0x05,0xdc,0x05,0xe2,0x05,0xe8,0x05,0xee,0x05,0xf4,
        0x05,0xfa,0x06,0x00,0x00,0x82,0x04,0x21,0xcf,0xb6,0x0d,0xe3,0x04,0xe3,0x81,0x50,
        0xf9,0x87,0xbc,0xc3,0xcc,0x3e,0x73,0xe0,0x38,0x1f,0x84,0x42,0x00,0xcd,0x94,0x7b,
        0x26,0x0c,0x31,0x84,0x88,0x00,0x00,0x06,0x70,0x4f,0x3c,0x33,0xc7,0x3e,0x71,0xc3,
        0x0c,0x18,0x06,0x1c,0x71,0xcf,0x1e,0xf3,0xef,0x9e,0x89,0xc0,0x92,0x42,0x28,0x9c,
        0xf1,0xcf,0x1e,0xfa,0x28,0xa2,0x8a,0x2f,0x9e,0xc1,0xe2,0x00,0x60,0x08,0x00,0x08,
        0x01,0x80,0x80,0x01,0x20,0x60,0x00,0x00,0x00,0x00,0x00,0x20,0x00,0x00,0x00,0x00,
        0x0e,0x31,0xc4,0x00,0x79,0x41,0x08,0x51,0x02,0x00,0x21,0x44,0x14,0x21,0x05,0x08,
        0x20,0x07,0x88,0x51,0x02,0x10,0x51,0x45,0x04,0x1a,0x2f,0x06,0x10,0x41,0x04,0xf1,
        0xe7,0x1c,0x60,0x00,0x30,0xc0,0xc3,0x6c,0x69,0xa3,0x42,0x01,0xe4,0x1a,0x69,0x41,
        0x08,0x69,0xe7,0xbd,0x4b,0xa9,0xbc,0x7b,0xff,0x1c,0x7b,0xfc,0x1e,0xf3,0x0d,0x8e,
        0xf9,0xbf,0xb6,0xf9,0xcd,0x5e,0x3b,0xe0,0x3e,0xd8,0xc0,0x00,0x01,0xcf,0xc0,0xf8,
        0xe0,0x00,0x71,0xc7,0x0c,0x18,0x83,0x8c,0x78,0x86,0x06,0x0c,0xc2,0x1a,0x30,0xc0,
        0x00,0x71,0xc7,0x3e,0x01,0xc2,0x06,0x62,0xaf,0x2a,0x1a,0x17,0x86,0x82,0x01,0x50,
        0xc8,0x80,0x84,0xc2,0x0c,0x02,0x53,0x67,0x20,0x3f,0x42,0xf4,0x00,0xcd,0xbe,0xa3,
        0x4d,0x0c,0x60,0xc3,0x08,0x00,0x00,0x0c,0x98,0xc0,0x82,0x52,0x08,0x02,0x8a,0x23,
        0x0c,0x31,0xe3,0x26,0x8a,0x28,0xa0,0x8a,0x08,0x20,0x88,0x80,0x94,0x43,0x6c,0xa2,
        0x8a,0x28,0xa0,0x22,0x28,0xa2,0x52,0x21,0x18,0x60,0x67,0x00,0x61,0xcf,0x1c,0x79,
        0xc2,0x1e,0xb1,0x81,0x24,0x21,0x4f,0x1c,0xf1,0xe7,0x0e,0x72,0x28,0xa2,0x4a,0x27,
        0x8c,0x30,0xce,0x88,0x80,0x02,0x14,0x00,0x80,0x1e,0x50,0x02,0x00,0x50,0x80,0x00,
        0xfb,0xca,0x14,0x00,0x85,0x08,0x00,0x00,0x0e,0x23,0x6d,0x88,0x20,0x82,0x08,0x00,
        0x00,0xa2,0x00,0x00,0x30,0xc0,0x06,0xf6,0xb2,0xc4,0x8c,0x72,0xc2,0x2c,0xb0,0x02,
        0x1c,0xeb,0x38,0xd7,0x01,0x2d,0x8c,0x08,0x61,0x8c,0x31,0xbd,0x86,0x1b,0xe7,0xc6,
        0xd9,0xb9,0xb6,0x18,0x6d,0x56,0x18,0x6f,0xe6,0xd8,0xa2,0x16,0x6b,0x66,0xfe,0x61,
        0xc6,0xbe,0x73,0x6d,0x9a,0x21,0xc4,0x12,0x03,0xe1,0x98,0x10,0xc0,0x2c,0x49,0xe0,
        0x07,0x68,0x63,0x00,0x03,0x62,0x3b,0xdf,0x6e,0x1c,0xb2,0x97,0x84,0xde,0xe1,0x50,
        0xc8,0x8f,0xbe,0xc3,0xef,0x8e,0x73,0x20,0xb7,0x60,0x62,0x94,0x00,0xc9,0x14,0x70,
        0x86,0x18,0x60,0xc7,0xbe,0x01,0xe0,0x18,0xa8,0x47,0x1c,0x93,0xcf,0x04,0x71,0xe0,
        0x00,0x60,0x01,0x8c,0xbb,0xef,0x20,0x8b,0xcf,0x26,0xf8,0x80,0x98,0x42,0xaa,0xa2,
        0xf2,0x2f,0x1c,0x22,0x28,0xaa,0x21,0x42,0x18,0x30,0x6d,0x80,0x30,0x28,0xa0,0x8b,
        0xe7,0xa2,0xc8,0x81,0x38,0x23,0xe8,0xa2,0x8a,0x24,0x98,0x22,0x28,0xaa,0x32,0x21,
        0x18,0x30,0x6b,0x9c,0x82,0x27,0x1c,0x71,0xc7,0x20,0x71,0xc7,0x18,0x21,0x87,0x1c,
        0x80,0xef,0x9c,0x71,0xc8,0xa2,0x89,0xc8,0x98,0x71,0xcf,0x1e,0x71,0x87,0x22,0xf1,
        0x2f,0xa2,0x61,0xe7,0xb6,0xcc,0xcd,0x9b,0x71,0xc5,0x96,0xba,0xe7,0x1c,0x70,0x00,
        0x08,0xea,0xdb,0x55,0x49,0x27,0x0c,0x18,0x6d,0x8c,0x19,0xbd,0x86,0x18,0x66,0x46,
        0xd9,0xbd,0x9c,0xd8,0x6f,0x56,0x18,0x66,0xf6,0x71,0xc7,0x2d,0xd3,0xc6,0x54,0x33,
        0x66,0x8c,0xab,0xed,0x9c,0x72,0xa7,0x92,0x78,0x86,0x06,0x30,0xcf,0x80,0x30,0xc3,
        0x04,0x68,0xc1,0x00,0x00,0x8d,0x86,0x62,0xac,0xaa,0xe2,0xdf,0xdc,0x93,0xa3,0x58,
        0xd9,0xcc,0x06,0xd8,0x69,0x8c,0xdb,0xef,0xa4,0x40,0x21,0x68,0x00,0xc0,0x3e,0x29,
        0x6e,0x80,0x60,0xc3,0x08,0x30,0x03,0x30,0xc8,0x48,0x02,0xf8,0x28,0x88,0x88,0x23,
        0x0c,0x31,0xe3,0x0c,0xb2,0x28,0xa0,0x8a,0x08,0x22,0x88,0x88,0x94,0x42,0x29,0xa2,
        0x82,0x2a,0x02,0x22,0x25,0x36,0x50,0x84,0x18,0x18,0x60,0x00,0x03,0xe8,0xa0,0x8a,
        0x02,0x1e,0x88,0x81,0x24,0x22,0xa8,0xa2,0x8a,0x24,0x06,0x22,0x25,0x2a,0x31,0xe2,
        0x0c,0x30,0xc1,0x32,0x82,0x2f,0x82,0x08,0x20,0xa0,0xfb,0xef,0x88,0x20,0x88,0xa2,
        0xf3,0x8a,0x22,0x8a,0x28,0xa2,0x7a,0x28,0x8e,0x20,0x8d,0x88,0x08,0x88,0xa2,0x89,
        0xa7,0x9c,0x61,0x00,0x8b,0x14,0xc6,0xf6,0x0a,0x26,0x9a,0xa2,0xc8,0xa2,0x88,0x00,
        0x08,0x6a,0xf8,0xc0,0x4b,0xad,0x8c,0x38,0x6d,0x8c,0x19,0xbd,0x80,0x18,0x66,0x46,
        0xd9,0xb1,0x8e,0xd0,0x6c,0x56,0x18,0x66,0xc6,0x31,0xcd,0xad,0xd3,0x66,0x14,0x63,
        0x66,0x8c,0xab,0x65,0x36,0xaa,0xa4,0x12,0x00,0x00,0x00,0x30,0xc0,0x1a,0x00,0x03,
        0x34,0x69,0xe7,0x00,0x00,0x87,0x04,0x21,0xc9,0xb6,0x42,0x10,0x3c,0x18,0xe7,0x5c,
        0xd9,0xcc,0x06,0xf8,0x6d,0x8c,0xd8,0x67,0x3c,0x71,0xee,0xf0,0x00,0x00,0x14,0xf2,
        0x6d,0x00,0x31,0x84,0x88,0x30,0x03,0x20,0x70,0x4f,0xbc,0x13,0xc7,0x08,0x71,0xc3,
        0x04,0x18,0x06,0x00,0x82,0x2f,0x1e,0xf3,0xe8,0x1e,0x89,0xc7,0x12,0x7a,0x28,0x9c,
        0x81,0xc9,0xbc,0x21,0xe2,0x22,0x88,0x8f,0x9e,0x09,0xe0,0x00,0x01,0xef,0x1c,0x79,
        0xc2,0x02,0x89,0xc1,0x22,0x72,0x28,0x9c,0xf1,0xe4,0x1c,0x11,0xe2,0x36,0x48,0x27,
        0x8e,0x31,0xc0,0x3e,0x7a,0x28,0x3e,0xfb,0xef,0x9e,0x82,0x08,0x08,0x20,0x8f,0xbe,
        0x81,0xeb,0xa2,0x8a,0x28,0xa2,0x0a,0x28,0x84,0x79,0xcf,0x08,0xf8,0x88,0xa2,0x89,
        0x60,0x00,0xc9,0x00,0x86,0x3c,0xc3,0x6c,0xfa,0x24,0x8c,0x79,0xef,0xbe,0x88,0x00,
        0x08,0x2b,0x1a,0x40,0x48,0x2c,0xbe,0x68,0x6d,0x8c,0x19,0xbf,0x80,0xf1,0xe6,0xde,
        0x73,0xff,0xbe,0xc0,0x6f,0xf6,0x18,0x67,0xc6,0x32,0x88,0x9a,0x6b,0xc6,0x14,0xc1,
        0xc7,0xcc,0x71,0xcd,0xb6,0x71,0xc3,0x92,0x7b,0xe7,0x9e,0x30,0x82,0x2c,0x00,0x00,
        0x1c,0x00,0x00,0x00,0x00,0x82,0x00,0x00,0x00,0x00,0x01,0xe3,0x18,0x10,0xb6,0x4c,
        0xf9,0xcf,0xbe,0x1b,0xef,0x8c,0xf8,0x60,0x07,0x58,0xac,0x00,0x00,0xc0,0x00,0x20,
        0x06,0x80,0x00,0x00,0x00,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x08,0x00,0x00,0x0c,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x3e,0x00,0x00,0x00,0x00,
        0x00,0x3c,0x00,0x0e,0x00,0x00,0x00,0x00,0x80,0x20,0x00,0x00,0x00,0x00,0x03,0xc0,
        0x00,0x30,0x00,0x00,0xc1,0xe7,0x1e,0x79,0xe7,0xb8,0x71,0xc7,0x1c,0x71,0xc8,0xa2,
        0xf8,0x00,0x1c,0x71,0xc7,0x9e,0xf1,0xc7,0x80,0x00,0x8c,0x30,0x79,0xc7,0x1e,0x89,
        0x2f,0xbe,0x70,0x00,0x0f,0x04,0xc0,0x00,0x79,0xcb,0x10,0x00,0x08,0xa2,0x70,0x00,
        0x00,0x29,0xe7,0x80,0x10,0xc0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0xc0,0x00,0x00,0x00,0x00,0x00,0x01,0x80,0x00,0x03,0x06,0x00,0xf8,
        0x0c,0x08,0x70,0x00,0x1c,0xc0,0x80,0x00,0x00,0x00,0x00,0x33,0x00,0x00,0x00,0x00,
    ];
    for (i, &b) in rom_bytes.iter().enumerate() { mem_real.write8(rom_base + i as u32, b); mem_ref.write8(rom_base + i as u32, b); }

    let dst_base: u32 = 0xf8f00;
    let dst_before: &[u8] = &[
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x08,0x00,0xf8,0x00,0x08,0x00,0x08,0x00,
        0x00,0x26,0x00,0x26,0x00,0x26,0x00,0x26,0x30,0x00,0x3f,0xff,0x30,0x00,0x30,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x08,0x38,0xf8,0x38,0x08,0x38,0x08,0x38,
        0x00,0x2c,0x00,0x2c,0x00,0x2c,0x00,0x2c,0x50,0x00,0x5f,0xff,0x50,0x00,0x50,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x08,0x44,0xf8,0x44,0x08,0x44,0x08,0x44,
        0x00,0x38,0x00,0x38,0x00,0x38,0x00,0x38,0x90,0x00,0x9f,0xff,0x90,0x00,0x90,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
    ];
    for (i, &b) in dst_before.iter().enumerate() { mem_real.write8(dst_base + i as u32, b); mem_ref.write8(dst_base + i as u32, b); }

    let mut bl = Blitter::new();
    let mut ref_buffer = 0u32;
    // call #0
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 42, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f86e0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #0 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #1
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88c8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 39, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f88c8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #1 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #2
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 36, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8ab0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #2 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #3
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c98);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 33, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8c98, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #3 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #4
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e80);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 30, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8e80, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #4 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #5
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 26, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f90e0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #5 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #6
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92c8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 23, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f92c8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #6 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #7
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 20, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f94b0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #7 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #8
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9698);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 17, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9698, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #8 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #9
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9880);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 14, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9880, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #9 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #10
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 10, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9ae0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #10 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #11
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cc8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 7, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9cc8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #11 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #12
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 4, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9eb0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #12 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #13
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa098);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 1, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa098, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #13 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #14
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa120);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 5, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa120, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #14 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #15
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 42, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f86e2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #15 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #16
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88ca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 39, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f88ca, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #16 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #17
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 36, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8ab2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #17 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #18
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c9a);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 33, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8c9a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #18 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #19
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e82);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 30, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8e82, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #19 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #20
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 26, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f90e2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #20 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #21
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 23, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f92ca, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #21 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #22
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 20, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f94b2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #22 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #23
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f969a);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 17, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f969a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #23 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #24
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9882);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 14, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9882, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #24 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #25
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 10, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9ae2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #25 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #26
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 7, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9cca, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #26 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #27
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 4, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9eb2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #27 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #28
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa09a);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 1, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa09a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #28 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #29
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa122);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 5, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa122, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #29 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #30
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 42, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f86e4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #30 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #31
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88cc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 39, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f88cc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #31 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #32
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 36, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8ab4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #32 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #33
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c9c);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 33, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8c9c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #33 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #34
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e84);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 30, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8e84, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #34 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #35
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 26, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f90e4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #35 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #36
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92cc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 23, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f92cc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #36 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #37
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 20, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f94b4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #37 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #38
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f969c);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 17, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f969c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #38 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #39
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9884);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 14, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9884, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #39 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #40
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 10, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9ae4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #40 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #41
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ccc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 7, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9ccc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #41 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #42
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 4, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9eb4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #42 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #43
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa09c);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 1, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa09c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #43 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #44
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa124);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 5, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa124, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #44 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #45
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 42, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f86e6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #45 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #46
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88ce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 39, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f88ce, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #46 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #47
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 36, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8ab6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #47 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #48
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c9e);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 33, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8c9e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #48 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #49
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e86);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 30, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f8e86, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #49 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #50
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 26, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f90e6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #50 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #51
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 23, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f92ce, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #51 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #52
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 20, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f94b6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #52 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #53
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f969e);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 17, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f969e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #53 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #54
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9886);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 7, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 1, y_count: 14, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9886, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #54 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #55
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 10, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9ae6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #55 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #56
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 7, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9cce, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #56 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #57
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 4, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0f9eb6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #57 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #58
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa09e);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 4, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 2, y_count: 1, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa09e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #58 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #59
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa126);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x0, skew_reg: 0x00, control_line: 5, smudge: false, src_x_inc: 0, src_y_inc: 0, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x000000, 0x0fa126, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #59 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #60
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a58);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x00, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f9a58, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #60 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #61
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d758);
    wl(&mut bl, reg::DST_ADDR, 0x0f9730);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d758, 0x0f9730, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #61 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #62
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d742);
    wl(&mut bl, reg::DST_ADDR, 0x0f9408);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d742, 0x0f9408, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #62 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #63
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d72e);
    wl(&mut bl, reg::DST_ADDR, 0x0f9058);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d72e, 0x0f9058, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #63 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #64
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d718);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d30);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d718, 0x0f8d30, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #64 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #65
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d702);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a08);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d702, 0x0f8a08, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #65 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #66
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6ee);
    wl(&mut bl, reg::DST_ADDR, 0x0f8658);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6ee, 0x0f8658, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #66 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #67
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f9a5a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #67 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #68
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d762);
    wl(&mut bl, reg::DST_ADDR, 0x0f9732);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d762, 0x0f9732, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #68 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #69
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d74c);
    wl(&mut bl, reg::DST_ADDR, 0x0f940a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d74c, 0x0f940a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #69 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #70
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d738);
    wl(&mut bl, reg::DST_ADDR, 0x0f905a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d738, 0x0f905a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #70 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #71
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d722);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d32);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d722, 0x0f8d32, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #71 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #72
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d70c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d70c, 0x0f8a0a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #72 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #73
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f865a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f865a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #73 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #74
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f9a5c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #74 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #75
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d762);
    wl(&mut bl, reg::DST_ADDR, 0x0f9734);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d762, 0x0f9734, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #75 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #76
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d74c);
    wl(&mut bl, reg::DST_ADDR, 0x0f940c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d74c, 0x0f940c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #76 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #77
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d738);
    wl(&mut bl, reg::DST_ADDR, 0x0f905c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d738, 0x0f905c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #77 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #78
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d722);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d34);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d722, 0x0f8d34, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #78 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #79
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d70c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d70c, 0x0f8a0c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #79 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #80
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f865c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f865c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #80 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #81
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f9a5e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #81 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #82
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d762);
    wl(&mut bl, reg::DST_ADDR, 0x0f9736);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d762, 0x0f9736, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #82 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #83
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d74c);
    wl(&mut bl, reg::DST_ADDR, 0x0f940e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d74c, 0x0f940e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #83 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #84
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d738);
    wl(&mut bl, reg::DST_ADDR, 0x0f905e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d738, 0x0f905e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #84 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #85
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d722);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d36);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d722, 0x0f8d36, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #85 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #86
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d70c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d70c, 0x0f8a0e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #86 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #87
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f865e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x7, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f865e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #87 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #88
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 8, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9ae0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #88 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #89
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cc8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 5, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9cc8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #89 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #90
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 2, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9eb0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #90 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #91
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 3, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9fe0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #91 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #92
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 8, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9ae2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #92 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #93
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 5, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9cca, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #93 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #94
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 2, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9eb2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #94 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #95
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 3, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9fe2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #95 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #96
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 8, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9ae4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #96 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #97
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ccc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 5, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9ccc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #97 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #98
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 2, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9eb4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #98 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #99
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 3, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9fe4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #99 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #100
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 11, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 8, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9ae6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #100 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #101
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 14, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 4, y_count: 5, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9cce, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #101 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #102
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 1, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 3, y_count: 2, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9eb6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #102 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #103
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 1, op: 0x3, skew_reg: 0x00, control_line: 3, smudge: false, src_x_inc: 0, src_y_inc: -2, dst_x_inc: 8, dst_y_inc: 128, x_count: 5, y_count: 0, endmask: [0xffff,0xffff,0xff00], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d6f8, 0x0f9fe6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #103 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #104
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a58);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x00, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7f8, 0x0f9a58, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #104 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #105
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7d8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9730);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7d8, 0x0f9730, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #105 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #106
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7c2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9408);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7c2, 0x0f9408, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #106 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #107
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7ae);
    wl(&mut bl, reg::DST_ADDR, 0x0f9058);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7ae, 0x0f9058, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #107 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #108
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d798);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d30);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d798, 0x0f8d30, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #108 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #109
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d782);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a08);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d782, 0x0f8a08, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #109 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #110
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d76e);
    wl(&mut bl, reg::DST_ADDR, 0x0f8658);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d76e, 0x0f8658, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #110 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #111
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7f8, 0x0f9a5a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #111 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #112
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7e2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9732);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7e2, 0x0f9732, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #112 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #113
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7cc);
    wl(&mut bl, reg::DST_ADDR, 0x0f940a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7cc, 0x0f940a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #113 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #114
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7b8);
    wl(&mut bl, reg::DST_ADDR, 0x0f905a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7b8, 0x0f905a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #114 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #115
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7a2);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d32);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7a2, 0x0f8d32, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #115 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #116
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d78c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d78c, 0x0f8a0a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #116 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #117
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f865a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f865a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #117 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #118
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7f8, 0x0f9a5c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #118 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #119
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7e2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9734);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7e2, 0x0f9734, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #119 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #120
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7cc);
    wl(&mut bl, reg::DST_ADDR, 0x0f940c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7cc, 0x0f940c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #120 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #121
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7b8);
    wl(&mut bl, reg::DST_ADDR, 0x0f905c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7b8, 0x0f905c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #121 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #122
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7a2);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d34);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7a2, 0x0f8d34, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #122 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #123
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d78c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d78c, 0x0f8a0c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #123 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #124
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f865c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f865c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #124 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #125
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 32, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7f8, 0x0f9a5e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #125 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #126
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7e2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9736);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 27, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7e2, 0x0f9736, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #126 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #127
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7cc);
    wl(&mut bl, reg::DST_ADDR, 0x0f940e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 22, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7cc, 0x0f940e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #127 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7b8);
    wl(&mut bl, reg::DST_ADDR, 0x0f905e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 16, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7b8, 0x0f905e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #128 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #129
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7a2);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d36);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 11, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 2, y_count: 11, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d7a2, 0x0f8d36, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #129 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #130
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d78c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 6, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 1, y_count: 6, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d78c, 0x0f8a0e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #130 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #131
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f865e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -2, dst_x_inc: -8, dst_y_inc: -144, x_count: 3, y_count: 0, endmask: [0xf000,0xffff,0x0fff], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0x00d778, 0x0f865e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #131 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #132
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92c8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x44, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef4, 0x0f92c8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #132 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #133
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f08);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa74, 0x0f8f08, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #133 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #134
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ca);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef4, 0x0f92ca, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #134 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #135
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f0a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa74, 0x0f8f0a, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #135 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #136
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92cc);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef4, 0x0f92cc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #136 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #137
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f0c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa74, 0x0f8f0c, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #137 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #138
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ce);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef4, 0x0f92ce, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #138 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #139
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f0e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x007e,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa74, 0x0f8f0e, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #139 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #140
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ea8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x03, control_line: 0, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9ea8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #140 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #141
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 10, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9ae8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #141 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #142
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eaa);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 0, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9eaa, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #142 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #143
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aea);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 10, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9aea, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #143 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eac);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 0, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9eac, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #144 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #145
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aec);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 10, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9aec, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #145 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #146
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eae);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 0, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9eae, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #146 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #147
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aee);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 10, smudge: false, src_x_inc: -6, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x3f00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9aee, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #147 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #148
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ea8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0a, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aefa, 0x0f9ea8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #148 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #149
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa7a, 0x0f9ae8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #149 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #150
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eaa);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aefa, 0x0f9eaa, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #150 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #151
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aea);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa7a, 0x0f9aea, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #151 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #152
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eac);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aefa, 0x0f9eac, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #152 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #153
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aec);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa7a, 0x0f9aec, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #153 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #154
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eae);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aefa, 0x0f9eae, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #154 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #155
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aee);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x00fc,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa7a, 0x0f9aee, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #155 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #156
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ea8);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x02, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 6, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9ea8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #156 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #157
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa8e);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae8);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 0, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa8e, 0x0f9ae8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #157 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #158
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eaa);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 6, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eaa, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #158 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #159
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aea);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 0, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa82, 0x0f9aea, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #159 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eac);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 6, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eac, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #160 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #161
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aec);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 0, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa82, 0x0f9aec, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #161 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #162
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eae);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 6, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eae, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #162 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #163
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aee);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -168, x_count: 2, y_count: 0, endmask: [0x0003,0xffff,0xf000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa82, 0x0f9aee, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #163 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #164
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x4c, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af00, 0x0f9eb0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #164 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #165
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 10, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa80, 0x0f9af0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #165 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #166
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af00, 0x0f9eb2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #166 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #167
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 10, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa80, 0x0f9af2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #167 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af00, 0x0f9eb4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #168 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #169
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 10, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa80, 0x0f9af4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #169 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #170
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 0, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af00, 0x0f9eb6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #170 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #171
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 10, smudge: false, src_x_inc: -2, src_y_inc: -192, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x0fc0,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa80, 0x0f9af6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #171 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #172
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x0e, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eb0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #172 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #173
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9af0, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #173 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #174
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eb2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #174 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #175
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa82, 0x0f9af2, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #175 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #176
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eb4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #176 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #177
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa82, 0x0f9af4, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #177 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #178
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2af02, 0x0f9eb6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #178 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #179
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0x003f,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa82, 0x0f9af6, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #179 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #180
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x8c, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9eb8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #180 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #181
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9af8, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #181 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #182
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eba);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9eba, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #182 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #183
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9afa);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9afa, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #183 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #184
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ebc);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9ebc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #184 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #185
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9afc);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9afc, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #185 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #186
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ebe);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 0, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 6, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aef6, 0x0f9ebe, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #186 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    // call #187
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9afe);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    run_to_completion(&mut bl, &mut mem_real);
    let call = Cfg { hop: 2, op: 0x4, skew_reg: 0x82, control_line: 10, smudge: false, src_x_inc: 2, src_y_inc: -194, dst_x_inc: 8, dst_y_inc: -160, x_count: 1, y_count: 0, endmask: [0xfc00,0xffff,0x0000], halftone: [0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff] };
    hatari_reference_execute_chained(&call, 0xe2aa76, 0x0f9afe, &mut mem_ref, &mut ref_buffer);
    for a in 0xf8f00u32..=0xf90cfu32 {
        if mem_real.read8(a) != mem_ref.read8(a) {
            println!("PREMIERE DIVERGENCE RUST68-VS-REFERENCE au call #187 addr={:#08x} reel={:#04x} reference={:#04x}", a, mem_real.read8(a), mem_ref.read8(a));
            std::process::exit(1);
        }
    }
    println!("AUCUNE divergence entre notre Blitter et la reference sur toute la sequence.");
}