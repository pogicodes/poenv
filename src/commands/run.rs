use super::{Ctx, guard_switch, require_app};
use crate::errors::{PoenvError, Res, code};
use crate::human::typing_person;
use crate::mask::{mask_patterns, pipe_masked};
use crate::project::{current_env, require_project};
use crate::store::model::{get_env, resolve_env};
use crate::store::open_store;
use std::process::{Command, Stdio};

pub const HELP: &str = "poenv run [ENV] -- COMMAND [ARGS…]

Run COMMAND with ENV's variables injected (default: the env this repo is on).
The env file isn't touched, and values are passed to COMMAND exactly.
When an agent or script runs it, values in COMMAND's output are shown as ***.";

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let sep = args.iter().position(|a| a == "--");
    let (before, command) = match sep {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &args[args.len()..]),
    };
    if command.is_empty() || before.len() > 1 {
        return Err(PoenvError::fix("Expected: poenv run [ENV] -- COMMAND", code::ERROR, "poenv run --help"));
    }
    let project = require_project(&ctx.cwd)?;
    let store = open_store()?.data;
    require_app(&store, &project)?;
    let env = match before.first() {
        Some(e) => e.clone(),
        None => current_env(&project)?,
    };
    get_env(&store, &project.app, &env)?;
    guard_switch(ctx, &store, &project.app, &env)?;

    let vars = resolve_env(&store, &project.app, &env)?;
    let patterns = if typing_person() { vec![] } else { mask_patterns(vars.values()) };
    let piped = !patterns.is_empty();
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).current_dir(&ctx.cwd).envs(vars.iter()).stdin(Stdio::inherit());
    if piped {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|_| {
        PoenvError::fix(
            format!("Couldn't start \"{}\".", command[0]),
            code::ERROR,
            format!("check that {} is installed and on PATH", command[0]),
        )
    })?;
    signals::forward_to(child.id());
    let threads = if piped {
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let (p1, p2) = (patterns.clone(), patterns);
        vec![
            std::thread::spawn(move || pipe_masked(out, std::io::stdout(), p1)),
            std::thread::spawn(move || pipe_masked(err, std::io::stderr(), p2)),
        ]
    } else {
        vec![]
    };
    let status = child.wait()?;
    for t in threads {
        let _ = t.join();
    }
    signals::stop();
    Ok(exit_code(status))
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(c) = status.code() {
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

/// SIGINT and SIGTERM reach poenv; pass them on to the child it started.
mod signals {
    #[cfg(unix)]
    mod imp {
        use std::sync::atomic::{AtomicI32, Ordering};
        static CHILD: AtomicI32 = AtomicI32::new(0);

        extern "C" fn on_signal(sig: libc::c_int) {
            let pid = CHILD.load(Ordering::SeqCst);
            if pid > 0 {
                unsafe { libc::kill(pid, sig) };
            }
        }

        pub fn forward_to(pid: u32) {
            CHILD.store(pid as i32, Ordering::SeqCst);
            unsafe {
                libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
                libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            }
        }

        pub fn stop() {
            CHILD.store(0, Ordering::SeqCst);
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
            }
        }
    }
    #[cfg(not(unix))]
    mod imp {
        // On Windows, Ctrl-C reaches every process attached to the console, the child included.
        pub fn forward_to(_: u32) {}
        pub fn stop() {}
    }
    pub use imp::*;
}
