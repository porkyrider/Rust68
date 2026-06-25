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
}
