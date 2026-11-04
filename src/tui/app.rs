// The poenv TUI: browse apps → environments → keys, and edit them. SPEC §8.
use crate::commands::secrets::mask;
use crate::errors::{PoenvError, Res};
use crate::project::{Project, current_env, has_block, write_env_file, write_marker};
use crate::repos::{refresh_other_repos, repos_for_app};
use crate::store::StoreHandle;
use crate::store::model::{
    RemoveMode, Store, Target, Var, create_env, move_to_shared, remove_env, remove_shared, remove_var, rename_env,
    resolve_env, set_protected, set_ref, set_var, shared_uses,
};
use crate::util::home_dir;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::path::Path;

pub const SHARED: &str = "★ shared";
const PAGE: usize = 14;
const PICK_PAGE: usize = 8;

/// A path with the home folder shortened to ~, for messages.
fn tilde(path: &str) -> String {
    let home = home_dir().to_string_lossy().into_owned();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

type Submit = Box<dyn FnOnce(&mut App, String)>;
type Yes = Box<dyn FnOnce(&mut App)>;

enum Mode {
    Browse,
    Input { label: String, hidden: bool, value: String, on_submit: Submit },
    Pick { label: String, items: Vec<String>, index: usize, on_pick: Submit },
    Typed { label: String, word: String, value: String, on_yes: Yes },
    YesNo { label: String, on_yes: Yes },
}

#[derive(PartialEq)]
enum View {
    Apps,
    App,
}

struct Message {
    text: String,
    ok: bool,
}

pub struct App {
    store: StoreHandle,
    project: Option<Project>,
    /// The folder poenv was started in (for the init hint).
    cwd_name: String,
    /// Whether [i] can hand off to `poenv init` (outside a set-up repo).
    can_init: bool,
    pub want_init: bool,
    pub quit: bool,
    file_env: Option<String>,
    view: View,
    app: String,
    app_index: usize,
    env_index: usize,
    key_index: usize,
    mode: Mode,
    message: Option<Message>,
    revealed: Vec<String>,
}

fn style() -> Style {
    Style::default()
}
fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), style().add_modifier(Modifier::DIM))
}
fn bold(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), style().add_modifier(Modifier::BOLD))
}
fn colored(s: impl Into<String>, c: Color) -> Span<'static> {
    Span::styled(s.into(), style().fg(c))
}
fn plain(s: impl Into<String>) -> Span<'static> {
    Span::raw(s.into())
}
fn pad_end(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width { s.to_string() } else { format!("{s}{}", " ".repeat(width - n)) }
}
fn inverse(line: Line<'static>, on: bool) -> Line<'static> {
    if on { line.patch_style(style().add_modifier(Modifier::REVERSED)) } else { line }
}

impl App {
    pub fn new(store: StoreHandle, project: Option<Project>, cwd: &Path, with_init: bool) -> App {
        let in_repo = project.as_ref().is_some_and(|p| store.data.apps.contains_key(&p.app));
        let file_env = project
            .as_ref()
            .filter(|p| has_block(p).unwrap_or(false))
            .and_then(|p| current_env(p).ok());
        let app_rows = app_rows(&store.data);
        let app = if in_repo { project.as_ref().unwrap().app.clone() } else { app_rows[0].clone() };
        let env_index = if in_repo {
            let p = project.as_ref().unwrap();
            let envs: Vec<&String> = store.data.apps[&p.app].envs.keys().collect();
            let want = file_env.clone().unwrap_or_else(|| p.env.clone());
            envs.iter().position(|e| **e == want).unwrap_or(0)
        } else {
            0
        };
        App {
            cwd_name: cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            can_init: with_init && !in_repo,
            want_init: false,
            quit: false,
            file_env,
            view: if in_repo { View::App } else { View::Apps },
            app,
            app_index: 0,
            env_index,
            key_index: 0,
            mode: Mode::Browse,
            message: None,
            revealed: vec![],
            store,
            project,
        }
    }

    #[cfg(test)]
    pub fn store(&self) -> &Store {
        &self.store.data
    }

    // --- what's selected right now ---
    fn is_shared(&self) -> bool {
        self.app == SHARED
    }
    fn envs(&self) -> Vec<String> {
        if self.is_shared() {
            return vec![];
        }
        self.store.data.apps.get(&self.app).map(|a| a.envs.keys().cloned().collect()).unwrap_or_default()
    }
    fn env(&self) -> String {
        let envs = self.envs();
        if envs.is_empty() {
            return String::new();
        }
        envs[self.env_index.min(envs.len() - 1)].clone()
    }
    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = if self.is_shared() {
            self.store.data.shared.keys().cloned().collect()
        } else {
            let env = self.env();
            self.store
                .data
                .apps
                .get(&self.app)
                .and_then(|a| a.envs.get(&env))
                .map(|e| e.vars.keys().cloned().collect())
                .unwrap_or_default()
        };
        keys.sort();
        keys
    }
    fn ki(&self) -> usize {
        self.key_index.min(self.keys().len().saturating_sub(1))
    }
    fn key(&self) -> Option<String> {
        self.keys().get(self.ki()).cloned()
    }
    /// The variable at `name` in the selected env (shared pool: literals).
    fn var(&self, name: &str) -> Option<Var> {
        if self.is_shared() {
            return self.store.data.shared.get(name).cloned().map(Var::Lit);
        }
        self.store.data.apps.get(&self.app)?.envs.get(&self.env())?.vars.get(name).cloned()
    }
    fn target(&self) -> Target {
        if self.is_shared() { Target::Shared } else { Target::app(&self.app, &self.env()) }
    }
    fn where_(&self) -> String {
        if self.is_shared() { "shared".into() } else { format!("{}/{}", self.app, self.env()) }
    }
    fn is_this_repo(&self) -> bool {
        self.project.as_ref().is_some_and(|p| !self.is_shared() && p.app == self.app)
    }
    fn protected(&self, app: &str, env: &str) -> bool {
        self.store.data.apps.get(app).and_then(|a| a.envs.get(env)).is_some_and(|e| e.protected)
    }

    fn ok(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), ok: true });
    }
    fn fail(&mut self, err: PoenvError) {
        self.message = Some(Message { text: err.problem, ok: false });
    }
    fn browse(&mut self) {
        self.mode = Mode::Browse;
    }
    fn input(&mut self, label: String, hidden: bool, initial: &str, on_submit: impl FnOnce(&mut App, String) + 'static) {
        self.mode = Mode::Input { label, hidden, value: initial.to_string(), on_submit: Box::new(on_submit) };
    }
    fn pick(&mut self, label: String, items: Vec<String>, on_pick: impl FnOnce(&mut App, String) + 'static) {
        self.mode = Mode::Pick { label, items, index: 0, on_pick: Box::new(on_pick) };
    }
    fn yes_no(&mut self, label: String, on_yes: impl FnOnce(&mut App) + 'static) {
        self.mode = Mode::YesNo { label, on_yes: Box::new(on_yes) };
    }
    fn typed(&mut self, label: String, word: String, on_yes: impl FnOnce(&mut App) + 'static) {
        self.mode = Mode::Typed { label, word, value: String::new(), on_yes: Box::new(on_yes) };
    }

    /// Apply a store change, rewrite the env files it affects (this repo's and other repos'), and report.
    fn change(&mut self, f: impl FnOnce(&mut Store) -> Res<()>, done: String) {
        self.browse();
        if let Err(e) = self.store.update(f) {
            return self.fail(e);
        }
        let mut note = String::new();
        if let Some(p) = &self.project {
            if has_block(p).unwrap_or(false) {
                // e.g. a dangling reference in the file's env; doctor reports it
                let r = current_env(p)
                    .and_then(|on| Ok((on.clone(), resolve_env(&self.store.data, &p.app, &on)?)))
                    .and_then(|(on, vars)| write_env_file(p, &on, &vars));
                if let Ok(r) = r {
                    if r.diff.count() > 0 {
                        note = format!("  ({} updated)", p.file);
                    }
                }
            }
        }
        let (updated, skipped) = refresh_other_repos(&self.store.data, self.project.as_ref().map(|p| p.root.as_str()));
        if !updated.is_empty() {
            let n = updated.len();
            note += &format!("  ({n} other repo{} updated)", if n == 1 { "" } else { "s" });
        }
        if !skipped.is_empty() {
            let names: Vec<String> = skipped
                .iter()
                .map(|s| Path::new(&s.root).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .collect();
            self.message = Some(Message {
                text: format!("{done}{note}. Couldn't update {}: run poenv use there", names.join(", ")),
                ok: false,
            });
            return;
        }
        self.ok(format!("{done}{note}"));
    }

    /// Switches the env file of this repo, or, for another app, of its repos on this machine (from the
    /// repo registry). The CLI's `poenv use` stays this-folder-only; the TUI is people-only.
    fn switch_file(&mut self) {
        let repos = if self.is_this_repo() { vec![self.project.clone().unwrap()] } else { repos_for_app(&self.app) };
        if repos.is_empty() {
            return self.fail(PoenvError::new(format!(
                "No repo for {} on this machine yet. Run poenv inside it once, then switch here.",
                self.app
            )));
        }
        let (app, env) = (self.app.clone(), self.env());
        let choose = move |me: &mut App| {
            if repos.len() == 1 {
                return me.switch_repos(&app, &env, repos);
            }
            let all = format!("All {}", repos.len());
            let mut items: Vec<String> = repos.iter().map(|p| tilde(&p.root)).collect();
            items.push(all.clone());
            let label = format!("Switch which {app} repo to {env}?");
            me.pick(label, items, move |me, item| {
                let targets = if item == all { repos } else { repos.into_iter().filter(|p| tilde(&p.root) == item).collect() };
                me.switch_repos(&app, &env, targets);
            });
        };
        if self.protected(&self.app, &self.env()) {
            let label = format!("{}/{} is protected.", self.app, self.env());
            self.typed(label, self.env(), choose);
        } else {
            choose(self);
        }
    }

    fn switch_repos(&mut self, app: &str, env: &str, targets: Vec<Project>) {
        self.browse();
        let result = (|| -> Res<Vec<usize>> {
            let vars = resolve_env(&self.store.data, app, env)?;
            let mut changed = vec![];
            for p in &targets {
                let r = write_env_file(p, env, &vars)?;
                if self.project.as_ref().is_some_and(|me| me.root == p.root) {
                    self.file_env = Some(env.to_string());
                }
                changed.push(r.diff.count());
            }
            Ok(changed)
        })();
        let changed = match result {
            Ok(c) => c,
            Err(e) => return self.fail(e),
        };
        if targets.len() > 1 {
            return self.ok(format!("{} {app} repos are now on {env}", targets.len()));
        }
        let p = &targets[0];
        let name = if self.project.as_ref().is_some_and(|me| me.root == p.root) {
            p.file.clone()
        } else {
            tilde(&Path::new(&p.root).join(&p.file).to_string_lossy())
        };
        let n = changed[0];
        self.ok(format!("{name} is now on {env} ({n} key{} changed)", if n == 1 { "" } else { "s" }));
    }

    fn share(&mut self, name: String) {
        let Some(v) = self.var(&name) else { return };
        if self.is_shared() {
            return;
        }
        let v = match v {
            Var::Ref(r) => return self.fail(PoenvError::new(format!("{name} is already shared ({}).", r.target))),
            Var::Lit(v) => v,
        };
        let existing = self.store.data.shared.get(&name).cloned();
        if existing.as_ref().is_some_and(|e| e.value != v.value) {
            return self.fail(PoenvError::new(format!(
                "A different shared {name} already exists. Rename one of them first."
            )));
        }
        let (app, env) = (self.app.clone(), self.env());
        let done = if existing.is_some() {
            format!("{name} now uses the shared value")
        } else {
            format!("{name} is now shared. To undo: d on it in {SHARED}, then keep copies")
        };
        let label = format!("Move {name} to shared? Every app that uses it changes when you rotate it.");
        self.yes_no(label, move |me| me.change(move |s| move_to_shared(s, &app, &env, &name).map(|_| ()), done));
    }

    /// Like `poenv env rename`: the env file's block and .poenv follow the new name.
    fn rename_current_env(&mut self) {
        let (app, env, where_) = (self.app.clone(), self.env(), self.where_());
        let ask = move |me: &mut App| {
            let label = format!("Rename {where_} to:");
            let initial = env.clone();
            me.input(label, false, &initial, move |me, name| {
                let n = name.trim().to_string();
                me.browse();
                if n.is_empty() || n == env {
                    return;
                }
                let result = (|| -> Res<()> {
                    me.store.update(|s| rename_env(s, &app, &env, &n))?;
                    if me.is_this_repo() && me.file_env.as_deref() == Some(env.as_str()) {
                        let p = me.project.clone().unwrap();
                        write_env_file(&p, &n, &resolve_env(&me.store.data, &app, &n)?)?;
                        me.file_env = Some(n.clone());
                    }
                    if me.is_this_repo() && me.project.as_ref().is_some_and(|p| p.env == env) {
                        let p = me.project.as_mut().unwrap();
                        p.env = n.clone();
                        write_marker(p)?;
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => me.ok(format!("Renamed {where_} to {app}/{n}")),
                    Err(e) => me.fail(e),
                }
            });
        };
        if self.protected(&self.app, &self.env()) {
            let label = format!("{} is protected.", self.where_());
            self.typed(label, self.env(), ask);
        } else {
            ask(self);
        }
    }

    /// Deleting a shared key that apps use asks what happens to them, so nothing is left broken.
    fn delete_shared(&mut self, name: String) {
        let uses = shared_uses(&self.store.data, &name);
        if uses.is_empty() {
            let label = format!("Delete {name} from shared?");
            return self.yes_no(label, move |me| {
                let done = format!("Deleted {name} from shared");
                me.change(move |s| remove_var(s, &Target::Shared, &name), done)
            });
        }
        let places: Vec<String> = uses.iter().map(|u| format!("{}/{}", u.app, u.env)).collect();
        let places = places.join(", ");
        let keep = "Keep a copy in each (they stop following the shared value)".to_string();
        let items = vec![keep.clone(), "Delete it everywhere".to_string()];
        let label = format!("{name} is used in {places}. Delete the shared key and:");
        self.pick(label, items, move |me, choice| {
            if choice == keep {
                let done = format!("{name} is no longer shared; {places} keep their own copy");
                me.change(move |s| remove_shared(s, &name, RemoveMode::KeepCopies), done);
            } else {
                let done = format!("Deleted {name} from shared and {places}");
                me.change(move |s| remove_shared(s, &name, RemoveMode::Everywhere), done);
            }
        });
    }

    /// Add a live reference to a shared key: pick it, then name it in this env.
    fn add_ref(&mut self) {
        if self.is_shared() {
            return;
        }
        let mut items: Vec<String> = self.store.data.shared.keys().cloned().collect();
        items.sort();
        if items.is_empty() {
            return self.fail(PoenvError::new(format!(
                "{SHARED} is empty. Add a key there first, or press s on a key to move it."
            )));
        }
        let (app, env, where_) = (self.app.clone(), self.env(), self.where_());
        self.pick(format!("Shared key to use in {where_}:"), items, move |me, r| {
            let label = format!("Name in {where_}:");
            let initial = r.clone();
            me.input(label, false, &initial, move |me, name| {
                let n = name.trim().to_string();
                if n.is_empty() {
                    return me.browse();
                }
                let exists = me.var(&n).is_some();
                let run = move |me: &mut App| {
                    let done = format!("Added {n} to {where_} (→ shared {r})");
                    me.change(move |s| set_ref(s, &app, &env, &n, &r), done);
                };
                if exists {
                    let label = format!("{} already exists in {}. Replace it?", name.trim(), me.where_());
                    me.yes_no(label, run);
                } else {
                    run(me);
                }
            });
        });
    }

    pub fn handle(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::Browse => self.browse_key(key),
            Mode::Input { label, hidden, mut value, on_submit } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => on_submit(self, value),
                KeyCode::Backspace | KeyCode::Delete => {
                    value.pop();
                    self.mode = Mode::Input { label, hidden, value, on_submit };
                }
                KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    value.push(c);
                    self.mode = Mode::Input { label, hidden, value, on_submit };
                }
                _ => self.mode = Mode::Input { label, hidden, value, on_submit },
            },
            Mode::Typed { label, word, mut value, on_yes } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if value == word {
                        on_yes(self);
                    } else {
                        self.fail(PoenvError::new("Cancelled."));
                    }
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    value.pop();
                    self.mode = Mode::Typed { label, word, value, on_yes };
                }
                KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    value.push(c);
                    self.mode = Mode::Typed { label, word, value, on_yes };
                }
                _ => self.mode = Mode::Typed { label, word, value, on_yes },
            },
            Mode::Pick { label, items, mut index, on_pick } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let item = items[index].clone();
                    on_pick(self, item);
                }
                KeyCode::Up => {
                    index = index.saturating_sub(1);
                    self.mode = Mode::Pick { label, items, index, on_pick };
                }
                KeyCode::Down => {
                    index = (index + 1).min(items.len() - 1);
                    self.mode = Mode::Pick { label, items, index, on_pick };
                }
                _ => self.mode = Mode::Pick { label, items, index, on_pick },
            },
            Mode::YesNo { label, on_yes } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => on_yes(self),
                KeyCode::Char(_) | KeyCode::Enter | KeyCode::Esc => {}
                _ => self.mode = Mode::YesNo { label, on_yes },
            },
        }
    }

    fn browse_key(&mut self, k: KeyEvent) {
        self.message = None;
        let ch = match k.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        if ch == Some('q') {
            self.quit = true;
            return;
        }
        if ch == Some('i') && self.can_init {
            self.want_init = true;
            self.quit = true;
            return;
        }
        let rows = app_rows(&self.store.data);
        if self.view == View::Apps {
            match (k.code, ch) {
                (KeyCode::Up, _) | (_, Some('k')) => self.app_index = self.app_index.saturating_sub(1),
                (KeyCode::Down, _) | (_, Some('j')) => self.app_index = (self.app_index + 1).min(rows.len() - 1),
                (KeyCode::Enter | KeyCode::Right, _) | (_, Some('l')) => {
                    let chosen = rows[self.app_index].clone();
                    let envs: Vec<String> = if chosen == SHARED {
                        vec![]
                    } else {
                        self.store.data.apps[&chosen].envs.keys().cloned().collect()
                    };
                    let mine = self.project.as_ref().is_some_and(|p| p.app == chosen);
                    self.env_index = match (&self.file_env, mine) {
                        (Some(f), true) => envs.iter().position(|e| e == f).unwrap_or(0),
                        _ => 0,
                    };
                    self.app = chosen;
                    self.key_index = 0;
                    self.view = View::App;
                }
                _ => {}
            }
            return;
        }

        if matches!(k.code, KeyCode::Esc | KeyCode::Backspace | KeyCode::Delete) {
            self.app_index = rows.iter().position(|r| *r == self.app).unwrap_or(0);
            self.view = View::Apps;
            return;
        }
        // Arrows, plus vim's hjkl (not shown in the hints, to keep them short).
        let envs = self.envs();
        let keys = self.keys();
        let ki = self.ki();
        match (k.code, ch) {
            (KeyCode::Left, _) | (_, Some('h')) => {
                self.env_index = self.env_index.saturating_sub(1);
                self.key_index = 0;
                return;
            }
            (KeyCode::Right | KeyCode::Tab, _) | (_, Some('l')) => {
                self.env_index = (self.env_index + 1).min(envs.len().saturating_sub(1));
                self.key_index = 0;
                return;
            }
            (KeyCode::Up, _) | (_, Some('k')) => {
                self.key_index = ki.saturating_sub(1);
                return;
            }
            (KeyCode::Down, _) | (_, Some('j')) => {
                self.key_index = (ki + 1).min(keys.len().saturating_sub(1));
                return;
            }
            _ => {}
        }

        // Lowercase acts on the selected key, uppercase on the whole env.
        let key = self.key();
        let shared = self.is_shared();
        let (target, where_, app, env) = (self.target(), self.where_(), self.app.clone(), self.env());
        match ch {
            Some('u') if !shared => self.switch_file(),
            Some('a') => {
                let w = where_.clone();
                self.input(format!("New key in {where_}:"), false, "", move |me, name| {
                    let n = name.trim().to_string();
                    if n.is_empty() {
                        return me.browse();
                    }
                    me.input(format!("Value for {n}:"), true, "", move |me, value| {
                        let done = format!("Added {n} to {w}");
                        me.change(move |s| set_var(s, &target, &n, &value), done);
                    });
                });
            }
            Some('e') => {
                if let Some(k) = key {
                    self.edit(k);
                }
            }
            Some('d') => match key {
                Some(k) if shared => self.delete_shared(k),
                Some(k) => {
                    self.yes_no(format!("Delete {k} from {where_}?"), move |me| {
                        let done = format!("Deleted {k} from {where_}");
                        me.change(move |s| remove_var(s, &target, &k), done);
                    });
                }
                None => {}
            },
            Some('v') => {
                if let Some(k) = key {
                    let id = format!("{where_}/{k}");
                    match self.revealed.iter().position(|r| *r == id) {
                        Some(i) => {
                            self.revealed.remove(i);
                        }
                        None => self.revealed.push(id),
                    }
                }
            }
            Some('s') => {
                if let Some(k) = key {
                    self.share(k);
                }
            }
            Some('r') => self.add_ref(),
            Some('P') if !shared => {
                let on = self.protected(&app, &env);
                let label = if on {
                    format!("Unprotect {where_}? Agents could then switch to it.")
                } else {
                    format!("Protect {where_}?")
                };
                self.yes_no(label, move |me| {
                    let done = format!("{where_} is {} protected", if on { "no longer" } else { "now" });
                    me.change(move |s| set_protected(s, &app, &env, !on), done);
                });
            }
            Some('A') if !shared => {
                let count = envs.len();
                self.input(format!("New environment in {app}:"), false, "", move |me, name| {
                    let n = name.trim().to_string();
                    if n.is_empty() {
                        return me.browse();
                    }
                    let a = app.clone();
                    me.change(move |s| create_env(s, &a, &n), format!("Created {app}/{}", name.trim()));
                    me.env_index = count;
                });
            }
            Some('R') if !shared => self.rename_current_env(),
            Some('D') if !shared => {
                let label = format!("Delete the whole {where_} environment and its {} key(s)?", keys.len());
                self.typed(label, env.clone(), move |me| {
                    me.change(move |s| remove_env(s, &app, &env), format!("Deleted {where_}"));
                    me.env_index = 0;
                });
            }
            _ => {
                if k.code == KeyCode::Enter {
                    if let Some(k) = key {
                        self.edit(k);
                    }
                }
            }
        }
    }

    fn edit(&mut self, name: String) {
        if let Some(Var::Ref(r)) = self.var(&name) {
            return self.fail(PoenvError::new(format!("{name} is a shared reference; edit {} in {SHARED}.", r.target)));
        }
        let (target, where_) = (self.target(), self.where_());
        self.input(format!("New value for {name}:"), true, "", move |me, value| {
            let done = format!("Set {name} in {where_}");
            me.change(move |s| set_var(s, &target, &name, &value), done);
        });
    }

    // --- render ---
    pub fn lines(&self) -> (Vec<Line<'static>>, Vec<Line<'static>>, Vec<Line<'static>>) {
        let mut top = vec![];
        let mut header = vec![Span::styled("poenv", style().fg(Color::Green).add_modifier(Modifier::BOLD)), plain("  ")];
        match (&self.project, &self.file_env) {
            (Some(p), Some(f)) => {
                header.extend([plain(format!("{} ", p.app)), dim("·"), plain(format!(" {} is on ", p.file)), bold(f.clone())]);
            }
            (Some(p), None) => header.push(dim(format!("{} (env file not written yet)", p.app))),
            (None, _) => header.push(dim("not inside a poenv repo")),
        }
        top.push(Line::from(header));
        if self.can_init {
            let what = match &self.project {
                Some(p) => format!("{} isn't in your vault yet.", p.app),
                None => format!("{} isn't set up for poenv.", self.cwd_name),
            };
            top.push(Line::from(vec![
                colored(what, Color::Yellow),
                dim(" Press "),
                Span::styled("[i]", style().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                dim(" to run poenv init here."),
            ]));
        }

        let data = &self.store.data;
        let mut body = vec![];
        if self.view == View::Apps {
            body.push(Line::from(dim("Apps")));
            for (i, name) in app_rows(data).iter().enumerate() {
                let desc = if name == SHARED {
                    format!("{} key(s)", data.shared.len())
                } else {
                    data.apps[name].envs.keys().cloned().collect::<Vec<_>>().join(", ")
                };
                let mark = if i == self.app_index { "› " } else { "  " };
                body.push(inverse(Line::from(vec![plain(format!("{mark}{}", pad_end(name, 24))), dim(desc)]), i == self.app_index));
            }
        } else {
            let shared = self.is_shared();
            let mut title = vec![bold(if shared { SHARED.to_string() } else { self.app.clone() })];
            if !shared {
                title.push(plain("  "));
            }
            for (i, e) in self.envs().iter().enumerate() {
                let on_file = self.is_this_repo() && self.file_env.as_deref() == Some(e.as_str());
                let text = format!(
                    " {}{e}{} ",
                    if on_file { "● " } else { "" },
                    if self.protected(&self.app, e) { " 🔒" } else { "" }
                );
                let s = if i == self.env_index.min(self.envs().len().saturating_sub(1)) {
                    style().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    style()
                };
                title.push(Span::styled(text, s));
            }
            body.push(Line::from(title));
            body.push(Line::from(""));
            let keys = self.keys();
            let ki = self.ki();
            if keys.is_empty() {
                body.push(Line::from(dim("  (no keys — press [a] to add one)")));
            }
            let width = keys.iter().map(|k| k.chars().count()).max().unwrap_or(0).clamp(8, 32);
            let start = ki.saturating_sub(PAGE / 2).min(keys.len().saturating_sub(PAGE));
            let where_ = self.where_();
            for (j, name) in keys.iter().skip(start).take(PAGE).enumerate() {
                let i = start + j;
                let v = self.var(name).unwrap();
                let shown = self.revealed.contains(&format!("{where_}/{name}"));
                let mut spans = vec![plain(format!("{}{} ", if i == ki { "› " } else { "  " }, pad_end(name, width)))];
                let value = match &v {
                    Var::Ref(r) => {
                        spans.push(colored(format!("→ shared {} ", r.target), Color::Cyan));
                        data.shared.get(&r.target).map(|s| s.value.clone())
                    }
                    Var::Lit(l) => Some(l.value.clone()),
                };
                spans.push(match value {
                    None => colored("(missing)", Color::Red),
                    Some(v) if shown => plain(v),
                    Some(v) => dim(mask(&v, false)),
                });
                body.push(inverse(Line::from(spans), i == ki));
            }
            if keys.len() > PAGE {
                body.push(Line::from(dim(format!("  {}/{}", ki + 1, keys.len()))));
            }
        }

        let mut bottom = vec![];
        match &self.mode {
            Mode::Input { label, hidden, value, .. } => {
                let shown = if *hidden { "•".repeat(value.chars().count()) } else { value.clone() };
                bottom.push(prompt_line(label, &shown));
            }
            Mode::Typed { label, word, value, .. } => {
                bottom.push(prompt_line(&format!("{label} Type \"{word}\" to continue:"), value));
            }
            Mode::Pick { label, items, index, .. } => {
                bottom.push(Line::from(bold(label.clone())));
                let start = index.saturating_sub(PICK_PAGE / 2).min(items.len().saturating_sub(PICK_PAGE));
                for (j, item) in items.iter().skip(start).take(PICK_PAGE).enumerate() {
                    let on = start + j == *index;
                    bottom.push(inverse(Line::from(format!("{}{item}", if on { "› " } else { "  " })), on));
                }
                bottom.push(Line::from(dim("  (↑↓ choose, Enter to pick, Esc to cancel)")));
            }
            Mode::YesNo { label, .. } => bottom.push(Line::from(vec![bold(label.clone()), plain(" "), dim("[y/N]")])),
            Mode::Browse => match &self.message {
                Some(m) if m.ok => bottom.push(Line::from(colored(format!("✓ {}", m.text), Color::Green))),
                Some(m) => bottom.push(Line::from(colored(format!("✗ {}", m.text), Color::Red))),
                None => bottom.push(Line::from(" ")),
            },
        }
        bottom.push(Line::from(""));
        let help: Vec<Vec<(&str, &str)>> = if self.view == View::Apps {
            let mut row = vec![("↑↓", "choose"), ("⏎", "open")];
            if self.can_init {
                row.push(("i", "set up this repo"));
            }
            row.push(("q", "quit"));
            vec![row]
        } else if self.is_shared() {
            vec![
                vec![("a", "add"), ("e", "edit"), ("d", "delete"), ("v", "show/hide")],
                vec![("↑↓", "key"), ("esc", "apps"), ("q", "quit")],
            ]
        } else {
            vec![
                vec![("a", "add"), ("r", "add shared"), ("e", "edit"), ("d", "delete"), ("s", "share"), ("v", "show/hide")],
                vec![("u", "use env"), ("A", "new env"), ("R", "rename env"), ("P", "protect"), ("D", "delete env")],
                vec![("←→", "env"), ("↑↓", "key"), ("esc", "apps"), ("q", "quit")],
            ]
        };
        for row in help {
            let mut spans = vec![];
            for (j, (k, label)) in row.iter().enumerate() {
                if j > 0 {
                    spans.push(plain("   "));
                }
                spans.push(Span::styled(format!("[{k}]"), style().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
                spans.push(plain(" "));
                spans.push(dim(label.to_string()));
            }
            bottom.push(Line::from(spans));
        }
        (top, body, bottom)
    }

    pub fn render(&self, frame: &mut Frame) {
        let (top, body, bottom) = self.lines();
        let area = frame.area();
        let x = area.x + 1;
        let w = area.width.saturating_sub(2);
        let mut y = area.y;
        let put = |frame: &mut Frame, lines: Vec<Line<'static>>, y: &mut u16| {
            let h = (lines.len() as u16).min(area.height.saturating_sub(*y));
            frame.render_widget(Paragraph::new(lines), Rect::new(x, *y, w, h));
            *y += h;
        };
        put(frame, top, &mut y);
        y += 1;
        let bh = (body.len() as u16 + 2).min(area.height.saturating_sub(y));
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(style().fg(Color::Gray));
        let inner = block.inner(Rect::new(x, y, w, bh));
        frame.render_widget(block, Rect::new(x, y, w, bh));
        frame.render_widget(Paragraph::new(body), Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height));
        y += bh + 1;
        put(frame, bottom, &mut y);
    }
}

fn prompt_line(label: &str, shown: &str) -> Line<'static> {
    Line::from(vec![
        bold(label.to_string()),
        plain(format!(" {shown}")),
        Span::styled(" ", style().add_modifier(Modifier::REVERSED)),
        dim("  (Enter to save, Esc to cancel)"),
    ])
}

fn app_rows(data: &Store) -> Vec<String> {
    let mut rows: Vec<String> = data.apps.keys().cloned().collect();
    rows.sort();
    rows.push(SHARED.to_string());
    rows
}
