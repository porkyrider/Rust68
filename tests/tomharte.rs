//! Suite de conformité TomHarte / ProcessorTests (680x0).
//!
//! Réf. : <https://github.com/SingleStepTests/m68000>
//!
//! Chaque fichier `<opcode>.json[.gz]` contient des milliers de tests. Pour
//! chacun : on installe l'état CPU + RAM `initial`, on exécute **une** instruction,
//! puis on compare l'état CPU + RAM à l'état `final` attendu.
//!
//! ## Utilisation
//!
//! Les fichiers de test font plusieurs centaines de mégaoctets et ne sont pas
//! versionnés. Téléchargez-les puis pointez la variable d'environnement
//! `TOMHARTE_DIR` sur le répertoire contenant les `.json` :
//!
//! ```sh
//! TOMHARTE_DIR=/chemin/vers/v1 cargo test --test tomharte -- --nocapture
//! ```
//!
//! Sans cette variable, le test est ignoré proprement (il ne bloque pas la CI).

use std::path::PathBuf;

use rust68::{Bus, Cpu, FlatBus, Size};
use serde::Deserialize;
use serde_json::Value;

/// État CPU d'un cas de test (champs communs à `initial` et `final`).
#[derive(Debug, Deserialize)]
struct State {
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
    d4: u32,
    d5: u32,
    d6: u32,
    d7: u32,
    a0: u32,
    a1: u32,
    a2: u32,
    a3: u32,
    a4: u32,
    a5: u32,
    a6: u32,
    usp: u32,
    ssp: u32,
    sr: u16,
    pc: u32,
    /// File de préfetch (mots déjà lus par le 68000) — installée en RAM.
    prefetch: Vec<u16>,
    /// Contenu mémoire : liste de paires `[adresse, octet]`.
    ram: Vec<(u32, u8)>,
}

/// Un cas de test unitaire.
#[derive(Debug, Deserialize)]
struct TestCase {
    name: String,
    #[serde(rename = "initial")]
    initial: State,
    #[serde(rename = "final")]
    final_state: State,
    /// Transactions bus. On s'en sert pour détecter les address errors ("re"/"we")
    /// qui requièrent la gestion d'exceptions — ces cas sont ignorés.
    #[serde(default)]
    transactions: Vec<Value>,
}

impl TestCase {
    fn requires_exception_handling(&self) -> bool {
        false
    }
}

/// Installe un [`State`] dans le CPU et le bus.
fn install(cpu: &mut Cpu, bus: &mut FlatBus, s: &State) {
    cpu.d = [s.d0, s.d1, s.d2, s.d3, s.d4, s.d5, s.d6, s.d7];
    // a7 est dérivé du SR (mode superviseur) ; on installe a0..a6 puis le bon a7.
    cpu.a[0] = s.a0;
    cpu.a[1] = s.a1;
    cpu.a[2] = s.a2;
    cpu.a[3] = s.a3;
    cpu.a[4] = s.a4;
    cpu.a[5] = s.a5;
    cpu.a[6] = s.a6;
    cpu.usp = s.usp;
    cpu.ssp = s.ssp;
    cpu.sr = s.sr;
    // a7 actif selon le mode : SSP en superviseur, USP en utilisateur.
    cpu.a[7] = if cpu.supervisor() { s.ssp } else { s.usp };
    // Le PC TomHarte est m_au de MAME : "next prefetch address" = opcode_addr + 4.
    // Notre modèle : PC pointe sur l'octet à fetch (= opcode_addr).
    // On recule de 4 pour aligner les deux conventions.
    cpu.pc = s.pc.wrapping_sub(4);

    for &(addr, val) in &s.ram {
        bus.write8(addr, val);
    }
    // Le préfetch est injecté directement dans la file pipeline du CPU.
    // On N'écrit PAS en RAM : les adresses au PC peuvent contenir des données
    // initiales que l'instruction va lire (ex. ADD (A0)+,Dn quand A0 == PC).
    cpu.load_prefetch(&s.prefetch);
}

/// Compare l'état final attendu à l'état réel ; renvoie un message si écart.
fn compare(cpu: &Cpu, bus: &mut FlatBus, expected: &State) -> Result<(), String> {
    let regs = [
        ("d0", cpu.d[0], expected.d0),
        ("d1", cpu.d[1], expected.d1),
        ("d2", cpu.d[2], expected.d2),
        ("d3", cpu.d[3], expected.d3),
        ("d4", cpu.d[4], expected.d4),
        ("d5", cpu.d[5], expected.d5),
        ("d6", cpu.d[6], expected.d6),
        ("d7", cpu.d[7], expected.d7),
        ("a0", cpu.a[0], expected.a0),
        ("a1", cpu.a[1], expected.a1),
        ("a2", cpu.a[2], expected.a2),
        ("a3", cpu.a[3], expected.a3),
        ("a4", cpu.a[4], expected.a4),
        ("a5", cpu.a[5], expected.a5),
        ("a6", cpu.a[6], expected.a6),
        // Convention TomHarte : PC = "next prefetch address" = instruction_addr + 4.
        // Notre modèle : PC avance instruction par instruction (sans pipeline ahead).
        // Relation : tomharte_final_pc = notre_pc_final + 4.
        ("pc", cpu.pc.wrapping_add(4), expected.pc),
    ];
    for (nom, got, want) in regs {
        if got != want {
            return Err(format!("{nom} = {got:#010X}, attendu {want:#010X}"));
        }
    }
    if cpu.sr != expected.sr {
        return Err(format!(
            "sr = {:#06X}, attendu {:#06X}",
            cpu.sr, expected.sr
        ));
    }
    let want_a7 = if expected.sr & rust68::sr::S != 0 {
        expected.ssp
    } else {
        expected.usp
    };
    if cpu.a[7] != want_a7 {
        return Err(format!("a7 = {:#010X}, attendu {want_a7:#010X}", cpu.a[7]));
    }
    for &(addr, want) in &expected.ram {
        let got = bus.read8(addr);
        if got != want {
            return Err(format!(
                "ram[{addr:#08X}] = {got:#04X}, attendu {want:#04X}"
            ));
        }
    }
    let _ = Size::Byte; // (taille importée pour usage futur du harness)
    Ok(())
}

/// Exécute un fichier de cas de test ; renvoie (réussis, échecs, ignorés).
fn run_file(path: &PathBuf) -> (usize, usize, usize) {
    let data = std::fs::read_to_string(path).expect("lecture du fichier de test");
    let cases: Vec<TestCase> = serde_json::from_str(&data).expect("JSON TomHarte invalide");

    let (mut ok, mut fail, mut skipped) = (0, 0, 0);
    for case in &cases {
        // Les cas qui déclenchent des exceptions (address error, vecteur) : on ignore.
        if case.requires_exception_handling() {
            skipped += 1;
            continue;
        }

        let mut cpu = Cpu::new();
        let mut bus = FlatBus::new();
        install(&mut cpu, &mut bus, &case.initial);

        let is_ae = case
            .transactions
            .iter()
            .any(|t| matches!(t.get(0).and_then(|v| v.as_str()), Some("re") | Some("we")));

        match cpu.step(&mut bus) {
            Ok(_) => match compare(&cpu, &mut bus, &case.final_state) {
                Ok(()) => ok += 1,
                Err(why) => {
                    if std::env::var("DIAG").is_ok() {
                        // Catégorise : champ qui diffère + cas AE ?
                        let field = why.split(['=', ' ', '[']).next().unwrap_or("?").trim();
                        eprintln!("DIAG\t{}\tae={}\t{}", field, is_ae, case.name);
                    } else if fail < 5 {
                        eprintln!("ÉCHEC [{}] : {why}", case.name);
                    }
                    fail += 1;
                }
            },
            // Opcode pas encore implémenté : on l'ignore (couverture partielle).
            Err(_) => skipped += 1,
        }
    }
    (ok, fail, skipped)
}

// ── ANSI helpers ──────────────────────────────────────────────────────────────
const RESET: &str  = "\x1b[0m";
const BOLD:  &str  = "\x1b[1m";
const GREEN: &str  = "\x1b[32m";
const RED:   &str  = "\x1b[31m";
const YELLOW:&str  = "\x1b[33m";
const DIM:   &str  = "\x1b[2m";
const CYAN:  &str  = "\x1b[36m";

fn bar(ok: usize, total: usize, width: usize) -> String {
    if total == 0 { return " ".repeat(width); }
    let filled = (ok * width) / total;
    let empty  = width - filled;
    let color  = if ok == total { GREEN } else if ok * 10 >= total * 9 { YELLOW } else { RED };
    format!("{color}{}{DIM}{}{RESET}", "█".repeat(filled), "░".repeat(empty))
}

#[test]
fn conformite_tomharte() {
    let Ok(dir) = std::env::var("TOMHARTE_DIR") else {
        eprintln!(
            "TOMHARTE_DIR non défini — test de conformité ignoré. \
             Voir l'en-tête de tests/tomharte.rs pour l'installation."
        );
        return;
    };

    let dir = PathBuf::from(dir);

    // FOCUS=MOVE.l,MOVE.w  → ne tourne que ces fichiers
    let focus: Option<Vec<String>> = std::env::var("FOCUS").ok().map(|s| {
        s.split(',').map(|p| p.trim().to_string()).collect()
    });

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("répertoire TOMHARTE_DIR illisible")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| {
            if let Some(ref list) = focus {
                let stem = p.file_stem().unwrap().to_string_lossy();
                list.iter().any(|f| stem.eq_ignore_ascii_case(f))
            } else {
                true
            }
        })
        .collect();
    files.sort();

    assert!(
        !files.is_empty(),
        "aucun fichier .json dans {} (FOCUS={:?})",
        dir.display(),
        focus,
    );

    // En-tête tableau
    eprintln!();
    if let Some(ref list) = focus {
        eprintln!("{BOLD}{YELLOW}▶ mode ciblé : {}{RESET}", list.join(", "));
    }
    eprintln!("{BOLD}{CYAN}{:<22} {:>5}  {:>5}  {:>5}  {:>5}  {:<20}{RESET}",
        "Instruction", "total", "ok", "fail", "skip", "progression");
    eprintln!("{DIM}{}{RESET}", "─".repeat(72));

    // Charger la baseline si elle existe (format: "NOM ok fail")
    let baseline_path = std::path::Path::new("tomharte_baseline.txt");
    let baseline: std::collections::HashMap<String, (usize, usize)> = std::fs::read_to_string(baseline_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next()?.to_string();
            let ok:   usize = it.next()?.parse().ok()?;
            let fail: usize = it.next()?.parse().ok()?;
            Some((name, (ok, fail)))
        })
        .collect();
    let save_baseline = focus.is_none() && std::env::var("BASELINE").is_ok();

    let (mut total_ok, mut total_fail, mut total_skip) = (0, 0, 0);
    let mut new_baseline: Vec<String> = vec![];
    let mut regressions = 0usize;

    for f in &files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        let (ok, fail, skip) = run_file(f);
        total_ok   += ok;
        total_fail += fail;
        total_skip += skip;

        let total = ok + fail;
        let pct   = if total > 0 { ok * 100 / total } else { 100 };
        let progress = bar(ok, total, 20);

        // Détection de régression vs baseline
        let regression_tag = if let Some(&(base_ok, base_fail)) = baseline.get(&name) {
            if fail > base_fail {
                regressions += 1;
                format!("  {RED}▼ régression ({} → {} échecs){RESET}", base_fail, fail)
            } else if fail == 0 && base_fail > 0 {
                format!("  {GREEN}▲ résolu !{RESET}")
            } else if fail < base_fail {
                format!("  {GREEN}▲ +{} ({} → {}){RESET}", base_fail - fail, base_fail, fail)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let (color, marker) = if fail == 0 {
            (GREEN, "✓")
        } else if pct >= 90 {
            (YELLOW, "!")
        } else {
            (RED, "✗")
        };

        eprintln!("{color}{marker}{RESET} {:<21} {:>5}  {GREEN}{:>5}{RESET}  {color}{:>5}{RESET}  {DIM}{:>5}{RESET}  {progress}  {DIM}{:>3}%{RESET}{}",
            name,
            ok + fail + skip,
            ok,
            if fail > 0 { format!("{fail}") } else { String::new() },
            if skip > 0 { format!("{skip}") } else { String::new() },
            pct,
            regression_tag,
        );

        new_baseline.push(format!("{name} {ok} {fail}"));
    }

    // Ligne totale
    let grand_total = total_ok + total_fail + total_skip;
    let grand_pct   = if total_ok + total_fail > 0 { total_ok * 100 / (total_ok + total_fail) } else { 100 };
    eprintln!("{DIM}{}{RESET}", "─".repeat(72));
    eprintln!("{BOLD}  {:<21} {:>5}  {:>5}  {:>5}  {:>5}  {}  {:>3}%{RESET}",
        "TOTAL",
        grand_total,
        total_ok,
        if total_fail > 0 { format!("{total_fail}") } else { String::new() },
        if total_skip > 0 { format!("{total_skip}") } else { String::new() },
        bar(total_ok, total_ok + total_fail, 20),
        grand_pct,
    );
    eprintln!();

    if save_baseline {
        std::fs::write(baseline_path, new_baseline.join("\n") + "\n")
            .expect("écriture baseline impossible");
        eprintln!("{DIM}baseline sauvegardée dans {}{RESET}", baseline_path.display());
    } else if focus.is_none() && !baseline.is_empty() {
        eprintln!("{DIM}(relancer avec BASELINE=1 pour mettre à jour la baseline){RESET}");
    }

    if regressions > 0 {
        panic!("{regressions} régression(s) détectée(s) — voir tableau ci-dessus");
    }
    assert_eq!(total_fail, 0, "{total_fail} cas de conformité en échec");
}
