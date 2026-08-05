//! Rejoue la séquence RÉELLE de 617 blits capturée (registres + mémoire
//! source réelle + ROM réelle) lors de la sélection de l'icône disque sur
//! TOS 1.62/STE, et compare le résultat final octet par octet contre un
//! dump mémoire RÉEL de Hatari (via son débogueur intégré, `memdump`) pris
//! au même point exact (avant/après la sélection).
//!
//! ÉTAT CONNU : 42 divergences sur 464 octets comparés (zone f8f00-f90cf).
//! Non résolu — voir la conversation associée pour le détail de ce qui a
//! été exclu (halftone désormais correctement semé ; Y_COUNT=0→1 testé et
//! rejeté ; persistance du buffer testée et neutre ; skew variable au sein
//! d'une même chaîne testé isolément via `tests/blitter_hatari_diff.rs` et
//! ne reproduit PAS la divergence — donc le bug est spécifique à quelque
//! chose dans la séquence réelle complète à 188 appels, pas reproductible
//! par un sous-ensemble synthétique plus court).
//!
//! Les appels #63 et #107 (dans l'ordre 0-indexé de cette séquence) sont
//! les premiers points de divergence identifiés — tous deux de la famille
//! "corps de l'icône" (HOP=2, OP=7, NFSR+skew, direction négative).
use rust68::peripherals::atari_st::blitter::{Blitter, reg};
use rust68::Bus;

struct Mem { m: std::collections::HashMap<u32,u8>, current_call: usize, last_writer: std::collections::HashMap<u32,usize> }
impl Bus for Mem {
    fn read8(&mut self, a: u32) -> u8 { *self.m.get(&a).unwrap_or(&0) }
    fn write8(&mut self, a: u32, v: u8) { self.m.insert(a,v); self.last_writer.insert(a, self.current_call); }
}

fn wl(bl: &mut Blitter, off: u32, v: u32) { bl.write_long(off, v); }
fn ww(bl: &mut Blitter, off: u32, v: u16) { bl.write_word(off, v); }

/// Rejoue jusqu'a completion totale, comme le fait le CPU reel via sa boucle
/// de scrutation TAS.B qui rappelle execute() a chaque reprise tant que
/// BUSY reste actif (execute() est volontairement incremental : une seule
/// invocation ne traite qu'une tranche de 16 mots en mode non-HOG). Un seul
/// appel par ligne de trace tronquait silencieusement les blits multi-tranche.
fn run_to_completion(bl: &mut Blitter, bus: &mut impl Bus) {
    bl.execute(bus);
    while bl.busy() {
        bl.execute(bus);
    }
}

fn main() {
    let mut mem = Mem { m: std::collections::HashMap::new(), current_call: usize::MAX, last_writer: std::collections::HashMap::new() };
    let src_base: u32 = 0xd000;
    let src_bytes: &[u8] = &[
        0x74,0x69,0x6f,0x6e,0x7c,0x22,0x46,0x6f,0x72,0x6d,0x61,0x74,0x61,0x67,0x65,0x22,
        0x20,0x64,0x75,0x20,0x6d,0x65,0x6e,0x75,0x20,0x46,0x69,0x63,0x68,0x69,0x65,0x72,
        0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x43,
        0x6f,0x70,0x69,0x65,0x72,0x20,0x6c,0x65,0x20,0x64,0x69,0x73,0x71,0x75,0x65,0x20,
        0x25,0x53,0x3a,0x20,0x73,0x75,0x72,0x20,0x6c,0x65,0x7c,0x64,0x69,0x73,0x71,0x75,
        0x65,0x20,0x25,0x53,0x3a,0x20,0x65,0x66,0x66,0x61,0x63,0x65,0x72,0x61,0x20,0x74,
        0x6f,0x75,0x74,0x65,0x73,0x20,0x6c,0x65,0x73,0x7c,0x64,0x6f,0x6e,0x6e,0x82,0x65,
        0x73,0x20,0x63,0x6f,0x6e,0x74,0x65,0x6e,0x75,0x65,0x73,0x20,0x73,0x75,0x72,0x20,
        0x6c,0x65,0x7c,0x64,0x69,0x73,0x71,0x75,0x65,0x20,0x25,0x53,0x3a,0x2e,0x20,0x43,
        0x6c,0x69,0x71,0x75,0x65,0x72,0x20,0x73,0x75,0x72,0x20,0x4f,0x4b,0x7c,0x70,0x6f,
        0x75,0x72,0x20,0x63,0x6f,0x6e,0x66,0x69,0x72,0x6d,0x65,0x72,0x20,0x6c,0x61,0x20,
        0x63,0x6f,0x70,0x69,0x65,0x2e,0x5d,0x5b,0x4f,0x4b,0x7c,0x41,0x4e,0x4e,0x55,0x4c,
        0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x56,0x6f,0x75,0x73,0x20,0x6e,0x65,0x20,
        0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,0x61,0x73,0x20,0x64,0x82,0x70,0x6c,0x61,
        0x63,0x65,0x72,0x7c,0x6c,0x65,0x73,0x20,0x64,0x6f,0x73,0x73,0x69,0x65,0x72,0x73,
        0x20,0x65,0x74,0x20,0x6c,0x65,0x73,0x20,0x66,0x69,0x63,0x68,0x69,0x65,0x72,0x73,
        0x7c,0x73,0x75,0x72,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,0x75,0x2e,0x5d,
        0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x56,0x6f,0x75,
        0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,0x61,0x73,0x20,
        0x70,0x6c,0x61,0x63,0x65,0x72,0x7c,0x6c,0x27,0x69,0x63,0x93,0x6e,0x65,0x20,0x64,
        0x65,0x20,0x6c,0x61,0x20,0x70,0x6f,0x75,0x62,0x65,0x6c,0x6c,0x65,0x20,0x64,0x61,
        0x6e,0x73,0x7c,0x75,0x6e,0x65,0x20,0x66,0x65,0x6e,0x88,0x74,0x72,0x65,0x2e,0x5d,
        0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x32,0x5d,0x5b,0x56,0x6f,0x75,
        0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,0x61,0x73,0x20,
        0x63,0x72,0x82,0x65,0x72,0x20,0x64,0x65,0x7c,0x64,0x6f,0x73,0x73,0x69,0x65,0x72,
        0x20,0x61,0x76,0x65,0x63,0x20,0x63,0x65,0x20,0x6e,0x6f,0x6d,0x2e,0x7c,0x52,0x65,
        0x63,0x6f,0x6d,0x6d,0x65,0x6e,0x63,0x65,0x7a,0x20,0x65,0x6e,0x20,0x64,0x6f,0x6e,
        0x6e,0x61,0x6e,0x74,0x20,0x75,0x6e,0x7c,0x61,0x75,0x74,0x72,0x65,0x20,0x6e,0x6f,
        0x6d,0x2c,0x20,0x6f,0x75,0x20,0x61,0x6e,0x6e,0x75,0x6c,0x65,0x7a,0x20,0x6c,0x61,
        0x7c,0x64,0x65,0x6d,0x61,0x6e,0x64,0x65,0x20,0x64,0x65,0x20,0x63,0x72,0x82,0x61,
        0x74,0x69,0x6f,0x6e,0x2e,0x5d,0x5b,0x52,0x90,0x45,0x53,0x53,0x41,0x59,0x45,0x52,
        0x7c,0x41,0x4e,0x4e,0x55,0x4c,0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x43,0x65,
        0x20,0x64,0x69,0x73,0x71,0x75,0x65,0x20,0x6e,0x27,0x61,0x20,0x70,0x61,0x73,0x20,
        0x61,0x73,0x73,0x65,0x7a,0x20,0x64,0x65,0x7c,0x70,0x6c,0x61,0x63,0x65,0x20,0x70,
        0x6f,0x75,0x72,0x20,0x63,0x65,0x74,0x74,0x65,0x20,0x6f,0x70,0x82,0x72,0x61,0x74,
        0x69,0x6f,0x6e,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,
        0x5d,0x5b,0x56,0x6f,0x75,0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,
        0x20,0x70,0x61,0x73,0x20,0x63,0x72,0x82,0x65,0x72,0x20,0x6f,0x75,0x7c,0x61,0x63,
        0x63,0x82,0x64,0x65,0x72,0x20,0x85,0x20,0x75,0x6e,0x20,0x64,0x6f,0x73,0x73,0x69,
        0x65,0x72,0x20,0x61,0x75,0x73,0x73,0x69,0x7c,0x6c,0x6f,0x69,0x6e,0x20,0x64,0x61,
        0x6e,0x73,0x20,0x6c,0x27,0x61,0x72,0x62,0x6f,0x72,0x65,0x73,0x63,0x65,0x6e,0x63,
        0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,
        0x56,0x6f,0x75,0x73,0x20,0x6e,0x65,0x20,0x70,0x6f,0x75,0x76,0x65,0x7a,0x20,0x70,
        0x61,0x73,0x20,0x63,0x6f,0x70,0x69,0x65,0x72,0x7c,0x75,0x6e,0x20,0x64,0x6f,0x73,
        0x73,0x69,0x65,0x72,0x20,0x6f,0x75,0x20,0x75,0x6e,0x20,0x66,0x69,0x63,0x68,0x69,
        0x65,0x72,0x20,0x64,0x65,0x7c,0x6f,0x75,0x20,0x73,0x75,0x72,0x20,0x75,0x6e,0x65,
        0x20,0x63,0x61,0x72,0x74,0x6f,0x75,0x63,0x68,0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,
        0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x4f,0x70,0x82,0x72,0x61,0x74,0x69,
        0x6f,0x6e,0x20,0x64,0x65,0x20,0x63,0x6f,0x70,0x69,0x65,0x20,0x69,0x6e,0x76,0x61,
        0x6c,0x69,0x64,0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,
        0x31,0x5d,0x5b,0x4c,0x61,0x20,0x70,0x6f,0x75,0x62,0x65,0x6c,0x6c,0x65,0x20,0x73,
        0x65,0x72,0x74,0x20,0x85,0x20,0x65,0x66,0x66,0x61,0x63,0x65,0x72,0x7c,0x75,0x6e,
        0x20,0x64,0x6f,0x73,0x73,0x69,0x65,0x72,0x20,0x6f,0x75,0x20,0x75,0x6e,0x20,0x66,
        0x69,0x63,0x68,0x69,0x65,0x72,0x7c,0x64,0x65,0x20,0x66,0x61,0x87,0x6f,0x6e,0x20,
        0x70,0x65,0x72,0x6d,0x61,0x6e,0x65,0x6e,0x74,0x65,0x2e,0x5d,0x5b,0x20,0x20,0x4f,
        0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x4c,0x65,0x20,0x73,0x79,0x73,0x74,
        0x8a,0x6d,0x65,0x20,0x6e,0x27,0x61,0x20,0x70,0x6c,0x75,0x73,0x20,0x61,0x73,0x73,
        0x65,0x7a,0x20,0x64,0x65,0x7c,0x6d,0x82,0x6d,0x6f,0x69,0x72,0x65,0x20,0x21,0x5d,
        0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x55,0x6e,0x65,
        0x20,0x65,0x72,0x72,0x65,0x75,0x72,0x20,0x61,0x20,0x82,0x74,0x82,0x20,0x64,0x82,
        0x74,0x65,0x63,0x74,0x82,0x65,0x7c,0x64,0x75,0x72,0x61,0x6e,0x74,0x20,0x75,0x6e,
        0x20,0x66,0x6f,0x72,0x6d,0x61,0x74,0x61,0x67,0x65,0x20,0x6f,0x75,0x20,0x75,0x6e,
        0x65,0x7c,0x63,0x6f,0x70,0x69,0x65,0x2e,0x20,0x4c,0x65,0x20,0x64,0x69,0x73,0x71,
        0x75,0x65,0x20,0x63,0x6f,0x6e,0x63,0x65,0x72,0x6e,0x82,0x20,0x65,0x73,0x74,0x7c,
        0x70,0x65,0x75,0x74,0x2d,0x88,0x74,0x72,0x65,0x20,0x70,0x72,0x6f,0x74,0x82,0x67,
        0x82,0x20,0x65,0x6e,0x20,0x82,0x63,0x72,0x69,0x74,0x75,0x72,0x65,0x7c,0x6f,0x75,
        0x20,0x69,0x6e,0x75,0x74,0x69,0x6c,0x69,0x73,0x61,0x62,0x6c,0x65,0x2e,0x5d,0x5b,
        0x52,0x90,0x45,0x53,0x53,0x41,0x59,0x45,0x52,0x7c,0x41,0x42,0x41,0x4e,0x44,0x4f,
        0x4e,0x4e,0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x43,0x65,0x20,0x64,0x69,0x73,
        0x71,0x75,0x65,0x20,0x63,0x6f,0x6e,0x74,0x69,0x65,0x6e,0x74,0x20,0x25,0x4c,0x7c,
        0x6f,0x63,0x74,0x65,0x74,0x73,0x20,0x64,0x69,0x73,0x70,0x6f,0x6e,0x69,0x62,0x6c,
        0x65,0x73,0x20,0x70,0x6f,0x75,0x72,0x7c,0x6c,0x27,0x75,0x74,0x69,0x6c,0x69,0x73,
        0x61,0x74,0x65,0x75,0x72,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,
        0x5b,0x33,0x5d,0x5b,0x4c,0x65,0x20,0x64,0x69,0x73,0x71,0x75,0x65,0x20,0x64,0x65,
        0x20,0x64,0x65,0x73,0x74,0x69,0x6e,0x61,0x74,0x69,0x6f,0x6e,0x20,0x6e,0x27,0x65,
        0x73,0x74,0x7c,0x70,0x61,0x73,0x20,0x64,0x75,0x20,0x6d,0x88,0x6d,0x65,0x20,0x74,
        0x79,0x70,0x65,0x20,0x71,0x75,0x65,0x20,0x6c,0x65,0x20,0x64,0x69,0x73,0x71,0x75,
        0x65,0x7c,0x64,0x65,0x20,0x64,0x82,0x70,0x61,0x72,0x74,0x2e,0x20,0x49,0x6e,0x73,
        0x82,0x72,0x65,0x72,0x20,0x75,0x6e,0x20,0x61,0x75,0x74,0x72,0x65,0x7c,0x64,0x69,
        0x73,0x71,0x75,0x65,0x2e,0x5d,0x5b,0x52,0x90,0x45,0x53,0x53,0x41,0x59,0x45,0x52,
        0x7c,0x41,0x42,0x41,0x4e,0x44,0x4f,0x4e,0x4e,0x45,0x52,0x5d,0x00,0x5b,0x31,0x5d,
        0x5b,0x53,0x61,0x75,0x76,0x65,0x72,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,
        0x75,0x20,0x3f,0x5d,0x5b,0x4f,0x4b,0x7c,0x41,0x4e,0x4e,0x55,0x4c,0x45,0x52,0x5d,
        0x00,0x5b,0x31,0x5d,0x5b,0x49,0x6d,0x70,0x72,0x65,0x73,0x73,0x69,0x6f,0x6e,0x20,
        0x64,0x65,0x20,0x6c,0x27,0x82,0x63,0x72,0x61,0x6e,0x20,0x3f,0x5d,0x5b,0x4f,0x4b,
        0x7c,0x41,0x4e,0x4e,0x55,0x4c,0x45,0x52,0x5d,0x00,0x5b,0x33,0x5d,0x5b,0x53,0x65,
        0x75,0x6c,0x73,0x20,0x6c,0x65,0x73,0x20,0x6c,0x65,0x63,0x74,0x65,0x75,0x72,0x73,
        0x20,0x64,0x65,0x7c,0x64,0x69,0x73,0x71,0x75,0x65,0x74,0x74,0x65,0x73,0x20,0x73,
        0x6f,0x6e,0x74,0x20,0x75,0x74,0x69,0x6c,0x69,0x73,0x61,0x62,0x6c,0x65,0x73,0x7c,
        0x70,0x6f,0x75,0x72,0x20,0x63,0x65,0x74,0x74,0x65,0x20,0x6f,0x70,0x82,0x72,0x61,
        0x74,0x69,0x6f,0x6e,0x2e,0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,
        0x31,0x5d,0x5b,0x41,0x62,0x61,0x6e,0x64,0x6f,0x6e,0x6e,0x65,0x72,0x20,0x63,0x65,
        0x74,0x74,0x65,0x20,0x6f,0x70,0x82,0x72,0x61,0x74,0x69,0x6f,0x6e,0x20,0x3f,0x5d,
        0x5b,0x20,0x4f,0x55,0x49,0x20,0x7c,0x4e,0x4f,0x4e,0x5d,0x00,0x5b,0x31,0x5d,0x5b,
        0x44,0x82,0x73,0x6f,0x6c,0x82,0x2c,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,
        0x75,0x20,0x6e,0x65,0x20,0x70,0x65,0x75,0x74,0x7c,0x70,0x61,0x73,0x20,0x69,0x6e,
        0x73,0x74,0x61,0x6c,0x6c,0x65,0x72,0x20,0x64,0x27,0x69,0x63,0x93,0x6e,0x65,0x7c,
        0x73,0x75,0x70,0x6c,0x82,0x6d,0x65,0x6e,0x74,0x61,0x69,0x72,0x65,0x2e,0x5d,0x5b,
        0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x5b,0x31,0x5d,0x5b,0x44,0x82,0x73,0x6f,
        0x6c,0x82,0x2c,0x20,0x6c,0x65,0x20,0x62,0x75,0x72,0x65,0x61,0x75,0x20,0x6e,0x65,
        0x20,0x70,0x65,0x75,0x74,0x20,0x70,0x61,0x73,0x7c,0x69,0x6e,0x73,0x74,0x61,0x6c,
        0x6c,0x65,0x72,0x20,0x64,0x27,0x61,0x70,0x70,0x6c,0x69,0x63,0x61,0x74,0x69,0x6f,
        0x6e,0x7c,0x73,0x75,0x70,0x6c,0x82,0x6d,0x65,0x6e,0x74,0x61,0x69,0x72,0x65,0x2e,
        0x5d,0x5b,0x20,0x20,0x4f,0x4b,0x20,0x20,0x5d,0x00,0x00,0x00,0x00,0x00,0x00,0x1b,
        0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x1b,
        0xb0,0x00,0x00,0x1b,0xb0,0x00,0x00,0x3b,0xb8,0x00,0x00,0x3b,0xb8,0x00,0x00,0x3b,
        0xb8,0x00,0x00,0x3b,0xb8,0x00,0x00,0x7b,0xbc,0x00,0x00,0x7b,0xbc,0x00,0x00,0xfb,
        0xbe,0x00,0x01,0xf3,0x9f,0x00,0x03,0xf3,0x9f,0x80,0x0f,0xe3,0x8f,0xe0,0x7f,0xc3,
        0x87,0xfc,0x7f,0x83,0x83,0xfc,0x7e,0x03,0x80,0xfc,0x78,0x03,0x80,0x3c,0x00,0x00,
        0x00,0x00,0x09,0xf9,0x0f,0x8c,0x1d,0xfb,0x8f,0xcc,0x1c,0x63,0x8c,0xec,0x36,0x66,
        0xcc,0xec,0x36,0x66,0xcd,0xcc,0x7f,0x6f,0xed,0x8c,0x7f,0x6f,0xed,0xcc,0x63,0x6c,
        0x6c,0xec,0x63,0x6c,0x6c,0x6c,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x0f,0xe0,0x00,0x00,0x1f,0xf0,0x00,0x7f,0x7f,0xfc,0x00,0xff,
        0xff,0xfc,0x03,0xff,0xff,0xff,0x03,0xff,0xff,0xff,0x0f,0xff,0xff,0xff,0x0f,0xff,
        0xff,0xff,0x3f,0xff,0xff,0xff,0x3f,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,
        0xff,0xff,0xff,0xff,0xff,0xfe,0xff,0xff,0xff,0xfc,0xff,0xff,0xff,0xf8,0xff,0xff,
        0xff,0xf0,0xff,0xff,0xff,0xe0,0xff,0xff,0xff,0xc0,0xff,0xff,0xff,0x80,0xff,0xff,
        0xff,0x00,0xff,0xff,0xfe,0x00,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x0f,0xe0,0x00,0x00,0x18,0x30,0x00,0x7f,0x70,0x1c,0x00,0xc1,
        0x80,0x04,0x03,0x80,0xff,0xf7,0x02,0x00,0x00,0x15,0x0f,0xfb,0xfb,0xd3,0x08,0x06,
        0x0c,0x57,0x3f,0xfc,0x07,0x4d,0x20,0x00,0x01,0x59,0xff,0xff,0xfe,0x31,0x80,0x00,
        0x02,0x63,0x80,0x00,0x02,0xc5,0x80,0x00,0x03,0x89,0x80,0x00,0x03,0x13,0x80,0x00,
        0x02,0x25,0x80,0x00,0x02,0x49,0x80,0x00,0x02,0x91,0x81,0xfe,0x03,0x23,0x81,0x02,
        0x02,0x46,0x81,0x02,0x02,0x8c,0x81,0xfe,0x03,0x18,0x80,0x00,0x02,0x30,0x80,0x00,
        0x02,0x60,0x83,0x06,0x02,0xc0,0x87,0xfc,0x03,0x80,0x80,0x00,0x03,0x00,0x80,0x00,
        0x02,0x00,0x80,0x00,0x02,0x00,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x0f,0xfc,0x00,0x00,0x1f,0xfe,0x1f,0xff,0xff,0xfe,0x3f,0xff,0xff,0xfe,0x3f,0xff,
        0xff,0xfe,0x3f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x3f,0xff,0xff,0xfc,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x0f,0xfc,0x00,0x00,0x18,0x06,0x1f,0xff,0xf0,0x02,0x20,0x00,0x00,0x02,0x3f,0xff,
        0xff,0xf2,0x20,0x00,0x00,0x0a,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x60,0x00,
        0x00,0x06,0x3f,0xff,0xff,0xfc,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x03,0xe0,0x00,0x00,0x7e,0x3f,0x00,0x01,0xff,0xff,0xc0,0x03,0xff,
        0xff,0xe0,0x03,0xff,0xff,0xe0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,
        0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x01,0xff,0xff,0xc0,0x00,0xff,
        0xff,0x80,0x00,0x3f,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x03,0xe0,0x00,0x00,0x7e,0x3f,0x00,0x01,0xc6,0x31,0xc0,0x02,0x00,
        0x00,0x20,0x03,0xc0,0x01,0xe0,0x01,0x7f,0xff,0x40,0x01,0x00,0x00,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x8a,0x40,0x01,0x44,0xda,0x40,0x01,0x44,0x72,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x44,
        0x22,0x40,0x01,0x44,0x22,0x40,0x01,0x64,0x26,0x40,0x01,0x86,0x60,0xc0,0x00,0xe0,
        0x03,0x80,0x00,0x3f,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x7f,0xff,0xff,0xfc,0x7f,0xff,
        0xff,0xfc,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,0xff,0xfe,0x7f,0xff,
        0xff,0xfe,0x1f,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x7f,0xff,0xff,0xfc,0x40,0x00,
        0x00,0x04,0x55,0x55,0x55,0x56,0x40,0x00,0x00,0x06,0x7f,0xff,0xff,0xfe,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,
        0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x40,0x00,0x00,0x06,0x7f,0xff,
        0xff,0xfe,0x1f,0xff,0xff,0xfe,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x1f,0xff,
        0xff,0x80,0x1f,0xff,0xff,0x80,0x1f,0xff,0xff,0xe0,0x1f,0xff,0xff,0xe0,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,
        0xff,0xf8,0x1f,0xff,0xff,0xf8,0x1f,0xff,0xff,0xf8,0x07,0xff,0xff,0xf8,0x07,0xff,
        0xff,0xf8,0x01,0xff,0xff,0xf8,0x01,0xff,0xff,0xf8,0x00,0x00,0x00,0x00,0x1f,0xff,
        0xff,0x80,0x10,0x00,0x00,0x80,0x10,0x00,0x00,0xe0,0x10,0x00,0x00,0xa0,0x10,0x00,
        0x00,0xb8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,0x00,0xa8,0x10,0x00,
        0x3f,0xa8,0x10,0x00,0x21,0xa8,0x10,0x00,0x23,0x28,0x10,0x00,0x26,0x28,0x10,0x00,
        0x2c,0x28,0x10,0x00,0x38,0x28,0x1f,0xff,0xf0,0x28,0x04,0x00,0x00,0x28,0x07,0xff,
        0xff,0xe8,0x01,0x00,0x00,0x08,0x01,0xff,0xff,0xf8,0x00,0x00,0xcc,0xc3,0x00,0x00,
        0xcc,0xe6,0x00,0x00,0xcc,0xee,0x00,0x00,0xcc,0xff,0x00,0x00,0xcd,0x09,0x00,0x00,
        0xcd,0x1d,0x00,0x00,0xcd,0x36,0x00,0x00,0xcd,0x4b,0x00,0x00,0xcd,0x60,0x00,0x00,
        0xcd,0x7a,0x00,0x00,0xcd,0x8e,0x00,0x00,0xcd,0xa3,0x00,0x00,0xcd,0xb7,0x00,0x00,
        0xce,0x07,0x00,0x00,0xce,0x7e,0x00,0x00,0xce,0xbd,0x00,0x00,0xcf,0x41,0x00,0x00,
        0xcf,0x90,0x00,0x00,0xd0,0x2b,0x00,0x00,0xd0,0xc4,0x00,0x00,0xd1,0x19,0x00,0x00,
        0xd1,0x69,0x00,0x00,0xd1,0xfa,0x00,0x00,0xd2,0x3e,0x00,0x00,0xd2,0x9c,0x00,0x00,
        0xd2,0xf5,0x00,0x00,0xd3,0x1f,0x00,0x00,0xd3,0x75,0x00,0x00,0xd3,0xa9,0x00,0x00,
        0xd4,0x46,0x00,0x00,0xd4,0x90,0x00,0x00,0xd5,0x0d,0x00,0x00,0xd5,0x31,0x00,0x00,
        0xd5,0x5a,0x00,0x00,0xd5,0xaf,0x00,0x00,0xd5,0xdc,0x00,0x00,0xd6,0x28,0x00,0x00,
        0xd6,0x7a,0x00,0x04,0x00,0x20,0x00,0x00,0x00,0x00,0x00,0x02,0x00,0x00,0xd6,0xfa,
        0x00,0x00,0xd7,0x7a,0x00,0x00,0xcc,0x4e,0x10,0x00,0x00,0x0d,0x00,0x0d,0x00,0x14,
        0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,0x00,0x48,0x00,0x08,0x00,0x00,
        0xd7,0xfa,0x00,0x00,0xd8,0x7a,0x00,0x00,0xcc,0x5b,0x10,0x00,0x00,0x00,0x00,0x00,
        0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,0x00,0x48,0x00,0x08,
        0x00,0x00,0xd8,0xfa,0x00,0x00,0xd9,0x7a,0x00,0x00,0xcc,0x68,0x10,0x00,0x00,0x00,
        0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,0x00,0x48,
        0x00,0x08,0x00,0x00,0xd9,0xfa,0x00,0x00,0xda,0x7a,0x00,0x00,0xcc,0x75,0x10,0x00,
        0x00,0x00,0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,0x00,0x20,
        0x00,0x48,0x00,0x08,0x00,0x00,0xda,0xfa,0x00,0x00,0xdb,0x7a,0x00,0x00,0xcc,0x82,
        0x10,0x00,0x00,0x00,0x00,0x00,0x00,0x14,0x00,0x00,0x00,0x20,0x00,0x20,0x00,0x00,
        0x00,0x20,0x00,0x48,0x00,0x08,0x00,0x00,0xc5,0x60,0x00,0x00,0xc5,0x62,0x00,0x00,
        0xc5,0x63,0x00,0x03,0x00,0x06,0x00,0x02,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x02,
        0x00,0x01,0x00,0x00,0xc5,0x64,0x00,0x00,0xc5,0x70,0x00,0x00,0xc5,0x83,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x13,0x00,0x00,
        0xc5,0x85,0x00,0x00,0xc5,0x90,0x00,0x00,0xc5,0xab,0x00,0x03,0x00,0x06,0x00,0x01,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0b,0x00,0x1b,0x00,0x00,0xc5,0xad,0x00,0x00,
        0xc5,0xb4,0x00,0x00,0xc5,0xc4,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x07,0x00,0x10,0x00,0x00,0xc5,0xc6,0x00,0x00,0xc5,0xcd,0x00,0x00,
        0xc5,0xde,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x07,
        0x00,0x11,0x00,0x00,0xc5,0xe0,0x00,0x00,0xc5,0xe6,0x00,0x00,0xc6,0x01,0x00,0x03,
        0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x1b,0x00,0x00,
        0xc6,0x03,0x00,0x00,0xc6,0x09,0x00,0x00,0xc6,0x24,0x00,0x03,0x00,0x06,0x00,0x01,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x1b,0x00,0x00,0xc6,0x4e,0x00,0x00,
        0xc6,0x50,0x00,0x00,0xc6,0x63,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x02,0x00,0x13,0x00,0x00,0xc6,0x65,0x00,0x00,0xc6,0x71,0x00,0x00,
        0xc6,0x8e,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,
        0x00,0x1d,0x00,0x00,0xc6,0x90,0x00,0x00,0xc6,0x96,0x00,0x00,0xc6,0xb6,0x00,0x03,
        0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x20,0x00,0x00,
        0xc6,0xb8,0x00,0x00,0xc6,0xbe,0x00,0x00,0xc6,0xde,0x00,0x03,0x00,0x06,0x00,0x01,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,0x00,0x20,0x00,0x00,0xc6,0xe0,0x00,0x00,
        0xc6,0xeb,0x00,0x00,0xc7,0x08,0x00,0x03,0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x0b,0x00,0x1d,0x00,0x00,0xc7,0x0a,0x00,0x00,0xc7,0x15,0x00,0x00,
        0xc7,0x35,0x00,0x03,0x00,0x06,0x00,0x01,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0b,
        0x00,0x20,0x00,0x00,0xc7,0xed,0x00,0x00,0xc7,0xf9,0x00,0x00,0xc8,0x0c,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x13,0x00,0x00,
        0xc8,0x1b,0x00,0x00,0xc8,0x42,0x00,0x00,0xc8,0x69,0x00,0x03,0x00,0x06,0x00,0x00,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x27,0x00,0x27,0x00,0x00,0xc8,0x89,0x00,0x00,
        0xc8,0x8b,0x00,0x00,0xc8,0x9f,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x02,0x00,0x14,0x00,0x00,0xc8,0xa1,0x00,0x00,0xc8,0xae,0x00,0x00,
        0xc8,0xcb,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0d,
        0x00,0x1d,0x00,0x00,0xc9,0x1c,0x00,0x00,0xc9,0x28,0x00,0x00,0xc9,0x4c,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x24,0x00,0x00,
        0xc9,0x4e,0x00,0x00,0xc9,0x52,0x00,0x00,0xc9,0x69,0x00,0x03,0x00,0x06,0x00,0x00,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x04,0x00,0x17,0x00,0x00,0xc9,0xce,0x00,0x00,
        0xc9,0xd4,0x00,0x00,0xc9,0xef,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x06,0x00,0x1b,0x00,0x00,0xc9,0xf1,0x00,0x00,0xc9,0xf7,0x00,0x00,
        0xca,0x12,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x06,
        0x00,0x1b,0x00,0x00,0xca,0x14,0x00,0x00,0xca,0x20,0x00,0x00,0xca,0x37,0x00,0x03,
        0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x17,0x00,0x00,
        0xca,0x39,0x00,0x00,0xca,0x45,0x00,0x00,0xca,0x5c,0x00,0x03,0x00,0x06,0x00,0x00,
        0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,0x00,0x17,0x00,0x00,0xca,0x69,0x00,0x00,
        0xca,0x6b,0x00,0x00,0xca,0x6c,0x00,0x03,0x00,0x06,0x00,0x02,0x11,0x80,0x00,0x00,
        0xff,0xff,0x00,0x02,0x00,0x01,0x00,0x00,0xca,0x6d,0x00,0x00,0xca,0x79,0x00,0x00,
        0xca,0x93,0x00,0x03,0x00,0x06,0x00,0x00,0x11,0x80,0x00,0x00,0xff,0xff,0x00,0x0c,
    ];
    for (i, &b) in src_bytes.iter().enumerate() { mem.write8(src_base + i as u32, b); }

    let rom_base: u32 = 0xe2aa00;
    let rom_bytes: &[u8] = &[
        0x02,0xfa,0x03,0x00,0x03,0x06,0x03,0x0c,0x03,0x12,0x03,0x18,0x03,0x1e,0x03,0x24,
        0x03,0x2a,0x03,0x30,0x03,0x36,0x03,0x3c,0x03,0x42,0x03,0x48,0x03,0x4e,0x03,0x54,
        0x03,0x5a,0x03,0x60,0x03,0x66,0x03,0x6c,0x03,0x72,0x03,0x78,0x03,0x7e,0x03,0x84,
        0x03,0x8a,0x03,0x90,0x03,0x96,0x03,0x9c,0x03,0xa2,0x03,0xa8,0x03,0xae,0x03,0xb4,
        0x03,0xba,0x03,0xc0,0x03,0xc6,0x03,0xcc,0x03,0xd2,0x03,0xd8,0x03,0xde,0x03,0xe4,
        0x03,0xea,0x03,0xf0,0x03,0xf6,0x03,0xfc,0x04,0x02,0x04,0x08,0x04,0x0e,0x04,0x14,
        0x04,0x1a,0x04,0x20,0x04,0x26,0x04,0x2c,0x04,0x32,0x04,0x38,0x04,0x3e,0x04,0x44,
        0x04,0x4a,0x04,0x50,0x04,0x56,0x04,0x5c,0x04,0x62,0x04,0x68,0x04,0x6e,0x04,0x74,
        0x04,0x7a,0x04,0x80,0x04,0x86,0x04,0x8c,0x04,0x92,0x04,0x98,0x04,0x9e,0x04,0xa4,
        0x04,0xaa,0x04,0xb0,0x04,0xb6,0x04,0xbc,0x04,0xc2,0x04,0xc8,0x04,0xce,0x04,0xd4,
        0x04,0xda,0x04,0xe0,0x04,0xe6,0x04,0xec,0x04,0xf2,0x04,0xf8,0x04,0xfe,0x05,0x04,
        0x05,0x0a,0x05,0x10,0x05,0x16,0x05,0x1c,0x05,0x22,0x05,0x28,0x05,0x2e,0x05,0x34,
        0x05,0x3a,0x05,0x40,0x05,0x46,0x05,0x4c,0x05,0x52,0x05,0x58,0x05,0x5e,0x05,0x64,
        0x05,0x6a,0x05,0x70,0x05,0x76,0x05,0x7c,0x05,0x82,0x05,0x88,0x05,0x8e,0x05,0x94,
        0x05,0x9a,0x05,0xa0,0x05,0xa6,0x05,0xac,0x05,0xb2,0x05,0xb8,0x05,0xbe,0x05,0xc4,
        0x05,0xca,0x05,0xd0,0x05,0xd6,0x05,0xdc,0x05,0xe2,0x05,0xe8,0x05,0xee,0x05,0xf4,
        0x05,0xfa,0x06,0x00,0x00,0x82,0x04,0x21,0xcf,0xb6,0x0d,0xe3,0x04,0xe3,0x81,0x50,
        0xf9,0x87,0xbc,0xc3,0xcc,0x3e,0x73,0xe0,0x38,0x1f,0x84,0x42,0x00,0xcd,0x94,0x7b,
        0x26,0x0c,0x31,0x84,0x88,0x00,0x00,0x06,0x70,0x4f,0x3c,0x33,0xc7,0x3e,0x71,0xc3,
        0x0c,0x18,0x06,0x1c,0x71,0xcf,0x1e,0xf3,0xef,0x9e,0x89,0xc0,0x92,0x42,0x28,0x9c,
        0xf1,0xcf,0x1e,0xfa,0x28,0xa2,0x8a,0x2f,0x9e,0xc1,0xe2,0x00,0x60,0x08,0x00,0x08,
        0x01,0x80,0x80,0x01,0x20,0x60,0x00,0x00,0x00,0x00,0x00,0x20,0x00,0x00,0x00,0x00,
        0x0e,0x31,0xc4,0x00,0x79,0x41,0x08,0x51,0x02,0x00,0x21,0x44,0x14,0x21,0x05,0x08,
        0x20,0x07,0x88,0x51,0x02,0x10,0x51,0x45,0x04,0x1a,0x2f,0x06,0x10,0x41,0x04,0xf1,
        0xe7,0x1c,0x60,0x00,0x30,0xc0,0xc3,0x6c,0x69,0xa3,0x42,0x01,0xe4,0x1a,0x69,0x41,
        0x08,0x69,0xe7,0xbd,0x4b,0xa9,0xbc,0x7b,0xff,0x1c,0x7b,0xfc,0x1e,0xf3,0x0d,0x8e,
        0xf9,0xbf,0xb6,0xf9,0xcd,0x5e,0x3b,0xe0,0x3e,0xd8,0xc0,0x00,0x01,0xcf,0xc0,0xf8,
        0xe0,0x00,0x71,0xc7,0x0c,0x18,0x83,0x8c,0x78,0x86,0x06,0x0c,0xc2,0x1a,0x30,0xc0,
        0x00,0x71,0xc7,0x3e,0x01,0xc2,0x06,0x62,0xaf,0x2a,0x1a,0x17,0x86,0x82,0x01,0x50,
        0xc8,0x80,0x84,0xc2,0x0c,0x02,0x53,0x67,0x20,0x3f,0x42,0xf4,0x00,0xcd,0xbe,0xa3,
        0x4d,0x0c,0x60,0xc3,0x08,0x00,0x00,0x0c,0x98,0xc0,0x82,0x52,0x08,0x02,0x8a,0x23,
        0x0c,0x31,0xe3,0x26,0x8a,0x28,0xa0,0x8a,0x08,0x20,0x88,0x80,0x94,0x43,0x6c,0xa2,
        0x8a,0x28,0xa0,0x22,0x28,0xa2,0x52,0x21,0x18,0x60,0x67,0x00,0x61,0xcf,0x1c,0x79,
        0xc2,0x1e,0xb1,0x81,0x24,0x21,0x4f,0x1c,0xf1,0xe7,0x0e,0x72,0x28,0xa2,0x4a,0x27,
        0x8c,0x30,0xce,0x88,0x80,0x02,0x14,0x00,0x80,0x1e,0x50,0x02,0x00,0x50,0x80,0x00,
        0xfb,0xca,0x14,0x00,0x85,0x08,0x00,0x00,0x0e,0x23,0x6d,0x88,0x20,0x82,0x08,0x00,
        0x00,0xa2,0x00,0x00,0x30,0xc0,0x06,0xf6,0xb2,0xc4,0x8c,0x72,0xc2,0x2c,0xb0,0x02,
        0x1c,0xeb,0x38,0xd7,0x01,0x2d,0x8c,0x08,0x61,0x8c,0x31,0xbd,0x86,0x1b,0xe7,0xc6,
        0xd9,0xb9,0xb6,0x18,0x6d,0x56,0x18,0x6f,0xe6,0xd8,0xa2,0x16,0x6b,0x66,0xfe,0x61,
        0xc6,0xbe,0x73,0x6d,0x9a,0x21,0xc4,0x12,0x03,0xe1,0x98,0x10,0xc0,0x2c,0x49,0xe0,
        0x07,0x68,0x63,0x00,0x03,0x62,0x3b,0xdf,0x6e,0x1c,0xb2,0x97,0x84,0xde,0xe1,0x50,
        0xc8,0x8f,0xbe,0xc3,0xef,0x8e,0x73,0x20,0xb7,0x60,0x62,0x94,0x00,0xc9,0x14,0x70,
        0x86,0x18,0x60,0xc7,0xbe,0x01,0xe0,0x18,0xa8,0x47,0x1c,0x93,0xcf,0x04,0x71,0xe0,
        0x00,0x60,0x01,0x8c,0xbb,0xef,0x20,0x8b,0xcf,0x26,0xf8,0x80,0x98,0x42,0xaa,0xa2,
        0xf2,0x2f,0x1c,0x22,0x28,0xaa,0x21,0x42,0x18,0x30,0x6d,0x80,0x30,0x28,0xa0,0x8b,
        0xe7,0xa2,0xc8,0x81,0x38,0x23,0xe8,0xa2,0x8a,0x24,0x98,0x22,0x28,0xaa,0x32,0x21,
        0x18,0x30,0x6b,0x9c,0x82,0x27,0x1c,0x71,0xc7,0x20,0x71,0xc7,0x18,0x21,0x87,0x1c,
        0x80,0xef,0x9c,0x71,0xc8,0xa2,0x89,0xc8,0x98,0x71,0xcf,0x1e,0x71,0x87,0x22,0xf1,
        0x2f,0xa2,0x61,0xe7,0xb6,0xcc,0xcd,0x9b,0x71,0xc5,0x96,0xba,0xe7,0x1c,0x70,0x00,
        0x08,0xea,0xdb,0x55,0x49,0x27,0x0c,0x18,0x6d,0x8c,0x19,0xbd,0x86,0x18,0x66,0x46,
        0xd9,0xbd,0x9c,0xd8,0x6f,0x56,0x18,0x66,0xf6,0x71,0xc7,0x2d,0xd3,0xc6,0x54,0x33,
        0x66,0x8c,0xab,0xed,0x9c,0x72,0xa7,0x92,0x78,0x86,0x06,0x30,0xcf,0x80,0x30,0xc3,
        0x04,0x68,0xc1,0x00,0x00,0x8d,0x86,0x62,0xac,0xaa,0xe2,0xdf,0xdc,0x93,0xa3,0x58,
        0xd9,0xcc,0x06,0xd8,0x69,0x8c,0xdb,0xef,0xa4,0x40,0x21,0x68,0x00,0xc0,0x3e,0x29,
        0x6e,0x80,0x60,0xc3,0x08,0x30,0x03,0x30,0xc8,0x48,0x02,0xf8,0x28,0x88,0x88,0x23,
        0x0c,0x31,0xe3,0x0c,0xb2,0x28,0xa0,0x8a,0x08,0x22,0x88,0x88,0x94,0x42,0x29,0xa2,
        0x82,0x2a,0x02,0x22,0x25,0x36,0x50,0x84,0x18,0x18,0x60,0x00,0x03,0xe8,0xa0,0x8a,
        0x02,0x1e,0x88,0x81,0x24,0x22,0xa8,0xa2,0x8a,0x24,0x06,0x22,0x25,0x2a,0x31,0xe2,
        0x0c,0x30,0xc1,0x32,0x82,0x2f,0x82,0x08,0x20,0xa0,0xfb,0xef,0x88,0x20,0x88,0xa2,
        0xf3,0x8a,0x22,0x8a,0x28,0xa2,0x7a,0x28,0x8e,0x20,0x8d,0x88,0x08,0x88,0xa2,0x89,
        0xa7,0x9c,0x61,0x00,0x8b,0x14,0xc6,0xf6,0x0a,0x26,0x9a,0xa2,0xc8,0xa2,0x88,0x00,
        0x08,0x6a,0xf8,0xc0,0x4b,0xad,0x8c,0x38,0x6d,0x8c,0x19,0xbd,0x80,0x18,0x66,0x46,
        0xd9,0xb1,0x8e,0xd0,0x6c,0x56,0x18,0x66,0xc6,0x31,0xcd,0xad,0xd3,0x66,0x14,0x63,
        0x66,0x8c,0xab,0x65,0x36,0xaa,0xa4,0x12,0x00,0x00,0x00,0x30,0xc0,0x1a,0x00,0x03,
        0x34,0x69,0xe7,0x00,0x00,0x87,0x04,0x21,0xc9,0xb6,0x42,0x10,0x3c,0x18,0xe7,0x5c,
        0xd9,0xcc,0x06,0xf8,0x6d,0x8c,0xd8,0x67,0x3c,0x71,0xee,0xf0,0x00,0x00,0x14,0xf2,
        0x6d,0x00,0x31,0x84,0x88,0x30,0x03,0x20,0x70,0x4f,0xbc,0x13,0xc7,0x08,0x71,0xc3,
        0x04,0x18,0x06,0x00,0x82,0x2f,0x1e,0xf3,0xe8,0x1e,0x89,0xc7,0x12,0x7a,0x28,0x9c,
        0x81,0xc9,0xbc,0x21,0xe2,0x22,0x88,0x8f,0x9e,0x09,0xe0,0x00,0x01,0xef,0x1c,0x79,
        0xc2,0x02,0x89,0xc1,0x22,0x72,0x28,0x9c,0xf1,0xe4,0x1c,0x11,0xe2,0x36,0x48,0x27,
        0x8e,0x31,0xc0,0x3e,0x7a,0x28,0x3e,0xfb,0xef,0x9e,0x82,0x08,0x08,0x20,0x8f,0xbe,
        0x81,0xeb,0xa2,0x8a,0x28,0xa2,0x0a,0x28,0x84,0x79,0xcf,0x08,0xf8,0x88,0xa2,0x89,
        0x60,0x00,0xc9,0x00,0x86,0x3c,0xc3,0x6c,0xfa,0x24,0x8c,0x79,0xef,0xbe,0x88,0x00,
        0x08,0x2b,0x1a,0x40,0x48,0x2c,0xbe,0x68,0x6d,0x8c,0x19,0xbf,0x80,0xf1,0xe6,0xde,
        0x73,0xff,0xbe,0xc0,0x6f,0xf6,0x18,0x67,0xc6,0x32,0x88,0x9a,0x6b,0xc6,0x14,0xc1,
        0xc7,0xcc,0x71,0xcd,0xb6,0x71,0xc3,0x92,0x7b,0xe7,0x9e,0x30,0x82,0x2c,0x00,0x00,
        0x1c,0x00,0x00,0x00,0x00,0x82,0x00,0x00,0x00,0x00,0x01,0xe3,0x18,0x10,0xb6,0x4c,
        0xf9,0xcf,0xbe,0x1b,0xef,0x8c,0xf8,0x60,0x07,0x58,0xac,0x00,0x00,0xc0,0x00,0x20,
        0x06,0x80,0x00,0x00,0x00,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x08,0x00,0x00,0x0c,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x3e,0x00,0x00,0x00,0x00,
        0x00,0x3c,0x00,0x0e,0x00,0x00,0x00,0x00,0x80,0x20,0x00,0x00,0x00,0x00,0x03,0xc0,
        0x00,0x30,0x00,0x00,0xc1,0xe7,0x1e,0x79,0xe7,0xb8,0x71,0xc7,0x1c,0x71,0xc8,0xa2,
        0xf8,0x00,0x1c,0x71,0xc7,0x9e,0xf1,0xc7,0x80,0x00,0x8c,0x30,0x79,0xc7,0x1e,0x89,
        0x2f,0xbe,0x70,0x00,0x0f,0x04,0xc0,0x00,0x79,0xcb,0x10,0x00,0x08,0xa2,0x70,0x00,
        0x00,0x29,0xe7,0x80,0x10,0xc0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0xc0,0x00,0x00,0x00,0x00,0x00,0x01,0x80,0x00,0x03,0x06,0x00,0xf8,
        0x0c,0x08,0x70,0x00,0x1c,0xc0,0x80,0x00,0x00,0x00,0x00,0x33,0x00,0x00,0x00,0x00,
    ];
    for (i, &b) in rom_bytes.iter().enumerate() { mem.write8(rom_base + i as u32, b); }

    let dst_base: u32 = 0xf8f00;
    let dst_before: &[u8] = &[
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x08,0x00,0xf8,0x00,0x08,0x00,0x08,0x00,
        0x00,0x26,0x00,0x26,0x00,0x26,0x00,0x26,0x30,0x00,0x3f,0xff,0x30,0x00,0x30,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x08,0x38,0xf8,0x38,0x08,0x38,0x08,0x38,
        0x00,0x2c,0x00,0x2c,0x00,0x2c,0x00,0x2c,0x50,0x00,0x5f,0xff,0x50,0x00,0x50,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x08,0x44,0xf8,0x44,0x08,0x44,0x08,0x44,
        0x00,0x38,0x00,0x38,0x00,0x38,0x00,0x38,0x90,0x00,0x9f,0xff,0x90,0x00,0x90,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
        0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,0x00,0x00,0xff,0xff,0x00,0x00,0x00,0x00,
    ];
    for (i, &b) in dst_before.iter().enumerate() { mem.write8(dst_base + i as u32, b); }

    let mut bl = Blitter::new();
    // call #0: src=0x000000 dst=0x0f86e0 x=5 y=42 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    mem.current_call = 0;
    run_to_completion(&mut bl, &mut mem);
    // call #1: src=0x000000 dst=0x0f88c8 x=4 y=39 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88c8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    mem.current_call = 1;
    run_to_completion(&mut bl, &mut mem);
    // call #2: src=0x000000 dst=0x0f8ab0 x=3 y=36 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    mem.current_call = 2;
    run_to_completion(&mut bl, &mut mem);
    // call #3: src=0x000000 dst=0x0f8c98 x=2 y=33 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c98);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    mem.current_call = 3;
    run_to_completion(&mut bl, &mut mem);
    // call #4: src=0x000000 dst=0x0f8e80 x=1 y=30 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e80);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    mem.current_call = 4;
    run_to_completion(&mut bl, &mut mem);
    // call #5: src=0x000000 dst=0x0f90e0 x=5 y=26 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    mem.current_call = 5;
    run_to_completion(&mut bl, &mut mem);
    // call #6: src=0x000000 dst=0x0f92c8 x=4 y=23 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92c8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    mem.current_call = 6;
    run_to_completion(&mut bl, &mut mem);
    // call #7: src=0x000000 dst=0x0f94b0 x=3 y=20 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    mem.current_call = 7;
    run_to_completion(&mut bl, &mut mem);
    // call #8: src=0x000000 dst=0x0f9698 x=2 y=17 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9698);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    mem.current_call = 8;
    run_to_completion(&mut bl, &mut mem);
    // call #9: src=0x000000 dst=0x0f9880 x=1 y=14 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9880);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    mem.current_call = 9;
    run_to_completion(&mut bl, &mut mem);
    // call #10: src=0x000000 dst=0x0f9ae0 x=5 y=10 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    mem.current_call = 10;
    run_to_completion(&mut bl, &mut mem);
    // call #11: src=0x000000 dst=0x0f9cc8 x=4 y=7 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cc8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    mem.current_call = 11;
    run_to_completion(&mut bl, &mut mem);
    // call #12: src=0x000000 dst=0x0f9eb0 x=3 y=4 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    mem.current_call = 12;
    run_to_completion(&mut bl, &mut mem);
    // call #13: src=0x000000 dst=0x0fa098 x=2 y=1 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa098);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    mem.current_call = 13;
    run_to_completion(&mut bl, &mut mem);
    // call #14: src=0x000000 dst=0x0fa120 x=5 y=0 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa120);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 14;
    run_to_completion(&mut bl, &mut mem);
    // call #15: src=0x000000 dst=0x0f86e2 x=5 y=42 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    mem.current_call = 15;
    run_to_completion(&mut bl, &mut mem);
    // call #16: src=0x000000 dst=0x0f88ca x=4 y=39 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88ca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    mem.current_call = 16;
    run_to_completion(&mut bl, &mut mem);
    // call #17: src=0x000000 dst=0x0f8ab2 x=3 y=36 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    mem.current_call = 17;
    run_to_completion(&mut bl, &mut mem);
    // call #18: src=0x000000 dst=0x0f8c9a x=2 y=33 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c9a);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    mem.current_call = 18;
    run_to_completion(&mut bl, &mut mem);
    // call #19: src=0x000000 dst=0x0f8e82 x=1 y=30 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e82);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    mem.current_call = 19;
    run_to_completion(&mut bl, &mut mem);
    // call #20: src=0x000000 dst=0x0f90e2 x=5 y=26 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    mem.current_call = 20;
    run_to_completion(&mut bl, &mut mem);
    // call #21: src=0x000000 dst=0x0f92ca x=4 y=23 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    mem.current_call = 21;
    run_to_completion(&mut bl, &mut mem);
    // call #22: src=0x000000 dst=0x0f94b2 x=3 y=20 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    mem.current_call = 22;
    run_to_completion(&mut bl, &mut mem);
    // call #23: src=0x000000 dst=0x0f969a x=2 y=17 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f969a);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    mem.current_call = 23;
    run_to_completion(&mut bl, &mut mem);
    // call #24: src=0x000000 dst=0x0f9882 x=1 y=14 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9882);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    mem.current_call = 24;
    run_to_completion(&mut bl, &mut mem);
    // call #25: src=0x000000 dst=0x0f9ae2 x=5 y=10 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    mem.current_call = 25;
    run_to_completion(&mut bl, &mut mem);
    // call #26: src=0x000000 dst=0x0f9cca x=4 y=7 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    mem.current_call = 26;
    run_to_completion(&mut bl, &mut mem);
    // call #27: src=0x000000 dst=0x0f9eb2 x=3 y=4 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    mem.current_call = 27;
    run_to_completion(&mut bl, &mut mem);
    // call #28: src=0x000000 dst=0x0fa09a x=2 y=1 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa09a);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    mem.current_call = 28;
    run_to_completion(&mut bl, &mut mem);
    // call #29: src=0x000000 dst=0x0fa122 x=5 y=0 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa122);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 29;
    run_to_completion(&mut bl, &mut mem);
    // call #30: src=0x000000 dst=0x0f86e4 x=5 y=42 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    mem.current_call = 30;
    run_to_completion(&mut bl, &mut mem);
    // call #31: src=0x000000 dst=0x0f88cc x=4 y=39 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88cc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    mem.current_call = 31;
    run_to_completion(&mut bl, &mut mem);
    // call #32: src=0x000000 dst=0x0f8ab4 x=3 y=36 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    mem.current_call = 32;
    run_to_completion(&mut bl, &mut mem);
    // call #33: src=0x000000 dst=0x0f8c9c x=2 y=33 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c9c);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    mem.current_call = 33;
    run_to_completion(&mut bl, &mut mem);
    // call #34: src=0x000000 dst=0x0f8e84 x=1 y=30 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e84);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    mem.current_call = 34;
    run_to_completion(&mut bl, &mut mem);
    // call #35: src=0x000000 dst=0x0f90e4 x=5 y=26 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    mem.current_call = 35;
    run_to_completion(&mut bl, &mut mem);
    // call #36: src=0x000000 dst=0x0f92cc x=4 y=23 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92cc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    mem.current_call = 36;
    run_to_completion(&mut bl, &mut mem);
    // call #37: src=0x000000 dst=0x0f94b4 x=3 y=20 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    mem.current_call = 37;
    run_to_completion(&mut bl, &mut mem);
    // call #38: src=0x000000 dst=0x0f969c x=2 y=17 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f969c);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    mem.current_call = 38;
    run_to_completion(&mut bl, &mut mem);
    // call #39: src=0x000000 dst=0x0f9884 x=1 y=14 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9884);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    mem.current_call = 39;
    run_to_completion(&mut bl, &mut mem);
    // call #40: src=0x000000 dst=0x0f9ae4 x=5 y=10 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    mem.current_call = 40;
    run_to_completion(&mut bl, &mut mem);
    // call #41: src=0x000000 dst=0x0f9ccc x=4 y=7 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ccc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    mem.current_call = 41;
    run_to_completion(&mut bl, &mut mem);
    // call #42: src=0x000000 dst=0x0f9eb4 x=3 y=4 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    mem.current_call = 42;
    run_to_completion(&mut bl, &mut mem);
    // call #43: src=0x000000 dst=0x0fa09c x=2 y=1 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa09c);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    mem.current_call = 43;
    run_to_completion(&mut bl, &mut mem);
    // call #44: src=0x000000 dst=0x0fa124 x=5 y=0 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa124);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 44;
    run_to_completion(&mut bl, &mut mem);
    // call #45: src=0x000000 dst=0x0f86e6 x=5 y=42 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f86e6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 42);
    mem.current_call = 45;
    run_to_completion(&mut bl, &mut mem);
    // call #46: src=0x000000 dst=0x0f88ce x=4 y=39 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f88ce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 39);
    mem.current_call = 46;
    run_to_completion(&mut bl, &mut mem);
    // call #47: src=0x000000 dst=0x0f8ab6 x=3 y=36 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8ab6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 36);
    mem.current_call = 47;
    run_to_completion(&mut bl, &mut mem);
    // call #48: src=0x000000 dst=0x0f8c9e x=2 y=33 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8c9e);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 33);
    mem.current_call = 48;
    run_to_completion(&mut bl, &mut mem);
    // call #49: src=0x000000 dst=0x0f8e86 x=1 y=30 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f8e86);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 30);
    mem.current_call = 49;
    run_to_completion(&mut bl, &mut mem);
    // call #50: src=0x000000 dst=0x0f90e6 x=5 y=26 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f90e6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 26);
    mem.current_call = 50;
    run_to_completion(&mut bl, &mut mem);
    // call #51: src=0x000000 dst=0x0f92ce x=4 y=23 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 23);
    mem.current_call = 51;
    run_to_completion(&mut bl, &mut mem);
    // call #52: src=0x000000 dst=0x0f94b6 x=3 y=20 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f94b6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 20);
    mem.current_call = 52;
    run_to_completion(&mut bl, &mut mem);
    // call #53: src=0x000000 dst=0x0f969e x=2 y=17 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f969e);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 17);
    mem.current_call = 53;
    run_to_completion(&mut bl, &mut mem);
    // call #54: src=0x000000 dst=0x0f9886 x=1 y=14 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 7);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9886);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 14);
    mem.current_call = 54;
    run_to_completion(&mut bl, &mut mem);
    // call #55: src=0x000000 dst=0x0f9ae6 x=5 y=10 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 10);
    mem.current_call = 55;
    run_to_completion(&mut bl, &mut mem);
    // call #56: src=0x000000 dst=0x0f9cce x=4 y=7 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 7);
    mem.current_call = 56;
    run_to_completion(&mut bl, &mut mem);
    // call #57: src=0x000000 dst=0x0f9eb6 x=3 y=4 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 4);
    mem.current_call = 57;
    run_to_completion(&mut bl, &mut mem);
    // call #58: src=0x000000 dst=0x0fa09e x=2 y=1 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 4);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa09e);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 1);
    mem.current_call = 58;
    run_to_completion(&mut bl, &mut mem);
    // call #59: src=0x000000 dst=0x0fa126 x=5 y=0 hop=1 op=0x0 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=0 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x0);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 5);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (0i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x000000);
    wl(&mut bl, reg::DST_ADDR, 0x0fa126);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 59;
    run_to_completion(&mut bl, &mut mem);
    // call #60: src=0x00d778 dst=0x0f9a58 x=3 y=32 hop=2 op=0x7 skew=0x00 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a58);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 60;
    run_to_completion(&mut bl, &mut mem);
    // call #61: src=0x00d758 dst=0x0f9730 x=2 y=27 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d758);
    wl(&mut bl, reg::DST_ADDR, 0x0f9730);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 61;
    run_to_completion(&mut bl, &mut mem);
    // call #62: src=0x00d742 dst=0x0f9408 x=1 y=22 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d742);
    wl(&mut bl, reg::DST_ADDR, 0x0f9408);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 62;
    run_to_completion(&mut bl, &mut mem);
    // call #63: src=0x00d72e dst=0x0f9058 x=3 y=16 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d72e);
    wl(&mut bl, reg::DST_ADDR, 0x0f9058);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 63;
    run_to_completion(&mut bl, &mut mem);
    // call #64: src=0x00d718 dst=0x0f8d30 x=2 y=11 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d718);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d30);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 64;
    run_to_completion(&mut bl, &mut mem);
    // call #65: src=0x00d702 dst=0x0f8a08 x=1 y=6 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d702);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a08);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 65;
    run_to_completion(&mut bl, &mut mem);
    // call #66: src=0x00d6ee dst=0x0f8658 x=3 y=0 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6ee);
    wl(&mut bl, reg::DST_ADDR, 0x0f8658);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 66;
    run_to_completion(&mut bl, &mut mem);
    // call #67: src=0x00d778 dst=0x0f9a5a x=3 y=32 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 67;
    run_to_completion(&mut bl, &mut mem);
    // call #68: src=0x00d762 dst=0x0f9732 x=2 y=27 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d762);
    wl(&mut bl, reg::DST_ADDR, 0x0f9732);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 68;
    run_to_completion(&mut bl, &mut mem);
    // call #69: src=0x00d74c dst=0x0f940a x=1 y=22 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d74c);
    wl(&mut bl, reg::DST_ADDR, 0x0f940a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 69;
    run_to_completion(&mut bl, &mut mem);
    // call #70: src=0x00d738 dst=0x0f905a x=3 y=16 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d738);
    wl(&mut bl, reg::DST_ADDR, 0x0f905a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 70;
    run_to_completion(&mut bl, &mut mem);
    // call #71: src=0x00d722 dst=0x0f8d32 x=2 y=11 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d722);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d32);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 71;
    run_to_completion(&mut bl, &mut mem);
    // call #72: src=0x00d70c dst=0x0f8a0a x=1 y=6 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d70c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 72;
    run_to_completion(&mut bl, &mut mem);
    // call #73: src=0x00d6f8 dst=0x0f865a x=3 y=0 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f865a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 73;
    run_to_completion(&mut bl, &mut mem);
    // call #74: src=0x00d778 dst=0x0f9a5c x=3 y=32 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 74;
    run_to_completion(&mut bl, &mut mem);
    // call #75: src=0x00d762 dst=0x0f9734 x=2 y=27 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d762);
    wl(&mut bl, reg::DST_ADDR, 0x0f9734);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 75;
    run_to_completion(&mut bl, &mut mem);
    // call #76: src=0x00d74c dst=0x0f940c x=1 y=22 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d74c);
    wl(&mut bl, reg::DST_ADDR, 0x0f940c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 76;
    run_to_completion(&mut bl, &mut mem);
    // call #77: src=0x00d738 dst=0x0f905c x=3 y=16 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d738);
    wl(&mut bl, reg::DST_ADDR, 0x0f905c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 77;
    run_to_completion(&mut bl, &mut mem);
    // call #78: src=0x00d722 dst=0x0f8d34 x=2 y=11 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d722);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d34);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 78;
    run_to_completion(&mut bl, &mut mem);
    // call #79: src=0x00d70c dst=0x0f8a0c x=1 y=6 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d70c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 79;
    run_to_completion(&mut bl, &mut mem);
    // call #80: src=0x00d6f8 dst=0x0f865c x=3 y=0 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f865c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 80;
    run_to_completion(&mut bl, &mut mem);
    // call #81: src=0x00d778 dst=0x0f9a5e x=3 y=32 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 81;
    run_to_completion(&mut bl, &mut mem);
    // call #82: src=0x00d762 dst=0x0f9736 x=2 y=27 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d762);
    wl(&mut bl, reg::DST_ADDR, 0x0f9736);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 82;
    run_to_completion(&mut bl, &mut mem);
    // call #83: src=0x00d74c dst=0x0f940e x=1 y=22 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d74c);
    wl(&mut bl, reg::DST_ADDR, 0x0f940e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 83;
    run_to_completion(&mut bl, &mut mem);
    // call #84: src=0x00d738 dst=0x0f905e x=3 y=16 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d738);
    wl(&mut bl, reg::DST_ADDR, 0x0f905e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 84;
    run_to_completion(&mut bl, &mut mem);
    // call #85: src=0x00d722 dst=0x0f8d36 x=2 y=11 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d722);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d36);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 85;
    run_to_completion(&mut bl, &mut mem);
    // call #86: src=0x00d70c dst=0x0f8a0e x=1 y=6 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d70c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 86;
    run_to_completion(&mut bl, &mut mem);
    // call #87: src=0x00d6f8 dst=0x0f865e x=3 y=0 hop=2 op=0x7 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x7);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f865e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 87;
    run_to_completion(&mut bl, &mut mem);
    // call #88: src=0x00d6f8 dst=0x0f9ae0 x=5 y=8 hop=1 op=0x3 skew=0x44 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    mem.current_call = 88;
    run_to_completion(&mut bl, &mut mem);
    // call #89: src=0x00d6f8 dst=0x0f9cc8 x=4 y=5 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cc8);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    mem.current_call = 89;
    run_to_completion(&mut bl, &mut mem);
    // call #90: src=0x00d6f8 dst=0x0f9eb0 x=3 y=2 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    mem.current_call = 90;
    run_to_completion(&mut bl, &mut mem);
    // call #91: src=0x00d6f8 dst=0x0f9fe0 x=5 y=0 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe0);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 91;
    run_to_completion(&mut bl, &mut mem);
    // call #92: src=0x00d6f8 dst=0x0f9ae2 x=5 y=8 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    mem.current_call = 92;
    run_to_completion(&mut bl, &mut mem);
    // call #93: src=0x00d6f8 dst=0x0f9cca x=4 y=5 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cca);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    mem.current_call = 93;
    run_to_completion(&mut bl, &mut mem);
    // call #94: src=0x00d6f8 dst=0x0f9eb2 x=3 y=2 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    mem.current_call = 94;
    run_to_completion(&mut bl, &mut mem);
    // call #95: src=0x00d6f8 dst=0x0f9fe2 x=5 y=0 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe2);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 95;
    run_to_completion(&mut bl, &mut mem);
    // call #96: src=0x00d6f8 dst=0x0f9ae4 x=5 y=8 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    mem.current_call = 96;
    run_to_completion(&mut bl, &mut mem);
    // call #97: src=0x00d6f8 dst=0x0f9ccc x=4 y=5 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ccc);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    mem.current_call = 97;
    run_to_completion(&mut bl, &mut mem);
    // call #98: src=0x00d6f8 dst=0x0f9eb4 x=3 y=2 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    mem.current_call = 98;
    run_to_completion(&mut bl, &mut mem);
    // call #99: src=0x00d6f8 dst=0x0f9fe4 x=5 y=0 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe4);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 99;
    run_to_completion(&mut bl, &mut mem);
    // call #100: src=0x00d6f8 dst=0x0f9ae6 x=5 y=8 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 8);
    mem.current_call = 100;
    run_to_completion(&mut bl, &mut mem);
    // call #101: src=0x00d6f8 dst=0x0f9cce x=4 y=5 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 14);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9cce);
    ww(&mut bl, reg::X_COUNT, 4);
    ww(&mut bl, reg::Y_COUNT, 5);
    mem.current_call = 101;
    run_to_completion(&mut bl, &mut mem);
    // call #102: src=0x00d6f8 dst=0x0f9eb6 x=3 y=2 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 1);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 2);
    mem.current_call = 102;
    run_to_completion(&mut bl, &mut mem);
    // call #103: src=0x00d6f8 dst=0x0f9fe6 x=5 y=0 hop=1 op=0x3 skew=0x00 endmask=[0xffff,0xffff,0xff00] sxi=0 syi=-2 dxi=8 dyi=128
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 1);
    bl.write(reg::OP, 0x3);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 3);
    ww(&mut bl, reg::SRC_X_INC, (0i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (128i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xffff);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xff00);
    wl(&mut bl, reg::SRC_ADDR, 0x00d6f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9fe6);
    ww(&mut bl, reg::X_COUNT, 5);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 103;
    run_to_completion(&mut bl, &mut mem);
    // call #104: src=0x00d7f8 dst=0x0f9a58 x=3 y=32 hop=2 op=0x4 skew=0x00 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x00);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a58);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 104;
    run_to_completion(&mut bl, &mut mem);
    // call #105: src=0x00d7d8 dst=0x0f9730 x=2 y=27 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7d8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9730);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 105;
    run_to_completion(&mut bl, &mut mem);
    // call #106: src=0x00d7c2 dst=0x0f9408 x=1 y=22 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7c2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9408);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 106;
    run_to_completion(&mut bl, &mut mem);
    // call #107: src=0x00d7ae dst=0x0f9058 x=3 y=16 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7ae);
    wl(&mut bl, reg::DST_ADDR, 0x0f9058);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 107;
    run_to_completion(&mut bl, &mut mem);
    // call #108: src=0x00d798 dst=0x0f8d30 x=2 y=11 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d798);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d30);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 108;
    run_to_completion(&mut bl, &mut mem);
    // call #109: src=0x00d782 dst=0x0f8a08 x=1 y=6 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d782);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a08);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 109;
    run_to_completion(&mut bl, &mut mem);
    // call #110: src=0x00d76e dst=0x0f8658 x=3 y=0 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d76e);
    wl(&mut bl, reg::DST_ADDR, 0x0f8658);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 110;
    run_to_completion(&mut bl, &mut mem);
    // call #111: src=0x00d7f8 dst=0x0f9a5a x=3 y=32 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 111;
    run_to_completion(&mut bl, &mut mem);
    // call #112: src=0x00d7e2 dst=0x0f9732 x=2 y=27 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7e2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9732);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 112;
    run_to_completion(&mut bl, &mut mem);
    // call #113: src=0x00d7cc dst=0x0f940a x=1 y=22 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7cc);
    wl(&mut bl, reg::DST_ADDR, 0x0f940a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 113;
    run_to_completion(&mut bl, &mut mem);
    // call #114: src=0x00d7b8 dst=0x0f905a x=3 y=16 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7b8);
    wl(&mut bl, reg::DST_ADDR, 0x0f905a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 114;
    run_to_completion(&mut bl, &mut mem);
    // call #115: src=0x00d7a2 dst=0x0f8d32 x=2 y=11 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7a2);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d32);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 115;
    run_to_completion(&mut bl, &mut mem);
    // call #116: src=0x00d78c dst=0x0f8a0a x=1 y=6 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d78c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 116;
    run_to_completion(&mut bl, &mut mem);
    // call #117: src=0x00d778 dst=0x0f865a x=3 y=0 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f865a);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 117;
    run_to_completion(&mut bl, &mut mem);
    // call #118: src=0x00d7f8 dst=0x0f9a5c x=3 y=32 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 118;
    run_to_completion(&mut bl, &mut mem);
    // call #119: src=0x00d7e2 dst=0x0f9734 x=2 y=27 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7e2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9734);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 119;
    run_to_completion(&mut bl, &mut mem);
    // call #120: src=0x00d7cc dst=0x0f940c x=1 y=22 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7cc);
    wl(&mut bl, reg::DST_ADDR, 0x0f940c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 120;
    run_to_completion(&mut bl, &mut mem);
    // call #121: src=0x00d7b8 dst=0x0f905c x=3 y=16 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7b8);
    wl(&mut bl, reg::DST_ADDR, 0x0f905c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 121;
    run_to_completion(&mut bl, &mut mem);
    // call #122: src=0x00d7a2 dst=0x0f8d34 x=2 y=11 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7a2);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d34);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 122;
    run_to_completion(&mut bl, &mut mem);
    // call #123: src=0x00d78c dst=0x0f8a0c x=1 y=6 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d78c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 123;
    run_to_completion(&mut bl, &mut mem);
    // call #124: src=0x00d778 dst=0x0f865c x=3 y=0 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f865c);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 124;
    run_to_completion(&mut bl, &mut mem);
    // call #125: src=0x00d7f8 dst=0x0f9a5e x=3 y=32 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7f8);
    wl(&mut bl, reg::DST_ADDR, 0x0f9a5e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 32);
    mem.current_call = 125;
    run_to_completion(&mut bl, &mut mem);
    // call #126: src=0x00d7e2 dst=0x0f9736 x=2 y=27 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7e2);
    wl(&mut bl, reg::DST_ADDR, 0x0f9736);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 27);
    mem.current_call = 126;
    run_to_completion(&mut bl, &mut mem);
    // call #127: src=0x00d7cc dst=0x0f940e x=1 y=22 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7cc);
    wl(&mut bl, reg::DST_ADDR, 0x0f940e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 22);
    mem.current_call = 127;
    run_to_completion(&mut bl, &mut mem);
    // call #128: src=0x00d7b8 dst=0x0f905e x=3 y=16 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7b8);
    wl(&mut bl, reg::DST_ADDR, 0x0f905e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 16);
    mem.current_call = 128;
    run_to_completion(&mut bl, &mut mem);
    // call #129: src=0x00d7a2 dst=0x0f8d36 x=2 y=11 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 11);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d7a2);
    wl(&mut bl, reg::DST_ADDR, 0x0f8d36);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 11);
    mem.current_call = 129;
    run_to_completion(&mut bl, &mut mem);
    // call #130: src=0x00d78c dst=0x0f8a0e x=1 y=6 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 6);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d78c);
    wl(&mut bl, reg::DST_ADDR, 0x0f8a0e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 130;
    run_to_completion(&mut bl, &mut mem);
    // call #131: src=0x00d778 dst=0x0f865e x=3 y=0 hop=2 op=0x4 skew=0x44 endmask=[0xf000,0xffff,0x0fff] sxi=-2 syi=-2 dxi=-8 dyi=-144
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-2i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (-8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-144i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xf000);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0fff);
    wl(&mut bl, reg::SRC_ADDR, 0x00d778);
    wl(&mut bl, reg::DST_ADDR, 0x0f865e);
    ww(&mut bl, reg::X_COUNT, 3);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 131;
    run_to_completion(&mut bl, &mut mem);
    // call #132: src=0xe2aef4 dst=0x0f92c8 x=1 y=6 hop=2 op=0x4 skew=0x44 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x44);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92c8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 132;
    run_to_completion(&mut bl, &mut mem);
    // call #133: src=0xe2aa74 dst=0x0f8f08 x=1 y=0 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f08);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 133;
    run_to_completion(&mut bl, &mut mem);
    // call #134: src=0xe2aef4 dst=0x0f92ca x=1 y=6 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ca);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 134;
    run_to_completion(&mut bl, &mut mem);
    // call #135: src=0xe2aa74 dst=0x0f8f0a x=1 y=0 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f0a);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 135;
    run_to_completion(&mut bl, &mut mem);
    // call #136: src=0xe2aef4 dst=0x0f92cc x=1 y=6 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92cc);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 136;
    run_to_completion(&mut bl, &mut mem);
    // call #137: src=0xe2aa74 dst=0x0f8f0c x=1 y=0 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f0c);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 137;
    run_to_completion(&mut bl, &mut mem);
    // call #138: src=0xe2aef4 dst=0x0f92ce x=1 y=6 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef4);
    wl(&mut bl, reg::DST_ADDR, 0x0f92ce);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 138;
    run_to_completion(&mut bl, &mut mem);
    // call #139: src=0xe2aa74 dst=0x0f8f0e x=1 y=0 hop=2 op=0x4 skew=0x03 endmask=[0x007e,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x007e);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa74);
    wl(&mut bl, reg::DST_ADDR, 0x0f8f0e);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 139;
    run_to_completion(&mut bl, &mut mem);
    // call #140: src=0xe2aef6 dst=0x0f9ea8 x=1 y=6 hop=2 op=0x4 skew=0x03 endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x03);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ea8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 140;
    run_to_completion(&mut bl, &mut mem);
    // call #141: src=0xe2aa76 dst=0x0f9ae8 x=1 y=0 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 141;
    run_to_completion(&mut bl, &mut mem);
    // call #142: src=0xe2aef6 dst=0x0f9eaa x=1 y=6 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eaa);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 142;
    run_to_completion(&mut bl, &mut mem);
    // call #143: src=0xe2aa76 dst=0x0f9aea x=1 y=0 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aea);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 143;
    run_to_completion(&mut bl, &mut mem);
    // call #144: src=0xe2aef6 dst=0x0f9eac x=1 y=6 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eac);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 144;
    run_to_completion(&mut bl, &mut mem);
    // call #145: src=0xe2aa76 dst=0x0f9aec x=1 y=0 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aec);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 145;
    run_to_completion(&mut bl, &mut mem);
    // call #146: src=0xe2aef6 dst=0x0f9eae x=1 y=6 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eae);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 146;
    run_to_completion(&mut bl, &mut mem);
    // call #147: src=0xe2aa76 dst=0x0f9aee x=1 y=0 hop=2 op=0x4 skew=0x0a endmask=[0x3f00,0xffff,0x0000] sxi=-6 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-6i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x3f00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aee);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 147;
    run_to_completion(&mut bl, &mut mem);
    // call #148: src=0xe2aefa dst=0x0f9ea8 x=1 y=6 hop=2 op=0x4 skew=0x0a endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0a);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ea8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 148;
    run_to_completion(&mut bl, &mut mem);
    // call #149: src=0xe2aa7a dst=0x0f9ae8 x=1 y=0 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 149;
    run_to_completion(&mut bl, &mut mem);
    // call #150: src=0xe2aefa dst=0x0f9eaa x=1 y=6 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eaa);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 150;
    run_to_completion(&mut bl, &mut mem);
    // call #151: src=0xe2aa7a dst=0x0f9aea x=1 y=0 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aea);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 151;
    run_to_completion(&mut bl, &mut mem);
    // call #152: src=0xe2aefa dst=0x0f9eac x=1 y=6 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eac);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 152;
    run_to_completion(&mut bl, &mut mem);
    // call #153: src=0xe2aa7a dst=0x0f9aec x=1 y=0 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aec);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 153;
    run_to_completion(&mut bl, &mut mem);
    // call #154: src=0xe2aefa dst=0x0f9eae x=1 y=6 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aefa);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eae);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 154;
    run_to_completion(&mut bl, &mut mem);
    // call #155: src=0xe2aa7a dst=0x0f9aee x=1 y=0 hop=2 op=0x4 skew=0x02 endmask=[0x00fc,0xffff,0x0000] sxi=2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x00fc);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa7a);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aee);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 155;
    run_to_completion(&mut bl, &mut mem);
    // call #156: src=0xe2af02 dst=0x0f9ea8 x=2 y=6 hop=2 op=0x4 skew=0x02 endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x02);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ea8);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 156;
    run_to_completion(&mut bl, &mut mem);
    // call #157: src=0xe2aa8e dst=0x0f9ae8 x=2 y=0 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa8e);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ae8);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 157;
    run_to_completion(&mut bl, &mut mem);
    // call #158: src=0xe2af02 dst=0x0f9eaa x=2 y=6 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eaa);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 158;
    run_to_completion(&mut bl, &mut mem);
    // call #159: src=0xe2aa82 dst=0x0f9aea x=2 y=0 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aea);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 159;
    run_to_completion(&mut bl, &mut mem);
    // call #160: src=0xe2af02 dst=0x0f9eac x=2 y=6 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eac);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 160;
    run_to_completion(&mut bl, &mut mem);
    // call #161: src=0xe2aa82 dst=0x0f9aec x=2 y=0 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aec);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 161;
    run_to_completion(&mut bl, &mut mem);
    // call #162: src=0xe2af02 dst=0x0f9eae x=2 y=6 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eae);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 162;
    run_to_completion(&mut bl, &mut mem);
    // call #163: src=0xe2aa82 dst=0x0f9aee x=2 y=0 hop=2 op=0x4 skew=0x4c endmask=[0x0003,0xffff,0xf000] sxi=2 syi=-192 dxi=8 dyi=-168
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-168i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0003);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0xf000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9aee);
    ww(&mut bl, reg::X_COUNT, 2);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 163;
    run_to_completion(&mut bl, &mut mem);
    // call #164: src=0xe2af00 dst=0x0f9eb0 x=1 y=6 hop=2 op=0x4 skew=0x4c endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x4c);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 164;
    run_to_completion(&mut bl, &mut mem);
    // call #165: src=0xe2aa80 dst=0x0f9af0 x=1 y=0 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 165;
    run_to_completion(&mut bl, &mut mem);
    // call #166: src=0xe2af00 dst=0x0f9eb2 x=1 y=6 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 166;
    run_to_completion(&mut bl, &mut mem);
    // call #167: src=0xe2aa80 dst=0x0f9af2 x=1 y=0 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 167;
    run_to_completion(&mut bl, &mut mem);
    // call #168: src=0xe2af00 dst=0x0f9eb4 x=1 y=6 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 168;
    run_to_completion(&mut bl, &mut mem);
    // call #169: src=0xe2aa80 dst=0x0f9af4 x=1 y=0 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 169;
    run_to_completion(&mut bl, &mut mem);
    // call #170: src=0xe2af00 dst=0x0f9eb6 x=1 y=6 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 0);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af00);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 170;
    run_to_completion(&mut bl, &mut mem);
    // call #171: src=0xe2aa80 dst=0x0f9af6 x=1 y=0 hop=2 op=0x4 skew=0x0e endmask=[0x0fc0,0xffff,0x0000] sxi=-2 syi=-192 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (-2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-192i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x0fc0);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa80);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 171;
    run_to_completion(&mut bl, &mut mem);
    // call #172: src=0xe2af02 dst=0x0f9eb0 x=1 y=6 hop=2 op=0x4 skew=0x0e endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x0e);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 172;
    run_to_completion(&mut bl, &mut mem);
    // call #173: src=0xe2aa76 dst=0x0f9af0 x=1 y=0 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af0);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 173;
    run_to_completion(&mut bl, &mut mem);
    // call #174: src=0xe2af02 dst=0x0f9eb2 x=1 y=6 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 174;
    run_to_completion(&mut bl, &mut mem);
    // call #175: src=0xe2aa82 dst=0x0f9af2 x=1 y=0 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af2);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 175;
    run_to_completion(&mut bl, &mut mem);
    // call #176: src=0xe2af02 dst=0x0f9eb4 x=1 y=6 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 176;
    run_to_completion(&mut bl, &mut mem);
    // call #177: src=0xe2aa82 dst=0x0f9af4 x=1 y=0 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af4);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 177;
    run_to_completion(&mut bl, &mut mem);
    // call #178: src=0xe2af02 dst=0x0f9eb6 x=1 y=6 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2af02);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 178;
    run_to_completion(&mut bl, &mut mem);
    // call #179: src=0xe2aa82 dst=0x0f9af6 x=1 y=0 hop=2 op=0x4 skew=0x8c endmask=[0x003f,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0x003f);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa82);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af6);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 179;
    run_to_completion(&mut bl, &mut mem);
    // call #180: src=0xe2aef6 dst=0x0f9eb8 x=1 y=6 hop=2 op=0x4 skew=0x8c endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x8c);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eb8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 180;
    run_to_completion(&mut bl, &mut mem);
    // call #181: src=0xe2aa76 dst=0x0f9af8 x=1 y=0 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9af8);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 181;
    run_to_completion(&mut bl, &mut mem);
    // call #182: src=0xe2aef6 dst=0x0f9eba x=1 y=6 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9eba);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 182;
    run_to_completion(&mut bl, &mut mem);
    // call #183: src=0xe2aa76 dst=0x0f9afa x=1 y=0 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9afa);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 183;
    run_to_completion(&mut bl, &mut mem);
    // call #184: src=0xe2aef6 dst=0x0f9ebc x=1 y=6 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ebc);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 184;
    run_to_completion(&mut bl, &mut mem);
    // call #185: src=0xe2aa76 dst=0x0f9afc x=1 y=0 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9afc);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 185;
    run_to_completion(&mut bl, &mut mem);
    // call #186: src=0xe2aef6 dst=0x0f9ebe x=1 y=6 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 64);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aef6);
    wl(&mut bl, reg::DST_ADDR, 0x0f9ebe);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 6);
    mem.current_call = 186;
    run_to_completion(&mut bl, &mut mem);
    // call #187: src=0xe2aa76 dst=0x0f9afe x=1 y=0 hop=2 op=0x4 skew=0x82 endmask=[0xfc00,0xffff,0x0000] sxi=2 syi=-194 dxi=8 dyi=-160
    ww(&mut bl, reg::HALFTONE_BASE + 0*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 1*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 2*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 3*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 4*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 5*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 6*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 7*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 8*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 9*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 10*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 11*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 12*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 13*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 14*2, 0xffff);
    ww(&mut bl, reg::HALFTONE_BASE + 15*2, 0xffff);
    bl.write(reg::HOP, 2);
    bl.write(reg::OP, 0x4);
    bl.write(reg::SKEW, 0x82);
    bl.write(reg::CONTROL, 10);
    ww(&mut bl, reg::SRC_X_INC, (2i16) as u16);
    ww(&mut bl, reg::SRC_Y_INC, (-194i16) as u16);
    ww(&mut bl, reg::DST_X_INC, (8i16) as u16);
    ww(&mut bl, reg::DST_Y_INC, (-160i16) as u16);
    ww(&mut bl, reg::ENDMASK_1, 0xfc00);
    ww(&mut bl, reg::ENDMASK_2, 0xffff);
    ww(&mut bl, reg::ENDMASK_3, 0x0000);
    wl(&mut bl, reg::SRC_ADDR, 0xe2aa76);
    wl(&mut bl, reg::DST_ADDR, 0x0f9afe);
    ww(&mut bl, reg::X_COUNT, 1);
    ww(&mut bl, reg::Y_COUNT, 0);
    mem.current_call = 187;
    run_to_completion(&mut bl, &mut mem);

    let dst_after: &[(u32,u8)] = &[
        (0xf8f00, 0x00),
        (0xf8f01, 0x00),
        (0xf8f02, 0xff),
        (0xf8f03, 0xff),
        (0xf8f04, 0x00),
        (0xf8f05, 0x00),
        (0xf8f06, 0x00),
        (0xf8f07, 0x00),
        (0xf8f08, 0x07),
        (0xf8f09, 0xff),
        (0xf8f0a, 0xf7),
        (0xf8f0b, 0xff),
        (0xf8f0c, 0x07),
        (0xf8f0d, 0xff),
        (0xf8f0e, 0x07),
        (0xf8f0f, 0xff),
        (0xf8f10, 0xff),
        (0xf8f11, 0xd9),
        (0xf8f12, 0xff),
        (0xf8f13, 0xd9),
        (0xf8f14, 0xff),
        (0xf8f15, 0xd9),
        (0xf8f16, 0xff),
        (0xf8f17, 0xd9),
        (0xf8f18, 0xc0),
        (0xf8f19, 0x00),
        (0xf8f1a, 0xcf),
        (0xf8f1b, 0xff),
        (0xf8f1c, 0xc0),
        (0xf8f1d, 0x00),
        (0xf8f1e, 0xc0),
        (0xf8f1f, 0x00),
        (0xf8f20, 0x00),
        (0xf8f21, 0x00),
        (0xf8f22, 0xff),
        (0xf8f23, 0xff),
        (0xf8f24, 0x00),
        (0xf8f25, 0x00),
        (0xf8f26, 0x00),
        (0xf8f27, 0x00),
        (0xf8f28, 0x00),
        (0xf8f29, 0x00),
        (0xf8f2a, 0xff),
        (0xf8f2b, 0xff),
        (0xf8f2c, 0x00),
        (0xf8f2d, 0x00),
        (0xf8f2e, 0x00),
        (0xf8f2f, 0x00),
        (0xf8f30, 0x00),
        (0xf8f31, 0x00),
        (0xf8f32, 0xff),
        (0xf8f33, 0xff),
        (0xf8f34, 0x00),
        (0xf8f35, 0x00),
        (0xf8f36, 0x00),
        (0xf8f37, 0x00),
        (0xf8f38, 0x00),
        (0xf8f39, 0x00),
        (0xf8f3a, 0xff),
        (0xf8f3b, 0xff),
        (0xf8f3c, 0x00),
        (0xf8f3d, 0x00),
        (0xf8f3e, 0x00),
        (0xf8f3f, 0x00),
        (0xf8f40, 0x00),
        (0xf8f41, 0x00),
        (0xf8f42, 0xff),
        (0xf8f43, 0xff),
        (0xf8f44, 0x00),
        (0xf8f45, 0x00),
        (0xf8f46, 0x00),
        (0xf8f47, 0x00),
        (0xf8f48, 0x00),
        (0xf8f49, 0x00),
        (0xf8f4a, 0xff),
        (0xf8f4b, 0xff),
        (0xf8f4c, 0x00),
        (0xf8f4d, 0x00),
        (0xf8f4e, 0x00),
        (0xf8f4f, 0x00),
        (0xf8f50, 0x00),
        (0xf8f51, 0x00),
        (0xf8f52, 0xff),
        (0xf8f53, 0xff),
        (0xf8f54, 0x00),
        (0xf8f55, 0x00),
        (0xf8f56, 0x00),
        (0xf8f57, 0x00),
        (0xf8f58, 0x00),
        (0xf8f59, 0x00),
        (0xf8f5a, 0xff),
        (0xf8f5b, 0xff),
        (0xf8f5c, 0x00),
        (0xf8f5d, 0x00),
        (0xf8f5e, 0x00),
        (0xf8f5f, 0x00),
        (0xf8f60, 0x00),
        (0xf8f61, 0x00),
        (0xf8f62, 0xff),
        (0xf8f63, 0xff),
        (0xf8f64, 0x00),
        (0xf8f65, 0x00),
        (0xf8f66, 0x00),
        (0xf8f67, 0x00),
        (0xf8f68, 0x00),
        (0xf8f69, 0x00),
        (0xf8f6a, 0xff),
        (0xf8f6b, 0xff),
        (0xf8f6c, 0x00),
        (0xf8f6d, 0x00),
        (0xf8f6e, 0x00),
        (0xf8f6f, 0x00),
        (0xf8f70, 0x00),
        (0xf8f71, 0x00),
        (0xf8f72, 0xff),
        (0xf8f73, 0xff),
        (0xf8f74, 0x00),
        (0xf8f75, 0x00),
        (0xf8f76, 0x00),
        (0xf8f77, 0x00),
        (0xf8f78, 0x00),
        (0xf8f79, 0x00),
        (0xf8f7a, 0xff),
        (0xf8f7b, 0xff),
        (0xf8f7c, 0x00),
        (0xf8f7d, 0x00),
        (0xf8f7e, 0x00),
        (0xf8f7f, 0x00),
        (0xf8f80, 0x00),
        (0xf8f81, 0x00),
        (0xf8f82, 0xff),
        (0xf8f83, 0xff),
        (0xf8f84, 0x00),
        (0xf8f85, 0x00),
        (0xf8f86, 0x00),
        (0xf8f87, 0x00),
        (0xf8f88, 0x00),
        (0xf8f89, 0x00),
        (0xf8f8a, 0xff),
        (0xf8f8b, 0xff),
        (0xf8f8c, 0x00),
        (0xf8f8d, 0x00),
        (0xf8f8e, 0x00),
        (0xf8f8f, 0x00),
        (0xf8f90, 0x00),
        (0xf8f91, 0x00),
        (0xf8f92, 0xff),
        (0xf8f93, 0xff),
        (0xf8f94, 0x00),
        (0xf8f95, 0x00),
        (0xf8f96, 0x00),
        (0xf8f97, 0x00),
        (0xf8f98, 0x00),
        (0xf8f99, 0x00),
        (0xf8f9a, 0xff),
        (0xf8f9b, 0xff),
        (0xf8f9c, 0x00),
        (0xf8f9d, 0x00),
        (0xf8f9e, 0x00),
        (0xf8f9f, 0x00),
        (0xf8fa0, 0x00),
        (0xf8fa1, 0x00),
        (0xf8fa2, 0xff),
        (0xf8fa3, 0xff),
        (0xf8fa4, 0x00),
        (0xf8fa5, 0x00),
        (0xf8fa6, 0x00),
        (0xf8fa7, 0x00),
        (0xf8fa8, 0x07),
        (0xf8fa9, 0xc7),
        (0xf8faa, 0xf7),
        (0xf8fab, 0xc7),
        (0xf8fac, 0x07),
        (0xf8fad, 0xc7),
        (0xf8fae, 0x07),
        (0xf8faf, 0xc7),
        (0xf8fb0, 0xff),
        (0xf8fb1, 0xd3),
        (0xf8fb2, 0xff),
        (0xf8fb3, 0xd3),
        (0xf8fb4, 0xff),
        (0xf8fb5, 0xd3),
        (0xf8fb6, 0xff),
        (0xf8fb7, 0xd3),
        (0xf8fb8, 0xa0),
        (0xf8fb9, 0x00),
        (0xf8fba, 0xaf),
        (0xf8fbb, 0xff),
        (0xf8fbc, 0xa0),
        (0xf8fbd, 0x00),
        (0xf8fbe, 0xa0),
        (0xf8fbf, 0x00),
        (0xf8fc0, 0x00),
        (0xf8fc1, 0x00),
        (0xf8fc2, 0xff),
        (0xf8fc3, 0xff),
        (0xf8fc4, 0x00),
        (0xf8fc5, 0x00),
        (0xf8fc6, 0x00),
        (0xf8fc7, 0x00),
        (0xf8fc8, 0x00),
        (0xf8fc9, 0x00),
        (0xf8fca, 0xff),
        (0xf8fcb, 0xff),
        (0xf8fcc, 0x00),
        (0xf8fcd, 0x00),
        (0xf8fce, 0x00),
        (0xf8fcf, 0x00),
        (0xf8fd0, 0x00),
        (0xf8fd1, 0x00),
        (0xf8fd2, 0xff),
        (0xf8fd3, 0xff),
        (0xf8fd4, 0x00),
        (0xf8fd5, 0x00),
        (0xf8fd6, 0x00),
        (0xf8fd7, 0x00),
        (0xf8fd8, 0x00),
        (0xf8fd9, 0x00),
        (0xf8fda, 0xff),
        (0xf8fdb, 0xff),
        (0xf8fdc, 0x00),
        (0xf8fdd, 0x00),
        (0xf8fde, 0x00),
        (0xf8fdf, 0x00),
        (0xf8fe0, 0x00),
        (0xf8fe1, 0x00),
        (0xf8fe2, 0xff),
        (0xf8fe3, 0xff),
        (0xf8fe4, 0x00),
        (0xf8fe5, 0x00),
        (0xf8fe6, 0x00),
        (0xf8fe7, 0x00),
        (0xf8fe8, 0x00),
        (0xf8fe9, 0x00),
        (0xf8fea, 0xff),
        (0xf8feb, 0xff),
        (0xf8fec, 0x00),
        (0xf8fed, 0x00),
        (0xf8fee, 0x00),
        (0xf8fef, 0x00),
        (0xf8ff0, 0x00),
        (0xf8ff1, 0x00),
        (0xf8ff2, 0xff),
        (0xf8ff3, 0xff),
        (0xf8ff4, 0x00),
        (0xf8ff5, 0x00),
        (0xf8ff6, 0x00),
        (0xf8ff7, 0x00),
        (0xf8ff8, 0x00),
        (0xf8ff9, 0x00),
        (0xf8ffa, 0xff),
        (0xf8ffb, 0xff),
        (0xf8ffc, 0x00),
        (0xf8ffd, 0x00),
        (0xf8ffe, 0x00),
        (0xf8fff, 0x00),
        (0xf9000, 0x00),
        (0xf9001, 0x00),
        (0xf9002, 0xff),
        (0xf9003, 0xff),
        (0xf9004, 0x00),
        (0xf9005, 0x00),
        (0xf9006, 0x00),
        (0xf9007, 0x00),
        (0xf9008, 0x00),
        (0xf9009, 0x00),
        (0xf900a, 0xff),
        (0xf900b, 0xff),
        (0xf900c, 0x00),
        (0xf900d, 0x00),
        (0xf900e, 0x00),
        (0xf900f, 0x00),
        (0xf9010, 0x00),
        (0xf9011, 0x00),
        (0xf9012, 0xff),
        (0xf9013, 0xff),
        (0xf9014, 0x00),
        (0xf9015, 0x00),
        (0xf9016, 0x00),
        (0xf9017, 0x00),
        (0xf9018, 0x00),
        (0xf9019, 0x00),
        (0xf901a, 0xff),
        (0xf901b, 0xff),
        (0xf901c, 0x00),
        (0xf901d, 0x00),
        (0xf901e, 0x00),
        (0xf901f, 0x00),
        (0xf9020, 0x00),
        (0xf9021, 0x00),
        (0xf9022, 0xff),
        (0xf9023, 0xff),
        (0xf9024, 0x00),
        (0xf9025, 0x00),
        (0xf9026, 0x00),
        (0xf9027, 0x00),
        (0xf9028, 0x00),
        (0xf9029, 0x00),
        (0xf902a, 0xff),
        (0xf902b, 0xff),
        (0xf902c, 0x00),
        (0xf902d, 0x00),
        (0xf902e, 0x00),
        (0xf902f, 0x00),
        (0xf9030, 0x00),
        (0xf9031, 0x00),
        (0xf9032, 0xff),
        (0xf9033, 0xff),
        (0xf9034, 0x00),
        (0xf9035, 0x00),
        (0xf9036, 0x00),
        (0xf9037, 0x00),
        (0xf9038, 0x00),
        (0xf9039, 0x00),
        (0xf903a, 0xff),
        (0xf903b, 0xff),
        (0xf903c, 0x00),
        (0xf903d, 0x00),
        (0xf903e, 0x00),
        (0xf903f, 0x00),
        (0xf9040, 0x00),
        (0xf9041, 0x00),
        (0xf9042, 0xff),
        (0xf9043, 0xff),
        (0xf9044, 0x00),
        (0xf9045, 0x00),
        (0xf9046, 0x00),
        (0xf9047, 0x00),
        (0xf9048, 0x07),
        (0xf9049, 0xbb),
        (0xf904a, 0xf7),
        (0xf904b, 0xbb),
        (0xf904c, 0x07),
        (0xf904d, 0xbb),
        (0xf904e, 0x07),
        (0xf904f, 0xbb),
        (0xf9050, 0xff),
        (0xf9051, 0xc7),
        (0xf9052, 0xff),
        (0xf9053, 0xc7),
        (0xf9054, 0xff),
        (0xf9055, 0xc7),
        (0xf9056, 0xff),
        (0xf9057, 0xc7),
        (0xf9058, 0x60),
        (0xf9059, 0x00),
        (0xf905a, 0x6f),
        (0xf905b, 0xff),
        (0xf905c, 0x60),
        (0xf905d, 0x00),
        (0xf905e, 0x60),
        (0xf905f, 0x00),
        (0xf9060, 0x00),
        (0xf9061, 0x00),
        (0xf9062, 0xff),
        (0xf9063, 0xff),
        (0xf9064, 0x00),
        (0xf9065, 0x00),
        (0xf9066, 0x00),
        (0xf9067, 0x00),
        (0xf9068, 0x00),
        (0xf9069, 0x00),
        (0xf906a, 0xff),
        (0xf906b, 0xff),
        (0xf906c, 0x00),
        (0xf906d, 0x00),
        (0xf906e, 0x00),
        (0xf906f, 0x00),
        (0xf9070, 0x00),
        (0xf9071, 0x00),
        (0xf9072, 0xff),
        (0xf9073, 0xff),
        (0xf9074, 0x00),
        (0xf9075, 0x00),
        (0xf9076, 0x00),
        (0xf9077, 0x00),
        (0xf9078, 0x00),
        (0xf9079, 0x00),
        (0xf907a, 0xff),
        (0xf907b, 0xff),
        (0xf907c, 0x00),
        (0xf907d, 0x00),
        (0xf907e, 0x00),
        (0xf907f, 0x00),
        (0xf9080, 0x00),
        (0xf9081, 0x00),
        (0xf9082, 0xff),
        (0xf9083, 0xff),
        (0xf9084, 0x00),
        (0xf9085, 0x00),
        (0xf9086, 0x00),
        (0xf9087, 0x00),
        (0xf9088, 0x00),
        (0xf9089, 0x00),
        (0xf908a, 0xff),
        (0xf908b, 0xff),
        (0xf908c, 0x00),
        (0xf908d, 0x00),
        (0xf908e, 0x00),
        (0xf908f, 0x00),
        (0xf9090, 0x00),
        (0xf9091, 0x00),
        (0xf9092, 0xff),
        (0xf9093, 0xff),
        (0xf9094, 0x00),
        (0xf9095, 0x00),
        (0xf9096, 0x00),
        (0xf9097, 0x00),
        (0xf9098, 0x00),
        (0xf9099, 0x00),
        (0xf909a, 0xff),
        (0xf909b, 0xff),
        (0xf909c, 0x00),
        (0xf909d, 0x00),
        (0xf909e, 0x00),
        (0xf909f, 0x00),
        (0xf90a0, 0x00),
        (0xf90a1, 0x00),
        (0xf90a2, 0xff),
        (0xf90a3, 0xff),
        (0xf90a4, 0x00),
        (0xf90a5, 0x00),
        (0xf90a6, 0x00),
        (0xf90a7, 0x00),
        (0xf90a8, 0x00),
        (0xf90a9, 0x00),
        (0xf90aa, 0xff),
        (0xf90ab, 0xff),
        (0xf90ac, 0x00),
        (0xf90ad, 0x00),
        (0xf90ae, 0x00),
        (0xf90af, 0x00),
        (0xf90b0, 0x00),
        (0xf90b1, 0x00),
        (0xf90b2, 0xff),
        (0xf90b3, 0xff),
        (0xf90b4, 0x00),
        (0xf90b5, 0x00),
        (0xf90b6, 0x00),
        (0xf90b7, 0x00),
        (0xf90b8, 0x00),
        (0xf90b9, 0x00),
        (0xf90ba, 0xff),
        (0xf90bb, 0xff),
        (0xf90bc, 0x00),
        (0xf90bd, 0x00),
        (0xf90be, 0x00),
        (0xf90bf, 0x00),
        (0xf90c0, 0x00),
        (0xf90c1, 0x00),
        (0xf90c2, 0xff),
        (0xf90c3, 0xff),
        (0xf90c4, 0x00),
        (0xf90c5, 0x00),
        (0xf90c6, 0x00),
        (0xf90c7, 0x00),
        (0xf90c8, 0x00),
        (0xf90c9, 0x00),
        (0xf90ca, 0xff),
        (0xf90cb, 0xff),
        (0xf90cc, 0x00),
        (0xf90cd, 0x00),
        (0xf90ce, 0x00),
        (0xf90cf, 0x00),
    ];
    let mut diffs = 0;
    for &(a, expected) in dst_after {
        let got = mem.read8(a);
        if got != expected {
            println!("DIVERGENCE addr={:#08x} rust68={:#04x} hatari_reel={:#04x}", a, got, expected);
            diffs += 1;
        }
    }
    println!("Total divergences: {} / {}", diffs, dst_after.len());
    for &(a, _) in dst_after {
        let got = mem.read8(a);
        let expected = mem.last_writer.get(&a).copied();
        let _ = got;
        if let Some(idx) = expected {
            if idx != usize::MAX {
                println!("addr={:#08x} last_call={}", a, idx);
            }
        }
    }
}