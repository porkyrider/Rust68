# Rust68 — État de l'émulateur MC68000

> Dernière mise à jour : 2026-07-28

## Licence

GPL-3.0-or-later (voir [LICENSE](LICENSE)). Toute redistribution, y
compris dans un projet fermé ou commercial, doit republier le code source
de ses modifications sous les mêmes termes.

## Score TomHarte actuel

| Catégorie | Nombre |
|-----------|--------|
| **Réussis (état registres/RAM ET cycles)** | **317 500** |
| Échecs | **0** |
| **Total tests** | **317 500** |

**Conformité état (registres/RAM) : 100 %. Conformité cycle-exacte : 100 %.**

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

## Architecture du code

Le cœur 68000 (`cpu.rs`/`addressing.rs`/`execute.rs`/`bus.rs`) ne dépend
d'aucun système particulier. Les périphériques et boards sont rangés **un
sous-module par système émulé** sous `peripherals/`/`systems/`, chacun
derrière sa propre feature Cargo — aucune activée par défaut : `cargo
build`/`cargo test` sans option ne compilent/rapatrient que le cœur, il
faut explicitement `--features atari-st` pour l'Atari ST :

```
src/
  cpu.rs          (~520 l.)  — registres, SR, USP/SSP, fetch_word, take_exception,
                                take_bus_error_full, take_interrupt, take_trace_exception,
                                exception_log
  addressing.rs   (~380 l.)  — Operand, Size, resolve_ea (tous les modes 68000)
  execute.rs     (~2810 l.)  — décodage et exécution de toutes les instructions
  bus.rs          (~195 l.)  — trait Bus + FlatBus (16 Mo RAM plate) + TimedBus
                                (wait-states DRAM/vidéo) + take_bus_fault() + irq_level/irq_ack
  peripherals/
    mod.rs                   — `#[cfg(feature = "atari-st")] pub mod atari_st;`
    atari_st/
      mfp.rs      (~550 l.)  — MC68901 MFP (chip seul, cf. section dédiée)
      glue.rs     (~130 l.)  — timing HBL/VBL du GLUE (cf. section dédiée)
      acia.rs     (~150 l.)  — MC6850 ACIA (chip seul, cf. section dédiée)
      ym2149.rs   (~375 l.)  — PSG YM2149 (chip seul, cf. section dédiée)
      shifter.rs  (~265 l.)  — vidéo Shifter (chip seul, cf. section dédiée)
      wd1772.rs   (~370 l.)  — contrôleur disquette WD1772 (chip seul, cf. section dédiée)
      blitter.rs  (~340 l.)  — coprocesseur BitBlt Blitter (chip seul, cf. section dédiée)
      stx.rs                 — lecteur minimal de disquettes `.stx` (Pasti)
  systems/
    mod.rs                   — `#[cfg(feature = "atari-st")] pub mod atari_st;`
    atari_st.rs   (~570 l.)  — board ST minimal (RAM/ROM/MFP/GLUE/ACIA/YM2149/Shifter/WD1772/
                                Blitter, cf. section dédiée)
  bin/
    atari_st_sdl2.rs         — frontend SDL2 (vidéo/clavier/souris/son), feature `sdl2-frontend`
  lib.rs                     — exports publics
tests/
  instructions.rs            — tests unitaires ciblés (CPU)
  interrupts.rs               — tests du mécanisme IPL (Cpu::take_interrupt)
  trace.rs                     — tests du mode trace (Cpu::take_trace_exception)
  mfp.rs                      — tests du MFP 68901 (feature `atari-st`)
  glue.rs                      — tests du GLUE (HBL/VBL) (feature `atari-st`)
  acia.rs                      — tests du MC6850 ACIA (feature `atari-st`)
  ym2149.rs                    — tests du PSG YM2149 (feature `atari-st`)
  shifter.rs                   — tests de la vidéo Shifter (feature `atari-st`)
  wd1772.rs                    — tests du contrôleur disquette WD1772 (feature `atari-st`)
  blitter.rs                   — tests du Blitter (feature `atari-st`)
  atari_st.rs                 — tests du board Atari ST, bout-en-bout (feature `atari-st`)
  tomharte.rs                — harnais de conformité TomHarte (avec FOCUS et baseline)
```

### Features Cargo

| Feature | Défaut | Effet |
|---|---|---|
| `atari-st` | désactivée | Compile `peripherals::atari_st` et `systems::atari_st`. Sans elle (défaut), on n'a que le cœur 68000. |
| `sdl2-frontend` | désactivée | Compile le binaire `atari_st_sdl2` (dépend de `atari-st`, active `sdl2` en dépendance). |

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

### Interruptions IPL
`Cpu::take_interrupt` (appelé par `Cpu::step` avant le fetch de chaque
opcode) : reconnaît une demande d'interruption externe et la prend si son
niveau dépasse le masque IPL courant du SR (niveau 7 = NMI, toujours pris).
Frame standard 6 octets (SR+PC), bascule superviseur, masque IPL relevé au
niveau accepté, cycle d'acquittement via deux méthodes du trait `Bus` :

- `Bus::irq_level() -> u8` : niveau demandé (0 = aucune demande) par le
  système hôte.
- `Bus::irq_ack(level) -> u8` : vecteur à utiliser (défaut : autovecteur
  24+level).

Coût approximatif de 44 cycles (aucune suite TomHarte ne couvre les
interruptions). Testé dans `tests/interrupts.rs`. Pas géré : réveil sur
STOP, edge-detect propre du niveau 7.

### Trace (bit T du SR)
Le 68000 déclenche l'exception vecteur 9 après chaque instruction si le
bit T du SR est actif à la fin de celle-ci (relu après l'instruction, pas
avant : une instruction qui pose T elle-même — MOVE/ANDI/ORI to SR, RTE
qui dépile un SR avec T=1 — se trace donc dès sa propre fin). TomHarte
capture volontairement l'effet d'une seule instruction sans enchaîner sur
la trace même quand T=1 en entrée (le `final.sr`/`final.pc` d'un NOP avec
T=1 est identique à T=0, aucun frame poussé) : `Cpu::step` ne prend donc
**pas** l'exception lui-même, il pose `Cpu::trace_pending` à vrai — à
l'appelant (une vraie boucle d'émulation, pas le harnais de conformité)
d'appeler `Cpu::take_trace_exception` après chaque `step` s'il veut
l'effet réel, avant le `step` suivant (préserve la priorité
trace-avant-interruption du silicium). Frame standard 6 octets, coût de
34 cycles. Aucune exception logicielle interne à l'instruction (TRAP,
CHK, division par zéro, ILLEGAL, Line-A/F, violation de privilège) ne
déclenche de trace supplémentaire : leur propre `take_exception` efface
déjà T en entrant dans leur frame. Testé dans `tests/trace.rs` (7 tests).

### Journal diagnostique
`Cpu::exception_log` : buffer circulaire (`EXCEPTION_LOG_CAP` = 4096) des
exceptions prises (vecteur, PC empilé, adresse fautive, write?, PC du handler,
cycles). Pur enregistrement sans effet sur l'exécution, pour les harnais de
diagnostic externes.

---

## MFP 68901 (`src/peripherals/atari_st/mfp.rs`)

`peripherals::atari_st::mfp::Mfp` modélise la puce **seule**, indépendamment de tout
câblage système : c'est au board de mapper `Mfp::read`/`write` dans son
`Bus`, de brancher `Mfp::iack` sur `Bus::irq_ack`, et de relier
`Mfp::interrupt_requested()` à `Bus::irq_level` (câblé sur IPL6 sur
ST/STE réel).

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
- Pas de priorité imbriquée entre canaux.
- `tick()` suppose une horloge CPU fixe à 8 MHz (ST/STE) ; la période d'un
  timer delay-mode est modélisée comme `(data+1) × prescale` cycles MFP —
  non vérifiée contre une référence matérielle externe (aucune suite de
  test équivalente à TomHarte n'existe pour le MFP).
- Si plusieurs périodes s'écoulent en un seul appel `tick()`, une seule
  interruption est signalée, pas une par période.

Testé dans `tests/mfp.rs` (11 tests).

---

## GLUE (`src/peripherals/atari_st/glue.rs`)

`peripherals::atari_st::glue::Glue` modélise le générateur de timing HBL/VBL de la
puce GLUE (rôle mémoire/bus de GLUE non couvert) :

- `tick(cpu_cycles)` avance un compteur de ligne ; à chaque fin de ligne,
  arme HBL (IPL2) ; à chaque fin de trame (313 lignes en PAL 50 Hz, 263
  en NTSC 60 Hz — `VideoMode`), arme VBL (IPL4) et incrémente
  `frame_count()`.
- `ack_hbl`/`ack_vbl` acquittent (à appeler depuis `Bus::irq_ack`).
- `current_line()`/`frame_count()`/`lines_per_frame()` exposés en lecture
  pour synchroniser un rendu vidéo sur la position de balayage.

Limitations : uniquement le timing (pas le rôle mémoire/bus de GLUE) ;
constantes 512 cycles/ligne (PAL) et 508 (NTSC) usuelles Hatari/WinSTon,
non vérifiées contre une référence matérielle formelle.

Testé dans `tests/glue.rs` (5 tests).

---

## ACIA 6850 (`src/peripherals/atari_st/acia.rs`)

`peripherals::atari_st::acia::Acia` modélise une puce MC6850 seule ; l'Atari ST en
embarque **deux** (clavier, MIDI), chacune une instance séparée dans
`AtariSt` (`acia_keyboard`/`acia_midi`), dont les IRQ sont OR câblées sur
`GPIP4` du MFP.

- Registres CONTROL/STATUS et DATA, flags RDRF/TDRE/OVRN/FE fidèles au
  MC6850 : lire DATA efface RDRF+OVRN ensemble ; écrire CONTROL avec
  bits0-1=`11` déclenche un Master Reset (efface les flags, réarme TDRE).
- Pas de FIFO de réception (comme le silicium réel) : un octet reçu alors
  que le précédent n'a pas été lu déclenche `OVRN` et le nouvel octet est
  **perdu**.
- `irq_requested()` : `(RDRF && RIE) || (TDRE && TIE)` — `DCD`/`CTS`
  toujours à 0 (pas de handshake externe simulé), `PE` toujours à 0 (pas
  de simulation de parité).
- `push_rx_byte`/`take_tx_byte` : modèle "au byte".

Testé dans `tests/acia.rs` (7 tests).

---

## YM2149 (`src/peripherals/atari_st/ym2149.rs`)

`peripherals::atari_st::ym2149::Ym2149` modélise la puce seule (compatible registre à
registre avec le General Instrument AY-3-8910) : 3 canaux de tonalité
carrée, un générateur de bruit partagé, un générateur d'enveloppe, 2 ports
d'E/S 8 bits. Mappée dans `AtariSt` (champ public `ym2149`) au sélecteur
`0xFF8800` / données `0xFF8802`, cadencée par `AtariSt::tick` (CPU/4 = 2 MHz
sur ST/STE). Ne génère pas d'IPL (pas d'interruption sur ST).

- 16 registres avec masquage exact de leur largeur réelle (ex: tone coarse
  4 bits, période de bruit 5 bits, forme d'enveloppe 4 bits).
- Générateurs de tonalité (3×) et de bruit (LFSR 17 bits, polynôme
  standard bit0 XOR bit3) pilotés par `tick()`.
- Générateur d'enveloppe : rampe 5 bits (résolution double de l'ampli fixe
  4 bits), table des 10 formes standard (Continue/Attack/Alternate/Hold).
  Écrire le registre de forme relance toujours l'enveloppe depuis le
  début (comportement documenté du silicium).
- `channel_level(canal)` : niveau 0-31 combinant le portillonnage
  tonalité/bruit (`MIXER`) et l'amplitude fixe ou l'enveloppe — pas de
  conversion en échantillons PCM (à la charge d'un futur pipeline audio).
- Ports A/B : registres 8 bits bruts, direction par bits 6-7 de `MIXER` ;
  signification des bits (sélection lecteur, joystick, Centronics…) non
  interprétée, à charge du board/de l'appelant.

Testé dans `tests/ym2149.rs` (9 tests).

---

## Shifter (`src/peripherals/atari_st/shifter.rs`)

`peripherals::atari_st::shifter::Shifter` modélise la puce vidéo seule : lit la RAM
vidéo ligne par ligne et la convertit en pixels RGB 24 bits selon la
résolution programmée (basse 320×200/4 plans, moyenne 640×200/2 plans,
haute 640×400/1 plan monochrome). Mappé dans `AtariSt` (champ public
`shifter`, adresses `$FF8201`/`03` base vidéo, `$FF8205`/`07`/`09` compteur
vidéo, `$FF8240`-`$FF825E` palette 16 couleurs, `$FF8260` résolution).

- Déinterlacement des plans (format bitplane word-interleaved standard
  Atari ST : pour un groupe de 16 pixels, un mot consécutif par plan) —
  fait mot par mot puis bit par bit, MSB en premier.
- Palette 16 couleurs au format ST (3×3 bits RGB), convertie en RGB 24
  bits par réplication de bits (`v<<5 | v<<2 | v>>1`, exacte aux deux
  extrémités 0/255).
- Câblé dans `AtariSt::tick` sur le rythme HBL/VBL du GLUE : détecte les
  changements de `Glue::current_line`/`frame_count` (via un compteur de
  ligne **absolu**, pas le compteur bouclant du GLUE) et alimente
  `AtariSt::framebuffer` (`Vec<Vec<(u8,u8,u8)>>`, une entrée par ligne).

Limitations : format de palette ST uniquement (pas l'extension 12 bits
STE) ; compteur vidéo toujours accepté en écriture (comportement STE, pas
lecture-seule comme sur ST d'origine) ; pas de défilement fin ;
convention de polarité du mode haute résolution (bit=1 → noir) non
vérifiée contre une capture matérielle réelle ; pas de contention
DRAM/vidéo modélisée pour l'accès Shifter.

Testé dans `tests/shifter.rs` (8 tests) et `tests/atari_st.rs` (5 tests
supplémentaires, dont un rendu de trame PAL complète de 313 lignes).

---

## WD1772 (`src/peripherals/atari_st/wd1772.rs`)

`peripherals::atari_st::wd1772::Wd1772` modélise le contrôleur de disquette seul :
4 registres (Commande/Statut, Piste, Secteur, Donnée) multiplexés par les
lignes A1-A0, jeu de commandes Type I (Restore/Seek/Step/Step-In/
Step-Out), Type II (Read/Write Sector, mono et multi-secteur) et Type IV
(Force Interrupt).

- Le disque est abstrait via le trait `FloppyDisk` (adressage piste/face/
  secteur, pas octet), objet (`Box<dyn FloppyDisk>` côté `AtariSt`) pour
  accepter plusieurs formats sans coupler le board à l'un d'eux :
  `RawDiskImage` pour le `.st` brut (secteurs linéaires), `stx::StxImage`
  pour un lecteur `.stx` (Pasti) minimal — reverse-engineé par inspection
  directe de fichiers réels (aucune spec officielle consultée), ignore
  volontairement toute la métadonnée de protection (fuzzy bits, timing),
  extrait juste le contenu brut des secteurs.
- Transfert Type II via le trait `DmaChannel` (`pull`/`push` un octet à la
  fois) : le WD1772 ne connaît pas la RAM, seulement le disque et ce
  canal — c'est au board de l'implémenter avec accès à sa RAM.
- Modèle synchrone "instantané par secteur" : toute commande s'exécute
  entièrement avant que `execute_command` ne rende la main — `BUSY` n'est
  donc jamais observable par un polling logiciel. Pas de vérification/CRC
  réels (bit V toujours réussi), Type III (Read Address/Track, Write
  Track/Format) non implémenté.

Câblage dans `systems::atari_st::AtariSt` : registre multiplexé à
`0xFF8604` (sélecteur de registre via `0xFF8606`, modèle simplifié — pas
de registre de nombre de secteurs ni de sélection FDC/HDC réels),
compteur d'adresse DMA à `0xFF8609`/`0B`/`0D`. `/INTRQ` câblé sur `GPIP5`
du MFP, relayé par `AtariSt::tick`. Disque inséré via le champ public
`AtariSt::floppy_a`.

Testé dans `tests/wd1772.rs` (13 tests) et `tests/atari_st.rs` (6 tests
supplémentaires, dont un aller-retour lecture/écriture secteur bout-en-bout
via DMA).

---

## Blitter (`src/peripherals/atari_st/blitter.rs`)

`peripherals::atari_st::blitter::Blitter` modélise le coprocesseur de transfert de
blocs (BitBlt) de l'Atari STE seul : combine un mot source (optionnellement
décalé bit à bit via `skew`), un motif de demi-teinte, et le contenu
destination, via une fonction booléenne programmable (`OP`, une des 16
fonctions à 2 entrées, convention de table de vérité standard partagée par
de nombreuses puces "raster op"), avec masquage de bord de ligne
(`ENDMASK1/2/3`) et parcours par incréments X/Y. Mappé dans `AtariSt` à
`0xFF8A00` (champ public `blitter`) ; `execute()` prend un `Bus` (adapté
vers la RAM du board via un petit type `RamBus` interne) et exécute le
blit en entier de façon synchrone, déclenché par l'écriture du bit
BUSY/START du registre de contrôle.

**Limitations à prendre avec prudence** (aucune suite équivalente à
TomHarte n'existe pour ce périphérique) :
- `skew` (décalage bit à bit du mot source, pour aligner des images non
  alignées sur un mot) : implémenté d'après ma meilleure compréhension du
  mécanisme documenté, **non vérifié contre une référence matérielle
  réelle ou un émulateur existant**. `NFSR`/`FXSR` non modélisés
  distinctement (une lecture d'amorçage est toujours faite si `skew!=0`).
- Mode "dessin de ligne" (line number pour du tracé de polygone) non
  implémenté : seul le blit rectangulaire standard est couvert.
- Pas de vol de cycles bus au CPU modélisé (bit HOG/STEAL) : le blit
  s'exécute intégralement de façon synchrone, `BUSY` jamais observable
  par polling.
- Halftone : le mot utilisé par ligne cycle `halftone[line % 16]`,
  `line` incrémentant à chaque ligne Y — comportement usuel documenté,
  non vérifié indépendamment.

Testé dans `tests/blitter.rs` (8 tests : table de vérité OP, HOP, endmask,
parcours X/Y, cycle halftone, skew=0) et `tests/atari_st.rs` (3 tests
supplémentaires, dont un blit bout-en-bout déclenché via le registre de
contrôle).

---

## Board Atari ST (`src/systems/atari_st.rs`)

`systems::atari_st::AtariSt` implémente `Bus` pour un ST/STE minimal, en
reliant toutes les puces ci-dessus :

- RAM installée à `0x000000` (taille au choix), ROM TOS à `0xFC0000`
  (`DEFAULT_ROM_BASE`, lecture seule).
- MFP 68901 aux adresses impaires `0xFFFA01`-`0xFFFA2F` (champ public
  `mfp`). ACIA clavier (`0xFFFC00`/`02`) et MIDI (`0xFFFC04`/`06`), champs
  publics `acia_keyboard`/`acia_midi`. YM2149 (`0xFF8800`/`02`, champ
  `ym2149`). Shifter (`0xFF8201`+, champ `shifter`). WD1772/DMA
  (`0xFF8604`+, champs `wd1772`/`floppy_a`). Blitter (`0xFF8A00`+, champ
  `blitter`, déclenché par l'écriture du bit BUSY/START de son registre
  de contrôle).
- Le "trou" physique entre le haut de la RAM installée et `0xFF8000`
  déclenche un bus error via `take_bus_fault` — mécanisme utilisé par de
  nombreux programmes/démos pour détecter la RAM installée.
- Reste de la zone d'E/S non mappée : chip select réel mais périphérique
  pas encore émulé → lecture neutre `0xFF`, écriture ignorée (pas de bus
  error, pour ne pas casser le polling de statut du logiciel).
- `irq_level`/`irq_ack` câblent MFP (IPL6), VBL (IPL4) et HBL (IPL2) par
  priorité décroissante. Les deux ACIA et le WD1772 ne génèrent pas d'IPL
  directement : leurs IRQ sont OR câblées sur `GPIP4`/`GPIP5` du MFP.
- `reset_bus` réinitialise MFP/ACIA×2/YM2149/Shifter/WD1772 ; le GLUE,
  lui, n'est **pas** réinitialisé (le timing vidéo continue indépendamment
  d'un `/RESET` CPU sur silicium réel) ; le disque inséré (`floppy_a`)
  non plus (support physique, pas état de puce).
- `AtariSt::tick(cpu_cycles)` fait progresser MFP/GLUE/YM2149, relaie les
  IRQ ACIA/WD1772, et déclenche le rendu vidéo (Shifter) au rythme
  HBL/VBL du GLUE — à appeler explicitement par l'appelant après chaque
  `Cpu::step` (ce crate ne fait pas progresser les périphériques tout
  seul).

Limitations connues : pas de miroir ROM `0xE00000` (130ST) ; pas de
contention DRAM/vidéo (`is_contended` reste `false`) ; décodage UDS/LDS
des adresses paires adjacentes au MFP non modélisé précisément ;
registres DMA/WD1772 simplifiés (voir section WD1772).

Testé dans `tests/atari_st.rs` (32 tests, dont plusieurs bout-en-bout :
interruption GPIP→MFP→IPL, priorité MFP/VBL/HBL, ACIA→GPIP4→MFP,
WD1772→GPIP5→MFP, rendu vidéo, lecture/écriture disquette via DMA, blit
déclenché via le registre de contrôle, moniteur couleur persistant après
`/RESET`).

---

## Ce qui reste pour l'émulation Atari ST

Le CPU MC68000 est complet, 100 % conforme en état ET 100 % cycle-exact,
trace mode compris. L'interruption IPL, le MFP 68901, le GLUE, les deux
ACIA 6850, le YM2149, le Shifter, le WD1772, le Blitter et un board
minimal reliant tout ça sont en place — tous les composants de la feuille
de route initiale sont couverts.

Un vrai TOS 1.62 non modifié démarre jusqu'au bureau GEM interactif
(vidéo couleur basse résolution, clavier, souris, icônes disquette) via le
binaire de démonstration `atari_st_sdl2` (`cargo run --release --features
sdl2-frontend --bin atari_st_sdl2 -- <rom.img> [disque.stx|.st]`) — voir
la section Architecture pour le détail des features Cargo.

Pistes pour aller plus loin (aucune n'est un blocage, ce sont des
approfondissements) :
- Vérification des points documentés comme non confirmés contre une
  référence matérielle réelle (timing MFP/GLUE, skew du Blitter,
  polarité haute résolution du Shifter).
- Contention DRAM/vidéo pour les accès Shifter/Blitter (mécanisme déjà
  générique via `Bus::is_contended`, pas encore branché sur ces deux
  puces).
- `.stx` : métadonnées de protection par secteur (fuzzy bits, timing)
  volontairement ignorées par le lecteur minimal — de vraies protections
  de jeux resteraient bloquées.
