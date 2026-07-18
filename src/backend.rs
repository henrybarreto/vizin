//! Typed API over the rizin pipe: analysis data, disassembly, Ghidra decompilation,
//! xrefs, symbols, editing (rename/comment/patch) and project persistence.

use crate::pipe::RzPipe;
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct FunctionInfo {
    pub offset: u64,
    pub name: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Instr {
    pub offset: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub disasm: String,
    #[serde(default)]
    pub bytes: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub jump: Option<u64>,
    /// data/pointer this instruction references (e.g. a `lea reg, str.Foo`
    /// operand) — rizin's `ptr` field, only meaningful when `refptr` is set.
    #[serde(default)]
    pub ptr: Option<u64>,
    #[serde(default)]
    pub flags: Vec<String>,
    /// base64-encoded in pdj output
    #[serde(default)]
    pub comment: Option<String>,
}

impl Instr {
    pub fn comment_text(&self) -> Option<String> {
        let raw = self.comment.as_deref()?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .unwrap_or_else(|_| raw.as_bytes().to_vec());
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Annotation {
    pub start: usize,
    pub end: usize,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub syntax_highlight: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecompResult {
    pub code: String,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Xref {
    pub from: u64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub opcode: String,
    #[serde(default)]
    pub fcn_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct XrefFrom {
    #[serde(rename = "to")]
    pub to: u64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StringInfo {
    pub vaddr: u64,
    #[serde(default)]
    pub section: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub string: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub bind: String,
    #[serde(default)]
    pub plt: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vaddr: u64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SegmentInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vaddr: u64,
    #[serde(default, alias = "vsize")]
    pub size: u64,
    #[serde(default)]
    pub perm: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BinInfo {
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub bits: u64,
}

pub struct Backend {
    pipe: RzPipe,
    pub writable: bool,
    pub has_ghidra: bool,
    pub file: String,
}

impl Backend {
    fn from_value<T: serde::de::DeserializeOwned>(v: serde_json::Value, cmd: &str) -> Result<T> {
        serde_json::from_value(v).with_context(|| format!("unexpected JSON shape for {cmd}"))
    }

    /// Run a rizin JSON command and deserialize its output as `T`.
    fn query<T: serde::de::DeserializeOwned>(&mut self, cmd: &str) -> Result<T> {
        Self::from_value(self.pipe.cmdj(cmd)?, cmd)
    }

    pub fn open(file: &str, writable: bool, project: Option<&str>) -> Result<Self> {
        let mut pipe = RzPipe::open(file, writable, project)?;
        let plugins = pipe.cmd("Lc").unwrap_or_default();
        let has_ghidra = plugins.to_lowercase().contains("ghidra");
        Ok(Self {
            pipe,
            writable,
            has_ghidra,
            file: file.to_string(),
        })
    }

    pub fn analyze(&mut self) -> Result<()> {
        self.pipe.cmd("aaa")?;
        Ok(())
    }

    pub fn bin_info(&mut self) -> Result<BinInfo> {
        let v = self.pipe.cmdj("ij")?;
        let bin = v.get("bin").cloned().unwrap_or_else(|| serde_json::json!({}));
        Self::from_value(bin, "ij.bin")
    }

    pub fn functions(&mut self) -> Result<Vec<FunctionInfo>> {
        self.query("aflj")
    }

    pub fn function_at(&mut self, addr: u64) -> Result<Option<FunctionInfo>> {
        let v = self.pipe.cmdj(&format!("afij @ {addr:#x}"))?;
        let fns: Vec<FunctionInfo> = Self::from_value(v, "afij").unwrap_or_default();
        Ok(fns.into_iter().next())
    }

    pub fn disasm(&mut self, addr: u64, count: usize) -> Result<Vec<Instr>> {
        self.query(&format!("pdj {count} @ {addr:#x}"))
    }

    /// Best-effort backwards disassembly ending just before `addr`.
    pub fn disasm_back(&mut self, addr: u64, count: usize) -> Result<Vec<Instr>> {
        let mut instrs: Vec<Instr> = self.query(&format!("pdj -{count} @ {addr:#x}"))?;
        instrs.retain(|i| i.offset < addr);
        Ok(instrs)
    }

    pub fn xrefs_to(&mut self, addr: u64) -> Result<Vec<Xref>> {
        self.query(&format!("axtj @ {addr:#x}"))
    }

    pub fn xrefs_from(&mut self, addr: u64) -> Result<Vec<XrefFrom>> {
        self.query(&format!("axfj @ {addr:#x}"))
    }

    pub fn strings(&mut self) -> Result<Vec<StringInfo>> {
        self.query("izzj")
    }

    pub fn imports(&mut self) -> Result<Vec<ImportInfo>> {
        self.query("iij")
    }

    pub fn exports(&mut self) -> Result<Vec<ExportInfo>> {
        self.query("iEj")
    }

    pub fn segments(&mut self) -> Result<Vec<SegmentInfo>> {
        self.query("iSj")
    }

    /// Resolve an address expression: hex/dec number, symbol or flag name.
    pub fn resolve(&mut self, expr: &str) -> Result<u64> {
        let e = expr.trim();
        if let Some(hex) = e.strip_prefix("0x") {
            if let Ok(a) = u64::from_str_radix(hex, 16) {
                return Ok(a);
            }
        }
        if let Ok(a) = e.parse::<u64>() {
            return Ok(a);
        }
        if !Self::is_safe_name(e) {
            bail!("invalid address or symbol: {e}");
        }
        let out = self.pipe.cmd(&format!("%v {e}"))?;
        let out = out.trim();
        let addr = out
            .strip_prefix("0x")
            .and_then(|h| u64::from_str_radix(h, 16).ok())
            .or_else(|| out.parse::<u64>().ok());
        match addr {
            Some(a) if a != 0 => Ok(a),
            _ => bail!("cannot resolve: {e}"),
        }
    }

    // ----- editing -----

    // Edit methods return the rizin command they ran, so the caller can forward
    // the same command to the background decompiler instance to keep names synced.

    pub fn rename_function(&mut self, addr: u64, new: &str) -> Result<String> {
        Self::check_name(new)?;
        let cmd = format!("afn {new} @ {addr:#x}");
        self.pipe.cmd(&cmd)?;
        Ok(cmd)
    }

    pub fn rename_variable(&mut self, fcn_addr: u64, old: &str, new: &str) -> Result<String> {
        Self::check_name(old)?;
        Self::check_name(new)?;
        // Ghidra's decompiler synthesizes its own SSA temporaries (pcVar8, iVar4,
        // auVar2…) that have no backing rizin stack/register variable. `afvn`
        // silently no-ops (just an ERROR on rizin's own console, invisible to us)
        // if `old` isn't a real variable, so check first and fail loudly instead.
        if !self.is_real_variable(fcn_addr, old)? {
            bail!(
                "'{old}' is a Ghidra-only decompiler temporary, not a real variable — rizin has nothing to rename"
            );
        }
        // rizin's afvn takes the new name first: `afvn <new_name> <old_name>`.
        let cmd = format!("afvn {new} {old} @ {fcn_addr:#x}");
        self.pipe.cmd(&cmd)?;
        Ok(cmd)
    }

    /// Whether `name` is a real rizin stack/register variable of the function
    /// at `fcn_addr` (as opposed to a name Ghidra's decompiler invented on its own).
    pub fn is_real_variable(&mut self, fcn_addr: u64, name: &str) -> Result<bool> {
        Ok(self.variable_names(fcn_addr)?.iter().any(|n| n == name))
    }

    /// The string rizin detected at `addr` (e.g. the target of a `lea reg, str.Foo`).
    pub fn string_at(&mut self, addr: u64) -> Result<String> {
        #[derive(Deserialize)]
        struct StringAt {
            string: String,
        }
        let s: StringAt = self.query(&format!("psj @ {addr:#x}"))?;
        if s.string.is_empty() {
            bail!("no string at {addr:#x}");
        }
        Ok(s.string)
    }

    /// The comment set at exactly `addr`, if any (`None` if there isn't one).
    pub fn comment_at(&mut self, addr: u64) -> Result<Option<String>> {
        let out = self.pipe.cmd(&format!("CC. @ {addr:#x}"))?;
        let out = out.trim();
        if out.is_empty() {
            Ok(None)
        } else {
            Ok(Some(out.to_string()))
        }
    }

    /// Names of real stack/register-backed local variables and arguments of a function.
    fn variable_names(&mut self, fcn_addr: u64) -> Result<Vec<String>> {
        let v = self.pipe.cmdj(&format!("afvlj @ {fcn_addr:#x}"))?;
        let names = ["stack", "reg", "bp"]
            .into_iter()
            .filter_map(|key| v.get(key).and_then(|a| a.as_array()))
            .flatten()
            .filter_map(|item| item.get("name").and_then(|n| n.as_str()))
            .map(String::from)
            .collect();
        Ok(names)
    }

    /// Rename the flag exactly at `addr` (data labels, string labels...).
    pub fn rename_flag(&mut self, addr: u64, new: &str) -> Result<String> {
        Self::check_name(new)?;
        let v = self.pipe.cmdj(&format!("fdj @ {addr:#x}"))?;
        let name = v.get("name").and_then(serde_json::Value::as_str).unwrap_or_default();
        let dist = v.get("offset").and_then(serde_json::Value::as_i64).unwrap_or(-1);
        if name.is_empty() || dist != 0 {
            bail!("no flag exactly at {addr:#x}");
        }
        let name = name.to_string();
        let cmd = format!("fr {name} {new}");
        self.pipe.cmd(&cmd)?;
        Ok(cmd)
    }

    pub fn set_comment(&mut self, addr: u64, text: &str) -> Result<String> {
        let cmd = if text.is_empty() {
            format!("CC- @ {addr:#x}")
        } else {
            let b64 = base64::engine::general_purpose::STANDARD.encode(text);
            format!("CCu base64:{b64} @ {addr:#x}")
        };
        self.pipe.cmd(&cmd)?;
        Ok(cmd)
    }

    pub fn read_bytes(&mut self, addr: u64, len: usize) -> Result<Vec<u8>> {
        self.query(&format!("pxj {len} @ {addr:#x}"))
    }

    pub fn write_bytes(&mut self, addr: u64, bytes: &[u8]) -> Result<()> {
        if !self.writable {
            bail!("file opened read-only — start with -w (or :oo+) to patch");
        }
        let hex: String = bytes.iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        self.pipe.cmd(&format!("wx {hex} @ {addr:#x}"))?;
        Ok(())
    }

    /// Reopen the file in read-write mode (`oo+`).
    pub fn reopen_writable(&mut self) -> Result<()> {
        self.pipe.cmd("oo+")?;
        self.writable = true;
        Ok(())
    }

    pub fn save_project(&mut self, path: &str) -> Result<()> {
        let out = self.pipe.cmd(&format!("Ps {path}"))?;
        if out.to_lowercase().contains("error") {
            bail!("project save failed: {out}");
        }
        Ok(())
    }

    pub fn raw_cmd(&mut self, command: &str) -> Result<String> {
        self.pipe.cmd(command)
    }

    fn is_safe_name(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '@' | '$'))
    }

    fn check_name(s: &str) -> Result<()> {
        if !Self::is_safe_name(s) {
            bail!("invalid name (allowed: letters, digits, . _ : @ $): {s}");
        }
        Ok(())
    }
}
