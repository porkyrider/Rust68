# Rust68 — État de l'émulateur MC68000

> Dernière mise à jour : 2026-07-27

## Score TomHarte actuel

| Catégorie | Nombre |
|-----------|--------|
| **Réussis (état registres/RAM)** | **317 099** |
| Échecs (état registres/RAM) | **0** |
| dont corrects en état mais avec un nombre de cycles erroné | 401 |
| **Total tests** | **317 099** |

**Conformité état (registres/RAM) : 100 %.**
**Conformité cycle-exacte : 99.87 %** (401 cas sur 317 099 ont le bon
résultat mais un coût en cycles encore faux — chantier en cours, non
bloquant, cf. section Timing ci-dessous).

Les 12 échecs `SBCD` documentés dans le commit initial (nibbles BCD
invalides, ">9") sont **résolus** : voir « Bug SBCD » ci-dessous.

---

## Architecture du code

```
src/
  cpu.rs          (~440 l.)  — registres, SR, USP/SSP, fetch_word, take_exception,
                                take_bus_error_full, exception_log (log diagnostic)
  addressing.rs   (~380 l.)  — Operand, Size, resolve_ea (tous les modes 68000)
  execute.rs     (~2780 l.)  — décodage et exécution de toutes les instructions
  bus.rs          (~160 l.)  — trait Bus + FlatBus (16 Mo RAM plate) + TimedBus
                                (wait-states DRAM/vidéo) + take_bus_fault()
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
- `NEG/NEGX`, `MULU/MULS`, `DIVU/DIVS` *(timing data-dépendant, cf. Timing)*

### Arithmétique BCD
- `ABCD`, `SBCD` (100 % conforme, y compris nibbles BCD invalides — cf.
  Bug SBCD), `NBCD` *(flags V/N indéfinis côté hardware)*

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
- Divide-by-zero (vecteur 5) : frame PC = adresse de l'instruction
  *suivante* (trap post-instruction, RTE ne doit pas ré-exécuter le DIVU/DIVS)

### Address Error (vecteur 3) & Bus Error (vecteur 2)
Implémentation complète du frame 14 octets pour tous les accès word/long sur
adresse impaire : read AE, write AE, instruction fetch AE. Gestion correcte de
tous les cas MOVE.l (ordre d'écriture LSW-first pour `-(An)`, CCR selon src/dst
mode, frame_pc pipeline-correct), avec `fault_prefix` calculé automatiquement
par `resolve_ea` pour ne pas re-facturer le coût déjà engagé par
l'instruction fautive.

Le Bus error (même frame, vecteur 2) est déclenché via `Bus::take_bus_fault()`
— un hook optionnel (défaut : jamais de fault) que l'implémentation de `Bus`
peut renseigner pour signaler un accès dans une zone sans aucun chip select
(le « trou » physique entre RAM installée et ROM sur un ST/STE réel). C'est le
mécanisme qu'utilisent de nombreux programmes/démos pour détecter la RAM
installée (handler au vecteur $8 + scan d'adresses croissantes). Vérifié après
le fetch d'opcode et après chaque exécution dans `Cpu::step`.

### Journal diagnostique
`Cpu::exception_log` : buffer circulaire (`EXCEPTION_LOG_CAP` = 4096) des
exceptions prises (vecteur, PC empilé, adresse fautive, write?, PC du handler,
cycles). Pur enregistrement sans effet sur l'exécution, pour les harnais de
diagnostic externes (`crates/app/examples/*`).

---

## Bug SBCD (résolu)

Les 12 cas `SBCD` avec nibbles BCD invalides (>9) échouaient sur le flag
C/X (le résultat en registre/RAM était déjà correct). Cause : le calcul de
l'emprunt (`hi_borrow`, nibble par nibble) ne détecte pas les cas où la
correction décimale fait passer le résultat sous zéro alors qu'aucun nibble
pris isolément n'indique d'emprunt — ex. `dst=0xb2, src=0xad, X=0` : le nibble
haut ne "borrow" pas (11-10-1=0) mais la correction du nibble bas (-6) fait
passer le total corrigé à -1 avant masquage 8 bits. Le silicium détecte
l'emprunt sur cette valeur *avant* masquage, pas sur l'arithmétique nibble par
nibble.

Fix (`src/execute.rs`, `op_sbcd`) : `actual_borrow = hi_borrow || corrected_raw
< 0`, où `corrected_raw` est la valeur signée avant le `& 0xFF` final. Vérifié
exhaustivement contre les 2 500 cas `SBCD` de TomHarte (0 échec) sans toucher
au calcul du résultat ni régresser `ABCD`/`NBCD`.

---

## Timing cycle-exact (chantier en cours)

Plusieurs passes de mise en conformité des coûts en cycles (au-delà de
l'état registres/RAM, déjà correct à 100 %) :

1. **Coûts de base par instruction** (`ea_extra_cycles`, wait-states
   DRAM/vidéo via `TimedBus`) — voir commit `29b183d`.
2. **Précision data-dépendante** : DIVU/DIVS (algorithme microcode porté de
   WinUAE/Hatari), `fault_prefix` pour l'address error, corrections MULS/JSR/
   MOVEM/CMPI/CMPM/ADDA.l/SUBA.l — voir commit `a21e2ef`.
3. **Address error sur écriture** (2026-07-27, non commité) :
   - `ADDX.w`/`SUBX.w` en mode mémoire `-(An),-(An)` : le préfixe de coût en
     cas d'address error sur la lecture dst était erroné (12 au lieu de 8)
     dès que le registre source n'était pas A7 — vérifié exhaustivement
     contre les 2 × 2500 cas TomHarte, corrigé en préfixe 8 constant
     (410 cas résolus).
   - `MOVEM` mode `(d8,PC,Xn)` en source : tombait dans la case générique
     "16" au lieu de "18" (même coût que `(d8,An,Xn)`) — 1 ligne, 40 cas
     résolus.
   - `MOVE.w`/`MOVE.l` avec address error sur l'écriture destination : le
     code renvoyait un coût fixe de 50 (dispatch Group 0 seul), en ignorant
     le coût déjà engagé pour la lecture source (qui, elle, a réussi). Fix :
     `src_extra + move_dst_base(dst_mode, dst_reg, Word) + 50`, en forçant
     la variante *Word* du coût dst (un seul transfert 16 bits est tenté
     avant la détection de l'adresse impaire — la moitié haute d'un Long
     n'est jamais payée), plus +4 pour `-(An)` spécifiquement (prefetch
     matériel supplémentaire avant le write cycle). Dérivé et vérifié par
     analyse exhaustive des ~90 combinaisons mode-source × mode-dest ×
     taille dans les jeux de test TomHarte (1253 cas sur 1266 résolus).

Progression globale : 60 106 → **401** échecs de cycle-count (-99.3 %),
sans jamais régresser l'état registres/RAM.

Résidus documentés, non résolus, **data-dépendants** (le coût varie entre
deux valeurs pour des opérandes structurellement identiques — comparable à
la note MAME sur TRAPV ; une correction naïve a déjà empiré le score par le
passé, voir commentaire dans `op_line_4`/CHK) :
- `CHK` (388 cas) : le coût de l'exception vecteur 6 (`Dn<0` vs `Dn>bound`)
  varie entre 38 et 40 pour des cas structurellement identiques.
- `MOVE.w`/`MOVE.l` avec dst `(xxx).L` en address error (13 cas) : varie
  entre deux valeurs (écart de 4) selon un facteur non identifié.

---

## Harnais de test TomHarte

```sh
# Test ciblé (~1s)
TOMHARTE_DIR=… FOCUS=MOVE.l cargo test --release --test tomharte -- --nocapture

# Run complet avec détection de régression (~40s)
TOMHARTE_DIR=… cargo test --release --test tomharte -- --nocapture

# Mettre à jour la baseline après amélioration
TOMHARTE_DIR=… BASELINE=1 cargo test --release --test tomharte -- --nocapture
```

La baseline (`tomharte_baseline.txt`, dans .gitignore) permet de détecter les
régressions et de visualiser les progrès instruction par instruction.

---

## Ce qui reste pour l'émulation Atari ST

Le CPU MC68000 est complet et 100 % conforme en état (cycles en cours de
finalisation). Pour émuler un Atari ST il faut :

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
| Finaliser le timing cycle-exact (2 104 résidus) | Basse |
| Trace mode (bit T SR) | Basse |
