//! Interface mémoire/bus du CPU.
//!
//! Le 68000 ne connaît pas la disposition physique de la mémoire : il émet des
//! accès sur son bus, et le système (Atari, Amiga, banc de test…) décide ce que
//! ces adresses recouvrent (RAM, ROM, registres de périphériques…).
//!
//! L'appelant implémente [`Bus`] pour son système. Seuls [`Bus::read8`] et
//! [`Bus::write8`] sont obligatoires ; les accès 16 et 32 bits sont dérivés en
//! **big-endian** (l'ordre natif du 68000) et peuvent être surchargés pour de
//! meilleures performances ou pour modéliser un comportement particulier.

/// Bus mémoire vu par le CPU 68000.
///
/// Le 68000 dispose d'un espace d'adressage de 24 bits (16 Mo). Les adresses
/// passées ici sont déjà tronquées par le CPU à 24 bits significatifs.
pub trait Bus {
    /// Lit un octet à l'adresse `addr`.
    fn read8(&mut self, addr: u32) -> u8;

    /// Écrit un octet `value` à l'adresse `addr`.
    fn write8(&mut self, addr: u32, value: u8);

    /// Lit un mot (16 bits) big-endian à l'adresse `addr`.
    fn read16(&mut self, addr: u32) -> u16 {
        let hi = self.read8(addr) as u16;
        let lo = self.read8(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }

    /// Lit un mot long (32 bits) big-endian à l'adresse `addr`.
    fn read32(&mut self, addr: u32) -> u32 {
        let hi = self.read16(addr) as u32;
        let lo = self.read16(addr.wrapping_add(2)) as u32;
        (hi << 16) | lo
    }

    /// Écrit un mot (16 bits) big-endian à l'adresse `addr`.
    fn write16(&mut self, addr: u32, value: u16) {
        self.write8(addr, (value >> 8) as u8);
        self.write8(addr.wrapping_add(1), value as u8);
    }

    /// Écrit un mot long (32 bits) big-endian à l'adresse `addr`.
    fn write32(&mut self, addr: u32, value: u32) {
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }

    /// Appelé lorsque le CPU exécute l'instruction RESET (opcode 0x4E70).
    /// Sur le 68000, cette instruction génère le signal /RESET vers les
    /// périphériques externes pendant 124 cycles. Implémentation par défaut
    /// no-op ; surcharger pour propager le reset aux périphériques.
    fn reset_bus(&mut self) {}

    /// Vrai si `addr` est soumise à une contention de bus DRAM/vidéo (le
    /// Shifter/la puce vidéo partage le bus RAM avec le CPU et lui vole des
    /// cycles selon un motif périodique — modélisé par le CPU comme un
    /// arrondi à 4 cycles, cf. `Cpu::step`). Par défaut : aucune contention
    /// (ROM, registres périphériques, ou système sans modèle de contention).
    /// Les implémentations Atari ST/STE doivent renvoyer `true` pour les
    /// adresses RAM effectivement peuplées.
    fn is_contended(&self, addr: u32) -> bool {
        let _ = addr;
        false
    }

    /// Consulté par [`crate::cpu::Cpu::step`] juste après chaque transaction
    /// bus (fetch d'instruction, puis à nouveau après exécution) pour savoir
    /// si l'accès le plus récent a touché une adresse sans aucun chip select
    /// (le "trou" physique entre le haut de la RAM installée et le début de
    /// la ROM sur un ST/STE réel). Si `Some((fault_addr, is_write))` est
    /// renvoyé, le CPU déclenche immédiatement un bus error (vecteur 2) au
    /// lieu de continuer normalement, et l'indicateur doit être consommé
    /// (remis à `None`) par cet appel.
    ///
    /// Implémentation par défaut : jamais de bus error (mapping "toujours
    /// disponible" — comportement historique de tous les `Bus` existants,
    /// notamment les bancs de test ProcessorTests qui n'ont pas de notion de
    /// RAM limitée).
    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        None
    }

    /// Comme [`Self::take_bus_fault`], mais sans consommer l'indicateur —
    /// pour les instructions à accès multiples (MOVEM) qui doivent
    /// s'arrêter au PREMIER accès fautif (silicium réel : le CPU avorte
    /// immédiatement, il ne continue pas à essayer les registres suivants
    /// de la liste) tout en laissant [`Self::take_bus_fault`] intact pour
    /// que la vérification générique post-instruction de `Cpu::step`
    /// déclenche l'exception normalement, avec l'adresse du PREMIER accès
    /// fautif plutôt que du dernier.
    ///
    /// Implémentation par défaut : jamais de fault en attente (cohérent
    /// avec [`Self::take_bus_fault`] par défaut).
    fn has_pending_bus_fault(&self) -> bool {
        false
    }

    /// Niveau d'interruption (IPL2-0) actuellement demandé par les
    /// périphériques au CPU : 0 = aucune demande, 1-6 = niveau normal,
    /// 7 = non masquable (NMI). Consulté par [`crate::cpu::Cpu::step`] avant
    /// le fetch de chaque instruction : le CPU prend l'interruption si
    /// `level` est strictement supérieur au masque IPL courant du SR (ou
    /// toujours pour le niveau 7). Implémentation par défaut : aucune
    /// demande (bancs de test ProcessorTests, systèmes sans périphérique
    /// interruptif).
    fn irq_level(&self) -> u8 {
        0
    }

    /// Cycle d'acquittement d'interruption (IACK) pour le niveau `level` que
    /// le CPU vient d'accepter : le périphérique renvoie le numéro de
    /// vecteur (0-255) à utiliser pour construire l'adresse du handler.
    /// Implémentation par défaut : autovecteur (24 + level), le cas le plus
    /// courant sur Atari ST (GLUE HBL/VBL). Un périphérique vectorisé (MFP
    /// 68901) doit surcharger cette méthode pour renvoyer son propre
    /// vecteur programmé pour ce niveau.
    fn irq_ack(&mut self, level: u8) -> u8 {
        24 + level
    }
}

/// Puits d'événements pour [`TracingBus`] — découplé du format de sortie
/// (fichier, canal en mémoire, etc.) pour que `TracingBus` reste générique
/// et testable sans I/O.
pub trait TraceSink {
    /// `pc` : adresse de l'instruction CPU en cours (fournie par l'appelant,
    /// voir [`TracingBus::pc`]). `size` : 1/2/4 octets. `value` : la valeur
    /// lue/écrite, alignée à droite (pas de décalage selon `size`).
    fn on_read(&mut self, pc: u32, addr: u32, size: u8, value: u32);
    fn on_write(&mut self, pc: u32, addr: u32, size: u8, value: u32);
}

/// Bus intercalé qui journalise toute transaction bus réelle (taille exacte
/// de l'accès — .B/.W/.L — telle qu'émise par le CPU ou tout autre appelant
/// générique sur `impl Bus`) vers un [`TraceSink`], de façon transparente
/// pour le code traversé (comme [`TimedBus`] pour le minutage).
///
/// Contrairement à un traçage ad hoc dispersé dans les implémentations
/// `Bus` individuelles (qui ne peut voir qu'un octet à la fois dès que
/// l'appelant utilise les implémentations `read16`/`write16` par défaut du
/// trait, ou qui doit dupliquer la logique de traçage dans chaque
/// surcharge `read16`/`write16`/`read32`/`write32` spécifique à un
/// système), ce décorateur capture la transaction telle qu'émise par
/// l'appelant, à sa taille réelle, en un seul point — puis délègue à
/// `inner`, qui peut la décomposer ou la court-circuiter comme il
/// l'entend sans jamais avoir besoin de connaître le traçage.
///
/// `sink: None` est le chemin rapide quand le traçage est désactivé (un
/// seul test `Option::is_none` par accès, pas d'allocation ni de
/// formatage) — permet de toujours envelopper le bus dans la boucle
/// principale sans code dupliqué pour le cas "pas de traçage".
pub struct TracingBus<'b, B: Bus> {
    pub inner: &'b mut B,
    pub sink: Option<&'b mut dyn TraceSink>,
    /// PC de l'instruction en cours — à mettre à jour par l'appelant avant
    /// chaque `Cpu::step` (le CPU n'expose pas son PC courant au `Bus`).
    pub pc: u32,
}

impl<'b, B: Bus> Bus for TracingBus<'b, B> {
    fn read8(&mut self, addr: u32) -> u8 {
        let v = self.inner.read8(addr);
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_read(self.pc, addr, 1, v as u32);
        }
        v
    }

    fn write8(&mut self, addr: u32, value: u8) {
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_write(self.pc, addr, 1, value as u32);
        }
        self.inner.write8(addr, value);
    }

    fn read16(&mut self, addr: u32) -> u16 {
        let v = self.inner.read16(addr);
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_read(self.pc, addr, 2, v as u32);
        }
        v
    }

    fn write16(&mut self, addr: u32, value: u16) {
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_write(self.pc, addr, 2, value as u32);
        }
        self.inner.write16(addr, value);
    }

    fn read32(&mut self, addr: u32) -> u32 {
        let v = self.inner.read32(addr);
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_read(self.pc, addr, 4, v);
        }
        v
    }

    fn write32(&mut self, addr: u32, value: u32) {
        if let Some(sink) = self.sink.as_deref_mut() {
            sink.on_write(self.pc, addr, 4, value);
        }
        self.inner.write32(addr, value);
    }

    fn reset_bus(&mut self) {
        self.inner.reset_bus();
    }

    fn is_contended(&self, addr: u32) -> bool {
        self.inner.is_contended(addr)
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.inner.take_bus_fault()
    }

    fn has_pending_bus_fault(&self) -> bool {
        self.inner.has_pending_bus_fault()
    }

    fn irq_level(&self) -> u8 {
        self.inner.irq_level()
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        self.inner.irq_ack(level)
    }
}

/// Bus intercalé qui applique le modèle de wait-state DRAM/vidéo (façon
/// Steem : `RAM_ACCESS_WS`) à chaque transaction bus réelle, de façon
/// transparente pour tout le code du CPU (addressing.rs / execute.rs)
/// puisqu'ils sont génériques sur `impl Bus` et ne savent pas qu'ils
/// traversent ce wrapper.
///
/// `pos` est la position courante sur la grille de 4 cycles — initialisée
/// depuis `Cpu.cycles` par `Cpu::step` avant chaque instruction et jamais
/// remise à zéro entre les instructions, pour rester en phase avec
/// l'horloge vidéo continue. Chaque transaction consomme nominalement 4
/// cycles ; si `is_contended(addr)` et que la position n'est pas déjà
/// alignée sur 4, l'écart est perdu (aligné au 4 supérieur) avant la
/// transaction — exactement le mécanisme Steem.
pub struct TimedBus<'b, B: Bus> {
    pub inner: &'b mut B,
    pub pos: u64,
    pub access_count: u32,
}

impl<'b, B: Bus> TimedBus<'b, B> {
    fn charge(&mut self, addr: u32) {
        if self.inner.is_contended(addr) {
            let rem = self.pos & 3;
            if rem != 0 {
                self.pos += 4 - rem;
            }
        }
        self.access_count += 1;
        self.pos += 4;
    }
}

impl<'b, B: Bus> Bus for TimedBus<'b, B> {
    fn read8(&mut self, addr: u32) -> u8 {
        self.charge(addr);
        self.inner.read8(addr)
    }

    fn write8(&mut self, addr: u32, value: u8) {
        self.charge(addr);
        self.inner.write8(addr, value);
    }

    fn read16(&mut self, addr: u32) -> u16 {
        self.charge(addr);
        self.inner.read16(addr)
    }

    fn write16(&mut self, addr: u32, value: u16) {
        self.charge(addr);
        self.inner.write16(addr, value);
    }

    fn read32(&mut self, addr: u32) -> u32 {
        // Bus 16 bits réel : un accès long = deux cycles bus mot indépendants,
        // chacun avec son propre alignement de grille (pas un seul accès 32
        // bits atomique).
        let hi = self.read16(addr) as u32;
        let lo = self.read16(addr.wrapping_add(2)) as u32;
        (hi << 16) | lo
    }

    fn write32(&mut self, addr: u32, value: u32) {
        self.write16(addr, (value >> 16) as u16);
        self.write16(addr.wrapping_add(2), value as u16);
    }

    fn reset_bus(&mut self) {
        self.inner.reset_bus();
    }

    fn is_contended(&self, addr: u32) -> bool {
        self.inner.is_contended(addr)
    }

    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.inner.take_bus_fault()
    }

    fn has_pending_bus_fault(&self) -> bool {
        self.inner.has_pending_bus_fault()
    }

    fn irq_level(&self) -> u8 {
        self.inner.irq_level()
    }

    fn irq_ack(&mut self, level: u8) -> u8 {
        self.inner.irq_ack(level)
    }
}
