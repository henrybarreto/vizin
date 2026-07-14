//! Minimal rizin pipe: spawn `rizin -q0`, send commands, read NUL-terminated replies.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct RzPipe {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RzPipe {
    /// Spawn rizin on `file`. `writable` opens with -w, `project` loads a .rzdb.
    pub fn open(file: &str, writable: bool, project: Option<&str>) -> Result<Self> {
        let mut cmd = Command::new("rizin");
        cmd.arg("-q0")
            .args(["-e", "scr.color=0"])
            .args(["-e", "scr.interactive=false"])
            .args(["-e", "scr.utf8=false"]);
        if writable {
            cmd.arg("-w");
        }
        if let Some(p) = project {
            cmd.args(["-p", p]);
        }
        cmd.arg("--").arg(file);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn rizin — is it installed and on PATH?")?;

        let stdin = child.stdin.take().context("no stdin on rizin process")?;
        let stdout = child.stdout.take().context("no stdout on rizin process")?;
        let mut pipe = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        // rizin prints a NUL byte once the file is loaded.
        pipe.read_reply()
            .context("rizin did not initialize (bad file?)")?;
        Ok(pipe)
    }

    fn read_reply(&mut self) -> Result<String> {
        let mut buf = Vec::new();
        let n = self.stdout.read_until(0, &mut buf)?;
        if n == 0 {
            bail!("rizin closed the pipe unexpectedly");
        }
        if buf.last() == Some(&0) {
            buf.pop();
        }
        let mut s = String::from_utf8_lossy(&buf).into_owned();
        while s.ends_with('\n') {
            s.pop();
        }
        Ok(s)
    }

    /// Run a rizin command, return its raw text output.
    pub fn cmd(&mut self, command: &str) -> Result<String> {
        self.stdin.write_all(command.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        self.read_reply()
    }

    /// Run a command and parse its output as JSON.
    pub fn cmdj(&mut self, command: &str) -> Result<serde_json::Value> {
        let out = self.cmd(command)?;
        serde_json::from_str(out.trim()).with_context(|| {
            format!(
                "invalid JSON from `{command}`: {}",
                &out.chars().take(120).collect::<String>()
            )
        })
    }
    /// Frame a reply the way rizin does (used by protocol tests).
    #[cfg(test)]
    fn frame_reply(payload: &str) -> Vec<u8> {
        let mut v = payload.as_bytes().to_vec();
        v.push(0);
        v
    }
}

impl Drop for RzPipe {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(b"q!\n");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_framing_strips_nul_and_newlines() {
        // Simulate the read side with a cursor over a framed reply.
        let framed = RzPipe::frame_reply("hello\n");
        let mut rdr = BufReader::new(std::io::Cursor::new(framed));
        let mut buf = Vec::new();
        rdr.read_until(0, &mut buf).unwrap();
        assert_eq!(buf.pop(), Some(0));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim_end_matches('\n'), "hello");
    }
}
