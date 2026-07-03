//! Cœur du CPU Motorola 68000.
//!
//! Cette première version cible le **MC68000** d'origine (Atari ST, Amiga 500…).
//! Les variantes ultérieures (68010, 68020…) s'appuieront sur le même modèle de
//! registres, étendu au besoin.

use crate::bus::Bus;

/// Masque d'adressage du 68000 : bus d'adresses de 24 bits (16 Mo).
pub const ADDR_MASK: u32 = 0x00FF_FFFF;

/// Bits du Condition Code Register (CCR, octet bas du SR).
pub mod ccr {
    /// Carry — retenue.
    pub const C: u16 = 1 << 0;
    /// Overflow — débordement signé.
    pub const V: u16 = 1 << 1;
    /// Zero — résultat nul.
    pub const Z: u16 = 1 << 2;
    /// Negative — bit de signe du résultat.
    pub const N: u16 = 1 << 3;
    /// Extend — retenue étendue (arithmétique multi-précision).
    pub const X: u16 = 1 << 4;
}

/// Bits de l'octet système du Status Register (SR).
pub mod sr {
    /// Supervisor — mode superviseur (vs utilisateur).
    pub const S: u16 = 1 << 13;
    /// Trace — exécution pas à pas.
    pub const T: u16 = 1 << 15;
    /// Masque du niveau de priorité d'interruption (IPL, bits 8-10).
    pub const IPL_MASK: u16 = 0b111 << 8;
}

/// Indice du pointeur de pile dans le banc de registres d'adresse (A7).
const SP: usize = 7;

/// État complet d'un cœur 68000.
///
/// Le CPU ne contient pas de mémoire : tous les accès passent par un [`Bus`]
/// fourni par l'appelant aux méthodes qui en ont besoin.
#[derive(Debug, Clone)]
pub struct Cpu {
    /// Registres de données D0–D7.
    pub d: [u32; 8],
    /// Registres d'adresse A0–A7 ; A7 est le pointeur de pile **actif**.
    pub a: [u32; 8],
    /// Compteur de programme.
    pub pc: u32,
    /// Status Register (octet système + CCR).
    pub sr: u16,
    /// Pointeur de pile utilisateur sauvegardé quand on est en superviseur.
    pub usp: u32,
    /// Pointeur de pile superviseur sauvegardé quand on est en utilisateur.
    pub ssp: u32,
    /// Cycles consommés depuis la dernière remise à zéro du compteur.
    pub cycles: u64,
    /// File de préfetch (pipeline d'instruction du 68000 : 2 mots max).
    /// Les mots sont consommés en FIFO avant toute lecture bus lors d'un fetch.
    pub prefetch: [u16; 2],
    /// Nombre de mots valides dans `prefetch` (0, 1 ou 2).
    pub prefetch_len: usize,
    /// Opcode de l'instruction en cours (Instruction Register, utilisé pour les frames d'exception).
    pub current_ir: u16,
    /// Adresse erronée si un fetch d'instruction a touché une adresse impaire.
    /// `Some((fault_addr, is_write, pc_at_fault))` déclenche une address error.
    pub pending_address_error: Option<(u32, bool, u32)>,
    /// PC à inscrire dans le frame d'address error pour un accès données via la
    /// dernière EA résolue. Dépend du mode d'adressage (cf. resolve_ea).
    pub ea_frame_pc: u32,
    /// True si la dernière EA résolue est un mode PC-relatif ((d16,PC) ou (d8,PC,Xn)).
    /// Ces modes accèdent à l'espace programme (FC=2/6), pas données (FC=1/5).
    pub ea_is_pc_relative: bool,
    /// IR à utiliser dans le frame d'une write AE, quand il diffère de current_ir.
    /// Pour MOVE.w/b avec dst -(An), le 68000 a déjà avancé son pipeline avant le write :
    /// l'IR dans le frame est le mot suivant dans le flux programme (bus.read16(pc)),
    /// pas l'opcode en cours.
    pub write_ae_ir: Option<u16>,
    /// Cycles supplémentaires de calcul d'adresse effective pour la dernière EA
    /// résolue (dépend du mode d'adressage et de la taille — cf. resolve_ea).
    /// Chaque handler d'instruction l'additionne à son coût de base après avoir
    /// appelé resolve_ea, sur le même principe que ea_frame_pc/ea_is_pc_relative.
    pub ea_extra_cycles: u32,
    /// Préfixe (en cycles) à ajouter devant `ea_extra + 50` si le `ae_read`
    /// qui suit immédiatement déclenche une address error. Calibré cas par
    /// cas contre ProcessorTests (les trois formes coexistent, pas de règle
    /// générale unique) :
    ///   - 4 (défaut) : simple lecture d'opérande source (DIVU/DIVS, `<ea>,Dn`,
    ///     TST, CMP...) — le préfixe est juste le fetch de l'opcode.
    ///   - 0 : relecture RMW de la valeur destination pour un `Dn,<ea>` à deux
    ///     opérandes registre+mémoire (`OR/AND/EOR/ADD/SUB Dn,<ea>`).
    ///   - 8 : relecture RMW dans la famille immédiat-vers-mémoire
    ///     (`ORI/ANDI/SUBI/ADDI/EORI`, partagent `op_line_0`) — le préfixe
    ///     inclut le fetch de l'immédiat en plus de l'opcode.
    /// Remis à 4 au début de chaque `step()`.
    pub fault_prefix: u32,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// Crée un CPU dans un état neutre (avant `reset`).
    pub fn new() -> Self {
        Cpu {
            d: [0xFFFF_FFFF; 8],
            a: [0xFFFF_FFFF; 8],
            pc: 0,
            sr: sr::S,
            usp: 0,
            ssp: 0,
            cycles: 0,
            prefetch: [0; 2],
            prefetch_len: 0,
            current_ir: 0,
            pending_address_error: None,
            ea_frame_pc: 0,
            ea_is_pc_relative: false,
            write_ae_ir: None,
            ea_extra_cycles: 0,
            fault_prefix: 4,
        }
    }

    /// Charge des mots dans la file de préfetch (utilisé par les harnais de test).
    ///
    /// Les mots sont fournis dans l'ordre de lecture (premier mot = prochain à consommer).
    pub fn load_prefetch(&mut self, words: &[u16]) {
        let n = words.len().min(2);
        self.prefetch_len = n;
        for i in 0..n {
            self.prefetch[i] = words[i];
        }
    }

    /// Indique si le CPU est en mode superviseur.
    #[inline]
    pub fn supervisor(&self) -> bool {
        self.sr & sr::S != 0
    }

    /// Pointeur de pile actif (A7).
    #[inline]
    pub fn sp(&self) -> u32 {
        self.a[SP]
    }

    /// Définit le pointeur de pile actif (A7).
    #[inline]
    pub fn set_sp(&mut self, value: u32) {
        self.a[SP] = value;
    }

    /// Bascule le bit superviseur, en commutant les pointeurs de pile A7.
    ///
    /// Sur le 68000, USP et SSP sont deux registres physiques distincts ; seul
    /// l'un d'eux est exposé via A7 selon le mode courant. Cette méthode gère
    /// l'échange pour que `self.a[7]` reflète toujours la pile du bon mode.
    pub fn set_supervisor(&mut self, supervisor: bool) {
        if supervisor == self.supervisor() {
            return;
        }
        if supervisor {
            // user -> supervisor : on sauve l'USP courant, on restaure le SSP.
            self.usp = self.a[SP];
            self.a[SP] = self.ssp;
            self.sr |= sr::S;
        } else {
            // supervisor -> user : on sauve le SSP courant, on restaure l'USP.
            self.ssp = self.a[SP];
            self.a[SP] = self.usp;
            self.sr &= !sr::S;
        }
    }

    /// Effectue un **reset** matériel.
    ///
    /// Le 68000 charge le SSP initial depuis l'adresse `0x000000` et le PC
    /// initial depuis `0x000004` (les deux premiers mots longs du vecteur de
    /// reset), passe en superviseur, trace désactivée, IPL = 7.
    pub fn reset(&mut self, bus: &mut impl Bus) {
        self.sr = sr::S | sr::IPL_MASK; // superviseur, IPL=7, trace off
        let ssp = bus.read32(0x0000_0000);
        let pc = bus.read32(0x0000_0004);
        self.ssp = ssp;
        self.a[SP] = self.ssp;
        self.pc = pc;
        self.cycles = 0;
    }

    // --- Mécanisme d'exception ----------------------------------------------

    /// Déclenche une exception : empile SR + PC sur la pile superviseur,
    /// passe en superviseur, désactive la trace, saute au vecteur.
    ///
    /// `vector` est le numéro de vecteur (0–255). L'adresse du vecteur = vector*4.
    /// `pc_to_push` est le PC à empiler (adresse de retour ou adresse de l'instruction).
    pub fn take_exception(&mut self, bus: &mut impl Bus, vector: u32, pc_to_push: u32) {
        // Passer en mode superviseur sans changer les flags CCR
        let saved_sr = self.sr;
        if !self.supervisor() {
            self.usp = self.a[SP];
            self.a[SP] = self.ssp;
        }
        self.sr = (saved_sr | sr::S) & !sr::T;
        self.sr &= 0xA71F; // masque bits réservés

        // Empiler SR puis PC (format: SR word, PC longword)
        let sp = self.a[SP].wrapping_sub(6);
        self.a[SP] = sp;
        bus.write16(sp & ADDR_MASK, saved_sr);
        bus.write32((sp + 2) & ADDR_MASK, pc_to_push);

        // Lire l'adresse du vecteur
        let vec_addr = (vector * 4) & ADDR_MASK;
        let new_pc = bus.read32(vec_addr);
        // TomHarte convention : final.pc = m_au = new_pc + 4.
        // Notre modèle: cpu.pc + 4 = final.pc → cpu.pc = new_pc.
        self.pc = new_pc;
    }

    /// Déclenche une exception address error (vecteur 3).
    ///
    /// Frame de 14 octets :
    ///   SP+0..1  : access_info = (IR & 0xFFE0) | (R/W << 4) | FC
    ///   SP+2     : 0x00
    ///   SP+3..5  : fault_addr (adresse impaire, 24 bits)
    ///   SP+6..7  : IR (opcode)
    ///   SP+8..9  : SR sauvegardé
    ///   SP+10..13: PC pipeline au moment de l'accès
    pub fn take_address_error(&mut self, bus: &mut impl Bus, fault_addr: u32, is_write: bool) {
        self.take_address_error_at(bus, fault_addr, is_write, None)
    }

    pub fn take_address_error_at(
        &mut self,
        bus: &mut impl Bus,
        fault_addr: u32,
        is_write: bool,
        explicit_pc: Option<u32>,
    ) {
        self.take_address_error_full(bus, fault_addr, is_write, explicit_pc, false)
    }

    pub fn take_address_error_full(
        &mut self,
        bus: &mut impl Bus,
        fault_addr: u32,
        is_write: bool,
        explicit_pc: Option<u32>,
        is_instruction_fetch: bool,
    ) {
        let saved_sr = self.sr;
        // Sur le 68000 réel, le pipeline effectue un préfetch avant tout write cycle,
        // avançant le PC de 2 supplémentaires par rapport aux read cycles.
        let pc_at_access = explicit_pc.unwrap_or_else(|| {
            if is_write {
                self.pc.wrapping_add(2)
            } else {
                self.pc
            }
        });
        let ir = if is_write {
            self.write_ae_ir.unwrap_or(self.current_ir)
        } else {
            self.current_ir
        };
        self.write_ae_ir = None;

        if !self.supervisor() {
            self.usp = self.a[7];
            self.a[7] = self.ssp;
        }
        self.sr = (saved_sr | sr::S) & !sr::T;
        self.sr &= 0xA71F;

        // FC : 1=user data, 2=user program, 5=supervisor data, 6=supervisor program
        // Les modes PC-relatifs ((d16,PC),(d8,PC,Xn)) accèdent à l'espace programme.
        let supervisor = saved_sr & sr::S != 0;
        let is_program = is_instruction_fetch || self.ea_is_pc_relative;
        let fc: u16 = match (supervisor, is_program) {
            (false, false) => 1, // user data
            (false, true) => 2,  // user program
            (true, false) => 5,  // supervisor data
            (true, true) => 6,   // supervisor program
        };
        let rw_bit: u16 = if is_write { 0 } else { 1 };
        let access_info = (ir & 0xFFE0) | (rw_bit << 4) | fc;

        let sp = self.a[7].wrapping_sub(14);
        self.a[7] = sp;

        bus.write16(sp & ADDR_MASK, access_info);
        // L'adresse fautive est stockée en 32 bits (+2..+5), MSB inclus.
        bus.write32(sp.wrapping_add(2) & ADDR_MASK, fault_addr);
        bus.write16(sp.wrapping_add(6) & ADDR_MASK, ir);
        bus.write16(sp.wrapping_add(8) & ADDR_MASK, saved_sr);
        bus.write32(sp.wrapping_add(10) & ADDR_MASK, pc_at_access);

        let new_pc = bus.read32(3 * 4);
        self.pc = new_pc;
    }

    // --- Accès groupés à l'octet CCR ---------------------------------------

    /// Renvoie l'octet bas du SR (les flags CCR : X N Z V C).
    #[inline]
    pub fn ccr(&self) -> u8 {
        self.sr as u8
    }

    /// Écrit le SR en gérant le switch USP/SSP si le bit S change.
    pub fn write_sr(&mut self, new_sr: u16) {
        let old_super = self.supervisor();
        self.sr = new_sr;
        let new_super = self.supervisor();
        if old_super && !new_super {
            self.ssp = self.a[7];
            self.a[7] = self.usp;
        } else if !old_super && new_super {
            self.usp = self.a[7];
            self.a[7] = self.ssp;
        }
    }

    /// Positionne un drapeau CCR à `set`.
    #[inline]
    pub fn set_flag(&mut self, flag: u16, set: bool) {
        if set {
            self.sr |= flag;
        } else {
            self.sr &= !flag;
        }
    }

    /// Renvoie l'état d'un drapeau (CCR ou bit système).
    #[inline]
    pub fn flag(&self, flag: u16) -> bool {
        self.sr & flag != 0
    }

    // --- Lecture du flot d'instructions ------------------------------------

    /// Lit le mot 16 bits pointé par le PC et avance le PC de 2.
    ///
    /// Le PC interne est 32 bits (le 68000 ne masque que lors des accès bus).
    /// Si la file de préfetch contient des mots, le premier est consommé (sans
    /// accès bus). Cela permet aux harnais de test d'injecter le pipeline
    /// hardware sans écraser la mémoire de données.
    pub fn fetch_word(&mut self, bus: &mut impl Bus) -> u16 {
        let addr = self.pc;
        self.pc = self.pc.wrapping_add(2);
        if self.prefetch_len > 0 {
            let word = self.prefetch[0];
            self.prefetch[0] = self.prefetch[1];
            self.prefetch_len -= 1;
            word
        } else {
            // Détection d'adresse impaire sur fetch instruction (FC = programme)
            if addr & 1 != 0 && self.pending_address_error.is_none() {
                self.pending_address_error = Some((addr, false, self.pc));
            }
            bus.read16(addr & ADDR_MASK)
        }
    }

    /// Lit le mot long 32 bits pointé par le PC et avance le PC de 4.
    pub fn fetch_long(&mut self, bus: &mut impl Bus) -> u32 {
        let hi = self.fetch_word(bus) as u32;
        let lo = self.fetch_word(bus) as u32;
        (hi << 16) | lo
    }
}
