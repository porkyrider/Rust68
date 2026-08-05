#![cfg(feature = "atari-st")]
//! Vérifie le double bus fault (`Cpu::halted`) : un bus/address error
//! survenant PENDANT l'empilement du frame d'un précédent bus/address error
//! (ou la lecture de son vecteur) doit HALTer le CPU immédiatement, sans
//! rebondir — comportement du 68000 réel, et vérifié identique sur Hatari
//! (`src/cpu/newcpu.c`, `Exception()` :
//! `if ((m68k_areg(regs,7) & 1) || exception_in_exception < 0)
//! cpu_halt(CPU_HALT_DOUBLE_FAULT);`, arrêt immédiat, jamais de nouvelle
//! tentative). Un temps envisagé un rebond façon CLK (github.com/TomHarte/
//! CLK) sur la foi de son code source, mais CLK n'a en réalité jamais codé
//! ce cas (TODO reconnu par son auteur) — ce n'est pas une référence
//! faisant autorité ici, contrairement à Hatari.

use rust68::{Bus, Cpu, CpuType, sr};

/// Bus RAM plat avec un "trou" configurable : toute lecture/écriture dans
/// `hole` échoue (bus error) au lieu de réussir silencieusement.
struct HoleBus {
    mem: Vec<u8>,
    hole: std::ops::Range<u32>,
    fault: Option<(u32, bool)>,
}

impl HoleBus {
    fn new(size: usize, hole: std::ops::Range<u32>) -> Self {
        HoleBus {
            mem: vec![0; size],
            hole,
            fault: None,
        }
    }
}

impl Bus for HoleBus {
    fn read8(&mut self, addr: u32) -> u8 {
        if self.hole.contains(&addr) {
            self.fault = Some((addr, false));
            return 0;
        }
        self.mem[(addr as usize) % self.mem.len()]
    }
    fn write8(&mut self, addr: u32, value: u8) {
        if self.hole.contains(&addr) {
            self.fault = Some((addr, true));
            return;
        }
        let len = self.mem.len();
        self.mem[(addr as usize) % len] = value;
    }
    fn take_bus_fault(&mut self) -> Option<(u32, bool)> {
        self.fault.take()
    }
}

#[test]
fn address_error_pendant_l_empilement_halte_immediatement() {
    // Pile initiale à 0x0100, dans un trou qui couvre tout ce qui se trouve
    // en dessous : le premier empilement (14 octets) échoue forcément.
    let hole = 0x0000..0x0100;
    let mut bus = HoleBus::new(0x1000, hole);

    let mut cpu = Cpu::new();
    cpu.cpu_type = CpuType::M68000;
    cpu.sr = sr::S; // superviseur
    cpu.a[7] = 0x0100;
    cpu.pc = 0x0400;

    // Adresse impaire => address error (vecteur 3), déclenché "à la main"
    // comme le ferait le décodeur sur un accès mal aligné.
    cpu.take_address_error(&mut bus, 0x1235, true);

    assert!(
        cpu.halted,
        "un bus/address error en empilant le frame doit halter immédiatement (pas de rebond)"
    );
}

#[test]
fn address_error_avec_pile_valide_ne_halte_pas() {
    // Pile initiale valide (aucun trou) : l'empilement et la lecture du
    // vecteur réussissent tous les deux, aucun halt.
    let mut bus = HoleBus::new(0x1000, 0x0000..0x0000);
    bus.write32(0x0000_000C, 0x0000_0800); // vecteur 3 (address error)

    let mut cpu = Cpu::new();
    cpu.cpu_type = CpuType::M68000;
    cpu.sr = sr::S;
    cpu.a[7] = 0x0100;
    cpu.pc = 0x0400;

    cpu.take_address_error(&mut bus, 0x1235, true);

    assert!(!cpu.halted, "une exception isolée, non imbriquée, ne doit jamais halter");
    assert_eq!(cpu.pc, 0x0800);
    assert_eq!(cpu.a[7], 0x0100 - 14);
}

#[test]
fn cpu68010_meme_scenario_halte_aussi() {
    // Frame group 0 "long format" 68010+ non implémentée (voir l'assert de
    // `take_group0_exception`) : `take_exception` garde l'arrêt matériel
    // immédiat dans ce cas plutôt que d'escalader vers elle et paniquer.
    let hole = 0x00F0..0x0100;
    let mut bus = HoleBus::new(0x1000, hole);

    let mut cpu = Cpu::new();
    cpu.cpu_type = CpuType::M68010;
    cpu.sr = sr::S;
    cpu.a[7] = 0x0100;
    cpu.pc = 0x0400;

    // take_exception (frame court, vecteur 5 = division par zéro par ex.)
    // avec une pile qui échoue dès le premier empilement.
    cpu.take_exception(&mut bus, 5, 0x0400);

    assert!(cpu.halted, "68010+ : arrêt matériel immédiat, comme sur 68000");
}
