use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use wait_timeout::ChildExt;

/// A position inside the solution's source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub line: usize,
    pub column: Option<usize>,
}

/// How hard to work at pinning a crash to a line of the solution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Diagnostics {
    /// Do not look for a source location.
    Off,
    /// Only read what the program itself wrote, such as a sanitizer report.
    #[default]
    Output,
    /// Also re-run a silent crash under a debugger.
    Debugger,
}

/// Find the first line of `output` that names a position in `source`.
///
/// This understands anything that spells out `<file>:<line>[:<column>]`, which
/// covers sanitizer reports as well as LLDB and GDB backtraces. Frames from
/// other files are skipped, so a crash inside a standard library header still
/// reports the solution's own line.
#[must_use]
pub fn find_source_location(output: &str, source: &Path) -> Option<SourceLocation> {
    let file_name = source.file_name()?.to_str()?;
    let marker = format!("{file_name}:");
    output.lines().find_map(|line| {
        let (_, suffix) = line.rsplit_once(&marker)?;
        let mut parts = suffix.split(':');
        let line = parts.next()?.parse().ok()?;
        let column = parts
            .next()
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .filter(|column| *column > 0);
        (line > 0).then_some(SourceLocation { line, column })
    })
}

/// Re-run a crashing binary under a debugger and read the location off its
/// backtrace. Returns `None` when no debugger is available.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[must_use]
pub fn crash_source_location(
    binary: &Path,
    input: &Path,
    source: &Path,
    test_timeout: Duration,
) -> Option<SourceLocation> {
    let mut command = debugger_command(binary, input);
    let diagnostic_timeout = test_timeout.min(Duration::from_secs(2)) + Duration::from_secs(3);
    let output = capture_command(&mut command, diagnostic_timeout)?;
    find_source_location(&output, source)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[must_use]
pub fn crash_source_location(
    _binary: &Path,
    _input: &Path,
    _source: &Path,
    _test_timeout: Duration,
) -> Option<SourceLocation> {
    None
}

#[cfg(target_os = "macos")]
fn debugger_command(binary: &Path, input: &Path) -> Command {
    let input_command = format!(
        "settings set target.input-path {}",
        quote_lldb_argument(input)
    );
    let mut command = Command::new("lldb");
    command
        .arg("--batch")
        .arg("--no-lldbinit")
        .arg("--source-quietly")
        .arg("--no-use-colors")
        .arg("-o")
        .arg(input_command)
        .arg("-o")
        .arg("run")
        .arg("-k")
        .arg("bt 16")
        .arg("-k")
        .arg("process kill")
        .arg("-k")
        .arg("quit")
        .arg(binary);
    command
}

#[cfg(target_os = "linux")]
fn debugger_command(binary: &Path, input: &Path) -> Command {
    let run_command = format!("run < {}", quote_shell_argument(input));
    let mut command = Command::new("gdb");
    command
        .arg("--batch")
        .arg("--quiet")
        .arg("-ex")
        .arg("set pagination off")
        .arg("-ex")
        .arg(run_command)
        .arg("-ex")
        .arg("bt 16")
        .arg("--args")
        .arg(binary);
    command
}

#[cfg(target_os = "macos")]
fn quote_lldb_argument(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

#[cfg(target_os = "linux")]
fn quote_shell_argument(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn capture_command(command: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let stdout_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).map(|_| buffer)
    });
    let stderr_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer).map(|_| buffer)
    });

    let finished = child.wait_timeout(timeout).ok()?.is_some();
    if !finished {
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut output = stdout_thread.join().ok()?.ok()?;
    output.extend(stderr_thread.join().ok()?.ok()?);
    Some(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{SourceLocation, find_source_location};

    #[test]
    fn extracts_source_location_from_sanitizer_reports() {
        let source = PathBuf::from("/work/attempt/main.cpp");
        let address = "    #0 0x000102ddca90 in main main.cpp:5\n";
        let undefined = concat!(
            "main.cpp:4:15: runtime error: signed integer overflow: ",
            "2147483647 + 1 cannot be represented in type 'int'\n",
        );

        assert_eq!(
            find_source_location(address, &source),
            Some(SourceLocation {
                line: 5,
                column: None,
            })
        );
        assert_eq!(
            find_source_location(undefined, &source),
            Some(SourceLocation {
                line: 4,
                column: Some(15),
            })
        );
    }

    #[test]
    fn skips_sanitizer_frames_outside_the_solution() {
        let source = PathBuf::from("/work/attempt/main.cpp");
        let report = concat!(
            "==10907==ERROR: AddressSanitizer: heap-buffer-overflow on address 0x6020000000fc\n",
            "READ of size 4 at 0x6020000000fc thread T0\n",
            "    #0 0x000102ddca90 in std::__1::vector<int>::operator[] vector:1387\n",
            "    #1 0x000102ddca90 in main main.cpp:9:20\n",
        );

        assert_eq!(
            find_source_location(report, &source),
            Some(SourceLocation {
                line: 9,
                column: Some(20),
            })
        );
    }

    #[test]
    fn extracts_source_location_from_lldb_and_gdb_backtraces() {
        let source = PathBuf::from("/work/attempt/main.cpp");
        let lldb = "frame #5: binary`main at main.cpp:15:34\n";
        let gdb = "#5  main () at /work/attempt/main.cpp:27\n";

        assert_eq!(
            find_source_location(lldb, &source),
            Some(SourceLocation {
                line: 15,
                column: Some(34),
            })
        );
        assert_eq!(
            find_source_location(gdb, &source),
            Some(SourceLocation {
                line: 27,
                column: None,
            })
        );
    }
}
