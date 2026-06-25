# Rust68 — État de l'émulateur MC68000

> Dernière mise à jour : 2026-06-25

## Score TomHarte actuel

| Catégorie | Nombre |
|-----------|--------|
| **Réussis** | **317 488** |
| Échecs (SBCD nibbles invalides) | 12 |
| Ignorés (non implémentés) | 0 |
| **Total tests** | **317 500** |

**Conformité : 99.996 %**

Les 12 échecs restants sont des cas `SBCD` avec des demi-octets BCD > 9 :
comportement non spécifié par Motorola, variable selon le silicon. Non
corrigeable sans simuler les anomalies du chip exact utilisé par TomHarte.

---

## Architecture du code

```
src/
  cpu.rs          (~250 l.)  — registres, SR, USP/SSP, fetch_word, take_exception
  addressing.rs   (~250 l.)  — Operand, Size, resolve_ea (tous les modes 68000)
  execute.rs     (~1600 l.)  — décodage et exécution de toutes les instructions
  bus.rs                     — trait Bus + FlatBus (16 Mo RAM plate)
  lib.rs                     — exports publics
tests/
  instructions.rs            — tests unitaires ciblés
  tomharte.rs                — harnais de conformité TomHarte (avec FOCUS et baseline)
```

---

## Instructions implémentées (jeu complet MC68000)

### Transferts de données
- `MOVE.b/w/l`, `MOVEA.w/l`, `MOVEQ`
- `MOVEM.w/l`, `MOVEP.w/l`
- `LEA`, `PEA`
- `MOVE from/to CCR`, `MOVE from/to SR` *(to SR : privilégié)*
- `MOVE from/to USP` *(privilégié)*
- `EXT.w/l`, `SWAP`, `EXG`, `CLR`

### Arithmétique entière
- `ADD/ADDA/ADDQ/ADDX`, `SUB/SUBA/SUBQ/SUBX`
- `NEG/NEGX`, `MULU/MULS`, `DIVU/DIVS`

### Arithmétique BCD
- `ABCD`, `SBCD`, `NBCD` *(flags V/N indéfinis côté hardware)*

### Logique & bits
- `AND/OR/EOR/NOT`, `ORI/ANDI/EORI to CCR/SR`
- `BTST/BSET/BCLR/BCHG`

### Comparaison & test
- `CMP/CMPA/CMPI/CMPM`, `TST`, `CHK`

### Décalages & rotations
- `LSL/LSR`, `ASL/ASR`, `ROL/ROR`, `ROXL/ROXR` (b/w/l, registre et mémoire)

### Branchements
- `BRA`, `BSR`, `Bcc`, `DBcc`, `Scc`, `JMP`, `JSR`, `RTS`, `RTR`
- `LINK`, `UNLK`

### Contrôle CPU
- `NOP`, `RESET`, `STOP`, `RTE`, `TAS`

### Exceptions logicielles
- `TRAP #0–#15`, `TRAPV`, `ILLEGAL`, Line-A, Line-F
- Privilege violation pour toutes les instructions privilégiées en user mode

### Address Error (AE)
Implémentation complète du frame 14 octets pour tous les accès word/long sur
adresse impaire : read AE, write AE, instruction fetch AE. Gestion correcte de
tous les cas MOVE.l (ordre d'écriture LSW-first pour `-(An)`, CCR selon src/dst
mode, frame_pc pipeline-correct).

---

## Harnais de test TomHarte

```sh
# Test ciblé (~1s)
TOMHARTE_DIR=… FOCUS=MOVE.l cargo test --test tomharte -- --nocapture

# Run complet avec détection de régression (~55s)
TOMHARTE_DIR=… cargo test --test tomharte -- --nocapture

# Mettre à jour la baseline après amélioration
TOMHARTE_DIR=… BASELINE=1 cargo test --test tomharte -- --nocapture
```

La baseline (`tomharte_baseline.txt`, dans .gitignore) permet de détecter les
régressions et de visualiser les progrès instruction par instruction.

---

## Ce qui reste pour l'émulation Atari ST

Le CPU MC68000 est complet. Pour émuler un Atari ST il faut :

| Composant | Priorité |
|-----------|----------|
| Interruptions IPL (niveaux 2/4/6) | Haute |
| MFP 68901 (timers, UART, IRQ) | Haute |
| GLUE (HBL/VBL) | Haute |
| ACIA 6850 × 2 (clavier/MIDI) | Haute |
| YM2149 (son + I/O joysticks) | Moyenne |
| WD1772 (floppy) | Moyenne |
| Shifter (vidéo ST low/med/high) | Moyenne |
| Blitter | Basse |
| Timing cycle-accurate | Basse |
| Trace mode (bit T SR) | Basse |
