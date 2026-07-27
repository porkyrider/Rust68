//! Décodage et exécution des instructions du MC68000.

use crate::addressing::{Operand, Size};
use crate::bus::Bus;
use crate::cpu::{ADDR_MASK, Cpu, ccr};

/// Erreur d'exécution non gérée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepError {
    /// Opcode non encore implémenté.
    Unimplemented(u16),
    /// Encodage d'adresse effective invalide.
    IllegalAddressing,
    /// Accès word/long à une adresse impaire : address error (vecteur 3).
    /// Champs : (adresse_impaire, is_write, pc_au_moment_de_l_acces)
    AddressError(u32, bool, u32),
}

/// Convertit une erreur d'adresse de lecture en StepError.
#[inline(always)]
fn ae_read(r: Result<u32, (u32, u32)>) -> Result<u32, StepError> {
    r.map_err(|(addr, pc)| StepError::AddressError(addr, false, pc))
}

/// Convertit une erreur d'adresse d'écriture en StepError.
#[inline(always)]
fn ae_write(r: Result<(), (u32, u32)>) -> Result<(), StepError> {
    r.map_err(|(addr, pc)| StepError::AddressError(addr, true, pc))
}

/// Coût cycle-exact du cœur microcode DIVU (algorithme non restaurateur du
/// 68000, cf. Yacht.txt "DIVU" flowchart + port direct de la référence WinUAE
/// `getDivu68kCycles`, utilisée par Hatari — vendored dans ce dépôt). Ne
/// couvre PAS le cas diviseur nul (traité séparément par l'appelant) ni le
/// coût de fetch de l'opcode/EA (ajoutés par l'appelant : `+ 4 + ea_extra`).
/// Calibré et vérifié à 100% (435/435 cas) contre la suite de conformité
/// ProcessorTests (SingleStepTests/m68000, MAME microcoded core).
fn divu_core_cycles(dividend: u32, divisor: u16) -> u32 {
    let divisor = divisor as u32;
    // Overflow : le quotient ne tiendrait pas sur 16 bits.
    if (dividend >> 16) >= divisor {
        return 5 * 2 - 4;
    }
    let mut mcycles: i32 = 38;
    let hdivisor = divisor << 16;
    let mut dividend = dividend;
    for _ in 0..15 {
        let temp = dividend;
        dividend <<= 1;
        if (temp as i32) < 0 {
            // Retenue depuis le décalage : soustraction implicite.
            dividend = dividend.wrapping_sub(hdivisor);
        } else {
            mcycles += 2;
            if dividend >= hdivisor {
                dividend = dividend.wrapping_sub(hdivisor);
                mcycles -= 1;
            }
        }
    }
    (mcycles * 2 - 4) as u32
}

/// Coût cycle-exact du cœur microcode DIVS (port direct de la référence
/// WinUAE `getDivs68kCycles`). Mêmes conditions que [`divu_core_cycles`] :
/// diviseur nul exclu, fetch opcode/EA ajoutés par l'appelant. Calibré et
/// vérifié à 100% (423/423 cas) contre ProcessorTests.
fn divs_core_cycles(dividend: i32, divisor: i16) -> u32 {
    let mut mcycles: i32 = 6;
    if dividend < 0 {
        mcycles += 1;
    }
    let abs_dividend = dividend.unsigned_abs();
    let abs_divisor = (divisor as i32).unsigned_abs();
    if (abs_dividend >> 16) >= abs_divisor {
        return ((mcycles + 2) * 2 - 4) as u32;
    }
    let mut aquot = (abs_dividend / abs_divisor) as u16;
    mcycles += 55;
    if divisor >= 0 {
        if dividend >= 0 {
            mcycles -= 1;
        } else {
            mcycles += 1;
        }
    }
    for _ in 0..15 {
        if aquot & 0x8000 == 0 {
            mcycles += 1;
        }
        aquot <<= 1;
    }
    (mcycles * 2 - 4) as u32
}

/// Coût de base de l'écriture destination de MOVE, par mode/taille (Yacht.txt
/// lignes 338-553 du dépôt STAY : tables `<ea>,Dn` / `<ea>,(An)` / ... par
/// mode destination). S'ajoute au `ea_extra_cycles` de la SOURCE (capturé par
/// op_move avant de résoudre la destination) — les deux termes sont
/// indépendants et non redondants (l'un lit, l'autre écrit).
fn move_dst_base(dst_mode: u16, dst_reg: u16, size: Size) -> u32 {
    let long = size == Size::Long;
    match dst_mode {
        0b000 | 0b001 => 4, // Dn, An (MOVEA)
        0b010 | 0b011 | 0b100 => {
            if long {
                12
            } else {
                8
            }
        } // (An), (An)+, -(An)
        0b101 => {
            if long {
                16
            } else {
                12
            }
        } // (d16,An)
        0b110 => {
            if long {
                18
            } else {
                14
            }
        } // (d8,An,Xn)
        0b111 => match dst_reg {
            0b000 => {
                if long {
                    16
                } else {
                    12
                }
            } // (xxx).W
            0b001 => {
                if long {
                    20
                } else {
                    16
                }
            } // (xxx).L
            _ => {
                if long {
                    12
                } else {
                    8
                }
            } // non valide en MOVE dst — repli prudent
        },
        _ => {
            if long {
                12
            } else {
                8
            }
        }
    }
}

/// Coût partagé par OR/AND et la forme générique ADD/SUB (Yacht.txt lignes
/// 1064-1105 et 1258-1299 du dépôt STAY — même famille microcode, même
/// forme). `to_ea` : direction Dn,<ea> (true) ou <ea>,Dn (false). `ea_is_reg`
/// (pertinent seulement si `!to_ea`) : source Dn/An, sans coût d'EA mémoire.
fn logic_op_cost(to_ea: bool, ea_is_reg: bool, size: Size, ea_extra: u32) -> u32 {
    let long = size == Size::Long;
    if to_ea {
        if long { 12 + ea_extra } else { 8 + ea_extra }
    } else if long {
        // ea_is_reg couvre aussi #imm (voir call sites) : sa base (8) suit
        // toujours + ea_extra, qui vaut 0 pour Dn/An donc inchangé pour eux.
        if ea_is_reg { 8 + ea_extra } else { 6 + ea_extra }
    } else {
        // ea_extra vaut 0 pour Dn/An, donc "4" et "4+ea_extra" coïncident déjà
        // pour eux : plus besoin de distinguer ea_is_reg ici (l'ancienne
        // branche "4" nue cassait #imm, qui a besoin de son ea_extra=4/8).
        4 + ea_extra
    }
}

/// Coût partagé par CLR/NEG/NEGX/NOT (Yacht.txt lignes 564-586 du dépôt
/// STAY) : lecture-modification-écriture sur Dn ou en mémoire.
fn rmw_cost(is_dn: bool, size: Size, ea_extra: u32) -> u32 {
    let long = size == Size::Long;
    if is_dn {
        if long { 6 } else { 4 }
    } else if long {
        12 + ea_extra
    } else {
        8 + ea_extra
    }
}

/// Coût de JSR par mode (Yacht.txt lignes 806-814) — table dédiée, ne suit
/// pas le motif générique de `ea_extra_cycles`.
fn jsr_cost(mode: u16, reg: u16) -> u32 {
    match mode {
        0b101 => 18, // (d16,An)
        0b110 => 22, // (d8,An,Xn)
        0b111 => match reg {
            0b000 => 18, // (xxx).W
            0b001 => 20, // (xxx).L
            0b010 => 18, // (d16,PC)
            0b011 => 22, // (d8,PC,Xn)
            _ => 16,
        },
        _ => 16, // (An)
    }
}

/// Coût de JMP par mode (Yacht.txt lignes 817-825).
fn jmp_cost(mode: u16, reg: u16) -> u32 {
    match mode {
        0b101 => 10,
        0b110 => 14,
        0b111 => match reg {
            0b000 => 10,
            0b001 => 12,
            0b010 => 10,
            0b011 => 14,
            _ => 8,
        },
        _ => 8,
    }
}

/// Coût de PEA/LEA par mode (Yacht.txt lignes 634-644 / 794-803) — même forme
/// pour les deux instructions, décalée d'une constante (PEA = LEA + 8).
fn pea_lea_cost(mode: u16, reg: u16, is_pea: bool) -> u32 {
    let lea = match mode {
        0b101 => 8,  // (d16,An)
        0b110 => 12, // (d8,An,Xn)
        0b111 => match reg {
            0b000 => 8,  // (xxx).W
            0b001 => 12, // (xxx).L
            0b010 => 8,  // (d16,PC)
            0b011 => 12, // (d8,PC,Xn)
            _ => 4,
        },
        _ => 4, // (An)
    };
    if is_pea { lea + 8 } else { lea }
}

/// Coût de base de MOVEM, par mode/direction (Yacht.txt lignes 654-696 du
/// dépôt STAY) — indépendant de la taille (word/long ne changent que le coût
/// par registre, `4` ou `8`, ajouté séparément par le nombre de registres du
/// masque). `-(An)` (mode 0b100, R→M seulement) partage la base de `(An)`.
fn movem_base(mode: u16, reg: u16, to_regs: bool) -> u32 {
    if to_regs {
        match mode {
            0b010 | 0b011 => 12, // (An), (An)+
            0b101 => 16,         // (d16,An)
            0b110 => 18,         // (d8,An,Xn)
            0b111 => {
                if reg == 0b001 {
                    20
                } else {
                    16
                }
            } // (xxx).L / (xxx).W
            _ => 12,
        }
    } else {
        match mode {
            0b010 | 0b100 => 8, // (An), -(An)
            0b101 => 12,        // (d16,An)
            0b110 => 14,        // (d8,An,Xn)
            0b111 => {
                if reg == 0b001 {
                    16
                } else {
                    12
                }
            } // (xxx).L / (xxx).W
            _ => 8,
        }
    }
}

impl Cpu {
    /// Lit en mémoire à `addr` (non masquée) avec détection d'address error.
    /// Renvoie `Err((fault_addr, pc))` si word/long sur adresse impaire.
    /// Pour un accès **long**, l'adresse fautive rapportée est `addr + 2`
    /// (le 68000 rapporte l'erreur sur le second cycle word du transfert long).
    fn read_mem_checked(
        &self,
        bus: &mut impl Bus,
        addr: u32,
        size: Size,
    ) -> Result<u32, (u32, u32)> {
        if size != Size::Byte && addr & 1 != 0 {
            let fault = if size == Size::Long {
                addr.wrapping_add(2)
            } else {
                addr
            };
            return Err((fault, self.ea_frame_pc));
        }
        Ok(match size {
            Size::Byte => bus.read8(addr & ADDR_MASK) as u32,
            Size::Word => bus.read16(addr & ADDR_MASK) as u32,
            Size::Long => bus.read32(addr & ADDR_MASK),
        })
    }

    /// Écrit en mémoire à `addr` (non masquée) avec détection d'address error.
    /// `frame_pc` est le PC à inscrire dans le frame d'exception.
    /// Pour un accès **long**, l'adresse fautive rapportée est `addr + 2`.
    fn write_mem_checked(
        &self,
        bus: &mut impl Bus,
        addr: u32,
        size: Size,
        value: u32,
        frame_pc: u32,
    ) -> Result<(), StepError> {
        if size != Size::Byte && addr & 1 != 0 {
            let fault = if size == Size::Long {
                addr.wrapping_add(2)
            } else {
                addr
            };
            return Err(StepError::AddressError(fault, true, frame_pc));
        }
        match size {
            Size::Byte => bus.write8(addr & ADDR_MASK, value as u8),
            Size::Word => bus.write16(addr & ADDR_MASK, value as u16),
            Size::Long => bus.write32(addr & ADDR_MASK, value),
        }
        Ok(())
    }

    /// Vérifie le mode superviseur. Si en user mode, déclenche l'exception
    /// privilege violation (vecteur 8) et renvoie Some(cycles).
    /// Renvoie None si on est en superviseur (exécution normale).
    fn check_privilege(&mut self, bus: &mut impl Bus) -> Option<u32> {
        if !self.supervisor() {
            let pc_push = self.pc.wrapping_sub(2); // opcode_addr
            self.take_exception(bus, 8, pc_push);
            Some(34)
        } else {
            None
        }
    }

    pub fn step(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        self.pending_address_error = None;
        self.fault_prefix = 4;
        // Enveloppe le bus pour appliquer les wait-states DRAM/vidéo (Steem
        // RAM_ACCESS_WS) à chaque transaction réelle. `addressing.rs` et les
        // handlers d'opcodes sont génériques sur `impl Bus` et n'ont pas
        // besoin de savoir qu'ils traversent ce wrapper — l'interception est
        // donc transparente, aucun site d'accès bus n'a besoin d'être modifié.
        // `pos` reprend `self.cycles` : la grille de 4 cycles reste en phase
        // avec l'horloge continue à travers les instructions, pas remise à
        // zéro à chaque step().
        let mut timed = crate::bus::TimedBus {
            inner: bus,
            pos: self.cycles,
            access_count: 0,
        };
        let opcode = self.fetch_word(&mut timed);
        self.current_ir = opcode;
        // PC juste après fetch de l'opcode (= opcode_addr + 2), avant les mots d'extension.
        // C'est le PC à sauvegarder dans le frame si une instruction-fetch AE survient.
        let pc_after_opcode = self.pc;

        // Le fetch de l'opcode lui-même a-t-il touché une adresse sans chip
        // select (le "trou" physique) ? Cas typique : un programme qui a
        // sauté dans une zone non mappée (ex: pile corrompue, ou notre
        // watchdog de crash qui voit le PC sortir de toute plage valide).
        if let Some((fault_addr, is_write)) = timed.take_bus_fault() {
            self.take_bus_error_full(
                &mut timed,
                fault_addr,
                is_write,
                Some(pc_after_opcode.wrapping_sub(2)),
                true,
            );
            return Ok(self.finalize_cycles(&timed, 50));
        }

        let result = self.execute(&mut timed, opcode);
        match result {
            Ok(cycles) => {
                // Yacht.txt "Group 0 : Address error = 50(4/7)" is the cost of
                // the exception dispatch ALONE (vector fetch + frame push +
                // first handler fetch) — it does not replace whatever the
                // faulting instruction had already cost to reach this point
                // (e.g. RTE fully executes at its normal 20 cycles, THEN the
                // address error fires while fetching the next instruction
                // from the bad popped PC: total = 20 + 50 = 70, matching
                // ProcessorTests). Confirmed against conformance data: RTE
                // 20+50=70, RTS 16+50=66, RTR 20+50=70 all match exactly.
                //
                // pending_address_error est setté par BSR/BRA pour override le PC du frame.
                // Pour JMP/RTS, on détecte juste self.pc & 1 et utilise pc_after_opcode.
                if let Some((fault_addr, is_write, explicit_pc)) = self.pending_address_error.take()
                {
                    self.take_address_error_full(
                        &mut timed,
                        fault_addr,
                        is_write,
                        Some(explicit_pc),
                        true,
                    );
                    return Ok(self.finalize_cycles(&timed, cycles + 50));
                }
                // Après exécution, si PC est impaire → address error sur instruction fetch
                // (ex: JMP/RTS qui saute à une adresse impaire)
                if self.pc & 1 != 0 {
                    let fault_addr = self.pc;
                    // PC dans le frame = adresse après fetch de l'opcode courant, PAS le target
                    self.take_address_error_full(
                        &mut timed,
                        fault_addr,
                        false,
                        Some(pc_after_opcode),
                        true,
                    );
                    return Ok(self.finalize_cycles(&timed, cycles + 50));
                }
                // A donnée/écriture pendant l'exécution a-t-elle touché le "trou"
                // physique (au-delà de la RAM installée, avant la ROM) ? C'est le
                // mécanisme utilisé par d'innombrables programmes/démos pour
                // détecter la RAM installée : ils installent leur propre handler
                // au vecteur $8 puis scannent la mémoire jusqu'à faire planter
                // le bus — sans cette exception réelle, le scan continue à
                // l'infini (ou jusqu'à une borne arbitraire) et finit par
                // écraser sa propre pile avec les données qu'il écrit.
                if let Some((fault_addr, is_write)) = timed.take_bus_fault() {
                    self.take_bus_error_full(&mut timed, fault_addr, is_write, None, false);
                    return Ok(self.finalize_cycles(&timed, cycles + 50));
                }
                Ok(self.finalize_cycles(&timed, cycles))
            }
            Err(StepError::AddressError(fault_addr, is_write, pc_at_fault)) => {
                self.take_address_error_at(&mut timed, fault_addr, is_write, Some(pc_at_fault));
                // Fault while reading/writing the operand right after EA
                // resolution (the common case: ae_read(ea.read(...))? /
                // ae_write(ea.write(...))? immediately following resolve_ea).
                // Cost = fault_prefix + ea_extra + 50 (Yacht.txt "Group 0:
                // Address error"). Three prefixes, all calibrated against
                // ProcessorTests — see `Cpu::fault_prefix` doc:
                //  - 4: simple source-operand read (DIVU/DIVS divisor, <ea>,Dn,
                //    TST, CMP...). E.g. (d8,An,Xn)=4+10+50=64, -(An)=4+6+50=60.
                //  - 0: RMW re-read of the OLD destination for a two-operand
                //    Dn,<ea> (OR/AND/EOR/ADD/SUB). E.g. (An)=0+8+50=58.
                //  - 8: immediate-to-memory RMW family sharing op_line_0
                //    (ORI/ANDI/SUBI/ADDI/EORI). E.g. -(An)=8+6+50=64.
                // Instructions whose fault happens somewhere other than "right
                // after resolve_ea" (e.g. UNLINK reading straight from An) have
                // their own different prefix cost and are not covered here —
                // tracked separately.
                let table_cost = self.fault_prefix + self.ea_extra_cycles + 50;
                Ok(self.finalize_cycles(&timed, table_cost))
            }
            Err(e) => Err(e),
        }
    }

    /// Combine les cycles bus réellement observés pendant l'instruction
    /// (transactions × 4 + wait-states insérés, déjà accumulés dans
    /// `timed.pos`) avec la part purement interne du coût de table calculé
    /// par le handler d'opcode (`table_cost` moins les 4 cycles par
    /// transaction déjà comptés côté bus, pour éviter un double comptage).
    /// Avance `self.cycles` et renvoie le total réellement consommé.
    fn finalize_cycles<B: Bus>(&mut self, timed: &crate::bus::TimedBus<B>, table_cost: u32) -> u32 {
        let bus_cycles = timed.pos - self.cycles;
        let internal = (table_cost as u64).saturating_sub(4 * timed.access_count as u64);
        let total = bus_cycles + internal;
        self.cycles = self.cycles.wrapping_add(total);
        total as u32
    }

    fn execute(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        match opcode >> 12 {
            0b0000 => self.op_line_0(bus, opcode), // ORI, ANDI, SUBI, ADDI, EORI, CMPI, BTST/BCHG/BCLR/BSET
            0b0001..=0b0011 => self.op_move(bus, opcode),
            0b0100 => self.op_line_4(bus, opcode),
            0b0101 => self.op_line_5(bus, opcode), // ADDQ, SUBQ, Scc, DBcc
            0b0110 => self.op_branch(bus, opcode),
            0b0111 => self.op_moveq(opcode),
            0b1000 => self.op_line_8(bus, opcode), // DIVU, DIVS, OR
            0b1001 => self.op_sub(bus, opcode),
            0b1011 => self.op_line_b(bus, opcode), // CMP, CMPA, CMPM, EOR
            0b1100 => self.op_line_c(bus, opcode), // MULU, MULS, AND, EXG, ABCD
            0b1101 => self.op_add(bus, opcode),
            0b1110 => self.op_line_e(bus, opcode), // ASL/ASR/LSL/LSR/ROL/ROR/ROXL/ROXR
            0b1010 => {
                // Line A : exception vecteur 10
                let pc_push = self.pc.wrapping_sub(2); // opcode_addr (pour replay)
                self.take_exception(bus, 10, pc_push);
                Ok(34)
            }
            0b1111 => {
                // Line F : exception vecteur 11
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 11, pc_push);
                Ok(34)
            }
            _ => Err(StepError::Unimplemented(opcode)),
        }
    }

    // =========================================================================
    // Ligne 0000 : opérations immédiates + manipulation de bits
    // =========================================================================

    fn op_line_0(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;

        // MOVEP : 0000 ddd 1 0z 001 rrr (bit 8=1, mode=001, bit7=dir, bit6=size)
        if opcode & 0x0100 != 0 && mode == 0b001 {
            return self.op_movep(bus, opcode);
        }

        // BTST/BCHG/BCLR/BSET Dn,<ea> : 0000 rrr 1 tt mmm rrr
        // Yacht.txt lignes 226-312 : coût mem = base+ea_extra ; coût registre
        // dépend du numéro de bit (< 16 ou >= 16) et diffère pour BCLR.
        if opcode & 0x0100 != 0 && (opcode >> 8) & 0b111 != 0b100 {
            let bit_reg = ((opcode >> 9) & 0b111) as usize;
            let op = (opcode >> 6) & 0b11;
            let is_mem = mode != 0b000;
            let sz = if is_mem { Size::Byte } else { Size::Long };
            let ea = self
                .resolve_ea(bus, mode, reg, sz)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, sz))?;
            let modulus = if is_mem { 8 } else { 32 };
            let bit = self.d[bit_reg] as u32 % modulus;
            let mask = 1u32 << bit;
            self.set_flag(ccr::Z, val & mask == 0);
            let result = match op {
                0b00 => {
                    // BTST — pas d'écriture. Cas #imm comme cible (rare mais
                    // valide : BTST Dn,#imm lit l'immédiat comme un octet à
                    // tester) : +2 par rapport à la table EA générique —
                    // calibré et vérifié exact contre ProcessorTests.
                    let imm_bonus = if mode == 0b111 && reg == 0b100 { 2 } else { 0 };
                    return Ok(if is_mem {
                        4 + ea_extra + imm_bonus
                    } else {
                        6
                    });
                }
                0b01 => val ^ mask,  // BCHG
                0b10 => val & !mask, // BCLR
                0b11 => val | mask,  // BSET
                _ => unreachable!(),
            };
            ae_write(ea.write(self, bus, sz, result))?;
            let reg_cost = if op == 0b10 {
                // BCLR
                if bit < 16 { 8 } else { 10 }
            } else {
                // BCHG / BSET
                if bit < 16 { 6 } else { 8 }
            };
            return Ok(if is_mem { 8 + ea_extra } else { reg_cost });
        }

        // BTST/BCHG/BCLR/BSET #imm,<ea> : 0000 1000 xx mmm rrr (bits 11-9 = 100)
        if (opcode >> 9) & 0b111 == 0b100 {
            let op = (opcode >> 6) & 0b11;
            let bit_num = self.fetch_word(bus) as u32 & 0xFF;
            let is_mem = mode != 0b000;
            let sz = if is_mem { Size::Byte } else { Size::Long };
            let ea = self
                .resolve_ea(bus, mode, reg, sz)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, sz))?;
            let modulus = if is_mem { 8 } else { 32 };
            let bit = bit_num % modulus;
            let mask = 1u32 << bit;
            self.set_flag(ccr::Z, val & mask == 0);
            let result = match op {
                0b00 => {
                    // BTST
                    return Ok(if is_mem { 8 + ea_extra } else { 10 });
                }
                0b01 => val ^ mask,  // BCHG
                0b10 => val & !mask, // BCLR
                0b11 => val | mask,  // BSET
                _ => unreachable!(),
            };
            ae_write(ea.write(self, bus, sz, result))?;
            let reg_cost = if op == 0b10 {
                // BCLR
                if bit < 16 { 12 } else { 14 }
            } else {
                // BCHG / BSET
                if bit < 16 { 10 } else { 12 }
            };
            return Ok(if is_mem { 12 + ea_extra } else { reg_cost });
        }

        // ORI/ANDI/EORI to CCR/SR : opcodes fixes
        const CCR_VALID: u16 = 0x001F;
        match opcode {
            0x003C => {
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr | (imm & CCR_VALID)) & 0xA71F);
                return Ok(20);
            } // ORI to CCR
            0x007C => {
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr | imm) & 0xA71F);
                return Ok(20);
            } // ORI to SR
            0x023C => {
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr & ((imm & CCR_VALID) | !CCR_VALID)) & 0xA71F);
                return Ok(20);
            } // ANDI to CCR
            0x027C => {
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr & imm) & 0xA71F);
                return Ok(20);
            } // ANDI to SR
            0x0A3C => {
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr ^ (imm & CCR_VALID)) & 0xA71F);
                return Ok(20);
            } // EORI to CCR
            0x0A7C => {
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                let imm = self.fetch_word(bus);
                self.write_sr((self.sr ^ imm) & 0xA71F);
                return Ok(20);
            } // EORI to SR
            _ => {}
        }

        // ORI/ANDI/SUBI/ADDI/EORI/CMPI
        let op = (opcode >> 9) & 0b111;
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let imm = match size {
            Size::Byte => self.fetch_word(bus) as u8 as u32,
            Size::Word => self.fetch_word(bus) as u32,
            Size::Long => self.fetch_long(bus),
        };

        let ea = self
            .resolve_ea(bus, mode, reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let is_dn = mode == 0b000;
        // Tous les opcodes de ce groupe (y compris CMPI, lecture seule) fetchent
        // un immédiat avant de résoudre l'EA : le fault a donc le même préfixe
        // fixe de 8 (fetch opcode + fetch immédiat), quelle que soit la taille,
        // qu'il y ait réécriture ou non. Confirmé via ProcessorTests : CMP.l
        // "#,-(An)" (CMPI.l groupé dans le fichier CMP.l) suit "8+ea_extra+50",
        // comme EORI.w/EORI.l suivent tous les deux la même formule.
        self.fault_prefix = 8;
        let val = ae_read(ea.read(self, bus, size))?;
        let long = size == Size::Long;

        match op {
            0b000 | 0b001 | 0b010 | 0b011 | 0b101 => {
                // ORI/ANDI/SUBI/ADDI/EORI
                let r = match op {
                    0b000 => {
                        let r = val | imm;
                        self.set_logic_flags(r, size);
                        r
                    }
                    0b001 => {
                        let r = val & imm;
                        self.set_logic_flags(r, size);
                        r
                    }
                    0b010 => self.sub_with_flags(val, imm, size),
                    0b011 => self.add_with_flags(val, imm, size),
                    0b101 => {
                        let r = val ^ imm;
                        self.set_logic_flags(r, size);
                        r
                    }
                    _ => unreachable!(),
                };
                ae_write(ea.write(self, bus, size, r))?;
                Ok(if long {
                    if is_dn { 16 } else { 20 + ea_extra }
                } else if is_dn {
                    8
                } else {
                    12 + ea_extra
                })
            }
            0b110 => {
                // CMPI — lecture seule
                self.cmp_flags(val, imm, size);
                Ok(if long {
                    if is_dn { 14 } else { 12 + ea_extra }
                } else {
                    8 + ea_extra // uniforme, y compris Dn (ea_extra=0 alors)
                })
            }
            _ => Err(StepError::Unimplemented(opcode)),
        }
    }

    // =========================================================================
    // MOVE / MOVEA
    // =========================================================================

    // ABCD : addition BCD avec X
    fn op_abcd(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dst_reg = ((opcode >> 9) & 0b111) as usize;
        let src_reg = (opcode & 0b111) as usize;
        let mem_mode = opcode & 0x0008 != 0;
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };

        let (src_val, dst_val, dst_op) = if mem_mode {
            // A7 reste aligné sur un mot : décrément de 2 même en octet.
            let s_step = if src_reg == 7 { 2 } else { 1 };
            self.a[src_reg] = self.a[src_reg].wrapping_sub(s_step);
            let src_addr = self.a[src_reg] & ADDR_MASK;
            let d_step = if dst_reg == 7 { 2 } else { 1 };
            self.a[dst_reg] = self.a[dst_reg].wrapping_sub(d_step);
            let dst_addr = self.a[dst_reg] & ADDR_MASK;
            let s = bus.read8(src_addr) as u32;
            let d = bus.read8(dst_addr) as u32;
            (s, d, Operand::Memory(dst_addr))
        } else {
            (
                self.d[src_reg] & 0xFF,
                self.d[dst_reg] & 0xFF,
                Operand::DataReg(dst_reg),
            )
        };

        // Algorithme BCD 68000 (comportement MAME)
        let src = src_val;
        let dst = dst_val;

        // Nibble bas : correction si somme > 9
        let lo = (src & 0x0F) + (dst & 0x0F) + x;
        let lo_correct = if lo > 9 { 6u32 } else { 0 };

        // Nibble haut : avec carry du nibble bas
        let hi = (src >> 4) + (dst >> 4) + (lo + lo_correct) / 16;
        let hi_correct = if hi > 9 { 6u32 } else { 0 };

        // Résultat complet
        let raw_sum = src + dst + x;
        let corrected = raw_sum + lo_correct + (hi_correct << 4);
        let result = corrected & 0xFF;
        let carry = corrected >= 0x100;

        self.set_flag(ccr::C, carry);
        self.set_flag(ccr::X, carry);
        // V (silicium 68000) : bit 7 passant de 0 à 1 lors de la correction décimale.
        let raw_byte = raw_sum & 0xFF;
        self.set_flag(ccr::V, (!raw_byte & result) & 0x80 != 0);
        if result != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::N, result & 0x80 != 0);

        ae_write(dst_op.write(self, bus, Size::Byte, result))?;
        Ok(if mem_mode { 18 } else { 6 })
    }

    // SBCD : soustraction BCD avec X
    fn op_sbcd(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dst_reg = ((opcode >> 9) & 0b111) as usize;
        let src_reg = (opcode & 0b111) as usize;
        let mem_mode = opcode & 0x0008 != 0;
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };

        let (src_val, dst_val, dst_op) = if mem_mode {
            // A7 reste aligné sur un mot : décrément de 2 même en octet.
            let s_step = if src_reg == 7 { 2 } else { 1 };
            self.a[src_reg] = self.a[src_reg].wrapping_sub(s_step);
            let src_addr = self.a[src_reg] & ADDR_MASK;
            let d_step = if dst_reg == 7 { 2 } else { 1 };
            self.a[dst_reg] = self.a[dst_reg].wrapping_sub(d_step);
            let dst_addr = self.a[dst_reg] & ADDR_MASK;
            let s = bus.read8(src_addr) as u32;
            let d = bus.read8(dst_addr) as u32;
            (s, d, Operand::Memory(dst_addr))
        } else {
            (
                self.d[src_reg] & 0xFF,
                self.d[dst_reg] & 0xFF,
                Operand::DataReg(dst_reg),
            )
        };

        // Soustraction BCD : dst - src - X
        let dst = dst_val as i32;
        let src = src_val as i32;
        let xi = x as i32;

        let lo = (dst & 0x0F) - (src & 0x0F) - xi;
        let lo_borrow = lo < 0;
        let lo_correct = if lo_borrow { 6i32 } else { 0 };

        let hi = (dst >> 4) - (src >> 4) - (if lo_borrow { 1 } else { 0 });
        let hi_borrow = hi < 0;
        let hi_correct = if hi_borrow { 6i32 } else { 0 };

        let raw = (dst - src - xi + 0x100) & 0xFF;
        let corrected = (raw - lo_correct - (hi_correct << 4)) & 0xFF;
        let result = corrected as u32;
        let actual_borrow = hi_borrow;

        self.set_flag(ccr::C, actual_borrow);
        self.set_flag(ccr::X, actual_borrow);
        // V (silicium 68000) : bit 7 passant de 1 à 0 lors de la correction décimale.
        self.set_flag(ccr::V, (raw & !corrected) & 0x80 != 0);
        if result != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::N, result & 0x80 != 0);
        ae_write(dst_op.write(self, bus, Size::Byte, result))?;
        Ok(if mem_mode { 18 } else { 6 })
    }

    // NBCD : negate BCD (0 - dst - X)
    fn op_nbcd(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Byte)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let is_dn = mode == 0b000;
        let dst = ae_read(ea.read(self, bus, Size::Byte))?;

        // NBCD = 0 - dst - X (BCD)
        let d = dst as i32;
        let xi = x as i32;

        let lo = -(d & 0x0F) - xi;
        let lo_borrow = lo < 0;
        let lo_correct = if lo_borrow { 6i32 } else { 0 };

        let hi = -(d >> 4) - (if lo_borrow { 1 } else { 0 });
        let hi_borrow = hi < 0;
        let hi_correct = if hi_borrow { 6i32 } else { 0 };

        let raw = (0 - d - xi + 0x100) & 0xFF;
        let corrected = (raw - lo_correct - (hi_correct << 4)) & 0xFF;
        let result = corrected as u32;
        let actual_borrow = hi_borrow;

        self.set_flag(ccr::C, actual_borrow);
        self.set_flag(ccr::X, actual_borrow);
        // V (silicium 68000) : bit 7 passant de 1 à 0 lors de la correction décimale.
        self.set_flag(ccr::V, (raw & !corrected) & 0x80 != 0);
        if result != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::N, result & 0x80 != 0);

        ae_write(ea.write(self, bus, Size::Byte, result))?;
        Ok(if is_dn { 6 } else { 8 + ea_extra })
    }

    // MOVEP : 0000 ddd 1 0z 001 rrr  (ligne 0, dispatché avant MOVE)
    // Encodage dans op_line_0 pour z=0 (word) et z=1 (long), d/r depuis registres
    fn op_movep(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dreg = ((opcode >> 9) & 0b111) as usize;
        let areg = (opcode & 0b111) as usize;
        let to_mem = opcode & 0x0080 != 0; // bit 7 : 0=mem→Dn, 1=Dn→mem
        let long = opcode & 0x0040 != 0; // bit 6 : 0=word, 1=long
        let disp = self.fetch_word(bus) as i16 as i32;
        let base = (self.a[areg] as i32).wrapping_add(disp) as u32 & ADDR_MASK;

        if to_mem {
            if long {
                bus.write8(base, (self.d[dreg] >> 24) as u8);
                bus.write8(base + 2, (self.d[dreg] >> 16) as u8);
                bus.write8(base + 4, (self.d[dreg] >> 8) as u8);
                bus.write8(base + 6, self.d[dreg] as u8);
            } else {
                bus.write8(base, (self.d[dreg] >> 8) as u8);
                bus.write8(base + 2, self.d[dreg] as u8);
            }
        } else if long {
            let b0 = bus.read8(base) as u32;
            let b1 = bus.read8(base + 2) as u32;
            let b2 = bus.read8(base + 4) as u32;
            let b3 = bus.read8(base + 6) as u32;
            self.d[dreg] = (b0 << 24) | (b1 << 16) | (b2 << 8) | b3;
        } else {
            let b0 = bus.read8(base) as u32;
            let b1 = bus.read8(base + 2) as u32;
            let word = (b0 << 8) | b1;
            self.d[dreg] = (self.d[dreg] & 0xFFFF_0000) | word;
        }
        Ok(if long { 24 } else { 16 })
    }

    fn op_move(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let size = match opcode >> 12 {
            0b0001 => Size::Byte,
            0b0011 => Size::Word,
            0b0010 => Size::Long,
            _ => unreachable!(),
        };
        // PC après fetch de l'opcode (avant tout mot d'extension src/dst).
        // Utilisé pour calculer frame_pc des write AE MOVE.l.
        let pc_after_opcode = self.pc;
        let src_mode = (opcode >> 3) & 0b111;
        let src_reg = opcode & 0b111;
        let src = self
            .resolve_ea(bus, src_mode, src_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let value = ae_read(src.read(self, bus, size))?;
        // Capturer avant que le resolve_ea de la destination n'écrase ea_extra_cycles :
        // seul le coût de calcul d'EA de la SOURCE s'ajoute (le coût d'écriture dst
        // est couvert par move_dst_base, dérivé de Yacht.txt, pas du terme générique).
        let src_extra = self.ea_extra_cycles;

        let dst_reg = (opcode >> 9) & 0b111;
        let dst_mode = (opcode >> 6) & 0b111;
        let dst = self
            .resolve_ea(bus, dst_mode, dst_reg, size)
            .ok_or(StepError::IllegalAddressing)?;

        // Pour -(An) destination, ea_frame_pc a déjà intégré le prefetch du pré-décrément.
        // Operand::write ajoute +2 (prefetch avant write cycle), ce qui serait un double-comptage.
        // On annule ce +2 avant le write pour qu'il soit réajouté et atteigne la valeur correcte.
        if dst_mode == 0b100 {
            self.ea_frame_pc = self.ea_frame_pc.wrapping_sub(2);
        }

        let sr_before_flags = self.sr;
        if dst_mode != 0b001 {
            self.set_logic_flags(value, size);
        }

        // Pour MOVE.w/b avec dst -(An), le 68000 fait un Prefetch() avant le write cycle.
        // Ce Prefetch avance le pipeline : l'IR dans le frame AE write = mot suivant dans le flux,
        // pas l'opcode. Ce mot = bus.read16(self.pc) à ce point (après consommation des extensions).
        if dst_mode == 0b100 && size != Size::Long {
            self.write_ae_ir = Some(bus.read16(self.pc & crate::cpu::ADDR_MASK));
        }

        // MOVE.l -(An) : CLK écrit LSW en premier (à An-2), puis MSW (à An-4).
        // fault_addr = adresse 32 bits complète (avant masquage 24 bits).
        let write_result = if size == Size::Long && dst_mode == 0b100 {
            // Après resolve_ea, An a déjà été décrémenté de 4.
            // An_current = An_initial - 4 ; LSW addr (32 bits) = An_initial - 2 = An_current + 2
            let an_full = self.a[dst_reg as usize];
            let lsw_addr_full = an_full.wrapping_add(2);
            let msw_addr_full = an_full;
            let lsw_addr = lsw_addr_full & crate::cpu::ADDR_MASK;
            let msw_addr = msw_addr_full & crate::cpu::ADDR_MASK;
            let frame_pc_ae = self.ea_frame_pc.wrapping_add(2);
            if lsw_addr & 1 != 0 {
                Err((lsw_addr_full, frame_pc_ae))
            } else {
                bus.write16(lsw_addr, value as u16);
                if msw_addr & 1 != 0 {
                    Err((msw_addr_full, frame_pc_ae))
                } else {
                    bus.write16(msw_addr, (value >> 16) as u16);
                    Ok(())
                }
            }
        } else {
            dst.write(self, bus, size, value)
        };
        if let Err((fault_addr, frame_pc_from_ea)) = write_result {
            // Write AE : les CCR dans le frame sauvegardé dépendent du dst_mode et de la taille.
            // Analyse TomHarte (267 cas Dn/An src, 0 fail) :
            //   MOVE.w : mask=0x10 (X seul préservé) | N<<3 | Z<<2 — universel pour tous modes
            //   MOVE.l : mask varie par dst_mode :
            //     dm=0,2,3 (Dn, (An), (An)+) → SR inchangé
            //     dm=4,7   (-(An), abs)       → mask=0x10 | NZ
            //     dm=5,6   ((d16,An),(d8,Xn)) → mask=0x13 (X+V+C) | NZ
            let sr_for_frame = if dst_mode == 0b001 {
                // MOVEA : aucune mise à jour de flags, SR inchangé toujours
                sr_before_flags
            } else if size == Size::Long {
                // imm (sm=7,sr=4) se comporte comme Dn/An : déjà dans le pipeline
                let src_is_reg = src_mode <= 0b001 || (src_mode == 0b111 && src_reg == 0b100);
                // Pour dm=2,3 src=mémoire, ou sm=3+(An)+ src avec dst=abs.l : flags sur LSW.
                // Autrement : flags sur valeur 32 bits complète.
                let is_dst_abs_long = dst_mode == 0b111 && dst_reg == 0b001;
                let use_lsw = !src_is_reg
                    && (dst_mode == 0b010
                        || dst_mode == 0b011
                        || (src_mode == 0b011 && is_dst_abs_long));
                let (n, z) = if use_lsw {
                    let lsw = value & 0xFFFF;
                    ((lsw >> 15) & 1, if lsw == 0 { 1u32 } else { 0 })
                } else {
                    ((value >> 31) & 1, if value == 0 { 1u32 } else { 0 })
                };
                match dst_mode {
                    // (An) et (An)+ : unchanged si src=Dn/An, X+NZ(LSW) si src=mémoire
                    0b010 | 0b011 => {
                        if src_is_reg {
                            sr_before_flags
                        } else {
                            (sr_before_flags & 0xFFE0)
                                | (sr_before_flags & 0x0010)
                                | ((n as u16) << 3)
                                | ((z as u16) << 2)
                        }
                    }
                    // (d16,An), (d8,Xn) : X+V+C si src=Dn/An, X seul si src=mémoire
                    0b101 | 0b110 => {
                        let preserve = if src_is_reg { 0x0013u16 } else { 0x0010u16 };
                        (sr_before_flags & 0xFFE0)
                            | (sr_before_flags & preserve)
                            | ((n as u16) << 3)
                            | ((z as u16) << 2)
                    }
                    // -(An), abs → X seul préservé (toutes sources)
                    _ => {
                        (sr_before_flags & 0xFFE0)
                            | (sr_before_flags & 0x0010)
                            | ((n as u16) << 3)
                            | ((z as u16) << 2)
                    }
                }
            } else {
                // MOVE.b et MOVE.w : X seul préservé pour tous les modes
                let msb = size.msb();
                let n: u16 = if value & msb != 0 { 1 } else { 0 };
                let z: u16 = if value & size.mask() == 0 { 1 } else { 0 };
                (sr_before_flags & 0xFFE0) | (sr_before_flags & 0x0010) | (n << 3) | (z << 2)
            };

            // Pour (An)+ MOVE.w/b : le post-incrément doit être annulé en cas d'AE write.
            // Pour MOVE.l à addr impaire, le post-inc n'est jamais committé (voir resolve_ea mode 3).
            if dst_mode == 0b011 && size != Size::Long {
                let step = if dst_reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[dst_reg as usize] = self.a[dst_reg as usize].wrapping_sub(step);
            }
            // Pour -(An) MOVE.l : annuler le pré-décrément dst en cas d'AE write.
            // Fait avant take_address_error_full car si dst=A7, le rollback ajuste la base de SP.
            if dst_mode == 0b100 && size == Size::Long {
                self.a[dst_reg as usize] = self.a[dst_reg as usize].wrapping_add(size.bytes());
            }

            // Pour MOVE write AE, frame_pc dépend du nb d'extensions SRC (cycle bus pipeline).
            // Règle : frame_pc = pc_after_opcode + 2 + 2 × nb_ext_src_words
            //   nb_ext_src = 0 si src mode sans extension (Dn/An/indirect sans disp)
            //               = 1 si src a 1 ext word (d16,An / d8,An,Xn / abs.w / PC-rel / imm b/w)
            //               = 2 si src a 2 ext words (abs.l / imm.l)
            // Exception: src=Dn/An (mode 0,1) avec dst=abs.l (dm=7, dr=1) → +2 supplémentaire
            // Même règle pour MOVE.l, MOVE.w, MOVE.b
            let nb_src_ext: u32 = match src_mode {
                0b101 | 0b110 => 1, // d16,An ou d8,An,Xn
                0b111 => match src_reg {
                    0b000 | 0b010 | 0b011 => 1, // abs.w, d16,PC, d8,PC,Xn
                    0b001 => 2,                 // abs.l
                    0b100 => {
                        if size == Size::Long {
                            2
                        } else {
                            1
                        }
                    } // imm.l = 2, imm.b/w = 1
                    _ => 0,
                },
                _ => 0, // Dn, An, (An), (An)+, -(An)
            };
            let is_src_reg = src_mode <= 0b001;
            let is_dst_abs_long = dst_mode == 0b111 && dst_reg == 0b001;
            let extra: u32 = if is_src_reg && is_dst_abs_long { 2 } else { 0 };
            let frame_pc = pc_after_opcode.wrapping_add(2 + nb_src_ext * 2 + extra);

            self.sr = sr_for_frame;
            self.take_address_error_full(bus, fault_addr, true, Some(frame_pc), false);
            return Ok(50);
        }
        Ok(src_extra + move_dst_base(dst_mode, dst_reg, size))
    }

    // =========================================================================
    // Ligne 0100 : miscellaneous
    // =========================================================================

    fn op_line_4(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;

        match opcode {
            0x4E71 => return Ok(4), // NOP
            0x4E70 => {
                // RESET (privilégié) — génère /RESET sur le bus (124 cycles)
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                bus.reset_bus();
                return Ok(132);
            }
            0x4E75 => return self.op_rts(bus),
            0x4E73 => {
                // RTE (privilégié)
                if let Some(c) = self.check_privilege(bus) {
                    return Ok(c);
                }
                return self.op_rte(bus);
            }
            0x4E77 => return self.op_rtr(bus),
            0x4AFC => {
                // ILLEGAL : exception vecteur 4, empile opcode_addr
                let pc_push = self.pc.wrapping_sub(2);
                self.take_exception(bus, 4, pc_push);
                return Ok(34);
            }
            0x4E76 => {
                // TRAPV : exception vecteur 7 si V=1
                if self.flag(ccr::V) {
                    let pc_push = self.pc; // opcode_addr + 2
                    self.take_exception(bus, 7, pc_push);
                    return Ok(34);
                }
                return Ok(4);
            }
            _ => {}
        }

        // TRAP : 0100 1110 0100 vvvv (vecteur 32+v)
        if opcode & 0xFFF0 == 0x4E40 {
            let v = (opcode & 0xF) as u32;
            let pc_push = self.pc; // PC après fetch opcode = opcode_addr + 2
            self.take_exception(bus, 32 + v, pc_push);
            return Ok(34);
        }
        // LINK : 0100 1110 0101 0rrr
        if opcode & 0xFFF8 == 0x4E50 {
            return self.op_link(bus, reg as usize);
        }
        // UNLK : 0100 1110 0101 1rrr
        if opcode & 0xFFF8 == 0x4E58 {
            return self.op_unlk(bus, reg as usize);
        }
        // MOVE to USP : 0100 1110 0110 0rrr (privilégié)
        if opcode & 0xFFF8 == 0x4E60 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            self.usp = self.a[reg as usize];
            return Ok(4);
        }
        // MOVE from USP : 0100 1110 0110 1rrr (privilégié)
        if opcode & 0xFFF8 == 0x4E68 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            self.a[reg as usize] = self.usp;
            return Ok(4);
        }
        // STOP : 0100 1110 0111 0010 (privilégié)
        if opcode == 0x4E72 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            let new_sr = self.fetch_word(bus);
            self.pc = self.pc.wrapping_sub(4);
            self.write_sr(new_sr & 0xA71F);
            return Ok(4);
        }
        // JSR : 0100 1110 10 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1110_1000_0000 {
            return self.op_jsr(bus, mode, reg);
        }
        // JMP : 0100 1110 11 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1110_1100_0000 {
            return self.op_jmp(bus, mode, reg);
        }
        // LEA : 0100 aaa 111 mmm rrr
        if opcode & 0b1111_0001_1100_0000 == 0b0100_0001_1100_0000 {
            return self.op_lea(bus, opcode);
        }
        // SWAP : 0100 1000 0100 0rrr — doit précéder PEA (même masque 0xffc0)
        if opcode & 0b1111_1111_1111_1000 == 0b0100_1000_0100_0000 {
            let r = reg as usize;
            self.d[r] = (self.d[r] >> 16) | (self.d[r] << 16);
            self.set_logic_flags(self.d[r], Size::Long);
            return Ok(4);
        }
        // PEA : 0100 1000 01 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1000_0100_0000 {
            return self.op_pea(bus, mode, reg);
        }
        // CHK : 0100 rrr 110 mmm rrr
        if opcode & 0b1111_0001_1100_0000 == 0b0100_0001_1000_0000 {
            let dn_reg = ((opcode >> 9) & 0b111) as usize;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let upper = ae_read(ea.read(self, bus, Size::Word))? as i16;
            let dn = (self.d[dn_reg] & 0xFFFF) as i16;
            if dn < 0 {
                // Effacer N,V,Z,C puis set N
                self.sr &= !0x000F;
                self.set_flag(ccr::N, true);
                let pc_push = self.pc; // opcode_addr + 2 (après fetch opcode + EA)
                self.take_exception(bus, 6, pc_push);
                // NOTE : deux cas "dn<0" structurellement identiques (registre
                // direct, ea_extra=0) attendent 38 ET 40 selon ProcessorTests —
                // CHK a une dépendance aux données/pipeline non résolue ici
                // (comparable à la remarque MAME sur TRAPV). Ne pas retoucher
                // sans nouvelle piste : un essai de "toujours 38" a empiré le
                // résultat (388 → 402 échecs cycles).
                return Ok(40 + ea_extra);
            } else if dn > upper {
                self.sr &= !0x000F;
                let pc_push = self.pc;
                self.take_exception(bus, 6, pc_push);
                return Ok(38 + ea_extra);
            }
            // Dans la plage : effacer N,V,Z,C (comportement MAME)
            self.sr &= !0x000F;
            return Ok(10 + ea_extra);
        }
        // EXT : 0100 1000 1 sz 000 rrr — doit précéder MOVEM (même zone de bits)
        if opcode & 0b1111_1111_1011_1000 == 0b0100_1000_1000_0000 {
            let to_long = opcode & 0x0040 != 0;
            let r = reg as usize;
            if to_long {
                self.d[r] = self.d[r] as u16 as i16 as i32 as u32;
                self.set_logic_flags(self.d[r], Size::Long);
            } else {
                let word = (self.d[r] as u8 as i8 as i16 as u16) as u32;
                self.d[r] = (self.d[r] & 0xFFFF_0000) | word;
                self.set_logic_flags(word, Size::Word);
            }
            return Ok(4);
        }
        // MOVEM : 0100 1 d00 1sz mmm rrr  (d=0: regs→mem, d=1: mem→regs)
        if opcode & 0b1111_1011_1000_0000 == 0b0100_1000_1000_0000 {
            return self.op_movem(bus, opcode);
        }
        // MOVE from SR : 0100 0000 11 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0000_1100_0000 {
            let sr = self.sr;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            // Dummy read avant write (RMW du 68000) : AE rapportée comme read error.
            ae_read(ea.read(self, bus, Size::Word))?;
            ae_write(ea.write(self, bus, Size::Word, sr as u32))?;
            return Ok(if is_dn { 6 } else { 8 + ea_extra });
        }
        // MOVE to CCR : 0100 0100 11 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0100_1100_0000 {
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, Size::Word))?;
            self.write_sr(((self.sr & 0xFF00) | (val as u16 & 0x001F)) & 0xA71F);
            return Ok(12 + ea_extra);
        }
        // MOVE to SR : 0100 0110 11 mmm rrr (privilégié)
        if opcode & 0b1111_1111_1100_0000 == 0b0100_0110_1100_0000 {
            if let Some(c) = self.check_privilege(bus) {
                return Ok(c);
            }
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, Size::Word))?;
            self.write_sr(val as u16 & 0xA71F);
            return Ok(12 + ea_extra);
        }
        // CLR : 0100 0010 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0010_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let dst = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            // Le 68000 effectue un dummy read avant l'écriture (RMW) :
            // une adresse impaire déclenche un *read* error, pas un write error.
            ae_read(dst.read(self, bus, size))?;
            ae_write(dst.write(self, bus, size, 0))?;
            self.set_flag(ccr::N, false);
            self.set_flag(ccr::Z, true);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // TAS : 0100 1010 11 mmm rrr — doit précéder TST (même octet haut)
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1010_1100_0000 {
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Byte)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, Size::Byte))?;
            self.set_flag(ccr::N, val & 0x80 != 0);
            self.set_flag(ccr::Z, val == 0);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            ae_write(ea.write(self, bus, Size::Byte, val | 0x80))?;
            return Ok(if is_dn { 4 } else { 10 + ea_extra });
        }
        // TST : 0100 1010 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_1010_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, size))?;
            self.set_logic_flags(val, size);
            return Ok(4 + ea_extra);
        }
        // NEG : 0100 0100 SS mmm rrr  (attention : bits 10-8 = 010)
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0100_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, size))?;
            let result = self.sub_with_flags(0, val, size);
            // X = C pour NEG
            let c = self.flag(ccr::C);
            self.set_flag(ccr::X, c);
            ae_write(ea.write(self, bus, size, result))?;
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // NEGX : 0100 0000 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0000_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, size))?;
            let x = if self.flag(ccr::X) { 1u32 } else { 0 };
            let result = self.subx_with_flags(0, val, x, size);
            ae_write(ea.write(self, bus, size, result))?;
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // NOT : 0100 0110 SS mmm rrr
        if opcode & 0b1111_1111_0000_0000 == 0b0100_0110_0000_0000 {
            let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let is_dn = mode == 0b000;
            let val = ae_read(ea.read(self, bus, size))?;
            let result = !val & size.mask();
            ae_write(ea.write(self, bus, size, result))?;
            self.set_logic_flags(result, size);
            return Ok(rmw_cost(is_dn, size, ea_extra));
        }
        // NBCD : 0100 1000 00 mmm rrr
        if opcode & 0b1111_1111_1100_0000 == 0b0100_1000_0000_0000 {
            return self.op_nbcd(bus, mode, reg);
        }

        Err(StepError::Unimplemented(opcode))
    }

    fn op_jsr(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        let addr = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        // Cible impaire : address error au fetch de la cible (FC = programme),
        // AVANT le push. Le SP n'est pas décrémenté ; le PC du frame est self.pc.
        // On passe par pending_address_error pour que le frame porte FC programme.
        // Coût partiel avant le fault = jsr_cost(mode,reg) moins le push jamais
        // exécuté (2 mots = 8 cycles) — calibré exact contre ProcessorTests :
        // (An)=16-8=8, (d16,An)=18-8=10, (d8,An,Xn)=22-8=14.
        if addr & 1 != 0 {
            self.pending_address_error = Some((addr, false, self.pc));
            return Ok(jsr_cost(mode, reg).saturating_sub(8));
        }
        let ret = self.pc;
        self.set_sp(self.sp().wrapping_sub(4));
        bus.write32(self.sp() & ADDR_MASK, ret);
        self.pc = addr;
        Ok(jsr_cost(mode, reg))
    }

    fn op_jmp(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        let addr = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        self.pc = addr;
        Ok(jmp_cost(mode, reg))
    }

    fn op_rts(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let addr = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        self.pc = addr;
        Ok(16)
    }

    fn op_rte(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let new_sr = bus.read16(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(2));
        let new_pc = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        self.write_sr(new_sr & 0xA71F);
        self.pc = new_pc;
        Ok(20)
    }

    fn op_rtr(&mut self, bus: &mut impl Bus) -> Result<u32, StepError> {
        let new_ccr = bus.read16(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(2));
        let new_pc = bus.read32(self.sp() & ADDR_MASK);
        self.set_sp(self.sp().wrapping_add(4));
        self.sr = ((self.sr & 0xFF00) | (new_ccr & 0x001F)) & 0xA71F;
        self.pc = new_pc;
        Ok(20)
    }

    fn op_link(&mut self, bus: &mut impl Bus, reg: usize) -> Result<u32, StepError> {
        let disp = self.fetch_word(bus) as i16 as i32;
        let saved_an = self.a[reg]; // sauvegarder An avant toute modification (cas LINK A7)
        self.set_sp(self.sp().wrapping_sub(4));
        bus.write32(self.sp() & ADDR_MASK, saved_an);
        self.a[reg] = self.sp();
        self.set_sp((self.sp() as i32).wrapping_add(disp) as u32);
        Ok(16)
    }

    fn op_unlk(&mut self, bus: &mut impl Bus, reg: usize) -> Result<u32, StepError> {
        let frame = self.a[reg];
        // UNLK effectue un prefetch supplémentaire avant l'accès pile :
        // le PC du frame d'exception est avancé de 2 (our_pc+4 au lieu de +2).
        //
        // Pas de resolve_ea ici (accès direct via le registre) : ea_extra_cycles
        // resterait sinon à la valeur périmée de l'instruction précédente. Coût
        // d'address error calibré contre ProcessorTests : opcode(4) + prefetch
        // supplémentaire(4) + 50 = 58 pour "UNLINK An", confirmé exact.
        if frame & 1 != 0 {
            self.ea_extra_cycles = 0;
            self.fault_prefix = 8;
            return Err(StepError::AddressError(
                frame,
                false,
                self.pc.wrapping_add(2),
            ));
        }
        let saved = ae_read(Operand::Memory(frame).read(self, bus, Size::Long))?;
        self.set_sp(frame.wrapping_add(4));
        self.a[reg] = saved;
        Ok(12)
    }

    fn op_pea(&mut self, bus: &mut impl Bus, mode: u16, reg: u16) -> Result<u32, StepError> {
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        let addr = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        self.set_sp(self.sp().wrapping_sub(4));
        bus.write32(self.sp() & ADDR_MASK, addr);
        Ok(pea_lea_cost(mode, reg, true))
    }

    fn op_lea(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let areg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;
        let ea = self
            .resolve_ea(bus, mode, reg, Size::Long)
            .ok_or(StepError::IllegalAddressing)?;
        self.a[areg] = match ea {
            Operand::Memory(a) => a,
            _ => return Err(StepError::IllegalAddressing),
        };
        Ok(pea_lea_cost(mode, reg, false))
    }

    fn op_movem(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let to_regs = opcode & 0x0400 != 0; // bit 10 : 0=regs→mem, 1=mem→regs
        let size = if opcode & 0x0040 != 0 {
            Size::Long
        } else {
            Size::Word
        };
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;
        let mask = self.fetch_word(bus);

        if to_regs {
            // Mémoire → registres (mode post-incrément possible)
            // Sauver A[reg] : resolve_ea applique un post-incrément pour (An)+ ;
            // en cas d'address error, l'instruction est avortée et le registre
            // doit rester inchangé.
            let saved_areg = self.a[reg as usize];
            let ea = self
                .resolve_ea(bus, mode, reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            // MOVEM fetches an extra register-mask word before resolving the
            // EA; resolve_ea's size-based default (Long=0/Word=4) doesn't
            // know about it. +4 confirmed exact against ProcessorTests for
            // (An)/(An)+: Long=4+ea_extra+50, Word=8+ea_extra+50.
            self.fault_prefix += 4;
            let base = match ea {
                Operand::Memory(a) => a,
                _ => return Err(StepError::IllegalAddressing),
            };
            let frame_pc = self.pc.wrapping_add(2);
            // MOVEM accède la mémoire mot par mot. Une adresse impaire déclenche
            // une address error rapportée sur l'adresse de base (PAS base+2 comme
            // un accès long unique). On vérifie donc l'alignement de la base.
            if base & 1 != 0 {
                self.a[reg as usize] = saved_areg;
                return Err(StepError::AddressError(base, false, frame_pc));
            }
            let mut addr = base;
            let mut new_d = self.d;
            let mut new_a = self.a;
            for i in 0..16usize {
                if mask & (1 << i) != 0 {
                    let val = if size == Size::Long {
                        bus.read32(addr & ADDR_MASK)
                    } else {
                        bus.read16(addr & ADDR_MASK) as i16 as i32 as u32
                    };
                    if i < 8 {
                        new_d[i] = val;
                    } else {
                        new_a[i - 8] = val;
                    }
                    addr = addr.wrapping_add(size.bytes());
                }
            }
            self.d = new_d;
            self.a = new_a;
            // Pour (An)+, mettre à jour le registre d'adresse
            if mode == 0b011 {
                self.a[reg as usize] = addr;
            }
        } else {
            // Registres → mémoire
            let predec = mode == 0b100;
            let frame_pc = self.pc.wrapping_add(2);

            if predec {
                // Pour -(An), on gère le décrément manuellement sans passer par resolve_ea
                // (resolve_ea ferait un premier décrément parasite).
                // Masque inversé : bit 0 = A7, bit 7 = A0, bit 8 = D7, bit 15 = D0.
                let mut addr = self.a[reg as usize];
                // L'AE survient sur la première adresse écrite. En long, le 68000
                // écrit d'abord le mot bas (à ea+2), donc l'adresse fautive est
                // (base - 4) + 2. En word, c'est (base - 2). EA non mise à jour.
                for i in 0..16usize {
                    if mask & (1 << i) != 0 {
                        addr = addr.wrapping_sub(size.bytes());
                        if addr & 1 != 0 {
                            let fault = if size == Size::Long {
                                addr.wrapping_add(2)
                            } else {
                                addr
                            };
                            // Décrément manuel, pas de resolve_ea : ea_extra_cycles
                            // resterait à une valeur périmée. Calibré contre
                            // ProcessorTests — coûts 2 cycles sous la table
                            // générique "-(An)" (10/6), car MOVEM ne paie pas deux
                            // fois l'idle de décrément déjà inclus par ailleurs.
                            self.ea_extra_cycles = if size == Size::Long { 8 } else { 4 };
                            self.fault_prefix = if size == Size::Long { 4 } else { 8 };
                            return Err(StepError::AddressError(fault, true, frame_pc));
                        }
                        break;
                    }
                }
                let mut addr = self.a[reg as usize];
                for i in 0..16usize {
                    if mask & (1 << i) != 0 {
                        let val = if i < 8 { self.a[7 - i] } else { self.d[15 - i] };
                        addr = addr.wrapping_sub(size.bytes());
                        if size == Size::Long {
                            bus.write32(addr & ADDR_MASK, val);
                        } else {
                            bus.write16(addr & ADDR_MASK, val as u16);
                        }
                    }
                }
                self.a[reg as usize] = addr;
            } else {
                let ea = self
                    .resolve_ea(bus, mode, reg, size)
                    .ok_or(StepError::IllegalAddressing)?;
                // Voir le commentaire équivalent côté mem→registres : +4 pour
                // le mot de masque déjà lu avant resolve_ea.
                self.fault_prefix += 4;
                let base = match ea {
                    Operand::Memory(a) => a,
                    _ => return Err(StepError::IllegalAddressing),
                };
                let frame_pc = self.pc.wrapping_add(2);
                if base & 1 != 0 {
                    return Err(StepError::AddressError(base, true, frame_pc));
                }
                let mut addr = base;
                for i in 0..16usize {
                    if mask & (1 << i) != 0 {
                        let val = if i < 8 { self.d[i] } else { self.a[i - 8] };
                        if size == Size::Long {
                            bus.write32(addr & ADDR_MASK, val);
                        } else {
                            bus.write16(addr & ADDR_MASK, val as u16);
                        }
                        addr = addr.wrapping_add(size.bytes());
                    }
                }
            }
        }
        let m = mask.count_ones();
        let per_reg = if size == Size::Long { 8 } else { 4 };
        Ok(movem_base(mode, reg, to_regs) + per_reg * m)
    }

    // =========================================================================
    // Ligne 0101 : ADDQ, SUBQ, Scc, DBcc
    // =========================================================================

    fn op_line_5(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let mode = (opcode >> 3) & 0b111;
        let reg = opcode & 0b111;

        // DBcc : 0101 cccc 1100 1rrr
        // Yacht.txt lignes 952-966 : 10 si la boucle continue (branche prise),
        // 12 si la condition devient vraie (sortie), 14 si le compteur expire (sortie).
        if opcode & 0b1111_0000_1111_1000 == 0b0101_0000_1100_1000 {
            let cc = (opcode >> 8) & 0b1111;
            let r = reg as usize;
            if !self.test_condition(cc) {
                let base = self.pc; // PC avant le mot de déplacement
                let disp = self.fetch_word(bus) as i16 as i32;
                // PC de l'instruction suivante (après le déplacement) = frame_pc en cas d'AE.
                let next_pc = self.pc;
                let word = (self.d[r] & 0xFFFF) as u16;
                let decremented = word.wrapping_sub(1);
                if decremented != 0xFFFF {
                    let target = (base as i32).wrapping_add(disp) as u32;
                    if target & 1 != 0 {
                        // Cible impaire : address error au fetch (FC programme). L'instruction
                        // est avortée : le décrément du compteur N'EST PAS committé.
                        self.pc = target;
                        self.pending_address_error = Some((target, false, next_pc));
                    } else {
                        self.d[r] = (self.d[r] & 0xFFFF_0000) | (decremented as u32);
                        self.pc = target;
                    }
                    return Ok(10); // boucle continue
                } else {
                    self.d[r] = (self.d[r] & 0xFFFF_0000) | (decremented as u32);
                    return Ok(14); // compteur expiré
                }
            } else {
                self.fetch_word(bus); // consomme le déplacement
                return Ok(12); // condition devenue vraie
            }
        }

        // Scc : 0101 cccc 11 mmm rrr
        if opcode & 0b0000_0000_1100_0000 == 0b0000_0000_1100_0000 {
            let cc = (opcode >> 8) & 0b1111;
            let taken = self.test_condition(cc);
            let is_dn = mode == 0b000;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Byte)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            ae_write(ea.write(self, bus, Size::Byte, if taken { 0xFF } else { 0x00 }))?;
            return Ok(if is_dn {
                if taken { 6 } else { 4 }
            } else {
                8 + ea_extra
            });
        }

        // ADDQ / SUBQ
        let is_sub = opcode & 0x0100 != 0;
        let imm_bits = (opcode >> 9) & 0b111;
        let imm = if imm_bits == 0 { 8u32 } else { imm_bits as u32 };

        // Pour un registre d'adresse, pas de flags, taille toujours Long.
        // Yacht.txt lignes 943-950 : coût réel 8, quelle que soit la taille de
        // l'instruction (le M68000UM annonce 4 pour B/W, mais Yacht documente
        // l'erratum confirmé sur silicium réel).
        if mode == 0b001 {
            let r = reg as usize;
            if is_sub {
                self.a[r] = self.a[r].wrapping_sub(imm);
            } else {
                self.a[r] = self.a[r].wrapping_add(imm);
            }
            return Ok(8);
        }

        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let ea = self
            .resolve_ea(bus, mode, reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let is_dn = mode == 0b000;
        let val = ae_read(ea.read(self, bus, size))?;
        let result = if is_sub {
            self.sub_with_flags(val, imm, size)
        } else {
            self.add_with_flags(val, imm, size)
        };
        ae_write(ea.write(self, bus, size, result))?;
        let long = size == Size::Long;
        Ok(if is_dn {
            if long { 8 } else { 4 }
        } else if long {
            12 + ea_extra
        } else {
            8 + ea_extra
        })
    }

    // =========================================================================
    // Ligne 0110 : BRA / BSR / Bcc
    // =========================================================================

    fn op_branch(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let condition = (opcode >> 8) & 0b1111;
        let byte_disp = opcode as i8;
        let base = self.pc;
        let disp = if byte_disp == 0 {
            self.fetch_word(bus) as i16 as i32
        } else {
            byte_disp as i32
        };
        let target = (base as i32).wrapping_add(disp) as u32;

        match condition {
            0b0000 => {
                // BRA vers adresse impaire : frame_pc = pc après fetch opcode (= opcode_addr+2)
                let next_pc = base;
                self.pc = target;
                if target & 1 != 0 {
                    self.pending_address_error = Some((target, false, next_pc));
                }
                Ok(10)
            }
            0b0001 => {
                let ret = self.pc;
                let new_sp = self.sp().wrapping_sub(4);
                self.set_sp(new_sp);
                if new_sp & 1 != 0 {
                    return Err(StepError::AddressError(new_sp, true, self.pc));
                }
                bus.write32(self.sp() & ADDR_MASK, ret);
                // BSR vers adresse impaire : frame_pc = target (adresse de la cible)
                self.pc = target;
                if target & 1 != 0 {
                    self.pending_address_error = Some((target, false, target));
                }
                Ok(18) // Yacht.txt lignes 1005-1032 : la mise en pile n'était pas comptée
            }
            _ => {
                // Bcc : Yacht.txt lignes 1005-1032. Forme .B (déplacement non nul) :
                // prise=10, non prise=8. Forme .W (mot d'extension) : prise=10, non prise=12.
                let taken = self.test_condition(condition);
                if taken {
                    self.pc = target;
                }
                Ok(if taken {
                    10
                } else if byte_disp != 0 {
                    8
                } else {
                    12
                })
            }
        }
    }

    // =========================================================================
    // MOVEQ
    // =========================================================================

    fn op_moveq(&mut self, opcode: u16) -> Result<u32, StepError> {
        if opcode & 0x0100 != 0 {
            return Err(StepError::Unimplemented(opcode));
        }
        let reg = ((opcode >> 9) & 0b111) as usize;
        let value = opcode as i8 as i32 as u32;
        self.d[reg] = value;
        self.set_logic_flags(value, Size::Long);
        Ok(4)
    }

    // =========================================================================
    // Ligne 1000 : OR, DIVU, DIVS, SBCD
    // =========================================================================

    fn op_line_8(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // DIVU : opmode 011
        if size_bits == 0b011 {
            let opcode_addr = self.pc.wrapping_sub(2); // avant resolve_ea
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let divisor = ae_read(src.read(self, bus, Size::Word))?;
            if divisor == 0 {
                self.take_exception(bus, 5, opcode_addr);
                return Ok(ea_extra + 10);
            }
            let dividend = self.d[reg];
            let quotient = dividend / divisor;
            let remainder = dividend % divisor;
            let div_cycles = divu_core_cycles(dividend, divisor as u16);
            if quotient > 0xFFFF {
                // DIVU overflow (silicium 68000) : N=1, Z=0, V=1, C=0, X préservé.
                self.set_flag(ccr::N, true);
                self.set_flag(ccr::Z, false);
                self.set_flag(ccr::V, true);
                self.set_flag(ccr::C, false);
            } else {
                self.d[reg] = (remainder << 16) | (quotient & 0xFFFF);
                self.set_flag(ccr::N, quotient & 0x8000 != 0);
                self.set_flag(ccr::Z, quotient == 0);
                self.set_flag(ccr::V, false);
                self.set_flag(ccr::C, false);
            }
            // DIVU ne modifie pas X
            return Ok(div_cycles + 4 + ea_extra);
        }
        // DIVS : opmode 111
        if size_bits == 0b111 {
            let opcode_addr = self.pc.wrapping_sub(2);
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let divisor = ae_read(src.read(self, bus, Size::Word))? as u16 as i16 as i32;
            if divisor == 0 {
                self.take_exception(bus, 5, opcode_addr);
                return Ok(ea_extra + 10);
            }
            let dividend = self.d[reg] as i32;
            let quotient = dividend / divisor;
            let remainder = dividend % divisor;
            let div_cycles = divs_core_cycles(dividend, divisor as i16);
            if quotient > 0x7FFF || quotient < -0x8000 {
                // DIVS overflow (silicium 68000) : N=1, Z=0, V=1, C=0, X préservé.
                self.set_flag(ccr::N, true);
                self.set_flag(ccr::Z, false);
                self.set_flag(ccr::V, true);
                self.set_flag(ccr::C, false);
            } else {
                let q = quotient as u16 as u32;
                let r = (remainder as u16 as u32) << 16;
                self.d[reg] = r | q;
                self.set_flag(ccr::N, quotient < 0);
                self.set_flag(ccr::Z, quotient == 0);
                self.set_flag(ccr::V, false);
                self.set_flag(ccr::C, false);
            }
            // DIVS ne modifie pas X
            return Ok(div_cycles + 4 + ea_extra);
        }
        // SBCD : 1000 rrr 10000 mrrr
        if opcode & 0b1111_0001_1111_0000 == 0b1000_0001_0000_0000 {
            return self.op_sbcd(bus, opcode);
        }

        // OR : 1000 rrr d SS mmm rrr
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let r = a | b;
            ae_write(ea.write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        } else {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let r = a | b;
            ae_write(Operand::DataReg(reg).write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    // =========================================================================
    // Ligne 1001 : SUB / SUBA / SUBX
    // =========================================================================

    fn op_sub(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // SUBX : bit 8 = 1, bits5:4 = 00, size_bits ∉ {3,7} (distingue de SUBA)
        if opcode & 0x0100 != 0 && (opcode >> 4) & 0b11 == 0b00 && size_bits != 3 && size_bits != 7
        {
            let src_reg = ea_reg as usize;
            let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
            let mem_mode = opcode & 0x0008 != 0;
            let x = if self.flag(ccr::X) { 1u32 } else { 0 };

            let (src_val, dst_val, dst_op) = if mem_mode {
                // SUBX -(An),-(An) effectue un prefetch supplémentaire : frame_pc = pc+2.
                self.ea_frame_pc = self.pc.wrapping_add(2);
                let saved_src = self.a[src_reg];
                let saved_dst = self.a[reg];
                // Ordre matériel : décrément src → lecture src → décrément dst → lecture dst.
                let step = if src_reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[src_reg] = self.a[src_reg].wrapping_sub(step);
                let src_addr = self.a[src_reg];
                let s = match self.read_mem_checked(bus, src_addr, size) {
                    Ok(v) => v,
                    Err((addr, pc)) => {
                        if size == Size::Long {
                            self.a[src_reg] = saved_src;
                        }
                        // Voir ADDX (structure identique) : fault sur src,
                        // ea_extra="-(An)" générique ; préfixe nul en Long, +4
                        // en Word (même ajustement de taille que partout ailleurs).
                        self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                        self.fault_prefix = if size == Size::Long { 0 } else { 4 };
                        return Err(StepError::AddressError(addr, false, pc));
                    }
                };
                let dstep = if reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[reg] = self.a[reg].wrapping_sub(dstep);
                let dst_addr = self.a[reg];
                let d = match self.read_mem_checked(bus, dst_addr, size) {
                    Ok(v) => v,
                    Err((addr, pc)) => {
                        if size == Size::Long {
                            // AE sur dst (long) : seul le décrément dst est annulé.
                            // Le décrément src reste committé (y compris si src=A7/SSP).
                            self.a[reg] = saved_dst;
                        }
                        // Fault sur dst : src déjà lu avec succès, son coût réduit
                        // sert de préfixe (cf. ADDX), +4 en Word — sauf si src=A7
                        // (SSP), qui garde le préfixe réduit même en Word (calibré
                        // contre ProcessorTests : "-(A7),-(An)" fault dst = 64, pas 68).
                        self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                        self.fault_prefix = if size == Size::Long || src_reg == 7 {
                            8
                        } else {
                            12
                        };
                        return Err(StepError::AddressError(addr, false, pc));
                    }
                };
                (s, d, Operand::Memory(dst_addr & ADDR_MASK))
            } else {
                (
                    self.d[src_reg] & size.mask(),
                    self.d[reg] & size.mask(),
                    Operand::DataReg(reg),
                )
            };
            let result = self.subx_with_flags(dst_val, src_val, x, size);
            ae_write(dst_op.write(self, bus, size, result))?;
            let long = size == Size::Long;
            return Ok(if mem_mode {
                if long { 30 } else { 18 }
            } else if long {
                8
            } else {
                4
            });
        }

        // SUBA : opmode 011 ou 111
        if size_bits == 0b011 || size_bits == 0b111 {
            let size = if size_bits == 0b011 {
                Size::Word
            } else {
                Size::Long
            };
            let src = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            // #imm suit la même base que Dn/An (8+ea_extra, ea_extra=0 pour ces
            // derniers donc inchangé) plutôt que "6+ea_extra" des vrais modes
            // mémoire — calibré et vérifié exact contre ProcessorTests
            // (SUBA.l/ADDA.l "#,An" attendaient 16, pas 14).
            let src_is_reg_or_imm = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
            let value = size.sign_extend(ae_read(src.read(self, bus, size))?);
            self.a[reg] = self.a[reg].wrapping_sub(value);
            return Ok(if size == Size::Long {
                if src_is_reg_or_imm {
                    8 + ea_extra
                } else {
                    6 + ea_extra
                }
            } else {
                8 + ea_extra
            });
        }

        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let result = self.sub_with_flags(a, b, size);
            ae_write(ea.write(self, bus, size, result))?;
        } else {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let result = self.sub_with_flags(a, b, size);
            ae_write(Operand::DataReg(reg).write(self, bus, size, result))?;
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    // =========================================================================
    // Ligne 1011 : CMP / CMPA / CMPM / EOR
    // =========================================================================

    fn op_line_b(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // CMPA : opmode 011 ou 111
        if size_bits == 0b011 || size_bits == 0b111 {
            let size = if size_bits == 0b011 {
                Size::Word
            } else {
                Size::Long
            };
            let src = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let value = size.sign_extend(ae_read(src.read(self, bus, size))?);
            self.cmp_flags(self.a[reg], value, Size::Long);
            return Ok(6 + ea_extra);
        }

        // EOR : bit 8 = 1, destination ≠ An
        if opcode & 0x0100 != 0 {
            // CMPM : 1011 rrr 1 SS 001 rrr
            if mode == 0b001 {
                let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
                let src_r = ea_reg as usize;
                let dst_r = reg;
                let src_step = if src_r == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                let dst_step = if dst_r == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                // frame_pc pour CMPM : préfetch supplémentaire, pc+2 après fetch opcode
                self.ea_frame_pc = self.pc.wrapping_add(2);
                self.ea_is_pc_relative = false;
                let src_addr = self.a[src_r];

                // AE sur src pour accès word/long impair : incrément partiel +2.
                // Pour .b, une adresse impaire est légale → toujours commit_src.
                let commit_src = if size != Size::Byte && src_addr & 1 != 0 {
                    2
                } else {
                    src_step
                };
                self.a[src_r] = self.a[src_r].wrapping_add(commit_src);
                // Pas de resolve_ea ((An)+ géré manuellement) : ea_extra_cycles/
                // fault_prefix seraient sinon périmés. Calibré contre
                // ProcessorTests, même schéma que ADDX/SUBX : fault sur src
                // (première lecture) = ea_extra "(An)+" générique, préfixe nul
                // en Long / +4 en Word.
                self.ea_extra_cycles = if size == Size::Long { 8 } else { 4 };
                self.fault_prefix = if size == Size::Long { 0 } else { 4 };
                let s = ae_read(Operand::Memory(src_addr).read(self, bus, size))?;

                let dst_addr = self.a[dst_r];
                // AE sur dst uniquement pour accès word/long impair.
                // Pour .b, une adresse impaire est légale → toujours commit_dst.
                let dst_ae = size != Size::Byte && dst_addr & 1 != 0;
                let commit_dst = if dst_ae { 0 } else { dst_step };
                self.a[dst_r] = self.a[dst_r].wrapping_add(commit_dst);
                // Fault sur dst (deuxième lecture) : src déjà lu, son coût réduit
                // sert de préfixe = 8, identique en Word et Long ici (contrairement
                // à ADDX/SUBX qui ont besoin de +4 en Word) — calibré et vérifié
                // exact contre ProcessorTests.
                self.ea_extra_cycles = if size == Size::Long { 8 } else { 4 };
                self.fault_prefix = 8;
                let d = ae_read(Operand::Memory(dst_addr).read(self, bus, size))?;

                self.cmp_flags(d, s, size);
                return Ok(if size == Size::Long { 20 } else { 12 });
            }
            // EOR Dn,<ea>
            let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
            let ea = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let r = a ^ b;
            ae_write(ea.write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
            let long = size == Size::Long;
            return Ok(if ea_is_reg {
                if long { 8 } else { 4 }
            } else if long {
                12 + ea_extra
            } else {
                8 + ea_extra
            });
        }

        // CMP
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let src = ae_read(ea.read(self, bus, size))?;
        let dst = self.d[reg] & size.mask();
        self.cmp_flags(dst, src, size);
        let base = if size == Size::Long { 6 } else { 4 };
        Ok(base + ea_extra)
    }

    // =========================================================================
    // Ligne 1100 : AND / MULU / MULS / EXG / ABCD
    // =========================================================================

    fn op_line_c(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        // MULU : opmode 011
        // Yacht.txt 1211-1234 : coût exact = 38 + 2×(bits à 1 du mot source) + ea_extra.
        if size_bits == 0b011 {
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(src.read(self, bus, Size::Word))?;
            let result = (self.d[reg] & 0xFFFF) * (val & 0xFFFF);
            self.d[reg] = result;
            self.set_flag(ccr::N, result & 0x8000_0000 != 0);
            self.set_flag(ccr::Z, result == 0);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            let m = (val as u16).count_ones();
            return Ok(38 + 2 * m + ea_extra);
        }
        // MULS : opmode 111
        // Yacht.txt 1211-1234 : m = nb de transitions de bits adjacents (01/10)
        // dans les 16 bits de la source — même coût que MULU sinon.
        if size_bits == 0b111 {
            let src = self
                .resolve_ea(bus, mode, ea_reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let src_word = ae_read(src.read(self, bus, Size::Word))?;
            let val = src_word as u16 as i16 as i32;
            let result = ((self.d[reg] & 0xFFFF) as u16 as i16 as i32) * val;
            self.d[reg] = result as u32;
            self.set_flag(ccr::N, result < 0);
            self.set_flag(ccr::Z, result == 0);
            self.set_flag(ccr::V, false);
            self.set_flag(ccr::C, false);
            // Algorithme de Booth : bit[-1] (juste sous le LSB) vaut 0. m = nb de
            // transitions 01/10 entre bit[i] et bit[i-1] pour i=0..15 (bit0 est
            // comparé à ce bit[-1]=0 implicite, d'où le compte via src^(src<<1),
            // masqué à 16 bits pour ignorer le débordement du décalage).
            // Calibré et vérifié exact contre ProcessorTests
            // (src=$CBE0 → m=5, src=$9633 → m=9).
            let src16 = src_word as u16 as u32;
            let m = ((src16 ^ (src16 << 1)) & 0xFFFF).count_ones();
            return Ok(38 + 2 * m + ea_extra);
        }

        // EXG / ABCD : bit 8 = 1
        if opcode & 0x0100 != 0 {
            let op = (opcode >> 3) & 0b11111;
            match op {
                0b01000 => {
                    // EXG Dx,Dy
                    self.d.swap(reg, ea_reg as usize);
                    return Ok(6);
                }
                0b01001 => {
                    // EXG Ax,Ay
                    self.a.swap(reg, ea_reg as usize);
                    return Ok(6);
                }
                0b10001 => {
                    // EXG Dx,Ay
                    let tmp = self.d[reg];
                    self.d[reg] = self.a[ea_reg as usize];
                    self.a[ea_reg as usize] = tmp;
                    return Ok(6);
                }
                _ => {}
            }
            // ABCD : 1100 rrr 10000 mrrr (op bits 7-3 = 0000m)
            if op <= 0b00001 {
                return self.op_abcd(bus, opcode);
            }
        }

        // AND : 1100 rrr d SS mmm rrr
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let r = a & b;
            ae_write(ea.write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        } else {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let r = a & b;
            ae_write(Operand::DataReg(reg).write(self, bus, size, r))?;
            self.set_logic_flags(r, size);
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    // =========================================================================
    // Ligne 1101 : ADD / ADDA / ADDX
    // =========================================================================

    fn op_add(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let reg = ((opcode >> 9) & 0b111) as usize;
        let mode = (opcode >> 3) & 0b111;
        let ea_reg = opcode & 0b111;
        let size_bits = (opcode >> 6) & 0b111;

        if opcode & 0x0100 != 0 && (opcode >> 4) & 0b11 == 0b00 && size_bits != 3 && size_bits != 7
        {
            return self.op_addx(bus, opcode);
        }
        if size_bits == 0b011 || size_bits == 0b111 {
            let size = if size_bits == 0b011 {
                Size::Word
            } else {
                Size::Long
            };
            let src = self
                .resolve_ea(bus, mode, ea_reg, size)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            // Voir SUBA (formule identique) : #imm suit la base Dn/An.
            let src_is_reg_or_imm = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
            let value = size.sign_extend(ae_read(src.read(self, bus, size))?);
            self.a[reg] = self.a[reg].wrapping_add(value);
            return Ok(if size == Size::Long {
                if src_is_reg_or_imm {
                    8 + ea_extra
                } else {
                    6 + ea_extra
                }
            } else {
                8 + ea_extra
            });
        }
        let size = Size::from_bits(size_bits).ok_or(StepError::IllegalAddressing)?;
        let to_ea = opcode & 0x0100 != 0;
        let ea = self
            .resolve_ea(bus, mode, ea_reg, size)
            .ok_or(StepError::IllegalAddressing)?;
        let ea_extra = self.ea_extra_cycles;
        let ea_is_reg = mode <= 0b001 || (mode == 0b111 && ea_reg == 0b100);
        if to_ea {
            let a = self.d[reg] & size.mask();
            let b = ae_read(ea.read(self, bus, size))?;
            let result = self.add_with_flags(a, b, size);
            ae_write(ea.write(self, bus, size, result))?;
        } else {
            let a = ae_read(ea.read(self, bus, size))?;
            let b = self.d[reg] & size.mask();
            let result = self.add_with_flags(a, b, size);
            ae_write(Operand::DataReg(reg).write(self, bus, size, result))?;
        }
        Ok(logic_op_cost(to_ea, ea_is_reg, size, ea_extra))
    }

    fn op_addx(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        let dst_reg = ((opcode >> 9) & 0b111) as usize;
        let src_reg = (opcode & 0b111) as usize;
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let mem_mode = opcode & 0x0008 != 0;
        let x = if self.flag(ccr::X) { 1u32 } else { 0 };

        let (src_val, dst_val, dst_op) = if mem_mode {
            // ADDX -(An),-(An) effectue un prefetch supplémentaire : frame_pc = pc+2.
            self.ea_frame_pc = self.pc.wrapping_add(2);
            let saved_src = self.a[src_reg];
            let saved_dst = self.a[dst_reg];
            // Ordre matériel : décrément src → lecture src → décrément dst → lecture dst.
            let step = if src_reg == 7 && size == Size::Byte {
                2
            } else {
                size.bytes()
            };
            self.a[src_reg] = self.a[src_reg].wrapping_sub(step);
            let src_addr = self.a[src_reg];
            let s = match self.read_mem_checked(bus, src_addr, size) {
                Ok(v) => v,
                Err((addr, pc)) => {
                    // AE sur la lecture src : sur un accès long, les deux décréments
                    // sont annulés ; sur un word, le décrément src est conservé.
                    if size == Size::Long {
                        self.a[src_reg] = saved_src;
                    }
                    // Pas de resolve_ea (décréments manuels) : coûts calibrés
                    // contre ProcessorTests. Fault sur la lecture src (première
                    // des deux) : ea_extra="-(An)" générique (10/6) ; préfixe nul
                    // en Long, +4 en Word.
                    self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                    self.fault_prefix = if size == Size::Long { 0 } else { 4 };
                    return Err(StepError::AddressError(addr, false, pc));
                }
            };
            let dstep = if dst_reg == 7 && size == Size::Byte {
                2
            } else {
                size.bytes()
            };
            self.a[dst_reg] = self.a[dst_reg].wrapping_sub(dstep);
            let dst_addr = self.a[dst_reg];
            let d = match self.read_mem_checked(bus, dst_addr, size) {
                Ok(v) => v,
                Err((addr, pc)) => {
                    if size == Size::Long {
                        // AE sur dst (long) : seul le décrément dst est annulé.
                        // Le décrément src reste committé (y compris si src=A7/SSP).
                        self.a[dst_reg] = saved_dst;
                    }
                    // Fault sur la lecture dst (deuxième) : la lecture src a déjà
                    // réussi, son coût "-(An)" compte comme préfixe (réduit de 2,
                    // comme pour MOVEM — l'idle de décrément n'est pas repayé),
                    // +4 en Word sauf si src=A7 (SSP), qui garde le préfixe réduit
                    // (calibré contre ProcessorTests) ; ea_extra_cycles porte le
                    // coût "-(An)" du dst qui, lui, fault.
                    self.ea_extra_cycles = if size == Size::Long { 10 } else { 6 };
                    self.fault_prefix = if size == Size::Long || src_reg == 7 {
                        8
                    } else {
                        12
                    };
                    return Err(StepError::AddressError(addr, false, pc));
                }
            };
            (s, d, Operand::Memory(dst_addr & ADDR_MASK))
        } else {
            (
                self.d[src_reg] & size.mask(),
                self.d[dst_reg] & size.mask(),
                Operand::DataReg(dst_reg),
            )
        };
        let result = self.addx_with_flags(src_val, dst_val, x, size);
        ae_write(dst_op.write(self, bus, size, result))?;
        let long = size == Size::Long;
        Ok(if mem_mode {
            if long { 30 } else { 18 }
        } else if long {
            8
        } else {
            4
        })
    }

    // =========================================================================
    // Ligne 1110 : décalages et rotations
    // =========================================================================

    fn op_line_e(&mut self, bus: &mut impl Bus, opcode: u16) -> Result<u32, StepError> {
        // Décalage mémoire (1 bit seulement) : 1110 tt d 11 EA
        // bit 8 = direction (1=gauche, 0=droite) ; bits 10:9 = type (AS/LS/ROX/RO)
        if opcode & 0b1111_0000_1100_0000 == 0b1110_0000_1100_0000 {
            let dir = (opcode >> 8) & 1; // 0=droite, 1=gauche
            let shift_type = (opcode >> 9) & 0b11;
            let mode = (opcode >> 3) & 0b111;
            let reg = opcode & 0b111;
            let ea = self
                .resolve_ea(bus, mode, reg, Size::Word)
                .ok_or(StepError::IllegalAddressing)?;
            let ea_extra = self.ea_extra_cycles;
            let val = ae_read(ea.read(self, bus, Size::Word))?;
            let result = self.do_shift(val, 1, dir != 0, shift_type, Size::Word);
            ae_write(ea.write(self, bus, Size::Word, result))?;
            return Ok(8 + ea_extra);
        }

        // Décalage registre : 1110 ccc d SS i tt rrr
        let dir = (opcode >> 8) & 1;
        let size = Size::from_bits(opcode >> 6).ok_or(StepError::IllegalAddressing)?;
        let count_reg = opcode & 0x0020 != 0; // bit 5 : 0=immédiat, 1=Dn
        let shift_type = (opcode >> 3) & 0b11;
        let dst_reg = (opcode & 0b111) as usize;
        let count_raw = ((opcode >> 9) & 0b111) as u32;

        let count = if count_reg {
            self.d[count_raw as usize] % 64
        } else {
            if count_raw == 0 { 8 } else { count_raw }
        };

        let val = self.d[dst_reg] & size.mask();
        let result = self.do_shift(val, count, dir != 0, shift_type, size);
        ae_write(Operand::DataReg(dst_reg).write(self, bus, size, result))?;
        let base = if size == Size::Long { 8 } else { 6 };
        Ok(base + 2 * count)
    }

    /// Effectue un décalage/rotation et positionne les flags.
    ///
    /// `shift_type` : 00=AS, 01=LS, 10=ROX, 11=RO
    fn do_shift(&mut self, val: u32, count: u32, left: bool, shift_type: u16, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let bits = size.bytes() * 8;
        let val = val & mask;

        self.set_flag(ccr::C, false);
        self.set_flag(ccr::V, false);

        let result = if count == 0 {
            // Pas de décalage : C inchangé, X inchangé (sauf ROX où C = X)
            if shift_type == 0b10 {
                self.set_flag(ccr::C, self.flag(ccr::X));
            }
            val
        } else {
            match shift_type {
                0b00 => {
                    // ASL/ASR
                    if left {
                        let result = if count >= bits {
                            0
                        } else {
                            (val << count) & mask
                        };
                        let last_out = if count > bits {
                            false
                        } else if count == bits {
                            val & 1 != 0
                        } else {
                            (val >> (bits - count)) & 1 != 0
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        // V = 1 si un bit de signe a changé : val original ou intermédiaire
                        let v = if count >= bits {
                            val != 0 // tout est sorti, signe devenu 0, débordement si val≠0
                        } else {
                            // Vérifier si le signe a varié sur count pas
                            let mask_hi = if count >= bits {
                                mask
                            } else {
                                (mask >> (bits - count - 1)) << (bits - count - 1)
                            };
                            let _ = mask_hi;
                            // Approche simple : s'il existe un bit dans [val.msb..val.msb+count] qui diffère du signe initial
                            let orig_sign = val & msb != 0;
                            (0..count).any(|i| ((val << i) & mask & msb != 0) != orig_sign)
                                || (result & msb != 0) != orig_sign
                        };
                        self.set_flag(ccr::V, v);
                        result
                    } else {
                        let sign_bit = val & msb != 0;
                        let sign_fill = if sign_bit { mask } else { 0 };
                        let result = if count >= bits {
                            sign_fill
                        } else {
                            // Arithmetic right shift : sign-extend
                            let signed = (val as i32) << (32 - bits);
                            ((signed >> count) as u32 >> (32 - bits)) & mask
                        };
                        let last_out = if count > bits {
                            sign_bit
                        } else if count == bits {
                            val & msb != 0
                        } else {
                            (val >> (count - 1)) & 1 != 0
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        result
                    }
                }
                0b01 => {
                    // LSL/LSR
                    if left {
                        let result = if count >= bits {
                            0
                        } else {
                            val.wrapping_shl(count) & mask
                        };
                        let last_out = if count <= bits {
                            (val >> (bits - count)) & 1 != 0
                        } else {
                            false
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        result
                    } else {
                        let result = if count >= bits { 0 } else { val >> count };
                        let last_out = if count <= bits {
                            (val >> (count - 1)) & 1 != 0
                        } else {
                            false
                        };
                        self.set_flag(ccr::C, last_out);
                        self.set_flag(ccr::X, last_out);
                        result
                    }
                }
                0b10 => {
                    // ROXL/ROXR (rotation avec X)
                    let effective = count % (bits + 1);
                    if effective == 0 {
                        self.set_flag(ccr::C, self.flag(ccr::X));
                        val
                    } else if left {
                        let x_bit: u64 = if self.flag(ccr::X) { 1 } else { 0 };
                        let wide = (val as u64) | (x_bit << bits);
                        let n = bits + 1;
                        let rotated =
                            ((wide << effective) | (wide >> (n - effective))) & ((1u64 << n) - 1);
                        let new_x = (rotated >> bits) & 1 != 0;
                        self.set_flag(ccr::C, new_x);
                        self.set_flag(ccr::X, new_x);
                        rotated as u32 & mask
                    } else {
                        let x_bit: u64 = if self.flag(ccr::X) { 1 } else { 0 };
                        let wide = (val as u64) | (x_bit << bits);
                        let n = bits + 1;
                        let rotated =
                            ((wide >> effective) | (wide << (n - effective))) & ((1u64 << n) - 1);
                        let new_x = (rotated >> bits) & 1 != 0;
                        self.set_flag(ccr::C, new_x);
                        self.set_flag(ccr::X, new_x);
                        rotated as u32 & mask
                    }
                }
                0b11 => {
                    // ROL/ROR
                    let effective = count % bits;
                    if effective == 0 {
                        let last = if left { val & 1 != 0 } else { val & msb != 0 };
                        self.set_flag(ccr::C, last);
                        val
                    } else if left {
                        let result = ((val << effective) | (val >> (bits - effective))) & mask;
                        self.set_flag(ccr::C, result & 1 != 0);
                        result
                    } else {
                        let result = ((val >> effective) | (val << (bits - effective))) & mask;
                        self.set_flag(ccr::C, result & msb != 0);
                        result
                    }
                }
                _ => unreachable!(),
            }
        };

        self.set_flag(ccr::N, result & msb != 0);
        self.set_flag(ccr::Z, result == 0);
        result
    }

    // =========================================================================
    // Calcul des flags
    // =========================================================================

    pub(crate) fn set_logic_flags(&mut self, value: u32, size: Size) {
        let v = value & size.mask();
        self.set_flag(ccr::N, v & size.msb() != 0);
        self.set_flag(ccr::Z, v == 0);
        self.set_flag(ccr::V, false);
        self.set_flag(ccr::C, false);
    }

    fn add_with_flags(&mut self, a: u32, b: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let sum64 = (a as u64) + (b as u64);
        let sum = (sum64 as u32) & mask;

        let carry = sum64 > mask as u64;
        let overflow = ((a ^ sum) & (b ^ sum) & msb) != 0;

        self.set_flag(ccr::N, sum & msb != 0);
        self.set_flag(ccr::Z, sum == 0);
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, carry);
        self.set_flag(ccr::X, carry);
        sum
    }

    fn sub_with_flags(&mut self, a: u32, b: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let diff64 = (a as u64).wrapping_sub(b as u64);
        let diff = (diff64 as u32) & mask;

        let borrow = b > a;
        let overflow = ((a ^ b) & (a ^ diff) & msb) != 0;

        self.set_flag(ccr::N, diff & msb != 0);
        self.set_flag(ccr::Z, diff == 0);
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, borrow);
        self.set_flag(ccr::X, borrow);
        diff
    }

    fn addx_with_flags(&mut self, a: u32, b: u32, x: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let sum64 = (a as u64) + (b as u64) + (x as u64);
        let sum = (sum64 as u32) & mask;

        let carry = sum64 > mask as u64;
        let overflow = ((a ^ sum) & (b ^ sum) & msb) != 0;

        self.set_flag(ccr::N, sum & msb != 0);
        if sum != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, carry);
        self.set_flag(ccr::X, carry);
        sum
    }

    /// Soustraction pour CMP : positionne N Z V C mais **pas X**.
    fn cmp_flags(&mut self, a: u32, b: u32, size: Size) {
        let saved_x = self.flag(ccr::X);
        self.sub_with_flags(a, b, size);
        self.set_flag(ccr::X, saved_x);
    }

    fn subx_with_flags(&mut self, a: u32, b: u32, x: u32, size: Size) -> u32 {
        let mask = size.mask();
        let msb = size.msb();
        let a = a & mask;
        let b = b & mask;
        let diff64 = (a as u64).wrapping_sub(b as u64).wrapping_sub(x as u64);
        let diff = (diff64 as u32) & mask;

        let borrow = (b as u64) + (x as u64) > (a as u64);
        let overflow = ((a ^ b) & (a ^ diff) & msb) != 0;

        self.set_flag(ccr::N, diff & msb != 0);
        if diff != 0 {
            self.set_flag(ccr::Z, false);
        }
        self.set_flag(ccr::V, overflow);
        self.set_flag(ccr::C, borrow);
        self.set_flag(ccr::X, borrow);
        diff
    }

    pub fn test_condition(&self, cc: u16) -> bool {
        let n = self.flag(ccr::N);
        let z = self.flag(ccr::Z);
        let v = self.flag(ccr::V);
        let c = self.flag(ccr::C);
        match cc {
            0b0000 => true,
            0b0001 => false,
            0b0010 => !c && !z,
            0b0011 => c || z,
            0b0100 => !c,
            0b0101 => c,
            0b0110 => !z,
            0b0111 => z,
            0b1000 => !v,
            0b1001 => v,
            0b1010 => !n,
            0b1011 => n,
            0b1100 => n == v,
            0b1101 => n != v,
            0b1110 => !z && (n == v),
            0b1111 => z || (n != v),
            _ => unreachable!(),
        }
    }
}
