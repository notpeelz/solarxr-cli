use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

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

#[derive(serde::Deserialize, Debug)]
struct CargoMetadata {
    packages: Vec<Package>,
}

#[derive(serde::Deserialize, Debug)]
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
    input_build_out_dir: PathBuf,
    cli_exe: PathBuf,
    input_exe: PathBuf,
}

fn build<S: AsRef<str>>(args: &[S]) -> Result<BuildArtifacts> {
    let metadata = cargo_metadata();
    let cli_package_id = metadata.get_package_id("solarxr-cli")?;
    let input_package_id = metadata.get_package_id("solarxr-input")?;

    let mut cmd = std::process::Command::new(cargo_path())
        .args(
            [
                "build",
                "--message-format",
                "json-render-diagnostics",
                "-p",
                "solarxr-cli",
                "-p",
                "solarxr-input",
            ]
            .iter()
            .map(Deref::deref)
            .chain(args.iter().map(AsRef::as_ref)),
        )
        .stdout(std::process::Stdio::piped())
        .spawn()
        .wrap_err("failed to call cargo")?;

    let stdout = cmd
        .stdout
        .take()
        .ok_or_else(|| eyre!("failed to capture cargo stdout"))?;
    let reader = io::BufReader::new(stdout);

    let mut cli_build_out_dir: Option<PathBuf> = None;
    let mut input_build_out_dir: Option<PathBuf> = None;
    let mut cli_exe: Option<PathBuf> = None;
    let mut input_exe: Option<PathBuf> = None;

    let mut seen_compiler_message = false;
    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };

        let msg = match serde_json::from_str::<Message>(&line) {
            Ok(msg) => {
                seen_compiler_message = true;
                msg
            }
            Err(e) => {
                if e.is_syntax() {
                    println!("{line}");
                }
                continue;
            }
        };
        match msg {
            Message::BuildScriptExecuted(msg) if msg.package_id == cli_package_id => {
                cli_build_out_dir = Some(PathBuf::from(msg.out_dir));
            }
            Message::BuildScriptExecuted(msg) if msg.package_id == input_package_id => {
                input_build_out_dir = Some(PathBuf::from(msg.out_dir));
            }
            Message::CompilerArtifact(artifact) if artifact.target.name == "solarxr-cli" => {
                if let Some(exe) = artifact.executable {
                    cli_exe = Some(PathBuf::from(exe));
                }
            }
            Message::CompilerArtifact(artifact) if artifact.target.name == "solarxr-input" => {
                if let Some(exe) = artifact.executable {
                    input_exe = Some(PathBuf::from(exe));
                }
            }
            _ => {}
        }
    }

    let status = cmd
        .wait()
        .wrap_err("failed to wait on cargo build process")?;
    if !status.success() {
        bail!("failed to build solarxr-cli");
    }

    if !seen_compiler_message {
        bail!("no compiler output");
    }

    let cli_build_out_dir = cli_build_out_dir
        .ok_or_else(|| eyre!("couldn't find solarxr-cli build script output dir"))?;
    let input_build_out_dir = input_build_out_dir
        .ok_or_else(|| eyre!("couldn't find solarxr-input build script output dir"))?;
    let cli_exe = cli_exe.ok_or_else(|| eyre!("couldn't find solarxr-cli binary"))?;
    let input_exe = input_exe.ok_or_else(|| eyre!("couldn't find solarxr-input binary"))?;

    Ok(BuildArtifacts {
        cli_build_out_dir,
        input_build_out_dir,
        cli_exe,
        input_exe,
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
    Build {
        #[arg(last = true)]
        build_args: Vec<String>,
    },
    Install {
        dest: PathBuf,
        #[arg(long = "prefix", default_value = "/usr")]
        prefix: PathBuf,
        #[arg(long = "bindir", default_value = "bin")]
        bindir: PathBuf,
        #[arg(long = "libdir", default_value = "lib")]
        libdir: PathBuf,
        #[arg(long = "datarootdir", default_value = "share")]
        datarootdir: PathBuf,
        #[arg(long = "sysconfdir", default_value = "/etc")]
        sysconfdir: PathBuf,
        #[arg(last = true)]
        build_args: Vec<String>,
    },
}

fn install_dir<P: AsRef<Path>, D: AsRef<Path>>(root: P, path: D, perm: u32) -> io::Result<()> {
    let root = root.as_ref();
    if !root.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "invalid root dir"));
    }

    let path = {
        let path = path.as_ref();
        if path.is_absolute() {
            path.strip_prefix("/").unwrap()
        } else {
            path
        }
    };

    let mut subpath = root.to_path_buf();
    for part in path.components() {
        subpath.push(part);
        match fs::create_dir(&subpath).err() {
            Some(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Some(err) => {
                return Err(err);
            }
            None => {
                fs::set_permissions(&subpath, fs::Permissions::from_mode(perm))?;
            }
        }
    }
    Ok(())
}

fn install_file<P: AsRef<Path>, D: AsRef<Path>, Q: AsRef<Path>>(
    src: P,
    root: D,
    path: Q,
    perm: u32,
) -> io::Result<()> {
    let root = root.as_ref();
    if !root.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "invalid root dir"));
    }

    let path = {
        let path = path.as_ref();
        if path.is_absolute() {
            path.strip_prefix("/").unwrap()
        } else {
            path
        }
    };

    let dest = root.join(&path);
    fs::copy(&src, &dest)?;
    fs::set_permissions(&dest, fs::Permissions::from_mode(perm))?;
    Ok(())
}

fn main() -> Result<()> {
    let args = <Args as clap::Parser>::parse();
    match args.command {
        Command::Build { build_args } => {
            build(&build_args)?;
            Ok(())
        }
        Command::Install {
            dest,
            prefix,
            bindir,
            libdir,
            datarootdir,
            sysconfdir,
            build_args,
        } => {
            match fs::create_dir(&dest).err() {
                Some(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Some(err) => {
                    return Err(err.into());
                }
                None => {}
            }

            match dest.metadata() {
                Ok(m) => {
                    if m.is_dir() {
                        let mut entries = dest
                            .read_dir()
                            .wrap_err("failed to enumerate entries in output dir")?;
                        let is_dir_empty = entries.next().is_none();
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

            let project_root = {
                let mut p = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
                p.pop();
                p
            };
            let artifacts = build(&build_args)?;

            let bin_dir = prefix.join(bindir);
            install_dir(&dest, &bin_dir, 0o755)?;
            install_file(artifacts.cli_exe, &dest, bin_dir.join("solarxr-cli"), 0o755)?;
            install_file(
                artifacts.input_exe,
                &dest,
                bin_dir.join("solarxr-input"),
                0o755,
            )?;

            let lib_dir = prefix.join(libdir);

            let share_dir = prefix.join(datarootdir);
            install_dir(&dest, &share_dir, 0o755)?;

            let bash_comp_dir = share_dir.join("bash-completion/completions");
            install_dir(&dest, &bash_comp_dir, 0o755)?;
            install_file(
                artifacts.cli_build_out_dir.join("solarxr-cli.bash"),
                &dest,
                bash_comp_dir.join("solarxr-cli"),
                0o644,
            )?;
            install_file(
                artifacts.input_build_out_dir.join("solarxr-input.bash"),
                &dest,
                bash_comp_dir.join("solarxr-input"),
                0o644,
            )?;

            let zsh_comp_dir = share_dir.join("zsh/site-functions");
            install_dir(&dest, &zsh_comp_dir, 0o755)?;
            install_file(
                artifacts.cli_build_out_dir.join("_solarxr-cli"),
                &dest,
                zsh_comp_dir.join("_solarxr_cli"),
                0o644,
            )?;
            install_file(
                artifacts.input_build_out_dir.join("_solarxr-input"),
                &dest,
                zsh_comp_dir.join("_solarxr_input"),
                0o644,
            )?;

            let fish_comp_dir = share_dir.join("fish/vendor_completions.d");
            install_dir(&dest, &fish_comp_dir, 0o755)?;
            install_file(
                artifacts.cli_build_out_dir.join("solarxr-cli.fish"),
                &dest,
                fish_comp_dir.join("solarxr-cli.fish"),
                0o644,
            )?;
            install_file(
                artifacts.input_build_out_dir.join("solarxr-input.fish"),
                &dest,
                fish_comp_dir.join("solarxr-input.fish"),
                0o644,
            )?;

            let etc_dir = prefix.join(sysconfdir);
            install_dir(&dest, &etc_dir, 0o755)?;

            let etc_xdg_input_dir = etc_dir.join("xdg/solarxr-input");
            install_dir(&dest, &etc_xdg_input_dir, 0o755)?;
            install_file(
                project_root.join("solarxr-input/config.json"),
                &dest,
                etc_xdg_input_dir.join("config.json"),
                0o644,
            )?;

            Ok(())
        }
    }
}
