use clap_complete::{generate_to, shells::Shell};
use std::{env, io};

include!("src/cli.rs");

fn main() -> Result<(), io::Error> {
    let outdir = match env::var_os("OUT_DIR") {
        None => return Ok(()),
        Some(outdir) => outdir,
    };

    let mut cmd = <Args as clap::CommandFactory>::command();
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        let path = generate_to(shell, &mut cmd, "solarxr-cli", &outdir)?;
        println!("cargo:warning=completion file generated: {path:?}");
    }

    Ok(())
}
