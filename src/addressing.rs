//! Tailles d'opération et modes d'adressage effectifs (EA) du 68000, étendus
//! au sous-ensemble 68020 du mot d'extension "complet" (voir
//! [`Cpu::resolve_indexed_full`]).

use crate::bus::Bus;
use crate::cpu::{ADDR_MASK, CpuType, Cpu};

/// Taille d'une opération.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// Octet (8 bits).
    Byte,
    /// Mot (16 bits).
    Word,
    /// Mot long (32 bits).
    Long,
}

impl Size {
    /// Nombre d'octets de la taille.
    pub fn bytes(self) -> u32 {
        match self {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        }
    }

    /// Décode le champ taille standard à 2 bits (00=byte, 01=word, 10=long).
    pub fn from_bits(bits: u16) -> Option<Size> {
        match bits & 0b11 {
            0b00 => Some(Size::Byte),
            0b01 => Some(Size::Word),
            0b10 => Some(Size::Long),
            _ => None,
        }
    }

    /// Étend une valeur de cette taille en `u32`, en propageant le signe.
    pub fn sign_extend(self, value: u32) -> u32 {
        match self {
            Size::Byte => value as u8 as i8 as i32 as u32,
            Size::Word => value as u16 as i16 as i32 as u32,
            Size::Long => value,
        }
    }

    /// Masque ne gardant que les bits significatifs de la taille.
    pub fn mask(self) -> u32 {
        match self {
            Size::Byte => 0x0000_00FF,
            Size::Word => 0x0000_FFFF,
            Size::Long => 0xFFFF_FFFF,
        }
    }

    /// Bit de signe (le bit de poids fort de la taille).
    pub fn msb(self) -> u32 {
        match self {
            Size::Byte => 0x0000_0080,
            Size::Word => 0x0000_8000,
            Size::Long => 0x8000_0000,
        }
    }
}

/// Une adresse effective résolue, prête à être lue ou écrite.
///
/// On résout l'EA en une fois (en consommant les mots d'extension nécessaires
/// et en appliquant les effets de bord pré-décrément / post-incrément), puis on
/// lit/écrit via [`Operand::read`] / [`Operand::write`].
#[derive(Debug, Clone, Copy)]
pub enum Operand {
    /// Registre de données Dn.
    DataReg(usize),
    /// Registre d'adresse An.
    AddrReg(usize),
    /// Emplacement mémoire à l'adresse résolue.
    Memory(u32),
    /// Donnée immédiate déjà extraite du flot d'instructions.
    Immediate(u32),
}

impl Operand {
    /// Lit la valeur de l'opérande à la taille donnée.
    /// Renvoie `Err((fault_addr, pc_at_fault))` si l'accès mémoire word/long est sur une adresse impaire.
    pub fn read(self, cpu: &Cpu, bus: &mut impl Bus, size: Size) -> Result<u32, (u32, u32)> {
        match self {
            Operand::DataReg(r) => Ok(cpu.d[r] & size.mask()),
            Operand::AddrReg(r) => Ok(match size {
                Size::Word => cpu.a[r] as u16 as i16 as i32 as u32 & size.mask(),
                _ => cpu.a[r] & size.mask(),
            }),
            Operand::Memory(addr) => {
                if size != Size::Byte && addr & 1 != 0 {
                    return Err((addr, cpu.ea_frame_pc));
                }
                Ok(read_sized(bus, addr, size))
            }
            Operand::Immediate(v) => Ok(v & size.mask()),
        }
    }

    /// Écrit `value` dans l'opérande à la taille donnée.
    /// Renvoie `Err((fault_addr, pc_at_fault))` si l'accès mémoire word/long est sur une adresse impaire.
    pub fn write(
        self,
        cpu: &mut Cpu,
        bus: &mut impl Bus,
        size: Size,
        value: u32,
    ) -> Result<(), (u32, u32)> {
        match self {
            Operand::DataReg(r) => {
                let keep = !size.mask();
                cpu.d[r] = (cpu.d[r] & keep) | (value & size.mask());
                Ok(())
            }
            Operand::AddrReg(r) => {
                cpu.a[r] = size.sign_extend(value);
                Ok(())
            }
            Operand::Memory(addr) => {
                if size != Size::Byte && addr & 1 != 0 {
                    // Un write effectue un prefetch supplémentaire avant le cycle bus :
                    // le PC du frame est avancé de 2 par rapport à un read.
                    return Err((addr, cpu.ea_frame_pc.wrapping_add(2)));
                }
                write_sized(bus, addr, size, value);
                Ok(())
            }
            Operand::Immediate(_) => panic!("écriture impossible dans une donnée immédiate"),
        }
    }
}

/// Lit en mémoire une valeur de la taille donnée.
fn read_sized(bus: &mut impl Bus, addr: u32, size: Size) -> u32 {
    let addr = addr & ADDR_MASK;
    match size {
        Size::Byte => bus.read8(addr) as u32,
        Size::Word => bus.read16(addr) as u32,
        Size::Long => bus.read32(addr),
    }
}

/// Écrit en mémoire une valeur de la taille donnée.
fn write_sized(bus: &mut impl Bus, addr: u32, size: Size, value: u32) {
    let addr = addr & ADDR_MASK;
    match size {
        Size::Byte => bus.write8(addr, value as u8),
        Size::Word => bus.write16(addr, value as u16),
        Size::Long => bus.write32(addr, value),
    }
}

/// Cycles supplémentaires de calcul d'adresse effective (EA), selon le mode
/// d'adressage et la taille — table "OPERAND EFFECTIVE ADDRESS CALCULATION
/// TIMES" de Yacht.txt (3rdparty/doc/Yacht.txt, lignes 127-153 du dépôt STAY).
/// Ce coût s'ajoute au coût de base de l'instruction ; il ne remplace rien.
pub fn ea_extra_cycles(mode: u16, reg: u16, size: Size) -> u32 {
    let long = size == Size::Long;
    match mode {
        0b000 | 0b001 => 0, // Dn, An
        0b010 | 0b011 => {
            if long {
                8
            } else {
                4
            }
        } // (An), (An)+
        0b100 => {
            if long {
                10
            } else {
                6
            }
        } // -(An)
        0b101 => {
            if long {
                12
            } else {
                8
            }
        } // (d16,An)
        0b110 => {
            if long {
                14
            } else {
                10
            }
        } // (d8,An,Xn)
        0b111 => match reg {
            0b000 => {
                if long {
                    12
                } else {
                    8
                }
            } // (xxx).W
            0b001 => {
                if long {
                    16
                } else {
                    12
                }
            } // (xxx).L
            0b010 => {
                if long {
                    12
                } else {
                    8
                }
            } // (d16,PC)
            0b011 => {
                if long {
                    14
                } else {
                    10
                }
            } // (d8,PC,Xn)
            0b100 => {
                if long {
                    8
                } else {
                    4
                }
            } // #imm
            _ => 0,
        },
        _ => 0,
    }
}

impl Cpu {
    /// Résout une adresse effective décrite par les champs `mode` (3 bits) et
    /// `reg` (3 bits) extraits de l'instruction, pour une opération de taille
    /// `size`. Consomme les mots d'extension nécessaires et applique les
    /// pré-décréments / post-incréments.
    ///
    /// Renvoie `None` pour un encodage de mode invalide.
    pub fn resolve_ea(
        &mut self,
        bus: &mut impl Bus,
        mode: u16,
        reg: u16,
        size: Size,
    ) -> Option<Operand> {
        self.ea_extra_cycles = ea_extra_cycles(mode, reg, size);
        // Préfixe d'address error par défaut pour un ae_read/ae_write qui suit
        // immédiatement : calibré contre ProcessorTests (voir Cpu::fault_prefix).
        // Byte/Word = 4 (fetch de l'opcode) ; Long = 0 — un accès long, décomposé
        // en deux transactions mot par TimedBus, ne "paie" pas ce préfixe une
        // deuxième fois quand la faute survient sur le premier mot. Constaté
        // identique que ce soit une lecture simple (TST/CMP/DIVU...) ou la
        // relecture RMW d'un `Dn,<ea>` (OR/AND/EOR/ADD/SUB) — seule la famille
        // immédiat-vers-mémoire (ORI/ANDI/SUBI/ADDI/EORI, op_line_0) diffère et
        // s'auto-corrige après cet appel.
        self.fault_prefix = if size == Size::Long { 0 } else { 4 };
        let reg = reg as usize;
        // PC après le fetch de l'opcode (avant tout mot d'extension de cette EA).
        // C'est le PC du frame d'address error pour les modes basés sur An :
        // les déplacements/index relatifs à An n'avancent pas le PC du frame.
        let pc_before_ext = self.pc;
        self.ea_frame_pc = pc_before_ext;
        self.ea_is_pc_relative = false;
        match mode {
            // Dn
            0b000 => Some(Operand::DataReg(reg)),
            // An
            0b001 => Some(Operand::AddrReg(reg)),
            // (An)
            0b010 => Some(Operand::Memory(self.a[reg])),
            // (An)+ : post-incrément
            0b011 => {
                let addr = self.a[reg];
                // Address error sur accès LONG à adresse impaire : le post-incrément
                // (+4) n'est PAS validé (les deux cycles word sont avortés). Pour un
                // accès WORD, le post-incrément (+2) reste committé. Vérifié sur
                // CMP/ADD/AND/OR/SUB/CLR/NOT/NEG/TST/MOVEtoCCR/MOVEfromSR.
                if size == Size::Long && addr & 1 != 0 {
                    return Some(Operand::Memory(addr));
                }
                // A7 reste aligné sur un mot même pour un accès octet.
                let step = if reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[reg] = self.a[reg].wrapping_add(step);
                Some(Operand::Memory(addr))
            }
            // -(An) : pré-décrément
            0b100 => {
                let step = if reg == 7 && size == Size::Byte {
                    2
                } else {
                    size.bytes()
                };
                self.a[reg] = self.a[reg].wrapping_sub(step);
                // Le pré-décrément consomme un cycle de prefetch supplémentaire
                // pour les accès word/byte uniquement (pas long).
                if size != Size::Long {
                    self.ea_frame_pc = self.ea_frame_pc.wrapping_add(2);
                }
                Some(Operand::Memory(self.a[reg]))
            }
            // (d16,An) : déplacement 16 bits signé depuis An
            0b101 => {
                let disp = self.fetch_word(bus) as i16 as i32;
                let addr = (self.a[reg] as i32).wrapping_add(disp) as u32;
                Some(Operand::Memory(addr))
            }
            // (d8,An,Xn) : adressage indexé avec déplacement 8 bits (ou mot
            // d'extension "complet" 68020, voir `resolve_indexed`).
            0b110 => {
                let addr = self.resolve_indexed(bus, self.a[reg])?;
                Some(Operand::Memory(addr))
            }
            // Modes 0b111 : la sélection se fait sur `reg`.
            0b111 => match reg {
                // (xxx).W : adresse absolue courte, signe étendu en 32 bits
                0b000 => {
                    let addr = self.fetch_word(bus) as i16 as i32 as u32;
                    // Modes absolus : le PC du frame avance avec la lecture de l'adresse.
                    self.ea_frame_pc = self.pc;
                    Some(Operand::Memory(addr))
                }
                // (xxx).L : adresse absolue longue
                0b001 => {
                    let addr = self.fetch_long(bus);
                    self.ea_frame_pc = self.pc;
                    Some(Operand::Memory(addr))
                }
                // (d16,PC) : déplacement relatif au PC — espace programme (FC=2/6)
                0b010 => {
                    let base = self.pc;
                    let disp = self.fetch_word(bus) as i16 as i32;
                    let addr = (base as i32).wrapping_add(disp) as u32;
                    self.ea_is_pc_relative = true;
                    Some(Operand::Memory(addr))
                }
                // (d8,PC,Xn) : indexé relatif au PC — espace programme (FC=2/6)
                0b011 => {
                    let base = self.pc;
                    let addr = self.resolve_indexed(bus, base)?;
                    self.ea_is_pc_relative = true;
                    Some(Operand::Memory(addr))
                }
                // #imm : donnée immédiate
                0b100 => {
                    let value = match size {
                        Size::Byte => self.fetch_word(bus) as u8 as u32,
                        Size::Word => self.fetch_word(bus) as u32,
                        Size::Long => self.fetch_long(bus),
                    };
                    Some(Operand::Immediate(value))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Résout un mode indexé `(d8,base,Xn)` : un mot d'extension fournit le
    /// registre d'index, sa taille (W/L), et un déplacement signé sur 8
    /// bits — le format "bref", seul existant sur 68000/68010. Sur 68020,
    /// si le bit 8 du mot d'extension est posé, délègue au format "complet"
    /// ([`Self::resolve_indexed_full`]) : silicium 68000/68010 réel
    /// n'inspecte jamais ce bit, donc le gate est sur `cpu_type`, pas
    /// seulement sur le bit lui-même — un programme 68000 qui produirait
    /// accidentellement un mot avec ce bit posé serait quand même décodé en
    /// bref, comme sur le vrai matériel.
    ///
    /// Renvoie `None` si le mot d'extension complet désigne une indirection
    /// mémoire (`I/IS != 000`, pré/post-indexée) — non implémentée (voir
    /// [`Self::resolve_indexed_full`]) ; l'appelant (`resolve_ea`) propage
    /// ce `None` comme n'importe quel encodage d'EA invalide.
    fn resolve_indexed(&mut self, bus: &mut impl Bus, base: u32) -> Option<u32> {
        let ext = self.fetch_word(bus);
        if ext & 0x0100 != 0 && self.cpu_type == CpuType::M68020 {
            return self.resolve_indexed_full(bus, base, ext);
        }
        let is_addr = ext & 0x8000 != 0; // bit 15 : A/D
        let xreg = ((ext >> 12) & 0b111) as usize;
        let long = ext & 0x0800 != 0; // bit 11 : W/L
        let disp = ext as i8 as i32; // déplacement 8 bits signé

        let index_full = if is_addr { self.a[xreg] } else { self.d[xreg] };
        let index = if long {
            index_full as i32
        } else {
            index_full as u16 as i16 as i32
        };

        Some((base as i32).wrapping_add(disp).wrapping_add(index) as u32)
    }

    /// Résout le mot d'extension "complet" du 68020 (bit 8 posé), sous-
    /// ensemble sans indirection mémoire (`I/IS == 000` uniquement — voir
    /// la doc de [`Self::resolve_indexed`]) :
    /// - bits 15/14-12/11 : D/A, Xn, W/L — identiques au format bref.
    /// - bits 10-9 : SCALE (×1/×2/×4/×8), appliqué à l'index une fois
    ///   étendu en signe selon W/L (`index << scale`, PAS l'inverse — le
    ///   68020 étend le signe d'abord, puis met à l'échelle).
    /// - bit 7 : BS (Base register Suppress) — le registre de base (`base`,
    ///   passé par l'appelant : An ou PC) est ignoré si posé.
    /// - bit 6 : IS (Index Suppress) — le terme d'index (registre ET
    ///   échelle) est ignoré si posé.
    /// - bits 5-4 : taille du déplacement de base (00 réservé, traité
    ///   défensivement comme nul ; 01 nul ; 10 mot signé ; 11 long),
    ///   mots d'extension supplémentaires consommés via `fetch_word`/
    ///   `fetch_long` (même mécanisme que `(d16,An)`/`(xxx).L` ailleurs
    ///   dans ce fichier).
    /// - bits 2-0 : I/IS — sélection d'indirection mémoire. Seul `000`
    ///   (aucune indirection, adressage indexé "complet" direct) est géré
    ///   ici ; toute autre valeur (pré/post-indexée) renvoie `None` — hors
    ///   périmètre pour l'instant (rare en pratique, lecture mémoire
    ///   intermédiaire + déplacement externe propre à ajouter dans un
    ///   passage ultérieur).
    fn resolve_indexed_full(&mut self, bus: &mut impl Bus, base: u32, ext: u16) -> Option<u32> {
        if ext & 0b111 != 0 {
            return None; // indirection mémoire : non implémentée
        }
        let is_addr = ext & 0x8000 != 0;
        let xreg = ((ext >> 12) & 0b111) as usize;
        let long = ext & 0x0800 != 0;
        let scale = (ext >> 9) & 0b11;
        let base_suppress = ext & 0x0080 != 0;
        let index_suppress = ext & 0x0040 != 0;
        let bd_size = (ext >> 4) & 0b11;

        let base_disp: i32 = match bd_size {
            0b10 => self.fetch_word(bus) as i16 as i32,
            0b11 => self.fetch_long(bus) as i32,
            // 00 (réservé) et 01 (nul) : aucun mot supplémentaire, terme nul.
            _ => 0,
        };

        let base_term: i32 = if base_suppress { 0 } else { base as i32 };
        let index_term: i32 = if index_suppress {
            0
        } else {
            let index_full = if is_addr { self.a[xreg] } else { self.d[xreg] };
            let index = if long {
                index_full as i32
            } else {
                index_full as u16 as i16 as i32
            };
            index << scale
        };

        Some(base_term.wrapping_add(base_disp).wrapping_add(index_term) as u32)
    }
}
