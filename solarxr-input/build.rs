use clap_complete::{generate_to, shells::Shell};
use std::env;
use std::io;

include!("src/cli.rs");

fn main() -> Result<(), io::Error> {
    if let Some(out_dir) = env::var_os("OUT_DIR") {
        let mut cmd = <Args as clap::CommandFactory>::command();
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let path = generate_to(shell, &mut cmd, "solarxr-input", &out_dir)?;
            println!("cargo:warning=completion file generated: {path:?}");
        }
    }

    Ok(())
}
