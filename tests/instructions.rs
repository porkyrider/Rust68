//! Tests unitaires ciblés du cœur 68000.
//!
//! Ces tests valident le comportement observable instruction par instruction.
//! Ils servent de filet de sécurité pendant le développement, en complément de
//! la suite de conformité TomHarte (voir `tests/tomharte.rs`).

use rust68::{Bus, Cpu, FlatBus, StepError, ccr};

/// Construit un CPU + bus, place un vecteur de reset (SSP=0x2000, PC=0x0400)
/// puis charge `words` (mots 16 bits big-endian) à partir de 0x0400.
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
fn reset_charge_ssp_et_pc() {
    let (cpu, _bus) = setup(&[]);
    assert_eq!(cpu.ssp, 0x2000);
    assert_eq!(cpu.sp(), 0x2000);
    assert_eq!(cpu.pc, 0x0400);
    assert!(cpu.supervisor());
}

#[test]
fn nop_avance_le_pc() {
    let (mut cpu, mut bus) = setup(&[0x4E71]);
    let cycles = cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0402);
    assert_eq!(cycles, 4);
}

#[test]
fn moveq_charge_constante_signee() {
    // MOVEQ #-1, D3  => 0111 011 0 11111111 = 0x76FF
    let (mut cpu, mut bus) = setup(&[0x76FF]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[3], 0xFFFF_FFFF);
    assert!(cpu.flag(ccr::N));
    assert!(!cpu.flag(ccr::Z));

    // MOVEQ #0, D0 => 0x7000
    let (mut cpu, mut bus) = setup(&[0x7000]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0);
    assert!(cpu.flag(ccr::Z));
    assert!(!cpu.flag(ccr::N));
}

#[test]
fn move_immediate_long_vers_data_reg() {
    // MOVE.L #$12345678, D0
    // 00 10 000 000 111 100  (taille L, dst Dn reg0, src immediate)
    // = 0x203C, suivi de la valeur longue
    let (mut cpu, mut bus) = setup(&[0x203C, 0x1234, 0x5678]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0x1234_5678);
    assert!(!cpu.flag(ccr::Z));
    assert!(!cpu.flag(ccr::N));
}

#[test]
fn movea_ne_touche_pas_les_flags() {
    // Positionne d'abord Z via MOVEQ #0,D0 puis MOVEA.L #$1000,A1.
    // MOVEA.L : 00 10 001 001 111 100 = 0x227C
    let (mut cpu, mut bus) = setup(&[0x7000, 0x227C, 0x0000, 0x1000]);
    cpu.step(&mut bus).unwrap(); // MOVEQ #0,D0 -> Z=1
    assert!(cpu.flag(ccr::Z));
    cpu.step(&mut bus).unwrap(); // MOVEA.L
    assert_eq!(cpu.a[1], 0x0000_1000);
    assert!(cpu.flag(ccr::Z), "MOVEA ne doit pas modifier le CCR");
}

#[test]
fn add_data_reg_avec_carry_et_overflow() {
    // D0 = 0x7FFFFFFF, ADD.L D0,D0 -> overflow signé, pas de carry.
    // MOVE.L #$7FFFFFFF,D0 ; ADD.L D0,D0
    // ADD.L D0,D0 (Dn+ea->ea, dst=ea=D0): 1101 000 1 10 000 000 = 0xD180
    let (mut cpu, mut bus) = setup(&[0x203C, 0x7FFF, 0xFFFF, 0xD180]);
    cpu.step(&mut bus).unwrap(); // charge D0
    cpu.step(&mut bus).unwrap(); // ADD.L D0,D0
    assert_eq!(cpu.d[0], 0xFFFF_FFFE);
    assert!(cpu.flag(ccr::N));
    assert!(cpu.flag(ccr::V), "0x7FFFFFFF + lui-même déborde en signé");
    assert!(!cpu.flag(ccr::C));
    assert!(!cpu.flag(ccr::X));
}

#[test]
fn add_produit_carry() {
    // D0 = 0xFFFFFFFF ; ADD.L D0,D0 -> carry, X
    let (mut cpu, mut bus) = setup(&[0x203C, 0xFFFF, 0xFFFF, 0xD180]);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0xFFFF_FFFE);
    assert!(cpu.flag(ccr::C));
    assert!(cpu.flag(ccr::X));
    assert!(!cpu.flag(ccr::V), "pas d'overflow signé ici");
}

#[test]
fn clr_met_a_zero_et_z() {
    // MOVEQ #5,D2 ; CLR.L D2
    // CLR.L D2 : 0100 0010 10 000 010 = 0x4282
    let (mut cpu, mut bus) = setup(&[0x7405, 0x4282]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[2], 5);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[2], 0);
    assert!(cpu.flag(ccr::Z));
    assert!(!cpu.flag(ccr::N));
}

#[test]
fn lea_charge_adresse_sans_dereferencer() {
    // LEA (d16,PC),A0 : 0100 000 111 111 010 = 0x41FA, disp.
    // base PC pointe sur le mot de déplacement (0x0402), disp=0x10 -> 0x0412.
    let (mut cpu, mut bus) = setup(&[0x41FA, 0x0010]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[0], 0x0412);
}

#[test]
fn bra_branche_inconditionnellement() {
    // BRA.B +4 : 0110 0000 00000100 = 0x6004
    // base = PC après opcode = 0x0402 ; cible = 0x0406.
    let (mut cpu, mut bus) = setup(&[0x6004]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0406);
}

#[test]
fn beq_branche_si_zero() {
    // MOVEQ #0,D0 (Z=1) ; BEQ +4
    // BEQ.B : 0110 0111 00000100 = 0x6704 ; base après opcode = 0x0404
    let (mut cpu, mut bus) = setup(&[0x7000, 0x6704]);
    cpu.step(&mut bus).unwrap(); // Z=1
    cpu.step(&mut bus).unwrap(); // BEQ pris
    assert_eq!(cpu.pc, 0x0408);
}

#[test]
fn beq_non_pris_si_pas_zero() {
    // MOVEQ #1,D0 (Z=0) ; BEQ +4 -> non pris, PC continue après l'opcode.
    let (mut cpu, mut bus) = setup(&[0x7001, 0x6704]);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0404, "BEQ non pris : PC juste après l'opcode");
}

#[test]
fn bsr_empile_le_retour() {
    // BSR.B +4 : 0110 0001 00000100 = 0x6104
    let (mut cpu, mut bus) = setup(&[0x6104]);
    let sp_avant = cpu.sp();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0406);
    assert_eq!(cpu.sp(), sp_avant - 4);
    // L'adresse de retour empilée pointe juste après l'opcode BSR.
    assert_eq!(bus.read32(cpu.sp()), 0x0402);
}

#[test]
fn line_a_declenche_exception() {
    // 0xA000 (Line-A) déclenche l'exception vecteur 10.
    // Le vecteur 10 est à l'adresse 40 (0x28). On installe une adresse cible.
    let mut bus = FlatBus::new();
    bus.write32(0x0000, 0x0000_2000); // SSP initial
    bus.write32(0x0004, 0x0000_0400); // PC initial
    bus.write32(0x0028, 0x0000_0800); // vecteur 10 -> 0x0800
    bus.write16(0x0400, 0xA000);      // Line-A
    let mut cpu = Cpu::new();
    cpu.reset(&mut bus);
    let result = cpu.step(&mut bus);
    assert!(result.is_ok(), "Line-A doit réussir (exception)");
    // Notre convention : cpu.pc = adresse_vecteur, cpu.pc+4 = adresse_vecteur+4 (TomHarte)
    assert_eq!(cpu.pc, 0x0800, "PC doit égaler l'adresse du vecteur");
}
