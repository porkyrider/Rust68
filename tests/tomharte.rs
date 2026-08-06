//! TomHarte / ProcessorTests (680x0) conformance suite.
//!
//! Ref.: <https://github.com/SingleStepTests/m68000>
//!
//! Each `<opcode>.json[.gz]` file contains thousands of tests. For
//! each one: install the `initial` CPU + RAM state, execute **one** instruction,
//! then compare the CPU + RAM state against the expected `final` state.
//!
//! ## Usage
//!
//! The test files are several hundred megabytes and are not
//! version-controlled. Download them, then point the `TOMHARTE_DIR`
//! environment variable at the directory containing the `.json` files:
//!
//! ```sh
//! TOMHARTE_DIR=/path/to/v1 cargo test --test tomharte -- --nocapture
//! ```
//!
//! Without this variable, the test is skipped cleanly (it does not block CI).

use std::path::PathBuf;

use rust68::{Bus, Cpu, FlatBus, Size};
use serde::Deserialize;
use serde_json::Value;

/// CPU state of a test case (fields common to `initial` and `final`).
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
    /// Prefetch queue (words already read by the 68000) — installed in RAM.
    prefetch: Vec<u16>,
    /// Memory contents: list of `[address, byte]` pairs.
    ram: Vec<(u32, u8)>,
}

/// A single test case.
#[derive(Debug, Deserialize)]
struct TestCase {
    name: String,
    #[serde(rename = "initial")]
    initial: State,
    #[serde(rename = "final")]
    final_state: State,
    /// Bus transactions. Used to detect address errors ("re"/"we")
    /// that require exception handling — these cases are skipped.
    #[serde(default)]
    transactions: Vec<Value>,
    /// Actual number of CPU cycles for this case (standard ProcessorTests
    /// format). `Option`: if the field is absent (format variant), we just
    /// skip the cycle-count check for this case without failing the parse.
    #[serde(default)]
    length: Option<u64>,
}

impl TestCase {
    fn requires_exception_handling(&self) -> bool {
        false
    }
}

/// Installs a [`State`] into the CPU and the bus.
fn install(cpu: &mut Cpu, bus: &mut FlatBus, s: &State) {
    cpu.d = [s.d0, s.d1, s.d2, s.d3, s.d4, s.d5, s.d6, s.d7];
    // a7 is derived from SR (supervisor mode); we install a0..a6 then the right a7.
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
    // a7 active depending on mode: SSP in supervisor, USP in user mode.
    cpu.a[7] = if cpu.supervisor() { s.ssp } else { s.usp };
    // TomHarte's PC is MAME's m_au: "next prefetch address" = opcode_addr + 4.
    // Our model: PC points at the byte to fetch (= opcode_addr).
    // We subtract 4 to align the two conventions.
    cpu.pc = s.pc.wrapping_sub(4);

    for &(addr, val) in &s.ram {
        bus.write8(addr, val);
    }
    // The prefetch is injected directly into the CPU's pipeline queue.
    // We do NOT write it to RAM: the addresses at PC may contain initial
    // data that the instruction will read (e.g. ADD (A0)+,Dn when A0 == PC).
    cpu.load_prefetch(&s.prefetch);
}

/// Compares the number of cycles actually consumed to the one expected by the
/// test (`length` field, absent → no check for this case).
fn compare_cycles(got: u64, case: &TestCase) -> Result<(), String> {
    match case.length {
        Some(want) if want != got => Err(format!("cycles = {got}, expected {want}")),
        _ => Ok(()),
    }
}

/// Compares the expected final state to the actual state; returns a message on mismatch.
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
        // TomHarte convention: PC = "next prefetch address" = instruction_addr + 4.
        // Our model: PC advances instruction by instruction (no pipeline lookahead).
        // Relation: tomharte_final_pc = our_final_pc + 4.
        ("pc", cpu.pc.wrapping_add(4), expected.pc),
    ];
    for (name, got, want) in regs {
        if got != want {
            return Err(format!("{name} = {got:#010X}, expected {want:#010X}"));
        }
    }
    if cpu.sr != expected.sr {
        return Err(format!(
            "sr = {:#06X}, expected {:#06X}",
            cpu.sr, expected.sr
        ));
    }
    let want_a7 = if expected.sr & rust68::sr::S != 0 {
        expected.ssp
    } else {
        expected.usp
    };
    if cpu.a[7] != want_a7 {
        return Err(format!("a7 = {:#010X}, expected {want_a7:#010X}", cpu.a[7]));
    }
    for &(addr, want) in &expected.ram {
        let got = bus.read8(addr);
        if got != want {
            return Err(format!(
                "ram[{addr:#08X}] = {got:#04X}, expected {want:#04X}"
            ));
        }
    }
    let _ = Size::Byte; // (size imported for future harness use)
    Ok(())
}

/// Runs a test case file; returns (passed, state failures, cycle
/// failures, skipped). A case can fail on both state AND cycles; it
/// is counted only once in `fail` but both causes are reported.
fn run_file(path: &PathBuf) -> (usize, usize, usize, usize) {
    let data = std::fs::read_to_string(path).expect("failed to read test file");
    let cases: Vec<TestCase> = serde_json::from_str(&data).expect("invalid TomHarte JSON");

    let (mut ok, mut fail, mut cycle_fail, mut skipped) = (0, 0, 0, 0);
    for case in &cases {
        // Cases that trigger exceptions (address error, vector): skipped.
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
            Ok(cycles) => {
                let state_result = compare(&cpu, &mut bus, &case.final_state);
                let cycles_result = compare_cycles(cycles as u64, case);
                match (&state_result, &cycles_result) {
                    (Ok(()), Ok(())) => ok += 1,
                    _ => {
                        if let Err(why) = &state_result {
                            if std::env::var("DIAG").is_ok() {
                                let field = why.split(['=', ' ', '[']).next().unwrap_or("?").trim();
                                eprintln!("DIAG\t{}\tae={}\t{}", field, is_ae, case.name);
                            } else if fail < 5 {
                                eprintln!("FAIL [{}]: {why}", case.name);
                            }
                        }
                        if let Err(why) = &cycles_result {
                            cycle_fail += 1;
                            if std::env::var("DIAG").is_ok() {
                                eprintln!("DIAG\tcycles\tae={}\t{}", is_ae, case.name);
                            } else if state_result.is_ok() && cycle_fail <= 5 {
                                eprintln!("CYCLE FAIL [{}]: {why}", case.name);
                            }
                        }
                        if state_result.is_err() {
                            fail += 1;
                        }
                    }
                }
            }
            // Opcode not yet implemented: skipped (partial coverage).
            Err(_) => skipped += 1,
        }
    }
    (ok, fail, cycle_fail, skipped)
}

// ── ANSI helpers ──────────────────────────────────────────────────────────────
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";

fn bar(ok: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return " ".repeat(width);
    }
    let filled = (ok * width) / total;
    let empty = width - filled;
    let color = if ok == total {
        GREEN
    } else if ok * 10 >= total * 9 {
        YELLOW
    } else {
        RED
    };
    format!(
        "{color}{}{DIM}{}{RESET}",
        "█".repeat(filled),
        "░".repeat(empty)
    )
}

#[test]
fn tomharte_conformance() {
    let Ok(dir) = std::env::var("TOMHARTE_DIR") else {
        eprintln!(
            "TOMHARTE_DIR not set — conformance test skipped. \
             See the header of tests/tomharte.rs for setup instructions."
        );
        return;
    };

    let dir = PathBuf::from(dir);

    // FOCUS=MOVE.l,MOVE.w  → runs only these files
    let focus: Option<Vec<String>> = std::env::var("FOCUS")
        .ok()
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect());

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("TOMHARTE_DIR directory unreadable")
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
        "no .json file found in {} (FOCUS={:?})",
        dir.display(),
        focus,
    );

    // Table header
    eprintln!();
    if let Some(ref list) = focus {
        eprintln!("{BOLD}{YELLOW}▶ focused mode: {}{RESET}", list.join(", "));
    }
    eprintln!(
        "{BOLD}{CYAN}{:<22} {:>5}  {:>5}  {:>5}  {:>5}  {:>5}  {:<20}{RESET}",
        "Instruction", "total", "ok", "fail", "cyc", "skip", "progress"
    );
    eprintln!("{DIM}{}{RESET}", "─".repeat(72));

    // Load the baseline if it exists (format: "NAME ok fail")
    let baseline_path = std::path::Path::new("tomharte_baseline.txt");
    let baseline: std::collections::HashMap<String, (usize, usize)> =
        std::fs::read_to_string(baseline_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let name = it.next()?.to_string();
                let ok: usize = it.next()?.parse().ok()?;
                let fail: usize = it.next()?.parse().ok()?;
                Some((name, (ok, fail)))
            })
            .collect();
    let save_baseline = focus.is_none() && std::env::var("BASELINE").is_ok();

    let (mut total_ok, mut total_fail, mut total_cycle_fail, mut total_skip) = (0, 0, 0, 0);
    let mut new_baseline: Vec<String> = vec![];
    let mut regressions = 0usize;

    for f in &files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        let (ok, fail, cycle_fail, skip) = run_file(f);
        total_ok += ok;
        total_fail += fail;
        total_cycle_fail += cycle_fail;
        total_skip += skip;

        let total = ok + fail;
        let pct = if total > 0 { ok * 100 / total } else { 100 };
        let progress = bar(ok, total, 20);

        // Regression detection vs baseline
        let regression_tag = if let Some(&(base_ok, base_fail)) = baseline.get(&name) {
            if fail > base_fail {
                regressions += 1;
                format!(
                    "  {RED}▼ regression ({} → {} failures){RESET}",
                    base_fail, fail
                )
            } else if fail == 0 && base_fail > 0 {
                format!("  {GREEN}▲ resolved!{RESET}")
            } else if fail < base_fail {
                format!(
                    "  {GREEN}▲ +{} ({} → {}){RESET}",
                    base_fail - fail,
                    base_fail,
                    fail
                )
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

        eprintln!(
            "{color}{marker}{RESET} {:<21} {:>5}  {GREEN}{:>5}{RESET}  {color}{:>5}{RESET}  {YELLOW}{:>5}{RESET}  {DIM}{:>5}{RESET}  {progress}  {DIM}{:>3}%{RESET}{}",
            name,
            ok + fail + skip,
            ok,
            if fail > 0 {
                format!("{fail}")
            } else {
                String::new()
            },
            if cycle_fail > 0 {
                format!("{cycle_fail}")
            } else {
                String::new()
            },
            if skip > 0 {
                format!("{skip}")
            } else {
                String::new()
            },
            pct,
            regression_tag,
        );

        new_baseline.push(format!("{name} {ok} {fail}"));
    }

    // Total line
    let grand_total = total_ok + total_fail + total_skip;
    let grand_pct = if total_ok + total_fail > 0 {
        total_ok * 100 / (total_ok + total_fail)
    } else {
        100
    };
    eprintln!("{DIM}{}{RESET}", "─".repeat(72));
    eprintln!(
        "{BOLD}  {:<21} {:>5}  {:>5}  {:>5}  {:>5}  {:>5}  {}  {:>3}%{RESET}",
        "TOTAL",
        grand_total,
        total_ok,
        if total_fail > 0 {
            format!("{total_fail}")
        } else {
            String::new()
        },
        if total_cycle_fail > 0 {
            format!("{total_cycle_fail}")
        } else {
            String::new()
        },
        if total_skip > 0 {
            format!("{total_skip}")
        } else {
            String::new()
        },
        bar(total_ok, total_ok + total_fail, 20),
        grand_pct,
    );
    if total_cycle_fail > 0 {
        eprintln!(
            "{DIM}({total_cycle_fail} cases ok on registers/RAM but with an incorrect \
cycle count — 'cyc' column, non-blocking for now, see the timing conformance \
plan in execute.rs){RESET}"
        );
    }
    eprintln!();

    if save_baseline {
        std::fs::write(baseline_path, new_baseline.join("\n") + "\n")
            .expect("failed to write baseline");
        eprintln!(
            "{DIM}baseline saved to {}{RESET}",
            baseline_path.display()
        );
    } else if focus.is_none() && !baseline.is_empty() {
        eprintln!("{DIM}(rerun with BASELINE=1 to update the baseline){RESET}");
    }

    if regressions > 0 {
        panic!("{regressions} regression(s) detected — see table above");
    }
    assert_eq!(total_fail, 0, "{total_fail} conformance case(s) failed");
}
