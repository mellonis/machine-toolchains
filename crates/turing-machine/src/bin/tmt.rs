use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match mtc_turing_machine::cli::execute(&args) {
        Ok(out) => {
            print!("{}", out.stdout);
            eprint!("{}", out.stderr);
            ExitCode::from(out.code)
        }
        Err(message) => {
            eprintln!("tmt: {message}");
            ExitCode::FAILURE
        }
    }
}
