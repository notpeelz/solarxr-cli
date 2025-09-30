use std::{
    env, fs,
    io::{self, BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use eyre::{Context, Result, bail, eyre};

#[derive(serde::Deserialize, Debug)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum Message {
    CompilerArtifact(Artifact),
    BuildScriptExecuted(BuildScriptExecution),
    #[serde(other)]
    Unknown,
}

#[derive(serde::Deserialize, Debug)]
struct Artifact {
    target: ArtifactTarget,
    executable: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
struct ArtifactTarget {
    name: String,
}

#[derive(serde::Deserialize, Debug)]
struct BuildScriptExecution {
    package_id: String,
    out_dir: String,
}

#[derive(serde::Deserialize)]
struct CargoMetadata {
    packages: Vec<Package>,
}

#[derive(serde::Deserialize)]
struct Package {
    name: String,
    id: String,
}

fn cargo_path() -> &'static str {
    static CARGO_PATH: LazyLock<String> =
        LazyLock::new(|| env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    &CARGO_PATH
}

fn cargo_metadata() -> CargoMetadata {
    let output = std::process::Command::new(cargo_path())
        .args(["metadata", "--format-version", "1"])
        .stdout(std::process::Stdio::piped())
        .output()
        .expect("failed to run cargo metadata");
    serde_json::from_slice::<CargoMetadata>(&output.stdout).expect("invalid cargo metadata JSON")
}

impl CargoMetadata {
    fn get_package_id(&self, name: &str) -> Result<&str> {
        self.packages
            .iter()
            .find(|pkg| pkg.name == name)
            .map(|x| x.id.as_str())
            .ok_or_else(|| eyre!("could not find package {name} in cargo metadata"))
    }
}

struct BuildArtifacts {
    cli_build_out_dir: PathBuf,
    cli_exe: PathBuf,
}

fn build() -> Result<BuildArtifacts> {
    let metadata = cargo_metadata();
    let package_id = metadata.get_package_id("solarxr-cli")?;

    let mut cmd = std::process::Command::new(cargo_path())
        .args([
            "build",
            "--release",
            "--message-format",
            "json-render-diagnostics",
            "-p",
            "solarxr-cli",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .wrap_err("failed to call cargo")?;

    let stdout = cmd
        .stdout
        .take()
        .ok_or_else(|| eyre!("failed to capture cargo stdout"))?;
    let reader = BufReader::new(stdout);

    let mut cli_build_out_dir: Option<PathBuf> = None;
    let mut cli_exe: Option<PathBuf> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };

        match serde_json::from_str::<Message>(&line) {
            Ok(Message::BuildScriptExecuted(msg)) if msg.package_id == package_id => {
                cli_build_out_dir = Some(PathBuf::from(msg.out_dir));
            }
            Ok(Message::CompilerArtifact(artifact)) if artifact.target.name == "solarxr-cli" => {
                if let Some(exe) = artifact.executable {
                    cli_exe = Some(PathBuf::from(exe));
                }
            }
            Ok(_) => {}
            Err(e) => {
                if e.is_syntax() {
                    println!("{line}");
                }
            }
        }
    }

    let status = cmd
        .wait()
        .wrap_err("failed to wait on cargo build process")?;
    if !status.success() {
        return Err(eyre!("failed to build solarxr-cli"));
    }

    let cli_build_out_dir = cli_build_out_dir
        .ok_or_else(|| eyre!("couldn't find solarxr-cli build script output dir"))?;
    let cli_exe = cli_exe.ok_or_else(|| eyre!("couldn't find solarxr-cli binary"))?;

    Ok(BuildArtifacts {
        cli_build_out_dir,
        cli_exe,
    })
}

#[derive(clap::Parser)]
#[command()]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    Build,
    Install { dest: PathBuf },
}

fn install_dir<P: AsRef<Path>>(path: P, perm: u32) -> io::Result<()> {
    fs::create_dir_all(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(perm))?;
    Ok(())
}

fn install_file<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dest: Q, perm: u32) -> io::Result<()> {
    fs::copy(&src, &dest)?;
    fs::set_permissions(&dest, fs::Permissions::from_mode(perm))?;
    Ok(())
}

fn main() -> Result<()> {
    let args = <Args as clap::Parser>::parse();
    match args.command {
        Command::Build => {
            build()?;
            Ok(())
        }
        Command::Install { dest } => {
            match dest.metadata() {
                Ok(m) => {
                    if m.is_dir() {
                        let is_dir_empty = dest.read_dir().iter().next().is_none();
                        if !is_dir_empty {
                            bail!("output dir isn't empty");
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(_) => {
                    bail!("failed to query metadata of build dir");
                }
            }

            let build_artifacts = build()?;

            let bin_dir = dest.join("bin");
            install_dir(&bin_dir, 0o755)?;
            install_file(build_artifacts.cli_exe, bin_dir.join("solarxr-cli"), 0o755)?;

            let share_dir = dest.join("share");
            install_dir(&share_dir, 0o755)?;

            let bash_completions_dir = share_dir.join("bash-completion/completions");
            install_dir(&bash_completions_dir, 0o755)?;
            install_file(
                build_artifacts.cli_build_out_dir.join("solarxr-cli.bash"),
                bash_completions_dir.join("solarxr-cli"),
                0o644,
            )?;

            let zsh_completions_dir = share_dir.join("zsh/site-functions");
            install_dir(&zsh_completions_dir, 0o755)?;
            install_file(
                build_artifacts.cli_build_out_dir.join("_solarxr-cli"),
                zsh_completions_dir.join("_solarxr_cli"),
                0o644,
            )?;

            let fish_completions_dir = share_dir.join("fish/vendor_completions.d");
            install_dir(&fish_completions_dir, 0o755)?;
            install_file(
                build_artifacts.cli_build_out_dir.join("solarxr-cli.fish"),
                fish_completions_dir.join("solarxr-cli.fish"),
                0o644,
            )?;

            Ok(())
        }
    }
}
