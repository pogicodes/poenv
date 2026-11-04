// A stand-in for the commands `poenv run` starts in tests, so they need nothing else installed.
//   child exit N              exit with N
//   child env-is KEY VALUE    exit 0 when $KEY is exactly VALUE, else 1
//   child mask-demo           print $DB, $OPENAI_API_KEY in base64 (stderr), then "done"
//   child last-arg ...        print the last argument
//   child spawn CMD ARGS...   run CMD and copy its stdout and stderr to stdout
//   child ancestor            print the agent poenv finds among this process's parents
use base64::Engine;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("exit") => std::process::exit(args[1].parse().unwrap()),
        Some("env-is") => std::process::exit(if var(&args[1]) == args[2] { 0 } else { 1 }),
        Some("mask-demo") => {
            println!("db {}", var("DB"));
            eprintln!("key {}", base64::engine::general_purpose::STANDARD.encode(var("OPENAI_API_KEY")));
            println!("done");
        }
        Some("last-arg") => println!("{}", args.last().unwrap()),
        Some("spawn") => {
            let out = std::process::Command::new(&args[1])
                .args(&args[2..])
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            let mut stdout = std::io::stdout();
            stdout.write_all(&out.stdout).unwrap();
            stdout.write_all(&out.stderr).unwrap();
        }
        Some("ancestor") => println!("{}", poenv::human::agent_ancestor().unwrap_or("none")),
        _ => std::process::exit(2),
    }
}
