//! Reproduction headless (sans SDL2, sans écran réel) de l'ouverture d'un
//! menu GEM via clics/déplacements IKBD directs, avec la même
//! instrumentation que `atari_st_sdl2.rs` (RUST68_TRACE_FILLPAT/E480/SIM)
//! pour capturer la routine de remplissage responsable de la corruption du
//! menu documentée dans ETAT.md, sans dépendre d'une interaction manuelle.

use rust68::systems::atari_st::model::AtariModel;
use rust68::systems::atari_st::AtariSt;
use rust68::{Bus, Cpu};

/// Ecrit `st.framebuffer` (deja calcule par `Shifter::render_scanline`,
/// meme mecanisme que la touche F3 de `atari_st_sdl2.rs`) en PPM brut —
/// ouvrable directement (Aperçu sur macOS, la plupart des visionneuses).
fn dump_ppm(st: &AtariSt, path: &str) {
    let h = st.framebuffer.len();
    let w = st.framebuffer.first().map(|r| r.len()).unwrap_or(0);
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for row in &st.framebuffer {
        for &(r, g, b) in row {
            out.push(r);
            out.push(g);
            out.push(b);
        }
    }
    if let Err(e) = std::fs::write(path, &out) {
        eprintln!("[dump_ppm] echec ecriture {path}: {e}");
    } else {
        eprintln!("[dump_ppm] ecrit {path} ({w}x{h})");
    }
}

fn main() {
    let rom = std::fs::read("ressources (local)/tos162.img").expect("lecture ROM");
    let os_version = u16::from_be_bytes(rom[2..4].try_into().unwrap());
    let rom_base = if os_version >= 0x0106 { 0x00E0_0000 } else { rust68::systems::atari_st::DEFAULT_ROM_BASE };

    let profile = AtariModel::Ste1040.profile();
    let ram_size = profile.ram_size;
    let mut st = AtariSt::from_model(profile, rom);
    st.set_rom_base(rom_base);

    st.write32(0x420, 0x752019F3);
    st.write32(0x43A, 0x237698AA);
    st.write32(0x51A, 0x5555AAAA);
    st.write32(0x42E, ram_size as u32);
    st.write8(0x0EE4, 0x11);
    st.write8(0x0EE5, 0x11);

    let mut cpu = Cpu::new();
    cpu.reset(&mut st);

    if !st.has_cartridge() {
        if let Some(conf) = AtariSt::expected_memory_conf(ram_size) {
            st.pin_memory_conf(conf);
        }
    }

    // Séquence de clics/déplacements à injecter, en cycles CPU depuis le
    // début (approximatif, calé sur le temps de boot habituel ~15-20M
    // cycles pour atteindre le bureau GEM interactif) : (cycle, dx, dy, boutons).
    // Convention boutons : bit1=gauche, bit0=droit (voir queue_mouse_move,
    // atari_st_sdl2.rs).
    let mut events: Vec<(u64, i8, i8, u8)> = vec![
        // Recentre à (0,0) par une grande course négative, en plusieurs pas
        // (MAX_MOVE=15 par paquet réel, mais ikbd.mouse_move clippe déjà en
        // interne — quelques gros pas suffisent).
    ];
    let no_click = std::env::var("RUST68_NO_CLICK").is_ok();
    let mut t: u64 = 30_000_000; // apres boot, bureau GEM pret
    // Balaie plusieurs positions X le long de la barre de menu (Y=4, dans
    // la premiere rangee de caracteres) : recentre a (0,0), va a (x,4),
    // presse le bouton gauche (ouvre un eventuel menu), glisse vers le bas,
    // relache, marque la frontiere (t, "marker", x) pour correler avec
    // fillpat_hits ensuite.
    let mut markers: Vec<(u64, i16)> = Vec::new();
    if !no_click {
        for x in [8i16, 24, 40, 56, 72, 88, 104, 120, 136, 152, 168, 184, 200] {
            // Recentre a (0,0).
            for _ in 0..40 {
                events.push((t, -15, -15, 0));
                t += 20_000;
            }
            markers.push((t, x));
            // Va vers (x, 4).
            let mut remaining = x;
            while remaining > 0 {
                let step = remaining.min(15);
                events.push((t, step as i8, 0, 0));
                remaining -= step;
                t += 20_000;
            }
            events.push((t, 0, 4, 0));
            t += 20_000;
            // Presse bouton gauche.
            events.push((t, 0, 0, 0b10));
            t += 1_000_000;
            // Glisse vers le bas sur un item.
            for _ in 0..3 {
                events.push((t, 0, 8, 0b10));
                t += 200_000;
            }
            t += 500_000;
            // Relache.
            events.push((t, 0, 0, 0));
            t += 500_000;
        }
    }

    let trace_fillpat = std::env::var("RUST68_TRACE_FILLPAT").is_ok();
    let trace_e480 = std::env::var("RUST68_TRACE_E480").is_ok();
    let trace_sim = std::env::var("RUST68_TRACE_SIM").is_ok();
    let mut fillpat_hits: u32 = 0;
    let mut e480_hits: u32 = 0;
    let mut sim_entry: Option<(u64, [u32; 8], [u32; 8], u16)> = None;

    let mut total: u64 = 0;
    let mut step: u64 = 0;
    let mut ev_idx = 0usize;
    let mut marker_idx = 0usize;
    let mut current_x: i16 = -1;
    let mut hits_at_marker_start: u32 = 0;
    let limit_cycles: u64 = 200_000_000;
    while total < limit_cycles {
        if cpu.halted {
            eprintln!("[step {step}] CPU HALTED pc={:#08x}", cpu.pc);
            break;
        }
        while ev_idx < events.len() && events[ev_idx].0 <= total {
            let (_, dx, dy, buttons) = events[ev_idx];
            st.ikbd.mouse_move(dx, dy, buttons);
            ev_idx += 1;
        }
        while marker_idx < markers.len() && markers[marker_idx].0 <= total {
            if current_x >= 0 {
                eprintln!(
                    "[x={current_x}] fillpat_hits pendant ce segment = {}",
                    fillpat_hits - hits_at_marker_start
                );
                dump_ppm(&st, &format!("/tmp/rd_menu_x{current_x:03}.ppm"));
            }
            current_x = markers[marker_idx].1;
            hits_at_marker_start = fillpat_hits;
            marker_idx += 1;
        }
        let pc_before = cpu.pc;

        if trace_fillpat
            && fillpat_hits < 5000
            && matches!(pc_before, 0xE10C04 | 0xE10C44 | 0xE10C94)
        {
            fillpat_hits += 1;
            let sp = cpu.a[7];
            let ret_addr = st.read32(sp);
            use rust68::peripherals::atari_st::blitter::reg as blitreg;
            let hop = st.blitter.read(blitreg::HOP);
            let op = st.blitter.read(blitreg::OP);
            let x_count = ((st.blitter.read(blitreg::X_COUNT) as u16) << 8)
                | st.blitter.read(blitreg::X_COUNT1) as u16;
            let y_count = ((st.blitter.read(blitreg::Y_COUNT) as u16) << 8)
                | st.blitter.read(blitreg::Y_COUNT1) as u16;
            let long = |a: u32| {
                ((st.blitter.read(a) as u32) << 24)
                    | ((st.blitter.read(a + 1) as u32) << 16)
                    | ((st.blitter.read(a + 2) as u32) << 8)
                    | st.blitter.read(a + 3) as u32
            };
            let dst_addr = long(blitreg::DST_ADDR);
            eprintln!(
                "[fillpat] hit #{fillpat_hits} step={step} branche={pc_before:#08x} retour={ret_addr:#010x} hop={hop} op={op:#04x} x={x_count} y={y_count} dst_addr={dst_addr:#010x} a2={:#010x}",
                cpu.a[2],
            );
        }

        if trace_e480 && e480_hits < 500 && pc_before == 0xE0E480 {
            e480_hits += 1;
            let sp = cpu.a[7];
            let ret_addr = st.read32(sp);
            eprintln!(
                "[e480] appel #{e480_hits} step={step} retour={ret_addr:#010x} a4={:#010x} a1={:#010x} d6={:#010x} d7={:#010x}",
                cpu.a[4], cpu.a[1], cpu.d[6], cpu.d[7],
            );
        }

        if trace_sim && pc_before == 0xE12C1C && sim_entry.is_none() {
            let mut d = [0u32; 8];
            let mut a = [0u32; 8];
            d.copy_from_slice(&cpu.d);
            a.copy_from_slice(&cpu.a);
            sim_entry = Some((step, d, a, cpu.sr));
            eprintln!(
                "[sim-entree] step={step} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x} d5={:#010x} d6={:#010x} d7={:#010x}",
                d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
            );
            eprintln!(
                "[sim-entree] a0={:#010x} a1={:#010x} a2={:#010x} a3={:#010x} a4={:#010x} a5={:#010x} a6={:#010x} a7={:#010x} sr={:#06x}",
                a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], cpu.sr
            );
        }

        let cycles = match cpu.step(&mut st) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[step {step}] step error pc={:#08x}: {:?}", cpu.pc, e);
                break;
            }
        };
        st.tick(cycles);
        if cpu.trace_pending {
            cpu.take_trace_exception(&mut st);
        }
        total += cycles as u64;
        step += 1;
    }
    if current_x >= 0 {
        eprintln!(
            "[x={current_x}] fillpat_hits pendant ce segment = {}",
            fillpat_hits - hits_at_marker_start
        );
        dump_ppm(&st, &format!("/tmp/rd_menu_x{current_x:03}.ppm"));
    }
    dump_ppm(&st, "/tmp/rd_menu_final.ppm");
    eprintln!(
        "fin: total_cycles={total} steps={step} fillpat_hits={fillpat_hits} e480_hits={e480_hits} sim_capture={}",
        sim_entry.is_some()
    );

    if std::env::var("RUST68_DUMP_MENU_REGION").is_ok() {
        // Dessine la zone d'ecran autour des adresses touchees par fillpat
        // (0xf8000-base, mode basse resolution : 160 octets/ligne, 4 plans
        // entrelaces par mot -> on affiche juste les octets bruts en binaire
        // pour reperer un motif structure vs du bruit).
        let base = 0xf_8000u32;
        for line in 0..120u32 {
            let addr = base + line * 8;
            let bytes: Vec<u8> = (0..8).map(|o| st.read8(addr + o)).collect();
            let bits: String = bytes
                .iter()
                .map(|b| {
                    (0..8)
                        .rev()
                        .map(|i| if b & (1 << i) != 0 { '#' } else { '.' })
                        .collect::<String>()
                })
                .collect();
            eprintln!("{addr:#08x} {bits}");
        }
    }
}
