//! Tests unitaires ciblés du sous-ensemble 68010 (`CpuType::M68010`).
//!
//! Aucun vecteur SingleStepTests n'existe pour le 68010 (vérifié auprès de
//! github.com/SingleStepTests/m68000 et 680x0 : seul le 68000 y est publié) —
//! ces tests sont donc écrits à la main, sur le même patron que
//! `tests/instructions.rs`. Ils couvrent : VBR (registre + relocalisation de
//! la table de vecteurs), le mot de format des frames d'exception courtes,
//! MOVEC (VBR/SFC/DFC/USP), RTD, MOVES, MOVE from CCR, et — point important —
//! que le comportement 68000 par défaut (`CpuType::M68000`, jamais touché par
//! ces changements) reste inchangé : ces mêmes opcodes doivent toujours lever
//! l'exception "illegal instruction" (vecteur 4) sur 68000, exactement comme
//! avant cette extension.

use rust68::{Bus, Cpu, CpuType, FlatBus};

/// Construit un CPU + bus, place un vecteur de reset (SSP=0x2000, PC=0x0400)
/// puis charge `words` (mots 16 bits big-endian) à partir de 0x0400 — même
/// convention que `tests/instructions.rs`.
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

// --- Régression 68000 : ces opcodes restent "illegal instruction" ---------

#[test]
fn movec_leve_illegal_instruction_sur_68000() {
    let (mut cpu, mut bus) = setup(&[0x4E7A, 0x0801]); // MOVEC VBR,D0
    bus.write32(0x10, 0x0000_0600); // vecteur 4
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn rtd_leve_illegal_instruction_sur_68000() {
    let (mut cpu, mut bus) = setup(&[0x4E74, 0x0010]); // RTD #16
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn moves_leve_illegal_instruction_sur_68000() {
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x8000]); // MOVES.W D0,(A0)
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn move_from_ccr_leve_illegal_instruction_sur_68000() {
    let (mut cpu, mut bus) = setup(&[0x42C0]); // MOVE from CCR,D0
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

#[test]
fn frame_courte_68000_ne_change_pas_de_taille() {
    // TRAP #0 : SR (mot) + PC (mot long) = 6 octets, pas de mot de format.
    let (mut cpu, mut bus) = setup(&[0x4E40]);
    bus.write32(0x80, 0x0000_2000); // vecteur 32 (TRAP #0)
    let sp_avant = cpu.sp();
    cpu.step(&mut bus).unwrap();
    assert_eq!(sp_avant - cpu.sp(), 6, "68000 : frame de 6 octets, jamais de mot de format");
}

// --- MOVEC (68010) ---------------------------------------------------------

#[test]
fn movec_vbr_aller_retour() {
    let (mut cpu, mut bus) = setup(&[
        0x4E7B, 0x0801, // MOVEC D0,VBR
        0x4E7A, 0x1801, // MOVEC VBR,D1
    ]);
    cpu.cpu_type = CpuType::M68010;
    cpu.d[0] = 0x0000_1234;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.vbr, 0x0000_1234, "VBR doit recevoir la valeur de D0");
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[1], 0x0000_1234, "D1 doit recevoir la valeur de VBR");
}

#[test]
fn movec_sfc_dfc_masques_a_3_bits() {
    let (mut cpu, mut bus) = setup(&[
        0x4E7B, 0x0000, // MOVEC D0,SFC
        0x4E7B, 0x0001, // MOVEC D0,DFC
    ]);
    cpu.cpu_type = CpuType::M68010;
    cpu.d[0] = 0xFFFF_FFFF;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.sfc, 0b111, "SFC ne garde que les 3 bits utiles");
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.dfc, 0b111, "DFC ne garde que les 3 bits utiles");
}

#[test]
fn movec_usp_aller_retour() {
    let (mut cpu, mut bus) = setup(&[
        0x4E7B, 0x0800, // MOVEC D0,USP
        0x4E7A, 0x1800, // MOVEC USP,D1
    ]);
    cpu.cpu_type = CpuType::M68010;
    cpu.d[0] = 0x0000_7000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.usp, 0x0000_7000);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[1], 0x0000_7000);
}

#[test]
fn movec_rc_inconnu_leve_illegal_instruction() {
    // Sélecteur de registre de contrôle réservé (ni SFC/DFC/USP/VBR).
    let (mut cpu, mut bus) = setup(&[0x4E7A, 0x0002]);
    cpu.cpu_type = CpuType::M68010;
    bus.write32(0x10, 0x0000_0600);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

// --- RTD (68010) ------------------------------------------------------------

#[test]
fn rtd_depile_pc_et_ajuste_sp() {
    let (mut cpu, mut bus) = setup(&[0x4E74, 0x0010]); // RTD #16
    cpu.cpu_type = CpuType::M68010;
    // cpu.a[7] == cpu.ssp == 0x2000 après reset (voir `setup`).
    bus.write32(cpu.sp(), 0x0000_1234); // adresse de retour empilée
    let sp_avant = cpu.sp();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0000_1234);
    assert_eq!(cpu.sp(), sp_avant + 4 + 16, "dépile le PC (4) puis ajoute le déplacement (16)");
}

// --- MOVES (68010) -----------------------------------------------------------

#[test]
fn moves_registre_vers_memoire() {
    // MOVES.W D0,(A0)
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x8000]);
    cpu.cpu_type = CpuType::M68010;
    cpu.a[0] = 0x0000_3000;
    cpu.d[0] = 0x0000_ABCD;
    cpu.step(&mut bus).unwrap();
    assert_eq!(bus.read16(0x3000), 0xABCD);
}

#[test]
fn moves_memoire_vers_registre_preserve_les_octets_hauts() {
    // MOVES.W (A0),D1 — destination Dn : seuls les 16 bits bas changent.
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x1000]);
    cpu.cpu_type = CpuType::M68010;
    cpu.a[0] = 0x0000_3000;
    cpu.d[1] = 0x1111_0000;
    bus.write16(0x3000, 0xBEEF);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[1], 0x1111_BEEF);
}

#[test]
fn moves_leve_privilege_violation_en_mode_utilisateur() {
    let (mut cpu, mut bus) = setup(&[0x0E50, 0x8000]);
    cpu.cpu_type = CpuType::M68010;
    cpu.set_supervisor(false);
    bus.write32(0x20, 0x0000_0700); // vecteur 8 (privilege violation)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0000_0700);
}

// --- MOVE from CCR (68010) ---------------------------------------------------

#[test]
fn move_from_ccr_non_privilegiee() {
    // MOVE from CCR,D0 : 0100 0010 11 000 000 = 0x42C0
    let (mut cpu, mut bus) = setup(&[0x42C0]);
    cpu.cpu_type = CpuType::M68010;
    cpu.set_supervisor(false); // non privilégiée : doit réussir en mode utilisateur
    cpu.sr = (cpu.sr & 0xFF00) | 0x0015; // X=1,Z=1,C=1 (0b10101)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0] & 0xFFFF, 0x0015);
}

// --- VBR : relocalisation de la table de vecteurs ---------------------------

#[test]
fn vbr_relocalise_la_table_de_vecteurs() {
    let (mut cpu, mut bus) = setup(&[0x4E40]); // TRAP #0 (vecteur 32)
    cpu.cpu_type = CpuType::M68010;
    cpu.vbr = 0x0000_1000;
    // Vecteur 32 relogé : vbr + 32*4 = 0x1000 + 0x80 = 0x1080 (PAS 0x80).
    bus.write32(0x0080, 0x0000_9999); // handler par défaut (adresse 0), ne doit PAS être pris
    bus.write32(0x1080, 0x0000_2000); // vrai handler, relatif à VBR
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0000_2000);
}

#[test]
fn frame_courte_68010_ajoute_un_mot_de_format() {
    let (mut cpu, mut bus) = setup(&[0x4E40]); // TRAP #0
    cpu.cpu_type = CpuType::M68010;
    bus.write32(0x80, 0x0000_2000);
    let sp_avant = cpu.sp();
    cpu.step(&mut bus).unwrap();
    let sp_apres = cpu.sp();
    assert_eq!(sp_avant - sp_apres, 8, "68010 : frame de 8 octets (SR+PC+format)");
    let format_word = bus.read16(sp_apres + 6);
    assert_eq!(format_word, (32u16) << 2, "nibble de format à 0, offset de vecteur = 32");
}
