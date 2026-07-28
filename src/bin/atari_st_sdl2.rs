//! Frontend SDL2 minimal pour l'Atari ST émulé — vidéo + clavier + son.
//!
//! Binaire de démonstration/test, séparé de la bibliothèque `rust68` (voir
//! la feature Cargo `sdl2-frontend`) : sert à vérifier visuellement et
//! interactivement le bon fonctionnement de l'émulation (CPU + périphériques
//! ST), pas à être un émulateur ST "complet" (pas de configuration GUI, pas
//! de sauvegarde d'état, pas de support disquette en écriture persistante).
//!
//! Usage : `atari_st_sdl2 [--model <nom>] <rom.img> [disque.stx|disque.st]`
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

use rust68::peripherals::atari_st::stx::StxImage;
use rust68::peripherals::atari_st::wd1772::{FloppyDisk, RawDiskImage};
use rust68::systems::atari_st::AtariSt;
use rust68::{Bus, Cpu};

use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::event::Event;
use sdl2::keyboard::Scancode;
use sdl2::pixels::PixelFormatEnum;
use std::time::Duration;

const AUDIO_SAMPLE_RATE: i32 = 44_100;
/// Seuil de remplissage de la file audio (en octets, mono i16 = 2 octets/échantillon)
/// au-delà duquel on ralentit l'émulation pour rester au rythme temps réel.
const AUDIO_QUEUE_HIGH_WATERMARK: u32 = (AUDIO_SAMPLE_RATE as u32) * 2 / 4; // ~250 ms
/// Modèle par défaut si `--model` n'est pas précisé.
const DEFAULT_MODEL: &str = "1040ste";

fn usage(program: &str) -> ! {
    eprintln!(
        "Usage : {program} [--model <nom>] <rom.img> [disque.stx|disque.st]\n\n\
         Modèles disponibles (--model, casse indifférente) : 520st, 1040st, megast, \
         520ste, 1040ste (défaut), megaste — voir `systems::atari_st::model` pour le \
         détail de chaque profil (RAM, Blitter…)."
    );
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let program = args[0].clone();

    let mut model_name = DEFAULT_MODEL.to_string();
    let mut positional: Vec<&String> = Vec::new();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--model" {
            match iter.next() {
                Some(v) => model_name = v.clone(),
                None => usage(&program),
            }
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
    if let Some(path) = disk_path {
        match load_floppy(path) {
            Ok(disk) => st.floppy_a = Some(disk),
            Err(e) => eprintln!("Impossible de charger le disque {path} : {e} (démarrage sans disquette)"),
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
    }

    let mut cpu = Cpu::new();
    cpu.reset(&mut st);

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

    let desired_spec = AudioSpecDesired {
        freq: Some(AUDIO_SAMPLE_RATE),
        channels: Some(1),
        samples: Some(1024),
    };
    let audio_queue: AudioQueue<i16> = audio_subsystem
        .open_queue(None, &desired_spec)
        .expect("ouverture file audio");
    audio_queue.resume();

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
    // L'ACIA (MC6850) est un registre de réception unique, sans FIFO,
    // fidèle au silicium réel (voir `peripherals::acia`) : un octet non
    // encore lu par le programme quand un nouveau arrive est perdu (OVRN).
    // Sur ST réel, l'IKBD envoie les octets un par un à un débit série fixe
    // (~7812 bauds), laissant largement le temps à l'interruption RDRF
    // d'être traitée entre deux — ce timing n'existe pas ici (une trame
    // souris = 3 octets), donc on les met en attente dans cette file et on
    // n'en pousse qu'un seul dans l'ACIA par pas CPU, une fois le
    // précédent effectivement consommé (RDRF retombé) : sans ça, les 2e/3e
    // octets de chaque trame souris (dX/dY) étaient perdus par overrun, et
    // le TOS interprétait à tort les octets d'en-tête suivants (`0xF8` =
    // -8 en signé) comme des deltas — d'où une dérive constante en diagonale.
    let mut ikbd_tx_queue: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
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

    // `profile.cpu_hz` : voir sa doc dans `model.rs` — informatif seulement
    // pour l'instant (tous les profils renvoient 8 MHz), mais c'est déjà la
    // seule source de vérité pour ce calcul plutôt qu'une constante séparée
    // qui pourrait diverger le jour où un mode CPU accéléré sera modélisé.
    let cycles_per_sample = profile.cpu_hz as f64 / AUDIO_SAMPLE_RATE as f64;
    let mut audio_cycle_acc = 0.0f64;
    let mut audio_buffer: Vec<i16> = Vec::with_capacity(2048);

    // `RUST68_DEBUG=1` : affiche un point d'avancement (pas exécutés, PC,
    // SR) une fois par seconde sur stderr — utile pour vérifier que
    // l'émulation progresse sans être bloquée, sans instrumenter chaque
    // instruction en usage normal.
    let debug = std::env::var("RUST68_DEBUG").is_ok();
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
    let mut step_count: u64 = 0;
    let mut last_report = std::time::Instant::now();

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    scancode: Some(sc),
                    repeat: false,
                    ..
                } => {
                    if let Some(code) = st_scancode(sc) {
                        ikbd_tx_queue.push_back(code);
                    }
                }
                Event::KeyUp {
                    scancode: Some(sc), ..
                } => {
                    if let Some(code) = st_scancode(sc) {
                        ikbd_tx_queue.push_back(code | 0x80);
                    }
                }
                Event::MouseMotion { x, y, .. } => {
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
                        queue_mouse_packet(&mut ikbd_tx_queue, dx as i32, dy as i32, mouse_left, mouse_right);
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
                    queue_mouse_packet(&mut ikbd_tx_queue, 0, 0, mouse_left, mouse_right);
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
                    queue_mouse_packet(&mut ikbd_tx_queue, 0, 0, mouse_left, mouse_right);
                }
                _ => {}
            }
        }

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
            let cycles = match cpu.step(&mut st) {
                Ok(cycles) => cycles,
                Err(e) => {
                    eprintln!("Erreur CPU, arrêt : {e:?} pc={:#08x}", cpu.pc);
                    break 'running;
                }
            };
            st.tick(cycles);
            step_count += 1;

            // Ne pousse l'octet suivant dans l'ACIA que si le précédent a
            // bien été consommé par le programme (RDRF retombé) — lecture
            // du registre de contrôle/statut, sans effet de bord (voir
            // `Acia::read`, contrairement à une lecture du registre de
            // données qui, elle, acquitte RDRF).
            if !ikbd_tx_queue.is_empty() {
                let rdrf = st.acia_keyboard.read(rust68::peripherals::atari_st::acia::reg::CONTROL_STATUS) & 0x01 != 0;
                if !rdrf {
                    if let Some(byte) = ikbd_tx_queue.pop_front() {
                        if debug {
                            eprintln!("[mouse] octet remis à l'ACIA : {byte:#04x} (reste {} en attente)", ikbd_tx_queue.len());
                        }
                        st.acia_keyboard.push_rx_byte(byte);
                    }
                }
            }

            if debug && last_report.elapsed().as_secs_f64() >= 1.0 {
                eprintln!(
                    "steps={step_count} pc={:#08x} sr={:#06x} video_base={:#08x} video_counter={:#08x} phystop={:#08x} v_bas_ad={:#08x}",
                    cpu.pc,
                    cpu.sr,
                    st.shifter.video_base(),
                    st.shifter.video_counter(),
                    st.read32(0x42E),
                    st.read32(0x44E)
                );
                last_report = std::time::Instant::now();
            }

            audio_cycle_acc += cycles as f64;
            while audio_cycle_acc >= cycles_per_sample {
                audio_cycle_acc -= cycles_per_sample;
                audio_buffer.push(mix_sample(&st.ym2149));
            }

            if st.glue.frame_count() != frame_before {
                break;
            }
        }

        if !audio_buffer.is_empty() {
            let _ = audio_queue.queue_audio(&audio_buffer);
            audio_buffer.clear();
        }

        render_frame(&st, &mut texture);
        canvas.clear();
        let _ = canvas.copy(&texture, None, None);
        canvas.present();

        // Throttling temps réel : la file audio se vide au rythme réel de la
        // carte son hôte, c'est donc l'horloge de référence pour ne pas
        // émuler plus vite que le temps réel (aucun minuteur dédié n'existe
        // ailleurs dans ce crate).
        while audio_queue.size() > AUDIO_QUEUE_HIGH_WATERMARK {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Mixe les 3 canaux du YM2149 (niveaux 0-31 chacun) en un échantillon PCM
/// signé 16 bits mono, centré sur zéro.
fn mix_sample(ym: &rust68::peripherals::atari_st::ym2149::Ym2149) -> i16 {
    let total = ym.channel_level(0) as i32 + ym.channel_level(1) as i32 + ym.channel_level(2) as i32;
    // total in 0..=93 ; centre sur 0 et met à l'échelle sur la pleine plage i16.
    ((total - 46) * (i16::MAX as i32 / 46)) as i16
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

/// Charge un fichier disquette, `.stx` (détecté par sa signature `"RSY\0"`)
/// ou `.st` brut (géométrie devinée à partir de la taille du fichier — les
/// formats les plus courants sont testés dans l'ordre).
fn load_floppy(path: &str) -> Result<Box<dyn FloppyDisk>, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if data.len() >= 4 && &data[0..4] == b"RSY\0" {
        return StxImage::parse(&data)
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
/// module). `dx`/`dy` sont en pixels ; un paquet IKBD ne code que -128..127
/// par composante, donc un déplacement plus grand (arrivé en une seule fois
/// depuis SDL2) est découpé en plusieurs paquets successifs, comme le
/// documente le protocole pour un mouvement dépassant la plage codable.
/// Ne pousse PAS directement dans l'ACIA (voir `ikbd_tx_queue` dans `main`)
/// : un burst de plusieurs octets sans laisser le programme les consommer
/// un par un écraserait les octets suivants par overrun (registre de
/// réception unique du MC6850, sans FIFO).
fn queue_mouse_packet(
    queue: &mut std::collections::VecDeque<u8>,
    mut dx: i32,
    mut dy: i32,
    left: bool,
    right: bool,
) {
    let header = 0xF8 | (left as u8) | ((right as u8) << 1);
    let debug = std::env::var("RUST68_DEBUG").is_ok();
    loop {
        let cx = dx.clamp(-128, 127);
        let cy = dy.clamp(-128, 127);
        queue.push_back(header);
        queue.push_back(cx as i8 as u8);
        queue.push_back(cy as i8 as u8);
        if debug {
            eprintln!(
                "[mouse] paquet mis en attente : {header:#04x} {:#04x} {:#04x} (file={} octets)",
                cx as i8 as u8,
                cy as i8 as u8,
                queue.len()
            );
        }
        dx -= cx;
        dy -= cy;
        if dx == 0 && dy == 0 {
            break;
        }
    }
}
