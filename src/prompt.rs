// Interactive prompts. They write to stderr so stdout stays clean for --json and pipes.
use crate::errors::{PoenvError, Res, code};
use crate::output::eprint;
use std::io::{BufRead, Read};

pub fn ask(question: &str, fallback: &str) -> Res<String> {
    let hint = if fallback.is_empty() { String::new() } else { format!(" [{fallback}]") };
    eprint(&format!("{question}{hint}: "));
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let answer = line.trim();
    Ok(if answer.is_empty() { fallback.to_string() } else { answer.to_string() })
}

pub fn confirm(question: &str, default_yes: bool) -> Res<bool> {
    let answer = ask(&format!("{question} {}", if default_yes { "[Y/n]" } else { "[y/N]" }), "")?.to_lowercase();
    if answer.is_empty() {
        return Ok(default_yes);
    }
    Ok(answer == "y" || answer == "yes")
}

pub fn pick(question: &str, options: &[String], current: Option<&str>) -> Res<String> {
    eprint(&format!("{question}\n"));
    for (i, o) in options.iter().enumerate() {
        let tag = if Some(o.as_str()) == current { "  (current)" } else { "" };
        eprint(&format!("  {}) {o}{tag}\n", i + 1));
    }
    let fallback = match current.and_then(|c| options.iter().position(|o| o == c)) {
        Some(i) => (i + 1).to_string(),
        None => "1".into(),
    };
    let answer = ask("Choose", &fallback)?;
    let by_number = answer.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| options.get(i));
    match by_number.or_else(|| options.iter().find(|o| **o == answer)) {
        Some(c) => Ok(c.clone()),
        None => Err(PoenvError::new(format!("\"{answer}\" isn't one of the choices."))),
    }
}

/// Reads a line without echoing it (for secret values).
pub fn ask_hidden(question: &str) -> Res<String> {
    eprint(question);
    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    let mut buf: Vec<u8> = vec![];
    let mut stdin = std::io::stdin().lock();
    let mut byte = [0u8; 1];
    let result = loop {
        match stdin.read(&mut byte) {
            Ok(0) => break Ok(()),
            Ok(_) => match byte[0] {
                b'\r' | b'\n' => break Ok(()),
                0x03 => break Err(PoenvError::code("Cancelled.", code::REFUSED)),
                0x7f | 0x08 => {
                    // drop the last UTF-8 character
                    while buf.last().is_some_and(|b| b & 0xC0 == 0x80) {
                        buf.pop();
                    }
                    buf.pop();
                }
                b => buf.push(b),
            },
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => break Err(e.into()),
        }
    };
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    eprint("\n");
    result.map(|_| String::from_utf8_lossy(&buf).into_owned())
}

/// All of stdin, minus one trailing newline (so `echo value |` works).
pub fn read_stdin() -> Res<String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    if let Some(t) = text.strip_suffix('\n') {
        text = t.strip_suffix('\r').unwrap_or(t).to_string();
    }
    Ok(text)
}
