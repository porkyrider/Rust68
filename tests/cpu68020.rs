//! Tests unitaires ciblés du sous-ensemble 68020 (`CpuType::M68020`).
//!
//! Comme pour `tests/cpu68010.rs`, aucun vecteur SingleStepTests n'existe
//! pour le 68020 (vérifié auprès de github.com/SingleStepTests/680x0 : seul
//! `68000/` y est publié) — tests écrits à la main. La partie adressage
//! (mot d'extension "complet", voir `Cpu::resolve_indexed_full`) est la
//! plus risquée (aucun filet de conformité automatique, beaucoup de
//! combinaisons SCALE/BS/IS/BD SIZE) : couverte par une matrice
//! systématique plutôt que 2-3 sondages ponctuels.
//!
//! Toutes les adresses effectives sont observées via LEA (aucun accès
//! mémoire réel nécessaire pour vérifier le calcul d'adresse lui-même).

use rust68::{Bus, Cpu, CpuType, FlatBus};

/// Construit un CPU + bus, place un vecteur de reset (SSP=0x2000, PC=0x0400)
/// puis charge `words` à partir de 0x0400 — même convention que
/// `tests/cpu68010.rs`/`tests/instructions.rs`.
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
    cpu.cpu_type = CpuType::M68020;
    (cpu, bus)
}

// --- Adressage : mot d'extension complet, LEA (d8,A0,Xn),A1 = 0x43F0 -------
// (aaa=001=A1, opmode=111, mode=110, base An=000=A0 — voir la doc du fichier)

#[test]
fn full_format_scale_x1_x2_x4_x8() {
    // bit11=1(long) bit8=1(complet) bits5-4=01(BD nul) — seul SCALE varie.
    let cases = [(0x0910u16, 1u32), (0x0B10, 2), (0x0D10, 4), (0x0F10, 8)];
    for (ext, expected_shift) in cases {
        let (mut cpu, mut bus) = setup(&[0x43F0, ext]);
        cpu.a[0] = 0x0000_1000;
        cpu.d[0] = 1;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.a[1], 0x0000_1000 + expected_shift, "ext={ext:#06x}");
    }
}

#[test]
fn full_format_base_suppress() {
    // IS=1 (index supprimé) fixe ; BD=$0050 (mot) ; BS varie.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0960, 0x0050]); // BS=0
    cpu.a[0] = 0x0000_2000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0000_2050, "BS=0 : base incluse");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x09E0, 0x0050]); // BS=1
    cpu.a[0] = 0x0000_2000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0000_0050, "BS=1 : base supprimée, ignorée même non nulle");
}

#[test]
fn full_format_index_suppress() {
    // BS=1 (base supprimée) fixe, BD nul ; index D0=3 (long, ×1) ; IS varie.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0990]); // IS=0
    cpu.a[0] = 0x9999_0000;
    cpu.d[0] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 3, "IS=0 : index inclus");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x09D0]); // IS=1
    cpu.a[0] = 0x9999_0000;
    cpu.d[0] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0, "IS=1 : index supprimé, ignoré même non nul");
}

#[test]
fn full_format_bd_size_nul_mot_long() {
    // BS=1 et IS=1 fixes (adresse = BD seul).
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x01D0]); // BD nul
    cpu.a[0] = 0x1234_5678;
    cpu.d[0] = 0xFFFF_FFFF;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0, "BD nul");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x01E0, 0xFFF0]); // BD mot, négatif
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0xFFFF_FFF0, "BD mot : extension de signe (-16)");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x01F0, 0x0001, 0x2345]); // BD long
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0001_2345, "BD long : 32 bits complets");
}

#[test]
fn full_format_tous_les_termes_non_nuls() {
    // BS=0, IS=0, SCALE=×4, BD=$0100 (mot) — détecte une erreur
    // d'accumulation/de signe qui ne se verrait pas si un terme était nul.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0D20, 0x0100]);
    cpu.a[0] = 0x0001_0000;
    cpu.d[0] = 5;
    cpu.step(&mut bus).unwrap();
    // 0x00010000 + 0x0100 + (5<<2=20=0x14)
    assert_eq!(cpu.a[1], 0x0001_0114);
}

#[test]
fn full_format_index_long_vs_mot() {
    // D0 = 0x0001_FFF0 : en mot (bits bas $FFF0 = -16 signé) vs en long
    // (0x0001FFF0, positif) — deux résultats très différents, détecte un
    // mauvais traitement du bit W/L de l'index.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0190]); // bit11=0 (mot)
    cpu.a[0] = 0;
    cpu.d[0] = 0x0001_FFF0;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0xFFFF_FFF0, "index mot : signe étendu depuis les 16 bits bas");

    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0990]); // bit11=1 (long)
    cpu.a[0] = 0;
    cpu.d[0] = 0x0001_FFF0;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.a[1], 0x0001_FFF0, "index long : valeur 32 bits complète");
}

#[test]
fn full_format_via_pc_relatif() {
    // LEA (d8,PC,Xn),A1 = 0x43FB (mode=111,reg=011) — `resolve_indexed` est
    // partagée par les deux appelants, vérifie que le format complet
    // fonctionne aussi via ce second chemin.
    let (mut cpu, mut bus) = setup(&[0x43FB, 0x0160, 0x0010]); // IS=1, BD=$0010 mot
    cpu.step(&mut bus).unwrap();
    // base = PC juste après l'opcode = 0x0402 (avant tout mot d'extension).
    assert_eq!(cpu.a[1], 0x0402 + 0x0010);
}

#[test]
fn full_format_indirection_memoire_non_geree() {
    // I/IS=001 (bits2-0) : indirection mémoire, hors périmètre — ne doit
    // pas calculer une adresse silencieusement fausse. En interne,
    // `execute` échoue explicitement (`StepError::IllegalAddressing`),
    // mais `step` ne le laisse pas remonter : comme le silicium réel (et
    // vérifié contre Hatari), il dispatche l'exception illegal instruction
    // (vecteur 4) plutôt que de planter — voir le commentaire au site
    // d'interception dans `Cpu::step`.
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0911]);
    cpu.a[0] = 0x1000;
    bus.write32(0x10, 0x0000_0600); // vecteur 4
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

// --- EXTB.L ------------------------------------------------------------------

#[test]
fn extb_l_octet_positif_et_negatif() {
    let (mut cpu, mut bus) = setup(&[0x49C0]); // EXTB.L D0
    cpu.d[0] = 0x1234_5642; // octet bas = 0x42 (positif)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0x0000_0042);

    let (mut cpu, mut bus) = setup(&[0x49C0]);
    cpu.d[0] = 0x1234_56F0; // octet bas = 0xF0 (négatif, -16)
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 0xFFFF_FFF0);
}

// --- MULU.L / MULS.L -----------------------------------------------------

#[test]
fn mulu_l_32x32_vers_32() {
    let (mut cpu, mut bus) = setup(&[0x4C01, 0x0000]); // MULU.L D1,D0
    cpu.d[0] = 1000;
    cpu.d[1] = 2000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 2_000_000);
}

#[test]
fn muls_l_operande_negatif() {
    let (mut cpu, mut bus) = setup(&[0x4C01, 0x0800]); // MULS.L D1,D0 (bit11=1)
    cpu.d[0] = 0xFFFF_FFFB; // -5
    cpu.d[1] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0] as i32, -15);
}

#[test]
fn mulu_l_resultat_64_bits() {
    // MULU.L D1,D2:D0 (Dh=D2, Dl=D0, résultat 64 bits)
    let (mut cpu, mut bus) = setup(&[0x4C01, 0x2400]);
    cpu.d[0] = 0x0001_0000;
    cpu.d[1] = 0x0001_0000;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[2], 1, "poids fort");
    assert_eq!(cpu.d[0], 0, "poids faible");
}

// --- DIVU.L / DIVS.L -----------------------------------------------------

#[test]
fn divu_l_32_sur_32_avec_reste() {
    // DIVU.L D1,D2:D0 (Dr=D2, Dq=D0, dividende 32 bits)
    let (mut cpu, mut bus) = setup(&[0x4C41, 0x2000]);
    cpu.d[0] = 100;
    cpu.d[1] = 7;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 14, "quotient");
    assert_eq!(cpu.d[2], 2, "reste");
}

#[test]
fn divu_l_64_sur_32() {
    // DIVU.L D1,D2:D0 avec bit10=1 (dividende 64 bits, Dr:Dq = D2:D0 = 10)
    let (mut cpu, mut bus) = setup(&[0x4C41, 0x2400]);
    cpu.d[0] = 10;
    cpu.d[2] = 0;
    cpu.d[1] = 3;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.d[0], 3, "quotient");
    assert_eq!(cpu.d[2], 1, "reste");
}

#[test]
fn divu_l_par_zero_leve_vecteur_5() {
    let (mut cpu, mut bus) = setup(&[0x4C41, 0x2000]);
    cpu.d[1] = 0; // diviseur nul
    bus.write32(0x14, 0x0000_0600); // vecteur 5
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc, 0x0600);
}

// --- Régression 68000/68010 ------------------------------------------------

#[test]
fn nouvelles_instructions_illegal_sur_68000_et_68010() {
    for cpu_type in [CpuType::M68000, CpuType::M68010] {
        for words in [
            [0x49C0u16, 0].as_slice(),           // EXTB.L
            [0x4C01, 0x0000].as_slice(),         // MULU.L
            [0x4C41, 0x2000].as_slice(),         // DIVU.L
        ] {
            let (mut cpu, mut bus) = setup(words);
            cpu.cpu_type = cpu_type;
            bus.write32(0x10, 0x0000_0600); // vecteur 4
            cpu.step(&mut bus).unwrap();
            assert_eq!(cpu.pc, 0x0600, "cpu_type={cpu_type:?} words={words:?}");
        }
    }
}

#[test]
fn mot_d_extension_complet_ignore_sur_68010_decode_en_bref() {
    // Sous 68010, le bit 8 du mot d'extension n'est jamais inspecté : LEA
    // (d8,A0,Xn),A1 avec un mot qui SERAIT un format complet sous 68020 doit
    // être décodé comme format bref (déplacement 8 bits signé = octet bas).
    let (mut cpu, mut bus) = setup(&[0x43F0, 0x0910]); // octet bas = 0x10
    cpu.cpu_type = CpuType::M68010;
    cpu.a[0] = 0x0000_1000;
    cpu.d[0] = 1; // index bref : ajouté tel quel (word, non étendu ici car =1)
    cpu.step(&mut bus).unwrap();
    // Format bref : addr = A0 + index(D0, mot signé) + déplacement 8 bits ($10).
    assert_eq!(cpu.a[1], 0x0000_1000 + 1 + 0x10);
}
