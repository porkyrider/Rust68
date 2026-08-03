//! Reproduction headless (sans SDL2, sans intervention clavier) du test K1
//! (clavier) de la cartouche de diagnostic usine STe, pour vérifier si le
//! délai de reset de l'IKBD explique l'échec "K1 Keyboard not responding"
//! indépendamment de toute frappe utilisateur.

use rust68::systems::atari_st::model::AtariModel;
use rust68::systems::atari_st::AtariSt;
use rust68::{Bus, Cpu};

fn main() {
    let rom_path = std::env::var("RUST68_ROM").expect("RUST68_ROM manquant");

    let rom = std::fs::read(&rom_path).expect("lecture ROM");

    let os_version = u16::from_be_bytes(rom[2..4].try_into().unwrap());
    let rom_base = if os_version >= 0x0106 { 0x00E0_0000 } else { rust68::systems::atari_st::DEFAULT_ROM_BASE };

    let profile = AtariModel::Ste1040.profile();
    let ram_size = profile.ram_size;
    let mut st = AtariSt::from_model(profile, rom);
    st.set_rom_base(rom_base);

    if let (Ok(hgh_path), Ok(low_path)) =
        (std::env::var("RUST68_CARTRIDGE_HGH"), std::env::var("RUST68_CARTRIDGE_LOW"))
    {
        let hgh = std::fs::read(&hgh_path).expect("lecture HGH");
        let low = std::fs::read(&low_path).expect("lecture LOW");
        let mut cart = Vec::with_capacity(hgh.len() * 2);
        for (h, l) in hgh.iter().zip(low.iter()) {
            cart.push(*h);
            cart.push(*l);
        }
        st.load_cartridge(cart);
    }

    if std::env::var("RUST68_COLD_BOOT").is_err() {
        st.write32(0x420, 0x752019F3);
        st.write32(0x43A, 0x237698AA);
        st.write32(0x51A, 0x5555AAAA);
        st.write32(0x42E, ram_size as u32);
        st.write8(0x0EE4, 0x11);
        st.write8(0x0EE5, 0x11);
    }

    let mut cpu = Cpu::new();
    cpu.reset(&mut st);

    if std::env::var("RUST68_COLD_BOOT").is_err() && !st.has_cartridge() {
        // Voir la doc equivalente dans atari_st_sdl2.rs.
        if let Some(conf) = AtariSt::expected_memory_conf(ram_size) {
            st.pin_memory_conf(conf);
        }
    }

    let target_cycles: u64 = std::env::var("RUST68_CYCLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000_000);

    let mut total: u64 = 0;
    let mut last_28d: u8 = 0xFF;
    let mut last_49c: u8 = 0xFF;
    let mut last_tsr: u8 = 0xFF;
    let mut spin_loop_hits: u64 = 0;
    let mut udr_writes: u64 = 0;
    let mut last_udr_write_total: u64 = 0;
    let mut last_49a: u8 = 0xFF;
    // Instant d'armement du timer en cours de test (écriture TACR/TBCR/TCDCR
    // dans la séquence T0), pour mesurer le délai réel jusqu'au déclenchement
    // de son interruption (entrée dans le gestionnaire partagé $14de).
    let mut timer_armed_at: Option<(u64, u32)> = None;
    while total < target_cycles {
        if cpu.halted {
            eprintln!("[{total:>10}] CPU HALTED (double bus fault) pc={:#08x}", cpu.pc);
            break;
        }
        let pc = cpu.pc;
        {
            let phystop = st.read32(0x42E);
            static mut LAST_PHYSTOP: u32 = 0xFFFFFFFF;
            unsafe {
                if phystop != LAST_PHYSTOP {
                    eprintln!("[{total:>10}] phystop(\\$42E) = {phystop:#010x} pc={:#08x}", cpu.pc);
                    LAST_PHYSTOP = phystop;
                }
            }
        }
        if pc == 0xfa6c46 {
            spin_loop_hits += 1;
        }
        if matches!(pc, 0xfa31e6 | 0xfa31fe | 0xfa3216 | 0xfa322e) {
            timer_armed_at = Some((total, pc));
        }
        if pc == 0xfa14de {
            if let Some((armed_total, armed_pc)) = timer_armed_at {
                eprintln!(
                    "[{total:>10}] IRQ timer prise en charge : armé à {armed_total} (pc={armed_pc:#08x}), délai={} cycles",
                    total - armed_total
                );
            }
        }
        {
            let vbase = ((st.read8(0xFF8201) as u32) << 16) | ((st.read8(0xFF8203) as u32) << 8);
            static mut LAST_VBASE: u32 = 0xFFFFFFFF;
            unsafe {
                if vbase != LAST_VBASE {
                    eprintln!("[{total:>10}] video_base($8201/$8203) = {vbase:#08x} pc={:#08x}", cpu.pc);
                    LAST_VBASE = vbase;
                }
            }
        }
        if pc == 0xfa0fc4 {
            let vc = (st.read8(0xFF8205) as u32) << 16 | (st.read8(0xFF8207) as u32) << 8 | st.read8(0xFF8209) as u32;
            eprintln!("[{total:>10}] VBL pris (\\$292 efface via fa0fc4), vc_reel={vc:#08x}");
        }
        if pc == 0xfa3406 {
            eprintln!("[{total:>10}] TBDR<-2 (arme T4)", );
        }
        if pc == 0xfa340c {
            eprintln!("[{total:>10}] TBCR<-8 (reload T4)", );
        }
        {
            let g290 = st.read16(0x290);
            static mut LAST_G290: u16 = 0xFFFF;
            unsafe {
                if g290 != LAST_G290 {
                    let vc = (st.read8(0xFF8205) as u32) << 16
                        | (st.read8(0xFF8207) as u32) << 8
                        | st.read8(0xFF8209) as u32;
                    eprintln!("[{total:>10}] \\$290={g290:#06x} (IRQ TimerB) vc_reel={vc:#08x} pc={:#08x}", cpu.pc);
                    LAST_G290 = g290;
                }
            }
        }
        if pc == 0xfa343c {
            let cur = st.read32(0x21A);
            static mut LAST_21A: u32 = 0;
            unsafe {
                eprintln!(
                    "[{total:>10}] T4 check: lu(\\$21a)={cur:#08x} attendu(\\$4da)={:#08x} delta_depuis_dernier={:#x}",
                    st.read32(0x4DA),
                    cur.wrapping_sub(LAST_21A),
                );
                LAST_21A = cur;
            }
        }
        {
            let v204 = st.read32(0x204);
            static mut LAST_204: u32 = 0xFFFFFFFF;
            unsafe {
                if v204 != LAST_204 {
                    eprintln!("[{total:>10}] \\$204 = {v204:#010x} pc={:#08x}", cpu.pc);
                    LAST_204 = v204;
                }
            }
        }
        if matches!(pc, 0xfa0060 | 0xfa007e | 0xfa00a2) {
            eprintln!("[{total:>10}] taille de banque détectée à pc={:#08x} (d1 avant maj)", cpu.pc);
        }
        if matches!(pc, 0xfa00de | 0xfa00f6 | 0xfa010c) {
            eprintln!("[{total:>10}] expansion supplémentaire détectée à pc={:#08x}", cpu.pc);
        }
        if pc == 0xfa0206 {
            eprintln!("[{total:>10}] \\$204 <- d5={:#010x} (taille finale détectée)", cpu.d[5]);
        }
        if pc == 0xfa1c14 {
            eprintln!(
                "[{total:>10}] fa1c14 (test bloc RAM) a0={:#08x} a1={:#08x} d0={:#06x}",
                cpu.a[0], cpu.a[1], cpu.d[0]
            );
        }
        if pc == 0xfa1c30 {
            eprintln!(
                "[{total:>10}] fa1c30 (echec comparaison) a0={:#08x} a1={:#08x}",
                cpu.a[0], cpu.a[1]
            );
        }
        if pc == 0xfa030c {
            eprintln!("[{total:>10}] écriture I7 (move.w d0,$0.w) atteinte, d0={:#06x} sr={:#06x}", cpu.d[0], cpu.sr);
        }
        if pc == 0xfa03e6 {
            eprintln!("[{total:>10}] gestionnaire bus error de la cartouche exécuté (I7 détecté)", );
        }
        if pc == 0xfa0320 {
            eprintln!("[{total:>10}] I7 : bus error NON détecté", );
        }
        if pc == 0xfa3556 {
            if let Some((armed_total, _)) = timer_armed_at.take() {
                eprintln!(
                    "[{total:>10}] fin de la fenêtre d'attente logicielle (TST.B $284) : {} cycles depuis l'armement",
                    total - armed_total
                );
            }
        }
        if std::env::var("RUST68_TRACE_WAIT_LOOP").is_ok()
            && total >= 4369936
            && total < 4390000
            && matches!(pc, 0xfa354a..=0xfa3566 | 0xfa14de..=0xfa1538)
        {
            eprintln!("[{total:>10}] pc={pc:#08x} d2={:#06x} sr={:#06x}", cpu.d[2], cpu.sr);
        }
        let cycles = match cpu.step(&mut st) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("step error at pc={:#08x}: {:?}", cpu.pc, e);
                break;
            }
        };
        st.tick(cycles);
        total += cycles as u64;

        if pc == 0xfa6c5a {
            udr_writes += 1;
            last_udr_write_total = total;
        }

        let b28d = st.read8(0x28D);
        let b49c = st.read8(0x49C);
        let tsr = st.read8(0xFFFFFA2D);
        if b28d != last_28d {
            eprintln!("[{total:>10}] $28D = {b28d:#04x} (bit0=K1 en cours/échec) pc={:#08x}", cpu.pc);
            last_28d = b28d;
        }
        if b49c != last_49c {
            eprintln!("[{total:>10}] $49C = {b49c:#04x} pc={:#08x}", cpu.pc);
            last_49c = b49c;
        }
        if tsr != last_tsr {
            eprintln!("[{total:>10}] TSR($FFFFFA2D) = {tsr:#04x} pc={:#08x} spin_hits_cumules={spin_loop_hits}", cpu.pc);
            last_tsr = tsr;
        }
        let vr = st.read8(0xFFFFFA17);
        static mut LAST_VR: u8 = 0xFF;
        unsafe {
            if vr != LAST_VR {
                eprintln!("[{total:>10}] VR($FFFFFA17) = {vr:#04x} pc={:#08x}", cpu.pc);
                LAST_VR = vr;
            }
        }
        let isra = st.read8(0xFFFFFA0F);
        static mut LAST_ISRA: u8 = 0xFF;
        unsafe {
            if isra != LAST_ISRA {
                eprintln!("[{total:>10}] ISRA($FFFFFA0F) = {isra:#04x} pc={:#08x}", cpu.pc);
                LAST_ISRA = isra;
            }
        }
        let b49a = st.read8(0x49A);
        if b49a != last_49a {
            eprintln!("[{total:>10}] $49A (erreurs T0) = {b49a:#04x} pc={:#08x}", cpu.pc);
            last_49a = b49a;
        }
    }
    eprintln!(
        "udr_writes={udr_writes} dernier_udr_write_cycle={last_udr_write_total} spin_loop_hits_total={spin_loop_hits}"
    );

    eprintln!("--- fin après {total} cycles ---");
    eprintln!("$28D (drapeaux K1) = {:#04x}", st.read8(0x28D));
    eprintln!("$460:$462 (compteur octets IKBD) = {:#06x}:{:#06x}", st.read16(0x460), st.read16(0x462));
    eprintln!("PC final = {:#08x}", cpu.pc);
    eprintln!("halted = {}", cpu.halted);
    eprintln!("phystop final = {:#010x}", st.read32(0x42E));
}
