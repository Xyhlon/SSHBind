#[cfg(any(target_os = "linux", target_os = "windows"))]
mod cli; // put real CLI in src/real.rs

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    cli::main();
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    println!("This program only runs on Linux and Windows.");
}
