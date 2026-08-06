//! Generic trace sink (see [`crate::bus::TracingBus`]): logs every bus
//! transaction (exact size .B/.W/.L, address, value, caller PC) to a file,
//! with address-range filtering to keep the volume manageable over a full
//! interactive session.
//!
//! Entirely driven by environment variables (same conventions as the rest
//! of the project's `RUST68_TRACE_*` probes) to stay zero-cost when
//! disabled — see [`FileTraceSink::from_env`].

use crate::bus::TraceSink;
use std::io::{BufWriter, Write};

/// Concrete trace sink: writes one event per line to a file, filtered to
/// an address range, with a component label supplied by the caller (the
/// address layout is system-specific — `TracingBus`/`TraceSink` stay
/// generic, this label does not).
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
    /// Builds from environment variables:
    /// - `RUST68_TRACE_ALL=1`: enables it (otherwise `None`, zero cost).
    /// - `RUST68_TRACE_ALL_PATH=<file>`: output path (default:
    ///   `rust68_trace_all.log` in the current directory).
    /// - `RUST68_TRACE_ALL_MIN` / `RUST68_TRACE_ALL_MAX` (hexadecimal, no
    ///   `0x` prefix): address filter for bus accesses — essential over a
    ///   full interactive session, or files would grow to several
    ///   gigabytes.
    /// - `RUST68_TRACE_ALL_CPU=1`: also traces every CPU step (instruction
    ///   address) — disabled by default (very verbose, rarely needed to
    ///   diagnose memory accesses).
    ///
    /// `classify`: address -> component name function, supplied by the
    /// host system (see `AtariSt::describe_addr`).
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

    /// Logs a CPU step (address of the instruction about to be executed) —
    /// to be called by the host code right before `Cpu::step` if
    /// `RUST68_TRACE_ALL_CPU` is active (see [`Self::wants_cpu_trace`]).
    pub fn cpu_step(&mut self, pc: u32) {
        let _ = writeln!(self.writer, "S pc={pc:08x}");
    }

    pub fn wants_cpu_trace(&self) -> bool {
        self.trace_cpu
    }

    /// Lightweight "breakpoint": if `pc` matches `RUST68_TRACE_ALL_BREAK`,
    /// logs the full register state (D0-D7, A0-A7, SR) — in particular
    /// `a[7]` (stack), to recover the return address of a routine whose
    /// entry point is all that's known (read directly from memory by the
    /// caller if needed; this method only logs the state). Capped at 5
    /// triggers so as not to flood the log if the address is hit in a
    /// loop. To be called by the host code right before `Cpu::step`,
    /// independently of `RUST68_TRACE_ALL_CPU`.
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
