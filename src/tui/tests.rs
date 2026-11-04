// The TUI driven key by key against a real vault in a temp folder, rendered to an in-memory screen.
use super::app::App;
use crate::envfile::{Dialect, apply_env};
use crate::project::{Project, ValuesMode, write_marker};
use crate::repos::remember_repo;
use crate::store::model::{Target, Var, create_app, resolve_env, set_ref, set_var};
use crate::store::{StoreHandle, init_store, open_store};
use crate::util::{hex, random_bytes};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

// POENV_HOME is process-wide, so these tests take turns.
static LOCK: Mutex<()> = Mutex::new(());

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("poenv-test-{}", hex(&random_bytes(6))));
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

struct Setup {
    _guard: MutexGuard<'static, ()>,
    project: Project,
    root: PathBuf,
}

impl Setup {
    fn file(&self) -> String {
        fs::read_to_string(self.root.join(".env")).unwrap()
    }
}

fn project(root: &Path, app: &str) -> Project {
    Project {
        root: root.to_string_lossy().into_owned(),
        app: app.into(),
        env: "local".into(),
        file: ".env".into(),
        format: Dialect::Dotenv,
        values: ValuesMode::File,
    }
}

/// A vault with my-app (DB in local+staging, OPENAI shared) and a repo whose .env is on local.
fn setup() -> Setup {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { std::env::set_var("POENV_HOME", temp_dir()) };
    init_store().unwrap();
    let mut store = open_store().unwrap();
    store
        .update(|s| {
            create_app(s, "my-app")?;
            set_var(s, &Target::app("my-app", "local"), "DB", "postgres://localhost/dev")?;
            set_var(s, &Target::app("my-app", "staging"), "DB", "postgres://staging-host/app")?;
            set_var(s, &Target::Shared, "OPENAI_API_KEY", "sk-shared-secret-123456")?;
            set_ref(s, "my-app", "local", "OPENAI_API_KEY", "OPENAI_API_KEY")?;
            create_app(s, "other-app")
        })
        .unwrap();
    let root = temp_dir().join("my-app");
    fs::create_dir(&root).unwrap();
    let p = project(&root, "my-app");
    write_marker(&p).unwrap();
    let vars = resolve_env(&store.data, "my-app", "local").unwrap();
    apply_env(&root.join(".env"), "my-app", "local", &vars, Dialect::Dotenv).unwrap();
    Setup { _guard: guard, project: p, root }
}

/// Another clone of my-app, known to the repo registry, with its .env on local.
fn clone(name: &str) -> PathBuf {
    let root = temp_dir().join(name);
    fs::create_dir(&root).unwrap();
    write_marker(&project(&root, "my-app")).unwrap();
    let vars = resolve_env(&open_store().unwrap().data, "my-app", "local").unwrap();
    apply_env(&root.join(".env"), "my-app", "local", &vars, Dialect::Dotenv).unwrap();
    remember_repo(&root.to_string_lossy());
    root
}

fn store() -> StoreHandle {
    open_store().unwrap()
}

fn app(project: Option<&Project>) -> App {
    App::new(store(), project.cloned(), Path::new("/tmp/somewhere"), true)
}

/// The screen as one line, whitespace collapsed (long temp paths don't matter).
fn frame(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(160, 50)).unwrap();
    t.draw(|f| app.render(f)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

enum K {
    C(char),
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Back,
}
use K::*;

fn press(app: &mut App, keys: &[K]) {
    for k in keys {
        let code = match k {
            C(c) => KeyCode::Char(*c),
            Down => KeyCode::Down,
            Left => KeyCode::Left,
            Right => KeyCode::Right,
            Enter => KeyCode::Enter,
            Esc => KeyCode::Esc,
            Back => KeyCode::Backspace,
        };
        app.handle(KeyEvent::new(code, KeyModifiers::NONE));
    }
}

fn typ(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, &[C(c)]);
    }
}

fn value(app: &App, env: &str, key: &str) -> Option<Var> {
    app.store().apps["my-app"].envs[env].vars.get(key).cloned()
}

fn lit(v: Option<Var>) -> Option<String> {
    match v {
        Some(Var::Lit(l)) => Some(l.value),
        _ => None,
    }
}

fn reference(v: Option<Var>) -> Option<String> {
    match v {
        Some(Var::Ref(r)) => Some(r.target),
        _ => None,
    }
}

#[test]
fn opens_on_this_repos_app_and_env_with_values_masked() {
    let s = setup();
    let f = frame(&app(Some(&s.project)));
    assert!(f.contains(".env is on local"));
    assert!(f.contains("● local"));
    assert!(f.contains("DB"));
    assert!(f.contains("→ shared OPENAI_API_KEY"));
    assert!(!f.contains("postgres://localhost/dev"));
    assert!(!f.contains("sk-shared-secret"));
    assert!(f.contains("[r] add shared"));
    assert!(f.contains("[u] use env"));
    assert!(!f.contains("[j")); // hjkl work but stay out of the hints
    assert!(f.contains("[esc] apps"));
}

#[test]
fn v_reveals_the_selected_value_and_hides_it_again() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('v')]);
    assert!(frame(&a).contains("postgres://localhost/dev"));
    press(&mut a, &[C('v')]);
    assert!(!frame(&a).contains("postgres://localhost/dev"));
}

#[test]
fn right_then_u_switches_the_repos_env_file() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[Right, C('u')]);
    assert!(frame(&a).contains(".env is now on staging"));
    assert!(s.file().contains("# app: my-app  env: staging"));
    assert!(s.file().contains("DB=postgres://staging-host/app"));
}

#[test]
fn switching_to_a_protected_env_needs_its_name_typed() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[Right, Right, C('u')]);
    assert!(frame(&a).contains("Type \"prod\" to continue"));
    typ(&mut a, "nope");
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("Cancelled."));
    assert!(s.file().contains("env: local"));
    press(&mut a, &[C('u')]);
    typ(&mut a, "prod");
    press(&mut a, &[Enter]);
    assert!(s.file().contains("env: prod"));
}

#[test]
fn a_adds_a_key_with_a_hidden_value_and_the_env_file_follows() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('a')]);
    typ(&mut a, "NEW_KEY");
    press(&mut a, &[Enter]);
    typ(&mut a, "s3cret-value");
    assert!(!frame(&a).contains("s3cret-value"));
    assert!(frame(&a).contains("••••••••••••"));
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("Added NEW_KEY to my-app/local"));
    assert_eq!(lit(value(&a, "local", "NEW_KEY")).as_deref(), Some("s3cret-value"));
    assert!(s.file().contains("NEW_KEY=s3cret-value"));
}

#[test]
fn e_edits_d_deletes_after_y_and_esc_cancels_a_prompt() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('e')]);
    typ(&mut a, "postgres://new/db");
    press(&mut a, &[Esc]);
    assert_eq!(lit(value(&a, "local", "DB")).as_deref(), Some("postgres://localhost/dev"));
    press(&mut a, &[C('e')]);
    typ(&mut a, "postgres://new/db");
    press(&mut a, &[Enter]);
    assert!(s.file().contains("DB=postgres://new/db"));
    press(&mut a, &[C('d')]);
    assert!(frame(&a).contains("Delete DB from my-app/local?"));
    press(&mut a, &[C('y')]);
    assert!(!s.file().contains("DB="));
}

#[test]
fn editing_a_shared_reference_points_to_the_shared_pool() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[Down, C('e')]);
    assert!(frame(&a).contains("is a shared reference"));
}

#[test]
fn s_asks_before_sharing_n_leaves_the_key_alone() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('s')]);
    assert!(frame(&a).contains("Move DB to shared?"));
    press(&mut a, &[C('n')]);
    assert!(!a.store().shared.contains_key("DB"));
}

#[test]
fn d_on_a_shared_key_that_apps_use_can_keep_a_copy_in_each() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[Esc, Down, Down, Enter, C('d')]);
    assert!(frame(&a).contains("OPENAI_API_KEY is used in my-app/local"));
    press(&mut a, &[Enter]);
    assert!(!a.store().shared.contains_key("OPENAI_API_KEY"));
    assert!(s.file().contains("OPENAI_API_KEY=sk-shared-secret-123456"));
}

#[test]
fn s_moves_a_key_into_the_shared_pool() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('s'), C('y')]);
    assert!(frame(&a).contains("DB is now shared"));
    assert_eq!(a.store().shared["DB"].value, "postgres://localhost/dev");
    assert!(frame(&a).contains("→ shared DB"));
}

#[test]
fn r_adds_a_live_reference_to_a_shared_key_optionally_renamed() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[Right, C('r')]);
    assert!(frame(&a).contains("Shared key to use in my-app/staging:"));
    assert!(frame(&a).contains("OPENAI_API_KEY"));
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("Name in my-app/staging: OPENAI_API_KEY"));
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("Added OPENAI_API_KEY to my-app/staging"));
    assert_eq!(reference(value(&a, "staging", "OPENAI_API_KEY")).as_deref(), Some("OPENAI_API_KEY"));

    // Renamed, in the env the file is on: the file follows.
    press(&mut a, &[Left, C('r'), Enter]);
    for _ in 0.."OPENAI_API_KEY".len() {
        press(&mut a, &[Back]);
    }
    typ(&mut a, "NEXT_PUBLIC_OPENAI_KEY");
    press(&mut a, &[Enter]);
    assert_eq!(reference(value(&a, "local", "NEXT_PUBLIC_OPENAI_KEY")).as_deref(), Some("OPENAI_API_KEY"));
    assert!(s.file().contains("NEXT_PUBLIC_OPENAI_KEY=sk-shared-secret-123456"));
}

#[test]
fn r_asks_before_replacing_an_existing_key_and_explains_an_empty_shared_pool() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('r'), Enter, Enter]);
    assert!(frame(&a).contains("OPENAI_API_KEY already exists in my-app/local. Replace it?"));
    press(&mut a, &[C('n')]);

    store()
        .update(|st| {
            st.shared.shift_remove("OPENAI_API_KEY");
            Ok(())
        })
        .unwrap();
    let mut other = app(Some(&s.project));
    press(&mut other, &[C('r')]);
    let f = frame(&other);
    assert!(f.contains("is empty"));
    assert!(f.contains("press s on a key to move it"));
}

#[test]
fn esc_goes_to_the_app_list_enter_opens_an_app_and_the_shared_pool_is_listed() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[Esc]);
    let f = frame(&a);
    assert!(f.contains("Apps"));
    assert!(f.contains("other-app"));
    assert!(f.contains("★ shared"));
    press(&mut a, &[Down, Down, Enter]);
    assert!(frame(&a).contains("OPENAI_API_KEY"));
    assert!(!frame(&a).contains("→ shared"));
}

#[test]
fn big_a_creates_an_environment_big_d_deletes_it_after_typing_its_name() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('A')]);
    typ(&mut a, "qa");
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("Created my-app/qa"));
    assert!(a.store().apps["my-app"].envs.contains_key("qa"));
    press(&mut a, &[C('D')]);
    typ(&mut a, "qa");
    press(&mut a, &[Enter]);
    assert!(!a.store().apps["my-app"].envs.contains_key("qa"));
}

#[test]
fn big_p_protects_and_unprotects_an_env() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('P')]);
    assert!(frame(&a).contains("Protect my-app/local?"));
    press(&mut a, &[C('y')]);
    assert!(a.store().apps["my-app"].envs["local"].protected);
    assert!(frame(&a).contains("my-app/local is now protected"));
    press(&mut a, &[C('P'), C('y')]);
    assert!(!a.store().apps["my-app"].envs["local"].protected);
}

#[test]
fn big_r_renames_the_env_and_the_env_file_and_poenv_follow_it() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('R')]);
    for _ in 0.."local".len() {
        press(&mut a, &[Back]);
    }
    typ(&mut a, "dev");
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("Renamed my-app/local to my-app/dev"));
    let envs: Vec<&String> = a.store().apps["my-app"].envs.keys().collect();
    assert_eq!(envs, ["dev", "staging", "prod"]);
    assert!(s.file().contains("env: dev"));
    assert!(fs::read_to_string(s.root.join(".poenv")).unwrap().contains("env = \"dev\""));
}

#[test]
fn hjkl_move_like_the_arrows() {
    let s = setup();
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('l'), C('u')]);
    assert!(frame(&a).contains(".env is now on staging"));
    press(&mut a, &[C('h'), C('j'), C('v')]);
    assert!(frame(&a).contains("sk-shared-secret-123456"));
}

#[test]
fn outside_a_repo_u_switches_the_selected_apps_repo_found_in_the_registry() {
    let s = setup();
    remember_repo(&s.root.to_string_lossy());
    let mut a = app(None);
    assert!(frame(&a).contains("not inside a poenv repo"));
    press(&mut a, &[Enter, C('l'), C('u')]);
    assert!(frame(&a).contains("staging (2 keys changed)"));
    assert!(s.file().contains("postgres://staging-host/app"));
}

#[test]
fn outside_a_repo_an_app_with_no_known_repo_says_how_to_fix_it() {
    let _s = setup();
    let mut a = app(None);
    press(&mut a, &[Enter, C('u')]);
    assert!(frame(&a).contains("No repo for my-app on this machine yet"));
}

#[test]
fn an_app_with_several_repos_asks_which_and_all_switches_every_one() {
    let s = setup();
    remember_repo(&s.root.to_string_lossy());
    let second = clone("my-app-2");
    let mut a = app(None);
    press(&mut a, &[Enter, C('l'), C('u')]);
    assert!(frame(&a).contains("Switch which my-app repo to staging?"));
    press(&mut a, &[Down, Down, Enter]);
    assert!(frame(&a).contains("2 my-app repos are now on staging"));
    assert!(s.file().contains("env: staging"));
    assert!(fs::read_to_string(second.join(".env")).unwrap().contains("env: staging"));
}

#[test]
fn inside_a_repo_u_switches_only_this_repo_even_when_the_app_has_other_clones() {
    let s = setup();
    let second = clone("my-app-2");
    let mut a = app(Some(&s.project));
    press(&mut a, &[C('l'), C('u')]);
    assert!(frame(&a).contains(".env is now on staging"));
    assert!(s.file().contains("env: staging"));
    assert!(fs::read_to_string(second.join(".env")).unwrap().contains("env: local"));
}

#[test]
fn switching_another_apps_repo_to_a_protected_env_still_needs_its_name_typed() {
    let s = setup();
    store().update(|st| set_var(st, &Target::app("other-app", "prod"), "K", "v")).unwrap();
    let root = temp_dir().join("other-app");
    fs::create_dir(&root).unwrap();
    write_marker(&project(&root, "other-app")).unwrap();
    remember_repo(&root.to_string_lossy());
    let mut a = app(Some(&s.project));
    press(&mut a, &[Esc, Down, Enter, C('l'), C('l'), C('u')]);
    assert!(frame(&a).contains("other-app/prod is protected"));
    typ(&mut a, "prod");
    press(&mut a, &[Enter]);
    assert!(frame(&a).contains("(1 key changed)"));
    assert!(fs::read_to_string(root.join(".env")).unwrap().contains("env: prod"));
}

#[test]
fn outside_a_repo_i_hands_off_to_poenv_init_inside_one_it_does_nothing() {
    let s = setup();
    let mut outside = app(None);
    let f = frame(&outside);
    assert!(f.contains("isn't set up for poenv"));
    assert!(f.contains("[i] set up this repo"));
    press(&mut outside, &[C('i')]);
    assert!(outside.want_init && outside.quit);

    let mut inside = app(Some(&s.project));
    assert!(!frame(&inside).contains("poenv init"));
    press(&mut inside, &[C('i')]);
    assert!(!inside.want_init);
}

#[test]
fn write_marker_round_trips() {
    let _s = setup();
    let root = temp_dir();
    write_marker(&project(&root, "a")).unwrap();
    assert!(fs::read_to_string(root.join(".poenv")).unwrap().contains("app = \"a\""));
}

