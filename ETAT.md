# Rust68 — État de l'émulateur MC68000

> Dernière mise à jour : 2026-08-05

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
  cpu.rs          (~685 l.)  — registres, SR, USP/SSP, fetch_word, take_exception,
                                take_bus_error_full, take_interrupt, take_trace_exception,
                                exception_log
  addressing.rs   (~455 l.)  — Operand, Size, resolve_ea (tous les modes 68000)
  execute.rs     (~3290 l.)  — décodage et exécution de toutes les instructions
  bus.rs          (~320 l.)  — trait Bus + FlatBus (16 Mo RAM plate) + TimedBus
                                (wait-states DRAM/vidéo) + take_bus_fault() + irq_level/irq_ack
  peripherals/
    mod.rs                   — `#[cfg(feature = "atari-st")] pub mod atari_st;`
    atari_st/
      mfp.rs      (~615 l.)  — MC68901 MFP (chip seul, cf. section dédiée)
      glue.rs     (~170 l.)  — timing HBL/VBL du GLUE (cf. section dédiée)
      acia.rs     (~150 l.)  — MC6850 ACIA (chip seul, cf. section dédiée)
      ikbd.rs     (~335 l.)  — contrôleur IKBD HD6301 clavier/souris (cf. section dédiée)
      ym2149.rs   (~465 l.)  — PSG YM2149 (chip seul, cf. section dédiée)
      microwire.rs(~205 l.)  — interface série vers le mixeur LMC1992 (STE)
      dma_sound.rs(~295 l.)  — DMA Sound STE (lecture PCM 8 bits depuis la RAM)
      drive_sound.rs(~270 l.)— bruitage mécanique du lecteur de disquette
      shifter.rs  (~390 l.)  — vidéo Shifter (chip seul, cf. section dédiée)
      wd1772.rs   (~945 l.)  — contrôleur disquette WD1772 (chip seul, cf. section dédiée)
      stx.rs      (~610 l.)  — lecteur minimal de disquettes `.stx` (Pasti)
      msa.rs      (~265 l.)  — lecteur de disquettes `.msa` (Magic Shadow Archiver)
      blitter.rs  (~880 l.)  — coprocesseur BitBlt Blitter (chip seul, cf. section dédiée)
  systems/
    mod.rs                   — `#[cfg(feature = "atari-st")] pub mod atari_st;`
    atari_st/
      mod.rs     (~1695 l.)  — board ST minimal (RAM/ROM/MFP/GLUE/ACIA/YM2149/Shifter/WD1772/
                                Blitter, cf. section dédiée)
      model.rs                — lexique des modèles ST/STE/Mega ST/Mega STE (RAM, Blitter, TOS
                                d'origine — cf. section dédiée)
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
  blitter_hatari_diff.rs      — oracle différentiel (portage direct de `Blitter_ProcessWord`)
  atari_st.rs                 — tests du board Atari ST, bout-en-bout (feature `atari-st`)
  tomharte.rs                — harnais de conformité TomHarte (avec FOCUS et baseline)
examples/
  rd_menu_repro.rs / rd_menu_ca6a.rs — reproduction headless (sans SDL2) d'un clic-glisser
                                de menu GEM, avec traçage Blitter/CPU ciblé — infrastructure
                                de diagnostic conservée, pas des scripts jetables.
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

**Double bus fault (`Cpu::halted`)** : un bus/address error survenant en
empilant le frame d'un précédent bus/address error (ou en lisant son
vecteur) — typiquement un pointeur de pile hors de toute zone mappée —
est détecté et arrête définitivement le CPU (`StepError::DoubleFault`),
comme le ferait un vrai 68000 (HALT jusqu'à un `/RESET` matériel), plutôt
que de rebondir indéfiniment sur un vecteur avec un frame corrompu. Bug
réel trouvé en creusant le boot froid Atari ST (voir plus bas) :
auparavant, le flag de fault posé par la poussée de frame elle-même
restait non consommé et contaminait le `step` suivant, provoquant une
cascade infinie et silencieuse.

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

Testé dans `tests/mfp.rs` (13 tests).

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

## IKBD (`src/peripherals/atari_st/ikbd.rs`)

`peripherals::atari_st::ikbd::Ikbd` modélise le contrôleur HD6301
(clavier/souris/joystick), câblé sur `acia_keyboard` — auparavant absent :
les commandes envoyées par le TOS pour configurer l'IKBD (reset,
modes souris…) étaient écrites dans l'ACIA mais jamais lues/interprétées
(`take_tx_byte` n'était appelé nulle part), et le binaire de démonstration
gérait lui-même, à la main, une file de sortie ad hoc (`ikbd_tx_queue`).
Port très largement inspiré du projet compagnon Stay (mêmes constats,
mêmes correctifs, adaptés à l'absence de joystick ici) :

- Réponse d'autotest `0xF1` **différée** (`IKBD_RESET_CYCLES`, 5 000 000
  cycles) plutôt qu'immédiate au reset : la livrer trop tôt fait arriver
  l'octet avant que le TOS ait fini de configurer IERB/IMRB du MFP,
  l'interruption ACIA correspondante (en attente mais encore masquée)
  étant alors silencieusement effacée par l'écriture ultérieure normale du
  TOS dans IERB — l'octet n'est ensuite jamais lu, RDRF reste plein en
  permanence, bloquant tout octet suivant (clavier ET souris) pour
  toujours.
- `receive_cmd`/`execute_cmd` : reset (`0x80 0x01`), mode souris relatif
  (`0x08`), axe Y (`0x0F`/`0x10`), interrogation de position absolue
  (`0x0D`) réellement implémentés ; les commandes joystick (non modélisé,
  pas de frontend manette) consomment quand même le bon nombre d'octets de
  paramètres pour ne pas désynchroniser le flux de commandes suivant.
- `mouse_move(dx, dy, buttons)` : position absolue interne suivie et
  bornée (0..639/0..399), paquet relatif standard `0xF8|boutons, dx, dy`
  émis seulement si quelque chose a changé.
- `AtariSt::tick` relie l'ensemble : drainage de `take_tx_byte` vers
  `Ikbd::receive_cmd`, livraison d'un octet de `Ikbd::pop_tx` par tick
  (gardée par RDRF), et **force explicitement un relâchement GPIP4** avant
  de réarmer RDRF pour l'octet suivant dans le même tick — sur silicium
  réel, `/IRQ` de l'ACIA remonte réellement le temps de l'intervalle série
  entre deux octets ; sans ce relâchement forcé, le 2e/3e octet de chaque
  trame (ex. une trame souris) ne produit jamais de front pour
  `Mfp::set_gpip_input`, qui est à juste titre edge-triggered — bug exact
  déjà isolé et corrigé dans Stay.
- Le binaire de démonstration (`atari_st_sdl2`) échantillonne le
  mouvement souris une seule fois par trame vidéo plutôt qu'à chaque
  évènement SDL brut (potentiellement bien plus fréquent que le ~50 Hz
  réel), et plafonne chaque paquet à ±15 unités (limite réelle du
  firmware IKBD, pas seulement ±127 du champ signé sur un octet),
  découpant un déplacement plus grand en plusieurs paquets — même
  approche que `Machine::flush_input_vbl`/`mouse_move` de Stay.
- Pré-remplissage RAM supplémentaire au boot "redémarrage à chaud" (voir
  section suivante) : `$0EE4`/`$0EE5` = `0x11`/`0x11` (porte Timer-C de
  l'IKBD, normalement posée par le TOS lui-même pendant sa mise en place —
  sans elle, `ASL.W #3,($0EE4).L; BPL` du gestionnaire Timer C prendrait
  la mauvaise branche et n'écoulerait jamais les octets IKBD en attente).

Testé dans `src/peripherals/atari_st/ikbd.rs` (8 tests unitaires).

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

Testé dans `tests/ym2149.rs` (10 tests).

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

Testé dans `tests/shifter.rs` (9 tests) et `tests/atari_st.rs` (rendu de
trame PAL complète de 313 lignes, entre autres tests bout-en-bout).

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
  directe de fichiers réels puis recoupé avec la doc publique du format et
  Hatari, expose désormais `bit_position` (position réelle du champ ID sur
  la piste physique, utilisée par `Wd1772::cycles_to_target_sector` pour un
  calcul de latence rotationnelle fidèle même sur une piste au formatage
  non standard) — et `msa::parse` pour `.msa` (Magic Shadow Archiver,
  simple conteneur compressé RLE piste par piste, décompressé en mémoire
  vers une `RawDiskImage` équivalente, aucune métadonnée de protection dans
  ce format).
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

Testé dans `tests/wd1772.rs` (20 tests, dont la latence rotationnelle via
`bit_position` et l'arrêt au compte réel de secteurs par piste),
`src/peripherals/atari_st/msa.rs` (8 tests), `src/peripherals/atari_st/stx.rs`
(9 tests) et `tests/atari_st.rs` (aller-retour lecture/écriture secteur
bout-en-bout via DMA, entre autres tests bout-en-bout).

---

## Blitter (`src/peripherals/atari_st/blitter.rs`)

`peripherals::atari_st::blitter::Blitter` modélise le coprocesseur de transfert de
blocs (BitBlt) de l'Atari STE seul : combine un mot source (optionnellement
décalé bit à bit via `skew`, registre à décalage 32 bits PERSISTANT sur
toute la durée de vie de la puce — pas remis à zéro entre deux lignes ni
entre deux blits logiquement séparés), un motif de demi-teinte, et le
contenu destination, via une fonction booléenne programmable (`OP`, une des
16 fonctions à 2 entrées, convention de table de vérité standard partagée
par de nombreuses puces "raster op"), avec masquage de bord de ligne
(`ENDMASK1/2/3`) et parcours par incréments X/Y. Mappé dans `AtariSt` à
`0xFF8A00` (champ public `blitter`). Traité **mot par mot** (traduction
directe de la machine à états de Hatari, `Blitter_ProcessWord`), pas ligne
par ligne avec une formule d'avance précalculée, pour reproduire fidèlement
ce registre à décalage persistant.

En mode HOG (bit 6 de `CONTROL`), `execute()` traite tout le blit en un
seul appel. En mode non-HOG (le plus courant en pratique), un seul appel ne
traite que 64 accès bus réels (lecture source, lecture/écriture
destination, chacun compté séparément — valeur et méthode de comptage
reprises de Hatari, `BLITTER_NONHOG_BUS_BLITTER`) avant de rendre la main,
`BUSY` restant posé entre deux tranches — le CPU dispose alors de 256
cycles (`AtariSt::BLITTER_SLICE_CYCLES`, calibré sur le même `64*4` que
Hatari) pour s'exécuter en parallèle avant que `AtariSt::tick` ne rappelle
`execute()` pour la tranche suivante. Le déclenchement initial se fait via
l'écriture du bit BUSY/START du registre de contrôle ; un déclenchement
CONTROL accidentel (`TAS.B` dans la boucle de relance logicielle typique)
n'y ré-exécute PAS tout le blit depuis le début — `execute()` reprend
exactement où la tranche précédente s'est arrêtée (voir `armed`).

Registres `CONTROL` (`0xFF8A3C` : BUSY/HOG/SMUDGE/numéro de ligne de
demi-teinte) et `SKEW` (`0xFF8A3D` : FXSR/NFSR/décalage) conformes à
l'offset réel de la puce — croisés contre le datasheet `BLITTER.TXT`
(info-coach.fr), le `BLIT_FAQ.TXT` (dépôt `ggnkua/Atari_ST_Sources`) et
le code source de Hatari (`src/blitter.c`), qui se recoupent tous les
trois. Une première version avait ces deux offsets inversés (bug
corrigé) et prenait `HOP=0` pour "tout à zéro" au lieu de "tout à un"
(table du datasheet, également corrigé).

- `FXSR`/`NFSR` (bits du registre `SKEW`) honorés explicitement plutôt
  que déduits de `skew != 0` : `FXSR` déclenche la lecture d'amorçage en
  début de ligne, `NFSR` supprime la dernière lecture source de la ligne.
- `SMUDGE` (bit du registre `CONTROL`) implémenté : le mot de demi-teinte
  utilisé pour chaque mot vient des 4 bits bas du mot source décalé,
  potentiellement différent à chaque mot d'une même ligne (au lieu du
  numéro de ligne courant en mode normal).
- Le numéro de ligne de demi-teinte (bits 0-3 de `CONTROL`) est
  directement lisible/inscriptible par le logiciel (pas un compteur
  interne caché), et avance ou recule en fin de ligne selon le signe de
  `DST_Y_INC`, conformément au datasheet.
- Pas de mode "dessin de ligne" pour tracé de polygone (contrairement au
  Blitter Amiga) : ce n'est pas une limitation de cette implémentation,
  la puce Atari STE n'a simplement pas cette fonctionnalité — le champ
  "numéro de ligne" du registre CONTROL sert uniquement à la sélection
  de demi-teinte ci-dessus.

`skew` gère bien la direction du parcours (`SRC_X_INC` positif/négatif,
blit "miroir") : le registre à décalage source est alimenté différemment
selon le signe (`shift_buffer`/`fetch_buffer`, traduction directe de
`Blitter_SourceShift`/`Blitter_SourceFetch` de Hatari) — une version
antérieure supposait toujours un parcours croissant.

**Limitations restantes, à prendre avec prudence** (aucune suite
équivalente à TomHarte n'existe pour ce périphérique) :
- Le modèle de tranche à 64 accès bus reste une approximation du mode
  "cycle exact" de Hatari, qui entrelace ces accès AU MILIEU de
  l'exécution des instructions CPU (pas seulement entre deux instructions
  complètes) et reproduit un cas de bug documenté du silicium réel où le
  Blitter s'arrête parfois à 63 accès au lieu de 64 — non modélisé ici.

Testé dans `tests/blitter.rs` (23 tests : table de vérité OP, HOP y
compris HOP=0, endmask, parcours X/Y, cycle et registre de numéro de
ligne de demi-teinte, FXSR, NFSR, SMUDGE, skew=0/miroir, tranches non-HOG
comptées en accès bus réels), `tests/blitter_hatari_diff.rs` (portage
direct de `Blitter_ProcessWord` comme oracle différentiel) et
`tests/atari_st.rs` (blit bout-en-bout déclenché via le registre de
contrôle, entre autres tests bout-en-bout).

---

## Board Atari ST (`src/systems/atari_st/mod.rs`)

`systems::atari_st::AtariSt` implémente `Bus` pour un ST/STE minimal, en
reliant toutes les puces ci-dessus :

- RAM installée à `0x000000` (taille au choix), ROM TOS à `0xFC0000`
  (`DEFAULT_ROM_BASE`, lecture seule).
- MFP 68901 aux adresses impaires `0xFFFA01`-`0xFFFA2F` (champ public
  `mfp`). ACIA clavier (`0xFFFC00`/`02`) et MIDI (`0xFFFC04`/`06`), champs
  publics `acia_keyboard`/`acia_midi`, la première câblée sur un
  contrôleur IKBD (champ `ikbd`, voir section dédiée). YM2149
  (`0xFF8800`/`02`, champ `ym2149`). Shifter (`0xFF8201`+, champ
  `shifter`). WD1772/DMA (`0xFF8604`+, champs `wd1772`/`floppy_a`).
  Blitter (`0xFF8A00`+, champ `blitter`, déclenché par l'écriture du bit
  BUSY/START de son registre de contrôle).
- Au-delà de la RAM installée mais dans l'espace d'adressage "RAM ST" fixe
  de 4 Mo (deux banques MMU de 2 Mo sur ST/STE réel), l'accès ne
  déclenche **jamais** de bus error : le MMU y répond toujours /DTACK sur
  silicium réel, même sans RAM physique à l'adresse précise (confirmé par
  la communauté Atari). Modélisé par une valeur fixe non stockée (une
  lecture ne renvoie jamais ce qui vient d'être écrit) plutôt que la
  valeur réelle (résidu de capacité du bus, non déterministe). C'est
  cette absence de persistance, pas un bus error, que le TOS observe pour
  sa détection de RAM au tout début du boot froid. Le vrai "trou"
  (bus error via `take_bus_fault`, mécanisme qu'utilisent de nombreux
  programmes/démos une fois le TOS démarré) commence seulement à 4 Mo,
  jusqu'à `0xFF8000`.
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

Testé dans `tests/atari_st.rs` (48 tests, dont plusieurs bout-en-bout :
interruption GPIP→MFP→IPL, priorité MFP/VBL/HBL, ACIA→GPIP4→MFP,
WD1772→GPIP5→MFP, rendu vidéo, lecture/écriture disquette via DMA, blit
déclenché via le registre de contrôle, moniteur couleur persistant après
`/RESET`).

---

## Lexique des modèles (`src/systems/atari_st/model.rs`)

Principe général, valable pour n'importe quelle machine qu'on émule (pas
seulement celle-ci) : émuler une machine réelle précise, ce n'est pas
choisir une taille de RAM au hasard puis espérer que ça marche — c'est un
ensemble de caractéristiques qui vont ensemble (vitesse CPU, RAM
d'origine, ROM/BIOS attendue, options matérielles). `model::AtariModel`
rassemble ces caractéristiques pour la gamme ST/STE sous forme
consultable (`AtariModel::profile() -> MachineProfile`) plutôt que de les
laisser éparpillées en constantes magiques dans le binaire de
démonstration.

- 6 modèles couverts : `St520`, `St1040`, `MegaSt`, `Ste520`, `Ste1040`,
  `MegaSte` — caractéristiques (RAM, Blitter de série, TOS d'origine,
  fréquence CPU) croisées depuis plusieurs références publiques
  (Wikipedia, old-computers.com, atari-wiki.com, atari-forum.com).
- `AtariModel::parse(nom)` : reconnaissance insensible à la casse et aux
  séparateurs (`"1040ste"`, `"1040STE"`, `"Mega-STE"`…).
- `AtariSt::from_model(profile, rom)` : construit le board avec la RAM et
  la présence du Blitter du modèle choisi (`AtariSt::blitter_present`,
  vérifié par `is_blitter_addr`) ; le ROM est fourni séparément (la
  version de TOS installée n'est **pas** une propriété du modèle — un ST
  réel peut très bien tourner avec un TOS plus récent que celui
  d'origine, mise à jour EPROM courante). La base ROM (`0xFC0000` vs
  `0xE00000`) reste réglée indépendamment via `set_rom_base`, déjà
  auto-détectée depuis `os_version` dans l'en-tête TOS.
- Binaire de démonstration : `--model <nom>` (défaut `1040ste`), voir
  `atari_st_sdl2 --help` pour la liste. Exemple :
  `cargo run --release --features sdl2-frontend --bin atari_st_sdl2 --
  --model 520ste tos162.img disque.stx`.

Ce qui n'est **pas** (encore) modélisé par ce lexique :
- `cpu_hz` est informatif seulement : le rythme MFP (ratio horloge fixe
  192/625) et le pacing audio du binaire `atari_st_sdl2` supposent tous
  les deux un CPU à 8 MHz. Choisir un modèle Mega STE ne fait donc pas
  tourner l'émulation à 16 MHz.
- Le Mega ST est modélisé Blitter présent par défaut (support PLCC
  existant sur toutes les cartes mères mais pas systématiquement peuplé
  en usine sur les premières séries) — à ajuster manuellement via
  `MachineProfile.has_blitter` si besoin d'un Mega ST sans Blitter précis.

Testé dans `src/systems/atari_st/model.rs` (3 tests unitaires) et
`tests/atari_st.rs` (1 test bout-en-bout : RAM/Blitter réglés d'après le
modèle).

---

## Ce qui reste pour l'émulation Atari ST

Le CPU MC68000 est complet, 100 % conforme en état ET 100 % cycle-exact,
trace mode compris. L'interruption IPL, le MFP 68901, le GLUE, les deux
ACIA 6850, le YM2149, le Shifter, le WD1772, le Blitter et un board
minimal reliant tout ça sont en place — tous les composants de la feuille
de route initiale sont couverts.

Un vrai TOS 1.62 non modifié démarre jusqu'au bureau GEM interactif
(vidéo couleur basse résolution, clavier, souris, icônes disquette, menus
déroulants) via le binaire de démonstration `atari_st_sdl2` (`cargo run
--release --features sdl2-frontend --bin atari_st_sdl2 -- <rom.img>
[disque.st|.stx|.msa]`) — voir la section Architecture pour le détail des
features Cargo. Le raccourci "redémarrage à chaud" (cookies
`memvalid`/`memval2`/`memval3`/`phystop` pré-remplis, voir le code de
`main`) reste le chemin par défaut recommandé (rapide, fiable) ;
`RUST68_COLD_BOOT=1` force un vrai boot froid (détection de RAM réelle
par le TOS, plus lente), fonctionnel mais moins testé au quotidien.

**Bugs GEM résolus** :
- *Clics souris sans effet* (icônes, items de menu) — cause en plusieurs
  couches : le contrôleur IKBD n'existait pas du tout (commandes TOS
  jamais interprétées, front GPIP4 starvé entre octets d'une même trame,
  `$0EE4` non pré-rempli — voir section IKBD), puis un dernier bug plus
  simple une fois l'IKBD en place : les bits gauche/droit du paquet
  souris étaient inversés (`queue_mouse_move`, `atari_st_sdl2.rs`). Le
  mode souris "exclusif" (`Cmd+Shift+F10`) a été ajouté à cette occasion.
- *Texte corrompu dans les menus déroulants GEM* — `AtariSt::write16`
  scindait toute écriture `.W` combinée CONTROL+SKEW du Blitter
  (`$FF8A3C`/`3D`) en deux `write8` séquentiels, CONTROL puis SKEW ; or
  l'écriture de CONTROL déclenche `Blitter::execute()` de façon
  synchrone dès que le bit BUSY est posé, donc le Blitter démarrait
  parfois avec l'ANCIEN SKEW, un instant avant que le nouveau ne soit
  posé par ce même accès `.W` (cas réel de TOS 1.62 : `MOVE.W D7,(A5)`
  en `$E11746`, armant les 4 plans d'un blit de restauration de menu
  avec un SKEW partagé). Corrigé en écrivant SKEW avant de déléguer à
  `write8` pour CONTROL. Vérifié par comparaison directe avec un vrai
  Hatari (patch de trace différentiel `RUST68_HATARI_TRACE` sur
  `blitter.c`) : les deux tournent désormais à l'identique sur ce blit.
  Outils de diagnostic conservés (coût nul si non activés) :
  `RUST68_TRACE_BLIT_REGS`/`RUST68_TRACE_BLIT_START`/
  `RUST68_TRACE_BLITTER_WORDS`, `examples/rd_menu_repro.rs`/
  `rd_menu_ca6a.rs` (reproduction headless d'un clic-glisser de menu,
  sans SDL2 ni interaction réelle).

Pistes pour aller plus loin (aucune n'est un blocage, ce sont des
approfondissements) :
- Vérification des points documentés comme non confirmés contre une
  référence matérielle réelle (timing MFP/GLUE, polarité haute
  résolution du Shifter, cas limite "63 accès bus au lieu de 64" du
  Blitter).
- Contention DRAM/vidéo pour les accès Shifter/Blitter (mécanisme déjà
  générique via `Bus::is_contended`, pas encore branché sur ces deux
  puces).
- `.stx` : métadonnées de protection par secteur (fuzzy bits, timing)
  volontairement ignorées par le lecteur minimal — de vraies protections
  de jeux resteraient bloquées.
- Résidu de dérive de timing (~60-80 VBL sur un très gros transfert
  disquette protégé, `Rick_Dangerous.stx`) après le correctif de latence
  rotationnelle STX (`bit_position`) — cause non identifiée, crash final
  inchangé (A6=0 au même endroit).
