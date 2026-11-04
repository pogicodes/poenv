// Creates a vault in $POENV_HOME for the release smoke test. The CLI only lets a person at a
// terminal do this (it shows the recovery phrase), so CI makes it from source, then uses the binary.
//   POENV_HOME=… cargo run --example smoke_vault
fn main() {
    let home = std::env::var_os("POENV_HOME").expect("usage: POENV_HOME=… cargo run --example smoke_vault");
    poenv::store::init_store().unwrap_or_else(|e| panic!("{e}"));
    println!("vault created in {}", home.to_string_lossy());
}
