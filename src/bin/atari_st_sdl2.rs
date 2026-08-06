//! Frontend SDL2 minimal pour l'Atari ST émulé — vidéo + clavier + son.
//!
//! Binaire de démonstration/test, séparé de la bibliothèque `rust68` (voir
//! la feature Cargo `sdl2-frontend`) : sert à vérifier visuellement et
//! interactivement le bon fonctionnement de l'émulation (CPU + périphériques
//! ST), pas à être un émulateur ST "complet" (pas de configuration GUI, pas
//! de sauvegarde d'état, pas de support disquette en écriture persistante).
//!
//! Usage : `atari_st_sdl2 [--model <nom>] <rom.img> [disque.stx|disque.msa|disque.st]`
//!
//! `--model` sélectionne un profil de machine du lexique
//! `systems::atari_st::model` (RAM, présence du Blitter — voir sa doc pour
//! la liste et ce qui est réellement pris en compte). Le TOS chargé est
//! indépendant du modèle : sa version est auto-détectée depuis son en-tête
//! (`os_version`) pour choisir la bonne base ROM, pas depuis `--model`.
//!
//! ## Choix d'architecture
//! - Pas de minuteur temps réel dédié : ce crate n'en fournit aucun (voir
//!   `AtariSt::tick`, purement comptable). La cadence réelle est obtenue en
//!   throttlant sur le remplissage de la file audio SDL2 (`AudioQueue`) —
//!   le rythme de consommation de cette file par le pilote audio hôte EST
//!   l'horloge de référence temps réel.
//! - Le son est généré en échantillonnant `Ym2149::channel_level` au rythme
//!   `cycles_per_sample` dérivé de l'horloge CPU (8 MHz) et de la fréquence
//!   d'échantillonnage cible, indépendamment du pas d'exécution CPU
//!   (variable selon l'instruction).
//! - Le clavier traduit les `Scancode` SDL2 en scancodes ST (table figée,
//!   dérivée de la disposition PC-XT documentée publiquement que le
//!   contrôleur clavier HD6301 de l'ST reprend largement) : make code tel
//!   quel (bit 7 = 0), break code = make code | 0x80.
//! - La souris est injectée via le même canal série que le clavier : sur
//!   ST réel, le contrôleur IKBD (HD6301) gère clavier ET souris/joystick
//!   ensemble et envoie les deux à travers l'ACIA clavier. Un mouvement
//!   relatif de souris est un paquet de 3 octets documenté publiquement
//!   (protocole IKBD) : en-tête `0xF8 | gauche | (droit << 1)`, puis dX et
//!   dY signés sur un octet. Le delta est calculé nous-mêmes à partir de la
//!   position absolue de chaque évènement `MouseMotion` (pas le mode souris
//!   relatif natif de SDL2, dont le warp de curseur s'est révélé générer
//!   des évènements synthétiques parasites selon la plateforme/le backend) :
//!   le curseur hôte reste donc visible, en plus de celui dessiné par GEM.

use rust68::peripherals::atari_st::drive_sound::{wav, DriveSound, Slot};
use rust68::peripherals::atari_st::msa;
use rust68::peripherals::atari_st::stx::StxImage;
use rust68::peripherals::atari_st::wd1772::{FloppyDisk, RawDiskImage};
use rust68::peripherals::atari_st::ym2149::mix_channels_model;
use rust68::systems::atari_st::AtariSt;
use rust68::trace::FileTraceSink;
use rust68::{Bus, Cpu, TraceSink, TracingBus};

use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::{Mod, Scancode};
use sdl2::pixels::PixelFormatEnum;
use std::time::Duration;

const AUDIO_SAMPLE_RATE: i32 = 44_100;
/// Octets par échantillon de sortie : stéréo (2 canaux) × i16 (2 octets)
/// — voir `desired_spec`.
const AUDIO_BYTES_PER_SAMPLE: u32 = 4;
/// Seuil de remplissage de la file audio (en octets) au-delà duquel on
/// ralentit l'émulation pour rester au rythme temps réel.
///
/// Volontairement généreux (~500 ms, plutôt que les ~250 ms d'une version
/// précédente) : le cadencement horloge murale (voir plus bas, où ce seuil
/// n'est qu'un filet de sécurité secondaire) reste vulnérable à un `sleep`
/// ponctuel bien plus long que demandé — regroupement de minuteurs macOS,
/// mise en veille de l'app, ou simple préemption OS — qui peut vider une
/// bonne partie de la file d'un coup ; une marge plus large absorbe ce genre
/// d'épisode sans décroché audible plutôt que d'espérer l'éliminer à la
/// source (hors de portée d'un simple `std::thread::sleep`).
const AUDIO_QUEUE_HIGH_WATERMARK: u32 = AUDIO_SAMPLE_RATE as u32 * AUDIO_BYTES_PER_SAMPLE / 2; // ~500 ms
/// Seuil de remplissage initial (en octets) avant de démarrer la lecture —
/// sans cette marge, la file démarre vide et le device audio hôte consomme
/// plus vite qu'on ne peut produire pendant les toutes premières trames
/// (et à chaque micro-ralentissement ponctuel de l'émulation ensuite,
/// puisque seul un seuil HAUT existait auparavant, sans coussin en cas de
/// sous-remplissage) : chaque fois que la file se vide complètement, SDL2
/// rejoue du silence ou boucle le dernier bloc selon le backend, entendu
/// comme un « parasite »/clic — d'où le démarrage différé de la lecture ici.
const AUDIO_QUEUE_PRIME_WATERMARK: u32 = AUDIO_SAMPLE_RATE as u32 * AUDIO_BYTES_PER_SAMPLE / 5; // ~200 ms
/// Coefficient de lissage du gain Microwire (voir sa doc dans `main`) —
/// constante de temps d'environ 1/coeff échantillons, ~4-5 ms à 44100 Hz :
/// assez rapide pour suivre un vrai changement de volume voulu, assez lent
/// pour éviter un saut audible en plein milieu d'un signal non nul.
const GAIN_SMOOTH_COEFF: f32 = 0.005;
/// Modèle par défaut si `--model` n'est pas précisé.
const DEFAULT_MODEL: &str = "1040ste";

fn usage(program: &str) -> ! {
    eprintln!(
        "Usage : {program} [--model <nom>] [--ntsc] <rom.img> [disque.stx|disque.msa|disque.st]\n\n\
         Modèles disponibles (--model, casse indifférente) : 520st, 1040st, megast, \
         520ste, 1040ste (défaut), megaste — voir `systems::atari_st::model` pour le \
         détail de chaque profil (RAM, Blitter…).\n\
         --ntsc : mode vidéo 60 Hz (263 lignes/trame) au lieu du PAL 50 Hz par défaut \
         (313 lignes/trame) — voir `peripherals::atari_st::glue::VideoMode`."
    );
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let program = args[0].clone();

    let mut model_name = DEFAULT_MODEL.to_string();
    let mut ntsc = false;
    let mut positional: Vec<&String> = Vec::new();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--model" {
            match iter.next() {
                Some(v) => model_name = v.clone(),
                None => usage(&program),
            }
        } else if arg == "--ntsc" {
            ntsc = true;
        } else {
            positional.push(arg);
        }
    }
    if positional.is_empty() {
        usage(&program);
    }
    let rom_path = positional[0];
    let disk_path = positional.get(1);

    let model = rust68::systems::atari_st::model::AtariModel::parse(&model_name)
        .unwrap_or_else(|| {
            eprintln!("Modèle inconnu : {model_name}");
            usage(&program);
        });
    let profile = model.profile();
    eprintln!(
        "Modèle : {} (RAM {} Ko, Blitter {})",
        profile.name,
        profile.ram_size / 1024,
        if profile.has_blitter { "présent" } else { "absent" }
    );

    let rom = std::fs::read(rom_path).unwrap_or_else(|e| {
        eprintln!("Impossible de lire la ROM {rom_path} : {e}");
        std::process::exit(1);
    });

    if rom.len() < 4 {
        eprintln!("ROM trop courte pour contenir un en-tête TOS (os_version)");
        std::process::exit(1);
    }
    // `os_version` (BCD) est au décalage 2 du header TOS, documenté
    // publiquement (ex: 0x0162 pour TOS 1.62) : TOS >= 1.06 est mappé à
    // 0xE00000 sur ST/STE réel, TOS <= 1.04 à 0xFC0000 (voir `set_rom_base`).
    // Indépendant du modèle de machine (--model) : n'importe quel TOS
    // compatible peut être flashé dans une machine réelle donnée.
    let os_version = u16::from_be_bytes(rom[2..4].try_into().unwrap());
    let rom_base = if os_version >= 0x0106 {
        0x00E0_0000
    } else {
        rust68::systems::atari_st::DEFAULT_ROM_BASE
    };

    let ram_size = profile.ram_size;
    let mut st = AtariSt::from_model(profile, rom);
    st.set_rom_base(rom_base);
    if ntsc {
        st.set_video_mode(rust68::peripherals::atari_st::glue::VideoMode::Ntsc60);
        eprintln!("Mode vidéo : NTSC 60 Hz (--ntsc)");
    }
    if let Some(path) = disk_path {
        match load_floppy(path) {
            Ok(disk) => st.floppy_a = Some(disk),
            Err(e) => eprintln!("Impossible de charger le disque {path} : {e} (démarrage sans disquette)"),
        }
    }

    // `RUST68_CARTRIDGE_HGH`/`RUST68_CARTRIDGE_LOW` : deux images ROM 8 bits
    // séparées (octet haut / octet bas de chaque mot — format EPROM courant
    // des cartouches de diagnostic matériel ST/STE), entrelacées ici en une
    // seule image 16 bits native et mappée au port cartouche (`$FA0000`).
    if let (Ok(hgh_path), Ok(low_path)) = (
        std::env::var("RUST68_CARTRIDGE_HGH"),
        std::env::var("RUST68_CARTRIDGE_LOW"),
    ) {
        match (std::fs::read(&hgh_path), std::fs::read(&low_path)) {
            (Ok(hgh), Ok(low)) if hgh.len() == low.len() => {
                let mut cart = Vec::with_capacity(hgh.len() * 2);
                for (h, l) in hgh.iter().zip(low.iter()) {
                    cart.push(*h);
                    cart.push(*l);
                }
                eprintln!("Cartouche chargée : {} octets ({hgh_path} + {low_path})", cart.len());
                st.load_cartridge(cart);
            }
            (Ok(hgh), Ok(low)) => {
                eprintln!(
                    "Tailles HGH ({}) et LOW ({}) différentes, cartouche non chargée",
                    hgh.len(),
                    low.len()
                );
            }
            (hgh_res, low_res) => {
                if let Err(e) = hgh_res {
                    eprintln!("Impossible de lire {hgh_path} : {e}");
                }
                if let Err(e) = low_res {
                    eprintln!("Impossible de lire {low_path} : {e}");
                }
            }
        }
    }

    // TOS effectue au boot froid une détection de RAM installée en
    // provoquant délibérément des bus errors au-delà de la taille réelle
    // (restart via l'overlay ROM à chaque échec) — un algorithme précis et
    // notoirement délicat à reproduire à l'identique (même les émulateurs
    // établis y consacrent un effort dédié). Puisqu'on connaît déjà la
    // taille de RAM allouée, on pré-remplit les variables système du
    // "cookie" de warmstart (adresses documentées publiquement dans toute
    // référence TOS : `memvalid`/`memval2`/`memval3`/`phystop`) pour que le
    // TOS prenne directement le chemin "redémarrage à chaud" et saute
    // cette détection.
    //
    // `RUST68_COLD_BOOT=1` désactive ce raccourci (boot froid réel, plus
    // lent) — utile pour vérifier si un problème d'affichage vient d'une
    // initialisation normalement faite pendant le boot froid (palette,
    // config bureau...) que le chemin "redémarrage à chaud" suppose déjà
    // faite et saute.
    if std::env::var("RUST68_COLD_BOOT").is_err() {
        st.write32(0x420, 0x752019F3); // memvalid
        st.write32(0x43A, 0x237698AA); // memval2
        st.write32(0x51A, 0x5555AAAA); // memval3
        st.write32(0x42E, ram_size as u32); // phystop
        // Porte Timer-C de l'IKBD ($0EE4, RAM) : le TOS l'initialise
        // normalement à 0x1111 pendant sa propre mise en place de l'IKBD
        // (A5=0). Le gestionnaire de Timer C (200 Hz) fait `ASL.W
        // #3,($0EE4).L` puis saute le traitement des octets IKBD en attente
        // si le résultat est positif ou nul (BPL pris) — avec 0x1111,
        // ASL.W #3 = 0x8888 (bit 15 = 1, N posé, BPL non pris, octets bien
        // traités). Le raccourci "redémarrage à chaud" ci-dessus saute
        // exactement le code TOS qui pose normalement cette valeur, et la
        // RAM démarre à zéro (`AtariSt::new`) — sans ce pré-remplissage,
        // 0x0000 donnerait ASL.W #3 = 0x0000 (N=0, BPL pris), ce chemin de
        // traitement IKBD restant inactif jusqu'à ce que le TOS repasse par
        // là de lui-même. Identifié via le projet compagnon Stay (même
        // pré-remplissage, mêmes octets).
        st.write8(0x0EE4, 0x11);
        st.write8(0x0EE5, 0x11);
    }

    let mut cpu = Cpu::new();
    cpu.reset(&mut st);

    if std::env::var("RUST68_COLD_BOOT").is_err() && !st.has_cartridge() {
        // MEMORY_CONF ($FF8001) : même raccourci que memvalid/phystop
        // ci-dessus — sauf que le TOS, lui, écrit INCONDITIONNELLEMENT
        // MEMORY_CONF=0 très tôt au boot (avant même de consulter
        // memvalid), normalement corrigé seulement en toute fin de
        // l'algorithme de détection RAM que ce raccourci saute justement.
        // Un simple pré-remplissage se ferait donc aussitôt écraser :
        // `pin_memory_conf` fige la valeur pour qu'elle survive à cette
        // écriture (voir sa doc). Fait APRÈS `cpu.reset()`, pas avant :
        // désactiver l'overlay ROM à l'adresse 0 avant la lecture du
        // vecteur reset (SSP/PC) le ferait lire dans de la RAM encore
        // vierge plutôt que dans la ROM.
        //
        // Sauté si une cartouche est chargée (voir la doc de
        // `has_cartridge`) : elle fait sa propre détection RAM en écrivant
        // librement ce même registre, que figer casserait.
        if let Some(conf) = AtariSt::expected_memory_conf(ram_size) {
            st.pin_memory_conf(conf);
        }
    }

    // `RUST68_TRACE_ALL=1` : journalise CHAQUE transaction bus réelle (CPU
    // et Blitter — voir `RamBus` dans `systems::atari_st`, non couvert par
    // ceci puisque le Blitter accède à la mémoire via son propre bus, pas
    // celui du CPU) à sa taille exacte (.B/.W/.L), avec l'adresse de
    // l'instruction appelante et une étiquette de composant — voir
    // `rust68::trace::FileTraceSink` pour les variables d'environnement de
    // filtrage (indispensables : sans `RUST68_TRACE_ALL_MIN`/`_MAX`, une
    // session interactive complète génère des gigaoctets).
    let ram_len = st.ram().len();
    let rom_len = st.rom_len();
    let blitter_present = st.blitter_present();
    let mut trace_sink = FileTraceSink::from_env(Box::new(move |addr: u32| {
        AtariSt::describe_addr_static(ram_len, rom_base, rom_len, blitter_present, addr)
    }));
    let trace_cpu_wanted = trace_sink.as_ref().map(|s| s.wants_cpu_trace()).unwrap_or(false);

    let sdl_context = sdl2::init().expect("init SDL2");
    let video_subsystem = sdl_context.video().expect("init sous-système vidéo");
    let audio_subsystem = sdl_context.audio().expect("init sous-système audio");

    let window = video_subsystem
        .window("Rust68 — Atari ST", 640, 400)
        .position_centered()
        .build()
        .expect("création fenêtre");
    let mut canvas = window.into_canvas().build().expect("création canvas");
    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(PixelFormatEnum::RGB24, 640, 400)
        .expect("création texture");

    // Souris exclusive à la fenêtre tant qu'elle a le focus : confinée à la
    // fenêtre (`set_grab`) et curseur hôte caché (le GEM dessine le sien).
    // Volontairement PAS le mode "relatif" SDL2 (`set_relative_mouse_mode`)
    // — voir le commentaire sur `last_mouse_pos` ci-dessous : son recentrage
    // interne par warp a déjà causé une dérive diagonale sur ce backend: le
    // calcul de delta par position absolue existant reste inchangé,
    // seulement le confinement/l'affichage du curseur changent ici.
    // `RUST68_NO_MOUSE_GRAB=1` désactive tout ça (debug/automatisation) :
    // le confinement gêne un positionnement absolu externe (ex. `cliclick`
    // depuis un outil d'automatisation, qui ne peut plus faire sortir le
    // curseur hôte de la fenêtre pour ensuite le repositionner ailleurs).
    let mouse_grab_enabled = std::env::var("RUST68_NO_MOUSE_GRAB").is_err();
    if mouse_grab_enabled {
        canvas.window_mut().set_grab(true);
        sdl_context.mouse().show_cursor(false);
    }

    let desired_spec = AudioSpecDesired {
        freq: Some(AUDIO_SAMPLE_RATE),
        // Stéréo (pas mono) : le son DMA (STE) a de vrais canaux gauche/
        // droite indépendants (voir `DmaSound::next_sample`), avec des
        // volumes gauche/droite indépendants pilotés par le Microwire/
        // LMC1992 (voir `Microwire::left_gain`/`right_gain`) — la cartouche
        // de diagnostic usine STe (test "Stereo 1 kHz/500 Hz tones") en
        // dépend justement pour faire entendre 2 tons à des volumes
        // distincts. Les fusionner en un seul canal mono, comme avant,
        // rendait cette distinction impossible à entendre.
        channels: Some(2),
        samples: Some(1024),
    };
    let audio_queue: AudioQueue<i16> = audio_subsystem
        .open_queue(None, &desired_spec)
        .expect("ouverture file audio");
    // Lecture démarrée seulement une fois `AUDIO_QUEUE_PRIME_WATERMARK`
    // atteint (voir sa doc) — pas de `resume()` ici.
    let mut audio_started = false;
    let mut drive_sound = load_drive_sound(AUDIO_SAMPLE_RATE as u32);

    let mut event_pump = sdl_context.event_pump().expect("event pump");
    let mut mouse_left = false;
    let mut mouse_right = false;
    // Delta calculé nous-mêmes à partir de la position absolue (`x`/`y` de
    // l'évènement), plutôt que de s'appuyer sur `xrel`/`yrel` en mode
    // souris relatif SDL2 : ce mode recentre le curseur hôte en continu par
    // un warp, et selon la plateforme/le backend (notamment `sdl2-compat`,
    // une couche de compatibilité par-dessus SDL3, pas SDL2 d'origine) ce
    // warp peut lui-même générer un évènement de mouvement synthétique —
    // observé en pratique : dérive continue en diagonale au lieu de suivre
    // la souris. `None` tant qu'aucune position n'a encore été vue, pour ne
    // pas envoyer un premier delta énorme depuis une origine arbitraire.
    let mut last_mouse_pos: Option<(i32, i32)> = None;
    // La fenêtre affiche toujours 640x400 (voir `render_frame`), mais la
    // résolution ST logique peut être plus petite (320x200 en basse
    // résolution) : un mouvement en pixels de FENÊTRE doit être mis à
    // l'échelle en pixels ST LOGIQUES avant d'être envoyé à l'IKBD, sinon
    // la souris se déplace 2x plus vite côté ST que ce que montre
    // visuellement la fenêtre — le curseur GEM atteint (et dépasse) le bord
    // de l'écran bien avant que le curseur hôte n'y arrive vraiment. Le
    // reliquat fractionnaire (`mouse_scale_carry`) est reporté d'un
    // évènement à l'autre pour ne pas perdre les petits mouvements lents
    // par troncature répétée.
    let mut mouse_scale_carry = (0.0f64, 0.0f64);
    // Mouvement souris accumulé (en pixels ST logiques, après mise à
    // l'échelle) depuis le dernier échantillonnage — injecté dans l'IKBD
    // une seule fois par trame plutôt qu'immédiatement à chaque évènement
    // SDL brut, comme le fait le firmware IKBD réel (échantillonnage au
    // rythme VBL, voir `Machine::flush_input_vbl`/`IKBD_VBL()` de Steem
    // SSE dans le projet compagnon Stay). Envoyer un paquet par évènement
    // SDL — potentiellement bien plus fréquent que le ~50 Hz réel — a pu
    // désynchroniser le suivi du curseur GEM par rapport au vrai matériel.
    let mut mouse_dx_acc = 0i32;
    let mut mouse_dy_acc = 0i32;

    // `profile.cpu_hz` : voir sa doc dans `model.rs` — informatif seulement
    // pour l'instant (tous les profils renvoient 8 MHz), mais c'est déjà la
    // seule source de vérité pour ce calcul plutôt qu'une constante séparée
    // qui pourrait diverger le jour où un mode CPU accéléré sera modélisé.
    let cycles_per_sample = profile.cpu_hz as f64 / AUDIO_SAMPLE_RATE as f64;
    let mut audio_cycle_acc = 0.0f64;
    let mut audio_buffer: Vec<i16> = Vec::with_capacity(2048);
    // Bloqueur DC (filtre passe-haut un pôle) sur le mélange final,
    // gauche/droite séparément : notre mixage PSG (`mix_sample`) centre le
    // niveau total des 3 canaux sur une constante fixe plutôt que sur la
    // moyenne réelle (glissante) du signal, ce qui laisse un biais continu
    // (DC) chaque fois qu'un nombre de canaux actifs différent de la
    // moyenne théorique joue — visible comme une raie à 0 Hz au spectre, et
    // audible comme un « pop » à chaque démarrage/arrêt de tonalité. Un
    // bloqueur DC standard élimine ce biais dynamiquement, quelle que soit
    // sa cause exacte, plutôt que de chercher une constante de centrage
    // parfaite (impossible : la bonne valeur dépend du nombre de canaux
    // actifs à cet instant précis, pas d'une constante globale).
    let mut dc_blocker_left = DcBlocker::new();
    let mut dc_blocker_right = DcBlocker::new();
    // Gain lissé (pas appliqué tel quel à chaque échantillon) : le Microwire
    // peut changer de volume À TOUT MOMENT du signal, y compris en plein
    // milieu d'une période de tonalité non nulle — un saut instantané de
    // gain à cet instant précis produit une discontinuité audible (un
    // « clic »), même si le volume cible lui-même est correct. C'est un
    // artefact bien connu des contrôles de volume numériques ("zipper
    // noise") : le vrai LMC1992 (silicium analogique) a lui-même une
    // certaine constante de temps de transition, donc lisser ici est plus
    // fidèle qu'un saut instantané, pas juste un correctif ad-hoc.
    // Initialisé au gain RÉEL de départ (pas 0) pour ne pas produire son
    // propre clic de démarrage en rampant depuis le silence.
    let mut gain_smooth_left = st.microwire.left_gain();
    let mut gain_smooth_right = st.microwire.right_gain();
    // `RUST68_RECORD_WAV=chemin.wav` : enregistre tel quel (après filtre DC
    // et mixage, avant `mute`) le flux stéréo 16 bits envoyé à la file audio
    // — pour inspecter le spectre du son réellement produit.
    let mut wav_recorder = std::env::var("RUST68_RECORD_WAV").ok().map(|path| {
        WavRecorder::create(&path).unwrap_or_else(|e| panic!("création RUST68_RECORD_WAV={path} : {e}"))
    });

    // `RUST68_DEBUG=1` : affiche un point d'avancement (pas exécutés, PC,
    // SR) une fois par seconde sur stderr — utile pour vérifier que
    // l'émulation progresse sans être bloquée, sans instrumenter chaque
    // instruction en usage normal.
    let debug = std::env::var("RUST68_DEBUG").is_ok();
    // `RUST68_DUMP_VIDEO=1` (avec `RUST68_DEBUG=1`) : dump hexadécimal des
    // 14 premières lignes de RAM vidéo à partir de `video_base`, une fois
    // par seconde — pour inspecter le contenu réel pendant une corruption
    // d'affichage observée visuellement (indépendant du clavier : se
    // déclenche automatiquement, pas besoin d'envoyer une touche à la
    // fenêtre SDL2 au bon moment).
    let dump_video = std::env::var("RUST68_DUMP_VIDEO").is_ok();
    // `RUST68_TRACE_STEPS=N` : trace pc/sr/opcode des N premiers pas CPU
    // (diagnostic ponctuel, ex. pour observer la détection RAM au boot
    // froid instruction par instruction).
    let trace_steps: u64 = std::env::var("RUST68_TRACE_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let trace_stride: u64 = std::env::var("RUST68_TRACE_STRIDE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    // `RUST68_WATCH_VIDEO=1` : surveille la zone RAM vidéo lignes 11-66
    // (celle où la corruption d'affichage a été observée) et logue le PC
    // de l'instruction qui vient de la modifier, la première fois où son
    // contenu change — pour identifier l'instruction TOS responsable sans
    // avoir à comparer manuellement des dumps avant/après.
    let watch_video = std::env::var("RUST68_WATCH_VIDEO").is_ok();
    let mut watch_video_hash: Option<u64> = None;
    let mut watch_video_hits: u32 = 0;
    let mut watch_video_last_cycles: u64 = 0;
    let mut step_count: u64 = 0;
    let mut last_report = std::time::Instant::now();
    let last_report_start = std::time::Instant::now();

    // `RUST68_WATCH_RANGE=debut,fin` (hex, ex: "0fb900,0fc900") : surveille
    // OCTET PAR OCTET (pas juste un hash global) une zone précise de RAM
    // vidéo, et logue pc/adresse/avant/après pour CHAQUE octet modifié —
    // pour repérer exactement quelle instruction écrit quoi, et quand,
    // dans le contenu déjà en place avant qu'un blit de "teinte" (hop=1)
    // ne s'applique dessus (diagnostic du menu déroulant "Visualisation").
    let watch_range: Option<(u32, u32)> = std::env::var("RUST68_WATCH_RANGE").ok().and_then(|s| {
        let (a, b) = s.split_once(',')?;
        Some((
            u32::from_str_radix(a.trim(), 16).ok()?,
            u32::from_str_radix(b.trim(), 16).ok()?,
        ))
    });
    let mut watch_range_snapshot: Vec<u8> = Vec::new();
    if let Some((start, end)) = watch_range {
        watch_range_snapshot = (start..end).map(|a| st.read8(a)).collect();
    }
    let mut watch_range_hits: u64 = 0;

    // `RUST68_TRACE_FILL=1` : à chaque entrée dans la routine de
    // remplissage rectangulaire logicielle ($E12C40, identifiée via
    // RUST68_WATCH_VIDEO comme responsable de la corruption du menu),
    // dump l'état complet des registres + les 2 mots au sommet de la
    // pile (adresse de retour probable si entrée par JSR/BSR) — pour
    // comparer avec un run de référence (Hatari) au même point d'entrée.
    let trace_fill = std::env::var("RUST68_TRACE_FILL").is_ok();
    const TRACE_FILL_PCS: [u32; 4] = [0xE12C40, 0xE12C4E, 0xE12C56, 0xE12C62];
    let mut trace_fill_hits: [u32; TRACE_FILL_PCS.len()] = [0; TRACE_FILL_PCS.len()];
    const TRACE_FILL_MAX_HITS_EACH: u32 = 6;

    // `RUST68_TRACE_BLITDET=1` : trace la routine de détection matérielle du
    // Blitter ($E01062, identifiée en désassemblant le ROM TOS) — probe
    // volontaire d'un bus-error sur TST.W $FFFF8A00 (registre demi-teinte 0
    // du Blitter). Logue l'entrée (a0=0 attendu) et la sortie (D0=2 si
    // Blitter détecté, D0=0 sinon) pour vérifier si Rust68 déclenche par
    // erreur un vrai bus-error là où le silicium réel ne le ferait pas.
    let trace_blitdet = std::env::var("RUST68_TRACE_BLITDET").is_ok();
    let mut blitdet_hits: u32 = 0;

    // `RUST68_TRACE_DISPATCH=1` : trace le `jmp (a5)` en $E0B4AA qui choisit,
    // via un pointeur de fonction lu à $9A(A4), entre la routine Blitter
    // matérielle ($E0B4AC) et le repli logiciel ($E12C1C) — logue A4, le
    // pointeur lu à $9A(A4) (donc A5 juste avant le jmp) et D4/D6/A0 pour
    // voir si le pointeur lui-même ou les paramètres amont sont corrompus.
    let trace_dispatch = std::env::var("RUST68_TRACE_DISPATCH").is_ok();
    let mut dispatch_hits: u32 = 0;

    // `RUST68_TRACE_SIM=1` : capture l'état complet à l'entrée d'un appel
    // ($E12C1C, avant toute mutation de registre par la routine) puis, à sa
    // sortie (RTS en $E12C4C/$E12C7C/$E12C92), dump les octets réellement
    // écrits en RAM vidéo à partir de l'A1 d'origine — pour rejouer
    // l'algorithme à la main (Python) et comparer au résultat réel, sans
    // dépendre d'une référence Hatari.
    let trace_sim = std::env::var("RUST68_TRACE_SIM").is_ok();
    // `RUST68_TRACE_CURSOR=1` : dump l'état complet à chaque exécution de
    // $E1366C (écriture d'un mot du sprite curseur — voir désassemblage
    // dans ETAT.md), pour comparer registre par registre entre une
    // position "propre" (loin du menu) et une position "corrompue" (près
    // du menu / clic sur icône).
    let trace_cursor = std::env::var("RUST68_TRACE_CURSOR").is_ok();
    // `RUST68_TRACE_BLIT_CALLER=1` : à l'entrée de la routine de
    // déclenchement du blit curseur ($E0B160, TOS 1.62), logue l'adresse
    // de retour (sommet de pile) et D7 (futur Y_COUNT, avant le calcul
    // conditionnel) pour remonter à l'appelant qui fournit ce paramètre.
    let trace_blit_caller = std::env::var("RUST68_TRACE_BLIT_CALLER").is_ok();
    let mut blit_caller_hits: u32 = 0;
    let mut cursor_hits: u32 = 0;
    // `RUST68_TRACE_CA6A=1` : à CHAQUE entrée dans la boucle de "skew
    // logiciel" ($E0CA6A, dispatch OR/AND/NOT/XOR avec décalage bit à bit,
    // implémentation CPU pure — voir investigation menu "Visualisation")
    // logue l'adresse de retour (sommet de pile, seulement au tout premier
    // passage de chaque appel — a1 encore à sa valeur de départ) ainsi que
    // a1 (destination courante) pour identifier l'appelant et corréler
    // avec la zone mémoire touchée.
    let trace_ca6a = std::env::var("RUST68_TRACE_CA6A").is_ok();
    let mut ca6a_hits: u32 = 0;
    let mut ca6a_last_ret: u32 = 0;
    // Trace pas-à-pas (instruction par instruction) de la routine logicielle
    // de "skew" du blitter (0xE0CA6A/0xE0CAB8), armée sur le premier appel
    // seulement, pour observer d6 (décalage), les mots source lus via (a0)+
    // et le résultat combiné réellement produit à l'exécution.
    let trace_ca6a_steps = std::env::var("RUST68_TRACE_CA6A_STEPS").is_ok();
    let mut ca6a_step_budget: u32 = 0;
    let mut ca6a_step_armed = false;
    // Trace l'entrée dans le dispatcher de dessin de texte/motif à
    // 0xE0E480 (table de saut sur 8 entrées indexée via d6/d7, alimentant
    // 0xE0E4B6/0xE0E4E6) : ces deux routines s'exécutent deux fois moins
    // souvent en STE qu'en ST pour l'ouverture du même menu — objectif :
    // voir si les registres d'entrée diffèrent déjà à ce point.
    let trace_e480 = std::env::var("RUST68_TRACE_E480").is_ok();
    let mut e480_hits: u32 = 0;
    // Trace la routine de remplissage à motif (TOS 1.62, 0xE10B54-0xE10CA8)
    // à ses trois points de déclenchement Blitter (TAS.B des branches
    // "motif standard via table", "motif standard via copie directe" et
    // "motif personnalisé/largeur variable") : objectif, retrouver dans QUEL
    // appelant (adresse de retour) et avec quels paramètres d'objet (a2)
    // naît l'appel hop=1 x=8 y=9 responsable du remplissage en damier
    // observé à la place du texte du menu — corrélé dans le MÊME run que
    // la trace complète des paramètres Blitter, pour éviter le piège
    // méthodologique d'une comparaison entre deux runs séparés (la position
    // de la souris fait légèrement varier l'alignement pixel d'un
    // lancement manuel à l'autre).
    let trace_fillpat = std::env::var("RUST68_TRACE_FILLPAT").is_ok();
    let mut fillpat_hits: u32 = 0;
    // `RUST68_TRACE_IKBD_READER=1` : logue le PC + SR de l'instruction qui
    // vient de lire ACIA_KEYBOARD_DATA (RDRF retombé pendant ce pas) —
    // contrairement à `RUST68_TRACE_IKBD` (côté bus, sait QUOI a été lu
    // mais pas QUI l'a lu), ceci permet de vérifier que c'est bien le
    // gestionnaire d'interruption clavier/souris du TOS qui consomme les
    // octets IKBD, et pas une boucle d'attente sans rapport (SR indique le
    // niveau IPL courant et le bit S : à l'intérieur d'un vrai handler
    // d'interruption MFP, IPL doit valoir 6 et S=1).
    let trace_ikbd_reader = std::env::var("RUST68_TRACE_IKBD_READER").is_ok();
    let mut ikbd_reader_hits: u32 = 0;
    // `RUST68_TRACE_IKBD_DISPATCH=1` : logue la cible du `jsr (a2)` en
    // $E03DC4 (fin d'assemblage d'un paquet IKBD, TOS 1.62) — le
    // gestionnaire spécifique au type de paquet (souris/joystick/clavier)
    // qui reçoit le paquet assemblé en `a0` — pour savoir où continue le
    // traitement d'un paquet souris complet.
    let trace_ikbd_dispatch = std::env::var("RUST68_TRACE_IKBD_DISPATCH").is_ok();
    // `RUST68_MUTE=1` : coupe la sortie audio (débogage vidéo sans le bruit
    // parasite du son STE/DMA — bug distinct, pas encore investigué).
    let mute = std::env::var("RUST68_MUTE").is_ok();
    let mut ikbd_dispatch_hits: u32 = 0;
    let mut sim_entry: Option<([u32; 8], [u32; 8])> = None;
    let mut sim_hits: u32 = 0;
    let mut frame_count_dbg: u32 = 0;

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                // Cmd+Shift+F10 : bascule manuelle de l'exclusivité souris
                // sans perdre le focus de la fenêtre (récupérer le curseur
                // réel temporairement, par ex. pour changer d'application).
                // Interceptée avant la remontée générale des touches vers
                // l'IKBD ci-dessous : ne doit jamais atteindre le clavier ST.
                Event::KeyDown {
                    scancode: Some(Scancode::F10),
                    keymod,
                    repeat: false,
                    ..
                } if mouse_grab_enabled
                    && keymod.intersects(Mod::LGUIMOD | Mod::RGUIMOD)
                    && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                {
                    let now_grabbed = !canvas.window().grab();
                    canvas.window_mut().set_grab(now_grabbed);
                    sdl_context.mouse().show_cursor(!now_grabbed);
                    if debug {
                        eprintln!("[souris] exclusivité basculée -> {now_grabbed}");
                    }
                }
                Event::Window { win_event: WindowEvent::FocusGained, .. } if mouse_grab_enabled => {
                    canvas.window_mut().set_grab(true);
                    sdl_context.mouse().show_cursor(false);
                }
                Event::Window { win_event: WindowEvent::FocusLost, .. } if mouse_grab_enabled => {
                    canvas.window_mut().set_grab(false);
                    sdl_context.mouse().show_cursor(true);
                }
                // `RUST68_RAM_DUMP_KEY=1` : F2 écrit un instantané brut de
                // toute la RAM installée dans `RUST68_RAM_DUMP_PATH` — pour
                // rejouer ensuite une trace `RUST68_TRACE_BLITTER` depuis un
                // état RAM complet et réel, sans le surcoût prohibitif de
                // `RUST68_WATCH_RANGE` sur toute la RAM (qui rend le boot
                // trop lent pour être interactif).
                Event::KeyDown {
                    scancode: Some(Scancode::F2),
                    repeat: false,
                    ..
                } if std::env::var("RUST68_RAM_DUMP_KEY").is_ok() => {
                    if let Ok(path) = std::env::var("RUST68_RAM_DUMP_PATH") {
                        match std::fs::write(&path, st.ram()) {
                            Ok(()) => eprintln!("[ram] instantané ({} octets) écrit dans {path}", st.ram().len()),
                            Err(e) => eprintln!("[ram] échec d'écriture de l'instantané : {e}"),
                        }
                    }
                }
                // `RUST68_FB_DUMP_KEY=1` : F3 écrit `st.framebuffer` (déjà
                // calculé par `Shifter::render_scanline`, avant tout passage
                // par la texture SDL2) en PPM brut dans
                // `RUST68_FB_DUMP_PATH` — pour comparer directement ce que
                // notre moteur calcule en interne à une reconstruction de
                // référence, sans dépendre de l'affichage SDL2 lui-même.
                Event::KeyDown {
                    scancode: Some(Scancode::F3),
                    repeat: false,
                    ..
                } if std::env::var("RUST68_FB_DUMP_KEY").is_ok() => {
                    if let Ok(path) = std::env::var("RUST68_FB_DUMP_PATH") {
                        let h = st.framebuffer.len();
                        let w = st.framebuffer.first().map(|r| r.len()).unwrap_or(0);
                        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
                        for row in &st.framebuffer {
                            for &(r, g, b) in row {
                                out.push(r);
                                out.push(g);
                                out.push(b);
                            }
                        }
                        match std::fs::write(&path, &out) {
                            Ok(()) => eprintln!("[fb] instantané ({w}x{h}) écrit dans {path}"),
                            Err(e) => eprintln!("[fb] échec d'écriture de l'instantané : {e}"),
                        }
                    }
                }
                Event::KeyDown {
                    scancode: Some(sc),
                    repeat: false,
                    ..
                } => {
                    if let Some(code) = st_scancode(sc) {
                        st.ikbd.key_make(code);
                    }
                }
                Event::KeyUp {
                    scancode: Some(sc), ..
                } => {
                    if let Some(code) = st_scancode(sc) {
                        st.ikbd.key_break(code);
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    if debug {
                        eprintln!("[mouse] motion event reçu x={x} y={y} last_mouse_pos={last_mouse_pos:?}");
                    }
                    if let Some((last_x, last_y)) = last_mouse_pos {
                        use rust68::peripherals::atari_st::shifter::Resolution;
                        let (logical_w, logical_h) = match st.shifter.resolution() {
                            Resolution::Low => (320.0, 200.0),
                            Resolution::Medium => (640.0, 200.0),
                            Resolution::High => (640.0, 400.0),
                        };
                        let raw_dx = (x - last_x) as f64 * (logical_w / 640.0) + mouse_scale_carry.0;
                        let raw_dy = (y - last_y) as f64 * (logical_h / 400.0) + mouse_scale_carry.1;
                        let dx = raw_dx.trunc();
                        let dy = raw_dy.trunc();
                        mouse_scale_carry = (raw_dx - dx, raw_dy - dy);
                        mouse_dx_acc += dx as i32;
                        mouse_dy_acc += dy as i32;
                    }
                    last_mouse_pos = Some((x, y));
                }
                Event::MouseButtonDown { mouse_btn, .. } => {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Left => mouse_left = true,
                        sdl2::mouse::MouseButton::Right => mouse_right = true,
                        _ => {}
                    }
                    if debug {
                        eprintln!("[mouse] down {mouse_btn:?} -> left={mouse_left} right={mouse_right}");
                    }
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    match mouse_btn {
                        sdl2::mouse::MouseButton::Left => mouse_left = false,
                        sdl2::mouse::MouseButton::Right => mouse_right = false,
                        _ => {}
                    }
                    if debug {
                        eprintln!("[mouse] up {mouse_btn:?} -> left={mouse_left} right={mouse_right}");
                    }
                }
                _ => {}
            }
        }

        // Un seul échantillonnage souris par trame (voir la doc de
        // `mouse_dx_acc`) : injecte le mouvement accumulé depuis la
        // dernière trame, quel que soit le nombre d'évènements SDL bruts
        // reçus entre-temps.
        queue_mouse_move(&mut st.ikbd, mouse_dx_acc, mouse_dy_acc, mouse_left, mouse_right);
        mouse_dx_acc = 0;
        mouse_dy_acc = 0;

        // Fait tourner le CPU jusqu'à ce qu'une trame vidéo complète se soit
        // écoulée (détecté via le compteur de trame du GLUE), en échantillonnant
        // l'audio au fil de l'eau.
        let frame_before = st.glue.frame_count();
        loop {
            if step_count < trace_steps && step_count % trace_stride == 0 {
                eprintln!(
                    "[trace] step={step_count} pc={:#08x} sr={:#06x} opcode={:#06x} a7={:#08x} ssp={:#08x}",
                    cpu.pc,
                    cpu.sr,
                    (st.read8(cpu.pc) as u16) << 8 | st.read8(cpu.pc.wrapping_add(1)) as u16,
                    cpu.a[7],
                    cpu.ssp
                );
            }
            let pc_before = cpu.pc;

            if trace_blitdet && blitdet_hits < 4 && pc_before == 0xE0108A {
                blitdet_hits += 1;
                eprintln!(
                    "[blitdet] hit #{blitdet_hits} pc={pc_before:#08x} d0={:#010x} (2=blitter détecté, 0=absent/bus-error) sr={:#06x}",
                    cpu.d[0], cpu.sr
                );
            }

            if trace_dispatch && dispatch_hits < 20 && pc_before == 0xE0B4AA {
                dispatch_hits += 1;
                eprintln!(
                    "[dispatch] hit #{dispatch_hits} pc={pc_before:#08x} a4={:#010x} a5(cible du jmp)={:#010x} d4={:#010x} d6={:#010x} a0={:#010x} d0={:#010x}",
                    cpu.a[4], cpu.a[5], cpu.d[4], cpu.d[6], cpu.a[0], cpu.d[0]
                );
            }

            if trace_sim && sim_hits < 200 {
                if pc_before == 0xE12C1C && sim_entry.is_none() {
                    sim_entry = Some((cpu.d, cpu.a));
                } else if sim_entry.is_some()
                    && (pc_before == 0xE12C4C || pc_before == 0xE12C7C || pc_before == 0xE12C92)
                {
                    let (entry_d, entry_a) = sim_entry.take().unwrap();
                    sim_hits += 1;
                    let a1_orig = entry_a[1];
                    let bytes: Vec<u8> = (0..64u32)
                        .map(|o| st.read8(a1_orig.wrapping_add(o)))
                        .collect();
                    if bytes.iter().any(|&b| b != 0) {
                        eprintln!(
                            "[sim] #{sim_hits} entree_d={:x?} entree_a={:x?} sortie_pc={pc_before:#08x} a1_origine={a1_orig:#010x} octets={}",
                            entry_d, entry_a,
                            bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("")
                        );
                    }
                }
            }

            if trace_fill {
                if let Some(idx) = TRACE_FILL_PCS.iter().position(|&p| p == pc_before) {
                    if trace_fill_hits[idx] < TRACE_FILL_MAX_HITS_EACH {
                        trace_fill_hits[idx] += 1;
                        let sp = cpu.a[7];
                        let ret_word0 = st.read16(sp);
                        let ret_word1 = st.read16(sp.wrapping_add(2));
                        let video_base = st.shifter.video_base();
                        eprintln!(
                            "[fill] hit #{} pc={pc_before:#08x} video_base={video_base:#010x} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x} d5={:#010x} d6={:#010x} d7={:#010x} a0={:#010x} a1={:#010x} a2={:#010x} a3={:#010x} a4={:#010x} a5={:#010x} a6={:#010x} a7={sp:#010x} sr={:#06x} pile[a7..a7+4]={ret_word0:#06x}{ret_word1:04x}",
                            trace_fill_hits[idx],
                            cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.d[7],
                            cpu.a[0], cpu.a[1], cpu.a[2], cpu.a[3], cpu.a[4], cpu.a[5], cpu.a[6],
                            cpu.sr,
                        );
                    }
                }
            }

            if trace_ikbd_dispatch && ikbd_dispatch_hits < 20 && pc_before == 0xE03DC4 {
                ikbd_dispatch_hits += 1;
                let a0 = cpu.a[0];
                let bytes: Vec<u8> = (0..4).map(|o| st.read8(a0.wrapping_add(o))).collect();
                eprintln!(
                    "[ikbd-dispatch] hit #{ikbd_dispatch_hits} cible=jsr(a2)={:#010x} a0(paquet)={a0:#010x} octets={}",
                    cpu.a[2],
                    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
                );
            }
            if trace_ikbd_dispatch && ikbd_dispatch_hits < 20 && pc_before == 0xE10CAA {
                let read32 = |st: &mut rust68::systems::atari_st::AtariSt, a: u32| {
                    ((st.read8(a) as u32) << 24)
                        | ((st.read8(a + 1) as u32) << 16)
                        | ((st.read8(a + 2) as u32) << 8)
                        | st.read8(a + 3) as u32
                };
                eprintln!(
                    "[ikbd-hooks] vecteur_bouton($2AE2)={:#010x} vecteur_position($2AEA)={:#010x} vecteur_notif($2AE6)={:#010x} pos_actuelle($28C2)={:#010x}",
                    read32(&mut st, 0x2AE2),
                    read32(&mut st, 0x2AEA),
                    read32(&mut st, 0x2AE6),
                    read32(&mut st, 0x28C2),
                );
            }

            if trace_blit_caller
                && blit_caller_hits < 300
                && matches!(pc_before, 0xE0B1B2 | 0xE0B518 | 0xE10C02)
            {
                blit_caller_hits += 1;
                let sp = cpu.a[7];
                let ret_addr = st.read32(sp);
                use rust68::peripherals::atari_st::blitter::reg as blitreg;
                let y_count = ((st.blitter.read(blitreg::Y_COUNT) as u16) << 8)
                    | st.blitter.read(blitreg::Y_COUNT1) as u16;
                let x_count = ((st.blitter.read(blitreg::X_COUNT) as u16) << 8)
                    | st.blitter.read(blitreg::X_COUNT1) as u16;
                eprintln!(
                    "[blit-caller] hit #{blit_caller_hits} pc={pc_before:#08x} retour={ret_addr:#010x} X_COUNT={x_count} Y_COUNT={y_count} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x} d6={:#010x} d7={:#010x} a4={:#010x}",
                    cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[4], cpu.d[6], cpu.d[7], cpu.a[4],
                );
            }

            if trace_e480 && e480_hits < 500 && pc_before == 0xE0E480 {
                e480_hits += 1;
                let sp = cpu.a[7];
                let ret_addr = st.read32(sp);
                let a1 = cpu.a[1];
                let before: Vec<String> = (0..8).map(|o| format!("{:02x}", st.read8(a1.wrapping_add(o)))).collect();
                eprintln!(
                    "[e480] appel #{e480_hits} retour={ret_addr:#010x} a4={:#010x} a1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x} d6={:#010x} d7={:#010x} avant(a1..+8)={}",
                    cpu.a[4], cpu.a[1], cpu.d[2], cpu.d[3], cpu.d[4], cpu.d[6], cpu.d[7], before.join(" "),
                );
            }

            if trace_fillpat
                && fillpat_hits < 500
                && matches!(pc_before, 0xE10C04 | 0xE10C44 | 0xE10C94)
            {
                fillpat_hits += 1;
                let sp = cpu.a[7];
                // Au premier passage sur ce TAS.B, l'adresse de retour de la
                // routine PARTAGÉE (e10b54-e10ca8) est au sommet de pile ;
                // l'appelant de CETTE routine (celui qui nous intéresse
                // vraiment, pour savoir QUI a demandé ce remplissage) est
                // au niveau suivant.
                let ret_addr = st.read32(sp);
                let a2 = cpu.a[2];
                let obj_bytes: Vec<u32> = (0..4).map(|i| st.read16(a2.wrapping_add(i * 2)) as u32).collect();
                use rust68::peripherals::atari_st::blitter::reg as blitreg;
                let hop = st.blitter.read(blitreg::HOP);
                let op = st.blitter.read(blitreg::OP);
                let x_count = ((st.blitter.read(blitreg::X_COUNT) as u16) << 8)
                    | st.blitter.read(blitreg::X_COUNT1) as u16;
                let y_count = ((st.blitter.read(blitreg::Y_COUNT) as u16) << 8)
                    | st.blitter.read(blitreg::Y_COUNT1) as u16;
                let long = |a: u32| {
                    ((st.blitter.read(a) as u32) << 24)
                        | ((st.blitter.read(a + 1) as u32) << 16)
                        | ((st.blitter.read(a + 2) as u32) << 8)
                        | st.blitter.read(a + 3) as u32
                };
                let dst_addr = long(blitreg::DST_ADDR);
                let dst_x_inc = ((st.blitter.read(blitreg::DST_X_INC) as u16) << 8)
                    | st.blitter.read(blitreg::DST_X_INC1) as u16;
                let dst_y_inc = ((st.blitter.read(blitreg::DST_Y_INC) as u16) << 8)
                    | st.blitter.read(blitreg::DST_Y_INC1) as u16;
                eprintln!(
                    "[fillpat] hit #{fillpat_hits} branche={pc_before:#08x} retour_routine_partagee={ret_addr:#010x} hop={hop} op={op:#04x} x={x_count} y={y_count} dst_addr={dst_addr:#010x} dst_xinc={dst_x_inc:#06x} dst_yinc={dst_y_inc:#06x} a2={a2:#010x} d3={:#010x} d4={:#010x} d5={:#010x} d6={:#010x} d7={:#010x}",
                    cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.d[7],
                );
                let _ = obj_bytes;
            }

            if trace_ca6a && ca6a_hits < 500 && pc_before == 0xE0CA6A {
                let sp = cpu.a[7];
                let ret_addr = st.read32(sp);
                if ret_addr != ca6a_last_ret {
                    ca6a_last_ret = ret_addr;
                    ca6a_hits += 1;
                    eprintln!(
                        "[ca6a] appel #{ca6a_hits} retour={ret_addr:#010x} a1(dst)={:#010x} a2={:#010x} a3(mode)={:#010x} d3={:#010x} d4(stride)={:#010x} d5(compte-1)={:#010x} d6(skew)={:#010x} a0(src)={:#010x}",
                        cpu.a[1], cpu.a[2], cpu.a[3], cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.a[0],
                    );
                    eprintln!(
                        "[ca6a-video] resolution={:?} video_base={:#010x}",
                        st.shifter.resolution(),
                        st.shifter.video_base(),
                    );
                    let table_base = cpu.a[0] & !1u32;
                    let words: Vec<String> = (0..16)
                        .map(|i| format!("{:04x}", st.read16(table_base.wrapping_add(i * 2))))
                        .collect();
                    eprintln!(
                        "[ca6a-table] base={table_base:#010x} 16 mots = {}",
                        words.join(" ")
                    );
                    if trace_ca6a_steps && !ca6a_step_armed {
                        ca6a_step_armed = true;
                        ca6a_step_budget = 400;
                    }
                }
            }

            if trace_cursor && cursor_hits < 200_000 && pc_before == 0xE1366C {
                let a5 = cpu.a[5];
                let read16 = |st: &mut rust68::systems::atari_st::AtariSt, a: u32| st.read16(a);
                let video_base = st.shifter.video_base();
                let a1 = cpu.a[1];
                let row = if a1 >= video_base { (a1 - video_base) / 160 } else { u32::MAX };
                // Ne logue que les écritures proches ou dans la barre de
                // menu (lignes 0-14) — les milliers d'autres (curseur
                // ailleurs sur le bureau) noieraient le signal.
                if row <= 14 {
                cursor_hits += 1;
                eprintln!(
                    "[cursor] hit #{cursor_hits} a1(dest)={a1:#010x} ligne_fb={row} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} a0={:#010x} shift(-24,a5)={:#06x} d2src(-20,a5)={:#06x} nmots(-12,a5)={:#06x} stride_src(-c,a5)={:#06x} stride_dst(-e,a5)={:#06x} op1(-26,a5)={:#06x} op2(-2a,a5)={:#06x} op3(-28,a5)={:#06x} op4(-2c,a5)={:#06x}",
                    cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.a[0],
                    read16(&mut st, a5.wrapping_sub(0x24)),
                    read16(&mut st, a5.wrapping_sub(0x20)),
                    read16(&mut st, a5.wrapping_sub(0x12)),
                    read16(&mut st, a5.wrapping_sub(0x0c)),
                    read16(&mut st, a5.wrapping_sub(0x0e)),
                    read16(&mut st, a5.wrapping_sub(0x26)),
                    read16(&mut st, a5.wrapping_sub(0x2a)),
                    read16(&mut st, a5.wrapping_sub(0x28)),
                    read16(&mut st, a5.wrapping_sub(0x2c)),
                );
                }
            }

            let rdrf_before = trace_ikbd_reader
                && ikbd_reader_hits < 40
                && st.acia_keyboard.read(rust68::peripherals::atari_st::acia::reg::CONTROL_STATUS) & 0x01 != 0;

            st.last_pc = cpu.pc;
            if let Some(sink) = trace_sink.as_mut() {
                if trace_cpu_wanted {
                    sink.cpu_step(cpu.pc);
                }
                sink.maybe_break(&cpu);
            }
            let sink_ref: Option<&mut dyn TraceSink> =
                trace_sink.as_mut().map(|s| s as &mut dyn TraceSink);
            let mut tracing_bus = TracingBus {
                inner: &mut st,
                sink: sink_ref,
                pc: cpu.pc,
            };
            let cycles = match cpu.step(&mut tracing_bus) {
                Ok(cycles) => cycles,
                Err(e) => {
                    eprintln!("Erreur CPU, arrêt : {e:?} pc={:#08x}", cpu.pc);
                    break 'running;
                }
            };

            if ca6a_step_budget > 0 {
                ca6a_step_budget -= 1;
                eprintln!(
                    "[ca6a-step] pc={pc_before:#010x} -> {:#010x} d0={:#010x} d1={:#010x} d2={:#010x} d3={:#010x} d4={:#010x} d5={:#010x} d6={:#010x} a0={:#010x} a1={:#010x} a2={:#010x}",
                    cpu.pc, cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.a[0], cpu.a[1], cpu.a[2],
                );
                if ca6a_step_budget == 0 {
                    ca6a_step_armed = false;
                }
            }

            if rdrf_before {
                let rdrf_after = st.acia_keyboard.read(rust68::peripherals::atari_st::acia::reg::CONTROL_STATUS) & 0x01 != 0;
                if !rdrf_after {
                    ikbd_reader_hits += 1;
                    let ipl = (cpu.sr >> 8) & 0x07;
                    let supervisor = cpu.sr & 0x2000 != 0;
                    eprintln!(
                        "[ikbd-reader] hit #{ikbd_reader_hits} pc={pc_before:#08x} sr={:#06x} ipl={ipl} superviseur={supervisor}",
                        cpu.sr
                    );
                }
            }

            st.tick(cycles);
            // `Cpu::step` ne prend jamais lui-même l'exception trace
            // (vecteur 9, bit T du SR) — voir sa doc : c'est à l'appelant de
            // vérifier `trace_pending` et d'appeler `take_trace_exception`
            // avant le step suivant. Ce binaire ne le faisait pas du tout
            // jusqu'ici : un programme qui pose T=1 (technique de
            // protection anti-copie/anti-débogueur courante — trouvé en
            // creusant pourquoi Rick_Dangerous.stx ne démarre jamais)
            // continuait alors à s'exécuter normalement au lieu de
            // déclencher un trap après CHAQUE instruction comme le
            // silicium réel, dérivant rapidement vers un flux d'exécution
            // qui n'a plus rien à voir avec le programme réel.
            if cpu.trace_pending {
                cpu.take_trace_exception(&mut st);
            }
            let drive_sound_events = st.wd1772.take_sound_events();
            if !drive_sound_events.is_empty() {
                drive_sound.handle_events(&drive_sound_events);
            }
            step_count += 1;

            if watch_video {
                let base = st.shifter.video_base();
                // Lignes 0-66 : inclut désormais la barre de menu elle-même
                // (lignes 0-10) en plus de la zone sous le menu, pour
                // repérer l'instruction qui écrit le curseur/la barre de
                // menu quand le curseur en approche (indice utilisateur :
                // le curseur devient partiellement jaune/rouge près du
                // menu, et au clic sur une icône).
                let start = base;
                let len: u32 = 67 * 160;
                let mut hash: u64 = 0xcbf29ce484222325;
                for o in 0..len {
                    hash ^= st.read8(start.wrapping_add(o)) as u64;
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                if watch_video_hash != Some(hash) {
                    if watch_video_hash.is_some() {
                        watch_video_hits += 1;
                        // `cpu.cycles` (et le delta depuis la dernière
                        // modification détectée) permet de mesurer combien de
                        // cycles CPU réels s'écoulent entre deux écritures
                        // successives dans cette zone (ex. les 4 plans d'un
                        // même sprite curseur) — à comparer aux 512
                        // cycles/ligne PAL pour juger si une séquence
                        // multi-plans réaliste peut effectivement chevaucher
                        // une frontière de ligne (question du "cursor turns
                        // yellow" : chevauchement de scanline pendant une
                        // mise à jour multi-plans non atomique).
                        let delta = cpu.cycles.wrapping_sub(watch_video_last_cycles);
                        eprintln!(
                            "[watch] zone vidéo (lignes 0-66) modifiée par l'instruction en pc={pc_before:#08x} (changement #{watch_video_hits}) cycles={} delta={delta}",
                            cpu.cycles
                        );
                    }
                    watch_video_hash = Some(hash);
                    watch_video_last_cycles = cpu.cycles;
                }
            }

            if let Some((start, end)) = watch_range {
                for (i, prev) in watch_range_snapshot.iter_mut().enumerate() {
                    let addr = start + i as u32;
                    let now = st.read8(addr);
                    if now != *prev {
                        watch_range_hits += 1;
                        eprintln!(
                            "[range] addr={addr:#08x} {:#04x}->{:#04x} pc={pc_before:#08x} (#{watch_range_hits})",
                            *prev, now
                        );
                        *prev = now;
                    }
                }
                let _ = end;
            }

            if debug && last_report.elapsed().as_secs_f64() >= 1.0 {
                eprintln!(
                    "steps={step_count} cycles={} mhz_effectif={:.3} pc={:#08x} sr={:#06x} video_base={:#08x} video_counter={:#08x} phystop={:#08x} v_bas_ad={:#08x}",
                    cpu.cycles,
                    cpu.cycles as f64 / 1_000_000.0 / last_report_start.elapsed().as_secs_f64(),
                    cpu.pc,
                    cpu.sr,
                    st.shifter.video_base(),
                    st.shifter.video_counter(),
                    st.read32(0x42E),
                    st.read32(0x44E)
                );
                if dump_video {
                    let base = st.shifter.video_base();
                    eprintln!("[dump] video_base={base:#08x}");
                    for line in 0..100usize {
                        if let Some(row) = st.framebuffer.get(line) {
                            let distinct: std::collections::BTreeSet<(u8, u8, u8)> = row.iter().copied().collect();
                            // N'affiche les octets bruts que pour les lignes
                            // "intéressantes" (plus de 3 couleurs) : évite de
                            // noyer le log pour les lignes déjà comprises
                            // (fond blanc/texte noir/vert uni).
                            if distinct.len() > 3 {
                                let addr = base.wrapping_add(line as u32 * 160);
                                let bytes: Vec<u8> = (0..160).map(|o| st.read8(addr.wrapping_add(o))).collect();
                                eprintln!(
                                    "[raw] ligne {line:02} +{:#06x} : {}",
                                    line * 160,
                                    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("")
                                );
                            }
                            eprintln!(
                                "[fb] ligne {line:02} : {} couleur(s) distincte(s) : {}",
                                distinct.len(),
                                distinct.iter().take(8).map(|(r, g, b)| format!("({r:02x},{g:02x},{b:02x})")).collect::<Vec<_>>().join(" ")
                            );
                        }
                    }
                }
                last_report = std::time::Instant::now();
            }

            audio_cycle_acc += cycles as f64;
            while audio_cycle_acc >= cycles_per_sample {
                audio_cycle_acc -= cycles_per_sample;
                let dma_lr = st.next_dma_sample(AUDIO_SAMPLE_RATE as u32);
                let ym_levels = st.ym2149.take_averaged_levels();
                let (raw_left, raw_right) = mix_sample(ym_levels, dma_lr);
                // Bruitage mécanique du lecteur : ambiance de fond mono,
                // mélangée telle quelle sur les deux canaux (pas de
                // filtre DC ni de gain Microwire — c'est un bruit
                // d'origine mécanique, pas un signal issu du LMC1992).
                let mut drive_sample = [0i32];
                drive_sound.mix_into(&mut drive_sample);
                let raw_left = raw_left + drive_sample[0] as f32;
                let raw_right = raw_right + drive_sample[0] as f32;
                // DC retiré AVANT le filtre Microwire (artefact de notre
                // mixage PSG, pas un phénomène physique que le volume/le
                // filtre devrait mettre à l'échelle) ; gain lissé (évite un
                // « clic » de zipper noise sur un changement de volume
                // brutal) puis injecté DANS le filtre graves/aigus (façon
                // Hatari, `DmaSnd_IIRfilterL`/`R` — le gain s'applique à
                // l'entrée du filtre, pas en aval, voir la doc de
                // `Microwire::filter_left`).
                let dc_left = dc_blocker_left.process(raw_left);
                let dc_right = dc_blocker_right.process(raw_right);
                gain_smooth_left += (st.microwire.left_gain() - gain_smooth_left) * GAIN_SMOOTH_COEFF;
                gain_smooth_right += (st.microwire.right_gain() - gain_smooth_right) * GAIN_SMOOTH_COEFF;
                let filtered_left = st.microwire.filter_left(dc_left, gain_smooth_left);
                let filtered_right = st.microwire.filter_right(dc_right, gain_smooth_right);
                let left = filtered_left.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                let right = filtered_right.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                // Entrelacé L,R,L,R... (file audio stéréo, voir `desired_spec`).
                audio_buffer.push(left);
                audio_buffer.push(right);
            }

            if st.glue.frame_count() != frame_before {
                break;
            }
        }

        if !audio_buffer.is_empty() {
            let mut recording_failed = false;
            if let Some(recorder) = wav_recorder.as_mut() {
                // Avant `mute` : on enregistre le son réellement produit,
                // pas un silence si l'utilisateur a coupé le son hôte.
                if let Err(e) = recorder.append(&audio_buffer) {
                    eprintln!("RUST68_RECORD_WAV : erreur d'écriture ({e}), enregistrement arrêté");
                    recording_failed = true;
                }
            }
            if recording_failed {
                wav_recorder = None;
            }
            // `mute` : n'envoie pas le son réel à SDL, mais continue de
            // remplir la file audio (avec du silence, même nombre
            // d'échantillons) — c'est le REMPLISSAGE de cette file qui
            // cadence la vitesse d'émulation (voir
            // AUDIO_QUEUE_HIGH_WATERMARK plus haut) ; sauter complètement
            // `queue_audio` (comme le faisait une version précédente)
            // laissait la file en permanence vide et supprimait donc aussi
            // ce cadencement, faisant tourner l'émulation bien plus vite
            // que la vitesse réelle.
            if mute {
                audio_buffer.iter_mut().for_each(|s| *s = 0);
            }
            let _ = audio_queue.queue_audio(&audio_buffer);
            audio_buffer.clear();
        }

        if !audio_started && audio_queue.size() >= AUDIO_QUEUE_PRIME_WATERMARK {
            audio_queue.resume();
            audio_started = true;
        }

        if std::env::var("RUST68_TRACE_PALETTE").is_ok() {
            frame_count_dbg += 1;
            if frame_count_dbg % 100 == 0 {
                eprintln!("[palette] frame={frame_count_dbg} {:?}", st.shifter.palette_raw());
            }
        }
        render_frame(&st, &mut texture);
        canvas.clear();
        let _ = canvas.copy(&texture, None, None);
        canvas.present();

        // Cadencement temps réel : on dort exactement l'écart entre le temps
        // CPU émulé (cpu.cycles / cpu_hz) et le temps réel écoulé depuis le
        // début (`last_report_start`), pour ne jamais émuler plus vite que
        // le temps réel. Remplace l'ancien cadencement indirect par le seul
        // remplissage de la file audio : ce dernier ne corrigeait qu'une
        // AVANCE sur le temps réel (sommeil tant que la file est trop
        // pleine), jamais un RETARD — un simple sommeil ponctuel trop long
        // (regroupement de minuteurs / mise en veille de l'app côté macOS,
        // entre autres) faisait alors dériver l'émulation en retard sur le
        // temps réel sans aucun rattrapage, vidant la file et provoquant un
        // décroché audio toutes les quelques secondes. Une référence
        // horloge murale directe s'auto-corrige : un sommeil trop long une
        // fois ne fait que réduire d'autant le suivant, sans dérive
        // cumulative — et si on est très en retard (fenêtre mise en arrière-
        // plan, point d'arrêt de débogueur...), on ne dort simplement pas le
        // temps de rattraper, sans à-coup de rattrapage brutal.
        let target_elapsed = Duration::from_secs_f64(cpu.cycles as f64 / profile.cpu_hz as f64);
        let real_elapsed = last_report_start.elapsed();
        if target_elapsed > real_elapsed {
            std::thread::sleep(target_elapsed - real_elapsed);
        }

        // Filet de sécurité : borne aussi la file audio elle-même (device
        // audio hôte anormalement lent à la vider, par exemple), au cas où
        // le cadencement horloge murale ci-dessus ne suffirait pas seul.
        while audio_queue.size() > AUDIO_QUEUE_HIGH_WATERMARK {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Mixe la PSG (YM2149, 3 canaux niveaux 0-31 déjà MOYENNÉS temporellement
/// depuis le dernier échantillon — voir [`Ym2149::take_averaged_levels`],
/// sans quoi une tonalité aiguë (période puce plus courte que la période
/// d'échantillonnage audio) donnerait un repliement de spectre sonnant
/// comme un parasite granuleux plutôt qu'une tonalité propre) et le DMA
/// Sound (STE, vrais échantillons gauche/droite indépendants) en un
/// échantillon stéréo, **brut** : ni gain Microwire (volume ET graves/
/// aigus) ni retrait de DC ni écrêtage — voir leur application séparée
/// dans `main` (gain lissé pour éviter un « clic » de zipper noise, DC
/// retiré avant ce gain puisque c'est un artefact de ce mixage, pas un
/// phénomène physique que le volume devrait mettre à l'échelle ; filtre
/// graves/aigus après, façon Hatari `DmaSnd_Apply_LMC` — voir la doc de
/// `Microwire`).
///
/// Les 3 canaux PSG sont combinés de façon NON-LINÉAIRE (façon Hatari,
/// [`mix_channels_model`], voir sa doc) plutôt que sommés : le DAC réel du
/// YM2149 sature nettement en dessous de 3× une seule voie quand plusieurs
/// canaux jouent à pleine amplitude simultanément. Le résultat
/// (`0.0..=65535.0`) est centré et réduit de moitié pour laisser de la
/// marge et éviter l'écrêtage une fois combiné au DMA Sound.
fn mix_sample(ym_levels: [f32; 3], dma_lr: (i8, i8)) -> (f32, f32) {
    let combined = mix_channels_model(ym_levels); // 0.0..=65535.0
    let ym_sample = (combined - 32767.5) * (i16::MAX as f32 / 32767.5) / 2.0;

    let (l, r) = dma_lr;
    // Échantillons PCM 8 bits signés (-128..127), mis à l'échelle sur
    // environ la moitié de la plage i16 (comme pour la PSG) — gauche et
    // droite restent SÉPARÉS (pas de moyenne), contrairement à la version
    // mono précédente.
    let dma_left = l as f32 * 128.0;
    let dma_right = r as f32 * 128.0;

    (ym_sample + dma_left, ym_sample + dma_right)
}

/// Enregistreur WAV minimal (PCM 16 bits stéréo, pas de dépendance externe)
/// — voir `RUST68_RECORD_WAV` dans `main`. L'en-tête est réécrit après
/// chaque ajout (coût négligeable : 44 octets, une fois par trame vidéo)
/// plutôt qu'une seule fois à la fermeture, pour que le fichier reste un
/// WAV valide même si le processus est arrêté brutalement (fermeture de
/// fenêtre, Ctrl-C) sans passage par un code de sortie propre.
struct WavRecorder {
    file: std::fs::File,
    data_bytes_written: u32,
}

impl WavRecorder {
    const CHANNELS: u16 = 2;
    const BITS_PER_SAMPLE: u16 = 16;

    fn create(path: &str) -> std::io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        Self::write_header(&mut file, 0)?;
        Ok(WavRecorder { file, data_bytes_written: 0 })
    }

    fn write_header(file: &mut std::fs::File, data_bytes: u32) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        file.seek(SeekFrom::Start(0))?;
        let sample_rate = AUDIO_SAMPLE_RATE as u32;
        let byte_rate = sample_rate * Self::CHANNELS as u32 * Self::BITS_PER_SAMPLE as u32 / 8;
        let block_align = Self::CHANNELS * Self::BITS_PER_SAMPLE / 8;
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + data_bytes).to_le_bytes())?;
        file.write_all(b"WAVE")?;
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?; // PCM
        file.write_all(&Self::CHANNELS.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&Self::BITS_PER_SAMPLE.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data_bytes.to_le_bytes())?;
        Ok(())
    }

    fn append(&mut self, samples: &[i16]) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        self.file.seek(SeekFrom::End(0))?;
        for s in samples {
            self.file.write_all(&s.to_le_bytes())?;
        }
        self.data_bytes_written += (samples.len() * 2) as u32;
        Self::write_header(&mut self.file, self.data_bytes_written)?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

/// Bloqueur DC (filtre passe-haut un pôle) : `y[n] = x[n] - x[n-1] + R·y[n-1]`,
/// technique standard pour retirer un biais continu d'un signal audio sans
/// connaître sa cause exacte ni sa valeur précise — voir le commentaire sur
/// son usage dans `main`. `R` proche de 1 place la coupure très bas (quasi
/// inaudible sur le contenu utile, seul le DC pur est retiré).
struct DcBlocker {
    prev_x: f32,
    prev_y: f32,
}

impl DcBlocker {
    const R: f32 = 0.995;

    fn new() -> Self {
        DcBlocker { prev_x: 0.0, prev_y: 0.0 }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.prev_x + Self::R * self.prev_y;
        self.prev_x = x;
        self.prev_y = y;
        y
    }
}

/// Copie le framebuffer du board (une ligne par entrée, RGB 24 bits) dans la
/// texture SDL2 640x400, en répétant les lignes/colonnes pour les résolutions
/// plus petites (basse résolution notamment).
fn render_frame(st: &AtariSt, texture: &mut sdl2::render::Texture) {
    use rust68::peripherals::atari_st::shifter::Resolution;
    let resolution = st.shifter.resolution();
    let (src_width, visible_height) = match resolution {
        Resolution::Low => (320usize, 200usize),
        Resolution::Medium => (640, 200),
        Resolution::High => (640, 400),
    };

    let _ = texture.with_lock(None, |buffer: &mut [u8], pitch: usize| {
        for dst_y in 0..400usize {
            let src_y = dst_y * visible_height / 400;
            let row = st.framebuffer.get(src_y);
            for dst_x in 0..640usize {
                let src_x = dst_x * src_width / 640;
                let (r, g, b) = row
                    .and_then(|r| r.get(src_x))
                    .copied()
                    .unwrap_or((0, 0, 0));
                let offset = dst_y * pitch + dst_x * 3;
                buffer[offset] = r;
                buffer[offset + 1] = g;
                buffer[offset + 2] = b;
            }
        }
    });
}

/// Charge un fichier disquette, `.stx` (détecté par sa signature `"RSY\0"`),
/// `.msa` (détecté par sa signature `$0E0F`) ou `.st` brut (géométrie
/// devinée à partir de la taille du fichier — les formats les plus courants
/// sont testés dans l'ordre).
fn load_floppy(path: &str) -> Result<Box<dyn FloppyDisk>, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if data.len() >= 4 && &data[0..4] == b"RSY\0" {
        return StxImage::parse(&data)
            .map(|img| Box::new(img) as Box<dyn FloppyDisk>)
            .map_err(|e| format!("{e:?}"));
    }
    if data.len() >= 2 && data[0] == 0x0E && data[1] == 0x0F {
        return msa::parse(&data)
            .map(|img| Box::new(img) as Box<dyn FloppyDisk>)
            .map_err(|e| format!("{e:?}"));
    }

    // Géométries `.st` courantes : (pistes, faces, secteurs/piste).
    const GEOMETRIES: &[(u8, u8, u8)] = &[
        (80, 2, 9),  // 720 Ko, double face standard
        (80, 1, 9),  // 360 Ko, simple face standard
        (80, 2, 10), // 800 Ko, extra-secteurs
        (82, 2, 9),  // 738 Ko, pistes étendues
        (80, 2, 11), // 880 Ko, extra-secteurs
    ];
    let geometry = GEOMETRIES
        .iter()
        .find(|(t, s, spt)| {
            *t as usize * *s as usize * *spt as usize * 512 == data.len()
        })
        .copied()
        .unwrap_or_else(|| {
            eprintln!("Géométrie .st non reconnue pour une taille de {} octets, estimation approximative.", data.len());
            let sectors_per_track = 9u8;
            let sides = 2u8;
            let tracks = (data.len() / (sectors_per_track as usize * sides as usize * 512)).min(255) as u8;
            (tracks, sides, sectors_per_track)
        });

    Ok(Box::new(RawDiskImage::new(
        data, geometry.0, geometry.1, geometry.2,
    )))
}

/// Charge le bruitage mécanique du lecteur de disquette (démarrage moteur,
/// bourdonnement, clic de pas, bourdonnement de recherche) depuis un
/// dossier contenant les 4 fichiers `.wav` du jeu Steem SSE
/// `3rdparty/DriveSound` (même jeu que celui utilisé par le projet
/// compagnon Stay). Chemin par défaut : `ressources (local)/drivesound`
/// (dossier local non versionné, voir `.gitignore`), surchargeable via
/// `RUST68_DRIVE_SOUND_DIR`. Chaque fichier manquant ou invalide dégrade
/// silencieusement vers un slot muet plutôt que d'empêcher le démarrage —
/// ce bruitage n'est qu'une ambiance, pas une fonctionnalité requise.
fn load_drive_sound(sample_rate: u32) -> DriveSound {
    let dir = std::env::var("RUST68_DRIVE_SOUND_DIR")
        .unwrap_or_else(|_| "ressources (local)/drivesound".to_string());
    let mut drive_sound = DriveSound::new(sample_rate);
    for (slot, filename) in [
        (Slot::Start, "drive_startup.wav"),
        (Slot::Motor, "drive_spin.wav"),
        (Slot::Step, "drive_click.wav"),
        (Slot::Seek, "drive_seek.wav"),
    ] {
        let path = format!("{dir}/{filename}");
        match std::fs::read(&path) {
            Ok(data) => match wav::load_wav_mono_i16(&data) {
                Ok((mut samples, native_rate)) => {
                    if slot == Slot::Step {
                        // `drive_click.wav` (jeu Steem SSE 3rdparty/DriveSound)
                        // a une longue traîne de résonance (~200 ms au total,
                        // encore ~15 % de l'amplitude crête à 80 ms) — mesuré
                        // par analyse d'enveloppe. Sur une piste protégée
                        // relisant piste par piste (Rick_Dangerous.stx), les
                        // commandes Step séparées arrivent toutes les 6-7 ms :
                        // sans rognage, chaque nouveau clic coupe net la
                        // traîne encore forte du précédent, ce qui sonne comme
                        // un magma continu plutôt qu'un train de clics
                        // distincts et réguliers. On ne garde donc que les
                        // ~60 premières ms (attaque + rebond mécanique
                        // secondaire vers 40-55 ms), qui suffisent seules à
                        // rendre le clic reconnaissable.
                        let keep = (native_rate as usize * 60) / 1000;
                        samples.truncate(keep);
                    }
                    drive_sound.set_sample(slot, samples, native_rate);
                }
                Err(e) => eprintln!("Bruitage disquette : {path} illisible ({e}), slot muet"),
            },
            Err(e) => eprintln!("Bruitage disquette : {path} absent ({e}), slot muet"),
        }
    }
    drive_sound
}

/// Traduit un `Scancode` SDL2 en scancode ST (make code, bit 7 = 0 — voir
/// l'appelant pour le break code). Table dérivée de la disposition PC-XT
/// documentée publiquement, largement reprise par le contrôleur clavier
/// HD6301 de l'Atari ST ; ne couvre que les touches usuelles (pas le pavé
/// numérique complet, pas les touches ST spécifiques comme Alt-Help).
fn st_scancode(sc: Scancode) -> Option<u8> {
    use Scancode::*;
    Some(match sc {
        Escape => 0x01,
        Num1 => 0x02,
        Num2 => 0x03,
        Num3 => 0x04,
        Num4 => 0x05,
        Num5 => 0x06,
        Num6 => 0x07,
        Num7 => 0x08,
        Num8 => 0x09,
        Num9 => 0x0A,
        Num0 => 0x0B,
        Minus => 0x0C,
        Equals => 0x0D,
        Backspace => 0x0E,
        Tab => 0x0F,
        Q => 0x10,
        W => 0x11,
        E => 0x12,
        R => 0x13,
        T => 0x14,
        Y => 0x15,
        U => 0x16,
        I => 0x17,
        O => 0x18,
        P => 0x19,
        LeftBracket => 0x1A,
        RightBracket => 0x1B,
        Return => 0x1C,
        LCtrl => 0x1D,
        A => 0x1E,
        S => 0x1F,
        D => 0x20,
        F => 0x21,
        G => 0x22,
        H => 0x23,
        J => 0x24,
        K => 0x25,
        L => 0x26,
        Semicolon => 0x27,
        Apostrophe => 0x28,
        Grave => 0x29,
        LShift => 0x2A,
        Backslash => 0x2B,
        Z => 0x2C,
        X => 0x2D,
        C => 0x2E,
        V => 0x2F,
        B => 0x30,
        N => 0x31,
        M => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        RShift => 0x36,
        LAlt => 0x38,
        Space => 0x39,
        CapsLock => 0x3A,
        F1 => 0x3B,
        F2 => 0x3C,
        F3 => 0x3D,
        F4 => 0x3E,
        F5 => 0x3F,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        Up => 0x48,
        Left => 0x4B,
        Right => 0x4D,
        Down => 0x50,
        Insert => 0x52,
        Delete => 0x53,
        Home => 0x47,
        _ => return None,
    })
}

/// Met en attente un (ou plusieurs) rapport(s) de position relative de
/// souris au protocole IKBD, via le même canal série que le clavier (l'IKBD
/// partage une seule liaison avec l'ACIA clavier — voir le commentaire de
/// module). `dx`/`dy` sont en pixels ST logiques. Le firmware IKBD réel ne
/// code que ±15 unités par paquet (pas ±127, la seule limite du champ
/// signé sur un octet) — un déplacement plus grand est découpé en
/// plusieurs paquets successifs, comme le fait réellement le HD6301 (voir
/// `Machine::mouse_move`/`flush_input_vbl` du projet compagnon Stay,
/// vérifié contre Steem SSE `ikbd_mouse_move`).
fn queue_mouse_move(ikbd: &mut rust68::peripherals::atari_st::ikbd::Ikbd, dx: i32, dy: i32, left: bool, right: bool) {
    const MAX_MOVE: i32 = 15;
    // Constaté empiriquement (voir ETAT.md) : le TOS échange les deux bits
    // de bouton par rapport à la convention documentée bit0=gauche/
    // bit1=droit — bit0=droit/bit1=gauche redonne le bon comportement.
    let buttons = (right as u8) | ((left as u8) << 1);
    let (mut x1, mut y1) = (0i32, 0i32);
    while (dx - x1).abs() > MAX_MOVE || (dy - y1).abs() > MAX_MOVE {
        let x2 = (dx - x1).clamp(-MAX_MOVE, MAX_MOVE);
        let y2 = (dy - y1).clamp(-MAX_MOVE, MAX_MOVE);
        ikbd.mouse_move(x2 as i8, y2 as i8, buttons);
        x1 += x2;
        y1 += y2;
    }
    ikbd.mouse_move((dx - x1) as i8, (dy - y1) as i8, buttons);
}
