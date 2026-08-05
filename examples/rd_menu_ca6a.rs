//! Reproduction headless ciblée du menu "Bureau" corrompu (x=184 confirmé
//! par `rd_menu_repro.rs`), avec le traçage complet de la routine de "skew
//! logiciel" ($E0CA6A — combine/rotation façon Blitter en pur CPU, voir
//! pseudocode déjà décodé dans une session antérieure) en plus des sondes
//! déjà présentes dans `atari_st_sdl2.rs` (fillpat/dispatch/blitdet).

use rust68::peripherals::atari_st::blitter::DEBUG_LAST_PC;
use rust68::systems::atari_st::model::AtariModel;
use rust68::systems::atari_st::AtariSt;
use rust68::{Bus, Cpu};

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
    let _ = std::fs::write(path, &out);
    eprintln!("[dump_ppm] ecrit {path} ({w}x{h})");
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

    let mut events: Vec<(u64, i8, i8, u8)> = Vec::new();
    let mut t: u64 = 30_000_000;
    for _ in 0..40 {
        events.push((t, -15, -15, 0));
        t += 20_000;
    }
    let x = 184i16;
    let mut remaining = x;
    while remaining > 0 {
        let step = remaining.min(15);
        events.push((t, step as i8, 0, 0));
        remaining -= step;
        t += 20_000;
    }
    events.push((t, 0, 4, 0));
    t += 20_000;
    events.push((t, 0, 0, 0b10));
    t += 1_000_000;
    for _ in 0..3 {
        events.push((t, 0, 8, 0b10));
        t += 200_000;
    }
    t += 1_500_000; // menu deroule stable, bouton encore presse : dump ici
    let dump_before_release_at = t;
    events.push((t, 0, 0, 0));
    t += 2_000_000; // laisse le temps a l'affichage de se stabiliser

    let mut ca6a_hits: u32 = 0;
    let mut ca6a_last_ret: u32 = 0;
    let mut ca6a_step_budget: u32 = 0;
    let mut ca6a_step_armed = false;
    // Routine inconnue ecrivant a 0xfab18/19 (un seul plan sur 4 — voir
    // [watch]) : trace pas a pas pour identifier ce qu'elle fait.
    let mut e117_hits: u32 = 0;
    let mut e117_step_budget: u32 = 0;
    let mut fillpat_hits: u32 = 0;
    let mut dispatch_hits: u32 = 0;
    let mut blitdet_hits: u32 = 0;

    // Surveillance directe des adresses memoire correspondant aux pixels
    // jaunes confirmes par lecture PPM brute : (x=306-317,y=68) et
    // (x=171-172,y=20), converties via la formule bas-res reelle (4 plans
    // entrelaces par mot, 8 octets/groupe de 16 pixels, 160 octets/ligne) —
    // video_base+0x2B18 et video_base+0xCD0 en supposant video_base=0xf8000.
    // Capture TOUTE ecriture (Blitter ou CPU), sans le filtre "interessant"
    // de RUST68_TRACE_BLITTER_WORDS, pour ne rien manquer.
    let watch_addrs: Vec<u32> = (0..8u32)
        .map(|i| 0x000f_ab18 + i)
        .chain((0..8u32).map(|i| 0x000f_8cd0 + i))
        .collect();
    let mut watch_prev: Vec<u8> = watch_addrs.iter().map(|&a| st.read8(a)).collect();

    let mut total: u64 = 0;
    let mut step: u64 = 0;
    let mut ev_idx = 0usize;
    let mut dumped_before_release = false;
    let limit_cycles: u64 = 45_000_000;
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
        if !dumped_before_release && total >= dump_before_release_at {
            dumped_before_release = true;
            dump_ppm(&st, "/tmp/rd_menu_ca6a_ouvert.ppm");
        }
        let pc_before = cpu.pc;

        // Detection materielle du Blitter ($E01062).
        if pc_before == 0xE01062 && blitdet_hits < 20 {
            blitdet_hits += 1;
            eprintln!("[blitdet] entree #{blitdet_hits} step={step} a0={:#010x}", cpu.a[0]);
        }

        // Dispatch materiel/logiciel ($E0B4AA).
        if pc_before == 0xE0B4AA && dispatch_hits < 200 {
            dispatch_hits += 1;
            let a4 = cpu.a[4];
            let fn_ptr = st.read32(a4.wrapping_add(0x9A));
            eprintln!(
                "[dispatch] hit #{dispatch_hits} step={step} a4={a4:#010x} fn_ptr@9A(a4)={fn_ptr:#010x} d4={:#010x} d6={:#010x} a0={:#010x}",
                cpu.d[4], cpu.d[6], cpu.a[0],
            );
        }

        // Routine de remplissage partagee (3 points de declenchement).
        if fillpat_hits < 500 && matches!(pc_before, 0xE10C04 | 0xE10C44 | 0xE10C94) {
            fillpat_hits += 1;
            use rust68::peripherals::atari_st::blitter::reg as blitreg;
            let hop = st.blitter.read(blitreg::HOP);
            let op = st.blitter.read(blitreg::OP);
            let x_count = ((st.blitter.read(blitreg::X_COUNT) as u16) << 8) | st.blitter.read(blitreg::X_COUNT1) as u16;
            let y_count = ((st.blitter.read(blitreg::Y_COUNT) as u16) << 8) | st.blitter.read(blitreg::Y_COUNT1) as u16;
            let long = |a: u32| {
                ((st.blitter.read(a) as u32) << 24)
                    | ((st.blitter.read(a + 1) as u32) << 16)
                    | ((st.blitter.read(a + 2) as u32) << 8)
                    | st.blitter.read(a + 3) as u32
            };
            eprintln!(
                "[fillpat] hit #{fillpat_hits} step={step} branche={pc_before:#08x} hop={hop} op={op:#04x} x={x_count} y={y_count} dst_addr={:#010x} a2={:#010x}",
                long(blitreg::DST_ADDR), cpu.a[2],
            );
        }

        // Routine de skew logiciel ($E0CA6A) : un hit par appel distinct
        // (adresse de retour differente du precedent), plus trace pas a pas
        // des 60 premieres instructions du tout premier appel.
        if pc_before == 0xE0CA6A && ca6a_hits < 500 {
            let sp = cpu.a[7];
            let ret_addr = st.read32(sp);
            if ret_addr != ca6a_last_ret {
                ca6a_last_ret = ret_addr;
                ca6a_hits += 1;
                eprintln!(
                    "[ca6a] appel #{ca6a_hits} step={step} retour={ret_addr:#010x} a1(dst)={:#010x} a2={:#010x} a3(mode)={:#010x} d3={:#010x} d4(stride)={:#010x} d5(compte-1)={:#010x} d6(skew)={:#010x} a0(src)={:#010x}",
                    cpu.a[1], cpu.a[2], cpu.a[3], cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.a[0],
                );
                eprintln!(
                    "[ca6a-video] resolution={:?} video_base={:#010x}",
                    st.shifter.resolution(), st.shifter.video_base(),
                );
                let table_base = cpu.a[0] & !1u32;
                let words: Vec<String> = (0..16).map(|i| format!("{:04x}", st.read16(table_base.wrapping_add(i * 2)))).collect();
                eprintln!("[ca6a-table] base={table_base:#010x} 16 mots = {}", words.join(" "));
                if !ca6a_step_armed {
                    ca6a_step_armed = true;
                    ca6a_step_budget = 200;
                }
            }
        }

        if pc_before == 0xE11746 && (2_332_000..2_332_700).contains(&step) && e117_hits < 200 {
            e117_hits += 1;
            let sp = cpu.a[7];
            let ret_addr = st.read32(sp);
            eprintln!(
                "[e117] entree #{e117_hits} step={step} retour={ret_addr:#010x} a0={:#010x} a1={:#010x} a2={:#010x} a3={:#010x} a4={:#010x} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x}",
                cpu.a[0], cpu.a[1], cpu.a[2], cpu.a[3], cpu.a[4], cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[4],
            );
            e117_step_budget = 60;
        }

        DEBUG_LAST_PC.store(pc_before, std::sync::atomic::Ordering::Relaxed);
        let cycles = match cpu.step(&mut st) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[step {step}] step error pc={:#08x}: {:?}", cpu.pc, e);
                break;
            }
        };

        if e117_step_budget > 0 {
            e117_step_budget -= 1;
            eprintln!(
                "[e117-step] pc={pc_before:#010x} op={:#06x} -> {:#010x} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d6={:#010x} d7={:#010x} a0={:#010x} a1={:#010x} a2={:#010x} a3={:#010x} a5={:#010x}",
                st.read16(pc_before), cpu.pc, cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[6], cpu.d[7], cpu.a[0], cpu.a[1], cpu.a[2], cpu.a[3], cpu.a[5],
            );
        }

        if ca6a_step_budget > 0 {
            ca6a_step_budget -= 1;
            eprintln!(
                "[ca6a-step] pc={pc_before:#010x} -> {:#010x} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x} d5={:#010x} d6={:#010x} a0={:#010x} a1={:#010x} a2={:#010x}",
                cpu.pc, cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.a[0], cpu.a[1], cpu.a[2],
            );
        }

        st.tick(cycles);
        if cpu.trace_pending {
            cpu.take_trace_exception(&mut st);
        }

        for (idx, &addr) in watch_addrs.iter().enumerate() {
            let v = st.read8(addr);
            if v != watch_prev[idx] {
                eprintln!(
                    "[watch] step={step} pc={pc_before:#010x} addr={addr:#08x} {:#04x} -> {v:#04x} video_base={:#08x}",
                    watch_prev[idx], st.shifter.video_base(),
                );
                watch_prev[idx] = v;
            }
        }

        total += cycles as u64;
        step += 1;
    }
    dump_ppm(&st, "/tmp/rd_menu_ca6a_final.ppm");
    eprintln!(
        "fin: total_cycles={total} steps={step} fillpat_hits={fillpat_hits} ca6a_hits={ca6a_hits} dispatch_hits={dispatch_hits} blitdet_hits={blitdet_hits}"
    );
}
