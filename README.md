# Rust68

Émulateur de la série **Motorola 68000** (68k) en Rust.

L'objectif à terme est de couvrir toute la famille 68000 et ses variantes
(68010, 68020, 68030…). Cette première version cible le **MC68000** d'origine,
celui qui équipe l'Atari ST et l'Amiga.

## Architecture

- **`Cpu`** — l'état du processeur : registres `D0–D7` / `A0–A7`, `PC`, `SR`
  (octet système + CCR), pointeurs de pile `USP`/`SSP`.
- **`Bus`** — un *trait* que l'appelant implémente pour son système. Le CPU ne
  contient aucune mémoire : tous les accès passent par le bus. Cela permet de
  modéliser des cartes mémoire différentes (Atari vs Amiga) sans toucher au
  cœur. Seuls `read8`/`write8` sont obligatoires ; les accès 16/32 bits sont
  dérivés en **big-endian** (l'ordre natif du 68000).
- **`FlatBus`** — une implémentation de `Bus` toute simple (16 Mo de RAM plate),
  fournie pour les tests et le prototypage.
- **Timing** — comptage de cycles **par instruction** (tables Motorola). Le
  timing bus cycle-accurate viendra plus tard.

## Exemple

```rust
use rust68::{Cpu, Bus, FlatBus};

let mut bus = FlatBus::new();
bus.write32(0x0000, 0x0000_1000); // SSP initial
bus.write32(0x0004, 0x0000_0400); // PC initial
bus.write16(0x0400, 0x4E71);      // NOP

let mut cpu = Cpu::new();
cpu.reset(&mut bus);
cpu.step(&mut bus).unwrap();
assert_eq!(cpu.pc, 0x0402);
```

## Tests

```sh
cargo test
```

Deux niveaux de tests :

1. **Tests unitaires ciblés** (`tests/instructions.rs`) — un comportement
   observable par instruction. Filet de sécurité pendant le développement.
2. **Conformité TomHarte** (`tests/tomharte.rs`) — la suite
   [SingleStepTests/m68000](https://github.com/SingleStepTests/m68000), qui
   fournit des vecteurs état-avant / état-après pour chaque opcode.

   Les fichiers font plusieurs centaines de Mo et ne sont pas versionnés.
   Téléchargez-les puis :

   ```sh
   TOMHARTE_DIR=/chemin/vers/les/json cargo test --test tomharte -- --nocapture
   ```

   Sans `TOMHARTE_DIR`, ce test est ignoré proprement. Les opcodes pas encore
   implémentés sont comptés comme « ignorés » et non comme des échecs, ce qui
   permet de suivre la progression de la couverture au fil de l'implémentation.

## État d'avancement

Jeu d'instructions actuellement implémenté (amorçage) :

- Transferts : `MOVE`, `MOVEA`, `MOVEQ`, `LEA`, `CLR`
- Arithmétique : `ADD`, `ADDA` (avec flags X/N/Z/V/C corrects)
- Contrôle : `NOP`, `RESET`, `BRA`, `BSR`, `Bcc`

Modes d'adressage : tous les modes du 68000 (`Dn`, `An`, `(An)`, `(An)+`,
`-(An)`, `(d16,An)`, `(d8,An,Xn)`, absolu court/long, relatif PC, immédiat).

Le squelette de décodage est conçu pour accueillir le reste du jeu
d'instructions sans refonte.
