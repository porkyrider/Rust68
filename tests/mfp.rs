#![cfg(feature = "atari-st")]
//! Tests unitaires du MC68901 MFP (`rust68::peripherals::atari_st::mfp`).
//!
//! Aucune suite TomHarte n'existe pour ce périphérique (spécifique Atari
//! ST, pas partie de la famille 68k elle-même) : ce fichier est le seul
//! filet de sécurité, sur le même principe que `tests/instructions.rs`.

use rust68::peripherals::atari_st::mfp::{Mfp, channel, reg};

#[test]
fn gpip_lecture_reflete_ddr() {
    let mut mfp = Mfp::new();
    // DDR=0x0F : P0-P3 en sortie, P4-P7 en entrée.
    mfp.write(reg::DDR, 0x0F);
    mfp.write(reg::GPIP, 0b0000_0101); // latch de sortie pour P0-P3
    mfp.set_gpip_input(4, true); // P4 (entrée) à 1

    let gpip = mfp.read(reg::GPIP);
    assert_eq!(gpip & 0x0F, 0b0101, "les bits de sortie reflètent le latch écrit");
    assert_eq!(gpip & 0x10, 0x10, "P4 (entrée) reflète le niveau appliqué");
}

#[test]
fn gpip_front_montant_ou_descendant_selon_aer() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00); // tout en entrée
    mfp.write(reg::IERB, 1 << channel::GPIP0); // activer le canal GPIP0
    mfp.write(reg::IMRB, 1 << channel::GPIP0);

    // AER bit0 = 0 : déclenche sur front DESCENDANT.
    mfp.write(reg::AER, 0x00);
    mfp.set_gpip_input(0, true); // montant : ne doit rien déclencher
    assert!(!mfp.interrupt_requested());
    mfp.set_gpip_input(0, false); // descendant : doit déclencher
    assert!(mfp.interrupt_requested());

    // Acquitter, puis vérifier le sens inverse avec AER bit0 = 1.
    mfp.iack();
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::IERB, 1 << channel::GPIP0);
    mfp.write(reg::IMRB, 1 << channel::GPIP0);
    mfp.write(reg::AER, 0x01); // front montant
    mfp.set_gpip_input(0, true);
    assert!(mfp.interrupt_requested());
}

#[test]
fn interruption_masquee_sans_ier_ni_imr() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    // IER non activé : le front ne doit même pas armer IPR.
    mfp.set_gpip_input(0, true);
    assert!(!mfp.interrupt_requested());
    assert_eq!(mfp.read(reg::IPRB) & 1, 0);

    // IER activé mais IMR non activé : IPR s'arme, mais pas de requête visible.
    mfp.write(reg::IERB, 0x01);
    mfp.set_gpip_input(0, false);
    mfp.set_gpip_input(0, true);
    assert_eq!(mfp.read(reg::IPRB) & 1, 1, "IPR s'arme dès que IER est actif");
    assert!(
        !mfp.interrupt_requested(),
        "IMR non activé : pas de requête vers le CPU"
    );
}

#[test]
fn ipr_isr_ne_s_effacent_que_par_ecriture_de_zero() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    mfp.write(reg::IERB, 0x01);
    mfp.set_gpip_input(0, true);
    assert_eq!(mfp.read(reg::IPRB) & 1, 1);

    // Écrire 1 ne doit pas ré-armer / n'a aucun effet (bit déjà à 1).
    mfp.write(reg::IPRB, 0xFF);
    assert_eq!(mfp.read(reg::IPRB) & 1, 1, "écrire 1 n'affecte pas le bit");

    // Écrire 0 efface.
    mfp.write(reg::IPRB, 0xFE);
    assert_eq!(mfp.read(reg::IPRB) & 1, 0, "écrire 0 efface le bit");
}

#[test]
fn iack_calcule_le_vecteur_et_arme_isr_dans_les_deux_modes_eoi() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    mfp.write(reg::IERB, 1 << channel::GPIP0);
    mfp.write(reg::IMRB, 1 << channel::GPIP0);
    mfp.write(reg::VR, 0x40); // base vecteur 0x40 (bits 7-3), pas d'auto-EOI (bit3=0)
    mfp.set_gpip_input(0, true);

    let vector = mfp.iack();
    assert_eq!(vector, 0x40, "vecteur = VR[7:4] | canal (canal 0 ici)");
    assert_eq!(
        mfp.read(reg::ISRB) & 1,
        1,
        "sans auto-EOI, ISR s'arme après l'IACK"
    );
    assert_eq!(mfp.read(reg::IPRB) & 1, 0, "IPR est effacé par l'IACK");

    // Mode auto-EOI (bit 3 du VR) : ISR s'arme quand même après l'IACK — ce
    // bit dispense seulement le logiciel d'avoir à l'effacer lui-même en fin
    // de traitement (le matériel le ferait automatiquement), il ne rend pas
    // le bit invisible pendant que le gestionnaire tourne. Confirmé par la
    // cartouche de diagnostic usine STe : son gestionnaire d'interruption
    // partagé (test "T0 MFP timer") programme VR=0x48 puis distingue quel
    // timer a déclenché en lisant précisément ces bits ISR — un mécanisme
    // qui ne pourrait jamais fonctionner sur aucun ST/STE réel si ISR
    // n'était jamais armé en auto-EOI.
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0x01);
    mfp.write(reg::IERB, 1 << channel::GPIP0);
    mfp.write(reg::IMRB, 1 << channel::GPIP0);
    mfp.write(reg::VR, 0x48); // bit3 = auto-EOI
    mfp.set_gpip_input(0, true);
    mfp.iack();
    assert_eq!(mfp.read(reg::ISRB) & 1, 1, "auto-EOI : ISR s'arme aussi après l'IACK");
}

#[test]
fn iack_exclut_le_bit_s_auto_eoi_du_vecteur() {
    // Le bit 3 du VR (S, auto-EOI) est un bit de contrôle séparé, PAS le
    // bit de poids fort du numéro de canal : seuls VR[7:4] forment la base
    // du vecteur, les 4 bits de canal (0-15) occupent tout le bas. Un TOS
    // réel programme VR=0x48 (base 0x40, auto-EOI actif) puis installe ses
    // gestionnaires à `0x100 + canal*4` (vecteur `0x40 | canal`) — confondre
    // le bit S avec le bit haut du canal calculerait `0x48 | canal` à la
    // place, un décalage de 8 vecteurs (32 octets) qui fait vectorer vers
    // un gestionnaire jamais installé (bug réel rencontré en faisant
    // démarrer un TOS réel : Timer C, VR=0x48, vectorait vers `$134`
    // au lieu de `$114` où le TOS avait réellement installé son
    // gestionnaire).
    let mut mfp = Mfp::new();
    mfp.write(reg::IERB, 1 << channel::TIMER_C);
    mfp.write(reg::IMRB, 1 << channel::TIMER_C);
    mfp.write(reg::VR, 0x48);
    // Timer C en mode delay, prescaler ÷4 (control=1), data=1 : décompte
    // dès le premier tick et déclenche `request()` en interne. La donnée
    // doit être écrite AVANT le contrôle : `reload()` (déclenché par
    // l'écriture de TCDCR) recharge le compteur depuis la donnée.
    mfp.write(reg::TCDR, 1);
    mfp.write(reg::TCDCR, 0x01 << 4);
    mfp.tick(100);

    let vector = mfp.iack();
    assert_eq!(
        vector,
        0x40 | channel::TIMER_C,
        "VR=0x48 avec S=1 : vecteur = VR[7:4](0x40) | canal, pas VR[7:3](0x48) | canal"
    );
}

#[test]
fn priorite_du_canal_le_plus_haut() {
    let mut mfp = Mfp::new();
    mfp.write(reg::DDR, 0x00);
    mfp.write(reg::AER, 0xFF); // tous fronts montants
    mfp.write(reg::IERB, (1 << channel::GPIP0) | (1 << channel::GPIP2));
    mfp.write(reg::IMRB, (1 << channel::GPIP0) | (1 << channel::GPIP2));
    mfp.set_gpip_input(0, true);
    mfp.set_gpip_input(2, true);

    // Canal 2 (GPIP2) > canal 0 (GPIP0) : doit être acquitté en premier.
    let vector = mfp.iack();
    assert_eq!(vector & 0x07, channel::GPIP2);
    let vector2 = mfp.iack();
    assert_eq!(vector2 & 0x07, channel::GPIP0);
}

#[test]
fn timer_a_delay_mode_declenche_et_recharge() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 5); // valeur de rechargement
    mfp.write(reg::TACR, 1); // démarre en mode delay, ÷4

    // Lecture pendant que le timer tourne : renvoie le compte courant, pas 5.
    assert_eq!(mfp.read(reg::TADR), 5);

    // Budget clairement insuffisant : pas encore déclenché.
    mfp.tick(10);
    assert!(
        !mfp.interrupt_requested(),
        "10 cycles CPU ne suffisent pas à épuiser data=5 en ÷4"
    );

    // Budget généreux (largement supérieur à (5+1)*4 cycles MFP convertis
    // en cycles CPU) : doit avoir déclenché au moins une fois.
    for _ in 0..30 {
        mfp.tick(10);
    }
    assert!(
        mfp.interrupt_requested(),
        "budget généreux : le timer A doit avoir déclenché"
    );
    assert_eq!(mfp.read(reg::IPRA) & (1 << (channel::TIMER_A - 8)), 1 << (channel::TIMER_A - 8));
}

#[test]
fn timer_arrete_ne_declenche_jamais() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 1); // période minimale
    mfp.write(reg::TACR, 0); // arrêté

    for _ in 0..1000 {
        mfp.tick(100);
    }
    assert!(!mfp.interrupt_requested(), "timer arrêté (control=0) : jamais de déclenchement");
    assert_eq!(mfp.read(reg::TADR), 1, "lecture à l'arrêt = valeur de rechargement telle quelle");
}

#[test]
fn timer_a_event_count_ignore_tick_et_reagit_a_pulse() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::IMRA, 1 << (channel::TIMER_A - 8));
    mfp.write(reg::TADR, 3);
    mfp.write(reg::TACR, 0x08); // mode event-count

    // tick() ne doit jamais faire progresser un timer en event-count.
    for _ in 0..10_000 {
        mfp.tick(1000);
    }
    assert!(!mfp.interrupt_requested(), "event-count : tick() ne doit rien décompter");

    // pulse_ta() doit décompter : période = data = 3 pulses (même compteur
    // que le mode delay, voir la doc de `Timer::decrement` — le compte
    // affiché juste après rechargement vaut déjà `data`, donc le
    // déclenchement a lieu au `data`-ième décrément, pas au `data+1`-ième).
    for _ in 0..2 {
        mfp.pulse_ta();
        assert!(!mfp.interrupt_requested());
    }
    mfp.pulse_ta();
    assert!(mfp.interrupt_requested(), "3e pulse : déclenchement attendu");
}

#[test]
fn timer_c_et_d_partagent_tcdcr() {
    let mut mfp = Mfp::new();
    mfp.write(reg::TCDCR, (0x03 << 4) | 0x02); // Timer C = ÷16, Timer D = ÷10
    assert_eq!(mfp.read(reg::TCDCR), (0x03 << 4) | 0x02);
}

#[test]
fn usart_reception_et_transmission_au_niveau_octet() {
    let mut mfp = Mfp::new();
    mfp.write(reg::IERA, 1 << (channel::RX_FULL - 8));
    mfp.write(reg::IMRA, 1 << (channel::RX_FULL - 8));

    mfp.push_rx_byte(0x41);
    assert!(mfp.interrupt_requested(), "octet reçu : RX_FULL doit s'armer");
    assert_eq!(mfp.read(reg::RSR) & 0x80, 0x80, "buffer full");
    assert_eq!(mfp.read(reg::UDR), 0x41, "lecture d'UDR renvoie l'octet reçu");
    assert_eq!(mfp.read(reg::RSR) & 0x80, 0, "lecture d'UDR efface buffer full");

    // Transmission : écrire UDR doit être récupérable via take_tx_byte.
    mfp.write(reg::UDR, 0x42);
    assert_eq!(mfp.take_tx_byte(), Some(0x42));
    assert_eq!(mfp.take_tx_byte(), None);
    assert_eq!(mfp.read(reg::TSR) & 0x80, 0x80, "buffer empty après transmission");
}

#[test]
fn tsr_buffer_empty_a_1_au_reset_et_survit_a_l_activation_emetteur() {
    // Reproduit la séquence d'initialisation standard de l'USART (ex.
    // cartouche de diagnostic usine STe) : TSR démarre à "buffer empty" par
    // défaut (aucun octet en attente au reset), et l'activer via bit0
    // (Transmitter Enable) ne doit pas effacer ce bit de statut matériel —
    // sinon plus aucune transmission ne peut jamais avoir lieu ensuite.
    let mut mfp = Mfp::new();
    assert_eq!(mfp.read(reg::TSR) & 0x80, 0x80, "buffer empty dès le reset");

    mfp.write(reg::TSR, 0x01); // active l'émetteur (bit0), comme le ferait un vrai pilote
    assert_eq!(
        mfp.read(reg::TSR) & 0x80,
        0x80,
        "buffer empty doit survivre à l'activation de l'émetteur"
    );
    assert_eq!(mfp.read(reg::TSR) & 0x01, 0x01, "bit d'activation bien pris en compte");

    mfp.write(reg::UDR, 0x55);
    assert_eq!(mfp.take_tx_byte(), Some(0x55), "la transmission doit fonctionner après l'activation");
}
