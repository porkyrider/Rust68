//! Puits de traçage générique (voir [`crate::bus::TracingBus`]) : journalise
//! chaque transaction bus (taille exacte .B/.W/.L, adresse, valeur, PC
//! appelant) dans un fichier, avec filtrage par plage d'adresses pour garder
//! le volume gérable sur une session interactive complète.
//!
//! Piloté entièrement par variables d'environnement (mêmes conventions que
//! le reste des sondes `RUST68_TRACE_*` du projet) pour rester à coût nul
//! quand désactivé — voir [`FileTraceSink::from_env`].

use crate::bus::TraceSink;
use std::io::{BufWriter, Write};

/// Puits de traçage concret : écrit un événement par ligne dans un fichier,
/// filtré à une plage d'adresses, avec une étiquette de composant fournie
/// par l'appelant (le format des adresses est spécifique au système —
/// `TracingBus`/`TraceSink` restent génériques, cette étiquette ne l'est
/// pas).
pub struct FileTraceSink {
    writer: BufWriter<std::fs::File>,
    addr_min: u32,
    addr_max: u32,
    classify: Box<dyn Fn(u32) -> &'static str>,
    trace_cpu: bool,
    break_addr: Option<u32>,
    break_hits: u32,
}

impl FileTraceSink {
    /// Construit depuis les variables d'environnement :
    /// - `RUST68_TRACE_ALL=1` : active (sinon `None`, aucun coût).
    /// - `RUST68_TRACE_ALL_PATH=<fichier>` : chemin de sortie (défaut :
    ///   `rust68_trace_all.log` dans le répertoire courant).
    /// - `RUST68_TRACE_ALL_MIN` / `RUST68_TRACE_ALL_MAX` (hexadécimal, sans
    ///   préfixe `0x`) : filtre d'adresses pour les accès bus — indispensable
    ///   sur une session interactive complète, sous peine de fichiers de
    ///   plusieurs gigaoctets.
    /// - `RUST68_TRACE_ALL_CPU=1` : trace aussi chaque pas CPU (adresse de
    ///   l'instruction) — désactivé par défaut (très verbeux, rarement
    ///   nécessaire pour diagnostiquer des accès mémoire).
    ///
    /// `classify` : fonction adresse -> nom de composant, fournie par le
    /// système hôte (voir `AtariSt::describe_addr`).
    pub fn from_env(classify: Box<dyn Fn(u32) -> &'static str>) -> Option<Self> {
        if std::env::var("RUST68_TRACE_ALL").is_err() {
            return None;
        }
        let path = std::env::var("RUST68_TRACE_ALL_PATH")
            .unwrap_or_else(|_| "rust68_trace_all.log".to_string());
        let file = std::fs::File::create(&path)
            .unwrap_or_else(|e| panic!("RUST68_TRACE_ALL_PATH={path} : {e}"));
        let parse_hex = |var: &str, default: u32| {
            std::env::var(var)
                .ok()
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(default)
        };
        let addr_min = parse_hex("RUST68_TRACE_ALL_MIN", 0);
        let addr_max = parse_hex("RUST68_TRACE_ALL_MAX", 0x00FF_FFFF);
        let trace_cpu = std::env::var("RUST68_TRACE_ALL_CPU").is_ok();
        let break_addr = std::env::var("RUST68_TRACE_ALL_BREAK")
            .ok()
            .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
        Some(FileTraceSink {
            writer: BufWriter::new(file),
            addr_min,
            addr_max,
            classify,
            trace_cpu,
            break_addr,
            break_hits: 0,
        })
    }

    /// Journalise un pas CPU (adresse de l'instruction sur le point d'être
    /// exécutée) — à appeler par le code hôte juste avant `Cpu::step` si
    /// `RUST68_TRACE_ALL_CPU` est actif (voir [`Self::wants_cpu_trace`]).
    pub fn cpu_step(&mut self, pc: u32) {
        let _ = writeln!(self.writer, "S pc={pc:08x}");
    }

    pub fn wants_cpu_trace(&self) -> bool {
        self.trace_cpu
    }

    /// "Point d'arrêt" léger : si `pc` correspond à `RUST68_TRACE_ALL_BREAK`,
    /// journalise l'état complet des registres (D0-D7, A0-A7, SR) — en
    /// particulier `a[7]` (pile), pour retrouver l'adresse de retour d'une
    /// routine dont on ne connaît que le point d'entrée (lue directement en
    /// mémoire par l'appelant si besoin, cette méthode ne fait que journaliser
    /// l'état). Plafonné à 5 déclenchements pour ne pas noyer le journal si
    /// l'adresse est atteinte en boucle. À appeler par le code hôte juste
    /// avant `Cpu::step`, indépendamment de `RUST68_TRACE_ALL_CPU`.
    pub fn maybe_break(&mut self, cpu: &crate::Cpu) {
        let Some(break_addr) = self.break_addr else {
            return;
        };
        if cpu.pc != break_addr || self.break_hits >= 5 {
            return;
        }
        self.break_hits += 1;
        let _ = writeln!(
            self.writer,
            "B pc={:08x} d0={:08x} d1={:08x} d2={:08x} d3={:08x} d4={:08x} d5={:08x} d6={:08x} d7={:08x} a0={:08x} a1={:08x} a2={:08x} a3={:08x} a4={:08x} a5={:08x} a6={:08x} a7={:08x} sr={:04x}",
            cpu.pc,
            cpu.d[0], cpu.d[1], cpu.d[2], cpu.d[3], cpu.d[4], cpu.d[5], cpu.d[6], cpu.d[7],
            cpu.a[0], cpu.a[1], cpu.a[2], cpu.a[3], cpu.a[4], cpu.a[5], cpu.a[6], cpu.a[7],
            cpu.sr,
        );
    }
}

impl TraceSink for FileTraceSink {
    fn on_read(&mut self, pc: u32, addr: u32, size: u8, value: u32) {
        if addr < self.addr_min || addr > self.addr_max {
            return;
        }
        let comp = (self.classify)(addr);
        let _ = writeln!(
            self.writer,
            "R pc={pc:08x} addr={addr:08x} sz={size} val={value:0width$x} comp={comp}",
            width = (size as usize) * 2
        );
    }

    fn on_write(&mut self, pc: u32, addr: u32, size: u8, value: u32) {
        if addr < self.addr_min || addr > self.addr_max {
            return;
        }
        let comp = (self.classify)(addr);
        let _ = writeln!(
            self.writer,
            "W pc={pc:08x} addr={addr:08x} sz={size} val={value:0width$x} comp={comp}",
            width = (size as usize) * 2
        );
    }
}
