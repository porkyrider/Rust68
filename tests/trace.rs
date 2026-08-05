//! Tests unitaires du mode trace (bit T du SR, `Cpu::trace_pending` /
//! `Cpu::take_trace_exception`).
//!
//! TomHarte capture volontairement l'effet d'une seule instruction sans
//! enchaîner sur la trace (voir `tests/tomharte.rs` : NOP avec T=1 en
//! entrée a un `final.sr` identique, aucun frame poussé) — ce fichier est
//! donc le seul filet de sécurité pour ce mécanisme, sur le même principe
//! que `tests/interrupts.rs`.

use rust68::{Bus, Cpu, FlatBus, sr};

fn setup(words: &[u16]) -> (Cpu, FlatBus) {
    let mut bus = FlatBus::new();
    bus.write32(0x0000, 0x0000_2000);
    bus.write32(0x0004, 0x0000_0400);
    let mut addr = 0x0400;
    for &w in words {
        bus.write16(addr, w);
        addr += 2;
    }
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    (cpu, bus)
}

#[test]
fn pas_de_trace_si_t_est_a_zero() {
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP
    cpu.sr &= !sr::T;
    cpu.step(&mut bus).unwrap();
    assert!(!cpu.trace_pending);
}

#[test]
fn step_ne_prend_pas_la_trace_lui_meme_mais_pose_trace_pending() {
    // Reproduit exactement le cas TomHarte "NOP avec T=1 en entrée" :
    // l'effet direct de l'instruction (SR, PC, RAM) doit être identique à
    // T=0, seul trace_pending doit signaler que la trace est due.
    let (mut cpu, mut bus) = setup(&[0x4E71]); // NOP
    cpu.sr |= sr::T;
    let sr_avant = cpu.sr;
    let sp_avant = cpu.sp();

    let cycles = cpu.step(&mut bus).unwrap();

    assert_eq!(cycles, 4, "coût inchangé : la trace n'est pas prise dans step()");
    assert_eq!(cpu.pc, 0x0402);
    assert_eq!(cpu.sr, sr_avant, "SR inchangé par step() lui-même");
    assert_eq!(cpu.sp(), sp_avant, "aucun frame poussé par step() lui-même");
    assert!(cpu.trace_pending);
}

#[test]
fn take_trace_exception_pousse_le_frame_et_efface_t() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr |= sr::T;
    bus.write32(0x0024, 0x0000_0900); // vecteur 9 (trace) * 4 = 0x24
    let sr_avant = cpu.sr;
    cpu.step(&mut bus).unwrap();
    let pc_apres_nop = cpu.pc;

    let cycles = cpu.take_trace_exception(&mut bus);

    assert_eq!(cycles, Some(34));
    assert_eq!(cpu.pc, 0x0900, "saut au handler du vecteur 9");
    assert_eq!(cpu.sr & sr::T, 0, "T doit être effacé en entrant dans le handler");
    // Frame standard 6 octets : SP-6 = SR sauvegardé, SP-2 (longword) = PC de retour.
    assert_eq!(bus.read16(cpu.sp()), sr_avant);
    assert_eq!(bus.read32(cpu.sp().wrapping_add(2)), pc_apres_nop);
    assert!(!cpu.trace_pending, "consommé par take_trace_exception");
}

#[test]
fn take_trace_exception_ne_fait_rien_si_rien_n_est_en_attente() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    cpu.sr &= !sr::T;
    cpu.step(&mut bus).unwrap();
    let pc_avant = cpu.pc;
    assert_eq!(cpu.take_trace_exception(&mut bus), None);
    assert_eq!(cpu.pc, pc_avant, "aucun effet si aucune trace n'est due");
}

#[test]
fn une_instruction_qui_pose_t_elle_meme_ne_se_trace_qu_apres_la_suivante() {
    // ORI #$8000,SR (0x007C + immédiat) puis un NOP. Vérifié empiriquement
    // contre Hatari (2026-08-04, `Rick_Dangerous.stx` : une routine TOS qui
    // pose T via ORI to SR pour piloter une boucle de déchiffrement
    // instruction par instruction) : le silicium réel exécute ENCORE
    // l'instruction suivante avant que la trace ne devienne effective —
    // même mécanisme matériel que le délai du masque IPL (voir
    // `Cpu::sr_write_pending_delay`). L'ancienne hypothèse ("dès la fin de
    // CETTE instruction") n'avait jamais été vérifiée contre une référence
    // et causait un double bus fault que Hatari n'a jamais sur ce jeu.
    let (mut cpu, mut bus) = setup(&[0x007C, 0x8000, 0x4E71]);
    cpu.sr &= !sr::T;

    cpu.step(&mut bus).unwrap(); // ORI #$8000,SR
    assert!(cpu.sr & sr::T != 0, "T doit être posé par ORI to SR");
    assert!(
        !cpu.trace_pending,
        "pas encore : c'est CETTE instruction qui vient de poser T"
    );

    cpu.step(&mut bus).unwrap(); // NOP
    assert!(
        cpu.trace_pending,
        "la trace se déclenche après l'instruction SUIVANT celle qui a posé T"
    );
}

#[test]
fn trap_n_active_pas_trace_pending_meme_avec_t_initialement_pose() {
    // take_exception (appelé par TRAP) efface T en entrant dans son propre
    // frame : trace_pending ne doit donc PAS se poser en plus.
    let (mut cpu, mut bus) = setup(&[0x4E40]); // TRAP #0
    bus.write32(0x0080, 0x0000_0900); // vecteur 32 (TRAP #0) * 4 = 0x80
    cpu.sr |= sr::T;

    cpu.step(&mut bus).unwrap();

    assert_eq!(cpu.pc, 0x0900, "TRAP a bien sauté à son propre handler");
    assert_eq!(cpu.sr & sr::T, 0, "T effacé par l'entrée d'exception de TRAP");
    assert!(
        !cpu.trace_pending,
        "pas de trace supplémentaire : l'exception de TRAP a déjà géré T"
    );
}

#[test]
fn rte_qui_depile_un_sr_avec_t_pose_ne_declenche_la_trace_qu_apres_la_suivante() {
    let (mut cpu, mut bus) = setup(&[0x4E73]); // RTE
    bus.write16(0x0800, 0x4E71); // NOP à l'adresse de retour
    // Empile un frame de retour : SR (avec T=1, superviseur) puis PC=0x0800.
    let sp = cpu.sp() - 6;
    cpu.set_sp(sp);
    bus.write16(sp, sr::T | rust68::sr::S);
    bus.write32(sp + 2, 0x0800);

    cpu.step(&mut bus).unwrap(); // RTE

    assert_eq!(cpu.pc, 0x0800);
    assert!(cpu.sr & sr::T != 0);
    assert!(
        !cpu.trace_pending,
        "pas encore : c'est CETTE instruction (RTE) qui vient de restaurer T=1"
    );

    cpu.step(&mut bus).unwrap(); // NOP
    assert!(
        cpu.trace_pending,
        "la trace se déclenche après l'instruction SUIVANT le RTE qui a restauré T"
    );
}
