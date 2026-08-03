#![cfg(feature = "atari-st")]
//! Test différentiel : compare `Blitter::execute` à un portage direct, en
//! Rust, de la machine à états de Hatari (`src/blitter.c`,
//! `Blitter_ProcessWord`/`Blitter_SourceShift`/`Blitter_SourceFetch`/
//! `Blitter_LOP_*`), sur un large balayage de configurations (HOP, OP,
//! skew, FXSR, NFSR, direction, X_COUNT/Y_COUNT, masques).
//!
//! Contrairement aux tests unitaires de `tests/blitter.rs` (qui vérifient
//! des propriétés isolées), celui-ci compare directement, octet par octet,
//! la mémoire écrite par les deux implémentations pour un très grand
//! nombre de combinaisons — objectif : trouver une divergence qu'une
//! relecture manuelle du code n'a pas révélée (corruption encore observée
//! en pratique sur TOS 1.62/STE malgré une relecture croisée exhaustive du
//! code face à `hatari_blitter.c`).

use rust68::peripherals::atari_st::blitter::{Blitter, reg};
use rust68::Bus;

/// Bus de test léger (par opposition à `FlatBus`, qui alloue 16 Mo — bien
/// trop coûteux répété sur plusieurs milliers de configurations dans le
/// balayage ci-dessous). Adressage modulo la taille du tampon : suffisant
/// puisque toutes les adresses utilisées restent dans une plage étroite
/// autour de `SRC_BASE`/`DST_BASE`.
struct SmallBus {
    mem: Vec<u8>,
}

impl SmallBus {
    fn new() -> Self {
        SmallBus {
            mem: vec![0; 0x10000],
        }
    }
}

impl Bus for SmallBus {
    fn read8(&mut self, addr: u32) -> u8 {
        self.mem[(addr as usize) % self.mem.len()]
    }
    fn write8(&mut self, addr: u32, value: u8) {
        let len = self.mem.len();
        self.mem[(addr as usize) % len] = value;
    }
}

fn write_word(bl: &mut Blitter, offset: u32, value: u16) {
    bl.write_word(offset, value);
}

fn write_long(bl: &mut Blitter, offset: u32, value: u32) {
    bl.write_long(offset, value);
}

#[derive(Clone, Copy, Debug)]
struct Cfg {
    hop: u8,
    op: u8,
    skew_reg: u8, // octet brut : bit7=FXSR, bit6=NFSR, bits3-0=décalage
    control_line: u8, // bits 3-0 du CONTROL initial (numéro de ligne halftone)
    smudge: bool,
    src_x_inc: i16,
    src_y_inc: i16,
    dst_x_inc: i16,
    dst_y_inc: i16,
    x_count: u16,
    y_count: u16,
    endmask: [u16; 3],
    halftone: [u16; 16],
}

fn apply_op_reference(op: u8, s: u16, d: u16) -> u16 {
    // Traduction directe des fonctions Blitter_LOP_0..F de hatari_blitter.c
    // (pas la formule "bit-index 3-((s<<1)|d)" utilisée par notre
    // implémentation — vérification indépendante des deux approches).
    match op & 0x0F {
        0x0 => 0,
        0x1 => s & d,
        0x2 => s & !d,
        0x3 => s,
        0x4 => !s & d,
        0x5 => d,
        0x6 => s ^ d,
        0x7 => s | d,
        0x8 => !s & !d,
        0x9 => !(s ^ d),
        0xA => !d,
        0xB => s | !d,
        0xC => !s,
        0xD => !s | d,
        0xE => !s | !d,
        0xF => 0xFFFF,
        _ => unreachable!(),
    }
}

/// Portage direct de la machine à états de `Blitter_ProcessWord` (Hatari
/// `src/blitter.c`). Traite le blit dans son intégralité (equivalent à
/// enchaîner tous les appels non-HOG jusqu'à la fin, ce qui ne change rien
/// au résultat final puisque notre propre modèle est de toute façon
/// synchrone/instantané).
fn hatari_reference_execute(cfg: &Cfg, src_addr0: u32, dst_addr0: u32, bus: &mut impl Bus) {
    let mut buffer = 0u32;
    hatari_reference_execute_chained(cfg, src_addr0, dst_addr0, bus, &mut buffer);
}

/// Comme [`hatari_reference_execute`], mais `buffer` (le registre à
/// décalage source) est fourni par l'appelant et mis à jour en place —
/// permet de tester un ENCHAÎNEMENT de blits logiquement séparés qui
/// partagent le même registre matériel persistant (voir
/// `sequence_chainee_avec_nfsr_et_skew`), plutôt qu'un unique blit isolé.
fn hatari_reference_execute_chained(
    cfg: &Cfg,
    src_addr0: u32,
    dst_addr0: u32,
    bus: &mut impl Bus,
    buffer: &mut u32,
) {
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

    let shift = |buffer: &mut u32, src_x_inc: i16| {
        if src_x_inc < 0 {
            *buffer >>= 16;
        } else {
            *buffer <<= 16;
        }
    };
    let fetch = |buffer: &mut u32, src_x_inc: i16, word: u16| {
        if src_x_inc < 0 {
            *buffer |= (word as u32) << 16;
        } else {
            *buffer |= word as u32;
        }
    };

    while y_count > 0 {
        let first_word = x_count == x_count_reset;
        if first_word {
            nfsr_dynamic = false;
        }

        let mask = if first_word || x_count_reset == 1 {
            cfg.endmask[0]
        } else if x_count == 1 {
            cfg.endmask[2]
        } else {
            cfg.endmask[1]
        };

        if fxsr_reg && !have_fxsr && need_src {
            shift(buffer, cfg.src_x_inc);
            let w = bus.read16(src_addr & rust68::ADDR_MASK);
            bus_word = w;
            fetch(buffer, cfg.src_x_inc, w);
            src_addr = src_addr.wrapping_add(cfg.src_x_inc as i32 as u32);
            have_fxsr = true;
        }

        let mut fetch_src = false;
        if need_src && !nfsr_dynamic {
            shift(buffer, cfg.src_x_inc);
            let w = bus.read16(src_addr & rust68::ADDR_MASK);
            bus_word = w;
            fetch(buffer, cfg.src_x_inc, w);
            fetch_src = true;
        }

        let weird_case = nfsr_reg && x_count == 1;
        if weird_case {
            shift(buffer, cfg.src_x_inc);
            fetch(buffer, cfg.src_x_inc, bus_word);
        }

        let source_val = (*buffer >> skew) as u16;
        let halftone_word = if cfg.smudge {
            cfg.halftone[(source_val & 0x0F) as usize]
        } else {
            cfg.halftone[halftone_line]
        };
        let hop_result = match cfg.hop & 0x3 {
            0 => 0xFFFF,
            1 => halftone_word,
            2 => source_val,
            3 => source_val & halftone_word,
            _ => unreachable!(),
        };
        let dst_word = bus.read16(dst_addr & rust68::ADDR_MASK);
        let lop = apply_op_reference(cfg.op, hop_result, dst_word);
        let dst_data = if mask != 0xFFFF {
            (lop & mask) | (dst_word & !mask)
        } else {
            lop
        };
        bus.write16(dst_addr & rust68::ADDR_MASK, dst_data);

        if weird_case {
            shift(buffer, cfg.src_x_inc);
            fetch(buffer, cfg.src_x_inc, bus_word);
        }

        if x_count == 2 && nfsr_reg {
            nfsr_dynamic = true;
        }

        if fetch_src {
            if x_count == 1 || nfsr_dynamic {
                src_addr = src_addr.wrapping_add(cfg.src_y_inc as i32 as u32);
            } else {
                src_addr = src_addr.wrapping_add(cfg.src_x_inc as i32 as u32);
            }
        }

        if x_count == 1 {
            have_fxsr = false;
            y_count -= 1;
            x_count = x_count_reset;
            dst_addr = dst_addr.wrapping_add(cfg.dst_y_inc as i32 as u32);
            halftone_line = if cfg.dst_y_inc >= 0 {
                (halftone_line + 1) & 15
            } else {
                halftone_line.wrapping_sub(1) & 15
            };
        } else {
            x_count -= 1;
            dst_addr = dst_addr.wrapping_add(cfg.dst_x_inc as i32 as u32);
        }
    }
}

const SRC_BASE: u32 = 0x2000;
const DST_BASE: u32 = 0x5000;

fn run_case(cfg: Cfg) -> Option<String> {
    // Mémoire source : motif varié (pas uniforme) pour que toute erreur de
    // décalage/lecture se voie. Mémoire destination : autre motif varié
    // (pas 0 partout) pour exercer réellement les masques/OP dépendant de D.
    let make_bus = || {
        let mut bus = SmallBus::new();
        for i in 0..64u32 {
            let sv = (0x1357u32.wrapping_mul(i + 1) & 0xFFFF) as u16;
            bus.write16(SRC_BASE + i * 2, sv);
            let dv = (0x9E37u32.wrapping_mul(i + 3) & 0xFFFF) as u16;
            bus.write16(DST_BASE + i * 2, dv);
        }
        bus
    };

    let mut bus_real = make_bus();
    let mut bus_ref = make_bus();

    // --- Notre implémentation, via l'API registre publique ---
    let mut bl = Blitter::new();
    bl.write(reg::HOP, cfg.hop);
    bl.write(reg::OP, cfg.op);
    bl.write(reg::SKEW, cfg.skew_reg);
    bl.write(reg::CONTROL, cfg.control_line & 0x0F);
    write_word(&mut bl, reg::SRC_X_INC, cfg.src_x_inc as u16);
    write_word(&mut bl, reg::SRC_Y_INC, cfg.src_y_inc as u16);
    write_word(&mut bl, reg::DST_X_INC, cfg.dst_x_inc as u16);
    write_word(&mut bl, reg::DST_Y_INC, cfg.dst_y_inc as u16);
    write_word(&mut bl, reg::ENDMASK_1, cfg.endmask[0]);
    write_word(&mut bl, reg::ENDMASK_2, cfg.endmask[1]);
    write_word(&mut bl, reg::ENDMASK_3, cfg.endmask[2]);
    for (i, &w) in cfg.halftone.iter().enumerate() {
        write_word(&mut bl, reg::HALFTONE_BASE + (i as u32) * 2, w);
    }
    write_long(&mut bl, reg::SRC_ADDR, SRC_BASE);
    write_long(&mut bl, reg::DST_ADDR, DST_BASE);
    write_word(&mut bl, reg::X_COUNT, cfg.x_count);
    write_word(&mut bl, reg::Y_COUNT, cfg.y_count); // arme le blit (dernier registre écrit)
    bl.execute(&mut bus_real);

    // --- Référence Hatari ---
    hatari_reference_execute(&cfg, SRC_BASE, DST_BASE, &mut bus_ref);

    // --- Comparaison de toute la zone mémoire potentiellement touchée ---
    let span = 64 * 2;
    for base in [SRC_BASE, DST_BASE] {
        for off in 0..span {
            let a = base + off;
            let (vr, vh) = (bus_real.read8(a), bus_ref.read8(a));
            if vr != vh {
                return Some(format!(
                    "divergence addr={a:#06x} rust68={vr:#04x} hatari_ref={vh:#04x} cfg={cfg:?}"
                ));
            }
        }
    }
    None
}

#[test]
fn balayage_differentiel_vs_hatari() {
    let halftone_uniform_ffff = [0xFFFFu16; 16];
    let mut halftone_checker = [0u16; 16];
    for (i, h) in halftone_checker.iter_mut().enumerate() {
        *h = if i % 2 == 0 { 0xAAAA } else { 0x5555 };
    }

    let mut failures = Vec::new();
    let mut total = 0u32;

    for &hop in &[0u8, 1, 2, 3] {
        for &op in &[0x1u8, 0x3, 0x4, 0x6, 0x7, 0x9, 0xB, 0xD] {
            for &skew_nibble in &[0u8, 2, 7, 8, 15] {
                for &(fxsr, nfsr) in &[(false, false), (true, false), (false, true)] {
                    for &(sxi, syi, dxi, dyi) in &[
                        (2i16, 20i16, 2i16, 20i16),
                        (-2i16, -20i16, 2i16, 20i16),
                        (2i16, 20i16, 2i16, -20i16),
                    ] {
                        for &x_count in &[1u16, 2, 4] {
                            for &y_count in &[1u16, 3] {
                                for &halftone in &[halftone_uniform_ffff, halftone_checker] {
                                    let skew_reg = skew_nibble
                                        | if fxsr { 0x80 } else { 0 }
                                        | if nfsr { 0x40 } else { 0 };
                                    let cfg = Cfg {
                                        hop,
                                        op,
                                        skew_reg,
                                        control_line: 3,
                                        smudge: false,
                                        src_x_inc: sxi,
                                        src_y_inc: syi,
                                        dst_x_inc: dxi,
                                        dst_y_inc: dyi,
                                        x_count,
                                        y_count,
                                        endmask: [0xFFFF, 0xFFFF, 0xFFFF],
                                        halftone,
                                    };
                                    total += 1;
                                    if let Some(msg) = run_case(cfg) {
                                        failures.push(msg);
                                        if failures.len() >= 15 {
                                            panic!(
                                                "{} divergence(s) sur {total} cas testés, ex.:\n{}",
                                                failures.len(),
                                                failures.join("\n")
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} divergence(s) sur {total} cas testés :\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Rejoue une SÉQUENCE de blits étroits (X_COUNT=1..3) NFSR+skew non nul,
/// direction négative — motif exact observé en pratique lors du dessin
/// d'une icône ou d'un glyphe colonne par colonne sur TOS 1.62/STE (voir
/// commentaire de `Blitter::execute` sur la persistance de `buffer`) — sur
/// la MÊME instance de `Blitter` (donc `buffer` persiste réellement entre
/// les appels, contrairement à `balayage_differentiel_vs_hatari` qui teste
/// chaque configuration isolément avec un `Blitter::new()` frais). Objectif
/// : trouver une divergence spécifique à l'ENCHAÎNEMENT que le balayage à
/// blits isolés ne peut pas révéler.
#[test]
fn sequence_chainee_avec_nfsr_et_skew() {
    let halftone = [0xFFFFu16; 16];
    let base_cfg = Cfg {
        hop: 2,
        op: 0x7,
        skew_reg: 0x44, // NFSR=1, FXSR=0, skew=4
        control_line: 0,
        smudge: false,
        src_x_inc: -2,
        src_y_inc: -2,
        dst_x_inc: -8,
        dst_y_inc: -144,
        x_count: 0, // écrasé par appel
        y_count: 0, // écrasé par appel
        endmask: [0xF000, 0xFFFF, 0x0FFF],
        halftone,
    };

    // Rejoue exactement les X_COUNT observés dans la trace réelle
    // (3,2,1,3,2,1,3) pour un des quatre plans de bits de l'icône.
    let x_counts = [3u16, 2, 1, 3, 2, 1, 3];

    let mut bl = Blitter::new();
    let mut bus_real = SmallBus::new();
    let mut bus_ref = SmallBus::new();
    // Mémoire source variée (pas uniforme) pour exercer réellement le
    // registre à décalage.
    for i in 0..128u32 {
        let sv = (0x1357u32.wrapping_mul(i + 1) & 0xFFFF) as u16;
        bus_real.write16(SRC_BASE + i * 2, sv);
        bus_ref.write16(SRC_BASE + i * 2, sv);
        let dv = (0x9E37u32.wrapping_mul(i + 3) & 0xFFFF) as u16;
        bus_real.write16(DST_BASE + i * 2, dv);
        bus_ref.write16(DST_BASE + i * 2, dv);
    }

    let mut ref_buffer = 0u32;
    let mut src_addr = SRC_BASE + 100;
    let mut dst_addr = DST_BASE + 100;

    for (call_idx, &xc) in x_counts.iter().enumerate() {
        let cfg = Cfg {
            x_count: xc,
            y_count: 4,
            // Motif réel observé : le tout premier appel de la chaîne
            // utilise SKEW=0 (pas de NFSR), les suivants SKEW=0x44
            // (NFSR=1, décalage=4) — le buffer persistant hérite donc
            // d'un état amorcé SANS décalage avant de commencer à décaler.
            skew_reg: if call_idx == 0 { 0x00 } else { 0x44 },
            ..base_cfg
        };

        bl.write(reg::HOP, cfg.hop);
        bl.write(reg::OP, cfg.op);
        bl.write(reg::SKEW, cfg.skew_reg);
        bl.write(reg::CONTROL, cfg.control_line & 0x0F);
        write_word(&mut bl, reg::SRC_X_INC, cfg.src_x_inc as u16);
        write_word(&mut bl, reg::SRC_Y_INC, cfg.src_y_inc as u16);
        write_word(&mut bl, reg::DST_X_INC, cfg.dst_x_inc as u16);
        write_word(&mut bl, reg::DST_Y_INC, cfg.dst_y_inc as u16);
        write_word(&mut bl, reg::ENDMASK_1, cfg.endmask[0]);
        write_word(&mut bl, reg::ENDMASK_2, cfg.endmask[1]);
        write_word(&mut bl, reg::ENDMASK_3, cfg.endmask[2]);
        write_long(&mut bl, reg::SRC_ADDR, src_addr);
        write_long(&mut bl, reg::DST_ADDR, dst_addr);
        write_word(&mut bl, reg::X_COUNT, cfg.x_count);
        write_word(&mut bl, reg::Y_COUNT, cfg.y_count);
        bl.execute(&mut bus_real);

        hatari_reference_execute_chained(&cfg, src_addr, dst_addr, &mut bus_ref, &mut ref_buffer);

        let span = 128 * 2;
        for off in 0..span {
            let a = DST_BASE + off;
            let (vr, vh) = (bus_real.read8(a), bus_ref.read8(a));
            if vr != vh {
                panic!(
                    "divergence après appel #{call_idx} (x_count={xc}) addr={a:#06x} rust68={vr:#04x} hatari_ref={vh:#04x}"
                );
            }
        }

        // Prépare le prochain appel de la séquence : adresses décroissantes
        // (direction négative), comme dans la trace réelle.
        src_addr = src_addr.wrapping_sub(16);
        dst_addr = dst_addr.wrapping_sub(16);
    }
}
