use std::io::Write;
use std::process::ExitCode;

/// Writes `text` through `writer` and treats a closed pipe as a normal end
/// of output (the reader closed it on purpose, e.g. `| head`) rather than
/// the panic `print!`/`eprint!` would raise: Rust binaries ignore
/// `SIGPIPE`, so a write past a closed pipe surfaces as `EPIPE`, and the
/// stdlib macros `.expect()` that away into a panic. Returns `Ok(())` on a
/// clean write OR a broken pipe; any other write error is `Err`.
fn write_or_broken_pipe(writer: &mut dyn Write, text: &str) -> std::io::Result<()> {
    match writer.write_all(text.as_bytes()) {
        Ok(()) => writer.flush().or_else(ignore_broken_pipe),
        Err(err) => ignore_broken_pipe(err),
    }
}

fn ignore_broken_pipe(err: std::io::Error) -> std::io::Result<()> {
    if err.kind() == std::io::ErrorKind::BrokenPipe {
        Ok(())
    } else {
        Err(err)
    }
}

/// Best-effort one-line error report: a broken stderr pipe here must not
/// itself panic, so failures writing the message are silently dropped —
/// the process is already on its way to `ExitCode::FAILURE`.
fn report_error(message: &str) {
    let stderr = std::io::stderr();
    let _ = write_or_broken_pipe(&mut stderr.lock(), &format!("pmt: {message}\n"));
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match mtc_post_machine::cli::execute(&args) {
        Ok(out) => {
            let stdout = std::io::stdout();
            if let Err(err) = write_or_broken_pipe(&mut stdout.lock(), &out.stdout) {
                report_error(&err.to_string());
                return ExitCode::FAILURE;
            }
            let stderr = std::io::stderr();
            if let Err(err) = write_or_broken_pipe(&mut stderr.lock(), &out.stderr) {
                report_error(&err.to_string());
                return ExitCode::FAILURE;
            }
            ExitCode::from(out.code)
        }
        Err(message) => {
            report_error(&message);
            ExitCode::FAILURE
        }
    }
}
