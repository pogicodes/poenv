// Strict option parsing, like Node's util.parseArgs({ strict: true, allowPositionals: true }).
use crate::errors::{PoenvError, Res, code};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Str,
}

#[derive(Clone)]
pub struct Opt {
    pub name: &'static str,
    pub kind: Kind,
    pub short: Option<char>,
}

pub const fn flag(name: &'static str) -> Opt {
    Opt { name, kind: Kind::Bool, short: None }
}
pub const fn string(name: &'static str) -> Opt {
    Opt { name, kind: Kind::Str, short: None }
}

#[derive(Default)]
pub struct Parsed {
    strings: HashMap<String, String>,
    flags: HashMap<String, bool>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn str(&self, name: &str) -> Option<String> {
        self.strings.get(name).cloned()
    }
    pub fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
}

pub fn parse(command: &str, args: &[String], opts: &[Opt]) -> Res<Parsed> {
    parse_inner(args, opts).map_err(|m| PoenvError::fix(m, code::ERROR, format!("poenv {command} --help")))
}

fn parse_inner(args: &[String], opts: &[Opt]) -> Result<Parsed, String> {
    let mut out = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            out.positionals.extend(args[i..].iter().cloned());
            break;
        }
        let (opt, inline, shown) = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            match opts.iter().find(|o| o.name == name) {
                Some(o) => (o, inline, format!("--{name}")),
                None => return Err(unknown(&format!("--{name}"))),
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            let c = arg[1..].chars().next().unwrap();
            match opts.iter().find(|o| o.short == Some(c)) {
                Some(o) if arg.chars().count() == 2 => (o, None, format!("-{c}")),
                _ => return Err(unknown(&format!("-{c}"))),
            }
        } else {
            out.positionals.push(arg.clone());
            continue;
        };
        match opt.kind {
            Kind::Bool => {
                if inline.is_some() {
                    return Err(format!("Option '{shown}' does not take an argument"));
                }
                out.flags.insert(opt.name.to_string(), true);
            }
            Kind::Str => {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        let Some(next) = args.get(i) else {
                            return Err(format!("Option '{shown} <value>' argument missing"));
                        };
                        if next.starts_with('-') {
                            return Err(format!(
                                "Option '{shown}' argument is ambiguous.\nDid you forget to specify the option argument for '{shown}'?\nTo specify an option argument starting with a dash use '--{}=-XYZ'.",
                                opt.name
                            ));
                        }
                        i += 1;
                        next.clone()
                    }
                };
                out.strings.insert(opt.name.to_string(), value);
            }
        }
    }
    Ok(out)
}

fn unknown(name: &str) -> String {
    format!(
        "Unknown option '{name}'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"{name}\"'"
    )
}
