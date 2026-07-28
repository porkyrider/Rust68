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
  cpu.rs          (~490 l.)  — registres, SR, USP/SSP, fetch_word, take_exception,
                                take_bus_error_full, take_interrupt, exception_log
  addressing.rs   (~380 l.)  — Operand, Size, resolve_ea (tous les modes 68000)
  execute.rs     (~2790 l.)  — décodage et exécution de toutes les instructions
  bus.rs          (~195 l.)  — trait Bus + FlatBus (16 Mo RAM plate) + TimedBus
                                (wait-states DRAM/vidéo) + take_bus_fault() + irq_level/irq_ack
  peripherals/
    mfp.rs        (~550 l.)  — MC68901 MFP (chip seul, cf. section dédiée)
    glue.rs       (~125 l.)  — timing HBL/VBL du GLUE (cf. section dédiée)
    acia.rs       (~150 l.)  — MC6850 ACIA (chip seul, cf. section dédiée)
  systems/
    atari_st.rs   (~240 l.)  — board ST minimal (RAM/ROM/MFP/GLUE/ACIA, cf. section dédiée)
  lib.rs                     — exports publics
tests/
  instructions.rs            — tests unitaires ciblés (CPU)
  interrupts.rs               — tests du mécanisme IPL (Cpu::take_interrupt)
  mfp.rs                      — tests du MFP 68901
  glue.rs                      — tests du GLUE (HBL/VBL)
  acia.rs                      — tests du MC6850 ACIA
  atari_st.rs                 — tests du board Atari ST (dont 3 tests bout-en-bout)
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
3. **Address error sur écriture** (2026-07-27) :
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

Confirmation empirique (2026-07-27) : pour `MOVE (xxx).L` dst, sur
l'ensemble des cas structurellement identiques (même src_mode/dst_mode/
taille) qui déclenchent une address error sur l'écriture, la répartition
est quasi 50/50 entre les deux coûts possibles (14 cas à correction 0,
13 cas à correction -4) — changer la constante ne ferait que déplacer
l'échec vers l'autre moitié, pas le réduire. Ce n'est donc pas une
formule qui manque mais un vrai plancher matériel (comme documenté pour
CHK) : ces 401 cas ne sont pas résolubles sans modéliser le pipeline
d'instruction cycle par cycle (au-delà du modèle actuel table+bus).

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

## Interruptions IPL (résolu — mécanisme CPU)

`Cpu::take_interrupt` (appelé par `Cpu::step` avant le fetch de chaque
opcode, en dehors du `TimedBus`) : reconnaît une demande d'interruption
externe et la prend si son niveau dépasse le masque IPL courant du SR
(niveau 7 = NMI, toujours pris). Frame standard 6 octets (SR+PC, pas le
frame Group 0/1 des address/bus errors), bascule superviseur, masque IPL
relevé au niveau accepté, cycle d'acquittement via deux nouvelles méthodes
du trait `Bus` :

- `Bus::irq_level() -> u8` : niveau demandé (0 = aucune demande), à
  implémenter par le système hôte (GLUE, MFP…). Défaut : 0 (jamais de
  demande — inchangé pour FlatBus/ProcessorTests).
- `Bus::irq_ack(level) -> u8` : vecteur à utiliser. Défaut : autovecteur
  (24+level, cas GLUE HBL/VBL) ; un périphérique vectorisé (MFP 68901)
  surchargera pour renvoyer son propre vecteur programmé.

Coût approximatif de 44 cycles (aucune suite TomHarte ne couvre les
interruptions, événements externes et non des opcodes — à calibrer plus
tard). Testé dans `tests/interrupts.rs` (masquage, prise, NMI, vecteur
fourni par le périphérique, bascule user→superviseur). Pas encore géré :
réveil sur STOP, edge-detect propre du niveau 7.

Ce que ce mécanisme ne fournit **pas** encore : aucun périphérique ne
demande d'interruption pour l'instant (`irq_level` renvoie toujours 0 par
défaut) — c'est le rôle du MFP 68901 / GLUE, ci-dessous.

---

## MFP 68901 (nouveau module `src/peripherals/mfp.rs`)

Décision d'organisation (2026-07-27) : les périphériques Atari ST vivent
dans **ce même crate**, sous `src/peripherals/`, plutôt que dans un
workspace séparé — `rust68` reste un seul crate à publier/tester, au prix
de mélanger cœur 68k générique et matériel Atari-spécifique.

`peripherals::mfp::Mfp` modélise la puce **seule**, indépendamment de tout
câblage système : c'est au "board" Atari ST (pas encore implémenté) de
mapper `Mfp::read`/`write` dans son `Bus`, de brancher `Mfp::iack` sur
`Bus::irq_ack`, et de relier `Mfp::interrupt_requested()` à `Bus::irq_level`
(le MFP est câblé sur IPL6 sur ST/STE réel — ce choix appartient au board).

Couvert :
- 24 registres logiques (`mfp::reg`) : GPIP/AER/DDR, IERA/B, IPRA/B,
  ISRA/B, IMRA/B, VR, TACR/TBCR/TCDCR + 4 registres de données timer,
  SCR/UCR/RSR/TSR/UDR (USART).
- Contrôleur d'interruption à 16 canaux (`mfp::channel`, table fixée par
  le datasheet) : IPR ne s'arme que si IER actif, `interrupt_requested()`
  ne remonte que si IMR non masqué, IPR/ISR ne s'effacent que par écriture
  de 0 (jamais par écriture de 1). `iack()` calcule le vecteur
  (`VR[7:3] | canal`), efface IPR, arme ISR sauf en mode auto-EOI (bit 3
  du VR).
- 4 timers A/B/C/D : mode delay (prescale ÷4/÷10/÷16/÷50/÷64/÷100/÷200) et
  mode event-count (A/B uniquement, piloté par `pulse_ta`/`pulse_tb`
  plutôt que par l'horloge). Lire le registre de données d'un timer EN
  MARCHE renvoie le compte à rebours courant (comportement réel du chip,
  utilisé par TOS pour lire une horloge sans l'arrêter).
- GPIP avec détection de front (AER) par broche, sur les 8 canaux dédiés.
- USART simplifié "au byte" (`push_rx_byte`/`take_tx_byte`) : pas de
  simulation bit start/stop/parité ni de baud rate réel.

Limitations connues, documentées dans le module :
- Pas de priorité imbriquée entre canaux (un canal en service ne bloque
  pas un canal de priorité supérieure de se signaler, mais ne l'empêche
  pas non plus explicitement — simplification volontaire pour cette v1).
- `tick()` suppose une horloge CPU fixe à 8 MHz (ST/STE) ; la période d'un
  timer delay-mode est modélisée comme `(data+1) × prescale` cycles MFP —
  cette convention (+1) n'a pas été vérifiée contre une référence
  matérielle externe (aucune suite de test équivalente à TomHarte
  n'existe pour le MFP) ; à confirmer plus tard si la précision fine
  compte (Hatari/WinUAE font référence).
- Si plusieurs périodes s'écoulent en un seul appel `tick()` (cycles très
  larges), une seule interruption est signalée, pas une par période —
  sans impact pour un usage normal (tick() appelé à chaque instruction).

Testé dans `tests/mfp.rs` (11 tests : GPIP/AER, IER/IPR/IMR, IACK/priorité/
auto-EOI, timers delay et event-count, USART).

---

## Board Atari ST (nouveau module `src/systems/atari_st.rs`)

`systems::atari_st::AtariSt` implémente `Bus` pour un ST/STE minimal, en
reliant tout ce qui précède :

- RAM installée à `0x000000` (taille au choix), ROM TOS à `0xFC0000`
  (`DEFAULT_ROM_BASE`, lecture seule).
- MFP 68901 mappé aux adresses impaires `0xFFFA01`-`0xFFFA2F` (champ
  public `AtariSt::mfp`, pour y injecter GPIP/USART/timers depuis
  l'appelant).
- ACIA clavier (`0xFFFC00`/`0xFFFC02`) et ACIA MIDI (`0xFFFC04`/
  `0xFFFC06`), adresses paires (champs publics `acia_keyboard`/
  `acia_midi`) — voir section ACIA ci-dessous.
- Le "trou" physique entre le haut de la RAM installée et `0xFF8000`
  déclenche un bus error via `take_bus_fault` (mécanisme construit plus
  tôt spécifiquement pour ce cas — première utilisation concrète).
- Reste de la zone d'E/S (`0xFF8000`-`0xFFFFFF` : PSG, FDC, Shifter…) :
  chip select réel mais périphérique pas encore émulé → lecture neutre
  `0xFF`, écriture ignorée (pas de bus error, pour ne pas casser le
  polling de statut du logiciel).
- `irq_level`/`irq_ack` câblent MFP (IPL6), VBL (IPL4) et HBL (IPL2) par
  priorité décroissante — voir section GLUE ci-dessous. Les deux ACIA ne
  génèrent pas d'IPL directement : leurs IRQ sont OR câblées sur `GPIP4`
  du MFP (câblage réel), relayé par `AtariSt::tick`.
- `reset_bus` réinitialise MFP + ACIA×2 (RESET CPU → `/RESET`
  périphériques) ; le GLUE, lui, n'est **pas** réinitialisé (le timing
  vidéo continue indépendamment d'un `/RESET` CPU sur silicium réel).
- `AtariSt::tick(cpu_cycles)` fait progresser MFP + GLUE et relaie l'IRQ
  ACIA→GPIP4 ; à appeler explicitement par l'appelant après chaque
  `Cpu::step` (ce crate ne fait pas progresser les périphériques tout
  seul).

Limitations connues : pas de miroir ROM `0xE00000` (130ST), pas de
contention DRAM/vidéo (`is_contended` reste `false`, nécessite le
Shifter), décodage UDS/LDS des adresses paires adjacentes au MFP non
modélisé précisément.

Testé dans `tests/atari_st.rs` (14 tests), dont un test d'intégration
bout-en-bout : un `Cpu` réel prend une interruption GPIP du MFP à travers
toute la chaîne `Cpu::step` → `Bus::irq_level` → `Bus::irq_ack` →
`Mfp::iack`, saute au bon handler et relève le masque IPL à 6 ; un test
de priorité MFP > VBL > HBL acquittés dans l'ordre ; et un test de bout
en bout ACIA → GPIP4 → MFP → IPL6.

---

## GLUE (nouveau module `src/peripherals/glue.rs`)

`peripherals::glue::Glue` modélise le générateur de timing HBL/VBL de la
puce GLUE (rôle mémoire/bus de GLUE non couvert — cf. limitations) :

- `tick(cpu_cycles)` avance un compteur de ligne ; à chaque fin de ligne,
  arme HBL (IPL2) ; à chaque fin de trame (313 lignes en PAL 50 Hz, 263
  en NTSC 60 Hz — `VideoMode`), arme VBL (IPL4) et incrémente
  `frame_count()`.
- `ack_hbl`/`ack_vbl` acquittent (à appeler depuis `Bus::irq_ack`, voir
  le câblage dans `AtariSt`).
- `current_line()`/`frame_count()` exposés en lecture pour synchroniser
  un futur rendu vidéo (Shifter) sur la position de balayage.

Limitations documentées dans le module : uniquement le timing (pas le
rôle mémoire/bus de GLUE) ; constantes 512 cycles/ligne (PAL) et 508
(NTSC) usuelles Hatari/WinSTon, non vérifiées contre une référence
matérielle formelle (aucune suite équivalente à TomHarte n'existe pour
ce composant, même limitation que pour le MFP).

Testé dans `tests/glue.rs` (5 tests : HBL par ligne, VBL + bouclage de
ligne en fin de trame, PAL vs NTSC, plusieurs lignes en un seul `tick`).

---

## ACIA 6850 (nouveau module `src/peripherals/acia.rs`)

`peripherals::acia::Acia` modélise une puce MC6850 seule ; l'Atari ST en
embarque **deux** (clavier, MIDI), chacune une instance séparée dans
`AtariSt` (`acia_keyboard`/`acia_midi`), dont les IRQ sont OR câblées sur
`GPIP4` du MFP (voir section Board).

- Registres CONTROL/STATUS et DATA, flags RDRF/TDRE/OVRN/FE fidèles au
  MC6850 : lire DATA efface RDRF+OVRN ensemble ; écrire CONTROL avec
  bits0-1=`11` déclenche un Master Reset (efface les flags, réarme TDRE).
- Pas de FIFO de réception (comme le silicium réel) : un octet reçu alors
  que le précédent n'a pas été lu déclenche `OVRN` et le nouvel octet est
  **perdu** (contrairement au MFP, dont l'USART simplifié met en file).
- `irq_requested()` : `(RDRF && RIE) || (TDRE && TIE)` — `DCD`/`CTS`
  toujours à 0 (pas de handshake externe simulé), `PE` toujours à 0 (pas
  de simulation de parité).
- `push_rx_byte`/`take_tx_byte` : même modèle "au byte" que le MFP.

Testé dans `tests/acia.rs` (7 tests : reset, réception/lecture, overrun,
transmission, IRQ soumise à RIE/TIE, master reset).

---

## Ce qui reste pour l'émulation Atari ST

Le CPU MC68000 est complet et 100 % conforme en état (cycles en cours de
finalisation). L'interruption IPL, le MFP 68901, le GLUE (HBL/VBL), les
deux ACIA 6850 et un board minimal (RAM/ROM/MFP/GLUE/ACIA) sont en place.
Pour émuler un Atari ST complet il faut encore :

| Composant | Priorité |
|-----------|----------|
| YM2149 (son + I/O joysticks) | Moyenne |
| WD1772 (floppy) | Moyenne |
| Shifter (vidéo ST low/med/high) | Moyenne |
| Blitter | Basse |
| Finaliser le timing cycle-exact (401 résidus, plancher matériel data-dépendant) | Basse |
| Trace mode (bit T SR) | Basse |
